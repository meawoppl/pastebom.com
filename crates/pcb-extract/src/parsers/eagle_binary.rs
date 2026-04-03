use crate::bom::{generate_bom, BomConfig};
use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;
use std::collections::HashMap;

// ─── Constants ──────────────────────────────────────────────────────

const SEC_START: u8 = 0x10;
const SEC_LIBRARY: u8 = 0x15;
const SEC_PACKAGES: u8 = 0x19;
const SEC_BOARD: u8 = 0x1b;
const SEC_BOARD_NET: u8 = 0x1c;
const SEC_PACKAGE: u8 = 0x1e;
const SEC_LINE: u8 = 0x22;
const SEC_CIRCLE: u8 = 0x25;
const SEC_RECT: u8 = 0x26;
const SEC_HOLE: u8 = 0x28;
const SEC_VIA: u8 = 0x29;
const SEC_PAD: u8 = 0x2a;
const SEC_SMD: u8 = 0x2b;
const SEC_BOARD_PACKAGE: u8 = 0x2e;
const SEC_BOARD_PACKAGE2: u8 = 0x2f;
const SEC_TEXT: u8 = 0x31;

const RECORD_SIZE: usize = 24;

// String table sentinel
const STRING_SENTINEL: [u8; 4] = [0x13, 0x12, 0x99, 0x19];

// Inline-string overflow marker
const STRING_OVERFLOW: u8 = 0x7f;

/// Internal unit = 0.1 um. Convert to mm.
fn units_to_mm(val: i32) -> f64 {
    val as f64 / 10_000.0
}

/// Decode an angle from 12-bit field (4096 = 360 degrees).
fn decode_angle(raw: u16) -> f64 {
    let bits = raw & 0x0FFF;
    360.0 * bits as f64 / 4096.0
}

// ─── Raw record ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RawRecord {
    data: [u8; RECORD_SIZE],
}

impl RawRecord {
    fn sec_type(&self) -> u8 {
        self.data[0]
    }

    fn u8_at(&self, offset: usize) -> u8 {
        self.data[offset]
    }

    fn u16_at(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.data[offset], self.data[offset + 1]])
    }

    fn i32_at(&self, offset: usize) -> i32 {
        i32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    fn u32_at(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    /// Read an inline string field, returning None if it overflows to the string table.
    fn inline_string(&self, offset: usize, len: usize) -> Option<String> {
        if self.data[offset] == STRING_OVERFLOW {
            None
        } else {
            let slice = &self.data[offset..offset + len];
            let end = slice.iter().position(|&b| b == 0).unwrap_or(len);
            Some(String::from_utf8_lossy(&slice[..end]).to_string())
        }
    }
}

// ─── Parsed section tree node ───────────────────────────────────────

#[derive(Debug)]
struct Section {
    record: RawRecord,
    children: Vec<Section>,
}

// ─── Parsed board-level types ───────────────────────────────────────

struct BinPackage {
    pads: Vec<BinPad>,
    smds: Vec<BinSmd>,
    wires: Vec<BinWire>,
    circles: Vec<BinCircle>,
    rects: Vec<BinRect>,
}

struct BinPad {
    name: String,
    x: f64,
    y: f64,
    drill: f64,
    diameter: f64,
    shape: String,
}

struct BinSmd {
    name: String,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    layer: u8,
    roundness: u8,
}

struct BinWire {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
    layer: u8,
}

struct BinCircle {
    x: f64,
    y: f64,
    radius: f64,
    width: f64,
    layer: u8,
}

struct BinRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    layer: u8,
}

struct BinPlacement {
    name: String,
    value: String,
    x: f64,
    y: f64,
    angle: f64,
    mirrored: bool,
    lib_idx: u16,
    pkg_idx: u16,
}

// ─── Main parse function ────────────────────────────────────────────

pub fn is_eagle_binary(data: &[u8]) -> bool {
    data.len() >= RECORD_SIZE && data[0] == SEC_START
}

pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    if data.len() < RECORD_SIZE {
        return Err(ExtractError::ParseError(
            "Eagle binary: file too short".to_string(),
        ));
    }
    if data[0] != SEC_START {
        return Err(ExtractError::ParseError(
            "Eagle binary: missing start section (0x10)".to_string(),
        ));
    }

    // Parse start record to get section count
    let start_rec = read_record(data, 0)?;
    let num_secs = start_rec.u32_at(4) as usize;
    let _major = start_rec.u8_at(8);
    let _minor = start_rec.u8_at(9);

    if num_secs * RECORD_SIZE > data.len() {
        return Err(ExtractError::ParseError(format!(
            "Eagle binary: section count {} exceeds file size",
            num_secs
        )));
    }

    // Read all raw records
    let mut records = Vec::with_capacity(num_secs);
    for i in 0..num_secs {
        records.push(read_record(data, i * RECORD_SIZE)?);
    }

    // Parse string table
    let string_offset = num_secs * RECORD_SIZE;
    let strings = parse_string_table(data, string_offset)?;

    // Build section tree
    let tree = build_section_tree(&records)?;

    // Extract board data from tree
    let mut string_idx = 0;
    extract_board(&tree, &strings, &mut string_idx, opts)
}

fn read_record(data: &[u8], offset: usize) -> Result<RawRecord, ExtractError> {
    if offset + RECORD_SIZE > data.len() {
        return Err(ExtractError::ParseError(format!(
            "Eagle binary: unexpected end of data at offset {}",
            offset
        )));
    }
    let mut buf = [0u8; RECORD_SIZE];
    buf.copy_from_slice(&data[offset..offset + RECORD_SIZE]);
    Ok(RawRecord { data: buf })
}

fn parse_string_table(data: &[u8], offset: usize) -> Result<Vec<String>, ExtractError> {
    if offset + 8 > data.len() {
        return Ok(Vec::new());
    }

    // Check sentinel
    if data[offset..offset + 4] != STRING_SENTINEL {
        return Ok(Vec::new());
    }

    let size = u32::from_le_bytes([
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ]) as usize;

    let str_start = offset + 8;
    let str_end = (str_start + size).min(data.len());
    let str_data = &data[str_start..str_end];

    let strings: Vec<String> = str_data
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();

    Ok(strings)
}

// ─── Section tree building ──────────────────────────────────────────

fn build_section_tree(records: &[RawRecord]) -> Result<Section, ExtractError> {
    if records.is_empty() {
        return Err(ExtractError::ParseError(
            "Eagle binary: no records".to_string(),
        ));
    }

    let mut idx = 0;
    let root = parse_section_recursive(records, &mut idx)?;
    Ok(root)
}

fn subsection_counts(rec: &RawRecord) -> Vec<usize> {
    match rec.sec_type() {
        SEC_START => {
            let subsecs = rec.u16_at(2) as usize;
            let numsecs = rec.u32_at(4) as usize;
            // subsections layout: [settings subsecs, remaining subsecs]
            let remaining = numsecs.saturating_sub(subsecs + 1);
            vec![subsecs, remaining]
        }
        SEC_BOARD => {
            let draw_subs = rec.u16_at(2) as usize;
            let def_subs = rec.u32_at(12) as usize;
            let pac_subs = rec.u32_at(16) as usize;
            let net_subs = rec.u32_at(20) as usize;
            vec![def_subs, draw_subs, pac_subs, net_subs]
        }
        SEC_LIBRARY => {
            vec![rec.u16_at(2) as usize]
        }
        SEC_PACKAGES => {
            vec![rec.u32_at(4) as usize]
        }
        SEC_PACKAGE => {
            vec![rec.u16_at(2) as usize]
        }
        SEC_BOARD_NET => {
            vec![rec.u16_at(2) as usize]
        }
        SEC_BOARD_PACKAGE => {
            vec![rec.u16_at(2) as usize]
        }
        // Polygon has subsections (its vertices are children)
        0x21 => {
            vec![rec.u16_at(2) as usize]
        }
        _ => vec![],
    }
}

