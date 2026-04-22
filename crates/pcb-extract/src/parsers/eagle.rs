use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;

/// Parse an Eagle/Fusion360 .brd/.fbrd file into PcbData.
///
/// Supports both Eagle XML (6.0+) and Eagle binary (pre-6.0) formats.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    if super::eagle_binary::is_eagle_binary(data) {
        return super::eagle_binary::parse(data, opts);
    }

    if is_boardview_ascii(data) {
        return Err(ExtractError::ParseError(
            "File is an OpenBoardView ASCII .brd (debug/diagnostic export), not an Eagle design file. \
             This format is not supported."
                .to_string(),
        ));
    }

    let text = std::str::from_utf8(data).map_err(|_| {
        ExtractError::ParseError(
            "File appears to be a binary PCB format (possibly Cadence Allegro). \
             Only Eagle XML and Eagle binary .brd files are supported."
                .to_string(),
        )
    })?;
    let parse_opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, parse_opts)
        .map_err(|e| ExtractError::ParseError(format!("XML parse error: {e}")))?;

    let board = doc
        .descendants()
        .find(|n| n.has_tag_name("board"))
        .ok_or_else(|| ExtractError::ParseError("No <board> element found".to_string()))?;

    // 1. Parse libraries → footprint definitions
    let packages = parse_libraries(&board);

    // 2. Parse elements → component placements
    let (footprints, components) = parse_elements(&board, &packages, opts);

    // 3. Parse plain → board edges, drawings
    let (edges, silk_f, silk_b, fab_f, fab_b) = parse_plain(&board);

    // 4. Parse signals → tracks and copper-pour zones
    let (track_f, track_b, zones_f, zones_b) = if opts.include_tracks {
        parse_signals(&board)
    } else {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
    };

    let edges_bbox = BBox::from_drawings(&edges);
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

    let zones = if opts.include_tracks && !(zones_f.is_empty() && zones_b.is_empty()) {
        Some(LayerData {
            front: zones_f,
            back: zones_b,
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
        format: None,
        bom,
        parser_version: None,
        ibom_version: None,
        tracks,
        copper_pads: None,
        zones,
        nets: None,
        font_data: None,
    })
}

// ─── Layer mapping ───────────────────────────────────────────────────

enum EagleLayerCat {
    CopperF,
    CopperB,
    SilkF,
    SilkB,
    FabF,
    FabB,
    Edge,
    Other,
}

/// Detect OpenBoardView ASCII `.brd` files, which share the `.brd` extension
/// with Eagle but have a completely different format. The first section
/// keyword (`BRDOUT:`, `NETS:`, `PARTS:`, `PINS:`, `NAILS:`, or `FORMAT:`)
/// appears within the first few lines.
fn is_boardview_ascii(data: &[u8]) -> bool {
    let prefix_len = data.len().min(512);
    let Ok(head) = std::str::from_utf8(&data[..prefix_len]) else {
        return false;
    };
    const MARKERS: &[&str] = &["BRDOUT:", "NETS:", "PARTS:", "PINS:", "NAILS:", "FORMAT:"];
    MARKERS.iter().any(|m| head.contains(m))
}

fn categorize_eagle_layer(layer: u32) -> EagleLayerCat {
    match layer {
        1 => EagleLayerCat::CopperF,
        16 => EagleLayerCat::CopperB,
        20 => EagleLayerCat::Edge,
        21 | 25 => EagleLayerCat::SilkF,
        22 | 26 => EagleLayerCat::SilkB,
        27 | 51 => EagleLayerCat::FabF,
        28 | 52 => EagleLayerCat::FabB,
        _ => EagleLayerCat::Other,
    }
}

fn layer_side(layer: u32) -> &'static str {
    match layer {
        1 | 21 | 25 | 27 | 51 => "F",
        16 | 22 | 26 | 28 | 52 => "B",
        _ => "F",
    }
}

