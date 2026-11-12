//! ELK's layering: `NetworkSimplexLayerer` and `NetworkSimplex`
//! (`org.eclipse.elk.alg.layered.p2layers` / `org.eclipse.elk.alg.common.networksimplex`),
//! ported step for step so that where several layerings are equally short, the one
//! ELK picks comes out: the same connected components in the same order, the same
//! initial tight tree, the same leaving and entering edges, the same normalisation
//! and the same balancing (which counts the nodes of components layered before).
//!
//! The graph is ELK's `LGraph` at that point: its nodes in ELK's order (real nodes,
//! then label dummies), every edge end a port of its own. A port's place in its
//! node's port list is given by a key; the outgoing edges of a node, and the edges
//! a depth-first search follows from it, come in port order.

/// One edge of the layered graph (after cycle breaking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NsEdge {
    pub from: usize,
    pub to: usize,
    /// Its minimum length (ELK: always 1).
    pub delta: i64,
    /// Keys of its ports in the port lists of `from` and `to`.
    pub ports: (usize, usize),
}

/// ELK's `thoroughness` (7) times `NetworkSimplexLayerer.ITER_LIMIT_FACTOR` (4).
const ITER_FACTOR: usize = 28;
/// `NetworkSimplex.REMOVE_SUBTREES_THRESH`.
const REMOVE_SUBTREES_THRESH: usize = 40;
/// `NetworkSimplex.FUZZY_ST_ZERO`.
const FUZZY_ST_ZERO: f64 = -1e-10;

/// ELK's layering of a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Layering {
    /// Per node: its layer. Each connected component starts on layer 0.
    pub layers: Vec<usize>,
    /// Per node: its layer before balancing.
    pub unbalanced: Vec<usize>,
    /// Per node: when it was put in its layer (layers list their nodes in that
    /// order: component by component, each in `NetworkSimplex`'s node order).
    pub created: Vec<usize>,
}

/// The layers of `n` nodes.
pub(super) fn layer(n: usize, edges: &[NsEdge]) -> Layering {
    // Per node: its ports in order, each (key, edge).
    let mut ports: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        ports[e.from].push((e.ports.0, i));
        ports[e.to].push((e.ports.1, i));
    }
    for p in &mut ports {
        p.sort_by_key(|&(k, _)| k);
    }
    // `connectedComponents`: depth first, the biggest component so far first.
    let mut visited = vec![false; n];
    let mut components: std::collections::VecDeque<Vec<usize>> = Default::default();
    for v in 0..n {
        if visited[v] {
            continue;
        }
        let mut comp = Vec::new();
        dfs(v, &ports, edges, &mut visited, &mut comp);
        if components.front().is_none_or(|f| f.len() < comp.len()) {
            components.push_front(comp);
        } else {
            components.push_back(comp);
        }
    }
    let mut balanced = vec![0usize; n];
    let mut plain = vec![0usize; n];
    let mut created = vec![0usize; n];
    let mut next = 0;
    let mut counts: Vec<usize> = Vec::new();
    let several = components.len() > 1;
    let mut previous: Option<Vec<usize>> = None;
    for comp in &components {
        let limit = ITER_FACTOR * (comp.len() as f64).sqrt() as usize;
        let mut g = NGraph::new(comp, &ports, edges, n);
        let unbalanced = g.execute(limit, previous.as_deref());
        for &k in &g.order {
            let v = comp[k];
            let l = g.nodes[k].layer as usize;
            balanced[v] = l;
            plain[v] = unbalanced[k];
            created[v] = next;
            next += 1;
            if counts.len() <= l {
                counts.resize(l + 1, 0);
            }
            counts[l] += 1;
        }
        if several {
            previous = Some(counts.clone());
        }
    }
    Layering {
        layers: balanced,
        unbalanced: plain,
        created,
    }
}

