# MouseFly

One keyboard and mouse, many computers — across Windows and macOS over LAN. Move the cursor off the edge of one host's screen and it appears on the neighbor's, with key events following along.

Status: **Phases 0–5 + 3.5 landed** — macOS + Windows backends (Linux stub), QUIC transport, mDNS discovery, SPAKE2 pairing, monitor-layout SVG canvas with edge-handoff math, HID-based cross-OS keyboard, tray icon, reconnect-with-backoff, lock-to-host, clipboard text sync, signed-or-unsigned installers via Tauri bundling. Real Linux X11 input, ALPN-multiplexed pair+data, and code-signing certificates are the remaining production-readiness pieces.

## Docs

- [PLAN.md](PLAN.md) — design, architecture, roadmap.
- [AGENTS.md](AGENTS.md) — contributor + AI-agent guide.
