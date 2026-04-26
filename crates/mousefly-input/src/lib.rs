//! Per-OS input capture and injection backends behind a single trait.
//!
//! - macOS: `CGEventTap` + `CGEventPost` + `CGGetActiveDisplayList` (this file
//!   level + `macos.rs`).
//! - Windows: `SetWindowsHookEx` + `SendInput` + `EnumDisplayMonitors`
//!   (`windows.rs`, gated on `cfg(target_os = "windows")`).
//! - Linux: not implemented yet (Phase 5).

use anyhow::Result;
use crossbeam_channel::Receiver;
use mousefly_core::{Frame, Monitor};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacBackend as Platform;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::WinBackend as Platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub mod stub;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use stub::StubBackend as Platform;

/// Common interface for OS-specific input plumbing.
pub trait InputBackend: Send + Sync {
    /// Enumerate all monitors attached to this host.
    fn enumerate_monitors(&self) -> Result<Vec<Monitor>>;

    /// Begin emitting captured pointer / scroll / button / key frames on the
    /// returned channel. The capture cannot be stopped without exiting the
    /// process — the kill switch (see [`install_kill_switch`]) is the
    /// user-facing escape.
    fn start_capture(&self) -> Result<Receiver<Frame>>;

    /// Apply an inbound frame to the local OS state. Non-input frames are
    /// silently ignored.
    fn inject(&self, frame: &Frame) -> Result<()>;
}

/// Install the kill-switch tap on the current OS:
/// - macOS: `Ctrl+Cmd+Shift+Esc`
/// - Windows: `Ctrl+Win+Shift+Esc`
///
/// MUST be called before any [`InputBackend::start_capture`] so the user
/// always has an escape hatch (AGENTS.md hard rule #1). Blocks until the tap
/// is verified live OR returns the install error.
pub fn install_kill_switch() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::install_kill_switch()
    }
    #[cfg(target_os = "windows")]
    {
        windows::install_kill_switch()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("install_kill_switch: no backend for this OS")
    }
}

/// macOS-only: returns true if the process has Accessibility permission.
/// Returns true on non-macOS as a no-op (other OSes don't gate input capture
/// the same way; Windows uses elevation, Linux uses portals).
pub fn permissions_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::accessibility_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}
