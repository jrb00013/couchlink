# ADR: Kaleidoscope seats (split-seat oracle + host control panel)

**Status:** Accepted  
**Date:** 2026-10-11  
**Issues:** #85, #89–#94  
**Branch:** `feat/kaleidoscope-seats`

## Context

Friends on CouchLink often see the full local split-screen layout. They want only
their seat, with host opt-in, without regressing encode FPS, input latency, or
quality. A host control panel should replace convenience shell scripts.

## Decision

1. **Crop at draw time** on each client (normalized rects). Host still encodes once.
2. **Seat oracle** runs in a client Worker (heuristic seam + pad/mapping correlation);
   optional ONNX path trains offline and never touches the host encode thread.
3. **Host Control Panel** is a native Rust desktop app (`crates/host-panel`, egui)
   wrapping existing scripts and isolation master-enable — same language as the host,
   no Electron/Node required for host ops.
4. **Never** restart a live RPCS3/CouchLink session for development without host OK.

## Consequences

- Protocol adds `split_screen_set` / `split_screen_config` on the signaling WS with
  server-stamped slots (silent-drop risk if `ws.rs` arms are missing).
- Disabled isolation path must remain byte-identical to today's full-frame paint.
- Upscale is free (stretch crop to window), not a second encode / AI upscaler.

## Non-regression contract

| Path | Encode | Pad / input | Present |
|------|--------|-------------|---------|
| Master **off** (default) | unchanged (one host encode) | unchanged | `drawImage(frame,0,0)` / UV 0..1 |
| Master **on**, view **full** | same encode | unchanged | same full-frame paint |
| Master **on**, view **own** | same encode | unchanged | crop-at-draw only |

Seat oracle runs in a client Worker / timer — never on the host encode thread and
never on the pad DataChannel. Host FPS / relay ms must match master-off within
normal session noise when toggling master on a live stream.
