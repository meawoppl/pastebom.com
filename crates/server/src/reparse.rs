use crate::s3::S3Client;
use pcb_extract::ExtractOptions;
use std::path::Path;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A lightweight struct to check parser_version without deserializing full PcbData.
#[derive(serde::Deserialize)]
struct VersionProbe {
    #[serde(default)]
    parser_version: Option<String>,
}

/// Scan all stored boards and re-parse any with a stale or missing parser_version.
/// Runs as a background task after server startup.
pub async fn reparse_stale_boards(s3: S3Client) {
    let bom_objects = match s3.list_objects("boms/").await {
        Ok(objs) => objs,
        Err(e) => {
            tracing::warn!("Failed to list boms for reparse scan: {e}");
            return;
        }
    };

    let bom_ids: Vec<String> = bom_objects
        .iter()
        .filter_map(|o| {
            o.key
                .strip_prefix("boms/")
                .and_then(|k| k.strip_suffix(".json"))
                .filter(|k| !k.ends_with(".meta"))
                .map(|k| k.to_string())
        })
        .collect();

    tracing::info!(
        "Reparse scan: checking {} boards against parser v{}",
        bom_ids.len(),
        CURRENT_VERSION
    );

    let mut reparsed = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for id in &bom_ids {
        match check_and_reparse(&s3, id).await {
            ReparseResult::Current => {}
            ReparseResult::Reparsed => reparsed += 1,
            ReparseResult::Skipped(reason) => {
                tracing::debug!("Skipped reparse for {id}: {reason}");
                skipped += 1;
            }
            ReparseResult::Failed(e) => {
                tracing::warn!("Reparse failed for {id}: {e}");
                failed += 1;
            }
        }

        // Yield between boards to avoid starving request handling
        tokio::task::yield_now().await;
    }

    if reparsed > 0 || failed > 0 {
        tracing::info!(
            "Reparse complete: {reparsed} updated, {skipped} skipped (no upload), {failed} failed"
        );
    }
}

enum ReparseResult {
    Current,
    Reparsed,
    Skipped(String),
    Failed(String),
}

async fn check_and_reparse(s3: &S3Client, id: &str) -> ReparseResult {
    // Load just the parser_version field
    let bom_key = format!("boms/{id}.json");
    let json_bytes = match s3.get_object(&bom_key).await {
        Ok(b) => b,
        Err(_) => return ReparseResult::Failed("could not read bom json".into()),
    };

    let probe: VersionProbe = match serde_json::from_slice(&json_bytes) {
        Ok(p) => p,
        Err(_) => return ReparseResult::Failed("could not parse bom json".into()),
    };

    if probe.parser_version.as_deref() == Some(CURRENT_VERSION) {
        return ReparseResult::Current;
    }

    let old_version = probe
        .parser_version
        .as_deref()
        .unwrap_or("none")
        .to_string();

    // Find the original upload
    let upload_objects = match s3.list_objects(&format!("uploads/{id}/")).await {
        Ok(objs) => objs,
        Err(_) => return ReparseResult::Skipped("could not list uploads".into()),
    };

    let upload_key = match upload_objects.first() {
        Some(obj) => &obj.key,
        None => return ReparseResult::Skipped("no original upload found".into()),
    };

    let filename = upload_key
        .rsplit('/')
        .next()
        .unwrap_or("upload.bin")
        .to_string();

    let upload_data = match s3.get_object(upload_key).await {
        Ok(d) => d,
        Err(_) => return ReparseResult::Skipped("could not read original upload".into()),
    };

    // Detect format and re-parse
    let path = Path::new(&filename);
    let format = match pcb_extract::detect_format_with_content(path, &upload_data) {
        Some(f) => f,
        None => return ReparseResult::Skipped("could not detect format".into()),
    };

    let pcb_data = match tokio::task::spawn_blocking(move || {
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: true,
        };
        pcb_extract::extract_bytes(&upload_data, format, &opts)
    })
    .await
    {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => return ReparseResult::Failed(format!("parse error: {e}")),
        Err(_) => return ReparseResult::Failed("parse task panicked".into()),
    };

    // Serialize to tempfile and stream to storage
    let tempfile = match crate::routes::serialize_to_tempfile(&pcb_data) {
        Ok(f) => f,
        Err(_) => return ReparseResult::Failed("json serialization failed".into()),
    };

    if let Err(e) = s3
        .put_object_from_file(&bom_key, tempfile.path(), "application/json")
        .await
    {
        return ReparseResult::Failed(format!("could not store updated bom: {e}"));
    }

    // Invalidate cached thumbnail
    let thumb_key = format!("thumbnails/{id}.svg");
    let _ = s3.delete_object(&thumb_key).await;

    tracing::info!("Reparsed {id} ({filename}): v{old_version} -> v{CURRENT_VERSION}");
    ReparseResult::Reparsed
}
