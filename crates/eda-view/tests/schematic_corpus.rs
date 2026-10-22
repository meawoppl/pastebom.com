//! Optional smoke coverage of the locally installed KiCad demonstration corpus.

use std::collections::HashMap;
use std::path::Path;

fn collect(root: &Path, dir: &Path, files: &mut HashMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            collect(root, &path, files);
        } else if kind.is_file() && path.extension().is_some_and(|s| s == "kicad_sch") {
            files.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                std::fs::read(&path).unwrap(),
            );
        }
    }
}

fn no_null_numbers(value: &serde_json::Value) -> bool {
    match value {
        // Scene optional metadata is omitted, so JSON null means a nonfinite
        // geometry number serialized by serde_json.
        serde_json::Value::Null => false,
        serde_json::Value::Array(values) => values.iter().all(no_null_numbers),
        serde_json::Value::Object(values) => values.values().all(no_null_numbers),
        _ => true,
    }
}

#[test]
#[ignore = "requires KiCad demos installed under /usr/share/kicad/demos"]
fn installed_kicad_projects_produce_serializable_finite_scenes() {
    let root = Path::new("/usr/share/kicad/demos");
    let mut files = HashMap::new();
    collect(root, root, &mut files);
    assert!(!files.is_empty(), "no KiCad schematic fixtures installed");
    let mut paths: Vec<_> = files.keys().collect();
    paths.sort();
    let mut failures = Vec::new();
    let mut items = 0;
    for path in &paths {
        eprintln!("Checking {path}");
        match eda_view::schematic::parse_project(path, &files) {
            Ok(scene) => {
                items += scene.items.len();
                let json = serde_json::to_value(&scene).unwrap();
                if !no_null_numbers(&json) {
                    failures.push(format!("{path}: nonfinite geometry serialized to null"));
                }
                if let Err(error) = serde_json::from_value::<eda_view::vector::Scene>(json) {
                    failures.push(format!("{path}: deserialization failed: {error}"));
                }
            }
            Err(error) => failures.push(format!("{path}: {error}")),
        }
    }
    eprintln!(
        "Parsed {} demo roots, {items} scene items, {} failures",
        paths.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
