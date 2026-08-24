# Leaveoff: no-regress gates + beat Ricardo / beat-self

**Branch:** `fix/hybrid-clvd-idr-photon` @ `430c40e` (PR #50 → `feat/amazing-interactive-latency` / PR #48)  
**Date:** 2026-08-24  
**Goal next session:** Prove live that we **beat Ricardo floors** and **beat-self bars** on every axis — **input responsiveness**, **framerate**, **bitrate/quality**, and **no blackouts** — without walking back hybrid RTP + thin CLVD.

---

## North-star metrics (must not regress)

Authority for S_p50 = **real Chrome** scrape via `window.__couchlinkRicardo()`, not Playwright.

| Axis | Ricardo floor (must clear) | Beat-self bar (wow) | What it protects |
|------|----------------------------|---------------------|------------------|
| **Input** S_p50 (Φ−R) | ≤ **45** ms | ≤ **5** ms | Amazing pad→photon responsiveness |
| **Push** fps | ≥ **74** (soft live ≥50) | ≥ **90** | Host delivery cadence |
| **Paint** fps | ≥ **74** | ≥ **100** | Visible feel / smoothness |
| **Shed** % | ≤ **3** (soft ≤8) | ≤ **1** | Link health (no mush) |
| **Encode** kbps | ≥ **5000** | ≥ **5000** | Picture quality / definition |
| **Resolution** | 1280×720 | 1280×720 | Playable shape (not 1080@18M on WAN) |
| **Blackouts** | **0** freezes / no cutouts | **0** | Shared-encoder IDR storms |

Frozen Ricardo A drawer: `5.00 Mbps @ 60 · 1280×720 · push 0.1ms · paint 74 · shed 0% · RTT ~48`.

Sacred units (host):

- `b_bitrate_hold_never_drops_below_playable_5mbps`
- `b_healthy_hybrid_keeps_full_rtp_for_paint`
- `b_live_sim_target_beats_ricardo_on_all_axes`

---

## Architecture that must stay (do not “simplify” away)

**Hybrid present path (locked):**

- **Visible paint = full-rate RTP forever** (v25 feel)
- **CLVD = thin sidecar** (IDR + every 2nd AU) for WebCodecs + `input_wm` / S_p50
- **Never exclusive PATH_WEBCODECS** that kills RTP (v26 → 1 fps / black)
- **No FEC while RTP is live** (promote FEC tax → Chrome RTCP PLI → shared IDR black)
- **Ignore RTCP PLI in hybrid dual**; one **bootstrap DC PLI** until first WC paint (≥3s coalesce)
- **P-frame SCTP cap 24 KiB** (wow bar); **IDR ceiling 256 KiB** so multi-fragment keyframes complete (v33→v34: 24 KiB aborted mid-IDR → 0 wm samples)

**Governor (locked):**

- Hold **≥5 Mbps** — step **fps**, never bits
- Frame shed = shed only when **no** peer delivered
- Do **not** revive bitrate ladder / 1250 floor

**Ops (locked):**

- Friend-night: `COUCHLINK_PRESET=720p60`, `COUCHLINK_CAPTURE_FPS=120`, Marvel/window as needed
- One command: `./scripts/run.sh host --online` (full stack: signaling + TURN + host + cloudflared + win-capture)
- `--mesh-first` only if deliberately testing mesh; default online = cloudflare path

---

## Failure modes already proven (must stay fixed)

| Symptom | Root cause | Fix that must remain |
|---------|------------|----------------------|
| Green RTP, **0 wm / S blank**, `produced no frames` | 24 KiB SCTP aborted mid-IDR; photon sidecar blocked all PLI | IDR 256 KiB + one bootstrap DC PLI |
| Periodic **black cutouts** after promote | FEC on CLVD + RTCP PLI → shared encoder IDR | No FEC while RTP; ignore RTCP PLI in dual |
| Paint ~1 fps after “fallback” | Exclusive CLVD / IDR-only RTP | Full RTP always when hybrid |
| Shed 20–67% | Full-rate dual CLVD+RTP | Thin CLVD only |
| Encode 1250, mushy picture | Bitrate-step governor | Bitrate-hold + fps-step |
| Stuck black until refresh | LowLatencyCanvas pump died | Pump self-restart / re-attach |

---

## Checklist for next agent / next night

### A. Offline (no live friend)

```bash
cargo test -p couchlink-host ricardo_playable_ab amazing_latency_ab
cargo test -p couchlink-host link_gov wan3_math
cd web && npx vitest run
bash -n scripts/run.sh scripts/ensure-host-stack.sh install.sh
```

- [ ] All sacred A/B tests green
- [ ] Hybrid path_flags = `(true, true)` for UNKNOWN / WARMUP / WEBCODECS

### B. Bring-up (one command)

```bash
export COUCHLINK_PRESET=720p60
export COUCHLINK_CAPTURE_FPS=120
export COUCHLINK_CAPTURE_SOURCE=window
export COUCHLINK_CAPTURE_WINDOW='Marvel - Ultimate Alliance'   # or picker
./scripts/run.sh host --online
# optional: --verbose
```

- [ ] Join URL prints `https://*.trycloudflare.com`
- [ ] Processes up: signaling, turnserver, couchlink-host, cloudflared, win-capture
- [ ] Host log: streaming ≥90 fps soon after join, shed 0%, target **5.00 Mbps**

### C. Live Chrome scrape (Joel / local)

1. Hard-refresh tunnel URL in Chrome  
2. Pad wiggling ≥30s  
3. Console: `copy(JSON.stringify(window.__couchlinkRicardo()))`  
4. Score:

```bash
HOST_LOG=/tmp/couchlink-stack-vNN.log ./scripts/joel-live-gate.sh /tmp/ricardo.json
# or
BEAT_SELF=1 HOST_LOG=… CLIENT_SCRAPE=/tmp/ricardo.json node scripts/regression-latency-live.mjs
```

### D. Pass / fail matrix (all must pass to claim “done”)

| Case | Pass condition | Fail = regress |
|------|----------------|----------------|
| **Input responsiveness** | `watermarkActive: true`, samples ≥16, S_p50 ≤5 (beat-self) or ≤45 (Ricardo) | “waiting for CLVD input_wm”, wm ring 0 |
| **Framerate push** | push ≥90 (beat-self) / ≥74 Ricardo | push collapses to IDR rate ~1 |
| **Framerate paint** | paint ≥100 / ≥74 | canvas pump dead / 1 fps |
| **Quality bitrate** | encode **≥5000** entire scrape | HUD shows 1250 / governor cut bits |
| **Definition** | 1280×720 (or agreed climb only after gates green) | accidental 1080@18M WAN thrash |
| **No blackouts** | no black flashes on promote / long play; freeze 0 | PLI/IDR storm after promote |
| **Hybrid intact** | RTP canvas visible + WC photon path; presentMode eventually has wm | exclusive binary / RTP killed |
| **Console health** | `VideoDecoder configured`; no sustained `produced no frames` | WC never configures |

### E. Explicit do-nots next session

1. Do **not** switch to RTP-only to “fix” blacks (kills S_p50).  
2. Do **not** re-enable FEC while RTP is live.  
3. Do **not** honor Chrome RTCP PLI in hybrid dual.  
4. Do **not** reopen bitrate-step governor / floor &lt;5 Mbps.  
5. Do **not** set `COUCHLINK_RTP_FULL=1` on WAN.  
6. Do **not** drop IDR SCTP ceiling back to 24 KiB.

---

## What’s already landed on this PR branch

| Commit | What |
|--------|------|
| `bd21765` | CLVD IDR delivery under hybrid (256 KiB IDR + bootstrap PLI) |
| `430c40e` | `run.sh host --online` = full stack + `ensure-host-stack.sh`; `install.sh --run` forwards flags |

**Still open for live proof:** beat-self **S_p50 ≤ 5** on Joel / local after hard-refresh of post-`bd21765` build (v34+). Green RTP axes were already seen; wm path was the blocker this leaveoff targets.

---

## Suggested next live session command block

```bash
# Restart clean
./scripts/run.sh host --online
# Play 30s+ with pad, then:
# copy(JSON.stringify(window.__couchlinkRicardo()))
./scripts/joel-live-gate.sh /tmp/ricardo.json
```

Claim **ship** only when **D** is all green on a real scrape — not unit tests alone.
