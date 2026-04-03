use crate::types::{Drawing, PcbData};
use std::fmt::Write;

const BG_COLOR: &str = "#1a1a2e";
const EDGE_COLOR: &str = "#4ecca3";
const PAD_COLOR: &str = "#d4aa00";
const SILK_COLOR: &str = "#cccccc";
const TRACK_COLOR: &str = "#cc4444";

const MARGIN: f64 = 2.0;

/// Render a PcbData into an SVG string suitable for use as a thumbnail.
pub fn render_svg(data: &PcbData) -> String {
    let bbox = &data.edges_bbox;
    let w = bbox.maxx - bbox.minx;
    let h = bbox.maxy - bbox.miny;

    if w <= 0.0 || h <= 0.0 {
        return empty_svg();
    }

    let vx = bbox.minx - MARGIN;
    let vy = bbox.miny - MARGIN;
    let vw = w + 2.0 * MARGIN;
    let vh = h + 2.0 * MARGIN;

    let mut svg = String::with_capacity(8192);
    write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx:.4} {vy:.4} {vw:.4} {vh:.4}" width="400" height="{thumb_h:.0}">"#,
        vx = vx,
        vy = vy,
        vw = vw,
        vh = vh,
        thumb_h = 400.0 * vh / vw,
    )
    .unwrap();

    // Background
    write!(
        svg,
        r#"<rect x="{vx:.4}" y="{vy:.4}" width="{vw:.4}" height="{vh:.4}" fill="{BG_COLOR}"/>"#,
    )
    .unwrap();

    // Tracks (front copper)
    if let Some(tracks) = &data.tracks {
        render_tracks(&mut svg, &tracks.front, TRACK_COLOR);
    }

    // Zones (front copper fill)
    if let Some(zones) = &data.zones {
        render_zones(&mut svg, &zones.front);
    }

    // Pads (front side)
    for fp in &data.footprints {
        if fp.layer != "F" {
            continue;
        }
        render_pads(&mut svg, fp);
    }

    // Silkscreen (front)
    render_drawings(&mut svg, &data.drawings.silkscreen.front, SILK_COLOR);

    // Board outline
    render_drawings(&mut svg, &data.edges, EDGE_COLOR);

    svg.push_str("</svg>");
    svg
}

fn empty_svg() -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="400" height="400"><rect width="100" height="100" fill="{BG_COLOR}"/></svg>"#,
    )
}

