//! GDSII tile pipeline — stage 1: placed-record stream.
//!
//! Resolves the GDSII cell hierarchy (SREF/AREF) into a flat stream of
//! [`PlacedRecord`]s in **world nanometers**, keeping `(layer, datatype)`,
//! geometry, and a per-record axis-aligned extent. Unlike the `PcbData`
//! [`super::parse`] path, nothing is squashed into PCB layers and TEXT / BOX /
//! NODE are preserved. Coordinates are GDSII-native (Y-up); the Y flip happens
//! once, later, in the tile `viewBox`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::ExtractError;

use super::reader::{
    self, get_ascii, get_f64, get_i16, get_i32, get_xy_pairs, ANGLE, AREF, BGNSTR, BOUNDARY,
    COLROW, DATATYPE, ENDEL, ENDSTR, LAYER, MAG, PATH, PATHTYPE, SNAME, SREF, STRANS, STRING,
    STRNAME, TEXT, UNITS, WIDTH, XY,
};

// Record types not promoted to named constants in the reader.
const NODE: u8 = 0x15;
const BOX: u8 = 0x2D;

/// Cell-hierarchy recursion limit (matches the `PcbData` flattener).
const MAX_DEPTH: usize = 64;
/// Upper bound on emitted placed records. The tile path tolerates far more
/// geometry than the BOM path; this is a safety stop for the eager flatten.
const MAX_PLACED_RECORDS: usize = 5_000_000;
/// Upper bound on AREF array instances (file-controlled COLROW counts).
const MAX_AREF_INSTANCES: usize = 1_000_000;

/// Axis-aligned extent in world nanometers (the integer twin of [`crate::types::BBox`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldBox {
    pub minx: i64,
    pub miny: i64,
    pub maxx: i64,
    pub maxy: i64,
}

impl WorldBox {
    /// An inverted/empty box that expands to fit the first point added.
    pub fn empty() -> Self {
        Self {
            minx: i64::MAX,
            miny: i64::MAX,
            maxx: i64::MIN,
            maxy: i64::MIN,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.minx > self.maxx || self.miny > self.maxy
    }

    pub fn expand_point(&mut self, x: i64, y: i64) {
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }

    pub fn union(&mut self, other: &WorldBox) {
        if other.is_empty() {
            return;
        }
        self.expand_point(other.minx, other.miny);
        self.expand_point(other.maxx, other.maxy);
    }

    /// True if this box intersects `other` (touching edges count).
    pub fn intersects(&self, other: &WorldBox) -> bool {
        self.minx <= other.maxx
            && self.maxx >= other.minx
            && self.miny <= other.maxy
            && self.maxy >= other.miny
    }
}

/// The GDSII element kind a placed record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordKind {
    Boundary,
    Path,
    Box,
    Node,
    Text,
}

/// Resolved geometry of a placed record, in world nanometers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Geom {
    /// Closed polygon ring(s).
    Poly { rings: Vec<Vec<[i64; 2]>> },
    /// Center-line path with a half-width and GDSII path end-cap type.
    Path {
        pts: Vec<[i64; 2]>,
        half_width: i64,
        pathtype: u8,
    },
    /// A text label anchored at a point.
    Label {
        at: [i64; 2],
        text: String,
        mag: f64,
        angle: f64,
    },
}

/// One fully-placed (instanced) geometry element in world nanometers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedRecord {
    pub layer: i16,
    pub datatype: i16,
    pub kind: RecordKind,
    pub bbox: WorldBox,
    pub geom: Geom,
}

/// Result of streaming a GDSII file: the placed records plus the world unit.
#[derive(Debug, Clone)]
pub struct RecordStream {
    pub records: Vec<PlacedRecord>,
    /// Nanometers per database unit (from the file's `UNITS` record).
    pub nm_per_dbu: f64,
    /// Whether the emit cap was hit (records were dropped).
    pub truncated: bool,
}

// ─── Cell model (keeps datatype / box / node / text, unlike the BOM path) ───

