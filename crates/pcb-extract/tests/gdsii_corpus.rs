//! Corpus-driven tests for the GDSII loader.
//!
//! The catalog `gdsii_corpus.json` (PR #247) pins 862 real-world `.gds` files
//! by blob SHA. Two tests consume it:
//!
//! * [`corpus_manifest_is_valid`] — always-on; checks the catalog is
//!   well-formed (runs in CI, no network).
//! * [`corpus_parse_local`] — `#[ignore]`; runs the GDSII pipeline over
//!   pre-fetched corpus files and reports per-case pass/fail, grouped by the
//!   optional `expect` tag. A case may set `"expect": "fail"` (we know it
//!   should be rejected — e.g. a truncated/malformed file) or `"expect": "ok"`
//!   (it must parse). Results print in two groups: **Standard** and
//!   **Expected-fail**, so the intentional failures sort apart from real ones.
//!   The run fails on any parser panic, any `expect=fail` case that parses
//!   cleanly (`XPASS`), or any `expect=ok` case that fails. Untagged files that
//!   error are report-only. Fetch the files first with
//!   `scripts/fetch-gds-corpus.sh`, then:
//!
//!   ```sh
//!   GDS_CORPUS_DIR=/tmp/gds cargo test -p pcb-extract --test gdsii_corpus -- --ignored --nocapture
//!   ```
//!
//! Download is intentionally kept out of the Rust harness (no HTTP dependency),
//! so the fetch script owns the network and integrity check.

use std::time::Instant;

use serde_json::Value;

const CORPUS: &str = include_str!("gdsii_corpus.json");
const TIERS: [&str; 3] = ["smoke", "standard", "stress"];

fn corpus() -> Value {
    serde_json::from_str(CORPUS).expect("gdsii_corpus.json must be valid JSON")
}

