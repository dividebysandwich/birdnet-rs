# birdnet-rs

A Rust port of [birdnet-go](https://github.com/tphakala/birdnet-go) — a
self-hosted, realtime soundscape analyzer that listens to a microphone 24/7,
identifies bird species with the BirdNET neural network, filters false
positives, stores detections, saves audio clips, and shows them live in a web
dashboard.

This is **Phase 1: a working core realtime pipeline**. It deliberately covers
the critical path end-to-end and leaves larger surface area (RTSP, MQTT,
BirdWeather, multi-model, auth, the full Svelte UI) for later phases. The module
layout mirrors birdnet-go's Go packages so those slot in cleanly.

## Pipeline

```
soundcard (cpal) → downmix mono → resample 48 kHz (rubato)
  → 3 s windows w/ overlap → BirdNET ONNX inference (birdnet-onnx crate, on ort)
  → top-k + activation → threshold + false-positive + dynamic-threshold filters
  → actions: store (SeaORM/SQLite) + WAV clip (hound) + SSE broadcast
  → Leptos dashboard
```

## Architecture

A single **Leptos fullstack** crate built with `cargo-leptos`:

- The server (`ssr` feature) runs the realtime daemon and an `axum` server.
- The browser app (`hydrate` feature → WASM) is the same crate, compiled for
  `wasm32-unknown-unknown`; backend deps are `ssr`-only and never reach the WASM bundle.
- Queries use `#[server]` functions (e.g. `list_detections`); the realtime push
  feed (live detections + audio meter) and binary media (clip WAV, spectrogram
  PNG) are plain `axum` routes mounted on the same server, since those don't fit
  request/response server functions.

## Design choices

- **Inference via the [`birdnet-onnx`](https://github.com/tphakala/rust-birdnet-onnx)
  crate** (the birdnet-go author's own library, built on `ort` 2.0-rc). It
  auto-detects the model type and applies the correct activation (BirdNET →
  sigmoid, Perch → softmax, BSG → pre-sigmoid), so we don't hand-roll it.
- **Leptos fullstack** (SSR + hydration + server functions), one crate.
- **64-bit targets** (`x86_64`, `aarch64`); 32-bit ARM is not supported.
- **SeaORM + SQLite** datastore (MySQL/Postgres possible later).

## Getting the model

BirdNET v2.4 ships officially as TensorFlow Lite, so the flow is: download the
model + labels, then convert the model to ONNX once.

### One-shot (download + convert)

```bash
scripts/download_model.sh --convert   # → models/BirdNET_GLOBAL_6K_V2.4.onnx + labels
```

`download_model.sh` pulls the FP32 model and the index-aligned label file from
birdnet-go's own embedded data (so you don't have to run birdnet-go first, and
the model/labels are guaranteed to match). Useful flags:

- `--locale de` — common names in another language (run with `--help` for the list)
- `--out DIR` — download directory (default `models`)
- `--force` — re-download even if files exist
- `--convert` — also run the ONNX conversion afterwards

### Or do the two steps separately

```bash
scripts/download_model.sh                     # → models/*.tflite + *_Labels_en_us.txt
scripts/convert_model.sh                      # → models/BirdNET_GLOBAL_6K_V2.4.onnx
```

`convert_model.sh` needs `git` and a Python interpreter in the **3.10–3.13**
range (TensorFlow has no wheels for 3.14+; the script auto-detects `python3.12`/
`python3.11`/… or takes `--python <path>`). On first run it checks out the
[official BirdNET ONNX converter](https://github.com/tphakala/birdnet-onnx-converter)
into `.cache/` and builds an isolated virtualenv (this downloads TensorFlow, so
the first conversion takes a few minutes; later runs reuse it). Pass `--fp16`
for a half-precision model (smaller, good on a Raspberry Pi 5).

> **Why not a plain `tf2onnx` one-liner?** BirdNET bakes its mel-spectrogram
> front-end (an STFT) into the model graph, which `tf2onnx` lowers to an
> `RFFT2D` op it can't convert. The official converter handles this by keeping
> `RFFT2D` as a custom op and rewriting it to a `MatMul` with a precomputed DFT
> matrix during optimization. `convert_model.sh` just drives that tool with the
> dependency versions it pins.

The resulting ONNX model keeps the embedded mel front-end, so it takes raw
48 kHz PCM (`[1, 144000]`) and outputs one logit per species (`[1, 6522]`).
Models are not committed to the repo due to size and licensing reasons (the
official BirdNET v2.4 weights are CC BY-NC-SA from the Cornell Lab; canonical
source is [Zenodo 15050749](https://zenodo.org/records/15050749)).

## Build & run

Needs the wasm target and `cargo-leptos`:

```bash
rustup target add wasm32-unknown-unknown
cargo install cargo-leptos

# Build server + WASM + bundle the site, then run.
cargo leptos build --release

# First run writes a default config.yaml you can edit (model/audio settings).
LEPTOS_SITE_ROOT=target/site ./target/release/birdnet-rs
```

Or, for development with hot reload of both server and UI:

```bash
cargo leptos watch
```

Then open <http://localhost:8080> (the address is the Leptos `site-addr`).

### Dashboard

- **Live audio monitor** — a VU meter (RMS + peak) and a scrolling spectrogram
  of the microphone input, fed by `audio` SSE events ~20×/s. This is the
  "is the mic working / what's the ambient soundscape" indicator.
- **Input device selector** — pick the *actual* capture device from a dropdown;
  the choice is applied live (no restart) and persisted to the OS user-config
  directory (`~/.config/birdnet-rs/preferences.json` on Linux, `%APPDATA%\…`
  on Windows, `~/Library/Application Support/…` on macOS) so it's restored next
  launch. On Linux, capture uses ALSA directly so individual PipeWire/Pulse
  sources (and hardware cards on plain-ALSA systems) are listed — not just the
  high-level API plugins cpal exposes; macOS/Windows use cpal.
- **Sample-rate selector** — choose the capture rate (44.1 kHz … 256 kHz, filtered
  to what the device reports); also persisted. The BirdNET v2.4 model runs at
  48 kHz, so higher rates are captured and downsampled for inference (real
  high-rate capture, groundwork for ultrasonic/bat models).
- **Closest match** — the current best-guess species + match % shown in
  realtime as audio is analyzed (`live` SSE event), even when it's below the
  detection threshold.
- **Detections** — recent detections (loaded via the `list_detections` server
  function, live-prepended via `detection` SSE), with **search** by name/code,
  per-row **review** (✓ correct / ✗ false-positive), a rendered **clip
  spectrogram** thumbnail, and an audio player.

Detections are stored in `birdnet.db`; clips are written under `clips/<date>/`.

### Configuration (`config.yaml`)

Audio/model/storage settings live in `config.yaml` (path overridable with
`BIRDNET_CONFIG`); the **web address** is the Leptos `site-addr`
(`Cargo.toml [package.metadata.leptos]`, or `LEPTOS_SITE_ADDR`, default
`0.0.0.0:8080`).

```yaml
birdnet:
  model_path: models/BirdNET_GLOBAL_6K_V2.4.onnx
  labels_path: models/BirdNET_GLOBAL_6K_V2.4_Labels_en_us.txt
  threshold: 0.3       # minimum confidence to report
  overlap: 0.0         # seconds of overlap between 3 s windows (1.5 = 50%)
  locale: en_us
  latitude: 0.0
  longitude: 0.0
  taxonomy_path: models/eBird_taxonomy_codes_2021E.json   # species codes
  range_filter:        # location/date plausibility filter (BirdNET meta model)
    enabled: false     # also needs the range model + non-zero lat/lon
    model_path: models/BirdNET_GLOBAL_6K_V2.4_RangeModel.onnx
    threshold: 0.01
    rerank: false      # true → multiply confidence by location score
realtime:
  audio:
    source: default    # device name, or "default"
    export:
      enabled: true
      path: clips
      retention:       # automatic clip cleanup
        enabled: false
        max_age_days: 30
output:
  sqlite:
    path: birdnet.db
```

To enable the **range filter** + species codes, fetch and convert the extras:

```bash
scripts/download_model.sh --range --convert   # adds taxonomy + range ONNX model
```

Then set `birdnet.latitude`/`longitude` and `birdnet.range_filter.enabled: true`.

Env overrides: `BIRDNET_CONFIG`, `BIRDNET_THRESHOLD`, `BIRDNET_MODEL_PATH`,
`BIRDNET_LABELS_PATH`, `BIRDNET_DB_PATH`.

## HTTP surface

| Route | Description |
|---|---|
| `POST /api/list_detections` | `#[server]` function — recent detections (JSON) |
| `POST /api/search_detections` | `#[server]` function — search by name / species code |
| `POST /api/review_detection` | `#[server]` function — mark correct / false-positive |
| `POST /api/list_audio_devices` / `set_audio_device` | `#[server]` functions — input device list / switch |
| `GET /stream` | live SSE: `detection` + `audio` + `live` (best-guess) events |
| `GET /media/clip/{id}` | the WAV clip |
| `GET /media/spectrogram/{id}` | clip rendered as a spectrogram PNG |
| `GET /` , `/pkg/*` | SSR dashboard + WASM/JS/CSS bundle |

## Tests

```bash
cargo test                  # server-side unit + route tests (default `ssr` feature)
cargo leptos build          # full fullstack build (server + WASM + bundle)

# End-to-end inference test (needs the converted model on disk):
BIRDNET_TEST_MODEL=models/BirdNET_GLOBAL_6K_V2.4.onnx \
BIRDNET_TEST_LABELS=models/BirdNET_GLOBAL_6K_V2.4_Labels_en_us.txt \
  cargo test -- --ignored
```

## Status / roadmap

Implemented: soundcard capture, resampling, windowing, ONNX inference,
confidence/dynamic/false-positive filtering, **location/date range filter**,
**eBird taxonomy (species codes)**, SQLite storage, clip export, **clip
retention**, clip + live spectrograms, a live audio monitor (VU meter +
scrolling spectrogram), and a Leptos (WASM) dashboard with live SSE updates,
**detection search, and review (mark correct / false-positive)**.

Planned: RTSP & multiple sources, Perch/Bat models, MQTT, BirdWeather,
notifications, weather + image providers, auth/OIDC, MySQL/Postgres.
