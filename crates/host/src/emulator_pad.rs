//! Match the virtual pad and the emulator binding to the controller the player
//! is actually holding.
//!
//! The browser Gamepad API normalises every pad, so `PadFrame` looks identical
//! whether the player holds an Xbox pad or a DualSense. The player therefore
//! announces its family over signaling (`PadInfo`), and this module reconciles
//! two things against it: which virtual device the companion presents, and
//! which device the emulator has bound to that player slot. Getting either
//! wrong drops every button with no error anywhere — the failure is silent on
//! the host, in the browser, and in the emulator.

use std::path::PathBuf;
use std::process::Command;

use tracing::{info, warn};

/// Pad family used the moment a player sits down, before the browser has
/// announced a real controller. Must stay a kind `backend_for` maps to
/// `xbox360` — that is the only backend with a working PCSX2 auto-link.
pub const JOIN_PAD_KIND: &str = "generic";

/// Virtual-pad backend and emulator handler for a reported controller family.
///
/// Always XInput/`xbox360`, regardless of the family the player reports.
///
/// This used to route real DualSense/DualShock4 hardware to the `ds4` ViGEm
/// backend (SDL-shaped) to preserve controller identity/icons. That backend
/// has no working PCSX2 auto-link: `link-emulator-pad.sh` binds PCSX2 by
/// naming an `SDL-<n>` device index, and that index depends on every
/// SDL-visible device's connect order on the *host's* machine — something
/// this side cannot predict or read back from a running PCSX2, so it has
/// always just skipped PCSX2 entirely for `ds4` and stayed silent about it
/// (`pcsx2: "skipped"` in the RESULT log line, with no error to the player or
/// the host). Live-reproduced 2026-08-22: a friend joining with a real
/// controller got a `ds4` backend, connected fine, showed up in the couchlink
/// UI and in PCSX2's own controller settings, and simply had no input in the
/// running game because Pad<N> was never bound at all.
///
/// `xbox360`/XInput is the one backend both RPCS3 and PCSX2 auto-link
/// reliably (see `link-emulator-pad.sh`), so every kind maps to it now. The
/// cost is losing DualSense-specific button icons/identity in games that
/// care; the alternative was a player whose controller silently never works.
fn backend_for(_kind: &str) -> &'static str {
    "xbox360"
}

/// Repo root, so the helper scripts can be found from a binary in `target/`.
fn repo_root() -> Option<PathBuf> {
    if let Ok(r) = std::env::var("COUCHLINK_ROOT") {
        let p = PathBuf::from(r);
        if p.join("scripts").is_dir() {
            return Some(p);
        }
    }
    // target/release/couchlink-host -> repo root
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?.parent()?.parent()?.to_path_buf();
    root.join("scripts").is_dir().then_some(root)
}

/// Remote player slots couchlink can seat. Mirrors
/// `couchlink_signaling::players::MAX_PLAYERS`; the host crate does not depend
/// on the signaling crate, so the value is restated rather than imported.
pub const MAX_REMOTE_SLOTS: u8 = 3;

/// Emulator player port driven by couchlink remote `slot` (1-based).
///
/// The host's own physical pad owns emulator player 1, so remote slot 1 is
/// emulator player 2, and so on. Written once here rather than as a bare
/// `slot + 1` at each call site: the join path, the PadInfo path and the
/// pre-bind path must agree exactly, and a slot bound to the wrong player
/// fails silently — the pad works everywhere observable and simply does
/// nothing in the game.
pub fn emulator_player_for(slot: u8) -> u8 {
    slot + 1
}

/// Every remote slot a session can seat, in bind order.
fn prebind_slots() -> impl Iterator<Item = u8> {
    1..=MAX_REMOTE_SLOTS
}

