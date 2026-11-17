//! The node placement of mermaid 12.3's swimlane layout: `lanes.ts`,
//! `createEdgeLabelNodes`, `toGraphView`, the Sugiyama phases of `pipeline.ts`
//! (`phase0.helpers` .. `phase4.coordinates`) and `writeBackToLayoutData`.
//!
//! Anchored nodes (`metadata.anchorTo`) and pools (`metadata.laneRole`) never
//! occur in a `swimlane-beta` diagram: flowDb strips the internal keys and the
//! syntax has no way to anchor a node, so neither is modelled.

use super::model::{EPS, LNode, LayoutData, OMap, Origin, js_round, locale_cmp, num_cmp};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// lanes.ts
// ---------------------------------------------------------------------------

/// `buildLaneModel` for diagrams without pools: every top-level group is a lane.
pub struct LaneModel {
    lanes: HashSet<String>,
    parent: HashMap<String, Option<String>>,
}

impl LaneModel {
    pub fn new(nodes: &[LNode]) -> Self {
        let mut parent = HashMap::new();
        let mut lanes = HashSet::new();
        for n in nodes {
            parent.insert(n.id.clone(), n.parent_id.clone());
            if n.is_group && n.parent_id.is_none() {
                lanes.insert(n.id.clone());
            }
        }
        LaneModel { lanes, parent }
    }

    pub fn is_lane(&self, id: &str) -> bool {
        self.lanes.contains(id)
    }

    /// `laneIdOf`: the lane above `id` (not `id` itself), if any.
    pub fn lane_id_of(&self, id: &str) -> Option<String> {
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(id.to_string());
        let mut current = self.parent.get(id).cloned().flatten();
        while let Some(c) = current {
            if seen.contains(&c) {
                break;
            }
            if self.lanes.contains(&c) {
                return Some(c);
            }
            seen.insert(c.clone());
            current = self.parent.get(&c).cloned().flatten();
        }
        None
    }
}

// ---------------------------------------------------------------------------
// createEdgeLabelNodes
// ---------------------------------------------------------------------------

/// Adds a label node and two layout-only edges for every labelled edge. `has_label`
/// tells which edges carry a label; label nodes get `label_size` of their edge.
pub fn create_edge_label_nodes(data: &mut LayoutData, label_size: &dyn Fn(usize) -> (f64, f64)) {
    let mut add = Vec::new();
    let mut layout_only = Vec::new();
    let parent_of: HashMap<String, Option<String>> = data
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.parent_id.clone()))
        .collect();
    for e in data.edges.iter_mut() {
        let Some(ei) = e.origin else { continue };
        if e.is_layout_only || e.label_node_id.is_some() {
            continue;
        }
        let (w, h) = label_size(ei);
        if w < 0.0 {
            continue; // no label
        }
        let (Some(sp), Some(tp)) = (parent_of.get(&e.start), parent_of.get(&e.end)) else {
            continue;
        };
        let id = format!("edge-label-{}-{}-{}", e.start, e.end, e.id);
        let lane = if sp != tp { tp.clone() } else { sp.clone() };
        let mut n = LNode::new(id.clone(), Origin::EdgeLabel(ei));
        n.shape = "labelRect".into();
        n.width = Some(w);
        n.height = Some(h);
        n.is_edge_label = true;
        n.is_dummy = true;
        n.parent_id = lane;
        n.edge_start = Some(e.start.clone());
        n.edge_end = Some(e.end.clone());
        add.push(n);
        e.label_node_id = Some(id.clone());
        for (suffix, s, t) in [
            ("-to-label", e.start.clone(), id.clone()),
            ("-from-label", id.clone(), e.end.clone()),
        ] {
            layout_only.push(super::model::LEdge {
                id: format!("{}{}", e.id, suffix),
                origin: None,
                start: s,
                end: t,
                points: None,
                label_node_id: None,
                is_layout_only: true,
                arrow_type_start: None,
                arrow_type_end: None,
            });
        }
    }
    data.nodes.extend(add);
    data.edges.extend(layout_only);
}

// ---------------------------------------------------------------------------
// Graph view
// ---------------------------------------------------------------------------

/// What the phases read of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct GNode {
    pub is_group: bool,
    pub is_dummy: bool,
    pub width: f64,
    pub height: f64,
}

/// `EdgeRef`: an edge of the graph view and the layout edge it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRef {
    pub id: String,
    pub src: String,
    pub dst: String,
    /// The layout edge's id, start and end (`ref`).
    pub ref_id: String,
    pub ref_start: String,
    pub ref_end: String,
}

#[derive(Debug, Clone)]
pub struct Graph {
    pub nodes: Vec<String>,
    pub edges: Vec<EdgeRef>,
    pub node_by_id: OMap<GNode>,
}

/// `toGraphView` (already through `normalizeGraph`, which keeps `nodeById`'s order).
pub fn to_graph_view(layout: &LayoutData) -> Graph {
    let mut node_by_id = OMap::new();
    for n in layout.nodes.iter().filter(|n| !n.is_edge_label) {
        node_by_id.set(
            n.id.clone(),
            GNode {
                is_group: n.is_group,
                is_dummy: n.is_dummy,
                width: n.width.unwrap_or(0.0),
                height: n.height.unwrap_or(0.0),
            },
        );
    }
    let mut edges = Vec::new();
    for e in &layout.edges {
        if e.is_layout_only || e.start == e.end {
            continue;
        }
        edges.push(EdgeRef {
            id: e.id.clone(),
            src: e.start.clone(),
            dst: e.end.clone(),
            ref_id: e.id.clone(),
            ref_start: e.start.clone(),
            ref_end: e.end.clone(),
        });
    }
    normalize_graph(&Graph {
        nodes: node_by_id.keys().cloned().collect(),
        edges,
        node_by_id,
    })
}

/// `normalizeGraph`: drops edges to unknown nodes and duplicates.
pub fn normalize_graph(g: &Graph) -> Graph {
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for e in &g.edges {
        if !g.node_by_id.has(&e.src) || !g.node_by_id.has(&e.dst) {
            continue;
        }
        let key = format!("{}:{}->{}", e.id, e.src, e.dst);
        if !seen.insert(key) {
            continue;
        }
        edges.push(e.clone());
    }
    Graph {
        nodes: g.node_by_id.keys().cloned().collect(),
        edges,
        node_by_id: g.node_by_id.clone(),
    }
}

fn in_degree(g: &Graph) -> OMap<usize> {
    let mut indeg = OMap::new();
    for v in &g.nodes {
        indeg.set(v.clone(), 0usize);
    }
    for e in &g.edges {
        indeg.add(&e.dst, 1);
    }
    indeg
}

