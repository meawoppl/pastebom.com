mod layers;
mod records;

use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// Parse an Altium .PcbDoc file from bytes into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let cursor = std::io::Cursor::new(data);
    let mut cfb = cfb::CompoundFile::open(cursor)
        .map_err(|e| ExtractError::ParseError(format!("Not a valid OLE2/CFB file: {e}")))?;

    // 1. Parse string table
    let wide_strings = parse_wide_strings(&mut cfb)?;

    // 2. Parse board config
    let board_records = read_text_records(&mut cfb, "/Board6/Data")?;
    let layer_map = layers::build_layer_map(&board_records);

    // 3. Parse components
    let comp_records = read_text_records(&mut cfb, "/Components6/Data")?;
    let components = records::parse_components(&comp_records, &wide_strings);

    // 4. Parse nets
    let net_records = read_text_records(&mut cfb, "/Nets6/Data")?;
    let nets = records::parse_nets(&net_records);

    // 5. Parse geometry objects
    let pads = read_binary_stream(&mut cfb, "/Pads6/Data")
        .map(|data| records::parse_pads(&data))
        .unwrap_or_default();

    let tracks = read_binary_stream(&mut cfb, "/Tracks6/Data")
        .map(|data| records::parse_tracks(&data))
        .unwrap_or_default();

    let arcs = read_binary_stream(&mut cfb, "/Arcs6/Data")
        .map(|data| records::parse_arcs(&data))
        .unwrap_or_default();

    let vias = read_binary_stream(&mut cfb, "/Vias6/Data")
        .map(|data| records::parse_vias(&data))
        .unwrap_or_default();

    let fills = read_binary_stream(&mut cfb, "/Fills6/Data")
        .map(|data| records::parse_fills(&data))
        .unwrap_or_default();

    let texts = read_binary_stream(&mut cfb, "/Texts6/Data")
        .map(|data| records::parse_texts(&data))
        .unwrap_or_default();

    // 6. Build footprints from components + child objects
    let footprints = build_footprints(
        &components,
        &pads,
        &tracks,
        &arcs,
        &fills,
        &texts,
        &nets,
        &layer_map,
    );

    // 6b. Build Component structs for BOM generation
    let bom_components: Vec<Component> = components
        .iter()
        .enumerate()
        .map(|(idx, comp)| {
            let side = if layer_map.side(comp.layer) == "B" {
                Side::Back
            } else {
                Side::Front
            };
            Component {
                ref_: comp.designator.clone(),
                val: comp.comment.clone(),
                footprint_name: comp.pattern.clone(),
                layer: side,
                footprint_index: idx,
                extra_fields: HashMap::new(),
                attr: None,
            }
        })
        .collect();

    let bom = Some(generate_bom(
        &footprints,
        &bom_components,
        &BomConfig::default(),
    ));

    // 7. Board edges
    let edges = extract_board_edges(&board_records);
    let edges_bbox = compute_edges_bbox(&edges);

    // 8. Categorize board-level drawings (silkscreen, fabrication)
    let drawings = categorize_drawings(&tracks, &arcs, &fills, &layer_map);

    // 9. Tracks and zones
    let (track_data, zone_data) = if opts.include_tracks {
        (
            Some(build_track_data(&tracks, &arcs, &vias, &nets, &layer_map)),
            Some(LayerData {
                front: Vec::new(),
                back: Vec::new(),
                inner: HashMap::new(),
            }),
        )
    } else {
        (None, None)
    };

    let net_names = if opts.include_nets {
        Some(nets.iter().map(|n| n.name.clone()).collect())
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox,
        edges,
        drawings,
        footprints,
        metadata: extract_metadata(&board_records),
        bom,
        ibom_version: None,
        tracks: track_data,
        zones: zone_data,
        nets: net_names,
        font_data: None,
    })
}

// ─── CFB stream reading ──────────────────────────────────────────────

fn read_stream<R: Read + Seek>(cfb: &mut cfb::CompoundFile<R>, path: &str) -> Option<Vec<u8>> {
    let mut stream = cfb.open_stream(path).ok()?;
    let mut data = Vec::new();
    stream.read_to_end(&mut data).ok()?;
    Some(data)
}

fn read_text_records<R: Read + Seek>(
    cfb: &mut cfb::CompoundFile<R>,
    path: &str,
) -> Result<Vec<HashMap<String, String>>, ExtractError> {
    let data = read_stream(cfb, path).unwrap_or_default();
    Ok(parse_text_record_stream(&data))
}

