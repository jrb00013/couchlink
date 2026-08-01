#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"

RUN_AFTER=0
RUN_MODE="local"
UNBLOCK_FIREWALL=0
# Default = friend/player. Gaming PC uses --host.
INSTALL_ROLE="${COUCHLINK_INSTALL_ROLE:-client}"
INSTALL_MESH="${COUCHLINK_INSTALL_MESH:-1}"
for arg in "$@"; do
  case "$arg" in
    --run) RUN_AFTER=1 ;;
    --online) RUN_MODE="online"; RUN_AFTER=1 ;;
    --local) RUN_MODE="local"; RUN_AFTER=1 ;;
    --host) INSTALL_ROLE="host" ;;
    --client|--player) INSTALL_ROLE="client" ;; # legacy aliases; default is already client
    --mesh) INSTALL_MESH=1 ;;
    --no-mesh) INSTALL_MESH=0 ;;
    --unblock-firewall) UNBLOCK_FIREWALL=1 ;;
    -h|--help)
      cat <<EOF
usage: ./install.sh [--host] [--run|--online|--local] [--mesh|--no-mesh] [--unblock-firewall]

  Default (friend / player):
    ./install.sh              build player (Headscale-ready; no Tailscale Inc popup)
    ./install.sh --run        then start client --local (paste host join URL)
    ./install.sh --online     then start client --online (paste host join URL;
                              auto-joins Headscale when invite has hs= + tskey=)
    ./install.sh --online --unblock-firewall
                              also open local OS firewall for mesh/TURN

  Host (gaming PC):
    ./install.sh --host                 build host + Headscale + WireGuard
    ./install.sh --host --online        then host --online (Headscale PRIME mesh)
    ./install.sh --host --local|--run   then host --local

  --mesh / --no-mesh   mesh tooling; default on
  Opt-in Tailscale Inc cloud install: COUCHLINK_INSTALL_TAILSCALE_CLOUD=1
EOF
      exit 0
      ;;
  esac
done
# Keep env override authoritative when set explicitly to 0/1 before invoke.
COUCHLINK_INSTALL_MESH="$INSTALL_MESH"
COUCHLINK_INSTALL_ROLE="$INSTALL_ROLE"
export COUCHLINK_INSTALL_MESH COUCHLINK_INSTALL_ROLE

echo "==> couchlink install ($INSTALL_ROLE)"


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
      APT_PKGS=(
        build-essential pkg-config curl ca-certificates
        libx11-dev libxcb1-dev libxcb-shm0-dev libxcb-randr0-dev
        libhidapi-hidraw-dev libudev-dev udev
      )
      if [[ "$INSTALL_ROLE" == "host" ]]; then
        APT_PKGS+=(coturn miniupnpc)
      fi
      if ! as_root apt-get install -y -qq "${APT_PKGS[@]}"; then
        echo "warning: some apt packages failed to install — build may still succeed"
      fi
    fi
    if [[ "$INSTALL_ROLE" == "host" ]]; then
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
    fi
    ;;
  macos)
    echo "==> macOS deps via Homebrew"
    BREW="$(couchlink_brew_bin || true)"
    if [[ -z "$BREW" ]]; then
      echo "Homebrew not found — install from https://brew.sh then re-run ./install.sh"
      echo "Continuing without brew formulae (cargo build may still succeed)…"
    else
      echo "==> brew: $BREW"
      if [[ "$INSTALL_ROLE" == "host" ]]; then
        run_as_user "$BREW" install pkg-config cmake coturn miniupnpc || \
          echo "warning: brew install had errors — continuing"
      else
        run_as_user "$BREW" install pkg-config cmake || \
          echo "warning: brew install had errors — continuing"
      fi
      TOOL_PATH="$(couchlink_tool_path "$REAL_HOME")"
    fi
    if [[ "$INSTALL_ROLE" == "host" ]]; then
      echo "note: virtual DualSense injection is Linux/WSL-only — macOS host streams video only."
    fi
    ;;
  *)
    echo "warning: unrecognized platform '$PLATFORM' — attempting cargo build anyway"
    ;;
esac

# Native Linux: scrap needs -lxcb-randr; no-op on macOS / when linker names exist.
# shellcheck disable=SC1091
source "$ROOT/scripts/ensure-linux-link-libs.sh"

