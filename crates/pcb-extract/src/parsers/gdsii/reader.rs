//! Byte-level GDSII record reader.
//!
//! Turns a raw `.gds`/`.gds2` byte stream into a `Vec<Record>`: the record
//! framing loop, the excess-64 (IBM) float decoder, and typed accessors over
//! `RecordData`. This layer is format-faithful and carries no PCB or layout
//! semantics, so both the `PcbData` parser and the tile pipeline can share it.

use crate::error::ExtractError;

// GDSII record types
pub(crate) const HEADER: u8 = 0x00;
pub(crate) const BGNLIB: u8 = 0x01;
pub(crate) const LIBNAME: u8 = 0x02;
pub(crate) const UNITS: u8 = 0x03;
pub(crate) const ENDLIB: u8 = 0x04;
pub(crate) const BGNSTR: u8 = 0x05;
pub(crate) const STRNAME: u8 = 0x06;
pub(crate) const ENDSTR: u8 = 0x07;
pub(crate) const BOUNDARY: u8 = 0x08;
pub(crate) const PATH: u8 = 0x09;
pub(crate) const SREF: u8 = 0x0A;
pub(crate) const AREF: u8 = 0x0B;
pub(crate) const TEXT: u8 = 0x0C;
pub(crate) const LAYER: u8 = 0x0D;
pub(crate) const DATATYPE: u8 = 0x0E;
pub(crate) const WIDTH: u8 = 0x0F;
pub(crate) const XY: u8 = 0x10;
pub(crate) const ENDEL: u8 = 0x11;
pub(crate) const SNAME: u8 = 0x12;
pub(crate) const COLROW: u8 = 0x13;
pub(crate) const TEXTTYPE: u8 = 0x16;
pub(crate) const STRING: u8 = 0x19;
pub(crate) const STRANS: u8 = 0x1A;
pub(crate) const MAG: u8 = 0x1B;
pub(crate) const ANGLE: u8 = 0x1C;
pub(crate) const PATHTYPE: u8 = 0x21;
pub(crate) const BGNEXTN: u8 = 0x30;
pub(crate) const ENDEXTN: u8 = 0x31;

/// All valid GDSII record type codes.
const KNOWN_RECORD_TYPES: &[u8] = &[
    0x00, // HEADER
    0x01, // BGNLIB
    0x02, // LIBNAME
    0x03, // UNITS
    0x04, // ENDLIB
    0x05, // BGNSTR
    0x06, // STRNAME
    0x07, // ENDSTR
    0x08, // BOUNDARY
    0x09, // PATH
    0x0A, // SREF
    0x0B, // AREF
    0x0C, // TEXT
    0x0D, // LAYER
    0x0E, // DATATYPE
    0x0F, // WIDTH
    0x10, // XY
    0x11, // ENDEL
    0x12, // SNAME
    0x13, // COLROW
    0x14, // TEXTNODE
    0x15, // NODE
    0x16, // TEXTTYPE
    0x17, // PRESENTATION
    0x19, // STRING
    0x1A, // STRANS
    0x1B, // MAG
    0x1C, // ANGLE
    0x1F, // REFLIBS
    0x20, // FONTS
    0x21, // PATHTYPE
    0x22, // GENERATIONS
    0x23, // ATTRTABLE
    0x26, // ELFLAGS
    0x2A, // NODETYPE
    0x2B, // PROPATTR
    0x2C, // PROPVALUE
    0x2D, // BOX
    0x2E, // BOXTYPE
    0x2F, // PLEX
    0x30, // BGNEXTN
    0x31, // ENDEXTN
    0x32, // TAPENUM
    0x33, // TAPECODE
    0x36, // FORMAT
    0x37, // MASK
    0x38, // ENDMASKS
];

// GDSII data types
pub(crate) const DT_NONE: u8 = 0x00;
pub(crate) const DT_BITARRAY: u8 = 0x01;
pub(crate) const DT_I16: u8 = 0x02;
pub(crate) const DT_I32: u8 = 0x03;
pub(crate) const DT_F64: u8 = 0x05;
pub(crate) const DT_ASCII: u8 = 0x06;

/// A parsed GDSII record.
pub(crate) struct Record {
    pub(crate) record_type: u8,
    pub(crate) data: RecordData,
}

