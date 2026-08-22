# Audit: 4:4:4 color accuracy at HD without paying input lag

Status: **audit / plan only** — no encoder or transport changes in this document.
Companions: `LATENCY.md`, `OPTIMIZATION_PLAN.md`,
`superpowers/plans/2026-08-06-full-latency-optimization-plan.md`.

Today's path (Windows capture → MF H.264 → host relay → CLVD/WebCodecs or RTP):

| Stage | Current format | Chroma |
|---|---|---|
| DXGI capture | `B8G8R8A8_UNORM` | full RGB (4:4:4 equivalent) |
| GPU convert (`gpu_convert`) | `DXGI_FORMAT_NV12` | **4:2:0** |
| MF encode input | `MFVideoFormat_NV12` | **4:2:0** |
| Bitstream | H.264 Main / Baseline | **4:2:0** (constrained) |
| Browser paint | WebCodecs → canvas | RGB after decode |

Color accuracy is lost at the first NV12 conversion. Latency is already fought
elsewhere (`CODECAPI_AVLowLatencyMode`, Hyper-V socket handoff, CLVD without
jitter buffer). The job is to regain chroma **without** reintroducing queueing.

---

## 1. Reframe the reality

The usual question is:

> "How do we switch H.264 to 4:4:4 and still hit &lt;40–60 ms glass-to-glass at
> 1080p60?"

That question bakes in a false wall. Consumer H.264 hardware paths (NVENC /
Quick Sync / AMF via Media Foundation) are built around **NV12 4:2:0**. Asking
them for 4:4:4 either fails capability negotiation or falls back to a software
path that burns the encode budget we already measured as ~0 ms on GPU.

**Change the question to:**

> "Where does chroma actually matter for co-play, and can we keep 4:4:4 only on
> that surface while the motion path stays 4:2:0 HD low-latency?"

Emulator UI text, CRT shaders, and saturated HUD colors care about chroma.
Fast camera pans and gameplay motion mostly care about luma + frame age.
Treating "whole stream must be 4:4:4" as the requirement forces the wrong codec
and the wrong bitrate curve. Treating "perceptually accurate color on the
pixels players stare at" as the requirement opens cheaper doors.

Also separate **color fidelity** from **input lag**. Pad path (`CLPD` ~250 Hz)
and video path share a machine, not a fate. Spending encode budget on chroma
must never lengthen pad inject or grow the viewer's jitter buffer — those are
what "laggy" feels like.

---

## 2. The outsider loop

Don't push 4:4:4 through the pipe that already works for gaming latency.
Change what "accuracy" attaches to.

### 2a. Prefer Hi444PP / HEVC 4:4:4 only where decoders exist — else fake it

| Option | Chroma | HD 60 encode | Browser decode today | Verdict |
|---|---|---|---|---|
| H.264 High 4:4:4 Predictive (Hi444PP) | 4:4:4 | Usually CPU / rare HW | Chrome WebCodecs: limited / often no | Research spike only |
| HEVC Main 4:4:4 | 4:4:4 | Some discrete GPUs | Patchy; Safari/licensing | Not default |
| AV1 4:4:4 | 4:4:4 | Slow or rare HW | Better on desktop Chrome | Too early for default |
| Stay H.264 4:2:0 + **smart chroma protection** | 4:2:0 | Current zero-copy path | Already works (CLVD) | Ship-first |

**Outsider move:** keep NV12 → H.264 Main (or High) for the live stream, and
recover apparent 4:4:4 by:

1. **Stop BT.601 on a BT.709 desktop.** `bgra_to_nv12` documents BT.601; GPU VP
   convert may disagree with WebCodecs' assumed matrix. Wrong matrix reads as
   washed / tinted chroma — players report "color accuracy" when the fix is
   matrix + full-range flags, not 4:4:4.
2. **Full-range (PC) vs studio-swing.** Capture is PC levels. If encode/decode
   assume TV range, midtones crush and saturation looks wrong. Tag
   `video_full_range_flag` / WebCodecs `colorSpace` explicitly end-to-end.
3. **Bitrate before chroma.** At 1080p60, `StreamPreset::P1080_60` is 18 Mbps.
   Chroma error from 4:2:0 is often invisible next to macroblocking on UI text.
   Raise bits on keyframes / high-detail tiles before changing subsample.
4. **Optional RGB stills / ROI.** For menu-heavy moments, a rare lossless or
   PNG-over-DataChannel snapshot of the HUD region is true 4:4:4 where eyes
   linger — without taxing the 60 Hz path. Same spirit as idle-FPS: spend
   quality where motion is zero.

### 2b. If true 4:4:4 encode is required later — dual-tier, not dual-tax

Do **not** replace the low-latency ladder with a single 4:4:4 encode.

- Tier A (default): current zero-copy NV12 → H.264, `AVLowLatencyMode`, 1080p60 /
  720p60 presets, CLVD present.
- Tier B (opt-in "color accurate"): AYUV/Y410 → HEVC or software Hi444PP, capped
  fps (30) or resolution, LAN/WG only, behind a preset like `1080p30-444`.

Players who need pixel color pick Tier B knowing they trade some freshness;
everyone else keeps the lag budget in `LATENCY.md`.

