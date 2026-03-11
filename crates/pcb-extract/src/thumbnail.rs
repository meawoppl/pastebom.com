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
        // Should have board outline segments
        assert!(svg.contains("<line"));
        // Should have a pad circle
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
}
