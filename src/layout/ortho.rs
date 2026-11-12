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

impl Trunk {
    fn lo(&self) -> f64 {
        self.incoming
            .iter()
            .chain(&self.outgoing)
            .copied()
            .fold(f64::INFINITY, f64::min)
    }
    fn hi(&self) -> f64 {
        self.incoming
            .iter()
            .chain(&self.outgoing)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

impl From<Jog> for Trunk {
    fn from(j: Jog) -> Self {
        Trunk {
            incoming: vec![j.from],
            outgoing: vec![j.to],
        }
    }
}

/// `OrthogonalRoutingGenerator.countConflicts` (critical ones count as plain
/// ones): a merge walk over two sorted lists of positions.
fn conflicts(a: &[f64], b: &[f64], threshold: f64) -> i64 {
    let (Some(&a0), Some(&b0)) = (a.first(), b.first()) else {
        return 0;
    };
    let (mut i, mut j, mut p1, mut p2) = (0, 0, a0, b0);
    let mut count = 0;
    loop {
        if p1 > p2 - threshold && p1 < p2 + threshold {
            count += 1;
        }
        if p1 <= p2 && i + 1 < a.len() {
            i += 1;
            p1 = a[i];
        } else if p2 <= p1 && j + 1 < b.len() {
            j += 1;
            p2 = b[j];
        } else {
            return count;
        }
    }
}

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
/// difference; equal non-zero penalties tie both ways. Cycles are broken as
/// `HyperEdgeCycleDetector` does (a linear ordering from sinks and sources, the
/// node of largest outflow next, where ELK picks at random among ties the first
/// here), dropping tied dependencies and reversing the rest; each jog's slot is
/// then its longest dependency path. ELK splits segments in critical conflicts
/// (verticals almost on top of each other); here those are plain conflicts.
#[cfg(test)]
pub fn assign_slots_elk(jogs: &[Jog], edge_spacing: f64) -> (Vec<usize>, usize) {
    let trunks: Vec<Trunk> = jogs.iter().map(|&j| j.into()).collect();
    assign_slots_trunks(&trunks, edge_spacing)
}

/// [`assign_slots_elk`] for trunks with any number of ends on either side
/// (positions sorted ascending).
pub fn assign_slots_trunks(jogs: &[Trunk], edge_spacing: f64) -> (Vec<usize>, usize) {
    let n = jogs.len();
    let threshold = 0.5 * edge_spacing;
    // deps[a] = (b, weight): a before b.
    let mut edges: Vec<(usize, usize, i64)> = Vec::new();
    for a in 0..n {
        for b in a + 1..n {
            let (ja, jb) = (&jogs[a], &jogs[b]);
            let c1 = conflicts(&ja.outgoing, &jb.incoming, threshold);
            let c2 = conflicts(&jb.outgoing, &ja.incoming, threshold);
            let x1 = crossings(&ja.outgoing, jb.lo(), jb.hi())
                + crossings(&jb.incoming, ja.lo(), ja.hi());
            let x2 = crossings(&jb.outgoing, ja.lo(), ja.hi())
                + crossings(&ja.incoming, jb.lo(), jb.hi());
            let (d1, d2) = (c1 + 16 * x1, c2 + 16 * x2);
            if d1 < d2 {
                edges.push((a, b, d2 - d1));
            } else if d1 > d2 {
                edges.push((b, a, d1 - d2));
            } else if d1 > 0 {
                edges.push((a, b, 0));
                edges.push((b, a, 0));
            }
        }
    }
    // Linear ordering marks (`computeLinearOrderingMarks`).
    let mut inw = vec![0i64; n];
    let mut outw = vec![0i64; n];
    for &(a, b, w) in &edges {
        outw[a] += w;
        inw[b] += w;
    }
    let mut mark: Vec<i64> = (0..n).map(|i| -(i as i64) - 1).collect();
    let mut sources = std::collections::VecDeque::new();
    let mut sinks = std::collections::VecDeque::new();
    for i in 0..n {
        if outw[i] == 0 {
            sinks.push_back(i);
        } else if inw[i] == 0 {
            sources.push_back(i);
        }
    }
    let base = n as i64;
    let (mut next_sink, mut next_source) = (base - 1, base + 1);
    let mut done = vec![false; n];
    let update = |v: usize,
                  mark: &[i64],
                  inw: &mut [i64],
                  outw: &mut [i64],
                  sources: &mut std::collections::VecDeque<usize>,
                  sinks: &mut std::collections::VecDeque<usize>| {
        for &(a, b, w) in &edges {
            if a == v && mark[b] < 0 && w > 0 {
                inw[b] -= w;
                if inw[b] <= 0 && outw[b] > 0 {
                    sources.push_back(b);
                }
            }
            if b == v && mark[a] < 0 && w > 0 {
                outw[a] -= w;
                if outw[a] <= 0 && inw[a] > 0 {
                    sinks.push_back(a);
                }
            }
        }
    };
    while done.iter().any(|d| !d) {
        while let Some(v) = sinks.pop_front() {
            done[v] = true;
            mark[v] = next_sink;
            next_sink -= 1;
            update(v, &mark, &mut inw, &mut outw, &mut sources, &mut sinks);
        }
        while let Some(v) = sources.pop_front() {
            done[v] = true;
            mark[v] = next_source;
            next_source += 1;
            update(v, &mark, &mut inw, &mut outw, &mut sources, &mut sinks);
        }
        // Unprocessed in ascending (initial) mark: the last jog first.
        let mut best: Option<(i64, usize)> = None;
        for v in (0..n).rev().filter(|&v| !done[v]) {
            let flow = outw[v] - inw[v];
            if best.is_none_or(|(f, _)| flow > f) {
                best = Some((flow, v));
            }
        }
        if let Some((_, v)) = best {
            done[v] = true;
            mark[v] = next_source;
            next_source += 1;
            update(v, &mark, &mut inw, &mut outw, &mut sources, &mut sinks);
        }
    }
    for m in mark.iter_mut() {
        if *m < base {
            *m += base + 1;
        }
    }
    let mut deps: Vec<(usize, usize)> = Vec::new();
    for &(a, b, w) in &edges {
        if mark[a] > mark[b] {
            if w != 0 {
                deps.push((b, a));
            }
        } else {
            deps.push((a, b));
        }
    }
    // Topological numbering: longest path from the sources.
    let mut indeg = vec![0usize; n];
    for &(_, b) in &deps {
        indeg[b] += 1;
    }
    let mut slot = vec![0usize; n];
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    while let Some(v) = queue.pop_front() {
        for &(a, b) in &deps {
            if a == v {
                slot[b] = slot[b].max(slot[v] + 1);
                indeg[b] -= 1;
                if indeg[b] == 0 {
                    queue.push_back(b);
                }
            }
        }
    }
    let count = slot.iter().map(|l| l + 1).max().unwrap_or(0);
    (slot, count)
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
