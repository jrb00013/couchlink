# Amazing Interactive Latency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Ricardo’s playable session into a felt step-change: Chrome lands on WebCodecs by default, the drawer shows honest **input→photon p50**, and capture IPC is instrumented so SHM only lands if Hyper-V wait is proven dominant — without reopening the IDR/push death spiral.

**Architecture:** Separate **input clock** from **presentation clock**. Video = CLVD + WebCodecs latest-frame-wins; RTP = warmup/stall rescue only. CLPD gains `client_ts_ms`; CLVD v4 carries `input_wm` (pad seq). Client correlates paint to the pad send that produced that watermark. Capture stays Hyper-V until counters justify SHM.

**Tech Stack:** `couchlink-host`, `couchlink-proto` (CLPD/CLVD), `web/` (player, WebCodecsCanvas, inputPhoton, DebugDrawer), vitest, cargo unit tests, existing `ricardo_playable_ab`.

**Design:** `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-design.md`

## Global Constraints

- Never reintroduce: 20ms P-budget, 1s keyframe budget, IDR-on-every-timeout, full dual-send when `present_path=webcodecs`.
- Keep `ricardo_playable_ab` (7/7) + sacred `path_flags` / fps-hold / floor≥1250 green.
- Do not kill `couchlink-ds-vhid` or close PCSX2 to test.
- Photon UI labels estimation honesty until clocks are unified (`input→photon (est.)`).
- SHM behind `COUCHLINK_CAPTURE_IPC` with Hyper-V fallback; default stays hyperv until Task 4 decision gate.
- Wire versions must be additive: old clients/hosts interoperate without panic.

---

## File map

| Responsibility | Create / modify |
|---|---|
| WebCodecs stickiness logging + promote | Modify: `web/src/App.tsx`, `web/src/player.ts` |
| CLPD v2 (`client_ts_ms`) | Modify: `crates/proto/src/pad_frame.rs`, `web/src/clpd.ts`, tests |
| Host pad timeline + wm | Modify: `crates/host/src/webrtc_peer.rs` (pad decode + stamp CLVD) |
| CLVD v4 (`input_wm`) | Modify: `crates/proto/src/video_frame.rs`, `web/src/clvd.ts`, tests |
| True input→photon | Modify: `web/src/inputPhoton.ts`, `webCodecsCanvas.ts`, `App.tsx`, `DebugDrawer.tsx` |
| Soft button hold (one tick) | Modify: `web/src/player.ts` (+ small helper) |
| Handoff counters / IPC enum | Modify: `crates/host/src/capture/mod.rs`, `hyperv_bridge.rs`, win-capture writer |
| Amazing A/B gates | Create: `crates/host/src/amazing_latency_ab.rs`; Modify: `lib`/`main` module include |
| Regressions | Extend existing `*_test.ts` / `*_ab.rs` |

---

### Task 1: WebCodecs-default (felt path)

**Files:**
- Modify: `web/src/App.tsx` (fallback / stall / promote paths)
- Modify: `web/src/player.ts` (`preferRtpPresent`, `resumeWarmup`, `promoteWebcodecs`)
- Test: `web/src/presentPromote.test.ts` (new) — pure helpers extracted if needed

**Interfaces:**
- Consumes: existing `promoteWebcodecs()`, `resumeWarmup()`, `preferRtpPresent()`, `canUseWebCodecs()`
- Produces: `PresentStuckReason` union + `logPresentStuck(reason)` called whenever visible path is still canvas/RTP after 3s while WebCodecs was attempted

- [ ] **Step 1: Write the failing test for stuck-reason taxonomy**

Create `web/src/presentPromote.ts`:

```ts
export type PresentStuckReason =
  | "no_au"
  | "decoder_fail"
  | "fallback_timer"
  | "ua_legacy"
  | "stall_warmup";

export function classifyPresentStuck(opts: {
  preferLegacy: boolean;
  hasDecoder: boolean;
  sawAu: boolean;
  painted: boolean;
  stalled: boolean;
  fallbackFired: boolean;
}): PresentStuckReason | null {
  if (opts.preferLegacy) return "ua_legacy";
  if (!opts.hasDecoder) return "decoder_fail";
  if (opts.stalled) return "stall_warmup";
  if (opts.fallbackFired && !opts.painted) return "fallback_timer";
  if (!opts.sawAu) return "no_au";
  return null;
}
```

Create `web/src/presentPromote.test.ts` asserting each branch returns the matching reason and that `painted && !stalled && !fallbackFired` → `null`.

