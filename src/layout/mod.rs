//! Layered (Sugiyama-style) graph layout, written from scratch.
//!
//! Pipeline, all pure:
//! 1. remove self-loops, break cycles by reversing DFS back edges;
//! 2. rank with longest-path + source tightening (minlen-aware);
//! 3. split long edges into dummy vertices (edge labels become sized dummies, as in dagre);
//! 4. add left/right border vertices for every cluster on every rank it spans;
//! 5. order layers with weighted barycentre sweeps, keeping clusters contiguous,
//!    scoring with an O(E log V) crossing count and keeping the best ordering;
//! 6. assign cross-axis coordinates with Brandes–Köpf, exactly as dagre does
//!    (four median alignments, block compaction, balanced), see `bk.rs`;
//! 7. stack ranks along the rank axis and rotate for TB / BT / LR / RL.

use crate::geom::{Point, Rect};

mod bk;
pub mod boxes;
mod nested;
mod ortho;
pub use ortho::{EdgeRouting, OrthoSpacing};
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
    /// Source node; for an edge leaving a whole cluster, a member of `from_cluster`.
    pub from: usize,
    /// Target node; for an edge entering a whole cluster, a member of `to_cluster`.
    pub to: usize,
    pub minlen: usize,
    /// Label size in output space (width, height).
    pub label: Option<(f64, f64)>,
    /// The edge leaves this cluster as a whole (a subgraph / composite used as endpoint).
    pub from_cluster: Option<usize>,
    /// The edge enters this cluster as a whole.
    pub to_cluster: Option<usize>,
}

