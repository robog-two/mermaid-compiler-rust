//! The clean-up passes of `postProcessSwimlaneLayout` other than the
//! materialized-geometry ones: `detourSimplification`, `siblingSharedFaceRouting`,
//! `portSwap`, `labelAnchoring`, `endpointClip`, `terminalStub` and
//! `sharedTrackNudging`.

use super::geometry::*;
use super::model::{LEdge, LNode, LayoutData, Rb, sign, to_fixed};
use super::sugiyama::LaneModel;
use crate::geom::Point;
use std::collections::HashMap;

fn pts_of(e: &LEdge) -> &[Point] {
    e.points.as_deref().unwrap_or(&[])
}

/// The point lists of every edge but `skip`.
fn others(edges: &[LEdge], skip: usize) -> impl Iterator<Item = &[Point]> {
    edges
        .iter()
        .enumerate()
        .filter(move |(i, e)| *i != skip && !e.is_layout_only)
        .map(|(_, e)| pts_of(e))
}

pub fn node_index(nodes: &[LNode]) -> HashMap<String, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect()
}

// ---------------------------------------------------------------------------
// detourSimplification
// ---------------------------------------------------------------------------

/// `simplifyDetouredEdges`.
pub fn simplify_detoured_edges(data: &mut LayoutData) {
    const EPS: f64 = 1e-3;
    const MIN_SHARED: f64 = 8.0;
    const ANCHOR: f64 = 20.0;
    const BEND_THRESHOLD: usize = 4;
    let (infos, rects) = real_node_bounds(&data.nodes);
    let info = |id: &str| infos.iter().find(|i| i.id == id);
    let fold = |f: &dyn Fn(&Rb) -> f64, min: bool| {
        rects.iter().map(|r| f(&r.rect)).fold(
            if min {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            },
            |a, b| if min { a.min(b) } else { a.max(b) },
        )
    };
    let outside_top = fold(&|r| r.top, true) - ANCHOR;
    let outside_bottom = fold(&|r| r.bottom, false) + ANCHOR;
    let outside_left = fold(&|r| r.left, true) - ANCHOR;
    let outside_right = fold(&|r| r.right, false) + ANCHOR;
    let outside = |s: RectSide| match s {
        RectSide::Top => outside_top,
        RectSide::Bottom => outside_bottom,
        RectSide::Left => outside_left,
        RectSide::Right => outside_right,
    };
    let candidates = |src: Point, ss: RectSide, dst: Point, ds: RectSide| {
        let mut paths = Vec::new();
        if let Some(b) = orthogonal_port_path(src, ss, dst, ds, ANCHOR, EPS) {
            paths.push(b);
        }
        if ss == ds {
            paths.push(same_side_track_path(src, ss, dst, outside(ss)));
        }
        paths
    };
    let hits_node = |pts: &[Point], ex: &[&str]| {
        (0..pts.len().saturating_sub(1))
            .any(|i| seg_hits_any_rect(pts[i], pts[i + 1], &rects, ex, 1.0))
    };
    let conflict_count = |edges: &[LEdge], path: &[Point], cur: usize, incident: bool| -> usize {
        let mut c = 0;
        let ps = segments(path, EPS);
        let (cs, ce) = (&edges[cur].start, &edges[cur].end);
        for (oi, other) in edges.iter().enumerate() {
            if oi == cur || other.is_layout_only {
                continue;
            }
            if !incident
                && (other.start == *cs
                    || other.start == *ce
                    || other.end == *cs
                    || other.end == *ce)
            {
                continue;
            }
            let op = pts_of(other);
            if op.len() < 2 {
                continue;
            }
            let os = segments(op, EPS);
            for a in &ps {
                for b in &os {
                    if segs_cross(a.a, a.b, b.a, b.b, EPS, EPS) {
                        c += 1;
                        continue;
                    }
                    if same_axis_overlap(a, b, EPS) >= MIN_SHARED {
                        c += 1;
                    }
                }
            }
        }
        c
    };
    let nearest_side = |p: Point, i: &NodeInfo| {
        let mut best = RectSide::Top;
        let mut bd = (p.y - i.rect.top).abs();
        for (s, d) in [
            (RectSide::Bottom, (p.y - i.rect.bottom).abs()),
            (RectSide::Left, (p.x - i.rect.left).abs()),
            (RectSide::Right, (p.x - i.rect.right).abs()),
        ] {
            if d < bd {
                best = s;
                bd = d;
            }
        }
        best
    };
    // node id -> (side, edge id) claims
    let mut claims: HashMap<String, Vec<(RectSide, String)>> = HashMap::new();
    for e in data.edges.iter().filter(|e| !e.is_layout_only) {
        let p = pts_of(e);
        if p.is_empty() {
            continue;
        }
        if let Some(i) = info(&e.start) {
            claims
                .entry(e.start.clone())
                .or_default()
                .push((nearest_side(p[0], i), e.id.clone()));
        }
        if let Some(i) = info(&e.end) {
            claims
                .entry(e.end.clone())
                .or_default()
                .push((nearest_side(p[p.len() - 1], i), e.id.clone()));
        }
    }
    for ei in 0..data.edges.len() {
        let edge = &data.edges[ei];
        if edge.is_layout_only {
            continue;
        }
        let p = pts_of(edge).to_vec();
        if p.len() < 2 {
            continue;
        }
        let cur_bends = count_bends(&p, EPS);
        if cur_bends < BEND_THRESHOLD {
            continue;
        }
        let (src_id, dst_id, eid) = (edge.start.clone(), edge.end.clone(), edge.id.clone());
        let (Some(si), Some(di)) = (info(&src_id), info(&dst_id)) else {
            continue;
        };
        let claimed = |claims: &HashMap<String, Vec<(RectSide, String)>>, n: &str, s: RectSide| {
            claims
                .get(n)
                .is_some_and(|c| c.iter().any(|(cs, ce)| *ce != eid && *cs == s))
        };
        let cur_cross = conflict_count(&data.edges, &p, ei, true);
        let cur_non = conflict_count(&data.edges, &p, ei, false);
        let mut best: Option<Vec<Point>> = None;
        let mut best_cross = cur_cross;
        let mut best_bends = cur_bends;
        for ss in RectSide::ALL {
            if claimed(&claims, &src_id, ss) {
                continue;
            }
            let sp = port_for_side(si, ss);
            for ds in RectSide::ALL {
                if claimed(&claims, &dst_id, ds) {
                    continue;
                }
                let dp = port_for_side(di, ds);
                for path in candidates(sp, ss, dp, ds) {
                    if hits_node(&path, &[&src_id, &dst_id]) {
                        continue;
                    }
                    let pb = count_bends(&path, EPS);
                    if cur_cross > 0 {
                        let pc = conflict_count(&data.edges, &path, ei, true);
                        if pc > best_cross || (pc == best_cross && pb >= best_bends) {
                            continue;
                        }
                        best_cross = pc;
                        best_bends = pb;
                        best = Some(path);
                        continue;
                    }
                    if conflict_count(&data.edges, &path, ei, false) > cur_non {
                        continue;
                    }
                    if pb < best_bends {
                        best_bends = pb;
                        best = Some(path);
                    }
                }
            }
        }
        if let Some(bp) = best {
            for id in [&src_id, &dst_id] {
                if let Some(c) = claims.get_mut(id) {
                    c.retain(|(_, ce)| *ce != eid);
                }
            }
            claims
                .entry(src_id.clone())
                .or_default()
                .push((nearest_side(bp[0], si), eid.clone()));
            claims
                .entry(dst_id.clone())
                .or_default()
                .push((nearest_side(bp[bp.len() - 1], di), eid.clone()));
            data.edges[ei].points = Some(bp);
        }
    }
}

