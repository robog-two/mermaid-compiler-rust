//! mermaid 12.3's architecture layout: `getDataStructures` (a spatial map per
//! connected part, from the edges' port sides), `getAlignments` and
//! `getRelativeConstraints` (fcose constraints from those maps and the
//! `align` hints), and the two seeded fcose runs of `layoutArchitecture`
//! (the second starts from the first's result, as mermaid re-runs the layout
//! from its `layoutstop` handler).

use super::architecture::{Architecture, NodeKind, Side};
use crate::geom::{Point, Rect};
use crate::layout::force::fcose::{self, Constraints, Relative};
use std::collections::HashMap;

/// `iconSize`, `padding`, `idealEdgeLengthMultiplier`, `edgeElasticity`.
pub const ICON_SIZE: f64 = 80.0;
pub const PADDING: f64 = 40.0;
const IDEAL_MULT: f64 = 1.5;
const ELASTICITY: f64 = 0.45;

/// Positions mermaid's cytoscape run gives the nodes, and the bounding boxes
/// cytoscape reports for the groups (both in cytoscape coordinates: a
/// service is drawn with its icon's top left corner at its position).
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub node_pos: Vec<Point>,
    /// Positions after the first run, which fixed the bent edges' segment
    /// weights and distances (mermaid's `layoutstop` handler).
    pub first_pos: Vec<Point>,
    pub group_bbox: Vec<Rect>,
}

/// `ArchitectureDirectionPair` as `(source side, target side)`.
type Pair = (Side, Side);

fn valid_pair(a: Side, b: Side) -> Option<Pair> {
    (a != b).then_some((a, b))
}

fn is_x(s: Side) -> bool {
    matches!(s, Side::L | Side::R)
}

