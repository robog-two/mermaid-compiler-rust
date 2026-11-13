//! CoSE-Bilkent (cytoscape-cose-bilkent 4.1.0 on cose-base 1.0.3 / layout-base
//! 1.0.2), as mermaid 12.3 runs it for mindmaps: `quality: 'proof'`,
//! `randomize: true`, everything else default.
//!
//! Only flat trees occur there (a mindmap is one tree, no compound nodes), and
//! for a flat forest CoSE starts from its deterministic radial placement
//! instead of random positions, so the whole run is deterministic and is
//! reproduced here step by step — same iteration order, same floating-point
//! operation order — to land on mermaid's coordinates.
//!
//! Upstream quirks kept on purpose: `findCenterOfTree` removes every other
//! node of its list instead of the leaves, the radial layout's angles wrap with
//! `%`, and the repulsion neighbourhood is only refreshed every tenth iteration.

use super::geometry::{RectD, clip_points, separation_amount, sign};
use super::jsmath;

/// `FDLayoutConstants` as cose-bilkent's defaults set them.
const IDEAL_EDGE_LENGTH: f64 = 50.0;
const SPRING_CONSTANT: f64 = 0.45;
const REPULSION_CONSTANT: f64 = 4500.0;
const GRAVITY_CONSTANT: f64 = 0.25;
const GRAVITY_RANGE_FACTOR: f64 = 3.8;
const MAX_ITERATIONS: usize = 2500;
const CONVERGENCE_CHECK_PERIOD: usize = 100;
const GRID_CALCULATION_CHECK_PERIOD: usize = 10;
/// `MAX_NODE_DISPLACEMENT_INCREMENTAL * 3`.
const MAX_NODE_DISPLACEMENT: f64 = 300.0;
/// `DEFAULT_EDGE_LENGTH / 10`, fixed when the constants module loads.
const MIN_REPULSION_DIST: f64 = 5.0;
/// `DEFAULT_GRAPH_MARGIN` of the root graph.
const GRAPH_MARGIN: f64 = 15.0;
/// `CoSEConstants.DEFAULT_RADIAL_SEPARATION`.
const RADIAL_SEPARATION: f64 = 50.0;
const COMPONENT_SEPARATION: f64 = 60.0;
const WORLD_CENTER_X: f64 = 1200.0;
const WORLD_CENTER_Y: f64 = 900.0;
const WORLD_BOUNDARY: f64 = 1e6;
/// `Integer.MAX_VALUE` / `MIN_VALUE`.
const INT_MAX: f64 = 2147483647.0;
const INT_MIN: f64 = -2147483648.0;

struct Node {
    rect: RectD,
    /// Incident edges in insertion order.
    edges: Vec<usize>,
    spring: (f64, f64),
    repulsion: (f64, f64),
    gravitation: (f64, f64),
    grid: (i64, i64, i64, i64),
    surrounding: Vec<usize>,
}

struct Edge {
    source: usize,
    target: usize,
}

