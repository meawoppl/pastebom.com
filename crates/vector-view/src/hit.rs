//! Hit testing: which item is under a scene-space point.
//!
//! [`hit_test`] scans linearly; [`HitIndex`] buckets item bounds in a uniform
//! grid so large boards stay fast. Both resolve overlaps by role priority
//! (pad > via > track > text > others), then by the topmost layer, then by
//! drawing order.

use crate::scene::{BBox, Item, ItemId, LayerId, Point, Prim, Role, Scene};
use crate::style::{PickPlan, ViewState};
use std::f64::consts::TAU;

/// Higher wins when several items are under the cursor.
pub fn role_priority(role: Role) -> u8 {
    match role {
        Role::Pad | Role::Pin => 6,
        Role::Via => 5,
        Role::Track | Role::Wire | Role::Bus | Role::Junction | Role::NoConnect => 4,
        Role::Text | Role::Label | Role::Field => 3,
        Role::Hole => 2,
        Role::Zone => 0,
        _ => 1,
    }
}

fn dist2_point_segment(p: Point, a: Point, b: Point) -> f64 {
    let (cx, cy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = cx * cx + cy * cy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * cx + (p[1] - a[1]) * cy) / len2).clamp(0.0, 1.0)
    };
    let (dx, dy) = (p[0] - (a[0] + t * cx), p[1] - (a[1] + t * cy));
    dx * dx + dy * dy
}

fn near_path(points: &[Point], p: Point, r: f64, closed: bool) -> bool {
    let r2 = r * r;
    match points.len() {
        0 => false,
        1 => dist2_point_segment(p, points[0], points[0]) <= r2,
        n => {
            let open = points
                .windows(2)
                .any(|w| dist2_point_segment(p, w[0], w[1]) <= r2);
            open || (closed && dist2_point_segment(p, points[n - 1], points[0]) <= r2)
        }
    }
}

/// Even-odd crossing test for one ring.
fn ring_crossings(ring: &[Point], p: Point) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Point-in-polygon with holes under the even-odd rule (as the renderer fills).
pub fn polygon_contains(outer: &[Point], holes: &[Vec<Point>], p: Point) -> bool {
    let mut inside = ring_crossings(outer, p);
    for h in holes {
        if ring_crossings(h, p) {
            inside = !inside;
        }
    }
    inside
}

/// Is `angle` within the counter-clockwise sweep `start -> end`, with
/// angular slack `tol`?
fn in_sweep(angle: f64, start: f64, end: f64, tol: f64) -> bool {
    let sweep = (end - start).rem_euclid(TAU);
    if sweep == 0.0 && (end - start).abs() >= TAU - 1e-12 {
        return true;
    }
    let off = (angle - start).rem_euclid(TAU);
    off <= sweep + tol || off >= TAU - tol
}

/// Does `prim`, as drawn, cover `p` (within `tol` scene units)?
pub fn prim_contains(prim: &Prim, p: Point, tol: f64) -> bool {
    match prim {
        Prim::Polyline { points, width } => near_path(points, p, width / 2.0 + tol, false),
        Prim::Polygon {
            outer,
            holes,
            fill,
            stroke,
        } => {
            let edge = stroke / 2.0 + tol;
            if *fill && polygon_contains(outer, holes, p) {
                return true;
            }
            near_path(outer, p, edge, true) || holes.iter().any(|h| near_path(h, p, edge, true))
        }
        Prim::Circle {
            center,
            radius,
            fill,
            stroke,
        } => {
            let d = ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt();
            if *fill {
                d <= radius + stroke / 2.0 + tol
            } else {
                (d - radius).abs() <= stroke / 2.0 + tol
            }
        }
        Prim::Arc {
            center,
            radius,
            start,
            end,
            width,
        } => {
            let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
            let d = (dx * dx + dy * dy).sqrt();
            let half = width / 2.0 + tol;
            if (d - radius).abs() > half {
                return false;
            }
            let slack = if *radius > 0.0 { half / radius } else { TAU };
            in_sweep(dy.atan2(dx), *start, *end, slack)
        }
        Prim::Strokes { strokes, width } => strokes
            .iter()
            .any(|s| near_path(s, p, width / 2.0 + tol, false)),
        Prim::Hole {
            center,
            size,
            rotation,
        } => {
            // Oblong: a segment along the long axis, swept by half the short side.
            let (s, c) = (-rotation).sin_cos();
            let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
            let local = [dx * c - dy * s, dx * s + dy * c];
            let (w, h) = (size[0] / 2.0, size[1] / 2.0);
            let (a, b, r) = if w >= h {
                ([-(w - h), 0.0], [w - h, 0.0], h)
            } else {
                ([0.0, -(h - w)], [0.0, h - w], w)
            };
            dist2_point_segment(local, a, b) <= (r + tol).powi(2)
        }
    }
}

