# Changelog

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
