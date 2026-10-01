#!/usr/bin/env bash
# Wait for a fork CI run to finish, then stage its Windows MSVC build + PDB into RPCS3\versions\<name>.
# Never switches the active build or touches a running game; switch afterwards with switch-rpcs3.cmd <name>.
#   tools/rpcs3-debug/stage-build.sh <run-id|latest> <version-name> [branch]
# Env: REPO (default jrb00013/rpcs3) RPCS3_DIR (default /mnt/c/Users/josep/RPCS3) WORK (scratch dir)
set -uo pipefail
RUN="${1:?run id or 'latest'}"; NAME="${2:?version name, e.g. 19984-play3}"; BRANCH="${3:-couchlink-play-19984}"
REPO="${REPO:-jrb00013/rpcs3}"; RPCS3_DIR="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}"; WORK="${WORK:-$(mktemp -d)}"
[ "$RUN" = latest ] && RUN=$(gh run list --repo "$REPO" --branch "$BRANCH" --limit 1 --json databaseId --jq '.[0].databaseId')
echo "run $RUN on $REPO ($BRANCH)"
while true; do
  st=$(gh run view "$RUN" --repo "$REPO" --json status,conclusion,jobs --jq '[.status,(.conclusion//""),([.jobs[]|select(.name=="RPCS3 Windows")|.status+"/"+(.conclusion//"")][0]//"")]|join(" ")' 2>/dev/null)
  echo "$(date +%T) $st"
  # the MSVC artifact is uploaded before the job fully finishes; accept it as soon as it exists
  if gh api "repos/$REPO/actions/runs/$RUN/artifacts" --jq '.artifacts[].name' 2>/dev/null | grep -q 'RPCS3 Windows MSVC PDB'; then break; fi
  case "$st" in completed\ failure*|completed\ cancelled*) echo "run did not succeed: $st"; exit 1;; esac
  sleep 60
done
rm -rf "$WORK/exe" "$WORK/pdb" "$WORK/x"; mkdir -p "$WORK/x"
gh run download "$RUN" -R "$REPO" -n "RPCS3 for Windows (MSVC)" -D "$WORK/exe" || exit 1
gh run download "$RUN" -R "$REPO" -n "RPCS3 Windows MSVC PDB" -D "$WORK/pdb" || exit 1
Z=$(find "$WORK/exe" -name '*.7z' | head -1); 7z x -y -o"$WORK/x" "$Z" >/dev/null || exit 1
SRC=$(find "$WORK/x" -name rpcs3.exe | head -1 | xargs dirname)
V="$RPCS3_DIR/versions/$NAME"; mkdir -p "$V/pdb"
cp -a "$SRC/rpcs3.exe" "$V/"; cp -a "$SRC"/*.dll "$V/" 2>/dev/null
for d in qt6 Icons fonts test; do [ -d "$SRC/$d" ] && rsync -a "$SRC/$d/" "$V/$d/"; done
cp -a "$WORK"/pdb/*.pdb "$V/pdb/"
echo "staged: $V  ($(basename "$Z"))"
echo "check: dump trigger string count = $(strings -a "$V/rpcs3.exe" | grep -c 'dump_threads.trigger'); contention log string = $(strings -a "$V/rpcs3.exe" | grep -c 'PPU reservation contention')"
echo "switch with:  cmd.exe /c \"$(wslpath -w "$RPCS3_DIR")\\switch-rpcs3.cmd $NAME\"  (RPCS3 must be closed)"
