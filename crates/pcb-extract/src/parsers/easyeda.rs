use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use serde_json::Value;
use std::collections::HashMap;

/// Parse an EasyEDA PCB JSON file into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let text = std::str::from_utf8(data)
        .map_err(|e| ExtractError::ParseError(format!("Invalid UTF-8: {e}")))?;
    let root: Value = serde_json::from_str(text)?;

    // EasyEDA JSON can be a single object or array of objects
    let pcb_obj = if root.is_array() {
        root.as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|o| o.get("docType").and_then(|d| d.as_str()) == Some("5"))
            })
            .ok_or_else(|| ExtractError::ParseError("No PCB document in array".to_string()))?
    } else {
        &root
    };

    let canvas = pcb_obj.get("canvas").and_then(|c| c.as_str()).unwrap_or("");
    let canvas_parts: Vec<&str> = canvas.split('~').collect();
    let origin_x: f64 = canvas_parts
        .get(16)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let origin_y: f64 = canvas_parts
        .get(17)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let mut edges = Vec::new();
    let mut silk_f = Vec::new();
    let mut silk_b = Vec::new();
    let mut fab_f = Vec::new();
    let mut fab_b = Vec::new();
    let mut footprints = Vec::new();
    let mut components = Vec::new();
    let mut track_f = Vec::new();
    let mut track_b = Vec::new();

    // Parse shapes from the root level
    if let Some(shape_str) = pcb_obj.get("shape").and_then(|s| s.as_array()) {
        for shape_val in shape_str {
            if let Some(s) = shape_val.as_str() {
                parse_shape(
                    s,
                    origin_x,
                    origin_y,
                    &mut edges,
                    &mut silk_f,
                    &mut silk_b,
                    &mut fab_f,
                    &mut fab_b,
                    &mut track_f,
                    &mut track_b,
                );
            }
        }
    }

    // Parse footprints from "components" array (newer EasyEDA format)
    if let Some(components_arr) = pcb_obj.get("components").and_then(|c| c.as_array()) {
        for comp_obj in components_arr {
            if let Some((fp, comp)) =
                parse_easyeda_component(comp_obj, origin_x, origin_y, footprints.len())
            {
                footprints.push(fp);
                components.push(comp);
            }
        }
    }

    // Parse footprints from "dataStr" (EasyEDA Pro / standard format)
    if let Some(data_str) = pcb_obj.get("dataStr") {
        if let Some(routes) = data_str.get("routes") {
            if let Some(arr) = routes.as_array() {
                for comp_obj in arr {
                    if let Some((fp, comp)) =
                        parse_easyeda_component(comp_obj, origin_x, origin_y, footprints.len())
                    {
                        footprints.push(fp);
                        components.push(comp);
                    }
                }
            }
        }
    }

    let edges_bbox = compute_bbox(&edges);
    let bom = Some(generate_bom(
        &footprints,
        &components,
        &BomConfig::default(),
    ));

    let tracks = if opts.include_tracks {
        Some(LayerData {
            front: track_f,
            back: track_b,
            inner: HashMap::new(),
        })
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox,
        edges,
        drawings: Drawings {
            silkscreen: LayerData {
                front: silk_f,
                back: silk_b,
                inner: HashMap::new(),
            },
            fabrication: LayerData {
                front: fab_f,
                back: fab_b,
                inner: HashMap::new(),
            },
        },
        footprints,
        metadata: Metadata {
            title: String::new(),
            revision: String::new(),
            company: String::new(),
            date: String::new(),
        },
        bom,
        ibom_version: None,
        tracks,
        copper_pads: None,
        zones: None,
        nets: None,
        font_data: None,
    })
}

fn mil_to_mm(mil: f64) -> f64 {
    mil * 0.0254
}

#[derive(Debug, PartialEq)]
enum EasyEdaLayerCat {
    CopperF,
    CopperB,
    SilkF,
    SilkB,
    Edge,
    Other,
}

