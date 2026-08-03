# DualSense VHID companion — Implementation Plan

> **For agentic workers:** implement task-by-task; commit after each green test chunk.

**Spec:** `docs/superpowers/specs/2026-08-02-dualsense-vhid-companion-design.md`  
**Branch:** `feat/dualsense-vhid-companion`

## File map

| Path | Responsibility |
|------|----------------|
| `crates/pad/src/vhid_proto.rs` | DSVH/DSVO encode/decode + tests |
| `crates/pad/src/linux_uhid.rs` | Linux `/dev/uhid` DualSense + output poll |
| `crates/pad/src/windows_pad.rs` | Pipe/TCP client; read DSVO |
| `crates/pad/src/virtual_pad.rs` | Linux Auto: UHID → uinput; output callback hook |
| `crates/ds-vhid/` | Windows companion binary (pipe+TCP server, ViGEm DS4 backend) |
| `crates/host/src/webrtc_peer.rs` | Poll virtual-pad outputs → `send_feedback` |
| `docs/EMULATORS.md` / README | Install companion; P1 vs P2 note |

## Tasks

1. Protocol module + tests ✅  
2. Linux UHID DualSense (+ fallback uinput) ✅  
3. Extend Windows DualSenseVhid client (TCP + DSVO read) ✅  
4. `couchlink-ds-vhid` companion crate (Windows) ✅ — WinUHid Auto + ViGEm DS4/Xbox360  
5. Host feedback pump ✅  
6. Docs + CI workspace member ✅  
