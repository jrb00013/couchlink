#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "==> couchlink install"

if ! command -v cargo >/dev/null; then
  echo "Rust/cargo required: https://rustup.rs"
  exit 1
fi

# Linux deps for capture + uinput + hid
if [[ "$(uname -s)" == Linux ]]; then
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential pkg-config libx11-dev libxcb1-dev \
      libxcb-shm0-dev libxcb-randr0-dev libhidapi-hidraw-dev libudev-dev udev || true
  fi
  # uinput access
  sudo tee /etc/udev/rules.d/99-couchlink-uinput.rules >/dev/null <<'RULE'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
RULE
  sudo udevadm control --reload-rules || true
  sudo modprobe uinput || true
  if getent group input >/dev/null; then
    sudo usermod -aG input "$USER" || true
    echo "Added $USER to group 'input' — re-login may be required for /dev/uinput"
  fi
fi

cargo build --release --workspace
mkdir -p "$HOME/.local/bin"
install -Dm755 target/release/couchlink-signaling "$HOME/.local/bin/couchlink-signaling"
install -Dm755 target/release/couchlink-host "$HOME/.local/bin/couchlink-host"
install -Dm755 target/release/couchlink-client "$HOME/.local/bin/couchlink-client"

if [[ ! -f .env.couchlink ]]; then
  cp .env.example .env.couchlink
fi

mkdir -p web/dist
if [[ ! -f web/dist/index.html ]]; then
  cat > web/dist/index.html <<'HTML'
<!doctype html>
<html><head><meta charset="utf-8"><title>couchlink</title>
<style>
  body{font-family:system-ui;background:#0b0f14;color:#e8eef7;display:grid;place-items:center;min-height:100vh;margin:0}
  main{max-width:36rem;padding:2rem}
  code{background:#1a2330;padding:.1rem .35rem;border-radius:4px}
</style></head>
<body><main>
<h1>couchlink</h1>
<p>Signaling is up. Run <code>couchlink-host</code> and <code>couchlink-client</code> for HD co-play.</p>
<p>See docs/GETTING_STARTED.md</p>
</main></body></html>
HTML
fi

echo "OK — binaries in ~/.local/bin"
echo "source .env.couchlink && couchlink-signaling"
