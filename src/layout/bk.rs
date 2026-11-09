//! Cross-axis coordinate assignment after Brandes & Köpf, "Fast and Simple
//! Horizontal Coordinate Assignment" (2001), following dagre's `position/bk`
//! (which mermaid uses through dagre-d3-es) step by step:
//!
//! 1. mark type-1 conflicts (a non-inner segment crossing an inner segment
//!    between two dummies) and type-2 conflicts (inner segments crossing
//!    cluster border chains);
//! 2. for each of the four directions (up/down × left/right) align every
//!    vertex with one of its median neighbours into vertical blocks;
//! 3. compact each alignment horizontally (block graph, two sweeps);
//! 4. shift the four results onto the narrowest one and take the average of
//!    the two median candidates per vertex.
//!
//! Aligning a dummy with the node above *or* below it, instead of
//! centring it, is what gives mermaid's edges their characteristic S-curves
//! once they are drawn as basis splines.

use std::collections::{HashMap, HashSet};

/// The layered graph as seen by the coordinate assignment.
pub(super) struct LayerGraph<'a> {
    /// Vertex ids per rank, in left-to-right order.
    pub layers: &'a [Vec<usize>],
    /// Predecessors (one rank up) of each vertex, without duplicates.
    pub preds: &'a [Vec<usize>],
    /// Successors (one rank down) of each vertex, without duplicates.
    pub succs: &'a [Vec<usize>],
    /// Edge dummies and cluster border vertices.
    pub dummy: &'a [bool],
    /// Cluster border vertices only.
    pub border: &'a [Option<BorderSide>],
    /// Extent along the cross axis.
    pub width: &'a [f64],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BorderSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vert {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Horiz {
    Left,
    Right,
}

/// Unordered vertex pairs whose segment must not be used for alignment.
#[derive(Default)]
struct Conflicts(HashSet<(usize, usize)>);

impl Conflicts {
    fn add(&mut self, a: usize, b: usize) {
        self.0.insert((a.min(b), a.max(b)));
    }
    fn has(&self, a: usize, b: usize) -> bool {
        self.0.contains(&(a.min(b), a.max(b)))
    }
}

/// Compute cross-axis centres. `sep(a, b)` is the minimum centre distance
/// between `a` and `b` when `a` is immediately left of `b` in a layer.
pub(super) fn position_x(g: &LayerGraph, sep: &dyn Fn(usize, usize) -> f64) -> Vec<f64> {
    let nv = g.dummy.len();
    let mut order = vec![0usize; nv];
    for layer in g.layers {
        for (i, &v) in layer.iter().enumerate() {
            order[v] = i;
        }
    }
    let mut conflicts = Conflicts::default();
    type1_conflicts(g, &order, &mut conflicts);
    type2_conflicts(g, &order, &mut conflicts);

    let mut xss: Vec<Vec<f64>> = Vec::with_capacity(4);
    for vert in [Vert::Up, Vert::Down] {
        for horiz in [Horiz::Left, Horiz::Right] {
            let mut layering: Vec<Vec<usize>> = g.layers.to_vec();
            if vert == Vert::Down {
                layering.reverse();
            }
            if horiz == Horiz::Right {
                for layer in &mut layering {
                    layer.reverse();
                }
            }
            let neighbours = if vert == Vert::Up { g.preds } else { g.succs };
            let root = vertical_alignment(nv, &layering, &conflicts, neighbours);
            let mut xs = horizontal_compaction(g, &layering, &root, horiz, sep);
            if horiz == Horiz::Right {
                xs.iter_mut().for_each(|x| *x = -*x);
            }
            xss.push(xs);
        }
    }
    let smallest = smallest_width_alignment(g, &xss);
    align_coordinates(&mut xss, smallest);
    balance(&xss, nv)
}

/// dagre's `findOtherInnerSegmentNode`: a dummy predecessor of a dummy.
fn inner_segment_pred(g: &LayerGraph, v: usize) -> Option<usize> {
    if !g.dummy[v] {
        return None;
    }
    g.preds[v].iter().copied().find(|&u| g.dummy[u])
}

