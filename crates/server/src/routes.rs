use axum::{
    extract::{multipart::MultipartRejection, DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use pcb_extract::ExtractOptions;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

const MAX_RECENT: usize = 50;
const RECENT_KEY: &str = "recent.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct RecentEntry {
    pub id: String,
    pub filename: String,
    pub components: usize,
    #[serde(default)]
    pub file_size: usize,
    pub created: String,
}

pub async fn load_recent(s3: &crate::s3::S3Client) -> Vec<RecentEntry> {
    match s3.get_object(RECENT_KEY).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn router() -> Router<AppState> {
    const MAX_UPLOAD: usize = 50 * 1024 * 1024;
    Router::new()
        .route("/", get(index))
        .route("/upload", post(upload))
        .route("/api/recent", get(get_recent))
        .route("/b/{id}", get(get_bom))
        .route("/b/{id}/data", get(get_bom_data))
        .route("/b/{id}/meta", get(get_meta))
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
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
}

async fn upload(
    State(state): State<AppState>,
    multipart_result: Result<Multipart, MultipartRejection>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let mut multipart = multipart_result
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Upload error: {e}")))?;
    let mut file_data: Option<(String, Vec<u8>)> = None;
    let mut secret = false;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, "Failed to read upload"))?;
            file_data = Some((filename, data.to_vec()));
        } else if name == "secret" {
            let val = field.text().await.unwrap_or_default();
            secret = val == "true";
        }
    }

    let (filename, data) =
        file_data.ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "No file uploaded"))?;

    const MAX_SIZE: usize = 50 * 1024 * 1024;
    if data.len() > MAX_SIZE {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "File too large (50 MB limit)",
        ));
    }

    let path = std::path::Path::new(&filename);
    let format = pcb_extract::detect_format(path)
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "Unsupported file format"))?;

    let file_size = data.len();
    let id = Uuid::new_v4().to_string();

    // Always store the original upload first
    let upload_key = format!("uploads/{id}/{filename}");
    let _ = state
        .s3
        .put_object(&upload_key, data.clone(), "application/octet-stream")
        .await;

    let opts = ExtractOptions {
        include_tracks: true,
        include_nets: true,
    };
    let pcb_data = match pcb_extract::extract_bytes(&data, format, &opts) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Parse error for {filename}: {e}");
            return Err(error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Failed to parse PCB file",
            ));
        }
    };

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
    };
    let meta_key = format!("boms/{id}.meta.json");
    if let Ok(meta_json) = serde_json::to_vec(&meta) {
        let _ = state
            .s3
            .put_object(&meta_key, meta_json, "application/json")
            .await;
    }

    if !secret {
        let entry = RecentEntry {
            id: id.clone(),
            filename: filename.clone(),
            components: component_count,
            file_size,
            created: chrono::Utc::now().to_rfc3339(),
        };
        let mut recent = state.recent.write().await;
        recent.insert(0, entry);
        recent.truncate(MAX_RECENT);
        if let Ok(json) = serde_json::to_vec(&*recent) {
            let _ = state
                .s3
                .put_object(RECENT_KEY, json, "application/json")
                .await;
        }
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

async fn get_recent(State(state): State<AppState>) -> Json<Vec<RecentEntry>> {
    let recent = state.recent.read().await;
    Json(recent.clone())
}

/// Serve the Yew viewer shell HTML at /b/{id}
async fn get_bom(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Verify the BOM exists
    let key = format!("boms/{id}.json");
    let _ = state
        .s3
        .get_object(&key)
        .await
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "BOM not found"))?;

    // Serve the viewer index.html
    let index_path = state.viewer_dir.join("index.html");
    let html = tokio::fs::read_to_string(&index_path)
        .await
        .map_err(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "Viewer not available"))?;
    Ok(Html(html))
}

/// Serve pcbdata JSON at /b/{id}/data
async fn get_bom_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
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

fn error_response(status: StatusCode, msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}
