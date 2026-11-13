//! mermaid's dagre layout (`rendering-util/layout-algorithms/dagre`): the graph
//! mermaid builds for dagre (`prepareLayoutForDagre`: self-loops through two
//! tiny dummy nodes), its cluster handling (`mermaid-graphlib.js`: edges to a
//! cluster re-anchored on a member, clusters no edge crosses taken out and laid
//! out on their own in the other direction) and the recursive render that sizes
//! and places those, over a faithful port of dagre itself ([`core`]).

mod core;
mod graph;

use self::core::{InEdge, InNode, Opts, Pt, RankDir};
use self::graph::Graph;
use super::{Direction, EdgeRoute, LayoutInput, LayoutOutput};
use crate::geom::{Point, Rect};
use std::collections::HashMap;

/// What mermaid's dagre layout needs beyond [`LayoutInput`]: the ids it keys its
/// graph by (JS objects enumerate integer-like keys first, so ids change orders)
/// and the order flowDb hands it the nodes in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DagreModel {
    /// Per layout node: its id.
    pub node_names: Vec<String>,
    /// Per cluster: its id.
    pub cluster_names: Vec<String>,
    /// flowDb's `getData` order: subgraphs (last closed first), then vertices.
    /// A cluster left out is not a dagre node (an empty subgraph is a node).
    pub order: Vec<DagreItem>,
    /// Per cluster: its `direction` was written `TD`. dagre lays that out as
    /// `TB`, but a subgraph inside takes `TB` (not `LR`) from it.
    pub cluster_td: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DagreItem {
    Node(usize),
    Cluster(usize),
}

/// The size of the dummy nodes a self-loop runs through: a `labelRect` with an
/// empty label measures as its 0.1 × 0.1 rect.
const SELF_LOOP_DUMMY: f64 = 0.1;
/// flowDb's subgraph `padding`, by which `insertCluster` pads the title.
const CLUSTER_PAD: f64 = 8.0;
/// dagre's `marginx` / `marginy` as mermaid sets them.
const MARGIN: f64 = 8.0;

#[derive(Debug, Clone, Default)]
enum MNode {
    /// A node only referenced so far (graphlib's `undefined` label).
    #[default]
    Undefined,
    Leaf {
        idx: usize,
    },
    /// A self-loop's dummy node (`labelRect`).
    Special,
    /// A subgraph's own data (`isGroup`).
    Cluster {
        c: usize,
    },
    /// A subgraph taken out into a graph of its own.
    ClusterNode {
        c: usize,
        graph: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Whole,
    /// One of a self-loop's three segments.
    Loop(u8),
}

#[derive(Debug, Clone, Copy)]
struct MEdge {
    idx: usize,
    part: Part,
    minlen: f64,
    w: f64,
    h: f64,
}

impl Default for MEdge {
    fn default() -> Self {
        MEdge {
            idx: usize::MAX,
            part: Part::Whole,
            minlen: 1.0,
            w: 0.0,
            h: 0.0,
        }
    }
}

type MG = Graph<MNode, MEdge>;

struct Sub {
    g: MG,
    dir: Direction,
    /// `rankdir === 'TB'` (a subgraph's `TD` is not).
    tb: bool,
    nodesep: f64,
    ranksep: f64,
}

#[derive(Debug, Clone, Default)]
struct DbEntry {
    anchor: Option<String>,
    external: bool,
}

/// `clusterDb` (a JS `Map`: insertion order) and `descendants`.
#[derive(Default)]
struct Db {
    keys: Vec<String>,
    entries: HashMap<String, DbEntry>,
    descendants: HashMap<String, Vec<String>>,
}

impl Db {
    fn set(&mut self, k: &str, e: DbEntry) {
        if !self.entries.contains_key(k) {
            self.keys.push(k.to_string());
        }
        self.entries.insert(k.to_string(), e);
    }
    fn is_descendant(&self, id: &str, ancestor: &str) -> bool {
        self.descendants
            .get(ancestor)
            .is_some_and(|d| d.iter().any(|x| x == id))
    }
}

struct Ctx<'a> {
    input: &'a LayoutInput,
    model: &'a DagreModel,
    subs: Vec<Sub>,
    db: Db,
}

