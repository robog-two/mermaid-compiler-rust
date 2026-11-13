//! fCoSE (cytoscape-fcose 2.2.0 on cose-base 2.2.0 / layout-base 2.0.1) as
//! mermaid 12.3 runs it for `architecture-beta`: `quality: 'proof'`,
//! `randomize: false` (no spectral draft, an incremental CoSE run from the
//! current positions), alignment and relative-placement constraints, compound
//! nodes, per-edge ideal lengths and elasticities. mermaid wraps the run in a
//! mulberry32-seeded `Math.random` (seed 1), so the run is deterministic and
//! reproduced here step by step.
//!
//! mermaid always hands fcose an `alignmentConstraint` object, even when both
//! of its lists are empty, so fcose treats every architecture as constrained:
//! it turns off tiling and component packing and skips tree reduction. Those
//! parts and the spectral layout (only for `randomize: true`) are therefore
//! not ported, nor are fixed-node constraints, which mermaid never sets.

use super::geometry::{RectD, clip_points, separation_amount, sign};
use super::jsmath;

/// One cytoscape node. A node that some other node names as `parent` is a
/// compound; its own size is derived from its children.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub parent: Option<usize>,
    /// `layoutDimensions` of a leaf.
    pub width: f64,
    pub height: f64,
    /// `parseInt(css('padding'))`.
    pub padding: f64,
}

/// One cytoscape edge with its `idealEdgeLength` / `edgeElasticity`.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub source: usize,
    pub target: usize,
    pub ideal_length: f64,
    pub elasticity: f64,
}

/// `relativePlacementConstraint` entries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Relative {
    /// `{left, right, gap}`.
    LeftRight(usize, usize, f64),
    /// `{top, bottom, gap}`.
    TopBottom(usize, usize, f64),
}

/// fcose's constraint options (node indices).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Constraints {
    /// `alignmentConstraint.vertical`: groups sharing an x.
    pub vertical: Vec<Vec<usize>>,
    /// `alignmentConstraint.horizontal`: groups sharing a y.
    pub horizontal: Vec<Vec<usize>>,
    pub relative: Vec<Relative>,
}

/// Run fcose once. `positions` are the cytoscape positions of the leaves (the
/// entries of compounds are ignored) and `center` the centre of the
/// collection's bounding box before the run, which the result is moved onto
/// (`relocateComponent`). Returns every node's box, compounds included.
pub fn run(
    nodes: &[Node],
    edges: &[Edge],
    constraints: &Constraints,
    positions: &[(f64, f64)],
    center: (f64, f64),
) -> Vec<RectD> {
    let mut l = Fcose::build(nodes, edges, positions);
    l.constraints = constraints.clone();
    l.run();
    // aux.relocateComponent over every laid-out node.
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for &v in &l.cy_to_l {
        let r = l.nodes[v].rect;
        min_x = min_x.min(r.x);
        max_x = max_x.max(r.x + r.w);
        min_y = min_y.min(r.y);
        max_y = max_y.max(r.y + r.h);
    }
    let dx = center.0 - (max_x + min_x) / 2.0;
    let dy = center.1 - (max_y + min_y) / 2.0;
    l.cy_to_l
        .iter()
        .map(|&v| {
            let r = l.nodes[v].rect;
            let (cx, cy) = (r.center_x() + dx, r.center_y() + dy);
            RectD {
                x: cx - r.w / 2.0,
                y: cy - r.h / 2.0,
                w: r.w,
                h: r.h,
            }
        })
        .collect()
}

/// `Math.random` as mermaid's `withSeededRandom` replaces it (mulberry32).
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

const ROOT_MARGIN: f64 = 15.0;
const SIMPLE_NODE_SIZE: f64 = 40.0;
const EMPTY_COMPOUND_NODE_SIZE: f64 = 40.0;
const NODE_REPULSION: f64 = 4500.0;
const GRAVITY: f64 = 0.25;
const COMPOUND_GRAVITY: f64 = 1.0;
const GRAVITY_RANGE: f64 = 3.8;
const COMPOUND_GRAVITY_RANGE: f64 = 1.5;
const NESTING_FACTOR: f64 = 0.1;
const INITIAL_ENERGY: f64 = 0.3;
const MAX_ITERATIONS: usize = 2500;
const CONVERGENCE_CHECK_PERIOD: usize = 100;
const GRID_CALCULATION_CHECK_PERIOD: usize = 10;
const MAX_NODE_DISPLACEMENT_INCREMENTAL: f64 = 100.0;
const FINAL_TEMPERATURE: f64 = 0.04;
const INT_MAX: f64 = 2147483647.0;

struct LNode {
    rect: RectD,
    edges: Vec<usize>,
    owner: usize,
    child: Option<usize>,
    no_of_children: f64,
    estimated_size: f64,
    depth: f64,
    /// `paddingLeft`; `None` for the root graph's parent.
    padding: Option<f64>,
    spring: (f64, f64),
    repulsion: (f64, f64),
    gravitation: (f64, f64),
    disp: (f64, f64),
    grid: (i64, i64, i64, i64),
    surrounding: Vec<usize>,
    /// Cytoscape node index (`usize::MAX` for the root's dummy parent).
    cy: usize,
}

struct LGraph {
    nodes: Vec<usize>,
    edges: Vec<usize>,
    parent: usize,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    estimated_size: f64,
    connected: bool,
}

struct LEdge {
    source: usize,
    target: usize,
    ideal: f64,
    elasticity: f64,
    inter: bool,
    lca: usize,
    source_in_lca: usize,
    target_in_lca: usize,
}

/// A key of the constraint maps: a leaf or an alignment dummy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Node(usize),
    Dummy(usize),
}

/// A JavaScript `Map` with insertion-ordered keys.
#[derive(Clone)]
struct OrderedMap<V> {
    keys: Vec<Key>,
    vals: std::collections::HashMap<Key, V>,
}

impl<V: Clone> OrderedMap<V> {
    fn new() -> Self {
        OrderedMap {
            keys: Vec::new(),
            vals: std::collections::HashMap::new(),
        }
    }
    fn has(&self, k: Key) -> bool {
        self.vals.contains_key(&k)
    }
    fn get(&self, k: Key) -> Option<&V> {
        self.vals.get(&k)
    }
    fn get_mut(&mut self, k: Key) -> Option<&mut V> {
        self.vals.get_mut(&k)
    }
    fn set(&mut self, k: Key, v: V) {
        if self.vals.insert(k, v).is_none() {
            self.keys.push(k);
        }
    }
    fn iter(&self) -> impl Iterator<Item = (Key, &V)> {
        self.keys.iter().map(move |k| (*k, &self.vals[k]))
    }
}

/// A DAG adjacency entry: `{id, gap, direction}`.
#[derive(Clone, Copy)]
struct Adj {
    id: Key,
    gap: f64,
}

