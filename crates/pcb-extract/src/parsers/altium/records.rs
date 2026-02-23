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
    units_per_mil: i32,
) -> Vec<AltiumComponent> {
    records
        .iter()
        .filter(|r| {
            // Some files have RECORD=Component, others have no RECORD field at all
            // (all records in Components6/Data are components)
            r.get("RECORD")
                .map(|v| v.eq_ignore_ascii_case("Component"))
                .unwrap_or(true)
        })
        .map(|r| AltiumComponent {
            designator: r
                .get("SOURCEDESIGNATOR")
                .or_else(|| r.get("DESIGNATOR"))
                .cloned()
                .unwrap_or_default(),
            pattern: r.get("PATTERN").cloned().unwrap_or_default(),
            comment: r
                .get("COMMENT")
                .filter(|v| !v.is_empty())
                .or_else(|| r.get("SOURCEDESCRIPTION"))
                .cloned()
                .unwrap_or_default(),
            x: parse_coord(r, "X", units_per_mil),
            y: parse_coord(r, "Y", units_per_mil),
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
    for r in records.iter().filter(|r| {
        // Some files have RECORD=Net, others have no RECORD field at all
        r.get("RECORD")
            .map(|v| v.eq_ignore_ascii_case("Net"))
            .unwrap_or(true)
    }) {
        nets.push(AltiumNet {
            name: r.get("NAME").cloned().unwrap_or_default(),
        });
    }
    nets
}

fn parse_coord(record: &HashMap<String, String>, key: &str, units_per_mil: i32) -> i32 {
    record
        .get(key)
        .and_then(|v| parse_altium_value(v, units_per_mil))
        .unwrap_or(0)
}

/// Parse an Altium coordinate/dimension value.
/// Handles both raw internal units (integer) and "mil" suffix format.
/// For "mil" suffix: uses units_per_mil to convert (10000 for PCB 6.0, 1000 for older).
pub fn parse_altium_value(s: &str, units_per_mil: i32) -> Option<i32> {
    let trimmed = s.trim();
    if let Some(mil_str) = trimmed.strip_suffix("mil") {
        mil_str
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * units_per_mil as f64) as i32)
    } else {
        trimmed.parse::<f64>().ok().map(|v| v as i32)
    }
}

/// Check if text records use "mil" suffix on coordinates (indicates PCB 6.0 format).
pub fn detect_mil_format(records: &[HashMap<String, String>]) -> bool {
    records
        .iter()
        .any(|r| r.get("X").map(|v| v.ends_with("mil")).unwrap_or(false))
}

fn parse_layer_id(record: &HashMap<String, String>) -> u8 {
    // Try V7_LAYER first, then LAYER
    let val = record
        .get("V7_LAYER")
        .or_else(|| record.get("LAYER"))
        .map(|v| v.as_str())
        .unwrap_or("1");

    // Handle string layer names (e.g. "TOP", "BOTTOM")
    match val.to_uppercase().as_str() {
        "TOP" => return 1,
        "BOTTOM" => return 32,
        "TOPOVERLAY" => return 33,
        "BOTTOMOVERLAY" => return 34,
        "MULTILAYER" => return 74,
        _ => {}
    }

    // Numeric layer ID
    val.parse::<u32>()
        .ok()
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

pub fn parse_pads(data: &[u8], use_fine_scale: bool) -> Vec<AltiumPad> {
    if use_fine_scale {
        parse_pads_v6(data)
    } else {
        parse_pads_legacy(data)
    }
}

/// Parse pads from PCB 6.0 format where binary chunks use length-only prefixes
/// after the initial type+length subrecords.
fn parse_pads_v6(data: &[u8]) -> Vec<AltiumPad> {
    let mut pads = Vec::new();
    let mut offset = 0;

    while offset + 12 < data.len() {
        // Sub-record A (type+len): pad name
        if offset + 5 > data.len() {
            break;
        }
        let _sr_type_a = data[offset];
        offset += 1;
        let sr_len_a = read_u32_le(data, offset) as usize;
        offset += 4;
        if offset + sr_len_a > data.len() {
            break;
        }
        let name = if sr_len_a > 1 {
            let name_len = data[offset] as usize;
            String::from_utf8_lossy(&data[offset + 1..offset + 1 + name_len.min(sr_len_a - 1)])
                .to_string()
        } else if sr_len_a == 1 {
            String::from_utf8_lossy(&data[offset..offset + 1]).to_string()
        } else {
            String::new()
        };
        offset += sr_len_a;

        // Sub-record B (type+len): flags/empty
        if offset + 5 > data.len() {
            break;
        }
        offset += 1; // type
        let sr_len_b = read_u32_le(data, offset) as usize;
        offset += 4;
        if offset + sr_len_b > data.len() {
            break;
        }
        offset += sr_len_b;

        // Length-prefixed binary chunks (no type byte)
        let mut geometry: Option<&[u8]> = None;
        loop {
            if offset + 4 > data.len() {
                break;
            }
            let chunk_len = read_u32_le(data, offset) as usize;
            if chunk_len > 100_000 {
                break;
            }
            offset += 4;
            if offset + chunk_len > data.len() {
                break;
            }
            // The largest chunk (typically ~200 bytes) is the pad geometry
            if chunk_len >= 60 {
                geometry = Some(&data[offset..offset + chunk_len]);
            }
            offset += chunk_len;
            // Next pad starts with type byte 0x02
            if offset < data.len() && data[offset] == 0x02 {
                break;
            }
        }

        if let Some(geom) = geometry {
            if geom.len() >= 60 {
                pads.push(AltiumPad {
                    name,
                    layer: read_u8(geom, 0),
                    net_id: read_u16(geom, 3),
                    component_id: read_u16(geom, 7),
                    x: read_i32(geom, 13),
                    y: read_i32(geom, 17),
                    size_x: read_i32(geom, 21),
                    size_y: read_i32(geom, 25),
                    hole_size: read_i32(geom, 45),
                    shape: read_u8(geom, 49),
                    rotation: read_f64(geom, 52),
                });
            }
        }
    }

    pads
}

/// Parse pads from older Altium format using type+length subrecords throughout.
fn parse_pads_legacy(data: &[u8]) -> Vec<AltiumPad> {
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
            let next_tag = all_subrecords[i].0;
            if next_tag != all_subrecords[0].0 {
                i += 1;
            }
        }

        pads.push(pad);
    }

    pads
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
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

