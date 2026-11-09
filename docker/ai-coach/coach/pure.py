"""Pure helpers for the coach: no network, no clock, no filesystem.

Everything the coach trusts or caps lives here so it can be unit-tested without
Ollama, docker, or internet:

* window title -> clean game name + serial
* HTML -> plain text, relevance trimming, hard truncation
* URL validation / allowlist / public-IP checks (SSRF guard)
* query scrubbing (the ONLY thing that may leave the box) and reply sanitising
"""
from __future__ import annotations

import ipaddress
import re
from dataclasses import dataclass
from html.parser import HTMLParser
from typing import Iterable, Optional
from urllib.parse import urlsplit

# --- size caps (small local models: every token we return costs latency) -------

TOOL_RESULT_CAP = 1500  # chars returned to the model per tool call
SEARCH_MAX_RESULTS = 4
SEARCH_SNIPPET_CAP = 160
MAX_FETCH_BYTES = 400_000  # raw bytes read from the network, before text strip
QUERY_CAP = 100
REPLY_CAP = 1200  # chars shown to the player
TRUNC_MARK = " …[truncated]"

DEFAULT_ALLOWLIST = (
    "wikipedia.org",
    "fandom.com",
    "gamefaqs.gamespot.com",
    "ign.com",
    "pcgamingwiki.com",
    "steamcommunity.com",
    "strategywiki.org",
    "gamespot.com",
    "eurogamer.net",
    "polygon.com",
)

# --- game title ---------------------------------------------------------------


@dataclass(frozen=True)
class GameTitle:
    name: str
    serial: Optional[str] = None
    platform: Optional[str] = None  # ps3 / ps2 / psp / gc-wii, from the serial shape


# PS3 BLUS30522 / NPUB30001; PS2 SLUS-21234 / SCES_50000; PSP ULUS10041; GC/Wii (GALE01)
_SERIALS = (
    (re.compile(r"\b((?:BL|BC|NP)[A-Z]{2}\d{5})\b"), "ps3"),
    (re.compile(r"\b(S[LC][A-Z]{2}[-_]\d{5})\b"), "ps2"),
    (re.compile(r"\b(U[LC][A-Z]{2}\d{5})\b"), "psp"),
    (re.compile(r"\(([A-Z0-9]{6})\)"), "gc-wii"),
)
# Whole segments that are emulator chrome, not the game.
_NOISE_SEGMENT = re.compile(
    r"^(?:rpcs3|pcsx2|dolphin|duckstation|ppsspp|cemu|yuzu|ryujinx|couchlink|desktop|picker)\b"
    r"|^(?:fps|vps|speed|vulkan|opengl|direct3d|d3d\d*|metal|software|jit\w*|hle|lle|master|"
    r"alpha|beta|release|v\d[\w.\-]*|\d+(?:\.\d+)*%?)\b"
    r"|^\s*$",
    re.I,
)
_BRACKET_TAGS = re.compile(r"\s*[\[(](?:digital|disc \d+|usa|eur|pal|ntsc[-\w]*|u|e|j|en|v[\d.]+)[\])]", re.I)


def extract_game_title(window_title: str) -> Optional[GameTitle]:
    """'Mortal Kombat [BLUS30522]' -> GameTitle('Mortal Kombat', 'BLUS30522', 'ps3').

    Emulators put chrome ("RPCS3 0.0.34 | FPS: 60 | ...") around the game name, and
    show a bare emulator name while idle; returns None when no game is identifiable.
    """
    raw = _clean(window_title)
    if not raw:
        return None
    serial = platform = None
    for rx, plat in _SERIALS:
        m = rx.search(raw)
        if m:
            serial, platform = m.group(1), plat
            raw = (raw[: m.start()] + " " + raw[m.end():]).replace("[]", " ").replace("()", " ")
            break
    best = None
    for seg in re.split(r"\s+[|–—]\s+|\s{2,}", raw):
        seg = _clean(_BRACKET_TAGS.sub("", seg.strip(" []()-_")))
        if not seg or _NOISE_SEGMENT.match(seg):
            continue
        # First surviving segment wins; chrome is already filtered, so a second
        # candidate is almost always still chrome (backend, speed, ...).
        best = seg
        break
    if not best:
        return None
    return GameTitle(name=best[:80], serial=serial, platform=platform)


