//! Layered (Sugiyama-style) graph layout, written from scratch.
//!
//! Pipeline, all pure:
//! 1. remove self-loops, break cycles by reversing DFS back edges;
//! 2. rank with longest-path + source tightening (minlen-aware);
//! 3. split long edges into dummy vertices (edge labels become sized dummies, as in dagre);
//! 4. add left/right border vertices for every cluster on every rank it spans;
//! 5. order layers with weighted barycentre sweeps, keeping clusters contiguous,
//!    scoring with an O(E log V) crossing count and keeping the best ordering;
//! 6. assign cross-axis coordinates by repeatedly solving, per layer, the exact
//!    weighted least-squares problem "be near your neighbours, keep minimum
//!    separation" with the pool-adjacent-violators algorithm;
//! 7. stack ranks along the rank axis and rotate for TB / BT / LR / RL.

use crate::geom::{Point, Rect};

pub mod boxes;
pub mod rank;

/// Flow direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl Direction {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::LeftRight | Direction::RightLeft)
    }

    /// Parse mermaid's `TB`, `TD`, `BT`, `LR`, `RL` (and `<`, `>`, `^`, `v`).
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "TB" | "TD" | "v" => Some(Direction::TopBottom),
            "BT" | "^" => Some(Direction::BottomTop),
            "LR" | ">" => Some(Direction::LeftRight),
            "RL" | "<" => Some(Direction::RightLeft),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub minlen: usize,
    /// Label size in output space (width, height).
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutCluster {
    pub parent: Option<usize>,
    /// Height of the title band reserved at the top of the cluster.
    pub title_height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutInput {
    pub nodes: Vec<LayoutNode>,
    pub edges: Vec<LayoutEdge>,
    pub clusters: Vec<LayoutCluster>,
    pub direction: Direction,
    pub node_sep: f64,
    pub rank_sep: f64,
    pub edge_sep: f64,
    pub cluster_padding: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRoute {
    /// From source centre to target centre (unclipped), in output space.
    pub points: Vec<Point>,
    pub label: Option<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOutput {
    pub nodes: Vec<Point>,
    pub edges: Vec<EdgeRoute>,
    /// One rectangle per input cluster (zero-sized if the cluster is empty).
    pub clusters: Vec<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Real,
    Dummy { labeled: bool },
    Border { cluster: usize, side: Side },
}

#[derive(Debug, Clone)]
struct Vertex {
    kind: Kind,
    rank: usize,
    /// Size in layout space: `w` along the cross axis, `h` along the rank axis.
    w: f64,
    h: f64,
    cluster: Option<usize>,
}

/// Vertices of a split edge (oriented), its label vertex, and whether it was reversed.
type Chain = (Vec<usize>, Option<usize>, bool);

struct Oriented {
    from: usize,
    to: usize,
    minlen: usize,
    reversed: bool,
}

/// Run the full layout.
pub fn layout(input: &LayoutInput) -> LayoutOutput {
    let horizontal = input.direction.is_horizontal();
    let swap = |w: f64, h: f64| if horizontal { (h, w) } else { (w, h) };
    let n = input.nodes.len();

    let has_labels = input
        .edges
        .iter()
        .any(|e| e.label.is_some() && e.from != e.to);
    let rank_scale = if has_labels { 2 } else { 1 };
    let rank_sep = input.rank_sep / rank_scale as f64;

    // 1. Orientation.
    let oriented = orient_edges(n, &input.edges, rank_scale);

    // 2. Ranking.
    let ranks = rank_nodes(n, &oriented);

    // 3. Vertices + chains.
    let mut verts: Vec<Vertex> = input
        .nodes
        .iter()
        .zip(&ranks)
        .map(|(nd, &r)| {
            let (w, h) = swap(nd.width, nd.height);
            Vertex {
                kind: Kind::Real,
                rank: r,
                w,
                h,
                cluster: nd.cluster,
            }
        })
        .collect();
    let cluster_chain = cluster_chains(&input.clusters);
    let mut links: Vec<(usize, usize, f64)> = Vec::new();
    // Per edge: chain in oriented order, label vertex.
    let mut chains: Vec<Option<Chain>> = vec![None; input.edges.len()];
    for (ei, o) in oriented.iter().enumerate() {
        let Some(o) = o else { continue };
        let (ru, rv) = (ranks[o.from], ranks[o.to]);
        let span = rv - ru;
        let label = input.edges[ei].label.map(|(w, h)| swap(w, h));
        let label_rank = ru + span / 2;
        let dummy_cluster = lca(
            &cluster_chain,
            input.nodes[o.from].cluster,
            input.nodes[o.to].cluster,
        );
        let mut chain = vec![o.from];
        let mut label_vertex = None;
        for r in ru + 1..rv {
            let is_label = label.is_some() && r == label_rank;
            let (w, h) = if is_label {
                label.unwrap_or((0.0, 0.0))
            } else {
                (0.0, 0.0)
            };
            verts.push(Vertex {
                kind: Kind::Dummy { labeled: is_label },
                rank: r,
                w,
                h,
                cluster: dummy_cluster,
            });
            let id = verts.len() - 1;
            if is_label {
                label_vertex = Some(id);
            }
            chain.push(id);
        }
        chain.push(o.to);
        for pair in chain.windows(2) {
            let dummies = pair.iter().filter(|&&v| v >= n).count();
            links.push((pair[0], pair[1], [1.0, 2.0, 8.0][dummies]));
        }
        chains[ei] = Some((chain, label_vertex, o.reversed));
    }

    // 4. Cluster spans and border vertices.
    let nc = input.clusters.len();
    let mut span: Vec<Option<(usize, usize)>> = vec![None; nc];
    for (v, nd) in input.nodes.iter().enumerate() {
        if let Some(c) = nd.cluster {
            for &a in &cluster_chain[c] {
                let r = ranks[v];
                span[a] = Some(match span[a] {
                    Some((lo, hi)) => (lo.min(r), hi.max(r)),
                    None => (r, r),
                });
            }
        }
    }
    let mut borders: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nc]; // (left, right) per rank
    for c in 0..nc {
        if let Some((lo, hi)) = span[c] {
            for r in lo..=hi {
                verts.push(Vertex {
                    kind: Kind::Border {
                        cluster: c,
                        side: Side::Left,
                    },
                    rank: r,
                    w: 0.0,
                    h: 0.0,
                    cluster: Some(c),
                });
                verts.push(Vertex {
                    kind: Kind::Border {
                        cluster: c,
                        side: Side::Right,
                    },
                    rank: r,
                    w: 0.0,
                    h: 0.0,
                    cluster: Some(c),
                });
                borders[c].push((verts.len() - 2, verts.len() - 1));
            }
        }
    }

    let nv = verts.len();
    let mut up: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    let mut down: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    for &(a, b, w) in &links {
        down[a].push((b, w));
        up[b].push((a, w));
    }
    let max_rank = verts.iter().map(|v| v.rank).max().unwrap_or(0);

    // 5. Ordering.
    let mut layers = initial_order(&verts, n, &down, max_rank);
    let ctx = OrderCtx {
        verts: &verts,
        chains: &cluster_chain,
    };
    for layer in layers.iter_mut() {
        let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
        *layer = ctx.arrange(layer, &keys);
    }
    let mut best = layers.clone();
    let mut best_cross = total_crossings(&layers, &down, nv);
    let max_iter = if nv > 3000 { 6 } else { 24 };
    let mut stale = 0;
    for it in 0..max_iter {
        if best_cross <= 0.0 {
            break;
        }
        sweep(
            &mut layers,
            &ctx,
            if it % 2 == 0 { &up } else { &down },
            it % 2 == 0,
            nv,
        );
        let cross = total_crossings(&layers, &down, nv);
        if cross < best_cross {
            best_cross = cross;
            best = layers.clone();
            stale = 0;
        } else {
            stale += 1;
            if stale >= 4 {
                break;
            }
        }
    }
    let layers = best;

    // 6. Cross-axis coordinates.
    let pad = input.cluster_padding;
    let title_cross_low = horizontal;
    let pad_x_low = |c: usize| {
        pad + if title_cross_low {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    let gap = |a: usize, b: usize| -> f64 {
        let half = |v: &Vertex| match v.kind {
            Kind::Real | Kind::Dummy { labeled: true } => v.w / 2.0,
            _ => 0.0,
        };
        let spacing = |v: &Vertex| match v.kind {
            Kind::Dummy { labeled: false } => input.edge_sep,
            _ => input.node_sep,
        };
        let (va, vb) = (&verts[a], &verts[b]);
        let a_in = matches!(
            va.kind,
            Kind::Border {
                side: Side::Left,
                ..
            }
        );
        let b_in = matches!(
            vb.kind,
            Kind::Border {
                side: Side::Right,
                ..
            }
        );
        if a_in || b_in {
            let mut g = half(va) + half(vb);
            if let Kind::Border {
                cluster,
                side: Side::Left,
            } = va.kind
            {
                g += pad_x_low(cluster);
            }
            if let Kind::Border {
                side: Side::Right, ..
            } = vb.kind
            {
                g += pad;
            }
            g
        } else {
            half(va) + half(vb) + spacing(va) / 2.0 + spacing(vb) / 2.0
        }
    };
    let x = assign_x(&layers, &verts, &up, &down, &borders, &gap);

    // 7. Rank-axis coordinates.
    let pad_rank_low = |c: usize| {
        pad + if input.direction == Direction::TopBottom {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    let pad_rank_high = |c: usize| {
        pad + if input.direction == Direction::BottomTop {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    let mut rank_h = vec![0.0f64; max_rank + 1];
    for v in &verts {
        rank_h[v.rank] = rank_h[v.rank].max(v.h);
    }
    let mut start_pad = vec![0.0f64; max_rank + 1];
    let mut end_pad = vec![0.0f64; max_rank + 1];
    for (c, s) in span.iter().enumerate() {
        if let Some((lo, hi)) = *s {
            start_pad[lo] += pad_rank_low(c);
            end_pad[hi] += pad_rank_high(c);
        }
    }
    let mut rank_y = vec![0.0f64; max_rank + 1];
    for r in 0..=max_rank {
        rank_y[r] = if r == 0 {
            start_pad[0] + rank_h[0] / 2.0
        } else {
            rank_y[r - 1]
                + rank_h[r - 1] / 2.0
                + end_pad[r - 1]
                + rank_sep
                + start_pad[r]
                + rank_h[r] / 2.0
        };
    }

    // Cluster rectangles in layout space.
    let mut cluster_rects = vec![Rect::default(); nc];
    for c in 0..nc {
        let Some((lo, hi)) = span[c] else { continue };
        let x0 = borders[c]
            .iter()
            .map(|&(l, _)| x[l])
            .fold(f64::INFINITY, f64::min);
        let x1 = borders[c]
            .iter()
            .map(|&(_, r)| x[r])
            .fold(f64::NEG_INFINITY, f64::max);
        let same_start: f64 = (0..nc)
            .filter(|&d| cluster_chain[d].contains(&c) && span[d].map(|s| s.0) == Some(lo))
            .map(&pad_rank_low)
            .sum();
        let same_end: f64 = (0..nc)
            .filter(|&d| cluster_chain[d].contains(&c) && span[d].map(|s| s.1) == Some(hi))
            .map(&pad_rank_high)
            .sum();
        let y0 = rank_y[lo] - rank_h[lo] / 2.0 - same_start;
        let y1 = rank_y[hi] + rank_h[hi] / 2.0 + same_end;
        cluster_rects[c] = Rect::new(x0, y0, x1 - x0, y1 - y0);
    }

    // 8. Transform to output space.
    let extent_y = rank_y.last().copied().unwrap_or(0.0)
        + rank_h.last().copied().unwrap_or(0.0) / 2.0
        + end_pad.last().copied().unwrap_or(0.0);
    let to_out = |p: Point| -> Point {
        match input.direction {
            Direction::TopBottom => p,
            Direction::BottomTop => Point::new(p.x, extent_y - p.y),
            Direction::LeftRight => Point::new(p.y, p.x),
            Direction::RightLeft => Point::new(extent_y - p.y, p.x),
        }
    };
    let rect_out = |r: Rect| -> Rect {
        let a = to_out(Point::new(r.x, r.y));
        let b = to_out(Point::new(r.right(), r.bottom()));
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        )
    };
    let pos = |v: usize| to_out(Point::new(x[v], rank_y[verts[v].rank]));

    let nodes: Vec<Point> = (0..n).map(pos).collect();
    let edges = input
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| {
            if e.from == e.to {
                return self_loop(nodes[e.from], &input.nodes[e.from], e.label);
            }
            let Some((chain, label_v, reversed)) = &chains[ei] else {
                return EdgeRoute {
                    points: vec![nodes[e.from], nodes[e.to]],
                    label: None,
                };
            };
            let mut points: Vec<Point> = chain.iter().map(|&v| pos(v)).collect();
            if *reversed {
                points.reverse();
            }
            let label = label_v.map(pos).or_else(|| {
                e.label
                    .map(|_| points[0].lerp(points[points.len() - 1], 0.5))
            });
            EdgeRoute { points, label }
        })
        .collect();
    let clusters = cluster_rects
        .into_iter()
        .enumerate()
        .map(|(c, r)| if span[c].is_some() { rect_out(r) } else { r })
        .collect();
    LayoutOutput {
        nodes,
        edges,
        clusters,
    }
}

fn self_loop(c: Point, node: &LayoutNode, label: Option<(f64, f64)>) -> EdgeRoute {
    let (w, h) = (node.width, node.height);
    let ext = 22.0;
    let points = vec![
        Point::new(c.x + w / 4.0, c.y - h / 2.0),
        Point::new(c.x + w / 4.0, c.y - h / 2.0 - ext),
        Point::new(c.x + w / 2.0 + ext, c.y - h / 2.0 - ext),
        Point::new(c.x + w / 2.0 + ext, c.y),
        Point::new(c.x + w / 2.0, c.y),
    ];
    let label =
        label.map(|(lw, _)| Point::new(c.x + w / 2.0 + ext + lw / 2.0 + 4.0, c.y - h / 2.0 - ext));
    EdgeRoute { points, label }
}

/// DFS-based cycle breaking. Returns `None` for self-loops.
fn orient_edges(n: usize, edges: &[LayoutEdge], scale: usize) -> Vec<Option<Oriented>> {
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to {
            out_edges[e.from].push(i);
        }
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }
    let mut mark = vec![Mark::White; n];
    let mut reversed = vec![false; edges.len()];
    for root in 0..n {
        if mark[root] != Mark::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        mark[root] = Mark::Gray;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if *i < out_edges[v].len() {
                let ei = out_edges[v][*i];
                *i += 1;
                let w = edges[ei].to;
                match mark[w] {
                    Mark::Gray => reversed[ei] = true,
                    Mark::White => {
                        mark[w] = Mark::Gray;
                        stack.push((w, 0));
                    }
                    Mark::Black => {}
                }
            } else {
                mark[v] = Mark::Black;
                stack.pop();
            }
        }
    }
    edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            (e.from != e.to).then(|| {
                let (from, to) = if reversed[i] {
                    (e.to, e.from)
                } else {
                    (e.from, e.to)
                };
                Oriented {
                    from,
                    to,
                    minlen: e.minlen.max(1) * scale,
                    reversed: reversed[i],
                }
            })
        })
        .collect()
}

const NETWORK_SIMPLEX_MAX_NODES: usize = 4000;

/// Longest-path ranking refined by network simplex.
fn rank_nodes(n: usize, edges: &[Option<Oriented>]) -> Vec<usize> {
    let mut indeg = vec![0usize; n];
    let mut outs: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut ins: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for o in edges.iter().flatten() {
        indeg[o.to] += 1;
        outs[o.from].push((o.to, o.minlen));
        ins[o.to].push((o.from, o.minlen));
    }
    let mut order = Vec::with_capacity(n);
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    let mut rank = vec![0i64; n];
    while let Some(v) = queue.pop_front() {
        order.push(v);
        for &(w, len) in &outs[v] {
            rank[w] = rank[w].max(rank[v] + len as i64);
            indeg[w] -= 1;
            if indeg[w] == 0 {
                queue.push_back(w);
            }
        }
    }
    // Longest-path puts every node as high as possible, so a chain hanging off a
    // late source starts at the top and drags a very long edge. Network simplex
    // (as in dagre/graphviz) finds the ranking with minimum total edge length.
    // Cheap greedy pre-pass: slide nodes toward successors when that shortens
    // edges (reverse topological order moves chains together). This gets close
    // to optimal quickly and leaves network simplex few pivots to do.
    for _ in 0..4 {
        let mut changed = false;
        for &v in order.iter().rev() {
            if outs[v].is_empty() || outs[v].len() <= ins[v].len() {
                continue;
            }
            let limit = outs[v]
                .iter()
                .map(|&(w, len)| rank[w] - len as i64)
                .min()
                .unwrap_or(rank[v]);
            if limit > rank[v] {
                rank[v] = limit;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if n <= NETWORK_SIMPLEX_MAX_NODES {
        let rank_edges: Vec<rank::RankEdge> = edges
            .iter()
            .flatten()
            .map(|o| rank::RankEdge {
                tail: o.from,
                head: o.to,
                minlen: o.minlen as i64,
                weight: 1.0,
            })
            .collect();
        rank::network_simplex(n, &rank_edges, &mut rank, 10 * n + 100);
    }
    let min = rank.iter().copied().min().unwrap_or(0);
    rank.into_iter().map(|r| (r - min) as usize).collect()
}

/// Root-to-cluster ancestor chains (inclusive).
fn cluster_chains(clusters: &[LayoutCluster]) -> Vec<Vec<usize>> {
    (0..clusters.len())
        .map(|c| {
            let mut chain = vec![c];
            let mut cur = clusters[c].parent;
            while let Some(p) = cur {
                if chain.contains(&p) || chain.len() > clusters.len() {
                    break;
                }
                chain.push(p);
                cur = clusters[p].parent;
            }
            chain.reverse();
            chain
        })
        .collect()
}

fn lca(chains: &[Vec<usize>], a: Option<usize>, b: Option<usize>) -> Option<usize> {
    let (a, b) = (a?, b?);
    chains[a]
        .iter()
        .zip(&chains[b])
        .take_while(|(x, y)| x == y)
        .last()
        .map(|(x, _)| *x)
}

fn initial_order(
    verts: &[Vertex],
    n: usize,
    down: &[Vec<(usize, f64)>],
    max_rank: usize,
) -> Vec<Vec<usize>> {
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    let mut seen = vec![false; verts.len()];
    let mut roots: Vec<usize> = (0..n).collect();
    roots.sort_by_key(|&v| verts[v].rank);
    for root in roots {
        if seen[root] {
            continue;
        }
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if seen[v] {
                continue;
            }
            seen[v] = true;
            layers[verts[v].rank].push(v);
            for &(w, _) in down[v].iter().rev() {
                if !seen[w] {
                    stack.push(w);
                }
            }
        }
    }
    for (v, s) in seen.iter().enumerate() {
        if !s {
            layers[verts[v].rank].push(v);
        }
    }
    layers
}

struct OrderCtx<'a> {
    verts: &'a [Vertex],
    chains: &'a [Vec<usize>],
}

enum Unit {
    Single(usize),
    Group(usize, Vec<(usize, f64)>),
}

impl OrderCtx<'_> {
    fn chain(&self, v: usize) -> &[usize] {
        match self.verts[v].cluster {
            Some(c) => &self.chains[c],
            None => &[],
        }
    }

    /// Sort a layer by key while keeping every cluster contiguous and its border
    /// vertices at the block ends.
    fn arrange(&self, layer: &[usize], keys: &[f64]) -> Vec<usize> {
        let items: Vec<(usize, f64)> = layer.iter().copied().zip(keys.iter().copied()).collect();
        let mut out = Vec::with_capacity(layer.len());
        self.arrange_rec(items, 0, &mut out);
        out
    }

    fn arrange_rec(&self, items: Vec<(usize, f64)>, depth: usize, out: &mut Vec<usize>) {
        let mut units: Vec<(u8, f64, Unit)> = Vec::new();
        for (v, k) in items {
            let chain = self.chain(v);
            // A vertex whose cluster chain goes deeper than `depth` belongs to the
            // sub-cluster at that depth; border vertices sit one level inside their
            // own cluster, where they are pinned to the block ends below.
            if chain.len() > depth {
                let c = chain[depth];
                match units
                    .iter_mut()
                    .find(|(_, _, u)| matches!(u, Unit::Group(g, _) if *g == c))
                {
                    Some((_, _, Unit::Group(_, members))) => members.push((v, k)),
                    _ => units.push((1, 0.0, Unit::Group(c, vec![(v, k)]))),
                }
            } else {
                let prio = match self.verts[v].kind {
                    Kind::Border {
                        side: Side::Left, ..
                    } => 0,
                    Kind::Border {
                        side: Side::Right, ..
                    } => 2,
                    _ => 1,
                };
                units.push((prio, k, Unit::Single(v)));
            }
        }
        for u in units.iter_mut() {
            if let Unit::Group(_, members) = &u.2 {
                u.1 = members.iter().map(|m| m.1).sum::<f64>() / members.len() as f64;
            }
        }
        units.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        for (_, _, u) in units {
            match u {
                Unit::Single(v) => out.push(v),
                Unit::Group(_, members) => self.arrange_rec(members, depth + 1, out),
            }
        }
    }
}

fn sweep(
    layers: &mut [Vec<usize>],
    ctx: &OrderCtx,
    neigh: &[Vec<(usize, f64)>],
    downward: bool,
    nv: usize,
) {
    let mut pos = vec![0.0f64; nv];
    for layer in layers.iter() {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i as f64;
        }
    }
    let n_layers = layers.len();
    let order: Vec<usize> = if downward {
        (1..n_layers).collect()
    } else {
        (0..n_layers.saturating_sub(1)).rev().collect()
    };
    for li in order {
        let layer = &layers[li];
        let keys: Vec<f64> = layer
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (sum, wsum) = neigh[v]
                    .iter()
                    .fold((0.0, 0.0), |(s, ws), &(u, w)| (s + pos[u] * w, ws + w));
                if wsum > 0.0 { sum / wsum } else { i as f64 }
            })
            .collect();
        let new_layer = ctx.arrange(layer, &keys);
        for (i, &v) in new_layer.iter().enumerate() {
            pos[v] = i as f64;
        }
        layers[li] = new_layer;
    }
}