// ---------------------------------------------------------------------------
// siblingSharedFaceRouting
// ---------------------------------------------------------------------------

/// `MIN_ARROWHEAD_SPACING`.
const MIN_ARROWHEAD_SPACING: f64 = 12.0;

fn crowds_face_neighbour(
    edges: &[LEdge],
    edge: usize,
    node_id: &str,
    c: Point,
    end: Point,
    along_x: bool,
) -> bool {
    let across = |q: Point| if along_x { q.y } else { q.x };
    let along = |q: Point| if along_x { q.x } else { q.y };
    let face = sign(across(end) - across(c));
    for (oi, o) in edges.iter().enumerate() {
        let op = pts_of(o);
        if oi == edge || o.is_layout_only || op.is_empty() {
            continue;
        }
        let mut attached = Vec::new();
        if o.end == node_id {
            attached.push(op[op.len() - 1]);
        }
        if o.start == node_id {
            attached.push(op[0]);
        }
        if attached.iter().any(|&q| {
            sign(across(q) - across(c)) == face
                && (along(q) - along(end)).abs() < MIN_ARROWHEAD_SPACING
        }) {
            return true;
        }
    }
    false
}

/// `straightenCollinearSiblingDetours`.
pub fn straighten_collinear_sibling_detours(data: &mut LayoutData) {
    const EPS: f64 = 1e-6;
    const PORT_SHIFT: f64 = 4.0;
    const LABEL_BUFFER: f64 = 3.0;
    let (infos, rects) = real_node_bounds(&data.nodes);
    let label_dim: HashMap<String, (f64, f64)> = data
        .nodes
        .iter()
        .filter(|n| !n.is_group && n.is_edge_label)
        .map(|n| {
            (
                n.id.clone(),
                (n.width.unwrap_or(0.0), n.height.unwrap_or(0.0)),
            )
        })
        .collect();
    let pair_key = |a: &str, b: &str| {
        if a < b {
            format!("{a}::{b}")
        } else {
            format!("{b}::{a}")
        }
    };
    for ei in 0..data.edges.len() {
        let edge = &data.edges[ei];
        if edge.is_layout_only || classify_three(edge.points.as_deref(), EPS).is_none() {
            continue;
        }
        let Some((si, di, cx, cy)) = node_pair(&edge.start, &edge.end, &infos, EPS) else {
            continue;
        };
        if cx == cy {
            continue;
        }
        let (ts, td) = if cx {
            let below = di.cy > si.cy;
            (
                pt(si.cx, if below { si.rect.bottom } else { si.rect.top }),
                pt(di.cx, if below { di.rect.top } else { di.rect.bottom }),
            )
        } else {
            let east = di.cx > si.cx;
            (
                pt(if east { si.rect.right } else { si.rect.left }, si.cy),
                pt(if east { di.rect.left } else { di.rect.right }, di.cy),
            )
        };
        let (sid, did) = (si.id.clone(), di.id.clone());
        if seg_hits_any_rect(ts, td, &rects, &[&sid, &did], 1.0) {
            continue;
        }
        // labelClearanceFor
        let target = pair_key(&sid, &did);
        let mut max_half: f64 = 0.0;
        let mut consider = |l: &Option<String>| {
            if let Some((w, h)) = l.as_ref().and_then(|l| label_dim.get(l)) {
                let half = if cx { w / 2.0 } else { h / 2.0 };
                if half > max_half {
                    max_half = half;
                }
            }
        };
        consider(&edge.label_node_id);
        for (oi, o) in data.edges.iter().enumerate() {
            if oi == ei || o.is_layout_only || o.start.is_empty() || o.end.is_empty() {
                continue;
            }
            if pair_key(&o.start, &o.end) != target {
                continue;
            }
            consider(&o.label_node_id);
        }
        let label_shift = if max_half > 0.0 {
            max_half + LABEL_BUFFER
        } else {
            0.0
        };
        let eff = if label_shift > PORT_SHIFT {
            label_shift
        } else {
            PORT_SHIFT
        };
        let mut crowded: Option<Vec<Point>> = None;
        let mut done: Option<Vec<Point>> = None;
        for delta in [0.0, eff, -eff] {
            let (mut a, mut b) = (ts, td);
            if cx {
                a.x += delta;
                b.x += delta;
                if a.x <= si.rect.left || a.x >= si.rect.right {
                    continue;
                }
                if b.x <= di.rect.left || b.x >= di.rect.right {
                    continue;
                }
            } else {
                a.y += delta;
                b.y += delta;
                if a.y <= si.rect.top || a.y >= si.rect.bottom {
                    continue;
                }
                if b.y <= di.rect.top || b.y >= di.rect.bottom {
                    continue;
                }
            }
            if seg_hits_any_rect(a, b, &rects, &[&sid, &did], 1.0) {
                continue;
            }
            if seg_conflicts_with_edges(a, b, others(&data.edges, ei), EPS, false) {
                continue;
            }
            if crowds_face_neighbour(&data.edges, ei, &sid, pt(si.cx, si.cy), a, cx)
                || crowds_face_neighbour(&data.edges, ei, &did, pt(di.cx, di.cy), b, cx)
            {
                if crowded.is_none() {
                    crowded = Some(vec![a, b]);
                }
                continue;
            }
            done = Some(vec![a, b]);
            crowded = None;
            break;
        }
        if let Some(d) = done.or(crowded) {
            data.edges[ei].points = Some(d);
        }
    }
}

