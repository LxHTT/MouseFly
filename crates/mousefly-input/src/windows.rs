//! Windows input backend.
//!
//! Capture: `SetWindowsHookEx(WH_MOUSE_LL / WH_KEYBOARD_LL)` on a dedicated
//! thread that pumps a Win32 message loop. The OS calls our hook callbacks on
//! that same thread; without `GetMessageW` running, the callbacks never fire.
//! The callbacks themselves do the absolute minimum — convert to `Frame`,
//! `try_send` on a bounded crossbeam channel, return `CallNextHookEx` so we
//! never consume the input. Windows: low-level hooks have a system-wide
//! `LowLevelHooksTimeout` (default ~300 ms in the registry); a slow callback
//! gets the hook silently uninstalled.
//!
//! Injection: `SendInput` for both mouse and keyboard. Absolute mouse moves
//! are normalized to the 0..65535 virtual-desktop coordinate range and tagged
//! with `MOUSEEVENTF_VIRTUALDESK` so multi-monitor placement works.
//!
//! Monitors: `EnumDisplayMonitors` + `GetMonitorInfoW` + `GetDpiForMonitor`.
//! We call `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` once per
//! process so DPI numbers match what apps actually see; without it Windows
//! would virtualise everything to 96 DPI and lie to us.
//!
//! Permissions: there's no per-app entitlement like macOS, but injecting into
//! windows owned by an elevated process requires our process to be elevated
//! too — UAC silently drops the input otherwise. Surface this in the GUI.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use mousefly_core::{button, keymap, modifier, Frame, Monitor, MonitorId};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::sync::mpsc::sync_channel;
use std::sync::{Mutex, OnceLock};
use std::thread;
use tracing::{error, warn};

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, SetWindowsHookExW, ShowCursor,
    TranslateMessage, HC_ACTION, KBDLLHOOKSTRUCT, MONITORINFOF_PRIMARY, MSG, MSLLHOOKSTRUCT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WHEEL_DELTA,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::InputBackend;

/// Single global capture sender. The hook callback is a `extern "system" fn`
/// pointer with no user-data slot, so the only way to reach the channel from
/// inside it is a process-global. We set it exactly once per `start_capture`
/// call (Phase 1: a backend can only be captured once).
static CAPTURE_TX: OnceLock<Sender<Frame>> = OnceLock::new();

/// Same trick as `CAPTURE_TX` but for the kill switch. Separate slot so the
/// kill switch can be installed independently of capture.
static KILLSWITCH_INSTALLED: OnceLock<()> = OnceLock::new();

/// Cached current button mask, updated by the mouse hook so we can emit the
/// full mask after each transition.
static CAPTURE_BUTTONS: Mutex<u32> = Mutex::new(0);

pub struct WinBackend {
    /// Last mouse-button mask we *injected*. Used to diff against an incoming
    /// `MouseButton` frame and emit the right per-button up/down events.
    last_buttons: Mutex<u32>,
    /// Last absolute pointer position we injected, in device pixels relative
    /// to the virtual desktop origin. Stored so future-phase relative deltas
    /// can be added without an extra round-trip.
    last_pos: Mutex<(i32, i32)>,
}

impl WinBackend {
    pub fn new() -> Self {
        ensure_dpi_aware();
        Self {
            last_buttons: Mutex::new(0),
            last_pos: Mutex::new((0, 0)),
        }
    }
}

