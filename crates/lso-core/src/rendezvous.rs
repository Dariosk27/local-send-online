//! DHT rendezvous key for a device.
//!
//! NAT-ed devices run Kademlia in *client* mode, so they are not in anybody's
//! routing table and a plain FIND_NODE(PeerId) cannot locate them. Instead a
//! receiving device publishes a *provider record* under a key derived from
//! its own PeerId; the record carries its current (relay) addresses. Anyone
//! who knows the PeerId can derive the key and ask the DHT for providers.
//!
//! The key is a sha2-256 multihash so that go/js DHT servers, which expect
//! CIDs/multihashes for provider records, accept it.

use libp2p::{kad::RecordKey, PeerId};
use sha2::{Digest, Sha256};

pub fn key_for(peer: &PeerId) -> RecordKey {
    let digest = Sha256::new()
        .chain_update(b"/lso/v1/rendezvous/")
        .chain_update(peer.to_bytes())
        .finalize();
    let mut mh = Vec::with_capacity(34);
    mh.extend_from_slice(&[0x12, 0x20]);
    mh.extend_from_slice(&digest);
    RecordKey::new(&mh)
}
