//! Long-lived ed25519 host identity.
//!
//! The signing key is the durable proof of "this host is the same one I paired
//! with last time". We persist the 32-byte secret seed in bincode form;
//! `chmod 600` on unix keeps casual readers out, but anyone with the file
//! impersonates the host. Treat it accordingly.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub signing_key_bytes: [u8; 32],
    pub verifying_key_bytes: [u8; 32],
    pub created_at_unix: u64,
}

impl Identity {
    pub fn generate() -> Self {
        let mut rng = OsRng;
        let signing = SigningKey::generate(&mut rng);
        let verifying = signing.verifying_key();
        let created_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            signing_key_bytes: signing.to_bytes(),
            verifying_key_bytes: verifying.to_bytes(),
            created_at_unix,
        }
    }

    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.signing_key_bytes)
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        // Stored bytes were derived from the signing key on creation, so the
        // unwrap is structurally sound. If it ever fails, the file is corrupt
        // and the caller should regenerate.
        VerifyingKey::from_bytes(&self.verifying_key_bytes)
            .expect("stored verifying key is invalid; identity file corrupt")
    }

    pub fn host_id_hex(&self) -> String {
        hex::encode(self.verifying_key_bytes)
    }
}

pub fn load_or_create_identity(path: &Path) -> Result<Identity> {
    if path.exists() {
        let bytes =
            std::fs::read(path).with_context(|| format!("read identity at {}", path.display()))?;
        let id: Identity = bincode::deserialize(&bytes)
            .with_context(|| format!("decode identity at {}", path.display()))?;
        Ok(id)
    } else {
        let id = Identity::generate();
        save_identity(path, &id)?;
        Ok(id)
    }
}

pub fn save_identity(path: &Path, id: &Identity) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent of {}", path.display()))?;
    }
    let bytes = bincode::serialize(id).context("encode identity")?;

    // Atomic-ish write: tmp + rename. Avoids leaving a half-written identity
    // file if the process dies mid-write.
    let tmp = path.with_extension("bin.tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .with_context(|| format!("open {}", tmp.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().ok();
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perms)
            .with_context(|| format!("chmod 600 {}", tmp.display()))?;
    }

    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

pub fn default_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("mousefly");
        }
        PathBuf::from(".").join("mousefly")
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".config").join("mousefly");
        }
        PathBuf::from(".").join("mousefly")
    }
}

pub fn identity_path() -> PathBuf {
    default_config_dir().join("identity.bin")
}

pub fn paired_peers_path() -> PathBuf {
    default_config_dir().join("paired-peers.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_identity_on_disk() {
        let dir = tempdir();
        let path = dir.join("identity.bin");
        let a = load_or_create_identity(&path).unwrap();
        let b = load_or_create_identity(&path).unwrap();
        assert_eq!(a.signing_key_bytes, b.signing_key_bytes);
        assert_eq!(a.verifying_key_bytes, b.verifying_key_bytes);
        assert_eq!(a.host_id_hex().len(), 64);
    }

    #[test]
    fn signing_key_round_trip_signs_verifies() {
        let id = Identity::generate();
        let sk = id.signing_key();
        let vk = id.verifying_key();
        use ed25519_dalek::Signer;
        let sig = sk.sign(b"hello");
        use ed25519_dalek::Verifier;
        vk.verify(b"hello", &sig).unwrap();
    }

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("mousefly-pair-test-{}", rand_suffix()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn rand_suffix() -> String {
        use rand::RngCore;
        let mut buf = [0u8; 8];
        OsRng.fill_bytes(&mut buf);
        hex::encode(buf)
    }
}
