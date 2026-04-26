//! Paired-peers store, on-disk JSON.
//!
//! JSON because the file is small (one entry per paired host) and human
//! debuggable — useful when a user complains "MouseFly thinks I paired with
//! a host I don't recognize". Production-grade tamper resistance is not a
//! goal: the cert fingerprints stored here are pinned against the QUIC
//! connection at runtime, so editing the file just removes trust, never adds
//! it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPeer {
    pub host_id_hex: String,
    pub instance_name: String,
    pub cert_fingerprint_hex: String,
    pub paired_at_unix: u64,
}

#[derive(Debug)]
pub struct PairedPeerStore {
    path: PathBuf,
    peers: HashMap<String, PairedPeer>,
}

#[derive(Serialize, Deserialize, Default)]
struct OnDisk {
    peers: Vec<PairedPeer>,
}

impl PairedPeerStore {
    pub fn load(path: &Path) -> Result<Self> {
        let peers = if path.exists() {
            let bytes = std::fs::read(path)
                .with_context(|| format!("read paired peers at {}", path.display()))?;
            let on_disk: OnDisk = serde_json::from_slice(&bytes)
                .with_context(|| format!("decode paired peers at {}", path.display()))?;
            on_disk
                .peers
                .into_iter()
                .map(|p| (p.host_id_hex.clone(), p))
                .collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            peers,
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent of {}", self.path.display()))?;
        }
        let mut peers: Vec<&PairedPeer> = self.peers.values().collect();
        peers.sort_by(|a, b| a.host_id_hex.cmp(&b.host_id_hex));
        let on_disk = OnDiskRef { peers };
        let bytes = serde_json::to_vec_pretty(&on_disk).context("encode paired peers")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    pub fn upsert(&mut self, p: PairedPeer) {
        self.peers.insert(p.host_id_hex.clone(), p);
    }

    pub fn remove(&mut self, host_id_hex: &str) {
        self.peers.remove(host_id_hex);
    }

    pub fn get(&self, host_id_hex: &str) -> Option<&PairedPeer> {
        self.peers.get(host_id_hex)
    }

    pub fn list(&self) -> Vec<&PairedPeer> {
        let mut v: Vec<&PairedPeer> = self.peers.values().collect();
        v.sort_by(|a, b| a.host_id_hex.cmp(&b.host_id_hex));
        v
    }
}

#[derive(Serialize)]
struct OnDiskRef<'a> {
    peers: Vec<&'a PairedPeer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("mousefly-pair-store-{}", rand_suffix()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn rand_suffix() -> String {
        use rand::rngs::OsRng;
        use rand::RngCore;
        let mut buf = [0u8; 8];
        OsRng.fill_bytes(&mut buf);
        hex::encode(buf)
    }

    #[test]
    fn upsert_save_load_roundtrip() {
        let dir = tempdir();
        let path = dir.join("peers.json");
        let mut s = PairedPeerStore::load(&path).unwrap();
        s.upsert(PairedPeer {
            host_id_hex: "aa".into(),
            instance_name: "Studio".into(),
            cert_fingerprint_hex: "bb".into(),
            paired_at_unix: 1,
        });
        s.upsert(PairedPeer {
            host_id_hex: "cc".into(),
            instance_name: "Lap".into(),
            cert_fingerprint_hex: "dd".into(),
            paired_at_unix: 2,
        });
        s.save().unwrap();

        let s2 = PairedPeerStore::load(&path).unwrap();
        assert_eq!(s2.list().len(), 2);
        assert_eq!(s2.get("aa").unwrap().instance_name, "Studio");
        assert!(s2.get("zz").is_none());
    }

    #[test]
    fn remove_drops_entry() {
        let dir = tempdir();
        let path = dir.join("peers.json");
        let mut s = PairedPeerStore::load(&path).unwrap();
        s.upsert(PairedPeer {
            host_id_hex: "aa".into(),
            instance_name: "Studio".into(),
            cert_fingerprint_hex: "bb".into(),
            paired_at_unix: 1,
        });
        s.remove("aa");
        assert!(s.list().is_empty());
    }
}
