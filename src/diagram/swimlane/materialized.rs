//! `direction/materializedGeometry.ts`: the clean-up passes that run on the edge
//! geometry as it will be drawn (terminal lanes, doglegs, rails hugging nodes,
//! lane title bands, crossing reduction).

use super::geometry::*;
use super::model::{LEdge, LNode, LayoutData, Rb};
use super::sugiyama::LaneModel;
use crate::geom::Point;

pub(super) const E: f64 = 1e-3;
pub(super) const MIN_SHARED: f64 = 8.0;

pub(super) fn pts_of(e: &LEdge) -> &[Point] {
    e.points.as_deref().unwrap_or(&[])
}

pub(super) fn segs(p: &[Point]) -> Vec<Seg> {
    segments(p, GEPS)
}

fn aligned(a: Point, b: Point) -> bool {
    same_x(a, b, E) || same_y(a, b, E)
}

pub(super) fn visible(data: &LayoutData) -> Vec<usize> {
    (0..data.edges.len())
        .filter(|&i| !data.edges[i].is_layout_only)
        .collect()
}

pub(super) fn ends(e: &LEdge) -> Vec<&str> {
    [e.start.as_str(), e.end.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
}

pub(super) fn node<'a>(data: &'a LayoutData, id: &str) -> Option<&'a LNode> {
    data.nodes.iter().find(|n| n.id == id)
}

/// Points of edge `i` (deduped), with `repl` standing in for some edges.
pub(super) fn points_with(data: &LayoutData, i: usize, repl: &[(usize, Vec<Point>)]) -> Vec<Point> {
    match repl.iter().find(|(k, _)| *k == i) {
        Some((_, p)) => dedupe(p, GEPS),
        None => dedupe(pts_of(&data.edges[i]), GEPS),
    }
}