fn read_binary_stream<R: Read + Seek>(
    cfb: &mut cfb::CompoundFile<R>,
    path: &str,
) -> Option<Vec<u8>> {
    read_stream(cfb, path)
}

fn parse_wide_strings<R: Read + Seek>(
    cfb: &mut cfb::CompoundFile<R>,
) -> Result<HashMap<u32, String>, ExtractError> {
    let data = match read_stream(cfb, "/WideStrings6/Data") {
        Some(d) => d,
        None => return Ok(HashMap::new()),
    };
    let mut strings = HashMap::new();
    if data.len() < 4 {
        return Ok(strings);
    }
    let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut offset = 4;
    for _ in 0..count {
        if offset + 8 > data.len() {
            break;
        }
        let id = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;
        let len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        let byte_len = len * 2;
        if offset + byte_len > data.len() {
            break;
        }
        let utf16: Vec<u16> = data[offset..offset + byte_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        strings.insert(id, String::from_utf16_lossy(&utf16));
        offset += byte_len;
    }
    Ok(strings)
}

// ─── Text property record parsing ────────────────────────────────────

fn parse_text_record_stream(data: &[u8]) -> Vec<HashMap<String, String>> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + len > data.len() {
            break;
        }
        let record_bytes = &data[offset..offset + len];
        offset += len;

        // Parse as Latin-1 (Altium uses Windows-1252)
        let text: String = record_bytes
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();

        let mut props = HashMap::new();
        for pair in text.split('|').filter(|s| !s.is_empty()) {
            if let Some((key, value)) = pair.split_once('=') {
                props.insert(key.to_uppercase(), value.to_string());
            }
        }
        if !props.is_empty() {
            records.push(props);
        }
    }
    records
}

// ─── Coordinate conversion ───────────────────────────────────────────

fn altium_to_mm(units: i32) -> f64 {
    units as f64 * 0.0000254
}

fn convert_point(x: i32, y: i32) -> [f64; 2] {
    [altium_to_mm(x), -altium_to_mm(y)]
}

// ─── Board edges ─────────────────────────────────────────────────────

fn extract_board_edges(board_records: &[HashMap<String, String>]) -> Vec<Drawing> {
    let mut edges = Vec::new();

    for record in board_records {
        if record.get("KIND").map(|v| v.as_str()) != Some("0") {
            continue;
        }
        let vcount: usize = record
            .get("VCOUNT")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        for i in 0..vcount {
            let x0 = parse_altium_coord(record, &format!("VX{i}"));
            let y0 = parse_altium_coord(record, &format!("VY{i}"));
            let next = (i + 1) % vcount;
            let x1 = parse_altium_coord(record, &format!("VX{next}"));
            let y1 = parse_altium_coord(record, &format!("VY{next}"));

            let start = convert_point(x0, y0);
            let end = convert_point(x1, y1);

            // Check for arc segment (SA = start angle, EA = end angle, CX/CY = center)
            let sa_key = format!("SA{i}");
            let ea_key = format!("EA{i}");
            let cx_key = format!("CX{i}");
            let cy_key = format!("CY{i}");

            let has_arc = record.contains_key(&sa_key);

            if has_arc {
                let sa: f64 = record
                    .get(&sa_key)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                let ea: f64 = record
                    .get(&ea_key)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(360.0);
                let cx = parse_altium_coord(record, &cx_key);
                let cy = parse_altium_coord(record, &cy_key);
                let center = convert_point(cx, cy);

                // Compute radius from center to start point
                let dx = start[0] - center[0];
                let dy = start[1] - center[1];
                let radius = (dx * dx + dy * dy).sqrt();

                edges.push(Drawing::Arc {
                    start: center,
                    radius,
                    startangle: sa,
                    endangle: ea,
                    width: 0.05,
                });
            } else {
                edges.push(Drawing::Segment {
                    start,
                    end,
                    width: 0.05,
                });
            }
        }
    }
    edges
}

fn parse_altium_coord(record: &HashMap<String, String>, key: &str) -> i32 {
    record
        .get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as i32)
        .unwrap_or(0)
}

