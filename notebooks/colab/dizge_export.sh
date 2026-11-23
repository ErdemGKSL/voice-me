#!/bin/bash
# dizge_export.sh [checkpoint]: newest checkpoint -> /content/piper-run/<model>-voice.zip
# (<model>.onnx + <model>.onnx.json with phoneme_type dizge), as the notebook's cell 7 does.
set -euo pipefail
REPO_DIR=/content/piper1-gpl
OUT=$(ls -d /content/piper-out/*/ | head -1); OUT=${OUT%/}
MODEL=$(basename "$OUT")
CKPT=${1:-$(ls -t "$OUT"/lightning_logs/*/checkpoints/*.ckpt | head -1)}
echo "checkpoint: $CKPT"
cd "$REPO_DIR"
python -m piper.train.export_onnx --checkpoint "$CKPT" --output-file "$OUT/$MODEL.onnx"
python3 - "$OUT" "$MODEL" <<'PY'
import json, shutil, sys, zipfile
out, model = sys.argv[1], sys.argv[2]
cfg = json.load(open(f"{out}/config.json", encoding="utf-8"))
assert cfg["phoneme_type"] == "dizge", cfg["phoneme_type"]
shutil.copy(f"{out}/config.json", f"{out}/{model}.onnx.json")
zip_path = f"/content/piper-run/{model}-voice.zip"
with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as z:
    z.write(f"{out}/{model}.onnx", f"{model}.onnx")
    z.write(f"{out}/{model}.onnx.json", f"{model}.onnx.json")
print("ready:", zip_path)
PY
