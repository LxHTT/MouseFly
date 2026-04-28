//! MouseFly app binary — Tauri shell + role wiring.
//!
//! Roles:
//!   - sender:   `pnpm dev -- -- --peer 192.168.x.y:7878`
//!   - receiver: `pnpm dev -- -- --listen 0.0.0.0:7878 [--inject]`
//!
//! Loopback safety: `--inject` defaults off so running both roles on the same
//! Mac (`--listen :7878` and `--peer 127.0.0.1:7878`) doesn't feedback-loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod layout;
mod pairing;

use layout::{SharedLayout, Side};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;
use mousefly_core::{Frame, Monitor};
use mousefly_discovery::{AdvertiseConfig, Browser, DiscoveredPeer};
use mousefly_input::{install_kill_switch, permissions_granted, InputBackend, Platform};
use mousefly_net::{connect, serve, Endpoint, LinkHealth};
use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, error, info, warn};

use pairing::PairingState;

#[derive(Parser, Debug, Clone)]
#[command(
    version,
    about = "MouseFly — keyboard/mouse forwarding across Macs (Windows in flight)"
)]
struct Cli {
    /// Bind address for the receiver role, e.g. `0.0.0.0:7878`.
    #[arg(long, conflicts_with = "peer")]
    listen: Option<String>,

    /// Peer address for the sender role, e.g. `192.168.1.5:7878`.
    #[arg(long, conflicts_with = "listen")]
    peer: Option<String>,

    /// Inject events into the local OS on the receiver. Off by default for
    /// safe single-machine loopback testing.
    #[arg(long)]
    inject: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Role {
    Idle,
    Sender { peer: String },
    Receiver { listen: String, inject: bool },
}

/// Holds the currently-running role task, if any. Tauri commands replace it
/// when the user starts a new link or stops the current one.
#[derive(Default)]
struct LinkRuntime {
    role: AsyncMutex<Option<Role>>,
    handle: AsyncMutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[tauri::command]
async fn start_link(
    app: AppHandle,
    runtime: tauri::State<'_, Arc<LinkRuntime>>,
    role: Role,
) -> std::result::Result<(), String> {
    // Stop any currently-running role first.
    {
        let mut h = runtime.handle.lock().await;
        if let Some(handle) = h.take() {
            handle.abort();
        }
    }
    if matches!(role, Role::Idle) {
        *runtime.role.lock().await = Some(Role::Idle);
        let _ = app.emit("role", &Role::Idle);
        return Ok(());
    }
    *runtime.role.lock().await = Some(role.clone());
    let _ = app.emit("role", &role);

    let layout = app.state::<SharedLayout>().inner().clone();
    let role_for_task = role.clone();
    let app_for_task = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = run_role(role_for_task, app_for_task.clone(), layout).await {
            error!("role task exited: {e:#}");
            emit_status(&app_for_task, "error", format!("{e:#}"));
        }
    });
    *runtime.handle.lock().await = Some(handle);
    Ok(())
}

#[tauri::command]
async fn stop_link(
    app: AppHandle,
    runtime: tauri::State<'_, Arc<LinkRuntime>>,
) -> std::result::Result<(), String> {
    if let Some(handle) = runtime.handle.lock().await.take() {
        handle.abort();
    }
    *runtime.role.lock().await = Some(Role::Idle);
    let _ = app.emit("role", &Role::Idle);
    emit_status(&app, "info", "Link stopped");
    Ok(())
}

#[tauri::command]
async fn current_role(
    runtime: tauri::State<'_, Arc<LinkRuntime>>,
) -> std::result::Result<Role, String> {
    Ok(runtime.role.lock().await.clone().unwrap_or(Role::Idle))
}

#[derive(Debug, Clone, Serialize)]
struct LinkHealthEvent {
    role: &'static str,
    p50_us: u32,
    p99_us: u32,
    events_per_sec: u32,
    clock_offset_ns: i64,
}

#[derive(Debug, Clone, Serialize)]
struct LinkStatus {
    severity: &'static str,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct LayoutEvent {
    side: &'static str,
    monitors: Vec<Monitor>,
}

#[derive(Debug, Clone, Serialize)]
struct LogEntry {
    level: &'static str,
    message: String,
}

fn emit_status(app: &AppHandle, severity: &'static str, text: impl Into<String>) {
    let text = text.into();
    let _ = app.emit(
        "link-status",
        &LinkStatus {
            severity,
            text: text.clone(),
        },
    );
    let _ = app.emit(
        "log-entry",
        &LogEntry {
            level: severity,
            message: text,
        },
    );
}