// ─── Build footprints ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_footprints(
    components: &[records::AltiumComponent],
    pads: &[records::AltiumPad],
    tracks: &[records::AltiumTrack],
    arcs: &[records::AltiumArc],
    fills: &[records::AltiumFill],
    texts: &[records::AltiumText],
    nets: &[records::AltiumNet],
    layer_map: &layers::LayerMap,
) -> Vec<Footprint> {
    components
        .iter()
        .enumerate()
        .map(|(idx, comp)| {
            let comp_id = idx as u16;
            let center = convert_point(comp.x, comp.y);

            // Collect pads belonging to this component
            let fp_pads: Vec<Pad> = pads
                .iter()
                .filter(|p| p.component_id == comp_id)
                .map(|p| convert_pad(p, comp, nets, layer_map))
                .collect();

            // Collect drawings
            let mut fp_drawings = Vec::new();
            for t in tracks.iter().filter(|t| t.component_id == comp_id) {
                if let Some(d) = convert_track_drawing(t, layer_map) {
                    fp_drawings.push(d);
                }
            }
            for a in arcs.iter().filter(|a| a.component_id == comp_id) {
                if let Some(d) = convert_arc_drawing(a, layer_map) {
                    fp_drawings.push(d);
                }
            }
            for f in fills.iter().filter(|f| f.component_id == comp_id) {
                if let Some(d) = convert_fill_drawing(f, layer_map) {
                    fp_drawings.push(d);
                }
            }
            for txt in texts.iter().filter(|t| t.component_id == comp_id) {
                if let Some(d) = convert_text_drawing(txt, comp, layer_map) {
                    fp_drawings.push(d);
                }
            }

            // Bounding box
            let mut bbox = BBox::empty();
            for pad in &fp_pads {
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
                    minx: center[0] - 0.5,
                    miny: center[1] - 0.5,
                    maxx: center[0] + 0.5,
                    maxy: center[1] + 0.5,
                };
            }

            let side = layer_map.side(comp.layer);

            Footprint {
                ref_: comp.designator.clone(),
                center,
                bbox: FootprintBBox {
                    pos: center,
                    relpos: [bbox.minx - center[0], bbox.miny - center[1]],
                    size: [bbox.maxx - bbox.minx, bbox.maxy - bbox.miny],
                    angle: comp.rotation,
                },
                pads: fp_pads,
                drawings: fp_drawings,
                layer: side.to_string(),
            }
        })
        .collect()
}

fn convert_pad(
    pad: &records::AltiumPad,
    _comp: &records::AltiumComponent,
    nets: &[records::AltiumNet],
    layer_map: &layers::LayerMap,
) -> Pad {
    let pos = convert_point(pad.x, pad.y);
    let size_x = altium_to_mm(pad.size_x);
    let size_y = altium_to_mm(pad.size_y);

    let (shape, polygons) = match pad.shape {
        1 => ("circle", None),
        2 => ("rect", None),
        3 => {
            // Octagonal pad — generate 8-vertex polygon
            let sx = size_x / 2.0;
            let sy = size_y / 2.0;
            let chamfer = sx.min(sy) * 0.3; // ~30% chamfer for octagon
            let verts = vec![
                [sx, sy - chamfer],
                [sx - chamfer, sy],
                [-(sx - chamfer), sy],
                [-sx, sy - chamfer],
                [-sx, -(sy - chamfer)],
                [-(sx - chamfer), -sy],
                [sx - chamfer, -sy],
                [sx, -(sy - chamfer)],
            ];
            ("custom", Some(vec![verts]))
        }
        9 => ("roundrect", None),
        _ => ("rect", None),
    };

    let is_th = pad.hole_size > 0;
    let pad_type = if is_th { "th" } else { "smd" };

    let layers = if pad.layer == 74 || is_th {
        vec!["F".to_string(), "B".to_string()]
    } else {
        vec![layer_map.side(pad.layer).to_string()]
    };

    let net = nets
        .get(pad.net_id as usize)
        .map(|n| n.name.clone())
        .filter(|n| !n.is_empty());

    let pin1 = if pad.name == "1" || pad.name == "A1" {
        Some(1u8)
    } else {
        None
    };

    let (drillshape, drillsize) = if is_th {
        let d = altium_to_mm(pad.hole_size);
        (Some("circle".to_string()), Some([d, d]))
    } else {
        (None, None)
    };

    let angle = if pad.rotation != 0.0 {
        Some(pad.rotation)
    } else {
        None
    };

    Pad {
        layers,
        pos,
        size: [size_x, size_y],
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
        polygons,
    }
}

fn convert_track_drawing(
    track: &records::AltiumTrack,
    layer_map: &layers::LayerMap,
) -> Option<FootprintDrawing> {
    let cat = layer_map.category(track.layer);
    let side = match cat {
        layers::LayerCategory::SilkF | layers::LayerCategory::FabF => "F",
        layers::LayerCategory::SilkB | layers::LayerCategory::FabB => "B",
        _ => return None,
    };
    let start = convert_point(track.start_x, track.start_y);
    let end = convert_point(track.end_x, track.end_y);
    let width = altium_to_mm(track.width);
    Some(FootprintDrawing {
        layer: side.to_string(),
        drawing: FootprintDrawingItem::Shape(Drawing::Segment { start, end, width }),
    })
}

