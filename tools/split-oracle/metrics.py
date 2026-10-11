"""Seam IoU and seat-assignment confusion helpers for split-oracle eval."""

from __future__ import annotations

from typing import Iterable, Mapping, Sequence


def _rect_area(r: Mapping[str, float]) -> float:
    return max(0.0, float(r["w"])) * max(0.0, float(r["h"]))


def rect_intersection(a: Mapping[str, float], b: Mapping[str, float]) -> float:
    ax2 = float(a["x"]) + float(a["w"])
    ay2 = float(a["y"]) + float(a["h"])
    bx2 = float(b["x"]) + float(b["w"])
    by2 = float(b["y"]) + float(b["h"])
    ix = max(0.0, min(ax2, bx2) - max(float(a["x"]), float(b["x"])))
    iy = max(0.0, min(ay2, by2) - max(float(a["y"]), float(b["y"])))
    return ix * iy


def rect_iou(a: Mapping[str, float], b: Mapping[str, float]) -> float:
    inter = rect_intersection(a, b)
    union = _rect_area(a) + _rect_area(b) - inter
    if union <= 0:
        return 0.0
    return inter / union


def seam_iou(
    pred_regions: Sequence[Mapping[str, float]],
    truth_regions: Sequence[Mapping[str, float]],
) -> float:
    """Mean best-match IoU per predicted region (order-agnostic)."""
    if not pred_regions and not truth_regions:
        return 1.0
    if not pred_regions or not truth_regions:
        return 0.0
    scores: list[float] = []
    for pr in pred_regions:
        best = max(rect_iou(pr, tr) for tr in truth_regions)
        scores.append(best)
    return sum(scores) / len(scores)


def seat_confusion_matrix(
    pred_seats: Sequence[int],
    truth_seats: Sequence[int],
    labels: Iterable[int] | None = None,
) -> dict[tuple[int, int], int]:
    """
    Build confusion counts for seat assignment.
    Rows = truth seat id, cols = predicted seat id.
    """
    if labels is None:
        labels = sorted(set(truth_seats) | set(pred_seats))
    label_list = list(labels)
    matrix: dict[tuple[int, int], int] = {}
    n = min(len(pred_seats), len(truth_seats))
    for i in range(n):
        t, p = int(truth_seats[i]), int(pred_seats[i])
        if t not in label_list or p not in label_list:
            continue
        matrix[(t, p)] = matrix.get((t, p), 0) + 1
    return matrix
