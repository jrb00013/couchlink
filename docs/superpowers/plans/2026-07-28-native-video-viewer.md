# Native Video Viewer for couchlink-client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `couchlink-client` into a windowed app that decodes and displays the host's H.264 video stream (via `openh264`, mirroring the host's own encoder) and accepts both DualSense and keyboard input, while keeping today's headless pad-only mode available via `--headless`.

**Architecture:** WebRTC video track → RTP depacketize (`rtp::codecs::h264::H264Packet`) → Annex-B NALs → `openh264` decode on a dedicated OS thread → RGBA frame → `wgpu` texture upload + draw, in a `winit` window running on the main thread. Networking (signaling, WebRTC, decode thread, DualSense polling) runs on a background OS thread hosting its own Tokio runtime, bridged to the winit main-thread loop via channels — `winit`'s event loop must own the main thread and blocks it, so it cannot itself be `#[tokio::main]`.

**Tech Stack:** `openh264` 0.6 (decode, already a workspace dependency via the host crate), `rtp` (transitively pulled by `webrtc` 0.17, used directly for H.264 depacketization), `wgpu` 0.20, `winit` 0.30 (`ApplicationHandler` API), existing `tokio`/`webrtc`/`couchlink-proto` stack.

## Global Constraints

- Video pipeline stages, in order, per the approved spec: Desktop → capture → OpenH264 encode (host, unchanged) → network → OpenH264 decode (new) → wgpu render (new) → input (existing DualSense path, unchanged in its own logic).
- No hardware decode in this iteration — ship `openh264` software decode instrumented with per-frame latency logging (p50/p99 every N frames via `tracing`), and only revisit hardware decode if that data shows ~12-18ms/frame or worse. Do not add VideoToolbox/DXVA/VAAPI code now.
- No new heavy system dependency — do not add ffmpeg/libavcodec to `install.sh` on any platform.
- `couchlink-client` stays one binary. `--headless` (default `false`) preserves today's exact pad-only behavior for automation/testing and as the auto-fallback when window/GPU creation fails.
- Host role (`crates/host`, `run.sh host`) is unchanged by this plan — no edits to `crates/host/**`.
- Decoder errors (bad NAL, keyframe loss) log and drop the frame — never crash the client.
- Keyboard input is a second, independent input source alongside DualSense (added mid-plan per user request) — both must work standalone; when DualSense is connected its frame for a given tick takes priority over the keyboard snapshot for that same tick.
- Multi-player (4-6 pads) is explicitly out of scope for this plan — tracked as a separate follow-on plan (roadmap: "Multi-player (2+ remote pads)").

---

## File Structure

