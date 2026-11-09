# AI game coach (opt-in)

A host-enabled local Ollama container that teaches one player how to play the
game currently on screen. It is bound to a controller seat, knows the game from
the emulator window title, and can look up instructions on the web through two
tiny, locked-down tools. **Off by default. When off, nothing runs and nothing
touches the network.**

Status: phase 1 (host bridge, coach service, docker stack, CLI). The in-browser
chat panel is phase 2 (see "Rollout"). Nothing here has been run against a real
model yet; see "What is verified".

## Architecture

```
 emulator window ──title──┐                              (host machine, all loopback)
 (RPCS3/PCSX2/Dolphin)    │
                          ▼
  ┌────────────────── couchlink-host (existing process) ──────────────────┐
  │  capture ► encode ► WebRTC     pad ► virtual pad     <- HOT PATHS,    │
  │  (untouched)                                            untouched     │
  │                                                                       │
  │  ai_coach.rs  (only if COUCHLINK_AI_COACH=1)                          │
  │    one tokio task, tick 10 s, slots.try_lock() snapshot               │
  │    POST /v1/context {title, seats[{slot,pad_kind}], bound_seats}      │
  └───────────────────────────────┬───────────────────────────────────────┘
                                  │ 127.0.0.1:18790 only
        ┌─────────────────────────▼──────────────── docker ───────────────┐
        │  coach (python, stdlib only)         net: inference + egress     │
        │   title -> game+serial   per-seat history   tool loop (<=3)      │
        │   tools: search_howto(game, query)  fetch_page(result #/url)     │
        │              │ chat (HTTP)                       │ HTTPS         │
        │  ┌───────────▼─────────┐                         ▼               │
        │  │ ollama (small model)│ net: inference     allowlisted public   │
        │  │ no egress, no ports │ (internal)         sites only           │
        │  └─────────────────────┘                                         │
        └──────────────────────────────────────────────────────────────────┘
  player question ──► POST /v1/coach/chat {seat, message}   (phase 1: CLI/curl;
                                                              phase 2: web panel via host)
```

## Data flow

1. Host (flag on) reads the game title: `COUCHLINK_AI_COACH_TITLE`, else the first
   line of `COUCHLINK_AI_COACH_TITLE_FILE`, else the pinned capture window
   (`COUCHLINK_CAPTURE_WINDOW`). It reads seats from the existing `slots` map
   (slot number and the pad kind each player already reported).
2. Every 10 s, and at least once a minute, it posts that to the coach. Unchanged
   context is not re-sent between heartbeats.
3. Coach normalises the title (below), clears per-seat history when the game
   changes, and holds the context in memory only.
4. A chat request names a seat. The coach checks the seat is **bound** and
   **occupied**, builds a short system prompt (game, serial, the seat's button
   dialect), and runs a bounded tool loop against Ollama.
5. The reply is sanitised (no links/images, max 1200 chars) and returned.

## Seat binding

* Seat numbers match the host: 0 = host's own pad, 1-3 = remote players.
* `COUCHLINK_AI_COACH_SEAT` (default `1`, comma list allowed) says which seats may
  use the coach. Any other seat gets 403. An unoccupied seat gets 403.
* The seat's pad kind picks the vocabulary: DualSense says Cross/Circle, Xbox says A/B.
* History is per seat, 6 turns, in memory, wiped on game change. Tool output is
  never stored in history.
* Phase 1 does not authenticate *which human* calls the endpoint; it trusts the
  loopback boundary. Phase 2 must derive the seat from the player's authenticated
  signaling connection on the host, never from a client-supplied number.

## Game title detection

`coach/pure.py::extract_game_title` strips emulator chrome and extracts a serial:

| Window title | name | serial | platform |
|---|---|---|---|
| `Mortal Kombat [BLUS30522]` | Mortal Kombat | BLUS30522 | ps3 |
| `RPCS3 0.0.34 Alpha \| FPS: 59.9 \| Call of Duty: Black Ops II [BLUS31011]` | Call of Duty: Black Ops II | BLUS31011 | ps3 |
| `Marvel - Ultimate Alliance \| SLUS-21234 \| 60 FPS` | Marvel - Ultimate Alliance | SLUS-21234 | ps2 |
| `Dolphin 5.0 \| JIT64 \| OpenGL \| Super Smash Bros. Melee (GALE01)` | Super Smash Bros. Melee | GALE01 | gc-wii |
| `RPCS3`, `PCSX2`, `desktop`, empty | none (coach says it cannot see a game) | | |

