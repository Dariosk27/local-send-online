//! Data plane: the `/lso/transfer/1.0.0` stream protocol.
//!
//! Runs only on direct connections (enforced by `DirectOnly`). Wire format,
//! all integers big-endian:
//!
//! ```text
//! S->R  "LSO1" | u32 len | JSON Offer {files:[{name,size}], from}
//! R->S  u32 len | JSON Answer {accept, offsets:[u64], reason, name?}
//! for each file i:
//!   S->R  raw bytes [offsets[i] .. size)
//!   S->R  32-byte BLAKE3 of the *whole* file
//!   R->S  1 byte: 0 = hash verified and file stored, 1 = mismatch
//! ```
//!
//! `offsets` lets an interrupted transfer resume: the receiver keeps a
//! `.lso-part` file and tells the sender how much it already has. The final
//! hash always covers the whole file, so a resumed prefix is verified too.
//! Progress is reported from real byte counts: on the receiver after the
//! bytes are written to disk, on the sender after the transport accepted them
//! (QUIC/yamux flow control bounds how far that can run ahead of the peer).
//! The sender only reports success after the receiver's hash verdict.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use libp2p::{PeerId, Stream};
use serde::{Deserialize, Serialize};
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::config::{PULL_PROTOCOL, TRANSFER_PROTOCOL};
use libp2p::StreamProtocol;

const MAGIC: &[u8; 4] = b"LSO1";
const CHUNK: usize = 256 * 1024;
const MAX_HEADER: u32 = 1024 * 1024;
const MAX_FILES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub files: Vec<FileEntry>,
    pub from: String,
}

impl Offer {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Answer {
    accept: bool,
    offsets: Vec<u64>,
    reason: Option<String>,
    /// Receiver's device name (optional, added after v0.1.0).
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Progress {
    /// Starting file `index`; `offset` bytes were already present (resume).
    FileStart {
        index: usize,
        name: String,
        size: u64,
        offset: u64,
    },
    /// Cumulative bytes of the current file that are done.
    Bytes {
        index: usize,
        done: u64,
    },
    FileVerified {
        index: usize,
        path: Option<PathBuf>,
    },
}

/// Sends `paths` to `peer` over an already established direct connection.
pub async fn send_files(
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    paths: &[PathBuf],
    from: &str,
    progress: impl FnMut(Progress),
) -> Result<()> {
    send_files_named(control, peer, paths, from, progress)
        .await
        .map(|_| ())
}

/// Like [`send_files`], and returns the name the receiver announced, if any.
pub async fn send_files_named(
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    paths: &[PathBuf],
    from: &str,
    mut progress: impl FnMut(Progress),
) -> Result<Option<String>> {
    let mut files = Vec::new();
    for p in paths {
        let meta = fs::metadata(p)
            .await
            .with_context(|| format!("{}", p.display()))?;
        ensure!(
            meta.is_file(),
            "{} is not a regular file (directories are not supported yet)",
            p.display()
        );
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .context("file name is not valid UTF-8")?;
        files.push(FileEntry {
            name: name.to_string(),
            size: meta.len(),
        });
    }
    let offer = Offer {
        files,
        from: from.to_string(),
    };

    let mut s = open_with_retry(control, peer, TRANSFER_PROTOCOL)
        .await?
        .compat();

    s.write_all(MAGIC).await?;
    write_json(&mut s, &offer).await?;
    s.flush().await?;

    let answer: Answer = read_json(&mut s)
        .await
        .context("receiver closed before answering")?;
    if !answer.accept {
        bail!(
            "rifiutato dal ricevente: {}",
            answer.reason.unwrap_or_default()
        );
    }
    ensure!(
        answer.offsets.len() == offer.files.len(),
        "protocol error: offsets"
    );

    let mut buf = vec![0u8; CHUNK];
    for (index, (path, entry)) in paths.iter().zip(&offer.files).enumerate() {
        let offset = answer.offsets[index];
        ensure!(
            offset <= entry.size,
            "protocol error: offset beyond file size"
        );
        progress(Progress::FileStart {
            index,
            name: entry.name.clone(),
            size: entry.size,
            offset,
        });

        let mut file = fs::File::open(path).await?;
        let mut hasher = blake3::Hasher::new();
        // The hash covers the whole file, including the part the receiver
        // already has from an earlier attempt.
        hash_prefix(&mut file, offset, &mut hasher, &mut buf).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;

        let mut done = offset;
        while done < entry.size {
            let want = CHUNK.min((entry.size - done) as usize);
            let n = file.read(&mut buf[..want]).await?;
            ensure!(n > 0, "{} shrank while sending", path.display());
            hasher.update(&buf[..n]);
            s.write_all(&buf[..n]).await?;
            done += n as u64;
            progress(Progress::Bytes { index, done });
        }
        s.write_all(hasher.finalize().as_bytes()).await?;
        s.flush().await?;

        let verdict = s
            .read_u8()
            .await
            .context("connection lost waiting for verification")?;
        ensure!(
            verdict == 0,
            "il ricevente segnala hash non corrispondente per {}",
            entry.name
        );
        progress(Progress::FileVerified { index, path: None });
    }
    s.shutdown().await.ok();
    Ok(answer.name)
}

/// Right after hole punching there can be several direct connections (TCP and
/// QUIC, one per direction) and the redundant ones get closed; a stream
/// requested on a connection that is being closed fails. Nothing has been
/// sent yet at that point, so retrying is safe.
async fn open_with_retry(
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    protocol: StreamProtocol,
) -> Result<Stream> {
    let mut last = None;
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            control.open_stream(peer, protocol.clone()),
        )
        .await
        {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(libp2p_stream::OpenStreamError::UnsupportedProtocol(p))) => {
                bail!("il peer non supporta {p}")
            }
            Ok(Err(e)) => last = Some(e.to_string()),
            Err(_) => last = Some("timeout".into()),
        }
    }
    Err(anyhow!(
        "cannot open transfer stream: {}",
        last.unwrap_or_default()
    ))
}