- `crates/client/Cargo.toml` — modify: add `rtp`, `openh264`, `wgpu`, `winit`, `pollster`, `bytemuck` dependencies.
- `crates/client/src/decode.rs` — new: `H264Decoder` (openh264 wrapper + latency instrumentation) and `DecodedFrame` (RGBA output).
- `crates/client/src/webrtc_player.rs` — modify: `on_track` handler depacketizes RTP → Annex-B NALs, feeds `decode.rs`, exposes decoded frames to the caller.
- `crates/client/src/keyboard_input.rs` — new: `KeyboardPad` (held-key state → `PadFrame`), with the WASD/arrow/face-button mapping table.
- `crates/client/src/view.rs` — new: `winit` window + `wgpu` renderer; owns keyboard event capture, forwards to `keyboard_input.rs`.
- `crates/client/src/main.rs` — modify: `--headless` flag, sync `fn main()` that dispatches to `run_windowed` (main-thread winit + background Tokio thread) or the existing `run_headless` async flow (today's `main` body, renamed and left otherwise unchanged), and merges DualSense + keyboard pad sources.
- `scripts/run.ps1` — modify: doc comment only (client role already launches `couchlink-client`, which now defaults to windowed — no argument changes needed).
- `README.md`, `docs/GETTING_STARTED.md` — modify: mention the native viewer window and keyboard input support.

---

### Task 1: Add dependencies

**Files:**
- Modify: `crates/client/Cargo.toml`

**Interfaces:**
- Produces: `openh264`, `rtp`, `wgpu`, `winit`, `pollster`, `bytemuck` available as crates for Tasks 2-5.

- [ ] **Step 1: Add the new dependencies**

Edit `crates/client/Cargo.toml`, adding to `[dependencies]` (after the existing `rustls` line):

```toml
openh264 = "0.6"
rtp = "0.11"
wgpu = "0.20"
winit = "0.30"
pollster = "0.3"
bytemuck = { version = "1", features = ["derive"] }
```

Note: `rtp = "0.11"` is intentional even though `webrtc 0.17` pulls in `rtp 0.17` transitively — check what actually resolves before writing depacketize code in Task 3:

```bash
cargo tree -p couchlink-client -i rtp
```

If it resolves to `0.17.x`, change the `rtp` line above to `rtp = "0.17"` instead (match whatever `webrtc` actually pulled in, so there's only one `rtp` version in the dependency graph — two versions would make `rtp::packet::Packet` from `webrtc`'s calls incompatible with the `rtp::codecs::h264::H264Packet` type used in Task 3).

- [ ] **Step 2: Verify it builds**

Run: `cargo build -p couchlink-client 2>&1 | tail -30`
Expected: succeeds (new deps fetched and compiled, no code uses them yet so no new warnings beyond unused-dependency notes, which are fine at this stage).

- [ ] **Step 3: Commit**

```bash
git add crates/client/Cargo.toml Cargo.lock
git commit -m "build(client): add openh264/rtp/wgpu/winit deps for native viewer"
```

---

### Task 2: `decode.rs` — H.264 decoder wrapper with latency instrumentation

**Files:**
- Create: `crates/client/src/decode.rs`
- Modify: `crates/client/src/main.rs:1-4` (add `mod decode;`)

**Interfaces:**
- Produces:
  - `pub struct DecodedFrame { pub rgba: Vec<u8>, pub width: u32, pub height: u32 }`
  - `pub struct H264Decoder` with `pub fn new() -> anyhow::Result<Self>` and `pub fn decode(&mut self, annex_b_nal: &[u8]) -> anyhow::Result<Option<DecodedFrame>>`
- Consumes: nothing from other new modules (uses only `openh264`, `tracing`, `anyhow`).

- [ ] **Step 1: Write the failing test**

Create `crates/client/src/decode.rs` with just the test first — it reuses the host's own encoder (already a `couchlink-host`-only dependency, so we depend on `openh264` directly here rather than on `couchlink-host` to avoid a cross-binary dependency; the test constructs its own tiny encoder inline):

```rust
//! H.264 decode via openh264, mirroring the host's openh264 encode path
//! (crates/host/src/encode.rs). Runs on its own OS thread (see webrtc_player.rs) —
//! never call `decode` from an async context.

use anyhow::Result;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::formats::YUVSource;
use std::time::Instant;
use tracing::{info, warn};

pub struct DecodedFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct H264Decoder {
    decoder: Decoder,
    frame_count: u64,
    latencies_us: Vec<u64>,
}

const LATENCY_LOG_EVERY: usize = 120; // ~once every 2s at 60fps

impl H264Decoder {
    pub fn new() -> Result<Self> {
        let decoder = Decoder::with_config(DecoderConfig::new())?;
        Ok(Self {
            decoder,
            frame_count: 0,
            latencies_us: Vec::with_capacity(LATENCY_LOG_EVERY),
        })
    }

    /// `annex_b_nal` may contain one or more Annex-B NAL units (start-code prefixed).
    /// Returns `Some(frame)` once a full picture has been decoded, `None` if this
    /// call only advanced decoder state (e.g. parameter sets) without emitting a frame.
    pub fn decode(&mut self, annex_b_nal: &[u8]) -> Result<Option<DecodedFrame>> {
        let start = Instant::now();
        let result = self.decoder.decode(annex_b_nal);
        let elapsed_us = start.elapsed().as_micros() as u64;

        let decoded = match result {
            Ok(Some(d)) => d,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!("h264 decode error, dropping frame: {e}");
                return Ok(None);
            }
        };

        self.latencies_us.push(elapsed_us);
        self.frame_count += 1;
        if self.latencies_us.len() >= LATENCY_LOG_EVERY {
            self.log_latency_stats();
        }

        let (width, height) = decoded.dimensions();
        let mut rgba = vec![0u8; width * height * 4];
        decoded.write_rgba8(&mut rgba);

        Ok(Some(DecodedFrame {
            rgba,
            width: width as u32,
            height: height as u32,
        }))
    }

    fn log_latency_stats(&mut self) {
        self.latencies_us.sort_unstable();
        let p50 = self.latencies_us[self.latencies_us.len() / 2];
        let p99 = self.latencies_us[(self.latencies_us.len() * 99 / 100).min(self.latencies_us.len() - 1)];
        info!(
            "decode latency over {} frames: p50={:.1}ms p99={:.1}ms",
            self.latencies_us.len(),
            p50 as f64 / 1000.0,
            p99 as f64 / 1000.0
        );
        self.latencies_us.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openh264::encoder::{Encoder, EncoderConfig};
    use openh264::formats::{RgbSliceU8, YUVBuffer};

    fn encode_one_solid_frame(width: usize, height: usize) -> Vec<u8> {
        let mut encoder = Encoder::with_config(EncoderConfig::new()).unwrap();
        let rgb = vec![80u8; width * height * 3];
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (width, height)));
        let bitstream = encoder.encode(&yuv).unwrap();
        let mut out = Vec::new();
        for l in 0..bitstream.num_layers() {
            let layer = bitstream.layer(l).unwrap();
            for n in 0..layer.nal_count() {
                out.extend_from_slice(layer.nal_unit(n).unwrap());
            }
        }
        out
    }

    #[test]
    fn decodes_a_real_encoded_frame() {
        let width = 64;
        let height = 64;
        let annex_b = encode_one_solid_frame(width, height);

        let mut decoder = H264Decoder::new().unwrap();
        let frame = decoder
            .decode(&annex_b)
            .unwrap()
            .expect("first real frame should decode to a picture");

        assert_eq!(frame.width, width as u32);
        assert_eq!(frame.height, height as u32);
        assert_eq!(frame.rgba.len(), width * height * 4);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/client/src/main.rs`, add near the other `mod` declarations at the top:

```rust
mod decode;
```

- [ ] **Step 3: Run test to verify it fails or passes**

Run: `cargo test -p couchlink-client decode::tests::decodes_a_real_encoded_frame -- --nocapture`

This test is written against the real `openh264` API confirmed by reading the installed crate source (`~/.cargo/registry/.../openh264-0.6.6/src/decoder.rs`), so it should compile and pass on the first try — but `openh264`/`rtp` exact method names can differ by patch version. If it fails to compile, check the actual installed version's API:

```bash
find ~/.cargo/registry/src -maxdepth 1 -iname "openh264-0.6*"
grep -n "pub fn with_config\|pub fn decode\b\|pub fn write_rgba8\|pub fn dimensions\b" ~/.cargo/registry/src/*/openh264-0.6*/src/decoder.rs
```

Adjust `decode.rs` to match, then rerun.

Expected: PASS (`frame.width == 64`, `frame.height == 64`, correct buffer length).

- [ ] **Step 4: Commit**

```bash
git add crates/client/src/decode.rs crates/client/src/main.rs
git commit -m "feat(client): openh264 decode wrapper with latency instrumentation"
```

---

### Task 3: Wire the video track into the decoder

**Files:**
- Modify: `crates/client/src/webrtc_player.rs`

**Interfaces:**
- Consumes: `decode::H264Decoder`, `decode::DecodedFrame` (Task 2).
- Produces: `WebRtcPlayer::new(..)` now also returns a `tokio::sync::mpsc::UnboundedReceiver<decode::DecodedFrame>` — signature becomes:
  ```rust
  pub async fn new(
      signal_out: mpsc::UnboundedSender<SignalMessage>,
      turn_url: Option<String>,
      turn_user: Option<String>,
      turn_pass: Option<String>,
  ) -> Result<(Self, mpsc::UnboundedReceiver<crate::decode::DecodedFrame>)>
  ```
  (Every existing caller of `WebRtcPlayer::new` must be updated to destructure the tuple — see Task 6.)

- [ ] **Step 1: Confirm the RTP depacketizer API matches what's installed**

```bash
grep -n "pub struct H264Packet\|impl Depacketizer for H264Packet\|fn depacketize" \
  ~/.cargo/registry/src/*/rtp-*/src/codecs/h264/mod.rs
```

Confirm `H264Packet { is_avc: bool, .. }` and `fn depacketize(&mut self, b: &Bytes) -> Result<Bytes>` still match (verified against `rtp-0.17.2` while writing this plan — re-check if `cargo tree` in Task 1 Step 1 resolved a different version).

- [ ] **Step 2: Add the depacketize + decode task in `on_track`**

Replace the existing `pc.on_track(...)` stub in `crates/client/src/webrtc_player.rs`:

```rust
pc.on_track(Box::new(move |track, _, _| {
    Box::pin(async move {
        info!("video track received: {}", track.codec().capability.mime_type);
        // Decode/display is left to a viewer frontend or SDL sink in a follow-up.
    })
}));
```

with:

```rust
pc.on_track(Box::new(move |track, _, _| {
    let nal_tx = nal_tx.clone();
    Box::pin(async move {
        info!("video track received: {}", track.codec().capability.mime_type);
        let mut depacketizer = rtp::codecs::h264::H264Packet {
            is_avc: false, // false => depacketize() emits Annex-B (start-code) NALs
            ..Default::default()
        };
        loop {
            match track.read_rtp().await {
                Ok((packet, _attrs)) => {
                    use rtp::packetizer::Depacketizer;
                    match depacketizer.depacketize(&packet.payload) {
                        Ok(nal) if !nal.is_empty() => {
                            if nal_tx.send(nal).is_err() {
                                break; // decode thread gone, stop reading
                            }
                        }
                        Ok(_) => {} // mid-fragment, nothing to emit yet
                        Err(e) => warn!("rtp depacketize error: {e}"),
                    }
                }
                Err(e) => {
                    warn!("video track read_rtp ended: {e}");
                    break;
                }
            }
        }
    })
}));
```

- [ ] **Step 3: Spawn the decode thread and wire channels in `WebRtcPlayer::new`**

Change the signature and add, right before the existing `pc.on_track(...)` block (so `nal_tx` exists when that closure captures it):

```rust
use crate::decode::{DecodedFrame, H264Decoder};
use std::sync::mpsc as std_mpsc;

// ... inside `pub async fn new(...) -> Result<(Self, mpsc::UnboundedReceiver<DecodedFrame>)> {`
// (change the return type on the fn signature itself, and wrap the final `Ok(Self { pc, pad_dc })`)

let (nal_tx, nal_rx) = mpsc::unbounded_channel::<bytes::Bytes>();
let (frame_tx, frame_rx) = mpsc::unbounded_channel::<DecodedFrame>();

std::thread::Builder::new()
    .name("couchlink-decode".into())
    .spawn(move || {
        let mut decoder = match H264Decoder::new() {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("failed to init h264 decoder: {e}");
                return;
            }
        };
        let mut nal_rx = nal_rx;
        while let Some(nal) = nal_rx.blocking_recv() {
            match decoder.decode(&nal) {
                Ok(Some(frame)) => {
                    if frame_tx.send(frame).is_err() {
                        break; // viewer gone
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("decode error: {e}"),
            }
        }
    })
    .expect("spawn decode thread");
```

Then at the end of the function, change:

```rust
Ok(Self { pc, pad_dc })
```

to:

```rust
Ok((Self { pc, pad_dc }, frame_rx))
```

Add `mod decode;` is already done in Task 2 — but `decode.rs` needs to be reachable from `webrtc_player.rs` via `crate::decode`, which it is since both are top-level modules in the same crate.

- [ ] **Step 4: Fix the caller in `main.rs` temporarily so the crate compiles**

This will be finished properly in Task 6, but to keep the build green after this task, update the one call site in `crates/client/src/main.rs`:

```rust
let player = webrtc_player::WebRtcPlayer::new(
    signal_out.clone(),
    args.turn_url.clone(),
    args.turn_user.clone(),
    args.turn_pass.clone(),
)
.await?;
```

to:

```rust
let (player, mut _video_frames) = webrtc_player::WebRtcPlayer::new(
    signal_out.clone(),
    args.turn_url.clone(),
    args.turn_user.clone(),
    args.turn_pass.clone(),
)
.await?;
```

(`_video_frames` is intentionally unused here — Task 6 replaces this whole call site as part of the headless/windowed split.)

- [ ] **Step 5: Build and fix any API mismatches**

Run: `cargo build -p couchlink-client 2>&1 | tail -60`

Fix any compile errors against the actual installed `rtp`/`webrtc` API (method names occasionally shift between patch versions — trust the compiler error over this plan's snippets if they disagree).

Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add crates/client/src/webrtc_player.rs crates/client/src/main.rs
git commit -m "feat(client): depacketize video track RTP and decode via H264Decoder"
```

---

### Task 4: `keyboard_input.rs` — keyboard as a second pad input source

**Files:**
- Create: `crates/client/src/keyboard_input.rs`
- Modify: `crates/client/src/main.rs` (add `mod keyboard_input;`)

**Interfaces:**
- Produces:
  - `pub struct KeyboardPad` with `pub fn new() -> Self`, `pub fn set_key(&mut self, code: winit::keyboard::KeyCode, pressed: bool)`, `pub fn to_pad_frame(&self, seq: u32) -> couchlink_proto::PadFrame`, `pub fn any_key_active(&self) -> bool`.
- Consumes: `couchlink_proto::PadFrame` and `couchlink_proto::pad_frame::buttons::*` (already a workspace dependency, `crates/proto/src/pad_frame.rs`).

- [ ] **Step 1: Write the failing test**

Create `crates/client/src/keyboard_input.rs`:

```rust
//! Keyboard → PadFrame mapping, so a friend without a DualSense can still play.
//! Fixed layout (not remappable yet — keep it simple until someone asks):
//!
//! WASD          → left stick (digital: full deflection, no analog values)
//! Arrow keys     → D-pad
//! Space          → Cross
//! Left Shift     → Square
//! Left Ctrl      → Circle
//! E              → Triangle
//! Q / R          → L1 / R1
//! 1 / 2          → L2 / R2 (digital: 0 or 255)
//! Enter          → Options
//! Tab            → Create

use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;
use std::collections::HashSet;
use winit::keyboard::KeyCode;

const NEUTRAL: u8 = 127;
const FULL: u8 = 255;
const ZERO: u8 = 0;

#[derive(Default)]
pub struct KeyboardPad {
    held: HashSet<KeyCode>,
}

impl KeyboardPad {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    pub fn any_key_active(&self) -> bool {
        !self.held.is_empty()
    }

    pub fn to_pad_frame(&self, seq: u32) -> PadFrame {
        let h = &self.held;
        let mut buttons_mask = 0u32;

        let mut set = |cond: bool, bit: u32| {
            if cond {
                buttons_mask |= bit;
            }
        };
        set(h.contains(&KeyCode::Space), buttons::CROSS);
        set(h.contains(&KeyCode::ShiftLeft), buttons::SQUARE);
        set(h.contains(&KeyCode::ControlLeft), buttons::CIRCLE);
        set(h.contains(&KeyCode::KeyE), buttons::TRIANGLE);
        set(h.contains(&KeyCode::KeyQ), buttons::L1);
        set(h.contains(&KeyCode::KeyR), buttons::R1);
        set(h.contains(&KeyCode::Enter), buttons::OPTIONS);
        set(h.contains(&KeyCode::Tab), buttons::CREATE);
        set(h.contains(&KeyCode::ArrowUp), buttons::DPAD_UP);
        set(h.contains(&KeyCode::ArrowDown), buttons::DPAD_DOWN);
        set(h.contains(&KeyCode::ArrowLeft), buttons::DPAD_LEFT);
        set(h.contains(&KeyCode::ArrowRight), buttons::DPAD_RIGHT);

        let lx = if h.contains(&KeyCode::KeyA) {
            ZERO
        } else if h.contains(&KeyCode::KeyD) {
            FULL
        } else {
            NEUTRAL
        };
        let ly = if h.contains(&KeyCode::KeyW) {
            ZERO
        } else if h.contains(&KeyCode::KeyS) {
            FULL
        } else {
            NEUTRAL
        };
        let l2 = if h.contains(&KeyCode::Digit1) { FULL } else { ZERO };
        let r2 = if h.contains(&KeyCode::Digit2) { FULL } else { ZERO };

        PadFrame {
            seq,
            buttons: buttons_mask,
            lx,
            ly,
            rx: NEUTRAL,
            ry: NEUTRAL,
            l2,
            r2,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_keys_held_is_neutral() {
        let kp = KeyboardPad::new();
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons, 0);
        assert_eq!(f.lx, NEUTRAL);
        assert_eq!(f.ly, NEUTRAL);
        assert!(!kp.any_key_active());
    }

    #[test]
    fn wasd_maps_to_left_stick() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::KeyD, true);
        kp.set_key(KeyCode::KeyS, true);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.lx, FULL);
        assert_eq!(f.ly, FULL);
        assert!(kp.any_key_active());
    }

    #[test]
    fn space_maps_to_cross_button() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::Space, true);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons & buttons::CROSS, buttons::CROSS);
    }

    #[test]
    fn releasing_a_key_clears_it() {
        let mut kp = KeyboardPad::new();
        kp.set_key(KeyCode::Space, true);
        kp.set_key(KeyCode::Space, false);
        let f = kp.to_pad_frame(1);
        assert_eq!(f.buttons & buttons::CROSS, 0);
    }
}
```

- [ ] **Step 2: Register the module**

Add to `crates/client/src/main.rs`: `mod keyboard_input;`

- [ ] **Step 3: Run the tests**

Run: `cargo test -p couchlink-client keyboard_input:: -- --nocapture`
Expected: all 4 tests PASS. If `winit::keyboard::KeyCode` variant names differ from what's used here, check:
```bash
grep -n "pub enum KeyCode" -A 40 ~/.cargo/registry/src/*/winit-0.30*/src/keyboard.rs
```
and adjust variant names (they're stable across 0.30.x but confirm against what's actually installed).

- [ ] **Step 4: Commit**

```bash
git add crates/client/src/keyboard_input.rs crates/client/src/main.rs
git commit -m "feat(client): keyboard input as a second pad source alongside DualSense"
```

---

### Task 5: `view.rs` — winit window + wgpu renderer

**Files:**
- Create: `crates/client/src/view.rs`
- Modify: `crates/client/src/main.rs` (add `mod view;`)

**Interfaces:**
- Consumes: `decode::DecodedFrame` (Task 2), `keyboard_input::KeyboardPad` (Task 4).
- Produces:
  ```rust
  pub fn run(
      frame_rx: std::sync::mpsc::Receiver<crate::decode::DecodedFrame>,
      keyboard_pad: std::sync::Arc<std::sync::Mutex<crate::keyboard_input::KeyboardPad>>,
      shutdown_tx: std::sync::mpsc::Sender<()>,
  ) -> anyhow::Result<()>
  ```
  Blocks the calling thread until the window is closed (this is `winit`'s normal behavior) or GPU/window setup fails, in which case it returns `Err` so the caller (Task 6) can fall back to headless mode. On close/Esc, sends `()` on `shutdown_tx` so the background networking thread can shut down cleanly.

  Note the channel type change from Task 3/6: `view.rs` runs on the plain OS main thread, not inside Tokio, so it takes a **std** `mpsc::Receiver`, not a Tokio one. Task 6 bridges the Tokio `UnboundedReceiver<DecodedFrame>` from `webrtc_player.rs` into this std channel.

- [ ] **Step 1: Write the module**

Create `crates/client/src/view.rs`:

```rust
//! winit window + wgpu renderer for the decoded H.264 stream. Runs on the main
//! thread (winit requirement); networking/decoding happens on a background
//! thread and hands frames to this one over a channel — see main.rs.

