#!/usr/bin/env bash
# Portable launcher for the tar.gz distribution: points the server at the
# bundled `site/` directory and runs from the unpacked folder, so the daemon
# writes config.yaml, the SQLite DB, clips/ and images/ alongside the binary.
# Download the BirdNET model first: ./scripts/download_model.sh --convert
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export LEPTOS_SITE_ROOT="$DIR/site"
export LEPTOS_SITE_ADDR="${LEPTOS_SITE_ADDR:-0.0.0.0:8080}"
cd "$DIR"

# First run: fetch the pre-converted BirdNET model (CC BY-NC-SA 4.0) if missing.
if [[ ! -s "$DIR/models/BirdNET_GLOBAL_6K_V2.4.onnx" ]]; then
  echo "BirdNET model not found — downloading the pre-converted model ..."
  "$DIR/scripts/fetch-model.sh" "$DIR/models" \
    || echo "model download failed — dashboard will start, detection stays idle until a model is present." >&2
fi

exec "$DIR/birdnet-rs" "$@"