### 2c. Attack lag on a different axis than chroma

From existing measurements and plans — still the highest leverage for *feel*:

| Lever | Why it beats "more chroma" for lag |
|---|---|
| Hyper-V / shared-memory handoff (not TCP vSwitch) | Same-host frames shouldn't queue like a NIC |
| Single present path (CLVD *or* RTP, not both) | Removes self-inflicted jitter |
| Phase-lock capture to composition | Deletes ~½ frame of average wait |
| Wake-on-input (expedite frame after pad) | Cuts felt input→pixels delay |
| Glass-to-glass `age` stamp | Makes every claim falsifiable |
| Keep `AVLowLatencyMode` + short GOP | Encoder delay stays bounded when quality knobs move |

4:4:4 work that increases encode time, forces B-frames, or grows the jitter
buffer fails the product bar even if PSNR chroma looks perfect.

---

## 3. The system fix

Make **chroma mode a first-class stream property**, negotiated once at join,
instrumented every session — so we never again debate "does 4:4:4 hurt lag?"
without numbers.

### 3.1 Wire / capability contract

Extend preset / signaling (sketch — not implemented):

```text
chroma: "420" | "422" | "444"
color_primaries: bt709
matrix: bt709
range: full | limited
codec: h264 | hevc | av1
```

Host advertises what the GPU MFT actually accepted (probe at encoder create).
Player advertises what WebCodecs `isConfigSupported` returned. Intersection
picks the mode. No silent fallback to a high-latency software encode.

### 3.2 Pipeline ownership (where code would change)

| Layer | File / area | 4:2:0 today | 4:4:4 / accuracy work |
|---|---|---|---|
| Capture convert | `crates/capture-bridge/src/gpu_convert.rs` | BGRA → NV12 | Optional AYUV/Y410 path; keep zero-copy |
| CPU fallback | `mf_encoder.rs` `bgra_to_nv12` | BT.601 4:2:0 | BT.709 + documented range; or skip when GPU path live |
| Encoder | `mf_encoder.rs` | NV12 + H.264 Main, `AVLowLatencyMode` | Probe 4:4:4 profiles; refuse if low-latency flag drops |
| SDP / track | `crates/host/src/webrtc_peer.rs` | `profile-level-id=4d0028` | Profile that matches actual SPS; don't claim Hi444 if NV12 |
| Present | `web/src/webCodecsCanvas.ts` | H.264 Annex-B | Pass `colorSpace` into `VideoFrame` / canvas draw |
| Native client | `crates/client/src/view.rs` | Notes studio-swing BT.601 risk | Align matrix with host tags |

### 3.3 HD + low latency acceptance gates

Before any chroma mode ships as default:

1. **Hold 1080p60 (or configured HD preset)** with drop_pct and paint fps within
   ~5% of the 4:2:0 baseline on the same link.
2. **Pad send-rate Hz** must not regress (input path CPU must stay free).
3. **`jitterBufferMs` / CLVD frame age** must not rise more than measurement noise.
4. **`CODECAPI_AVLowLatencyMode` remains on** for that preset; if the MFT rejects
   it for 4:4:4, that preset is LAN-opt-in only, never default.
5. Bitrate: expect ~1.5–2× bits for similar luma quality at 4:4:4 — budget from
   uplink headroom (`OPTIMIZATION_PLAN` utilisation), not by cutting fps first.
   Link governor already prefers cutting fps after quality; don't invert that
   for chroma vanity.

### 3.4 Recommended implementation order

1. **Matrix + range audit (days).** Fix BT.601 vs BT.709 and full-range tagging
   end-to-end; A/B screenshots of saturated UI. Often removes the "need 4:4:4"
   complaint.
2. **Instrument glass-to-glass `age` + chroma mode in `host_stats`.** Without
   this, 4:4:4 debates are vibes.
3. **Finish transport wins** already ranked in `OPTIMIZATION_PLAN.md` (frame
   accounting, IPC/Hyper-V handoff). Free latency budget *before* spending it
   on chroma bits.
4. **Probe HW 4:4:4** on NVENC/QSV/AMF; log accept/reject. Gate a
   `1080p30-444` opt-in preset only if low-latency mode sticks.
5. **Only then** consider Hi444PP/HEVC default — and only when WebCodecs support
   is verified on the browsers friends actually use.

### 3.5 Explicit non-goals (for now)

- Replacing CLVD with a custom UDP codec just to carry 4:4:4.
- Software x264 `zerolatency` + `high444` as the Windows default (CPU encode
  reintroduces the latency cliff hardware encode removed).
- Forcing 4:4:4 on internet/TURN sessions where utilisation is already the
  governor's problem.

---

## Bottom line

Right to bring this here: **true 4:4:4 through today's H.264 MF path is the
wrong wall.** Recover color by fixing matrix/range and bitrate where players
look; keep HD on the zero-copy 4:2:0 low-latency spine; offer real 4:4:4 as a
negotiated opt-in only when the encoder keeps `AVLowLatencyMode` and the
decoder is proven. Spend the lag budget on phase, handoff, and single present
path — not on chroma subsample as a fashion choice.
