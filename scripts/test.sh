#!/usr/bin/env bash
# Local test gate. Runs every check that doesn't need user-granted OS perms or
# a second machine. Mirrors PLAN.md §verification items 1–3.
#
# Manual checks (kill-switch, real input capture, two-Mac inject) stay manual.

set -euo pipefail
cd "$(dirname "$0")/.."

c_blue='\033[1;34m'; c_green='\033[1;32m'; c_red='\033[1;31m'; c_off='\033[0m'
step() { printf "\n${c_blue}==> %s${c_off}\n" "$*"; }
ok()   { printf "${c_green}✔ %s${c_off}\n" "$*"; }
fail() { printf "${c_red}✘ %s${c_off}\n" "$*" >&2; exit 1; }

step "Rust formatting (cargo fmt --check)"
cargo fmt --all -- --check && ok "fmt clean"

step "Rust lints (cargo clippy -D warnings, host)"
cargo clippy --workspace --all-targets -- -D warnings && ok "clippy clean"

# Optional cross-target check: if the Windows target is installed, also lint
# the Windows backend so we catch regressions before pushing to CI.
if rustup target list --installed 2>/dev/null | grep -q '^x86_64-pc-windows-msvc$'; then
  step "Rust lints (cargo clippy, target=x86_64-pc-windows-msvc)"
  cargo clippy -p mousefly-input --target x86_64-pc-windows-msvc -- -D warnings && \
    ok "clippy clean (windows-msvc cross-check)"
else
  printf "  (skipping windows-msvc cross-check; install with: rustup target add x86_64-pc-windows-msvc)\n"
fi

step "Rust tests (cargo test --workspace)"
cargo test --workspace --quiet && ok "tests pass"

step "Rust release build"
cargo build --workspace --release --quiet && ok "release build"

step "Frontend typecheck (vue-tsc)"
pnpm --filter mousefly-ui typecheck && ok "typecheck clean"

step "Frontend build (vite)"
pnpm --filter mousefly-ui build && ok "vite build"

step "Smoke: receiver binds TCP port"
PORT=17878  # avoid colliding with the dev default 7878
LOG=$(mktemp)
RUST_LOG=info ./target/release/mousefly --listen 127.0.0.1:$PORT >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; rm -f "$LOG"' EXIT

# Wait up to ~5 s for the listener to come up.
for _ in $(seq 1 25); do
  if lsof -nP -iTCP:$PORT -sTCP:LISTEN -p $PID >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done

if lsof -nP -iTCP:$PORT -sTCP:LISTEN -p $PID >/dev/null 2>&1; then
  ok "receiver bound 127.0.0.1:$PORT (pid $PID)"
else
  echo "--- receiver log ---" >&2
  cat "$LOG" >&2
  fail "receiver never bound :$PORT"
fi

step "Smoke: TCP handshake (sender → receiver)"
# Quick TCP probe instead of running the full sender (which needs CGEventTap
# permissions and would error before connecting in CI).
if (echo > /dev/tcp/127.0.0.1/$PORT) >/dev/null 2>&1; then
  ok "tcp connect succeeded"
else
  fail "tcp connect to :$PORT failed"
fi

printf "\n${c_green}All local checks passed.${c_off}\n"
