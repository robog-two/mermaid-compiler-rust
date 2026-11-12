//! Orthogonal edge routing in the style of ELK layered's `OrthogonalRoutingGenerator`
//! (mermaid 12's default `layout: elk`).
//!
//! Everything here works in layout space (cross axis `x`, rank axis `y` growing in
//! the flow direction), on edges oriented along the flow:
//!
//! * every edge end gets its own port on the facing side of its node, the ports of
//!   one side spread evenly over it (`w · (i + 1) / (n + 1)`) in the order of the
//!   vertices they lead to, so the routes leaving one side do not cross;
//! * between two consecutive layers that hold nodes or edge labels (an "obstacle"
//!   layer; layers of plain long-edge dummies are free space), an edge whose
//!   cross-axis position changes takes one horizontal *slot*. Slots of horizontally
//!   overlapping segments are ordered to cause the fewest crossings and stacked
//!   `edge_edge` apart, at least `edge_node` clear of the layers on either side
//!   (the gap is widened when they do not fit);
//! * a long edge runs straight through its dummy positions in obstacle layers,
//!   which the layer ordering keeps clear of nodes.

use super::elkorder::JavaRandom;
use crate::geom::Point;

/// ELK's spacings for orthogonal routes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrthoSpacing {
    /// Clearance between a slot and the layers above and below it
    /// (`spacing.edgeNodeBetweenLayers`).
    pub edge_node: f64,
    /// Distance between two stacked slots (`spacing.edgeEdgeBetweenLayers`).
    pub edge_edge: f64,
}

impl OrthoSpacing {
    /// The root graph: mermaid's `spacing.baseValue: 40`, of which ELK derives both
    /// between-layer edge spacings as half.
    pub const ROOT: OrthoSpacing = OrthoSpacing {
        edge_node: 20.0,
        edge_edge: 20.0,
    };
    /// A container laid out on its own: `elk.layered.spacing.edgeNodeBetweenLayers: 30`
    /// (`createSubgraphElkNode`); its `spacing.edgeEdgeBetweenLayers` is not set, so
    /// ELK derives it from the container's `spacing.baseValue: 24` as half (mermaid's
    /// `elk.spacing.edgeEdge: 20` is the spacing within a layer).
    pub const CONTAINER: OrthoSpacing = OrthoSpacing {
        edge_node: 30.0,
        edge_edge: 12.0,
    };

    /// The gap a channel with `slots` slots needs.
    pub fn required_gap(self, slots: usize) -> f64 {
        if slots == 0 {
            0.0
        } else {
            2.0 * self.edge_node + (slots - 1) as f64 * self.edge_edge
        }
    }
}

/// How edges are routed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EdgeRouting {
    /// Through the long-edge dummy vertices, node centre to node centre (dagre; the
    /// caller draws a spline through them).
    #[default]
    Splines,
    /// Right-angled routes from port to port (ELK).
    Orthogonal(OrthoSpacing),
}

/// Cross-axis offsets of `n` ports spread over a side of length `len`, as ELK
/// distributes the ports of one node side.
pub fn port_offsets(n: usize, len: f64) -> Vec<f64> {
    (0..n)
        .map(|i| len * (i + 1) as f64 / (n + 1) as f64 - len / 2.0)
        .collect()
}

/// One horizontal jog within a channel: from cross-axis `from` (where the edge
/// comes down) to `to` (where it continues down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jog {
    pub from: f64,
    pub to: f64,
}

/// A hyperedge segment of one channel in general: the cross-axis positions
/// where it meets the layer before (`incoming`) and the layer after
/// (`outgoing`). An in-layer edge has both its ends on one of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trunk {
    pub incoming: Vec<f64>,
    pub outgoing: Vec<f64>,
}

impl From<Jog> for Trunk {
    fn from(j: Jog) -> Self {
        Trunk {
            incoming: vec![j.from],
            outgoing: vec![j.to],
        }
    }
}

/// `OrthogonalRoutingGenerator.TOLERANCE`: ends of a segment closer than this
/// across are straight, no jog.
pub const TOLERANCE: f64 = 1e-3;

/// `OrthogonalRoutingGenerator.countCrossings`: the positions within `[lo, hi]`.
fn crossings(posis: &[f64], lo: f64, hi: f64) -> i64 {
    posis.iter().filter(|&&p| p >= lo && p <= hi).count() as i64
}

