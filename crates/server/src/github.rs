//! `/gh-render`: render a PCB file straight from a GitHub repository.
//!
//! Two storage indexes make repeat views cheap:
//! - `gh/{repo}/{ref}/{sha256(path)}.json` ([`RefEntry`]) records which file
//!   content (git blob SHA) a ref last resolved to.
//! - `gh-blob/{blob_sha}.json` ([`BlobEntry`]) records the render outcome for
//!   that content, so identical bytes on any ref or path are parsed once, and
//!   files that fail to parse are not re-parsed until the parser changes.
//!
//! A full 40-hex commit SHA as `ref` is immutable: those requests skip the
//! GitHub API entirely and are served with long-lived cache headers. Branch
//! and tag refs check the current blob SHA through the API on each request and
//! fall back to the last known render when the API is rate limited.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::routes::ParseError;
use crate::AppState;

const PARSER_VERSION: &str = env!("CARGO_PKG_VERSION");
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const MUTABLE_CACHE: &str = "public, max-age=300";
const NO_STORE: &str = "no-store";
const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// GitHub endpoints plus per-key locks that collapse concurrent renders of
/// the same file into a single download and parse.
#[derive(Clone)]
pub struct GhRender {
    api_base: String,
    raw_base: String,
    inflight: Arc<std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl Default for GhRender {
    fn default() -> Self {
        Self::new(
            "https://api.github.com",
            "https://raw.githubusercontent.com",
        )
    }
}

impl GhRender {
    pub fn new(api_base: &str, raw_base: &str) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            raw_base: raw_base.trim_end_matches('/').to_string(),
            inflight: Arc::default(),
        }
    }

    /// Serialize work on `key` across concurrent requests in this process.
    async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.inflight.lock().unwrap();
            map.retain(|_, weak| weak.strong_count() > 0);
            match map.get(key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    map.insert(key.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum OutputFormat {
    #[default]
    Svg,
    Json,
}

#[derive(Deserialize)]
pub struct GhRenderParams {
    /// Single path: owner/repo/path/to/file.kicad_pcb
    file: String,
    /// Optional branch/tag/commit override
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    /// When true, don't add the render to the public recent list.
    #[serde(default)]
    secret: bool,
    /// `svg` (default) returns the thumbnail; `json` returns board metadata.
    #[serde(default)]
    format: OutputFormat,
}

/// Validate a git ref (branch/tag/sha) before it is interpolated into storage
/// keys and GitHub URLs. Git refs may contain `/` but not `..`.
fn valid_git_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 256
        && !r.contains("..")
        && !r.starts_with('/')
        && !r.ends_with('/')
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// A full commit SHA names immutable content.
fn is_commit_sha(r: &str) -> bool {
    r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Git's blob object ID for `bytes`, identical to GitHub's contents API `sha`.
fn git_blob_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Which file content a (repo, ref, path) last resolved to.
#[derive(Serialize, Deserialize)]
struct RefEntry {
    blob_sha: String,
}

/// Render outcome for one file content, shared by every ref and path that
/// holds identical bytes.
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum BlobEntry {
    Rendered {
        bom_id: String,
        components: usize,
    },
    Failed {
        error: String,
        parser_version: String,
    },
}

struct Rendered {
    bom_id: String,
    components: usize,
    blob_sha: String,
}

#[derive(Serialize)]
struct RenderJson {
    id: String,
    viewer_url: String,
    thumb_url: String,
    components: usize,
    blob_sha: String,
    #[serde(rename = "ref")]
    git_ref: String,
}

#[derive(Debug)]
enum RenderError {
    BadRequest(String),
    NotFound(String),
    TooLarge(String),
    Unsupported,
    ParseFailed(String),
    Busy,
    RateLimited,
    Upstream(String),
    Internal(String),
}

impl RenderError {
    fn status(&self) -> StatusCode {
        match self {
            RenderError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RenderError::NotFound(_) => StatusCode::NOT_FOUND,
            RenderError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            RenderError::Unsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            RenderError::ParseFailed(_) => StatusCode::UNPROCESSABLE_ENTITY,
            RenderError::Busy => StatusCode::SERVICE_UNAVAILABLE,
            RenderError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            RenderError::Upstream(_) => StatusCode::BAD_GATEWAY,
            RenderError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn message(&self) -> String {
        match self {
            RenderError::BadRequest(m)
            | RenderError::NotFound(m)
            | RenderError::TooLarge(m)
            | RenderError::ParseFailed(m)
            | RenderError::Upstream(m)
            | RenderError::Internal(m) => m.clone(),
            RenderError::Unsupported => "Unsupported file format".to_string(),
            RenderError::Busy => "Server busy — try again later".to_string(),
            RenderError::RateLimited => {
                "GitHub API rate limit exceeded — try again later".to_string()
            }
        }
    }

    /// Transient errors may succeed on retry, so clients must not cache them.
    fn cache_control(&self) -> &'static str {
        match self {
            RenderError::Busy
            | RenderError::RateLimited
            | RenderError::Upstream(_)
            | RenderError::Internal(_) => NO_STORE,
            _ => MUTABLE_CACHE,
        }
    }
}

impl From<GhError> for RenderError {
    fn from(e: GhError) -> Self {
        match e {
            GhError::NotFound => RenderError::NotFound("File not found on GitHub".to_string()),
            GhError::RateLimited => RenderError::RateLimited,
            GhError::TooLarge(limit_mb) => {
                RenderError::TooLarge(format!("File too large ({limit_mb} MB limit)"))
            }
            GhError::Other(msg) => RenderError::Upstream(format!("GitHub error: {msg}")),
        }
    }
}

/// A validated request for one file in one repository.
struct Target {
    repo: String,
    path: String,
    git_ref: Option<String>,
}

impl Target {
    fn from_params(params: &GhRenderParams) -> Result<Self, RenderError> {
        let file = params.file.trim_matches('/');
        if file.contains("..") {
            return Err(RenderError::BadRequest("Invalid path".to_string()));
        }
        let parts: Vec<&str> = file.splitn(3, '/').collect();
        if parts.len() < 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(RenderError::BadRequest(
                "Use: ?file=owner/repo/path/to/file".to_string(),
            ));
        }
        if pcb_extract::detect_format(std::path::Path::new(parts[2])).is_none() {
            return Err(RenderError::Unsupported);
        }
        if let Some(r) = &params.git_ref {
            if !valid_git_ref(r) {
                return Err(RenderError::BadRequest("Invalid ref".to_string()));
            }
        }
        Ok(Self {
            repo: format!("{}/{}", parts[0], parts[1]),
            path: parts[2].to_string(),
            git_ref: params.git_ref.clone(),
        })
    }

    fn immutable_ref(&self) -> Option<&str> {
        self.git_ref.as_deref().filter(|r| is_commit_sha(r))
    }

    fn filename(&self) -> String {
        std::path::Path::new(&self.path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string())
    }
}

/// GET /gh-render?file=owner/repo/path/to/file.kicad_pcb&ref=main&format=svg
pub async fn gh_render(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<GhRenderParams>,
) -> Response {
    let format = params.format;
    let target = match Target::from_params(&params) {
        Ok(target) => target,
        Err(e) => return error_response(format, &e),
    };

    // A commit-pinned URL always yields the same response, so any validator
    // the client holds for it is current.
    if let Some(commit) = target.immutable_ref() {
        if etag_matches(&headers, commit) {
            return not_modified(commit, IMMUTABLE_CACHE);
        }
    }

    let (git_ref, rendered) = match resolve(&state, &target, params.secret).await {
        Ok(found) => found,
        Err(e) => return error_response(format, &e),
    };

    let (etag, cache) = match target.immutable_ref() {
        Some(commit) => (commit.to_string(), IMMUTABLE_CACHE),
        None => (rendered.blob_sha.clone(), MUTABLE_CACHE),
    };
    if etag_matches(&headers, &etag) {
        return not_modified(&etag, cache);
    }

    let mut out = HeaderMap::new();
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    out.insert(header::ETAG, quoted(&etag));
    if let Ok(id) = HeaderValue::from_str(&rendered.bom_id) {
        out.insert("x-pastebom-id", id);
    }

    match format {
        OutputFormat::Json => {
            let base_url =
                std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
            let body = RenderJson {
                viewer_url: format!("{base_url}/b/{}", rendered.bom_id),
                thumb_url: format!("{base_url}/b/{}/thumb.svg", rendered.bom_id),
                id: rendered.bom_id,
                components: rendered.components,
                blob_sha: rendered.blob_sha,
                git_ref,
            };
            (StatusCode::OK, out, Json(body)).into_response()
        }
        OutputFormat::Svg => match crate::routes::load_thumbnail(&state, &rendered.bom_id).await {
            Ok(svg) => {
                out.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("image/svg+xml"),
                );
                (StatusCode::OK, out, svg).into_response()
            }
            Err((_, msg)) => error_response(format, &RenderError::Internal(msg.to_string())),
        },
    }
}

/// Resolve the target to a rendered board, returning the ref it was found on.
async fn resolve(
    state: &AppState,
    target: &Target,
    secret: bool,
) -> Result<(String, Rendered), RenderError> {
    if let Some(commit) = target.immutable_ref() {
        return resolve_immutable(state, target, commit, secret)
            .await
            .map(|r| (commit.to_string(), r));
    }
    match &target.git_ref {
        Some(r) => resolve_mutable(state, target, &[r.as_str()], secret).await,
        None => resolve_mutable(state, target, &DEFAULT_BRANCHES, secret).await,
    }
}

/// Commit-pinned content never changes: serve from the index when possible,
/// otherwise download once (raw downloads do not use the GitHub API quota).
async fn resolve_immutable(
    state: &AppState,
    target: &Target,
    commit: &str,
    secret: bool,
) -> Result<Rendered, RenderError> {
    let ref_key = build_cache_key(&target.repo, commit, &target.path);
    if let Some(result) = cached_ref(state, &ref_key).await {
        return result;
    }

    let _guard = state.github.lock(&ref_key).await;
    if let Some(result) = cached_ref(state, &ref_key).await {
        return result;
    }

    let bytes = download_raw(state, &target.repo, commit, &target.path).await?;
    let blob_sha = git_blob_sha(&bytes);
    let result = render_blob(state, target, commit, &blob_sha, Some(bytes), secret).await;
    remember_ref(state, &ref_key, &blob_sha, &result).await;
    result
}

/// Branch and tag refs can move: ask GitHub for the current blob SHA, then
/// reuse any render of that content.
async fn resolve_mutable(
    state: &AppState,
    target: &Target,
    candidates: &[&str],
    secret: bool,
) -> Result<(String, Rendered), RenderError> {
    for git_ref in candidates {
        let ref_key = build_cache_key(&target.repo, git_ref, &target.path);
        let blob_sha = match fetch_blob_sha(state, &target.repo, &target.path, git_ref).await {
            Ok(sha) => sha,
            Err(GhError::NotFound) => continue,
            Err(GhError::RateLimited) => {
                // Serve the last known render rather than failing outright.
                return match cached_ref(state, &ref_key).await {
                    Some(result) => result.map(|r| (git_ref.to_string(), r)),
                    None => Err(RenderError::RateLimited),
                };
            }
            Err(e) => return Err(e.into()),
        };
        let result = render_blob(state, target, git_ref, &blob_sha, None, secret).await;
        remember_ref(state, &ref_key, &blob_sha, &result).await;
        return result.map(|r| (git_ref.to_string(), r));
    }
    Err(RenderError::NotFound(match candidates {
        [single] => format!("File not found on {single}"),
        _ => format!("File not found on {}", candidates.join(" or ")),
    }))
}

/// Return the render for `blob_sha`, parsing it if this content has not been
/// seen before. `bytes` may be supplied when already downloaded.
async fn render_blob(
    state: &AppState,
    target: &Target,
    git_ref: &str,
    blob_sha: &str,
    bytes: Option<Vec<u8>>,
    secret: bool,
) -> Result<Rendered, RenderError> {
    if let Some(result) = lookup_blob(state, blob_sha).await {
        return result;
    }
    let _guard = state.github.lock(&blob_key(blob_sha)).await;
    if let Some(result) = lookup_blob(state, blob_sha).await {
        return result;
    }

    let bytes = match bytes {
        Some(bytes) => bytes,
        None => download_raw(state, &target.repo, git_ref, &target.path).await?,
    };
    let file_path = std::path::Path::new(&target.path);
    let format = pcb_extract::detect_format_with_content(file_path, &bytes)
        .ok_or(RenderError::Unsupported)?;

    let pcb_data = match crate::routes::parse_pcb_guarded(state, bytes.clone(), format).await {
        Ok(data) => data,
        Err(ParseError::Busy) => return Err(RenderError::Busy),
        Err(ParseError::Failed(error)) => {
            tracing::warn!(
                "gh-render parse failed for {}/{}: {error}",
                target.repo,
                target.path
            );
            let entry = BlobEntry::Failed {
                error: error.clone(),
                parser_version: PARSER_VERSION.to_string(),
            };
            put_json(state, &blob_key(blob_sha), &entry).await;
            return Err(RenderError::ParseFailed(error));
        }
    };

    let filename = target.filename();
    let bom_id = Uuid::new_v4().to_string();
    let components = pcb_data.footprints.len();
    let file_size = bytes.len();

    // Every distinct version keeps its own raw upload and board.
    let _ = state
        .s3
        .put_object(
            &format!("uploads/{bom_id}/{filename}"),
            bytes,
            "application/octet-stream",
        )
        .await;

    let pcbdata_json = serde_json::to_vec(&pcb_data)
        .map_err(|_| RenderError::Internal("Serialization failed".to_string()))?;
    state
        .s3
        .put_object(
            &format!("boms/{bom_id}.json"),
            pcbdata_json,
            "application/json",
        )
        .await
        .map_err(|_| RenderError::Internal("Storage failed".to_string()))?;

    let meta = serde_json::json!({
        "id": bom_id,
        "filename": filename,
        "components": components,
        "file_size": file_size,
        "github_repo": target.repo,
        "github_path": target.path,
        "github_ref": git_ref,
        "github_blob_sha": blob_sha,
    });
    put_json(state, &format!("boms/{bom_id}.meta.json"), &meta).await;

    if !secret {
        crate::routes::add_recent(
            state,
            crate::routes::RecentEntry {
                id: bom_id.clone(),
                filename,
                components,
                file_size,
                created: chrono::Utc::now().to_rfc3339(),
            },
        )
        .await;
    }

    let svg = tokio::task::spawn_blocking(move || pcb_extract::thumbnail::render_svg(&pcb_data))
        .await
        .map_err(|_| RenderError::Internal("Thumbnail render failed".to_string()))?;
    let _ = state
        .s3
        .put_object(
            &format!("thumbnails/{bom_id}.svg"),
            svg.into_bytes(),
            "image/svg+xml",
        )
        .await;

    // Publish to the index last, once everything it points at is stored.
    let entry = BlobEntry::Rendered {
        bom_id: bom_id.clone(),
        components,
    };
    put_json(state, &blob_key(blob_sha), &entry).await;

    Ok(Rendered {
        bom_id,
        components,
        blob_sha: blob_sha.to_string(),
    })
}

/// Look up the render a ref last resolved to.
async fn cached_ref(state: &AppState, ref_key: &str) -> Option<Result<Rendered, RenderError>> {
    let entry: RefEntry = get_json(state, ref_key).await?;
    lookup_blob(state, &entry.blob_sha).await
}

/// Look up the render for a blob. Failures recorded by an older parser are
/// ignored so parser fixes get a fresh attempt.
async fn lookup_blob(state: &AppState, blob_sha: &str) -> Option<Result<Rendered, RenderError>> {
    match get_json(state, &blob_key(blob_sha)).await? {
        BlobEntry::Rendered { bom_id, components } => Some(Ok(Rendered {
            bom_id,
            components,
            blob_sha: blob_sha.to_string(),
        })),
        BlobEntry::Failed {
            error,
            parser_version,
        } if parser_version == PARSER_VERSION => Some(Err(RenderError::ParseFailed(error))),
        BlobEntry::Failed { .. } => None,
    }
}

/// Point a ref at a blob once its outcome is settled (rendered or failed).
async fn remember_ref(
    state: &AppState,
    ref_key: &str,
    blob_sha: &str,
    result: &Result<Rendered, RenderError>,
) {
    if !matches!(result, Ok(_) | Err(RenderError::ParseFailed(_))) {
        return;
    }
    if let Some(entry) = get_json::<RefEntry>(state, ref_key).await {
        if entry.blob_sha == blob_sha {
            return;
        }
    }
    let entry = RefEntry {
        blob_sha: blob_sha.to_string(),
    };
    put_json(state, ref_key, &entry).await;
}

async fn get_json<T: serde::de::DeserializeOwned>(state: &AppState, key: &str) -> Option<T> {
    let bytes = state.s3.get_object(key).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn put_json<T: Serialize>(state: &AppState, key: &str, value: &T) {
    if let Ok(json) = serde_json::to_vec(value) {
        let _ = state.s3.put_object(key, json, "application/json").await;
    }
}

fn build_cache_key(repo: &str, git_ref: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let path_hash = hex::encode(hasher.finalize());
    format!("gh/{repo}/{git_ref}/{path_hash}.json")
}

fn blob_key(blob_sha: &str) -> String {
    format!("gh-blob/{blob_sha}.json")
}

fn quoted(tag: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{tag}\"")).unwrap_or(HeaderValue::from_static("\"\""))
}

/// Whether `If-None-Match` lists `tag` (strong or weak form) or `*`.
fn etag_matches(headers: &HeaderMap, tag: &str) -> bool {
    let Some(value) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let want = format!("\"{tag}\"");
    value
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == want)
}

