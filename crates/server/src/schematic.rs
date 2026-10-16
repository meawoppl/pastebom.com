//! KiCad schematic ingest and serving.
//!
//! A `.kicad_sch` upload, several KiCad files uploaded together, or a zip of a
//! KiCad project becomes a [`Project`]: the schematic hierarchy is rendered by
//! `schematic-extract` into a `vector_view::Scene` stored at
//! `boms/{id}.schematic.json`, and a `.kicad_pcb` in the project goes down the
//! normal PcbData path. The interactive viewer (`crates/sch-viewer`) is served
//! from `SCH_VIEWER_DIR` at `/sview/` and opened at `/b/{id}/schematic`.

use std::collections::HashMap;
use std::io::Read;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
};
use schematic_extract::vector_view::{svg::SvgOptions, BBox, Scene};
use uuid::Uuid;

use crate::routes::ParseError;
use crate::AppState;

const SEMAPHORE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PARSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Canvas colour baked into exported SVGs, matching the viewer.
const SVG_BACKGROUND: [u8; 4] = [0x1a, 0x1b, 0x26, 255];
/// Root sheet page path in scene metadata.
const ROOT_PAGE: &str = "/";

/// Project files kept from uploads; everything else in a zip is ignored.
/// KiCad always writes these lowercase, and sheet references use the exact
/// filename, so matching is case-sensitive throughout.
const KICAD_EXTENSIONS: [&str; 3] = [".kicad_sch", ".kicad_pcb", ".kicad_pro"];

pub fn scene_key(id: &str) -> String {
    format!("boms/{id}.schematic.json")
}

fn svg_key(id: &str) -> String {
    format!("thumbnails/{id}.schematic.svg")
}

fn is_kicad_file(name: &str) -> bool {
    KICAD_EXTENSIONS.iter().any(|ext| name.ends_with(ext))
}

/// macOS archive debris (`__MACOSX/`, `._name` resource forks).
fn is_resource_fork(name: &str) -> bool {
    name.split('/')
        .any(|part| part == "__MACOSX" || part.starts_with("._"))
}

fn is_zip(name: &str) -> bool {
    name.to_lowercase().ends_with(".zip")
}

/// Whether a zip archive holds a KiCad schematic or board.
pub fn zip_has_kicad(data: &[u8]) -> bool {
    let Ok(archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) else {
        return false;
    };
    let found = archive.file_names().any(|n| {
        !is_resource_fork(n) && (n.ends_with(".kicad_sch") || n.ends_with(".kicad_pcb"))
    });
    found
}

/// Whether an upload of these filenames goes through [`Project`] ingest.
pub fn is_project_upload(uploads: &[(String, Vec<u8>)]) -> bool {
    match uploads {
        [(name, data)] => name.ends_with(".kicad_sch") || (is_zip(name) && zip_has_kicad(data)),
        [] => false,
        _ => true,
    }
}

/// KiCad project files keyed by slash-separated relative path.
#[derive(Debug, Default)]
pub struct Project {
    pub files: HashMap<String, Vec<u8>>,
}

impl Project {
    /// Gather KiCad files from uploads (plain files or zips), keeping at most
    /// `max_bytes` of decompressed content so a zip bomb cannot exhaust memory.
    pub fn collect(uploads: Vec<(String, Vec<u8>)>, max_bytes: usize) -> Result<Self, String> {
        let mut files = HashMap::new();
        let mut total = 0usize;
        for (name, data) in uploads {
            if is_zip(&name) {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
                    .map_err(|e| format!("{name}: not a readable zip ({e})"))?;
                for i in 0..archive.len() {
                    let mut entry = archive
                        .by_index(i)
                        .map_err(|e| format!("{name}: bad zip entry ({e})"))?;
                    let path = entry.name().replace('\\', "/");
                    if entry.is_dir() || is_resource_fork(&path) || !is_kicad_file(&path) {
                        continue;
                    }
                    let budget = max_bytes.saturating_sub(total);
                    let mut buf = Vec::new();
                    (&mut entry)
                        .take(budget as u64 + 1)
                        .read_to_end(&mut buf)
                        .map_err(|e| format!("{name}: failed to read {path} ({e})"))?;
                    if buf.len() > budget {
                        return Err("Project is too large once unzipped".into());
                    }
                    total += buf.len();
                    insert_unique(&mut files, path, buf)?;
                }
            } else if is_kicad_file(&name) {
                total += data.len();
                if total > max_bytes {
                    return Err("Project is too large".into());
                }
                insert_unique(&mut files, base_name(&name).to_string(), data)?;
            } else {
                return Err(format!(
                    "{name}: only KiCad files (.kicad_sch, .kicad_pcb, .kicad_pro) or a zip of them can be uploaded together"
                ));
            }
        }
        Ok(Self { files })
    }

    pub fn has_schematic(&self) -> bool {
        self.files.keys().any(|k| k.ends_with(".kicad_sch"))
    }

