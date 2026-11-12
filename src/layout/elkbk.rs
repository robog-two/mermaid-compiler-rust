//! ELK's Brandes–Köpf node placement (`org.eclipse.elk.alg.layered.p4nodes.bk`),
//! the `BRANDES_KOEPF` strategy mermaid's ELK layout runs with
//! `fixedAlignment: BALANCED` and ELK's default `edgeStraightening:
//! IMPROVE_STRAIGHTNESS`.
//!
//! It differs from dagre's variant (see `bk.rs`) in ways that show in the drawing:
//! - blocks align *ports*, not centres: a node with several edges on one side
//!   shifts so that the edge it is aligned along runs straight;
//! - the spacing between two vertices of a layer depends on both their types
//!   (`Spacings.getVerticalSpacing`): node–node, edge–node and edge–edge;
//! - compaction places blocks into classes, separated by the node spacing, and the
//!   threshold strategy pulls a block towards an already placed neighbour so that
//!   one more edge runs straight; a post-pass straightens what it could not;
//! - the balanced layout aligns the four layouts on the narrowest one by its top
//!   (down layouts) or bottom (up layouts), and is dropped for the narrowest
//!   layout that keeps every layer's order if it breaks one.
//!
//! Coordinates follow ELK's internal (left-to-right) frame: layers run along x,
//! and "y" here is the cross axis, a vertex's `y` being its top.

/// ELK's node types, as far as the spacing between two vertices goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NodeType {
    Normal,
    LongEdge,
    Label,
    ExternalPort,
}

/// One vertex of the layered graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Node {
    pub kind: NodeType,
    /// Extent along the cross axis.
    pub size: f64,
    /// Margins before (`top`) and after (`bottom`) it along the cross axis.
    pub margin_top: f64,
    pub margin_bottom: f64,
}

/// An edge between two consecutive layers, left to right, with the cross-axis
/// offsets of its ports from the tops of their vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Edge {
    pub from: usize,
    pub to: usize,
    pub from_port: f64,
    pub to_port: f64,
    /// Where the ports are in ELK's port lists of their sides, top to bottom
    /// (which, for ports a laid out container fixes, its placement need not
    /// follow): ELK takes a node's neighbours and the edge between two
    /// vertices in that order.
    pub from_order: f64,
    pub to_order: f64,
}

/// The spacings between the vertices of a layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Spacing {
    pub node_node: f64,
    pub edge_node: f64,
    pub edge_edge: f64,
    pub port_port: f64,
}

