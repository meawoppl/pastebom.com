use std::collections::HashMap;

use schematic_extract::{parse, parse_project};
use vector_view::{BBox, GroupKind, Prim, Prop, Role, Scene};

const LIBRARY: &str = r#"
    (lib_symbols (symbol "Test:Multi"
      (pin_names hide) (pin_numbers hide)
      (symbol "Multi_0_0"
        (circle (center 0 0) (radius 1))
        (pin power_in line (at -5 0 0) (length 2) (name "VCC") (number "8")))
      (symbol "Multi_1_1"
        (circle (center 0 0) (radius 2))
        (pin input line (at -5 2 0) (length 2) (name "A") (number "1")))
      (symbol "Multi_2_1"
        (circle (center 0 0) (radius 3))
        (pin input line (at -5 3 0) (length 2) (name "B") (number "2")))
      (symbol "Multi_2_2"
        (circle (center 0 0) (radius 4))
        (pin input line (at -5 4 0) (length 2) (name "C") (number "3")))))
"#;

fn document(uuid: &str, content: &str) -> String {
    format!("(kicad_sch (version 20250114) (uuid {uuid}) {LIBRARY} {content})")
}

fn prop<'a>(props: &'a [Prop], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

fn radii(scene: &Scene, group: u32) -> Vec<f64> {
    scene
        .items
        .iter()
        .filter_map(|item| {
            if item.group == Some(group) && item.role == Role::SymbolBody {
                if let Prim::Circle {
                    radius,
                    fill: false,
                    ..
                } = item.prim
                {
                    return Some(radius);
                }
            }
            None
        })
        .collect()
}

#[test]
fn repeated_hierarchy_uses_instance_reference_and_unit_for_fields_and_geometry() {
    let root = document(
        "root",
        r#"
        (sheet (at 0 0) (size 10 10) (uuid first)
          (property "Sheetname" "First") (property "Sheetfile" "child.kicad_sch"))
        (sheet (at 20 0) (size 10 10) (uuid second)
          (property "Sheetname" "Second") (property "Sheetfile" "child.kicad_sch"))
    "#,
    );
    let child = document(
        "child",
        r#"
        (symbol (lib_id "Test:Multi") (at 50 50 0) (unit 1) (uuid part)
          (property "Reference" "U?" (at 50 40 0) (effects (font (size 1 1))))
          (instances (project "root"
            (path "/root/first" (reference "U11") (unit 1))
            (path "/root/second" (reference "U22") (unit 2)))))
    "#,
    );
    let scene = parse_project(
        "root.kicad_sch",
        &HashMap::from([
            ("root.kicad_sch".into(), root.into_bytes()),
            ("child.kicad_sch".into(), child.into_bytes()),
        ]),
    )
    .unwrap();
    let groups: Vec<_> = scene
        .groups
        .iter()
        .filter(|g| g.kind == GroupKind::Symbol)
        .collect();
    assert_eq!(groups.len(), 2);
    assert_ne!(
        prop(&groups[0].props, "page"),
        prop(&groups[1].props, "page")
    );
    for (reference, unit, radius, pin) in [("U11", "1", 2.0, "1"), ("U22", "2", 3.0, "2")] {
        let group = groups.iter().find(|g| g.label == reference).unwrap();
        assert_eq!(prop(&group.props, "reference"), Some(reference));
        assert_eq!(prop(&group.props, "unit"), Some(unit));
        assert_eq!(radii(&scene, group.id), [1.0, radius]);
        let fields: Vec<_> = scene
            .items
            .iter()
            .filter(|i| i.group == Some(group.id) && i.role == Role::Field)
            .collect();
        assert_eq!(fields.len(), 1);
        assert_eq!(prop(&fields[0].props, "text"), Some(reference));
        assert!(matches!(&fields[0].prim, Prim::Strokes { strokes, .. } if !strokes.is_empty()));
        let numbers: Vec<_> = scene
            .items
            .iter()
            .filter(|i| i.group == Some(group.id) && i.role == Role::Pin)
            .map(|i| prop(&i.props, "number").unwrap())
            .collect();
        assert_eq!(
            numbers,
            ["8", pin],
            "common pins and only the selected unit must appear"
        );
    }
    let fields: Vec<_> = scene
        .items
        .iter()
        .filter(|i| i.role == Role::Field)
        .collect();
    assert_ne!(
        fields[0].prim, fields[1].prim,
        "reference stroke geometry must reflect the override"
    );
}

