use std::collections::{HashMap, HashSet};

use schematic_extract::{parse, parse_project};
use vector_view::{svg::SvgOptions, GroupKind, Prop, Role, Scene, SceneKind};

#[test]
fn options_preserve_defaults_and_allow_branding_and_palette() {
    let input = document("");
    let plain = parse(&input).unwrap();
    assert_eq!(
        plain,
        schematic_extract::parse_with(&input, &Default::default()).unwrap()
    );
    let mut options = schematic_extract::ExtractOptions {
        frame_label: Some("kicadmium".into()),
        ..Default::default()
    };
    options.palette.frame = [1, 2, 3, 255];
    let branded = schematic_extract::parse_with(&input, &options).unwrap();
    assert_eq!(branded.layer(11).unwrap().color, [1, 2, 3, 255]);
    assert!(branded
        .items
        .iter()
        .any(|i| prop(&i.props, "text") == Some("kicadmium")));
    assert!(!plain
        .items
        .iter()
        .any(|i| prop(&i.props, "text") == Some("kicadmium")));
    assert_eq!(plain.items.len() + 1, branded.items.len());
}

fn document(contents: &str) -> String {
    format!(
        "(kicad_sch (version 20250114) (generator eeschema) (uuid root) (paper A4) (lib_symbols) {contents})"
    )
}

fn sheet(uuid: &str, name: &str, file: &str) -> String {
    let uuid = serde_json::to_string(uuid).unwrap();
    let name = serde_json::to_string(name).unwrap();
    let file = serde_json::to_string(file).unwrap();
    format!(
        r#"(sheet (at 20 20) (size 30 20) (uuid {uuid})
          (property "Sheetname" {name} (at 20 19 0))
          (property "Sheetfile" {file} (at 20 41 0)))"#
    )
}

fn files(entries: &[(&str, String)]) -> HashMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

fn prop<'a>(props: &'a [Prop], name: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| p.key == name)
        .map(|p| p.value.as_str())
}

fn pages(scene: &Scene) -> Vec<(&str, &str)> {
    scene
        .meta
        .iter()
        .filter_map(|p| {
            Some((
                p.key.strip_prefix("page:")?.strip_suffix(":file")?,
                p.value.as_str(),
            ))
        })
        .collect()
}

const WIRE: &str = r#"(wire (pts (xy 50 50) (xy 60 50))
    (stroke (width 0) (type default)) (uuid wire))
    (label "SIGNAL" (at 50 50 0) (effects (font (size 1.27 1.27))) (uuid label))"#;

