use axum::{
    extract::{multipart::MultipartRejection, DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use pcb_extract::ExtractOptions;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

const MAX_RECENT: usize = 50;
const RECENT_KEY: &str = "recent.json";
const SEMAPHORE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PARSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Decompressed KiCad file budget for one project upload.
const MAX_PROJECT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub enum ParseError {
    /// No parse slot was free in time; retrying later may succeed.
    Busy,
    /// The file itself could not be parsed (error, panic, or timeout).
    Failed(String),
}

impl ParseError {
    pub fn status(&self) -> StatusCode {
        match self {
            ParseError::Busy => StatusCode::SERVICE_UNAVAILABLE,
            ParseError::Failed(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Busy => write!(f, "Server busy — try again later"),
            ParseError::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

/// Acquire parse semaphore (with timeout) and run PCB extraction off the async runtime.
pub async fn parse_pcb_guarded(
    state: &AppState,
    data: Vec<u8>,
    format: pcb_extract::PcbFormat,
) -> Result<pcb_extract::types::PcbData, ParseError> {
    let _permit = tokio::time::timeout(SEMAPHORE_TIMEOUT, state.parse_semaphore.acquire())
        .await
        .map_err(|_| ParseError::Busy)?
        .map_err(|_| ParseError::Busy)?;
    let handle = tokio::task::spawn_blocking(move || {
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: true,
        };
        pcb_extract::extract_bytes(&data, format, &opts)
    });
    // Bound parse wall-clock time so a pathological file can't hold the permit
    // indefinitely. On timeout the permit is released here; the blocking task
    // finishes on its own (parser work is itself bounded).
    let result = tokio::time::timeout(PARSE_TIMEOUT, handle)
        .await
        .map_err(|_| ParseError::Failed("Parsing timed out".to_string()))?
        .map_err(|_| ParseError::Failed("Parse task failed".to_string()))?
        .map_err(|e| ParseError::Failed(format!("Failed to parse PCB file: {e}")))?;
    Ok(result)
}

/// Prepend an entry to the recent list and persist to storage.
pub async fn add_recent(state: &AppState, entry: RecentEntry) {
    // Serialize under the lock, then drop it before the S3 round-trip so
    // uploads and /api/recent readers don't serialize on the network call.
    let json = {
        let mut recent = state.recent.write().await;
        recent.insert(0, entry);
        recent.truncate(MAX_RECENT);
        serde_json::to_vec(&*recent)
    };
    if let Ok(json) = json {
        let _ = state
            .s3
            .put_object(RECENT_KEY, json, "application/json")
            .await;
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RecentEntry {
    pub id: String,
    pub filename: String,
    pub components: usize,
    #[serde(default)]
    pub file_size: usize,
    pub created: String,
}

/// Reduce a client-supplied filename to a single safe path component for use
/// in a storage key, preventing traversal (`../`) on the filesystem backend.
fn safe_filename(name: &str) -> String {
    let base = std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "upload.bin".to_string()
    } else {
        cleaned
    }
}

/// Reject ids that aren't UUIDs so they can't traverse the storage root.
fn validate_id(id: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "BOM not found"))
}

fn is_supported_format(filename: &str) -> bool {
    let name = filename.to_lowercase();
    !name.ends_with(".gds") && !name.ends_with(".gds2")
}

pub async fn load_recent(s3: &crate::s3::S3Client) -> Vec<RecentEntry> {
    if let Ok(bytes) = s3.get_object(RECENT_KEY).await {
        if let Ok(entries) = serde_json::from_slice::<Vec<RecentEntry>>(&bytes) {
            let filtered: Vec<_> = entries
                .into_iter()
                .filter(|e| is_supported_format(&e.filename))
                .collect();
            if !filtered.is_empty() {
                return filtered;
            }
        }
    }

    tracing::info!("recent.json not found, scanning storage for recent uploads");
    let entries = reconstruct_recent(s3).await;

    if !entries.is_empty() {
        if let Ok(json) = serde_json::to_vec(&entries) {
            let _ = s3.put_object(RECENT_KEY, json, "application/json").await;
        }
        tracing::info!(
            "Reconstructed {} recent entries from storage",
            entries.len()
        );
    }

    entries
}

async fn reconstruct_recent(s3: &crate::s3::S3Client) -> Vec<RecentEntry> {
    let objects = match s3.list_objects("boms/").await {
        Ok(objs) => objs,
        Err(e) => {
            tracing::warn!("Failed to list objects for recent reconstruction: {e}");
            return Vec::new();
        }
    };

    let mut meta_objects: Vec<_> = objects
        .into_iter()
        .filter(|o| o.key.ends_with(".meta.json"))
        .collect();
    meta_objects.sort_by_key(|o| std::cmp::Reverse(o.last_modified));
    meta_objects.truncate(MAX_RECENT);

    let mut entries = Vec::new();
    for obj in meta_objects {
        if let Ok(bytes) = s3.get_object(&obj.key).await {
            if let Ok(meta) = serde_json::from_slice::<BomMeta>(&bytes) {
                if is_supported_format(&meta.filename) {
                    entries.push(RecentEntry {
                        id: meta.id,
                        filename: meta.filename,
                        components: meta.components,
                        file_size: meta.file_size,
                        created: obj.last_modified.to_rfc3339(),
                    });
                }
            }
        }
    }

    entries
}

pub fn router(max_upload_bytes: usize) -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/terms", get(terms))
        .route("/upload", post(upload))
        .route("/api/recent", get(get_recent))
        .route("/b/{id}", get(get_bom))
        .route("/b/{id}/data", get(get_bom_data))
        .route("/b/{id}/meta", get(get_meta))
        .route("/b/{id}/thumb.svg", get(get_thumb_svg))
        .route("/b/{id}/schematic", get(crate::schematic::get_viewer))
        .route("/b/{id}/schematic.json", get(crate::schematic::get_scene))
        .route("/b/{id}/schematic.svg", get(crate::schematic::get_svg))
        .route("/sview/{*path}", get(crate::schematic::serve_assets))
        .route("/sview/", get(crate::schematic::serve_assets))
        .route("/gh-render", get(crate::github::gh_render))
        // GDSII tile viewer
        .route("/g/{id}", get(crate::gdsii_tiles::get_gds_viewer))
        .route(
            "/g/{id}/manifest.json",
            get(crate::gdsii_tiles::get_gds_manifest),
        )
        .route(
            "/g/{id}/tiles/{z}/{x}/{y}/{key}",
            get(crate::gdsii_tiles::get_gds_tile),
        )
        .route("/gview/{*path}", get(crate::gdsii_tiles::serve_gview))
        .route("/gview/", get(crate::gdsii_tiles::serve_gview))
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(max_upload_bytes))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn terms() -> Html<&'static str> {
    Html(include_str!("../static/terms.html"))
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct UploadResponse {
    url: String,
    id: String,
    filename: String,
    components: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize, Deserialize)]
struct BomMeta {
    id: String,
    filename: String,
    components: usize,
    file_size: usize,
    /// A board is stored at `boms/{id}.json`; false for schematic-only uploads.
    #[serde(default = "default_true")]
    has_pcb: bool,
    /// A schematic scene is stored at `boms/{id}.schematic.json`.
    #[serde(default)]
    has_schematic: bool,
}

fn default_true() -> bool {
    true
}

async fn upload(
    State(state): State<AppState>,
    multipart_result: Result<Multipart, MultipartRejection>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let mut multipart = multipart_result
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Upload error: {e}")))?;
    // Several `file` fields upload a KiCad project's files together; the
    // size limit applies to their total.
    let mut uploads: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0usize;
    let mut secret = false;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let limit = state.max_upload_bytes;
            let mut data = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, "Failed to read upload"))?
            {
                data.extend_from_slice(&chunk);
                total += chunk.len();
                if total > limit {
                    let limit_mb = limit / (1024 * 1024);
                    return Err(error_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        &format!("File too large ({limit_mb} MB limit)"),
                    ));
                }
            }
            uploads.push((filename, data));
        } else if name == "secret" {
            let val = field.text().await.unwrap_or_default();
            secret = val == "true";
        }
    }

    if uploads.is_empty() {
        return Err(error_response(StatusCode::BAD_REQUEST, "No file uploaded"));
    }
    if crate::schematic::is_project_upload(&uploads) {
        return upload_project(&state, uploads, secret).await.map(Json);
    }
    let (filename, data) = uploads.remove(0);

    // GDSII goes to the tile-viewer pipeline, not the PcbData/BOM path.
    let lower = filename.to_lowercase();
    if lower.ends_with(".gds") || lower.ends_with(".gds2") {
        let id = Uuid::new_v4().to_string();
        crate::gdsii_tiles::ingest(&state, &id, data)
            .await
            .map_err(|e| {
                tracing::error!("GDSII ingest error for {filename}: {e}");
                error_response(StatusCode::UNPROCESSABLE_ENTITY, &e)
            })?;
        let base_url =
            std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
        return Ok(Json(UploadResponse {
            url: format!("{base_url}/g/{id}"),
            id,
            filename,
            components: 0,
        }));
    }

    let path = std::path::Path::new(&filename);
    let format = pcb_extract::detect_format_with_content(path, &data)
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "Unsupported file format"))?;

    let file_size = data.len();
    let id = Uuid::new_v4().to_string();

    // Always store the original upload first (sanitize the client filename so
    // it can't escape the upload directory on the filesystem backend).
    let upload_key = format!("uploads/{id}/{}", safe_filename(&filename));
    let _ = state
        .s3
        .put_object(&upload_key, data.clone(), "application/octet-stream")
        .await;

    let pcb_data = parse_pcb_guarded(&state, data, format).await.map_err(|e| {
        tracing::error!("Parse error for {filename}: {e}");
        error_response(e.status(), &e.to_string())
    })?;

    let component_count = pcb_data.footprints.len();

    // Store pcbdata as JSON
    let pcbdata_json = serde_json::to_vec(&pcb_data).map_err(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "JSON serialization failed",
        )
    })?;
    let bom_key = format!("boms/{id}.json");
    state
        .s3
        .put_object(&bom_key, pcbdata_json, "application/json")
        .await
        .map_err(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to store BOM"))?;

    // Store metadata
    let meta = BomMeta {
        id: id.clone(),
        filename: filename.clone(),
        components: component_count,
        file_size,
        has_pcb: true,
        has_schematic: false,
    };
    let meta_key = format!("boms/{id}.meta.json");
    if let Ok(meta_json) = serde_json::to_vec(&meta) {
        let _ = state
            .s3
            .put_object(&meta_key, meta_json, "application/json")
            .await;
    }

    if !secret {
        add_recent(
            &state,
            RecentEntry {
                id: id.clone(),
                filename: filename.clone(),
                components: component_count,
                file_size,
                created: chrono::Utc::now().to_rfc3339(),
            },
        )
        .await;
    }

    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
    Ok(Json(UploadResponse {
        url: format!("{base_url}/b/{id}"),
        id,
        filename,
        components: component_count,
    }))
}