// ------------------------------------------------------------ short codes
//
// `/lso/pull/1.0.0`, on a direct connection:
//   R->S  "LSOP" | u32 len | JSON {code, name}
//   S->R  1 byte: 1 = code valid, the sharer now opens a normal transfer
//         stream towards R; 0 = unknown or expired code.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub code: String,
    pub name: String,
}

/// Receiver side: "I have your code, send me the files".
pub async fn request_pull(
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    code: &str,
    name: &str,
) -> Result<()> {
    let mut s = open_with_retry(control, peer, PULL_PROTOCOL)
        .await?
        .compat();
    s.write_all(b"LSOP").await?;
    write_json(
        &mut s,
        &PullRequest {
            code: code.into(),
            name: name.into(),
        },
    )
    .await?;
    s.flush().await?;
    let ok = s
        .read_u8()
        .await
        .context("no answer from the sharing device")?;
    s.shutdown().await.ok();
    ensure!(ok == 1, "codice scaduto o già usato");
    Ok(())
}

pub struct PullResponder {
    s: tokio_util::compat::Compat<Stream>,
}

impl PullResponder {
    pub async fn answer(mut self, ok: bool) -> Result<()> {
        self.s.write_u8(ok as u8).await?;
        self.s.flush().await?;
        self.s.shutdown().await.ok();
        Ok(())
    }
}

/// Sharer side: reads a pull request.
pub async fn read_pull(stream: Stream) -> Result<(PullRequest, PullResponder)> {
    let mut s = stream.compat();
    let mut magic = [0u8; 4];
    s.read_exact(&mut magic).await?;
    ensure!(&magic == b"LSOP", "not a lso pull request");
    let req: PullRequest = read_json(&mut s).await?;
    Ok((req, PullResponder { s }))
}

/// Reads the offer at the start of an inbound transfer stream.
pub async fn read_offer(stream: Stream) -> Result<(Offer, IncomingTransfer)> {
    let mut s = stream.compat();
    let mut magic = [0u8; 4];
    s.read_exact(&mut magic).await?;
    ensure!(&magic == MAGIC, "not a lso transfer");
    let offer: Offer = read_json(&mut s).await?;
    ensure!(offer.files.len() <= MAX_FILES, "too many files");
    for f in &offer.files {
        sanitize_name(&f.name)?;
    }
    Ok((offer.clone(), IncomingTransfer { s, offer }))
}

pub struct IncomingTransfer {
    s: tokio_util::compat::Compat<Stream>,
    offer: Offer,
}

impl IncomingTransfer {
    pub async fn reject(mut self, reason: &str) -> Result<()> {
        let a = Answer {
            accept: false,
            offsets: vec![],
            reason: Some(reason.into()),
            name: None,
        };
        write_json(&mut self.s, &a).await?;
        self.s.shutdown().await.ok();
        Ok(())
    }

    /// Accepts the offer and stores the files in `dest`. Returns final paths.
    pub async fn accept(
        self,
        peer: PeerId,
        dest: &Path,
        progress: impl FnMut(Progress),
    ) -> Result<Vec<PathBuf>> {
        self.accept_as(peer, dest, None, progress).await
    }

