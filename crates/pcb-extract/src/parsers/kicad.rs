use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::parsers::kicad_sexpr::{self, SExpr};
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;
use std::f64::consts::PI;

/// Parse a KiCad .kicad_pcb file from bytes into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let root = kicad_sexpr::parse(data)
        .map_err(|e| ExtractError::ParseError(format!("S-expression parse error: {e}")))?;

    if root.tag() != Some("kicad_pcb") {
        return Err(ExtractError::ParseError("not a kicad_pcb file".to_string()));
    }

    let nets = parse_nets(&root);
    let layer_map = parse_layers(&root);
    let metadata = parse_metadata(&root);

    // Parse board-level graphic items
    let mut edges = Vec::new();
    let mut silk_f = Vec::new();
    let mut silk_b = Vec::new();
    let mut fab_f = Vec::new();
    let mut fab_b = Vec::new();

    for child in root.children() {
        let tag = match child.tag() {
            Some(t) => t,
            None => continue,
        };

        let graphic = match tag {
            "gr_line" => parse_gr_line(child),
            "gr_rect" => parse_gr_rect(child),
            "gr_circle" => parse_gr_circle(child),
            "gr_arc" => parse_gr_arc(child),
            "gr_curve" => parse_gr_curve(child),
            "gr_poly" => parse_gr_poly(child),
            _ => continue,
        };

        if let Some((drawing, layer_name)) = graphic {
            let cat = categorize_layer(&layer_name, &layer_map);
            match cat {
                LayerCategory::EdgeCuts => edges.push(drawing),
                LayerCategory::SilkF => silk_f.push(drawing),
                LayerCategory::SilkB => silk_b.push(drawing),
                LayerCategory::FabF => fab_f.push(drawing),
                LayerCategory::FabB => fab_b.push(drawing),
                _ => {}
            }
        }
    }

    // Parse footprints and collect component data for BOM
    let fp_nodes: Vec<&SExpr> = root
        .find_all("footprint")
        .into_iter()
        .chain(root.find_all("module").into_iter())
        .collect();

    let mut footprints = Vec::new();
    let mut components = Vec::new();

    for (idx, fp) in fp_nodes.iter().enumerate() {
        let (footprint, comp) = parse_footprint(fp, &layer_map, &nets, idx);
        footprints.push(footprint);
        components.push(comp);
    }

    // Generate BOM
    let bom = Some(generate_bom(
        &footprints,
        &components,
        &BomConfig::default(),
    ));

    // Parse tracks and zones if requested
    let (tracks, zones) = if opts.include_tracks {
        (
            Some(parse_tracks(&root, &layer_map, &nets)),
            Some(parse_zones(&root, &layer_map, &nets)),
        )
    } else {
        (None, None)
    };

    let net_names = if opts.include_nets {
        Some(nets.clone())
    } else {
        None
    };

    // Compute edges bounding box
    let edges_bbox = compute_edges_bbox(&edges);

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
        metadata,
        bom,
        ibom_version: None,
        tracks,
        zones,
        nets: net_names,
        font_data: None,
    })
}

// ─── Layer handling ──────────────────────────────────────────────────

