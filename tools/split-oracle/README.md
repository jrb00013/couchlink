# split-oracle (v1 harness)

Offline labeling, training, and metrics for the Kaleidoscope seat oracle. The browser player keeps using heuristic `detectSeamsSync` on the hot path; an optional ONNX model can run in a Web Worker via `web/src/splitOnnx.ts` without changing crop APIs.

## Layout

| Path | Role |
|------|------|
| `label_export.py` | Export labeled crops + JSONL metadata into `dataset/` |
| `train.py` | Train a tiny layout classifier and export ONNX |
| `metrics.py` | Seam IoU + seat-assignment confusion helpers |
| `dataset/` | Generated exports (gitignored in normal use) |

## Labeling

1. Capture PNG frames (160×90 downscale matches the client detector input).
2. Write one JSON object per line (`labels.jsonl`):

```json
{"file": "frame001.png", "layout": "vertical", "regions": [{"x":0,"y":0,"w":0.5,"h":1},{"x":0.5,"y":0,"w":0.5,"h":1}], "seats": [0, 1]}
```

3. Export:

```bash
python3 tools/split-oracle/label_export.py \
  --images-dir /path/to/pngs \
  --labels tools/split-oracle/labels.jsonl \
  --out tools/split-oracle/dataset
```

## Train + ONNX

Minimal stub (valid tiny ONNX, no ML deps):

```bash
python3 tools/split-oracle/train.py --stub --out tools/split-oracle/model.onnx
```

Full sklearn path (optional):

```bash
python3 -m venv .venv-split-oracle
source .venv-split-oracle/bin/activate
pip install -r tools/split-oracle/requirements-train.txt
python3 tools/split-oracle/train.py --dataset tools/split-oracle/dataset --out tools/split-oracle/model.onnx
```

Copy the ONNX next to the web app or serve it from your static host; wire the URL in the Worker stub when ready.

## Metrics

```python
from metrics import seam_iou, seat_confusion_matrix
```

Use after inference on a held-out set to compare heuristic vs ONNX seam rects and seat tags.

## Browser Worker path

See `web/src/splitOnnx.ts`: loads ONNX when configured, otherwise delegates to `detectSeamsSync`.
