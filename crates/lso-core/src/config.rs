//! Static configuration of the control plane.

use std::time::Duration;

use libp2p::{Multiaddr, StreamProtocol};

/// Application protocol carried on direct connections only (data plane).
pub const TRANSFER_PROTOCOL: StreamProtocol = StreamProtocol::new("/lso/transfer/1.0.0");

/// Agent string announced through identify.
pub const AGENT_VERSION: &str = concat!("lso/", env!("CARGO_PKG_VERSION"));

/// Public IPFS/libp2p bootstrap nodes (same list kubo ships with).
///
/// These are *not* operated by us and are only used to enter the public
/// Kademlia DHT. They are a soft point of centralisation: if all of them are
/// unreachable a node that has never been online can't join the network
/// (see docs/ANALISI.md, "bootstrap"). Users can override them with
/// `--bootstrap`.
pub const IPFS_BOOTSTRAP: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/dnsaddr/va1.bootstrap.libp2p.io/p2p/12D3KooWKnDdG3iXw9eTFijk3EWSunZcFi54Zka4wmtqtt6rPxc8",
    "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
    "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
];

pub fn default_bootstrap() -> Vec<Multiaddr> {
    IPFS_BOOTSTRAP
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// How many relay reservations a receiving node tries to keep open. More
/// than one so that a single relay going away does not make us unreachable.
pub const TARGET_RESERVATIONS: usize = 2;

/// Upper bound for the whole "reach the peer directly" phase (relay dial +
/// DCUtR, which itself retries 3 times).
pub const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Re-announce ourselves in the DHT this often (and whenever our relay
/// addresses change), so that a new IP is picked up quickly.
pub const REPROVIDE_INTERVAL: Duration = Duration::from_secs(10 * 60);