fn categorize_easyeda_layer(layer_id: u32) -> EasyEdaLayerCat {
    match layer_id {
        1 => EasyEdaLayerCat::CopperF,
        2 => EasyEdaLayerCat::CopperB,
        3 => EasyEdaLayerCat::SilkF,
        4 => EasyEdaLayerCat::SilkB,
        10 => EasyEdaLayerCat::Edge,
        _ => EasyEdaLayerCat::Other,
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_shape(
    shape: &str,
    origin_x: f64,
    origin_y: f64,
    edges: &mut Vec<Drawing>,
    silk_f: &mut Vec<Drawing>,
    silk_b: &mut Vec<Drawing>,
    _fab_f: &mut Vec<Drawing>,
    _fab_b: &mut Vec<Drawing>,
    track_f: &mut Vec<Track>,
    track_b: &mut Vec<Track>,
) {
    let parts: Vec<&str> = shape.split('~').collect();
    if parts.is_empty() {
        return;
    }

    match parts[0] {
        "TRACK" => {
            if parts.len() < 4 {
                return;
            }
            let width = mil_to_mm(parts[1].parse::<f64>().unwrap_or(0.0));
            let layer: u32 = parts[2].parse().unwrap_or(0);
            // Points are space-separated pairs
            let coords: Vec<f64> = parts[3]
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            for i in (0..coords.len().saturating_sub(2)).step_by(2) {
                let start = [
                    mil_to_mm(coords[i] - origin_x),
                    mil_to_mm(coords[i + 1] - origin_y),
                ];
                let end = [
                    mil_to_mm(coords[i + 2] - origin_x),
                    mil_to_mm(coords[i + 3] - origin_y),
                ];
                let drawing = Drawing::Segment { start, end, width };
                match categorize_easyeda_layer(layer) {
                    EasyEdaLayerCat::Edge => edges.push(drawing),
                    EasyEdaLayerCat::SilkF => silk_f.push(drawing),
                    EasyEdaLayerCat::SilkB => silk_b.push(drawing),
                    EasyEdaLayerCat::CopperF => track_f.push(Track::Segment {
                        start,
                        end,
                        width,
                        net: None,
                        drillsize: None,
                    }),
                    EasyEdaLayerCat::CopperB => track_b.push(Track::Segment {
                        start,
                        end,
                        width,
                        net: None,
                        drillsize: None,
                    }),
                    _ => {}
                }
            }
        }
        "CIRCLE" => {
            if parts.len() < 6 {
                return;
            }
            let cx = mil_to_mm(parts[1].parse::<f64>().unwrap_or(0.0) - origin_x);
            let cy = mil_to_mm(parts[2].parse::<f64>().unwrap_or(0.0) - origin_y);
            let radius = mil_to_mm(parts[3].parse::<f64>().unwrap_or(0.0));
            let width = mil_to_mm(parts[4].parse::<f64>().unwrap_or(0.0));
            let layer: u32 = parts[5].parse().unwrap_or(0);
            let drawing = Drawing::Circle {
                start: [cx, cy],
                radius,
                width,
                filled: None,
            };
            match categorize_easyeda_layer(layer) {
                EasyEdaLayerCat::Edge => edges.push(drawing),
                EasyEdaLayerCat::SilkF => silk_f.push(drawing),
                EasyEdaLayerCat::SilkB => silk_b.push(drawing),
                _ => {}
            }
        }
        "ARC" => {
            if parts.len() < 6 {
                return;
            }
            let width = mil_to_mm(parts[1].parse::<f64>().unwrap_or(0.0));
            let layer: u32 = parts[2].parse().unwrap_or(0);
            // EasyEDA arcs use SVG path notation - simplified handling
            let drawing = Drawing::Segment {
                start: [0.0, 0.0],
                end: [0.0, 0.0],
                width,
            };
            if categorize_easyeda_layer(layer) == EasyEdaLayerCat::Edge {
                edges.push(drawing);
            }
        }
        _ => {}
    }
}

fn parse_easyeda_component(
    comp: &Value,
    origin_x: f64,
    origin_y: f64,
    fp_index: usize,
) -> Option<(Footprint, Component)> {
    let shape_arr = comp.get("shape").and_then(|s| s.as_array())?;

    let package_detail = comp.get("packageDetail").and_then(|p| p.as_object());

    let designator = comp
        .get("c_para")
        .and_then(|p| p.get("Designator"))
        .and_then(|d| d.as_str())
        .or_else(|| {
            comp.get("c_para")
                .and_then(|p| p.get("name"))
                .and_then(|d| d.as_str())
        })
        .unwrap_or("")
        .to_string();

    let value = comp
        .get("c_para")
        .and_then(|p| p.get("Value"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            comp.get("c_para")
                .and_then(|p| p.get("comment"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();

    let fp_name = comp
        .get("c_para")
        .and_then(|p| p.get("Footprint"))
        .and_then(|f| f.as_str())
        .or_else(|| {
            package_detail
                .and_then(|p| p.get("title"))
                .and_then(|t| t.as_str())
        })
        .unwrap_or("")
        .to_string();

    let mut pads = Vec::new();
    let mut drawings = Vec::new();
    let mut bbox = BBox::empty();
    let mut center = [0.0f64, 0.0];
    let mut layer_str = "F".to_string();

    for shape_val in shape_arr {
        let s = match shape_val.as_str() {
            Some(s) => s,
            None => continue,
        };
        let parts: Vec<&str> = s.split('~').collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "PAD" => {
                if let Some(pad) = parse_easyeda_pad(&parts, origin_x, origin_y) {
                    bbox.expand_point(
                        pad.pos[0] - pad.size[0] / 2.0,
                        pad.pos[1] - pad.size[1] / 2.0,
                    );
                    bbox.expand_point(
                        pad.pos[0] + pad.size[0] / 2.0,
                        pad.pos[1] + pad.size[1] / 2.0,
                    );
                    pads.push(pad);
                }
            }
            "TRACK" => {
                if parts.len() >= 4 {
                    let width = mil_to_mm(parts[1].parse::<f64>().unwrap_or(0.0));
                    let layer_id: u32 = parts[2].parse().unwrap_or(0);
                    let side = easyeda_layer_to_side(layer_id);
                    let coords: Vec<f64> = parts[3]
                        .split_whitespace()
                        .filter_map(|c| c.parse().ok())
                        .collect();
                    for i in (0..coords.len().saturating_sub(2)).step_by(2) {
                        let start = [
                            mil_to_mm(coords[i] - origin_x),
                            mil_to_mm(coords[i + 1] - origin_y),
                        ];
                        let end = [
                            mil_to_mm(coords[i + 2] - origin_x),
                            mil_to_mm(coords[i + 3] - origin_y),
                        ];
                        bbox.expand_point(start[0], start[1]);
                        bbox.expand_point(end[0], end[1]);
                        drawings.push(FootprintDrawing {
                            layer: side.to_string(),
                            drawing: FootprintDrawingItem::Shape(Drawing::Segment {
                                start,
                                end,
                                width,
                            }),
                        });
                    }
                }
            }
            "CIRCLE" => {
                if parts.len() >= 6 {
                    let cx = mil_to_mm(parts[1].parse::<f64>().unwrap_or(0.0) - origin_x);
                    let cy = mil_to_mm(parts[2].parse::<f64>().unwrap_or(0.0) - origin_y);
                    let radius = mil_to_mm(parts[3].parse::<f64>().unwrap_or(0.0));
                    let width = mil_to_mm(parts[4].parse::<f64>().unwrap_or(0.0));
                    let layer_id: u32 = parts[5].parse().unwrap_or(0);
                    let side = easyeda_layer_to_side(layer_id);
                    bbox.expand_point(cx - radius, cy - radius);
                    bbox.expand_point(cx + radius, cy + radius);
                    drawings.push(FootprintDrawing {
                        layer: side.to_string(),
                        drawing: FootprintDrawingItem::Shape(Drawing::Circle {
                            start: [cx, cy],
                            radius,
                            width,
                            filled: None,
                        }),
                    });
                }
            }
            _ => {}
        }
    }

    // Determine component center from bbox or first pad
    if !pads.is_empty() && bbox.minx != f64::INFINITY {
        center = [(bbox.minx + bbox.maxx) / 2.0, (bbox.miny + bbox.maxy) / 2.0];
    }

    // Determine layer from first pad
    if let Some(first_pad) = pads.first() {
        if let Some(l) = first_pad.layers.first() {
            layer_str = l.clone();
        }
    }

    if bbox.minx == f64::INFINITY {
        bbox = BBox {
            minx: center[0] - 0.5,
            miny: center[1] - 0.5,
            maxx: center[0] + 0.5,
            maxy: center[1] + 0.5,
        };
    }

    let side = if layer_str == "B" {
        Side::Back
    } else {
        Side::Front
    };

    let fp = Footprint {
        ref_: designator.clone(),
        center,
        bbox: FootprintBBox {
            pos: center,
            relpos: [bbox.minx - center[0], bbox.miny - center[1]],
            size: [bbox.maxx - bbox.minx, bbox.maxy - bbox.miny],
            angle: 0.0,
        },
        pads,
        drawings,
        layer: layer_str,
    };

    let comp = Component {
        ref_: designator,
        val: value,
        footprint_name: fp_name,
        layer: side,
        footprint_index: fp_index,
        extra_fields: std::collections::HashMap::new(),
        attr: None,
    };

    Some((fp, comp))
}

fn parse_easyeda_pad(parts: &[&str], origin_x: f64, origin_y: f64) -> Option<Pad> {
    // PAD format: PAD~shape~x~y~width~height~layer~net~number~holeRadius~...
    if parts.len() < 10 {
        return None;
    }

    let shape_type = parts[1];
    let x = mil_to_mm(parts[2].parse::<f64>().unwrap_or(0.0) - origin_x);
    let y = mil_to_mm(parts[3].parse::<f64>().unwrap_or(0.0) - origin_y);
    let width = mil_to_mm(parts[4].parse::<f64>().unwrap_or(0.0));
    let height = mil_to_mm(parts[5].parse::<f64>().unwrap_or(0.0));
    let layer_id: u32 = parts[6].parse().unwrap_or(0);
    let net_name = parts[7];
    let pad_number = parts[8];
    let hole_radius = mil_to_mm(parts[9].parse::<f64>().unwrap_or(0.0));

    let rotation: f64 = if parts.len() > 11 {
        parts[11].parse().unwrap_or(0.0)
    } else {
        0.0
    };

    let shape = match shape_type {
        "ELLIPSE" | "OVAL" => "oval",
        "RECT" => "rect",
        "ROUND" => "circle",
        "POLYGON" => "custom",
        _ => "circle",
    };

    let is_th = hole_radius > 0.0;
    let pad_type = if is_th { "th" } else { "smd" };

    let layers = if layer_id == 11 || is_th {
        vec!["F".to_string(), "B".to_string()]
    } else {
        vec![easyeda_layer_to_side(layer_id).to_string()]
    };

    let net = if net_name.is_empty() {
        None
    } else {
        Some(net_name.to_string())
    };

    let pin1 = if pad_number == "1" || pad_number == "A1" {
        Some(1u8)
    } else {
        None
    };

    let (drillshape, drillsize) = if is_th {
        let d = hole_radius * 2.0;
        (Some("circle".to_string()), Some([d, d]))
    } else {
        (None, None)
    };

    let angle = if rotation != 0.0 {
        Some(rotation)
    } else {
        None
    };

    Some(Pad {
        layers,
        pos: [x, y],
        size: [width, height],
        shape: shape.to_string(),
        pad_type: pad_type.to_string(),
        angle,
        pin1,
        net,
        offset: None,
        radius: None,
        chamfpos: None,
        chamfratio: None,
        drillshape,
        drillsize,
        svgpath: None,
        polygons: None,
    })
}

fn easyeda_layer_to_side(layer_id: u32) -> &'static str {
    match layer_id {
        1 | 3 | 5 | 12 => "F",
        2 | 4 | 6 | 13 => "B",
        11 => "F", // Multi-layer, treat as front
        _ => "F",
    }
}

fn compute_bbox(edges: &[Drawing]) -> BBox {
    let mut bbox = BBox::empty();
    for edge in edges {
        match edge {
            Drawing::Segment { start, end, .. } => {
                bbox.expand_point(start[0], start[1]);
                bbox.expand_point(end[0], end[1]);
            }
            Drawing::Circle { start, radius, .. } => {
                bbox.expand_point(start[0] - radius, start[1] - radius);
                bbox.expand_point(start[0] + radius, start[1] + radius);
            }
            _ => {}
        }
    }
    if bbox.minx == f64::INFINITY {
        BBox {
            minx: 0.0,
            miny: 0.0,
            maxx: 100.0,
            maxy: 100.0,
        }
    } else {
        bbox
    }
}
