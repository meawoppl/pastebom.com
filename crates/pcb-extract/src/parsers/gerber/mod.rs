mod apertures;
pub mod commands;
pub mod coord;
pub mod excellon;
pub mod interpreter;
pub mod layers;
pub mod lexer;
pub mod macros;

use std::collections::HashMap;
use std::io::{Cursor, Read};

use serde::Serialize;

use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;

use self::commands::GerberCommand;
use self::interpreter::GerberLayerOutput;
use self::layers::{GerberLayerType, LayerFunction, LayerSide};

/// Parse a zip file containing Gerber files into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    parse_with_limit(data, opts, crate::MAX_DECOMPRESSED_BYTES)
}

fn parse_with_limit(
    data: &[u8],
    opts: &ExtractOptions,
    max_decompressed: u64,
) -> Result<PcbData, ExtractError> {
    let mut layers: Vec<GerberLayer> = Vec::new();
    let mut had_gerber = false;

    for (filename, content) in read_zip_entries(data, max_decompressed)? {
        if let Ok(layer) = parse_file(&filename, &content) {
            had_gerber = true;
            if layer.layer_type != GerberLayerType::Unknown || !layer.drawings.is_empty() {
                layers.push(layer);
            }
        }
    }

    if !had_gerber {
        return Err(ExtractError::ParseError(
            "No Gerber files found in zip".into(),
        ));
    }

    assemble_pcb_data(layers, opts)
}

/// Read every file in a zip archive, enforcing a total decompressed size limit.
fn read_zip_entries(
    data: &[u8],
    max_decompressed: u64,
) -> Result<Vec<(String, Vec<u8>)>, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
    let mut entries = Vec::new();
    let mut total_decompressed: u64 = 0;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let filename = file.name().to_string();

        let remaining = max_decompressed.saturating_sub(total_decompressed);
        let mut content = Vec::new();
        if file.take(remaining + 1).read_to_end(&mut content).is_err() {
            continue;
        }
        total_decompressed += content.len() as u64;
        if total_decompressed > max_decompressed {
            return Err(ExtractError::DecompressionBomb);
        }
        entries.push((filename, content));
    }
    Ok(entries)
}

/// One parsed fabrication file: a Gerber layer or an Excellon drill file.
#[derive(Debug, Clone, Serialize)]
pub struct GerberLayer {
    /// Source filename, including any directory path inside an archive.
    pub name: String,
    #[serde(skip)]
    pub layer_type: GerberLayerType,
    pub function: LayerFunction,
    pub side: Option<LayerSide>,
    /// Inner copper layer name (`In1`, `In2`, ...) for inner copper layers.
    pub inner: Option<String>,
    /// Dark-polarity geometry.
    pub drawings: Vec<Drawing>,
    /// Clear-polarity (%LPC%) geometry that erases previously drawn dark geometry.
    pub clear_drawings: Vec<Drawing>,
    pub bbox: Option<BBox>,
}

impl GerberLayer {
    fn new(name: &str, layer_type: GerberLayerType, output: GerberLayerOutput) -> Self {
        let mut bbox = BBox::empty();
        for d in &output.drawings {
            expand_bbox_drawing(&mut bbox, d);
        }
        Self {
            name: name.to_string(),
            function: layer_type.function(),
            side: layer_type.side(),
            inner: layer_type.inner_name().map(str::to_string),
            layer_type,
            drawings: output.drawings,
            clear_drawings: output.clear_drawings,
            bbox: bbox.minx.is_finite().then_some(bbox),
        }
    }
}

/// A set of fabrication files parsed independently, one layer per file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GerberProject {
    /// Layers in back-to-front stacking order.
    pub layers: Vec<GerberLayer>,
    /// Board bounds: the outline's extents when present, else copper, else everything.
    pub bbox: Option<BBox>,
    /// Problems worth showing: files that look like fabrication data but failed to
    /// parse, and layers whose function could not be identified.
    pub warnings: Vec<String>,
    /// Informational: files that are not fabrication data (logs, PDFs, READMEs) or
    /// have nothing to draw (empty layers, drill files without holes).
    pub skipped: Vec<String>,
}

