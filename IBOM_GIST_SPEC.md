# ibom-gist: PCB BOM Hosting Service

A GitHub Gist-style hosting service for Interactive HTML BOMs. Upload a PCB file, get a shareable UUID link.

## Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              ibom-gist                                      │
│                                                                             │
│   User uploads .kicad_pcb / .json / .brd                                    │
│                     │                                                       │
│                     ▼                                                       │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  POST /upload                                                       │   │
│   │    → Validate file type & size                                      │   │
│   │    → Select parser (KiCad / EasyEDA / Eagle / Generic JSON)         │   │
│   │    → Parse PCB data                                                 │   │
│   │    → Generate BOM                                                   │   │
│   │    → Render self-contained HTML                                     │   │
│   │    → Store at /data/boms/{uuid}.html                                │   │
│   │    → Return https://bom.example.com/b/{uuid}                        │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                     │                                                       │
│                     ▼                                                       │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  GET /b/{uuid}                                                      │   │
│   │    → Serve /data/boms/{uuid}.html                                   │   │
│   │    → (Self-contained, no further server interaction needed)         │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Supported Input Formats

| Format | Extension | Parser | Dependencies |
|--------|-----------|--------|--------------|
| KiCad | `.kicad_pcb` | `PcbnewParser` | `pcbnew` (KiCad system lib) |
| EasyEDA | `.json` | `EasyEdaParser` | Pure Python |
| Eagle/Fusion360 | `.brd`, `.fbrd` | `FusionEagleParser` | Pure Python |
| Generic JSON | `.json` (with `pcbdata` key) | `GenericJsonParser` | Pure Python + jsonschema |

### KiCad Dependency Strategy

The `pcbnew` module is a C++ library bundled with KiCad, not pip-installable. Options:

| Approach | Pros | Cons |
|----------|------|------|
| **Docker with KiCad (recommended)** | Full support | ~1.5GB image |
| EasyEDA/Eagle only | Tiny image | Miss most users |
| Require pre-exported JSON | No deps | Friction for users |

**Recommendation:** Use KiCad's official Docker image as base.

---

## API Design

### Endpoints

#### `GET /`
Landing page with upload form.

**Response:** HTML page

#### `GET /health`
Health check for load balancer.

**Response:**
```json
{"status": "ok", "version": "1.0.0"}
```

#### `POST /upload`
Upload a PCB file and generate BOM.

**Request:**
- Content-Type: `multipart/form-data`
- Field: `file` - The PCB file
- Optional field: `config` - JSON configuration overrides

**Response (success):**
```json
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "url": "https://bom.example.com/b/a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "filename": "my-board.kicad_pcb",
  "components": 47,
  "created_at": "2025-01-03T12:34:56Z",
  "expires_at": null
}
```

**Response (error):**
```json
{
  "error": "unsupported_format",
  "message": "File type .xyz is not supported",
  "supported": [".kicad_pcb", ".json", ".brd", ".fbrd"]
}
```

**Error codes:**
| Code | HTTP | Description |
|------|------|-------------|
| `unsupported_format` | 400 | Unknown file extension |
| `file_too_large` | 413 | Exceeds MAX_UPLOAD_SIZE_MB |
| `parse_error` | 422 | Parser failed to read file |
| `generation_error` | 500 | HTML generation failed |

#### `GET /b/{uuid}`
View a generated BOM.

**Response:** The self-contained HTML file (served with `text/html` content type).

**404** if UUID not found.

#### `GET /b/{uuid}/meta`
Get metadata about a BOM (optional endpoint).

**Response:**
```json
{
  "id": "a1b2c3d4-...",
  "filename": "my-board.kicad_pcb",
  "components": 47,
  "created_at": "2025-01-03T12:34:56Z",
  "file_size_bytes": 245678,
  "format": "kicad"
}
```

#### `DELETE /b/{uuid}` (optional)
Delete a BOM. Could require a delete token returned at creation.

---

## Configuration

### Environment Variables

