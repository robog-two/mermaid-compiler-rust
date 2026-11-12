//! `swimlane-beta` diagrams (mermaid 12.3 `swimlane`).
//!
//! Upstream a swimlane diagram is a flowchart drawn by a different layout engine
//! (`rendering-util/layout-algorithms/swimlanes`): the flowchart parser and node
//! shapes are reused, every top-level subgraph is a lane, and nodes outside any
//! subgraph go to a synthesised untitled lane. This module does the same:
//!
//! 1. [`prepare`] parses the body with our flowchart parser and assigns each node
//!    to its lane (pure).
//! 2. [`layout`] is the lane-aware Sugiyama core of `layoutCore.ts` in canonical
//!    top-to-bottom space (cycle removal, `assignLayers_LaneAwareCompact` with one
//!    node per lane and rank, `assignCoordinates` with label room), followed by the
//!    lane sizing of `writeBackToLayoutData` (TB/BT) or `applyLrDirectionTransform`
//!    (LR/RL) (pure).
//! 3. [`route`] is a small orthogonal router in the same spirit as upstream's
//!    `routeEdgesOrthogonal`: straight runs where nodes line up, L/Z handoffs between
//!    lanes, side rails for back edges and a rail around blocked same-rank edges (pure).
//!    Subgraphs nested in a lane (any depth) get a titled, padded box; their
//!    members are kept in their own track across the flow so siblings stay out.
//! 4. [`render`] draws lanes (title band + body), nodes through the shared flowchart
//!    shape code, rounded orthogonal edges with line hops, and labels.

use super::flowchart;
use super::graph::{self, ClusterId, Endpoint, GraphDiagram, LineKind};
use super::markers::{self, End, MarkerColors};
use crate::config::{Look as ConfigLook, MermaidConfig};
use crate::error::Result;
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::Direction;
use crate::scene::{DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::{ColorSlot, Palette};
use std::collections::{HashMap, HashSet};

/// `EDGE_ROUTING.LANE_MARGIN`: distance kept between a rail and the nodes it passes.
const LANE_MARGIN: f64 = 20.0;
/// `EDGE_ROUTING.EDGE_GAP`: spacing between parallel rails.
const EDGE_GAP: f64 = 12.0;
/// `COORDINATES.EDGE_LABEL_CLEARANCE`.
const EDGE_LABEL_CLEARANCE: f64 = 16.0;
/// `TOP_LANE_TITLE_BAND_HEIGHT` / header room of a TB lane (`minHeaderMargin`).
const TB_HEADER_MARGIN: f64 = 36.0;
/// `LANE_TITLE_BAND` of the LR transform.
const LR_TITLE_BAND: f64 = 36.0;
/// `MIN_TOP_LANE_HORIZONTAL_PADDING`.
const MIN_LANE_PADDING: f64 = 20.0;
/// Flowchart subgraph `padding`.
const LANE_PADDING: f64 = 8.0;
/// Line-hop radius (`applySwimlaneLineJumps`: `jumpRadius: 6`).
const HOP_RADIUS: f64 = 6.0;
/// Corner cut of the `rounded` edge curve.
const CORNER_CUT: f64 = 5.0 * std::f64::consts::SQRT_2;
/// Edge labels wrap at this width (`max-width: 120px` on swimlane labels).
const LABEL_WRAP: f64 = 120.0;

// ---------------------------------------------------------------------------
// Preparation: flowchart IR → lanes
// ---------------------------------------------------------------------------

/// A lane: a top-level subgraph, or the untitled lane holding loose nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// The subgraph, `None` for the synthesised default lane.
    pub cluster: Option<ClusterId>,
    pub id: String,
    pub title: String,
    pub slot: ColorSlot,
}

/// A parsed swimlane diagram: the flowchart graph plus its lanes.
#[derive(Debug, Clone, PartialEq)]
pub struct Swimlanes {
    pub graph: GraphDiagram,
    /// Lanes in drawing order (the default lane, if any, first).
    pub lanes: Vec<Lane>,
    /// Lane index of every node.
    pub lane_of: Vec<usize>,
}

/// Parse a `swimlane-beta [dir]` body.
pub fn parse(body: &str, title: Option<String>) -> Result<Swimlanes> {
    let src = flowchart_source(body);
    let g = flowchart::parse(&src, title)?;
    Ok(prepare(g, &lanes_with_content(body)))
}

/// The body with its `swimlane-beta` keyword replaced by `flowchart`.
fn flowchart_source(body: &str) -> String {
    match body.find("swimlane-beta") {
        Some(i) => format!(
            "{}flowchart{}",
            &body[..i],
            &body[i + "swimlane-beta".len()..]
        ),
        None => body.to_string(),
    }
}

/// Ids of the top-level subgraphs that list something between `subgraph` and `end`.
/// A lane that lists members but ends up with none lost them all to an earlier lane
/// (`dropLanesEmptiedBySharedNodes`); one the author left empty stays.
fn lanes_with_content(body: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut depth = 0usize;
    let mut current: Option<String> = None;
    let mut auto = 0usize;
    for (_, stmt) in super::statements(body) {
        let (kw, rest) = match stmt.find(char::is_whitespace) {
            Some(i) => (&stmt[..i], stmt[i..].trim()),
            None => (stmt.as_str(), ""),
        };
        match kw {
            "subgraph" => {
                let id = if rest.starts_with('"') || rest.is_empty() {
                    auto += 1;
                    format!("subGraph{}", auto - 1)
                } else {
                    let end = rest.find(['[', ' ', '\t']).unwrap_or(rest.len());
                    rest[..end].trim().to_string()
                };
                if depth == 0 {
                    current = Some(id);
                } else if let Some(c) = &current {
                    out.insert(c.clone());
                }
                depth += 1;
            }
            "end" if rest.is_empty() && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    current = None;
                }
            }
            _ => {
                if let Some(c) = current.as_ref().filter(|_| depth > 0) {
                    out.insert(c.clone());
                }
            }
        }
    }
    out
}

/// Assign every node to its lane (`prepareLayoutForSwimlanes`).
pub fn prepare(g: GraphDiagram, with_content: &HashSet<String>) -> Swimlanes {
    let top = |mut c: ClusterId| {
        for _ in 0..=g.clusters.len() {
            match g.clusters[c.0].parent {
                Some(p) => c = p,
                None => break,
            }
        }
        c
    };
    let node_top: Vec<Option<ClusterId>> = g.nodes.iter().map(|n| n.cluster.map(top)).collect();
    let edge_targets: HashSet<usize> = g
        .edges
        .iter()
        .flat_map(|e| [e.from, e.to])
        .filter_map(|ep| match ep {
            Endpoint::Cluster(c) => Some(top(c).0),
            Endpoint::Node(_) => None,
        })
        .collect();
    let mut lanes = Vec::new();
    if node_top.iter().any(Option::is_none) {
        lanes.push(Lane {
            cluster: None,
            id: "__swimlane_default__".into(),
            title: String::new(),
            slot: ColorSlot(g.clusters.len()),
        });
    }
    for (ci, c) in g.clusters.iter().enumerate() {
        if c.parent.is_some() {
            continue;
        }
        let empty = !node_top.contains(&Some(ClusterId(ci)));
        if empty && with_content.contains(&c.id) && !edge_targets.contains(&ci) {
            continue; // emptied by nodes an earlier lane kept
        }
        lanes.push(Lane {
            cluster: Some(ClusterId(ci)),
            id: c.id.clone(),
            title: c.title.clone(),
            slot: c.color_slot.unwrap_or(ColorSlot(ci)),
        });
    }
    // `prepareLayoutForSwimlanes`: the synthesised lane takes the slot after the
    // highest one of the subgraphs that are still drawn.
    if lanes.first().is_some_and(|l| l.cluster.is_none()) {
        let kept = |ci: usize| {
            let t = top(ClusterId(ci));
            lanes.iter().any(|l| l.cluster == Some(t))
        };
        let next = g
            .clusters
            .iter()
            .enumerate()
            .filter(|(ci, _)| kept(*ci))
            .map(|(ci, c)| c.color_slot.unwrap_or(ColorSlot(ci)).0 + 1)
            .max()
            .unwrap_or(0);
        lanes[0].slot = ColorSlot(next);
    }
    let lane_of = node_top
        .iter()
        .map(|t| {
            lanes
                .iter()
                .position(|l| l.cluster == *t)
                .unwrap_or_default()
        })
        .collect();
    Swimlanes {
        graph: g,
        lanes,
        lane_of,
    }
}

// ---------------------------------------------------------------------------
// Layout (canonical TB space, then direction transform)
// ---------------------------------------------------------------------------

/// A node to place: its drawn size and lane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutNode {
    pub w: f64,
    pub h: f64,
    pub lane: usize,
    /// Innermost subgraph nested in the lane that holds the node.
    pub group: Option<usize>,
}

/// A subgraph nested in a lane (at any depth): a titled, padded box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutGroup {
    /// The nested subgraph around this one, `None` when the lane holds it directly.
    pub parent: Option<usize>,
    pub lane: usize,
    /// Title text size, `(0, 0)` without a title.
    pub title: (f64, f64),
    /// Padding kept between the box and what it holds.
    pub pad: f64,
}

/// An edge between two nodes, with its label size if it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<(f64, f64)>,
}

/// Everything the lane layout needs.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutInput {
    /// Node ids, used (like upstream) to break ties deterministically.
    pub ids: Vec<String>,
    pub nodes: Vec<LayoutNode>,
    pub edges: Vec<LayoutEdge>,
    /// Title text size of every lane.
    pub lanes: Vec<(f64, f64)>,
    /// Subgraphs nested in lanes, parents before children not required.
    pub groups: Vec<LayoutGroup>,
    pub direction: Direction,
    /// `flowchart.nodeSpacing`.
    pub node_gap: f64,
    /// `flowchart.rankSpacing`.
    pub layer_gap: f64,
    /// `swimlane.ignoreCrossLaneEdges`.
    pub ignore_cross_lane_edges: bool,
}

/// A lane's band: the whole lane and its title band.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LaneBand {
    pub rect: Rect,
    pub title: Rect,
}

/// Positions produced by [`layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOutput {
    pub centers: Vec<Point>,
    pub ranks: Vec<usize>,
    /// Seed position of every edge label (anchored onto the route later).
    pub labels: Vec<Option<Point>>,
    pub lanes: Vec<LaneBand>,
    /// Box of every nested subgraph (zero-sized when it holds no node).
    pub groups: Vec<Rect>,
}

/// String order used by upstream's `localeCompare` / `<` tie breaks.
fn by_id<'a>(ids: &'a [String]) -> impl Fn(&usize, &usize) -> std::cmp::Ordering + 'a {
    move |a, b| ids[*a].cmp(&ids[*b])
}