/// Ingest a KiCad project: a `.kicad_sch`, several KiCad files, or a project
/// zip. The board (if any) is parsed as usual; the schematic hierarchy is
/// stored as a scene. A schematic failure only fails the upload when there is
/// no board to show instead.
async fn upload_project(
    state: &AppState,
    uploads: Vec<(String, Vec<u8>)>,
    secret: bool,
) -> Result<UploadResponse, (StatusCode, Json<ErrorResponse>)> {
    let id = Uuid::new_v4().to_string();
    let file_size: usize = uploads.iter().map(|(_, d)| d.len()).sum();
    for (name, data) in &uploads {
        let key = format!("uploads/{id}/{}", safe_filename(name));
        let _ = state
            .s3
            .put_object(&key, data.clone(), "application/octet-stream")
            .await;
    }
    let upload_name = uploads[0].0.clone();
    let project = crate::schematic::Project::collect(uploads, MAX_PROJECT_BYTES)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &e))?;

    let mut components = 0;
    let has_pcb = match project.pcb() {
        Some((name, data)) => {
            let pcb = parse_pcb_guarded(state, data.to_vec(), pcb_extract::PcbFormat::KiCad)
                .await
                .map_err(|e| {
                    tracing::error!("Parse error for {name} in {upload_name}: {e}");
                    error_response(e.status(), &e.to_string())
                })?;
            components = pcb.footprints.len();
            let json = serde_json::to_vec(&pcb).map_err(|_| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "JSON serialization failed",
                )
            })?;
            state
                .s3
                .put_object(&format!("boms/{id}.json"), json, "application/json")
                .await
                .map_err(|_| {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to store BOM")
                })?;
            true
        }
        None => false,
    };

    let has_schematic = if project.has_schematic() {
        match crate::schematic::parse_guarded(state, project.files).await {
            Ok(scene) => {
                if !has_pcb {
                    components = crate::schematic::symbol_count(&scene);
                }
                let json = serde_json::to_vec(&scene).map_err(|_| {
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "JSON serialization failed",
                    )
                })?;
                state
                    .s3
                    .put_object(
                        &crate::schematic::scene_key(&id),
                        json,
                        "application/json",
                    )
                    .await
                    .map_err(|_| {
                        error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to store schematic",
                        )
                    })?;
                true
            }
            Err(e) if has_pcb => {
                tracing::warn!("Schematic skipped for {upload_name}: {e}");
                false
            }
            Err(e) => {
                tracing::error!("Schematic parse error for {upload_name}: {e}");
                return Err(error_response(e.status(), &e.to_string()));
            }
        }
    } else {
        false
    };

    if !has_pcb && !has_schematic {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "No .kicad_pcb or .kicad_sch found in upload",
        ));
    }

    let meta = BomMeta {
        id: id.clone(),
        filename: upload_name.clone(),
        components,
        file_size,
        has_pcb,
        has_schematic,
    };
    if let Ok(meta_json) = serde_json::to_vec(&meta) {
        let _ = state
            .s3
            .put_object(&format!("boms/{id}.meta.json"), meta_json, "application/json")
            .await;
    }

    if !secret {
        add_recent(
            state,
            RecentEntry {
                id: id.clone(),
                filename: upload_name.clone(),
                components,
                file_size,
                created: chrono::Utc::now().to_rfc3339(),
            },
        )
        .await;
    }

    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
    let path = if has_pcb {
        format!("/b/{id}")
    } else {
        format!("/b/{id}/schematic")
    };
    Ok(UploadResponse {
        url: format!("{base_url}{path}"),
        id,
        filename: upload_name,
        components,
    })
}