#[test]
fn corpus_manifest_is_valid() {
    let doc = corpus();
    let cases = doc["cases"].as_array().expect("`cases` array");
    assert!(!cases.is_empty(), "corpus has no cases");

    let mut ids = std::collections::HashSet::new();
    let mut tier_tally = std::collections::HashMap::<String, usize>::new();
    for (i, c) in cases.iter().enumerate() {
        let id = c["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {i} missing id"));
        assert!(!id.is_empty(), "case {i} has empty id");
        assert!(ids.insert(id.to_string()), "duplicate case id: {id}");

        let url = c["raw_url"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: no raw_url"));
        assert!(url.starts_with("http"), "{id}: raw_url not a URL: {url}");

        let sha = c["blob_sha"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: no blob_sha"));
        assert!(
            sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "{id}: blob_sha not a 40-char hex digest: {sha}"
        );

        let tier = c["tier"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: no tier"));
        assert!(TIERS.contains(&tier), "{id}: unknown tier {tier}");
        *tier_tally.entry(tier.to_string()).or_default() += 1;

        assert!(c["size_bytes"].is_u64(), "{id}: size_bytes not a number");
        assert!(
            c["sources"].as_array().is_some_and(|s| !s.is_empty()),
            "{id}: no sources"
        );

        // If a case declares an expectation it must be one we understand.
        if let Some(expect) = c["expect"].as_str() {
            assert!(
                matches!(expect, "ok" | "fail"),
                "{id}: expect must be \"ok\" or \"fail\", got {expect}"
            );
        }
    }

    // The published tier_counts should match what's actually in `cases`.
    if let Some(counts) = doc["tier_counts"].as_object() {
        for (tier, n) in counts {
            let actual = tier_tally.get(tier).copied().unwrap_or(0);
            assert_eq!(
                n.as_u64().unwrap_or(0) as usize,
                actual,
                "tier_counts[{tier}] disagrees with the cases array"
            );
        }
    }
    eprintln!("corpus OK: {} cases ({:?})", cases.len(), tier_tally);
}

#[test]
#[ignore = "needs pre-fetched files in $GDS_CORPUS_DIR; run with --ignored"]
fn corpus_parse_local() {
    let Ok(dir) = std::env::var("GDS_CORPUS_DIR") else {
        eprintln!("set GDS_CORPUS_DIR (see scripts/fetch-gds-corpus.sh); skipping");
        return;
    };
    let want_tier = std::env::var("GDS_CORPUS_TIER").unwrap_or_else(|_| "smoke".to_string());
    let dir = std::path::PathBuf::from(dir);

    let doc = corpus();
    let cases = doc["cases"].as_array().unwrap();

    // One row per attempted case; we sort and group these for the report.
    struct CaseResult {
        id: String,
        expect: Option<String>, // "ok" | "fail" | None (untagged)
        elapsed: std::time::Duration,
        outcome: Result<(usize, usize, usize), String>,
    }
    let mut results: Vec<CaseResult> = Vec::new();
    let mut missing = 0u32;

    for c in cases {
        let tier = c["tier"].as_str().unwrap_or("");
        if want_tier != "all" && tier != want_tier {
            continue;
        }
        let id = c["id"].as_str().unwrap_or("?");
        let sha = c["blob_sha"].as_str().unwrap_or("");
        // The fetch script names files by blob SHA.
        let path = dir.join(format!("{sha}.gds"));
        let Ok(bytes) = std::fs::read(&path) else {
            missing += 1;
            continue;
        };

        let t = Instant::now();
        // Catch panics so one bad file reports a failure instead of aborting
        // the whole run — a panic here is itself a finding (the DoS caps in
        // the parser are meant to prevent exactly that).
        let result = std::panic::catch_unwind(|| {
            let stream = pcb_extract::parsers::gdsii::tile::stream_records(&bytes)?;
            let n_records = stream.records.len();
            let n_arrays = stream.arrays.len();
            let ts = pcb_extract::parsers::gdsii::tileset::build_tileset(id, &bytes, 3)?;
            Ok::<_, pcb_extract::error::ExtractError>((n_records, n_arrays, ts.tiles.len()))
        });
        let elapsed = t.elapsed();

        let outcome: Result<(usize, usize, usize), String> = match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("parse error: {e}")),
            Err(_) => Err("PANIC".to_string()),
        };

        results.push(CaseResult {
            id: id.to_string(),
            expect: c["expect"].as_str().map(str::to_string),
            elapsed,
            outcome,
        });
    }

    // Split on the `expect: "fail"` tag so the two groups report separately.
    let (expect_fail, standard): (Vec<&CaseResult>, Vec<&CaseResult>) = results
        .iter()
        .partition(|r| r.expect.as_deref() == Some("fail"));

    // Violations that must fail the run, accumulated across both groups.
    let mut panicked: Vec<String> = Vec::new();
    let mut xpass: Vec<String> = Vec::new(); // tagged expect=fail but parsed ok
    let mut expected_ok_failed: Vec<String> = Vec::new(); // tagged expect=ok but failed

    eprintln!("\n=== Standard ({}) ===", standard.len());
    for r in &standard {
        match &r.outcome {
            Ok((recs, arrays, tiles)) => eprintln!(
                "ok    {}  ({recs} recs, {arrays} arrays, {tiles} tiles, {:.2?})",
                r.id, r.elapsed
            ),
            Err(msg) => {
                // A panic is always a defect; a file tagged expect=ok that
                // fails breaks its contract. Untagged errors are report-only.
                if msg == "PANIC" {
                    panicked.push(r.id.clone());
                } else if r.expect.as_deref() == Some("ok") {
                    expected_ok_failed.push(r.id.clone());
                }
                eprintln!("FAIL  {}  {msg}  ({:.2?})", r.id, r.elapsed);
            }
        }
    }

    eprintln!("\n=== Expected-fail ({}) ===", expect_fail.len());
    for r in &expect_fail {
        match &r.outcome {
            // Parsing cleanly despite an expect=fail tag is a mismatch.
            Ok(_) => {
                xpass.push(r.id.clone());
                eprintln!(
                    "XPASS {}  parsed ok but tagged expect=fail  ({:.2?})",
                    r.id, r.elapsed
                );
            }
            Err(msg) => {
                if msg == "PANIC" {
                    panicked.push(r.id.clone());
                    eprintln!("PANIC {}  {msg}  ({:.2?})", r.id, r.elapsed);
                } else {
                    // Rejected as expected — the good outcome for this group.
                    eprintln!("xfail {}  {msg}  ({:.2?})", r.id, r.elapsed);
                }
            }
        }
    }

    let tried = results.len();
    let ok = results.iter().filter(|r| r.outcome.is_ok()).count();
    eprintln!(
        "\nGDSII corpus ({want_tier}): {tried} parsed, {ok} ok, {} failed, {missing} not downloaded \
         [{} expect-fail]",
        tried - ok,
        expect_fail.len(),
    );
    if tried == 0 {
        eprintln!(
            "no corpus files found in {} — run scripts/fetch-gds-corpus.sh",
            dir.display()
        );
    }

    // A panic in the parser is always a defect, and a file tagged expect=fail
    // that parses cleanly means the tag (or the parser) is wrong. Untagged
    // parse errors on real-world files stay report-only.
    assert!(
        panicked.is_empty(),
        "parser panicked on {} case(s): {panicked:?}",
        panicked.len()
    );
    assert!(
        xpass.is_empty(),
        "{} case(s) tagged expect=fail parsed cleanly: {xpass:?}",
        xpass.len()
    );
    assert!(
        expected_ok_failed.is_empty(),
        "{} case(s) tagged expect=ok failed to parse: {expected_ok_failed:?}",
        expected_ok_failed.len()
    );
}
