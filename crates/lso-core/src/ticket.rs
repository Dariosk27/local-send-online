//! A ticket is what the receiver hands to the sender out of band (chat, QR,
//! e-mail): its PeerId plus the addresses it believes are reachable *right
//! now*. The PeerId is the part that matters (it authenticates the remote via
//! Noise); addresses are hints and may be stale, in which case the sender
//! falls back to a DHT lookup of the PeerId.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "lso1";

#[derive(Debug, Clone)]
pub struct Ticket {
    pub peer: PeerId,
    pub addrs: Vec<Multiaddr>,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    p: String,
    a: Vec<String>,
}

impl Ticket {
    pub fn encode(&self) -> String {
        let wire = Wire {
            p: self.peer.to_base58(),
            a: self.addrs.iter().map(|a| a.to_string()).collect(),
        };
        format!(
            "{PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&wire).unwrap())
        )
    }

    pub fn decode(s: &str) -> Result<Self> {
        let Some(body) = s.trim().strip_prefix(PREFIX) else {
            bail!("not a lso ticket");
        };
        let wire: Wire = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body)?)?;
        Ok(Ticket {
            peer: wire.p.parse().context("bad peer id in ticket")?,
            addrs: wire.a.iter().filter_map(|a| a.parse().ok()).collect(),
        })
    }
}

/// Parses whatever the user typed as destination: a ticket or a bare PeerId.
pub fn parse_target(s: &str) -> Result<Ticket> {
    if s.starts_with(PREFIX) {
        return Ticket::decode(s);
    }
    Ok(Ticket {
        peer: s.trim().parse().context("neither a ticket nor a PeerId")?,
        addrs: vec![],
    })
}

/// Removes a trailing `/p2p/<peer>` so addresses can be re-used with any dial.
pub fn strip_peer(mut addr: Multiaddr, peer: &PeerId) -> Multiaddr {
    if let Some(Protocol::P2p(p)) = addr.iter().last() {
        if &p == peer {
            addr.pop();
        }
    }
    addr
}
