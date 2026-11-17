//! `direction/geometry.ts`: the orthogonal-polyline helpers the clean-up passes share.

use super::model::{LNode, Rb};
use crate::geom::Point;

/// The helpers' default epsilon.
pub const GEPS: f64 = 1e-3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RectSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl RectSide {
    pub const ALL: [RectSide; 4] = [
        RectSide::Top,
        RectSide::Bottom,
        RectSide::Left,
        RectSide::Right,
    ];
    pub fn horizontal_face(self) -> bool {
        matches!(self, RectSide::Left | RectSide::Right)
    }
}

/// A node's centre and box (`NodeBoundsInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub id: String,
    pub cx: f64,
    pub cy: f64,
    pub rect: Rb,
}

/// `RectEntry`.
#[derive(Debug, Clone, PartialEq)]
pub struct RectEntry {
    pub id: String,
    pub rect: Rb,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub index: usize,
    pub a: Point,
    pub b: Point,
    pub horizontal: bool,
    pub vertical: bool,
}

pub fn pt(x: f64, y: f64) -> Point {
    Point { x, y }
}

pub fn rect_from_center_size(cx: f64, cy: f64, w: f64, h: f64) -> Rb {
    Rb {
        left: cx - w / 2.0,
        right: cx + w / 2.0,
        top: cy - h / 2.0,
        bottom: cy + h / 2.0,
    }
}

fn measured(n: &LNode) -> Option<(f64, f64, Rb)> {
    let cx = n.x.unwrap_or(0.0);
    let cy = n.y.unwrap_or(0.0);
    let w = n.width.unwrap_or(0.0);
    let h = n.height.unwrap_or(0.0);
    (w > 0.0 && h > 0.0).then(|| (cx, cy, rect_from_center_size(cx, cy, w, h)))
}

fn info_for(n: &LNode) -> Option<NodeInfo> {
    if n.is_group {
        return None;
    }
    let (cx, cy, rect) = measured(n)?;
    Some(NodeInfo {
        id: n.id.clone(),
        cx,
        cy,
        rect,
    })
}

/// `rectOfNodeBounds`.
pub fn rect_of(n: &LNode) -> Option<Rb> {
    measured(n).map(|m| m.2)
}

pub fn same_point(a: Point, b: Point, e: f64) -> bool {
    (a.x - b.x).abs() < e && (a.y - b.y).abs() < e
}
pub fn same_x(a: Point, b: Point, e: f64) -> bool {
    (a.x - b.x).abs() < e
}
pub fn same_y(a: Point, b: Point, e: f64) -> bool {
    (a.y - b.y).abs() < e
}
pub fn is_h(a: Point, b: Point, e: f64) -> bool {
    same_y(a, b, e) && (a.x - b.x).abs() > e
}
pub fn is_v(a: Point, b: Point, e: f64) -> bool {
    same_x(a, b, e) && (a.y - b.y).abs() > e
}

pub fn overlap_length(a1: f64, a2: f64, b1: f64, b2: f64) -> f64 {
    (a1.max(a2).min(b1.max(b2)) - a1.min(a2).max(b1.min(b2))).max(0.0)
}

pub fn same_axis_overlap(a: &Seg, b: &Seg, e: f64) -> f64 {
    if a.horizontal && b.horizontal && same_y(a.a, b.a, e) {
        return overlap_length(a.a.x, a.b.x, b.a.x, b.b.x);
    }
    if a.vertical && b.vertical && same_x(a.a, b.a, e) {
        return overlap_length(a.a.y, a.b.y, b.a.y, b.b.y);
    }
    0.0
}

/// `orthogonalSegmentsForPoints`.
pub fn segments(points: &[Point], e: f64) -> Vec<Seg> {
    let mut out = Vec::new();
    for i in 0..points.len().saturating_sub(1) {
        let (a, b) = (points[i], points[i + 1]);
        let horizontal = is_h(a, b, e);
        let vertical = is_v(a, b, e);
        if horizontal || vertical {
            out.push(Seg {
                index: i,
                a,
                b,
                horizontal,
                vertical,
            });
        }
    }
    out
}

pub fn count_bends(points: &[Point], e: f64) -> usize {
    let s = segments(points, e);
    (1..s.len())
        .filter(|&i| s[i - 1].horizontal != s[i].horizontal)
        .count()
}

fn bend_points(points: &[Point]) -> Vec<Point> {
    let s = segments(&dedupe(points, GEPS), GEPS);
    let mut out = Vec::new();
    for i in 1..s.len() {
        if s[i - 1].horizontal != s[i].horizontal {
            out.push(s[i].a);
        }
    }
    out
}

