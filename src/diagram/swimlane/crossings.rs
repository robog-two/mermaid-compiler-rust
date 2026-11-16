//! The crossing clean-up of `direction/materializedGeometry.ts`:
//! `shortcutRedundantOrthogonalJogs` and `resolveRenderedOrthogonalCrossings`.

use super::geometry::*;
use super::materialized::{E, MIN_SHARED, crossing_count, ends, points_with, segs, visible};
use super::model::{LayoutData, Rb, to_fixed};
use crate::geom::Point;
use std::collections::HashMap;

/// `EDGE_ROUTING.ROUTE_TOUCH_TOLERANCE`.
const ROUTE_TOUCH_TOLERANCE: f64 = 6.0;

fn path_key(p: &[Point]) -> String {
    p.iter()
        .map(|q| format!("{},{}", to_fixed(q.x, 3), to_fixed(q.y, 3)))
        .collect::<Vec<_>>()
        .join("|")
}

fn runs_along_border(s: &Seg, r: &Rb) -> bool {
    if s.horizontal {
        let y = s.a.y;
        let on = (y - r.top).abs() < 1.0 || (y - r.bottom).abs() < 1.0;
        return on && overlap_length(s.a.x, s.b.x, r.left, r.right) >= MIN_SHARED;
    }
    if s.vertical {
        let x = s.a.x;
        let on = (x - r.left).abs() < 1.0 || (x - r.right).abs() < 1.0;
        return on && overlap_length(s.a.y, s.b.y, r.top, r.bottom) >= MIN_SHARED;
    }
    false
}

/// `shortcutRedundantOrthogonalJogs`.
pub fn shortcut_redundant_orthogonal_jogs(data: &mut LayoutData) {
    const BUFFER: f64 = 2.0;
    let (real, labels) = node_rect_entries(data.nodes.iter());
    let vis = visible(data);
    let seg_length = |p: &[Point]| -> f64 {
        segs(p)
            .iter()
            .map(|s| (s.a.x - s.b.x).hypot(s.a.y - s.b.y))
            .sum()
    };
    let end_rects = |data: &LayoutData, ei: usize| -> Vec<Rb> {
        ends(&data.edges[ei])
            .iter()
            .filter_map(|id| data.nodes.iter().find(|n| n.id == *id).and_then(rect_of))
            .collect()
    };
    let candidates_at = |p: &[Point], i: usize| -> Vec<Vec<Point>> {
        if i + 3 >= p.len() {
            return Vec::new();
        }
        let (p0, p1, p2, p3) = (p[i], p[i + 1], p[i + 2], p[i + 3]);
        let hvh = is_h(p0, p1, E) && is_v(p1, p2, E) && is_h(p2, p3, E);
        let vhv = is_v(p0, p1, E) && is_h(p1, p2, E) && is_v(p2, p3, E);
        if !hvh && !vhv {
            return Vec::new();
        }
        let oppose = if hvh {
            super::model::sign(p1.x - p0.x) != super::model::sign(p3.x - p2.x)
        } else {
            super::model::sign(p1.y - p0.y) != super::model::sign(p3.y - p2.y)
        };
        if !oppose {
            return Vec::new();
        }
        let raw: Vec<Vec<Point>> = if same_x(p0, p3, E) || same_y(p0, p3, E) {
            let mut v = p[..=i].to_vec();
            v.extend_from_slice(&p[i + 3..]);
            vec![v]
        } else {
            [pt(p0.x, p3.y), pt(p3.x, p0.y)]
                .iter()
                .map(|&c| {
                    let mut v = p[..=i].to_vec();
                    v.push(c);
                    v.extend_from_slice(&p[i + 3..]);
                    v
                })
                .collect()
        };
        let mut seen = std::collections::HashSet::new();
        raw.into_iter()
            .map(|c| simplify(&dedupe(&c, GEPS)))
            .filter(|c| {
                if segs(c).len() + 1 != c.len() {
                    return false;
                }
                if !c.iter().any(|&q| same_point(q, p3, E)) {
                    return false;
                }
                seen.insert(path_key(c))
            })
            .collect()
    };
    let safe = |data: &LayoutData, ei: usize, cand: &[Point], current: usize| -> bool {
        let ex = ends(&data.edges[ei]);
        let er = end_rects(data, ei);
        for s in segs(cand) {
            if seg_hits_any_rect(s.a, s.b, &real, &ex, -BUFFER) {
                return false;
            }
            if seg_hits_any_rect(s.a, s.b, &labels, &[], -BUFFER) {
                return false;
            }
            if er.iter().any(|r| runs_along_border(&s, r)) {
                return false;
            }
        }
        let cs = segs(cand);
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
        let mut best: Option<(usize, Vec<Point>)> = None;
        let mut best_c = current;
        let mut best_b = usize::MAX;
        let mut best_l = f64::INFINITY;
        for &ei in &vis {
            let cur = points_with(data, ei, &[]);
            let cb = count_bends(&cur, E);
            let cl = seg_length(&cur);
            let mut idx = 0;
            while idx + 4 <= cur.len() {
                for cand in candidates_at(&cur, idx) {
                    let nb = count_bends(&cand, E);
                    let nl = seg_length(&cand);
                    let improves = nb < cb || (nb == cb && nl < cl - E);
                    if !improves || !safe(data, ei, &cand, current) {
                        continue;
                    }
                    let nc = crossing_count(data, &vis, &[(ei, cand.clone())]);
                    if nc > best_c
                        || (nc == best_c && (nb > best_b || (nb == best_b && nl >= best_l)))
                    {
                        continue;
                    }
                    best = Some((ei, cand));
                    best_c = nc;
                    best_b = nb;
                    best_l = nl;
                }
                idx += 1;
            }
        }
        let Some((ei, p)) = best else { return };
        data.edges[ei].points = Some(p);
    }
}