// ---------------------------------------------------------------------------
// portSwap
// ---------------------------------------------------------------------------

/// `portSwapToLShape`.
pub fn port_swap_to_l_shape(data: &mut LayoutData) {
    const EPS: f64 = 1e-6;
    const SHIFT: f64 = 8.0;
    let (infos, rects) = real_node_bounds(&data.nodes);
    for ei in 0..data.edges.len() {
        let edge = &data.edges[ei];
        if edge.is_layout_only {
            continue;
        }
        let p = pts_of(edge);
        if p.len() < 4 {
            continue;
        }
        let dd = dedupe(p, EPS);
        let Some(hvh) = classify_three(Some(&dd), EPS) else {
            continue;
        };
        let p3 = dd[3];
        let Some((si, di, cx, cy)) = node_pair(&edge.start, &edge.end, &infos, EPS) else {
            continue;
        };
        if cx || cy {
            continue;
        }
        let sr = si.rect;
        let mut new_pts: Option<Vec<Point>> = None;
        for delta in [0.0, SHIFT, -SHIFT, 2.0 * SHIFT, -2.0 * SHIFT] {
            let (np0, np1, np2);
            if hvh {
                let ny = if di.cy > si.cy { sr.bottom } else { sr.top };
                let nx = si.cx + delta;
                if nx <= sr.left + EPS || nx >= sr.right - EPS {
                    continue;
                }
                np0 = pt(nx, ny);
                np1 = pt(nx, p3.y);
                np2 = p3;
            } else {
                let nx = if di.cx > si.cx { sr.right } else { sr.left };
                let ny = si.cy + delta;
                if ny <= sr.top + EPS || ny >= sr.bottom - EPS {
                    continue;
                }
                np0 = pt(nx, ny);
                np1 = pt(p3.x, ny);
                np2 = p3;
            }
            let d1 = same_point(np0, np1, EPS);
            let d2 = same_point(np1, np2, EPS);
            if d1 && d2 {
                continue;
            }
            if !d1 && seg_hits_any_rect(np0, np1, &rects, &[&si.id], 1.0) {
                continue;
            }
            if !d2 && seg_hits_any_rect(np1, np2, &rects, &[&di.id], 1.0) {
                continue;
            }
            let c1 = !d1 && seg_conflicts_with_edges(np0, np1, others(&data.edges, ei), EPS, true);
            let c2 = !d2 && seg_conflicts_with_edges(np1, np2, others(&data.edges, ei), EPS, true);
            if c1 || c2 {
                continue;
            }
            new_pts = Some(if d1 {
                vec![np1, np2]
            } else if d2 {
                vec![np0, np1]
            } else {
                vec![np0, np1, np2]
            });
            break;
        }
        if let Some(n) = new_pts {
            data.edges[ei].points = Some(n);
        }
    }
}

// ---------------------------------------------------------------------------
// labelAnchoring
// ---------------------------------------------------------------------------

fn marker_clearance_rect(pts: &[Point], at_start: bool) -> Option<Rb> {
    const EPS: f64 = 1e-3;
    if pts.len() < 2 {
        return None;
    }
    let (tip, inner) = if at_start {
        (pts[0], pts[1])
    } else {
        (pts[pts.len() - 1], pts[pts.len() - 2])
    };
    let dx = inner.x - tip.x;
    let dy = inner.y - tip.y;
    if dx.abs() + dy.abs() < EPS {
        return None;
    }
    if dy.abs() <= EPS {
        let x2 = tip.x + sign(dx) * 10.0;
        return Some(Rb {
            left: tip.x.min(x2),
            right: tip.x.max(x2),
            top: tip.y - 7.0,
            bottom: tip.y + 7.0,
        });
    }
    if dx.abs() <= EPS {
        let y2 = tip.y + sign(dy) * 10.0;
        return Some(Rb {
            left: tip.x - 7.0,
            right: tip.x + 7.0,
            top: tip.y.min(y2),
            bottom: tip.y.max(y2),
        });
    }
    Some(Rb {
        left: tip.x.min(inner.x),
        right: tip.x.max(inner.x),
        top: tip.y.min(inner.y),
        bottom: tip.y.max(inner.y),
    })
}

fn label_overlaps_own_marker(r: &Rb, pts: &[Point]) -> bool {
    let v = dedupe(pts, GEPS);
    [
        marker_clearance_rect(&v, true),
        marker_clearance_rect(&v, false),
    ]
    .iter()
    .flatten()
    .any(|m| {
        let n = Rb {
            left: m.left.min(m.right),
            right: m.left.max(m.right),
            top: m.top.min(m.bottom),
            bottom: m.top.max(m.bottom),
        };
        rects_overlap(r, &n)
    })
}