```bash
# Required
STORAGE_PATH=/data/boms              # Where to store generated HTMLs

# Optional
MAX_UPLOAD_SIZE_MB=50                # Max upload size (default: 50)
BOM_EXPIRY_DAYS=0                    # Auto-delete after N days (0 = never)
BASE_URL=https://bom.example.com     # For generating URLs in responses
LOG_LEVEL=info                       # debug, info, warning, error

# BOM Generation Defaults (match InteractiveHtmlBom config)
DEFAULT_DARK_MODE=false
DEFAULT_SHOW_PADS=true
DEFAULT_SHOW_SILKSCREEN=true
DEFAULT_SHOW_FABRICATION=false
DEFAULT_COMPRESSION=true
DEFAULT_INCLUDE_TRACKS=false
DEFAULT_INCLUDE_NETS=false
```

### Upload Config Override

Users can optionally pass config options in the upload:

```json
{
  "dark_mode": true,
  "show_pads": true,
  "show_silkscreen": true,
  "show_fabrication": false,
  "include_tracks": true,
  "include_nets": true,
  "highlight_pin1": "all",
  "board_rotation": 0,
  "checkboxes": "Sourced,Placed",
  "bom_view": "left-right",
  "layer_view": "FB"
}
```

---

## Storage

### Directory Structure

```
/data/boms/
├── a1/
│   └── a1b2c3d4-e5f6-7890-abcd-ef1234567890.html
├── b2/
│   └── b2c3d4e5-f6a7-8901-bcde-f23456789012.html
└── ...
```

**Sharding:** First 2 characters of UUID used as subdirectory to avoid too many files in one directory.

### Metadata Storage Options

**Option A: Filesystem (simplest)**
Store metadata in JSON sidecar files:
```
/data/boms/a1/a1b2c3d4-....html
/data/boms/a1/a1b2c3d4-....meta.json
```

**Option B: SQLite**
Single `boms.db` file with table:
```sql
CREATE TABLE boms (
    id TEXT PRIMARY KEY,
    filename TEXT,
    format TEXT,
    components INTEGER,
    file_size INTEGER,
    created_at TIMESTAMP,
    expires_at TIMESTAMP
);
```

**Option C: No metadata**
Just serve HTMLs, no tracking. Simplest but no analytics.

**Recommendation:** Start with Option A (JSON sidecars), migrate to SQLite if needed.

---

## Implementation

### Project Structure

```
ibom-gist/
├── Dockerfile
├── requirements.txt
├── app/
│   ├── __init__.py
│   ├── main.py              # FastAPI app
│   ├── config.py            # Settings from env
│   ├── routes/
│   │   ├── __init__.py
│   │   ├── upload.py        # POST /upload
│   │   ├── view.py          # GET /b/{uuid}
│   │   └── health.py        # GET /health
│   ├── services/
│   │   ├── __init__.py
│   │   ├── generator.py     # Wraps InteractiveHtmlBom
│   │   └── storage.py       # File storage operations
│   ├── models/
│   │   ├── __init__.py
│   │   └── bom.py           # Pydantic models
│   └── static/
│       └── index.html       # Upload page
├── InteractiveHtmlBom/      # Git submodule or vendored copy
│   └── ...
└── tests/
    ├── test_upload.py
    ├── test_generator.py
    └── fixtures/
        ├── sample.kicad_pcb
        ├── sample.json
        └── sample.brd
```

### Core Generator Service

The key integration point with InteractiveHtmlBom:

