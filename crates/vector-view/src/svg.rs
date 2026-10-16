//! Standalone, font-free SVG export of scene geometry.
//!
//! Layer and item identifiers are preserved as `data-layer` and `data-item`
//! attributes. Exporting a document page is a matter of selecting its item ids.

use std::fmt::Write;

use crate::{BBox, ItemId, LayerId, Point, Prim, Scene};

/// Controls the exported drawing in scene units (millimetres).
#[derive(Clone, Debug)]
pub struct SvgOptions {
    /// Explicit crop; otherwise bounds are computed from the selected items.
    pub bbox: Option<BBox>,
    pub margin: f64,
    /// Optional opaque or translucent background.
    pub background: Option<[u8; 4]>,
    /// Explicit layer selection overrides the layers' default visibility.
    pub layers: Option<Vec<LayerId>>,
    /// Select a subset, for example items belonging to one schematic sheet.
    pub item_ids: Option<Vec<ItemId>>,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            bbox: None,
            margin: 1.0,
            background: None,
            layers: None,
            item_ids: None,
        }
    }
}

impl Scene {
    /// Export a self-contained SVG, with one group per selected layer.
    pub fn to_svg(&self, options: &SvgOptions) -> String {
        to_svg(self, options)
    }
}

/// Export geometry without a browser, native dependencies, or external fonts.
pub fn to_svg(scene: &Scene, options: &SvgOptions) -> String {
    let mut layers: Vec<_> = scene
        .layers
        .iter()
        .filter(|layer| {
            options
                .layers
                .as_ref()
                .map_or(layer.visible, |ids| ids.contains(&layer.id))
        })
        .collect();
    layers.sort_by_key(|layer| layer.z);
    let selected = |id: ItemId| {
        options
            .item_ids
            .as_ref()
            .is_none_or(|ids| ids.contains(&id))
    };
    let mut bounds = options.bbox.unwrap_or(BBox::EMPTY);
    if options.bbox.is_none() {
        for item in &scene.items {
            if selected(item.id) && layers.iter().any(|layer| layer.id == item.layer) {
                bounds.union(&item.prim.bbox());
            }
        }
    }
    if bounds.is_empty() || !bounds.min.iter().chain(&bounds.max).all(|v| v.is_finite()) {
        bounds = BBox {
            min: [0.0, 0.0],
            max: [1.0, 1.0],
        };
    }
    let margin = if options.margin.is_finite() {
        options.margin.max(0.0)
    } else {
        0.0
    };
    let x = bounds.min[0] - margin;
    let y = if scene.y_down {
        bounds.min[1]
    } else {
        -bounds.max[1]
    } - margin;
    let width = (bounds.width() + margin * 2.0).max(0.001);
    let height = (bounds.height() + margin * 2.0).max(0.001);
    let mut svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{x} {y} {width} {height}\" width=\"{width}mm\" height=\"{height}mm\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n");
    if let Some(color) = options.background {
        writeln!(svg, "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" fill=\"{}\" fill-opacity=\"{}\"/>", color_hex(color), color[3] as f64 / 255.0).unwrap();
    }
    if !scene.y_down {
        svg.push_str("<g transform=\"scale(1 -1)\">\n");
    }
    for layer in layers {
        writeln!(
            svg,
            "<g id=\"layer-{}\" data-layer=\"{}\" data-name=\"{}\" color=\"{}\" opacity=\"{}\">",
            layer.id,
            layer.id,
            escape(&layer.name),
            color_hex(layer.color),
            layer.color[3] as f64 / 255.0
        )
        .unwrap();
        for item in scene
            .items
            .iter()
            .filter(|item| item.layer == layer.id && selected(item.id))
        {
            write!(svg, "<g data-item=\"{}\"", item.id).unwrap();
            if let Some(group) = item.group {
                write!(svg, " data-group=\"{group}\"").unwrap();
            }
            if let Some(net) = item.net {
                write!(svg, " data-net=\"{net}\"").unwrap();
            }
            svg.push_str(">\n");
            primitive(&mut svg, &item.prim);
            svg.push_str("</g>\n");
        }
        svg.push_str("</g>\n");
    }
    if !scene.y_down {
        svg.push_str("</g>\n");
    }
    svg.push_str("</svg>\n");
    svg
}