async fn get_recent(State(state): State<AppState>) -> Json<Vec<RecentEntry>> {
    let recent = state.recent.read().await;
    Json(recent.clone())
}

/// Serve the Yew viewer shell HTML at /b/{id}
async fn get_bom(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    validate_id(&id)?;
    // Verify the BOM exists; schematic-only uploads open the schematic viewer.
    let key = format!("boms/{id}.json");
    if state.s3.get_object(&key).await.is_err() {
        if crate::schematic::exists(&state, &id).await {
            return Ok(Redirect::temporary(&format!("/b/{id}/schematic")).into_response());
        }
        return Err(error_response(StatusCode::NOT_FOUND, "BOM not found"));
    }

    // Serve the viewer index.html
    let index_path = state.viewer_dir.join("index.html");
    let html = tokio::fs::read_to_string(&index_path)
        .await
        .map_err(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "Viewer not available"))?;
    Ok(Html(html).into_response())
}

/// Serve pcbdata JSON at /b/{id}/data
async fn get_bom_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    validate_id(&id)?;
    let key = format!("boms/{id}.json");
    let json_bytes = state
        .s3
        .get_object(&key)
        .await
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "BOM not found"))?;
    Ok((
        StatusCode::OK,
        [("content-type", "application/json")],
        json_bytes,
    ))
}

