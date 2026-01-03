"""FastAPI application for ibom-gist."""

import json
import logging
from datetime import datetime
from pathlib import Path
from typing import Optional

from fastapi import FastAPI, File, Form, HTTPException, UploadFile, Request
from fastapi.responses import HTMLResponse, FileResponse
from fastapi.staticfiles import StaticFiles

from .config import settings
from .services.generator import BomGenerator
from .models.bom import BomResponse, BomMeta

# Configure logging
logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL.upper()),
    format="%(asctime)-15s %(levelname)s %(message)s"
)

app = FastAPI(
    title="ibom-gist",
    description="GitHub Gist-style hosting for Interactive HTML BOMs",
    version="1.0.0"
)

# Initialize generator
generator = BomGenerator(settings.STORAGE_PATH)

# Serve static files
static_dir = Path(__file__).parent / "static"
if static_dir.exists():
    app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")


@app.get("/", response_class=HTMLResponse)
async def index():
    """Serve upload page."""
    index_path = Path(__file__).parent / "static" / "index.html"
    if index_path.exists():
        return FileResponse(str(index_path))
    # Fallback minimal page if static file missing
    return HTMLResponse(content="""
    <!DOCTYPE html>
    <html>
    <head><title>ibom-gist</title></head>
    <body>
        <h1>ibom-gist</h1>
        <p>Upload page not found. POST to /upload with a PCB file.</p>
    </body>
    </html>
    """)


@app.get("/health")
async def health():
    """Health check endpoint."""
    return {"status": "ok", "version": "1.0.0"}


@app.post("/upload", response_model=BomResponse)
async def upload(
    file: UploadFile = File(...),
    config: Optional[str] = Form(None)
):
    """Upload a PCB file and generate interactive BOM."""

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
        logging.exception("Generation error")
        raise HTTPException(
            status_code=500,
            detail={"error": "generation_error", "message": str(e)}
        )

    return BomResponse(
        id=result["id"],
        url=f"{settings.BASE_URL}/b/{result['id']}",
        filename=result["filename"],
        components=result["components"],
        created_at=datetime.utcnow().isoformat() + "Z",
        expires_at=None
    )


@app.get("/b/{bom_id}", response_class=HTMLResponse)
async def view_bom(bom_id: str):
    """View a generated BOM."""
    html = generator.get(bom_id)
    if html is None:
        raise HTTPException(status_code=404, detail="BOM not found")
    return HTMLResponse(content=html)


@app.get("/b/{bom_id}/meta")
async def bom_meta(bom_id: str):
    """Get BOM metadata."""
    meta = generator.get_meta(bom_id)
    if meta is None:
        raise HTTPException(status_code=404, detail="BOM not found")
    return meta
