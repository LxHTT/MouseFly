# MouseFly — Plan

Living design document. Update this when decisions change; do not let it rot.

## 1. Vision

One keyboard and mouse, many computers. Move the cursor off the edge of one host's screen and it lands on the neighboring host's screen, with key events following along. Like macOS Universal Control, but cross-platform (Windows + macOS first, Linux later) and with a layout GUI good enough that arranging four hosts on the desk feels obvious.

## 2. Goals (v1)

- Windows ↔ macOS, mouse + keyboard.
- LAN-only, peer-to-peer, no cloud.
- Pairing by 6-digit code, persistent identity after first pair.
- Drag-to-arrange layout GUI with multi-monitor support per host.
- Per-monitor DPI / physical-size aware cursor scaling.
- Single-binary install per host.

## 3. Non-goals (v1)

- Cloud relay or WAN traversal.
- File transfer or drag-drop between hosts.
- Clipboard sync (planned for v2; text-only).
- Touch / gesture forwarding.
- Mobile (iOS/Android).
- Linux (planned for v2; X11 first, Wayland once `libei` + portals are ubiquitous).

## 4. Architecture

Two components per host, **same OS process**:

```text
┌─────────────────── MouseFly host ────────────────────┐
│                                                       │
│  Tauri webview (Vue) ──IPC──┐                        │
│  layout, pairing,           │                        │
│  tray, settings             ▼                        │
│                       ┌────────────────────┐          │
│                       │   Rust core        │          │
│                       │                    │          │
│                       │  input backend ────┼──► OS    │
│                       │  (per-OS, trait)   │          │
│                       │                    │          │
│                       │  QUIC peer ────────┼──► LAN   │
│                       │  mDNS + SPAKE2     │          │
│                       └────────────────────┘          │
└──────────────────────────────────────────────────────┘
```

A host is **active** (owns the physical input) or **passive** (receiving events) at any given moment. Role flips when the cursor crosses a mapped edge.

Single process keeps v1 simple. Reconsider splitting daemon ↔ GUI only if Windows UAC forces it (elevated daemon for hooking elevated apps + unelevated GUI).

## 5. Tech stack

### Rust core

- `tokio` — async runtime.
- `quinn` — QUIC transport.
- `rustls` + `ed25519-dalek` — TLS + identity keys.
- `spake2` — PAKE for pairing-by-code.
- `mdns-sd` — service discovery on LAN.
- `serde` + `bincode` or `postcard` — wire format for protocol messages.
- `tracing` — structured logging.
- Per-OS input crates: evaluate `rdev` / `enigo` first, expect to write our own thin per-OS modules behind one trait.

### Frontend

- Vue 3 (Composition API, `<script setup lang="ts">`).
- Vite + TypeScript.
- Tailwind v4.
- shadcn-vue for components (copy-paste, no runtime lock-in).
- Pinia for state.
- VueUse for reactive utilities.
- **Custom SVG** for the monitor-arrangement canvas — no graph library. Reasoning: our geometry is grouped rectangles with shared-edge snapping and per-edge mapping zones, very specific; bending `vue-flow` or similar to fit costs more than rolling it. SVG over Canvas because <50 hit-testable rects, accessible, devtools-debuggable.

### Why Vue over Svelte/React

Svelte has the smallest bundle but the maintainer is unfamiliar with it. Vue is the chosen middle ground — strong ecosystem, idiomatic Composition API matches Tauri's IPC model well, shadcn-vue gives modern aesthetics without lock-in, Pinia handles backend event streams cleanly. React is a fine fallback if a future contributor needs it; the architecture doesn't depend on the framework choice.

## 6. Coordinate model

Each host enumerates its monitors and publishes:

```rust
struct Monitor {
    id: MonitorId,                  // EDID hash if available, else (connector, res, pos)
    logical_size_px: (u32, u32),
    scale_factor: f32,              // OS-reported DPI scale
    physical_size_mm: Option<(u32, u32)>,
    position_in_local_vd: (i32, i32),  // OS's own virtual-desktop coords
    rotation: Rotation,
}
```