/// ELK's routing-slot assignment for one channel (`OrthogonalRoutingGenerator`),
/// with every jog a hyperedge segment of its own (mermaid has ELK give each edge
/// its own ports). Returns the slot of each jog (0 = nearest the upper layer)
/// and the number of slots.
///
/// Only jogs that would cross or nearly share a vertical run depend on each
/// other: for each pair, the order with the lower penalty (a crossing weighs 16,
/// two verticals closer than half the edge spacing 1) is preferred by the
/// difference; equal non-zero penalties tie both ways; verticals closer than a
/// fifth of the least distance between two ends conflict critically. Cycles are
/// broken as `HyperEdgeCycleDetector` does (a linear ordering from sinks and
/// sources, the node of largest outflow next, picked among ties with the
/// graph's random generator as ELK does), critical ones first by splitting a
/// jog in two, the others by dropping tied dependencies and reversing the
/// rest; each jog's slot is then its longest dependency path. The jogs must be
/// in ELK's order: by their upper ends, then in-layer edges on the lower layer
/// by theirs (see [`assign_slots_trunks`]).
#[cfg(test)]
pub fn assign_slots_elk(jogs: &[Jog], edge_spacing: f64) -> (Vec<usize>, usize) {
    let trunks: Vec<Trunk> = jogs.iter().map(|&j| j.into()).collect();
    let out = assign_slots_trunks(&trunks, edge_spacing, &mut JavaRandom::new(1));
    (out.slot, out.count)
}

/// What [`assign_slots_trunks`] makes of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Slots {
    /// Per trunk: its slot (0 = nearest the layer before).
    pub slot: Vec<usize>,
    /// Per trunk split in two (`HyperEdgeSegmentSplitter`, for a critical
    /// cycle): where the two parts meet across, and the second part's slot.
    pub split: Vec<Option<(f64, usize)>>,
    /// The number of slots the channel takes.
    pub count: usize,
}

/// ELK's routing-slot assignment for one channel (`OrthogonalRoutingGenerator`)
/// over trunks with any number of ends on either side (positions sorted
/// ascending), in ELK's order (see [`assign_slots_elk`]); straight trunks take
/// no slot but count for the free areas and the critical threshold. With two
/// or more critical dependencies (verticals almost on top of each other), the
/// critical cycles are broken first by splitting a trunk in two; the other
/// cycles then by `HyperEdgeCycleDetector`, which draws from `rng`.
pub fn assign_slots_trunks(jogs: &[Trunk], edge_spacing: f64, rng: &mut JavaRandom) -> Slots {
    let n = jogs.len();
    let mut ch = Channel {
        segs: jogs
            .iter()
            .map(|t| Seg::new(t.incoming.clone(), t.outgoing.clone()))
            .collect(),
        deps: Vec::new(),
        threshold: 0.5 * edge_spacing,
        critical: 0.0,
    };
    ch.critical = CRITICAL_CONFLICT_THRESHOLD_FACTOR * ch.min_horizontal_distance();
    let mut critical = 0;
    for a in 0..n.saturating_sub(1) {
        for b in a + 1..n {
            critical += ch.dependency_if_necessary(a, b);
        }
    }
    if critical >= 2 {
        let cycle = ch.detect_cycles(true, rng);
        ch.split_segments(&cycle);
    }
    for d in ch.detect_cycles(false, rng) {
        if ch.deps[d].weight == 0 {
            ch.set_ends(d, None, None);
        } else {
            let (s, t) = (ch.deps[d].src, ch.deps[d].tgt);
            ch.set_ends(d, t, s);
        }
    }
    ch.topological_numbering();
    let mut count = -1;
    for s in &ch.segs {
        if !s.straight() {
            count = count.max(s.slot);
        }
    }
    let slot_of = |i: usize| ch.segs[i].slot.max(0) as usize;
    Slots {
        slot: (0..n).map(slot_of).collect(),
        split: (0..n)
            .map(|i| {
                let p = ch.segs[i].split_partner?;
                Some((*ch.segs[i].outgoing.first()?, slot_of(p)))
            })
            .collect(),
        count: (count + 1) as usize,
    }
}

/// `OrthogonalRoutingGenerator.CRITICAL_CONFLICT_THRESHOLD_FACTOR`.
const CRITICAL_CONFLICT_THRESHOLD_FACTOR: f64 = 0.2;