fn type1_conflicts(g: &LayerGraph, order: &[usize], out: &mut Conflicts) {
    for pair in g.layers.windows(2) {
        let (prev, layer) = (&pair[0], &pair[1]);
        let Some(&last) = layer.last() else { continue };
        let mut k0 = 0usize;
        let mut scan_pos = 0usize;
        for (i, &v) in layer.iter().enumerate() {
            let w = inner_segment_pred(g, v);
            let k1 = w.map_or(prev.len(), |w| order[w]);
            if w.is_some() || v == last {
                for &scan in &layer[scan_pos..=i] {
                    for &u in &g.preds[scan] {
                        let pos = order[u];
                        if (pos < k0 || k1 < pos) && !(g.dummy[u] && g.dummy[scan]) {
                            out.add(u, scan);
                        }
                    }
                }
                scan_pos = i + 1;
                k0 = k1;
            }
        }
    }
}

fn type2_conflicts(g: &LayerGraph, order: &[usize], out: &mut Conflicts) {
    if g.border.iter().all(Option::is_none) {
        return;
    }
    let scan = |south: &[usize], from: usize, to: usize, lo: i64, hi: i64, out: &mut Conflicts| {
        for &v in &south[from..to] {
            if !g.dummy[v] {
                continue;
            }
            for &u in &g.preds[v] {
                let pos = order[u] as i64;
                if g.dummy[u] && (pos < lo || pos > hi) {
                    out.add(u, v);
                }
            }
        }
    };
    for pair in g.layers.windows(2) {
        let (north, south) = (&pair[0], &pair[1]);
        let (mut prev_north, mut next_north) = (-1i64, -1i64);
        let mut south_pos = 0usize;
        for (lookahead, &v) in south.iter().enumerate() {
            if g.border[v].is_some() {
                if let Some(&first) = g.preds[v].first() {
                    next_north = order[first] as i64;
                    scan(south, south_pos, lookahead, prev_north, next_north, out);
                    south_pos = lookahead;
                    prev_north = next_north;
                }
            }
            // dagre runs this tail scan inside the loop, once per vertex.
            scan(
                south,
                south_pos,
                south.len(),
                next_north,
                north.len() as i64,
                out,
            );
        }
    }
}

