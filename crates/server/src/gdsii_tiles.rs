//! GDSII tile viewer: ingest + serving routes.
//!
//! On upload of a `.gds`/`.gds2` file, [`ingest`] builds the tile set
//! (manifest + serialized BSP index + eager tiles) and stores it under
//! `gdsii/{id}/`. The routes serve the viewer shell, the manifest, and tiles
//! (pull-through cache; deep-zoom tiles are rendered on demand from the cached
//! index). Viewer assets are served from `GDS_VIEWER_DIR` at `/gview/`.

use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
};
use pcb_extract::parsers::gdsii::tile::WorldBox;
use pcb_extract::parsers::gdsii::tileset::{self, Manifest, TileIndex};
use uuid::Uuid;

use crate::AppState;

/// Zoom levels pre-rendered at ingest; deeper levels render on demand.
const EAGER_MAX_Z: u32 = 5;
const INGEST_TIMEOUT: Duration = Duration::from_secs(120);
const SEMAPHORE_TIMEOUT: Duration = Duration::from_secs(30);

fn err(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

/// Reject ids that aren't UUIDs (storage-key path-traversal guard).
fn valid_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok()
}

/// Tile `key` must be `"{layer}_{datatype}"` or `"__lod"` — restrict to a safe
/// charset so it can't escape the tile directory.
fn valid_tile_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Build and store the tile set for an uploaded GDSII file. Returns on success
/// with artifacts written under `gdsii/{id}/`.
pub async fn ingest(state: &AppState, id: &str, data: Vec<u8>) -> Result<(), String> {
    // Archive the original upload.
    let _ = state
        .s3
        .put_object(
            &format!("gdsii/{id}/raw.gds"),
            data.clone(),
            "application/octet-stream",
        )
        .await;

    // Build the tile set off the async runtime, bounded by the parse semaphore
    // and a wall-clock timeout (tiling is CPU-heavy).
    let _permit = tokio::time::timeout(SEMAPHORE_TIMEOUT, state.parse_semaphore.acquire())
        .await
        .map_err(|_| "Server busy — try again later".to_string())?
        .map_err(|_| "Server busy".to_string())?;

    let id_owned = id.to_string();
    let tileset = tokio::time::timeout(
        INGEST_TIMEOUT,
        tokio::task::spawn_blocking(move || tileset::build_tileset(&id_owned, &data, EAGER_MAX_Z)),
    )
    .await
    .map_err(|_| "GDSII tiling timed out".to_string())?
    .map_err(|_| "Tiling task failed".to_string())?
    .map_err(|e| format!("Failed to tile GDSII: {e}"))?;

    let manifest_json = serde_json::to_vec(&tileset.manifest)
        .map_err(|_| "Manifest serialization failed".to_string())?;
    state
        .s3
        .put_object(
            &format!("gdsii/{id}/manifest.json"),
            manifest_json,
            "application/json",
        )
        .await
        .map_err(|_| "Failed to store manifest".to_string())?;

    let _ = state
        .s3
        .put_object(
            &format!("gdsii/{id}/index.bin"),
            tileset.index_bytes,
            "application/octet-stream",
        )
        .await;

    for (path, body) in tileset.tiles {
        let _ = state
            .s3
            .put_object(&format!("gdsii/{id}/tiles/{path}"), body, "image/svg+xml")
            .await;
    }

    Ok(())
}

/// Serve the GDSII viewer shell at `/g/{id}` (the WASM app reads the id from the
/// path and fetches the manifest).
pub async fn get_gds_viewer(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    if state
        .s3
        .get_object(&format!("gdsii/{id}/manifest.json"))
        .await
        .is_err()
    {
        return err(StatusCode::NOT_FOUND, "GDSII view not found");
    }
    let index_path = state.gds_viewer_dir.join("index.html");
    match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Viewer not available"),
    }
}

/// Serve the tile-set manifest.
pub async fn get_gds_manifest(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    match state
        .s3
        .get_object(&format!("gdsii/{id}/manifest.json"))
        .await
    {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "public, max-age=300"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => err(StatusCode::NOT_FOUND, "manifest not found"),
    }
}

/// Serve one tile, `{key}` = `"{layer}_{datatype}"` or `"__lod"`. Pull-through
/// cache: a miss for a deep-zoom tile is rendered on demand from the cached
/// index and stored. Bodies are gzipped SVG (`Content-Encoding: gzip`).
pub async fn get_gds_tile(
    State(state): State<AppState>,
    Path((id, z, x, y, key)): Path<(String, u32, u32, u32, String)>,
) -> Response {
    if !valid_id(&id) || !valid_tile_key(&key) {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    let tile_key = format!("gdsii/{id}/tiles/{z}/{x}/{y}/{key}.svgz");

    if let Ok(cached) = state.s3.get_object(&tile_key).await {
        return svgz_response(cached);
    }

    // Cache miss — render this tile on demand from the cached index + manifest.
    let Ok(manifest_bytes) = state
        .s3
        .get_object(&format!("gdsii/{id}/manifest.json"))
        .await
    else {
        return err(StatusCode::NOT_FOUND, "view not found");
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&manifest_bytes) else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "bad manifest");
    };
    if z > manifest.zoom.max {
        return err(StatusCode::NOT_FOUND, "zoom level out of range");
    }
    let Ok(index_bytes) = state.s3.get_object(&format!("gdsii/{id}/index.bin")).await else {
        return err(StatusCode::NOT_FOUND, "index not found");
    };

    let bounds = WorldBox {
        minx: manifest.extent_nm.minx,
        miny: manifest.extent_nm.miny,
        maxx: manifest.extent_nm.maxx,
        maxy: manifest.extent_nm.maxy,
    };
    let blobs = match tokio::task::spawn_blocking(move || {
        TileIndex::from_bytes(&index_bytes)
            .map(|index| tileset::render_tile(bounds, &index, z, x, y))
    })
    .await
    {
        Ok(Ok(b)) => b,
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "tile render failed"),
    };

    // Store every layer blob for this tile; return the requested one.
    let want = format!("{z}/{x}/{y}/{key}.svgz");
    let mut found: Option<Vec<u8>> = None;
    for (path, body) in blobs {
        if path == want {
            found = Some(body.clone());
        }
        let _ = state
            .s3
            .put_object(&format!("gdsii/{id}/tiles/{path}"), body, "image/svg+xml")
            .await;
    }
    match found {
        Some(body) => svgz_response(body),
        // Empty tile for this layer — 204 so the viewer simply skips it.
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

fn svgz_response(body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CONTENT_ENCODING, "gzip"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
        .into_response()
}

/// Serve GDSII viewer static assets from `GDS_VIEWER_DIR` at `/gview/`, with an
/// SPA fallback to `index.html`.
pub async fn serve_gview(State(state): State<AppState>, uri: Uri) -> Response {
    let rel = uri.path().trim_start_matches("/gview/");
    let rel = if rel.is_empty() { "index.html" } else { rel };
    // Reject traversal; only serve plain relative paths.
    if rel.contains("..") {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    let path = state.gds_viewer_dir.join(rel);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => match tokio::fs::read(state.gds_viewer_dir.join("index.html")).await {
            Ok(b) => b,
            Err(_) => return err(StatusCode::NOT_FOUND, "asset not found"),
        },
    };
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime)],
        Body::from(bytes),
    )
        .into_response()
}