/// A `HyperEdgeSegment`.
#[derive(Debug, Clone)]
struct Seg {
    incoming: Vec<f64>,
    outgoing: Vec<f64>,
    start: f64,
    end: f64,
    mark: i64,
    in_w: i64,
    out_w: i64,
    crit_in_w: i64,
    crit_out_w: i64,
    in_deps: Vec<usize>,
    out_deps: Vec<usize>,
    slot: i64,
    split_partner: Option<usize>,
    split_by: Option<usize>,
}

impl Seg {
    fn new(incoming: Vec<f64>, outgoing: Vec<f64>) -> Self {
        let mut s = Seg {
            incoming,
            outgoing,
            start: f64::NAN,
            end: f64::NAN,
            mark: 0,
            in_w: 0,
            out_w: 0,
            crit_in_w: 0,
            crit_out_w: 0,
            in_deps: Vec::new(),
            out_deps: Vec::new(),
            slot: 0,
            split_partner: None,
            split_by: None,
        };
        s.recompute_extent();
        s
    }

    fn recompute_extent(&mut self) {
        let all = self.incoming.iter().chain(&self.outgoing).copied();
        self.start = all.clone().fold(f64::NAN, f64::min);
        self.end = all.fold(f64::NAN, f64::max);
    }

    fn straight(&self) -> bool {
        (self.start - self.end).abs() < TOLERANCE
    }

    fn represents_hyperedge(&self) -> bool {
        self.incoming.len() + self.outgoing.len() > 2
    }
}

/// A `HyperEdgeSegmentDependency`.
#[derive(Debug, Clone, Copy)]
struct Dep {
    critical: bool,
    src: Option<usize>,
    tgt: Option<usize>,
    weight: i64,
}

/// The hyperedge segments of one channel and their dependencies.
struct Channel {
    segs: Vec<Seg>,
    deps: Vec<Dep>,
    threshold: f64,
    critical: f64,
}

impl Channel {
    fn add_dep(&mut self, critical: bool, src: usize, tgt: usize, weight: i64) {
        self.deps.push(Dep {
            critical,
            src: None,
            tgt: None,
            weight,
        });
        let d = self.deps.len() - 1;
        self.set_ends(d, Some(src), Some(tgt));
    }

    /// `setSource` then `setTarget`: off the old ends' lists, onto the new ones'.
    fn set_ends(&mut self, d: usize, src: Option<usize>, tgt: Option<usize>) {
        if let Some(s) = self.deps[d].src {
            self.segs[s].out_deps.retain(|&x| x != d);
        }
        self.deps[d].src = src;
        if let Some(s) = src {
            self.segs[s].out_deps.push(d);
        }
        if let Some(t) = self.deps[d].tgt {
            self.segs[t].in_deps.retain(|&x| x != d);
        }
        self.deps[d].tgt = tgt;
        if let Some(t) = tgt {
            self.segs[t].in_deps.push(d);
        }
    }

