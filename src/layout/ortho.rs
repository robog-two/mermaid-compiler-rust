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
    /// A container laid out on its own: `elk.layered.spacing.edgeNodeBetweenLayers: 30`,
    /// `elk.spacing.edgeEdge: 20` (`createSubgraphElkNode`).
    pub const CONTAINER: OrthoSpacing = OrthoSpacing {
        edge_node: 30.0,
        edge_edge: 20.0,
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

impl Jog {
    fn lo(&self) -> f64 {
        self.from.min(self.to)
    }
    fn hi(&self) -> f64 {
        self.from.max(self.to)
    }
    fn covers(&self, x: f64) -> bool {
        x > self.lo() - 1e-6 && x < self.hi() + 1e-6
    }
}

/// Assign every jog of one channel a slot level (0 = nearest the upper layer) and
/// return the levels and the number of slots used.
///
/// Two jogs conflict when their spans come within `gap` of each other. For each
/// conflicting pair the order with fewer crossings is preferred (a jog placed above
/// another crosses that one's incoming vertical if it spans the other's `from`, and
/// is crossed by the other on its own outgoing vertical); the preferences are turned
/// into a total order greedily (fewest unmet preferences first, which also breaks
/// cycles), and each jog then takes the lowest level below every conflicting jog
/// ordered before it.
pub fn assign_slots(jogs: &[Jog], gap: f64) -> (Vec<usize>, usize) {
    let n = jogs.len();
    let conflict = |a: &Jog, b: &Jog| a.lo() < b.hi() + gap && b.lo() < a.hi() + gap;
    // prefer[a][b]: a should be above b.
    let mut prefer = vec![vec![false; n]; n];
    let mut conflicts = vec![vec![false; n]; n];
    for a in 0..n {
        for b in a + 1..n {
            let (ja, jb) = (&jogs[a], &jogs[b]);
            if !conflict(ja, jb) {
                continue;
            }
            conflicts[a][b] = true;
            conflicts[b][a] = true;
            let a_above = usize::from(ja.covers(jb.from)) + usize::from(jb.covers(ja.to));
            let b_above = usize::from(jb.covers(ja.from)) + usize::from(ja.covers(jb.to));
            if a_above < b_above {
                prefer[a][b] = true;
            } else if b_above < a_above {
                prefer[b][a] = true;
            }
        }
    }
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for _ in 0..n {
        let unmet = |j: usize| (0..n).filter(|&i| !placed[i] && prefer[i][j]).count();
        let Some(next) = (0..n)
            .filter(|&j| !placed[j])
            .min_by_key(|&j| (unmet(j), j))
        else {
            break;
        };
        placed[next] = true;
        order.push(next);
    }
    let mut level = vec![0usize; n];
    for (k, &j) in order.iter().enumerate() {
        level[j] = order[..k]
            .iter()
            .filter(|&&i| conflicts[i][j])
            .map(|&i| level[i] + 1)
            .max()
            .unwrap_or(0);
    }
    let count = level.iter().map(|l| l + 1).max().unwrap_or(0);
    (level, count)
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
        let (lv, n) = assign_slots(
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
            10.0,
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
        let (lv, n) = assign_slots(&jogs, 10.0);
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
        let (lv, _) = assign_slots(&jogs, 10.0);
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
