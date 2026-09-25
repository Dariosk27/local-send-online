//! Local Send Online core library.
//!
//! * control plane: `node` (libp2p: DHT, relays for signalling, DCUtR)
//! * data plane: `transfer` (direct-only stream protocol, see `direct_only`)

pub mod config;
pub mod direct_only;
pub mod identity;
pub mod node;
pub mod rendezvous;
pub mod ticket;
pub mod transfer;

pub use libp2p::{identity::Keypair, Multiaddr, PeerId};
