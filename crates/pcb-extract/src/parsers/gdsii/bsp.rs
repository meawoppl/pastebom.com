//! GDSII tile pipeline — stage 2: BSP spatial index.
//!
//! A k-d-style binary space partition over [`PlacedRecord`] extents, used to
//! answer per-tile AABB range queries in `O(log n + k)` instead of scanning
//! every record. Splits alternate X/Y at the median of record centroids.
//!
//! It is a **loose** BSP: a record whose extent straddles a cut plane is
//! referenced from *both* children, so a range query that prunes by the cut
//! can never miss a straddler. Queries dedup via a visited set.

use serde::{Deserialize, Serialize};

use super::tile::{PlacedRecord, WorldBox};

/// Records per leaf before we stop splitting.
const LEAF_CAP: usize = 64;
/// Maximum tree depth (bounds recursion and serialized size).
const MAX_DEPTH: usize = 24;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BspNode {
    Leaf {
        record_ids: Vec<u32>,
    },
    Split {
        axis: Axis,
        cut: i64,
        below: u32,
        above: u32,
    },
}

/// A spatial index over placed records. Owns the records; nodes form an arena.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BspIndex {
    pub bounds: WorldBox,
    root: u32,
    nodes: Vec<BspNode>,
    records: Vec<PlacedRecord>,
}

impl BspIndex {
    /// Build an index over `records`.
    pub fn build(records: Vec<PlacedRecord>) -> Self {
        let mut bounds = WorldBox::empty();
        for r in &records {
            bounds.union(&r.bbox);
        }
        let mut nodes = Vec::new();
        let root = if records.is_empty() {
            0
        } else {
            let ids: Vec<u32> = (0..records.len() as u32).collect();
            build_node(&records, ids, 0, &mut nodes)
        };
        Self {
            bounds,
            root,
            nodes,
            records,
        }
    }

