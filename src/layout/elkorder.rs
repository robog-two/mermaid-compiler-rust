//! ELK's layer-sweep crossing minimisation (`LayerSweepCrossingMinimizer` with the
//! `BarycenterHeuristic`), used under ELK in place of the dagre-style sweeps.
//!
//! As mermaid configures ELK (`elk.layered`, `crossingMinimization.strategy:
//! LAYER_SWEEP`, ELK's defaults otherwise):
//! - every edge end is a port of its own; a node's ports on the side facing the
//!   fixed layer get ranks (`NodeRelativePortDistributor` or, chosen by a coin
//!   flip as ELK does, `LayerTotalPortDistributor`), and a node's barycentre is
//!   the mean rank of the ports its edges reach, plus a small random perturbation;
//!   after each layer the ports facing the sweep are sorted by the ranks they
//!   reach (`AbstractBarycenterPortDistributor`), and crossings are counted
//!   between ports, not nodes;
//! - `thoroughness` (7) runs, each sweeping back and forth while the crossing count
//!   falls, the best run kept; runs start from a random first layer, except, with
//!   `considerModelOrder` (mermaid's `NODES_AND_EDGES`, on the root graph only),
//!   the first two, which start from the model order (and the first stops at once
//!   if that order has no crossing);
//! - `java.util.Random` seeded with ELK's `randomSeed` (1), drawn from in ELK's order;
//! - a two-sided greedy switch afterwards where ELK runs one by default: graphs laid
//!   out on their own (not hierarchically) with fewer than 40 nodes.
//!
//! Under ELK every container is a graph of its own (see `nested`); the graphs of
//! one hierarchical run are ordered together (`minimize_run`), as ELK's
//! hierarchical layer sweep does: a container is either ordered on its own first
//! ("bottom up", its border ports then fixed in its parent in the order of its
//! port dummies) or within its parent's sweeps, which dive into it. Clusters
//! integrated into a pass (dagre-style) stay contiguous, with their order kept
//! over the layers, through the same `OrderCtx::arrange` the dagre-style sweeps
//! use.

use super::{InLayer, OrderCtx};

/// `java.util.Random` (a 48-bit linear congruential generator), bit for bit.
#[derive(Debug, Clone)]
pub(super) struct JavaRandom(u64);

const MULTIPLIER: u64 = 0x5_DEEC_E66D;
const MASK: u64 = (1 << 48) - 1;

impl JavaRandom {
    pub(super) fn new(seed: i64) -> Self {
        JavaRandom((seed as u64 ^ MULTIPLIER) & MASK)
    }

    pub(super) fn set_seed(&mut self, seed: i64) {
        *self = JavaRandom::new(seed);
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(MULTIPLIER).wrapping_add(0xB)) & MASK;
        (self.0 >> (48 - bits)) as u32 as i32
    }

    pub(super) fn next_long(&mut self) -> i64 {
        let hi = self.next(32) as i64;
        let lo = self.next(32) as i64;
        (hi << 32).wrapping_add(lo)
    }

    pub(super) fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    pub(super) fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        ((hi << 27) + lo) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub(super) fn next_float(&mut self) -> f64 {
        self.next(24) as f64 / (1u32 << 24) as f64
    }
}

/// How ELK minimises crossings in one graph.
#[derive(Debug, Clone, Copy)]
pub(super) struct Options {
    /// `considerModelOrder` other than `NONE`: the first runs start from the
    /// model order.
    pub model_order: bool,
    /// Run the two-sided greedy switch afterwards.
    pub greedy_switch: bool,
    /// ELK's `thoroughness`: the number of runs.
    pub thoroughness: usize,
}

/// `BarycenterHeuristic.RANDOM_AMOUNT` (a Java `float`).
const RANDOM_AMOUNT: f64 = 0.07_f32 as f64;

struct Sweeper<'a> {
    layers: Vec<Vec<usize>>,
    /// Cluster border vertices: no ELK node, kept at their block's ends by `arrange`.
    border: &'a [bool],
    /// Edge segments between consecutive layers (from the upper layer's vertex).
    segs: &'a [(usize, usize)],
    /// Per vertex: its ports (segments) on the side towards the next / previous
    /// layer, in top-to-bottom order.
    east: Vec<Vec<usize>>,
    west: Vec<Vec<usize>>,
    /// Per segment: the rank of its source (east) and target (west) port.
    east_rank: Vec<f64>,
    west_rank: Vec<f64>,
    node_relative: bool,
    rng: JavaRandom,
    /// Nodes whose port order is fixed (a container ordered on its own):
    /// never redistributed.
    fixed_node: Vec<bool>,
    /// Per vertex: the run's graph of the container it is, if any.
    nested: Vec<Option<usize>>,
    /// External port dummies.
    ext: Vec<bool>,
    ctx: &'a OrderCtx<'a>,
    /// The in-layer edges of inverted ports, and per vertex the ones it has.
    inl: &'a [InLayer],
    inl_of: Vec<Vec<usize>>,
    /// Per in-layer edge: where its port on the node is among the node's ports
    /// on that side, top to bottom (`usize::MAX`: last).
    inl_pos: Vec<usize>,
    /// Per in-layer edge: the key of its edge (see `RunGraph::seg_key`).
    inl_key: Vec<usize>,
}

/// A port in the sweeps: a segment's on its source (east) or target (west), or
/// an end of an in-layer edge (`true`: on its node, else on its dummy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Port {
    East(usize),
    West(usize),
    InLayer(usize, bool),
}

/// ELK's `BinaryIndexedTree`: counts of positions, with `rank(i)` the number
/// below `i`.
struct Fenwick {
    tree: Vec<usize>,
    count: Vec<usize>,
    size: usize,
}

impl Fenwick {
    fn new(n: usize) -> Self {
        Fenwick {
            tree: vec![0; n + 1],
            count: vec![0; n],
            size: 0,
        }
    }

    fn add(&mut self, i: usize) {
        self.count[i] += 1;
        self.size += 1;
        let mut j = i + 1;
        while j < self.tree.len() {
            self.tree[j] += 1;
            j += j & j.wrapping_neg();
        }
    }

    fn rank(&self, i: usize) -> usize {
        let (mut j, mut sum) = (i, 0);
        while j > 0 {
            sum += self.tree[j];
            j -= j & j.wrapping_neg();
        }
        sum
    }

    fn remove_all(&mut self, i: usize) {
        let c = std::mem::take(&mut self.count[i]);
        if c == 0 {
            return;
        }
        self.size -= c;
        let mut j = i + 1;
        while j < self.tree.len() {
            self.tree[j] -= c;
            j += j & j.wrapping_neg();
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    layers: Vec<Vec<usize>>,
    east: Vec<Vec<usize>>,
    west: Vec<Vec<usize>>,
    inl_pos: Vec<usize>,
}

/// What ELK knows of an edge segment: its edge (whose index is the edge's model
/// order), the real node the edge finally leads to, and whether it was reversed.
#[derive(Debug, Clone, Copy)]
pub(super) struct SegInfo {
    pub edge: usize,
    pub target: usize,
    pub reversed: bool,
}

/// The graph ELK orders: its vertices (the first `real` are real nodes, whose
/// index is their model order) and the edge segments between its layers.
pub(super) struct Graph<'a> {
    /// Cluster border vertices.
    pub border: &'a [bool],
    /// Label dummies (made before layering).
    pub label: &'a [bool],
    /// Edge segments between consecutive layers (from the upper layer's vertex).
    pub segs: &'a [(usize, usize)],
    pub info: &'a [SegInfo],
    pub real: usize,
    /// The in-layer edges of inverted ports.
    pub inl: &'a [InLayer],
}

/// Order the layers as ELK's layer sweep does. `layers` holds ELK's layers.
pub(super) fn minimize(
    layers: Vec<Vec<usize>>,
    graph: &Graph,
    ctx: &OrderCtx,
    opts: Options,
) -> Vec<Vec<usize>> {
    let nv = graph.border.len();
    let run = vec![RunGraph {
        layers,
        graph: Graph { ..*graph },
        ctx,
        opts,
        ext_key: vec![None; nv],
        seg_key: (0..graph.segs.len()).collect(),
        parent: None,
    }];
    minimize_run(run).pop().unwrap_or_default()
}

/// One graph of an ELK run (`hierarchyHandling: INCLUDE_CHILDREN`): the root of
/// the run, or a container laid out within it.
pub(super) struct RunGraph<'a> {
    /// ELK's layers.
    pub layers: Vec<Vec<usize>>,
    pub graph: Graph<'a>,
    pub ctx: &'a OrderCtx<'a>,
    pub opts: Options,
    /// Per vertex: for an external port dummy, the key of the edge it is the
    /// port of (the container's port with the same key in its parent).
    pub ext_key: Vec<Option<usize>>,
    /// Per segment: the key of its edge.
    pub seg_key: Vec<usize>,
    /// The graph's container: (its graph's index in the run, its vertex there).
    pub parent: Option<(usize, usize)>,
}