/// A relative constraint as `initConstraintVariables` stores it per node.
#[derive(Clone, Copy)]
enum RelSide {
    /// `{right: id, gap}` / `{bottom: id, gap}`: the other node comes after.
    After(Key, f64),
    /// `{left: id, gap}` / `{top: id, gap}`.
    Before(Key, f64),
}

#[derive(Default)]
struct RelaxState {
    dummy_to_node_v: OrderedMapVec,
    dummy_to_node_h: OrderedMapVec,
    in_relative_h: Vec<Key>,
    in_relative_v: Vec<Key>,
    rel_map_h: std::collections::HashMap<Key, Vec<RelSide>>,
    rel_map_v: std::collections::HashMap<Key, Vec<RelSide>>,
    temp_h: std::collections::HashMap<Key, f64>,
    temp_v: std::collections::HashMap<Key, f64>,
}

type OrderedMapVec = std::collections::HashMap<Key, Vec<usize>>;

struct Fcose {
    nodes: Vec<LNode>,
    graphs: Vec<LGraph>,
    edges: Vec<LEdge>,
    inter_edges: Vec<usize>,
    all_nodes: Vec<usize>,
    all_edges: Vec<usize>,
    gravity_nodes: Vec<usize>,
    /// Cytoscape node index → layout node.
    cy_to_l: Vec<usize>,
    constraints: Constraints,
    relax: Option<RelaxState>,
    rng: Mulberry32,
    // Constants derived from the edges (`DEFAULT_EDGE_LENGTH` and friends).
    default_edge_length: f64,
    ideal_edge_length: f64,
    min_repulsion_dist: f64,
    // Spring embedder state.
    cooling_factor: f64,
    initial_cooling_factor: f64,
    max_cooling_cycle: f64,
    cooling_cycle: usize,
    max_iterations: usize,
    total_iterations: usize,
    total_displacement: f64,
    old_total_displacement: f64,
    total_displacement_threshold: f64,
    max_node_displacement: f64,
    repulsion_range: f64,
    grid: Vec<Vec<Vec<usize>>>,
}

impl Fcose {
    /// fcose's `coseLayout`: the layout graph from the cytoscape elements.
    fn build(nodes: &[Node], edges: &[Edge], positions: &[(f64, f64)]) -> Fcose {
        let n = nodes.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut top: Vec<usize> = Vec::new();
        for (i, nd) in nodes.iter().enumerate() {
            match nd.parent {
                Some(p) if p < n => children[p].push(i),
                _ => top.push(i),
            }
        }
        let mut l = Fcose {
            nodes: Vec::new(),
            graphs: Vec::new(),
            edges: Vec::new(),
            inter_edges: Vec::new(),
            all_nodes: Vec::new(),
            all_edges: Vec::new(),
            gravity_nodes: Vec::new(),
            cy_to_l: vec![usize::MAX; n],
            constraints: Constraints::default(),
            relax: None,
            rng: Mulberry32(1),
            default_edge_length: 50.0,
            ideal_edge_length: 50.0,
            min_repulsion_dist: 5.0,
            cooling_factor: INITIAL_ENERGY,
            initial_cooling_factor: INITIAL_ENERGY,
            max_cooling_cycle: 0.0,
            cooling_cycle: 0,
            max_iterations: MAX_ITERATIONS,
            total_iterations: 0,
            total_displacement: 0.0,
            old_total_displacement: 0.0,
            total_displacement_threshold: 0.0,
            max_node_displacement: MAX_NODE_DISPLACEMENT_INCREMENTAL,
            repulsion_range: 0.0,
            grid: Vec::new(),
        };
        // gm.addRoot(): a graph whose parent is a padding-less dummy node.
        let root_parent = l.new_node(RectD::default(), None, usize::MAX);
        let root = l.new_graph(root_parent);
        l.nodes[root_parent].owner = usize::MAX;
        l.process_children(root, &top, nodes, &children, positions);
        // getAllNodes(): the graphs' node lists in creation order.
        l.all_nodes = l.graphs.iter().flat_map(|g| g.nodes.clone()).collect();
        // processEdges.
        let (mut total, mut count) = (0.0, 0usize);
        for e in edges {
            let (Some(&s), Some(&t)) = (l.cy_to_l.get(e.source), l.cy_to_l.get(e.target)) else {
                continue;
            };
            if s == usize::MAX || t == usize::MAX || s == t || l.edge_between(s, t) {
                continue;
            }
            l.add_edge(s, t, e.ideal_length, e.elasticity);
            total += e.ideal_length;
            count += 1;
        }
        l.default_edge_length = if count > 0 {
            total / count as f64
        } else {
            50.0
        };
        l.min_repulsion_dist = l.default_edge_length / 10.0;
        l.all_edges = l
            .graphs
            .iter()
            .flat_map(|g| g.edges.clone())
            .chain(l.inter_edges.clone())
            .collect();
        l
    }

    fn new_node(&mut self, rect: RectD, padding: Option<f64>, cy: usize) -> usize {
        self.nodes.push(LNode {
            rect,
            edges: Vec::new(),
            owner: usize::MAX,
            child: None,
            no_of_children: 1.0,
            estimated_size: 0.0,
            depth: 0.0,
            padding,
            spring: (0.0, 0.0),
            repulsion: (0.0, 0.0),
            gravitation: (0.0, 0.0),
            disp: (0.0, 0.0),
            grid: (0, 0, 0, 0),
            surrounding: Vec::new(),
            cy,
        });
        self.nodes.len() - 1
    }

