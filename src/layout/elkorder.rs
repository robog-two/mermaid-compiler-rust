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
//! Clusters stay contiguous, with their order kept over the layers, through the
//! same `OrderCtx::arrange` the dagre-style sweeps use; ELK instead orders a
//! compound node's contents in its own nested graph, so the graphs only agree in
//! structure without clusters.

use super::OrderCtx;

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
    ctx: &'a OrderCtx<'a>,
}

#[derive(Clone)]
struct Snapshot {
    layers: Vec<Vec<usize>>,
    east: Vec<Vec<usize>>,
    west: Vec<Vec<usize>>,
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
}

/// Order the layers as ELK's layer sweep does. `layers` holds ELK's layers.
pub(super) fn minimize(
    mut layers: Vec<Vec<usize>>,
    graph: &Graph,
    ctx: &OrderCtx,
    opts: Options,
) -> Vec<Vec<usize>> {
    let Graph {
        border,
        label,
        segs,
        info,
        real,
    } = *graph;
    let nv = border.len();
    let mut east = vec![Vec::new(); nv];
    let mut west = vec![Vec::new(); nv];
    for (s, &(a, b)) in segs.iter().enumerate() {
        east[a].push(s);
        west[b].push(s);
    }
    creation_order(&mut layers, &east, segs, info, real, label);
    if opts.model_order {
        sort_by_model_order(&mut layers, &mut east, &mut west, segs, info, real, ctx);
    } else {
        for layer in layers.iter_mut() {
            let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
            *layer = ctx.arrange(layer, &keys, &mut Vec::new());
        }
        let mut pos = vec![0usize; nv];
        for layer in &layers {
            for (i, &v) in layer.iter().enumerate() {
                pos[v] = i;
            }
        }
        for w in west.iter_mut() {
            w.sort_by_key(|&s| (pos[segs[s].0], s));
        }
    }
    let mut rng = JavaRandom::new(1);
    // `LayerSweepCrossingMinimizer.initialize`: the seed every run restarts from,
    // then the graph's port distributor, by a coin flip.
    let seed = rng.next_long();
    let node_relative = rng.next_boolean();
    let mut sw = Sweeper {
        layers,
        border,
        segs,
        east,
        west,
        east_rank: vec![0.0; segs.len()],
        west_rank: vec![0.0; segs.len()],
        node_relative,
        rng,
        ctx,
    };
    if sw.layers.len() < 2 {
        return sw.layers;
    }
    sw.rng.set_seed(seed);
    let best = sw.compare_randomized_layouts(opts);
    if let Some(best) = best {
        sw.layers = best.layers;
        sw.east = best.east;
        sw.west = best.west;
    }
    if opts.greedy_switch {
        // A second `LayerSweepCrossingMinimizer` (two-sided greedy switch) on the
        // same random generator.
        let _ = sw.rng.next_long();
        sw.greedy_switch();
    }
    sw.layers
}