/// ELK's `hierarchicalSweepiness` default: a container whose paths lead
/// mostly to random nodes rather than its border ports is ordered on its own
/// (`LayerSweepTypeDecider`).
const HIERARCHICAL_SWEEPINESS: f64 = 0.1;

/// Per graph of a run: (options, external port keys, segment keys, parent).
type RunExtra = (
    Options,
    Vec<Option<usize>>,
    Vec<usize>,
    Option<(usize, usize)>,
);

/// Order every graph of an ELK run as `LayerSweepCrossingMinimizer` does with
/// hierarchical crossing minimisation: the containers it decides to order
/// "bottom up" each on their own, deepest first, their border ports' order
/// fixed in their parents from where their contents put the port dummies; the
/// others within their parent's sweeps, which dive into them on reaching their
/// layer (the parent's ports there order the container's first layer of port
/// dummies, and its last layer then orders the ports on the far side).
/// `graphs[0]` is the root. Returns each graph's layers.
pub(super) fn minimize_run(graphs: Vec<RunGraph>) -> Vec<Vec<Vec<usize>>> {
    let ng = graphs.len();
    let mut rng = JavaRandom::new(1);
    // `LayerSweepCrossingMinimizer.initialize`: the seed every run restarts from,
    // then each graph's port distributor, by a coin flip of its own generator.
    let seed = rng.next_long();
    let mut sws = Vec::with_capacity(ng);
    // Per graph, per layer: its ghost vertices, and whether it has others.
    let mut ghosts: Vec<Vec<(Vec<usize>, bool)>> = Vec::with_capacity(ng);
    let mut extra: Vec<RunExtra> = Vec::with_capacity(ng);
    for (gi, g) in graphs.into_iter().enumerate() {
        let RunGraph {
            layers,
            graph,
            ctx,
            opts,
            ext_key,
            seg_key,
            parent,
        } = g;
        let Graph {
            border,
            label,
            segs,
            info,
            real,
            inl,
        } = graph;
        let nv = border.len();
        let mut inl_of = vec![Vec::new(); nv];
        for (k, l) in inl.iter().enumerate() {
            inl_of[l.node].push(k);
            inl_of[l.dummy].push(k);
        }
        let mut east = vec![Vec::new(); nv];
        let mut west = vec![Vec::new(); nv];
        for (s, &(a, b)) in segs.iter().enumerate() {
            east[a].push(s);
            west[b].push(s);
        }
        // External ports without an edge inside (an edge to the container
        // itself) are ELK nodes only where the container has other external
        // ports (`CompoundGraphPreprocessor` makes their dummies only then);
        // otherwise they are kept out of the sweeps, and put back at the ends of
        // their layers afterwards.
        let unconnected = |v: usize| east[v].is_empty() && west[v].is_empty();
        let connected_ext = (0..nv).any(|v| ext_key[v].is_some() && !unconnected(v));
        let ghost = |v: usize| !connected_ext && ext_key[v].is_some() && unconnected(v);
        let mut kept = Vec::with_capacity(layers.len());
        let mut layer_ghosts = Vec::with_capacity(layers.len());
        for layer in layers {
            let (g, l): (Vec<usize>, Vec<usize>) = layer.into_iter().partition(|&v| ghost(v));
            layer_ghosts.push((g, !l.is_empty()));
            if !l.is_empty() {
                kept.push(l);
            }
        }
        ghosts.push(layer_ghosts);
        let mut layers = kept;
        creation_order(&mut layers, &east, segs, info, real, label, inl);
        if opts.model_order {
            sort_by_model_order(
                &mut layers,
                &mut east,
                &mut west,
                segs,
                info,
                real,
                inl,
                ctx,
            );
        } else {
            for layer in layers.iter_mut() {
                let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
                *layer = ctx.arrange(layer, &keys, &mut Vec::new());
            }
            // A node's ports in the order they were made, edge by edge; ELK lists
            // them clockwise, so its west side runs bottom to top.
            for w in west.iter_mut() {
                w.sort_by_key(|&s| std::cmp::Reverse((info[s].edge, s)));
            }
        }
        let mut own = if gi == 0 {
            std::mem::replace(&mut rng, JavaRandom::new(0))
        } else {
            JavaRandom::new(1)
        };
        let node_relative = own.next_boolean();
        sws.push(Sweeper {
            layers,
            border,
            segs,
            east,
            west,
            east_rank: vec![0.0; segs.len()],
            west_rank: vec![0.0; segs.len()],
            node_relative,
            rng: own,
            fixed_node: vec![false; nv],
            nested: vec![None; nv],
            ext: ext_key.iter().map(Option::is_some).collect(),
            ctx,
            inl,
            inl_of,
            inl_pos: vec![usize::MAX; inl.len()],
            inl_key: inl
                .iter()
                .map(|l| {
                    (0..segs.len())
                        .find(|&s| info[s].edge == l.edge)
                        .map_or(usize::MAX, |s| seg_key[s])
                })
                .collect(),
        });
        extra.push((opts, ext_key, seg_key, parent));
    }
    let mut into = vec![false; ng];
    for (gi, e) in extra.iter().enumerate() {
        if let Some((pg, v)) = e.3 {
            sws[pg].nested[v] = Some(gi);
            into[gi] = !bottom_up(&sws[gi], &sws[pg], v);
        }
    }
    let mut run = Run {
        sw: sws,
        extra,
        into,
        seed,
    };
    // Bottom-up graphs (and the root) in the reverse of a breadth-first walk
    // that finds a graph's children in its layers' order
    // (`LayerSweepCrossingMinimizer.initialize`).
    let mut bfs = vec![0];
    let mut i = 0;
    while i < bfs.len() {
        let cur = bfs[i];
        for layer in &run.sw[cur].layers {
            bfs.extend(layer.iter().filter_map(|&v| run.sw[cur].nested[v]));
        }
        i += 1;
    }
    // (Every graph is reached; any that were not would go first.)
    let mut seen = vec![false; ng];
    for &g in &bfs {
        seen[g] = true;
    }
    let missed: Vec<usize> = (0..ng).filter(|&g| !seen[g]).collect();
    bfs.extend(missed);
    let order: Vec<usize> = bfs.into_iter().rev().filter(|&g| !run.into[g]).collect();
    for g in order {
        let opts = run.extra[g].0;
        // A lone graph of a single layer has nothing to order (`process`'s
        // short cut); a container is still swept.
        if run.sw[g].layers.is_empty() || (ng == 1 && run.sw[g].layers.len() < 2) {
            continue;
        }
        run.sw[0].rng.set_seed(run.seed);
        if let Some(best) = run.compare_randomized_layouts(g, opts) {
            for (k, snap) in best.into_iter().enumerate() {
                if let Some(snap) = snap {
                    run.sw[k].layers = snap.layers;
                    run.sw[k].east = snap.east;
                    run.sw[k].west = snap.west;
                    run.sw[k].inl_pos = snap.inl_pos;
                }
            }
        }
        if opts.greedy_switch && ng == 1 {
            // A second `LayerSweepCrossingMinimizer` (two-sided greedy switch) on the
            // same random generator.
            let _ = run.sw[0].rng.next_long();
            run.sw[0].greedy_switch();
        }
        // `setPortOrderOnParentGraph`.
        if let Some((pg, v)) = run.extra[g].3 {
            let last = run.sw[g].layers.len().saturating_sub(1);
            run.ports_from_dummies(g, last, pg, v, true);
            run.ports_from_dummies(g, 0, pg, v, false);
            run.sw[pg].fixed_node[v] = true;
        }
    }
    run.sw
        .into_iter()
        .zip(ghosts)
        .map(|(s, gs)| {
            let mut kept = s.layers.into_iter();
            gs.into_iter()
                .map(|(g, has_others)| {
                    let mut layer = if has_others {
                        kept.next().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    layer.extend(g);
                    layer
                })
                .collect()
        })
        .collect()
}

/// `LayerSweepTypeDecider.useBottomUp` for the graph of `child`, the container
/// at vertex `v` of `parent`.
fn bottom_up(child: &Sweeper, parent: &Sweeper, v: usize) -> bool {
    if parent.side_ports(v, true).len() < 2 && parent.side_ports(v, false).len() < 2 {
        return true;
    }
    let nv = child.border.len();
    // (connected edges, hierarchical influence, random influence)
    let mut info = vec![(0usize, 0usize, 0usize); nv];
    let (mut to_random, mut to_hier) = (0usize, 0usize);
    let last = child.layers.len().saturating_sub(1);
    for (li, layer) in child.layers.iter().enumerate() {
        for &u in layer {
            if child.border[u] {
                continue;
            }
            let mut cur = info[u];
            if child.ext[u] {
                cur.1 = 1;
                if li == last && li > 0 {
                    to_hier += cur.0;
                }
            } else if child.side_ports(u, false).is_empty() {
                cur.2 = 1;
            } else if child.side_ports(u, true).is_empty() {
                to_random += cur.0;
            }
            info[u] = cur;
            // Its outgoing edges: to the next layer, and in-layer ones.
            let in_layer = child.inl_of[u]
                .iter()
                .map(|&k| child.inl[k])
                .filter_map(|l| {
                    let source = if l.before { l.node } else { l.dummy };
                    (source == u).then_some(if l.before { l.dummy } else { l.node })
                });
            let targets: Vec<usize> = child.east[u]
                .iter()
                .map(|&s| child.segs[s].1)
                .chain(in_layer)
                .collect();
            for t in targets {
                to_random += cur.2;
                to_hier += cur.1;
                info[t].0 += cur.0 + 1;
                info[t].1 += cur.1;
                info[t].2 += cur.2;
            }
        }
    }
    let all = (to_random + to_hier) as f64;
    let normalized = if all == 0.0 {
        f64::INFINITY
    } else {
        (to_random as f64 - to_hier as f64) / all
    };
    normalized >= HIERARCHICAL_SWEEPINESS
}

/// The graphs of an ELK run being ordered.
struct Run<'a> {
    sw: Vec<Sweeper<'a>>,
    extra: Vec<RunExtra>,
    /// Per graph: ordered within its parent's sweeps.
    into: Vec<bool>,
    seed: i64,
}