/// Lays `input` out as mermaid's dagre renderer does.
pub fn layout(input: &LayoutInput, model: &DagreModel) -> LayoutOutput {
    layout_full(input, model).0
}

/// What the renderer needs beyond [`LayoutOutput`].
#[derive(Debug, Clone, Default)]
pub struct Extras {
    /// The boxes of what mermaid draws invisibly (the dummy nodes of
    /// self-loops), which count towards the drawing's bounding box.
    pub hidden: Vec<Rect>,
    /// Per edge: the points dagre hands the renderer (`edge.points`: ends on
    /// the node boxes, or a self-loop's merged corners), against which
    /// `positionEdgeLabel` measures how far a cut path moved the label.
    pub raw_points: Vec<Vec<Point>>,
}

/// [`layout`], with its [`Extras`].
pub fn layout_full(input: &LayoutInput, model: &DagreModel) -> (LayoutOutput, Extras) {
    let mut g: MG = Graph::new(true, true, true);
    let parent_name = |c: Option<usize>| c.map(|c| model.cluster_names[c].clone());
    for item in &model.order {
        match *item {
            DagreItem::Node(i) => {
                g.set_node(&model.node_names[i], MNode::Leaf { idx: i });
                if let Some(p) = parent_name(input.nodes[i].cluster) {
                    g.set_parent(&model.node_names[i], Some(&p));
                }
            }
            DagreItem::Cluster(c) => {
                g.set_node(&model.cluster_names[c], MNode::Cluster { c });
                if let Some(p) = parent_name(input.clusters[c].parent) {
                    g.set_parent(&model.cluster_names[c], Some(&p));
                }
            }
        }
    }
    for (i, e) in input.edges.iter().enumerate() {
        let start = match e.from_cluster {
            Some(c) => model.cluster_names[c].clone(),
            None => model.node_names[e.from].clone(),
        };
        let end = match e.to_cluster {
            Some(c) => model.cluster_names[c].clone(),
            None => model.node_names[e.to].clone(),
        };
        let (w, h) = e.label.unwrap_or((0.0, 0.0));
        let minlen = e.minlen as f64;
        if start == end {
            let parent = g.parent(&start);
            let s1 = format!("{start}---{start}---1");
            let s2 = format!("{start}---{start}---2");
            for s in [&s1, &s2] {
                g.set_node(s, MNode::Special);
                g.set_parent(s, parent.as_deref());
            }
            let seg = |k: u8, (w, h): (f64, f64)| MEdge {
                idx: i,
                part: Part::Loop(k),
                minlen,
                w,
                h,
            };
            g.set_edge(
                &start,
                &s1,
                seg(0, (0.0, 0.0)),
                Some(&format!("{start}-cyclic-special-0")),
            );
            g.set_edge(
                &s1,
                &s2,
                seg(1, (w, h)),
                Some(&format!("{start}-cyclic-special-1")),
            );
            g.set_edge(
                &s2,
                &start,
                seg(2, (0.0, 0.0)),
                Some(&format!("{start}-cyclic-special-2")),
            );
        } else {
            g.set_edge(
                &start,
                &end,
                MEdge {
                    idx: i,
                    part: Part::Whole,
                    minlen,
                    w,
                    h,
                },
                Some(&format!("e{i}")),
            );
        }
    }
    let mut ctx = Ctx {
        input,
        model,
        subs: vec![Sub {
            g,
            dir: input.direction,
            tb: input.direction == Direction::TopBottom,
            nodesep: input.node_sep,
            ranksep: input.rank_sep,
        }],
        db: Db::default(),
    };
    adjust_clusters_and_edges(&mut ctx);
    let r = render(&mut ctx, 0, None);

    let mut out = LayoutOutput {
        nodes: vec![Point::default(); input.nodes.len()],
        edges: vec![
            EdgeRoute {
                points: Vec::new(),
                label: None,
            };
            input.edges.len()
        ],
        clusters: vec![Rect::default(); input.clusters.len()],
        cluster_directions: vec![input.direction; input.clusters.len()],
    };
    for (i, p) in r.nodes {
        out.nodes[i] = p;
    }
    for (c, rect, dir) in r.clusters {
        out.clusters[c] = rect;
        out.cluster_directions[c] = dir;
    }
    for (i, route) in r.edges {
        out.edges[i] = route;
    }
    let mut raw_points = vec![Vec::new(); input.edges.len()];
    for (i, pts) in r.raw {
        raw_points[i] = pts;
    }
    (
        out,
        Extras {
            hidden: r.hidden,
            raw_points,
        },
    )
}