```python
# app/services/generator.py

import io
import json
import logging
import os
import tempfile
import uuid
from pathlib import Path

# Add InteractiveHtmlBom to path
import sys
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "InteractiveHtmlBom"))

from InteractiveHtmlBom.ecad import get_parser_by_extension
from InteractiveHtmlBom.core.ibom import generate_bom, generate_file, round_floats
from InteractiveHtmlBom.core.lzstring import LZString
from InteractiveHtmlBom.version import version

class HeadlessLogger:
    """Logger that doesn't depend on wx"""
    def __init__(self):
        self.logger = logging.getLogger('ibom-gist')

    def info(self, *args):
        self.logger.info(*args)

    def error(self, msg):
        self.logger.error(msg)

    def warn(self, msg):
        self.logger.warning(msg)


class HeadlessConfig:
    """Config that doesn't depend on wx.FileConfig"""

    def __init__(self, **kwargs):
        self.version = version

        # HTML defaults
        self.dark_mode = kwargs.get('dark_mode', False)
        self.show_pads = kwargs.get('show_pads', True)
        self.show_fabrication = kwargs.get('show_fabrication', False)
        self.show_silkscreen = kwargs.get('show_silkscreen', True)
        self.redraw_on_drag = kwargs.get('redraw_on_drag', True)
        self.highlight_pin1 = kwargs.get('highlight_pin1', 'none')
        self.board_rotation = kwargs.get('board_rotation', 0)
        self.offset_back_rotation = kwargs.get('offset_back_rotation', False)
        self.checkboxes = kwargs.get('checkboxes', 'Sourced,Placed')
        self.mark_when_checked = kwargs.get('mark_when_checked', '')
        self.bom_view = kwargs.get('bom_view', 'left-right')
        self.layer_view = kwargs.get('layer_view', 'FB')
        self.compression = kwargs.get('compression', True)
        self.open_browser = False  # Never for web service
        self.kicad_text_formatting = kwargs.get('kicad_text_formatting', True)

        # General
        self.bom_dest_dir = ''  # We handle output ourselves
        self.bom_name_format = 'ibom'
        self.component_sort_order = ['C', 'R', 'L', 'D', 'U', 'Y', 'X', 'F',
                                     'SW', 'A', '~', 'HS', 'CNN', 'J', 'P',
                                     'NT', 'MH']
        self.component_blacklist = []
        self.blacklist_virtual = kwargs.get('blacklist_virtual', True)
        self.blacklist_empty_val = kwargs.get('blacklist_empty_val', False)
        self.include_tracks = kwargs.get('include_tracks', False)
        self.include_nets = kwargs.get('include_nets', False)

        # Fields
        self.extra_data_file = None
        self.netlist_initial_directory = ''
        self.show_fields = kwargs.get('show_fields', ['Value', 'Footprint'])
        self.group_fields = kwargs.get('group_fields', ['Value', 'Footprint'])
        self.normalize_field_case = kwargs.get('normalize_field_case', False)
        self.board_variant_field = ''
        self.board_variant_whitelist = []
        self.board_variant_blacklist = []
        self.dnp_field = kwargs.get('dnp_field', '')

    def get_html_config(self):
        html_fields = [
            'dark_mode', 'show_pads', 'show_fabrication', 'show_silkscreen',
            'highlight_pin1', 'redraw_on_drag', 'board_rotation', 'checkboxes',
            'bom_view', 'layer_view', 'offset_back_rotation',
            'kicad_text_formatting', 'mark_when_checked'
        ]
        d = {f: getattr(self, f) for f in html_fields}
        d["fields"] = self.show_fields
        return json.dumps(d)


class BomGenerator:
    """Generates interactive HTML BOMs from PCB files"""

    def __init__(self, storage_path: str):
        self.storage_path = Path(storage_path)
        self.storage_path.mkdir(parents=True, exist_ok=True)
        self.logger = HeadlessLogger()

    def generate(self, file_content: bytes, filename: str,
                 config_overrides: dict = None) -> dict:
        """
        Generate an interactive HTML BOM from uploaded file.

        Args:
            file_content: Raw bytes of the uploaded file
            filename: Original filename (used to detect format)
            config_overrides: Optional config overrides from user

        Returns:
            dict with 'id', 'url', 'filename', 'components', 'created_at'

        Raises:
            ValueError: If file format unsupported
            RuntimeError: If parsing or generation fails
        """
        config = HeadlessConfig(**(config_overrides or {}))

        # Write to temp file (parsers expect file path)
        with tempfile.NamedTemporaryFile(
            suffix=os.path.splitext(filename)[1],
            delete=False
        ) as tmp:
            tmp.write(file_content)
            tmp_path = tmp.name

        try:
            # Get appropriate parser
            parser = get_parser_by_extension(tmp_path, config, self.logger)
            if parser is None:
                ext = os.path.splitext(filename)[1]
                raise ValueError(f"Unsupported file format: {ext}")

            # Parse PCB data
            pcbdata, components = parser.parse()
            if not pcbdata or not components:
                raise RuntimeError("Failed to parse PCB file")

            # Generate BOM
            pcbdata["bom"] = generate_bom(components, config)
            pcbdata["ibom_version"] = config.version

            # Generate HTML
            html_content = self._render_html(pcbdata, config)

            # Store with UUID
            bom_id = str(uuid.uuid4())
            self._store(bom_id, html_content, filename, len(components))

            return {
                'id': bom_id,
                'filename': filename,
                'components': len(components),
            }

        finally:
            os.unlink(tmp_path)

    def _render_html(self, pcbdata: dict, config: HeadlessConfig) -> str:
        """Render the self-contained HTML file"""

        def get_file_content(file_name: str) -> str:
            # Path relative to InteractiveHtmlBom package
            path = Path(__file__).parent.parent.parent / \
                   "InteractiveHtmlBom" / "InteractiveHtmlBom" / "web" / file_name
            if not path.exists():
                return ""
            return path.read_text(encoding='utf-8')

        # Generate pcbdata JavaScript
        pcbdata_str = json.dumps(round_floats(pcbdata, 6))
        if config.compression:
            pcbdata_str = json.dumps(
                LZString().compress_to_base64(pcbdata_str)
            )
            pcbdata_js = f"var pcbdata = JSON.parse(LZString.decompressFromBase64({pcbdata_str}))"
        else:
            pcbdata_js = f"var pcbdata = {pcbdata_str}"

        config_js = "var config = " + config.get_html_config()

        # Build HTML from template
        html = get_file_content("ibom.html")
        html = html.replace('///CSS///', get_file_content('ibom.css'))
        html = html.replace('///USERCSS///', get_file_content('user.css'))
        html = html.replace('///SPLITJS///', get_file_content('split.js'))
        html = html.replace('///LZ-STRING///',
                           get_file_content('lz-string.js') if config.compression else '')
        html = html.replace('///POINTER_EVENTS_POLYFILL///', get_file_content('pep.js'))
        html = html.replace('///CONFIG///', config_js)
        html = html.replace('///UTILJS///', get_file_content('util.js'))
        html = html.replace('///RENDERJS///', get_file_content('render.js'))
        html = html.replace('///TABLEUTILJS///', get_file_content('table-util.js'))
        html = html.replace('///IBOMJS///', get_file_content('ibom.js'))
        html = html.replace('///USERJS///', get_file_content('user.js'))
        html = html.replace('///USERHEADER///', get_file_content('userheader.html'))
        html = html.replace('///USERFOOTER///', get_file_content('userfooter.html'))
        html = html.replace('///PCBDATA///', pcbdata_js)

        return html

    def _store(self, bom_id: str, html_content: str,
               filename: str, components: int) -> None:
        """Store the generated HTML and metadata"""

        # Shard by first 2 chars of UUID
        shard = bom_id[:2]
        shard_dir = self.storage_path / shard
        shard_dir.mkdir(exist_ok=True)

        # Write HTML
        html_path = shard_dir / f"{bom_id}.html"
        html_path.write_text(html_content, encoding='utf-8')

        # Write metadata
        meta_path = shard_dir / f"{bom_id}.meta.json"
        meta = {
            'id': bom_id,
            'filename': filename,
            'components': components,
            'file_size': len(html_content.encode('utf-8')),
            'created_at': datetime.utcnow().isoformat() + 'Z',
        }
        meta_path.write_text(json.dumps(meta), encoding='utf-8')

    def get(self, bom_id: str) -> str | None:
        """Retrieve stored HTML by UUID"""
        shard = bom_id[:2]
        html_path = self.storage_path / shard / f"{bom_id}.html"
        if html_path.exists():
            return html_path.read_text(encoding='utf-8')
        return None

    def get_meta(self, bom_id: str) -> dict | None:
        """Retrieve metadata by UUID"""
        shard = bom_id[:2]
        meta_path = self.storage_path / shard / f"{bom_id}.meta.json"
        if meta_path.exists():
            return json.loads(meta_path.read_text(encoding='utf-8'))
        return None
```