fn sorted_successors(g: &Graph) -> HashMap<String, Vec<String>> {
    let mut succ: HashMap<String, Vec<String>> =
        g.nodes.iter().map(|v| (v.clone(), Vec::new())).collect();
    for e in &g.edges {
        succ.get_mut(&e.src).unwrap().push(e.dst.clone());
    }
    for s in succ.values_mut() {
        s.sort_by(|a, b| locale_cmp(a, b));
    }
    succ
}

/// `sortedZeroInDegreeNodes`.
fn sorted_zero_in_degree(g: &Graph, indeg: &OMap<usize>) -> Vec<String> {
    let mut sorted: Vec<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| id.clone())
        .collect();
    sorted.sort_by(|a, b| locale_cmp(a, b));
    let linked: HashSet<&str> = g
        .edges
        .iter()
        .flat_map(|e| [e.src.as_str(), e.dst.as_str()])
        .collect();
    let unconnected_id =
        |id: &str| !linked.contains(id) && !g.node_by_id.get(id).is_some_and(|n| n.is_group);
    let unconnected: Vec<String> = indeg
        .iter()
        .filter(|(id, d)| **d == 0 && unconnected_id(id))
        .map(|(id, _)| id.clone())
        .collect();
    let mut next = 0;
    sorted
        .into_iter()
        .map(|id| {
            if unconnected_id(&id) {
                next += 1;
                unconnected[next - 1].clone()
            } else {
                id
            }
        })
        .collect()
}

/// `topoSortIfAcyclic` (Kahn, queue kept in code-unit order).
fn topo_sort(g: &Graph) -> Option<Vec<String>> {
    let mut indeg = in_degree(g);
    let mut queue: std::collections::VecDeque<String> = sorted_zero_in_degree(g, &indeg).into();
    let adj = sorted_successors(g);
    let mut order = Vec::new();
    while let Some(u) = queue.pop_front() {
        for v in &adj[&u] {
            let d = indeg.get(v).copied().unwrap_or(0) - 1;
            indeg.set(v.clone(), d);
            if d == 0 {
                let i = queue.iter().position(|q| q >= v).unwrap_or(queue.len());
                queue.insert(i, v.clone());
            }
        }
        order.push(u);
    }
    (order.len() == g.nodes.len()).then_some(order)
}

/// `topoSortByGenerationIfAcyclic`.
fn topo_sort_by_generation(g: &Graph) -> Option<Vec<String>> {
    let mut indeg = in_degree(g);
    let adj = sorted_successors(g);
    let mut frontier = sorted_zero_in_degree(g, &indeg);
    let mut order = Vec::new();
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for u in &frontier {
            order.push(u.clone());
            for v in &adj[u] {
                let d = indeg.get(v).copied().unwrap_or(0) - 1;
                indeg.set(v.clone(), d);
                if d == 0 {
                    next.push(v.clone());
                }
            }
        }
        next.sort_by(|a, b| locale_cmp(a, b));
        frontier = next;
    }
    (order.len() == g.nodes.len()).then_some(order)
}

/// `removeCycles_DFS`: the graph with its back edges reversed.
pub fn remove_cycles(g: &Graph) -> Graph {
    let gn = normalize_graph(g);
    let mut adj: HashMap<String, Vec<usize>> =
        gn.nodes.iter().map(|v| (v.clone(), Vec::new())).collect();
    for (i, e) in gn.edges.iter().enumerate() {
        adj.get_mut(&e.src).unwrap().push(i);
    }
    for list in adj.values_mut() {
        list.sort_by(|&a, &b| {
            let (ea, eb) = (&gn.edges[a], &gn.edges[b]);
            if ea.dst == eb.dst {
                locale_cmp(&ea.id, &eb.id)
            } else {
                locale_cmp(&ea.dst, &eb.dst)
            }
        });
    }
    let mut color: HashMap<String, u8> = gn.nodes.iter().map(|v| (v.clone(), 0)).collect();
    let mut reversed: HashSet<String> = HashSet::new();
    fn dfs(
        u: &str,
        gn: &Graph,
        adj: &HashMap<String, Vec<usize>>,
        color: &mut HashMap<String, u8>,
        reversed: &mut HashSet<String>,
    ) {
        color.insert(u.to_string(), 1);
        for &ei in &adj[u] {
            let e = &gn.edges[ei];
            match color[&e.dst] {
                0 => dfs(&e.dst, gn, adj, color, reversed),
                1 => {
                    reversed.insert(format!("{}:{}->{}", e.id, e.src, e.dst));
                }
                _ => {}
            }
        }
        color.insert(u.to_string(), 2);
    }
    let indeg = in_degree(&gn);
    let seeds: Vec<String> = gn
        .nodes
        .iter()
        .filter(|v| indeg.get(v) == Some(&0))
        .chain(gn.nodes.iter().filter(|v| indeg.get(v) != Some(&0)))
        .cloned()
        .collect();
    for v in &seeds {
        if color[v] == 0 {
            dfs(v, &gn, &adj, &mut color, &mut reversed);
        }
    }
    let edges = gn
        .edges
        .iter()
        .map(|e| {
            if reversed.contains(&format!("{}:{}->{}", e.id, e.src, e.dst)) {
                EdgeRef {
                    src: e.dst.clone(),
                    dst: e.src.clone(),
                    ..e.clone()
                }
            } else {
                e.clone()
            }
        })
        .collect();
    Graph {
        nodes: gn.nodes.clone(),
        edges,
        node_by_id: gn.node_by_id.clone(),
    }
}

/// `Layering`.
pub struct Layering {
    pub layers: Vec<Vec<String>>,
    pub rank_of: HashMap<String, usize>,
}