// ---------------------------------------------------------------------------
// resolveRenderedOrthogonalCrossings
// ---------------------------------------------------------------------------

const ANCHOR: f64 = 20.0;
const EXTRA_CHANNELS: usize = 2;
const MAX_PAIR_CANDIDATES: usize = 48;

struct Snapshot {
    count: usize,
    /// (first, second, count) by edge index.
    pairs: Vec<(usize, usize, usize)>,
    /// The edges in crossings, in visible order.
    edges: Vec<usize>,
}

#[derive(Clone)]
struct PairCandidate {
    path: Vec<Point>,
    segs: Vec<Seg>,
    conflicts: Vec<usize>,
    total_bends: usize,
    length: f64,
}

struct Ctx<'a> {
    data: &'a LayoutData,
    vis: Vec<usize>,
    infos: Vec<NodeInfo>,
    rects: Vec<RectEntry>,
    outside: [f64; 4],
}

fn side_idx(s: RectSide) -> usize {
    match s {
        RectSide::Top => 0,
        RectSide::Bottom => 1,
        RectSide::Left => 2,
        RectSide::Right => 3,
    }
}

fn cross_between(a: &[Seg], b: &[Seg]) -> usize {
    let mut c = 0;
    for x in a {
        for y in b {
            if segs_strictly_cross(x.a, x.b, y.a, y.b, E) {
                c += 1;
            }
        }
    }
    c
}

fn manhattan(p: &[Point]) -> f64 {
    (1..p.len())
        .map(|i| (p[i].x - p[i - 1].x).abs() + (p[i].y - p[i - 1].y).abs())
        .sum()
}

fn touches(a: &[Point], b: &[Point]) -> bool {
    routes_touch_at_bend(a, b, ROUTE_TOUCH_TOLERANCE)
}

