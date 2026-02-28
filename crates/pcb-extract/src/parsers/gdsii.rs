use std::collections::{HashMap, HashSet};

use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;

// GDSII record types
const HEADER: u8 = 0x00;
const LIBNAME: u8 = 0x02;
const UNITS: u8 = 0x03;
const BGNSTR: u8 = 0x05;
const STRNAME: u8 = 0x06;
const ENDSTR: u8 = 0x07;
const BOUNDARY: u8 = 0x08;
const PATH: u8 = 0x09;
const SREF: u8 = 0x0A;
const AREF: u8 = 0x0B;
const TEXT: u8 = 0x0C;
const LAYER: u8 = 0x0D;
const DATATYPE: u8 = 0x0E;
const WIDTH: u8 = 0x0F;
const XY: u8 = 0x10;
const ENDEL: u8 = 0x11;
const SNAME: u8 = 0x12;
const COLROW: u8 = 0x13;
const TEXTTYPE: u8 = 0x16;
const STRING: u8 = 0x19;
const STRANS: u8 = 0x1A;
const MAG: u8 = 0x1B;
const ANGLE: u8 = 0x1C;
const PATHTYPE: u8 = 0x21;

// GDSII data types
const DT_NONE: u8 = 0x00;
const DT_BITARRAY: u8 = 0x01;
const DT_I16: u8 = 0x02;
const DT_I32: u8 = 0x03;
const DT_F64: u8 = 0x05;
const DT_ASCII: u8 = 0x06;

/// A parsed GDSII record.
struct Record {
    record_type: u8,
    data: RecordData,
}

/// Data payload of a GDSII record.
enum RecordData {
    None,
    BitArray(Vec<u16>),
    Int16(Vec<i16>),
    Int32(Vec<i32>),
    Float64(Vec<f64>),
    Ascii(String),
}

/// A GDSII element within a structure.
enum GdsElement {
    Boundary {
        layer: i16,
        xy: Vec<(i32, i32)>,
    },
    Path {
        layer: i16,
        width: i32,
        xy: Vec<(i32, i32)>,
    },
    SRef {
        sname: String,
        xy: (i32, i32),
        strans: u16,
        mag: f64,
        angle: f64,
    },
    ARef {
        sname: String,
        xy: Vec<(i32, i32)>,
        cols: i16,
        rows: i16,
        strans: u16,
        mag: f64,
        angle: f64,
    },
    Text {
        layer: i16,
        xy: (i32, i32),
        text: String,
    },
}

/// A GDSII structure (cell).
struct GdsStructure {
    name: String,
    elements: Vec<GdsElement>,
}

/// Convert GDSII excess-64 (IBM) float to IEEE 754 f64.
///
/// Format: 1 sign bit, 7 exponent bits (biased by 64), 56 mantissa bits.
/// value = (-1)^sign * (mantissa / 2^56) * 16^(exponent - 64)
fn gds_float_to_f64(bytes: &[u8]) -> f64 {
    if bytes.len() != 8 {
        return 0.0;
    }
    let sign = (bytes[0] >> 7) & 1;
    let exponent = (bytes[0] & 0x7F) as i32;
    let mut mantissa: u64 = 0;
    for &b in &bytes[1..8] {
        mantissa = (mantissa << 8) | (b as u64);
    }
    if mantissa == 0 {
        return 0.0;
    }
    let value = (mantissa as f64 / (1u64 << 56) as f64) * 16f64.powi(exponent - 64);
    if sign == 1 {
        -value
    } else {
        value
    }
}

/// Read a big-endian u16 from a byte slice.
fn read_u16(data: &[u8], offset: usize) -> Result<u16, ExtractError> {
    if offset + 2 > data.len() {
        return Err(ExtractError::ParseError(
            "GDSII: unexpected end of data reading u16".into(),
        ));
    }
    Ok(u16::from_be_bytes([data[offset], data[offset + 1]]))
}

/// Parse all records from a GDSII byte stream.
fn parse_records(data: &[u8]) -> Result<Vec<Record>, ExtractError> {
    let mut records = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        if offset + 4 > data.len() {
            break;
        }

        let length = read_u16(data, offset)? as usize;
        if length < 4 {
            return Err(ExtractError::ParseError(format!(
                "GDSII: invalid record length {} at offset {}",
                length, offset
            )));
        }
        if offset + length > data.len() {
            return Err(ExtractError::ParseError(format!(
                "GDSII: record at offset {} extends past end of data (length {})",
                offset, length
            )));
        }

        let record_type = data[offset + 2];
        let data_type = data[offset + 3];
        let payload = &data[offset + 4..offset + length];

        let record_data = parse_record_data(data_type, payload)?;
        records.push(Record {
            record_type,
            data: record_data,
        });

        offset += length;
    }

    Ok(records)
}

/// Parse the data payload of a record based on data type.
fn parse_record_data(data_type: u8, payload: &[u8]) -> Result<RecordData, ExtractError> {
    match data_type {
        DT_NONE => Ok(RecordData::None),
        DT_BITARRAY => {
            let mut vals = Vec::new();
            let mut i = 0;
            while i + 1 < payload.len() {
                vals.push(u16::from_be_bytes([payload[i], payload[i + 1]]));
                i += 2;
            }
            Ok(RecordData::BitArray(vals))
        }
        DT_I16 => {
            let mut vals = Vec::new();
            let mut i = 0;
            while i + 1 < payload.len() {
                vals.push(i16::from_be_bytes([payload[i], payload[i + 1]]));
                i += 2;
            }
            Ok(RecordData::Int16(vals))
        }
        DT_I32 => {
            let mut vals = Vec::new();
            let mut i = 0;
            while i + 3 < payload.len() {
                vals.push(i32::from_be_bytes([
                    payload[i],
                    payload[i + 1],
                    payload[i + 2],
                    payload[i + 3],
                ]));
                i += 4;
            }
            Ok(RecordData::Int32(vals))
        }
        DT_F64 => {
            let mut vals = Vec::new();
            let mut i = 0;
            while i + 7 < payload.len() {
                vals.push(gds_float_to_f64(&payload[i..i + 8]));
                i += 8;
            }
            Ok(RecordData::Float64(vals))
        }
        DT_ASCII => {
            let mut s = String::from_utf8_lossy(payload).to_string();
            // GDSII strings are padded with null bytes
            if let Some(pos) = s.find('\0') {
                s.truncate(pos);
            }
            Ok(RecordData::Ascii(s))
        }
        _ => Ok(RecordData::None),
    }
}