/// `assignLayers_LaneAwareCompact`.
pub fn assign_layers_lane_aware(
    g_acyclic: &Graph,
    lanes: &LaneModel,
    ignore_cross_lane: bool,
    direction: &str,
) -> Layering {
    let g = normalize_graph(g_acyclic);
    let order = if direction == "LR" {
        topo_sort_by_generation(&g)
    } else {
        topo_sort(&g)
    }
    .unwrap_or_else(|| {
        let mut all = g.nodes.clone();
        all.sort();
        all
    });
    let lane_of = |id: &str| lanes.lane_id_of(id).unwrap_or_else(|| id.to_string());
    let mut rank_of: HashMap<String, usize> = HashMap::new();
    let mut next_free: HashMap<String, usize> = HashMap::new();
    for v in &order {
        if g.node_by_id.get(v).is_some_and(|n| n.is_group) {
            continue;
        }
        let mut base = 0;
        for e in g.edges.iter().filter(|e| &e.dst == v) {
            let ru = rank_of.get(&e.src).copied().unwrap_or(0);
            let w = if !ignore_cross_lane || lane_of(&e.src) == lane_of(v) {
                1
            } else {
                0
            };
            base = base.max(ru + w);
        }
        let lane = lane_of(v);
        let nf = next_free.get(&lane).copied().unwrap_or(0);
        let l = base.max(nf);
        rank_of.insert(v.clone(), l);
        next_free.insert(lane, l + 1);
    }
    // `buildLayersFromRanks` with `skipGroups`.
    let is_group = |v: &str| g.node_by_id.get(v).is_some_and(|n| n.is_group);
    let max_rank = g
        .nodes
        .iter()
        .filter(|v| !is_group(v))
        .map(|v| rank_of.get(v).copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let mut layers = vec![Vec::new(); max_rank + 1];
    for v in &order {
        if is_group(v) {
            continue;
        }
        layers[rank_of.get(v).copied().unwrap_or(0)].push(v.clone());
    }
    Layering { layers, rank_of }
}

/// `makeProperLayering`: dummy nodes on the ranks a long edge passes.
pub fn make_proper_layering(layering: &Layering, g_acyclic: &Graph) -> (Layering, Graph) {
    let g = normalize_graph(g_acyclic);
    let mut rank_of = layering.rank_of.clone();
    let mut layers = layering.layers.clone();
    let mut node_by_id = g.node_by_id.clone();
    let mut dummies: Vec<String> = Vec::new();
    let mut edges_sorted = g.edges.clone();
    edges_sorted.sort_by(|a, b| {
        if a.id == b.id {
            if a.src == b.src {
                locale_cmp(&a.dst, &b.dst)
            } else {
                locale_cmp(&a.src, &b.src)
            }
        } else {
            locale_cmp(&a.id, &b.id)
        }
    });
    let mut new_edges = Vec::new();
    for e in &edges_sorted {
        let ru = rank_of.get(&e.src).copied().unwrap_or(0) as i64;
        let rv = rank_of.get(&e.dst).copied().unwrap_or(0) as i64;
        if rv - ru <= 1 {
            new_edges.push(e.clone());
            continue;
        }
        let mut prev = e.src.clone();
        for (k, l) in ((ru + 1)..rv).enumerate() {
            let id = format!("placeholder-{}", dummies.len());
            node_by_id.set(
                id.clone(),
                GNode {
                    is_group: false,
                    is_dummy: true,
                    width: 0.0,
                    height: 0.0,
                },
            );
            dummies.push(id.clone());
            while layers.len() <= l as usize {
                layers.push(Vec::new());
            }
            layers[l as usize].push(id.clone());
            rank_of.insert(id.clone(), l as usize);
            new_edges.push(EdgeRef {
                id: format!("{}#{}", e.id, k),
                src: prev.clone(),
                dst: id.clone(),
                ..e.clone()
            });
            prev = id;
        }
        let last_index = rv - ru - 2;
        new_edges.push(EdgeRef {
            id: format!("{}#{}", e.id, (last_index + 1).max(0)),
            src: prev,
            dst: e.dst.clone(),
            ..e.clone()
        });
    }
    let mut nodes = g.nodes.clone();
    nodes.extend(dummies.iter().filter(|d| !g.nodes.contains(d)).cloned());
    (
        Layering { layers, rank_of },
        Graph {
            nodes,
            edges: new_edges,
            node_by_id,
        },
    )
}

// ---------------------------------------------------------------------------
// phase3.ordering
// ---------------------------------------------------------------------------

/// The lane each graph node sits in (`createTopLaneResolver`).
fn top_lane_resolver(g: &Graph, lanes: &LaneModel) -> HashMap<String, Option<String>> {
    g.nodes
        .iter()
        .map(|id| (id.clone(), lanes.lane_id_of(id)))
        .collect()
}

/// `buildTopLaneOrder` without pools or lane indices: the lanes, last declared first.
pub fn top_lane_order(layout: &LayoutData, lanes: &LaneModel) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = layout
        .nodes
        .iter()
        .filter(|n| lanes.is_lane(&n.id))
        .filter(|n| seen.insert(n.id.clone()))
        .map(|n| n.id.clone())
        .collect();
    out.reverse();
    out
}

fn median(values: &[f64]) -> f64 {
    let n = values.len();
    if n == 0 {
        return f64::INFINITY;
    }
    let mut a = values.to_vec();
    a.sort_by(|x, y| num_cmp(*x, *y));
    if n % 2 == 1 {
        a[(n - 1) / 2]
    } else {
        0.5 * (a[n / 2 - 1] + a[n / 2])
    }
}

fn barycenter(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::INFINITY;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn layer_index(layer: &[String]) -> HashMap<&str, usize> {
    layer
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect()
}

/// `countInversions` (merge sort).
fn count_inversions(values: &mut [usize]) -> usize {
    let n = values.len();
    if n <= 1 {
        return 0;
    }
    let mid = n / 2;
    let mut inv = count_inversions(&mut values[..mid]) + count_inversions(&mut values[mid..]);
    let mut tmp = Vec::with_capacity(n);
    let (mut i, mut j) = (0, mid);
    while i < mid || j < n {
        if j >= n || (i < mid && values[i] <= values[j]) {
            tmp.push(values[i]);
            i += 1;
        } else {
            tmp.push(values[j]);
            j += 1;
            inv += mid - i;
        }
    }
    values.copy_from_slice(&tmp);
    inv
}

fn count_crossings_between(upper: &[String], lower: &[String], edges: &[&EdgeRef]) -> usize {
    let ui = layer_index(upper);
    let li = layer_index(lower);
    let mut pairs: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|e| Some((*ui.get(e.src.as_str())?, *li.get(e.dst.as_str())?)))
        .collect();
    pairs.sort();
    let mut vs: Vec<usize> = pairs.iter().map(|p| p.1).collect();
    count_inversions(&mut vs)
}

