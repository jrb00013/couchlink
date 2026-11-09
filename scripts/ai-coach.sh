#!/usr/bin/env bash
# Opt-in AI game coach (local Ollama + coach container). See docs/AI_GAME_COACH.md.
#
#   scripts/ai-coach.sh up [--gpu]        build + start ollama and the coach (loopback only)
#   scripts/ai-coach.sh down              stop both (model volume is kept)
#   scripts/ai-coach.sh status            containers, coach health, current game/seats
#   scripts/ai-coach.sh pull-model        download COUCHLINK_AI_COACH_MODEL (one-shot, has egress)
#   scripts/ai-coach.sh chat SEAT "text"  ask the coach as a seat (dev/test helper)
#
# Nothing here touches the running host/signaling/capture stack. The host only
# talks to the coach when it is started with COUCHLINK_AI_COACH=1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/docker/ai-coach"
PORT="${COUCHLINK_AI_COACH_PORT:-18790}"
BASE="http://127.0.0.1:$PORT"

compose() {
  local files=(-f "$DIR/docker-compose.yml")
  [[ "${COUCHLINK_AI_COACH_GPU:-0}" == "1" ]] && files+=(-f "$DIR/docker-compose.gpu.yml")
  docker compose "${files[@]}" "$@"
}

need_docker() {
  command -v docker >/dev/null 2>&1 || { echo "docker not found" >&2; exit 1; }
}

cmd="${1:-status}"
shift || true

case "$cmd" in
  up)
    need_docker
    [[ "${1:-}" == "--gpu" ]] && export COUCHLINK_AI_COACH_GPU=1
    compose up -d --build
    echo "==> coach API: $BASE (loopback only)"
    echo "==> model: ${COUCHLINK_AI_COACH_MODEL:-qwen2.5:3b} — run '$0 pull-model' once if not downloaded"
    echo "==> enable in the host with: COUCHLINK_AI_COACH=1 COUCHLINK_AI_COACH_SEAT=1 (see docs/AI_GAME_COACH.md)"
    ;;
  down)
    need_docker
    compose down
    ;;
  pull-model)
    need_docker
    compose --profile pull run --rm ollama-pull
    ;;
  status)
    if command -v docker >/dev/null 2>&1; then
      compose ps 2>/dev/null || true
    fi
    if curl -fsS --max-time 2 "$BASE/health" >/dev/null 2>&1; then
      echo "coach: up"
      curl -fsS --max-time 2 "$BASE/v1/coach/state" || true
      echo
    else
      echo "coach: not reachable at $BASE"
    fi
    ;;
  chat)
    seat="${1:?usage: $0 chat SEAT \"message\"}"
    msg="${2:?usage: $0 chat SEAT \"message\"}"
    [[ "$seat" =~ ^[0-3]$ ]] || { echo "seat must be 0-3" >&2; exit 1; }
    body="$(python3 -c 'import json,sys; print(json.dumps({"seat": int(sys.argv[1]), "message": sys.argv[2]}))' "$seat" "$msg")"
    curl -fsS --max-time 120 -H 'Content-Type: application/json' -d "$body" "$BASE/v1/coach/chat"
    echo
    ;;
  *)
    echo "usage: $0 {up [--gpu]|down|status|pull-model|chat SEAT \"text\"}" >&2
    exit 2
    ;;
esac
