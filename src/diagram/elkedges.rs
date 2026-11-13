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

use crate::geom::{Point, Rect};

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
///
/// mermaid works in ELK's coordinates, which start `origin` before ours (the
/// root's padding); the routes are measured there, so that a step of exactly
/// `TERMINAL_JOG_MAX` is one as it is for mermaid and not a hair over (or under)
/// it by rounding.
pub fn straighten_edge_terminals(
    routes: &mut [Option<Vec<Point>>],
    labels: &mut [Option<Label>],
    origin: f64,
) {
    let shift = |routes: &mut [Option<Vec<Point>>], labels: &mut [Option<Label>], d: f64| {
        for p in routes.iter_mut().flatten().flatten() {
            *p = Point::new(p.x + d, p.y + d);
        }
        for l in labels.iter_mut().flatten() {
            l.center = Point::new(l.center.x + d, l.center.y + d);
        }
    };
    shift(routes, labels, origin);
    straighten_in_frame(routes, labels);
    shift(routes, labels, -origin);
}

fn straighten_in_frame(routes: &mut [Option<Vec<Point>>], labels: &mut [Option<Label>]) {
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

/// A terminal (end) label of an edge (a class cardinality), as mermaid hands it
/// to ELK (`terminalElkLabels`): at its edge's start or end, beside the port of
/// `node`, its measured size, padded past the end's marker for ELK.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Terminal {
    pub edge: usize,
    pub start: bool,
    pub node: usize,
    /// Where mermaid wants it: right of the direction of travel (`…Right`), or left.
    pub right: bool,
    pub width: f64,
    pub height: f64,
    /// Marker clearance on every side (`TERMINAL_LABEL_MARKER_CLEARANCE`, or 0).
    pub pad: f64,
}

/// ELK's `spacing.edgeLabel` (`spacing.baseValue: 40`) plus the edge's thickness:
/// the gap between an end label and its edge.
const END_LABEL_EDGE_GAP: f64 = 5.0;
/// `TERMINAL_LABEL_FRAME_CLEARANCE`.
const TERMINAL_LABEL_FRAME_CLEARANCE: f64 = 2.0;