- [ ] **Step 2: Run test to verify it fails**

```bash
cd web && npx vitest run src/presentPromote.test.ts
```

Expected: FAIL (module missing).

- [ ] **Step 3: Implement classifier + wire logging in App**

Implement `presentPromote.ts`. In `App.tsx` `armWebCodecsFallback` and stall handler, call:

```ts
clog("present stuck", {
  reason: classifyPresentStuck({ ... }),
  hasDecoder: typeof VideoDecoder === "function",
  secure: window.isSecureContext,
});
```

Confirm stall path already: `resumeWarmup()` + keep decoder warm + re-promote on next paint (do **not** call `preferRtpPresent()` on the 2.5s timer).

- [ ] **Step 4: Run tests**

```bash
cd web && npx vitest run src/presentPromote.test.ts src/presentAge.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/presentPromote.ts web/src/presentPromote.test.ts web/src/App.tsx
git commit -m "$(cat <<'EOF'
feat(web): classify why present stays off WebCodecs

Log structured stuck reasons and keep stall→warmup re-promote so Chrome
can become the felt path within seconds of join.
EOF
)"
```

### Regressions — T1

| ID | Case | Pass |
|---|---|---|
| T1-R1 | First paint | `promoteWebcodecs` → host `present_path=webcodecs` |
| T1-R2 | Stall then paint | re-promote; not permanent RTP |
| T1-R3 | No VideoDecoder | canvas/rtp; no black hole |
| T1-R4 | Sacred S1 | `path_flags(PATH_WEBCODECS)=(false,true)` |
| T1-R5 | Live Chrome | present webcodecs &lt;3s |

---

### Task 2: CLPD v2 — `client_ts_ms` on the wire

**Files:**
- Modify: `crates/proto/src/pad_frame.rs`
- Modify: `web/src/clpd.ts`, `web/src/clpd.test.ts`
- Modify: `web/src/player.ts` (`pollAndSendPad` — stamp + soft hold)
- Test: proto unit tests in `pad_frame.rs`; `web/src/clpd.test.ts`

**Interfaces:**
- Consumes: existing `PadFrame` / `PadState` / `encodeClpd`
- Produces:
  - Rust: `PAD_VERSION_V2: u8 = 2`, `PAD_FRAME_LEN_V2: usize = 35`, `PadFrame.client_ts_ms: u32` (0 = unknown / v1)
  - TS: `PadState.clientTsMs?: number`; `encodeClpd` writes 35 bytes when `clientTsMs` set
  - Decode: accept len≥31 v1 **and** len≥35 v2; reject other versions

Wire layout v2 = v1 (31 bytes) + `client_ts_ms` u32 LE (replace trailing reserved semantics: byte 30 stays reserved 0; bytes 31–34 = ts).

- [ ] **Step 1: Failing Rust test — v1 still decodes; v2 round-trips ts**

In `crates/proto/src/pad_frame.rs` tests:

```rust
#[test]
fn v1_31_byte_frame_decodes_with_zero_client_ts() {
    let f = PadFrame { seq: 7, buttons: 1, ..PadFrame::neutral() };
    let mut buf = BytesMut::new();
    f.encode_v1(&mut buf); // or encode with version forced to 1
    assert_eq!(buf.len(), 31);
    let back = PadFrame::decode(&buf).unwrap();
    assert_eq!(back.seq, 7);
    assert_eq!(back.client_ts_ms, 0);
}

#[test]
fn v2_round_trips_client_ts_ms() {
    let f = PadFrame {
        seq: 9,
        client_ts_ms: 1_234_567,
        ..PadFrame::neutral()
    };
    let mut buf = BytesMut::new();
    f.encode(&mut buf); // default encode = v2 once shipped
    assert_eq!(buf.len(), 35);
    let back = PadFrame::decode(&buf).unwrap();
    assert_eq!(back.client_ts_ms, 1_234_567);
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p couchlink-proto v2_round_trips_client_ts_ms -- --nocapture
```

- [ ] **Step 3: Implement pad_frame v2**

- Add `client_ts_ms: u32` to `PadFrame` (default 0).
- `encode`: write `PAD_VERSION=2`, full 35 bytes.
- `decode`: if ver==1 && len>=31 → ts=0; if ver==2 && len>=35 → read ts; else BadVersion/Short.
- Keep a private/`#[cfg(test)]` v1 encode helper for compatibility tests **or** encode v1 when `client_ts_ms==0 && force_v1` — prefer dual decode + always-v2 encode from new clients.

