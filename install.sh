#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "==> couchlink install"

# When invoked via `sudo ./install.sh`, keep the invoking user's home/PATH —
# root's login PATH does not include ~/.cargo/bin, and we must not install
# Rust or binaries into /root.
if [[ "${EUID:-$(id -u)}" -eq 0 && -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
  REAL_USER="$SUDO_USER"
  REAL_HOME="$(getent passwd "$REAL_USER" | cut -d: -f6)"
  REAL_HOME="${REAL_HOME:-/home/$REAL_USER}"
else
  REAL_USER="$(id -un)"
  REAL_HOME="${HOME:-$(getent passwd "$REAL_USER" | cut -d: -f6)}"
fi

run_as_user() {
  local user_env=(
    "HOME=$REAL_HOME"
    "USER=$REAL_USER"
    "LOGNAME=$REAL_USER"
    "CARGO_HOME=$REAL_HOME/.cargo"
    "RUSTUP_HOME=$REAL_HOME/.rustup"
    "PATH=$REAL_HOME/.cargo/bin:$REAL_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin"
  )
  if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
    # Drop privileges for build/tooling — never write cargo artifacts as root.
    sudo -u "$REAL_USER" -H env "${user_env[@]}" "$@"
  else
    env "${user_env[@]}" "$@"
  fi
}

as_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null; then
    sudo "$@"
  else
    echo "need root for: $*" >&2
    exit 1
  fi
}

ensure_rust() {
  export PATH="$REAL_HOME/.cargo/bin:$PATH"
  if command -v cargo >/dev/null 2>&1 || [[ -x "$REAL_HOME/.cargo/bin/cargo" ]]; then
    export PATH="$REAL_HOME/.cargo/bin:$PATH"
    echo "==> rust: $(run_as_user cargo --version 2>/dev/null || "$REAL_HOME/.cargo/bin/cargo" --version)"
    return 0
  fi

  echo "==> Rust/cargo not found — installing rustup for $REAL_USER"
  local platform
  platform="$(uname -s)"
  case "$platform" in
    Linux|Darwin) ;;
    *)
      echo "Rust/cargo required: https://rustup.rs" >&2
      exit 1
      ;;
  esac

  run_as_user bash -c '
    set -euo pipefail
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  '
  export PATH="$REAL_HOME/.cargo/bin:$PATH"
  if ! run_as_user bash -lc 'command -v cargo >/dev/null'; then
    echo "rustup install finished but cargo is still missing on PATH for $REAL_USER" >&2
    exit 1
  fi
  echo "==> $(run_as_user bash -lc 'cargo --version')"
}

ensure_rust

PLATFORM="linux"
if grep -qi microsoft /proc/version 2>/dev/null; then
  PLATFORM="wsl"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  PLATFORM="macos"
fi
echo "==> platform: $PLATFORM (user: $REAL_USER)"

# Linux/WSL deps for capture + uinput + hid
if [[ "$PLATFORM" == "linux" || "$PLATFORM" == "wsl" ]]; then
  if command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    echo "==> apt: refreshing package lists (can take a minute)…"
    # Broken third-party PPAs must not abort install — continue with cached indexes.
    if ! as_root apt-get update -qq; then
      echo "warning: apt-get update reported errors (often a dead PPA) — continuing with existing indexes"
    fi
    echo "==> apt: installing build + runtime deps…"
    if ! as_root apt-get install -y -qq \
      build-essential pkg-config curl ca-certificates \
      libx11-dev libxcb1-dev libxcb-shm0-dev libxcb-randr0-dev \
      libhidapi-hidraw-dev libudev-dev udev coturn miniupnpc; then
      echo "warning: some apt packages failed to install — build may still succeed"
    fi
  fi
  echo "==> configuring /dev/uinput access…"
  # uinput access — apply for the real user, not root
  as_root tee /etc/udev/rules.d/99-couchlink-uinput.rules >/dev/null <<'RULE'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
