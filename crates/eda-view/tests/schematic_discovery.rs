use std::collections::HashMap;

use eda_view::schematic::find_root;

#[test]
fn parse_files_normalizes_keys_and_rejects_ambiguous_aliases() {
    let mut files = std::collections::HashMap::from([
        (
            "./project//root.kicad_sch".into(),
            b"(kicad_sch (paper A4))".to_vec(),
        ),
        ("./project/root.kicad_pro".into(), Vec::new()),
    ]);
    let scene = eda_view::schematic::parse_files(&files).unwrap();
    assert!(scene
        .meta
        .iter()
        .any(|p| p.key == "root" && p.value == "project/root.kicad_sch"));
    files.insert("project/root.kicad_sch".into(), b"(kicad_sch)".to_vec());
    assert!(eda_view::schematic::parse_files(&files)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    assert!(eda_view::schematic::parse_files(&std::collections::HashMap::new()).is_err());
}

fn schematic(children: &[&str]) -> Vec<u8> {
    let sheets: String = children
        .iter()
        .map(|file| {
            format!(
                "(sheet (property \"Sheetfile\" {}))",
                serde_json::to_string(file).unwrap()
            )
        })
        .collect();
    format!("(kicad_sch {sheets})").into_bytes()
}

fn files(entries: &[(&str, Vec<u8>)]) -> HashMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(name, bytes)| ((*name).to_owned(), bytes.clone()))
        .collect()
}

#[test]
fn matching_project_or_board_prefers_a_schematic_in_the_same_directory() {
    for companion in ["kicad_pro", "kicad_pcb"] {
        let mut bundle = files(&[
            ("a.kicad_sch", schematic(&[])),
            ("nested/design.kicad_sch", schematic(&[])),
            ("design.kicad_pro", vec![]),
        ]);
        // A companion in a different directory is not a match.
        assert_eq!(find_root(&bundle).as_deref(), Some("a.kicad_sch"));
        bundle.insert(format!("nested/design.{companion}"), vec![]);
        assert_eq!(
            find_root(&bundle).as_deref(),
            Some("nested/design.kicad_sch")
        );
    }
}

#[test]
fn child_references_select_the_root_even_when_the_child_sorts_first() {
    let bundle = files(&[
        ("a_child.kicad_sch", schematic(&[])),
        (
            "z_root.kicad_sch",
            schematic(&["a_child.kicad_sch", "a_child.kicad_sch"]),
        ),
    ]);
    assert_eq!(find_root(&bundle).as_deref(), Some("z_root.kicad_sch"));
}

#[test]
fn nested_relative_references_are_resolved_without_escaping_the_bundle() {
    let bundle = files(&[
        ("common/child file.kicad_sch", schematic(&[])),
        (
            "design/root.kicad_sch",
            schematic(&["../common/./child file.kicad_sch"]),
        ),
    ]);
    assert_eq!(find_root(&bundle).as_deref(), Some("design/root.kicad_sch"));

    for child in [
        "../a.kicad_sch",
        "/a.kicad_sch",
        "C:/a.kicad_sch",
        "folder\\a.kicad_sch",
    ] {
        let bundle = files(&[
            ("a.kicad_sch", schematic(&[])),
            ("z.kicad_sch", schematic(&[child])),
        ]);
        assert_eq!(
            find_root(&bundle).as_deref(),
            Some("a.kicad_sch"),
            "{child}"
        );
    }
}

#[test]
fn ties_are_shallow_then_lexical_independent_of_hashmap_insertion_order() {
    let entries = [
        ("sub/a.kicad_sch", schematic(&[])),
        ("z.kicad_sch", schematic(&[])),
        ("b.kicad_sch", schematic(&[])),
        ("child.kicad_sch", schematic(&[])),
    ];
    for reverse in [false, true] {
        let mut entries = entries.to_vec();
        if reverse {
            entries.reverse();
        }
        let mut bundle = files(&entries);
        assert_eq!(find_root(&bundle).as_deref(), Some("b.kicad_sch"));
        bundle.insert("b.kicad_sch".into(), schematic(&["child.kicad_sch"]));
        bundle.insert("z.kicad_sch".into(), schematic(&["child.kicad_sch"]));
        assert_eq!(find_root(&bundle).as_deref(), Some("b.kicad_sch"));
        for name in ["sub/a.kicad_pro", "z.kicad_pro", "b.kicad_pcb"] {
            bundle.insert(name.into(), vec![]);
        }
        assert_eq!(find_root(&bundle).as_deref(), Some("b.kicad_sch"));
    }
}

#[test]
fn macos_metadata_is_ignored_in_every_path_component() {
    let bundle = files(&[
        ("__MACOSX/a.kicad_sch", schematic(&[])),
        ("nested/__MACOSX/a.kicad_sch", schematic(&[])),
        ("._a.kicad_sch", schematic(&[])),
        ("._folder/a.kicad_sch", schematic(&[])),
        ("z.kicad_sch", schematic(&[])),
    ]);
    assert_eq!(find_root(&bundle).as_deref(), Some("z.kicad_sch"));
    let only_metadata: HashMap<_, _> = bundle
        .into_iter()
        .filter(|(name, _)| name != "z.kicad_sch")
        .collect();
    assert_eq!(find_root(&only_metadata), None);
}

#[test]
fn malformed_files_and_cycles_remain_candidates_for_parser_errors() {
    for content in [
        b"(kicad_sch".to_vec(),
        vec![0xff],
        format!("{}{}", "(".repeat(200), ")".repeat(200)).into_bytes(),
    ] {
        assert_eq!(
            find_root(&files(&[("broken.kicad_sch", content)])).as_deref(),
            Some("broken.kicad_sch")
        );
    }
    let cycle = files(&[
        ("a.kicad_sch", schematic(&["b.kicad_sch"])),
        ("b.kicad_sch", schematic(&["a.kicad_sch"])),
    ]);
    assert_eq!(find_root(&cycle).as_deref(), Some("a.kicad_sch"));
    assert_eq!(find_root(&files(&[("readme.txt", vec![])])), None);
}
