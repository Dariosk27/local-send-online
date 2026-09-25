//! Device identity: an Ed25519 key generated locally on first start.
//!
//! The PeerId derived from it is the only stable "address" of a device: IPs
//! change, the PeerId does not. There are no accounts; whoever holds the key
//! *is* the device, and Noise authenticates it on every connection.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use libp2p::identity::Keypair;

pub fn load_or_create(path: &Path) -> Result<Keypair> {
    if path.exists() {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return Keypair::from_protobuf_encoding(&bytes).context("identity file is corrupted");
    }
    let key = Keypair::generate_ed25519();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, key.to_protobuf_encoding()?)?;
    restrict_permissions(path);
    Ok(key)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Deterministic identity derived from a string. Only for automated tests
/// (e.g. CI jobs on different machines that must know each other's PeerId);
/// anyone knowing the seed can impersonate the device.
pub fn from_test_seed(seed: &str) -> Keypair {
    use sha2::{Digest, Sha256};
    let bytes: [u8; 32] = Sha256::digest(seed.as_bytes()).into();
    Keypair::ed25519_from_bytes(bytes).expect("32 bytes")
}
