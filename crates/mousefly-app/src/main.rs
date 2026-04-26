//! MouseFly app binary — Tauri shell + role wiring.
//!
//! Roles:
//!   - sender:   `pnpm dev -- -- --peer 192.168.x.y:7878`
//!   - receiver: `pnpm dev -- -- --listen 0.0.0.0:7878 [--inject]`
//!
//! Loopback safety: `--inject` defaults off so running both roles on the same
//! Mac (`--listen :7878` and `--peer 127.0.0.1:7878`) doesn't feedback-loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{anyhow, Result};
use clap::Parser;
use mousefly_core::{Frame, Monitor};
use mousefly_input::{install_kill_switch, permissions_granted, InputBackend, Platform};
use mousefly_net::{connect, serve, LinkHealth};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::{debug, error, info, warn};

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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Role {
    Sender { peer: String },
    Receiver { listen: String, inject: bool },
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

fn emit_status(app: &AppHandle, severity: &'static str, text: impl Into<String>) {
    let payload = LinkStatus {
        severity,
        text: text.into(),
    };
    if let Err(e) = app.emit("link-status", &payload) {
        warn!("emit link-status failed: {e}");
    }
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
    let role = match (cli.listen.as_deref(), cli.peer.as_deref()) {
        (Some(addr), None) => Role::Receiver {
            listen: addr.to_string(),
            inject: cli.inject,
        },
        (None, Some(addr)) => Role::Sender {
            peer: addr.to_string(),
        },
        (Some(_), Some(_)) => return Err(anyhow!("--listen and --peer are mutually exclusive")),
        (None, None) => return Err(anyhow!("must specify --listen or --peer")),
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

    let role_for_setup = role.clone();
    tauri::Builder::default()
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let role = role_for_setup.clone();
            if let Err(e) = app_handle.emit("role", &role) {
                warn!("emit role failed: {e}");
            }
            if !perms_ok {
                emit_status(
                    &app_handle,
                    "warn",
                    "Accessibility / Input Monitoring permission missing. Grant both in \
                     System Settings → Privacy & Security, then relaunch MouseFly. \
                     Network link will still come up below.",
                );
            }
            tauri::async_runtime::spawn(async move {
                if let Err(e) = run_role(role, app_handle.clone()).await {
                    error!("role task exited: {e:#}");
                    emit_status(&app_handle, "error", format!("{e:#}"));
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow!("tauri: {e}"))
}

async fn run_role(role: Role, app: AppHandle) -> Result<()> {
    match role {
        Role::Sender { peer } => run_sender(&peer, app).await,
        Role::Receiver { listen, inject } => run_receiver(&listen, inject, app).await,
    }
}

async fn run_sender(peer: &str, app: AppHandle) -> Result<()> {
    let backend = Platform::new();

    // Publish our local monitor set to the GUI immediately — useful even
    // before the link is up.
    let monitors = backend.enumerate_monitors().unwrap_or_default();
    emit_layout(&app, "local", &monitors);

    emit_status(&app, "info", format!("Connecting to {peer}…"));
    let mut link = connect(peer).await?;
    info!("link up; dialing capture");
    emit_status(
        &app,
        "info",
        "Link up — requesting input capture (may prompt for permissions)",
    );
    let capture = match backend.start_capture() {
        Ok(c) => c,
        Err(e) => {
            emit_status(&app, "error", format!("Capture failed: {e:#}"));
            return Err(e);
        }
    };
    info!("forwarding events to {peer}");
    emit_status(&app, "info", format!("Forwarding events to {peer}"));

    // Send our layout to the peer so they can render the global arrangement.
    let _ = link
        .outbound
        .send(Frame::LayoutUpdate {
            monitors: monitors.clone(),
        })
        .await;

    spawn_health_emitter(app.clone(), link.health.clone(), "sender");

    let outbound = link.outbound.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(frame) = capture.recv() {
            if outbound.blocking_send(frame).is_err() {
                break;
            }
        }
    });

    while let Some(inbound) = link.inbound.recv().await {
        match inbound.frame {
            Frame::LayoutUpdate { monitors } => emit_layout(&app, "remote", &monitors),
            other => debug!(?other, "sender saw inbound frame (ignored)"),
        }
    }
    Ok(())
}

async fn run_receiver(addr: &str, inject: bool, app: AppHandle) -> Result<()> {
    let backend = Platform::new();
    let monitors = backend.enumerate_monitors().unwrap_or_default();
    emit_layout(&app, "local", &monitors);

    info!("waiting for sender on {addr}");
    emit_status(&app, "info", format!("Listening on {addr} for sender…"));
    let mut link = serve(addr).await?;
    info!(inject, "link up; receiving events");
    emit_status(
        &app,
        "info",
        format!(
            "Sender connected; injection {}",
            if inject { "ON" } else { "off" }
        ),
    );

    let _ = link.outbound.send(Frame::LayoutUpdate { monitors }).await;

    spawn_health_emitter(app.clone(), link.health.clone(), "receiver");

    while let Some(inbound) = link.inbound.recv().await {
        match inbound.frame {
            Frame::LayoutUpdate { monitors } => emit_layout(&app, "remote", &monitors),
            ref f if inject => {
                if let Err(e) = backend.inject(f) {
                    warn!("inject failed: {e:#}");
                }
            }
            _ => {}
        }
    }
    Ok(())
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