### FastAPI Application

```python
# app/main.py

import os
from datetime import datetime
from pathlib import Path
from typing import Optional

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.responses import HTMLResponse, FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from .services.generator import BomGenerator
from .config import settings

app = FastAPI(
    title="ibom-gist",
    description="GitHub Gist-style hosting for Interactive HTML BOMs",
    version="1.0.0"
)

generator = BomGenerator(settings.STORAGE_PATH)

# Serve static upload page
app.mount("/static", StaticFiles(directory="app/static"), name="static")


class BomResponse(BaseModel):
    id: str
    url: str
    filename: str
    components: int
    created_at: str
    expires_at: Optional[str] = None


class ErrorResponse(BaseModel):
    error: str
    message: str
    supported: Optional[list[str]] = None


@app.get("/", response_class=HTMLResponse)
async def index():
    """Serve upload page"""
    return FileResponse("app/static/index.html")


@app.get("/health")
async def health():
    """Health check endpoint"""
    return {"status": "ok", "version": "1.0.0"}


@app.post("/upload", response_model=BomResponse)
async def upload(
    file: UploadFile = File(...),
    config: Optional[str] = Form(None)
):
    """Upload a PCB file and generate interactive BOM"""

    # Validate file size
    content = await file.read()
    max_size = settings.MAX_UPLOAD_SIZE_MB * 1024 * 1024
    if len(content) > max_size:
        raise HTTPException(
            status_code=413,
            detail={
                "error": "file_too_large",
                "message": f"File exceeds {settings.MAX_UPLOAD_SIZE_MB}MB limit"
            }
        )

    # Parse config overrides if provided
    config_overrides = {}
    if config:
        try:
            import json
            config_overrides = json.loads(config)
        except json.JSONDecodeError:
            raise HTTPException(
                status_code=400,
                detail={"error": "invalid_config", "message": "Config must be valid JSON"}
            )

    # Generate BOM
    try:
        result = generator.generate(content, file.filename, config_overrides)
    except ValueError as e:
        raise HTTPException(
            status_code=400,
            detail={
                "error": "unsupported_format",
                "message": str(e),
                "supported": [".kicad_pcb", ".json", ".brd", ".fbrd"]
            }
        )
    except RuntimeError as e:
        raise HTTPException(
            status_code=422,
            detail={"error": "parse_error", "message": str(e)}
        )
    except Exception as e:
        raise HTTPException(
            status_code=500,
            detail={"error": "generation_error", "message": str(e)}
        )

    return BomResponse(
        id=result['id'],
        url=f"{settings.BASE_URL}/b/{result['id']}",
        filename=result['filename'],
        components=result['components'],
        created_at=datetime.utcnow().isoformat() + 'Z',
        expires_at=None  # TODO: implement expiry
    )


@app.get("/b/{bom_id}", response_class=HTMLResponse)
async def view_bom(bom_id: str):
    """View a generated BOM"""
    html = generator.get(bom_id)
    if html is None:
        raise HTTPException(status_code=404, detail="BOM not found")
    return HTMLResponse(content=html)


@app.get("/b/{bom_id}/meta")
async def bom_meta(bom_id: str):
    """Get BOM metadata"""
    meta = generator.get_meta(bom_id)
    if meta is None:
        raise HTTPException(status_code=404, detail="BOM not found")
    return meta
```

