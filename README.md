# MouseFly

One keyboard and mouse, many computers — over LAN.

MouseFly forwards keyboard and pointer input between hosts on the same local network so a single physical keyboard and mouse can drive multiple computers, like macOS Universal Control but cross-platform. Hosts discover each other automatically over mDNS, pair with a short code via [SPAKE2](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-spake2-23), and stream input over [QUIC](https://datatracker.ietf.org/doc/html/rfc9000). LAN-only by design — there is no cloud relay.

## Status

| Platform | Input capture / injection | Notes |
| --- | --- | --- |
| macOS (Apple Silicon, macOS 11+) | ✅ | Tested. Requires Accessibility + Input Monitoring permissions. |
| Windows 10 / 11 (x86_64) | ✅ | Cross-compiled and CI-built. Requires WebView2 (preinstalled on Win11 / via Edge updates on Win10). |
| Linux (X11 / Wayland) | 🚧 | Stub backend only — compiles but does nothing. Real `XTest` / `libei` work is on the roadmap. |

The session shell, mDNS discovery, SPAKE2 pairing, QUIC transport, monitor layout canvas with edge-handoff math, HID keycode translation, tray icon, autostart, reconnect-with-backoff, lock-to-host, and plain-text clipboard sync all ship. Pre-1.0; expect rough edges.

See [PLAN.md](PLAN.md) for the full roadmap and design rationale.

## Features

- **Cross-platform input forwarding.** Mouse, keyboard, scroll, and click events flow between paired hosts at LAN-class latency (~5 ms wired, ~10–15 ms over Wi-Fi).
- **Multi-monitor aware.** Each host publishes its monitor set; the canvas-driven layout maps cursor positions in physical millimetres so the cursor crosses screen edges naturally even across hosts of different DPI.
- **Cross-OS keyboards.** Keystrokes travel as HID Usage IDs; each receiver maps them through its own active layout. Mac → Win and Win → Mac type the right physical key regardless of QWERTY / Dvorak / etc.
- **Code-based pairing.** Hosts advertise themselves on mDNS only after the user clicks *Start session*. Pairing uses a 6-digit numeric code (auto-generated, optionally rotating every 5 / 30 / 60 minutes) or a user-defined alphanumeric code (≥6 chars). The handshake is authenticated by SPAKE2 + a per-pair ed25519 identity; both sides display a 32-bit verification string after success.
- **QUIC transport.** Pointer deltas ride unreliable datagrams (drop is better than retransmit-late); button / key events go on a reliable stream; layout updates and RTT probes have their own stream so latency probes are never head-of-line-blocked.
- **Plain-text clipboard sync.** 500 ms poll cadence with watermark-based echo suppression so two hosts don't ping-pong the clipboard.
- **Tray icon, autostart toggle, lock-to-host hotkey.** Standard productivity-app niceties.
- **No cloud, no relay, no telemetry.** Everything runs on the LAN.

## Installation

### macOS

Pre-built (unsigned) DMG: download `mousefly-macos-aarch64.dmg` from the [latest workflow run](https://github.com/LxHTT/MouseFly/actions/workflows/build.yml).

The DMG is ad-hoc-signed — Gatekeeper will refuse on first launch. Right-click the app and choose *Open*, then confirm. After launch, macOS will prompt for **Accessibility** and **Input Monitoring** in System Settings → Privacy & Security; grant both and relaunch the app.

### Windows

Pre-built (unsigned) installer: download `mousefly-windows-x86_64.exe` from the [latest workflow run](https://github.com/LxHTT/MouseFly/actions/workflows/build.yml). SmartScreen will warn — click *More info* → *Run anyway*.

### Linux

Not yet supported. Cargo workspace compiles on Linux (input backend is a stub) but the GUI is not packaged.

## Usage

1. Install MouseFly on both hosts and launch.
2. On the host you want to **be controlled** (the one whose mouse should move), open the **Session** tab → click **Start session**. The window shows a 6-digit code.
3. On the host you want to **control from**, the other device should appear under *Or join a session on the LAN*. Click **Join**, enter the code, submit.
4. Both windows show a 4-character verification string after pairing — confirm they match.
5. Open the **Layout** tab on either host and drag the two monitor groups into the arrangement that matches your desk. Cursor crossings now work end-to-end.

The kill switch — `Ctrl + ⌘ + ⇧ + Esc` on macOS, `Ctrl + Win + ⇧ + Esc` on Windows — exits both processes immediately if anything goes wrong.

## Building from source

Prerequisites:

- [Rust](https://rustup.rs/) stable (1.75+ via the pinned `rust-toolchain.toml`)
- [Bun](https://bun.sh/) 1.3+
- macOS: Xcode Command Line Tools (`xcode-select --install`)
- Windows: Visual Studio Build Tools 2022 with the Windows SDK
- Linux: standard libwebkit2gtk / libgtk dev packages (`apt install libwebkit2gtk-4.1-dev libgtk-3-dev`); will produce a stub binary

```bash
git clone https://github.com/LxHTT/MouseFly.git
cd MouseFly
bun install                             # JS deps
bun run build                           # full release build → dmg / nsis / exe
```

Development:

```bash
bun run dev                             # Tauri dev with HMR for the Vue side
bun run start                           # Loopback dev: spawns receiver + sender side-by-side
bun run test                            # Local CI gate (fmt + clippy + tests + builds + smoke)
```

See [AGENTS.md](AGENTS.md) for the full command reference and conventions.

## Architecture

A six-crate Cargo workspace plus a single Vue/Vite frontend, all packaged as one Tauri 2 binary per host:

```
┌──────────────────────────  Tauri 2 process  ──────────────────────────┐
│                                                                        │
│   Vue 3 webview ──┐                                                    │
│   (Session +      │       ┌──────────  Rust core  ──────────┐          │
│    Layout tabs)   │       │                                  │         │
│                   │ IPC   │  mousefly-input ────► OS APIs    │         │
│                   ├──────►│  mousefly-net (QUIC) ──► LAN     │ ──► UDP │
│                   │       │  mousefly-discovery (mDNS)       │   :7878 │
│                   │       │  mousefly-pair (SPAKE2 +         │         │
│                   │       │    ed25519)                      │         │
│                   │       │  mousefly-core (wire types)      │         │
│                   │       └──────────────────────────────────┘         │
└────────────────────────────────────────────────────────────────────────┘
```

Read [PLAN.md](PLAN.md) for the full design — it documents why each major decision was made (QUIC over TCP, custom SVG canvas, SPAKE2 over a plain shared secret, etc.).

## Contributing

[AGENTS.md](AGENTS.md) is the contributor primer — read its **Hard rules** before touching the input-capture or networking code. The short version:

- Conventional Commits, branches `feat/...` / `fix/...` off `master`.
- `bun run test` before pushing — CI runs the same gate.
- `cargo fmt --all` is mandatory; CI fails on any diff.
- New abstractions wait for a second concrete caller.

## License

Apache License 2.0 — see [LICENSE](LICENSE).
