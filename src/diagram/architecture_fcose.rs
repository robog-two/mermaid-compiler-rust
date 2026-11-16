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

/// The `architecture` config fields the layout reads with `getConfigField`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArchConfig {
    /// `iconSize`: a service's or junction's square.
    pub icon_size: f64,
    /// `padding`: a group's padding (and the viewBox margin).
    pub padding: f64,
    /// `fontSize`: cytoscape's font for the service labels it sizes groups by.
    pub font_size: f64,
    /// `idealEdgeLengthMultiplier`: ideal length within a group, in icons.
    pub ideal_mult: f64,
    /// `edgeElasticity` within a group.
    pub elasticity: f64,
    /// `randomize`, `nodeSeparation`, `seed`, `numIter`.
    pub fcose: fcose::Options,
}

impl Default for ArchConfig {
    fn default() -> Self {
        ArchConfig {
            icon_size: 80.0,
            padding: 40.0,
            font_size: 16.0,
            ideal_mult: 1.5,
            elasticity: 0.45,
            fcose: fcose::Options::default(),
        }
    }
}

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
fn relative_constraints(d: &Architecture, maps: &[SpatialMap], c: &ArchConfig) -> Vec<Relative> {
    let gap = c.ideal_mult * c.icon_size;
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
fn elements(d: &Architecture, c: &ArchConfig) -> (Vec<fcose::Node>, Vec<fcose::Edge>) {
    let ng = d.groups.len();
    let mut nodes: Vec<fcose::Node> = d
        .groups
        .iter()
        .map(|g| fcose::Node {
            parent: g.parent,
            // A group without members is a plain node: cytoscape's default
            // 30px body plus its padding on both sides.
            width: 30.0 + 2.0 * c.padding,
            height: 30.0 + 2.0 * c.padding,
            padding: c.padding,
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
            width: c.icon_size,
            height: c.icon_size,
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
                    c.ideal_mult * c.icon_size
                } else {
                    0.5 * c.icon_size
                },
                elasticity: if same { c.elasticity } else { 0.001 },
            }
        })
        .collect();
    (nodes, edges)
}

