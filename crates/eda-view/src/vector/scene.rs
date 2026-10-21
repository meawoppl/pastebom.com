//! The scene contract: layers, primitives, pick ids, nets, groups, properties.
//!
//! Units are millimetres in the document's own frame. `y_down` records the
//! producer's axis convention (KiCad is y-down) so viewers never guess.
//! Everything a viewer needs to draw, hit-test, highlight or show in a
//! properties panel is here; nothing requires knowing the source format.

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to this schema.
pub const SCENE_VERSION: u32 = 1;

pub type Point = [f64; 2];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scene {
    pub version: u32,
    /// What the scene depicts (drives sensible viewer defaults).
    pub kind: SceneKind,
    /// `true` when +y points down the page (KiCad, Gerber viewers flip).
    pub y_down: bool,
    pub bbox: BBox,
    /// Drawing order: lower `z` first.
    pub layers: Vec<Layer>,
    pub items: Vec<Item>,
    #[serde(default)]
    pub nets: Vec<Net>,
    /// Footprints, symbols, sheets, zones: things selection expands to.
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Free-form document facts (source path, revision, title).
    #[serde(default)]
    pub meta: Vec<Prop>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SceneKind {
    Pcb,
    Schematic,
    Footprint,
    Symbol,
    Gerber,
    Other,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct BBox {
    pub min: Point,
    pub max: Point,
}

impl BBox {
    pub const EMPTY: BBox = BBox {
        min: [f64::INFINITY, f64::INFINITY],
        max: [f64::NEG_INFINITY, f64::NEG_INFINITY],
    };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1]
    }

    pub fn include(&mut self, p: Point) {
        self.min[0] = self.min[0].min(p[0]);
        self.min[1] = self.min[1].min(p[1]);
        self.max[0] = self.max[0].max(p[0]);
        self.max[1] = self.max[1].max(p[1]);
    }

    /// Grow by `r` in every direction (stroke half-widths, radii).
    pub fn include_padded(&mut self, p: Point, r: f64) {
        self.include([p[0] - r, p[1] - r]);
        self.include([p[0] + r, p[1] + r]);
    }

    pub fn union(&mut self, other: &BBox) {
        if !other.is_empty() {
            self.include(other.min);
            self.include(other.max);
        }
    }

    pub fn width(&self) -> f64 {
        (self.max[0] - self.min[0]).max(0.0)
    }

    pub fn height(&self) -> f64 {
        (self.max[1] - self.min[1]).max(0.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p[0] >= self.min[0] && p[0] <= self.max[0] && p[1] >= self.min[1] && p[1] <= self.max[1]
    }
}

pub type LayerId = u16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    /// Display name, e.g. "F.Cu", "B.Silkscreen", "Wires".
    pub name: String,
    pub kind: LayerKind,
    pub side: Side,
    pub z: i32,
    /// Default colour, RGBA 0-255. Viewers may theme over it.
    pub color: [u8; 4],
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    Copper,
    Silkscreen,
    SolderMask,
    Paste,
    Fabrication,
    Courtyard,
    EdgeCuts,
    Drill,
    Drawing,
    Text,
    /// Schematic: wires, buses, junctions, no-connects.
    Connectivity,
    /// Schematic: symbol bodies and pins.
    Symbol,
    /// Schematic: sheet frames and sheet pins.
    Sheet,
    /// Selection/highlight overlays the producer wants drawn on top.
    Overlay,
    Other,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Front,
    Back,
    Inner,
    #[default]
    None,
}

pub type ItemId = u32;
pub type NetId = u32;
pub type GroupId = u32;

/// One drawable, pickable thing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub layer: LayerId,
    pub role: Role,
    pub prim: Prim,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<NetId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
    /// Per-item properties for the properties panel (pad number, width...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Prop>,
}

/// What an item is, so viewers can pick sensible hit-test priority and
/// highlight behaviour without understanding the source format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Pad,
    Track,
    Via,
    Zone,
    Hole,
    Graphic,
    Text,
    Outline,
    Wire,
    Bus,
    Junction,
    NoConnect,
    Pin,
    Label,
    Field,
    SymbolBody,
    SheetFrame,
    Other,
}