/// The centres of the terminal labels: where ELK places them
/// (`EndLabelPreprocessor`: off the node by the edge-label spacing, beside the
/// port on the side `LabelSideSelector` (`SMART_DOWN`) picks), then slid off
/// group frames (`slideTerminalLabelsOffFrames`) and mirrored across their end
/// segment to their own side where that is free (`putTerminalLabelsOnTheirSide`).
/// `routes` are the edges' final routes, `nodes` the boxes of the nodes, `frames`
/// the groups' and `centre_labels` the edges' own labels.
pub fn place_terminal_labels(
    routes: &[Option<Vec<Point>>],
    terms: &[Terminal],
    nodes: &[Rect],
    frames: &[Rect],
    centre_labels: &[Label],
) -> Vec<Option<Point>> {
    // The end segment of each label: (far point, port), and its index.
    let end_of = |t: &Terminal| -> Option<(Point, Point, usize)> {
        let pts = routes.get(t.edge)?.as_ref()?;
        let n = pts.len();
        if n < 2 {
            return None;
        }
        let end = if t.start {
            (pts[1], pts[0], 0)
        } else {
            (pts[n - 2], pts[n - 1], n - 2)
        };
        (end.0.dist(end.1) > 1e-9).then_some(end)
    };
    // ELK: below the edge (the cross axis' positive side), off the node by the
    // edge-label spacing; the labels on one side of a node that would overlap
    // stack away from it (`RectangleStripOverlapRemover`, greedily in the order
    // of their near cross borders). ELK removes the overlaps only on the sides
    // a vertical layout's edges leave from (in a horizontal one it runs on the
    // sides across the flow, which carry none): labels beside a node's left or
    // right side stay where they are, overlapping or not.
    let geometry = |t: &Terminal| {
        let (far, port, _) = end_of(t)?;
        let d = far.minus(port);
        let vertical = d.y.abs() >= d.x.abs();
        let sign = if vertical { d.y.signum() } else { d.x.signum() };
        let (w, h) = (t.width + 2.0 * t.pad, t.height + 2.0 * t.pad);
        let (along, across) = if vertical { (h, w) } else { (w, h) };
        let cross_lo = if vertical { port.x } else { port.y } + END_LABEL_EDGE_GAP;
        Some((vertical, sign, port, along, across, cross_lo))
    };
    let mut offset = vec![0.0f64; terms.len()];
    let mut done = vec![false; terms.len()];
    for i in 0..terms.len() {
        let Some((vertical, sign, _, _, _, _)) = geometry(&terms[i]) else {
            continue;
        };
        if done[i] || !vertical {
            continue;
        }
        let mut side: Vec<usize> = (i..terms.len())
            .filter(|&j| {
                terms[j].node == terms[i].node
                    && geometry(&terms[j]).is_some_and(|g| g.0 == vertical && g.1 == sign)
            })
            .collect();
        side.sort_by(|&a, &b| {
            let lo = |k: usize| geometry(&terms[k]).map_or(0.0, |g| g.5);
            lo(a).total_cmp(&lo(b)).then(a.cmp(&b))
        });
        let mut placed: Vec<usize> = Vec::new();
        for &j in &side {
            let Some((_, _, _, along, across, lo)) = geometry(&terms[j]) else {
                continue;
            };
            let mut over: Vec<usize> = placed
                .iter()
                .copied()
                .filter(|&k| {
                    geometry(&terms[k]).is_some_and(|g| g.5 < lo + across && lo < g.5 + g.4)
                })
                .collect();
            over.sort_by(|&a, &b| offset[a].total_cmp(&offset[b]));
            let mut y = 0.0;
            for k in over {
                let h = geometry(&terms[k]).map_or(0.0, |g| g.3);
                if y < offset[k] + h && y + along > offset[k] {
                    y = offset[k] + h;
                }
            }
            offset[j] = y;
            placed.push(j);
            done[j] = true;
        }
    }
    let mut centres: Vec<Option<Point>> = terms
        .iter()
        .zip(&offset)
        .map(|(t, &off)| {
            let (vertical, sign, port, along, across, lo) = geometry(t)?;
            let near = crate::layout::ELK_END_LABEL_SPACING + off + along / 2.0;
            let cross = lo + across / 2.0;
            Some(if vertical {
                Point::new(cross, port.y + sign * near)
            } else {
                Point::new(port.x + sign * near, cross)
            })
        })
        .collect();
    let boxed = |c: Point, t: &Terminal| Label {
        center: c,
        width: t.width,
        height: t.height,
    };
    let overlaps = |a: &Label, b: &Label| {
        (a.center.x - b.center.x).abs() * 2.0 < a.width + b.width
            && (a.center.y - b.center.y).abs() * 2.0 < a.height + b.height
    };
    let hits_frame = |l: &Label, clearance: f64| {
        let l = Label {
            width: l.width + 2.0 * clearance,
            height: l.height + 2.0 * clearance,
            ..*l
        };
        frames.iter().any(|f| {
            let (a, b, c, d) = (
                Point::new(f.x, f.y),
                Point::new(f.right(), f.y),
                Point::new(f.right(), f.bottom()),
                Point::new(f.x, f.bottom()),
            );
            [(a, b), (b, c), (c, d), (d, a)]
                .iter()
                .any(|&(p, q)| segment_hits_box(p, q, &l))
        })
    };
    let blocked =
        |l: &Label, i: usize, segment: usize, clearance: f64, centres: &[Option<Point>]| {
            let t = &terms[i];
            nodes.iter().any(|n| {
                overlaps(
                    l,
                    &Label {
                        center: Point::new(n.x + n.w / 2.0, n.y + n.h / 2.0),
                        width: n.w,
                        height: n.h,
                    },
                )
            }) || centre_labels.iter().any(|c| overlaps(l, c))
                || hits_frame(l, clearance)
                || routes.iter().enumerate().any(|(ei, r)| {
                    r.as_ref().is_some_and(|pts| {
                        pts.windows(2).enumerate().any(|(k, s)| {
                            !(ei == t.edge && k == segment) && segment_hits_box(s[0], s[1], l)
                        })
                    })
                })
                || terms
                    .iter()
                    .enumerate()
                    .any(|(j, o)| j != i && centres[j].is_some_and(|c| overlaps(l, &boxed(c, o))))
        };
    // Off the frames, along the end segment.
    for (i, t) in terms.iter().enumerate() {
        let (Some(centre), Some((far, port, segment))) = (centres[i], end_of(t)) else {
            continue;
        };
        if !hits_frame(&boxed(centre, t), 0.0) {
            continue;
        }
        let length = far.dist(port);
        let dir = port.minus(far).scale(1.0 / length);
        let half = (dir.x.abs() * t.width + dir.y.abs() * t.height) / 2.0;
        let along = |c: Point| (c.x - port.x) * dir.x + (c.y - port.y) * dir.y;
        let on_segment = |c: Point| along(c) + half <= 0.0 && along(c) - half >= -length;
        let mut step = 1.0;
        while step <= length {
            let found = [step, -step]
                .iter()
                .map(|&shift| centre.plus(dir.scale(shift)))
                .find(|&moved| {
                    on_segment(moved)
                        && !blocked(
                            &boxed(moved, t),
                            i,
                            segment,
                            TERMINAL_LABEL_FRAME_CLEARANCE,
                            &centres,
                        )
                });
            if let Some(moved) = found {
                centres[i] = Some(moved);
                break;
            }
            step += 1.0;
        }
    }
    // Onto their own side of the end segment, where that is free.
    for (i, t) in terms.iter().enumerate() {
        let (Some(centre), Some((far, port, segment))) = (centres[i], end_of(t)) else {
            continue;
        };
        // The segment in the direction of travel.
        let (from, to) = if t.start { (port, far) } else { (far, port) };
        let length = from.dist(to);
        let normal = Point::new(-(to.y - from.y) / length, (to.x - from.x) / length);
        let offset = (centre.x - from.x) * normal.x + (centre.y - from.y) * normal.y;
        if offset == 0.0 || (offset > 0.0) == t.right {
            continue;
        }
        let mirrored = centre.minus(normal.scale(2.0 * offset));
        if !blocked(&boxed(mirrored, t), i, segment, 0.0, &centres) {
            centres[i] = Some(mirrored);
        }
    }
    centres
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
        straighten_edge_terminals(&mut routes, &mut labels, 0.0);
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

    /// A step of exactly 16 in ELK's coordinates (mermaid's rnd4 g045: n1 --> n4
    /// from x 518.1453125 to 534.1453125) is straightened, as mermaid does; 12
    /// before that, as we lay it out, the difference rounds to a hair over 16.
    #[test]
    fn measures_the_step_in_elk_coordinates() {
        let route = vec![
            p(506.1453125, 45.0),
            p(506.1453125, 65.0),
            p(522.1453125, 65.0),
            p(522.1453125, 150.0),
            p(542.04375, 150.0),
            p(542.04375, 190.0),
        ];
        let mut routes = vec![Some(route.clone())];
        straighten_edge_terminals(&mut routes, &mut [None], 0.0);
        assert_eq!(routes[0].as_ref(), Some(&route));
        straighten_edge_terminals(&mut routes, &mut [None], 12.0);
        let r = routes[0].as_ref().expect("route");
        assert_eq!(r.len(), 4, "{r:?}");
        assert!((r[1].x - 506.1453125).abs() < 1e-9 && (r[1].y - 150.0).abs() < 1e-9);
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
        straighten_edge_terminals(&mut routes, &mut [None], 0.0);
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
        straighten_edge_terminals(&mut routes, &mut [None, None], 0.0);
        assert_eq!(routes[0].as_ref(), Some(&small));
    }

    fn term(edge: usize, start: bool, node: usize, width: f64, pad: f64) -> Terminal {
        Terminal {
            edge,
            start,
            node,
            right: start,
            width,
            height: 16.5,
            pad,
        }
    }

    fn close(a: Option<Point>, x: f64, y: f64) -> bool {
        a.is_some_and(|a| (a.x - x).abs() < 0.01 && (a.y - y).abs() < 0.01)
    }

    /// `Customer "1" --> "*" Ticket` top to bottom (mermaid's numbers): ELK puts
    /// both labels right of the edge, 4px off the nodes (the end one padded past
    /// the arrow); mermaid then mirrors the start one (`startLabelRight`) to the
    /// right of travel, which going down is the left.
    #[test]
    fn terminal_labels_beside_their_ports_on_their_side() {
        let routes = vec![Some(vec![p(56.68, 93.0), p(56.68, 198.0)])];
        let terms = [
            term(0, true, 0, 6.125, 0.0),
            term(0, false, 1, 4.28125, 6.0),
        ];
        let nodes = [
            Rect::centered(p(56.68, 52.5), 89.36, 81.0),
            Rect::centered(p(56.68, 238.5), 64.2, 81.0),
        ];
        let at = place_terminal_labels(&routes, &terms, &nodes, &[], &[]);
        assert!(close(at[0], 48.6175, 105.25), "{at:?}");
        assert!(close(at[1], 69.820625, 179.75), "{at:?}");
        // Left to right: below the edge, the end one mirrored above it.
        let routes = vec![Some(vec![p(101.36, 52.5), p(178.0, 52.5)])];
        let nodes = [
            Rect::centered(p(56.68, 52.5), 89.36, 81.0),
            Rect::centered(p(210.1, 52.5), 64.2, 81.0),
        ];
        let at = place_terminal_labels(&routes, &terms, &nodes, &[], &[]);
        assert!(close(at[0], 108.4225, 65.75), "{at:?}");
        assert!(close(at[1], 165.859375, 33.25), "{at:?}");
    }

    /// Two start labels beside one side of a node that would overlap stack
    /// away from it, in the order of their ports (`A "1" --> B`, `A "0..1" --> C`
    /// from ports 4.7px apart).
    #[test]
    fn terminal_labels_on_one_side_stack() {
        let routes = vec![
            Some(vec![p(48.0, 93.0), p(48.0, 150.0), p(29.1, 150.0)]),
            Some(vec![p(52.7, 93.0), p(52.7, 150.0), p(71.6, 150.0)]),
        ];
        let terms = [term(0, true, 0, 6.125, 0.0), term(1, true, 0, 18.36, 0.0)];
        let nodes = [Rect::centered(p(50.3, 52.5), 34.1, 81.0)];
        let at = place_terminal_labels(&routes, &terms, &nodes, &[], &[]);
        assert!(close(at[0], 39.9375, 105.25), "{at:?}");
        assert!(close(at[1], 38.52, 121.75), "{at:?}");
    }

    /// Beside a node's left or right side (a horizontal layout) ELK leaves the
    /// labels where they are: the same pair, turned, does not stack.
    #[test]
    fn terminal_labels_beside_a_horizontal_side_do_not_stack() {
        let routes = vec![
            Some(vec![p(93.0, 48.0), p(150.0, 48.0), p(150.0, 29.1)]),
            Some(vec![p(93.0, 52.7), p(150.0, 52.7), p(150.0, 71.6)]),
        ];
        let terms = [term(0, true, 0, 6.125, 0.0), term(1, true, 0, 18.36, 0.0)];
        let nodes = [Rect::centered(p(52.5, 50.3), 81.0, 34.1)];
        let at = place_terminal_labels(&routes, &terms, &nodes, &[], &[]);
        let x = |k: usize| at[k].map_or(f64::NAN, |c| c.x);
        // Both start the edge-label spacing off the node's right side (93).
        let lefts = [x(0) - 6.125 / 2.0, x(1) - 18.36 / 2.0];
        assert!(lefts.iter().all(|l| (l - 97.0).abs() < 1e-6), "{at:?}");
    }
}