- [ ] **Step 4: Mirror in `web/src/clpd.ts`**

```ts
export function encodeClpd(p: PadState): ArrayBuffer {
  const withTs = p.clientTsMs != null;
  const buf = new ArrayBuffer(withTs ? 35 : 31);
  // ... existing fields ...
  v.setUint8(4, withTs ? 2 : 1);
  if (withTs) v.setUint32(31, p.clientTsMs! >>> 0, true);
  return buf;
}
```

Update `player.ts` `pollAndSendPad`:

```ts
const clientTsMs = performance.now() >>> 0; // wrap ok — relative deltas only
const state = { ...fromBrowserGamepad(gp, this.seq), clientTsMs };
// soft hold: if digital buttons all clear but previous had buttons <8ms ago, keep prev buttons once
this.padDc.send(encodeClpd(holdDigitalOneTick(state, this.lastPadState, now)));
notePadSent(performance.now(), state.seq, clientTsMs);
```

- [ ] **Step 5: Tests + commit**

```bash
cargo test -p couchlink-proto pad_frame
cd web && npx vitest run src/clpd.test.ts src/inputPhoton.test.ts
```

```bash
git add crates/proto/src/pad_frame.rs web/src/clpd.ts web/src/clpd.test.ts web/src/player.ts
git commit -m "$(cat <<'EOF'
feat(pad): CLPD v2 carries client_ts_ms for photon correlation

Additive decode keeps v1 peers working; new clients stamp every poll.
EOF
)"
```

### Regressions — T2a

| ID | Case | Pass |
|---|---|---|
| T2-R1 | Old 31-byte pad | host applies, no panic |
| T2-R4 | Soft hold one tick | R2 stays pressed across one cleared poll |
| T2-R6 | Mixed clients | session joins |

---

### Task 3: CLVD v4 `input_wm` + host timeline + true photon

**Files:**
- Modify: `crates/proto/src/video_frame.rs`, `web/src/clvd.ts`, `web/src/clvd.test.ts`
- Modify: `crates/host/src/webrtc_peer.rs` (track last pad seq/ts per peer; stamp AU)
- Modify: `web/src/inputPhoton.ts`, `web/src/inputPhoton.test.ts`
- Modify: `web/src/webCodecsCanvas.ts` (pass `inputWm` into paint stats)
- Modify: `web/src/App.tsx`, `web/src/DebugDrawer.tsx`

**Interfaces:**
- Consumes: Task 2 `PadFrame.client_ts_ms` / `notePadSent(seq, clientTsMs)`
- Produces:
  - `VIDEO_VERSION_V4 = 4`, header len **30** = v3(26) + `input_wm: u32` LE
  - `VideoAccessUnit.input_wm: u32` (0 = none)
  - Host: on each pad decode, `last_input_wm = frame.seq` (monotonic max)
  - On CLVD encode: `au.input_wm = last_input_wm`
  - Client: `recordPhotonSample(paintMs, inputWm) → photonMs | null`
  - Drawer: `photonP50Ms` labeled `input→photon (est.)`

Photon formula (client clocks only — honest “est.”):

```
notePadSent(perfNow, seq, clientTsMs)  // ring buffer last N≈256
onPaint(paintPerf, inputWm):
  entry = ring.find(seq == inputWm)
  if entry: photonMs = paintPerf - entry.perfSent
```

Do **not** subtract host `stamp_us` into this metric (different clock domain).

- [ ] **Step 1: Failing proto test for v4 header**

```rust
#[test]
fn v4_round_trips_input_wm() {
    let au = VideoAccessUnit {
        seq: 3,
        width: 1280,
        height: 720,
        keyframe: false,
        annex_b: vec![0, 0, 0, 1, 0x09],
        stamp_us: 99,
        input_wm: 42,
    };
    let frags = au.encode_fragments();
    let back = /* assemble */ ;
    assert_eq!(back.input_wm, 42);
}

#[test]
fn v3_peer_still_decodes_with_wm_zero() {
    // encode with VIDEO_VERSION=3 path or fixture bytes without wm
    assert_eq!(decoded.input_wm, 0);
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p couchlink-proto v4_round_trips_input_wm
```

- [ ] **Step 3: Implement v4 in Rust + TS**

- Bump default encode version to 4; decode accepts 2, 3, 4.
- Update `wan3_math::clvd_wire_bytes` header size if it hardcodes 26.
- Host pad handler: store `AtomicU32` / mutex `last_pad_seq` on the peer; set `stamp` path in existing CLVD build (`stamp_us: age::now_us()`, `input_wm: last_pad_seq`).
- TS `decodeClvdFragment`: if ver===4 read `inputWm` at offset 26.