enum CellElem {
    Boundary {
        layer: i16,
        datatype: i16,
        xy: Vec<(i32, i32)>,
    },
    Path {
        layer: i16,
        datatype: i16,
        width: i32,
        pathtype: u8,
        xy: Vec<(i32, i32)>,
    },
    Box {
        layer: i16,
        datatype: i16,
        xy: Vec<(i32, i32)>,
    },
    Node {
        layer: i16,
        datatype: i16,
        xy: Vec<(i32, i32)>,
    },
    Text {
        layer: i16,
        datatype: i16,
        xy: (i32, i32),
        text: String,
        strans: u16,
        mag: f64,
        angle: f64,
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
}

struct Cell {
    name: String,
    elems: Vec<CellElem>,
}

/// A point's placement frame: reflect-about-X, magnification, rotation (deg),
/// and a translation already expressed in world nm.
#[derive(Clone, Copy)]
struct Frame {
    origin: [f64; 2],
    reflect: bool,
    mag: f64,
    angle_deg: f64,
}

impl Frame {
    fn root() -> Self {
        Self {
            origin: [0.0, 0.0],
            reflect: false,
            mag: 1.0,
            angle_deg: 0.0,
        }
    }

    /// Transform a child-local point (world nm) into the parent frame.
    fn apply(&self, mut x: f64, mut y: f64) -> [f64; 2] {
        if self.reflect {
            y = -y;
        }
        x *= self.mag;
        y *= self.mag;
        if self.angle_deg != 0.0 {
            let r = self.angle_deg.to_radians();
            let (s, c) = (r.sin(), r.cos());
            let rx = x * c - y * s;
            let ry = x * s + y * c;
            x = rx;
            y = ry;
        }
        [x + self.origin[0], y + self.origin[1]]
    }
}

/// Stream a GDSII byte buffer into placed records (world nanometers).
pub fn stream_records(data: &[u8]) -> Result<RecordStream, ExtractError> {
    let records = reader::parse_records(data)?;
    let nm_per_dbu = units_nm_per_dbu(&records);
    let cells = parse_cells(&records);

    let index: HashMap<&str, usize> = cells
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();

    let top = top_cell(&cells, &index);
    let mut out = Vec::new();
    let mut truncated = false;
    if let Some(top) = top {
        flatten(
            top,
            &cells,
            &index,
            nm_per_dbu,
            Frame::root(),
            0,
            &mut out,
            &mut truncated,
        );
    }

    Ok(RecordStream {
        records: out,
        nm_per_dbu,
        truncated,
    })
}

/// Nanometers per database unit from the `UNITS` record (`meters_per_db_unit`
/// is the second value). Defaults to 1 nm/DBU.
fn units_nm_per_dbu(records: &[reader::Record]) -> f64 {
    for r in records {
        if r.record_type == UNITS {
            let vals = get_f64(&r.data);
            if vals.len() >= 2 && vals[1] > 0.0 {
                return vals[1] * 1e9;
            }
        }
    }
    1.0
}

fn parse_cells(records: &[reader::Record]) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut i = 0;
    while i < records.len() {
        if records[i].record_type != BGNSTR {
            i += 1;
            continue;
        }
        i += 1;
        if i >= records.len() || records[i].record_type != STRNAME {
            continue;
        }
        let name = get_ascii(&records[i].data);
        i += 1;
        let mut elems = Vec::new();
        while i < records.len() && records[i].record_type != ENDSTR {
            let (elem, next) = match records[i].record_type {
                BOUNDARY => parse_element(records, i, RecordKind::Boundary),
                PATH => parse_element(records, i, RecordKind::Path),
                BOX => parse_element(records, i, RecordKind::Box),
                NODE => parse_element(records, i, RecordKind::Node),
                TEXT => parse_text(records, i),
                SREF => parse_sref(records, i),
                AREF => parse_aref(records, i),
                _ => (None, i + 1),
            };
            if let Some(e) = elem {
                elems.push(e);
            }
            i = next;
        }
        if i < records.len() {
            i += 1; // skip ENDSTR
        }
        cells.push(Cell { name, elems });
    }
    cells
}