/// Geometry. Text arrives pre-converted to strokes (`Strokes`) so viewers
/// never need fonts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Prim {
    /// Open path with round caps/joins of `width`.
    Polyline { points: Vec<Point>, width: f64 },
    /// Closed area with optional holes; `stroke` > 0 also outlines it.
    Polygon {
        outer: Vec<Point>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        holes: Vec<Vec<Point>>,
        fill: bool,
        #[serde(default)]
        stroke: f64,
    },
    Circle {
        center: Point,
        radius: f64,
        fill: bool,
        #[serde(default)]
        stroke: f64,
    },
    /// Counter-clockwise from `start` to `end` (radians) in scene axes.
    Arc {
        center: Point,
        radius: f64,
        start: f64,
        end: f64,
        width: f64,
    },
    /// Stroked glyph outlines for one text run.
    Strokes {
        strokes: Vec<Vec<Point>>,
        width: f64,
    },
    /// Drill hole or slot: an oblong of `size` rotated by `rotation` (radians).
    Hole {
        center: Point,
        size: [f64; 2],
        #[serde(default)]
        rotation: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Net {
    pub id: NetId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub kind: GroupKind,
    /// Short label, e.g. "R1", "U3", "Power sheet".
    pub label: String,
    #[serde(default)]
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    Footprint,
    Symbol,
    Sheet,
    Zone,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prop {
    pub key: String,
    pub value: String,
}

impl Prop {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Prop {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl Prim {
    /// Axis-aligned bounds including stroke half-widths.
    pub fn bbox(&self) -> BBox {
        let mut b = BBox::EMPTY;
        match self {
            Prim::Polyline { points, width } => {
                for p in points {
                    b.include_padded(*p, width / 2.0);
                }
            }
            Prim::Polygon { outer, stroke, .. } => {
                for p in outer {
                    b.include_padded(*p, stroke / 2.0);
                }
            }
            Prim::Circle {
                center,
                radius,
                stroke,
                ..
            } => b.include_padded(*center, radius + stroke / 2.0),
            Prim::Arc {
                center,
                radius,
                width,
                ..
            } => b.include_padded(*center, radius + width / 2.0),
            Prim::Strokes { strokes, width } => {
                for s in strokes {
                    for p in s {
                        b.include_padded(*p, width / 2.0);
                    }
                }
            }
            Prim::Hole { center, size, .. } => {
                b.include_padded(*center, size[0].max(size[1]) / 2.0)
            }
        }
        b
    }
}

impl Scene {
    pub fn new(kind: SceneKind, y_down: bool) -> Self {
        Scene {
            version: SCENE_VERSION,
            kind,
            y_down,
            bbox: BBox::EMPTY,
            layers: Vec::new(),
            items: Vec::new(),
            nets: Vec::new(),
            groups: Vec::new(),
            meta: Vec::new(),
        }
    }

    /// Recompute `bbox` from all items.
    pub fn recompute_bbox(&mut self) {
        let mut b = BBox::EMPTY;
        for item in &self.items {
            b.union(&item.prim.bbox());
        }
        self.bbox = b;
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn net(&self, id: NetId) -> Option<&Net> {
        self.nets.iter().find(|n| n.id == id)
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Scene {
        let mut s = Scene::new(SceneKind::Pcb, true);
        s.layers.push(Layer {
            id: 0,
            name: "F.Cu".into(),
            kind: LayerKind::Copper,
            side: Side::Front,
            z: 10,
            color: [200, 52, 52, 255],
            visible: true,
        });
        s.nets.push(Net {
            id: 1,
            name: "GND".into(),
        });
        s.groups.push(Group {
            id: 7,
            kind: GroupKind::Footprint,
            label: "R1".into(),
            props: vec![Prop::new("value", "1k")],
        });
        s.items.push(Item {
            id: 1,
            layer: 0,
            role: Role::Track,
            prim: Prim::Polyline {
                points: vec![[0.0, 0.0], [10.0, 0.0]],
                width: 0.25,
            },
            net: Some(1),
            group: None,
            props: vec![],
        });
        s.items.push(Item {
            id: 2,
            layer: 0,
            role: Role::Pad,
            prim: Prim::Circle {
                center: [10.0, 5.0],
                radius: 1.0,
                fill: true,
                stroke: 0.0,
            },
            net: Some(1),
            group: Some(7),
            props: vec![Prop::new("pad", "1")],
        });
        s.recompute_bbox();
        s
    }

    #[test]
    fn json_round_trip() {
        let s = sample();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"t\":\"polyline\""));
        let back: Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn bbox_includes_stroke_and_radius() {
        let s = sample();
        assert_eq!(s.bbox.min, [-0.125, -0.125]);
        assert_eq!(s.bbox.max, [11.0, 6.0]);
    }

    #[test]
    fn lookups() {
        let s = sample();
        assert_eq!(s.layer(0).unwrap().name, "F.Cu");
        assert_eq!(s.net(1).unwrap().name, "GND");
        assert_eq!(s.group(7).unwrap().label, "R1");
        assert!(s.layer(9).is_none());
    }
}