/// `removeCycles_DFS`: the edges (by index) that point backwards.
fn reversed_edges(input: &LayoutInput) -> Vec<bool> {
    let n = input.nodes.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for (ei, e) in input.edges.iter().enumerate() {
        adj[e.from].push(ei);
        indeg[e.to] += 1;
    }
    for list in &mut adj {
        list.sort_by(|a, b| {
            let (ea, eb) = (&input.edges[*a], &input.edges[*b]);
            input.ids[ea.to].cmp(&input.ids[eb.to]).then(a.cmp(b))
        });
    }
    let mut color = vec![0u8; n];
    let mut reversed = vec![false; input.edges.len()];
    let seeds = (0..n)
        .filter(|&v| indeg[v] == 0)
        .chain((0..n).filter(|&v| indeg[v] != 0));
    for s in seeds.collect::<Vec<_>>() {
        if color[s] != 0 {
            continue;
        }
        // Iterative DFS keeping the recursive visiting order.
        let mut stack: Vec<(usize, usize)> = vec![(s, 0)];
        color[s] = 1;
        while let Some(&mut (u, ref mut i)) = stack.last_mut() {
            if let Some(&ei) = adj[u].get(*i) {
                *i += 1;
                let v = input.edges[ei].to;
                match color[v] {
                    0 => {
                        color[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => reversed[ei] = true,
                    _ => {}
                }
            } else {
                color[u] = 2;
                stack.pop();
            }
        }
    }
    reversed
}

/// `sortedZeroInDegreeNodes`: zero in-degree nodes by id, except that nodes without
/// any edge keep their source order in the slots they take.
fn sorted_zero_indegree(ids: &[String], indeg: &[usize], linked: &[bool]) -> Vec<usize> {
    let mut sorted: Vec<usize> = (0..ids.len()).filter(|&v| indeg[v] == 0).collect();
    sorted.sort_by(by_id(ids));
    let mut unconnected = (0..ids.len()).filter(|&v| indeg[v] == 0 && !linked[v]);
    sorted
        .into_iter()
        .map(|v| {
            if linked[v] {
                v
            } else {
                unconnected.next().unwrap_or(v)
            }
        })
        .collect()
}

/// Topological order of the acyclic graph: Kahn with a sorted queue (TB), or by
/// generations (`topoSortByGenerationIfAcyclic`, LR).
fn topo_order(ids: &[String], edges: &[(usize, usize)], generations: bool) -> Vec<usize> {
    let n = ids.len();
    let mut indeg = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut linked = vec![false; n];
    for &(a, b) in edges {
        indeg[b] += 1;
        succ[a].push(b);
        linked[a] = true;
        linked[b] = true;
    }
    for s in &mut succ {
        s.sort_by(by_id(ids));
    }
    let mut order = Vec::with_capacity(n);
    let mut frontier = sorted_zero_indegree(ids, &indeg, &linked);
    if generations {
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for &u in &frontier {
                order.push(u);
                for &v in &succ[u] {
                    indeg[v] -= 1;
                    if indeg[v] == 0 {
                        next.push(v);
                    }
                }
            }
            next.sort_by(by_id(ids));
            frontier = next;
        }
    } else {
        let mut queue = std::collections::VecDeque::from(frontier);
        while let Some(u) = queue.pop_front() {
            order.push(u);
            for &v in &succ[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    let i = queue
                        .iter()
                        .position(|&q| ids[q] >= ids[v])
                        .unwrap_or(queue.len());
                    queue.insert(i, v);
                }
            }
        }
    }
    if order.len() == n {
        order
    } else {
        let mut all: Vec<usize> = (0..n).collect();
        all.sort_by(by_id(ids));
        all
    }
}

/// `assignLayers_LaneAwareCompact`: ranks with at most one node per lane and rank.
/// With `ignoreCrossLaneEdges` an edge between lanes does not push its target down a
/// rank, so a handoff can stay level.
pub fn assign_ranks(input: &LayoutInput) -> Vec<usize> {
    let reversed = reversed_edges(input);
    let acyclic: Vec<(usize, usize)> = input
        .edges
        .iter()
        .zip(&reversed)
        .filter(|(e, _)| e.from != e.to)
        .map(|(e, &r)| if r { (e.to, e.from) } else { (e.from, e.to) })
        .collect();
    let order = topo_order(&input.ids, &acyclic, input.direction.is_horizontal());
    let lane = |v: usize| input.nodes[v].lane;
    let mut rank = vec![0usize; input.nodes.len()];
    let mut next_free: HashMap<usize, usize> = HashMap::new();
    for v in order {
        let base = acyclic
            .iter()
            .filter(|&&(_, b)| b == v)
            .map(|&(u, _)| {
                let w = usize::from(!input.ignore_cross_lane_edges || lane(u) == lane(v));
                rank[u] + w
            })
            .max()
            .unwrap_or(0);
        let r = base.max(next_free.get(&lane(v)).copied().unwrap_or(0));
        rank[v] = r;
        next_free.insert(lane(v), r + 1);
    }
    rank
}

/// `flowAxisStretch`: average node width over average height, never below 1.
fn flow_stretch(nodes: &[LayoutNode]) -> f64 {
    if nodes.is_empty() {
        return 1.0;
    }
    let w: f64 = nodes.iter().map(|n| n.w).sum();
    let h: f64 = nodes.iter().map(|n| n.h).sum();
    if h > 0.0 { (w / h).max(1.0) } else { 1.0 }
}

/// Something a lane or nested subgraph holds directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Node(usize),
    Group(usize),
}

/// Placement across the flow of lanes that hold nested subgraphs. Each subgraph is
/// a block over the ranks of its members; the blocks and nodes a container holds go
/// into side-by-side tracks so that nothing shares a rank inside one track, which
/// keeps sibling nodes out of a subgraph's box. Upstream frames the members where
/// they land (and lets its title sit under the first node); reserving room keeps the
/// title readable and the boxes clear of their siblings.
struct Nesting<'a> {
    input: &'a LayoutInput,
    ranks: &'a [usize],
    /// First and last rank of every group's members, `None` for an empty group.
    span: Vec<Option<(usize, usize)>>,
    /// Lowest node index inside every group (source order tie break).
    first: Vec<usize>,
    /// Across-flow width of every group's box.
    block: Vec<f64>,
}

impl<'a> Nesting<'a> {
    fn new(input: &'a LayoutInput, ranks: &'a [usize]) -> Self {
        let count = input.groups.len();
        let mut span: Vec<Option<(usize, usize)>> = vec![None; count];
        let mut first = vec![usize::MAX; count];
        for (v, nd) in input.nodes.iter().enumerate() {
            for g in ancestors(&input.groups, nd.group) {
                let r = ranks[v];
                span[g] = Some(span[g].map_or((r, r), |(a, b)| (a.min(r), b.max(r))));
                first[g] = first[g].min(v);
            }
        }
        let mut nest = Nesting {
            input,
            ranks,
            span,
            first,
            block: vec![0.0; count],
        };
        let mut order: Vec<usize> = (0..count).collect();
        let depth = |g: usize| ancestors(&input.groups, Some(g)).count();
        order.sort_by_key(|&g| std::cmp::Reverse(depth(g)));
        for g in order {
            let gr = &input.groups[g];
            nest.block[g] = nest.content_width(g) + 2.0 * gr.pad + nest.header_across(g);
        }
        nest
    }

    fn horizontal(&self) -> bool {
        self.input.direction.is_horizontal()
    }

    /// Title room across the flow: LR/RL titles sit above the box, which is the
    /// canonical left side.
    fn header_across(&self, g: usize) -> f64 {
        let t = self.input.groups[g].title;
        if self.horizontal() && t.0 > 0.0 {
            t.1
        } else {
            0.0
        }
    }

    /// Title room along the flow: TB titles sit above the box (BT: below, mirrored).
    fn header_along(&self, g: usize) -> f64 {
        let t = self.input.groups[g].title;
        if !self.horizontal() && t.0 > 0.0 {
            t.1
        } else {
            0.0
        }
    }

    fn content_width(&self, g: usize) -> f64 {
        let tracks = self.tracks(Some(g), self.input.groups[g].lane);
        let title = if self.horizontal() {
            0.0
        } else {
            self.input.groups[g].title.0
        };
        tracks_width(&tracks, self.input.node_gap).max(title)
    }

    fn item_span(&self, it: Item) -> (usize, usize) {
        match it {
            Item::Node(v) => (self.ranks[v], self.ranks[v]),
            Item::Group(g) => self.span[g].unwrap_or_default(),
        }
    }

    fn item_width(&self, it: Item) -> f64 {
        match it {
            Item::Node(v) => self.input.nodes[v].w,
            Item::Group(g) => self.block[g],
        }
    }

    /// What `container` (a group, or the lane itself) holds directly, by first rank.
    fn items(&self, container: Option<usize>, lane: usize) -> Vec<Item> {
        let mut items: Vec<(usize, usize, Item)> = Vec::new();
        for (v, nd) in self.input.nodes.iter().enumerate() {
            if nd.group == container && nd.lane == lane {
                items.push((self.ranks[v], v, Item::Node(v)));
            }
        }
        for (g, gr) in self.input.groups.iter().enumerate() {
            if gr.parent == container && gr.lane == lane {
                if let Some((lo, _)) = self.span[g] {
                    items.push((lo, self.first[g], Item::Group(g)));
                }
            }
        }
        items.sort_by_key(|&(r, v, _)| (r, v));
        items.into_iter().map(|(_, _, it)| it).collect()
    }

    /// First-fit tracks: an item joins the first track none of whose items share a
    /// rank with it. Returns each track's width and items.
    fn tracks(&self, container: Option<usize>, lane: usize) -> Vec<(f64, Vec<Item>)> {
        let mut tracks: Vec<(f64, Vec<Item>)> = Vec::new();
        for it in self.items(container, lane) {
            let (lo, hi) = self.item_span(it);
            let free = tracks.iter().position(|(_, items)| {
                items.iter().all(|&o| {
                    let (a, b) = self.item_span(o);
                    b < lo || hi < a
                })
            });
            let w = self.item_width(it);
            match free {
                Some(t) => {
                    tracks[t].0 = tracks[t].0.max(w);
                    tracks[t].1.push(it);
                }
                None => tracks.push((w, vec![it])),
            }
        }
        tracks
    }

    /// Place everything `container` holds, its tracks centred in `[left, left + width]`.
    fn place(&self, container: Option<usize>, lane: usize, left: f64, width: f64, x: &mut [f64]) {
        let gap = self.input.node_gap;
        let tracks = self.tracks(container, lane);
        let mut cursor = left + (width - tracks_width(&tracks, gap)) / 2.0;
        for (tw, items) in &tracks {
            for &it in items {
                match it {
                    Item::Node(v) => x[v] = cursor + tw / 2.0,
                    Item::Group(g) => {
                        let pad = self.input.groups[g].pad;
                        let inner_left =
                            cursor + (tw - self.block[g]) / 2.0 + pad + self.header_across(g);
                        let inner_w = self.block[g] - 2.0 * pad - self.header_across(g);
                        self.place(Some(g), lane, inner_left, inner_w, x);
                    }
                }
            }
            cursor += tw + gap;
        }
    }

    /// Extra room along the flow after rank `r`, so that the boxes ending at `r` and
    /// those starting at `r + 1` fit their padding and titles.
    fn gap_after(&self, r: usize) -> f64 {
        let bt = self.input.direction == Direction::BottomTop;
        let groups = &self.input.groups;
        // Room a box needs before its first rank (canonical top) and after its last.
        let before = |g: usize| groups[g].pad + if bt { 0.0 } else { self.header_along(g) };
        let after = |g: usize| groups[g].pad + if bt { self.header_along(g) } else { 0.0 };
        let chain = |g: usize, end: bool| -> f64 {
            ancestors(groups, Some(g))
                .take_while(|&a| {
                    let (lo, hi) = self.span[a].unwrap_or_default();
                    let (glo, ghi) = self.span[g].unwrap_or_default();
                    if end { hi == ghi } else { lo == glo }
                })
                .map(|a| if end { after(a) } else { before(a) })
                .sum()
        };
        let mut ending = 0.0f64;
        let mut starting = 0.0f64;
        for g in 0..groups.len() {
            if let Some((lo, hi)) = self.span[g] {
                if hi == r {
                    ending = ending.max(chain(g, true));
                }
                if lo == r + 1 {
                    starting = starting.max(chain(g, false));
                }
            }
        }
        ending + starting
    }
}

fn tracks_width(tracks: &[(f64, Vec<Item>)], gap: f64) -> f64 {
    tracks.iter().map(|(w, _)| w).sum::<f64>() + gap * tracks.len().saturating_sub(1) as f64
}

/// `g` and the groups around it, innermost first.
fn ancestors(groups: &[LayoutGroup], g: Option<usize>) -> impl Iterator<Item = usize> + '_ {
    let mut cur = g;
    let mut guard = groups.len() + 1;
    std::iter::from_fn(move || {
        let g = cur.filter(|_| guard > 0)?;
        guard -= 1;
        cur = groups.get(g).and_then(|gr| gr.parent);
        Some(g)
    })
}

