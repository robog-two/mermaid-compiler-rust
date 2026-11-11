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

use super::{
    Direction, EdgeRoute, EdgeRouting, LayoutCluster, LayoutEdge, LayoutInput, LayoutNode,
    LayoutOutput, OrthoSpacing, cluster_chains, layout_flat,
};
use crate::geom::{Point, Rect};

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
    bbox: Rect,
}

pub(super) fn layout(input: &LayoutInput) -> LayoutOutput {
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
        separate: populated,
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
    if !tree.separate.iter().any(|&s| s) {
        return layout_flat(input);
    }

    // Deepest separate clusters first, the root last.
    let mut scopes: Vec<Option<usize>> = (0..nc).filter(|&c| tree.separate[c]).map(Some).collect();
    scopes.sort_by_key(|s| std::cmp::Reverse(s.map_or(0, |c| tree.chains[c].len())));
    scopes.push(None);

    let pad = input.cluster_padding;
    let mut box_size: Vec<(f64, f64)> = vec![(0.0, 0.0); nc];
    let mut levels: Vec<Option<Level>> = (0..=nc).map(|_| None).collect();
    let slot = |s: Option<usize>| s.unwrap_or(nc);
    for &scope in &scopes {
        let level = layout_level(&tree, &ends, scope, &box_size);
        if let Some(c) = scope {
            let cl = &input.clusters[c];
            box_size[c] = (
                (level.bbox.w + 2.0 * pad).max(cl.min_width),
                level.bbox.h + 2.0 * pad + cl.title_height,
            );
        }
        levels[slot(scope)] = Some(level);
    }

    // Compose: place every level at its box, top-down.
    let n = input.nodes.len();
    let mut nodes = vec![Point::new(0.0, 0.0); n];
    let mut routes: Vec<Option<EdgeRoute>> = vec![None; input.edges.len()];
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
            routes[ei] = Some(EdgeRoute {
                points: r.points.iter().map(|p| p.plus(off)).collect(),
                label: r.label.map(|p| p.plus(off)),
            });
        }
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
    LayoutOutput {
        nodes,
        edges,
        clusters,
    }
}

fn layout_level(
    tree: &Tree,
    ends: &[(Member, Member)],
    scope: Option<usize>,
    box_size: &[(f64, f64)],
) -> Level {
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
    });
    for (i, it) in items.iter().enumerate() {
        match *it {
            Item::Node(v) => node_item[v] = Some(i),
            Item::Box(c) => box_item[c] = Some(i),
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
        Some(_) if tree.direction(scope).is_horizontal() => {
            l.map(|(w, h)| (w + input.inner_rank_sep, h))
        }
        Some(_) => l.map(|(w, h)| (w, h + input.inner_rank_sep)),
    };
    let mut edges = Vec::new();
    let mut edge_ids = Vec::new();
    for (ei, (e, &(a, b))) in input.edges.iter().zip(ends).enumerate() {
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
        });
        edge_ids.push(ei);
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
    let out = layout_flat(&flat);
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
    for (nd, p) in flat.nodes.iter().zip(&out.nodes) {
        add(Rect::centered(*p, nd.width, nd.height));
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
    Level {
        items,
        out,
        flat_clusters,
        edge_ids,
        bbox: bbox.unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

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
}
