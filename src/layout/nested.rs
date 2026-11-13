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
    elk_options, elkorder, finish, finish_rng, layout_flat, layout_flat_in, prepare,
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
    /// Its external port dummy stands on the side against the segment's flow:
    /// in the last layer for an edge entering, the first for one leaving. ELK
    /// picks a port's side by its net flow (`calculateNetFlow`), which counts an
    /// edge between a container and anything deeper than its own children as
    /// flowing the other way, so such a segment runs backwards.
    against: bool,
}

impl Seg {
    /// Whether its external port dummy (if any) is in the level's first layer.
    fn ext_first(&self) -> Option<bool> {
        match (self.from, self.to) {
            (SegEnd::Ext, _) => Some(!self.against),
            (_, SegEnd::Ext) => Some(self.against),
            _ => None,
        }
    }
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
        // An `INCLUDE_CHILDREN` cluster (one an edge crosses) inside a
        // `SEPARATE_CHILDREN` one is the root of a run of its own; anything
        // else follows its nearest cluster with a policy of its own.
        let mut cur = if self.crossed[c] {
            self.parent[c]
        } else {
            Some(c)
        };
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

    /// Per edge: when ELK imports it, breadth-first by the graph containing it
    /// (the two ends' common ancestor), then in edge order.
    fn import_order(&self) -> Vec<usize> {
        let made: Vec<(usize, usize)> = self
            .input
            .edges
            .iter()
            .enumerate()
            .map(|(e, edge)| {
                let a = edge
                    .from_cluster
                    .map_or(Member::Node(edge.from), Member::Cluster);
                let b = edge
                    .to_cluster
                    .map_or(Member::Node(edge.to), Member::Cluster);
                let depth = self
                    .common_ancestor(a, b)
                    .map_or(0, |c| self.chains[c].len());
                (depth, e)
            })
            .collect();
        let mut by: Vec<usize> = (0..made.len()).collect();
        by.sort_by_key(|&e| made[e]);
        let mut rank = vec![0; made.len()];
        for (r, e) in by.into_iter().enumerate() {
            rank[e] = r;
        }
        rank
    }

    /// How many graphs deep a separate cluster's own graph is (the root's: 0).
    fn depth(&self, scope: Option<usize>) -> usize {
        scope.map_or(0, |c| 1 + self.depth(self.owner_of_cluster(c)))
    }