fn parse_section_recursive(
    records: &[RawRecord],
    idx: &mut usize,
) -> Result<Section, ExtractError> {
    if *idx >= records.len() {
        return Err(ExtractError::ParseError(
            "Eagle binary: unexpected end of sections".to_string(),
        ));
    }

    let record = records[*idx].clone();
    *idx += 1;

    let counts = subsection_counts(&record);
    // Budget = total flat records in the children portion (including descendants).
    // Each child consumes 1+ records from the budget.
    let budget: usize = counts.iter().sum();

    let mut children = Vec::new();
    let mut consumed = 0;
    while consumed < budget && *idx < records.len() {
        let before = *idx;
        children.push(parse_section_recursive(records, idx)?);
        consumed += *idx - before;
    }

    Ok(Section { record, children })
}

// ─── Board extraction ───────────────────────────────────────────────

fn extract_board(
    tree: &Section,
    strings: &[String],
    string_idx: &mut usize,
    opts: &ExtractOptions,
) -> Result<PcbData, ExtractError> {
    // Find the board section in the tree
    let board = find_section(tree, SEC_BOARD).ok_or_else(|| {
        ExtractError::ParseError("Eagle binary: no board section found".to_string())
    })?;

    // Parse library packages: the board's first child group is definitions
    // which contains Libraries > Packages > Package > drawables
    let packages = extract_packages(board, strings, string_idx);

    // Parse component placements (board packages)
    let placements = extract_placements(board, strings, string_idx);

    // Convert packages + placements to footprints and components
    let (footprints, components) = build_footprints(&packages, &placements);

    // Parse board-level drawings (edges, silk, fab)
    let (edges, silk_f, silk_b, fab_f, fab_b) = extract_board_drawings(board, strings, string_idx);

    // Parse tracks/vias from nets
    let (track_f, track_b) = if opts.include_tracks {
        extract_tracks(board, strings, string_idx)
    } else {
        (Vec::new(), Vec::new())
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
        ibom_version: None,
        tracks,
        copper_pads: None,
        zones: None,
        nets: None,
        font_data: None,
    })
}

fn find_section(tree: &Section, sec_type: u8) -> Option<&Section> {
    if tree.record.sec_type() == sec_type {
        return Some(tree);
    }
    for child in &tree.children {
        if let Some(found) = find_section(child, sec_type) {
            return Some(found);
        }
    }
    None
}

fn find_all_sections(tree: &Section, sec_type: u8) -> Vec<&Section> {
    let mut result = Vec::new();
    collect_sections(tree, sec_type, &mut result);
    result
}

fn collect_sections<'a>(tree: &'a Section, sec_type: u8, result: &mut Vec<&'a Section>) {
    if tree.record.sec_type() == sec_type {
        result.push(tree);
    }
    for child in &tree.children {
        collect_sections(child, sec_type, result);
    }
}

// ─── Package extraction ─────────────────────────────────────────────

fn extract_packages(
    board: &Section,
    strings: &[String],
    string_idx: &mut usize,
) -> Vec<Vec<BinPackage>> {
    // In .brd files, Packages sections (0x19) serve as library containers.
    // Each Packages section contains Package (0x1e) children.
    let pkg_containers = find_all_sections(board, SEC_PACKAGES);
    let mut all_libs = Vec::new();

    for container in &pkg_containers {
        let mut lib_packages = Vec::new();
        for child in &container.children {
            if child.record.sec_type() == SEC_PACKAGE {
                lib_packages.push(parse_package_section(child, strings, string_idx));
            }
        }
        all_libs.push(lib_packages);
    }

    all_libs
}

