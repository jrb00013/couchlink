# Snappy input / FPS verify — 2026-11-15 (MUA / PCSX2)

Read-only check that the live CouchLink build did **not** regress friend input
response time or stream FPS during the Marvel Ultimate Alliance session.

## Method

- Host log: `/tmp/couchlink-stack-20261114-233920.log` (same process all session)
- Host binary built ~2026-11-14 23:39 from the stack restart that night
- 20‑minute sample window 13:48–14:06 local, plus full-log thirds
- Congestion-only friend signals: `client rtt ≥250ms` / loss governor info lines
  (absence ⇒ friends stayed under the lag threshold the host bothers to log)

## Results

| Window | FPS avg | Relay avg | Drop |
|--------|---------|-----------|------|
| Early baseline (samples 50–200) | 54.0 | 4.3 ms | 0% |
| Full session first third | 54.0 | 4.31 ms | ~0% |
| Full session last third | 55.3 | 4.11 ms | ~0.6% |
| 20‑min monitor | 55.2 | 4.2 ms | ~0.9% avg (brief peer blips, max ~10%) |

- `client rtt ≥250ms` lines: **0**
- Client loss / governor step-down lines: **0**
- Present path recovered to `webcodecs` after warmup

**Verdict: NO_REGRESSION / AS_GOOD_OR_BETTER** vs early baseline.

## Related

- Overlap restart fix (scripts only, cannot affect pad/WebRTC feel): PR #115
- Sacred host tests on that main tip: `input_photon` + `link_gov` — 35 passed, 0 failed
