#!/usr/bin/env python3
"""Export labeled split-screen crops into tools/split-oracle/dataset/."""

from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path


def load_jsonl(path: Path) -> list[dict]:
    rows: list[dict] = []
    with path.open(encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rows.append(json.loads(line))
    return rows


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--images-dir", type=Path, required=True, help="Directory of source PNGs")
    p.add_argument("--labels", type=Path, required=True, help="JSONL labels file")
    p.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parent / "dataset",
        help="Output dataset root",
    )
    args = p.parse_args()

    images_dir: Path = args.images_dir
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)
    (out / "images").mkdir(exist_ok=True)

    labels_out: list[dict] = []
    for row in load_jsonl(args.labels):
        name = row.get("file")
        if not name:
            raise SystemExit("each JSONL row needs a 'file' field")
        src = images_dir / name
        if not src.is_file():
            raise SystemExit(f"missing image: {src}")
        dst = out / "images" / name
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        labels_out.append(
            {
                "file": f"images/{name}",
                "layout": row.get("layout", "none"),
                "regions": row.get("regions", []),
                "seats": row.get("seats", []),
            }
        )

    manifest = out / "manifest.jsonl"
    with manifest.open("w", encoding="utf-8") as f:
        for row in labels_out:
            f.write(json.dumps(row) + "\n")

    print(f"exported {len(labels_out)} samples -> {out}")
    print(f"manifest: {manifest}")


if __name__ == "__main__":
    main()