impl Ctx<'_> {
    fn pts(&self, ei: usize, repl: &[(usize, Vec<Point>)]) -> Vec<Point> {
        points_with(self.data, ei, repl)
    }

    fn outward(&self, side: RectSide) -> Vec<f64> {
        let o = if matches!(side, RectSide::Left | RectSide::Top) {
            -1.0
        } else {
            1.0
        };
        (0..=EXTRA_CHANNELS)
            .map(|c| self.outside[side_idx(side)] + o * ANCHOR * c as f64)
            .collect()
    }

    fn snapshot(&self, repl: &[(usize, Vec<Point>)]) -> Snapshot {
        let mut count = 0;
        let mut pairs = Vec::new();
        let mut edges: Vec<usize> = Vec::new();
        let all: Vec<Vec<Seg>> = self.vis.iter().map(|&i| segs(&self.pts(i, repl))).collect();
        for i in 0..self.vis.len() {
            for j in i + 1..self.vis.len() {
                let c = cross_between(&all[i], &all[j]);
                if c > 0 {
                    count += c;
                    pairs.push((self.vis[i], self.vis[j], c));
                    for e in [self.vis[i], self.vis[j]] {
                        if !edges.contains(&e) {
                            edges.push(e);
                        }
                    }
                }
            }
        }
        let pos = |e: usize| self.vis.iter().position(|&v| v == e).unwrap_or(0);
        edges.sort_by_key(|&e| pos(e));
        Snapshot {
            count,
            pairs,
            edges,
        }
    }

    fn count_with(&self, cur: &Snapshot, repl: &[(usize, Vec<Point>)]) -> usize {
        if repl.is_empty() {
            return cur.count;
        }
        let changed = |e: usize| repl.iter().any(|(k, _)| *k == e);
        let cur_aff: usize = cur
            .pairs
            .iter()
            .filter(|(a, b, _)| changed(*a) || changed(*b))
            .map(|p| p.2)
            .sum();
        let mut rep_aff = 0;
        for i in 0..self.vis.len() {
            let fc = changed(self.vis[i]);
            let fs = segs(&self.pts(self.vis[i], repl));
            for j in i + 1..self.vis.len() {
                if !fc && !changed(self.vis[j]) {
                    continue;
                }
                rep_aff += cross_between(&fs, &segs(&self.pts(self.vis[j], repl)));
            }
        }
        cur.count - cur_aff + rep_aff
    }

    fn components(&self, snap: &Snapshot) -> Vec<Vec<usize>> {
        let mut nbrs: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut add = |a: usize, b: usize| match nbrs.iter_mut().find(|(k, _)| *k == a) {
            Some((_, v)) => {
                if !v.contains(&b) {
                    v.push(b)
                }
            }
            None => nbrs.push((a, vec![b])),
        };
        for &(a, b, _) in &snap.pairs {
            add(a, b);
            add(b, a);
        }
        let pos = |e: usize| self.vis.iter().position(|&v| v == e).unwrap_or(0);
        let mut comps = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        for &e in &snap.edges {
            if seen.contains(&e) {
                continue;
            }
            let mut queue = vec![e];
            let mut comp = Vec::new();
            seen.push(e);
            while let Some(c) = queue.pop() {
                comp.push(c);
                if let Some((_, ns)) = nbrs.iter().find(|(k, _)| *k == c) {
                    for &n in ns {
                        if !seen.contains(&n) {
                            seen.push(n);
                            queue.push(n);
                        }
                    }
                }
            }
            comp.sort_by_key(|&x| pos(x));
            if comp.len() > 1 {
                comps.push(comp);
            }
        }
        comps
    }

    fn search_groups(&self, snap: &Snapshot) -> Vec<Vec<usize>> {
        let pos = |e: usize| self.vis.iter().position(|&v| v == e).unwrap_or(0);
        let mut groups = Vec::new();
        for comp in self.components(snap) {
            let ids: Vec<String> = comp
                .iter()
                .flat_map(|&e| {
                    ends(&self.data.edges[e])
                        .into_iter()
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut group = comp.clone();
            for &e in &self.vis {
                if comp.contains(&e) {
                    continue;
                }
                if ends(&self.data.edges[e])
                    .iter()
                    .any(|id| ids.iter().any(|x| x == id))
                {
                    group.push(e);
                }
            }
            group.sort_by_key(|&x| pos(x));
            groups.push(group);
        }
        groups
    }

    fn path_conflict(&self, ei: usize, path: &[Point]) -> bool {
        let ps = segs(path);
        for &o in &self.vis {
            if o == ei {
                continue;
            }
            let op = self.pts(o, &[]);
            let os = segs(&op);
            for c in &ps {
                for s in &os {
                    if same_axis_overlap(c, s, 0.5) >= MIN_SHARED {
                        return true;
                    }
                }
            }
            if touches(path, &op) {
                return true;
            }
        }
        false
    }

    fn hits_node(&self, ei: usize, path: &[Point]) -> bool {
        let ex = ends(&self.data.edges[ei]);
        segs(path)
            .iter()
            .any(|s| seg_hits_any_rect(s.a, s.b, &self.rects, &ex, -2.0))
    }

    fn candidates_for_sides(
        &self,
        src: Point,
        ss: RectSide,
        dst: Point,
        ds: RectSide,
    ) -> Vec<Vec<Point>> {
        let mut c: Vec<Vec<Point>> = Vec::new();
        let push = |c: &mut Vec<Vec<Point>>, p: Vec<Point>| {
            let s = simplify(&dedupe(&p, GEPS));
            if segs(&s).len() + 1 == s.len() {
                c.push(s);
            }
        };
        if let Some(b) = orthogonal_port_path(src, ss, dst, ds, ANCHOR, E) {
            push(&mut c, b);
        }
        if ss == ds {
            let o = if matches!(ss, RectSide::Left | RectSide::Top) {
                -1.0
            } else {
                1.0
            };
            let local = match ss {
                RectSide::Left => src.x.min(dst.x) - ANCHOR,
                RectSide::Right => src.x.max(dst.x) + ANCHOR,
                RectSide::Top => src.y.min(dst.y) - ANCHOR,
                RectSide::Bottom => src.y.max(dst.y) + ANCHOR,
            };
            for seed in [local, self.outside[side_idx(ss)]] {
                for ch in 0..=EXTRA_CHANNELS {
                    push(
                        &mut c,
                        same_side_track_path(src, ss, dst, seed + o * ANCHOR * ch as f64),
                    );
                }
            }
        }
        let sh = ss.horizontal_face();
        let dh = ds.horizontal_face();
        if sh && !dh {
            for xt in self.outward(ss) {
                for yt in self.outward(ds) {
                    push(
                        &mut c,
                        vec![src, pt(xt, src.y), pt(xt, yt), pt(dst.x, yt), dst],
                    );
                }
            }
        } else if !sh && dh {
            for yt in self.outward(ss) {
                for xt in self.outward(ds) {
                    push(
                        &mut c,
                        vec![src, pt(src.x, yt), pt(xt, yt), pt(xt, dst.y), dst],
                    );
                }
            }
        } else if sh {
            let mut ys = self.outward(RectSide::Top);
            ys.extend(self.outward(RectSide::Bottom));
            for st in self.outward(ss) {
                for dt in self.outward(ds) {
                    for &yt in &ys {
                        push(
                            &mut c,
                            vec![
                                src,
                                pt(st, src.y),
                                pt(st, yt),
                                pt(dt, yt),
                                pt(dt, dst.y),
                                dst,
                            ],
                        );
                    }
                }
            }
        } else {
            let mut xs = self.outward(RectSide::Left);
            xs.extend(self.outward(RectSide::Right));
            for st in self.outward(ss) {
                for dt in self.outward(ds) {
                    for &xt in &xs {
                        push(
                            &mut c,
                            vec![
                                src,
                                pt(src.x, st),
                                pt(xt, st),
                                pt(xt, dt),
                                pt(dst.x, dt),
                                dst,
                            ],
                        );
                    }
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        c.into_iter()
            .map(|p| dedupe(&p, GEPS))
            .filter(|p| p.len() >= 2 && seen.insert(path_key(p)))
            .collect()
    }

    fn terminal_preserving(&self, ei: usize) -> Vec<Vec<Point>> {
        let e = &self.data.edges[ei];
        let Some(dn) = self.infos.iter().find(|i| i.id == e.end) else {
            return Vec::new();
        };
        if e.start.is_empty() {
            return Vec::new();
        }
        let points = dedupe(e.points.as_deref().unwrap_or(&[]), GEPS);
        if points.len() < 4 {
            return Vec::new();
        }
        let (first, dep) = (points[0], points[1]);
        let mut c: Vec<Vec<Point>> = Vec::new();
        let push = |c: &mut Vec<Vec<Point>>, p: Vec<Point>| {
            let s = simplify(&dedupe(&p, GEPS));
            if segs(&s).len() + 1 == s.len() {
                c.push(s);
            }
        };
        let mut ext_x = self.outward(RectSide::Left);
        ext_x.extend(self.outward(RectSide::Right));
        let mut ext_y = self.outward(RectSide::Top);
        ext_y.extend(self.outward(RectSide::Bottom));
        if is_v(first, dep, E) {
            for side in RectSide::ALL {
                let dst = port_for_side(dn, side);
                let target_y = if matches!(side, RectSide::Top | RectSide::Bottom) {
                    self.outward(side)
                } else {
                    ext_y.clone()
                };
                for &t in &ext_x {
                    if side.horizontal_face() {
                        push(&mut c, vec![first, dep, pt(t, dep.y), pt(t, dst.y), dst]);
                    }
                    for &tt in &target_y {
                        push(
                            &mut c,
                            vec![first, dep, pt(t, dep.y), pt(t, tt), pt(dst.x, tt), dst],
                        );
                    }
                }
            }
        } else if is_h(first, dep, E) {
            for side in RectSide::ALL {
                let dst = port_for_side(dn, side);
                let target_x = if side.horizontal_face() {
                    self.outward(side)
                } else {
                    ext_x.clone()
                };
                for &t in &ext_y {
                    if !side.horizontal_face() {
                        push(&mut c, vec![first, dep, pt(dep.x, t), pt(dst.x, t), dst]);
                    }
                    for &tt in &target_x {
                        push(
                            &mut c,
                            vec![first, dep, pt(dep.x, t), pt(tt, t), pt(tt, dst.y), dst],
                        );
                    }
                }
            }
        }
        c
    }

    fn candidate_paths(&self, ei: usize) -> Vec<Vec<Point>> {
        let e = &self.data.edges[ei];
        let (Some(sn), Some(dn)) = (
            self.infos.iter().find(|i| i.id == e.start),
            self.infos.iter().find(|i| i.id == e.end),
        ) else {
            return Vec::new();
        };
        let mut c = Vec::new();
        for ss in RectSide::ALL {
            let sp = port_for_side(sn, ss);
            for ds in RectSide::ALL {
                c.extend(self.candidates_for_sides(sp, ss, port_for_side(dn, ds), ds));
            }
        }
        c.extend(self.terminal_preserving(ei));
        c
    }

    fn pair_candidates(
        &self,
        ei: usize,
        cur: &Snapshot,
        base: &HashMap<usize, Vec<Seg>>,
        by_edge: &HashMap<usize, usize>,
    ) -> Vec<PairCandidate> {
        let mut seen = std::collections::HashSet::new();
        struct Scored {
            path: Vec<Point>,
            segs: Vec<Seg>,
            crossings: usize,
            bends: usize,
            total_bends: usize,
            length: f64,
        }
        let mut cands: Vec<Scored> = self
            .candidate_paths(ei)
            .into_iter()
            .map(|c| simplify(&dedupe(&c, GEPS)))
            .filter(|c| !self.hits_node(ei, c) && c.len() >= 2 && seen.insert(path_key(c)))
            .map(|c| {
                let cs = segs(&c);
                let mut rep = 0;
                for &o in &self.vis {
                    if o == ei {
                        continue;
                    }
                    rep += cross_between(&cs, &base[&o]);
                }
                Scored {
                    crossings: cur.count - by_edge.get(&ei).copied().unwrap_or(0) + rep,
                    bends: count_bends(&c, E),
                    total_bends: count_bends(&c, GEPS),
                    length: manhattan(&c),
                    segs: cs,
                    path: c,
                }
            })
            .filter(|s| s.crossings <= cur.count)
            .collect();
        cands.sort_by(|a, b| {
            a.crossings
                .cmp(&b.crossings)
                .then(a.bends.cmp(&b.bends))
                .then(super::model::num_cmp(a.length, b.length))
        });
        cands.truncate(MAX_PAIR_CANDIDATES);
        cands
            .into_iter()
            .map(|s| {
                let mut conflicts = Vec::new();
                for &o in &self.vis {
                    if o == ei {
                        continue;
                    }
                    let os = &base[&o];
                    if s.segs.iter().any(|c| {
                        os.iter()
                            .any(|x| same_axis_overlap(c, x, 0.5) >= MIN_SHARED)
                    }) || touches(&s.path, &self.pts(o, &[]))
                    {
                        conflicts.push(o);
                    }
                }
                PairCandidate {
                    path: s.path,
                    segs: s.segs,
                    conflicts,
                    total_bends: s.total_bends,
                    length: s.length,
                }
            })
            .collect()
    }

    fn best_paired(&self, cur: &Snapshot) -> Option<Vec<(usize, Vec<Point>)>> {
        let cur_bends: usize = self
            .vis
            .iter()
            .map(|&e| count_bends(&self.pts(e, &[]), GEPS))
            .sum();
        let cur_len: f64 = self.vis.iter().map(|&e| manhattan(&self.pts(e, &[]))).sum();
        let base: HashMap<usize, Vec<Seg>> = self
            .vis
            .iter()
            .map(|&e| (e, segs(&self.pts(e, &[]))))
            .collect();
        let mut by_edge: HashMap<usize, usize> = HashMap::new();
        for &(a, b, c) in &cur.pairs {
            *by_edge.entry(a).or_default() += c;
            *by_edge.entry(b).or_default() += c;
        }
        let base_bends: HashMap<usize, usize> = self
            .vis
            .iter()
            .map(|&e| (e, count_bends(&self.pts(e, &[]), GEPS)))
            .collect();
        let base_len: HashMap<usize, f64> = self
            .vis
            .iter()
            .map(|&e| (e, manhattan(&self.pts(e, &[]))))
            .collect();
        let groups = self.search_groups(cur);
        let mut options: HashMap<usize, Vec<PairCandidate>> = HashMap::new();
        for g in &groups {
            for &e in g {
                if options.contains_key(&e) {
                    continue;
                }
                let c = self.pair_candidates(e, cur, &base, &by_edge);
                if !c.is_empty() {
                    options.insert(e, c);
                }
            }
        }
        // (replacements, crossings, bends, length)
        let mut best: (Vec<(usize, Vec<Point>)>, usize, usize, f64) =
            (Vec::new(), cur.count, cur_bends, cur_len);
        for g in &groups {
            let crossing: Vec<usize> = g
                .iter()
                .copied()
                .filter(|e| cur.edges.contains(e))
                .collect();
            let opts: Vec<usize> = g
                .iter()
                .copied()
                .filter(|e| options.contains_key(e))
                .collect();
            for i in 0..opts.len() {
                let f = opts[i];
                for &s in &opts[i + 1..] {
                    if !crossing.contains(&f) && !crossing.contains(&s) {
                        continue;
                    }
                    for fc in &options[&f] {
                        for sc in &options[&s] {
                            let compatible = fc.conflicts.iter().all(|&c| c == s)
                                && sc.conflicts.iter().all(|&c| c == f)
                                && !(fc.segs.iter().any(|a| {
                                    sc.segs
                                        .iter()
                                        .any(|b| same_axis_overlap(a, b, 0.5) >= MIN_SHARED)
                                }) || touches(&fc.path, &sc.path));
                            if !compatible {
                                continue;
                            }
                            // pairCrossingCount
                            let cur_aff: usize = cur
                                .pairs
                                .iter()
                                .filter(|(a, b, _)| *a == f || *b == f || *a == s || *b == s)
                                .map(|p| p.2)
                                .sum();
                            let mut rep = cross_between(&fc.segs, &sc.segs);
                            for &o in &self.vis {
                                if o == f || o == s {
                                    continue;
                                }
                                rep += cross_between(&fc.segs, &base[&o])
                                    + cross_between(&sc.segs, &base[&o]);
                            }
                            let crossings = cur.count - cur_aff + rep;
                            if crossings >= cur.count {
                                continue;
                            }
                            let bends = cur_bends - base_bends[&f] - base_bends[&s]
                                + fc.total_bends
                                + sc.total_bends;
                            let length =
                                cur_len - base_len[&f] - base_len[&s] + fc.length + sc.length;
                            let better = crossings < best.1
                                || (crossings == best.1
                                    && (bends < best.2 || (bends == best.2 && length < best.3)));
                            if better {
                                best = (
                                    vec![(f, fc.path.clone()), (s, sc.path.clone())],
                                    crossings,
                                    bends,
                                    length,
                                );
                            }
                        }
                    }
                }
            }
        }
        (!best.0.is_empty()).then_some(best.0)
    }
}

/// `resolveRenderedOrthogonalCrossings`.
pub fn resolve_rendered_orthogonal_crossings(data: &mut LayoutData) {
    let (infos, _) = real_node_bounds(&data.nodes);
    // `realNodes`: not groups, not labels, with a size (in node order).
    if infos.is_empty() {
        return;
    }
    let rects: Vec<RectEntry> = infos
        .iter()
        .map(|i| RectEntry {
            id: i.id.clone(),
            rect: i.rect,
        })
        .collect();
    let fold = |f: &dyn Fn(&Rb) -> f64, min: bool| {
        infos.iter().map(|i| f(&i.rect)).fold(
            if min {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            },
            |a, b| if min { a.min(b) } else { a.max(b) },
        )
    };
    let outside = [
        fold(&|r| r.top, true) - ANCHOR,
        fold(&|r| r.bottom, false) + ANCHOR,
        fold(&|r| r.left, true) - ANCHOR,
        fold(&|r| r.right, false) + ANCHOR,
    ];
    for _ in 0..4 {
        let (best, paired) = {
            let ctx = Ctx {
                data,
                vis: visible(data),
                infos: infos.clone(),
                rects: rects.clone(),
                outside,
            };
            let cur = ctx.snapshot(&[]);
            if cur.count == 0 {
                return;
            }
            let mut best: Option<(usize, Vec<Point>)> = None;
            let mut best_c = cur.count;
            let mut best_b = usize::MAX;
            for &ei in &cur.edges {
                let eb = count_bends(&ctx.pts(ei, &[]), E);
                for cand in ctx.candidate_paths(ei) {
                    let hit = ctx.hits_node(ei, &cand);
                    let conflict = !hit && ctx.path_conflict(ei, &cand);
                    let cc = ctx.count_with(&cur, &[(ei, cand.clone())]);
                    let cb = count_bends(&cand, E);
                    if hit || conflict {
                        continue;
                    }
                    let improves = cc < cur.count || (cc == cur.count && cb < eb);
                    if !improves {
                        continue;
                    }
                    if cc > best_c || (cc == best_c && cb >= best_b) {
                        continue;
                    }
                    best = Some((ei, cand));
                    best_c = cc;
                    best_b = cb;
                }
            }
            let paired = if best.is_none() {
                ctx.best_paired(&cur)
            } else {
                None
            };
            (best, paired)
        };
        if let Some((ei, p)) = best {
            data.edges[ei].points = Some(p);
            continue;
        }
        let Some(p) = paired else { return };
        for (ei, pts) in p {
            data.edges[ei].points = Some(pts);
        }
    }
}