fn dfs(
    v: usize,
    ports: &[Vec<(usize, usize)>],
    edges: &[NsEdge],
    visited: &mut [bool],
    comp: &mut Vec<usize>,
) {
    visited[v] = true;
    comp.push(v);
    for &(_, i) in &ports[v] {
        let e = edges[i];
        // The port's edge: its other end (a self-loop's is the node itself).
        let other = if e.from == v { e.to } else { e.from };
        if !visited[other] {
            dfs(other, ports, edges, visited, comp);
        }
    }
}

struct NNode {
    layer: i64,
    tree_node: bool,
    internal_id: usize,
    incoming: Vec<usize>,
    outgoing: Vec<usize>,
}

impl NNode {
    /// `getConnectedEdges`: incoming, then outgoing.
    fn connected(&self) -> Vec<usize> {
        self.incoming
            .iter()
            .chain(&self.outgoing)
            .copied()
            .collect()
    }
}

struct NEdge {
    source: usize,
    target: usize,
    delta: i64,
    weight: f64,
    tree_edge: bool,
    internal_id: usize,
}

impl NEdge {
    fn other(&self, v: usize) -> usize {
        if self.source == v {
            self.target
        } else {
            self.source
        }
    }
}

/// `NGraph` with `NetworkSimplex`'s state.
struct NGraph {
    nodes: Vec<NNode>,
    all_edges: Vec<NEdge>,
    /// `graph.nodes`: the nodes taking part (subtrees removed), in order.
    order: Vec<usize>,
    /// `edges`: the edges taking part, by internal id.
    edges: Vec<usize>,
    /// `treeEdges`, a linked hash set: insertion order.
    tree_edges: Vec<usize>,
    edge_visited: Vec<bool>,
    /// Per node: its post-order number in the tree, and the lowest in its subtree
    /// (`poID`, `lowestPoID`).
    po_id: Vec<usize>,
    lowest_po_id: Vec<usize>,
    /// Per edge: its cut value (tree edges).
    cutvalue: Vec<f64>,
    /// Per node: its tree edges.
    tree_adj: Vec<Vec<usize>>,
    subtrees: Vec<(usize, usize)>,
    /// Per node: its connected edges while the simplex runs (they only change
    /// when subtrees are removed and put back).
    conn: Vec<Vec<usize>>,
}

impl NGraph {
    /// `NetworkSimplexLayerer.initialize`: the component's nodes in order, and per
    /// node its outgoing edges in port order (self-loops left out).
    fn new(comp: &[usize], ports: &[Vec<(usize, usize)>], edges: &[NsEdge], n: usize) -> Self {
        let mut local = vec![usize::MAX; n];
        for (k, &v) in comp.iter().enumerate() {
            local[v] = k;
        }
        let mut nodes: Vec<NNode> = comp
            .iter()
            .map(|_| NNode {
                layer: 0,
                tree_node: false,
                internal_id: 0,
                incoming: Vec::new(),
                outgoing: Vec::new(),
            })
            .collect();
        let mut all_edges = Vec::new();
        for &v in comp {
            for &(_, i) in &ports[v] {
                let e = edges[i];
                if e.from != v || e.from == e.to {
                    continue;
                }
                // A self-loop's two ports would both list it; it is left out anyway.
                let (s, t) = (local[e.from], local[e.to]);
                let id = all_edges.len();
                all_edges.push(NEdge {
                    source: s,
                    target: t,
                    delta: e.delta,
                    weight: 1.0,
                    tree_edge: false,
                    internal_id: 0,
                });
                nodes[s].outgoing.push(id);
                nodes[t].incoming.push(id);
            }
        }
        NGraph {
            order: (0..nodes.len()).collect(),
            nodes,
            all_edges,
            edges: Vec::new(),
            tree_edges: Vec::new(),
            edge_visited: Vec::new(),
            po_id: Vec::new(),
            lowest_po_id: Vec::new(),
            cutvalue: Vec::new(),
            tree_adj: Vec::new(),
            subtrees: Vec::new(),
            conn: Vec::new(),
        }
    }

