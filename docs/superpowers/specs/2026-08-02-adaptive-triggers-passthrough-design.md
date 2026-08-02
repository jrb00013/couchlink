# Adaptive triggers / DualSense output-report passthrough — Design

**Date:** 2026-08-02  
**Status:** Implementation  
**Branch:** `feat/adaptive-triggers-passthrough`

## Problem

Host → player feedback today is stubbed (`PadFeedback` JSON exists; client only logs).
Games that use DualSense rumble, lightbar, player LEDs, or **adaptive triggers** never
reach the friend's physical pad.

## Goals

1. Extend `PadFeedback` with structured adaptive-trigger effects and raw USB output
   report passthrough.
2. Pack DualSense USB output report `0x02` from feedback messages (rumble / lightbar /
   LEDs / triggers).
3. Native client: on pad-channel JSON from host, write the report to the open DualSense
   hidraw node.
4. Host: keep a handle on the pad DataChannel and expose `send_feedback` so adapters /
   tests / a future VHID companion can push effects.

## Non-goals

- Emulating adaptive triggers on ViGEm Xbox 360 / DS4 (not supported by those targets).
- Reading game HID output from Linux `uinput` (kernel cannot express DS adaptive triggers).
- Browser Gamepad haptic / vibration Actuator (can follow later).

## Protocol (JSON on `pad` channel, host → player)

Existing:

```json
{"type":"rumble","large":120,"small":40}
{"type":"lightbar","r":0,"g":0,"b":255}
{"type":"player_led","mask":1}
```

New:

```json
{"type":"adaptive_triggers","left_mode":1,"left_params":[0,200,0,0,0,0,0,0,0,0],"right_mode":2,"right_params":[10,40,180,0,0,0,0,0,0,0]}
{"type":"raw_output","report":[2,255,241,40,120,0,...]}
```

`raw_output.report` is the full USB HID buffer (report id `0x02` first). Prefer this when
a companion DualSense VHID forwards the exact bytes the game wrote.

## Client apply path

```
pad DC on_message (string) → PadFeedback → build USB report → DualSenseReader.write_output
```

Xbox / keyboard clients ignore DualSense-only effects (best-effort no-op).

## Success

- Unit tests pack rumble + adaptive trigger bytes at known DualSense offsets.
- Client applies feedback without panicking when no DualSense is open.
- Host `send_feedback` sends JSON on the live pad channel.