// ---------------------------------------------------------------- mermaid-graphlib

fn extract_descendants(g: &MG, id: &str) -> Vec<String> {
    let mut res = g.children(Some(id));
    for c in g.children(Some(id)) {
        res.extend(extract_descendants(g, &c));
    }
    res
}

fn find_common_edges(g: &MG, id1: Option<&str>, id2: &str) -> usize {
    let edges = g.edges();
    let Some(id1) = id1 else { return 0 };
    let e1: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.v == id1 || e.w == id1)
        .map(|e| {
            let v = if e.v == id1 {
                id2.to_string()
            } else {
                e.v.clone()
            };
            let w = if e.w == id1 {
                id1.to_string()
            } else {
                e.w.clone()
            };
            (v, w)
        })
        .collect();
    let e2: Vec<(&str, &str)> = edges
        .iter()
        .filter(|e| e.v == id2 || e.w == id2)
        .map(|e| (e.v.as_str(), e.w.as_str()))
        .collect();
    e1.iter()
        .filter(|(v, w)| e2.iter().any(|(a, b)| a == v && b == w))
        .count()
}

fn find_non_cluster_child(g: &MG, id: &str, cluster: Option<&str>) -> Option<String> {
    let children = g.children(Some(id));
    if children.is_empty() {
        return Some(id.to_string());
    }
    let mut reserve = None;
    for child in children {
        let found = find_non_cluster_child(g, &child, cluster);
        if let Some(f) = found {
            if find_common_edges(g, cluster, &f) > 0 {
                reserve = Some(f);
            } else {
                return Some(f);
            }
        }
    }
    reserve
}

fn in_extractable_cluster(g: &MG, db: &Db, node: &str, root: &str) -> bool {
    let mut parent = g.parent(node);
    while let Some(p) = parent {
        if p == root {
            break;
        }
        if db.entries.get(&p).is_some_and(|c| !c.external) {
            return true;
        }
        parent = g.parent(&p);
    }
    false
}

fn find_safe_anchor(g: &MG, db: &Db, cluster: &str, excluded: Option<&str>) -> Option<String> {
    for child in g.children(Some(cluster)) {
        if let Some(ex) = excluded {
            if child == ex || db.is_descendant(&child, ex) {
                continue;
            }
        }
        let Some(cand) = find_non_cluster_child(g, &child, Some(cluster)) else {
            continue;
        };
        if !in_extractable_cluster(g, db, &cand, cluster) {
            return Some(cand);
        }
    }
    None
}

