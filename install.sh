#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"

echo "==> couchlink install"

# When invoked via `sudo ./install.sh`, keep the invoking user's home/PATH —
# root's login PATH does not include ~/.cargo/bin, and we must not install
# Rust or binaries into /root (or /var/root on macOS).
if [[ "${EUID:-$(id -u)}" -eq 0 && -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
  REAL_USER="$SUDO_USER"
  REAL_HOME="$(couchlink_user_home "$REAL_USER")"
else
  REAL_USER="$(id -un)"
  REAL_HOME="$(couchlink_user_home "$REAL_USER")"
  REAL_HOME="${REAL_HOME:-${HOME:-}}"
fi

TOOL_PATH="$(couchlink_tool_path "$REAL_HOME")"

run_as_user() {
  local user_env=(
    "HOME=$REAL_HOME"
    "USER=$REAL_USER"
    "LOGNAME=$REAL_USER"
    "CARGO_HOME=$REAL_HOME/.cargo"
    "RUSTUP_HOME=$REAL_HOME/.rustup"
    "PATH=$TOOL_PATH"
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
  export PATH="$TOOL_PATH${PATH:+:$PATH}"
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

  if [[ "$platform" == "Darwin" ]] && ! xcode-select -p >/dev/null 2>&1; then
    echo "==> Xcode Command Line Tools required — triggering install prompt"
    xcode-select --install 2>/dev/null || true
    echo "Install the CLT, then re-run ./install.sh" >&2
    exit 1
  fi

  run_as_user bash -c '
    set -euo pipefail
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  '
  export PATH="$REAL_HOME/.cargo/bin:$PATH"
  TOOL_PATH="$(couchlink_tool_path "$REAL_HOME")"
  if ! run_as_user bash -lc 'command -v cargo >/dev/null'; then
    echo "rustup install finished but cargo is still missing on PATH for $REAL_USER" >&2
    exit 1
  fi
  echo "==> $(run_as_user cargo --version)"
}

ensure_rust

PLATFORM="$(couchlink_detect_platform)"
echo "==> platform: $PLATFORM (user: $REAL_USER, home: $REAL_HOME)"

case "$PLATFORM" in
  linux|wsl)
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
    as_root tee /etc/udev/rules.d/99-couchlink-uinput.rules >/dev/null <<'RULE'
KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
RULE
    as_root udevadm control --reload-rules || true
    as_root modprobe uinput || true
    if getent group input >/dev/null; then
      as_root usermod -aG input "$REAL_USER" || true
      echo "Added $REAL_USER to group 'input' — re-login may be required for /dev/uinput"
    fi
    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
      chmod 666 /dev/uinput 2>/dev/null || true
    fi
    [[ "$PLATFORM" == "wsl" ]] && echo "WSL: host role needs uinput passed through (usbipd-win / wsl2 kernel with CONFIG_INPUT_UINPUT)"
    ;;
  macos)
    echo "==> macOS deps via Homebrew (client + signaling + video-only host)"
    BREW="$(couchlink_brew_bin || true)"
    if [[ -z "$BREW" ]]; then
      echo "Homebrew not found — install from https://brew.sh then re-run ./install.sh"
      echo "Continuing without brew formulae (cargo build may still succeed)…"
    else
      echo "==> brew: $BREW"
      # pkg-config for native crates; coturn/miniupnpc for --online; cmake for some deps.
      run_as_user "$BREW" install pkg-config cmake coturn miniupnpc || \
        echo "warning: brew install had errors — continuing"
      TOOL_PATH="$(couchlink_tool_path "$REAL_HOME")"
    fi
    echo "note: virtual DualSense injection is Linux/WSL-only — macOS host streams video only; use './scripts/run.sh client' to play."
    ;;
  *)
    echo "warning: unrecognized platform '$PLATFORM' — attempting cargo build anyway"
    ;;
esac

# Native Linux: scrap needs -lxcb-randr; no-op on macOS / when linker names exist.
# shellcheck disable=SC1091
source "$ROOT/scripts/ensure-linux-link-libs.sh"

echo "==> cargo build --release (this can take several minutes on first run)…"
run_as_user env "PATH=$TOOL_PATH${PATH:+:$PATH}" bash -c "
  set -euo pipefail
  cd \"$ROOT\"
  # shellcheck disable=SC1091
  source \"$ROOT/scripts/ensure-linux-link-libs.sh\"
  cargo build --release --workspace
"
echo "==> cargo build done"

mkdir -p "$REAL_HOME/.local/bin"
couchlink_install_bin target/release/couchlink-signaling "$REAL_HOME/.local/bin/couchlink-signaling"
couchlink_install_bin target/release/couchlink-host "$REAL_HOME/.local/bin/couchlink-host"
couchlink_install_bin target/release/couchlink-client "$REAL_HOME/.local/bin/couchlink-client"
if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
  chown "$REAL_USER" \
    "$REAL_HOME/.local/bin/couchlink-signaling" \
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
    chown "$REAL_USER" .env.couchlink 2>/dev/null || true
  fi
fi

echo "OK — binaries in $REAL_HOME/.local/bin"
case "$PLATFORM" in
  macos)
    echo "Run friend/client:  ./scripts/run.sh client"
    echo "Run video-only host: ./scripts/run.sh host --local   (pad injection needs Linux/WSL)"
    ;;
  *)
    echo "Run everything with: ./scripts/run.sh host   (or ./scripts/run.sh client to join a friend)"
    ;;
esac

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
