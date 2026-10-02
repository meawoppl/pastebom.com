//! Board rendering and picking through `vector-view`.
//!
//! `pcb_extract::scene::to_scene` turns the board into a plain vector scene;
//! this module only decides, per canvas and per pass, which scene layers to
//! show at what opacity and colour so the result matches the classic iBOM
//! look (see-through opposite side, dimmed inner layers, highlight overlay).

use std::collections::{HashMap, HashSet};

use pcb_extract::scene::{
    layer_name, BOUNDS, COPPER_PADS, DRILLS, EDGE_CUTS, FAB, FOOTPRINTS, HOLES, PADS, SILKSCREEN,
    SILKSCREEN_CLEAR, TRACKS, ZONES,
};
use vector_view::hit::HitIndex;
use vector_view::render::{self as vr, PathCache};
use vector_view::style::{parse_css_color, Highlight, HighlightStyle, Rgba, ViewState};
use vector_view::view::View;
use vector_view::{GroupId, ItemId, LayerId, NetId, Prop, Role, Scene};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::state::Settings;

fn deg2rad(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

/// Theme colours read from the page's CSS custom properties.
#[derive(Clone)]
pub struct Colors {
    pub pcb_edge: Rgba,
    pub pad: Rgba,
    pub pad_hole: Rgba,
    pub pad_highlight: Rgba,
    pub pad_highlight_both: Rgba,
    pub pad_highlight_marked: Rgba,
    pub pin1_outline: Rgba,
    pub pin1_outline_highlight: Rgba,
    pub pin1_outline_highlight_both: Rgba,
    pub pin1_outline_highlight_marked: Rgba,
    pub silk_edge: Rgba,
    pub silk_polygon: Rgba,
    pub fab_edge: Rgba,
    pub fab_polygon: Rgba,
    pub track_front: Rgba,
    pub track_back: Rgba,
    pub track_highlight: Rgba,
    pub zone_front: Rgba,
    pub zone_back: Rgba,
    pub zone_highlight: Rgba,
}

impl Colors {
    pub fn from_element(el: &web_sys::Element) -> Self {
        let style = web_sys::window()
            .unwrap()
            .get_computed_style(el)
            .unwrap()
            .unwrap();
        let g = |name: &str| -> Rgba {
            let v = style.get_property_value(name).unwrap_or_default();
            parse_css_color(&v).unwrap_or([255, 0, 255, 255])
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
            fab_edge: g("--fabrication-edge-color"),
            fab_polygon: g("--fabrication-polygon-color"),
            track_front: g("--track-color-front"),
            track_back: g("--track-color-back"),
            track_highlight: g("--track-color-highlight"),
            zone_front: g("--zone-color-front"),
            zone_back: g("--zone-color-back"),
            zone_highlight: g("--zone-color-highlight"),
        }
    }

    fn track(&self, side: &str) -> Rgba {
        if side == "F" {
            self.track_front
        } else {
            self.track_back
        }
    }

    fn zone(&self, side: &str) -> Rgba {
        if side == "F" {
            self.zone_front
        } else {
            self.zone_back
        }
    }
}

/// The board as a scene plus the lookups the iBOM UI needs.
pub struct Board {
    pub scene: Scene,
    index: HitIndex,
    layers: HashMap<String, LayerId>,
    /// Inner copper names, sorted (as in the layer panel).
    inner: Vec<String>,
    pin1_pads: HashMap<LayerId, Vec<ItemId>>,
    ref_texts: HashSet<ItemId>,
    value_texts: HashSet<ItemId>,
    pads_by_group: HashMap<GroupId, Vec<ItemId>>,
    bounds_by_group: HashMap<GroupId, ItemId>,
}

fn has_prop(props: &[Prop], key: &str, value: &str) -> bool {
    props.iter().any(|p| p.key == key && p.value == value)
}

impl Board {
    pub fn new(scene: Scene) -> Self {
        let layers: HashMap<String, LayerId> = scene
            .layers
            .iter()
            .map(|l| (l.name.clone(), l.id))
            .collect();
        let mut inner: Vec<String> = scene
            .layers
            .iter()
            .filter(|l| l.side == vector_view::Side::Inner)
            .filter_map(|l| {
                [ZONES, TRACKS, COPPER_PADS]
                    .iter()
                    .find_map(|s| l.name.strip_suffix(&format!(".{s}")))
                    .map(str::to_string)
            })
            .collect();
        inner.sort();
        inner.dedup();
        let bounds_layers: HashSet<LayerId> = ["F", "B"]
            .iter()
            .filter_map(|s| layers.get(&layer_name(s, BOUNDS)).copied())
            .collect();
        let mut board = Board {
            index: HitIndex::new(&scene),
            layers,
            inner,
            pin1_pads: HashMap::new(),
            ref_texts: HashSet::new(),
            value_texts: HashSet::new(),
            pads_by_group: HashMap::new(),
            bounds_by_group: HashMap::new(),
            scene: Scene::new(vector_view::SceneKind::Pcb, true),
        };
        for item in &scene.items {
            match item.role {
                Role::Pad => {
                    if has_prop(&item.props, "pin1", "1") {
                        board.pin1_pads.entry(item.layer).or_default().push(item.id);
                    }
                    if let Some(g) = item.group {
                        board.pads_by_group.entry(g).or_default().push(item.id);
                    }
                }
                Role::Text => {
                    if has_prop(&item.props, "kind", "ref") {
                        board.ref_texts.insert(item.id);
                    } else if has_prop(&item.props, "kind", "value") {
                        board.value_texts.insert(item.id);
                    }
                }
                _ => {
                    if let (Some(g), true) = (item.group, bounds_layers.contains(&item.layer)) {
                        board.bounds_by_group.insert(g, item.id);
                    }
                }
            }
        }
        board.scene = scene;
        board
    }

    fn layer(&self, side: &str, suffix: &str) -> Option<LayerId> {
        self.layers.get(&layer_name(side, suffix)).copied()
    }

    fn named(&self, name: &str) -> Option<LayerId> {
        self.layers.get(name).copied()
    }

    pub fn net_name(&self, id: NetId) -> Option<&str> {
        self.scene.net(id).map(|n| n.name.as_str())
    }

    pub fn net_id(&self, name: &str) -> Option<NetId> {
        self.scene
            .nets
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.id)
    }
}