/// Sort key for a candidate: role priority, then layer z, then draw order.
fn rank(scene: &Scene, idx: usize) -> (u8, i32, usize) {
    let item = &scene.items[idx];
    let z = scene.layer(item.layer).map_or(0, |l| l.z);
    (role_priority(item.role), z, idx)
}

fn candidate(
    scene: &Scene,
    idx: usize,
    p: Point,
    tol: f64,
    visible: &dyn Fn(LayerId) -> bool,
) -> bool {
    let item: &Item = &scene.items[idx];
    visible(item.layer) && prim_contains(&item.prim, p, tol)
}

/// Linear-scan hit test. `visible` decides which layers are pickable.
pub fn hit_test(
    scene: &Scene,
    p: Point,
    tol: f64,
    visible: &dyn Fn(LayerId) -> bool,
) -> Option<ItemId> {
    (0..scene.items.len())
        .filter(|&i| candidate(scene, i, p, tol, visible))
        .max_by_key(|&i| rank(scene, i))
        .map(|i| scene.items[i].id)
}

/// Rank for view-aware picking: role priority, then topmost paint-plan entry,
/// then draw order. `None` when the item is not drawn.
fn view_rank(scene: &Scene, plan: &PickPlan, idx: usize) -> Option<(u8, usize, usize)> {
    let item = &scene.items[idx];
    plan.rank(item)
        .map(|pass| (role_priority(item.role), pass, idx))
}

