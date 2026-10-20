//! Library previews using the same geometry and colours as schematics.

use kicad_schema::{schema::library::LibrarySymbol, sexp::SExp};
use vector_view::{
    Group, GroupKind, Item, Layer, LayerKind, Prim, Prop, Role, Scene, SceneKind, Side,
};

use crate::{
    symbol_graphics::{body_prims, Fill},
    Palette,
};

const SYMBOL: u16 = 1;
const PIN: u16 = 2;
const TEXT: u16 = 3;
const FILL: u16 = 4;

/// Preview an already-resolved library symbol. The model contains the selected
/// body graphics; use [`symbol_scene_for_unit`] to select a unit before parsing.
pub fn symbol_scene(symbol: &LibrarySymbol, palette: &Palette) -> Scene {
    let mut scene = Scene::new(SceneKind::Symbol, true);
    scene.layers = [
        (
            FILL,
            "Symbol body fill",
            LayerKind::Symbol,
            5,
            palette.body_fill,
        ),
        (SYMBOL, "Symbol body", LayerKind::Symbol, 10, palette.body),
        (PIN, "Pins", LayerKind::Symbol, 20, palette.pin),
        (TEXT, "Text", LayerKind::Text, 30, palette.pin_text),
    ]
    .into_iter()
    .map(|(id, name, kind, z, color)| Layer {
        id,
        name: name.into(),
        kind,
        side: Side::None,
        z,
        color,
        visible: true,
    })
    .collect();
    scene.groups.push(Group {
        id: 1,
        kind: GroupKind::Symbol,
        label: symbol.name.clone(),
        props: symbol
            .properties
            .iter()
            .map(|(key, value)| Prop::new(key, value))
            .collect(),
    });
    scene.meta.push(Prop::new("symbol", &symbol.name));
    for graphic in &symbol.graphics {
        let Some(body) = body_prims(
            graphic,
            |(x, y)| [x, -y],
            |width| if width > 0.0 { width } else { 0.15 },
        ) else {
            continue;
        };
        if let Some((fill, prim)) = body.fill {
            push(
                &mut scene,
                if fill == Fill::Outline { SYMBOL } else { FILL },
                Role::SymbolBody,
                prim,
                vec![],
            );
        }
        push(&mut scene, SYMBOL, Role::SymbolBody, body.outline, vec![]);
    }
    for pin in &symbol.pins {
        let start = [pin.position.0, -pin.position.1];
        let a = (-pin.rotation).to_radians();
        let end = [
            start[0] + pin.length * a.cos(),
            start[1] + pin.length * a.sin(),
        ];
        push(
            &mut scene,
            PIN,
            Role::Pin,
            Prim::Polyline {
                points: vec![start, end],
                width: 0.15,
            },
            vec![
                Prop::new("number", &pin.number),
                Prop::new("name", &pin.name),
                Prop::new("type", &pin.pin_type),
            ],
        );
        text(&mut scene, &pin.number, start);
        if !pin.name.is_empty() && pin.name != "~" {
            text(&mut scene, &pin.name, end);
        }
    }
    scene.recompute_bbox();
    scene
}

/// Select a unit and body style from a resolved `(symbol ...)` expression.
/// Unit/body-style zero graphics are shared. Resolve `extends` in the source
/// library before calling this function; [`symbol_scene`] accepts resolved models.
pub fn symbol_scene_for_unit(
    symbol: &SExp,
    unit: i64,
    body_style: i64,
    palette: &Palette,
) -> Scene {
    symbol_scene(
        &LibrarySymbol::from_sexp_for_unit(symbol, unit, body_style),
        palette,
    )
}

fn push(scene: &mut Scene, layer: u16, role: Role, prim: Prim, props: Vec<Prop>) {
    scene.items.push(Item {
        id: scene.items.len() as u32 + 1,
        layer,
        role,
        prim,
        net: None,
        group: Some(1),
        props,
    });
}

fn text(scene: &mut Scene, value: &str, pos: [f64; 2]) {
    let strokes = kicad_strokes::to_strokes(&kicad_strokes::TextSpec {
        text: value.into(),
        pos,
        size: [1.0, 1.0],
        y_down: true,
        ..Default::default()
    });
    if !strokes.strokes.is_empty() {
        push(
            scene,
            TEXT,
            Role::Field,
            strokes.into_prim(),
            vec![Prop::new("text", value)],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_body_style_before_flattening_and_uses_palette() {
        let source = kicad_schema::sexp::parse(
            r#"(symbol "U"
          (symbol "U_0_1" (circle (center 0 0) (radius 1) (fill (type background))))
          (symbol "U_1_1" (rectangle (start 0 0) (end 2 2)))
          (symbol "U_2_1" (rectangle (start 0 0) (end 4 4)))
          (symbol "U_1_2" (rectangle (start 0 0) (end 6 6))))"#,
        )
        .unwrap();
        let palette = Palette {
            body: [1, 2, 3, 255],
            ..Palette::default()
        };
        let normal = symbol_scene_for_unit(&source, 1, 1, &palette);
        let alternate = symbol_scene_for_unit(&source, 1, 2, &palette);
        assert_eq!(normal.layer(SYMBOL).unwrap().color, palette.body);
        assert!(normal.items.iter().any(|i| i.layer == FILL));
        assert!(normal.bbox.width() < alternate.bbox.width());
        assert_eq!(normal.kind, SceneKind::Symbol);
    }
}
