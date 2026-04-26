# AGENTS.md — MouseFly

Guidance for AI coding agents (Claude, Codex, Cursor, etc.) working in this repo. Humans should read it too — it's the contributor primer alongside [README.md](README.md).

## What this project is

MouseFly lets one keyboard and mouse drive multiple computers across a LAN — like macOS Universal Control, but Windows ↔ macOS (Linux later). The cursor moves off one host's screen edge and re-appears on the neighbouring host's screen, with keyboard events following.

Pairing uses a short code over mDNS-discovered services; transport is QUIC. The user-facing model is a single **Session** — one host hosts, others join.

Read [PLAN.md](PLAN.md) before making non-trivial changes — it has architecture, roadmap, and design rationale.

## Stack

- **Rust** — six-crate Cargo workspace under `crates/`. Input capture/injection, QUIC transport, mDNS discovery, pairing, monitor enumeration.
- **Tauri 2** — desktop shell, packages the Rust core with the webview GUI into a single binary. Tray + autostart plugins.
- **Vue 3 + TypeScript + Vite** — GUI. Composition API, `<script setup lang="ts">` only.
- **Tailwind v4** — styling. Hand-rolled SVG for the monitor canvas (no graph/diagram library; see PLAN.md §6).
- **Pinia** — frontend state.
- **Bun + Turborepo** — JS package manager + task orchestration. `bun.lock` is the source of truth; turbo handles caching for `typecheck` / `build`.

## Repo layout

```text
crates/
  mousefly-core/         OS-agnostic types: Frame enum, Monitor, MonitorId,
                         keymap (HID Usage IDs ↔ macOS / Win VKs).
  mousefly-input/        InputBackend trait + per-OS impls: macos.rs (CGEventTap +
                         CGEventPost + CGDisplay), windows.rs (SetWindowsHookEx +
                         SendInput + EnumDisplayMonitors), stub.rs (Linux/other).
  mousefly-net/          QUIC transport via quinn. Datagrams for pointer deltas,
                         reliable streams for keys / control. NTP-style RTT probe,
                         self-signed cert + optional fingerprint pinning.
  mousefly-discovery/    mDNS Advertiser + Browser (mdns-sd). TXT keys: fp / id / dp.
  mousefly-pair/         SPAKE2 + ed25519 + paired-peers store + verification SAS.
  mousefly-app/          Tauri main binary; wires everything. Modules:
                         clipboard.rs, layout.rs (GVL math), pairing.rs.
src-ui/                  Vue frontend (Vite root, port 1420 in dev).
  src/
    App.vue              Tabbed shell: Session + Layout, ResizeObserver-driven
                         adaptive window height with cubic-ease animation.
    views/
      SessionView.vue    Setup-or-join UX (merged Pair + Link).
      LayoutView.vue     Wraps the SVG monitor canvas.
    components/
      MonitorCanvas.vue  Hand-rolled SVG: pan/zoom, drag-snap, edge highlights.
    stores/              Pinia stores (link, layout, pairing).
    ipc/                 Typed wrappers over Tauri invoke / event listen.
crates/mousefly-app/
  capabilities/          Tauri 2 capability JSON (required for release builds —
                         core:default + core:window:allow-set-size + autostart).
  tauri.conf.json
  icons/                 SVG source + Python rasterizer (_render.py) + outputs.
.github/workflows/       build.yml — mac + windows CI via oven-sh/setup-bun.
scripts/
  test.sh                Local fmt + clippy + tests + builds + smoke gate.
  start.sh               Loopback dev (receiver + sender, two windows).
PLAN.md                  Design doc — read before non-trivial changes.
AGENTS.md                This file.
README.md                Project README.
```

## Common commands

```bash
bun install                             # one-time JS deps (bun.lock pinned)
bun run test                            # bash scripts/test.sh — full local gate
bun run start                           # bash scripts/start.sh — loopback dev
bun run dev                             # tauri dev, no role (GUI sets one)
bun run build                           # tauri build → dmg + app + nsis (signed if certs present)

# Pin a role from CLI (CI / scripted demos):
bun run receiver                        # tauri dev --listen 0.0.0.0:7878
bun run receiver:inject                 # same, inject ON (real second machine only)
bun run sender                          # tauri dev --peer 127.0.0.1:7878
bun run dev -- -- --listen 0.0.0.0:7878 # custom args

# Per-package tasks (turbo caches typecheck / build outputs):
bunx turbo run typecheck --filter=mousefly-ui
bunx turbo run build --filter=mousefly-ui
bun run --filter=mousefly-ui dev        # plain bun filter, no turbo cache

# Rust:
cargo test --workspace
cargo fmt --all                         # ALWAYS run before commit; CI fails on fmt diff
cargo fmt --all -- --check              # what CI runs
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p mousefly-input --target x86_64-pc-windows-msvc -- -D warnings  # cross-check Win
```

