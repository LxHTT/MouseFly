//! HID Usage ID ↔ OS-native keycode translation tables.
//!
//! Sender emits HID Usage IDs (USB-IF Page 0x07 keyboard usage codes) on the
//! wire. Each receiver translates back to its OS-native code at inject time.
//! This lets Mac→Win and Win→Mac type the right physical key regardless of
//! layout — the receiver's own active keyboard layout decides which character
//! actually appears.
//!
//! Coverage is intentionally Phase-spike-scoped: ANSI letters, digits, common
//! punctuation, modifiers, arrows, F1–F12, and the most-used control keys.
//! Symbols that vary across layouts (`/`, `\`, `;`, `'`, `[`, `]`, etc.) use
//! the ANSI codes; non-ANSI / IME / dead keys are out of scope until we wire
//! a proper xkb-style layout pass.

#![allow(non_upper_case_globals)]

/// HID Usage IDs (Keyboard Page 0x07). Names match USB HID Usage Tables 1.21.
pub mod hid {
    pub const A: u16 = 0x04;
    pub const B: u16 = 0x05;
    pub const C: u16 = 0x06;
    pub const D: u16 = 0x07;
    pub const E: u16 = 0x08;
    pub const F: u16 = 0x09;
    pub const G: u16 = 0x0A;
    pub const H: u16 = 0x0B;
    pub const I: u16 = 0x0C;
    pub const J: u16 = 0x0D;
    pub const K: u16 = 0x0E;
    pub const L: u16 = 0x0F;
    pub const M: u16 = 0x10;
    pub const N: u16 = 0x11;
    pub const O: u16 = 0x12;
    pub const P: u16 = 0x13;
    pub const Q: u16 = 0x14;
    pub const R: u16 = 0x15;
    pub const S: u16 = 0x16;
    pub const T: u16 = 0x17;
    pub const U: u16 = 0x18;
    pub const V: u16 = 0x19;
    pub const W: u16 = 0x1A;
    pub const X: u16 = 0x1B;
    pub const Y: u16 = 0x1C;
    pub const Z: u16 = 0x1D;

    pub const N1: u16 = 0x1E;
    pub const N2: u16 = 0x1F;
    pub const N3: u16 = 0x20;
    pub const N4: u16 = 0x21;
    pub const N5: u16 = 0x22;
    pub const N6: u16 = 0x23;
    pub const N7: u16 = 0x24;
    pub const N8: u16 = 0x25;
    pub const N9: u16 = 0x26;
    pub const N0: u16 = 0x27;

    pub const RETURN: u16 = 0x28;
    pub const ESCAPE: u16 = 0x29;
    pub const BACKSPACE: u16 = 0x2A;
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    pub const MINUS: u16 = 0x2D;
    pub const EQUAL: u16 = 0x2E;
    pub const LBRACKET: u16 = 0x2F;
    pub const RBRACKET: u16 = 0x30;
    pub const BACKSLASH: u16 = 0x31;
    pub const SEMICOLON: u16 = 0x33;
    pub const QUOTE: u16 = 0x34;
    pub const GRAVE: u16 = 0x35;
    pub const COMMA: u16 = 0x36;
    pub const PERIOD: u16 = 0x37;
    pub const SLASH: u16 = 0x38;
    pub const CAPSLOCK: u16 = 0x39;

    pub const F1: u16 = 0x3A;
    pub const F2: u16 = 0x3B;
    pub const F3: u16 = 0x3C;
    pub const F4: u16 = 0x3D;
    pub const F5: u16 = 0x3E;
    pub const F6: u16 = 0x3F;
    pub const F7: u16 = 0x40;
    pub const F8: u16 = 0x41;
    pub const F9: u16 = 0x42;
    pub const F10: u16 = 0x43;
    pub const F11: u16 = 0x44;
    pub const F12: u16 = 0x45;

    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;

    pub const HOME: u16 = 0x4A;
    pub const PAGE_UP: u16 = 0x4B;
    pub const DELETE_FORWARD: u16 = 0x4C;
    pub const END: u16 = 0x4D;
    pub const PAGE_DOWN: u16 = 0x4E;

    pub const LCTRL: u16 = 0xE0;
    pub const LSHIFT: u16 = 0xE1;
    pub const LALT: u16 = 0xE2;
    pub const LMETA: u16 = 0xE3;
    pub const RCTRL: u16 = 0xE4;
    pub const RSHIFT: u16 = 0xE5;
    pub const RALT: u16 = 0xE6;
    pub const RMETA: u16 = 0xE7;
}