- [ ] **Step 4: Expand `inputPhoton.ts`**

```ts
type PadSend = { seq: number; perfSent: number };
const ring: PadSend[] = [];
const MAX = 256;
const photonSamples: number[] = [];

export function notePadSent(atMs = performance.now(), seq?: number, _clientTsMs?: number): void {
  lastPadSentAt = atMs;
  if (seq != null) {
    ring.push({ seq, perfSent: atMs });
    if (ring.length > MAX) ring.shift();
  }
}

export function notePhotonPaint(paintMs: number, inputWm: number): number | null {
  if (!inputWm) return null;
  const hit = [...ring].reverse().find((e) => e.seq === inputWm);
  if (!hit) return null;
  const ms = Math.max(0, paintMs - hit.perfSent);
  photonSamples.push(ms);
  if (photonSamples.length > 120) photonSamples.shift();
  return ms;
}

export function photonP50Ms(): number | null { /* sort copy, percentile */ }

export function inputFreshnessMs(...) // keep existing for canvas path
```

Wire `notePhotonPaint` from WebCodecs paint path when AU carries `inputWm`.

- [ ] **Step 5: HUD**

DebugDrawer / videoDiag string:

`LIVE · {fps} · {age}ms · photon {p50}ms (est.) · decode {ms}`

- [ ] **Step 6: Tests + commit**

```bash
cargo test -p couchlink-proto video_frame
cargo test -p couchlink-host ricardo_playable_ab
cd web && npx vitest run src/clvd.test.ts src/inputPhoton.test.ts
```

```bash
git add crates/proto/src/video_frame.rs crates/host/src/webrtc_peer.rs \
  web/src/clvd.ts web/src/inputPhoton.ts web/src/webCodecsCanvas.ts \
  web/src/App.tsx web/src/DebugDrawer.tsx web/src/**/*.test.ts
git commit -m "$(cat <<'EOF'
feat: CLVD v4 input watermark + client input→photon (est.)

Pad seq stamps each AU; drawer shows p50 so we optimize button→picture
instead of push_ms.
EOF
)"
```

### Regressions — T3 / photon

| ID | Case | Pass |
|---|---|---|
| T2-R2 | Watermark monotonic | frame N wm ≥ N−1 (host unit) |
| T2-R3 | Photon fixture | known Δms in vitest |
| T2-R5 | Live | photon p50 ≤ RTT+45ms Ricardo-class |
| T3-R1 | LFW `shouldReplacePending` | newer wins |
| T3-R3 | Drawer shows photon while pad active | not `—` |

---

### Task 4: Handoff truth → SHM (gated)

**Files:**
- Modify: `crates/host/src/capture/hyperv_bridge.rs`, `crates/host/src/capture/mod.rs`, `crates/host/src/main.rs` (5s log already has wait/copy — extend)
- Modify: `crates/capture-bridge/src/bin/win_capture.rs` (frames_sent counter log)
- Create (only if gate trips): `crates/host/src/capture/shm_bridge.rs` + win-capture shm writer
- Test: `parse_capture_ipc` unit tests in `capture/mod.rs`

**Interfaces:**
- Consumes: existing `take_handoff_ms() -> (wait_ms, copy_ms)`
- Produces:
  - `enum CaptureIpc { HyperV, Tcp, Shm }` via `COUCHLINK_CAPTURE_IPC` + `--windows-capture` prefix `shm:…`
  - Counters: `frames_sent` (win-capture), `frames_received` (host), logged every 5s with wait/copy
  - **Decision gate (do not implement SHM until):** live `wait_ms` p95 &gt; 1.0 **or** sent−recv gap material over a 60s window

- [ ] **Step 1: Failing test — IPC parse**

```rust
#[test]
fn parse_capture_ipc_accepts_shm_hyperv_tcp() {
    assert_eq!(parse_capture_ipc("shm"), CaptureIpc::Shm);
    assert_eq!(parse_capture_ipc("hyperv"), CaptureIpc::HyperV);
    assert!(parse_capture_ipc("nope").is_err());
}
```

- [ ] **Step 2: Implement counters + parse only (no SHM body yet)**

Log line shape:

```text
capture ipc=hyperv frames_sent=… frames_recv=… handoff wait=…ms copy=…ms
```