/// Write every remote slot's emulator binding once, before anyone connects.
///
/// PCSX2 reads `PCSX2.ini` exactly once, at launch: an edit made while it is
/// running is ignored, and it rewrites the file from memory on exit. Binding a
/// slot only when its player joins therefore forces a brittle order on the
/// host — every player has to be seated *before* PCSX2 starts, or their pad is
/// simply absent for the entire session, and relaunching PCSX2 to pick up a
/// late joiner discards whatever was written in the meantime.
///
/// Nothing about writing the binding actually needs a connected player: the
/// slot -> device mapping is fixed (slot 1 -> XInput-0 -> port 1B, slot 2 ->
/// XInput-1 -> 1C, slot 3 -> XInput-2 -> 1D), and a binding whose XInput
/// device never shows up is inert — PCSX2 just sees no input on that port. So
/// write them all up front and let PCSX2 be started whenever, in any order.
///
/// Best-effort like the rest of this module: a failure here leaves the
/// per-join `apply` path as the fallback it always was.
pub fn prebind_all() {
    let Some(root) = repo_root() else {
        warn!("repo root not found — cannot pre-bind emulator pads");
        return;
    };
    let backend = backend_for(JOIN_PAD_KIND);
    // Companion first so the virtual pads exist as early as possible; PCSX2
    // hot-plugs devices, but a device already present at launch is one less
    // thing depending on that.
    run(&root, "scripts/ensure-ds-vhid.sh", backend, None);
    for slot in prebind_slots() {
        run(
            &root,
            "scripts/link-emulator-pad.sh",
            backend,
            Some(emulator_player_for(slot)),
        );
    }
    info!(
        "pre-bound {MAX_REMOTE_SLOTS} emulator pad slot(s) — PCSX2 can be started before or after players join"
    );
}

/// Bind this slot's virtual pad into the emulator as soon as the player joins.
///
/// PadInfo used to be the only trigger, so a seated player who had not yet
/// sent a keystroke never got a PCSX2 section at all. Join creates the
/// xbox360/XInput target and writes the matching Pad3/4/5 slot; a later
/// PadInfo with a different family can still re-run `apply`.
pub fn apply_on_join(slot: u8) {
    apply(JOIN_PAD_KIND, "join", slot);
}

/// Reconcile the virtual pad + emulator binding with `kind` for `slot`.
///
/// `slot` is the couchlink player slot (1-based). The host's own physical pad
/// owns emulator P1, so remote slot `s` drives the emulator's P`slot + 1`.
///
/// Best-effort by design: every failure here still leaves video streaming, so
/// nothing in this path is allowed to take the session down.
pub fn apply(kind: &str, id: &str, slot: u8) {
    let backend = backend_for(kind);
    let Some(root) = repo_root() else {
        warn!("pad_info {kind}: repo root not found — emulator binding unchanged");
        return;
    };

    info!(
        "player pad is {kind} ({id}) for emulator P{} — virtual pad backend {backend}",
        emulator_player_for(slot)
    );

    // Companion first: the emulator binding names the device the companion
    // presents, so rebinding before it exists would point at nothing. The
    // companion is a single process that presents every slot's pad.
    run(&root, "scripts/ensure-ds-vhid.sh", backend, None);
    // Each slot binds a different emulator player port so a second/third
    // controller never overwrites an already-seated player's binding.
    run(
        &root,
        "scripts/link-emulator-pad.sh",
        backend,
        Some(emulator_player_for(slot)),
    );
}

