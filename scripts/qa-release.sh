#!/bin/sh

set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

finish() {
  status=$?
  trap - EXIT
  if pgrep -x cloudflared >/dev/null; then
    echo "Release QA found an orphan cloudflared process:" >&2
    pgrep -fl cloudflared >&2
    status=1
  fi
  exit "$status"
}
trap finish EXIT

if pgrep -x cloudflared >/dev/null; then
  echo "Stop existing Porta/cloudflared sessions before running release QA." >&2
  exit 1
fi

cd "$repo_root"

echo "==> UI unit tests and production build"
npm --prefix ui run test
npm --prefix ui run build

echo "==> Rust tests, clippy, and formatting"
(
  cd src-tauri
  cargo test
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check

  echo "==> Ten bundled-cloudflared start/stop cycles"
  cargo test \
    tunnel::tests::bundled_cloudflared_has_zero_orphans_after_ten_start_stop_cycles \
    -- --ignored --exact --nocapture
)

echo "Release QA passed: UI tests, TypeScript, Rust, clippy, formatting, and zero cloudflared orphans after 10 cycles."