fn sort_by_heuristic(
    nodes: &[String],
    neighbors: &HashMap<String, Vec<f64>>,
    current: &HashMap<&str, usize>,
) -> Vec<String> {
    let mut out = nodes.to_vec();
    out.sort_by(|a, b| {
        let sa = median(neighbors.get(a).map(Vec::as_slice).unwrap_or(&[]));
        let sb = median(neighbors.get(b).map(Vec::as_slice).unwrap_or(&[]));
        if sa == sb {
            let ia = current.get(a.as_str()).copied().unwrap_or(0);
            let ib = current.get(b.as_str()).copied().unwrap_or(0);
            return if ia != ib {
                ia.cmp(&ib)
            } else {
                locale_cmp(a, b)
            };
        }
        if !sa.is_finite() {
            return std::cmp::Ordering::Greater;
        }
        if !sb.is_finite() {
            return std::cmp::Ordering::Less;
        }
        num_cmp(sa, sb)
    });
    out
}

fn reorder_layer(
    fixed: &[String],
    target: &[String],
    edges: &[EdgeRef],
    down: bool,
    top_lane: &HashMap<String, Option<String>>,
    lane_order: &[String],
) -> Vec<String> {
    let fixed_index = layer_index(fixed);
    let curr = layer_index(target);
    let mut neighbors: HashMap<String, Vec<f64>> =
        target.iter().map(|v| (v.clone(), Vec::new())).collect();
    for e in edges {
        if down {
            if let (Some(&i), true) = (
                fixed_index.get(e.src.as_str()),
                neighbors.contains_key(&e.dst),
            ) {
                neighbors.get_mut(&e.dst).unwrap().push(i as f64);
            }
        } else if let (Some(&i), true) = (
            fixed_index.get(e.dst.as_str()),
            neighbors.contains_key(&e.src),
        ) {
            neighbors.get_mut(&e.src).unwrap().push(i as f64);
        }
    }
    if lane_order.is_empty() {
        return sort_by_heuristic(target, &neighbors, &curr);
    }
    let lane = |id: &str| top_lane.get(id).cloned().flatten();
    let mut by_lane: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for id in target {
        let l = lane(id);
        match by_lane.iter_mut().find(|(k, _)| *k == l) {
            Some((_, v)) => v.push(id.clone()),
            None => by_lane.push((l, vec![id.clone()])),
        }
    }
    let mut result: Vec<String> = Vec::new();
    for l in lane_order {
        if let Some((_, nodes)) = by_lane.iter().find(|(k, _)| k.as_deref() == Some(l)) {
            if !nodes.is_empty() {
                result.extend(sort_by_heuristic(nodes, &neighbors, &curr));
            }
        }
    }
    if let Some((_, nulls)) = by_lane.iter().find(|(k, _)| k.is_none()) {
        let sorted = sort_by_heuristic(nulls, &neighbors, &curr);
        for nid in sorted {
            let bc = barycenter(&neighbors[&nid]);
            let mut best = result.len();
            if bc.is_finite() {
                for (i, rid) in result.iter().enumerate() {
                    let r = barycenter(neighbors.get(rid).map(Vec::as_slice).unwrap_or(&[]));
                    if bc < r {
                        best = i;
                        break;
                    }
                }
            }
            result.insert(best, nid);
        }
    }
    result
}

fn transpose_improve(
    upper: &[String],
    current: &[String],
    edges: &[EdgeRef],
    next: Option<&[String]>,
    top_lane: &HashMap<String, Option<String>>,
) -> Vec<String> {
    let mut best = current.to_vec();
    let upper_set: HashSet<&str> = upper.iter().map(String::as_str).collect();
    let layer_set: HashSet<&str> = current.iter().map(String::as_str).collect();
    let edges_in: Vec<&EdgeRef> = edges
        .iter()
        .filter(|e| upper_set.contains(e.src.as_str()) && layer_set.contains(e.dst.as_str()))
        .collect();
    let edges_out: Option<Vec<&EdgeRef>> = next.map(|n| {
        let next_set: HashSet<&str> = n.iter().map(String::as_str).collect();
        edges
            .iter()
            .filter(|e| layer_set.contains(e.src.as_str()) && next_set.contains(e.dst.as_str()))
            .collect()
    });
    let score = |order: &[String]| {
        let mut s = count_crossings_between(upper, order, &edges_in);
        if let (Some(out), Some(n)) = (&edges_out, next) {
            s += count_crossings_between(order, n, out);
        }
        s
    };
    let lane = |id: &str| top_lane.get(id).cloned().flatten();
    let mut improved = true;
    let mut best_score = score(&best);
    while improved {
        improved = false;
        let mut i = 0;
        while i + 1 < best.len() {
            if lane(&best[i]) != lane(&best[i + 1]) {
                i += 1;
                continue;
            }
            let prev = best_score;
            best.swap(i, i + 1);
            let ns = score(&best);
            if ns < prev {
                best_score = ns;
                improved = true;
            } else {
                best.swap(i, i + 1);
            }
            i += 1;
        }
    }
    best
}

/// `orderLayers`: three down/up median sweeps, each followed by transposes.
pub fn order_layers(
    layering: &Layering,
    g: &Graph,
    lanes: &LaneModel,
    lane_order: &[String],
) -> Vec<Vec<String>> {
    let mut layers = layering.layers.clone();
    let top_lane = top_lane_resolver(g, lanes);
    for _ in 0..3 {
        for i in 1..layers.len() {
            layers[i] = reorder_layer(
                &layers[i - 1],
                &layers[i],
                &g.edges,
                true,
                &top_lane,
                lane_order,
            );
            let next = layers.get(i + 1).cloned();
            layers[i] = transpose_improve(
                &layers[i - 1],
                &layers[i],
                &g.edges,
                next.as_deref(),
                &top_lane,
            );
        }
        for i in (0..layers.len().saturating_sub(1)).rev() {
            layers[i] = reorder_layer(
                &layers[i + 1],
                &layers[i],
                &g.edges,
                false,
                &top_lane,
                lane_order,
            );
            let prev = if i > 0 {
                Some(layers[i - 1].clone())
            } else {
                None
            };
            layers[i] = transpose_improve(
                &layers[i + 1],
                &layers[i],
                &g.edges,
                prev.as_deref(),
                &top_lane,
            );
        }
    }
    layers
}

// ---------------------------------------------------------------------------
// phase4.coordinates
// ---------------------------------------------------------------------------

/// `COORDINATES.EDGE_LABEL_CLEARANCE`.
const EDGE_LABEL_CLEARANCE: f64 = 16.0;

/// `flowAxisStretch`: the nodes' average width over their average height, at least 1.
pub fn flow_axis_stretch<'a>(nodes: impl Iterator<Item = &'a LNode>) -> f64 {
    let scaled: Vec<&LNode> = nodes.filter(|n| !n.is_group && !n.is_edge_label).collect();
    if scaled.is_empty() {
        return 1.0;
    }
    let tw: f64 = scaled.iter().map(|n| n.width.unwrap_or(0.0)).sum();
    let th: f64 = scaled.iter().map(|n| n.height.unwrap_or(0.0)).sum();
    let aw = tw / scaled.len() as f64;
    let ah = th / scaled.len() as f64;
    if ah > 0.0 { (aw / ah).max(1.0) } else { 1.0 }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Before,
    After,
}