/// Root graph bounds (`LGraph.left/right/top/bottom`).
#[derive(Default, Clone, Copy)]
struct Bounds {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

struct Cose {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    bounds: Bounds,
    estimated_size: f64,
    connected: bool,
    cooling_factor: f64,
    initial_cooling_factor: f64,
    final_temperature: f64,
    max_cooling_cycle: f64,
    cooling_cycle: usize,
    max_iterations: usize,
    total_iterations: usize,
    total_displacement: f64,
    old_total_displacement: f64,
    total_displacement_threshold: f64,
    repulsion_range: f64,
    grid: Vec<Vec<Vec<usize>>>,
}

/// Lay out a tree with CoSE-Bilkent. `sizes` are the node boxes (`w`, `h`) in
/// the order the nodes are added to cytoscape, `edges` `(source, target)` pairs
/// in theirs. Returns each node's centre in cytoscape's coordinates. Panics if
/// the graph is not a non-empty tree (`edges.len() + 1 == sizes.len()`, all
/// connected).
pub fn layout_tree(sizes: &[(f64, f64)], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = sizes.len();
    assert!(n > 0 && edges.len() + 1 == n, "CoSE port expects a tree");
    let mut c = Cose {
        nodes: sizes
            .iter()
            .map(|&(w, h)| Node {
                // Every node starts at (0, 0) (mermaid passes no position).
                rect: RectD {
                    x: 0.0 - w / 2.0,
                    y: 0.0 - h / 2.0,
                    w,
                    h,
                },
                edges: Vec::new(),
                spring: (0.0, 0.0),
                repulsion: (0.0, 0.0),
                gravitation: (0.0, 0.0),
                grid: (0, 0, 0, 0),
                surrounding: Vec::new(),
            })
            .collect(),
        edges: Vec::new(),
        bounds: Bounds::default(),
        estimated_size: 0.0,
        connected: false,
        cooling_factor: 0.0,
        initial_cooling_factor: 0.0,
        final_temperature: CONVERGENCE_CHECK_PERIOD as f64 / MAX_ITERATIONS as f64,
        max_cooling_cycle: MAX_ITERATIONS as f64 / CONVERGENCE_CHECK_PERIOD as f64,
        cooling_cycle: 0,
        max_iterations: MAX_ITERATIONS,
        total_iterations: 0,
        total_displacement: 0.0,
        old_total_displacement: 0.0,
        total_displacement_threshold: 0.0,
        repulsion_range: 0.0,
        grid: Vec::new(),
    };
    for &(s, t) in edges {
        // cose-bilkent skips self loops and parallel edges.
        if s == t || c.edge_between(s, t) {
            continue;
        }
        let e = c.edges.len();
        c.edges.push(Edge {
            source: s,
            target: t,
        });
        c.nodes[s].edges.push(e);
        c.nodes[t].edges.push(e);
    }
    c.run();
    c.nodes
        .iter()
        .map(|nd| (nd.rect.center_x(), nd.rect.center_y()))
        .collect()
}

impl Cose {
    fn edge_between(&self, a: usize, b: usize) -> bool {
        self.nodes[a].edges.iter().any(|&e| {
            let ed = &self.edges[e];
            ed.target == b || ed.source == b
        })
    }

    fn other_end(&self, e: usize, n: usize) -> usize {
        let ed = &self.edges[e];
        if ed.source == n { ed.target } else { ed.source }
    }

    /// `Layout.runLayout` → `CoSELayout.classicLayout` → `doPostLayout`.
    fn run(&mut self) {
        self.connected = self.update_connected();
        // calcEstimatedSize of the root graph (only used by gravity).
        let size: f64 = self
            .nodes
            .iter()
            .map(|nd| (nd.rect.w + nd.rect.h) / 2.0)
            .sum();
        self.estimated_size = if size == 0.0 {
            40.0
        } else {
            size / (self.nodes.len() as f64).sqrt()
        };
        let forest = self.flat_forest();
        self.position_nodes_radially(&forest);
        self.init_spring_embedder();
        while !self.tick() {}
        // doPostLayout: transform() moves the root graph's top left to (0, 0).
        self.transform(0.0, 0.0);
    }

    /// `LGraph.updateConnected` for the root graph.
    fn update_connected(&self) -> bool {
        let n = self.nodes.len();
        let mut visited = vec![false; n];
        let mut queue = std::collections::VecDeque::from([0]);
        visited[0] = true;
        let mut count = 1;
        while let Some(cur) = queue.pop_front() {
            for &e in &self.nodes[cur].edges {
                let o = self.other_end(e, cur);
                if !visited[o] {
                    visited[o] = true;
                    count += 1;
                    queue.push_back(o);
                }
            }
        }
        count == n
    }