impl Run<'_> {
    /// The order of the ports on one side of the container at vertex `v` of
    /// graph `pg` from the order of its port dummies in layer `li` of its graph
    /// `g` (`sortPortsByDummyPositionsInLastLayer`); `east`: the side towards
    /// the next layer.
    fn ports_from_dummies(&mut self, g: usize, li: usize, pg: usize, v: usize, east: bool) {
        let Some(layer) = self.sw[g].layers.get(li) else {
            return;
        };
        if layer.first().is_none_or(|&u| !self.sw[g].ext[u]) {
            return;
        }
        let keys: Vec<usize> = layer.iter().filter_map(|&u| self.extra[g].1[u]).collect();
        let seg_key = &self.extra[pg].2;
        let rank_of = |key: usize| keys.iter().position(|&k| k == key).unwrap_or(usize::MAX);
        let parent = &mut self.sw[pg];
        let ports = if east {
            &mut parent.east[v]
        } else {
            &mut parent.west[v]
        };
        ports.sort_by_key(|&s| rank_of(seg_key[s]));
        // An inverted port on that side goes where its dummy is.
        for i in 0..parent.inl_of[v].len() {
            let k = parent.inl_of[v][i];
            let l = parent.inl[k];
            if l.node != v || l.before == east {
                continue;
            }
            let r = rank_of(parent.inl_key[k]);
            let ports = if east {
                &parent.east[v]
            } else {
                &parent.west[v]
            };
            parent.inl_pos[k] = ports.iter().filter(|&&s| rank_of(seg_key[s]) < r).count();
        }
    }

    /// Order layer `li` of graph `g` (its port dummies) by the ports on one side
    /// of its container, vertex `v` of graph `pg` (`sortPortDummiesByPortPositions`).
    fn dummies_from_ports(&mut self, g: usize, li: usize, pg: usize, v: usize, west: bool) {
        let parent = &self.sw[pg];
        let keys: Vec<usize> = parent
            .side_ports(v, !west)
            .into_iter()
            .filter_map(|p| match p {
                Port::East(s) | Port::West(s) => Some(self.extra[pg].2[s]),
                Port::InLayer(k, true) => Some(parent.inl_key[k]),
                Port::InLayer(_, false) => None,
            })
            .collect();
        let ext_key = &self.extra[g].1;
        let rank = |u: usize| {
            ext_key[u]
                .and_then(|k| keys.iter().position(|&x| x == k))
                .unwrap_or(usize::MAX)
        };
        self.sw[g].layers[li].sort_by_key(|&u| rank(u));
    }

    /// Graph `g` and the graphs ordered within its sweeps.
    fn subtree(&self, g: usize) -> Vec<usize> {
        let mut out = vec![g];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            for (c, e) in self.extra.iter().enumerate() {
                if self.into[c] && e.3.is_some_and(|(p, _)| p == cur) {
                    out.push(c);
                }
            }
            i += 1;
        }
        out
    }

    fn snapshot(&self, g: usize) -> Vec<Option<Snapshot>> {
        let mut out: Vec<Option<Snapshot>> = (0..self.sw.len()).map(|_| None).collect();
        for k in self.subtree(g) {
            out[k] = Some(self.sw[k].snapshot());
        }
        out
    }

    /// Crossings of graph `g` and of the graphs ordered within its sweeps.
    fn crossings(&self, g: usize) -> usize {
        self.subtree(g)
            .iter()
            .map(|&k| self.sw[k].crossings())
            .sum()
    }

    /// `compareDifferentRandomizedLayouts`: the best of `thoroughness` runs, or
    /// `None` to keep the order as it is.
    fn compare_randomized_layouts(
        &mut self,
        g: usize,
        opts: Options,
    ) -> Option<Vec<Option<Snapshot>>> {
        let mut first_try = opts.model_order;
        let mut best = None;
        let mut best_crossings = usize::MAX;
        for _ in 0..opts.thoroughness.max(1) {
            let (crossings, run_best) = self.run(g, &mut first_try);
            if crossings < best_crossings {
                best_crossings = crossings;
                if run_best.is_some() {
                    best = run_best;
                }
                if crossings == 0 {
                    break;
                }
            }
        }
        best
    }

    /// `minimizeCrossingsWithCounter`: one run; its crossing count and best order.
    /// The first run with model order sweeps forward from the model order (and
    /// stops at once if that has no crossing); the others start from a random
    /// first layer. (Later ELK versions also start the second run from the
    /// current order; elkjs 0.12, which mermaid 12.3 bundles, does not.)
    fn run(&mut self, g: usize, first_try: &mut bool) -> (usize, Option<Vec<Option<Snapshot>>>) {
        // The sweep direction comes from the run's (the root's) generator.
        let mut forward = self.sw[0].rng.next_boolean();
        if *first_try && self.crossings(g) == 0 {
            // The model order is already free of crossings: keep it.
            return (0, None);
        }
        if *first_try {
            forward = true;
        } else {
            self.sw[g].set_first_layer_order(forward);
        }
        self.sweep(g, forward, !*first_try, true);
        *first_try = false;
        let mut crossings = self.crossings(g);
        loop {
            let best = self.snapshot(g);
            if crossings == 0 {
                return (0, Some(best));
            }
            forward = !forward;
            let old = crossings;
            self.sweep(g, forward, false, false);
            crossings = self.crossings(g);
            if old <= crossings {
                return (old, Some(best));
            }
        }
    }

    /// `sweepReducingCrossings` on graph `g`: `first` places nodes without a
    /// barycentre at random; `first_sweep` is the run's first sweep, which the
    /// graphs dived into take as theirs.
    fn sweep(&mut self, g: usize, forward: bool, first: bool, first_sweep: bool) {
        let n = self.sw[g].layers.len();
        if n == 0 {
            return;
        }
        let mut before = Vec::new();
        let start = self.sw[g].start(forward);
        {
            let sw = &self.sw[g];
            let keys: Vec<f64> = (0..sw.layers[start].len()).map(|i| i as f64).collect();
            sw.ctx.arrange(&sw.layers[start], &keys, &mut before);
        }
        self.dive(g, start, forward, first_sweep);
        let free: Vec<usize> = if forward {
            (1..n).collect()
        } else {
            (0..n - 1).rev().collect()
        };
        for li in free {
            let fixed = if forward { li - 1 } else { li + 1 };
            let sw = &mut self.sw[g];
            sw.rank_ports(fixed, forward);
            let layer = std::mem::take(&mut sw.layers[li]);
            // A layer of port dummies counts as ordered (`preOrdered`).
            let first = first && layer.first().is_none_or(|&u| !sw.ext[u]);
            let bary = sw.barycenters(&layer, forward, first);
            sw.layers[li] = sw.sorted(&layer, bary, &mut before);
            sw.distribute_ports(li, fixed, forward);
            self.dive(g, li, forward, first_sweep);
        }
    }

    /// `sweepInHierarchicalNodes`: sweep the graphs of the containers in layer
    /// `li` of graph `g` that are ordered within its sweeps.
    fn dive(&mut self, g: usize, li: usize, forward: bool, first_sweep: bool) {
        let containers: Vec<(usize, usize)> = self.sw[g].layers[li]
            .iter()
            .filter_map(|&v| Some((v, self.sw[g].nested[v]?)))
            .filter(|&(_, c)| self.into[c])
            .collect();
        for (v, c) in containers {
            let n = self.sw[c].layers.len();
            if n == 0 {
                continue;
            }
            let start = self.sw[c].start(forward);
            let first_is_port = self.sw[c].layers[start]
                .first()
                .is_some_and(|&u| self.sw[c].ext[u]);
            if first_is_port {
                self.dummies_from_ports(c, start, g, v, forward);
            } else {
                self.sw[c].set_first_layer_order(forward);
            }
            self.sweep(c, forward, first_sweep, first_sweep);
            let end = if forward { n - 1 } else { 0 };
            self.ports_from_dummies(c, end, g, v, forward);
        }
    }
}