impl GerberProject {
    /// Parse a list of `(filename, contents)` pairs. See [`parse_source`].
    pub fn from_files<'a, I>(files: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a [u8])>,
    {
        let mut project = Self::default();
        for (name, data) in files {
            project.add_source(name, data);
        }
        project
    }

    /// Parse one source and merge its layers into the project in stacking order.
    pub fn add_source(&mut self, name: &str, data: &[u8]) {
        let parsed = parse_source(name, data);
        self.warnings.extend(parsed.warnings);
        self.skipped.extend(parsed.skipped);
        for layer in parsed.layers {
            let order = layer.layer_type.stack_order();
            let pos = self
                .layers
                .partition_point(|l| l.layer_type.stack_order() <= order);
            self.layers.insert(pos, layer);
        }
        self.bbox = board_bbox(&self.layers);
    }
}

/// Layers and diagnostics from one source. Diagnostics read `"<file>: <reason>"`.
#[derive(Debug, Default)]
pub struct ParsedSource {
    pub layers: Vec<GerberLayer>,
    /// See [`GerberProject::warnings`].
    pub warnings: Vec<String>,
    /// See [`GerberProject::skipped`].
    pub skipped: Vec<String>,
}

/// Longest diagnostic reason kept; parser errors can quote whole files.
const MAX_REASON_CHARS: usize = 160;

impl ParsedSource {
    fn warn(&mut self, name: &str, reason: impl std::fmt::Display) {
        self.warnings.push(diagnostic(name, reason));
    }

    fn skip(&mut self, name: &str, reason: impl std::fmt::Display) {
        self.skipped.push(diagnostic(name, reason));
    }

    fn add_file(&mut self, name: &str, data: &[u8]) {
        match parse_file(name, data) {
            Ok(layer) if layer.drawings.is_empty() && layer.clear_drawings.is_empty() => {
                self.skip(name, "no geometry");
            }
            Ok(layer) => {
                if layer.layer_type == GerberLayerType::Unknown {
                    self.warn(name, "could not identify layer function");
                }
                self.layers.push(layer);
            }
            Err(FileError::NotFabricationData(reason)) => self.skip(name, reason),
            Err(FileError::Invalid(e)) => self.warn(name, e),
        }
    }
}

fn diagnostic(name: &str, reason: impl std::fmt::Display) -> String {
    let reason = reason.to_string();
    match reason.char_indices().nth(MAX_REASON_CHARS) {
        Some((cut, _)) => format!("{name}: {}…", &reason[..cut]),
        None => format!("{name}: {reason}"),
    }
}

/// Parse one source file into layers. Zip archives are expanded, with entries named
/// `archive.zip/path/in/archive`. Unknown-function files are still returned as
/// layers, with a warning.
pub fn parse_source(name: &str, data: &[u8]) -> ParsedSource {
    let mut parsed = ParsedSource::default();
    if data.starts_with(b"PK\x03\x04") {
        match read_zip_entries(data, crate::MAX_DECOMPRESSED_BYTES) {
            Ok(entries) => {
                for (entry_name, content) in &entries {
                    parsed.add_file(&format!("{name}/{entry_name}"), content);
                }
            }
            Err(e) => parsed.warn(name, e),
        }
    } else {
        parsed.add_file(name, data);
    }
    parsed
}

