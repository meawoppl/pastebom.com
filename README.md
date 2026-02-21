# pastebom.com

Shareable interactive PCB BOM viewer. Upload a PCB design file, get a link to an interactive board viewer with BOM table.

## How It Works

1. **Upload** — Drop a PCB file onto the web form. The server parses it into a format-independent JSON representation containing footprints, pads, tracks, zones, nets, and board outline geometry.

2. **Store** — The parsed JSON and original file are stored under a unique ID (filesystem locally, S3 in production).

3. **View** — The shareable link `/b/{id}` loads a Yew WASM app that fetches the JSON and renders the board interactively on HTML5 Canvas. The viewer supports:
   - Front/back board views with symmetric rendering
   - Click-to-highlight nets and components across all copper layers
   - Interactive BOM table with search, sorting, and grouping
   - Layer visibility toggles with color key
   - Pan, zoom, and board rotation
   - Dark mode with persistent settings via localStorage

## Supported Formats

| EDA Tool | Extension | Notes |
|----------|-----------|-------|
| KiCad | `.kicad_pcb` | S-expression format |
| EasyEDA | `.json` | Exported JSON |
| Eagle | `.brd`, `.fbrd` | XML board files |
| Altium | `.pcbdoc` | Binary compound document |

## Architecture

Cargo workspace with three crates:

```
crates/
├── pcb-extract/    Parser library + CLI. Reads PCB files, outputs JSON.
├── server/         Axum web server. Handles uploads, serves viewer.
└── viewer/         Yew WASM frontend. Canvas-based board rendering.
```

**Upload flow:** Browser POST `/upload` → server detects format from extension → `pcb-extract` parses to intermediate types → serialized to JSON → stored with UUID → returns viewer URL.

**Viewer flow:** Browser loads `/b/{id}` → WASM app fetches `/b/{id}/data` → deserializes into typed Rust structs → renders board layers on stacked canvases (background, fabrication, silkscreen, highlight).

**Storage:** Dual-backend `S3Client` — uses S3 when `S3_BUCKET` is set, otherwise writes to `STORAGE_PATH` on disk.

## Quick Start

```bash
./dev.sh start        # build Docker image and run on port 8080
./dev.sh start 9000   # run on a different port
./dev.sh stop         # stop the container
./dev.sh status       # check if running and print URL
```

### Local Development (without Docker)

```bash
# Build viewer WASM (requires trunk and wasm32-unknown-unknown target)
cd crates/viewer && trunk build --release && cd ../..

# Run server
STORAGE_PATH=./localdata cargo run -p pastebom-server
```

## API

| Method | Route | Description |
|--------|-------|-------------|
| `GET` | `/` | Upload form |
| `POST` | `/upload` | Parse PCB file, store, return viewer link |
| `GET` | `/b/{id}` | Interactive viewer |
| `GET` | `/b/{id}/data` | Parsed PCB data (JSON) |
| `GET` | `/b/{id}/meta` | Upload metadata |
| `GET` | `/health` | Health check |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `BIND_ADDR` | `0.0.0.0:8000` | Server listen address |
| `VIEWER_DIR` | `crates/viewer/dist` | Path to built WASM assets |
| `STORAGE_PATH` | `./data` | Filesystem storage root (when S3 is not configured) |
| `S3_BUCKET` | — | S3 bucket name; enables S3 backend when set |
| `S3_PREFIX` | — | Key prefix for S3 objects |
| `BASE_URL` | `http://localhost:8000` | Base URL for generated sharing links |

## Build

Docker multi-stage build: Rust 1.93 builder compiles server binaries and WASM viewer via Trunk, then copies artifacts into a minimal Debian slim runtime image.

```bash
cargo build                                          # build all crates
cargo test                                           # run tests
cargo clippy -- -W clippy::all                       # lint
cd crates/viewer && trunk build --release            # build WASM
```