async fn get_meta(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    validate_id(&id)?;
    let key = format!("boms/{id}.meta.json");
    let meta_bytes = state
        .s3
        .get_object(&key)
        .await
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "BOM not found"))?;
    let meta: BomMeta = serde_json::from_slice(&meta_bytes)
        .map_err(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "Invalid metadata"))?;
    Ok(Json(meta))
}

/// Serve SVG thumbnail at /b/{id}/thumb.svg with S3 pull-through cache.
async fn get_thumb_svg(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    validate_id(&id)?;
    let svg_bytes = load_thumbnail(&state, &id)
        .await
        .map_err(|(status, msg)| error_response(status, msg))?;
    Ok((
        StatusCode::OK,
        [
            ("content-type", "image/svg+xml"),
            ("cache-control", "public, max-age=86400"),
        ],
        svg_bytes,
    ))
}

/// Load a board's SVG thumbnail, rendering and caching it from the stored
/// PcbData if it is not in storage yet.
pub async fn load_thumbnail(
    state: &AppState,
    id: &str,
) -> Result<Vec<u8>, (StatusCode, &'static str)> {
    let thumb_key = format!("thumbnails/{id}.svg");
    if let Ok(cached) = state.s3.get_object(&thumb_key).await {
        return Ok(cached);
    }

    let data_key = format!("boms/{id}.json");
    let Ok(json_bytes) = state.s3.get_object(&data_key).await else {
        // Schematic-only uploads use their root sheet.
        return crate::schematic::load_svg(state, id)
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, "BOM not found"));
    };

    let pcb_data: pcb_extract::types::PcbData =
        serde_json::from_slice(&json_bytes).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to parse BOM data",
            )
        })?;

    let svg = tokio::task::spawn_blocking(move || pcb_extract::thumbnail::render_svg(&pcb_data))
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Render failed"))?;
    let svg_bytes = svg.into_bytes();

    // Store in cache (fire and forget)
    let s3 = state.s3.clone();
    let cache_bytes = svg_bytes.clone();
    tokio::spawn(async move {
        let _ = s3
            .put_object(&thumb_key, cache_bytes, "image/svg+xml")
            .await;
    });

    Ok(svg_bytes)
}

fn error_response(status: StatusCode, msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_strips_traversal() {
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("/abs/board.kicad_pcb"), "board.kicad_pcb");
        assert_eq!(safe_filename("board.kicad_pcb"), "board.kicad_pcb");
        assert_eq!(safe_filename(".."), "upload.bin");
        assert_eq!(safe_filename(""), "upload.bin");
    }

    #[test]
    fn safe_filename_has_no_separators() {
        for name in ["../../x", "a/b/c", "x\\y", "..\\..\\y"] {
            let s = safe_filename(name);
            assert!(!s.contains('/') && !s.contains('\\'), "{name} -> {s}");
        }
    }

    #[test]
    fn validate_id_accepts_uuid_rejects_traversal() {
        let id = Uuid::new_v4().to_string();
        assert!(validate_id(&id).is_ok());
        assert!(validate_id("../../etc/passwd").is_err());
        assert!(validate_id("not-a-uuid").is_err());
        assert!(validate_id("../boms/secret").is_err());
    }
}
