//! Typed access to the schematic metadata contract, without a parser or font.
//!
//! Missing optional properties use the defaults of older scene producers.
//! Page order follows metadata traversal order, matching title-block numbering.

use std::collections::HashSet;

use crate::{BBox, GroupId, GroupKind, ItemId, Prop, Scene};

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub path: String,
    pub file: String,
    pub parent: Option<String>,
    /// Paper width and height in millimetres, when valid and present.
    pub size: Option<[f64; 2]>,
}

/// A placed symbol unit. Multiple units may have the same reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub group: GroupId,
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub page: String,
    pub uuid: String,
    pub unit: u32,
    pub dnp: bool,
    pub in_bom: bool,
    pub on_board: bool,
}

fn prop<'a>(props: &'a [Prop], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
}

/// Sheets in traversal order. Duplicate page declarations use the first value.
pub fn pages(scene: &Scene) -> Vec<Page> {
    let mut seen = HashSet::new();
    scene
        .meta
        .iter()
        .filter_map(|p| {
            let path = p.key.strip_prefix("page:")?.strip_suffix(":file")?;
            if !seen.insert(path) {
                return None;
            }
            Some(Page {
                path: path.to_owned(),
                file: p.value.clone(),
                parent: parent(scene, path).map(str::to_owned),
                size: page_box(scene, path).map(|b| b.max),
            })
        })
        .collect()
}

/// Paper bounds, excluding invalid, non-finite or nonpositive dimensions.
pub fn page_box(scene: &Scene, page: &str) -> Option<BBox> {
    let (w, h) = prop(&scene.meta, &format!("page:{page}:size"))?.split_once(',')?;
    let size: [f64; 2] = [w.trim().parse().ok()?, h.trim().parse().ok()?];
    size.iter()
        .all(|v| v.is_finite() && *v > 0.0)
        .then_some(BBox {
            min: [0.0, 0.0],
            max: size,
        })
}

/// Items on a page. Legacy items without a page property belong to `/`.
pub fn page_item_ids(scene: &Scene, page: &str) -> HashSet<ItemId> {
    scene
        .items
        .iter()
        .filter(|item| prop(&item.props, "page").unwrap_or("/") == page)
        .map(|item| item.id)
        .collect()
}

/// Direct parent sheet, if declared by the producer.
pub fn parent<'a>(scene: &'a Scene, page: &str) -> Option<&'a str> {
    prop(&scene.meta, &format!("page:{page}:parent"))
}

/// Child sheet targeted by a sheet group.
pub fn child_page(scene: &Scene, group: GroupId) -> Option<&str> {
    let group = scene.group(group)?;
    (group.kind == GroupKind::Sheet)
        .then(|| prop(&group.props, "child_page"))
        .flatten()
}

/// Producer warnings, in their original order.
pub fn warnings(scene: &Scene) -> Vec<&str> {
    scene
        .meta
        .iter()
        .filter(|p| p.key == "warning")
        .map(|p| p.value.as_str())
        .collect()
}

/// Placed symbol units, including power symbols and excluded BOM parts.
pub fn symbols(scene: &Scene) -> Vec<Symbol> {
    scene
        .groups
        .iter()
        .filter(|g| g.kind == GroupKind::Symbol)
        .map(|g| {
            let text = |key| prop(&g.props, key).unwrap_or("").to_owned();
            let flag = |key, default| {
                prop(&g.props, key)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default)
            };
            Symbol {
                group: g.id,
                reference: prop(&g.props, "reference").unwrap_or(&g.label).to_owned(),
                value: text("value"),
                footprint: text("footprint"),
                uuid: text("uuid"),
                page: prop(&g.props, "page").unwrap_or("/").to_owned(),
                unit: prop(&g.props, "unit")
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(1),
                dnp: flag("dnp", false),
                in_bom: flag("in_bom", true),
                on_board: flag("on_board", true),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Group, Item, Prim, Role, SceneKind};

    #[test]
    fn traversal_and_paper_validation() {
        let mut scene = Scene::new(SceneKind::Schematic, true);
        scene.meta = vec![
            Prop::new("page:/:file", "root.kicad_sch"),
            Prop::new("page:/z:file", "z.kicad_sch"),
            Prop::new("page:/a:file", "a.kicad_sch"),
            Prop::new("page:/z:file", "duplicate"),
            Prop::new("page:/:size", "297,210"),
            Prop::new("page:/z:size", "NaN,210"),
            Prop::new("page:/a:size", "-1,210"),
            Prop::new("page:/z:parent", "/"),
            Prop::new("warning", "Missing sheet"),
        ];
        let result = pages(&scene);
        assert_eq!(
            result.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["/", "/z", "/a"]
        );
        assert_eq!(result[0].size, Some([297.0, 210.0]));
        assert_eq!(result[1].parent.as_deref(), Some("/"));
        assert!(result[1].size.is_none() && result[2].size.is_none());
        assert_eq!(warnings(&scene), ["Missing sheet"]);
        assert_eq!(parent(&scene, "/"), None);
    }

    #[test]
    fn legacy_symbols_and_page_items() {
        let mut scene = Scene::new(SceneKind::Schematic, true);
        scene.groups = vec![
            Group {
                id: 1,
                kind: GroupKind::Symbol,
                label: "R1".into(),
                props: vec![],
            },
            Group {
                id: 2,
                kind: GroupKind::Sheet,
                label: "Subsheet".into(),
                props: vec![Prop::new("child_page", "/sub")],
            },
            Group {
                id: 3,
                kind: GroupKind::Symbol,
                label: "U1".into(),
                props: vec![
                    Prop::new("unit", "2"),
                    Prop::new("in_bom", "false"),
                    Prop::new("dnp", "true"),
                ],
            },
        ];
        for (id, props) in [(1, vec![]), (2, vec![Prop::new("page", "/sub")])] {
            scene.items.push(Item {
                id,
                layer: 0,
                role: Role::Wire,
                prim: Prim::Polyline {
                    points: vec![],
                    width: 0.1,
                },
                net: None,
                group: None,
                props,
            });
        }
        let result = symbols(&scene);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].reference, "R1");
        assert_eq!(result[0].unit, 1);
        assert!(result[0].in_bom && result[0].on_board && !result[0].dnp);
        assert_eq!(result[1].unit, 2);
        assert!(result[1].dnp && !result[1].in_bom);
        assert_eq!(child_page(&scene, 2), Some("/sub"));
        assert_eq!(child_page(&scene, 1), None);
        assert_eq!(page_item_ids(&scene, "/"), HashSet::from([1]));
        assert_eq!(page_item_ids(&scene, "/sub"), HashSet::from([2]));
    }
}
