//! `PcbData` -> [`vector_view::Scene`] adapter (feature `vector-view`).
//!
//! All iBOM-specific interpretation happens here so viewers only draw plain
//! geometry: pad shapes become polygons/circles in board coordinates, drawing
//! and text transforms are applied, curves are flattened, stroke-font text is
//! converted to strokes (when `font_data` is present) and every item lands on
//! a named scene layer.
//!
//! # Layers
//!
//! For each side `S` in `F`, `B` (and inner copper names such as `In1.Cu`):
//!
//! | name               | contents                                        |
//! |--------------------|-------------------------------------------------|
//! | `S.Zones`          | copper zones                                    |
//! | `S.Tracks`         | tracks, arcs, then vias (with their drill holes)|
//! | `S.CopperPads`     | `copper_pads` (Gerber copper flashes)           |
//! | `S.Footprints`     | footprint graphics and text                     |
//! | `S.Pads`           | footprint pads                                  |
//! | `S.Fab`            | board-level fabrication drawings                |
//! | `S.Silkscreen`     | board-level silkscreen drawings                 |
//! | `S.SilkscreenClear`| clear-polarity silkscreen (hidden; erase layer) |
//! | `S.Bounds`         | footprint bounding boxes (hidden; for picking)  |
//!
//! plus side-less `Holes` (plated pad drills), `Edge.Cuts` and `Drills`
//! (fabrication drill hits, which iBOM-style viewers punch through).
//!
//! # Ids
//!
//! - `GroupId` = footprint index in `PcbData::footprints`.
//! - `NetId` = index into `PcbData::nets` (unknown names are appended).
//! - Item ids are sequential from 1 in emission order.

use crate::types::{
    Drawing, FontData, Footprint, FootprintDrawingItem, Pad, PcbData, TextDrawing, Track, Zone,
};
use std::collections::HashMap;
use std::f64::consts::PI;
use vector_view::{
    BBox, Group, GroupId, GroupKind, Item, ItemId, Layer, LayerId, LayerKind, Net, NetId, Point,
    Prim, Prop, Role, Scene, SceneKind, Side,
};

pub const HOLES: &str = "Holes";
pub const EDGE_CUTS: &str = "Edge.Cuts";
pub const DRILLS: &str = "Drills";

/// Per-side layer suffixes, in front-view paint order.
pub const ZONES: &str = "Zones";
pub const TRACKS: &str = "Tracks";
pub const COPPER_PADS: &str = "CopperPads";
pub const FOOTPRINTS: &str = "Footprints";
pub const PADS: &str = "Pads";
pub const FAB: &str = "Fab";
pub const SILKSCREEN: &str = "Silkscreen";
pub const SILKSCREEN_CLEAR: &str = "SilkscreenClear";
pub const BOUNDS: &str = "Bounds";

/// `"F"` + `"Pads"` -> `"F.Pads"`.
pub fn layer_name(side: &str, suffix: &str) -> String {
    format!("{side}.{suffix}")
}

const PAD: [u8; 4] = [0x87, 0x87, 0x87, 255];
const HOLE: [u8; 4] = [0xcc, 0xcc, 0xcc, 255];

fn side_colors(side: &str, suffix: &str) -> [u8; 4] {
    let (track, zone) = match side {
        "F" => ([0xcc, 0x44, 0x44, 255], [0xe8, 0xa0, 0xa0, 255]),
        "B" => ([0x44, 0x44, 0xcc, 255], [0xa0, 0xa0, 0xe8, 255]),
        _ => ([0xb0, 0x90, 0x40, 255], [0xd8, 0xc8, 0x98, 255]),
    };
    match suffix {
        ZONES => zone,
        TRACKS | COPPER_PADS => track,
        FOOTPRINTS | PADS | BOUNDS => PAD,
        FAB => [0x90, 0x76, 0x51, 255],
        SILKSCREEN | SILKSCREEN_CLEAR => [0xaa, 0xaa, 0x44, 255],
        _ => PAD,
    }
}

fn deg2rad(d: f64) -> f64 {
    d * PI / 180.0
}