struct RowLabel {
    label: String,
    rank: usize,
    lane: Option<String>,
    side: Side,
}

struct GapLabel {
    label: String,
    start: String,
    end: String,
    gap: usize,
    crosses_lanes: bool,
}

fn lane_key(l: &Option<String>) -> String {
    l.clone().unwrap_or_else(|| "null".into())
}

/// `assignCoordinates`: centres of the graph's nodes and seeds of the edge labels.
#[allow(clippy::too_many_arguments)]
pub fn assign_coordinates(
    layers: &[Vec<String>],
    g: &Graph,
    layout: &LayoutData,
    lanes: &LaneModel,
    lane_order_global: &[String],
    layer_gap: f64,
    node_gap: f64,
    direction: &str,
) -> (HashMap<String, f64>, HashMap<String, f64>) {
    let lane_gap = node_gap * 2.0;
    let horizontal = direction == "LR" || direction == "RL";
    let mut x: HashMap<String, f64> = HashMap::new();
    let mut y: HashMap<String, f64> = HashMap::new();
    let label_by_id: HashMap<&str, &LNode> = layout
        .nodes
        .iter()
        .filter(|n| n.is_edge_label)
        .map(|n| (n.id.as_str(), n))
        .collect();
    let width = |id: &str| -> f64 {
        if let Some(n) = label_by_id.get(id) {
            return n.width.unwrap_or(0.0);
        }
        g.node_by_id.get(id).map(|n| n.width).unwrap_or(0.0)
    };
    let height = |id: &str| -> f64 {
        if let Some(n) = label_by_id.get(id) {
            return n.height.unwrap_or(0.0);
        }
        g.node_by_id.get(id).map(|n| n.height).unwrap_or(0.0)
    };
    let top_lane_node = top_lane_resolver(g, lanes);
    let top_lane_of_node = |id: &str| top_lane_node.get(id).cloned().flatten();
    let mut label_lane: HashMap<String, Option<String>> = HashMap::new();

    let mut used: Vec<Option<String>> = Vec::new();
    for layer in layers {
        for id in layer {
            let l = top_lane_of_node(id);
            if !used.contains(&l) {
                used.push(l);
            }
        }
    }
    let has_null = used.contains(&None);
    let lanes_used: Vec<String> = lane_order_global
        .iter()
        .filter(|l| used.contains(&Some((*l).clone())))
        .cloned()
        .collect();
    let mut columns: Vec<Option<String>> = Vec::new();
    if has_null {
        columns.push(None);
    }
    columns.extend(lanes_used.iter().cloned().map(Some));

    let mut rank_of: HashMap<&str, usize> = HashMap::new();
    for (li, layer) in layers.iter().enumerate() {
        for id in layer {
            rank_of.insert(id.as_str(), li);
        }
    }
    let mut row_labels: Vec<RowLabel> = Vec::new();
    let mut gap_labels: Vec<GapLabel> = Vec::new();
    let mut unranked: Vec<(String, String)> = Vec::new();
    for e in &layout.edges {
        let Some(label) = &e.label_node_id else {
            continue;
        };
        if !label_by_id.contains_key(label.as_str()) {
            continue;
        }
        let (rs, rt) = (rank_of.get(e.start.as_str()), rank_of.get(e.end.as_str()));
        let (Some(&rs), Some(&rt)) = (rs, rt) else {
            let near = if rs.is_none() { &e.end } else { &e.start };
            unranked.push((label.clone(), near.clone()));
            continue;
        };
        label_lane.insert(label.clone(), top_lane_of_node(&e.end));
        if rs == rt {
            let column = |id: &str| {
                let l = top_lane_of_node(id);
                columns
                    .iter()
                    .position(|c| *c == l)
                    .map(|p| p as i64)
                    .unwrap_or(-1)
            };
            row_labels.push(RowLabel {
                label: label.clone(),
                rank: rt,
                lane: top_lane_of_node(&e.end),
                side: if e.start != e.end && column(&e.start) < column(&e.end) {
                    Side::Before
                } else {
                    Side::After
                },
            });
            continue;
        }
        let lo = rs.min(rt);
        let hi = rs.max(rt);
        gap_labels.push(GapLabel {
            label: label.clone(),
            start: e.start.clone(),
            end: e.end.clone(),
            gap: lo + (hi - lo - 1) / 2,
            crosses_lanes: top_lane_of_node(&e.start) != top_lane_of_node(&e.end),
        });
    }
    let top_lane_of = |id: &str| -> Option<String> {
        match label_lane.get(id) {
            Some(l) => l.clone(),
            None => top_lane_of_node(id),
        }
    };

    let mut layer_heights: Vec<f64> = layers
        .iter()
        .map(|l| l.iter().fold(0.0, |m: f64, v| m.max(height(v))))
        .collect();
    for r in &row_labels {
        layer_heights[r.rank] = layer_heights[r.rank].max(height(&r.label));
    }
    let mut extra: Vec<f64> = vec![0.0; layers.len()];
    if horizontal {
        for i in 0..layers.len().saturating_sub(1) {
            let this_w = layers[i].iter().fold(0.0, |m: f64, v| m.max(width(v)));
            let next_w = layers[i + 1].iter().fold(0.0, |m: f64, v| m.max(width(v)));
            let normal = layer_heights[i] / 2.0 + layer_heights[i + 1] / 2.0;
            let required = (this_w + next_w) / 2.0;
            extra[i] = (required - normal - layer_gap).max(0.0);
        }
    }
    let stretch = if horizontal {
        flow_axis_stretch(layout.nodes.iter())
    } else {
        1.0
    };
    let mut need: Vec<f64> = vec![0.0; layers.len()];
    for gl in &gap_labels {
        let extent = if horizontal {
            width(&gl.label)
        } else {
            height(&gl.label)
        };
        let n = (extent + 2.0 * EDGE_LABEL_CLEARANCE) / stretch;
        need[gl.gap] = need[gl.gap].max(n);
    }
    for (gap, n) in need.iter().enumerate() {
        extra[gap] += n;
    }

    let mut lane_width: HashMap<String, f64> = HashMap::new();
    for l in &lanes_used {
        lane_width.insert(l.clone(), 0.0);
    }
    if has_null {
        lane_width.insert("null".into(), 0.0);
    }
    for layer in layers {
        let mut per_lane: Vec<(String, Vec<&String>)> = Vec::new();
        let mut null_ids: Vec<&String> = Vec::new();
        for id in layer {
            match top_lane_of(id) {
                None => null_ids.push(id),
                Some(l) => match per_lane.iter_mut().find(|(k, _)| *k == l) {
                    Some((_, v)) => v.push(id),
                    None => per_lane.push((l, vec![id])),
                },
            }
        }
        for (l, ids) in &per_lane {
            let total = ids.iter().map(|id| width(id)).sum::<f64>()
                + node_gap * (ids.len().saturating_sub(1)) as f64;
            let cur = lane_width.get(l).copied().unwrap_or(0.0);
            lane_width.insert(l.clone(), cur.max(total));
        }
        if has_null && !null_ids.is_empty() {
            let total = null_ids.iter().map(|id| width(id)).sum::<f64>()
                + node_gap * (null_ids.len().saturating_sub(1)) as f64;
            let cur = lane_width.get("null").copied().unwrap_or(0.0);
            lane_width.insert("null".into(), cur.max(total));
        }
    }
    // Labels in one gap and lane, side by side.
    let mut gap_groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, gl) in gap_labels.iter().enumerate() {
        let key = format!("{}:{}", gl.gap, lane_key(&top_lane_of(&gl.label)));
        match gap_groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(i),
            None => gap_groups.push((key, vec![i])),
        }
    }
    for (_, group) in &gap_groups {
        if let Some(l) = top_lane_of(&gap_labels[group[0]].label) {
            let span: f64 = group
                .iter()
                .enumerate()
                .map(|(i, &gi)| width(&gap_labels[gi].label) + if i > 0 { node_gap } else { 0.0 })
                .sum();
            let cur = lane_width.get(&l).copied().unwrap_or(0.0);
            lane_width.insert(l, cur.max(span));
        }
    }
    let mut row_spans: HashMap<String, f64> = HashMap::new();
    let row_key = |rank: usize, lane: &Option<String>, side: Side| {
        format!(
            "{}:{}:{}",
            rank,
            lane_key(lane),
            if side == Side::Before {
                "before"
            } else {
                "after"
            }
        )
    };
    for r in &row_labels {
        let k = row_key(r.rank, &r.lane, r.side);
        let cur = row_spans.get(&k).copied().unwrap_or(0.0);
        row_spans.insert(k, cur + node_gap + width(&r.label));
    }
    for r in &row_labels {
        let nodes_width: f64 = layers[r.rank]
            .iter()
            .filter(|id| top_lane_of(id) == r.lane)
            .enumerate()
            .map(|(i, id)| width(id) + if i > 0 { node_gap } else { 0.0 })
            .sum();
        let side = row_spans
            .get(&row_key(r.rank, &r.lane, Side::Before))
            .copied()
            .unwrap_or(0.0)
            .max(
                row_spans
                    .get(&row_key(r.rank, &r.lane, Side::After))
                    .copied()
                    .unwrap_or(0.0),
            );
        let key = lane_key(&r.lane);
        let cur = lane_width.get(&key).copied().unwrap_or(0.0);
        lane_width.insert(key, cur.max(nodes_width + 2.0 * side));
    }

    let mut center_x: HashMap<Option<String>, f64> = HashMap::new();
    {
        let widths: Vec<f64> = columns
            .iter()
            .map(|l| lane_width.get(&lane_key(l)).copied().unwrap_or(0.0))
            .collect();
        let total =
            widths.iter().sum::<f64>() + lane_gap * (columns.len().saturating_sub(1)) as f64;
        let mut cursor = -total / 2.0;
        for (i, l) in columns.iter().enumerate() {
            let w = widths[i];
            center_x.insert(l.clone(), cursor + w / 2.0);
            cursor += w;
            if i + 1 < columns.len() {
                cursor += lane_gap;
            }
        }
    }

    let mut gap_center_y: Vec<f64> = vec![0.0; layers.len()];
    let mut y_offset = 0.0;
    for (li, layer) in layers.iter().enumerate() {
        let lh = layer_heights[li];
        let mut by_lane: Vec<(Option<String>, Vec<&String>)> = Vec::new();
        for id in layer {
            let l = top_lane_of(id);
            match by_lane.iter_mut().find(|(k, _)| *k == l) {
                Some((_, v)) => v.push(id),
                None => by_lane.push((l, vec![id])),
            }
        }
        for l in &columns {
            let Some((_, in_lane)) = by_lane.iter().find(|(k, _)| k == l) else {
                continue;
            };
            let cx = center_x[l];
            if in_lane.len() == 1 {
                x.insert(in_lane[0].clone(), cx);
                y.insert(in_lane[0].clone(), y_offset + lh / 2.0);
            } else {
                let widths: Vec<f64> = in_lane.iter().map(|id| width(id)).collect();
                let total = widths.iter().sum::<f64>() + node_gap * (in_lane.len() - 1) as f64;
                let mut start = cx - total / 2.0;
                for (i, id) in in_lane.iter().enumerate() {
                    let w = widths[i];
                    x.insert((*id).clone(), start + w / 2.0);
                    y.insert((*id).clone(), y_offset + lh / 2.0);
                    start += w + node_gap;
                }
            }
        }
        let gap_size = layer_gap + extra[li];
        gap_center_y[li] = y_offset + lh + gap_size / 2.0;
        y_offset += lh + gap_size;
    }

    let mut row_edge: HashMap<String, f64> = HashMap::new();
    for r in &row_labels {
        let key = row_key(r.rank, &r.lane, r.side);
        if !row_edge.contains_key(&key) {
            let in_row: Vec<&String> = layers[r.rank]
                .iter()
                .filter(|id| top_lane_of(id) == r.lane)
                .collect();
            let edges: Vec<f64> = in_row
                .iter()
                .map(|id| {
                    let cx = x.get(id.as_str()).copied().unwrap_or(f64::NAN);
                    if r.side == Side::Before {
                        cx - width(id) / 2.0
                    } else {
                        cx + width(id) / 2.0
                    }
                })
                .collect();
            let v = if r.side == Side::Before {
                edges.iter().fold(f64::INFINITY, |a, &b| a.min(b))
            } else {
                edges.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b))
            };
            row_edge.insert(key.clone(), v);
        }
        let w = width(&r.label);
        let dir = if r.side == Side::Before { -1.0 } else { 1.0 };
        let at = row_edge[&key] + dir * (node_gap + w / 2.0);
        x.insert(r.label.clone(), at);
        let first = layers[r.rank]
            .iter()
            .find(|id| top_lane_of(id) == r.lane)
            .and_then(|id| y.get(id).copied())
            .unwrap_or(f64::NAN);
        y.insert(r.label.clone(), first);
        row_edge.insert(key, at + dir * (w / 2.0));
    }

    for (_, group) in &gap_groups {
        let wanted: Vec<f64> = group
            .iter()
            .map(|&gi| {
                let gl = &gap_labels[gi];
                if gl.crosses_lanes {
                    x.get(&gl.end).copied().unwrap_or(f64::NAN)
                } else {
                    (x.get(&gl.start).copied().unwrap_or(0.0)
                        + x.get(&gl.end).copied().unwrap_or(0.0))
                        / 2.0
                }
            })
            .collect();
        let mut order: Vec<usize> = (0..group.len()).collect();
        order.sort_by(|&a, &b| {
            let d = wanted[a] - wanted[b];
            if d != 0.0 && !d.is_nan() {
                num_cmp(d, 0.0)
            } else {
                a.cmp(&b)
            }
        });
        let mut placed = vec![0.0; group.len()];
        let mut right = f64::NEG_INFINITY;
        for &i in &order {
            let w = width(&gap_labels[group[i]].label);
            placed[i] = wanted[i].max(right + node_gap + w / 2.0);
            right = placed[i] + w / 2.0;
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let shift = mean(&wanted) - mean(&placed);
        for (i, &gi) in group.iter().enumerate() {
            let gl = &gap_labels[gi];
            x.insert(gl.label.clone(), placed[i] + shift);
            y.insert(gl.label.clone(), gap_center_y[gl.gap]);
        }
    }

    for (label, near) in &unranked {
        let nx = x.get(near).copied().unwrap_or(0.0);
        let ny = y.get(near).copied().unwrap_or(0.0);
        x.insert(label.clone(), nx);
        y.insert(label.clone(), ny);
    }

    // Dummy chains of an edge line up between its ends.
    let mut by_ref: Vec<(String, Vec<&EdgeRef>)> = Vec::new();
    for e in &g.edges {
        match by_ref.iter_mut().find(|(k, _)| *k == e.ref_id) {
            Some((_, v)) => v.push(e),
            None => by_ref.push((e.ref_id.clone(), vec![e])),
        }
    }
    for (_, chain) in &by_ref {
        let r = chain[0];
        let mid = js_round(
            (x.get(&r.ref_start).copied().unwrap_or(0.0)
                + x.get(&r.ref_end).copied().unwrap_or(0.0))
                / 2.0,
        );
        let mut involved: Vec<&String> = Vec::new();
        for e in chain {
            for v in [&e.src, &e.dst] {
                if !involved.contains(&v) {
                    involved.push(v);
                }
            }
        }
        for v in involved {
            if *v == r.ref_start || *v == r.ref_end {
                continue;
            }
            if g.node_by_id.get(v).is_some_and(|n| n.is_dummy) {
                x.insert(v.clone(), mid);
            }
        }
    }
    (x, y)
}

