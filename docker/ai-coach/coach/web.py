"""The only code in the coach that touches the internet.

Two tools for a small model: `search_howto` and `fetch_page`. Both go through
`SafeWeb`, which enforces: http/https only, allowlisted hosts, every resolved IP
public (connection is pinned to the checked IP, so DNS cannot rebind between
check and connect), manual redirects re-validated, byte and wall-clock limits,
no cookies, no request headers beyond a fixed UA. Nothing from the host, the
session or the seat is ever put in a request; callers pass only a scrubbed query
or a URL that came out of a previous search.
"""
from __future__ import annotations

import http.client
import socket
import ssl
import time
from collections import OrderedDict
from html.parser import HTMLParser
from typing import Callable, Iterable, Optional
from urllib.parse import parse_qs, quote_plus, urljoin, urlsplit

from . import pure

UA = "couchlink-ai-coach/0.1 (+local; game-help lookup)"
SEARCH_URL = "https://html.duckduckgo.com/html/?q="
SEARCH_HOST = "html.duckduckgo.com"
MAX_REDIRECTS = 3
DEFAULT_TIMEOUT = 6.0  # per socket op
DEFAULT_DEADLINE = 10.0  # whole request

# (status, headers (lowercased keys), body bytes <= MAX_FETCH_BYTES)
HttpGet = Callable[[str, "list[str]"], "tuple[int, dict, bytes]"]


class FetchError(Exception):
    pass


def resolve_public(host: str) -> "list[str]":
    """Resolve and require *every* address to be public; return them."""
    try:
        infos = socket.getaddrinfo(host, None, type=socket.SOCK_STREAM)
    except OSError as e:
        raise FetchError(f"dns failed: {e}") from e
    ips = sorted({i[4][0] for i in infos})
    if not ips or not all(pure.is_public_ip(ip) for ip in ips):
        raise FetchError("host resolves to a non-public address")
    return ips


class _PinnedHTTPConnection(http.client.HTTPConnection):
    def __init__(self, host, ip, timeout):
        super().__init__(host, timeout=timeout)
        self._ip = ip

    def connect(self):
        self.sock = socket.create_connection((self._ip, self.port), self.timeout)


class _PinnedHTTPSConnection(http.client.HTTPSConnection):
    def __init__(self, host, ip, timeout):
        super().__init__(host, timeout=timeout, context=ssl.create_default_context())
        self._ip = ip

    def connect(self):
        raw = socket.create_connection((self._ip, self.port), self.timeout)
        self.sock = self._context.wrap_socket(raw, server_hostname=self.host)  # SNI + cert check on the *name*


def pinned_get(url: str, ips: "list[str]", timeout: float = DEFAULT_TIMEOUT) -> "tuple[int, dict, bytes]":
    u = urlsplit(url)
    cls = _PinnedHTTPSConnection if u.scheme == "https" else _PinnedHTTPConnection
    conn = cls(u.hostname, ips[0], timeout)
    if u.port:
        conn.port = u.port
    try:
        path = (u.path or "/") + (("?" + u.query) if u.query else "")
        conn.request("GET", path, headers={"User-Agent": UA, "Accept": "text/html,text/plain;q=0.8", "Accept-Encoding": "identity"})
        r = conn.getresponse()
        headers = {k.lower(): v for k, v in r.getheaders()}
        body = r.read(pure.MAX_FETCH_BYTES + 1)[: pure.MAX_FETCH_BYTES]
        return r.status, headers, body
    except (OSError, http.client.HTTPException) as e:
        raise FetchError(f"request failed: {type(e).__name__}") from e
    finally:
        conn.close()


class SafeWeb:
    def __init__(
        self,
        allowlist: Iterable[str] = pure.DEFAULT_ALLOWLIST,
        resolve: Callable[[str], "list[str]"] = resolve_public,
        http_get: Callable[..., "tuple[int, dict, bytes]"] = pinned_get,
        clock: Callable[[], float] = time.monotonic,
        cache_size: int = 32,
        cache_ttl: float = 600.0,
    ):
        self.allowlist = tuple(allowlist)
        self._resolve, self._get, self._clock = resolve, http_get, clock
        self._cache: "OrderedDict[str, tuple[float, str]]" = OrderedDict()
        self._cache_size, self._cache_ttl = cache_size, cache_ttl

    def get_text(self, url: str, *, search: bool = False) -> str:
        """GET -> plain text. `search=True` lets the fixed search host through the allowlist."""
        allow = self.allowlist + ((SEARCH_HOST,) if search else ())
        deadline = self._clock() + DEFAULT_DEADLINE
        cur = url
        for _ in range(MAX_REDIRECTS + 1):
            try:
                cur = pure.validate_url(cur, allow)
            except pure.UrlRejected as e:
                raise FetchError(f"url rejected: {e}") from e
            if self._clock() > deadline:
                raise FetchError("deadline exceeded")
            ips = self._resolve(urlsplit(cur).hostname or "")
            status, headers, body = self._get(cur, ips)
            if status in (301, 302, 303, 307, 308) and headers.get("location"):
                cur = urljoin(cur, headers["location"])
                continue
            if status != 200:
                raise FetchError(f"http {status}")
            ctype = headers.get("content-type", "text/html").split(";")[0].strip().lower()
            if ctype not in ("text/html", "text/plain", "application/xhtml+xml"):
                raise FetchError(f"unsupported content-type {ctype}")
            charset = "utf-8"
            if "charset=" in headers.get("content-type", ""):
                charset = headers["content-type"].split("charset=")[1].split(";")[0].strip() or "utf-8"
            try:
                return body.decode(charset, "replace")
            except LookupError:
                return body.decode("utf-8", "replace")
        raise FetchError("too many redirects")

    # -- tools -----------------------------------------------------------------

    def search(self, game: str, query: str) -> "list[dict]":
        q = pure.scrub_query(game, query)
        if not q:
            raise FetchError("empty query")
        hit = self._cache.get(q)
        if hit and self._clock() - hit[0] < self._cache_ttl:
            self._cache.move_to_end(q)
            return _parse_ddg(hit[1])  # re-parse: cheap, keeps cache a plain string
        html = self.get_text(SEARCH_URL + quote_plus(q), search=True)
        self._cache[q] = (self._clock(), html)
        while len(self._cache) > self._cache_size:
            self._cache.popitem(last=False)
        return _parse_ddg(html)

    def fetch(self, url: str, query: str = "") -> str:
        html = self.get_text(url)
        text = pure.html_to_text(html)
        return pure.select_relevant(text, query, pure.TOOL_RESULT_CAP)


class _DDG(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.results: "list[dict]" = []
        self._mode: Optional[str] = None

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        cls = a.get("class") or ""
        if tag == "a" and "result__a" in cls:
            self.results.append({"title": "", "url": _unwrap_ddg(a.get("href") or ""), "snippet": ""})
            self._mode = "title"
        elif tag == "a" and "result__snippet" in cls and self.results:
            self._mode = "snippet"

    def handle_endtag(self, tag):
        if tag == "a":
            self._mode = None

    def handle_data(self, data):
        if self._mode and self.results:
            self.results[-1][self._mode] += data


def _unwrap_ddg(href: str) -> str:
    if href.startswith("//"):
        href = "https:" + href
    q = parse_qs(urlsplit(href).query)
    return q["uddg"][0] if "uddg" in q else href


def _parse_ddg(html: str) -> "list[dict]":
    p = _DDG()
    try:
        p.feed(html)
    except Exception:
        pass
    return [
        {k: " ".join(v.split()) for k, v in r.items()}
        for r in p.results
        if r["url"] and r["title"].strip()
    ]