/// The fitted board transform plus the user's pan/zoom on top of it.
#[derive(Clone, Copy)]
pub struct Viewport {
    pub fit: View,
    /// Screen-space pan/zoom (rotation and mirror unused), driven by input.
    pub user: View,
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            fit: View::default(),
            user: View::default(),
            width: 1.0,
            height: 1.0,
            dpr: 1.0,
        }
    }
}

impl Viewport {
    /// `screen = user(fit(p))`.
    pub fn view(&self) -> View {
        View {
            scale: self.user.scale * self.fit.scale,
            tx: self.user.scale * self.fit.tx + self.user.tx,
            ty: self.user.scale * self.fit.ty + self.user.ty,
            ..self.fit
        }
    }

    pub fn rotation(settings: &Settings, flipped: bool) -> f64 {
        let back = if flipped && settings.offset_back_rotation {
            -180.0
        } else {
            0.0
        };
        deg2rad(settings.board_rotation + back)
    }

    /// Refit the board into a `width` x `height` (CSS px) viewport,
    /// keeping the user's pan/zoom.
    pub fn refit(
        &mut self,
        board: &Board,
        width: f64,
        height: f64,
        settings: &Settings,
        flipped: bool,
    ) {
        self.width = width;
        self.height = height;
        self.fit = View {
            rotation: Self::rotation(settings, flipped),
            mirrored: flipped,
            ..View::default()
        };
        self.fit.fit(&board.scene.bbox, width, height, 0.0);
        self.fit.zoom_at(width / 2.0, height / 2.0, 0.98);
    }

