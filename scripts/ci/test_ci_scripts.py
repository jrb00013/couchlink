#!/usr/bin/env python3
"""Unit tests for regression-guard.py and check-weakened-assertions.py.

    python3 -m unittest scripts/ci/test_ci_scripts.py -v   (or: python3 scripts/ci/test_ci_scripts.py)
"""
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def load(name, fname):
    spec = importlib.util.spec_from_file_location(name, HERE / fname)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


guard = load("guard", "regression-guard.py")
weak = load("weak", "check-weakened-assertions.py")

PATH = "crates/host/src/ricardo_playable_ab.rs"


def d(rem, add, start=10):
    """Build a minimal -U0 diff."""
    out = [f"@@ -{start},{len(rem)} +{start},{len(add)} @@"]
    out += ["-" + r for r in rem] + ["+" + a for a in add]
    return "\n".join(out)


class WeakenedDiff(unittest.TestCase):
    def check(self, rem, add):
        return weak.check_diff(PATH, d(rem, add), "")

    def test_lowered_ge_flagged(self):
        self.assertTrue(self.check(["        assert!(floor.fps >= 45);"], ["        assert!(floor.fps >= 30);"]))

    def test_raised_ge_ok(self):
        self.assertFalse(self.check(["        assert!(floor.fps >= 45);"], ["        assert!(floor.fps >= 50);"]))

    def test_raised_le_flagged(self):
        self.assertTrue(self.check(["    assert!(shed <= 3);"], ["    assert!(shed <= 8);"]))

    def test_lowered_le_ok(self):
        self.assertFalse(self.check(["    assert!(shed <= 3);"], ["    assert!(shed <= 1);"]))

    def test_message_operator_ignored(self):
        self.assertTrue(self.check(['    assert!(x >= 45, "needs <= wow");'], ['    assert!(x >= 40, "needs <= wow");']))

    def test_eq_change_flagged(self):
        self.assertTrue(self.check(["    assert_eq!(p.bitrate_kbps, 10_000);"], ["    assert_eq!(p.bitrate_kbps, 5_000);"]))

    def test_min_const_lowered_flagged_and_raised_ok(self):
        self.assertTrue(self.check(["    pub const MIN_PUSH_FPS: f64 = 90.0;"], ["    pub const MIN_PUSH_FPS: f64 = 80.0;"]))
        self.assertFalse(self.check(["    pub const MIN_PUSH_FPS: f64 = 90.0;"], ["    pub const MIN_PUSH_FPS: f64 = 95.0;"]))

    def test_max_const_raised_flagged(self):
        self.assertTrue(self.check(["    pub const MAX_SHED_PCT: u32 = 1;"], ["    pub const MAX_SHED_PCT: u32 = 5;"]))

    def test_assertion_removed_flagged(self):
        self.assertTrue(self.check(["    assert!(floor.fps >= 45);"], []))

    def test_comment_only_change_ok(self):
        self.assertFalse(self.check(["    // was 5"], ["    // now 6"]))

    def test_unrelated_number_in_non_assert_ok(self):
        self.assertFalse(self.check(["    let n = 3;"], ["    let n = 4;"]))

    def test_link_gov_non_test_region_ignored(self):
        text = "const DOWN_TRIGGER_PCT: u32 = 8;\n" * 3 + "#[cfg(test)]\nmod tests {}\n"
        diff = d(["const DOWN_TRIGGER_PCT: u32 = 8;"], ["const DOWN_TRIGGER_PCT: u32 = 20;"], start=2)
        self.assertEqual(weak.check_diff("crates/host/src/link_gov.rs", diff, text), [])


class LiveTests(unittest.TestCase):
    def test_ignore_and_missing_attr(self):
        src = "#[test]\nfn a() {}\n#[test]\n#[ignore]\nfn b() {}\nfn c() {}\n#[tokio::test]\nasync fn d() {}\n"
        self.assertEqual(weak.live_tests(src), {"a", "d"})