/// macOS virtual keycodes (subset, from `HIToolbox/Events.h`). Not HID Usage
/// IDs — that's the whole reason this module exists.
mod mac {
    pub const A: u32 = 0x00;
    pub const S: u32 = 0x01;
    pub const D: u32 = 0x02;
    pub const F: u32 = 0x03;
    pub const H: u32 = 0x04;
    pub const G: u32 = 0x05;
    pub const Z: u32 = 0x06;
    pub const X: u32 = 0x07;
    pub const C: u32 = 0x08;
    pub const V: u32 = 0x09;
    pub const B: u32 = 0x0B;
    pub const Q: u32 = 0x0C;
    pub const W: u32 = 0x0D;
    pub const E: u32 = 0x0E;
    pub const R: u32 = 0x0F;
    pub const Y: u32 = 0x10;
    pub const T: u32 = 0x11;
    pub const N1: u32 = 0x12;
    pub const N2: u32 = 0x13;
    pub const N3: u32 = 0x14;
    pub const N4: u32 = 0x15;
    pub const N6: u32 = 0x16;
    pub const N5: u32 = 0x17;
    pub const EQUAL: u32 = 0x18;
    pub const N9: u32 = 0x19;
    pub const N7: u32 = 0x1A;
    pub const MINUS: u32 = 0x1B;
    pub const N8: u32 = 0x1C;
    pub const N0: u32 = 0x1D;
    pub const RBRACKET: u32 = 0x1E;
    pub const O: u32 = 0x1F;
    pub const U: u32 = 0x20;
    pub const LBRACKET: u32 = 0x21;
    pub const I: u32 = 0x22;
    pub const P: u32 = 0x23;
    pub const RETURN: u32 = 0x24;
    pub const L: u32 = 0x25;
    pub const J: u32 = 0x26;
    pub const QUOTE: u32 = 0x27;
    pub const K: u32 = 0x28;
    pub const SEMICOLON: u32 = 0x29;
    pub const BACKSLASH: u32 = 0x2A;
    pub const COMMA: u32 = 0x2B;
    pub const SLASH: u32 = 0x2C;
    pub const N: u32 = 0x2D;
    pub const M: u32 = 0x2E;
    pub const PERIOD: u32 = 0x2F;
    pub const TAB: u32 = 0x30;
    pub const SPACE: u32 = 0x31;
    pub const GRAVE: u32 = 0x32;
    pub const BACKSPACE: u32 = 0x33;
    pub const ESCAPE: u32 = 0x35;
    pub const META: u32 = 0x37;
    pub const SHIFT: u32 = 0x38;
    pub const CAPSLOCK: u32 = 0x39;
    pub const ALT: u32 = 0x3A;
    pub const CTRL: u32 = 0x3B;
    pub const F5: u32 = 0x60;
    pub const F6: u32 = 0x61;
    pub const F7: u32 = 0x62;
    pub const F3: u32 = 0x63;
    pub const F8: u32 = 0x64;
    pub const F9: u32 = 0x65;
    pub const F11: u32 = 0x67;
    pub const F10: u32 = 0x6D;
    pub const F12: u32 = 0x6F;
    pub const F4: u32 = 0x76;
    pub const END: u32 = 0x77;
    pub const F2: u32 = 0x78;
    pub const PAGE_DOWN: u32 = 0x79;
    pub const F1: u32 = 0x7A;
    pub const LEFT: u32 = 0x7B;
    pub const RIGHT: u32 = 0x7C;
    pub const DOWN: u32 = 0x7D;
    pub const UP: u32 = 0x7E;
    pub const HOME: u32 = 0x73;
    pub const PAGE_UP: u32 = 0x74;
    pub const DELETE_FORWARD: u32 = 0x75;
}