### Settings

```python
# app/config.py

from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    STORAGE_PATH: str = "/data/boms"
    MAX_UPLOAD_SIZE_MB: int = 50
    BOM_EXPIRY_DAYS: int = 0
    BASE_URL: str = "http://localhost:8080"
    LOG_LEVEL: str = "info"

    # BOM defaults
    DEFAULT_DARK_MODE: bool = False
    DEFAULT_COMPRESSION: bool = True

    class Config:
        env_file = ".env"


settings = Settings()
```

### Upload Page

```html
<!-- app/static/index.html -->
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ibom-gist - Interactive BOM Hosting</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #1a1a2e;
            color: #eee;
            min-height: 100vh;
        }
        h1 { color: #4ecca3; margin-bottom: 0.5rem; }
        .subtitle { color: #888; margin-bottom: 2rem; }
        .upload-zone {
            border: 2px dashed #4ecca3;
            border-radius: 12px;
            padding: 3rem;
            text-align: center;
            cursor: pointer;
            transition: all 0.2s;
            background: #16213e;
        }
        .upload-zone:hover, .upload-zone.dragover {
            border-color: #7effc3;
            background: #1a2744;
        }
        .upload-zone input { display: none; }
        .formats { color: #888; font-size: 0.9rem; margin-top: 1rem; }
        .result {
            margin-top: 2rem;
            padding: 1.5rem;
            background: #16213e;
            border-radius: 8px;
            display: none;
        }
        .result.show { display: block; }
        .result a {
            color: #4ecca3;
            word-break: break-all;
            font-size: 1.1rem;
        }
        .result .meta { color: #888; margin-top: 0.5rem; font-size: 0.9rem; }
        .error { background: #3d1a1a; border: 1px solid #ff4444; }
        .loading { opacity: 0.6; pointer-events: none; }
        button {
            background: #4ecca3;
            color: #1a1a2e;
            border: none;
            padding: 0.75rem 2rem;
            border-radius: 6px;
            font-size: 1rem;
            cursor: pointer;
            margin-top: 1rem;
        }
        button:hover { background: #7effc3; }
    </style>
</head>
<body>
    <h1>ibom-gist</h1>
    <p class="subtitle">Share interactive PCB BOMs with a link</p>

    <div class="upload-zone" id="dropzone">
        <input type="file" id="fileInput" accept=".kicad_pcb,.json,.brd,.fbrd">
        <p>Drop your PCB file here or click to browse</p>
        <p class="formats">Supports: KiCad (.kicad_pcb), EasyEDA (.json), Eagle (.brd, .fbrd)</p>
    </div>

    <div class="result" id="result"></div>

    <script>
        const dropzone = document.getElementById('dropzone');
        const fileInput = document.getElementById('fileInput');
        const result = document.getElementById('result');

        dropzone.addEventListener('click', () => fileInput.click());
        dropzone.addEventListener('dragover', (e) => {
            e.preventDefault();
            dropzone.classList.add('dragover');
        });
        dropzone.addEventListener('dragleave', () => {
            dropzone.classList.remove('dragover');
        });
        dropzone.addEventListener('drop', (e) => {
            e.preventDefault();
            dropzone.classList.remove('dragover');
            if (e.dataTransfer.files.length) {
                uploadFile(e.dataTransfer.files[0]);
            }
        });
        fileInput.addEventListener('change', () => {
            if (fileInput.files.length) {
                uploadFile(fileInput.files[0]);
            }
        });

        async function uploadFile(file) {
            dropzone.classList.add('loading');
            result.className = 'result';
            result.innerHTML = 'Generating BOM...';
            result.classList.add('show');

            const formData = new FormData();
            formData.append('file', file);

            try {
                const response = await fetch('/upload', {
                    method: 'POST',
                    body: formData
                });

                const data = await response.json();

                if (!response.ok) {
                    throw new Error(data.detail?.message || data.message || 'Upload failed');
                }

                result.innerHTML = `
                    <p>Your BOM is ready:</p>
                    <a href="${data.url}" target="_blank">${data.url}</a>
                    <p class="meta">${data.filename} - ${data.components} components</p>
                    <button onclick="navigator.clipboard.writeText('${data.url}')">
                        Copy Link
                    </button>
                `;
            } catch (err) {
                result.classList.add('error');
                result.innerHTML = `<p>Error: ${err.message}</p>`;
            } finally {
                dropzone.classList.remove('loading');
            }
        }
    </script>
</body>
</html>
```