impl Spacing {
    fn between(&self, a: NodeType, b: NodeType) -> f64 {
        use NodeType::*;
        match (a, b) {
            (Normal, Normal) | (Normal, Label) | (Label, Normal) => self.node_node,
            (LongEdge, LongEdge) | (Label, Label) => self.edge_edge,
            (ExternalPort, ExternalPort) => self.port_port,
            (ExternalPort, LongEdge) | (LongEdge, ExternalPort) => self.edge_edge,
            _ => self.edge_node,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VDir {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HDir {
    Right,
    Left,
}

struct Graph<'a> {
    layers: &'a [Vec<usize>],
    nodes: &'a [Node],
    edges: &'a [Edge],
    sp: Spacing,
    layer_of: Vec<usize>,
    index: Vec<usize>,
    /// Incoming / outgoing edges per vertex in ELK's port order (clockwise:
    /// outgoing top to bottom, incoming bottom to top).
    incoming: Vec<Vec<usize>>,
    outgoing: Vec<Vec<usize>>,
    /// (neighbour, edge), sorted by the neighbour's index in its layer.
    left: Vec<Vec<(usize, usize)>>,
    right: Vec<Vec<(usize, usize)>>,
}

impl<'a> Graph<'a> {
    fn new(layers: &'a [Vec<usize>], nodes: &'a [Node], edges: &'a [Edge], sp: Spacing) -> Self {
        let n = nodes.len();
        let mut layer_of = vec![usize::MAX; n];
        let mut index = vec![0; n];
        for (l, layer) in layers.iter().enumerate() {
            for (i, &v) in layer.iter().enumerate() {
                layer_of[v] = l;
                index[v] = i;
            }
        }
        let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (ei, e) in edges.iter().enumerate() {
            outgoing[e.from].push(ei);
            incoming[e.to].push(ei);
        }
        for list in &mut outgoing {
            list.sort_by(|&a, &b| edges[a].from_order.total_cmp(&edges[b].from_order));
        }
        for list in &mut incoming {
            list.sort_by(|&a, &b| edges[b].to_order.total_cmp(&edges[a].to_order));
        }
        let left: Vec<Vec<(usize, usize)>> = incoming
            .iter()
            .map(|list| {
                let mut v: Vec<(usize, usize)> = list.iter().map(|&e| (edges[e].from, e)).collect();
                v.sort_by_key(|&(u, _)| index[u]);
                v
            })
            .collect();
        let right: Vec<Vec<(usize, usize)>> = outgoing
            .iter()
            .map(|list| {
                let mut v: Vec<(usize, usize)> = list.iter().map(|&e| (edges[e].to, e)).collect();
                v.sort_by_key(|&(u, _)| index[u]);
                v
            })
            .collect();
        Graph {
            layers,
            nodes,
            edges,
            sp,
            layer_of,
            index,
            incoming,
            outgoing,
            left,
            right,
        }
    }

    fn spacing(&self, a: usize, b: usize) -> f64 {
        self.sp.between(self.nodes[a].kind, self.nodes[b].kind)
    }

    /// `BKNodePlacer.incidentToInnerSegment`: a long-edge dummy fed by another.
    fn inner(&self, v: usize) -> bool {
        self.nodes[v].kind == NodeType::LongEdge
            && self.incoming[v]
                .iter()
                .any(|&e| self.nodes[self.edges[e].from].kind == NodeType::LongEdge)
    }

    /// `BKNodePlacer.markConflicts`: type 1 conflicts, from the third layer on.
    fn mark_conflicts(&self) -> Vec<bool> {
        let mut marked = vec![false; self.edges.len()];
        let nl = self.layers.len();
        if nl < 3 {
            return marked;
        }
        for i in 1..nl - 1 {
            let layer = &self.layers[i + 1];
            let upper = self.layers[i].len();
            let (mut k0, mut l) = (0usize, 0usize);
            for (l1, &v) in layer.iter().enumerate() {
                let inner = self.inner(v);
                if l1 == layer.len() - 1 || inner {
                    let k1 = if inner {
                        self.index[self.left[v][0].0]
                    } else {
                        upper.saturating_sub(1)
                    };
                    while l <= l1 {
                        let vl = layer[l];
                        if !self.inner(vl) {
                            for &(u, e) in &self.left[vl] {
                                let k = self.index[u];
                                if k < k0 || k > k1 {
                                    marked[e] = true;
                                }
                            }
                        }
                        l += 1;
                    }
                    k0 = k1;
                }
            }
        }
        marked
    }

    /// `BKNodePlacer.getEdge`: the first edge (in port order) between two
    /// vertices of neighbouring layers.
    fn edge_between(&self, a: usize, b: usize) -> usize {
        self.outgoing[a]
            .iter()
            .find(|&&e| self.edges[e].to == b)
            .or_else(|| self.incoming[a].iter().find(|&&e| self.edges[e].from == b))
            .copied()
            .expect("aligned vertices share an edge")
    }
}

/// One of the four layouts (`BKAlignedLayout`).
struct Layout {
    vdir: VDir,
    hdir: HDir,
    root: Vec<usize>,
    align: Vec<usize>,
    inner_shift: Vec<f64>,
    block_size: Vec<f64>,
    sink: Vec<usize>,
    shift: Vec<f64>,
    y: Vec<Option<f64>>,
    su: Vec<bool>,
    od: Vec<bool>,
}

impl Layout {
    fn new(n: usize, vdir: VDir, hdir: HDir) -> Self {
        Layout {
            vdir,
            hdir,
            root: (0..n).collect(),
            align: (0..n).collect(),
            inner_shift: vec![0.0; n],
            block_size: vec![0.0; n],
            sink: (0..n).collect(),
            shift: vec![0.0; n],
            y: vec![None; n],
            su: vec![false; n],
            od: vec![true; n],
        }
    }

    fn y(&self, v: usize) -> f64 {
        self.y[v].unwrap_or(0.0)
    }

    /// Top of `v` (its block's position plus its shift inside the block).
    fn pos(&self, v: usize) -> f64 {
        self.y(v) + self.inner_shift[v]
    }

    fn size(&self, g: &Graph) -> f64 {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for layer in g.layers {
            for &v in layer {
                let y = self.y(v);
                lo = lo.min(y);
                hi = hi.max(y + self.block_size[self.root[v]]);
            }
        }
        hi - lo
    }

    fn min_y(&self, g: &Graph, v: usize) -> f64 {
        self.y(self.root[v]) + self.inner_shift[v] - g.nodes[v].margin_top
    }

    fn max_y(&self, g: &Graph, v: usize) -> f64 {
        self.y(self.root[v]) + self.inner_shift[v] + g.nodes[v].size + g.nodes[v].margin_bottom
    }

    fn block(&self, start: usize) -> Vec<usize> {
        let mut out = vec![start];
        let mut cur = self.align[start];
        while cur != start {
            out.push(cur);
            cur = self.align[cur];
        }
        out
    }

    fn shift_block(&mut self, start: usize, delta: f64) {
        for v in self.block(start) {
            self.y[v] = Some(self.y(v) + delta);
        }
    }

    fn space_above(&self, g: &Graph, start: usize, delta: f64) -> f64 {
        let mut avail = delta;
        for v in self.block(start) {
            let i = g.index[v];
            if i > 0 {
                let nb = g.layers[g.layer_of[v]][i - 1];
                avail = avail.min(self.min_y(g, v) - (self.max_y(g, nb) + g.spacing(v, nb)));
            }
        }
        avail
    }

    fn space_below(&self, g: &Graph, start: usize, delta: f64) -> f64 {
        let mut avail = delta;
        for v in self.block(start) {
            let i = g.index[v];
            let layer = &g.layers[g.layer_of[v]];
            if i + 1 < layer.len() {
                let nb = layer[i + 1];
                avail = avail.min(self.min_y(g, nb) - (self.max_y(g, v) + g.spacing(v, nb)));
            }
        }
        avail
    }

    /// The port offset of edge `e` at its `from` (left) or `to` (right) end,
    /// relative to the block position of that vertex.
    fn port_pos(&self, g: &Graph, e: usize, left: bool) -> f64 {
        let ed = &g.edges[e];
        let (v, p) = if left {
            (ed.from, ed.from_port)
        } else {
            (ed.to, ed.to_port)
        };
        self.y(self.root[v]) + self.inner_shift[v] + p
    }
}

/// `BKAligner.verticalAlignment`.
fn vertical_alignment(g: &Graph, bal: &mut Layout, marked: &[bool]) {
    let order: Vec<usize> = match bal.hdir {
        HDir::Right => (0..g.layers.len()).collect(),
        HDir::Left => (0..g.layers.len()).rev().collect(),
    };
    for l in order {
        let layer = &g.layers[l];
        let nodes: Vec<usize> = match bal.vdir {
            VDir::Down => layer.clone(),
            VDir::Up => layer.iter().rev().copied().collect(),
        };
        let mut r: i64 = match bal.vdir {
            VDir::Down => -1,
            VDir::Up => i64::MAX,
        };
        for v in nodes {
            let nbs = match bal.hdir {
                HDir::Left => &g.right[v],
                HDir::Right => &g.left[v],
            };
            if nbs.is_empty() {
                continue;
            }
            let d = nbs.len();
            let low = d.div_ceil(2) - 1;
            let high = (d + 2) / 2 - 1;
            let ms: Vec<usize> = match bal.vdir {
                VDir::Up => (low..=high).rev().collect(),
                VDir::Down => (low..=high).collect(),
            };
            for m in ms {
                if bal.align[v] != v {
                    continue;
                }
                let (u, e) = nbs[m];
                let k = g.index[u] as i64;
                let ok = match bal.vdir {
                    VDir::Up => r > k,
                    VDir::Down => r < k,
                };
                if !marked[e] && ok {
                    bal.align[u] = v;
                    bal.root[v] = bal.root[u];
                    bal.align[v] = bal.root[v];
                    let rt = bal.root[v];
                    bal.od[rt] &= g.nodes[v].kind == NodeType::LongEdge;
                    r = k;
                }
            }
        }
    }
}

/// `BKAligner.insideBlockShift`.
fn inside_block_shift(g: &Graph, bal: &mut Layout) {
    for layer in g.layers {
        for &root in layer {
            if bal.root[root] != root {
                continue;
            }
            let nd = &g.nodes[root];
            let mut above = nd.margin_top;
            let mut below = nd.size + nd.margin_bottom;
            bal.inner_shift[root] = 0.0;
            let mut cur = root;
            loop {
                let next = bal.align[cur];
                if next == root {
                    break;
                }
                let e = g.edge_between(cur, next);
                let ed = &g.edges[e];
                let diff = match bal.hdir {
                    HDir::Left => ed.to_port - ed.from_port,
                    HDir::Right => ed.from_port - ed.to_port,
                };
                let shift = bal.inner_shift[cur] + diff;
                bal.inner_shift[next] = shift;
                let nn = &g.nodes[next];
                above = above.max(nn.margin_top - shift);
                below = below.max(shift + nn.size + nn.margin_bottom);
                cur = next;
            }
            for v in bal.block(root) {
                bal.inner_shift[v] += above;
            }
            bal.block_size[root] = above + below;
        }
    }
}

/// The class graph of one compaction (`BKCompactor.ClassNode`), in creation order.
#[derive(Default)]
struct Classes {
    sinks: Vec<usize>,
    of: std::collections::HashMap<usize, usize>,
    shift: Vec<Option<f64>>,
    out: Vec<Vec<(usize, f64)>>,
    indeg: Vec<usize>,
}

impl Classes {
    fn get(&mut self, sink: usize) -> usize {
        if let Some(&c) = self.of.get(&sink) {
            return c;
        }
        let c = self.sinks.len();
        self.sinks.push(sink);
        self.of.insert(sink, c);
        self.shift.push(None);
        self.out.push(Vec::new());
        self.indeg.push(0);
        c
    }

    fn add(&mut self, a: usize, b: usize, sep: f64) {
        self.out[a].push((b, sep));
        self.indeg[b] += 1;
    }
}

/// `ThresholdStrategy.SimpleThresholdStrategy`.
struct Threshold {
    finished: Vec<bool>,
    queue: std::collections::VecDeque<(usize, bool)>,
}

impl Threshold {
    fn invalid(bal: &Layout) -> f64 {
        match bal.vdir {
            VDir::Up => f64::INFINITY,
            VDir::Down => f64::NEG_INFINITY,
        }
    }

    /// `pickEdge`: an edge of `free` towards a finished block, or `None`; and
    /// whether it has edges to consider at all.
    fn pick(&self, g: &Graph, bal: &Layout, free: usize, is_root: bool) -> (Option<usize>, bool) {
        let incoming = if is_root {
            bal.hdir == HDir::Right
        } else {
            bal.hdir == HDir::Left
        };
        let edges = if incoming {
            &g.incoming[free]
        } else {
            &g.outgoing[free]
        };
        let mut has = false;
        for &e in edges {
            if bal.su[bal.root[free]] {
                continue;
            }
            has = true;
            let ed = &g.edges[e];
            let other = if ed.from == free { ed.to } else { ed.from };
            if self.finished[bal.root[other]] {
                return (Some(e), true);
            }
        }
        (None, has)
    }

    fn bound(&mut self, g: &Graph, bal: &mut Layout, v: usize, is_root: bool) -> f64 {
        let invalid = Self::invalid(bal);
        match self.pick(g, bal, v, is_root) {
            (None, true) => {
                self.queue.push_back((v, is_root));
                invalid
            }
            (Some(e), _) => {
                let ed = g.edges[e];
                // The edge's end at `v`'s block is on its right (`to`) side when ...
                let root_is_right = if is_root {
                    bal.hdir == HDir::Right
                } else {
                    bal.hdir == HDir::Left
                };
                let (root_node, root_port, other_node, other_port) = if root_is_right {
                    (ed.to, ed.to_port, ed.from, ed.from_port)
                } else {
                    (ed.from, ed.from_port, ed.to, ed.to_port)
                };
                let t = bal.y(bal.root[other_node]) + bal.inner_shift[other_node] + other_port
                    - bal.inner_shift[root_node]
                    - root_port;
                let (a, b) = (bal.root[ed.from], bal.root[ed.to]);
                bal.su[a] = true;
                bal.su[b] = true;
                t
            }
            (None, false) => invalid,
        }
    }

    fn calculate(&mut self, g: &Graph, bal: &mut Layout, old: f64, root: usize, cur: usize) -> f64 {
        let is_root = root == cur;
        let is_last = bal.align[cur] == root;
        if !(is_root || is_last) {
            return old;
        }
        let mut t = old;
        if is_root {
            t = self.bound(g, bal, root, true);
        }
        if t.is_infinite() && is_last {
            t = self.bound(g, bal, cur, false);
        }
        t
    }

    fn post_process(&mut self, g: &Graph, bal: &mut Layout) {
        let mut stack = Vec::new();
        while let Some((free, is_root)) = self.queue.pop_front() {
            let (Some(e), _) = self.pick(g, bal, free, is_root) else {
                continue;
            };
            if !Self::process(g, bal, free, e) {
                stack.push((free, e));
            }
        }
        while let Some((free, e)) = stack.pop() {
            Self::process(g, bal, free, e);
        }
    }

    fn process(g: &Graph, bal: &mut Layout, free: usize, e: usize) -> bool {
        let ed = &g.edges[e];
        // `fix` is the end away from `free`, `block` the end at it.
        let free_is_from = ed.from == free;
        let fix = bal.port_pos(g, e, !free_is_from);
        let block = bal.port_pos(g, e, free_is_from);
        let node = free;
        let delta = block - fix;
        if delta > 0.0 && delta < f64::MAX {
            let avail = bal.space_above(g, node, delta);
            bal.shift_block(node, -avail);
            avail > 0.0
        } else if delta < 0.0 && -delta < f64::MAX {
            let avail = bal.space_below(g, node, -delta);
            bal.shift_block(node, avail);
            avail > 0.0
        } else {
            false
        }
    }
}

/// `BKCompactor.horizontalCompaction`.
fn horizontal_compaction(g: &Graph, bal: &mut Layout) {
    let n = g.nodes.len();
    let inf = match bal.vdir {
        VDir::Up => f64::NEG_INFINITY,
        VDir::Down => f64::INFINITY,
    };
    bal.sink = (0..n).collect();
    bal.shift = vec![inf; n];
    bal.y = vec![None; n];
    let order: Vec<usize> = match bal.hdir {
        HDir::Right => (0..g.layers.len()).collect(),
        HDir::Left => (0..g.layers.len()).rev().collect(),
    };
    let mut th = Threshold {
        finished: vec![false; n],
        queue: Default::default(),
    };
    let mut classes = Classes::default();
    for &l in &order {
        let layer = &g.layers[l];
        let nodes: Vec<usize> = match bal.vdir {
            VDir::Down => layer.clone(),
            VDir::Up => layer.iter().rev().copied().collect(),
        };
        for v in nodes {
            if bal.root[v] == v {
                place_block(g, bal, &mut th, &mut classes, v);
            }
        }
    }
    // `placeClasses`.
    let mut queue: std::collections::VecDeque<usize> = (0..classes.sinks.len())
        .filter(|&c| classes.indeg[c] == 0)
        .collect();
    while let Some(c) = queue.pop_front() {
        let s = *classes.shift[c].get_or_insert(0.0);
        for k in 0..classes.out[c].len() {
            let (t, sep) = classes.out[c][k];
            let cand = s + sep;
            classes.shift[t] = Some(match (classes.shift[t], bal.vdir) {
                (None, _) => cand,
                (Some(old), VDir::Down) => old.min(cand),
                (Some(old), VDir::Up) => old.max(cand),
            });
            classes.indeg[t] -= 1;
            if classes.indeg[t] == 0 {
                queue.push_back(t);
            }
        }
    }
    for (c, &sink) in classes.sinks.iter().enumerate() {
        if let Some(s) = classes.shift[c] {
            bal.shift[sink] = s;
        }
    }
    for &l in &order {
        for &v in &g.layers[l] {
            let r = bal.root[v];
            let mut y = bal.y(r);
            if v == r {
                let s = bal.shift[bal.sink[v]];
                if s.is_finite() {
                    y += s;
                }
            }
            bal.y[v] = Some(y);
        }
    }
    th.post_process(g, bal);
}

/// `BKCompactor.placeBlock`, with an explicit stack in place of recursion.
fn place_block(
    g: &Graph,
    bal: &mut Layout,
    th: &mut Threshold,
    classes: &mut Classes,
    start: usize,
) {
    if bal.y[start].is_some() {
        return;
    }
    struct Frame {
        root: usize,
        cur: usize,
        thresh: f64,
        initial: bool,
        /// The neighbour block of `cur` has been placed.
        ready: bool,
    }
    let new_frame = |bal: &mut Layout, root: usize| {
        bal.y[root] = Some(0.0);
        Frame {
            root,
            cur: root,
            thresh: Threshold::invalid(bal),
            initial: true,
            ready: false,
        }
    };
    let mut stack = vec![new_frame(bal, start)];
    while let Some(f) = stack.last_mut() {
        let (root, cur) = (f.root, f.cur);
        let i = g.index[cur];
        let layer = &g.layers[g.layer_of[cur]];
        let neighbor = match bal.vdir {
            VDir::Down => i.checked_sub(1).map(|j| layer[j]),
            VDir::Up => layer.get(i + 1).copied(),
        };
        if let Some(nb) = neighbor {
            let nroot = bal.root[nb];
            if !f.ready {
                f.ready = true;
                if bal.y[nroot].is_none() {
                    let fr = new_frame(bal, nroot);
                    stack.push(fr);
                    continue;
                }
            }
            let f = stack.last_mut().expect("frame");
            f.thresh = th.calculate(g, bal, f.thresh, root, cur);
            if bal.sink[root] == root {
                bal.sink[root] = bal.sink[nroot];
            }
            if bal.sink[root] == bal.sink[nroot] {
                let spacing = g.spacing(cur, nb);
                let (cn, nn) = (&g.nodes[cur], &g.nodes[nb]);
                let here = bal.y(root);
                match bal.vdir {
                    VDir::Up => {
                        let p = bal.y(nroot) + bal.inner_shift[nb]
                            - nn.margin_top
                            - spacing
                            - cn.margin_bottom
                            - cn.size
                            - bal.inner_shift[cur];
                        let v = if f.initial {
                            p.min(f.thresh)
                        } else {
                            here.min(p.min(f.thresh))
                        };
                        bal.y[root] = Some(v);
                    }
                    VDir::Down => {
                        let p = bal.y(nroot)
                            + bal.inner_shift[nb]
                            + nn.size
                            + nn.margin_bottom
                            + spacing
                            + cn.margin_top
                            - bal.inner_shift[cur];
                        let v = if f.initial {
                            p.max(f.thresh)
                        } else {
                            here.max(p.max(f.thresh))
                        };
                        bal.y[root] = Some(v);
                    }
                }
                f.initial = false;
            } else {
                let spacing = g.sp.node_node;
                let a = classes.get(bal.sink[root]);
                let b = classes.get(bal.sink[nroot]);
                let (cn, nn) = (&g.nodes[cur], &g.nodes[nb]);
                let req = match bal.vdir {
                    VDir::Up => {
                        bal.y(root) + bal.inner_shift[cur] + cn.size + cn.margin_bottom + spacing
                            - (bal.y(nroot) + bal.inner_shift[nb] - nn.margin_top)
                    }
                    VDir::Down => {
                        bal.y(root) + bal.inner_shift[cur]
                            - cn.margin_top
                            - bal.y(nroot)
                            - bal.inner_shift[nb]
                            - nn.size
                            - nn.margin_bottom
                            - spacing
                    }
                };
                classes.add(a, b, req);
            }
        } else {
            f.thresh = th.calculate(g, bal, f.thresh, root, cur);
        }
        let f = stack.last_mut().expect("frame");
        f.cur = bal.align[cur];
        f.ready = false;
        if f.cur == root {
            th.finished[root] = true;
            stack.pop();
        }
    }
}

/// `BKNodePlacer.checkOrderConstraint`.
fn keeps_order(g: &Graph, top: &dyn Fn(usize) -> f64) -> bool {
    for layer in g.layers {
        let mut pos = f64::NEG_INFINITY;
        for &v in layer {
            let nd = &g.nodes[v];
            let t = top(v) - nd.margin_top;
            let b = top(v) + nd.size + nd.margin_bottom;
            if t > pos && b > pos {
                pos = top(v) + nd.size + nd.margin_bottom;
            } else {
                return false;
            }
        }
    }
    true
}

/// Cross-axis tops of every vertex in `layers` (others stay 0).
pub(super) fn place(
    layers: &[Vec<usize>],
    nodes: &[Node],
    edges: &[Edge],
    sp: Spacing,
) -> Vec<f64> {
    let g = Graph::new(layers, nodes, edges, sp);
    let n = nodes.len();
    let marked = g.mark_conflicts();
    let mut layouts = vec![
        Layout::new(n, VDir::Down, HDir::Right),
        Layout::new(n, VDir::Up, HDir::Right),
        Layout::new(n, VDir::Down, HDir::Left),
        Layout::new(n, VDir::Up, HDir::Left),
    ];
    for bal in &mut layouts {
        vertical_alignment(&g, bal, &marked);
        inside_block_shift(&g, bal);
    }
    for bal in &mut layouts {
        horizontal_compaction(&g, bal);
    }
    // `createBalancedLayout`.
    let sizes: Vec<f64> = layouts.iter().map(|b| b.size(&g)).collect();
    let mut narrowest = 0;
    for (i, &s) in sizes.iter().enumerate() {
        if sizes[narrowest] > s {
            narrowest = i;
        }
    }
    let bounds: Vec<(f64, f64)> = layouts
        .iter()
        .map(|b| {
            let (mut lo, mut hi) = (f64::from(i32::MAX), f64::from(i32::MIN));
            for layer in layers {
                for &v in layer {
                    let p = b.pos(v);
                    lo = lo.min(p);
                    hi = hi.max(p + nodes[v].size);
                }
            }
            (lo, hi)
        })
        .collect();
    let shift: Vec<f64> = layouts
        .iter()
        .zip(&bounds)
        .map(|(b, &(lo, hi))| match b.vdir {
            VDir::Down => bounds[narrowest].0 - lo,
            VDir::Up => bounds[narrowest].1 - hi,
        })
        .collect();
    let mut balanced = vec![0.0; n];
    for layer in layers {
        for &v in layer {
            let mut c: Vec<f64> = layouts
                .iter()
                .zip(&shift)
                .map(|(b, s)| b.pos(v) + s)
                .collect();
            c.sort_by(f64::total_cmp);
            balanced[v] = (c[1] + c[2]) / 2.0;
        }
    }
    if keeps_order(&g, &|v| balanced[v]) {
        return balanced;
    }
    let mut chosen: Option<usize> = None;
    for (i, b) in layouts.iter().enumerate() {
        if keeps_order(&g, &|v| b.pos(v)) && chosen.is_none_or(|c| sizes[c] > sizes[i]) {
            chosen = Some(i);
        }
    }
    let b = &layouts[chosen.unwrap_or(0)];
    let mut out = vec![0.0; n];
    for layer in layers {
        for &v in layer {
            out[v] = b.pos(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SP: Spacing = Spacing {
        node_node: 40.0,
        edge_node: 20.0,
        edge_edge: 20.0,
        port_port: 20.0,
    };

    fn node(size: f64) -> Node {
        Node {
            kind: NodeType::Normal,
            size,
            margin_top: 0.0,
            margin_bottom: 0.0,
        }
    }

    fn edge(from: usize, to: usize, from_port: f64, to_port: f64) -> Edge {
        Edge {
            from,
            to,
            from_port,
            to_port,
            from_order: from_port,
            to_order: to_port,
        }
    }

    /// Nodes of a layer keep their order and the node spacing.
    #[test]
    fn layer_keeps_order_and_spacing() {
        let nodes = vec![node(45.0), node(45.0), node(45.0)];
        let y = place(&[vec![0, 1, 2]], &nodes, &[], SP);
        assert_eq!(y[1] - y[0], 85.0);
        assert_eq!(y[2] - y[1], 85.0);
    }

    /// A chain runs straight from port to port: the second node shifts so that its
    /// port lines up with the first one's, not its centre.
    #[test]
    fn chain_aligns_ports() {
        let nodes = vec![node(45.0), node(80.0)];
        let y = place(&[vec![0], vec![1]], &nodes, &[edge(0, 1, 22.5, 40.0)], SP);
        assert!((y[0] + 22.5 - (y[1] + 40.0)).abs() < 1e-9, "{y:?}");
    }

    /// A long-edge dummy keeps the edge–node spacing from a node, not the node spacing.
    #[test]
    fn dummy_keeps_edge_node_spacing() {
        let dummy = Node {
            kind: NodeType::LongEdge,
            size: 0.0,
            margin_top: 0.0,
            margin_bottom: 0.0,
        };
        let nodes = vec![node(45.0), dummy];
        let y = place(&[vec![0, 1]], &nodes, &[], SP);
        assert_eq!(y[1] - (y[0] + 45.0), 20.0);
    }

    /// A fork: the parent is balanced between its two children, its ports spread.
    #[test]
    fn fork_is_balanced() {
        let nodes = vec![node(45.0), node(45.0), node(45.0)];
        let y = place(
            &[vec![0], vec![1, 2]],
            &nodes,
            &[edge(0, 1, 16.0, 22.5), edge(0, 2, 29.0, 22.5)],
            SP,
        );
        let mid = (y[1] + y[2]) / 2.0;
        assert!((y[0] - mid).abs() < 1e-9, "{y:?}");
        assert!(y[2] - y[1] >= 85.0);
    }
}
