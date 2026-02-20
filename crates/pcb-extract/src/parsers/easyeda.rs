use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use serde_json::Value;

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

    // Parse components (LIB entries)
    let _fp_index_start = footprints.len();
    if let Some(data_str) = pcb_obj.get("dataStr") {
        if let Some(libs) = data_str.get("head").and_then(|h| h.get("libs")) {
            // libs is unused, components come from shape data
            let _ = libs;
        }
    }

    // Parse footprints from component objects
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
            },
            fabrication: LayerData {
                front: fab_f,
                back: fab_b,
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
            match categorize_easyeda_layer(layer) {
                EasyEdaLayerCat::Edge => edges.push(drawing),
                _ => {}
            }
        }
        _ => {}
    }
}

fn parse_easyeda_component(
    _comp: &Value,
    _origin_x: f64,
    _origin_y: f64,
    _fp_index: usize,
) -> Option<(Footprint, Component)> {
    // EasyEDA component parsing is format-version dependent.
    // Basic stub - real implementation would parse the shape array
    // within each component to extract pads and drawings.
    None
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