/// `anchorLabelsToPolyline`.
pub fn anchor_labels_to_polyline(data: &mut LayoutData) {
    const EPS: f64 = 1e-3;
    const BUFFER: f64 = 3.0;
    const LANE_MARGIN: f64 = 1.0;
    const ENDPOINT_CLEARANCE: f64 = 12.0;
    const TS: [f64; 9] = [0.5, 0.25, 0.75, 0.05, 0.95, 0.15, 0.85, 0.1, 0.9];
    // (edge id, p1, p2)
    let mut all_segs: Vec<(String, Point, Point)> = Vec::new();
    for e in data.edges.iter().filter(|e| !e.is_layout_only) {
        let p = pts_of(e);
        for i in 0..p.len().saturating_sub(1) {
            all_segs.push((e.id.clone(), p[i], p[i + 1]));
        }
    }
    let lanes = LaneModel::new(&data.nodes);
    let mut foreign: Vec<(String, Rb)> = Vec::new();
    let mut lane_groups: Vec<(String, Rb)> = Vec::new();
    for n in &data.nodes {
        if lanes.is_lane(&n.id) {
            if let Some(r) = rect_of(n) {
                lane_groups.push((n.id.clone(), r));
            }
            continue;
        }
        if n.is_group || n.is_edge_label {
            continue;
        }
        if let Some(r) = rect_of(n) {
            foreign.push((n.id.clone(), r));
        }
    }
    let over_node = |label: &str, r: &Rb| {
        let b = inflate(r, BUFFER);
        foreign
            .iter()
            .any(|(id, nr)| id != label && rects_overlap(&b, nr))
    };
    let over_edge = |eid: &str, r: &Rb| {
        let b = inflate(r, BUFFER);
        all_segs
            .iter()
            .any(|(id, p1, p2)| id != eid && seg_overlaps_rect(*p1, *p2, &b, 0.0))
    };
    let containing_lane = |r: &Rb| {
        lane_groups
            .iter()
            .find(|(_, lr)| rect_contains(lr, r))
            .map(|(id, _)| id.clone())
    };
    let mut placed: Vec<(String, Rb)> = Vec::new();
    let index = node_index(&data.nodes);
    for ei in 0..data.edges.len() {
        let edge = &data.edges[ei];
        if edge.is_layout_only {
            continue;
        }
        let Some(label_id) = edge.label_node_id.clone() else {
            continue;
        };
        let Some(&li) = index.get(&label_id) else {
            continue;
        };
        let pts = pts_of(edge).to_vec();
        if pts.len() < 2 {
            continue;
        }
        let eid = edge.id.clone();
        let lw = data.nodes[li].width.unwrap_or(0.0);
        let lh = data.nodes[li].height.unwrap_or(0.0);
        if lw <= 0.0 || lh <= 0.0 {
            continue;
        }
        // (idx, length, horizontal, midX, midY)
        struct Cand {
            idx: usize,
            len: f64,
            horizontal: bool,
            mx: f64,
            my: f64,
        }
        let mut segs: Vec<Cand> = Vec::new();
        for i in 0..pts.len() - 1 {
            let (a, b) = (pts[i], pts[i + 1]);
            let dx = (a.x - b.x).abs();
            let dy = (a.y - b.y).abs();
            if dx < EPS && dy < EPS {
                continue;
            }
            if dx >= EPS && dy >= EPS {
                continue;
            }
            segs.push(Cand {
                idx: i,
                len: dx + dy,
                horizontal: dx >= EPS,
                mx: (a.x + b.x) / 2.0,
                my: (a.y + b.y) / 2.0,
            });
        }
        if segs.is_empty() {
            continue;
        }
        let all: Vec<usize> = (0..segs.len()).collect();
        let middle: Vec<usize> = if segs.len() >= 3 {
            (0..segs.len())
                .filter(|&k| segs[k].idx > 0 && segs[k].idx < segs.len() - 1)
                .collect()
        } else {
            all.clone()
        };
        let pool_base = if middle.is_empty() {
            all.clone()
        } else {
            middle
        };
        let long_h = lw >= lh;
        let rank = |pool: &[usize]| -> Vec<usize> {
            let mut r = pool.to_vec();
            r.sort_by(|&a, &b| {
                let (sa, sb) = (&segs[a], &segs[b]);
                let al = sa.horizontal == long_h;
                let bl = sb.horizontal == long_h;
                if al != bl {
                    return if al {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    };
                }
                let af = sa.len >= if sa.horizontal { lw } else { lh } + 2.0;
                let bf = sb.len >= if sb.horizontal { lw } else { lh } + 2.0;
                if af != bf {
                    return if af {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    };
                }
                super::model::num_cmp(sb.len, sa.len)
            });
            r
        };
        let first_vis = 0;
        let last_vis = segs.len() - 1;
        let at_t = |k: usize, t: f64| {
            let (a, b) = (pts[segs[k].idx], pts[segs[k].idx + 1]);
            pt(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
        };
        let inside_incl = |q: Point, r: &Rb| {
            q.x >= r.left - EPS
                && q.x <= r.right + EPS
                && q.y >= r.top - EPS
                && q.y <= r.bottom + EPS
        };
        let placement = |anchor: Point| -> Option<(String, Point, Rb)> {
            let cr = rect_from_center_size(anchor.x, anchor.y, lw, lh);
            if let Some(l) = containing_lane(&cr) {
                return Some((l, anchor, cr));
            }
            let (lid, lr) = lane_groups.iter().find(|(_, r)| inside_incl(anchor, r))?;
            let min_x = lr.left + lw / 2.0 + LANE_MARGIN;
            let max_x = lr.right - lw / 2.0 - LANE_MARGIN;
            let min_y = lr.top + lh / 2.0 + LANE_MARGIN;
            let max_y = lr.bottom - lh / 2.0 - LANE_MARGIN;
            if min_x > max_x || min_y > max_y {
                return None;
            }
            let ca = pt(
                anchor.x.max(min_x).min(max_x),
                anchor.y.max(min_y).min(max_y),
            );
            let crr = rect_from_center_size(ca.x, ca.y, lw, lh);
            inside_incl(anchor, &crr).then(|| (lid.clone(), ca, crr))
        };
        let clears_ends = |k: usize, anchor: Point| {
            let s = &segs[k];
            let half = if s.horizontal { lw / 2.0 } else { lh / 2.0 };
            let req = half + ENDPOINT_CLEARANCE;
            let dist = |q: Point| {
                if s.horizontal {
                    (anchor.x - q.x).abs()
                } else {
                    (anchor.y - q.y).abs()
                }
            };
            if k == first_vis && dist(pts[s.idx]) + EPS < req {
                return false;
            }
            if k == last_vis && dist(pts[s.idx + 1]) + EPS < req {
                return false;
            }
            true
        };
        let overlaps_placed = |placed: &[(String, Rb)], r: &Rb| {
            placed
                .iter()
                .any(|(id, pr)| *id != label_id && rects_overlap(r, pr))
        };
        let try_pool = |pool: &[usize], placed: &[(String, Rb)]| -> Option<(String, Point)> {
            for k in rank(pool) {
                for t in TS {
                    let a = at_t(k, t);
                    if !clears_ends(k, a) {
                        continue;
                    }
                    let Some((lane, anchor, r)) = placement(a) else {
                        continue;
                    };
                    if label_overlaps_own_marker(&r, &pts) || overlaps_placed(placed, &r) {
                        continue;
                    }
                    if !(over_node(&label_id, &r) || over_edge(&eid, &r)) {
                        return Some((lane, anchor));
                    }
                }
            }
            None
        };
        let fallback = |pool: &[usize], placed: &[(String, Rb)], req: bool, allow_edge: bool| {
            for k in rank(pool) {
                let a = pt(segs[k].mx, segs[k].my);
                if req && !clears_ends(k, a) {
                    continue;
                }
                if let Some((lane, anchor, r)) = placement(a) {
                    if !label_overlaps_own_marker(&r, &pts)
                        && !overlaps_placed(placed, &r)
                        && !over_node(&label_id, &r)
                        && (allow_edge || !over_edge(&eid, &r))
                    {
                        return Some((lane, anchor));
                    }
                }
            }
            None
        };
        let chosen = try_pool(&pool_base, &placed)
            .or_else(|| {
                (pool_base.len() < segs.len())
                    .then(|| try_pool(&all, &placed))
                    .flatten()
            })
            .or_else(|| fallback(&all, &placed, true, false))
            .or_else(|| fallback(&all, &placed, false, false))
            .or_else(|| fallback(&all, &placed, false, true));
        if let Some((lane, anchor)) = chosen {
            let n = &mut data.nodes[li];
            n.x = Some(anchor.x);
            n.y = Some(anchor.y);
            n.parent_id = Some(lane);
            let r = rect_from_center_size(anchor.x, anchor.y, lw, lh);
            match placed.iter_mut().find(|(id, _)| *id == label_id) {
                Some(p) => p.1 = r,
                None => placed.push((label_id, r)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// endpointClip
// ---------------------------------------------------------------------------

const CLIP_EPS: f64 = 1e-3;
const INSIDE_EPS: f64 = 0.5;
const CORNER_CLEARANCE: f64 = 4.0;

fn end_rects(
    data: &LayoutData,
    index: &HashMap<String, usize>,
    e: &LEdge,
) -> (Option<Rb>, Option<Rb>) {
    let r = |id: &str| index.get(id).and_then(|&i| rect_of(&data.nodes[i]));
    (r(&e.start), r(&e.end))
}

fn segment_enter_point(outside: Point, inside: Point, r: &Rb) -> Point {
    if same_y(outside, inside, CLIP_EPS) {
        return pt(if outside.x < r.left { r.left } else { r.right }, outside.y);
    }
    if same_x(outside, inside, CLIP_EPS) {
        return pt(outside.x, if outside.y < r.top { r.top } else { r.bottom });
    }
    pt(
        r.right.min(r.left.max(outside.x)),
        r.bottom.min(r.top.max(outside.y)),
    )
}

fn clip_endpoint(points: Vec<Point>, r: &Rb, at_start: bool) -> Vec<Point> {
    let n = points.len() as i64;
    let step: i64 = if at_start { 1 } else { -1 };
    let mut oi: i64 = if at_start { 0 } else { n - 1 };
    while oi >= 0 && oi < n && point_inside_rect(points[oi as usize], r, INSIDE_EPS) {
        oi += step;
    }
    if oi < 0 || oi >= n {
        return points;
    }
    let ii = oi - step;
    if ii < 0 || ii >= n {
        return points;
    }
    let entry = segment_enter_point(points[oi as usize], points[ii as usize], r);
    if at_start {
        let mut v = vec![entry];
        v.extend_from_slice(&points[oi as usize..]);
        v
    } else {
        let mut v = points[..=oi as usize].to_vec();
        v.push(entry);
        v
    }
}

/// `clipEdgeEndpointsToNodeBoundaries`.
pub fn clip_edge_endpoints(data: &mut LayoutData) {
    let index = node_index(&data.nodes);
    for ei in 0..data.edges.len() {
        let e = &data.edges[ei];
        if e.is_layout_only || pts_of(e).len() < 2 {
            continue;
        }
        let (sr, dr) = end_rects(data, &index, e);
        let mut next = pts_of(e).to_vec();
        if let Some(r) = &sr {
            next = clip_endpoint(next, r, true);
        }
        if let Some(r) = &dr {
            next = clip_endpoint(next, r, false);
        }
        next = simplify(&orthogonalize(&next));
        next = clear_straight_corner_connections(&next, sr.as_ref(), dr.as_ref()).unwrap_or(next);
        data.edges[ei].points = Some(simplify(&orthogonalize(&next)));
    }
}

/// `snapEndpointToBoundary`: the snapped point, `None` when it stays as it is.
fn snap_endpoint(inner: Point, endpoint: Point, r: &Rb, approach: bool) -> Option<Point> {
    if same_y(inner, endpoint, CLIP_EPS) {
        if endpoint.y < r.top - CLIP_EPS || endpoint.y > r.bottom + CLIP_EPS {
            return None;
        }
        if approach {
            if inner.x < r.left - CLIP_EPS {
                return Some(pt(r.left, inner.y));
            }
            if inner.x > r.right + CLIP_EPS {
                return Some(pt(r.right, inner.y));
            }
        }
        let to_left = (endpoint.x - r.left).abs() <= (endpoint.x - r.right).abs();
        return Some(pt(if to_left { r.left } else { r.right }, inner.y));
    }
    if same_x(inner, endpoint, CLIP_EPS) {
        if endpoint.x < r.left - CLIP_EPS || endpoint.x > r.right + CLIP_EPS {
            return None;
        }
        if approach {
            if inner.y < r.top - CLIP_EPS {
                return Some(pt(inner.x, r.top));
            }
            if inner.y > r.bottom + CLIP_EPS {
                return Some(pt(inner.x, r.bottom));
            }
        }
        let to_top = (endpoint.y - r.top).abs() <= (endpoint.y - r.bottom).abs();
        return Some(pt(inner.x, if to_top { r.top } else { r.bottom }));
    }
    None
}

fn first_distinct_adjacent(points: &[Point], ei: usize, forward: bool) -> Option<Point> {
    let e = points[ei];
    let mut i = ei as i64 + if forward { 1 } else { -1 };
    while i >= 0 && (i as usize) < points.len() {
        let c = points[i as usize];
        if !same_point(c, e, CLIP_EPS) {
            return Some(c);
        }
        i += if forward { 1 } else { -1 };
    }
    let j = ei as i64 + if forward { 1 } else { -1 };
    (j >= 0 && (j as usize) < points.len()).then(|| points[j as usize])
}

fn corner_range(min: f64, max: f64) -> (f64, f64) {
    let lo = min + CORNER_CLEARANCE;
    let hi = max - CORNER_CLEARANCE;
    if lo <= hi {
        (lo, hi)
    } else {
        ((min + max) / 2.0, (min + max) / 2.0)
    }
}

fn range_for_side(r: &Rb, side: RectSide) -> (f64, f64) {
    if side.horizontal_face() {
        corner_range(r.top, r.bottom)
    } else {
        corner_range(r.left, r.right)
    }
}

fn terminal_side(end: Point, adj: Point, r: &Rb) -> Option<RectSide> {
    let y_within = end.y >= r.top - CLIP_EPS && end.y <= r.bottom + CLIP_EPS;
    let x_within = end.x >= r.left - CLIP_EPS && end.x <= r.right + CLIP_EPS;
    if same_y(end, adj, CLIP_EPS) && y_within {
        if (end.x - r.left).abs() < CLIP_EPS {
            return Some(RectSide::Left);
        }
        if (end.x - r.right).abs() < CLIP_EPS {
            return Some(RectSide::Right);
        }
    }
    if same_x(end, adj, CLIP_EPS) && x_within {
        if (end.y - r.top).abs() < CLIP_EPS {
            return Some(RectSide::Top);
        }
        if (end.y - r.bottom).abs() < CLIP_EPS {
            return Some(RectSide::Bottom);
        }
    }
    None
}

/// `clearStraightEndpointCornerConnections`: `None` when the points stay.
fn clear_straight_corner_connections(
    points: &[Point],
    sr: Option<&Rb>,
    dr: Option<&Rb>,
) -> Option<Vec<Point>> {
    if points.len() != 2 {
        return None;
    }
    let (start, end) = (points[0], points[1]);
    let horizontal = if same_y(start, end, CLIP_EPS) {
        true
    } else if same_x(start, end, CLIP_EPS) {
        false
    } else {
        return None;
    };
    let mut ranges = Vec::new();
    let ss = sr.and_then(|r| terminal_side(start, end, r));
    let ds = dr.and_then(|r| terminal_side(end, start, r));
    if let (Some(r), Some(s)) = (sr, ss) {
        if s.horizontal_face() == horizontal {
            ranges.push(range_for_side(r, s));
        }
    }
    if let (Some(r), Some(s)) = (dr, ds) {
        if s.horizontal_face() == horizontal {
            ranges.push(range_for_side(r, s));
        }
    }
    if ranges.is_empty() {
        return None;
    }
    let lo = ranges.iter().fold(f64::NEG_INFINITY, |a, r| a.max(r.0));
    let hi = ranges.iter().fold(f64::INFINITY, |a, r| a.min(r.1));
    if lo > hi {
        return None;
    }
    let cur = if horizontal { start.y } else { start.x };
    let next = hi.min(lo.max(cur));
    if (next - cur).abs() < CLIP_EPS {
        return None;
    }
    Some(if horizontal {
        vec![pt(start.x, next), pt(end.x, next)]
    } else {
        vec![pt(next, start.y), pt(next, end.y)]
    })
}

fn clear_endpoint_corner(points: Vec<Point>, r: &Rb, at_start: bool) -> Vec<Point> {
    if points.len() < 2 {
        return points;
    }
    let ei = if at_start { 0 } else { points.len() - 1 };
    let end = points[ei];
    let Some(adj) = first_distinct_adjacent(&points, ei, at_start) else {
        return points;
    };
    let Some(side) = terminal_side(end, adj, r) else {
        return points;
    };
    let hterm = side.horizontal_face();
    let clamp = |v: f64, min: f64, max: f64| {
        let (lo, hi) = corner_range(min, max);
        hi.min(lo.max(v))
    };
    let adjusted = if hterm {
        pt(end.x, clamp(end.y, r.top, r.bottom))
    } else {
        pt(clamp(end.x, r.left, r.right), end.y)
    };
    if same_point(end, adjusted, CLIP_EPS) {
        return points;
    }
    let mut next = points.clone();
    let mut i = ei as i64;
    while i >= 0 && (i as usize) < points.len() {
        let q = points[i as usize];
        if hterm && !same_y(q, end, CLIP_EPS) {
            break;
        }
        if !hterm && !same_x(q, end, CLIP_EPS) {
            break;
        }
        if hterm {
            next[i as usize].y = adjusted.y;
        } else {
            next[i as usize].x = adjusted.x;
        }
        i += if at_start { 1 } else { -1 };
    }
    next
}

fn border_side_for_segment(a: Point, b: Point, r: &Rb) -> Option<RectSide> {
    let xw = a.x.min(b.x) >= r.left - CLIP_EPS && a.x.max(b.x) <= r.right + CLIP_EPS;
    let yw = a.y.min(b.y) >= r.top - CLIP_EPS && a.y.max(b.y) <= r.bottom + CLIP_EPS;
    let e = CLIP_EPS;
    if (a.y - r.top).abs() < e && (b.y - r.top).abs() < e && xw {
        return Some(RectSide::Top);
    }
    if (a.y - r.bottom).abs() < e && (b.y - r.bottom).abs() < e && xw {
        return Some(RectSide::Bottom);
    }
    if (a.x - r.left).abs() < e && (b.x - r.left).abs() < e && yw {
        return Some(RectSide::Left);
    }
    if (a.x - r.right).abs() < e && (b.x - r.right).abs() < e && yw {
        return Some(RectSide::Right);
    }
    None
}

fn leaves_outward(side: RectSide, from: Point, to: Point, r: &Rb) -> bool {
    match side {
        RectSide::Top => same_x(from, to, CLIP_EPS) && to.y < r.top - CLIP_EPS,
        RectSide::Bottom => same_x(from, to, CLIP_EPS) && to.y > r.bottom + CLIP_EPS,
        RectSide::Left => same_y(from, to, CLIP_EPS) && to.x < r.left - CLIP_EPS,
        RectSide::Right => same_y(from, to, CLIP_EPS) && to.x > r.right + CLIP_EPS,
    }
}

fn collapse_own_border_stub(points: Vec<Point>, r: &Rb, at_start: bool) -> Vec<Point> {
    if points.len() < 3 {
        return points;
    }
    if at_start {
        if let Some(s) = border_side_for_segment(points[0], points[1], r) {
            if leaves_outward(s, points[1], points[2], r) {
                return points[1..].to_vec();
            }
        }
        return points;
    }
    let l = points.len() - 1;
    if let Some(s) = border_side_for_segment(points[l - 1], points[l], r) {
        if leaves_outward(s, points[l - 1], points[l - 2], r) {
            return points[..l].to_vec();
        }
    }
    points
}

fn snap_and_collapse(points: Vec<Point>, sr: Option<&Rb>, dr: Option<&Rb>) -> Vec<Point> {
    let mut next = points;
    if let Some(r) = sr {
        if let Some(adj) = first_distinct_adjacent(&next, 0, true) {
            if let Some(s) = snap_endpoint(adj, next[0], r, false) {
                next[0] = s;
            }
        }
        next = collapse_own_border_stub(next, r, true);
    }
    if let Some(r) = dr {
        let last = next.len() - 1;
        if let Some(adj) = first_distinct_adjacent(&next, last, false) {
            if let Some(s) = snap_endpoint(adj, next[last], r, true) {
                next[last] = s;
            }
        }
        next = collapse_own_border_stub(next, r, false);
    }
    let cleared = clear_straight_corner_connections(&next, sr, dr);
    if cleared.is_some() || next.len() == 2 {
        return cleared.unwrap_or(next);
    }
    if let Some(r) = sr {
        next = clear_endpoint_corner(next, r, true);
    }
    if let Some(r) = dr {
        next = clear_endpoint_corner(next, r, false);
    }
    next
}

/// `prepareEdgeEndpointsForRenderer`.
pub fn prepare_edge_endpoints(data: &mut LayoutData) {
    let index = node_index(&data.nodes);
    for ei in 0..data.edges.len() {
        let e = &data.edges[ei];
        if e.is_layout_only || pts_of(e).len() < 2 {
            continue;
        }
        let (sr, dr) = end_rects(data, &index, e);
        let input = dedupe(pts_of(e), CLIP_EPS);
        let np = snap_and_collapse(input, sr.as_ref(), dr.as_ref());
        if np.len() < 3 {
            data.edges[ei].points = Some(np);
            continue;
        }
        let mut d = vec![np[0], np[0]];
        d.extend_from_slice(&np[1..np.len() - 1]);
        d.push(np[np.len() - 1]);
        d.push(np[np.len() - 1]);
        data.edges[ei].points = Some(d);
    }
}

// ---------------------------------------------------------------------------
// terminalStub
// ---------------------------------------------------------------------------

/// `collapseShortTerminalStub`.
pub fn collapse_short_terminal_stub(data: &mut LayoutData) {
    const MIN_STUB: f64 = 10.0;
    const E: f64 = 1e-3;
    const BUFFER: f64 = 2.0;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let index = node_index(&data.nodes);
    for ei in 0..data.edges.len() {
        let edge = &data.edges[ei];
        if edge.is_layout_only {
            continue;
        }
        let raw = pts_of(edge);
        if raw.len() < 4 {
            continue;
        }
        let pts = dedupe(raw, E);
        if pts.len() < 4 {
            continue;
        }
        let nl = pts.len() - 1;
        let (end, pen, prev) = (pts[nl], pts[nl - 1], pts[nl - 2]);
        let last_len = (end.x - pen.x).hypot(end.y - pen.y);
        if !(E..MIN_STUB).contains(&last_len) {
            continue;
        }
        let pdx = pen.x - prev.x;
        let pdy = pen.y - prev.y;
        if pdx.hypot(pdy) < E {
            continue;
        }
        let lh = is_h(pen, end, E);
        let lv = is_v(pen, end, E);
        let ph = is_h(prev, pen, E);
        let pv = is_v(prev, pen, E);
        if !((lh && pv) || (lv && ph)) {
            continue;
        }
        let (dst_id, src_id) = (edge.end.clone(), edge.start.clone());
        let Some(&di) = index.get(&dst_id) else {
            continue;
        };
        let dst = &data.nodes[di];
        let (dcx, dcy) = (dst.x.unwrap_or(0.0), dst.y.unwrap_or(0.0));
        let Some(dr) = rect_of(dst) else { continue };
        let (new_prev, new_end) = if pv {
            let from_below = pdy < 0.0;
            (
                pt(dcx, prev.y),
                pt(dcx, if from_below { dr.bottom } else { dr.top }),
            )
        } else {
            let from_left = pdx > 0.0;
            (
                pt(prev.x, dcy),
                pt(if from_left { dr.right } else { dr.left }, dcy),
            )
        };
        if seg_hits_any_rect(new_prev, new_end, &real, &[&dst_id], -BUFFER) {
            continue;
        }
        if seg_hits_any_rect(new_prev, new_end, &labels, &[], -BUFFER) {
            continue;
        }
        if let Some(sr) = index.get(&src_id).and_then(|&i| rect_of(&data.nodes[i])) {
            if point_inside_rect(new_prev, &sr, BUFFER) {
                continue;
            }
        }
        let key = |a: Point, b: Point| {
            format!(
                "{},{}|{},{}",
                to_fixed(a.x, 3),
                to_fixed(a.y, 3),
                to_fixed(b.x, 3),
                to_fixed(b.y, 3)
            )
        };
        let own: std::collections::HashSet<String> = (0..pts.len() - 1)
            .map(|i| key(pts[i], pts[i + 1]))
            .collect();
        let crosses_other = |from: Point, to: Point| {
            data.edges.iter().enumerate().any(|(oi, o)| {
                if oi == ei || o.is_layout_only {
                    return false;
                }
                let op = pts_of(o);
                (0..op.len().saturating_sub(1)).any(|i| {
                    !own.contains(&key(op[i], op[i + 1]))
                        && segs_strictly_cross(from, to, op[i], op[i + 1], E)
                })
            })
        };
        if crosses_other(new_prev, new_end) {
            continue;
        }
        if nl >= 3 {
            let before = pts[nl - 3];
            if seg_hits_any_rect(before, new_prev, &real, &[&src_id, &dst_id], -BUFFER) {
                continue;
            }
            if crosses_other(before, new_prev) {
                continue;
            }
        }
        let mut np: Vec<Point> = pts[..nl - 2].to_vec();
        np.push(new_prev);
        np.push(new_end);
        let label = data.edges[ei].label_node_id.clone();
        data.edges[ei].points = Some(np.clone());
        if let Some(li) = label.and_then(|l| index.get(&l).copied()) {
            let lw = data.nodes[li].width.unwrap_or(0.0);
            let lhh = data.nodes[li].height.unwrap_or(0.0);
            if lw > 0.0 && lhh > 0.0 {
                let mut best: Option<Point> = None;
                let mut best_len = -1.0;
                for i in 0..np.len() - 1 {
                    let (a, b) = (np[i], np[i + 1]);
                    let len = (b.x - a.x).hypot(b.y - a.y);
                    let hh = same_y(a, b, E);
                    let vv = same_x(a, b, E);
                    let fits = (hh && len >= lw + 2.0) || (vv && len >= lhh + 2.0);
                    if !fits {
                        continue;
                    }
                    if len > best_len {
                        best_len = len;
                        best = Some(pt((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
                    }
                }
                if let Some(b) = best {
                    data.nodes[li].x = Some(b.x);
                    data.nodes[li].y = Some(b.y);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// sharedTrackNudging
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct NSeg {
    s: Seg,
    edge: usize,
    interior: bool,
}

fn nsegs(edge: usize, points: &[Point]) -> Vec<NSeg> {
    segments(points, GEPS)
        .into_iter()
        .map(|s| NSeg {
            s,
            edge,
            interior: s.index >= 1 && (s.index as i64) <= points.len() as i64 - 3,
        })
        .collect()
}

fn crowded_parallel(a: &Seg, b: &Seg) -> bool {
    const MIN_SHARED: f64 = 8.0;
    const GAP: f64 = 7.0;
    if a.horizontal && b.horizontal {
        return overlap_length(a.a.x, a.b.x, b.a.x, b.b.x) >= MIN_SHARED
            && (a.a.y - b.a.y).abs() < GAP;
    }
    if a.vertical && b.vertical {
        return overlap_length(a.a.y, a.b.y, b.a.y, b.b.y) >= MIN_SHARED
            && (a.a.x - b.a.x).abs() < GAP;
    }
    false
}

/// `nudgeSharedInteriorSubpaths`.
pub fn nudge_shared_interior_subpaths(data: &mut LayoutData) {
    const SHIFT: f64 = 7.0;
    const STUB: f64 = 20.0;
    const BUFFER: f64 = 2.0;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let index = node_index(&data.nodes);
    let safe = |edges: &[LEdge], ei: usize, cand: &[Point]| -> bool {
        let cs = nsegs(ei, cand);
        if cs.len() + 1 != cand.len() {
            return false;
        }
        let e = &edges[ei];
        let ends: Vec<&str> = [e.start.as_str(), e.end.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let own: Vec<&str> = e.label_node_id.iter().map(String::as_str).collect();
        for s in &cs {
            if seg_hits_any_rect(s.s.a, s.s.b, &real, &ends, -BUFFER) {
                return false;
            }
            if seg_hits_any_rect(s.s.a, s.s.b, &labels, &own, -BUFFER) {
                return false;
            }
        }
        for (oi, o) in edges.iter().enumerate() {
            if oi == ei || o.is_layout_only {
                continue;
            }
            let op = pts_of(o);
            if op.len() < 2 {
                continue;
            }
            let os = nsegs(oi, &dedupe(op, GEPS));
            for c in &cs {
                for s in &os {
                    if crowded_parallel(&c.s, &s.s) {
                        return false;
                    }
                    if segs_strictly_cross(c.s.a, c.s.b, s.s.a, s.s.b, GEPS) {
                        return false;
                    }
                }
            }
        }
        true
    };
    let shifted = |edges: &[LEdge], s: &NSeg, shift: f64| -> Option<Vec<Point>> {
        let points = dedupe(pts_of(&edges[s.edge]), GEPS);
        if points.len() < 4 || s.s.index >= points.len() - 1 {
            return None;
        }
        let mut c = points.clone();
        if s.s.horizontal {
            c[s.s.index].y += shift;
            c[s.s.index + 1].y += shift;
        } else if s.s.vertical {
            c[s.s.index].x += shift;
            c[s.s.index + 1].x += shift;
        } else {
            return None;
        }
        (nsegs(s.edge, &c).len() + 1 == c.len()).then_some(c)
    };
    let detour = |data: &LayoutData, s: &NSeg, shift: f64| -> Option<Vec<Point>> {
        let e = &data.edges[s.edge];
        let points = dedupe(pts_of(e), GEPS);
        if points.len() != 4 || s.s.index != 1 {
            return None;
        }
        let sn = index.get(&e.start).map(|&i| &data.nodes[i])?;
        let tn = index.get(&e.end).map(|&i| &data.nodes[i])?;
        let sr = rect_of(sn)?;
        let tr = rect_of(tn)?;
        let tail: Vec<Point> = points[s.s.index + 2..].to_vec();
        if tail.is_empty() {
            return None;
        }
        let sc = pt(
            sn.x.unwrap_or((sr.left + sr.right) / 2.0),
            sn.y.unwrap_or((sr.top + sr.bottom) / 2.0),
        );
        let tc = pt(
            tn.x.unwrap_or((tr.left + tr.right) / 2.0),
            tn.y.unwrap_or((tr.top + tr.bottom) / 2.0),
        );
        let e = GEPS;
        if s.s.vertical {
            let below = tc.y >= sc.y;
            let port_y = if below { sr.bottom } else { sr.top };
            let stub_y = port_y + if below { STUB } else { -STUB };
            if (below && s.s.b.y <= stub_y + e) || (!below && s.s.b.y >= stub_y - e) {
                return None;
            }
            let rail_x = s.s.a.x + shift;
            let mut v = vec![
                pt(sc.x, port_y),
                pt(sc.x, stub_y),
                pt(rail_x, stub_y),
                pt(rail_x, s.s.b.y),
            ];
            v.extend(tail);
            return Some(dedupe(&v, e));
        }
        if s.s.horizontal {
            let right = tc.x >= sc.x;
            let port_x = if right { sr.right } else { sr.left };
            let stub_x = port_x + if right { STUB } else { -STUB };
            if (right && s.s.b.x <= stub_x + e) || (!right && s.s.b.x >= stub_x - e) {
                return None;
            }
            let rail_y = s.s.a.y + shift;
            let mut v = vec![
                pt(port_x, sc.y),
                pt(stub_x, sc.y),
                pt(stub_x, rail_y),
                pt(s.s.b.x, rail_y),
            ];
            v.extend(tail);
            return Some(dedupe(&v, e));
        }
        None
    };
    let shifts = [
        -SHIFT,
        SHIFT,
        -2.0 * SHIFT,
        2.0 * SHIFT,
        -3.0 * SHIFT,
        3.0 * SHIFT,
    ];
    for _ in 0..12 {
        let mut all: Vec<NSeg> = Vec::new();
        for (ei, e) in data.edges.iter().enumerate() {
            if e.is_layout_only {
                continue;
            }
            let p = pts_of(e);
            if p.len() < 2 {
                continue;
            }
            all.extend(nsegs(ei, &dedupe(p, GEPS)));
        }
        let mut fixed = false;
        'outer: for i in 0..all.len() {
            for j in i + 1..all.len() {
                let (f, s) = (all[i], all[j]);
                if f.edge == s.edge || !crowded_parallel(&f.s, &s.s) {
                    continue;
                }
                for seg in [f, s].iter().filter(|x| x.interior) {
                    for &sh in &shifts {
                        if let Some(d) = shifted(&data.edges, seg, sh) {
                            if safe(&data.edges, seg.edge, &d) {
                                data.edges[seg.edge].points = Some(d);
                                fixed = true;
                                break;
                            }
                        }
                        if let Some(d) = detour(data, seg, sh) {
                            if safe(&data.edges, seg.edge, &d) {
                                data.edges[seg.edge].points = Some(d);
                                fixed = true;
                                break;
                            }
                        }
                    }
                    if fixed {
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