fn rotate(p: Point, rad: f64) -> Point {
    let (s, c) = rad.sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

struct Builder {
    scene: Scene,
    layer_ids: HashMap<String, LayerId>,
    nets: HashMap<String, NetId>,
    next_item: ItemId,
}

impl Builder {
    fn layer(
        &mut self,
        name: &str,
        kind: LayerKind,
        side: Side,
        z: i32,
        color: [u8; 4],
    ) -> LayerId {
        if let Some(id) = self.layer_ids.get(name) {
            return *id;
        }
        let id = self.scene.layers.len() as LayerId;
        let visible = !(name.ends_with(BOUNDS) || name.ends_with(SILKSCREEN_CLEAR));
        self.scene.layers.push(Layer {
            id,
            name: name.to_string(),
            kind,
            side,
            z,
            color,
            visible,
        });
        self.layer_ids.insert(name.to_string(), id);
        id
    }

    /// Layer `side.suffix`, created on first use with front-view z ordering.
    fn side_layer(&mut self, side: &str, suffix: &str, inner_rank: i32) -> LayerId {
        let name = layer_name(side, suffix);
        let (kind, rank) = match suffix {
            ZONES => (LayerKind::Copper, 1),
            TRACKS => (LayerKind::Copper, 2),
            COPPER_PADS => (LayerKind::Copper, 3),
            FOOTPRINTS => (LayerKind::Drawing, 4),
            PADS => (LayerKind::Copper, 5),
            FAB => (LayerKind::Fabrication, 6),
            SILKSCREEN => (LayerKind::Silkscreen, 7),
            SILKSCREEN_CLEAR => (LayerKind::Silkscreen, 8),
            BOUNDS => (LayerKind::Courtyard, 9),
            _ => (LayerKind::Other, 6),
        };
        let (scene_side, z) = match side {
            "F" => (Side::Front, 100 + rank),
            "B" => (Side::Back, -100 - rank),
            _ => (Side::Inner, -50 + inner_rank * 10 + rank),
        };
        self.layer(&name, kind, scene_side, z, side_colors(side, suffix))
    }

    fn net(&mut self, name: Option<&String>) -> Option<NetId> {
        let name = name?;
        if name.is_empty() {
            return None;
        }
        if let Some(id) = self.nets.get(name) {
            return Some(*id);
        }
        let id = self.scene.nets.len() as NetId;
        self.scene.nets.push(Net {
            id,
            name: name.clone(),
        });
        self.nets.insert(name.clone(), id);
        Some(id)
    }

    fn push(
        &mut self,
        layer: LayerId,
        role: Role,
        prim: Prim,
        net: Option<NetId>,
        group: Option<GroupId>,
        props: Vec<Prop>,
    ) -> ItemId {
        let id = self.next_item;
        self.next_item += 1;
        self.scene.items.push(Item {
            id,
            layer,
            role,
            prim,
            net,
            group,
            props,
        });
        id
    }
}

/// Flatten a cubic Bézier into a polyline.
fn bezier(p0: Point, p1: Point, p2: Point, p3: Point) -> Vec<Point> {
    let len = |a: Point, b: Point| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    let est = len(p0, p1) + len(p1, p2) + len(p2, p3);
    let n = ((est / 0.25).ceil() as usize).clamp(8, 64);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            [
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            ]
        })
        .collect()
}

/// Split iBOM polygon rings into an even-odd `Polygon` (first ring outer).
fn rings_to_polygon(rings: Vec<Vec<Point>>, fill: bool, stroke: f64) -> Option<Prim> {
    let mut rings = rings.into_iter().filter(|r| !r.is_empty());
    let outer = rings.next()?;
    Some(Prim::Polygon {
        outer,
        holes: rings.collect(),
        fill,
        stroke,
    })
}

/// Geometry for an iBOM drawing. `solid` fills rects and circles regardless of
/// `filled` (copper pad flashes).
fn drawing_prim(d: &Drawing, solid: bool) -> Option<Prim> {
    Some(match d {
        Drawing::Segment { start, end, width } => Prim::Polyline {
            points: vec![*start, *end],
            width: *width,
        },
        Drawing::Rect { start, end, width } => Prim::Polygon {
            outer: vec![*start, [start[0], end[1]], *end, [end[0], start[1]]],
            holes: vec![],
            fill: solid,
            stroke: if solid { 0.0 } else { *width },
        },
        Drawing::Circle {
            start,
            radius,
            width,
            filled,
        } => {
            let fill = solid || filled.is_some_and(|f| f != 0);
            Prim::Circle {
                center: *start,
                radius: *radius,
                fill,
                stroke: if fill { 0.0 } else { *width },
            }
        }
        Drawing::Arc {
            start,
            radius,
            startangle,
            endangle,
            width,
        } => Prim::Arc {
            center: *start,
            radius: *radius,
            start: deg2rad(*startangle),
            end: deg2rad(*endangle),
            width: *width,
        },
        Drawing::Curve {
            start,
            end,
            cpa,
            cpb,
            width,
        } => Prim::Polyline {
            points: bezier(*start, *cpa, *cpb, *end),
            width: *width,
        },
        Drawing::Polygon {
            pos,
            angle,
            polygons,
            filled,
            width,
        } => {
            let rot = deg2rad(-angle);
            let rings = polygons
                .iter()
                .map(|r| r.iter().map(|p| add(*pos, rotate(*p, rot))).collect())
                .collect();
            let fill = filled.is_none_or(|f| f != 0);
            return rings_to_polygon(rings, fill, if fill { 0.0 } else { *width });
        }
    })
}

