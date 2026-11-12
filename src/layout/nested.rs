//! Clusters laid out on their own, the way mermaid's (default) ELK layout treats
//! subgraphs, composite states and namespaces.
//!
//! mermaid hands ELK every container as a hierarchical node. A container keeps its
//! own layout (`elk.hierarchyHandling: SEPARATE_CHILDREN`, so its own `direction`
//! and its own, tighter spacing) unless an edge crosses its border: for every edge
//! whose two ends have different parents, the containers from each end up to (and
//! including) their common ancestor are switched to `INCLUDE_CHILDREN` and laid out
//! together with their parent (`configureCrossHierarchyEdges` /
//! `setIncludeChildrenPolicy` in `layout-algorithms/elk/render.ts`).
//!
//! Here that becomes: every "separate" cluster is laid out first, bottom-up, as an
//! independent layered graph in its own direction; its result is a box (content +
//! padding + title band) that the parent level places like any other node. The
//! remaining ("integrated") clusters stay clusters of the parent's layered pass.
//!
//! Under ELK every populated cluster is such a level; the levels of one ELK run
//! (the root, or a container with a layout of its own, and the containers laid
//! out within it) are all prepared first and ordered together
//! (`order_runs`), then placed bottom-up. A container of a run has a port on its
//! border for every edge that crosses it or ends at it, with a port dummy inside.

use super::{
    Direction, EdgeRoute, EdgeRouting, ElkGraph, ElkLayers, Hierarchy, LayoutCluster, LayoutEdge,
    LayoutInput, LayoutNode, LayoutOutput, OrderCtx, OrthoSpacing, Prepared, cluster_chains,
    elk_options, elkorder, finish, layout_flat, layout_flat_in, prepare,
};
use crate::geom::{Point, Rect};
use std::collections::HashMap;

/// Something with a parent in the cluster tree: a node or a cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    Node(usize),
    Cluster(usize),
}

/// A vertex of one level's layered pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Node(usize),
    /// A separate cluster, already laid out, placed as one box.
    Box(usize),
    /// The external port of this level's container where edge `.0` crosses its
    /// border (ELK: an external port dummy).
    Ext(usize),
}

/// A level and what stands for an edge end there.
type LevelEnd = (Option<usize>, SegEnd);

/// One end of an edge segment at one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegEnd {
    Node(usize),
    Box(usize),
    /// The level's own border.
    Ext,
}

/// The part of an edge laid out at one level (ELK splits an edge crossing
/// container borders into one segment per graph of its hierarchy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seg {
    edge: usize,
    scope: Option<usize>,
    from: SegEnd,
    to: SegEnd,
    /// The segment between the two ends' common ancestor's children, which holds
    /// the label.
    main: bool,
}

/// What an edge end attaches to at one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Item(usize),
    /// A whole integrated cluster (flat cluster index), drawn from a representative item.
    Cluster {
        flat: usize,
        rep: usize,
    },
}

struct Tree<'a> {
    input: &'a LayoutInput,
    chains: Vec<Vec<usize>>,
    parent: Vec<Option<usize>>,
    separate: Vec<bool>,
    /// Under ELK every populated cluster is laid out as a graph of its own; one an
    /// edge crosses (`INCLUDE_CHILDREN`) takes its parent's direction.
    crossed: Vec<bool>,
}

impl Tree<'_> {
    fn parent_of(&self, m: Member) -> Option<usize> {
        match m {
            Member::Node(n) => self.input.nodes[n].cluster,
            Member::Cluster(c) => self.parent[c],
        }
    }

    /// mermaid's `findCommonAncestor` (`None` is the root).
    fn common_ancestor(&self, a: Member, b: Member) -> Option<usize> {
        if a == b {
            return self.parent_of(a);
        }
        let up = |m: Member| -> Vec<Member> {
            let mut out = vec![m];
            let mut cur = self.parent_of(m);
            while let Some(c) = cur {
                if out.len() > self.parent.len() + 1 {
                    break;
                }
                out.push(Member::Cluster(c));
                cur = self.parent[c];
            }
            out
        };
        let (ua, ub) = (up(a), up(b));
        if ua.contains(&b) {
            return match b {
                Member::Cluster(c) => Some(c),
                Member::Node(n) => self.input.nodes[n].cluster,
            };
        }
        ub.iter().find(|m| ua.contains(m)).and_then(|m| match m {
            Member::Cluster(c) => Some(*c),
            Member::Node(_) => None,
        })
    }

    /// mermaid's `setIncludeChildrenPolicy`: `m` and its ancestors up to `anc` join their
    /// parent's layout.
    fn integrate(&mut self, m: Member, anc: Option<usize>) {
        let mut cur = match m {
            Member::Cluster(c) => Some(c),
            Member::Node(_) => self.parent_of(m),
        };
        let mut guard = 0;
        while let Some(c) = cur {
            self.separate[c] = false;
            guard += 1;
            if Some(c) == anc || guard > self.parent.len() {
                break;
            }
            cur = self.parent[c];
        }
    }

    /// Whether cluster `c` is laid out within its parent's ELK run
    /// (`INCLUDE_CHILDREN`, inherited from the root), not as a run of its own
    /// (`SEPARATE_CHILDREN`: a cluster with its own direction no edge crosses, and
    /// everything inside one).
    fn hierarchical(&self, c: usize) -> bool {
        let mut cur = Some(c);
        for _ in 0..=self.parent.len() {
            let Some(a) = cur else { return true };
            if self.crossed[a] {
                return true;
            }
            if self.input.clusters[a].direction.is_some() {
                return false;
            }
            cur = self.parent[a];
        }
        true
    }

    /// Nearest separate cluster strictly above `c` (`None` = root).
    fn owner_of_cluster(&self, c: usize) -> Option<usize> {
        self.chains[c][..self.chains[c].len() - 1]
            .iter()
            .rev()
            .copied()
            .find(|&a| self.separate[a])
    }

    /// Nearest separate cluster containing node `n` (`None` = root).
    fn owner_of_node(&self, n: usize) -> Option<usize> {
        let c = self.input.nodes[n].cluster?;
        self.chains[c]
            .iter()
            .rev()
            .copied()
            .find(|&a| self.separate[a])
    }

    /// The box at level `scope` that contains cluster `c` (inclusive), if any.
    fn box_at(&self, c: usize, scope: Option<usize>) -> Option<usize> {
        self.chains[c]
            .iter()
            .rev()
            .copied()
            .find(|&a| self.separate[a] && Some(a) != scope && self.owner_of_cluster(a) == scope)
    }

    /// A cluster's own direction. Without one it is laid out with the root's, unless
    /// it sits inside a cluster that has a direction of its own: that container is an
    /// `elk.hierarchyHandling: SEPARATE_CHILDREN` layout run, whose child containers
    /// are separate runs too, and `elk.direction` is not inherited, so ELK's layered
    /// default (`RIGHT`) applies there (`flowchart.inheritDir` sets it explicitly).
    fn direction(&self, scope: Option<usize>) -> Direction {
        let Some(c) = scope else {
            return self.input.direction;
        };
        if self.crossed[c] {
            return self.direction(self.owner_of_cluster(c));
        }
        if let Some(d) = self.input.clusters[c].direction {
            return d;
        }
        let mut owner = self.owner_of_cluster(c);
        for _ in 0..=self.parent.len() {
            let Some(o) = owner else { break };
            if self.input.clusters[o].direction.is_some() {
                return Direction::LeftRight;
            }
            owner = self.owner_of_cluster(o);
        }
        self.input.direction
    }
}

/// Result of one level's layered pass, in that level's own coordinates.
struct Level {
    items: Vec<Item>,
    out: LayoutOutput,
    /// Flat cluster index → input cluster.
    flat_clusters: Vec<usize>,
    /// Edges routed at this level (input indices), in flat order.
    edge_ids: Vec<usize>,
    /// Under ELK, per routed edge: the segment it is.
    segs: Vec<Option<Seg>>,
    /// Per external port: (item, entering, unconnected inside).
    ext_kinds: Vec<(usize, bool, bool)>,
    bbox: Rect,
}

pub(super) fn layout(input: &LayoutInput) -> LayoutOutput {
    if !matches!(input.routing, EdgeRouting::Orthogonal(_)) || input.clusters.is_empty() {
        return layout_tree(input);
    }
    // mermaid's ELK adapter (`elk.orientFeedbackEdges`, on by default) turns round
    // the feedback edges between whole clusters before ELK sees them; they are laid
    // out the other way and drawn back from their own source.
    let flags = subgraph_feedback_edges(input);
    let mut turned = input.clone();
    for (e, _) in turned.edges.iter_mut().zip(&flags).filter(|(_, f)| **f) {
        std::mem::swap(&mut e.from, &mut e.to);
        std::mem::swap(&mut e.from_cluster, &mut e.to_cluster);
    }
    // Then it hands ELK the edges in `orderNestedEdgeEnds`'s order.
    let order = nested_edge_order(&turned);
    if order.iter().enumerate().any(|(i, &k)| i != k) {
        turned.edges = order.iter().map(|&k| turned.edges[k].clone()).collect();
    }
    let laid = layout_tree(&turned);
    let mut edges = laid.edges.clone();
    for (route, &k) in laid.edges.into_iter().zip(&order) {
        edges[k] = route;
    }
    for (r, _) in edges.iter_mut().zip(&flags).filter(|(_, f)| **f) {
        r.points.reverse();
    }
    let mut out = LayoutOutput { edges, ..laid };
    even_group_frames(input, &mut out);
    out
}