fn adjust_clusters_and_edges(ctx: &mut Ctx) {
    let g = &mut ctx.subs[0].g;
    let db = &mut ctx.db;
    for id in g.nodes() {
        if g.has_children(&id) {
            db.descendants
                .insert(id.clone(), extract_descendants(g, &id));
            let anchor = find_non_cluster_child(g, &id, Some(&id));
            db.set(
                &id,
                DbEntry {
                    anchor,
                    external: false,
                },
            );
        }
    }
    for id in g.nodes() {
        if g.has_children(&id) {
            for e in g.edges() {
                let d1 = db.is_descendant(&e.v, &id);
                let d2 = db.is_descendant(&e.w, &id);
                if d1 ^ d2 {
                    db.entries.get_mut(&id).unwrap().external = true;
                }
            }
        }
    }
    for id in db.keys.clone() {
        let ncc = db.entries[&id].anchor.clone();
        let parent = ncc.as_deref().and_then(|n| g.parent(n));
        if let Some(p) = &parent {
            if *p != id && db.entries.get(p).is_some_and(|e| !e.external) {
                db.entries.get_mut(&id).unwrap().anchor = Some(p.clone());
            }
        }
        let direct_out = g.edges().iter().any(|e| e.v == id);
        if let Some(n) = &ncc {
            if db.entries[&id].external && direct_out && in_extractable_cluster(g, db, n, &id) {
                if let Some(safe) = find_safe_anchor(g, db, &id, g.parent(n).as_deref()) {
                    db.entries.get_mut(&id).unwrap().anchor = Some(safe);
                }
            }
        }
    }
    let anchor_of = |db: &Db, id: &str| -> String {
        match db.entries.get(id) {
            Some(e) if e.external => e.anchor.clone().unwrap_or_else(|| id.to_string()),
            _ => id.to_string(),
        }
    };
    for e in g.edges() {
        if db.entries.contains_key(&e.v) || db.entries.contains_key(&e.w) {
            let v = anchor_of(db, &e.v);
            let w = anchor_of(db, &e.w);
            let label = g.remove_edge(&e).unwrap();
            if v != e.v {
                if let Some(p) = g.parent(&v) {
                    if let Some(x) = db.entries.get_mut(&p) {
                        x.external = true;
                    }
                }
            }
            if w != e.w {
                if let Some(p) = g.parent(&w) {
                    if let Some(x) = db.entries.get_mut(&p) {
                        x.external = true;
                    }
                }
            }
            g.set_edge(&v, &w, label, e.name.as_deref());
        }
    }
    extractor(ctx, 0, 0);
}

fn edge_in_cluster(db: &Db, v: &str, w: &str, cluster: &str) -> bool {
    if v == cluster || w == cluster {
        return false;
    }
    db.is_descendant(v, cluster) || db.is_descendant(w, cluster)
}

/// `copy(clusterId, graph, newGraph, rootId)`: moves a cluster's contents into
/// its own graph.
fn copy(db: &Db, from: &mut MG, to: &mut MG, cluster: &str, root: &str) {
    let mut nodes = from.children(Some(cluster));
    if cluster != root {
        nodes.push(cluster.to_string());
    }
    for node in nodes {
        if from.has_children(&node) {
            copy(db, from, to, &node, root);
        } else {
            let data = from.node(&node).cloned().unwrap_or_default();
            to.set_node(&node, data);
            let parent = from.parent(&node);
            if parent.as_deref() != Some(root) {
                to.set_parent(&node, parent.as_deref());
            }
            if cluster != root && node != cluster {
                to.set_parent(&node, Some(cluster));
            }
            for e in from.edges() {
                if edge_in_cluster(db, &e.v, &e.w, root) {
                    let label = *from.e(&e);
                    to.set_edge(&e.v, &e.w, label, e.name.as_deref());
                }
            }
        }
        from.remove_node(&node);
    }
}

fn extractor(ctx: &mut Ctx, gi: usize, depth: usize) {
    if depth > 10 {
        return;
    }
    let nodes = ctx.subs[gi].g.nodes();
    if !nodes.iter().any(|v| ctx.subs[gi].g.has_children(v)) {
        return;
    }
    for node in &nodes {
        let Some(entry) = ctx.db.entries.get(node) else {
            continue;
        };
        if entry.external || !ctx.subs[gi].g.has_children(node) {
            continue;
        }
        let Some(MNode::Cluster { c }) = ctx.subs[gi].g.node(node).cloned() else {
            continue;
        };
        let (dir, tb) = match ctx.input.clusters[c].direction {
            Some(d) => (
                d,
                d == Direction::TopBottom && !ctx.model.cluster_td.get(c).copied().unwrap_or(false),
            ),
            None if ctx.subs[gi].tb => (Direction::LeftRight, false),
            None => (Direction::TopBottom, true),
        };
        let mut sub: MG = Graph::new(true, true, true);
        copy(&ctx.db, &mut ctx.subs[gi].g, &mut sub, node, node);
        let k = ctx.subs.len();
        ctx.subs.push(Sub {
            g: sub,
            dir,
            tb,
            nodesep: 50.0,
            ranksep: 50.0,
        });
        ctx.subs[gi]
            .g
            .set_node(node, MNode::ClusterNode { c, graph: k });
    }
    for node in ctx.subs[gi].g.nodes() {
        if let Some(MNode::ClusterNode { graph, .. }) = ctx.subs[gi].g.node(&node).cloned() {
            extractor(ctx, graph, depth + 1);
        }
    }
}