// ---------------------------------------------------------------------------
// writeBackToLayoutData
// ---------------------------------------------------------------------------

const TOP_LANE_TITLE_BAND_HEIGHT: f64 = 21.0;
const MIN_TOP_LANE_HORIZONTAL_PADDING: f64 = 20.0;

fn top_lane_padding(lane: &LNode) -> f64 {
    lane.padding
        .unwrap_or(MIN_TOP_LANE_HORIZONTAL_PADDING)
        .max(MIN_TOP_LANE_HORIZONTAL_PADDING)
}

fn assign_top_lane_title_rect(lane: &mut LNode) {
    let (Some(x), Some(y), Some(w), Some(h), Some(ct)) = (
        lane.x,
        lane.y,
        lane.width,
        lane.height,
        lane.swimlane_content_top,
    ) else {
        lane.group_title_rect = None;
        return;
    };
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite() && ct.is_finite())
        || w <= 0.0
        || h <= 0.0
    {
        lane.group_title_rect = None;
        return;
    }
    let top = y - h / 2.0;
    let header_bottom = ct.min(y + h / 2.0);
    let th = TOP_LANE_TITLE_BAND_HEIGHT.min((header_bottom - top).max(0.0));
    let bottom = top + th;
    if bottom <= top {
        lane.group_title_rect = None;
        return;
    }
    lane.group_title_rect = Some(super::model::Rb {
        left: x - w / 2.0,
        right: x + w / 2.0,
        top,
        bottom,
    });
}