#[test]
fn repeated_sheet_instances_have_separate_pages_and_item_ids() {
    let root = document(&format!(
        "{} {}",
        sheet("first", "First", "shared.kicad_sch"),
        sheet("second", "Second", "shared.kicad_sch")
    ));
    let scene = parse_project(
        "root.kicad_sch",
        &files(&[
            ("root.kicad_sch", root),
            ("shared.kicad_sch", document(WIRE)),
        ]),
    )
    .unwrap();
    let pages = pages(&scene);
    assert_eq!(pages.len(), 3);
    assert_eq!(
        pages
            .iter()
            .filter(|(_, file)| *file == "shared.kicad_sch")
            .count(),
        2
    );
    assert_eq!(
        pages
            .iter()
            .map(|(page, _)| page)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    let wires: Vec<_> = scene
        .items
        .iter()
        .filter(|i| i.role == Role::Wire)
        .collect();
    assert_eq!(wires.len(), 2);
    assert_ne!(prop(&wires[0].props, "page"), prop(&wires[1].props, "page"));
    assert_eq!(wires[0].prim, wires[1].prim);
    assert_eq!(
        scene
            .items
            .iter()
            .map(|i| i.id)
            .collect::<HashSet<_>>()
            .len(),
        scene.items.len()
    );
    for group in scene.groups.iter().filter(|g| g.kind == GroupKind::Sheet) {
        let child = prop(&group.props, "child_page").unwrap();
        assert!(pages.iter().any(|(page, _)| *page == child));
        assert_eq!(
            prop(&scene.meta, &format!("page:{child}:parent")),
            Some("/")
        );
    }
}

#[test]
fn punctuation_and_duplicate_sheet_names_do_not_merge_pages() {
    let root = document(&format!(
        "{} {} {}",
        sheet("first", "A/B", "shared.kicad_sch"),
        sheet("second", "A B", "shared.kicad_sch"),
        sheet("third", "A/B", "shared.kicad_sch")
    ));
    let scene = parse_project(
        "root.kicad_sch",
        &files(&[
            ("root.kicad_sch", root),
            ("shared.kicad_sch", document(WIRE)),
        ]),
    )
    .unwrap();
    let pages = pages(&scene);
    assert_eq!(pages.len(), 4);
    assert_eq!(
        pages
            .iter()
            .map(|(page, _)| page)
            .collect::<HashSet<_>>()
            .len(),
        4
    );
}

#[test]
fn direct_and_indirect_recursive_sheets_are_rejected() {
    let direct = files(&[(
        "root.kicad_sch",
        document(&sheet("loop", "Loop", "./root.kicad_sch")),
    )]);
    assert!(parse_project("root.kicad_sch", &direct)
        .unwrap_err()
        .to_string()
        .contains("recursive"));
    let indirect = files(&[
        (
            "root.kicad_sch",
            document(&sheet("child", "Child", "sub/child.kicad_sch")),
        ),
        (
            "sub/child.kicad_sch",
            document(&sheet("loop", "Loop", "../root.kicad_sch")),
        ),
    ]);
    assert!(parse_project("root.kicad_sch", &indirect)
        .unwrap_err()
        .to_string()
        .contains("recursive"));
}

#[test]
fn missing_child_is_a_warning_and_keeps_root_geometry() {
    let scene = parse(&document(&format!(
        "{WIRE} {}",
        sheet("missing", "Missing", "missing.kicad_sch")
    )))
    .unwrap();
    assert_eq!(pages(&scene).len(), 1);
    assert!(scene.items.iter().any(|i| i.role == Role::Wire));
    assert!(scene.items.iter().any(|i| i.role == Role::SheetFrame));
    assert!(scene
        .meta
        .iter()
        .any(|p| p.key == "warning" && p.value.contains("missing.kicad_sch")));
}

#[test]
fn child_paths_resolve_relative_to_their_parent() {
    let scene = parse_project(
        "./design/root.kicad_sch",
        &files(&[
            (
                "design/root.kicad_sch",
                document(&sheet("child", "Child", "../common/./child.kicad_sch")),
            ),
            ("common/child.kicad_sch", document(WIRE)),
        ]),
    )
    .unwrap();
    assert_eq!(prop(&scene.meta, "root"), Some("design/root.kicad_sch"));
    assert!(pages(&scene)
        .iter()
        .any(|(_, file)| *file == "common/child.kicad_sch"));
    assert!(!scene.meta.iter().any(|p| p.key == "warning"));
}

#[test]
fn nested_sheets_record_their_actual_parent_and_quoted_filenames() {
    let scene = parse_project(
        "root.kicad_sch",
        &files(&[
            (
                "root.kicad_sch",
                document(&sheet("child", "Child", "sub/child file.kicad_sch")),
            ),
            (
                "sub/child file.kicad_sch",
                document(&sheet("leaf", "Leaf", "leaf & detail.kicad_sch")),
            ),
            ("sub/leaf & detail.kicad_sch", document(WIRE)),
        ]),
    )
    .unwrap();
    let pages = pages(&scene);
    assert_eq!(pages.len(), 3);
    let child = pages
        .iter()
        .find(|(_, file)| *file == "sub/child file.kicad_sch")
        .unwrap()
        .0;
    let leaf = pages
        .iter()
        .find(|(_, file)| *file == "sub/leaf & detail.kicad_sch")
        .unwrap()
        .0;
    assert_eq!(
        prop(&scene.meta, &format!("page:{leaf}:parent")),
        Some(child)
    );
    assert_ne!(leaf, child);
    assert!(scene
        .items
        .iter()
        .filter(|i| i.role == Role::Wire)
        .all(|i| prop(&i.props, "page") == Some(leaf)));
}

#[test]
fn excessive_hierarchy_depth_returns_a_bounded_error() {
    let mut files = HashMap::new();
    for n in 0..65 {
        let contents = if n == 64 {
            String::new()
        } else {
            sheet(&format!("id{n}"), "Child", &format!("{}.kicad_sch", n + 1))
        };
        files.insert(format!("{n}.kicad_sch"), document(&contents).into_bytes());
    }
    assert!(parse_project("0.kicad_sch", &files)
        .unwrap_err()
        .to_string()
        .contains("hierarchy exceeds"));
}

#[test]
fn paths_cannot_escape_project_or_use_host_absolute_paths() {
    for path in [
        "../outside.kicad_sch",
        "/tmp/outside.kicad_sch",
        "C:/outside.kicad_sch",
        "folder\\outside.kicad_sch",
    ] {
        assert!(parse_project(path, &HashMap::new()).is_err(), "{path}");
        let root = document(&sheet("bad", "Bad", path));
        assert!(
            parse_project("root.kicad_sch", &files(&[("root.kicad_sch", root)])).is_err(),
            "{path}"
        );
    }
}

#[test]
fn invalid_missing_and_non_utf8_roots_return_errors() {
    assert!(parse_project("root.kicad_sch", &HashMap::new()).is_err());
    assert!(parse("(kicad_pcb)").is_err());
    assert!(parse("(kicad_sch").is_err());
    assert!(parse_project(
        "root.kicad_sch",
        &HashMap::from([("root.kicad_sch".into(), vec![0xff])])
    )
    .is_err());
}

#[test]
fn scene_round_trip_and_page_svg_preserve_pick_metadata() {
    let scene = parse_project(
        "root.kicad_sch",
        &files(&[
            (
                "root.kicad_sch",
                document(&sheet("child", "Child & <details>", "child.kicad_sch")),
            ),
            ("child.kicad_sch", document(WIRE)),
        ]),
    )
    .unwrap();
    assert_eq!(scene.kind, SceneKind::Schematic);
    assert!(scene.y_down);
    let json = serde_json::to_string(&scene).unwrap();
    let restored: Scene = serde_json::from_str(&json).unwrap();
    assert_eq!(scene.meta, restored.meta);
    assert_eq!(scene.groups, restored.groups);
    assert_eq!(scene.layers, restored.layers);
    assert_eq!(scene.nets, restored.nets);
    assert_eq!(scene.items.len(), restored.items.len());
    for (before, after) in scene.items.iter().zip(&restored.items) {
        assert_eq!(before.id, after.id);
        assert_eq!(before.props, after.props);
        assert_eq!(before.group, after.group);
        assert_eq!(before.net, after.net);
        assert_eq!(before.layer, after.layer);
        assert_eq!(before.role, after.role);
        assert_json_geometry(
            &serde_json::to_value(&before.prim).unwrap(),
            &serde_json::to_value(&after.prim).unwrap(),
        );
    }

    let root_ids: Vec<_> = scene
        .items
        .iter()
        .filter(|i| prop(&i.props, "page") == Some("/"))
        .map(|i| i.id)
        .collect();
    let svg = scene.to_svg(&SvgOptions {
        item_ids: Some(root_ids.clone()),
        ..SvgOptions::default()
    });
    for item in &scene.items {
        assert_eq!(
            svg.contains(&format!("data-item=\"{}\"", item.id)),
            root_ids.contains(&item.id)
        );
        if item.role == Role::SheetFrame {
            assert!(svg.contains(&format!("data-group=\"{}\"", item.group.unwrap())));
        }
    }
    let child_svg = scene.to_svg(&SvgOptions::default());
    let wire = scene.items.iter().find(|i| i.role == Role::Wire).unwrap();
    assert!(child_svg.contains(&format!("data-net=\"{}\"", wire.net.unwrap())));
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(
        !svg.contains("<text"),
        "text must export as font-free strokes"
    );
    assert!(!svg.contains("NaN"));
    assert!(!svg.contains("Infinity"));
}

fn assert_json_geometry(before: &serde_json::Value, after: &serde_json::Value) {
    use serde_json::Value;
    match (before, after) {
        (Value::Number(a), Value::Number(b)) => {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-10);
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                assert_json_geometry(a, b);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len());
            for (key, value) in a {
                assert_json_geometry(value, &b[key]);
            }
        }
        _ => assert_eq!(before, after),
    }
}