    /// Adjust the user pan so the board point at the viewport centre stays
    /// put when the view is mirrored.
    pub fn flip_pan(&mut self) {
        self.user.tx = self.width - self.user.scale * self.width - self.user.tx;
    }

    pub fn reset_user(&mut self) {
        self.user = View::default();
    }
}

pub struct Canvases {
    pub bg: HtmlCanvasElement,
    pub fab: HtmlCanvasElement,
    pub silk: HtmlCanvasElement,
    pub highlight: HtmlCanvasElement,
}

impl Canvases {
    fn all(&self) -> [&HtmlCanvasElement; 4] {
        [&self.bg, &self.fab, &self.silk, &self.highlight]
    }

    /// Size the backing stores to `width` x `height` CSS px at `dpr`.
    pub fn resize(&self, width: f64, height: f64, dpr: f64) {
        for canvas in self.all() {
            canvas.set_width((width * dpr) as u32);
            canvas.set_height((height * dpr) as u32);
            let _ = canvas.style().set_property("width", &format!("{width}px"));
            let _ = canvas
                .style()
                .set_property("height", &format!("{height}px"));
        }
    }
}

fn get_ctx(canvas: &HtmlCanvasElement) -> CanvasRenderingContext2d {
    canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into::<CanvasRenderingContext2d>()
        .unwrap()
}

fn clear(canvas: &HtmlCanvasElement) {
    vr::clear(
        &get_ctx(canvas),
        canvas.width() as f64,
        canvas.height() as f64,
        None,
    );
}

/// Everything that decides what a frame looks like.
pub struct Frame<'a> {
    pub board: &'a Board,
    pub colors: &'a Colors,
    pub settings: &'a Settings,
    /// Side being viewed: "F" or "B".
    pub side: &'a str,
    pub highlighted_footprints: &'a [usize],
    pub marked_footprints: &'a HashSet<usize>,
    pub highlighted_net: Option<NetId>,
    pub dnp: &'a HashSet<usize>,
}

/// One pass: an ordered list of (layer, alpha) plus colour overrides.
struct Pass {
    layers: Vec<(LayerId, f64)>,
    colors: HashMap<LayerId, Rgba>,
    fills: HashMap<LayerId, Rgba>,
}

impl Pass {
    fn new() -> Self {
        Self {
            layers: Vec::new(),
            colors: HashMap::new(),
            fills: HashMap::new(),
        }
    }

    fn add(&mut self, layer: Option<LayerId>, alpha: f64, color: Rgba) {
        if let Some(id) = layer {
            self.layers.push((id, alpha));
            self.colors.insert(id, color);
        }
    }
}