if [[ "$INSTALL_ROLE" == "client" ]]; then
  echo "==> cargo build --release -p couchlink-client …"
  run_as_user env "PATH=$TOOL_PATH${PATH:+:$PATH}" bash -c "
    set -euo pipefail
    cd \"$ROOT\"
    # shellcheck disable=SC1091
    source \"$ROOT/scripts/ensure-linux-link-libs.sh\"
    cargo build --release -p couchlink-client
  "
else
  echo "==> cargo build --release (workspace — this can take several minutes on first run)…"
  run_as_user env "PATH=$TOOL_PATH${PATH:+:$PATH}" bash -c "
    set -euo pipefail
    cd \"$ROOT\"
    # shellcheck disable=SC1091
    source \"$ROOT/scripts/ensure-linux-link-libs.sh\"
    cargo build --release --workspace
  "
fi
echo "==> cargo build done"

mkdir -p "$REAL_HOME/.local/bin"
if [[ "$INSTALL_ROLE" == "host" ]]; then
  couchlink_install_bin target/release/couchlink-signaling "$REAL_HOME/.local/bin/couchlink-signaling"
  couchlink_install_bin target/release/couchlink-host "$REAL_HOME/.local/bin/couchlink-host"
fi
couchlink_install_bin target/release/couchlink-client "$REAL_HOME/.local/bin/couchlink-client"
if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
  chown "$REAL_USER" "$REAL_HOME/.local/bin/couchlink-client" 2>/dev/null || true
  if [[ "$INSTALL_ROLE" == "host" ]]; then
    chown "$REAL_USER" \
      "$REAL_HOME/.local/bin/couchlink-signaling" \
      "$REAL_HOME/.local/bin/couchlink-host" 2>/dev/null || true
  fi
fi

if [[ "$INSTALL_ROLE" == "host" && "$PLATFORM" == "wsl" ]]; then
  echo "==> building Windows capture bridge (auto for WSL → Windows desktop/window)"
  if command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
      "$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")"
  else
    echo "warning: powershell.exe missing — cannot build couchlink-win-capture.exe"
  fi
fi

if [[ "$INSTALL_ROLE" == "host" ]]; then
  if run_as_user bash -lc 'command -v npm >/dev/null'; then
    echo "==> building player UI"
    run_as_user bash -c "cd '$ROOT/web' && npm install && npm run build"
  else
    echo "npm not found — skipping player UI (browser client needs: cd web && npm install && npm run build)"
  fi
else
  echo "==> skipping web UI build (client uses native Couchlink Player)"
fi

# Optional helpers only — not required for host/client. Skip unless opted in;
# a global `poetry` from unrelated projects makes this look like a hang.
if [[ "${COUCHLINK_INSTALL_PYTHON:-0}" == "1" ]] && run_as_user bash -lc 'command -v poetry >/dev/null'; then
  echo "==> installing python helpers (poetry)"
  run_as_user bash -c "cd '$ROOT/python' && poetry install"
else
  echo "==> skipping python helpers (optional; set COUCHLINK_INSTALL_PYTHON=1 to enable)"
fi

if [[ ! -f .env.couchlink ]]; then
  cp .env.example .env.couchlink
  if [[ "${EUID:-$(id -u)}" -eq 0 && "$REAL_USER" != "root" ]]; then
    chown "$REAL_USER" .env.couchlink 2>/dev/null || true
  fi
fi