impl LayoutEdge {
    /// A node-to-node edge.
    pub fn new(from: usize, to: usize, minlen: usize, label: Option<(f64, f64)>) -> Self {
        LayoutEdge {
            from,
            to,
            minlen,
            label,
            from_cluster: None,
            to_cluster: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutCluster {
    pub parent: Option<usize>,
    /// Height of the title band reserved at the top of the cluster.
    pub title_height: f64,
    /// Narrowest the cluster may be (its painted title plus padding).
    pub min_width: f64,
    /// The cluster's own `direction`, honoured when it is laid out on its own (see `nested`).
    pub direction: Option<Direction>,
}

impl LayoutCluster {
    pub fn new(parent: Option<usize>, title_height: f64) -> Self {
        LayoutCluster {
            parent,
            title_height,
            min_width: 0.0,
            direction: None,
        }
    }
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
    /// Node / rank separation inside a cluster that is laid out on its own.
    pub inner_node_sep: f64,
    pub inner_rank_sep: f64,
    /// How edges are routed (dagre-style splines or ELK-style orthogonal routes).
    pub routing: EdgeRouting,
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
    /// Ranking weight (0 for the cluster-membership constraints of `rank_with_clusters`).
    weight: f64,
}

/// Run the full layout: clusters that no edge crosses are laid out on their own
/// (with their own direction and spacing, as mermaid's ELK layout does), the
/// rest in one layered pass with their parent (see `nested`).
pub fn layout(input: &LayoutInput) -> LayoutOutput {
    nested::layout(input)
}

/// One layered pass over the whole input (every cluster integrated).
fn layout_flat(input: &LayoutInput) -> LayoutOutput {
    let horizontal = input.direction.is_horizontal();
    let swap = |w: f64, h: f64| if horizontal { (h, w) } else { (w, h) };
    let n = input.nodes.len();

    // dagre's makeSpaceForEdgeLabels: every edge spans two ranks at half the
    // separation, labelled or not, so each edge gets a midpoint vertex.
    let rank_scale = 2;
    let rank_sep = input.rank_sep / rank_scale as f64;

    // 1. Orientation.
    let mut oriented = orient_edges(n, &input.edges, rank_scale);

    // 2. Ranking.
    let ranks = rank_with_clusters(n, &mut oriented, input);

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
    let x = assign_x(&layers, &verts, &links, &borders, &gap);
    let plan = match input.routing {
        EdgeRouting::Orthogonal(s) => Some(ortho_plan(&verts, n, &chains, &x, &layers, &gap, s)),
        EdgeRouting::Splines => None,
    };

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
    // Orthogonal routes: widen a gap whose slots do not fit (ELK sizes the space
    // between two layers from the slots routed through it).
    // ELK gives edge labels a layer of their own with the full layer gap on both
    // sides; the split around the label vertex leaves only half the gap each side.
    let mut label_gap = vec![0.0f64; max_rank + 1];
    if plan.is_some() {
        for v in &verts {
            if matches!(v.kind, Kind::Dummy { labeled: true }) {
                label_gap[v.rank] = rank_sep;
            }
        }
    }
    let gap_before = |r: usize| rank_sep + label_gap[r - 1] + label_gap[r];
    let mut extra = vec![0.0f64; max_rank + 1];
    if let Some(p) = &plan {
        for (ra, ch) in p.channels.iter().enumerate() {
            let Some((rb, slots)) = *ch else { continue };
            let free: f64 = (ra + 1..rb)
                .map(|r| rank_h[r] + start_pad[r] + end_pad[r])
                .sum();
            let have = (ra + 1..=rb).map(gap_before).sum::<f64>() + free;
            extra[rb] = extra[rb].max(p.spacing.required_gap(slots) - have);
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
                + gap_before(r)
                + extra[r]
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
    let mut loops_on = vec![0usize; n];
    let edges = input
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| {
            if e.from == e.to {
                if plan.is_some() {
                    // ELK: a loop on the node's low cross-axis side (west in TB,
                    // north in LR), its two ports a quarter of the node apart.
                    let v = e.from;
                    let k = loops_on[v];
                    loops_on[v] += 1;
                    let (cx, cy) = (x[v], rank_y[verts[v].rank]);
                    let (side, s) = (cx - verts[v].w / 2.0, verts[v].h / 4.0);
                    let out = side - SELF_LOOP_EXT - k as f64 * SELF_LOOP_EXT * 0.75;
                    let points = [
                        Point::new(side, cy - s / 2.0),
                        Point::new(out, cy - s / 2.0),
                        Point::new(out, cy + s / 2.0),
                        Point::new(side, cy + s / 2.0),
                    ]
                    .map(to_out)
                    .to_vec();
                    let label = e.label.map(|(lw, lh)| {
                        let lw = swap(lw, lh).0;
                        to_out(Point::new(out - lw / 2.0 - 2.0, cy))
                    });
                    return EdgeRoute { points, label };
                }
                return self_loop(nodes[e.from], &input.nodes[e.from], e.label);
            }
            let Some((chain, label_v, reversed)) = &chains[ei] else {
                return EdgeRoute {
                    points: vec![nodes[e.from], nodes[e.to]],
                    label: None,
                };
            };
            if let Some(p) = &plan {
                let mut points: Vec<Point> = p
                    .route(ei, &verts, &rank_y, &rank_h, &start_pad, &end_pad)
                    .into_iter()
                    .map(to_out)
                    .collect();
                if *reversed {
                    points.reverse();
                }
                let label = label_v
                    .map(|v| to_out(Point::new(p.xs[v], rank_y[verts[v].rank])))
                    .or_else(|| {
                        e.label
                            .map(|_| points[0].lerp(points[points.len() - 1], 0.5))
                    });
                return EdgeRoute { points, label };
            }
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

/// How far an orthogonal self-loop stands off its node.
const SELF_LOOP_EXT: f64 = 20.0;

/// The rank-axis-independent part of the orthogonal routes of one layered pass
/// (see [`ortho`]).
struct OrthoPlan {
    /// Per edge (oriented along the flow): the points it must pass, as (vertex,
    /// cross-axis position): its source port, its dummies in obstacle ranks, its
    /// target port.
    stops: Vec<Vec<(usize, f64)>>,
    /// Per edge, per pair of consecutive stops: the slot level of the jog between
    /// them, if their positions differ.
    jogs: Vec<Vec<Option<usize>>>,
    /// Per rank: the channel below it, as (next obstacle rank, slots used).
    channels: Vec<Option<(usize, usize)>>,
    spacing: OrthoSpacing,
    /// Cross-axis positions, with dummies moved in line with their edges' ports.
    xs: Vec<f64>,
}

fn ortho_plan(
    verts: &[Vertex],
    n: usize,
    chains: &[Option<Chain>],
    x: &[f64],
    layers: &[Vec<usize>],
    gap: &dyn Fn(usize, usize) -> f64,
    spacing: OrthoSpacing,
) -> OrthoPlan {
    let max_rank = verts.iter().map(|v| v.rank).max().unwrap_or(0);
    // Ranks holding nodes or edge labels; ranks of plain long-edge dummies are free.
    let mut obstacle = vec![false; max_rank + 1];
    for v in verts {
        if matches!(v.kind, Kind::Real | Kind::Dummy { labeled: true }) {
            obstacle[v.rank] = true;
        }
    }
    // Ports: the edge ends on each node's two flow sides, in the order of the
    // vertices they lead to, spread evenly over the side.
    let ne = chains.len();
    let mut out_side: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); n];
    let mut in_side: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); n];
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, _, _)) = ch else { continue };
        let (u, w) = (chain[0], chain[chain.len() - 1]);
        out_side[u].push((x[chain[1]], x[w], ei));
        in_side[w].push((x[chain[chain.len() - 2]], x[u], ei));
    }
    let mut start_x = vec![0.0; ne];
    let mut end_x = vec![0.0; ne];
    for v in 0..n {
        for (side, port_x) in [
            (&mut out_side[v], &mut start_x),
            (&mut in_side[v], &mut end_x),
        ] {
            side.sort_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(a.1.total_cmp(&b.1))
                    .then(a.2.cmp(&b.2))
            });
            for (&(_, _, ei), off) in side.iter().zip(ortho::port_offsets(side.len(), verts[v].w)) {
                port_x[ei] = x[v] + off;
            }
        }
    }
    // ELK places nodes with their ports in view (`nodeFlexibility: PORT_POSITION`),
    // so an edge leaving a port runs straight on where it can. Here: top-down, each
    // dummy in an obstacle rank moves as far toward the position the edge arrives
    // at (its source port, or its previous such dummy) as its neighbours allow.
    let mut xs = x.to_vec();
    let mut arrive: Vec<Option<(usize, usize)>> = vec![None; verts.len()]; // (edge, prev)
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, _, _)) = ch else { continue };
        let mut prev = chain[0];
        for &v in &chain[1..chain.len() - 1] {
            if obstacle[verts[v].rank] {
                arrive[v] = Some((ei, prev));
                prev = v;
            }
        }
    }
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            let Some((ei, prev)) = arrive[v] else {
                continue;
            };
            let want = if prev < n { start_x[ei] } else { xs[prev] };
            let lo = i
                .checked_sub(1)
                .map_or(f64::NEG_INFINITY, |j| xs[layer[j]] + gap(layer[j], v));
            let hi = layer
                .get(i + 1)
                .map_or(f64::INFINITY, |&r| xs[r] - gap(v, r));
            if lo <= hi {
                xs[v] = want.clamp(lo, hi);
            }
        }
    }
    let mut stops: Vec<Vec<(usize, f64)>> = vec![Vec::new(); ne];
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, _, _)) = ch else { continue };
        let last = chain.len() - 1;
        stops[ei] = chain
            .iter()
            .enumerate()
            .filter_map(|(k, &v)| match k {
                0 => Some((v, start_x[ei])),
                _ if k == last => Some((v, end_x[ei])),
                _ => obstacle[verts[v].rank].then_some((v, xs[v])),
            })
            .collect();
    }
    // Jogs, channel by channel (a channel is named by the obstacle rank above it).
    let mut by_channel: Vec<Vec<(usize, usize, ortho::Jog)>> = vec![Vec::new(); max_rank + 1];
    let mut channels: Vec<Option<(usize, usize)>> = vec![None; max_rank + 1];
    let mut jogs: Vec<Vec<Option<usize>>> = stops
        .iter()
        .map(|s| vec![None; s.len().saturating_sub(1)])
        .collect();
    for (ei, s) in stops.iter().enumerate() {
        for (k, w) in s.windows(2).enumerate() {
            let ((a, ax), (b, bx)) = (w[0], w[1]);
            if (ax - bx).abs() > 0.5 {
                let ra = verts[a].rank;
                by_channel[ra].push((ei, k, ortho::Jog { from: ax, to: bx }));
                channels[ra] = Some((verts[b].rank, 0));
            }
        }
    }
    for (ra, list) in by_channel.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let segs: Vec<ortho::Jog> = list.iter().map(|&(_, _, j)| j).collect();
        let (levels, count) = ortho::assign_slots(&segs, spacing.edge_edge / 2.0);
        for (&(ei, k, _), l) in list.iter().zip(levels) {
            jogs[ei][k] = Some(l);
        }
        if let Some((rb, _)) = channels[ra] {
            channels[ra] = Some((rb, count));
        }
    }
    OrthoPlan {
        stops,
        jogs,
        channels,
        spacing,
        xs,
    }
}