    /// The board: the `.kicad_pcb` sharing a `.kicad_pro`'s stem when there is
    /// one, otherwise the shallowest (then alphabetically first).
    pub fn pcb(&self) -> Option<(&str, &[u8])> {
        let mut boards: Vec<&String> = self
            .files
            .keys()
            .filter(|k| k.ends_with(".kicad_pcb"))
            .collect();
        boards.sort_by_key(|k| (k.matches('/').count(), (*k).clone()));
        let pro_stems: Vec<&str> = self
            .files
            .keys()
            .filter_map(|k| k.strip_suffix(".kicad_pro"))
            .collect();
        let chosen = boards
            .iter()
            .find(|b| pro_stems.contains(&b.trim_end_matches(".kicad_pcb")))
            .or(boards.first())?;
        Some((chosen.as_str(), self.files[*chosen].as_slice()))
    }
}

/// Two uploads with the same path would make sheet references ambiguous.
fn insert_unique(
    files: &mut HashMap<String, Vec<u8>>,
    path: String,
    data: Vec<u8>,
) -> Result<(), String> {
    if files.contains_key(&path) {
        return Err(format!("{path} appears more than once in the upload"));
    }
    files.insert(path, data);
    Ok(())
}

/// Final path component of a client filename.
fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Acquire a parse slot and render the project's schematic hierarchy.
pub async fn parse_guarded(
    state: &AppState,
    files: HashMap<String, Vec<u8>>,
) -> Result<Scene, ParseError> {
    let semaphore = state.parse_semaphore.clone();
    let permit = tokio::time::timeout(SEMAPHORE_TIMEOUT, semaphore.acquire_owned())
        .await
        .map_err(|_| ParseError::Busy)?
        .map_err(|_| ParseError::Busy)?;
    // The permit lives with the blocking task, so a timed-out parse still
    // counts against MAX_CONCURRENT_PARSES until it actually finishes.
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        schematic_extract::parse_files(&files)
    });
    tokio::time::timeout(PARSE_TIMEOUT, handle)
        .await
        .map_err(|_| ParseError::Failed("Schematic parsing timed out".to_string()))?
        .map_err(|_| ParseError::Failed("Schematic parse task failed".to_string()))?
        .map_err(|e| ParseError::Failed(format!("Failed to parse schematic: {e}")))
}

/// Symbols placed on the schematic, excluding power symbols (`#PWR`, `#FLG`).
pub fn symbol_count(scene: &Scene) -> usize {
    scene
        .groups
        .iter()
        .filter(|g| {
            g.kind == schematic_extract::vector_view::GroupKind::Symbol && !g.label.starts_with('#')
        })
        .count()
}

/// One sheet as a standalone SVG framed on its paper.
pub fn page_svg(scene: &Scene, page: &str) -> String {
    let item_ids = scene
        .items
        .iter()
        .filter(|i| i.props.iter().any(|p| p.key == "page" && p.value == page))
        .map(|i| i.id)
        .collect();
    let size_key = format!("page:{page}:size");
    let bbox = scene
        .meta
        .iter()
        .find(|p| p.key == size_key)
        .and_then(|p| {
            let (w, h) = p.value.split_once(',')?;
            Some(BBox {
                min: [0.0, 0.0],
                max: [w.parse().ok()?, h.parse().ok()?],
            })
        });
    scene.to_svg(&SvgOptions {
        bbox,
        background: Some(SVG_BACKGROUND),
        item_ids: Some(item_ids),
        ..Default::default()
    })
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

pub async fn exists(state: &AppState, id: &str) -> bool {
    state.s3.get_object(&scene_key(id)).await.is_ok()
}

/// Root sheet SVG, rendered from the stored scene and cached in storage.
pub async fn load_svg(state: &AppState, id: &str) -> Result<Vec<u8>, (StatusCode, &'static str)> {
    let key = svg_key(id);
    if let Ok(cached) = state.s3.get_object(&key).await {
        return Ok(cached);
    }
    let json = state
        .s3
        .get_object(&scene_key(id))
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Schematic not found"))?;
    let svg = tokio::task::spawn_blocking(move || {
        serde_json::from_slice::<Scene>(&json).map(|scene| page_svg(&scene, ROOT_PAGE))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Render failed"))?
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read schematic",
        )
    })?
    .into_bytes();
    let s3 = state.s3.clone();
    let cache = svg.clone();
    tokio::spawn(async move {
        let _ = s3.put_object(&key, cache, "image/svg+xml").await;
    });
    Ok(svg)
}

/// `/b/{id}/schematic`: the schematic viewer shell.
pub async fn get_viewer(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if Uuid::parse_str(&id).is_err() || !exists(&state, &id).await {
        return err(StatusCode::NOT_FOUND, "Schematic not found");
    }
    match tokio::fs::read_to_string(state.sch_viewer_dir.join("index.html")).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Viewer not available"),
    }
}

