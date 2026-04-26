//! Pairing-side state machine and Tauri command handlers.
//!
//! Two endpoints run in this app:
//!   - Data endpoint (serve/connect Link, port from `--listen` / 7878 default)
//!   - Pairing endpoint (separate ephemeral port, advertised via mDNS so peers
//!     can find it during the brief pairing window)
//!
//! The pairing endpoint's accept loop is permanent: it waits for connections,
//! checks `pending_code` state, and either runs `mousefly_pair::run_responder`
//! or rejects the connection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use mousefly_net::{cert_fingerprint, pair_connect, pair_serve, Endpoint};
use mousefly_pair::{
    generate_pairing_code, run_initiator, run_responder, Identity, PairedPeer, PairedPeerStore,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// How long a generated pairing code is valid for. Apple / WhatsApp / Signal
/// all use ~5 minutes; matching that.
const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// Max consecutive failed attempts before the responder forces a fresh code.
/// SPAKE2 already requires a full handshake per attempt (which is expensive
/// online), but this is defence-in-depth.
const MAX_FAILED_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct PairingCodePayload {
    pub code: String,
    pub expires_unix: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingResultPayload {
    pub ok: bool,
    pub peer: Option<PairedPeer>,
    pub reason: Option<String>,
    pub verification_sas: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalIdentityPayload {
    pub host_id_hex: String,
    pub instance_name: String,
    pub cert_fingerprint_hex: String,
}

/// Pending pairing slot. `expires_at` is checked at the start of every
/// incoming attempt; `failed_attempts` is bumped on PairingError and forces a
/// fresh code once it crosses MAX_FAILED_ATTEMPTS so brute-force attempts
/// can't stack.
#[derive(Debug, Clone)]
pub struct PendingCode {
    pub raw: String,
    pub expires_at: Instant,
    pub failed_attempts: u32,
}

/// Shared mutable state for the pairing daemon.
pub struct PairingState {
    pub identity: Arc<Identity>,
    pub instance_name: String,
    pub data_cert_fingerprint_hex: String,
    pub paired_peers: Arc<Mutex<PairedPeerStore>>,
    pub pending_code: Arc<Mutex<Option<PendingCode>>>,
    pub pair_endpoint: Arc<Endpoint>,
}

/// Background accept loop for incoming pairing connections.
pub async fn run_pair_acceptor(state: PairingState, app: AppHandle) {
    loop {
        let (send, recv) = match pair_serve(&state.pair_endpoint).await {
            Ok(s) => s,
            Err(e) => {
                warn!("pair_serve error: {e:#}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        // Atomically grab the current code AND clear it (single-use). If the
        // attempt fails, we re-insert below if there are retries left.
        let code = {
            let mut guard = state.pending_code.lock().await;
            match guard.as_ref() {
                Some(p) if p.expires_at > Instant::now() => guard.take(),
                Some(_) => {
                    *guard = None;
                    None
                }
                None => None,
            }
        };
        match code {
            Some(pending) => {
                let pending_code = pending.raw.clone();
                let id = state.identity.clone();
                let store = state.paired_peers.clone();
                let pending_slot = state.pending_code.clone();
                let app2 = app.clone();
                let fp = state.data_cert_fingerprint_hex.clone();
                let name = state.instance_name.clone();
                tokio::spawn(async move {
                    let result = run_responder((recv, send), &pending_code, &id, &fp, &name).await;
                    let succeeded = result.is_ok();
                    finish_pairing(app2.clone(), store, result).await;
                    if !succeeded {
                        // Put the code back if there are retries left.
                        let mut guard = pending_slot.lock().await;
                        let next = PendingCode {
                            failed_attempts: pending.failed_attempts + 1,
                            ..pending.clone()
                        };
                        if next.failed_attempts >= MAX_FAILED_ATTEMPTS
                            || next.expires_at <= Instant::now()
                        {
                            *guard = None;
                            let _ = app2.emit(
                                "pairing-locked",
                                &serde_json::json!({
                                    "reason": "too many failed attempts — generate a new code",
                                }),
                            );
                        } else {
                            *guard = Some(next);
                        }
                    }
                });
            }
            None => {
                warn!("dropped unsolicited or expired pairing connection");
                drop(send);
                drop(recv);
            }
        }
    }
}

async fn finish_pairing(
    app: AppHandle,
    store: Arc<Mutex<PairedPeerStore>>,
    result: std::result::Result<mousefly_pair::PairingResult, mousefly_pair::PairingError>,
) {
    let payload = match result {
        Ok(r) => {
            let peer = PairedPeer {
                host_id_hex: r.peer_host_id_hex.clone(),
                instance_name: r.instance_name.clone(),
                cert_fingerprint_hex: r.peer_cert_fingerprint_hex.clone(),
                paired_at_unix: mousefly_pair::now_unix(),
            };
            let mut s = store.lock().await;
            s.upsert(peer.clone());
            if let Err(e) = s.save() {
                warn!("paired-peers save failed: {e:#}");
            }
            info!(host_id = %r.peer_host_id_hex, sas = %r.verification_sas, "pairing successful");
            PairingResultPayload {
                ok: true,
                peer: Some(peer),
                reason: None,
                verification_sas: Some(r.verification_sas),
            }
        }
        Err(e) => {
            warn!("pairing failed: {e}");
            PairingResultPayload {
                ok: false,
                peer: None,
                reason: Some(format!("{e}")),
                verification_sas: None,
            }
        }
    };
    if let Err(e) = app.emit("pairing-result", &payload) {
        warn!("emit pairing-result failed: {e}");
    }
}

#[tauri::command]
pub async fn start_pair_responder(
    app: AppHandle,
    state: State<'_, PairingState>,
) -> std::result::Result<String, String> {
    let code = generate_pairing_code();
    let raw = code.replace(' ', "");
    let expires_at = Instant::now() + CODE_TTL;
    let expires_unix = mousefly_pair::now_unix() + CODE_TTL.as_secs();
    *state.pending_code.lock().await = Some(PendingCode {
        raw: raw.clone(),
        expires_at,
        failed_attempts: 0,
    });
    let _ = app.emit(
        "pairing-code",
        &PairingCodePayload {
            code: raw.clone(),
            expires_unix,
        },
    );
    Ok(code)
}

#[tauri::command]
pub async fn get_local_identity(
    state: State<'_, PairingState>,
) -> std::result::Result<LocalIdentityPayload, String> {
    Ok(LocalIdentityPayload {
        host_id_hex: state.identity.host_id_hex(),
        instance_name: state.instance_name.clone(),
        cert_fingerprint_hex: state.data_cert_fingerprint_hex.clone(),
    })
}

#[tauri::command]
pub async fn start_pair_initiator(
    app: AppHandle,
    state: State<'_, PairingState>,
    addr: String,
    code: String,
) -> std::result::Result<(), String> {
    let raw = code.replace(' ', "");
    let identity = state.identity.clone();
    let store = state.paired_peers.clone();
    let fp = state.data_cert_fingerprint_hex.clone();
    let name = state.instance_name.clone();

    // Use a fresh client endpoint for each initiator attempt — the data
    // endpoint is for accepting; pairing as initiator opens a new conn.
    let client_endpoint = Endpoint::client().map_err(|e| format!("client endpoint: {e:#}"))?;

    tokio::spawn(async move {
        let outcome = run_initiator_flow(client_endpoint, addr, raw, identity, fp, name).await;
        finish_pairing(app, store, outcome).await;
    });
    Ok(())
}

async fn run_initiator_flow(
    endpoint: Endpoint,
    addr: String,
    code: String,
    identity: Arc<Identity>,
    fp: String,
    name: String,
) -> std::result::Result<mousefly_pair::PairingResult, mousefly_pair::PairingError> {
    let (send, recv) = pair_connect(&endpoint, &addr)
        .await
        .map_err(|e| mousefly_pair::PairingError::Framing(format!("connect: {e:#}")))?;
    run_initiator((recv, send), &code, &identity, &fp, &name).await
}

#[tauri::command]
pub async fn list_paired_peers(
    state: State<'_, PairingState>,
) -> std::result::Result<Vec<PairedPeer>, String> {
    let s = state.paired_peers.lock().await;
    Ok(s.list().into_iter().cloned().collect())
}

#[tauri::command]
pub async fn cancel_pairing(state: State<'_, PairingState>) -> std::result::Result<(), String> {
    *state.pending_code.lock().await = None;
    Ok(())
}

/// Caller helper: SHA-256 fingerprint of a cert as 64-char hex.
pub fn fingerprint_hex(cert_der: &[u8]) -> String {
    hex::encode(cert_fingerprint(cert_der))
}

/// Best-effort hostname for use as an mDNS instance name.
pub fn host_label() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "mousefly".into())
}

/// Convenience: load identity from disk, or generate + persist a fresh one.
pub fn load_or_create_identity() -> Result<Identity> {
    let dir = mousefly_pair::default_config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = mousefly_pair::identity_path();
    mousefly_pair::load_or_create_identity(&path).map_err(|e| anyhow!("identity: {e:#}"))
}

/// Convenience: load paired-peers store from the canonical path.
pub fn load_paired_peers() -> Result<PairedPeerStore> {
    let dir = mousefly_pair::default_config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = mousefly_pair::paired_peers_path();
    mousefly_pair::PairedPeerStore::load(&path).map_err(|e| anyhow!("paired-peers: {e:#}"))
}