`bun run test` runs everything CI runs (`fmt --check`, `clippy -D warnings`, `cargo test --workspace`, frontend typecheck + build, UDP smoke). Use it before pushing.

## Conventions

- **Commits**: Conventional Commits (`feat:`, `fix:`, `refactor:`, `chore:`, `docs:`, `test:`, optional scope).
- **Branches**: `feat/...`, `fix/...`, work off `master` (this project uses `master`, not `main`).
- **Rust**: rustfmt default — **run `cargo fmt --all` before every commit**, CI rejects fmt diffs. Clippy clean with `-D warnings`. MSRV = latest stable.
- **TS/Vue**: prettier default (no eslint config yet — vue-tsc is the gate). Always `<script setup lang="ts">`.
- **Per-OS code**: gate with `#[cfg(target_os = "...")]`. Trait in `mousefly-input/src/lib.rs`, impls in `macos.rs` / `windows.rs` / `stub.rs`.
- **Tauri capabilities**: any new Tauri / plugin invoke needs an entry in `crates/mousefly-app/capabilities/default.json`. Release builds deny everything by default.
- **License**: Apache-2.0.

## Hard rules — read before touching input or networking code

1. **Never test a global keyboard hook without an escape hatch.** A buggy `WH_KEYBOARD_LL` or `CGEventTap` can swallow your input and lock you out. Always:
   - Wire the kill-switch tap (`Ctrl+Cmd+Shift+Esc` / `Ctrl+Win+Shift+Esc`) *before* installing the capture tap. `install_kill_switch()` blocks until verified live.
   - Have a separate terminal / SSH ready to `pkill mousefly`.
   - On macOS, run from a terminal you can reach without the keyboard you're testing.
2. **Never request elevated / Accessibility permissions silently.** Use the Vue status banner to explain *why*, then trigger the OS prompt. macOS caches permission per-pid — relaunch is required after a grant.
3. **Never log raw key codes / key text by default.** Keystrokes can contain passwords. Use a gated `--debug-input` flag and surface the risk in the UI.
4. **Pairing crypto is not DIY.** Use the `spake2` and `ed25519-dalek` crates. The wire format (SPAKE2 message → confirm tag → signed identity claim, with HMAC over the session key) is in `mousefly-pair/src/handshake.rs` — don't reshape it without updating PLAN.md §7 first.
5. **mDNS advertisement is opt-in.** The app only broadcasts on `_mousefly._udp.local.` after the user clicks **Start session** in the GUI (or `start_advertising` is called explicitly). Don't move the Advertiser back to app launch — the user expects to be invisible until they opt in.
6. **Don't add cloud / relay features.** v1 is LAN-only by design — see PLAN.md "Non-goals".

## Working style for agents

- Read [PLAN.md](PLAN.md) first when starting any non-trivial task — it documents *why* the architecture is the way it is.
- Prefer editing existing files over creating new ones.
- No comments explaining *what* the code does — only *why* when non-obvious.
- Don't introduce abstractions ahead of a second concrete use case.
- When OS-specific behaviour surprises you, prefix the explanatory comment with `// macOS:` or `// Windows:` so future readers know it's intentional.
- Open questions and decisions go in PLAN.md's "Decisions log", not scattered TODOs.
- The maintainer prefers Vue 3 idioms; avoid unexplained Svelte/React jargon in comments or PR descriptions.
- **Always run `bun run test` (or at minimum `cargo fmt --all`) before pushing** — CI failures on fmt are common when this is skipped.

## Where to find things

- Architecture or scope question → [PLAN.md](PLAN.md).
- Wire format / Frame variants → `crates/mousefly-core/src/lib.rs` (and PLAN.md §13 for transport classes).
- Pairing protocol → `crates/mousefly-pair/src/handshake.rs` (and PLAN.md §7).
- Per-OS input quirks → top-of-file doc comment in `crates/mousefly-input/src/macos.rs` or `windows.rs`.
- Tauri capability scope → `crates/mousefly-app/capabilities/default.json`.
- Anything unanswered → ask the human before guessing.