#[derive(Debug)]
struct KicadLayerMap {
    #[allow(dead_code)]
    entries: Vec<(i64, String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
enum LayerCategory {
    CopperF,
    CopperB,
    CopperInner(String),
    SilkF,
    SilkB,
    FabF,
    FabB,
    EdgeCuts,
    Other,
}

fn parse_layers(root: &SExpr) -> KicadLayerMap {
    let mut entries = Vec::new();
    if let Some(layers_node) = root.find("layers") {
        for child in layers_node.children() {
            if let SExpr::List(items) = child {
                if items.len() >= 3 {
                    let id = items[0].as_atom().and_then(|s| s.parse::<i64>().ok());
                    let name = items[1].as_atom();
                    let user_name = if items.len() >= 4 {
                        items[3].as_atom().unwrap_or("").to_string()
                    } else {
                        String::new()
                    };
                    if let (Some(id), Some(name)) = (id, name) {
                        entries.push((id, name.to_string(), user_name));
                    }
                }
            }
        }
    }
    KicadLayerMap { entries }
}

fn categorize_layer(name: &str, _layer_map: &KicadLayerMap) -> LayerCategory {
    match name {
        "F.Cu" => LayerCategory::CopperF,
        "B.Cu" => LayerCategory::CopperB,
        "F.SilkS" | "F.Silkscreen" => LayerCategory::SilkF,
        "B.SilkS" | "B.Silkscreen" => LayerCategory::SilkB,
        "F.Fab" | "F.Fabrication" => LayerCategory::FabF,
        "B.Fab" | "B.Fabrication" => LayerCategory::FabB,
        "Edge.Cuts" => LayerCategory::EdgeCuts,
        n if n.ends_with(".Cu") => LayerCategory::CopperInner(n.to_string()),
        _ => LayerCategory::Other,
    }
}

fn layer_to_side(name: &str) -> Option<&'static str> {
    match name {
        n if n.starts_with("F.") || n == "F.Cu" => Some("F"),
        n if n.starts_with("B.") || n == "B.Cu" => Some("B"),
        _ => None,
    }
}

fn layer_is_copper(name: &str) -> bool {
    name.ends_with(".Cu")
}

// ─── Nets ────────────────────────────────────────────────────────────

fn parse_nets(root: &SExpr) -> Vec<String> {
    let mut nets = Vec::new();
    for child in root.find_all("net") {
        let id = child.f64_at(0).unwrap_or(0.0) as usize;
        let name = child.atom_at(1).unwrap_or("").to_string();
        while nets.len() <= id {
            nets.push(String::new());
        }
        nets[id] = name;
    }
    nets
}

// ─── Metadata ────────────────────────────────────────────────────────

fn parse_metadata(root: &SExpr) -> Metadata {
    let mut title = String::new();
    let mut revision = String::new();
    let mut company = String::new();
    let mut date = String::new();

    if let Some(setup) = root.find("title_block") {
        title = setup.value("title").unwrap_or("").to_string();
        revision = setup.value("rev").unwrap_or("").to_string();
        company = setup.value("company").unwrap_or("").to_string();
        date = setup.value("date").unwrap_or("").to_string();
    }

    Metadata {
        title,
        revision,
        company,
        date,
    }
}

// ─── Graphic items ───────────────────────────────────────────────────

fn get_layer_name(node: &SExpr) -> String {
    node.value("layer")
        .or_else(|| {
            // KiCad 8+ uses (layer "X") as a direct child atom
            node.find("layer").and_then(|l| l.atom_at(0))
        })
        .unwrap_or("")
        .to_string()
}

fn parse_xy(node: &SExpr, tag: &str) -> Option<[f64; 2]> {
    node.find(tag)
        .map(|n| [n.f64_at(0).unwrap_or(0.0), n.f64_at(1).unwrap_or(0.0)])
}

fn parse_width(node: &SExpr) -> f64 {
    node.value_f64("width")
        .or_else(|| {
            // KiCad 7+ uses (stroke (width N))
            node.find("stroke").and_then(|s| s.value_f64("width"))
        })
        .unwrap_or(0.0)
}

fn parse_gr_line(node: &SExpr) -> Option<(Drawing, String)> {
    let start = parse_xy(node, "start")?;
    let end = parse_xy(node, "end")?;
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((Drawing::Segment { start, end, width }, layer))
}

fn parse_gr_rect(node: &SExpr) -> Option<(Drawing, String)> {
    let start = parse_xy(node, "start")?;
    let end = parse_xy(node, "end")?;
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((Drawing::Rect { start, end, width }, layer))
}

fn parse_gr_circle(node: &SExpr) -> Option<(Drawing, String)> {
    let center = parse_xy(node, "center").or_else(|| parse_xy(node, "start"))?;
    let end = parse_xy(node, "end")?;
    let dx = end[0] - center[0];
    let dy = end[1] - center[1];
    let radius = (dx * dx + dy * dy).sqrt();
    let width = parse_width(node);
    let fill_node = node.find("fill");
    let filled = fill_node
        .and_then(|f| f.value("type"))
        .map(|t| if t == "solid" { 1u8 } else { 0u8 });
    let layer = get_layer_name(node);
    Some((
        Drawing::Circle {
            start: center,
            radius,
            width,
            filled,
        },
        layer,
    ))
}

fn parse_gr_arc(node: &SExpr) -> Option<(Drawing, String)> {
    // KiCad 7+ uses (start, mid, end) for arcs
    // KiCad 5-6 uses (start=center, end=startpoint, angle)
    let width = parse_width(node);
    let layer = get_layer_name(node);

    if let Some(mid) = parse_xy(node, "mid") {
        // KiCad 7+ three-point arc
        let start = parse_xy(node, "start")?;
        let end = parse_xy(node, "end")?;
        let (center, radius, start_angle, end_angle) = arc_from_three_points(start, mid, end)?;
        Some((
            Drawing::Arc {
                start: center,
                radius,
                startangle: start_angle,
                endangle: end_angle,
                width,
            },
            layer,
        ))
    } else {
        // Legacy: center + end + angle
        let center = parse_xy(node, "start")?;
        let endpoint = parse_xy(node, "end")?;
        let angle = node.value_f64("angle").unwrap_or(0.0);
        let dx = endpoint[0] - center[0];
        let dy = endpoint[1] - center[1];
        let radius = (dx * dx + dy * dy).sqrt();
        let start_angle = dy.atan2(dx) * 180.0 / PI;
        let end_angle = start_angle + angle;
        Some((
            Drawing::Arc {
                start: center,
                radius,
                startangle: start_angle,
                endangle: end_angle,
                width,
            },
            layer,
        ))
    }
}

fn parse_gr_curve(node: &SExpr) -> Option<(Drawing, String)> {
    let pts = node.find("pts")?;
    let points: Vec<[f64; 2]> = pts
        .find_all("xy")
        .iter()
        .map(|xy| [xy.f64_at(0).unwrap_or(0.0), xy.f64_at(1).unwrap_or(0.0)])
        .collect();
    if points.len() < 4 {
        return None;
    }
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((
        Drawing::Curve {
            start: points[0],
            cpa: points[1],
            cpb: points[2],
            end: points[3],
            width,
        },
        layer,
    ))
}

fn parse_gr_poly(node: &SExpr) -> Option<(Drawing, String)> {
    let pts = node.find("pts")?;
    let points: Vec<[f64; 2]> = pts
        .find_all("xy")
        .iter()
        .map(|xy| [xy.f64_at(0).unwrap_or(0.0), xy.f64_at(1).unwrap_or(0.0)])
        .collect();
    if points.is_empty() {
        return None;
    }
    let width = parse_width(node);
    let layer = get_layer_name(node);
    let fill_node = node.find("fill");
    let filled = fill_node
        .and_then(|f| f.value("type"))
        .map(|t| if t == "solid" { 1u8 } else { 0u8 })
        .or(Some(1)); // polygons default to filled
    Some((
        Drawing::Polygon {
            pos: [0.0, 0.0],
            angle: 0.0,
            polygons: vec![points],
            filled,
            width,
        },
        layer,
    ))
}

// ─── Footprint parsing ──────────────────────────────────────────────

fn parse_footprint(
    node: &SExpr,
    _layer_map: &KicadLayerMap,
    nets: &[String],
    footprint_index: usize,
) -> (Footprint, Component) {
    // Footprint position
    let at_node = node.find("at");
    let fp_x = at_node.and_then(|n| n.f64_at(0)).unwrap_or(0.0);
    let fp_y = at_node.and_then(|n| n.f64_at(1)).unwrap_or(0.0);
    let fp_angle = at_node.and_then(|n| n.f64_at(2)).unwrap_or(0.0);

    // Layer
    let fp_layer = get_layer_name(node);
    let side = if fp_layer.starts_with("B.") { "B" } else { "F" };

    // Footprint library name (e.g. "Resistor_SMD:R_0402_1005Metric")
    let fp_lib_name = node.atom_at(0).unwrap_or("").to_string();

    // Reference, value, and extra fields
    let mut ref_ = String::new();
    let mut value = String::new();
    let mut extra_fields: HashMap<String, String> = HashMap::new();
    let mut attr = None;

    for child in node.children() {
        match child.tag() {
            Some("fp_text") => {
                let text_type = child.atom_at(0).unwrap_or("");
                let text_val = child.atom_at(1).unwrap_or("");
                match text_type {
                    "reference" => ref_ = text_val.to_string(),
                    "value" => value = text_val.to_string(),
                    "user" => {
                        let field_name = text_val.to_string();
                        if !field_name.is_empty() {
                            extra_fields.insert(format!("user_{}", extra_fields.len()), field_name);
                        }
                    }
                    _ => {}
                }
            }
            Some("property") => {
                let prop_name = child.atom_at(0).unwrap_or("");
                let prop_val = child.atom_at(1).unwrap_or("");
                match prop_name {
                    "Reference" => ref_ = prop_val.to_string(),
                    "Value" => value = prop_val.to_string(),
                    "Footprint" | "ki_fp_filters" | "ki_description" => {}
                    _ => {
                        extra_fields.insert(prop_name.to_string(), prop_val.to_string());
                    }
                }
            }
            Some("attr") => {
                // Component attributes (e.g. "smd", "through_hole", "virtual", "board_only")
                attr = child.atom_at(0).map(|s| s.to_string());
            }
            _ => {}
        }
    }

    // Parse pads
    let pads: Vec<Pad> = node
        .find_all("pad")
        .into_iter()
        .map(|p| parse_pad(p, fp_x, fp_y, fp_angle, nets))
        .collect();

    // Parse footprint drawings
    let mut drawings = Vec::new();
    for child in node.children() {
        let tag = match child.tag() {
            Some(t) => t,
            None => continue,
        };

        // Handle shape drawings
        let graphic = match tag {
            "fp_line" => parse_fp_line(child, fp_x, fp_y, fp_angle),
            "fp_rect" => parse_fp_rect(child, fp_x, fp_y, fp_angle),
            "fp_circle" => parse_fp_circle(child, fp_x, fp_y, fp_angle),
            "fp_arc" => parse_fp_arc(child, fp_x, fp_y, fp_angle),
            "fp_poly" => parse_fp_poly(child, fp_x, fp_y, fp_angle),
            _ => None,
        };
        if let Some((drawing, layer_name)) = graphic {
            if let Some(s) = layer_to_side(&layer_name) {
                if layer_is_copper(&layer_name)
                    || layer_name.contains("Silk")
                    || layer_name.contains("Fab")
                {
                    drawings.push(FootprintDrawing {
                        layer: s.to_string(),
                        drawing: FootprintDrawingItem::Shape(drawing),
                    });
                }
            }
        }

        // Handle text drawings
        if tag == "fp_text" || tag == "property" {
            if let Some((text_drawing, layer_name)) =
                parse_fp_text(child, tag, fp_x, fp_y, fp_angle)
            {
                if let Some(s) = layer_to_side(&layer_name) {
                    if layer_name.contains("Silk") || layer_name.contains("Fab") {
                        drawings.push(FootprintDrawing {
                            layer: s.to_string(),
                            drawing: FootprintDrawingItem::Text(text_drawing),
                        });
                    }
                }
            }
        }
    }

    // Compute bounding box from pads
    let mut bbox = BBox::empty();
    for pad in &pads {
        bbox.expand_point(
            pad.pos[0] - pad.size[0] / 2.0,
            pad.pos[1] - pad.size[1] / 2.0,
        );
        bbox.expand_point(
            pad.pos[0] + pad.size[0] / 2.0,
            pad.pos[1] + pad.size[1] / 2.0,
        );
    }
    if bbox.minx == f64::INFINITY {
        bbox = BBox {
            minx: fp_x - 0.5,
            miny: fp_y - 0.5,
            maxx: fp_x + 0.5,
            maxy: fp_y + 0.5,
        };
    }

    let fp_bbox = FootprintBBox {
        pos: [fp_x, fp_y],
        relpos: [bbox.minx - fp_x, bbox.miny - fp_y],
        size: [bbox.maxx - bbox.minx, bbox.maxy - bbox.miny],
        angle: fp_angle,
    };

    let comp_side = if side == "B" { Side::Back } else { Side::Front };

    let footprint = Footprint {
        ref_: ref_.clone(),
        center: [fp_x, fp_y],
        bbox: fp_bbox,
        pads,
        drawings,
        layer: side.to_string(),
    };

    let component = Component {
        ref_: ref_,
        val: value,
        footprint_name: fp_lib_name,
        layer: comp_side,
        footprint_index,
        extra_fields,
        attr,
    };

    (footprint, component)
}

// ─── Pad parsing ─────────────────────────────────────────────────────

fn parse_pad(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64, nets: &[String]) -> Pad {
    let pad_name = node.atom_at(0).unwrap_or("").to_string();
    let pad_type_str = node.atom_at(1).unwrap_or("smd");
    let shape_str = node.atom_at(2).unwrap_or("rect");

    let at_node = node.find("at");
    let local_x = at_node.and_then(|n| n.f64_at(0)).unwrap_or(0.0);
    let local_y = at_node.and_then(|n| n.f64_at(1)).unwrap_or(0.0);
    let pad_angle = at_node.and_then(|n| n.f64_at(2)).unwrap_or(0.0);

    // Transform local to absolute coordinates
    let (abs_x, abs_y) = rotate_and_translate(local_x, local_y, fp_x, fp_y, fp_angle);

    let size_node = node.find("size");
    let size_w = size_node.and_then(|n| n.f64_at(0)).unwrap_or(0.0);
    let size_h = size_node.and_then(|n| n.f64_at(1)).unwrap_or(0.0);

    let pad_type = match pad_type_str {
        "thru_hole" => "th",
        "np_thru_hole" => "th",
        _ => "smd",
    };

    let shape = match shape_str {
        "roundrect" => "roundrect",
        "circle" => "circle",
        "oval" => "oval",
        "rect" => "rect",
        "custom" => "custom",
        "chamfrect" | "chamfered_rect" => "chamfrect",
        _ => "rect",
    };

    // Layers
    let layers: Vec<String> = node
        .find("layers")
        .map(|l| {
            l.children()
                .iter()
                .filter_map(|c| c.as_atom())
                .filter_map(|name| {
                    if name.contains("Cu") {
                        layer_to_side(name).map(|s| s.to_string())
                    } else if name == "*.Cu" {
                        None // handled below
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let layers = if layers.is_empty() {
        // Check for *.Cu (all copper = through-hole)
        let has_all_cu = node
            .find("layers")
            .map(|l| l.children().iter().any(|c| c.as_atom() == Some("*.Cu")))
            .unwrap_or(false);
        if has_all_cu {
            vec!["F".to_string(), "B".to_string()]
        } else {
            vec!["F".to_string()]
        }
    } else {
        layers
    };

    // Net
    let net_id = node
        .find("net")
        .and_then(|n| n.f64_at(0))
        .map(|v| v as usize);
    let net = net_id
        .and_then(|id| nets.get(id).cloned())
        .filter(|n| !n.is_empty());

    // Pin 1
    let pin1 = if pad_name == "1" || pad_name == "A1" {
        Some(1u8)
    } else {
        None
    };

    // Drill
    let (drillshape, drillsize) = if pad_type == "th" {
        let drill_node = node.find("drill");
        if let Some(drill) = drill_node {
            let is_oval = drill.atom_at(0) == Some("oval");
            if is_oval {
                let dw = drill.f64_at(1).unwrap_or(0.0);
                let dh = drill.f64_at(2).unwrap_or(dw);
                (Some("oblong".to_string()), Some([dw, dh]))
            } else {
                let d = drill.f64_at(0).unwrap_or(0.0);
                (Some("circle".to_string()), Some([d, d]))
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // Roundrect ratio
    let radius = if shape == "roundrect" || shape == "chamfrect" {
        node.value_f64("roundrect_rratio")
            .map(|r| r * size_w.min(size_h) / 2.0)
    } else {
        None
    };

    // Chamfer
    let (chamfpos, chamfratio) = if shape == "chamfrect" {
        let ratio = node.value_f64("chamfer_ratio").unwrap_or(0.0);
        let mut pos = 0u8;
        if let Some(chamfer) = node.find("chamfer") {
            for child in chamfer.children() {
                match child.as_atom() {
                    Some("top_left") => pos |= 1,
                    Some("top_right") => pos |= 2,
                    Some("bottom_right") => pos |= 4,
                    Some("bottom_left") => pos |= 8,
                    _ => {}
                }
            }
        }
        (Some(pos), Some(ratio))
    } else {
        (None, None)
    };

    // Pad offset
    let offset = node
        .find("offset")
        .map(|o| [o.f64_at(0).unwrap_or(0.0), o.f64_at(1).unwrap_or(0.0)]);

    Pad {
        layers,
        pos: [abs_x, abs_y],
        size: [size_w, size_h],
        shape: shape.to_string(),
        pad_type: pad_type.to_string(),
        angle: if pad_angle != 0.0 {
            Some(pad_angle + fp_angle)
        } else if fp_angle != 0.0 {
            Some(fp_angle)
        } else {
            None
        },
        pin1,
        net,
        offset,
        radius,
        chamfpos,
        chamfratio,
        drillshape,
        drillsize,
        svgpath: None,
        polygons: None,
    }
}

// ─── Footprint drawing items ─────────────────────────────────────────

fn parse_fp_line(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64) -> Option<(Drawing, String)> {
    let start_local = parse_xy(node, "start")?;
    let end_local = parse_xy(node, "end")?;
    let (sx, sy) = rotate_and_translate(start_local[0], start_local[1], fp_x, fp_y, fp_angle);
    let (ex, ey) = rotate_and_translate(end_local[0], end_local[1], fp_x, fp_y, fp_angle);
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((
        Drawing::Segment {
            start: [sx, sy],
            end: [ex, ey],
            width,
        },
        layer,
    ))
}

fn parse_fp_rect(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64) -> Option<(Drawing, String)> {
    let start_local = parse_xy(node, "start")?;
    let end_local = parse_xy(node, "end")?;
    let (sx, sy) = rotate_and_translate(start_local[0], start_local[1], fp_x, fp_y, fp_angle);
    let (ex, ey) = rotate_and_translate(end_local[0], end_local[1], fp_x, fp_y, fp_angle);
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((
        Drawing::Rect {
            start: [sx, sy],
            end: [ex, ey],
            width,
        },
        layer,
    ))
}

fn parse_fp_circle(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64) -> Option<(Drawing, String)> {
    let center_local = parse_xy(node, "center").or_else(|| parse_xy(node, "start"))?;
    let end_local = parse_xy(node, "end")?;
    let (cx, cy) = rotate_and_translate(center_local[0], center_local[1], fp_x, fp_y, fp_angle);
    let (ex, ey) = rotate_and_translate(end_local[0], end_local[1], fp_x, fp_y, fp_angle);
    let dx = ex - cx;
    let dy = ey - cy;
    let radius = (dx * dx + dy * dy).sqrt();
    let width = parse_width(node);
    let layer = get_layer_name(node);
    let fill_node = node.find("fill");
    let filled = fill_node
        .and_then(|f| f.value("type"))
        .map(|t| if t == "solid" { 1u8 } else { 0u8 });
    Some((
        Drawing::Circle {
            start: [cx, cy],
            radius,
            width,
            filled,
        },
        layer,
    ))
}

fn parse_fp_arc(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64) -> Option<(Drawing, String)> {
    let width = parse_width(node);
    let layer = get_layer_name(node);

    if let Some(mid_local) = parse_xy(node, "mid") {
        let start_local = parse_xy(node, "start")?;
        let end_local = parse_xy(node, "end")?;
        let (sx, sy) = rotate_and_translate(start_local[0], start_local[1], fp_x, fp_y, fp_angle);
        let (mx, my) = rotate_and_translate(mid_local[0], mid_local[1], fp_x, fp_y, fp_angle);
        let (ex, ey) = rotate_and_translate(end_local[0], end_local[1], fp_x, fp_y, fp_angle);
        let (center, radius, start_angle, end_angle) =
            arc_from_three_points([sx, sy], [mx, my], [ex, ey])?;
        Some((
            Drawing::Arc {
                start: center,
                radius,
                startangle: start_angle,
                endangle: end_angle,
                width,
            },
            layer,
        ))
    } else {
        let center_local = parse_xy(node, "start")?;
        let endpoint_local = parse_xy(node, "end")?;
        let angle = node.value_f64("angle").unwrap_or(0.0);
        let (cx, cy) = rotate_and_translate(center_local[0], center_local[1], fp_x, fp_y, fp_angle);
        let (ex, ey) =
            rotate_and_translate(endpoint_local[0], endpoint_local[1], fp_x, fp_y, fp_angle);
        let dx = ex - cx;
        let dy = ey - cy;
        let radius = (dx * dx + dy * dy).sqrt();
        let start_angle = dy.atan2(dx) * 180.0 / PI;
        let end_angle = start_angle + angle;
        Some((
            Drawing::Arc {
                start: [cx, cy],
                radius,
                startangle: start_angle,
                endangle: end_angle,
                width,
            },
            layer,
        ))
    }
}

fn parse_fp_poly(node: &SExpr, fp_x: f64, fp_y: f64, fp_angle: f64) -> Option<(Drawing, String)> {
    let pts = node.find("pts")?;
    let points: Vec<[f64; 2]> = pts
        .find_all("xy")
        .iter()
        .map(|xy| {
            let lx = xy.f64_at(0).unwrap_or(0.0);
            let ly = xy.f64_at(1).unwrap_or(0.0);
            let (ax, ay) = rotate_and_translate(lx, ly, fp_x, fp_y, fp_angle);
            [ax, ay]
        })
        .collect();
    if points.is_empty() {
        return None;
    }
    let width = parse_width(node);
    let layer = get_layer_name(node);
    Some((
        Drawing::Polygon {
            pos: [0.0, 0.0],
            angle: 0.0,
            polygons: vec![points],
            filled: Some(1),
            width,
        },
        layer,
    ))
}

// ─── Text extraction ─────────────────────────────────────────────────

fn parse_fp_text(
    node: &SExpr,
    tag: &str,
    fp_x: f64,
    fp_y: f64,
    fp_angle: f64,
) -> Option<(TextDrawing, String)> {
    let (text_type, text_val, layer) = if tag == "fp_text" {
        let text_type = node.atom_at(0)?;
        let text_val = node.atom_at(1).unwrap_or("");
        let layer = get_layer_name(node);
        (text_type, text_val, layer)
    } else {
        // KiCad 8+ property node
        let prop_name = node.atom_at(0)?;
        let text_val = node.atom_at(1).unwrap_or("");
        let layer = get_layer_name(node);
        // Map property name to text type
        let text_type = match prop_name {
            "Reference" => "reference",
            "Value" => "value",
            _ => "user",
        };
        (text_type, text_val, layer)
    };

    // Skip hidden text
    if node.find("hide").is_some() {
        return None;
    }

    // Get text position
    let at_node = node.find("at");
    let text_local_x = at_node.and_then(|n| n.f64_at(0)).unwrap_or(0.0);
    let text_local_y = at_node.and_then(|n| n.f64_at(1)).unwrap_or(0.0);
    let text_angle = at_node.and_then(|n| n.f64_at(2)).unwrap_or(0.0);

    let (abs_x, abs_y) = rotate_and_translate(text_local_x, text_local_y, fp_x, fp_y, fp_angle);

    // Get text effects (font size, thickness, justification)
    let effects = node.find("effects");
    let font = effects.and_then(|e| e.find("font"));
    let size_node = font.and_then(|f| f.find("size"));
    let height = size_node.and_then(|s| s.f64_at(0)).unwrap_or(1.0);
    let width = size_node.and_then(|s| s.f64_at(1)).unwrap_or(1.0);
    let thickness = font.and_then(|f| f.value_f64("thickness")).unwrap_or(0.15);

    // Justification
    let justify = effects.and_then(|e| e.find("justify"));
    let mut jx: i8 = 0; // 0=center, -1=left, 1=right
    let mut jy: i8 = 0; // 0=center, -1=top, 1=bottom
    if let Some(j) = justify {
        for child in j.children() {
            match child.as_atom() {
                Some("left") => jx = -1,
                Some("right") => jx = 1,
                Some("top") => jy = -1,
                Some("bottom") => jy = 1,
                Some("mirror") => {}
                _ => {}
            }
        }
    }

    let is_ref = if text_type == "reference" {
        Some(1u8)
    } else {
        None
    };
    let is_val = if text_type == "value" {
        Some(1u8)
    } else {
        None
    };

    let mut attrs = Vec::new();
    if font.and_then(|f| f.find("italic")).is_some() {
        attrs.push("italic".to_string());
    }

    Some((
        TextDrawing {
            svgpath: None,
            thickness: Some(thickness),
            is_ref,
            val: is_val,
            pos: Some([abs_x, abs_y]),
            text: Some(text_val.to_string()),
            height: Some(height),
            width: Some(width),
            justify: Some([jx, jy]),
            angle: Some(text_angle + fp_angle),
            attr: if attrs.is_empty() { None } else { Some(attrs) },
        },
        layer,
    ))
}

// ─── Tracks ──────────────────────────────────────────────────────────

fn parse_tracks(root: &SExpr, layer_map: &KicadLayerMap, nets: &[String]) -> LayerData<Vec<Track>> {
    let mut front = Vec::new();
    let mut back = Vec::new();
    let mut inner: HashMap<String, Vec<Track>> = HashMap::new();

    for child in root.children() {
        match child.tag() {
            Some("segment") => {
                let start = match parse_xy(child, "start") {
                    Some(v) => v,
                    None => continue,
                };
                let end = match parse_xy(child, "end") {
                    Some(v) => v,
                    None => continue,
                };
                let width = child.value_f64("width").unwrap_or(0.25);
                let layer = get_layer_name(child);
                let net_id = child.value_f64("net").map(|v| v as usize);
                let net = net_id
                    .and_then(|id| nets.get(id).cloned())
                    .filter(|n| !n.is_empty());

                let track = Track::Segment {
                    start,
                    end,
                    width,
                    net,
                    drillsize: None,
                };
                match categorize_layer(&layer, layer_map) {
                    LayerCategory::CopperF => front.push(track),
                    LayerCategory::CopperB => back.push(track),
                    LayerCategory::CopperInner(name) => inner.entry(name).or_default().push(track),
                    _ => {}
                }
            }
            Some("via") => {
                let at = match parse_xy(child, "at") {
                    Some(v) => v,
                    None => continue,
                };
                let size = child.value_f64("size").unwrap_or(0.6);
                let drill = child.value_f64("drill").unwrap_or(0.3);
                let net_id = child.value_f64("net").map(|v| v as usize);
                let net = net_id
                    .and_then(|id| nets.get(id).cloned())
                    .filter(|n| !n.is_empty());
                let via = Track::Segment {
                    start: at,
                    end: at,
                    width: size,
                    net: net.clone(),
                    drillsize: Some(drill),
                };
                // Vias appear on all copper layers
                front.push(via.clone());
                back.push(via.clone());
                for layer_tracks in inner.values_mut() {
                    layer_tracks.push(via.clone());
                }
                // Also ensure vias appear on inner layers defined in the board
                for (_, name, _) in &layer_map.entries {
                    if name.ends_with(".Cu")
                        && name != "F.Cu"
                        && name != "B.Cu"
                        && !inner.contains_key(name)
                    {
                        inner.entry(name.clone()).or_default().push(via.clone());
                    }
                }
            }
            Some("arc") => {
                // Top-level arc tracks (KiCad 7+)
                let start = match parse_xy(child, "start") {
                    Some(v) => v,
                    None => continue,
                };
                let mid = match parse_xy(child, "mid") {
                    Some(v) => v,
                    None => continue,
                };
                let end = match parse_xy(child, "end") {
                    Some(v) => v,
                    None => continue,
                };
                let width = child.value_f64("width").unwrap_or(0.25);
                let layer = get_layer_name(child);
                let net_id = child.value_f64("net").map(|v| v as usize);
                let net = net_id
                    .and_then(|id| nets.get(id).cloned())
                    .filter(|n| !n.is_empty());

                if let Some((center, radius, start_angle, end_angle)) =
                    arc_from_three_points(start, mid, end)
                {
                    let track = Track::Arc {
                        center,
                        startangle: start_angle,
                        endangle: end_angle,
                        radius,
                        width,
                        net,
                    };
                    match categorize_layer(&layer, layer_map) {
                        LayerCategory::CopperF => front.push(track),
                        LayerCategory::CopperB => back.push(track),
                        LayerCategory::CopperInner(name) => {
                            inner.entry(name).or_default().push(track)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    LayerData { front, back, inner }
}

// ─── Zones ───────────────────────────────────────────────────────────

fn parse_zones(root: &SExpr, layer_map: &KicadLayerMap, _nets: &[String]) -> LayerData<Vec<Zone>> {
    let mut front = Vec::new();
    let mut back = Vec::new();
    let mut inner: HashMap<String, Vec<Zone>> = HashMap::new();

    for zone in root.find_all("zone") {
        let net_name = zone.value("net_name").unwrap_or("").to_string();
        let layer = get_layer_name(zone);
        let cat = categorize_layer(&layer, layer_map);

        // Get filled polygons
        for fp in zone.find_all("filled_polygon") {
            let fp_layer = get_layer_name(fp);
            let fp_cat = if !fp_layer.is_empty() {
                categorize_layer(&fp_layer, layer_map)
            } else {
                cat.clone()
            };

            if let Some(pts) = fp.find("pts") {
                let points: Vec<[f64; 2]> = pts
                    .find_all("xy")
                    .iter()
                    .map(|xy| [xy.f64_at(0).unwrap_or(0.0), xy.f64_at(1).unwrap_or(0.0)])
                    .collect();

                if !points.is_empty() {
                    let z = Zone {
                        polygons: Some(vec![points]),
                        svgpath: None,
                        width: Some(0.0),
                        net: if net_name.is_empty() {
                            None
                        } else {
                            Some(net_name.clone())
                        },
                        fillrule: None,
                    };
                    match fp_cat {
                        LayerCategory::CopperF => front.push(z),
                        LayerCategory::CopperB => back.push(z),
                        LayerCategory::CopperInner(name) => inner.entry(name).or_default().push(z),
                        _ => {}
                    }
                }
            }
        }
    }

    LayerData { front, back, inner }
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Rotate point (lx, ly) by angle degrees and translate to (tx, ty).
fn rotate_and_translate(lx: f64, ly: f64, tx: f64, ty: f64, angle_deg: f64) -> (f64, f64) {
    if angle_deg == 0.0 {
        return (lx + tx, ly + ty);
    }
    let angle_rad = -angle_deg * PI / 180.0;
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();
    let rx = lx * cos_a - ly * sin_a;
    let ry = lx * sin_a + ly * cos_a;
    (rx + tx, ry + ty)
}

/// Compute arc center, radius, start angle, end angle from three points.
fn arc_from_three_points(
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
) -> Option<([f64; 2], f64, f64, f64)> {
    // Find circumcenter of three points
    let ax = p1[0];
    let ay = p1[1];
    let bx = p2[0];
    let by = p2[1];
    let cx = p3[0];
    let cy = p3[1];

    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-10 {
        return None;
    }

    let ux = ((ax * ax + ay * ay) * (by - cy)
        + (bx * bx + by * by) * (cy - ay)
        + (cx * cx + cy * cy) * (ay - by))
        / d;
    let uy = ((ax * ax + ay * ay) * (cx - bx)
        + (bx * bx + by * by) * (ax - cx)
        + (cx * cx + cy * cy) * (bx - ax))
        / d;

    let radius = ((ax - ux).powi(2) + (ay - uy).powi(2)).sqrt();
    let start_angle = (ay - uy).atan2(ax - ux) * 180.0 / PI;
    let end_angle = (cy - uy).atan2(cx - ux) * 180.0 / PI;

    Some(([ux, uy], radius, start_angle, end_angle))
}

fn compute_edges_bbox(edges: &[Drawing]) -> BBox {
    let mut bbox = BBox::empty();
    for edge in edges {
        match edge {
            Drawing::Segment { start, end, .. } => {
                bbox.expand_point(start[0], start[1]);
                bbox.expand_point(end[0], end[1]);
            }
            Drawing::Rect { start, end, .. } => {
                bbox.expand_point(start[0], start[1]);
                bbox.expand_point(end[0], end[1]);
            }
            Drawing::Circle { start, radius, .. } => {
                bbox.expand_point(start[0] - radius, start[1] - radius);
                bbox.expand_point(start[0] + radius, start[1] + radius);
            }
            Drawing::Arc { start, radius, .. } => {
                bbox.expand_point(start[0] - radius, start[1] - radius);
                bbox.expand_point(start[0] + radius, start[1] + radius);
            }
            Drawing::Curve {
                start,
                end,
                cpa,
                cpb,
                ..
            } => {
                bbox.expand_point(start[0], start[1]);
                bbox.expand_point(end[0], end[1]);
                bbox.expand_point(cpa[0], cpa[1]);
                bbox.expand_point(cpb[0], cpb[1]);
            }
            Drawing::Polygon { polygons, .. } => {
                for poly in polygons {
                    for pt in poly {
                        bbox.expand_point(pt[0], pt[1]);
                    }
                }
            }
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
