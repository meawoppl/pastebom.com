mod routes;
mod s3;

use axum::Router;
use std::path::PathBuf;
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

    let state = AppState {
        s3: s3_client,
        viewer_dir: viewer_dir.clone(),
    };

    let app = Router::new()
        .merge(routes::router())
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
}