    /// `NetworkSimplex.execute` with balancing; returns the layers (by local
    /// node) before balancing.
    fn execute(&mut self, limit: usize, previous: Option<&[usize]>) -> Vec<usize> {
        for v in &mut self.nodes {
            v.layer = 0;
        }
        let remove = self.order.len() >= REMOVE_SUBTREES_THRESH;
        if remove {
            self.remove_subtrees();
        }
        self.initialize();
        self.feasible_tree();
        let mut e = self.leave_edge();
        let mut iter = 0;
        while let Some(leave) = e {
            if iter >= limit {
                break;
            }
            let enter = self.enter_edge(leave);
            self.exchange(leave, enter);
            e = self.leave_edge();
            iter += 1;
        }
        if remove {
            self.reattach_subtrees();
        }
        let mut filling = self.normalize(previous);
        let plain = self.nodes.iter().map(|v| v.layer as usize).collect();
        self.balance(&mut filling);
        plain
    }

    fn remove_subtrees(&mut self) {
        let mut leafs: std::collections::VecDeque<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&v| self.nodes[v].incoming.len() + self.nodes[v].outgoing.len() == 1)
            .collect();
        while let Some(v) = leafs.pop_front() {
            let Some(&e) = self.nodes[v].connected().first() else {
                continue;
            };
            let is_out = !self.nodes[v].outgoing.is_empty();
            let other = self.all_edges[e].other(v);
            let list = if is_out {
                &mut self.nodes[other].incoming
            } else {
                &mut self.nodes[other].outgoing
            };
            if let Some(p) = list.iter().position(|&x| x == e) {
                list.remove(p);
            }
            if self.nodes[other].incoming.len() + self.nodes[other].outgoing.len() == 1 {
                leafs.push_back(other);
            }
            self.subtrees.push((v, e));
            if let Some(p) = self.order.iter().position(|&x| x == v) {
                self.order.remove(p);
            }
        }
    }

    fn reattach_subtrees(&mut self) {
        while let Some((v, e)) = self.subtrees.pop() {
            let placed = self.all_edges[e].other(v);
            let delta = self.all_edges[e].delta;
            if self.all_edges[e].target == v {
                self.nodes[placed].outgoing.push(e);
                self.nodes[v].layer = self.nodes[placed].layer + delta;
            } else {
                self.nodes[placed].incoming.push(e);
                self.nodes[v].layer = self.nodes[placed].layer - delta;
            }
            self.order.push(v);
        }
    }

    fn initialize(&mut self) {
        let mut edges = Vec::new();
        for (index, &v) in self.order.iter().enumerate() {
            self.nodes[v].tree_node = false;
            self.nodes[v].internal_id = index;
            edges.extend(self.nodes[v].outgoing.iter().copied());
        }
        for (k, &e) in edges.iter().enumerate() {
            self.all_edges[e].internal_id = k;
            self.all_edges[e].tree_edge = false;
        }
        let m = edges.len();
        self.cutvalue = vec![0.0; self.all_edges.len()];
        self.edge_visited = vec![false; m];
        self.po_id = vec![0; self.nodes.len()];
        self.lowest_po_id = vec![0; self.nodes.len()];
        self.edges = edges;
        self.conn = self.nodes.iter().map(NNode::connected).collect();
        self.tree_edges.clear();
    }

    fn sources(&self) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&v| self.nodes[v].incoming.is_empty())
            .collect()
    }

    fn feasible_tree(&mut self) {
        let sources = self.sources();
        self.layering_topological_numbering(sources);
        if self.edges.is_empty() {
            return;
        }
        let first = self.order[0];
        self.edge_visited.fill(false);
        while self.tight_tree_dfs(first) < self.order.len() {
            let e = self.minimal_slack();
            let ed = &self.all_edges[e];
            let mut slack = self.nodes[ed.target].layer - self.nodes[ed.source].layer - ed.delta;
            if self.nodes[ed.target].tree_node {
                slack = -slack;
            }
            for &v in &self.order {
                if self.nodes[v].tree_node {
                    self.nodes[v].layer += slack;
                }
            }
            self.edge_visited.fill(false);
        }
        self.tree_adj = vec![Vec::new(); self.nodes.len()];
        for &e in &self.tree_edges {
            let (s, t) = (self.all_edges[e].source, self.all_edges[e].target);
            self.tree_adj[s].push(e);
            self.tree_adj[t].push(e);
        }
        self.tree_values();
    }

    fn layering_topological_numbering(&mut self, roots: Vec<usize>) {
        let mut incident = vec![0usize; self.order.len()];
        for &v in &self.order {
            incident[self.nodes[v].internal_id] += self.nodes[v].incoming.len();
        }
        let mut roots: std::collections::VecDeque<usize> = roots.into();
        while let Some(v) = roots.pop_front() {
            for k in 0..self.nodes[v].outgoing.len() {
                let e = self.nodes[v].outgoing[k];
                let (t, d) = (self.all_edges[e].target, self.all_edges[e].delta);
                self.nodes[t].layer = self.nodes[t].layer.max(self.nodes[v].layer + d);
                let id = self.nodes[t].internal_id;
                incident[id] -= 1;
                if incident[id] == 0 {
                    roots.push_back(t);
                }
            }
        }
    }

    fn tight_tree_dfs(&mut self, v: usize) -> usize {
        let mut count = 1;
        self.nodes[v].tree_node = true;
        for k in 0..self.conn[v].len() {
            let e = self.conn[v][k];
            let id = self.all_edges[e].internal_id;
            if self.edge_visited[id] {
                continue;
            }
            self.edge_visited[id] = true;
            let opposite = self.all_edges[e].other(v);
            let ed = &self.all_edges[e];
            if ed.tree_edge {
                count += self.tight_tree_dfs(opposite);
            } else if !self.nodes[opposite].tree_node
                && ed.delta == self.nodes[ed.target].layer - self.nodes[ed.source].layer
            {
                self.all_edges[e].tree_edge = true;
                self.tree_edges.push(e);
                count += self.tight_tree_dfs(opposite);
            }
        }
        count
    }

    fn minimal_slack(&self) -> usize {
        let mut min = i64::MAX;
        let mut best = self.edges[0];
        for &e in &self.edges {
            let ed = &self.all_edges[e];
            if self.nodes[ed.source].tree_node ^ self.nodes[ed.target].tree_node {
                let slack = self.nodes[ed.target].layer - self.nodes[ed.source].layer - ed.delta;
                if slack < min {
                    min = slack;
                    best = e;
                }
            }
        }
        best
    }

    /// `postorderTraversal` and `cutvalues`: the post-order numbers of the tree
    /// (rooted at the first node) and the cut values of its edges. Which way the
    /// tree is walked (ELK: in the order of each node's edges) changes the numbers
    /// but neither which side of a tree edge a node is on (`isInHead`) nor the cut
    /// values, so the tree is walked in any order and the cut values are summed
    /// bottom-up (each from its lower end's edges and its subtree's tree edges).
    fn tree_values(&mut self) {
        let root = self.order[0];
        // Iterative post-order walk: (node, edge to its parent, next tree edge).
        let mut post = Vec::with_capacity(self.order.len());
        let mut parent_edge = vec![usize::MAX; self.nodes.len()];
        let mut stack: Vec<(usize, usize, usize)> = vec![(root, usize::MAX, 0)];
        let mut next_po = 1;
        while let Some(top) = stack.last_mut() {
            let (v, pe, k) = *top;
            if k < self.tree_adj[v].len() {
                top.2 += 1;
                let e = self.tree_adj[v][k];
                if e != pe {
                    let w = self.all_edges[e].other(v);
                    parent_edge[w] = e;
                    stack.push((w, e, 0));
                }
                continue;
            }
            stack.pop();
            let lowest = self.tree_adj[v]
                .iter()
                .filter(|&&e| e != pe)
                .map(|&e| self.lowest_po_id[self.all_edges[e].other(v)])
                .min()
                .unwrap_or(usize::MAX);
            self.po_id[v] = next_po;
            self.lowest_po_id[v] = lowest.min(next_po);
            next_po += 1;
            post.push(v);
        }
        for &v in &post {
            let e = parent_edge[v];
            if e == usize::MAX {
                continue;
            }
            let child_is_tail = self.all_edges[e].source == v;
            let mut cut = self.all_edges[e].weight;
            for &f in &self.conn[v] {
                if f == e {
                    continue;
                }
                let fe = &self.all_edges[f];
                let points_to_head = (fe.source == v) == child_is_tail;
                cut += if points_to_head {
                    fe.weight
                } else {
                    -fe.weight
                };
                if fe.tree_edge {
                    let c = self.cutvalue[f];
                    cut += if points_to_head { -c } else { c };
                }
            }
            self.cutvalue[e] = cut;
        }
    }

    fn is_in_head(&self, v: usize, e: usize) -> bool {
        let (s, t) = (self.all_edges[e].source, self.all_edges[e].target);
        let p = self.po_id[v];
        let (lo, po) = (&self.lowest_po_id, &self.po_id);
        if lo[s] <= p && p <= po[s] && lo[t] <= p && p <= po[t] {
            return po[s] >= po[t];
        }
        po[s] < po[t]
    }

    fn leave_edge(&self) -> Option<usize> {
        self.tree_edges
            .iter()
            .copied()
            .find(|&e| self.all_edges[e].tree_edge && self.cutvalue[e] < FUZZY_ST_ZERO)
    }

    fn enter_edge(&self, leave: usize) -> usize {
        let mut best = None;
        let mut rep = i64::MAX;
        for &e in &self.edges {
            let ed = &self.all_edges[e];
            if self.is_in_head(ed.source, leave) && !self.is_in_head(ed.target, leave) {
                let slack = self.nodes[ed.target].layer - self.nodes[ed.source].layer - ed.delta;
                if slack < rep {
                    rep = slack;
                    best = Some(e);
                }
            }
        }
        best.expect("network simplex: no entering edge")
    }

    fn exchange(&mut self, leave: usize, enter: usize) {
        self.all_edges[leave].tree_edge = false;
        self.tree_edges.retain(|&e| e != leave);
        self.all_edges[enter].tree_edge = true;
        self.tree_edges.push(enter);
        for (e, add) in [(leave, false), (enter, true)] {
            for end in [self.all_edges[e].source, self.all_edges[e].target] {
                let adj = &mut self.tree_adj[end];
                if add {
                    adj.push(e);
                } else if let Some(p) = adj.iter().position(|&x| x == e) {
                    adj.remove(p);
                }
            }
        }
        let ed = &self.all_edges[enter];
        let mut delta = self.nodes[ed.target].layer - self.nodes[ed.source].layer - ed.delta;
        if !self.is_in_head(ed.target, leave) {
            delta = -delta;
        }
        for k in 0..self.order.len() {
            let v = self.order[k];
            if !self.is_in_head(v, leave) {
                self.nodes[v].layer += delta;
            }
        }
        self.tree_values();
    }

    /// Shifts the layers to start at 0; returns the nodes per layer, those of
    /// earlier components included.
    fn normalize(&mut self, previous: Option<&[usize]>) -> Vec<usize> {
        let lowest = self
            .order
            .iter()
            .map(|&v| self.nodes[v].layer)
            .min()
            .unwrap_or(0);
        let highest = self
            .order
            .iter()
            .map(|&v| self.nodes[v].layer)
            .max()
            .unwrap_or(0);
        let mut filling = vec![0usize; (highest - lowest + 1) as usize];
        for &v in &self.order {
            self.nodes[v].layer -= lowest;
            filling[self.nodes[v].layer as usize] += 1;
        }
        if let Some(prev) = previous {
            for (f, &p) in filling.iter_mut().zip(prev) {
                *f += p;
            }
        }
        filling
    }

    fn minimal_span(&self, v: usize) -> (i64, i64) {
        let (mut span_in, mut span_out) = (i64::MAX, i64::MAX);
        for e in self.nodes[v].connected() {
            let ed = &self.all_edges[e];
            let span = self.nodes[ed.target].layer - self.nodes[ed.source].layer;
            if ed.target == v && span < span_in {
                span_in = span;
            } else if span < span_out {
                span_out = span;
            }
        }
        let fix = |s: i64| if s == i64::MAX { -1 } else { s };
        (fix(span_in), fix(span_out))
    }

    fn balance(&mut self, filling: &mut [usize]) {
        for k in 0..self.order.len() {
            let v = self.order[k];
            if self.nodes[v].incoming.len() != self.nodes[v].outgoing.len() {
                continue;
            }
            let layer = self.nodes[v].layer;
            let mut new_layer = layer;
            let (span_in, span_out) = self.minimal_span(v);
            let mut i = layer - span_in + 1;
            while i < layer + span_out {
                if filling[i as usize] < filling[new_layer as usize] {
                    new_layer = i;
                }
                i += 1;
            }
            if filling[new_layer as usize] < filling[layer as usize] {
                filling[layer as usize] -= 1;
                filling[new_layer as usize] += 1;
                self.nodes[v].layer = new_layer;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(pairs: &[(usize, usize)]) -> Vec<NsEdge> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| NsEdge {
                from,
                to,
                delta: 1,
                ports: (i, i),
            })
            .collect()
    }

    #[test]
    fn chain_takes_consecutive_layers() {
        let l = layer(3, &edges(&[(0, 1), (1, 2)])).layers;
        assert_eq!(l, vec![0, 1, 2]);
    }

    #[test]
    fn edges_are_as_short_as_possible() {
        // 3 hangs off 2 alone: longest path would put it on layer 0.
        let l = layer(4, &edges(&[(0, 1), (1, 2), (3, 2)])).unbalanced;
        assert_eq!(l, vec![0, 1, 2, 1]);
    }

    #[test]
    fn components_each_start_on_layer_zero() {
        let l = layer(4, &edges(&[(0, 1), (2, 3)])).layers;
        assert_eq!(l, vec![0, 1, 0, 1]);
    }

    #[test]
    fn balancing_counts_earlier_components() {
        // The first component (0..5) puts three nodes on layer 2. In the second,
        // node 9 (5 -> 9 -> 8) may take layer 1 or 2; on its own the component has
        // fewer nodes on layer 2, but with the first one's counted, layer 1 is the
        // emptier, so 9 stays there.
        let e = edges(&[
            (0, 1),
            (1, 2),
            (1, 3),
            (1, 4),
            (5, 6),
            (6, 7),
            (7, 8),
            (5, 9),
            (9, 8),
        ]);
        let l = layer(10, &e).layers;
        assert_eq!(l, vec![0, 1, 2, 2, 2, 0, 1, 2, 3, 1]);
        // Alone, it moves to layer 2.
        let alone: Vec<NsEdge> = e[4..]
            .iter()
            .map(|x| NsEdge {
                from: x.from - 5,
                to: x.to - 5,
                ..*x
            })
            .collect();
        assert_eq!(layer(5, &alone).layers, vec![0, 1, 2, 3, 2]);
    }

    #[test]
    fn layers_list_nodes_depth_first_over_ports() {
        // 0 -> 2, 1 -> 2, 0 -> 3: from 0, its first port leads to 2, whose next
        // port leads to 1; 3 comes last.
        let e = edges(&[(0, 2), (1, 2), (0, 3)]);
        let l = layer(4, &e);
        assert_eq!(l.created, vec![0, 2, 1, 3]);
        // The biggest component is layered first.
        let e = edges(&[(0, 1), (2, 3), (3, 4)]);
        assert_eq!(layer(5, &e).created, vec![3, 4, 0, 1, 2]);
    }

    #[test]
    fn big_graph_with_subtrees_is_layered_feasibly() {
        let mut pairs = Vec::new();
        for v in 1..60 {
            pairs.push(((v - 1) / 2, v));
        }
        pairs.push((3, 50));
        let e = edges(&pairs);
        let Layering {
            layers: l,
            unbalanced: p,
            ..
        } = layer(60, &e);
        for x in &e {
            assert!(l[x.to] > l[x.from] && p[x.to] > p[x.from]);
        }
    }
}