/// Windows VK_ codes (subset).
mod win {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const CONTROL: u32 = 0x11;
    pub const MENU: u32 = 0x12; // Alt
    pub const ESCAPE: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const PRIOR: u32 = 0x21; // PgUp
    pub const NEXT: u32 = 0x22; // PgDn
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const DELETE: u32 = 0x2E;
    pub const N0: u32 = 0x30;
    pub const N1: u32 = 0x31;
    pub const N2: u32 = 0x32;
    pub const N3: u32 = 0x33;
    pub const N4: u32 = 0x34;
    pub const N5: u32 = 0x35;
    pub const N6: u32 = 0x36;
    pub const N7: u32 = 0x37;
    pub const N8: u32 = 0x38;
    pub const N9: u32 = 0x39;
    pub const A: u32 = 0x41;
    pub const B: u32 = 0x42;
    pub const C: u32 = 0x43;
    pub const D: u32 = 0x44;
    pub const E: u32 = 0x45;
    pub const F: u32 = 0x46;
    pub const G: u32 = 0x47;
    pub const H: u32 = 0x48;
    pub const I: u32 = 0x49;
    pub const J: u32 = 0x4A;
    pub const K: u32 = 0x4B;
    pub const L: u32 = 0x4C;
    pub const M: u32 = 0x4D;
    pub const N: u32 = 0x4E;
    pub const O: u32 = 0x4F;
    pub const P: u32 = 0x50;
    pub const Q: u32 = 0x51;
    pub const R: u32 = 0x52;
    pub const S: u32 = 0x53;
    pub const T: u32 = 0x54;
    pub const U: u32 = 0x55;
    pub const V: u32 = 0x56;
    pub const W: u32 = 0x57;
    pub const X: u32 = 0x58;
    pub const Y: u32 = 0x59;
    pub const Z: u32 = 0x5A;
    pub const LWIN: u32 = 0x5B;
    pub const RWIN: u32 = 0x5C;
    pub const F1: u32 = 0x70;
    pub const F2: u32 = 0x71;
    pub const F3: u32 = 0x72;
    pub const F4: u32 = 0x73;
    pub const F5: u32 = 0x74;
    pub const F6: u32 = 0x75;
    pub const F7: u32 = 0x76;
    pub const F8: u32 = 0x77;
    pub const F9: u32 = 0x78;
    pub const F10: u32 = 0x79;
    pub const F11: u32 = 0x7A;
    pub const F12: u32 = 0x7B;
    pub const CAPITAL: u32 = 0x14;
    pub const LSHIFT: u32 = 0xA0;
    pub const RSHIFT: u32 = 0xA1;
    pub const LCONTROL: u32 = 0xA2;
    pub const RCONTROL: u32 = 0xA3;
    pub const LMENU: u32 = 0xA4;
    pub const RMENU: u32 = 0xA5;
    pub const OEM_PLUS: u32 = 0xBB; // =
    pub const OEM_COMMA: u32 = 0xBC;
    pub const OEM_MINUS: u32 = 0xBD;
    pub const OEM_PERIOD: u32 = 0xBE;
    pub const OEM_2: u32 = 0xBF; // /
    pub const OEM_3: u32 = 0xC0; // `
    pub const OEM_4: u32 = 0xDB; // [
    pub const OEM_5: u32 = 0xDC; // \
    pub const OEM_6: u32 = 0xDD; // ]
    pub const OEM_7: u32 = 0xDE; // '
    pub const OEM_1: u32 = 0xBA; // ;
}