/// mermaid's `evenGroupFrames`, after ELK: deepest first, a container's frame
/// is pulled in (never pushed out) to its padding from what it holds (its
/// children's boxes, the routes of edges with both ends inside it and the
/// ends of edges on it) on the left, the right and the bottom; the top keeps
/// its title band. ELK sizes a container around more (lanes of edges that
/// leave it, the room around its port dummies), which stays laid out but
/// unframed.
fn even_group_frames(input: &LayoutInput, out: &mut LayoutOutput) {
    let nc = input.clusters.len();
    let chains = cluster_chains(&input.clusters);
    let pad = input.cluster_padding;
    let inside = |cluster: Option<usize>, c: usize| {
        cluster.is_some_and(|k| chains.get(k).is_some_and(|ch| ch.contains(&c)))
    };
    let mut order: Vec<usize> = (0..nc).collect();
    order.sort_by_key(|&c| std::cmp::Reverse(chains[c].len()));
    for c in order {
        let frame = out.clusters[c];
        let mut xs: Vec<f64> = Vec::new();
        let mut ys: Vec<f64> = Vec::new();
        let mut boxes = 0;
        for (v, nd) in input.nodes.iter().enumerate() {
            if nd.cluster == Some(c) && nd.width > 0.0 && nd.height > 0.0 {
                let r = Rect::centered(out.nodes[v], nd.width, nd.height);
                xs.extend([r.x, r.right()]);
                ys.extend([r.y, r.bottom()]);
                boxes += 1;
            }
        }
        for (k, cl) in input.clusters.iter().enumerate() {
            let r = out.clusters[k];
            if cl.parent == Some(c) && k != c && r.w > 0.0 && r.h > 0.0 {
                xs.extend([r.x, r.right()]);
                ys.extend([r.y, r.bottom()]);
                boxes += 1;
            }
        }
        if boxes == 0 || frame.w <= 0.0 || frame.h <= 0.0 {
            continue;
        }
        for (e, route) in input.edges.iter().zip(&out.edges) {
            let end_in = |node: usize, whole: Option<usize>| match whole {
                Some(k) => k != c && inside(Some(k), c),
                None => inside(input.nodes[node].cluster, c),
            };
            let (first, last) = (route.points.first(), route.points.last());
            let points: Vec<Point> = if end_in(e.from, e.from_cluster) && end_in(e.to, e.to_cluster)
            {
                route.points.clone()
            } else {
                let start = first.filter(|_| e.from_cluster == Some(c));
                let end = last.filter(|_| e.to_cluster == Some(c));
                start.into_iter().chain(end).copied().collect()
            };
            xs.extend(points.iter().map(|p| p.x));
            ys.extend(points.iter().map(|p| p.y));
        }
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let left = frame.x.max(min(&xs) - pad);
        let right = frame.right().min(max(&xs) + pad);
        let bottom = frame.bottom().min(max(&ys) + pad);
        let (mut x, mut w) = (left, right - left);
        let floor = input.clusters[c].min_width;
        if w < floor {
            x -= (floor - w) / 2.0;
            w = floor;
            x = frame.x.max(x.min(frame.right() - w));
            w = w.min(frame.w);
        }
        let h = bottom - frame.y;
        if h > 0.0 && w > 0.0 {
            out.clusters[c] = Rect::new(x, frame.y, w, h);
        }
    }
}

/// mermaid's `orderNestedEdgeEnds`: an edge moves in front of the first edge
/// before it that leaves a cluster around its source for the same target, or
/// that enters a cluster around its target from the same source (so that ELK's
/// model order does not cross the two). The edge indices in their new order.
fn nested_edge_order(input: &LayoutInput) -> Vec<usize> {
    let chains = cluster_chains(&input.clusters);
    let end = |e: &LayoutEdge, source: bool| -> Member {
        match source {
            true => e.from_cluster.map_or(Member::Node(e.from), Member::Cluster),
            false => e.to_cluster.map_or(Member::Node(e.to), Member::Cluster),
        }
    };
    // Whether `m` sits inside cluster member `anc`, at any depth.
    let inside = |m: Member, anc: Member| -> bool {
        let Member::Cluster(a) = anc else {
            return false;
        };
        let parent = match m {
            Member::Node(v) => input.nodes[v].cluster,
            Member::Cluster(c) => input.clusters[c].parent,
        };
        parent.is_some_and(|p| chains.get(p).is_some_and(|ch| ch.contains(&a)))
    };
    let edges = &input.edges;
    let mut order: Vec<usize> = (0..edges.len()).collect();
    for (index, e) in edges.iter().enumerate() {
        let (source, target) = (end(e, true), end(e, false));
        let Some(from) = order.iter().position(|&k| k == index) else {
            continue;
        };
        let to = order[..from].iter().position(|&other| {
            let earlier = &edges[other];
            (end(earlier, false) == target && inside(source, end(earlier, true)))
                || (end(earlier, true) == source && inside(target, end(earlier, false)))
        });
        if let Some(to) = to {
            order.remove(from);
            order.insert(to, index);
        }
    }
    order
}

/// mermaid's `findSubgraphFeedbackEdges`: per common ancestor, the edges between its
/// children (each end lifted to the child of the ancestor containing it) form a
/// graph; a depth-first search from its sources first, then from every vertex, in
/// order of first appearance, finds the edges closing a cycle. Such an edge is
/// flagged if an end was lifted, i.e. it attaches to (or inside) a cluster.
fn subgraph_feedback_edges(input: &LayoutInput) -> Vec<bool> {
    let chains = cluster_chains(&input.clusters);
    let mut flags = vec![false; input.edges.len()];
    // The ancestor chain of a member, itself first, up to (not including) the root.
    let up = |m: Member| -> Vec<Member> {
        let mut out = vec![m];
        let cluster = match m {
            Member::Node(n) => input.nodes[n].cluster,
            Member::Cluster(c) => input.clusters[c].parent,
        };
        if let Some(c) = cluster {
            out.extend(chains[c].iter().rev().map(|&a| Member::Cluster(a)));
        }
        out
    };
    // (ancestor, [(edge, from, to, collapsed)]) in order of first appearance.
    type Level = (Option<usize>, Vec<(usize, Member, Member, bool)>);
    let mut levels: Vec<Level> = Vec::new();
    for (ei, e) in input.edges.iter().enumerate() {
        let src = e.from_cluster.map_or(Member::Node(e.from), Member::Cluster);
        let dst = e.to_cluster.map_or(Member::Node(e.to), Member::Cluster);
        if src == dst {
            continue;
        }
        let (us, ud) = (up(src), up(dst));
        // `findCommonAncestor`: the first of the target's chain on the source's.
        let anc = ud.iter().find(|m| us.contains(m)).copied();
        if anc == Some(src) || anc == Some(dst) {
            continue;
        }
        let anc = anc.and_then(|m| match m {
            Member::Cluster(c) => Some(c),
            Member::Node(_) => None,
        });
        let below = |chain: &[Member]| -> Member {
            let k = match anc {
                Some(a) => chain
                    .iter()
                    .position(|&m| m == Member::Cluster(a))
                    .unwrap_or(chain.len()),
                None => chain.len(),
            };
            chain[k.saturating_sub(1)]
        };
        let (from, to) = (below(&us), below(&ud));
        let edge = (ei, from, to, from != src || to != dst);
        match levels.iter_mut().find(|(a, _)| *a == anc) {
            Some((_, list)) => list.push(edge),
            None => levels.push((anc, vec![edge])),
        }
    }
    for (_, level) in &levels {
        let mut nodes: Vec<Member> = Vec::new();
        let mut indeg: Vec<usize> = Vec::new();
        for &(_, f, t, _) in level {
            for m in [f, t] {
                if !nodes.contains(&m) {
                    nodes.push(m);
                    indeg.push(0);
                }
            }
            let k = nodes.iter().position(|&m| m == t).unwrap_or(0);
            indeg[k] += 1;
        }
        let idx = |m: Member| nodes.iter().position(|&x| x == m).unwrap_or(0);
        let succ: Vec<Vec<usize>> = (0..nodes.len())
            .map(|k| (0..level.len()).filter(|&i| idx(level[i].1) == k).collect())
            .collect();
        // 0 unseen, 1 on the stack, 2 done.
        let mut state = vec![0u8; nodes.len()];
        let roots = (0..nodes.len())
            .filter(|&k| indeg[k] == 0)
            .chain(0..nodes.len());
        for root in roots.collect::<Vec<_>>() {
            if state[root] != 0 {
                continue;
            }
            state[root] = 1;
            let mut stack = vec![(root, 0usize)];
            while let Some(&mut (v, ref mut next)) = stack.last_mut() {
                let Some(&i) = succ[v].get(*next) else {
                    state[v] = 2;
                    stack.pop();
                    continue;
                };
                *next += 1;
                let (ei, _, to, collapsed) = level[i];
                let w = idx(to);
                match state[w] {
                    1 => flags[ei] = collapsed,
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    _ => {}
                }
            }
        }
    }
    flags
}