/// Returns the block root of every vertex.
fn vertical_alignment(
    nv: usize,
    layering: &[Vec<usize>],
    conflicts: &Conflicts,
    neighbours: &[Vec<usize>],
) -> Vec<usize> {
    let mut root: Vec<usize> = (0..nv).collect();
    let mut align: Vec<usize> = (0..nv).collect();
    let mut pos = vec![0usize; nv];
    for layer in layering {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
    let mut ws: Vec<usize> = Vec::new();
    for layer in layering {
        let mut prev_idx: i64 = -1;
        for &v in layer {
            ws.clear();
            ws.extend_from_slice(&neighbours[v]);
            if ws.is_empty() {
                continue;
            }
            ws.sort_by_key(|&w| pos[w]);
            let mp = (ws.len() - 1) as f64 / 2.0;
            for &w in &ws[mp.floor() as usize..=mp.ceil() as usize] {
                if align[v] == v && prev_idx < pos[w] as i64 && !conflicts.has(v, w) {
                    align[w] = v;
                    root[v] = root[w];
                    align[v] = root[w];
                    prev_idx = pos[w] as i64;
                }
            }
        }
    }
    root
}

fn horizontal_compaction(
    g: &LayerGraph,
    layering: &[Vec<usize>],
    root: &[usize],
    horiz: Horiz,
    sep: &dyn Fn(usize, usize) -> f64,
) -> Vec<f64> {
    let nv = root.len();
    // Block graph over roots: edge (left block → right block) with the
    // largest separation required by any adjacent pair.
    let mut weight: HashMap<(usize, usize), f64> = HashMap::new();
    let mut is_block = vec![false; nv];
    for layer in layering {
        for (i, &v) in layer.iter().enumerate() {
            is_block[root[v]] = true;
            if i == 0 {
                continue;
            }
            let u = layer[i - 1];
            // In a reversed layering `u` sits to the right of `v`.
            let s = match horiz {
                Horiz::Left => sep(u, v),
                Horiz::Right => sep(v, u),
            };
            let e = weight.entry((root[u], root[v])).or_insert(0.0);
            *e = e.max(s);
        }
    }
    let mut out_e: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    let mut in_e: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    let mut edges: Vec<((usize, usize), f64)> = weight.into_iter().collect();
    edges.sort_by_key(|&(k, _)| k);
    for ((a, b), w) in edges {
        out_e[a].push((b, w));
        in_e[b].push((a, w));
    }
    let topo = topological(&is_block, &out_e, &in_e);

    let mut xs = vec![0.0f64; nv];
    for &b in &topo {
        xs[b] = in_e[b]
            .iter()
            .fold(0.0f64, |acc, &(a, w)| acc.max(xs[a] + w));
    }
    // dagre leaves the outer border of a cluster where pass one put it.
    let pinned = match horiz {
        Horiz::Left => BorderSide::Right,
        Horiz::Right => BorderSide::Left,
    };
    for &b in topo.iter().rev() {
        let min = out_e[b]
            .iter()
            .fold(f64::INFINITY, |acc, &(c, w)| acc.min(xs[c] - w));
        if min.is_finite() && g.border[b] != Some(pinned) {
            xs[b] = xs[b].max(min);
        }
    }
    root.iter().map(|&r| xs[r]).collect()
}

/// Kahn's algorithm over the block graph. The graph is acyclic by
/// construction; any leftovers (defensive) are appended in id order.
fn topological(
    is_block: &[bool],
    out_e: &[Vec<(usize, f64)>],
    in_e: &[Vec<(usize, f64)>],
) -> Vec<usize> {
    let n = is_block.len();
    let mut indeg: Vec<usize> = in_e.iter().map(Vec::len).collect();
    let mut queue: Vec<usize> = (0..n)
        .filter(|&b| is_block[b] && indeg[b] == 0)
        .rev()
        .collect();
    let mut done = vec![false; n];
    let mut topo = Vec::with_capacity(n);
    while let Some(b) = queue.pop() {
        done[b] = true;
        topo.push(b);
        for &(c, _) in &out_e[b] {
            indeg[c] -= 1;
            if indeg[c] == 0 {
                queue.push(c);
            }
        }
    }
    topo.extend((0..n).filter(|&b| is_block[b] && !done[b]));
    topo
}

fn smallest_width_alignment(g: &LayerGraph, xss: &[Vec<f64>]) -> usize {
    let mut best = (f64::INFINITY, 0);
    for (i, xs) in xss.iter().enumerate() {
        let (lo, hi) = xs
            .iter()
            .zip(g.width)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (&x, &w)| {
                (lo.min(x - w / 2.0), hi.max(x + w / 2.0))
            });
        if hi - lo < best.0 {
            best = (hi - lo, i);
        }
    }
    best.1
}

/// Left-biased results share the narrowest one's minimum, right-biased ones
/// its maximum. Indices follow the order ul, ur, dl, dr.
fn align_coordinates(xss: &mut [Vec<f64>], to: usize) {
    let min_of = |xs: &[f64]| xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_of = |xs: &[f64]| xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (to_min, to_max) = (min_of(&xss[to]), max_of(&xss[to]));
    for (i, xs) in xss.iter_mut().enumerate() {
        if i == to || xs.is_empty() {
            continue;
        }
        let delta = if i % 2 == 0 {
            to_min - min_of(xs)
        } else {
            to_max - max_of(xs)
        };
        if delta != 0.0 {
            xs.iter_mut().for_each(|x| *x += delta);
        }
    }
}