impl Default for WinBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBackend for WinBackend {
    fn enumerate_monitors(&self) -> Result<Vec<Monitor>> {
        ensure_dpi_aware();
        let mut monitors: Vec<Monitor> = Vec::new();
        // SAFETY: `EnumDisplayMonitors` calls our `extern "system"` callback
        // synchronously on this thread; passing a `&mut Vec<Monitor>` through
        // `LPARAM` is sound for the duration of the call. Windows: passing a
        // null HDC means "enumerate all monitors on the virtual desktop".
        let ok = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut monitors as *mut Vec<Monitor> as isize),
            )
        };
        if !ok.as_bool() {
            return Err(anyhow!("EnumDisplayMonitors returned FALSE"));
        }
        Ok(monitors)
    }

    fn start_capture(&self) -> Result<Receiver<Frame>> {
        let (tx, rx) = bounded::<Frame>(1024);
        CAPTURE_TX
            .set(tx)
            .map_err(|_| anyhow!("capture already started"))?;

        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(0);
        thread::Builder::new()
            .name("mousefly-capture".into())
            .spawn(move || {
                let install_result = install_capture_hooks();
                let installed_ok = install_result.is_ok();
                // Forward install result to the caller so they can `?` failure.
                let _ = ready_tx.send(install_result);
                if installed_ok {
                    run_message_loop();
                }
            })
            .context("spawning capture thread")?;

        ready_rx
            .recv()
            .context("capture thread died before signaling ready")??;
        Ok(rx)
    }

    fn inject(&self, frame: &Frame) -> Result<()> {
        match *frame {
            Frame::PointerAbs { x, y, .. } => {
                let (vx, vy, vw, vh) = virtual_desktop_rect();
                if vw <= 0 || vh <= 0 {
                    return Err(anyhow!("invalid virtual desktop size"));
                }
                let px = x.round() as i32;
                let py = y.round() as i32;
                *self.last_pos.lock().unwrap() = (px, py);
                let nx = (((px - vx) as i64 * 65535) / vw as i64).clamp(0, 65535) as i32;
                let ny = (((py - vy) as i64 * 65535) / vh as i64).clamp(0, 65535) as i32;
                let mi = MOUSEINPUT {
                    dx: nx,
                    dy: ny,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                };
                send_mouse(&[mi])?;
            }
            Frame::MouseButton { buttons } => {
                let prev = {
                    let mut last = self.last_buttons.lock().unwrap();
                    let prev = *last;
                    *last = buttons;
                    prev
                };
                let mut inputs: Vec<MOUSEINPUT> = Vec::with_capacity(6);
                let diff_down = buttons & !prev;
                let diff_up = prev & !buttons;
                let mut push = |flags: MOUSE_EVENT_FLAGS| {
                    inputs.push(MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    });
                };
                if diff_down & button::LEFT != 0 {
                    push(MOUSEEVENTF_LEFTDOWN);
                }
                if diff_up & button::LEFT != 0 {
                    push(MOUSEEVENTF_LEFTUP);
                }
                if diff_down & button::RIGHT != 0 {
                    push(MOUSEEVENTF_RIGHTDOWN);
                }
                if diff_up & button::RIGHT != 0 {
                    push(MOUSEEVENTF_RIGHTUP);
                }
                if diff_down & button::MIDDLE != 0 {
                    push(MOUSEEVENTF_MIDDLEDOWN);
                }
                if diff_up & button::MIDDLE != 0 {
                    push(MOUSEEVENTF_MIDDLEUP);
                }
                if !inputs.is_empty() {
                    send_mouse(&inputs)?;
                }
            }
            Frame::Scroll { dx, dy } => {
                let mut inputs: Vec<MOUSEINPUT> = Vec::with_capacity(2);
                if dy != 0.0 {
                    inputs.push(MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        // Windows: mouseData carries the signed wheel delta in
                        // its bit pattern even though the field type is u32.
                        mouseData: ((dy * WHEEL_DELTA as f32) as i32) as u32,
                        dwFlags: MOUSEEVENTF_WHEEL,
                        time: 0,
                        dwExtraInfo: 0,
                    });
                }
                if dx != 0.0 {
                    inputs.push(MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: ((dx * WHEEL_DELTA as f32) as i32) as u32,
                        dwFlags: MOUSEEVENTF_HWHEEL,
                        time: 0,
                        dwExtraInfo: 0,
                    });
                }
                if !inputs.is_empty() {
                    send_mouse(&inputs)?;
                }
            }
            Frame::Key { code, down, .. } => {
                // Wire-format `code` is HID Usage ID. Translate to a Windows
                // VK_ before injecting; drop unmapped keys.
                let vk = match keymap::to_windows(code as u16) {
                    Some(v) => v as u16,
                    None => {
                        warn!(hid = code, "unmapped HID key — dropping");
                        return Ok(());
                    }
                };
                let flags = if down {
                    KEYBD_EVENT_FLAGS(0)
                } else {
                    KEYEVENTF_KEYUP
                };
                let ki = KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                };
                send_key(ki)?;
            }
            Frame::PointerOnMonitor { .. } => {
                return Err(anyhow!(
                    "Frame::PointerOnMonitor must be resolved to PointerAbs by the app layer \
                     before injection"
                ));
            }
            Frame::Heartbeat
            | Frame::RttProbe { .. }
            | Frame::RttReply { .. }
            | Frame::LayoutUpdate { .. }
            | Frame::Clipboard { .. }
            | Frame::SessionExit
            | Frame::LayoutEditLock { .. }
            | Frame::RemoteControlState { .. } => {}
        }
        Ok(())
    }

    fn set_cursor_visible(&self, visible: bool) -> Result<()> {
        // SAFETY: ShowCursor is safe to call from any thread. It increments or
        // decrements a display counter; when the counter is < 0, the cursor is
        // hidden. Multiple calls may be needed to fully show/hide if other code
        // has also called ShowCursor.
        unsafe {
            if visible {
                // Increment the display counter until cursor is visible.
                while ShowCursor(true) < 0 {}
            } else {
                // Decrement the display counter until cursor is hidden.
                while ShowCursor(false) >= 0 {}
            }
        }
        Ok(())
    }
}

