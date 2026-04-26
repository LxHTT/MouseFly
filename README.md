# MouseFly

One keyboard and mouse, many computers — across Windows and macOS over LAN. Move the cursor off the edge of one host's screen and it appears on the neighbor's, with key events following along.

Status: **Phases 0–5 landed** — macOS + Windows backends (Linux stub), QUIC transport, mDNS discovery, SPAKE2 pairing, monitor-layout SVG canvas, tray icon, reconnect-with-backoff, lock-to-host, clipboard text sync. Edge-handoff math (cursor crossing → remote inject) and full Linux input are next.

## Docs

- [PLAN.md](PLAN.md) — design, architecture, roadmap.
- [AGENTS.md](AGENTS.md) — contributor + AI-agent guide.
