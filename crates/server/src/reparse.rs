use crate::s3::S3Client;
use pcb_extract::ExtractOptions;
use std::io::Write;
use std::path::Path;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Storage key holding the board currently being reparsed. Overwritten before each
/// board's memory-spiking work so that, after a SIGKILL/OOM, it names the offender.
const PROGRESS_KEY: &str = "reparse_progress";

/// Skip probing any bom JSON larger than this. The probe reads the whole object into
/// memory to extract two fields, so a pathological legacy artifact (e.g. a 4.3 MB GDS
/// that expanded to 1.2 GB of JSON) would OOM-kill the container. A real PCB bom is at
/// most a few MB, so this only ever rejects degenerate records.
const MAX_REPARSE_BOM_BYTES: u64 = 256 * 1024 * 1024;

/// A lightweight struct to check parser_version/format without deserializing full PcbData.
#[derive(serde::Deserialize)]
struct VersionProbe {
    #[serde(default)]
    parser_version: Option<String>,
    #[serde(default)]
    format: Option<pcb_extract::PcbFormat>,
}

/// True when reparse is switched off via `SKIP_REPARSE` or `DISABLE_REPARSE`.
/// Lets ops stop the crash-loop while triaging an OOM.
fn reparse_disabled() -> bool {
    ["SKIP_REPARSE", "DISABLE_REPARSE"].iter().any(|var| {
        std::env::var(var).is_ok_and(|v| {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "no"
        })
    })
}

/// True when an upload is a format no longer served (GDSII), detected by extension.
fn is_unsupported_upload(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gds") || e.eq_ignore_ascii_case("gdsii"))
}

/// Current resident set size in MiB from `/proc/self/status`. Returns 0 when unreadable
/// (e.g. non-Linux); coarse is fine — it only has to flag a board that spikes but survives.
fn rss_mb() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|k| k.parse::<u64>().ok())
            {
                return kb / 1024;
            }
        }
    }
    0
}