/// The layout proper (see the module documentation).
fn layout_tree(input: &LayoutInput) -> LayoutOutput {
    let nc = input.clusters.len();
    if nc == 0 {
        return layout_flat(input);
    }
    let chains = cluster_chains(&input.clusters);
    let parent: Vec<Option<usize>> = chains
        .iter()
        .map(|ch| (ch.len() >= 2).then(|| ch[ch.len() - 2]))
        .collect();
    let mut populated = vec![false; nc];
    for nd in &input.nodes {
        if let Some(c) = nd.cluster.filter(|&c| c < nc) {
            for &a in &chains[c] {
                populated[a] = true;
            }
        }
    }
    let mut tree = Tree {
        input,
        chains,
        parent,
        separate: populated.clone(),
        crossed: vec![false; nc],
    };
    let ends: Vec<(Member, Member)> = input
        .edges
        .iter()
        .map(|e| {
            (
                e.from_cluster.map_or(Member::Node(e.from), Member::Cluster),
                e.to_cluster.map_or(Member::Node(e.to), Member::Cluster),
            )
        })
        .collect();
    for &(a, b) in &ends {
        if tree.parent_of(a) != tree.parent_of(b) {
            let anc = tree.common_ancestor(a, b);
            tree.integrate(a, anc);
            tree.integrate(b, anc);
        }
    }
    // Under ELK every cluster is a graph of its own (see `segments`).
    let elk = matches!(input.routing, EdgeRouting::Orthogonal(_));
    if elk {
        for (c, &p) in populated.iter().enumerate() {
            tree.crossed[c] = p && !tree.separate[c];
        }
        tree.separate = populated;
    }
    if !tree.separate.iter().any(|&s| s) {
        return layout_flat(input);
    }
    let segs = elk.then(|| segments(&tree, &ends));
    // ELK gives a container of a hierarchical run an external port dummy for every
    // port on its border, an edge to the container itself included: that one is
    // unconnected inside, but takes its place among the others
    // (`CompoundGraphPreprocessor`). (edge, container, entering)
    let mut stubs: Vec<(usize, usize, bool)> = Vec::new();
    if elk {
        for (ei, &(a, b)) in ends.iter().enumerate() {
            if a == b {
                continue;
            }
            for (m, entering) in [(a, false), (b, true)] {
                if let Member::Cluster(c) = m
                    && tree.separate[c]
                    && tree.hierarchical(c)
                    && tree.common_ancestor(a, b) != Some(c)
                {
                    stubs.push((ei, c, entering));
                }
            }
        }
    }

    // Deepest separate clusters first, the root last.
    let mut scopes: Vec<Option<usize>> = (0..nc).filter(|&c| tree.separate[c]).map(Some).collect();
    scopes.sort_by_key(|s| std::cmp::Reverse(s.map_or(0, |c| tree.chains[c].len())));
    scopes.push(None);

    let pad = input.cluster_padding;
    let mut box_size: Vec<(f64, f64)> = vec![(0.0, 0.0); nc];
    let mut levels: Vec<Option<Level>> = (0..=nc).map(|_| None).collect();
    let slot = |s: Option<usize>| s.unwrap_or(nc);
    // Per (container, edge): the cross-axis offset of the edge's external port on
    // the container's box from the box's centre.
    let mut ext_off: HashMap<(usize, usize), f64> = HashMap::new();
    // Under ELK the graphs of one run are ordered together (see `order_runs`)
    // before any is placed.
    let mut ordered: Vec<Option<RunLevel>> = (0..=nc).map(|_| None).collect();
    if elk {
        let inputs: Vec<LevelInput> = scopes
            .iter()
            .map(|&scope| {
                level_input(
                    &tree,
                    &ends,
                    scope,
                    &box_size,
                    segs.as_deref(),
                    &stubs,
                    &ext_off,
                )
            })
            .collect();
        for (&scope, done) in scopes.iter().zip(order_runs(&tree, &scopes, &inputs)) {
            ordered[slot(scope)] = Some(done);
        }
    }
    for &scope in &scopes {
        let li = level_input(
            &tree,
            &ends,
            scope,
            &box_size,
            segs.as_deref(),
            &stubs,
            &ext_off,
        );
        let out = match ordered[slot(scope)].take() {
            Some((prep, layers, port_rank)) => {
                let hier = Hierarchy {
                    port_rank,
                    ..li.hier.clone()
                };
                finish(&li.flat, li.graph, &hier, prep, layers)
            }
            None => layout_flat_in(&li.flat, li.graph, &li.hier),
        };
        let level = level_output(li, out);
        if let Some(c) = scope {
            let cl = &input.clusters[c];
            let (w, h) = (
                (level.bbox.w + 2.0 * pad).max(cl.min_width),
                level.bbox.h + 2.0 * pad + cl.title_height,
            );
            box_size[c] = (w, h);
            // Where the composition below puts the contents in the box.
            let bb = level.bbox;
            let horizontal = tree.direction(scope).is_horizontal();
            for (i, it) in level.items.iter().enumerate() {
                if let Item::Ext(ei) = *it {
                    let p = level.out.nodes[i];
                    let off = if horizontal {
                        p.y - bb.y + pad + cl.title_height - h / 2.0
                    } else {
                        p.x - bb.x + pad + (w - 2.0 * pad - bb.w) / 2.0 - w / 2.0
                    };
                    ext_off.insert((c, ei), off);
                }
            }
            // Ports all unconnected inside: no ELK nodes there (the container
            // has no external port dummies), so ELK spreads them over their
            // side, as a node's own ports. (With others, they are dummies
            // placed inside like the rest.)
            let side_len = if horizontal { h } else { w };
            let all_unconnected = level.ext_kinds.iter().all(|k| k.2);
            for entering in [true, false] {
                let on_side = |k: &(usize, bool, bool)| k.1 == entering;
                if all_unconnected {
                    let mut stub: Vec<(f64, usize)> = level
                        .ext_kinds
                        .iter()
                        .filter(|k| on_side(k))
                        .map(|&(i, _, _)| {
                            let p = level.out.nodes[i];
                            (if horizontal { p.y } else { p.x }, i)
                        })
                        .collect();
                    stub.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    let inset = super::elk_port_inset(match tree.owner_of_cluster(c) {
                        None => ElkGraph::Root,
                        Some(_) => ElkGraph::Container { separate: false },
                    });
                    let offs = super::elk_port_offsets(stub.len(), side_len, inset);
                    for (&(_, i), o) in stub.iter().zip(offs) {
                        if let Item::Ext(ei) = level.items[i] {
                            ext_off.insert((c, ei), o - side_len / 2.0);
                        }
                    }
                }
            }
        }
        levels[slot(scope)] = Some(level);
    }

    // Compose: place every level at its box, top-down.
    let n = input.nodes.len();
    let mut nodes = vec![Point::new(0.0, 0.0); n];
    let mut routes: Vec<Option<EdgeRoute>> = vec![None; input.edges.len()];
    // Under ELK: per edge, its segments' routes.
    let mut pieces: Vec<Vec<(Seg, EdgeRoute)>> = vec![Vec::new(); input.edges.len()];
    let mut clusters = vec![Rect::default(); nc];
    let mut stack: Vec<(Option<usize>, Point)> = vec![(None, Point::new(0.0, 0.0))];
    while let Some((scope, off)) = stack.pop() {
        let Some(level) = levels[slot(scope)].take() else {
            continue;
        };
        for (i, it) in level.items.iter().enumerate() {
            let p = level.out.nodes[i].plus(off);
            match *it {
                Item::Node(v) => nodes[v] = p,
                Item::Ext(_) => {}
                Item::Box(b) => {
                    let (w, h) = box_size[b];
                    let r = Rect::centered(p, w, h);
                    clusters[b] = r;
                    let inner = levels[slot(Some(b))].as_ref().map(|l| l.bbox);
                    if let Some(bb) = inner {
                        let origin = Point::new(
                            r.x + pad + (r.w - 2.0 * pad - bb.w) / 2.0,
                            r.y + pad + input.clusters[b].title_height,
                        );
                        stack.push((Some(b), Point::new(origin.x - bb.x, origin.y - bb.y)));
                    }
                }
            }
        }
        for (k, &c) in level.flat_clusters.iter().enumerate() {
            let r = level.out.clusters[k];
            clusters[c] = Rect::new(r.x + off.x, r.y + off.y, r.w, r.h);
        }
        for (k, &ei) in level.edge_ids.iter().enumerate() {
            let r = &level.out.edges[k];
            let mut route = EdgeRoute {
                points: r.points.iter().map(|p| p.plus(off)).collect(),
                label: r.label.map(|p| p.plus(off)),
            };
            match level.segs.get(k).copied().flatten() {
                Some(seg) => {
                    // An external port sits on the container's border.
                    if let (Some(c), Some(first), Some(last)) = (
                        scope,
                        route.points.first().copied(),
                        route.points.last().copied(),
                    ) {
                        let r = clusters[c];
                        let dir = tree.direction(scope);
                        let snap = |p: Point, entering: bool| -> Point {
                            let low = entering
                                != matches!(dir, Direction::BottomTop | Direction::RightLeft);
                            match (dir.is_horizontal(), low) {
                                (false, true) => Point::new(p.x, r.y),
                                (false, false) => Point::new(p.x, r.bottom()),
                                (true, true) => Point::new(r.x, p.y),
                                (true, false) => Point::new(r.right(), p.y),
                            }
                        };
                        if seg.from == SegEnd::Ext {
                            route.points[0] = snap(first, true);
                        }
                        if seg.to == SegEnd::Ext {
                            let at = route.points.len() - 1;
                            route.points[at] = snap(last, false);
                        }
                    }
                    pieces[ei].push((seg, route));
                }
                None => routes[ei] = Some(route),
            }
        }
    }
    // An edge crossing container borders: its segments from its source outwards,
    // then inwards to its target.
    for (ei, mut segs) in pieces.into_iter().enumerate() {
        if segs.is_empty() {
            continue;
        }
        let depth = |s: &Seg| s.scope.map_or(0, |c| tree.chains[c].len()) as i64;
        segs.sort_by_key(|(s, _)| match (s.main, s.to == SegEnd::Ext) {
            (true, _) => (1, 0),
            (false, true) => (0, -depth(s)),
            (false, false) => (2, depth(s)),
        });
        let mut points = Vec::new();
        let mut label = None;
        for (s, r) in segs {
            points.extend(r.points);
            if s.main {
                label = r.label;
            }
        }
        routes[ei] = Some(EdgeRoute {
            points: super::ortho::simplify(&points),
            label,
        });
    }
    let edges = routes
        .into_iter()
        .zip(&input.edges)
        .map(|(r, e)| {
            r.unwrap_or_else(|| EdgeRoute {
                points: vec![nodes[e.from], nodes[e.to]],
                label: e.label.map(|_| nodes[e.from].lerp(nodes[e.to], 0.5)),
            })
        })
        .collect();
    let cluster_directions = (0..nc)
        .map(|c| {
            let scope = if tree.separate[c] {
                Some(c)
            } else {
                tree.owner_of_cluster(c)
            };
            tree.direction(scope)
        })
        .collect();
    LayoutOutput {
        nodes,
        edges,
        clusters,
        cluster_directions,
    }
}

