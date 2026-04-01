use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct GhRenderParams {
    /// Single path: owner/repo/path/to/file.kicad_pcb
    file: String,
    /// Optional branch/tag override
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct GhCacheEntry {
    sha: String,
    bom_id: String,
}

/// GET /gh-render?file=owner/repo/path/to/file.kicad_pcb&ref=main
pub async fn gh_render(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<GhRenderParams>,
) -> impl IntoResponse {
    match gh_render_inner(state, headers, params).await {
        Ok(response) => response,
        Err(msg) => (StatusCode::OK, svg_headers("error"), error_svg(&msg)),
    }
}

async fn gh_render_inner(
    state: AppState,
    headers: HeaderMap,
    params: GhRenderParams,
) -> Result<(StatusCode, HeaderMap, Vec<u8>), String> {
    // Parse file param: owner/repo/path/to/file.ext
    let file = params.file.trim_matches('/');
    if file.contains("..") {
        return Err("Invalid path".to_string());
    }

    let parts: Vec<&str> = file.splitn(3, '/').collect();
    if parts.len() < 3 || parts.iter().any(|p| p.is_empty()) {
        return Err("Use: ?file=owner/repo/path/to/file".to_string());
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let path = parts[2].to_string();

    // Validate the file has a recognized PCB extension
    let file_path = std::path::Path::new(&path);
    if pcb_extract::detect_format(file_path).is_none() {
        return Err("Unsupported file format".to_string());
    }

    // Resolve git ref: explicit, or try main then master
    let git_ref = if let Some(r) = params.git_ref {
        r
    } else {
        resolve_default_ref(&state.http_client, &repo, &path).await?
    };

    let cache_key = build_cache_key(&repo, &git_ref, &path);

    // Check GitHub for current file SHA
    let gh_info = fetch_file_info(&state.http_client, &repo, &path, &git_ref)
        .await
        .map_err(|e| match e {
            GhError::NotFound => "File not found on GitHub".to_string(),
            GhError::RateLimited => "GitHub API rate limit exceeded — try again later".to_string(),
            GhError::Other(msg) => format!("GitHub error: {msg}"),
        })?;

    // Check if we have a cached entry with matching SHA
    if let Ok(cached_bytes) = state.s3.get_object(&cache_key).await {
        if let Ok(entry) = serde_json::from_slice::<GhCacheEntry>(&cached_bytes) {
            if entry.sha == gh_info.sha {
                // Check If-None-Match for 304
                if let Some(etag) = headers.get("if-none-match") {
                    if etag.as_bytes() == format!("\"{}\"", entry.sha).as_bytes() {
                        return Ok((StatusCode::NOT_MODIFIED, HeaderMap::new(), Vec::new()));
                    }
                }

                // Serve cached thumbnail
                let thumb_key = format!("thumbnails/{}.svg", entry.bom_id);
                if let Ok(svg) = state.s3.get_object(&thumb_key).await {
                    return Ok((StatusCode::OK, svg_headers(&entry.sha), svg));
                }
            }
        }
    }

    // Cache miss or stale — download from GitHub
    let file_bytes = download_raw(&state.http_client, &repo, &git_ref, &path)
        .await
        .map_err(|e| match e {
            GhError::NotFound => "File not found on GitHub".to_string(),
            GhError::RateLimited => "GitHub API rate limit exceeded — try again later".to_string(),
            GhError::Other(msg) => format!("Download failed: {msg}"),
        })?;

    if file_bytes.len() > state.max_upload_bytes {
        let limit_mb = state.max_upload_bytes / (1024 * 1024);
        return Err(format!("File too large ({limit_mb} MB limit)"));
    }

    // Detect format with content
    let format = pcb_extract::detect_format_with_content(file_path, &file_bytes)
        .ok_or_else(|| "Unsupported file format".to_string())?;

    // Parse with concurrency limit
    let pcb_data = crate::routes::parse_pcb_guarded(&state, file_bytes.clone(), format).await?;

    let filename = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let bom_id = Uuid::new_v4().to_string();
    let component_count = pcb_data.footprints.len();
    let file_size = file_bytes.len();

    // Store through normal upload path
    let upload_key = format!("uploads/{bom_id}/{filename}");
    let _ = state
        .s3
        .put_object(&upload_key, file_bytes, "application/octet-stream")
        .await;

    let pcbdata_json =
        serde_json::to_vec(&pcb_data).map_err(|_| "Serialization failed".to_string())?;
    let bom_key = format!("boms/{bom_id}.json");
    state
        .s3
        .put_object(&bom_key, pcbdata_json, "application/json")
        .await
        .map_err(|_| "Storage failed".to_string())?;

    // Store metadata
    let meta = serde_json::json!({
        "id": bom_id,
        "filename": filename,
        "components": component_count,
        "file_size": file_size,
        "github_repo": repo,
        "github_path": path,
        "github_ref": git_ref,
    });
    let meta_key = format!("boms/{bom_id}.meta.json");
    if let Ok(meta_json) = serde_json::to_vec(&meta) {
        let _ = state
            .s3
            .put_object(&meta_key, meta_json, "application/json")
            .await;
    }

    // Add to recent list
    crate::routes::add_recent(
        &state,
        crate::routes::RecentEntry {
            id: bom_id.clone(),
            filename: filename.clone(),
            components: component_count,
            file_size,
            created: chrono::Utc::now().to_rfc3339(),
        },
    )
    .await;

    // Render thumbnail
    let svg = tokio::task::spawn_blocking(move || pcb_extract::thumbnail::render_svg(&pcb_data))
        .await
        .map_err(|_| "Thumbnail render failed".to_string())?;
    let svg_bytes = svg.into_bytes();

    // Store thumbnail
    let thumb_key = format!("thumbnails/{bom_id}.svg");
    let _ = state
        .s3
        .put_object(&thumb_key, svg_bytes.clone(), "image/svg+xml")
        .await;

    // Store cache entry
    let cache_entry = GhCacheEntry {
        sha: gh_info.sha.clone(),
        bom_id,
    };
    if let Ok(cache_json) = serde_json::to_vec(&cache_entry) {
        let _ = state
            .s3
            .put_object(&cache_key, cache_json, "application/json")
            .await;
    }

    Ok((StatusCode::OK, svg_headers(&gh_info.sha), svg_bytes))
}

fn svg_headers(sha: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "image/svg+xml".parse().unwrap());
    headers.insert("cache-control", "public, max-age=300".parse().unwrap());
    headers.insert("etag", format!("\"{sha}\"").parse().unwrap());
    headers
}

fn error_svg(message: &str) -> Vec<u8> {
    // Escape XML special characters
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");

    // Word-wrap long messages into lines of ~40 chars
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    for word in escaped.split_whitespace() {
        if !current_line.is_empty() && current_line.len() + word.len() + 1 > 40 {
            lines.push(current_line);
            current_line = word.to_string();
        } else {
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(word);
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    let line_height = 18;
    let text_block_height = lines.len() as u32 * line_height;
    let height = 120.max(60 + text_block_height);

    let err_color = "#ff6b6b";
    let mut text_elements = String::new();
    let start_y = (height - text_block_height) / 2 + 14;
    for (i, line) in lines.iter().enumerate() {
        let y = start_y + i as u32 * line_height;
        text_elements.push_str(&format!(
            r#"<text x="200" y="{y}" text-anchor="middle" fill="{err_color}" font-family="monospace" font-size="14">{line}</text>"#
        ));
    }

    let bg = "#1a1a2e";
    let accent = "#4ecca3";
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 {height}" width="400" height="{height}"><rect width="400" height="{height}" fill="{bg}" rx="8"/><text x="200" y="30" text-anchor="middle" fill="{accent}" font-family="monospace" font-size="16" font-weight="bold">pastebom.com</text>{text_elements}</svg>"#
    ).into_bytes()
}

async fn resolve_default_ref(
    client: &reqwest::Client,
    repo: &str,
    path: &str,
) -> Result<String, String> {
    // Try "main" first, then "master"
    for branch in &["main", "master"] {
        if fetch_file_info(client, repo, path, branch).await.is_ok() {
            return Ok(branch.to_string());
        }
    }
    Err("File not found on main or master branch".to_string())
}

fn build_cache_key(repo: &str, git_ref: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let path_hash = hex::encode(hasher.finalize());
    format!("gh/{repo}/{git_ref}/{path_hash}.json")
}

enum GhError {
    NotFound,
    RateLimited,
    Other(String),
}

#[derive(Deserialize)]
struct GitHubContentsResponse {
    sha: String,
}

async fn fetch_file_info(
    client: &reqwest::Client,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> Result<GitHubContentsResponse, GhError> {
    let url = format!("https://api.github.com/repos/{repo}/contents/{path}?ref={git_ref}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| GhError::Other(format!("GitHub API request failed: {e}")))?;

    match resp.status().as_u16() {
        200 => resp
            .json::<GitHubContentsResponse>()
            .await
            .map_err(|e| GhError::Other(format!("Failed to parse GitHub response: {e}"))),
        404 => Err(GhError::NotFound),
        403 | 429 => Err(GhError::RateLimited),
        status => Err(GhError::Other(format!(
            "GitHub API returned status {status}"
        ))),
    }
}

async fn download_raw(
    client: &reqwest::Client,
    repo: &str,
    git_ref: &str,
    path: &str,
) -> Result<Vec<u8>, GhError> {
    let url = format!("https://raw.githubusercontent.com/{repo}/{git_ref}/{path}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| GhError::Other(format!("Download failed: {e}")))?;

    match resp.status().as_u16() {
        200 => resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| GhError::Other(format!("Failed to read response body: {e}"))),
        404 => Err(GhError::NotFound),
        403 | 429 => Err(GhError::RateLimited),
        status => Err(GhError::Other(format!("GitHub returned status {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_svg_is_valid_svg() {
        let svg = error_svg("File not found on GitHub");
        let text = String::from_utf8(svg).unwrap();
        assert!(text.starts_with("<svg"));
        assert!(text.ends_with("</svg>"));
        assert!(text.contains("pastebom.com"));
    }

    #[test]
    fn test_error_svg_contains_message() {
        let svg = error_svg("File not found on GitHub");
        let text = String::from_utf8(svg).unwrap();
        assert!(text.contains("File not found on GitHub"));
    }

    #[test]
    fn test_error_svg_escapes_xml() {
        let svg = error_svg("Error: <script>alert(1)</script> & stuff");
        let text = String::from_utf8(svg).unwrap();
        assert!(!text.contains("<script>"));
        assert!(text.contains("&lt;script&gt;"));
        assert!(text.contains("&amp;"));
    }

    #[test]
    fn test_error_svg_wraps_long_messages() {
        let svg = error_svg(
            "This is a very long error message that should be wrapped across multiple lines",
        );
        let text = String::from_utf8(svg).unwrap();
        // Should have multiple <text> elements for the message (plus the header)
        let text_count = text.matches("<text").count();
        assert!(text_count >= 3); // header + at least 2 wrapped lines
    }

    #[test]
    fn test_error_svg_minimum_height() {
        let svg = error_svg("Short");
        let text = String::from_utf8(svg).unwrap();
        assert!(text.contains(r#"height="120""#));
    }

    #[test]
    fn test_cache_key_deterministic() {
        let a = build_cache_key("owner/repo", "main", "path/to/file.kicad_pcb");
        let b = build_cache_key("owner/repo", "main", "path/to/file.kicad_pcb");
        assert_eq!(a, b);
    }

    #[test]
    fn test_cache_key_varies_by_ref() {
        let a = build_cache_key("owner/repo", "main", "file.kicad_pcb");
        let b = build_cache_key("owner/repo", "dev", "file.kicad_pcb");
        assert_ne!(a, b);
    }

    #[test]
    fn test_cache_key_varies_by_path() {
        let a = build_cache_key("owner/repo", "main", "a.kicad_pcb");
        let b = build_cache_key("owner/repo", "main", "b.kicad_pcb");
        assert_ne!(a, b);
    }

    #[test]
    fn test_cache_key_format() {
        let key = build_cache_key("owner/repo", "main", "board.kicad_pcb");
        assert!(key.starts_with("gh/owner/repo/main/"));
        assert!(key.ends_with(".json"));
    }
}