/// Rounded/chamfered rectangle centred on the origin, as iBOM draws it.
/// `chamf` bits: 1 top-left, 2 top-right, 4 bottom-left, 8 bottom-right
/// (y-down local frame).
fn rounded_rect(size: [f64; 2], radius: f64, chamf: u8, ratio: f64) -> Vec<Point> {
    let (hw, hh) = (size[0] / 2.0, size[1] / 2.0);
    let r = radius.clamp(0.0, hw.min(hh));
    let c = size[0].min(size[1]) * ratio;
    let mut pts = Vec::new();
    // Corners in drawing order with the arc start angle for rounding.
    let corners = [
        (4u8, [-hw, hh], PI / 2.0),
        (8, [hw, hh], 0.0),
        (2, [hw, -hh], -PI / 2.0),
        (1, [-hw, -hh], PI),
    ];
    for (bit, [x, y], a0) in corners {
        let (sx, sy) = (x.signum(), y.signum());
        if chamf & bit != 0 {
            // Chamfer: approach along the previous edge, cut diagonally.
            let (p_from, p_to) = if sy > 0.0 && sx < 0.0 {
                ([x, y - c], [x + c, y])
            } else if sy > 0.0 {
                ([x - c, y], [x, y - c])
            } else if sx > 0.0 {
                ([x, y + c], [x - c, y])
            } else {
                ([x + c, y], [x, y + c])
            };
            pts.push(p_from);
            pts.push(p_to);
        } else if r > 0.0 {
            let centre = [x - sx * r, y - sy * r];
            let steps = 8;
            for i in 0..=steps {
                let a = a0 + (PI / 2.0) * (i as f64 / steps as f64);
                pts.push([centre[0] + r * a.cos(), centre[1] + r * a.sin()]);
            }
        } else {
            pts.push([x, y]);
        }
    }
    pts
}

/// Pad outline in board coordinates.
fn pad_prim(pad: &Pad) -> Option<Prim> {
    let rot = -deg2rad(pad.angle.unwrap_or(0.0));
    let offset = pad.offset.unwrap_or([0.0, 0.0]);
    let place = |p: Point| add(pad.pos, rotate(add(p, offset), rot));
    let poly = |local: Vec<Point>| Prim::Polygon {
        outer: local.into_iter().map(place).collect(),
        holes: vec![],
        fill: true,
        stroke: 0.0,
    };
    let [w, h] = pad.size;
    Some(match pad.shape.as_str() {
        "rect" => poly(rounded_rect(pad.size, 0.0, 0, 0.0)),
        "oval" => poly(rounded_rect(pad.size, w.min(h) / 2.0, 0, 0.0)),
        "circle" => Prim::Circle {
            center: place([0.0, 0.0]),
            radius: w / 2.0,
            fill: true,
            stroke: 0.0,
        },
        "roundrect" => poly(rounded_rect(pad.size, pad.radius.unwrap_or(0.0), 0, 0.0)),
        "chamfrect" => poly(rounded_rect(
            pad.size,
            pad.radius.unwrap_or(0.0),
            pad.chamfpos.unwrap_or(0),
            pad.chamfratio.unwrap_or(0.0),
        )),
        "custom" => {
            let rings = pad
                .polygons
                .as_ref()?
                .iter()
                .map(|r| r.iter().map(|p| place(*p)).collect())
                .collect();
            return rings_to_polygon(rings, true, 0.0);
        }
        _ => return None,
    })
}

/// Drill hole of a through-hole pad (iBOM draws it without the pad offset).
fn pad_hole(pad: &Pad) -> Option<Prim> {
    if pad.pad_type != "th" {
        return None;
    }
    let size = pad.drillsize?;
    let rot = -deg2rad(pad.angle.unwrap_or(0.0));
    Some(match pad.drillshape.as_deref() {
        Some("oblong") => Prim::Hole {
            center: pad.pos,
            size,
            rotation: rot,
        },
        Some("rect") => Prim::Polygon {
            outer: rounded_rect(size, 0.0, 0, 0.0)
                .into_iter()
                .map(|p| add(pad.pos, rotate(p, rot)))
                .collect(),
            holes: vec![],
            fill: true,
            stroke: 0.0,
        },
        _ => Prim::Hole {
            center: pad.pos,
            size: [size[0], size[0]],
            rotation: 0.0,
        },
    })
}