/// Strict crossings between every pair of visible edges.
pub(super) fn crossing_count(
    data: &LayoutData,
    vis: &[usize],
    repl: &[(usize, Vec<Point>)],
) -> usize {
    let all: Vec<Vec<Seg>> = vis
        .iter()
        .map(|&i| segs(&points_with(data, i, repl)))
        .collect();
    let mut count = 0;
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            for a in &all[i] {
                for b in &all[j] {
                    if segs_strictly_cross(a.a, a.b, b.a, b.b, E) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// separateSharedRenderedTerminalLanes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TerminalLane {
    edge: usize,
    node_id: String,
    at_start: bool,
    vertical: bool,
    coord: f64,
    min: f64,
    max: f64,
    boundary: Point,
    rail_end: Point,
    rect: Rb,
}

fn rect_intersect(n: &LNode, p: Point) -> Point {
    let x = n.x.unwrap_or(0.0);
    let y = n.y.unwrap_or(0.0);
    let dx = p.x - x;
    let dy = p.y - y;
    let mut w = n.width.unwrap_or(0.0) / 2.0;
    let mut h = n.height.unwrap_or(0.0) / 2.0;
    if dy.abs() * w > dx.abs() * h {
        if dy < 0.0 {
            h = -h;
        }
        return pt(x + if dy == 0.0 { 0.0 } else { h * dx / dy }, y + h);
    }
    if dx < 0.0 {
        w = -w;
    }
    pt(x + w, y + if dx == 0.0 { 0.0 } else { w * dy / dx })
}

fn terminal_lane_for(
    data: &LayoutData,
    ei: usize,
    points: &[Point],
    at_start: bool,
) -> Option<TerminalLane> {
    let points = dedupe(points, GEPS);
    if points.len() < 2 {
        return None;
    }
    let e = &data.edges[ei];
    let node_id = if at_start { &e.start } else { &e.end };
    let n = node(data, node_id)?;
    let rect = rect_of(n)?;
    let endpoint = if at_start {
        points[0]
    } else {
        points[points.len() - 1]
    };
    let adjacent = if at_start {
        points[1]
    } else {
        points[points.len() - 2]
    };
    let boundary = rect_intersect(n, endpoint);
    let rail_end = if aligned(adjacent, boundary) {
        adjacent
    } else {
        endpoint
    };
    if same_x(boundary, rail_end, E) {
        return Some(TerminalLane {
            edge: ei,
            node_id: node_id.clone(),
            at_start,
            vertical: true,
            coord: boundary.x,
            min: boundary.y.min(rail_end.y),
            max: boundary.y.max(rail_end.y),
            boundary,
            rail_end,
            rect,
        });
    }
    if same_y(boundary, rail_end, E) {
        return Some(TerminalLane {
            edge: ei,
            node_id: node_id.clone(),
            at_start,
            vertical: false,
            coord: boundary.y,
            min: boundary.x.min(rail_end.x),
            max: boundary.x.max(rail_end.x),
            boundary,
            rail_end,
            rect,
        });
    }
    None
}

fn projected_overlap(a: &TerminalLane, b: &TerminalLane) -> f64 {
    (a.max.min(b.max) - a.min.max(b.min)).max(0.0)
}

fn same_terminal_face(a: &TerminalLane, b: &TerminalLane) -> bool {
    if a.node_id != b.node_id || a.vertical != b.vertical {
        return false;
    }
    if !a.vertical {
        let on =
            (a.boundary.x - a.rect.left).abs() < 1.0 || (a.boundary.x - a.rect.right).abs() < 1.0;
        return on && same_x(a.boundary, b.boundary, 1.0);
    }
    let on = (a.boundary.y - a.rect.top).abs() < 1.0 || (a.boundary.y - a.rect.bottom).abs() < 1.0;
    on && same_y(a.boundary, b.boundary, 1.0)
}

fn exact_conflict(a: &TerminalLane, b: &TerminalLane) -> bool {
    if a.node_id != b.node_id || a.vertical != b.vertical {
        return false;
    }
    projected_overlap(a, b) >= MIN_SHARED && (a.coord - b.coord).abs() < 0.5
}

fn near_conflict(a: &TerminalLane, b: &TerminalLane) -> bool {
    if a.node_id != b.node_id || a.vertical != b.vertical || a.vertical || a.at_start == b.at_start
    {
        return false;
    }
    let shared = projected_overlap(a, b);
    if shared < MIN_SHARED {
        return false;
    }
    let span = a.rect.bottom - a.rect.top;
    if shared < span || shared > 2.0 * span {
        return false;
    }
    same_terminal_face(a, b) && (a.coord - b.coord).abs() < 16.0
}

/// `separateSharedRenderedTerminalLanes`.
pub fn separate_shared_rendered_terminal_lanes(data: &mut LayoutData) {
    const SHIFT: f64 = 7.0;
    let shifted = |data: &LayoutData, lane: &TerminalLane, shift: f64| -> Option<Vec<Point>> {
        let points = dedupe(pts_of(&data.edges[lane.edge]), GEPS);
        if points.len() < 2 {
            return None;
        }
        let (sb, sr) = if lane.vertical {
            (
                pt(lane.boundary.x + shift, lane.boundary.y),
                pt(lane.rail_end.x + shift, lane.rail_end.y),
            )
        } else {
            (
                pt(lane.boundary.x, lane.boundary.y + shift),
                pt(lane.rail_end.x, lane.rail_end.y + shift),
            )
        };
        let stays = if (lane.boundary.y - lane.rect.top).abs() < 1.0
            || (lane.boundary.y - lane.rect.bottom).abs() < 1.0
        {
            same_y(sb, lane.boundary, E)
                && sb.x >= lane.rect.left + 1.0
                && sb.x <= lane.rect.right - 1.0
        } else if (lane.boundary.x - lane.rect.left).abs() < 1.0
            || (lane.boundary.x - lane.rect.right).abs() < 1.0
        {
            same_x(sb, lane.boundary, E)
                && sb.y >= lane.rect.top + 1.0
                && sb.y <= lane.rect.bottom - 1.0
        } else {
            false
        };
        if !stays {
            return None;
        }
        if lane.at_start {
            let adj = points.len() > 1 && same_point(points[1], lane.rail_end, E);
            let rest = &points[if adj { 2 } else { 1 }..];
            if let Some(&next) = rest.first() {
                if !aligned(next, sr) {
                    return None;
                }
            }
            let mut v = vec![sb, sr];
            v.extend_from_slice(rest);
            return Some(v);
        }
        let adj = points.len() > 1 && same_point(points[points.len() - 2], lane.rail_end, E);
        let cut = if adj { 2 } else { 1 };
        let before = &points[..points.len().saturating_sub(cut)];
        if let Some(&prev) = before.last() {
            if !aligned(prev, sr) {
                return None;
            }
        }
        let mut v = before.to_vec();
        v.push(sr);
        v.push(sb);
        Some(v)
    };
    let straight_collinear = |data: &LayoutData, lane: &TerminalLane| -> bool {
        let e = &data.edges[lane.edge];
        let points = dedupe(pts_of(e), GEPS);
        if points.len() != 2 {
            return false;
        }
        let (Some(s), Some(t)) = (node(data, &e.start), node(data, &e.end)) else {
            return false;
        };
        let (sx, sy) = (s.x.unwrap_or(0.0), s.y.unwrap_or(0.0));
        let (tx, ty) = (t.x.unwrap_or(0.0), t.y.unwrap_or(0.0));
        let (a, b) = (points[0], points[1]);
        (same_y(a, b, E) && (sy - ty).abs() < 1.0 && (sx - tx).abs() > 1.0)
            || (same_x(a, b, E) && (sx - tx).abs() < 1.0 && (sy - ty).abs() > 1.0)
    };
    let shifts = [
        -SHIFT,
        SHIFT,
        -2.0 * SHIFT,
        2.0 * SHIFT,
        -3.0 * SHIFT,
        3.0 * SHIFT,
    ];
    for _ in 0..8 {
        let mut lanes: Vec<TerminalLane> = Vec::new();
        for i in visible(data) {
            let p = pts_of(&data.edges[i]).to_vec();
            lanes.extend(terminal_lane_for(data, i, &p, true));
            lanes.extend(terminal_lane_for(data, i, &p, false));
        }
        let mut fixed = false;
        'outer: for i in 0..lanes.len() {
            for j in i + 1..lanes.len() {
                let (f, s) = (&lanes[i], &lanes[j]);
                if f.edge == s.edge || !(exact_conflict(f, s) || near_conflict(f, s)) {
                    continue;
                }
                let fixing_near = !exact_conflict(f, s);
                let mut cands = vec![f.clone(), s.clone()];
                cands.sort_by(|a, b| {
                    let ap = straight_collinear(data, a);
                    let bp = straight_collinear(data, b);
                    if ap != bp {
                        return (ap as i32).cmp(&(bp as i32));
                    }
                    ((!b.at_start) as i32).cmp(&((!a.at_start) as i32))
                });
                for lane in &cands {
                    for &sh in &shifts {
                        let Some(c) = shifted(data, lane, sh) else {
                            continue;
                        };
                        let Some(nl) = terminal_lane_for(data, lane.edge, &c, lane.at_start) else {
                            continue;
                        };
                        if lanes.iter().any(|o| {
                            o.edge != lane.edge
                                && (exact_conflict(&nl, o)
                                    || (fixing_near && near_conflict(&nl, o)))
                        }) {
                            continue;
                        }
                        data.edges[lane.edge].points = Some(c);
                        fixed = true;
                        break 'outer;
                    }
                }
            }
        }
        if !fixed {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// collapseRedundantRectangularDoglegs
// ---------------------------------------------------------------------------

/// `collapseRedundantRectangularDoglegs`.
pub fn collapse_redundant_rectangular_doglegs(data: &mut LayoutData) {
    const BUFFER: f64 = 2.0;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let safe = |data: &LayoutData, ei: usize, cand: &[Point]| -> bool {
        let cs = segs(cand);
        if cs.len() + 1 != cand.len() {
            return false;
        }
        let ex = ends(&data.edges[ei]);
        for s in &cs {
            if seg_hits_any_rect(s.a, s.b, &real, &ex, -BUFFER) {
                return false;
            }
            if seg_hits_any_rect(s.a, s.b, &labels, &[], -BUFFER) {
                return false;
            }
        }
        for (oi, o) in data.edges.iter().enumerate() {
            if oi == ei || o.is_layout_only {
                continue;
            }
            let op = pts_of(o);
            if op.len() < 2 {
                continue;
            }
            let os = segs(&dedupe(op, GEPS));
            for c in &cs {
                for s in &os {
                    if same_axis_overlap(c, s, 0.5) >= MIN_SHARED {
                        return false;
                    }
                    if segs_strictly_cross(c.a, c.b, s.a, s.b, E) {
                        return false;
                    }
                }
            }
        }
        true
    };
    let without = |p: &[Point], i: usize| -> Option<Vec<Point>> {
        if i + 4 >= p.len() {
            return None;
        }
        let (p0, p1, p2, p3, p4) = (p[i], p[i + 1], p[i + 2], p[i + 3], p[i + 4]);
        let tv = is_h(p0, p1, GEPS)
            && is_v(p1, p2, GEPS)
            && is_h(p2, p3, GEPS)
            && is_v(p3, p4, GEPS)
            && same_x(p0, p3, E)
            && same_x(p0, p4, E)
            && same_x(p1, p2, E)
            && (p1.x - p0.x) * (p3.x - p2.x) < 0.0;
        let th = is_v(p0, p1, GEPS)
            && is_h(p1, p2, GEPS)
            && is_v(p2, p3, GEPS)
            && is_h(p3, p4, GEPS)
            && same_y(p0, p3, E)
            && same_y(p0, p4, E)
            && same_y(p1, p2, E)
            && (p1.y - p0.y) * (p3.y - p2.y) < 0.0;
        if tv || th {
            let mut v = p[..=i].to_vec();
            v.push(p4);
            v.extend_from_slice(&p[i + 5..]);
            return Some(dedupe(&v, GEPS));
        }
        if i + 5 >= p.len() {
            return None;
        }
        let p5 = p[i + 5];
        let vd = is_v(p0, p1, GEPS)
            && is_h(p1, p2, GEPS)
            && is_v(p2, p3, GEPS)
            && is_h(p3, p4, GEPS)
            && is_v(p4, p5, GEPS)
            && same_x(p0, p4, E)
            && same_x(p0, p5, E)
            && same_x(p2, p3, E)
            && (p2.x - p1.x) * (p4.x - p3.x) < 0.0;
        let hd = is_h(p0, p1, GEPS)
            && is_v(p1, p2, GEPS)
            && is_h(p2, p3, GEPS)
            && is_v(p3, p4, GEPS)
            && is_h(p4, p5, GEPS)
            && same_y(p0, p4, E)
            && same_y(p0, p5, E)
            && same_y(p2, p3, E)
            && (p2.y - p1.y) * (p4.y - p3.y) < 0.0;
        if !vd && !hd {
            return None;
        }
        let mut v = p[..=i].to_vec();
        v.push(p5);
        v.extend_from_slice(&p[i + 6..]);
        Some(dedupe(&v, GEPS))
    };
    for _ in 0..8 {
        let mut fixed = false;
        for ei in 0..data.edges.len() {
            if data.edges[ei].is_layout_only {
                continue;
            }
            let points = dedupe(pts_of(&data.edges[ei]), GEPS);
            let mut i = 0;
            while i + 5 <= points.len() {
                if let Some(c) = without(&points, i) {
                    if safe(data, ei, &c) {
                        data.edges[ei].points = Some(c);
                        fixed = true;
                        break;
                    }
                }
                i += 1;
            }
            if fixed {
                break;
            }
        }
        if !fixed {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// liftObstacleHuggingSameSideRails
// ---------------------------------------------------------------------------

/// `liftObstacleHuggingSameSideRails`.
pub fn lift_obstacle_hugging_same_side_rails(data: &mut LayoutData) {
    const BUFFER: f64 = 2.0;
    const CLEARANCE: f64 = 20.0;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let vis = visible(data);
    let middle_rail = |p: &[Point]| -> Option<Seg> {
        let s = segs(p);
        if s.len() != 3 {
            return None;
        }
        let m = s[1];
        if s[0].horizontal == m.horizontal || s[2].horizontal == m.horizontal {
            return None;
        }
        Some(m)
    };
    let blockers = |e: &LEdge, rail: &Seg| -> Vec<Rb> {
        let ex = ends(e);
        real.iter()
            .filter(|r| {
                if ex.contains(&r.id.as_str()) {
                    return false;
                }
                let rc = &r.rect;
                if rail.horizontal {
                    overlap_length(rail.a.x, rail.b.x, rc.left, rc.right) >= MIN_SHARED
                        && rail.a.y >= rc.top - BUFFER
                        && rail.a.y <= rc.bottom + BUFFER
                } else {
                    overlap_length(rail.a.y, rail.b.y, rc.top, rc.bottom) >= MIN_SHARED
                        && rail.a.x >= rc.left - BUFFER
                        && rail.a.x <= rc.right + BUFFER
                }
            })
            .map(|r| r.rect)
            .collect()
    };
    let moved = |p: &[Point], rail: &Seg, coord: f64| -> Option<Vec<Point>> {
        let mut c = p.to_vec();
        if rail.horizontal {
            c[rail.index].y = coord;
            c[rail.index + 1].y = coord;
        } else if rail.vertical {
            c[rail.index].x = coord;
            c[rail.index + 1].x = coord;
        } else {
            return None;
        }
        let s = simplify(&dedupe(&c, GEPS));
        (segs(&s).len() + 1 == s.len()).then_some(s)
    };
    let safe = |data: &LayoutData, ei: usize, cand: &[Point], current: usize| -> bool {
        let ex = ends(&data.edges[ei]);
        let cs = segs(cand);
        if cs.len() + 1 != cand.len() {
            return false;
        }
        for s in &cs {
            if seg_hits_any_rect(s.a, s.b, &real, &ex, -BUFFER) {
                return false;
            }
            if seg_hits_any_rect(s.a, s.b, &labels, &[], -BUFFER) {
                return false;
            }
        }
        for &oi in &vis {
            if oi == ei {
                continue;
            }
            let os = segs(&points_with(data, oi, &[]));
            for c in &cs {
                for s in &os {
                    if same_axis_overlap(c, s, 0.5) >= MIN_SHARED {
                        return false;
                    }
                }
            }
        }
        crossing_count(data, &vis, &[(ei, cand.to_vec())]) <= current
    };
    for _ in 0..8 {
        let current = crossing_count(data, &vis, &[]);
        let mut fixed = false;
        for &ei in &vis {
            let points = points_with(data, ei, &[]);
            let Some(rail) = middle_rail(&points) else {
                continue;
            };
            let b = blockers(&data.edges[ei], &rail);
            if b.is_empty() {
                continue;
            }
            let coords = if rail.horizontal {
                [
                    b.iter().fold(f64::INFINITY, |m, r| m.min(r.top)) - CLEARANCE,
                    b.iter().fold(f64::NEG_INFINITY, |m, r| m.max(r.bottom)) + CLEARANCE,
                ]
            } else {
                [
                    b.iter().fold(f64::INFINITY, |m, r| m.min(r.left)) - CLEARANCE,
                    b.iter().fold(f64::NEG_INFINITY, |m, r| m.max(r.right)) + CLEARANCE,
                ]
            };
            for c in coords {
                let Some(cand) = moved(&points, &rail, c) else {
                    continue;
                };
                if !safe(data, ei, &cand, current) {
                    continue;
                }
                data.edges[ei].points = Some(cand);
                fixed = true;
                break;
            }
            if fixed {
                break;
            }
        }
        if !fixed {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Lane title bands
// ---------------------------------------------------------------------------

fn valid_title_rect(n: &LNode) -> Option<Rb> {
    let r = n.group_title_rect?;
    if !(r.left.is_finite() && r.right.is_finite() && r.top.is_finite() && r.bottom.is_finite())
        || r.right <= r.left
        || r.bottom <= r.top
    {
        return None;
    }
    Some(r)
}

/// `liftTopLaneTitleBandsAboveRails`.
pub fn lift_top_lane_title_bands_above_rails(data: &mut LayoutData) {
    const CLEARANCE: f64 = 4.0;
    let lanes = LaneModel::new(&data.nodes);
    let dir = data.direction.to_uppercase();
    let titles: Vec<(usize, Rb)> = data
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            if !lanes.is_lane(&n.id) || dir == "LR" || dir == "RL" || dir == "BT" {
                return None;
            }
            let r = valid_title_rect(n)?;
            let (y, h) = (n.y?, n.height?);
            if !y.is_finite() || !h.is_finite() || h <= 0.0 {
                return None;
            }
            let (tw, th) = (r.right - r.left, r.bottom - r.top);
            if th <= 0.0 || tw < th {
                return None;
            }
            Some((i, r))
        })
        .collect();
    if titles.is_empty() {
        return;
    }
    let mut delta: f64 = 0.0;
    for e in data.edges.iter().filter(|e| !e.is_layout_only) {
        for s in segs(&dedupe(pts_of(e), GEPS)) {
            for (_, r) in &titles {
                if !s.horizontal {
                    continue;
                }
                let y = s.a.y;
                if y < r.top - E || y > r.bottom + E {
                    continue;
                }
                if overlap_length(s.a.x, s.b.x, r.left, r.right) < MIN_SHARED {
                    continue;
                }
                delta = delta.max(r.bottom - y + CLEARANCE);
            }
        }
    }
    if delta <= E {
        return;
    }
    for (i, r) in titles {
        let n = &mut data.nodes[i];
        let (Some(y), Some(h)) = (n.y, n.height) else {
            continue;
        };
        if !y.is_finite() || !h.is_finite() || h <= 0.0 {
            continue;
        }
        n.y = Some(y - delta / 2.0);
        n.height = Some(h + delta);
        n.group_title_rect = Some(Rb {
            top: r.top - delta,
            bottom: r.bottom - delta,
            ..r
        });
    }
}

/// `shiftLeftLaneTitleBandsLeftOfRails`.
pub fn shift_left_lane_title_bands_left_of_rails(data: &mut LayoutData) {
    const CLEARANCE: f64 = 4.0;
    let lanes = LaneModel::new(&data.nodes);
    let dir = data.direction.to_uppercase();
    let titles: Vec<(usize, Rb)> = data
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            if !lanes.is_lane(&n.id) || dir != "LR" {
                return None;
            }
            let r = valid_title_rect(n)?;
            let (x, w) = (n.x?, n.width?);
            if !x.is_finite() || !w.is_finite() || w <= 0.0 {
                return None;
            }
            let (tw, th) = (r.right - r.left, r.bottom - r.top);
            if tw <= 0.0 || th < tw {
                return None;
            }
            Some((i, r))
        })
        .collect();
    if titles.is_empty() {
        return;
    }
    let mut delta: f64 = 0.0;
    for e in data.edges.iter().filter(|e| !e.is_layout_only) {
        for s in segs(&dedupe(pts_of(e), GEPS)) {
            for (_, r) in &titles {
                let vhit = s.vertical
                    && !(s.a.x < r.left - E || s.a.x > r.right + E)
                    && overlap_length(s.a.y, s.b.y, r.top, r.bottom) >= MIN_SHARED;
                if vhit {
                    delta = delta.max(r.right - s.a.x + CLEARANCE);
                    continue;
                }
                let hhit = s.horizontal
                    && !(s.a.y < r.top - E || s.a.y > r.bottom + E)
                    && overlap_length(s.a.x, s.b.x, r.left, r.right) >= MIN_SHARED;
                if hhit {
                    delta = delta.max(r.right - s.a.x.min(s.b.x) + CLEARANCE);
                }
            }
        }
    }
    if delta <= E {
        return;
    }
    for (i, r) in titles {
        let n = &mut data.nodes[i];
        let (Some(x), Some(w)) = (n.x, n.width) else {
            continue;
        };
        if !x.is_finite() || !w.is_finite() || w <= 0.0 {
            continue;
        }
        n.x = Some(x - delta / 2.0);
        n.width = Some(w + delta);
        n.group_title_rect = Some(Rb {
            left: r.left - delta,
            right: r.right - delta,
            ..r
        });
    }
}

// ---------------------------------------------------------------------------
// swapDestinationTerminalTailsToReduceCrossings
// ---------------------------------------------------------------------------

fn total_bends(data: &LayoutData, vis: &[usize], repl: &[(usize, Vec<Point>)]) -> usize {
    vis.iter()
        .map(|&i| count_bends(&points_with(data, i, repl), GEPS))
        .sum()
}

/// `swapDestinationTerminalTailsToReduceCrossings`.
pub fn swap_destination_terminal_tails(data: &mut LayoutData) {
    const BUFFER: f64 = 2.0;
    let (real, _) = node_rect_entries(data.nodes.iter());
    let vis = visible(data);
    let tail_for = |data: &LayoutData, i: usize| -> Option<(Point, Point)> {
        let p = points_with(data, i, &[]);
        if p.len() < 4 {
            return None;
        }
        let (ts, t) = (p[p.len() - 2], p[p.len() - 1]);
        if !is_h(ts, t, E) && !is_v(ts, t, E) {
            return None;
        }
        Some((ts, t))
    };
    let with_tail = |data: &LayoutData, i: usize, tail: (Point, Point)| -> Option<Vec<Point>> {
        let p = points_with(data, i, &[]);
        if p.len() < 3 {
            return None;
        }
        let (start, first) = (p[0], p[1]);
        let connector = if is_h(start, first, E) {
            pt(first.x, tail.0.y)
        } else if is_v(start, first, E) {
            pt(tail.0.x, first.y)
        } else {
            return None;
        };
        let c = simplify(&dedupe(&[start, first, connector, tail.0, tail.1], GEPS));
        (segs(&c).len() + 1 == c.len()).then_some(c)
    };
    let safe =
        |data: &LayoutData, i: usize, path: &[Point], repl: &[(usize, Vec<Point>)]| -> bool {
            let ex = ends(&data.edges[i]);
            let ps = segs(path);
            if ps
                .iter()
                .any(|s| seg_hits_any_rect(s.a, s.b, &real, &ex, -BUFFER))
            {
                return false;
            }
            for &oi in &vis {
                if oi == i {
                    continue;
                }
                for c in &ps {
                    for s in &segs(&points_with(data, oi, repl)) {
                        if same_axis_overlap(c, s, 0.5) >= MIN_SHARED {
                            return false;
                        }
                    }
                }
            }
            true
        };
    for _ in 0..4 {
        let current = crossing_count(data, &vis, &[]);
        if current == 0 {
            return;
        }
        let cur_bends = total_bends(data, &vis, &[]);
        let mut best: Option<Vec<(usize, Vec<Point>)>> = None;
        let mut best_c = current;
        let mut best_b = cur_bends;
        // edgesByDestination
        let mut by_dst: Vec<(String, Vec<usize>)> = Vec::new();
        for &ei in &vis {
            let d = &data.edges[ei].end;
            if d.is_empty() || node(data, d).is_none() {
                continue;
            }
            if points_with(data, ei, &[]).len() < 4 {
                continue;
            }
            match by_dst.iter_mut().find(|(k, _)| k == d) {
                Some((_, v)) => v.push(ei),
                None => by_dst.push((d.clone(), vec![ei])),
            }
        }
        for (_, group) in &by_dst {
            for i in 0..group.len() {
                for j in i + 1..group.len() {
                    let (f, s) = (group[i], group[j]);
                    let (Some(ft), Some(st)) = (tail_for(data, f), tail_for(data, s)) else {
                        continue;
                    };
                    let (Some(fc), Some(sc)) = (with_tail(data, f, st), with_tail(data, s, ft))
                    else {
                        continue;
                    };
                    let repl = vec![(f, fc.clone()), (s, sc.clone())];
                    if !safe(data, f, &fc, &repl) || !safe(data, s, &sc, &repl) {
                        continue;
                    }
                    let cc = crossing_count(data, &vis, &repl);
                    let cb = total_bends(data, &vis, &repl);
                    if cc >= current {
                        continue;
                    }
                    if cc > best_c || (cc == best_c && cb >= best_b) {
                        continue;
                    }
                    best = Some(repl);
                    best_c = cc;
                    best_b = cb;
                }
            }
        }
        let Some(b) = best else { return };
        for (i, p) in b {
            data.edges[i].points = Some(p);
        }
    }
}

// ---------------------------------------------------------------------------
// reassignCrossingExternalRailChannels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ExternalRail {
    edge: usize,
    points: Vec<Point>,
    seg_index: usize,
    vertical: bool,
    side: RectSide,
    coord: f64,
    min: f64,
    max: f64,
}

fn permutations(
    coords: &[f64],
    used: &mut Vec<bool>,
    next: &mut Vec<f64>,
    n: usize,
    current: &[f64],
    out: &mut Vec<Vec<f64>>,
) {
    if next.len() == n {
        if next
            .iter()
            .enumerate()
            .any(|(i, c)| (c - current[i]).abs() >= E)
        {
            out.push(next.clone());
        }
        return;
    }
    for i in 0..coords.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        next.push(coords[i]);
        permutations(coords, used, next, n, current, out);
        next.pop();
        used[i] = false;
    }
}

/// `reassignCrossingExternalRailChannels`.
pub fn reassign_crossing_external_rail_channels(data: &mut LayoutData) {
    const BUFFER: f64 = 2.0;
    const GAP: f64 = 12.0;
    const MAX_EXHAUSTIVE: usize = 6;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let vis = visible(data);
    let end_rects = |data: &LayoutData, ei: usize| -> Option<(Rb, Rb)> {
        let e = &data.edges[ei];
        let s = node(data, &e.start).and_then(rect_of)?;
        let d = node(data, &e.end).and_then(rect_of)?;
        Some((s, d))
    };
    let rail_for =
        |data: &LayoutData, ei: usize, points: &[Point], s: &Seg| -> Option<ExternalRail> {
            if s.index == 0 || s.index + 1 >= points.len() - 1 {
                return None;
            }
            let (sr, dr) = end_rects(data, ei)?;
            let (c, lo, hi, min, max) = if s.vertical {
                (
                    s.a.x,
                    sr.left.min(dr.left),
                    sr.right.max(dr.right),
                    s.a.y.min(s.b.y),
                    s.a.y.max(s.b.y),
                )
            } else if s.horizontal {
                (
                    s.a.y,
                    sr.top.min(dr.top),
                    sr.bottom.max(dr.bottom),
                    s.a.x.min(s.b.x),
                    s.a.x.max(s.b.x),
                )
            } else {
                return None;
            };
            let side = if c < lo - E {
                if s.vertical {
                    RectSide::Left
                } else {
                    RectSide::Top
                }
            } else if c > hi + E {
                if s.vertical {
                    RectSide::Right
                } else {
                    RectSide::Bottom
                }
            } else {
                return None;
            };
            Some(ExternalRail {
                edge: ei,
                points: points.to_vec(),
                seg_index: s.index,
                vertical: s.vertical,
                side,
                coord: c,
                min,
                max,
            })
        };
    let interact = |a: &ExternalRail, b: &ExternalRail| {
        a.edge != b.edge
            && a.vertical == b.vertical
            && a.side == b.side
            && overlap_length(a.min, a.max, b.min, b.max) >= MIN_SHARED
    };
    let replacements_for =
        |comp: &[ExternalRail], asg: &[f64]| -> Option<Vec<(usize, Vec<Point>)>> {
            let mut draft: Vec<(usize, Vec<Point>)> = Vec::new();
            for (i, r) in comp.iter().enumerate() {
                let c = asg[i];
                let k = match draft.iter().position(|(e, _)| *e == r.edge) {
                    Some(k) => k,
                    None => {
                        draft.push((r.edge, r.points.clone()));
                        draft.len() - 1
                    }
                };
                let p = &mut draft[k].1;
                if r.vertical {
                    p[r.seg_index].x = c;
                    p[r.seg_index + 1].x = c;
                } else {
                    p[r.seg_index].y = c;
                    p[r.seg_index + 1].y = c;
                }
            }
            let mut out = Vec::new();
            for (e, p) in draft {
                let s = simplify(&dedupe(&p, GEPS));
                if segs(&s).len() + 1 != s.len() {
                    return None;
                }
                out.push((e, s));
            }
            Some(out)
        };
    let safe = |data: &LayoutData, repl: &[(usize, Vec<Point>)]| -> bool {
        for (ei, p) in repl {
            let ex = ends(&data.edges[*ei]);
            for s in segs(p) {
                if seg_hits_any_rect(s.a, s.b, &real, &ex, -BUFFER) {
                    return false;
                }
                if seg_hits_any_rect(s.a, s.b, &labels, &[], -BUFFER) {
                    return false;
                }
            }
        }
        let changed = |i: usize| repl.iter().any(|(e, _)| *e == i);
        for i in 0..vis.len() {
            let fc = changed(vis[i]);
            let fs = segs(&points_with(data, vis[i], repl));
            for j in i + 1..vis.len() {
                if !fc && !changed(vis[j]) {
                    continue;
                }
                let ss = segs(&points_with(data, vis[j], repl));
                for a in &fs {
                    for b in &ss {
                        if same_axis_overlap(a, b, 0.5) >= MIN_SHARED {
                            return false;
                        }
                    }
                }
            }
        }
        true
    };
    for _ in 0..4 {
        let current = crossing_count(data, &vis, &[]);
        if current == 0 {
            return;
        }
        let mut best: Option<Vec<(usize, Vec<Point>)>> = None;
        let mut best_c = current;
        let mut best_b = total_bends(data, &vis, &[]);
        let mut best_d = f64::INFINITY;
        let mut rails: Vec<ExternalRail> = Vec::new();
        for &ei in &vis {
            let p = points_with(data, ei, &[]);
            for s in segs(&p) {
                if let Some(r) = rail_for(data, ei, &p, &s) {
                    rails.push(r);
                }
            }
        }
        let mut comps: Vec<Vec<ExternalRail>> = Vec::new();
        let mut seen = vec![false; rails.len()];
        for r in 0..rails.len() {
            if seen[r] {
                continue;
            }
            let mut queue = vec![r];
            let mut comp = Vec::new();
            seen[r] = true;
            while let Some(cur) = queue.pop() {
                comp.push(rails[cur].clone());
                for n in 0..rails.len() {
                    if !seen[n] && interact(&rails[cur], &rails[n]) {
                        seen[n] = true;
                        queue.push(n);
                    }
                }
            }
            if comp.len() > 1 {
                comps.push(comp);
            }
        }
        for comp in &comps {
            let current_c: Vec<f64> = comp.iter().map(|r| r.coord).collect();
            let mut coords: Vec<f64> = Vec::new();
            for r in comp {
                if !coords.iter().any(|c| (c - r.coord).abs() < E) {
                    coords.push(r.coord);
                }
            }
            while coords.len() < comp.len() {
                let mn = coords.iter().fold(f64::INFINITY, |a, &b| a.min(b));
                let mx = coords.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
                let k = (comp.len() - coords.len()) as f64;
                coords.push(if matches!(comp[0].side, RectSide::Left | RectSide::Top) {
                    mn - GAP * k
                } else {
                    mx + GAP * k
                });
            }
            let mut assignments: Vec<Vec<f64>> = Vec::new();
            if comp.len() <= MAX_EXHAUSTIVE {
                let mut used = vec![false; coords.len()];
                permutations(
                    &coords,
                    &mut used,
                    &mut Vec::new(),
                    comp.len(),
                    &current_c,
                    &mut assignments,
                );
            } else {
                for i in 0..current_c.len() {
                    for j in i + 1..current_c.len() {
                        let mut a = current_c.clone();
                        a.swap(i, j);
                        assignments.push(a);
                    }
                }
            }
            for asg in &assignments {
                let Some(repl) = replacements_for(comp, asg) else {
                    continue;
                };
                if !safe(data, &repl) {
                    continue;
                }
                let cc = crossing_count(data, &vis, &repl);
                if cc >= current {
                    continue;
                }
                let cb = total_bends(data, &vis, &repl);
                let cd: f64 = comp
                    .iter()
                    .enumerate()
                    .map(|(i, r)| (asg[i] - r.coord).abs())
                    .sum();
                if cc > best_c || (cc == best_c && (cb > best_b || (cb == best_b && cd >= best_d)))
                {
                    continue;
                }
                best = Some(repl);
                best_c = cc;
                best_b = cb;
                best_d = cd;
            }
        }
        let Some(b) = best else { return };
        for (i, p) in b {
            data.edges[i].points = Some(p);
        }
    }
}