#[test]
fn alternate_body_style_keeps_common_graphics_and_selects_matching_pins() {
    let scene = parse(&document(
        "root",
        r#"
        (symbol (lib_id "Test:Multi") (at 10 10 0) (unit 2) (convert 2) (uuid part)
          (property "Reference" "U2" (at 10 5 0) (effects (font (size 1 1)))))
    "#,
    ))
    .unwrap();
    let group = scene
        .groups
        .iter()
        .find(|g| g.kind == GroupKind::Symbol)
        .unwrap();
    assert_eq!(radii(&scene, group.id), [1.0, 4.0]);
    let numbers: Vec<_> = scene
        .items
        .iter()
        .filter(|i| i.role == Role::Pin)
        .map(|i| prop(&i.props, "number").unwrap())
        .collect();
    assert_eq!(numbers, ["8", "3"]);
}

fn field_geometry(size: f64, justify: &str) -> BBox {
    let scene = parse(&document(
        "root",
        &format!(
            r#"
        (symbol (lib_id "Test:Multi") (at 10 10 0) (unit 1) (uuid part)
          (property "Reference" "HHH" (at 50 50 0)
            (effects (font (size {size} {size})) (justify {justify}))))
    "#
        ),
    ))
    .unwrap();
    let field = scene.items.iter().find(|i| i.role == Role::Field).unwrap();
    let Prim::Strokes { strokes, .. } = &field.prim else {
        panic!("field must be stroked text")
    };
    let mut bounds = BBox::EMPTY;
    for point in strokes.iter().flatten() {
        bounds.include(*point);
    }
    bounds
}

#[test]
fn explicit_field_font_size_and_justification_change_geometry() {
    let small = field_geometry(1.0, "left");
    let large = field_geometry(2.0, "left");
    assert!((large.width() / small.width() - 2.0).abs() < 1e-9);
    assert!((large.height() / small.height() - 2.0).abs() < 1e-9);
    let right = field_geometry(1.0, "right");
    assert!(small.min[0] >= 50.0);
    assert!(right.max[0] <= 50.0);
    assert!((small.width() - right.width()).abs() < 1e-9);
}

#[test]
fn hidden_label_effects_suppress_text_without_losing_connectivity_names() {
    for tag in ["label", "hierarchical_label", "global_label"] {
        for hidden in ["hide", "(hide yes)"] {
            let scene = parse(&document(
                "root",
                &format!(
                    r#"
                (wire (pts (xy 10 10) (xy 20 10)) (uuid wire))
                ({tag} "SECRET" (at 10 10 0) (uuid hidden)
                  (effects (font (size 1 1)) {hidden}))
                ({tag} "VISIBLE" (at 30 30 0) (uuid shown)
                  (effects (font (size 1 1)) (hide no)))
            "#
                ),
            ))
            .unwrap();
            let labels: Vec<_> = scene
                .items
                .iter()
                .filter(|i| i.role == Role::Label)
                .map(|i| prop(&i.props, "text").unwrap())
                .collect();
            assert_eq!(labels, ["VISIBLE"], "{tag} {hidden}");
            let wire = scene.items.iter().find(|i| i.role == Role::Wire).unwrap();
            assert!(scene
                .nets
                .iter()
                .any(|net| Some(net.id) == wire.net && net.name == "SECRET"));
        }
    }
}

#[test]
fn numeric_magnitudes_above_limit_are_rejected() {
    for number in ["1000000001", "-1000000001", "1e10", "-1e10"] {
        let error = parse(&document(
            "root",
            &format!("(wire (pts (xy {number} 0) (xy 0 0)) (uuid wire))"),
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains("numeric magnitude"),
            "{number}: {error}"
        );
    }
}