/// The order ELK's layers have before crossing minimisation: real nodes in model
/// order, then label dummies (made before layering) in edge order, then long-edge
/// dummies as `LongEdgeSplitter` appends them, layer by layer, node by node, edge
/// by edge (an inverted port's dummy not yet there: the edge leaves its node
/// through the port, its clockwise last), and last the dummies of inverted
/// ports (`InvertedPortProcessor`), by their nodes.
fn creation_order(
    layers: &mut [Vec<usize>],
    east: &[Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    label: &[bool],
    inl: &[InLayer],
) {
    let nv = east.len();
    let mut edge_of = vec![usize::MAX; nv];
    for (s, &(_, b)) in segs.iter().enumerate() {
        edge_of[b] = edge_of[b].min(info[s].edge);
    }
    let mut placed = vec![false; nv];
    let mut inverted = vec![false; nv];
    // Per node: the dummies of its inverted ports on its previous layer's side.
    let mut before_of: Vec<Vec<usize>> = vec![Vec::new(); nv];
    for l in inl {
        placed[l.dummy] = true;
        inverted[l.dummy] = true;
        if l.before {
            before_of[l.node].push(l.dummy);
        }
    }
    for layer in layers.iter_mut() {
        layer.retain(|&v| !inverted[v]);
    }
    for layer in layers.iter_mut() {
        let mut first: Vec<usize> = layer
            .iter()
            .copied()
            .filter(|&v| v < real || label[v])
            .collect();
        first.sort_by_key(|&v| if v < real { (0, v) } else { (1, edge_of[v]) });
        for &v in &first {
            placed[v] = true;
        }
        let rest: Vec<usize> = layer.iter().copied().filter(|&v| !placed[v]).collect();
        first.extend(rest);
        *layer = first;
    }
    for li in 0..layers.len().saturating_sub(1) {
        let mut appended = Vec::new();
        for &u in &layers[li] {
            for &s in east[u]
                .iter()
                .chain(before_of[u].iter().flat_map(|&d| &east[d]))
            {
                let d = segs[s].1;
                if !placed[d] {
                    placed[d] = true;
                    appended.push(d);
                }
            }
        }
        let next = &mut layers[li + 1];
        let mut order: Vec<usize> = next
            .iter()
            .copied()
            .filter(|&v| v < real || label[v])
            .collect();
        let fresh = order.len();
        order.extend(appended);
        let mut taken = vec![false; nv];
        for &v in &order {
            taken[v] = true;
        }
        order.extend(next.iter().copied().filter(|&v| !taken[v]));
        debug_assert!(order.len() == next.len() && fresh <= order.len());
        *next = order;
    }
    // The dummies of inverted ports, node by node: those of input ports on the
    // next layer's side, then of output ports on the previous layer's.
    let mut at = vec![usize::MAX; if inl.is_empty() { 0 } else { nv }];
    for layer in layers.iter_mut().filter(|_| !inl.is_empty()) {
        for (i, &v) in layer.iter().enumerate() {
            at[v] = i;
        }
        let mut dummies: Vec<(usize, bool, usize, usize)> = inl
            .iter()
            .enumerate()
            .filter(|(_, l)| at[l.node] != usize::MAX)
            .map(|(k, l)| (at[l.node], l.before, k, l.dummy))
            .collect();
        dummies.sort_unstable();
        for &v in layer.iter() {
            at[v] = usize::MAX;
        }
        layer.extend(dummies.into_iter().map(|d| d.3));
    }
}

/// `SortByInputModelProcessor` with `considerModelOrder: NODES_AND_EDGES`, layer
/// by layer from the first: the layer's nodes sorted (`ModelOrderNodeComparator`),
/// their ports sorted (`ModelOrderPortComparator`), the nodes sorted again. Real
/// nodes keep their model order among themselves; a dummy goes by where its edge
/// comes from in the layer before, and above a real node without an edge from
/// there (`longEdgeStrategy: DUMMY_NODE_OVER`), else by its edge's model order.
#[allow(clippy::too_many_arguments)]
fn sort_by_model_order(
    layers: &mut [Vec<usize>],
    east: &mut [Vec<usize>],
    west: &mut [Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    inl: &[InLayer],
    ctx: &OrderCtx,
) {
    let nv = east.len();
    // An in-layer edge into a node's first port (clockwise) with an incoming
    // edge gives the node its edge model order: the port of an inverted input
    // port on the next layer's side, or the dummy of one on the previous layer's.
    let mut first_in: Vec<Option<usize>> = vec![None; if inl.is_empty() { 0 } else { nv }];
    // Per dummy of an inverted port (a feedback long-edge dummy to ELK): its
    // edge's source and target nodes, one of them the node beside it.
    let mut feedback: Vec<Option<(usize, usize)>> = first_in.iter().map(|_| None).collect();
    for l in inl {
        first_in[if l.before { l.dummy } else { l.node }] = Some(l.edge);
        let (into, from) = (&west[l.dummy], &east[l.dummy]);
        feedback[l.dummy] = if l.before {
            from.first().map(|&s| (l.node, segs[s].1))
        } else {
            into.first().map(|&s| (segs[s].0, l.node))
        };
    }
    let mut pos = vec![usize::MAX; nv];
    // Per segment: its port's index on its source's east side.
    let mut port_at = vec![0usize; segs.len()];
    let mut before = Vec::new();
    for slot in layers.iter_mut() {
        let mut layer = std::mem::take(slot);
        let ctx_fb = (&first_in[..], &feedback[..]);
        model_order_sort(&mut layer, west, segs, info, real, &pos, &port_at, ctx_fb);
        for &v in &layer {
            // Incoming ports by where they come from (top to bottom).
            west[v].sort_by_key(|&s| (pos[segs[s].0], port_at[s]));
            // Outgoing ports by the model order of the node they lead to: the
            // earliest edge there, so that edges to one node stay together.
            let first_to = |t: usize| {
                east[v]
                    .iter()
                    .filter(|&&s| info[s].target == t && !info[s].reversed)
                    .map(|&s| info[s].edge)
                    .min()
            };
            let mut keyed: Vec<(usize, usize, usize)> = east[v]
                .iter()
                .map(|&s| {
                    let own = info[s].edge;
                    (first_to(info[s].target).unwrap_or(own), own, s)
                })
                .collect();
            keyed.sort_unstable();
            east[v] = keyed.into_iter().map(|k| k.2).collect();
            for (i, &s) in east[v].iter().enumerate() {
                port_at[s] = i;
            }
        }
        let ctx_fb = (&first_in[..], &feedback[..]);
        model_order_sort(&mut layer, west, segs, info, real, &pos, &port_at, ctx_fb);
        let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
        let layer = ctx.arrange(&layer, &keys, &mut before);
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
        *slot = layer;
    }
}

/// Insertion sort with ELK's `ModelOrderNodeComparator` (`pos`: positions in the
/// layers sorted so far, `usize::MAX` elsewhere). Like ELK's, the comparator
/// remembers its decisions and their transitive consequences, which settles
/// pairs its rules alone would order inconsistently.
#[allow(clippy::too_many_arguments)]
fn model_order_sort(
    layer: &mut [usize],
    west: &[Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    pos: &[usize],
    port_at: &[usize],
    (first_in, feedback): Feedback,
) {
    use std::cmp::Ordering::{Greater, Less};
    let n = layer.len();
    if n < 2 {
        return;
    }
    let old = layer.to_vec();
    let mut slot_of = std::collections::HashMap::new();
    for (i, &v) in old.iter().enumerate() {
        slot_of.insert(v, i);
    }
    let fb = |v: usize| feedback.get(v).copied().flatten();
    // `handleHelperDummyNodes`: a feedback dummy goes after the node beside it
    // (its edge's source or target), and otherwise as that node goes; `None`
    // where it says nothing.
    let helper = |i: usize, j: usize| -> Option<Rule> {
        let (a, b) = (old[i], old[j]);
        let side = |(src, dst): (usize, usize)| -> Option<usize> {
            [src, dst].into_iter().find(|v| slot_of.contains_key(v))
        };
        match (fb(a), fb(b)) {
            (Some(fa), None) if b < real => {
                if fa.0 == b || fa.1 == b {
                    return Some(Rule::Decided(Greater));
                }
                side(fa).map(|r| Rule::Redirect(slot_of[&r], j))
            }
            (None, Some(fb_)) if a < real => {
                if fb_.0 == a || fb_.1 == a {
                    return Some(Rule::Decided(Less));
                }
                side(fb_).map(|r| Rule::Redirect(i, slot_of[&r]))
            }
            (Some(fa), Some(fb_)) => {
                let (ra, rb) = (side(fa)?, side(fb_)?);
                if ra != rb {
                    return Some(Rule::Redirect(slot_of[&ra], slot_of[&rb]));
                }
                // On one node: a source feedback dummy (of a port towards the
                // previous layer) after a target one.
                match (fa.0 == ra, fb_.0 == rb) {
                    (true, true) => Some(Rule::Decided(if first_in[a] > first_in[b] {
                        Greater
                    } else {
                        Less
                    })),
                    (true, false) => Some(Rule::Decided(Greater)),
                    (false, true) => Some(Rule::Decided(Less)),
                    (false, false) => None,
                }
            }
            _ => None,
        }
    };
    // Per slot: the first port (clockwise: the west side from the bottom) with an
    // edge from the layer before, and the model order of the last west port's edge.
    let source: Vec<Option<usize>> = old
        .iter()
        .map(|&v| {
            west[v]
                .iter()
                .rev()
                .copied()
                .find(|&s| pos[segs[s].0] != usize::MAX)
        })
        .collect();
    let order: Vec<i64> = old
        .iter()
        .map(|&v| match first_in.get(v).copied().flatten() {
            Some(edge) => edge as i64,
            None => west[v]
                .last()
                .map_or(i64::from(i32::MAX), |&s| info[s].edge as i64),
        })
        .collect();
    let rule = |i: usize, j: usize| -> Rule {
        let (a, b) = (old[i], old[j]);
        if a < real && b < real {
            return Rule::Decided(a.cmp(&b));
        }
        Rule::Decided(match (source[i], source[j]) {
            (Some(sa), Some(sb)) => {
                let (na, nb) = (segs[sa].0, segs[sb].0);
                if na == nb {
                    port_at[sa].cmp(&port_at[sb])
                } else {
                    pos[na].cmp(&pos[nb])
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                if let Some(r) = helper(i, j) {
                    return r;
                }
                if order[i] > order[j] { Greater } else { Less }
            }
            (None, None) => {
                if let Some(r) = helper(i, j) {
                    return r;
                }
                Less
            }
        })
    };
    // A rule followed through its redirections, without bookkeeping.
    let plain = |mut i: usize, mut j: usize| -> std::cmp::Ordering {
        for _ in 0..=n {
            match rule(i, j) {
                Rule::Decided(o) => return o,
                Rule::Redirect(a, b) => (i, j) = (a, b),
            }
        }
        Less
    };
    // Where every node has an edge from the layer before and the real nodes'
    // model order agrees with where their edges come from, the rules are one
    // total order (sources are distinct ports), which the insertion sort below
    // would only find more slowly.
    let key = |i: usize| source[i].map(|s| (pos[segs[s].0], port_at[s]));
    if source.iter().all(Option::is_some) {
        let mut reals: Vec<usize> = (0..n).filter(|&i| old[i] < real).collect();
        reals.sort_unstable_by_key(|&i| old[i]);
        if reals.windows(2).all(|w| key(w[0]) < key(w[1])) {
            let mut slots: Vec<usize> = (0..n).collect();
            slots.sort_unstable_by_key(|&i| key(i));
            for (dst, &i) in layer.iter_mut().zip(&slots) {
                *dst = old[i];
            }
            return;
        }
    }
    let mut slots: Vec<usize> = (0..n).collect();
    if n > EXACT_MODEL_ORDER_SORT {
        // The transitive bookkeeping grows with the cube of the layer's width:
        // wider layers are insertion-sorted with the rules alone.
        insertion_sort(&mut slots, |i, j| plain(i, j) == Greater);
        for (dst, &i) in layer.iter_mut().zip(&slots) {
            *dst = old[i];
        }
        return;
    }
    // `biggerThan` / `smallerThan`, as bit sets over the layer's slots: row `i`
    // holds the slots known to be smaller (bigger) than slot `i`.
    let words = n.div_ceil(64);
    let mut bigger = vec![0u64; n * words];
    let mut smaller = vec![0u64; n * words];
    let has = |set: &[u64], i: usize, j: usize| set[i * words + j / 64] >> (j % 64) & 1 == 1;
    let mut row = vec![0u64; words];
    let mut members = Vec::with_capacity(n);
    // `updateBiggerAndSmallerAssociations(b, s)`.
    let mut associate = |bigger: &mut [u64], smaller: &mut [u64], b: usize, s: usize| {
        bigger[b * words + s / 64] |= 1 << (s % 64);
        smaller[s * words + b / 64] |= 1 << (b % 64);
        // Everything smaller than `s` is smaller than `b`, and gets `b` and all
        // that is bigger than `b` as bigger than itself.
        row.copy_from_slice(&smaller[b * words..(b + 1) * words]);
        row[b / 64] |= 1 << (b % 64);
        set_bits(&bigger[s * words..(s + 1) * words], &mut members);
        for &vs in &members {
            bigger[b * words + vs / 64] |= 1 << (vs % 64);
            for (w, x) in smaller[vs * words..(vs + 1) * words].iter_mut().zip(&row) {
                *w |= x;
            }
        }
        // Everything bigger than `b` is bigger than `s`, and gets `s` and all
        // that is smaller than `s` as smaller than itself.
        row.copy_from_slice(&bigger[s * words..(s + 1) * words]);
        row[s / 64] |= 1 << (s % 64);
        set_bits(&smaller[b * words..(b + 1) * words], &mut members);
        for &vb in &members {
            smaller[s * words + vb / 64] |= 1 << (vb % 64);
            for (w, x) in bigger[vb * words..(vb + 1) * words].iter_mut().zip(&row) {
                *w |= x;
            }
        }
    };
    let mut compare = |mut i: usize, mut j: usize| -> std::cmp::Ordering {
        for _ in 0..=n {
            if has(&bigger, i, j) {
                return Greater;
            }
            if has(&bigger, j, i) || has(&smaller, i, j) {
                return Less;
            }
            let o = match rule(i, j) {
                Rule::Decided(o) => o,
                Rule::Redirect(a, b) => {
                    (i, j) = (a, b);
                    continue;
                }
            };
            if o == Greater {
                associate(&mut bigger, &mut smaller, i, j);
            } else {
                associate(&mut bigger, &mut smaller, j, i);
            }
            return o;
        }
        Less
    };
    // Sort slots, so that the sets are indexed by them.
    insertion_sort(&mut slots, |i, j| compare(i, j) == Greater);
    for (dst, &i) in layer.iter_mut().zip(&slots) {
        *dst = old[i];
    }
}

/// Per vertex: the model order an in-layer edge gives it, and for a dummy of
/// an inverted port its edge's source and target (see `sort_by_model_order`).
type Feedback<'a> = (&'a [Option<usize>], &'a [Option<(usize, usize)>]);

/// What `ModelOrderNodeComparator` makes of two nodes: an order, or the order
/// of two others (a feedback dummy goes as the node beside it).
#[derive(Debug, Clone, Copy)]
enum Rule {
    Decided(std::cmp::Ordering),
    Redirect(usize, usize),
}

/// The widest layer `model_order_sort` sorts exactly as ELK does.
const EXACT_MODEL_ORDER_SORT: usize = 128;

/// `SortByInputModelProcessor.insertionSort`: `greater(a, b)` says `a` belongs
/// after `b`.
fn insertion_sort(items: &mut [usize], mut greater: impl FnMut(usize, usize) -> bool) {
    for i in 1..items.len() {
        let t = items[i];
        let mut j = i;
        while j > 0 && greater(items[j - 1], t) {
            items[j] = items[j - 1];
            j -= 1;
        }
        items[j] = t;
    }
}

/// The indices of the set bits of `set`, ascending.
fn set_bits(set: &[u64], out: &mut Vec<usize>) {
    out.clear();
    for (w, &word) in set.iter().enumerate() {
        let mut x = word;
        while x != 0 {
            out.push(w * 64 + x.trailing_zeros() as usize);
            x &= x - 1;
        }
    }
}

impl Sweeper<'_> {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            layers: self.layers.clone(),
            east: self.east.clone(),
            west: self.west.clone(),
            inl_pos: self.inl_pos.clone(),
        }
    }

    fn start(&self, forward: bool) -> usize {
        if forward { 0 } else { self.layers.len() - 1 }
    }

    /// `setFirstLayerOrder`: the first layer in random order.
    fn set_first_layer_order(&mut self, forward: bool) {
        let li = self.start(forward);
        let layer = std::mem::take(&mut self.layers[li]);
        let mut bary = Vec::with_capacity(layer.len());
        for &v in &layer {
            if !self.border[v] {
                bary.push((v, self.rng.next_double()));
            }
        }
        self.layers[li] = self.sorted(&layer, bary, &mut Vec::new());
    }

    /// Sort a layer's nodes by barycentre (a stable sort, as `Collections.sort`),
    /// then keep clusters contiguous.
    fn sorted(
        &self,
        layer: &[usize],
        mut bary: Vec<(usize, f64)>,
        before: &mut Vec<(usize, usize)>,
    ) -> Vec<usize> {
        bary.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut items: Vec<usize> = bary.iter().map(|&(v, _)| v).collect();
        let mut keys: Vec<f64> = bary.iter().map(|&(_, k)| k).collect();
        for &v in layer.iter().filter(|&&v| self.border[v]) {
            items.push(v);
            keys.push(0.0);
        }
        self.ctx.arrange(&items, &keys, before)
    }

    /// `calculatePortRanks`: ranks for the ports of layer `li` on its side towards
    /// the next layer (`east`) or the previous one.
    fn rank_ports(&mut self, li: usize, east: bool) {
        let mut sum = 0.0f64;
        for &v in &self.layers[li] {
            if self.border[v] {
                continue;
            }
            let ports = if east { &self.east[v] } else { &self.west[v] };
            let ranks = if east {
                &mut self.east_rank
            } else {
                &mut self.west_rank
            };
            // In-layer ports count by their type (output towards the next
            // layer, input from the previous one), whatever their side: an
            // output one comes after the others clockwise, an input one first.
            let extra = self.inl_of[v]
                .iter()
                .filter(|&&k| {
                    let l = &self.inl[k];
                    let output = (l.node == v) == l.before;
                    output == east
                })
                .count();
            let k = ports.len() + extra;
            if self.node_relative {
                let incr = 1.0 / (k + 1) as f64;
                if east {
                    let mut p = sum + incr;
                    for &s in ports {
                        ranks[s] = p;
                        p += incr;
                    }
                } else {
                    // Assigned bottom to top, as ELK walks the west side clockwise.
                    let mut p = sum + 1.0 - incr - extra as f64 * incr;
                    for &s in ports.iter().rev() {
                        ranks[s] = p;
                        p -= incr;
                    }
                }
                sum += 1.0;
            } else {
                for (j, &s) in ports.iter().enumerate() {
                    ranks[s] = sum + (j + 1) as f64;
                }
                sum += k as f64;
            }
        }
    }

    /// Barycentres of the layer's nodes over the ports they reach in the fixed
    /// layer (whose ranks are set), as `BarycenterHeuristic.minimizeCrossings`.
    fn barycenters(&mut self, layer: &[usize], forward: bool, first: bool) -> Vec<(usize, f64)> {
        let mut bary: Vec<(usize, Option<f64>)> = Vec::with_capacity(layer.len());
        if self.inl.is_empty() {
            for &v in layer {
                if self.border[v] {
                    continue;
                }
                // ELK walks a node's ports clockwise: the west side bottom to top.
                let (sum, degree) = self.own_weight(v, forward);
                let b = if degree > 0 {
                    let sum = sum + self.rng.next_float() * RANDOM_AMOUNT - RANDOM_AMOUNT / 2.0;
                    Some(sum / degree as f64)
                } else {
                    None
                };
                bary.push((v, b));
            }
        } else {
            // An in-layer edge adds the weight of the node at its other end (with
            // that node's random perturbation), computed first where it is not yet.
            let mut state: Vec<Option<(f64, usize)>> = vec![None; self.border.len()];
            for &v in layer {
                if !self.border[v] {
                    self.weigh(v, forward, &mut state);
                }
            }
            for &v in layer {
                if self.border[v] {
                    continue;
                }
                let b = state[v].filter(|w| w.1 > 0).map(|(s, d)| s / d as f64);
                bary.push((v, b));
            }
        }
        // `fillInUnknownBarycenters`.
        let mut out = Vec::with_capacity(bary.len());
        if !first {
            let mut last = -1.0;
            for i in 0..bary.len() {
                let value = match bary[i].1 {
                    Some(b) => b,
                    None => {
                        let next = bary[i + 1..].iter().find_map(|b| b.1).unwrap_or(last + 1.0);
                        let value = (last + next) / 2.0;
                        bary[i].1 = Some(value);
                        value
                    }
                };
                last = value;
                out.push((bary[i].0, value));
            }
        } else {
            let max = bary.iter().filter_map(|b| b.1).fold(0.0, f64::max) + 2.0;
            for (v, b) in bary {
                let value = match b {
                    Some(b) => b,
                    None => self.rng.next_float() * max - 1.0,
                };
                out.push((v, value));
            }
        }
        out
    }

    /// The summed ranks and the number of the ports `v`'s edges reach in the
    /// fixed layer (before or after it).
    fn own_weight(&self, v: usize, forward: bool) -> (f64, usize) {
        if forward {
            self.west[v]
                .iter()
                .rev()
                .fold((0.0, 0usize), |(s, d), &p| (s + self.east_rank[p], d + 1))
        } else {
            self.east[v]
                .iter()
                .fold((0.0, 0usize), |(s, d), &p| (s + self.west_rank[p], d + 1))
        }
    }

    /// `BarycenterHeuristic.calculateBarycenter` with in-layer edges: `state`
    /// holds each visited vertex's summed weight and degree.
    fn weigh(&mut self, v: usize, forward: bool, state: &mut [Option<(f64, usize)>]) {
        if state[v].is_some() {
            return;
        }
        state[v] = Some((0.0, 0));
        // Clockwise: the in-layer ports on the next layer's side, then the
        // ports to the fixed layer, then the in-layer ones on the previous side.
        // Walking forward, a node's predecessors count (it is the in-layer
        // edge's target); backward its successors (it is the source).
        let partners = |before: bool| -> Vec<usize> {
            self.inl_of[v]
                .iter()
                .map(|&k| self.inl[k])
                .filter(|l| l.before == before && ((l.node == v) != l.before) == forward)
                .map(|l| if l.node == v { l.dummy } else { l.node })
                .collect()
        };
        let (east_side, west_side) = (partners(false), partners(true));
        let (mut sum, mut degree) = (0.0, 0usize);
        for other in east_side {
            self.weigh(other, forward, state);
            let (s, d) = state[other].unwrap_or_default();
            sum += s;
            degree += d;
        }
        let (s, d) = self.own_weight(v, forward);
        sum += s;
        degree += d;
        for other in west_side {
            self.weigh(other, forward, state);
            let (s, d) = state[other].unwrap_or_default();
            sum += s;
            degree += d;
        }
        if degree > 0 {
            sum += self.rng.next_float() * RANDOM_AMOUNT - RANDOM_AMOUNT / 2.0;
        }
        state[v] = Some((sum, degree));
    }

    /// `distributePortsWhileSweeping`: sort the free layer's ports facing the
    /// fixed layer by the ranks they reach, then the fixed layer's ports facing
    /// the free layer likewise.
    fn distribute_ports(&mut self, free: usize, fixed: usize, forward: bool) {
        self.rank_ports(fixed, forward);
        for i in 0..self.layers[free].len() {
            let v = self.layers[free][i];
            if self.fixed_node[v] {
                continue;
            }
            if forward {
                let mut ports = std::mem::take(&mut self.west[v]);
                ports.sort_by(|&a, &b| self.east_rank[a].total_cmp(&self.east_rank[b]));
                self.west[v] = ports;
            } else {
                let mut ports = std::mem::take(&mut self.east[v]);
                ports.sort_by(|&a, &b| self.west_rank[a].total_cmp(&self.west_rank[b]));
                self.east[v] = ports;
            }
        }
        self.rank_ports(free, !forward);
        for i in 0..self.layers[fixed].len() {
            let v = self.layers[fixed][i];
            // A container's ports on the fixed side are its contents' to order.
            if self.fixed_node[v] || self.nested[v].is_some() {
                continue;
            }
            if forward {
                let mut ports = std::mem::take(&mut self.east[v]);
                ports.sort_by(|&a, &b| self.west_rank[a].total_cmp(&self.west_rank[b]));
                self.east[v] = ports;
            } else {
                let mut ports = std::mem::take(&mut self.west[v]);
                ports.sort_by(|&a, &b| self.east_rank[a].total_cmp(&self.east_rank[b]));
                self.west[v] = ports;
            }
        }
    }

    /// Crossings between consecutive layers, counted between ports
    /// (`CrossingsCounter.countCrossingsBetweenLayers`).
    fn crossings(&self) -> usize {
        if !self.inl.is_empty() {
            return self.crossings_with_in_layer_edges();
        }
        let mut total = 0;
        let mut west_pos = vec![0usize; self.segs.len()];
        let mut tree: Vec<usize> = Vec::new();
        for li in 0..self.layers.len() - 1 {
            let mut k = 0;
            for &v in &self.layers[li + 1] {
                for &s in &self.west[v] {
                    west_pos[s] = k;
                    k += 1;
                }
            }
            tree.clear();
            tree.resize(k + 1, 0);
            let mut inserted = 0;
            for &v in &self.layers[li] {
                for &s in &self.east[v] {
                    // Segments already inserted that end below this one cross it.
                    let p = west_pos[s] + 1;
                    let mut i = p;
                    let mut at_or_above = 0;
                    while i > 0 {
                        at_or_above += tree[i];
                        i -= i & i.wrapping_neg();
                    }
                    total += inserted - at_or_above;
                    let mut i = p;
                    while i <= k {
                        tree[i] += 1;
                        i += i & i.wrapping_neg();
                    }
                    inserted += 1;
                }
            }
        }
        total
    }

    /// The ports of `v` on its side towards the next layer (`east`) or the
    /// previous one, top to bottom, its in-layer ports among them.
    fn side_ports(&self, v: usize, east: bool) -> Vec<Port> {
        let segs = if east { &self.east[v] } else { &self.west[v] };
        let mut out: Vec<Port> = segs
            .iter()
            .map(|&s| if east { Port::East(s) } else { Port::West(s) })
            .collect();
        for &k in &self.inl_of[v] {
            let l = &self.inl[k];
            if l.before == east {
                continue;
            }
            let node_end = l.node == v;
            let at = if node_end {
                self.inl_pos[k]
            } else {
                usize::MAX
            };
            out.insert(at.min(out.len()), Port::InLayer(k, node_end));
        }
        out
    }

    /// `AllCrossingsCounter.countAllCrossings` where in-layer edges take part:
    /// between two layers the ports of both form a circle (the left layer's
    /// east sides top down, the right layer's west sides bottom up), every
    /// edge between them a chord, the in-layer edges on those sides included;
    /// on the outer sides of the first and last layers, in-layer edges cross
    /// each other and the edges leaving there.
    fn crossings_with_in_layer_edges(&self) -> usize {
        let ns = self.segs.len();
        let index = |p: Port| match p {
            Port::East(s) => s,
            Port::West(s) => ns + s,
            Port::InLayer(k, node_end) => 2 * ns + 2 * k + usize::from(node_end),
        };
        let other = |p: Port| match p {
            Port::East(s) => Port::West(s),
            Port::West(s) => Port::East(s),
            Port::InLayer(k, node_end) => Port::InLayer(k, !node_end),
        };
        let mut pos = vec![usize::MAX; 2 * ns + 2 * self.inl.len()];
        let mut total = 0;
        let nl = self.layers.len();
        // Ports in order; `between`: counting on one layer's outer side.
        let mut count = |ports: &[Port], between: bool| {
            for (i, &p) in ports.iter().enumerate() {
                pos[index(p)] = i;
            }
            let mut tree = Fenwick::new(ports.len());
            let mut ends = Vec::new();
            for (i, &p) in ports.iter().enumerate() {
                tree.remove_all(i);
                let q = other(p);
                let in_layer = matches!(p, Port::InLayer(..));
                if between && !in_layer {
                    total += tree.size;
                    continue;
                }
                let end = pos[index(q)];
                if end != usize::MAX && end > i {
                    total += tree.rank(end);
                    ends.push(end);
                }
                for e in ends.drain(..) {
                    tree.add(e);
                }
            }
            for &p in ports {
                pos[index(p)] = usize::MAX;
            }
        };
        if nl > 0 {
            let first: Vec<Port> = self.layers[0]
                .iter()
                .flat_map(|&v| self.side_ports(v, false))
                .collect();
            count(&first, true);
            let last: Vec<Port> = self.layers[nl - 1]
                .iter()
                .flat_map(|&v| self.side_ports(v, true))
                .collect();
            count(&last, true);
        }
        for li in 0..nl.saturating_sub(1) {
            let mut ports: Vec<Port> = self.layers[li]
                .iter()
                .flat_map(|&v| self.side_ports(v, true))
                .collect();
            for &v in self.layers[li + 1].iter().rev() {
                ports.extend(self.side_ports(v, false).into_iter().rev());
            }
            count(&ports, false);
        }
        total
    }

    /// ELK's two-sided greedy switch (`GreedySwitchHeuristic` in
    /// `minimizeCrossingsNoCounter`): sweeping back and forth, swap neighbours of a
    /// layer while that removes crossings with both adjacent layers, until no swap
    /// helps. Only nodes of the same cluster swap, so clusters stay contiguous.
    fn greedy_switch(&mut self) {
        let mut forward = self.rng.next_boolean();
        let n = self.layers.len();
        let mut pos = vec![0usize; self.border.len()];
        let mut improved = true;
        let mut rounds = 0;
        while improved && rounds < 64 {
            rounds += 1;
            let order: Vec<usize> = if forward {
                (0..n).collect()
            } else {
                (0..n).rev().collect()
            };
            improved = false;
            for (k, li) in order.into_iter().enumerate() {
                for layer in &self.layers {
                    for (i, &v) in layer.iter().enumerate() {
                        pos[v] = i;
                    }
                }
                // The start layer gets one pass, the others are switched until
                // nothing improves.
                loop {
                    let switched = self.switch_pass(li, &mut pos);
                    improved |= switched;
                    if !switched || k == 0 {
                        break;
                    }
                }
            }
            forward = !forward;
        }
    }

    /// One downward pass over layer `li`; whether anything was swapped.
    fn switch_pass(&mut self, li: usize, pos: &mut [usize]) -> bool {
        let mut switched = false;
        for i in 0..self.layers[li].len().saturating_sub(1) {
            let (u, v) = (self.layers[li][i], self.layers[li][i + 1]);
            if self.border[u] || self.border[v] || self.ctx.chain(u) != self.ctx.chain(v) {
                continue;
            }
            if self.pair_crossings(u, v, pos) > self.pair_crossings(v, u, pos) {
                self.layers[li].swap(i, i + 1);
                pos[u] = i + 1;
                pos[v] = i;
                switched = true;
            }
        }
        switched
    }

    /// Crossings between the edges of `u` and of `v` (both sides) with `u` above `v`.
    fn pair_crossings(&self, u: usize, v: usize, pos: &[usize]) -> usize {
        let side = |ports_u: &[usize], ports_v: &[usize], end: &dyn Fn(usize) -> usize| {
            let mut c = 0;
            for &a in ports_u {
                for &b in ports_v {
                    if pos[end(a)] > pos[end(b)] {
                        c += 1;
                    }
                }
            }
            c
        };
        side(&self.west[u], &self.west[v], &|s| self.segs[s].0)
            + side(&self.east[u], &self.east[v], &|s| self.segs[s].1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, Vertex};
    use super::*;

    fn verts(n: usize) -> Vec<Vertex> {
        (0..n)
            .map(|_| Vertex {
                kind: Kind::Real,
                rank: 0,
                w: 1.0,
                h: 1.0,
                cluster: None,
            })
            .collect()
    }

    fn info(segs: &[(usize, usize)]) -> Vec<SegInfo> {
        segs.iter()
            .enumerate()
            .map(|(edge, &(_, target))| SegInfo {
                edge,
                target,
                reversed: false,
            })
            .collect()
    }

    /// Crossings of `layers` as ELK counts them (ports in segment order).
    fn count(layers: &[Vec<usize>], segs: &[(usize, usize)]) -> usize {
        let mut pos = vec![0; segs.iter().map(|s| s.0.max(s.1)).max().unwrap_or(0) + 1];
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos[v] = i;
            }
        }
        let mut c = 0;
        for (i, a) in segs.iter().enumerate() {
            for b in &segs[i + 1..] {
                let same_layer = layers.iter().any(|l| l.contains(&a.0) && l.contains(&b.0));
                if same_layer
                    && (pos[a.0] as i64 - pos[b.0] as i64) * (pos[a.1] as i64 - pos[b.1] as i64) < 0
                {
                    c += 1;
                }
            }
        }
        c
    }

    fn run(layers: Vec<Vec<usize>>, segs: &[(usize, usize)], opts: Options) -> Vec<Vec<usize>> {
        let n: usize = layers.iter().map(Vec::len).sum();
        let vs = verts(n);
        let ctx = OrderCtx {
            verts: &vs,
            chains: &[],
        };
        let border = vec![false; n];
        let label = vec![false; n];
        let info = info(segs);
        let graph = Graph {
            border: &border,
            label: &label,
            segs,
            info: &info,
            real: n,
            inl: &[],
        };
        minimize(layers, &graph, &ctx, opts)
    }

    const OPTS: Options = Options {
        model_order: true,
        greedy_switch: false,
        thoroughness: 7,
    };

    #[test]
    fn unconnected_ports_of_a_container_without_others_are_kept_aside() {
        // Root: the container (0) alone.
        let root_v = verts(1);
        let root_ctx = OrderCtx {
            verts: &root_v,
            chains: &[],
        };
        // Inside: p (0) -> q (1), and two external port dummies (2, 3) of edges
        // to the container itself: no ELK nodes, but still in their layers.
        let segs = [(0, 1)];
        let child_info = info(&segs);
        let child_v = verts(4);
        let child_ctx = OrderCtx {
            verts: &child_v,
            chains: &[],
        };
        let no = [false; 4];
        let graphs = vec![
            RunGraph {
                layers: vec![vec![0]],
                graph: Graph {
                    border: &no[..1],
                    label: &no[..1],
                    segs: &[],
                    info: &[],
                    real: 1,
                    inl: &[],
                },
                ctx: &root_ctx,
                opts: OPTS,
                ext_key: vec![None],
                seg_key: vec![],
                parent: None,
            },
            RunGraph {
                layers: vec![vec![2], vec![0], vec![3, 1]],
                graph: Graph {
                    border: &no,
                    label: &no,
                    segs: &segs,
                    info: &child_info,
                    real: 2,
                    inl: &[],
                },
                ctx: &child_ctx,
                opts: OPTS,
                ext_key: vec![None, None, Some(5), Some(6)],
                seg_key: vec![3],
                parent: Some((0, 0)),
            },
        ];
        let out = minimize_run(graphs);
        assert_eq!(out[1], vec![vec![2], vec![0], vec![1, 3]]);
    }

    /// An inverted port's in-layer edge (from a's port towards the previous
    /// layer to its dummy d) crosses the edge into b where b lies between: the
    /// sweeps keep d next to a, and the dummy starts at the end of its layer.
    #[test]
    fn in_layer_edges_count_as_crossings() {
        // p (0), q (1); a (2), b (3), t (4) real; d (5) the dummy.
        let segs = [(0, 2), (1, 3), (5, 4)];
        let nv = 6;
        let vs = verts(nv);
        let ctx = OrderCtx {
            verts: &vs,
            chains: &[],
        };
        let no = vec![false; nv];
        let info = info(&segs);
        let inl = [InLayer {
            edge: 2,
            node: 2,
            dummy: 5,
            before: true,
        }];
        let graph = Graph {
            border: &no,
            label: &no,
            segs: &segs,
            info: &info,
            real: 5,
            inl: &inl,
        };
        let mut sweeper_layers = vec![vec![0, 1], vec![5, 2, 3], vec![4]];
        let mut east = vec![Vec::new(); nv];
        for (s, &(a, _)) in segs.iter().enumerate() {
            east[a].push(s);
        }
        creation_order(&mut sweeper_layers, &east, &segs, &info, 5, &no, &inl);
        assert_eq!(sweeper_layers[1], vec![2, 3, 5]);
        for opts in [
            OPTS,
            Options {
                model_order: false,
                ..OPTS
            },
        ] {
            let out = minimize(sweeper_layers.clone(), &graph, &ctx, opts);
            let at = |v: usize| out[1].iter().position(|&u| u == v).unwrap_or(9);
            assert_eq!(at(2).abs_diff(at(5)), 1, "{out:?}");
        }
    }

    #[test]
    fn untangles_a_crossing() {
        // 0 -> 3 and 1 -> 2 cross in model order.
        let segs = [(0, 3), (1, 2)];
        let out = run(vec![vec![0, 1], vec![2, 3]], &segs, OPTS);
        assert_eq!(count(&out, &segs), 0);
        let out = run(
            vec![vec![0, 1], vec![2, 3]],
            &segs,
            Options {
                model_order: false,
                ..OPTS
            },
        );
        assert_eq!(count(&out, &segs), 0);
    }

    #[test]
    fn keeps_a_crossing_free_model_order() {
        let segs = [(0, 2), (1, 3), (2, 4), (3, 5)];
        let layers = vec![vec![0, 1], vec![2, 3], vec![4, 5]];
        assert_eq!(run(layers.clone(), &segs, OPTS), layers);
    }

    #[test]
    fn reduces_crossings_of_a_tangled_graph() {
        // Three layers of five, every node of a layer wired to a scrambled one.
        let mut segs = Vec::new();
        for (i, j) in [(0, 3), (1, 0), (2, 4), (3, 1), (4, 2)] {
            segs.push((i, 5 + j));
            segs.push((5 + i, 10 + (j + 2) % 5));
        }
        let layers: Vec<Vec<usize>> = (0..3).map(|l| (l * 5..l * 5 + 5).collect()).collect();
        let before = count(&layers, &segs);
        for greedy_switch in [false, true] {
            let out = run(
                layers.clone(),
                &segs,
                Options {
                    greedy_switch,
                    ..OPTS
                },
            );
            for (a, b) in out.iter().zip(&layers) {
                let (mut a, mut b) = (a.clone(), b.clone());
                a.sort_unstable();
                b.sort_unstable();
                assert_eq!(a, b, "a permutation of each layer");
            }
            assert!(count(&out, &segs) < before);
        }
    }

    #[test]
    fn model_order_sort_puts_dummies_by_their_source() {
        // Layer 0: real 0, 1. Layer 1: real 2 (from 1), dummy 3 (from 0).
        let segs = [(1, 2), (0, 3)];
        let info = info(&segs);
        let mut west = vec![Vec::new(), Vec::new(), vec![0], vec![1]];
        let pos = [0, 1, usize::MAX, usize::MAX];
        let port_at = [0, 0];
        let mut layer = vec![2, 3];
        model_order_sort(
            &mut layer,
            &west,
            &segs,
            &info,
            3,
            &pos,
            &port_at,
            (&[], &[]),
        );
        assert_eq!(layer, vec![3, 2]);
        // Real nodes keep their model order whatever their sources.
        west[3].clear();
        let mut layer = vec![2, 0, 1];
        let pos = [usize::MAX, usize::MAX, usize::MAX, usize::MAX];
        model_order_sort(
            &mut layer,
            &west,
            &segs,
            &info,
            3,
            &pos,
            &port_at,
            (&[], &[]),
        );
        assert_eq!(layer, vec![0, 1, 2]);
    }

    #[test]
    fn wide_layers_sort_like_narrow_ones_where_the_rules_agree() {
        for n in [40, EXACT_MODEL_ORDER_SORT + 72] {
            // Real nodes in reverse, without edges from the layer before.
            let mut layer: Vec<usize> = (0..n).rev().collect();
            let west = vec![Vec::new(); n];
            let pos = vec![usize::MAX; n];
            model_order_sort(&mut layer, &west, &[], &[], n, &pos, &[], (&[], &[]));
            assert_eq!(layer, (0..n).collect::<Vec<_>>());
        }
    }

    /// A container with two ports on a side, whose paths all lead to its ports,
    /// is ordered within its parent's sweeps: the parent's port order there
    /// orders its port dummies, and its next layer follows them.
    #[test]
    fn run_dives_into_a_container() {
        // Root: x (0) feeds the container (1) twice (edges 7 and 8).
        let root_segs = [(0, 1), (0, 1)];
        let root_info = info(&root_segs);
        let root_v = verts(2);
        let root_ctx = OrderCtx {
            verts: &root_v,
            chains: &[],
        };
        // Inside: port dummies 0 (edge 8) and 1 (edge 7), then a (2) fed by 1 and
        // b (3) fed by 0: in this order the two edges cross.
        let segs = [(0, 3), (1, 2)];
        let child_info = info(&segs);
        let child_v = verts(4);
        let child_ctx = OrderCtx {
            verts: &child_v,
            chains: &[],
        };
        let no = [false; 4];
        let graphs = vec![
            RunGraph {
                layers: vec![vec![0], vec![1]],
                graph: Graph {
                    border: &no[..2],
                    label: &no[..2],
                    segs: &root_segs,
                    info: &root_info,
                    real: 2,
                    inl: &[],
                },
                ctx: &root_ctx,
                opts: OPTS,
                ext_key: vec![None; 2],
                seg_key: vec![7, 8],
                parent: None,
            },
            RunGraph {
                layers: vec![vec![0, 1], vec![2, 3]],
                graph: Graph {
                    border: &no,
                    label: &no,
                    segs: &segs,
                    info: &child_info,
                    real: 4,
                    inl: &[],
                },
                ctx: &child_ctx,
                opts: Options {
                    model_order: false,
                    ..OPTS
                },
                ext_key: vec![Some(8), Some(7), None, None],
                seg_key: vec![8, 7],
                parent: Some((0, 1)),
            },
        ];
        let out = minimize_run(graphs);
        assert_eq!(out[1], vec![vec![1, 0], vec![2, 3]]);
    }

    /// `new Random(1)` in Java.
    #[test]
    fn java_random_matches_java() {
        let mut r = JavaRandom::new(1);
        assert_eq!(r.next_long(), -4_964_420_948_893_066_024);
        let mut r = JavaRandom::new(42);
        // new Random(42).nextInt() == -1170105035
        assert_eq!(r.next(32), -1_170_105_035);
        let mut r = JavaRandom::new(0);
        // new Random(0).nextDouble() == 0.730967787376657
        assert!((r.next_double() - 0.730_967_787_376_657).abs() < 1e-15);
        let mut r = JavaRandom::new(0);
        // new Random(0).nextBoolean() == true
        assert!(r.next_boolean());
    }
}