### Layout

The GUI shows each host as a **group** of monitor rectangles in their local arrangement (faintly bordered host container). User drags whole hosts to arrange them, then snaps individual edges to neighbor monitors on other hosts.

```text
┌───── Studio (Mac) ─────┐  ┌── Lap (Win) ──┐
│  ┌──────┐ ┌────────┐   │  │               │
│  │ Mon0 │ │ Mon1   │───┼──│   Mon0        │
│  │ 4K   │ │ QHD    │   │  │   3K 14"      │
│  └──────┘ └────────┘   │  │               │
└────────────────────────┘  └───────────────┘
```

### Cursor crossing

- **Within a host**: the OS handles it. MouseFly's hook only observes; no remap, no network. Inner edges (Studio Mon0 ↔ Mon1) are invisible to us.
- **Across hosts**: only fires at outer edges of the host's monitor union that the user mapped to a neighbor.
- **Handoff message** carries `{target_host, target_monitor_id, position_mm_along_edge, modifier_state}`.
- **Receiving host** picks the monitor, converts mm → its pixels using *that monitor's* DPI/physical size, calls `SetCursorPos` / `CGWarpMouseCursorPosition`.

### Mapping math

Use **physical millimetres** as the canonical unit when EDID size is available, so cursor speed feels continuous in real-world space across hosts of different DPI. Fall back to logical-scale ratios when physical size is missing or implausible.

### Edge cases (designed in, not deferred)

- **Mismatched edge lengths** (portrait next to landscape): only the overlapping segment is a handoff zone; outside it the cursor stops at the edge.
- **Hot-plug** (display added/removed/rotated, dock connect/disconnect): host re-enumerates and pushes a fresh layout to peers + GUI. In-flight handoffs targeting a now-gone monitor fall back to the host's primary.
- **DPI change mid-session** (Windows `WM_DPICHANGED`): re-read DPI on each handoff; never cache.
- **Chained passthrough** (host B as middle node between A and C): just two independent edge mappings — no special "chain" type.
- **Stable monitor identity across reconnects**: prefer EDID hash; fall back to `(connector, resolution, position)` tuple. OS-assigned indexes shuffle.

## 7. Network & pairing

### Transport

**QUIC via `quinn`.** UDP-based, multiplexed streams (one for input events, one for control, one for future clipboard), built-in TLS 1.3, ~1 RTT setup, tolerates packet loss without head-of-line blocking. Strictly better than Synergy's TCP for our workload.

### Discovery

mDNS service `_mousefly._udp.local`, advertising hostname + identity-key fingerprint. No central directory.

### Pairing

1. Host A enters pairing mode → 6-digit code shown on screen.
2. Host B scans mDNS, user picks A from a list, types the code.
3. Both run **SPAKE2** keyed by the code. Eavesdropper sees only PAKE traffic; the code itself never crosses the wire.
4. On success, hosts exchange long-lived **ed25519** public keys and store each other in a paired-peers file.
5. Future connections use mutual cert-pinned TLS over QUIC; no code re-entry.

### Wire protocol

Versioned, length-prefixed `postcard`-encoded messages:

```rust
enum Frame {
    Hello { version: u16, host_id: PublicKey, monitors: Vec<Monitor> },
    LayoutUpdate { monitors: Vec<Monitor> },
    HandoffEnter { monitor: MonitorId, pos_mm: (f32, f32), modifiers: Modifiers },
    PointerDelta { dx_mm: f32, dy_mm: f32, buttons: Buttons },
    PointerAbs   { monitor: MonitorId, pos_mm: (f32, f32), buttons: Buttons },
    Scroll       { dx: f32, dy: f32 },
    Key          { code: KeyCode, down: bool, modifiers: Modifiers },
    HandoffLeave,
    Heartbeat,
}
```

Bump `version` on any breaking change; old peers must refuse cleanly with a "please upgrade" toast in the GUI.

## 8. Per-OS notes