// ─── Package definition ──────────────────────────────────────────────

struct EaglePackage {
    pads: Vec<EaglePad>,
    smds: Vec<EagleSmd>,
    wires: Vec<EagleWire>,
    circles: Vec<EagleCircle>,
    rects: Vec<EagleRect>,
}

struct EaglePad {
    name: String,
    x: f64,
    y: f64,
    drill: f64,
    diameter: f64,
    shape: String,
}

struct EagleSmd {
    name: String,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    layer: u32,
    roundness: f64,
}

struct EagleWire {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
    layer: u32,
}

struct EagleCircle {
    x: f64,
    y: f64,
    radius: f64,
    width: f64,
    layer: u32,
}

struct EagleRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    layer: u32,
}

// ─── Parse libraries ─────────────────────────────────────────────────

fn parse_libraries(board: &roxmltree::Node) -> HashMap<String, EaglePackage> {
    let mut packages = HashMap::new();

    for lib in board.children().filter(|n| n.has_tag_name("libraries")) {
        for library in lib.children().filter(|n| n.has_tag_name("library")) {
            let lib_name = library.attribute("name").unwrap_or("");
            for pkgs in library.children().filter(|n| n.has_tag_name("packages")) {
                for pkg in pkgs.children().filter(|n| n.has_tag_name("package")) {
                    let pkg_name = pkg.attribute("name").unwrap_or("");
                    let key = format!("{lib_name}/{pkg_name}");
                    let package = parse_package(&pkg);
                    packages.insert(key, package);
                }
            }
        }
    }

    packages
}

fn parse_package(pkg: &roxmltree::Node) -> EaglePackage {
    let mut pads = Vec::new();
    let mut smds = Vec::new();
    let mut wires = Vec::new();
    let mut circles = Vec::new();
    let mut rects = Vec::new();

    for child in pkg.children() {
        match child.tag_name().name() {
            "pad" => {
                pads.push(EaglePad {
                    name: child.attribute("name").unwrap_or("").to_string(),
                    x: parse_f64(&child, "x"),
                    y: parse_f64(&child, "y"),
                    drill: parse_f64(&child, "drill"),
                    diameter: parse_f64_or(&child, "diameter", 0.0),
                    shape: child.attribute("shape").unwrap_or("round").to_string(),
                });
            }
            "smd" => {
                smds.push(EagleSmd {
                    name: child.attribute("name").unwrap_or("").to_string(),
                    x: parse_f64(&child, "x"),
                    y: parse_f64(&child, "y"),
                    dx: parse_f64(&child, "dx"),
                    dy: parse_f64(&child, "dy"),
                    layer: parse_u32(&child, "layer"),
                    roundness: parse_f64_or(&child, "roundness", 0.0),
                });
            }
            "wire" => {
                wires.push(EagleWire {
                    x1: parse_f64(&child, "x1"),
                    y1: parse_f64(&child, "y1"),
                    x2: parse_f64(&child, "x2"),
                    y2: parse_f64(&child, "y2"),
                    width: parse_f64(&child, "width"),
                    layer: parse_u32(&child, "layer"),
                });
            }
            "circle" => {
                circles.push(EagleCircle {
                    x: parse_f64(&child, "x"),
                    y: parse_f64(&child, "y"),
                    radius: parse_f64(&child, "radius"),
                    width: parse_f64(&child, "width"),
                    layer: parse_u32(&child, "layer"),
                });
            }
            "rectangle" => {
                rects.push(EagleRect {
                    x1: parse_f64(&child, "x1"),
                    y1: parse_f64(&child, "y1"),
                    x2: parse_f64(&child, "x2"),
                    y2: parse_f64(&child, "y2"),
                    layer: parse_u32(&child, "layer"),
                });
            }
            _ => {}
        }
    }

    EaglePackage {
        pads,
        smds,
        wires,
        circles,
        rects,
    }
}