// ---------------------------------------------------------------- recursive render

/// One graph's drawing, in its own coordinates.
#[derive(Default)]
struct Rendered {
    nodes: Vec<(usize, Point)>,
    clusters: Vec<(usize, Rect, Direction)>,
    edges: Vec<(usize, EdgeRoute)>,
    bbox: Option<Rect>,
    /// Invisible boxes (self-loop dummies).
    hidden: Vec<Rect>,
    /// Per edge: dagre's own points (see [`Extras::raw_points`]).
    raw: Vec<(usize, Vec<Point>)>,
    /// The `diff` of the last group in the graph (`positionNode`'s offset).
    diff: f64,
}

impl Rendered {
    fn add_box(&mut self, r: Rect) {
        self.bbox = Some(match self.bbox {
            Some(b) => b.union(&r),
            None => r,
        });
    }
    fn absorb(&mut self, inner: Rendered, dx: f64, dy: f64) {
        let shift = |p: Point| Point::new(p.x + dx, p.y + dy);
        for (i, p) in inner.nodes {
            self.nodes.push((i, shift(p)));
        }
        for (c, r, d) in inner.clusters {
            self.clusters
                .push((c, Rect::new(r.x + dx, r.y + dy, r.w, r.h), d));
        }
        for (i, mut e) in inner.edges {
            e.points = e.points.into_iter().map(shift).collect();
            e.label = e.label.map(shift);
            self.edges.push((i, e));
        }
        for b in inner.hidden {
            self.hidden.push(Rect::new(b.x + dx, b.y + dy, b.w, b.h));
        }
        for (i, pts) in inner.raw {
            self.raw.push((i, pts.into_iter().map(shift).collect()));
        }
        if let Some(b) = inner.bbox {
            self.add_box(Rect::new(b.x + dx, b.y + dy, b.w, b.h));
        }
    }
}

/// One of a self-loop's three segments: its ends and dagre's route.
type LoopSeg = (String, String, core::OutEdge);

fn rankdir(d: Direction) -> RankDir {
    match d {
        Direction::TopBottom => RankDir::TB,
        Direction::BottomTop => RankDir::BT,
        Direction::LeftRight => RankDir::LR,
        Direction::RightLeft => RankDir::RL,
    }
}

/// `clamp(v, lo, hi)`.
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(v))
}