/// Scan all stored boards and re-parse any with a stale or missing parser_version.
/// Runs as a background task after server startup.
pub async fn reparse_stale_boards(s3: S3Client) {
    if reparse_disabled() {
        tracing::info!("Reparse scan disabled via SKIP_REPARSE/DISABLE_REPARSE; skipping");
        return;
    }

    let bom_objects = match s3.list_objects("boms/").await {
        Ok(objs) => objs,
        Err(e) => {
            tracing::warn!("Failed to list boms for reparse scan: {e}");
            return;
        }
    };

    let mut boms: Vec<(String, u64)> = bom_objects
        .iter()
        .filter_map(|o| {
            o.key
                .strip_prefix("boms/")
                .and_then(|k| k.strip_suffix(".json"))
                .filter(|k| !k.ends_with(".meta"))
                .map(|k| (k.to_string(), o.size))
        })
        .collect();

    // Ascending board-id order so "the board it dies on" is reproducible run-to-run.
    boms.sort_by(|a, b| a.0.cmp(&b.0));

    let total = boms.len();
    tracing::info!(
        "Reparse scan: checking {total} boards against parser v{CURRENT_VERSION} (ascending board-id order)"
    );

    let mut reparsed = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for (idx, (id, bom_size)) in boms.iter().enumerate() {
        match check_and_reparse(&s3, id, *bom_size, idx + 1, total).await {
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

async fn check_and_reparse(
    s3: &S3Client,
    id: &str,
    bom_size: u64,
    pos: usize,
    total: usize,
) -> ReparseResult {
    let bom_key = format!("boms/{id}.json");

    // Guard the probe download: reading a multi-gigabyte bom into memory to check two
    // fields OOM-kills the container. Skip (and name it in the log, flushed) before any
    // large read so a single pathological artifact can't take the process down.
    if bom_size > MAX_REPARSE_BOM_BYTES {
        tracing::warn!(
            "reparse: [{pos}/{total}] board={id} skipped: bom json {bom_size} bytes exceeds {MAX_REPARSE_BOM_BYTES} byte probe limit"
        );
        let _ = std::io::stdout().flush();
        return ReparseResult::Current;
    }

    // Load just the parser_version field
    let json_bytes = match s3.get_object(&bom_key).await {
        Ok(b) => b,
        Err(_) => return ReparseResult::Failed("could not read bom json".into()),
    };

    let probe: VersionProbe = match serde_json::from_slice(&json_bytes) {
        Ok(p) => p,
        Err(_) => return ReparseResult::Failed("could not parse bom json".into()),
    };

    // Skip formats no longer supported on the site
    if probe.format == Some(pcb_extract::PcbFormat::Gdsii) {
        return ReparseResult::Current;
    }

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

    let upload_obj = match upload_objects.first() {
        Some(obj) => obj,
        None => return ReparseResult::Skipped("no original upload found".into()),
    };
    let upload_key = &upload_obj.key;
    let upload_bytes = upload_obj.size;

    let filename = upload_key
        .rsplit('/')
        .next()
        .unwrap_or("upload.bin")
        .to_string();

    // Skip formats no longer served. Legacy GDSII records predate the stored `format`
    // field, so the probe check above can't catch them; detect by upload extension and
    // skip before attempting a parse that detect_format would only drop anyway.
    if is_unsupported_upload(&filename) {
        return ReparseResult::Current;
    }

    // Emit + flush a per-board start line and persist a progress marker BEFORE the
    // memory-spiking download/parse. A SIGKILL/OOM leaves no unwind, so whatever is
    // flushed to stdout and written to storage here names the last board attempted.
    tracing::info!(
        "reparse: [{pos}/{total}] board={id} upload_bytes={upload_bytes} parser={old_version}->{CURRENT_VERSION}"
    );
    let _ = std::io::stdout().flush();
    let marker = format!(
        "[{pos}/{total}] board={id} upload_bytes={upload_bytes} parser={old_version}->{CURRENT_VERSION}\n"
    );
    let _ = s3
        .put_object(PROGRESS_KEY, marker.into_bytes(), "text/plain")
        .await;

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

    // Store updated pcbdata
    let pcbdata_json = match serde_json::to_vec(&pcb_data) {
        Ok(j) => j,
        Err(_) => return ReparseResult::Failed("json serialization failed".into()),
    };
    let parsed_bytes = pcbdata_json.len();

    if let Err(e) = s3
        .put_object(&bom_key, pcbdata_json, "application/json")
        .await
    {
        return ReparseResult::Failed(format!("could not store updated bom: {e}"));
    }

    // Invalidate cached thumbnail
    let thumb_key = format!("thumbnails/{id}.svg");
    let _ = s3.delete_object(&thumb_key).await;

    // Completion line with peak-ish RSS so a board that spikes but survives is still visible.
    tracing::info!(
        "reparse: [{pos}/{total}] board={id} done rss={}MB upload_bytes={upload_bytes} parsed_bytes={parsed_bytes} v{old_version}->v{CURRENT_VERSION}",
        rss_mb()
    );
    ReparseResult::Reparsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_legacy_gdsii_uploads_by_extension() {
        assert!(is_unsupported_upload("sky130_sram_1rw1r_80x64_8.gds"));
        assert!(is_unsupported_upload("design.GDS"));
        assert!(is_unsupported_upload("layout.gdsii"));
        assert!(is_unsupported_upload("uploads/id/chip.Gds"));
    }

    #[test]
    fn keeps_supported_uploads() {
        assert!(!is_unsupported_upload("board.kicad_pcb"));
        assert!(!is_unsupported_upload("board.brd"));
        assert!(!is_unsupported_upload("upload.bin"));
        assert!(!is_unsupported_upload("no_extension"));
    }

    #[test]
    fn size_guard_rejects_only_degenerate_boms() {
        // A real PCB bom (a few MB) is probed; a 1.23 GB legacy GDSII artifact is not.
        assert!(5 * 1024 * 1024 <= MAX_REPARSE_BOM_BYTES);
        assert!(1_291_972_963 > MAX_REPARSE_BOM_BYTES);
    }
}