/// Stroke-font text to strokes, following the iBOM renderer.
fn text_strokes(t: &TextDrawing, font: &FontData) -> Option<Prim> {
    let (pos, txt, height, width, angle) =
        (t.pos?, t.text.as_deref()?, t.height?, t.width?, t.angle?);
    let justify = t.justify.unwrap_or([0, 0]);
    let (jx, jy) = (justify[0] as f64, justify[1] as f64);
    let thickness = t.thickness.unwrap_or(0.15);
    let attr = t.attr.as_deref().unwrap_or(&[]);
    let mirrored = attr.iter().any(|a| a == "mirrored");
    let tilt = if attr.iter().any(|a| a == "italic") {
        0.125
    } else {
        0.0
    };
    let draw_angle = deg2rad(if mirrored { angle } else { -angle });
    let place = |p: Point| {
        let r = rotate(p, draw_angle);
        let m = if mirrored { [-r[0], r[1]] } else { r };
        [pos[0] + thickness * 0.5 + m[0], pos[1] + m[1]]
    };

    let interline = height * 1.5 + thickness;
    let lines: Vec<&str> = txt.split('\n').collect();
    let line_count = if lines.last() == Some(&"") {
        lines.len() - 1
    } else {
        lines.len()
    };
    let space = font.get(" ").map(|g| 4.0 * g.w * width);
    let mut strokes = Vec::new();
    let mut offsety = (1.0 - jy) / 2.0 * height;
    offsety -= (line_count as f64 - 1.0) * (jy + 1.0) / 2.0 * interline;

    for line in &lines[..line_count] {
        let chars: Vec<char> = line.chars().collect();
        let mut line_width = thickness + interline / 2.0 * tilt;
        let mut j = 0;
        while j < chars.len() {
            if chars[j] == '\t' {
                if let Some(four) = space {
                    line_width += four - line_width % four;
                }
            } else {
                if chars[j] == '~' {
                    j += 1;
                    if j >= chars.len() {
                        break;
                    }
                }
                if let Some(g) = font.get(&chars[j].to_string()) {
                    line_width += g.w * width;
                }
            }
            j += 1;
        }
        let mut offsetx = -line_width * (jx + 1.0) / 2.0;
        j = 0;
        while j < chars.len() {
            if chars[j] == '\t' {
                if let Some(four) = space {
                    offsetx += four - offsetx % four;
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
            if let Some(g) = font.get(&chars[j].to_string()) {
                for l in &g.l {
                    if l.len() < 2 {
                        continue;
                    }
                    strokes.push(
                        l.iter()
                            .map(|lp| {
                                let x = lp[0] * width + offsetx - (lp[1] + 0.5) * height * tilt;
                                place([x, lp[1] * height + offsety])
                            })
                            .collect(),
                    );
                }
                offsetx += g.w * width;
            }
            j += 1;
        }
        offsety += interline;
    }
    if strokes.is_empty() {
        return None;
    }
    Some(Prim::Strokes {
        strokes,
        width: thickness,
    })
}

fn text_props(t: &TextDrawing) -> Vec<Prop> {
    let kind = if t.is_ref.is_some() {
        "ref"
    } else if t.val.is_some() {
        "value"
    } else {
        "text"
    };
    let mut props = vec![Prop::new("kind", kind)];
    if let Some(text) = &t.text {
        props.push(Prop::new("text", text.clone()));
    }
    props
}

/// Inner layer names in a stable order.
fn inner_names(pcb: &PcbData) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |keys: Vec<&String>| {
        for k in keys {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    };
    if let Some(t) = &pcb.tracks {
        add(t.inner.keys().collect());
    }
    if let Some(z) = &pcb.zones {
        add(z.inner.keys().collect());
    }
    if let Some(p) = &pcb.copper_pads {
        add(p.inner.keys().collect());
    }
    names.sort();
    names
}

fn copper_sides(pcb: &PcbData) -> Vec<(String, i32)> {
    let mut sides = vec![("B".to_string(), 0), ("F".to_string(), 0)];
    sides.extend(
        inner_names(pcb)
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i as i32)),
    );
    sides
}

fn layer_get<'a, T>(data: &'a crate::types::LayerData<T>, side: &str) -> Option<&'a T> {
    match side {
        "F" => Some(&data.front),
        "B" => Some(&data.back),
        other => data.inner.get(other),
    }
}

fn emit_tracks(b: &mut Builder, tracks: &[Track], layer: LayerId) {
    let mut vias = Vec::new();
    for t in tracks {
        match t {
            Track::Segment {
                start,
                end,
                width,
                net,
                drillsize,
            } => {
                let net = b.net(net.as_ref());
                if let (Some(d), true) = (drillsize, start == end) {
                    vias.push((*start, *width, *d, net));
                    continue;
                }
                b.push(
                    layer,
                    Role::Track,
                    Prim::Polyline {
                        points: vec![*start, *end],
                        width: *width,
                    },
                    net,
                    None,
                    vec![],
                );
            }
            Track::Arc {
                center,
                startangle,
                endangle,
                radius,
                width,
                net,
            } => {
                let net = b.net(net.as_ref());
                b.push(
                    layer,
                    Role::Track,
                    Prim::Arc {
                        center: *center,
                        radius: *radius,
                        start: deg2rad(*startangle),
                        end: deg2rad(*endangle),
                        width: *width,
                    },
                    net,
                    None,
                    vec![],
                );
            }
        }
    }
    for (c, w, d, net) in vias {
        b.push(
            layer,
            Role::Via,
            Prim::Circle {
                center: c,
                radius: w / 2.0,
                fill: true,
                stroke: 0.0,
            },
            net,
            None,
            vec![Prop::new("drill", format!("{d}"))],
        );
        b.push(
            layer,
            Role::Hole,
            Prim::Hole {
                center: c,
                size: [d, d],
                rotation: 0.0,
            },
            net,
            None,
            vec![],
        );
    }
}