use crate::decode::DecodedFrame;
use crate::keyboard_input::KeyboardPad;
use anyhow::Result;
use std::sync::{mpsc::Receiver, Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    current_texture: Option<(wgpu::BindGroup, u32, u32)>,
}

impl Renderer {
    fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow::anyhow!("no compatible GPU adapter"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("couchlink-client"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
            },
            None,
        ))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(
                r#"
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VOut {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0),
    );
    var out: VOut;
    out.pos = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;
@group(0) @binding(1) var s_frame: sampler;

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return textureSample(t_frame, s_frame, in.uv);
}
"#,
            )),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frame-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(config.format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            current_texture: None,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn upload_frame(&mut self, frame: &DecodedFrame) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame-texture"),
            size: wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.current_texture = Some((bind_group, frame.width, frame.height));
    }

    fn draw(&mut self) -> Result<()> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some((bind_group, _, _)) = &self.current_texture {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    frame_rx: Receiver<DecodedFrame>,
    keyboard_pad: Arc<Mutex<KeyboardPad>>,
    shutdown_tx: std::sync::mpsc::Sender<()>,
    init_error: Option<anyhow::Error>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("couchlink");
        match event_loop.create_window(attrs) {
            Ok(w) => {
                let window = Arc::new(w);
                match Renderer::new(window.clone()) {
                    Ok(r) => {
                        self.renderer = Some(r);
                        self.window = Some(window);
                    }
                    Err(e) => {
                        self.init_error = Some(e);
                        event_loop.exit();
                    }
                }
            }
            Err(e) => {
                self.init_error = Some(anyhow::anyhow!("window creation failed: {e}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let _ = self.shutdown_tx.send(());
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent {
                    physical_key: PhysicalKey::Code(code),
                    state,
                    logical_key,
                    ..
                },
                ..
            } => {
                if state == ElementState::Pressed && logical_key == Key::Named(NamedKey::Escape) {
                    let _ = self.shutdown_tx.send(());
                    event_loop.exit();
                    return;
                }
                if state == ElementState::Pressed && logical_key == Key::Named(NamedKey::F11) {
                    if let Some(w) = &self.window {
                        let fullscreen = w.fullscreen().is_some();
                        w.set_fullscreen(if fullscreen {
                            None
                        } else {
                            Some(winit::window::Fullscreen::Borderless(None))
                        });
                    }
                    return;
                }
                let mut kp = self.keyboard_pad.lock().unwrap();
                kp.set_key(code, state == ElementState::Pressed);
            }
            WindowEvent::RedrawRequested => {
                while let Ok(frame) = self.frame_rx.try_recv() {
                    if let Some(r) = &mut self.renderer {
                        r.upload_frame(&frame);
                    }
                }
                if let Some(r) = &mut self.renderer {
                    if let Err(e) = r.draw() {
                        tracing::warn!("draw error: {e}");
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Blocks the calling thread (must be the process main thread) running the
/// window until closed, Esc pressed, or window/GPU init fails.
pub fn run(
    frame_rx: Receiver<DecodedFrame>,
    keyboard_pad: Arc<Mutex<KeyboardPad>>,
    shutdown_tx: std::sync::mpsc::Sender<()>,
) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App {
        window: None,
        renderer: None,
        frame_rx,
        keyboard_pad,
        shutdown_tx,
        init_error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.init_error {
        return Err(e);
    }
    Ok(())
}
```

- [ ] **Step 2: Register the module**

Add to `crates/client/src/main.rs`: `mod view;`

- [ ] **Step 3: Build and fix API mismatches**

Run: `cargo build -p couchlink-client 2>&1 | tail -80`

`wgpu` 0.20 and `winit` 0.30 are both under active development; field/method names in `wgpu::SurfaceConfiguration`, `wgpu::DeviceDescriptor`, or the `ApplicationHandler` trait signature can shift slightly between patch releases. If it doesn't compile, check the installed version's actual struct definitions:

```bash
find ~/.cargo/registry/src -maxdepth 1 -iname "wgpu-0.20*" -o -iname "winit-0.30*"
grep -n "pub struct SurfaceConfiguration" -A 15 ~/.cargo/registry/src/*/wgpu-0.20*/src/lib.rs
grep -n "trait ApplicationHandler" -A 20 ~/.cargo/registry/src/*/winit-0.30*/src/application.rs
```

Fix field names/signatures to match, then rebuild. Do not skip this step — this file is the highest-risk one in the plan for API drift because `wgpu`/`winit` evolve faster than `openh264`/`rtp`.

Expected: builds clean, `view::run` is unused-but-compiling until Task 6 calls it (allow the dead-code warning for now, it disappears in Task 6).

- [ ] **Step 4: Commit**

```bash
git add crates/client/src/view.rs crates/client/src/main.rs
git commit -m "feat(client): wgpu+winit window rendering the decoded stream"
```

---

### Task 6: Wire it all together in `main.rs`

**Files:**
- Modify: `crates/client/src/main.rs` (full rewrite of the body)

**Interfaces:**
- Consumes: `webrtc_player::WebRtcPlayer::new` (Task 3, now returns a tuple), `decode::DecodedFrame` (Task 2), `keyboard_input::KeyboardPad` (Task 4), `view::run` (Task 5), existing `dualsense_reader::DualSenseReader`, existing `signaling_client::SignalingClient`.
- Produces: final `couchlink-client` binary behavior — see acceptance criteria below.

- [ ] **Step 1: Rewrite `main.rs`**

Replace the entire contents of `crates/client/src/main.rs` with:

```rust
mod decode;
mod dualsense_reader;
mod feedback_apply;
mod keyboard_input;
mod signaling_client;
mod view;
mod webrtc_player;

use anyhow::Result;
use clap::Parser;
use couchlink_proto::SignalMessage;
use keyboard_input::KeyboardPad;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use tracing::{info, warn};

#[derive(Parser, Debug, Clone)]
#[command(name = "couchlink-client", about = "Join a couchlink co-play session", version)]
struct Args {
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    session_id: String,
    #[arg(long, env = "COUCHLINK_PIN")]
    pin: String,
    /// Poll DualSense and send pad frames even without video decode UI.
    #[arg(long, default_value_t = true)]
    send_pad: bool,
    #[arg(long, env = "COUCHLINK_TURN_URL")]
    turn_url: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_USER")]
    turn_user: Option<String>,
    #[arg(long, env = "COUCHLINK_TURN_PASS")]
    turn_pass: Option<String>,
    /// Skip the video window entirely — pad-only, for automation/testing, or
    /// the automatic fallback when window/GPU creation fails.
    #[arg(long, default_value_t = false)]
    headless: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.headless {
        run_headless(args)
    } else {
        run_windowed(args)
    }
}

/// Today's exact pad-only behavior, unchanged, just renamed and made callable
/// as a fallback from `run_windowed`. Owns its own Tokio runtime.
fn run_headless(args: Args) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .try_init()
        .ok();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, None, None))
}

/// Opens the video window on this (main) thread; networking + decode + pad
/// polling run on a background thread with its own Tokio runtime. Falls back
/// to `run_headless` if window/GPU creation fails.
fn run_windowed(args: Args) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .try_init()
        .ok();

    let (frame_tx, frame_rx) = std_mpsc::channel::<decode::DecodedFrame>();
    let (shutdown_tx, shutdown_rx) = std_mpsc::channel::<()>();
    let keyboard_pad = Arc::new(Mutex::new(KeyboardPad::new()));

    let net_args = args.clone();
    let net_keyboard_pad = keyboard_pad.clone();
    let net_thread = std::thread::Builder::new()
        .name("couchlink-net".into())
        .spawn(move || -> Result<()> {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async_main(
                net_args,
                Some(frame_tx),
                Some((net_keyboard_pad, shutdown_rx)),
            ))
        })
        .expect("spawn network thread");

    match view::run(frame_rx, keyboard_pad, shutdown_tx) {
        Ok(()) => {}
        Err(e) => {
            warn!("windowed viewer failed ({e}), falling back to headless mode");
            // The network thread is mid-run against a now-abandoned frame_tx;
            // stop it and start clean in headless mode instead.
            drop(net_thread);
            return run_headless(args);
        }
    }

    let _ = net_thread.join();
    Ok(())
}

/// Shared networking core for both modes. In windowed mode, `video_frame_out`
/// forwards decoded frames to the window thread and `keyboard` supplies the
/// keyboard-derived pad state plus a shutdown signal from the window.
async fn async_main(
    args: Args,
    video_frame_out: Option<std_mpsc::Sender<decode::DecodedFrame>>,
    keyboard: Option<(Arc<Mutex<KeyboardPad>>, std_mpsc::Receiver<()>)>,
) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_player(args.session_id.clone(), args.pin.clone())
        .await?;

    let signal_out = signaling.outbound.clone();
    let (player, mut video_frames) = webrtc_player::WebRtcPlayer::new(
        signal_out.clone(),
        args.turn_url.clone(),
        args.turn_user.clone(),
        args.turn_pass.clone(),
    )
    .await?;

    let mut dualsense = if args.send_pad {
        dualsense_reader::DualSenseReader::open_first().ok()
    } else {
        None
    };
    if dualsense.is_none() && args.send_pad {
        info!("no DualSense found — keyboard input is still available in windowed mode");
    }

    let mut pad_interval = tokio::time::interval(std::time::Duration::from_millis(4)); // ~250 Hz
    let mut seq: u32 = 0;

    loop {
        // Non-blocking check for the window's shutdown signal (windowed mode only).
        if let Some((_, shutdown_rx)) = &keyboard {
            if shutdown_rx.try_recv().is_ok() {
                info!("window closed, shutting down network thread");
                break;
            }
        }

        tokio::select! {
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Offer { sdp }) => {
                        info!("got offer");
                        player.handle_offer(sdp, &signal_out).await?;
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = player.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::StreamInfo { width, height, fps, codec }) => {
                        info!("stream {width}x{height}@{fps} {codec}");
                    }
                    Some(SignalMessage::PeerLeft) => warn!("host left"),
                    None => break,
                    _ => {}
                }
            }
            frame = video_frames.recv() => {
                if let (Some(frame), Some(out)) = (frame, &video_frame_out) {
                    let _ = out.send(frame);
                }
            }
            _ = pad_interval.tick() => {
                seq = seq.wrapping_add(1);
                // DualSense takes priority for this tick when it produces a frame;
                // otherwise fall back to whatever the keyboard reports (which is
                // "neutral" if no keys are held, so this is always safe to send).
                let ds_frame = match dualsense.as_mut() {
                    Some(r) => r.read_frame().ok().flatten(),
                    None => None,
                };
                let frame = match ds_frame {
                    Some(f) => Some(f),
                    None => keyboard.as_ref().map(|(kp, _)| kp.lock().unwrap().to_pad_frame(seq)),
                };
                if let Some(frame) = frame {
                    if let Err(e) = player.send_pad(&frame).await {
                        warn!("send pad: {e}");
                    }
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p couchlink-client 2>&1 | tail -80`

Fix any remaining type mismatches (in particular double check `dualsense_reader::DualSenseReader::open_first()`'s real return type — it's used here as `.ok()` assuming it returns `Result<DualSenseReader, _>`; confirm against `crates/client/src/dualsense_reader.rs` before assuming, since the original code used `?` on it directly which suggests it's fallible but check whether "no DualSense connected" is `Err` or some other signal):

```bash
grep -n "pub fn open_first" -A 10 crates/client/src/dualsense_reader.rs
```

If `open_first()` doesn't return `Result`, adjust the `.ok()` call accordingly.

Expected: builds clean, no unused-import/dead-code warnings for `view`, `decode`, `keyboard_input` (all now actually used).

- [ ] **Step 3: Manual smoke test — headless mode (regression check)**

This confirms Task 6 didn't change the pre-existing pad-only behavior. Requires a running signaling server and host (or at minimum a signaling server to connect to):

```bash
./scripts/start-signaling.sh &
cargo run -p couchlink-client -- --headless --session-id test --pin 000000
```

Expected: connects to signaling, registers as player, logs "no DualSense found — keyboard input is still available in windowed mode" (since `--headless` still runs with keyboard=None, so this message won't print in headless — actually skip that expectation and just confirm it registers and waits without crashing).

- [ ] **Step 4: Manual smoke test — windowed mode**

```bash
cargo run -p couchlink-client -- --session-id test --pin 000000
```

Expected: a window titled "couchlink" opens. Without a host connected there's no video yet (black window), but it shouldn't crash. Press some WASD/Space keys — no visible effect without a host to receive pad frames, but confirm no panics in the terminal log. Close the window (or press Esc) — process exits cleanly within ~1s.

- [ ] **Step 5: Full end-to-end manual test (requires a host)**

On a Linux/WSL machine: `./scripts/run.sh host`. On this machine: `cargo run -p couchlink-client -- --session-id <printed> --pin <printed>`. Confirm:
- Video renders in the window.
- Pressing DualSense buttons (if connected) or WASD/Space/etc (if not) moves Player 2 in the emulator.
- Decode latency log lines appear in the terminal roughly every 2 seconds (`decode latency over 120 frames: p50=...ms p99=...ms`) — note the p50/p99 values here for the "measure before optimizing" follow-up decision.

- [ ] **Step 6: Commit**

```bash
git add crates/client/src/main.rs
git commit -m "feat(client): windowed video viewer with headless fallback, wired to decode+keyboard+dualsense"
```

---

### Task 7: Update docs and `run.ps1`

**Files:**
- Modify: `README.md`
- Modify: `docs/GETTING_STARTED.md`
- Modify: `scripts/run.ps1`

**Interfaces:** none (documentation only).

- [ ] **Step 1: Update `scripts/run.ps1`'s header comment**

In `scripts/run.ps1`, change the comment block at the top from:

```powershell
# Native Windows can run the friend/client role: it reads your local DualSense
# and sends CLPD pad frames to a host running elsewhere.
```

to:

```powershell
# Native Windows can run the friend/client role: it opens a window showing the
# host's video and reads your local DualSense (or keyboard, if no DualSense is
# connected) and sends CLPD pad frames to a host running elsewhere.
```

No functional changes — `couchlink-client.exe` already defaults to windowed mode after Task 6, so the existing invocation in `run.ps1` needs no argument changes.

- [ ] **Step 2: Update README**

In `README.md`, find the section describing the native client alternative and add a line noting the window + keyboard support. Locate the current text (added in an earlier change) that reads something like:

```
./scripts/run.sh client        # Linux / WSL / macOS
.\scripts\run.ps1 client       # native Windows (PowerShell)
```

and add directly beneath it:

```
The client opens a window showing the host's stream. Plug in a DualSense, or
just use the keyboard (WASD + arrows + Space/Shift/Ctrl/E/Q/R/1/2/Enter/Tab —
see `crates/client/src/keyboard_input.rs` for the full mapping).
```

- [ ] **Step 3: Update `docs/GETTING_STARTED.md`**

In the "Friend — native client (optional)" section, add a note after the existing code block:

```markdown
This opens a window with the video stream. If you don't have a DualSense
plugged in, the keyboard works as a fallback input (WASD + arrows for
movement, Space/Shift/Ctrl/E for face buttons, Q/R for bumpers, 1/2 for
triggers, Enter/Tab for Options/Create).
```

- [ ] **Step 4: Commit**

```bash
git add README.md docs/GETTING_STARTED.md scripts/run.ps1
git commit -m "docs: document native viewer window and keyboard input support"
```

---

## Self-review notes (for whoever executes this plan)

- Task 1's `rtp` version pin is the single highest-risk unknown in this plan — resolve it first and don't proceed to Task 3 until `cargo tree -p couchlink-client -i rtp` shows exactly one version in the graph.
- Tasks 2 and 4 have no external-API uncertainty (their dependencies' APIs were read directly from the installed crate source while writing this plan) and should need no deviation.
- Task 5 (`wgpu`/`winit`) is the second highest-risk task — budget extra time for API drift against whatever patch versions `cargo` actually resolves.
- Out of scope, tracked separately: hardware decode (only pursue if Task 6 Step 5's measured p50/p99 justifies it), multi-player (4-6 pads), SDL2 fallback implementation (documented in the design spec only), and any change to the host role.