fn not_modified(tag: &str, cache: &'static str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::ETAG, quoted(tag));
    (StatusCode::NOT_MODIFIED, headers).into_response()
}

/// SVG callers (e.g. README `<img>` tags) get a 200 error image they can
/// display; JSON callers get the real status code.
fn error_response(format: OutputFormat, err: &RenderError) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(err.cache_control()),
    );
    match format {
        OutputFormat::Svg => {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("image/svg+xml"),
            );
            (StatusCode::OK, headers, error_svg(&err.message())).into_response()
        }
        OutputFormat::Json => (
            err.status(),
            headers,
            Json(serde_json::json!({ "error": err.message() })),
        )
            .into_response(),
    }
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

enum GhError {
    NotFound,
    RateLimited,
    TooLarge(usize),
    Other(String),
}

#[derive(Deserialize)]
struct GitHubContentsResponse {
    sha: String,
}

/// Current blob SHA of `path` at `git_ref`, via the GitHub contents API.
async fn fetch_blob_sha(
    state: &AppState,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> Result<String, GhError> {
    let url = format!(
        "{}/repos/{repo}/contents/{path}?ref={git_ref}",
        state.github.api_base
    );
    let resp = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|e| GhError::Other(format!("GitHub API request failed: {e}")))?;

    match resp.status().as_u16() {
        200 => resp
            .json::<GitHubContentsResponse>()
            .await
            .map(|info| info.sha)
            .map_err(|e| GhError::Other(format!("Failed to parse GitHub response: {e}"))),
        404 => Err(GhError::NotFound),
        403 | 429 => Err(GhError::RateLimited),
        status => Err(GhError::Other(format!(
            "GitHub API returned status {status}"
        ))),
    }
}