    pub fn records(&self) -> &[PlacedRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Number of arena nodes (for tests / introspection).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Record ids whose extent intersects `q` (deduped, unspecified order).
    pub fn query(&self, q: &WorldBox) -> Vec<u32> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return out;
        }
        let mut visited = vec![false; self.records.len()];
        self.query_node(self.root as usize, q, &mut visited, &mut out);
        out
    }

    /// Records whose extent intersects `q`.
    pub fn query_records(&self, q: &WorldBox) -> Vec<&PlacedRecord> {
        self.query(q)
            .into_iter()
            .map(|id| &self.records[id as usize])
            .collect()
    }

    fn query_node(&self, node: usize, q: &WorldBox, visited: &mut [bool], out: &mut Vec<u32>) {
        match &self.nodes[node] {
            BspNode::Leaf { record_ids } => {
                for &id in record_ids {
                    let i = id as usize;
                    if !visited[i] && self.records[i].bbox.intersects(q) {
                        visited[i] = true;
                        out.push(id);
                    }
                }
            }
            BspNode::Split {
                axis,
                cut,
                below,
                above,
            } => {
                let (qmin, qmax) = match axis {
                    Axis::X => (q.minx, q.maxx),
                    Axis::Y => (q.miny, q.maxy),
                };
                if qmin < *cut {
                    self.query_node(*below as usize, q, visited, out);
                }
                if qmax >= *cut {
                    self.query_node(*above as usize, q, visited, out);
                }
            }
        }
    }

    /// Serialize to a compact binary blob (for `gdsii/{id}/index.bin`).
    pub fn to_bytes(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Deserialize from a blob produced by [`Self::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

/// Build a subtree over `ids`, returning the arena index of its root node.
/// Children are pushed before their parent, so the returned index is the
/// last node pushed for this subtree.
fn build_node(
    records: &[PlacedRecord],
    ids: Vec<u32>,
    depth: usize,
    nodes: &mut Vec<BspNode>,
) -> u32 {
    if ids.len() <= LEAF_CAP || depth >= MAX_DEPTH {
        return push_leaf(nodes, ids);
    }

    let axis = if depth.is_multiple_of(2) {
        Axis::X
    } else {
        Axis::Y
    };

    // Cut at the median of record centroids along the chosen axis.
    let mut centroids: Vec<i64> = ids
        .iter()
        .map(|&id| {
            let b = &records[id as usize].bbox;
            match axis {
                Axis::X => (b.minx + b.maxx) / 2,
                Axis::Y => (b.miny + b.maxy) / 2,
            }
        })
        .collect();
    centroids.sort_unstable();
    let cut = centroids[centroids.len() / 2];

    // Loose partition by extent: a record is in `below` if it starts before the
    // cut and in `above` if it ends at/after the cut — straddlers go to both.
    let mut below = Vec::new();
    let mut above = Vec::new();
    for &id in &ids {
        let b = &records[id as usize].bbox;
        let (lo, hi) = match axis {
            Axis::X => (b.minx, b.maxx),
            Axis::Y => (b.miny, b.maxy),
        };
        if lo < cut {
            below.push(id);
        }
        if hi >= cut {
            above.push(id);
        }
    }

    // Stop if the split makes no progress (a child equals the parent, e.g. all
    // records straddle) — keeps recursion strictly decreasing and terminating.
    if below.len() == ids.len() || above.len() == ids.len() || below.is_empty() || above.is_empty()
    {
        return push_leaf(nodes, ids);
    }

    let below_idx = build_node(records, below, depth + 1, nodes);
    let above_idx = build_node(records, above, depth + 1, nodes);
    let idx = nodes.len() as u32;
    nodes.push(BspNode::Split {
        axis,
        cut,
        below: below_idx,
        above: above_idx,
    });
    idx
}

fn push_leaf(nodes: &mut Vec<BspNode>, ids: Vec<u32>) -> u32 {
    let idx = nodes.len() as u32;
    nodes.push(BspNode::Leaf { record_ids: ids });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::gdsii::tile::{Geom, RecordKind};

    fn rec(minx: i64, miny: i64, maxx: i64, maxy: i64) -> PlacedRecord {
        PlacedRecord {
            layer: 0,
            datatype: 0,
            kind: RecordKind::Boundary,
            bbox: WorldBox {
                minx,
                miny,
                maxx,
                maxy,
            },
            geom: Geom::Poly { rings: vec![] },
        }
    }

    /// A 20x20 grid of unit boxes plus a few wide straddlers.
    fn sample() -> Vec<PlacedRecord> {
        let mut v = Vec::new();
        for gy in 0..20 {
            for gx in 0..20 {
                let x = gx * 10;
                let y = gy * 10;
                v.push(rec(x, y, x + 4, y + 4));
            }
        }
        // straddlers spanning the whole field
        v.push(rec(0, 0, 200, 4));
        v.push(rec(0, 0, 4, 200));
        v
    }

    fn brute_force(records: &[PlacedRecord], q: &WorldBox) -> Vec<u32> {
        let mut ids: Vec<u32> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.bbox.intersects(q))
            .map(|(i, _)| i as u32)
            .collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn each_record_finds_itself() {
        let recs = sample();
        let idx = BspIndex::build(recs.clone());
        for (i, r) in recs.iter().enumerate() {
            let hits = idx.query(&r.bbox);
            assert!(
                hits.contains(&(i as u32)),
                "record {i} not returned by a query of its own bbox"
            );
        }
    }

    #[test]
    fn query_matches_brute_force() {
        let recs = sample();
        let idx = BspIndex::build(recs.clone());
        let queries = [
            WorldBox {
                minx: 0,
                miny: 0,
                maxx: 200,
                maxy: 200,
            }, // everything
            WorldBox {
                minx: 0,
                miny: 0,
                maxx: 5,
                maxy: 5,
            }, // corner
            WorldBox {
                minx: 95,
                miny: 95,
                maxx: 115,
                maxy: 115,
            }, // middle
            WorldBox {
                minx: 1000,
                miny: 1000,
                maxx: 2000,
                maxy: 2000,
            }, // empty
            WorldBox {
                minx: -50,
                miny: 50,
                maxx: 3,
                maxy: 60,
            }, // edge / straddler
        ];
        for q in &queries {
            let mut got = idx.query(q);
            got.sort_unstable();
            assert_eq!(got, brute_force(&recs, q), "mismatch for {q:?}");
        }
    }

    #[test]
    fn empty_index_is_safe() {
        let idx = BspIndex::build(vec![]);
        assert!(idx.is_empty());
        assert!(idx
            .query(&WorldBox {
                minx: 0,
                miny: 0,
                maxx: 10,
                maxy: 10
            })
            .is_empty());
    }

    #[test]
    fn serialize_round_trip_preserves_queries() {
        let idx = BspIndex::build(sample());
        let bytes = idx.to_bytes().unwrap();
        let back = BspIndex::from_bytes(&bytes).unwrap();
        let q = WorldBox {
            minx: 0,
            miny: 0,
            maxx: 60,
            maxy: 60,
        };
        let mut a = idx.query(&q);
        let mut b = back.query(&q);
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        assert_eq!(idx.len(), back.len());
    }

    #[test]
    fn all_straddlers_still_terminates_and_is_correct() {
        // 500 boxes that all span the whole field — the split can't separate
        // them, so the builder must fall back to a leaf without looping.
        let recs: Vec<PlacedRecord> = (0..500).map(|_| rec(0, 0, 1000, 1000)).collect();
        let idx = BspIndex::build(recs.clone());
        let q = WorldBox {
            minx: 10,
            miny: 10,
            maxx: 20,
            maxy: 20,
        };
        let mut got = idx.query(&q);
        got.sort_unstable();
        assert_eq!(got, brute_force(&recs, &q));
    }
}