class GitEndToEnd(unittest.TestCase):
    """Real git repo: base commit + head commit, run the script as CI would."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.f = self.tmp / PATH
        self.f.parent.mkdir(parents=True)
        self.f.write_text(
            "#[cfg(test)]\nmod t {\n    #[test]\n    fn sacred() {\n        assert!(fps >= 45);\n    }\n}\n"
        )
        self.git("init", "-q", "-b", "main")
        self.git("add", "-A")
        self.git("commit", "-qm", "base")

    def git(self, *a):
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@t")
        subprocess.run(["git", *a], cwd=self.tmp, check=True, env=env, capture_output=True)

    def run_check(self, body=""):
        env = dict(os.environ, PR_BODY=body)
        return subprocess.run(
            [sys.executable, str(HERE / "check-weakened-assertions.py"), "--base", "main", "--head", "HEAD"],
            cwd=self.tmp, env=env, capture_output=True, text=True,
        )

    def commit(self, msg="change"):
        self.git("add", "-A")
        self.git("commit", "-qm", msg)

    def start_branch(self):
        self.git("checkout", "-qb", "pr")

    def test_untouched_passes(self):
        self.start_branch()
        self.assertEqual(self.run_check().returncode, 0)

    def test_loosened_fails_then_marker_in_body_passes(self):
        self.start_branch()
        self.f.write_text(self.f.read_text().replace(">= 45", ">= 30"))
        self.commit()
        r = self.run_check()
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("lower bound lowered", r.stdout)
        ok = self.run_check("desc\nregression-baseline-change: intentional test\n")
        self.assertEqual(ok.returncode, 0, ok.stdout)

    def test_marker_in_commit_message_passes(self):
        self.start_branch()
        self.f.write_text(self.f.read_text().replace(">= 45", ">= 30"))
        self.commit("x\n\nregression-baseline-change: because")
        self.assertEqual(self.run_check().returncode, 0)

    def test_removed_test_fails(self):
        self.start_branch()
        self.f.write_text("#[cfg(test)]\nmod t {}\n")
        self.commit()
        r = self.run_check()
        self.assertEqual(r.returncode, 1)
        self.assertIn("sacred", r.stdout)

    def test_renamed_test_fails(self):
        self.start_branch()
        self.f.write_text(self.f.read_text().replace("fn sacred", "fn sacred2"))
        self.commit()
        self.assertEqual(self.run_check().returncode, 1)

    def test_tightened_passes(self):
        self.start_branch()
        self.f.write_text(self.f.read_text().replace(">= 45", ">= 60"))
        self.commit()
        self.assertEqual(self.run_check().returncode, 0)

    def test_out_of_scope_file_ignored(self):
        self.start_branch()
        (self.tmp / "other.rs").write_text("assert!(x >= 1);")
        self.commit()
        self.assertEqual(self.run_check().returncode, 0)


class GuardOnRepo(unittest.TestCase):
    def tree(self):
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, True)
        for sub in ("crates", "ci", "web"):
            shutil.copytree(ROOT / sub, tmp / sub, ignore=shutil.ignore_patterns("node_modules", "target", "dist"))
        return tmp

    def run_guard(self, tmp):
        return guard.run(tmp, tmp / "ci" / "regression-baseline.toml")[0]

    def mutate(self, tmp, rel, old, new):
        p = tmp / rel
        s = p.read_text()
        self.assertIn(old, s, f"mutation anchor missing in {rel}")
        p.write_text(s.replace(old, new, 1))

    def test_main_is_clean(self):
        self.assertEqual(guard.run(ROOT, ROOT / "ci" / "regression-baseline.toml")[0], [])

    def test_bitrate_drift(self):
        t = self.tree()
        self.mutate(t, "crates/proto/src/signal.rs", "bitrate_kbps: 10_000,\n    };\n    pub const P720_30", "bitrate_kbps: 5_000,\n    };\n    pub const P720_30")
        self.assertTrue(any("baseline-bitrate-kbps" in f for f in self.run_guard(t)))

    def test_idr_ceiling_drift(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/webrtc_peer.rs", "256 * 1024", "24 * 1024")
        self.assertTrue(any("idr-ceiling" in f for f in self.run_guard(t)))

    def test_frame_stale_drift(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/capture/bridge.rs", "Duration::from_secs(4)", "Duration::from_millis(500)")
        self.assertTrue(any("frame-stale-bridge" in f for f in self.run_guard(t)))

    def test_governor_trigger_drift(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/link_gov.rs", "DOWN_TRIGGER_PCT: u32 = 8", "DOWN_TRIGGER_PCT: u32 = 2")
        self.assertTrue(any("gov-down-trigger" in f for f in self.run_guard(t)))

    def test_pli_ignore_removed(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/webrtc_peer.rs", "if send_rtp && send_dc {\n                            continue;", "if false {\n                            continue;")
        self.assertTrue(any("rtcp-pli-ignored" in f for f in self.run_guard(t)))

    def test_sacred_renamed(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/ricardo_playable_ab.rs", "fn b_healthy_hybrid_keeps_full_rtp_for_paint", "fn b_healthy_hybrid_renamed")
        self.assertTrue(any("b_healthy_hybrid_keeps_full_rtp_for_paint" in f for f in self.run_guard(t)))

    def test_sacred_ignored(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/ricardo_playable_ab.rs", "    #[test]\n    fn b_live_sim_target_beats_ricardo_on_all_axes", "    #[test]\n    #[ignore]\n    fn b_live_sim_target_beats_ricardo_on_all_axes")
        self.assertTrue(any("b_live_sim_target" in f for f in self.run_guard(t)))

    def test_missing_file(self):
        t = self.tree()
        (t / "crates/host/src/link_gov.rs").unlink()
        self.assertTrue(any("missing-file" in f for f in self.run_guard(t)))

    def test_fec_enabled_with_rtp(self):
        t = self.tree()
        self.mutate(t, "crates/host/src/webrtc_peer.rs", "self.fec_enabled && path == PATH_WEBCODECS && !send_rtp", "self.fec_enabled && path == PATH_WEBCODECS")
        self.assertTrue(any("no-fec-while-rtp-live" in f for f in self.run_guard(t)))

    def test_test_log_requires_sacred_ran(self):
        import tomllib
        cfg = tomllib.loads((ROOT / "ci" / "regression-baseline.toml").read_text())
        names = [t for c in cfg["sacred"] for t in c["tests"]]
        good = "\n".join(f"test mod::{t} ... ok" for t in names)
        self.assertEqual(guard.check_test_log(cfg, good)[0], [])
        bad = good.replace(f"test mod::{names[0]} ... ok", f"test mod::{names[0]} ... FAILED")
        self.assertEqual(len(guard.check_test_log(cfg, bad)[0]), 1)
        self.assertEqual(len(guard.check_test_log(cfg, "")[0]), len(names))

    def test_expression_eval(self):
        self.assertEqual(guard.eval_rust_number("256 * 1024"), 262144)
        self.assertEqual(guard.eval_rust_number("Duration::from_secs(2)"), 2000)
        self.assertEqual(guard.eval_rust_number("10_000"), 10000)


if __name__ == "__main__":
    unittest.main()