async fn download_raw(
    state: &AppState,
    repo: &str,
    git_ref: &str,
    path: &str,
) -> Result<Vec<u8>, GhError> {
    let url = format!("{}/{repo}/{git_ref}/{path}", state.github.raw_base);
    let resp = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|e| GhError::Other(format!("Download failed: {e}")))?;

    let limit = state.max_upload_bytes;
    let too_large = GhError::TooLarge(limit / (1024 * 1024));
    match resp.status().as_u16() {
        200 => {
            if resp
                .content_length()
                .is_some_and(|len| len as usize > limit)
            {
                return Err(too_large);
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| GhError::Other(format!("Failed to read response body: {e}")))?;
            if bytes.len() > limit {
                return Err(too_large);
            }
            Ok(bytes.to_vec())
        }
        404 => Err(GhError::NotFound),
        403 | 429 => Err(GhError::RateLimited),
        status => Err(GhError::Other(format!("GitHub returned status {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_git_ref_accepts_normal_refs() {
        assert!(valid_git_ref("main"));
        assert!(valid_git_ref("master"));
        assert!(valid_git_ref("feature/new-board"));
        assert!(valid_git_ref("v1.2.3"));
        assert!(valid_git_ref("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"));
    }

    #[test]
    fn valid_git_ref_rejects_traversal_and_junk() {
        assert!(!valid_git_ref(""));
        assert!(!valid_git_ref("../../etc/passwd"));
        assert!(!valid_git_ref(".."));
        assert!(!valid_git_ref("/main"));
        assert!(!valid_git_ref("main/"));
        assert!(!valid_git_ref("main?ref=x"));
        assert!(!valid_git_ref("a b"));
    }

    #[test]
    fn commit_sha_detection() {
        assert!(is_commit_sha("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"));
        assert!(is_commit_sha("A1B2C3D4E5F6A7B8C9D0E1F2A3B4C5D6E7F8A9B0"));
        assert!(!is_commit_sha("a1b2c3d"));
        assert!(!is_commit_sha("main"));
        assert!(!is_commit_sha("g1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"));
    }

    #[test]
    fn git_blob_sha_matches_git() {
        // `git hash-object --stdin` outputs.
        assert_eq!(
            git_blob_sha(b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            git_blob_sha(b"hello"),
            "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
        );
    }

    #[test]
    fn etag_matching() {
        let mut headers = HeaderMap::new();
        assert!(!etag_matches(&headers, "abc"));
        headers.insert(header::IF_NONE_MATCH, "\"xyz\", W/\"abc\"".parse().unwrap());
        assert!(etag_matches(&headers, "abc"));
        assert!(etag_matches(&headers, "xyz"));
        assert!(!etag_matches(&headers, "ab"));
        headers.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
        assert!(etag_matches(&headers, "anything"));
    }

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

    #[test]
    fn blob_key_does_not_collide_with_ref_keys() {
        assert!(blob_key("abc").starts_with("gh-blob/"));
        assert!(!build_cache_key("gh-blob", "x", "y").starts_with("gh-blob/"));
    }
}

/// End-to-end `/gh-render` tests against a stub GitHub (API + raw) server.
#[cfg(test)]
mod render_tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::extract::{Path, Query};
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tokio::sync::{RwLock, Semaphore};
    use tower::ServiceExt;

    use super::*;

    const BOARD: &[u8] = br#"(kicad_pcb (version 20230121) (generator "test")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (net 0 "")
  (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts") (width 0.1))
  (gr_line (start 50 0) (end 50 30) (layer "Edge.Cuts") (width 0.1))
  (gr_line (start 50 30) (end 0 30) (layer "Edge.Cuts") (width 0.1))
  (gr_line (start 0 30) (end 0 0) (layer "Edge.Cuts") (width 0.1))
)
"#;
    const BAD_BOARD: &[u8] = b"(footprint \"not a board\")";
    const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Fake GitHub: files keyed by (ref, path) for repo `o/r`.
    #[derive(Default)]
    struct Stub {
        files: std::sync::Mutex<HashMap<(String, String), Vec<u8>>>,
        api_hits: AtomicUsize,
        raw_hits: AtomicUsize,
        rate_limited: AtomicBool,
    }

    impl Stub {
        fn put(&self, git_ref: &str, path: &str, bytes: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert((git_ref.to_string(), path.to_string()), bytes.to_vec());
        }

        fn get(&self, git_ref: &str, path: &str) -> Option<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(&(git_ref.to_string(), path.to_string()))
                .cloned()
        }

        fn api_hits(&self) -> usize {
            self.api_hits.load(Ordering::SeqCst)
        }

        fn raw_hits(&self) -> usize {
            self.raw_hits.load(Ordering::SeqCst)
        }
    }

    #[derive(Deserialize)]
    struct RefQuery {
        #[serde(rename = "ref")]
        git_ref: String,
    }

    async fn stub_contents(
        State(stub): State<Arc<Stub>>,
        Path((_owner, _repo, path)): Path<(String, String, String)>,
        Query(q): Query<RefQuery>,
    ) -> Response {
        stub.api_hits.fetch_add(1, Ordering::SeqCst);
        if stub.rate_limited.load(Ordering::SeqCst) {
            return StatusCode::FORBIDDEN.into_response();
        }
        match stub.get(&q.git_ref, &path) {
            Some(bytes) => Json(serde_json::json!({ "sha": git_blob_sha(&bytes) })).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn stub_raw(
        State(stub): State<Arc<Stub>>,
        Path((_owner, _repo, git_ref, path)): Path<(String, String, String, String)>,
    ) -> Response {
        stub.raw_hits.fetch_add(1, Ordering::SeqCst);
        match stub.get(&git_ref, &path) {
            Some(bytes) => bytes.into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn setup() -> (Router, AppState, Arc<Stub>) {
        let stub = Arc::new(Stub::default());
        let stub_app = Router::new()
            .route(
                "/api/repos/{owner}/{repo}/contents/{*path}",
                get(stub_contents),
            )
            .route("/raw/{owner}/{repo}/{git_ref}/{*path}", get(stub_raw))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, stub_app).await.unwrap() });

        let root = std::env::temp_dir().join(format!("pastebom-gh-render-{}", Uuid::new_v4()));
        let state = AppState {
            s3: crate::s3::S3Client::filesystem(root),
            viewer_dir: PathBuf::from("crates/viewer/dist"),
            gds_viewer_dir: PathBuf::from("crates/gds-viewer/dist"),
            recent: Arc::new(RwLock::new(Vec::new())),
            http_client: reqwest::Client::new(),
            max_upload_bytes: 1024 * 1024,
            parse_semaphore: Arc::new(Semaphore::new(4)),
            github: GhRender::new(&format!("{base}/api"), &format!("{base}/raw")),
        };
        (crate::build_app(state.clone()), state, stub)
    }

    async fn get_req(app: &Router, uri: &str, etag: Option<&str>) -> Response {
        let mut req = Request::get(uri);
        if let Some(etag) = etag {
            req = req.header(header::IF_NONE_MATCH, etag);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn json_body(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_uri(git_ref: &str, path: &str) -> String {
        format!("/gh-render?file=o/r/{path}&ref={git_ref}&format=json&secret=true")
    }

    fn cache_control(resp: &Response) -> &str {
        resp.headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap()
    }

    #[tokio::test]
    async fn commit_ref_never_calls_github_api() {
        let (app, _state, stub) = setup().await;
        stub.put(COMMIT_A, "hw/board.kicad_pcb", BOARD);

        let first = get_req(&app, &json_uri(COMMIT_A, "hw/board.kicad_pcb"), None).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(cache_control(&first), IMMUTABLE_CACHE);
        let first = json_body(first).await;

        let second = get_req(&app, &json_uri(COMMIT_A, "hw/board.kicad_pcb"), None).await;
        assert_eq!(second.status(), StatusCode::OK);
        let second = json_body(second).await;

        assert_eq!(first["id"], second["id"]);
        assert_eq!(first["blob_sha"], git_blob_sha(BOARD));
        assert_eq!(first["ref"], COMMIT_A);
        assert_eq!(stub.api_hits(), 0);
        assert_eq!(stub.raw_hits(), 1);
    }

    #[tokio::test]
    async fn commit_ref_revalidation_is_free() {
        let (app, _state, stub) = setup().await;
        let resp = get_req(
            &app,
            &json_uri(COMMIT_A, "board.kicad_pcb"),
            Some(&format!("\"{COMMIT_A}\"")),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(stub.api_hits() + stub.raw_hits(), 0);
    }

    #[tokio::test]
    async fn identical_content_is_parsed_once_across_refs_and_paths() {
        let (app, state, stub) = setup().await;
        stub.put(COMMIT_A, "a.kicad_pcb", BOARD);
        stub.put(COMMIT_B, "renamed/b.kicad_pcb", BOARD);

        let a = json_body(get_req(&app, &json_uri(COMMIT_A, "a.kicad_pcb"), None).await).await;
        // Any further parse would now fail as busy.
        state.parse_semaphore.close();
        let b = get_req(&app, &json_uri(COMMIT_B, "renamed/b.kicad_pcb"), None).await;
        assert_eq!(b.status(), StatusCode::OK);
        let b = json_body(b).await;

        assert_eq!(a["id"], b["id"]);
        assert_eq!(stub.raw_hits(), 2);
    }

    #[tokio::test]
    async fn concurrent_requests_download_and_parse_once() {
        let (app, _state, stub) = setup().await;
        stub.put(COMMIT_A, "board.kicad_pcb", BOARD);

        let requests = (0..8).map(|_| {
            let app = app.clone();
            tokio::spawn(async move {
                json_body(get_req(&app, &json_uri(COMMIT_A, "board.kicad_pcb"), None).await).await
            })
        });
        let mut ids = Vec::new();
        for handle in requests {
            ids.push(handle.await.unwrap()["id"].clone());
        }
        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(stub.raw_hits(), 1);
    }

    #[tokio::test]
    async fn parse_failures_are_cached() {
        let (app, state, stub) = setup().await;
        stub.put(COMMIT_A, "bad.kicad_pcb", BAD_BOARD);
        stub.put(COMMIT_B, "bad.kicad_pcb", BAD_BOARD);
        stub.put(COMMIT_B, "good.kicad_pcb", BOARD);

        let first = get_req(&app, &json_uri(COMMIT_A, "bad.kicad_pcb"), None).await;
        assert_eq!(first.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(cache_control(&first), MUTABLE_CACHE);

        state.parse_semaphore.close();
        // Same commit: answered from the ref index, no download or parse.
        let again = get_req(&app, &json_uri(COMMIT_A, "bad.kicad_pcb"), None).await;
        assert_eq!(again.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(stub.raw_hits(), 1);
        // Same bytes on another commit: downloaded, but not parsed again.
        let other = get_req(&app, &json_uri(COMMIT_B, "bad.kicad_pcb"), None).await;
        assert_eq!(other.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // New content does need a parse, which the closed semaphore refuses.
        let fresh = get_req(&app, &json_uri(COMMIT_B, "good.kicad_pcb"), None).await;
        assert_eq!(fresh.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(cache_control(&fresh), NO_STORE);
    }

    #[tokio::test]
    async fn stale_parser_failures_are_retried() {
        let (app, state, stub) = setup().await;
        stub.put(COMMIT_A, "board.kicad_pcb", BOARD);
        let stale = BlobEntry::Failed {
            error: "old parser bug".to_string(),
            parser_version: "0.0.0".to_string(),
        };
        put_json(&state, &blob_key(&git_blob_sha(BOARD)), &stale).await;

        let resp = get_req(&app, &json_uri(COMMIT_A, "board.kicad_pcb"), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn branch_ref_checks_api_and_reuses_render() {
        let (app, _state, stub) = setup().await;
        stub.put("main", "board.kicad_pcb", BOARD);

        let first = get_req(&app, &json_uri("main", "board.kicad_pcb"), None).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(cache_control(&first), MUTABLE_CACHE);
        let etag = first.headers().get(header::ETAG).unwrap().clone();
        assert_eq!(etag, format!("\"{}\"", git_blob_sha(BOARD)));
        let first = json_body(first).await;

        let second = get_req(&app, &json_uri("main", "board.kicad_pcb"), None).await;
        assert_eq!(json_body(second).await["id"], first["id"]);
        assert_eq!(stub.api_hits(), 2);
        assert_eq!(stub.raw_hits(), 1);

        let revalidated = get_req(
            &app,
            &json_uri("main", "board.kicad_pcb"),
            Some(etag.to_str().unwrap()),
        )
        .await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn branch_ref_serves_last_render_when_rate_limited() {
        let (app, _state, stub) = setup().await;
        stub.put("main", "board.kicad_pcb", BOARD);
        let first =
            json_body(get_req(&app, &json_uri("main", "board.kicad_pcb"), None).await).await;

        stub.rate_limited.store(true, Ordering::SeqCst);
        let cached = get_req(&app, &json_uri("main", "board.kicad_pcb"), None).await;
        assert_eq!(cached.status(), StatusCode::OK);
        assert_eq!(json_body(cached).await["id"], first["id"]);

        let unseen = get_req(&app, &json_uri("dev", "board.kicad_pcb"), None).await;
        assert_eq!(unseen.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(cache_control(&unseen), NO_STORE);
    }

    #[tokio::test]
    async fn default_ref_falls_back_to_master() {
        let (app, _state, stub) = setup().await;
        stub.put("master", "board.kicad_pcb", BOARD);

        let resp = get_req(
            &app,
            "/gh-render?file=o/r/board.kicad_pcb&format=json&secret=true",
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json_body(resp).await["ref"], "master");
        // One lookup per candidate branch; no repeated calls.
        assert_eq!(stub.api_hits(), 2);
    }

    #[tokio::test]
    async fn json_errors_carry_status_codes() {
        let (app, _state, _stub) = setup().await;
        let cases = [
            (
                json_uri(COMMIT_A, "missing.kicad_pcb"),
                StatusCode::NOT_FOUND,
            ),
            (
                json_uri(COMMIT_A, "notes.txt"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "/gh-render?file=o/r&format=json".to_string(),
                StatusCode::BAD_REQUEST,
            ),
            (json_uri("bad..ref", "a.kicad_pcb"), StatusCode::BAD_REQUEST),
        ];
        for (uri, status) in cases {
            let resp = get_req(&app, &uri, None).await;
            assert_eq!(resp.status(), status, "{uri}");
            assert!(json_body(resp).await["error"].is_string());
        }
    }

    #[tokio::test]
    async fn oversized_files_are_rejected() {
        let (app, _state, stub) = setup().await;
        stub.put(COMMIT_A, "huge.kicad_pcb", &vec![b' '; 2 * 1024 * 1024]);
        let resp = get_req(&app, &json_uri(COMMIT_A, "huge.kicad_pcb"), None).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn svg_mode_returns_thumbnail_with_board_id() {
        let (app, _state, stub) = setup().await;
        stub.put(COMMIT_A, "board.kicad_pcb", BOARD);

        let resp = get_req(
            &app,
            &format!("/gh-render?file=o/r/board.kicad_pcb&ref={COMMIT_A}&secret=true"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );
        assert!(resp.headers().contains_key("x-pastebom-id"));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.starts_with(b"<svg"));
    }

    #[tokio::test]
    async fn svg_mode_errors_stay_displayable() {
        let (app, _state, _stub) = setup().await;
        let resp = get_req(
            &app,
            &format!("/gh-render?file=o/r/missing.kicad_pcb&ref={COMMIT_A}"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("File not found"));
    }
}