fn distance_to_segment(p: Point, s: &Seg) -> f64 {
    let dx = (s.a.x.min(s.b.x) - p.x)
        .max(0.0)
        .max(p.x - s.a.x.max(s.b.x));
    let dy = (s.a.y.min(s.b.y) - p.y)
        .max(0.0)
        .max(p.y - s.a.y.max(s.b.y));
    dx.hypot(dy)
}

/// `routesTouchAtBend`.
pub fn routes_touch_at_bend(first: &[Point], second: &[Point], tol: f64) -> bool {
    let touches = |bends_of: &[Point], path: &[Point]| {
        let s = segments(&dedupe(path, GEPS), GEPS);
        bend_points(bends_of)
            .iter()
            .any(|&b| s.iter().any(|seg| distance_to_segment(b, seg) < tol))
    };
    touches(first, second) || touches(second, first)
}

/// `dedupeConsecutivePoints`.
pub fn dedupe(points: &[Point], e: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    for &p in points {
        if out.last().is_none_or(|&l| !same_point(l, p, e)) {
            out.push(p);
        }
    }
    out
}

/// `classifyThreeSegmentRoute`: `Some(true)` for H-V-H, `Some(false)` for V-H-V.
pub fn classify_three(points: Option<&[Point]>, e: f64) -> Option<bool> {
    let p = points?;
    if p.len() != 4 {
        return None;
    }
    if is_h(p[0], p[1], e) && is_v(p[1], p[2], e) && is_h(p[2], p[3], e) {
        return Some(true);
    }
    (is_v(p[0], p[1], e) && is_h(p[1], p[2], e) && is_v(p[2], p[3], e)).then_some(false)
}

pub fn seg_overlaps_rect(a: Point, b: Point, r: &Rb, buffer: f64) -> bool {
    a.x.max(b.x) > r.left - buffer
        && a.x.min(b.x) < r.right + buffer
        && a.y.max(b.y) > r.top - buffer
        && a.y.min(b.y) < r.bottom + buffer
}

pub fn point_inside_rect(p: Point, r: &Rb, buffer: f64) -> bool {
    p.x > r.left + buffer
        && p.x < r.right - buffer
        && p.y > r.top + buffer
        && p.y < r.bottom - buffer
}

pub fn rect_contains(outer: &Rb, inner: &Rb) -> bool {
    outer.left <= inner.left
        && outer.right >= inner.right
        && outer.top <= inner.top
        && outer.bottom >= inner.bottom
}

pub fn rects_overlap(a: &Rb, b: &Rb) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

pub fn inflate(r: &Rb, m: f64) -> Rb {
    Rb {
        left: r.left - m,
        right: r.right + m,
        top: r.top - m,
        bottom: r.bottom + m,
    }
}

/// `portForRectSide`.
pub fn port_for_side(info: &NodeInfo, side: RectSide) -> Point {
    match side {
        RectSide::Top => pt(info.cx, info.rect.top),
        RectSide::Bottom => pt(info.cx, info.rect.bottom),
        RectSide::Left => pt(info.rect.left, info.cy),
        RectSide::Right => pt(info.rect.right, info.cy),
    }
}

/// `buildOrthogonalPortPath`.
pub fn orthogonal_port_path(
    src: Point,
    ss: RectSide,
    dst: Point,
    ds: RectSide,
    anchor: f64,
    e: f64,
) -> Option<Vec<Point>> {
    use RectSide::*;
    let sh = ss.horizontal_face();
    let dh = ds.horizontal_face();
    if sh && dh {
        let opposing = (ss == Right && ds == Left && src.x < dst.x)
            || (ss == Left && ds == Right && src.x > dst.x);
        if opposing {
            if same_y(src, dst, e) {
                return Some(vec![src, dst]);
            }
            let mx = (src.x + dst.x) / 2.0;
            return Some(vec![src, pt(mx, src.y), pt(mx, dst.y), dst]);
        }
        if ss == ds {
            if same_y(src, dst, e) {
                return None;
            }
            let ix = if ss == Left {
                src.x.min(dst.x) - anchor
            } else {
                src.x.max(dst.x) + anchor
            };
            return Some(vec![src, pt(ix, src.y), pt(ix, dst.y), dst]);
        }
        return None;
    }
    if !sh && !dh {
        if ss == ds {
            if same_x(src, dst, e) {
                return None;
            }
            let iy = if ss == Top {
                src.y.min(dst.y) - anchor
            } else {
                src.y.max(dst.y) + anchor
            };
            return Some(vec![src, pt(src.x, iy), pt(dst.x, iy), dst]);
        }
        let same_dir = (ss == Bottom && ds == Top && src.y < dst.y)
            || (ss == Top && ds == Bottom && src.y > dst.y);
        if !same_dir {
            return None;
        }
        if same_x(src, dst, e) {
            return Some(vec![src, dst]);
        }
        let my = (src.y + dst.y) / 2.0;
        return Some(vec![src, pt(src.x, my), pt(dst.x, my), dst]);
    }
    if sh && !dh {
        let a = (ss == Right && dst.x > src.x) || (ss == Left && dst.x < src.x);
        let b = (ds == Top && src.y < dst.y) || (ds == Bottom && src.y > dst.y);
        return (a && b).then(|| vec![src, pt(dst.x, src.y), dst]);
    }
    let a = (ss == Bottom && dst.y > src.y) || (ss == Top && dst.y < src.y);
    let b = (ds == Left && src.x < dst.x) || (ds == Right && src.x > dst.x);
    (a && b).then(|| vec![src, pt(src.x, dst.y), dst])
}