/// macOS CGKeyCode → HID Usage ID. Returns None for keys we haven't mapped.
pub fn from_macos(code: u32) -> Option<u16> {
    let h = match code {
        mac::A => hid::A,
        mac::B => hid::B,
        mac::C => hid::C,
        mac::D => hid::D,
        mac::E => hid::E,
        mac::F => hid::F,
        mac::G => hid::G,
        mac::H => hid::H,
        mac::I => hid::I,
        mac::J => hid::J,
        mac::K => hid::K,
        mac::L => hid::L,
        mac::M => hid::M,
        mac::N => hid::N,
        mac::O => hid::O,
        mac::P => hid::P,
        mac::Q => hid::Q,
        mac::R => hid::R,
        mac::S => hid::S,
        mac::T => hid::T,
        mac::U => hid::U,
        mac::V => hid::V,
        mac::W => hid::W,
        mac::X => hid::X,
        mac::Y => hid::Y,
        mac::Z => hid::Z,
        mac::N1 => hid::N1,
        mac::N2 => hid::N2,
        mac::N3 => hid::N3,
        mac::N4 => hid::N4,
        mac::N5 => hid::N5,
        mac::N6 => hid::N6,
        mac::N7 => hid::N7,
        mac::N8 => hid::N8,
        mac::N9 => hid::N9,
        mac::N0 => hid::N0,
        mac::RETURN => hid::RETURN,
        mac::ESCAPE => hid::ESCAPE,
        mac::BACKSPACE => hid::BACKSPACE,
        mac::TAB => hid::TAB,
        mac::SPACE => hid::SPACE,
        mac::MINUS => hid::MINUS,
        mac::EQUAL => hid::EQUAL,
        mac::LBRACKET => hid::LBRACKET,
        mac::RBRACKET => hid::RBRACKET,
        mac::BACKSLASH => hid::BACKSLASH,
        mac::SEMICOLON => hid::SEMICOLON,
        mac::QUOTE => hid::QUOTE,
        mac::GRAVE => hid::GRAVE,
        mac::COMMA => hid::COMMA,
        mac::PERIOD => hid::PERIOD,
        mac::SLASH => hid::SLASH,
        mac::CAPSLOCK => hid::CAPSLOCK,
        mac::F1 => hid::F1,
        mac::F2 => hid::F2,
        mac::F3 => hid::F3,
        mac::F4 => hid::F4,
        mac::F5 => hid::F5,
        mac::F6 => hid::F6,
        mac::F7 => hid::F7,
        mac::F8 => hid::F8,
        mac::F9 => hid::F9,
        mac::F10 => hid::F10,
        mac::F11 => hid::F11,
        mac::F12 => hid::F12,
        mac::RIGHT => hid::RIGHT,
        mac::LEFT => hid::LEFT,
        mac::DOWN => hid::DOWN,
        mac::UP => hid::UP,
        mac::HOME => hid::HOME,
        mac::PAGE_UP => hid::PAGE_UP,
        mac::DELETE_FORWARD => hid::DELETE_FORWARD,
        mac::END => hid::END,
        mac::PAGE_DOWN => hid::PAGE_DOWN,
        mac::CTRL => hid::LCTRL,
        mac::SHIFT => hid::LSHIFT,
        mac::ALT => hid::LALT,
        mac::META => hid::LMETA,
        _ => return None,
    };
    Some(h)
}

/// HID Usage ID → macOS CGKeyCode.
pub fn to_macos(hid: u16) -> Option<u32> {
    let m = match hid {
        self::hid::A => mac::A,
        self::hid::B => mac::B,
        self::hid::C => mac::C,
        self::hid::D => mac::D,
        self::hid::E => mac::E,
        self::hid::F => mac::F,
        self::hid::G => mac::G,
        self::hid::H => mac::H,
        self::hid::I => mac::I,
        self::hid::J => mac::J,
        self::hid::K => mac::K,
        self::hid::L => mac::L,
        self::hid::M => mac::M,
        self::hid::N => mac::N,
        self::hid::O => mac::O,
        self::hid::P => mac::P,
        self::hid::Q => mac::Q,
        self::hid::R => mac::R,
        self::hid::S => mac::S,
        self::hid::T => mac::T,
        self::hid::U => mac::U,
        self::hid::V => mac::V,
        self::hid::W => mac::W,
        self::hid::X => mac::X,
        self::hid::Y => mac::Y,
        self::hid::Z => mac::Z,
        self::hid::N1 => mac::N1,
        self::hid::N2 => mac::N2,
        self::hid::N3 => mac::N3,
        self::hid::N4 => mac::N4,
        self::hid::N5 => mac::N5,
        self::hid::N6 => mac::N6,
        self::hid::N7 => mac::N7,
        self::hid::N8 => mac::N8,
        self::hid::N9 => mac::N9,
        self::hid::N0 => mac::N0,
        self::hid::RETURN => mac::RETURN,
        self::hid::ESCAPE => mac::ESCAPE,
        self::hid::BACKSPACE => mac::BACKSPACE,
        self::hid::TAB => mac::TAB,
        self::hid::SPACE => mac::SPACE,
        self::hid::MINUS => mac::MINUS,
        self::hid::EQUAL => mac::EQUAL,
        self::hid::LBRACKET => mac::LBRACKET,
        self::hid::RBRACKET => mac::RBRACKET,
        self::hid::BACKSLASH => mac::BACKSLASH,
        self::hid::SEMICOLON => mac::SEMICOLON,
        self::hid::QUOTE => mac::QUOTE,
        self::hid::GRAVE => mac::GRAVE,
        self::hid::COMMA => mac::COMMA,
        self::hid::PERIOD => mac::PERIOD,
        self::hid::SLASH => mac::SLASH,
        self::hid::CAPSLOCK => mac::CAPSLOCK,
        self::hid::F1 => mac::F1,
        self::hid::F2 => mac::F2,
        self::hid::F3 => mac::F3,
        self::hid::F4 => mac::F4,
        self::hid::F5 => mac::F5,
        self::hid::F6 => mac::F6,
        self::hid::F7 => mac::F7,
        self::hid::F8 => mac::F8,
        self::hid::F9 => mac::F9,
        self::hid::F10 => mac::F10,
        self::hid::F11 => mac::F11,
        self::hid::F12 => mac::F12,
        self::hid::RIGHT => mac::RIGHT,
        self::hid::LEFT => mac::LEFT,
        self::hid::DOWN => mac::DOWN,
        self::hid::UP => mac::UP,
        self::hid::HOME => mac::HOME,
        self::hid::PAGE_UP => mac::PAGE_UP,
        self::hid::DELETE_FORWARD => mac::DELETE_FORWARD,
        self::hid::END => mac::END,
        self::hid::PAGE_DOWN => mac::PAGE_DOWN,
        self::hid::LCTRL | self::hid::RCTRL => mac::CTRL,
        self::hid::LSHIFT | self::hid::RSHIFT => mac::SHIFT,
        self::hid::LALT | self::hid::RALT => mac::ALT,
        self::hid::LMETA | self::hid::RMETA => mac::META,
        _ => return None,
    };
    Some(m)
}