fn emit_zones(b: &mut Builder, zones: &[Zone], layer: LayerId) {
    for z in zones {
        let Some(polys) = &z.polygons else { continue };
        let net = b.net(z.net.as_ref());
        let stroke = z.width.unwrap_or(0.0).max(0.0);
        if let Some(prim) = rings_to_polygon(polys.clone(), true, stroke) {
            b.push(layer, Role::Zone, prim, net, None, vec![]);
        }
    }
}

fn emit_footprint(b: &mut Builder, fp: &Footprint, index: usize, font: Option<&FontData>) {
    let group = Some(index as GroupId);
    for side in ["F", "B"] {
        let drawings: Vec<_> = fp.drawings.iter().filter(|d| d.layer == side).collect();
        if !drawings.is_empty() {
            let layer = b.side_layer(side, FOOTPRINTS, 0);
            for d in drawings {
                match &d.drawing {
                    FootprintDrawingItem::Shape(s) => {
                        if let Some(prim) = drawing_prim(s, false) {
                            b.push(layer, Role::Graphic, prim, None, group, vec![]);
                        }
                    }
                    FootprintDrawingItem::Text(t) => {
                        if let Some(prim) = font.and_then(|f| text_strokes(t, f)) {
                            b.push(layer, Role::Text, prim, None, group, text_props(t));
                        }
                    }
                }
            }
        }
    }
    for pad in &fp.pads {
        let Some(prim) = pad_prim(pad) else { continue };
        let net = b.net(pad.net.as_ref());
        let mut props = vec![
            Prop::new("shape", pad.shape.clone()),
            Prop::new("type", pad.pad_type.clone()),
        ];
        if pad.pin1.is_some_and(|p| p != 0) {
            props.push(Prop::new("pin1", "1"));
        }
        for l in &pad.layers {
            let layer = b.side_layer(l, PADS, 0);
            b.push(layer, Role::Pad, prim.clone(), net, group, props.clone());
        }
    }
}

/// Footprint bounding box as iBOM picks it: `pos + R(-angle) (relpos + size)`.
fn footprint_bounds(fp: &Footprint) -> Prim {
    let bb = &fp.bbox;
    let rot = deg2rad(-bb.angle);
    let [x0, y0] = bb.relpos;
    let [w, h] = bb.size;
    let outer = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]]
        .into_iter()
        .map(|p| add(bb.pos, rotate(p, rot)))
        .collect();
    Prim::Polygon {
        outer,
        holes: vec![],
        fill: true,
        stroke: 0.0,
    }
}

