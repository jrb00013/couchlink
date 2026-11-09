"""Coach HTTP API. Container-internal bind; compose publishes it on 127.0.0.1 only.

  GET  /health               liveness (no model call)
  GET  /v1/coach/state       current game + seats + bound seats
  POST /v1/context           host -> coach: {"title": str, "seats": [{"slot","pad_kind"}], "bound_seats": [int]}
  POST /v1/coach/chat        {"seat": int, "message": str} -> {"reply", "tools_used", "game"}
  POST /v1/coach/reset       {"seat": int}

The context endpoint accepts exactly title + seat slots/pad kinds: no session id,
PIN or join link ever reaches this process.
"""
from __future__ import annotations

import json
import os
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from . import pure
from .agent import Busy, Coach, SeatRejected
from .llm import OllamaClient
from .web import SafeWeb

MAX_BODY = 4096


def make_coach() -> Coach:
    llm = OllamaClient()
    web = None
    if os.environ.get("COUCHLINK_AI_COACH_WEB", "1") not in ("0", "false", "off"):
        allow = [a for a in os.environ.get("COUCHLINK_AI_COACH_ALLOW", "").replace(" ", "").split(",") if a]
        web = SafeWeb(allow or pure.DEFAULT_ALLOWLIST)
    return Coach(llm.chat, web)


def make_handler(coach: Coach):
    class H(BaseHTTPRequestHandler):
        server_version = "couchlink-coach"

        def log_message(self, fmt, *a):  # never log request bodies (player chat)
            pass

        def _send(self, code: int, obj) -> None:
            b = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def _body(self):
            try:
                n = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                n = -1
            if n < 0 or n > MAX_BODY:
                return None
            try:
                v = json.loads(self.rfile.read(n) or b"{}")
            except ValueError:
                return None
            return v if isinstance(v, dict) else None

        def do_GET(self):
            if self.path == "/health":
                return self._send(200, {"ok": True, "service": "ai-coach"})
            if self.path == "/v1/coach/state":
                c = coach.ctx
                return self._send(200, {
                    "game": c.game and {"name": c.game.name, "serial": c.game.serial, "platform": c.game.platform},
                    "seats": c.seats, "bound_seats": list(c.bound),
                })
            self._send(404, {"error": "not found"})

        def do_POST(self):
            body = self._body()
            if body is None:
                return self._send(400, {"error": "bad body"})
            try:
                if self.path == "/v1/context":
                    seats = body.get("seats") if isinstance(body.get("seats"), list) else []
                    bound = body.get("bound_seats") if isinstance(body.get("bound_seats"), list) else []
                    coach.set_context(str(body.get("title") or ""), [s for s in seats if isinstance(s, dict)], bound)
                    return self._send(200, {"ok": True})
                if self.path == "/v1/coach/chat":
                    seat = body.get("seat")
                    if not isinstance(seat, int):
                        return self._send(400, {"error": "seat must be an int"})
                    return self._send(200, coach.ask(seat, str(body.get("message") or "")))
                if self.path == "/v1/coach/reset":
                    if isinstance(body.get("seat"), int):
                        coach.reset(body["seat"])
                    return self._send(200, {"ok": True})
            except SeatRejected as e:
                return self._send(403, {"error": str(e)})
            except Busy as e:
                return self._send(429, {"error": str(e)})
            except (urllib.error.URLError, OSError, TimeoutError):
                return self._send(502, {"error": "model backend unavailable (is the model pulled?)"})
            self._send(404, {"error": "not found"})

    return H


def main() -> None:
    bind = os.environ.get("COACH_BIND", "0.0.0.0:18790")
    host, port = bind.rsplit(":", 1)
    srv = ThreadingHTTPServer((host, int(port)), make_handler(make_coach()))
    print(f"ai-coach listening on {bind} model={OllamaClient().model}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