fn ensure_dpi_aware() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // SAFETY: setting a process-wide DPI awareness flag has no pointer
        // arguments and is safe to call from any thread. The result is best
        // effort — if a manifest already set awareness this just fails benign.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    });
}

fn virtual_desktop_rect() -> (i32, i32, i32, i32) {
    // SAFETY: GetSystemMetrics is a pure read; no preconditions.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn send_mouse(events: &[MOUSEINPUT]) -> Result<()> {
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|mi| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: *mi },
        })
        .collect();
    // SAFETY: `inputs` lives for the duration of the call; size matches.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(anyhow!(
            "SendInput accepted {sent}/{} mouse events",
            inputs.len()
        ));
    }
    Ok(())
}

fn send_key(ki: KEYBDINPUT) -> Result<()> {
    let inputs = [INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    }];
    // SAFETY: stack array lives across the call; size matches.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(anyhow!("SendInput accepted {sent}/1 key event"));
    }
    Ok(())
}

fn install_capture_hooks() -> Result<()> {
    // SAFETY: `LowLevelMouseProc` matches the expected signature; passing the
    // current module is allowed for low-level hooks.
    let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), None, 0) }
        .map_err(|e| anyhow!("SetWindowsHookExW(WH_MOUSE_LL) failed: {e}"))?;
    if mouse_hook.is_invalid() {
        return Err(anyhow!("WH_MOUSE_LL hook handle is invalid"));
    }
    // SAFETY: same as above for keyboard.
    let kb_hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), None, 0) }
            .map_err(|e| anyhow!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))?;
    if kb_hook.is_invalid() {
        return Err(anyhow!("WH_KEYBOARD_LL hook handle is invalid"));
    }
    Ok(())
}