/// ELK's split of every edge into one segment per graph of the hierarchy it
/// passes (`CompoundGraphPreprocessor`): from its source to its container's
/// border, from there to the next container's border, ..., between the
/// children of the two ends' common ancestor, and so on inwards to its target.
/// An edge between a cluster and one of its own members is left out.
fn segments(tree: &Tree, ends: &[(Member, Member)]) -> Vec<Seg> {
    let nc = tree.input.clusters.len();
    let mut out = Vec::new();
    for (ei, &(a, b)) in ends.iter().enumerate() {
        let anc = tree.common_ancestor(a, b);
        if anc.is_some_and(|c| a == Member::Cluster(c) || b == Member::Cluster(c)) {
            continue;
        }
        // The levels from `m`'s up to (not including) the common ancestor's, each
        // with what stands for `m` there; and what stands for it at the ancestor.
        let chain = |m: Member| -> Option<(Vec<LevelEnd>, SegEnd)> {
            let (mut scope, mut item) = match m {
                Member::Node(v) => (tree.owner_of_node(v), SegEnd::Node(v)),
                Member::Cluster(c) if tree.separate[c] => {
                    (tree.owner_of_cluster(c), SegEnd::Box(c))
                }
                Member::Cluster(_) => return None,
            };
            let mut levels = Vec::new();
            while scope != anc {
                let c = scope?;
                if levels.len() > nc {
                    return None;
                }
                levels.push((scope, item));
                item = SegEnd::Box(c);
                scope = tree.owner_of_cluster(c);
            }
            Some((levels, item))
        };
        let (Some((up_a, ia)), Some((up_b, ib))) = (chain(a), chain(b)) else {
            continue;
        };
        if ia == ib && matches!(ia, SegEnd::Box(_)) && a != b {
            continue;
        }
        for (scope, from) in up_a {
            out.push(Seg {
                edge: ei,
                scope,
                from,
                to: SegEnd::Ext,
                main: false,
            });
        }
        out.push(Seg {
            edge: ei,
            scope: anc,
            from: ia,
            to: ib,
            main: true,
        });
        for (scope, to) in up_b {
            out.push(Seg {
                edge: ei,
                scope,
                from: SegEnd::Ext,
                to,
                main: false,
            });
        }
    }
    out
}

/// The edges of the segments of level `scope` that reach its border, in the
/// order `CompoundGraphPreprocessor` makes their external port dummies: first
/// those passing on from the containers within (`processInnerHierarchicalEdgeSegments`:
/// container by container, in the order each made its own), then those of the
/// level's own children (`processOuterHierarchicalEdgeSegments`: child by
/// child, port by port, a port being an edge end, made in edge order). The
/// level's unconnected ports (edges to the container itself) come after these.
fn ext_order(
    tree: &Tree,
    ends: &[(Member, Member)],
    all: &[Seg],
    scope: Option<usize>,
) -> Vec<usize> {
    let input = tree.input;
    // Where a child stands among the level's children (as `level_input` orders them).
    let child_key = |c: usize| {
        (0..input.nodes.len())
            .find(|&v| {
                input.nodes[v]
                    .cluster
                    .is_some_and(|m| tree.chains[m].contains(&c))
            })
            .unwrap_or(usize::MAX)
    };
    let mut keyed: Vec<(bool, usize, usize, usize)> = Vec::new();
    let mut inner: HashMap<usize, Vec<usize>> = HashMap::new();
    for s in all.iter().filter(|s| s.scope == scope) {
        let (inside, out) = match (s.from, s.to) {
            (end, SegEnd::Ext) => (end, true),
            (SegEnd::Ext, end) => (end, false),
            _ => continue,
        };
        let own_end = if out { ends[s.edge].0 } else { ends[s.edge].1 };
        let key = match inside {
            SegEnd::Node(v) => (true, v, s.edge),
            SegEnd::Box(c) if own_end == Member::Cluster(c) => (true, child_key(c), s.edge),
            SegEnd::Box(c) => {
                let order = inner
                    .entry(c)
                    .or_insert_with(|| ext_order(tree, ends, all, Some(c)));
                let r = order
                    .iter()
                    .position(|&e| e == s.edge)
                    .unwrap_or(usize::MAX);
                (false, child_key(c), r)
            }
            SegEnd::Ext => continue,
        };
        keyed.push((key.0, key.1, key.2, s.edge));
    }
    keyed.sort();
    keyed.into_iter().map(|k| k.3).collect()
}

/// One level's layered pass, before it is laid out.
struct LevelInput {
    items: Vec<Item>,
    flat: LayoutInput,
    hier: Hierarchy,
    graph: ElkGraph,
    flat_clusters: Vec<usize>,
    edge_ids: Vec<usize>,
    seg_of: Vec<Option<Seg>>,
    ext_kinds: Vec<(usize, bool, bool)>,
    elk: bool,
}