    fn new_graph(&mut self, parent: usize) -> usize {
        self.graphs.push(LGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            parent,
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
            estimated_size: 0.0,
            connected: false,
        });
        let g = self.graphs.len() - 1;
        self.nodes[parent].child = Some(g);
        g
    }

    fn process_children(
        &mut self,
        graph: usize,
        list: &[usize],
        nodes: &[Node],
        children: &[Vec<usize>],
        positions: &[(f64, f64)],
    ) {
        for &c in list {
            let nd = &nodes[c];
            let (x, y) = positions.get(c).copied().unwrap_or((0.0, 0.0));
            let rect = RectD {
                x: x - nd.width / 2.0,
                y: y - nd.height / 2.0,
                w: nd.width,
                h: nd.height,
            };
            let v = self.new_node(rect, Some(nd.padding), c);
            self.nodes[v].owner = graph;
            self.graphs[graph].nodes.push(v);
            self.cy_to_l[c] = v;
            if !children[c].is_empty() {
                let g = self.new_graph(v);
                self.process_children(g, &children[c], nodes, children, positions);
            }
        }
    }

    fn edge_between(&self, a: usize, b: usize) -> bool {
        self.nodes[a].edges.iter().any(|&e| {
            let ed = &self.edges[e];
            ed.target == b || ed.source == b
        })
    }

    fn add_edge(&mut self, s: usize, t: usize, ideal: f64, elasticity: f64) {
        let inter = self.nodes[s].owner != self.nodes[t].owner;
        let e = self.edges.len();
        self.edges.push(LEdge {
            source: s,
            target: t,
            ideal,
            elasticity,
            inter,
            lca: 0,
            source_in_lca: s,
            target_in_lca: t,
        });
        if inter {
            self.inter_edges.push(e);
        } else {
            let g = self.nodes[s].owner;
            self.graphs[g].edges.push(e);
        }
        self.nodes[s].edges.push(e);
        if t != s {
            self.nodes[t].edges.push(e);
        }
    }

    fn other_end(&self, e: usize, n: usize) -> usize {
        let ed = &self.edges[e];
        if ed.source == n { ed.target } else { ed.source }
    }

    fn root(&self) -> usize {
        0
    }

    /// `LEdge.getOtherEndInGraph`.
    fn other_end_in_graph(&self, e: usize, n: usize, graph: usize) -> Option<usize> {
        let mut other = self.other_end(e, n);
        loop {
            let owner = self.nodes[other].owner;
            if owner == graph {
                return Some(other);
            }
            if owner == self.root() {
                return None;
            }
            other = self.graphs[owner].parent;
        }
    }

    /// `LNode.withChildren` (insertion order of the JS `Set`).
    fn with_children(&self, v: usize, out: &mut Vec<usize>) {
        if !out.contains(&v) {
            out.push(v);
        }
        if let Some(g) = self.nodes[v].child {
            for &c in &self.graphs[g].nodes {
                self.with_children(c, out);
            }
        }
    }

    /// `LGraph.updateConnected`.
    fn update_connected(&mut self, g: usize) {
        if self.graphs[g].nodes.is_empty() {
            self.graphs[g].connected = true;
            return;
        }
        let mut visited: Vec<usize> = Vec::new();
        let mut seen = vec![false; self.nodes.len()];
        let mut queue = std::collections::VecDeque::new();
        let mut first = Vec::new();
        self.with_children(self.graphs[g].nodes[0], &mut first);
        for v in first {
            queue.push_back(v);
            if !seen[v] {
                seen[v] = true;
                visited.push(v);
            }
        }
        while let Some(cur) = queue.pop_front() {
            for k in 0..self.nodes[cur].edges.len() {
                let e = self.nodes[cur].edges[k];
                if let Some(nb) = self.other_end_in_graph(e, cur, g) {
                    if !seen[nb] {
                        let mut kids = Vec::new();
                        self.with_children(nb, &mut kids);
                        for v in kids {
                            queue.push_back(v);
                            if !seen[v] {
                                seen[v] = true;
                                visited.push(v);
                            }
                        }
                    }
                }
            }
        }
        let n = self.graphs[g].nodes.len();
        self.graphs[g].connected = visited.len() >= n
            && visited
                .iter()
                .filter(|&&v| self.nodes[v].owner == g)
                .count()
                == n;
    }

    /// `LGraphManager.calcLowestCommonAncestors`.
    fn calc_lowest_common_ancestors(&mut self) {
        let root = self.root();
        for k in 0..self.all_edges.len() {
            let e = self.all_edges[k];
            let (s, t) = (self.edges[e].source, self.edges[e].target);
            self.edges[e].source_in_lca = s;
            self.edges[e].target_in_lca = t;
            if s == t {
                self.edges[e].lca = self.nodes[s].owner;
                continue;
            }
            let mut lca = None;
            let mut source_graph = self.nodes[s].owner;
            while lca.is_none() {
                self.edges[e].target_in_lca = t;
                let mut target_graph = self.nodes[t].owner;
                while lca.is_none() {
                    if target_graph == source_graph {
                        lca = Some(target_graph);
                        break;
                    }
                    if target_graph == root {
                        break;
                    }
                    let tin = self.graphs[target_graph].parent;
                    self.edges[e].target_in_lca = tin;
                    target_graph = self.nodes[tin].owner;
                }
                if source_graph == root {
                    break;
                }
                if lca.is_none() {
                    let sin = self.graphs[source_graph].parent;
                    self.edges[e].source_in_lca = sin;
                    source_graph = self.nodes[sin].owner;
                }
            }
            self.edges[e].lca = lca.unwrap_or(root);
        }
    }

    fn calc_inclusion_tree_depths(&mut self, g: usize, depth: f64) {
        for k in 0..self.graphs[g].nodes.len() {
            let v = self.graphs[g].nodes[k];
            self.nodes[v].depth = depth;
            if let Some(c) = self.nodes[v].child {
                self.calc_inclusion_tree_depths(c, depth + 1.0);
            }
        }
    }

    fn graph_depth(&self, g: usize) -> f64 {
        if g == self.root() {
            1.0
        } else {
            self.nodes[self.graphs[g].parent].depth
        }
    }

    fn node_estimated_size(&mut self, v: usize) -> f64 {
        match self.nodes[v].child {
            None => {
                let r = self.nodes[v].rect;
                self.nodes[v].estimated_size = (r.w + r.h) / 2.0;
            }
            Some(g) => {
                let s = self.graph_estimated_size(g);
                self.nodes[v].estimated_size = s;
                self.nodes[v].rect.w = s;
                self.nodes[v].rect.h = s;
            }
        }
        self.nodes[v].estimated_size
    }

    fn graph_estimated_size(&mut self, g: usize) -> f64 {
        let mut size = 0.0;
        for k in 0..self.graphs[g].nodes.len() {
            let v = self.graphs[g].nodes[k];
            size += self.node_estimated_size(v);
        }
        let est = if size == 0.0 {
            EMPTY_COMPOUND_NODE_SIZE
        } else {
            size / (self.graphs[g].nodes.len() as f64).sqrt()
        };
        self.graphs[g].estimated_size = est;
        est
    }

    fn no_of_children(&self, v: usize) -> f64 {
        let n = match self.nodes[v].child {
            None => 1.0,
            Some(g) => self.graphs[g]
                .nodes
                .iter()
                .map(|&c| self.no_of_children(c))
                .sum(),
        };
        if n == 0.0 { 1.0 } else { n }
    }

    /// `CoSELayout.classicLayout` (incremental) and the spring embedder.
    fn run(&mut self) {
        if self.graphs[self.root()].nodes.is_empty() {
            return;
        }
        // calculateNodesToApplyGravitationTo.
        let mut gravity = Vec::new();
        for g in 0..self.graphs.len() {
            self.update_connected(g);
            if !self.graphs[g].connected {
                gravity.extend(self.graphs[g].nodes.iter().copied());
            }
        }
        self.gravity_nodes = gravity;
        for k in 0..self.all_nodes.len() {
            let v = self.all_nodes[k];
            self.nodes[v].no_of_children = self.no_of_children(v);
        }
        self.calc_lowest_common_ancestors();
        let root = self.root();
        self.calc_inclusion_tree_depths(root, 1.0);
        self.graph_estimated_size(root);
        self.calc_ideal_edge_lengths();
        // mermaid always passes an `alignmentConstraint` object (possibly
        // with both lists empty), so fcose takes its constrained path on every
        // architecture: no tiling, no tree reduction, and the constraint
        // handler re-centres every leaf even when it has nothing to enforce.
        self.handle_constraints();
        self.init_constraint_variables();
        self.init_spring_embedder();
        while !self.tick() {}
        self.update_bounds();
    }

    /// `FDLayout.calcIdealEdgeLengths`.
    fn calc_ideal_edge_lengths(&mut self) {
        for k in 0..self.all_edges.len() {
            let e = self.all_edges[k];
            if !self.edges[e].inter {
                continue;
            }
            let original = self.edges[e].ideal;
            let ed = &self.edges[e];
            let size_s = self.nodes[ed.source_in_lca].estimated_size;
            let size_t = self.nodes[ed.target_in_lca].estimated_size;
            let lca_depth = self.graph_depth(ed.lca);
            let depths = self.nodes[ed.source].depth + self.nodes[ed.target].depth;
            let mut ideal = original;
            ideal += size_s + size_t - 2.0 * SIMPLE_NODE_SIZE;
            ideal += original * NESTING_FACTOR * (depths - 2.0 * lca_depth);
            self.edges[e].ideal = ideal;
        }
    }

    /// `FDLayout.initSpringEmbedder` (incremental) + CoSE's additions.
    fn init_spring_embedder(&mut self) {
        let s = self.all_nodes.len() as f64;
        if s > 1000.0 {
            self.cooling_factor = (self.cooling_factor * 0.33).max(
                self.cooling_factor
                    - (s - 1000.0) / (5000.0 - 1000.0) * self.cooling_factor * (1.0 - 0.33),
            );
        }
        self.max_node_displacement = MAX_NODE_DISPLACEMENT_INCREMENTAL;
        self.max_iterations = (self.all_nodes.len() * 5).max(MAX_ITERATIONS);
        self.total_displacement_threshold = 3.0 * self.default_edge_length / 100.0 * s;
        self.ideal_edge_length = self.default_edge_length.max(10.0);
        // CoSELayout.calcRepulsionRange: 2 · (level + 1) · idealEdgeLength.
        self.repulsion_range = 2.0 * self.ideal_edge_length;
        self.cooling_cycle = 0;
        self.max_cooling_cycle = self.max_iterations as f64 / CONVERGENCE_CHECK_PERIOD as f64;
    }

    fn is_converged(&mut self) -> bool {
        let mut oscillating = false;
        if self.total_iterations as f64 > self.max_iterations as f64 / 3.0 {
            oscillating = (self.total_displacement - self.old_total_displacement).abs() < 2.0;
        }
        let converged = self.total_displacement < self.total_displacement_threshold;
        self.old_total_displacement = self.total_displacement;
        converged || oscillating
    }

    /// `CoSELayout.tick`.
    fn tick(&mut self) -> bool {
        self.total_iterations += 1;
        if self.total_iterations == self.max_iterations {
            return true;
        }
        if self.total_iterations % CONVERGENCE_CHECK_PERIOD == 0 {
            if self.is_converged() {
                return true;
            }
            self.cooling_cycle += 1;
            let exponent = jsmath::ln(100.0 * (self.initial_cooling_factor - FINAL_TEMPERATURE))
                / jsmath::ln(self.max_cooling_cycle);
            self.cooling_factor = (self.initial_cooling_factor
                - jsmath::pow(self.cooling_cycle as f64, exponent) / 100.0 * 1.0)
                .max(FINAL_TEMPERATURE);
        }
        self.total_displacement = 0.0;
        self.update_bounds();
        self.calc_spring_forces();
        self.calc_repulsion_forces();
        self.calc_gravitational_forces();
        self.move_nodes();
        false
    }

    /// `LGraphManager.updateBounds`: the root graph, recursively.
    fn update_bounds(&mut self) {
        let root = self.root();
        self.update_graph_bounds(root);
    }

    fn update_graph_bounds(&mut self, g: usize) {
        let (mut left, mut right, mut top, mut bottom) = (INT_MAX, -INT_MAX, INT_MAX, -INT_MAX);
        for k in 0..self.graphs[g].nodes.len() {
            let v = self.graphs[g].nodes[k];
            if let Some(c) = self.nodes[v].child {
                if !self.graphs[c].nodes.is_empty() {
                    self.update_graph_bounds(c);
                    let cg = &self.graphs[c];
                    let (l, t, r, b) = (cg.left, cg.top, cg.right, cg.bottom);
                    let rect = &mut self.nodes[v].rect;
                    rect.x = l;
                    rect.y = t;
                    rect.w = r - l;
                    rect.h = b - t;
                }
            }
            let r = self.nodes[v].rect;
            left = left.min(r.x);
            right = right.max(r.x + r.w);
            top = top.min(r.y);
            bottom = bottom.max(r.y + r.h);
        }
        let parent = self.graphs[g].parent;
        if left == INT_MAX {
            let p = self.nodes[parent].rect;
            let gr = &mut self.graphs[g];
            gr.left = p.x;
            gr.right = p.x + p.w;
            gr.top = p.y;
            gr.bottom = p.y + p.h;
            return;
        }
        let margin = self.nodes[parent].padding.unwrap_or(ROOT_MARGIN);
        let (w, h) = (right - left, bottom - top);
        let gr = &mut self.graphs[g];
        gr.left = left - margin;
        gr.right = left + w + margin;
        gr.top = top - margin;
        gr.bottom = top + h + margin;
    }

    fn calc_spring_forces(&mut self) {
        for k in 0..self.all_edges.len() {
            let e = self.all_edges[k];
            let (s, t) = (self.edges[e].source, self.edges[e].target);
            let (clip, overlapping) = clip_points(&self.nodes[t].rect, &self.nodes[s].rect);
            if overlapping {
                continue;
            }
            let mut lx = clip[0] - clip[2];
            let mut ly = clip[1] - clip[3];
            if lx.abs() < 1.0 {
                lx = sign(lx);
            }
            if ly.abs() < 1.0 {
                ly = sign(ly);
            }
            let length = (lx * lx + ly * ly).sqrt();
            if length == 0.0 {
                continue;
            }
            let force = self.edges[e].elasticity * (length - self.edges[e].ideal);
            let fx = force * (lx / length);
            let fy = force * (ly / length);
            self.nodes[s].spring.0 += fx;
            self.nodes[s].spring.1 += fy;
            self.nodes[t].spring.0 -= fx;
            self.nodes[t].spring.1 -= fy;
        }
    }

    fn calc_repulsion_forces(&mut self) {
        let refresh = self.total_iterations % GRID_CALCULATION_CHECK_PERIOD == 1;
        if refresh {
            self.update_grid();
        }
        let mut processed = vec![false; self.nodes.len()];
        for k in 0..self.all_nodes.len() {
            let a = self.all_nodes[k];
            if refresh {
                self.find_surrounding(a, &processed);
            }
            for j in 0..self.nodes[a].surrounding.len() {
                let b = self.nodes[a].surrounding[j];
                self.calc_repulsion_force(a, b);
            }
            processed[a] = true;
        }
    }

    fn update_grid(&mut self) {
        let root = &self.graphs[self.root()];
        let (left, top) = (root.left, root.top);
        let range = self.repulsion_range;
        let size_x = ((root.right - root.left) / range).ceil().max(0.0) as usize;
        let size_y = ((root.bottom - root.top) / range).ceil().max(0.0) as usize;
        self.grid = vec![vec![Vec::new(); size_y]; size_x];
        for k in 0..self.all_nodes.len() {
            let v = self.all_nodes[k];
            let r = self.nodes[v].rect;
            let sx = ((r.x - left) / range).floor() as i64;
            let fx = ((r.w + r.x - left) / range).floor() as i64;
            let sy = ((r.y - top) / range).floor() as i64;
            let fy = ((r.h + r.y - top) / range).floor() as i64;
            for i in sx..=fx {
                for j in sy..=fy {
                    if let Some(cell) = usize::try_from(i)
                        .ok()
                        .zip(usize::try_from(j).ok())
                        .and_then(|(i, j)| self.grid.get_mut(i)?.get_mut(j))
                    {
                        cell.push(v);
                    }
                }
            }
            self.nodes[v].grid = (sx, fx, sy, fy);
        }
    }

    fn find_surrounding(&mut self, a: usize, processed: &[bool]) {
        let (sx, fx, sy, fy) = self.nodes[a].grid;
        let ra = self.nodes[a].rect;
        let owner = self.nodes[a].owner;
        let range = self.repulsion_range;
        let gx = self.grid.len() as i64;
        let gy = self.grid.first().map_or(0, Vec::len) as i64;
        let mut surrounding: Vec<usize> = Vec::new();
        for i in sx - 1..fx + 2 {
            for j in sy - 1..fy + 2 {
                if i < 0 || j < 0 || i >= gx || j >= gy {
                    continue;
                }
                for &b in &self.grid[i as usize][j as usize] {
                    if self.nodes[b].owner != owner || b == a {
                        continue;
                    }
                    if processed[b] || surrounding.contains(&b) {
                        continue;
                    }
                    let rb = self.nodes[b].rect;
                    let dx = (ra.center_x() - rb.center_x()).abs() - (ra.w / 2.0 + rb.w / 2.0);
                    let dy = (ra.center_y() - rb.center_y()).abs() - (ra.h / 2.0 + rb.h / 2.0);
                    if dx <= range && dy <= range {
                        surrounding.push(b);
                    }
                }
            }
        }
        self.nodes[a].surrounding = surrounding;
    }

    fn calc_repulsion_force(&mut self, a: usize, b: usize) {
        let ra = self.nodes[a].rect;
        let rb = self.nodes[b].rect;
        let (na, nb) = (self.nodes[a].no_of_children, self.nodes[b].no_of_children);
        if ra.intersects(&rb) {
            let o = separation_amount(&ra, &rb, self.default_edge_length / 2.0);
            let (rx, ry) = (2.0 * o[0], 2.0 * o[1]);
            let c = na * nb / (na + nb);
            self.nodes[a].repulsion.0 -= c * rx;
            self.nodes[a].repulsion.1 -= c * ry;
            self.nodes[b].repulsion.0 += c * rx;
            self.nodes[b].repulsion.1 += c * ry;
            return;
        }
        let (clip, _) = clip_points(&ra, &rb);
        let mut dx = clip[2] - clip[0];
        let mut dy = clip[3] - clip[1];
        if dx.abs() < self.min_repulsion_dist {
            dx = sign(dx) * self.min_repulsion_dist;
        }
        if dy.abs() < self.min_repulsion_dist {
            dy = sign(dy) * self.min_repulsion_dist;
        }
        let dist_sq = dx * dx + dy * dy;
        let dist = dist_sq.sqrt();
        let force = (NODE_REPULSION / 2.0 + NODE_REPULSION / 2.0) * na * nb / dist_sq;
        let fx = force * dx / dist;
        let fy = force * dy / dist;
        self.nodes[a].repulsion.0 -= fx;
        self.nodes[a].repulsion.1 -= fy;
        self.nodes[b].repulsion.0 += fx;
        self.nodes[b].repulsion.1 += fy;
    }

    fn calc_gravitational_forces(&mut self) {
        let root = self.root();
        for k in 0..self.gravity_nodes.len() {
            let v = self.gravity_nodes[k];
            let g = self.nodes[v].owner;
            let gr = &self.graphs[g];
            let cx = (gr.right + gr.left) / 2.0;
            let cy = (gr.top + gr.bottom) / 2.0;
            let r = self.nodes[v].rect;
            let dx = r.center_x() - cx;
            let dy = r.center_y() - cy;
            let ax = dx.abs() + r.w / 2.0;
            let ay = dy.abs() + r.h / 2.0;
            if g == root {
                let est = gr.estimated_size * GRAVITY_RANGE;
                if ax > est || ay > est {
                    self.nodes[v].gravitation = (-GRAVITY * dx, -GRAVITY * dy);
                }
            } else {
                let est = gr.estimated_size * COMPOUND_GRAVITY_RANGE;
                if ax > est || ay > est {
                    self.nodes[v].gravitation = (
                        -GRAVITY * dx * COMPOUND_GRAVITY,
                        -GRAVITY * dy * COMPOUND_GRAVITY,
                    );
                }
            }
        }
    }

    /// `CoSENode.propogateDisplacementToChildren`.
    fn propagate(&mut self, v: usize, dx: f64, dy: f64) {
        let Some(g) = self.nodes[v].child else {
            return;
        };
        for k in 0..self.graphs[g].nodes.len() {
            let c = self.graphs[g].nodes[k];
            if self.nodes[c].child.is_none() {
                self.nodes[c].disp.0 += dx;
                self.nodes[c].disp.1 += dy;
            } else {
                self.propagate(c, dx, dy);
            }
        }
    }

    /// `CoSELayout.moveNodes`.
    fn move_nodes(&mut self) {
        let cf = self.cooling_factor;
        let limit = cf * self.max_node_displacement;
        for k in 0..self.all_nodes.len() {
            let v = self.all_nodes[k];
            let nd = &mut self.nodes[v];
            nd.disp.0 += cf * (nd.spring.0 + nd.repulsion.0 + nd.gravitation.0) / nd.no_of_children;
            nd.disp.1 += cf * (nd.spring.1 + nd.repulsion.1 + nd.gravitation.1) / nd.no_of_children;
            if nd.disp.0.abs() > limit {
                nd.disp.0 = limit * sign(nd.disp.0);
            }
            if nd.disp.1.abs() > limit {
                nd.disp.1 = limit * sign(nd.disp.1);
            }
            let has_children = nd.child.is_some_and(|g| !self.graphs[g].nodes.is_empty());
            if has_children {
                let (dx, dy) = nd.disp;
                self.propagate(v, dx, dy);
            }
        }
        self.update_displacements();
        for k in 0..self.all_nodes.len() {
            let v = self.all_nodes[k];
            let leaf = self.nodes[v]
                .child
                .is_none_or(|g| self.graphs[g].nodes.is_empty());
            let nd = &mut self.nodes[v];
            if leaf {
                nd.rect.x += nd.disp.0;
                nd.rect.y += nd.disp.1;
                self.total_displacement += nd.disp.0.abs() + nd.disp.1.abs();
            }
            nd.spring = (0.0, 0.0);
            nd.repulsion = (0.0, 0.0);
            nd.gravitation = (0.0, 0.0);
            nd.disp = (0.0, 0.0);
        }
    }

    fn leaf_of(&self, cy: usize) -> Option<usize> {
        let v = *self.cy_to_l.get(cy)?;
        (v != usize::MAX && self.nodes[v].child.is_none()).then_some(v)
    }

    /// `ConstraintHandler.handleConstraints` with `ENFORCE_CONSTRAINTS` only
    /// (no transformation: `randomize` is false) and no fixed nodes.
    fn handle_constraints(&mut self) {
        // Leaves in getAllNodes order.
        let mut index_of = std::collections::HashMap::new();
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for &v in &self.all_nodes {
            if self.nodes[v].child.is_none() {
                index_of.insert(self.nodes[v].cy, xs.len());
                xs.push(self.nodes[v].rect.center_x());
                ys.push(self.nodes[v].rect.center_y());
            }
        }
        let c = self.constraints.clone();
        let idx = |cy: usize| index_of.get(&cy).copied();
        let avg = |set: &[usize], coords: &[f64]| {
            let mut sum = 0.0;
            let mut n = 0.0;
            for &id in set {
                if let Some(i) = idx(id) {
                    sum += coords[i];
                }
                n += 1.0;
            }
            sum / n
        };
        for set in &c.vertical {
            let set = dedup(set);
            let x = avg(&set, &xs);
            for &id in &set {
                if let Some(i) = idx(id) {
                    xs[i] = x;
                }
            }
        }
        for set in &c.horizontal {
            let set = dedup(set);
            let y = avg(&set, &ys);
            for &id in &set {
                if let Some(i) = idx(id) {
                    ys[i] = y;
                }
            }
        }
        if !c.relative.is_empty() {
            // The DAG of relative placements, keyed in constraint order.
            let mut dag: OrderedMap<Vec<(Adj, bool)>> = OrderedMap::new();
            for r in &c.relative {
                let (a, b, gap, horizontal) = match *r {
                    Relative::LeftRight(a, b, g) => (a, b, g, true),
                    Relative::TopBottom(a, b, g) => (a, b, g, false),
                };
                let adj = Adj {
                    id: Key::Node(b),
                    gap,
                };
                match dag.get_mut(Key::Node(a)) {
                    Some(list) => list.push((adj, horizontal)),
                    None => dag.set(Key::Node(a), vec![(adj, horizontal)]),
                }
                if !dag.has(Key::Node(b)) {
                    dag.set(Key::Node(b), Vec::new());
                }
            }
            let mut node_to_dummy_v = std::collections::HashMap::new();
            let mut node_to_dummy_h = std::collections::HashMap::new();
            let mut dummy_to_node_v: OrderedMap<Vec<usize>> = OrderedMap::new();
            let mut dummy_to_node_h: OrderedMap<Vec<usize>> = OrderedMap::new();
            let mut dummy_pos_v = std::collections::HashMap::new();
            let mut dummy_pos_h = std::collections::HashMap::new();
            for (i, set) in c.vertical.iter().enumerate() {
                dummy_to_node_v.set(Key::Dummy(i), Vec::new());
                for &id in set {
                    node_to_dummy_v.insert(id, i);
                    if let Some(l) = dummy_to_node_v.get_mut(Key::Dummy(i)) {
                        l.push(id);
                    }
                }
                if let Some(i0) = set.first().and_then(|&id| idx(id)) {
                    dummy_pos_v.insert(Key::Dummy(i), xs[i0]);
                }
            }
            for (i, set) in c.horizontal.iter().enumerate() {
                dummy_to_node_h.set(Key::Dummy(i), Vec::new());
                for &id in set {
                    node_to_dummy_h.insert(id, i);
                    if let Some(l) = dummy_to_node_h.get_mut(Key::Dummy(i)) {
                        l.push(id);
                    }
                }
                if let Some(i0) = set.first().and_then(|&id| idx(id)) {
                    dummy_pos_h.insert(Key::Dummy(i), ys[i0]);
                }
            }
            let mut dag_h: OrderedMap<Vec<Adj>> = OrderedMap::new();
            let mut dag_v: OrderedMap<Vec<Adj>> = OrderedMap::new();
            for (key, list) in dag.iter() {
                let Key::Node(node) = key else { continue };
                for &(adj, horizontal) in list {
                    let Key::Node(target) = adj.id else { continue };
                    let (to_dummy, dg) = if horizontal {
                        (&node_to_dummy_v, &mut dag_h)
                    } else {
                        (&node_to_dummy_h, &mut dag_v)
                    };
                    let source = to_dummy
                        .get(&node)
                        .map_or(Key::Node(node), |&d| Key::Dummy(d));
                    let target = Adj {
                        id: to_dummy
                            .get(&target)
                            .map_or(Key::Node(target), |&d| Key::Dummy(d)),
                        gap: adj.gap,
                    };
                    match dg.get_mut(source) {
                        Some(l) => l.push(target),
                        None => dg.set(source, vec![target]),
                    }
                    if !dg.has(target.id) {
                        dg.set(target.id, Vec::new());
                    }
                }
            }
            let pos_h = place_relative(&dag_h, &index_of, &xs, &dummy_pos_v);
            let pos_v = place_relative(&dag_v, &index_of, &ys, &dummy_pos_h);
            for (key, &p) in pos_h.iter() {
                match key {
                    Key::Dummy(_) => {
                        if let Some(list) = dummy_to_node_v.get(key) {
                            for &id in list {
                                if let Some(i) = idx(id) {
                                    xs[i] = p;
                                }
                            }
                        }
                    }
                    Key::Node(id) => {
                        if let Some(i) = idx(id) {
                            xs[i] = p;
                        }
                    }
                }
            }
            for (key, &p) in pos_v.iter() {
                match key {
                    Key::Dummy(_) => {
                        if let Some(list) = dummy_to_node_h.get(key) {
                            for &id in list {
                                if let Some(i) = idx(id) {
                                    ys[i] = p;
                                }
                            }
                        }
                    }
                    Key::Node(id) => {
                        if let Some(i) = idx(id) {
                            ys[i] = p;
                        }
                    }
                }
            }
        }
        for k in 0..self.all_nodes.len() {
            let v = self.all_nodes[k];
            if self.nodes[v].child.is_none() {
                if let Some(&i) = index_of.get(&self.nodes[v].cy) {
                    let r = &mut self.nodes[v].rect;
                    r.x = xs[i] - r.w / 2.0;
                    r.y = ys[i] - r.h / 2.0;
                }
            }
        }
    }

    /// `CoSELayout.initConstraintVariables` (`RELAX_MOVEMENT_ON_CONSTRAINTS`).
    fn init_constraint_variables(&mut self) {
        if self.constraints.relative.is_empty() {
            return;
        }
        let c = self.constraints.clone();
        let mut st = RelaxState::default();
        let mut node_to_dummy_v = std::collections::HashMap::new();
        let mut node_to_dummy_h = std::collections::HashMap::new();
        for (i, set) in c.vertical.iter().enumerate() {
            st.dummy_to_node_v.insert(Key::Dummy(i), Vec::new());
            for &id in set {
                node_to_dummy_v.insert(id, i);
                if let Some(l) = st.dummy_to_node_v.get_mut(&Key::Dummy(i)) {
                    l.push(id);
                }
            }
        }
        for (i, set) in c.horizontal.iter().enumerate() {
            st.dummy_to_node_h.insert(Key::Dummy(i), Vec::new());
            for &id in set {
                node_to_dummy_h.insert(id, i);
                if let Some(l) = st.dummy_to_node_h.get_mut(&Key::Dummy(i)) {
                    l.push(id);
                }
            }
        }
        let center_x =
            |l: &Fcose, id: usize| l.leaf_of(id).map_or(0.0, |v| l.nodes[v].rect.center_x());
        let center_y =
            |l: &Fcose, id: usize| l.leaf_of(id).map_or(0.0, |v| l.nodes[v].rect.center_y());
        for r in &c.relative {
            match *r {
                Relative::LeftRight(a, b, gap) => {
                    let ka = node_to_dummy_v
                        .get(&a)
                        .map_or(Key::Node(a), |&d| Key::Dummy(d));
                    let kb = node_to_dummy_v
                        .get(&b)
                        .map_or(Key::Node(b), |&d| Key::Dummy(d));
                    for k in [ka, kb] {
                        if !st.in_relative_h.contains(&k) {
                            st.in_relative_h.push(k);
                            st.rel_map_h.insert(k, Vec::new());
                            let pos = match k {
                                Key::Dummy(_) => st.dummy_to_node_v[&k]
                                    .first()
                                    .map_or(0.0, |&id| center_x(self, id)),
                                Key::Node(id) => center_x(self, id),
                            };
                            st.temp_h.insert(k, pos);
                        }
                    }
                    st.rel_map_h
                        .entry(ka)
                        .or_default()
                        .push(RelSide::After(kb, gap));
                    st.rel_map_h
                        .entry(kb)
                        .or_default()
                        .push(RelSide::Before(ka, gap));
                }
                Relative::TopBottom(a, b, gap) => {
                    let ka = node_to_dummy_h
                        .get(&a)
                        .map_or(Key::Node(a), |&d| Key::Dummy(d));
                    let kb = node_to_dummy_h
                        .get(&b)
                        .map_or(Key::Node(b), |&d| Key::Dummy(d));
                    for k in [ka, kb] {
                        if !st.in_relative_v.contains(&k) {
                            st.in_relative_v.push(k);
                            st.rel_map_v.insert(k, Vec::new());
                            let pos = match k {
                                Key::Dummy(_) => st.dummy_to_node_h[&k]
                                    .first()
                                    .map_or(0.0, |&id| center_y(self, id)),
                                Key::Node(id) => center_y(self, id),
                            };
                            st.temp_v.insert(k, pos);
                        }
                    }
                    st.rel_map_v
                        .entry(ka)
                        .or_default()
                        .push(RelSide::After(kb, gap));
                    st.rel_map_v
                        .entry(kb)
                        .or_default()
                        .push(RelSide::Before(ka, gap));
                }
            }
        }
        self.relax = Some(st);
    }

    /// `CoSELayout.updateDisplacements` (no fixed nodes).
    fn update_displacements(&mut self) {
        let c = &self.constraints;
        for set in &c.vertical {
            let mut total = 0.0;
            for &id in set {
                if let Some(v) = self.leaf_of(id) {
                    total += self.nodes[v].disp.0;
                }
            }
            let avg = total / set.len() as f64;
            for &id in set {
                if let Some(v) = self.leaf_of(id) {
                    self.nodes[v].disp.0 = avg;
                }
            }
        }
        for set in &c.horizontal {
            let mut total = 0.0;
            for &id in set {
                if let Some(v) = self.leaf_of(id) {
                    total += self.nodes[v].disp.1;
                }
            }
            let avg = total / set.len() as f64;
            for &id in set {
                if let Some(v) = self.leaf_of(id) {
                    self.nodes[v].disp.1 = avg;
                }
            }
        }
        let Some(mut st) = self.relax.take() else {
            return;
        };
        if self.total_iterations % 10 == 0 {
            shuffle(&mut st.in_relative_h, &mut self.rng);
            shuffle(&mut st.in_relative_v, &mut self.rng);
        }
        for axis in [true, false] {
            let order = if axis {
                st.in_relative_h.clone()
            } else {
                st.in_relative_v.clone()
            };
            for key in order {
                let (dummies, rel, temp) = if axis {
                    (&st.dummy_to_node_v, &st.rel_map_h, &mut st.temp_h)
                } else {
                    (&st.dummy_to_node_h, &st.rel_map_v, &mut st.temp_v)
                };
                let disp_of = |l: &Fcose, id: usize| {
                    l.leaf_of(id).map_or(0.0, |v| {
                        if axis {
                            l.nodes[v].disp.0
                        } else {
                            l.nodes[v].disp.1
                        }
                    })
                };
                let mut displacement = match key {
                    Key::Dummy(_) => dummies
                        .get(&key)
                        .and_then(|l| l.first())
                        .map_or(0.0, |&id| disp_of(self, id)),
                    Key::Node(id) => disp_of(self, id),
                };
                let here = temp.get(&key).copied().unwrap_or(0.0);
                for side in rel.get(&key).map(Vec::as_slice).unwrap_or(&[]) {
                    match *side {
                        RelSide::After(other, gap) => {
                            let diff =
                                temp.get(&other).copied().unwrap_or(0.0) - here - displacement;
                            if diff < gap {
                                displacement -= gap - diff;
                            }
                        }
                        RelSide::Before(other, gap) => {
                            let diff =
                                here - temp.get(&other).copied().unwrap_or(0.0) + displacement;
                            if diff < gap {
                                displacement += gap - diff;
                            }
                        }
                    }
                }
                temp.insert(key, here + displacement);
                let targets: Vec<usize> = match key {
                    Key::Dummy(_) => dummies.get(&key).cloned().unwrap_or_default(),
                    Key::Node(id) => vec![id],
                };
                for id in targets {
                    if let Some(v) = self.leaf_of(id) {
                        if axis {
                            self.nodes[v].disp.0 = displacement;
                        } else {
                            self.nodes[v].disp.1 = displacement;
                        }
                    }
                }
            }
        }
        self.relax = Some(st);
    }
}