impl OrthoPlan {
    /// Edge `ei`'s route in layout space, oriented along the flow: from its source
    /// port on the node's side, through its stops, to its target port.
    fn route(
        &self,
        ei: usize,
        verts: &[Vertex],
        rank_y: &[f64],
        rank_h: &[f64],
        start_pad: &[f64],
        end_pad: &[f64],
    ) -> Vec<Point> {
        let stops = &self.stops[ei];
        let (Some(&(u, ux)), Some(&(w, wx))) = (stops.first(), stops.last()) else {
            return Vec::new();
        };
        let mut pts = vec![Point::new(ux, rank_y[verts[u].rank] + verts[u].h / 2.0)];
        for (k, pair) in stops.windows(2).enumerate() {
            let ((a, ax), (b, bx)) = (pair[0], pair[1]);
            if let Some(level) = self.jogs[ei][k] {
                let ra = verts[a].rank;
                let (rb, slots) = self.channels[ra].unwrap_or((verts[b].rank, 1));
                let top = rank_y[ra] + rank_h[ra] / 2.0 + end_pad[ra];
                let bottom = rank_y[rb] - rank_h[rb] / 2.0 - start_pad[rb];
                let ee = self.spacing.edge_edge;
                let y = (top + bottom) / 2.0 - (slots - 1) as f64 * ee / 2.0 + level as f64 * ee;
                pts.push(Point::new(ax, y));
                pts.push(Point::new(bx, y));
            }
            if b != w {
                pts.push(Point::new(bx, rank_y[verts[b].rank]));
            }
        }
        pts.push(Point::new(wx, rank_y[verts[w].rank] - verts[w].h / 2.0));
        ortho::simplify(&pts)
    }
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
                    weight: 1.0,
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
                weight: o.weight,
            })
            .collect();
        rank::network_simplex(n, &rank_edges, &mut rank, 10 * n + 100);
    }
    let min = rank.iter().copied().min().unwrap_or(0);
    rank.into_iter().map(|r| (r - min) as usize).collect()
}