Limit: the host cannot read a Windows window title from WSL today (the
`couchlink-win-capture` process owns that), so phase 1 uses the env/file sources
above. Phase 2: win-capture writes the matched window title to a file or reports
it over the existing capture socket.

## Tools (few, tiny, capped)

| Tool | Input | Output cap |
|---|---|---|
| `search_howto(game, query)` | `game` is advisory; the **detected** game is always used. `query` is scrubbed. | up to 4 results, 80-char title + 160-char snippet each |
| `fetch_page(url)` | a result number (1-4) or a URL **returned by the last search** | text trimmed to the lines best matching the query, 1500 chars |

Per turn: at most 3 tool calls, then the model is called once more without tools to
force an answer. Model options: `num_ctx 4096`, `num_predict 400`. Search results are
cached 10 min (32 entries).

Default model: `qwen2.5:3b` (about 2 GB, instruct, tool-calling). Override with
`COUCHLINK_AI_COACH_MODEL`; any Ollama model that supports tools works
(`llama3.2:3b`, `qwen3:4b`). A model without tool support still answers, just
from its own knowledge.

## Threat model

Assets: controller/session/join-link data, host environment, LAN services.
Attackers: (a) a malicious or compromised web page (prompt injection), (b) a
player trying to make the host fetch internal URLs, (c) a LAN peer reaching the
coach port.

| Threat | Mitigation |
|---|---|
| Prompt injection in fetched pages | Output wrapped in `<untrusted_web_content>` (wrapper tags stripped from content so it cannot be closed early); system prompt says never follow it; tools have no side effects beyond a capped read; tool output never enters history; reply sanitised. |
| Exfiltration via search query | Model-written query is scrubbed: URLs, 16+ hex or 28+ token-like strings, and `pin/session/join/token/password/secret/signaling` words are removed; 100-char cap. The coach never receives PIN, session id, join link or signaling URL in the first place (the context endpoint takes only title + seats). |
| Exfiltration via fetch URL | `fetch_page` only accepts results of the previous search; URL must also pass the allowlist. No request carries cookies or headers beyond a fixed User-Agent. |
| SSRF | http/https only, ports 80/443, no userinfo, host allowlist (default: Wikipedia, Fandom, GameFAQs, IGN, PCGamingWiki, Steam community, StrategyWiki, GameSpot, Eurogamer, Polygon), numeric/`.local`/`.internal` names rejected, **every resolved IP must be public**, connection pinned to the checked IP (no DNS rebinding window), redirects followed manually (max 3) and re-validated each hop. `COUCHLINK_AI_COACH_ALLOW=*` widens the allowlist but private/loopback targets stay blocked. |
| Resource exhaustion | 400 KB read cap, 6 s socket / 10 s total deadline, 4 KB request bodies, one model call at a time (429 when busy), per-seat 1.5 s rate limit, container mem/pids limits. |
| Markdown/link smuggling to the player's client | Reply stripped of images, links and URLs. |
| Arbitrary code execution | None: no shell/file/eval tools; python stdlib only, no `pickle`/`eval`; container is read-only, non-root, `cap_drop: ALL`. |
| LAN access to the coach | Published on `127.0.0.1` only; Ollama publishes nothing. Host bridge refuses non-loopback coach addresses. |
| Ollama egress | Ollama sits on an `internal` network. Only the one-shot `ollama-pull` and the coach have egress. |

Residual risks: a poisoned allowlisted page can still mislead the *advice*
(wrong inputs); a 3B model can be steered into odd answers but not into actions.
Search goes to DuckDuckGo's HTML endpoint, so the game title and query are visible
to it. Player chat text is sent to the local model only, never to the web.

## Config

