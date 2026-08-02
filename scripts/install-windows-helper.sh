#!/usr/bin/env bash
# Build + elevate-install Couchlink Helper on Windows (one UAC).
# Usage: ./scripts/install-windows-helper.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ps_launch='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
[[ -x "$ps_launch" ]] || ps_launch="$(command -v powershell.exe || true)"
[[ -n "${ps_launch:-}" ]] || {
  echo "powershell.exe required (WSL/Windows)" >&2
  exit 1
}

echo "==> building couchlink-helper (Windows release)"
if ! "$ps_launch" -NoProfile -ExecutionPolicy Bypass -Command \
  "Set-Location '$(wslpath -w "$ROOT")'; cargo build --release -p couchlink-windows-helper"; then
  echo "cargo build failed" >&2
  exit 1
fi

exe_w="$(wslpath -w "$ROOT/target/release/couchlink-helper.exe")"
scripts_w="$(wslpath -w "$ROOT/scripts/windows")"
WIN_USER="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
marker_user="/mnt/c/Users/${WIN_USER}/AppData/Local/couchlink-run/helper-install.exit"
marker_pd="/mnt/c/ProgramData/Couchlink/run/helper-install.exit"
mkdir -p "$(dirname "$marker_user")" "$(dirname "$marker_pd")"
rm -f "$marker_user" "$marker_pd"

echo "==> elevating install (approve the Windows UAC prompt)…"
set +e
"$ps_launch" -NoProfile -ExecutionPolicy Bypass -Command \
  "\$p = Start-Process -FilePath '$exe_w' -Verb RunAs -PassThru -Wait -ArgumentList @('install','--script-dir','$scripts_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
ec=$?
set -e

# Prefer marker written by elevated install (more reliable than Start-Process exit under WSL).
if [[ -f "$marker_pd" ]]; then
  ec="$(tr -d '\r\n' <"$marker_pd")"
elif [[ -f "$marker_user" ]]; then
  ec="$(tr -d '\r\n' <"$marker_user")"
fi

if [[ "$ec" != "0" ]]; then
  echo "==> elevated install failed (exit ${ec:-?})" >&2
  echo "    On the Windows desktop, approve UAC, or run elevated PowerShell:" >&2
  echo "      & '$exe_w' install --script-dir '$scripts_w'" >&2
  exit "${ec:-1}"
fi

echo "==> verifying helper pipe…"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-windows-helper.sh"
if couchlink_helper_ping "$ROOT"; then
  echo "OK — Couchlink Helper service is up (no UAC on later --online)"
else
  echo "WARN — install finished but ping failed; check: Get-Service CouchlinkHelper" >&2
  exit 1
fi