/// The layered pass of level `scope`: its items (with the boxes of its
/// separate children at `box_size`) and edges.
fn level_input(
    tree: &Tree,
    ends: &[(Member, Member)],
    scope: Option<usize>,
    box_size: &[(f64, f64)],
    all_segs: Option<&[Seg]>,
    stubs: &[(usize, usize, bool)],
    ext_off: &HashMap<(usize, usize), f64>,
) -> LevelInput {
    let input = tree.input;
    let nc = input.clusters.len();
    // Integrated clusters of this level.
    let mut flat_of: Vec<Option<usize>> = vec![None; nc];
    let mut flat_clusters = Vec::new();
    for (c, flat) in flat_of.iter_mut().enumerate() {
        if !tree.separate[c] && tree.owner_of_cluster(c) == scope && Some(c) != scope {
            *flat = Some(flat_clusters.len());
            flat_clusters.push(c);
        }
    }
    // Items: this level's own nodes, then the boxes of its separate children.
    let mut items = Vec::new();
    let mut node_item: Vec<Option<usize>> = vec![None; input.nodes.len()];
    let mut box_item: Vec<Option<usize>> = vec![None; nc];
    for v in 0..input.nodes.len() {
        if tree.owner_of_node(v) == scope {
            items.push(Item::Node(v));
        }
    }
    for c in 0..nc {
        if tree.separate[c] && Some(c) != scope && tree.owner_of_cluster(c) == scope {
            items.push(Item::Box(c));
        }
    }
    // Keep the input's (model) order: a box stands where its first member does.
    let first_member = |c: usize| {
        (0..input.nodes.len())
            .find(|&v| {
                input.nodes[v]
                    .cluster
                    .is_some_and(|m| tree.chains[m].contains(&c))
            })
            .unwrap_or(usize::MAX)
    };
    items.sort_by_key(|it| match *it {
        Item::Node(v) => v,
        Item::Box(c) => first_member(c),
        Item::Ext(_) => usize::MAX,
    });
    // External ports after everything else, one per segment reaching the border,
    // in the order ELK makes their dummies (see `ext_order`).
    let segs: Vec<Seg> = all_segs
        .unwrap_or_default()
        .iter()
        .filter(|s| s.scope == scope)
        .copied()
        .collect();
    let mut ext_segs: Vec<Seg> = segs
        .iter()
        .filter(|s| s.from == SegEnd::Ext || s.to == SegEnd::Ext)
        .copied()
        .collect();
    if let Some(all) = all_segs {
        let order = ext_order(tree, ends, all, scope);
        ext_segs.sort_by_key(|s| order.iter().position(|&e| e == s.edge));
    }
    let mut ext_item: HashMap<usize, usize> = HashMap::new();
    for s in &ext_segs {
        ext_item.insert(s.edge, items.len());
        items.push(Item::Ext(s.edge));
    }
    // Per external port: (item, entering, unconnected inside).
    let mut ext_kinds: Vec<(usize, bool, bool)> = ext_segs
        .iter()
        .filter_map(|s| Some((*ext_item.get(&s.edge)?, s.from == SegEnd::Ext, false)))
        .collect();
    let mut stub_ext = Vec::new();
    for &(ei, _, entering) in stubs.iter().filter(|s| Some(s.1) == scope) {
        stub_ext.push((items.len(), entering));
        ext_kinds.push((items.len(), entering, true));
        items.push(Item::Ext(ei));
    }
    for (i, it) in items.iter().enumerate() {
        match *it {
            Item::Node(v) => node_item[v] = Some(i),
            Item::Box(c) => box_item[c] = Some(i),
            Item::Ext(_) => {}
        }
    }
    let node_end = |v: usize| -> Option<usize> {
        node_item[v].or_else(|| {
            let c = input.nodes[v].cluster?;
            box_item[tree.box_at(c, scope)?]
        })
    };
    let end_of = |m: Member| -> Option<End> {
        match m {
            Member::Node(v) => node_end(v).map(End::Item),
            Member::Cluster(c) if Some(c) == scope => None,
            Member::Cluster(c) => {
                if let Some(flat) = flat_of[c] {
                    let rep = (0..input.nodes.len())
                        .find(|&v| {
                            input.nodes[v]
                                .cluster
                                .is_some_and(|nc_| tree.chains[nc_].contains(&c))
                        })
                        .and_then(node_end)?;
                    Some(End::Cluster { flat, rep })
                } else {
                    tree.box_at(c, scope)
                        .and_then(|b| box_item[b])
                        .map(End::Item)
                }
            }
        }
    };
    // Inside a container ELK gives an edge label a layer of its own, with the full
    // layer gap on both sides of it; the dagre-style split around the label vertex
    // only leaves half the gap each side, so pad the label along the rank axis.
    let label_of = |l: Option<(f64, f64)>| match scope {
        None => l,
        // The orthogonal pass pads its label layers itself.
        Some(_) if matches!(input.routing, EdgeRouting::Orthogonal(_)) => l,
        Some(_) if tree.direction(scope).is_horizontal() => {
            l.map(|(w, h)| (w + input.inner_rank_sep, h))
        }
        Some(_) => l.map(|(w, h)| (w, h + input.inner_rank_sep)),
    };
    let mut edges = Vec::new();
    let mut edge_ids = Vec::new();
    let mut seg_of = Vec::new();
    let mut hier = Hierarchy::default();
    hier.ext.extend(stub_ext);
    // A container with external port dummies keeps its ports' sides
    // (`CompoundGraphPreprocessor` makes it `FIXED_SIDE`).
    hier.side_fixed = items
        .iter()
        .map(|it| match *it {
            Item::Box(c) => all_segs
                .unwrap_or_default()
                .iter()
                .any(|s| s.scope == Some(c) && (s.from == SegEnd::Ext || s.to == SegEnd::Ext)),
            _ => false,
        })
        .collect();
    // ELK makes a node's ports as it imports the edges, breadth-first by the
    // graph containing each (the two ends' common ancestor): an edge from further
    // out comes first.
    let mut segs = segs;
    segs.sort_by_key(|s| {
        let (a, b) = ends[s.edge];
        tree.common_ancestor(a, b)
            .map_or(0, |c| tree.chains[c].len())
    });
    for s in &segs {
        let e = &input.edges[s.edge];
        let end = |x: SegEnd| -> Option<usize> {
            match x {
                SegEnd::Node(v) => node_item[v],
                SegEnd::Box(c) => box_item[c],
                SegEnd::Ext => ext_item.get(&s.edge).copied(),
            }
        };
        let (Some(from), Some(to)) = (end(s.from), end(s.to)) else {
            continue;
        };
        let port = |x: SegEnd| match x {
            SegEnd::Box(c) => ext_off.get(&(c, s.edge)).copied(),
            _ => None,
        };
        hier.ports.push((port(s.from), port(s.to)));
        // A port of a container where the edge goes on inside it (not one on
        // the container itself) stands off its border.
        let (a, b) = ends[s.edge];
        let crossing =
            |x: SegEnd, m: Member| matches!(x, SegEnd::Box(c) if m != Member::Cluster(c));
        if crossing(s.from, a) || crossing(s.to, b) {
            hier.margins.resize(items.len(), (0.0, 0.0));
            if crossing(s.from, a) {
                hier.margins[from].1 = super::ELK_HIERARCHICAL_PORT_OFFSET;
            }
            if crossing(s.to, b) {
                hier.margins[to].0 = super::ELK_HIERARCHICAL_PORT_OFFSET;
            }
        }
        if s.from == SegEnd::Ext {
            hier.ext.push((from, true));
        }
        if s.to == SegEnd::Ext {
            hier.ext.push((to, false));
        }
        edges.push(LayoutEdge {
            from,
            to,
            minlen: e.minlen,
            label: e.label.filter(|_| s.main),
            from_cluster: None,
            to_cluster: None,
            // A terminal label stays with the segment at its node.
            terminals: [
                e.terminals[0].filter(|_| matches!(s.from, SegEnd::Node(_))),
                e.terminals[1].filter(|_| matches!(s.to, SegEnd::Node(_))),
            ],
        });
        edge_ids.push(s.edge);
        seg_of.push(Some(*s));
    }
    for (ei, (e, &(a, b))) in input
        .edges
        .iter()
        .zip(ends)
        .enumerate()
        .filter(|_| all_segs.is_none())
    {
        let (Some(ea), Some(eb)) = (end_of(a), end_of(b)) else {
            continue;
        };
        if let (End::Item(x), End::Item(y)) = (ea, eb) {
            // Both ends inside the same box: routed by that box's own level, unless the
            // edge really is a loop on the cluster itself.
            if x == y && matches!(items[x], Item::Box(_)) && a != b {
                continue;
            }
        }
        let split = |end: End| match end {
            End::Item(i) => (i, None),
            End::Cluster { flat, rep } => (rep, Some(flat)),
        };
        let ((from, from_cluster), (to, to_cluster)) = (split(ea), split(eb));
        edges.push(LayoutEdge {
            from,
            to,
            minlen: e.minlen,
            label: label_of(e.label),
            from_cluster,
            to_cluster,
            terminals: [
                e.terminals[0].filter(|_| from_cluster.is_none()),
                e.terminals[1].filter(|_| to_cluster.is_none()),
            ],
        });
        edge_ids.push(ei);
        seg_of.push(None);
    }
    let nodes: Vec<LayoutNode> = items
        .iter()
        .map(|it| match *it {
            Item::Node(v) => LayoutNode {
                cluster: input.nodes[v].cluster.and_then(|c| flat_of[c]),
                ..input.nodes[v].clone()
            },
            Item::Box(b) => LayoutNode {
                width: box_size[b].0,
                height: box_size[b].1,
                cluster: tree.parent[b].and_then(|p| flat_of[p]),
            },
            Item::Ext(_) => LayoutNode {
                width: 0.0,
                height: 0.0,
                cluster: None,
            },
        })
        .collect();
    let clusters = flat_clusters
        .iter()
        .map(|&c| LayoutCluster {
            parent: tree.parent[c].and_then(|p| flat_of[p]),
            ..input.clusters[c].clone()
        })
        .collect();
    let (node_sep, rank_sep) = match scope {
        None => (input.node_sep, input.rank_sep),
        Some(_) => (input.inner_node_sep, input.inner_rank_sep),
    };
    // A container laid out on its own routes with its own (wider) edge spacing.
    let routing = match (input.routing, scope) {
        (EdgeRouting::Orthogonal(_), Some(_)) => EdgeRouting::Orthogonal(OrthoSpacing::CONTAINER),
        (r, _) => r,
    };
    let flat = LayoutInput {
        nodes,
        edges,
        clusters,
        direction: tree.direction(scope),
        node_sep,
        rank_sep,
        routing,
        ..input.clone()
    };
    let graph = match scope {
        None => ElkGraph::Root,
        Some(c) => ElkGraph::Container {
            separate: !tree.hierarchical(c),
        },
    };
    LevelInput {
        items,
        flat,
        hier,
        graph,
        flat_clusters,
        edge_ids,
        seg_of,
        ext_kinds,
        elk: all_segs.is_some(),
    }
}