impl Frame<'_> {
    fn opposite(&self) -> &'static str {
        if self.side == "F" {
            "B"
        } else {
            "F"
        }
    }

    fn base_state(&self, vp: &Viewport) -> ViewState {
        let mut st = ViewState::new(vp.view());
        st.dpr = vp.dpr;
        // One device pixel, like the classic renderer's `1 / scalefactor`.
        st.min_line_px = 1.0 / vp.dpr;
        st.theme.hole = Some(self.colors.pad_hole);
        let s = self.settings;
        if !s.render_references {
            st.hidden_items.extend(self.board.ref_texts.iter().copied());
        }
        if !s.render_values {
            st.hidden_items
                .extend(self.board.value_texts.iter().copied());
        }
        if s.render_dnp_outline {
            for g in self.dnp {
                if let Some(pads) = self.board.pads_by_group.get(&(*g as GroupId)) {
                    st.outline_items.extend(pads.iter().copied());
                }
            }
        }
        st
    }

    fn apply(&self, st: &mut ViewState, pass: Pass) {
        st.layer_order = Some(pass.layers.iter().map(|(l, _)| *l).collect());
        for (l, a) in &pass.layers {
            st.visibility.set(*l, true);
            st.layer_alpha.insert(*l, *a);
        }
        st.theme.layer_colors = pass.colors;
        st.theme.fill_colors = pass.fills;
    }

    fn inner_visible(&self) -> impl Iterator<Item = &String> {
        self.board
            .inner
            .iter()
            .filter(|n| !self.settings.hidden_layers.contains(n.as_str()))
    }

    /// Copper and pads for one side, as `draw_nets` + pads + footprints did.
    fn copper(
        &self,
        pass: &mut Pass,
        side: &str,
        alpha: f64,
        inner: bool,
        net_colors: (Rgba, Rgba),
    ) {
        let b = self.board;
        let s = self.settings;
        let (track_c, zone_c) = net_colors;
        if s.render_zones {
            pass.add(b.layer(side, ZONES), alpha, zone_c);
            if inner {
                for n in self.inner_visible() {
                    pass.add(b.layer(n, ZONES), alpha * 0.25, zone_c);
                }
            }
        }
        if s.render_tracks {
            pass.add(b.layer(side, TRACKS), alpha, track_c);
            if inner {
                for n in self.inner_visible() {
                    pass.add(b.layer(n, TRACKS), alpha * 0.25, track_c);
                }
            }
        }
    }

    fn background_pass(&self) -> Pass {
        let b = self.board;
        let s = self.settings;
        let c = self.colors;
        let (side, opp) = (self.side, self.opposite());
        let mut pass = Pass::new();

        // See-through opposite side.
        self.copper(&mut pass, opp, 0.35, false, (c.track(opp), c.zone(opp)));
        if s.render_tracks {
            pass.add(b.layer(opp, COPPER_PADS), 0.35, c.track(opp));
        }
        pass.add(b.layer(opp, FOOTPRINTS), 0.35, c.pad);
        if s.render_pads {
            pass.add(b.layer(opp, PADS), 0.35, c.pad);
        }

        // Viewed side, inner layers dimmed in its colours.
        self.copper(&mut pass, side, 1.0, true, (c.track(side), c.zone(side)));
        if s.render_tracks {
            pass.add(b.layer(side, COPPER_PADS), 1.0, c.track(side));
            for n in self.inner_visible() {
                pass.add(b.layer(n, COPPER_PADS), 0.25, c.track(side));
            }
        }
        pass.add(b.layer(side, FOOTPRINTS), 1.0, c.pad);
        if s.render_pads {
            pass.add(b.layer(side, PADS), 1.0, c.pad);
            pass.add(b.named(HOLES), 1.0, c.pad_hole);
        }
        if s.render_edge_cuts {
            pass.add(b.named(EDGE_CUTS), 1.0, c.pcb_edge);
        }
        // Drill hits punch through so the page shows (see `erase_layers`).
        pass.add(b.named(DRILLS), 1.0, c.pad_hole);
        pass
    }

    fn pin1_state(&self, vp: &Viewport, pads: Vec<ItemId>, color: Rgba) -> Option<ViewState> {
        let layer = self.board.layer(self.side, PADS)?;
        if pads.is_empty() {
            return None;
        }
        let mut st = self.base_state(vp);
        let mut pass = Pass::new();
        pass.add(Some(layer), 1.0, color);
        self.apply(&mut st, pass);
        st.outline_items = pads.iter().copied().collect();
        st.highlight = Highlight::Items(st.outline_items.clone());
        st.highlight_style = HighlightStyle::Only;
        st.theme.highlight = Some(color);
        Some(st)
    }

    fn side_pin1(&self) -> Vec<ItemId> {
        self.board
            .layer(self.side, PADS)
            .and_then(|l| self.board.pin1_pads.get(&l))
            .cloned()
            .unwrap_or_default()
    }

    /// Draw background, fabrication and silkscreen canvases.
    pub fn draw_background(&self, canvases: &Canvases, vp: &Viewport, cache: &mut PathCache) {
        let scene = &self.board.scene;
        let b = self.board;
        let s = self.settings;
        let c = self.colors;
        for canvas in [&canvases.bg, &canvases.fab, &canvases.silk] {
            clear(canvas);
        }

        let mut st = self.base_state(vp);
        self.apply(&mut st, self.background_pass());
        if let Some(d) = b.named(DRILLS) {
            st.erase_layers.insert(d);
        }
        let bg = get_ctx(&canvases.bg);
        vr::draw_cached(scene, &bg, &st, cache);
        if s.render_pads && s.highlight_pin1 == "all" {
            if let Some(st) = self.pin1_state(vp, self.side_pin1(), c.pin1_outline) {
                vr::draw_cached(scene, &bg, &st, cache);
            }
        }

        if s.render_fabrication {
            let mut st = self.base_state(vp);
            let mut pass = Pass::new();
            let l = b.layer(self.side, FAB);
            pass.add(l, 1.0, c.fab_edge);
            if let Some(l) = l {
                pass.fills.insert(l, c.fab_polygon);
            }
            self.apply(&mut st, pass);
            vr::draw_cached(scene, &get_ctx(&canvases.fab), &st, cache);
        }
        if s.render_silkscreen {
            let mut st = self.base_state(vp);
            let mut pass = Pass::new();
            let l = b.layer(self.side, SILKSCREEN);
            pass.add(l, 1.0, c.silk_edge);
            if let Some(l) = l {
                pass.fills.insert(l, c.silk_polygon);
            }
            let clear_layer = b.layer(self.side, SILKSCREEN_CLEAR);
            pass.add(clear_layer, 1.0, c.silk_edge);
            self.apply(&mut st, pass);
            if let Some(l) = clear_layer {
                st.erase_layers.insert(l);
            }
            vr::draw_cached(scene, &get_ctx(&canvases.silk), &st, cache);
        }
    }

    /// Draw the highlight overlay canvas.
    pub fn draw_highlights(&self, canvases: &Canvases, vp: &Viewport, cache: &mut PathCache) {
        let scene = &self.board.scene;
        let b = self.board;
        let s = self.settings;
        let c = self.colors;
        clear(&canvases.highlight);
        let ctx = get_ctx(&canvases.highlight);

        // Footprints: highlighted, marked, or both, each in its own colours.
        let hl: HashSet<usize> = self.highlighted_footprints.iter().copied().collect();
        let mk = self.marked_footprints;
        let categories = [
            (
                hl.intersection(mk).copied().collect::<Vec<_>>(),
                c.pad_highlight_both,
                c.pin1_outline_highlight_both,
            ),
            (
                hl.difference(mk).copied().collect(),
                c.pad_highlight,
                c.pin1_outline_highlight,
            ),
            (
                mk.difference(&hl).copied().collect(),
                c.pad_highlight_marked,
                c.pin1_outline_highlight_marked,
            ),
        ];
        for (fps, pad_c, pin1_c) in categories {
            if fps.is_empty() {
                continue;
            }
            let groups: HashSet<GroupId> = fps.iter().map(|i| *i as GroupId).collect();
            let mut st = self.base_state(vp);
            let mut pass = Pass::new();
            pass.add(b.layer(self.side, BOUNDS), 0.2, pad_c);
            pass.add(b.layer(self.side, FOOTPRINTS), 1.0, pad_c);
            if s.render_pads {
                pass.add(b.layer(self.side, PADS), 1.0, pad_c);
                pass.add(b.named(HOLES), 1.0, c.pad_hole);
            }
            self.apply(&mut st, pass);
            st.highlight = Highlight::Group(groups.clone());
            st.highlight_style = HighlightStyle::Only;
            st.theme.highlight = Some(pad_c);
            vr::draw_cached(scene, &ctx, &st, cache);

            // Bounding-box outline at full opacity.
            if let Some(l) = b.layer(self.side, BOUNDS) {
                let outlines: HashSet<ItemId> = groups
                    .iter()
                    .filter_map(|g| b.bounds_by_group.get(g).copied())
                    .collect();
                let mut st = self.base_state(vp);
                let mut pass = Pass::new();
                pass.add(Some(l), 1.0, pad_c);
                self.apply(&mut st, pass);
                st.highlight = Highlight::Items(outlines.clone());
                st.highlight_style = HighlightStyle::Only;
                st.outline_items = outlines;
                vr::draw_cached(scene, &ctx, &st, cache);
            }

            if s.render_pads && s.highlight_pin1 == "selected" {
                let pads: Vec<ItemId> = self
                    .side_pin1()
                    .into_iter()
                    .filter(|id| {
                        scene
                            .items
                            .get(*id as usize - 1)
                            .and_then(|i| i.group)
                            .is_some_and(|g| groups.contains(&g))
                    })
                    .collect();
                if let Some(st) = self.pin1_state(vp, pads, pin1_c) {
                    vr::draw_cached(scene, &ctx, &st, cache);
                }
            }
        }

        // Net: zones and tracks on both sides (inner layers with the viewed side).
        if let Some(net) = self.highlighted_net {
            let mut st = self.base_state(vp);
            let mut pass = Pass::new();
            let hlc = (c.track_highlight, c.zone_highlight);
            self.copper(&mut pass, self.opposite(), 1.0, false, hlc);
            self.copper(&mut pass, self.side, 1.0, true, hlc);
            self.apply(&mut st, pass);
            st.highlight = Highlight::Net(net);
            st.highlight_style = HighlightStyle::Only;
            vr::draw_cached(scene, &ctx, &st, cache);
        }
    }
}

