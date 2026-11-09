# couchlink-host-panel

Native **egui** desktop panel for CouchLink hosts (see `docs/adr/2026-10-11-kaleidoscope-seats.md`).
Wraps the same shell scripts as manual ops and talks to the host split-seat control HTTP API on localhost.

## Run

From the repo root (set `COUCHLINK_ROOT` if the binary is not run from a checkout):

```bash
export COUCHLINK_ROOT=/path/to/couchlink   # optional if cwd/exe discovery finds scripts/
cargo run -p couchlink-host-panel
```

Release build:

```bash
cargo build --release -p couchlink-host-panel
./target/release/couchlink-host-panel
```

## Environment

| Variable | Purpose |
|----------|---------|
| `COUCHLINK_ROOT` | Repo root (must contain `scripts/`) |
| `COUCHLINK_SPLIT_SEAT_CTRL` | Split-seat HTTP base (default `127.0.0.1:18765`) |

## Safety

Restarting the stack kills the live host/signaling/capture session. The panel shows a confirmation dialog before running `scripts/restart-stack-for-game.sh`.