    /// `Layout.getFlatForest`: each tree's nodes in breadth-first visiting order.
    fn flat_forest(&self) -> Vec<Vec<usize>> {
        let n = self.nodes.len();
        let mut forest = Vec::new();
        let mut unprocessed: Vec<usize> = (0..n).collect();
        while let Some(&start) = unprocessed.first() {
            let mut visited = Vec::new();
            let mut seen = vec![false; n];
            let mut parent: Vec<Option<usize>> = vec![None; n];
            let mut to_visit = std::collections::VecDeque::from([start]);
            while let Some(cur) = to_visit.pop_front() {
                if !seen[cur] {
                    seen[cur] = true;
                    visited.push(cur);
                }
                for &e in &self.nodes[cur].edges {
                    let nb = self.other_end(e, cur);
                    if parent[cur] != Some(nb) && !seen[nb] {
                        to_visit.push_back(nb);
                        parent[nb] = Some(cur);
                    }
                }
            }
            unprocessed.retain(|v| !seen[*v]);
            forest.push(visited);
        }
        forest
    }

    /// `Layout.findCenterOfTree`. Its loop splices the node it looks at out of
    /// the list while the index keeps counting, so each pass drops every other
    /// node; the degree bookkeeping never influences the result.
    fn center_of_tree(tree: &[usize]) -> usize {
        let mut list = tree.to_vec();
        while list.len() > 2 {
            let mut i = 0;
            while i < list.len() {
                list.remove(i);
                i += 1;
            }
        }
        list[0]
    }

    /// `CoSELayout.positionNodesRadially`.
    fn position_nodes_radially(&mut self, forest: &[Vec<usize>]) {
        let columns = (forest.len() as f64).sqrt().ceil() as usize;
        let mut height = 0.0_f64;
        let mut current_y = 0.0;
        let mut current_x = 0.0;
        let mut point = (0.0, 0.0);
        for (i, tree) in forest.iter().enumerate() {
            if i % columns == 0 {
                current_x = 0.0;
                current_y = height;
                if i != 0 {
                    current_y += COMPONENT_SEPARATION;
                }
                height = 0.0;
            }
            let center = Self::center_of_tree(tree);
            point = self.radial_layout(tree, center, (current_x, current_y));
            if point.1 > height {
                height = point.1.floor();
            }
            current_x = (point.0 + COMPONENT_SEPARATION).floor();
        }
        self.transform(
            WORLD_CENTER_X - point.0 / 2.0,
            WORLD_CENTER_Y - point.1 / 2.0,
        );
    }

    /// `CoSELayout.radialLayout`: returns the tree's bottom right after moving it
    /// to `start`.
    fn radial_layout(&mut self, tree: &[usize], center: usize, start: (f64, f64)) -> (f64, f64) {
        let mut max_diagonal = INT_MIN;
        for &v in tree {
            let r = self.nodes[v].rect;
            let d = (r.w * r.w + r.h * r.h).sqrt();
            if d > max_diagonal {
                max_diagonal = d;
            }
        }
        let sep = max_diagonal.max(RADIAL_SEPARATION);
        self.branch_radial_layout(center, None, 0.0, 359.0, 0.0, sep);
        let (mut left, mut right, mut top, mut bottom) = (INT_MAX, -INT_MAX, INT_MAX, -INT_MAX);
        for &v in tree {
            let r = self.nodes[v].rect;
            left = left.min(r.x);
            right = right.max(r.right());
            top = top.min(r.y);
            bottom = bottom.max(r.bottom());
        }
        // RectangleD(left, top, right - left, bottom - top): maxX = x + width.
        let (bw, bh) = (right - left, bottom - top);
        for &v in tree {
            self.transform_node(v, (left, top), start);
        }
        (start.0 + (left + bw - left), start.1 + (top + bh - top))
    }

