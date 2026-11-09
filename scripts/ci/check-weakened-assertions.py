#!/usr/bin/env python3
"""Fail a PR that loosens numeric thresholds or deletes tests in the latency/sacred test files.

    python3 scripts/ci/check-weakened-assertions.py --base origin/main [--head HEAD]
                [--body-file FILE | env PR_BODY]

Scope (Rust): crates/host/src/ricardo_*.rs, amazing_latency*.rs,
latency_live_sim/**, and the #[cfg(test)] part of link_gov.rs.

Flagged (unless a line `regression-baseline-change: <reason>` appears in the PR
body or any commit message in base..head):
  * a test fn removed, renamed, or losing its #[test] attribute / gaining #[ignore]
  * an assert line removed with no equivalent added
  * a numeric literal changed on an assert/const line in a way that is not
    clearly stricter:  `>= N` lowered, `<= N` raised, MIN_* lowered, MAX_*
    raised, any `== N` / unknown-direction change.
Stricter changes (>= raised, <= lowered) pass.
"""
from __future__ import annotations

import argparse
import fnmatch
import os
import re
import subprocess
import sys

SCOPE_GLOBS = [
    "crates/host/src/ricardo_*.rs",
    "crates/host/src/amazing_latency*.rs",
    "crates/host/src/latency_live_sim/*.rs",
    "crates/host/src/latency_live_sim/**/*.rs",
    "crates/host/src/link_gov.rs",
]
TEST_ONLY_AFTER_CFG = {"crates/host/src/link_gov.rs"}
MARKER_RE = re.compile(r"^\s*regression-baseline-change:\s*\S+", re.M | re.I)
NUM_RE = re.compile(r"(?<![\w.])\d[\d_]*(?:\.\d+)?")


def in_scope(path: str) -> bool:
    return any(fnmatch.fnmatch(path, g) for g in SCOPE_GLOBS)