pub fn parse_texts(data: &[u8], use_fine_scale: bool) -> Vec<AltiumText> {
    if use_fine_scale {
        parse_texts_v6(data)
    } else {
        parse_texts_legacy(data)
    }
}

/// Parse texts from PCB 6.0 format with chunk-based sub-records.
fn parse_texts_v6(data: &[u8]) -> Vec<AltiumText> {
    let mut texts = Vec::new();
    let mut offset = 0;

    while offset + 12 < data.len() {
        // Sub-record A (type+len): text content
        if offset + 5 > data.len() {
            break;
        }
        offset += 1; // type
        let sr_len_a = read_u32_le(data, offset) as usize;
        offset += 4;
        if offset + sr_len_a > data.len() {
            break;
        }
        let text_str = String::from_utf8_lossy(&data[offset..offset + sr_len_a])
            .trim_end_matches('\0')
            .to_string();
        offset += sr_len_a;

        // Length-prefixed binary chunks (no type byte)
        let mut geometry: Option<&[u8]> = None;
        loop {
            if offset + 4 > data.len() {
                break;
            }
            let chunk_len = read_u32_le(data, offset) as usize;
            if chunk_len > 100_000 {
                break;
            }
            offset += 4;
            if offset + chunk_len > data.len() {
                break;
            }
            if chunk_len >= 35 {
                geometry = Some(&data[offset..offset + chunk_len]);
            }
            offset += chunk_len;
            // Next text starts with a type byte — check for typical text type bytes
            if offset < data.len() && (data[offset] == 0x05 || data[offset] == 0x04) {
                break;
            }
        }

        if let Some(geom) = geometry {
            if geom.len() >= 35 {
                let layer = read_u8(geom, 0);
                let component_id = read_u16(geom, 7);
                let x = read_i32(geom, 13);
                let y = read_i32(geom, 17);
                let height = read_i32(geom, 21);
                let rotation = read_f64(geom, 27);

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
        }
    }

    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut r = HashMap::new();
        for (k, v) in pairs {
            r.insert(k.to_string(), v.to_string());
        }
        r
    }

    #[test]
    fn test_comment_used_when_present() {
        let record = make_record(&[
            ("SOURCEDESIGNATOR", "R1"),
            ("PATTERN", "0402"),
            ("COMMENT", "10k"),
            ("SOURCEDESCRIPTION", "Thick Film Resistors - SMD 10 kOhm 1%"),
            ("LAYER", "TOP"),
            ("X", "0"),
            ("Y", "0"),
            ("ROTATION", "0"),
        ]);
        let components = parse_components(&[record], &HashMap::new(), 10000);
        assert_eq!(components[0].comment, "10k");
    }

    #[test]
    fn test_sourcedescription_fallback_when_comment_absent() {
        let record = make_record(&[
            ("SOURCEDESIGNATOR", "R1"),
            ("PATTERN", "0402"),
            ("SOURCEDESCRIPTION", "Thick Film Resistors - SMD 10 kOhm 1%"),
            ("LAYER", "TOP"),
            ("X", "0"),
            ("Y", "0"),
            ("ROTATION", "0"),
        ]);
        let components = parse_components(&[record], &HashMap::new(), 10000);
        assert_eq!(
            components[0].comment,
            "Thick Film Resistors - SMD 10 kOhm 1%"
        );
    }

    #[test]
    fn test_empty_comment_falls_back_to_sourcedescription() {
        let record = make_record(&[
            ("SOURCEDESIGNATOR", "C1"),
            ("PATTERN", "0402"),
            ("COMMENT", ""),
            ("SOURCEDESCRIPTION", "Generic Capacitor, 100nF"),
            ("LAYER", "TOP"),
            ("X", "0"),
            ("Y", "0"),
            ("ROTATION", "0"),
        ]);
        let components = parse_components(&[record], &HashMap::new(), 10000);
        assert_eq!(components[0].comment, "Generic Capacitor, 100nF");
    }
}

/// Parse texts from older Altium format using type+length subrecords.
fn parse_texts_legacy(data: &[u8]) -> Vec<AltiumText> {
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
