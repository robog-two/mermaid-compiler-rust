//! `orthogonalRouter/router.ts` of mermaid 12.3: `routeEdgesOrthogonal`.
//!
//! Every edge gets ports on the faces of its nodes (spread when several edges
//! share a face), anchors 20px out, and a path between the anchors: straight when
//! the faces line up, an L when one is free, else A* over a grid of "pipes" laid
//! 15px around the nodes in the way. Segments that share a pipe are then spread
//! over parallel tracks so they neither overlap nor cross near their ends.

use super::model::{EPS, LNode, LayoutData, OMap, js_round, num_cmp, sign, to_fixed};
use crate::geom::Point;
use std::collections::{HashMap, HashSet};

const NODE_PADDING: f64 = 8.0;
const HORIZONTAL_PIPE_MARGIN: f64 = 15.0;
const VERTICAL_PIPE_MARGIN: f64 = 15.0;
const ROUTING_MARGIN: f64 = 25.0;
const ANCHOR_OFFSET: f64 = 20.0;
const TRACK_SPACING: f64 = 10.0;
const CROSSING_PENALTY: f64 = 1000.0;
const MIN_PORT_SPACING: f64 = 8.0;
const MAX_PORT_SPACING: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Bottom => "bottom",
            Side::Left => "left",
            Side::Right => "right",
        }
    }
    fn vertical(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }
}

fn p(x: f64, y: f64) -> Point {
    Point { x, y }
}

/// `chooseOrthogonalSide`.
fn choose_side(node: &LNode, target: Point, fallback: Side) -> Side {
    let dx = target.x - node.x.unwrap_or(0.0);
    let dy = target.y - node.y.unwrap_or(0.0);
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax < EPS && ay < EPS {
        return fallback;
    }
    if ay > EPS && ay * 3.0 >= ax {
        return if dy > 0.0 { Side::Bottom } else { Side::Top };
    }
    if ax > EPS {
        return if dx > 0.0 { Side::Right } else { Side::Left };
    }
    fallback
}

/// `drawnSize` (flowchart nodes carry no `drawnExtent`).
fn drawn_size(node: &LNode, fallback: f64) -> (f64, f64) {
    (
        node.width.unwrap_or(fallback),
        node.height.unwrap_or(fallback),
    )
}

#[derive(Debug, Clone)]
struct Obs {
    node_id: String,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    visual_x_half_extent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SegRef {
    edge: usize,
    seg: usize,
    from: f64,
    to: f64,
}

#[derive(Debug, Clone)]
struct Track {
    index: usize,
    segments: Vec<SegRef>,
}

#[derive(Debug, Clone)]
struct Pipe {
    horizontal: bool,
    coord: f64,
    span_min: f64,
    span_max: f64,
    /// Sparse like the JavaScript array (`tracks[idx] = ...`).
    tracks: Vec<Option<Track>>,
    /// Found by `getOrAddPipe` (a fast path's pipe is private to its edge).
    registered: bool,
}

#[derive(Debug, Clone)]
struct RSeg {
    edge: usize,
    seg: usize,
    horizontal: bool,
    pipe: usize,
    track: usize,
    from: f64,
    to: f64,
}

#[derive(Debug, Clone)]
struct SideInfo {
    edge: usize,
    src: String,
    dst: String,
    src_side: Side,
    dst_side: Side,
    abs_dx: f64,
    abs_dy: f64,
    dx_sign: f64,
    dy_sign: f64,
}

/// A segment on a track: (edge, segment, track, from, to).
type TrackSeg = (usize, usize, usize, f64, f64);

#[derive(Debug, Clone, Copy, Default)]
struct DestInfo {
    dest: f64,
    deviation: f64,
    delta: f64,
}

struct Router<'a> {
    pipes: Vec<Pipe>,
    segs: Vec<RSeg>,
    edge_segs: Vec<Vec<usize>>,
    obstacles: Vec<Obs>,
    node_by_id: HashMap<&'a str, &'a LNode>,
}

