#!/usr/bin/env python3
"""Thresholds guard: fail if documented CouchLink "optimals" drift.

Reads ci/regression-baseline.toml (the single source of expected values) and
checks the Rust/TS sources statically. No build, no network, stdlib only.

    python3 scripts/ci/regression-guard.py [--root DIR] [--baseline FILE]

Exit 0 = all pass, 1 = at least one drift, 2 = guard/baseline itself broken.
"""
from __future__ import annotations

import argparse
import ast
import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - py<3.11
    sys.exit("regression-guard needs Python >= 3.11 (tomllib)")


def strip_comments(src: str) -> str:
    """Drop // line comments so doc text cannot satisfy or trip a check."""
    return re.sub(r"//[^\n]*", "", src)


def eval_rust_number(expr: str) -> float:
    """Evaluate `10_000`, `256 * 1024`, `Duration::from_secs(2)` -> number.

    Durations normalise to milliseconds.
    """
    e = expr.strip()
    e = re.sub(r"(?:std::time::)?Duration::from_secs\(\s*([\d_]+)\s*\)", lambda m: f"({m.group(1)}*1000)", e)
    e = re.sub(r"(?:std::time::)?Duration::from_millis\(\s*([\d_]+)\s*\)", lambda m: f"({m.group(1)})", e)
    e = re.sub(r"(?<=\d)_(?=\d)", "", e)
    e = re.sub(r"(?<=\d)(?:u8|u16|u32|u64|usize|i32|i64)\b", "", e)
    tree = ast.parse(e, mode="eval")
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Expression, ast.BinOp, ast.Constant, ast.Mult, ast.Add, ast.Sub, ast.Div, ast.UnaryOp, ast.USub)):
            raise ValueError(f"unsupported expression: {expr!r}")
    return eval(compile(tree, "<expr>", "eval"), {"__builtins__": {}}, {})  # noqa: S307 - AST whitelisted above


def read_const(src: str, item: str) -> float:
    code = strip_comments(src)
    if "." in item:  # struct const field: P720_60.width
        const, field = item.split(".", 1)
        block = re.search(rf"const\s+{re.escape(const)}\s*:[^=]*=\s*[A-Za-z_]+\s*\{{(.*?)\}}\s*;", code, re.S)
        if not block:
            raise LookupError(f"const block {const} not found")
        m = re.search(rf"\b{re.escape(field)}\s*:\s*([^,}}]+)", block.group(1))
        if not m:
            raise LookupError(f"field {field} not found in {const}")
        return eval_rust_number(m.group(1))
    m = re.search(rf"\bconst\s+{re.escape(item)}\s*:[^=;]*=\s*([^;]+);", code)
    if not m:
        raise LookupError(f"const {item} not found")
    return eval_rust_number(m.group(1))


def count_rust_tests(src: str) -> int:
    return len(re.findall(r"#\[(?:tokio::)?test\b", src))


def has_live_test(src: str, name: str) -> bool:
    """`fn name` preceded by #[test]/#[tokio::test] attrs and no #[ignore]."""
    for m in re.finditer(rf"((?:[ \t]*#\[[^\n]*\]\s*)+)(?:pub\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\(", src):
        attrs = m.group(1)
        if re.search(r"#\[(?:tokio::)?test\b", attrs) and "#[ignore" not in attrs:
            return True
    return False


def count_web_tests(src: str) -> int:
    return len(re.findall(r"^\s*(?:it|test)(?:\.each\([^)]*\))?\(", src, re.M))


def check_test_log(cfg: dict, log: str) -> tuple[list[str], int]:
    """Every sacred test must appear as `test <path>::<name> ... ok` in a cargo test log.

    Catches a filter that silently matched nothing (0 tests run = green but blind).
    """
    failures, n = [], 0
    for c in cfg.get("sacred", []):
        for t in c["tests"]:
            n += 1
            if not re.search(rf"^test \S*::{re.escape(t)} \.\.\. ok$", log, re.M):
                failures.append(f"[sacred-ran] {t} did not run and pass in the cargo test log")
    return failures, n