fn best_view(
    scene: &Scene,
    plan: &PickPlan,
    candidates: impl Iterator<Item = usize>,
    p: Point,
    tol: f64,
) -> Vec<(u8, usize, usize)> {
    let mut ranked: Vec<(u8, usize, usize)> = candidates
        .filter_map(|i| {
            let r = view_rank(scene, plan, i)?;
            prim_contains(&scene.items[i].prim, p, tol).then_some(r)
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    ranked
}

/// Hit test honouring a [`ViewState`]: invisible layers, hidden items and
/// role-filtered passes are not pickable, and overlaps resolve by role
/// priority, then the topmost pass (or layer in paint order).
pub fn hit_test_view(scene: &Scene, state: &ViewState, p: Point, tol: f64) -> Option<ItemId> {
    let plan = state.pick_plan(scene);
    best_view(scene, &plan, 0..scene.items.len(), p, tol)
        .first()
        .map(|r| scene.items[r.2].id)
}

/// Uniform-grid spatial index over item bounds.
#[derive(Debug, Clone)]
pub struct HitIndex {
    origin: Point,
    cell: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    /// Items too large to bucket (board-sized zones): always checked.
    large: Vec<u32>,
    bounds: Vec<BBox>,
}

/// Items spanning more cells than this go to the always-checked list.
const MAX_CELLS_PER_ITEM: usize = 256;

impl HitIndex {
    pub fn new(scene: &Scene) -> Self {
        let bounds: Vec<BBox> = scene.items.iter().map(|i| i.prim.bbox()).collect();
        let mut all = BBox::EMPTY;
        for b in &bounds {
            all.union(b);
        }
        let n = scene.items.len().max(1);
        // Aim for a handful of items per cell.
        let side = ((n as f64 / 4.0).sqrt().ceil() as usize).clamp(1, 1024);
        let (w, h) = (all.width().max(1e-9), all.height().max(1e-9));
        let cell = (w.max(h) / side as f64).max(1e-9);
        let cols = ((w / cell).ceil() as usize).max(1);
        let rows = ((h / cell).ceil() as usize).max(1);
        let origin = if all.is_empty() { [0.0, 0.0] } else { all.min };
        let mut index = HitIndex {
            origin,
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
            large: Vec::new(),
            bounds,
        };
        for i in 0..index.bounds.len() {
            let b = index.bounds[i];
            if b.is_empty() {
                continue;
            }
            let (c0, r0) = index.cell_of(b.min);
            let (c1, r1) = index.cell_of(b.max);
            if (c1 - c0 + 1) * (r1 - r0 + 1) > MAX_CELLS_PER_ITEM {
                index.large.push(i as u32);
                continue;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    index.cells[r * cols + c].push(i as u32);
                }
            }
        }
        index
    }

    fn cell_of(&self, p: Point) -> (usize, usize) {
        let c = ((p[0] - self.origin[0]) / self.cell).floor();
        let r = ((p[1] - self.origin[1]) / self.cell).floor();
        (
            (c.max(0.0) as usize).min(self.cols - 1),
            (r.max(0.0) as usize).min(self.rows - 1),
        )
    }

    /// Indices (into `scene.items`) whose padded bounds contain `p`.
    pub fn candidates(&self, p: Point, tol: f64) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let (c0, r0) = self.cell_of([p[0] - tol, p[1] - tol]);
        let (c1, r1) = self.cell_of([p[0] + tol, p[1] + tol]);
        for r in r0..=r1 {
            for c in c0..=c1 {
                out.extend(self.cells[r * self.cols + c].iter().map(|&i| i as usize));
            }
        }
        out.extend(self.large.iter().map(|&i| i as usize));
        out.sort_unstable();
        out.dedup();
        out.retain(|&i| {
            let b = &self.bounds[i];
            p[0] >= b.min[0] - tol
                && p[0] <= b.max[0] + tol
                && p[1] >= b.min[1] - tol
                && p[1] <= b.max[1] + tol
        });
        out
    }

    /// All items under `p`, best first.
    pub fn hits(
        &self,
        scene: &Scene,
        p: Point,
        tol: f64,
        visible: &dyn Fn(LayerId) -> bool,
    ) -> Vec<ItemId> {
        let mut c: Vec<usize> = self
            .candidates(p, tol)
            .into_iter()
            .filter(|&i| candidate(scene, i, p, tol, visible))
            .collect();
        c.sort_by_key(|&i| std::cmp::Reverse(rank(scene, i)));
        c.into_iter().map(|i| scene.items[i].id).collect()
    }

    /// Same result as [`hit_test_view`], via the grid.
    pub fn hit_test_view(
        &self,
        scene: &Scene,
        state: &ViewState,
        p: Point,
        tol: f64,
    ) -> Option<ItemId> {
        self.hits_view(scene, state, p, tol).into_iter().next()
    }

    /// All drawn items under `p` for `state`, best first.
    pub fn hits_view(&self, scene: &Scene, state: &ViewState, p: Point, tol: f64) -> Vec<ItemId> {
        let plan = state.pick_plan(scene);
        best_view(scene, &plan, self.candidates(p, tol).into_iter(), p, tol)
            .into_iter()
            .map(|r| scene.items[r.2].id)
            .collect()
    }

    /// Same result as [`hit_test`], via the grid.
    pub fn hit_test(
        &self,
        scene: &Scene,
        p: Point,
        tol: f64,
        visible: &dyn Fn(LayerId) -> bool,
    ) -> Option<ItemId> {
        self.candidates(p, tol)
            .into_iter()
            .filter(|&i| candidate(scene, i, p, tol, visible))
            .max_by_key(|&i| rank(scene, i))
            .map(|i| scene.items[i].id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::*;

    fn layer(id: LayerId, z: i32) -> Layer {
        Layer {
            id,
            name: format!("L{id}"),
            kind: LayerKind::Copper,
            side: Side::Front,
            z,
            color: [255, 0, 0, 255],
            visible: true,
        }
    }

    fn item(id: ItemId, layer: LayerId, role: Role, prim: Prim) -> Item {
        Item {
            id,
            layer,
            role,
            prim,
            net: None,
            group: None,
            props: vec![],
        }
    }

    fn scene() -> Scene {
        let mut s = Scene::new(SceneKind::Pcb, true);
        s.layers = vec![layer(0, 0), layer(1, 5)];
        s.items = vec![
            item(
                10,
                0,
                Role::Zone,
                Prim::Polygon {
                    outer: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
                    holes: vec![vec![[8.0, 8.0], [12.0, 8.0], [12.0, 12.0], [8.0, 12.0]]],
                    fill: true,
                    stroke: 0.0,
                },
            ),
            item(
                11,
                0,
                Role::Track,
                Prim::Polyline {
                    points: vec![[0.0, 5.0], [20.0, 5.0]],
                    width: 0.5,
                },
            ),
            item(
                12,
                1,
                Role::Pad,
                Prim::Circle {
                    center: [15.0, 5.0],
                    radius: 1.0,
                    fill: true,
                    stroke: 0.0,
                },
            ),
            item(
                13,
                1,
                Role::Graphic,
                Prim::Arc {
                    center: [30.0, 0.0],
                    radius: 5.0,
                    start: 0.0,
                    end: std::f64::consts::FRAC_PI_2,
                    width: 0.2,
                },
            ),
            item(
                14,
                1,
                Role::Hole,
                Prim::Hole {
                    center: [40.0, 0.0],
                    size: [4.0, 1.0],
                    rotation: std::f64::consts::FRAC_PI_2,
                },
            ),
            item(
                15,
                1,
                Role::Text,
                Prim::Strokes {
                    strokes: vec![vec![[50.0, 0.0], [52.0, 0.0]]],
                    width: 0.1,
                },
            ),
        ];
        s.recompute_bbox();
        s
    }

    fn all(_: LayerId) -> bool {
        true
    }

    #[test]
    fn priority_pad_over_track_over_zone() {
        let s = scene();
        assert_eq!(hit_test(&s, [15.0, 5.0], 0.0, &all), Some(12));
        assert_eq!(hit_test(&s, [3.0, 5.1], 0.0, &all), Some(11));
        assert_eq!(hit_test(&s, [3.0, 15.0], 0.0, &all), Some(10));
        assert!(role_priority(Role::Via) > role_priority(Role::Track));
        assert!(role_priority(Role::Track) > role_priority(Role::Text));
        assert!(role_priority(Role::Text) > role_priority(Role::Graphic));
    }

    #[test]
    fn polygon_holes_are_empty() {
        let s = scene();
        assert_eq!(hit_test(&s, [10.0, 10.0], 0.0, &all), None);
    }

    #[test]
    fn hidden_layers_are_not_pickable() {
        let s = scene();
        let only0 = |l: LayerId| l == 0;
        assert_eq!(hit_test(&s, [15.0, 5.0], 0.0, &only0), Some(11));
    }

    #[test]
    fn arc_respects_sweep() {
        let s = scene();
        let a = std::f64::consts::FRAC_PI_4;
        assert_eq!(
            hit_test(&s, [30.0 + 5.0 * a.cos(), 5.0 * a.sin()], 0.0, &all),
            Some(13)
        );
        // Opposite quadrant is not part of the arc.
        assert_eq!(hit_test(&s, [25.0, -0.5], 0.0, &all), None);
    }

    #[test]
    fn rotated_hole_and_strokes_with_tolerance() {
        let s = scene();
        // Rotated 90deg: long axis is vertical.
        assert_eq!(hit_test(&s, [40.0, 1.8], 0.0, &all), Some(14));
        assert_eq!(hit_test(&s, [41.8, 0.0], 0.0, &all), None);
        assert_eq!(hit_test(&s, [51.0, 0.3], 0.0, &all), None);
        assert_eq!(hit_test(&s, [51.0, 0.3], 0.3, &all), Some(15));
    }

    #[test]
    fn prim_bboxes_cover_every_hit() {
        let s = scene();
        for item in &s.items {
            let b = item.prim.bbox();
            assert!(!b.is_empty());
            for i in 0..2000 {
                let p = [(i % 50) as f64 * 1.2 - 2.0, (i / 50) as f64 * 0.6 - 4.0];
                if prim_contains(&item.prim, p, 0.0) {
                    assert!(b.contains(p), "item {} point {p:?} outside {b:?}", item.id);
                }
            }
        }
        let mut u = BBox::EMPTY;
        assert!(u.is_empty() && u.width() == 0.0);
        u.union(&BBox::EMPTY);
        assert!(u.is_empty());
        u.union(&s.items[2].prim.bbox());
        assert_eq!((u.min, u.max), ([14.0, 4.0], [16.0, 6.0]));
        assert_eq!(s.bbox.min, [-0.25, -5.1]);
        let hole = Prim::Hole {
            center: [0.0, 0.0],
            size: [4.0, 1.0],
            rotation: 1.0,
        };
        assert_eq!(hole.bbox().max, [2.0, 2.0]);
    }

    #[test]
    fn view_hit_respects_visibility_hidden_and_passes() {
        use crate::style::{Pass, ViewState};
        use crate::view::View;
        let s = scene();
        let idx = HitIndex::new(&s);
        let mut st = ViewState::new(View::default());
        let pad = [15.0, 5.0];
        assert_eq!(hit_test_view(&s, &st, pad, 0.0), Some(12));
        // Hidden items fall through to what is underneath.
        st.hidden_items.insert(12);
        assert_eq!(hit_test_view(&s, &st, pad, 0.0), Some(11));
        assert_eq!(idx.hit_test_view(&s, &st, pad, 0.0), Some(11));
        st.hidden_items.clear();
        // Invisible layers are not pickable.
        st.visibility.set(1, false);
        assert_eq!(hit_test_view(&s, &st, pad, 0.0), Some(11));
        st.visibility.set(0, false);
        assert_eq!(hit_test_view(&s, &st, pad, 0.0), None);
        st.visibility.overrides.clear();

        // Passes: only listed layers/roles are pickable.
        st.passes = Some(vec![Pass::roles(0, &[Role::Zone], 0.5), Pass::new(1, 1.0)]);
        assert_eq!(hit_test_view(&s, &st, [3.0, 5.0], 0.0), Some(10));
        assert_eq!(idx.hits_view(&s, &st, pad, 0.0), vec![12, 10]);

        // Same role on two layers: the topmost pass wins, regardless of z.
        let mut s2 = s.clone();
        s2.items[1].layer = 1; // the track now on layer 1 too
        s2.items.push(item(
            99,
            0,
            Role::Track,
            Prim::Polyline {
                points: vec![[0.0, 5.0], [20.0, 5.0]],
                width: 0.5,
            },
        ));
        let at = [3.0, 5.0];
        st.passes = Some(vec![Pass::new(1, 1.0), Pass::new(0, 1.0)]);
        assert_eq!(hit_test_view(&s2, &st, at, 0.0), Some(99));
        st.passes = Some(vec![Pass::new(0, 1.0), Pass::new(1, 1.0)]);
        assert_eq!(hit_test_view(&s2, &st, at, 0.0), Some(11));
        // Without passes the paint order (by z) decides: layer 1 is on top.
        st.passes = None;
        assert_eq!(hit_test_view(&s2, &st, at, 0.0), Some(11));
        st.layer_order = Some(vec![1, 0]);
        assert_eq!(hit_test_view(&s2, &st, at, 0.0), Some(99));
    }

    #[test]
    fn index_matches_linear_scan() {
        let mut s = scene();
        // Pad the scene with a grid of small pads to exercise bucketing.
        for i in 0..400u32 {
            let (x, y) = ((i % 20) as f64 * 3.0, 30.0 + (i / 20) as f64 * 3.0);
            s.items.push(item(
                100 + i,
                1,
                Role::Pad,
                Prim::Polygon {
                    outer: vec![[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]],
                    holes: vec![],
                    fill: true,
                    stroke: 0.0,
                },
            ));
        }
        let idx = HitIndex::new(&s);
        for i in 0..200 {
            let p = [(i as f64 * 0.37) % 60.0, (i as f64 * 1.13) % 90.0 - 5.0];
            assert_eq!(
                idx.hit_test(&s, p, 0.1, &all),
                hit_test(&s, p, 0.1, &all),
                "{p:?}"
            );
        }
        let hits = idx.hits(&s, [15.0, 5.0], 0.0, &all);
        assert_eq!(hits, vec![12, 11, 10]);
    }
}