/// Weighted crossing count between consecutive layers (Barth–Jünger–Mutzel with a Fenwick tree).
fn total_crossings(layers: &[Vec<usize>], down: &[Vec<(usize, f64)>], nv: usize) -> f64 {
    let mut pos = vec![0usize; nv];
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
    let mut total = 0.0;
    for li in 0..layers.len().saturating_sub(1) {
        let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
        for &v in &layers[li] {
            for &(w, wt) in &down[v] {
                pairs.push((pos[v], pos[w], wt));
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let size = layers[li + 1].len() + 1;
        let mut tree = vec![0.0f64; size + 1];
        let mut inserted = 0.0;
        for &(_, p, wt) in &pairs {
            // weight of already-inserted edges ending strictly right of p
            let mut i = p + 1;
            let mut le = 0.0;
            while i > 0 {
                le += tree[i];
                i -= i & i.wrapping_neg();
            }
            total += (inserted - le) * wt;
            let mut i = p + 1;
            while i <= size {
                tree[i] += wt;
                i += i & i.wrapping_neg();
            }
            inserted += wt;
        }
    }
    total
}

/// Cross-axis coordinates via iterated per-layer weighted isotonic regression.
fn assign_x(
    layers: &[Vec<usize>],
    verts: &[Vertex],
    up: &[Vec<(usize, f64)>],
    down: &[Vec<(usize, f64)>],
    borders: &[Vec<(usize, usize)>],
    gap: &dyn Fn(usize, usize) -> f64,
) -> Vec<f64> {
    let nv = verts.len();
    let mut x = vec![0.0f64; nv];
    let offsets: Vec<Vec<f64>> = layers
        .iter()
        .map(|layer| {
            let mut o = Vec::with_capacity(layer.len());
            let mut acc = 0.0;
            for (i, &v) in layer.iter().enumerate() {
                if i > 0 {
                    acc += gap(layer[i - 1], v);
                }
                o.push(acc);
            }
            o
        })
        .collect();
    for (layer, o) in layers.iter().zip(&offsets) {
        let shift = o.last().copied().unwrap_or(0.0) / 2.0;
        for (&v, &ov) in layer.iter().zip(o) {
            x[v] = ov - shift;
        }
    }
    let mut border_of: Vec<Option<(usize, Side)>> = vec![None; nv];
    for v in 0..nv {
        if let Kind::Border { cluster, side } = verts[v].kind {
            border_of[v] = Some((cluster, side));
        }
    }

    #[derive(Clone, Copy)]
    enum Pass {
        Down,
        Up,
        Both,
    }
    let passes = [
        Pass::Down,
        Pass::Up,
        Pass::Down,
        Pass::Up,
        Pass::Down,
        Pass::Up,
        Pass::Both,
        Pass::Both,
        Pass::Both,
    ];
    for pass in passes {
        let order: Vec<usize> = match pass {
            Pass::Down | Pass::Both => (0..layers.len()).collect(),
            Pass::Up => (0..layers.len()).rev().collect(),
        };
        // Cluster border alignment targets, recomputed per pass.
        let targets: Vec<(f64, f64)> = borders
            .iter()
            .map(|bs| {
                let l = bs.iter().map(|&(l, _)| x[l]).fold(f64::INFINITY, f64::min);
                let r = bs
                    .iter()
                    .map(|&(_, r)| x[r])
                    .fold(f64::NEG_INFINITY, f64::max);
                (l, r)
            })
            .collect();
        for li in order {
            let layer = &layers[li];
            if layer.is_empty() {
                continue;
            }
            let mut desired = Vec::with_capacity(layer.len());
            let mut weights = Vec::with_capacity(layer.len());
            for &v in layer {
                if let Some((c, side)) = border_of[v] {
                    desired.push(if side == Side::Left {
                        targets[c].0
                    } else {
                        targets[c].1
                    });
                    weights.push(4.0);
                    continue;
                }
                let neigh: Box<dyn Iterator<Item = &(usize, f64)>> = match pass {
                    Pass::Down => Box::new(up[v].iter()),
                    Pass::Up => Box::new(down[v].iter()),
                    Pass::Both => Box::new(up[v].iter().chain(down[v].iter())),
                };
                let (s, ws) = neigh.fold((0.0, 0.0), |(s, ws), &(u, w)| (s + x[u] * w, ws + w));
                let base = match verts[v].kind {
                    Kind::Dummy { .. } => 2.0,
                    _ => 1.0,
                };
                if ws > 0.0 {
                    desired.push(s / ws);
                    weights.push(base);
                } else {
                    desired.push(x[v]);
                    weights.push(0.25);
                }
            }
            let solved = isotonic(&desired, &weights, &offsets[li]);
            for (&v, xv) in layer.iter().zip(solved) {
                x[v] = xv;
            }
        }
    }
    // Normalise so the leftmost extent sits at zero.
    let min = (0..nv)
        .map(|v| {
            x[v] - match verts[v].kind {
                Kind::Real | Kind::Dummy { labeled: true } => verts[v].w / 2.0,
                _ => 0.0,
            }
        })
        .fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        for xv in x.iter_mut() {
            *xv -= min;
        }
    }
    x
}

/// Minimise Σ wᵢ(xᵢ − dᵢ)² subject to xᵢ₊₁ − xᵢ ≥ oᵢ₊₁ − oᵢ (pool adjacent violators).
fn isotonic(desired: &[f64], weights: &[f64], offsets: &[f64]) -> Vec<f64> {
    // Substitute yᵢ = xᵢ − oᵢ, which turns the constraints into y non-decreasing.
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(desired.len()); // (Σw·t, Σw, len)
    for i in 0..desired.len() {
        let w = weights[i].max(1e-9);
        blocks.push((w * (desired[i] - offsets[i]), w, 1));
        while blocks.len() >= 2 {
            let (s1, w1, _) = blocks[blocks.len() - 2];
            let (s2, w2, _) = blocks[blocks.len() - 1];
            if s1 / w1 > s2 / w2 {
                let top = blocks.pop().unwrap_or((0.0, 0.0, 0));
                if let Some(prev) = blocks.last_mut() {
                    prev.0 += top.0;
                    prev.1 += top.1;
                    prev.2 += top.2;
                }
            } else {
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(desired.len());
    for (s, w, len) in blocks {
        for _ in 0..len {
            out.push(s / w + offsets[out.len()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(cluster: Option<usize>) -> LayoutNode {
        LayoutNode {
            width: 80.0,
            height: 40.0,
            cluster,
        }
    }

    fn edge(from: usize, to: usize) -> LayoutEdge {
        LayoutEdge {
            from,
            to,
            minlen: 1,
            label: None,
        }
    }

    fn input(
        nodes: Vec<LayoutNode>,
        edges: Vec<LayoutEdge>,
        clusters: Vec<LayoutCluster>,
        direction: Direction,
    ) -> LayoutInput {
        LayoutInput {
            nodes,
            edges,
            clusters,
            direction,
            node_sep: 50.0,
            rank_sep: 50.0,
            edge_sep: 20.0,
            cluster_padding: 8.0,
        }
    }

    fn rects(inp: &LayoutInput, out: &LayoutOutput) -> Vec<Rect> {
        inp.nodes
            .iter()
            .zip(&out.nodes)
            .map(|(n, p)| Rect::centered(*p, n.width, n.height))
            .collect()
    }

    #[test]
    fn isotonic_respects_gaps() {
        let x = isotonic(&[0.0, 0.0, 0.0], &[1.0, 1.0, 1.0], &[0.0, 10.0, 20.0]);
        assert!((x[1] - x[0] - 10.0).abs() < 1e-9 && (x[2] - x[1] - 10.0).abs() < 1e-9);
        assert!((x[1]).abs() < 1e-9, "centred on the common target");
    }

    #[test]
    fn chain_is_vertical_in_tb() {
        let inp = input(
            vec![node(None), node(None), node(None)],
            vec![edge(0, 1), edge(1, 2)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!(out.nodes[0].y < out.nodes[1].y && out.nodes[1].y < out.nodes[2].y);
        assert!((out.nodes[0].x - out.nodes[2].x).abs() < 1e-6);
    }

    #[test]
    fn lr_flows_horizontally_and_rl_reverses() {
        let lr = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::LeftRight,
        ));
        assert!(lr.nodes[0].x < lr.nodes[1].x);
        let rl = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::RightLeft,
        ));
        assert!(rl.nodes[0].x > rl.nodes[1].x);
        let bt = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::BottomTop,
        ));
        assert!(bt.nodes[0].y > bt.nodes[1].y);
    }

    #[test]
    fn cycles_are_handled() {
        let inp = input(
            vec![node(None), node(None), node(None)],
            vec![edge(0, 1), edge(1, 2), edge(2, 0), edge(1, 1)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert_eq!(out.edges.len(), 4);
        assert!(out.edges.iter().all(|e| e.points.len() >= 2));
    }

    #[test]
    fn no_node_overlaps_in_dense_graph() {
        let n = 30;
        let nodes = (0..n).map(|_| node(None)).collect();
        let mut edges = Vec::new();
        for i in 0..n {
            edges.push(edge(i, (i * 7 + 3) % n));
            edges.push(edge(i, (i * 11 + 5) % n));
        }
        let inp = input(nodes, edges, vec![], Direction::TopBottom);
        let out = layout(&inp);
        let rs = rects(&inp, &out);
        for i in 0..n {
            for j in i + 1..n {
                assert!(!rs[i].intersects(&rs[j]), "nodes {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn clusters_contain_members_and_exclude_others() {
        let nodes = vec![
            node(Some(0)),
            node(Some(0)),
            node(None),
            node(Some(1)),
            node(None),
        ];
        let edges = vec![edge(0, 1), edge(2, 0), edge(1, 3), edge(2, 4), edge(4, 3)];
        let clusters = vec![
            LayoutCluster {
                parent: None,
                title_height: 24.0,
            },
            LayoutCluster {
                parent: Some(0),
                title_height: 24.0,
            },
        ];
        let inp = input(nodes, edges, clusters, Direction::TopBottom);
        let out = layout(&inp);
        let rs = rects(&inp, &out);
        for (i, nd) in inp.nodes.iter().enumerate() {
            if nd.cluster.is_some() {
                let c = &out.clusters[0];
                assert!(
                    c.contains(Point::new(rs[i].x, rs[i].y))
                        && c.contains(Point::new(rs[i].right(), rs[i].bottom())),
                    "member {i} outside cluster"
                );
            } else {
                assert!(
                    !out.clusters[0].intersects(&rs[i]),
                    "non-member {i} overlaps cluster"
                );
            }
        }
    }

    #[test]
    fn labels_get_space() {
        let inp = input(
            vec![node(None), node(None)],
            vec![LayoutEdge {
                from: 0,
                to: 1,
                minlen: 1,
                label: Some((60.0, 24.0)),
            }],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let l = out.edges[0].label.expect("label position");
        assert!(l.y > out.nodes[0].y + 20.0 && l.y < out.nodes[1].y - 20.0);
    }
}