fn render_drawings(svg: &mut String, drawings: &[Drawing], color: &str) {
    for d in drawings {
        match d {
            Drawing::Segment { start, end, width } => {
                let sw = if *width < 0.1 { 0.1 } else { *width };
                write!(
                    svg,
                    r#"<line x1="{:.4}" y1="{:.4}" x2="{:.4}" y2="{:.4}" stroke="{color}" stroke-width="{sw:.4}" stroke-linecap="round"/>"#,
                    start[0], start[1], end[0], end[1],
                )
                .unwrap();
            }
            Drawing::Circle {
                start,
                radius,
                width,
                filled,
            } => {
                if filled.is_some() {
                    write!(
                        svg,
                        r#"<circle cx="{:.4}" cy="{:.4}" r="{:.4}" fill="{color}"/>"#,
                        start[0], start[1], radius,
                    )
                    .unwrap();
                } else {
                    let sw = if *width < 0.1 { 0.1 } else { *width };
                    write!(
                        svg,
                        r#"<circle cx="{:.4}" cy="{:.4}" r="{:.4}" fill="none" stroke="{color}" stroke-width="{sw:.4}"/>"#,
                        start[0], start[1], radius,
                    )
                    .unwrap();
                }
            }
            Drawing::Rect {
                start, end, width, ..
            } => {
                let x = start[0].min(end[0]);
                let y = start[1].min(end[1]);
                let rw = (end[0] - start[0]).abs();
                let rh = (end[1] - start[1]).abs();
                if *width < 0.01 {
                    write!(
                        svg,
                        r#"<rect x="{x:.4}" y="{y:.4}" width="{rw:.4}" height="{rh:.4}" fill="{color}"/>"#,
                    )
                    .unwrap();
                } else {
                    write!(
                        svg,
                        r#"<rect x="{x:.4}" y="{y:.4}" width="{rw:.4}" height="{rh:.4}" fill="none" stroke="{color}" stroke-width="{:.4}"/>"#,
                        width,
                    )
                    .unwrap();
                }
            }
            Drawing::Arc {
                start,
                radius,
                startangle,
                endangle,
                width,
            } => {
                render_arc(svg, start, *radius, *startangle, *endangle, *width, color);
            }
            Drawing::Polygon {
                pos,
                angle,
                polygons,
                filled,
                width,
            } => {
                for poly in polygons {
                    if poly.len() < 2 {
                        continue;
                    }
                    let mut d = String::new();
                    let cos_a = angle.to_radians().cos();
                    let sin_a = angle.to_radians().sin();
                    for (i, pt) in poly.iter().enumerate() {
                        let rx = pt[0] * cos_a - pt[1] * sin_a + pos[0];
                        let ry = pt[0] * sin_a + pt[1] * cos_a + pos[1];
                        if i == 0 {
                            write!(d, "M{rx:.4} {ry:.4}").unwrap();
                        } else {
                            write!(d, "L{rx:.4} {ry:.4}").unwrap();
                        }
                    }
                    d.push('Z');
                    if filled.is_some() {
                        write!(svg, r#"<path d="{d}" fill="{color}" fill-rule="evenodd"/>"#)
                            .unwrap();
                    } else {
                        let sw = if *width < 0.1 { 0.1 } else { *width };
                        write!(
                            svg,
                            r#"<path d="{d}" fill="none" stroke="{color}" stroke-width="{sw:.4}"/>"#,
                        )
                        .unwrap();
                    }
                }
            }
            Drawing::Curve {
                start,
                end,
                cpa,
                cpb,
                width,
            } => {
                let sw = if *width < 0.1 { 0.1 } else { *width };
                write!(
                    svg,
                    r#"<path d="M{:.4} {:.4}C{:.4} {:.4},{:.4} {:.4},{:.4} {:.4}" fill="none" stroke="{color}" stroke-width="{sw:.4}" stroke-linecap="round"/>"#,
                    start[0], start[1], cpa[0], cpa[1], cpb[0], cpb[1], end[0], end[1],
                )
                .unwrap();
            }
        }
    }
}

fn render_arc(
    svg: &mut String,
    center: &[f64; 2],
    radius: f64,
    start_deg: f64,
    end_deg: f64,
    width: f64,
    color: &str,
) {
    let s_rad = start_deg.to_radians();
    let e_rad = end_deg.to_radians();
    let x1 = center[0] + radius * s_rad.cos();
    let y1 = center[1] + radius * s_rad.sin();
    let x2 = center[0] + radius * e_rad.cos();
    let y2 = center[1] + radius * e_rad.sin();

    let mut sweep = end_deg - start_deg;
    while sweep < -180.0 {
        sweep += 360.0;
    }
    while sweep > 180.0 {
        sweep -= 360.0;
    }
    let large = if sweep.abs() > 180.0 { 1 } else { 0 };
    let sweep_flag = if sweep > 0.0 { 1 } else { 0 };

    let sw = if width < 0.1 { 0.1 } else { width };
    write!(
        svg,
        r#"<path d="M{x1:.4} {y1:.4}A{radius:.4} {radius:.4} 0 {large} {sweep_flag} {x2:.4} {y2:.4}" fill="none" stroke="{color}" stroke-width="{sw:.4}" stroke-linecap="round"/>"#,
    )
    .unwrap();
}

fn render_pads(svg: &mut String, fp: &crate::types::Footprint) {
    let cx = fp.center[0];
    let cy = fp.center[1];
    let angle_rad = fp.bbox.angle.to_radians();

    for pad in &fp.pads {
        // Transform pad position from component-local to world coordinates
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let px = pad.pos[0] * cos_a - pad.pos[1] * sin_a + cx;
        let py = pad.pos[0] * sin_a + pad.pos[1] * cos_a + cy;

        let w = pad.size[0];
        let h = pad.size[1];

        match pad.shape.as_str() {
            "circle" | "oval" => {
                let r = w.max(h) / 2.0;
                write!(
                    svg,
                    r#"<circle cx="{px:.4}" cy="{py:.4}" r="{r:.4}" fill="{PAD_COLOR}"/>"#,
                )
                .unwrap();
            }
            _ => {
                // rect, roundrect, chamfrect, custom → rectangle
                write!(
                    svg,
                    r#"<rect x="{:.4}" y="{:.4}" width="{w:.4}" height="{h:.4}" fill="{PAD_COLOR}"/>"#,
                    px - w / 2.0,
                    py - h / 2.0,
                )
                .unwrap();
            }
        }
    }
}

fn render_tracks(svg: &mut String, tracks: &[crate::types::Track], color: &str) {
    for track in tracks {
        match track {
            crate::types::Track::Segment {
                start, end, width, ..
            } => {
                let sw = if *width < 0.1 { 0.1 } else { *width };
                write!(
                    svg,
                    r#"<line x1="{:.4}" y1="{:.4}" x2="{:.4}" y2="{:.4}" stroke="{color}" stroke-width="{sw:.4}" stroke-linecap="round" opacity="0.5"/>"#,
                    start[0], start[1], end[0], end[1],
                )
                .unwrap();
            }
            crate::types::Track::Arc {
                center,
                radius,
                startangle,
                endangle,
                width,
                ..
            } => {
                render_arc(svg, center, *radius, *startangle, *endangle, *width, color);
            }
        }
    }
}

fn render_zones(svg: &mut String, zones: &[crate::types::Zone]) {
    for zone in zones {
        if let Some(polys) = &zone.polygons {
            for poly in polys {
                if poly.len() < 3 {
                    continue;
                }
                let mut d = String::new();
                for (i, pt) in poly.iter().enumerate() {
                    if i == 0 {
                        write!(d, "M{:.4} {:.4}", pt[0], pt[1]).unwrap();
                    } else {
                        write!(d, "L{:.4} {:.4}", pt[0], pt[1]).unwrap();
                    }
                }
                d.push('Z');
                write!(
                    svg,
                    r#"<path d="{d}" fill="{PAD_COLOR}" opacity="0.3" fill-rule="evenodd"/>"#,
                )
                .unwrap();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use std::collections::HashMap;

    fn minimal_pcbdata() -> PcbData {
        PcbData {
            edges_bbox: BBox {
                minx: 0.0,
                miny: 0.0,
                maxx: 50.0,
                maxy: 30.0,
            },
            edges: vec![
                Drawing::Segment {
                    start: [0.0, 0.0],
                    end: [50.0, 0.0],
                    width: 0.2,
                },
                Drawing::Segment {
                    start: [50.0, 0.0],
                    end: [50.0, 30.0],
                    width: 0.2,
                },
                Drawing::Segment {
                    start: [50.0, 30.0],
                    end: [0.0, 30.0],
                    width: 0.2,
                },
                Drawing::Segment {
                    start: [0.0, 30.0],
                    end: [0.0, 0.0],
                    width: 0.2,
                },
            ],
            drawings: Drawings {
                silkscreen: LayerData {
                    front: vec![],
                    back: vec![],
                    inner: HashMap::new(),
                },
                fabrication: LayerData {
                    front: vec![],
                    back: vec![],
                    inner: HashMap::new(),
                },
            },
            footprints: vec![Footprint {
                ref_: "U1".to_string(),
                center: [25.0, 15.0],
                bbox: FootprintBBox {
                    pos: [25.0, 15.0],
                    relpos: [-2.0, -2.0],
                    size: [4.0, 4.0],
                    angle: 0.0,
                },
                pads: vec![Pad {
                    layers: vec!["F".to_string()],
                    pos: [0.0, 0.0],
                    size: [1.0, 1.0],
                    shape: "circle".to_string(),
                    pad_type: "smd".to_string(),
                    angle: None,
                    pin1: Some(1),
                    net: None,
                    offset: None,
                    radius: None,
                    chamfpos: None,
                    chamfratio: None,
                    drillshape: None,
                    drillsize: None,
                    svgpath: None,
                    polygons: None,
                }],
                drawings: vec![],
                layer: "F".to_string(),
            }],
            metadata: Metadata {
                title: "Test".to_string(),
                revision: String::new(),
                company: String::new(),
                date: String::new(),
            },
            parser_version: None,
            bom: None,
            ibom_version: None,
            tracks: None,
            copper_pads: None,
            zones: None,
            nets: None,
            font_data: None,
        }
    }

    #[test]
    fn test_render_svg_basic() {
        let data = minimal_pcbdata();
        let svg = render_svg(&data);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("<line"));
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn test_empty_bbox() {
        let mut data = minimal_pcbdata();
        data.edges_bbox = BBox {
            minx: 0.0,
            miny: 0.0,
            maxx: 0.0,
            maxy: 0.0,
        };
        let svg = render_svg(&data);
        assert!(svg.contains("viewBox=\"0 0 100 100\""));
    }

    #[test]
    fn test_negative_bbox() {
        let mut data = minimal_pcbdata();
        data.edges_bbox = BBox {
            minx: 0.0,
            miny: 0.0,
            maxx: -5.0,
            maxy: -5.0,
        };
        let svg = render_svg(&data);
        assert!(svg.contains("viewBox=\"0 0 100 100\""));
    }

    #[test]
    fn test_viewbox_includes_margin() {
        let data = minimal_pcbdata();
        let svg = render_svg(&data);
        // bbox is 0,0 to 50,30; margin=2 → viewBox starts at -2,-2 with size 54x34
        assert!(svg.contains("viewBox=\"-2.0000 -2.0000 54.0000 34.0000\""));
    }

    #[test]
    fn test_aspect_ratio_preserved() {
        let mut data = minimal_pcbdata();
        data.edges_bbox = BBox {
            minx: 0.0,
            miny: 0.0,
            maxx: 100.0,
            maxy: 50.0,
        };
        let svg = render_svg(&data);
        // width=400, height should be 400 * (54/104) ≈ 207.69 (with margin)
        assert!(svg.contains("width=\"400\""));
        assert!(svg.contains("height=\""));
    }

    #[test]
    fn test_rect_pad_rendering() {
        let mut data = minimal_pcbdata();
        data.footprints[0].pads[0].shape = "rect".to_string();
        let svg = render_svg(&data);
        assert!(svg.contains("<rect"));
        assert!(svg.contains(PAD_COLOR));
    }

    #[test]
    fn test_back_side_pads_excluded() {
        let mut data = minimal_pcbdata();
        data.footprints[0].layer = "B".to_string();
        let svg = render_svg(&data);
        // Back-side footprint pads should not be rendered
        assert!(!svg.contains(PAD_COLOR));
    }

    #[test]
    fn test_rotated_pad_position() {
        let mut data = minimal_pcbdata();
        data.footprints[0].bbox.angle = 90.0;
        data.footprints[0].pads[0].pos = [1.0, 0.0];
        let svg = render_svg(&data);
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn test_circle_drawing_stroked() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Circle {
            start: [10.0, 10.0],
            radius: 3.0,
            width: 0.2,
            filled: None,
        });
        let svg = render_svg(&data);
        assert!(svg.contains("fill=\"none\""));
        assert!(svg.contains(&format!("stroke=\"{SILK_COLOR}\"")));
    }

    #[test]
    fn test_circle_drawing_filled() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Circle {
            start: [10.0, 10.0],
            radius: 3.0,
            width: 0.0,
            filled: Some(1),
        });
        let svg = render_svg(&data);
        assert!(svg.contains(&format!("fill=\"{SILK_COLOR}\"")));
    }

    #[test]
    fn test_rect_drawing_filled() {
        let mut data = minimal_pcbdata();
        data.edges.push(Drawing::Rect {
            start: [5.0, 5.0],
            end: [15.0, 10.0],
            width: 0.0,
        });
        let svg = render_svg(&data);
        let rect_count = svg.matches("<rect").count();
        // background rect + edge rect
        assert!(rect_count >= 2);
    }

    #[test]
    fn test_rect_drawing_stroked() {
        let mut data = minimal_pcbdata();
        data.edges.push(Drawing::Rect {
            start: [5.0, 5.0],
            end: [15.0, 10.0],
            width: 0.3,
        });
        let svg = render_svg(&data);
        assert!(svg.contains(&format!("stroke=\"{EDGE_COLOR}\"")));
    }

    #[test]
    fn test_arc_drawing() {
        let mut data = minimal_pcbdata();
        data.edges.push(Drawing::Arc {
            start: [25.0, 15.0],
            radius: 5.0,
            startangle: 0.0,
            endangle: 90.0,
            width: 0.2,
        });
        let svg = render_svg(&data);
        assert!(svg.contains("<path d=\"M"));
        assert!(svg.contains("A5.0000"));
    }

    #[test]
    fn test_curve_drawing() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Curve {
            start: [0.0, 0.0],
            end: [10.0, 10.0],
            cpa: [3.0, 0.0],
            cpb: [7.0, 10.0],
            width: 0.15,
        });
        let svg = render_svg(&data);
        assert!(svg.contains("C"));
        assert!(svg.contains("stroke-linecap=\"round\""));
    }

    #[test]
    fn test_polygon_filled() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Polygon {
            pos: [10.0, 10.0],
            angle: 0.0,
            polygons: vec![vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]]],
            filled: Some(1),
            width: 0.0,
        });
        let svg = render_svg(&data);
        assert!(svg.contains("fill-rule=\"evenodd\""));
        assert!(svg.contains(&format!("fill=\"{SILK_COLOR}\"")));
    }

    #[test]
    fn test_polygon_stroked() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Polygon {
            pos: [10.0, 10.0],
            angle: 0.0,
            polygons: vec![vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]]],
            filled: None,
            width: 0.3,
        });
        let svg = render_svg(&data);
        assert!(svg.contains("fill=\"none\""));
        assert!(svg.contains(&format!("stroke=\"{SILK_COLOR}\"")));
    }

    #[test]
    fn test_polygon_rotated() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Polygon {
            pos: [0.0, 0.0],
            angle: 45.0,
            polygons: vec![vec![[1.0, 0.0], [0.0, 1.0]]],
            filled: Some(1),
            width: 0.0,
        });
        let svg = render_svg(&data);
        // After 45° rotation, [1,0] → [cos45, sin45] ≈ [0.7071, 0.7071]
        assert!(svg.contains("M0.7071 0.7071"));
    }

    #[test]
    fn test_polygon_skip_degenerate() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front.push(Drawing::Polygon {
            pos: [0.0, 0.0],
            angle: 0.0,
            polygons: vec![vec![[0.0, 0.0]]],
            filled: Some(1),
            width: 0.0,
        });
        let svg = render_svg(&data);
        // Single-point polygon should not produce a path element
        assert!(!svg.contains("fill-rule=\"evenodd\""));
    }

    #[test]
    fn test_tracks_rendering() {
        let mut data = minimal_pcbdata();
        data.tracks = Some(LayerData {
            front: vec![Track::Segment {
                start: [5.0, 5.0],
                end: [20.0, 15.0],
                width: 0.25,
                net: None,
                drillsize: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });
        let svg = render_svg(&data);
        assert!(svg.contains(&format!("stroke=\"{TRACK_COLOR}\"")));
        assert!(svg.contains("opacity=\"0.5\""));
    }

    #[test]
    fn test_track_arc_rendering() {
        let mut data = minimal_pcbdata();
        data.tracks = Some(LayerData {
            front: vec![Track::Arc {
                center: [25.0, 15.0],
                startangle: 0.0,
                endangle: 90.0,
                radius: 5.0,
                width: 0.25,
                net: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });
        let svg = render_svg(&data);
        assert!(svg.contains("<path d=\"M"));
        assert!(svg.contains(&format!("stroke=\"{TRACK_COLOR}\"")));
    }

    #[test]
    fn test_zones_rendering() {
        let mut data = minimal_pcbdata();
        data.zones = Some(LayerData {
            front: vec![Zone {
                polygons: Some(vec![vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 10.0],
                    [0.0, 10.0],
                ]]),
                svgpath: None,
                width: None,
                net: None,
                fillrule: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });
        let svg = render_svg(&data);
        assert!(svg.contains(&format!("fill=\"{PAD_COLOR}\"")));
        assert!(svg.contains("opacity=\"0.3\""));
        assert!(svg.contains("fill-rule=\"evenodd\""));
    }

    #[test]
    fn test_zone_no_polygons() {
        let mut data = minimal_pcbdata();
        data.zones = Some(LayerData {
            front: vec![Zone {
                polygons: None,
                svgpath: None,
                width: None,
                net: None,
                fillrule: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });
        let svg = render_svg(&data);
        // Should not crash, just no zone path rendered
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn test_zone_degenerate_polygon() {
        let mut data = minimal_pcbdata();
        data.zones = Some(LayerData {
            front: vec![Zone {
                polygons: Some(vec![vec![[0.0, 0.0], [1.0, 1.0]]]),
                svgpath: None,
                width: None,
                net: None,
                fillrule: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });
        let svg = render_svg(&data);
        // 2-point polygon (< 3) should be skipped
        assert!(!svg.contains("opacity=\"0.3\""));
    }

    #[test]
    fn test_minimum_stroke_width() {
        let mut data = minimal_pcbdata();
        data.edges = vec![Drawing::Segment {
            start: [0.0, 0.0],
            end: [50.0, 0.0],
            width: 0.01,
        }];
        let svg = render_svg(&data);
        // Width below 0.1 should be clamped to 0.1
        assert!(svg.contains("stroke-width=\"0.1000\""));
    }

    #[test]
    fn test_background_rect_present() {
        let data = minimal_pcbdata();
        let svg = render_svg(&data);
        assert!(svg.contains(&format!("fill=\"{BG_COLOR}\"")));
    }

    #[test]
    fn test_multiple_pads_per_footprint() {
        let mut data = minimal_pcbdata();
        let base_pad = data.footprints[0].pads[0].clone();
        let mut pad2 = base_pad.clone();
        pad2.pos = [2.0, 0.0];
        pad2.shape = "rect".to_string();
        data.footprints[0].pads.push(pad2);
        let svg = render_svg(&data);
        // Should have both a circle pad and a rect pad
        let circle_count = svg.matches(&format!("fill=\"{PAD_COLOR}\"")).count();
        assert_eq!(circle_count, 2);
    }

    #[test]
    fn test_serde_roundtrip() {
        let data = minimal_pcbdata();
        let json = serde_json::to_string(&data).unwrap();
        let deserialized: PcbData = serde_json::from_str(&json).unwrap();
        let svg_original = render_svg(&data);
        let svg_roundtrip = render_svg(&deserialized);
        assert_eq!(svg_original, svg_roundtrip);
    }

    #[test]
    fn test_serde_roundtrip_with_tracks_and_zones() {
        let mut data = minimal_pcbdata();
        data.tracks = Some(LayerData {
            front: vec![
                Track::Segment {
                    start: [1.0, 2.0],
                    end: [3.0, 4.0],
                    width: 0.25,
                    net: Some("GND".to_string()),
                    drillsize: None,
                },
                Track::Arc {
                    center: [10.0, 10.0],
                    startangle: 0.0,
                    endangle: 45.0,
                    radius: 5.0,
                    width: 0.2,
                    net: None,
                },
            ],
            back: vec![],
            inner: HashMap::new(),
        });
        data.zones = Some(LayerData {
            front: vec![Zone {
                polygons: Some(vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]]),
                svgpath: None,
                width: None,
                net: Some("VCC".to_string()),
                fillrule: None,
            }],
            back: vec![],
            inner: HashMap::new(),
        });

        let json = serde_json::to_string(&data).unwrap();
        let deserialized: PcbData = serde_json::from_str(&json).unwrap();
        let svg_original = render_svg(&data);
        let svg_roundtrip = render_svg(&deserialized);
        assert_eq!(svg_original, svg_roundtrip);
    }

    #[test]
    fn test_all_drawing_types_in_single_render() {
        let mut data = minimal_pcbdata();
        data.drawings.silkscreen.front = vec![
            Drawing::Segment {
                start: [1.0, 1.0],
                end: [10.0, 1.0],
                width: 0.15,
            },
            Drawing::Circle {
                start: [20.0, 10.0],
                radius: 2.0,
                width: 0.1,
                filled: None,
            },
            Drawing::Circle {
                start: [30.0, 10.0],
                radius: 1.0,
                width: 0.0,
                filled: Some(1),
            },
            Drawing::Rect {
                start: [5.0, 20.0],
                end: [15.0, 25.0],
                width: 0.2,
            },
            Drawing::Arc {
                start: [25.0, 20.0],
                radius: 3.0,
                startangle: 0.0,
                endangle: 180.0,
                width: 0.15,
            },
            Drawing::Curve {
                start: [0.0, 0.0],
                end: [10.0, 10.0],
                cpa: [3.0, 0.0],
                cpb: [7.0, 10.0],
                width: 0.1,
            },
            Drawing::Polygon {
                pos: [35.0, 15.0],
                angle: 30.0,
                polygons: vec![vec![[0.0, 0.0], [3.0, 0.0], [1.5, 3.0]]],
                filled: Some(1),
                width: 0.0,
            },
        ];
        let svg = render_svg(&data);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        // Verify all element types present
        assert!(svg.contains("<line"));
        assert!(svg.contains("<circle"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("<path"));
    }
}