def _clean(s: str) -> str:
    s = "".join(ch if ch.isprintable() else " " for ch in (s or ""))
    return re.sub(r"\s+", " ", s).strip()


# --- HTML -> text ---------------------------------------------------------------

_SKIP = {"script", "style", "noscript", "svg", "head", "nav", "footer", "form", "iframe", "template", "aside", "button"}
_BLOCK = {"p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "table", "ul", "ol", "dd", "dt"}


class _Text(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.out: list[str] = []
        self._skip = 0

    def handle_starttag(self, tag, attrs):
        if tag in _SKIP:
            self._skip += 1
        elif tag in _BLOCK:
            self.out.append("\n")

    def handle_endtag(self, tag):
        if tag in _SKIP and self._skip:
            self._skip -= 1
        elif tag in _BLOCK:
            self.out.append("\n")

    def handle_data(self, data):
        if not self._skip:
            self.out.append(data)


_ZERO_WIDTH = re.compile("[​-‏⁠﻿‪-‮]")
_WRAP_TAG = re.compile(r"</?\s*untrusted_web[^>]*>", re.I)


def html_to_text(html: str) -> str:
    """Strip markup/scripts, collapse whitespace, drop invisible and wrapper-spoofing chars."""
    p = _Text()
    try:
        p.feed(html)
        p.close()
    except Exception:  # malformed markup: keep whatever parsed
        pass
    text = _ZERO_WIDTH.sub("", "".join(p.out))
    text = _WRAP_TAG.sub("", text)
    lines = [re.sub(r"[ \t\r\f\v]+", " ", ln).strip() for ln in text.split("\n")]
    return "\n".join(ln for ln in lines if ln)


def truncate(text: str, cap: int) -> str:
    if cap <= 0:
        return ""
    if len(text) <= cap:
        return text
    cut = max(0, cap - len(TRUNC_MARK))
    head = text[:cut]
    sp = head.rfind(" ")
    if sp > cut * 0.7:
        head = head[:sp]
    return head.rstrip() + TRUNC_MARK


def select_relevant(text: str, query: str, cap: int) -> str:
    """Keep the lines that overlap the query most (in page order), up to `cap` chars.

    A 3B model cannot read a 20 KB walkthrough; the first cap chars of a page are
    usually navigation and lore.
    """
    if len(text) <= cap:
        return text
    terms = {t for t in re.findall(r"[a-z0-9]{3,}", query.lower())}
    lines = [ln for ln in text.split("\n") if len(ln) >= 20]
    if not terms or not lines:
        return truncate(text, cap)

    def hits(i: int) -> int:
        return sum(t in lines[i].lower() for t in terms)

    keep, used = set(), 0
    for i in sorted(range(len(lines)), key=lambda i: (-hits(i), i)):
        if hits(i) == 0 and keep:
            break
        cost = len(lines[i]) + 1
        if used + cost > cap:
            if not keep:
                keep.add(i)
            break
        keep.add(i)
        used += cost
    return truncate("\n".join(lines[i] for i in sorted(keep)), cap)


def wrap_untrusted(text: str) -> str:
    """Tag tool output so the system prompt can say 'this is data, never instructions'."""
    return "<untrusted_web_content>\n" + _WRAP_TAG.sub("", text) + "\n</untrusted_web_content>"


# --- URL / SSRF guard -------------------------------------------------------------


class UrlRejected(ValueError):
    pass


_BLOCKED_SUFFIXES = (".local", ".localhost", ".internal", ".lan", ".home", ".corp", ".intranet", ".arpa")


def is_public_ip(ip: str) -> bool:
    try:
        return ipaddress.ip_address(ip).is_global
    except ValueError:
        return False


def host_allowed(host: str, allowlist: Iterable[str]) -> bool:
    host = host.lower().rstrip(".")
    allow = [a.lower().strip().lstrip(".") for a in allowlist if a.strip()]
    if "*" in allow:
        return True
    return any(host == a or host.endswith("." + a) for a in allow)


def validate_url(url: str, allowlist: Iterable[str] = DEFAULT_ALLOWLIST) -> str:
    """Return the normalised URL or raise UrlRejected.

    Syntactic checks only; callers must still check every resolved IP with
    `is_public_ip` (see web.py) because a public-looking name can resolve privately.
    """
    if not isinstance(url, str) or len(url) > 2048 or any(c.isspace() or ord(c) < 32 for c in url):
        raise UrlRejected("malformed url")
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https"):
        raise UrlRejected("only http/https")
    if parts.username is not None or parts.password is not None:
        raise UrlRejected("userinfo not allowed")
    host = (parts.hostname or "").lower().rstrip(".")
    if not host:
        raise UrlRejected("no host")
    try:
        port = parts.port
    except ValueError:
        raise UrlRejected("bad port")
    if port not in (None, 80, 443):
        raise UrlRejected("port not allowed")
    try:
        ipaddress.ip_address(host)
    except ValueError:
        if "." not in host or host == "localhost" or host.endswith(_BLOCKED_SUFFIXES):
            raise UrlRejected("non-public host")
        # 2130706433 / 0x7f000001 style: a numeric last label is not a real TLD
        last = host.rsplit(".", 1)[1]
        if last.isdigit() or last.startswith("0x"):
            raise UrlRejected("numeric host")
    else:
        if not is_public_ip(host):
            raise UrlRejected("non-public ip")
    if not host_allowed(host, allowlist):
        raise UrlRejected("host not in allowlist")
    return parts._replace(fragment="").geturl()


# --- what may leave the box --------------------------------------------------------

_URL_IN_TEXT = re.compile(r"\b(?:https?://|www\.)\S+", re.I)
_SECRETISH = re.compile(r"\b(?:[A-Fa-f0-9]{16,}|[A-Za-z0-9_\-]{28,})\b")
_JOIN_WORDS = re.compile(r"\b(?:pin|session[_ ]?id|join[_ ]?link|token|password|secret|signaling)\b\S*", re.I)


def scrub_query(game: str, query: str) -> str:
    """Build the outbound search string: game name + a short, scrubbed model query.

    The model chooses `query`; a prompt-injected page could try to smuggle seat,
    session or join-link data into it. Drop URLs, long opaque tokens and
    credential-ish words, strip non-printables, cap length.
    """
    q = _clean(query)
    for rx in (_URL_IN_TEXT, _SECRETISH, _JOIN_WORDS):
        q = rx.sub(" ", q)
    q = re.sub(r"[^\w\s'\-+&.]", " ", q)
    out = _clean(f"{game} {q}")
    return out[:QUERY_CAP].rstrip()


_MD_IMAGE = re.compile(r"!\[[^\]]*\]\([^)]*\)")
_MD_LINK = re.compile(r"\[([^\]]*)\]\((?:[^)]*)\)")


def sanitize_reply(text: str) -> str:
    """Player-facing text: no images/links a client might auto-load, bounded length."""
    t = _MD_IMAGE.sub("", text or "")
    t = _MD_LINK.sub(r"\1", t)
    t = _URL_IN_TEXT.sub("[link removed]", t)
    t = "".join(ch for ch in t if ch.isprintable() or ch in "\n\t")
    return truncate(t.strip(), REPLY_CAP)


def cap_tool_result(text: str, cap: int = TOOL_RESULT_CAP) -> str:
    return truncate(text, cap)