def run(root: Path, baseline_path: Path, test_log: str | None = None) -> tuple[list[str], int]:
    cfg = tomllib.loads(baseline_path.read_text(encoding="utf-8"))
    failures: list[str] = []
    checked = 0
    cache: dict[str, str] = {}

    def src(rel: str) -> str | None:
        if rel not in cache:
            p = root / rel
            cache[rel] = p.read_text(encoding="utf-8") if p.is_file() else ""
            if not p.is_file():
                failures.append(f"[missing-file] {rel} does not exist (renamed/deleted?)")
                cache[rel] = None  # type: ignore[assignment]
        return cache[rel]

    for c in cfg.get("const", []):
        checked += 1
        s = src(c["file"])
        if s is None:
            continue
        try:
            v = read_const(s, c["item"])
        except (LookupError, ValueError, SyntaxError) as e:
            failures.append(f"[{c['id']}] {c['file']}: cannot read {c['item']}: {e}")
            continue
        bad = []
        if "eq" in c and v != c["eq"]:
            bad.append(f"expected == {c['eq']}")
        if "min" in c and v < c["min"]:
            bad.append(f"expected >= {c['min']}")
        if "max" in c and v > c["max"]:
            bad.append(f"expected <= {c['max']}")
        if bad:
            failures.append(f"[{c['id']}] {c['file']} {c['item']} = {v:g} ({', '.join(bad)}) -- {c['why']}")

    for c in cfg.get("snippet", []):
        checked += 1
        s = src(c["file"])
        if s is None:
            continue
        n = len(re.findall(c["pattern"], strip_comments(s)))
        if n < c["count"]:
            failures.append(f"[{c['id']}] {c['file']}: pattern /{c['pattern']}/ matched {n}x, need >= {c['count']} -- {c['why']}")

    for c in cfg.get("forbidden", []):
        checked += 1
        s = src(c["file"])
        if s is None:
            continue
        n = len(re.findall(c["pattern"], strip_comments(s)))
        if n:
            failures.append(f"[{c['id']}] {c['file']}: forbidden pattern /{c['pattern']}/ present {n}x -- {c['why']}")

    for c in cfg.get("sacred", []):
        s = src(c["file"])
        for t in c["tests"]:
            checked += 1
            if s is not None and not has_live_test(s, t):
                failures.append(f"[sacred-test] {c['file']}: #[test] fn {t} missing, renamed or #[ignore]d -- {c['why']}")

    for c in cfg.get("min_tests", []):
        checked += 1
        s = src(c["file"])
        if s is None:
            continue
        n = count_rust_tests(s)
        if n < c["count"]:
            failures.append(f"[min-tests] {c['file']}: {n} #[test] fns, baseline requires >= {c['count']}")

    for c in cfg.get("web_tests", []):
        checked += 1
        s = src(c["file"])
        if s is None:
            continue
        n = count_web_tests(s)
        if n < c["count"]:
            failures.append(f"[web-tests] {c['file']}: {n} it()/test() cases, baseline requires >= {c['count']}")

    if test_log is not None:
        f, n = check_test_log(cfg, test_log)
        failures += f
        checked += n
    return failures, checked


def main() -> int:
    here = Path(__file__).resolve().parent.parent.parent
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=Path, default=here)
    ap.add_argument("--baseline", type=Path, default=None)
    ap.add_argument("--test-log", type=Path, default=None,
                    help="cargo test output; also require every sacred test to have run and passed")
    a = ap.parse_args()
    baseline = a.baseline or a.root / "ci" / "regression-baseline.toml"
    try:
        failures, checked = run(a.root, baseline, a.test_log.read_text(encoding='utf-8', errors='replace') if a.test_log else None)
    except Exception as e:  # baseline unreadable etc.
        print(f"regression-guard: internal error: {e}", file=sys.stderr)
        return 2
    if failures:
        print(f"regression-guard: FAIL ({len(failures)} of {checked} checks)")
        for f in failures:
            print(f"  - {f}")
        print("If the change is intentional, update ci/regression-baseline.toml (see docs/REGRESSION_CI.md).")
        return 1
    print(f"regression-guard: OK ({checked} checks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