/// `/b/{id}/schematic.json`: the stored scene.
pub async fn get_scene(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if Uuid::parse_str(&id).is_err() {
        return err(StatusCode::NOT_FOUND, "Schematic not found");
    }
    match state.s3.get_object(&scene_key(&id)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => err(StatusCode::NOT_FOUND, "Schematic not found"),
    }
}

/// `/b/{id}/schematic.svg`: the root sheet as SVG.
pub async fn get_svg(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if Uuid::parse_str(&id).is_err() {
        return err(StatusCode::NOT_FOUND, "Schematic not found");
    }
    match load_svg(&state, &id).await {
        Ok(svg) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            svg,
        )
            .into_response(),
        Err((status, msg)) => err(status, msg),
    }
}

/// Static viewer assets under `/sview/`.
pub async fn serve_assets(State(state): State<AppState>, uri: Uri) -> Response {
    let rel = uri.path().trim_start_matches("/sview/");
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.contains("..") || rel.contains('\\') {
        return err(StatusCode::NOT_FOUND, "not found");
    }
    let path = state.sch_viewer_dir.join(rel);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return err(StatusCode::NOT_FOUND, "asset not found");
    };
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    // Trunk content-hashes everything except index.html.
    let cache = if rel == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        Body::from(bytes),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in entries {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn detects_project_uploads() {
        let sch = vec![("a.kicad_sch".to_string(), b"(kicad_sch)".to_vec())];
        assert!(is_project_upload(&sch));
        let pcb = vec![("a.kicad_pcb".to_string(), Vec::new())];
        assert!(!is_project_upload(&pcb));
        let kicad_zip = zip_of(&[("proj/a.kicad_pcb", b"x")]);
        assert!(is_project_upload(&[("p.zip".into(), kicad_zip)]));
        let gerber_zip = zip_of(&[("top.gtl", b"x"), ("__MACOSX/._a.kicad_sch", b"x")]);
        assert!(!is_project_upload(&[("g.zip".into(), gerber_zip)]));
    }

    #[test]
    fn collect_keeps_kicad_files_and_skips_debris() {
        let data = zip_of(&[
            ("proj/board.kicad_pcb", b"pcb"),
            ("proj/board.kicad_pro", b"{}"),
            ("proj/board.kicad_sch", b"sch"),
            ("proj/sub/child.kicad_sch", b"child"),
            ("proj/fab/top.gtl", b"gerber"),
            ("__MACOSX/proj/._board.kicad_sch", b"fork"),
        ]);
        let p = Project::collect(vec![("p.zip".into(), data)], 1 << 20).unwrap();
        let mut keys: Vec<_> = p.files.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "proj/board.kicad_pcb",
                "proj/board.kicad_pro",
                "proj/board.kicad_sch",
                "proj/sub/child.kicad_sch"
            ]
        );
        assert!(p.has_schematic());
        assert_eq!(p.pcb().unwrap().0, "proj/board.kicad_pcb");
    }

    #[test]
    fn collect_enforces_decompressed_budget() {
        let big = vec![b' '; 4096];
        let data = zip_of(&[("a.kicad_sch", &big)]);
        assert!(Project::collect(vec![("p.zip".into(), data)], 1024).is_err());
    }

    #[test]
    fn collect_rejects_foreign_loose_files() {
        let uploads = vec![
            ("a.kicad_sch".into(), b"x".to_vec()),
            ("notes.txt".into(), b"x".to_vec()),
        ];
        assert!(Project::collect(uploads, 1 << 20).is_err());
    }

    #[test]
    fn pcb_prefers_project_stem() {
        let mut p = Project::default();
        p.files.insert("a/aaa.kicad_pcb".into(), b"1".to_vec());
        p.files.insert("a/main.kicad_pcb".into(), b"2".to_vec());
        p.files.insert("a/main.kicad_pro".into(), b"{}".to_vec());
        assert_eq!(p.pcb().unwrap().0, "a/main.kicad_pcb");
        p.files.remove("a/main.kicad_pro");
        assert_eq!(p.pcb().unwrap().0, "a/aaa.kicad_pcb");
    }

    #[test]
    fn collect_rejects_duplicate_paths() {
        let uploads = vec![
            ("a/board.kicad_sch".into(), b"x".to_vec()),
            ("b/board.kicad_sch".into(), b"y".to_vec()),
        ];
        assert!(Project::collect(uploads, 1 << 20).is_err());
    }

    #[test]
    fn extensions_are_case_sensitive() {
        assert!(!is_project_upload(&[("A.KICAD_SCH".into(), Vec::new())]));
        let uploads = vec![("A.KICAD_SCH".into(), b"x".to_vec())];
        assert!(Project::collect(uploads, 1 << 20).is_err());
    }

    #[test]
    fn loose_files_use_base_names() {
        let uploads = vec![("../x/board.kicad_sch".into(), b"x".to_vec())];
        let p = Project::collect(uploads, 1 << 20).unwrap();
        assert!(p.files.contains_key("board.kicad_sch"));
    }
}