| Var | Default | Meaning |
|---|---|---|
| `COUCHLINK_AI_COACH` | off | Host: enable the bridge task (`1/true/yes/on`). |
| `COUCHLINK_AI_COACH_SEAT` | `1` | Host: bound seat(s), comma list, 0-3. |
| `COUCHLINK_AI_COACH_ADDR` | `127.0.0.1:18790` | Host: coach address; must be loopback. |
| `COUCHLINK_AI_COACH_TITLE` | | Host: static title override. |
| `COUCHLINK_AI_COACH_TITLE_FILE` | | Host: file whose first line is the window title. |
| `COUCHLINK_AI_COACH_MODEL` | `qwen2.5:3b` | Ollama model tag. |
| `COUCHLINK_AI_COACH_WEB` | `1` | `0` = offline coach, no tools, no egress use. |
| `COUCHLINK_AI_COACH_ALLOW` | built-in list | Comma list of host suffixes, or `*`. |
| `COUCHLINK_AI_COACH_KEEP_ALIVE` | `5m` | How long Ollama keeps the model in memory. |
| `COUCHLINK_AI_COACH_PORT` | `18790` | Published loopback port. |
| `COUCHLINK_AI_COACH_MEM` / `_CPUS` | `6g` / `4` | Ollama container limits. |

Run it: `scripts/ai-coach.sh up [--gpu]`, `pull-model` once, then start the host with
`COUCHLINK_AI_COACH=1`. Try it: `scripts/ai-coach.sh chat 1 "how do I block?"`.

## Latency impact

None when disabled: `spawn` returns before creating any task, timer or socket.
When enabled the host adds one task that wakes every 10 s, takes a `try_lock` on
the slots map (skipped if contended, held for microseconds when not), and makes a
loopback POST with a 2 s timeout. No work per frame or per input packet, no new
await in `push_bounded`, capture, encode, or pad paths. The model runs in another
container; on a shared GPU, inference competes with the emulator, which is why GPU
use is opt-in (`--gpu`) and the model is small and idles out after 5 minutes.

## What is verified

Unit tests (no Ollama, no docker, no internet): title extraction, HTML-to-text,
truncation, URL/SSRF validation, query scrubbing, reply sanitising, the tool loop
with a scripted fake model and fake web, seat binding, rate limit, the HTTP API,
and the host bridge's gating, loopback restriction, JSON shape and POST.
`docker compose config` validates both compose files. **Not run:** the compose
stack, a real Ollama model, real DuckDuckGo/page fetches, the pinned TLS connect,
GPU passthrough.

## Rollout and limits

1. Phase 1 (this change): host bridge + coach + stack + CLI. Dev/test via curl.
2. Phase 2: window title from win-capture; host relays player chat over the
   existing signaling data channel (seat from the authenticated connection);
   web panel (toggle + chat) shown only when the host advertises coach support.
3. Phase 3: per-game cached tips, optional streaming replies, voice.

Limits: one model call at a time; 3B-model quality (it will sometimes be wrong -
the prompt tells it to say so); advice is text only, never injects input.

## Why Python and not a Rust crate

The coach is I/O glue (HTTP in, HTTP to Ollama, HTTPS fetch). A Rust crate would
add an HTTP+TLS client stack (reqwest/hyper/rustls) to the workspace lockfile and
to every `cargo build`, slow CI, and pull a web client into a workspace whose
host binary should stay free of one. Stdlib-only Python adds zero dependencies, a
~50 MB slim image with no build step, and keeps the whole feature deletable as one
directory. The host side that touches the hot-path neighbourhood is Rust and
dependency-free.

## Open questions

* Which window-title source for WSL hosts: file written by win-capture, or a new
  field on the capture socket?
* Should the host or the coach own seat authorization once chat arrives over
  signaling (recommended: host, coach stays dumb)?
* Search backend: DuckDuckGo HTML scraping is fragile; self-hosted SearXNG, or
  Wikipedia/Fandom API search only, would be sturdier but narrower.
* Default allowlist: add Reddit/YouTube transcripts? Both are noisy for a 3B model.
* GPU contention with RPCS3: do we cap the coach to CPU while a game runs?
* Should the coach be available to the host seat (0) by default?