def git(*args: str, check: bool = True) -> str:
    r = subprocess.run(["git", *args], capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout if r.returncode == 0 else ""


def show(ref: str, path: str) -> str | None:
    r = subprocess.run(["git", "show", f"{ref}:{path}"], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def test_region(path: str, text: str) -> str:
    """Return the part of the file the check applies to (whole file, or the test mod)."""
    if path in TEST_ONLY_AFTER_CFG:
        i = text.find("#[cfg(test)]")
        return text[i:] if i >= 0 else ""
    return text


def live_tests(text: str) -> set[str]:
    out = set()
    for m in re.finditer(r"((?:[ \t]*#\[[^\n]*\]\s*)+)(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(", text):
        attrs = m.group(1)
        if re.search(r"#\[(?:tokio::)?test\b", attrs) and "#[ignore" not in attrs:
            out.add(m.group(2))
    return out


def nums(line: str) -> list[float]:
    return [float(n.replace("_", "")) for n in NUM_RE.findall(line)]


def skeleton(line: str) -> str:
    return re.sub(r"\s+", " ", NUM_RE.sub("#", line.strip().rstrip(",;")))


def direction(line: str) -> str:
    """'lower' (>=,>,MIN_), 'upper' (<=,<,MAX_), 'eq', or 'unknown'."""
    code = re.sub(r"//.*", "", line)
    code = re.sub(r'"[^"]*"', '""', code)  # ignore operators inside messages
    has_lo = bool(re.search(r">=|(?<![-=])>(?!=)", code))
    has_hi = bool(re.search(r"<=|(?<![-<])<(?!=)", code))
    has_eq = bool(re.search(r"==|assert_eq!|assert_ne!", code))
    m = re.search(r"\bconst\s+(\w+)", code)
    if m:
        n = m.group(1)
        if n.startswith("MIN_") or "_MIN_" in n:
            return "lower"
        if n.startswith("MAX_") or "_MAX_" in n:
            return "upper"
        return "unknown"
    if has_eq and not (has_lo or has_hi):
        return "eq"
    if has_lo and not has_hi:
        return "lower"
    if has_hi and not has_lo:
        return "upper"
    return "unknown"


def is_threshold_line(line: str) -> bool:
    s = line.strip()
    return ("assert" in s or re.search(r"\bconst\s+\w+", s) is not None) and bool(NUM_RE.search(s))


def check_diff(path: str, diff: str, new_text: str) -> list[str]:
    """Pure function over a -U0 diff: used by analyse() and the unit tests."""
    problems: list[str] = []
    cfg_line = None
    if path in TEST_ONLY_AFTER_CFG:
        i = new_text.find("#[cfg(test)]")
        cfg_line = new_text[:i].count("\n") + 1 if i >= 0 else 10**9
    chunks: list[tuple[int, list[str], list[str]]] = []
    start, rem, add = 0, [], []
    for ln in diff.splitlines():
        if ln.startswith("@@"):
            if rem or add:
                chunks.append((start, rem, add))
            m = re.search(r"\+(\d+)", ln)
            start = int(m.group(1)) if m else 0
            rem, add = [], []
        elif ln.startswith("-") and not ln.startswith("---"):
            rem.append(ln[1:])
        elif ln.startswith("+") and not ln.startswith("+++"):
            add.append(ln[1:])
    if rem or add:
        chunks.append((start, rem, add))
    all_added_skel = {skeleton(a) for _, _, ad in chunks for a in ad}
    for start, rem, add in chunks:
        if cfg_line is not None and start < cfg_line:
            continue
        pool = list(add)
        for r in rem:
            if not is_threshold_line(r):
                continue
            if skeleton(r) in all_added_skel and nums(r) in [nums(a) for a in add]:
                continue  # moved/unchanged
            sk = skeleton(r)
            mate = next((a for a in pool if skeleton(a) == sk), None)
            if mate is None:
                if "assert" in r and not any("assert" in a for a in add):
                    problems.append(f"{path}: assertion removed: `{r.strip()}`")
                elif "assert" in r:
                    problems.append(f"{path}: assertion rewritten, verify not loosened: `{r.strip()}` -> `{add[0].strip() if add else ''}`")
                else:
                    problems.append(f"{path}: threshold line removed: `{r.strip()}`")
                continue
            pool.remove(mate)
            o, n = nums(r), nums(mate)
            if o == n:
                continue
            d = direction(mate)
            changed = [(a, b) for a, b in zip(o, n) if a != b]
            loosened = (
                d in ("eq", "unknown")
                or (d == "lower" and any(b < a for a, b in changed))
                or (d == "upper" and any(b > a for a, b in changed))
            )
            if loosened:
                kind = {"eq": "expectation changed", "unknown": "threshold changed (direction unclear)",
                        "lower": "lower bound lowered", "upper": "upper bound raised"}[d]
                problems.append(f"{path}: {kind}: `{r.strip()}` -> `{mate.strip()}`")
    return problems


def analyse(path: str, base: str, head: str) -> list[str]:
    old = show(base, path)
    new = show(head, path)
    problems: list[str] = []
    if old is None:
        return problems
    old_r = test_region(path, old)
    if new is None:
        return [f"{path}: file deleted, test `{t}` gone" for t in sorted(live_tests(old_r))] or [f"{path}: file deleted"]
    for t in sorted(live_tests(old_r) - live_tests(test_region(path, new))):
        problems.append(f"{path}: test fn `{t}` removed, renamed, un-#[test]ed or #[ignore]d")
    diff = git("diff", "-U0", "--no-color", base, head, "--", path)
    problems += check_diff(path, diff, new)
    return problems


def marker_present(base: str, head: str, body: str) -> bool:
    if MARKER_RE.search(body or ""):
        return True
    msgs = git("log", "--format=%B", f"{base}..{head}", check=False)
    return bool(MARKER_RE.search(msgs))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", required=True, help="base ref (e.g. origin/main)")
    ap.add_argument("--head", default="HEAD")
    ap.add_argument("--body-file")
    a = ap.parse_args()
    body = os.environ.get("PR_BODY", "")
    if a.body_file and os.path.isfile(a.body_file):
        body += "\n" + open(a.body_file, encoding="utf-8").read()
    base = git("merge-base", a.base, a.head, check=False).strip() or a.base
    changed = [p for p in git("diff", "--name-only", base, a.head).splitlines() if in_scope(p)]
    # Files deleted in head still need reporting; git diff --name-only includes them.
    problems: list[str] = []
    for p in changed:
        problems += analyse(p, base, a.head)
    if not problems:
        print(f"check-weakened-assertions: OK ({len(changed)} in-scope file(s) changed)")
        return 0
    if marker_present(base, a.head, body):
        print("check-weakened-assertions: weakening detected but 'regression-baseline-change:' marker present -> allowed")
        for p in problems:
            print(f"  ~ {p}")
        return 0
    print(f"check-weakened-assertions: FAIL ({len(problems)} issue(s))")
    for p in problems:
        print(f"  - {p}")
    print("Intentional? Add a line `regression-baseline-change: <reason>` to the PR description or a commit message,")
    print("and update ci/regression-baseline.toml where relevant (docs/REGRESSION_CI.md).")
    return 1


if __name__ == "__main__":
    sys.exit(main())
