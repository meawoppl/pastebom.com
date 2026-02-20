use std::collections::HashMap;

// ─── Parsed record types ─────────────────────────────────────────────

#[derive(Debug)]
pub struct AltiumComponent {
    pub designator: String,
    pub pattern: String,
    pub comment: String,
    pub x: i32,
    pub y: i32,
    pub rotation: f64,
    pub layer: u8,
}

#[derive(Debug)]
pub struct AltiumNet {
    pub name: String,
}

#[derive(Debug)]
pub struct AltiumPad {
    pub name: String,
    pub layer: u8,
    pub net_id: u16,
    pub component_id: u16,
    pub x: i32,
    pub y: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub hole_size: i32,
    pub shape: u8,
    pub rotation: f64,
}

#[derive(Debug)]
pub struct AltiumTrack {
    pub layer: u8,
    pub net_id: u16,
    pub component_id: u16,
    pub start_x: i32,
    pub start_y: i32,
    pub end_x: i32,
    pub end_y: i32,
    pub width: i32,
}

#[derive(Debug)]
pub struct AltiumArc {
    pub layer: u8,
    pub net_id: u16,
    pub component_id: u16,
    pub center_x: i32,
    pub center_y: i32,
    pub radius: i32,
    pub start_angle: f64,
    pub end_angle: f64,
    pub width: i32,
}

#[derive(Debug)]
pub struct AltiumVia {
    pub net_id: u16,
    pub x: i32,
    pub y: i32,
    pub diameter: i32,
    pub hole_size: i32,
}

#[derive(Debug)]
pub struct AltiumFill {
    pub layer: u8,
    pub component_id: u16,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

// ─── Text property record parsers ────────────────────────────────────

pub fn parse_components(
    records: &[HashMap<String, String>],
    _wide_strings: &HashMap<u32, String>,
) -> Vec<AltiumComponent> {
    records
        .iter()
        .filter(|r| r.get("RECORD").map(|v| v.as_str()) == Some("Component"))
        .map(|r| AltiumComponent {
            designator: r
                .get("SOURCEDESIGNATOR")
                .or_else(|| r.get("DESIGNATOR"))
                .cloned()
                .unwrap_or_default(),
            pattern: r.get("PATTERN").cloned().unwrap_or_default(),
            comment: r.get("COMMENT").cloned().unwrap_or_default(),
            x: parse_coord(r, "X"),
            y: parse_coord(r, "Y"),
            rotation: r
                .get("ROTATION")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            layer: parse_layer_id(r),
        })
        .collect()
}

pub fn parse_nets(records: &[HashMap<String, String>]) -> Vec<AltiumNet> {
    // Net records include index 0 as empty
    let mut nets = vec![AltiumNet {
        name: String::new(),
    }];
    for r in records
        .iter()
        .filter(|r| r.get("RECORD").map(|v| v.as_str()) == Some("Net"))
    {
        nets.push(AltiumNet {
            name: r.get("NAME").cloned().unwrap_or_default(),
        });
    }
    nets
}

fn parse_coord(record: &HashMap<String, String>, key: &str) -> i32 {
    record
        .get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as i32)
        .unwrap_or(0)
}

fn parse_layer_id(record: &HashMap<String, String>) -> u8 {
    // Try V7_LAYER first, then LAYER
    record
        .get("V7_LAYER")
        .or_else(|| record.get("LAYER"))
        .and_then(|v| v.parse::<u32>().ok())
        .map(|v| {
            // V7 layer IDs have base 0x01000000
            if v > 0x01000000 {
                (v & 0xFF) as u8
            } else {
                v as u8
            }
        })
        .unwrap_or(1)
}

// ─── Binary record parsers ───────────────────────────────────────────