/// Boxes of the nested groups around node rects centred on `centers`: members plus
/// padding, with the title band on top (or at the bottom, for BT before mirroring).
fn fit_groups(input: &LayoutInput, centers: &[Point], title_below: bool) -> Vec<Rect> {
    let groups = &input.groups;
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&g| std::cmp::Reverse(ancestors(groups, Some(g)).count()));
    let mut rects = vec![Rect::default(); groups.len()];
    let mut filled = vec![false; groups.len()];
    for g in order {
        let mut b = Bounds::default();
        let mut any = false;
        for (v, nd) in input.nodes.iter().enumerate() {
            if nd.group == Some(g) {
                b.add_rect(Rect::centered(centers[v], nd.w, nd.h));
                any = true;
            }
        }
        for (c, gr) in groups.iter().enumerate() {
            if gr.parent == Some(g) && filled[c] {
                b.add_rect(rects[c]);
                any = true;
            }
        }
        if !any {
            continue;
        }
        let gr = &groups[g];
        let mut r = b.rect().pad(gr.pad);
        if gr.title.0 > 0.0 {
            if !title_below {
                r.y -= gr.title.1;
            }
            r.h += gr.title.1;
            let min_w = gr.title.0 + 2.0 * gr.pad;
            if r.w < min_w {
                r.x -= (min_w - r.w) / 2.0;
                r.w = min_w;
            }
        }
        rects[g] = r;
        filled[g] = true;
    }
    rects
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Before,
    After,
}

/// Lay out the lanes. Pure.
pub fn layout(input: &LayoutInput) -> LayoutOutput {
    let n = input.nodes.len();
    let ranks = assign_ranks(input);
    let horizontal = input.direction.is_horizontal();
    let node_gap = input.node_gap;
    let lane_gap = node_gap * 2.0;
    let layer_count = ranks.iter().map(|r| r + 1).max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for v in 0..n {
        layers[ranks[v]].push(v);
    }
    let lane_of = |v: usize| input.nodes[v].lane;
    // Columns: the lanes that hold a node, in lane order.
    let columns: Vec<usize> = (0..input.lanes.len())
        .filter(|l| input.nodes.iter().any(|nd| nd.lane == *l))
        .collect();
    let column = |l: usize| columns.iter().position(|&c| c == l).unwrap_or(0);

    // Edge labels: in a row (same rank) or in the gap between two ranks.
    struct RowLabel {
        edge: usize,
        rank: usize,
        lane: usize,
        side: Side,
    }
    struct GapLabel {
        edge: usize,
        gap: usize,
        crosses: bool,
    }
    let mut row_labels = Vec::new();
    let mut gap_labels = Vec::new();
    for (ei, e) in input.edges.iter().enumerate() {
        if e.label.is_none() || e.from == e.to {
            continue;
        }
        let (rs, rt) = (ranks[e.from], ranks[e.to]);
        if rs == rt {
            row_labels.push(RowLabel {
                edge: ei,
                rank: rt,
                lane: lane_of(e.to),
                side: if column(lane_of(e.from)) < column(lane_of(e.to)) {
                    Side::Before
                } else {
                    Side::After
                },
            });
        } else {
            let (lo, hi) = (rs.min(rt), rs.max(rt));
            gap_labels.push(GapLabel {
                edge: ei,
                gap: lo + (hi - lo - 1) / 2,
                crosses: lane_of(e.from) != lane_of(e.to),
            });
        }
    }
    let label_size = |ei: usize| input.edges[ei].label.unwrap_or((0.0, 0.0));

    let mut layer_h: Vec<f64> = layers
        .iter()
        .map(|l| l.iter().map(|&v| input.nodes[v].h).fold(0.0, f64::max))
        .collect();
    for rl in &row_labels {
        layer_h[rl.rank] = layer_h[rl.rank].max(label_size(rl.edge).1);
    }
    let layer_w: Vec<f64> = layers
        .iter()
        .map(|l| l.iter().map(|&v| input.nodes[v].w).fold(0.0, f64::max))
        .collect();
    let mut extra = vec![0.0; layer_count];
    if horizontal {
        for i in 0..layer_count.saturating_sub(1) {
            let normal = layer_h[i] / 2.0 + layer_h[i + 1] / 2.0;
            let required = (layer_w[i] + layer_w[i + 1]) / 2.0;
            extra[i] = (required - normal - input.layer_gap).max(0.0);
        }
    }
    let stretch = if horizontal {
        flow_stretch(&input.nodes)
    } else {
        1.0
    };
    let mut gap_need = vec![0.0f64; layer_count];
    for gl in &gap_labels {
        let (w, h) = label_size(gl.edge);
        let extent = if horizontal { w } else { h };
        gap_need[gl.gap] = gap_need[gl.gap].max((extent + 2.0 * EDGE_LABEL_CLEARANCE) / stretch);
    }
    for (e, need) in extra.iter_mut().zip(&gap_need) {
        *e += need;
    }
    let nesting = Nesting::new(input, &ranks);
    let nested_lanes: HashSet<usize> = input.groups.iter().map(|g| g.lane).collect();
    if !input.groups.is_empty() {
        for (r, e) in extra.iter_mut().enumerate() {
            *e += nesting.gap_after(r) / stretch;
        }
    }

    // Lane widths across the flow.
    let mut lane_w: HashMap<usize, f64> = columns.iter().map(|&l| (l, 0.0)).collect();
    for layer in &layers {
        let mut per: HashMap<usize, (f64, usize)> = HashMap::new();
        for &v in layer {
            let e = per.entry(lane_of(v)).or_default();
            e.0 += input.nodes[v].w;
            e.1 += 1;
        }
        for (l, (sum, count)) in per {
            let total = sum + node_gap * count.saturating_sub(1) as f64;
            let w = lane_w.entry(l).or_default();
            *w = w.max(total);
        }
    }
    let mut nested_w: HashMap<usize, f64> = HashMap::new();
    for &l in &nested_lanes {
        let w = tracks_width(&nesting.tracks(None, l), node_gap);
        nested_w.insert(l, w);
        let lw = lane_w.entry(l).or_default();
        *lw = lw.max(w);
    }
    let mut gap_groups: Vec<((usize, usize), Vec<usize>)> = Vec::new();
    for (gi, gl) in gap_labels.iter().enumerate() {
        let key = (gl.gap, lane_of(input.edges[gl.edge].to));
        match gap_groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, g)) => g.push(gi),
            None => gap_groups.push((key, vec![gi])),
        }
    }
    for ((_, l), group) in &gap_groups {
        let span: f64 = group
            .iter()
            .enumerate()
            .map(|(i, &gi)| label_size(gap_labels[gi].edge).0 + if i > 0 { node_gap } else { 0.0 })
            .sum();
        let w = lane_w.entry(*l).or_default();
        *w = w.max(span);
    }
    let mut row_span: HashMap<(usize, usize, Side), f64> = HashMap::new();
    for rl in &row_labels {
        *row_span.entry((rl.rank, rl.lane, rl.side)).or_default() +=
            node_gap + label_size(rl.edge).0;
    }
    for rl in &row_labels {
        let nodes_w: f64 = layers[rl.rank]
            .iter()
            .filter(|&&v| lane_of(v) == rl.lane)
            .enumerate()
            .map(|(i, &v)| input.nodes[v].w + if i > 0 { node_gap } else { 0.0 })
            .sum();
        let side = row_span
            .get(&(rl.rank, rl.lane, Side::Before))
            .copied()
            .unwrap_or(0.0)
            .max(
                row_span
                    .get(&(rl.rank, rl.lane, Side::After))
                    .copied()
                    .unwrap_or(0.0),
            );
        let w = lane_w.entry(rl.lane).or_default();
        *w = w.max(nodes_w + 2.0 * side);
    }

    let mut center_x: HashMap<usize, f64> = HashMap::new();
    {
        let widths: Vec<f64> = columns.iter().map(|l| lane_w[l]).collect();
        let total: f64 =
            widths.iter().sum::<f64>() + lane_gap * columns.len().saturating_sub(1) as f64;
        let mut cursor = -total / 2.0;
        for (i, &l) in columns.iter().enumerate() {
            center_x.insert(l, cursor + widths[i] / 2.0);
            cursor += widths[i] + lane_gap;
        }
    }

    let mut x = vec![0.0; n];
    let mut y = vec![0.0; n];
    let mut gap_center = vec![0.0; layer_count];
    let mut offset = 0.0;
    for (li, layer) in layers.iter().enumerate() {
        let lh = layer_h[li];
        for &l in &columns {
            let in_lane: Vec<usize> = layer.iter().copied().filter(|&v| lane_of(v) == l).collect();
            if in_lane.is_empty() {
                continue;
            }
            let total: f64 = in_lane.iter().map(|&v| input.nodes[v].w).sum::<f64>()
                + node_gap * (in_lane.len() - 1) as f64;
            let mut start = center_x[&l] - total / 2.0;
            for v in in_lane {
                x[v] = start + input.nodes[v].w / 2.0;
                y[v] = offset + lh / 2.0;
                start += input.nodes[v].w + node_gap;
            }
        }
        let gap = input.layer_gap + extra[li];
        gap_center[li] = offset + lh + gap / 2.0;
        offset += lh + gap;
    }

    for (&l, &w) in &nested_w {
        nesting.place(None, l, center_x[&l] - w / 2.0, w, &mut x);
    }

    // Label seeds.
    let mut labels: Vec<Option<Point>> = vec![None; input.edges.len()];
    let mut row_edge: HashMap<(usize, usize, Side), f64> = HashMap::new();
    for rl in &row_labels {
        let key = (rl.rank, rl.lane, rl.side);
        let in_row: Vec<usize> = layers[rl.rank]
            .iter()
            .copied()
            .filter(|&v| lane_of(v) == rl.lane)
            .collect();
        let edge = *row_edge.entry(key).or_insert_with(|| {
            let faces = in_row.iter().map(|&v| match rl.side {
                Side::Before => x[v] - input.nodes[v].w / 2.0,
                Side::After => x[v] + input.nodes[v].w / 2.0,
            });
            match rl.side {
                Side::Before => faces.fold(f64::INFINITY, f64::min),
                Side::After => faces.fold(f64::NEG_INFINITY, f64::max),
            }
        });
        let w = label_size(rl.edge).0;
        let sign = if rl.side == Side::Before { -1.0 } else { 1.0 };
        let at = edge + sign * (node_gap + w / 2.0);
        labels[rl.edge] = Some(Point::new(at, y[in_row[0]]));
        row_edge.insert(key, at + sign * w / 2.0);
    }
    for (_, group) in &gap_groups {
        let wanted: Vec<f64> = group
            .iter()
            .map(|&gi| {
                let e = &input.edges[gap_labels[gi].edge];
                if gap_labels[gi].crosses {
                    x[e.to]
                } else {
                    (x[e.from] + x[e.to]) / 2.0
                }
            })
            .collect();
        let mut order: Vec<usize> = (0..group.len()).collect();
        order.sort_by(|&a, &b| wanted[a].total_cmp(&wanted[b]).then(a.cmp(&b)));
        let mut placed = vec![0.0; group.len()];
        let mut right = f64::NEG_INFINITY;
        for i in order {
            let w = label_size(gap_labels[group[i]].edge).0;
            placed[i] = wanted[i].max(right + node_gap + w / 2.0);
            right = placed[i] + w / 2.0;
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let shift = mean(&wanted) - mean(&placed);
        for (i, &gi) in group.iter().enumerate() {
            let gl = &gap_labels[gi];
            labels[gl.edge] = Some(Point::new(placed[i] + shift, gap_center[gl.gap]));
        }
    }

    // Label boxes count towards the lane of the edge's target.
    let label_boxes: Vec<(usize, usize, Point)> = labels
        .iter()
        .enumerate()
        .filter_map(|(ei, p)| p.map(|p| (ei, lane_of(input.edges[ei].to), p)))
        .collect();

    let mut out = LayoutOutput {
        centers: (0..n).map(|v| Point::new(x[v], y[v])).collect(),
        ranks,
        labels,
        lanes: Vec::new(),
        groups: Vec::new(),
    };
    match input.direction {
        Direction::TopBottom | Direction::BottomTop => {
            out.groups = fit_groups(input, &out.centers, input.direction == Direction::BottomTop);
            out.lanes = tb_lanes(input, &out.centers, &out.groups, &label_boxes);
            if input.direction == Direction::BottomTop {
                mirror(&mut out, input, false);
            }
        }
        Direction::LeftRight | Direction::RightLeft => {
            lr_transform(input, &mut out, &label_boxes, stretch);
            if input.direction == Direction::RightLeft {
                mirror(&mut out, input, true);
            }
        }
    }
    out
}