fn render(ctx: &mut Ctx, gi: usize, parent: Option<usize>) -> Rendered {
    // measureDagreGraph: every node of a cluster's graph goes into the cluster.
    if let Some(c) = parent {
        let name = ctx.model.cluster_names[c].clone();
        let g = &mut ctx.subs[gi].g;
        for v in g.nodes() {
            g.set_node(&name, MNode::Cluster { c });
            if g.parent(&v).is_none() {
                g.set_parent(&v, Some(&name));
            }
        }
    }
    let mut inner: HashMap<String, (Rendered, f64, f64)> = HashMap::new();
    let (nodesep, ranksep) = (ctx.subs[gi].nodesep, ctx.subs[gi].ranksep);
    for v in ctx.subs[gi].g.nodes() {
        if let Some(MNode::ClusterNode { c, graph }) = ctx.subs[gi].g.node(&v).cloned() {
            ctx.subs[graph].ranksep = ranksep + 25.0;
            ctx.subs[graph].nodesep = nodesep;
            let r = render(ctx, graph, Some(c));
            let b = r.bbox.unwrap_or_default();
            let (w, h) = (b.w, b.h);
            inner.insert(v, (r, w, h));
        }
    }
    let sub = &ctx.subs[gi];
    let g = &sub.g;
    let input = ctx.input;
    let size = |v: &str| -> (f64, f64) {
        match g.node(v) {
            Some(MNode::Leaf { idx }) => (input.nodes[*idx].width, input.nodes[*idx].height),
            Some(MNode::Special) => (SELF_LOOP_DUMMY, SELF_LOOP_DUMMY),
            Some(MNode::ClusterNode { .. }) => inner.get(v).map_or((0.0, 0.0), |r| (r.1, r.2)),
            _ => (0.0, 0.0),
        }
    };
    // dagre
    let mut dg: Graph<InNode, InEdge> = Graph::new(true, true, true);
    for v in g.nodes() {
        let (width, height) = size(&v);
        dg.set_node(&v, InNode { width, height });
        let p = g.parent(&v);
        dg.set_parent(&v, p.as_deref());
    }
    for e in g.edges() {
        let l = g.e(&e);
        dg.set_edge(
            &e.v,
            &e.w,
            InEdge {
                minlen: l.minlen,
                weight: 1.0,
                width: l.w,
                height: l.h,
                labelpos: 'c',
                labeloffset: 10.0,
            },
            e.name.as_deref(),
        );
    }
    let out = core::layout(
        &dg,
        Opts {
            rankdir: rankdir(sub.dir),
            nodesep,
            ranksep,
            edgesep: 20.0,
            marginx: MARGIN,
            marginy: MARGIN,
        },
    );

    // paintDagreLayoutCore
    let mut res = Rendered::default();
    let pos = |v: &str| {
        out.nodes
            .get(v)
            .map_or(Point::default(), |n| Point::new(n.x, n.y))
    };
    let mut diffs: Vec<(String, f64)> = Vec::new();
    for v in g.nodes() {
        let Some(n) = out.nodes.get(&v).copied() else {
            continue;
        };
        match g.node(&v) {
            Some(MNode::ClusterNode { .. }) => {
                let Some((r, w, h)) = inner.remove(&v) else {
                    continue;
                };
                // `positionNode`: the inner drawing's box, centred, then moved by
                // the inner graph's `diff` and up by the padding.
                let dx = n.x + r.diff - w / 2.0;
                let dy = n.y - h / 2.0 - MARGIN;
                res.absorb(r, dx, dy);
            }
            Some(MNode::Cluster { c }) if g.has_children(&v) => {
                let need = input.clusters[*c].min_width.max(CLUSTER_PAD);
                let (width, diff) = if n.width <= need {
                    (need, (need - n.width) / 2.0 - CLUSTER_PAD)
                } else {
                    (n.width, -CLUSTER_PAD)
                };
                let rect = Rect::new(n.x - width / 2.0, n.y - n.height / 2.0, width, n.height);
                res.add_box(rect);
                res.clusters.push((*c, rect, sub.dir));
                diffs.push((v.clone(), diff));
            }
            Some(MNode::Leaf { idx }) => {
                res.nodes.push((*idx, Point::new(n.x, n.y)));
                res.add_box(Rect::centered(Point::new(n.x, n.y), n.width, n.height));
            }
            Some(MNode::Special) => {
                let r = Rect::new(n.x, n.y, SELF_LOOP_DUMMY, SELF_LOOP_DUMMY);
                res.add_box(r);
                res.hidden.push(r);
            }
            _ => {}
        }
    }
    // Edges (`getEdgesToRender`).
    let mut loops: HashMap<usize, [Option<LoopSeg>; 3]> = HashMap::new();
    let mut loop_order: Vec<usize> = Vec::new();
    for e in g.edges() {
        let l = *g.e(&e);
        let Some(oe) = out.edges.get(&e).cloned() else {
            continue;
        };
        match l.part {
            Part::Whole => {
                let n = oe.points.len();
                let mut points = vec![pos(&e.v)];
                if n > 2 {
                    points.extend(oe.points[1..n - 1].iter().map(|p| Point::new(p.x, p.y)));
                }
                points.push(pos(&e.w));
                let label = input.edges[l.idx].label.map(|_| match oe.label {
                    Some(p) => Point::new(p.x, p.y),
                    None => points[0].lerp(points[points.len() - 1], 0.5),
                });
                if let Some(lp) = label {
                    res.add_box(Rect::centered(lp, l.w, l.h));
                }
                res.raw.push((
                    l.idx,
                    oe.points.iter().map(|p| Point::new(p.x, p.y)).collect(),
                ));
                res.edges.push((l.idx, EdgeRoute { points, label }));
            }
            Part::Loop(k) => {
                // Keyed by the original edge id: two loops on one node share it.
                let key = l.idx;
                let slot = loops.entry(key).or_insert_with(|| {
                    loop_order.push(key);
                    [None, None, None]
                });
                slot[k as usize] = Some((e.v.clone(), e.w.clone(), oe));
            }
        }
    }
    for key in loop_order {
        let segs = &loops[&key];
        let (Some(s0), Some(s1), Some(s2)) = (&segs[0], &segs[1], &segs[2]) else {
            continue;
        };
        let node_name = &s0.0;
        let Some(n) = out.nodes.get(node_name).copied() else {
            continue;
        };
        let (lw, lh) = input.edges[key].label.unwrap_or((0.0, 0.0));
        // getSelfLoopSide: from the dummies' centre.
        let mut hints = Vec::new();
        for id in [&s0.1, &s1.0, &s1.1, &s2.0] {
            if id != node_name && !hints.iter().any(|(h, _): &(&String, Pt)| *h == id) {
                if let Some(d) = out.nodes.get(id) {
                    hints.push((id, Pt { x: d.x, y: d.y }));
                }
            }
        }
        let default_side = match sub.dir {
            Direction::BottomTop => 'b',
            Direction::LeftRight => 'r',
            Direction::RightLeft => 'l',
            Direction::TopBottom => 't',
        };
        let side = if hints.is_empty() {
            default_side
        } else {
            let k = hints.len() as f64;
            let (cx, cy) = hints
                .iter()
                .fold((0.0, 0.0), |a, (_, p)| (a.0 + p.x / k, a.1 + p.y / k));
            let (dx, dy) = (cx - n.x, cy - n.y);
            if dx.abs() > dy.abs() {
                if dx > 0.0 { 'r' } else { 'l' }
            } else if dy.abs() > 0.0 {
                if dy > 0.0 { 'b' } else { 't' }
            } else {
                default_side
            }
        };
        let (x, y, hw, hh) = (n.x, n.y, n.width / 2.0, n.height / 2.0);
        let max_span = 36.0f64.max(100.0f64.min(n.width * 0.8));
        let span = clamp(lw.max(n.width * 0.35), 36.0, max_span);
        let depth = clamp(n.width.min(n.height) * 0.45, 24.0, 48.0);
        let p = Point::new;
        let points = match side {
            'b' => vec![
                p(x - span / 2.0, y + hh),
                p(x - span / 2.0, y + hh + depth),
                p(x + span / 2.0, y + hh + depth),
                p(x + span / 2.0, y + hh),
            ],
            'r' => vec![
                p(x + hw, y - span / 2.0),
                p(x + hw + depth, y - span / 2.0),
                p(x + hw + depth, y + span / 2.0),
                p(x + hw, y + span / 2.0),
            ],
            'l' => vec![
                p(x - hw, y - span / 2.0),
                p(x - hw - depth, y - span / 2.0),
                p(x - hw - depth, y + span / 2.0),
                p(x - hw, y + span / 2.0),
            ],
            _ => vec![
                p(x - span / 2.0, y - hh),
                p(x - span / 2.0, y - hh - depth),
                p(x + span / 2.0, y - hh - depth),
                p(x + span / 2.0, y - hh),
            ],
        };
        let gap = 4.0;
        let xs = points.iter().map(|q| q.x);
        let ys = points.iter().map(|q| q.y);
        let label_at = match side {
            'b' => p(x, ys.fold(f64::NEG_INFINITY, f64::max) + lh / 2.0 + gap),
            'r' => p(xs.fold(f64::NEG_INFINITY, f64::max) + lw / 2.0 + gap, y),
            'l' => p(xs.fold(f64::INFINITY, f64::min) - lw / 2.0 - gap, y),
            _ => p(x, ys.fold(f64::INFINITY, f64::min) - lh / 2.0 - gap),
        };
        let label = input.edges[key].label.map(|_| label_at);
        if let Some(lp) = label {
            res.add_box(Rect::centered(lp, lw, lh));
        }
        res.raw.push((key, points.clone()));
        res.edges.push((key, EdgeRoute { points, label }));
    }
    // The last group's `diff` (`n.isGroup`).
    res.diff = diffs.last().map_or(0.0, |d| d.1);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{EdgeRouting, LayoutCluster, LayoutEdge, LayoutNode};

    fn input(n: usize, edges: &[(usize, usize)], dir: Direction) -> (LayoutInput, DagreModel) {
        let inp = LayoutInput {
            nodes: (0..n)
                .map(|_| LayoutNode {
                    width: 60.0,
                    height: 40.0,
                    cluster: None,
                })
                .collect(),
            edges: edges
                .iter()
                .map(|&(a, b)| LayoutEdge::new(a, b, 1, None))
                .collect(),
            clusters: Vec::new(),
            direction: dir,
            node_sep: 50.0,
            rank_sep: 50.0,
            edge_sep: 20.0,
            cluster_padding: 8.0,
            inner_node_sep: 50.0,
            inner_rank_sep: 50.0,
            routing: EdgeRouting::Splines,
            dagre_groups: true,
            dagre: None,
        };
        let model = DagreModel {
            node_names: (0..n).map(|i| format!("n{i}")).collect(),
            cluster_names: Vec::new(),
            order: (0..n).map(DagreItem::Node).collect(),
            cluster_td: Vec::new(),
        };
        (inp, model)
    }

    #[test]
    fn chain_ranks_are_a_rank_and_a_half_apart() {
        let (inp, m) = input(3, &[(0, 1), (1, 2)], Direction::TopBottom);
        let out = layout(&inp, &m);
        assert_eq!(out.nodes[1].y - out.nodes[0].y, 90.0);
        assert_eq!(out.nodes[2].y - out.nodes[1].y, 90.0);
        assert_eq!(out.nodes[0].x, out.nodes[2].x);
        assert_eq!(out.edges[0].points.len(), 3);
    }

    #[test]
    fn isolated_subgraph_turns_to_lr_and_is_centred_on_its_node() {
        let (mut inp, mut m) = input(3, &[(0, 1)], Direction::TopBottom);
        inp.clusters.push(LayoutCluster::new(None, 0.0));
        inp.nodes[0].cluster = Some(0);
        inp.nodes[1].cluster = Some(0);
        m.cluster_names.push("S".into());
        m.order.insert(0, DagreItem::Cluster(0));
        let out = layout(&inp, &m);
        assert_eq!(out.cluster_directions[0], Direction::LeftRight);
        // LR inside: the two members side by side.
        assert_eq!(out.nodes[0].y, out.nodes[1].y);
        assert!(out.nodes[1].x > out.nodes[0].x);
        let r = out.clusters[0];
        assert!((r.center().y - out.nodes[0].y).abs() < 1e-9);
    }

    #[test]
    fn a_self_loop_goes_round_the_node_once() {
        let (inp, m) = input(2, &[(0, 1), (1, 1)], Direction::TopBottom);
        let out = layout(&inp, &m);
        assert_eq!(out.edges[1].points.len(), 4);
    }
}
