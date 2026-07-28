#!/usr/bin/env bash
# Print a random session id + 6-digit PIN
set -euo pipefail
SID=$(head -c 6 /dev/urandom | xxd -p)
PIN=$(printf '%06d' $((RANDOM % 1000000)))
echo "COUCHLINK_SESSION_ID=$SID"
echo "COUCHLINK_PIN=$PIN"
