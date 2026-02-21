use std::collections::HashMap;
use std::f64::consts::PI;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Path2d};

use crate::pcbdata::*;
use crate::state::Settings;

fn deg2rad(deg: f64) -> f64 {
    deg * PI / 180.0
}

#[derive(Clone)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub s: f64,
    pub panx: f64,
    pub pany: f64,
    pub zoom: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            s: 1.0,
            panx: 0.0,
            pany: 0.0,
            zoom: 1.0,
        }
    }
}

pub struct LayerCanvases {
    pub bg: HtmlCanvasElement,
    pub fab: HtmlCanvasElement,
    pub silk: HtmlCanvasElement,
    pub highlight: HtmlCanvasElement,
    pub layer: String,
    pub transform: Transform,
}

impl LayerCanvases {
    pub fn all_canvases(&self) -> [&HtmlCanvasElement; 4] {
        [&self.bg, &self.fab, &self.silk, &self.highlight]
    }
}

#[derive(Clone)]
pub struct Colors {
    pub pcb_edge: String,
    pub pad: String,
    pub pad_hole: String,
    pub pad_highlight: String,
    pub pad_highlight_both: String,
    pub pad_highlight_marked: String,
    pub pin1_outline: String,
    pub pin1_outline_highlight: String,
    pub pin1_outline_highlight_both: String,
    pub pin1_outline_highlight_marked: String,
    pub silk_edge: String,
    pub silk_polygon: String,
    pub silk_text: String,
    pub fab_edge: String,
    pub fab_polygon: String,
    pub fab_text: String,
    pub track_front: String,
    pub track_back: String,
    pub track_highlight: String,
    pub zone_front: String,
    pub zone_back: String,
    pub zone_highlight: String,
}