/// Lane bands of a TB layout (`writeBackToLayoutData`): every lane spans the whole
/// content height plus header room, and the lanes tile the width without gaps.
fn tb_lanes(
    input: &LayoutInput,
    centers: &[Point],
    groups: &[Rect],
    labels: &[(usize, usize, Point)],
) -> Vec<LaneBand> {
    let lane_count = input.lanes.len();
    let mut bx: Vec<Option<(f64, f64, f64, f64)>> = vec![None; lane_count];
    let mut grow = |l: usize, x0: f64, x1: f64, y0: f64, y1: f64, with_x: bool| {
        let b = bx[l].get_or_insert((f64::INFINITY, f64::NEG_INFINITY, y0, y1));
        if with_x {
            b.0 = b.0.min(x0);
            b.1 = b.1.max(x1);
        }
        b.2 = b.2.min(y0);
        b.3 = b.3.max(y1);
    };
    for (v, nd) in input.nodes.iter().enumerate() {
        let c = centers[v];
        grow(
            nd.lane,
            c.x - nd.w / 2.0,
            c.x + nd.w / 2.0,
            c.y - nd.h / 2.0,
            c.y + nd.h / 2.0,
            true,
        );
    }
    for (gr, r) in input.groups.iter().zip(groups).filter(|(_, r)| r.w > 0.0) {
        grow(gr.lane, r.x, r.right(), r.y, r.bottom(), true);
    }
    for &(ei, l, p) in labels {
        let (_, h) = input.edges[ei].label.unwrap_or_default();
        grow(l, p.x, p.x, p.y - h / 2.0, p.y + h / 2.0, false);
    }
    let lane_pad = LANE_PADDING.max(MIN_LANE_PADDING);
    let (min_y, max_y) = bx
        .iter()
        .flatten()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, b| {
            (a.0.min(b.2), a.1.max(b.3))
        });
    let (min_y, max_y) = if min_y.is_finite() {
        (min_y, max_y)
    } else {
        (0.0, 0.0)
    };
    let margin = LANE_PADDING.max(TB_HEADER_MARGIN);
    let top = min_y - margin;
    let height = max_y - min_y + 2.0 * margin;

    let used: Vec<usize> = (0..lane_count)
        .filter(|&l| bx[l].is_some_and(|b| b.0.is_finite()))
        .collect();
    let mut sorted = used.clone();
    let cx = |l: usize| {
        let b = bx[l].unwrap_or_default();
        (b.0 + b.1) / 2.0
    };
    sorted.sort_by(|a, b| cx(*a).total_cmp(&cx(*b)));
    let base: Vec<f64> = sorted
        .iter()
        .map(|&l| {
            let b = bx[l].unwrap_or_default();
            b.1 - b.0 + 2.0 * lane_pad
        })
        .collect();
    let centers_x: Vec<f64> = sorted.iter().map(|&l| cx(l)).collect();
    let widths = tile_widths(&centers_x, &base);
    let mut bands: Vec<Option<LaneBand>> = vec![None; lane_count];
    let mut right_edge = f64::NEG_INFINITY;
    for (i, &l) in sorted.iter().enumerate() {
        let rect = Rect::new(centers_x[i] - widths[i] / 2.0, top, widths[i], height);
        right_edge = right_edge.max(rect.right());
        bands[l] = Some(LaneBand {
            rect,
            title: Rect::new(rect.x, top, rect.w, input.lanes[l].1.min(margin)),
        });
    }
    if !right_edge.is_finite() {
        right_edge = 0.0;
    }
    // Lanes left empty by the author go after the others.
    for (band, title) in bands.iter_mut().zip(&input.lanes) {
        if band.is_none() {
            let w = title.0 + 2.0 * lane_pad;
            let rect = Rect::new(right_edge, top, w, height);
            right_edge += w;
            *band = Some(LaneBand {
                rect,
                title: Rect::new(rect.x, top, w, title.1.min(margin)),
            });
        }
    }
    bands.into_iter().map(Option::unwrap_or_default).collect()
}

/// Widths that make lanes centred on `centers` share their borders, each at least
/// its `base` width (the alternating-sum solve in `writeBackToLayoutData`).
pub fn tile_widths(centers: &[f64], base: &[f64]) -> Vec<f64> {
    let count = centers.len();
    if count <= 1 {
        return base.to_vec();
    }
    let mut u = vec![0.0; count];
    for i in 0..count - 1 {
        u[i + 1] = 2.0 * (centers[i + 1] - centers[i]) - u[i];
    }
    let (mut lo, mut hi) = (0.0f64, f64::INFINITY);
    for i in 0..count {
        if i % 2 == 0 {
            lo = lo.max(base[i] - u[i]);
        } else {
            hi = hi.min(u[i] - base[i]);
        }
    }
    let x = if lo <= hi { (lo + hi) / 2.0 } else { lo };
    (0..count)
        .map(|i| {
            let w = u[i] + if i % 2 == 0 { x } else { -x };
            base[i].max(w)
        })
        .collect()
}

/// `applyLrDirectionTransform`: ranks run along x (stretched by the node aspect),
/// lanes become horizontal bands with a title band on the left.
fn lr_transform(
    input: &LayoutInput,
    out: &mut LayoutOutput,
    labels: &[(usize, usize, Point)],
    stretch: f64,
) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    for p in out.centers.iter().chain(out.labels.iter().flatten()) {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        min_y = 0.0;
    }
    let tf = |p: Point| Point::new((p.y - min_y) * stretch + LR_TITLE_BAND, p.x - min_x);
    for c in &mut out.centers {
        *c = tf(*c);
    }
    for p in out.labels.iter_mut().flatten() {
        *p = tf(*p);
    }
    let lane_count = input.lanes.len();
    let mut bx: Vec<Option<(f64, f64, f64, f64)>> = vec![None; lane_count];
    let mut grow = |l: usize, c: Point, w: f64, h: f64| {
        let b = bx[l].get_or_insert((
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ));
        b.0 = b.0.min(c.x - w / 2.0);
        b.1 = b.1.max(c.x + w / 2.0);
        b.2 = b.2.min(c.y - h / 2.0);
        b.3 = b.3.max(c.y + h / 2.0);
    };
    for (v, nd) in input.nodes.iter().enumerate() {
        grow(nd.lane, out.centers[v], nd.w, nd.h);
    }
    for &(ei, l, _) in labels {
        if let (Some(p), Some((w, h))) = (out.labels[ei], input.edges[ei].label) {
            grow(l, p, w, h);
        }
    }
    out.groups = fit_groups(input, &out.centers, false);
    for (gr, r) in input
        .groups
        .iter()
        .zip(&out.groups)
        .filter(|(_, r)| r.w > 0.0)
    {
        grow(gr.lane, r.center(), r.w, r.h);
    }
    let (gx0, gx1) = bx
        .iter()
        .flatten()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, b| {
            (a.0.min(b.0), a.1.max(b.1))
        });
    let (gx0, gx1) = if gx0.is_finite() {
        (gx0, gx1)
    } else {
        (0.0, 0.0)
    };
    let margin_x = LANE_PADDING.max(10.0);
    let body_w = gx1 - gx0 + 2.0 * margin_x;
    let lane_w = LR_TITLE_BAND + body_w;
    let lane_left = (gx0 + gx1) / 2.0 - body_w / 2.0 - LR_TITLE_BAND;
    let margin_y = LANE_PADDING.max(LR_TITLE_BAND);
    let mut used: Vec<usize> = (0..lane_count).filter(|&l| bx[l].is_some()).collect();
    let cy = |l: usize| {
        let b = bx[l].unwrap_or_default();
        (b.2 + b.3) / 2.0
    };
    used.sort_by(|a, b| cy(*a).total_cmp(&cy(*b)));
    let mut bands: Vec<Option<LaneBand>> = vec![None; lane_count];
    let mut bottom_edge = f64::NEG_INFINITY;
    for (i, &l) in used.iter().enumerate() {
        let b = bx[l].unwrap_or_default();
        let top = if i == 0 {
            b.2 - margin_y
        } else {
            (bx[used[i - 1]].unwrap_or_default().3 + b.2) / 2.0
        };
        let bottom = if i + 1 == used.len() {
            b.3 + margin_y
        } else {
            (b.3 + bx[used[i + 1]].unwrap_or_default().2) / 2.0
        };
        bottom_edge = bottom_edge.max(bottom);
        let rect = Rect::new(lane_left, top, lane_w, bottom - top);
        bands[l] = Some(LaneBand {
            rect,
            title: Rect::new(lane_left, top, input.lanes[l].1 + 8.0, bottom - top),
        });
    }
    if !bottom_edge.is_finite() {
        bottom_edge = 0.0;
    }
    for (band, title) in bands.iter_mut().zip(&input.lanes) {
        if band.is_none() {
            let h = title.0 + 2.0 * MIN_LANE_PADDING;
            let rect = Rect::new(lane_left, bottom_edge, lane_w, h);
            bottom_edge += h;
            *band = Some(LaneBand {
                rect,
                title: Rect::new(lane_left, rect.y, title.1 + 8.0, h),
            });
        }
    }
    out.lanes = bands.into_iter().map(Option::unwrap_or_default).collect();
}