/// Rank like `rank_nodes`, but an edge leaving (entering) a whole cluster is
/// constrained to start below its last (end above its first) member, the way
/// dagre's nesting graph ties cluster borders to their contents: every member
/// hangs between a virtual top and bottom vertex of the cluster and the edge
/// runs from the bottom / to the top vertex. An edge whose constraint would be
/// contradictory (e.g. from a cluster into its own member) is ranked by its plain
/// member ends instead; the other edges keep theirs.
///
/// When the constraints hold, an edge leaving a cluster is re-attached to the member
/// on the cluster's last rank (one entering it to a member on its first rank), so the
/// route leaves from the cluster's far side instead of from wherever the caller's
/// representative member happens to sit.
fn rank_with_clusters(
    n: usize,
    oriented: &mut [Option<Oriented>],
    input: &LayoutInput,
) -> Vec<usize> {
    let mut slot: Vec<Option<usize>> = vec![None; input.clusters.len()];
    let mut used = Vec::new();
    let mut ends = Vec::with_capacity(oriented.len());
    for (ei, o) in oriented.iter().enumerate() {
        let e = &input.edges[ei];
        let (src, dst) = match o {
            Some(o) if o.reversed => (e.to_cluster, e.from_cluster),
            Some(_) => (e.from_cluster, e.to_cluster),
            None => (None, None),
        };
        for c in [src, dst].into_iter().flatten() {
            if c < slot.len() && slot[c].is_none() {
                slot[c] = Some(used.len());
                used.push(c);
            }
        }
        ends.push((src, dst));
    }
    if used.is_empty() {
        return rank_nodes(n, oriented);
    }
    let chains = cluster_chains(&input.clusters);
    let top = |c: usize| slot[c].map(|k| n + 2 * k);
    let bottom = |c: usize| slot[c].map(|k| n + 2 * k + 1);
    let total = n + 2 * used.len();
    let mut edges: Vec<Option<Oriented>> = Vec::new();
    let constraint = |from: usize, to: usize| {
        Some(Oriented {
            from,
            to,
            minlen: 0,
            reversed: false,
            weight: 0.0,
        })
    };
    for (v, nd) in input.nodes.iter().enumerate() {
        let Some(c) = nd.cluster else { continue };
        for &a in &chains[c] {
            if let (Some(t), Some(b)) = (top(a), bottom(a)) {
                edges.push(constraint(t, v));
                edges.push(constraint(v, b));
            }
        }
    }
    // The constraint graph must stay acyclic. Membership constraints and plain node
    // edges are acyclic on their own; each cluster-bound edge is then added only if it
    // keeps the graph acyclic (else it falls back to its plain member ends, or, if
    // even that closes a cycle, does not constrain the ranking), so one contradictory
    // edge does not cost every other edge its cluster constraint.
    let mut outs: Vec<Vec<usize>> = vec![Vec::new(); total];
    for o in edges.iter().flatten() {
        outs[o.from].push(o.to);
    }
    let reaches = |outs: &[Vec<usize>], from: usize, to: usize| -> bool {
        let mut seen = vec![false; outs.len()];
        let mut stack = vec![from];
        while let Some(v) = stack.pop() {
            if v == to {
                return true;
            }
            if std::mem::replace(&mut seen[v], true) {
                continue;
            }
            stack.extend(outs[v].iter().copied());
        }
        false
    };
    let mut push = |edges: &mut Vec<Option<Oriented>>, from: usize, to: usize, o: &Oriented| {
        if from != to && reaches(&outs, to, from) {
            return false;
        }
        outs[from].push(to);
        edges.push(Some(Oriented {
            from,
            to,
            minlen: o.minlen,
            reversed: false,
            weight: o.weight,
        }));
        true
    };
    for (o, (src, dst)) in oriented.iter().zip(&ends) {
        if let Some(o) = o.as_ref().filter(|_| src.is_none() && dst.is_none()) {
            push(&mut edges, o.from, o.to, o);
        }
    }
    for (o, end) in oriented.iter().zip(ends.iter_mut()) {
        let Some(o) = o else { continue };
        let (src, dst) = *end;
        if src.is_none() && dst.is_none() {
            continue;
        }
        let from = src.and_then(bottom).unwrap_or(o.from);
        let to = dst.and_then(top).unwrap_or(o.to);
        if !push(&mut edges, from, to, o) {
            *end = (None, None);
            push(&mut edges, o.from, o.to, o);
        }
    }
    let all = rank_nodes(total, &edges);
    let min = all[..n].iter().copied().min().unwrap_or(0);
    let ranks: Vec<usize> = all[..n].iter().map(|r| r - min).collect();
    let in_cluster = |v: usize, c: usize| {
        input.nodes[v]
            .cluster
            .is_some_and(|m| chains[m].contains(&c))
    };
    for (o, (src, dst)) in oriented.iter_mut().zip(&ends) {
        let Some(o) = o else { continue };
        if let Some(c) = *src {
            if let Some(v) = (0..n)
                .filter(|&v| in_cluster(v, c))
                .max_by_key(|&v| ranks[v])
            {
                o.from = v;
            }
        }
        if let Some(c) = *dst {
            if let Some(v) = (0..n)
                .filter(|&v| in_cluster(v, c))
                .min_by_key(|&v| ranks[v])
            {
                o.to = v;
            }
        }
    }
    ranks
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

/// Cross-axis coordinates (Brandes–Köpf, as dagre does it; see `bk.rs`).
fn assign_x(
    layers: &[Vec<usize>],
    verts: &[Vertex],
    links: &[(usize, usize, f64)],
    borders: &[Vec<(usize, usize)>],
    gap: &dyn Fn(usize, usize) -> f64,
) -> Vec<f64> {
    let nv = verts.len();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut link = |a: usize, b: usize| {
        if !succs[a].contains(&b) {
            succs[a].push(b);
            preds[b].push(a);
        }
    };
    for &(a, b, _) in links {
        link(a, b);
    }
    // dagre chains each cluster's border vertices rank to rank.
    for bs in borders {
        for pair in bs.windows(2) {
            link(pair[0].0, pair[1].0);
            link(pair[0].1, pair[1].1);
        }
    }
    let dummy: Vec<bool> = verts.iter().map(|v| v.kind != Kind::Real).collect();
    let border: Vec<Option<bk::BorderSide>> = verts
        .iter()
        .map(|v| match v.kind {
            Kind::Border {
                side: Side::Left, ..
            } => Some(bk::BorderSide::Left),
            Kind::Border {
                side: Side::Right, ..
            } => Some(bk::BorderSide::Right),
            _ => None,
        })
        .collect();
    let width: Vec<f64> = verts
        .iter()
        .map(|v| match v.kind {
            Kind::Real | Kind::Dummy { labeled: true } => v.w,
            _ => 0.0,
        })
        .collect();
    let graph = bk::LayerGraph {
        layers,
        preds: &preds,
        succs: &succs,
        dummy: &dummy,
        border: &border,
        width: &width,
    };
    let mut x = bk::position_x(&graph, gap);
    // Normalise so the leftmost extent sits at zero.
    let min = (0..nv)
        .map(|v| x[v] - width[v] / 2.0)
        .fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        for xv in x.iter_mut() {
            *xv -= min;
        }
    }
    x
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
        LayoutEdge::new(from, to, 1, None)
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
            inner_node_sep: 50.0,
            inner_rank_sep: 24.0,
            routing: EdgeRouting::Splines,
        }
    }

    fn rects(inp: &LayoutInput, out: &LayoutOutput) -> Vec<Rect> {
        inp.nodes
            .iter()
            .zip(&out.nodes)
            .map(|(n, p)| Rect::centered(*p, n.width, n.height))
            .collect()
    }

    /// As in dagre (and so mermaid): every edge, labelled or not, gets a
    /// midpoint vertex, and Brandes–Köpf lines that midpoint up with one end
    /// rather than halfway, so a basis spline through the points bends
    /// (the "S" curve) instead of drawing a straight diagonal.
    #[test]
    fn fan_out_edges_bend_like_dagre() {
        let inp = input(
            vec![node(None), node(None), node(None), node(None)],
            vec![edge(0, 1), edge(0, 2), edge(0, 3)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        for (ei, route) in out.edges.iter().enumerate() {
            assert_eq!(route.points.len(), 3, "edge {ei} has a midpoint");
            let [a, m, b] = [route.points[0], route.points[1], route.points[2]];
            assert!(
                (m.x - a.x).abs() < 1e-6 || (m.x - b.x).abs() < 1e-6,
                "edge {ei}: midpoint {m:?} is aligned with an end ({a:?}, {b:?})"
            );
            assert!(a.y < m.y && m.y < b.y);
        }
        // The outer edges are not straight lines.
        let r = &out.edges[0].points;
        assert!((r[0].x - r[2].x).abs() > 1.0);
    }

    /// Orthogonal routes: right angles only, port to port on the facing sides, never
    /// through another node, and stacked slots `edge_edge` apart.
    #[test]
    fn orthogonal_routes_avoid_nodes() {
        for dir in [
            Direction::TopBottom,
            Direction::LeftRight,
            Direction::BottomTop,
            Direction::RightLeft,
        ] {
            let mut inp = input(
                (0..6).map(|_| node(None)).collect(),
                vec![
                    edge(0, 1),
                    edge(0, 2),
                    edge(0, 3),
                    edge(1, 4),
                    edge(2, 4),
                    edge(3, 4),
                    edge(0, 4),
                    edge(4, 5),
                    edge(5, 0),
                    LayoutEdge::new(1, 5, 1, Some((40.0, 20.0))),
                    edge(2, 2),
                ],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            let boxes = rects(&inp, &out);
            for (ei, (e, r)) in inp.edges.iter().zip(&out.edges).enumerate() {
                assert!(ortho::is_orthogonal(&r.points), "{dir:?} edge {ei}: {r:?}");
                // Ends on the bounding boxes of their nodes.
                let on = |p: Point, b: &Rect| {
                    ((p.x - b.x).abs() < 1e-6 || (p.x - b.right()).abs() < 1e-6)
                        && p.y >= b.y - 1e-6
                        && p.y <= b.bottom() + 1e-6
                        || ((p.y - b.y).abs() < 1e-6 || (p.y - b.bottom()).abs() < 1e-6)
                            && p.x >= b.x - 1e-6
                            && p.x <= b.right() + 1e-6
                };
                assert!(on(r.points[0], &boxes[e.from]), "{dir:?} edge {ei} start");
                assert!(
                    on(r.points[r.points.len() - 1], &boxes[e.to]),
                    "{dir:?} edge {ei} end"
                );
                // No segment enters any node's interior.
                for w in r.points.windows(2) {
                    for (v, b) in boxes.iter().enumerate() {
                        let inner = b.pad(-1.0);
                        let (lo, hi) = (
                            Point::new(w[0].x.min(w[1].x), w[0].y.min(w[1].y)),
                            Point::new(w[0].x.max(w[1].x), w[0].y.max(w[1].y)),
                        );
                        let hits = lo.x < inner.right()
                            && hi.x > inner.x
                            && lo.y < inner.bottom()
                            && hi.y > inner.y;
                        assert!(!hits, "{dir:?} edge {ei} runs through node {v}: {w:?}");
                    }
                }
            }
            // The labelled edge passes through its label.
            let r = &out.edges[9];
            let l = r.label.unwrap();
            assert!(r.points.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                (a.x - l.x).abs() < 1e-6 && (a.y - l.y) * (b.y - l.y) <= 0.0
                    || (a.y - l.y).abs() < 1e-6 && (a.x - l.x) * (b.x - l.x) <= 0.0
            }));
        }
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
            LayoutCluster::new(None, 24.0),
            LayoutCluster::new(Some(0), 24.0),
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
    fn one_contradictory_cluster_edge_keeps_the_other_constraints() {
        // Cluster 0 = {0 → 1}. `C → 3` is given with the first member as its
        // representative, so only the cluster constraint puts 3 below 1. The edge
        // `C → 0` (into its own member) is contradictory; it must not cost `C → 3`
        // its constraint.
        let cl = |from: usize, to: usize, fc: Option<usize>, tc: Option<usize>| LayoutEdge {
            from_cluster: fc,
            to_cluster: tc,
            ..LayoutEdge::new(from, to, 1, None)
        };
        let nodes = vec![node(Some(0)), node(Some(0)), node(None), node(None)];
        let edges = vec![
            edge(0, 1),
            cl(2, 1, None, Some(0)),
            cl(0, 3, Some(0), None),
            cl(1, 0, Some(0), None),
        ];
        let inp = input(
            nodes,
            edges,
            vec![LayoutCluster::new(None, 24.0)],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let y = |i: usize| out.nodes[i].y;
        assert!(y(2) < y(0) && y(0) < y(1), "{:?}", out.nodes);
        assert!(
            y(3) > y(1),
            "C → 3 still ranks 3 after the whole cluster: {:?}",
            out.nodes
        );
    }

    #[test]
    fn labels_get_space() {
        let inp = input(
            vec![node(None), node(None)],
            vec![LayoutEdge::new(0, 1, 1, Some((60.0, 24.0)))],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let l = out.edges[0].label.expect("label position");
        assert!(l.y > out.nodes[0].y + 20.0 && l.y < out.nodes[1].y - 20.0);
    }
}
