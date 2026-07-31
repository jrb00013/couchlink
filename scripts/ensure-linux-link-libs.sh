#!/usr/bin/env bash
# Ensure linker can find XCB libs on native Linux when only runtime packages
# are installed (e.g. libxcb-randr0 without libxcb-randr0-dev). scrap needs
# -lxcb-randr at link time; the unversioned .so symlink comes from -dev.
#
# Source this before `cargo build` on Linux, or run it standalone.
# Safe no-op on non-Linux and when the system already has the linker names.
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  return 0 2>/dev/null || exit 0
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIBDIR="$ROOT/.local-libs"
mkdir -p "$LIBDIR"

# libname → preferred soname candidates under multiarch / lib dirs
ensure_so() {
  local name="$1"
  shift
  local dest="$LIBDIR/lib${name}.so"
  if [[ -e "$dest" || -L "$dest" ]]; then
    return 0
  fi
  # Already linkable from the system (dev package or ldconfig).
  if echo "int main(){}" | cc -x c - -l"$name" -o /dev/null 2>/dev/null; then
    return 0
  fi
  local cand
  for cand in "$@"; do
    if [[ -e "$cand" ]]; then
      ln -sfn "$cand" "$dest"
      echo "==> linux link helper: $dest -> $cand"
      return 0
    fi
  done
  return 0
}

MULTI=(
  /usr/lib/x86_64-linux-gnu
  /usr/lib/aarch64-linux-gnu
  /usr/lib64
  /usr/lib
)

randr_cands=()
xcb_cands=()
shm_cands=()
for d in "${MULTI[@]}"; do
  randr_cands+=("$d/libxcb-randr.so" "$d/libxcb-randr.so.0")
  xcb_cands+=("$d/libxcb.so" "$d/libxcb.so.1")
  shm_cands+=("$d/libxcb-shm.so" "$d/libxcb-shm.so.0")
done

ensure_so xcb-randr "${randr_cands[@]}"
ensure_so xcb "${xcb_cands[@]}"
ensure_so xcb-shm "${shm_cands[@]}"

export RUSTFLAGS="${RUSTFLAGS:-} -L native=${LIBDIR}"
# Also for any nested cargo invocations that inherit the environment.
export LIBRARY_PATH="${LIBDIR}${LIBRARY_PATH:+:$LIBRARY_PATH}"
