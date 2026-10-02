//! Everything that decides *how* a scene is drawn, independent of the canvas
//! backend: visibility, paint order, highlight, theme colours and alpha.
//! The canvas renderer (`render`, feature `canvas`) only executes what this
//! module resolves, so the decisions are unit-testable natively.

use crate::scene::{GroupId, Item, ItemId, Layer, LayerId, NetId, Role, Scene, Side};
use crate::view::View;
use std::collections::{HashMap, HashSet};

pub type Rgba = [u8; 4];

/// Which items are emphasised.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Highlight {
    #[default]
    None,
    Items(HashSet<ItemId>),
    Net(NetId),
    Group(HashSet<GroupId>),
}

impl Highlight {
    pub fn is_active(&self) -> bool {
        match self {
            Highlight::None => false,
            Highlight::Items(s) => !s.is_empty(),
            Highlight::Group(s) => !s.is_empty(),
            Highlight::Net(_) => true,
        }
    }

    pub fn contains(&self, item: &Item) -> bool {
        match self {
            Highlight::None => false,
            Highlight::Items(s) => s.contains(&item.id),
            Highlight::Net(n) => item.net == Some(*n),
            Highlight::Group(s) => item.group.is_some_and(|g| s.contains(&g)),
        }
    }
}

/// How a highlight is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HighlightStyle {
    /// Draw everything; non-highlighted items are multiplied by `dim`.
    #[default]
    Dim,
    /// Draw only highlighted items (an overlay pass over a normal draw).
    Only,
}

/// Colour overrides layered over the scene's own layer colours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Theme {
    /// Replaces `Layer::color`.
    pub layer_colors: HashMap<LayerId, Rgba>,
    /// Colour for filled areas (polygons, filled circles) on a layer, when it
    /// should differ from the stroke colour.
    pub fill_colors: HashMap<LayerId, Rgba>,
    /// Colour for highlighted items (holes keep `hole`).
    pub highlight: Option<Rgba>,
    /// Colour for `Prim::Hole`; defaults to the layer colour.
    pub hole: Option<Rgba>,
}

/// Visibility overrides on top of `Layer::visible`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerVisibility {
    pub overrides: HashMap<LayerId, bool>,
}

impl LayerVisibility {
    pub fn is_visible(&self, layer: &Layer) -> bool {
        self.overrides
            .get(&layer.id)
            .copied()
            .unwrap_or(layer.visible)
    }

    pub fn set(&mut self, id: LayerId, visible: bool) {
        self.overrides.insert(id, visible);
    }

    /// Predicate for [`crate::hit::hit_test`].
    pub fn predicate<'a>(&'a self, scene: &'a Scene) -> impl Fn(LayerId) -> bool + 'a {
        move |id| scene.layer(id).is_some_and(|l| self.is_visible(l))
    }
}

/// One compositing pass: the items of `layer` (optionally only some roles)
/// drawn opaque into an offscreen canvas, then composited at `alpha` so
/// overlapping translucent items blend as one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub layer: LayerId,
    /// Only these roles (e.g. split one copper layer into zones/tracks/pads).
    pub roles: Option<Vec<Role>>,
    pub alpha: f64,
}

impl Pass {
    pub fn new(layer: LayerId, alpha: f64) -> Self {
        Self {
            layer,
            roles: None,
            alpha,
        }
    }

    pub fn roles(layer: LayerId, roles: &[Role], alpha: f64) -> Self {
        Self {
            layer,
            roles: Some(roles.to_vec()),
            alpha,
        }
    }

    pub fn matches(&self, item: &Item) -> bool {
        item.layer == self.layer && self.roles.as_ref().is_none_or(|r| r.contains(&item.role))
    }
}

/// A highlight drawn on top of everything in a single colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub highlight: Highlight,
    pub color: Rgba,
    /// Composited as one unit at this opacity.
    pub alpha: f64,
}

/// A dot grid at decade spacings (0.1, 1, 10 ... scene units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub color: Rgba,
    /// The finest decade whose dots are at least this far apart is used.
    pub min_spacing_px: f64,
    /// Dot size in CSS pixels.
    pub dot_px: f64,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            color: [128, 128, 128, 160],
            min_spacing_px: 12.0,
            dot_px: 1.5,
        }
    }
}