    /// `minimumHorizontalSegmentDistance`.
    fn min_horizontal_distance(&self) -> f64 {
        let min_diff = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.dedup();
            v.windows(2).map(|w| w[1] - w[0]).fold(f64::MAX, f64::min)
        };
        let inc = self
            .segs
            .iter()
            .flat_map(|s| s.incoming.iter().copied())
            .collect();
        let out = self
            .segs
            .iter()
            .flat_map(|s| s.outgoing.iter().copied())
            .collect();
        min_diff(inc).min(min_diff(out))
    }

    /// `countConflicts`: `None` for a critical conflict.
    fn conflicts(&self, a: &[f64], b: &[f64]) -> Option<i64> {
        let (Some(&a0), Some(&b0)) = (a.first(), b.first()) else {
            return Some(0);
        };
        let (mut i, mut j, mut p1, mut p2) = (0, 0, a0, b0);
        let mut count = 0;
        loop {
            if p1 > p2 - self.critical && p1 < p2 + self.critical {
                return None;
            } else if p1 > p2 - self.threshold && p1 < p2 + self.threshold {
                count += 1;
            }
            if p1 <= p2 && i + 1 < a.len() {
                i += 1;
                p1 = a[i];
            } else if p2 <= p1 && j + 1 < b.len() {
                j += 1;
                p2 = b[j];
            } else {
                return Some(count);
            }
        }
    }

    /// `createDependencyIfNecessary`: the number of critical dependencies added.
    fn dependency_if_necessary(&mut self, a: usize, b: usize) -> usize {
        let (sa, sb) = (&self.segs[a], &self.segs[b]);
        if sa.straight() || sb.straight() {
            return 0;
        }
        let c1 = self.conflicts(&sa.outgoing, &sb.incoming);
        let c2 = self.conflicts(&sb.outgoing, &sa.incoming);
        if c1.is_none() || c2.is_none() {
            let mut added = 0;
            if c1.is_none() {
                self.add_dep(true, b, a, 1);
                added += 1;
            }
            if c2.is_none() {
                self.add_dep(true, a, b, 1);
                added += 1;
            }
            return added;
        }
        let x1 =
            crossings(&sa.outgoing, sb.start, sb.end) + crossings(&sb.incoming, sa.start, sa.end);
        let x2 =
            crossings(&sb.outgoing, sa.start, sa.end) + crossings(&sa.incoming, sb.start, sb.end);
        let d1 = c1.unwrap_or(0) + 16 * x1;
        let d2 = c2.unwrap_or(0) + 16 * x2;
        if d1 < d2 {
            self.add_dep(false, a, b, d2 - d1);
        } else if d1 > d2 {
            self.add_dep(false, b, a, d1 - d2);
        } else if d1 > 0 && d2 > 0 {
            self.add_dep(false, a, b, 0);
            self.add_dep(false, b, a, 0);
        }
        0
    }

    fn counts(&self, d: usize, critical_only: bool) -> bool {
        !critical_only || self.deps[d].critical
    }

    /// `HyperEdgeCycleDetector.detectCycles`: the dependencies to remove or
    /// reverse.
    fn detect_cycles(&mut self, critical_only: bool, rng: &mut JavaRandom) -> Vec<usize> {
        let n = self.segs.len();
        let mut sources = std::collections::VecDeque::new();
        let mut sinks = std::collections::VecDeque::new();
        for i in 0..n {
            let sum = |list: &[usize], crit: bool| -> i64 {
                list.iter()
                    .filter(|&&d| !crit || self.deps[d].critical)
                    .map(|&d| self.deps[d].weight)
                    .sum()
            };
            let (ci, co) = (
                sum(&self.segs[i].in_deps, true),
                sum(&self.segs[i].out_deps, true),
            );
            let (iw, ow) = if critical_only {
                (ci, co)
            } else {
                (
                    sum(&self.segs[i].in_deps, false),
                    sum(&self.segs[i].out_deps, false),
                )
            };
            let s = &mut self.segs[i];
            s.mark = -(i as i64) - 1;
            (s.in_w, s.out_w, s.crit_in_w, s.crit_out_w) = (iw, ow, ci, co);
            if ow == 0 {
                sinks.push_back(i);
            } else if iw == 0 {
                sources.push_back(i);
            }
        }
        let base = n as i64;
        let (mut next_sink, mut next_source) = (base - 1, base + 1);
        let mut unprocessed = vec![true; n];
        let mut left = n;
        while left > 0 {
            while let Some(v) = sinks.pop_front() {
                if std::mem::take(&mut unprocessed[v]) {
                    left -= 1;
                }
                self.segs[v].mark = next_sink;
                next_sink -= 1;
                self.update_neighbors(v, &mut sources, &mut sinks, critical_only);
            }
            while let Some(v) = sources.pop_front() {
                if std::mem::take(&mut unprocessed[v]) {
                    left -= 1;
                }
                self.segs[v].mark = next_source;
                next_source += 1;
                self.update_neighbors(v, &mut sources, &mut sinks, critical_only);
            }
            // Unprocessed in ascending mark (as they started): the last first.
            let mut max_flow = i64::MIN;
            let mut ties: Vec<usize> = Vec::new();
            for v in (0..n).rev().filter(|&v| unprocessed[v]) {
                let s = &self.segs[v];
                if !critical_only && s.crit_out_w > 0 && s.crit_in_w <= 0 {
                    ties = vec![v];
                    break;
                }
                let flow = s.out_w - s.in_w;
                if flow >= max_flow {
                    if flow > max_flow {
                        ties.clear();
                        max_flow = flow;
                    }
                    ties.push(v);
                }
            }
            if !ties.is_empty() {
                let v = ties[rng.next_int(ties.len() as i32) as usize];
                if std::mem::take(&mut unprocessed[v]) {
                    left -= 1;
                }
                self.segs[v].mark = next_source;
                next_source += 1;
                self.update_neighbors(v, &mut sources, &mut sinks, critical_only);
            }
        }
        for s in &mut self.segs {
            if s.mark < base {
                s.mark += base + 1;
            }
        }
        let mut out = Vec::new();
        for (i, s) in self.segs.iter().enumerate() {
            for &d in &s.out_deps {
                let t = self.deps[d].tgt.expect("live dependency");
                if self.counts(d, critical_only) && self.segs[i].mark > self.segs[t].mark {
                    out.push(d);
                }
            }
        }
        out
    }

    fn update_neighbors(
        &mut self,
        v: usize,
        sources: &mut std::collections::VecDeque<usize>,
        sinks: &mut std::collections::VecDeque<usize>,
        critical_only: bool,
    ) {
        for k in 0..self.segs[v].out_deps.len() {
            let d = self.segs[v].out_deps[k];
            let Dep {
                critical, weight, ..
            } = self.deps[d];
            let t = self.deps[d].tgt.expect("live dependency");
            if self.counts(d, critical_only) && self.segs[t].mark < 0 && weight > 0 {
                let s = &mut self.segs[t];
                s.in_w -= weight;
                if critical {
                    s.crit_in_w -= weight;
                }
                if s.in_w <= 0 && s.out_w > 0 {
                    sources.push_back(t);
                }
            }
        }
        for k in 0..self.segs[v].in_deps.len() {
            let d = self.segs[v].in_deps[k];
            let Dep {
                critical, weight, ..
            } = self.deps[d];
            let f = self.deps[d].src.expect("live dependency");
            if self.counts(d, critical_only) && self.segs[f].mark < 0 && weight > 0 {
                let s = &mut self.segs[f];
                s.out_w -= weight;
                if critical {
                    s.crit_out_w -= weight;
                }
                if s.out_w <= 0 && s.in_w > 0 {
                    sinks.push_back(f);
                }
            }
        }
    }

    /// `HyperEdgeSegmentSplitter.splitSegments`.
    fn split_segments(&mut self, cycle: &[usize]) {
        if cycle.is_empty() {
            return;
        }
        let mut free = self.free_areas();
        let mut to_split: Vec<usize> = Vec::new();
        for &d in cycle {
            let (s, t) = (
                self.deps[d].src.expect("dep"),
                self.deps[d].tgt.expect("dep"),
            );
            if to_split.contains(&s) || to_split.contains(&t) {
                continue;
            }
            let (split, by) =
                if self.segs[s].represents_hyperedge() && !self.segs[t].represents_hyperedge() {
                    (t, s)
                } else {
                    (s, t)
                };
            to_split.push(split);
            self.segs[split].split_by = Some(by);
        }
        to_split.sort_by(|&a, &b| {
            let len = |i: usize| self.segs[i].end - self.segs[i].start;
            len(a).total_cmp(&len(b))
        });
        for seg in to_split {
            let pos = self.split_position(seg, &mut free);
            // `splitAt`.
            let mut partner = Seg::new(vec![pos], std::mem::take(&mut self.segs[seg].outgoing));
            partner.split_partner = Some(seg);
            self.segs.push(partner);
            let p = self.segs.len() - 1;
            self.segs[seg].split_partner = Some(p);
            self.segs[seg].outgoing = vec![pos];
            self.segs[seg].recompute_extent();
            while let Some(&d) = self.segs[seg].in_deps.first() {
                self.set_ends(d, None, None);
            }
            while let Some(&d) = self.segs[seg].out_deps.first() {
                self.set_ends(d, None, None);
            }
            // `updateDependencies`.
            let by = self.segs[seg].split_by.expect("split by");
            self.add_dep(true, seg, by, 1);
            self.add_dep(true, by, p, 1);
            for other in 0..self.segs.len() {
                if other != by && other != seg && other != p {
                    self.dependency_if_necessary(other, seg);
                    self.dependency_if_necessary(other, p);
                }
            }
        }
    }

    /// `findFreeAreas`: (start, end) of the gaps between the ends.
    fn free_areas(&self) -> Vec<(f64, f64)> {
        let mut all: Vec<f64> = self
            .segs
            .iter()
            .flat_map(|s| s.incoming.iter().copied())
            .chain(self.segs.iter().flat_map(|s| s.outgoing.iter().copied()))
            .collect();
        all.sort_by(f64::total_cmp);
        let c = self.critical;
        all.windows(2)
            .filter(|w| w[1] - w[0] >= 2.0 * c)
            .map(|w| (w[0] + c, w[1] - c))
            .collect()
    }

    /// `computePositionToSplitAndUpdateFreeAreas`.
    fn split_position(&self, seg: usize, free: &mut Vec<(f64, f64)>) -> f64 {
        let s = &self.segs[seg];
        let (mut first, mut last) = (None, 0);
        for (i, &(a, b)) in free.iter().enumerate() {
            if a > s.end {
                break;
            } else if b >= s.start {
                first.get_or_insert(i);
                last = i;
            }
        }
        let Some(first) = first else {
            return (s.start + s.end) / 2.0;
        };
        let mut best = first;
        if first < last {
            let mut best_rating = self.rate_area(seg, free[first]);
            for (i, &area) in free.iter().enumerate().take(last + 1).skip(first + 1) {
                let r = self.rate_area(seg, area);
                let size = |a: (f64, f64)| a.1 - a.0;
                if r.1 < best_rating.1
                    || r.1 == best_rating.1
                        && (r.0 < best_rating.0
                            || r.0 == best_rating.0 && size(area) > size(free[best]))
                {
                    best = i;
                    best_rating = r;
                }
            }
        }
        let (a, b) = free.remove(best);
        let centre = (a + b) / 2.0;
        // `useArea`.
        let c = self.critical;
        if (b - a) / 2.0 >= c {
            let mut at = best;
            if a <= centre - c {
                free.insert(at, (a, centre - c));
                at += 1;
            }
            if centre + c <= b {
                free.insert(at, (centre + c, b));
            }
        }
        centre
    }

    /// `rateArea`: (dependencies, crossings) of splitting `seg` in `area`.
    fn rate_area(&self, seg: usize, area: (f64, f64)) -> (i64, i64) {
        let centre = (area.0 + area.1) / 2.0;
        let s = &self.segs[seg];
        let first = Seg::new(s.incoming.clone(), vec![centre]);
        let second = Seg::new(vec![centre], s.outgoing.clone());
        let single = |l: &Seg, r: &Seg| {
            crossings(&l.outgoing, r.start, r.end) + crossings(&r.incoming, l.start, l.end)
        };
        let (mut deps, mut cross) = (0, 0);
        let mut both = |a: &Seg, b: &Seg| {
            let (x1, x2) = (single(a, b), single(b, a));
            if x1 == x2 {
                if x1 > 0 {
                    deps += 2;
                    cross += x1;
                }
            } else {
                deps += 1;
                cross += x1.min(x2);
            }
        };
        let others = s
            .in_deps
            .iter()
            .map(|&d| self.deps[d].src)
            .chain(s.out_deps.iter().map(|&d| self.deps[d].tgt));
        for o in others.flatten() {
            both(&first, &self.segs[o]);
            both(&second, &self.segs[o]);
        }
        deps += 2;
        let by = &self.segs[s.split_by.expect("split by")];
        cross += single(&first, by) + single(by, &second);
        (deps, cross)
    }

    /// `topologicalNumbering`.
    fn topological_numbering(&mut self) {
        let n = self.segs.len();
        let mut sources = std::collections::VecDeque::new();
        let mut rightward = std::collections::VecDeque::new();
        for i in 0..n {
            let s = &mut self.segs[i];
            s.in_w = s.in_deps.len() as i64;
            s.out_w = s.out_deps.len() as i64;
            if s.in_w == 0 {
                sources.push_back(i);
            }
            if s.out_w == 0 && s.incoming.is_empty() {
                rightward.push_back(i);
            }
        }
        let mut max_rank = -1;
        while let Some(v) = sources.pop_front() {
            for k in 0..self.segs[v].out_deps.len() {
                let d = self.segs[v].out_deps[k];
                let t = self.deps[d].tgt.expect("live dependency");
                let slot = self.segs[v].slot + 1;
                let s = &mut self.segs[t];
                s.slot = s.slot.max(slot);
                max_rank = max_rank.max(s.slot);
                s.in_w -= 1;
                if s.in_w == 0 {
                    sources.push_back(t);
                }
            }
        }
        if max_rank > -1 {
            for &v in &rightward {
                self.segs[v].slot = max_rank;
            }
            while let Some(v) = rightward.pop_front() {
                for k in 0..self.segs[v].in_deps.len() {
                    let d = self.segs[v].in_deps[k];
                    let f = self.deps[d].src.expect("live dependency");
                    if !self.segs[f].incoming.is_empty() {
                        continue;
                    }
                    let slot = self.segs[v].slot - 1;
                    let s = &mut self.segs[f];
                    s.slot = s.slot.min(slot);
                    s.out_w -= 1;
                    if s.out_w == 0 {
                        rightward.push_back(f);
                    }
                }
            }
        }
    }
}

