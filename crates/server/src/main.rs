mod github;
mod routes;
mod s3;

use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
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

    let state = AppState {
        s3: s3_client,
        viewer_dir: viewer_dir.clone(),
        recent: Arc::new(RwLock::new(recent)),
        http_client,
        max_upload_bytes,
    };

    let app = Router::new()
        .merge(routes::router(max_upload_bytes))
        .nest_service("/viewer", ServeDir::new(&viewer_dir))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8000".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    tracing::info!("Listening on {bind_addr}");
    axum::serve(listener, app).await.unwrap();
}

#[derive(Clone)]
pub struct AppState {
    pub s3: s3::S3Client,
    pub viewer_dir: PathBuf,
    pub recent: Arc<RwLock<Vec<routes::RecentEntry>>>,
    pub http_client: reqwest::Client,
    pub max_upload_bytes: usize,
}
