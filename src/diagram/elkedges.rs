//! mermaid's post-processing of ELK's edge routes (`applyElkEdgeLayout` in
//! `layout-algorithms/elk/render.ts`), on the clipped routes of every edge:
//!
//! * `straightenEdgeTerminals`: the port-to-channel step at either end of a
//!   route (ELK's jog towards an inline label, or between a port and its
//!   channel) is removed by pulling the channel onto the port's line, where
//!   that adds no crossing and no label overlap; a label riding on the moved
//!   run moves with it;
//! * `separateOppositeEdgeLabels`: the labels of the two edges between one pair
//!   of nodes, in opposite directions, are pushed apart sideways until they
//!   clear each other, so they end up beside their (now straight) edges.

use crate::geom::Point;

/// `JOG_EPS`: below this a step is no step.
const JOG_EPS: f64 = 0.01;
/// `TERMINAL_JOG_MAX`: the widest step that is straightened.
const TERMINAL_JOG_MAX: f64 = 16.0;
/// `TERMINAL_RUN_MAX`: the longest first segment the step may follow.
const TERMINAL_RUN_MAX: f64 = 30.0;
/// `OPPOSITE_LABEL_MIN_GAP`.
const OPPOSITE_LABEL_MIN_GAP: f64 = 4.0;

/// An edge label: its centre and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Label {
    pub center: Point,
    pub width: f64,
    pub height: f64,
}

/// A run moved by `straighten_front`: where it was and where it went.
#[derive(Debug, Clone, Copy)]
struct Run {
    old: (Point, Point),
    new: (Point, Point),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    H,
    V,
}

fn axis_of(a: Point, b: Point) -> Option<Axis> {
    let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
    if dx > JOG_EPS && dy <= JOG_EPS {
        Some(Axis::H)
    } else if dy > JOG_EPS && dx <= JOG_EPS {
        Some(Axis::V)
    } else {
        None
    }
}

/// `straightenFront`: straighten the step at the front of `pts`.
fn straighten_front(pts: &[Point]) -> Option<(Vec<Point>, Run)> {
    if pts.len() < 5 {
        return None;
    }
    let (p0, p1, p2, p3) = (pts[0], pts[1], pts[2], pts[3]);
    let axis = axis_of(p0, p1)?;
    let cross = match axis {
        Axis::H => Axis::V,
        Axis::V => Axis::H,
    };
    if axis_of(p2, p3) != Some(axis) || axis_of(p1, p2) != Some(cross) {
        return None;
    }
    if p0.dist(p1) > TERMINAL_RUN_MAX {
        return None;
    }
    let jog = match axis {
        Axis::H => (p2.y - p1.y).abs(),
        Axis::V => (p2.x - p1.x).abs(),
    };
    if !(JOG_EPS..=TERMINAL_JOG_MAX).contains(&jog) {
        return None;
    }
    // The route keeps going the same way after the step.
    let forward = match axis {
        Axis::H => (p1.x - p0.x).signum() == (p3.x - p2.x).signum(),
        Axis::V => (p1.y - p0.y).signum() == (p3.y - p2.y).signum(),
    };
    if !forward {
        return None;
    }
    // The whole run moves, up to where the route turns, but never the far end.
    let mut last = 3;
    while last + 1 < pts.len() && axis_of(pts[last], pts[last + 1]) == Some(axis) {
        last += 1;
    }
    if last == pts.len() - 1 {
        return None;
    }
    let mut moved = pts.to_vec();
    for i in 2..=last {
        moved[i] = match axis {
            Axis::H => Point::new(pts[i].x, p0.y),
            Axis::V => Point::new(p0.x, pts[i].y),
        };
    }
    let run = Run {
        old: (pts[2], pts[last]),
        new: (moved[2], moved[last]),
    };
    moved.drain(1..3);
    Some((moved, run))
}

/// `straightenTerminalJogsWithRuns`: both ends; `None` where nothing applies.
fn straighten_terminal_jogs(points: &[Point]) -> Option<(Vec<Point>, Vec<Run>)> {
    let mut runs = Vec::new();
    let mut pts = points.to_vec();
    if let Some((p, run)) = straighten_front(&pts) {
        pts = p;
        runs.push(run);
    }
    let mut rev: Vec<Point> = pts.iter().rev().copied().collect();
    if let Some((p, run)) = straighten_front(&rev) {
        rev = p;
        rev.reverse();
        pts = rev;
        runs.push(run);
    }
    (!runs.is_empty()).then_some((pts, runs))
}

