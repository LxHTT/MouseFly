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

use anyhow::{anyhow, Result};
use mousefly_net::{cert_fingerprint, pair_connect, pair_serve, Endpoint};
use mousefly_pair::{
    generate_pairing_code, run_initiator, run_responder, Identity, PairedPeer, PairedPeerStore,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct PairingCodePayload {
    pub code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingResultPayload {
    pub ok: bool,
    pub peer: Option<PairedPeer>,
    pub reason: Option<String>,
}

/// Shared mutable state for the pairing daemon.
pub struct PairingState {
    pub identity: Arc<Identity>,
    pub instance_name: String,
    pub data_cert_fingerprint_hex: String,
    pub paired_peers: Arc<Mutex<PairedPeerStore>>,
    pub pending_code: Arc<Mutex<Option<String>>>,
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
        let code = state.pending_code.lock().await.take();
        match code {
            Some(code) => {
                let id = state.identity.clone();
                let store = state.paired_peers.clone();
                let app2 = app.clone();
                let fp = state.data_cert_fingerprint_hex.clone();
                let name = state.instance_name.clone();
                tokio::spawn(async move {
                    let result = run_responder((recv, send), &code, &id, &fp, &name).await;
                    finish_pairing(app2, store, result).await;
                });
            }
            None => {
                warn!("dropped unsolicited pairing connection");
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
            info!(host_id = %r.peer_host_id_hex, "pairing successful");
            PairingResultPayload {
                ok: true,
                peer: Some(peer),
                reason: None,
            }
        }
        Err(e) => {
            warn!("pairing failed: {e}");
            PairingResultPayload {
                ok: false,
                peer: None,
                reason: Some(format!("{e}")),
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
    *state.pending_code.lock().await = Some(raw.clone());
    let _ = app.emit("pairing-code", &PairingCodePayload { code: raw.clone() });
    Ok(code)
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