/// Parse a BOUNDARY/PATH/BOX/NODE element (layer + datatype + XY [+ width]).
fn parse_element(
    records: &[reader::Record],
    start: usize,
    kind: RecordKind,
) -> (Option<CellElem>, usize) {
    let mut i = start + 1;
    let mut layer = 0i16;
    let mut datatype = 0i16;
    let mut width = 0i32;
    let mut pathtype = 0u8;
    let mut xy = Vec::new();
    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            LAYER => layer = first_i16(&records[i].data),
            DATATYPE => datatype = first_i16(&records[i].data),
            WIDTH => width = first_i32(&records[i].data),
            PATHTYPE => pathtype = first_i16(&records[i].data) as u8,
            XY => xy = get_xy_pairs(&records[i].data),
            _ => {}
        }
        i += 1;
    }
    if i < records.len() {
        i += 1; // skip ENDEL
    }
    let elem = match kind {
        RecordKind::Boundary => CellElem::Boundary {
            layer,
            datatype,
            xy,
        },
        RecordKind::Path => CellElem::Path {
            layer,
            datatype,
            width,
            pathtype,
            xy,
        },
        RecordKind::Box => CellElem::Box {
            layer,
            datatype,
            xy,
        },
        RecordKind::Node => CellElem::Node {
            layer,
            datatype,
            xy,
        },
        RecordKind::Text => unreachable!("text handled separately"),
    };
    (Some(elem), i)
}

fn parse_text(records: &[reader::Record], start: usize) -> (Option<CellElem>, usize) {
    let mut i = start + 1;
    let mut layer = 0i16;
    let mut datatype = 0i16;
    let mut xy = (0i32, 0i32);
    let mut text = String::new();
    let mut strans = 0u16;
    let mut mag = 1.0f64;
    let mut angle = 0.0f64;
    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            LAYER => layer = first_i16(&records[i].data),
            // TEXTTYPE shares the datatype slot conceptually; keep it as datatype.
            reader::TEXTTYPE => datatype = first_i16(&records[i].data),
            STRANS => strans = first_i16(&records[i].data) as u16,
            MAG => mag = first_f64(&records[i].data, 1.0),
            ANGLE => angle = first_f64(&records[i].data, 0.0),
            XY => {
                let pairs = get_xy_pairs(&records[i].data);
                if let Some(&p) = pairs.first() {
                    xy = p;
                }
            }
            STRING => text = get_ascii(&records[i].data),
            _ => {}
        }
        i += 1;
    }
    if i < records.len() {
        i += 1;
    }
    (
        Some(CellElem::Text {
            layer,
            datatype,
            xy,
            text,
            strans,
            mag,
            angle,
        }),
        i,
    )
}