/// Convert parsed board data into a vector-view scene.
pub fn to_scene(pcb: &PcbData) -> Scene {
    let mut b = Builder {
        scene: Scene::new(SceneKind::Pcb, true),
        layer_ids: HashMap::new(),
        nets: HashMap::new(),
        next_item: 1,
    };
    // Keep NetId == index into PcbData::nets, including KiCad's unnamed net 0.
    for (i, name) in pcb.nets.iter().flatten().enumerate() {
        b.scene.nets.push(Net {
            id: i as NetId,
            name: name.clone(),
        });
        if !name.is_empty() {
            b.nets.entry(name.clone()).or_insert(i as NetId);
        }
    }
    let font = pcb.font_data.as_ref();

    // Copper per side (and inner layers): zones, tracks, copper pad flashes.
    for (side, rank) in copper_sides(pcb) {
        if let Some(zones) = pcb.zones.as_ref().and_then(|z| layer_get(z, &side)) {
            if !zones.is_empty() {
                let l = b.side_layer(&side, ZONES, rank);
                emit_zones(&mut b, zones, l);
            }
        }
        if let Some(tracks) = pcb.tracks.as_ref().and_then(|t| layer_get(t, &side)) {
            if !tracks.is_empty() {
                let l = b.side_layer(&side, TRACKS, rank);
                emit_tracks(&mut b, tracks, l);
            }
        }
        if let Some(pads) = pcb.copper_pads.as_ref().and_then(|p| layer_get(p, &side)) {
            if !pads.is_empty() {
                let l = b.side_layer(&side, COPPER_PADS, rank);
                for d in pads {
                    if let Some(prim) = drawing_prim(d, true) {
                        b.push(l, Role::Pad, prim, None, None, vec![]);
                    }
                }
            }
        }
    }

    // Footprints: graphics, pads, plated holes, pick bounds.
    let dnp: std::collections::HashSet<usize> = pcb
        .bom
        .as_ref()
        .map(|bom| bom.skipped.iter().copied().collect())
        .unwrap_or_default();
    for (i, fp) in pcb.footprints.iter().enumerate() {
        let mut props = vec![Prop::new("side", fp.layer.clone())];
        if dnp.contains(&i) {
            props.push(Prop::new("dnp", "1"));
        }
        b.scene.groups.push(Group {
            id: i as GroupId,
            kind: GroupKind::Footprint,
            label: fp.ref_.clone(),
            props,
        });
        emit_footprint(&mut b, fp, i, font);
    }
    let holes: Vec<(Prim, Option<NetId>, GroupId)> = pcb
        .footprints
        .iter()
        .enumerate()
        .flat_map(|(i, fp)| {
            fp.pads
                .iter()
                .filter_map(move |p| pad_hole(p).map(|h| (h, p.net.clone(), i as GroupId)))
        })
        .map(|(h, net, g)| (h, b.nets.get(net.as_deref().unwrap_or("")).copied(), g))
        .collect();
    if !holes.is_empty() {
        let l = b.layer(HOLES, LayerKind::Drill, Side::None, 200, HOLE);
        for (prim, net, g) in holes {
            b.push(l, Role::Hole, prim, net, Some(g), vec![]);
        }
    }

    // Board-level drawings.
    let silk = &pcb.drawings.silkscreen;
    let fab = &pcb.drawings.fabrication;
    for (side, list) in [("F", &fab.front), ("B", &fab.back)] {
        if !list.is_empty() {
            let l = b.side_layer(side, FAB, 0);
            for d in list {
                if let Some(prim) = drawing_prim(d, false) {
                    b.push(l, Role::Graphic, prim, None, None, vec![]);
                }
            }
        }
    }
    for (side, list) in [("F", &silk.front), ("B", &silk.back)] {
        if !list.is_empty() {
            let l = b.side_layer(side, SILKSCREEN, 0);
            for d in list {
                if let Some(prim) = drawing_prim(d, false) {
                    b.push(l, Role::Graphic, prim, None, None, vec![]);
                }
            }
        }
    }
    for side in ["F", "B"] {
        if let Some(list) = silk.inner.get(&format!("{side}_Clear")) {
            if !list.is_empty() {
                let l = b.side_layer(side, SILKSCREEN_CLEAR, 0);
                for d in list {
                    if let Some(prim) = drawing_prim(d, false) {
                        b.push(l, Role::Graphic, prim, None, None, vec![]);
                    }
                }
            }
        }
    }

    if !pcb.edges.is_empty() {
        let l = b.layer(
            EDGE_CUTS,
            LayerKind::EdgeCuts,
            Side::None,
            201,
            [0, 0, 0, 255],
        );
        for d in &pcb.edges {
            if let Some(prim) = drawing_prim(d, false) {
                b.push(l, Role::Outline, prim, None, None, vec![]);
            }
        }
    }
    if let Some(drills) = fab.inner.get(DRILLS).filter(|d| !d.is_empty()) {
        let l = b.layer(DRILLS, LayerKind::Drill, Side::None, 202, [0, 0, 0, 255]);
        for d in drills {
            if let Drawing::Circle { start, radius, .. } = d {
                let size = [radius * 2.0, radius * 2.0];
                let prim = Prim::Hole {
                    center: *start,
                    size,
                    rotation: 0.0,
                };
                b.push(l, Role::Hole, prim, None, None, vec![]);
            }
        }
    }

    // Pick bounds last: hidden, but hit-testable with the lowest priority.
    for (i, fp) in pcb.footprints.iter().enumerate() {
        let side = if fp.layer == "B" { "B" } else { "F" };
        let l = b.side_layer(side, BOUNDS, 0);
        b.push(
            l,
            Role::Other,
            footprint_bounds(fp),
            None,
            Some(i as GroupId),
            vec![],
        );
    }

    let m = &pcb.metadata;
    b.scene.meta = vec![
        Prop::new("title", m.title.clone()),
        Prop::new("revision", m.revision.clone()),
        Prop::new("company", m.company.clone()),
        Prop::new("date", m.date.clone()),
    ];
    if let Some(f) = pcb.format {
        let name = serde_json::to_value(f)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        b.scene.meta.push(Prop::new("format", name));
    }

    // Fit to the board outline when there is one, like the iBOM viewer.
    match &pcb.edges_bbox {
        Some(e) if e.minx.is_finite() && e.maxx >= e.minx => {
            b.scene.bbox = BBox {
                min: [e.minx, e.miny],
                max: [e.maxx, e.maxy],
            };
        }
        _ => b.scene.recompute_bbox(),
    }
    b.scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{extract_bytes, ExtractOptions, PcbFormat};
    use vector_view::hit::{hit_test, HitIndex};

    const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user) (44 "Edge.Cuts" user))
  (net 0 "") (net 1 "GND") (net 2 "VCC")
  (footprint "R_0603" (layer "F.Cu") (at 10 10 90)
    (property "Reference" "R1" (at 0 -1.5 90) (layer "F.SilkS")
      (effects (font (size 1 1) (thickness 0.15))))
    (property "Value" "1k" (at 0 1.5 90) (layer "F.Fab")
      (effects (font (size 1 1) (thickness 0.15))))
    (fp_line (start -1 -0.5) (end 1 -0.5) (layer "F.SilkS") (stroke (width 0.12) (type solid)))
    (pad "1" smd roundrect (at -0.8 0 90) (size 0.9 0.95) (layers "F.Cu" "F.Paste" "F.Mask")
      (roundrect_rratio 0.25) (net 1 "GND"))
    (pad "2" smd roundrect (at 0.8 0 90) (size 0.9 0.95) (layers "F.Cu" "F.Paste" "F.Mask")
      (roundrect_rratio 0.25) (net 2 "VCC")))
  (footprint "Conn_TH" (layer "B.Cu") (at 30 10)
    (property "Reference" "J1" (at 0 -2) (layer "B.SilkS")
      (effects (font (size 1 1) (thickness 0.15)) (justify mirror)))
    (pad "1" thru_hole rect (at 0 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 1 "GND"))
    (pad "2" thru_hole oval (at 2.54 0) (size 1.7 1.7) (drill 1) (layers "*.Cu" "*.Mask") (net 2 "VCC")))
  (segment (start 10 9.2) (end 30 10) (width 0.25) (layer "F.Cu") (net 1))
  (via (at 20 20) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net 2))
  (gr_rect (start 0 0) (end 40 30) (stroke (width 0.1) (type solid)) (layer "Edge.Cuts"))
)"#;

    fn kicad_scene() -> (PcbData, Scene) {
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: true,
        };
        let pcb = extract_bytes(BOARD.as_bytes(), PcbFormat::KiCad, &opts).expect("parse");
        let scene = to_scene(&pcb);
        (pcb, scene)
    }

    fn layer<'a>(s: &'a Scene, name: &str) -> &'a Layer {
        s.layers
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("layer {name} missing"))
    }

    fn items_on<'a>(s: &'a Scene, name: &str) -> Vec<&'a Item> {
        let id = layer(s, name).id;
        s.items.iter().filter(|i| i.layer == id).collect()
    }

    #[test]
    fn kicad_board_maps_onto_layers_groups_and_nets() {
        let (pcb, s) = kicad_scene();
        assert_eq!(s.kind, SceneKind::Pcb);
        assert!(s.y_down);
        assert_eq!(s.groups.len(), pcb.footprints.len());
        assert_eq!(s.groups[0].label, "R1");
        assert_eq!(s.groups[0].kind, GroupKind::Footprint);
        // Net ids follow PcbData::nets.
        let nets = pcb.nets.as_ref().unwrap();
        for n in &s.nets {
            assert_eq!(nets[n.id as usize], n.name);
        }

        let f_pads = items_on(&s, "F.Pads");
        let b_pads = items_on(&s, "B.Pads");
        // Two SMD pads on F; both TH pads appear on both sides.
        assert_eq!(f_pads.len(), 4);
        assert_eq!(b_pads.len(), 2);
        assert!(f_pads.iter().all(|i| i.role == Role::Pad));
        let gnd = s.nets.iter().find(|n| n.name == "GND").unwrap().id;
        assert!(f_pads
            .iter()
            .any(|i| i.net == Some(gnd) && i.group == Some(0)));
        assert!(f_pads
            .iter()
            .any(|i| i.props.contains(&Prop::new("pin1", "1"))));

        let tracks = items_on(&s, "F.Tracks");
        assert!(tracks
            .iter()
            .any(|i| i.role == Role::Track && i.net == Some(gnd)));
        assert!(tracks.iter().any(|i| i.role == Role::Via));
        assert!(tracks.iter().any(|i| i.role == Role::Hole));
        assert!(items_on(&s, "B.Tracks").iter().any(|i| i.role == Role::Via));

        assert_eq!(items_on(&s, HOLES).len(), 2);
        assert!(!items_on(&s, "F.Footprints").is_empty());
        assert_eq!(items_on(&s, EDGE_CUTS).len(), 1);
        assert!(!layer(&s, "F.Bounds").visible);
        assert_eq!(layer(&s, "F.Pads").side, Side::Front);
        assert!(layer(&s, "F.Pads").z > layer(&s, "F.Tracks").z);
        assert!(layer(&s, "B.Pads").z < layer(&s, "F.Tracks").z);
        assert!(layer(&s, EDGE_CUTS).z > layer(&s, "F.Pads").z);

        // Scene bbox is the board outline.
        assert_eq!(s.bbox.min, [0.0, 0.0]);
        assert_eq!(s.bbox.max, [40.0, 30.0]);
        let ids: std::collections::HashSet<_> = s.items.iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), s.items.len(), "item ids unique");
    }

    #[test]
    fn pads_are_placed_with_rotation_and_pickable() {
        let (pcb, s) = kicad_scene();
        let visible = |id: LayerId| s.layer(id).is_some_and(|l| l.visible);
        // Each SMD pad centre hits a pad of R1 with the pad's net.
        for pad in &pcb.footprints[0].pads {
            let hit = hit_test(&s, pad.pos, 0.0, &visible).expect("pad hit");
            let item = s.items.iter().find(|i| i.id == hit).unwrap();
            assert_eq!(item.role, Role::Pad);
            assert_eq!(item.group, Some(0));
            assert_eq!(item.net.map(|n| s.nets[n as usize].name.clone()), pad.net);
        }
        // Between the pads only the hidden bounds (when allowed) are under the cursor.
        let mid = pcb.footprints[0].center;
        assert_eq!(hit_test(&s, mid, 0.0, &visible), None);
        let all = |_: LayerId| true;
        let idx = HitIndex::new(&s);
        let hit = idx.hit_test(&s, mid, 0.0, &all).unwrap();
        let item = s.items.iter().find(|i| i.id == hit).unwrap();
        assert_eq!((item.role, item.group), (Role::Other, Some(0)));
    }

    #[test]
    fn rounded_and_chamfered_rects() {
        let r = rounded_rect([2.0, 1.0], 0.0, 0, 0.0);
        assert_eq!(r, vec![[-1.0, 0.5], [1.0, 0.5], [1.0, -0.5], [-1.0, -0.5]]);
        let c = rounded_rect([2.0, 2.0], 0.0, 1, 0.25);
        // Top-left corner replaced by a 0.5 chamfer.
        assert!(c.contains(&[-0.5, -1.0]) && c.contains(&[-1.0, -0.5]));
        let o = rounded_rect([2.0, 1.0], 0.5, 0, 0.0);
        for p in &o {
            assert!(p[0].abs() <= 1.0 + 1e-9 && p[1].abs() <= 0.5 + 1e-9);
        }
    }

    #[test]
    fn stroke_font_text_becomes_strokes() {
        let mut font = FontData::new();
        font.insert(
            "I".into(),
            crate::types::GlyphData {
                w: 0.6,
                l: vec![vec![[0.3, -1.0], [0.3, 0.0]]],
            },
        );
        let t = TextDrawing {
            svgpath: None,
            thickness: Some(0.1),
            is_ref: Some(1),
            val: None,
            pos: Some([5.0, 5.0]),
            text: Some("II".into()),
            height: Some(1.0),
            width: Some(1.0),
            justify: Some([0, 0]),
            angle: Some(0.0),
            attr: None,
        };
        let Some(Prim::Strokes { strokes, width }) = text_strokes(&t, &font) else {
            panic!("expected strokes");
        };
        assert_eq!(strokes.len(), 2);
        assert_eq!(width, 0.1);
        // Centred about x = 5 (+ half the thickness), vertical glyph lines.
        let xs: Vec<f64> = strokes.iter().map(|s| s[0][0]).collect();
        assert!((xs[0] + xs[1]) / 2.0 - 5.05 < 0.1);
        assert!(strokes.iter().all(|s| (s[0][0] - s[1][0]).abs() < 1e-9));
        assert_eq!(text_props(&t)[0], Prop::new("kind", "ref"));
    }

    #[test]
    fn eagle_fixture_converts() {
        let data = include_bytes!("../test-fixtures/eagle-binary/grove-button.brd");
        let opts = ExtractOptions {
            include_tracks: true,
            include_nets: true,
        };
        let pcb = extract_bytes(data, PcbFormat::Eagle, &opts).expect("parse");
        let s = to_scene(&pcb);
        assert_eq!(s.groups.len(), pcb.footprints.len());
        let pads: usize = pcb
            .footprints
            .iter()
            .map(|f| f.pads.iter().map(|p| p.layers.len()).sum::<usize>())
            .sum();
        let pad_items = s.items.iter().filter(|i| i.role == Role::Pad).count();
        assert!(pad_items <= pads && pad_items > 0);
        assert!(!s.bbox.is_empty());
        // Round-trips through JSON (what a server would ship).
        let json = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items.len(), s.items.len());
    }
}