# Mesh tooling — Headscale PRIME. Tailscale Inc cloud install is OPT-IN only
# (kept in scripts/setup-tailscale.sh — do not auto-run; it pops Windows UAC).
# Disable mesh tooling with --no-mesh or COUCHLINK_INSTALL_MESH=0.
# Opt into Tailscale Inc cloud: COUCHLINK_INSTALL_TAILSCALE_CLOUD=1
if [[ "${COUCHLINK_INSTALL_MESH:-1}" == "1" ]]; then
  if [[ "$INSTALL_ROLE" == "client" ]]; then
    echo "==> Headscale-ready client (open-source mesh client; no Tailscale Inc login)"
    if [[ -x "$ROOT/scripts/ensure-headscale-client.sh" ]]; then
      run_as_user bash "$ROOT/scripts/ensure-headscale-client.sh" >/dev/null \
        || echo "warning: Headscale client binary not ready — will install on first --online join"
    fi
    if [[ "${COUCHLINK_INSTALL_TAILSCALE_CLOUD:-0}" == "1" && -x "$ROOT/scripts/setup-tailscale.sh" ]]; then
      echo "==> COUCHLINK_INSTALL_TAILSCALE_CLOUD=1 — installing Tailscale Inc client (optional fallback)"
      run_as_user bash "$ROOT/scripts/setup-tailscale.sh" --ensure \
        || echo "warning: setup-tailscale.sh failed — see docs/MESH.md"
    fi
    if [[ "$UNBLOCK_FIREWALL" == "1" ]]; then
      bash "$ROOT/scripts/unblock-firewall.sh" \
        || echo "warning: unblock-firewall failed (best-effort)"
    fi
  else
    echo "==> mesh tooling (Headscale PRIME + WireGuard fallback)"
    case "$PLATFORM" in
      linux|wsl)
        if command -v apt-get >/dev/null; then
          as_root apt-get install -y -qq wireguard wireguard-tools 2>/dev/null \
            || echo "warning: apt wireguard install failed — install wireguard-tools manually"
        fi
        ;;
      macos)
        BREW="$(couchlink_brew_bin || true)"
        if [[ -n "${BREW:-}" ]]; then
          run_as_user "$BREW" install wireguard-tools \
            || echo "warning: brew wireguard-tools failed"
        fi
        ;;
    esac
    if [[ -x "$ROOT/scripts/setup-headscale.sh" ]]; then
      echo "==> Headscale binary + config (PRIME mesh for --online)"
      run_as_user bash "$ROOT/scripts/setup-headscale.sh" \
        || echo "warning: setup-headscale.sh failed — see docs/HEADSCALE.md"
    fi
    if [[ -x "$ROOT/scripts/ensure-headscale-client.sh" ]]; then
      run_as_user bash "$ROOT/scripts/ensure-headscale-client.sh" >/dev/null \
        || echo "warning: Headscale client binary not ready — enable-headscale will retry"
    fi
    if [[ "${COUCHLINK_INSTALL_TAILSCALE_CLOUD:-0}" == "1" && -x "$ROOT/scripts/setup-tailscale.sh" ]]; then
      echo "==> COUCHLINK_INSTALL_TAILSCALE_CLOUD=1 — Tailscale Inc fallback (optional)"
      run_as_user bash "$ROOT/scripts/setup-tailscale.sh" --ensure \
        || echo "warning: setup-tailscale.sh failed — see docs/MESH.md"
    fi
    if [[ -x "$ROOT/scripts/setup-wireguard.sh" ]]; then
      run_as_user bash "$ROOT/scripts/setup-wireguard.sh" \
        || echo "warning: setup-wireguard.sh failed — see docs/WIREGUARD.md"
    fi
    if [[ -x "$ROOT/scripts/enable-wireguard.sh" ]]; then
      echo "==> bringing WireGuard tunnel up (fallback if Headscale down; UAC once on Windows/WSL)"
      bash "$ROOT/scripts/enable-wireguard.sh" \
        || echo "warning: enable-wireguard failed — import conf manually (docs/MESH.md)"
    fi
  fi
fi

echo ""
echo "OK — install finished ($INSTALL_ROLE)"
echo "  binaries: $REAL_HOME/.local/bin"
if [[ "$INSTALL_ROLE" == "client" ]]; then
  echo "  next:    ./install.sh --online                 # paste host join URL (Headscale auto-join)"
  echo "           ./install.sh --online --unblock-firewall"
  echo "  tip:     no Tailscale Inc account — hs=/tskey= in the invite"
else
  echo "  next:    ./install.sh --host --online"
  echo "  friend:  ./install.sh && ./install.sh --online"
  echo "  docs:    docs/HEADSCALE.md"
fi
echo ""

if [[ "$RUN_AFTER" == "1" ]]; then
  RUN_ARGS=("$INSTALL_ROLE" "--${RUN_MODE}")
  if [[ "$UNBLOCK_FIREWALL" == "1" ]]; then
    RUN_ARGS+=(--unblock-firewall)
  fi
  echo "==> starting ./scripts/run.sh ${RUN_ARGS[*]}"
  exec bash "$ROOT/scripts/run.sh" "${RUN_ARGS[@]}"
fi

# Auto-source .env.couchlink only when explicitly requested — the interactive
# shell handoff looks like a hang / mysterious prompt after a long cargo build.
if [[ -t 0 && -t 1 && "${COUCHLINK_SHELL_HANDOFF:-0}" == "1" && "${EUID:-$(id -u)}" -ne 0 ]]; then
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
  echo "Installed as root for $REAL_USER — open a normal shell, then: source .env.couchlink"
else
  echo "Load env vars with: source .env.couchlink"
fi