---

## Dockerfile

```dockerfile
# Dockerfile

# =============================================================================
# Stage 1: Build environment with KiCad
# =============================================================================
FROM kicad/kicad:8.0 AS base

# Install Python dependencies
RUN apt-get update && apt-get install -y \
    python3-pip \
    python3-venv \
    && rm -rf /var/lib/apt/lists/*

# Create venv to avoid system package conflicts
RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Install Python packages
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# =============================================================================
# Stage 2: Runtime
# =============================================================================
FROM kicad/kicad:8.0

# Copy venv from builder
COPY --from=base /opt/venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Set up KiCad Python path
ENV PYTHONPATH="/usr/lib/python3/dist-packages:${PYTHONPATH}"

# Create app directory
WORKDIR /app

# Copy InteractiveHtmlBom (vendored or submodule)
COPY InteractiveHtmlBom/ /app/InteractiveHtmlBom/

# Copy application
COPY app/ /app/app/

# Create data directory
RUN mkdir -p /data/boms

# Environment
ENV STORAGE_PATH=/data/boms
ENV LOG_LEVEL=info

EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Run with uvicorn
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8080"]
```

### requirements.txt

```
fastapi>=0.109.0
uvicorn[standard]>=0.27.0
python-multipart>=0.0.6
pydantic>=2.0
pydantic-settings>=2.0
jsonschema>=4.1
```

