//! macOS input backend.
//!
//! Capture: `CGEventTap` at HID level, listen-only in Phase 0/1 so the local
//! cursor isn't suppressed (we can both observe and let the OS draw it). The
//! tap callback runs on a background thread's `CFRunLoop`; it does the
//! absolute minimum — convert and push to a bounded channel, return — because
//! the OS disables the tap if callbacks exceed ~1 s and throttles us if
//! they're often slow.
//!
//! Injection: `CGEventPost` at HID level. Click frames re-use the last
//! absolute position we delivered; key frames go through
//! `CGEventCreateKeyboardEvent`.
//!
//! Monitors: enumerated via `CGDisplay::active_displays`; physical size via
//! `CGDisplayScreenSize`. EDID-hash IDs land in a later phase.
//!
//! Permissions: capture requires both Accessibility and Input Monitoring.
//! `accessibility_trusted` calls `AXIsProcessTrustedWithOptions(NULL)` so the
//! GUI can preflight before installing taps.

use anyhow::{anyhow, Context, Result};
use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_foundation::string::CFStringRef;
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CGMouseButton, EventField, KeyCode,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use crossbeam_channel::{bounded, Receiver, Sender};
use mousefly_core::{button, modifier, Frame, Modifiers, Monitor, MonitorId};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::thread;
use tracing::{debug, error, info, warn};

use crate::InputBackend;

pub struct MacBackend {
    /// Last absolute cursor position we injected. Click frames re-use this.
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
    fn enumerate_monitors(&self) -> Result<Vec<Monitor>> {
        let ids = CGDisplay::active_displays()
            .map_err(|e| anyhow!("CGDisplay::active_displays failed: {e:?}"))?;
        let primary = unsafe { core_graphics::display::CGMainDisplayID() };
        Ok(ids
            .into_iter()
            .map(|id| {
                let d = CGDisplay::new(id);
                let bounds = d.bounds();
                let size_mm = d.screen_size();
                let physical = if size_mm.width > 0.0 && size_mm.height > 0.0 {
                    Some((size_mm.width as u32, size_mm.height as u32))
                } else {
                    None
                };
                let logical = (bounds.size.width as u32, bounds.size.height as u32);
                let position = (bounds.origin.x as i32, bounds.origin.y as i32);
                // macOS: vendor + serial + model gives us a stable-enough id
                // across reconnects; EDID hash is Phase 3 work.
                let mut h = DefaultHasher::new();
                d.vendor_number().hash(&mut h);
                d.model_number().hash(&mut h);
                d.serial_number().hash(&mut h);
                position.hash(&mut h);
                Monitor {
                    id: MonitorId(h.finish()),
                    name: format!("Display {}", d.model_number()),
                    logical_size_px: logical,
                    scale_factor: pixel_scale(&d),
                    physical_size_mm: physical,
                    position_in_local_vd: position,
                    primary: id == primary,
                }
            })
            .collect())
    }

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
        match *frame {
            Frame::PointerAbs { x, y, buttons } => {
                let source = new_source()?;
                let pos = CGPoint::new(x as f64, y as f64);
                inject_mouse_at(&source, pos, buttons, &mut self.last_pos.lock().unwrap())?;
            }
            // Phase 3.5: receiver-side resolution from PointerOnMonitor → pixel
            // happens in mousefly-app (which has the local Monitor list it
            // produced via enumerate_monitors). The input backend is simply
            // told an absolute pixel; PointerAbs handles that. So we treat this
            // variant as a programming error if it reaches inject().
            Frame::PointerOnMonitor { .. } => {
                return Err(anyhow!(
                    "Frame::PointerOnMonitor must be resolved to PointerAbs by the app layer \
                     before injection"
                ));
            }
            Frame::MouseButton { buttons } => {
                let source = new_source()?;
                let (x, y) = *self.last_pos.lock().unwrap();
                let pos = CGPoint::new(x, y);
                // Phase 1 simplification: only LEFT is wired through. Right /
                // middle land alongside per-button state diffs.
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
                debug!(dx, dy, "Phase 1: scroll injection not implemented");
            }
            Frame::Key {
                code,
                down,
                modifiers,
            } => {
                let source = new_source()?;
                let evt = CGEvent::new_keyboard_event(source, code as u16, down)
                    .map_err(|()| anyhow!("CGEvent::new_keyboard_event failed (code={code})"))?;
                evt.set_flags(modifiers_to_cgflags(modifiers));
                evt.post(CGEventTapLocation::HID);
            }
            Frame::Heartbeat
            | Frame::RttProbe { .. }
            | Frame::RttReply { .. }
            | Frame::LayoutUpdate { .. }
            | Frame::Clipboard { .. } => {}
        }
        Ok(())
    }
}

fn new_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow!("CGEventSource::new failed"))
}

fn inject_mouse_at(
    source: &CGEventSource,
    pos: CGPoint,
    buttons: u32,
    last_pos: &mut (f64, f64),
) -> Result<()> {
    *last_pos = (pos.x, pos.y);
    // macOS: while a button is held, MouseMoved is the wrong event type —
    // must be the corresponding *Dragged so receiving apps see the drag.
    let etype = if buttons & button::LEFT != 0 {
        CGEventType::LeftMouseDragged
    } else if buttons & button::RIGHT != 0 {
        CGEventType::RightMouseDragged
    } else {
        CGEventType::MouseMoved
    };
    let evt = CGEvent::new_mouse_event(source.clone(), etype, pos, CGMouseButton::Left)
        .map_err(|()| anyhow!("CGEvent::new_mouse_event failed"))?;
    evt.post(CGEventTapLocation::HID);
    Ok(())
}