fn run_message_loop() {
    let mut msg = MSG::default();
    // SAFETY: `GetMessageW` writes through `&mut msg`. Loop terminates when
    // it returns 0 (WM_QUIT) or -1 (error); both end the thread cleanly.
    // Windows: `BOOL(-1)` is the error sentinel, so we can't blindly check
    // truthiness — compare the inner i32 explicitly.
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: per MSDN, when `code == HC_ACTION` the lparam is a valid
        // pointer to MSLLHOOKSTRUCT for the lifetime of this call.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if let Some(frame) = mouse_event_to_frame(wparam.0 as u32, info) {
            if let Some(tx) = CAPTURE_TX.get() {
                if let Err(e) = tx.try_send(frame) {
                    if e.is_full() {
                        warn!("capture channel full; dropping mouse event");
                    }
                }
            }
        }
    }
    // SAFETY: forward to the next hook with the original parameters.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn mouse_event_to_frame(msg: u32, info: &MSLLHOOKSTRUCT) -> Option<Frame> {
    let pt: POINT = info.pt;
    match msg {
        WM_MOUSEMOVE => {
            let buttons = *CAPTURE_BUTTONS.lock().unwrap();
            // Windows: MSLLHOOKSTRUCT doesn't expose raw deltas directly. The OS
            // provides absolute coordinates only. We track the last position and
            // compute deltas ourselves. This is less accurate than macOS's
            // EventField::MOUSE_EVENT_DELTA_X/Y (which gives true hardware deltas
            // even when the cursor is clamped at screen edges), but it's the best
            // Windows offers via low-level hooks. For edge-crossing to work, we
            // need *some* delta — zero deltas prevent the virtual cursor from
            // accumulating movement when pinned at the screen boundary.
            static LAST_PT: Mutex<Option<POINT>> = Mutex::new(None);
            let mut last = LAST_PT.lock().unwrap();
            let (dx, dy) = match *last {
                Some(prev) => ((pt.x - prev.x) as f32, (pt.y - prev.y) as f32),
                None => (0.0, 0.0),
            };
            *last = Some(pt);
            Some(Frame::PointerAbs {
                x: pt.x as f32,
                y: pt.y as f32,
                dx,
                dy,
                buttons,
            })
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let bit = match msg {
                WM_LBUTTONDOWN => button::LEFT,
                WM_RBUTTONDOWN => button::RIGHT,
                _ => button::MIDDLE,
            };
            let mut held = CAPTURE_BUTTONS.lock().unwrap();
            *held |= bit;
            Some(Frame::MouseButton { buttons: *held })
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let bit = match msg {
                WM_LBUTTONUP => button::LEFT,
                WM_RBUTTONUP => button::RIGHT,
                _ => button::MIDDLE,
            };
            let mut held = CAPTURE_BUTTONS.lock().unwrap();
            *held &= !bit;
            Some(Frame::MouseButton { buttons: *held })
        }
        WM_MOUSEWHEEL => {
            // Windows: high word of mouseData is signed wheel delta; positive
            // = wheel rotated forward (away from user).
            let raw = ((info.mouseData >> 16) as u16) as i16;
            let dy = raw as f32 / WHEEL_DELTA as f32;
            Some(Frame::Scroll { dx: 0.0, dy })
        }
        WM_MOUSEHWHEEL => {
            let raw = ((info.mouseData >> 16) as u16) as i16;
            let dx = raw as f32 / WHEEL_DELTA as f32;
            Some(Frame::Scroll { dx, dy: 0.0 })
        }
        _ => None,
    }
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: per MSDN, lparam points at a valid KBDLLHOOKSTRUCT here.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
        if down || up {
            // Translate VK_ → HID before pushing on the wire so cross-OS
            // receivers don't have to know the Windows keycode space.
            if let Some(hid) = keymap::from_windows(info.vkCode) {
                let frame = Frame::Key {
                    code: hid as u32,
                    down,
                    modifiers: read_modifier_mask(),
                };
                if let Some(tx) = CAPTURE_TX.get() {
                    if let Err(e) = tx.try_send(frame) {
                        if e.is_full() {
                            warn!("capture channel full; dropping key event");
                        }
                    }
                }
            }
        }
    }
    // SAFETY: forward to next hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn read_modifier_mask() -> u32 {
    // SAFETY: GetKeyState reads thread-local key state; no preconditions.
    let down = |vk: VIRTUAL_KEY| unsafe { (GetKeyState(vk.0 as i32) as u16 & 0x8000) != 0 };
    let mut m = 0u32;
    if down(VK_SHIFT) {
        m |= modifier::SHIFT;
    }
    if down(VK_CONTROL) {
        m |= modifier::CTRL;
    }
    if down(VK_MENU) {
        m |= modifier::ALT;
    }
    if down(VK_LWIN) || down(VK_RWIN) {
        m |= modifier::META;
    }
    m
}

