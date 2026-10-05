//! Hosts the embeddable Gerber viewer (`crates/gerber-view`) so other sites can
//! vendor or load it: the wasm-pack bundle under `/gerber-view/pkg/` and the
//! example page under `/gerber-view/examples/`. Assets come from `GERBER_VIEW_DIR`.

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
};

use crate::AppState;

const EXAMPLE_PAGE: &str = "/gerber-view/examples/index.html";

/// Only these subdirectories of `GERBER_VIEW_DIR` are public.
const SERVED_DIRS: [&str; 2] = ["pkg/", "examples/"];

pub async fn serve(State(state): State<AppState>, uri: Uri) -> Response {
    let rel = uri
        .path()
        .trim_start_matches("/gerber-view")
        .trim_start_matches('/');
    if rel.is_empty() || rel == "examples/" {
        return Redirect::temporary(EXAMPLE_PAGE).into_response();
    }
    if rel.contains("..") || rel.contains('\\') || !SERVED_DIRS.iter().any(|d| rel.starts_with(d)) {
        return not_found();
    }

    let path = state.gerber_view_dir.join(rel);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return not_found();
    };
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            // Bundle filenames are not content-hashed, so keep caching short.
            (header::CACHE_CONTROL, "public, max-age=300".to_string()),
        ],
        Body::from(bytes),
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}