/// Board bounds for a set of layers: the outline layers' extents when present,
/// otherwise the copper layers', otherwise all geometry. Skipping documentation
/// layers keeps fab drawings and title blocks from dwarfing the board.
pub fn board_bbox<'a, I>(layers: I) -> Option<BBox>
where
    I: IntoIterator<Item = &'a GerberLayer>,
    I::IntoIter: Clone,
{
    let layers = layers.into_iter();
    let union = |pred: &dyn Fn(&GerberLayer) -> bool| {
        let mut bbox = BBox::empty();
        for b in layers
            .clone()
            .filter(|l| pred(l))
            .filter_map(|l| l.bbox.as_ref())
        {
            bbox.expand_point(b.minx, b.miny);
            bbox.expand_point(b.maxx, b.maxy);
        }
        bbox.minx.is_finite().then_some(bbox)
    };
    union(&|l| l.function == LayerFunction::Outline)
        .or_else(|| union(&|l| l.function == LayerFunction::Copper))
        .or_else(|| union(&|_| true))
}

/// Why a file produced no layer.
#[derive(Debug)]
pub enum FileError {
    /// Not Gerber or Excellon data (binary, logs, PDFs, ...), or a drill file with
    /// no holes.
    NotFabricationData(&'static str),
    /// Looked like Gerber data but could not be parsed.
    Invalid(ExtractError),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFabricationData(reason) => f.write_str(reason),
            Self::Invalid(e) => write!(f, "{e}"),
        }
    }
}

/// Parse one fabrication file. Gerber (RS-274X) is tried first, then Excellon drill.
pub fn parse_file(name: &str, data: &[u8]) -> Result<GerberLayer, FileError> {
    let content =
        std::str::from_utf8(data).map_err(|_| FileError::NotFabricationData("not a text file"))?;

    // Every RS-274X file declares its coordinate format (%FSLAX..., %FSTAX...).
    let declares_format = ["FSLA", "FSTA", "FSLI", "FSTI"]
        .iter()
        .any(|fs| content.contains(fs));
    if declares_format {
        return parse_single_gerber(name, content)
            .map(|(layer_type, output)| GerberLayer::new(name, layer_type, output))
            .map_err(FileError::Invalid);
    }

    match excellon::parse_excellon(content) {
        Some(drawings) if !drawings.is_empty() => Ok(GerberLayer::new(
            name,
            GerberLayerType::Drills,
            GerberLayerOutput {
                drawings,
                ..Default::default()
            },
        )),
        Some(_) => Err(FileError::NotFabricationData(
            "Excellon drill file with no drill hits",
        )),
        None => Err(FileError::NotFabricationData(
            "not a Gerber or Excellon file",
        )),
    }
}

/// Parse a single Gerber file, returning its detected layer type and geometry.
fn parse_single_gerber(
    filename: &str,
    content: &str,
) -> Result<(GerberLayerType, GerberLayerOutput), ExtractError> {
    // Quick sanity check — Gerber files should contain at least one * terminator
    if !content.contains('*') {
        return Err(ExtractError::ParseError(
            "Not a Gerber file (no * terminator)".into(),
        ));
    }

    let tokens = lexer::tokenize(content);
    if tokens.is_empty() {
        return Err(ExtractError::ParseError("Empty Gerber file".into()));
    }

    let cmds = commands::parse_commands(&tokens)?;

    // Determine layer type: first try X2 attributes from file content
    let layer_type = detect_layer_type(filename, &cmds);

    let output = interpreter::interpret(&cmds)?;

    Ok((layer_type, output))
}

/// Detect layer type by checking X2 attributes first, then falling back to filename.
fn detect_layer_type(filename: &str, cmds: &[GerberCommand]) -> GerberLayerType {
    // Check for X2 FileFunction attribute in the commands
    for cmd in cmds {
        if let GerberCommand::FileFunction(func) = cmd {
            let layer_type = layers::identify_from_x2(func);
            if layer_type != GerberLayerType::Unknown {
                return layer_type;
            }
        }
    }

    // Fall back to filename-based identification
    layers::identify_from_filename(filename)
}