/// The relaxed-movement shuffle: Fisher–Yates over the last third only.
fn shuffle(a: &mut [Key], rng: &mut Mulberry32) {
    let n = a.len();
    if n == 0 {
        return;
    }
    let mut i = n - 1;
    while i as f64 >= 2.0 * n as f64 / 3.0 {
        let j = (rng.next() * (i + 1) as f64).floor() as usize;
        a.swap(i, j);
        if i == 0 {
            break;
        }
        i -= 1;
    }
}

fn dedup(set: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for &id in set {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `findAppropriatePositionForRelativePlacement` with an empty set of fixed
/// nodes (fcose passes a `Set`, so the fixed-node branches run).
fn place_relative(
    dag: &OrderedMap<Vec<Adj>>,
    index_of: &std::collections::HashMap<usize, usize>,
    coords: &[f64],
    dummy_pos: &std::collections::HashMap<Key, f64>,
) -> OrderedMap<f64> {
    let pos_before = |k: Key| match k {
        Key::Node(id) => index_of.get(&id).map_or(0.0, |&i| coords[i]),
        Key::Dummy(_) => dummy_pos.get(&k).copied().unwrap_or(0.0),
    };
    // Components of the undirected DAG and their sources.
    let mut undirected: OrderedMap<Vec<Key>> = OrderedMap::new();
    for (k, _) in dag.iter() {
        undirected.set(k, Vec::new());
    }
    let mut reversed: std::collections::HashMap<Key, usize> = std::collections::HashMap::new();
    for (k, list) in dag.iter() {
        for a in list {
            if let Some(l) = undirected.get_mut(k) {
                l.push(a.id);
            }
            if let Some(l) = undirected.get_mut(a.id) {
                l.push(k);
            }
            *reversed.entry(a.id).or_default() += 1;
        }
    }
    let mut components: Vec<Vec<Key>> = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for (k, _) in undirected.iter() {
        if visited.contains(&k) {
            continue;
        }
        let mut comp = vec![k];
        visited.insert(k);
        let mut queue = std::collections::VecDeque::from([k]);
        while let Some(cur) = queue.pop_front() {
            for &nb in undirected.get(cur).map(Vec::as_slice).unwrap_or(&[]) {
                if visited.insert(nb) {
                    queue.push_back(nb);
                    comp.push(nb);
                }
            }
        }
        components.push(comp);
    }
    let sources: Vec<Vec<Key>> = components
        .iter()
        .map(|c| {
            c.iter()
                .copied()
                .filter(|k| reversed.get(k).copied().unwrap_or(0) == 0)
                .collect()
        })
        .collect();

    let mut in_degree: OrderedMap<usize> = OrderedMap::new();
    for (k, _) in dag.iter() {
        in_degree.set(k, 0);
    }
    for (_, list) in dag.iter() {
        for a in list {
            if let Some(d) = in_degree.get_mut(a.id) {
                *d += 1;
            }
        }
    }
    let mut position: OrderedMap<f64> = OrderedMap::new();
    let mut past: std::collections::HashMap<Key, Vec<Key>> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    for (k, &d) in in_degree.iter() {
        if d == 0 {
            queue.push_back(k);
        } else {
            position.set(k, f64::NEG_INFINITY);
        }
        past.insert(k, vec![k]);
    }
    // Every source of a component starts at the component sources' mean.
    for comp in &sources {
        let mut p = 0.0;
        for &k in comp {
            p += pos_before(k);
        }
        p /= comp.len() as f64;
        for &k in comp {
            position.set(k, p);
        }
    }
    let get = |m: &OrderedMap<f64>, k: Key| m.get(k).copied().unwrap_or(f64::NAN);
    while let Some(cur) = queue.pop_front() {
        for a in dag.get(cur).map(Vec::as_slice).unwrap_or(&[]) {
            if get(&position, a.id) < get(&position, cur) + a.gap {
                position.set(a.id, get(&position, cur) + a.gap);
            }
            if let Some(d) = in_degree.get_mut(a.id) {
                *d -= 1;
                if *d == 0 {
                    queue.push_back(a.id);
                }
            }
            // pastMap: union of the current node's past into the neighbour's.
            let mut union = past.get(&cur).cloned().unwrap_or_default();
            for k in past.get(&a.id).cloned().unwrap_or_default() {
                if !union.contains(&k) {
                    union.push(k);
                }
            }
            past.insert(a.id, union);
        }
    }
    // Re-centre the components (grouped by the sinks' pasts) on their spans before.
    let sinks: std::collections::HashSet<Key> = dag
        .iter()
        .filter(|(_, l)| l.is_empty())
        .map(|(k, _)| k)
        .collect();
    let mut comps: Vec<Vec<Key>> = Vec::new();
    for (k, _) in in_degree.iter() {
        if !sinks.contains(&k) {
            continue;
        }
        let value = past.get(&k).cloned().unwrap_or_default();
        let first = value.first().copied();
        let at = first.and_then(|f| comps.iter().rposition(|c| c.contains(&f)));
        match at {
            None => comps.push(value),
            Some(i) => {
                for kk in value {
                    if !comps[i].contains(&kk) {
                        comps[i].push(kk);
                    }
                }
            }
        }
    }
    for comp in comps {
        let (mut min_b, mut max_b, mut min_a, mut max_a) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for &k in &comp {
            let b = pos_before(k);
            let a = get(&position, k);
            min_b = min_b.min(b);
            max_b = max_b.max(b);
            min_a = min_a.min(a);
            max_a = max_a.max(a);
        }
        let diff = (min_b + max_b) / 2.0 - (min_a + max_a) / 2.0;
        for &k in &comp {
            let p = get(&position, k) + diff;
            position.set(k, p);
        }
    }
    position
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_matches_mermaids_seeded_random() {
        let mut r = Mulberry32(1);
        // withSeededRandom(1, () => [Math.random(), Math.random(), Math.random()])
        let got = [r.next(), r.next(), r.next()];
        assert_eq!(
            got,
            [0.6270739405881613, 0.002735721180215478, 0.5274470399599522]
        );
    }
}