fn balance(xss: &[Vec<f64>], nv: usize) -> Vec<f64> {
    (0..nv)
        .map(|v| {
            let mut c = [xss[0][v], xss[1][v], xss[2][v], xss[3][v]];
            c.sort_by(f64::total_cmp);
            (c[1] + c[2]) / 2.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        layers: Vec<Vec<usize>>,
        preds: Vec<Vec<usize>>,
        succs: Vec<Vec<usize>>,
        dummy: Vec<bool>,
        border: Vec<Option<BorderSide>>,
        width: Vec<f64>,
    }

    impl Fixture {
        fn new(
            layers: Vec<Vec<usize>>,
            edges: &[(usize, usize)],
            dummies: &[usize],
            widths: &[f64],
        ) -> Self {
            let nv = widths.len();
            let mut preds = vec![Vec::new(); nv];
            let mut succs = vec![Vec::new(); nv];
            for &(a, b) in edges {
                succs[a].push(b);
                preds[b].push(a);
            }
            let mut dummy = vec![false; nv];
            for &d in dummies {
                dummy[d] = true;
            }
            Fixture {
                layers,
                preds,
                succs,
                dummy,
                border: vec![None; nv],
                width: widths.to_vec(),
            }
        }
        fn run(&self) -> Vec<f64> {
            let g = LayerGraph {
                layers: &self.layers,
                preds: &self.preds,
                succs: &self.succs,
                dummy: &self.dummy,
                border: &self.border,
                width: &self.width,
            };
            let w = self.width.clone();
            position_x(&g, &move |a, b| w[a] / 2.0 + w[b] / 2.0 + 50.0)
        }
    }

    #[test]
    fn chain_is_straight() {
        let f = Fixture::new(
            vec![vec![0], vec![1], vec![2]],
            &[(0, 1), (1, 2)],
            &[1],
            &[40.0, 0.0, 40.0],
        );
        let x = f.run();
        assert_eq!(x[0], x[1]);
        assert_eq!(x[1], x[2]);
    }

    #[test]
    fn layers_keep_separation() {
        // 0 → {1, 2, 3}
        let f = Fixture::new(
            vec![vec![0], vec![1, 2, 3]],
            &[(0, 1), (0, 2), (0, 3)],
            &[],
            &[40.0, 40.0, 40.0, 40.0],
        );
        let x = f.run();
        assert!(x[2] - x[1] >= 90.0 - 1e-9 && x[3] - x[2] >= 90.0 - 1e-9);
        // The parent sits over the median child.
        assert_eq!(x[0], x[2]);
    }

    /// The dummy of an edge between offset nodes lines up with one end, as in
    /// dagre, rather than sitting halfway (which would draw a straight line).
    #[test]
    fn dummy_aligns_with_an_endpoint() {
        // Rank 0: a(0) b(1); rank 1: dummies d(2) under a, e(3) under b;
        // rank 2: c(4) fed by both.
        let f = Fixture::new(
            vec![vec![0, 1], vec![2, 3], vec![4]],
            &[(0, 2), (1, 3), (2, 4), (3, 4)],
            &[2, 3],
            &[100.0, 100.0, 0.0, 0.0, 100.0],
        );
        let x = f.run();
        let aligned = |d: usize| {
            (x[d] - x[0]).abs() < 1e-9 || (x[d] - x[1]).abs() < 1e-9 || (x[d] - x[4]).abs() < 1e-9
        };
        assert!(aligned(2) && aligned(3), "{x:?}");
    }

    #[test]
    fn inner_segment_conflict_is_detected() {
        // Layer 0: u0(0) d0(1); layer 1: d1(2) u1(3), edges u0→u1 crosses d0→d1.
        let f = Fixture::new(
            vec![vec![0, 1], vec![2, 3]],
            &[(0, 3), (1, 2)],
            &[1, 2],
            &[10.0, 0.0, 0.0, 10.0],
        );
        let g = LayerGraph {
            layers: &f.layers,
            preds: &f.preds,
            succs: &f.succs,
            dummy: &f.dummy,
            border: &f.border,
            width: &f.width,
        };
        let order = vec![0, 1, 0, 1];
        let mut c = Conflicts::default();
        type1_conflicts(&g, &order, &mut c);
        assert!(c.has(0, 3));
        assert!(!c.has(1, 2));
    }
}