RULE
  as_root udevadm control --reload-rules || true
  as_root modprobe uinput || true
  if getent group input >/dev/null; then
    as_root usermod -aG input "$REAL_USER" || true
    echo "Added $REAL_USER to group 'input' — re-login may be required for /dev/uinput"
  fi
  # Make current session usable immediately when install runs as root.
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    chmod 666 /dev/uinput 2>/dev/null || true
  fi
  [[ "$PLATFORM" == "wsl" ]] && echo "WSL: host role needs uinput passed through (usbipd-win / wsl2 kernel with CONFIG_INPUT_UINPUT)"
elif [[ "$PLATFORM" == "macos" ]]; then
  if command -v brew >/dev/null; then
    run_as_user brew install pkg-config hidapi coturn miniupnpc || true
  else
    echo "Homebrew not found — install manually: https://brew.sh"
  fi
  echo "macOS: no uinput — host role (virtual pad injection) is Linux/WSL only. macOS can run signaling/turn/client."
fi

# Native Linux: scrap needs -lxcb-randr; provide linker names if only runtime
# packages are present (no -dev / no sudo). No-op when system already links.
# shellcheck disable=SC1091
source "$ROOT/scripts/ensure-linux-link-libs.sh"

echo "==> cargo build --release (this can take several minutes on first run)…"
run_as_user bash -c "
  set -euo pipefail
  cd '$ROOT'
  # shellcheck disable=SC1091
  source '$ROOT/scripts/ensure-linux-link-libs.sh'
  cargo build --release --workspace
"
echo "==> cargo build done"

mkdir -p "$REAL_HOME/.local/bin"
install -Dm755 target/release/couchlink-signaling "$REAL_HOME/.local/bin/couchlink-signaling"
install -Dm755 target/release/couchlink-host "$REAL_HOME/.local/bin/couchlink-host"
install -Dm755 target/release/couchlink-client "$REAL_HOME/.local/bin/couchlink-client"
if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
  chown -R "$REAL_USER:" "$REAL_HOME/.local/bin/couchlink-signaling" \
    "$REAL_HOME/.local/bin/couchlink-host" \
    "$REAL_HOME/.local/bin/couchlink-client" 2>/dev/null || true
fi

if [[ "$PLATFORM" == "wsl" ]]; then
  echo "==> building Windows capture bridge (auto for WSL → Windows desktop/window)"
  if command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
      "$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")"
  else
    echo "warning: powershell.exe missing — cannot build couchlink-win-capture.exe"
  fi
fi

if run_as_user bash -lc 'command -v npm >/dev/null'; then
  echo "==> building player UI"
  run_as_user bash -c "cd '$ROOT/web' && npm install && npm run build"
fi

if run_as_user bash -lc 'command -v poetry >/dev/null'; then
  echo "==> installing python helpers (poetry)"
  run_as_user bash -c "cd '$ROOT/python' && poetry install"
else
  echo "poetry not found — skipping python helpers (https://python-poetry.org/docs/#installation)"
fi

if [[ ! -f .env.couchlink ]]; then
  cp .env.example .env.couchlink
  if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
    chown "$REAL_USER:" .env.couchlink
  fi
fi

echo "OK — binaries in $REAL_HOME/.local/bin"
echo "Run everything with: ./scripts/run.sh host   (or ./scripts/run.sh client to join a friend)"

# Auto-source .env.couchlink for you: a script can't export vars into the shell
# that launched it, so instead we hand you back an interactive shell that
# already has it sourced — only when run interactively (not from CI/non-tty).
# Skip handoff when root — drop back to the invoking user instead of a root shell.
if [[ -t 0 && -t 1 && -z "${COUCHLINK_NO_SHELL_HANDOFF:-}" && "${EUID:-$(id -u)}" -ne 0 ]]; then
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
elif [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
  echo "Installed as root for $REAL_USER — open a new shell (or re-login for the 'input' group) then: source .env.couchlink"
fi