fn convert_arc_drawing(
    arc: &records::AltiumArc,
    layer_map: &layers::LayerMap,
) -> Option<FootprintDrawing> {
    let cat = layer_map.category(arc.layer);
    let side = match cat {
        layers::LayerCategory::SilkF | layers::LayerCategory::FabF => "F",
        layers::LayerCategory::SilkB | layers::LayerCategory::FabB => "B",
        _ => return None,
    };
    let center = convert_point(arc.center_x, arc.center_y);
    let radius = altium_to_mm(arc.radius);
    let width = altium_to_mm(arc.width);
    Some(FootprintDrawing {
        layer: side.to_string(),
        drawing: FootprintDrawingItem::Shape(Drawing::Arc {
            start: center,
            radius,
            startangle: arc.start_angle,
            endangle: arc.end_angle,
            width,
        }),
    })
}

fn convert_fill_drawing(
    fill: &records::AltiumFill,
    layer_map: &layers::LayerMap,
) -> Option<FootprintDrawing> {
    let cat = layer_map.category(fill.layer);
    let side = match cat {
        layers::LayerCategory::SilkF | layers::LayerCategory::FabF => "F",
        layers::LayerCategory::SilkB | layers::LayerCategory::FabB => "B",
        _ => return None,
    };
    let start = convert_point(fill.x1, fill.y1);
    let end = convert_point(fill.x2, fill.y2);
    Some(FootprintDrawing {
        layer: side.to_string(),
        drawing: FootprintDrawingItem::Shape(Drawing::Rect {
            start,
            end,
            width: 0.0,
        }),
    })
}

fn convert_text_drawing(
    txt: &records::AltiumText,
    comp: &records::AltiumComponent,
    layer_map: &layers::LayerMap,
) -> Option<FootprintDrawing> {
    let cat = layer_map.category(txt.layer);
    let side = match cat {
        layers::LayerCategory::SilkF | layers::LayerCategory::FabF => "F",
        layers::LayerCategory::SilkB | layers::LayerCategory::FabB => "B",
        _ => return None,
    };
    let pos = convert_point(txt.x, txt.y);
    let height = altium_to_mm(txt.height);

    let text_content = if txt.is_designator {
        comp.designator.clone()
    } else if txt.is_comment {
        comp.comment.clone()
    } else {
        txt.text.clone()
    };

    let is_ref = if txt.is_designator { Some(1u8) } else { None };
    let val = if txt.is_comment { Some(1u8) } else { None };
    let angle = if txt.rotation != 0.0 {
        Some(txt.rotation)
    } else {
        None
    };

    Some(FootprintDrawing {
        layer: side.to_string(),
        drawing: FootprintDrawingItem::Text(TextDrawing {
            svgpath: None,
            thickness: None,
            is_ref,
            val,
            pos: Some(pos),
            text: Some(text_content),
            height: Some(height),
            width: None,
            justify: None,
            angle,
            attr: None,
        }),
    })
}

// ─── Board-level drawings ────────────────────────────────────────────