/// Mirror a finished layout across the flow axis (BT: `y`, RL: `x`) about the
/// range of node centres, as `mirrorAxis` does.
fn mirror(out: &mut LayoutOutput, input: &LayoutInput, along_x: bool) {
    let coord = |p: &Point| if along_x { p.x } else { p.y };
    let (lo, hi) = out
        .centers
        .iter()
        .map(coord)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| {
            (a.0.min(v), a.1.max(v))
        });
    if !lo.is_finite() {
        return;
    }
    let m = |v: f64| lo + hi - v;
    let flip_point = |p: &mut Point| {
        if along_x {
            p.x = m(p.x);
        } else {
            p.y = m(p.y);
        }
    };
    out.centers.iter_mut().for_each(flip_point);
    out.labels.iter_mut().flatten().for_each(flip_point);
    let flip_rect = |r: Rect| {
        if along_x {
            Rect::new(m(r.right()), r.y, r.w, r.h)
        } else {
            Rect::new(r.x, m(r.bottom()), r.w, r.h)
        }
    };
    for r in out.groups.iter_mut().filter(|r| r.w > 0.0) {
        *r = flip_rect(*r);
    }
    for (l, band) in out.lanes.iter_mut().enumerate() {
        band.rect = flip_rect(band.rect);
        if along_x {
            // RL: the title band moves to the right-hand end.
            band.title = flip_rect(band.title);
        } else {
            // BT keeps the title at the top of the lane.
            band.title = Rect::new(
                band.rect.x,
                band.rect.y,
                band.rect.w,
                input.lanes[l].1.min(TB_HEADER_MARGIN),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Orthogonal routing
// ---------------------------------------------------------------------------

/// The frame the router works in: flow runs towards +y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame(Direction);

impl Frame {
    fn to(self, p: Point) -> Point {
        match self.0 {
            Direction::TopBottom => p,
            Direction::BottomTop => Point::new(p.x, -p.y),
            Direction::LeftRight => Point::new(p.y, p.x),
            Direction::RightLeft => Point::new(p.y, -p.x),
        }
    }
    fn back(self, p: Point) -> Point {
        match self.0 {
            Direction::TopBottom => p,
            Direction::BottomTop => Point::new(p.x, -p.y),
            Direction::LeftRight => Point::new(p.y, p.x),
            Direction::RightLeft => Point::new(-p.y, p.x),
        }
    }
    fn rect(self, r: Rect) -> Rect {
        let a = self.to(Point::new(r.x, r.y));
        let b = self.to(Point::new(r.right(), r.bottom()));
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        )
    }
}

/// Whether the open segment `a`–`b` (axis-aligned) passes through `r`.
fn hits(a: Point, b: Point, r: &Rect) -> bool {
    const EPS: f64 = 1.0;
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    x1 > r.x + EPS && x0 < r.right() - EPS && y1 > r.y + EPS && y0 < r.bottom() - EPS
}

fn clear(pts: &[Point], obstacles: &[Rect]) -> bool {
    pts.windows(2)
        .all(|w| obstacles.iter().all(|r| !hits(w[0], w[1], r)))
}

/// Rails already taken: (`vertical`, fixed coordinate, span).
#[derive(Default)]
struct Rails(Vec<(bool, f64, f64, f64)>);

impl Rails {
    /// Move `at` outward by `step` until no rail of the same orientation lies within
    /// half a gap over an overlapping span.
    fn claim(&mut self, vertical: bool, mut at: f64, lo: f64, hi: f64, step: f64) -> f64 {
        for _ in 0..64 {
            let taken = self.0.iter().any(|&(v, c, a, b)| {
                v == vertical && (c - at).abs() < EDGE_GAP / 2.0 && a < hi && lo < b
            });
            if !taken {
                break;
            }
            at += step;
        }
        self.0.push((vertical, at, lo, hi));
        at
    }
}

fn mid(a: f64, b: f64) -> f64 {
    (a + b) / 2.0
}

/// One route in the router frame.
fn route_one(s: Rect, t: Rect, obstacles: &[Rect], rails: &mut Rails) -> Vec<Point> {
    let (sc, tc) = (s.center(), t.center());
    let same_row = s.y < t.bottom() && t.y < s.bottom();
    let same_col = s.x < t.right() && t.x < s.right();
    if same_row && !same_col {
        // A handoff within one rank: straight across, or round a rail below.
        let y0 = if (sc.y - tc.y).abs() < 1.0 {
            sc.y
        } else {
            mid(s.y.max(t.y), s.bottom().min(t.bottom()))
        };
        let (a, b) = if tc.x > sc.x {
            (Point::new(s.right(), y0), Point::new(t.x, y0))
        } else {
            (Point::new(s.x, y0), Point::new(t.right(), y0))
        };
        if clear(&[a, b], obstacles) {
            return vec![a, b];
        }
        let (x0, x1) = (sc.x.min(tc.x), sc.x.max(tc.x));
        let below = obstacles
            .iter()
            .filter(|r| r.right() > x0 && r.x < x1 && r.bottom() > s.y.min(t.y))
            .map(|r| r.bottom())
            .fold(s.bottom().max(t.bottom()), f64::max);
        let rail = rails.claim(false, below + LANE_MARGIN, x0, x1, EDGE_GAP);
        return vec![
            Point::new(sc.x, s.bottom()),
            Point::new(sc.x, rail),
            Point::new(tc.x, rail),
            Point::new(tc.x, t.bottom()),
        ];
    }
    let forward = t.y >= s.bottom() - 1.0;
    if forward && same_col {
        let x0 = if sc.x > t.x && sc.x < t.right() {
            sc.x
        } else if tc.x > s.x && tc.x < s.right() {
            tc.x
        } else {
            mid(s.x.max(t.x), s.right().min(t.right()))
        };
        let path = vec![Point::new(x0, s.bottom()), Point::new(x0, t.y)];
        if clear(&path, obstacles) {
            return path;
        }
    } else if !same_col {
        // Handoff to another lane: leave sideways, turn into the target's near face
        // (top for a later rank, bottom for an earlier one).
        let a = if tc.x > sc.x {
            Point::new(s.right(), sc.y)
        } else {
            Point::new(s.x, sc.y)
        };
        if !forward {
            // Back to an earlier rank: up out of the source, across into the target.
            let b = vec![
                Point::new(sc.x, s.y),
                Point::new(sc.x, tc.y),
                if tc.x > sc.x {
                    Point::new(t.x, tc.y)
                } else {
                    Point::new(t.right(), tc.y)
                },
            ];
            if (tc.x - sc.x).abs() > (s.w + t.w) / 2.0 && clear(&b, obstacles) {
                return b;
            }
        }
        let end_y = if forward { t.y } else { t.bottom() };
        let l = vec![a, Point::new(tc.x, sc.y), Point::new(tc.x, end_y)];
        if (tc.y - sc.y).abs() > t.h / 2.0 && clear(&l, obstacles) {
            return l;
        }
        if forward {
            let ym = mid(s.bottom(), t.y);
            let z = vec![
                Point::new(sc.x, s.bottom()),
                Point::new(sc.x, ym),
                Point::new(tc.x, ym),
                Point::new(tc.x, t.y),
            ];
            if clear(&z, obstacles) {
                return z;
            }
        }
    }
    // Back edge (or a blocked one): run along a rail beside the nodes it passes.
    let left = tc.x < sc.x - 1.0 || (tc.x - sc.x).abs() <= 1.0;
    let (y0, y1) = (sc.y.min(tc.y), sc.y.max(tc.y));
    let mut rail = if left {
        s.x.min(t.x) - LANE_MARGIN
    } else {
        s.right().max(t.right()) + LANE_MARGIN
    };
    for _ in 0..obstacles.len() + 1 {
        let blocker = obstacles
            .iter()
            .filter(|r| r.bottom() > y0 && r.y < y1 && r.x < rail + 1.0 && r.right() > rail - 1.0)
            .map(|r| if left { r.x } else { r.right() })
            .reduce(if left { f64::min } else { f64::max });
        match blocker {
            Some(edge) => {
                rail = if left {
                    edge - LANE_MARGIN
                } else {
                    edge + LANE_MARGIN
                }
            }
            None => break,
        }
    }
    let step = if left { -EDGE_GAP } else { EDGE_GAP };
    let rail = rails.claim(true, rail, y0, y1, step);
    let (sx, tx) = if left {
        (s.x, t.x)
    } else {
        (s.right(), t.right())
    };
    vec![
        Point::new(sx, sc.y),
        Point::new(rail, sc.y),
        Point::new(rail, tc.y),
        Point::new(tx, tc.y),
    ]
}

/// Route every edge `(from, to)` between `boxes` orthogonally. Boxes before
/// `obstacle_count` are nodes that other routes keep clear of. Pure.
pub fn route(
    boxes: &[Rect],
    obstacle_count: usize,
    edges: &[(usize, usize)],
    direction: Direction,
) -> Vec<Vec<Point>> {
    let frame = Frame(direction);
    let fb: Vec<Rect> = boxes.iter().map(|r| frame.rect(*r)).collect();
    let mut rails = Rails::default();
    edges
        .iter()
        .map(|&(a, b)| {
            let s = fb[a];
            let pts = if a == b {
                let c = s.center();
                let x = s.right() + LANE_MARGIN;
                vec![
                    Point::new(s.right(), c.y - s.h / 4.0),
                    Point::new(x, c.y - s.h / 4.0),
                    Point::new(x, c.y + s.h / 4.0),
                    Point::new(s.right(), c.y + s.h / 4.0),
                ]
            } else {
                let obstacles: Vec<Rect> = fb[..obstacle_count.min(fb.len())]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != a && *i != b)
                    .map(|(_, r)| *r)
                    .collect();
                route_one(s, fb[b], &obstacles, &mut rails)
            };
            pts.into_iter().map(|p| frame.back(p)).collect()
        })
        .collect()
}

/// Where a label sits on a route: the middle of its longest horizontal run that
/// holds it, otherwise of its longest run.
pub fn label_anchor(pts: &[Point], label_w: f64) -> Point {
    let seg_len = |w: &[Point]| w[0].dist(w[1]);
    let horizontal = pts
        .windows(2)
        .filter(|w| (w[0].y - w[1].y).abs() < 0.5 && seg_len(w) >= label_w + 8.0)
        .max_by(|a, b| seg_len(a).total_cmp(&seg_len(b)));
    let best = horizontal.or_else(|| {
        pts.windows(2)
            .max_by(|a, b| seg_len(a).total_cmp(&seg_len(b)))
    });
    best.map(|w| w[0].lerp(w[1], 0.5))
        .unwrap_or_else(|| pts.first().copied().unwrap_or_default())
}

/// Crossings where a horizontal run of route `i` hops over a vertical run of another
/// route (`findEdgeIntersections`: the horizontal segment takes the hop).
fn hop_points(routes: &[Vec<Point>], i: usize, seg: usize) -> Vec<f64> {
    let (a, b) = (routes[i][seg], routes[i][seg + 1]);
    if (a.y - b.y).abs() > 0.5 {
        return Vec::new();
    }
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let clearance = HOP_RADIUS + CORNER_CUT + 2.0;
    let mut xs: Vec<f64> = routes
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .flat_map(|(_, r)| r.windows(2))
        .filter(|w| (w[0].x - w[1].x).abs() < 0.5)
        .filter(|w| {
            let (y0, y1) = (w[0].y.min(w[1].y), w[0].y.max(w[1].y));
            a.y > y0 + 1.0 && a.y < y1 - 1.0 && w[0].x > x0 + clearance && w[0].x < x1 - clearance
        })
        .map(|w| w[0].x)
        .collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|p, q| (*p - *q).abs() < 2.0 * HOP_RADIUS);
    if b.x < a.x {
        xs.reverse();
    }
    xs
}

/// Path of a route with rounded corners (`curve: 'rounded'`) and line hops.
fn route_path(routes: &[Vec<Point>], i: usize, hops: bool) -> Vec<PathCmd> {
    let pts = &routes[i];
    let n = pts.len();
    let mut cmds = vec![PathCmd::MoveTo(pts[0])];
    // Corner cut at every interior point.
    let cut: Vec<f64> = (0..n)
        .map(|k| {
            if k == 0 || k + 1 == n {
                0.0
            } else {
                CORNER_CUT
                    .min(pts[k].dist(pts[k - 1]) / 2.0)
                    .min(pts[k].dist(pts[k + 1]) / 2.0)
            }
        })
        .collect();
    for k in 0..n - 1 {
        let (a, b) = (pts[k], pts[k + 1]);
        let d = b.minus(a).norm();
        if hops {
            for x in hop_points(routes, i, k) {
                let dir = if b.x > a.x { 1.0 } else { -1.0 };
                let start = Point::new(x - dir * HOP_RADIUS, a.y);
                cmds.push(PathCmd::LineTo(start));
                let c = Point::new(x, a.y);
                let (a0, a1) = if dir > 0.0 {
                    (std::f64::consts::PI, std::f64::consts::TAU)
                } else {
                    (0.0, -std::f64::consts::PI)
                };
                cmds.extend(geom::arc_cubics(c, HOP_RADIUS, a0, a1));
            }
        }
        let end = b.minus(d.scale(cut[k + 1]));
        cmds.push(PathCmd::LineTo(end));
        if k + 2 < n && cut[k + 1] > 0.0 {
            let next = pts[k + 2].minus(b).norm();
            let out = b.plus(next.scale(cut[k + 1]));
            // Quadratic corner through `b` as a cubic.
            cmds.push(PathCmd::CubicTo(
                end.lerp(b, 2.0 / 3.0),
                out.lerp(b, 2.0 / 3.0),
                out,
            ));
        }
    }
    cmds
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Parse, lay out and draw a `swimlane-beta` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let sl = parse(body, title)?;
    Ok(draw(&sl, cfg, pal))
}