### Windows

- Capture: `SetWindowsHookEx(WH_MOUSE_LL / WH_KEYBOARD_LL)`.
- Inject: `SendInput`.
- Displays: `EnumDisplayMonitors` + `GetDpiForMonitor` (per-monitor DPI v2 awareness manifest required).
- Caveat: must run at the same elevation as any target window you want to inject into; UAC silently drops events otherwise. Document this.

### macOS

- Capture: `CGEventTap` at session level. Requires **Accessibility** + **Input Monitoring** permissions; show explanatory dialog before triggering the prompt.
- Inject: `CGEventPost`.
- Displays: `NSScreen` + `CGDisplayScreenSize` for physical mm.
- Caveat: tap is killed if the process becomes unresponsive (>~1s in callback). Keep the hook handler trivial — push events to an async channel, do work elsewhere.

### Linux (deferred)

- X11: `XTest` + `XInput2`. Straightforward.
- Wayland: `libei` + the input-capture portal. Implement once GNOME/KDE both ship it stably.

## 9. Roadmap

| Phase | Scope | Status |
| --- | --- | --- |
| **0. Spike** | Tauri 2 + Vue scaffold, Rust workspace, throwaway Mac→Mac mouse forward over plain TCP. Goal: prove the input loop end-to-end. | ✅ done |
| **1. Input core** | Win + Mac capture/injection behind `trait InputBackend`. Permissions UX. Mouse + keyboard from day one. | ✅ done |
| **2. Transport + pairing** | QUIC, mDNS discovery, SPAKE2 pairing flow, persistent ed25519 identity, paired-peers store. | ✅ done |
| **3. Layout GUI** | Vue shell, drag-arrange monitor canvas, tray icon, autostart. Per-edge handoff math is the deferred 3.5 piece. | ✅ done (canvas + chrome) |
| **4. Polish & v1 release** | Reconnection, lock-to-host hotkey, basic logs/diagnostics, installer per OS. | ✅ done (reconnect + lock); installer config still skipped (`bundle.active=false`) |
| **5. v2** | Clipboard text sync (✅ done), Linux X11 (stub), then Wayland (deferred). | partial |
| **3.5 / future** | Per-edge handoff math: sender's cursor crossing a mapped edge → mm-coords on receiver; ALPN-multiplexed pair+data on a single endpoint; full cross-OS HID keycode translation; EDID-hash monitor IDs. | not started |

## 10. Decisions log

