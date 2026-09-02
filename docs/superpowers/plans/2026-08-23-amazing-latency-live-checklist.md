# Live amazing-latency checklist (PR #48)

Friend-night measurement card. **Bitrate/1080 climb is blocked** until MATH-2
passes live. Each player reads **debug → Latency** on *their* browser.

## Unit gates (CI — must be green before invite)

```bash
cargo test -p couchlink-host input_photon_budget ricardo_playable_ab amazing_latency_ab
cargo test -p couchlink-host
cd web && npx vitest run
```

## Live gates

| ID | What | Pass |
|---|---|---|
| MATH-1 | Φ\* at R=48 | 93ms (unit-locked) |
| MATH-2 | S_p50 = Φ_p50 − RTT | ≤ 45ms wow (stretch 30 after handoff proof) |
| MATH-3 | Chrome present | `webcodecs` &lt;3s |
| MATH-4 | Host log `SHM_*` | document trip **or** skip; no SHM body until trip |
| MATH-5 | Sacred | `ricardo_playable_ab` 7/7; no IDR death spiral |
| MATH-6 | Drawer | Latency tab shows Φ / S — not push as hero |
| AMAZE-1 | Friend drawer | photon p50 ≤ RTT+45 |
| AMAZE-2 | Present path | webcodecs &lt;3s Chrome |
| AMAZE-3 | Keyframes | no 1Hz IDR storm |
| AMAZE-4 | Units | host + web green |
| AMAZE-5 | Handoff | wait_p95 + omega in host log; SHM only if gate |

## Host log scrape (every ~5s on Hyper-V path)

Look for:

```text
handoff wait=…ms copy=…ms wait_p95=…ms omega=…
SHM_SKIP — wait p95 not material; keep hyperv/tcp
# or
SHM_GATE_TRIP — implement shm ring (COUCHLINK_CAPTURE_IPC=shm)
```

If `SHM_SKIP` and frames received ≈ pushed → **do not implement SHM**; paste
proof on the PR.

## Per-player Latency tab

- Φ last / Φ p50 / S p50 / RTT / wow rows
- Present mode + stuck reason
- Host pipeline is **shared**; interactive metrics are **local**
