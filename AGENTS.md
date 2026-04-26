# AGENTS.md — MouseFly

Guidance for AI coding agents (Claude, Codex, Cursor, etc.) working in this repo. Humans should read it too; it doubles as the contributor primer until we write a proper README.

## What this project is

MouseFly lets one keyboard and mouse drive multiple computers across a LAN — like macOS Universal Control, but Windows ↔ macOS (Linux later). The cursor moves off one host's screen edge and re-appears on the neighboring host's screen. Pairing is by short numeric code over mDNS; transport is QUIC.

Read [PLAN.md](PLAN.md) before making non-trivial changes — it has the architecture, roadmap, and design rationale.

## Stack

- **Rust** — core: input capture/injection, QUIC transport, pairing, monitor enumeration. Cargo workspace under `crates/`.
- **Tauri 2** — desktop shell, packages the Rust core with the webview GUI into a single binary.
- **Vue 3 + TypeScript + Vite** — GUI. Composition API, `<script setup lang="ts">` only.
- **Tailwind v4 + shadcn-vue** — styling and components.
- **Pinia** — frontend state.
- **VueUse** — reactive utilities.
- **Custom SVG** — monitor-arrangement canvas (no graph/diagram library; see PLAN.md for why).

## Repo layout (target)

```text
crates/
  mousefly-core/      OS-agnostic types, layout math, protocol messages
  mousefly-input/     InputBackend trait + per-OS impls (cfg-gated)
  mousefly-net/       QUIC peer, mDNS discovery, SPAKE2 pairing
  mousefly-app/       Tauri main binary; wires everything
src-ui/               Vue frontend (Vite root)
  src/
    components/ui/    shadcn-vue components
    canvas/           SVG monitor layout
    stores/           Pinia stores
    ipc/              typed wrappers around Tauri invoke/events
    views/            top-level pages (Layout, Pairing, Settings)
PLAN.md
AGENTS.md
CLAUDE.md
```

## Common commands

```bash
pnpm install                            # one-time JS deps
pnpm test                               # bash scripts/test.sh — fmt + clippy + tests + builds + smoke
pnpm start                              # bash scripts/start.sh — full loopback dev (receiver + sender, two windows)

# Single role (manual):
pnpm receiver                           # tauri dev --listen 0.0.0.0:7878
pnpm receiver:inject                    # same, with injection ON (real second machine only)
pnpm sender                             # tauri dev --peer 127.0.0.1:7878

# Tauri custom args:
pnpm dev -- -- --listen 0.0.0.0:7878
pnpm dev -- -- --peer 192.168.x.y:7878

# Lower-level checks:
pnpm --filter mousefly-ui typecheck     # vue-tsc --noEmit
pnpm --filter mousefly-ui build         # vite build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p mousefly-input --target x86_64-pc-windows-msvc -- -D warnings   # cross-check Windows backend
cargo fmt --all
```

## Conventions

- **Commits**: Conventional Commits (`feat:`, `fix:`, `refactor:`, `chore:`, `docs:`, `test:`).
- **Branches**: `feat/...`, `fix/...`, work off `master` (this project uses `master`, not `main`).
- **Rust**: rustfmt default, clippy clean with `-D warnings`. MSRV = latest stable.
- **TS/Vue**: prettier default, eslint with `@vue/eslint-config-typescript`. Always `<script setup lang="ts">`.
- **Per-OS code**: gate with `#[cfg(target_os = "...")]`. Trait in `mousefly-input/src/lib.rs`, impls in `windows.rs` / `macos.rs` / `linux.rs`.
- **License**: Apache-2.0 (see PLAN.md decisions log).

## Hard rules — read before touching input or networking code

1. **Never test a global keyboard hook without an escape hatch.** A buggy `WH_KEYBOARD_LL` or `CGEventTap` can swallow your input and lock you out. Always:
   - Wire a kill-switch hotkey *before* installing the hook.
   - Have a separate terminal/SSH ready to `pkill mousefly`.
   - On macOS, run from a terminal you can reach without the keyboard you're testing.
2. **Never request elevated / Accessibility permissions silently.** Show a Vue dialog explaining *why*, then trigger the OS prompt.
3. **Never log raw key codes by default.** Keystrokes can contain passwords. Use a gated `--debug-input` flag and surface the risk in the UI.
4. **Pairing crypto is not DIY.** Use the `spake2` and `ed25519-dalek` crates. Don't change the wire protocol without updating PLAN.md first.
5. **Don't add cloud / relay features.** v1 is LAN-only by design — see PLAN.md "Non-goals".

## Working style for agents

- Read [PLAN.md](PLAN.md) first when starting any non-trivial task.
- Prefer editing existing files over creating new ones.
- No comments explaining *what* the code does — only *why* when non-obvious.
- Don't introduce abstractions ahead of a second concrete use case.
- When OS-specific behavior surprises you, prefix the explanatory comment with `// macOS:` or `// Windows:` so future readers know it's intentional.
- Open questions and decisions go in PLAN.md's "Decisions log", not scattered TODOs.
- The maintainer prefers Vue 3 and is not familiar with Svelte/React internals — write idiomatic Vue and avoid unexplained framework jargon in comments or PR descriptions.

## Where to find things

- Architecture or scope question → PLAN.md.
- "How does input capture work on X?" → the per-OS module's top-of-file doc comment.
- Anything unanswered → ask the human before guessing.