impl Colors {
    pub fn from_element(el: &web_sys::Element) -> Self {
        let style = web_sys::window()
            .unwrap()
            .get_computed_style(el)
            .unwrap()
            .unwrap();
        let g = |name: &str| -> String {
            style
                .get_property_value(name)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        Self {
            pcb_edge: g("--pcb-edge-color"),
            pad: g("--pad-color"),
            pad_hole: g("--pad-hole-color"),
            pad_highlight: g("--pad-color-highlight"),
            pad_highlight_both: g("--pad-color-highlight-both"),
            pad_highlight_marked: g("--pad-color-highlight-marked"),
            pin1_outline: g("--pin1-outline-color"),
            pin1_outline_highlight: g("--pin1-outline-color-highlight"),
            pin1_outline_highlight_both: g("--pin1-outline-color-highlight-both"),
            pin1_outline_highlight_marked: g("--pin1-outline-color-highlight-marked"),
            silk_edge: g("--silkscreen-edge-color"),
            silk_polygon: g("--silkscreen-polygon-color"),
            silk_text: g("--silkscreen-text-color"),
            fab_edge: g("--fabrication-edge-color"),
            fab_polygon: g("--fabrication-polygon-color"),
            fab_text: g("--fabrication-text-color"),
            track_front: g("--track-color-front"),
            track_back: g("--track-color-back"),
            track_highlight: g("--track-color-highlight"),
            zone_front: g("--zone-color-front"),
            zone_back: g("--zone-color-back"),
            zone_highlight: g("--zone-color-highlight"),
        }
    }
}

/// Cache for Path2D objects (keyed by a unique string identifier)
pub struct PathCache {
    pads: HashMap<String, Path2d>,
}

impl PathCache {
    pub fn new() -> Self {
        Self {
            pads: HashMap::new(),
        }
    }
}

// ─── Path Builders ──────────────────────────────────────────────────

fn get_chamfered_rect_path(size: [f64; 2], radius: f64, chamfpos: u8, chamfratio: f64) -> Path2d {
    let path = Path2d::new().unwrap();
    let width = size[0];
    let height = size[1];
    let x = width * -0.5;
    let y = height * -0.5;
    let chamf_offset = width.min(height) * chamfratio;

    path.move_to(x, 0.0);

    if chamfpos & 4 != 0 {
        path.line_to(x, y + height - chamf_offset);
        path.line_to(x + chamf_offset, y + height);
        path.line_to(0.0, y + height);
    } else {
        path.arc_to(x, y + height, x + width, y + height, radius)
            .unwrap();
    }

    if chamfpos & 8 != 0 {
        path.line_to(x + width - chamf_offset, y + height);
        path.line_to(x + width, y + height - chamf_offset);
        path.line_to(x + width, 0.0);
    } else {
        path.arc_to(x + width, y + height, x + width, y, radius)
            .unwrap();
    }

    if chamfpos & 2 != 0 {
        path.line_to(x + width, y + chamf_offset);
        path.line_to(x + width - chamf_offset, y);
        path.line_to(0.0, y);
    } else {
        path.arc_to(x + width, y, x, y, radius).unwrap();
    }

    if chamfpos & 1 != 0 {
        path.line_to(x + chamf_offset, y);
        path.line_to(x, y + chamf_offset);
        path.line_to(x, 0.0);
    } else {
        path.arc_to(x, y, x, y + height, radius).unwrap();
    }

    path.close_path();
    path
}

fn get_oblong_path(size: [f64; 2]) -> Path2d {
    get_chamfered_rect_path(size, size[0].min(size[1]) / 2.0, 0, 0.0)
}

fn get_circle_path(radius: f64) -> Path2d {
    let path = Path2d::new().unwrap();
    path.arc(0.0, 0.0, radius, 0.0, 2.0 * PI).unwrap();
    path.close_path();
    path
}

fn get_polygons_path(polygons: &[Vec<[f64; 2]>]) -> Path2d {
    let path = Path2d::new().unwrap();
    for polygon in polygons {
        if let Some(first) = polygon.first() {
            path.move_to(first[0], first[1]);
            for pt in &polygon[1..] {
                path.line_to(pt[0], pt[1]);
            }
            path.close_path();
        }
    }
    path
}

fn get_pad_path(pad: &Pad, cache: &mut PathCache, key: &str) -> Path2d {
    if let Some(p) = cache.pads.get(key) {
        return p.clone();
    }
    let path = match pad.shape.as_str() {
        "rect" => {
            let p = Path2d::new().unwrap();
            p.rect(
                -pad.size[0] * 0.5,
                -pad.size[1] * 0.5,
                pad.size[0],
                pad.size[1],
            );
            p
        }
        "oval" => get_oblong_path(pad.size),
        "circle" => get_circle_path(pad.size[0] / 2.0),
        "roundrect" => get_chamfered_rect_path(pad.size, pad.radius.unwrap_or(0.0), 0, 0.0),
        "chamfrect" => get_chamfered_rect_path(
            pad.size,
            pad.radius.unwrap_or(0.0),
            pad.chamfpos.unwrap_or(0),
            pad.chamfratio.unwrap_or(0.0),
        ),
        "custom" => {
            if let Some(ref svgpath) = pad.svgpath {
                Path2d::new_with_path_string(svgpath).unwrap_or_else(|_| Path2d::new().unwrap())
            } else if let Some(ref polygons) = pad.polygons {
                get_polygons_path(polygons)
            } else {
                Path2d::new().unwrap()
            }
        }
        _ => Path2d::new().unwrap(),
    };
    cache.pads.insert(key.to_string(), path.clone());
    path
}

// ─── Drawing Functions ──────────────────────────────────────────────

fn draw_edge(ctx: &CanvasRenderingContext2d, scalefactor: f64, drawing: &Drawing, color: &str) {
    ctx.set_stroke_style_str(color);
    ctx.set_fill_style_str(color);
    ctx.set_line_cap("round");
    ctx.set_line_join("round");

    match drawing {
        Drawing::Segment { start, end, width } => {
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.begin_path();
            ctx.move_to(start[0], start[1]);
            ctx.line_to(end[0], end[1]);
            ctx.stroke();
        }
        Drawing::Rect { start, end, width } => {
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.begin_path();
            ctx.move_to(start[0], start[1]);
            ctx.line_to(start[0], end[1]);
            ctx.line_to(end[0], end[1]);
            ctx.line_to(end[0], start[1]);
            ctx.line_to(start[0], start[1]);
            ctx.stroke();
        }
        Drawing::Arc {
            start,
            radius,
            startangle,
            endangle,
            width,
        } => {
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.begin_path();
            ctx.arc(
                start[0],
                start[1],
                *radius,
                deg2rad(*startangle),
                deg2rad(*endangle),
            )
            .unwrap();
            ctx.stroke();
        }
        Drawing::Circle {
            start,
            radius,
            width,
            filled,
        } => {
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.begin_path();
            ctx.arc(start[0], start[1], *radius, 0.0, 2.0 * PI).unwrap();
            ctx.close_path();
            if filled.is_some_and(|f| f != 0) {
                ctx.fill();
            } else {
                ctx.stroke();
            }
        }
        Drawing::Curve {
            start,
            end,
            cpa,
            cpb,
            width,
        } => {
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.begin_path();
            ctx.move_to(start[0], start[1]);
            ctx.bezier_curve_to(cpa[0], cpa[1], cpb[0], cpb[1], end[0], end[1]);
            ctx.stroke();
        }
        Drawing::Polygon { .. } => {
            draw_polygon_shape(ctx, scalefactor, drawing, color);
        }
    }
}

fn draw_polygon_shape(
    ctx: &CanvasRenderingContext2d,
    scalefactor: f64,
    drawing: &Drawing,
    color: &str,
) {
    if let Drawing::Polygon {
        pos,
        angle,
        polygons,
        filled,
        width,
    } = drawing
    {
        ctx.save();
        ctx.translate(pos[0], pos[1]).unwrap();
        ctx.rotate(deg2rad(-angle)).unwrap();
        let path = get_polygons_path(polygons);
        if filled.is_none_or(|f| f != 0) {
            ctx.set_fill_style_str(color);
            ctx.fill_with_path_2d(&path);
        } else {
            ctx.set_stroke_style_str(color);
            ctx.set_line_width((1.0 / scalefactor).max(*width));
            ctx.set_line_cap("round");
            ctx.set_line_join("round");
            ctx.stroke_with_path(&path);
        }
        ctx.restore();
    }
}

fn draw_text(
    ctx: &CanvasRenderingContext2d,
    text: &TextDrawing,
    color: &str,
    settings: &Settings,
    font_data: Option<&FontData>,
) {
    if text.is_ref.is_some() && !settings.render_references {
        return;
    }
    if text.val.is_some() && !settings.render_values {
        return;
    }

    ctx.save();
    ctx.set_fill_style_str(color);
    ctx.set_stroke_style_str(color);
    ctx.set_line_cap("round");
    ctx.set_line_join("round");

    if let Some(ref svgpath) = text.svgpath {
        if let Ok(path) = Path2d::new_with_path_string(svgpath) {
            if let Some(thickness) = text.thickness {
                ctx.set_line_width(thickness);
                ctx.stroke_with_path(&path);
            } else if text.fillrule.is_some() {
                ctx.fill_with_path_2d(&path);
            }
        }
        ctx.restore();
        return;
    }

    if let Some(thickness) = text.thickness {
        ctx.set_line_width(thickness);
    }

    if let Some(ref polygons) = text.polygons {
        let path = get_polygons_path(polygons);
        ctx.fill_with_path_2d(&path);
        ctx.restore();
        return;
    }

    // Stroke font rendering
    if let (Some(pos), Some(txt), Some(height), Some(width), Some(justify), Some(angle)) = (
        text.pos,
        text.text.as_deref(),
        text.height,
        text.width,
        text.justify,
        text.angle,
    ) {
        if let Some(fd) = font_data {
            let thickness = text.thickness.unwrap_or(0.15);
            ctx.set_line_width(thickness);
            ctx.translate(pos[0], pos[1]).unwrap();
            ctx.translate(thickness * 0.5, 0.0).unwrap();

            let attr = text.attr.as_deref().unwrap_or(&[]);
            let mut draw_angle = -angle;
            if attr.iter().any(|a| a == "mirrored") {
                ctx.scale(-1.0, 1.0).unwrap();
                draw_angle = -draw_angle;
            }
            let tilt = if attr.iter().any(|a| a == "italic") {
                0.125
            } else {
                0.0
            };

            let interline = height * 1.5 + thickness;
            let lines: Vec<&str> = txt.split('\n').collect();
            let line_count = if lines.last() == Some(&"") {
                lines.len() - 1
            } else {
                lines.len()
            };

            ctx.rotate(deg2rad(draw_angle)).unwrap();

            let mut offsety = (1.0 - justify[1]) / 2.0 * height;
            offsety -= (line_count as f64 - 1.0) * (justify[1] + 1.0) / 2.0 * interline;

            for line_str in &lines[..line_count] {
                let chars: Vec<char> = line_str.chars().collect();
                // Calculate line width
                let mut line_width = thickness + interline / 2.0 * tilt;
                let mut j = 0;
                while j < chars.len() {
                    if chars[j] == '\t' {
                        if let Some(sp) = fd.get(" ") {
                            let four_spaces = 4.0 * sp.w * width;
                            line_width += four_spaces - line_width % four_spaces;
                        }
                    } else {
                        if chars[j] == '~' {
                            j += 1;
                            if j >= chars.len() {
                                break;
                            }
                        }
                        let ch = chars[j].to_string();
                        if let Some(glyph) = fd.get(&ch) {
                            line_width += glyph.w * width;
                        }
                    }
                    j += 1;
                }

                let mut offsetx = -line_width * (justify[0] + 1.0) / 2.0;
                j = 0;
                while j < chars.len() {
                    if chars[j] == '\t' {
                        if let Some(sp) = fd.get(" ") {
                            let four_spaces = 4.0 * sp.w * width;
                            offsetx += four_spaces - offsetx % four_spaces;
                        }
                        j += 1;
                        continue;
                    }
                    if chars[j] == '~' {
                        j += 1;
                        if j >= chars.len() {
                            break;
                        }
                        if chars[j] != '~' {
                            j += 1;
                            continue;
                        }
                    }

                    let ch = chars[j].to_string();
                    if let Some(glyph) = fd.get(&ch) {
                        for line in &glyph.l {
                            if line.len() < 2 {
                                continue;
                            }
                            ctx.begin_path();
                            let p0 =
                                calc_font_point(line[0], width, height, offsetx, offsety, tilt);
                            ctx.move_to(p0[0], p0[1]);
                            for pt in &line[1..] {
                                let p = calc_font_point(*pt, width, height, offsetx, offsety, tilt);
                                ctx.line_to(p[0], p[1]);
                            }
                            ctx.stroke();
                        }
                        offsetx += glyph.w * width;
                    }
                    j += 1;
                }
                offsety += interline;
            }
        }
    }

    ctx.restore();
}

fn calc_font_point(
    linepoint: [f64; 2],
    width: f64,
    height: f64,
    offsetx: f64,
    offsety: f64,
    tilt: f64,
) -> [f64; 2] {
    let mut point = [
        linepoint[0] * width + offsetx,
        linepoint[1] * height + offsety,
    ];
    // Approximate pcbnew text tilt
    point[0] -= (linepoint[1] + 0.5) * height * tilt;
    point
}

fn draw_drawing(
    ctx: &CanvasRenderingContext2d,
    scalefactor: f64,
    item: &FootprintDrawingItem,
    color: &str,
    settings: &Settings,
    font_data: Option<&FontData>,
) {
    match item {
        FootprintDrawingItem::Shape(drawing) => {
            draw_edge(ctx, scalefactor, drawing, color);
        }
        FootprintDrawingItem::Text(text) => {
            draw_text(ctx, text, color, settings, font_data);
        }
    }
}

fn draw_pad(
    ctx: &CanvasRenderingContext2d,
    pad: &Pad,
    color: &str,
    outline: bool,
    cache: &mut PathCache,
    pad_key: &str,
) {
    ctx.save();
    ctx.translate(pad.pos[0], pad.pos[1]).unwrap();
    ctx.rotate(-deg2rad(pad.angle.unwrap_or(0.0))).unwrap();
    if let Some(offset) = pad.offset {
        ctx.translate(offset[0], offset[1]).unwrap();
    }
    ctx.set_fill_style_str(color);
    ctx.set_stroke_style_str(color);
    let path = get_pad_path(pad, cache, pad_key);
    if outline {
        ctx.stroke_with_path(&path);
    } else {
        ctx.fill_with_path_2d(&path);
    }
    ctx.restore();
}

fn draw_pad_hole(ctx: &CanvasRenderingContext2d, pad: &Pad, hole_color: &str) {
    if pad.pad_type != "th" {
        return;
    }
    ctx.save();
    ctx.translate(pad.pos[0], pad.pos[1]).unwrap();
    ctx.rotate(-deg2rad(pad.angle.unwrap_or(0.0))).unwrap();
    ctx.set_fill_style_str(hole_color);

    if let Some(ref drillsize) = pad.drillsize {
        let path = match pad.drillshape.as_deref() {
            Some("oblong") => get_oblong_path(*drillsize),
            Some("rect") => get_chamfered_rect_path(*drillsize, 0.0, 0, 0.0),
            _ => get_circle_path(drillsize[0] / 2.0),
        };
        ctx.fill_with_path_2d(&path);
    }
    ctx.restore();
}

struct FootprintColors {
    pad: String,
    pad_hole: String,
    outline: String,
}

#[allow(clippy::too_many_arguments)]
fn draw_footprint(
    ctx: &CanvasRenderingContext2d,
    layer: &str,
    scalefactor: f64,
    footprint: &Footprint,
    fp_index: usize,
    colors: &FootprintColors,
    highlight: bool,
    outline: bool,
    settings: &Settings,
    font_data: Option<&FontData>,
    cache: &mut PathCache,
) {
    if highlight && footprint.layer == layer {
        ctx.save();
        ctx.set_global_alpha(0.2);
        ctx.translate(footprint.bbox.pos[0], footprint.bbox.pos[1])
            .unwrap();
        ctx.rotate(deg2rad(-footprint.bbox.angle)).unwrap();
        ctx.translate(footprint.bbox.relpos[0], footprint.bbox.relpos[1])
            .unwrap();
        ctx.set_fill_style_str(&colors.pad);
        ctx.fill_rect(0.0, 0.0, footprint.bbox.size[0], footprint.bbox.size[1]);
        ctx.set_global_alpha(1.0);
        ctx.set_stroke_style_str(&colors.pad);
        ctx.set_line_width(3.0 / scalefactor);
        ctx.stroke_rect(0.0, 0.0, footprint.bbox.size[0], footprint.bbox.size[1]);
        ctx.restore();
    }

    for drawing in &footprint.drawings {
        if drawing.layer == layer {
            draw_drawing(
                ctx,
                scalefactor,
                &drawing.drawing,
                &colors.pad,
                settings,
                font_data,
            );
        }
    }

    ctx.set_line_width(3.0 / scalefactor);

    if settings.render_pads {
        for (pi, pad) in footprint.pads.iter().enumerate() {
            if pad.layers.iter().any(|l| l == layer) {
                let pad_key = format!("fp{}pad{}", fp_index, pi);
                draw_pad(ctx, pad, &colors.pad, outline, cache, &pad_key);
                if pad.pin1.is_some()
                    && (settings.highlight_pin1 == "all"
                        || (settings.highlight_pin1 == "selected" && highlight))
                {
                    draw_pad(ctx, pad, &colors.outline, true, cache, &pad_key);
                }
            }
        }
        for pad in &footprint.pads {
            draw_pad_hole(ctx, pad, &colors.pad_hole);
        }
    }
}

pub fn draw_edge_cuts(
    canvas: &HtmlCanvasElement,
    scalefactor: f64,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    font_data: Option<&FontData>,
) {
    let ctx = get_ctx(canvas);
    for edge in &pcbdata.edges {
        match edge {
            Drawing::Polygon { .. } => {
                draw_polygon_shape(&ctx, scalefactor, edge, &colors.pcb_edge)
            }
            _ => draw_edge(&ctx, scalefactor, edge, &colors.pcb_edge),
        }
    }
    let _ = (settings, font_data);
}

#[allow(clippy::too_many_arguments)]
pub fn draw_footprints(
    canvas: &HtmlCanvasElement,
    layer: &str,
    scalefactor: f64,
    highlight: bool,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    highlighted_footprints: &[usize],
    marked_footprints: &std::collections::HashSet<usize>,
    cache: &mut PathCache,
) {
    let ctx = get_ctx(canvas);
    ctx.set_line_width(3.0 / scalefactor);
    let font_data = pcbdata.font_data.as_ref();

    for (i, fp) in pcbdata.footprints.iter().enumerate() {
        let is_dnp = pcbdata.bom.as_ref().is_some_and(|b| b.skipped.contains(&i));
        let outline = settings.render_dnp_outline && is_dnp;
        let h = highlighted_footprints.contains(&i);
        let d = marked_footprints.contains(&i);

        if highlight {
            let fp_colors = if h && d {
                FootprintColors {
                    pad: colors.pad_highlight_both.clone(),
                    pad_hole: colors.pad_hole.clone(),
                    outline: colors.pin1_outline_highlight_both.clone(),
                }
            } else if h {
                FootprintColors {
                    pad: colors.pad_highlight.clone(),
                    pad_hole: colors.pad_hole.clone(),
                    outline: colors.pin1_outline_highlight.clone(),
                }
            } else if d {
                FootprintColors {
                    pad: colors.pad_highlight_marked.clone(),
                    pad_hole: colors.pad_hole.clone(),
                    outline: colors.pin1_outline_highlight_marked.clone(),
                }
            } else {
                continue;
            };
            draw_footprint(
                &ctx,
                layer,
                scalefactor,
                fp,
                i,
                &fp_colors,
                true,
                outline,
                settings,
                font_data,
                cache,
            );
        } else {
            let fp_colors = FootprintColors {
                pad: colors.pad.clone(),
                pad_hole: colors.pad_hole.clone(),
                outline: colors.pin1_outline.clone(),
            };
            draw_footprint(
                &ctx,
                layer,
                scalefactor,
                fp,
                i,
                &fp_colors,
                false,
                outline,
                settings,
                font_data,
                cache,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_bg_layer(
    canvas: &HtmlCanvasElement,
    layer_name: &str,
    layer: &str,
    scalefactor: f64,
    pcbdata: &PcbData,
    edge_color: &str,
    polygon_color: &str,
    text_color: &str,
    settings: &Settings,
) {
    let ctx = get_ctx(canvas);
    let font_data = pcbdata.font_data.as_ref();

    let drawings = match layer_name {
        "silkscreen" => &pcbdata.drawings.silkscreen,
        "fabrication" => &pcbdata.drawings.fabrication,
        _ => return,
    };
    let items = match drawings.get(layer) {
        Some(items) => items,
        None => return,
    };

    for d in items {
        match d {
            Drawing::Polygon { .. } => draw_polygon_shape(&ctx, scalefactor, d, polygon_color),
            Drawing::Segment { .. }
            | Drawing::Arc { .. }
            | Drawing::Circle { .. }
            | Drawing::Curve { .. }
            | Drawing::Rect { .. } => draw_edge(&ctx, scalefactor, d, edge_color),
        }
    }

    let _ = (text_color, settings, font_data);
}

pub fn draw_tracks(
    canvas: &HtmlCanvasElement,
    layer: &str,
    default_color: &str,
    highlight: bool,
    pcbdata: &PcbData,
    highlighted_net: &Option<String>,
) {
    let tracks = match pcbdata.tracks.as_ref().and_then(|t| t.get(layer)) {
        Some(t) => t,
        None => return,
    };
    let ctx = get_ctx(canvas);
    ctx.set_line_cap("round");

    for track in tracks {
        match track {
            Track::Segment {
                start,
                end,
                width,
                net,
                drillsize,
            } => {
                if highlight && highlighted_net.as_ref() != net.as_ref() {
                    continue;
                }
                let is_via = drillsize.is_some() && start == end;
                if !is_via {
                    ctx.set_stroke_style_str(default_color);
                    ctx.set_line_width(*width);
                    ctx.begin_path();
                    ctx.move_to(start[0], start[1]);
                    ctx.line_to(end[0], end[1]);
                    ctx.stroke();
                }
            }
            Track::Arc {
                center,
                startangle,
                endangle,
                radius,
                width,
                net,
            } => {
                if highlight && highlighted_net.as_ref() != net.as_ref() {
                    continue;
                }
                ctx.set_stroke_style_str(default_color);
                ctx.set_line_width(*width);
                ctx.begin_path();
                ctx.arc(
                    center[0],
                    center[1],
                    *radius,
                    deg2rad(*startangle),
                    deg2rad(*endangle),
                )
                .unwrap();
                ctx.stroke();
            }
        }
    }

    // Second pass: untented vias
    for track in tracks {
        if let Track::Segment {
            start,
            end,
            width,
            net,
            drillsize: Some(ds),
        } = track
        {
            if start != end {
                continue;
            }
            if highlight && highlighted_net.as_ref() != net.as_ref() {
                continue;
            }
            ctx.set_stroke_style_str(default_color);
            ctx.set_line_width(*width);
            ctx.begin_path();
            ctx.move_to(start[0], start[1]);
            ctx.line_to(end[0], end[1]);
            ctx.stroke();
            // Draw hole
            ctx.set_stroke_style_str("#CCCCCC"); // pad hole color
            ctx.set_line_width(*ds);
            ctx.line_to(end[0], end[1]);
            ctx.stroke();
        }
    }
}

pub fn draw_zones(
    canvas: &HtmlCanvasElement,
    layer: &str,
    default_color: &str,
    highlight: bool,
    pcbdata: &PcbData,
    highlighted_net: &Option<String>,
    zone_cache: &mut HashMap<String, Path2d>,
) {
    let zones = match pcbdata.zones.as_ref().and_then(|z| z.get(layer)) {
        Some(z) => z,
        None => return,
    };
    let ctx = get_ctx(canvas);
    ctx.set_line_join("round");

    for (i, zone) in zones.iter().enumerate() {
        if highlight && highlighted_net.as_ref() != zone.net.as_ref() {
            continue;
        }
        ctx.set_stroke_style_str(default_color);
        ctx.set_fill_style_str(default_color);

        let cache_key = format!("{}{}", layer, i);
        let path = zone_cache.entry(cache_key).or_insert_with(|| {
            if let Some(ref svgpath) = zone.svgpath {
                Path2d::new_with_path_string(svgpath).unwrap_or_else(|_| Path2d::new().unwrap())
            } else if let Some(ref polygons) = zone.polygons {
                get_polygons_path(polygons)
            } else {
                Path2d::new().unwrap()
            }
        });

        ctx.fill_with_path_2d(path);
        if let Some(w) = zone.width {
            if w > 0.0 {
                ctx.set_line_width(w);
                ctx.stroke_with_path(path);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_nets(
    canvas: &HtmlCanvasElement,
    layer: &str,
    highlight: bool,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    highlighted_net: &Option<String>,
    zone_cache: &mut HashMap<String, Path2d>,
) {
    let track_color = if highlight {
        &colors.track_highlight
    } else if layer == "F" {
        &colors.track_front
    } else {
        &colors.track_back
    };
    let zone_color = if highlight {
        &colors.zone_highlight
    } else if layer == "F" {
        &colors.zone_front
    } else {
        &colors.zone_back
    };

    if settings.render_zones {
        draw_zones(
            canvas,
            layer,
            zone_color,
            highlight,
            pcbdata,
            highlighted_net,
            zone_cache,
        );
    }
    if settings.render_tracks {
        draw_tracks(
            canvas,
            layer,
            track_color,
            highlight,
            pcbdata,
            highlighted_net,
        );
        // Also draw inner copper layer tracks (not zones - those are plane fills)
        if let Some(ref tracks) = pcbdata.tracks {
            let ctx = get_ctx(canvas);
            ctx.save();
            ctx.set_global_alpha(0.25);
            for name in tracks.inner_layer_names() {
                draw_tracks(
                    canvas,
                    name,
                    track_color,
                    highlight,
                    pcbdata,
                    highlighted_net,
                );
            }
            ctx.restore();
        }
    }
}

pub fn clear_canvas(canvas: &HtmlCanvasElement, color: Option<&str>) {
    let ctx = get_ctx(canvas);
    ctx.save();
    ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0).unwrap();
    if let Some(c) = color {
        ctx.set_fill_style_str(c);
        ctx.fill_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
    } else {
        ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
    }
    ctx.restore();
}

pub fn prepare_canvas(
    canvas: &HtmlCanvasElement,
    flip: bool,
    transform: &Transform,
    settings: &Settings,
) {
    let ctx = get_ctx(canvas);
    ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0).unwrap();
    ctx.scale(transform.zoom, transform.zoom).unwrap();
    ctx.translate(transform.panx, transform.pany).unwrap();
    if flip {
        ctx.scale(-1.0, 1.0).unwrap();
    }
    ctx.translate(transform.x, transform.y).unwrap();
    let rot = settings.board_rotation
        + if flip && settings.offset_back_rotation {
            -180.0
        } else {
            0.0
        };
    ctx.rotate(deg2rad(rot)).unwrap();
    ctx.scale(transform.s, transform.s).unwrap();
}

pub fn prepare_layer(layer: &LayerCanvases, settings: &Settings) {
    let flip = layer.layer == "B";
    for canvas in layer.all_canvases() {
        prepare_canvas(canvas, flip, &layer.transform, settings);
    }
}

fn rotate_vector(v: [f64; 2], angle: f64) -> [f64; 2] {
    let a = deg2rad(angle);
    [
        v[0] * a.cos() - v[1] * a.sin(),
        v[0] * a.sin() + v[1] * a.cos(),
    ]
}

fn apply_rotation(bbox: &BBox, flip: bool, settings: &Settings) -> BBox {
    let corners = [
        [bbox.minx, bbox.miny],
        [bbox.minx, bbox.maxy],
        [bbox.maxx, bbox.miny],
        [bbox.maxx, bbox.maxy],
    ];
    let rot = settings.board_rotation
        + if flip && settings.offset_back_rotation {
            -180.0
        } else {
            0.0
        };
    let rotated: Vec<[f64; 2]> = corners.iter().map(|v| rotate_vector(*v, rot)).collect();
    BBox {
        minx: rotated.iter().map(|v| v[0]).fold(f64::INFINITY, f64::min),
        miny: rotated.iter().map(|v| v[1]).fold(f64::INFINITY, f64::min),
        maxx: rotated
            .iter()
            .map(|v| v[0])
            .fold(f64::NEG_INFINITY, f64::max),
        maxy: rotated
            .iter()
            .map(|v| v[1])
            .fold(f64::NEG_INFINITY, f64::max),
    }
}

pub fn recalc_layer_scale(
    layer: &mut LayerCanvases,
    width: f64,
    height: f64,
    pcbdata: &PcbData,
    settings: &Settings,
) {
    let flip = layer.layer == "B";
    let bbox = apply_rotation(&pcbdata.edges_bbox, flip, settings);
    let mut scalefactor =
        0.98 * (width / (bbox.maxx - bbox.minx)).min(height / (bbox.maxy - bbox.miny));
    if scalefactor < 0.1 {
        scalefactor = 1.0;
    }
    layer.transform.s = scalefactor;
    if flip {
        layer.transform.x = -((bbox.maxx + bbox.minx) * scalefactor + width) * 0.5;
    } else {
        layer.transform.x = -((bbox.maxx + bbox.minx) * scalefactor - width) * 0.5;
    }
    layer.transform.y = -((bbox.maxy + bbox.miny) * scalefactor - height) * 0.5;

    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .unwrap_or(1.0);

    for canvas in layer.all_canvases() {
        canvas.set_width(width as u32);
        canvas.set_height(height as u32);
        let _ = canvas
            .style()
            .set_property("width", &format!("{}px", width / dpr));
        let _ = canvas
            .style()
            .set_property("height", &format!("{}px", height / dpr));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_background(
    layer: &LayerCanvases,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    highlighted_footprints: &[usize],
    marked_footprints: &std::collections::HashSet<usize>,
    highlighted_net: &Option<String>,
    cache: &mut PathCache,
    zone_cache: &mut HashMap<String, Path2d>,
) {
    clear_canvas(&layer.bg, None);
    clear_canvas(&layer.fab, None);
    clear_canvas(&layer.silk, None);

    // Draw opposite layer at reduced opacity (see-through)
    let opposite = if layer.layer == "F" { "B" } else { "F" };
    {
        let ctx = get_ctx(&layer.bg);
        ctx.save();
        ctx.set_global_alpha(0.35);
    }
    draw_nets(
        &layer.bg,
        opposite,
        false,
        pcbdata,
        colors,
        settings,
        highlighted_net,
        zone_cache,
    );
    draw_footprints(
        &layer.bg,
        opposite,
        layer.transform.s * layer.transform.zoom,
        false,
        pcbdata,
        colors,
        settings,
        highlighted_footprints,
        marked_footprints,
        cache,
    );
    get_ctx(&layer.bg).restore();

    // Draw primary layer at full opacity
    draw_nets(
        &layer.bg,
        &layer.layer,
        false,
        pcbdata,
        colors,
        settings,
        highlighted_net,
        zone_cache,
    );
    draw_footprints(
        &layer.bg,
        &layer.layer,
        layer.transform.s * layer.transform.zoom,
        false,
        pcbdata,
        colors,
        settings,
        highlighted_footprints,
        marked_footprints,
        cache,
    );
    draw_edge_cuts(
        &layer.bg,
        layer.transform.s * layer.transform.zoom,
        pcbdata,
        colors,
        settings,
        pcbdata.font_data.as_ref(),
    );

    if settings.render_silkscreen {
        draw_bg_layer(
            &layer.silk,
            "silkscreen",
            &layer.layer,
            layer.transform.s * layer.transform.zoom,
            pcbdata,
            &colors.silk_edge,
            &colors.silk_polygon,
            &colors.silk_text,
            settings,
        );
    }
    if settings.render_fabrication {
        draw_bg_layer(
            &layer.fab,
            "fabrication",
            &layer.layer,
            layer.transform.s * layer.transform.zoom,
            pcbdata,
            &colors.fab_edge,
            &colors.fab_polygon,
            &colors.fab_text,
            settings,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_highlights_on_layer(
    layer: &LayerCanvases,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    highlighted_footprints: &[usize],
    marked_footprints: &std::collections::HashSet<usize>,
    highlighted_net: &Option<String>,
    cache: &mut PathCache,
    zone_cache: &mut HashMap<String, Path2d>,
) {
    clear_canvas(&layer.highlight, None);

    if !marked_footprints.is_empty() || !highlighted_footprints.is_empty() {
        draw_footprints(
            &layer.highlight,
            &layer.layer,
            layer.transform.s * layer.transform.zoom,
            true,
            pcbdata,
            colors,
            settings,
            highlighted_footprints,
            marked_footprints,
            cache,
        );
    }
    if highlighted_net.is_some() {
        // Draw both layers at full opacity for saturated highlight
        let opposite = if layer.layer == "F" { "B" } else { "F" };
        draw_nets(
            &layer.highlight,
            opposite,
            true,
            pcbdata,
            colors,
            settings,
            highlighted_net,
            zone_cache,
        );
        draw_nets(
            &layer.highlight,
            &layer.layer,
            true,
            pcbdata,
            colors,
            settings,
            highlighted_net,
            zone_cache,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn redraw_canvas(
    layer: &LayerCanvases,
    pcbdata: &PcbData,
    colors: &Colors,
    settings: &Settings,
    highlighted_footprints: &[usize],
    marked_footprints: &std::collections::HashSet<usize>,
    highlighted_net: &Option<String>,
    cache: &mut PathCache,
    zone_cache: &mut HashMap<String, Path2d>,
) {
    prepare_layer(layer, settings);
    draw_background(
        layer,
        pcbdata,
        colors,
        settings,
        highlighted_footprints,
        marked_footprints,
        highlighted_net,
        cache,
        zone_cache,
    );
    draw_highlights_on_layer(
        layer,
        pcbdata,
        colors,
        settings,
        highlighted_footprints,
        marked_footprints,
        highlighted_net,
        cache,
        zone_cache,
    );
}

// ─── Hit Testing ────────────────────────────────────────────────────

fn point_within_distance_to_segment(
    x: f64,
    y: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    d: f64,
) -> bool {
    let a = x - x1;
    let b = y - y1;
    let c = x2 - x1;
    let dd = y2 - y1;
    let dot = a * c + b * dd;
    let len_sq = c * c + dd * dd;
    let (dx, dy) = if len_sq == 0.0 {
        (x - x1, y - y1)
    } else {
        let param = dot / len_sq;
        if param < 0.0 {
            (x - x1, y - y1)
        } else if param > 1.0 {
            (x - x2, y - y2)
        } else {
            (x - (x1 + param * c), y - (y1 + param * dd))
        }
    };
    dx * dx + dy * dy <= d * d
}

pub fn bbox_hit_scan(layer: &str, x: f64, y: f64, pcbdata: &PcbData) -> Vec<usize> {
    let opposite = if layer == "F" { "B" } else { "F" };
    let mut result = Vec::new();
    // Check primary layer first, then opposite
    for check_layer in &[layer, opposite] {
        for (i, fp) in pcbdata.footprints.iter().enumerate() {
            if fp.layer == *check_layer {
                let v = rotate_vector([x - fp.bbox.pos[0], y - fp.bbox.pos[1]], fp.bbox.angle);
                if fp.bbox.relpos[0] <= v[0]
                    && v[0] <= fp.bbox.relpos[0] + fp.bbox.size[0]
                    && fp.bbox.relpos[1] <= v[1]
                    && v[1] <= fp.bbox.relpos[1] + fp.bbox.size[1]
                {
                    result.push(i);
                }
            }
        }
        if !result.is_empty() {
            return result;
        }
    }
    result
}

fn track_hit_scan(tracks: &[Track], x: f64, y: f64) -> Option<String> {
    for track in tracks {
        match track {
            Track::Segment {
                start,
                end,
                width,
                net,
                ..
            } => {
                if point_within_distance_to_segment(
                    x,
                    y,
                    start[0],
                    start[1],
                    end[0],
                    end[1],
                    width / 2.0,
                ) {
                    return net.clone();
                }
            }
            Track::Arc {
                center,
                radius,
                width,
                net,
                ..
            } => {
                let dx = x - center[0];
                let dy = y - center[1];
                let dist = (dx * dx + dy * dy).sqrt();
                if (dist - radius).abs() <= width / 2.0 {
                    return net.clone();
                }
            }
        }
    }
    None
}

pub fn net_hit_scan(
    layer: &str,
    x: f64,
    y: f64,
    pcbdata: &PcbData,
    settings: &Settings,
) -> Option<String> {
    let opposite = if layer == "F" { "B" } else { "F" };

    // Build layer check order: primary, opposite, then inner layers
    let mut layers_to_check: Vec<&str> = vec![layer, opposite];
    if let Some(ref tracks_data) = pcbdata.tracks {
        for name in tracks_data.inner_layer_names() {
            layers_to_check.push(name.as_str());
        }
    }

    for check_layer in &layers_to_check {
        if settings.render_tracks {
            if let Some(tracks) = pcbdata.tracks.as_ref().and_then(|t| t.get(check_layer)) {
                if let Some(net) = track_hit_scan(tracks, x, y) {
                    return Some(net);
                }
            }
        }
        if settings.render_pads {
            for fp in &pcbdata.footprints {
                for pad in &fp.pads {
                    if pad.layers.iter().any(|l| l == *check_layer) {
                        let v = rotate_vector(
                            [x - pad.pos[0], y - pad.pos[1]],
                            pad.angle.unwrap_or(0.0),
                        );
                        let hx = pad.size[0] / 2.0;
                        let hy = pad.size[1] / 2.0;
                        if v[0].abs() <= hx && v[1].abs() <= hy {
                            return pad.net.clone();
                        }
                    }
                }
            }
        }
    }
    None
}

/// Convert screen coordinates to board coordinates
pub fn screen_to_board(
    offset_x: f64,
    offset_y: f64,
    transform: &Transform,
    layer: &str,
    settings: &Settings,
) -> [f64; 2] {
    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .unwrap_or(1.0);
    let flip = layer == "B";
    let x = if flip {
        (dpr * offset_x / transform.zoom - transform.panx + transform.x) / -transform.s
    } else {
        (dpr * offset_x / transform.zoom - transform.panx - transform.x) / transform.s
    };
    let y = (dpr * offset_y / transform.zoom - transform.y - transform.pany) / transform.s;
    let rot = -settings.board_rotation
        + if flip && settings.offset_back_rotation {
            -180.0
        } else {
            0.0
        };
    rotate_vector([x, y], rot)
}

fn get_ctx(canvas: &HtmlCanvasElement) -> CanvasRenderingContext2d {
    canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into::<CanvasRenderingContext2d>()
        .unwrap()
}
