use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;

/// Parse an Eagle/Fusion360 .brd/.fbrd file into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let text = std::str::from_utf8(data)
        .map_err(|e| ExtractError::ParseError(format!("Invalid UTF-8: {e}")))?;
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

    // 4. Parse signals → tracks
    let (track_f, track_b) = if opts.include_tracks {
        parse_signals(&board)
    } else {
        (Vec::new(), Vec::new())
    };

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
        zones: None,
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
                    minx: x - 0.5,
                    miny: -y - 0.5,
                    maxx: x + 0.5,
                    maxy: -y + 0.5,
                };
            }

            let fp_bbox = FootprintBBox {
                pos: [x, -y],
                relpos: [bbox.minx - x, bbox.miny - (-y)],
                size: [bbox.maxx - bbox.minx, bbox.maxy - bbox.miny],
                angle,
            };

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

fn parse_signals(board: &roxmltree::Node) -> (Vec<Track>, Vec<Track>) {
    let mut front = Vec::new();
    let mut back = Vec::new();

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
                    _ => {}
                }
            }
        }
    }

    (front, back)
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

fn compute_bbox(edges: &[Drawing]) -> BBox {
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
