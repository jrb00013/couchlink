# Kaleidoscope seats — implementation plan

Extends `2026-10-04-split-screen-isolation.md` with host panel + seat oracle.

## Phases

| Phase | Deliverable | Issues |
|-------|-------------|--------|
| 1 | ADR + this plan | — |
| 2 | Proto + `ws.rs` + host master state | #90 |
| 3 | Web/native crop + toggles | #92 |
| 4 | Oracle v0 Worker | #91 |
| 5 | Host panel (egui) | #89 |
| 6 | Labeling harness + ONNX stub | #93 #94 |
| 7 | PR — live test only after host OK | — |

## Session safety

Do not kill RPCS3, win-capture, or the live CouchLink stack unless the host
explicitly OK's a test window.