/// Convert Drawing primitives to Track primitives (for copper layers).
fn drawing_to_track(drawing: &Drawing) -> Option<Track> {
    match drawing {
        Drawing::Segment { start, end, width } => Some(Track::Segment {
            start: *start,
            end: *end,
            width: *width,
            net: None,
            drillsize: None,
        }),
        Drawing::Arc {
            start,
            radius,
            startangle,
            endangle,
            width,
        } => Some(Track::Arc {
            center: *start,
            startangle: *startangle,
            endangle: *endangle,
            radius: *radius,
            width: *width,
            net: None,
        }),
        // Flashed pads (circles, rects) and polygons in copper are kept as drawings
        // but can't be directly represented as Track, so we skip them for tracks.
        _ => None,
    }
}

/// Assemble parsed layer outputs into a PcbData structure.
fn assemble_pcb_data(
    layers: Vec<GerberLayer>,
    opts: &ExtractOptions,
) -> Result<PcbData, ExtractError> {
    let mut edges: Vec<Drawing> = Vec::new();
    let mut silk_f: Vec<Drawing> = Vec::new();
    let mut silk_b: Vec<Drawing> = Vec::new();
    let mut silk_f_clear: Vec<Drawing> = Vec::new();
    let mut silk_b_clear: Vec<Drawing> = Vec::new();
    let mut drills: Vec<Drawing> = Vec::new();
    let mut tracks_f: Vec<Track> = Vec::new();
    let mut tracks_b: Vec<Track> = Vec::new();
    let mut tracks_inner: HashMap<String, Vec<Track>> = HashMap::new();
    let mut pads_f: Vec<Drawing> = Vec::new();
    let mut pads_b: Vec<Drawing> = Vec::new();
    let mut pads_inner: HashMap<String, Vec<Drawing>> = HashMap::new();

    for output in layers {
        match output.layer_type {
            GerberLayerType::BoardOutline => {
                edges.extend(output.drawings);
            }
            GerberLayerType::SilkscreenTop => {
                silk_f.extend(output.drawings);
                silk_f_clear.extend(output.clear_drawings);
            }
            GerberLayerType::SilkscreenBottom => {
                silk_b.extend(output.drawings);
                silk_b_clear.extend(output.clear_drawings);
            }
            GerberLayerType::Drills => {
                drills.extend(output.drawings);
            }
            GerberLayerType::CopperTop => {
                if opts.include_tracks {
                    for d in &output.drawings {
                        if let Some(track) = drawing_to_track(d) {
                            tracks_f.push(track);
                        } else {
                            pads_f.push(d.clone());
                        }
                    }
                }
            }
            GerberLayerType::CopperBottom => {
                if opts.include_tracks {
                    for d in &output.drawings {
                        if let Some(track) = drawing_to_track(d) {
                            tracks_b.push(track);
                        } else {
                            pads_b.push(d.clone());
                        }
                    }
                }
            }
            GerberLayerType::CopperInner(ref name) if opts.include_tracks => {
                let inner_tracks = tracks_inner.entry(name.clone()).or_default();
                let inner_pads = pads_inner.entry(name.clone()).or_default();
                for d in &output.drawings {
                    if let Some(track) = drawing_to_track(d) {
                        inner_tracks.push(track);
                    } else {
                        inner_pads.push(d.clone());
                    }
                }
            }
            // SolderMask, Unknown, etc. — skip
            _ => {}
        }
    }

    // Compute bounding box from edges
    let mut bbox = BBox::empty();
    for edge in &edges {
        expand_bbox_drawing(&mut bbox, edge);
    }
    // If no edges, compute from all geometry
    if edges.is_empty() {
        for d in silk_f.iter().chain(silk_b.iter()) {
            expand_bbox_drawing(&mut bbox, d);
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

    let copper_pads = if opts.include_tracks
        && (!pads_f.is_empty() || !pads_b.is_empty() || !pads_inner.is_empty())
    {
        Some(LayerData {
            front: pads_f,
            back: pads_b,
            inner: pads_inner,
        })
    } else {
        None
    };

    Ok(PcbData {
        edges_bbox: if bbox.minx.is_finite() {
            Some(bbox)
        } else {
            None
        },
        edges,
        drawings: Drawings {
            silkscreen: LayerData {
                front: silk_f,
                back: silk_b,
                inner: {
                    let mut m = HashMap::new();
                    if !silk_f_clear.is_empty() {
                        m.insert("F_Clear".to_string(), silk_f_clear);
                    }
                    if !silk_b_clear.is_empty() {
                        m.insert("B_Clear".to_string(), silk_b_clear);
                    }
                    m
                },
            },
            fabrication: LayerData {
                front: Vec::new(),
                back: Vec::new(),
                inner: if drills.is_empty() {
                    HashMap::new()
                } else {
                    HashMap::from([("Drills".to_string(), drills)])
                },
            },
        },
        footprints: Vec::new(),
        metadata: Metadata {
            title: String::new(),
            revision: String::new(),
            company: String::new(),
            date: String::new(),
        },
        format: None,
        bom: None,
        parser_version: None,
        ibom_version: None,
        tracks,
        copper_pads,
        zones: None,
        nets: None,
        font_data: None,
    })
}

/// Expand bounding box to include a Drawing's extents.
fn expand_bbox_drawing(bbox: &mut BBox, drawing: &Drawing) {
    match drawing {
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
        Drawing::Polygon { polygons, .. } => {
            for poly in polygons {
                for pt in poly {
                    bbox.expand_point(pt[0], pt[1]);
                }
            }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use std::io::Write;

    /// Create a minimal in-memory zip with Gerber files for testing.
    fn make_test_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    const OUTLINE_GERBER: &str = "\
%FSLAX24Y24*%
%MOMM*%
%ADD10C,0.050*%
G01*
D10*
X0Y0D02*
X500000Y0D01*
X500000Y300000D01*
X0Y300000D01*
X0Y0D01*
M02*
";

    const COPPER_TOP_GERBER: &str = "\
%FSLAX24Y24*%
%MOMM*%
%TF.FileFunction,Copper,L1,Top*%
%ADD10C,0.200*%
G01*
D10*
X10000Y10000D02*
X40000Y10000D01*
M02*
";

    const SILK_TOP_GERBER: &str = "\
%FSLAX24Y24*%
%MOMM*%
%ADD10C,0.100*%
G01*
D10*
X5000Y5000D02*
X5000Y25000D01*
M02*
";

    #[test]
    fn test_parse_gerber_zip() {
        let zip_data = make_test_zip(&[
            ("board.GKO", OUTLINE_GERBER),
            ("board.GTL", COPPER_TOP_GERBER),
            ("board.GTO", SILK_TOP_GERBER),
        ]);

        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: false,
        };

        let pcb = parse(&zip_data, &opts).unwrap();

        // Board outline: 4 segments forming a 50x30mm rectangle
        assert_eq!(pcb.edges.len(), 4);

        // Bounding box should be ~50x30mm (Y is negated: 0 to -30)
        let bb = pcb.edges_bbox.as_ref().expect("Expected bounding box");
        assert_abs_diff_eq!(bb.maxx, 50.0, epsilon = 0.1);
        assert_abs_diff_eq!(bb.miny, -30.0, epsilon = 0.1);
        assert_abs_diff_eq!(bb.maxy, 0.0, epsilon = 0.1);

        // Copper top: 1 track segment
        let tracks = pcb.tracks.unwrap();
        assert_eq!(tracks.front.len(), 1);

        // Silkscreen top: 1 drawing
        assert_eq!(pcb.drawings.silkscreen.front.len(), 1);

        // No footprints, BOM, or nets
        assert!(pcb.footprints.is_empty());
        assert!(pcb.bom.is_none());
        assert!(pcb.nets.is_none());
    }

    #[test]
    fn test_empty_zip_returns_error() {
        let zip_data = make_test_zip(&[("readme.txt", "Not a Gerber file")]);
        let opts = ExtractOptions::default();
        let result = parse(&zip_data, &opts);
        assert!(result.is_err());
    }

    #[test]
    fn test_x2_overrides_filename() {
        // File named .GBL (bottom copper) but X2 attribute says top copper
        let gerber = "\
%FSLAX24Y24*%
%MOMM*%
%TF.FileFunction,Copper,L1,Top*%
%ADD10C,0.200*%
G01*
D10*
X10000Y10000D02*
X40000Y10000D01*
M02*
";
        let zip_data = make_test_zip(&[("board.GBL", gerber)]);
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: false,
        };
        let pcb = parse(&zip_data, &opts).unwrap();
        let tracks = pcb.tracks.unwrap();

        // Should be in front (top) despite .GBL filename
        assert_eq!(tracks.front.len(), 1);
        assert!(tracks.back.is_empty());
    }

    #[test]
    fn test_tracks_not_included_when_option_off() {
        let zip_data = make_test_zip(&[("board.GTL", COPPER_TOP_GERBER)]);
        let opts = ExtractOptions {
            include_tracks: false,
            include_nets: false,
        };
        let pcb = parse(&zip_data, &opts).unwrap();
        assert!(pcb.tracks.is_none());
    }

    #[test]
    fn test_inner_copper_layers() {
        let inner_gerber = "\
%FSLAX24Y24*%
%MOMM*%
%TF.FileFunction,Copper,L2,Inr*%
%ADD10C,0.200*%
G01*
D10*
X0Y0D02*
X10000Y0D01*
M02*
";
        let zip_data = make_test_zip(&[("board.G1", inner_gerber)]);
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: false,
        };
        let pcb = parse(&zip_data, &opts).unwrap();
        let tracks = pcb.tracks.unwrap();
        assert!(!tracks.inner.is_empty());
        // X2 L2 is the first inner layer.
        assert!(tracks.inner.contains_key("In1"));
    }

    #[test]
    fn test_clear_polarity_silk() {
        // A silkscreen layer with a clear-polarity segment should store it in
        // silkscreen.inner["F_Clear"], not in silkscreen.front.
        let silk_with_clear = "\
%FSLAX24Y24*%
%MOMM*%
%TF.FileFunction,Legend,Top*%
%ADD10C,0.100*%
G01*
D10*
%LPD*%
X0Y0D02*
X10000Y0D01*
%LPC*%
X20000Y0D02*
X30000Y0D01*
M02*
";
        let zip_data = make_test_zip(&[("board.GTO", silk_with_clear)]);
        let opts = ExtractOptions::default();
        let pcb = parse(&zip_data, &opts).unwrap();

        // Dark drawings go to front
        assert_eq!(pcb.drawings.silkscreen.front.len(), 1);
        // Clear drawings go to F_Clear inner key
        let clears = pcb
            .drawings
            .silkscreen
            .inner
            .get("F_Clear")
            .expect("F_Clear key should exist");
        assert_eq!(clears.len(), 1);
    }

    #[test]
    fn test_drill_file_in_zip() {
        let drill_content = "\
M48
METRIC,TZ,000.000
T01C0.300
T02C0.800
%
T01
X5.000Y5.000
X10.000Y10.000
T02
X20.000Y20.000
M30
";
        let zip_data = make_test_zip(&[
            ("board.GKO", OUTLINE_GERBER),
            ("board.GTL", COPPER_TOP_GERBER),
            ("drill.xln", drill_content),
        ]);

        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: false,
        };

        let pcb = parse(&zip_data, &opts).unwrap();

        // Board outline and copper should still work
        assert_eq!(pcb.edges.len(), 4);
        let tracks = pcb.tracks.unwrap();
        assert_eq!(tracks.front.len(), 1);

        // Drill holes should be in fabrication.inner["Drills"]
        let drills = pcb.drawings.fabrication.inner.get("Drills").unwrap();
        assert_eq!(drills.len(), 3);

        // First drill: T01 (0.3mm dia = 0.15mm radius)
        match &drills[0] {
            Drawing::Circle {
                start,
                radius,
                filled,
                ..
            } => {
                assert_abs_diff_eq!(start[0], 5.0, epsilon = 1e-6);
                assert_abs_diff_eq!(start[1], -5.0, epsilon = 1e-6);
                assert_abs_diff_eq!(*radius, 0.15, epsilon = 1e-6);
                assert_eq!(*filled, Some(1));
            }
            _ => panic!("Expected Circle"),
        }

        // Third drill: T02 (0.8mm dia = 0.4mm radius)
        match &drills[2] {
            Drawing::Circle { radius, .. } => {
                assert_abs_diff_eq!(*radius, 0.4, epsilon = 1e-6);
            }
            _ => panic!("Expected Circle"),
        }
    }

    #[test]
    fn test_decompression_bomb_rejected() {
        // Create a ZIP whose decompressed content exceeds a small limit.
        let content = "X".repeat(2048); // 2 KB of text
        let zip_data = make_test_zip(&[("bomb.gtl", &content)]);
        let opts = ExtractOptions::default();
        // Use a limit smaller than the content to trigger bomb detection
        let result = parse_with_limit(&zip_data, &opts, 1024);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ExtractError::DecompressionBomb),
            "expected DecompressionBomb error, got: {err}"
        );
    }

    #[test]
    fn test_decompression_within_limit_succeeds() {
        // Same content but with a generous limit should not trigger bomb detection
        let gerber = "\
%FSLAX24Y24*%
%MOMM*%
%ADD10C,0.200*%
G01*
D10*
X10000Y10000D02*
X40000Y10000D01*
M02*
";
        let zip_data = make_test_zip(&[("board.GTL", gerber)]);
        let opts = ExtractOptions::default();
        let result = parse_with_limit(&zip_data, &opts, 1024 * 1024);
        assert!(result.is_ok());
    }

    #[test]
    fn test_project_from_loose_files() {
        let files: [(&str, &[u8]); 4] = [
            ("board.GKO", OUTLINE_GERBER.as_bytes()),
            ("board.GTO", SILK_TOP_GERBER.as_bytes()),
            ("board.GTL", COPPER_TOP_GERBER.as_bytes()),
            ("README.txt", b"fabricate me please"),
        ];
        let project = GerberProject::from_files(files);

        let names: Vec<&str> = project.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["board.GTL", "board.GTO", "board.GKO"]);

        let copper = &project.layers[0];
        assert_eq!(copper.function, LayerFunction::Copper);
        assert_eq!(copper.side, Some(LayerSide::Top));
        assert_eq!(copper.drawings.len(), 1);

        let bbox = project.bbox.expect("board bbox");
        assert_abs_diff_eq!(bbox.minx, 0.0, epsilon = 0.05);
        assert_abs_diff_eq!(bbox.maxx, 50.0, epsilon = 0.05);
        // Gerber Y-up coordinates are flipped to screen-space Y-down.
        assert_abs_diff_eq!(bbox.miny, -30.0, epsilon = 0.05);
        assert_abs_diff_eq!(bbox.maxy, 0.0, epsilon = 0.05);

        // A non-Gerber file is informational, not a problem.
        assert!(project.warnings.is_empty());
        assert_eq!(
            project.skipped,
            ["README.txt: not a Gerber or Excellon file"]
        );
    }

    #[test]
    fn test_project_diagnostic_severity() {
        let empty_gerber = "%FSLAX24Y24*%\n%MOMM*%\nM02*\n";
        let broken_gerber = "%FSLAX24Y24*%\n%MOMM*%\n%ADD10Q,oops*%\nM02*\n";
        let empty_drill = "M48\nMETRIC\n%\nM30\n";
        let files: [(&str, &[u8]); 4] = [
            ("board-B_Paste.gbr", empty_gerber.as_bytes()),
            ("board-F_Cu.gbr", broken_gerber.as_bytes()),
            ("board-NPTH.drl", empty_drill.as_bytes()),
            ("notes.pdf", &[0x25, 0x50, 0x44, 0x46, 0xff]),
        ];
        let project = GerberProject::from_files(files);
        assert!(project.layers.is_empty());
        assert_eq!(project.warnings.len(), 1, "{:?}", project.warnings);
        assert!(project.warnings[0].starts_with("board-F_Cu.gbr:"));
        assert_eq!(
            project.skipped,
            [
                "board-B_Paste.gbr: no geometry",
                "board-NPTH.drl: Excellon drill file with no drill hits",
                "notes.pdf: not a text file",
            ]
        );
    }

    #[test]
    fn test_diagnostic_truncates_long_reasons() {
        let long = "x".repeat(500);
        let d = diagnostic("f.gbr", &long);
        assert_eq!(d.chars().count(), "f.gbr: ".len() + MAX_REASON_CHARS + 1);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn test_board_bbox_prefers_copper_over_documentation() {
        let fab_drawing = "\
%FSLAX24Y24*%
%MOMM*%
%TF.FileFunction,Other,Drawing*%
%ADD10C,0.100*%
D10*
X-1000000Y-1000000D02*
X1000000Y1000000D01*
M02*
";
        let files: [(&str, &[u8]); 2] = [
            ("board.GTL", COPPER_TOP_GERBER.as_bytes()),
            ("fab.gbr", fab_drawing.as_bytes()),
        ];
        let bbox = GerberProject::from_files(files).bbox.expect("bbox");
        assert_abs_diff_eq!(bbox.minx, 1.0, epsilon = 1e-6);
        assert_abs_diff_eq!(bbox.maxx, 4.0, epsilon = 1e-6);
    }

    #[test]
    fn test_project_expands_zip_sources() {
        let zip_data = make_test_zip(&[
            ("gerbers/board.GTL", COPPER_TOP_GERBER),
            ("gerbers/board.GKO", OUTLINE_GERBER),
        ]);
        let project = GerberProject::from_files([("fab.zip", zip_data.as_slice())]);

        let names: Vec<&str> = project.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            ["fab.zip/gerbers/board.GTL", "fab.zip/gerbers/board.GKO"]
        );
        assert!(project.warnings.is_empty());
    }

    #[test]
    fn test_project_bbox_without_outline_uses_all_layers() {
        let project = GerberProject::from_files([("board.GTL", COPPER_TOP_GERBER.as_bytes())]);
        let bbox = project.bbox.expect("bbox from copper");
        assert_abs_diff_eq!(bbox.minx, 1.0, epsilon = 1e-6);
        assert_abs_diff_eq!(bbox.maxx, 4.0, epsilon = 1e-6);
    }

    #[test]
    fn test_project_keeps_unknown_layers_with_warning() {
        let project = GerberProject::from_files([("mystery.gbr", COPPER_TOP_GERBER.as_bytes())]);
        // X2 FileFunction identifies this despite the filename.
        assert_eq!(project.layers[0].function, LayerFunction::Copper);

        let plain = OUTLINE_GERBER.replace("%MOMM*%", "%MOMM*%\n");
        let project = GerberProject::from_files([("mystery.gbr", plain.as_bytes())]);
        assert_eq!(project.layers.len(), 1);
        assert_eq!(project.layers[0].function, LayerFunction::Unknown);
        assert_eq!(project.warnings.len(), 1);
    }

    #[test]
    fn test_parse_file_reads_excellon() {
        let drill = "M48\nMETRIC\nT1C0.8\n%\nT1\nX10.0Y10.0\nM30\n";
        let layer = parse_file("board.drl", drill.as_bytes()).expect("drill parses");
        assert_eq!(layer.function, LayerFunction::Drill);
        assert_eq!(layer.drawings.len(), 1);
    }

    #[test]
    fn test_parse_file_rejects_binary() {
        assert!(parse_file("image.png", &[0x89, 0x50, 0xff, 0xfe]).is_err());
    }
}