/// Draw a parsed swimlane diagram.
pub fn draw(sl: &Swimlanes, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let g = &sl.graph;
    let gcfg = &cfg.flowchart;
    let font = pal.font_size;
    let horizontal = g.direction.is_horizontal();
    let sizes: Vec<(f64, f64)> = g
        .nodes
        .iter()
        .map(|n| graph::node_size(n, DiagramKind::Flowchart, gcfg, pal, horizontal))
        .collect();
    let label_blocks: Vec<Option<TextBlock>> = g
        .edges
        .iter()
        .map(|e| {
            e.label
                .as_ref()
                .map(|l| {
                    TextBlock::new(
                        super::timeline::wrap_text(l, LABEL_WRAP, font, false),
                        font,
                        false,
                    )
                })
                .filter(|b| !b.is_empty())
        })
        .collect();
    let lane_titles: Vec<TextBlock> = sl
        .lanes
        .iter()
        .map(|l| TextBlock::label(&l.title, font))
        .collect();
    let (group_of, groups) = nested_groups(sl, |t| {
        let t = TextBlock::label(t, font);
        if t.is_empty() {
            (0.0, 0.0)
        } else {
            (t.width, t.height)
        }
    });
    let lane_of_cluster = |ci: usize| lane_of_cluster(sl, ClusterId(ci));
    let node_edges = layout_edges(sl, &group_of);
    let input = LayoutInput {
        ids: g.nodes.iter().map(|n| n.id.clone()).collect(),
        nodes: g
            .nodes
            .iter()
            .enumerate()
            .map(|(i, _)| LayoutNode {
                w: sizes[i].0,
                h: sizes[i].1,
                lane: sl.lane_of[i],
                group: g.nodes[i].cluster.and_then(|c| group_of[c.0]),
            })
            .collect(),
        groups,
        edges: node_edges
            .iter()
            .map(|&(ei, a, b)| LayoutEdge {
                from: a,
                to: b,
                label: label_blocks[ei].as_ref().map(|b| (b.width, b.height)),
            })
            .collect(),
        lanes: lane_titles
            .iter()
            // An untitled lane has no title strip (its label box is empty).
            .map(|t| {
                if t.is_empty() {
                    (0.0, 0.0)
                } else {
                    (t.width, t.height)
                }
            })
            .collect(),
        direction: g.direction,
        node_gap: gcfg.node_spacing,
        layer_gap: gcfg.rank_spacing,
        ignore_cross_lane_edges: cfg.section_bool("swimlane", "ignoreCrossLaneEdges", true),
    };
    let out = layout(&input);

    // Boxes for routing: nodes, then every subgraph (lanes and nested boxes).
    let node_rects: Vec<Rect> = out
        .centers
        .iter()
        .zip(&sizes)
        .map(|(c, (w, h))| Rect::centered(*c, *w, *h))
        .collect();
    let cluster_rects: Vec<Rect> = (0..g.clusters.len())
        .map(|ci| {
            if let Some(l) = sl
                .lanes
                .iter()
                .position(|l| l.cluster == Some(ClusterId(ci)))
            {
                return out.lanes[l].rect;
            }
            // An empty nested subgraph has no box (upstream sizes it to nothing):
            // edges to it end on the nearest box around it.
            let mut c = Some(ClusterId(ci));
            for _ in 0..=g.clusters.len() {
                match c.and_then(|c| group_of[c.0]).map(|gi| out.groups[gi]) {
                    Some(r) if r.w > 0.0 => return r,
                    Some(_) => c = c.and_then(|c| g.clusters[c.0].parent),
                    None => break,
                }
            }
            out.lanes[lane_of_cluster(ci)].rect
        })
        .collect();
    let mut boxes = node_rects.clone();
    boxes.extend(cluster_rects.iter().copied());
    let n = g.nodes.len();
    let index = |ep: Endpoint| match ep {
        Endpoint::Node(i) => i.0,
        Endpoint::Cluster(c) => n + c.0,
    };
    let visible: Vec<usize> = (0..g.edges.len())
        .filter(|&ei| g.edges[ei].line != LineKind::Invisible)
        .collect();
    let pairs: Vec<(usize, usize)> = visible
        .iter()
        .map(|&ei| (index(g.edges[ei].from), index(g.edges[ei].to)))
        .collect();
    let routes = route(&boxes, n, &pairs, g.direction);
    let hops = !matches!(
        cfg.section("swimlane", "lineHops"),
        Some(crate::json::Json::Bool(false))
    );

    let mut bounds = Bounds::default();
    // Lanes.
    let mut lane_els = Vec::new();
    for (l, lane) in sl.lanes.iter().enumerate() {
        let band = out.lanes[l];
        bounds.add_rect(band.rect);
        let style = lane
            .cluster
            .map(|c| g.clusters[c.0].style.clone())
            .unwrap_or_default();
        let slot = pal.color_slots.as_ref().map(|p| p.slot(lane.slot));
        let fill = style
            .fill
            .clone()
            .or_else(|| slot.as_ref().and_then(|s| s.fill.clone()))
            .unwrap_or_else(|| pal.cluster_bkg.clone());
        let stroke = style
            .stroke
            .clone()
            .or_else(|| slot.as_ref().map(|s| s.border.clone()))
            .unwrap_or_else(|| pal.cluster_border.clone());
        let mut paint = Paint::both(&fill, &stroke, style.stroke_width.unwrap_or(1.0));
        paint.dash = style.dash.clone();
        let mut children = vec![Element::classed_shape(
            Geometry::Rect {
                rect: band.rect,
                rx: 0.0,
            },
            paint.clone(),
            "swimlane-body",
        )];
        if band.title.w > 0.0 && band.title.h > 0.0 {
            children.push(Element::classed_shape(
                Geometry::Rect {
                    rect: band.title,
                    rx: 0.0,
                },
                paint,
                "swimlane-title",
            ));
        }
        let t = &lane_titles[l];
        if !t.is_empty() {
            let color = style
                .color
                .clone()
                .unwrap_or_else(|| pal.title_color.clone());
            let c = band.title.center();
            let text = Text::centered(c, &t.lines, font, t.line_height, &color);
            let label = Element::classed_text(text, "cluster-label swimlane-label");
            children.push(if horizontal {
                Element::Rotated {
                    deg: -90.0,
                    about: c,
                    children: vec![label],
                }
            } else {
                label
            });
        }
        lane_els.push(Element::Group {
            id: Some(lane.id.clone()),
            class: Some("cluster swimlane".into()),
            children,
        });
    }
    // Subgraphs nested in a lane: titled boxes, outermost first, coloured from the
    // palette slot of their subgraph like the lanes.
    let mut nested: Vec<usize> = (0..g.clusters.len())
        .filter(|&ci| group_of[ci].is_some_and(|gi| out.groups[gi].w > 0.0))
        .collect();
    let depth = |ci: usize| {
        let mut d = 0;
        let mut c = g.clusters[ci].parent;
        while let Some(p) = c.filter(|_| d <= g.clusters.len()) {
            d += 1;
            c = g.clusters[p.0].parent;
        }
        d
    };
    nested.sort_by_key(|&ci| depth(ci));
    for ci in nested {
        let c = &g.clusters[ci];
        let r = cluster_rects[ci];
        let slot = pal
            .color_slots
            .as_ref()
            .map(|p| p.slot(c.color_slot.unwrap_or(ColorSlot(ci))));
        let fill = c
            .style
            .fill
            .clone()
            .or_else(|| slot.as_ref().and_then(|s| s.fill.clone()))
            .unwrap_or_else(|| pal.cluster_bkg.clone());
        let stroke = c
            .style
            .stroke
            .clone()
            .or_else(|| slot.as_ref().map(|s| s.border.clone()))
            .unwrap_or_else(|| pal.cluster_border.clone());
        let mut paint = Paint::both(&fill, &stroke, c.style.stroke_width.unwrap_or(1.0));
        paint.dash = c.style.dash.clone();
        let mut children = vec![Element::rect(r, 0.0, paint)];
        let t = TextBlock::label(&c.title, font);
        if !t.is_empty() {
            let color = c
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.title_color.clone());
            let cy = r.y + t.height / 2.0;
            children.push(Element::classed_text(
                Text::centered(
                    Point::new(r.x + r.w / 2.0, cy),
                    &t.lines,
                    font,
                    t.line_height,
                    &color,
                ),
                "cluster-label",
            ));
        }
        lane_els.push(Element::Group {
            id: Some(c.id.clone()),
            class: Some("cluster".into()),
            children,
        });
    }

    // Nodes (their boundaries are needed to end edges on round shapes).
    let mut node_els = Vec::new();
    for (i, nd) in g.nodes.iter().enumerate() {
        let (el, _) = graph::draw_node(
            nd,
            i,
            DiagramKind::Flowchart,
            gcfg,
            pal,
            horizontal,
            out.centers[i],
        );
        bounds.add_rect(node_rects[i]);
        node_els.push(el);
    }

    // Edges and labels.
    let edge_width = pal.stroke_width.unwrap_or(1.0);
    let neo = gcfg.look == ConfigLook::Neo;
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    let mut drawn_routes = routes.clone();
    for (k, &ei) in visible.iter().enumerate() {
        let e = &g.edges[ei];
        let pts = &mut drawn_routes[k];
        let last = pts.len() - 1;
        for (head, at, from) in [(e.end, last, last - 1), (e.start, 0, 1)] {
            // The marker's pull-back and, under the neo look, the end the path
            // leaves undrawn beneath it (as `graph` does).
            let back = markers::pullback_for(head, neo) + markers::hidden_end(head, neo);
            if back > 0.0 && pts[at].dist(pts[from]) > back {
                let dir = pts[at].minus(pts[from]).norm();
                pts[at] = pts[at].minus(dir.scale(back));
            }
        }
    }
    for (k, &ei) in visible.iter().enumerate() {
        let e = &g.edges[ei];
        let pts = &routes[k];
        let last = pts.len() - 1;
        let stroke = e
            .style
            .stroke
            .clone()
            .unwrap_or_else(|| pal.line_color.clone());
        let width = e
            .style
            .stroke_width
            .unwrap_or(if e.line == LineKind::Thick {
                3.5
            } else {
                edge_width
            });
        let colors = MarkerColors {
            line: &stroke,
            background: &pal.main_bkg,
        };
        let mut heads = markers::elements(
            e.end,
            End::End,
            pts[last],
            pts[last].minus(pts[last - 1]).norm(),
            colors,
            width,
            neo,
        );
        heads.extend(markers::elements(
            e.start,
            End::Start,
            pts[0],
            pts[0].minus(pts[1]).norm(),
            colors,
            width,
            neo,
        ));
        for p in pts {
            bounds.add_point(*p);
        }
        let mut paint = Paint::stroke(&stroke, width);
        paint.dash = e.style.dash.clone().or_else(|| match e.line {
            LineKind::Dotted => Some(vec![2.0]),
            LineKind::Dashed => Some(vec![3.0]),
            _ => None,
        });
        let from_id = endpoint_id(g, e.from);
        let to_id = endpoint_id(g, e.to);
        let mut children = vec![Element::classed_shape(
            Geometry::Path(route_path(&drawn_routes, k, hops)),
            paint,
            format!("flowchart-link LS-{from_id} LE-{to_id}"),
        )];
        children.extend(heads);
        edge_els.push(Element::Group {
            id: Some(format!("L-{from_id}-{to_id}-{ei}")),
            class: Some("edgePath".into()),
            children,
        });
        if let Some(block) = &label_blocks[ei] {
            let at = label_anchor(pts, block.width);
            let r = Rect::centered(at, block.width + 4.0, block.height);
            bounds.add_rect(r);
            let color = e
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.text_color.clone());
            label_els.push(Element::group(
                "edgeLabel",
                vec![
                    Element::rect(r, 0.0, Paint::fill(&pal.edge_label_background)),
                    Element::text(Text::centered(
                        at,
                        &block.lines,
                        font,
                        block.line_height,
                        &color,
                    )),
                ],
            ));
        }
    }

    let mut title_els = Vec::new();
    if let Some(t) = g.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], font * 1.125, false);
        let center = Point::new(
            content.x + content.w / 2.0,
            content.y - gcfg.title_top_margin + tb.height / 2.0 - tb.height,
        );
        bounds.add_rect(Rect::centered(center, tb.width, tb.height));
        title_els.push(Element::classed_text(
            Text::centered(
                center,
                &tb.lines,
                tb.font_size,
                tb.line_height,
                &pal.title_color,
            ),
            "flowchartTitleText",
        ));
    }
    let mut elements = vec![
        Element::group("clusters", lane_els),
        Element::group("edgePaths", edge_els),
        Element::group("edgeLabels", label_els),
        Element::group("nodes", node_els),
    ];
    elements.extend(title_els);
    Scene {
        kind: DiagramKind::Swimlane,
        view_box: bounds.rect().pad(gcfg.diagram_padding),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// The lane a subgraph belongs to: the lane of its top-level ancestor.
fn lane_of_cluster(sl: &Swimlanes, ci: ClusterId) -> usize {
    let g = &sl.graph;
    let mut c = ci;
    for _ in 0..=g.clusters.len() {
        match g.clusters[c.0].parent {
            Some(p) => c = p,
            None => break,
        }
    }
    sl.lanes
        .iter()
        .position(|l| l.cluster == Some(c))
        .unwrap_or_default()
}

/// Subgraphs nested in a lane, at any depth: the group index of every subgraph
/// (`None` for lanes) and the groups, sized by `title`.
fn nested_groups(
    sl: &Swimlanes,
    title: impl Fn(&str) -> (f64, f64),
) -> (Vec<Option<usize>>, Vec<LayoutGroup>) {
    let g = &sl.graph;
    let mut next = 0;
    let group_of: Vec<Option<usize>> = g
        .clusters
        .iter()
        .map(|c| {
            c.parent.map(|_| {
                next += 1;
                next - 1
            })
        })
        .collect();
    let groups = g
        .clusters
        .iter()
        .enumerate()
        .filter(|(_, c)| c.parent.is_some())
        .map(|(ci, c)| LayoutGroup {
            parent: c.parent.and_then(|p| group_of[p.0]),
            lane: lane_of_cluster(sl, ClusterId(ci)),
            title: title(&c.title),
            pad: LANE_PADDING,
        })
        .collect();
    (group_of, groups)
}

/// The edges that take part in the layout as `(edge, from, to)` node indices: edges
/// between nodes, and edges to a subgraph nested in a lane, which are ranked as edges
/// to its first node in source order (as dagre does with clusters). Edges to a lane
/// only get routed.
fn layout_edges(sl: &Swimlanes, group_of: &[Option<usize>]) -> Vec<(usize, usize, usize)> {
    let g = &sl.graph;
    let representative = |c: ClusterId| -> Option<usize> {
        group_of[c.0]?;
        g.nodes.iter().position(|nd| {
            let mut cur = nd.cluster;
            for _ in 0..=g.clusters.len() {
                match cur {
                    Some(x) if x == c => return true,
                    Some(x) => cur = g.clusters[x.0].parent,
                    None => return false,
                }
            }
            false
        })
    };
    let layout_end = |ep: Endpoint| match ep {
        Endpoint::Node(i) => Some(i.0),
        Endpoint::Cluster(c) => representative(c),
    };
    g.edges
        .iter()
        .enumerate()
        .filter_map(|(ei, e)| match (layout_end(e.from), layout_end(e.to)) {
            (Some(a), Some(b)) if a != b || e.from == e.to => Some((ei, a, b)),
            _ => None,
        })
        .collect()
}

fn endpoint_id(g: &GraphDiagram, e: Endpoint) -> String {
    match e {
        Endpoint::Node(i) => g.nodes[i.0].id.clone(),
        Endpoint::Cluster(c) => g.clusters[c.0].id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORT: &str = "swimlane-beta LR
  subgraph Customer
    request[Request service]
    receive[Receive update]
  end
  subgraph Support
    triage[Triage request]
    answer[Send answer]
  end
  subgraph Engineering
    investigate[Investigate issue]
    fix[Prepare fix]
  end
  request --> triage
  triage -->|Known issue| answer
  triage -->|Needs code change| investigate
  investigate --> fix --> answer
  answer --> receive";

    fn scene(src: &str) -> Scene {
        let cfg = MermaidConfig::default().scoped("swimlane");
        let pal = cfg.palette();
        let sl = parse(src, None).unwrap();
        draw(&sl, &cfg, &pal)
    }

    fn input_for(sl: &Swimlanes, dir: Direction) -> LayoutInput {
        let g = &sl.graph;
        let (group_of, groups) = nested_groups(sl, |t| {
            if t.is_empty() {
                (0.0, 0.0)
            } else {
                (60.0, 21.0)
            }
        });
        LayoutInput {
            ids: g.nodes.iter().map(|n| n.id.clone()).collect(),
            nodes: (0..g.nodes.len())
                .map(|i| LayoutNode {
                    w: 150.0,
                    h: 45.0,
                    lane: sl.lane_of[i],
                    group: g.nodes[i].cluster.and_then(|c| group_of[c.0]),
                })
                .collect(),
            groups,
            edges: layout_edges(sl, &group_of)
                .into_iter()
                .map(|(ei, a, b)| LayoutEdge {
                    from: a,
                    to: b,
                    label: g.edges[ei].label.as_ref().map(|_| (60.0, 21.0)),
                })
                .collect(),
            lanes: sl.lanes.iter().map(|_| (60.0, 21.0)).collect(),
            direction: dir,
            node_gap: 50.0,
            layer_gap: 50.0,
            ignore_cross_lane_edges: true,
        }
    }

    fn rank_of(sl: &Swimlanes, ranks: &[usize], id: &str) -> usize {
        ranks[sl.graph.nodes.iter().position(|n| n.id == id).unwrap()]
    }

    #[test]
    fn top_level_subgraphs_are_lanes_in_order() {
        let sl = parse(SUPPORT, None).unwrap();
        let ids: Vec<&str> = sl.lanes.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["Customer", "Support", "Engineering"]);
        assert_eq!(sl.graph.direction, Direction::LeftRight);
    }

    #[test]
    fn cross_lane_edges_keep_ranks_level() {
        // As upstream: a handoff to another lane does not push its target a rank down.
        let sl = parse(SUPPORT, None).unwrap();
        let ranks = assign_ranks(&input_for(&sl, Direction::LeftRight));
        for (id, r) in [
            ("request", 0),
            ("triage", 0),
            ("investigate", 0),
            ("fix", 1),
            ("answer", 1),
            ("receive", 1),
        ] {
            assert_eq!(rank_of(&sl, &ranks, id), r, "{id}");
        }
    }

    #[test]
    fn one_node_per_lane_and_rank() {
        let sl = parse(
            "swimlane-beta\nsubgraph L\n a\n b\n c\nend\nsubgraph M\n d\nend\na --> d\nb --> d",
            None,
        )
        .unwrap();
        let input = input_for(&sl, Direction::TopBottom);
        let ranks = assign_ranks(&input);
        let mut seen = HashSet::new();
        for (v, r) in ranks.iter().enumerate() {
            assert!(
                seen.insert((input.nodes[v].lane, *r)),
                "two nodes share lane and rank"
            );
        }
    }

    #[test]
    fn back_edges_are_reversed_for_ranking() {
        let sl = parse(
            "swimlane-beta TB\nsubgraph A\n x --> y --> z\n z --> x\nend",
            None,
        )
        .unwrap();
        let ranks = assign_ranks(&input_for(&sl, Direction::TopBottom));
        assert_eq!(ranks, [0, 1, 2]);
    }

    #[test]
    fn nodes_stay_inside_their_lane() {
        for dir in ["TB", "LR", "BT", "RL"] {
            let src = SUPPORT.replacen("LR", dir, 1);
            let sl = parse(&src, None).unwrap();
            let input = input_for(&sl, Direction::parse(dir).unwrap());
            let out = layout(&input);
            for (v, nd) in input.nodes.iter().enumerate() {
                let r = Rect::centered(out.centers[v], nd.w, nd.h);
                let lane = out.lanes[nd.lane].rect;
                assert!(
                    r.x >= lane.x - 1e-6
                        && r.right() <= lane.right() + 1e-6
                        && r.y >= lane.y - 1e-6
                        && r.bottom() <= lane.bottom() + 1e-6,
                    "{dir}: node {v} {r:?} outside lane {lane:?}"
                );
                assert!(
                    !r.intersects(&out.lanes[nd.lane].title),
                    "{dir}: node {v} under the title band"
                );
                for (l, band) in out.lanes.iter().enumerate() {
                    if l != nd.lane {
                        assert!(
                            !band.rect.pad(-0.5).intersects(&r),
                            "{dir}: node {v} in lane {l}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lanes_tile_without_gaps() {
        let sl = parse(SUPPORT, None).unwrap();
        let tb = layout(&input_for(&sl, Direction::TopBottom));
        for w in tb.lanes.windows(2) {
            assert!((w[0].rect.right() - w[1].rect.x).abs() < 1e-6);
            assert_eq!(w[0].rect.h, w[1].rect.h);
        }
        let lr = layout(&input_for(&sl, Direction::LeftRight));
        for w in lr.lanes.windows(2) {
            assert!((w[0].rect.bottom() - w[1].rect.y).abs() < 1e-6);
            assert_eq!(w[0].rect.w, w[1].rect.w);
        }
        // LR title bands sit on the left; TB ones on top.
        assert_eq!(lr.lanes[0].title.x, lr.lanes[0].rect.x);
        assert!(lr.lanes[0].title.w < 40.0);
        assert_eq!(tb.lanes[0].title.y, tb.lanes[0].rect.y);
    }

    #[test]
    fn tile_widths_share_borders() {
        let w = tile_widths(&[0.0, 260.0, 600.0], &[192.0, 211.0, 340.0]);
        assert!((w[0] / 2.0 + w[1] / 2.0 - 260.0).abs() < 1e-9);
        assert!((w[1] / 2.0 + w[2] / 2.0 - 340.0).abs() < 1e-9);
        assert!(w.iter().zip([192.0, 211.0, 340.0]).all(|(a, b)| *a >= b));
    }

    #[test]
    fn routes_are_orthogonal_and_avoid_nodes() {
        for dir in ["TB", "LR", "BT", "RL"] {
            let src = format!(
                "swimlane-beta {dir}\nsubgraph Intake\n collect[Collect]\n validate[Validate]\nend\nsubgraph Review\n review[Review]\n decide{{Ready?}}\nend\nsubgraph Delivery\n schedule[Schedule]\nend\ncollect --> validate --> review --> decide\ndecide -->|Yes| schedule\ndecide -->|No| collect"
            );
            let sl = parse(&src, None).unwrap();
            let input = input_for(&sl, Direction::parse(dir).unwrap());
            let out = layout(&input);
            let rects: Vec<Rect> = out
                .centers
                .iter()
                .map(|c| Rect::centered(*c, 150.0, 45.0))
                .collect();
            let pairs: Vec<(usize, usize)> = input.edges.iter().map(|e| (e.from, e.to)).collect();
            let routes = route(&rects, rects.len(), &pairs, input.direction);
            for (k, pts) in routes.iter().enumerate() {
                assert!(pts.len() >= 2);
                for w in pts.windows(2) {
                    assert!(
                        (w[0].x - w[1].x).abs() < 1e-6 || (w[0].y - w[1].y).abs() < 1e-6,
                        "{dir}: diagonal segment in route {k}"
                    );
                    for (i, r) in rects.iter().enumerate() {
                        if i != pairs[k].0 && i != pairs[k].1 {
                            assert!(!hits(w[0], w[1], r), "{dir}: route {k} crosses node {i}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn loose_nodes_get_a_default_lane_first() {
        let sl = parse(
            "swimlane-beta\n  start --> a\n  subgraph ops [Operations]\n    a\n  end",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 2);
        assert!(sl.lanes[0].cluster.is_none());
        assert_eq!(sl.lanes[1].title, "Operations");
        assert_eq!(sl.lane_of, [0, 1]);
    }

    #[test]
    fn shared_node_stays_in_first_lane_and_emptied_lane_is_dropped() {
        // swimlanesDiagram.spec.ts (#8386)
        let sl = parse(
            "swimlane-beta LR\nsubgraph lane1[Performer A]\n  a[\"step one\"]\nend\nsubgraph phase1[Phase 1]\n  a\nend",
            None,
        )
        .unwrap();
        let ids: Vec<&str> = sl.lanes.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["lane1"]);
        assert_eq!(sl.lane_of, [0]);
    }

    #[test]
    fn author_empty_lane_is_kept() {
        let src = "swimlane-beta LR\nsubgraph l1\n  a\nend\nsubgraph l2\nend";
        let sl = parse(src, None).unwrap();
        let ids: Vec<&str> = sl.lanes.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["l1", "l2"]);
        assert!(scene(src).element_count() > 4);
    }

    #[test]
    fn nested_subgraph_belongs_to_its_lane() {
        let sl = parse(
            "swimlane-beta LR\nsubgraph lane1[Performer A]\n  subgraph ph1[Phase 1]\n    a[\"step one\"]\n  end\nend",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 1);
        assert_eq!(sl.lane_of, [0]);
    }

    #[test]
    fn renders_lanes_nodes_and_rotated_titles() {
        let s = scene(SUPPORT);
        assert_eq!(s.kind, DiagramKind::Swimlane);
        let Element::Group { children, .. } = &s.elements[0] else {
            panic!("clusters group first")
        };
        assert_eq!(children.len(), 3);
        let rotated = children.iter().all(|c| match c {
            Element::Group { children, .. } => children
                .iter()
                .any(|e| matches!(e, Element::Rotated { deg, .. } if *deg == -90.0)),
            _ => false,
        });
        assert!(rotated, "LR lane titles are vertical");
        let s = scene(&SUPPORT.replacen("LR", "TB", 1));
        assert!(s.view_box.w > 0.0 && s.view_box.h > 0.0);
    }

    #[test]
    fn line_hops_on_crossings() {
        let routes = vec![
            vec![Point::new(0.0, 50.0), Point::new(100.0, 50.0)],
            vec![Point::new(50.0, 0.0), Point::new(50.0, 100.0)],
        ];
        assert_eq!(hop_points(&routes, 0, 0), [50.0]);
        assert!(hop_points(&routes, 1, 0).is_empty());
        let path = route_path(&routes, 0, true);
        assert!(path.iter().any(|c| matches!(c, PathCmd::CubicTo(..))));
    }

    /// Sales holds Intake (with Inner checks nested in it) and a sibling step that
    /// shares ranks with the nested boxes; Warehouse holds two plain steps.
    const NESTED: &str = "swimlane-beta TB
subgraph L1[Sales]
  subgraph G1[Intake]
    subgraph G2[Inner checks]
      A[Receive order] --> B[Check stock]
    end
    X[Log]
  end
  S[Side task]
  C[Confirm]
end
subgraph L2[Warehouse]
  D[Pick] --> E[Ship]
end
P --> S
A --> X
B --> C
X --> C
C --> D
P --> G1
G2 --> E";

    fn contains(outer: Rect, inner: Rect) -> bool {
        outer.x <= inner.x + 1e-6
            && outer.right() >= inner.right() - 1e-6
            && outer.y <= inner.y + 1e-6
            && outer.bottom() >= inner.bottom() - 1e-6
    }

    #[test]
    fn nested_subgraphs_hold_their_nodes_inside_their_lane() {
        for dir in ["TB", "LR", "BT", "RL"] {
            let sl = parse(&NESTED.replacen("TB", dir, 1), None).unwrap();
            let input = input_for(&sl, Direction::parse(dir).unwrap());
            assert_eq!(input.groups.len(), 2);
            let out = layout(&input);
            let rect =
                |v: usize| Rect::centered(out.centers[v], input.nodes[v].w, input.nodes[v].h);
            for (gi, gr) in input.groups.iter().enumerate() {
                let r = out.groups[gi];
                assert!(r.w > 0.0 && r.h > 0.0, "{dir}: group {gi} has a box");
                // Inside its lane, and inside the group around it.
                assert!(
                    contains(out.lanes[gr.lane].rect, r),
                    "{dir}: group {gi} leaves its lane"
                );
                assert!(
                    !r.intersects(&out.lanes[gr.lane].title),
                    "{dir}: group {gi} under the lane title"
                );
                if let Some(p) = gr.parent {
                    assert!(
                        contains(out.groups[p], r.pad(gr.pad - 1.0)),
                        "{dir}: {gi} not padded inside {p}"
                    );
                }
                // The title band at the top holds no node.
                let band = Rect::new(r.x, r.y, r.w, gr.title.1);
                for (v, nd) in input.nodes.iter().enumerate() {
                    let inside = ancestors(&input.groups, nd.group).any(|a| a == gi);
                    if inside {
                        assert!(
                            contains(r, rect(v).pad(gr.pad - 1.0)),
                            "{dir}: node {v} not padded inside {gi}"
                        );
                        assert!(
                            !band.intersects(&rect(v)),
                            "{dir}: node {v} under the title of {gi}"
                        );
                    } else {
                        assert!(
                            !r.intersects(&rect(v)),
                            "{dir}: sibling node {v} overlaps group {gi}"
                        );
                    }
                }
            }
            // Siblings at one level do not overlap each other either.
            for v in 0..input.nodes.len() {
                for w in v + 1..input.nodes.len() {
                    assert!(
                        !rect(v).intersects(&rect(w)),
                        "{dir}: nodes {v} and {w} overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn nested_subgraphs_reserve_room_for_their_box() {
        // Two steps in Sales, directly or inside a titled subgraph: the lane grows by
        // the padding (both sides) and, in TB, the flow by padding and title.
        let lane = |nested: bool, dir: &str| {
            let body = if nested {
                "subgraph G[Intake]\n a --> b\n end"
            } else {
                "a --> b"
            };
            let src = format!("swimlane-beta {dir}\nsubgraph L[Sales]\n {body}\nend");
            let sl = parse(&src, None).unwrap();
            let input = input_for(&sl, Direction::parse(dir).unwrap());
            layout(&input).lanes[0].rect
        };
        let (plain, nested) = (lane(false, "TB"), lane(true, "TB"));
        assert!((nested.w - plain.w - 2.0 * LANE_PADDING).abs() < 1e-6);
        assert!(nested.h - plain.h >= 2.0 * LANE_PADDING + 21.0 - 1e-6);
        let (plain, nested) = (lane(false, "LR"), lane(true, "LR"));
        assert!(nested.h - plain.h >= 2.0 * LANE_PADDING + 21.0 - 1e-6);
    }

    #[test]
    fn edges_to_nested_subgraphs_rank_and_route_to_their_box() {
        let s = scene(NESTED);
        let find = |id: &str| -> bool {
            fn walk(e: &Element, id: &str) -> bool {
                match e {
                    Element::Group { id: Some(i), .. } if i.starts_with(id) => true,
                    Element::Group { children, .. } => children.iter().any(|c| walk(c, id)),
                    _ => false,
                }
            }
            s.elements.iter().any(|e| walk(e, id))
        };
        assert!(find("G1") && find("G2"), "nested boxes are drawn");
        assert!(
            find("L-P-G1-") && find("L-G2-E-"),
            "edges to subgraphs are drawn"
        );
        // Within a lane, an edge into a subgraph ranks it after its source.
        let sl = parse(
            "swimlane-beta\nsubgraph L\n p\n subgraph G\n a\n end\nend\np --> G",
            None,
        )
        .unwrap();
        let input = input_for(&sl, Direction::TopBottom);
        let out = layout(&input);
        assert!(out.groups[0].y > out.centers[0].y, "G below p");
        let (group_of, _) = nested_groups(&sl, |_| (0.0, 0.0));
        assert_eq!(group_of, [None, Some(0)]);
    }

    #[test]
    fn empty_nested_subgraph_has_no_box() {
        for src in [
            "swimlane-beta LR\nsubgraph A\na --> b\nsubgraph B\nend\nend\nsubgraph C\nc\nend\nb --> c",
            "swimlane-beta TB\nn9\nsubgraph s0\nn2\nsubgraph s1\nsubgraph s2\nend\nend\nend\nn9 --> s1\ns2 --> n2",
        ] {
            let sl = parse(src, None).unwrap();
            let input = input_for(&sl, sl.graph.direction);
            let out = layout(&input);
            assert!(out.groups.iter().all(|r| r.w == 0.0));
            let s = scene(src);
            let drawn = |id: &str| {
                s.elements.iter().any(|e| match e {
                    Element::Group { children, .. } => children
                        .iter()
                        .any(|c| matches!(c, Element::Group { id: Some(i), .. } if i == id)),
                    _ => false,
                })
            };
            assert!(
                !drawn("B") && !drawn("s1") && !drawn("s2"),
                "no box for an empty subgraph"
            );
            // Every point stays within the drawing, near the lanes, not at the origin.
            assert!(s.view_box.w > 0.0);
        }
    }

    #[test]
    fn default_lane_takes_the_slot_after_the_kept_subgraphs() {
        // Lane B lost its only node to A and is dropped: the default lane gets slot 1.
        let sl = parse(
            "swimlane-beta\nsubgraph A\na\nend\nsubgraph B\na\nend\na-->b",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 2);
        assert_eq!(sl.lanes[0].cluster, None);
        assert_eq!(sl.lanes[0].slot, ColorSlot(1));
        let sl = parse(
            "swimlane-beta\nsubgraph A\na\nend\nsubgraph B\nc\nend\na-->b",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes[0].slot, ColorSlot(2));
    }

    #[test]
    fn untitled_default_lane_has_no_title_strip() {
        let s = scene("swimlane-beta\nsubgraph A\na\nend\na-->b");
        let lanes = match &s.elements[0] {
            Element::Group { children, .. } => children.clone(),
            _ => unreachable!(),
        };
        let strips = |id: &str| {
            lanes
                .iter()
                .filter_map(|l| match l {
                    Element::Group {
                        id: Some(i),
                        children,
                        ..
                    } if i == id => Some(children.len()),
                    _ => None,
                })
                .next()
                .unwrap()
        };
        assert_eq!(strips("__swimlane_default__"), 1, "body only");
        assert!(strips("A") >= 2, "body, title strip and label");
    }

    #[test]
    fn bad_input_is_a_parse_error() {
        assert!(parse("swimlane-beta XY\n a --> b", None).is_err());
        assert!(parse("swimlane-beta\nsubgraph A\n a", None).is_err());
    }

    /// Under the neo look a circle or cross head leaves the last 12.5px of its
    /// line undrawn beneath it (`markerOffsets2`), as `graph` does.
    #[test]
    fn neo_circle_head_leaves_the_line_end_undrawn() {
        let end = |look: &str| -> Point {
            let src = format!(
                "---\nconfig:\n  look: {look}\n---\nswimlane-beta\nsubgraph L\n  a\n  b\nend\na --o b"
            );
            let r = crate::render::render(&src, &MermaidConfig::default()).expect("renders");
            fn find(e: &Element) -> Option<Point> {
                match e {
                    Element::Group {
                        class: Some(c),
                        children,
                        ..
                    } if c == "edgePath" => children.iter().find_map(|c| match c {
                        Element::Shape {
                            geom: Geometry::Path(cmds),
                            ..
                        } => cmds.iter().rev().find_map(|c| match c {
                            PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) => Some(*p),
                            _ => None,
                        }),
                        _ => None,
                    }),
                    Element::Group { children, .. } => children.iter().find_map(find),
                    _ => None,
                }
            }
            r.scene.elements.iter().find_map(find).expect("edge path")
        };
        let (classic, neo) = (end("classic"), end("neo"));
        assert!(classic.dist(neo) >= 12.5 - 1e-6, "{classic:?} {neo:?}");
    }
}