/// A level from its pass's result.
fn level_output(li: LevelInput, out: LayoutOutput) -> Level {
    let LevelInput {
        items,
        flat,
        flat_clusters,
        edge_ids,
        seg_of,
        ext_kinds,
        elk,
        ..
    } = li;
    let mut bbox: Option<Rect> = None;
    let mut add = |r: Rect| {
        bbox = Some(match bbox {
            None => r,
            Some(b) => {
                let x0 = b.x.min(r.x);
                let y0 = b.y.min(r.y);
                Rect::new(
                    x0,
                    y0,
                    b.right().max(r.right()) - x0,
                    b.bottom().max(r.bottom()) - y0,
                )
            }
        });
    };
    for ((nd, p), it) in flat.nodes.iter().zip(&out.nodes).zip(&items) {
        if !matches!(it, Item::Ext(_)) {
            add(Rect::centered(*p, nd.width, nd.height));
        }
    }
    // ELK sizes a graph by everything in its layers, edges included.
    if elk {
        for route in &out.edges {
            for p in &route.points {
                add(Rect::new(p.x, p.y, 0.0, 0.0));
            }
        }
    }
    for (e, route) in flat.edges.iter().zip(&out.edges) {
        if let (Some((w, h)), Some(p)) = (e.label, route.label) {
            add(Rect::centered(p, w, h));
        }
        // Orthogonal self-loops stand off their node's side.
        if e.from == e.to && matches!(flat.routing, EdgeRouting::Orthogonal(_)) {
            for p in &route.points {
                add(Rect::new(p.x, p.y, 0.0, 0.0));
            }
        }
    }
    for r in &out.clusters {
        if r.w > 0.0 || r.h > 0.0 {
            add(*r);
        }
    }
    // ELK sizes a layer of external port dummies with `spacing.portsSurrounding`
    // beyond its first and last dummy (`LayerSizeAndGraphHeightCalculator`);
    // where they are ELK nodes at all (see `elkorder::minimize_run`).
    let mut bbox = bbox.unwrap_or_default();
    if elk && !ext_kinds.iter().all(|k| k.2) {
        let horizontal = flat.direction.is_horizontal();
        for entering in [true, false] {
            let cross: Vec<f64> = ext_kinds
                .iter()
                .filter(|k| k.1 == entering)
                .map(|k| out.nodes[k.0])
                .map(|p| if horizontal { p.y } else { p.x })
                .collect();
            let lo = cross.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = cross.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if lo > hi {
                continue;
            }
            let (lo, hi) = (lo - ELK_PORTS_SURROUNDING, hi + ELK_PORTS_SURROUNDING);
            if horizontal {
                let (y0, y1) = (bbox.y.min(lo), bbox.bottom().max(hi));
                bbox = Rect::new(bbox.x, y0, bbox.w, y1 - y0);
            } else {
                let (x0, x1) = (bbox.x.min(lo), bbox.right().max(hi));
                bbox = Rect::new(x0, bbox.y, x1 - x0, bbox.h);
            }
        }
    }
    Level {
        items,
        out,
        flat_clusters,
        edge_ids,
        segs: seg_of,
        ext_kinds,
        bbox,
    }
}

/// mermaid's `elk.spacing.portsSurrounding` (12 on every side).
const ELK_PORTS_SURROUNDING: f64 = 12.0;

/// ELK's crossing minimisation for every level, the levels of one ELK run
/// (the root, or a container laid out on its own, with the containers laid out
/// within it: `INCLUDE_CHILDREN`) ordered together (`elkorder::minimize_run`).
/// Per level (in `scopes`' order): its prepared pass and its ordered layers.
fn order_runs(tree: &Tree, scopes: &[Option<usize>], inputs: &[LevelInput]) -> Vec<RunLevel> {
    let k = scopes.len();
    let preps: Vec<Prepared> = inputs
        .iter()
        .map(|li| prepare(&li.flat, &li.hier))
        .collect();
    let index = |s: Option<usize>| scopes.iter().position(|&x| x == s);
    // Per level: the level of the container it is laid out within, if any.
    let parent: Vec<Option<usize>> = scopes
        .iter()
        .map(|&s| {
            let c = s.filter(|&c| tree.hierarchical(c))?;
            index(tree.owner_of_cluster(c))
        })
        .collect();
    let mut setups = Vec::with_capacity(k);
    let mut elk_layers = Vec::with_capacity(k);
    for p in &preps {
        let (setup, layers) = ElkLayers::new(p.layers.clone(), &p.verts, &p.chains, &p.inl);
        setups.push(setup);
        elk_layers.push(Some(layers));
    }
    let borders: Vec<Vec<bool>> = preps.iter().map(Prepared::border).collect();
    let ctxs: Vec<OrderCtx> = preps
        .iter()
        .map(|p| OrderCtx {
            verts: &p.verts,
            chains: &p.cluster_chain,
        })
        .collect();
    let mut ordered: Vec<Option<elkorder::RunOrder>> = (0..k).map(|_| None).collect();
    for root in (0..k).filter(|&i| parent[i].is_none()) {
        // The run: its root first, then every level whose chain of containers
        // leads to it.
        let mut members = vec![root];
        let mut i = 0;
        while i < members.len() {
            let cur = members[i];
            members.extend((0..k).filter(|&j| parent[j] == Some(cur)));
            i += 1;
        }
        let at = |level: usize| members.iter().position(|&m| m == level);
        let graphs: Vec<elkorder::RunGraph> = members
            .iter()
            .map(|&i| {
                let li = &inputs[i];
                let n = preps[i].n;
                let nv = preps[i].verts.len();
                let ext_key = (0..nv)
                    .map(|v| match li.items.get(v).filter(|_| v < n) {
                        Some(Item::Ext(ei)) => Some(*ei),
                        _ => None,
                    })
                    .collect();
                let seg_key = setups[i].seg_edges().map(|e| li.edge_ids[e]).collect();
                let up = parent[i].and_then(|p| {
                    let c = scopes[i]?;
                    let v = inputs[p].items.iter().position(|&it| it == Item::Box(c))?;
                    Some((at(p)?, v))
                });
                elkorder::RunGraph {
                    layers: elk_layers[i].take().unwrap_or_default(),
                    graph: setups[i].graph(&borders[i], n),
                    ctx: &ctxs[i],
                    opts: elk_options(li.graph, n),
                    ext_key,
                    seg_key,
                    parent: up,
                }
            })
            .collect();
        for (&i, o) in members.iter().zip(elkorder::minimize_run(graphs)) {
            ordered[i] = Some(o);
        }
    }
    let complete: Vec<(Vec<Vec<usize>>, PortRanks)> = setups
        .into_iter()
        .zip(ordered)
        .enumerate()
        .map(|(i, (setup, o))| {
            let (layers, seg_rank) = o.map(|o| (o.layers, o.port_rank)).unwrap_or_default();
            // Per edge of the level: the ranks at its real ends.
            let mut rank: PortRanks = Vec::new();
            for (e, r) in setup.seg_edges().zip(seg_rank) {
                if rank.len() <= e {
                    rank.resize(e + 1, (None, None));
                }
                rank[e].0 = rank[e].0.or(r.0);
                rank[e].1 = rank[e].1.or(r.1);
            }
            let layers = setup.complete(layers, &preps[i].verts, &ctxs[i]);
            (layers, rank)
        })
        .collect();
    drop(ctxs);
    preps
        .into_iter()
        .zip(complete)
        .map(|(p, (l, r))| (p, l, r))
        .collect()
}

/// Per edge: the ranks of its ports in ELK's port lists (see
/// `elkorder::RunOrder::port_rank`).
type PortRanks = Vec<(Option<usize>, Option<usize>)>;