fn parse_package_section(pkg: &Section, strings: &[String], string_idx: &mut usize) -> BinPackage {
    let mut pads = Vec::new();
    let mut smds = Vec::new();
    let mut wires = Vec::new();
    let mut circles = Vec::new();
    let mut rects = Vec::new();

    // The package name is in bytes 15..24 (9 bytes inline string)
    // but we don't need it here — it's resolved by index during placement

    for child in &pkg.children {
        match child.record.sec_type() {
            SEC_PAD => {
                let rec = &child.record;
                let x = units_to_mm(rec.i32_at(4));
                let y = units_to_mm(rec.i32_at(8));
                let drill = units_to_mm(rec.u16_at(12) as i32 * 2);
                let diameter = units_to_mm(rec.u16_at(14) as i32 * 2);
                let shape_bits = rec.u8_at(2) & 0x07;
                let shape = match shape_bits {
                    0 => "square",
                    1 => "round",
                    2 => "octagon",
                    3 => "long",
                    4 => "offset",
                    _ => "round",
                };
                let name = resolve_string(rec, 19, 5, strings, string_idx);
                pads.push(BinPad {
                    name,
                    x,
                    y,
                    drill,
                    diameter,
                    shape: shape.to_string(),
                });
            }
            SEC_SMD => {
                let rec = &child.record;
                let x = units_to_mm(rec.i32_at(4));
                let y = units_to_mm(rec.i32_at(8));
                let dx = units_to_mm(rec.u16_at(12) as i32 * 2);
                let dy = units_to_mm(rec.u16_at(14) as i32 * 2);
                let layer = rec.u8_at(3);
                let roundness = rec.u8_at(2);
                let name = resolve_string(rec, 19, 5, strings, string_idx);
                smds.push(BinSmd {
                    name,
                    x,
                    y,
                    dx,
                    dy,
                    layer,
                    roundness,
                });
            }
            SEC_LINE => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x1 = units_to_mm(rec.i32_at(4));
                let y1 = units_to_mm(rec.i32_at(8));
                let x2 = units_to_mm(rec.i32_at(12));
                let y2 = units_to_mm(rec.i32_at(16));
                let width = units_to_mm(rec.u16_at(20) as i32 * 2);
                wires.push(BinWire {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    layer,
                });
            }
            SEC_CIRCLE => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x = units_to_mm(rec.i32_at(4));
                let y = units_to_mm(rec.i32_at(8));
                let radius = units_to_mm(rec.i32_at(12));
                let width = units_to_mm(rec.u16_at(16) as i32 * 2);
                circles.push(BinCircle {
                    x,
                    y,
                    radius,
                    width,
                    layer,
                });
            }
            SEC_RECT => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x1 = units_to_mm(rec.i32_at(4));
                let y1 = units_to_mm(rec.i32_at(8));
                let x2 = units_to_mm(rec.i32_at(12));
                let y2 = units_to_mm(rec.i32_at(16));
                rects.push(BinRect {
                    x1,
                    y1,
                    x2,
                    y2,
                    layer,
                });
            }
            _ => {}
        }
    }

    BinPackage {
        pads,
        smds,
        wires,
        circles,
        rects,
    }
}

// ─── Component placement extraction ─────────────────────────────────

fn extract_placements(
    board: &Section,
    strings: &[String],
    string_idx: &mut usize,
) -> Vec<BinPlacement> {
    let mut placements = Vec::new();
    let bp_sections = find_all_sections(board, SEC_BOARD_PACKAGE);

    for bp in &bp_sections {
        let rec = &bp.record;
        let x = units_to_mm(rec.i32_at(4));
        let y = units_to_mm(rec.i32_at(8));
        let lib_idx = rec.u16_at(12);
        let pkg_idx = rec.u16_at(14);
        let angle_mirror = rec.u16_at(16);
        let angle = decode_angle(angle_mirror);
        let mirrored = (angle_mirror & 0x1000) != 0;

        // Name and value come from the BoardPackage2 child (0x2f)
        let mut name = String::new();
        let mut value = String::new();
        for child in &bp.children {
            if child.record.sec_type() == SEC_BOARD_PACKAGE2 {
                name = resolve_string(&child.record, 2, 8, strings, string_idx);
                value = resolve_string(&child.record, 10, 14, strings, string_idx);
                break;
            }
        }

        placements.push(BinPlacement {
            name,
            value,
            x,
            y,
            angle,
            mirrored,
            lib_idx,
            pkg_idx,
        });
    }

    placements
}

// ─── Build footprints + components ──────────────────────────────────

