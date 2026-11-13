//! dagre's `layout` (dagre-d3-es 7.0.14, as mermaid 12.3 bundles it), ported
//! step by step: the same passes in the same order over a [`Graph`] that
//! enumerates like graphlib's, so ties break as they do upstream.

use super::graph::{EdgeObj, Graph};
use std::collections::{HashMap, HashSet};

/// `rankdir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankDir {
    TB,
    BT,
    LR,
    RL,
}

/// The input graph's settings (`nodesep`, `ranksep`, …).
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub rankdir: RankDir,
    pub nodesep: f64,
    pub ranksep: f64,
    pub edgesep: f64,
    pub marginx: f64,
    pub marginy: f64,
}

/// What dagre reads from an input node.
#[derive(Debug, Clone, Copy, Default)]
pub struct InNode {
    pub width: f64,
    pub height: f64,
}

/// What dagre reads from an input edge.
#[derive(Debug, Clone, Copy)]
pub struct InEdge {
    pub minlen: f64,
    pub weight: f64,
    pub width: f64,
    pub height: f64,
    /// `labelpos`: `'c'`, `'l'` or `'r'`.
    pub labelpos: char,
    pub labeloffset: f64,
}

impl Default for InEdge {
    fn default() -> Self {
        InEdge {
            minlen: 1.0,
            weight: 1.0,
            width: 0.0,
            height: 0.0,
            labelpos: 'r',
            labeloffset: 10.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

/// A laid out node: centre and size (the size changes only for compound nodes).
#[derive(Debug, Clone, Copy, Default)]
pub struct OutNode {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A laid out edge: its points and, with a label, the label's centre.
#[derive(Debug, Clone, Default)]
pub struct OutEdge {
    pub points: Vec<Pt>,
    pub label: Option<Pt>,
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    pub nodes: HashMap<String, OutNode>,
    pub edges: HashMap<EdgeObj, OutEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dummy {
    Edge,
    EdgeLabel,
    Border,
    SelfEdge,
    Root,
    EdgeProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorderType {
    Left,
    Right,
}

#[derive(Debug, Clone, Default)]
struct NodeAttrs {
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    rank: Option<f64>,
    order: Option<usize>,
    dummy: Option<Dummy>,
    labelpos: Option<char>,
    border_type: Option<BorderType>,
    border_top: Option<String>,
    border_bottom: Option<String>,
    border_left: Vec<Option<String>>,
    border_right: Vec<Option<String>>,
    min_rank: Option<i64>,
    max_rank: Option<i64>,
    self_edges: Vec<(EdgeObj, EdgeAttrs)>,
    e: Option<EdgeObj>,
    edge_label: Option<Box<EdgeAttrs>>,
    edge_obj: Option<EdgeObj>,
    label: Option<Box<EdgeAttrs>>,
}

#[derive(Debug, Clone, Default)]
struct EdgeAttrs {
    minlen: f64,
    weight: f64,
    width: f64,
    height: f64,
    labeloffset: f64,
    labelpos: char,
    label_rank: Option<f64>,
    points: Vec<Pt>,
    x: Option<f64>,
    y: Option<f64>,
    reversed: bool,
    forward_name: Option<String>,
    nesting_edge: bool,
}

type G = Graph<NodeAttrs, EdgeAttrs>;

struct Label {
    opts: Opts,
    ranksep: f64,
    nesting_root: Option<String>,
    node_rank_factor: f64,
    dummy_chains: Vec<String>,
    uid: usize,
}

impl Label {
    /// lodash's `uniqueId(prefix)`.
    fn unique_id(&mut self, prefix: &str) -> String {
        self.uid += 1;
        format!("{prefix}{}", self.uid)
    }
}

fn add_dummy_node(
    g: &mut G,
    l: &mut Label,
    kind: Dummy,
    mut attrs: NodeAttrs,
    name: &str,
) -> String {
    let mut v = l.unique_id(name);
    while g.has_node(&v) {
        v = l.unique_id(name);
    }
    attrs.dummy = Some(kind);
    g.set_node(&v, attrs);
    v
}

/// lodash `_.minBy`: the first item with the least defined, non-NaN key.
fn min_by<T>(items: impl IntoIterator<Item = T>, f: impl Fn(&T) -> Option<f64>) -> Option<T> {
    let mut best: Option<(f64, T)> = None;
    for it in items {
        if let Some(k) = f(&it) {
            if k.is_nan() {
                continue;
            }
            if best.as_ref().is_none_or(|(b, _)| k < *b) {
                best = Some((k, it));
            }
        }
    }
    best.map(|(_, t)| t)
}

/// Runs dagre on `input`.
pub fn layout(input: &Graph<InNode, InEdge>, opts: Opts) -> Output {
    let mut g: G = Graph::new(true, true, true);
    for v in input.nodes() {
        let n = input.n(&v);
        g.set_node(
            &v,
            NodeAttrs {
                width: n.width,
                height: n.height,
                ..Default::default()
            },
        );
        let p = input.parent(&v);
        g.set_parent(&v, p.as_deref());
    }
    for e in input.edges() {
        let l = input.e(&e);
        g.set_edge(
            &e.v,
            &e.w,
            EdgeAttrs {
                minlen: l.minlen,
                weight: l.weight,
                width: l.width,
                height: l.height,
                labeloffset: l.labeloffset,
                labelpos: l.labelpos,
                ..Default::default()
            },
            e.name.as_deref(),
        );
    }
    let mut lab = Label {
        opts,
        ranksep: opts.ranksep,
        nesting_root: None,
        node_rank_factor: 0.0,
        dummy_chains: Vec::new(),
        uid: 0,
    };
    run_layout(&mut g, &mut lab);
    translate_graph(&mut g, &lab);
    assign_node_intersects(&mut g);
    for e in g.edges() {
        let l = g.e_mut(&e);
        if l.reversed {
            l.points.reverse();
        }
    }
    acyclic_undo(&mut g);

    let mut out = Output::default();
    for v in input.nodes() {
        let n = g.n(&v);
        let compound = g.has_children(&v);
        out.nodes.insert(
            v.clone(),
            OutNode {
                x: n.x,
                y: n.y,
                width: if compound { n.width } else { input.n(&v).width },
                height: if compound {
                    n.height
                } else {
                    input.n(&v).height
                },
            },
        );
    }
    for e in input.edges() {
        if let Some(l) = g.edge(&e.v, &e.w, e.name.as_deref()) {
            out.edges.insert(
                e.clone(),
                OutEdge {
                    points: l.points.clone(),
                    label: l.x.zip(l.y).map(|(x, y)| Pt { x, y }),
                },
            );
        }
    }
    out
}

fn run_layout(g: &mut G, l: &mut Label) {
    make_space_for_edge_labels(g, l);
    remove_self_edges(g);
    acyclic_run(g, l);
    nesting_run(g, l);
    rank(g);
    inject_edge_label_proxies(g, l);
    remove_empty_ranks(g, l);
    nesting_cleanup(g, l);
    normalize_ranks(g);
    assign_rank_min_max(g);
    remove_edge_label_proxies(g);
    normalize_run(g, l);
    parent_dummy_chains(g, l);
    add_border_segments(g, l);
    order(g, l);
    insert_self_edges(g, l);
    if matches!(l.opts.rankdir, RankDir::LR | RankDir::RL) {
        swap_width_height(g);
    }
    position(g, l);
    position_self_edges(g);
    remove_border_nodes(g);
    normalize_undo(g, l);
    // fixupEdgeLabelCoords: only `l` / `r` labels move.
    for e in g.edges() {
        let ed = g.e_mut(&e);
        if ed.x.is_some() {
            if ed.labelpos == 'l' || ed.labelpos == 'r' {
                ed.width -= ed.labeloffset;
            }
            let shift = ed.width / 2.0 + ed.labeloffset;
            if let Some(x) = ed.x.as_mut() {
                match ed.labelpos {
                    'l' => *x -= shift,
                    'r' => *x += shift,
                    _ => {}
                }
            }
        }
    }
    // undoCoordinateSystem
    if matches!(l.opts.rankdir, RankDir::BT | RankDir::RL) {
        for v in g.nodes() {
            let n = g.n_mut(&v);
            n.y = -n.y;
        }
        for e in g.edges() {
            let ed = g.e_mut(&e);
            for p in &mut ed.points {
                p.y = -p.y;
            }
            if let Some(y) = ed.y.as_mut() {
                *y = -*y;
            }
        }
    }
    if matches!(l.opts.rankdir, RankDir::LR | RankDir::RL) {
        for v in g.nodes() {
            let n = g.n_mut(&v);
            std::mem::swap(&mut n.x, &mut n.y);
        }
        for e in g.edges() {
            let ed = g.e_mut(&e);
            for p in &mut ed.points {
                std::mem::swap(&mut p.x, &mut p.y);
            }
            if ed.x.is_some() {
                std::mem::swap(&mut ed.x, &mut ed.y);
            }
        }
        swap_width_height(g);
    }
}

fn make_space_for_edge_labels(g: &mut G, l: &mut Label) {
    l.ranksep /= 2.0;
    let vertical = matches!(l.opts.rankdir, RankDir::TB | RankDir::BT);
    for e in g.edges() {
        let ed = g.e_mut(&e);
        ed.minlen *= 2.0;
        if ed.labelpos != 'c' {
            if vertical {
                ed.width += ed.labeloffset;
            } else {
                ed.height += ed.labeloffset;
            }
        }
    }
}

fn remove_self_edges(g: &mut G) {
    for e in g.edges() {
        if e.v == e.w {
            let label = g.remove_edge(&e).unwrap();
            g.n_mut(&e.v).self_edges.push((e, label));
        }
    }
}

fn acyclic_run(g: &mut G, l: &mut Label) {
    fn dfs(
        g: &G,
        v: &str,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
        fas: &mut Vec<EdgeObj>,
    ) {
        if !visited.insert(v.to_string()) {
            return;
        }
        stack.insert(v.to_string());
        for e in g.out_edges(v) {
            if stack.contains(&e.w) {
                fas.push(e);
            } else {
                dfs(g, &e.w, visited, stack, fas);
            }
        }
        stack.remove(v);
    }
    let mut fas = Vec::new();
    let (mut visited, mut stack) = (HashSet::new(), HashSet::new());
    for v in g.nodes() {
        dfs(g, &v, &mut visited, &mut stack, &mut fas);
    }
    for e in fas {
        let Some(mut label) = g.remove_edge(&e) else {
            continue;
        };
        label.forward_name = e.name.clone();
        label.reversed = true;
        let name = l.unique_id("rev");
        g.set_edge(&e.w, &e.v, label, Some(&name));
    }
}

fn acyclic_undo(g: &mut G) {
    for e in g.edges() {
        if g.e(&e).reversed {
            let mut label = g.remove_edge(&e).unwrap();
            let fwd = label.forward_name.take();
            label.reversed = false;
            g.set_edge(&e.w, &e.v, label, fwd.as_deref());
        }
    }
}

fn nesting_run(g: &mut G, l: &mut Label) {
    let root = add_dummy_node(g, l, Dummy::Root, NodeAttrs::default(), "_root");
    let mut depths: HashMap<String, f64> = HashMap::new();
    fn tree_depths(g: &G, v: &str, depth: f64, depths: &mut HashMap<String, f64>) {
        for c in g.children(Some(v)) {
            tree_depths(g, &c, depth + 1.0, depths);
        }
        depths.insert(v.to_string(), depth);
    }
    for v in g.children(None) {
        tree_depths(g, &v, 1.0, &mut depths);
    }
    let height = depths.values().copied().fold(f64::NEG_INFINITY, f64::max) - 1.0;
    let node_sep = 2.0 * height + 1.0;
    l.nesting_root = Some(root.clone());
    for e in g.edges() {
        g.e_mut(&e).minlen *= node_sep;
    }
    let weight: f64 = g.edges().iter().map(|e| g.e(e).weight).sum::<f64>() + 1.0;
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        g: &mut G,
        l: &mut Label,
        root: &str,
        node_sep: f64,
        weight: f64,
        height: f64,
        depths: &HashMap<String, f64>,
        v: &str,
    ) {
        let children = g.children(Some(v));
        if children.is_empty() {
            if v != root {
                g.set_edge(
                    root,
                    v,
                    EdgeAttrs {
                        weight: 0.0,
                        minlen: node_sep,
                        ..Default::default()
                    },
                    None,
                );
            }
            return;
        }
        let top = add_border_node(g, l, "_bt");
        let bottom = add_border_node(g, l, "_bb");
        g.set_parent(&top, Some(v));
        g.n_mut(v).border_top = Some(top.clone());
        g.set_parent(&bottom, Some(v));
        g.n_mut(v).border_bottom = Some(bottom.clone());
        for child in children {
            dfs(g, l, root, node_sep, weight, height, depths, &child);
            let cn = g.n(&child);
            let child_top = cn.border_top.clone().unwrap_or_else(|| child.clone());
            let child_bottom = cn.border_bottom.clone().unwrap_or_else(|| child.clone());
            let this_weight = if cn.border_top.is_some() {
                weight
            } else {
                2.0 * weight
            };
            let minlen = if child_top != child_bottom {
                1.0
            } else {
                height - depths[v] + 1.0
            };
            let attrs = EdgeAttrs {
                weight: this_weight,
                minlen,
                nesting_edge: true,
                ..Default::default()
            };
            g.set_edge(&top, &child_top, attrs.clone(), None);
            g.set_edge(&child_bottom, &bottom, attrs, None);
        }
        if g.parent(v).is_none() {
            g.set_edge(
                root,
                &top,
                EdgeAttrs {
                    weight: 0.0,
                    minlen: height + depths[v],
                    ..Default::default()
                },
                None,
            );
        }
    }
    for child in g.children(None) {
        dfs(g, l, &root, node_sep, weight, height, &depths, &child);
    }
    l.node_rank_factor = node_sep;
}

fn add_border_node(g: &mut G, l: &mut Label, prefix: &str) -> String {
    add_dummy_node(g, l, Dummy::Border, NodeAttrs::default(), prefix)
}

fn nesting_cleanup(g: &mut G, l: &mut Label) {
    if let Some(root) = l.nesting_root.take() {
        g.remove_node(&root);
    }
    for e in g.edges() {
        if g.e(&e).nesting_edge {
            g.remove_edge(&e);
        }
    }
}

// ---------------------------------------------------------------- rank

#[derive(Debug, Clone, Default)]
struct RNode {
    rank: f64,
}

#[derive(Debug, Clone, Default)]
struct REdge {
    weight: f64,
    minlen: f64,
}

#[derive(Debug, Clone, Default)]
struct TNode {
    low: usize,
    lim: usize,
    parent: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TEdge {
    cutvalue: f64,
}

type SG = Graph<RNode, REdge>;
type TG = Graph<TNode, TEdge>;

/// `rank(asNonCompoundGraph(g))` with the network simplex ranker.
fn rank(g: &mut G) {
    // asNonCompoundGraph, then simplify: the nodes without children, and one edge
    // per pair (weights summed, the largest minlen).
    let mut s: SG = Graph::new(true, false, false);
    for v in g.nodes() {
        if !g.has_children(&v) {
            s.set_node(&v, RNode::default());
        }
    }
    for e in g.edges() {
        let l = g.e(&e);
        let (w0, m0) = s
            .edge(&e.v, &e.w, None)
            .map(|x| (x.weight, x.minlen))
            .unwrap_or((0.0, 1.0));
        s.set_edge(
            &e.v,
            &e.w,
            REdge {
                weight: w0 + l.weight,
                minlen: m0.max(l.minlen),
            },
            None,
        );
    }
    network_simplex(&mut s);
    for v in s.nodes() {
        if let Some(n) = g.node_mut(&v) {
            n.rank = Some(s.n(&v).rank);
        }
    }
}

fn longest_path(g: &mut SG) {
    fn dfs(g: &mut SG, v: &str, visited: &mut HashSet<String>) -> f64 {
        if !visited.insert(v.to_string()) {
            return g.n(v).rank;
        }
        let mut rank: Option<f64> = None;
        for e in g.out_edges(v) {
            let r = dfs(g, &e.w, visited) - g.e(&e).minlen;
            if !r.is_nan() && rank.is_none_or(|x| r < x) {
                rank = Some(r);
            }
        }
        let r = rank.unwrap_or(0.0);
        g.n_mut(v).rank = r;
        r
    }
    let mut visited = HashSet::new();
    for v in g.sources() {
        dfs(g, &v, &mut visited);
    }
}

fn slack(g: &SG, e: &EdgeObj) -> f64 {
    g.n(&e.w).rank - g.n(&e.v).rank - g.e(e).minlen
}

fn feasible_tree(g: &mut SG) -> TG {
    let mut t: TG = Graph::new(false, false, false);
    let nodes = g.nodes();
    let Some(start) = nodes.first() else {
        return t;
    };
    let size = g.node_count();
    t.set_node(start, TNode::default());
    fn tight_tree(t: &mut TG, g: &SG) -> usize {
        fn dfs(t: &mut TG, g: &SG, v: &str) {
            for e in g.node_edges(v) {
                let w = if v == e.v { e.w.clone() } else { e.v.clone() };
                if !t.has_node(&w) && slack(g, &e) == 0.0 {
                    t.set_node(&w, TNode::default());
                    t.set_edge(v, &w, TEdge::default(), None);
                    dfs(t, g, &w);
                }
            }
        }
        for v in t.nodes() {
            dfs(t, g, &v);
        }
        t.node_count()
    }
    while tight_tree(&mut t, g) < size {
        let edges = g.edges();
        let Some(edge) = min_by(edges.iter(), |e| {
            (t.has_node(&e.v) != t.has_node(&e.w)).then(|| slack(g, e))
        }) else {
            break;
        };
        let delta = if t.has_node(&edge.v) {
            slack(g, edge)
        } else {
            -slack(g, edge)
        };
        for v in t.nodes() {
            g.n_mut(&v).rank += delta;
        }
    }
    t
}

fn network_simplex(g: &mut SG) {
    longest_path(g);
    let mut t = feasible_tree(g);
    init_low_lim(&mut t);
    init_cut_values(&mut t, g);
    while let Some(e) = leave_edge(&t) {
        let Some(f) = enter_edge(&t, g, &e) else {
            break;
        };
        t.remove_edge(&e);
        t.set_edge(&f.v, &f.w, TEdge::default(), None);
        init_low_lim(&mut t);
        init_cut_values(&mut t, g);
        update_ranks(&t, g);
    }
}

fn init_low_lim(t: &mut TG) {
    let Some(root) = t.nodes().into_iter().next() else {
        return;
    };
    fn dfs(
        t: &mut TG,
        visited: &mut HashSet<String>,
        mut next_lim: usize,
        v: &str,
        parent: Option<&str>,
    ) -> usize {
        let low = next_lim;
        visited.insert(v.to_string());
        for w in t.neighbors(v) {
            if !visited.contains(&w) {
                next_lim = dfs(t, visited, next_lim, &w, Some(v));
            }
        }
        let label = t.n_mut(v);
        label.low = low;
        label.lim = next_lim;
        next_lim += 1;
        label.parent = parent.map(str::to_string);
        next_lim
    }
    dfs(t, &mut HashSet::new(), 1, &root, None);
}

/// graphlib's `alg.postorder` / `alg.preorder`.
fn dfs_order<N: Default, E: Default>(g: &Graph<N, E>, vs: &[String], post: bool) -> Vec<String> {
    fn go<N: Default, E: Default>(
        g: &Graph<N, E>,
        v: &str,
        post: bool,
        visited: &mut HashSet<String>,
        acc: &mut Vec<String>,
    ) {
        if !visited.insert(v.to_string()) {
            return;
        }
        if !post {
            acc.push(v.to_string());
        }
        let next = if g.is_directed() {
            g.successors(v)
        } else {
            g.neighbors(v)
        };
        for w in next {
            go(g, &w, post, visited, acc);
        }
        if post {
            acc.push(v.to_string());
        }
    }
    let mut acc = Vec::new();
    let mut visited = HashSet::new();
    for v in vs {
        go(g, v, post, &mut visited, &mut acc);
    }
    acc
}

fn init_cut_values(t: &mut TG, g: &SG) {
    let mut vs = dfs_order(t, &t.nodes(), true);
    vs.pop();
    for v in vs {
        let parent = t.n(&v).parent.clone().expect("tree parent");
        let cv = calc_cut_value(t, g, &v, &parent);
        t.edge_mut(&v, &parent, None).unwrap().cutvalue = cv;
    }
}

fn calc_cut_value(t: &TG, g: &SG, child: &str, parent: &str) -> f64 {
    let mut child_is_tail = true;
    let graph_edge = match g.edge(child, parent, None) {
        Some(e) => e,
        None => {
            child_is_tail = false;
            g.edge(parent, child, None).expect("tree edge in graph")
        }
    };
    let mut cut = graph_edge.weight;
    for e in g.node_edges(child) {
        let is_out = e.v == child;
        let other = if is_out { &e.w } else { &e.v };
        if other != parent {
            let points_to_head = is_out == child_is_tail;
            let w = g.e(&e).weight;
            cut += if points_to_head { w } else { -w };
            if let Some(te) = t.edge(child, other, None) {
                let ocv = te.cutvalue;
                cut += if points_to_head { -ocv } else { ocv };
            }
        }
    }
    cut
}

fn leave_edge(t: &TG) -> Option<EdgeObj> {
    t.edges().into_iter().find(|e| t.e(e).cutvalue < 0.0)
}

fn enter_edge(t: &TG, g: &SG, edge: &EdgeObj) -> Option<EdgeObj> {
    let (mut v, mut w) = (edge.v.clone(), edge.w.clone());
    if !g.has_edge(&v, &w, None) {
        std::mem::swap(&mut v, &mut w);
    }
    let (vl, wl) = (t.n(&v), t.n(&w));
    let (tail, flip) = if vl.lim > wl.lim {
        (wl, true)
    } else {
        (vl, false)
    };
    let desc = |x: &TNode| tail.low <= x.lim && x.lim <= tail.lim;
    let cands: Vec<EdgeObj> = g
        .edges()
        .into_iter()
        .filter(|e| flip == desc(t.n(&e.v)) && flip != desc(t.n(&e.w)))
        .collect();
    min_by(cands, |e| Some(slack(g, e)))
}

fn update_ranks(t: &TG, g: &mut SG) {
    let Some(root) = t.nodes().into_iter().next() else {
        return;
    };
    let vs = dfs_order(t, &[root], false);
    for v in vs.into_iter().skip(1) {
        let parent = t.n(&v).parent.clone().expect("tree parent");
        let (minlen, flipped) = match g.edge(&v, &parent, None) {
            Some(e) => (e.minlen, false),
            None => (g.edge(&parent, &v, None).unwrap().minlen, true),
        };
        let pr = g.n(&parent).rank;
        g.n_mut(&v).rank = pr + if flipped { minlen } else { -minlen };
    }
}

// ---------------------------------------------------------------- ranks clean-up

fn inject_edge_label_proxies(g: &mut G, l: &mut Label) {
    for e in g.edges() {
        let ed = g.e(&e);
        if ed.width != 0.0 && !ed.width.is_nan() && ed.height != 0.0 && !ed.height.is_nan() {
            let vr = g.n(&e.v).rank.unwrap_or(f64::NAN);
            let wr = g.n(&e.w).rank.unwrap_or(f64::NAN);
            let attrs = NodeAttrs {
                rank: Some((wr - vr) / 2.0 + vr),
                e: Some(e.clone()),
                ..Default::default()
            };
            add_dummy_node(g, l, Dummy::EdgeProxy, attrs, "_ep");
        }
    }
}

/// lodash `_.min` over the defined, non-NaN ranks.
fn min_rank(g: &G) -> Option<f64> {
    g.nodes()
        .iter()
        .filter_map(|v| g.n(v).rank)
        .filter(|r| !r.is_nan())
        .fold(None, |m, r| {
            Some(m.map_or(r, |m: f64| if r < m { r } else { m }))
        })
}

fn remove_empty_ranks(g: &mut G, l: &Label) {
    let Some(offset) = min_rank(g) else { return };
    let mut layers: Vec<Option<Vec<String>>> = Vec::new();
    for v in g.nodes() {
        let Some(r) = g.n(&v).rank else { continue };
        let r = r - offset;
        if r < 0.0 || r.fract() != 0.0 || r.is_nan() {
            continue;
        }
        let r = r as usize;
        if layers.len() <= r {
            layers.resize(r + 1, None);
        }
        layers[r].get_or_insert_with(Vec::new).push(v);
    }
    let mut delta = 0.0;
    let factor = l.node_rank_factor;
    for (i, vs) in layers.iter().enumerate() {
        if vs.is_none() && (i as f64) % factor != 0.0 {
            delta -= 1.0;
        } else if delta != 0.0 {
            for v in vs.iter().flatten() {
                if let Some(r) = g.n_mut(v).rank.as_mut() {
                    *r += delta;
                }
            }
        }
    }
}

fn normalize_ranks(g: &mut G) {
    let Some(min) = min_rank(g) else { return };
    for v in g.nodes() {
        if let Some(r) = g.n_mut(&v).rank.as_mut() {
            *r -= min;
        }
    }
}

fn assign_rank_min_max(g: &mut G) {
    for v in g.nodes() {
        let n = g.n(&v);
        if let Some(t) = n.border_top.clone() {
            let b = n.border_bottom.clone().unwrap();
            let tr = g.n(&t).rank.unwrap_or(0.0) as i64;
            let br = g.n(&b).rank.unwrap_or(0.0) as i64;
            let n = g.n_mut(&v);
            n.min_rank = Some(tr);
            n.max_rank = Some(br);
        }
    }
}

fn remove_edge_label_proxies(g: &mut G) {
    for v in g.nodes() {
        let n = g.n(&v);
        if n.dummy == Some(Dummy::EdgeProxy) {
            let (e, r) = (n.e.clone().unwrap(), n.rank);
            if let Some(ed) = g.edge_mut(&e.v, &e.w, e.name.as_deref()) {
                ed.label_rank = r;
            }
            g.remove_node(&v);
        }
    }
}

// ---------------------------------------------------------------- normalize

fn normalize_run(g: &mut G, l: &mut Label) {
    l.dummy_chains.clear();
    for e in g.edges() {
        let mut v = e.v.clone();
        let mut v_rank = g.n(&v).rank.unwrap_or(f64::NAN);
        let w = e.w.clone();
        let w_rank = g.n(&w).rank.unwrap_or(f64::NAN);
        let label_rank = g.e(&e).label_rank;
        if w_rank == v_rank + 1.0 {
            continue;
        }
        let mut edge_label = g.remove_edge(&e).unwrap();
        edge_label.points.clear();
        let weight = edge_label.weight;
        let mut i = 0;
        v_rank += 1.0;
        let mut head: Option<String> = None;
        while v_rank < w_rank {
            let mut attrs = NodeAttrs {
                rank: Some(v_rank),
                edge_obj: Some(e.clone()),
                ..Default::default()
            };
            let mut kind = Dummy::Edge;
            if Some(v_rank) == label_rank {
                attrs.width = edge_label.width;
                attrs.height = edge_label.height;
                attrs.labelpos = Some(edge_label.labelpos);
                kind = Dummy::EdgeLabel;
            }
            let dummy = add_dummy_node(g, l, kind, attrs, "_d");
            g.set_edge(
                &v,
                &dummy,
                EdgeAttrs {
                    weight,
                    ..Default::default()
                },
                e.name.as_deref(),
            );
            if i == 0 {
                l.dummy_chains.push(dummy.clone());
                head = Some(dummy.clone());
            }
            v = dummy;
            i += 1;
            v_rank += 1.0;
        }
        g.set_edge(
            &v,
            &w,
            EdgeAttrs {
                weight,
                ..Default::default()
            },
            e.name.as_deref(),
        );
        if let Some(h) = head {
            g.n_mut(&h).edge_label = Some(Box::new(edge_label));
        }
    }
}

fn normalize_undo(g: &mut G, l: &Label) {
    for v0 in &l.dummy_chains {
        let Some(node) = g.node(v0) else { continue };
        let mut label = *node.edge_label.clone().unwrap();
        let obj = node.edge_obj.clone().unwrap();
        let mut v = v0.clone();
        loop {
            let node = g.n(&v).clone();
            if node.dummy.is_none() {
                break;
            }
            let w = g.successors(&v).into_iter().next();
            g.remove_node(&v);
            label.points.push(Pt {
                x: node.x,
                y: node.y,
            });
            if node.dummy == Some(Dummy::EdgeLabel) {
                label.x = Some(node.x);
                label.y = Some(node.y);
                label.width = node.width;
                label.height = node.height;
            }
            match w {
                Some(w) => v = w,
                None => break,
            }
        }
        g.set_edge(&obj.v, &obj.w, label, obj.name.as_deref());
    }
}

// ---------------------------------------------------------------- parent dummy chains

fn parent_dummy_chains(g: &mut G, l: &Label) {
    let mut post: HashMap<String, (usize, usize)> = HashMap::new();
    fn dfs(g: &G, v: &str, lim: &mut usize, post: &mut HashMap<String, (usize, usize)>) {
        let low = *lim;
        for c in g.children(Some(v)) {
            dfs(g, &c, lim, post);
        }
        post.insert(v.to_string(), (low, *lim));
        *lim += 1;
    }
    let mut lim = 0;
    for v in g.children(None) {
        dfs(g, &v, &mut lim, &mut post);
    }
    for head in &l.dummy_chains {
        let obj = g.n(head).edge_obj.clone().unwrap();
        // findPath
        let (pv, pw) = (post[&obj.v], post[&obj.w]);
        let low = pv.0.min(pw.0);
        let lim = pv.1.max(pw.1);
        let mut path: Vec<Option<String>> = Vec::new();
        let mut parent = Some(obj.v.clone());
        loop {
            parent = g.parent(parent.as_deref().unwrap());
            path.push(parent.clone());
            match &parent {
                Some(p) if post[p].0 > low || lim > post[p].1 => {}
                _ => break,
            }
        }
        let lca = parent;
        let mut wpath = Vec::new();
        let mut parent = Some(obj.w.clone());
        loop {
            parent = parent.as_deref().and_then(|p| g.parent(p));
            if parent == lca {
                break;
            }
            wpath.push(parent.clone());
            if parent.is_none() {
                break;
            }
        }
        wpath.reverse();
        path.extend(wpath);

        let mut idx = 0;
        let mut ascending = true;
        let mut v = head.clone();
        while v != obj.w {
            let rank = g.n(&v).rank.unwrap_or(f64::NAN);
            let mut path_v = path[idx].clone();
            if ascending {
                loop {
                    path_v = path[idx].clone();
                    if path_v == lca {
                        break;
                    }
                    let max = path_v
                        .as_deref()
                        .and_then(|p| g.n(p).max_rank)
                        .map_or(f64::NAN, |m| m as f64);
                    if max < rank {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                if path_v == lca {
                    ascending = false;
                }
            }
            if !ascending {
                while idx + 1 < path.len() {
                    let next = path[idx + 1].clone();
                    let min = next
                        .as_deref()
                        .and_then(|p| g.n(p).min_rank)
                        .map_or(f64::NAN, |m| m as f64);
                    if min <= rank {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                path_v = path[idx].clone();
            }
            g.set_parent(&v, path_v.as_deref());
            match g.successors(&v).into_iter().next() {
                Some(s) => v = s,
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------- border segments

fn add_border_segments(g: &mut G, l: &mut Label) {
    fn dfs(g: &mut G, l: &mut Label, v: &str) {
        for c in g.children(Some(v)) {
            dfs(g, l, &c);
        }
        let n = g.n(v);
        if let (Some(min), Some(max)) = (n.min_rank, n.max_rank) {
            let len = (max + 1).max(0) as usize;
            {
                let n = g.n_mut(v);
                n.border_left = vec![None; len];
                n.border_right = vec![None; len];
            }
            for rank in min..=max {
                for side in [BorderType::Left, BorderType::Right] {
                    let prefix = if side == BorderType::Left {
                        "_bl"
                    } else {
                        "_br"
                    };
                    let attrs = NodeAttrs {
                        rank: Some(rank as f64),
                        border_type: Some(side),
                        ..Default::default()
                    };
                    let n = g.n(v);
                    let list = if side == BorderType::Left {
                        &n.border_left
                    } else {
                        &n.border_right
                    };
                    let prev = if rank >= 1 {
                        list.get(rank as usize - 1).cloned().flatten()
                    } else {
                        None
                    };
                    let curr = add_dummy_node(g, l, Dummy::Border, attrs, prefix);
                    let n = g.n_mut(v);
                    let list = if side == BorderType::Left {
                        &mut n.border_left
                    } else {
                        &mut n.border_right
                    };
                    list[rank as usize] = Some(curr.clone());
                    g.set_parent(&curr, Some(v));
                    if let Some(p) = prev {
                        g.set_edge(
                            &p,
                            &curr,
                            EdgeAttrs {
                                weight: 1.0,
                                ..Default::default()
                            },
                            None,
                        );
                    }
                }
            }
        }
    }
    for v in g.children(None) {
        dfs(g, l, &v);
    }
}

// ---------------------------------------------------------------- order

fn max_rank(g: &G) -> Option<usize> {
    g.nodes()
        .iter()
        .filter_map(|v| g.n(v).rank)
        .filter(|r| !r.is_nan())
        .fold(None, |m: Option<f64>, r| Some(m.map_or(r, |m| m.max(r))))
        .map(|r| r as usize)
}

fn build_layer_matrix(g: &G, include: impl Fn(&str) -> bool) -> Vec<Vec<String>> {
    let Some(max) = max_rank(g) else {
        return Vec::new();
    };
    let mut layers: Vec<Vec<Option<String>>> = vec![Vec::new(); max + 1];
    for v in g.nodes() {
        if !include(&v) {
            continue;
        }
        let n = g.n(&v);
        if let (Some(r), Some(o)) = (n.rank, n.order) {
            let layer = &mut layers[r as usize];
            if layer.len() <= o {
                layer.resize(o + 1, None);
            }
            layer[o] = Some(v);
        }
    }
    layers
        .into_iter()
        .map(|l| l.into_iter().flatten().collect())
        .collect()
}

#[derive(Debug, Clone, Default)]
enum LgLabel {
    /// The layout graph's own label for the node (graphlib's default label).
    #[default]
    Shared,
    /// A subgraph's borders on this rank.
    Border {
        left: Option<String>,
        right: Option<String>,
    },
}

type LG = Graph<LgLabel, f64>;

struct LayerGraph {
    g: LG,
    root: String,
}

fn build_layer_graph(g: &G, l: &mut Label, rank: usize, inward: bool) -> LayerGraph {
    let root = loop {
        let v = l.unique_id("_root");
        if !g.has_node(&v) {
            break v;
        }
    };
    let mut res: LG = Graph::new(true, false, true);
    let r = rank as f64;
    for v in g.nodes() {
        let n = g.n(&v);
        let in_span = matches!((n.min_rank, n.max_rank), (Some(a), Some(b)) if a as f64 <= r && r <= b as f64);
        if n.rank == Some(r) || in_span {
            res.ensure_node(&v);
            let parent = g.parent(&v).unwrap_or_else(|| root.clone());
            res.set_parent(&v, Some(&parent));
            let edges = if inward {
                g.in_edges(&v)
            } else {
                g.out_edges(&v)
            };
            for e in edges {
                let u = if e.v == v { e.w.clone() } else { e.v.clone() };
                let w0 = res.edge(&u, &v, None).copied().unwrap_or(0.0);
                res.set_edge(&u, &v, g.e(&e).weight + w0, None);
            }
            if n.min_rank.is_some() {
                res.set_node(
                    &v,
                    LgLabel::Border {
                        left: n.border_left.get(rank).cloned().flatten(),
                        right: n.border_right.get(rank).cloned().flatten(),
                    },
                );
            }
        }
    }
    LayerGraph { g: res, root }
}

fn init_order(g: &G) -> Vec<Vec<String>> {
    let simple: Vec<String> = g
        .nodes()
        .into_iter()
        .filter(|v| !g.has_children(v))
        .collect();
    let max = simple
        .iter()
        .filter_map(|v| g.n(v).rank)
        .fold(None, |m: Option<f64>, r| Some(m.map_or(r, |m| m.max(r))));
    let Some(max) = max else {
        return Vec::new();
    };
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); max as usize + 1];
    let mut ordered = simple.clone();
    ordered.sort_by(|a, b| {
        let (ra, rb) = (
            g.n(a).rank.unwrap_or(f64::NAN),
            g.n(b).rank.unwrap_or(f64::NAN),
        );
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut visited = HashSet::new();
    // An explicit stack in the order a recursive DFS would visit.
    for start in ordered {
        let mut stack = vec![start];
        while let Some(v) = stack.pop() {
            if !visited.insert(v.clone()) {
                continue;
            }
            if let Some(r) = g.n(&v).rank {
                layers[r as usize].push(v.clone());
            }
            let mut sucs = g.successors(&v);
            sucs.reverse();
            stack.extend(sucs);
        }
    }
    layers
}

fn order(g: &mut G, l: &mut Label) {
    let Some(max) = max_rank(g) else { return };
    let down: Vec<LayerGraph> = (1..=max)
        .map(|r| build_layer_graph(g, l, r, true))
        .collect();
    let up: Vec<LayerGraph> = (0..max)
        .rev()
        .map(|r| build_layer_graph(g, l, r, false))
        .collect();
    let layering = init_order(g);
    assign_order(g, &layering);
    let mut best_cc = f64::INFINITY;
    let mut best: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    let mut last_best = 0;
    while last_best < 4 {
        sweep_layer_graphs(g, if i % 2 == 1 { &down } else { &up }, i % 4 >= 2);
        let layering = build_layer_matrix(g, |_| true);
        let cc = cross_count(g, &layering);
        if cc < best_cc {
            last_best = 0;
            best = layering;
            best_cc = cc;
        }
        i += 1;
        last_best += 1;
    }
    assign_order(g, &best);
}

fn assign_order(g: &mut G, layering: &[Vec<String>]) {
    for layer in layering {
        for (i, v) in layer.iter().enumerate() {
            g.n_mut(v).order = Some(i);
        }
    }
}

fn sweep_layer_graphs(g: &mut G, lgs: &[LayerGraph], bias_right: bool) {
    let mut cg: Graph<(), ()> = Graph::new(true, false, false);
    for lg in lgs {
        let sorted = sort_subgraph(g, &lg.g, &lg.root, &mut cg, bias_right);
        for (i, v) in sorted.vs.iter().enumerate() {
            if let Some(n) = g.node_mut(v) {
                n.order = Some(i);
            }
        }
        add_subgraph_constraints(&lg.g, &mut cg, &sorted.vs);
    }
}

#[derive(Debug, Clone)]
struct BcEntry {
    v: String,
    barycenter: Option<f64>,
    weight: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct Sorted {
    vs: Vec<String>,
    barycenter: Option<f64>,
    weight: Option<f64>,
}

fn order_of(g: &G, v: &str) -> f64 {
    g.node(v)
        .and_then(|n| n.order)
        .map_or(f64::NAN, |o| o as f64)
}

fn sort_subgraph(g: &G, lg: &LG, v: &str, cg: &mut Graph<(), ()>, bias_right: bool) -> Sorted {
    let mut movable = lg.children(Some(v));
    let (bl, br) = match lg.node(v) {
        Some(LgLabel::Border { left, right }) => (left.clone(), right.clone()),
        _ => (None, None),
    };
    if bl.is_some() {
        movable.retain(|w| Some(w) != bl.as_ref() && Some(w) != br.as_ref());
    }
    let mut barycenters: Vec<BcEntry> = movable
        .iter()
        .map(|m| {
            let in_v = lg.in_edges(m);
            if in_v.is_empty() {
                BcEntry {
                    v: m.clone(),
                    barycenter: None,
                    weight: None,
                }
            } else {
                let (mut sum, mut weight) = (0.0, 0.0);
                for e in in_v {
                    let w = *lg.e(&e);
                    sum += w * order_of(g, &e.v);
                    weight += w;
                }
                BcEntry {
                    v: m.clone(),
                    barycenter: Some(sum / weight),
                    weight: Some(weight),
                }
            }
        })
        .collect();
    let mut subgraphs: HashMap<String, Sorted> = HashMap::new();
    for entry in &mut barycenters {
        if lg.has_children(&entry.v) {
            let sub = sort_subgraph(g, lg, &entry.v.clone(), cg, bias_right);
            if let Some(sb) = sub.barycenter {
                let sw = sub.weight.unwrap_or(f64::NAN);
                match entry.barycenter {
                    Some(b) => {
                        let w = entry.weight.unwrap_or(f64::NAN);
                        entry.barycenter = Some((b * w + sb * sw) / (w + sw));
                        entry.weight = Some(w + sw);
                    }
                    None => {
                        entry.barycenter = Some(sb);
                        entry.weight = Some(sw);
                    }
                }
            }
            subgraphs.insert(entry.v.clone(), sub);
        }
    }
    let mut entries = resolve_conflicts(&barycenters, cg);
    for e in &mut entries {
        e.vs =
            e.vs.iter()
                .flat_map(|v| match subgraphs.get(v) {
                    Some(s) => s.vs.clone(),
                    None => vec![v.clone()],
                })
                .collect();
    }
    let mut result = sort(entries, bias_right);
    if let (Some(bl), Some(br)) = (bl, br) {
        let mut vs = vec![bl.clone()];
        vs.append(&mut result.vs);
        vs.push(br.clone());
        result.vs = vs;
        let bl_preds = lg.predecessors(&bl);
        if !bl_preds.is_empty() {
            let bl_pred = order_of(g, &bl_preds[0]);
            let br_pred = lg
                .predecessors(&br)
                .first()
                .map_or(f64::NAN, |p| order_of(g, p));
            if result.barycenter.is_none() {
                result.barycenter = Some(0.0);
                result.weight = Some(0.0);
            }
            let (b, w) = (
                result.barycenter.unwrap(),
                result.weight.unwrap_or(f64::NAN),
            );
            result.barycenter = Some((b * w + bl_pred + br_pred) / (w + 2.0));
            result.weight = Some(w + 2.0);
        }
    }
    result
}

#[derive(Debug, Clone)]
struct ResEntry {
    indegree: usize,
    ins: Vec<usize>,
    outs: Vec<usize>,
    vs: Vec<String>,
    i: usize,
    barycenter: Option<f64>,
    weight: Option<f64>,
    merged: bool,
}

#[derive(Debug, Clone)]
struct SortEntry {
    vs: Vec<String>,
    i: usize,
    barycenter: Option<f64>,
    weight: Option<f64>,
}

fn truthy(x: Option<f64>) -> bool {
    x.is_some_and(|x| x != 0.0 && !x.is_nan())
}

fn resolve_conflicts(entries: &[BcEntry], cg: &Graph<(), ()>) -> Vec<SortEntry> {
    let mut all: Vec<ResEntry> = Vec::new();
    let mut by_v: super::graph::OMap<usize> = super::graph::OMap::default();
    for (i, e) in entries.iter().enumerate() {
        let entry = ResEntry {
            indegree: 0,
            ins: Vec::new(),
            outs: Vec::new(),
            vs: vec![e.v.clone()],
            i,
            barycenter: e.barycenter,
            weight: if e.barycenter.is_some() {
                e.weight
            } else {
                None
            },
            merged: false,
        };
        // `mappedEntries[entry.v] = …` (a repeated key keeps its place).
        if let Some(&k) = by_v.get(&e.v) {
            all[k] = entry;
        } else {
            by_v.insert(&e.v, all.len());
            all.push(entry);
        }
    }
    for e in cg.edges() {
        if let (Some(&a), Some(&b)) = (by_v.get(&e.v), by_v.get(&e.w)) {
            all[b].indegree += 1;
            all[a].outs.push(b);
        }
    }
    let mut source_set: Vec<usize> = by_v
        .values()
        .into_iter()
        .copied()
        .filter(|&k| all[k].indegree == 0)
        .collect();
    let mut out: Vec<usize> = Vec::new();
    while let Some(v) = source_set.pop() {
        out.push(v);
        let mut ins = std::mem::take(&mut all[v].ins);
        ins.reverse();
        for &u in &ins {
            if all[u].merged {
                continue;
            }
            let (ub, vb) = (all[u].barycenter, all[v].barycenter);
            if ub.is_none() || vb.is_none() || ub.unwrap() >= vb.unwrap() {
                // mergeEntries(v, u)
                let (mut sum, mut weight) = (0.0, 0.0);
                if truthy(all[v].weight) {
                    sum += all[v].barycenter.unwrap_or(f64::NAN) * all[v].weight.unwrap();
                    weight += all[v].weight.unwrap();
                }
                if truthy(all[u].weight) {
                    sum += all[u].barycenter.unwrap_or(f64::NAN) * all[u].weight.unwrap();
                    weight += all[u].weight.unwrap();
                }
                let mut vs = all[u].vs.clone();
                vs.extend(all[v].vs.iter().cloned());
                all[v].vs = vs;
                all[v].barycenter = Some(sum / weight);
                all[v].weight = Some(weight);
                all[v].i = all[u].i.min(all[v].i);
                all[u].merged = true;
            }
        }
        all[v].ins = ins;
        for w in all[v].outs.clone() {
            all[w].ins.push(v);
            all[w].indegree -= 1;
            if all[w].indegree == 0 {
                source_set.push(w);
            }
        }
    }
    out.into_iter()
        .filter(|&k| !all[k].merged)
        .map(|k| SortEntry {
            vs: all[k].vs.clone(),
            i: all[k].i,
            barycenter: all[k].barycenter,
            weight: all[k].weight,
        })
        .collect()
}

/// V8's `Array.prototype.sort` for a comparator returning a number: TimSort,
/// which for fewer than 64 items is one run extended by binary insertion.
/// Matters where the comparator is inconsistent (NaN barycentres).
fn js_sort<T: Clone>(a: &mut [T], cmp: impl Fn(&T, &T) -> f64) {
    let n = a.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        a.sort_by(|x, y| {
            let c = cmp(x, y);
            if c < 0.0 {
                std::cmp::Ordering::Less
            } else if c > 0.0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        return;
    }
    // CountAndMakeRun
    let mut run = 2;
    let descending = cmp(&a[1], &a[0]) < 0.0;
    while run < n {
        let o = cmp(&a[run], &a[run - 1]);
        if descending {
            if o >= 0.0 || o.is_nan() {
                break;
            }
        } else if o < 0.0 {
            break;
        }
        run += 1;
    }
    if descending {
        a[..run].reverse();
    }
    // BinaryInsertionSort
    for start in run..n {
        let pivot = a[start].clone();
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if cmp(&pivot, &a[mid]) < 0.0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        a[left..=start].rotate_right(1);
    }
}

fn sort(entries: Vec<SortEntry>, bias_right: bool) -> Sorted {
    let (mut sortable, rest): (Vec<SortEntry>, Vec<SortEntry>) =
        entries.into_iter().partition(|e| e.barycenter.is_some());
    let mut unsortable = rest;
    // `_.sortBy(rhs, e => -e.i)`
    unsortable.sort_by_key(|e| std::cmp::Reverse(e.i));
    js_sort(&mut sortable, |a, b| {
        let (x, y) = (a.barycenter.unwrap(), b.barycenter.unwrap());
        if x < y {
            -1.0
        } else if x > y {
            1.0
        } else if bias_right {
            b.i as f64 - a.i as f64
        } else {
            a.i as f64 - b.i as f64
        }
    });
    let mut vs: Vec<String> = Vec::new();
    let (mut sum, mut weight) = (0.0, 0.0);
    let consume = |vs: &mut Vec<String>, uns: &mut Vec<SortEntry>, mut index: usize| {
        while let Some(last) = uns.last() {
            if last.i > index {
                break;
            }
            let last = uns.pop().unwrap();
            vs.extend(last.vs);
            index += 1;
        }
        index
    };
    let mut idx = consume(&mut vs, &mut unsortable, 0);
    for e in sortable {
        idx += e.vs.len();
        let (b, w) = (e.barycenter.unwrap(), e.weight.unwrap_or(f64::NAN));
        vs.extend(e.vs);
        sum += b * w;
        weight += w;
        idx = consume(&mut vs, &mut unsortable, idx);
    }
    let mut result = Sorted {
        vs,
        ..Default::default()
    };
    if weight != 0.0 && !weight.is_nan() {
        result.barycenter = Some(sum / weight);
        result.weight = Some(weight);
    }
    result
}

fn add_subgraph_constraints(lg: &LG, cg: &mut Graph<(), ()>, vs: &[String]) {
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut root_prev: Option<String> = None;
    'outer: for v in vs {
        let mut child = lg.parent(v);
        while let Some(c) = child {
            let parent = lg.parent(&c);
            let prev_child = match &parent {
                Some(p) => prev.insert(p.clone(), c.clone()),
                None => root_prev.replace(c.clone()),
            };
            if let Some(pc) = prev_child {
                if pc != c {
                    cg.ensure_edge(&pc, &c, None);
                    continue 'outer;
                }
            }
            child = parent;
        }
    }
}

fn cross_count(g: &G, layering: &[Vec<String>]) -> f64 {
    let mut cc = 0.0;
    for i in 1..layering.len() {
        let (north, south) = (&layering[i - 1], &layering[i]);
        let south_pos: HashMap<&str, usize> = south
            .iter()
            .enumerate()
            .map(|(i, v)| (v.as_str(), i))
            .collect();
        let mut entries: Vec<(usize, f64)> = Vec::new();
        for v in north {
            let mut es: Vec<(usize, f64)> = g
                .out_edges(v)
                .iter()
                .filter_map(|e| south_pos.get(e.w.as_str()).map(|&p| (p, g.e(e).weight)))
                .collect();
            es.sort_by_key(|e| e.0);
            entries.extend(es);
        }
        let mut first = 1;
        while first < south.len() {
            first <<= 1;
        }
        let size = 2 * first - 1;
        first -= 1;
        let mut tree = vec![0.0; size];
        for (pos, weight) in entries {
            let mut index = pos + first;
            tree[index] += weight;
            let mut ws = 0.0;
            while index > 0 {
                if index % 2 == 1 {
                    ws += tree[index + 1];
                }
                index = (index - 1) >> 1;
                tree[index] += weight;
            }
            cc += weight * ws;
        }
    }
    cc
}

fn insert_self_edges(g: &mut G, l: &mut Label) {
    let layers = build_layer_matrix(g, |_| true);
    for layer in layers {
        let mut shift = 0;
        for (i, v) in layer.iter().enumerate() {
            g.n_mut(v).order = Some(i + shift);
            let selfs = std::mem::take(&mut g.n_mut(v).self_edges);
            let rank = g.n(v).rank;
            for (e, label) in selfs {
                shift += 1;
                let attrs = NodeAttrs {
                    width: label.width,
                    height: label.height,
                    rank,
                    order: Some(i + shift),
                    e: Some(e),
                    label: Some(Box::new(label)),
                    ..Default::default()
                };
                add_dummy_node(g, l, Dummy::SelfEdge, attrs, "_se");
            }
        }
    }
}

fn swap_width_height(g: &mut G) {
    for v in g.nodes() {
        let n = g.n_mut(&v);
        std::mem::swap(&mut n.width, &mut n.height);
    }
    for e in g.edges() {
        let ed = g.e_mut(&e);
        std::mem::swap(&mut ed.width, &mut ed.height);
    }
}

// ---------------------------------------------------------------- position

fn position(g: &mut G, l: &Label) {
    let simple = |g: &G, v: &str| !g.has_children(v);
    // positionY
    let layering = build_layer_matrix(g, |v| simple(g, v));
    let mut prev_y = 0.0;
    for layer in &layering {
        let max_h = layer
            .iter()
            .map(|v| g.n(v).height)
            .fold(None, |m: Option<f64>, h| Some(m.map_or(h, |m| m.max(h))))
            .unwrap_or(f64::NAN);
        for v in layer {
            g.n_mut(v).y = prev_y + max_h / 2.0;
        }
        prev_y += max_h + l.ranksep;
    }
    let xs = position_x(g, l, &layering);
    for (v, x) in xs {
        g.n_mut(&v).x = x;
    }
}

type Conflicts = HashMap<String, HashSet<String>>;

fn add_conflict(c: &mut Conflicts, v: &str, w: &str) {
    let (v, w) = if v > w { (w, v) } else { (v, w) };
    c.entry(v.to_string()).or_default().insert(w.to_string());
}

fn has_conflict(c: &Conflicts, v: &str, w: &str) -> bool {
    let (v, w) = if v > w { (w, v) } else { (v, w) };
    c.get(v).is_some_and(|s| s.contains(w))
}

fn find_type1_conflicts(g: &G, layering: &[Vec<String>], c: &mut Conflicts) {
    for k in 1..layering.len() {
        let (prev, layer) = (&layering[k - 1], &layering[k]);
        let mut k0 = 0usize;
        let mut scan_pos = 0usize;
        let prev_len = prev.len();
        let last = layer.last();
        for (i, v) in layer.iter().enumerate() {
            let w = if g.n(v).dummy.is_some() {
                g.predecessors(v)
                    .into_iter()
                    .find(|u| g.n(u).dummy.is_some())
            } else {
                None
            };
            let k1 = w.as_ref().map_or(prev_len, |w| g.n(w).order.unwrap_or(0));
            if w.is_some() || Some(v) == last {
                for scan in &layer[scan_pos..=i] {
                    for u in g.predecessors(scan) {
                        let ul = g.n(&u);
                        let up = ul.order.unwrap_or(0);
                        if (up < k0 || k1 < up)
                            && !(ul.dummy.is_some() && g.n(scan).dummy.is_some())
                        {
                            add_conflict(c, &u, scan);
                        }
                    }
                }
                scan_pos = i + 1;
                k0 = k1;
            }
        }
    }
}

fn find_type2_conflicts(g: &G, layering: &[Vec<String>], c: &mut Conflicts) {
    let scan = |c: &mut Conflicts,
                south: &[String],
                from: usize,
                to: usize,
                prev_border: f64,
                next_border: f64| {
        for v in south.iter().take(to).skip(from) {
            if g.n(v).dummy.is_some() {
                for u in g.predecessors(v) {
                    let un = g.n(&u);
                    let o = un.order.map_or(f64::NAN, |o| o as f64);
                    if un.dummy.is_some() && (o < prev_border || o > next_border) {
                        add_conflict(c, &u, v);
                    }
                }
            }
        }
    };
    for k in 1..layering.len() {
        let (north, south) = (&layering[k - 1], &layering[k]);
        let mut prev_north = -1.0;
        let mut next_north = f64::NAN;
        let mut south_pos = 0;
        for (look, v) in south.iter().enumerate() {
            if g.n(v).dummy == Some(Dummy::Border) {
                let preds = g.predecessors(v);
                if let Some(p) = preds.first() {
                    next_north = g.n(p).order.map_or(f64::NAN, |o| o as f64);
                    scan(c, south, south_pos, look, prev_north, next_north);
                    south_pos = look;
                    prev_north = next_north;
                }
            }
            scan(
                c,
                south,
                south_pos,
                south.len(),
                next_north,
                north.len() as f64,
            );
        }
    }
}

fn position_x(g: &G, l: &Label, layering: &[Vec<String>]) -> Vec<(String, f64)> {
    let mut conflicts = Conflicts::new();
    find_type1_conflicts(g, layering, &mut conflicts);
    find_type2_conflicts(g, layering, &mut conflicts);
    let mut xss: Vec<HashMap<String, f64>> = Vec::new();
    for vert_up in [true, false] {
        let mut adjusted: Vec<Vec<String>> = if vert_up {
            layering.to_vec()
        } else {
            layering.iter().rev().cloned().collect()
        };
        for right in [false, true] {
            if right {
                adjusted = adjusted
                    .iter()
                    .map(|inner| inner.iter().rev().cloned().collect())
                    .collect();
            }
            let (root, align) = vertical_alignment(g, &adjusted, &conflicts, vert_up);
            let mut xs = horizontal_compaction(g, l, &adjusted, &root, &align, right);
            if right {
                for x in xs.values_mut() {
                    *x = -*x;
                }
            }
            xss.push(xs);
        }
    }
    // findSmallestWidthAlignment
    let width = |xs: &HashMap<String, f64>| {
        let (mut max, mut min) = (f64::NEG_INFINITY, f64::INFINITY);
        for (v, &x) in xs {
            let hw = g.n(v).width / 2.0;
            max = (x + hw).max(max);
            min = (x - hw).min(min);
        }
        max - min
    };
    let smallest = min_by(0..4, |&i| Some(width(&xss[i]))).unwrap_or(0);
    // alignCoordinates
    let vals = |xs: &HashMap<String, f64>| {
        let mut mn = f64::INFINITY;
        let mut mx = f64::NEG_INFINITY;
        for &x in xs.values() {
            mn = mn.min(x);
            mx = mx.max(x);
        }
        (mn, mx)
    };
    let (to_min, to_max) = vals(&xss[smallest]);
    for (i, xs) in xss.iter_mut().enumerate() {
        if i == smallest {
            continue;
        }
        let (mn, mx) = vals(xs);
        let delta = if i % 2 == 0 { to_min - mn } else { to_max - mx };
        if delta != 0.0 && !delta.is_nan() {
            for x in xs.values_mut() {
                *x += delta;
            }
        }
    }
    // balance (no `align`)
    let mut keys: Vec<&String> = xss[0].keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|v| {
            let mut xs = [xss[0][v], xss[1][v], xss[2][v], xss[3][v]];
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (v.clone(), (xs[1] + xs[2]) / 2.0)
        })
        .collect()
}

fn vertical_alignment(
    g: &G,
    layering: &[Vec<String>],
    conflicts: &Conflicts,
    up: bool,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut root: HashMap<String, String> = HashMap::new();
    let mut align: HashMap<String, String> = HashMap::new();
    let mut pos: HashMap<String, usize> = HashMap::new();
    for layer in layering {
        for (o, v) in layer.iter().enumerate() {
            root.insert(v.clone(), v.clone());
            align.insert(v.clone(), v.clone());
            pos.insert(v.clone(), o);
        }
    }
    for layer in layering {
        let mut prev_idx: i64 = -1;
        for v in layer {
            let mut ws = if up {
                g.predecessors(v)
            } else {
                g.successors(v)
            };
            ws.retain(|w| pos.contains_key(w));
            if ws.is_empty() {
                continue;
            }
            ws.sort_by_key(|w| pos[w]);
            let mp = (ws.len() as f64 - 1.0) / 2.0;
            for w in &ws[mp.floor() as usize..=mp.ceil() as usize] {
                if align[v] == *v && prev_idx < pos[w] as i64 && !has_conflict(conflicts, v, w) {
                    align.insert(w.clone(), v.clone());
                    let rw = root[w].clone();
                    align.insert(v.clone(), rw.clone());
                    root.insert(v.clone(), rw);
                    prev_idx = pos[w] as i64;
                }
            }
        }
    }
    (root, align)
}

fn sep(g: &G, l: &Label, v: &str, w: &str, reverse: bool) -> f64 {
    let (vl, wl) = (g.n(v), g.n(w));
    let mut sum = vl.width / 2.0;
    let delta = match vl.labelpos {
        Some('l') => -vl.width / 2.0,
        Some('r') => vl.width / 2.0,
        _ => 0.0,
    };
    if delta != 0.0 {
        sum += if reverse { delta } else { -delta };
    }
    sum += if vl.dummy.is_some() {
        l.opts.edgesep
    } else {
        l.opts.nodesep
    } / 2.0;
    sum += if wl.dummy.is_some() {
        l.opts.edgesep
    } else {
        l.opts.nodesep
    } / 2.0;
    sum += wl.width / 2.0;
    let delta = match wl.labelpos {
        Some('l') => wl.width / 2.0,
        Some('r') => -wl.width / 2.0,
        _ => 0.0,
    };
    if delta != 0.0 {
        sum += if reverse { delta } else { -delta };
    }
    sum
}

fn horizontal_compaction(
    g: &G,
    l: &Label,
    layering: &[Vec<String>],
    root: &HashMap<String, String>,
    align: &HashMap<String, String>,
    reverse: bool,
) -> HashMap<String, f64> {
    // buildBlockGraph
    let mut bg: Graph<(), f64> = Graph::new(true, false, false);
    for layer in layering {
        let mut u: Option<&String> = None;
        for v in layer {
            let vr = &root[v];
            bg.ensure_node(vr);
            if let Some(u) = u {
                let ur = &root[u];
                let prev = bg.edge(ur, vr, None).copied().unwrap_or(0.0);
                let s = sep(g, l, v, u, reverse);
                bg.set_edge(ur, vr, s.max(prev), None);
            }
            u = Some(v);
        }
    }
    let border = if reverse {
        BorderType::Left
    } else {
        BorderType::Right
    };
    let mut xs: HashMap<String, f64> = HashMap::new();
    // pass 1: smallest coordinates, predecessors first
    let mut stack = bg.nodes();
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(elem) = stack.pop() {
        if visited.contains(&elem) {
            let x = bg.in_edges(&elem).iter().fold(0.0, |acc: f64, e| {
                acc.max(xs.get(&e.v).copied().unwrap_or(f64::NAN) + bg.e(e))
            });
            xs.insert(elem, x);
        } else {
            visited.insert(elem.clone());
            stack.push(elem.clone());
            stack.extend(bg.predecessors(&elem));
        }
    }
    // pass 2: greatest coordinates, successors first
    let mut stack = bg.nodes();
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(elem) = stack.pop() {
        if visited.contains(&elem) {
            let min = bg
                .out_edges(&elem)
                .iter()
                .fold(f64::INFINITY, |acc: f64, e| {
                    acc.min(xs.get(&e.w).copied().unwrap_or(f64::NAN) - bg.e(e))
                });
            if min != f64::INFINITY && g.n(&elem).border_type != Some(border) {
                let cur = xs[&elem];
                xs.insert(elem, cur.max(min));
            }
        } else {
            visited.insert(elem.clone());
            stack.push(elem.clone());
            stack.extend(bg.successors(&elem));
        }
    }
    let mut out = HashMap::new();
    for v in align.values() {
        out.insert(v.clone(), xs[&root[v]]);
    }
    out
}

fn position_self_edges(g: &mut G) {
    for v in g.nodes() {
        let n = g.n(&v);
        if n.dummy != Some(Dummy::SelfEdge) {
            continue;
        }
        let n = n.clone();
        let e = n.e.clone().unwrap();
        let sn = g.n(&e.v);
        let x = sn.x + sn.width / 2.0;
        let y = sn.y;
        let dx = n.x - x;
        let dy = sn.height / 2.0;
        let mut label = *n.label.unwrap();
        label.points = vec![
            Pt {
                x: x + 2.0 * dx / 3.0,
                y: y - dy,
            },
            Pt {
                x: x + 5.0 * dx / 6.0,
                y: y - dy,
            },
            Pt { x: x + dx, y },
            Pt {
                x: x + 5.0 * dx / 6.0,
                y: y + dy,
            },
            Pt {
                x: x + 2.0 * dx / 3.0,
                y: y + dy,
            },
        ];
        label.x = Some(n.x);
        label.y = Some(n.y);
        g.set_edge(&e.v, &e.w, label, e.name.as_deref());
        g.remove_node(&v);
    }
}

fn remove_border_nodes(g: &mut G) {
    for v in g.nodes() {
        if !g.has_children(&v) {
            continue;
        }
        let n = g.n(&v);
        let get = |k: &Option<String>| k.as_deref().and_then(|k| g.node(k)).map(|n| (n.x, n.y));
        let t = get(&n.border_top);
        let b = get(&n.border_bottom);
        let lft = get(&n.border_left.last().cloned().flatten());
        let rgt = get(&n.border_right.last().cloned().flatten());
        if let (Some(t), Some(b), Some(lft), Some(rgt)) = (t, b, lft, rgt) {
            let n = g.n_mut(&v);
            n.width = (rgt.0 - lft.0).abs();
            n.height = (b.1 - t.1).abs();
            n.x = lft.0 + n.width / 2.0;
            n.y = t.1 + n.height / 2.0;
        }
    }
    for v in g.nodes() {
        if g.n(&v).dummy == Some(Dummy::Border) {
            g.remove_node(&v);
        }
    }
}

/// `translateGraph`: everything moved inside the margins (the graph's own
/// width and height, which dagre also records, are not needed here).
fn translate_graph(g: &mut G, l: &Label) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) =
        (f64::INFINITY, 0.0f64, f64::INFINITY, 0.0f64);
    let mut ext = |x: f64, y: f64, w: f64, h: f64| {
        min_x = min_x.min(x - w / 2.0);
        max_x = max_x.max(x + w / 2.0);
        min_y = min_y.min(y - h / 2.0);
        max_y = max_y.max(y + h / 2.0);
    };
    for v in g.nodes() {
        let n = g.n(&v);
        ext(n.x, n.y, n.width, n.height);
    }
    for e in g.edges() {
        let ed = g.e(&e);
        if let (Some(x), Some(y)) = (ed.x, ed.y) {
            ext(x, y, ed.width, ed.height);
        }
    }
    min_x -= l.opts.marginx;
    min_y -= l.opts.marginy;
    for v in g.nodes() {
        let n = g.n_mut(&v);
        n.x -= min_x;
        n.y -= min_y;
    }
    for e in g.edges() {
        let ed = g.e_mut(&e);
        for p in &mut ed.points {
            p.x -= min_x;
            p.y -= min_y;
        }
        if let Some(x) = ed.x.as_mut() {
            *x -= min_x;
        }
        if let Some(y) = ed.y.as_mut() {
            *y -= min_y;
        }
    }
}

fn intersect_rect(n: &NodeAttrs, p: Pt) -> Pt {
    let (x, y) = (n.x, n.y);
    let (dx, dy) = (p.x - x, p.y - y);
    let (mut w, mut h) = (n.width / 2.0, n.height / 2.0);
    if dx == 0.0 && dy == 0.0 {
        return Pt { x, y };
    }
    let (sx, sy);
    if dy.abs() * w > dx.abs() * h {
        if dy < 0.0 {
            h = -h;
        }
        sx = h * dx / dy;
        sy = h;
    } else {
        if dx < 0.0 {
            w = -w;
        }
        sx = w;
        sy = w * dy / dx;
    }
    Pt {
        x: x + sx,
        y: y + sy,
    }
}

fn assign_node_intersects(g: &mut G) {
    for e in g.edges() {
        let nv = g.n(&e.v).clone();
        let nw = g.n(&e.w).clone();
        let ed = g.e_mut(&e);
        let (p1, p2) = if ed.points.is_empty() {
            (Pt { x: nw.x, y: nw.y }, Pt { x: nv.x, y: nv.y })
        } else {
            (ed.points[0], *ed.points.last().unwrap())
        };
        ed.points.insert(0, intersect_rect(&nv, p1));
        ed.points.push(intersect_rect(&nw, p2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(rankdir: RankDir) -> Opts {
        Opts {
            rankdir,
            nodesep: 50.0,
            ranksep: 50.0,
            edgesep: 20.0,
            marginx: 8.0,
            marginy: 8.0,
        }
    }

    #[test]
    fn chain_is_stacked_ranksep_apart() {
        let mut g: Graph<InNode, InEdge> = Graph::new(true, true, true);
        for v in ["a", "b", "c"] {
            g.set_node(
                v,
                InNode {
                    width: 40.0,
                    height: 20.0,
                },
            );
        }
        g.set_edge("a", "b", InEdge::default(), Some("e1"));
        g.set_edge("b", "c", InEdge::default(), Some("e2"));
        let out = layout(&g, opts(RankDir::TB));
        let (a, b, c) = (out.nodes["a"], out.nodes["b"], out.nodes["c"]);
        assert_eq!((a.x, a.y), (28.0, 18.0));
        assert_eq!(b.y - a.y, 70.0);
        assert_eq!(c.y - b.y, 70.0);
        assert_eq!(out.edges.len(), 2);
    }

    #[test]
    fn fan_out_is_centred_under_its_source() {
        let mut g: Graph<InNode, InEdge> = Graph::new(true, true, true);
        for v in ["a", "b", "c"] {
            g.set_node(
                v,
                InNode {
                    width: 40.0,
                    height: 20.0,
                },
            );
        }
        g.set_edge("a", "b", InEdge::default(), Some("e1"));
        g.set_edge("a", "c", InEdge::default(), Some("e2"));
        let out = layout(&g, opts(RankDir::TB));
        let (a, b, c) = (out.nodes["a"], out.nodes["b"], out.nodes["c"]);
        assert_eq!(c.x - b.x, 90.0);
        assert_eq!(a.x, (b.x + c.x) / 2.0);
    }

    #[test]
    fn js_sort_matches_v8_on_an_inconsistent_comparator() {
        let mut v = vec![3, 1, 2];
        js_sort(&mut v, |a, b| (*a - *b) as f64);
        assert_eq!(v, [1, 2, 3]);
    }
}
