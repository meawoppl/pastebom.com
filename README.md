# pastebom.com

Shareable interactive PCB BOM viewer. Upload a KiCad, EasyEDA, Eagle, or Altium PCB file and get a link to an interactive viewer.

## Quick Start

```bash
./dev.sh start        # build and run on port 8080
./dev.sh stop         # stop the container
./dev.sh status       # check if running
```

## Architecture

- `crates/pcb-extract` — PCB file parser library + CLI
- `crates/server` — Axum web server
- `crates/viewer` — Yew WASM interactive viewer
