# Wednesday 2026-08-26 — what’s next

**Branch:** `fix/hybrid-clvd-idr-photon` (PR #50 → PR #48 `feat/amazing-interactive-latency`)  
**HEAD:** `bd992be`  
**Written:** 2026-08-24 · **live update:** 2026-08-26  

**Mission Wednesday:** One clean live Chrome scrape that **beats Ricardo on all axes** and **clears beat-self** — especially **S_p50 ≤ 5** — without blackouts and without walking back hybrid RTP + thin CLVD.

### Live night (2026-08-26) — root causes, not band-aids

Scrape held greens (push~120, paint~116, shed 0%, 5 Mbps, wm live) but **Φ≈240 / S≈190** and one friend saw a **black stage**.

| Symptom | Root cause | Fix landed |
|---------|------------|------------|
| Φ≈240 / age_p95≈225 while fps green | CLVD SCTP `await` inside `join_all` HOL-blocked every peer | RTP-first + **CLVD budget** (P 6ms / IDR 48ms); densify on slack |
| Black stage | MSTC delivered ink-black frames; canvas stayed up | Runtime black-luma detect → `<video>` fallback |
| Opera UA skip | Band-aid | **Removed** in `bd992be` — fix the path |

Hard-refresh after redeploy; re-scrape for beat-self S≤5.

---

## Bars (must all pass)

| Axis | Ricardo floor | Beat-self |
|------|---------------|-----------|
| S_p50 (Φ−R) | ≤45 ms | **≤5 ms** |
| Push fps | ≥74 | **≥90** |
| Paint fps | ≥74 | **≥100** |
| Shed % | ≤3 | **≤1** |
| Encode kbps | ≥5000 | **≥5000** |
| Blackouts | 0 | **0** |

Authority: real Chrome `window.__couchlinkRicardo()` — not Playwright.

---

## Already landed (do not redo / do not undo)

| Commit | Why it matters |
|--------|----------------|
| `bd992be` | CLVD HOL-block budget + MSTC black→video (no UA hardcode) |
| `ee2295e` | RTP-first + densify CLVD + honest Φ clock (`perfSent`) |
| `bd21765` | CLVD IDR 256 KiB + hybrid bootstrap PLI (fixes RTP-green / 0 wm) |
| `3656bbc` | Bootstrap PLI no longer burns on throttle; hybrid PLI = 1 IDR |
| `3c4f229` | DC-open soft IDR + incomplete-IDR dual retry; `joel-prep.sh` |
| `430c40e` | `run.sh host --online` = full stack; `install.sh --run` forwards flags |
| `ef1b605` | No-regress leaveoff matrix |

**Locked architecture:**

- Visible paint = **full RTP forever**
- CLVD = **thin** sidecar for WC / `input_wm` / S_p50
- **No FEC** while RTP is live
- **Ignore RTCP PLI** in hybrid dual
- Governor: **hold ≥5 Mbps**, step **fps** (never bits)

---

## Wednesday runbook (in order)

### 1. Prep + start (one terminal)

```bash
cd ~/projects/couchlink
git checkout fix/hybrid-clvd-idr-photon
git pull
./scripts/joel-prep.sh
./scripts/run.sh host --online
```

Expect: signaling + TURN + host + cloudflared + win-capture; join URL `https://*.trycloudflare.com`.

Pinned env from prep: `720p60`, `CAPTURE_FPS=120`, Marvel window (or set `COUCHLINK_CAPTURE_WINDOW` / picker).

### 2. Client

1. Hard-refresh the **new** tunnel URL in Chrome (secure context).
2. Connect DualSense / pad — **wiggle ≥30s** (no pads ⇒ `input_wm` 0 ⇒ blank S forever).
3. Watch console for `VideoDecoder configured` (not sustained `produced no frames`).
4. Latency tab: watermark ring > 0, Φ / S_p50 numbers — not “waiting for CLVD input_wm”.

### 3. Scrape + score

```js
copy(JSON.stringify(window.__couchlinkRicardo()))
```

```bash
# paste clipboard → /tmp/ricardo.json
HOST_LOG=/tmp/couchlink-stack-*.log ./scripts/joel-live-gate.sh /tmp/ricardo.json
```

### 4. Pass / fail

**Ship claim only if all green.** If not:

| Symptom | First dig (do not “fix” by killing hybrid) |
|---------|-----------------------------------------------|
| RTP green, 0 wm / S blank | CLVD IDR complete? DC open IDR? bootstrap PLI? pad seqs? |
| Blacks on promote / play | FEC back on? RTCP PLI honored? IDR burst >1? |
| Encode &lt;5000 | Bitrate-step governor revived? wrong preset? |
| Paint &lt;100 | Capture fps / governor fps-step / canvas pump? |
| Shed &gt;1% | `COUCHLINK_RTP_FULL=1`? full dual CLVD? |

---

## Explicit do-nots Wednesday

1. Do **not** switch to RTP-only to stop blacks (kills S_p50).
2. Do **not** re-enable FEC while RTP is live.
3. Do **not** honor Chrome RTCP PLI in hybrid dual.
4. Do **not** reopen bitrate-step / floor &lt;5 Mbps.
5. Do **not** set `COUCHLINK_RTP_FULL=1` on WAN.
6. Do **not** drop IDR SCTP ceiling back to 24 KiB.

---

## If Wednesday scrape is green

1. Paste scrape summary into PR #50.
2. Merge #50 → #48 when Joel/live gate agrees.
3. Only then consider optional quality climb (7.5–10 Mbps / 1080) — Workstream F — **after** bars stay green.

## If Wednesday scrape is not green

Fix the failing axis only (see dig table). Re-run joel-live-gate. Do not open unrelated refactors.

---

## Quick refs

- Full matrix: `docs/superpowers/plans/2026-08-24-quality-bitrate-no-regress-audit.md`
- Prep: `./scripts/joel-prep.sh`
- Gate: `./scripts/joel-live-gate.sh`
- PR: https://github.com/jrb00013/couchlink/pull/50
