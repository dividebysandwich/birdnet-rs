#!/usr/bin/env bash
#
# Download the BirdNET GLOBAL 6K v2.4 model + labels so you don't have to run
# birdnet-go first. The files are pulled from birdnet-go's own embedded data, so
# the model and the labels are guaranteed to be index-aligned.
#
# The model ships as TensorFlow Lite; birdnet-rs runs ONNX, so after downloading
# you convert it once with scripts/convert_model.py (pass --convert to do both).
#
# Usage:
#   scripts/download_model.sh [--locale en_us] [--out models] [--ref main]
#                             [--force] [--convert]
#
# Examples:
#   scripts/download_model.sh                  # tflite + en_us labels into ./models
#   scripts/download_model.sh --locale de      # German common names
#   scripts/download_model.sh --convert        # download, then convert to ONNX

set -euo pipefail

LOCALE="en_us"
OUT="models"
REF="main"
FORCE=0
CONVERT=0

REPO_RAW="https://raw.githubusercontent.com/tphakala/birdnet-go"
DATA_DIR="internal/classifier/data"
MODEL_FILE="BirdNET_GLOBAL_6K_V2.4_Model_FP32.tflite"

# Locales available in birdnet-go's V2.4 label set.
LOCALES="af ar bg ca cs da de el en_uk en_us es et_ee fi fr he hi_in hr hu id \
is it ja ko lt lv_lv ml nl no pl pt_BR pt_PT ro ru sk sl sr sv th tr uk vi_vn zh"

usage() { sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//; /^set -euo/d'; exit "${1:-0}"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --locale) LOCALE="${2:?missing locale}"; shift 2 ;;
    --out)    OUT="${2:?missing dir}"; shift 2 ;;
    --ref)    REF="${2:?missing ref}"; shift 2 ;;
    --force)  FORCE=1; shift ;;
    --convert) CONVERT=1; shift ;;
    -h|--help) usage 0 ;;
    *) echo "unknown option: $1" >&2; usage 1 ;;
  esac
done

# Validate locale against the known list.
if ! grep -qw -- "$LOCALE" <<<"$LOCALES"; then
  echo "error: unknown locale '$LOCALE'." >&2
  echo "available: $LOCALES" >&2
  exit 1
fi

# Pick a downloader.
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fL --retry 3 --progress-bar -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --show-progress -O "$2" "$1"; }
else
  echo "error: need curl or wget installed" >&2
  exit 1
fi

mkdir -p "$OUT"
LABELS_FILE="BirdNET_GLOBAL_6K_V2.4_Labels_${LOCALE}.txt"
MODEL_URL="${REPO_RAW}/${REF}/${DATA_DIR}/${MODEL_FILE}"
LABELS_URL="${REPO_RAW}/${REF}/${DATA_DIR}/labels/V2.4/${LABELS_FILE}"

# Download one file unless it already exists (and --force not given).
get() {
  local url="$1" dest="$2"
  if [[ -s "$dest" && $FORCE -eq 0 ]]; then
    echo "exists, skipping: $dest  (use --force to re-download)"
    return
  fi
  echo "downloading $(basename "$dest") ..."
  fetch "$url" "$dest.part"
  mv "$dest.part" "$dest"
}

get "$MODEL_URL"  "$OUT/$MODEL_FILE"
get "$LABELS_URL" "$OUT/$LABELS_FILE"

# Sanity checks.
model_bytes=$(wc -c < "$OUT/$MODEL_FILE")
label_lines=$(wc -l < "$OUT/$LABELS_FILE")
echo
echo "model : $OUT/$MODEL_FILE  (${model_bytes} bytes)"
echo "labels: $OUT/$LABELS_FILE (~$((label_lines + 1)) species)"
if [[ "$model_bytes" -lt 1000000 ]]; then
  echo "WARNING: model file looks too small — download may have failed." >&2
fi

if [[ $CONVERT -eq 1 ]]; then
  echo
  echo "converting to ONNX ..."
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  bash "$script_dir/convert_model.sh" \
    --tflite "$OUT/$MODEL_FILE" \
    --out "$OUT/BirdNET_GLOBAL_6K_V2.4.onnx"
else
  echo
  echo "next: convert the TFLite model to ONNX with"
  echo "  scripts/convert_model.sh --tflite $OUT/$MODEL_FILE --out $OUT/BirdNET_GLOBAL_6K_V2.4.onnx"
fi

echo
echo "then set in config.yaml:"
echo "  birdnet.model_path:  $OUT/BirdNET_GLOBAL_6K_V2.4.onnx"
echo "  birdnet.labels_path: $OUT/$LABELS_FILE"
echo "  birdnet.locale:      $LOCALE"