    /// `CoSELayout.branchRadialLayout`.
    fn branch_radial_layout(
        &mut self,
        node: usize,
        parent: Option<usize>,
        start_angle: f64,
        end_angle: f64,
        distance: f64,
        sep: f64,
    ) {
        let mut half = (end_angle - start_angle + 1.0) / 2.0;
        if half < 0.0 {
            half += 180.0;
        }
        let node_angle = (half + start_angle) % 360.0;
        let teta = node_angle * (2.0 * std::f64::consts::PI) / 360.0;
        let x = distance * jsmath::cos(teta);
        let y = distance * jsmath::sin(teta);
        let r = &mut self.nodes[node].rect;
        r.x = x - r.w / 2.0;
        r.y = y - r.h / 2.0;
        let neighbor_edges = self.nodes[node].edges.clone();
        let inc = neighbor_edges.len();
        let child_count = inc - usize::from(parent.is_some());
        let mut start_index = 0;
        if let Some(p) = parent {
            // A tree has exactly one edge to the parent.
            let to_parent = neighbor_edges
                .iter()
                .position(|&e| self.other_end(e, node) == p)
                .unwrap_or(0);
            start_index = (to_parent + 1) % inc;
        }
        if child_count == 0 {
            return;
        }
        let step = (end_angle - start_angle).abs() / child_count as f64;
        let mut branch = 0;
        let mut i = start_index;
        while branch != child_count {
            let nb = self.other_end(neighbor_edges[i], node);
            if Some(nb) != parent {
                let child_start = (start_angle + branch as f64 * step) % 360.0;
                let child_end = (child_start + step) % 360.0;
                self.branch_radial_layout(
                    nb,
                    Some(node),
                    child_start,
                    child_end,
                    distance + sep,
                    sep,
                );
                branch += 1;
            }
            i = (i + 1) % inc;
        }
    }

    /// `LNode.transform` with a translation-only `Transform` mapping `device`
    /// to `world`.
    fn transform_node(&mut self, v: usize, device: (f64, f64), world: (f64, f64)) {
        let r = &mut self.nodes[v].rect;
        let left = r.x.clamp(-WORLD_BOUNDARY, WORLD_BOUNDARY);
        let top = r.y.clamp(-WORLD_BOUNDARY, WORLD_BOUNDARY);
        r.x = world.0 + (left - device.0) * 1.0 / 1.0;
        r.y = world.1 + (top - device.1) * 1.0 / 1.0;
    }

    /// `Layout.transform(newLeftTop)`: moves the root graph's top left (nodes'
    /// minimum less the margin) to the given point.
    fn transform(&mut self, x: f64, y: f64) {
        let (mut left, mut top) = (INT_MAX, INT_MAX);
        for nd in &self.nodes {
            top = top.min(nd.rect.y);
            left = left.min(nd.rect.x);
        }
        let device = (left - GRAPH_MARGIN, top - GRAPH_MARGIN);
        for v in 0..self.nodes.len() {
            self.transform_node(v, device, (x, y));
        }
    }

    /// `FDLayout.initSpringEmbedder` (non-incremental).
    fn init_spring_embedder(&mut self) {
        let n = self.nodes.len();
        self.cooling_factor = if n > 1000 {
            (1.0 - (n as f64 - 1e3) / (5e3 - 1e3) * (1.0 - 0.33)).max(0.33)
        } else {
            1.0
        };
        self.initial_cooling_factor = self.cooling_factor;
        self.max_iterations = (n * 5).max(MAX_ITERATIONS);
        self.total_displacement_threshold = 3.0 * IDEAL_EDGE_LENGTH / 100.0 * n as f64;
        // CoSELayout.calcRepulsionRange: 2 · (level + 1) · idealEdgeLength.
        self.repulsion_range = 2.0 * (0.0 + 1.0) * IDEAL_EDGE_LENGTH;
    }

    /// `FDLayout.isConverged`.
    fn is_converged(&mut self) -> bool {
        let mut oscillating = false;
        if self.total_iterations as f64 > self.max_iterations as f64 / 3.0 {
            oscillating = (self.total_displacement - self.old_total_displacement).abs() < 2.0;
        }
        let converged = self.total_displacement < self.total_displacement_threshold;
        self.old_total_displacement = self.total_displacement;
        converged || oscillating
    }

