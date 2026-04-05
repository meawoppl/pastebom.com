use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

struct CompressedAsset {
    raw: Vec<u8>,
    brotli: Vec<u8>,
    gzip: Vec<u8>,
    mime: String,
}

static CACHE: OnceLock<HashMap<String, CompressedAsset>> = OnceLock::new();

/// Pre-compress all viewer assets from disk at startup.
pub fn init_cache(viewer_dir: &Path) {
    CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        let mut total_raw = 0u64;
        let mut total_br = 0u64;

        let walker = match std::fs::read_dir(viewer_dir) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("Could not read viewer dir {}: {e}", viewer_dir.display());
                return map;
            }
        };

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = match path.file_name().and_then(|f| f.to_str()) {
                Some(f) => f.to_string(),
                None => continue,
            };
            let raw = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .to_string();

            let brotli_buf = {
                let mut buf = Vec::new();
                {
                    let mut writer = brotli::CompressorWriter::new(&mut buf, 4096, 11, 22);
                    writer.write_all(&raw).unwrap();
                }
                buf
            };

            let gzip_buf = {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                encoder.write_all(&raw).unwrap();
                encoder.finish().unwrap()
            };

            total_raw += raw.len() as u64;
            total_br += brotli_buf.len() as u64;

            map.insert(
                filename,
                CompressedAsset {
                    raw,
                    brotli: brotli_buf,
                    gzip: gzip_buf,
                    mime,
                },
            );
        }

        if total_raw > 0 {
            tracing::info!(
                "Pre-compressed {} viewer assets: {:.1} MB raw -> {:.1} MB brotli ({:.0}% reduction)",
                map.len(),
                total_raw as f64 / 1_048_576.0,
                total_br as f64 / 1_048_576.0,
                (1.0 - total_br as f64 / total_raw as f64) * 100.0
            );
        }

        map
    });
}

/// Serve pre-compressed viewer assets. Falls back to index.html for SPA routing.
pub async fn serve_viewer(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches("/viewer/");
    let path = if path.is_empty() { "index.html" } else { path };

    let cache = match CACHE.get() {
        Some(c) => c,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Asset cache not initialized",
            )
                .into_response()
        }
    };

    let accept = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let asset = cache.get(path).or_else(|| cache.get("index.html"));

    match asset {
        Some(asset) => {
            let (body, encoding) = if accept.contains("br") {
                (asset.brotli.as_slice(), Some("br"))
            } else if accept.contains("gzip") {
                (asset.gzip.as_slice(), Some("gzip"))
            } else {
                (asset.raw.as_slice(), None)
            };

            let cache_control = if path == "index.html" {
                "no-cache"
            } else {
                "public, max-age=31536000, immutable"
            };

            let mut resp = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, &asset.mime)
                .header(header::CACHE_CONTROL, cache_control)
                .body(Body::from(body.to_vec()))
                .unwrap();

            if let Some(enc) = encoding {
                resp.headers_mut()
                    .insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
            }

            resp
        }
        None => (StatusCode::NOT_FOUND, "Asset not found").into_response(),
    }
}
