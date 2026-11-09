"""Minimal Ollama /api/chat client (stdlib only, non-streaming)."""
from __future__ import annotations

import json
import os
import urllib.request

DEFAULT_MODEL = "qwen2.5:3b"  # ~2 GB, instruct-tuned, emits Ollama tool_calls reliably


class OllamaClient:
    def __init__(self, base_url: str | None = None, model: str | None = None, timeout: float = 90.0):
        self.base = (base_url or os.environ.get("OLLAMA_URL") or "http://ollama:11434").rstrip("/")
        self.model = model or os.environ.get("COUCHLINK_AI_COACH_MODEL") or DEFAULT_MODEL
        self.timeout = timeout

    def chat(self, messages: "list[dict]", tools: "list[dict] | None" = None) -> dict:
        body = {
            "model": self.model,
            "messages": messages,
            "stream": False,
            "keep_alive": os.environ.get("COUCHLINK_AI_COACH_KEEP_ALIVE", "5m"),
            # Small context + short answers = fast turns on a 3B model.
            "options": {"temperature": 0.3, "num_ctx": 4096, "num_predict": 400},
        }
        if tools:
            body["tools"] = tools
        req = urllib.request.Request(
            self.base + "/api/chat",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=self.timeout) as r:
            return json.loads(r.read(2_000_000)).get("message", {})
