//! Phase 3.5: global virtual layout (GVL).
//!
//! The Vue canvas lets the user drag two host groups around in a shared
//! 2-D space. We mirror that arrangement here so the sender can decide,
//! per captured frame, whether the cursor is on its own host (suppress)
//! or on the peer (forward as `Frame::PointerOnMonitor`).
//!
//! Coordinate spaces:
//! - **Host-local virtual desktop (LVD)**: what the OS reports — captured
//!   pointer positions are in this space, sender-side.
//! - **Global virtual layout (GVL)**: GVL = LVD + per-host offset. Both
//!   hosts arrange themselves in one shared GVL. Offsets come from the
//!   `update_layout` Tauri command, populated by the Vue canvas.

use std::sync::Arc;

use mousefly_core::{Frame, Monitor, MonitorId};
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tokio::sync::RwLock;

const DEFAULT_MM_PER_PX: f32 = 25.4 / 96.0;

#[derive(Debug, Clone, Default)]
pub struct HostState {
    pub offset: (f32, f32),
    pub monitors: Vec<Monitor>,
}

#[derive(Debug, Clone, Default)]
pub struct GlobalLayout {
    pub local: HostState,
    pub remote: HostState,
}

pub type SharedLayout = Arc<RwLock<GlobalLayout>>;

pub fn make_shared() -> SharedLayout {
    Arc::new(RwLock::new(GlobalLayout::default()))
}

#[derive(Debug, Deserialize)]
pub struct UpdateLayoutArgs {
    pub local_offset: (f32, f32),
    pub remote_offset: (f32, f32),
}

#[tauri::command]
pub async fn update_layout(
    app: AppHandle,
    args: UpdateLayoutArgs,
) -> std::result::Result<(), String> {
    let shared: tauri::State<SharedLayout> = app.state();
    let mut g = shared.write().await;
    g.local.offset = args.local_offset;
    g.remote.offset = args.remote_offset;
    Ok(())
}

impl GlobalLayout {
    /// Update our knowledge of one side's monitor set. Called when:
    /// - the local host enumerates monitors at startup,
    /// - or a `Frame::LayoutUpdate` arrives from the peer.
    pub fn set_monitors(&mut self, side: Side, monitors: Vec<Monitor>) {
        match side {
            Side::Local => self.local.monitors = monitors,
            Side::Remote => self.remote.monitors = monitors,
        }
    }

    /// Translate a sender-local pointer position (LVD pixels) into GVL.
    pub fn local_to_gvl(&self, x: f32, y: f32) -> (f32, f32) {
        (x + self.local.offset.0, y + self.local.offset.1)
    }

    /// Find the remote monitor (and mm-position within it) that contains
    /// `gvl_pos`, if any.
    pub fn gvl_to_remote_mm(&self, gvl_x: f32, gvl_y: f32) -> Option<(MonitorId, f32, f32)> {
        for m in &self.remote.monitors {
            let mx = self.remote.offset.0 + m.position_in_local_vd.0 as f32;
            let my = self.remote.offset.1 + m.position_in_local_vd.1 as f32;
            let mw = m.logical_size_px.0 as f32;
            let mh = m.logical_size_px.1 as f32;
            if gvl_x >= mx && gvl_x < mx + mw && gvl_y >= my && gvl_y < my + mh {
                let (mm_per_px_x, mm_per_px_y) = mm_per_pixel(m);
                let mm_x = (gvl_x - mx) * mm_per_px_x;
                let mm_y = (gvl_y - my) * mm_per_px_y;
                return Some((m.id, mm_x, mm_y));
            }
        }
        None
    }

    /// Receiver-side: translate `Frame::PointerOnMonitor` into a
    /// `Frame::PointerAbs` (LVD pixels) the input backend can inject. Returns
    /// `None` if we have no monitor matching that id.
    pub fn pointer_on_monitor_to_local(&self, frame: &Frame) -> Option<Frame> {
        let (monitor, mm_x, mm_y, buttons) = match *frame {
            Frame::PointerOnMonitor {
                monitor,
                mm_x,
                mm_y,
                buttons,
            } => (monitor, mm_x, mm_y, buttons),
            _ => return None,
        };
        let m = self.local.monitors.iter().find(|m| m.id == monitor)?;
        let (mm_per_px_x, mm_per_px_y) = mm_per_pixel(m);
        let px_x = m.position_in_local_vd.0 as f32 + mm_x / mm_per_px_x;
        let px_y = m.position_in_local_vd.1 as f32 + mm_y / mm_per_px_y;
        Some(Frame::PointerAbs {
            x: px_x,
            y: px_y,
            buttons,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Side {
    Local,
    Remote,
}

fn mm_per_pixel(m: &Monitor) -> (f32, f32) {
    match m.physical_size_mm {
        Some((mm_w, mm_h)) if mm_w > 0 && mm_h > 0 && m.logical_size_px.0 > 0 && m.logical_size_px.1 > 0 => {
            let scale = m.scale_factor.max(1.0);
            // physical_size_mm is reported in physical pixels' worth of mm.
            // logical_size_px is points. Convert: mm-per-point = mm / (logical * scale).
            let phys_w = mm_w as f32 / (m.logical_size_px.0 as f32 * scale);
            let phys_h = mm_h as f32 / (m.logical_size_px.1 as f32 * scale);
            (phys_w * scale, phys_h * scale)
        }
        _ => (DEFAULT_MM_PER_PX, DEFAULT_MM_PER_PX),
    }
}

/// Sender-side gate: given a captured frame and the current cursor LVD
/// position (state owned by the caller), return the frame to forward to the
/// peer, or `None` if we should drop it because the cursor is on our own host.
///
/// PointerAbs is rewritten into PointerOnMonitor when the cursor is on a
/// remote monitor; other input frames (button / scroll / key) pass through
/// unchanged but only when the cursor is on remote.
pub async fn gate_outbound(
    frame: Frame,
    layout: &SharedLayout,
    last_cursor: &mut (f32, f32),
) -> Option<Frame> {
    if let Frame::PointerAbs { x, y, .. } = &frame {
        *last_cursor = (*x, *y);
    }
    let g = layout.read().await;
    // Empty layouts (Vue hasn't pushed offsets yet, no remote monitors) →
    // fall back to forwarding everything as-is so Phase 0 / Phase 2 paths
    // keep working for users who haven't arranged their layout yet.
    if g.remote.monitors.is_empty() {
        return Some(frame);
    }
    let (gvl_x, gvl_y) = g.local_to_gvl(last_cursor.0, last_cursor.1);
    match g.gvl_to_remote_mm(gvl_x, gvl_y) {
        Some((monitor, mm_x, mm_y)) => match frame {
            Frame::PointerAbs { buttons, .. } => Some(Frame::PointerOnMonitor {
                monitor,
                mm_x,
                mm_y,
                buttons,
            }),
            _ => Some(frame),
        },
        None => None,
    }
}