/// Drop repeated points and the middle of three collinear points.
pub fn simplify(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points {
        if out
            .last()
            .is_some_and(|q| (q.x - p.x).abs() < 1e-6 && (q.y - p.y).abs() < 1e-6)
        {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let cross = (b.x - a.x) * (p.y - b.y) - (b.y - a.y) * (p.x - b.x);
            let dot = (b.x - a.x) * (p.x - b.x) + (b.y - a.y) * (p.y - b.y);
            if cross.abs() < 1e-6 && dot > 0.0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// True if every segment of `points` is horizontal or vertical.
#[cfg(test)]
pub fn is_orthogonal(points: &[Point]) -> bool {
    points
        .windows(2)
        .all(|w| (w[0].x - w[1].x).abs() < 1e-6 || (w[0].y - w[1].y).abs() < 1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_spread_evenly_over_the_side() {
        assert_eq!(port_offsets(1, 100.0), vec![0.0]);
        let p = port_offsets(3, 132.0);
        assert_eq!(p, vec![-33.0, 0.0, 33.0]);
    }

    #[test]
    fn swapping_jogs_split_one_and_draw_like_elk() {
        // Two jogs trading places meet each other's verticals: two critical
        // dependencies, a critical cycle, broken by splitting one in two at
        // the free area between their ends (vert/c31, as mermaid draws it).
        let jogs: Vec<Trunk> = [(109.0, 246.0), (246.0, 109.0)]
            .iter()
            .map(|&(from, to)| Jog { from, to }.into())
            .collect();
        let mut rng = JavaRandom::new(1);
        let out = assign_slots_trunks(&jogs, 20.0, &mut rng);
        let split: Vec<_> = out.split.iter().flatten().collect();
        assert_eq!(split.len(), 1, "{out:?}");
        assert!((split[0].0 - 177.5).abs() < 1e-9, "{out:?}");
        assert_eq!(out.count, 3, "{out:?}");
        // The cycle detector drew once from the generator.
        let mut fresh = JavaRandom::new(1);
        let _ = fresh.next_int(2);
        assert_eq!(rng.next_long(), fresh.next_long());
    }

    #[test]
    fn disjoint_jogs_share_a_slot() {
        let (lv, n) = assign_slots_elk(
            &[
                Jog {
                    from: 0.0,
                    to: 50.0,
                },
                Jog {
                    from: 200.0,
                    to: 300.0,
                },
            ],
            20.0,
        );
        assert_eq!((lv, n), (vec![0, 0], 1));
    }

    #[test]
    fn parallel_jogs_are_stacked_without_crossing() {
        // Both run right; the one starting further right must turn first (above),
        // or its incoming vertical would cross the other's jog.
        let jogs = [
            Jog {
                from: 10.0,
                to: 100.0,
            },
            Jog {
                from: 20.0,
                to: 110.0,
            },
        ];
        let (lv, n) = assign_slots_elk(&jogs, 20.0);
        assert_eq!(n, 2);
        assert!(lv[1] < lv[0]);
        // Mirrored: running left, the leftmost start turns first.
        let jogs = [
            Jog {
                from: 110.0,
                to: 20.0,
            },
            Jog {
                from: 100.0,
                to: 10.0,
            },
        ];
        let (lv, _) = assign_slots_elk(&jogs, 20.0);
        assert!(lv[1] < lv[0]);
    }

    #[test]
    fn simplify_drops_collinear_and_repeated_points() {
        let p = |x, y| Point::new(x, y);
        let pts = [
            p(0.0, 0.0),
            p(0.0, 5.0),
            p(0.0, 5.0),
            p(0.0, 10.0),
            p(5.0, 10.0),
        ];
        assert_eq!(
            simplify(&pts),
            vec![p(0.0, 0.0), p(0.0, 10.0), p(5.0, 10.0)]
        );
        assert!(is_orthogonal(&pts));
        assert!(!is_orthogonal(&[p(0.0, 0.0), p(1.0, 1.0)]));
    }
}