/// Windows VK_ → HID Usage ID.
pub fn from_windows(code: u32) -> Option<u16> {
    let h = match code {
        win::A => hid::A,
        win::B => hid::B,
        win::C => hid::C,
        win::D => hid::D,
        win::E => hid::E,
        win::F => hid::F,
        win::G => hid::G,
        win::H => hid::H,
        win::I => hid::I,
        win::J => hid::J,
        win::K => hid::K,
        win::L => hid::L,
        win::M => hid::M,
        win::N => hid::N,
        win::O => hid::O,
        win::P => hid::P,
        win::Q => hid::Q,
        win::R => hid::R,
        win::S => hid::S,
        win::T => hid::T,
        win::U => hid::U,
        win::V => hid::V,
        win::W => hid::W,
        win::X => hid::X,
        win::Y => hid::Y,
        win::Z => hid::Z,
        win::N1 => hid::N1,
        win::N2 => hid::N2,
        win::N3 => hid::N3,
        win::N4 => hid::N4,
        win::N5 => hid::N5,
        win::N6 => hid::N6,
        win::N7 => hid::N7,
        win::N8 => hid::N8,
        win::N9 => hid::N9,
        win::N0 => hid::N0,
        win::RETURN => hid::RETURN,
        win::ESCAPE => hid::ESCAPE,
        win::BACK => hid::BACKSPACE,
        win::TAB => hid::TAB,
        win::SPACE => hid::SPACE,
        win::OEM_MINUS => hid::MINUS,
        win::OEM_PLUS => hid::EQUAL,
        win::OEM_4 => hid::LBRACKET,
        win::OEM_6 => hid::RBRACKET,
        win::OEM_5 => hid::BACKSLASH,
        win::OEM_1 => hid::SEMICOLON,
        win::OEM_7 => hid::QUOTE,
        win::OEM_3 => hid::GRAVE,
        win::OEM_COMMA => hid::COMMA,
        win::OEM_PERIOD => hid::PERIOD,
        win::OEM_2 => hid::SLASH,
        win::CAPITAL => hid::CAPSLOCK,
        win::F1 => hid::F1,
        win::F2 => hid::F2,
        win::F3 => hid::F3,
        win::F4 => hid::F4,
        win::F5 => hid::F5,
        win::F6 => hid::F6,
        win::F7 => hid::F7,
        win::F8 => hid::F8,
        win::F9 => hid::F9,
        win::F10 => hid::F10,
        win::F11 => hid::F11,
        win::F12 => hid::F12,
        win::RIGHT => hid::RIGHT,
        win::LEFT => hid::LEFT,
        win::DOWN => hid::DOWN,
        win::UP => hid::UP,
        win::HOME => hid::HOME,
        win::PRIOR => hid::PAGE_UP,
        win::DELETE => hid::DELETE_FORWARD,
        win::END => hid::END,
        win::NEXT => hid::PAGE_DOWN,
        win::CONTROL | win::LCONTROL => hid::LCTRL,
        win::RCONTROL => hid::RCTRL,
        win::SHIFT | win::LSHIFT => hid::LSHIFT,
        win::RSHIFT => hid::RSHIFT,
        win::MENU | win::LMENU => hid::LALT,
        win::RMENU => hid::RALT,
        win::LWIN => hid::LMETA,
        win::RWIN => hid::RMETA,
        _ => return None,
    };
    Some(h)
}