impl Grid {
    /// Grid spacing in scene units for `px_per_unit` CSS pixels per unit.
    pub fn spacing(&self, px_per_unit: f64) -> f64 {
        let min = self.min_spacing_px.max(1e-6) / px_per_unit.max(1e-12);
        10f64.powf(min.log10().ceil())
    }
}

/// One entry of the resolved paint plan (bottom first).
#[derive(Debug, Clone, Copy)]
pub struct PlannedPass<'a> {
    pub layer: &'a Layer,
    pub roles: Option<&'a [Role]>,
    /// Pass opacity (`1.0` without passes, where `layer_alpha` applies per item).
    pub alpha: f64,
    /// True when this entry came from `ViewState::passes`.
    pub composited: bool,
}

impl PlannedPass<'_> {
    pub fn matches(&self, item: &Item) -> bool {
        item.layer == self.layer.id && self.roles.is_none_or(|r| r.contains(&item.role))
    }
}

/// Everything `render::draw` needs besides the scene and the context.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub view: View,
    /// Device pixel ratio of the canvas backing store.
    pub dpr: f64,
    pub visibility: LayerVisibility,
    /// Per-layer opacity multiplier (default 1).
    pub layer_alpha: HashMap<LayerId, f64>,
    /// Explicit paint order; layers not listed are not drawn. `None` sorts
    /// by `z`, negating it for sided layers when the view is mirrored
    /// (bottom view) so the far side is painted first.
    pub layer_order: Option<Vec<LayerId>>,
    pub highlight: Highlight,
    pub highlight_style: HighlightStyle,
    pub theme: Theme,
    /// Alpha multiplier for non-highlighted items under `HighlightStyle::Dim`.
    pub dim: f64,
    /// Thinnest line drawn, in CSS pixels.
    pub min_line_px: f64,
    /// Items never drawn (e.g. reference designators toggled off).
    pub hidden_items: HashSet<ItemId>,
    /// Filled items drawn as outlines instead.
    pub outline_items: HashSet<ItemId>,
    /// Layers painted with `destination-out`: their shapes erase what is
    /// already on the canvas (clear polarity, see-through drills).
    pub erase_layers: HashSet<LayerId>,
    /// Compositing passes; when set they define paint order (overriding `z`
    /// and `layer_order`) and replace `layer_alpha`.
    pub passes: Option<Vec<Pass>>,
    /// Fill the canvas with this colour before drawing.
    pub background: Option<Rgba>,
    /// Dot grid drawn over the background, under the layers.
    pub grid: Option<Grid>,
    /// Highlight drawn over everything in one colour (`outline_items` are
    /// drawn as outlines there too).
    pub overlay: Option<Overlay>,
}

impl ViewState {
    pub fn new(view: View) -> Self {
        Self {
            view,
            dpr: 1.0,
            visibility: LayerVisibility::default(),
            layer_alpha: HashMap::new(),
            layer_order: None,
            highlight: Highlight::None,
            highlight_style: HighlightStyle::Dim,
            theme: Theme::default(),
            dim: 0.3,
            min_line_px: 0.5,
            hidden_items: HashSet::new(),
            outline_items: HashSet::new(),
            erase_layers: HashSet::new(),
            passes: None,
            background: None,
            grid: None,
            overlay: None,
        }
    }

    /// The resolved paint plan, bottom first: `passes` when set (invisible
    /// layers skipped), otherwise [`ViewState::paint_order`].
    pub fn paint_plan<'a>(&'a self, scene: &'a Scene) -> Vec<PlannedPass<'a>> {
        match &self.passes {
            Some(passes) => passes
                .iter()
                .filter_map(|p| {
                    let layer = scene.layer(p.layer)?;
                    self.visibility.is_visible(layer).then_some(PlannedPass {
                        layer,
                        roles: p.roles.as_deref(),
                        alpha: p.alpha,
                        composited: true,
                    })
                })
                .collect(),
            None => self
                .paint_order(scene)
                .into_iter()
                .map(|layer| PlannedPass {
                    layer,
                    roles: None,
                    alpha: 1.0,
                    composited: false,
                })
                .collect(),
        }
    }

    /// Picking order derived from the paint plan: for each item, the index
    /// of the topmost plan entry that draws it, or `None` when it is not
    /// drawn (invisible layer, filtered role, hidden item).
    pub fn pick_plan(&self, scene: &Scene) -> PickPlan {
        let mut by_layer: HashMap<LayerId, Vec<PlanSlot>> = HashMap::new();
        for (i, p) in self.paint_plan(scene).iter().enumerate() {
            by_layer
                .entry(p.layer.id)
                .or_default()
                .push((i, p.roles.map(<[Role]>::to_vec)));
        }
        PickPlan {
            by_layer,
            hidden: self.hidden_items.clone(),
        }
    }

