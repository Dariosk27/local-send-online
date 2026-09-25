//! Desktop shell for Local Send Online.
//!
//! The UI (app/ui) is plain HTML/CSS/JS; everything network-related is the
//! same `lso-core` used by the CLI. This file only adapts it to Tauri:
//! commands the UI calls, and events the UI listens to.
//!
//! Two ways to reach someone:
//! * short code: the sender shares files under a code like `7F3K-9H2P`
//!   (valid 5 minutes, single use); the receiver types it and pulls;
//! * recent devices: known PeerIds, found again through the DHT.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lso_core::{
    config::{self, PULL_PROTOCOL, TRANSFER_PROTOCOL},
    identity,
    node::{describe_hint, ConnectFailure, NatHint, Node, NodeConfig, NodeEvent, Role},
    rendezvous,
    ticket::{self, Ticket},
    transfer::{self, Progress},
    Multiaddr, PeerId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::oneshot;

struct App {
    node: Node,
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    contacts: Mutex<Vec<Contact>>,
    pending_offers: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    shares: Mutex<HashMap<String, Share>>,
    running: Mutex<HashMap<u64, tauri::async_runtime::JoinHandle<()>>>,
    next_id: AtomicU64,
}

struct Share {
    id: u64,
    files: Vec<PathBuf>,
    expires: Instant,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    device_name: String,
    download_dir: PathBuf,
    auto_accept_contacts: bool,
    notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let download_dir = directories::UserDirs::new()
            .map(|u| {
                u.download_dir()
                    .map(|d| d.to_path_buf())
                    .unwrap_or_else(|| u.home_dir().join("Downloads"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("LocalSendOnline");
        Settings {
            device_name: whoami::devicename(),
            download_dir,
            auto_accept_contacts: false,
            notifications: true,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Contact {
    peer_id: String,
    name: String,
    /// Last ticket seen for this peer: its addresses are hints, the PeerId is
    /// what matters (the DHT is asked for fresh addresses anyway).
    ticket: Option<String>,
    last_seen: u64,
}

#[derive(Serialize)]
struct Status {
    peer_id: String,
    ticket: String,
    /// "connecting" | "online" | "limited" | "offline"
    state: &'static str,
    reservations: usize,
    observed: Vec<String>,
    dht_peers: usize,
    hints: Vec<String>,
    symmetric_nat: bool,
    /// Local UDP/TCP port (for port forwarding on the home router).
    port: Option<u16>,
    ipv6: bool,
}

#[derive(Serialize, Clone)]
struct FileInfo {
    name: String,
    size: u64,
}

#[derive(Serialize, Clone)]
struct OfferEvent {
    id: u64,
    peer_id: String,
    from: String,
    files: Vec<FileInfo>,
    total: u64,
    known_contact: bool,
}

/// Everything the transfer screen needs.
#[derive(Serialize, Clone)]
struct TransferEvent {
    id: u64,
    direction: &'static str, // "in" | "out"
    /// "searching" | "connecting" | "waiting" | "transferring" | "verifying"
    /// | "done" | "failed" | "rejected"
    stage: &'static str,
    peer: String,
    files: Vec<FileInfo>,
    total: u64,
    done: u64,
    file_index: usize,
    /// Plain-language explanation shown to the user.
    message: Option<String>,
    /// Technical details (collapsed in the UI).
    detail: Option<String>,
    via: Option<String>,
    /// Where received files were stored.
    folder: Option<String>,
}

/// Another device reached us (or tried to): shown on the code screen.
#[derive(Serialize, Clone)]
struct PeerEvent {
    peer_id: String,
    name: Option<String>,
    /// "reached" | "direct" | "failed"
    kind: &'static str,
    message: Option<String>,
    detail: Option<String>,
}

#[derive(Serialize, Clone)]
struct CodeEvent {
    code: String,
    /// "searching" | "connecting" | "requesting" | "failed"
    stage: &'static str,
    message: Option<String>,
    detail: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short(p: &str) -> String {
    format!("…{}", &p[p.len().saturating_sub(6)..])
}

impl App {
    fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        let _ = std::fs::write(
            self.data_dir.join("settings.json"),
            serde_json::to_vec_pretty(&s).unwrap(),
        );
    }

    fn save_contacts(&self) {
        let c = self.contacts.lock().unwrap().clone();
        let _ = std::fs::write(
            self.data_dir.join("contacts.json"),
            serde_json::to_vec_pretty(&c).unwrap(),
        );
    }

    fn remember(&self, peer: &PeerId, name: Option<&str>, ticket: Option<String>) {
        let id = peer.to_base58();
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        {
            let mut contacts = self.contacts.lock().unwrap();
            match contacts.iter_mut().find(|c| c.peer_id == id) {
                Some(c) => {
                    if let Some(n) = name {
                        c.name = n.to_string();
                    }
                    if ticket.is_some() {
                        c.ticket = ticket;
                    }
                    c.last_seen = now();
                }
                None => contacts.push(Contact {
                    name: name
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Dispositivo {}", short(&id))),
                    peer_id: id,
                    ticket,
                    last_seen: now(),
                }),
            }
            contacts.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        }
        self.save_contacts();
    }

    fn contact_name(&self, peer: &PeerId) -> Option<String> {
        let id = peer.to_base58();
        self.contacts
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.peer_id == id)
            .map(|c| c.name.clone())
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

fn notify(handle: &AppHandle, app: &App, title: &str, body: &str) {
    if !app.settings.lock().unwrap().notifications {
        return;
    }
    let focused = handle
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if !focused {
        let _ = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

fn file_infos(paths: &[PathBuf]) -> Result<Vec<FileInfo>, String> {
    paths
        .iter()
        .map(|p| {
            let m = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
            if !m.is_file() {
                return Err(format!(
                    "{} non è un file (le cartelle non sono ancora supportate)",
                    p.display()
                ));
            }
            Ok(FileInfo {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                size: m.len(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------- commands

#[tauri::command]
async fn status(app: State<'_, Arc<App>>) -> Result<Status, String> {
    let s = app.node.snapshot().await.map_err(|e| e.to_string())?;
    let observed = !s.observed_addrs.is_empty() || !s.external_addrs.is_empty();
    let state = if !s.reservations.is_empty() {
        "online"
    } else if observed {
        "limited"
    } else if s.routing_peers > 0 {
        "connecting"
    } else {
        "offline"
    };
    Ok(Status {
        peer_id: app.node.peer_id().to_base58(),
        ticket: s.ticket().encode(),
        state,
        reservations: s.reservations.len(),
        observed: s.observed_addrs.iter().map(|a| a.to_string()).collect(),
        dht_peers: s.routing_peers,
        symmetric_nat: s
            .hints
            .iter()
            .any(|h| matches!(h, NatHint::SymmetricNat { .. })),
        hints: s.hints.iter().map(describe_hint).collect(),
        port: s.listen_addrs.iter().find_map(|a| {
            a.iter().find_map(|p| match p {
                lso_core::node::MultiaddrProtocol::Udp(port) => Some(port),
                _ => None,
            })
        }),
        ipv6: s
            .external_addrs
            .iter()
            .any(|a| a.to_string().starts_with("/ip6/2") || a.to_string().starts_with("/ip6/3")),
    })
}

#[tauri::command]
fn settings(app: State<'_, Arc<App>>) -> Settings {
    app.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: State<'_, Arc<App>>, settings: Settings) {
    {
        let mut s = app.settings.lock().unwrap();
        let name = settings.device_name.trim();
        if !name.is_empty() {
            s.device_name = name.chars().take(64).collect();
        }
        if !settings.download_dir.as_os_str().is_empty() {
            s.download_dir = settings.download_dir;
        }
        s.auto_accept_contacts = settings.auto_accept_contacts;
        s.notifications = settings.notifications;
    }
    app.save_settings();
}

#[tauri::command]
fn contacts(app: State<'_, Arc<App>>) -> Vec<Contact> {
    app.contacts.lock().unwrap().clone()
}

#[tauri::command]
fn forget_contact(app: State<'_, Arc<App>>, peer_id: String) {
    app.contacts
        .lock()
        .unwrap()
        .retain(|c| c.peer_id != peer_id);
    app.save_contacts();
}

#[tauri::command]
fn open_download_dir(app: State<'_, Arc<App>>) -> Result<(), String> {
    let dir = app.settings.lock().unwrap().download_dir.clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    reveal(&dir)
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    reveal(&PathBuf::from(path))
}

fn reveal(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let r = if path.is_file() {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else {
        std::process::Command::new("open").arg(path).spawn()
    };
    #[cfg(target_os = "windows")]
    let r = if path.is_file() {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("explorer").arg(path).spawn()
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let r = std::process::Command::new("xdg-open")
        .arg(if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        })
        .spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn file_sizes(paths: Vec<String>) -> Vec<Option<u64>> {
    paths
        .iter()
        .map(|p| {
            std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
        })
        .collect()
}

#[tauri::command]
fn answer_offer(app: State<'_, Arc<App>>, id: u64, accept: bool) {
    if let Some(tx) = app.pending_offers.lock().unwrap().remove(&id) {
        let _ = tx.send(accept);
    }
}

/// Stops a running transfer. The stream is dropped, the other side sees an
/// interruption; a partial download is kept for resuming.
#[tauri::command]
fn cancel(handle: AppHandle, app: State<'_, Arc<App>>, id: u64) {
    if let Some(task) = app.running.lock().unwrap().remove(&id) {
        task.abort();
        let _ = handle.emit("cancelled", id);
    }
}

#[derive(Serialize)]
struct ShareInfo {
    id: u64,
    code: String,
    expires_in: u64,
}

/// Starts sharing files under a new short code.
#[tauri::command]
async fn start_share(app: State<'_, Arc<App>>, paths: Vec<String>) -> Result<ShareInfo, String> {
    let files: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if files.is_empty() {
        return Err("Nessun file selezionato".into());
    }
    file_infos(&files)?;
    let code = rendezvous::generate_code();
    let id = app.next_id();
    app.shares.lock().unwrap().insert(
        code.clone(),
        Share {
            id,
            files,
            expires: Instant::now() + config::CODE_TTL,
        },
    );
    app.node.provide(rendezvous::key_for_code(&code)).await;

    let node = app.node.clone();
    let app2 = app.inner().clone();
    let code2 = code.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(config::CODE_TTL).await;
        if app2.shares.lock().unwrap().remove(&code2).is_some() {
            node.stop_providing(rendezvous::key_for_code(&code2)).await;
        }
    });
    Ok(ShareInfo {
        id,
        code,
        expires_in: config::CODE_TTL.as_secs(),
    })
}

#[tauri::command]
async fn stop_share(app: State<'_, Arc<App>>, code: String) -> Result<(), String> {
    if app.shares.lock().unwrap().remove(&code).is_some() {
        app.node
            .stop_providing(rendezvous::key_for_code(&code))
            .await;
    }
    Ok(())
}

/// Sends files straight to a known device (or a pasted long code).
#[tauri::command]
async fn send_to(
    handle: AppHandle,
    app: State<'_, Arc<App>>,
    target: String,
    paths: Vec<String>,
) -> Result<u64, String> {
    let target = resolve_target(&app, target.trim())?;
    let files: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let infos = file_infos(&files)?;
    let id = app.next_id();
    let app = app.inner().clone();
    let peer = target.peer;
    let name = app.contact_name(&peer);
    spawn_tracked(
        &app.clone(),
        id,
        run_send(handle, app, id, peer, Some(target), name, files, infos),
    );
    Ok(id)
}

/// Receiver side of a short code: find the sharer, connect, ask it to send.
/// Progress is reported with `code` events; the files then arrive as a
/// normal incoming offer.
#[tauri::command]
async fn receive_code(
    handle: AppHandle,
    app: State<'_, Arc<App>>,
    code: String,
) -> Result<String, String> {
    let code = rendezvous::normalize_code(&code)
        .ok_or("Codice non valido: sono 8 caratteri, ad esempio 7F3K-9H2P")?;
    let app = app.inner().clone();
    let c = code.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |stage, message: Option<String>, detail: Option<String>| {
            let _ = handle.emit(
                "code",
                CodeEvent {
                    code: c.clone(),
                    stage,
                    message,
                    detail,
                },
            );
        };
        emit("searching", None, None);
        let _ = app.node.wait_ready(false, Duration::from_secs(20)).await;
        let Some(peer) = app
            .node
            .find_provider(rendezvous::key_for_code(&c), Duration::from_secs(90))
            .await
        else {
            emit(
                "failed",
                Some("Nessun dispositivo trovato con questo codice. Controlla di averlo scritto bene e che non sia scaduto (vale 5 minuti).".into()),
                None,
            );
            return;
        };
        emit("connecting", None, None);
        if let Err(f) = app
            .node
            .connect_direct(Ticket {
                peer,
                addrs: vec![],
            })
            .await
        {
            emit("failed", Some(friendly_failure(&f)), Some(f.explain()));
            return;
        }
        emit("requesting", None, None);
        let name = app.settings.lock().unwrap().device_name.clone();
        let mut control = app.node.control();
        if let Err(e) = transfer::request_pull(&mut control, peer, &c, &name).await {
            emit(
                "failed",
                Some("Il codice non è più valido: chiedi un codice nuovo.".into()),
                Some(format!("{e:#}")),
            );
        }
        // Next: the sharer opens a transfer; it shows up as an offer.
    });
    Ok(code)
}

fn resolve_target(app: &App, target: &str) -> Result<Ticket, String> {
    // A contact's PeerId: reuse its last ticket as address hints.
    if !target.starts_with("lso1") {
        let hint = app
            .contacts
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.peer_id == target)
            .and_then(|c| c.ticket.clone());
        if let Some(t) = hint.and_then(|t| Ticket::decode(&t).ok()) {
            return Ok(t);
        }
    }
    ticket::parse_target(target).map_err(|_| "Destinatario non valido".to_string())
}

fn spawn_tracked(
    app: &Arc<App>,
    id: u64,
    fut: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let app2 = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        fut.await;
        app2.running.lock().unwrap().remove(&id);
    });
    app.running.lock().unwrap().insert(id, handle);
}

/// Sends to `peer`. With `target` set we first have to reach it (recent
/// device); without, a direct connection already exists (short code pull).
#[allow(clippy::too_many_arguments)]
async fn run_send(
    handle: AppHandle,
    app: Arc<App>,
    id: u64,
    peer: PeerId,
    target: Option<Ticket>,
    peer_name: Option<String>,
    paths: Vec<PathBuf>,
    files: Vec<FileInfo>,
) {
    let total = files.iter().map(|f| f.size).sum();
    let mut ev = TransferEvent {
        id,
        direction: "out",
        stage: "searching",
        peer: peer_name.unwrap_or_else(|| format!("Dispositivo {}", short(&peer.to_base58()))),
        files: files.clone(),
        total,
        done: 0,
        file_index: 0,
        message: None,
        detail: None,
        via: None,
        folder: None,
    };

    if let Some(target) = target {
        let _ = handle.emit("transfer", ev.clone());
        // Our own public address is needed before hole punching can work.
        let _ = app.node.wait_ready(false, Duration::from_secs(20)).await;
        ev.stage = "connecting";
        let _ = handle.emit("transfer", ev.clone());
        if let Err(failure) = app.node.connect_direct(target).await {
            ev.stage = "failed";
            ev.message = Some(friendly_failure(&failure));
            ev.detail = Some(failure.explain());
            let _ = handle.emit("transfer", ev);
            return;
        }
    }
    ev.via = app.node.direct_addr(peer).await.map(|a| describe_via(&a));
    ev.stage = "waiting"; // the receiver has to accept
    let _ = handle.emit("transfer", ev.clone());

    let from = app.settings.lock().unwrap().device_name.clone();
    let mut control = app.node.control();
    let mut throttle = Throttle::new();
    let sizes: Vec<u64> = files.iter().map(|f| f.size).collect();
    let mut base = 0u64;
    let res = transfer::send_files_named(&mut control, peer, &paths, &from, |p| match p {
        Progress::FileStart { index, offset, .. } => {
            base = sizes[..index].iter().sum();
            ev.stage = "transferring";
            ev.file_index = index;
            ev.done = base + offset;
            let _ = handle.emit("transfer", ev.clone());
        }
        Progress::Bytes { done, .. } => {
            ev.done = base + done;
            if ev.done == base + sizes[ev.file_index] {
                ev.stage = "verifying";
                let _ = handle.emit("transfer", ev.clone());
            } else if throttle.ready() {
                let _ = handle.emit("transfer", ev.clone());
            }
        }
        Progress::FileVerified { .. } => ev.stage = "transferring",
    })
    .await;
    match res {
        Ok(name) => {
            ev.stage = "done";
            ev.done = total;
            app.remember(&peer, name.as_deref(), None);
            if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
                ev.peer = n.trim().to_string();
            }
            notify(
                &handle,
                &app,
                "Trasferimento completato",
                &format!("{} inviati a {}", count_label(files.len()), ev.peer),
            );
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("rifiutato") {
                ev.stage = "rejected";
                ev.message = Some(format!("{} ha rifiutato i file.", ev.peer));
            } else {
                ev.stage = "failed";
                ev.message = Some(
                    "Il collegamento si è interrotto durante l'invio. Riprova: il trasferimento riprenderà da dove si era fermato."
                        .into(),
                );
            }
            ev.detail = Some(msg);
        }
    }
    let _ = handle.emit("transfer", ev);
}

fn count_label(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} file")
    }
}

fn friendly_failure(f: &ConnectFailure) -> String {
    let mut msg = if f.relayed_connection {
        format!(
            "L'altro dispositivo è online, ma tra le vostre due reti non è possibile un collegamento diretto \
             (succede di solito con reti mobili 4G/5G o aziendali). Rimedi: chi è su una rete di casa attiva \
             l'UPnP sul router, oppure apre la porta {} (TCP e UDP) verso il proprio computer; in \
             alternativa usate entrambi un Wi‑Fi.",
            config::DEFAULT_PORT
        )
    } else if !f.found_in_dht && f.dial_errors.is_empty() {
        "Dispositivo non trovato. L'app è aperta sull'altro dispositivo? Se è appena stata aperta, \
         aspetta un minuto e riprova."
            .to_string()
    } else {
        "L'altro dispositivo non risponde: potrebbe essere spento o offline.".to_string()
    };
    if !f.have_observed_addr {
        msg.push_str(" Nota: la tua rete sembra bloccare le connessioni necessarie.");
    }
    msg
}

fn describe_via(addr: &Multiaddr) -> String {
    let s = addr.to_string();
    let transport = if s.contains("quic") { "QUIC" } else { "TCP" };
    let lan = s.starts_with("/ip4/192.168.")
        || s.starts_with("/ip4/10.")
        || s.starts_with("/ip4/172.")
        || s.starts_with("/ip4/127.");
    if lan {
        format!("rete locale, {transport}")
    } else {
        format!("Internet, {transport}")
    }
}

struct Throttle(Instant);

impl Throttle {
    fn new() -> Self {
        Throttle(Instant::now() - Duration::from_secs(1))
    }
    fn ready(&mut self) -> bool {
        if self.0.elapsed() >= Duration::from_millis(120) {
            self.0 = Instant::now();
            true
        } else {
            false
        }
    }
}

// ------------------------------------------------------------- receiving

/// Short codes: a receiver typed one of our codes and asks us to send.
async fn pull_loop(handle: AppHandle, app: Arc<App>) {
    use futures::StreamExt;
    let mut control = app.node.control();
    let Ok(mut pulls) = control.accept(PULL_PROTOCOL) else {
        return;
    };
    while let Some((peer, stream)) = pulls.next().await {
        let Ok((req, responder)) = transfer::read_pull(stream).await else {
            continue;
        };
        let share = rendezvous::normalize_code(&req.code).and_then(|c| {
            let mut shares = app.shares.lock().unwrap();
            match shares.get(&c) {
                Some(s) if s.expires > Instant::now() => shares.remove(&c).map(|s| (c, s)),
                _ => None,
            }
        });
        let _ = responder.answer(share.is_some()).await;
        let Some((code, share)) = share else {
            continue;
        };
        app.node
            .stop_providing(rendezvous::key_for_code(&code))
            .await;
        let _ = handle.emit("share-used", share.id);
        app.remember(&peer, Some(&req.name), None);
        let Ok(infos) = file_infos(&share.files) else {
            continue;
        };
        spawn_tracked(
            &app,
            share.id,
            run_send(
                handle.clone(),
                app.clone(),
                share.id,
                peer,
                None,
                Some(req.name),
                share.files,
                infos,
            ),
        );
    }
}

async fn receive_loop(handle: AppHandle, app: Arc<App>) {
    use futures::StreamExt;
    let mut control = app.node.control();
    let Ok(mut incoming) = control.accept(TRANSFER_PROTOCOL) else {
        return;
    };
    while let Some((peer, stream)) = incoming.next().await {
        let handle = handle.clone();
        let app2 = app.clone();
        let id = app.next_id();
        spawn_tracked(&app, id, async move {
            let app = app2;
            let Ok((offer, incoming)) = transfer::read_offer(stream).await else {
                return;
            };
            let files: Vec<FileInfo> = offer
                .files
                .iter()
                .map(|f| FileInfo {
                    name: f.name.clone(),
                    size: f.size,
                })
                .collect();
            let total = offer.total_size();
            let known = app.contact_name(&peer).is_some();
            let auto = known && app.settings.lock().unwrap().auto_accept_contacts;
            let accepted = if auto {
                true
            } else {
                let (tx, rx) = oneshot::channel();
                app.pending_offers.lock().unwrap().insert(id, tx);
                let _ = handle.emit(
                    "offer",
                    OfferEvent {
                        id,
                        peer_id: peer.to_base58(),
                        from: offer.from.clone(),
                        files: files.clone(),
                        total,
                        known_contact: known,
                    },
                );
                notify(
                    &handle,
                    &app,
                    "Richiesta di trasferimento",
                    &format!("{} vuole inviarti {}", offer.from, count_label(files.len())),
                );
                let ok = matches!(
                    tokio::time::timeout(Duration::from_secs(300), rx).await,
                    Ok(Ok(true))
                );
                app.pending_offers.lock().unwrap().remove(&id);
                let _ = handle.emit("offer-closed", id);
                ok
            };
            if !accepted {
                let _ = incoming.reject("rifiutato dal destinatario").await;
                return;
            }
            let dest = app.settings.lock().unwrap().download_dir.clone();
            let mut ev = TransferEvent {
                id,
                direction: "in",
                stage: "transferring",
                peer: offer.from.clone(),
                files,
                total,
                done: 0,
                file_index: 0,
                message: None,
                detail: None,
                via: app.node.direct_addr(peer).await.map(|a| describe_via(&a)),
                folder: Some(dest.display().to_string()),
            };
            app.remember(&peer, Some(&offer.from), None);
            let _ = handle.emit("transfer", ev.clone());
            let sizes: Vec<u64> = offer.files.iter().map(|f| f.size).collect();
            let mut base = 0;
            let mut throttle = Throttle::new();
            let my_name = app.settings.lock().unwrap().device_name.clone();
            let res = incoming
                .accept_as(peer, &dest, Some(&my_name), |p| match p {
                    Progress::FileStart { index, offset, .. } => {
                        base = sizes[..index].iter().sum();
                        ev.stage = "transferring";
                        ev.file_index = index;
                        ev.done = base + offset;
                        let _ = handle.emit("transfer", ev.clone());
                    }
                    Progress::Bytes { done, .. } => {
                        ev.done = base + done;
                        if ev.done == base + sizes[ev.file_index] {
                            ev.stage = "verifying";
                            let _ = handle.emit("transfer", ev.clone());
                        } else if throttle.ready() {
                            let _ = handle.emit("transfer", ev.clone());
                        }
                    }
                    Progress::FileVerified { .. } => ev.stage = "transferring",
                })
                .await;
            match res {
                Ok(paths) => {
                    ev.stage = "done";
                    ev.done = total;
                    if paths.len() == 1 {
                        ev.folder = Some(paths[0].display().to_string());
                    }
                    notify(
                        &handle,
                        &app,
                        "Trasferimento completato",
                        &format!("{} ricevuti da {}", count_label(paths.len()), offer.from),
                    );
                }
                Err(e) => {
                    ev.stage = "failed";
                    ev.message = Some(
                        "Il collegamento si è interrotto. La parte già ricevuta è conservata: se l'altro dispositivo reinvia, si riparte da lì."
                            .into(),
                    );
                    ev.detail = Some(format!("{e:#}"));
                }
            }
            let _ = handle.emit("transfer", ev);
        });
    }
}

// ------------------------------------------------------------------ setup

fn load<T: for<'de> Deserialize<'de>>(path: PathBuf) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn bootstrap_list() -> Vec<Multiaddr> {
    // LSO_BOOTSTRAP / LSO_NO_DEFAULT_BOOTSTRAP: for the network lab only.
    let mut list: Vec<Multiaddr> = std::env::var("LSO_BOOTSTRAP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if std::env::var("LSO_NO_DEFAULT_BOOTSTRAP").is_err() {
        list.extend(config::default_bootstrap());
    }
    list
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|tauri_app| {
            let data_dir = match std::env::var("LSO_DATA_DIR") {
                Ok(d) => PathBuf::from(d),
                Err(_) => tauri_app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            let keypair = identity::load_or_create(&data_dir.join("identity.key"))?;
            let settings: Settings = load(data_dir.join("settings.json")).unwrap_or_default();
            let contacts: Vec<Contact> = load(data_dir.join("contacts.json")).unwrap_or_default();

            let node = tauri::async_runtime::block_on(Node::start(NodeConfig {
                keypair,
                role: Role::Device,
                bootstrap: bootstrap_list(),
                listen_port: std::env::var("LSO_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(config::DEFAULT_PORT),
                reachable: true,
                enable_mdns: true,
                enable_upnp: true,
                external_addrs: vec![],
            }))?;

            let app = Arc::new(App {
                node,
                data_dir,
                settings: Mutex::new(settings),
                contacts: Mutex::new(contacts),
                pending_offers: Mutex::new(HashMap::new()),
                shares: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
            });
            app.save_settings();
            tauri_app.manage(app.clone());

            let handle = tauri_app.handle().clone();
            tauri::async_runtime::spawn(receive_loop(handle.clone(), app.clone()));
            tauri::async_runtime::spawn(pull_loop(handle.clone(), app.clone()));

            // Nudge the UI to refresh its network status when it changes,
            // and tell it when another device reaches us (relayed signalling
            // connection) and how the upgrade to a direct connection went:
            // otherwise the person sharing a code sees nothing at all.
            let mut events = app.node.events();
            let app_ev = app.clone();
            tauri::async_runtime::spawn(async move {
                while let Ok(ev) = events.recv().await {
                    let peer_event = match &ev {
                        NodeEvent::ConnectionOpened { peer, relayed: true, .. } => {
                            Some((*peer, "reached", None))
                        }
                        NodeEvent::HolePunch { peer, result: Ok(()) } => Some((*peer, "direct", None)),
                        NodeEvent::HolePunch { peer, result: Err(e) } => Some((*peer, "failed", Some(e.clone()))),
                        _ => None,
                    };
                    if let Some((peer, kind, detail)) = peer_event {
                        let _ = handle.emit(
                            "peer",
                            PeerEvent {
                                peer_id: peer.to_base58(),
                                name: app_ev.contact_name(&peer),
                                kind,
                                message: (kind == "failed").then(|| {
                                    format!(
                                        "Un dispositivo ha inserito il codice, ma tra le vostre due reti il collegamento \
                                         diretto non è possibile (succede con le reti mobili 4G/5G). Rimedio: sul router \
                                         di casa attiva l'UPnP oppure apri la porta {} (TCP e UDP) verso questo \
                                         computer, poi fate riprovare: il codice resta valido.",
                                        config::DEFAULT_PORT
                                    )
                                }),
                                detail,
                            },
                        );
                    }
                    if matches!(
                        ev,
                        NodeEvent::ReservationAccepted { .. }
                            | NodeEvent::ReservationLost { .. }
                            | NodeEvent::NatHint(_)
                            | NodeEvent::Announced
                    ) {
                        let _ = handle.emit("net", ());
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            settings,
            save_settings,
            contacts,
            forget_contact,
            open_download_dir,
            reveal_path,
            file_sizes,
            answer_offer,
            cancel,
            start_share,
            stop_share,
            send_to,
            receive_code,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Local Send Online");
}
