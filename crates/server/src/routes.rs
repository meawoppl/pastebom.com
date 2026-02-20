use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use pcb_extract::ExtractOptions;
use serde::Serialize;
use uuid::Uuid;

use crate::{html, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/upload", post(upload))
        .route("/b/{id}", get(get_bom))
        .route("/health", get(health))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct UploadResponse {
    url: String,
    id: String,
    components: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let mut file_data: Option<(String, Vec<u8>)> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|_| error_response("Failed to read upload"))?;
            file_data = Some((filename, data.to_vec()));
        }
    }

    let (filename, data) = file_data.ok_or_else(|| error_response("No file uploaded"))?;

    // Validate file size (50 MB limit)
    const MAX_SIZE: usize = 50 * 1024 * 1024;
    if data.len() > MAX_SIZE {
        return Err(error_response("File too large (50 MB limit)"));
    }

    // Detect format from filename
    let path = std::path::Path::new(&filename);
    let format = pcb_extract::detect_format(path)
        .ok_or_else(|| error_response("Unsupported file format"))?;

    // Extract PCB data
    let opts = ExtractOptions {
        include_tracks: true,
        include_nets: true,
    };
    let pcb_data = match pcb_extract::extract_bytes(&data, format, &opts) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Parse error for {filename}: {e}");
            // Store failed upload for regression testing
            let fail_name = format!("{}_{}", Uuid::new_v4(), filename);
            let _ = state.s3.put_failed(&fail_name, data.clone()).await;
            return Err(error_response("Failed to parse PCB file"));
        }
    };

    // Generate HTML
    let title = if pcb_data.metadata.title.is_empty() {
        filename.clone()
    } else {
        pcb_data.metadata.title.clone()
    };
    let html_content = html::generate_html(&pcb_data, &title)
        .map_err(|_| error_response("HTML generation failed"))?;

    // Upload to S3
    let id = Uuid::new_v4().to_string();
    let bom_key = format!("boms/{id}.html");

    state
        .s3
        .put_object(&bom_key, html_content.into_bytes(), "text/html")
        .await
        .map_err(|_| error_response("Failed to store BOM"))?;

    // Also store the original upload
    let upload_key = format!("uploads/{id}/{filename}");
    let _ = state
        .s3
        .put_object(&upload_key, data, "application/octet-stream")
        .await;

    let component_count = pcb_data.footprints.len();
    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
    Ok(Json(UploadResponse {
        url: format!("{base_url}/b/{id}"),
        id: id.clone(),
        components: component_count,
    }))
}

async fn get_bom(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let key = format!("boms/{id}.html");
    let html_bytes = state.s3.get_object(&key).await.map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "BOM not found".to_string(),
            }),
        )
    })?;
    let html = String::from_utf8_lossy(&html_bytes).into_owned();
    Ok(Html(html))
}

fn error_response(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}