---

## Build & Deploy

### Local Development

```bash
# Clone with InteractiveHtmlBom
git clone https://github.com/meawoppl/ibom-gist.git
cd ibom-gist
git clone https://github.com/openscopeproject/InteractiveHtmlBom.git

# Create venv (need KiCad's pcbnew for full support)
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt

# Run locally (without KiCad support)
STORAGE_PATH=./data/boms uvicorn app.main:app --reload

# Or with Docker (full KiCad support)
docker build -t ibom-gist .
docker run -p 8080:8080 -v ./data:/data/boms ibom-gist
```

### GitHub Actions

```yaml
# .github/workflows/deploy.yml

name: Build and Deploy

on:
  push:
    branches: [main]

env:
  AWS_REGION: us-west-2
  ECR_REGISTRY: 877983347039.dkr.ecr.us-west-2.amazonaws.com
  ECR_REPOSITORY: ibom-gist
  EC2_HOST: ${{ secrets.EC2_HOST }}

jobs:
  build-and-deploy:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v4
        with:
          submodules: true  # If using InteractiveHtmlBom as submodule

      - name: Configure AWS credentials
        uses: aws-actions/configure-aws-credentials@v4
        with:
          aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
          aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
          aws-region: ${{ env.AWS_REGION }}

      - name: Login to Amazon ECR
        uses: aws-actions/amazon-ecr-login@v2

      - name: Build and push Docker image
        run: |
          docker build -t $ECR_REGISTRY/$ECR_REPOSITORY:latest .
          docker build -t $ECR_REGISTRY/$ECR_REPOSITORY:${{ github.sha }} .
          docker push $ECR_REGISTRY/$ECR_REPOSITORY:latest
          docker push $ECR_REGISTRY/$ECR_REPOSITORY:${{ github.sha }}

      - name: Deploy to EC2
        uses: appleboy/ssh-action@v1.0.3
        with:
          host: ${{ env.EC2_HOST }}
          username: ec2-user
          key: ${{ secrets.EC2_SSH_KEY }}
          script: |
            aws ecr get-login-password --region us-west-2 | \
              docker login --username AWS --password-stdin ${{ env.ECR_REGISTRY }}
            cd /opt/services
            docker compose pull ibom-gist
            docker compose up -d ibom-gist
            docker image prune -f
```

---

## Traefik Integration

From `docker-compose.yml` in infra repo:

```yaml
ibom-gist:
  image: 877983347039.dkr.ecr.us-west-2.amazonaws.com/ibom-gist:latest
  container_name: ibom-gist
  restart: unless-stopped
  environment:
    - STORAGE_PATH=/data/boms
    - BASE_URL=https://bom.meawoppl.com
    - MAX_UPLOAD_SIZE_MB=50
  volumes:
    - /data/boms:/data/boms
  labels:
    - "traefik.enable=true"
    - "traefik.http.routers.ibom.rule=Host(`bom.meawoppl.com`)"
    - "traefik.http.routers.ibom.entrypoints=websecure"
    - "traefik.http.routers.ibom.tls.certresolver=letsencrypt"
    - "traefik.http.services.ibom.loadbalancer.server.port=8080"
    # Increase upload size limit in Traefik
    - "traefik.http.middlewares.ibom-size.buffering.maxRequestBodyBytes=52428800"
    - "traefik.http.routers.ibom.middlewares=ibom-size"
  networks:
    - web
```