/// Data payload of a GDSII record.
pub(crate) enum RecordData {
    None,
    BitArray(Vec<u16>),
    Int16(Vec<i16>),
    Int32(Vec<i32>),
    Float64(Vec<f64>),
    Ascii(String),
}

/// Convert GDSII excess-64 (IBM) float to IEEE 754 f64.
///
/// Format: 1 sign bit, 7 exponent bits (biased by 64), 56 mantissa bits.
/// value = (-1)^sign * (mantissa / 2^56) * 16^(exponent - 64)
pub(crate) fn gds_float_to_f64(bytes: &[u8]) -> f64 {
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
pub(crate) fn parse_records(data: &[u8]) -> Result<Vec<Record>, ExtractError> {
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

        if !KNOWN_RECORD_TYPES.contains(&record_type) {
            return Err(ExtractError::ParseError(format!(
                "GDSII: unknown record type 0x{:02X} at offset {}",
                record_type, offset
            )));
        }

        let payload = &data[offset + 4..offset + length];

        let record_data = parse_record_data(data_type, payload, record_type, offset)?;
        records.push(Record {
            record_type,
            data: record_data,
        });

        offset += length;

        if record_type == ENDLIB {
            break;
        }
    }

    Ok(records)
}

/// Parse the data payload of a record based on data type.
fn parse_record_data(
    data_type: u8,
    payload: &[u8],
    record_type: u8,
    offset: usize,
) -> Result<RecordData, ExtractError> {
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
        _ => Err(ExtractError::ParseError(format!(
            "GDSII: invalid data type 0x{:02X} for record type 0x{:02X} at offset {}",
            data_type, record_type, offset
        ))),
    }
}

/// Extract i16 values from record data.
pub(crate) fn get_i16(data: &RecordData) -> Vec<i16> {
    match data {
        RecordData::Int16(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract i32 values from record data.
pub(crate) fn get_i32(data: &RecordData) -> Vec<i32> {
    match data {
        RecordData::Int32(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract f64 values from record data.
pub(crate) fn get_f64(data: &RecordData) -> Vec<f64> {
    match data {
        RecordData::Float64(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract ASCII string from record data.
pub(crate) fn get_ascii(data: &RecordData) -> String {
    match data {
        RecordData::Ascii(s) => s.clone(),
        _ => String::new(),
    }
}

/// Extract bitarray values from record data.
pub(crate) fn get_bitarray(data: &RecordData) -> Vec<u16> {
    match data {
        RecordData::BitArray(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Extract XY coordinate pairs from a record data (i32 pairs).
pub(crate) fn get_xy_pairs(data: &RecordData) -> Vec<(i32, i32)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert an f64 to GDSII excess-64 format (8 bytes), for round-trip tests.
    fn f64_to_gds_float(value: f64) -> [u8; 8] {
        if value == 0.0 {
            return [0u8; 8];
        }
        let sign = if value < 0.0 { 1u8 } else { 0u8 };
        let mut v = value.abs();
        let mut exponent = 64i32;
        while v >= 1.0 {
            v /= 16.0;
            exponent += 1;
        }
        while v < 1.0 / 16.0 {
            v *= 16.0;
            exponent -= 1;
        }
        let mantissa = (v * (1u64 << 56) as f64) as u64;
        let mut bytes = [0u8; 8];
        bytes[0] = (sign << 7) | (exponent as u8 & 0x7F);
        for (i, b) in bytes.iter_mut().enumerate().skip(1) {
            *b = ((mantissa >> (8 * (7 - i))) & 0xFF) as u8;
        }
        bytes
    }

    #[test]
    fn test_gds_float_to_f64() {
        assert_eq!(gds_float_to_f64(&[0u8; 8]), 0.0);
        for v in [1.0, 0.001, 1e-9, -2.5, 1234.5] {
            let bytes = f64_to_gds_float(v);
            let decoded = gds_float_to_f64(&bytes);
            assert!(
                (decoded - v).abs() < v.abs() * 1e-10 + 1e-15,
                "round-trip {v} -> {decoded}"
            );
        }
    }

    #[test]
    fn test_parse_records_rejects_short_length() {
        // A record claiming length < 4 is invalid.
        let bytes = [0x00, 0x02, HEADER, DT_I16];
        assert!(parse_records(&bytes).is_err());
    }
}
