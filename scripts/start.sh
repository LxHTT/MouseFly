#!/usr/bin/env bash
# Boot a full MouseFly loopback dev session on one Mac:
#   - Receiver: Tauri dev (Vite + cargo debug build), GUI listens on :7878.
#   - Sender:   release binary with its own GUI, connects to 127.0.0.1:7878.
# Both windows show live link health from the same RTT probe.
#
# Loopback safety: --inject is intentionally OFF (cursor would feedback-loop).
# Use `bun run receiver:inject` on a real second machine for actual mouse forwarding.
#
# Stop with Ctrl+C — child processes are cleaned up.

set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Building frontend dist (sender's standalone window needs it)"
bunx turbo run build --filter=mousefly-ui

echo "==> Building release binary (sender)"
cargo build --release -p mousefly-app

cleanup() {
  echo
  echo "==> Stopping…"
  kill 0 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup INT TERM EXIT

echo "==> Starting receiver (Tauri dev, listening on 0.0.0.0:7878)"
bun run receiver &

echo "==> Waiting for receiver to bind :7878"
until lsof -nP -iUDP:7878 >/dev/null 2>&1; do
  sleep 1
done
sleep 2  # let Tauri finish opening its window before the sender's appears

echo "==> Starting sender (release binary → 127.0.0.1:7878)"
./target/release/mousefly --peer 127.0.0.1:7878