    /// `CoSELayout.tick`; `true` once the layout has finished.
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
            // quality 'proof': the cooling adjuster stays 1.
            let exponent =
                jsmath::ln(100.0 * (self.initial_cooling_factor - self.final_temperature))
                    / jsmath::ln(self.max_cooling_cycle);
            self.cooling_factor = (self.initial_cooling_factor
                - jsmath::pow(self.cooling_cycle as f64, exponent) / 100.0 * 1.0)
                .max(self.final_temperature);
        }
        self.total_displacement = 0.0;
        self.update_bounds();
        self.calc_spring_forces();
        self.calc_repulsion_forces();
        self.calc_gravitational_forces();
        self.move_nodes();
        false
    }

    /// `LGraph.updateBounds` of the root graph.
    fn update_bounds(&mut self) {
        let (mut left, mut right, mut top, mut bottom) = (INT_MAX, -INT_MAX, INT_MAX, -INT_MAX);
        for nd in &self.nodes {
            let r = nd.rect;
            left = left.min(r.x);
            right = right.max(r.right());
            top = top.min(r.y);
            bottom = bottom.max(r.bottom());
        }
        let (w, h) = (right - left, bottom - top);
        self.bounds = Bounds {
            left: left - GRAPH_MARGIN,
            right: left + w + GRAPH_MARGIN,
            top: top - GRAPH_MARGIN,
            bottom: top + h + GRAPH_MARGIN,
        };
    }

    fn calc_spring_forces(&mut self) {
        for e in 0..self.edges.len() {
            let (s, t) = (self.edges[e].source, self.edges[e].target);
            // LEdge.updateLength: clip points of target then source.
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
            let force = SPRING_CONSTANT * (length - IDEAL_EDGE_LENGTH);
            let fx = force * (lx / length);
            let fy = force * (ly / length);
            self.nodes[s].spring.0 += fx;
            self.nodes[s].spring.1 += fy;
            self.nodes[t].spring.0 -= fx;
            self.nodes[t].spring.1 -= fy;
        }
    }

    /// `FDLayout.calcRepulsionForces` with the FR grid variant.
    fn calc_repulsion_forces(&mut self) {
        let refresh = self.total_iterations % GRID_CALCULATION_CHECK_PERIOD == 1;
        if refresh {
            self.update_grid();
        }
        let n = self.nodes.len();
        let mut processed = vec![false; n];
        for a in 0..n {
            if refresh {
                self.find_surrounding(a, &processed);
            }
            for k in 0..self.nodes[a].surrounding.len() {
                let b = self.nodes[a].surrounding[k];
                self.calc_repulsion_force(a, b);
            }
            processed[a] = true;
        }
    }

    /// `FDLayout.updateGrid`.
    fn update_grid(&mut self) {
        let b = self.bounds;
        let range = self.repulsion_range;
        let size_x = ((b.right - b.left) / range).ceil() as usize;
        let size_y = ((b.bottom - b.top) / range).ceil() as usize;
        self.grid = vec![vec![Vec::new(); size_y]; size_x];
        for v in 0..self.nodes.len() {
            let r = self.nodes[v].rect;
            let sx = ((r.x - b.left) / range).floor() as i64;
            let fx = ((r.w + r.x - b.left) / range).floor() as i64;
            let sy = ((r.y - b.top) / range).floor() as i64;
            let fy = ((r.h + r.y - b.top) / range).floor() as i64;
            for i in sx..=fx {
                for j in sy..=fy {
                    if let Some(cell) = self
                        .grid
                        .get_mut(i as usize)
                        .and_then(|col| col.get_mut(j as usize))
                    {
                        cell.push(v);
                    }
                }
            }
            self.nodes[v].grid = (sx, fx, sy, fy);
        }
    }

    /// The neighbourhood part of `FDLayout.calculateRepulsionForceOfANode`.
    fn find_surrounding(&mut self, a: usize, processed: &[bool]) {
        let (sx, fx, sy, fy) = self.nodes[a].grid;
        let ra = self.nodes[a].rect;
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
                    if b == a || processed[b] || surrounding.contains(&b) {
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

    /// `FDLayout.calcRepulsionForce` (every node is a leaf: `noOfChildren` 1).
    fn calc_repulsion_force(&mut self, a: usize, b: usize) {
        let ra = self.nodes[a].rect;
        let rb = self.nodes[b].rect;
        let (fx, fy);
        if ra.intersects(&rb) {
            let o = separation_amount(&ra, &rb, IDEAL_EDGE_LENGTH / 2.0);
            let (rx, ry) = (2.0 * o[0], 2.0 * o[1]);
            let c = 1.0 * 1.0 / (1.0 + 1.0);
            self.nodes[a].repulsion.0 -= c * rx;
            self.nodes[a].repulsion.1 -= c * ry;
            self.nodes[b].repulsion.0 += c * rx;
            self.nodes[b].repulsion.1 += c * ry;
            return;
        }
        let (clip, _) = clip_points(&ra, &rb);
        let mut dx = clip[2] - clip[0];
        let mut dy = clip[3] - clip[1];
        if dx.abs() < MIN_REPULSION_DIST {
            dx = sign(dx) * MIN_REPULSION_DIST;
        }
        if dy.abs() < MIN_REPULSION_DIST {
            dy = sign(dy) * MIN_REPULSION_DIST;
        }
        let dist_sq = dx * dx + dy * dy;
        let dist = dist_sq.sqrt();
        let force = REPULSION_CONSTANT * 1.0 * 1.0 / dist_sq;
        fx = force * dx / dist;
        fy = force * dy / dist;
        self.nodes[a].repulsion.0 -= fx;
        self.nodes[a].repulsion.1 -= fy;
        self.nodes[b].repulsion.0 += fx;
        self.nodes[b].repulsion.1 += fy;
    }

    /// `FDLayout.calcGravitationalForces`: only a disconnected root graph pulls
    /// its nodes towards its centre.
    fn calc_gravitational_forces(&mut self) {
        if self.connected {
            return;
        }
        let b = self.bounds;
        let cx = (b.right + b.left) / 2.0;
        let cy = (b.top + b.bottom) / 2.0;
        let range = self.estimated_size * GRAVITY_RANGE_FACTOR;
        for nd in &mut self.nodes {
            let dx = nd.rect.center_x() - cx;
            let dy = nd.rect.center_y() - cy;
            let ax = dx.abs() + nd.rect.w / 2.0;
            let ay = dy.abs() + nd.rect.h / 2.0;
            if ax > range || ay > range {
                nd.gravitation = (-GRAVITY_CONSTANT * dx, -GRAVITY_CONSTANT * dy);
            }
        }
    }

    /// `CoSENode.move` for every node.
    fn move_nodes(&mut self) {
        let cf = self.cooling_factor;
        let limit = cf * MAX_NODE_DISPLACEMENT;
        for nd in &mut self.nodes {
            let mut dx = cf * (nd.spring.0 + nd.repulsion.0 + nd.gravitation.0) / 1.0;
            let mut dy = cf * (nd.spring.1 + nd.repulsion.1 + nd.gravitation.1) / 1.0;
            if dx.abs() > limit {
                dx = limit * sign(dx);
            }
            if dy.abs() > limit {
                dy = limit * sign(dy);
            }
            nd.rect.x += dx;
            nd.rect.y += dy;
            self.total_displacement += dx.abs() + dy.abs();
            nd.spring = (0.0, 0.0);
            nd.repulsion = (0.0, 0.0);
            nd.gravitation = (0.0, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_of_tree_drops_every_other_node() {
        assert_eq!(Cose::center_of_tree(&[7]), 7);
        assert_eq!(Cose::center_of_tree(&[4, 9]), 4);
        assert_eq!(Cose::center_of_tree(&[0, 1, 2, 3, 4]), 1);
        assert_eq!(Cose::center_of_tree(&[0, 1, 2, 3, 4, 5, 6, 7, 8]), 3);
    }

    #[test]
    fn matches_cose_bilkent_on_a_small_tree() {
        // Reference: cose-base 1.0.3 run as cytoscape-cose-bilkent does
        // (quality 'proof', randomize true) on a→b, a→c, c→d.
        let p = layout_tree(
            &[(80.0, 40.0), (60.0, 30.0), (60.0, 30.0), (50.0, 30.0)],
            &[(0, 1), (0, 2), (2, 3)],
        );
        let want = [
            (271.9889528872169, 35.0),
            (395.48342933082483, 35.0),
            (148.49447644360805, 35.0),
            (40.0, 35.0),
        ];
        for (got, want) in p.iter().zip(want) {
            assert!(
                (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
                "{got:?} vs {want:?}"
            );
        }
    }
}