// ─── Parse elements ──────────────────────────────────────────────────

fn parse_elements(
    board: &roxmltree::Node,
    packages: &HashMap<String, EaglePackage>,
    _opts: &ExtractOptions,
) -> (Vec<Footprint>, Vec<Component>) {
    let mut footprints = Vec::new();
    let mut components = Vec::new();

    for elements in board.children().filter(|n| n.has_tag_name("elements")) {
        for elem in elements.children().filter(|n| n.has_tag_name("element")) {
            let name = elem.attribute("name").unwrap_or("").to_string();
            let value = elem.attribute("value").unwrap_or("").to_string();
            let lib = elem.attribute("library").unwrap_or("");
            let pkg = elem.attribute("package").unwrap_or("");
            let x = parse_f64(&elem, "x");
            let y = parse_f64(&elem, "y");

            let rot_str = elem.attribute("rot").unwrap_or("R0");
            let (angle, mirrored) = parse_eagle_rotation(rot_str);

            let pkg_key = format!("{lib}/{pkg}");
            let package = packages.get(&pkg_key);

            // Determine side from mirror flag
            let side = if mirrored { "B" } else { "F" };

            // Build pads from package definition
            let mut fp_pads = Vec::new();
            let mut fp_drawings = Vec::new();

            if let Some(package) = package {
                // Through-hole pads
                for pad in &package.pads {
                    let (px, py) = rotate_point(pad.x, pad.y, angle, mirrored);
                    let diameter = if pad.diameter > 0.0 {
                        pad.diameter
                    } else {
                        pad.drill * 2.0
                    };
                    fp_pads.push(Pad {
                        layers: vec!["F".to_string(), "B".to_string()],
                        pos: [x + px, -(y + py)],
                        size: [diameter, diameter],
                        shape: match pad.shape.as_str() {
                            "square" => "rect".to_string(),
                            "long" => "oval".to_string(),
                            "octagon" => "rect".to_string(),
                            _ => "circle".to_string(),
                        },
                        pad_type: "th".to_string(),
                        angle: if angle != 0.0 { Some(angle) } else { None },
                        pin1: if pad.name == "1" || pad.name == "A1" {
                            Some(1)
                        } else {
                            None
                        },
                        net: None,
                        offset: None,
                        radius: None,
                        chamfpos: None,
                        chamfratio: None,
                        drillshape: Some("circle".to_string()),
                        drillsize: Some([pad.drill, pad.drill]),
                        svgpath: None,
                        polygons: None,
                    });
                }

                // SMD pads
                for smd in &package.smds {
                    let (px, py) = rotate_point(smd.x, smd.y, angle, mirrored);
                    let pad_side = if mirrored {
                        mirror_layer(smd.layer)
                    } else {
                        layer_side(smd.layer).to_string()
                    };
                    let shape = if smd.roundness > 0.0 {
                        "roundrect"
                    } else {
                        "rect"
                    };
                    fp_pads.push(Pad {
                        layers: vec![pad_side],
                        pos: [x + px, -(y + py)],
                        size: [smd.dx, smd.dy],
                        shape: shape.to_string(),
                        pad_type: "smd".to_string(),
                        angle: if angle != 0.0 { Some(angle) } else { None },
                        pin1: if smd.name == "1" || smd.name == "A1" {
                            Some(1)
                        } else {
                            None
                        },
                        net: None,
                        offset: None,
                        radius: if smd.roundness > 0.0 {
                            Some(smd.roundness / 100.0 * smd.dx.min(smd.dy) / 2.0)
                        } else {
                            None
                        },
                        chamfpos: None,
                        chamfratio: None,
                        drillshape: None,
                        drillsize: None,
                        svgpath: None,
                        polygons: None,
                    });
                }

                // Package drawings (wires on silk/fab layers)
                for wire in &package.wires {
                    let effective_layer = if mirrored {
                        mirror_eagle_layer(wire.layer)
                    } else {
                        wire.layer
                    };
                    let cat = categorize_eagle_layer(effective_layer);
                    let draw_side = match cat {
                        EagleLayerCat::SilkF | EagleLayerCat::FabF => "F",
                        EagleLayerCat::SilkB | EagleLayerCat::FabB => "B",
                        _ => continue,
                    };
                    let (sx, sy) = rotate_point(wire.x1, wire.y1, angle, mirrored);
                    let (ex, ey) = rotate_point(wire.x2, wire.y2, angle, mirrored);
                    fp_drawings.push(FootprintDrawing {
                        layer: draw_side.to_string(),
                        drawing: FootprintDrawingItem::Shape(Drawing::Segment {
                            start: [x + sx, -(y + sy)],
                            end: [x + ex, -(y + ey)],
                            width: wire.width,
                        }),
                    });
                }

                for circle in &package.circles {
                    let effective_layer = if mirrored {
                        mirror_eagle_layer(circle.layer)
                    } else {
                        circle.layer
                    };
                    let cat = categorize_eagle_layer(effective_layer);
                    let draw_side = match cat {
                        EagleLayerCat::SilkF | EagleLayerCat::FabF => "F",
                        EagleLayerCat::SilkB | EagleLayerCat::FabB => "B",
                        _ => continue,
                    };
                    let (cx, cy) = rotate_point(circle.x, circle.y, angle, mirrored);
                    fp_drawings.push(FootprintDrawing {
                        layer: draw_side.to_string(),
                        drawing: FootprintDrawingItem::Shape(Drawing::Circle {
                            start: [x + cx, -(y + cy)],
                            radius: circle.radius,
                            width: circle.width,
                            filled: None,
                        }),
                    });
                }

                for rect in &package.rects {
                    let effective_layer = if mirrored {
                        mirror_eagle_layer(rect.layer)
                    } else {
                        rect.layer
                    };
                    let cat = categorize_eagle_layer(effective_layer);
                    let draw_side = match cat {
                        EagleLayerCat::SilkF | EagleLayerCat::FabF => "F",
                        EagleLayerCat::SilkB | EagleLayerCat::FabB => "B",
                        _ => continue,
                    };
                    let (sx, sy) = rotate_point(rect.x1, rect.y1, angle, mirrored);
                    let (ex, ey) = rotate_point(rect.x2, rect.y2, angle, mirrored);
                    fp_drawings.push(FootprintDrawing {
                        layer: draw_side.to_string(),
                        drawing: FootprintDrawingItem::Shape(Drawing::Rect {
                            start: [x + sx, -(y + sy)],
                            end: [x + ex, -(y + ey)],
                            width: 0.0,
                        }),
                    });
                }
            }

            let fp_bbox = FootprintBBox::from_pads(&fp_pads, [x, -y], angle);

            let idx = footprints.len();

            footprints.push(Footprint {
                ref_: name.clone(),
                center: [x, -y],
                bbox: fp_bbox,
                pads: fp_pads,
                drawings: fp_drawings,
                layer: side.to_string(),
            });

            components.push(Component {
                ref_: name,
                val: value,
                footprint_name: pkg.to_string(),
                layer: if mirrored { Side::Back } else { Side::Front },
                footprint_index: idx,
                extra_fields: HashMap::new(),
                attr: None,
            });
        }
    }

    (footprints, components)
}