fn color_hex(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn path(points: &[Point], close: bool, out: &mut String) {
    for (i, point) in points.iter().enumerate() {
        write!(
            out,
            "{}{} {} ",
            if i == 0 { 'M' } else { 'L' },
            point[0],
            point[1]
        )
        .unwrap();
    }
    if close && !points.is_empty() {
        out.push_str("Z ");
    }
}

fn primitive(svg: &mut String, prim: &Prim) {
    match prim {
        Prim::Polyline { points, width } => {
            let mut d = String::new();
            path(points, false, &mut d);
            stroke_path(svg, &d, *width);
        }
        Prim::Strokes { strokes, width } => {
            let mut d = String::new();
            for points in strokes {
                path(points, false, &mut d);
            }
            stroke_path(svg, &d, *width);
        }
        Prim::Polygon {
            outer,
            holes,
            fill,
            stroke,
        } => {
            let mut d = String::new();
            path(outer, true, &mut d);
            for hole in holes {
                path(hole, true, &mut d);
            }
            writeln!(svg, "<path d=\"{d}\" fill=\"{}\" fill-rule=\"evenodd\" stroke=\"{}\" stroke-width=\"{stroke}\"/>", if *fill { "currentColor" } else { "none" }, if *stroke > 0.0 { "currentColor" } else { "none" }).unwrap();
        }
        Prim::Circle {
            center,
            radius,
            fill,
            stroke,
        } => {
            writeln!(svg, "<circle cx=\"{}\" cy=\"{}\" r=\"{radius}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{stroke}\"/>", center[0], center[1], if *fill { "currentColor" } else { "none" }, if *stroke > 0.0 { "currentColor" } else { "none" }).unwrap();
        }
        Prim::Arc {
            center,
            radius,
            start,
            end,
            width,
        } => {
            // Canvas arcs advance in increasing angle, wrapping at one turn.
            let tau = std::f64::consts::TAU;
            let delta = if end - start >= tau {
                tau
            } else {
                (end - start).rem_euclid(tau)
            };
            let mut d = format!(
                "M{} {} ",
                center[0] + radius * start.cos(),
                center[1] + radius * start.sin()
            );
            // Two segments also represent a complete circle (SVG cannot draw
            // a full turn with a single arc whose endpoints coincide).
            for angle in [start + delta / 2.0, start + delta] {
                write!(
                    d,
                    "A{radius} {radius} 0 0 1 {} {} ",
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin()
                )
                .unwrap();
            }
            stroke_path(svg, &d, *width);
        }
        Prim::Hole {
            center,
            size,
            rotation,
        } => {
            let r = size[0].min(size[1]) / 2.0;
            writeln!(svg, "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{r}\" fill=\"currentColor\" transform=\"translate({} {}) rotate({})\"/>", -size[0] / 2.0, -size[1] / 2.0, size[0], size[1], center[0], center[1], rotation.to_degrees()).unwrap();
        }
    }
}

fn stroke_path(svg: &mut String, d: &str, width: f64) {
    writeln!(
        svg,
        "<path d=\"{d}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"{width}\"/>"
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::{to_svg, SvgOptions};
    use crate::{Item, Layer, LayerKind, Prim, Role, Scene, SceneKind, Side};

    #[test]
    fn export_filters_layers_and_items_and_escapes_names() {
        let mut scene = Scene::new(SceneKind::Schematic, false);
        for id in 0..2 {
            scene.layers.push(Layer {
                id,
                name: "<&\"".into(),
                kind: LayerKind::Drawing,
                side: Side::None,
                z: -(id as i32),
                color: [1, 2, 3, 255],
                visible: true,
            });
            scene.items.push(Item {
                id: id.into(),
                layer: id,
                role: Role::Graphic,
                prim: Prim::Circle {
                    center: [10.0, 20.0],
                    radius: 1.0,
                    fill: true,
                    stroke: 0.0,
                },
                net: None,
                group: None,
                props: vec![],
            });
        }
        let svg = to_svg(
            &scene,
            &SvgOptions {
                item_ids: Some(vec![1]),
                ..SvgOptions::default()
            },
        );
        assert!(svg.contains("viewBox=\"8 -22 4 4\""));
        assert!(svg.contains("scale(1 -1)"));
        assert!(svg.contains("&lt;&amp;&quot;"));
        assert!(svg.contains("data-item=\"1\""));
        assert!(!svg.contains("data-item=\"0\""));
        assert!(svg.find("layer-1").unwrap() < svg.find("layer-0").unwrap());
        let svg = scene.to_svg(&SvgOptions {
            layers: Some(vec![0]),
            ..SvgOptions::default()
        });
        assert!(!svg.contains("layer-1"));
    }

    #[test]
    fn empty_scene_has_finite_bounds() {
        let svg = Scene::new(SceneKind::Other, true).to_svg(&SvgOptions::default());
        assert!(!svg.contains("inf"));
        assert!(!svg.contains("NaN"));
    }

    #[test]
    fn polygons_preserve_holes_and_full_arcs_use_two_segments() {
        let mut svg = String::new();
        super::primitive(
            &mut svg,
            &Prim::Polygon {
                outer: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]],
                holes: vec![vec![[1.0, 1.0], [2.0, 1.0], [2.0, 2.0]]],
                fill: true,
                stroke: 0.0,
            },
        );
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert_eq!(svg.matches("Z ").count(), 2);
        svg.clear();
        super::primitive(
            &mut svg,
            &Prim::Arc {
                center: [0.0, 0.0],
                radius: 1.0,
                start: 0.0,
                end: std::f64::consts::TAU,
                width: 0.1,
            },
        );
        assert_eq!(svg.matches("A1 1").count(), 2);
    }
}
