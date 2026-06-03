#!/usr/bin/env bash
#
# Download the PRE-CONVERTED BirdNET model (+ labels, taxonomy, range model)
# that birdnet-rs publishes as GitHub release assets, so you don't need Python,
# TensorFlow or the TFLite->ONNX conversion locally. This is what the packaged
# builds (deb / tar.gz / zip / msi) use to fetch the model on first run.
#
# The model is BirdNET GLOBAL 6K v2.4, licensed CC BY-NC-SA 4.0 by the Cornell
# Lab of Ornithology — see the bundled MODEL_LICENSE.txt. Non-commercial use,
# attribution and share-alike apply.
#
# Usage:
#   scripts/fetch-model.sh [OUT_DIR]      # default OUT_DIR: models
#
# Override the source (e.g. to pin a specific release tag) with:
#   BIRDNET_MODEL_BASE_URL=https://github.com/dividebysandwich/birdnet-rs/releases/download/v0.1.0

set -euo pipefail

OUT="${1:-models}"
BASE="${BIRDNET_MODEL_BASE_URL:-https://github.com/dividebysandwich/birdnet-rs/releases/latest/download}"

FILES=(
  BirdNET_GLOBAL_6K_V2.4.onnx
  BirdNET_GLOBAL_6K_V2.4_RangeModel.onnx
  BirdNET_GLOBAL_6K_V2.4_Labels_en_us.txt
  eBird_taxonomy_codes_2021E.json
  MODEL_LICENSE.txt
)

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fL --retry 3 --progress-bar -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --show-progress -O "$2" "$1"; }
else
  echo "error: need curl or wget installed" >&2
  exit 1
fi

mkdir -p "$OUT"
for f in "${FILES[@]}"; do
  dest="$OUT/$f"
  if [[ -s "$dest" ]]; then
    echo "exists, skipping: $dest"
    continue
  fi
  echo "downloading $f ..."
  fetch "$BASE/$f" "$dest.part"
  mv "$dest.part" "$dest"
done

echo
echo "model ready in: $OUT"
echo "NOTE: the BirdNET model is CC BY-NC-SA 4.0 (Cornell Lab) — see $OUT/MODEL_LICENSE.txt"
