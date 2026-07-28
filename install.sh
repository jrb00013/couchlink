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
      libxcb-shm0-dev libxcb-randr0-dev libhidapi-hidraw-dev libudev-dev udev coturn || true
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

if command -v npm >/dev/null; then
  echo "==> building player UI"
  (cd web && npm install && npm run build)
fi

if command -v poetry >/dev/null; then
  echo "==> installing python helpers (poetry)"
  (cd python && poetry install)
else
  echo "poetry not found — skipping python helpers (https://python-poetry.org/docs/#installation)"
fi

if [[ ! -f .env.couchlink ]]; then
  cp .env.example .env.couchlink
fi

echo "OK — binaries in ~/.local/bin"
echo "source .env.couchlink && ./scripts/start-signaling.sh"
echo "Friend opens the join URL printed by couchlink-host (or http://HOST:8443)"
