# Native video viewer for couchlink-client

## Problem

Today `couchlink-client` only reads the local DualSense and sends `CLPD` pad
frames — it has no video code at all. The only way to *see* the host's stream
is the browser player (`web/`). We want a native, non-browser window for the
friend that both displays the stream and sends pad input, so joining feels
like launching a game client rather than opening a tab.

The host role stays exactly as it is today (CLI via `run.sh host`) — no
change there.

## Goals

- Friend runs one native binary that shows the host's video and sends their
  DualSense input, on Linux, WSL (with a display), macOS, and native Windows.
- No new heavy system dependency (no ffmpeg install step) — keep `install.sh`
  as simple as it is now.
- Ship the simplest pipeline that could work, instrumented to measure itself,
  rather than pre-optimizing for hardware decode before there's evidence it's
  needed.

## Non-goals (this iteration)

- Hardware-accelerated decode (VideoToolbox/DXVA/VAAPI). Only pursued later
  if measured decode latency justifies it (see Performance below).
- Changing the host role/UX — it remains the CLI started by `run.sh host`.
- SDL2 implementation — documented as a fallback option only (see below),
  not built now.

## Pipeline

```
Desktop (host)
  → capture (existing)
  → OpenH264 encode (existing, crates/host/src/encode.rs)
  → network (existing WebRTC/SRTP)
  → OpenH264 decode (NEW, crates/client/src/decode.rs)
  → wgpu render (NEW, crates/client/src/view.rs)
  → input (existing, crates/client/src/dualsense_reader.rs, unchanged)
```

Decode uses the `openh264` crate — the same crate the host already uses to
encode, so the dependency is already in the workspace's dependency tree
(`crates/host/Cargo.toml`). This avoids adding ffmpeg/libavcodec as a system
dependency on any of the four target platforms.

## Components

### `crates/client/src/decode.rs` (new)

- Wraps `openh264::decoder::Decoder`.
- Receives Annex-B H.264 NALs from the WebRTC video track (via
  `webrtc_player.rs`, mirroring how `host/src/encode.rs` already produces
  Annex-B NALs on the encode side).
- Emits `DecodedFrame { y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, width: u32, height: u32 }`
  over an `mpsc::UnboundedSender`, consumed by `view.rs`.
- Runs on its own thread/task so decode never blocks the WebRTC receive loop
  or the render loop.
- **Instrumentation (built in, not a follow-up):** times every
  `decoder.decode()` call and logs p50/p99 decode latency every N frames via
  `tracing`, the same crate used for logging elsewhere in the codebase. This
  is how we decide later whether hardware decode is worth it — per the
  measure-before-optimizing principle: ~2-5ms/frame means software decode is
  fine as-is; ~12-18ms/frame is the trigger to revisit with hardware decode.
- Decoder errors (corrupt NAL, mid-stream keyframe loss) are logged and the
  frame is dropped — video glitches until the next keyframe rather than the
  client crashing. This matches how real-time video players behave under
  packet loss.

### `crates/client/src/view.rs` (new)

- `winit` event loop + `wgpu` surface.
- Three `R8Unorm` textures (Y, U, V planes), updated from each `DecodedFrame`.
- A fragment shader does BT.709 YUV→RGB conversion on the GPU (avoids a
  per-frame CPU pixel-conversion loop).
- Presents as soon as a frame is decoded — no artificial vsync cap, matching
  the project's existing low-latency posture (GCC congestion control + the
  motion-adaptive idle FPS already implemented on the host).
- `F` / `F11` toggles borderless fullscreen.
- Window close / Esc triggers a clean shutdown: signaling disconnect, decoder
  thread stopped, process exits.
- **Window/GPU creation failure fallback:** if `winit`/`wgpu` can't create a
  window or surface (e.g. accidentally run on a headless server, or WSL
  without WSLg/X), log a warning and fall back to the existing headless
  pad-only behavior rather than hard-erroring — a friend running this in the
  wrong environment still gets *something* working (pad input) instead of a
  crash.

### `crates/client/src/main.rs` (modified)

- New `--headless` flag (default `false`). When set, skips `view.rs`
  entirely and behaves exactly as `couchlink-client` does today
  (pad-send only) — used for automation/testing/CI and the fallback path
  above.
- Everything else (signaling connect, pad reading/sending via
  `dualsense_reader.rs`) is unchanged.

### `scripts/run.ps1` (modified)

- The `client` role now launches the windowed client instead of the
  (previously only option) headless one.

## Data flow summary

Inbound (new): WebRTC video track → `decode.rs` (background thread, openh264)
→ channel → `view.rs` render loop (textures + draw call).

Outbound (unchanged): `dualsense_reader.rs` → channel → `signaling_client.rs`
→ WebRTC `pad` data channel.

These two flows are independent — the window only owns display + a couple of
UI keybinds (fullscreen/quit), not gamepad polling, so there's no new
coupling between the video and input paths beyond sharing one process.

## Testing

- `decode.rs` unit tests: feed known-good Annex-B NAL sequences (can reuse
  fixtures from the host's existing encode tests, or generate them by
  round-tripping through the host encoder in a test) and assert frames come
  out with expected dimensions and no decode errors.
- Manual verification: run `run.sh host` on one machine, `couchlink-client`
  (windowed, default) on another, confirm video renders and pad input
  reaches the host's virtual DualSense — same manual check used for the
  existing browser player.
- `--headless` mode gets no new automated test beyond what already covers
  `couchlink-client` today, since its behavior is unchanged.

## Documented fallback (not implemented): SDL2

If `wgpu`/`winit` ever proves too heavy or fragile on a target platform
(e.g. GPU passthrough issues under WSLg), an SDL2-based viewer is a viable
Plan B and simpler in one respect: `sdl2::render::Texture` has native
`YV12`/`IYUV` texture support, so the YUV upload works without a hand-written
shader — SDL2 does the YUV→RGB conversion internally. The trade-off is a
system dependency (`libsdl2-dev` / `brew install sdl2`) that `wgpu` doesn't
need, which is why it's not the default. This would replace `view.rs` only;
`decode.rs` stays identical since decode is orthogonal to the rendering
backend.

## Rollout / risk

- Additive: existing headless behavior is preserved via `--headless`, so
  nothing that currently works can regress.
- Main risk is `wgpu`/`winit` surface creation quirks across four platforms
  (especially WSL without WSLg) — mitigated by the fallback-to-headless
  behavior above rather than trying to solve every windowing edge case
  upfront.