/// `buildSameSideTrackPath`.
pub fn same_side_track_path(src: Point, side: RectSide, dst: Point, track: f64) -> Vec<Point> {
    if side.horizontal_face() {
        vec![src, pt(track, src.y), pt(track, dst.y), dst]
    } else {
        vec![src, pt(src.x, track), pt(dst.x, track), dst]
    }
}

/// `collectRealNodeBounds`: (info by id, real node rects) in node order.
pub fn real_node_bounds(nodes: &[LNode]) -> (Vec<NodeInfo>, Vec<RectEntry>) {
    let mut infos = Vec::new();
    let mut rects = Vec::new();
    for n in nodes {
        if n.is_edge_label {
            continue;
        }
        if let Some(i) = info_for(n) {
            rects.push(RectEntry {
                id: i.id.clone(),
                rect: i.rect,
            });
            infos.push(i);
        }
    }
    (infos, rects)
}

/// `collectNodeRectEntries`: (real node rects, label rects).
pub fn node_rect_entries<'a>(
    nodes: impl Iterator<Item = &'a LNode>,
) -> (Vec<RectEntry>, Vec<RectEntry>) {
    let mut real = Vec::new();
    let mut labels = Vec::new();
    for n in nodes {
        let Some(i) = info_for(n) else { continue };
        let e = RectEntry {
            id: i.id,
            rect: i.rect,
        };
        if n.is_edge_label {
            labels.push(e);
        } else {
            real.push(e);
        }
    }
    (real, labels)
}

/// `getNodePairGeometry`: (src info, dst info, collinear x, collinear y).
pub fn node_pair<'a>(
    start: &str,
    end: &str,
    infos: &'a [NodeInfo],
    e: f64,
) -> Option<(&'a NodeInfo, &'a NodeInfo, bool, bool)> {
    if start.is_empty() || end.is_empty() {
        return None;
    }
    let s = infos.iter().find(|i| i.id == start)?;
    let d = infos.iter().find(|i| i.id == end)?;
    Some((s, d, (s.cx - d.cx).abs() < e, (s.cy - d.cy).abs() < e))
}

/// `segmentHitsAnyRect`.
pub fn seg_hits_any_rect(
    a: Point,
    b: Point,
    rects: &[RectEntry],
    exclude: &[&str],
    shrink: f64,
) -> bool {
    rects
        .iter()
        .any(|r| !exclude.contains(&r.id.as_str()) && seg_overlaps_rect(a, b, &r.rect, -shrink))
}

/// `orthogonalSegmentsCross`.
pub fn segs_cross(a1: Point, b1: Point, a2: Point, b2: Point, e: f64, end_tol: f64) -> bool {
    let s1h = same_y(a1, b1, e);
    let s1v = same_x(a1, b1, e);
    let s2h = same_y(a2, b2, e);
    let s2v = same_x(a2, b2, e);
    if (s1h && s2h) || (s1v && s2v) {
        return false;
    }
    if !(s1h || s1v) || !(s2h || s2v) {
        return false;
    }
    let (ha, hb) = if s1h { (a1, b1) } else { (a2, b2) };
    let (va, vb) = if s1v { (a1, b1) } else { (a2, b2) };
    let hy = ha.y;
    let (hx1, hx2) = (ha.x.min(hb.x), ha.x.max(hb.x));
    let vx = va.x;
    let (vy1, vy2) = (va.y.min(vb.y), va.y.max(vb.y));
    if vx < hx1 || vx > hx2 || hy < vy1 || hy > vy2 {
        return false;
    }
    let at = |q: Point| (vx - q.x).abs() < end_tol && (hy - q.y).abs() < end_tol;
    let mh = at(ha) || at(hb);
    let mv = at(va) || at(vb);
    !(mh && mv)
}

