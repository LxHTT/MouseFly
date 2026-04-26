//! macOS input backend.
//!
//! Capture: `CGEventTap` at HID level, listen-only in Phase 0 so the local
//! cursor isn't suppressed (we can both observe and let the OS draw it). The
//! tap callback runs on a background thread's `CFRunLoop`; it does the absolute
//! minimum — convert and push to a bounded channel, return — because the OS
//! disables the tap if callbacks exceed ~1 s and throttles us if they're often
//! slow.
//!
//! Injection: `CGEventPost` at HID level. Click frames re-use the last absolute
//! position we delivered, so a typical sequence is `PointerAbs` → `MouseButton
//! { LEFT }` → ... → `MouseButton { 0 }`.
//!
//! Permissions: this module requires both Accessibility and Input Monitoring.
//! `CGEventTap::new` returns `Err(())` if either is missing — surface that as a
//! clear error so the GUI can prompt.

use anyhow::{anyhow, Context, Result};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CGMouseButton, EventField, KeyCode,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use crossbeam_channel::{bounded, Receiver, Sender};
use mousefly_core::{button, Frame};
use std::sync::Mutex;
use std::thread;
use tracing::{debug, error, warn};

use crate::InputBackend;

pub struct MacBackend {
    /// Last absolute cursor position we injected. Click frames re-use this so
    /// we don't have to round-trip through `CGEventCreate` on every click.
    last_pos: Mutex<(f64, f64)>,
}

impl MacBackend {
    pub fn new() -> Self {
        Self {
            last_pos: Mutex::new((0.0, 0.0)),
        }
    }
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for MacBackend {
    fn start_capture(&self) -> Result<Receiver<Frame>> {
        let (tx, rx) = bounded::<Frame>(1024);
        thread::Builder::new()
            .name("mousefly-capture".into())
            .spawn(move || {
                if let Err(e) = run_capture_loop(tx) {
                    error!("capture loop exited: {e:#}");
                }
            })
            .context("spawning capture thread")?;
        Ok(rx)
    }

    fn inject(&self, frame: &Frame) -> Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow!("CGEventSource::new failed"))?;
        match *frame {
            Frame::PointerAbs { x, y, buttons } => {
                let pos = CGPoint::new(x as f64, y as f64);
                *self.last_pos.lock().unwrap() = (pos.x, pos.y);
                // macOS: while a button is held, MouseMoved is not the right
                // event type — must be the corresponding *Dragged event so the
                // receiving app sees the drag.
                let etype = if buttons & button::LEFT != 0 {
                    CGEventType::LeftMouseDragged
                } else if buttons & button::RIGHT != 0 {
                    CGEventType::RightMouseDragged
                } else {
                    CGEventType::MouseMoved
                };
                let evt = CGEvent::new_mouse_event(source, etype, pos, CGMouseButton::Left)
                    .map_err(|()| anyhow!("CGEvent::new_mouse_event failed"))?;
                evt.post(CGEventTapLocation::HID);
            }
            Frame::MouseButton { buttons } => {
                let (x, y) = *self.last_pos.lock().unwrap();
                let pos = CGPoint::new(x, y);
                // Phase 0 simplification: only LEFT is wired through. Right /
                // middle land in Phase 1 with proper per-button state tracking.
                let down = buttons & button::LEFT != 0;
                let etype = if down {
                    CGEventType::LeftMouseDown
                } else {
                    CGEventType::LeftMouseUp
                };
                let evt = CGEvent::new_mouse_event(source, etype, pos, CGMouseButton::Left)
                    .map_err(|()| anyhow!("CGEvent::new_mouse_event failed"))?;
                evt.post(CGEventTapLocation::HID);
            }
            Frame::Scroll { dx, dy } => {
                debug!(dx, dy, "Phase 0: scroll injection not implemented");
            }
            Frame::Heartbeat | Frame::RttProbe { .. } | Frame::RttReply { .. } => {}
        }
        Ok(())
    }
}

fn run_capture_loop(tx: Sender<Frame>) -> Result<()> {
    let events_of_interest = vec![
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDragged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::ScrollWheel,
    ];

    let cb_tx = tx.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        events_of_interest,
        move |_proxy, etype, evt| {
            if let Some(frame) = event_to_frame(etype, evt) {
                if let Err(e) = cb_tx.try_send(frame) {
                    if e.is_full() {
                        warn!("capture channel full; dropping event");
                    }
                }
            }
            None
        },
    )
    .map_err(|()| {
        anyhow!(
            "CGEventTap::new failed — Accessibility and Input Monitoring \
             permissions both required (System Settings → Privacy & Security)"
        )
    })?;

    let current = CFRunLoop::get_current();
    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|()| anyhow!("create_runloop_source failed"))?;
    unsafe {
        current.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();
    CFRunLoop::run_current();
    drop(tx);
    Ok(())
}

fn event_to_frame(etype: CGEventType, evt: &CGEvent) -> Option<Frame> {
    match etype {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let p = evt.location();
            Some(Frame::PointerAbs {
                x: p.x as f32,
                y: p.y as f32,
                buttons: 0,
            })
        }
        CGEventType::LeftMouseDown => Some(Frame::MouseButton {
            buttons: button::LEFT,
        }),
        CGEventType::LeftMouseUp => Some(Frame::MouseButton { buttons: 0 }),
        CGEventType::RightMouseDown => Some(Frame::MouseButton {
            buttons: button::RIGHT,
        }),
        CGEventType::RightMouseUp => Some(Frame::MouseButton { buttons: 0 }),
        CGEventType::ScrollWheel => {
            let dy =
                evt.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1) as f32;
            let dx =
                evt.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2) as f32;
            Some(Frame::Scroll { dx, dy })
        }
        _ => None,
    }
}

/// Install the kill-switch tap on a dedicated thread + run loop. Listens for
/// `Ctrl+Cmd+Shift+Esc` and exits the process when it fires. Spawned with a
/// listen-only tap so it doesn't interfere with normal key events.
pub fn install_kill_switch() -> Result<()> {
    thread::Builder::new()
        .name("mousefly-killswitch".into())
        .spawn(|| {
            if let Err(e) = run_kill_switch_loop() {
                error!("kill switch install failed: {e:#}");
            }
        })
        .context("spawning kill-switch thread")?;
    Ok(())
}

fn run_kill_switch_loop() -> Result<()> {
    let required = CGEventFlags::CGEventFlagControl
        | CGEventFlags::CGEventFlagCommand
        | CGEventFlags::CGEventFlagShift;
    let required_bits = required.bits();

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::KeyDown],
        move |_proxy, etype, evt| {
            if matches!(etype, CGEventType::KeyDown) {
                let keycode =
                    evt.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                let flags = evt.get_flags().bits();
                if keycode == KeyCode::ESCAPE && (flags & required_bits) == required_bits {
                    eprintln!("[mousefly] kill switch fired — exiting");
                    std::process::exit(0);
                }
            }
            None
        },
    )
    .map_err(|()| anyhow!("kill-switch CGEventTap::new failed"))?;

    let current = CFRunLoop::get_current();
    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|()| anyhow!("create_runloop_source failed"))?;
    unsafe {
        current.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();
    CFRunLoop::run_current();
    Ok(())
}
