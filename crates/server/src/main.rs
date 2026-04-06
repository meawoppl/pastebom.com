mod compressed_assets;
mod github;
mod reparse;
mod routes;
mod s3;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let s3_client = s3::S3Client::from_env().await;
    let viewer_dir = PathBuf::from(
        std::env::var("VIEWER_DIR").unwrap_or_else(|_| "crates/viewer/dist".to_string()),
    );
    tracing::info!("Serving viewer assets from {}", viewer_dir.display());

    let recent = routes::load_recent(&s3_client).await;
    tracing::info!("Loaded {} recent public uploads", recent.len());

    let mut default_headers = reqwest::header::HeaderMap::new();
    default_headers.insert(
        "user-agent",
        reqwest::header::HeaderValue::from_static("pastebom.com"),
    );
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if let Ok(val) = format!("Bearer {token}").parse() {
            default_headers.insert("authorization", val);
        }
    }
    let http_client = reqwest::ClientBuilder::new()
        .default_headers(default_headers)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let max_upload_bytes: usize = std::env::var("MAX_UPLOAD_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50 * 1024 * 1024);
    tracing::info!("Max upload size: {} MB", max_upload_bytes / (1024 * 1024));

    let max_concurrent_parses: usize = std::env::var("MAX_CONCURRENT_PARSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    tracing::info!("Max concurrent parses: {max_concurrent_parses}");

    let state = AppState {
        s3: s3_client,
        viewer_dir: viewer_dir.clone(),
        recent: Arc::new(RwLock::new(recent)),
        http_client,
        max_upload_bytes,
        parse_semaphore: Arc::new(Semaphore::new(max_concurrent_parses)),
    };

    // Pre-compress viewer assets at startup
    compressed_assets::init_cache(&viewer_dir);

    // Spawn background re-parse of stale boards
    let reparse_s3 = state.s3.clone();
    tokio::spawn(async move {
        reparse::reparse_stale_boards(reparse_s3).await;
    });

    let app = build_app(state);

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8000".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    tracing::info!("Listening on {bind_addr}");
    axum::serve(listener, app).await.unwrap();
}

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .merge(routes::router(state.max_upload_bytes))
        .route("/viewer/{*path}", get(compressed_assets::serve_viewer))
        .route("/viewer/", get(compressed_assets::serve_viewer))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(120),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Clone)]
pub struct AppState {
    pub s3: s3::S3Client,
    pub viewer_dir: PathBuf,
    pub recent: Arc<RwLock<Vec<routes::RecentEntry>>>,
    pub http_client: reqwest::Client,
    pub max_upload_bytes: usize,
    pub parse_semaphore: Arc<Semaphore>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_state() -> AppState {
        let s3 = s3::S3Client::from_env().await;
        let viewer_dir = PathBuf::from("crates/viewer/dist");
        compressed_assets::init_cache(&viewer_dir);
        AppState {
            s3,
            viewer_dir,
            recent: Arc::new(RwLock::new(Vec::new())),
            http_client: reqwest::Client::new(),
            max_upload_bytes: 50 * 1024 * 1024,
            parse_semaphore: Arc::new(Semaphore::new(4)),
        }
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = build_app(test_state().await);
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_index_returns_html() {
        let app = build_app(test_state().await);
        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("html"), "Expected HTML content-type, got {ct}");
    }

    #[tokio::test]
    async fn test_recent_api() {
        let app = build_app(test_state().await);
        let resp = app
            .oneshot(Request::get("/api/recent").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn test_missing_bom_returns_404() {
        let app = build_app(test_state().await);
        let resp = app
            .oneshot(
                Request::get("/b/00000000-0000-0000-0000-000000000000/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }
}
