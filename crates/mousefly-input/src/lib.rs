//! Per-OS input capture and injection backends behind a single trait.
//!
//! Phase 0: macOS only via `CGEventTap` + `CGEventPost`. Phase 1 adds Windows
//! via `SetWindowsHookEx` + `SendInput` and gates the platform pick on
//! `#[cfg(target_os = ...)]`.

use anyhow::Result;
use crossbeam_channel::Receiver;
use mousefly_core::Frame;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacBackend as Platform;

/// Common interface for OS-specific input plumbing.
pub trait InputBackend: Send + Sync {
    /// Begin emitting captured pointer/scroll/button frames on the returned
    /// channel. The capture cannot be stopped without exiting the process —
    /// the kill switch (see [`install_kill_switch`]) is the user-facing escape.
    fn start_capture(&self) -> Result<Receiver<Frame>>;

    /// Apply an inbound frame to the local OS state. Non-input frames are
    /// silently ignored.
    fn inject(&self, frame: &Frame) -> Result<()>;
}

/// Install the kill-switch tap (`Ctrl+Cmd+Shift+Esc` exits the process). MUST
/// be called before [`InputBackend::start_capture`] so the user always has an
/// escape hatch — see AGENTS.md hard rule #1.
#[cfg(target_os = "macos")]
pub fn install_kill_switch() -> Result<()> {
    macos::install_kill_switch()
}