---

## Future Enhancements

### Phase 2: Nice-to-Haves

1. **Expiring links**
   - Add `expires_at` field to metadata
   - Cron job to clean up expired BOMs
   - Show expiry warning on view page

2. **Delete tokens**
   - Return `delete_token` on upload
   - `DELETE /b/{uuid}?token={delete_token}`

3. **Analytics**
   - View counts per BOM
   - Popular BOMs dashboard (opt-in)

4. **Customization UI**
   - Dark mode toggle on upload
   - BOM view options
   - Include tracks/nets checkbox

### Phase 3: Advanced

1. **Diff view**
   - Upload two versions, show differences

2. **Collaboration**
   - Comments on components
   - Checkbox state sync via WebSocket

3. **API tokens**
   - Programmatic uploads for CI/CD
   - Rate limiting per token

4. **Custom domains**
   - `your-company.ibom.dev` CNAMEs

---

## Testing

### Unit Tests

```python
# tests/test_generator.py

import pytest
from pathlib import Path
from app.services.generator import BomGenerator, HeadlessConfig

FIXTURES = Path(__file__).parent / "fixtures"


def test_headless_config_defaults():
    config = HeadlessConfig()
    assert config.compression is True
    assert config.dark_mode is False
    assert config.show_pads is True


def test_headless_config_overrides():
    config = HeadlessConfig(dark_mode=True, compression=False)
    assert config.dark_mode is True
    assert config.compression is False


@pytest.fixture
def generator(tmp_path):
    return BomGenerator(str(tmp_path / "boms"))


def test_generate_easyeda(generator):
    """Test EasyEDA JSON parsing (no KiCad dependency)"""
    sample = FIXTURES / "sample_easyeda.json"
    if not sample.exists():
        pytest.skip("No sample EasyEDA file")

    result = generator.generate(
        sample.read_bytes(),
        "sample_easyeda.json"
    )

    assert 'id' in result
    assert result['filename'] == "sample_easyeda.json"
    assert result['components'] > 0

    # Verify HTML was stored
    html = generator.get(result['id'])
    assert html is not None
    assert '<!DOCTYPE html>' in html


def test_generate_unsupported_format(generator):
    with pytest.raises(ValueError, match="Unsupported"):
        generator.generate(b"not a pcb", "file.xyz")
```

### Integration Tests

```python
# tests/test_api.py

import pytest
from fastapi.testclient import TestClient
from app.main import app

client = TestClient(app)


def test_health():
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json()["status"] == "ok"


def test_upload_no_file():
    response = client.post("/upload")
    assert response.status_code == 422


def test_upload_unsupported_format():
    response = client.post(
        "/upload",
        files={"file": ("test.xyz", b"content", "application/octet-stream")}
    )
    assert response.status_code == 400
    assert response.json()["detail"]["error"] == "unsupported_format"


def test_view_nonexistent():
    response = client.get("/b/nonexistent-uuid")
    assert response.status_code == 404
```

---

## Cost Estimate

| Resource | Monthly Cost |
|----------|--------------|
| ECR storage (~500MB images) | ~$0.50 |
| Compute (shared EC2) | (included in infra) |
| Storage (10GB BOMs) | ~$1 (EBS) |
| Data transfer | ~$1-5 |
| **Total incremental** | **~$3-7/mo** |

The Docker image with KiCad is large (~1.5GB) but ECR storage is cheap. Most cost is shared with other services.

---

## Summary

**Effort estimate:** 2-3 days for MVP

**Key decisions:**
1. Use KiCad Docker image for full format support
2. Filesystem storage with JSON metadata sidecars
3. FastAPI for simple async API
4. UUID sharding for scalability

**Dependencies on InteractiveHtmlBom:**
- `ecad/` parsers (imported at runtime)
- `core/ibom.py` for `generate_bom()` logic
- `web/` directory for HTML template and JS/CSS assets
- `core/lzstring.py` for compression

The existing InteractiveHtmlBom code does 95% of the work. This service is a thin wrapper that handles file upload, storage, and URL routing.
