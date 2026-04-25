# Changelog

## 1.9.0

- Fix KiCad arc direction by honouring the (start, mid, end) point convention; thumbnail and viewer now trace the same sweep
- Extract copper-pour zones from Eagle XML (`<polygon>` under `<signal>`)
- Render inner-layer copper zones in the viewer with the existing layer toggle
- Reject OpenBoardView ASCII `.brd` files with a clear error instead of a misleading XML parse failure
- Support Eagle binary `.brd` format (pre-6.0)
- Pre-compress viewer assets at startup and serve gzip/brotli responses
- Re-parse stale boards on deploy when `parser_version` is older than the current build (so this release reprocesses previously-stored arcs and zones)
- Reconstruct recent uploads list from S3 when `recent.json` is missing
- Add SVG thumbnail endpoint with S3 cache
- Mobile sidebars and per-layer toggles; trackpad two-finger scroll pans instead of zooming
- Match viewer dark theme to landing page; brighten pull-tab handles in dark mode
- Reject macOS resource fork files and malformed GDSII archives early; ZIP/tar bomb detection for archive uploads
- Filter GDSII boards from the recent uploads list and remove GDSII from the supported-format display
- Prevent large uploads from locking up the server (parse semaphore)
- Configurable max upload size via `MAX_UPLOAD_SIZE` env var
- Clamp non-finite f64 values to 0.0 during serialization

## 1.8.0

- Add ODB++ file format support (.tgz and .zip archives)
- Content-based format detection to distinguish ODB++ from Gerber in ZIP files
- Replace manual floating-point assertions with approx crate
- Fix ODB++ profile rendering (board outline as edges, not filled polygon)
- Fix ODB++ pad positions (absolute world coordinates)
- Negate ODB++ Y axis to match viewer coordinate system
- Add Edge.Cuts, F.SilkS, and F.Fab visibility toggles with color swatches

## 1.7.3

- Add Gerber zip file parser (RS-274X copper, silkscreen, board outline)
- Add aperture macro support with expression evaluator
- Fix polygon hole rendering (multi-contour regions, evenodd fill rule)
- Add Excellon drill file parsing with see-through hole rendering
- Fix Gerber silkscreen/soldermask layer detection for EAGLE CAM output
- Fix Altium PCB 6.0 format parsing
- Add pinch-to-zoom on PCB canvas
- Handle non-JSON error responses from uploads

## 0.2.0

- Add workspace-level versioning across all crates
- Display version on upload page
- Always save uploaded files before parsing (prevent data loss)
- Show uploaded filename in viewer sidebar
- Fix Eagle parser Y-coordinate inconsistency (components now align with board outline)
- Fix Eagle .brd DTD parse failure
- Add public upload feed with secret checkbox
- Add CI pipeline (fmt, clippy, test, audit)

## 0.1.0

- Initial release
- KiCad, EasyEDA, Eagle, and Altium PCB file parsing
- Axum web server with S3/filesystem storage
- Yew WASM interactive BOM viewer
- Upload page with drag-and-drop