/// The depth of each group (`groupsDeepestFirst`), deepest first, stable.
fn groups_deepest_first(nodes: &[LNode]) -> Vec<usize> {
    let by_id: HashMap<&str, &LNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let depth = |n: &LNode| {
        let mut d = 0;
        let mut seen: HashSet<&str> = HashSet::new();
        seen.insert(&n.id);
        let mut p = n.parent_id.as_deref();
        while let Some(pid) = p {
            if seen.contains(pid) {
                break;
            }
            seen.insert(pid);
            d += 1;
            p = by_id.get(pid).and_then(|n| n.parent_id.as_deref());
        }
        d
    };
    let mut groups: Vec<(usize, usize)> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_group)
        .map(|(i, n)| (i, depth(n)))
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));
    groups.into_iter().map(|(i, _)| i).collect()
}

/// `writeBackToLayoutData`: node centres, label seeds, group and lane boxes.
pub fn write_back(
    layout: &mut LayoutData,
    layers: &[Vec<String>],
    coords: &(HashMap<String, f64>, HashMap<String, f64>),
    lanes: &LaneModel,
    node_gap: f64,
    layer_gap: f64,
) {
    let (cx, cy) = coords;
    let index: HashMap<String, usize> = layout
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_edge_label)
        .map(|(i, n)| (n.id.clone(), i))
        .collect();
    for (li, layer) in layers.iter().enumerate() {
        for (oi, id) in layer.iter().enumerate() {
            let Some(&i) = index.get(id) else { continue };
            let n = &mut layout.nodes[i];
            n.layer = Some(li);
            n.order = Some(oi);
            n.x = Some(cx.get(id).copied().unwrap_or(oi as f64 * node_gap));
            n.y = Some(cy.get(id).copied().unwrap_or(li as f64 * layer_gap));
        }
    }
    for n in layout.nodes.iter_mut().filter(|n| n.is_edge_label) {
        if let (Some(&x), Some(&y)) = (cx.get(&n.id), cy.get(&n.id)) {
            n.x = Some(x);
            n.y = Some(y);
        }
    }
    struct B {
        min_x: f64,
        max_x: f64,
        min_y: f64,
        max_y: f64,
    }
    let mut group_bounds: HashMap<String, B> = HashMap::new();
    let mut top_level: Vec<usize> = Vec::new();
    for gi in groups_deepest_first(&layout.nodes) {
        let gid = layout.nodes[gi].id.clone();
        if lanes.is_lane(&gid) {
            top_level.push(gi);
        }
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for child in layout
            .nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(gid.as_str()))
        {
            let x = child.x.or_else(|| cx.get(&child.id).copied());
            let y = child.y.or_else(|| cy.get(&child.id).copied());
            let w = child.width.unwrap_or(0.0);
            let h = child.height.unwrap_or(0.0);
            if let (Some(x), Some(y)) = (x, y) {
                if !child.is_edge_label {
                    min_x = min_x.min(x - w / 2.0);
                    max_x = max_x.max(x + w / 2.0);
                }
                min_y = min_y.min(y - h / 2.0);
                max_y = max_y.max(y + h / 2.0);
            }
        }
        let g = &mut layout.nodes[gi];
        if min_x == f64::INFINITY || min_y == f64::INFINITY {
            g.x = Some(g.x.unwrap_or(0.0));
            g.y = Some(g.y.unwrap_or(0.0));
            g.width = Some(g.width.unwrap_or(0.0));
            g.height = Some(g.height.unwrap_or(0.0));
        } else {
            let pad = g.padding.unwrap_or(20.0);
            let hpad = if g.parent_id.is_some() {
                pad
            } else {
                2.0 * top_lane_padding(g)
            };
            g.x = Some((min_x + max_x) / 2.0);
            g.y = Some((min_y + max_y) / 2.0);
            g.width = Some((max_x - min_x).max(0.0) + hpad);
            g.height = Some((max_y - min_y).max(0.0) + pad);
            group_bounds.insert(
                gid,
                B {
                    min_x,
                    max_x,
                    min_y,
                    max_y,
                },
            );
        }
    }
    if top_level.is_empty() || group_bounds.is_empty() {
        return;
    }
    let mut gmin_y = f64::INFINITY;
    let mut gmax_y = f64::NEG_INFINITY;
    let mut max_pad: f64 = 0.0;
    for &li in &top_level {
        let lane = &layout.nodes[li];
        let pad = lane.padding.unwrap_or(20.0);
        if pad > max_pad {
            max_pad = pad;
        }
        if let Some(b) = group_bounds.get(&lane.id) {
            gmin_y = gmin_y.min(b.min_y);
            gmax_y = gmax_y.max(b.max_y);
        }
    }
    if gmin_y == f64::INFINITY || gmax_y == f64::NEG_INFINITY {
        return;
    }
    let content_h = (gmax_y - gmin_y).max(0.0);
    let vmargin = max_pad.max(36.0);
    let lane_h = content_h + 2.0 * vmargin;
    let center_y = (gmin_y + gmax_y) / 2.0;
    for &li in &top_level {
        let lane = &mut layout.nodes[li];
        lane.y = Some(center_y);
        lane.height = Some(lane_h);
        lane.swimlane_content_top = Some(gmin_y);
    }
    let mut sorted = top_level.clone();
    sorted.sort_by(|&a, &b| {
        num_cmp(
            layout.nodes[a].x.unwrap_or(0.0),
            layout.nodes[b].x.unwrap_or(0.0),
        )
    });
    let mut ids: Vec<String> = Vec::new();
    let mut centers: Vec<f64> = Vec::new();
    let mut base: Vec<f64> = Vec::new();
    for &li in &sorted {
        let lane = &layout.nodes[li];
        let Some(b) = group_bounds.get(&lane.id) else {
            continue;
        };
        ids.push(lane.id.clone());
        centers.push((b.min_x + b.max_x) / 2.0);
        base.push((b.max_x - b.min_x).max(0.0) + 2.0 * top_lane_padding(lane));
    }
    let count = ids.len();
    if count == 0 {
        return;
    }
    let mut widths: HashMap<String, f64> = HashMap::new();
    if count == 1 {
        widths.insert(ids[0].clone(), base[0]);
    } else {
        let mut u = vec![0.0; count];
        for i in 0..count - 1 {
            u[i + 1] = 2.0 * (centers[i + 1] - centers[i]) - u[i];
        }
        let mut lower: f64 = 0.0;
        let mut upper = f64::INFINITY;
        for i in 0..count {
            if i % 2 == 0 {
                lower = lower.max(base[i] - u[i]);
            } else {
                upper = upper.min(u[i] - base[i]);
            }
        }
        let xv = if lower <= upper {
            (lower + upper) / 2.0
        } else {
            lower
        };
        for i in 0..count {
            let w = u[i] + if i % 2 == 0 { xv } else { -xv };
            widths.insert(ids[i].clone(), base[i].max(w));
        }
    }
    for &li in &top_level {
        let lane = &mut layout.nodes[li];
        if let Some(&w) = widths.get(&lane.id) {
            lane.width = Some(w);
        }
        assign_top_lane_title_rect(lane);
    }
}