    /// Visible layers in paint order (first painted first).
    pub fn paint_order<'a>(&self, scene: &'a Scene) -> Vec<&'a Layer> {
        let layers: Vec<&Layer> = match &self.layer_order {
            Some(order) => order.iter().filter_map(|id| scene.layer(*id)).collect(),
            None => {
                let mut ls: Vec<&Layer> = scene.layers.iter().collect();
                let flip = self.view.mirrored;
                ls.sort_by_key(|l| {
                    if flip && l.side != Side::None {
                        -l.z
                    } else {
                        l.z
                    }
                });
                ls
            }
        };
        layers
            .into_iter()
            .filter(|l| self.visibility.is_visible(l))
            .collect()
    }

    /// Opacity for a whole layer.
    pub fn layer_opacity(&self, layer: &Layer) -> f64 {
        self.layer_alpha.get(&layer.id).copied().unwrap_or(1.0)
    }

    /// Resolve the paint for one item, or `None` to skip it.
    pub fn item_paint(&self, layer: &Layer, item: &Item) -> Option<ItemPaint> {
        self.item_paint_with(layer, item, self.layer_opacity(layer))
    }

    /// Like [`ViewState::item_paint`] with an explicit base opacity (passes
    /// draw items opaque and apply their alpha when compositing).
    pub fn item_paint_with(
        &self,
        layer: &Layer,
        item: &Item,
        base_alpha: f64,
    ) -> Option<ItemPaint> {
        if self.hidden_items.contains(&item.id) {
            return None;
        }
        let active = self.highlight.is_active();
        let lit = active && self.highlight.contains(item);
        if active && !lit && self.highlight_style == HighlightStyle::Only {
            return None;
        }
        let base = self
            .theme
            .layer_colors
            .get(&layer.id)
            .copied()
            .unwrap_or(layer.color);
        let fill_base = self
            .theme
            .fill_colors
            .get(&layer.id)
            .copied()
            .unwrap_or(base);
        let (stroke, fill) = if item.role == Role::Hole {
            let c = self.theme.hole.unwrap_or(base);
            (c, c)
        } else if lit {
            match self.theme.highlight {
                Some(h) => (h, h),
                None => (base, fill_base),
            }
        } else {
            (base, fill_base)
        };
        let mut alpha = base_alpha;
        if active && !lit {
            alpha *= self.dim;
        }
        Some(ItemPaint {
            stroke,
            fill,
            alpha,
            outline: self.outline_items.contains(&item.id),
        })
    }

    /// Minimum line width in scene units.
    pub fn min_line_world(&self) -> f64 {
        self.min_line_px * self.view.world_per_px()
    }
}

/// Resolved paint for one item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemPaint {
    pub stroke: Rgba,
    pub fill: Rgba,
    /// Multiplies the colours' own alpha.
    pub alpha: f64,
    /// Stroke filled shapes instead of filling them.
    pub outline: bool,
}

/// A plan index and its optional role filter.
type PlanSlot = (usize, Option<Vec<Role>>);

/// Which items are pickable and how they stack; see [`ViewState::pick_plan`].
#[derive(Debug, Clone, Default)]
pub struct PickPlan {
    by_layer: HashMap<LayerId, Vec<PlanSlot>>,
    hidden: HashSet<ItemId>,
}

impl PickPlan {
    /// Index of the topmost plan entry drawing `item`, if any.
    pub fn rank(&self, item: &Item) -> Option<usize> {
        if self.hidden.contains(&item.id) {
            return None;
        }
        self.by_layer
            .get(&item.layer)?
            .iter()
            .filter(|(_, roles)| roles.as_ref().is_none_or(|r| r.contains(&item.role)))
            .map(|(i, _)| *i)
            .max()
    }
}

/// CSS colour string for an RGBA colour at `alpha` extra opacity.
pub fn css_rgba(c: Rgba, alpha: f64) -> String {
    let a = (c[3] as f64 / 255.0) * alpha.clamp(0.0, 1.0);
    format!("rgba({},{},{},{:.4})", c[0], c[1], c[2], a)
}