win-capture: increment on each successful write; optionally JSON/stats on stderr every 5s (host already owns the 5s stage log — prefer host-received + wait/copy; add sent via bridge header later if needed). Minimum: host `frames_received` + wait/copy is enough for gate; add win-capture `frames_sent` log for A/B.

- [ ] **Step 3: Document decision in PR comment after one live night**

If wait p95 ≤ 0.5ms and sent≈recv → **skip SHM**, mark Task 4 done with proof.
If gate trips → implement SHM ring (same NAL/BGRA payload as today), flag default off.

- [ ] **Step 4: Commit instrumentation**

```bash
git commit -m "$(cat <<'EOF'
feat(capture): handoff counters + CaptureIpc parse for SHM gate

Measure wait vs copy before replacing Hyper-V with shared memory.
EOF
)"
```

### Regressions — T4

| ID | Case | Pass |
|---|---|---|
| T4-R1 | Spec parse | shm/hyperv/tcp; invalid errors |
| T4-R2 | SHM unavailable | Hyper-V fallback streams |
| T4-R3 | Capture blip | no IDR storm |
| T4-R4 | Live | wait p95 documented; sent≈recv or SHM on |

---

### Task 5: Amazing gate + climb last

**Files:**
- Create: `crates/host/src/amazing_latency_ab.rs`
- Modify: module include in `crates/host/src/lib.rs` or `main.rs` test cfg
- Modify: PR checklist comment template (inline in this plan — paste on PR)

**Interfaces:**
- Consumes: design bars (photon ≤ RTT+45, webcodecs default, no death spiral)
- Produces: unit tests locking formulas/flags; live checklist

- [ ] **Step 1: Failing amazing_latency_ab tests**

```rust
#[test]
fn photon_wow_bar_is_rtt_plus_45() {
    assert_eq!(photon_wow_budget_ms(48.0), 93.0);
}

#[test]
fn webcodecs_path_still_clvd_only() {
    assert_eq!(path_flags(PATH_WEBCODECS), (false, true));
}
```

- [ ] **Step 2: Implement helpers + include module**

- [ ] **Step 3: Full regression**

```bash
cargo test -p couchlink-host ricardo_playable_ab
cargo test -p couchlink-host amazing_latency_ab
cargo test -p couchlink-host
cd web && npx vitest run
```

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
test: lock amazing-latency bars (photon budget + webcodecs path)

Bitrate/1080 climb stays blocked until live photon p50 passes.
EOF
)"
```

### Live PR checklist (paste)

```text
AMAZE-1 photon p50 ≤ RTT+45ms (friend drawer)
AMAZE-2 present=webcodecs on Chrome <3s
AMAZE-3 no 1Hz keyframe-budget spam / IDR storm
AMAZE-4 ricardo_playable_ab 7/7 + host units green
AMAZE-5 handoff wait p95 recorded (SHM only if gate)
```

### Regressions — sacred

| ID | Case | Pass |
|---|---|---|
| S-all | `ricardo_playable_ab` | 7/7 |
| S-host | full host unit | 0 fail |
| S-web | age/present/photon vitest | 0 fail |
| AMAZE-* | live checklist | all |

---

## Execution order

```text
T1 WebCodecs-default     ← felt path
T2 CLPD client_ts        ← wire for correlation
T3 CLVD wm + photon HUD  ← north-star metric
T4 Handoff counters→SHM  ← only if measured
T5 Amazing gate          ← then quality climb
```

---

## Domain of validity

- Chrome/WebCodecs friends; Safari stays RTP.
- Ricardo-class WAN (~40–80ms RTT).
- PCSX2 + persistent ViGEm unchanged.

## Failed guesses (do not reopen)

| Guess | Why it felt “barely” |
|---|---|
| Chase push_ms / push fps | Already ~0 / ~76 |
| Age-only without input watermark | Display freshness ≠ interactivity |
| Bitrate climb first | Quality ≠ input→photon |

## Done when

Friends say the session feels snappier (not just “still playable”), drawer shows **input→photon (est.)**, Chrome is on WebCodecs, and sacred + Ricardo A/B stay green.

---

## Spec coverage self-review

| Design requirement | Task |
|---|---|
| WebCodecs within 3s | T1 |
| Continuous input→photon p50 | T2+T3 |
| ≤ RTT+45ms wow bar | T3 live + T5 |
| Handoff proof / SHM if needed | T4 |
| Sacred / no death spiral | Global + T5 |
| Bitrate climb last | T5 explicit |
| Soft button hold v1 | T2 |
| No native viewer | out of scope |

No TBD placeholders remain for wire sizes or formulas.