/// Strict crossing of two segments (interior to both).
fn cross_strict(a1: Point, a2: Point, b1: Point, b2: Point) -> bool {
    let side = |o: Point, p: Point, q: Point| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    let (d1, d2) = (side(b1, b2, a1), side(b1, b2, a2));
    let (d3, d4) = (side(a1, a2, b1), side(a1, a2, b2));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn crossing_count(a: &[Point], b: &[Point]) -> usize {
    let mut n = 0;
    for sa in a.windows(2) {
        for sb in b.windows(2) {
            if cross_strict(sa[0], sa[1], sb[0], sb[1]) {
                n += 1;
            }
        }
    }
    n
}

fn param_on_segment(p: Point, a: Point, b: Point) -> f64 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let len2 = abx * abx + aby * aby;
    if len2 == 0.0 {
        return 0.0;
    }
    (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0)
}

/// `projectLabelOntoStraightenedRun`.
fn project_label(label: Point, runs: &[Run]) -> Option<Point> {
    let mut best: Option<(f64, Point)> = None;
    for run in runs {
        let t = param_on_segment(label, run.old.0, run.old.1);
        let old = run.old.0.lerp(run.old.1, t);
        let d = (old.x - label.x).powi(2) + (old.y - label.y).powi(2);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, run.new.0.lerp(run.new.1, t)));
        }
    }
    best.map(|b| b.1)
}

/// Whether segment `a`-`b` passes through the box (Liang–Barsky).
fn segment_hits_box(a: Point, b: Point, l: &Label) -> bool {
    let (x1, y1) = (l.center.x - l.width / 2.0, l.center.y - l.height / 2.0);
    let (x2, y2) = (l.center.x + l.width / 2.0, l.center.y + l.height / 2.0);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, a.x - x1),
        (dx, x2 - a.x),
        (-dy, a.y - y1),
        (dy, y2 - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    t0 < t1
}

fn covers(route: &[Point], label: Option<&Label>) -> bool {
    label.is_some_and(|l| route.windows(2).any(|s| segment_hits_box(s[0], s[1], l)))
}

/// `straightenEdgeTerminals` over every edge's route (`None`: not drawn), in
/// order: an edge is straightened only where that adds no crossing with, and no
/// label over, the others. Labels on a moved run move with it.
pub fn straighten_edge_terminals(routes: &mut [Option<Vec<Point>>], labels: &mut [Option<Label>]) {
    for i in 0..routes.len() {
        let Some(original) = routes[i].clone().filter(|r| r.len() >= 5) else {
            continue;
        };
        let Some((candidate, runs)) = straighten_terminal_jogs(&original) else {
            continue;
        };
        let label = labels[i];
        let moved = label.and_then(|l| project_label(l.center, &runs));
        let candidate_label = match (label, moved) {
            (Some(l), Some(c)) => Some(Label { center: c, ..l }),
            _ => label,
        };
        let (mut before, mut after, mut cov_before, mut cov_after) = (0, 0, 0, 0);
        for (j, route) in routes.iter().enumerate() {
            let Some(route) = route.as_ref().filter(|r| j != i && r.len() >= 2) else {
                continue;
            };
            before += crossing_count(&original, route);
            after += crossing_count(&candidate, route);
            cov_before += usize::from(covers(route, label.as_ref()))
                + usize::from(covers(&original, labels[j].as_ref()));
            cov_after += usize::from(covers(route, candidate_label.as_ref()))
                + usize::from(covers(&candidate, labels[j].as_ref()));
        }
        if after > before || cov_after > cov_before {
            continue;
        }
        routes[i] = Some(candidate);
        labels[i] = candidate_label;
    }
}

