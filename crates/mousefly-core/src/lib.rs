//! Shared types for MouseFly: wire-protocol payloads and input bitmasks.
//! Pure data, no I/O. Multi-monitor enumeration types arrive in Phase 1.

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
    pub const META: u32 = 1 << 3;
}

/// One unit of payload on the wire. The net layer wraps each in a `WireFrame`
/// with a monotonic timestamp for latency measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// Absolute pointer position in the sender's screen coordinates (points).
    /// Phase 0 assumes both hosts share the same coordinate space.
    PointerAbs { x: f32, y: f32, buttons: Buttons },
    /// Mouse button state change. Carries the full button mask after the change.
    MouseButton { buttons: Buttons },
    /// Scroll wheel delta (points).
    Scroll { dx: f32, dy: f32 },
    /// Cheap keep-alive; doubles as an idle latency sample.
    Heartbeat,
    /// First leg of a 4-timestamp RTT probe. `t1_ns` is the sender's monotonic
    /// clock at send time.
    RttProbe { id: u32, t1_ns: u64 },
    /// Reply to an `RttProbe`. `t2_ns` is the receiver's clock at receive time;
    /// `t3_ns` is the receiver's clock at reply send time.
    RttReply { id: u32, t1_ns: u64, t2_ns: u64, t3_ns: u64 },
}
