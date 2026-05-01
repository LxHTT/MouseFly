//! Fallback backend for OSes without a real implementation. Compiles on Linux
//! so the workspace builds end-to-end; every method returns NotImplemented.
//! Phase 5 adds real X11 / Wayland backends and removes this.

use anyhow::{bail, Result};
use crossbeam_channel::Receiver;
use mousefly_core::{Frame, Monitor};

use crate::InputBackend;

pub struct StubBackend;

impl StubBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for StubBackend {
    fn enumerate_monitors(&self) -> Result<Vec<Monitor>> {
        Ok(Vec::new())
    }

    fn start_capture(&self) -> Result<Receiver<Frame>> {
        bail!("input capture is not yet implemented on this OS")
    }

    fn inject(&self, _frame: &Frame) -> Result<()> {
        bail!("input injection is not yet implemented on this OS")
    }

    fn set_cursor_visible(&self, _visible: bool) -> Result<()> {
        bail!("cursor visibility control is not yet implemented on this OS")
    }
}