/// What a click selects.
pub enum Pick {
    Net(String),
    Footprints(Vec<usize>),
    Nothing,
}

/// iBOM click semantics: a net under the cursor (tracks, then pads) wins;
/// otherwise every footprint whose bounds contain the point, viewed side first.
pub fn pick(
    board: &Board,
    vp: &Viewport,
    x: f64,
    y: f64,
    side: &str,
    settings: &Settings,
    nets: bool,
) -> Pick {
    let p = vp.view().to_world([x, y]);
    let mut pickable: HashSet<LayerId> = HashSet::new();
    let sides: Vec<&str> = ["F", "B"]
        .into_iter()
        .chain(board.inner.iter().map(String::as_str))
        .collect();
    for s in &sides {
        if settings.render_tracks {
            pickable.extend(board.layer(s, TRACKS));
        }
        if settings.render_pads {
            pickable.extend(board.layer(s, PADS));
        }
    }
    let opp = if side == "F" { "B" } else { "F" };
    let bounds = [board.layer(side, BOUNDS), board.layer(opp, BOUNDS)];
    pickable.extend(bounds.iter().flatten());
    let visible = |id: LayerId| pickable.contains(&id);
    let hits = board.index.hits(&board.scene, p, 0.0, &visible);
    let item = |id: ItemId| &board.scene.items[id as usize - 1];

    if nets {
        let net = hits.iter().map(|id| item(*id)).find_map(|i| match i.role {
            Role::Pad | Role::Track | Role::Via => i.net,
            _ => None,
        });
        if let Some(name) = net.and_then(|n| board.net_name(n)) {
            if !name.is_empty() {
                return Pick::Net(name.to_string());
            }
        }
    }
    for layer in bounds.into_iter().flatten() {
        let fps: Vec<usize> = hits
            .iter()
            .map(|id| item(*id))
            .filter(|i| i.layer == layer)
            .filter_map(|i| i.group.map(|g| g as usize))
            .collect();
        if !fps.is_empty() {
            let mut fps = fps;
            fps.sort_unstable();
            fps.dedup();
            return Pick::Footprints(fps);
        }
    }
    Pick::Nothing
}