// ─── Parse plain (board edges, drawings) ─────────────────────────────

#[allow(clippy::type_complexity)]
fn parse_plain(
    board: &roxmltree::Node,
) -> (
    Vec<Drawing>,
    Vec<Drawing>,
    Vec<Drawing>,
    Vec<Drawing>,
    Vec<Drawing>,
) {
    let mut edges = Vec::new();
    let mut silk_f = Vec::new();
    let mut silk_b = Vec::new();
    let mut fab_f = Vec::new();
    let mut fab_b = Vec::new();

    for plain in board.children().filter(|n| n.has_tag_name("plain")) {
        for child in plain.children() {
            match child.tag_name().name() {
                "wire" => {
                    let x1 = parse_f64(&child, "x1");
                    let y1 = -parse_f64(&child, "y1");
                    let x2 = parse_f64(&child, "x2");
                    let y2 = -parse_f64(&child, "y2");
                    let width = parse_f64(&child, "width");
                    let layer = parse_u32(&child, "layer");
                    let drawing = Drawing::Segment {
                        start: [x1, y1],
                        end: [x2, y2],
                        width,
                    };
                    match categorize_eagle_layer(layer) {
                        EagleLayerCat::Edge => edges.push(drawing),
                        EagleLayerCat::SilkF => silk_f.push(drawing),
                        EagleLayerCat::SilkB => silk_b.push(drawing),
                        EagleLayerCat::FabF => fab_f.push(drawing),
                        EagleLayerCat::FabB => fab_b.push(drawing),
                        _ => {}
                    }
                }
                "circle" => {
                    let x = parse_f64(&child, "x");
                    let y = -parse_f64(&child, "y");
                    let radius = parse_f64(&child, "radius");
                    let width = parse_f64(&child, "width");
                    let layer = parse_u32(&child, "layer");
                    let drawing = Drawing::Circle {
                        start: [x, y],
                        radius,
                        width,
                        filled: None,
                    };
                    match categorize_eagle_layer(layer) {
                        EagleLayerCat::Edge => edges.push(drawing),
                        EagleLayerCat::SilkF => silk_f.push(drawing),
                        EagleLayerCat::SilkB => silk_b.push(drawing),
                        EagleLayerCat::FabF => fab_f.push(drawing),
                        EagleLayerCat::FabB => fab_b.push(drawing),
                        _ => {}
                    }
                }
                "rectangle" => {
                    let x1 = parse_f64(&child, "x1");
                    let y1 = -parse_f64(&child, "y1");
                    let x2 = parse_f64(&child, "x2");
                    let y2 = -parse_f64(&child, "y2");
                    let layer = parse_u32(&child, "layer");
                    let drawing = Drawing::Rect {
                        start: [x1, y1],
                        end: [x2, y2],
                        width: 0.0,
                    };
                    match categorize_eagle_layer(layer) {
                        EagleLayerCat::Edge => edges.push(drawing),
                        EagleLayerCat::SilkF => silk_f.push(drawing),
                        EagleLayerCat::SilkB => silk_b.push(drawing),
                        EagleLayerCat::FabF => fab_f.push(drawing),
                        EagleLayerCat::FabB => fab_b.push(drawing),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    (edges, silk_f, silk_b, fab_f, fab_b)
}

// ─── Parse signals (tracks/vias) ─────────────────────────────────────

fn parse_signals(board: &roxmltree::Node) -> (Vec<Track>, Vec<Track>, Vec<Zone>, Vec<Zone>) {
    let mut front = Vec::new();
    let mut back = Vec::new();
    let mut zones_f = Vec::new();
    let mut zones_b = Vec::new();

    for signals in board.children().filter(|n| n.has_tag_name("signals")) {
        for signal in signals.children().filter(|n| n.has_tag_name("signal")) {
            let net_name = signal.attribute("name").unwrap_or("").to_string();
            let net = if net_name.is_empty() {
                None
            } else {
                Some(net_name)
            };

            for child in signal.children() {
                match child.tag_name().name() {
                    "wire" => {
                        let x1 = parse_f64(&child, "x1");
                        let y1 = -parse_f64(&child, "y1");
                        let x2 = parse_f64(&child, "x2");
                        let y2 = -parse_f64(&child, "y2");
                        let width = parse_f64(&child, "width");
                        let layer = parse_u32(&child, "layer");
                        let track = Track::Segment {
                            start: [x1, y1],
                            end: [x2, y2],
                            width,
                            net: net.clone(),
                            drillsize: None,
                        };
                        match categorize_eagle_layer(layer) {
                            EagleLayerCat::CopperF => front.push(track),
                            EagleLayerCat::CopperB => back.push(track),
                            _ => {}
                        }
                    }
                    "via" => {
                        let x = parse_f64(&child, "x");
                        let y = -parse_f64(&child, "y");
                        let drill = parse_f64(&child, "drill");
                        let diameter = parse_f64_or(&child, "diameter", drill * 2.0);
                        let via = Track::Segment {
                            start: [x, y],
                            end: [x, y],
                            width: diameter,
                            net: net.clone(),
                            drillsize: Some(drill),
                        };
                        front.push(via.clone());
                        back.push(Track::Segment {
                            start: [x, y],
                            end: [x, y],
                            width: diameter,
                            net: net.clone(),
                            drillsize: Some(drill),
                        });
                    }
                    "polygon" => {
                        let layer = parse_u32(&child, "layer");
                        let width = parse_f64(&child, "width");
                        let ring: Vec<[f64; 2]> = child
                            .children()
                            .filter(|n| n.has_tag_name("vertex"))
                            .map(|v| [parse_f64(&v, "x"), -parse_f64(&v, "y")])
                            .collect();
                        if ring.len() < 3 {
                            continue;
                        }
                        let zone = Zone {
                            polygons: Some(vec![ring]),
                            svgpath: None,
                            width: Some(width),
                            net: net.clone(),
                            fillrule: None,
                        };
                        match categorize_eagle_layer(layer) {
                            EagleLayerCat::CopperF => zones_f.push(zone),
                            EagleLayerCat::CopperB => zones_b.push(zone),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (front, back, zones_f, zones_b)
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn parse_f64(node: &roxmltree::Node, attr: &str) -> f64 {
    node.attribute(attr)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

fn parse_f64_or(node: &roxmltree::Node, attr: &str, default: f64) -> f64 {
    node.attribute(attr)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn parse_u32(node: &roxmltree::Node, attr: &str) -> u32 {
    node.attribute(attr)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn parse_eagle_rotation(rot: &str) -> (f64, bool) {
    let mirrored = rot.starts_with('M');
    let angle_str = rot.trim_start_matches('M').trim_start_matches('R');
    let angle: f64 = angle_str.parse().unwrap_or(0.0);
    (angle, mirrored)
}

fn rotate_point(x: f64, y: f64, angle: f64, mirror: bool) -> (f64, f64) {
    let x = if mirror { -x } else { x };
    if angle == 0.0 {
        return (x, y);
    }
    let rad = angle.to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();
    (x * cos_a - y * sin_a, x * sin_a + y * cos_a)
}

fn mirror_layer(layer: u32) -> String {
    match layer {
        1 => "B".to_string(),
        16 => "F".to_string(),
        _ => layer_side(layer).to_string(),
    }
}

fn mirror_eagle_layer(layer: u32) -> u32 {
    match layer {
        1 => 16,
        16 => 1,
        21 => 22,
        22 => 21,
        25 => 26,
        26 => 25,
        27 => 28,
        28 => 27,
        51 => 52,
        52 => 51,
        _ => layer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tracks() -> ExtractOptions {
        ExtractOptions {
            include_tracks: true,
            ..ExtractOptions::default()
        }
    }

    fn minimal_board_with_signal(signal_xml: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<eagle version="9.6.2">
<drawing><board>
<libraries/><elements/><plain/>
<signals>
{signal_xml}
</signals>
</board></drawing></eagle>"#
        )
    }

    #[test]
    fn detects_boardview_ascii_brdout() {
        let data = b"0\r\nBRDOUT: 73 5118 4724\r\n4968 1929\r\n";
        assert!(is_boardview_ascii(data));
    }

    #[test]
    fn detects_boardview_ascii_other_sections() {
        assert!(is_boardview_ascii(b"NETS: 12\nGND\n"));
        assert!(is_boardview_ascii(b"PARTS: 5\n"));
    }

    #[test]
    fn does_not_flag_eagle_xml_as_boardview() {
        let data = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<eagle version=\"9.6.2\">\n";
        assert!(!is_boardview_ascii(data));
    }

    #[test]
    fn rejects_boardview_with_clear_error() {
        let data = b"0\r\nBRDOUT: 73 5118 4724\r\n";
        let err = parse(data, &ExtractOptions::default()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("OpenBoardView"), "unexpected error: {msg}");
    }

    #[test]
    fn extracts_gnd_copper_pour_on_front() {
        let xml = minimal_board_with_signal(
            r#"<signal name="GND">
                <polygon width="0.254" layer="1" pour="solid">
                  <vertex x="0" y="0"/>
                  <vertex x="10" y="0"/>
                  <vertex x="10" y="5"/>
                  <vertex x="0" y="5"/>
                </polygon>
              </signal>"#,
        );
        let pcb = parse(xml.as_bytes(), &with_tracks()).unwrap();
        let zones = pcb.zones.expect("zones should be populated");
        assert_eq!(zones.back.len(), 0);
        assert_eq!(zones.front.len(), 1);

        let z = &zones.front[0];
        assert_eq!(z.net.as_deref(), Some("GND"));
        assert_eq!(z.width, Some(0.254));
        let polys = z.polygons.as_ref().expect("polygon vertices");
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].len(), 4);
        // Y should be flipped from Eagle's Y-up to pastebom's Y-down.
        assert_eq!(polys[0][0], [0.0, -0.0]);
        assert_eq!(polys[0][2], [10.0, -5.0]);
    }

    #[test]
    fn routes_bottom_copper_pour_to_back_layer() {
        let xml = minimal_board_with_signal(
            r#"<signal name="GND">
                <polygon width="0.3" layer="16">
                  <vertex x="0" y="0"/>
                  <vertex x="1" y="0"/>
                  <vertex x="1" y="1"/>
                </polygon>
              </signal>"#,
        );
        let pcb = parse(xml.as_bytes(), &with_tracks()).unwrap();
        let zones = pcb.zones.unwrap();
        assert_eq!(zones.front.len(), 0);
        assert_eq!(zones.back.len(), 1);
    }

    #[test]
    fn degenerate_polygon_is_skipped() {
        let xml = minimal_board_with_signal(
            r#"<signal name="GND">
                <polygon width="0.3" layer="1">
                  <vertex x="0" y="0"/>
                  <vertex x="1" y="1"/>
                </polygon>
              </signal>"#,
        );
        let pcb = parse(xml.as_bytes(), &with_tracks()).unwrap();
        assert!(pcb.zones.is_none());
    }

    #[test]
    fn boards_without_pours_emit_no_zones_field() {
        let xml = minimal_board_with_signal(
            r#"<signal name="N$1">
                <wire x1="0" y1="0" x2="5" y2="0" width="0.2" layer="1"/>
              </signal>"#,
        );
        let pcb = parse(xml.as_bytes(), &with_tracks()).unwrap();
        assert!(pcb.zones.is_none());
    }

    #[test]
    fn zones_respect_include_tracks_flag() {
        let xml = minimal_board_with_signal(
            r#"<signal name="GND">
                <polygon width="0.3" layer="1">
                  <vertex x="0" y="0"/>
                  <vertex x="1" y="0"/>
                  <vertex x="1" y="1"/>
                </polygon>
              </signal>"#,
        );
        let opts = ExtractOptions {
            include_tracks: false,
            ..ExtractOptions::default()
        };
        let pcb = parse(xml.as_bytes(), &opts).unwrap();
        assert!(pcb.zones.is_none());
    }
}
