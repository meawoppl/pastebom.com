mod apertures;
pub mod commands;
pub mod coord;
pub mod excellon;
pub mod interpreter;
pub mod layers;
pub mod lexer;
pub mod macros;

use std::collections::HashMap;
use std::io::Cursor;

use crate::error::ExtractError;
use crate::types::*;
use crate::ExtractOptions;

use self::commands::GerberCommand;
use self::interpreter::GerberLayerOutput;
use self::layers::GerberLayerType;

/// Parse a zip file containing Gerber files into PcbData.
pub fn parse(data: &[u8], opts: &ExtractOptions) -> Result<PcbData, ExtractError> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut layer_outputs: Vec<(GerberLayerType, GerberLayerOutput)> = Vec::new();
    let mut had_gerber = false;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }

        let filename = file.name().to_string();

        // Read file content
        let mut content = String::new();
        use std::io::Read;
        if file.read_to_string(&mut content).is_err() {
            // Binary file or encoding error — skip
            continue;
        }

        // Try to parse as Gerber first, then fall back to Excellon drill
        match parse_single_gerber(&filename, &content) {
            Ok((layer_type, output)) => {
                had_gerber = true;
                if layer_type != GerberLayerType::Unknown || !output.drawings.is_empty() {
                    layer_outputs.push((layer_type, output));
                }
            }
            Err(_) => {
                // Not a valid Gerber file — try Excellon drill format
                if let Some(drawings) = excellon::parse_excellon(&content) {
                    if !drawings.is_empty() {
                        had_gerber = true;
                        layer_outputs.push((
                            GerberLayerType::Drills,
                            GerberLayerOutput {
                                drawings,
                                ..Default::default()
                            },
                        ));
                    }
                }
            }
        }
    }

    if !had_gerber {
        return Err(ExtractError::ParseError(
            "No Gerber files found in zip".into(),
        ));
    }

    assemble_pcb_data(layer_outputs, opts)
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
    layer_outputs: Vec<(GerberLayerType, GerberLayerOutput)>,
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

    for (layer_type, output) in layer_outputs {
        match layer_type {
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
            GerberLayerType::CopperInner(ref name) => {
                if opts.include_tracks {
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
        edges_bbox: bbox,
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
        bom: None,
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

        // Bounding box should be ~50x30mm
        assert!((pcb.edges_bbox.maxx - 50.0).abs() < 0.1);
        assert!((pcb.edges_bbox.maxy - 30.0).abs() < 0.1);

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
        assert!(tracks.inner.contains_key("In2"));
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
                assert!((start[0] - 5.0).abs() < 1e-6);
                assert!((start[1] - 5.0).abs() < 1e-6);
                assert!((radius - 0.15).abs() < 1e-6);
                assert_eq!(*filled, Some(1));
            }
            _ => panic!("Expected Circle"),
        }

        // Third drill: T02 (0.8mm dia = 0.4mm radius)
        match &drills[2] {
            Drawing::Circle { radius, .. } => {
                assert!((radius - 0.4).abs() < 1e-6);
            }
            _ => panic!("Expected Circle"),
        }
    }
}