    /// Under `dagre_groups`: a separate cluster's padding along its flow and across
    /// it (dagre's compound borders, half a rank and half a node and edge gap away).
    fn dagre_group_padding(&self, c: usize) -> (f64, f64) {
        let input = self.input;
        let rank_sep = input.rank_sep + 25.0 * self.depth(Some(c)) as f64;
        (rank_sep / 2.0, (input.node_sep + input.edge_sep) / 2.0)
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
        if self.crossed[c] && self.hierarchical(c) {
            return self.direction(self.owner_of_cluster(c));
        }
        if let Some(d) = self.input.clusters[c].direction {
            return d;
        }
        if self.input.dagre_groups {
            return match self.direction(self.owner_of_cluster(c)) {
                Direction::TopBottom => Direction::LeftRight,
                _ => Direction::TopBottom,
            };
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
            // A loop on a container has a port for each end, like any edge.
            if a == b && !matches!(a, Member::Cluster(_)) {
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
        // In the order the container's ports were made: as ELK imports the
        // edges, breadth-first by the graph containing each (the two ends'
        // common ancestor), an edge from further out first.
        stubs.sort_by_key(|&(ei, _, _)| {
            let (a, b) = ends[ei];
            tree.common_ancestor(a, b)
                .map_or(0, |c| tree.chains[c].len())
        });
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
    let mut ext_off: ExtPorts = HashMap::new();
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
        for (&scope, done) in scopes.iter().zip(order_runs(&tree, &scopes, &inputs, None)) {
            ordered[slot(scope)] = Some(done);
        }
    }
    let ports_set: Vec<[bool; 2]> = (0..nc)
        .map(|c| ordered[c].as_ref().map_or([false; 2], |o| o.4))
        .collect();
    let import = tree.import_order();
    for &scope in &scopes {
        let mut li = level_input(
            &tree,
            &ends,
            scope,
            &box_size,
            segs.as_deref(),
            &stubs,
            &ext_off,
        );
        if elk {
            // The loops draw on the graph's random generator after its
            // crossing minimisation, before its edges are routed.
            let run = ordered[slot(scope)].as_mut();
            let (layers, rng) = match run {
                Some((_, layers, _, Some(rng), _)) => (Some(&layers[..]), rng),
                _ => (None, &mut elkorder::JavaRandom::new(1)),
            };
            (li.hier.loop_high, li.hier.loop_slot, li.hier.loop_legs) = loop_sides(
                &tree,
                &ends,
                segs.as_deref().unwrap_or_default(),
                &li,
                &ext_off,
                &ports_set,
                &import,
                rng,
                layers,
            );
            // A container's margins hold its loops' outermost legs on either
            // side (one `spacing.nodeSelfLoop` was reserved for every loop).
            let loop_ext = super::self_loop_ext(if scope.is_none() {
                ElkGraph::Root
            } else {
                ElkGraph::Container { separate: false }
            });
            // Per container: its loops, and their highest slot on the side
            // they come back to and the one they leave.
            let mut sides: Vec<(usize, usize, [usize; 2])> = Vec::new();
            for (k, seg) in li.seg_of.iter().enumerate() {
                let looped = seg.is_some_and(|s| {
                    s.from == s.to
                        && matches!(
                            (s.from, li.hier.ports.get(k)),
                            (SegEnd::Box(_), Some((Some(_), Some(_))))
                        )
                });
                if !looped {
                    continue;
                }
                let v = li.flat.edges[k].from;
                let [out, back] = li.hier.loop_legs.get(k).copied().unwrap_or_default();
                match sides.iter_mut().find(|(w, ..)| *w == v) {
                    Some((_, n, m)) => {
                        *n += 1;
                        *m = [m[0].max(back), m[1].max(out)];
                    }
                    None => sides.push((v, 1, [back, out])),
                }
            }
            for (v, n, [before, after]) in sides {
                // The first leg `spacing.nodeSelfLoop` out, each further slot
                // `spacing.edgeEdge` (20 in every graph) beyond.
                let spare = |slot: usize| {
                    loop_ext * n as f64 - (loop_ext + super::SELF_LOOP_EXT * slot as f64)
                };
                if let Some(m) = li.hier.margins.get_mut(v) {
                    *m = (m.0 - spare(before), m.1 - spare(after));
                }
            }
        }
        // A container whose run holds no other graph may be split.
        let alone = !scopes
            .iter()
            .any(|s| s.is_some_and(|c| tree.hierarchical(c) && tree.owner_of_cluster(c) == scope));
        let packed = if alone {
            layout_components(&tree, scope, &li)
        } else {
            None
        };
        let out = match (packed, ordered[slot(scope)].take()) {
            (Some(out), _) => out,
            (None, Some((prep, layers, port_rank, rng, _))) => {
                let hier = Hierarchy {
                    port_rank,
                    route_rng: rng,
                    ..li.hier.clone()
                };
                finish(&li.flat, li.graph, &hier, prep, layers)
            }
            (None, None) => layout_flat_in(&li.flat, li.graph, &li.hier),
        };
        let level = level_output(li, out);
        if let Some(c) = scope {
            let cl = &input.clusters[c];
            let (w, h) = if input.dagre_groups {
                let (along, across) = tree.dagre_group_padding(c);
                let (px, py) = if tree.direction(scope).is_horizontal() {
                    (along, across)
                } else {
                    (across, along)
                };
                (
                    (level.bbox.w + 2.0 * px).max(cl.min_width),
                    level.bbox.h + 2.0 * py,
                )
            } else {
                (
                    (level.bbox.w + 2.0 * pad).max(cl.min_width),
                    level.bbox.h + 2.0 * pad + cl.title_height,
                )
            };
            box_size[c] = (w, h);
            // Where the composition below puts the contents in the box.
            let bb = level.bbox;
            let horizontal = tree.direction(scope).is_horizontal();
            for (i, it) in level.items.iter().enumerate() {
                if let Item::Ext(ei) = *it {
                    let entering = level.ext_kinds.iter().any(|k| k.0 == i && k.1);
                    let ei = port_key(&ends, ei, entering);
                    let p = level.out.nodes[i];
                    let off = if horizontal {
                        p.y - bb.y + pad + cl.title_height - h / 2.0
                    } else {
                        p.x - bb.x + pad + (w - 2.0 * pad - bb.w) / 2.0 - w / 2.0
                    };
                    ext_off.insert((c, ei), (off, !entering));
                }
            }
            // Ports all unconnected inside: no ELK nodes there (the container
            // has no external port dummies), so ELK spreads them over their
            // side, as a node's own ports, in the order its parent's crossing
            // minimisation gives them: they are left free there. (With
            // others, they are dummies placed inside like the rest.)
            if level.ext_kinds.iter().all(|k| k.2) {
                for &(i, entering, _) in &level.ext_kinds {
                    if let Item::Ext(ei) = level.items[i] {
                        ext_off.remove(&(c, port_key(&ends, ei, entering)));
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
                        let origin = if input.dagre_groups {
                            Point::new(r.x + (r.w - bb.w) / 2.0, r.y + (r.h - bb.h) / 2.0)
                        } else {
                            Point::new(
                                r.x + pad + (r.w - 2.0 * pad - bb.w) / 2.0,
                                r.y + pad + input.clusters[b].title_height,
                            )
                        };
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
                        // A segment from a child straight to the border (its
                        // port dummy beside the child's own port) keeps the
                        // child's end: the run between them crosses the padding.
                        if route.points.len() == 1
                            && (seg.from == SegEnd::Ext) != (seg.to == SegEnd::Ext)
                        {
                            route.points.push(first);
                        }
                        if seg.from == SegEnd::Ext {
                            route.points[0] = snap(first, !seg.against);
                        }
                        if seg.to == SegEnd::Ext {
                            let at = route.points.len() - 1;
                            route.points[at] = snap(last, seg.against);
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
        if let Some(c) = anc.filter(|&c| a == Member::Cluster(c) || b == Member::Cluster(c)) {
            // An edge between a container and something inside it runs inside
            // it, between the member (or the box of the child holding it) and
            // a port dummy of the container's own (`CompoundGraphPreprocessor`).
            // The port's side is by its net flow: a child's edge flows with
            // the layering, a deeper member's against it.
            let leaving = b == Member::Cluster(c);
            let inner = if leaving { a } else { b };
            if !tree.separate[c] || inner == Member::Cluster(c) {
                continue;
            }
            let Some((up, item)) = chain(inner) else {
                continue;
            };
            let against = tree.parent_of(inner) != Some(c);
            let seg = |scope, from, to, main, against| Seg {
                edge: ei,
                scope,
                from,
                to,
                main,
                against,
            };
            if leaving {
                for (scope, from) in up {
                    out.push(seg(scope, from, SegEnd::Ext, false, false));
                }
                out.push(seg(Some(c), item, SegEnd::Ext, true, against));
            } else {
                out.push(seg(Some(c), SegEnd::Ext, item, true, against));
                for (scope, to) in up {
                    out.push(seg(scope, SegEnd::Ext, to, false, false));
                }
            }
            continue;
        }
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
                against: false,
            });
        }
        out.push(Seg {
            edge: ei,
            scope: anc,
            from: ia,
            to: ib,
            main: true,
            against: false,
        });
        for (scope, to) in up_b {
            out.push(Seg {
                edge: ei,
                scope,
                from: SegEnd::Ext,
                to,
                main: false,
                against: false,
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
/// child, port by port, a port being an edge end, made as the edges are
/// imported: breadth-first by the graph containing each, then in edge order). The
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
    // When a port was made on its node: as ELK imports the edges,
    // breadth-first by the graph containing each (the two ends' common
    // ancestor), then edge by edge.
    let made = |e: usize| {
        let (a, b) = ends[e];
        let depth = tree
            .common_ancestor(a, b)
            .map_or(0, |c| tree.chains[c].len());
        (depth, e)
    };
    let mut keyed: Vec<(bool, usize, (usize, usize), usize)> = Vec::new();
    let mut inner: HashMap<usize, Vec<usize>> = HashMap::new();
    for s in all.iter().filter(|s| s.scope == scope) {
        let (inside, out) = match (s.from, s.to) {
            (end, SegEnd::Ext) => (end, true),
            (SegEnd::Ext, end) => (end, false),
            _ => continue,
        };
        let own_end = if out { ends[s.edge].0 } else { ends[s.edge].1 };
        let key = match inside {
            SegEnd::Node(v) => (true, v, made(s.edge)),
            SegEnd::Box(c) if own_end == Member::Cluster(c) => (true, child_key(c), made(s.edge)),
            SegEnd::Box(c) => {
                let order = inner
                    .entry(c)
                    .or_insert_with(|| ext_order(tree, ends, all, Some(c)));
                let r = order
                    .iter()
                    .position(|&e| e == s.edge)
                    .unwrap_or(usize::MAX);
                (false, child_key(c), (r, 0))
            }
            SegEnd::Ext => continue,
        };
        keyed.push((key.0, key.1, key.2, s.edge));
    }
    keyed.sort();
    keyed.into_iter().map(|k| k.3).collect()
}

/// Per (container, port key): where the port's dummy stands inside, as the
/// cross-axis offset of the port from the container's centre, and whether it
/// is on the container's side towards the next layer.
type ExtPorts = HashMap<(usize, usize), (f64, bool)>;

/// ELK's port penalties of a self-loop passing a container's port
/// (`RoutingDirector`): one with edges, one without.
const CONNECTED_PORT_PENALTY: usize = 3;
const UNCONNECTED_PORT_PENALTY: usize = 1;

/// A container's port as a loop on it passes it (see [`loop_sides`]).
struct LoopPort {
    /// The port's key (see [`port_key`]).
    key: usize,
    /// Its dummy's cross-axis offset from the container's centre.
    off: f64,
    /// `CONNECTED_PORT_PENALTY` or `UNCONNECTED_PORT_PENALTY`.
    cost: usize,
    /// When ELK made it.
    made: (usize, usize),
}

/// Per edge of a level: whether it is a loop on a container whose ports keep
/// their sides that goes round the container's high cross-axis side.
///
/// Such a loop leaves on the side towards the next layer and comes back on the
/// other, round either end; ELK's `RoutingDirector` takes the way passing the
/// fewer ports (weighed by `CONNECTED_PORT_PENALTY` and
/// `UNCONNECTED_PORT_PENALTY`, the loop's own and the ports of edges to the
/// container from inside it being without edges there), and on a tie the
/// clockwise one: from the next layer's side round the high end. It counts
/// the ports in the container's port list as it stands then: on a side its
/// contents ordered (see `elkorder::RunOrder::ports_set`) as their dummies
/// stand inside, else as they were made (`PortListSorter`: those of edges to
/// the container itself as ELK imports the edges, then those of edges crossing
/// its border by their dummies, the side towards the previous layer
/// clockwise and then reversed but for its last port).
#[allow(clippy::too_many_arguments)]
fn loop_sides(
    tree: &Tree,
    ends: &[(Member, Member)],
    all: &[Seg],
    li: &LevelInput,
    ext_off: &ExtPorts,
    ports_set: &[[bool; 2]],
    import: &[usize],
    rng: &mut elkorder::JavaRandom,
    layers: Option<&[Vec<usize>]>,
) -> (Vec<bool>, Vec<usize>, Vec<[usize; 2]>) {
    let ne = ends.len();
    // Per container: its loops (level edge, their span in its port list:
    // first and last index clockwise, the list's length).
    let mut spans: Vec<(usize, Vec<LoopSpan>)> = Vec::new();
    // Per container: how many of its ports are on the side towards the next
    // layer (the first in its list).
    let mut east_len: HashMap<usize, usize> = HashMap::new();
    let within = |m: Member, c: usize| match m {
        Member::Node(v) => tree.input.nodes[v]
            .cluster
            .is_some_and(|nc| tree.chains[nc].contains(&c)),
        Member::Cluster(d) => tree.chains[d].contains(&c),
    };
    let mut high = vec![false; li.seg_of.len()];
    for (k, seg) in li.seg_of.iter().enumerate() {
        let Some(s) = seg else { continue };
        let SegEnd::Box(c) = s.from else { continue };
        if s.to != s.from {
            continue;
        }
        let fixed = li
            .items
            .iter()
            .position(|&it| it == Item::Box(c))
            .is_some_and(|v| li.hier.side_fixed.get(v).copied().unwrap_or(false));
        if !fixed {
            continue;
        }
        let inner = ext_order(tree, ends, all, Some(c));
        let mut sides: [Vec<LoopPort>; 2] = [Vec::new(), Vec::new()];
        for (&(cc, key), &(off, east)) in ext_off {
            if cc != c {
                continue;
            }
            let ei = key % ne;
            let (a, b) = ends[ei];
            let outside = |m: Member| m != Member::Cluster(c) && !within(m, c);
            let cost = if a != b && (outside(a) || outside(b)) {
                CONNECTED_PORT_PENALTY
            } else {
                UNCONNECTED_PORT_PENALTY
            };
            let made = if a == Member::Cluster(c) || b == Member::Cluster(c) {
                (0, import.get(ei).copied().unwrap_or(ei))
            } else {
                (1, inner.iter().position(|&e| e == ei).unwrap_or(usize::MAX))
            };
            sides[usize::from(!east)].push(LoopPort {
                key,
                off,
                cost,
                made,
            });
        }
        let set = ports_set.get(c).copied().unwrap_or_default();
        let [mut east, mut west] = sides;
        // ELK's list runs clockwise: the east side top down, the west bottom up.
        if set[0] {
            east.sort_by(|p, q| p.off.total_cmp(&q.off).then(p.key.cmp(&q.key)));
        } else {
            east.sort_by_key(|p| (p.made, p.key));
        }
        if set[1] {
            west.sort_by(|p, q| q.off.total_cmp(&p.off).then(p.key.cmp(&q.key)));
        } else {
            west.sort_by_key(|p| (p.made, p.key));
            let n = west.len();
            if n >= 4 {
                west[..n - 1].reverse();
            }
        }
        let list: Vec<(usize, usize)> = east.iter().chain(&west).map(|p| (p.key, p.cost)).collect();
        let at = |key: usize| list.iter().position(|p| p.0 == key);
        let (Some(a), Some(b)) = (at(s.edge), at(s.edge + ne)) else {
            continue;
        };
        east_len.insert(c, east.len());
        let (e, w) = if a < east.len() { (a, b) } else { (b, a) };
        // The penalty of going clockwise from the port at `l` to the one at
        // `r`, past the ports between them.
        let n = list.len();
        let pen = |l: usize, r: usize| -> usize {
            (1..n)
                .map(|d| (l + d) % n)
                .take_while(|&i| i != r)
                .map(|i| list[i].1)
                .sum()
        };
        high[k] = pen(e, w) <= pen(w, e);
        let span = if high[k] { (e, w) } else { (w, e) };
        let entry = (k, span.0, span.1, n);
        match spans.iter_mut().find(|(cc, _)| *cc == c) {
            Some((_, list)) => list.push(entry),
            None => spans.push((c, vec![entry])),
        }
    }
    // ELK's `SelfLoopRouter` takes the containers layer by layer, each top
    // down.
    if let Some(layers) = layers {
        let at = |c: usize| {
            let v = li.items.iter().position(|&it| it == Item::Box(c));
            layers
                .iter()
                .enumerate()
                .find_map(|(r, l)| Some((r, l.iter().position(|&u| Some(u) == v)?)))
                .unwrap_or((usize::MAX, usize::MAX))
        };
        spans.sort_by_key(|(c, _)| at(*c));
    }
    let mut slot = vec![0usize; li.seg_of.len()];
    let mut legs = vec![[0usize; 2]; li.seg_of.len()];
    for (c, loops) in &spans {
        let raw = routing_slots(loops, rng);
        let ne = east_len.get(c).copied().unwrap_or(0);
        for ((k, s), l) in raw.iter().zip(leg_slots(loops, &raw, ne)) {
            slot[*k] = *s;
            legs[*k] = l;
        }
    }
    (high, slot, legs)
}

/// `RoutingSlotAssigner.shiftTowardsNode` on the sides of a container with
/// ports (the one towards the next layer, its first `ne` ports, and the
/// other): taking the loops by their routing slots (`raw`, as
/// [`routing_slots`] gives them), each moves in to the lowest slot free at
/// every port of the side within its span. Per loop: its slots on the two
/// sides.
fn leg_slots(loops: &[LoopSpan], raw: &[(usize, usize)], ne: usize) -> Vec<[usize; 2]> {
    let mut out = vec![[0usize; 2]; loops.len()];
    let Some(&(_, _, _, n)) = loops.first() else {
        return out;
    };
    let mut order: Vec<usize> = (0..loops.len()).collect();
    order.sort_by_key(|&i| raw[i].1);
    let mut next_free = vec![0usize; n];
    for (side, ports) in [(0, 0..ne.min(n)), (1, ne.min(n)..n)] {
        for &i in &order {
            let (_, a, b, n) = loops[i];
            let active: Vec<usize> = ports
                .clone()
                .filter(|&p| (p + n - a) % n <= (b + n - a) % n)
                .collect();
            let s = active.iter().map(|&p| next_free[p]).max().unwrap_or(0);
            out[i][side] = s;
            for p in active {
                next_free[p] = s + 1;
            }
        }
    }
    out
}

/// A loop on a container: its level edge, the first and last index of its
/// span clockwise in the container's port list, the list's length.
type LoopSpan = (usize, usize, usize, usize);

/// ELK's `RoutingSlotAssigner` for the loops on one container (each: its
/// level edge, the first and last index of its span clockwise in the port
/// list, the list's length): a loop goes outside another where that crosses
/// fewer of their ports (a loop crosses those of the other's within its own
/// span), by the difference; a tie with crossings either way makes a
/// two-cycle of no weight. ELK breaks the cycles as its orthogonal router does
/// (`breakNonCriticalCycles`, drawing on the graph's random generator `rng`);
/// slots count up from the node, each loop one beyond the loops it must stay
/// outside of.
fn routing_slots(loops: &[LoopSpan], rng: &mut elkorder::JavaRandom) -> Vec<(usize, usize)> {
    let active = |l: &LoopSpan, i: usize| {
        let (_, a, b, n) = *l;
        (i + n - a) % n <= (b + n - a) % n
    };
    // `crossings(u, l)`: ports of loop `u` within the span of loop `l`, were
    // `u` routed outside it.
    let crossings = |u: usize, l: usize| {
        usize::from(active(&loops[l], loops[u].1)) + usize::from(active(&loops[l], loops[u].2))
    };
    let m = loops.len();
    // Dependencies (outer loop, inner loop, weight), in the order ELK makes them.
    let mut deps: Vec<(usize, usize, usize)> = Vec::new();
    for i in 0..m {
        for j in i + 1..m {
            let (ij, ji) = (crossings(i, j), crossings(j, i));
            if ij < ji {
                deps.push((i, j, ji - ij));
            } else if ji < ij {
                deps.push((j, i, ij - ji));
            } else if ij > 0 {
                deps.push((i, j, 0));
                deps.push((j, i, 0));
            }
        }
    }
    let cyclic = cycle_dependencies(m, &deps, rng);
    let mut outside: Vec<Vec<usize>> = vec![Vec::new(); m];
    for (d, &(a, b, w)) in deps.iter().enumerate() {
        match (cyclic.contains(&d), w) {
            (false, _) => outside[a].push(b),
            (true, 0) => {}
            (true, _) => outside[b].push(a),
        }
    }
    let mut slot = vec![None; m];
    fn level(i: usize, outside: &[Vec<usize>], slot: &mut [Option<usize>]) -> usize {
        if let Some(s) = slot[i] {
            return s;
        }
        // A cycle left over (none should be) counts no further.
        slot[i] = Some(0);
        let s = outside[i]
            .iter()
            .map(|&j| level(j, outside, slot) + 1)
            .max()
            .unwrap_or(0);
        slot[i] = Some(s);
        s
    }
    (0..m)
        .map(|i| (loops[i].0, level(i, &outside, &mut slot)))
        .collect()
}

/// ELK's `HyperEdgeCycleDetector.detectCycles` (all dependencies, not only
/// critical ones) over `m` segments and their dependencies (source, target,
/// weight): Eades' greedy linear ordering, sinks and sources first, else the
/// segment of the greatest outflow (a random one of several). The indices of
/// the dependencies that run against the ordering.
fn cycle_dependencies(
    m: usize,
    deps: &[(usize, usize, usize)],
    rng: &mut elkorder::JavaRandom,
) -> Vec<usize> {
    let w = |d: usize| deps[d].2 as i64;
    let outgoing: Vec<Vec<usize>> = (0..m)
        .map(|s| (0..deps.len()).filter(|&d| deps[d].0 == s).collect())
        .collect();
    let incoming: Vec<Vec<usize>> = (0..m)
        .map(|s| (0..deps.len()).filter(|&d| deps[d].1 == s).collect())
        .collect();
    let mut mark: Vec<i64> = (0..m).map(|s| -(s as i64) - 1).collect();
    let mut in_w: Vec<i64> = (0..m)
        .map(|s| incoming[s].iter().map(|&d| w(d)).sum())
        .collect();
    let mut out_w: Vec<i64> = (0..m)
        .map(|s| outgoing[s].iter().map(|&d| w(d)).sum())
        .collect();
    let mut sinks = std::collections::VecDeque::new();
    let mut sources = std::collections::VecDeque::new();
    for s in 0..m {
        if out_w[s] == 0 {
            sinks.push_back(s);
        } else if in_w[s] == 0 {
            sources.push_back(s);
        }
    }
    // The unprocessed segments, in the order of their (initial) marks: the
    // last segment first.
    let mut unprocessed: Vec<usize> = (0..m).rev().collect();
    let base = m as i64;
    let (mut next_sink, mut next_source) = (base - 1, base + 1);
    let update = |s: usize,
                  mark: &[i64],
                  in_w: &mut [i64],
                  out_w: &mut [i64],
                  sources: &mut std::collections::VecDeque<usize>,
                  sinks: &mut std::collections::VecDeque<usize>| {
        for &d in &outgoing[s] {
            let t = deps[d].1;
            if mark[t] < 0 && w(d) > 0 {
                in_w[t] -= w(d);
                if in_w[t] <= 0 && out_w[t] > 0 {
                    sources.push_back(t);
                }
            }
        }
        for &d in &incoming[s] {
            let t = deps[d].0;
            if mark[t] < 0 && w(d) > 0 {
                out_w[t] -= w(d);
                if out_w[t] <= 0 && in_w[t] > 0 {
                    sinks.push_back(t);
                }
            }
        }
    };
    while !unprocessed.is_empty() {
        while let Some(s) = sinks.pop_front() {
            unprocessed.retain(|&u| u != s);
            mark[s] = next_sink;
            next_sink -= 1;
            update(s, &mark, &mut in_w, &mut out_w, &mut sources, &mut sinks);
        }
        while let Some(s) = sources.pop_front() {
            unprocessed.retain(|&u| u != s);
            mark[s] = next_source;
            next_source += 1;
            update(s, &mark, &mut in_w, &mut out_w, &mut sources, &mut sinks);
        }
        let mut best: Vec<usize> = Vec::new();
        let mut max_outflow = i64::MIN;
        for &u in &unprocessed {
            let outflow = out_w[u] - in_w[u];
            if outflow >= max_outflow {
                if outflow > max_outflow {
                    best.clear();
                    max_outflow = outflow;
                }
                best.push(u);
            }
        }
        if !best.is_empty() {
            let s = best[rng.next_int(best.len() as i32) as usize];
            unprocessed.retain(|&u| u != s);
            mark[s] = next_source;
            next_source += 1;
            update(s, &mark, &mut in_w, &mut out_w, &mut sources, &mut sinks);
        }
    }
    for mk in &mut mark {
        if *mk < base {
            *mk += base + 1;
        }
    }
    (0..m)
        .flat_map(|s| outgoing[s].iter().copied())
        .filter(|&d| mark[deps[d].0] > mark[deps[d].1])
        .collect()
}

/// The key of an edge's port on a container: the edge, but for the port
/// where a loop on the container comes back in (a loop has two ports there).
fn port_key(ends: &[(Member, Member)], ei: usize, entering: bool) -> usize {
    let looped = ends.get(ei).is_some_and(|(a, b)| a == b);
    if looped && entering {
        ei + ends.len()
    } else {
        ei
    }
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
    ext_off: &ExtPorts,
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
        .filter_map(|s| Some((*ext_item.get(&s.edge)?, s.ext_first()?, false)))
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
        Some(_) if input.dagre_groups => l,
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
    let mut inner_order: HashMap<usize, Vec<usize>> = HashMap::new();
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
    let mut looped = Vec::new();
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
        let port = |x: SegEnd, entering: bool| match x {
            SegEnd::Box(c) => ext_off
                .get(&(c, port_key(ends, s.edge, entering)))
                .map(|p| p.0),
            _ => None,
        };
        let ports = (port(s.from, false), port(s.to, true));
        hier.ports.push(ports);
        // A loop on a container with ports of fixed sides leaves it on the far
        // side and comes back round to the near one, standing off both
        // (`SelfLoopRouter`), which widens its layer.
        if s.from == s.to && matches!((s.from, ports), (SegEnd::Box(_), (Some(_), Some(_)))) {
            looped.push(from);
        }
        // A box's port for an edge crossing its border is made after those
        // for edges to the box itself, in its dummy's order inside.
        let k = edges.len();
        let (a, b) = ends[s.edge];
        let mut made = |x: SegEnd, m: Member| match x {
            SegEnd::Box(c) if m != Member::Cluster(c) => {
                let order = inner_order.entry(c).or_insert_with(|| {
                    all_segs.map_or_else(Vec::new, |all| ext_order(tree, ends, all, Some(c)))
                });
                (
                    1,
                    order
                        .iter()
                        .position(|&e| e == s.edge)
                        .unwrap_or(usize::MAX),
                )
            }
            _ => (0, k),
        };
        hier.port_made.push([made(s.from, a), made(s.to, b)]);
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
        if let Some(first) = s.ext_first() {
            hier.ext
                .push((if s.from == SegEnd::Ext { from } else { to }, first));
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
    // The loop's legs stand off the ports on either side, those of edges
    // crossing the border included.
    let loop_ext = super::self_loop_ext(if scope.is_none() {
        ElkGraph::Root
    } else {
        ElkGraph::Container { separate: false }
    });
    for v in looped {
        hier.margins.resize(items.len(), (0.0, 0.0));
        let m = &mut hier.margins[v];
        *m = (m.0 + loop_ext, m.1 + loop_ext);
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
        Some(_) if input.dagre_groups => (
            input.node_sep,
            input.rank_sep + 25.0 * tree.depth(scope) as f64,
        ),
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
        // The root of a run of its own that includes its children is laid
        // out as a compound graph (`doCompoundLayout`) like any graph of a
        // hierarchical run.
        Some(c) => ElkGraph::Container {
            separate: !tree.hierarchical(c) && !tree.crossed[c],
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
    // ELK sizes a graph by everything in its layers, edges included: an edge
    // runs through a layer as a dummy node as thick as the edge, its port (so
    // the edge) on the dummy's side towards the lower cross coordinate
    // (`LongEdgeSplitter`: at the floor of half the thickness).
    if elk {
        let thick = if flat.direction.is_horizontal() {
            (0.0, super::ELK_EDGE_THICKNESS)
        } else {
            (super::ELK_EDGE_THICKNESS, 0.0)
        };
        let horizontal = flat.direction.is_horizontal();
        let rects: Vec<Rect> = (flat.nodes.iter().zip(&out.nodes))
            .map(|(nd, p)| Rect::centered(*p, nd.width, nd.height))
            .collect();
        for (e, route) in flat.edges.iter().zip(&out.edges) {
            let (w, h) = if e.from == e.to { (0.0, 0.0) } else { thick };
            let thin = inverted_port_runs(&route.points, [e.from, e.to], &rects, horizontal);
            for (p, thin) in route.points.iter().zip(thin) {
                if thin {
                    add(Rect::new(p.x, p.y, 0.0, 0.0));
                } else {
                    add(Rect::new(p.x, p.y, w, h));
                }
            }
        }
    }
    for (e, route) in flat.edges.iter().zip(&out.edges) {
        if let (Some((w, h)), Some(p)) = (e.label, route.label) {
            let r = Rect::centered(p, w, h);
            // Across a horizontal layout ELK's label dummy is the edge's
            // thickness taller than its label, which sits at the dummy's top
            // (`LabelDummyInserter`).
            if elk && flat.direction.is_horizontal() {
                add(Rect::new(r.x, r.y, w, h + super::ELK_EDGE_THICKNESS));
            } else {
                add(r);
            }
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
        // Their layers count along the flow too: a layer of them past a
        // routing channel stands the channel's spacing beyond it.
        for k in &ext_kinds {
            let p = out.nodes[k.0];
            let (x0, y0) = (bbox.x.min(p.x), bbox.y.min(p.y));
            let (x1, y1) = (bbox.right().max(p.x), bbox.bottom().max(p.y));
            if horizontal {
                bbox = Rect::new(x0, bbox.y, x1 - x0, bbox.h);
            } else {
                bbox = Rect::new(bbox.x, y0, bbox.w, y1 - y0);
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

/// Which points of an ELK route lie on the run through an inverted port's
/// dummy, which `InvertedPortProcessor` makes of no size (unlike the long-edge
/// dummies, as thick as their edge): where the edge leaves a node's port away
/// from the flow, turns, and runs back past the node in the node's own layer
/// (the run spans the node along the flow), to or from its dummy there. Only
/// where that run reaches no further layer (no other node lies along it, clear
/// of the node), so no long-edge dummy shares it.
fn inverted_port_runs(
    points: &[Point],
    ends: [usize; 2],
    nodes: &[Rect],
    horizontal: bool,
) -> Vec<bool> {
    let mut thin = vec![false; points.len()];
    let n = points.len();
    if n < 4 {
        return thin;
    }
    let flow = |p: Point| if horizontal { p.x } else { p.y };
    let cross = |p: Point| if horizontal { p.y } else { p.x };
    let span = |r: &Rect| {
        if horizontal {
            (r.x, r.right())
        } else {
            (r.y, r.bottom())
        }
    };
    for (end, idx) in [
        (ends[0], [0, 1, 2, 3]),
        (ends[1], [n - 1, n - 2, n - 3, n - 4]),
    ] {
        let [p0, p1, p2, p3] = idx.map(|i| points[i]);
        let along = |a: Point, b: Point| (cross(a) - cross(b)).abs() < 1e-9;
        if !(along(p0, p1) && !along(p1, p2) && along(p2, p3)) {
            continue;
        }
        let (lo, hi) = span(&nodes[end]);
        let (a, b) = (flow(p2).min(flow(p3)), flow(p2).max(flow(p3)));
        if !(a < lo && b > hi) {
            continue;
        }
        let further = nodes.iter().enumerate().any(|(k, r)| {
            let (l, h) = span(r);
            k != end && r.w > 0.0 && r.h > 0.0 && l >= a && h <= b && (h <= lo || l >= hi)
        });
        if !further {
            thin[idx[2]] = true;
            thin[idx[3]] = true;
        }
    }
    thin
}

/// mermaid's `elk.spacing.portsSurrounding` (12 on every side).
const ELK_PORTS_SURROUNDING: f64 = 12.0;

/// ELK's `spacing.componentComponent` in a container: derived (factor 1) from
/// the `spacing.baseValue: 24` mermaid gives every container.
const ELK_COMPONENT_SPACING: f64 = 24.0;

/// ELK Layered's default `aspectRatio` (a float option).
const ELK_ASPECT_RATIO: f32 = 1.6;

/// The connected components of a level's pass, as ELK's `ComponentsProcessor`
/// finds them: a depth-first search from each node in turn (in the graph's node
/// order), following each node's ports in the order the importer made them (by
/// the edge's source, then the edge), so each component lists its nodes in the
/// order the search reached them, which is the order ELK lays it out in.
/// Per component: its nodes and its edges (in their order).
fn components(flat: &LayoutInput) -> Vec<(Vec<usize>, Vec<usize>)> {
    let n = flat.nodes.len();
    let mut ports: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut by_source: Vec<usize> = (0..flat.edges.len()).collect();
    by_source.sort_by_key(|&e| flat.edges[e].from);
    for e in by_source {
        let edge = &flat.edges[e];
        ports[edge.from].push(e);
        ports[edge.to].push(e);
    }
    let mut seen = vec![false; n];
    let mut comp_of = vec![usize::MAX; n];
    let mut out: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let k = out.len();
        let mut nodes = Vec::new();
        // (node, next port to follow)
        let mut stack = vec![(start, 0usize)];
        seen[start] = true;
        nodes.push(start);
        comp_of[start] = k;
        while let Some(top) = stack.last_mut() {
            let (v, i) = *top;
            let Some(&e) = ports[v].get(i) else {
                stack.pop();
                continue;
            };
            top.1 += 1;
            let edge = &flat.edges[e];
            for w in [edge.from, edge.to] {
                if w != v || edge.from == edge.to {
                    if !seen[w] {
                        seen[w] = true;
                        nodes.push(w);
                        comp_of[w] = k;
                        stack.push((w, 0));
                    }
                    break;
                }
            }
        }
        out.push((nodes, Vec::new()));
    }
    for (e, edge) in flat.edges.iter().enumerate() {
        out[comp_of[edge.from]].1.push(e);
    }
    out
}

/// ELK's `SimpleRowGraphPlacer`: components (sizes `(w, h)`) sorted by area,
/// smallest first (a stable sort: mermaid gives no node a priority), then put
/// left to right in rows no wider than the widest component or the square root
/// of their total area times the aspect ratio, `spacing.componentComponent`
/// apart. Per component (in the input order): the top left corner of its box.
fn place_components(sizes: &[(f64, f64)]) -> Vec<Point> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| {
        let (sa, sb) = (sizes[a].0 * sizes[a].1, sizes[b].0 * sizes[b].1);
        sa.total_cmp(&sb)
    });
    let widest = sizes.iter().map(|s| s.0).fold(0.0, f64::max);
    let area: f64 = sizes.iter().map(|s| s.0 * s.1).sum();
    let max_row = widest.max(f64::from(area.sqrt() as f32) * f64::from(ELK_ASPECT_RATIO));
    let mut at = vec![Point::new(0.0, 0.0); sizes.len()];
    let (mut x, mut y, mut highest) = (0.0, 0.0, 0.0f64);
    for i in order {
        let (w, h) = sizes[i];
        if x + w > max_row {
            x = 0.0;
            y += highest + ELK_COMPONENT_SPACING;
            highest = 0.0;
        }
        at[i] = Point::new(x, y);
        highest = highest.max(h);
        x += w + ELK_COMPONENT_SPACING;
    }
    at
}

/// A container laid out on its own (`SEPARATE_CHILDREN`) is laid out by ELK one
/// connected component at a time (`ElkLayered.doLayout`: `ComponentsProcessor`
/// splits it, each component is a graph of its own, and `SimpleRowGraphPlacer`
/// packs them). `None` where the level is one component (or not such a graph).
fn layout_components(tree: &Tree, scope: Option<usize>, li: &LevelInput) -> Option<LayoutOutput> {
    if !li.elk
        || li.graph != (ElkGraph::Container { separate: true })
        || !li.flat.clusters.is_empty()
        || !li.hier.ext.is_empty()
        || li.items.iter().any(|it| matches!(it, Item::Ext(_)))
    {
        return None;
    }
    let comps = components(&li.flat);
    if comps.len() < 2 {
        return None;
    }
    let subs: Vec<LevelInput> = comps
        .iter()
        .map(|(nodes, edges)| {
            let mut index = vec![usize::MAX; li.flat.nodes.len()];
            for (k, &v) in nodes.iter().enumerate() {
                index[v] = k;
            }
            let pick = |v: &[(f64, f64)]| -> Vec<(f64, f64)> {
                if v.is_empty() {
                    Vec::new()
                } else {
                    nodes.iter().map(|&i| v[i]).collect()
                }
            };
            let hier = Hierarchy {
                ports: edges
                    .iter()
                    .map(|&e| li.hier.ports.get(e).copied().unwrap_or((None, None)))
                    .collect(),
                margins: pick(&li.hier.margins),
                side_fixed: nodes
                    .iter()
                    .map(|&i| li.hier.side_fixed.get(i).copied().unwrap_or(false))
                    .collect(),
                ..Hierarchy::default()
            };
            LevelInput {
                items: nodes.iter().map(|&i| li.items[i]).collect(),
                flat: LayoutInput {
                    nodes: nodes.iter().map(|&i| li.flat.nodes[i].clone()).collect(),
                    edges: edges
                        .iter()
                        .map(|&e| LayoutEdge {
                            from: index[li.flat.edges[e].from],
                            to: index[li.flat.edges[e].to],
                            ..li.flat.edges[e].clone()
                        })
                        .collect(),
                    ..li.flat.clone()
                },
                hier,
                graph: li.graph,
                flat_clusters: Vec::new(),
                edge_ids: edges.iter().map(|&e| li.edge_ids[e]).collect(),
                seg_of: edges.iter().map(|&e| li.seg_of[e]).collect(),
                ext_kinds: Vec::new(),
                elk: true,
            }
        })
        .collect();
    // The components share the graph's random generator (`GraphConfigurator`
    // makes it before the split), each drawing on where the last left it.
    let mut rng = elkorder::JavaRandom::new(1);
    let mut levels = Vec::with_capacity(subs.len());
    for sub in subs {
        let (prep, layers, port_rank, ordered, _) =
            order_runs(tree, &[scope], std::slice::from_ref(&sub), Some(&rng))
                .pop()
                .expect("one level");
        let hier = Hierarchy {
            port_rank,
            route_rng: ordered,
            ..sub.hier.clone()
        };
        let (out, after) = finish_rng(&sub.flat, sub.graph, &hier, prep, layers);
        // A lone node is not ordered at all (`LayerSweepCrossingMinimizer`'s
        // short cut for a graph laid out on its own), so draws nothing.
        if sub.flat.nodes.len() > 1 {
            rng = after;
        }
        levels.push(level_output(sub, out));
    }
    let sizes: Vec<(f64, f64)> = levels.iter().map(|l| (l.bbox.w, l.bbox.h)).collect();
    let at = place_components(&sizes);
    let mut out = LayoutOutput {
        nodes: vec![Point::new(0.0, 0.0); li.flat.nodes.len()],
        edges: vec![
            EdgeRoute {
                points: Vec::new(),
                label: None,
            };
            li.flat.edges.len()
        ],
        clusters: Vec::new(),
        cluster_directions: Vec::new(),
    };
    for ((level, (nodes, edges)), corner) in levels.iter().zip(&comps).zip(at) {
        let shift =
            |p: Point| Point::new(p.x - level.bbox.x + corner.x, p.y - level.bbox.y + corner.y);
        for (&v, &p) in nodes.iter().zip(&level.out.nodes) {
            out.nodes[v] = shift(p);
        }
        for (&e, r) in edges.iter().zip(&level.out.edges) {
            out.edges[e] = EdgeRoute {
                points: r.points.iter().map(|&p| shift(p)).collect(),
                label: r.label.map(shift),
            };
        }
    }
    Some(out)
}

/// ELK's crossing minimisation for every level, the levels of one ELK run
/// (the root, or a container laid out on its own, with the containers laid out
/// within it: `INCLUDE_CHILDREN`) ordered together (`elkorder::minimize_run`).
/// Per level (in `scopes`' order): its prepared pass and its ordered layers.
fn order_runs(
    tree: &Tree,
    scopes: &[Option<usize>],
    inputs: &[LevelInput],
    rng: Option<&elkorder::JavaRandom>,
) -> Vec<RunLevel> {
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
        let (setup, layers) =
            ElkLayers::new(p.layers.clone(), (&p.verts, &p.created), &p.chains, &p.inl);
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
    let import = tree.import_order();
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
                    import: &import,
                    parent: up,
                }
            })
            .collect();
        let run = match rng {
            Some(r) => elkorder::minimize_run_from(graphs, r.clone()),
            None => elkorder::minimize_run(graphs),
        };
        for (&i, o) in members.iter().zip(run) {
            ordered[i] = Some(o);
        }
    }
    #[allow(clippy::type_complexity)]
    let complete: Vec<(
        Vec<Vec<usize>>,
        PortRanks,
        Option<elkorder::JavaRandom>,
        [bool; 2],
    )> = setups
        .into_iter()
        .zip(ordered)
        .enumerate()
        .map(|(i, (setup, o))| {
            let (layers, seg_rank, rng, ports_set) = match o {
                Some(o) => (o.layers, o.port_rank, Some(o.rng), o.ports_set),
                None => (Vec::new(), Vec::new(), None, [false; 2]),
            };
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
            (layers, rank, rng, ports_set)
        })
        .collect();
    drop(ctxs);
    preps
        .into_iter()
        .zip(complete)
        .map(|(p, (l, r, g, s))| (p, l, r, g, s))
        .collect()
}

/// Per edge: the ranks of its ports in ELK's port lists (see
/// `elkorder::RunOrder::port_rank`).
type PortRanks = Vec<(Option<usize>, Option<usize>)>;

/// One level of an ELK run, ordered: its prepared pass, its layers, its
/// port ranks, its random generator and which sides of its container's port
/// list follow its port dummies (`elkorder::RunOrder::ports_set`).
type RunLevel = (
    Prepared,
    Vec<Vec<usize>>,
    PortRanks,
    Option<elkorder::JavaRandom>,
    [bool; 2],
);

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{
        Item, LevelInput, components, even_group_frames, inverted_port_runs, level_output,
        place_components, subgraph_feedback_edges,
    };

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
            dagre_groups: false,
            dagre: None,
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

    /// ELK's `RoutingSlotAssigner.shiftTowardsNode` on the sides with ports.
    #[test]
    fn leg_slots_move_in_where_the_ports_spanned_are_free() {
        use crate::layout::nested::leg_slots;
        // Port list of 6, the first 3 on the side towards the next layer. Loop
        // 1 (raw slot 0) spans 0..=4, loop 2 (raw slot 1) 2..=5: on the near
        // side they share port 2 (loop 2 one further out), on the far one
        // port 4 (loop 2 again).
        assert_eq!(
            leg_slots(&[(1, 0, 4, 6), (2, 2, 5, 6)], &[(1, 0), (2, 1)], 3),
            vec![[0, 0], [1, 1]]
        );
        // Loop 2 (raw slot 1) spans 4..=0, loop 1 (raw slot 0) 1..=3: they
        // share no port on either side, so loop 2 moves in next to the node.
        assert_eq!(
            leg_slots(&[(1, 1, 3, 6), (2, 4, 0, 6)], &[(1, 0), (2, 1)], 3),
            vec![[0, 0], [0, 0]]
        );
    }

    /// ELK's `RoutingSlotAssigner`: a loop whose span holds another's ports
    /// goes outside it; where both hold each other's (a tie with crossings)
    /// the earlier stays inside.
    #[test]
    fn routing_slots_nest_loops_by_their_spans() {
        let mut rng = elkorder::JavaRandom::new(1);
        // Port list of 6: loop 7 spans 0..=5, loop 8 spans 1..=4.
        assert_eq!(
            crate::layout::nested::routing_slots(&[(7, 0, 5, 6), (8, 1, 4, 6)], &mut rng),
            vec![(7, 1), (8, 0)]
        );
        assert_eq!(
            crate::layout::nested::routing_slots(&[(8, 1, 4, 6), (7, 0, 5, 6)], &mut rng),
            vec![(8, 0), (7, 1)]
        );
        // Interleaved: 0..=2 and 1..=3.
        assert_eq!(
            crate::layout::nested::routing_slots(&[(1, 0, 2, 4), (2, 1, 3, 4)], &mut rng),
            vec![(1, 0), (2, 1)]
        );
        // Apart: no crossings, both next to the node.
        assert_eq!(
            crate::layout::nested::routing_slots(&[(1, 0, 1, 4), (2, 2, 3, 4)], &mut rng),
            vec![(1, 0), (2, 0)]
        );
        // Four loops on a port list of 9: loop 2 must stay outside loop 1, and
        // loop 4 ties with each of the others. ELK orders the sinks (1, 3, 4)
        // after the source (2): loop 4 goes outside 1 and 3, inside 2.
        assert_eq!(
            crate::layout::nested::routing_slots(
                &[(1, 7, 0, 9), (2, 6, 1, 9), (3, 2, 5, 9), (4, 8, 3, 9)],
                &mut rng
            ),
            vec![(1, 0), (2, 2), (3, 0), (4, 1)]
        );
    }

    #[test]
    fn cycle_breaking_draws_on_the_random_generator() {
        // A three-cycle of equal weights: no sink or source, so ELK starts at
        // a random one of them (the generator's first `nextInt(3)`); the
        // dependency into it runs against the ordering.
        let deps = [(0, 1, 1), (1, 2, 1), (2, 0, 1)];
        let mut rng = elkorder::JavaRandom::new(1);
        let first = [2, 1, 0][elkorder::JavaRandom::new(1).next_int(3) as usize];
        let into = (0..3).find(|&d| deps[d].1 == first).unwrap();
        assert_eq!(super::cycle_dependencies(3, &deps, &mut rng), vec![into]);
        // The generator moved on by that draw.
        let mut after = elkorder::JavaRandom::new(1);
        after.next_int(3);
        assert_eq!(rng.next_long(), after.next_long());
    }

    #[test]
    fn components_follow_ports_depth_first() {
        // 1 -> 0, 0 -> 2, 3 alone, 4 -> 1: from node 0 the search takes its
        // ports in the order they were made (by the edge's source): 0 -> 2
        // first, so 2 before 1 (and then 4).
        let inp = input(
            vec![node(None); 5],
            vec![
                LayoutEdge::new(1, 0, 1, None),
                LayoutEdge::new(0, 2, 1, None),
                LayoutEdge::new(4, 1, 1, None),
            ],
            vec![],
            Direction::TopBottom,
        );
        let comps = components(&inp);
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].0, vec![0, 2, 1, 4]);
        assert_eq!(comps[0].1, vec![0, 1, 2]);
        assert_eq!(comps[1], (vec![3], vec![]));
    }

    #[test]
    fn components_pack_in_rows_smallest_first() {
        // Areas 400x100, 100x50, 300x100: the smallest first, then the
        // others while a row stays within max(400, sqrt(75000) * 1.6 = 438).
        let at = place_components(&[(400.0, 100.0), (100.0, 50.0), (300.0, 100.0)]);
        assert_eq!(at[1], Point::new(0.0, 0.0));
        assert_eq!(at[2], Point::new(124.0, 0.0));
        assert_eq!(at[0], Point::new(0.0, 124.0));
        // Equal areas keep their order; rows wrap past sqrt(8000) * 1.6 = 143.
        let at = place_components(&[(100.0, 40.0), (40.0, 100.0)]);
        assert_eq!(at, vec![Point::new(0.0, 0.0), Point::new(0.0, 64.0)]);
    }

    #[test]
    fn elk_container_lays_out_its_components_apart() {
        // An LR container (no edge crosses it) holding a chain of three and a
        // chain of two: ELK lays each out on its own and stacks the rows, the
        // smaller first, `spacing.componentComponent` (24) apart.
        let mut inp = input(
            vec![node(Some(0)); 5],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(1, 2, 1, None),
                LayoutEdge::new(3, 4, 1, None),
            ],
            vec![with_dir(None, Some(Direction::LeftRight))],
            Direction::TopBottom,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        let n = &out.nodes;
        // Each chain is a row, the two-node chain on top.
        assert!((n[0].y - n[1].y).abs() < 1e-6 && (n[1].y - n[2].y).abs() < 1e-6);
        assert!((n[3].y - n[4].y).abs() < 1e-6);
        assert!((n[0].y - n[3].y - (40.0 + 24.0)).abs() < 1e-6, "{n:?}");
        // Both rows start at the container's left.
        assert!((n[0].x - n[3].x).abs() < 1e-6);
    }

    #[test]
    fn elk_graph_size_takes_in_long_edge_dummies() {
        // A TB graph whose edge runs right of both nodes (through a layer as a
        // dummy one unit thick, the edge on its left side): the graph reaches
        // one unit past the edge. A self-loop stands off its node, no dummy.
        let mut flat = input(
            vec![node(None), node(None)],
            vec![
                LayoutEdge::new(0, 1, 1, None),
                LayoutEdge::new(1, 1, 1, None),
            ],
            vec![],
            Direction::TopBottom,
        );
        flat.routing = EdgeRouting::Orthogonal(OrthoSpacing::CONTAINER);
        let route = |pts: &[(f64, f64)]| EdgeRoute {
            points: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            label: None,
        };
        let out = LayoutOutput {
            nodes: vec![Point::new(40.0, 20.0), Point::new(40.0, 200.0)],
            edges: vec![
                route(&[(60.0, 40.0), (100.0, 40.0), (100.0, 180.0), (60.0, 180.0)]),
                route(&[(80.0, 190.0), (130.0, 190.0), (130.0, 210.0), (80.0, 210.0)]),
            ],
            clusters: vec![],
            cluster_directions: vec![],
        };
        let li = LevelInput {
            items: vec![Item::Node(0), Item::Node(1)],
            flat,
            hier: Hierarchy::default(),
            graph: ElkGraph::Container { separate: true },
            flat_clusters: vec![],
            edge_ids: vec![0, 1],
            seg_of: vec![None, None],
            ext_kinds: vec![],
            elk: true,
        };
        let level = level_output(li, out);
        assert_eq!(level.bbox.right(), 130.0);
        let mut out2 = level.out.clone();
        out2.edges[1] = route(&[(80.0, 190.0), (90.0, 190.0), (90.0, 210.0), (80.0, 210.0)]);
        let li = LevelInput {
            items: level.items.clone(),
            flat: {
                let mut f = input(
                    vec![node(None), node(None)],
                    vec![
                        LayoutEdge::new(0, 1, 1, None),
                        LayoutEdge::new(1, 1, 1, None),
                    ],
                    vec![],
                    Direction::TopBottom,
                );
                f.routing = EdgeRouting::Orthogonal(OrthoSpacing::CONTAINER);
                f
            },
            hier: Hierarchy::default(),
            graph: ElkGraph::Container { separate: true },
            flat_clusters: vec![],
            edge_ids: vec![0, 1],
            seg_of: vec![None, None],
            ext_kinds: vec![],
            elk: true,
        };
        assert_eq!(level_output(li, out2).bbox.right(), 101.0);
    }

    #[test]
    fn inverted_port_dummy_has_no_thickness() {
        // LR: an edge from node 1 (right) back into node 0's west side runs
        // round below node 0 through its inverted port's dummy in node 0's
        // layer: that run carries no thickness. Once the run reaches past
        // another layer's node (node 2), a long-edge dummy shares it.
        let pts = [
            (260.0, 20.0),
            (230.0, 20.0),
            (230.0, 80.0),
            (20.0, 80.0),
            (20.0, 20.0),
            (40.0, 20.0),
        ]
        .map(|(x, y)| Point::new(x, y));
        let rect = |x: f64| Rect::new(x, 0.0, 80.0, 40.0);
        let nodes = [rect(40.0), rect(260.0)];
        let thin = inverted_port_runs(&pts, [1, 0], &nodes, true);
        assert_eq!(thin, [false, false, true, true, false, false]);
        let nodes = [rect(40.0), rect(260.0), rect(140.0)];
        assert!(!inverted_port_runs(&pts, [1, 0], &nodes, true).contains(&true));
        // An ordinary back edge, leaving east of node 0, carries its thickness.
        let pts = [(120.0, 20.0), (140.0, 20.0), (140.0, 80.0), (240.0, 80.0)]
            .map(|(x, y)| Point::new(x, y));
        assert!(
            !inverted_port_runs(&pts, [0, 1], &[rect(40.0), rect(260.0)], true).contains(&true)
        );
    }

    #[test]
    fn elk_graph_size_takes_in_label_dummies() {
        // An LR graph whose labelled edge runs below both nodes: its label
        // (21 tall, at the top of its dummy) reaches the graph's bottom, and
        // the dummy is the edge's thickness taller than the label.
        let level = |direction: Direction| {
            let mut flat = input(
                vec![node(None), node(None)],
                vec![LayoutEdge::new(0, 1, 1, Some((40.0, 21.0)))],
                vec![],
                direction,
            );
            flat.routing = EdgeRouting::Orthogonal(OrthoSpacing::CONTAINER);
            let out = LayoutOutput {
                nodes: vec![Point::new(40.0, 20.0), Point::new(240.0, 20.0)],
                edges: vec![EdgeRoute {
                    points: vec![Point::new(80.0, 30.0), Point::new(200.0, 30.0)],
                    label: Some(Point::new(140.0, 60.5)),
                }],
                clusters: vec![],
                cluster_directions: vec![],
            };
            let li = LevelInput {
                items: vec![Item::Node(0), Item::Node(1)],
                flat,
                hier: Hierarchy::default(),
                graph: ElkGraph::Container { separate: true },
                flat_clusters: vec![],
                edge_ids: vec![0],
                seg_of: vec![None],
                ext_kinds: vec![],
                elk: true,
            };
            level_output(li, out).bbox
        };
        assert_eq!(level(Direction::LeftRight).bottom(), 72.0);
        assert_eq!(level(Direction::TopBottom).bottom(), 71.0);
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

    /// A member whose only edges leave its container takes ELK's `LAST` layer
    /// constraint (`LayerConstraintPreprocessor`): it sits on the container's last
    /// layer, not on its first (`subgraph S0: a -->|x| b; c end; b --> x; c --> x`).
    #[test]
    fn elk_member_only_leaving_its_container_sits_on_its_last_layer() {
        let inp = elk(input(
            vec![node(Some(0)), node(Some(0)), node(Some(0)), node(None)],
            vec![
                LayoutEdge::new(0, 1, 1, Some((30.0, 20.0))),
                LayoutEdge::new(2, 3, 1, None),
                LayoutEdge::new(1, 3, 1, None),
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        assert!((out.nodes[2].y - out.nodes[1].y).abs() < 1e-6, "{out:?}");
        assert!(out.nodes[0].y < out.nodes[1].y, "{out:?}");
    }

    /// mermaid's dagre group (ER): a graph of its own turned to LR, with 25 more
    /// rank spacing, framed by dagre's compound borders and no title band.
    #[test]
    fn dagre_group_is_its_own_turned_graph() {
        let mut inp = input(
            vec![node(Some(0)), node(Some(0)), node(None)],
            vec![LayoutEdge::new(0, 1, 1, None)],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        );
        inp.dagre_groups = true;
        let out = layout(&inp);
        assert_eq!(out.cluster_directions[0], Direction::LeftRight);
        let c = out.clusters[0];
        // Along the flow: two 80px nodes 75 apart, borders 37.5 away; across: the
        // 40px nodes, borders (50 + 20) / 2 away.
        assert!((c.w - (80.0 + 75.0 + 80.0 + 75.0)).abs() < 1e-6, "{c:?}");
        assert!((c.h - (40.0 + 70.0)).abs() < 1e-6, "{c:?}");
        assert!((out.nodes[0].y - out.nodes[1].y).abs() < 1e-6, "{out:?}");
        assert!((out.nodes[0].y - c.y - c.h / 2.0).abs() < 1e-6, "{out:?}");
    }

    /// A member joined only to ports where edges enter takes ELK's `FIRST`
    /// constraint, and `LayerConstraintPostprocessor` puts it back at the end of
    /// the first layer's list, which the crossing minimisation starts from (elkjs:
    /// `subgraph S0: s0n0 s0n1 s0n2 s0n3, s0n3 --> s0n0 end; x1 --> s0n1;
    /// s0n0 --> x1`: s0n2, s0n1, s0n3 from the left).
    #[test]
    fn elk_member_only_entered_from_outside_goes_last_in_its_layer() {
        let inp = elk(input(
            vec![
                node(Some(0)),
                node(Some(0)),
                node(Some(0)),
                node(Some(0)),
                node(None),
            ],
            vec![
                LayoutEdge::new(3, 0, 1, None),
                LayoutEdge::new(4, 1, 1, None),
                LayoutEdge::new(0, 4, 1, None),
            ],
            vec![with_dir(None, None)],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let x = |v: usize| out.nodes[v].x;
        assert!(x(2) < x(1) && x(1) < x(3), "{out:?}");
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
        // From the node's port out of C1's east side, eastwards ...
        assert!(first.x < c1.right() && pts[1].x > first.x, "{pts:?}");
        assert!(
            pts[1].x > c1.right() && first.y == pts[1].y,
            "{pts:?} {c1:?}"
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