fn build_footprints(
    packages: &[Vec<BinPackage>],
    placements: &[BinPlacement],
) -> (Vec<Footprint>, Vec<Component>) {
    let mut footprints = Vec::new();
    let mut components = Vec::new();

    for placement in placements {
        let lib_idx = placement.lib_idx as usize;
        let pkg_idx = placement.pkg_idx as usize;

        // Library and package indices are 1-based
        let package = packages
            .get(lib_idx.wrapping_sub(1))
            .and_then(|lib| lib.get(pkg_idx.wrapping_sub(1)));

        let side = if placement.mirrored { "B" } else { "F" };
        let angle = placement.angle;
        let x = placement.x;
        let y = placement.y;

        let mut fp_pads = Vec::new();
        let mut fp_drawings = Vec::new();

        if let Some(package) = package {
            for pad in &package.pads {
                let (px, py) = rotate_point(pad.x, pad.y, angle, placement.mirrored);
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

            for smd in &package.smds {
                let (px, py) = rotate_point(smd.x, smd.y, angle, placement.mirrored);
                let pad_side = if placement.mirrored {
                    mirror_layer_side(smd.layer)
                } else {
                    layer_side(smd.layer)
                };
                let shape = if smd.roundness > 0 {
                    "roundrect"
                } else {
                    "rect"
                };
                let roundness_frac = smd.roundness as f64 / 100.0;
                fp_pads.push(Pad {
                    layers: vec![pad_side.to_string()],
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
                    radius: if smd.roundness > 0 {
                        Some(roundness_frac * smd.dx.min(smd.dy) / 2.0)
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

            for wire in &package.wires {
                let effective_layer = if placement.mirrored {
                    mirror_eagle_layer(wire.layer)
                } else {
                    wire.layer
                };
                let cat = categorize_layer(effective_layer);
                let draw_side = match cat {
                    LayerCat::SilkF | LayerCat::FabF => "F",
                    LayerCat::SilkB | LayerCat::FabB => "B",
                    _ => continue,
                };
                let (sx, sy) = rotate_point(wire.x1, wire.y1, angle, placement.mirrored);
                let (ex, ey) = rotate_point(wire.x2, wire.y2, angle, placement.mirrored);
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
                let effective_layer = if placement.mirrored {
                    mirror_eagle_layer(circle.layer)
                } else {
                    circle.layer
                };
                let cat = categorize_layer(effective_layer);
                let draw_side = match cat {
                    LayerCat::SilkF | LayerCat::FabF => "F",
                    LayerCat::SilkB | LayerCat::FabB => "B",
                    _ => continue,
                };
                let (cx, cy) = rotate_point(circle.x, circle.y, angle, placement.mirrored);
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
                let effective_layer = if placement.mirrored {
                    mirror_eagle_layer(rect.layer)
                } else {
                    rect.layer
                };
                let cat = categorize_layer(effective_layer);
                let draw_side = match cat {
                    LayerCat::SilkF | LayerCat::FabF => "F",
                    LayerCat::SilkB | LayerCat::FabB => "B",
                    _ => continue,
                };
                let (sx, sy) = rotate_point(rect.x1, rect.y1, angle, placement.mirrored);
                let (ex, ey) = rotate_point(rect.x2, rect.y2, angle, placement.mirrored);
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
            ref_: placement.name.clone(),
            center: [x, -y],
            bbox: fp_bbox,
            pads: fp_pads,
            drawings: fp_drawings,
            layer: side.to_string(),
        });

        components.push(Component {
            ref_: placement.name.clone(),
            val: placement.value.clone(),
            footprint_name: String::new(),
            layer: if placement.mirrored {
                Side::Back
            } else {
                Side::Front
            },
            footprint_index: idx,
            extra_fields: HashMap::new(),
            attr: None,
        });
    }

    (footprints, components)
}

// ─── Board drawings extraction ──────────────────────────────────────

#[allow(clippy::type_complexity)]
fn extract_board_drawings(
    board: &Section,
    strings: &[String],
    string_idx: &mut usize,
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

    // Board-level drawables are direct children of the board section
    for child in &board.children {
        let drawing = match child.record.sec_type() {
            SEC_LINE => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x1 = units_to_mm(rec.i32_at(4));
                let y1 = units_to_mm(rec.i32_at(8));
                let x2 = units_to_mm(rec.i32_at(12));
                let y2 = units_to_mm(rec.i32_at(16));
                let width = units_to_mm(rec.u16_at(20) as i32 * 2);
                Some((
                    layer,
                    Drawing::Segment {
                        start: [x1, -y1],
                        end: [x2, -y2],
                        width,
                    },
                ))
            }
            SEC_CIRCLE => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x = units_to_mm(rec.i32_at(4));
                let y = units_to_mm(rec.i32_at(8));
                let radius = units_to_mm(rec.i32_at(12));
                let width = units_to_mm(rec.u16_at(16) as i32 * 2);
                Some((
                    layer,
                    Drawing::Circle {
                        start: [x, -y],
                        radius,
                        width,
                        filled: None,
                    },
                ))
            }
            SEC_RECT => {
                let rec = &child.record;
                let layer = rec.u8_at(3);
                let x1 = units_to_mm(rec.i32_at(4));
                let y1 = units_to_mm(rec.i32_at(8));
                let x2 = units_to_mm(rec.i32_at(12));
                let y2 = units_to_mm(rec.i32_at(16));
                Some((
                    layer,
                    Drawing::Rect {
                        start: [x1, -y1],
                        end: [x2, -y2],
                        width: 0.0,
                    },
                ))
            }
            SEC_HOLE => {
                let rec = &child.record;
                let x = units_to_mm(rec.i32_at(4));
                let y = units_to_mm(rec.i32_at(8));
                let drill = units_to_mm(rec.u16_at(12) as i32 * 2);
                Some((
                    20, // holes go to edge layer
                    Drawing::Circle {
                        start: [x, -y],
                        radius: drill / 2.0,
                        width: 0.0,
                        filled: None,
                    },
                ))
            }
            // Consume strings from text sections so the string index stays in sync
            SEC_TEXT => {
                let rec = &child.record;
                let _ = resolve_string(rec, 16, 8, strings, string_idx);
                None
            }
            _ => None,
        };

        if let Some((layer, d)) = drawing {
            match categorize_layer(layer) {
                LayerCat::Edge => edges.push(d),
                LayerCat::SilkF => silk_f.push(d),
                LayerCat::SilkB => silk_b.push(d),
                LayerCat::FabF => fab_f.push(d),
                LayerCat::FabB => fab_b.push(d),
                _ => {}
            }
        }
    }

    (edges, silk_f, silk_b, fab_f, fab_b)
}

// ─── Track extraction ───────────────────────────────────────────────

fn extract_tracks(
    board: &Section,
    _strings: &[String],
    _string_idx: &mut usize,
) -> (Vec<Track>, Vec<Track>) {
    let mut front = Vec::new();
    let mut back = Vec::new();

    let net_sections = find_all_sections(board, SEC_BOARD_NET);

    for net in &net_sections {
        for child in &net.children {
            match child.record.sec_type() {
                SEC_LINE => {
                    let rec = &child.record;
                    let layer = rec.u8_at(3);
                    let x1 = units_to_mm(rec.i32_at(4));
                    let y1 = units_to_mm(rec.i32_at(8));
                    let x2 = units_to_mm(rec.i32_at(12));
                    let y2 = units_to_mm(rec.i32_at(16));
                    let width = units_to_mm(rec.u16_at(20) as i32 * 2);
                    let track = Track::Segment {
                        start: [x1, -y1],
                        end: [x2, -y2],
                        width,
                        net: None,
                        drillsize: None,
                    };
                    match categorize_layer(layer) {
                        LayerCat::CopperF => front.push(track),
                        LayerCat::CopperB => back.push(track),
                        _ => {}
                    }
                }
                SEC_VIA => {
                    let rec = &child.record;
                    let x = units_to_mm(rec.i32_at(4));
                    let y = units_to_mm(rec.i32_at(8));
                    let drill = units_to_mm(rec.u16_at(12) as i32 * 2);
                    let diameter = units_to_mm(rec.u16_at(14) as i32 * 2);
                    let diameter = if diameter > 0.0 {
                        diameter
                    } else {
                        drill * 2.0
                    };
                    let via = Track::Segment {
                        start: [x, -y],
                        end: [x, -y],
                        width: diameter,
                        net: None,
                        drillsize: Some(drill),
                    };
                    front.push(via);
                    back.push(Track::Segment {
                        start: [x, -y],
                        end: [x, -y],
                        width: diameter,
                        net: None,
                        drillsize: Some(drill),
                    });
                }
                _ => {}
            }
        }
    }

    (front, back)
}

// ─── String resolution ──────────────────────────────────────────────

fn resolve_string(
    rec: &RawRecord,
    offset: usize,
    len: usize,
    strings: &[String],
    string_idx: &mut usize,
) -> String {
    match rec.inline_string(offset, len) {
        Some(s) => s,
        None => {
            // String overflows to the string table
            if *string_idx < strings.len() {
                let s = strings[*string_idx].clone();
                *string_idx += 1;
                s
            } else {
                String::new()
            }
        }
    }
}

// ─── Layer helpers ──────────────────────────────────────────────────

enum LayerCat {
    CopperF,
    CopperB,
    SilkF,
    SilkB,
    FabF,
    FabB,
    Edge,
    Other,
}

fn categorize_layer(layer: u8) -> LayerCat {
    match layer {
        1 => LayerCat::CopperF,
        16 => LayerCat::CopperB,
        20 => LayerCat::Edge,
        21 | 25 => LayerCat::SilkF,
        22 | 26 => LayerCat::SilkB,
        27 | 51 => LayerCat::FabF,
        28 | 52 => LayerCat::FabB,
        _ => LayerCat::Other,
    }
}

fn layer_side(layer: u8) -> &'static str {
    match layer {
        1 | 21 | 25 | 27 | 51 => "F",
        16 | 22 | 26 | 28 | 52 => "B",
        _ => "F",
    }
}

fn mirror_layer_side(layer: u8) -> &'static str {
    match layer {
        1 => "B",
        16 => "F",
        _ => layer_side(layer),
    }
}

fn mirror_eagle_layer(layer: u8) -> u8 {
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

// ─── Geometry helpers ───────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures")
            .join("eagle-binary")
            .join(name)
    }

    fn parse_fixture(name: &str) -> Result<PcbData, ExtractError> {
        let data = std::fs::read(fixture_path(name)).unwrap();
        parse(&data, &ExtractOptions::default())
    }

    #[test]
    fn test_is_eagle_binary() {
        let data = std::fs::read(fixture_path("grove-button.brd")).unwrap();
        assert!(is_eagle_binary(&data));
        assert!(!is_eagle_binary(b"<?xml"));
        assert!(!is_eagle_binary(&[]));
    }

    #[test]
    fn test_parse_grove_button() {
        let result = parse_fixture("grove-button.brd");
        assert!(
            result.is_ok(),
            "Failed to parse grove-button.brd: {:?}",
            result.err()
        );
        let pcb = result.unwrap();
        assert!(!pcb.footprints.is_empty(), "Expected footprints");

        // Verify we got reasonable data
        let has_pads = pcb.footprints.iter().any(|fp| !fp.pads.is_empty());
        assert!(has_pads, "Expected at least one footprint with pads");

        // Check bbox if edges were found
        if let Some(bb) = &pcb.edges_bbox {
            assert!(
                bb.maxx > bb.minx && bb.maxy > bb.miny,
                "Expected non-degenerate bounding box, got {:?}",
                bb
            );
        }
    }

    #[test]
    fn test_parse_grove_buzzer() {
        let result = parse_fixture("grove-buzzer.brd");
        assert!(
            result.is_ok(),
            "Failed to parse grove-buzzer.brd: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_electricity_sensor() {
        let result = parse_fixture("electricity-sensor.brd");
        assert!(
            result.is_ok(),
            "Failed to parse electricity-sensor.brd: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_parse_mma7660fc() {
        let result = parse_fixture("mma7660fc.brd");
        assert!(
            result.is_ok(),
            "Failed to parse mma7660fc.brd: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_xml_rejected() {
        let xml = b"<?xml version=\"1.0\" encoding=\"utf-8\"?><eagle></eagle>";
        assert!(!is_eagle_binary(xml));
    }
}