impl Router<'_> {
    fn get_or_add_pipe(&mut self, horizontal: bool, coord: f64, smin: f64, smax: f64) -> usize {
        let i = match self.pipes.iter().position(|p| {
            p.registered && p.horizontal == horizontal && (p.coord - coord).abs() < 1.0
        }) {
            Some(i) => i,
            None => {
                self.pipes.push(Pipe {
                    horizontal,
                    coord,
                    span_min: smin,
                    span_max: smax,
                    tracks: Vec::new(),
                    registered: true,
                });
                self.pipes.len() - 1
            }
        };
        let pipe = &mut self.pipes[i];
        pipe.span_min = pipe.span_min.min(smin);
        pipe.span_max = pipe.span_max.max(smax);
        i
    }

    fn crossing_penalty(&self, edge: usize, from: Point, to: Point) -> f64 {
        if self.segs.is_empty() {
            return 0.0;
        }
        let horizontal = (from.y - to.y).abs() < EPS;
        let vertical = (from.x - to.x).abs() < EPS;
        if !horizontal && !vertical {
            return 0.0;
        }
        let mut pen = 0.0;
        if horizontal {
            let y = from.y;
            let min_x = from.x.min(to.x) - EPS;
            let max_x = from.x.max(to.x) + EPS;
            if max_x <= min_x {
                return 0.0;
            }
            for s in &self.segs {
                if s.edge == edge || s.horizontal {
                    continue;
                }
                let c = self.pipes[s.pipe].coord;
                if c < min_x || c > max_x {
                    continue;
                }
                if s.from - EPS <= y && s.to + EPS >= y {
                    pen += CROSSING_PENALTY;
                }
            }
        } else {
            let x = from.x;
            let min_y = from.y.min(to.y) - EPS;
            let max_y = from.y.max(to.y) + EPS;
            if max_y <= min_y {
                return 0.0;
            }
            for s in &self.segs {
                if s.edge == edge || !s.horizontal {
                    continue;
                }
                let c = self.pipes[s.pipe].coord;
                if c < min_y || c > max_y {
                    continue;
                }
                if s.from - EPS <= x && s.to + EPS >= x {
                    pen += CROSSING_PENALTY;
                }
            }
        }
        pen
    }

    fn is_segment_blocked(&self, p1: Point, p2: Point, ex1: &str, ex2: &str) -> bool {
        let (sminx, smaxx) = (p1.x.min(p2.x), p1.x.max(p2.x));
        let (sminy, smaxy) = (p1.y.min(p2.y), p1.y.max(p2.y));
        self.obstacles.iter().any(|o| {
            if (!ex1.is_empty() && o.node_id == ex1) || (!ex2.is_empty() && o.node_id == ex2) {
                return false;
            }
            if (p1.x - p2.x).abs() > EPS {
                o.min_y < p1.y && o.max_y > p1.y && o.max_x > sminx && o.min_x < smaxx
            } else {
                o.min_x < p1.x && o.max_x > p1.x && o.max_y > sminy && o.min_y < smaxy
            }
        })
    }

    fn track_mut(&mut self, pipe: usize, track: usize) -> &mut Track {
        self.pipes[pipe].tracks[track].as_mut().expect("track")
    }

    fn try_swap(&mut self, s1: usize, s2: usize) -> bool {
        let (a, b) = (self.segs[s1].clone(), self.segs[s2].clone());
        let t1 = self.pipes[a.pipe].tracks[a.track].clone().expect("track");
        let t2 = self.pipes[b.pipe].tracks[b.track].clone().expect("track");
        let can1 = !t2
            .segments
            .iter()
            .any(|r| (r.edge != b.edge || r.seg != b.seg) && r.from < a.to && a.from < r.to);
        let can2 = !t1
            .segments
            .iter()
            .any(|r| (r.edge != a.edge || r.seg != a.seg) && r.from < b.to && b.from < r.to);
        if !(can1 && can2) {
            return false;
        }
        self.segs[s1].track = t2.index;
        self.segs[s2].track = t1.index;
        {
            let t = self.track_mut(a.pipe, a.track);
            t.segments.retain(|r| r.edge != a.edge || r.seg != a.seg);
            t.segments.push(SegRef {
                edge: b.edge,
                seg: b.seg,
                from: b.from,
                to: b.to,
            });
        }
        {
            let t = self.track_mut(b.pipe, b.track);
            t.segments.retain(|r| r.edge != b.edge || r.seg != b.seg);
            t.segments.push(SegRef {
                edge: a.edge,
                seg: a.seg,
                from: a.from,
                to: a.to,
            });
        }
        true
    }

    fn create_track(&mut self, pipe: usize) -> usize {
        let idx = self.pipes[pipe].tracks.len();
        self.pipes[pipe].tracks.push(Some(Track {
            index: idx,
            segments: Vec::new(),
        }));
        idx
    }

    fn move_to_track(&mut self, s: usize, track: usize) {
        let seg = self.segs[s].clone();
        self.track_mut(seg.pipe, seg.track)
            .segments
            .retain(|r| r.edge != seg.edge || r.seg != seg.seg);
        self.segs[s].track = track;
        self.track_mut(seg.pipe, track).segments.push(SegRef {
            edge: seg.edge,
            seg: seg.seg,
            from: seg.from,
            to: seg.to,
        });
    }

    fn move_chain_to_track(&mut self, s: usize, track: usize) {
        let pipe = self.segs[s].pipe;
        let indices = self.edge_segs[self.segs[s].edge].clone();
        for idx in indices {
            if self.segs[idx].pipe == pipe {
                self.move_to_track(idx, track);
            }
        }
    }

    fn adjacent(&self, s: usize) -> Vec<usize> {
        let indices = &self.edge_segs[self.segs[s].edge];
        let Some(i) = indices.iter().position(|&x| x == s) else {
            // `indexOf` of -1: the segment after "position -1" is the first one.
            return indices.first().copied().into_iter().collect();
        };
        let mut adj = Vec::new();
        if i > 0 {
            adj.push(indices[i - 1]);
        }
        if i + 1 < indices.len() {
            adj.push(indices[i + 1]);
        }
        adj
    }

    fn have_any_crossing(&self, a: usize, b: usize) -> bool {
        let (sa, sb) = (&self.segs[a], &self.segs[b]);
        if sa.horizontal == sb.horizontal {
            return false;
        }
        let (h, v) = if sa.horizontal { (sa, sb) } else { (sb, sa) };
        let vc = self.pipes[v.pipe].coord;
        let hc = self.pipes[h.pipe].coord;
        vc > h.from && vc < h.to && hc > v.from && hc < v.to
    }

    fn find_available_track(&self, pipe: usize, s: usize) -> Option<usize> {
        let seg = &self.segs[s];
        for t in self.pipes[pipe].tracks.iter().flatten() {
            let overlap = t.segments.iter().any(|r| {
                (r.edge != seg.edge || r.seg != seg.seg) && r.from < seg.to && seg.from < r.to
            });
            if !overlap {
                return Some(t.index);
            }
        }
        None
    }

    fn segments_conflict(&self, s1: usize, s2: usize) -> bool {
        let (a, b) = (&self.segs[s1], &self.segs[s2]);
        if a.track == b.track {
            return a.from < b.to && b.from < a.to;
        }
        let adj1 = self.adjacent(s1);
        let adj2 = self.adjacent(s2);
        adj1.iter()
            .any(|&x| adj2.iter().any(|&y| self.have_any_crossing(x, y)))
    }

    fn resolve_conflict(&mut self, s1: usize, s2: usize, chain: bool) {
        if self.try_swap(s1, s2) {
            return;
        }
        let pipe = self.segs[s1].pipe;
        let t = match self.find_available_track(pipe, s2) {
            Some(t) => t,
            None => self.create_track(pipe),
        };
        if chain {
            self.move_chain_to_track(s2, t);
        } else {
            self.move_to_track(s2, t);
        }
    }

    fn resolve_handle_conflicts(&mut self, handles: &[usize]) -> usize {
        let mut crossings = 0;
        for i in 0..handles.len() {
            for j in i + 1..handles.len() {
                let (h1, h2) = (handles[i], handles[j]);
                if self.segs[h1].pipe != self.segs[h2].pipe {
                    continue;
                }
                if self.segments_conflict(h1, h2) {
                    crossings += 1;
                    self.resolve_conflict(h1, h2, true);
                }
            }
        }
        crossings
    }
}