/// `sameAxisSegmentsOverlap`.
pub fn same_axis_segs_overlap(a1: Point, b1: Point, a2: Point, b2: Point, e: f64) -> bool {
    let s1h = same_y(a1, b1, e);
    let s1v = same_x(a1, b1, e);
    let s2h = same_y(a2, b2, e);
    let s2v = same_x(a2, b2, e);
    if s1v && s2v && same_x(a1, a2, e) {
        return overlap_length(a1.y, b1.y, a2.y, b2.y) > e;
    }
    if s1h && s2h && same_y(a1, a2, e) {
        return overlap_length(a1.x, b1.x, a2.x, b2.x) > e;
    }
    false
}

/// `segmentConflictsWithAnyEdge` over the point lists of the other edges.
pub fn seg_conflicts_with_edges<'a>(
    a: Point,
    b: Point,
    others: impl Iterator<Item = &'a [Point]>,
    e: f64,
    skip_degenerate: bool,
) -> bool {
    for pts in others {
        if pts.len() < 2 {
            continue;
        }
        for i in 0..pts.len() - 1 {
            let (oa, ob) = (pts[i], pts[i + 1]);
            if skip_degenerate && same_point(oa, ob, e) {
                continue;
            }
            if segs_cross(a, b, oa, ob, e, 1e-6) || same_axis_segs_overlap(a, b, oa, ob, e) {
                return true;
            }
        }
    }
    false
}

/// `orthogonalSegmentsStrictlyCross`.
pub fn segs_strictly_cross(a1: Point, b1: Point, a2: Point, b2: Point, e: f64) -> bool {
    let ah = same_y(a1, b1, e);
    let av = same_x(a1, b1, e);
    let bh = same_y(a2, b2, e);
    let bv = same_x(a2, b2, e);
    if !((ah && bv) || (av && bh)) {
        return false;
    }
    let ((ha, hb), (va, vb)) = if ah {
        ((a1, b1), (a2, b2))
    } else {
        ((a2, b2), (a1, b1))
    };
    let hy = ha.y;
    let (hxmin, hxmax) = (ha.x.min(hb.x), ha.x.max(hb.x));
    let vx = va.x;
    let (vymin, vymax) = (va.y.min(vb.y), va.y.max(vb.y));
    vx > hxmin + e && vx < hxmax - e && hy > vymin + e && hy < vymax - e
}

fn strictly_between(v: f64, a: f64, b: f64) -> bool {
    v > a.min(b) + GEPS && v < a.max(b) - GEPS
}

fn collinear_intermediate(prev: Point, cur: Point, next: Point) -> bool {
    if same_x(prev, cur, GEPS) && same_x(cur, next, GEPS) {
        return strictly_between(cur.y, prev.y, next.y);
    }
    if same_y(prev, cur, GEPS) && same_y(cur, next, GEPS) {
        return strictly_between(cur.x, prev.x, next.x);
    }
    false
}

fn simplify_once(points: &[Point]) -> (Vec<Point>, bool) {
    let mut changed = false;
    let mut out: Vec<Point> = Vec::new();
    let mut i = 0;
    while i < points.len() {
        let cur = points[i];
        if let (Some(&prev), Some(&next)) = (out.last(), points.get(i + 1)) {
            if same_point(prev, next, GEPS) {
                i += 2;
                changed = true;
                continue;
            }
            if collinear_intermediate(prev, cur, next) {
                changed = true;
                i += 1;
                continue;
            }
        }
        out.push(cur);
        i += 1;
    }
    (out, changed)
}

/// `orthogonalizePolyline`.
pub fn orthogonalize(pts: &[Point]) -> Vec<Point> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut cleaned: Vec<Point> = vec![pts[0]];
    for &curr in &pts[1..] {
        let prev = *cleaned.last().unwrap();
        if !same_x(prev, curr, GEPS) && !same_y(prev, curr, GEPS) {
            let incoming_vertical =
                cleaned.len() >= 2 && same_x(cleaned[cleaned.len() - 2], prev, GEPS);
            cleaned.push(if incoming_vertical {
                pt(prev.x, curr.y)
            } else {
                pt(curr.x, prev.y)
            });
        }
        cleaned.push(curr);
    }
    dedupe(&cleaned, GEPS)
}

/// `simplifyPolyline`.
pub fn simplify(pts: &[Point]) -> Vec<Point> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut work = pts.to_vec();
    for _ in 0..32 {
        let (next, changed) = simplify_once(&work);
        work = next;
        if !changed {
            break;
        }
    }
    work
}
