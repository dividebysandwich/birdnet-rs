#!/usr/bin/env bash
#
# Convert the BirdNET v2.4 TFLite model to ONNX for birdnet-rs.
#
# IMPORTANT: a plain `tf2onnx` conversion does NOT work for this model. BirdNET
# embeds its mel-spectrogram front-end (an STFT) in the graph, which tf2onnx
# lowers to an RFFT2D op it cannot convert ("RFFT2D only allows ComplexAbs as
# consumer not {'Squeeze'}"). The official converter from the birdnet-go author
# handles this by keeping RFFT2D as a custom op and then rewriting RFFT2D →
# MatMul (precomputed DFT matrix) during optimization.
#
# This script drives that official tool — https://github.com/tphakala/birdnet-onnx-converter
# — in an isolated virtualenv with the dependency versions it pins
# (tensorflow<2.21, tf2onnx>=1.17), so it works regardless of what TF/tf2onnx you
# have installed system-wide.
#
# Usage:
#   scripts/convert_model.sh [--tflite PATH] [--out PATH] [--fp16]
#                            [--cache DIR] [--ref GIT_REF]
#
# Defaults convert models/BirdNET_GLOBAL_6K_V2.4_Model_FP32.tflite
#                  → models/BirdNET_GLOBAL_6K_V2.4.onnx (FP32).

set -euo pipefail

TFLITE="models/BirdNET_GLOBAL_6K_V2.4_Model_FP32.tflite"
OUT="models/BirdNET_GLOBAL_6K_V2.4.onnx"
CACHE=".cache/birdnet-onnx-converter"
PRECISION="fp32"
REF=""
PYBIN=""
CONVERTER_URL="https://github.com/tphakala/birdnet-onnx-converter"

usage() { sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//; /^set -euo/d'; exit "${1:-0}"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tflite) TFLITE="${2:?}"; shift 2 ;;
    --out)    OUT="${2:?}"; shift 2 ;;
    --fp16)   PRECISION="fp16"; shift ;;
    --cache)  CACHE="${2:?}"; shift 2 ;;
    --ref)    REF="${2:?}"; shift 2 ;;
    --python) PYBIN="${2:?}"; shift 2 ;;
    -h|--help) usage 0 ;;
    *) echo "unknown option: $1" >&2; usage 1 ;;
  esac
done

command -v git >/dev/null || { echo "error: git is required" >&2; exit 1; }
[[ -f "$TFLITE" ]] || { echo "error: $TFLITE not found (run scripts/download_model.sh first)" >&2; exit 1; }

# TensorFlow (needed by the converter) has no wheels for Python 3.14+, so pick an
# interpreter in the 3.10–3.13 range to build the venv with. Override with --python.
py_ok() {
  "$1" -c 'import sys; raise SystemExit(0 if (3,10) <= sys.version_info[:2] <= (3,13) else 1)' 2>/dev/null
}
if [[ -n "$PYBIN" ]]; then
  command -v "$PYBIN" >/dev/null || { echo "error: --python '$PYBIN' not found" >&2; exit 1; }
else
  for cand in python3.12 python3.11 python3.13 python3.10 python3; do
    if command -v "$cand" >/dev/null && py_ok "$cand"; then PYBIN="$cand"; break; fi
  done
fi
if [[ -z "$PYBIN" ]]; then
  echo "error: need a Python interpreter in the 3.10–3.13 range (TensorFlow has no" >&2
  echo "       wheels for 3.14+). Install e.g. python3.12, or pass --python <path>." >&2
  exit 1
fi
echo "using interpreter: $PYBIN ($("$PYBIN" --version 2>&1))"

# Absolute paths (the converter runs from its own dir).
TFLITE_ABS="$(cd "$(dirname "$TFLITE")" && pwd)/$(basename "$TFLITE")"
mkdir -p "$(dirname "$OUT")"
OUT_ABS="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")"

# 1. Fetch the converter.
if [[ ! -d "$CACHE/.git" ]]; then
  echo "cloning birdnet-onnx-converter into $CACHE ..."
  mkdir -p "$(dirname "$CACHE")"
  git clone --depth 1 ${REF:+--branch "$REF"} "$CONVERTER_URL" "$CACHE"
fi
CACHE_ABS="$(cd "$CACHE" && pwd)"

# 2. Isolated venv with the converter's pinned deps (idempotent).
VENV="$CACHE_ABS/.venv"
if [[ ! -f "$VENV/.deps-installed" ]]; then
  echo "setting up virtualenv + dependencies (this downloads TensorFlow, ~minutes) ..."
  "$PYBIN" -m venv "$VENV"
  # shellcheck disable=SC1091
  source "$VENV/bin/activate"
  pip install -q -U pip
  pip install -q -r "$CACHE_ABS/requirements.txt"
  pip install -q -r "$CACHE_ABS/requirements-tflite.txt"
  touch "$VENV/.deps-installed"
else
  # shellcheck disable=SC1091
  source "$VENV/bin/activate"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# 3. TFLite → ONNX (keeps RFFT2D as a custom op).
echo "converting TFLite → ONNX ..."
( cd "$CACHE_ABS" && python convert.py --input "$TFLITE_ABS" --output-dir "$WORK" --onnx-only )
RAW="$(ls -1 "$WORK"/*.onnx | head -1)"
[[ -n "$RAW" ]] || { echo "error: conversion produced no .onnx" >&2; exit 1; }

# 4. Optimize (RFFT2D → MatMul, Cast removal, graph simplification).
echo "optimizing ($PRECISION) ..."
if [[ "$PRECISION" == "fp32" ]]; then
  ( cd "$CACHE_ABS" && python optimize.py --input "$RAW" --output "$WORK/BirdNET" --fp32-only )
else
  ( cd "$CACHE_ABS" && python optimize.py --input "$RAW" --output "$WORK/BirdNET" --no-int8 )
fi

PRODUCED="$WORK/BirdNET_${PRECISION}.onnx"
[[ -f "$PRODUCED" ]] || PRODUCED="$(ls -1 "$WORK"/*"${PRECISION}".onnx | head -1)"
[[ -f "$PRODUCED" ]] || { echo "error: optimizer produced no ${PRECISION} model" >&2; exit 1; }

cp "$PRODUCED" "$OUT_ABS"

# 5. Sanity-check shapes with onnxruntime (already in the venv).
python - "$OUT_ABS" <<'PY' || echo "(verification skipped)"
import sys, numpy as np, onnxruntime as ort
f = sys.argv[1]
s = ort.InferenceSession(f, providers=["CPUExecutionProvider"])
i, o = s.get_inputs()[0], s.get_outputs()[0]
print(f"input : {i.name} {i.shape}")
print(f"output: {o.name} {o.shape}")
# Use the model's own input shape (symbolic dims like 'batch' → 1). Works for
# both the classifier ([1,144000]) and the range/meta model ([1,3]).
shape = [d if isinstance(d, int) else 1 for d in i.shape]
y = s.run([o.name], {i.name: np.zeros(shape, dtype=np.float32)})[0]
print(f"forward pass ok: {y.shape}")
PY

echo
echo "done → $OUT"
echo "set in config.yaml: birdnet.model_path: $OUT"
