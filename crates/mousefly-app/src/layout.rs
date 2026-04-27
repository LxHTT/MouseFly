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
use tracing::{debug, info};

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
    info!(
        local_offset = ?args.local_offset,
        remote_offset = ?args.remote_offset,
        "layout offsets updated from Vue"
    );
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
        self.auto_arrange_if_needed();
    }

    /// When both sides have monitors but offsets are still at the default
    /// (0,0)/(0,0), place remote flush to the right of local so edge-crossing
    /// works out of the box without waiting for the Vue canvas push.
    fn auto_arrange_if_needed(&mut self) {
        if self.local.monitors.is_empty() || self.remote.monitors.is_empty() {
            return;
        }
        if self.local.offset != (0.0, 0.0) || self.remote.offset != (0.0, 0.0) {
            return;
        }
        let mut max_x: f32 = 0.0;
        for m in &self.local.monitors {
            let right = m.position_in_local_vd.0 as f32 + m.logical_size_px.0 as f32;
            if right > max_x {
                max_x = right;
            }
        }
        self.remote.offset = (max_x, 0.0);
        info!(
            remote_offset = ?self.remote.offset,
            "auto-arranged remote to right of local"
        );
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
            dx: 0.0,
            dy: 0.0,
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
        Some((mm_w, mm_h))
            if mm_w > 0 && mm_h > 0 && m.logical_size_px.0 > 0 && m.logical_size_px.1 > 0 =>
        {
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

/// Edge-crossing state for the sender. Tracks whether the virtual cursor
/// is "on remote" and its position there.
#[derive(Debug, Default)]
pub struct EdgeState {
    /// True when the virtual cursor has crossed onto a remote monitor.
    pub on_remote: bool,
    /// Virtual cursor position in GVL pixels (only meaningful when on_remote).
    pub virt_gvl_x: f32,
    pub virt_gvl_y: f32,
}

impl GlobalLayout {
    /// Bounding box of all local monitors in LVD pixels.
    fn local_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        if self.local.monitors.is_empty() {
            return None;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for m in &self.local.monitors {
            let x = m.position_in_local_vd.0 as f32;
            let y = m.position_in_local_vd.1 as f32;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + m.logical_size_px.0 as f32);
            max_y = max_y.max(y + m.logical_size_px.1 as f32);
        }
        Some((min_x, min_y, max_x, max_y))
    }

    /// Bounding box of all remote monitors in GVL pixels.
    fn remote_bounds_gvl(&self) -> Option<(f32, f32, f32, f32)> {
        if self.remote.monitors.is_empty() {
            return None;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for m in &self.remote.monitors {
            let x = self.remote.offset.0 + m.position_in_local_vd.0 as f32;
            let y = self.remote.offset.1 + m.position_in_local_vd.1 as f32;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + m.logical_size_px.0 as f32);
            max_y = max_y.max(y + m.logical_size_px.1 as f32);
        }
        Some((min_x, min_y, max_x, max_y))
    }
}

/// Sender-side gate with edge-crossing support.
///
/// When the user's cursor hits the edge of the local screen that faces the
/// remote host, we enter "remote mode": deltas are accumulated onto a virtual
/// cursor that lives in the remote monitor space. Non-pointer frames (keys,
/// scroll, buttons) are forwarded while on remote and suppressed while on
/// local.
///
/// When the virtual cursor returns past the crossing edge, we exit remote
/// mode and stop forwarding.
pub async fn gate_outbound(
    frame: Frame,
    layout: &SharedLayout,
    last_cursor: &mut (f32, f32),
    edge: &mut EdgeState,
) -> Option<Frame> {
    let (new_x, new_y, raw_dx, raw_dy, buttons) = match frame {
        Frame::PointerAbs {
            x,
            y,
            dx,
            dy,
            buttons,
        } => (x, y, dx, dy, buttons),
        Frame::Heartbeat | Frame::RttProbe { .. } | Frame::RttReply { .. } => {
            return Some(frame);
        }
        _ => {
            // Non-pointer input frames: forward only when on remote.
            return if edge.on_remote { Some(frame) } else { None };
        }
    };

    let g = layout.read().await;

    if g.remote.monitors.is_empty() {
        *last_cursor = (new_x, new_y);
        return Some(frame);
    }

    // When both offsets are (0,0) and no layout has been arranged, monitors
    // overlap. Fall back to forwarding everything so Phase 0/2 paths work.
    if g.local.offset == (0.0, 0.0) && g.remote.offset == (0.0, 0.0) {
        let (gvl_x, gvl_y) = g.local_to_gvl(new_x, new_y);
        *last_cursor = (new_x, new_y);
        if let Some((monitor, mm_x, mm_y)) = g.gvl_to_remote_mm(gvl_x, gvl_y) {
            edge.on_remote = true;
            return Some(Frame::PointerOnMonitor {
                monitor,
                mm_x,
                mm_y,
                buttons,
            });
        }
        edge.on_remote = false;
        return None;
    }

    // --- Edge-crossing logic (layout has been arranged) ---

    let local_bounds = match g.local_bounds() {
        Some(b) => b,
        None => {
            *last_cursor = (new_x, new_y);
            return Some(frame);
        }
    };
    let remote_gvl = match g.remote_bounds_gvl() {
        Some(b) => b,
        None => {
            *last_cursor = (new_x, new_y);
            return None;
        }
    };

    // Use raw hardware deltas — the OS clamps the absolute position at screen
    // edges, making position-based deltas zero when the cursor is pinned.
    let dx = raw_dx;
    let dy = raw_dy;
    *last_cursor = (new_x, new_y);

    let (l_min_x, l_min_y, l_max_x, l_max_y) = local_bounds;
    let edge_margin = 2.0;

    if edge.on_remote {
        edge.virt_gvl_x += dx;
        edge.virt_gvl_y += dy;

        // Clamp virtual cursor to remote bounds.
        edge.virt_gvl_x = edge.virt_gvl_x.clamp(remote_gvl.0, remote_gvl.2 - 1.0);
        edge.virt_gvl_y = edge.virt_gvl_y.clamp(remote_gvl.1, remote_gvl.3 - 1.0);

        // Check if the virtual cursor has returned past the crossing edge
        // toward local. Use the local bounds in GVL to detect this.
        let local_gvl_min_x = g.local.offset.0 + l_min_x;
        let local_gvl_max_x = g.local.offset.0 + l_max_x;
        let local_gvl_min_y = g.local.offset.1 + l_min_y;
        let local_gvl_max_y = g.local.offset.1 + l_max_y;

        // If remote is to the right and virtual cursor moved left past the
        // boundary, or remote is to the left and cursor moved right past it,
        // exit remote mode.
        let back_to_local = if remote_gvl.0 >= local_gvl_max_x - edge_margin {
            // Remote is to the right: exit when virt moves left of remote left edge
            edge.virt_gvl_x <= remote_gvl.0 && dx < 0.0
        } else if remote_gvl.2 <= local_gvl_min_x + edge_margin {
            // Remote is to the left
            edge.virt_gvl_x >= remote_gvl.2 - 1.0 && dx > 0.0
        } else if remote_gvl.1 >= local_gvl_max_y - edge_margin {
            // Remote is below
            edge.virt_gvl_y <= remote_gvl.1 && dy < 0.0
        } else if remote_gvl.3 <= local_gvl_min_y + edge_margin {
            // Remote is above
            edge.virt_gvl_y >= remote_gvl.3 - 1.0 && dy > 0.0
        } else {
            false
        };

        if back_to_local {
            edge.on_remote = false;
            return None;
        }

        if let Some((monitor, mm_x, mm_y)) = g.gvl_to_remote_mm(edge.virt_gvl_x, edge.virt_gvl_y) {
            return Some(Frame::PointerOnMonitor {
                monitor,
                mm_x,
                mm_y,
                buttons,
            });
        }
        return None;
    }

    // --- Not on remote: check if cursor hit a local screen edge ---

    let at_right = new_x >= l_max_x - edge_margin;
    let at_left = new_x <= l_min_x + edge_margin;
    let at_bottom = new_y >= l_max_y - edge_margin;
    let at_top = new_y <= l_min_y + edge_margin;

    let local_gvl_max_x = g.local.offset.0 + l_max_x;
    let local_gvl_min_x = g.local.offset.0 + l_min_x;
    let local_gvl_max_y = g.local.offset.1 + l_max_y;
    let local_gvl_min_y = g.local.offset.1 + l_min_y;

    // Determine if the remote is adjacent to the edge the cursor hit.
    let cross = if at_right && remote_gvl.0 >= local_gvl_max_x - edge_margin {
        let entry_y = g.local.offset.1 + new_y;
        if entry_y >= remote_gvl.1 && entry_y < remote_gvl.3 {
            Some((remote_gvl.0 + 1.0, entry_y))
        } else {
            None
        }
    } else if at_left && remote_gvl.2 <= local_gvl_min_x + edge_margin {
        let entry_y = g.local.offset.1 + new_y;
        if entry_y >= remote_gvl.1 && entry_y < remote_gvl.3 {
            Some((remote_gvl.2 - 2.0, entry_y))
        } else {
            None
        }
    } else if at_bottom && remote_gvl.1 >= local_gvl_max_y - edge_margin {
        let entry_x = g.local.offset.0 + new_x;
        if entry_x >= remote_gvl.0 && entry_x < remote_gvl.2 {
            Some((entry_x, remote_gvl.1 + 1.0))
        } else {
            None
        }
    } else if at_top && remote_gvl.3 <= local_gvl_min_y + edge_margin {
        let entry_x = g.local.offset.0 + new_x;
        if entry_x >= remote_gvl.0 && entry_x < remote_gvl.2 {
            Some((entry_x, remote_gvl.3 - 2.0))
        } else {
            None
        }
    } else {
        None
    };

    if let Some((gvl_x, gvl_y)) = cross {
        edge.on_remote = true;
        edge.virt_gvl_x = gvl_x;
        edge.virt_gvl_y = gvl_y;
        debug!(gvl_x, gvl_y, "edge crossing → entered remote");
        if let Some((monitor, mm_x, mm_y)) = g.gvl_to_remote_mm(gvl_x, gvl_y) {
            return Some(Frame::PointerOnMonitor {
                monitor,
                mm_x,
                mm_y,
                buttons,
            });
        }
    }

    None
}