/// `sugiyamaLayout` + `writeBackToLayoutData` (`runSwimlaneLayoutCore` up to routing).
pub fn layout_core(
    layout: &mut LayoutData,
    node_gap: f64,
    layer_gap: f64,
    ignore_cross_lane: bool,
) {
    let lanes = LaneModel::new(&layout.nodes);
    let g0 = to_graph_view(layout);
    let acyclic = remove_cycles(&g0);
    let layering = if ignore_cross_lane {
        assign_layers_lane_aware(&acyclic, &lanes, true, &layout.direction)
    } else {
        // `assignLayers_Gravity` is not ported: `ignoreCrossLaneEdges: false` ranks
        // every edge, which the lane-aware layering does with weight 1.
        assign_layers_lane_aware(&acyclic, &lanes, false, &layout.direction)
    };
    let (proper, with_dummies) = make_proper_layering(&layering, &acyclic);
    let lane_order = top_lane_order(layout, &lanes);
    let ordered = order_layers(&proper, &with_dummies, &lanes, &lane_order);
    let coords = assign_coordinates(
        &ordered,
        &with_dummies,
        layout,
        &lanes,
        &lane_order,
        layer_gap,
        node_gap,
        &layout.direction,
    );
    write_back(layout, &ordered, &coords, &lanes, node_gap, layer_gap);
    let _ = EPS;
}