| Date | Decision | Why |
| --- | --- | --- |
| 2026-04-26 | Tauri 2 + Rust core, single process | One codebase across Win/Mac/Linux; Rust ideal for input hooks and async networking; small binary. |
| 2026-04-26 | Vue 3 + Vite + TS + Tailwind + shadcn-vue + Pinia | Maintainer prefers Vue; ecosystem is strong; shadcn-vue gives modern look without runtime lock-in. |
| 2026-04-26 | Custom SVG canvas, no graph lib | Geometry is too specific (grouped rects, edge-segment mapping); graph libs constrain more than help. |
| 2026-04-26 | QUIC over TCP | Lower latency, multiplexed streams, built-in TLS, better loss tolerance. |
| 2026-04-26 | SPAKE2 + ed25519 for pairing | Standard PAKE primitives; short code never leaks; durable identity after first pair. |
| 2026-04-26 | Apache-2.0 license | Permissive, patent grant, friendly to commercial use; avoids GPL fork-back friction. |
| 2026-04-26 | LAN-only in v1, no cloud relay | Keeps scope tight; cloud is a different product surface and a different threat model. |
| 2026-04-26 | Mouse + keyboard from v1 | Same hook plumbing on each OS; splitting them would only add coordination work. |
| 2026-04-26 | Clipboard sync deferred to v2 | Easy to add later; pulls in privacy/format/encoding work that would slow v1. |
| 2026-04-26 | QUIC datagrams for pointer deltas, streams for state changes | Stale deltas worse than dropped; key/click events must arrive in order. |
| 2026-04-26 | Built-in latency probe in Phase 0 | Can't optimize what we can't measure; needed for bug reports too. |
| 2026-04-26 | Send physical scancodes, not characters | Matches Synergy/Universal Control; avoids layout-translation bugs across hosts. |
| 2026-04-26 | USB cables not a v1 feature | Thunderbolt / USB4 networking gives us the wired-low-latency case for free as a NIC. |
| 2026-04-26 | Phase 0 spike landed (commit 1e71fe4) | macOS-only mouse over plain TCP, Tauri+Vue scaffold, latency probe, kill switch. |
| 2026-04-26 | Phase 1 landed (commit f8a3acc) | Windows backend behind `trait InputBackend`, macOS keyboard support, monitor enumeration, accessibility preflight, GitHub Actions CI for mac + windows. |
| 2026-04-26 | Phase 2 landed (commit 1a70250) | TCP→QUIC swap (datagrams for pointer deltas, streams for state), mDNS discovery, SPAKE2 + ed25519 pairing flow, Vue pairing UI. Pair endpoint and data link still use separate certs (Phase 3.5 unifies via ALPN). |
| 2026-04-26 | Phase 3 landed (commit b2d056c) | Hand-rolled SVG monitor-arrangement canvas with snap-to-edge, system tray icon, autostart Tauri commands. Edge-handoff math (cursor mm-mapping → remote injection) deferred to Phase 3.5 — canvas exposes the geometry but cursor crossing still uses Phase 0's absolute pixel forwarding. |
| 2026-04-26 | Phase 4 landed (commit 6b98ccb) | Sender reconnects with exponential backoff (broadcast bridge keeps capture across reconnects); receiver loops on serve(); lock-to-host static + Tauri commands so the GUI can pin input locally without dropping the link. |
| 2026-04-27 | Phase 5 landed | Plain-text clipboard sync (arboard, 500 ms poll, watermark-based echo suppression). Linux X11 backend stays a stub — real `xtest` + `XInput2` impl is the next concrete piece of work. |

## 11. Open questions

- **Lock-to-host hotkey default**: Cmd/Ctrl+Alt+L? Surveys differ.
- **Monitor identity when EDID hash is unstable**: a few cheap displays return all-zero EDID. Fallback heuristic TBD; revisit once we have test hardware.
- **Tray UX on Windows**: full menu vs. minimal "click to open"? Defer until Phase 3.
- **Auto-update**: Tauri's updater is fine but needs a signing key + hosting. Pick host (GitHub Releases?) before v1 ship.
- **Per-app exclusions** (e.g. "don't forward keys when I'm in this VM"): users will ask for it; defer until requested.

## 12. Glossary

- **Host**: one computer running MouseFly.
- **Active host**: the one whose physical mouse/keyboard is currently driving things.
- **Passive host**: receiving forwarded events.
- **Handoff**: the moment the active role transfers from one host to another.
- **Edge mapping**: a user-configured association between an outer edge of one monitor and an outer edge of another (possibly partial, segment-based).

## 13. Latency

The product feels magical under ~10 ms end-to-end and broken above ~50 ms. This section is the budget, the strategy, and the things we explicitly refuse to do.

### Budget

| Sensation | Hook → pixels on remote |
| --- | --- |
| Indistinguishable from local | < 10 ms |
| Feels good | 10–20 ms |
| Noticeable lag | 20–50 ms |
| Broken | > 50 ms |

Targets: ~5 ms wired, ~10–15 ms on clean 5 GHz Wi-Fi. Receiver vsync (0–16 ms at 60 Hz) is unavoidable and not counted in our budget.

### Where the milliseconds go

| Stage | Wired typical | Wi-Fi typical |
| --- | --- | --- |
| OS hook callback | 0.05 ms | 0.05 ms |
| Hook → network thread (lock-free ring) | < 0.1 ms | < 0.1 ms |
| Encode (postcard) | < 0.1 ms | < 0.1 ms |
| Userspace → NIC | ~0.2 ms | ~0.2 ms |
| Wire transit | < 0.5 ms | 1–30 ms (bursty) |
| NIC → userspace | ~0.2 ms | ~0.2 ms |
| Decode + InjectInput | < 0.2 ms | < 0.2 ms |