/// HID Usage ID → Windows VK_.
pub fn to_windows(h: u16) -> Option<u32> {
    let v = match h {
        self::hid::A => win::A,
        self::hid::B => win::B,
        self::hid::C => win::C,
        self::hid::D => win::D,
        self::hid::E => win::E,
        self::hid::F => win::F,
        self::hid::G => win::G,
        self::hid::H => win::H,
        self::hid::I => win::I,
        self::hid::J => win::J,
        self::hid::K => win::K,
        self::hid::L => win::L,
        self::hid::M => win::M,
        self::hid::N => win::N,
        self::hid::O => win::O,
        self::hid::P => win::P,
        self::hid::Q => win::Q,
        self::hid::R => win::R,
        self::hid::S => win::S,
        self::hid::T => win::T,
        self::hid::U => win::U,
        self::hid::V => win::V,
        self::hid::W => win::W,
        self::hid::X => win::X,
        self::hid::Y => win::Y,
        self::hid::Z => win::Z,
        self::hid::N1 => win::N1,
        self::hid::N2 => win::N2,
        self::hid::N3 => win::N3,
        self::hid::N4 => win::N4,
        self::hid::N5 => win::N5,
        self::hid::N6 => win::N6,
        self::hid::N7 => win::N7,
        self::hid::N8 => win::N8,
        self::hid::N9 => win::N9,
        self::hid::N0 => win::N0,
        self::hid::RETURN => win::RETURN,
        self::hid::ESCAPE => win::ESCAPE,
        self::hid::BACKSPACE => win::BACK,
        self::hid::TAB => win::TAB,
        self::hid::SPACE => win::SPACE,
        self::hid::MINUS => win::OEM_MINUS,
        self::hid::EQUAL => win::OEM_PLUS,
        self::hid::LBRACKET => win::OEM_4,
        self::hid::RBRACKET => win::OEM_6,
        self::hid::BACKSLASH => win::OEM_5,
        self::hid::SEMICOLON => win::OEM_1,
        self::hid::QUOTE => win::OEM_7,
        self::hid::GRAVE => win::OEM_3,
        self::hid::COMMA => win::OEM_COMMA,
        self::hid::PERIOD => win::OEM_PERIOD,
        self::hid::SLASH => win::OEM_2,
        self::hid::CAPSLOCK => win::CAPITAL,
        self::hid::F1 => win::F1,
        self::hid::F2 => win::F2,
        self::hid::F3 => win::F3,
        self::hid::F4 => win::F4,
        self::hid::F5 => win::F5,
        self::hid::F6 => win::F6,
        self::hid::F7 => win::F7,
        self::hid::F8 => win::F8,
        self::hid::F9 => win::F9,
        self::hid::F10 => win::F10,
        self::hid::F11 => win::F11,
        self::hid::F12 => win::F12,
        self::hid::RIGHT => win::RIGHT,
        self::hid::LEFT => win::LEFT,
        self::hid::DOWN => win::DOWN,
        self::hid::UP => win::UP,
        self::hid::HOME => win::HOME,
        self::hid::PAGE_UP => win::PRIOR,
        self::hid::DELETE_FORWARD => win::DELETE,
        self::hid::END => win::END,
        self::hid::PAGE_DOWN => win::NEXT,
        self::hid::LCTRL => win::LCONTROL,
        self::hid::RCTRL => win::RCONTROL,
        self::hid::LSHIFT => win::LSHIFT,
        self::hid::RSHIFT => win::RSHIFT,
        self::hid::LALT => win::LMENU,
        self::hid::RALT => win::RMENU,
        self::hid::LMETA => win::LWIN,
        self::hid::RMETA => win::RWIN,
        _ => return None,
    };
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_roundtrip_letters() {
        for code in [mac::A, mac::Z, mac::SPACE, mac::ESCAPE, mac::F1, mac::LEFT] {
            let h = from_macos(code).expect("native→hid");
            let m = to_macos(h).expect("hid→native");
            assert_eq!(code, m);
        }
    }

    #[test]
    fn win_roundtrip_letters() {
        for code in [
            win::A,
            win::Z,
            win::SPACE,
            win::ESCAPE,
            win::F1,
            win::LEFT,
            win::OEM_2,
        ] {
            let h = from_windows(code).expect("native→hid");
            let v = to_windows(h).expect("hid→native");
            assert_eq!(code, v);
        }
    }

    #[test]
    fn cross_os_a_matches() {
        assert_eq!(from_macos(mac::A), from_windows(win::A));
    }
}