/// Lay the diagram out as mermaid does. `measure(text, edge)` is the width
/// cytoscape measures for a label (a service's title, or with `edge` an edge's
/// label), which widens the boxes it puts around nodes, edges and groups.
/// `c` holds the `architecture` config fields mermaid reads for the layout.
pub fn place(d: &Architecture, measure: &dyn Fn(&str, bool) -> f64, c: &ArchConfig) -> Placement {
    let ng = d.groups.len();
    let (maps, group_al) = data_structures(d);
    let (horizontal, vertical) = alignments(d, &maps, &group_al);
    let relative = relative_constraints(d, &maps, c);
    let (nodes, edges) = elements(d, c);
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
    let mut first_pos: Vec<Point> = Vec::new();
    for run in 0..2 {
        let node_pos: Vec<Point> = (0..d.nodes.len())
            .map(|i| Point::new(pos[cy_of[i]].0, pos[cy_of[i]].1))
            .collect();
        let first = (run == 1).then_some(first_pos.as_slice());
        let (_, all) = cy_boxes(d, &node_pos, first, &|g| pos[g], measure, c);
        let center = (all.x + all.w / 2.0, all.y + all.h / 2.0);
        let rects = fcose::run(&nodes, &edges, &constraints, &pos, center, c.fcose);
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
    let (group_bbox, _) = cy_boxes(d, &node_pos, Some(&first_pos), &|g| pos[g], measure, c);
    Placement {
        node_pos,
        first_pos,
        group_bbox,
    }
}

/// A node's `source-endpoint` / `target-endpoint` (`'100% 50%'` etc.:
/// fractions of the node's size from its position, which is where mermaid
/// draws the icon's top left corner).
fn endpoint(p: Point, side: Side, icon: f64) -> Point {
    let (fx, fy) = match side {
        Side::L => (0.0, 0.5),
        Side::R => (1.0, 0.5),
        Side::T => (0.5, 0.0),
        Side::B => (0.5, 1.0),
    };
    Point::new(p.x + fx * icon, p.y + fy * icon)
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

/// Cytoscape's source endpoint, midpoint and target endpoint of an edge with
/// the nodes at `now`. A `segments` edge (ports on perpendicular sides) bends
/// at the weight and distance mermaid's `layoutstop` handler derived from the
/// first run's positions `first` (cytoscape's default style before that).
fn cy_edge(
    now: &[Point],
    first: Option<&[Point]>,
    e: &super::architecture::ArchEdge,
    icon: f64,
) -> [Point; 3] {
    let (a, b) = (e.lhs.node, e.rhs.node);
    let (s, t) = (
        endpoint(now[a], e.lhs.side, icon),
        endpoint(now[b], e.rhs.side, icon),
    );
    let mid = if is_x(e.lhs.side) != is_x(e.rhs.side) {
        let (w, dist) = match first {
            Some(f) if f[a].x != f[b].x && f[a].y != f[b].y => {
                let (s1, t1) = (
                    endpoint(f[a], e.lhs.side, icon),
                    endpoint(f[b], e.rhs.side, icon),
                );
                let corner = if is_x(e.lhs.side) {
                    Point::new(t1.x, s1.y)
                } else {
                    Point::new(s1.x, t1.y)
                };
                segment_weights(s1, t1, corner)
            }
            _ => (0.0, 0.5),
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
    [s, mid, t]
}

/// An edge as svgDraw.ts draws it: `M start L mid L end`, from the
/// cytoscape endpoints and midpoint, with `{group}` ends pushed past the
/// group border and junction ends pulled to the junction's centre.
pub fn edge_points(
    d: &Architecture,
    p: &Placement,
    e: &super::architecture::ArchEdge,
    c: &ArchConfig,
) -> [Point; 3] {
    let [s, mid, t] = cy_edge(&p.node_pos, Some(&p.first_pos), e, c.icon_size);
    let shift = |mut q: Point, end: &super::architecture::EdgeEnd| {
        let group_shift = c.padding + 4.0;
        if end.group {
            match end.side {
                Side::L => q.x -= group_shift,
                Side::R => q.x += group_shift,
                Side::T => q.y -= group_shift,
                // +18: the service label below extends the group's bottom.
                Side::B => q.y += group_shift + 18.0,
            }
        } else if matches!(d.nodes[end.node].kind, NodeKind::Junction) {
            let half = c.icon_size / 2.0;
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
    first_pos: Option<&[Point]>,
    group_pos: &dyn Fn(usize) -> (f64, f64),
    measure: &dyn Fn(&str, bool) -> f64,
    c: &ArchConfig,
) -> (Vec<Rect>, Rect) {
    let ng = d.groups.len();
    // A leaf's box: its body widened by 1px, and its label (`fontSize` tall,
    // below the node) by cytoscape's 2px `marginOfError`. A group sizes
    // itself from its children's uncached bounds, where the label gets no
    // more (`label_margin` 0); the cached bounds of the whole graph add the
    // 1px to the labels too (`label_margin` 1).
    let leaf_box = |i: usize, label_margin: f64| {
        let p = node_pos[i];
        let lw = match &d.nodes[i].kind {
            NodeKind::Service { title: Some(t), .. } if !t.is_empty() => measure(t, false),
            _ => 0.0,
        };
        let icon_half = c.icon_size / 2.0 + 1.0;
        let m = 2.0 + label_margin;
        let (half, bottom) = if lw > 0.0 {
            (
                icon_half.max(lw / 2.0 + m),
                icon_half.max(c.icon_size / 2.0 + c.font_size + m),
            )
        } else {
            (icon_half, icon_half)
        };
        Rect::new(p.x - half, p.y - icon_half, 2.0 * half, icon_half + bottom)
    };
    let mut group_bbox: Vec<Option<Rect>> = vec![None; ng];
    // Groups nest at most as deep as there are groups; resolve bottom-up.
    for _ in 0..ng {
        for g in (0..ng).rev() {
            let mut acc: Option<(f64, f64, f64, f64)> = None;
            let mut add = |r: Rect| {
                let m = c.padding + 1.5;
                let e = (r.x - m, r.y - m, r.right() + m, r.bottom() + m);
                acc = Some(match acc {
                    None => e,
                    Some(a) => (a.0.min(e.0), a.1.min(e.1), a.2.max(e.2), a.3.max(e.3)),
                });
            };
            for i in 0..d.nodes.len() {
                if d.nodes[i].parent == Some(g) {
                    add(leaf_box(i, 0.0));
                }
            }
            for (ci, cg) in d.groups.iter().enumerate() {
                if cg.parent == Some(g) {
                    if let Some(r) = group_bbox[ci] {
                        add(r);
                    }
                }
            }
            group_bbox[g] = Some(match acc {
                Some((x0, y0, x1, y1)) => Rect::new(x0, y0, x1 - x0, y1 - y0),
                None => {
                    let p = group_pos(g);
                    let s = 30.0 + 2.0 * c.padding + 2.0;
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
        add(leaf_box(i, 1.0));
    }
    for &r in &groups {
        add(r);
    }
    // Edges: their source, middle and target points widened by half the
    // default 3px width and the 1px margin, and their labels (cytoscape's
    // default 16px font) by its 2px `marginOfError` and the same margin.
    for e in &d.edges {
        let pts = cy_edge(node_pos, first_pos, e, c.icon_size);
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for q in pts {
            x0 = x0.min(q.x);
            y0 = y0.min(q.y);
            x1 = x1.max(q.x);
            y1 = y1.max(q.y);
        }
        let m = 2.5;
        add(Rect::new(
            x0 - m,
            y0 - m,
            x1 - x0 + 2.0 * m,
            y1 - y0 + 2.0 * m,
        ));
        if let Some(l) = e.label.as_deref().filter(|l| !l.is_empty()) {
            let (hw, hh) = (measure(l, true) / 2.0 + 3.0, 8.0 + 3.0);
            add(Rect::new(pts[1].x - hw, pts[1].y - hh, 2.0 * hw, 2.0 * hh));
        }
    }
    (groups, all.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::architecture::parse;

    /// Any title narrower than the icon gives the same boxes.
    fn titled(_: &Architecture) -> impl Fn(&str, bool) -> f64 {
        |_, _| 40.0
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
        let p = place(&d, &titled(&d), &ArchConfig::default());
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
        let p = place(&d, &titled(&d), &ArchConfig::default());
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

    fn randomize() -> ArchConfig {
        let mut c = ArchConfig::default();
        c.fcose.randomize = true;
        c
    }

    /// `place` with `randomize: true` gives exactly mermaid's positions
    /// (and, where listed, its group rects: `x`, `y`, `width`, `height`).
    fn assert_randomized(src: &str, want: &[(f64, f64)], groups: &[(f64, f64, f64, f64)]) {
        let d = parse(src, None).unwrap();
        let p = place(&d, &titled(&d), &randomize());
        let got: Vec<(f64, f64)> = p.node_pos.iter().map(|q| (q.x, q.y)).collect();
        assert_eq!(got, want);
        let got: Vec<(f64, f64, f64, f64)> = p
            .group_bbox
            .iter()
            .map(|r| (r.x + 40.0, r.y + 40.0, r.w, r.h))
            .collect();
        assert_eq!(got, groups);
    }

    // Positions from mermaid 12.3.0 in Chromium with `randomize: true`.

    #[test]
    fn randomize_reproduces_mermaid_with_junctions_and_bends() {
        assert_randomized(
            "architecture-beta
    service s0(server)[Svc 0]
    service s1(server)[Svc 1]
    service s2(server)[Svc 2]
    service s3(server)[Svc 3]
    service s4(server)[Svc 4]
    service s5(server)[Svc 5]
    service s6(server)[Svc 6]
    service s7(server)[Svc 7]
    service s8(server)[Svc 8]
    service s9(server)[Svc 9]
    service s10(server)[Svc 10]
    service s11(server)[Svc 11]
    service s12(server)[Svc 12]
    service s13(server)[Svc 13]
    junction j0
    junction j1
    s9:R <--> L:s10
    s9:L <-- R:s6
    s10:B -- T:s7
    s10:B <--> T:s13
    s10:R <--> L:s1
    s1:B --> T:j1
    s9:B --> T:s0
    s10:L <-- R:s8
    s9:L --> R:s3
    s13:T --> R:s2
    s2:T <-- B:s12
    j1:L <-- R:s11
    s13:R -- L:s4
    s13:B <--> T:j0
    s7:T --> B:s5
    s0:L --> R:s5",
            &[
                (-81.32819616425863, 143.57098523899288),
                (268.2722159389102, -13.377516985289951),
                (-81.32819616425863, -13.377516985289951),
                (-225.27221593891022, -13.377516985289951),
                (268.2722159389102, 143.57098523899288),
                (-225.27221593891022, 143.57098523899288),
                (81.08107055104679, -187.32102370879306),
                (-225.27221593891022, 280.7132019189605),
                (-81.32819616425863, -13.377516985289951),
                (-81.32819616425863, -13.377516985289951),
                (66.43445986407114, -13.377516985289951),
                (66.43445986407114, 143.57098523899288),
                (-81.32819616425863, -221.21320191896052),
                (66.43445986407114, 143.57098523899288),
                (66.43445986407114, 280.7132019189605),
                (268.2722159389102, 143.57098523899288),
            ],
            &[],
        );
    }

    #[test]
    fn randomize_reproduces_mermaid_with_empty_groups() {
        // Two empty groups and a junction: the spectral draft joins the
        // disconnected parts through a dummy node.
        assert_randomized(
            "architecture-beta
    group g0(cloud)[Group 0]
    group g1(cloud)[Group 1]
    group g2(cloud)[Group 2]
    service s0(server)[Svc 0] in g2
    service s1(server)[Svc 1]
    service s2(server)[Svc 2]
    service s3(server)[Svc 3]
    service s4(server)[Svc 4]
    junction j0
    s3:L -- R:s0
    s0:L --> R:s2
    s2:B <--> T:s1
    s2:L --> R:j0
    s3:L <--> R:s4
    s4:L <--> R:s2",
            &[
                (66.09637631694586, -116.31941961313751),
                (-136.21898091296845, 85.7645847765595),
                (-136.21898091296845, -116.31941961313751),
                (267.8822831075071, -116.31941961313751),
                (66.09637631694586, -116.31941961313751),
                (-336.70950750972077, -116.31941961313751),
            ],
            &[
                (305.70950750972077, 37.61493750966889, 112.0, 112.0),
                (106.34304385655534, 140.8194196131375, 112.0, 112.0),
                (23.596376316945864, -158.8194196131375, 165.0, 182.0),
            ],
        );
    }

    #[test]
    fn randomize_reproduces_mermaid_with_disconnected_groups() {
        assert_randomized(
            "architecture-beta
    group g1(cloud)[G1]
    group g2(cloud)[G2]
    service a(server)[A] in g1
    service b(server)[B] in g1
    service c(server)[C] in g2
    service d(server)[D] in g2
    service e(server)[E]
    a:R -- L:b
    c:T -- B:d",
            &[
                (40.351070802823415, 14.00674526082635),
                (241.0376365640085, 14.00674526082635),
                (-241.0376365640085, 51.606121790450715),
                (-241.0376365640085, -149.08044397073428),
                (-41.48505813605246, 222.58044397073428),
            ],
            &[
                (
                    -2.148929197176585,
                    -28.49325473917365,
                    365.6865657611851,
                    182.0,
                ),
                (
                    -283.5376365640085,
                    -191.58044397073428,
                    165.0,
                    382.68656576118497,
                ),
            ],
        );
    }

    #[test]
    fn randomize_with_two_nodes_starts_the_second_at_the_origin() {
        // fcose places the second of two nodes at x = NaN (it adds mermaid's
        // `idealEdgeLength` function to a number) and then at rect.x = 0.
        assert_randomized(
            "architecture-beta
    service a(server)[A]
    service b(database)[B]
    a:R -- L:b",
            &[(-79.59328288059253, 18.0), (121.09328288059253, 18.0)],
            &[],
        );
    }

    const R12: &str = "architecture-beta
    group g0(cloud)[Group 0]
    group g1(cloud)[Group 1]
    service s0(server)[Svc 0]
    service s1(server)[Svc 1] in g0
    service s2(server)[Svc 2] in g0
    service s3(server)[Svc 3]
    service s4(server)[Svc 4] in g0
    service s5(server)[Svc 5] in g0
    service s6(server)[Svc 6]
    service s7(server)[Svc 7]
    junction j0 in g0
    s0:T -- L:s7
    s0:R --> B:j0
    j0:T <--> B:s6
    s7:L --> R:s3
    s0:T --> B:s4
    j0:L --> R:s1
    s4:R -- L:s2
    s7:L <-- R:s5
    s0:T -- B:s3
    j0:B <--> T:s2";

    /// `place` under `c` gives exactly mermaid's service positions (each
    /// service's titles measure `label` wide).
    fn assert_config(src: &str, c: &ArchConfig, label: f64, want: &[(f64, f64)]) {
        let d = parse(src, None).unwrap();
        let p = place(&d, &move |_: &str, _: bool| label, c);
        let got: Vec<(f64, f64)> = d
            .nodes
            .iter()
            .zip(&p.node_pos)
            .filter(|(n, _)| matches!(n.kind, NodeKind::Service { .. }))
            .map(|(_, q)| (q.x, q.y))
            .collect();
        assert_eq!(got, want);
    }

    // Positions from mermaid 12.3.0 in Chromium with non-default
    // `architecture` config.

    #[test]
    fn seed_and_num_iter_reproduce_mermaid() {
        let mut c = randomize();
        c.fcose.seed = 7;
        c.fcose.num_iter = 100;
        let (a, b) = (-21.7273911111386, 246.58620346257032);
        assert_config(
            R12,
            &c,
            40.0,
            &[
                (a, -452.1869309271797),
                (a, 472.7584396653098),
                (b, 606.1869309271798),
                (a, -572.1869309271797),
                (a, 606.1869309271798),
                (a, -572.1869309271797),
                (b, -452.1869309271797),
                (b, -572.1869309271797),
            ],
        );
    }

    #[test]
    fn every_layout_field_reproduces_mermaid() {
        let mut c = randomize();
        c.fcose.seed = 9;
        c.fcose.node_separation = 50.0;
        c.icon_size = 64.0;
        c.padding = 24.0;
        c.font_size = 20.0;
        c.ideal_mult = 2.0;
        c.elasticity = 0.3;
        let (a, b) = (-139.11087377948934, 147.11087377948942);
        assert_config(
            R12,
            &c,
            50.0,
            &[
                (a, -539.7607331044737),
                (a, 442.24201774674566),
                (b, 577.4172525871509),
                (a, -675.0256374331648),
                (a, 577.4172525871509),
                (a, -675.0256374331648),
                (b, -539.7607331044737),
                (b, -675.0256374331648),
            ],
        );
    }

    #[test]
    fn seed_zero_is_deterministic() {
        let d = parse(R12, None).unwrap();
        let mut c = randomize();
        c.fcose.seed = 0;
        let a = place(&d, &titled(&d), &c);
        let b = place(&d, &titled(&d), &c);
        assert_eq!(a.node_pos, b.node_pos);
        c.fcose.seed = 1;
        assert_ne!(a.node_pos, place(&d, &titled(&d), &c).node_pos);
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
        let rel = relative_constraints(&d, &maps, &ArchConfig::default());
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