The fat tail is Wi-Fi. **Receiver-side Wi-Fi power-save is the single largest p99 contributor** — it can add 100+ ms when the radio sleeps. Document this prominently and surface link type in the GUI.

### Per-message transport

| Message | Transport | Why |
| --- | --- | --- |
| `PointerDelta`, `Scroll` | QUIC unreliable datagram | A 10 ms-old delta is worse than a dropped one; never wait for retransmit. |
| `Key`, `PointerAbs` | QUIC reliable stream (input) | Must arrive; ordering matters; loss of a key-up causes stuck modifiers. |
| `HandoffEnter`, `HandoffLeave`, `LayoutUpdate`, `Hello` | QUIC reliable stream (control) | Correctness > latency; separate stream avoids HoL blocking from input stream. |
| `Heartbeat` | QUIC unreliable datagram | Loss is fine; arrival cadence is what matters. |

### Hot-path rules

- OS hook callback: timestamp, push to pre-allocated SPSC ring (`crossbeam::queue::ArrayQueue`), return. Nothing else.
- macOS: `CGEventTap` is **disabled by the OS** if a callback exceeds ~1 s, and throttled if frequently slow. Treat the callback as time-critical.
- Network thread: pinned core, high priority (`THREAD_PRIORITY_TIME_CRITICAL` on Win, `QOS_CLASS_USER_INTERACTIVE` on Mac).
- No allocations in the hot path. Pooled `Vec<u8>` for encode buffers; reuse on send completion.
- No batching. No coalescing. No smoothing.

### Connection management

- Open QUIC connections to every paired peer at startup, not on first handoff.
- Heartbeat every ~750 ms to keep NAT mappings alive and defeat receiver-side Wi-Fi power-save.
- Reconnect with exponential backoff capped at ~5 s.

### Built-in latency probe

Surface link health in the GUI. Required for any later optimization to be evaluable, and for users to file useful bug reports.

- Every datagram carries a monotonic send timestamp.
- Periodic 4-timestamp NTP-style RTT probe gives clock-offset estimate per peer.
- Compute one-way delay per event; aggregate p50 / p95 / p99 over a 30 s window.
- GUI shows: link type (Ethernet / Wi-Fi 5 / Wi-Fi 6 / Thunderbolt), RTT p50/p99, one-way delay p50/p99, packet-loss %.
- Logged at INFO once per minute for bug-report context.
- **Build this in Phase 0.**

### Interface selection

- Detect interface type per paired peer.
- If a peer is reachable on multiple interfaces (e.g., Ethernet *and* Wi-Fi), probe each at startup, pin to the lowest-RTT one.
- Re-probe on disconnect, or every 5 minutes — whichever first.
- Thunderbolt / USB4 networking appears as a normal Ethernet interface and is preferred automatically (see §14).

### Anti-patterns we explicitly refuse

- **Cursor extrapolation / prediction on receiver.** Overshoots on direction reversal; feels worse than honest jitter.
- **Smoothing / interpolation buffers.** Adds at least one frame of latency.
- **Custom WFP / kernel driver for "earlier" capture.** A few hundred µs of win in exchange for a code-signing and security-review nightmare.
- **Disabling encryption to save crypto cycles.** ChaCha20 is a few µs per packet on modern CPUs. Invisible.
- **Coalescing pointer deltas to save bandwidth.** Bandwidth is not the constraint; latency is.

If LAN p99 is bad enough that any of these look attractive, the real fix is upstream — Wi-Fi config, interface choice, power-save settings — not papering over the symptom.

## 14. USB-cable transport

Can we communicate over a USB cable instead of the network? Mostly no — except for one case that we get for free.

### What works for free: Thunderbolt / USB4 networking