/// The order ELK's layers have before crossing minimisation: real nodes in model
/// order, then label dummies (made before layering) in edge order, then long-edge
/// dummies as `LongEdgeSplitter` appends them, layer by layer, node by node, edge
/// by edge.
fn creation_order(
    layers: &mut [Vec<usize>],
    east: &[Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    label: &[bool],
) {
    let nv = east.len();
    let mut edge_of = vec![usize::MAX; nv];
    for (s, &(_, b)) in segs.iter().enumerate() {
        edge_of[b] = edge_of[b].min(info[s].edge);
    }
    let mut placed = vec![false; nv];
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
            for &s in &east[u] {
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
}

/// `SortByInputModelProcessor` with `considerModelOrder: NODES_AND_EDGES`, layer
/// by layer from the first: the layer's nodes sorted (`ModelOrderNodeComparator`),
/// their ports sorted (`ModelOrderPortComparator`), the nodes sorted again. Real
/// nodes keep their model order among themselves; a dummy goes by where its edge
/// comes from in the layer before, and above a real node without an edge from
/// there (`longEdgeStrategy: DUMMY_NODE_OVER`), else by its edge's model order.
fn sort_by_model_order(
    layers: &mut [Vec<usize>],
    east: &mut [Vec<usize>],
    west: &mut [Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    ctx: &OrderCtx,
) {
    let nv = east.len();
    let mut pos = vec![usize::MAX; nv];
    // Per segment: its port's index on its source's east side.
    let mut port_at = vec![0usize; segs.len()];
    let mut before = Vec::new();
    for slot in layers.iter_mut() {
        let mut layer = std::mem::take(slot);
        model_order_sort(&mut layer, west, segs, info, real, &pos, &port_at);
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
        model_order_sort(&mut layer, west, segs, info, real, &pos, &port_at);
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
fn model_order_sort(
    layer: &mut [usize],
    west: &[Vec<usize>],
    segs: &[(usize, usize)],
    info: &[SegInfo],
    real: usize,
    pos: &[usize],
    port_at: &[usize],
) {
    use std::cmp::Ordering::{Greater, Less};
    let n = layer.len();
    if n < 2 {
        return;
    }
    let old = layer.to_vec();
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
        .map(|&v| {
            west[v]
                .last()
                .map_or(i64::from(i32::MAX), |&s| info[s].edge as i64)
        })
        .collect();
    let rule = |i: usize, j: usize| -> std::cmp::Ordering {
        let (a, b) = (old[i], old[j]);
        if a < real && b < real {
            return a.cmp(&b);
        }
        match (source[i], source[j]) {
            (Some(sa), Some(sb)) => {
                let (na, nb) = (segs[sa].0, segs[sb].0);
                if na == nb {
                    port_at[sa].cmp(&port_at[sb])
                } else {
                    pos[na].cmp(&pos[nb])
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                if order[i] > order[j] {
                    Greater
                } else {
                    Less
                }
            }
            (None, None) => Less,
        }
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
        insertion_sort(&mut slots, |i, j| rule(i, j) == Greater);
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
    let mut compare = |i: usize, j: usize| -> std::cmp::Ordering {
        if has(&bigger, i, j) {
            return Greater;
        }
        if has(&bigger, j, i) || has(&smaller, i, j) {
            return Less;
        }
        let o = rule(i, j);
        if o == Greater {
            associate(&mut bigger, &mut smaller, i, j);
        } else {
            associate(&mut bigger, &mut smaller, j, i);
        }
        o
    };
    // Sort slots, so that the sets are indexed by them.
    insertion_sort(&mut slots, |i, j| compare(i, j) == Greater);
    for (dst, &i) in layer.iter_mut().zip(&slots) {
        *dst = old[i];
    }
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
        }
    }

    /// `compareDifferentRandomizedLayouts`: the best of `thoroughness` runs, or
    /// `None` to keep the order as it is.
    fn compare_randomized_layouts(&mut self, opts: Options) -> Option<Snapshot> {
        let mut first_try = opts.model_order;
        let mut best: Option<Snapshot> = None;
        let mut best_crossings = usize::MAX;
        for _ in 0..opts.thoroughness.max(1) {
            let (crossings, run_best) = self.run(&mut first_try);
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
    fn run(&mut self, first_try: &mut bool) -> (usize, Option<Snapshot>) {
        let mut forward = self.rng.next_boolean();
        if *first_try && self.crossings() == 0 {
            // The model order is already free of crossings: keep it.
            return (0, None);
        }
        if *first_try {
            forward = true;
        } else {
            self.set_first_layer_order(forward);
        }
        self.sweep(forward, !*first_try);
        *first_try = false;
        let mut crossings = self.crossings();
        loop {
            let best = self.snapshot();
            if crossings == 0 {
                return (0, Some(best));
            }
            forward = !forward;
            let old = crossings;
            self.sweep(forward, false);
            crossings = self.crossings();
            if old <= crossings {
                return (old, Some(best));
            }
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

    /// `sweepReducingCrossings`: one sweep; `first` places nodes without a
    /// barycentre at random (the first sweep of a random run).
    fn sweep(&mut self, forward: bool, first: bool) {
        let n = self.layers.len();
        // The start layer keeps its order; its clusters' order binds the others.
        let mut before = Vec::new();
        let start = self.start(forward);
        let keys: Vec<f64> = (0..self.layers[start].len()).map(|i| i as f64).collect();
        self.ctx.arrange(&self.layers[start], &keys, &mut before);
        let free: Vec<usize> = if forward {
            (1..n).collect()
        } else {
            (0..n - 1).rev().collect()
        };
        for li in free {
            let fixed = if forward { li - 1 } else { li + 1 };
            self.rank_ports(fixed, forward);
            let layer = std::mem::take(&mut self.layers[li]);
            let bary = self.barycenters(&layer, forward, first);
            self.layers[li] = self.sorted(&layer, bary, &mut before);
            self.distribute_ports(li, fixed, forward);
        }
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
            let k = ports.len();
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
                    let mut p = sum + 1.0 - incr;
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
        for &v in layer {
            if self.border[v] {
                continue;
            }
            // ELK walks a node's ports clockwise: the west side bottom to top.
            let (sum, degree) = if forward {
                self.west[v]
                    .iter()
                    .rev()
                    .fold((0.0, 0usize), |(s, d), &p| (s + self.east_rank[p], d + 1))
            } else {
                self.east[v]
                    .iter()
                    .fold((0.0, 0usize), |(s, d), &p| (s + self.west_rank[p], d + 1))
            };
            let b = if degree > 0 {
                let sum = sum + self.rng.next_float() * RANDOM_AMOUNT - RANDOM_AMOUNT / 2.0;
                Some(sum / degree as f64)
            } else {
                None
            };
            bary.push((v, b));
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

    /// `distributePortsWhileSweeping`: sort the free layer's ports facing the
    /// fixed layer by the ranks they reach, then the fixed layer's ports facing
    /// the free layer likewise.
    fn distribute_ports(&mut self, free: usize, fixed: usize, forward: bool) {
        self.rank_ports(fixed, forward);
        for i in 0..self.layers[free].len() {
            let v = self.layers[free][i];
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
    use super::*;

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