fn read_u8(data: &[u8], offset: usize) -> u8 {
    data.get(offset).copied().unwrap_or(0)
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() {
        return 0;
    }
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    if offset + 4 > data.len() {
        return 0;
    }
    i32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_f64(data: &[u8], offset: usize) -> f64 {
    if offset + 8 > data.len() {
        return 0.0;
    }
    f64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Parse binary subrecords from a stream.
/// Returns Vec of (record_type_tag, subrecord_data).
fn parse_subrecords(data: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset + 5 <= data.len() {
        let record_type = data[offset];
        offset += 1;
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
        records.push((record_type, data[offset..offset + len].to_vec()));
        offset += len;
    }
    records
}

pub fn parse_tracks(data: &[u8]) -> Vec<AltiumTrack> {
    let subrecords = parse_subrecords(data);
    subrecords
        .into_iter()
        .filter_map(|(_tag, sr)| {
            if sr.len() < 33 {
                return None;
            }
            Some(AltiumTrack {
                layer: read_u8(&sr, 0),
                net_id: read_u16(&sr, 3),
                component_id: read_u16(&sr, 7),
                start_x: read_i32(&sr, 13),
                start_y: read_i32(&sr, 17),
                end_x: read_i32(&sr, 21),
                end_y: read_i32(&sr, 25),
                width: read_i32(&sr, 29),
            })
        })
        .collect()
}

pub fn parse_arcs(data: &[u8]) -> Vec<AltiumArc> {
    let subrecords = parse_subrecords(data);
    subrecords
        .into_iter()
        .filter_map(|(_tag, sr)| {
            if sr.len() < 45 {
                return None;
            }
            Some(AltiumArc {
                layer: read_u8(&sr, 0),
                net_id: read_u16(&sr, 3),
                component_id: read_u16(&sr, 7),
                center_x: read_i32(&sr, 13),
                center_y: read_i32(&sr, 17),
                radius: read_i32(&sr, 21),
                start_angle: read_f64(&sr, 25),
                end_angle: read_f64(&sr, 33),
                width: read_i32(&sr, 41),
            })
        })
        .collect()
}

pub fn parse_vias(data: &[u8]) -> Vec<AltiumVia> {
    let subrecords = parse_subrecords(data);
    subrecords
        .into_iter()
        .filter_map(|(_tag, sr)| {
            if sr.len() < 29 {
                return None;
            }
            Some(AltiumVia {
                net_id: read_u16(&sr, 3),
                x: read_i32(&sr, 13),
                y: read_i32(&sr, 17),
                diameter: read_i32(&sr, 21),
                hole_size: read_i32(&sr, 25),
            })
        })
        .collect()
}

pub fn parse_fills(data: &[u8]) -> Vec<AltiumFill> {
    let subrecords = parse_subrecords(data);
    subrecords
        .into_iter()
        .filter_map(|(_tag, sr)| {
            if sr.len() < 29 {
                return None;
            }
            Some(AltiumFill {
                layer: read_u8(&sr, 0),
                component_id: read_u16(&sr, 7),
                x1: read_i32(&sr, 13),
                y1: read_i32(&sr, 17),
                x2: read_i32(&sr, 21),
                y2: read_i32(&sr, 25),
            })
        })
        .collect()
}

pub fn parse_pads(data: &[u8]) -> Vec<AltiumPad> {
    // Pads have multiple subrecords per pad:
    // Subrecord 0: pad name (variable-length string)
    // Subrecord 1: pad geometry
    // Subrecord 2: optional size-and-shape
    let all_subrecords = parse_subrecords(data);

    let mut pads = Vec::new();
    let mut i = 0;
    while i < all_subrecords.len() {
        // Subrecord 0: pad name
        let name = if i < all_subrecords.len() {
            let name_data = &all_subrecords[i].1;
            String::from_utf8_lossy(name_data)
                .trim_end_matches('\0')
                .to_string()
        } else {
            String::new()
        };
        i += 1;

        // Subrecord 1: pad geometry
        if i >= all_subrecords.len() {
            break;
        }
        let geom = &all_subrecords[i].1;
        i += 1;

        if geom.len() < 70 {
            // Skip optional subrecord 2 if present
            if i < all_subrecords.len() && all_subrecords[i].1.len() < 33 {
                i += 1;
            }
            continue;
        }

        let pad = AltiumPad {
            name,
            layer: read_u8(geom, 0),
            net_id: read_u16(geom, 7),
            component_id: read_u16(geom, 13),
            x: read_i32(geom, 23),
            y: read_i32(geom, 27),
            size_x: read_i32(geom, 31),
            size_y: read_i32(geom, 35),
            hole_size: read_i32(geom, 55),
            shape: read_u8(geom, 59),
            rotation: read_f64(geom, 62),
        };

        // Skip optional subrecord 2
        if i < all_subrecords.len() {
            // Heuristic: subrecord 2 is present if the next subrecord
            // doesn't look like a pad name (i.e., its type tag differs)
            let next_tag = all_subrecords[i].0;
            if next_tag != all_subrecords[0].0 {
                i += 1;
            }
        }

        pads.push(pad);
    }

    pads
}

// ─── Text records ───────────────────────────────────────────────────

#[derive(Debug)]
pub struct AltiumText {
    pub layer: u8,
    pub component_id: u16,
    pub x: i32,
    pub y: i32,
    pub height: i32,
    pub rotation: f64,
    pub text: String,
    pub is_designator: bool,
    pub is_comment: bool,
}

pub fn parse_texts(data: &[u8]) -> Vec<AltiumText> {
    // Texts6 stream contains paired subrecords:
    // Subrecord 0: text string (variable-length)
    // Subrecord 1: text geometry (binary)
    let all_subrecords = parse_subrecords(data);

    let mut texts = Vec::new();
    let mut i = 0;
    while i + 1 < all_subrecords.len() {
        let text_data = &all_subrecords[i].1;
        let geom = &all_subrecords[i + 1].1;
        i += 2;

        let text_str = String::from_utf8_lossy(text_data)
            .trim_end_matches('\0')
            .to_string();

        if geom.len() < 41 {
            continue;
        }

        let layer = read_u8(geom, 0);
        let component_id = read_u16(geom, 7);
        let x = read_i32(geom, 13);
        let y = read_i32(geom, 17);
        let height = read_i32(geom, 21);
        let rotation = read_f64(geom, 27);

        // Detect designator/comment from text content
        let is_designator = text_str == ".Designator";
        let is_comment = text_str == ".Comment";

        texts.push(AltiumText {
            layer,
            component_id,
            x,
            y,
            height,
            rotation,
            text: text_str,
            is_designator,
            is_comment,
        });
    }

    texts
}