When two machines are connected by a Thunderbolt 3/4 or USB4 cable and both have the OS's Thunderbolt-bridge driver enabled (default on macOS, available on Windows, `thunderbolt-net` on Linux), the link appears as a normal Ethernet interface. Bandwidth 10–40 Gbps, sub-millisecond latency, no Wi-Fi jitter.

Our QUIC code runs over it unchanged. The interface-selection logic in §13 picks it automatically because RTT will be the lowest of any path. **Zero USB-specific code required.**

Recommended in docs as the wired option for users who want minimum latency between two laptops.

### What doesn't work

- **Plain USB-A or USB-C cable host-to-host**: USB is host-device, not peer-to-peer. Connecting two normal PCs with a regular cable does nothing (and can damage USB controllers in pathological cases). Don't try.
- **USB transfer cables** (e.g., Plugable USB-3 transfer cable): contain a bridge chip, present as a NIC. Works on Windows, flaky on macOS, capped at USB-2/3 speeds. Strictly worse than Thunderbolt; not worth dedicated support.

### USB HID emulation (different product)

A separate idea: have the sender appear as a *USB keyboard/mouse device* to the receiver, via a hardware adapter (Pi Pico, custom USB-C dongle). Receiver needs no MouseFly install — the OS sees real HID input. Lowest possible latency, hardware-KVM territory.

This is a fundamentally different architecture: hardware-bound, one-directional, no software on the receiver, no shared layout GUI, no clipboard. **Not in scope for v1 or likely ever.** If a user wants HID-level injection they should buy a hardware KVM.

### Decision

- Treat USB cables as a non-feature in our codebase.
- We get the only useful case (Thunderbolt / USB4 networking) for free as a NIC.
- Don't ship USB-specific code paths.
- In docs: recommend Thunderbolt or USB4 cable + bridge networking when a wired link is wanted; don't recommend USB transfer cables.

## 15. Keyboard specifics

Keyboard forwarding is in v1 (decision logged 2026-04-26 — same hook plumbing as the mouse). A few sub-decisions worth pinning down now.

### Send physical scancodes, not characters

The wire format carries the OS's physical scancode (HID usage code where possible), not the resulting character. The receiver maps the scancode through its own active keyboard layout. Rationale:

- Matches every other tool in this category (Synergy, Input Leap, Universal Control, Mouse Without Borders).
- Avoids combinatorial layout-translation bugs (sender QWERTY → receiver Dvorak → which layer? dead keys? IME?).
- Per-host layouts "just work" — typing on a Dvorak Mac while controlling a QWERTY Windows box produces what each side's user expects.
- Cost: user must have the right layout configured on each host. Normal anyway.

### Modifier state is authoritative per event

Each key event carries a full modifier bitmask (Shift / Ctrl / Alt / Meta), not just the transition. Receiver reconciles to that exact state on every event. Eliminates stuck-modifier bugs from a dropped key-up, at the cost of a few extra bytes per event. Cheap insurance.

### Keys that stay local (cannot or should not be forwarded)

- **Windows**: `Ctrl+Alt+Del` (Secure Attention Sequence — intercepted by Winlogon before any hook fires), `Win+L` (kernel-level lock), some Fn-row hotkeys handled by the OEM keyboard driver. These never reach our hook on the source side. Document the limitation; do **not** try to spoof them on the receiver.
- **macOS**: Touch ID, Globe-key emoji picker, brightness / volume media keys when claimed by the OS. Some are capturable but shouldn't be forwarded by default (forwarding "decrease brightness" to a remote display is rarely what the user wants).
- Default policy: forward letter / number / punctuation / modifier / arrow / function keys. Allowlist for media and system keys, off by default, opt-in per peer.

### Lock-to-host hotkey

A user-configurable shortcut that pins input to the current host regardless of cursor position — escape hatch for full-screen games, RDP sessions, or "I'm typing a password and don't want a handoff accident". Default suggestion: `Ctrl+Alt+Cmd+L` (Mac) / `Ctrl+Alt+Win+L` (Win). Customizable in settings.