unsafe extern "system" fn monitor_enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: caller passed a `&mut Vec<Monitor>` via `LPARAM`; only this
    // synchronous enumeration is using it.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `&mut info.monitorInfo as *mut MONITORINFO` is valid for the
    // call; the OS writes back into the buffer we own.
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info.monitorInfo as *mut MONITORINFO) };
    if !ok.as_bool() {
        return BOOL(1);
    }

    let r = info.monitorInfo.rcMonitor;
    let width = (r.right - r.left).max(0) as u32;
    let height = (r.bottom - r.top).max(0) as u32;

    // szDevice is a UTF-16 null-terminated array; trim at the first NUL.
    let name_len = info
        .szDevice
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(info.szDevice.len());
    let name = String::from_utf16_lossy(&info.szDevice[..name_len]);

    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    // SAFETY: GetDpiForMonitor writes through both out pointers.
    let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    r.left.hash(&mut hasher);
    r.top.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    let id = MonitorId(hasher.finish());

    out.push(Monitor {
        id,
        name,
        logical_size_px: (width, height),
        scale_factor: dpi_x as f32 / 96.0,
        // Windows: physical mm requires EDID parsing (registry walk under
        // HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY). Deferred — Phase 1
        // uses the scale factor as a proxy.
        physical_size_mm: None,
        position_in_local_vd: (r.left, r.top),
        primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
    });
    BOOL(1)
}

/// Install the kill-switch hook on its own thread + message loop. Listens for
/// `Ctrl+Win+Shift+Esc` and exits the process when all four are held. Blocks
/// until the hook is verified live (or the install error is returned).
pub fn install_kill_switch() -> Result<()> {
    if KILLSWITCH_INSTALLED.set(()).is_err() {
        return Ok(());
    }
    let (ready_tx, ready_rx) = sync_channel::<Result<()>>(0);
    thread::Builder::new()
        .name("mousefly-killswitch".into())
        .spawn(move || {
            // SAFETY: same FFI invariants as the capture hook install.
            let install =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(kill_switch_proc), None, 0) };
            match install {
                Ok(h) if !h.is_invalid() => {
                    let _ = ready_tx.send(Ok(()));
                    run_message_loop();
                }
                Ok(_) => {
                    let _ = ready_tx.send(Err(anyhow!("kill-switch hook handle is invalid")));
                }
                Err(e) => {
                    let _ =
                        ready_tx.send(Err(anyhow!("SetWindowsHookExW(kill switch) failed: {e}")));
                }
            }
        })
        .context("spawning kill-switch thread")?;

    ready_rx
        .recv()
        .context("kill-switch thread died before signaling ready")?
        .map_err(|e| {
            error!("kill switch install failed: {e:#}");
            e
        })
}

unsafe extern "system" fn kill_switch_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wparam.0 as u32;
        if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN) {
            // SAFETY: HC_ACTION guarantees a live KBDLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if info.vkCode == VK_ESCAPE.0 as u32 {
                let mods = read_modifier_mask();
                let needed = modifier::CTRL | modifier::SHIFT | modifier::META;
                if mods & needed == needed {
                    eprintln!("[mousefly] kill switch fired — exiting");
                    std::process::exit(0);
                }
            }
        }
    }
    // SAFETY: forward to next hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
