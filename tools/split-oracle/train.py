#!/usr/bin/env python3
"""Train a tiny split-layout classifier and export ONNX (stub or sklearn)."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def write_stub_onnx(out: Path) -> None:
    """Write a minimal valid ONNX graph (Identity) for pipeline smoke tests."""
    # ONNX protobuf wire format is heavy; use onnx package when available.
    try:
        import numpy as np
        from onnx import TensorProto, helper, save

        X = helper.make_tensor_value_info("input", TensorProto.FLOAT, [1, 4])
        Y = helper.make_tensor_value_info("output", TensorProto.FLOAT, [1, 4])
        node = helper.make_node("Identity", ["input"], ["output"])
        graph = helper.make_graph([node], "split_oracle_stub", [X], [Y])
        model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
        save(model, str(out))
        return
    except ImportError:
        pass

    bundled = Path(__file__).resolve().parent / "stub_identity.onnx"
    if bundled.is_file():
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(bundled.read_bytes())
        return
    raise SystemExit(
        "stub ONNX needs `pip install onnx` (see requirements-train.txt) or stub_identity.onnx in tree"
    )


def train_sklearn(dataset: Path, out: Path) -> None:
    import numpy as np
    from sklearn.linear_model import LogisticRegression
    from skl2onnx import convert_sklearn
    from skl2onnx.common.data_types import FloatTensorType

    manifest = dataset / "manifest.jsonl"
    if not manifest.is_file():
        raise SystemExit(f"no manifest at {manifest} — run label_export.py first")

    layouts = ["none", "horizontal", "vertical", "grid2x2"]
    layout_to_y = {name: i for i, name in enumerate(layouts)}
    X_list: list[list[float]] = []
    y_list: list[int] = []

    with manifest.open(encoding="utf-8") as f:
        for line in f:
            row = json.loads(line)
            regions = row.get("regions") or []
            feats = [0.0, 0.0, 0.0, 0.0]
            for i, r in enumerate(regions[:4]):
                feats[i] = float(r.get("w", 0)) * float(r.get("h", 0))
            X_list.append(feats)
            y_list.append(layout_to_y.get(row.get("layout", "none"), 0))

    X = np.array(X_list, dtype=np.float32)
    y = np.array(y_list, dtype=np.int64)
    clf = LogisticRegression(max_iter=500)
    clf.fit(X, y)
    onnx_model = convert_sklearn(
        clf,
        initial_types=[("input", FloatTensorType([None, 4]))],
        target_opset=13,
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("wb") as fo:
        fo.write(onnx_model.SerializeToString())
    print(f"wrote {out} ({len(y_list)} samples)")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--dataset", type=Path, default=Path(__file__).resolve().parent / "dataset")
    p.add_argument("--out", type=Path, default=Path(__file__).resolve().parent / "model.onnx")
    p.add_argument("--stub", action="store_true", help="Export minimal Identity ONNX")
    args = p.parse_args()

    if args.stub:
        write_stub_onnx(args.out)
        print(f"wrote stub ONNX -> {args.out}")
        return

    train_sklearn(args.dataset, args.out)


if __name__ == "__main__":
    main()