/// Extract i16 values from record data.
fn get_i16(data: &RecordData) -> Vec<i16> {
    match data {
        RecordData::Int16(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract i32 values from record data.
fn get_i32(data: &RecordData) -> Vec<i32> {
    match data {
        RecordData::Int32(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract f64 values from record data.
fn get_f64(data: &RecordData) -> Vec<f64> {
    match data {
        RecordData::Float64(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract ASCII string from record data.
fn get_ascii(data: &RecordData) -> String {
    match data {
        RecordData::Ascii(s) => s.clone(),
        _ => String::new(),
    }
}

/// Extract bitarray values from record data.
fn get_bitarray(data: &RecordData) -> Vec<u16> {
    match data {
        RecordData::BitArray(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract XY coordinate pairs from a record data (i32 pairs).
fn get_xy_pairs(data: &RecordData) -> Vec<(i32, i32)> {
    let vals = get_i32(data);
    vals.chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                Some((chunk[0], chunk[1]))
            } else {
                None
            }
        })
        .collect()
}

/// Parse structures from records.
fn parse_structures(records: &[Record]) -> Result<Vec<GdsStructure>, ExtractError> {
    let mut structures = Vec::new();
    let mut i = 0;

    while i < records.len() {
        if records[i].record_type == BGNSTR {
            i += 1;
            // Next record should be STRNAME
            if i >= records.len() || records[i].record_type != STRNAME {
                return Err(ExtractError::ParseError(
                    "GDSII: expected STRNAME after BGNSTR".into(),
                ));
            }
            let name = get_ascii(&records[i].data);
            i += 1;

            let mut elements = Vec::new();

            // Parse elements until ENDSTR
            while i < records.len() && records[i].record_type != ENDSTR {
                match records[i].record_type {
                    BOUNDARY => {
                        let (elem, new_i) = parse_boundary(records, i)?;
                        elements.push(elem);
                        i = new_i;
                    }
                    PATH => {
                        let (elem, new_i) = parse_path(records, i)?;
                        elements.push(elem);
                        i = new_i;
                    }
                    SREF => {
                        let (elem, new_i) = parse_sref(records, i)?;
                        elements.push(elem);
                        i = new_i;
                    }
                    AREF => {
                        let (elem, new_i) = parse_aref(records, i)?;
                        elements.push(elem);
                        i = new_i;
                    }
                    TEXT => {
                        let (elem, new_i) = parse_text(records, i)?;
                        elements.push(elem);
                        i = new_i;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
            // Skip ENDSTR
            if i < records.len() && records[i].record_type == ENDSTR {
                i += 1;
            }

            structures.push(GdsStructure { name, elements });
        } else {
            i += 1;
        }
    }

    Ok(structures)
}

/// Parse a BOUNDARY element starting at index i.
fn parse_boundary(records: &[Record], start: usize) -> Result<(GdsElement, usize), ExtractError> {
    let mut i = start + 1; // skip BOUNDARY record
    let mut layer: i16 = 0;
    let mut xy = Vec::new();

    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            LAYER => {
                let vals = get_i16(&records[i].data);
                if !vals.is_empty() {
                    layer = vals[0];
                }
            }
            DATATYPE => {}
            XY => {
                xy = get_xy_pairs(&records[i].data);
            }
            _ => {}
        }
        i += 1;
    }
    // Skip ENDEL
    if i < records.len() && records[i].record_type == ENDEL {
        i += 1;
    }

    Ok((GdsElement::Boundary { layer, xy }, i))
}

/// Parse a PATH element starting at index i.
fn parse_path(records: &[Record], start: usize) -> Result<(GdsElement, usize), ExtractError> {
    let mut i = start + 1; // skip PATH record
    let mut layer: i16 = 0;
    let mut width: i32 = 0;
    let mut xy = Vec::new();

    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            LAYER => {
                let vals = get_i16(&records[i].data);
                if !vals.is_empty() {
                    layer = vals[0];
                }
            }
            DATATYPE | PATHTYPE => {}
            WIDTH => {
                let vals = get_i32(&records[i].data);
                if !vals.is_empty() {
                    width = vals[0];
                }
            }
            XY => {
                xy = get_xy_pairs(&records[i].data);
            }
            _ => {}
        }
        i += 1;
    }
    // Skip ENDEL
    if i < records.len() && records[i].record_type == ENDEL {
        i += 1;
    }

    Ok((GdsElement::Path { layer, width, xy }, i))
}

/// Parse an SREF element starting at index i.
fn parse_sref(records: &[Record], start: usize) -> Result<(GdsElement, usize), ExtractError> {
    let mut i = start + 1;
    let mut sname = String::new();
    let mut xy = (0i32, 0i32);
    let mut strans: u16 = 0;
    let mut mag = 1.0;
    let mut angle = 0.0;

    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            SNAME => {
                sname = get_ascii(&records[i].data);
            }
            XY => {
                let pairs = get_xy_pairs(&records[i].data);
                if !pairs.is_empty() {
                    xy = pairs[0];
                }
            }
            STRANS => {
                let vals = get_bitarray(&records[i].data);
                if !vals.is_empty() {
                    strans = vals[0];
                }
            }
            MAG => {
                let vals = get_f64(&records[i].data);
                if !vals.is_empty() {
                    mag = vals[0];
                }
            }
            ANGLE => {
                let vals = get_f64(&records[i].data);
                if !vals.is_empty() {
                    angle = vals[0];
                }
            }
            _ => {}
        }
        i += 1;
    }
    if i < records.len() && records[i].record_type == ENDEL {
        i += 1;
    }

    Ok((
        GdsElement::SRef {
            sname,
            xy,
            strans,
            mag,
            angle,
        },
        i,
    ))
}

/// Parse an AREF element starting at index i.
fn parse_aref(records: &[Record], start: usize) -> Result<(GdsElement, usize), ExtractError> {
    let mut i = start + 1;
    let mut sname = String::new();
    let mut xy = Vec::new();
    let mut cols: i16 = 1;
    let mut rows: i16 = 1;
    let mut strans: u16 = 0;
    let mut mag = 1.0;
    let mut angle = 0.0;

    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            SNAME => {
                sname = get_ascii(&records[i].data);
            }
            COLROW => {
                let vals = get_i16(&records[i].data);
                if vals.len() >= 2 {
                    cols = vals[0];
                    rows = vals[1];
                }
            }
            XY => {
                xy = get_xy_pairs(&records[i].data);
            }
            STRANS => {
                let vals = get_bitarray(&records[i].data);
                if !vals.is_empty() {
                    strans = vals[0];
                }
            }
            MAG => {
                let vals = get_f64(&records[i].data);
                if !vals.is_empty() {
                    mag = vals[0];
                }
            }
            ANGLE => {
                let vals = get_f64(&records[i].data);
                if !vals.is_empty() {
                    angle = vals[0];
                }
            }
            _ => {}
        }
        i += 1;
    }
    if i < records.len() && records[i].record_type == ENDEL {
        i += 1;
    }

    Ok((
        GdsElement::ARef {
            sname,
            xy,
            cols,
            rows,
            strans,
            mag,
            angle,
        },
        i,
    ))
}

/// Parse a TEXT element starting at index i.
fn parse_text(records: &[Record], start: usize) -> Result<(GdsElement, usize), ExtractError> {
    let mut i = start + 1;
    let mut layer: i16 = 0;
    let mut xy = (0i32, 0i32);
    let mut text = String::new();

    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            LAYER => {
                let vals = get_i16(&records[i].data);
                if !vals.is_empty() {
                    layer = vals[0];
                }
            }
            TEXTTYPE => {}
            XY => {
                let pairs = get_xy_pairs(&records[i].data);
                if !pairs.is_empty() {
                    xy = pairs[0];
                }
            }
            STRING => {
                text = get_ascii(&records[i].data);
            }
            STRANS | MAG | ANGLE => {}
            _ => {}
        }
        i += 1;
    }
    if i < records.len() && records[i].record_type == ENDEL {
        i += 1;
    }

    Ok((GdsElement::Text { layer, xy, text }, i))
}

/// Extract UNITS record values (user_units_per_db_unit, meters_per_db_unit).
fn extract_units(records: &[Record]) -> (f64, f64) {
    for rec in records {
        if rec.record_type == UNITS {
            let vals = get_f64(&rec.data);
            if vals.len() >= 2 {
                return (vals[0], vals[1]);
            }
        }
    }
    // Default: 1nm database units
    (0.001, 1e-9)
}

/// Extract library name from records.
fn extract_libname(records: &[Record]) -> String {
    for rec in records {
        if rec.record_type == LIBNAME {
            return get_ascii(&rec.data);
        }
    }
    String::new()
}

/// Map a GDSII layer number to a layer name string.
/// GDSII layers are arbitrary; we use a simple mapping convention.
fn layer_name(layer: i16) -> String {
    match layer {
        0 => "F".to_string(),
        1 => "B".to_string(),
        n if (2..=31).contains(&n) => format!("In{}", n),
        _ => format!("L{}", layer),
    }
}

/// Determine which side a layer is on.
fn layer_side(layer: i16) -> &'static str {
    match layer {
        0 => "F",
        1 => "B",
        _ => "F",
    }
}

/// Find the top-level structure: the one not referenced by any SREF/AREF.
fn find_top_structure(structures: &[GdsStructure]) -> Option<usize> {
    if structures.is_empty() {
        return None;
    }

    let mut referenced: HashSet<&str> = HashSet::new();
    for s in structures {
        for elem in &s.elements {
            match elem {
                GdsElement::SRef { sname, .. } | GdsElement::ARef { sname, .. } => {
                    referenced.insert(sname.as_str());
                }
                _ => {}
            }
        }
    }

    // Find first structure not referenced by others (prefer last defined)
    for (i, s) in structures.iter().enumerate().rev() {
        if !referenced.contains(s.name.as_str()) {
            return Some(i);
        }
    }

    // Fallback: last structure
    Some(structures.len() - 1)
}

/// Convert database units (i32) to millimeters.
fn db_to_mm(val: i32, scale: f64) -> f64 {
    val as f64 * scale
}

/// Convert a coordinate pair to mm, negating Y for the coordinate system.
fn xy_to_mm(x: i32, y: i32, scale: f64) -> [f64; 2] {
    [db_to_mm(x, scale), -db_to_mm(y, scale)]
}

/// Transform a point by SREF/AREF parameters (mirror, magnify, rotate, translate).
fn transform_point(
    pt: [f64; 2],
    origin: [f64; 2],
    mirror_x: bool,
    mag: f64,
    angle_deg: f64,
) -> [f64; 2] {
    let mut x = pt[0];
    let mut y = pt[1];

    // Mirror about X axis (flips Y before rotation)
    if mirror_x {
        y = -y;
    }

    // Scale
    x *= mag;
    y *= mag;

    // Rotate
    if angle_deg != 0.0 {
        let rad = angle_deg.to_radians();
        let cos_a = rad.cos();
        let sin_a = rad.sin();
        let rx = x * cos_a - y * sin_a;
        let ry = x * sin_a + y * cos_a;
        x = rx;
        y = ry;
    }

    // Translate
    [x + origin[0], y + origin[1]]
}

/// Accumulator for flattened geometry from GDSII structures.
struct FlattenOutput {
    boundaries: Vec<(i16, Vec<[f64; 2]>)>,
    paths: Vec<(i16, i32, Vec<[f64; 2]>)>,
    texts: Vec<(i16, [f64; 2], String)>,
}

/// Flatten structure elements into geometry, resolving SREF/AREF recursively.
#[allow(clippy::too_many_arguments)]
fn flatten_structure(
    idx: usize,
    structures: &[GdsStructure],
    struct_map: &HashMap<&str, usize>,
    scale: f64,
    origin: [f64; 2],
    mirror_x: bool,
    mag: f64,
    angle_deg: f64,
    depth: usize,
    out: &mut FlattenOutput,
) {
    if depth > 64 {
        return; // prevent infinite recursion
    }

    let structure = &structures[idx];

    for elem in &structure.elements {
        match elem {
            GdsElement::Boundary { layer, xy } => {
                let pts: Vec<[f64; 2]> = xy
                    .iter()
                    .map(|&(x, y)| {
                        let pt = xy_to_mm(x, y, scale);
                        transform_point(pt, origin, mirror_x, mag, angle_deg)
                    })
                    .collect();
                out.boundaries.push((*layer, pts));
            }
            GdsElement::Path {
                layer, width, xy, ..
            } => {
                let pts: Vec<[f64; 2]> = xy
                    .iter()
                    .map(|&(x, y)| {
                        let pt = xy_to_mm(x, y, scale);
                        transform_point(pt, origin, mirror_x, mag, angle_deg)
                    })
                    .collect();
                out.paths.push((*layer, *width, pts));
            }
            GdsElement::Text { layer, xy, text } => {
                let pt = xy_to_mm(xy.0, xy.1, scale);
                let pt = transform_point(pt, origin, mirror_x, mag, angle_deg);
                out.texts.push((*layer, pt, text.clone()));
            }
            GdsElement::SRef {
                sname,
                xy,
                strans,
                mag: ref_mag,
                angle: ref_angle,
            } => {
                if let Some(&ref_idx) = struct_map.get(sname.as_str()) {
                    let ref_origin = xy_to_mm(xy.0, xy.1, scale);
                    let ref_origin = transform_point(ref_origin, origin, mirror_x, mag, angle_deg);
                    let ref_mirror = (strans & 0x8000) != 0;
                    flatten_structure(
                        ref_idx,
                        structures,
                        struct_map,
                        scale,
                        ref_origin,
                        ref_mirror,
                        *ref_mag,
                        *ref_angle,
                        depth + 1,
                        out,
                    );
                }
            }
            GdsElement::ARef {
                sname,
                xy,
                cols,
                rows,
                strans,
                mag: ref_mag,
                angle: ref_angle,
            } => {
                if let Some(&ref_idx) = struct_map.get(sname.as_str()) {
                    // AREF XY has 3 points: origin, col spacing end, row spacing end
                    if xy.len() >= 3 {
                        let p0 = xy_to_mm(xy[0].0, xy[0].1, scale);
                        let p0 = transform_point(p0, origin, mirror_x, mag, angle_deg);
                        let p1 = xy_to_mm(xy[1].0, xy[1].1, scale);
                        let p1 = transform_point(p1, origin, mirror_x, mag, angle_deg);
                        let p2 = xy_to_mm(xy[2].0, xy[2].1, scale);
                        let p2 = transform_point(p2, origin, mirror_x, mag, angle_deg);

                        let ncols = *cols as usize;
                        let nrows = *rows as usize;

                        let col_dx = if ncols > 1 {
                            (p1[0] - p0[0]) / ncols as f64
                        } else {
                            0.0
                        };
                        let col_dy = if ncols > 1 {
                            (p1[1] - p0[1]) / ncols as f64
                        } else {
                            0.0
                        };
                        let row_dx = if nrows > 1 {
                            (p2[0] - p0[0]) / nrows as f64
                        } else {
                            0.0
                        };
                        let row_dy = if nrows > 1 {
                            (p2[1] - p0[1]) / nrows as f64
                        } else {
                            0.0
                        };

                        let ref_mirror = (strans & 0x8000) != 0;

                        for r in 0..nrows {
                            for c in 0..ncols {
                                let inst_origin = [
                                    p0[0] + c as f64 * col_dx + r as f64 * row_dx,
                                    p0[1] + c as f64 * col_dy + r as f64 * row_dy,
                                ];
                                flatten_structure(
                                    ref_idx,
                                    structures,
                                    struct_map,
                                    scale,
                                    inst_origin,
                                    ref_mirror,
                                    *ref_mag,
                                    *ref_angle,
                                    depth + 1,
                                    out,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Parse GDSII binary data into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    if data.len() < 4 {
        return Err(ExtractError::ParseError("GDSII: file too small".into()));
    }

    // Validate GDSII magic: first record should be HEADER
    if data.len() >= 4 && data[2] != HEADER {
        return Err(ExtractError::ParseError(
            "GDSII: missing HEADER record".into(),
        ));
    }

    let records = parse_records(data)?;
    if records.is_empty() {
        return Err(ExtractError::ParseError("GDSII: no records found".into()));
    }

    // Extract units
    let (_user_unit, meters_per_db_unit) = extract_units(&records);
    // Convert database units to mm: meters_per_db_unit * 1000
    let scale = meters_per_db_unit * 1000.0;

    let libname = extract_libname(&records);

    // Parse structures
    let structures = parse_structures(&records)?;
    if structures.is_empty() {
        return Err(ExtractError::ParseError(
            "GDSII: no structures found".into(),
        ));
    }

    // Build name -> index map
    let struct_map: HashMap<&str, usize> = structures
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();

    // Find top-level structure
    let top_idx = find_top_structure(&structures).unwrap_or(structures.len() - 1);

    // Flatten the top structure
    let mut flat = FlattenOutput {
        boundaries: Vec::new(),
        paths: Vec::new(),
        texts: Vec::new(),
    };

    flatten_structure(
        top_idx,
        &structures,
        &struct_map,
        scale,
        [0.0, 0.0],
        false,
        1.0,
        0.0,
        0,
        &mut flat,
    );

    let boundaries = flat.boundaries;
    let all_paths = flat.paths;

    // Build board outline from boundaries (use all geometry for bbox)
    let mut bbox = BBox::empty();
    let mut edges: Vec<Drawing> = Vec::new();

    // All boundaries contribute to the bounding box
    for (_, pts) in &boundaries {
        for pt in pts {
            bbox.expand_point(pt[0], pt[1]);
        }
    }
    for (_, _, pts) in &all_paths {
        for pt in pts {
            bbox.expand_point(pt[0], pt[1]);
        }
    }

    // Use the first boundary of layer 0 (or the largest boundary) as the board outline
    let mut outline_boundary_idx: Option<usize> = None;
    let mut max_area: f64 = 0.0;
    for (i, (layer, pts)) in boundaries.iter().enumerate() {
        if *layer == 0 && pts.len() >= 3 {
            let area = polygon_area(pts);
            if area > max_area {
                max_area = area;
                outline_boundary_idx = Some(i);
            }
        }
    }
    // If no layer-0 boundary, use the largest boundary overall
    if outline_boundary_idx.is_none() {
        for (i, (_, pts)) in boundaries.iter().enumerate() {
            if pts.len() >= 3 {
                let area = polygon_area(pts);
                if area > max_area {
                    max_area = area;
                    outline_boundary_idx = Some(i);
                }
            }
        }
    }

    if let Some(idx) = outline_boundary_idx {
        let pts = &boundaries[idx].1;
        // Convert boundary polygon to edge segments
        for w in pts.windows(2) {
            edges.push(Drawing::Segment {
                start: w[0],
                end: w[1],
                width: 0.05,
            });
        }
    }

    // Build footprints from non-top-level structures that are referenced
    let mut footprints: Vec<Footprint> = Vec::new();
    let mut components: Vec<Component> = Vec::new();

    // Collect SREF instances from the top structure as footprints
    for elem in &structures[top_idx].elements {
        if let GdsElement::SRef {
            sname,
            xy,
            strans,
            mag: ref_mag,
            angle: ref_angle,
        } = elem
        {
            if let Some(&ref_idx) = struct_map.get(sname.as_str()) {
                let center = xy_to_mm(xy.0, xy.1, scale);
                let mirror_x = (strans & 0x8000) != 0;

                // Flatten the referenced structure to get its local geometry
                let mut sub_flat = FlattenOutput {
                    boundaries: Vec::new(),
                    paths: Vec::new(),
                    texts: Vec::new(),
                };

                flatten_structure(
                    ref_idx,
                    &structures,
                    &struct_map,
                    scale,
                    [0.0, 0.0],
                    false,
                    1.0,
                    0.0,
                    0,
                    &mut sub_flat,
                );

                let sub_boundaries = sub_flat.boundaries;
                let sub_paths = sub_flat.paths;

                // Compute local bounding box
                let mut fp_bbox = BBox::empty();
                for (_, pts) in &sub_boundaries {
                    for pt in pts {
                        let transformed =
                            transform_point(*pt, [0.0, 0.0], mirror_x, *ref_mag, *ref_angle);
                        fp_bbox.expand_point(transformed[0], transformed[1]);
                    }
                }
                for (_, _, pts) in &sub_paths {
                    for pt in pts {
                        let transformed =
                            transform_point(*pt, [0.0, 0.0], mirror_x, *ref_mag, *ref_angle);
                        fp_bbox.expand_point(transformed[0], transformed[1]);
                    }
                }

                // If the sub-structure has no geometry, use a small default bbox
                if fp_bbox.minx == f64::INFINITY {
                    fp_bbox = BBox {
                        minx: -0.5,
                        miny: -0.5,
                        maxx: 0.5,
                        maxy: 0.5,
                    };
                }

                let size = [fp_bbox.maxx - fp_bbox.minx, fp_bbox.maxy - fp_bbox.miny];
                let relpos = [fp_bbox.minx, fp_bbox.miny];

                // Build drawings for this footprint
                let mut fp_drawings: Vec<FootprintDrawing> = Vec::new();
                for (layer, pts) in &sub_boundaries {
                    if pts.len() >= 3 {
                        let transformed: Vec<[f64; 2]> = pts
                            .iter()
                            .map(|pt| {
                                transform_point(*pt, [0.0, 0.0], mirror_x, *ref_mag, *ref_angle)
                            })
                            .collect();
                        fp_drawings.push(FootprintDrawing {
                            layer: layer_name(*layer),
                            drawing: FootprintDrawingItem::Shape(Drawing::Polygon {
                                pos: [0.0, 0.0],
                                angle: 0.0,
                                polygons: vec![transformed],
                                filled: Some(1),
                                width: 0.0,
                            }),
                        });
                    }
                }
                for (layer, width_db, pts) in &sub_paths {
                    let width_mm = (*width_db as f64 * scale).abs();
                    let width_mm = if width_mm < 0.001 { 0.05 } else { width_mm };
                    for w in pts.windows(2) {
                        let s = transform_point(w[0], [0.0, 0.0], mirror_x, *ref_mag, *ref_angle);
                        let e = transform_point(w[1], [0.0, 0.0], mirror_x, *ref_mag, *ref_angle);
                        fp_drawings.push(FootprintDrawing {
                            layer: layer_name(*layer),
                            drawing: FootprintDrawingItem::Shape(Drawing::Segment {
                                start: s,
                                end: e,
                                width: width_mm,
                            }),
                        });
                    }
                }

                let layer_str = layer_side(0).to_string();
                let fp_index = footprints.len();
                let ref_name = format!("{}_{}", sname, fp_index);

                footprints.push(Footprint {
                    ref_: ref_name.clone(),
                    center,
                    bbox: FootprintBBox {
                        pos: center,
                        relpos,
                        size,
                        angle: *ref_angle,
                    },
                    pads: Vec::new(),
                    drawings: fp_drawings,
                    layer: layer_str.clone(),
                });

                components.push(Component {
                    ref_: ref_name,
                    val: sname.clone(),
                    footprint_name: sname.clone(),
                    layer: if layer_str == "B" {
                        Side::Back
                    } else {
                        Side::Front
                    },
                    footprint_index: fp_index,
                    extra_fields: HashMap::new(),
                    attr: None,
                });
            }
        }
    }

    // Build tracks and zones from flattened geometry
    let mut tracks_f: Vec<Track> = Vec::new();
    let mut tracks_b: Vec<Track> = Vec::new();
    let mut tracks_inner: HashMap<String, Vec<Track>> = HashMap::new();
    let mut zones_f: Vec<Zone> = Vec::new();
    let mut zones_b: Vec<Zone> = Vec::new();
    let mut zones_inner: HashMap<String, Vec<Zone>> = HashMap::new();

    if opts.include_tracks {
        for (layer, pts) in &boundaries {
            let zone = Zone {
                polygons: Some(vec![pts.clone()]),
                svgpath: None,
                width: Some(0.0),
                net: None,
                fillrule: None,
            };
            match *layer {
                0 => zones_f.push(zone),
                1 => zones_b.push(zone),
                n => {
                    zones_inner.entry(layer_name(n)).or_default().push(zone);
                }
            }
        }

        for (layer, width_db, pts) in &all_paths {
            let width_mm = (*width_db as f64 * scale).abs();
            let width_mm = if width_mm < 0.001 { 0.05 } else { width_mm };
            for w in pts.windows(2) {
                let track = Track::Segment {
                    start: w[0],
                    end: w[1],
                    width: width_mm,
                    net: None,
                    drillsize: None,
                };
                match *layer {
                    0 => tracks_f.push(track),
                    1 => tracks_b.push(track),
                    n => {
                        tracks_inner.entry(layer_name(n)).or_default().push(track);
                    }
                }
            }
        }
    }

    let tracks = if opts.include_tracks {
        Some(LayerData {
            front: tracks_f,
            back: tracks_b,
            inner: tracks_inner,
        })
    } else {
        None
    };

    let zones = if opts.include_tracks
        && (!zones_f.is_empty() || !zones_b.is_empty() || !zones_inner.is_empty())
    {
        Some(LayerData {
            front: zones_f,
            back: zones_b,
            inner: zones_inner,
        })
    } else {
        None
    };

    let silk_f: Vec<Drawing> = Vec::new();
    let silk_b: Vec<Drawing> = Vec::new();

    // Generate BOM if there are components
    let bom = if !components.is_empty() {
        Some(crate::bom::generate_bom(
            &footprints,
            &components,
            &crate::bom::BomConfig::default(),
        ))
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox: bbox,
        edges,
        drawings: Drawings {
            silkscreen: LayerData {
                front: silk_f,
                back: silk_b,
                inner: HashMap::new(),
            },
            fabrication: LayerData {
                front: Vec::new(),
                back: Vec::new(),
                inner: HashMap::new(),
            },
        },
        footprints,
        metadata: Metadata {
            title: if libname.is_empty() {
                "GDSII Layout".to_string()
            } else {
                libname
            },
            revision: String::new(),
            company: String::new(),
            date: String::new(),
        },
        bom,
        ibom_version: None,
        tracks,
        copper_pads: None,
        zones,
        nets: None,
        font_data: None,
    })
}

/// Compute the area of a polygon using the shoelace formula.
fn polygon_area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i][0] * pts[j][1];
        area -= pts[j][0] * pts[i][1];
    }
    area.abs() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gds_float_to_f64() {
        // Test zero
        let zero = [0u8; 8];
        assert_eq!(gds_float_to_f64(&zero), 0.0);

        // Test 1.0: exponent = 65 (0x41), mantissa = 0x10000000000000
        // 1.0 = (1/16) * 16^1 = 0.0625 * 16 = 1.0
        let one = [0x41, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let val = gds_float_to_f64(&one);
        assert!((val - 1.0).abs() < 1e-10, "Expected 1.0, got {}", val);

        // Test -1.0
        let neg_one = [0xC1, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let val = gds_float_to_f64(&neg_one);
        assert!((val + 1.0).abs() < 1e-10, "Expected -1.0, got {}", val);

        // Test 1e-9 (1nm database unit): common GDSII unit
        let nanometer = [0x39, 0x44, 0xB8, 0x2F, 0xA0, 0x9B, 0x5A, 0x54];
        let val = gds_float_to_f64(&nanometer);
        assert!((val - 1e-9).abs() < 1e-18, "Expected 1e-9, got {}", val);

        // Verify round-trip through f64_to_gds for 1e-6
        let micron_bytes = f64_to_gds(1e-6);
        let val = gds_float_to_f64(&micron_bytes);
        assert!((val - 1e-6).abs() < 1e-15, "Expected 1e-6, got {}", val);
    }

    #[test]
    fn test_polygon_area() {
        // Unit square
        let pts = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!((polygon_area(&pts) - 1.0).abs() < 1e-10);

        // Triangle
        let pts = vec![[0.0, 0.0], [2.0, 0.0], [1.0, 2.0]];
        assert!((polygon_area(&pts) - 2.0).abs() < 1e-10);
    }

    /// Build a minimal GDSII binary from scratch for testing.
    fn build_gds_bytes(
        db_unit_in_meters: f64,
        user_unit: f64,
        structures: &[(&str, &[GdsTestElement])],
    ) -> Vec<u8> {
        let mut data = Vec::new();

        // HEADER record: version 600
        write_record(&mut data, HEADER, DT_I16, &600i16.to_be_bytes());

        // BGNLIB record: 12 i16 values (dates)
        let dates = [0i16; 12];
        let mut date_bytes = Vec::new();
        for d in &dates {
            date_bytes.extend_from_slice(&d.to_be_bytes());
        }
        write_record(&mut data, 0x01, DT_I16, &date_bytes); // BGNLIB

        // LIBNAME
        write_record(&mut data, LIBNAME, DT_ASCII, b"testlib\0");

        // UNITS
        let mut units_bytes = Vec::new();
        units_bytes.extend_from_slice(&f64_to_gds(user_unit));
        units_bytes.extend_from_slice(&f64_to_gds(db_unit_in_meters));
        write_record(&mut data, UNITS, DT_F64, &units_bytes);

        // Structures
        for (name, elements) in structures {
            // BGNSTR
            write_record(&mut data, BGNSTR, DT_I16, &date_bytes);

            // STRNAME
            let mut name_bytes = name.as_bytes().to_vec();
            if name_bytes.len() % 2 != 0 {
                name_bytes.push(0);
            }
            write_record(&mut data, STRNAME, DT_ASCII, &name_bytes);

            for elem in *elements {
                match elem {
                    GdsTestElement::Boundary { layer, xy } => {
                        write_record(&mut data, BOUNDARY, DT_NONE, &[]);
                        write_record(&mut data, LAYER, DT_I16, &layer.to_be_bytes());
                        write_record(&mut data, DATATYPE, DT_I16, &0i16.to_be_bytes());

                        let mut xy_bytes = Vec::new();
                        for (x, y) in xy.iter() {
                            xy_bytes.extend_from_slice(&x.to_be_bytes());
                            xy_bytes.extend_from_slice(&y.to_be_bytes());
                        }
                        write_record(&mut data, XY, DT_I32, &xy_bytes);
                        write_record(&mut data, ENDEL, DT_NONE, &[]);
                    }
                    GdsTestElement::Path { layer, width, xy } => {
                        write_record(&mut data, PATH, DT_NONE, &[]);
                        write_record(&mut data, LAYER, DT_I16, &layer.to_be_bytes());
                        write_record(&mut data, DATATYPE, DT_I16, &0i16.to_be_bytes());
                        write_record(&mut data, WIDTH, DT_I32, &width.to_be_bytes());

                        let mut xy_bytes = Vec::new();
                        for (x, y) in xy.iter() {
                            xy_bytes.extend_from_slice(&x.to_be_bytes());
                            xy_bytes.extend_from_slice(&y.to_be_bytes());
                        }
                        write_record(&mut data, XY, DT_I32, &xy_bytes);
                        write_record(&mut data, ENDEL, DT_NONE, &[]);
                    }
                    GdsTestElement::SRef { sname, x, y } => {
                        write_record(&mut data, SREF, DT_NONE, &[]);
                        let mut sname_bytes = sname.as_bytes().to_vec();
                        if sname_bytes.len() % 2 != 0 {
                            sname_bytes.push(0);
                        }
                        write_record(&mut data, SNAME, DT_ASCII, &sname_bytes);
                        let mut xy_bytes = Vec::new();
                        xy_bytes.extend_from_slice(&x.to_be_bytes());
                        xy_bytes.extend_from_slice(&y.to_be_bytes());
                        write_record(&mut data, XY, DT_I32, &xy_bytes);
                        write_record(&mut data, ENDEL, DT_NONE, &[]);
                    }
                    GdsTestElement::Text { layer, x, y, text } => {
                        write_record(&mut data, TEXT, DT_NONE, &[]);
                        write_record(&mut data, LAYER, DT_I16, &layer.to_be_bytes());
                        write_record(&mut data, TEXTTYPE, DT_I16, &0i16.to_be_bytes());
                        let mut xy_bytes = Vec::new();
                        xy_bytes.extend_from_slice(&x.to_be_bytes());
                        xy_bytes.extend_from_slice(&y.to_be_bytes());
                        write_record(&mut data, XY, DT_I32, &xy_bytes);
                        let mut text_bytes = text.as_bytes().to_vec();
                        if text_bytes.len() % 2 != 0 {
                            text_bytes.push(0);
                        }
                        write_record(&mut data, STRING, DT_ASCII, &text_bytes);
                        write_record(&mut data, ENDEL, DT_NONE, &[]);
                    }
                }
            }

            // ENDSTR
            write_record(&mut data, ENDSTR, DT_NONE, &[]);
        }

        // ENDLIB
        write_record(&mut data, 0x04, DT_NONE, &[]); // ENDLIB

        data
    }

    enum GdsTestElement {
        Boundary {
            layer: i16,
            xy: Vec<(i32, i32)>,
        },
        Path {
            layer: i16,
            width: i32,
            xy: Vec<(i32, i32)>,
        },
        SRef {
            sname: String,
            x: i32,
            y: i32,
        },
        Text {
            layer: i16,
            x: i32,
            y: i32,
            text: String,
        },
    }

    fn write_record(data: &mut Vec<u8>, record_type: u8, data_type: u8, payload: &[u8]) {
        let length = (4 + payload.len()) as u16;
        data.extend_from_slice(&length.to_be_bytes());
        data.push(record_type);
        data.push(data_type);
        data.extend_from_slice(payload);
    }

    /// Convert an f64 to GDSII excess-64 format (8 bytes).
    fn f64_to_gds(value: f64) -> [u8; 8] {
        if value == 0.0 {
            return [0u8; 8];
        }

        let sign = if value < 0.0 { 1u8 } else { 0u8 };
        let mut v = value.abs();

        // Find exponent: v = mantissa * 16^(exp-64), where 1/16 <= mantissa < 1
        let mut exp: i32 = 64;
        if v >= 1.0 {
            while v >= 1.0 {
                v /= 16.0;
                exp += 1;
            }
        } else if v < 1.0 / 16.0 {
            while v < 1.0 / 16.0 {
                v *= 16.0;
                exp -= 1;
            }
        }

        let mantissa = (v * (1u64 << 56) as f64) as u64;
        let mut bytes = [0u8; 8];
        bytes[0] = (sign << 7) | (exp as u8 & 0x7F);
        for i in 1..8 {
            bytes[i] = ((mantissa >> (56 - i * 8)) & 0xFF) as u8;
        }
        bytes
    }

    #[test]
    fn test_parse_simple_gdsii() {
        // 1nm database unit, 1um user unit
        let gds = build_gds_bytes(
            1e-9, // 1nm per db unit
            1e-3, // 1um per user unit (user_unit = db_per_user = 1000)
            &[(
                "TOP",
                &[
                    // 50mm x 30mm rectangle (50e6 x 30e6 nm)
                    GdsTestElement::Boundary {
                        layer: 0,
                        xy: vec![
                            (0, 0),
                            (50_000_000, 0),
                            (50_000_000, 30_000_000),
                            (0, 30_000_000),
                            (0, 0),
                        ],
                    },
                    // A path (track)
                    GdsTestElement::Path {
                        layer: 0,
                        width: 200_000, // 0.2mm
                        xy: vec![(1_000_000, 1_000_000), (10_000_000, 1_000_000)],
                    },
                ],
            )],
        );

        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: false,
        };

        let pcb = parse(&gds, &opts).unwrap();

        // Should have edges from the boundary
        assert!(
            !pcb.edges.is_empty(),
            "Expected edges from boundary polygon"
        );

        // Bounding box should be approximately 50mm x 30mm
        // Y is negated, so miny=-30, maxy=0
        let width = pcb.edges_bbox.maxx - pcb.edges_bbox.minx;
        assert!(
            (width - 50.0).abs() < 0.1,
            "Expected width ~50mm, got {}",
            width
        );

        let height = pcb.edges_bbox.maxy - pcb.edges_bbox.miny;
        assert!(
            (height - 30.0).abs() < 0.1,
            "Expected height ~30mm, got {}",
            height
        );

        // Should have tracks
        let tracks = pcb.tracks.as_ref().unwrap();
        assert!(!tracks.front.is_empty(), "Expected tracks on front layer");

        // Should have zones
        let zones = pcb.zones.as_ref().unwrap();
        assert!(!zones.front.is_empty(), "Expected zones on front layer");

        // Metadata title should be the libname
        assert_eq!(pcb.metadata.title, "testlib");
    }

    #[test]
    fn test_parse_gdsii_with_sref() {
        let gds = build_gds_bytes(
            1e-9,
            1e-3,
            &[
                (
                    "CELL_A",
                    &[GdsTestElement::Boundary {
                        layer: 0,
                        xy: vec![
                            (0, 0),
                            (1_000_000, 0),
                            (1_000_000, 1_000_000),
                            (0, 1_000_000),
                            (0, 0),
                        ],
                    }],
                ),
                (
                    "TOP",
                    &[
                        // Board outline
                        GdsTestElement::Boundary {
                            layer: 0,
                            xy: vec![
                                (0, 0),
                                (10_000_000, 0),
                                (10_000_000, 10_000_000),
                                (0, 10_000_000),
                                (0, 0),
                            ],
                        },
                        // Place CELL_A
                        GdsTestElement::SRef {
                            sname: "CELL_A".to_string(),
                            x: 2_000_000,
                            y: 2_000_000,
                        },
                    ],
                ),
            ],
        );

        let opts = ExtractOptions::default();
        let pcb = parse(&gds, &opts).unwrap();

        // TOP is the top-level structure (not referenced by others)
        // CELL_A is referenced by TOP via SREF -> becomes a footprint
        assert_eq!(pcb.footprints.len(), 1);
        assert!(pcb.footprints[0].ref_.starts_with("CELL_A"));

        // BOM should be generated since we have components
        assert!(pcb.bom.is_some());
    }

    #[test]
    fn test_parse_gdsii_no_header_fails() {
        let data = vec![0x00, 0x04, 0xFF, 0x00]; // invalid record type
        let result = parse(&data, &ExtractOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_fails() {
        let result = parse(&[], &ExtractOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_layer_name() {
        assert_eq!(layer_name(0), "F");
        assert_eq!(layer_name(1), "B");
        assert_eq!(layer_name(2), "In2");
        assert_eq!(layer_name(31), "In31");
        assert_eq!(layer_name(63), "L63");
    }

    #[test]
    fn test_transform_point_identity() {
        let pt = [1.0, 2.0];
        let result = transform_point(pt, [0.0, 0.0], false, 1.0, 0.0);
        assert!((result[0] - 1.0).abs() < 1e-10);
        assert!((result[1] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_transform_point_translate() {
        let pt = [1.0, 2.0];
        let result = transform_point(pt, [10.0, 20.0], false, 1.0, 0.0);
        assert!((result[0] - 11.0).abs() < 1e-10);
        assert!((result[1] - 22.0).abs() < 1e-10);
    }

    #[test]
    fn test_transform_point_rotate_90() {
        let pt = [1.0, 0.0];
        let result = transform_point(pt, [0.0, 0.0], false, 1.0, 90.0);
        assert!(result[0].abs() < 1e-10, "Expected ~0, got {}", result[0]);
        assert!(
            (result[1] - 1.0).abs() < 1e-10,
            "Expected ~1, got {}",
            result[1]
        );
    }

    #[test]
    fn test_parse_gdsii_with_text() {
        let gds = build_gds_bytes(
            1e-9,
            1e-3,
            &[(
                "TOP",
                &[
                    GdsTestElement::Boundary {
                        layer: 0,
                        xy: vec![
                            (0, 0),
                            (10_000_000, 0),
                            (10_000_000, 10_000_000),
                            (0, 10_000_000),
                            (0, 0),
                        ],
                    },
                    GdsTestElement::Text {
                        layer: 0,
                        x: 5_000_000,
                        y: 5_000_000,
                        text: "Hello".to_string(),
                    },
                ],
            )],
        );

        let opts = ExtractOptions::default();
        let pcb = parse(&gds, &opts).unwrap();

        // Should parse without error
        assert!(!pcb.edges.is_empty());
        assert_eq!(pcb.metadata.title, "testlib");
    }
}