    /// Like [`Self::accept`], telling the sender our device name.
    pub async fn accept_as(
        mut self,
        peer: PeerId,
        dest: &Path,
        name: Option<&str>,
        mut progress: impl FnMut(Progress),
    ) -> Result<Vec<PathBuf>> {
        fs::create_dir_all(dest).await?;
        let tag: String = peer.to_base58().chars().rev().take(8).collect();
        let mut parts = Vec::new();
        let mut offsets = Vec::new();
        for f in &self.offer.files {
            let part = dest.join(format!(
                ".{}.{}.{tag}.lso-part",
                sanitize_name(&f.name)?,
                f.size
            ));
            let have = fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
            let offset = if have <= f.size { have } else { 0 };
            if have > f.size {
                fs::remove_file(&part).await.ok();
            }
            parts.push(part);
            offsets.push(offset);
        }
        let answer = Answer {
            accept: true,
            offsets: offsets.clone(),
            reason: None,
            name: name.map(str::to_string),
        };
        write_json(&mut self.s, &answer).await?;
        self.s.flush().await?;

        let mut buf = vec![0u8; CHUNK];
        let mut stored = Vec::new();
        for (index, f) in self.offer.files.iter().enumerate() {
            let offset = offsets[index];
            progress(Progress::FileStart {
                index,
                name: f.name.clone(),
                size: f.size,
                offset,
            });
            let mut hasher = blake3::Hasher::new();
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(&parts[index])
                .await?;
            if offset > 0 {
                let mut r = fs::File::open(&parts[index]).await?;
                hash_prefix(&mut r, offset, &mut hasher, &mut buf).await?;
            }
            let mut done = offset;
            while done < f.size {
                let want = CHUNK.min((f.size - done) as usize);
                let n = self.s.read(&mut buf[..want]).await?;
                if n == 0 {
                    file.flush().await?;
                    bail!(
                        "connessione interrotta a {done}/{} byte di {}; il parziale è conservato \
                         e il prossimo invio riprenderà da qui",
                        f.size,
                        f.name
                    );
                }
                file.write_all(&buf[..n]).await?;
                hasher.update(&buf[..n]);
                done += n as u64;
                progress(Progress::Bytes { index, done });
            }
            file.flush().await?;
            file.sync_all().await?;
            drop(file);

            let mut expected = [0u8; 32];
            self.s.read_exact(&mut expected).await?;
            if hasher.finalize().as_bytes() != &expected {
                fs::remove_file(&parts[index]).await.ok();
                self.s.write_u8(1).await?;
                self.s.flush().await?;
                bail!(
                    "hash BLAKE3 non corrispondente per {}: file scartato",
                    f.name
                );
            }
            let final_path = unique_path(dest, &sanitize_name(&f.name)?).await;
            fs::rename(&parts[index], &final_path).await?;
            self.s.write_u8(0).await?;
            self.s.flush().await?;
            progress(Progress::FileVerified {
                index,
                path: Some(final_path.clone()),
            });
            stored.push(final_path);
        }
        // Wait for the sender to close so its last verdict is not cut off.
        let _ = self.s.read(&mut buf[..1]).await;
        Ok(stored)
    }
}

async fn hash_prefix(
    file: &mut fs::File,
    len: u64,
    hasher: &mut blake3::Hasher,
    buf: &mut [u8],
) -> Result<()> {
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let mut left = len;
    while left > 0 {
        let n = file.read(&mut buf[..CHUNK.min(left as usize)]).await?;
        ensure!(n > 0, "file shorter than expected");
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(())
}

/// Only a plain file name is accepted from the network: no separators, no
/// `..`, no control characters. Prevents path traversal on the receiver.
pub fn sanitize_name(name: &str) -> Result<String> {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    ensure!(
        !clean.is_empty() && clean.len() <= 255,
        "invalid file name {name:?}"
    );
    Ok(clean)
}

async fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if fs::metadata(&candidate).await.is_err() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 1.. {
        let c = dir.join(format!("{stem} ({i}){ext}"));
        if fs::metadata(&c).await.is_err() {
            return c;
        }
    }
    unreachable!()
}

async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, v: &T) -> Result<()> {
    let bytes = serde_json::to_vec(v)?;
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    Ok(())
}

async fn read_json<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T> {
    let len = r.read_u32().await?;
    ensure!(len <= MAX_HEADER, "header too large");
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("../../etc/passwd").unwrap(), "_.._etc_passwd");
        assert_eq!(sanitize_name("..\\x").unwrap(), "_x");
        assert_eq!(sanitize_name("foto.jpg").unwrap(), "foto.jpg");
        assert!(sanitize_name("..").is_err());
        assert!(sanitize_name("").is_err());
    }
}