/// `routeEdgesOrthogonal`: sets the points of every edge and drops layout-only edges.
pub fn route_edges_orthogonal(data: &mut LayoutData) {
    let is_lr = data.direction == "LR";
    let nodes = data.nodes.clone();
    // The routing view: every edge but the layout-only ones, by index into `data.edges`.
    let edge_idx: Vec<usize> = (0..data.edges.len())
        .filter(|&i| !data.edges[i].is_layout_only)
        .collect();
    let edges: Vec<super::model::LEdge> = edge_idx.iter().map(|&i| data.edges[i].clone()).collect();
    let mut points: Vec<Option<Vec<Point>>> = edges.iter().map(|e| e.points.clone()).collect();

    let node_by_id: HashMap<&str, &LNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    // Lanes: top-level groups, and every descendant of one.
    let lanes = super::sugiyama::LaneModel::new(&nodes);
    let mut lane_by_node: HashMap<String, String> = HashMap::new();
    for g in nodes.iter().filter(|n| lanes.is_lane(&n.id)) {
        let mut stack = vec![g.id.clone()];
        while let Some(id) = stack.pop() {
            lane_by_node.insert(id.clone(), g.id.clone());
            for c in nodes
                .iter()
                .filter(|c| c.parent_id.as_deref() == Some(id.as_str()))
            {
                stack.push(c.id.clone());
            }
        }
    }
    let obstacles: Vec<Obs> = nodes
        .iter()
        .filter(|n| !n.is_group && !n.is_edge_label)
        .map(|n| {
            let w = n.width.unwrap_or(10.0);
            let h = n.height.unwrap_or(10.0);
            let x = n.x.unwrap_or(0.0);
            let y = n.y.unwrap_or(0.0);
            Obs {
                node_id: n.id.clone(),
                min_x: x - w / 2.0 - NODE_PADDING,
                max_x: x + w / 2.0 + NODE_PADDING,
                min_y: y - h / 2.0 - NODE_PADDING,
                max_y: y + h / 2.0 + NODE_PADDING,
                visual_x_half_extent: if is_lr {
                    h / 2.0 + NODE_PADDING
                } else {
                    w / 2.0 + NODE_PADDING
                },
            }
        })
        .collect();
    let mut r = Router {
        pipes: Vec::new(),
        segs: Vec::new(),
        edge_segs: vec![Vec::new(); edges.len()],
        obstacles,
        node_by_id: node_by_id.clone(),
    };

    let port_for_side = |node: &LNode, side: Side| -> Point {
        let (w, h) = drawn_size(node, 10.0);
        let cx = node.x.unwrap_or(0.0);
        let cy = node.y.unwrap_or(0.0);
        match side {
            Side::Top => p(cx, cy - h / 2.0),
            Side::Bottom => p(cx, cy + h / 2.0),
            Side::Left => p(cx - w / 2.0, cy),
            Side::Right => p(cx + w / 2.0, cy),
        }
    };
    let pos = |n: &LNode| p(n.x.unwrap_or(0.0), n.y.unwrap_or(0.0));

    let mut straight_edges: HashSet<usize> = HashSet::new();

    // Routing order: cross-lane edges first, then shorter ones.
    let mut order: Vec<(usize, i32, f64, f64)> = edges
        .iter()
        .enumerate()
        .map(|(idx, e)| {
            let src = node_by_id.get(e.start.as_str());
            let dst = node_by_id.get(e.end.as_str());
            let sl = lane_by_node.get(&e.start);
            let dl = lane_by_node.get(&e.end);
            let cross = i32::from(matches!((sl, dl), (Some(a), Some(b)) if a != b));
            let (dx, dy) = match (src, dst) {
                (Some(s), Some(d)) => (
                    (d.x.unwrap_or(0.0) - s.x.unwrap_or(0.0)).abs(),
                    (d.y.unwrap_or(0.0) - s.y.unwrap_or(0.0)).abs(),
                ),
                _ => (0.0, 0.0),
            };
            (idx, cross, dx, dy)
        })
        .collect();
    order.sort_by(|a, b| {
        if a.1 != b.1 {
            return b.1.cmp(&a.1);
        }
        let ad = a.2 + a.3;
        let bd = b.2 + b.3;
        if (ad - bd).abs() > 1.0 {
            return num_cmp(ad, bd);
        }
        a.0.cmp(&b.0)
    });
    let routing_order: Vec<usize> = order.iter().map(|o| o.0).collect();

    // Step 6: port pre-assignment.
    let mut incident: HashMap<&str, usize> = HashMap::new();
    for e in &edges {
        if e.start == e.end {
            continue;
        }
        *incident.entry(e.start.as_str()).or_default() += 1;
        *incident.entry(e.end.as_str()).or_default() += 1;
    }
    let determine_side = |node: &LNode, target: Point| choose_side(node, target, Side::Bottom);
    let mut side_info: Vec<Option<SideInfo>> = vec![None; edges.len()];
    for (i, e) in edges.iter().enumerate() {
        if e.start == e.end || points[i].as_ref().is_some_and(|p| !p.is_empty()) {
            continue;
        }
        let (Some(src), Some(dst)) = (
            node_by_id.get(e.start.as_str()),
            node_by_id.get(e.end.as_str()),
        ) else {
            continue;
        };
        let dx = dst.x.unwrap_or(0.0) - src.x.unwrap_or(0.0);
        let dy = dst.y.unwrap_or(0.0) - src.y.unwrap_or(0.0);
        side_info[i] = Some(SideInfo {
            edge: i,
            src: e.start.clone(),
            dst: e.end.clone(),
            src_side: determine_side(src, pos(dst)),
            dst_side: determine_side(dst, pos(src)),
            abs_dx: dx.abs(),
            abs_dy: dy.abs(),
            dx_sign: sign(dx),
            dy_sign: sign(dy),
        });
    }
    let strength = |info: &SideInfo| -> f64 {
        if info.src_side.vertical() {
            if info.abs_dx == 0.0 {
                f64::INFINITY
            } else {
                info.abs_dy / info.abs_dx
            }
        } else if info.abs_dy == 0.0 {
            f64::INFINITY
        } else {
            info.abs_dx / info.abs_dy
        }
    };
    let secondary_sides = |info: &SideInfo| -> Vec<Side> {
        if info.src_side.vertical() {
            if info.dx_sign == 0.0 {
                return vec![Side::Right, Side::Left];
            }
            return vec![if info.dx_sign > 0.0 {
                Side::Right
            } else {
                Side::Left
            }];
        }
        if info.dy_sign == 0.0 {
            return vec![Side::Bottom, Side::Top];
        }
        vec![if info.dy_sign > 0.0 {
            Side::Bottom
        } else {
            Side::Top
        }]
    };
    let mut groups: OMap<Vec<usize>> = OMap::new();
    for info in side_info.iter().flatten() {
        let key = format!("{}:{}", info.src, info.src_side.name());
        groups.entry_or(&key, Vec::new).push(info.edge);
    }
    let lk = |id: &str, s: Side| format!("{}:{}", id, s.name());
    let mut load: HashMap<String, i64> = HashMap::new();
    for info in side_info.iter().flatten() {
        *load.entry(lk(&info.src, info.src_side)).or_default() += 1;
        *load.entry(lk(&info.dst, info.dst_side)).or_default() += 1;
    }
    let get_load = |load: &HashMap<String, i64>, k: &str| load.get(k).copied().unwrap_or(0);
    for group in groups.values() {
        if group.len() < 2 {
            continue;
        }
        let mut grp = group.clone();
        grp.sort_by(|&a, &b| {
            let sa = strength(side_info[a].as_ref().unwrap());
            let sb = strength(side_info[b].as_ref().unwrap());
            let d = sa - sb;
            // `Math.abs(sa - sb) > 1e-9` is false for Infinity - Infinity (NaN).
            if d.abs() > 1e-9 {
                return num_cmp(sb, sa);
            }
            a.cmp(&b)
        });
        for &ei in grp.iter().skip(1) {
            let info = side_info[ei].clone().unwrap();
            let primary = get_load(&load, &lk(&info.src, info.src_side));
            let cands = secondary_sides(&info);
            let mut secondary = cands[0];
            for &s in &cands[1..] {
                if get_load(&load, &lk(&info.src, s)) < get_load(&load, &lk(&info.src, secondary)) {
                    secondary = s;
                }
            }
            let sec_load = get_load(&load, &lk(&info.src, secondary));
            if sec_load >= primary {
                continue;
            }
            load.insert(lk(&info.src, info.src_side), primary - 1);
            load.insert(lk(&info.src, secondary), sec_load + 1);
            side_info[ei].as_mut().unwrap().src_side = secondary;
        }
    }
    // Step 6.2b: a diamond's in- and out-edges on separate faces.
    let shape_of = |id: &str| node_by_id.get(id).map(|n| n.shape.as_str()).unwrap_or("");
    let mut in_sides: HashMap<String, HashSet<Side>> = HashMap::new();
    for info in side_info.iter().flatten() {
        in_sides
            .entry(info.dst.clone())
            .or_default()
            .insert(info.dst_side);
    }
    for i in 0..side_info.len() {
        let Some(info) = side_info[i].clone() else {
            continue;
        };
        let sh = shape_of(&info.src);
        if sh != "question" && sh != "diamond" {
            continue;
        }
        let Some(ins) = in_sides.get(&info.src) else {
            continue;
        };
        if !ins.contains(&info.src_side) {
            continue;
        }
        let secondary = secondary_sides(&info)[0];
        if ins.contains(&secondary) || get_load(&load, &lk(&info.src, secondary)) > 0 {
            continue;
        }
        let primary = get_load(&load, &lk(&info.src, info.src_side));
        load.insert(lk(&info.src, info.src_side), (primary - 1).max(0));
        load.insert(lk(&info.src, secondary), 1);
        side_info[i].as_mut().unwrap().src_side = secondary;
    }
    // Step 6.2c: an in-edge off the face an out-edge of a hexagon leaves.
    let mut out_sides: HashMap<String, HashSet<Side>> = HashMap::new();
    for info in side_info.iter().flatten() {
        out_sides
            .entry(info.src.clone())
            .or_default()
            .insert(info.src_side);
    }
    for i in 0..side_info.len() {
        let Some(info) = side_info[i].clone() else {
            continue;
        };
        let sh = shape_of(&info.dst);
        if sh != "hexagon" && sh != "notch-pent" {
            continue;
        }
        let Some(outs) = out_sides.get(&info.dst) else {
            continue;
        };
        if !outs.contains(&info.dst_side) {
            continue;
        }
        let secondary = if info.dst_side.vertical() {
            if info.dx_sign >= 0.0 {
                Side::Left
            } else {
                Side::Right
            }
        } else if info.dy_sign >= 0.0 {
            Side::Top
        } else {
            Side::Bottom
        };
        if outs.contains(&secondary) || get_load(&load, &lk(&info.dst, secondary)) > 0 {
            continue;
        }
        let primary = get_load(&load, &lk(&info.dst, info.dst_side));
        load.insert(lk(&info.dst, info.dst_side), (primary - 1).max(0));
        load.insert(lk(&info.dst, secondary), 1);
        side_info[i].as_mut().unwrap().dst_side = secondary;
        let facing = if info.dy_sign >= 0.0 {
            Side::Bottom
        } else {
            Side::Top
        };
        let src_in = in_sides.get(&info.src);
        if secondary.vertical()
            && src_in.is_some_and(|s| s.contains(&info.src_side))
            && !src_in.is_some_and(|s| s.contains(&facing))
            && get_load(&load, &lk(&info.src, facing)) == 0
        {
            let sl = get_load(&load, &lk(&info.src, info.src_side));
            load.insert(lk(&info.src, info.src_side), (sl - 1).max(0));
            load.insert(lk(&info.src, facing), 1);
            side_info[i].as_mut().unwrap().src_side = facing;
        }
    }
    // Step 6.3: port groups.
    let mut port_groups: OMap<Vec<(usize, f64)>> = OMap::new();
    for info in side_info.iter().flatten() {
        let src = node_by_id[info.src.as_str()];
        let dst = node_by_id[info.dst.as_str()];
        let dc = if info.src_side.vertical() {
            dst.x.unwrap_or(0.0)
        } else {
            dst.y.unwrap_or(0.0)
        };
        port_groups
            .entry_or(
                &format!("{}:{}:src", info.src, info.src_side.name()),
                Vec::new,
            )
            .push((info.edge, dc));
        let sc = if info.dst_side.vertical() {
            src.x.unwrap_or(0.0)
        } else {
            src.y.unwrap_or(0.0)
        };
        port_groups
            .entry_or(
                &format!("{}:{}:dst", info.dst, info.dst_side.name()),
                Vec::new,
            )
            .push((info.edge, sc));
    }
    let mut port_offsets: HashMap<(usize, bool), f64> = HashMap::new();
    let keys: Vec<String> = port_groups.keys().cloned().collect();
    for key in keys {
        let group = port_groups.get_mut(&key).unwrap();
        if group.len() < 2 {
            continue;
        }
        group.sort_by(|a, b| num_cmp(a.1, b.1));
        let group = group.clone();
        let mut parts = key.rsplitn(3, ':');
        let role = parts.next().unwrap_or("");
        let side = parts.next().unwrap_or("");
        let node_id = parts.next().unwrap_or("");
        let Some(node) = node_by_id.get(node_id) else {
            continue;
        };
        let vertical_side = side == "left" || side == "right";
        let (dw, dh) = drawn_size(node, 10.0);
        let side_len = if vertical_side { dh } else { dw };
        let eff = if node.shape == "question" || node.shape == "diamond" {
            side_len * 0.3
        } else {
            side_len
        };
        let n = group.len() as f64;
        let spacing = MAX_PORT_SPACING
            .min(eff / (n - 1.0))
            .min(MIN_PORT_SPACING.max(eff / (n + 1.0)));
        let start = -(spacing * (n - 1.0)) / 2.0;
        for (j, (ei, _)) in group.iter().enumerate() {
            port_offsets.insert((*ei, role == "src"), start + j as f64 * spacing);
        }
    }
    let face_has_label = |id: &str, side: Side| -> bool {
        ["src", "dst"].iter().any(|role| {
            port_groups
                .get(&format!("{}:{}:{}", id, side.name(), role))
                .is_some_and(|g| g.iter().any(|(ei, _)| edges[*ei].label_node_id.is_some()))
        })
    };
    let face_total = |id: &str, side: Side| -> usize {
        ["src", "dst"]
            .iter()
            .map(|role| {
                port_groups
                    .get(&format!("{}:{}:{}", id, side.name(), role))
                    .map(Vec::len)
                    .unwrap_or(0)
            })
            .sum()
    };
    let ports_for_edge = |i: usize, src: &LNode, dst: &LNode| -> (Point, Point, Side, Side) {
        let info = side_info[i].as_ref();
        let src_side = info
            .map(|s| s.src_side)
            .unwrap_or_else(|| determine_side(src, pos(dst)));
        let dst_side = info
            .map(|s| s.dst_side)
            .unwrap_or_else(|| determine_side(dst, pos(src)));
        let mut ps = match info {
            Some(s) => port_for_side(src, s.src_side),
            None => port_for_side(src, choose_side(src, pos(dst), Side::Bottom)),
        };
        let mut pd = match info {
            Some(s) => port_for_side(dst, s.dst_side),
            None => port_for_side(dst, choose_side(dst, pos(src), Side::Top)),
        };
        let apply = |pt: Point, side: Side, off: f64| {
            if side.vertical() {
                p(pt.x + off, pt.y)
            } else {
                p(pt.x, pt.y + off)
            }
        };
        if let Some(&o) = port_offsets.get(&(i, true)) {
            ps = apply(ps, src_side, o);
        }
        if let Some(&o) = port_offsets.get(&(i, false)) {
            pd = apply(pd, dst_side, o);
        }
        (ps, pd, src_side, dst_side)
    };

    for &i in &routing_order {
        let e = &edges[i];
        r.edge_segs[i] = Vec::new();
        if points[i].as_ref().is_some_and(|p| !p.is_empty()) || e.start == e.end {
            continue;
        }
        let (Some(&src), Some(&dst)) = (
            node_by_id.get(e.start.as_str()),
            node_by_id.get(e.end.as_str()),
        ) else {
            continue;
        };
        let (ps, pd, src_side, dst_side) = ports_for_edge(i, src, dst);
        let mut sa = ps;
        let mut da = pd;
        if src_side.vertical() {
            sa.y = if ps.y > src.y.unwrap_or(0.0) {
                ps.y + ANCHOR_OFFSET
            } else {
                ps.y - ANCHOR_OFFSET
            };
        } else {
            sa.x = if ps.x > src.x.unwrap_or(0.0) {
                ps.x + ANCHOR_OFFSET
            } else {
                ps.x - ANCHOR_OFFSET
            };
        }
        if dst_side.vertical() {
            da.y = if pd.y > dst.y.unwrap_or(0.0) {
                pd.y + ANCHOR_OFFSET
            } else {
                pd.y - ANCHOR_OFFSET
            };
        } else {
            da.x = if pd.x > dst.x.unwrap_or(0.0) {
                pd.x + ANCHOR_OFFSET
            } else {
                pd.x - ANCHOR_OFFSET
            };
        }
        let inside = |pt: Point| -> Option<Obs> {
            r.obstacles
                .iter()
                .find(|o| {
                    o.node_id != e.start
                        && o.node_id != e.end
                        && pt.x > o.min_x
                        && pt.x < o.max_x
                        && pt.y > o.min_y
                        && pt.y < o.max_y
                })
                .cloned()
        };
        // `obstacleDetour`.
        let detour = |port: Point, node: &LNode, opp: &LNode, o: &Obs, vertical: bool| {
            if vertical {
                let pos_side = port.y > node.y.unwrap_or(0.0);
                let right = opp.x.unwrap_or(0.0) >= port.x;
                (
                    if right {
                        o.max_x + HORIZONTAL_PIPE_MARGIN
                    } else {
                        o.min_x - HORIZONTAL_PIPE_MARGIN
                    },
                    if pos_side {
                        o.max_y + VERTICAL_PIPE_MARGIN
                    } else {
                        o.min_y - VERTICAL_PIPE_MARGIN
                    },
                    pos_side,
                )
            } else {
                let pos_side = port.x > node.x.unwrap_or(0.0);
                let down = opp.y.unwrap_or(0.0) >= port.y;
                (
                    if pos_side {
                        o.max_x + HORIZONTAL_PIPE_MARGIN
                    } else {
                        o.min_x - HORIZONTAL_PIPE_MARGIN
                    },
                    if down {
                        o.max_y + VERTICAL_PIPE_MARGIN
                    } else {
                        o.min_y - VERTICAL_PIPE_MARGIN
                    },
                    pos_side,
                )
            }
        };
        let mut src_wp: Vec<Point> = Vec::new();
        if let Some(o) = inside(sa) {
            if src_side.vertical() {
                let (dx, dy, pos_side) = detour(ps, src, dst, &o, true);
                sa = p(dx, dy);
                let gap_y = if pos_side {
                    (o.min_y - 2.0).min(ps.y + ANCHOR_OFFSET)
                } else {
                    (o.max_y + 2.0).max(ps.y - ANCHOR_OFFSET)
                };
                src_wp = vec![p(ps.x, gap_y), p(dx, gap_y), p(dx, dy)];
            } else {
                let (dx, dy, pos_side) = detour(ps, src, dst, &o, false);
                let gap_x = if pos_side {
                    (o.min_x - 2.0).min(ps.x + ANCHOR_OFFSET)
                } else {
                    (o.max_x + 2.0).max(ps.x - ANCHOR_OFFSET)
                };
                sa = p(dx, dy);
                src_wp = vec![p(gap_x, ps.y), p(gap_x, dy), p(dx, dy)];
            }
        }
        let mut dst_wp: Vec<Point> = Vec::new();
        if let Some(o) = inside(da) {
            if dst_side.vertical() {
                let (dx, dy, _) = detour(pd, dst, src, &o, true);
                da = p(dx, dy);
                dst_wp = vec![p(dx, dy), p(pd.x, dy)];
            } else {
                let (dx, dy, _) = detour(pd, dst, src, &o, false);
                da = p(dx, dy);
                dst_wp = vec![p(dx, dy), p(dx, pd.y)];
            }
        }

        // Centered straight-line fast path.
        if src_wp.is_empty() && dst_wp.is_empty() {
            let same_x = (sa.x - da.x).abs() < HORIZONTAL_PIPE_MARGIN;
            let same_y = (sa.y - da.y).abs() < HORIZONTAL_PIPE_MARGIN;
            let has_offset =
                port_offsets.contains_key(&(i, true)) || port_offsets.contains_key(&(i, false));
            let sft = face_total(&e.start, src_side);
            let dft = face_total(&e.end, dst_side);
            let contested = sft > 1 || dft > 1;
            let sit = incident.get(e.start.as_str()).copied().unwrap_or(0);
            let dit = incident.get(e.end.as_str()).copied().unwrap_or(0);
            let contested_label = (sft > 1 && face_has_label(&e.start, src_side))
                || (dft > 1 && face_has_label(&e.end, dst_side));
            let can_keep =
                contested && !contested_label && (sft <= 1 || sit <= 2) && (dft <= 1 || dit <= 2);
            if (same_x || same_y)
                && !has_offset
                && (!contested || can_keep)
                && !r.is_segment_blocked(ps, pd, &e.start, &e.end)
            {
                points[i] = Some(vec![ps, sa, da, pd]);
                straight_edges.insert(i);
                let horizontal = same_y;
                let coord = if same_y { ps.y } else { ps.x };
                let (from, to) = if same_y {
                    (ps.x.min(pd.x), ps.x.max(pd.x))
                } else {
                    (ps.y.min(pd.y), ps.y.max(pd.y))
                };
                r.pipes.push(Pipe {
                    horizontal,
                    coord,
                    span_min: from,
                    span_max: to,
                    tracks: Vec::new(),
                    registered: false,
                });
                let pipe = r.pipes.len() - 1;
                r.segs.push(RSeg {
                    edge: i,
                    seg: 0,
                    horizontal,
                    pipe,
                    track: 0,
                    from,
                    to,
                });
                continue;
            }
        }

        let sp = r.get_or_add_pipe(false, sa.x, sa.y, sa.y);
        sa.x = r.pipes[sp].coord;
        let dp = r.get_or_add_pipe(false, da.x, da.y, da.y);
        da.x = r.pipes[dp].coord;

        let mut bb_min_x = sa.x.min(da.x) - 50.0;
        let mut bb_max_x = sa.x.max(da.x) + 50.0;
        let mut bb_min_y = sa.y.min(da.y) - 50.0;
        let mut bb_max_y = sa.y.max(da.y) + 50.0;
        let (pmin_x, pmax_x) = (sa.x.min(da.x), sa.x.max(da.x));
        let (pmin_y, pmax_y) = (sa.y.min(da.y), sa.y.max(da.y));
        for o in &r.obstacles {
            if o.min_x < pmax_x && o.max_x > pmin_x && o.min_y < pmax_y && o.max_y > pmin_y {
                bb_min_x = bb_min_x.min(o.min_x - ROUTING_MARGIN);
                bb_max_x = bb_max_x.max(o.max_x + ROUTING_MARGIN);
                bb_min_y = bb_min_y.min(o.min_y - ROUTING_MARGIN);
                bb_max_y = bb_max_y.max(o.max_y + ROUTING_MARGIN);
            }
        }
        let obs = r.obstacles.clone();
        for o in &obs {
            if o.max_x < bb_min_x || o.min_x > bb_max_x || o.max_y < bb_min_y || o.min_y > bb_max_y
            {
                continue;
            }
            r.get_or_add_pipe(true, o.min_y - HORIZONTAL_PIPE_MARGIN, bb_min_x, bb_max_x);
            r.get_or_add_pipe(true, o.max_y + HORIZONTAL_PIPE_MARGIN, bb_min_x, bb_max_x);
            r.get_or_add_pipe(false, o.min_x - VERTICAL_PIPE_MARGIN, bb_min_y, bb_max_y);
            r.get_or_add_pipe(false, o.max_x + VERTICAL_PIPE_MARGIN, bb_min_y, bb_max_y);
        }
        r.get_or_add_pipe(true, sa.y, bb_min_x, bb_max_x);
        r.get_or_add_pipe(true, da.y, bb_min_x, bb_max_x);
        let mut h_pipes: Vec<f64> = r
            .pipes
            .iter()
            .filter(|p| p.registered && p.horizontal && p.coord >= bb_min_y && p.coord <= bb_max_y)
            .map(|p| p.coord)
            .collect();
        let mut v_pipes: Vec<f64> = r
            .pipes
            .iter()
            .filter(|p| p.registered && !p.horizontal && p.coord >= bb_min_x && p.coord <= bb_max_x)
            .map(|p| p.coord)
            .collect();

        let key = |q: Point| format!("{},{}", to_fixed(q.x, 1), to_fixed(q.y, 1));
        let start_key = key(sa);
        let end_key = key(da);
        let blocked = |a: Point, b: Point| r.is_segment_blocked(a, b, &e.start, &e.end);

        let mut found: Vec<Point> = Vec::new();
        let corner_hv = p(da.x, sa.y);
        let hv_blocked = blocked(sa, corner_hv) || blocked(corner_hv, da);
        let corner_vh = p(sa.x, da.y);
        let vh_blocked = blocked(sa, corner_vh) || blocked(corner_vh, da);
        if !hv_blocked {
            if (sa.y - da.y).abs() < EPS || (sa.x - da.x).abs() < EPS {
                found = vec![sa, da];
            } else {
                found = vec![sa, corner_hv, da];
            }
        } else if !vh_blocked {
            if (sa.x - da.x).abs() < EPS {
                found = vec![sa, da];
            } else {
                found = vec![sa, corner_vh, da];
            }
        }

        if found.is_empty() {
            let mut g_score: HashMap<String, f64> = HashMap::new();
            let mut came_from: HashMap<String, Point> = HashMap::new();
            let mut arrival: HashMap<String, char> = HashMap::new();
            let mut open_set: HashSet<String> = HashSet::new();
            let mut open: Vec<(String, f64, Point)> = Vec::new();
            g_score.insert(start_key.clone(), 0.0);
            arrival.insert(start_key.clone(), 'n');
            open.push((start_key.clone(), (da.x - sa.x).hypot(da.y - sa.y), sa));
            open_set.insert(start_key.clone());
            v_pipes.sort_by(|a, b| num_cmp(*a, *b));
            h_pipes.sort_by(|a, b| num_cmp(*a, *b));
            while !open.is_empty() {
                open.sort_by(|a, b| num_cmp(a.1, b.1));
                let (ck, _, cp) = open.remove(0);
                open_set.remove(&ck);
                if ck == end_key {
                    let mut curr_key = end_key.clone();
                    found = vec![da];
                    while let Some(&prev) = came_from.get(&curr_key) {
                        found.insert(0, prev);
                        curr_key = key(prev);
                    }
                    break;
                }
                let (cx, cy) = (cp.x, cp.y);
                let vi = v_pipes.iter().position(|c| (c - cx).abs() < 1.0);
                let hi = h_pipes.iter().position(|c| (c - cy).abs() < 1.0);
                let mut nbrs: Vec<Point> = Vec::new();
                if let Some(vi) = vi {
                    if vi > 0 {
                        nbrs.push(p(v_pipes[vi - 1], cy));
                    }
                    if vi + 1 < v_pipes.len() {
                        nbrs.push(p(v_pipes[vi + 1], cy));
                    }
                }
                if let Some(hi) = hi {
                    if hi > 0 {
                        nbrs.push(p(cx, h_pipes[hi - 1]));
                    }
                    if hi + 1 < h_pipes.len() {
                        nbrs.push(p(cx, h_pipes[hi + 1]));
                    }
                }
                for nb in nbrs {
                    let (min_x, max_x) = (cx.min(nb.x), cx.max(nb.x));
                    let (min_y, max_y) = (cy.min(nb.y), cy.max(nb.y));
                    let hit = r.obstacles.iter().any(|o| {
                        if o.node_id == e.start || o.node_id == e.end {
                            return false;
                        }
                        if min_x != max_x {
                            o.min_y < cy && o.max_y > cy && o.max_x > min_x && o.min_x < max_x
                        } else {
                            o.min_x < cx && o.max_x > cx && o.max_y > min_y && o.min_y < max_y
                        }
                    });
                    if hit {
                        continue;
                    }
                    let nk = key(nb);
                    let dist = (nb.x - cx).abs() + (nb.y - cy).abs();
                    let pen = r.crossing_penalty(i, cp, nb);
                    let ddx = da.x - sa.x;
                    let ddy = da.y - sa.y;
                    let mdx = nb.x - cx;
                    let mdy = nb.y - cy;
                    let mut dir_pen = 0.0;
                    if (ddy > 10.0 && mdy < -5.0) || (ddy < -10.0 && mdy > 5.0) {
                        dir_pen = mdy.abs() * 100.0;
                    }
                    if (ddx > 10.0 && mdx < -5.0) || (ddx < -10.0 && mdx > 5.0) {
                        dir_pen += mdx.abs() * 50.0;
                    }
                    let cur_dir = arrival.get(&ck).copied().unwrap_or('n');
                    let move_dir = if mdx.abs() > EPS { 'h' } else { 'v' };
                    let bend = if cur_dir != 'n' && cur_dir != move_dir {
                        50.0
                    } else {
                        0.0
                    };
                    let tentative = g_score.get(&ck).copied().unwrap_or(f64::INFINITY)
                        + (dist + pen + dir_pen + bend);
                    let h = (da.x - nb.x).abs() + (da.y - nb.y).abs();
                    if tentative < g_score.get(&nk).copied().unwrap_or(f64::INFINITY) {
                        came_from.insert(nk.clone(), cp);
                        g_score.insert(nk.clone(), tentative);
                        arrival.insert(nk.clone(), move_dir);
                        if !open_set.contains(&nk) {
                            open.push((nk.clone(), tentative + h, nb));
                            open_set.insert(nk);
                        } else if let Some(o) = open.iter_mut().find(|o| o.0 == nk) {
                            o.1 = tentative + h;
                        }
                    }
                }
            }
        }
        if found.is_empty() {
            found = vec![sa, p(sa.x, da.y), da];
        }

        if found.len() > 4 {
            let start = found[0];
            let end = found[found.len() - 1];
            let mut min_x = start.x.min(end.x);
            let mut max_x = start.x.max(end.x);
            for pt in &found {
                min_x = min_x.min(pt.x);
                max_x = max_x.max(pt.x);
            }
            let went_right = max_x > start.x.max(end.x);
            let went_left = min_x < start.x.min(end.x);
            if is_lr {
                let margin = VERTICAL_PIPE_MARGIN;
                if went_right {
                    let path_x = start.x.max(end.x);
                    let (pmin, pmax) = (start.y.min(end.y), start.y.max(end.y));
                    let det: Vec<&Obs> = r
                        .obstacles
                        .iter()
                        .filter(|o| {
                            o.min_x < path_x && o.max_x > path_x && o.min_y < pmax && o.max_y > pmin
                        })
                        .collect();
                    if !det.is_empty() {
                        let mut vmax = start.x.max(end.x);
                        for o in det {
                            let c = (o.min_x + o.max_x) / 2.0;
                            if o.visual_x_half_extent.is_nan() {
                                continue;
                            }
                            vmax = vmax.max(c + o.visual_x_half_extent + margin);
                        }
                        if !vmax.is_nan() {
                            max_x = vmax;
                        }
                    }
                }
                if went_left {
                    let det: Vec<&Obs> = r
                        .obstacles
                        .iter()
                        .filter(|o| {
                            o.min_x < start.x.min(end.x) + margin
                                && o.min_y < start.y.max(end.y)
                                && o.max_y > start.y.min(end.y)
                        })
                        .collect();
                    if !det.is_empty() {
                        let mut vmin = start.x.min(end.x);
                        for o in det {
                            let c = (o.min_x + o.max_x) / 2.0;
                            vmin = vmin.min(c - o.visual_x_half_extent - margin);
                        }
                        min_x = vmin;
                    }
                }
            }
            let best_return_y = |detour_x: f64| -> f64 {
                let going_down = end.y > start.y;
                let relevant: Vec<&Obs> = r
                    .obstacles
                    .iter()
                    .filter(|o| {
                        start.x.min(end.x) < o.max_x
                            && start.x.max(end.x) > o.min_x
                            && start.y.min(end.y) < o.max_y
                            && start.y.max(end.y) > o.min_y
                    })
                    .collect();
                let mut filtered = relevant.clone();
                if is_lr && !relevant.is_empty() {
                    let at: Vec<&Obs> = relevant
                        .iter()
                        .filter(|o| o.min_x < detour_x && o.max_x > detour_x)
                        .copied()
                        .collect();
                    if !at.is_empty() {
                        filtered = at;
                    }
                }
                if filtered.is_empty() {
                    return end.y;
                }
                if going_down {
                    let lowest = filtered
                        .iter()
                        .fold(f64::NEG_INFINITY, |m, o| m.max(o.max_y));
                    let best = lowest + HORIZONTAL_PIPE_MARGIN;
                    if best < end.y - EPS {
                        return best;
                    }
                } else {
                    let highest = filtered.iter().fold(f64::INFINITY, |m, o| m.min(o.min_y));
                    let best = highest - HORIZONTAL_PIPE_MARGIN;
                    if best > end.y + EPS {
                        return best;
                    }
                }
                end.y
            };
            let try_detour = |detour_x: f64| -> Option<Vec<Point>> {
                let best_y = best_return_y(detour_x);
                let c1 = p(detour_x, start.y);
                let c2 = p(detour_x, best_y);
                let c3 = p(end.x, best_y);
                let b4 = if best_y != end.y {
                    blocked(c3, end)
                } else {
                    false
                };
                if !blocked(start, c1) && !blocked(c1, c2) && !blocked(c2, c3) && !b4 {
                    if (best_y - end.y).abs() < EPS {
                        return Some(vec![start, c1, c2, end]);
                    }
                    return Some(vec![start, c1, c2, c3, end]);
                }
                None
            };
            let simplified = if went_right && !went_left {
                try_detour(max_x)
            } else if went_left && !went_right {
                try_detour(min_x)
            } else {
                None
            };
            if let Some(s) = simplified {
                found = s;
            }
        }

        let mut full: Vec<Point> = vec![ps];
        full.extend(src_wp.iter().copied());
        full.extend(found.iter().copied());
        full.extend(dst_wp.iter().rev().copied());
        full.push(pd);
        if full.len() >= 3 {
            let n = full.len();
            let (a, b, c) = (full[n - 3], full[n - 2], full[n - 1]);
            let horiz = (a.y - b.y).abs() < EPS && (b.y - c.y).abs() < EPS;
            let vert = (a.x - b.x).abs() < EPS && (b.x - c.x).abs() < EPS;
            if horiz {
                let sab = sign(b.x - a.x);
                let sac = sign(c.x - a.x);
                if sab != 0.0 && sab == sac && (b.x - a.x).abs() > (c.x - a.x).abs() {
                    full.remove(n - 2);
                }
            } else if vert {
                let sab = sign(b.y - a.y);
                let sac = sign(c.y - a.y);
                if sab != 0.0 && sab == sac && (b.y - a.y).abs() > (c.y - a.y).abs() {
                    full.remove(n - 2);
                }
            }
        }
        let mut simp: Vec<Point> = vec![full[0]];
        for k in 1..full.len() - 1 {
            if k == 1 {
                simp.push(full[k]);
                continue;
            }
            let prev = *simp.last().unwrap();
            let curr = full[k];
            let next = full[k + 1];
            if (prev.y - curr.y).abs() < EPS && (curr.y - next.y).abs() < EPS {
                if (curr.x > prev.x) != (next.x > curr.x) {
                    simp.push(curr);
                }
                continue;
            }
            if (prev.x - curr.x).abs() < EPS && (curr.x - next.x).abs() < EPS {
                if (curr.y > prev.y) != (next.y > curr.y) {
                    simp.push(curr);
                }
                continue;
            }
            simp.push(curr);
        }
        simp.push(full[full.len() - 1]);

        for k in 0..simp.len() - 1 {
            let (p1, p2) = (simp[k], simp[k + 1]);
            let vertical = (p1.x - p2.x).abs() < EPS;
            let coord = if vertical { p1.x } else { p1.y };
            let (from, to) = if vertical {
                (p1.y.min(p2.y), p1.y.max(p2.y))
            } else {
                (p1.x.min(p2.x), p1.x.max(p2.x))
            };
            let pipe = r.get_or_add_pipe(!vertical, coord, from, to);
            r.segs.push(RSeg {
                edge: i,
                seg: k,
                horizontal: !vertical,
                pipe,
                track: 0,
                from,
                to,
            });
            let si = r.segs.len() - 1;
            r.edge_segs[i].push(si);
            if r.pipes[pipe].tracks.is_empty() {
                r.pipes[pipe].tracks.push(None);
            }
            if r.pipes[pipe].tracks[0].is_none() {
                r.pipes[pipe].tracks[0] = Some(Track {
                    index: 0,
                    segments: Vec::new(),
                });
            }
            r.track_mut(pipe, 0).segments.push(SegRef {
                edge: i,
                seg: k,
                from,
                to,
            });
        }
    }

    // Phase 2: crossing reduction.
    let mut dest_cache: HashMap<usize, DestInfo> = HashMap::new();
    let dest_info = |r: &Router, cache: &mut HashMap<usize, DestInfo>, ei: usize| -> DestInfo {
        if let Some(d) = cache.get(&ei) {
            return *d;
        }
        let indices = &r.edge_segs[ei];
        if indices.is_empty() {
            let d = DestInfo::default();
            cache.insert(ei, d);
            return d;
        }
        let base = r.pipes[r.segs[indices[0]].pipe].coord;
        let mut dest = base;
        for &idx in &indices[1..] {
            let s = &r.segs[idx];
            if s.horizontal {
                dest = if (s.from - base).abs() > (s.to - base).abs() {
                    s.from
                } else {
                    s.to
                };
                break;
            }
        }
        let d = DestInfo {
            dest,
            deviation: (dest - base).abs(),
            delta: dest - base,
        };
        cache.insert(ei, d);
        d
    };
    let edge_distance = |ei: usize| -> f64 {
        let e = &edges[ei];
        match (
            node_by_id.get(e.start.as_str()),
            node_by_id.get(e.end.as_str()),
        ) {
            (Some(s), Some(d)) => {
                (d.x.unwrap_or(0.0) - s.x.unwrap_or(0.0)).abs()
                    + (d.y.unwrap_or(0.0) - s.y.unwrap_or(0.0)).abs()
            }
            _ => 0.0,
        }
    };
    let mut iterations = 0;
    while iterations < 10 {
        let mut changed = 0;
        // fixSourceHandleCrossings
        {
            let mut by_src: OMap<Vec<usize>> = OMap::new();
            for (i, e) in edges.iter().enumerate() {
                if r.edge_segs[i].is_empty() {
                    continue;
                }
                by_src.entry_or(&e.start, Vec::new).push(i);
            }
            let groups: Vec<Vec<usize>> = by_src.values().cloned().collect();
            for mut grp in groups {
                let infos: HashMap<usize, DestInfo> = grp
                    .iter()
                    .map(|&ei| (ei, dest_info(&r, &mut dest_cache, ei)))
                    .collect();
                grp.sort_by(|&a, &b| {
                    let (ia, ib) = (infos[&a], infos[&b]);
                    if (ia.deviation - ib.deviation).abs() > 1.0 {
                        return num_cmp(ia.deviation, ib.deviation);
                    }
                    if (ia.dest - ib.dest).abs() > 1.0 {
                        return num_cmp(ia.dest, ib.dest);
                    }
                    let (da, db) = (edge_distance(a), edge_distance(b));
                    if (da - db).abs() > 1.0 {
                        return num_cmp(db, da);
                    }
                    let (la, lb) = (r.edge_segs[a].len(), r.edge_segs[b].len());
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    if la == 1 {
                        let (sa, sb) = (&r.segs[r.edge_segs[a][0]], &r.segs[r.edge_segs[b][0]]);
                        let (da, db) = ((sa.to - sa.from).abs(), (sb.to - sb.from).abs());
                        if (da - db).abs() > 1.0 {
                            return num_cmp(da, db);
                        }
                    }
                    std::cmp::Ordering::Equal
                });
                let handles: Vec<usize> = grp.iter().map(|&ei| r.edge_segs[ei][0]).collect();
                changed += r.resolve_handle_conflicts(&handles);
            }
        }
        // fixTargetHandleCrossings
        {
            let mut by_dst: OMap<Vec<usize>> = OMap::new();
            for (i, e) in edges.iter().enumerate() {
                if r.edge_segs[i].is_empty() {
                    continue;
                }
                by_dst.entry_or(&e.end, Vec::new).push(i);
            }
            let groups: Vec<Vec<usize>> = by_dst.values().cloned().collect();
            for mut grp in groups {
                let dist = |ei: usize| -> f64 {
                    let ind = &r.edge_segs[ei];
                    if ind.len() < 2 {
                        return 0.0;
                    }
                    let prev = &r.segs[ind[ind.len() - 2]];
                    (prev.to - prev.from).abs()
                };
                grp.sort_by(|&a, &b| {
                    let (sa, sb) = (dist(a), dist(b));
                    if (sa - sb).abs() > 0.1 {
                        return num_cmp(sa, sb);
                    }
                    a.cmp(&b)
                });
                let handles: Vec<usize> = grp
                    .iter()
                    .map(|&ei| *r.edge_segs[ei].last().unwrap())
                    .collect();
                changed += r.resolve_handle_conflicts(&handles);
            }
        }
        // fixPipeCrossings
        for pi in 0..r.pipes.len() {
            if !r.pipes[pi].registered {
                continue;
            }
            let mut segs: Vec<usize> = Vec::new();
            for t in r.pipes[pi].tracks.iter().flatten() {
                for rf in &t.segments {
                    if let Some(&idx) = r.edge_segs[rf.edge]
                        .iter()
                        .find(|&&ix| r.segs[ix].seg == rf.seg)
                    {
                        segs.push(idx);
                    }
                }
            }
            segs.sort_by(|&a, &b| {
                let (sa, sb) = (&r.segs[a], &r.segs[b]);
                sa.edge.cmp(&sb.edge).then(sa.seg.cmp(&sb.seg))
            });
            for i in 0..segs.len() {
                for j in i + 1..segs.len() {
                    if r.segments_conflict(segs[i], segs[j]) {
                        changed += 1;
                        r.resolve_conflict(segs[i], segs[j], false);
                    }
                }
            }
        }
        if changed == 0 {
            break;
        }
        iterations += 1;
    }

    // Phase 3: rebuild geometry.
    let mut seg_coords: HashMap<(usize, usize), f64> = HashMap::new();
    for pi in 0..r.pipes.len() {
        if !r.pipes[pi].registered {
            continue;
        }
        let pipe_coord = r.pipes[pi].coord;
        // (edge, seg, track, from, to)
        let mut segs: Vec<TrackSeg> = Vec::new();
        for t in r.pipes[pi].tracks.iter().flatten() {
            for s in &t.segments {
                segs.push((s.edge, s.seg, t.index, s.from, s.to));
            }
        }
        segs.sort_by(|a, b| num_cmp(a.3, b.3));
        let mut clusters: Vec<Vec<TrackSeg>> = Vec::new();
        if !segs.is_empty() {
            let mut cur = vec![segs[0]];
            let mut end = segs[0].4;
            for &s in &segs[1..] {
                if s.3 < end {
                    cur.push(s);
                    end = end.max(s.4);
                } else {
                    clusters.push(std::mem::take(&mut cur));
                    cur = vec![s];
                    end = s.4;
                }
            }
            clusters.push(cur);
        }
        for cluster in &clusters {
            let mut used: Vec<usize> = Vec::new();
            for s in cluster {
                if !used.contains(&s.2) {
                    used.push(s.2);
                }
            }
            let mut scores: HashMap<usize, f64> = HashMap::new();
            for s in cluster {
                let d = dest_info(&r, &mut dest_cache, s.0);
                *scores.entry(s.2).or_insert(0.0) += d.delta;
            }
            let score = |t: usize| scores.get(&t).copied().unwrap_or(0.0);
            let mut left: Vec<usize> = used.iter().copied().filter(|&t| score(t) < -1.0).collect();
            let mut right: Vec<usize> = used.iter().copied().filter(|&t| score(t) > 1.0).collect();
            let mut neutral: Vec<usize> = used
                .iter()
                .copied()
                .filter(|&t| score(t).abs() <= 1.0)
                .collect();
            left.sort_by(|&a, &b| num_cmp(score(b), score(a)));
            right.sort_by(|&a, &b| num_cmp(score(a), score(b)));
            let mut assign = |track: usize, coord: f64| {
                for s in cluster.iter().filter(|s| s.2 == track) {
                    let c = if straight_edges.contains(&s.0) {
                        pipe_coord
                    } else {
                        coord
                    };
                    seg_coords.insert((s.0, s.1), c);
                }
            };
            let mut lc = 0;
            for &t in &left {
                lc += 1;
                assign(t, pipe_coord - lc as f64 * TRACK_SPACING);
            }
            if neutral.is_empty() && !used.is_empty() {
                let mut by_abs = used.clone();
                by_abs.sort_by(|&a, &b| num_cmp(score(a).abs(), score(b).abs()));
                let best = by_abs[0];
                left.retain(|&t| t != best);
                right.retain(|&t| t != best);
                neutral.push(best);
            }
            for (k, &t) in neutral.iter().enumerate() {
                if k == 0 {
                    assign(t, pipe_coord);
                } else {
                    let dir = if k % 2 == 1 { 1.0 } else { -1.0 };
                    let mag = (k as f64 / 2.0).ceil();
                    assign(t, pipe_coord + dir * mag * TRACK_SPACING * 0.5);
                }
            }
            let mut rc = 0;
            for &t in &right {
                rc += 1;
                assign(t, pipe_coord + rc as f64 * TRACK_SPACING);
            }
        }
    }

    for (i, e) in edges.iter().enumerate() {
        let indices = r.edge_segs[i].clone();
        if indices.is_empty() {
            continue;
        }
        let src = node_by_id[e.start.as_str()];
        let dst = node_by_id[e.end.as_str()];
        let (ps, pd, _, _) = ports_for_edge(i, src, dst);
        struct Line {
            vertical: bool,
            coord: f64,
            from: f64,
            to: f64,
        }
        let lines: Vec<Line> = indices
            .iter()
            .map(|&idx| {
                let s = &r.segs[idx];
                Line {
                    vertical: !s.horizontal,
                    coord: seg_coords
                        .get(&(s.edge, s.seg))
                        .copied()
                        .unwrap_or(r.pipes[s.pipe].coord),
                    from: s.from,
                    to: s.to,
                }
            })
            .collect();
        let on = |l: &Line, along: f64| {
            if l.vertical {
                p(l.coord, along)
            } else {
                p(along, l.coord)
            }
        };
        let shared = |l: &Line, n: &Line| {
            if (l.to - n.from).abs() < EPS || (l.to - n.to).abs() < EPS {
                l.to
            } else {
                l.from
            }
        };
        let mut np: Vec<Point> = vec![ps];
        for k in 0..lines.len() {
            let l = &lines[k];
            let prev = *np.last().unwrap();
            let prev_along = if l.vertical { prev.y } else { prev.x };
            let prev_track = if l.vertical { prev.x } else { prev.y };
            if (prev_track - l.coord).abs() > EPS {
                np.push(on(l, prev_along));
            }
            if k + 1 < lines.len() {
                let n = &lines[k + 1];
                if n.vertical == l.vertical {
                    if (l.coord - n.coord).abs() > EPS {
                        let j = if l.vertical {
                            (prev_along + n.from) / 2.0
                        } else {
                            shared(l, n)
                        };
                        np.push(on(l, j));
                        np.push(on(n, j));
                    } else if k == 0 || k + 2 == lines.len() {
                        np.push(on(l, shared(l, n)));
                    }
                } else {
                    np.push(on(l, n.coord));
                }
            } else {
                let end_along = if (l.from - prev_along).abs() < (l.to - prev_along).abs() {
                    l.to
                } else {
                    l.from
                };
                np.push(on(l, end_along));
            }
        }
        let last = *np.last().unwrap();
        if (last.x - pd.x).abs() > EPS || (last.y - pd.y).abs() > EPS {
            np.push(pd);
        }
        let first = np[0];
        if (first.x - ps.x).abs() > EPS || (first.y - ps.y).abs() > EPS {
            np.insert(0, ps);
        }
        let mut filtered: Vec<Point> = vec![np[0]];
        for &q in &np[1..] {
            let prev = *filtered.last().unwrap();
            if (q.x - prev.x).abs() > EPS || (q.y - prev.y).abs() > EPS {
                filtered.push(q);
            }
        }
        points[i] = Some(filtered);
    }

    for (k, &di) in edge_idx.iter().enumerate() {
        if points[k].is_some() {
            data.edges[di].points = points[k].clone();
        }
    }
    data.edges.retain(|e| !e.is_layout_only);

    let clamp = |q: Point, node: &LNode| -> Point {
        let cx = node.x.unwrap_or(0.0);
        let cy = node.y.unwrap_or(0.0);
        let (w, h) = drawn_size(node, 0.0);
        if w <= 0.0 || h <= 0.0 {
            return q;
        }
        let (l, rt, t, b) = (cx - w / 2.0, cx + w / 2.0, cy - h / 2.0, cy + h / 2.0);
        if q.x < l || q.x > rt || q.y < t || q.y > b {
            return q;
        }
        let (dl, dr, dt, db) = (q.x - l, rt - q.x, q.y - t, b - q.y);
        let m = dl.min(dr).min(dt).min(db);
        if m == dl {
            p(l, q.y)
        } else if m == dr {
            p(rt, q.y)
        } else if m == dt {
            p(q.x, t)
        } else {
            p(q.x, b)
        }
    };
    for e in data.edges.iter_mut() {
        let Some(pts) = e.points.as_mut() else {
            continue;
        };
        if pts.len() < 2 {
            continue;
        }
        if let Some(src) = node_by_id.get(e.start.as_str()) {
            pts[0] = clamp(pts[0], src);
        }
        if let Some(dst) = node_by_id.get(e.end.as_str()) {
            let n = pts.len();
            pts[n - 1] = clamp(pts[n - 1], dst);
        }
    }
    let _ = (js_round, &r.node_by_id);
}