fn categorize_drawings(
    tracks: &[records::AltiumTrack],
    arcs: &[records::AltiumArc],
    fills: &[records::AltiumFill],
    layer_map: &layers::LayerMap,
) -> Drawings {
    let mut silk_f = Vec::new();
    let mut silk_b = Vec::new();
    let mut fab_f = Vec::new();
    let mut fab_b = Vec::new();

    // Free tracks (component_id == 0xFFFF)
    for t in tracks.iter().filter(|t| t.component_id == 0xFFFF) {
        let start = convert_point(t.start_x, t.start_y);
        let end = convert_point(t.end_x, t.end_y);
        let width = altium_to_mm(t.width);
        let drawing = Drawing::Segment { start, end, width };
        match layer_map.category(t.layer) {
            layers::LayerCategory::SilkF => silk_f.push(drawing),
            layers::LayerCategory::SilkB => silk_b.push(drawing),
            layers::LayerCategory::FabF => fab_f.push(drawing),
            layers::LayerCategory::FabB => fab_b.push(drawing),
            _ => {}
        }
    }

    for a in arcs.iter().filter(|a| a.component_id == 0xFFFF) {
        let center = convert_point(a.center_x, a.center_y);
        let radius = altium_to_mm(a.radius);
        let width = altium_to_mm(a.width);
        let drawing = Drawing::Arc {
            start: center,
            radius,
            startangle: a.start_angle,
            endangle: a.end_angle,
            width,
        };
        match layer_map.category(a.layer) {
            layers::LayerCategory::SilkF => silk_f.push(drawing),
            layers::LayerCategory::SilkB => silk_b.push(drawing),
            layers::LayerCategory::FabF => fab_f.push(drawing),
            layers::LayerCategory::FabB => fab_b.push(drawing),
            _ => {}
        }
    }

    for f in fills.iter().filter(|f| f.component_id == 0xFFFF) {
        let start = convert_point(f.x1, f.y1);
        let end = convert_point(f.x2, f.y2);
        let drawing = Drawing::Rect {
            start,
            end,
            width: 0.0,
        };
        match layer_map.category(f.layer) {
            layers::LayerCategory::SilkF => silk_f.push(drawing),
            layers::LayerCategory::SilkB => silk_b.push(drawing),
            layers::LayerCategory::FabF => fab_f.push(drawing),
            layers::LayerCategory::FabB => fab_b.push(drawing),
            _ => {}
        }
    }

    Drawings {
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
    }
}

// ─── Track data ──────────────────────────────────────────────────────

fn build_track_data(
    tracks: &[records::AltiumTrack],
    arcs: &[records::AltiumArc],
    vias: &[records::AltiumVia],
    nets: &[records::AltiumNet],
    layer_map: &layers::LayerMap,
) -> LayerData<Vec<Track>> {
    let mut front = Vec::new();
    let mut back = Vec::new();

    for t in tracks.iter().filter(|t| t.component_id == 0xFFFF) {
        let start = convert_point(t.start_x, t.start_y);
        let end = convert_point(t.end_x, t.end_y);
        let width = altium_to_mm(t.width);
        let net = nets
            .get(t.net_id as usize)
            .map(|n| n.name.clone())
            .filter(|n| !n.is_empty());
        let track = Track::Segment {
            start,
            end,
            width,
            net,
            drillsize: None,
        };
        match layer_map.category(t.layer) {
            layers::LayerCategory::CopperF => front.push(track),
            layers::LayerCategory::CopperB => back.push(track),
            _ => {}
        }
    }

    for a in arcs.iter().filter(|a| a.component_id == 0xFFFF) {
        let center = convert_point(a.center_x, a.center_y);
        let radius = altium_to_mm(a.radius);
        let width = altium_to_mm(a.width);
        let net = nets
            .get(a.net_id as usize)
            .map(|n| n.name.clone())
            .filter(|n| !n.is_empty());
        let track = Track::Arc {
            center,
            startangle: a.start_angle,
            endangle: a.end_angle,
            radius,
            width,
            net,
        };
        match layer_map.category(a.layer) {
            layers::LayerCategory::CopperF => front.push(track),
            layers::LayerCategory::CopperB => back.push(track),
            _ => {}
        }
    }

    for v in vias {
        let pos = convert_point(v.x, v.y);
        let size = altium_to_mm(v.diameter);
        let drill = altium_to_mm(v.hole_size);
        let net = nets
            .get(v.net_id as usize)
            .map(|n| n.name.clone())
            .filter(|n| !n.is_empty());
        let via = Track::Segment {
            start: pos,
            end: pos,
            width: size,
            net: net.clone(),
            drillsize: Some(drill),
        };
        front.push(via.clone());
        back.push(Track::Segment {
            start: pos,
            end: pos,
            width: size,
            net,
            drillsize: Some(drill),
        });
    }

    LayerData {
        front,
        back,
        inner: HashMap::new(),
    }
}

// ─── Metadata ────────────────────────────────────────────────────────

fn extract_metadata(board_records: &[HashMap<String, String>]) -> Metadata {
    let board = board_records.first();
    Metadata {
        title: board
            .and_then(|b| b.get("DESIGNNAME"))
            .cloned()
            .unwrap_or_default(),
        revision: String::new(),
        company: String::new(),
        date: String::new(),
    }
}

// ─── Bbox ────────────────────────────────────────────────────────────

fn compute_edges_bbox(edges: &[Drawing]) -> BBox {
    let mut bbox = BBox::empty();
    for edge in edges {
        match edge {
            Drawing::Segment { start, end, .. } => {
                bbox.expand_point(start[0], start[1]);
                bbox.expand_point(end[0], end[1]);
            }
            Drawing::Arc { start, radius, .. } => {
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