/// `separateOppositeEdgeLabels`: `pairs[i]` names edge `i`'s two ends (in
/// either order); where exactly two labelled edges join one pair and their
/// labels sit level and overlap, they are pushed apart along x.
pub fn separate_opposite_edge_labels<K: PartialEq + Copy>(
    labels: &mut [Option<Label>],
    pairs: &[K],
) {
    let mut buckets: Vec<(K, Vec<usize>)> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        if l.is_none_or(|l| l.width == 0.0) {
            continue;
        }
        match buckets.iter_mut().find(|(k, _)| *k == pairs[i]) {
            Some((_, list)) => list.push(i),
            None => buckets.push((pairs[i], vec![i])),
        }
    }
    for (_, list) in buckets {
        let [a, b] = list[..] else { continue };
        let (Some(la), Some(lb)) = (labels[a], labels[b]) else {
            continue;
        };
        if (la.center.y - lb.center.y).abs() > la.height.max(lb.height) {
            continue;
        }
        let required = la.width / 2.0 + lb.width / 2.0 + OPPOSITE_LABEL_MIN_GAP;
        let current = lb.center.x - la.center.x;
        let deficit = required - current.abs();
        if deficit <= 0.0 {
            continue;
        }
        let sign = if current == 0.0 {
            1.0
        } else {
            current.signum()
        };
        if let Some(l) = labels[a].as_mut() {
            l.center.x -= sign * deficit / 2.0;
        }
        if let Some(l) = labels[b].as_mut() {
            l.center.x += sign * deficit / 2.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// mermaid's state example: `accelerate` (Slow → Fast) and `brake` (back),
    /// each jogging to its inline label and back; both end straight, with their
    /// labels beside them (the numbers are mermaid's own).
    #[test]
    fn straightens_label_jogs_and_separates_opposite_labels() {
        let accelerate = vec![
            p(144.1, 347.0),
            p(144.1, 377.0),
            p(159.8, 377.0),
            p(159.8, 458.0),
            p(144.1, 458.0),
            p(144.1, 488.0),
        ];
        let brake = vec![
            p(105.9, 488.0),
            p(105.9, 458.0),
            p(90.2, 458.0),
            p(90.2, 377.0),
            p(105.9, 377.0),
            p(105.9, 347.0),
        ];
        let mut routes = vec![Some(accelerate), Some(brake)];
        let mut labels = vec![
            Some(Label {
                center: p(159.6, 417.5),
                width: 64.59375,
                height: 21.0,
            }),
            Some(Label {
                center: p(89.8, 417.5),
                width: 35.03125,
                height: 21.0,
            }),
        ];
        straighten_edge_terminals(&mut routes, &mut labels);
        for r in routes.iter().flatten() {
            assert!(
                r.windows(2).all(|w| (w[0].x - w[1].x).abs() < 1e-9),
                "{r:?}"
            );
        }
        let (a, b) = (labels[0].expect("label"), labels[1].expect("label"));
        assert!((a.center.x - 144.1).abs() < 0.3 && (b.center.x - 105.9).abs() < 0.3);
        separate_opposite_edge_labels(&mut labels, &[(0, 1), (0, 1)]);
        let (a, b) = (labels[0].expect("label"), labels[1].expect("label"));
        // Pushed apart by the deficit, half each way: 151.9 and 98.1 in mermaid.
        assert!((a.center.x - 151.94).abs() < 0.3, "{a:?}");
        assert!((b.center.x - 98.06).abs() < 0.3, "{b:?}");
    }

    #[test]
    fn keeps_a_step_wider_than_the_limit_or_one_that_would_cross() {
        let wide = vec![
            p(0.0, 0.0),
            p(0.0, 20.0),
            p(30.0, 20.0),
            p(30.0, 80.0),
            p(30.0, 100.0),
            p(40.0, 100.0),
        ];
        let mut routes = vec![Some(wide.clone())];
        straighten_edge_terminals(&mut routes, &mut [None]);
        assert_eq!(routes[0].as_ref(), Some(&wide));
        // A small step that, straightened, would cross a vertical edge between.
        let small = vec![
            p(0.0, 0.0),
            p(0.0, 20.0),
            p(10.0, 20.0),
            p(10.0, 80.0),
            p(40.0, 80.0),
            p(40.0, 100.0),
        ];
        let other = vec![p(-5.0, 50.0), p(5.0, 50.0)];
        let mut routes = vec![Some(small.clone()), Some(other)];
        straighten_edge_terminals(&mut routes, &mut [None, None]);
        assert_eq!(routes[0].as_ref(), Some(&small));
    }
}