fn pixel_scale(d: &CGDisplay) -> f32 {
    let logical = d.bounds().size;
    let pixels_w = d.pixels_wide() as f64;
    if logical.width > 0.0 {
        (pixels_w / logical.width) as f32
    } else {
        1.0
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
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
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
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let code = evt.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32;
            let down = matches!(etype, CGEventType::KeyDown);
            Some(Frame::Key {
                code,
                down,
                modifiers: cgflags_to_modifiers(evt.get_flags()),
            })
        }
        CGEventType::FlagsChanged => {
            // macOS: pure-modifier transitions (Shift / Cmd) come as
            // FlagsChanged with the keycode of the modifier itself. Map the
            // current flag mask: if any of our tracked bits flipped on, send
            // KeyDown; if all are off, send KeyUp. Phase 1 keeps it simple by
            // forwarding the raw event with the current modifier state.
            let code = evt.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32;
            let modifiers = cgflags_to_modifiers(evt.get_flags());
            Some(Frame::Key {
                code,
                down: modifiers != 0,
                modifiers,
            })
        }
        _ => None,
    }
}

fn cgflags_to_modifiers(flags: CGEventFlags) -> Modifiers {
    let mut m = 0u32;
    if flags.contains(CGEventFlags::CGEventFlagShift) {
        m |= modifier::SHIFT;
    }
    if flags.contains(CGEventFlags::CGEventFlagControl) {
        m |= modifier::CTRL;
    }
    if flags.contains(CGEventFlags::CGEventFlagAlternate) {
        m |= modifier::ALT;
    }
    if flags.contains(CGEventFlags::CGEventFlagCommand) {
        m |= modifier::META;
    }
    m
}

fn modifiers_to_cgflags(m: Modifiers) -> CGEventFlags {
    let mut f = CGEventFlags::empty();
    if m & modifier::SHIFT != 0 {
        f |= CGEventFlags::CGEventFlagShift;
    }
    if m & modifier::CTRL != 0 {
        f |= CGEventFlags::CGEventFlagControl;
    }
    if m & modifier::ALT != 0 {
        f |= CGEventFlags::CGEventFlagAlternate;
    }
    if m & modifier::META != 0 {
        f |= CGEventFlags::CGEventFlagCommand;
    }
    f
}

/// Returns true if the process is trusted for Accessibility (which is also
/// required for `CGEventTap` to receive events). Doesn't trigger the OS
/// prompt — pass `prompt = true` to [`accessibility_request_trust`] for that.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// Triggers the macOS Accessibility permission prompt if the process isn't
/// already trusted. After the user grants, **the process must be relaunched**
/// — macOS caches the trusted bit per-pid and won't refresh on permission
/// grant alone.
#[allow(dead_code)] // used once the GUI's preflight dialog is wired
pub fn accessibility_request_trust() -> bool {
    use core_foundation::base::CFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    let key: CFString = unsafe { TCFType::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let val: CFBoolean = CFBoolean::true_value();
    let pairs: Vec<(CFType, CFType)> = vec![(key.as_CFType(), val.as_CFType())];
    let dict = CFDictionary::from_CFType_pairs(&pairs);
    unsafe {
        AXIsProcessTrustedWithOptions(
            dict.as_concrete_TypeRef() as *const _ as *const std::ffi::c_void
        )
    }
}

/// Install the kill-switch tap on a dedicated thread + run loop. Listens for
/// `Ctrl+Cmd+Shift+Esc` and exits the process when it fires. **Blocks** until
/// the tap is verified live (or returns the install error so callers can `?`).
pub fn install_kill_switch() -> Result<()> {
    let (ready_tx, ready_rx) = sync_channel::<Result<()>>(0);
    thread::Builder::new()
        .name("mousefly-killswitch".into())
        .spawn(move || {
            run_kill_switch_loop(ready_tx);
        })
        .context("spawning kill-switch thread")?;
    ready_rx.recv().context("kill-switch thread died")?
}

fn run_kill_switch_loop(ready_tx: std::sync::mpsc::SyncSender<Result<()>>) {
    let required = CGEventFlags::CGEventFlagControl
        | CGEventFlags::CGEventFlagCommand
        | CGEventFlags::CGEventFlagShift;
    let required_bits = required.bits();

    let tap = match CGEventTap::new(
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
    ) {
        Ok(t) => t,
        Err(()) => {
            let _ = ready_tx.send(Err(anyhow!(
                "kill-switch CGEventTap::new failed — \
                 Accessibility / Input Monitoring permission missing"
            )));
            return;
        }
    };

    let current = CFRunLoop::get_current();
    let loop_source = match tap.mach_port.create_runloop_source(0) {
        Ok(s) => s,
        Err(()) => {
            let _ = ready_tx.send(Err(anyhow!("create_runloop_source failed")));
            return;
        }
    };
    unsafe {
        current.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();
    info!("kill switch tap enabled");
    let _ = ready_tx.send(Ok(()));
    CFRunLoop::run_current();
}

// macOS: AXIsProcessTrustedWithOptions lives in ApplicationServices, which
// CoreGraphics is part of, so we get the symbol for free via the existing
// CoreGraphics link. core-foundation links its own crate-level CF.
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}