/// One level of an ELK run, ordered: its prepared pass, its layers and its
/// port ranks.
type RunLevel = (Prepared, Vec<Vec<usize>>, PortRanks);

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{even_group_frames, subgraph_feedback_edges};

    fn node(cluster: Option<usize>) -> LayoutNode {
        LayoutNode {
            width: 80.0,
            height: 40.0,
            cluster,
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
            cluster_padding: 24.0,
            inner_node_sep: 50.0,
            inner_rank_sep: 24.0,
            routing: EdgeRouting::Splines,
        }
    }

    fn with_dir(parent: Option<usize>, dir: Option<Direction>) -> LayoutCluster {
        LayoutCluster {
            direction: dir,
            ..LayoutCluster::new(parent, 24.0)
        }
    }

    fn contains(outer: &Rect, inner: &Rect) -> bool {
        outer.x <= inner.x + 1e-6
            && outer.y <= inner.y + 1e-6
            && outer.right() >= inner.right() - 1e-6
            && outer.bottom() >= inner.bottom() - 1e-6
    }

    #[test]
    fn uncrossed_cluster_keeps_its_own_direction() {
        // TB graph, cluster 0 asks for LR and no edge crosses its border.
        let inp = input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge {
                    to_cluster: Some(0),
                    ..LayoutEdge::new(0, 1, 1, None)
                },
            ],
            vec![with_dir(None, Some(Direction::LeftRight))],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!((out.nodes[1].y - out.nodes[2].y).abs() < 1e-6, "{out:?}");
        assert!(out.nodes[1].x < out.nodes[2].x);
        // The cluster sits below its source, as a whole.
        assert!(out.clusters[0].y > out.nodes[0].y + 20.0);
    }

    #[test]
    fn crossed_cluster_inherits_the_parent_direction() {
        // Same, but an edge enters a member: ELK switches the cluster to
        // INCLUDE_CHILDREN and its `direction` no longer applies.
        let inp = input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(0, 1, 1, None),
            ],
            vec![with_dir(None, Some(Direction::LeftRight))],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!(out.nodes[1].y < out.nodes[2].y);
        assert!((out.nodes[1].x - out.nodes[2].x).abs() < 1e-6);
    }

    #[test]
    fn nested_directions_and_padding() {
        // LR root > TB cluster 0 > RL cluster 1.
        let inp = input(
            vec![node(Some(1)), node(Some(1)), node(Some(0))],
            vec![LayoutEdge::new(0, 1, 1, None)],
            vec![
                with_dir(None, Some(Direction::TopBottom)),
                with_dir(Some(0), Some(Direction::RightLeft)),
            ],
            Direction::LeftRight,
        );
        let out = layout(&inp);
        assert!(out.nodes[0].x > out.nodes[1].x, "RL inside: {out:?}");
        let r: Vec<Rect> = inp
            .nodes
            .iter()
            .zip(&out.nodes)
            .map(|(n, p)| Rect::centered(*p, n.width, n.height))
            .collect();
        assert!(contains(&out.clusters[1], &r[0]) && contains(&out.clusters[1], &r[1]));
        assert!(contains(&out.clusters[0], &out.clusters[1]));
        assert!(contains(&out.clusters[0], &r[2]));
        // Padding 24 on the sides, title band 24 + padding 24 on top.
        let c1 = out.clusters[1];
        let left = r[1].x.min(r[0].x) - c1.x;
        assert!((left - 24.0).abs() < 1e-6, "left padding {left}");
        let top = r[0].y.min(r[1].y) - c1.y;
        assert!((top - 48.0).abs() < 1e-6, "top padding {top}");
        // A cluster without its own direction inherits the root's.
        let inp2 = input(
            vec![node(Some(0)), node(Some(0))],
            vec![LayoutEdge::new(0, 1, 1, None)],
            vec![with_dir(None, None)],
            Direction::LeftRight,
        );
        let out2 = layout(&inp2);
        assert!(out2.nodes[0].x < out2.nodes[1].x);
    }

    #[test]
    fn undirected_cluster_inside_a_directed_one_runs_right() {
        // ELK: a container under a SEPARATE_CHILDREN (directed) one is laid out on its
        // own without `elk.direction`, i.e. RIGHT; a top-level undirected container is
        // part of the root run and follows the root.
        let inp = input(
            vec![node(Some(1)), node(Some(1)), node(Some(2)), node(Some(2))],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(2, 3, 1, None),
            ],
            vec![
                with_dir(None, Some(Direction::RightLeft)),
                with_dir(Some(0), None),
                with_dir(None, None),
            ],
            Direction::BottomTop,
        );
        let out = layout(&inp);
        let (a, b) = (out.nodes[0], out.nodes[1]);
        assert!(
            b.x > a.x && (a.y - b.y).abs() < 1e-6,
            "inner runs LR: {a:?} {b:?}"
        );
        let (c, d) = (out.nodes[2], out.nodes[3]);
        assert!(
            d.y < c.y && (c.x - d.x).abs() < 1e-6,
            "top level follows BT: {c:?} {d:?}"
        );
    }

    #[test]
    fn inner_rank_separation_is_tighter() {
        let inp = input(
            vec![node(Some(0)), node(Some(0))],
            vec![LayoutEdge::new(0, 1, 1, None)],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let gap = out.nodes[1].y - out.nodes[0].y - 40.0;
        assert!((gap - 24.0).abs() < 1e-6, "gap {gap}");
    }

    #[test]
    fn common_ancestor_joins_its_parent_layout() {
        // x (in 0) → y (in 1, inside 0): parents differ, the common ancestor is 0,
        // so both 0 and 1 are integrated (no direction of their own).
        let inp = input(
            vec![node(Some(0)), node(Some(1)), node(Some(1))],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(1, 2, 1, None),
            ],
            vec![
                with_dir(None, Some(Direction::LeftRight)),
                with_dir(Some(0), Some(Direction::LeftRight)),
            ],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!(out.nodes[0].y < out.nodes[1].y && out.nodes[1].y < out.nodes[2].y);
    }

    #[test]
    fn whole_cluster_edges_rank_after_every_member() {
        // Integrated cluster 0 (an edge reaches into it) with a chain a → b → c;
        // an edge from the cluster as a whole to d puts d below c, not beside b.
        let inp = input(
            vec![
                node(Some(0)),
                node(Some(0)),
                node(Some(0)),
                node(None),
                node(None),
            ],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(4, 0, 1, None),
                LayoutEdge {
                    from_cluster: Some(0),
                    ..LayoutEdge::new(0, 3, 1, None)
                },
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!(out.nodes[3].y > out.nodes[2].y + 40.0, "{out:?}");
        assert!(out.nodes[3].y > out.clusters[0].bottom());
    }

    /// `x --> C` with `b --> x` from inside C: mermaid's adapter turns the edge
    /// back into C round before ELK sees it, so x stays above the whole cluster
    /// (rather than C's members wrapping round x).
    #[test]
    fn feedback_edge_out_of_a_cluster_is_laid_out_reversed() {
        let to_c = LayoutEdge {
            to_cluster: Some(0),
            ..LayoutEdge::new(0, 1, 1, None)
        };
        let mut inp = input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                to_c,
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(2, 0, 1, None),
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        );
        // dagre has no such pass.
        assert_eq!(subgraph_feedback_edges(&inp), vec![false, false, true]);
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        assert!(out.nodes[0].y < out.clusters[0].y, "{out:?}");
        // The edge is still drawn from b to x.
        let pts = &out.edges[2].points;
        assert!(pts[0].y > pts[pts.len() - 1].y, "{pts:?}");
    }

    /// Two sibling clusters fed by x, one feeding back: both hang below x, side
    /// by side, as mermaid draws them.
    #[test]
    fn sibling_clusters_with_a_feedback_edge_hang_below_their_source() {
        let whole = |from: usize, to: usize, c: usize| LayoutEdge {
            to_cluster: Some(c),
            ..LayoutEdge::new(from, to, 1, None)
        };
        let mut inp = input(
            vec![
                node(None),
                node(Some(0)),
                node(Some(0)),
                node(Some(1)),
                node(Some(1)),
            ],
            vec![
                whole(0, 1, 0),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(3, 4, 1, None),
                whole(0, 3, 1),
                LayoutEdge::new(4, 0, 1, None),
            ],
            vec![with_dir(None, None), with_dir(None, None)],
            Direction::TopBottom,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        assert_eq!(
            subgraph_feedback_edges(&inp),
            vec![false, false, false, false, true]
        );
        let out = layout(&inp);
        let (c0, c1) = (out.clusters[0], out.clusters[1]);
        assert!(out.nodes[0].y < c0.y && out.nodes[0].y < c1.y, "{out:?}");
        assert!(!c0.intersects(&c1), "{out:?}");
        assert!(
            c0.y < c1.bottom() && c1.y < c0.bottom(),
            "side by side: {out:?}"
        );
    }

    /// Under ELK a cluster an edge crosses is still a graph of its own: its
    /// contents are layered on their own and the edge leaves through its border.
    /// `c1n0 --> C0` with C1 = c1n0 → c1n1 → c1n2: C0 hangs below the whole of
    /// C1 (merged into one layering, it would stand beside C1's last two rows),
    /// and the route crosses C1's bottom border straight down into C0.
    #[test]
    fn elk_crossed_cluster_is_laid_out_as_a_graph_of_its_own() {
        let to_c0 = LayoutEdge {
            to_cluster: Some(0),
            ..LayoutEdge::new(3, 0, 1, None)
        };
        let mut inp = input(
            vec![
                node(Some(0)),
                node(Some(0)),
                node(Some(0)),
                node(Some(1)),
                node(Some(1)),
                node(Some(1)),
            ],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(3, 4, 1, None),
                LayoutEdge::new(4, 5, 1, None),
                to_c0,
            ],
            vec![with_dir(None, None), with_dir(None, None)],
            Direction::TopBottom,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        let (c0, c1) = (out.clusters[0], out.clusters[1]);
        assert!(c0.y > c1.bottom(), "{out:?}");
        // Inside C1 its rows keep the container's own layer spacing.
        assert!((out.nodes[4].y - out.nodes[3].y) < 120.0, "{out:?}");
        let pts = &out.edges[4].points;
        let crossing = pts
            .windows(2)
            .find(|w| w[0].y < c1.bottom() && w[1].y > c1.bottom())
            .expect("crosses C1's bottom");
        assert!((crossing[0].x - crossing[1].x).abs() < 1e-6, "{pts:?}");
        assert!(
            crossing[0].x > c1.x && crossing[0].x < c1.right(),
            "{pts:?}"
        );
        assert!((pts[pts.len() - 1].y - c0.y).abs() < 1e-6, "{pts:?}");
    }

    fn elk(mut inp: LayoutInput) -> LayoutInput {
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        inp
    }

    fn to_cluster(from: usize, to: usize, c: usize) -> LayoutEdge {
        LayoutEdge {
            to_cluster: Some(c),
            ..LayoutEdge::new(from, to, 1, None)
        }
    }

    /// ELK puts the ports it makes for an edge crossing a container's border
    /// half an edge spacing (10) off the border, which deepens the container's
    /// layer by as much on that side; a port of an edge to the container itself
    /// stays on the border. (C0 = a → b, C1 = c → d, C2 = e; b → d crosses both
    /// borders, `C0 --> C1` neither.)
    #[test]
    fn elk_ports_of_crossing_edges_stand_off_the_border() {
        let nodes = vec![
            node(Some(0)),
            node(Some(0)),
            node(Some(1)),
            node(Some(1)),
            node(Some(2)),
        ];
        let clusters = vec![
            with_dir(None, None),
            with_dir(None, None),
            with_dir(None, None),
        ];
        let inner = [
            LayoutEdge::new(0, 1, 1, None),
            LayoutEdge::new(2, 3, 1, None),
        ];
        let run = |cross: LayoutEdge| {
            let mut edges = inner.to_vec();
            edges.push(cross);
            layout(&elk(input(
                nodes.clone(),
                edges,
                clusters.clone(),
                Direction::TopBottom,
            )))
        };
        let crossing = run(LayoutEdge::new(1, 3, 1, None));
        let direct = run(LayoutEdge {
            from_cluster: Some(0),
            to_cluster: Some(1),
            ..LayoutEdge::new(1, 3, 1, None)
        });
        let gap = |o: &LayoutOutput| o.clusters[1].y - o.clusters[0].bottom();
        assert!((gap(&crossing) - gap(&direct) - 20.0).abs() < 1e-6);
        // C2 shares C0's layer, centred on it with its margin.
        let mid = |r: Rect| r.y + r.h / 2.0;
        let (c0, c2) = (crossing.clusters[0], crossing.clusters[2]);
        assert!((mid(c2) - mid(c0) - 5.0).abs() < 1e-6, "{c0:?} {c2:?}");
        let (c0, c2) = (direct.clusters[0], direct.clusters[2]);
        assert!((mid(c2) - mid(c0)).abs() < 1e-6, "{c0:?} {c2:?}");
    }

    /// Where breaking a cycle reverses an edge between containers, ELK keeps
    /// their ports on their sides (`InvertedPortProcessor`). Here (LR) the
    /// edges `y --> x --> y`, `y --> c1n0`, `c0n0 --> x` and `c1n0 --> C0` (c0n0
    /// in C0, c1n0 in C1) put C1 after C0, so `c1n0 --> C0` runs backwards: it
    /// leaves C1 eastwards, runs round below everything and enters C0 from
    /// the west.
    #[test]
    fn elk_reversed_edge_keeps_the_container_ports_on_their_sides() {
        let nodes = vec![node(Some(0)), node(Some(1)), node(None), node(None)];
        let clusters = vec![with_dir(None, None), with_dir(None, None)];
        let edges = vec![
            LayoutEdge::new(2, 3, 1, None),
            LayoutEdge::new(2, 1, 1, None),
            to_cluster(1, 0, 0),
            LayoutEdge::new(3, 2, 1, None),
            LayoutEdge::new(0, 3, 1, None),
        ];
        let out = layout(&elk(input(nodes, edges, clusters, Direction::LeftRight)));
        let (c0, c1) = (out.clusters[0], out.clusters[1]);
        assert!(c0.right() < c1.x, "{c0:?} {c1:?}");
        let pts = &out.edges[2].points;
        assert!(super::super::ortho::is_orthogonal(pts), "{pts:?}");
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        // Out of C1's east side, eastwards ...
        assert!(
            (first.x - c1.right()).abs() < 1e-6 && pts[1].x > first.x,
            "{pts:?}"
        );
        // ... round below both ...
        let low = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        assert!(low > c0.bottom().max(c1.bottom()), "{pts:?}");
        // ... into C0's west side, eastwards.
        let before = pts[pts.len() - 2];
        assert!((last.x - c0.x).abs() < 1e-6 && before.x < last.x, "{pts:?}");
    }

    /// mermaid draws a container's frame its padding from what it holds on the
    /// left, right and bottom, whatever room ELK left around it; an edge with
    /// both ends inside keeps its lane framed, one leaving does not.
    #[test]
    fn group_frames_are_pulled_in_to_their_contents() {
        let nodes = vec![node(Some(0)), node(Some(0)), node(None)];
        let clusters = vec![with_dir(None, None)];
        let edges = vec![
            LayoutEdge::new(0, 1, 1, None),
            LayoutEdge::new(0, 2, 1, None),
        ];
        let inp = elk(input(nodes, edges, clusters, Direction::TopBottom));
        let p = |x: f64, y: f64| Point::new(x, y);
        let route = |points: Vec<Point>| EdgeRoute {
            points,
            label: None,
        };
        let mut out = LayoutOutput {
            nodes: vec![p(100.0, 100.0), p(100.0, 200.0), p(400.0, 400.0)],
            edges: vec![
                // Inside: a lane out to the right (x = 170).
                route(vec![p(100.0, 120.0), p(170.0, 150.0), p(100.0, 180.0)]),
                // Leaving: its lane on the left (x = 0) is not the frame's.
                route(vec![p(100.0, 120.0), p(0.0, 150.0), p(400.0, 380.0)]),
            ],
            clusters: vec![Rect::new(-20.0, 30.0, 260.0, 260.0)],
            cluster_directions: vec![Direction::TopBottom],
        };
        even_group_frames(&inp, &mut out);
        // Children span x 60..140, y 80..220: left 60 - 24, right the lane's
        // 170 + 24, bottom 220 + 24, the top as it was.
        assert_eq!(out.clusters[0], Rect::new(36.0, 30.0, 158.0, 214.0));
    }

    /// ELK gives an edge to a container of the run a port dummy inside it too,
    /// unconnected there; alone on its side, the port is spread over the side as
    /// a node's own (`x --> C0`, C0 = a, b, c with a → c: the edge meets C0 in
    /// the middle of its top).
    #[test]
    fn elk_lone_port_of_an_edge_to_a_cluster_is_centred() {
        let inp = elk(input(
            vec![node(None), node(Some(0)), node(Some(0)), node(Some(0))],
            vec![to_cluster(0, 1, 0), LayoutEdge::new(1, 3, 1, None)],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let c0 = out.clusters[0];
        let end = *out.edges[0].points.last().expect("route");
        assert!((end.y - c0.y).abs() < 1e-6, "{out:?}");
        assert!((end.x - (c0.x + c0.w / 2.0)).abs() < 1.0, "{end:?} {c0:?}");
    }

    /// Beside a port an edge crosses, that port dummy is packed after the
    /// crossing one, as ELK's node placement leaves it (`x --> a` into C0 and
    /// `x --> C0`): the two ports sit one port spacing apart.
    #[test]
    fn elk_port_of_an_edge_to_a_cluster_packs_beside_a_crossing_one() {
        let inp = elk(input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![LayoutEdge::new(0, 1, 1, None), to_cluster(0, 1, 0)],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let c0 = out.clusters[0];
        let crossing = out.edges[0]
            .points
            .windows(2)
            .find(|w| w[0].y < c0.y && w[1].y >= c0.y)
            .map(|w| w[1].x)
            .expect("crosses C0's top");
        let end = *out.edges[1].points.last().expect("route");
        assert!((end.y - c0.y).abs() < 1e-6, "{out:?}");
        assert!(
            ((end.x - crossing).abs() - 12.0).abs() < 1e-6,
            "{end:?} {crossing}"
        );
    }

    /// The container's ports keep the order its contents give their dummies, and
    /// the node feeding both edges orders its own ports alike: the edges into a
    /// cluster from one node do not cross. `x --> C0` (whole) and `x --> b`
    /// with C0 = a → b.
    #[test]
    fn elk_edges_into_a_cluster_from_one_node_do_not_cross() {
        let inp = elk(input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                to_cluster(0, 1, 0),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(0, 2, 1, None),
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let (a, b) = (&out.edges[0].points, &out.edges[2].points);
        // Order where they leave x and where they meet C0's top.
        let c0 = out.clusters[0];
        let at_top = |pts: &[Point]| {
            pts.windows(2)
                .find(|w| w[0].y < c0.y && w[1].y >= c0.y)
                .map(|w| w[1].x)
                .expect("reaches C0")
        };
        let leave = (a[0].x - b[0].x).signum();
        let meet = (at_top(a) - at_top(b)).signum();
        assert_eq!(leave, meet, "{a:?} {b:?}");
    }

    /// mermaid's `orderNestedEdgeEnds`: `x --> C` then `x --> b` with b in C:
    /// the edge into C's member moves in front.
    #[test]
    fn edge_into_a_cluster_member_moves_before_the_edge_to_the_cluster() {
        let inp = input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                to_cluster(0, 1, 0),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(0, 2, 1, None),
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        );
        assert_eq!(super::nested_edge_order(&inp), vec![2, 0, 1]);
    }

    /// ELK keeps a node's ports in the order it made them, and lists a side
    /// clockwise: the west side bottom to top. In a container laid out on its
    /// own, `a --> b : long label` and `b --> a : short` (turned round), the
    /// first sweep that reaches the labels from b puts the turned edge's label
    /// first, as mermaid draws state diagrams' `accelerate` / `brake`.
    #[test]
    fn elk_turned_edge_label_comes_first_in_a_container() {
        let inp = elk(input(
            vec![node(None), node(Some(0)), node(Some(0))],
            vec![
                to_cluster(0, 1, 0),
                LayoutEdge::new(1, 2, 1, Some((64.0, 21.0))),
                LayoutEdge::new(2, 1, 1, Some((35.0, 21.0))),
            ],
            vec![with_dir(None, Some(Direction::TopBottom))],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let (fwd, back) = (out.edges[1].label, out.edges[2].label);
        let (fwd, back) = (fwd.expect("label"), back.expect("label"));
        assert!(back.x < fwd.x, "{back:?} {fwd:?}");
    }
}