fn run(root: &PathBuf, script: &str, backend: &str, emulator_player: Option<u8>) {
    let path = root.join(script);
    if !path.is_file() {
        warn!("{script} missing — skipped");
        return;
    }
    let mut cmd = Command::new("bash");
    cmd.arg(&path)
        .current_dir(root)
        .env("COUCHLINK_DS_VHID_BACKEND", backend);
    if let Some(player) = emulator_player {
        cmd.env("COUCHLINK_EMU_PLAYER", player.to_string());
    }
    match cmd.output() {
        Ok(out) if out.status.success() => {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if !line.trim().is_empty() {
                    info!("{}", line.trim());
                }
            }
        }
        Ok(out) => warn!(
            "{script} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => warn!("{script} failed to run: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbox_and_generic_use_xinput_backend() {
        assert_eq!(backend_for("xbox"), "xbox360");
        // Generic pads must not land on a Sony backend: XInput is the handler
        // that exists without a vendor driver.
        assert_eq!(backend_for("generic"), "xbox360");
        assert_eq!(backend_for("something-new"), "xbox360");
    }

    /// Regression guard for the 2026-08-22 session: a player reporting a real
    /// DualSense/DualShock4 controller used to route to the `ds4` ViGEm
    /// backend, which `link-emulator-pad.sh` cannot auto-bind in PCSX2 (no
    /// predictable SDL device index) — so that player's pad registered
    /// everywhere (couchlink UI, PCSX2's own controller list) except the
    /// actual running game, with no error surfaced anywhere. Every kind must
    /// resolve to `xbox360` so PCSX2 auto-linking never gets silently skipped
    /// again. If this test ever needs to change, `link-emulator-pad.sh`'s
    /// PCSX2 path needs a real SDL-index-discovery fix FIRST, not this.
    #[test]
    fn every_known_pad_kind_uses_the_pcsx2_compatible_backend() {
        for kind in ["dualsense", "dualshock4", "ds4", "xbox", "generic", "something-new"] {
            assert_eq!(
                backend_for(kind),
                "xbox360",
                "kind {kind:?} must map to xbox360 — ds4 has no working PCSX2 auto-link"
            );
        }
    }

    /// The host's own pad owns emulator player 1, so remote slots start at 2.
    /// Binding a slot to the wrong player is a silent failure — the pad works
    /// everywhere it can be observed and does nothing in the game — so pin the
    /// mapping rather than trusting four separate `slot + 1` call sites.
    #[test]
    fn remote_slots_map_past_the_hosts_own_player_1() {
        assert_eq!(emulator_player_for(1), 2);
        assert_eq!(emulator_player_for(2), 3);
        assert_eq!(emulator_player_for(3), 4);
        // Never player 1: that is the host's own controller, and overwriting it
        // would unbind the person running the session.
        for slot in prebind_slots() {
            assert!(
                emulator_player_for(slot) >= 2,
                "slot {slot} must not claim emulator player 1 (the host's own pad)"
            );
        }
    }

    /// `prebind_all` exists to remove an ordering constraint: PCSX2 reads its
    /// ini only at launch, so a slot bound later than PCSX2's start is absent
    /// for the whole session. That only holds if pre-binding covers *every*
    /// seatable slot — one gap and that player is silently unplayable, which is
    /// exactly the bug it was written to prevent.
    #[test]
    fn prebind_covers_every_seatable_slot_exactly_once() {
        let slots: Vec<u8> = prebind_slots().collect();
        assert_eq!(slots, vec![1, 2, 3], "must cover slots 1..=MAX_REMOTE_SLOTS");
        assert_eq!(slots.len(), MAX_REMOTE_SLOTS as usize);

        let players: Vec<u8> = slots.iter().copied().map(emulator_player_for).collect();
        assert_eq!(players, vec![2, 3, 4], "emulator players 2-4, host keeps 1");

        // No slot may pre-bind onto another slot's player port: a duplicate
        // would have two players sharing one pad, which reads as "my input
        // moves someone else's character".
        let mut seen = players.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), players.len(), "duplicate emulator player port");
    }

    /// The join path and the pre-bind path write the same config file. If they
    /// ever disagreed about a slot's player port, whichever ran last would
    /// silently move that player mid-session.
    #[test]
    fn prebind_and_join_agree_on_every_slot() {
        // Both paths route through `emulator_player_for`, so the port cannot
        // diverge by construction — the live risk is the *backend*: pre-bind
        // uses JOIN_PAD_KIND, while a later PadInfo carries whatever the
        // browser reports. If those mapped to different backends, the second
        // run would rewrite the slot for a different device and silently
        // unbind the player mid-session.
        for kind in ["generic", "dualsense", "dualshock4", "ds4", "xbox"] {
            assert_eq!(
                backend_for(JOIN_PAD_KIND),
                backend_for(kind),
                "a later {kind:?} PadInfo must not swap the backend pre-bind wrote"
            );
        }
    }

    /// `MAX_REMOTE_SLOTS` restates `couchlink_signaling::players::MAX_PLAYERS`
    /// because the host crate does not depend on the signaling crate. A silent
    /// drift would under-bind (a seated player with no pad) or over-bind, so
    /// check it against the actual source of truth rather than a comment.
    #[test]
    fn max_remote_slots_matches_signaling() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../signaling/src/players.rs"),
        )
        .expect("read signaling players.rs");
        let line = src
            .lines()
            .find(|l| l.contains("pub const MAX_PLAYERS"))
            .expect("MAX_PLAYERS declaration");
        let value: u8 = line
            .rsplit('=')
            .next()
            .and_then(|v| v.trim().trim_end_matches(';').parse().ok())
            .expect("parse MAX_PLAYERS");
        assert_eq!(
            MAX_REMOTE_SLOTS, value,
            "MAX_REMOTE_SLOTS drifted from signaling's MAX_PLAYERS"
        );
    }

    /// Regression: pads used to bind only on PadInfo (first keypress), so a
    /// seated player with no announce never got a PCSX2 slot. Join must use a
    /// kind that already maps to the xbox360 backend.
    #[test]
    fn join_binds_without_waiting_for_pad_info() {
        assert_eq!(JOIN_PAD_KIND, "generic");
        assert_eq!(
            backend_for(JOIN_PAD_KIND),
            "xbox360",
            "join-time bind must use the PCSX2-compatible backend"
        );
    }
}
