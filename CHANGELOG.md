# Changelog

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