fn emit_log(app: &AppHandle, level: &'static str, message: impl Into<String>) {
    let _ = app.emit(
        "log-entry",
        &LogEntry {
            level,
            message: message.into(),
        },
    );
}

fn emit_layout(app: &AppHandle, side: &'static str, monitors: &[Monitor]) {
    if let Err(e) = app.emit(
        "layout",
        &LayoutEvent {
            side,
            monitors: monitors.to_vec(),
        },
    ) {
        warn!("emit layout failed: {e}");
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let initial_role: Option<Role> = match (cli.listen.as_deref(), cli.peer.as_deref()) {
        (Some(addr), None) => Some(Role::Receiver {
            listen: addr.to_string(),
            inject: cli.inject,
        }),
        (None, Some(addr)) => Some(Role::Sender {
            peer: addr.to_string(),
        }),
        (Some(_), Some(_)) => return Err(anyhow!("--listen and --peer are mutually exclusive")),
        // No CLI flags = launched from Finder / dock. Boot the GUI in idle and
        // let the user start a link from the Link tab.
        (None, None) => None,
    };

    // Phase 1 permissions preflight. macOS gates the event tap behind
    // Accessibility; if it's missing we still bring up the network layer so
    // users can see the GUI explain the problem. Capture / inject calls
    // surface their own errors when they actually need the permission.
    let perms_ok = permissions_granted();
    if perms_ok {
        if let Err(e) = install_kill_switch() {
            warn!("kill switch install failed: {e:#}");
        } else {
            info!("kill switch installed (Ctrl+Cmd+Shift+Esc / Ctrl+Win+Shift+Esc exits)");
        }
    } else {
        warn!("Accessibility permission missing — kill switch deferred until granted");
    }

    // Identity + paired-peers store can be loaded sync; quinn endpoint creation
    // requires a tokio runtime context (it spawns background tasks), so it's
    // deferred to the Tauri setup callback below.
    let identity =
        pairing::load_or_create_identity().map_err(|e| anyhow!("identity init: {e:#}"))?;
    let identity = Arc::new(identity);
    let paired_peers =
        pairing::load_paired_peers().map_err(|e| anyhow!("paired-peers init: {e:#}"))?;
    let paired_peers = Arc::new(AsyncMutex::new(paired_peers));
    let host_id_hex = identity.host_id_hex();
    let instance_name = pairing::host_label();
    info!(host_id = %host_id_hex, %instance_name, "identity loaded");

    let role_for_setup = initial_role.clone();

    let global_layout = layout::make_shared();
    let link_runtime = Arc::new(LinkRuntime::default());

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(global_layout.clone())
        .manage(link_runtime.clone())
        .invoke_handler(tauri::generate_handler![
            pairing::start_pair_responder,
            pairing::start_pair_initiator,
            pairing::list_paired_peers,
            pairing::cancel_pairing,
            pairing::get_local_identity,
            get_autostart,
            set_autostart,
            get_lock_to_host,
            set_lock_to_host,
            check_permissions,
            request_permissions,
            layout::update_layout,
            start_link,
            stop_link,
            current_role,
            start_advertising,
            stop_advertising,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let role_opt = role_for_setup.clone();
            // Always broadcast a "role" event — Idle when CLI didn't pin one.
            let initial_payload = role_opt.clone().unwrap_or(Role::Idle);
            if let Err(e) = app_handle.emit("role", &initial_payload) {
                warn!("emit role failed: {e}");
            }

            if let Err(e) = install_tray(app) {
                warn!("tray icon install failed: {e:#}");
            }
            emit_log(
                &app_handle,
                "info",
                format!("identity loaded: {instance_name} ({host_id_hex})"),
            );
            if !perms_ok {
                emit_status(
                    &app_handle,
                    "warn",
                    "Accessibility and Input Monitoring permissions required. Grant both in \
                     System Settings → Privacy & Security → Accessibility and Input Monitoring, \
                     then relaunch MouseFly. Network link will still come up below.",
                );
            }

            // Bind the pair endpoint NOW that the tokio runtime is live.
            // Failure here is non-fatal — pairing just won't work this run.
            // quinn::Endpoint::server spawns background tokio tasks, so it has
            // to run inside the runtime context (block_on does that for us).
            let pair_result =
                tauri::async_runtime::block_on(async { Endpoint::server("0.0.0.0:0") });
            match pair_result {
                Ok(pair_endpoint) => {
                    let pair_port = match pair_endpoint.local_port() {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("pair endpoint local_port: {e:#}");
                            return Ok(());
                        }
                    };
                    let pair_fp_hex = pairing::fingerprint_hex(pair_endpoint.cert_der());
                    info!(pair_port, pair_fp = %pair_fp_hex, "pair endpoint up");

                    let pairing_state = PairingState {
                        identity: identity.clone(),
                        instance_name: instance_name.clone(),
                        data_cert_fingerprint_hex: pair_fp_hex.clone(),
                        paired_peers: paired_peers.clone(),
                        pending_code: Arc::new(AsyncMutex::new(None)),
                        pair_endpoint: Arc::new(pair_endpoint),
                    };
                    app.manage(pairing_state);

                    // Default data port matches the convention used by the
                    // `pnpm receiver` script and the Vue UI's Setup form. If
                    // the user picks a different listen addr, mDNS advertises
                    // 7878 anyway — joiners over manual entry can override.
                    const DEFAULT_DATA_PORT: u16 = 7878;
                    let advertise_cfg = AdvertiseConfig {
                        instance_name: instance_name.clone(),
                        port: pair_port,
                        data_port: DEFAULT_DATA_PORT,
                        fingerprint_hex: pair_fp_hex.clone(),
                        host_id_hex: host_id_hex.clone(),
                    };
                    app.manage(Arc::new(HostingRuntime {
                        advertiser: AsyncMutex::new(None),
                        advertise_cfg,
                    }));
                    spawn_pair_daemon(app_handle.clone(), pair_fp_hex);
                }
                Err(e) => {
                    warn!("pair endpoint init failed (pairing disabled): {e:#}");
                }
            }

            // Only kick off run_role when CLI pinned a role; otherwise the
            // GUI starts the link via the start_link command.
            if let Some(role) = role_opt {
                let app_for_task = app_handle.clone();
                let layout = app_handle.state::<SharedLayout>().inner().clone();
                let runtime_clone = {
                    let runtime: tauri::State<Arc<LinkRuntime>> = app_handle.state();
                    runtime.inner().clone()
                };
                let role_for_task = role.clone();
                let handle = tauri::async_runtime::spawn(async move {
                    if let Err(e) = run_role(role_for_task, app_for_task.clone(), layout).await {
                        error!("role task exited: {e:#}");
                        emit_status(&app_for_task, "error", format!("{e:#}"));
                    }
                });
                tauri::async_runtime::block_on(async {
                    *runtime_clone.role.lock().await = Some(role);
                    *runtime_clone.handle.lock().await = Some(handle);
                });
            } else {
                emit_status(
                    &app_handle,
                    "info",
                    "No link configured. Open the Link tab to start a sender or wait for a peer.",
                );
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("tauri: {e}"))
}

#[tauri::command]
async fn get_autostart(app: AppHandle) -> std::result::Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn set_autostart(app: AppHandle, enable: bool) -> std::result::Result<(), String> {
    let mgr = app.autolaunch();
    if enable {
        mgr.enable().map_err(|e| format!("{e:#}"))
    } else {
        mgr.disable().map_err(|e| format!("{e:#}"))
    }
}

#[tauri::command]
fn check_permissions() -> bool {
    permissions_granted()
}

#[tauri::command]
fn request_permissions() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Request both Accessibility and Input Monitoring. Both are required for
        // CGEventTap to work on macOS 10.15+. The process must be relaunched
        // after the user grants either permission.
        let accessibility = mousefly_input::macos::accessibility_request_trust();
        let input_monitoring = mousefly_input::macos::input_monitoring_request();
        accessibility && input_monitoring
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Tray icon with Show / Hide / Quit menu. Click to focus the main window.
///
/// On macOS the tray icon is a **template image** — black-on-transparent with
/// `icon_as_template(true)` — so the OS auto-inverts it to white in dark menu
/// bars. On Windows we use the same monochrome PNG; the tray notification
/// area handles light/dark via Windows 11 personalisation natively.
fn install_tray(app: &tauri::App) -> Result<()> {
    // `include_image!` decodes the PNG at compile time, giving us a
    // 'static Image — no I/O cost or path resolution at startup.
    let icon = tauri::include_image!("icons/tray.png");
    let menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::new("Show").id("tray-show").build(app)?)
        .item(&MenuItemBuilder::new("Hide").id("tray-hide").build(app)?)
        .separator()
        .item(&MenuItemBuilder::new("Quit").id("tray-quit").build(app)?)
        .build()?;
    TrayIconBuilder::with_id("mousefly-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("MouseFly")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => focus_main_window(app),
            "tray-hide" => hide_main_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { .. } = event {
                focus_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn focus_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        w.show().ok();
        w.unminimize().ok();
        w.set_focus().ok();
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        w.hide().ok();
    }
}

/// Hosting runtime: holds the lazily-started mDNS Advertiser. Idle on app
/// launch — only set when the user clicks "Start session" in the GUI.
struct HostingRuntime {
    advertiser: AsyncMutex<Option<mousefly_discovery::Advertiser>>,
    advertise_cfg: AdvertiseConfig,
}

#[tauri::command]
async fn start_advertising(
    runtime: tauri::State<'_, Arc<HostingRuntime>>,
) -> std::result::Result<(), String> {
    let mut guard = runtime.advertiser.lock().await;
    if guard.is_some() {
        return Ok(());
    }
    let adv = mousefly_discovery::Advertiser::start(runtime.advertise_cfg.clone())
        .map_err(|e| format!("advertise: {e:#}"))?;
    *guard = Some(adv);
    Ok(())
}

#[tauri::command]
async fn stop_advertising(
    runtime: tauri::State<'_, Arc<HostingRuntime>>,
) -> std::result::Result<(), String> {
    if let Some(adv) = runtime.advertiser.lock().await.take() {
        // Best-effort: the discovery API returns a Result we don't propagate.
        let _ = adv.stop();
    }
    Ok(())
}

/// Brings up the **passive** discovery side (mDNS browser) and the always-on
/// pair acceptor. Advertising is opt-in via `start_advertising` — see
/// [`HostingRuntime`] — so the app doesn't appear in other devices' lists
/// until the user explicitly hosts a session.
fn spawn_pair_daemon(app: AppHandle, own_fp_hex: String) {
    tauri::async_runtime::spawn(async move {
        let browser = match Browser::start(own_fp_hex) {
            Ok(b) => b,
            Err(e) => {
                warn!("mdns browse failed: {e:#}");
                return;
            }
        };
        let mut events = browser.events();

        // Initial snapshot — usually empty at process start, but include for
        // determinism.
        let snap: Vec<DiscoveredPeer> = browser.snapshot();
        if let Err(e) = app.emit("discovered-peers", &snap) {
            warn!("emit discovered-peers (initial) failed: {e}");
        }

        let pair_state_clone = {
            let pair_state: tauri::State<PairingState> = app.state();
            PairingStateSnapshot {
                pair_endpoint: pair_state.pair_endpoint.clone(),
                paired_peers: pair_state.paired_peers.clone(),
                identity: pair_state.identity.clone(),
                instance_name: pair_state.instance_name.clone(),
                data_cert_fingerprint_hex: pair_state.data_cert_fingerprint_hex.clone(),
                pending_code: pair_state.pending_code.clone(),
            }
        };

        // Pair acceptor.
        let app_for_acc = app.clone();
        tokio::spawn(async move {
            pairing::run_pair_acceptor(
                PairingState {
                    identity: pair_state_clone.identity,
                    instance_name: pair_state_clone.instance_name,
                    data_cert_fingerprint_hex: pair_state_clone.data_cert_fingerprint_hex,
                    paired_peers: pair_state_clone.paired_peers,
                    pending_code: pair_state_clone.pending_code,
                    pair_endpoint: pair_state_clone.pair_endpoint,
                },
                app_for_acc,
            )
            .await;
        });

        // Forward Browser events as updated snapshots — UI keeps a flat list.
        let app_for_events = app.clone();
        loop {
            match events.recv().await {
                Ok(_evt) => {
                    let snap = browser.snapshot();
                    if let Err(e) = app_for_events.emit("discovered-peers", &snap) {
                        warn!("emit discovered-peers failed: {e}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "discovered-peers receiver lagged");
                }
                Err(_) => break,
            }
        }
    });
}

#[derive(Clone)]
struct PairingStateSnapshot {
    pair_endpoint: Arc<Endpoint>,
    paired_peers: Arc<AsyncMutex<mousefly_pair::PairedPeerStore>>,
    identity: Arc<mousefly_pair::Identity>,
    instance_name: String,
    data_cert_fingerprint_hex: String,
    pending_code: Arc<AsyncMutex<Option<pairing::PendingCode>>>,
}

async fn run_role(role: Role, app: AppHandle, layout: SharedLayout) -> Result<()> {
    match role {
        Role::Idle => Ok(()),
        Role::Sender { peer } => run_sender(&peer, app, layout).await,
        Role::Receiver { listen, inject } => run_receiver(&listen, inject, app, layout).await,
    }
}

/// Lock-to-host: when set, the sender drops pointer/key frames before
/// forwarding (RTT and Heartbeat still flow so the link stays warm). Toggled
/// from the GUI via `set_lock_to_host`.
static LOCK_ENGAGED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn get_lock_to_host() -> bool {
    LOCK_ENGAGED.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_lock_to_host(enable: bool, app: AppHandle) {
    LOCK_ENGAGED.store(enable, Ordering::Relaxed);
    let _ = app.emit("lock-to-host", &enable);
}

async fn run_sender(peer: &str, app: AppHandle, layout: SharedLayout) -> Result<()> {
    let backend = Platform::new();
    let monitors = backend.enumerate_monitors().unwrap_or_default();
    emit_layout(&app, "local", &monitors);
    {
        let mut g = layout.write().await;
        g.set_monitors(Side::Local, monitors.clone());
    }

    let clipboard_mark = clipboard::make_watermark();

    // Capture is started exactly once and feeds a tokio broadcast channel so
    // we can transparently re-attach to a fresh Link on each reconnect attempt.
    emit_status(
        &app,
        "info",
        "Requesting input capture (may prompt for permissions)",
    );
    let capture = match backend.start_capture() {
        Ok(c) => c,
        Err(e) => {
            emit_status(&app, "error", format!("Capture failed: {e:#}"));
            return Err(e);
        }
    };
    let (cap_tx, _cap_rx0) = tokio::sync::broadcast::channel::<Frame>(1024);
    let cap_tx_for_pump = cap_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(frame) = capture.recv() {
            let _ = cap_tx_for_pump.send(frame);
        }
    });

    let mut attempt: u32 = 0;
    loop {
        emit_status(&app, "info", format!("Connecting to {peer}…"));
        match connect(peer).await {
            Ok(mut link) => {
                attempt = 0;
                let actual_peer = link.remote_addr.to_string();
                info!("forwarding events to {actual_peer}");
                emit_status(&app, "info", format!("Forwarding events to {actual_peer}"));
                let _ = app.emit("peer-addr", &actual_peer);

                let _ = link
                    .outbound
                    .send(Frame::LayoutUpdate {
                        monitors: monitors.clone(),
                    })
                    .await;

                spawn_health_emitter(app.clone(), link.health.clone(), "sender");
                clipboard::spawn_poller(link.outbound.clone(), clipboard_mark.clone());

                let outbound = link.outbound.clone();
                let mut cap_rx = cap_tx.subscribe();
                let layout_for_pump = layout.clone();
                let app_for_pump = app.clone();
                let pump = tokio::spawn(async move {
                    let mut last_cursor = (0f32, 0f32);
                    let mut edge = layout::EdgeState::default();
                    let mut was_on_remote = false;
                    loop {
                        match cap_rx.recv().await {
                            Ok(frame) => {
                                if LOCK_ENGAGED.load(Ordering::Relaxed)
                                    && !matches!(
                                        frame,
                                        Frame::Heartbeat
                                            | Frame::RttProbe { .. }
                                            | Frame::RttReply { .. }
                                    )
                                {
                                    continue;
                                }
                                let gated = layout::gate_outbound(
                                    frame,
                                    &layout_for_pump,
                                    &mut last_cursor,
                                    &mut edge,
                                )
                                .await;
                                if edge.on_remote != was_on_remote {
                                    was_on_remote = edge.on_remote;
                                    let msg = if edge.on_remote {
                                        "Edge crossing: cursor entered remote"
                                    } else {
                                        "Edge crossing: cursor returned to local"
                                    };
                                    emit_log(&app_for_pump, "info", msg);
                                }
                                if let Some(frame) = gated {
                                    if outbound.send(frame).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                });

                while let Some(inbound) = link.inbound.recv().await {
                    match inbound.frame {
                        Frame::LayoutUpdate { monitors } => {
                            info!(
                                count = monitors.len(),
                                "sender received remote LayoutUpdate"
                            );
                            {
                                let mut g = layout.write().await;
                                g.set_monitors(Side::Remote, monitors.clone());
                            }
                            emit_layout(&app, "remote", &monitors);
                            emit_log(
                                &app,
                                "info",
                                format!("Remote layout: {} monitor(s)", monitors.len()),
                            );
                        }
                        Frame::Clipboard { text } => {
                            clipboard::apply(text, &clipboard_mark).await;
                        }
                        other => debug!(?other, "sender saw inbound frame (ignored)"),
                    }
                }
                pump.abort();
                emit_status(&app, "warn", "Link dropped — reconnecting…");
                let _ = app.emit("link-dropped", &());
            }
            Err(e) => {
                emit_status(&app, "warn", format!("Connect failed: {e:#} — retrying"));
            }
        }

        // Exponential backoff: 250 ms, 500, 1s, 2s, 4s, capped.
        let backoff_ms = 250u64 << attempt.min(4);
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
    }
}

async fn run_receiver(
    addr: &str,
    inject: bool,
    app: AppHandle,
    layout: SharedLayout,
) -> Result<()> {
    let backend = Platform::new();
    let monitors = backend.enumerate_monitors().unwrap_or_default();
    emit_layout(&app, "local", &monitors);
    {
        let mut g = layout.write().await;
        g.set_monitors(Side::Local, monitors.clone());
    }

    let clipboard_mark = clipboard::make_watermark();

    loop {
        info!("waiting for sender on {addr}");
        emit_status(&app, "info", format!("Listening on {addr} for sender…"));
        match serve(addr).await {
            Ok(mut link) => {
                let actual_peer = link.remote_addr.to_string();
                info!(inject, peer = %actual_peer, "link up; receiving events");
                emit_status(
                    &app,
                    "info",
                    format!(
                        "Sender {actual_peer} connected; injection {}",
                        if inject { "ON" } else { "off" }
                    ),
                );
                let _ = app.emit("peer-addr", &actual_peer);
                let _ = link
                    .outbound
                    .send(Frame::LayoutUpdate {
                        monitors: monitors.clone(),
                    })
                    .await;
                spawn_health_emitter(app.clone(), link.health.clone(), "receiver");
                clipboard::spawn_poller(link.outbound.clone(), clipboard_mark.clone());

                while let Some(inbound) = link.inbound.recv().await {
                    match inbound.frame {
                        Frame::LayoutUpdate { monitors } => {
                            {
                                let mut g = layout.write().await;
                                g.set_monitors(Side::Remote, monitors.clone());
                            }
                            emit_layout(&app, "remote", &monitors);
                        }
                        Frame::Clipboard { text } => {
                            clipboard::apply(text, &clipboard_mark).await;
                        }
                        Frame::PointerOnMonitor {
                            monitor,
                            mm_x,
                            mm_y,
                            ..
                        } if inject => {
                            static POM_COUNT: std::sync::atomic::AtomicU32 =
                                std::sync::atomic::AtomicU32::new(0);
                            let c = POM_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if c < 3 {
                                info!(?monitor, mm_x, mm_y, "receiver got PointerOnMonitor");
                            }
                            let resolved = {
                                let g = layout.read().await;
                                g.pointer_on_monitor_to_local(&inbound.frame)
                            };
                            if let Some(frame) = resolved {
                                if let Err(e) = backend.inject(&frame) {
                                    warn!("inject (mapped) failed: {e:#}");
                                }
                            } else {
                                warn!("PointerOnMonitor: no matching local monitor");
                            }
                        }
                        ref f if inject => {
                            if let Err(e) = backend.inject(f) {
                                warn!("inject failed: {e:#}");
                            }
                        }
                        _ => {}
                    }
                }
                emit_status(&app, "warn", "Sender disconnected — waiting for next");
                let _ = app.emit("link-dropped", &());
            }
            Err(e) => {
                emit_status(&app, "warn", format!("Listen failed: {e:#} — retrying"));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

fn spawn_health_emitter(
    app: AppHandle,
    mut rx: tokio::sync::watch::Receiver<LinkHealth>,
    role: &'static str,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let h = *rx.borrow();
            let payload = LinkHealthEvent {
                role,
                p50_us: h.latency_p50_us,
                p99_us: h.latency_p99_us,
                events_per_sec: h.events_per_sec,
                clock_offset_ns: h.clock_offset_ns,
            };
            if let Err(e) = app.emit("link-health", &payload) {
                warn!("emit link-health failed: {e}");
            }
        }
    });
}
