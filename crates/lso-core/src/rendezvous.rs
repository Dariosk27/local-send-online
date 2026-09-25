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

/// Alphabet for short codes: no 0/O, 1/I/L, easy to read aloud and type.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// A short, human-typeable code (8 symbols ≈ 39 bits), shown as `7F3K-9H2P`.
///
/// It is only a *discovery* secret: the sharer announces a provider record
/// under `H(code)` for a few minutes, whoever types the code finds the
/// sharer's PeerId and connects. The transfer itself is still authenticated
/// by Noise and must be accepted by the receiver.
pub fn generate_code() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("OS random source");
    let s: String = bytes
        .iter()
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &s[..4], &s[4..])
}

/// Normalises what the user typed (case, spaces, dashes). Returns `None` if it cannot be a valid code.
pub fn normalize_code(input: &str) -> Option<String> {
    let s: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if s.len() != 8 || !s.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        return None;
    }
    Some(format!("{}-{}", &s[..4], &s[4..]))
}

pub fn key_for_code(code: &str) -> RecordKey {
    let digest = Sha256::new()
        .chain_update(b"/lso/v1/code/")
        .chain_update(code.as_bytes())
        .finalize();
    let mut mh = Vec::with_capacity(34);
    mh.extend_from_slice(&[0x12, 0x20]);
    mh.extend_from_slice(&digest);
    RecordKey::new(&mh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip() {
        for _ in 0..100 {
            let c = generate_code();
            assert_eq!(c.len(), 9);
            assert_eq!(normalize_code(&c).as_deref(), Some(c.as_str()));
            assert_eq!(
                normalize_code(&c.to_lowercase().replace('-', " ")).as_deref(),
                Some(c.as_str())
            );
        }
        assert!(normalize_code("ABC").is_none());
        assert_eq!(key_for_code("7F3K-9H2P"), key_for_code("7F3K-9H2P"));
    }
}
