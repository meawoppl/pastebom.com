# Work Unit 2: Interactive Viewer (Yew + Axum)

A Rust web application replacing both the pastebom.com Python backend (FastAPI → Axum) and the InteractiveHtmlBom JavaScript viewer (vanilla JS → Yew/WASM). The viewer must reach full feature parity with InteractiveHtmlBom's HTML viewer.

## Motivation

The current stack has two layers of indirection: a Python web server that shells out to a Python library that wraps a C++ library. Replacing this with a single Rust binary that serves an embedded WASM viewer eliminates all of these dependencies and produces a ~20MB container instead of ~1.5GB.

The viewer itself is ~3000 lines of JavaScript across 5 files. Porting to Yew gives us type safety on the pcbdata schema, shared types between frontend and backend, and the ability to compile the PCB extractor (work unit 1) to WASM for client-side parsing in the future.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                     Rust Binary                          │
│                                                          │
│  ┌──────────────┐    ┌─────────────────────────────┐    │
│  │  Axum Server  │    │   pcb-extract (lib, WU1)    │    │
│  │              │    │                              │    │
│  │  POST /upload ├───►│  parse() → PcbData          │    │
│  │  GET /b/{id}  │    │  bom()   → BomData          │    │
│  │  GET /health  │    └─────────────────────────────┘    │
│  │  GET /        │                                       │
│  │  GET /assets  ├──► Embedded WASM + JS + CSS           │
│  └──────────────┘                                        │
│                                                          │
│  ┌──────────────┐                                        │
│  │  S3 Storage   │    Uploads, generated BOMs, metadata  │
│  └──────────────┘                                        │
└─────────────────────────────────────────────────────────┘
```

## Part A: Axum Backend

### Endpoints (1:1 with current FastAPI app)

| Endpoint | Method | Current Behavior | Notes |
|----------|--------|-----------------|-------|
| `/` | GET | Serve upload page HTML | Serve from embedded static assets |
| `/health` | GET | `{"status":"ok","version":"1.0.0"}` | Unchanged |
| `/upload` | POST | Multipart: `file` + optional `config` JSON | Use `axum-multipart` or `multer` |
| `/b/{id}` | GET | 302 redirect to S3 BOM URL | Unchanged |
| `/b/{id}/meta` | GET | Return metadata JSON from S3 | Unchanged |

### Request/Response Types

```rust
#[derive(Serialize)]
struct BomResponse {
    id: String,
    url: String,
    filename: String,
    components: usize,
    created_at: String,       // ISO 8601 with Z
    expires_at: Option<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,            // error code
    message: String,          // user-facing message
    #[serde(skip_serializing_if = "Option::is_none")]
    supported: Option<Vec<String>>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

#[derive(Serialize)]
struct BomMeta {
    id: String,
    filename: String,
    components: usize,
    file_size: usize,
    created_at: String,
}
```

### Error Codes (match current behavior)

| Code | HTTP Status | Condition |
|------|-------------|-----------|
| `unsupported_format` | 400 | Unknown file extension |
| `invalid_config` | 400 | Config JSON parse failure |
| `file_too_large` | 413 | Exceeds MAX_UPLOAD_SIZE_MB |
| `parse_error` | 422 | PCB parser failed |
| `generation_error` | 500 | HTML generation or S3 storage failed |

### Configuration (environment variables, match current)

```rust
struct Config {
    max_upload_size_mb: usize,       // default: 50
    bom_expiry_days: usize,          // default: 0 (never)
    base_url: String,                // default: "http://localhost:8080"
    log_level: String,               // default: "info"
    s3_bucket: String,               // required
    s3_region: String,               // default: "us-west-2"
    s3_endpoint_url: Option<String>, // for LocalStack/MinIO
    s3_uploads_prefix: String,       // default: "uploads"
    s3_boms_prefix: String,          // default: "boms"
    s3_failed_prefix: String,        // default: "failed"
}
```

### Upload Flow

1. Accept multipart form with `file` field and optional `config` field
2. Validate file size against `MAX_UPLOAD_SIZE_MB`
3. Parse `config` JSON if present (viewer config overrides)
4. Generate UUID
5. Store original upload to `s3://{bucket}/{uploads_prefix}/{uuid}/{filename}`
6. Call `pcb_extract::extract_bytes(content, format, opts)` (work unit 1)
7. Generate BOM data from extracted pcbdata
8. Render self-contained HTML (embed viewer WASM/JS + pcbdata)
9. Store HTML to `s3://{bucket}/{boms_prefix}/{uuid}.html`
10. Store metadata to `s3://{bucket}/{boms_prefix}/{uuid}.meta.json`
11. On failure: move to `s3://{bucket}/{failed_prefix}/{uuid}/` with error.txt
12. Return `BomResponse` with URL

### HTML Generation

The generated BOM HTML must be self-contained (single file, works offline). Two approaches:

**Option A: Embed Yew WASM in HTML (preferred)**

Generate an HTML file that contains:
- Inline CSS (viewer styles)
- Inline WASM binary (base64-encoded) + JS glue code
- Inline pcbdata JSON (optionally LZ-compressed)
- Inline config JSON

The JS glue instantiates the WASM module with the pcbdata. This is analogous to how InteractiveHtmlBom embeds all JS inline.

**Option B: Embed the current InteractiveHtmlBom JS viewer**

During the transition, generate HTML using the existing JavaScript viewer (render.js, ibom.js, etc.) bundled as static assets. This allows incremental migration — the backend moves to Rust first, the viewer migrates later.

Recommendation: Start with Option B for faster backend migration, then switch to Option A when the Yew viewer reaches parity.

### S3 Storage

Use `aws-sdk-s3` (the official AWS SDK for Rust):

```rust
struct S3Storage {
    client: aws_sdk_s3::Client,
    bucket: String,
    uploads_prefix: String,
    boms_prefix: String,
    failed_prefix: String,
    region: String,
    endpoint_url: Option<String>,
}

impl S3Storage {
    async fn store_upload(&self, bom_id: &str, filename: &str, content: &[u8]) -> Result<()>;
    async fn store_bom(&self, bom_id: &str, html: &str, meta: &BomMeta) -> Result<()>;
    async fn get_bom_url(&self, bom_id: &str) -> String;
    async fn get_meta(&self, bom_id: &str) -> Result<Option<BomMeta>>;
    async fn bom_exists(&self, bom_id: &str) -> Result<bool>;
    async fn move_to_failed(&self, bom_id: &str, filename: &str, error: &str) -> Result<()>;
}
```

## Part B: Yew Viewer (Feature Parity with InteractiveHtmlBom)

The viewer is the largest piece. InteractiveHtmlBom's viewer consists of:

| File | Lines | Purpose |
|------|-------|---------|
| `render.js` | ~800 | Canvas 2D rendering engine |
| `ibom.js` | ~1100 | Application logic, event handling, BOM table |
| `util.js` | ~500 | Utilities, settings persistence, storage |
| `table-util.js` | ~200 | BOM table column resize/drag |
| `split.js` | ~400 | Resizable split panes (third-party) |
| `ibom.css` | ~600 | All styles |
| `lz-string.js` | ~200 | LZString decompression |
| `pep.js` | ~300 | Pointer events polyfill |

Total: ~4100 lines of application code to port.

### Canvas Architecture

The viewer uses **8 HTML5 Canvas 2D** elements, stacked with z-index:

```
Per side (F and B):
  Layer 0: bg       — tracks, zones, pads, edge cuts
  Layer 1: fab      — fabrication drawings
  Layer 2: silk     — silkscreen drawings
  Layer 3: highlight — selected component/net highlights
```

In Yew, these canvases are created as `<canvas>` elements via `html!{}` and drawn to via `web_sys::CanvasRenderingContext2d`.

### Rendering Pipeline

On each `redrawCanvas()` call:

```
1. Clear all 4 canvases for the active side(s)
2. Apply transform: translate(panx, pany) → scale(zoom) → translate(boardCenter)
3. Background canvas:
   a. drawEdgeCuts() — board outline from edges[]
   b. drawFootprints() — all pads + copper drawings
   c. drawNets() — zones (filled polygons), tracks (line segments), via drills
4. Fabrication canvas:
   a. Draw fabrication drawings (if visible)
5. Silkscreen canvas:
   a. Draw silkscreen drawings (if visible)
6. Highlight canvas:
   a. Draw highlighted component bounding boxes
   b. Draw highlighted net (all pads + tracks + zones in net)
```

### Drawing Primitives to Implement

Each drawing type from the pcbdata schema needs a canvas renderer:

| Drawing Type | Canvas API |
|-------------|-----------|
| `segment` | `ctx.moveTo()` + `ctx.lineTo()` + `ctx.stroke()` with `lineWidth` |
| `rect` | `ctx.rect()` or manual corners |
| `circle` | `ctx.arc(cx, cy, r, 0, 2π)` |
| `arc` | `ctx.arc()` with start/end angles |
| `curve` | `ctx.bezierCurveTo()` |
| `polygon` | `ctx.moveTo()` + `ctx.lineTo()` loop + `ctx.fill()` |
| `svgpath` | `Path2D(svgpath)` + `ctx.fill()` or `ctx.stroke()` |
| text (stroke font) | Glyph-by-glyph `ctx.lineTo()` using `font_data` |

### Pad Shape Rendering

Pads are the most complex drawing element. InteractiveHtmlBom caches `Path2D` objects per pad shape:

| Shape | Rendering |
|-------|-----------|
| `rect` | Rectangle path |
| `circle` | `arc(0, 0, r, 0, 2π)` |
| `oval` | Two semicircles + straight edges |
| `roundrect` | Rectangle with `arcTo()` rounded corners, radius from `radius` field |
| `chamfrect` | Rectangle with chamfered corners per `chamfpos` bitmask |
| `custom` | `Path2D(svgpath)` or polygon fill |

Through-hole pads additionally render a drill hole (circle or oblong).

### Coordinate Transform System

The viewer maintains a transform state per side:

```rust
struct Transform {
    x: f64,      // board center X offset
    y: f64,      // board center Y offset
    s: f64,      // base scale (fit board to canvas)
    panx: f64,   // user pan X
    pany: f64,   // user pan Y
    zoom: f64,   // user zoom level
}
```

Screen-to-board coordinate conversion:
```
board_x = (screen_x * dpr / zoom - panx - x) / s
board_y = (screen_y * dpr / zoom - y - pany) / s
```

For the back side, X is negated (horizontal flip).

Board rotation is applied as an additional rotation transform.

### Hit Detection

Two types of hit detection on canvas click:

**Component hit (`bboxHitScan`):**
- Transform click point to board coordinates
- Test against each footprint's bounding box (oriented rectangle test)
- Return the first matching footprint index

**Net hit (`netHitScan`):**
- Transform click point to board coordinates
- Test against each pad (shape-aware point-in-pad test)
- Test against each track segment (point-to-segment distance < width/2)
- Return the net name of the first hit

### BOM Table

The BOM table has three modes:

| Mode | Content |
|------|---------|
| Grouped | Components grouped by value+footprint, one row per group |
| Ungrouped | One row per component |
| Netlist | One row per net, showing connected pads |

Table features:
- Column headers: checkboxes (configurable), Reference, Value, Footprint, Quantity
- Click row → highlight component(s) on canvas
- Text filter → filter rows by any field substring match
- Ref lookup → jump to specific reference designator
- Column drag-resize via mouse
- Sort by any column
- CSV/TXT export

### Settings Panel

All settings are persisted to `localStorage` per board (keyed by board title + revision):

| Setting | Type | Default | Effect |
|---------|------|---------|--------|
| Dark mode | bool | from config | Toggle CSS class on root |
| Board rotation | slider | 0 | 0-360 degrees |
| Fullscreen | bool | false | Toggle browser fullscreen |
| Show references | bool | true | Toggle ref text on silkscreen |
| Show values | bool | true | Toggle value text on silkscreen |
| Show pads | bool | true | Toggle pad rendering |
| Show fabrication | bool | false | Toggle fab layer |
| Show silkscreen | bool | true | Toggle silk layer |
| Highlight pin1 | enum | "none" | "none" / "all" / "selected" |
| Include tracks | bool | false | Toggle track/zone rendering |
| Redraw on drag | bool | true | Redraw while panning or wait for mouseup |

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Up/Down | Navigate BOM rows |
| Left/Right | Rotate board ±5° |
| `n` | Mark current row's first unchecked checkbox, advance to next |
| `Alt+F` | Focus filter input |
| `Alt+R` | Focus ref lookup input |
| `Alt+Z` | BOM-only layout |
| `Alt+X` | Left-right layout |
| `Alt+C` | Top-bottom layout |
| `Alt+V` | Front canvas only |
| `Alt+B` | Front + Back canvas |
| `Alt+N` | Back canvas only |
| `Alt+1` through `Alt+9` | Toggle BOM checkbox columns |

### Mouse/Touch Interactions

| Input | Action |
|-------|--------|
| Click on canvas | Highlight component under cursor |
| Click on track/pad | Highlight net |
| Click-drag | Pan |
| Scroll wheel | Zoom (centered on cursor) |
| Pinch (touch) | Zoom |
| Right-click / Double-tap | Reset view to fit board |

### Layout Modes

Three layout modes controlled by toolbar buttons:

| Mode | Layout |
|------|--------|
| BOM-only | Full-width BOM table, no canvas |
| Left-right | Canvas left, BOM right (resizable split) |
| Top-bottom | Canvas top, BOM bottom (resizable split) |

Each mode can show Front, Back, or both canvases.

### Color Theming

All PCB colors are defined as CSS custom properties. The Yew component should read them at draw time:

```css
:root {
  --pcb-edge-color: black;
  --pad-color: #878787;
  --pad-hole-color: #CCCCCC;
  --pad-color-highlight: #D04040;
  --pad-color-highlight-both: #D0D040;
  --pad-color-highlight-marked: #44a344;
  --pin1-outline-color: #ffb629;
  --silkscreen-edge-color: #aa4;
  --silkscreen-polygon-color: #4aa;
  --silkscreen-text-color: #4aa;
  --fabrication-edge-color: #907651;
  --fabrication-polygon-color: #907651;
  --fabrication-text-color: #a27c24;
  --track-color: #def5f1;
  --track-color-highlight: #D04040;
  --zone-color: #def5f1;
  --zone-color-highlight: #d0404080;
}
```

Dark mode overrides all of these with a `.dark` class on the root element.

### Data Flow in Yew

```
App (root component)
├── Toolbar
│   ├── LayoutToggle (bom-only / left-right / top-bottom)
│   ├── LayerToggle (F / FB / B)
│   ├── BomModeToggle (grouped / ungrouped / netlist)
│   └── SettingsMenu
├── SplitPane
│   ├── CanvasView
│   │   ├── FrontCanvas (bg + fab + silk + highlight)
│   │   └── BackCanvas (bg + fab + silk + highlight)
│   └── BomTable
│       ├── FilterInput
│       ├── TableHeader (sortable, resizable)
│       └── TableRows (clickable, with checkboxes)
└── StatsFooter
```

State management via Yew's `use_reducer` or a shared context:

```rust
struct AppState {
    pcbdata: PcbData,
    config: ViewerConfig,
    highlighted_refs: Vec<usize>,       // footprint indices
    highlighted_net: Option<String>,
    bom_mode: BomMode,                   // Grouped / Ungrouped / Netlist
    layout_mode: LayoutMode,             // BomOnly / LeftRight / TopBottom
    layer_mode: LayerMode,               // Front / FrontBack / Back
    filter_text: String,
    settings: Settings,                  // persisted to localStorage
    checkbox_state: HashMap<usize, Vec<bool>>,
}
```

### LZString Decompression

When pcbdata is LZ-compressed, the viewer must decompress it. Options:

1. **Port LZString to Rust/WASM**: The algorithm is simple (~200 lines). Port it directly.
2. **Use a crate**: No well-maintained `lz-string` crate exists for Rust as of 2025.
3. **Call the JS implementation**: Use `wasm-bindgen` to call the existing `lz-string.js`.

Option 1 is cleanest for a self-contained binary.

### Image Export

The viewer supports saving the board as a PNG image:
- Configurable dimensions (width × height)
- Transparent background option
- Renders the current view to an offscreen canvas
- Triggers browser download

Implement via `web_sys::HtmlCanvasElement::to_blob()`.

### BOM Export

- CSV export: all BOM columns, quoted fields
- TXT export: tab-separated
- Settings import/export: JSON blob of all settings

## Upload Page

The upload page (`/`) is a simple HTML form — not part of the Yew viewer. It can remain as a static HTML file served by Axum:

- Drag-and-drop upload zone
- File browser input (accept: `.kicad_pcb,.json,.brd,.fbrd`)
- Advanced options (dark mode, tracks, nets, fabrication)
- Spinner during upload
- Result display with copy-to-clipboard
- Recent BOMs list from localStorage

This page is currently `app/static/index.html` (~326 lines). It can be embedded in the Rust binary via `include_str!()` or served from a static assets directory.

## Crate Dependencies

### Backend (Axum)

| Crate | Purpose |
|-------|---------|
| `axum` | HTTP framework |
| `tokio` | Async runtime |
| `tower` | Middleware (CORS, logging, etc.) |
| `aws-sdk-s3` | S3 client |
| `uuid` | UUID generation |
| `serde` + `serde_json` | Serialization |
| `tracing` + `tracing-subscriber` | Structured logging |
| `pcb-extract` (WU1) | PCB parsing library |

### Frontend (Yew)

| Crate | Purpose |
|-------|---------|
| `yew` | Component framework |
| `web-sys` | Canvas API, DOM, localStorage |
| `wasm-bindgen` | JS interop |
| `gloo` | Browser APIs (events, timers, storage) |
| `serde` + `serde_json` | Deserialize pcbdata |
| `wasm-bindgen-futures` | Async in WASM |

## Project Structure

```
pastebom/
├── Cargo.toml                    # Workspace
├── crates/
│   ├── pcb-extract/              # Work unit 1 (library)
│   ├── server/                   # Axum backend
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs           # Entry point, router setup
│   │       ├── config.rs         # Environment config
│   │       ├── routes/
│   │       │   ├── mod.rs
│   │       │   ├── upload.rs     # POST /upload
│   │       │   ├── view.rs       # GET /b/{id}
│   │       │   └── health.rs     # GET /health
│   │       ├── storage.rs        # S3 operations
│   │       ├── generator.rs      # Orchestrate parse → render → store
│   │       └── assets.rs         # Embedded static files
│   └── viewer/                   # Yew WASM frontend
│       ├── Cargo.toml
│       ├── index.html            # WASM host page
│       └── src/
│           ├── lib.rs            # Yew app entry
│           ├── app.rs            # Root component
│           ├── state.rs          # AppState, actions, reducer
│           ├── components/
│           │   ├── mod.rs
│           │   ├── toolbar.rs
│           │   ├── canvas.rs     # PCB canvas rendering
│           │   ├── bom_table.rs  # BOM table
│           │   ├── settings.rs   # Settings panel
│           │   ├── split_pane.rs # Resizable split
│           │   └── filter.rs     # Search/filter input
│           ├── render/
│           │   ├── mod.rs
│           │   ├── draw.rs       # Drawing primitives
│           │   ├── pads.rs       # Pad shape rendering
│           │   ├── text.rs       # Stroke font text rendering
│           │   ├── hit_test.rs   # Component/net hit detection
│           │   └── colors.rs     # CSS variable color reading
│           └── lzstring.rs       # LZString decompression
├── static/
│   └── index.html                # Upload page
├── Dockerfile
└── docker-compose.yml
```

## Build Process

```bash
# Build WASM viewer
cd crates/viewer
trunk build --release     # produces dist/ with WASM + JS + HTML

# Build server (embeds viewer dist/)
cd crates/server
cargo build --release     # single binary with embedded assets

# Docker
docker build -t pastebom .
```

The Dockerfile becomes a simple multi-stage Rust build — no KiCad dependency:

```dockerfile
FROM rust:1.83 AS builder
WORKDIR /app
RUN cargo install trunk
COPY . .
RUN cd crates/viewer && trunk build --release
RUN cargo build --release --bin server

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/server /usr/local/bin/pastebom
EXPOSE 8080
CMD ["pastebom"]
```

Image size: ~20-50MB vs current ~1.5GB.

## Testing Strategy

### Backend Tests

- Unit tests for each route handler (mock S3)
- Integration test with LocalStack S3
- Multipart upload parsing tests
- Config validation tests

### Viewer Tests

- Snapshot rendering tests: render a known pcbdata to canvas, compare pixel output
- Component interaction tests via `wasm-bindgen-test`
- BOM table filtering/sorting unit tests
- Hit detection unit tests with known coordinates
- LZString round-trip tests

### Cross-Validation

Parse a PCB file with both the Python (InteractiveHtmlBom) and Rust (pcb-extract) pipelines. Compare:
1. pcbdata JSON output (should be identical)
2. Rendered canvas pixels (should be visually identical)

## Migration Path

1. **Phase 1**: Axum backend with InteractiveHtmlBom JS viewer (Option B above). Backend is Rust, viewer is unchanged JS. This validates the backend without requiring the Yew viewer.

2. **Phase 2**: Yew viewer consuming pcbdata JSON. Run both viewers side-by-side for comparison testing.

3. **Phase 3**: Switch generated HTML to embed Yew WASM instead of JS. Remove InteractiveHtmlBom dependency entirely.

## Risks and Unknowns

1. **WASM binary size.** A Yew app compiles to ~500KB-2MB of WASM. Embedding this in every generated HTML file may be too large. Mitigation: serve WASM from CDN/server instead of embedding, or use aggressive `wasm-opt` and `wasm-snip`.

2. **Canvas performance in WASM.** Each canvas draw call crosses the WASM-JS boundary. For boards with thousands of pads, this could be slow. Mitigation: batch draw calls, use `Path2D` objects, minimize per-frame allocations.

3. **Split pane library.** InteractiveHtmlBom uses `split.js` for resizable panes. No Yew equivalent exists. Need to implement from scratch or use a JS interop wrapper.

4. **localStorage from WASM.** Straightforward via `web_sys::window().local_storage()`, but serialization overhead may be noticeable for large settings objects.

5. **Self-contained HTML constraint.** The generated BOM must work offline as a single HTML file. Embedding WASM inline (base64) is possible but increases file size. The current JS approach produces ~500KB-1MB files; WASM could push this to 2-3MB.

## Source References

| Resource | URL |
|----------|-----|
| InteractiveHtmlBom render.js | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/render.js |
| InteractiveHtmlBom ibom.js | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/ibom.js |
| InteractiveHtmlBom util.js | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/util.js |
| InteractiveHtmlBom table-util.js | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/table-util.js |
| InteractiveHtmlBom ibom.css | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/ibom.css |
| InteractiveHtmlBom ibom.html | https://github.com/openscopeproject/InteractiveHtmlBom/blob/master/InteractiveHtmlBom/web/ibom.html |
| Yew framework | https://yew.rs |
| Yew Canvas example | https://github.com/yewstack/yew/tree/master/examples/webgl |
| web-sys CanvasRenderingContext2d | https://rustwasm.github.io/wasm-bindgen/api/web_sys/struct.CanvasRenderingContext2d.html |
| Axum framework | https://github.com/tokio-rs/axum |
| aws-sdk-s3 | https://docs.rs/aws-sdk-s3/latest |
| trunk (WASM build tool) | https://trunkrs.dev |
| Current pastebom.com source | app/main.py, app/services/generator.py, app/services/storage.py |
