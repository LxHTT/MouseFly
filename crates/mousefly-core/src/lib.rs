//! Shared types for MouseFly: wire-protocol payloads, input bitmasks, and
//! monitor descriptors. Pure data, no I/O.

pub mod keymap;

use serde::{Deserialize, Serialize};

pub type Buttons = u32;
pub type Modifiers = u32;

pub mod button {
    pub const LEFT: u32 = 1 << 0;
    pub const RIGHT: u32 = 1 << 1;
    pub const MIDDLE: u32 = 1 << 2;
}

pub mod modifier {
    pub const SHIFT: u32 = 1 << 0;
    pub const CTRL: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    /// Cmd on macOS, Win on Windows, Super on Linux.
    pub const META: u32 = 1 << 3;
}

/// Stable identity of a monitor across reconnects. Preferred fill is the
/// EDID hash (when available); fallback is `(connector, resolution, position)`
/// hashed. The actual hashing happens in the per-OS input backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonitorId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    /// Logical pixels (points), as the OS reports them.
    pub logical_size_px: (u32, u32),
    /// OS-reported HiDPI scale (1.0, 2.0, ...).
    pub scale_factor: f32,
    /// Physical size from EDID, when available. None for monitors that don't
    /// report it or report something implausible.
    pub physical_size_mm: Option<(u32, u32)>,
    /// Position in the host's local virtual desktop.
    pub position_in_local_vd: (i32, i32),
    pub primary: bool,
}

/// One unit of payload on the wire. The net layer wraps each in an envelope
/// with a monotonic timestamp for latency measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// Absolute pointer position in the sender's screen coordinates (points).
    /// `dx`/`dy` are the raw hardware deltas from the OS (not clamped to screen
    /// bounds) — used sender-side for edge-crossing virtual cursor movement.
    PointerAbs {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        buttons: Buttons,
    },
    /// Pointer position on a specific remote monitor, in millimetres from the
    /// monitor's top-left. The receiver looks up the monitor by its
    /// `MonitorId` (matched against its own enumeration) and converts mm back
    /// to local pixels using the recipient monitor's DPI / physical size.
    PointerOnMonitor {
        monitor: MonitorId,
        mm_x: f32,
        mm_y: f32,
        buttons: Buttons,
    },
    /// Mouse button state. Carries the full button mask after the change.
    MouseButton { buttons: Buttons },
    /// Scroll wheel delta (points).
    Scroll { dx: f32, dy: f32 },
    /// Physical key event. `code` is the OS-native scancode for now —
    /// cross-OS HID translation lands in a later phase. `modifiers` is the
    /// authoritative modifier mask AFTER the event (eliminates stuck-mod bugs
    /// from a dropped key-up).
    Key {
        code: u32,
        down: bool,
        modifiers: Modifiers,
    },
    /// Cheap keep-alive; doubles as an idle latency sample.
    Heartbeat,
    /// First leg of a 4-timestamp RTT probe. `t1_ns` is the sender's monotonic
    /// clock at send time.
    RttProbe { id: u32, t1_ns: u64 },
    /// Reply to an `RttProbe`. `t2_ns` is the receiver's clock at receive time;
    /// `t3_ns` is the receiver's clock at reply send time.
    RttReply {
        id: u32,
        t1_ns: u64,
        t2_ns: u64,
        t3_ns: u64,
    },
    /// Sender publishes its monitor set so the receiver can render the layout
    /// and reason about edge mappings. Sent on connect and on hot-plug.
    LayoutUpdate { monitors: Vec<Monitor> },
    /// Plain-text clipboard contents. Goes on the control stream (reliable,
    /// in-order). Loopback safety is the sender's job — see
    /// `mousefly-app/src/clipboard.rs`.
    Clipboard { text: String },
    /// Notifies peers that this host is exiting the session. Sent on graceful
    /// shutdown so peers can update their UI and clean up state.
    SessionExit,
}
