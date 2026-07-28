#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "==> couchlink install"

if ! command -v cargo >/dev/null; then
  echo "Rust/cargo required: https://rustup.rs"
  exit 1
fi

PLATFORM="linux"
if grep -qi microsoft /proc/version 2>/dev/null; then
  PLATFORM="wsl"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  PLATFORM="macos"
fi
echo "==> platform: $PLATFORM"

# Linux/WSL deps for capture + uinput + hid
if [[ "$PLATFORM" == "linux" || "$PLATFORM" == "wsl" ]]; then
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential pkg-config libx11-dev libxcb1-dev \
      libxcb-shm0-dev libxcb-randr0-dev libhidapi-hidraw-dev libudev-dev udev coturn miniupnpc || true
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
  [[ "$PLATFORM" == "wsl" ]] && echo "WSL: host role needs uinput passed through (usbipd-win / wsl2 kernel with CONFIG_INPUT_UINPUT)"
elif [[ "$PLATFORM" == "macos" ]]; then
  if command -v brew >/dev/null; then
    brew install pkg-config hidapi coturn miniupnpc || true
  else
    echo "Homebrew not found — install manually: https://brew.sh"
  fi
  echo "macOS: no uinput — host role (virtual pad injection) is Linux/WSL only. macOS can run signaling/turn/client."
fi

cargo build --release --workspace
mkdir -p "$HOME/.local/bin"
install -Dm755 target/release/couchlink-signaling "$HOME/.local/bin/couchlink-signaling"
install -Dm755 target/release/couchlink-host "$HOME/.local/bin/couchlink-host"
install -Dm755 target/release/couchlink-client "$HOME/.local/bin/couchlink-client"

if [[ "$PLATFORM" == "wsl" ]]; then
  echo "==> building Windows capture bridge (auto for WSL → Windows desktop/window)"
  if command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
      "$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")"
  else
    echo "warning: powershell.exe missing — cannot build couchlink-win-capture.exe"
  fi
fi

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
echo "Run everything with: ./scripts/run.sh host   (or ./scripts/run.sh client to join a friend)"

# Auto-source .env.couchlink for you: a script can't export vars into the shell
# that launched it, so instead we hand you back an interactive shell that
# already has it sourced — only when run interactively (not from CI/non-tty).
if [[ -t 0 && -t 1 && -z "${COUCHLINK_NO_SHELL_HANDOFF:-}" ]]; then
  case "${SHELL:-}" in
    */bash|"")
      echo "==> dropping you into a bash shell with .env.couchlink already sourced"
      exec bash --rcfile <(echo "[ -f ~/.bashrc ] && source ~/.bashrc; source '$ROOT/.env.couchlink'") -i
      ;;
    */zsh)
      echo "==> dropping you into a zsh shell with .env.couchlink already sourced"
      TMP_ZDOTDIR="$(mktemp -d)"
      { [[ -f "$HOME/.zshrc" ]] && cat "$HOME/.zshrc"; echo "source '$ROOT/.env.couchlink'"; } > "$TMP_ZDOTDIR/.zshrc"
      ZDOTDIR="$TMP_ZDOTDIR" exec zsh -i
      ;;
    *)
      echo "Run this to load env vars: source .env.couchlink"
      ;;
  esac
fi