/// Parse `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(...)`, `rgba(...)` and
/// a few names. Returns `None` for anything else.
pub fn parse_css_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => {
                let mut out = [0, 0, 0, 255];
                for (i, o) in out.iter_mut().enumerate().take(hex.len()) {
                    *o = digit(i)? * 17;
                }
                Some(out)
            }
            6 | 8 => {
                let mut out = [0, 0, 0, 255];
                for (i, o) in out.iter_mut().enumerate().take(hex.len() / 2) {
                    *o = byte(i * 2)?;
                }
                Some(out)
            }
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    if let Some(args) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let mut out = [0, 0, 0, 255];
        for i in 0..3 {
            out[i] = parts[i].parse::<f64>().ok()?.round().clamp(0.0, 255.0) as u8;
        }
        if let Some(a) = parts.get(3) {
            let a = match a.strip_suffix('%') {
                Some(p) => p.parse::<f64>().ok()? / 100.0,
                None => a.parse::<f64>().ok()?,
            };
            out[3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        return Some(out);
    }
    match lower.as_str() {
        "black" => Some([0, 0, 0, 255]),
        "white" => Some([255, 255, 255, 255]),
        "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::*;

    fn scene() -> Scene {
        let mut s = Scene::new(SceneKind::Pcb, true);
        let layer = |id, side, z| Layer {
            id,
            name: format!("L{id}"),
            kind: LayerKind::Copper,
            side,
            z,
            color: [10, 20, 30, 255],
            visible: true,
        };
        s.layers = vec![
            layer(0, Side::Front, 10),
            layer(1, Side::Back, 1),
            layer(2, Side::None, 100),
        ];
        s.layers.push(Layer {
            visible: false,
            ..layer(3, Side::Front, 11)
        });
        let item = |id, net, group, role| Item {
            id,
            layer: 0,
            role,
            prim: Prim::Circle {
                center: [0.0, 0.0],
                radius: 1.0,
                fill: true,
                stroke: 0.0,
            },
            net,
            group,
            props: vec![],
        };
        s.items = vec![
            item(1, Some(5), Some(9), Role::Pad),
            item(2, Some(6), None, Role::Track),
            item(3, Some(5), Some(9), Role::Hole),
        ];
        s
    }

    fn ids(ls: Vec<&Layer>) -> Vec<LayerId> {
        ls.into_iter().map(|l| l.id).collect()
    }

    #[test]
    fn paint_order_flips_sided_layers_for_bottom_view() {
        let s = scene();
        let mut st = ViewState::new(View::default());
        assert_eq!(ids(st.paint_order(&s)), vec![1, 0, 2]);
        st.view.mirrored = true;
        assert_eq!(ids(st.paint_order(&s)), vec![0, 1, 2]);
        st.visibility.set(3, true);
        st.visibility.set(2, false);
        assert_eq!(ids(st.paint_order(&s)), vec![3, 0, 1]);
        st.layer_order = Some(vec![2, 1]);
        assert_eq!(ids(st.paint_order(&s)), vec![1]);
    }

    #[test]
    fn highlight_dims_others_and_only_skips_them() {
        let s = scene();
        let l = &s.layers[0];
        let mut st = ViewState::new(View::default());
        st.theme.highlight = Some([0, 255, 0, 255]);
        st.theme.hole = Some([1, 1, 1, 255]);
        st.highlight = Highlight::Net(5);
        let lit = st.item_paint(l, &s.items[0]).unwrap();
        assert_eq!((lit.fill, lit.alpha), ([0, 255, 0, 255], 1.0));
        let other = st.item_paint(l, &s.items[1]).unwrap();
        assert_eq!((other.fill, other.alpha), ([10, 20, 30, 255], 0.3));
        // Holes keep their colour even when highlighted.
        assert_eq!(st.item_paint(l, &s.items[2]).unwrap().fill, [1, 1, 1, 255]);
        st.highlight_style = HighlightStyle::Only;
        assert!(st.item_paint(l, &s.items[1]).is_none());
        st.highlight = Highlight::Group([9].into());
        assert!(st.item_paint(l, &s.items[0]).is_some());
        st.highlight = Highlight::Items([2].into());
        assert!(st.item_paint(l, &s.items[0]).is_none());
        st.hidden_items.insert(2);
        assert!(st.item_paint(l, &s.items[1]).is_none());
    }

    #[test]
    fn theme_overrides_and_layer_alpha() {
        let s = scene();
        let l = &s.layers[0];
        let mut st = ViewState::new(View::default());
        st.theme.layer_colors.insert(0, [1, 2, 3, 255]);
        st.theme.fill_colors.insert(0, [4, 5, 6, 128]);
        st.layer_alpha.insert(0, 0.35);
        st.outline_items.insert(1);
        let p = st.item_paint(l, &s.items[0]).unwrap();
        assert_eq!(p.stroke, [1, 2, 3, 255]);
        assert_eq!(p.fill, [4, 5, 6, 128]);
        assert_eq!(p.alpha, 0.35);
        assert!(p.outline);
        assert_eq!(css_rgba([4, 5, 6, 255], 0.5), "rgba(4,5,6,0.5000)");
    }

    #[test]
    fn passes_define_order_and_skip_invisible_layers() {
        let s = scene();
        let mut st = ViewState::new(View::default());
        st.passes = Some(vec![
            Pass::new(2, 1.0),
            Pass::roles(0, &[Role::Track], 0.5),
            Pass::new(3, 1.0),
            Pass::roles(0, &[Role::Pad, Role::Hole], 0.25),
        ]);
        let plan = st.paint_plan(&s);
        let order: Vec<(LayerId, f64)> = plan.iter().map(|p| (p.layer.id, p.alpha)).collect();
        // Layer 3 is invisible by default; layer z is ignored.
        assert_eq!(order, vec![(2, 1.0), (0, 0.5), (0, 0.25)]);
        assert!(plan.iter().all(|p| p.composited));
        assert!(plan[1].matches(&s.items[1]) && !plan[1].matches(&s.items[0]));

        let picks = st.pick_plan(&s);
        assert_eq!(picks.rank(&s.items[0]), Some(2));
        assert_eq!(picks.rank(&s.items[1]), Some(1));
        st.hidden_items.insert(1);
        assert_eq!(st.pick_plan(&s).rank(&s.items[0]), None);

        st.passes = None;
        let plan = st.paint_plan(&s);
        assert!(plan.iter().all(|p| !p.composited && p.alpha == 1.0));
        assert_eq!(st.pick_plan(&s).rank(&s.items[1]), Some(1));
    }

    #[test]
    fn pass_paint_ignores_layer_alpha() {
        let s = scene();
        let mut st = ViewState::new(View::default());
        st.layer_alpha.insert(0, 0.35);
        let l = &s.layers[0];
        assert_eq!(st.item_paint(l, &s.items[0]).unwrap().alpha, 0.35);
        assert_eq!(st.item_paint_with(l, &s.items[0], 1.0).unwrap().alpha, 1.0);
    }

    #[test]
    fn grid_uses_decades() {
        let g = Grid::default();
        // 12 px minimum: at 10 px/mm the 10 mm decade is the first wide enough.
        assert_eq!(g.spacing(10.0), 10.0);
        assert_eq!(g.spacing(12.0), 1.0);
        assert_eq!(g.spacing(100.0), 1.0);
        assert!((g.spacing(150.0) - 0.1).abs() < 1e-12);
        assert_eq!(g.spacing(0.5), 100.0);
    }

    #[test]
    fn visibility_predicate() {
        let s = scene();
        let mut v = LayerVisibility::default();
        assert!(!(v.predicate(&s))(3));
        v.set(3, true);
        v.set(0, false);
        let pred = v.predicate(&s);
        assert!(pred(3) && !pred(0) && !pred(42));
    }

    #[test]
    fn css_colors() {
        assert_eq!(parse_css_color("#aa4"), Some([170, 170, 68, 255]));
        assert_eq!(parse_css_color("#00ee4480"), Some([0, 238, 68, 128]));
        assert_eq!(parse_css_color(" #CCCCCC "), Some([204, 204, 204, 255]));
        assert_eq!(parse_css_color("rgb(1, 2, 3)"), Some([1, 2, 3, 255]));
        assert_eq!(parse_css_color("rgba(1,2,3,0.5)"), Some([1, 2, 3, 128]));
        assert_eq!(parse_css_color("rgb(1 2 3 / 50%)"), Some([1, 2, 3, 128]));
        assert_eq!(parse_css_color("black"), Some([0, 0, 0, 255]));
        assert_eq!(parse_css_color("#12"), None);
        assert_eq!(parse_css_color("hsl(1,2%,3%)"), None);
    }
}