/// `shiftPositionByArchitectureDirectionPair` (y grows upwards: `T` is +1).
fn shift((x, y): (i64, i64), (lhs, rhs): Pair) -> (i64, i64) {
    let dx = |s: Side| if s == Side::L { -1 } else { 1 };
    let dy = |s: Side| if s == Side::T { 1 } else { -1 };
    if is_x(lhs) {
        if is_x(rhs) {
            (x + dx(lhs), y)
        } else {
            (x + dx(lhs), y + dy(rhs))
        }
    } else if is_x(rhs) {
        (x + if rhs == Side::L { 1 } else { -1 }, y + dy(lhs))
    } else {
        (x, y + dy(lhs))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// `getArchitectureDirectionAlignment`; `None` for `bend`.
fn alignment(a: Side, b: Side) -> Option<Axis> {
    if is_x(a) != is_x(b) {
        None
    } else if is_x(a) {
        Some(Axis::Horizontal)
    } else {
        Some(Axis::Vertical)
    }
}

/// Unordered pair of group ids (`architectureGroupAlignmentKey`).
fn group_key(a: Option<usize>, b: Option<usize>) -> (Option<usize>, Option<usize>) {
    if a <= b { (a, b) } else { (b, a) }
}

/// An insertion-ordered map with JS `Map.set` semantics.
fn map_set<K: PartialEq, V>(m: &mut Vec<(K, V)>, k: K, v: V) {
    match m.iter_mut().find(|(kk, _)| *kk == k) {
        Some(e) => e.1 = v,
        None => m.push((k, v)),
    }
}

type SpatialMap = Vec<(usize, (i64, i64))>;
type GroupAlignments = HashMap<(Option<usize>, Option<usize>), Axis>;

/// `ArchitectureDB.getDataStructures`.
fn data_structures(d: &Architecture) -> (Vec<SpatialMap>, GroupAlignments) {
    let n = d.nodes.len();
    // Each node's edges, in the order they were added.
    let mut node_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in d.edges.iter().enumerate() {
        node_edges[e.lhs.node].push(i);
        node_edges[e.rhs.node].push(i);
    }
    let mut group_al = GroupAlignments::new();
    let mut adj: Vec<Vec<(Pair, usize)>> = vec![Vec::new(); n];
    for id in 0..n {
        for &ei in &node_edges[id] {
            let e = &d.edges[ei];
            let (lg, rg) = (d.nodes[e.lhs.node].parent, d.nodes[e.rhs.node].parent);
            if let (Some(a), Some(b)) = (lg, rg) {
                if a != b {
                    if let Some(al) = alignment(e.lhs.side, e.rhs.side) {
                        group_al.insert(group_key(lg, rg), al);
                    }
                }
            }
            if e.lhs.node == id {
                if let Some(p) = valid_pair(e.lhs.side, e.rhs.side) {
                    map_set(&mut adj[id], p, e.rhs.node);
                }
            } else if let Some(p) = valid_pair(e.rhs.side, e.lhs.side) {
                map_set(&mut adj[id], p, e.lhs.node);
            }
        }
    }
    let mut visited = vec![false; n];
    let mut not_visited: Vec<usize> = (0..n).collect();
    let mut maps = Vec::new();
    while let Some(&start) = not_visited.first() {
        let mut map: SpatialMap = vec![(start, (0, 0))];
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(id) = queue.pop_front() {
            visited[id] = true;
            not_visited.retain(|&v| v != id);
            let pos = map.iter().find(|(k, _)| *k == id).map_or((0, 0), |e| e.1);
            for &(pair, rhs) in &adj[id] {
                if !visited[rhs] {
                    map_set(&mut map, rhs, shift(pos, pair));
                    queue.push_back(rhs);
                }
            }
        }
        maps.push(map);
    }
    (maps, group_al)
}

/// `getAlignments`: `(horizontal, vertical)` node groups.
fn alignments(
    d: &Architecture,
    maps: &[SpatialMap],
    group_al: &GroupAlignments,
) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    type AxisMap = Vec<(i64, Vec<(Option<usize>, Vec<usize>)>)>;
    let flatten = |m: &AxisMap, dir: Axis| -> Vec<Vec<usize>> {
        let mut flat: Vec<(String, Vec<usize>)> = Vec::new();
        for (num, groups) in m {
            let key = num.to_string();
            if groups.len() == 1 {
                map_set(&mut flat, key, groups[0].1.clone());
                continue;
            }
            let mut cnt = 0;
            for i in 0..groups.len() - 1 {
                for j in i + 1..groups.len() {
                    let (ga, na) = &groups[i];
                    let (gb, nb) = &groups[j];
                    let al = group_al.get(&group_key(*ga, *gb)).copied();
                    if al == Some(dir) || ga.is_none() || gb.is_none() {
                        let mut v = flat
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map(|e| e.1.clone())
                            .unwrap_or_default();
                        v.extend(na);
                        v.extend(nb);
                        map_set(&mut flat, key.clone(), v);
                    } else {
                        map_set(&mut flat, format!("{key}-{cnt}"), na.clone());
                        cnt += 1;
                        map_set(&mut flat, format!("{key}-{cnt}"), nb.clone());
                        cnt += 1;
                    }
                }
            }
        }
        flat.into_iter()
            .map(|(_, v)| v)
            .filter(|v| v.len() > 1)
            .collect()
    };
    let mut horizontal = Vec::new();
    let mut vertical = Vec::new();
    for map in maps {
        let mut h: AxisMap = Vec::new();
        let mut v: AxisMap = Vec::new();
        for &(id, (x, y)) in map {
            let g = d.nodes[id].parent;
            for (axis_map, k) in [(&mut h, y), (&mut v, x)] {
                if !axis_map.iter().any(|e| e.0 == k) {
                    axis_map.push((k, Vec::new()));
                }
                let groups = &mut axis_map
                    .iter_mut()
                    .find(|e| e.0 == k)
                    .expect("just added")
                    .1;
                match groups.iter_mut().find(|e| e.0 == g) {
                    Some(e) => e.1.push(id),
                    None => groups.push((g, vec![id])),
                }
            }
        }
        horizontal.extend(flatten(&h, Axis::Horizontal));
        vertical.extend(flatten(&v, Axis::Vertical));
    }
    // Declared `align` hints win over heuristic groups sharing a member.
    let declared: Vec<usize> = d.aligns.iter().flat_map(|a| a.members.clone()).collect();
    horizontal.retain(|g| !g.iter().any(|id| declared.contains(id)));
    vertical.retain(|g| !g.iter().any(|id| declared.contains(id)));
    for a in &d.aligns {
        if a.members.len() < 2 {
            continue;
        }
        if a.row {
            horizontal.push(a.members.clone());
        } else {
            vertical.push(a.members.clone());
        }
    }
    (horizontal, vertical)
}

/// `getRelativeConstraints`.
fn relative_constraints(d: &Architecture, maps: &[SpatialMap]) -> Vec<Relative> {
    let gap = IDEAL_MULT * ICON_SIZE;
    let mut out = Vec::new();
    let mut declared: Vec<(usize, usize)> = Vec::new();
    for a in &d.aligns {
        for w in a.members.windows(2) {
            declared.push((w[0], w[1]));
            declared.push((w[1], w[0]));
            out.push(if a.row {
                Relative::LeftRight(w[0], w[1], gap)
            } else {
                Relative::TopBottom(w[0], w[1], gap)
            });
        }
    }
    for map in maps {
        // invSpatialMap: the last node at a position wins.
        let mut inv: HashMap<(i64, i64), usize> = HashMap::new();
        for &(id, p) in map {
            inv.insert(p, id);
        }
        let mut queue = std::collections::VecDeque::from([(0i64, 0i64)]);
        let mut visited: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
        while let Some(cur) = queue.pop_front() {
            visited.insert(cur);
            let Some(&cur_id) = inv.get(&cur) else {
                continue;
            };
            for (dir, (dx, dy)) in [
                (Side::L, (-1, 0)),
                (Side::R, (1, 0)),
                (Side::T, (0, 1)),
                (Side::B, (0, -1)),
            ] {
                let np = (cur.0 + dx, cur.1 + dy);
                let Some(&new_id) = inv.get(&np) else {
                    continue;
                };
                if visited.contains(&np) {
                    continue;
                }
                queue.push_back(np);
                if declared.contains(&(cur_id, new_id)) {
                    continue;
                }
                out.push(match dir {
                    Side::L => Relative::LeftRight(new_id, cur_id, gap),
                    Side::R => Relative::LeftRight(cur_id, new_id, gap),
                    Side::T => Relative::TopBottom(new_id, cur_id, gap),
                    Side::B => Relative::TopBottom(cur_id, new_id, gap),
                });
            }
        }
    }
    out
}

/// The architecture's cytoscape elements: groups, services, junctions (node
/// indices `0..groups`, then `groups + node`), and its edges.
fn elements(d: &Architecture) -> (Vec<fcose::Node>, Vec<fcose::Edge>) {
    let ng = d.groups.len();
    let mut nodes: Vec<fcose::Node> = d
        .groups
        .iter()
        .map(|g| fcose::Node {
            parent: g.parent,
            // A group without members is a plain node: cytoscape's default
            // 30px body plus its padding on both sides.
            width: 30.0 + 2.0 * PADDING,
            height: 30.0 + 2.0 * PADDING,
            padding: PADDING,
        })
        .collect();
    let order: Vec<usize> = (0..d.nodes.len())
        .filter(|&i| matches!(d.nodes[i].kind, NodeKind::Service { .. }))
        .chain((0..d.nodes.len()).filter(|&i| matches!(d.nodes[i].kind, NodeKind::Junction)))
        .collect();
    let mut cy_of = vec![0; d.nodes.len()];
    for &i in &order {
        cy_of[i] = nodes.len();
        nodes.push(fcose::Node {
            parent: d.nodes[i].parent,
            width: ICON_SIZE,
            height: ICON_SIZE,
            padding: 0.0,
        });
    }
    debug_assert_eq!(nodes.len(), ng + d.nodes.len());
    let edges = d
        .edges
        .iter()
        .map(|e| {
            let same = d.nodes[e.lhs.node].parent == d.nodes[e.rhs.node].parent;
            fcose::Edge {
                source: cy_of[e.lhs.node],
                target: cy_of[e.rhs.node],
                ideal_length: if same {
                    IDEAL_MULT * ICON_SIZE
                } else {
                    0.5 * ICON_SIZE
                },
                elasticity: if same { ELASTICITY } else { 0.001 },
            }
        })
        .collect();
    (nodes, edges)
}

/// Lay the diagram out as mermaid does. `label_width(n)` is the width of a
/// service's title (0 for none), used for the group boxes cytoscape draws
/// around their members' labels. `None` when the edges and hints give fcose
/// no constraints (mermaid then tiles and prunes, which is not ported).
pub fn place(d: &Architecture, label_width: &dyn Fn(usize) -> f64) -> Option<Placement> {
    let ng = d.groups.len();
    let (maps, group_al) = data_structures(d);
    let (horizontal, vertical) = alignments(d, &maps, &group_al);
    let relative = relative_constraints(d, &maps);
    if horizontal.is_empty() && vertical.is_empty() && relative.is_empty() {
        return None;
    }
    let (nodes, edges) = elements(d);
    let order: Vec<usize> = (0..d.nodes.len())
        .filter(|&i| matches!(d.nodes[i].kind, NodeKind::Service { .. }))
        .chain((0..d.nodes.len()).filter(|&i| matches!(d.nodes[i].kind, NodeKind::Junction)))
        .collect();
    let mut cy_of = vec![0; d.nodes.len()];
    for (k, &i) in order.iter().enumerate() {
        cy_of[i] = ng + k;
    }
    let to_cy = |sets: Vec<Vec<usize>>| -> Vec<Vec<usize>> {
        sets.into_iter()
            .map(|s| s.into_iter().map(|i| cy_of[i]).collect())
            .collect()
    };
    let constraints = Constraints {
        vertical: to_cy(vertical),
        horizontal: to_cy(horizontal),
        relative: relative
            .into_iter()
            .map(|r| match r {
                Relative::LeftRight(a, b, g) => Relative::LeftRight(cy_of[a], cy_of[b], g),
                Relative::TopBottom(a, b, g) => Relative::TopBottom(cy_of[a], cy_of[b], g),
            })
            .collect(),
    };
    // Every node starts at the origin; the second run starts from the first.
    // Each run is moved onto the centre of the elements' bounding box before
    // it (cytoscape's, labels included).
    let mut pos = vec![(0.0, 0.0); nodes.len()];
    let mut first_pos = Vec::new();
    for run in 0..2 {
        let node_pos: Vec<Point> = (0..d.nodes.len())
            .map(|i| Point::new(pos[cy_of[i]].0, pos[cy_of[i]].1))
            .collect();
        let (_, all) = cy_boxes(d, &node_pos, &|g| pos[g], label_width);
        let center = (all.x + all.w / 2.0, all.y + all.h / 2.0);
        let rects = fcose::run(&nodes, &edges, &constraints, &pos, center);
        pos = rects.iter().map(|r| (r.center_x(), r.center_y())).collect();
        if run == 0 {
            first_pos = (0..d.nodes.len())
                .map(|i| Point::new(pos[cy_of[i]].0, pos[cy_of[i]].1))
                .collect();
        }
    }
    let node_pos: Vec<Point> = (0..d.nodes.len())
        .map(|i| Point::new(pos[cy_of[i]].0, pos[cy_of[i]].1))
        .collect();
    let (group_bbox, _) = cy_boxes(d, &node_pos, &|g| pos[g], label_width);
    Some(Placement {
        node_pos,
        first_pos,
        group_bbox,
    })
}

/// A node's `source-endpoint` / `target-endpoint` (`'100% 50%'` etc.:
/// fractions of the node's size from its position, which is where mermaid
/// draws the icon's top left corner).
fn endpoint(p: Point, side: Side) -> Point {
    let (fx, fy) = match side {
        Side::L => (0.0, 0.5),
        Side::R => (1.0, 0.5),
        Side::T => (0.5, 0.0),
        Side::B => (0.5, 1.0),
    };
    Point::new(p.x + fx * ICON_SIZE, p.y + fy * ICON_SIZE)
}

/// mermaid's `getSegmentWeights`: `(weight, distance)` that put the single
/// segment point of a bent edge at `point`.
fn segment_weights(s: Point, t: Point, point: Point) -> (f64, f64) {
    let slope = (s.y - t.y) / (s.x - t.x);
    let mut d = (point.y - s.y + (s.x - point.x) * (s.y - t.y) / (s.x - t.x))
        / (1.0 + slope * slope).sqrt();
    let mut w = ((point.y - s.y).powi(2) + (point.x - s.x).powi(2) - d * d).sqrt();
    w /= ((t.x - s.x).powi(2) + (t.y - s.y).powi(2)).sqrt();
    let delta1 = if (t.x - s.x) * (point.y - s.y) - (t.y - s.y) * (point.x - s.x) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let delta2 = if (t.x - s.x) * (point.x - s.x) + (t.y - s.y) * (point.y - s.y) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    d = d.abs() * delta1;
    w *= delta2;
    (w, d)
}

/// An edge as svgDraw.ts draws it: `M start L mid L end`, from the
/// cytoscape endpoints and midpoint, with `{group}` ends pushed past the
/// group border and junction ends pulled to the junction's centre.
pub fn edge_points(
    d: &Architecture,
    p: &Placement,
    e: &super::architecture::ArchEdge,
) -> [Point; 3] {
    let (a, b) = (e.lhs.node, e.rhs.node);
    let (s, t) = (
        endpoint(p.node_pos[a], e.lhs.side),
        endpoint(p.node_pos[b], e.rhs.side),
    );
    let mid = if is_x(e.lhs.side) != is_x(e.rhs.side) {
        // `segments` edge: weight and distance fixed after the first run.
        let (pa, pb) = (p.first_pos[a], p.first_pos[b]);
        let (w, dist) = if pa.x != pb.x && pa.y != pb.y {
            let (s1, t1) = (endpoint(pa, e.lhs.side), endpoint(pb, e.rhs.side));
            let corner = if is_x(e.lhs.side) {
                Point::new(t1.x, s1.y)
            } else {
                Point::new(s1.x, t1.y)
            };
            segment_weights(s1, t1, corner)
        } else {
            (0.0, 0.5)
        };
        let (dx, dy) = (t.x - s.x, t.y - s.y);
        let l = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if l > 0.0 {
            (-dy / l, dx / l)
        } else {
            (0.0, 0.0)
        };
        Point::new(
            s.x * (1.0 - w) + t.x * w + nx * dist,
            s.y * (1.0 - w) + t.y * w + ny * dist,
        )
    } else {
        Point::new((s.x + t.x) / 2.0, (s.y + t.y) / 2.0)
    };
    let shift = |mut q: Point, end: &super::architecture::EdgeEnd| {
        let group_shift = PADDING + 4.0;
        if end.group {
            match end.side {
                Side::L => q.x -= group_shift,
                Side::R => q.x += group_shift,
                Side::T => q.y -= group_shift,
                // +18: the service label below extends the group's bottom.
                Side::B => q.y += group_shift + 18.0,
            }
        } else if matches!(d.nodes[end.node].kind, NodeKind::Junction) {
            let half = ICON_SIZE / 2.0;
            match end.side {
                Side::L => q.x += half,
                Side::R => q.x -= half,
                Side::T => q.y += half,
                Side::B => q.y -= half,
            }
        }
        q
    };
    [shift(s, &e.lhs), mid, shift(t, &e.rhs)]
}

/// Cytoscape's bounding boxes for the groups (compounds: their members'
/// boxes, 1px outlines and service labels included, plus the padding) and for
/// all elements together. `group_pos` places a group without members.
fn cy_boxes(
    d: &Architecture,
    node_pos: &[Point],
    group_pos: &dyn Fn(usize) -> (f64, f64),
    label_width: &dyn Fn(usize) -> f64,
) -> (Vec<Rect>, Rect) {
    let ng = d.groups.len();
    let leaf_box = |i: usize| {
        let p = node_pos[i];
        let lw = label_width(i);
        let half = (ICON_SIZE / 2.0 + 1.0).max(lw / 2.0 + 1.0);
        let bottom = if lw > 0.0 { 59.0 } else { 41.0 };
        (
            Rect::new(p.x - half, p.y - 41.0, 2.0 * half, 41.0 + bottom),
            lw > 0.0,
        )
    };
    let mut group_bbox: Vec<Option<Rect>> = vec![None; ng];
    // Groups nest at most as deep as there are groups; resolve bottom-up.
    for _ in 0..ng {
        for g in (0..ng).rev() {
            let mut acc: Option<(f64, f64, f64, f64)> = None;
            let mut add = |r: Rect, bottom_pad: f64| {
                let e = (
                    r.x - 41.5,
                    r.y - 41.5,
                    r.right() + 41.5,
                    r.bottom() + bottom_pad,
                );
                acc = Some(match acc {
                    None => e,
                    Some(a) => (a.0.min(e.0), a.1.min(e.1), a.2.max(e.2), a.3.max(e.3)),
                });
            };
            for i in 0..d.nodes.len() {
                if d.nodes[i].parent == Some(g) {
                    let (r, labelled) = leaf_box(i);
                    add(r, if labelled { 40.5 } else { 41.5 });
                }
            }
            for (c, cg) in d.groups.iter().enumerate() {
                if cg.parent == Some(g) {
                    if let Some(r) = group_bbox[c] {
                        add(r, 41.5);
                    }
                }
            }
            group_bbox[g] = Some(match acc {
                Some((x0, y0, x1, y1)) => Rect::new(x0, y0, x1 - x0, y1 - y0),
                None => {
                    let p = group_pos(g);
                    let s = 30.0 + 2.0 * PADDING + 2.0;
                    Rect::new(p.0 - s / 2.0, p.1 - s / 2.0, s, s)
                }
            });
        }
    }
    let groups: Vec<Rect> = group_bbox
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect();
    let mut all: Option<Rect> = None;
    let mut add = |r: Rect| all = Some(all.map_or(r, |a| a.union(&r)));
    for i in 0..d.nodes.len() {
        add(leaf_box(i).0);
    }
    for &r in &groups {
        add(r);
    }
    (groups, all.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::architecture::parse;

    fn titled(d: &Architecture) -> impl Fn(usize) -> f64 + '_ {
        // Any title narrower than the icon gives the same boxes.
        |i| match &d.nodes[i].kind {
            NodeKind::Service { title: Some(_), .. } => 40.0,
            _ => 0.0,
        }
    }

    #[test]
    fn reproduces_mermaids_fcose_positions() {
        // Positions from mermaid 12.3.0 in Chromium.
        let d = parse(
            "architecture-beta
    group g0(cloud)[Group 0]
    service s0(server)[Svc 0]
    service s1(server)[Svc 1] in g0
    service s2(server)[Svc 2]
    service s3(server)[Svc 3] in g0
    s2:B --> T:s3
    s3:L -- R:s1
    s1:L -- R:s0",
            None,
        )
        .unwrap();
        let p = place(&d, &titled(&d)).unwrap();
        let want = [
            (-229.5803166068381, 105.67886565152662),
            (-9.62211433371776, 105.67886565152662),
            (191.08031660683807, -110.17886565152662),
            (191.08031660683807, 105.67886565152662),
        ];
        for (q, (x, y)) in p.node_pos.iter().zip(want) {
            assert!(
                (q.x - x).abs() < 1e-9 && (q.y - y).abs() < 1e-9,
                "{q:?} vs ({x}, {y})"
            );
        }
    }

    #[test]
    fn reproduces_mermaid_with_nested_groups_and_junctions() {
        let d = parse(
            "architecture-beta
    group g0(cloud)[Group 0]
    group g1(cloud)[Group 1] in g0
    group g2(cloud)[Group 2] in g0
    service s0(server)[Svc 0]
    service s1(server)[Svc 1] in g1
    service s2(server)[Svc 2] in g2
    service s3(server)[Svc 3] in g0
    service s4(server)[Svc 4]
    service s5(server)[Svc 5]
    service s6(server)[Svc 6]
    service s7(server)[Svc 7] in g2
    service s8(server)[Svc 8]
    service s9(server)[Svc 9]
    service s10(server)[Svc 10] in g1
    service s11(server)[Svc 11] in g2
    service s12(server)[Svc 12] in g1
    service s13(server)[Svc 13] in g0
    junction j0 in g0
    junction j1
    s13:T <-- B:j0
    s13:L --> R:s6
    j0:R -- T:s7
    j0:T -- B:s9
    s6:B -- T:s5
    s9:T -- B:s3
    s9:T <-- B:s2
    s3:L <-- R:s1
    j0:R -- T:s8
    s9:L <-- R:s10
    s2:R -- L:s0
    s2:L <--> R:s11
    s5:B <-- L:s4
    s13:L --> R:j1
    s5:B <-- T:s12",
            None,
        )
        .unwrap();
        let p = place(&d, &titled(&d)).unwrap();
        // mermaid's positions, compared relative to s0 (the absolute offset
        // depends on cytoscape's label metrics).
        let want = [
            (1688.4521535071972, -196.54615832202728),
            (-1542.7152394666628, 151.6343191514637),
            (1398.1293252210671, -196.54615832202728),
            (-1422.7152394666628, 151.6343191514637),
            (-1489.0624948773893, -556.1405213486032),
            (-1609.952153507198, -373.18215148569),
            (-1609.952153507198, -556.1405213486032),
            (412.78093797410963, -200.50424008247643),
            (1688.4521535071972, 222.68937904467037),
            (1398.1293252210671, 222.68937904467037),
            (1278.1293252210671, 222.68937904467037),
            (1278.1293252210671, -196.54615832202728),
            (-1609.952153507198, 3.689134819220527),
            (1398.1293252210671, 551.6405213486032),
        ];
        let o = p.node_pos[0];
        for (q, (x, y)) in p.node_pos.iter().zip(want) {
            let (dx, dy) = (q.x - o.x - (x - want[0].0), q.y - o.y - (y - want[0].1));
            assert!(dx.abs() < 1e-9 && dy.abs() < 1e-9, "{q:?} vs ({x}, {y})");
        }
    }

    #[test]
    fn segment_weights_reproduce_the_corner() {
        let (s, t) = (Point::new(10.0, 20.0), Point::new(110.0, 220.0));
        for corner in [Point::new(110.0, 20.0), Point::new(10.0, 220.0)] {
            let (w, dist) = segment_weights(s, t, corner);
            let (dx, dy) = (t.x - s.x, t.y - s.y);
            let l = (dx * dx + dy * dy).sqrt();
            let q = Point::new(
                s.x * (1.0 - w) + t.x * w - dy / l * dist,
                s.y * (1.0 - w) + t.y * w + dx / l * dist,
            );
            assert!(
                (q.x - corner.x).abs() < 1e-9 && (q.y - corner.y).abs() < 1e-9,
                "{q:?}"
            );
        }
    }

    #[test]
    fn spatial_map_follows_port_sides() {
        let d = parse(
            "architecture-beta\n service a(server)[A]\n service b(server)[B]\n service c(server)[C]\n a:R -- L:b\n b:B -- T:c",
            None,
        )
        .unwrap();
        let (maps, _) = data_structures(&d);
        assert_eq!(maps, vec![vec![(0, (0, 0)), (1, (1, 0)), (2, (1, -1))]]);
        let rel = relative_constraints(&d, &maps);
        assert_eq!(
            rel,
            vec![
                Relative::LeftRight(0, 1, 120.0),
                Relative::TopBottom(1, 2, 120.0),
            ]
        );
        let (h, v) = alignments(&d, &maps, &HashMap::new());
        assert_eq!((h, v), (vec![vec![0, 1]], vec![vec![1, 2]]));
    }
}