fn parse_sref(records: &[reader::Record], start: usize) -> (Option<CellElem>, usize) {
    let mut i = start + 1;
    let mut sname = String::new();
    let mut xy = (0i32, 0i32);
    let mut strans = 0u16;
    let mut mag = 1.0f64;
    let mut angle = 0.0f64;
    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            SNAME => sname = get_ascii(&records[i].data),
            STRANS => strans = first_i16(&records[i].data) as u16,
            MAG => mag = first_f64(&records[i].data, 1.0),
            ANGLE => angle = first_f64(&records[i].data, 0.0),
            XY => {
                let pairs = get_xy_pairs(&records[i].data);
                if let Some(&p) = pairs.first() {
                    xy = p;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if i < records.len() {
        i += 1;
    }
    (
        Some(CellElem::SRef {
            sname,
            xy,
            strans,
            mag,
            angle,
        }),
        i,
    )
}

fn parse_aref(records: &[reader::Record], start: usize) -> (Option<CellElem>, usize) {
    let mut i = start + 1;
    let mut sname = String::new();
    let mut xy = Vec::new();
    let mut cols = 1i16;
    let mut rows = 1i16;
    let mut strans = 0u16;
    let mut mag = 1.0f64;
    let mut angle = 0.0f64;
    while i < records.len() && records[i].record_type != ENDEL {
        match records[i].record_type {
            SNAME => sname = get_ascii(&records[i].data),
            STRANS => strans = first_i16(&records[i].data) as u16,
            MAG => mag = first_f64(&records[i].data, 1.0),
            ANGLE => angle = first_f64(&records[i].data, 0.0),
            COLROW => {
                let v = get_i16(&records[i].data);
                if v.len() >= 2 {
                    cols = v[0];
                    rows = v[1];
                }
            }
            XY => xy = get_xy_pairs(&records[i].data),
            _ => {}
        }
        i += 1;
    }
    if i < records.len() {
        i += 1;
    }
    (
        Some(CellElem::ARef {
            sname,
            xy,
            cols,
            rows,
            strans,
            mag,
            angle,
        }),
        i,
    )
}

fn first_i16(d: &reader::RecordData) -> i16 {
    get_i16(d).first().copied().unwrap_or(0)
}
fn first_i32(d: &reader::RecordData) -> i32 {
    get_i32(d).first().copied().unwrap_or(0)
}
fn first_f64(d: &reader::RecordData, default: f64) -> f64 {
    get_f64(d).first().copied().unwrap_or(default)
}

/// Choose the top cell: the one not referenced by any other cell, else the last.
fn top_cell(cells: &[Cell], index: &HashMap<&str, usize>) -> Option<usize> {
    if cells.is_empty() {
        return None;
    }
    let mut referenced = vec![false; cells.len()];
    for cell in cells {
        for e in &cell.elems {
            let name = match e {
                CellElem::SRef { sname, .. } | CellElem::ARef { sname, .. } => Some(sname.as_str()),
                _ => None,
            };
            if let Some(n) = name {
                if let Some(&idx) = index.get(n) {
                    referenced[idx] = true;
                }
            }
        }
    }
    (0..cells.len())
        .find(|&i| !referenced[i])
        .or(Some(cells.len() - 1))
}

#[allow(clippy::too_many_arguments)]
fn flatten(
    idx: usize,
    cells: &[Cell],
    index: &HashMap<&str, usize>,
    nm_per_dbu: f64,
    frame: Frame,
    depth: usize,
    out: &mut Vec<PlacedRecord>,
    truncated: &mut bool,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_PLACED_RECORDS {
        if out.len() >= MAX_PLACED_RECORDS {
            *truncated = true;
        }
        return;
    }
    let to_nm = |v: i32| v as f64 * nm_per_dbu;

    for elem in &cells[idx].elems {
        if out.len() >= MAX_PLACED_RECORDS {
            *truncated = true;
            return;
        }
        match elem {
            CellElem::Boundary {
                layer,
                datatype,
                xy,
            }
            | CellElem::Box {
                layer,
                datatype,
                xy,
            }
            | CellElem::Node {
                layer,
                datatype,
                xy,
            } => {
                let kind = match elem {
                    CellElem::Box { .. } => RecordKind::Box,
                    CellElem::Node { .. } => RecordKind::Node,
                    _ => RecordKind::Boundary,
                };
                let ring = place_ring(xy, &frame, to_nm);
                if ring.len() < 2 {
                    continue;
                }
                let bbox = ring_bbox(&ring);
                out.push(PlacedRecord {
                    layer: *layer,
                    datatype: *datatype,
                    kind,
                    bbox,
                    geom: Geom::Poly { rings: vec![ring] },
                });
            }
            CellElem::Path {
                layer,
                datatype,
                width,
                pathtype,
                xy,
            } => {
                let pts = place_ring(xy, &frame, to_nm);
                if pts.len() < 2 {
                    continue;
                }
                let half_width = ((*width as f64 * frame.mag * nm_per_dbu) / 2.0).round() as i64;
                let mut bbox = ring_bbox(&pts);
                // Inflate by the half-width so the stroke extent is captured.
                bbox.minx -= half_width;
                bbox.miny -= half_width;
                bbox.maxx += half_width;
                bbox.maxy += half_width;
                out.push(PlacedRecord {
                    layer: *layer,
                    datatype: *datatype,
                    kind: RecordKind::Path,
                    bbox,
                    geom: Geom::Path {
                        pts,
                        half_width,
                        pathtype: *pathtype,
                    },
                });
            }
            CellElem::Text {
                layer,
                datatype,
                xy,
                text,
                strans,
                mag,
                angle,
            } => {
                let p = frame.apply(to_nm(xy.0), to_nm(xy.1));
                let at = [p[0].round() as i64, p[1].round() as i64];
                let mut bbox = WorldBox::empty();
                bbox.expand_point(at[0], at[1]);
                // strans reflect-about-X negates the text's rotation sense.
                let reflect = (*strans & 0x8000) != 0;
                let label_angle = if reflect { -*angle } else { *angle } + frame.angle_deg;
                out.push(PlacedRecord {
                    layer: *layer,
                    datatype: *datatype,
                    kind: RecordKind::Text,
                    bbox,
                    geom: Geom::Label {
                        at,
                        text: text.clone(),
                        mag: *mag * frame.mag,
                        angle: label_angle,
                    },
                });
            }
            CellElem::SRef {
                sname,
                xy,
                strans,
                mag,
                angle,
            } => {
                let Some(&child) = index.get(sname.as_str()) else {
                    continue;
                };
                let origin = frame.apply(to_nm(xy.0), to_nm(xy.1));
                let child_frame = Frame {
                    origin,
                    reflect: (*strans & 0x8000) != 0,
                    mag: *mag,
                    angle_deg: *angle,
                };
                flatten(
                    child,
                    cells,
                    index,
                    nm_per_dbu,
                    child_frame,
                    depth + 1,
                    out,
                    truncated,
                );
            }
            CellElem::ARef {
                sname,
                xy,
                cols,
                rows,
                strans,
                mag,
                angle,
            } => {
                let Some(&child) = index.get(sname.as_str()) else {
                    continue;
                };
                if xy.len() < 3 {
                    continue;
                }
                let ncols = (*cols).max(0) as usize;
                let nrows = (*rows).max(0) as usize;
                if ncols == 0 || nrows == 0 || ncols.saturating_mul(nrows) > MAX_AREF_INSTANCES {
                    continue;
                }
                let p0 = frame.apply(to_nm(xy[0].0), to_nm(xy[0].1));
                let p1 = frame.apply(to_nm(xy[1].0), to_nm(xy[1].1));
                let p2 = frame.apply(to_nm(xy[2].0), to_nm(xy[2].1));
                let col_d = if ncols > 1 {
                    [
                        (p1[0] - p0[0]) / ncols as f64,
                        (p1[1] - p0[1]) / ncols as f64,
                    ]
                } else {
                    [0.0, 0.0]
                };
                let row_d = if nrows > 1 {
                    [
                        (p2[0] - p0[0]) / nrows as f64,
                        (p2[1] - p0[1]) / nrows as f64,
                    ]
                } else {
                    [0.0, 0.0]
                };
                let reflect = (*strans & 0x8000) != 0;
                'rows: for r in 0..nrows {
                    for c in 0..ncols {
                        if out.len() >= MAX_PLACED_RECORDS {
                            *truncated = true;
                            break 'rows;
                        }
                        let origin = [
                            p0[0] + c as f64 * col_d[0] + r as f64 * row_d[0],
                            p0[1] + c as f64 * col_d[1] + r as f64 * row_d[1],
                        ];
                        let child_frame = Frame {
                            origin,
                            reflect,
                            mag: *mag,
                            angle_deg: *angle,
                        };
                        flatten(
                            child,
                            cells,
                            index,
                            nm_per_dbu,
                            child_frame,
                            depth + 1,
                            out,
                            truncated,
                        );
                    }
                }
            }
        }
    }
}

/// Place a DBU coordinate list into world nm through `frame`, rounding to i64.
fn place_ring(xy: &[(i32, i32)], frame: &Frame, to_nm: impl Fn(i32) -> f64) -> Vec<[i64; 2]> {
    xy.iter()
        .map(|&(x, y)| {
            let p = frame.apply(to_nm(x), to_nm(y));
            [p[0].round() as i64, p[1].round() as i64]
        })
        .collect()
}

fn ring_bbox(ring: &[[i64; 2]]) -> WorldBox {
    let mut b = WorldBox::empty();
    for p in ring {
        b.expand_point(p[0], p[1]);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── Minimal GDSII byte-stream builder for tests ──────────────────────────

    fn rec_no_data(rt: u8) -> Vec<u8> {
        let mut v = vec![0, 4, rt, 0x00];
        let len = v.len() as u16;
        v[0..2].copy_from_slice(&len.to_be_bytes());
        v
    }
    fn rec_i16(rt: u8, vals: &[i16]) -> Vec<u8> {
        let mut payload = Vec::new();
        for &x in vals {
            payload.extend_from_slice(&x.to_be_bytes());
        }
        frame_record(rt, 0x02, &payload)
    }
    fn rec_i32(rt: u8, vals: &[i32]) -> Vec<u8> {
        let mut payload = Vec::new();
        for &x in vals {
            payload.extend_from_slice(&x.to_be_bytes());
        }
        frame_record(rt, 0x03, &payload)
    }
    fn rec_f64(rt: u8, vals: &[f64]) -> Vec<u8> {
        let mut payload = Vec::new();
        for &x in vals {
            payload.extend_from_slice(&f64_to_gds(x));
        }
        frame_record(rt, 0x05, &payload)
    }
    fn rec_ascii(rt: u8, s: &str) -> Vec<u8> {
        let mut payload = s.as_bytes().to_vec();
        if payload.len() % 2 == 1 {
            payload.push(0);
        }
        frame_record(rt, 0x06, &payload)
    }
    fn frame_record(rt: u8, dt: u8, payload: &[u8]) -> Vec<u8> {
        let len = (4 + payload.len()) as u16;
        let mut v = Vec::new();
        v.write_all(&len.to_be_bytes()).unwrap();
        v.push(rt);
        v.push(dt);
        v.extend_from_slice(payload);
        v
    }
    fn f64_to_gds(value: f64) -> [u8; 8] {
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

    /// A one-cell file with a single square boundary on layer 5/0, 1 nm/DBU.
    fn single_square() -> Vec<u8> {
        let mut g = Vec::new();
        g.extend(rec_i16(reader::HEADER, &[600]));
        g.extend(rec_i16(reader::BGNLIB, &[0; 12]));
        g.extend(rec_ascii(reader::LIBNAME, "TOP"));
        g.extend(rec_f64(UNITS, &[1e-3, 1e-9])); // 1 nm/DBU
        g.extend(rec_i16(BGNSTR, &[0; 12]));
        g.extend(rec_ascii(STRNAME, "TOP"));
        g.extend(rec_no_data(BOUNDARY));
        g.extend(rec_i16(LAYER, &[5]));
        g.extend(rec_i16(DATATYPE, &[0]));
        g.extend(rec_i32(XY, &[0, 0, 100, 0, 100, 100, 0, 100, 0, 0]));
        g.extend(rec_no_data(ENDEL));
        g.extend(rec_no_data(ENDSTR));
        g.extend(rec_no_data(reader::ENDLIB));
        g
    }

    #[test]
    fn streams_a_single_boundary() {
        let s = stream_records(&single_square()).unwrap();
        assert_eq!(s.records.len(), 1);
        let r = &s.records[0];
        assert_eq!(r.layer, 5);
        assert_eq!(r.datatype, 0);
        assert_eq!(r.kind, RecordKind::Boundary);
        assert_eq!(
            r.bbox,
            WorldBox {
                minx: 0,
                miny: 0,
                maxx: 100,
                maxy: 100
            }
        );
        assert!((s.nm_per_dbu - 1.0).abs() < 1e-9);
    }

    #[test]
    fn datatype_is_preserved() {
        // Same file but datatype 7 — the BOM path would discard this.
        let mut g = single_square();
        // rebuild with datatype 7 is simpler than patching bytes:
        let _ = &mut g;
        let mut f = Vec::new();
        f.extend(rec_i16(reader::HEADER, &[600]));
        f.extend(rec_i16(reader::BGNLIB, &[0; 12]));
        f.extend(rec_ascii(reader::LIBNAME, "T"));
        f.extend(rec_f64(UNITS, &[1e-3, 1e-9]));
        f.extend(rec_i16(BGNSTR, &[0; 12]));
        f.extend(rec_ascii(STRNAME, "T"));
        f.extend(rec_no_data(BOUNDARY));
        f.extend(rec_i16(LAYER, &[5]));
        f.extend(rec_i16(DATATYPE, &[7]));
        f.extend(rec_i32(XY, &[0, 0, 10, 0, 10, 10, 0, 10, 0, 0]));
        f.extend(rec_no_data(ENDEL));
        f.extend(rec_no_data(ENDSTR));
        f.extend(rec_no_data(reader::ENDLIB));
        let s = stream_records(&f).unwrap();
        assert_eq!(s.records[0].datatype, 7);
    }

    #[test]
    fn sref_translates_child_geometry() {
        // TOP places CELL (a square at 0..10) at (1000, 2000).
        let mut g = Vec::new();
        g.extend(rec_i16(reader::HEADER, &[600]));
        g.extend(rec_i16(reader::BGNLIB, &[0; 12]));
        g.extend(rec_ascii(reader::LIBNAME, "L"));
        g.extend(rec_f64(UNITS, &[1e-3, 1e-9]));
        // child cell
        g.extend(rec_i16(BGNSTR, &[0; 12]));
        g.extend(rec_ascii(STRNAME, "CELL"));
        g.extend(rec_no_data(BOUNDARY));
        g.extend(rec_i16(LAYER, &[1]));
        g.extend(rec_i16(DATATYPE, &[0]));
        g.extend(rec_i32(XY, &[0, 0, 10, 0, 10, 10, 0, 10, 0, 0]));
        g.extend(rec_no_data(ENDEL));
        g.extend(rec_no_data(ENDSTR));
        // top cell with an SREF of CELL
        g.extend(rec_i16(BGNSTR, &[0; 12]));
        g.extend(rec_ascii(STRNAME, "TOP"));
        g.extend(rec_no_data(SREF));
        g.extend(rec_ascii(SNAME, "CELL"));
        g.extend(rec_i32(XY, &[1000, 2000]));
        g.extend(rec_no_data(ENDEL));
        g.extend(rec_no_data(ENDSTR));
        g.extend(rec_no_data(reader::ENDLIB));

        let s = stream_records(&g).unwrap();
        assert_eq!(s.records.len(), 1);
        assert_eq!(
            s.records[0].bbox,
            WorldBox {
                minx: 1000,
                miny: 2000,
                maxx: 1010,
                maxy: 2010
            }
        );
    }

    #[test]
    fn aref_expands_grid() {
        // 2x3 array of a tiny square; pitch 100 in X, 200 in Y.
        let mut g = Vec::new();
        g.extend(rec_i16(reader::HEADER, &[600]));
        g.extend(rec_i16(reader::BGNLIB, &[0; 12]));
        g.extend(rec_ascii(reader::LIBNAME, "L"));
        g.extend(rec_f64(UNITS, &[1e-3, 1e-9]));
        g.extend(rec_i16(BGNSTR, &[0; 12]));
        g.extend(rec_ascii(STRNAME, "CELL"));
        g.extend(rec_no_data(BOUNDARY));
        g.extend(rec_i16(LAYER, &[1]));
        g.extend(rec_i16(DATATYPE, &[0]));
        g.extend(rec_i32(XY, &[0, 0, 5, 0, 5, 5, 0, 5, 0, 0]));
        g.extend(rec_no_data(ENDEL));
        g.extend(rec_no_data(ENDSTR));
        g.extend(rec_i16(BGNSTR, &[0; 12]));
        g.extend(rec_ascii(STRNAME, "TOP"));
        g.extend(rec_no_data(AREF));
        g.extend(rec_ascii(SNAME, "CELL"));
        g.extend(rec_i16(COLROW, &[2, 3])); // 2 cols, 3 rows
                                            // ref points: origin, col-end (origin + cols*pitchX), row-end (origin + rows*pitchY)
        g.extend(rec_i32(XY, &[0, 0, 200, 0, 0, 600]));
        g.extend(rec_no_data(ENDEL));
        g.extend(rec_no_data(ENDSTR));
        g.extend(rec_no_data(reader::ENDLIB));

        let s = stream_records(&g).unwrap();
        assert_eq!(s.records.len(), 6, "2x3 grid => 6 instances");
    }
}
