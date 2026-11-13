//! Layered (Sugiyama-style) graph layout, written from scratch.
//!
//! Pipeline, all pure:
//! 1. remove self-loops, break cycles by reversing DFS back edges (under ELK, the
//!    search starts from the sources, as ELK's depth-first cycle breaker does);
//! 2. rank with longest-path + source tightening (minlen-aware) and network simplex;
//!    under ELK in its layer units, with edge labels as nodes, by a port of ELK's
//!    own network simplex layerer (`elkns.rs`: its ties, normalisation, balancing);
//! 3. split long edges into dummy vertices (edge labels become sized dummies, as in dagre);
//! 4. add left/right border vertices for every cluster on every rank it spans;
//! 5. order layers with weighted barycentre sweeps, keeping clusters contiguous,
//!    scoring with an O(E log V) crossing count and keeping the best ordering;
//! 6. assign cross-axis coordinates with Brandes–Köpf, exactly as dagre does
//!    (four median alignments, block compaction, balanced), see `bk.rs`, then
//!    push cluster frames apart where their sides zig-zag over their layers;
//! 7. stack ranks along the rank axis and rotate for TB / BT / LR / RL.

use crate::geom::{Point, Rect};

mod bk;
pub mod boxes;
mod elkbk;
mod elkns;
mod elkorder;
mod nested;
mod ortho;
pub use ortho::{EdgeRouting, OrthoSpacing};
pub mod rank;

/// Flow direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl Direction {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::LeftRight | Direction::RightLeft)
    }

    /// Parse mermaid's `TB`, `TD`, `BT`, `LR`, `RL` (and `<`, `>`, `^`, `v`).
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "TB" | "TD" | "v" => Some(Direction::TopBottom),
            "BT" | "^" => Some(Direction::BottomTop),
            "LR" | ">" => Some(Direction::LeftRight),
            "RL" | "<" => Some(Direction::RightLeft),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub width: f64,
    pub height: f64,
    pub cluster: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutEdge {
    /// Source node; for an edge leaving a whole cluster, a member of `from_cluster`.
    pub from: usize,
    /// Target node; for an edge entering a whole cluster, a member of `to_cluster`.
    pub to: usize,
    pub minlen: usize,
    /// Label size in output space (width, height).
    pub label: Option<(f64, f64)>,
    /// The edge leaves this cluster as a whole (a subgraph / composite used as endpoint).
    pub from_cluster: Option<usize>,
    /// The edge enters this cluster as a whole.
    pub to_cluster: Option<usize>,
    /// Sizes in output space (width, height) of the labels at its start and end
    /// (class cardinalities), padded past their markers: ELK reserves room for
    /// them beside their ports (`edgeLabels.placement: TAIL / HEAD`).
    pub terminals: [Option<(f64, f64)>; 2],
}

impl LayoutEdge {
    /// A node-to-node edge.
    pub fn new(from: usize, to: usize, minlen: usize, label: Option<(f64, f64)>) -> Self {
        LayoutEdge {
            from,
            to,
            minlen,
            label,
            from_cluster: None,
            to_cluster: None,
            terminals: [None; 2],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutCluster {
    pub parent: Option<usize>,
    /// Height of the title band reserved at the top of the cluster.
    pub title_height: f64,
    /// Narrowest the cluster may be (its painted title plus padding).
    pub min_width: f64,
    /// The cluster's own `direction`, honoured when it is laid out on its own (see `nested`).
    pub direction: Option<Direction>,
}

impl LayoutCluster {
    pub fn new(parent: Option<usize>, title_height: f64) -> Self {
        LayoutCluster {
            parent,
            title_height,
            min_width: 0.0,
            direction: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutInput {
    pub nodes: Vec<LayoutNode>,
    pub edges: Vec<LayoutEdge>,
    pub clusters: Vec<LayoutCluster>,
    pub direction: Direction,
    pub node_sep: f64,
    pub rank_sep: f64,
    pub edge_sep: f64,
    pub cluster_padding: f64,
    /// Node / rank separation inside a cluster that is laid out on its own.
    pub inner_node_sep: f64,
    pub inner_rank_sep: f64,
    /// How edges are routed (dagre-style splines or ELK-style orthogonal routes).
    pub routing: EdgeRouting,
    /// mermaid's dagre layout of a cluster no edge crosses, as ER groups get it:
    /// a graph of its own (`recursiveRender`) in the other direction (`TB` turns
    /// to `LR`, any other to `TB`) unless it has its own, with the parent's node
    /// spacing and 25 more rank spacing, framed by dagre's compound borders (half
    /// a rank along the flow, half a node and an edge gap across) with no band
    /// for its title. `inner_*` and `cluster_padding` do not apply to it.
    pub dagre_groups: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRoute {
    /// From source centre to target centre (unclipped), in output space.
    pub points: Vec<Point>,
    pub label: Option<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOutput {
    pub nodes: Vec<Point>,
    pub edges: Vec<EdgeRoute>,
    /// One rectangle per input cluster (zero-sized if the cluster is empty).
    pub clusters: Vec<Rect>,
    /// Per input cluster: the direction its contents were laid out in (its own
    /// `direction` only where the layout honoured it).
    pub cluster_directions: Vec<Direction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Real,
    Dummy { labeled: bool },
    Border { cluster: usize, side: Side },
}

#[derive(Debug, Clone)]
struct Vertex {
    kind: Kind,
    rank: usize,
    /// Size in layout space: `w` along the cross axis, `h` along the rank axis.
    w: f64,
    h: f64,
    cluster: Option<usize>,
}

/// Vertices of a split edge (oriented), its label vertex, and whether it was reversed.
type Chain = (Vec<usize>, Option<usize>, bool);

struct Oriented {
    from: usize,
    to: usize,
    minlen: usize,
    reversed: bool,
    /// Ranking weight (0 for the cluster-membership constraints of `rank_with_clusters`).
    weight: f64,
}

/// Run the full layout: clusters that no edge crosses are laid out on their own
/// (with their own direction and spacing, as mermaid's ELK layout does), the
/// rest in one layered pass with their parent (see `nested`).
pub fn layout(input: &LayoutInput) -> LayoutOutput {
    nested::layout(input)
}

/// ELK's `thoroughness` default: the number of crossing-minimisation runs.
const ELK_THOROUGHNESS: usize = 7;
/// ELK's `greedySwitch.activationThreshold` default: below this many nodes the
/// greedy switch runs (where ELK runs it at all).
const ELK_GREEDY_SWITCH_THRESHOLD: usize = 40;

/// Which graph of mermaid's ELK hierarchy a layered pass lays out; under ELK it
/// sets how crossings are minimised (see `elkorder::Options`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElkGraph {
    /// The root graph: `considerModelOrder: NODES_AND_EDGES`, laid out
    /// hierarchically (`INCLUDE_CHILDREN`), so no greedy switch.
    Root,
    /// A container's own graph: ELK's defaults (no model order), with the greedy
    /// switch only where it is laid out on its own (`SEPARATE_CHILDREN`: mermaid
    /// gives that to a subgraph with its own `direction`).
    Container { separate: bool },
}

/// One layered pass over the whole input (every cluster integrated).
fn layout_flat(input: &LayoutInput) -> LayoutOutput {
    layout_flat_in(input, ElkGraph::Root, &Hierarchy::default())
}

/// What one graph of ELK's hierarchy adds to a layered pass (see `nested`).
#[derive(Debug, Clone, Default)]
struct Hierarchy {
    /// External ports: nodes (of zero size) standing for where an edge crossing
    /// the container's border meets it, `true` for one where it enters (ELK's
    /// `FIRST_SEPARATE` layer), `false` where it leaves (`LAST_SEPARATE`). They
    /// are left out of the layering, and their layers take no room.
    ext: Vec<(usize, bool)>,
    /// Per edge: fixed cross-axis offsets (from the node's centre) of its source
    /// and target ports, where it meets a laid out container's external port.
    ports: Vec<(Option<f64>, Option<f64>)>,
    /// Per edge (in the layering's orientation): where its ports at its upper
    /// and lower ends are in ELK's port lists after crossing minimisation
    /// (`elkorder::RunOrder::port_rank`).
    port_rank: Vec<(Option<usize>, Option<usize>)>,
    /// The graph's random generator after crossing minimisation, which the
    /// edge router draws from (see `elkorder::RunOrder::rng`).
    route_rng: Option<elkorder::JavaRandom>,
    /// Per node with any: its margins along the rank axis (before, after). ELK
    /// puts a container's ports for edges crossing its border
    /// (`CompoundGraphPreprocessor`) half an edge spacing outside it
    /// (`portBorderOffset`), which widens its layer.
    margins: Vec<(f64, f64)>,
    /// Per node: whether its ports keep their sides (a laid out container with
    /// external port dummies inside: `CompoundGraphPreprocessor` makes its port
    /// constraints `FIXED_SIDE`), so that an edge reversed by cycle breaking
    /// meets it on its original side (see [`InLayer`]).
    side_fixed: Vec<bool>,
    /// Per edge (empty: by the edge order): where its ports at its source and
    /// at its target were made among their node's ports, as keys to sort by. A
    /// laid out container makes its ports for edges to it as ELK imports
    /// them, and those for edges crossing its border after, in the order of
    /// their dummies inside (`CompoundGraphPreprocessor`).
    port_made: Vec<[(usize, usize); 2]>,
}

/// ELK's `InvertedPortProcessor`: where cycle breaking reverses an edge at a node
/// whose ports keep their sides, its port there faces away from the edge's
/// (reversed) direction. A long-edge dummy in the node's layer takes the edge
/// on, and the node's port is joined to the dummy by an in-layer edge, routed
/// in the channel on the port's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InLayer {
    /// The edge (whose oriented chain now starts or ends at `dummy`).
    edge: usize,
    /// The node with the port.
    node: usize,
    dummy: usize,
    /// The port is on the node's side towards the previous layer (an output
    /// port there: the chain starts at the dummy), else towards the next one
    /// (an input port there: the chain ends at the dummy).
    before: bool,
}

/// ELK's border offset of the ports it makes for edges crossing a container's
/// border: half the container's `spacing.edgeEdge` (20, as mermaid sets it).
const ELK_HIERARCHICAL_PORT_OFFSET: f64 = 10.0;

/// ELK's `spacing.edgeLabel` as mermaid sets it (`spacing.baseValue: 40`): the
/// gap between a node and an end label beside its port.
pub const ELK_END_LABEL_SPACING: f64 = 4.0;
/// The gap between an end label and its edge: the edge-label spacing plus the
/// edge's thickness (1).
const ELK_END_LABEL_EDGE_GAP: f64 = 5.0;

/// ELK's default `edge.thickness` (mermaid sets none).
const ELK_EDGE_THICKNESS: f64 = 1.0;

/// An end label beside a node side: (cross start, cross size, depth).
type EndLabel = (f64, f64, f64);

/// How deep a strip of end labels beside one side of a node is, once those that
/// overlap across are stacked away from the node (ELK's greedy
/// `RectangleStripOverlapRemover`, in the order of their cross starts). Labels
/// are (cross start, cross size, depth).
fn strip_depth(mut labels: Vec<EndLabel>) -> f64 {
    labels.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut size = 0.0f64;
    for (lo, across, depth) in labels {
        let mut over: Vec<(f64, f64)> = placed
            .iter()
            .filter(|p| p.0 < lo + across && lo < p.0 + p.1)
            .map(|p| (p.3, p.2))
            .collect();
        over.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut at = 0.0;
        for (y, h) in over {
            if at < y + h && at + depth > y {
                at = y + h;
            }
        }
        placed.push((lo, across, depth, at));
        size = size.max(at + depth);
    }
    size
}

/// `layout_flat` for one graph of the ELK hierarchy.
fn layout_flat_in(input: &LayoutInput, graph: ElkGraph, hier: &Hierarchy) -> LayoutOutput {
    let prep = prepare(input, hier);
    let (layers, route_rng) = order_flat(input, graph, &prep);
    let hier = Hierarchy {
        route_rng,
        ..hier.clone()
    };
    finish(input, graph, &hier, prep, layers)
}

/// A layered pass up to crossing minimisation: its vertices (edges split into
/// dummies, cluster borders added) on their ranks, in their initial order.
struct Prepared {
    n: usize,
    rank_sep: f64,
    is_ext: Vec<bool>,
    hidden_layers: (Option<usize>, Option<usize>),
    verts: Vec<Vertex>,
    cluster_chain: Vec<Vec<usize>>,
    links: Vec<(usize, usize, f64)>,
    chains: Vec<Option<Chain>>,
    /// The in-layer edges of inverted ports.
    inl: Vec<InLayer>,
    span: Vec<Option<(usize, usize)>>,
    borders: Vec<Vec<(usize, usize)>>,
    up: Vec<Vec<(usize, f64)>>,
    down: Vec<Vec<(usize, f64)>>,
    max_rank: usize,
    /// The initial order.
    layers: Vec<Vec<usize>>,
    /// Under ELK, per vertex: when ELK's layering put it in its layer (real
    /// nodes and label dummies; `usize::MAX` for the rest). Empty otherwise.
    created: Vec<usize>,
    /// Per edge: the rank its label moves to after crossing minimisation.
    label_switch: Vec<Option<usize>>,
}

impl Prepared {
    /// Cluster border vertices.
    fn border(&self) -> Vec<bool> {
        self.verts
            .iter()
            .map(|v| matches!(v.kind, Kind::Border { .. }))
            .collect()
    }
}

/// How ELK minimises crossings in `graph` (of `n` nodes).
fn elk_options(graph: ElkGraph, n: usize) -> elkorder::Options {
    elkorder::Options {
        model_order: graph == ElkGraph::Root,
        greedy_switch: graph == (ElkGraph::Container { separate: true })
            && n < ELK_GREEDY_SWITCH_THRESHOLD,
        thoroughness: ELK_THOROUGHNESS,
    }
}

/// Steps 1–5 of the pass (see the module documentation), up to the initial order.
fn prepare(input: &LayoutInput, hier: &Hierarchy) -> Prepared {
    let horizontal = input.direction.is_horizontal();
    let swap = |w: f64, h: f64| if horizontal { (h, w) } else { (w, h) };
    let n = input.nodes.len();

    // dagre's makeSpaceForEdgeLabels: every edge spans two ranks at half the
    // separation, labelled or not, so each edge gets a midpoint vertex.
    let rank_scale = RANK_STEP;
    let rank_sep = input.rank_sep / rank_scale as f64;

    // 1. Orientation.
    // ELK layers in its own units (see `elk_ranks`): an edge spans one layer, a
    // labelled one two, its label being a node of the layer between.
    let elk = matches!(input.routing, EdgeRouting::Orthogonal(_));
    // An edge into an external port dummy of the first layer (out of one of the
    // last) is reversed before cycle breaking (`EdgeAndLayerConstraintEdgeReverser`).
    let mut ext_first: Vec<Option<bool>> = vec![None; n];
    for &(v, first) in &hier.ext {
        ext_first[v] = Some(first);
    }
    let against: Vec<bool> = input
        .edges
        .iter()
        .map(|e| ext_first[e.to] == Some(true) || ext_first[e.from] == Some(false))
        .collect();
    // Where each edge's port at its (search) source was made.
    let source_made = |flip: bool| -> Vec<(usize, usize)> {
        (0..input.edges.len())
            .map(|i| {
                let side = usize::from(flip && against[i]);
                hier.port_made.get(i).map_or((0, i), |m| m[side])
            })
            .collect()
    };
    let mut oriented = if against.iter().any(|&a| a) {
        let flipped: Vec<LayoutEdge> = input
            .edges
            .iter()
            .zip(&against)
            .map(|(e, &a)| {
                let mut e = e.clone();
                if a {
                    std::mem::swap(&mut e.from, &mut e.to);
                }
                e
            })
            .collect();
        let mut o = orient_edges(
            n,
            &flipped,
            if elk { 1 } else { rank_scale },
            elk,
            &source_made(true),
        );
        for (o, _) in o.iter_mut().zip(&against).filter(|(_, a)| **a) {
            if let Some(o) = o.as_mut() {
                o.reversed = !o.reversed;
            }
        }
        o
    } else {
        orient_edges(
            n,
            &input.edges,
            if elk { 1 } else { rank_scale },
            elk,
            &source_made(false),
        )
    };
    if elk {
        for (o, e) in oriented.iter_mut().zip(&input.edges) {
            if let Some(o) = o.as_mut().filter(|_| e.label.is_some()) {
                o.minlen = o.minlen.max(2);
            }
        }
    }

    // 2. Ranking.
    let mut is_ext = vec![false; n];
    for &(v, _) in &hier.ext {
        is_ext[v] = true;
    }
    // External ports and their edges are hidden from the layering.
    let hidden: Vec<Option<Oriented>> = oriented
        .iter_mut()
        .map(|o| {
            if o.as_ref().is_some_and(|o| is_ext[o.from] || is_ext[o.to]) {
                o.take()
            } else {
                None
            }
        })
        .collect();
    let (mut ranks, mut layered, mut created, label_layer) =
        rank_with_clusters(n, &mut oriented, input, (&is_ext, &hidden), &hier.port_made);
    if elk && !hier.ext.is_empty() {
        let moved = pin_ext_neighbours(
            (&mut ranks, &mut layered, &created),
            &label_layer,
            input,
            &oriented,
            &hidden,
            &is_ext,
            hier,
        );
        // `LayerConstraintPostprocessor` appends what it moves to its layer.
        for (k, v) in moved.into_iter().enumerate() {
            if let Some(c) = created.get_mut(v) {
                *c = usize::MAX / 2 + k;
            }
        }
    }
    for (o, h) in oriented.iter_mut().zip(hidden) {
        if h.is_some() {
            *o = h;
        }
    }
    // Then they take a layer of their own before (or after) all others.
    let mut hidden_layers: (Option<usize>, Option<usize>) = (None, None);
    if !hier.ext.is_empty() {
        let shift = usize::from(hier.ext.iter().any(|&(_, first)| first));
        for v in (0..n).filter(|&v| !is_ext[v]) {
            ranks[v] += shift;
            layered[v] += shift;
        }
        let last = (0..n)
            .filter(|&v| !is_ext[v])
            .map(|v| ranks[v])
            .max()
            .unwrap_or(0)
            + 1;
        for &(v, first) in &hier.ext {
            ranks[v] = if first { 0 } else { last };
            layered[v] = ranks[v];
        }
        hidden_layers = (
            (shift == 1).then_some(0),
            hier.ext.iter().any(|e| !e.1).then_some(last),
        );
    }
    // Under ELK, per edge: the rank of its label (the layer its label dummy took),
    // and where `LabelDummySwitcher` moves it after crossing minimisation.
    let mut label_ranks: Vec<Option<usize>> = vec![None; input.edges.len()];
    let mut label_switch: Vec<Option<usize>> = vec![None; input.edges.len()];
    if elk {
        let to_rank = elk_ranks(&ranks);
        let ext_shift = usize::from(hier.ext.iter().any(|&(_, first)| first));
        for (ei, o) in oriented.iter().enumerate() {
            if let Some(o) = o.as_ref().filter(|_| input.edges[ei].label.is_some()) {
                // The switcher's median layer: half-way along the edge's dummies,
                // on the balanced layers. A reversed edge at a side-fixed node
                // has a dummy in the node's own layer too (`InvertedPortProcessor`).
                let inverted =
                    |v: usize| o.reversed && hier.side_fixed.get(v).copied().unwrap_or(false);
                let (lu, lv) = (layered[o.from], layered[o.to]);
                let first = if inverted(o.from) { lu } else { lu + 1 };
                let last = if inverted(o.to) {
                    lv
                } else {
                    lv.saturating_sub(1)
                };
                let median = if last >= first {
                    to_rank(first + (last - first) / 2)
                } else {
                    to_rank(lu + lv.saturating_sub(lu) / 2)
                };
                match label_layer.get(ei).copied().flatten() {
                    Some(l) => {
                        label_ranks[ei] = Some(to_rank(l + ext_shift));
                        label_switch[ei] = Some(median);
                    }
                    None => label_ranks[ei] = Some(median),
                }
            }
        }
        ranks = ranks.iter().map(|&l| to_rank(l)).collect();
        hidden_layers = (hidden_layers.0.map(&to_rank), hidden_layers.1.map(&to_rank));
    }

    // 3. Vertices + chains.
    let mut verts: Vec<Vertex> = input
        .nodes
        .iter()
        .zip(&ranks)
        .map(|(nd, &r)| {
            let (w, h) = swap(nd.width, nd.height);
            Vertex {
                kind: Kind::Real,
                rank: r,
                w,
                h,
                cluster: nd.cluster,
            }
        })
        .collect();
    let cluster_chain = cluster_chains(&input.clusters);
    let mut links: Vec<(usize, usize, f64)> = Vec::new();
    // Per edge: chain in oriented order, label vertex.
    let mut chains: Vec<Option<Chain>> = vec![None; input.edges.len()];
    let mut inl: Vec<InLayer> = Vec::new();
    // Under ELK an edge into (out of) a cluster enters (leaves) it through a port on
    // its border and runs inside it over the ranks the cluster spans, rather than
    // outside it until the last rank: it would otherwise cross the cluster's side,
    // and so its title in LR / RL, in the channel before its end.
    let orthogonal = matches!(input.routing, EdgeRouting::Orthogonal(_));
    let rank_span: Vec<Option<(usize, usize)>> = if orthogonal {
        let mut span = vec![None; input.clusters.len()];
        for (v, nd) in input.nodes.iter().enumerate() {
            for &c in nd.cluster.map_or(&[][..], |c| &cluster_chain[c][..]) {
                let r = ranks[v];
                span[c] = Some(match span[c] {
                    Some((lo, hi)) => (r.min(lo), r.max(hi)),
                    None => (r, r),
                });
            }
        }
        span
    } else {
        Vec::new()
    };
    for (ei, o) in oriented.iter().enumerate() {
        let Some(o) = o else { continue };
        let (ru, rv) = (ranks[o.from], ranks[o.to]);
        let span = rv - ru;
        let label = input.edges[ei].label.map(|(w, h)| swap(w, h));
        let label_rank = match label_ranks[ei] {
            Some(r) if span >= 2 => r.clamp(ru + 1, rv - 1),
            _ => ru + span / 2,
        };
        let dummy_cluster = lca(
            &cluster_chain,
            input.nodes[o.from].cluster,
            input.nodes[o.to].cluster,
        );
        let mut chain = vec![o.from];
        let mut label_vertex = None;
        // The deepest cluster of either end (below their common one) spanning `r`;
        // not a cluster the edge attaches to as a whole: it is cut at that one's
        // border, and its label stays outside.
        let e = &input.edges[ei];
        let (whole_from, whole_to) = if o.reversed {
            (e.to_cluster, e.from_cluster)
        } else {
            (e.from_cluster, e.to_cluster)
        };
        let cluster_at = |r: usize| -> Option<usize> {
            let base = dummy_cluster.map_or(0, |c| cluster_chain[c].len());
            [(o.from, whole_from), (o.to, whole_to)]
                .into_iter()
                .filter_map(|(end, whole)| Some((input.nodes[end].cluster?, whole)))
                .filter_map(|(c, whole)| {
                    cluster_chain[c][base..]
                        .iter()
                        .take_while(|&&a| {
                            Some(a) != whole
                                && rank_span[a].is_some_and(|(lo, hi)| lo <= r && r <= hi)
                        })
                        .last()
                        .copied()
                })
                .max_by_key(|&a| cluster_chain[a].len())
                .or(dummy_cluster)
        };
        for r in ru + 1..rv {
            let is_label = label.is_some() && r == label_rank;
            let (w, h) = if is_label {
                label.unwrap_or((0.0, 0.0))
            } else {
                (0.0, 0.0)
            };
            verts.push(Vertex {
                kind: Kind::Dummy { labeled: is_label },
                rank: r,
                w,
                h,
                cluster: if orthogonal {
                    cluster_at(r)
                } else {
                    dummy_cluster
                },
            });
            let id = verts.len() - 1;
            if is_label {
                label_vertex = Some(id);
            }
            chain.push(id);
        }
        chain.push(o.to);
        // ELK keeps a side-fixed node's port on its side when the edge is
        // reversed: a dummy beside the node takes the edge on.
        if elk && o.reversed && o.from != o.to {
            let last = chain.len() - 1;
            for (at, before) in [(0, true), (last, false)] {
                let node = chain[at];
                if !hier.side_fixed.get(node).copied().unwrap_or(false) {
                    continue;
                }
                verts.push(Vertex {
                    kind: Kind::Dummy { labeled: false },
                    rank: verts[node].rank,
                    w: 0.0,
                    h: 0.0,
                    cluster: verts[node].cluster,
                });
                let dummy = verts.len() - 1;
                chain[at] = dummy;
                inl.push(InLayer {
                    edge: ei,
                    node,
                    dummy,
                    before,
                });
            }
        }
        for pair in chain.windows(2) {
            let dummies = pair.iter().filter(|&&v| v >= n).count();
            links.push((pair[0], pair[1], [1.0, 2.0, 8.0][dummies]));
        }
        chains[ei] = Some((chain, label_vertex, o.reversed));
    }

    let created: Vec<usize> = if created.is_empty() {
        Vec::new()
    } else {
        let mut by_vertex = vec![usize::MAX; verts.len()];
        by_vertex[..n].copy_from_slice(&created[..n]);
        for (ei, c) in chains.iter().enumerate() {
            if let Some((_, Some(lv), _)) = c {
                by_vertex[*lv] = created[n + ei];
            }
        }
        by_vertex
    };

    // 4. Cluster spans and border vertices.
    let nc = input.clusters.len();
    let mut span: Vec<Option<(usize, usize)>> = vec![None; nc];
    for (v, nd) in input.nodes.iter().enumerate() {
        if let Some(c) = nd.cluster {
            for &a in &cluster_chain[c] {
                let r = ranks[v];
                span[a] = Some(match span[a] {
                    Some((lo, hi)) => (lo.min(r), hi.max(r)),
                    None => (r, r),
                });
            }
        }
    }
    let mut borders: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nc]; // (left, right) per rank
    for c in 0..nc {
        if let Some((lo, hi)) = span[c] {
            for r in lo..=hi {
                verts.push(Vertex {
                    kind: Kind::Border {
                        cluster: c,
                        side: Side::Left,
                    },
                    rank: r,
                    w: 0.0,
                    h: 0.0,
                    cluster: Some(c),
                });
                verts.push(Vertex {
                    kind: Kind::Border {
                        cluster: c,
                        side: Side::Right,
                    },
                    rank: r,
                    w: 0.0,
                    h: 0.0,
                    cluster: Some(c),
                });
                borders[c].push((verts.len() - 2, verts.len() - 1));
            }
        }
    }

    let nv = verts.len();
    let mut up: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    let mut down: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nv];
    for &(a, b, w) in &links {
        down[a].push((b, w));
        up[b].push((a, w));
    }
    let max_rank = verts.iter().map(|v| v.rank).max().unwrap_or(0);

    // 5. Ordering.
    let mut layers = initial_order(&verts, n, &down, max_rank);
    if elk {
        // ELK (`considerModelOrder: NODES_AND_EDGES`) starts crossing minimisation
        // from the model order: real nodes in input order, an edge's dummies after
        // the node the edge leaves, in edge order.
        let mut key: Vec<(usize, usize)> = (0..verts.len()).map(|v| (v, 0)).collect();
        let mut head: Vec<usize> = (0..verts.len()).collect();
        for l in &inl {
            head[l.dummy] = l.node;
        }
        for (ei, c) in chains.iter().enumerate() {
            if let Some((chain, _, _)) = c {
                for &d in &chain[1..chain.len() - 1] {
                    key[d] = (head[chain[0]], 1 + ei);
                }
            }
        }
        for layer in layers.iter_mut() {
            layer.sort_by_key(|&v| key[v]);
        }
    }
    let ctx = OrderCtx {
        verts: &verts,
        chains: &cluster_chain,
    };
    let mut before = Vec::new();
    for layer in layers.iter_mut() {
        let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
        *layer = ctx.arrange(layer, &keys, &mut before);
    }
    Prepared {
        n,
        rank_sep,
        is_ext,
        hidden_layers,
        verts,
        cluster_chain,
        links,
        chains,
        inl,
        span,
        borders,
        up,
        down,
        max_rank,
        layers,
        created,
        label_switch,
    }
}

/// ELK's `LayerConstraintPreprocessor` hides the external port dummies with their
/// edges before layering; a node left without any edge then takes the layer
/// constraint of the dummies it was joined to: `FIRST` if it only met ports where
/// edges enter, `LAST` if only ports where they leave (any other mix: none). It is
/// layered on its own, on the first layer, and `LayerConstraintPostprocessor` then
/// moves a `LAST` node to the last layer (dropping a layer left empty) and puts a
/// `FIRST` one back on the first: either goes to the end of its layer's list.
/// Returns the nodes it moved, in the order it moved them (layer by layer, in
/// the order of `created`).
fn pin_ext_neighbours(
    (ranks, layered, created): (&mut [usize], &mut [usize], &[usize]),
    label_layer: &[Option<usize>],
    input: &LayoutInput,
    oriented: &[Option<Oriented>],
    hidden: &[Option<Oriented>],
    is_ext: &[bool],
    hier: &Hierarchy,
) -> Vec<usize> {
    let mut moved = Vec::new();
    let n = ranks.len();
    let entering: Vec<Option<bool>> = {
        let mut k = vec![None; n];
        for &(v, first) in &hier.ext {
            k[v] = Some(first);
        }
        k
    };
    // Per node: (has a visible edge, met an entering port, met a leaving port).
    let mut seen = vec![(false, false, false); n];
    for (ei, e) in input.edges.iter().enumerate() {
        if e.from == e.to {
            seen[e.from].0 = true;
        } else if oriented[ei].is_some() {
            seen[e.from].0 = true;
            seen[e.to].0 = true;
        } else if hidden[ei].is_some() {
            for (a, b) in [(e.from, e.to), (e.to, e.from)] {
                if let (false, Some(first)) = (is_ext[a], entering[b]) {
                    // Its label dummy stays in the layering, tied to `a`.
                    if e.label.is_some() {
                        seen[a].0 = true;
                    }
                    if first {
                        seen[a].1 = true;
                    } else {
                        seen[a].2 = true;
                    }
                }
            }
        }
    }
    let real: Vec<usize> = (0..n).filter(|&v| !is_ext[v]).collect();
    // The last layer may hold just a label dummy (of an edge to an external
    // port, tied to its other end only).
    let Some(mut last) = real
        .iter()
        .map(|&v| ranks[v])
        .chain(label_layer.iter().flatten().copied())
        .max()
    else {
        return moved;
    };
    let mut pinned: Vec<usize> = real
        .iter()
        .copied()
        .filter(|&v| matches!(seen[v], (false, true, false) | (false, false, true)))
        .collect();
    pinned.sort_by_key(|&v| (ranks[v], created.get(v).copied().unwrap_or(v)));
    for v in pinned {
        moved.push(v);
        if seen[v].1 || ranks[v] == last {
            continue;
        }
        let old = ranks[v];
        ranks[v] = last;
        layered[v] = last;
        // The layer it left may now be empty (a label dummy's layer lies between
        // the ends of a labelled edge).
        let used = real.iter().any(|&w| ranks[w] == old)
            || oriented.iter().zip(&input.edges).any(|(o, e)| {
                o.as_ref()
                    .is_some_and(|o| e.label.is_some() && ranks[o.from] < old && old < ranks[o.to])
            });
        if !used {
            for &w in &real {
                if ranks[w] > old {
                    ranks[w] -= 1;
                    layered[w] = layered[w].saturating_sub(1);
                }
            }
            last -= 1;
        }
    }
    moved
}

/// ELK's `LabelDummySwitcher`, after crossing minimisation: a label dummy moves
/// to the median layer of its edge, swapping places with the edge's long-edge
/// dummy there (which takes the label dummy's place in its layer).
fn switch_labels(prep: &mut Prepared) {
    for (ei, target) in prep.label_switch.iter().enumerate() {
        let (Some(target), Some((chain, Some(lv), _))) = (*target, prep.chains[ei].as_mut()) else {
            continue;
        };
        let lv_ = *lv;
        if prep.verts[lv_].rank == target {
            continue;
        }
        // Any dummy of the chain: an inverted port's stands at its very end.
        let n = prep.n;
        let Some(&d) = chain
            .iter()
            .find(|&&d| d >= n && prep.verts[d].rank == target && d != lv_)
        else {
            continue;
        };
        let (a, b) = (prep.verts[lv_].clone(), prep.verts[d].clone());
        prep.verts[d] = Vertex {
            rank: b.rank,
            cluster: b.cluster,
            ..a
        };
        prep.verts[lv_] = Vertex {
            rank: a.rank,
            cluster: a.cluster,
            ..b
        };
        // Swapped with an inverted port's dummy (in its node's layer), the
        // label leaves a long-edge dummy behind, which keeps the label's
        // layer an ELK layer: it stands in as a label of no size.
        if prep.inl.iter().any(|l| l.dummy == d) {
            prep.verts[lv_].kind = Kind::Dummy { labeled: true };
        }
        *lv = d;
    }
}

/// Crossing minimisation of one pass on its own; under ELK also the random
/// generator it leaves.
fn order_flat(
    input: &LayoutInput,
    graph: ElkGraph,
    prep: &Prepared,
) -> (Vec<Vec<usize>>, Option<elkorder::JavaRandom>) {
    let Prepared {
        n,
        verts,
        cluster_chain,
        chains,
        up,
        down,
        layers,
        ..
    } = prep;
    let (n, nv) = (*n, verts.len());
    let ctx = OrderCtx {
        verts,
        chains: cluster_chain,
    };
    let layers = layers.clone();
    let elk = matches!(input.routing, EdgeRouting::Orthogonal(_));
    if elk {
        let border = prep.border();
        let (layers, rng) = elk_order(
            layers,
            (verts, &prep.created),
            n,
            chains,
            &prep.inl,
            &border,
            &ctx,
            elk_options(graph, n),
        );
        (layers, Some(rng))
    } else {
        (dagre_order(layers, &ctx, up, down, nv), None)
    }
}

/// Steps 6–8 of the pass: coordinates and routes for the ordered `layers`. The
/// real nodes take their sizes from `input` (a container's box may have been
/// sized after its pass was prepared).
fn finish(
    input: &LayoutInput,
    graph: ElkGraph,
    hier: &Hierarchy,
    prep: Prepared,
    layers: Vec<Vec<usize>>,
) -> LayoutOutput {
    finish_rng(input, graph, hier, prep, layers).0
}

/// `finish`, also giving back the graph's random generator as the edge router
/// leaves it (the next component of the same ELK graph draws on from there).
fn finish_rng(
    input: &LayoutInput,
    graph: ElkGraph,
    hier: &Hierarchy,
    mut prep: Prepared,
    layers: Vec<Vec<usize>>,
) -> (LayoutOutput, elkorder::JavaRandom) {
    switch_labels(&mut prep);
    let Prepared {
        n,
        rank_sep,
        is_ext,
        hidden_layers,
        mut verts,
        cluster_chain,
        links,
        chains,
        inl,
        span,
        borders,
        max_rank,
        ..
    } = prep;
    let horizontal = input.direction.is_horizontal();
    let swap = |w: f64, h: f64| if horizontal { (h, w) } else { (w, h) };
    let elk = matches!(input.routing, EdgeRouting::Orthogonal(_));
    let nc = input.clusters.len();
    for (v, nd) in input.nodes.iter().enumerate() {
        let (w, h) = swap(nd.width, nd.height);
        verts[v].w = w;
        verts[v].h = h;
    }

    // 6. Cross-axis coordinates.
    let pad = input.cluster_padding;
    let title_cross_low = horizontal;
    let pad_x_low = |c: usize| {
        pad + if title_cross_low {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    // Orthogonal self-loops stand off their node's low cross-axis side, each beyond
    // the ones before it (in model order) and their labels; the node keeps that much
    // room free. As ELK places them (`SelfLoopPortRestorer`, `SelfLoopRouter`): the
    // 2k ports of a node's k loops are spread evenly over the side inside its
    // `spacing.portsSurrounding`, the first loop on the middle pair and each later one
    // on the pair around it; the first leg stands one edge-node spacing (20) off the
    // side, each later one 20 beyond the previous leg or beyond its label, which
    // stands 4px (`spacing.edgeLabel` 2, twice) outside its leg.
    let mut loop_out = vec![0.0f64; input.edges.len()];
    let mut loop_half = vec![0.0f64; input.edges.len()];
    let mut loop_reach = vec![0.0f64; n];
    if matches!(input.routing, EdgeRouting::Orthogonal(_)) {
        let mut loops = vec![0usize; n];
        for e in input.edges.iter().filter(|e| e.from == e.to) {
            loops[e.from] += 1;
        }
        let mut seen = vec![0usize; n];
        for (ei, e) in input.edges.iter().enumerate() {
            if e.from != e.to {
                continue;
            }
            let v = e.from;
            let gap = ((verts[v].h - 2.0 * PORTS_SURROUNDING) / (2 * loops[v] + 1) as f64).max(0.0);
            loop_half[ei] = gap * (2 * seen[v] + 1) as f64 / 2.0;
            seen[v] += 1;
            loop_out[ei] = loop_reach[v] + SELF_LOOP_EXT;
            loop_reach[v] = loop_out[ei]
                + e.label
                    .map_or(0.0, |(lw, lh)| SELF_LOOP_LABEL_GAP + swap(lw, lh).0);
        }
    }
    let gap = |a: usize, b: usize| -> f64 {
        let half = |v: &Vertex| match v.kind {
            Kind::Real | Kind::Dummy { labeled: true } => v.w / 2.0,
            _ => 0.0,
        };
        let spacing = |v: &Vertex| match v.kind {
            Kind::Dummy { labeled: false } => input.edge_sep,
            _ => input.node_sep,
        };
        let (va, vb) = (&verts[a], &verts[b]);
        let a_in = matches!(
            va.kind,
            Kind::Border {
                side: Side::Left,
                ..
            }
        );
        let b_in = matches!(
            vb.kind,
            Kind::Border {
                side: Side::Right,
                ..
            }
        );
        let g = if a_in || b_in {
            let mut g = half(va) + half(vb);
            if let Kind::Border {
                cluster,
                side: Side::Left,
            } = va.kind
            {
                g += pad_x_low(cluster);
            }
            if let Kind::Border {
                side: Side::Right, ..
            } = vb.kind
            {
                g += pad;
            }
            g
        } else {
            half(va) + half(vb) + spacing(va) / 2.0 + spacing(vb) / 2.0
        };
        let loops = loop_reach.get(b).copied().unwrap_or(0.0);
        if loops > 0.0 {
            g.max(half(va) + half(vb) + loops + SELF_LOOP_CLEARANCE)
        } else {
            g
        }
    };
    // ELK's own node placement where no cluster is integrated into this pass.
    let elk_placed = (elk && borders.iter().all(|b| b.is_empty())).then(|| {
        // A label dummy's cross size and the offset of its ports, which carry the
        // edge: in a vertical layout the label's width and half its width rounded
        // up; in a horizontal one the label's height plus the edge's thickness and
        // half of that, the height rounded up (`LabelDummyInserter`).
        let label_across: Vec<(f64, f64)> = verts
            .iter()
            .map(|v| match v.kind {
                Kind::Dummy { labeled: true } if horizontal => (
                    v.w + ELK_EDGE_THICKNESS,
                    (v.w.ceil() + ELK_EDGE_THICKNESS) / 2.0,
                ),
                Kind::Dummy { labeled: true } => (v.w, v.w.ceil() / 2.0),
                _ => (0.0, 0.0),
            })
            .collect();
        // The cross size of each edge's end labels (at its source, its target).
        let end_labels: Vec<[Option<f64>; 2]> = input
            .edges
            .iter()
            .map(|e| e.terminals.map(|t| t.map(|(w, h)| swap(w, h).0)))
            .collect();
        elk_place(
            &layers,
            &verts,
            n,
            &chains,
            &loop_reach,
            &label_across,
            &is_ext,
            &hier.ports,
            &hier.port_rank,
            &inl,
            &end_labels,
            elk_spacing(graph, input.node_sep),
            elk_port_inset(graph),
        )
    });
    let (x, ports) = match elk_placed {
        Some((x, start, end)) => (x, Some((start, end))),
        None => (assign_x(&layers, &verts, &links, &borders, &gap), None),
    };
    let mut route_rng = hier
        .route_rng
        .clone()
        .unwrap_or_else(|| elkorder::JavaRandom::new(1));
    let plan = match input.routing {
        EdgeRouting::Orthogonal(s) => Some(ortho_plan(
            &verts,
            n,
            &chains,
            &x,
            &layers,
            &gap,
            s,
            ports,
            &inl,
            &hier.ports,
            &hier.port_rank,
            &mut route_rng,
        )),
        EdgeRouting::Splines => None,
    };

    // 7. Rank-axis coordinates.
    let pad_rank_low = |c: usize| {
        pad + if input.direction == Direction::TopBottom {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    let pad_rank_high = |c: usize| {
        pad + if input.direction == Direction::BottomTop {
            input.clusters[c].title_height
        } else {
            0.0
        }
    };
    // Margins along the rank axis: the containers' (see `Hierarchy::margins`),
    // and under ELK the room its end labels take beside their ports
    // (`EndLabelPreprocessor`: the edge-label spacing, then the label).
    let mut margins: Vec<(f64, f64)> = (0..verts.len())
        .map(|v| hier.margins.get(v).copied().unwrap_or((0.0, 0.0)))
        .collect();
    if let Some(p) = &plan {
        // Per node side (node, after): the labels beside it, as (cross start,
        // cross size, depth along the rank axis).
        let mut sides: std::collections::BTreeMap<(usize, bool), Vec<EndLabel>> =
            std::collections::BTreeMap::new();
        for (ei, e) in input.edges.iter().enumerate() {
            let Some((chain, _, _)) = &chains[ei] else {
                continue;
            };
            if e.from == e.to {
                continue;
            }
            let stops = &p.stops[ei];
            let (Some(&(_, first)), Some(&(_, last))) = (stops.first(), stops.last()) else {
                continue;
            };
            for (v, size) in [(e.from, e.terminals[0]), (e.to, e.terminals[1])] {
                let Some((w, h)) = size else { continue };
                let (across, along) = swap(w, h);
                // The port faces the next layer where the route leaves downwards.
                let (after, port) = if chain.first() == Some(&v) {
                    (true, first)
                } else if chain.last() == Some(&v) {
                    (false, last)
                } else {
                    continue;
                };
                if v < n {
                    sides.entry((v, after)).or_default().push((
                        port + ELK_END_LABEL_EDGE_GAP,
                        across,
                        along,
                    ));
                }
            }
        }
        for ((v, after), labels) in sides {
            // (Stacked only in a vertical layout, see `place_terminal_labels`.)
            let depth = ELK_END_LABEL_SPACING
                + if horizontal {
                    labels.iter().map(|l| l.2).fold(0.0, f64::max)
                } else {
                    strip_depth(labels)
                };
            let m = &mut margins[v];
            if after {
                m.1 += depth;
            } else {
                m.0 += depth;
            }
        }
    }
    let margin = |v: usize| margins[v];
    let mut rank_h = vec![0.0f64; max_rank + 1];
    // Per layer: the largest margin before and after its nodes.
    let mut rank_margin = vec![(0.0f64, 0.0f64); max_rank + 1];
    for (v, vert) in verts.iter().enumerate() {
        let (lo, hi) = margin(v);
        rank_h[vert.rank] = rank_h[vert.rank].max(vert.h + lo + hi);
        let m = &mut rank_margin[vert.rank];
        *m = (m.0.max(lo), m.1.max(hi));
    }
    if plan.is_some() {
        // An orthogonal self-loop's label stands beside its node, centred on it along
        // the rank axis: its layer must be as deep as the label.
        for e in &input.edges {
            if let (true, Some((lw, lh))) = (e.from == e.to, e.label) {
                let r = verts[e.from].rank;
                rank_h[r] = rank_h[r].max(swap(lw, lh).1);
            }
        }
    }
    let mut start_pad = vec![0.0f64; max_rank + 1];
    let mut end_pad = vec![0.0f64; max_rank + 1];
    for (c, s) in span.iter().enumerate() {
        if let Some((lo, hi)) = *s {
            start_pad[lo] += pad_rank_low(c);
            end_pad[hi] += pad_rank_high(c);
        }
    }
    // Orthogonal routes: widen a gap whose slots do not fit (ELK sizes the space
    // between two layers from the slots routed through it).
    // ELK gives edge labels a layer of their own with the full layer gap on both
    // sides; the split around the label vertex leaves only half the gap each side.
    // A label in a layer of nodes (its edge spans more layers) needs no more room.
    let mut label_gap = vec![0.0f64; max_rank + 1];
    if plan.is_some() {
        let mut has_node = vec![false; max_rank + 1];
        for v in verts.iter().filter(|v| v.kind == Kind::Real) {
            has_node[v.rank] = true;
        }
        for v in &verts {
            if matches!(v.kind, Kind::Dummy { labeled: true }) && !has_node[v.rank] {
                label_gap[v.rank] = rank_sep;
            }
        }
    }
    // The layers of external ports take no room: only their edges' slots do.
    // A label's layer between one and the next layer of nodes is a layer of
    // its own (also when `LabelDummySwitcher` has moved the label out of it):
    // only its side towards the ports takes no room.
    let label_rank = |r: usize| label_gap.get(r).is_some_and(|&g| g > 0.0);
    let beside_hidden = |r: usize| {
        hidden_layers
            .0
            .is_some_and(|h| r > h && r <= h + RANK_STEP && !(r > h + 1 && label_rank(r - 1)))
            || hidden_layers
                .1
                .is_some_and(|h| r <= h && r + RANK_STEP > h && !(r < h && label_rank(r)))
    };
    let gap_before = |r: usize| {
        if beside_hidden(r) {
            0.0
        } else {
            rank_sep + label_gap[r - 1] + label_gap[r]
        }
    };
    let mut extra = vec![0.0f64; max_rank + 1];
    if let Some(p) = &plan {
        for (ra, ch) in p.channels.iter().enumerate() {
            let Some((rb, slots)) = *ch else { continue };
            let free: f64 = (ra + 1..rb)
                .map(|r| rank_h[r] + start_pad[r] + end_pad[r])
                .sum();
            let have = (ra + 1..=rb).map(gap_before).sum::<f64>() + free;
            extra[rb] = extra[rb].max(p.spacing.required_gap(slots) - have);
        }
    }
    let outer_room = plan.as_ref().map_or((0.0, 0.0), OrthoPlan::outer_room);
    let mut rank_y = vec![0.0f64; max_rank + 1];
    for r in 0..=max_rank {
        rank_y[r] = if r == 0 {
            outer_room.0 + start_pad[0] + rank_h[0] / 2.0
        } else {
            rank_y[r - 1]
                + rank_h[r - 1] / 2.0
                + end_pad[r - 1]
                + gap_before(r)
                + extra[r]
                + start_pad[r]
                + rank_h[r] / 2.0
        };
    }

    // Cluster rectangles in layout space.
    let mut cluster_rects = vec![Rect::default(); nc];
    for c in 0..nc {
        let Some((lo, hi)) = span[c] else { continue };
        let x0 = borders[c]
            .iter()
            .map(|&(l, _)| x[l])
            .fold(f64::INFINITY, f64::min);
        let x1 = borders[c]
            .iter()
            .map(|&(_, r)| x[r])
            .fold(f64::NEG_INFINITY, f64::max);
        let same_start: f64 = (0..nc)
            .filter(|&d| cluster_chain[d].contains(&c) && span[d].map(|s| s.0) == Some(lo))
            .map(&pad_rank_low)
            .sum();
        let same_end: f64 = (0..nc)
            .filter(|&d| cluster_chain[d].contains(&c) && span[d].map(|s| s.1) == Some(hi))
            .map(&pad_rank_high)
            .sum();
        let y0 = rank_y[lo] - rank_h[lo] / 2.0 - same_start;
        let y1 = rank_y[hi] + rank_h[hi] / 2.0 + same_end;
        cluster_rects[c] = Rect::new(x0, y0, x1 - x0, y1 - y0);
    }

    // 8. Transform to output space.
    let extent_y = rank_y.last().copied().unwrap_or(0.0)
        + rank_h.last().copied().unwrap_or(0.0) / 2.0
        + end_pad.last().copied().unwrap_or(0.0)
        + outer_room.1;
    let to_out = |p: Point| -> Point {
        match input.direction {
            Direction::TopBottom => p,
            Direction::BottomTop => Point::new(p.x, extent_y - p.y),
            Direction::LeftRight => Point::new(p.y, p.x),
            Direction::RightLeft => Point::new(extent_y - p.y, p.x),
        }
    };
    let rect_out = |r: Rect| -> Rect {
        let a = to_out(Point::new(r.x, r.y));
        let b = to_out(Point::new(r.right(), r.bottom()));
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        )
    };
    // ELK aligns a node narrower than its layer (along the flow) by its edges
    // (`LGraphUtil.placeNodesHorizontally`, `alignment: AUTOMATIC`): towards the
    // side most of them leave from, as the share of outgoing ones.
    let mut vy: Vec<f64> = verts.iter().map(|v| rank_y[v.rank]).collect();
    if plan.is_some() {
        let mut ins = vec![0usize; n];
        let mut outs = vec![0usize; n];
        for (chain, _, _) in chains.iter().flatten() {
            if chain[0] < n {
                outs[chain[0]] += 1;
            }
            if chain[chain.len() - 1] < n {
                ins[chain[chain.len() - 1]] += 1;
            }
        }
        // An inverted port: an output on the previous layer's side, an input
        // on the next one's.
        for l in &inl {
            if l.before {
                outs[l.node] += 1;
            } else {
                ins[l.node] += 1;
            }
        }
        for e in input.edges.iter().filter(|e| e.from == e.to) {
            outs[e.from] += 1;
            ins[e.from] += 1;
        }
        for v in 0..n {
            let total = ins[v] + outs[v];
            let ratio = if total > 0 {
                outs[v] as f64 / total as f64
            } else {
                0.5
            };
            // Within the layer, towards the side its margins allow.
            let r = verts[v].rank;
            let (size, h) = (rank_h[r], verts[v].h);
            let (lo, hi) = margin(v);
            let (max_lo, max_hi) = rank_margin[r];
            let mut at = (size - h) * ratio;
            if ratio > 0.5 {
                at -= max_hi * 2.0 * (ratio - 0.5);
            } else if ratio < 0.5 {
                at += max_lo * 2.0 * (0.5 - ratio);
            }
            let at = at.min(size - hi - h).max(lo);
            vy[v] = rank_y[r] - size / 2.0 + at + h / 2.0;
        }
    }
    let pos = |v: usize| to_out(Point::new(x[v], vy[v]));

    let nodes: Vec<Point> = (0..n).map(pos).collect();
    let edges = input
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| {
            if e.from == e.to {
                if plan.is_some() {
                    // ELK: a loop on the node's low cross-axis side (west in TB,
                    // north in LR).
                    let v = e.from;
                    let (cx, cy) = (x[v], vy[v]);
                    let (side, half) = (cx - verts[v].w / 2.0, loop_half[ei]);
                    let out = side - loop_out[ei];
                    let points = [
                        Point::new(side, cy - half),
                        Point::new(out, cy - half),
                        Point::new(out, cy + half),
                        Point::new(side, cy + half),
                    ]
                    .map(to_out)
                    .to_vec();
                    let label = e.label.map(|(lw, lh)| {
                        let lw = swap(lw, lh).0;
                        to_out(Point::new(out - lw / 2.0 - SELF_LOOP_LABEL_GAP, cy))
                    });
                    return EdgeRoute { points, label };
                }
                return self_loop(nodes[e.from], &input.nodes[e.from], e.label);
            }
            let Some((chain, label_v, reversed)) = &chains[ei] else {
                return EdgeRoute {
                    points: vec![nodes[e.from], nodes[e.to]],
                    label: None,
                };
            };
            if let Some(p) = &plan {
                let mut points: Vec<Point> = p
                    .route(ei, &verts, &rank_y, &vy, &rank_h, &start_pad, &end_pad)
                    .into_iter()
                    .map(to_out)
                    .collect();
                if *reversed {
                    points.reverse();
                }
                let label = label_v
                    .map(|v| to_out(Point::new(p.xs[v], rank_y[verts[v].rank])))
                    .map(|c| elk_label_centre(c, e.label, horizontal))
                    .or_else(|| {
                        e.label
                            .map(|_| points[0].lerp(points[points.len() - 1], 0.5))
                    });
                return EdgeRoute { points, label };
            }
            let mut points: Vec<Point> = chain.iter().map(|&v| pos(v)).collect();
            if *reversed {
                points.reverse();
            }
            let label = label_v.map(pos).or_else(|| {
                e.label
                    .map(|_| points[0].lerp(points[points.len() - 1], 0.5))
            });
            EdgeRoute { points, label }
        })
        .collect();
    let clusters = cluster_rects
        .into_iter()
        .enumerate()
        .map(|(c, r)| if span[c].is_some() { rect_out(r) } else { r })
        .collect();
    (
        LayoutOutput {
            nodes,
            edges,
            clusters,
            cluster_directions: vec![input.direction; nc],
        },
        route_rng,
    )
}

/// ELK's crossing minimisation (see `elkorder`) on ELK's own layers: the ranks of
/// real nodes and of label-only layers. The ranks between, which hold nothing but
/// the midpoints of edges (see `elk_ranks`), are left out, their edges joined
/// across them, and each is then ordered after its edges' upper ends, so it adds
/// no crossing.
#[allow(clippy::too_many_arguments)]
fn elk_order(
    layers: Vec<Vec<usize>>,
    (verts, created): (&[Vertex], &[usize]),
    real: usize,
    chains: &[Option<Chain>],
    inl: &[InLayer],
    border: &[bool],
    ctx: &OrderCtx,
    opts: elkorder::Options,
) -> (Vec<Vec<usize>>, elkorder::JavaRandom) {
    let (setup, elk_layers) = ElkLayers::new(layers, (verts, created), chains, inl);
    let ordered = elkorder::minimize(elk_layers, &setup.graph(border, real), ctx, opts);
    let layers = setup.complete(ordered.layers, verts, ctx);
    (layers, ordered.rng)
}

/// One pass's vertices as ELK's crossing minimisation sees them (see
/// `elk_order`): its kept ranks and the edge segments between them.
struct ElkLayers {
    kept: Vec<bool>,
    /// The kept ranks, in order.
    ranks: Vec<usize>,
    segs: Vec<(usize, usize)>,
    info: Vec<elkorder::SegInfo>,
    label: Vec<bool>,
    created: Vec<usize>,
    /// Per left-out vertex: its edge's ends on the kept layers above and below.
    across: Vec<Option<(usize, usize)>>,
    /// The layers, the kept ones taken out.
    rest: Vec<Vec<usize>>,
    inl: Vec<InLayer>,
}

impl ElkLayers {
    /// Split `layers` into ELK's layers (returned) and the rest.
    fn new(
        layers: Vec<Vec<usize>>,
        (verts, created): (&[Vertex], &[usize]),
        chains: &[Option<Chain>],
        inl: &[InLayer],
    ) -> (Self, Vec<Vec<usize>>) {
        let mut kept = vec![false; layers.len()];
        for (r, k) in kept.iter_mut().enumerate() {
            *k = r % RANK_STEP == 0;
        }
        for v in verts {
            if matches!(v.kind, Kind::Dummy { labeled: true }) {
                kept[v.rank] = true;
            }
        }
        let mut segs = Vec::new();
        let mut info = Vec::new();
        let mut across: Vec<Option<(usize, usize)>> = vec![None; verts.len()];
        for (ei, (chain, _, reversed)) in chains
            .iter()
            .enumerate()
            .filter_map(|(ei, c)| Some((ei, c.as_ref()?)))
        {
            let target = chain[chain.len() - 1];
            let mut last = chain[0];
            let mut skipped = Vec::new();
            for &v in &chain[1..] {
                if kept[verts[v].rank] {
                    segs.push((last, v));
                    info.push(elkorder::SegInfo {
                        edge: ei,
                        target,
                        reversed: *reversed,
                    });
                    for &d in &skipped {
                        across[d] = Some((last, v));
                    }
                    skipped.clear();
                    last = v;
                } else {
                    skipped.push(v);
                }
            }
        }
        let ranks: Vec<usize> = (0..layers.len()).filter(|&r| kept[r]).collect();
        let label: Vec<bool> = verts
            .iter()
            .map(|v| matches!(v.kind, Kind::Dummy { labeled: true }))
            .collect();
        let mut rest = layers;
        let elk_layers: Vec<Vec<usize>> = ranks
            .iter()
            .map(|&r| std::mem::take(&mut rest[r]))
            .collect();
        let setup = ElkLayers {
            kept,
            ranks,
            segs,
            info,
            label,
            created: created.to_vec(),
            across,
            rest,
            inl: inl.to_vec(),
        };
        (setup, elk_layers)
    }

    /// Per segment: the index of its edge.
    fn seg_edges(&self) -> impl Iterator<Item = usize> + '_ {
        self.info.iter().map(|i| i.edge)
    }

    fn graph<'a>(&'a self, border: &'a [bool], real: usize) -> elkorder::Graph<'a> {
        elkorder::Graph {
            border,
            label: &self.label,
            created: &self.created,
            segs: &self.segs,
            info: &self.info,
            real,
            inl: &self.inl,
        }
    }

    /// All layers again, from ELK's ordered ones: the ranks between, which
    /// hold nothing but the midpoints of edges, are each ordered after their
    /// edges' upper ends, so they add no crossing.
    fn complete(
        self,
        ordered: Vec<Vec<usize>>,
        verts: &[Vertex],
        ctx: &OrderCtx,
    ) -> Vec<Vec<usize>> {
        let ElkLayers {
            kept,
            ranks,
            across,
            rest: mut all,
            ..
        } = self;
        for (&r, layer) in ranks.iter().zip(ordered) {
            all[r] = layer;
        }
        let mut pos = vec![0usize; verts.len()];
        for &r in &ranks {
            for (i, &v) in all[r].iter().enumerate() {
                pos[v] = i;
            }
        }
        // Sibling clusters keep the order the kept layers give them.
        let mut before = Vec::new();
        for &r in &ranks {
            let keys: Vec<f64> = (0..all[r].len()).map(|i| i as f64).collect();
            ctx.arrange(&all[r], &keys, &mut before);
        }
        for r in (0..all.len()).filter(|&r| !kept[r]) {
            let layer = std::mem::take(&mut all[r]);
            let width = (verts.len() + 1) as f64;
            let keys: Vec<f64> = layer
                .iter()
                .map(|&v| match across[v] {
                    Some((a, b)) => pos[a] as f64 * width + pos[b] as f64,
                    None => 0.0,
                })
                .collect();
            all[r] = ctx.arrange(&layer, &keys, &mut before);
        }
        all
    }
}

/// dagre-style ordering: weighted barycentre sweeps, alternating down and up,
/// keeping the ordering with the fewest (weighted) crossings.
fn dagre_order(
    mut layers: Vec<Vec<usize>>,
    ctx: &OrderCtx,
    up: &[Vec<(usize, f64)>],
    down: &[Vec<(usize, f64)>],
    nv: usize,
) -> Vec<Vec<usize>> {
    let mut best = layers.clone();
    let mut best_cross = total_crossings(&layers, down, nv);
    let max_iter = if nv > 3000 { 6 } else { 24 };
    let mut stale = 0;
    for it in 0..max_iter {
        if best_cross <= 0.0 {
            break;
        }
        sweep(
            &mut layers,
            ctx,
            if it % 2 == 0 { up } else { down },
            it % 2 == 0,
            nv,
        );
        let cross = total_crossings(&layers, down, nv);
        if cross < best_cross {
            best_cross = cross;
            best = layers.clone();
            stale = 0;
        } else {
            stale += 1;
            if stale >= 4 {
                break;
            }
        }
    }
    best
}

fn self_loop(c: Point, node: &LayoutNode, label: Option<(f64, f64)>) -> EdgeRoute {
    let (w, h) = (node.width, node.height);
    let ext = 22.0;
    let points = vec![
        Point::new(c.x + w / 4.0, c.y - h / 2.0),
        Point::new(c.x + w / 4.0, c.y - h / 2.0 - ext),
        Point::new(c.x + w / 2.0 + ext, c.y - h / 2.0 - ext),
        Point::new(c.x + w / 2.0 + ext, c.y),
        Point::new(c.x + w / 2.0, c.y),
    ];
    let label =
        label.map(|(lw, _)| Point::new(c.x + w / 2.0 + ext + lw / 2.0 + 4.0, c.y - h / 2.0 - ext));
    EdgeRoute { points, label }
}

/// ELK keeps a node's ports this far from the ends of its side (and spreads
/// them evenly over the rest).
const ELK_PORT_INSET: f64 = 10.0;

/// [`ELK_PORT_INSET`] for a node in graph `graph`: within a container (whose
/// `spacing.baseValue` is mermaid's 24, not the root's 40) ELK keeps ports
/// 10.8 from the ends (as measured: three ports on a 45px side at 16.65, 22.5
/// and 28.35).
fn elk_port_inset(graph: ElkGraph) -> f64 {
    match graph {
        ElkGraph::Root => ELK_PORT_INSET,
        ElkGraph::Container { .. } => 10.8,
    }
}

/// Offsets from a side's start of `n` ports spread over a side of length `len`
/// (`inset` from either end, see [`elk_port_inset`]).
fn elk_port_offsets(n: usize, len: f64, inset: f64) -> impl Iterator<Item = f64> {
    let inset = inset.min(len / 2.0);
    let free = len - 2.0 * inset;
    (0..n).map(move |i| inset + free * (i + 1) as f64 / (n + 1) as f64)
}

/// The spacings between the vertices of one ELK layer. The root graph derives
/// them from `spacing.baseValue: 40` (edges at half the node spacing); a
/// container from its own `spacing.baseValue: 24`, of which `spacing.edgeNode`
/// is half, with `spacing.nodeNode: 50` and `elk.spacing.edgeEdge: 20` set
/// explicitly (mermaid's `buildSubgraphLayoutOptions`).
fn elk_spacing(graph: ElkGraph, node_sep: f64) -> elkbk::Spacing {
    match graph {
        ElkGraph::Root => elkbk::Spacing {
            node_node: node_sep,
            edge_node: node_sep / 2.0,
            edge_edge: node_sep / 2.0,
            port_port: node_sep / 2.0,
        },
        ElkGraph::Container { .. } => elkbk::Spacing {
            node_node: node_sep,
            edge_node: ELK_CONTAINER_BASE_SPACING / 2.0,
            edge_edge: ELK_CONTAINER_EDGE_SPACING,
            port_port: ELK_CONTAINER_BASE_SPACING / 2.0,
        },
    }
}

/// A container's `spacing.baseValue` and `elk.spacing.edgeEdge` (mermaid's
/// `DEFAULT_SUBGRAPH_SPACING_BASE_VALUE` and `buildSubgraphLayoutOptions`).
const ELK_CONTAINER_BASE_SPACING: f64 = 24.0;
const ELK_CONTAINER_EDGE_SPACING: f64 = 20.0;

/// ELK's node placement (see `elkbk`) for a pass without integrated clusters:
/// cross-axis centres of every vertex (an edge label's: where its edge passes,
/// through the label's middle as mermaid's labels are inline), and every edge's
/// source and target port positions.
///
/// ELK's layers are the kept ranks of `elk_order`; an edge runs between two
/// consecutive ones through a port on each end: on a node one of the ports spread
/// over its side, in the order of the vertices they lead to; on a long-edge dummy
/// its top (it has no extent); on a label its middle.
#[allow(clippy::too_many_arguments)]
fn elk_place(
    layers: &[Vec<usize>],
    verts: &[Vertex],
    real: usize,
    chains: &[Option<Chain>],
    loop_reach: &[f64],
    label_across: &[(f64, f64)],
    is_ext: &[bool],
    fixed: &[(Option<f64>, Option<f64>)],
    port_rank: &[(Option<usize>, Option<usize>)],
    inl: &[InLayer],
    end_labels: &[[Option<f64>; 2]],
    spacing: elkbk::Spacing,
    port_inset: f64,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let nv = verts.len();
    let kept_rank = |r: usize| {
        r % RANK_STEP == 0
            || layers[r]
                .iter()
                .any(|&v| matches!(verts[v].kind, Kind::Dummy { labeled: true }))
    };
    let elk_layers: Vec<Vec<usize>> = (0..layers.len())
        .filter(|&r| kept_rank(r))
        .map(|r| layers[r].clone())
        .collect();
    let mut index = vec![0usize; nv];
    for layer in &elk_layers {
        for (i, &v) in layer.iter().enumerate() {
            index[v] = i;
        }
    }
    let kept = |v: usize| kept_rank(verts[v].rank);
    let mut nodes: Vec<elkbk::Node> = verts
        .iter()
        .enumerate()
        .map(|(v, vx)| {
            let (kind, size) = match vx.kind {
                Kind::Real if is_ext.get(v).copied().unwrap_or(false) => {
                    (elkbk::NodeType::ExternalPort, 0.0)
                }
                Kind::Real => (elkbk::NodeType::Normal, vx.w),
                Kind::Dummy { labeled: true } => (elkbk::NodeType::Label, label_across[v].0),
                // `LongEdgeSplitter` makes its dummies as thick as their edge;
                // `InvertedPortProcessor` makes its own of no size.
                _ if inl.iter().any(|l| l.dummy == v) => (elkbk::NodeType::LongEdge, 0.0),
                _ => (elkbk::NodeType::LongEdge, ELK_EDGE_THICKNESS),
            };
            elkbk::Node {
                kind,
                size,
                margin_top: loop_reach.get(v).copied().unwrap_or(0.0),
                margin_bottom: 0.0,
            }
        })
        .collect();
    // Per edge: its vertices on ELK's layers.
    let ne = chains.len();
    let stops: Vec<Vec<usize>> = chains
        .iter()
        .map(|c| {
            c.as_ref()
                .map(|(chain, _, _)| chain.iter().copied().filter(|&v| kept(v)).collect())
                .unwrap_or_default()
        })
        .collect();
    // Ports on the nodes' two flow sides.
    // Ports leading to one vertex go in the order of the ports they reach there,
    // where those are fixed by a laid out container.
    let fixed_end = |ei: usize, at_end: bool| {
        let (from, to) = fixed.get(ei).copied().unwrap_or((None, None));
        // The chain is oriented; a reversed edge's ends are swapped.
        let reversed = chains[ei].as_ref().is_some_and(|c| c.2);
        if at_end != reversed { to } else { from }
    };
    let key = |o: Option<f64>| o.map_or(i64::MIN, |o| (o * 1024.0).round() as i64);
    let mut out_side: Vec<Vec<(usize, i64, usize)>> = vec![Vec::new(); real];
    let mut in_side: Vec<Vec<(usize, i64, usize)>> = vec![Vec::new(); real];
    // Where crossing minimisation ordered a node's ports (ELK's port lists), a
    // port reaching a container's port stays there: the container's ports are
    // ordered by its contents' port dummies, whose placement need not follow.
    let own_rank = |ei: usize, at_end: bool| {
        let r = port_rank.get(ei).copied().unwrap_or_default();
        if at_end { r.1 } else { r.0 }
    };
    for (ei, s) in stops.iter().enumerate().filter(|(_, s)| s.len() >= 2) {
        let direct = s.len() == 2;
        let fixed_key = |at_end: bool| {
            let end = fixed_end(ei, at_end).filter(|_| direct);
            match (end, own_rank(ei, !at_end)) {
                (Some(_), Some(r)) => r as i64,
                _ => key(end),
            }
        };
        let to_key = fixed_key(true);
        let from_key = fixed_key(false);
        // (An inverted port's dummy has no ports to spread.)
        if s[0] < real {
            out_side[s[0]].push((index[s[1]], to_key, ei));
        }
        if s[s.len() - 1] < real {
            in_side[s[s.len() - 1]].push((index[s[s.len() - 2]], from_key, ei));
        }
    }
    let mut out_port = vec![0.0; ne];
    let mut in_port = vec![0.0; ne];
    for v in 0..real {
        for (side, port, at_end) in [
            (&mut out_side[v], &mut out_port, false),
            (&mut in_side[v], &mut in_port, true),
        ] {
            side.sort_unstable();
            // Ports fixed by a laid out container stay where its contents put them.
            side.retain(|&(_, _, ei)| match fixed_end(ei, at_end) {
                Some(off) => {
                    port[ei] = verts[v].w / 2.0 + off;
                    false
                }
                None => true,
            });
            let offs = elk_port_offsets(side.len(), verts[v].w, port_inset);
            for (&(_, _, ei), off) in side.iter().zip(offs) {
                port[ei] = off;
            }
        }
    }
    // ELK's `EndLabelPreprocessor` puts an edge's end labels beside its port,
    // on the port's far side along the cross axis (the edge-label spacing and
    // the edge's thickness from it), and widens the node's margins to hold them.
    for (ei, c) in chains.iter().enumerate() {
        let Some((chain, _, reversed)) = c else {
            continue;
        };
        let labels = end_labels.get(ei).copied().unwrap_or_default();
        let (at_from, at_to) = if *reversed {
            (labels[1], labels[0])
        } else {
            (labels[0], labels[1])
        };
        let ends = [
            (chain[0], at_from, out_port[ei]),
            (chain[chain.len() - 1], at_to, in_port[ei]),
        ];
        for (v, across, port) in ends {
            let Some(across) = across.filter(|_| v < real && chain.len() >= 2) else {
                continue;
            };
            let over = port + ELK_END_LABEL_EDGE_GAP + across - verts[v].w;
            nodes[v].margin_bottom = nodes[v].margin_bottom.max(over);
        }
    }
    let port_of = |v: usize| match nodes[v].kind {
        elkbk::NodeType::Label => label_across[v].1,
        _ => 0.0,
    };
    // An edge whose chain ends at an inverted port's dummy (a label dummy, if
    // `LabelDummySwitcher` moved one there) leaves or enters it at its port.
    for (ei, s) in stops.iter().enumerate().filter(|(_, s)| s.len() >= 2) {
        if s[0] >= real {
            out_port[ei] = port_of(s[0]);
        }
        if s[s.len() - 1] >= real {
            in_port[ei] = port_of(s[s.len() - 1]);
        }
    }
    // The order of a node's ports on a side: where a laid out container fixes
    // them all, the order crossing minimisation left them in (which its
    // contents' placement need not follow), else their positions.
    let mut out_order = out_port.clone();
    let mut in_order = in_port.clone();
    for (at_end, order) in [(false, &mut out_order), (true, &mut in_order)] {
        let mut by_node: Vec<Vec<usize>> = vec![Vec::new(); real];
        for (ei, s) in stops.iter().enumerate().filter(|(_, s)| s.len() >= 2) {
            let v = if at_end { s[s.len() - 1] } else { s[0] };
            if v < real {
                by_node[v].push(ei);
            }
        }
        for list in by_node {
            let ranks: Option<Vec<usize>> = list
                .iter()
                .map(|&ei| {
                    fixed_end(ei, at_end)?;
                    let r = port_rank.get(ei).copied().unwrap_or_default();
                    if at_end { r.1 } else { r.0 }
                })
                .collect();
            if let Some(ranks) = ranks {
                for (&ei, r) in list.iter().zip(ranks) {
                    order[ei] = r as f64;
                }
            }
        }
    }
    let mut edges = Vec::new();
    for (ei, s) in stops.iter().enumerate() {
        for (k, w) in s.windows(2).enumerate() {
            let from_port = if k == 0 { out_port[ei] } else { port_of(w[0]) };
            let to_port = if k + 2 == s.len() {
                in_port[ei]
            } else {
                port_of(w[1])
            };
            edges.push(elkbk::Edge {
                from: w[0],
                to: w[1],
                from_port,
                to_port,
                from_order: if k == 0 { out_order[ei] } else { from_port },
                to_order: if k + 2 == s.len() {
                    in_order[ei]
                } else {
                    to_port
                },
            });
        }
    }
    let top = elkbk::place(&elk_layers, &nodes, &edges, spacing);
    let mut x: Vec<f64> = (0..nv)
        .map(|v| match nodes[v].kind {
            elkbk::NodeType::Normal => top[v] + verts[v].w / 2.0,
            _ => top[v] + port_of(v),
        })
        .collect();
    // Vertices between ELK's layers follow their edge from the stop before.
    for (ei, c) in chains.iter().enumerate() {
        let Some((chain, _, _)) = c else { continue };
        let mut at = top[chain[0]] + out_port[ei];
        for &v in &chain[1..chain.len() - 1] {
            if kept(v) {
                at = x[v];
            } else {
                x[v] = at;
            }
        }
    }
    let mut start: Vec<f64> = (0..ne)
        .map(|ei| stops[ei].first().map_or(0.0, |&u| top[u] + out_port[ei]))
        .collect();
    let mut end: Vec<f64> = (0..ne)
        .map(|ei| stops[ei].last().map_or(0.0, |&w| top[w] + in_port[ei]))
        .collect();
    let min = (0..nv)
        .filter(|&v| kept(v))
        .map(|v| match nodes[v].kind {
            elkbk::NodeType::LongEdge => x[v],
            _ => top[v],
        })
        .fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        for p in x.iter_mut().chain(&mut start).chain(&mut end) {
            *p -= min;
        }
    }
    (x, start, end)
}

/// Where ELK puts a label whose dummy's ports carry the edge through `c`: not quite
/// centred on it, but half a pixel-rounded label before it (`ceil(width) / 2` left
/// of a vertical edge; `(ceil(height) + 1) / 2` above a horizontal one), in every
/// direction.
fn elk_label_centre(c: Point, label: Option<(f64, f64)>, horizontal: bool) -> Point {
    let Some((w, h)) = label else { return c };
    if horizontal {
        Point::new(c.x, c.y - (h.ceil() + 1.0 - h) / 2.0)
    } else {
        Point::new(c.x - (w.ceil() - w) / 2.0, c.y)
    }
}

/// How far an orthogonal self-loop stands off its node.
const SELF_LOOP_EXT: f64 = 20.0;
/// Gap between an ELK self-loop's outer leg and its label.
const SELF_LOOP_LABEL_GAP: f64 = 4.0;
/// ELK's `spacing.portsSurrounding` as mermaid sets it: no port within 12px of a
/// node's corner.
pub(crate) const PORTS_SURROUNDING: f64 = 12.0;
/// The least room between an orthogonal self-loop (or its label) and the node beside it.
const SELF_LOOP_CLEARANCE: f64 = 10.0;

/// The rank-axis-independent part of the orthogonal routes of one layered pass
/// (see [`ortho`]).
struct OrthoPlan {
    /// Per edge (oriented along the flow): the points it must pass, as (vertex,
    /// cross-axis position): its source port, its dummies in obstacle ranks, its
    /// target port.
    stops: Vec<Vec<(usize, f64)>>,
    /// Per edge, per pair of consecutive stops: the slot level of the jog between
    /// them, if their positions differ.
    jogs: Vec<Vec<Option<usize>>>,
    /// Per edge, per jog split in two (a critical cycle in its channel): where
    /// its two parts meet across, and the second part's slot level.
    splits: Vec<Vec<Option<(f64, usize)>>>,
    /// Per rank: the channel below it, as (next obstacle rank, slots used).
    channels: Vec<Option<(usize, usize)>>,
    spacing: OrthoSpacing,
    /// Cross-axis positions, with dummies moved in line with their edges' ports.
    xs: Vec<f64>,
    /// Per in-layer edge of an inverted port: the edge, its port and dummy
    /// positions, whether it is on the node's previous layer's side, the
    /// channel it runs in (named by the obstacle rank above it, `None` outside
    /// the outer layers) and its slot there.
    in_layer: Vec<InLayerRoute>,
    /// Slots in the channels outside the first and the last layer.
    outer: (usize, usize),
}

/// The route of an inverted port's in-layer edge (see [`OrthoPlan`]).
#[derive(Debug, Clone, Copy)]
struct InLayerRoute {
    edge: usize,
    node: usize,
    dummy: usize,
    port: f64,
    at: f64,
    before: bool,
    channel: Option<usize>,
    slot: usize,
    /// Split in two by the router (a critical cycle in its channel): where the
    /// two parts meet across, and the second part's slot.
    split: Option<(f64, usize)>,
}

#[allow(clippy::too_many_arguments)]
fn ortho_plan(
    verts: &[Vertex],
    n: usize,
    chains: &[Option<Chain>],
    x: &[f64],
    layers: &[Vec<usize>],
    gap: &dyn Fn(usize, usize) -> f64,
    spacing: OrthoSpacing,
    ports: Option<(Vec<f64>, Vec<f64>)>,
    inl: &[InLayer],
    fixed: &[(Option<f64>, Option<f64>)],
    port_rank: &[(Option<usize>, Option<usize>)],
    rng: &mut elkorder::JavaRandom,
) -> OrthoPlan {
    let max_rank = verts.iter().map(|v| v.rank).max().unwrap_or(0);
    // Ranks holding nodes or edge labels; ranks of plain long-edge dummies are free.
    let mut obstacle = vec![false; max_rank + 1];
    for v in verts {
        if matches!(v.kind, Kind::Real | Kind::Dummy { labeled: true }) {
            obstacle[v.rank] = true;
        }
    }
    // Ports: the edge ends on each node's two flow sides, in the order of the
    // vertices they lead to, spread evenly over the side.
    let ne = chains.len();
    let mut out_side: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); n];
    let mut in_side: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); n];
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, _, _)) = ch else { continue };
        let (u, w) = (chain[0], chain[chain.len() - 1]);
        if u < n {
            out_side[u].push((x[chain[1]], x[w], ei));
        }
        if w < n {
            in_side[w].push((x[chain[chain.len() - 2]], x[u], ei));
        }
    }
    let placed = ports.is_some();
    let (mut start_x, mut end_x) = ports.unwrap_or_else(|| (vec![0.0; ne], vec![0.0; ne]));
    for v in (0..n).filter(|_| !placed) {
        for (side, port_x) in [
            (&mut out_side[v], &mut start_x),
            (&mut in_side[v], &mut end_x),
        ] {
            side.sort_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(a.1.total_cmp(&b.1))
                    .then(a.2.cmp(&b.2))
            });
            for (&(_, _, ei), off) in side.iter().zip(ortho::port_offsets(
                side.len(),
                (verts[v].w - 2.0 * ELK_PORT_INSET).max(0.0),
            )) {
                port_x[ei] = x[v] + off;
            }
        }
    }
    // ELK places nodes with their ports in view (`nodeFlexibility: PORT_POSITION`),
    // so an edge leaving a port runs straight on where it can. Here: top-down, each
    // dummy in an obstacle rank moves as far toward the position the edge arrives
    // at (its source port, or its previous such dummy) as its neighbours allow.
    // An edge label, though, stands in line with the port the edge ends at (the
    // original target's, so a reversed edge's source port), so the edge bends
    // before its label and runs straight into the arrowhead (as mermaid's ELK
    // output does); on a direct edge the target port is the next stop.
    let mut xs = x.to_vec();
    let mut arrive: Vec<Option<(usize, usize, bool)>> = vec![None; verts.len()]; // (edge, prev, to target)
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, label_v, reversed)) = ch else {
            continue;
        };
        let mut prev = chain[0];
        let last = chain.len() - 1;
        for (k, &v) in chain.iter().enumerate().take(last).skip(1) {
            if obstacle[verts[v].rank] {
                let next_is_target = chain[k + 1..last].iter().all(|&d| !obstacle[verts[d].rank]);
                let to_target = !reversed && *label_v == Some(v) && next_is_target;
                arrive[v] = Some((ei, prev, to_target));
                prev = v;
            }
        }
    }
    for layer in layers.iter().filter(|_| !placed) {
        for (i, &v) in layer.iter().enumerate() {
            let Some((ei, prev, to_target)) = arrive[v] else {
                continue;
            };
            let want = if to_target {
                end_x[ei]
            } else if prev < n {
                start_x[ei]
            } else {
                xs[prev]
            };
            let lo = i
                .checked_sub(1)
                .map_or(f64::NEG_INFINITY, |j| xs[layer[j]] + gap(layer[j], v));
            let hi = layer
                .get(i + 1)
                .map_or(f64::INFINITY, |&r| xs[r] - gap(v, r));
            if lo <= hi {
                xs[v] = want.clamp(lo, hi);
            }
        }
    }
    let mut stops: Vec<Vec<(usize, f64)>> = vec![Vec::new(); ne];
    for (ei, ch) in chains.iter().enumerate() {
        let Some((chain, _, _)) = ch else { continue };
        let last = chain.len() - 1;
        stops[ei] = chain
            .iter()
            .enumerate()
            .filter_map(|(k, &v)| match k {
                0 => Some((v, start_x[ei])),
                _ if k == last => Some((v, end_x[ei])),
                _ => obstacle[verts[v].rank].then_some((v, xs[v])),
            })
            .collect();
    }
    // Jogs, channel by channel (a channel is named by the obstacle rank above it);
    // `usize::MAX` is an in-layer edge (by its index in `in_layer`).
    type Entry = (usize, usize, ortho::Trunk);
    let mut by_channel: Vec<Vec<Entry>> = vec![Vec::new(); max_rank + 1];
    let mut channels: Vec<Option<(usize, usize)>> = vec![None; max_rank + 1];
    let mut jogs: Vec<Vec<Option<usize>>> = stops
        .iter()
        .map(|s| vec![None; s.len().saturating_sub(1)])
        .collect();
    let mut splits: Vec<Vec<Option<(f64, usize)>>> =
        jogs.iter().map(|j| vec![None; j.len()]).collect();
    for (ei, s) in stops.iter().enumerate() {
        for (k, w) in s.windows(2).enumerate() {
            let ((a, ax), (b, bx)) = (w[0], w[1]);
            // A straight run takes no slot, but its ends count in the channel.
            let ra = verts[a].rank;
            by_channel[ra].push((ei, k, ortho::Jog { from: ax, to: bx }.into()));
            if (ax - bx).abs() >= ortho::TOLERANCE {
                channels[ra] = Some((verts[b].rank, 0));
            }
        }
    }
    // An inverted port's in-layer edge runs in the channel on its port's side,
    // both its ends on that side's layer.
    let mut in_layer: Vec<InLayerRoute> = Vec::new();
    let mut outer_lists: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for l in inl {
        let r = verts[l.dummy].rank;
        let (f, t) = fixed.get(l.edge).copied().unwrap_or((None, None));
        let port = x[l.node] + if l.before { t } else { f }.unwrap_or(0.0);
        // A label dummy (moved there by `LabelDummySwitcher`) carries the edge
        // at its port, as the rest of the route has it.
        let at = match (verts[l.dummy].kind, l.before) {
            (Kind::Dummy { labeled: true }, true) => start_x[l.edge],
            (Kind::Dummy { labeled: true }, false) => end_x[l.edge],
            _ => xs[l.dummy],
        };
        let (lo, hi) = (port.min(at), port.max(at));
        let trunk = if l.before {
            ortho::Trunk {
                incoming: Vec::new(),
                outgoing: vec![lo, hi],
            }
        } else {
            ortho::Trunk {
                incoming: vec![lo, hi],
                outgoing: Vec::new(),
            }
        };
        let channel = if l.before {
            (0..r).rev().find(|&q| obstacle[q]).map(|ra| (ra, r))
        } else {
            (r + 1..=max_rank).find(|&q| obstacle[q]).map(|rb| (r, rb))
        };
        let k = in_layer.len();
        in_layer.push(InLayerRoute {
            edge: l.edge,
            node: l.node,
            dummy: l.dummy,
            port,
            at,
            before: l.before,
            channel: channel.map(|c| c.0),
            slot: 0,
            split: None,
        });
        match channel {
            Some((ra, rb)) => {
                by_channel[ra].push((usize::MAX, k, trunk));
                channels[ra] = Some((rb, 0));
            }
            None => outer_lists[usize::from(!l.before)].push(k),
        }
    }
    // ELK routes channel by channel from the first (`OrthogonalEdgeRouter`): the
    // one before the first layer, those between layers, the one after the last;
    // each with the graph's random generator, its segments in ELK's order (see
    // `ortho::assign_slots_trunks`): by the upper layer's ports, then by the
    // lower layer's ports of in-layer edges.
    let in_layer_key = |r: &InLayerRoute| {
        if r.before { (1, r.port) } else { (0, r.at) }
    };
    let mut outer = (0, 0);
    let route_outer =
        |side: usize, in_layer: &mut [InLayerRoute], rng: &mut elkorder::JavaRandom| {
            let mut list = outer_lists[side].clone();
            list.sort_by(|&a, &b| {
                let (ka, kb) = (in_layer_key(&in_layer[a]), in_layer_key(&in_layer[b]));
                ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            });
            let segs: Vec<ortho::Trunk> = list
                .iter()
                .map(|&k| {
                    let r = &in_layer[k];
                    let ends = vec![r.port.min(r.at), r.port.max(r.at)];
                    match side {
                        0 => ortho::Trunk {
                            incoming: Vec::new(),
                            outgoing: ends,
                        },
                        _ => ortho::Trunk {
                            incoming: ends,
                            outgoing: Vec::new(),
                        },
                    }
                })
                .collect();
            let out = ortho::assign_slots_trunks(&segs, spacing.edge_edge, rng);
            for (i, &k) in list.iter().enumerate() {
                in_layer[k].slot = out.slot[i];
                in_layer[k].split = out.split[i];
            }
            out.count
        };
    outer.0 = route_outer(0, &mut in_layer, rng);
    for (ra, list) in by_channel.iter_mut().enumerate() {
        if list.is_empty() {
            continue;
        }
        // Node by node, port by port: a container's ports stand in the order
        // crossing minimisation gave them, which their positions (from its
        // contents' layout) need not follow.
        let key = |e: &Entry| match e.0 {
            usize::MAX => {
                let (g, at) = in_layer_key(&in_layer[e.1]);
                (g, at, at)
            }
            ei => {
                let (a, at) = stops[ei][e.1];
                let rank = port_rank.get(ei).and_then(|r| r.0).filter(|_| {
                    let reversed = chains[ei].as_ref().is_some_and(|c| c.2);
                    let (from, to) = fixed.get(ei).copied().unwrap_or((None, None));
                    e.1 == 0 && a < n && if reversed { to } else { from }.is_some()
                });
                match rank {
                    Some(r) => (0, x[a], r as f64),
                    None => (0, if a < n { x[a] } else { at }, at),
                }
            }
        };
        list.sort_by(|a, b| {
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.total_cmp(&kb.1))
                .then(ka.2.total_cmp(&kb.2))
        });
        let segs: Vec<ortho::Trunk> = list.iter().map(|(_, _, j)| j.clone()).collect();
        let out = ortho::assign_slots_trunks(&segs, spacing.edge_edge, rng);
        for (i, &(ei, k, ref t)) in list.iter().enumerate() {
            if ei == usize::MAX {
                in_layer[k].slot = out.slot[i];
                in_layer[k].split = out.split[i];
            } else if (t.incoming[0] - t.outgoing[0]).abs() >= ortho::TOLERANCE {
                jogs[ei][k] = Some(out.slot[i]);
                splits[ei][k] = out.split[i];
            }
        }
        if let Some((rb, _)) = channels[ra] {
            channels[ra] = Some((rb, out.count));
        }
    }
    outer.1 = route_outer(1, &mut in_layer, rng);
    OrthoPlan {
        stops,
        jogs,
        splits,
        channels,
        spacing,
        xs,
        in_layer,
        outer,
    }
}

impl OrthoPlan {
    /// Edge `ei`'s route in layout space, oriented along the flow: from its source
    /// port on the node's side, through its stops, to its target port.
    #[allow(clippy::too_many_arguments)]
    fn route(
        &self,
        ei: usize,
        verts: &[Vertex],
        rank_y: &[f64],
        vy: &[f64],
        rank_h: &[f64],
        start_pad: &[f64],
        end_pad: &[f64],
    ) -> Vec<Point> {
        let stops = &self.stops[ei];
        let (Some(&(u, ux)), Some(&(w, _))) = (stops.first(), stops.last()) else {
            return Vec::new();
        };
        let mut pts = vec![Point::new(ux, vy[u] + verts[u].h / 2.0)];
        // A run without a jog keeps its cross position: stops less than the jog
        // threshold apart would otherwise leave it slightly slanted.
        let mut cur = ux;
        for (k, pair) in stops.windows(2).enumerate() {
            let ((a, _), (b, bx)) = (pair[0], pair[1]);
            let ax = cur;
            if let Some(level) = self.jogs[ei][k] {
                let ra = verts[a].rank;
                let (rb, slots) = self.channels[ra].unwrap_or((verts[b].rank, 1));
                let top = rank_y[ra] + rank_h[ra] / 2.0 + end_pad[ra];
                let bottom = rank_y[rb] - rank_h[rb] / 2.0 - start_pad[rb];
                let ee = self.spacing.edge_edge;
                let at = |level: usize| {
                    (top + bottom) / 2.0 - (slots - 1) as f64 * ee / 2.0 + level as f64 * ee
                };
                let y = at(level);
                pts.push(Point::new(ax, y));
                if let Some((sx, second)) = self.splits[ei][k] {
                    pts.push(Point::new(sx, y));
                    pts.push(Point::new(sx, at(second)));
                    pts.push(Point::new(bx, at(second)));
                } else {
                    pts.push(Point::new(bx, y));
                }
                cur = bx;
            }
            if b != w {
                pts.push(Point::new(cur, vy[b]));
            }
        }
        pts.push(Point::new(cur, vy[w] - verts[w].h / 2.0));
        // An inverted port: from the node's port round through its channel to
        // the dummy, or from the dummy back to the port.
        for r in self.in_layer.iter().filter(|r| r.edge == ei) {
            let ee = self.spacing.edge_edge;
            let start = |q: usize| rank_y[q] - rank_h[q] / 2.0 - start_pad[q];
            let end = |q: usize| rank_y[q] + rank_h[q] / 2.0 + end_pad[q];
            let rank = verts[r.dummy].rank;
            let level = |slot: usize| match r.channel {
                Some(ra) => {
                    let (rb, slots) = self.channels[ra].unwrap_or((rank, 1));
                    (end(ra) + start(rb)) / 2.0 - (slots - 1) as f64 * ee / 2.0 + slot as f64 * ee
                }
                None if r.before => {
                    start(rank) - self.spacing.edge_node - (self.outer.0 - 1 - slot) as f64 * ee
                }
                None => end(rank) + self.spacing.edge_node + slot as f64 * ee,
            };
            // Along the in-layer edge, source to target (the port to the dummy
            // before the layer, the dummy to the port after it): its slot, and
            // where the router split it, the second part's slot from there on.
            let (from, to) = if r.before {
                (r.port, r.at)
            } else {
                (r.at, r.port)
            };
            let y = level(r.slot);
            let mut run = vec![Point::new(from, y)];
            if let Some((sx, second)) = r.split {
                let y2 = level(second);
                run.extend([Point::new(sx, y), Point::new(sx, y2), Point::new(to, y2)]);
            } else {
                run.push(Point::new(to, y));
            }
            let h = verts[r.node].h / 2.0;
            if r.before {
                run.insert(0, Point::new(r.port, vy[r.node] - h));
                pts.splice(0..0, run);
            } else {
                run.push(Point::new(r.port, vy[r.node] + h));
                pts.extend(run);
            }
        }
        ortho::simplify(&pts)
    }

    /// The room the channels outside the first and the last layer take.
    fn outer_room(&self) -> (f64, f64) {
        let room = |slots: usize| {
            if slots == 0 {
                0.0
            } else {
                self.spacing.edge_node + (slots - 1) as f64 * self.spacing.edge_edge
            }
        };
        (room(self.outer.0), room(self.outer.1))
    }
}

/// DFS-based cycle breaking. Returns `None` for self-loops.
///
/// dagre (`acyclic.dfsFAS`) starts a search from every node in turn. ELK's
/// `DepthFirstCycleBreaker` (mermaid's default `cycleBreaking: DEPTH_FIRST`) first
/// searches from every source, a node without incoming edges, in model order and
/// only then from the nodes left unvisited: a cycle hanging below a source is entered
/// from that source, so the edge closing it, not one leaving the first node, is the
/// one reversed (`sources_first`).
fn orient_edges(
    n: usize,
    edges: &[LayoutEdge],
    scale: usize,
    sources_first: bool,
    made: &[(usize, usize)],
) -> Vec<Option<Oriented>> {
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_in = vec![false; n];
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to {
            out_edges[e.from].push(i);
            has_in[e.to] = true;
        }
    }
    // A node's outgoing edges port by port, in the order its ports were made.
    if !made.is_empty() {
        for out in &mut out_edges {
            out.sort_by_key(|&i| (made[i], i));
        }
    }
    let roots: Vec<usize> = if sources_first {
        (0..n).filter(|&v| !has_in[v]).chain(0..n).collect()
    } else {
        (0..n).collect()
    };
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }
    let mut mark = vec![Mark::White; n];
    let mut reversed = vec![false; edges.len()];
    for root in roots {
        if mark[root] != Mark::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        mark[root] = Mark::Gray;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if *i < out_edges[v].len() {
                let ei = out_edges[v][*i];
                *i += 1;
                let w = edges[ei].to;
                match mark[w] {
                    Mark::Gray => reversed[ei] = true,
                    Mark::White => {
                        mark[w] = Mark::Gray;
                        stack.push((w, 0));
                    }
                    Mark::Black => {}
                }
            } else {
                mark[v] = Mark::Black;
                stack.pop();
            }
        }
    }
    edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            (e.from != e.to).then(|| {
                let (from, to) = if reversed[i] {
                    (e.to, e.from)
                } else {
                    (e.from, e.to)
                };
                Oriented {
                    from,
                    to,
                    minlen: e.minlen.max(1) * scale,
                    reversed: reversed[i],
                    weight: 1.0,
                }
            })
        })
        .collect()
}

const NETWORK_SIMPLEX_MAX_NODES: usize = 4000;

/// Ranks per edge of length one: every edge spans two ranks (see `layout_flat`).
const RANK_STEP: usize = 2;

/// ELK's layers (of real nodes and label dummies) as ranks: a layer holding a real
/// node gets the next even rank; one holding nothing but labels (and long edges)
/// gets the odd rank before it, where the labels of edges between neighbouring
/// layers sit anyway, so a label layer is spaced like dagre's label ranks.
fn elk_ranks(layers: &[usize]) -> impl Fn(usize) -> usize + use<> {
    let top = layers.iter().copied().max().unwrap_or(0);
    let mut occupied = vec![false; top + 2];
    for &l in layers {
        occupied[l] = true;
    }
    let mut rank = Vec::with_capacity(top + 2);
    let mut real: usize = 0;
    for occ in occupied {
        if occ {
            rank.push(2 * real);
            real += 1;
        } else {
            rank.push((2 * real).saturating_sub(1));
        }
    }
    move |l: usize| rank[l.min(rank.len() - 1)]
}

/// ELK's network simplex layering ends with `NetworkSimplex.balance`: a node with as
/// many incoming as outgoing edges, which can sit on any rank its edges allow at no
/// cost, moves to the least filled of those ranks if it is strictly emptier than its
/// own. Nodes are visited in model order and counted per connected component, as
/// `NetworkSimplexLayerer` lays out each component on its own. Only the first `real`
/// vertices are real nodes (and moved or counted); the rest, with their edges, only
/// bound the ranks a node may take. Ranks of real nodes stay multiples of `step`.
fn balance_ranks(
    ranks: &mut [usize],
    real: usize,
    edges: &[Option<Oriented>],
    step: usize,
) -> Vec<usize> {
    let total = ranks.len();
    // Per vertex: (other end, minlen, counts as an edge) of its incoming / outgoing edges.
    let mut ins: Vec<Vec<(usize, usize, bool)>> = vec![Vec::new(); total];
    let mut outs: Vec<Vec<(usize, usize, bool)>> = vec![Vec::new(); total];
    // Connected components of the real nodes (union-find).
    let mut comp: Vec<usize> = (0..total).collect();
    fn find(comp: &mut [usize], mut v: usize) -> usize {
        while comp[v] != v {
            comp[v] = comp[comp[v]];
            v = comp[v];
        }
        v
    }
    for o in edges.iter().flatten() {
        if o.from == o.to {
            continue;
        }
        let counts = o.weight > 0.0;
        outs[o.from].push((o.to, o.minlen, counts));
        ins[o.to].push((o.from, o.minlen, counts));
        let (a, b) = (find(&mut comp, o.from), find(&mut comp, o.to));
        comp[a] = b;
    }
    // Each component is layered on its own, so each starts on the first rank.
    let mut low: std::collections::HashMap<usize, usize> = Default::default();
    let all_roots: Vec<usize> = (0..total).map(|v| find(&mut comp, v)).collect();
    for v in 0..total {
        let m = low.entry(all_roots[v]).or_insert(usize::MAX);
        *m = (*m).min(ranks[v]);
    }
    for v in 0..total {
        ranks[v] -= low[&all_roots[v]] / step * step;
    }
    let layered = ranks.to_vec();
    let roots: Vec<usize> = (0..real).map(|v| find(&mut comp, v)).collect();
    let mut filling: std::collections::HashMap<(usize, usize), usize> = Default::default();
    for v in 0..real {
        *filling.entry((roots[v], ranks[v])).or_default() += 1;
    }
    for v in 0..real {
        let din = ins[v].iter().filter(|e| e.2).count();
        let dout = outs[v].iter().filter(|e| e.2).count();
        if din != dout || din == 0 {
            continue;
        }
        let cur = ranks[v];
        // ELK's `minimalSpan`, quirk included: an incoming edge no shorter than the
        // shortest incoming one before it is taken for an outgoing one, so a node with
        // two equally short incoming edges stays where it is.
        let (mut span_in, mut span_out) = (usize::MAX, usize::MAX);
        for &(u, _, _) in ins[v].iter().filter(|e| e.2) {
            let span = cur.saturating_sub(ranks[u]);
            if span < span_in {
                span_in = span;
            } else {
                span_out = span_out.min(span);
            }
        }
        for &(w, _, _) in outs[v].iter().filter(|e| e.2) {
            span_out = span_out.min(ranks[w].saturating_sub(cur));
        }
        // Edges that do not count (cluster constraints) still bound the rank.
        let lo = ins[v]
            .iter()
            .map(|&(u, len, _)| ranks[u] + len)
            .chain([(cur + step).saturating_sub(span_in)])
            .max()
            .unwrap_or(cur);
        let hi = outs[v]
            .iter()
            .map(|&(w, len, _)| ranks[w].saturating_sub(len))
            .chain([(cur + span_out).saturating_sub(step)])
            .min()
            .unwrap_or(cur);
        let fill = |r: usize| filling.get(&(roots[v], r)).copied().unwrap_or(0);
        let mut best = cur;
        let mut r = lo + (cur + step - lo % step) % step;
        while r <= hi {
            if fill(r) < fill(best) {
                best = r;
            }
            r += step;
        }
        if best != cur && fill(best) < fill(cur) {
            *filling.entry((roots[v], cur)).or_default() -= 1;
            *filling.entry((roots[v], best)).or_default() += 1;
            ranks[v] = best;
        }
    }
    layered
}

/// Longest-path ranking refined by network simplex.
fn rank_nodes(n: usize, edges: &[Option<Oriented>]) -> Vec<usize> {
    let mut indeg = vec![0usize; n];
    let mut outs: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut ins: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for o in edges.iter().flatten() {
        indeg[o.to] += 1;
        outs[o.from].push((o.to, o.minlen));
        ins[o.to].push((o.from, o.minlen));
    }
    let mut order = Vec::with_capacity(n);
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    let mut rank = vec![0i64; n];
    while let Some(v) = queue.pop_front() {
        order.push(v);
        for &(w, len) in &outs[v] {
            rank[w] = rank[w].max(rank[v] + len as i64);
            indeg[w] -= 1;
            if indeg[w] == 0 {
                queue.push_back(w);
            }
        }
    }
    // Longest-path puts every node as high as possible, so a chain hanging off a
    // late source starts at the top and drags a very long edge. Network simplex
    // (as in dagre/graphviz) finds the ranking with minimum total edge length.
    // Cheap greedy pre-pass: slide nodes toward successors when that shortens
    // edges (reverse topological order moves chains together). This gets close
    // to optimal quickly and leaves network simplex few pivots to do.
    for _ in 0..4 {
        let mut changed = false;
        for &v in order.iter().rev() {
            if outs[v].is_empty() || outs[v].len() <= ins[v].len() {
                continue;
            }
            let limit = outs[v]
                .iter()
                .map(|&(w, len)| rank[w] - len as i64)
                .min()
                .unwrap_or(rank[v]);
            if limit > rank[v] {
                rank[v] = limit;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if n <= NETWORK_SIMPLEX_MAX_NODES {
        let rank_edges: Vec<rank::RankEdge> = edges
            .iter()
            .flatten()
            .map(|o| rank::RankEdge {
                tail: o.from,
                head: o.to,
                minlen: o.minlen as i64,
                weight: o.weight,
            })
            .collect();
        rank::network_simplex(n, &rank_edges, &mut rank, 10 * n + 100);
    }
    let min = rank.iter().copied().min().unwrap_or(0);
    rank.into_iter().map(|r| (r - min) as usize).collect()
}

/// ELK's layering of a graph with edge labels: `LabelDummyInserter` has made each
/// labelled edge's label a node of its own (in the order of the nodes and their
/// outgoing edges) before `NetworkSimplexLayerer` runs, so the labels take part in
/// the layering and in its balancing like any node (where a label stands decides
/// which ranks are fullest). The layering is ELK's own (`elkns`), so that ties
/// between equally short layerings break as in ELK. Returns the real nodes' ranks,
/// twice: the labels are then placed by `LabelDummySwitcher`'s median rule on the
/// balanced ranks (see `prepare`); and the order the layering put the nodes and
/// labels in their layers (see [`Ranking`]).
fn rank_with_label_nodes(
    n: usize,
    oriented: &[Option<Oriented>],
    input: &LayoutInput,
    (hidden, hidden_edges): (&[bool], &[Option<Oriented>]),
    made: &[[(usize, usize); 2]],
) -> Ranking {
    // Every edge end is a port of its own, in the order the edges were imported;
    // a label dummy has its input port, then its output port.
    // Hidden nodes (external port dummies) are no nodes of the layering at all.
    let mut id = vec![usize::MAX; n];
    let mut next = 0;
    for v in (0..n).filter(|&v| !hidden.get(v).copied().unwrap_or(false)) {
        id[v] = next;
        next += 1;
    }
    let mut edges: Vec<elkns::NsEdge> = Vec::new();
    let mut label_of: Vec<Option<usize>> = vec![None; oriented.len()];
    // An edge to an external port dummy is hidden with it, but its label
    // dummy, made before, stays: tied to the edge's other end only.
    let edge_of = |ei: usize| -> Option<(&Oriented, bool)> {
        match (
            oriented[ei].as_ref(),
            hidden_edges.get(ei).and_then(Option::as_ref),
        ) {
            (Some(o), _) => Some((o, false)),
            (None, Some(o)) if input.edges[ei].label.is_some() => Some((o, true)),
            _ => None,
        }
    };
    // Port keys at the edge's (oriented) source and target: in the order the
    // ports were made (a laid out container's ports for edges crossing its
    // border come after those for edges to itself, see `Hierarchy::port_made`).
    let key = |ei: usize, o: &Oriented| {
        let at = |side: usize| {
            let (tier, k) = made.get(ei).map_or((0, ei), |m| m[side]);
            tier * (usize::MAX / 4) + 2 * k.min(usize::MAX / 16) + side
        };
        let (a, b) = (at(0), at(1));
        if o.reversed { (b, a) } else { (a, b) }
    };
    let mut by_source: Vec<(usize, usize, usize)> = (0..oriented.len())
        .filter_map(|ei| edge_of(ei).map(|(o, _)| (o.from, key(ei, o).0, ei)))
        .collect();
    by_source.sort_unstable();
    for (ei, o) in oriented.iter().enumerate() {
        let Some(o) = o.as_ref() else { continue };
        if input.edges[ei].label.is_none() || o.from == o.to {
            edges.push(elkns::NsEdge {
                from: id[o.from],
                to: id[o.to],
                delta: o.minlen as i64,
                ports: key(ei, o),
            });
        }
    }
    // `LabelDummyInserter` makes the dummies in node order, per node in the order
    // of its outgoing edges, and appends them to the nodes.
    for (_, _, ei) in by_source {
        if let Some((o, hid)) = edge_of(ei) {
            if input.edges[ei].label.is_some() && o.from != o.to {
                let (ks, kt) = key(ei, o);
                let shown = |v: usize| !hid || !hidden.get(v).copied().unwrap_or(false);
                if shown(o.from) {
                    edges.push(elkns::NsEdge {
                        from: id[o.from],
                        to: next,
                        delta: 1,
                        ports: (ks, 0),
                    });
                }
                if shown(o.to) {
                    edges.push(elkns::NsEdge {
                        from: next,
                        to: id[o.to],
                        delta: 1,
                        ports: (1, kt),
                    });
                }
                label_of[ei] = Some(next);
                next += 1;
            }
        }
    }
    let layering = elkns::layer(next, &edges);
    let of = |at: &[usize], v: usize, none: usize| at.get(id[v]).copied().unwrap_or(none);
    let ranks: Vec<usize> = (0..n).map(|v| of(&layering.layers, v, 0)).collect();
    let mut created: Vec<usize> = (0..n)
        .map(|v| of(&layering.created, v, usize::MAX))
        .collect();
    created.extend(
        label_of
            .iter()
            .map(|l| l.map_or(usize::MAX, |v| layering.created[v])),
    );
    let label_layer = label_of
        .iter()
        .map(|l| l.map(|v| layering.layers[v]))
        .collect();
    (ranks.clone(), ranks, created, label_layer)
}

/// Ranks of the real nodes, the layering's own (before ELK's balancing), and
/// under ELK the order the layering put things in their layers (see
/// `elkns::Layering::created`): per node, then per edge for its label dummy
/// (`usize::MAX` for none), and per edge the layer of its label dummy; both
/// empty otherwise.
type Ranking = (Vec<usize>, Vec<usize>, Vec<usize>, Vec<Option<usize>>);

/// Rank like `rank_nodes`, but an edge leaving (entering) a whole cluster is
/// constrained to start below its last (end above its first) member, the way
/// dagre's nesting graph ties cluster borders to their contents: every member
/// hangs between a virtual top and bottom vertex of the cluster and the edge
/// runs from the bottom / to the top vertex. An edge whose constraint would be
/// contradictory (e.g. from a cluster into its own member) is ranked by its plain
/// member ends instead; the other edges keep theirs.
///
/// When the constraints hold, an edge leaving a cluster is re-attached to the member
/// on the cluster's last rank (one entering it to a member on its first rank), so the
/// route leaves from the cluster's far side instead of from wherever the caller's
/// representative member happens to sit.
fn rank_with_clusters(
    n: usize,
    oriented: &mut [Option<Oriented>],
    input: &LayoutInput,
    (hidden, hidden_edges): (&[bool], &[Option<Oriented>]),
    made: &[[(usize, usize); 2]],
) -> Ranking {
    let mut slot: Vec<Option<usize>> = vec![None; input.clusters.len()];
    let mut used = Vec::new();
    let mut ends = Vec::with_capacity(oriented.len());
    for (ei, o) in oriented.iter().enumerate() {
        let e = &input.edges[ei];
        let (src, dst) = match o {
            Some(o) if o.reversed => (e.to_cluster, e.from_cluster),
            Some(_) => (e.from_cluster, e.to_cluster),
            None => (None, None),
        };
        for c in [src, dst].into_iter().flatten() {
            if c < slot.len() && slot[c].is_none() {
                slot[c] = Some(used.len());
                used.push(c);
            }
        }
        ends.push((src, dst));
    }
    let elk = matches!(input.routing, EdgeRouting::Orthogonal(_));
    if used.is_empty() && elk {
        return rank_with_label_nodes(n, oriented, input, (hidden, hidden_edges), made);
    }
    if used.is_empty() {
        let mut ranks = rank_nodes(n, oriented);
        let layered = if elk {
            balance_ranks(&mut ranks, n, oriented, 1)
        } else {
            ranks.clone()
        };
        return (ranks, layered, Vec::new(), Vec::new());
    }
    let chains = cluster_chains(&input.clusters);
    let top = |c: usize| slot[c].map(|k| n + 2 * k);
    let bottom = |c: usize| slot[c].map(|k| n + 2 * k + 1);
    let total = n + 2 * used.len();
    let mut edges: Vec<Option<Oriented>> = Vec::new();
    let constraint = |from: usize, to: usize| {
        Some(Oriented {
            from,
            to,
            minlen: 0,
            reversed: false,
            weight: 0.0,
        })
    };
    for (v, nd) in input.nodes.iter().enumerate() {
        let Some(c) = nd.cluster else { continue };
        for &a in &chains[c] {
            if let (Some(t), Some(b)) = (top(a), bottom(a)) {
                edges.push(constraint(t, v));
                edges.push(constraint(v, b));
            }
        }
    }
    // The constraint graph must stay acyclic. Membership constraints and plain node
    // edges are acyclic on their own; each cluster-bound edge is then added only if it
    // keeps the graph acyclic (else it falls back to its plain member ends, or, if
    // even that closes a cycle, does not constrain the ranking), so one contradictory
    // edge does not cost every other edge its cluster constraint.
    let mut outs: Vec<Vec<usize>> = vec![Vec::new(); total];
    for o in edges.iter().flatten() {
        outs[o.from].push(o.to);
    }
    let reaches = |outs: &[Vec<usize>], from: usize, to: usize| -> bool {
        let mut seen = vec![false; outs.len()];
        let mut stack = vec![from];
        while let Some(v) = stack.pop() {
            if v == to {
                return true;
            }
            if std::mem::replace(&mut seen[v], true) {
                continue;
            }
            stack.extend(outs[v].iter().copied());
        }
        false
    };
    let mut push = |edges: &mut Vec<Option<Oriented>>, from: usize, to: usize, o: &Oriented| {
        if from != to && reaches(&outs, to, from) {
            return false;
        }
        outs[from].push(to);
        edges.push(Some(Oriented {
            from,
            to,
            minlen: o.minlen,
            reversed: false,
            weight: o.weight,
        }));
        true
    };
    for (o, (src, dst)) in oriented.iter().zip(&ends) {
        if let Some(o) = o.as_ref().filter(|_| src.is_none() && dst.is_none()) {
            push(&mut edges, o.from, o.to, o);
        }
    }
    for (o, end) in oriented.iter_mut().zip(ends.iter_mut()) {
        let Some(o) = o else { continue };
        let (src, dst) = *end;
        if src.is_none() && dst.is_none() {
            continue;
        }
        let from = src.and_then(bottom).unwrap_or(o.from);
        let to = dst.and_then(top).unwrap_or(o.to);
        if !push(&mut edges, from, to, o) {
            *end = (None, None);
            if !push(&mut edges, o.from, o.to, o) {
                // Even its plain ends close a cycle (the cluster constraints of other
                // edges already order them the other way): the edge runs backwards,
                // as a reversed edge, which the graph always takes (it is acyclic).
                // Left unranked, it could end up pointing up or within one layer.
                push(&mut edges, o.to, o.from, o);
                std::mem::swap(&mut o.from, &mut o.to);
                o.reversed = !o.reversed;
            }
        }
    }
    let mut all = rank_nodes(total, &edges);
    let layered = if elk {
        balance_ranks(&mut all, n, &edges, 1)
    } else {
        all.clone()
    };
    let min = all[..n].iter().copied().min().unwrap_or(0);
    let ranks: Vec<usize> = all[..n].iter().map(|r| r - min).collect();
    let layered: Vec<usize> = layered[..n].iter().map(|r| r.saturating_sub(min)).collect();
    let in_cluster = |v: usize, c: usize| {
        input.nodes[v]
            .cluster
            .is_some_and(|m| chains[m].contains(&c))
    };
    for (o, (src, dst)) in oriented.iter_mut().zip(&ends) {
        let Some(o) = o else { continue };
        if let Some(c) = *src {
            if let Some(v) = (0..n)
                .filter(|&v| in_cluster(v, c))
                .max_by_key(|&v| ranks[v])
            {
                o.from = v;
            }
        }
        if let Some(c) = *dst {
            if let Some(v) = (0..n)
                .filter(|&v| in_cluster(v, c))
                .min_by_key(|&v| ranks[v])
            {
                o.to = v;
            }
        }
    }
    (ranks, layered, Vec::new(), Vec::new())
}

/// Root-to-cluster ancestor chains (inclusive).
fn cluster_chains(clusters: &[LayoutCluster]) -> Vec<Vec<usize>> {
    (0..clusters.len())
        .map(|c| {
            let mut chain = vec![c];
            let mut cur = clusters[c].parent;
            while let Some(p) = cur {
                if chain.contains(&p) || chain.len() > clusters.len() {
                    break;
                }
                chain.push(p);
                cur = clusters[p].parent;
            }
            chain.reverse();
            chain
        })
        .collect()
}

fn lca(chains: &[Vec<usize>], a: Option<usize>, b: Option<usize>) -> Option<usize> {
    let (a, b) = (a?, b?);
    chains[a]
        .iter()
        .zip(&chains[b])
        .take_while(|(x, y)| x == y)
        .last()
        .map(|(x, _)| *x)
}

fn initial_order(
    verts: &[Vertex],
    n: usize,
    down: &[Vec<(usize, f64)>],
    max_rank: usize,
) -> Vec<Vec<usize>> {
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    let mut seen = vec![false; verts.len()];
    let mut roots: Vec<usize> = (0..n).collect();
    roots.sort_by_key(|&v| verts[v].rank);
    for root in roots {
        if seen[root] {
            continue;
        }
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if seen[v] {
                continue;
            }
            seen[v] = true;
            layers[verts[v].rank].push(v);
            for &(w, _) in down[v].iter().rev() {
                if !seen[w] {
                    stack.push(w);
                }
            }
        }
    }
    for (v, s) in seen.iter().enumerate() {
        if !s {
            layers[verts[v].rank].push(v);
        }
    }
    layers
}

struct OrderCtx<'a> {
    verts: &'a [Vertex],
    chains: &'a [Vec<usize>],
}

enum Unit {
    Single(usize),
    Group(usize, Vec<(usize, f64)>),
}

impl OrderCtx<'_> {
    fn chain(&self, v: usize) -> &[usize] {
        match self.verts[v].cluster {
            Some(c) => &self.chains[c],
            None => &[],
        }
    }

    /// Sort a layer by key while keeping every cluster contiguous and its border
    /// vertices at the block ends. Sibling clusters keep the order `before` holds
    /// from the layers arranged earlier in the same sweep, and this layer's order
    /// is added to it (dagre's `addSubgraphConstraints` / `resolveConflicts`):
    /// otherwise two clusters could swap sides between layers and their boxes
    /// overlap.
    fn arrange(
        &self,
        layer: &[usize],
        keys: &[f64],
        before: &mut Vec<(usize, usize)>,
    ) -> Vec<usize> {
        let items: Vec<(usize, f64)> = layer.iter().copied().zip(keys.iter().copied()).collect();
        let mut out = Vec::with_capacity(layer.len());
        self.arrange_rec(items, 0, &mut out, before);
        out
    }

    fn arrange_rec(
        &self,
        items: Vec<(usize, f64)>,
        depth: usize,
        out: &mut Vec<usize>,
        before: &mut Vec<(usize, usize)>,
    ) {
        let mut units: Vec<(u8, f64, Unit)> = Vec::new();
        for (v, k) in items {
            let chain = self.chain(v);
            // A vertex whose cluster chain goes deeper than `depth` belongs to the
            // sub-cluster at that depth; border vertices sit one level inside their
            // own cluster, where they are pinned to the block ends below.
            if chain.len() > depth {
                let c = chain[depth];
                match units
                    .iter_mut()
                    .find(|(_, _, u)| matches!(u, Unit::Group(g, _) if *g == c))
                {
                    Some((_, _, Unit::Group(_, members))) => members.push((v, k)),
                    _ => units.push((1, 0.0, Unit::Group(c, vec![(v, k)]))),
                }
            } else {
                let prio = match self.verts[v].kind {
                    Kind::Border {
                        side: Side::Left, ..
                    } => 0,
                    Kind::Border {
                        side: Side::Right, ..
                    } => 2,
                    _ => 1,
                };
                units.push((prio, k, Unit::Single(v)));
            }
        }
        for u in units.iter_mut() {
            if let Unit::Group(_, members) = &u.2 {
                u.1 = members.iter().map(|m| m.1).sum::<f64>() / members.len() as f64;
            }
        }
        units.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        // The sibling clusters, in key order, rearranged over their own slots so
        // that none comes before one the constraints put ahead of it.
        let slots: Vec<usize> = (0..units.len())
            .filter(|&i| matches!(units[i].2, Unit::Group(..)))
            .collect();
        let group = |u: &Unit| match u {
            Unit::Group(c, _) => *c,
            Unit::Single(_) => usize::MAX,
        };
        let mut pending: Vec<usize> = slots.iter().map(|&i| group(&units[i].2)).collect();
        let mut order = Vec::with_capacity(pending.len());
        while !pending.is_empty() {
            let k = (0..pending.len())
                .find(|&k| {
                    pending
                        .iter()
                        .all(|&h| h == pending[k] || !precedes(before, h, pending[k]))
                })
                .unwrap_or(0);
            order.push(pending.remove(k));
        }
        for w in order.windows(2) {
            if !before.contains(&(w[0], w[1])) {
                before.push((w[0], w[1]));
            }
        }
        let mut taken: Vec<Option<(u8, f64, Unit)>> = units.into_iter().map(Some).collect();
        let mut groups: Vec<Option<(u8, f64, Unit)>> =
            slots.iter().map(|&i| taken[i].take()).collect();
        for (&slot, c) in slots.iter().zip(&order) {
            let k = groups
                .iter()
                .position(|g| g.as_ref().is_some_and(|g| group(&g.2) == *c))
                .unwrap_or(0);
            taken[slot] = groups[k].take();
        }
        for (_, _, u) in taken.into_iter().flatten() {
            match u {
                Unit::Single(v) => out.push(v),
                Unit::Group(_, members) => self.arrange_rec(members, depth + 1, out, before),
            }
        }
    }
}

/// Whether the constraints put cluster `a` (transitively) before cluster `b`.
fn precedes(before: &[(usize, usize)], a: usize, b: usize) -> bool {
    let mut seen = vec![a];
    let mut stack = vec![a];
    while let Some(x) = stack.pop() {
        for &(p, q) in before {
            if p == x && !seen.contains(&q) {
                if q == b {
                    return true;
                }
                seen.push(q);
                stack.push(q);
            }
        }
    }
    false
}

fn sweep(
    layers: &mut [Vec<usize>],
    ctx: &OrderCtx,
    neigh: &[Vec<(usize, f64)>],
    downward: bool,
    nv: usize,
) {
    let mut pos = vec![0.0f64; nv];
    for layer in layers.iter() {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i as f64;
        }
    }
    let n_layers = layers.len();
    let order: Vec<usize> = if downward {
        (1..n_layers).collect()
    } else {
        (0..n_layers.saturating_sub(1)).rev().collect()
    };
    // The layer the sweep starts from keeps its order, but its sibling clusters'
    // order still binds the layers arranged after it: otherwise two clusters could
    // swap sides between it and the next layer.
    let mut before = Vec::new();
    let fixed = if downward {
        0
    } else {
        n_layers.saturating_sub(1)
    };
    if let Some(layer) = layers.get(fixed) {
        let keys: Vec<f64> = (0..layer.len()).map(|i| i as f64).collect();
        ctx.arrange(layer, &keys, &mut before);
    }
    for li in order {
        let layer = &layers[li];
        let keys: Vec<f64> = layer
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (sum, wsum) = neigh[v]
                    .iter()
                    .fold((0.0, 0.0), |(s, ws), &(u, w)| (s + pos[u] * w, ws + w));
                if wsum > 0.0 { sum / wsum } else { i as f64 }
            })
            .collect();
        let new_layer = ctx.arrange(layer, &keys, &mut before);
        for (i, &v) in new_layer.iter().enumerate() {
            pos[v] = i as f64;
        }
        layers[li] = new_layer;
    }
}

/// Weighted crossing count between consecutive layers (Barth–Jünger–Mutzel with a Fenwick tree).
fn total_crossings(layers: &[Vec<usize>], down: &[Vec<(usize, f64)>], nv: usize) -> f64 {
    let mut pos = vec![0usize; nv];
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
    let mut total = 0.0;
    for li in 0..layers.len().saturating_sub(1) {
        let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
        for &v in &layers[li] {
            for &(w, wt) in &down[v] {
                pairs.push((pos[v], pos[w], wt));
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let size = layers[li + 1].len() + 1;
        let mut tree = vec![0.0f64; size + 1];
        let mut inserted = 0.0;
        for &(_, p, wt) in &pairs {
            // weight of already-inserted edges ending strictly right of p
            let mut i = p + 1;
            let mut le = 0.0;
            while i > 0 {
                le += tree[i];
                i -= i & i.wrapping_neg();
            }
            total += (inserted - le) * wt;
            let mut i = p + 1;
            while i <= size {
                tree[i] += wt;
                i += i & i.wrapping_neg();
            }
            inserted += wt;
        }
    }
    total
}

/// Cross-axis coordinates (Brandes–Köpf, as dagre does it; see `bk.rs`).
fn assign_x(
    layers: &[Vec<usize>],
    verts: &[Vertex],
    links: &[(usize, usize, f64)],
    borders: &[Vec<(usize, usize)>],
    gap: &dyn Fn(usize, usize) -> f64,
) -> Vec<f64> {
    let nv = verts.len();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut link = |a: usize, b: usize| {
        if !succs[a].contains(&b) {
            succs[a].push(b);
            preds[b].push(a);
        }
    };
    for &(a, b, _) in links {
        link(a, b);
    }
    // dagre chains each cluster's border vertices rank to rank.
    for bs in borders {
        for pair in bs.windows(2) {
            link(pair[0].0, pair[1].0);
            link(pair[0].1, pair[1].1);
        }
    }
    let dummy: Vec<bool> = verts.iter().map(|v| v.kind != Kind::Real).collect();
    let border: Vec<Option<bk::BorderSide>> = verts
        .iter()
        .map(|v| match v.kind {
            Kind::Border {
                side: Side::Left, ..
            } => Some(bk::BorderSide::Left),
            Kind::Border {
                side: Side::Right, ..
            } => Some(bk::BorderSide::Right),
            _ => None,
        })
        .collect();
    let width: Vec<f64> = verts
        .iter()
        .map(|v| match v.kind {
            Kind::Real | Kind::Dummy { labeled: true } => v.w,
            _ => 0.0,
        })
        .collect();
    let graph = bk::LayerGraph {
        layers,
        preds: &preds,
        succs: &succs,
        dummy: &dummy,
        border: &border,
        width: &width,
    };
    let mut x = bk::position_x(&graph, gap);
    separate_cluster_frames(layers, borders, gap, &mut x);
    // Normalise so the leftmost extent sits at zero.
    let min = (0..nv)
        .map(|v| x[v] - width[v] / 2.0)
        .fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        for xv in x.iter_mut() {
            *xv -= min;
        }
    }
    x
}

/// Make cluster frames hold apart over all their layers, not only layer by layer.
///
/// Brandes–Köpf keeps each layer's vertices (cluster borders included) apart,
/// but a cluster is drawn as one rectangle from its leftmost left border to its
/// rightmost right border over all its layers. When its borders zig-zag (a
/// cluster pulled one way on some layers and another way on others, typically
/// by edges to the whole cluster that contradict each other), a sibling, or a
/// node outside it, can fall inside that rectangle on another layer.
///
/// Here each cluster side becomes one variable (at its outermost border: the
/// rectangle's side), every layer's neighbours give a separation constraint
/// between their variables, and, in topological order, every variable is pushed
/// right just as far as its constraints need. Layouts that already hold apart do
/// not move. (Sibling clusters keep one order over all the layers they share, see
/// `OrderCtx::arrange`, so the constraints have no cycle; if they had one, the
/// layout would stay as it is.)
fn separate_cluster_frames(
    layers: &[Vec<usize>],
    borders: &[Vec<(usize, usize)>],
    gap: &dyn Fn(usize, usize) -> f64,
    x: &mut [f64],
) {
    if borders.iter().all(|b| b.is_empty()) {
        return;
    }
    let nv = x.len();
    // Variable per vertex: itself, or its cluster side (`nv + 2c` / `nv + 2c + 1`).
    let mut var: Vec<usize> = (0..nv).collect();
    let mut val: Vec<f64> = x.to_vec();
    val.resize(nv + 2 * borders.len(), 0.0);
    for (c, bs) in borders.iter().enumerate() {
        if bs.is_empty() {
            continue;
        }
        let (l, r) = (nv + 2 * c, nv + 2 * c + 1);
        val[l] = bs.iter().map(|&(a, _)| x[a]).fold(f64::INFINITY, f64::min);
        val[r] = bs
            .iter()
            .map(|&(_, b)| x[b])
            .fold(f64::NEG_INFINITY, f64::max);
        for &(a, b) in bs {
            var[a] = l;
            var[b] = r;
        }
    }
    let nvar = val.len();
    let mut out: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nvar];
    let mut indeg = vec![0usize; nvar];
    for layer in layers {
        for w in layer.windows(2) {
            let (a, b) = (var[w[0]], var[w[1]]);
            out[a].push((b, gap(w[0], w[1])));
            indeg[b] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..nvar).filter(|&v| indeg[v] == 0).collect();
    let mut seen = 0;
    let mut moved = false;
    while let Some(a) = queue.pop() {
        seen += 1;
        for &(b, g) in &out[a] {
            if val[a] + g > val[b] + 1e-9 {
                val[b] = val[a] + g;
                moved = true;
            }
            indeg[b] -= 1;
            if indeg[b] == 0 {
                queue.push(b);
            }
        }
    }
    if seen < nvar || !moved {
        return;
    }
    // Write back. A border stays where it was unless its side moved past it (the
    // left side) or what precedes it on its layer pushed it (the right side): the
    // rectangle's sides are still the variables, and every layer stays apart.
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            let u = var[v];
            x[v] = if u == v {
                val[v]
            } else if (u - nv) % 2 == 0 {
                x[v].max(val[u])
            } else {
                let after_prev = i
                    .checked_sub(1)
                    .map_or(f64::NEG_INFINITY, |j| x[layer[j]] + gap(layer[j], v));
                x[v].max(after_prev)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(cluster: Option<usize>) -> LayoutNode {
        LayoutNode {
            width: 80.0,
            height: 40.0,
            cluster,
        }
    }

    fn edge(from: usize, to: usize) -> LayoutEdge {
        LayoutEdge::new(from, to, 1, None)
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
            cluster_padding: 8.0,
            inner_node_sep: 50.0,
            inner_rank_sep: 24.0,
            routing: EdgeRouting::Splines,
            dagre_groups: false,
        }
    }

    fn rects(inp: &LayoutInput, out: &LayoutOutput) -> Vec<Rect> {
        inp.nodes
            .iter()
            .zip(&out.nodes)
            .map(|(n, p)| Rect::centered(*p, n.width, n.height))
            .collect()
    }

    /// As in dagre (and so mermaid): every edge, labelled or not, gets a
    /// midpoint vertex, and Brandes–Köpf lines that midpoint up with one end
    /// rather than halfway, so a basis spline through the points bends
    /// (the "S" curve) instead of drawing a straight diagonal.
    #[test]
    fn fan_out_edges_bend_like_dagre() {
        let inp = input(
            vec![node(None), node(None), node(None), node(None)],
            vec![edge(0, 1), edge(0, 2), edge(0, 3)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        for (ei, route) in out.edges.iter().enumerate() {
            assert_eq!(route.points.len(), 3, "edge {ei} has a midpoint");
            let [a, m, b] = [route.points[0], route.points[1], route.points[2]];
            assert!(
                (m.x - a.x).abs() < 1e-6 || (m.x - b.x).abs() < 1e-6,
                "edge {ei}: midpoint {m:?} is aligned with an end ({a:?}, {b:?})"
            );
            assert!(a.y < m.y && m.y < b.y);
        }
        // The outer edges are not straight lines.
        let r = &out.edges[0].points;
        assert!((r[0].x - r[2].x).abs() > 1.0);
    }

    /// Orthogonal routes: right angles only, port to port on the facing sides, never
    /// through another node, and stacked slots `edge_edge` apart.
    #[test]
    fn orthogonal_routes_avoid_nodes() {
        for dir in [
            Direction::TopBottom,
            Direction::LeftRight,
            Direction::BottomTop,
            Direction::RightLeft,
        ] {
            let mut inp = input(
                (0..6).map(|_| node(None)).collect(),
                vec![
                    edge(0, 1),
                    edge(0, 2),
                    edge(0, 3),
                    edge(1, 4),
                    edge(2, 4),
                    edge(3, 4),
                    edge(0, 4),
                    edge(4, 5),
                    edge(5, 0),
                    LayoutEdge::new(1, 5, 1, Some((40.0, 20.0))),
                    edge(2, 2),
                ],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            let boxes = rects(&inp, &out);
            for (ei, (e, r)) in inp.edges.iter().zip(&out.edges).enumerate() {
                assert!(ortho::is_orthogonal(&r.points), "{dir:?} edge {ei}: {r:?}");
                // Ends on the bounding boxes of their nodes.
                let on = |p: Point, b: &Rect| {
                    ((p.x - b.x).abs() < 1e-6 || (p.x - b.right()).abs() < 1e-6)
                        && p.y >= b.y - 1e-6
                        && p.y <= b.bottom() + 1e-6
                        || ((p.y - b.y).abs() < 1e-6 || (p.y - b.bottom()).abs() < 1e-6)
                            && p.x >= b.x - 1e-6
                            && p.x <= b.right() + 1e-6
                };
                assert!(on(r.points[0], &boxes[e.from]), "{dir:?} edge {ei} start");
                assert!(
                    on(r.points[r.points.len() - 1], &boxes[e.to]),
                    "{dir:?} edge {ei} end"
                );
                // No segment enters any node's interior.
                for w in r.points.windows(2) {
                    for (v, b) in boxes.iter().enumerate() {
                        let inner = b.pad(-1.0);
                        let (lo, hi) = (
                            Point::new(w[0].x.min(w[1].x), w[0].y.min(w[1].y)),
                            Point::new(w[0].x.max(w[1].x), w[0].y.max(w[1].y)),
                        );
                        let hits = lo.x < inner.right()
                            && hi.x > inner.x
                            && lo.y < inner.bottom()
                            && hi.y > inner.y;
                        assert!(!hits, "{dir:?} edge {ei} runs through node {v}: {w:?}");
                    }
                }
            }
            // The labelled edge passes through its label (within ELK's offset of
            // under a pixel, see `elk_label_centre`).
            let r = &out.edges[9];
            let l = r.label.unwrap();
            assert!(r.points.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                (a.x - l.x).abs() < 1.0
                    && (a.x - b.x).abs() < 1e-6
                    && (a.y - l.y) * (b.y - l.y) <= 0.0
                    || (a.y - l.y).abs() < 1.0
                        && (a.y - b.y).abs() < 1e-6
                        && (a.x - l.x) * (b.x - l.x) <= 0.0
            }));
        }
    }

    /// A labelled edge bends before its label and runs straight from the label into
    /// its target (as mermaid's ELK output does), so the arrowhead (or an ER crow's
    /// foot) is not drawn over a bend.
    #[test]
    fn orthogonal_label_is_in_line_with_the_target_port() {
        for dir in [Direction::TopBottom, Direction::LeftRight] {
            let mut wide = node(None);
            wide.width = 300.0;
            wide.height = 300.0;
            let mut inp = input(
                vec![wide, node(None), node(None), node(None)],
                vec![
                    LayoutEdge::new(0, 1, 1, Some((30.0, 20.0))),
                    LayoutEdge::new(0, 2, 1, Some((30.0, 20.0))),
                    LayoutEdge::new(0, 3, 1, Some((30.0, 20.0))),
                ],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            for (ei, r) in out.edges.iter().enumerate() {
                let l = r.label.unwrap();
                let n = r.points.len();
                let (a, b) = (r.points[n - 2], r.points[n - 1]);
                // (ELK sets a label up to a pixel off its edge: `elk_label_centre`.)
                let along = if dir.is_horizontal() {
                    (a.y - b.y).abs() < 1e-6 && (l.y - b.y).abs() < 1.0 && l.x > a.x - 1e-6
                } else {
                    (a.x - b.x).abs() < 1e-6 && (l.x - b.x).abs() < 1.0 && l.y > a.y - 1e-6
                };
                assert!(
                    along,
                    "{dir:?} edge {ei}: label {l:?} not on the last leg {r:?}"
                );
            }
        }
    }

    /// Several labelled self-loops on one node stand off one beyond the other's
    /// label, and the node beside them is pushed clear of the loops and labels.
    /// (On a short side an outer loop's legs pass over an inner loop's label, as in
    /// ELK, whose ports are spread over the side whatever the labels.)
    #[test]
    fn orthogonal_self_loops_and_labels_do_not_overlap() {
        for dir in [Direction::TopBottom, Direction::LeftRight] {
            let mut inp = input(
                vec![node(None), node(None), node(None)],
                vec![
                    edge(0, 1),
                    edge(0, 2),
                    LayoutEdge::new(2, 2, 1, Some((60.0, 20.0))),
                    LayoutEdge::new(2, 2, 1, Some((50.0, 20.0))),
                    edge(2, 2),
                ],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            let boxes = rects(&inp, &out);
            let mut taken: Vec<Rect> = boxes.clone();
            for (e, r) in inp.edges.iter().zip(&out.edges).skip(2) {
                for w in r.points.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let seg = Rect::new(
                        a.x.min(b.x),
                        a.y.min(b.y),
                        (a.x - b.x).abs(),
                        (a.y - b.y).abs(),
                    );
                    for (k, t) in boxes.iter().enumerate() {
                        if k != 2 {
                            assert!(
                                !seg.intersects(&t.pad(-0.5)),
                                "{dir:?}: loop {r:?} over {t:?}"
                            );
                        }
                    }
                }
                if let (Some((w, h)), Some(l)) = (e.label, r.label) {
                    let lr = Rect::centered(l, w, h);
                    for t in &taken {
                        assert!(
                            !lr.pad(-0.5).intersects(t),
                            "{dir:?}: label {lr:?} over {t:?}"
                        );
                    }
                    taken.push(lr);
                }
            }
        }
    }

    /// LR: `s --> a`, `a --> b`, `s --> b` with `a`, `b` in a titled cluster. Under ELK
    /// the long edge enters the cluster across its side facing `s` and runs inside it,
    /// rather than staying outside until `b`'s rank and dropping in across the
    /// cluster's top border, where its title is. (Its bottom border may be
    /// crossed: mermaid pulls a frame in to its contents, leaving the lanes of
    /// edges coming from outside below it.)
    #[test]
    fn orthogonal_edge_into_cluster_enters_across_its_side() {
        let mut inp = input(
            vec![node(None), node(Some(0)), node(Some(0)), node(None)],
            vec![edge(0, 1), edge(1, 2), edge(0, 2), edge(3, 1), edge(3, 2)],
            vec![LayoutCluster::new(None, 24.0)],
            Direction::LeftRight,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        let c = out.clusters[0];
        for (ei, r) in out.edges.iter().enumerate() {
            for w in r.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                let across = |y: f64| a.y.min(b.y) < y - 0.5 && a.y.max(b.y) > y + 0.5;
                let inside = a.x > c.x + 0.5 && a.x < c.right() - 0.5;
                assert!(
                    !(inside && across(c.y)),
                    "edge {ei} crosses the top of {c:?}: {:?}",
                    r.points
                );
            }
        }
    }

    /// `a --> b`, `b --> a` between nodes of slightly different widths: their ports
    /// are less than the jog threshold apart, so the route gets no jog, and it used
    /// to end at the far port anyway, slightly slanted.
    #[test]
    fn orthogonal_run_without_a_jog_is_straight() {
        let size = |w, h| LayoutNode {
            width: w,
            height: h,
            cluster: None,
        };
        let mut inp = input(
            vec![size(113.0, 31.0), size(42.0, 47.0), size(43.0, 44.0)],
            vec![edge(1, 2), edge(2, 1)],
            vec![],
            Direction::TopBottom,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        for r in &out.edges {
            for w in r.points.windows(2) {
                assert!(
                    (w[0].x - w[1].x).abs() < 1e-9 || (w[0].y - w[1].y).abs() < 1e-9,
                    "slanted: {:?}",
                    r.points
                );
            }
        }
    }

    /// Two sibling clusters in two layers: `c1 = {0, 2, 4, 6}`, `c0 = {1, 3, 5}`, with
    /// `2 --> 1` and `0 --> 6`. Each layer was sorted on its own, so the barycenters
    /// put c1 left of c0 in the first layer and right of it in the second, and the
    /// two cluster boxes overlapped. Sibling clusters now keep one order over the
    /// layers.
    #[test]
    fn sibling_clusters_keep_their_order_over_layers() {
        for routing in [
            EdgeRouting::Splines,
            EdgeRouting::Orthogonal(OrthoSpacing::ROOT),
        ] {
            let c = [1, 0, 1, 0, 1, 0, 1];
            let mut inp = input(
                c.iter().map(|&c| node(Some(c))).collect(),
                vec![edge(2, 1), edge(0, 6)],
                vec![
                    LayoutCluster::new(None, 24.0),
                    LayoutCluster::new(None, 24.0),
                ],
                Direction::TopBottom,
            );
            inp.routing = routing;
            let out = layout(&inp);
            let (a, b) = (out.clusters[0], out.clusters[1]);
            assert!(!a.intersects(&b), "{routing:?}: {a:?} {b:?}");
            for (k, r) in rects(&inp, &out).iter().enumerate() {
                let other = out.clusters[1 - c[k]];
                assert!(!r.intersects(&other), "{routing:?}: node {k} in {other:?}");
            }
        }
    }

    /// Clusters whose borders zig-zag over their layers: each layer is apart, but
    /// cluster 0 reaches further right on layer 1 than cluster 1 starts on layer 0.
    /// The frames are pushed apart; a layout already apart stays as it is.
    #[test]
    fn cluster_frames_are_apart_over_all_their_layers() {
        // Vertices: per layer [0.left, member, 0.right, 1.left, member, 1.right].
        let layers = vec![vec![0, 1, 2, 3, 4, 5], vec![6, 7, 8, 9, 10, 11]];
        let borders = vec![vec![(0, 2), (6, 8)], vec![(3, 5), (9, 11)]];
        let gap = |_: usize, _: usize| 10.0;
        let mut x = vec![
            0.0, 10.0, 20.0, 30.0, 40.0, 50.0, // layer 0
            100.0, 110.0, 120.0, 130.0, 140.0, 150.0, // layer 1
        ];
        separate_cluster_frames(&layers, &borders, &gap, &mut x);
        let right0 = x[2].max(x[8]);
        let left1 = x[3].min(x[9]);
        assert!(left1 >= right0 + 10.0, "{x:?}");
        for layer in &layers {
            for w in layer.windows(2) {
                assert!(x[w[1]] - x[w[0]] >= 10.0 - 1e-9, "{x:?}");
            }
        }
        // Already apart: nothing moves.
        let mut apart = vec![
            0.0, 10.0, 20.0, 130.0, 140.0, 150.0, 0.0, 10.0, 30.0, 140.0, 150.0, 160.0,
        ];
        let before = apart.clone();
        separate_cluster_frames(&layers, &borders, &gap, &mut apart);
        assert_eq!(apart, before);
    }

    /// The layer a sweep starts from is not rearranged, but its sibling clusters'
    /// order binds the next layers: cluster 0 left of cluster 2 on layer 0 stays
    /// left of it on layer 1, though an edge pulls cluster 2's member there left.
    #[test]
    fn sweeps_keep_sibling_order_from_the_fixed_layer() {
        for routing in [
            EdgeRouting::Splines,
            EdgeRouting::Orthogonal(OrthoSpacing::ROOT),
        ] {
            // x, y, z, c0n0..2, c1n0, c2n0, c2n1, c3n0..2 (the fuzzer's case).
            let cl = [
                None,
                None,
                None,
                Some(0),
                Some(0),
                Some(0),
                Some(1),
                Some(2),
                Some(2),
                Some(3),
                Some(3),
                Some(3),
            ];
            let mut inp = input(
                cl.iter().map(|&c| node(c)).collect(),
                vec![
                    edge(3, 4),
                    edge(4, 5),
                    edge(9, 10),
                    edge(10, 11),
                    edge(9, 4),
                    edge(0, 7),
                ],
                (0..4).map(|_| LayoutCluster::new(None, 24.0)).collect(),
                Direction::TopBottom,
            );
            inp.routing = routing;
            let out = layout(&inp);
            for a in 0..4 {
                for b in a + 1..4 {
                    let (ra, rb) = (out.clusters[a], out.clusters[b]);
                    assert!(!ra.intersects(&rb), "{routing:?}: {a} {b}: {ra:?} {rb:?}");
                }
            }
            for (k, r) in rects(&inp, &out).iter().enumerate() {
                for c in (0..4).filter(|&c| cl[k] != Some(c)) {
                    let f = out.clusters[c];
                    assert!(!r.intersects(&f), "{routing:?}: node {k} in {c}");
                }
            }
        }
    }

    /// Unlabelled self-loops on one node nest: each one's legs stand outside the
    /// previous one's, so their ends and arrowheads do not coincide.
    #[test]
    fn orthogonal_self_loops_nest_without_shared_ends() {
        for dir in [Direction::TopBottom, Direction::LeftRight] {
            let mut inp = input(
                vec![node(None)],
                vec![edge(0, 0), edge(0, 0), edge(0, 0)],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            for pair in out.edges.windows(2) {
                let (a, b) = (&pair[0].points, &pair[1].points);
                for (p, q) in [(a[0], b[0]), (a[a.len() - 1], b[b.len() - 1])] {
                    assert!(p.dist(q) > 2.0, "{dir:?}: shared end {p:?} {q:?}");
                }
            }
        }
    }

    /// ELK's self-loop geometry (elkjs, mermaid's options): a 100 x 81 node with
    /// loops labelled 50 x 21, unlabelled and 30 x 42 has its six ports spread
    /// 57/7 apart over the west side inside the 12px surrounding, the first loop on
    /// the middle pair; legs 20, 94 and 114 off the side; labels 4 beyond their leg.
    /// Laid out to the right, the loops stand on the north side the same way.
    #[test]
    fn orthogonal_self_loops_match_elk() {
        for dir in [Direction::TopBottom, Direction::LeftRight] {
            let horizontal = dir.is_horizontal();
            let mut inp = input(
                vec![LayoutNode {
                    width: 100.0,
                    height: 81.0,
                    cluster: None,
                }],
                vec![
                    LayoutEdge::new(0, 0, 1, Some((50.0, 21.0))),
                    edge(0, 0),
                    LayoutEdge::new(0, 0, 1, Some((30.0, 42.0))),
                ],
                vec![],
                dir,
            );
            inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
            let out = layout(&inp);
            let c = out.nodes[0];
            // (across the side, out from it): TB's west side, LR's north side.
            let rel = |p: Point| {
                if horizontal {
                    (p.x - c.x, c.y - 40.5 - p.y)
                } else {
                    (p.y - c.y, c.x - 50.0 - p.x)
                }
            };
            let side = if horizontal { 100.0 } else { 81.0 };
            let gap = (side - 24.0) / 7.0;
            let outs = if horizontal {
                [20.0, 65.0, 85.0]
            } else {
                [20.0, 94.0, 114.0]
            };
            for (i, r) in out.edges.iter().enumerate() {
                let half = gap * (2 * i + 1) as f64 / 2.0;
                let pts: Vec<(f64, f64)> = r.points.iter().map(|&p| rel(p)).collect();
                assert_eq!(pts.len(), 4, "{dir:?} {i}: {pts:?}");
                let expect = [(-half, 0.0), (-half, outs[i]), (half, outs[i]), (half, 0.0)];
                for (p, q) in pts.iter().zip(expect) {
                    let sorted = (p.0.abs(), p.1);
                    assert!(
                        (sorted.0 - q.0.abs()).abs() < 1e-6 && (sorted.1 - q.1).abs() < 1e-6,
                        "{dir:?} loop {i}: {pts:?} vs {expect:?}"
                    );
                }
            }
            for (i, across) in [
                (0, if horizontal { 21.0 } else { 50.0 }),
                (2, if horizontal { 42.0 } else { 30.0 }),
            ] {
                let l = rel(out.edges[i].label.expect("label"));
                assert!(l.0.abs() < 1e-6, "{dir:?} label {i}: {l:?}");
                assert!(
                    (l.1 - (outs[i] + 4.0 + across / 2.0)).abs() < 1e-6,
                    "{dir:?} label {i}: {l:?}"
                );
            }
        }
    }

    /// ELK carries a labelled edge through its label dummy half the label's
    /// rounded-up width in (vertical), or half its rounded-up height plus the
    /// edge's thickness (horizontal): the label sits 0.4 left of a vertical edge
    /// for a 40.2 x 21 label and 0.85 above a horizontal one for 40 x 21.3.
    #[test]
    fn elk_label_sits_off_its_edge_as_in_elk() {
        for (dir, size, off) in [
            (Direction::TopBottom, (40.2, 21.0), Point::new(-0.4, 0.0)),
            (Direction::BottomTop, (40.2, 21.0), Point::new(-0.4, 0.0)),
            (Direction::LeftRight, (40.0, 21.3), Point::new(0.0, -0.85)),
            (Direction::RightLeft, (40.0, 21.0), Point::new(0.0, -0.5)),
        ] {
            let out = layout(&elk(input(
                vec![node(None); 2],
                vec![LayoutEdge::new(0, 1, 1, Some(size))],
                vec![],
                dir,
            )));
            let r = &out.edges[0];
            let l = r.label.expect("label");
            let a = r.points[0];
            let d = l.minus(Point::new(
                if dir.is_horizontal() { l.x } else { a.x },
                if dir.is_horizontal() { a.y } else { l.y },
            ));
            assert!(d.dist(off) < 1e-9, "{dir:?}: {d:?} vs {off:?}");
        }
    }

    /// An inverted port's in-layer edge split by the router (a critical cycle in
    /// its channel) is drawn as ELK's `calculateBendPoints` draws any split segment:
    /// from its source along its own slot to the split position, across to the
    /// partner's slot, and on to its target. Here a port on node 0's side towards
    /// the previous layer (before the first one) leads to the dummy beside it.
    #[test]
    fn inverted_port_in_layer_edge_follows_its_split() {
        let vert = |kind, rank| Vertex {
            kind,
            rank,
            w: 40.0,
            h: 40.0,
            cluster: None,
        };
        let verts = vec![
            vert(Kind::Real, 0),
            vert(Kind::Real, 2),
            vert(Kind::Dummy { labeled: false }, 0),
        ];
        let route = InLayerRoute {
            edge: 0,
            node: 0,
            dummy: 2,
            port: 10.0,
            at: 100.0,
            before: true,
            channel: None,
            slot: 0,
            split: Some((60.0, 1)),
        };
        let plan = OrthoPlan {
            stops: vec![vec![(2, 100.0), (1, 100.0)]],
            jogs: vec![vec![None]],
            splits: vec![vec![None]],
            channels: vec![None; 3],
            spacing: OrthoSpacing::ROOT,
            xs: vec![10.0, 100.0, 100.0],
            in_layer: vec![route],
            outer: (2, 0),
        };
        let rank_y = [0.0, 50.0, 100.0];
        let vy = [0.0, 100.0, 0.0];
        let rank_h = [40.0, 0.0, 40.0];
        let pts = plan.route(0, &verts, &rank_y, &vy, &rank_h, &[0.0; 3], &[0.0; 3]);
        // Two slots before the first layer (top at -20): 20 clear of it, 20 apart.
        let (y0, y1) = (-20.0 - 20.0 - 20.0, -20.0 - 20.0);
        let expect = [
            Point::new(10.0, -20.0),
            Point::new(10.0, y0),
            Point::new(60.0, y0),
            Point::new(60.0, y1),
            Point::new(100.0, y1),
            Point::new(100.0, 80.0),
        ];
        assert_eq!(pts.len(), expect.len(), "{pts:?}");
        for (p, q) in pts.iter().zip(expect) {
            assert!(p.dist(q) < 1e-9, "{pts:?}");
        }
    }

    /// A self-loop label wider than its node (along the rank axis) gets a layer as
    /// deep as itself, clear of the labels in the next layer.
    #[test]
    fn orthogonal_self_loop_label_keeps_its_layer_clear() {
        let mut inp = input(
            vec![node(None), node(None)],
            vec![
                LayoutEdge::new(0, 1, 1, Some((400.0, 20.0))),
                LayoutEdge::new(0, 0, 1, Some((400.0, 20.0))),
            ],
            vec![],
            Direction::LeftRight,
        );
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        let out = layout(&inp);
        let a = Rect::centered(out.edges[0].label.unwrap(), 400.0, 20.0);
        let b = Rect::centered(out.edges[1].label.unwrap(), 400.0, 20.0);
        assert!(!a.intersects(&b), "{a:?} {b:?}");
    }

    /// `n4 --> s1` and `s1 --> n4` with `s1` nested in `s0`: the cluster constraints
    /// of the first edge order `n4` above all of `s1`, so the second cannot run down
    /// from any member to `n4`, not even from its plain ends. It used to be left out
    /// of the ranking and pointed upward across two layers without dummies, which
    /// broke the coordinate assignment (the two members overlapped) and, in a debug
    /// build, underflowed the rank span. It is now ranked as a reversed edge.
    #[test]
    fn contradictory_cluster_edge_runs_reversed() {
        let mut nd = node(Some(1));
        nd.width = 152.0;
        let mut into = LayoutEdge::new(2, 0, 1, None);
        into.to_cluster = Some(1);
        let mut out_of = LayoutEdge::new(1, 2, 1, None);
        out_of.from_cluster = Some(1);
        for routing in [
            EdgeRouting::Splines,
            EdgeRouting::Orthogonal(OrthoSpacing::ROOT),
        ] {
            let mut inp = input(
                vec![nd.clone(), nd.clone(), node(None)],
                vec![into.clone(), out_of.clone()],
                vec![
                    LayoutCluster::new(None, 24.0),
                    LayoutCluster::new(Some(0), 24.0),
                ],
                Direction::TopBottom,
            );
            inp.routing = routing;
            let out = layout(&inp);
            let b = rects(&inp, &out);
            assert!(!b[0].intersects(&b[1]), "{routing:?}: {b:?}");
            let r = &out.edges[1].points;
            // Drawn from its source (in the cluster, below) up to n4.
            assert!(r[0].y > r[r.len() - 1].y, "{routing:?}: {r:?}");
        }
    }

    #[test]
    fn chain_is_vertical_in_tb() {
        let inp = input(
            vec![node(None), node(None), node(None)],
            vec![edge(0, 1), edge(1, 2)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert!(out.nodes[0].y < out.nodes[1].y && out.nodes[1].y < out.nodes[2].y);
        assert!((out.nodes[0].x - out.nodes[2].x).abs() < 1e-6);
    }

    #[test]
    fn lr_flows_horizontally_and_rl_reverses() {
        let lr = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::LeftRight,
        ));
        assert!(lr.nodes[0].x < lr.nodes[1].x);
        let rl = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::RightLeft,
        ));
        assert!(rl.nodes[0].x > rl.nodes[1].x);
        let bt = layout(&input(
            vec![node(None), node(None)],
            vec![edge(0, 1)],
            vec![],
            Direction::BottomTop,
        ));
        assert!(bt.nodes[0].y > bt.nodes[1].y);
    }

    #[test]
    fn cycles_are_handled() {
        let inp = input(
            vec![node(None), node(None), node(None)],
            vec![edge(0, 1), edge(1, 2), edge(2, 0), edge(1, 1)],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        assert_eq!(out.edges.len(), 4);
        assert!(out.edges.iter().all(|e| e.points.len() >= 2));
    }

    #[test]
    fn no_node_overlaps_in_dense_graph() {
        let n = 30;
        let nodes = (0..n).map(|_| node(None)).collect();
        let mut edges = Vec::new();
        for i in 0..n {
            edges.push(edge(i, (i * 7 + 3) % n));
            edges.push(edge(i, (i * 11 + 5) % n));
        }
        let inp = input(nodes, edges, vec![], Direction::TopBottom);
        let out = layout(&inp);
        let rs = rects(&inp, &out);
        for i in 0..n {
            for j in i + 1..n {
                assert!(!rs[i].intersects(&rs[j]), "nodes {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn clusters_contain_members_and_exclude_others() {
        let nodes = vec![
            node(Some(0)),
            node(Some(0)),
            node(None),
            node(Some(1)),
            node(None),
        ];
        let edges = vec![edge(0, 1), edge(2, 0), edge(1, 3), edge(2, 4), edge(4, 3)];
        let clusters = vec![
            LayoutCluster::new(None, 24.0),
            LayoutCluster::new(Some(0), 24.0),
        ];
        let inp = input(nodes, edges, clusters, Direction::TopBottom);
        let out = layout(&inp);
        let rs = rects(&inp, &out);
        for (i, nd) in inp.nodes.iter().enumerate() {
            if nd.cluster.is_some() {
                let c = &out.clusters[0];
                assert!(
                    c.contains(Point::new(rs[i].x, rs[i].y))
                        && c.contains(Point::new(rs[i].right(), rs[i].bottom())),
                    "member {i} outside cluster"
                );
            } else {
                assert!(
                    !out.clusters[0].intersects(&rs[i]),
                    "non-member {i} overlaps cluster"
                );
            }
        }
    }

    #[test]
    fn one_contradictory_cluster_edge_keeps_the_other_constraints() {
        // Cluster 0 = {0 → 1}. `C → 3` is given with the first member as its
        // representative, so only the cluster constraint puts 3 below 1. The edge
        // `C → 0` (into its own member) is contradictory; it must not cost `C → 3`
        // its constraint.
        let cl = |from: usize, to: usize, fc: Option<usize>, tc: Option<usize>| LayoutEdge {
            from_cluster: fc,
            to_cluster: tc,
            ..LayoutEdge::new(from, to, 1, None)
        };
        let nodes = vec![node(Some(0)), node(Some(0)), node(None), node(None)];
        let edges = vec![
            edge(0, 1),
            cl(2, 1, None, Some(0)),
            cl(0, 3, Some(0), None),
            cl(1, 0, Some(0), None),
        ];
        let inp = input(
            nodes,
            edges,
            vec![LayoutCluster::new(None, 24.0)],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let y = |i: usize| out.nodes[i].y;
        assert!(y(2) < y(0) && y(0) < y(1), "{:?}", out.nodes);
        assert!(
            y(3) > y(1),
            "C → 3 still ranks 3 after the whole cluster: {:?}",
            out.nodes
        );
    }

    #[test]
    fn labels_get_space() {
        let inp = input(
            vec![node(None), node(None)],
            vec![LayoutEdge::new(0, 1, 1, Some((60.0, 24.0)))],
            vec![],
            Direction::TopBottom,
        );
        let out = layout(&inp);
        let l = out.edges[0].label.expect("label position");
        assert!(l.y > out.nodes[0].y + 20.0 && l.y < out.nodes[1].y - 20.0);
    }

    fn elk(mut inp: LayoutInput) -> LayoutInput {
        inp.routing = EdgeRouting::Orthogonal(OrthoSpacing::ROOT);
        inp
    }

    fn reversed(o: &[Option<Oriented>]) -> Vec<usize> {
        (0..o.len())
            .filter(|&i| o[i].as_ref().is_some_and(|o| o.reversed))
            .collect()
    }

    fn edges_of(pairs: &[(usize, usize)]) -> Vec<LayoutEdge> {
        pairs.iter().map(|&(a, b)| edge(a, b)).collect()
    }

    /// Nodes sorted top to bottom.
    fn column(out: &LayoutOutput) -> Vec<usize> {
        let mut v: Vec<usize> = (0..out.nodes.len()).collect();
        v.sort_by(|&a, &b| out.nodes[a].y.total_cmp(&out.nodes[b].y));
        v
    }

    /// dagre starts its search at the first node; ELK's depth-first cycle breaker
    /// at the sources (here only node 2), so it walks 2 → 3 → 1 → 0 → 4 and
    /// reverses the four edges closing a cycle on that path: the graph becomes a
    /// column (as mermaid draws it) instead of a wide layout.
    #[test]
    fn end_label_strip_stacks_overlapping_labels() {
        // Two labels whose cross extents overlap stack; a third clear of both
        // stays at the node.
        let depth = strip_depth(vec![
            (53.0, 6.1, 16.5),
            (57.7, 18.4, 16.5),
            (90.0, 5.0, 28.5),
        ]);
        assert!((depth - 33.0).abs() < 1e-9);
        assert!((strip_depth(vec![(0.0, 5.0, 16.5), (10.0, 5.0, 28.5)]) - 28.5).abs() < 1e-9);
    }

    /// ELK reserves room beside a port for its edge's end labels: the layer gap
    /// grows by the edge-label spacing and the label's depth on each side.
    #[test]
    fn elk_end_labels_widen_a_narrow_node_across() {
        // A -> B, A -> C, B -> C (TB) with narrow nodes: the long edge A -> C
        // passes B. B's end label at its port to C sticks out of B's right side
        // (port 17.0625 + 5 + 17.5 > 34.125) and keeps the long edge further off.
        let clear = |label: Option<(f64, f64)>| {
            let narrow = LayoutNode {
                width: 34.125,
                height: 81.0,
                cluster: None,
            };
            let mut inp = elk(input(
                vec![narrow.clone(), narrow.clone(), narrow],
                vec![edge(0, 1), edge(0, 2), edge(1, 2)],
                vec![],
                Direction::TopBottom,
            ));
            inp.edges[2].terminals = [label, None];
            let out = layout(&inp);
            let b = out.nodes[1];
            // Where the long edge passes B's rank.
            let pts = &out.edges[1].points;
            let pass = pts
                .windows(2)
                .find(|w| {
                    (w[0].x - w[1].x).abs() < 1e-9
                        && w[0].y.min(w[1].y) < b.y
                        && w[0].y.max(w[1].y) > b.y
                })
                .map(|w| w[0].x)
                .unwrap_or(f64::NAN);
            pass - (b.x + 34.125 / 2.0)
        };
        let plain = clear(None);
        let labelled = clear(Some((17.5, 28.5)));
        let over = 34.125 / 2.0 + 5.0 + 17.5 - 34.125;
        assert!((labelled - plain - over).abs() < 1e-6, "{plain} {labelled}");
    }

    #[test]
    fn elk_edge_label_keeps_its_whole_width_clear() {
        // Z -> A labelled, Z -> K -> A: K shares the label's layer, a node
        // spacing clear of the label's full width.
        let mut inp = elk(input(
            vec![node(None), node(None), node(None)],
            vec![edge(0, 1), edge(0, 2), edge(2, 1)],
            vec![],
            Direction::TopBottom,
        ));
        inp.edges[0].label = Some((83.0, 21.0));
        let out = layout(&inp);
        let label = out.edges[0].label.unwrap_or_default();
        let k = out.nodes[2];
        let gap = if k.x > label.x {
            (k.x - 40.0) - (label.x + 41.5)
        } else {
            (label.x - 41.5) - (k.x + 40.0)
        };
        assert!(
            gap >= inp.node_sep - 1e-6 && gap < inp.node_sep + 1.0,
            "{gap}"
        );
    }

    #[test]
    fn elk_end_labels_beside_a_horizontal_side_do_not_stack() {
        // LR: A -> B, A -> C, both with a start label at A. ELK leaves the two
        // where they are (no overlap removal there), so the layer gap grows by
        // the wider one only, however close their ports.
        let gap = |labels: bool| {
            let mut inp = elk(input(
                vec![node(None), node(None), node(None)],
                vec![edge(0, 1), edge(0, 2)],
                vec![],
                Direction::LeftRight,
            ));
            if labels {
                inp.edges[0].terminals = [Some((30.0, 16.5)), None];
                inp.edges[1].terminals = [Some((50.0, 16.5)), None];
            }
            let out = layout(&inp);
            out.nodes[1].x - out.nodes[0].x
        };
        assert!((gap(true) - gap(false) - (4.0 + 50.0)).abs() < 1e-9);
    }

    #[test]
    fn elk_end_labels_deepen_the_layer_gap() {
        let gap = |terminals: [Option<(f64, f64)>; 2]| {
            let mut inp = elk(input(
                vec![node(None), node(None)],
                vec![edge(0, 1)],
                vec![],
                Direction::TopBottom,
            ));
            inp.edges[0].terminals = terminals;
            let out = layout(&inp);
            out.nodes[1].y - out.nodes[0].y
        };
        let plain = gap([None; 2]);
        let labelled = gap([Some((6.0, 16.5)), Some((16.0, 28.5))]);
        assert!((labelled - plain - (4.0 + 16.5 + 4.0 + 28.5)).abs() < 1e-9);
    }

    #[test]
    fn cycle_breaking_follows_ports_in_the_order_they_were_made() {
        // 0 -> 1, 1 -> 2, 2 -> 1, 0 -> 2: the search follows 0's ports in the
        // order they were made. In edge order it enters the cycle at 1, so
        // 2 -> 1 closes it; with 0 -> 2's port made first it enters at 2, and
        // 1 -> 2 closes it.
        let edges = edges_of(&[(0, 1), (1, 2), (2, 1), (0, 2)]);
        assert_eq!(reversed(&orient_edges(3, &edges, 1, true, &[])), vec![2]);
        let made = [(1, 0), (0, 1), (0, 2), (0, 3)];
        assert_eq!(reversed(&orient_edges(3, &edges, 1, true, &made)), vec![1]);
    }

    #[test]
    fn elk_cycle_breaking_starts_at_the_sources() {
        let edges = edges_of(&[
            (0, 1),
            (0, 3),
            (0, 4),
            (1, 0),
            (1, 3),
            (2, 3),
            (2, 4),
            (3, 1),
            (4, 1),
        ]);
        assert_eq!(
            reversed(&orient_edges(5, &edges, 2, false, &[])),
            vec![3, 7]
        );
        assert_eq!(
            reversed(&orient_edges(5, &edges, 2, true, &[])),
            vec![0, 1, 4, 8]
        );

        let out = layout(&elk(input(
            vec![node(None); 5],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        let order = column(&out);
        assert_eq!(order, vec![2, 3, 1, 0, 4]);
        for w in order.windows(2) {
            assert!(out.nodes[w[1]].y > out.nodes[w[0]].y + 40.0, "{out:?}");
        }
    }

    /// dagre keeps its own cycle breaking: from the first node, not the sources.
    #[test]
    fn dagre_cycle_breaking_starts_at_the_first_node() {
        let edges = edges_of(&[(0, 1), (1, 0), (2, 1)]);
        assert_eq!(reversed(&orient_edges(3, &edges, 2, false, &[])), vec![1]);
        assert_eq!(reversed(&orient_edges(3, &edges, 2, true, &[])), vec![0]);
        let out = layout(&input(
            vec![node(None); 3],
            edges.clone(),
            vec![],
            Direction::TopBottom,
        ));
        assert!(out.nodes[0].y < out.nodes[1].y, "{out:?}");
        let out = layout(&elk(input(
            vec![node(None); 3],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert_eq!(column(&out), vec![2, 1, 0]);
    }

    /// ELK's network simplex balancing: node 2 (one edge in, one out) may sit on
    /// layer 1 or 2 at no cost and ends on the emptier one, layer 2.
    #[test]
    fn elk_balances_a_free_node_onto_the_emptier_layer() {
        let edges = edges_of(&[(0, 1), (1, 3), (3, 4), (0, 2), (2, 4), (5, 1), (5, 6)]);
        let out = layout(&elk(input(
            vec![node(None); 7],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[2].y - out.nodes[3].y).abs() < 1.0, "{out:?}");
        assert!((out.nodes[6].y - out.nodes[1].y).abs() < 1.0, "{out:?}");
    }

    /// Edge labels are nodes of ELK's layering (`LabelDummyInserter` runs before
    /// `NetworkSimplexLayerer`), so they count when it balances: node 2 (`A --> C
    /// --> D`) may sit on layer 1, beside the two labels of `A -->|l| B`, or on
    /// layer 2 beside B, and ends on layer 2, as in elkjs.
    #[test]
    fn elk_balancing_counts_edge_labels() {
        let mut edges = vec![
            LayoutEdge::new(0, 1, 1, Some((30.0, 20.0))),
            LayoutEdge::new(0, 1, 1, Some((30.0, 20.0))),
        ];
        edges.extend(edges_of(&[(1, 3), (0, 2), (2, 3)]));
        let out = layout(&elk(input(
            vec![node(None); 4],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[2].y - out.nodes[1].y).abs() < 1.0, "{out:?}");
    }

    /// With its labels in the layering and then moved to the median layer of their
    /// edges (`LabelDummySwitcher`), ELK leaves no layer holding only long-edge
    /// dummies: every layer between the first and the last holds a node or a label.
    #[test]
    fn elk_layering_leaves_no_layer_of_long_edge_dummies_alone() {
        let mut seed: u64 = 7;
        let mut rnd = |m: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % m
        };
        for _ in 0..300 {
            let n = 4 + rnd(8);
            let mut edges = Vec::new();
            for _ in 0..n + rnd(n) {
                let (a, b) = (rnd(n), rnd(n));
                if a != b {
                    let label = (rnd(5) < 2).then_some((30.0, 20.0));
                    edges.push(LayoutEdge::new(a.min(b), a.max(b), 1, label));
                }
            }
            let inp = elk(input(
                vec![node(None); n],
                edges,
                vec![],
                Direction::TopBottom,
            ));
            let oriented = orient_edges(n, &inp.edges, 1, true, &[]);
            let (ranks, _, _, _) = rank_with_label_nodes(n, &oriented, &inp, (&[], &[]), &[]);
            let mut used: Vec<usize> = ranks.clone();
            for (o, e) in oriented.iter().zip(&inp.edges) {
                if let (Some(o), Some(_)) = (o, e.label) {
                    let (u, w) = (ranks[o.from], ranks[o.to]);
                    used.push(u + w.saturating_sub(u) / 2);
                }
            }
            used.sort_unstable();
            used.dedup();
            for pair in used.windows(2) {
                assert_eq!(pair[1], pair[0] + 1, "{ranks:?} {used:?}");
            }
        }
    }

    /// ELK's `minimalSpan` takes a second, equally short incoming edge for an
    /// outgoing one, so node 2 (parents 0 and 1, children 4 and 5 two layers
    /// down) stays on the fuller layer 1.
    #[test]
    fn elk_balancing_keeps_a_node_with_two_short_incoming_edges() {
        let edges = edges_of(&[
            (0, 2),
            (1, 2),
            (2, 4),
            (2, 5),
            (0, 3),
            (3, 6),
            (6, 4),
            (6, 5),
            (0, 7),
            (1, 3),
        ]);
        let out = layout(&elk(input(
            vec![node(None); 8],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[2].y - out.nodes[3].y).abs() < 1.0, "{out:?}");
        assert!((out.nodes[7].y - out.nodes[3].y).abs() < 1.0, "{out:?}");
    }

    /// ELK layers every connected component on its own, from the first layer.
    #[test]
    fn elk_components_start_on_the_first_layer() {
        let edges = edges_of(&[(0, 1), (1, 2), (3, 2), (4, 5)]);
        let out = layout(&elk(input(
            vec![node(None); 6],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[4].y - out.nodes[0].y).abs() < 1.0, "{out:?}");
    }

    /// Under ELK a label is a node of its own layer: a labelled edge spans two
    /// layers, and its label, in a layer of nodes, takes no extra room there.
    #[test]
    fn elk_labelled_edge_spans_two_layers() {
        let mut edges = edges_of(&[(0, 1), (1, 2), (2, 3), (3, 4)]);
        edges.push(LayoutEdge::new(5, 4, 1, Some((30.0, 20.0))));
        let out = layout(&elk(input(
            vec![node(None); 6],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[5].y - out.nodes[2].y).abs() < 1.0, "{out:?}");
        let label = out.edges[4].label.expect("label");
        assert!((label.y - out.nodes[3].y).abs() < 1.0, "{out:?}");
        let gaps: Vec<f64> = (0..4)
            .map(|i| out.nodes[i + 1].y - out.nodes[i].y)
            .collect();
        assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 1.0), "{gaps:?}");
    }

    /// Where several layerings are equally short, ELK's network simplex picks its
    /// own (a random flowchart, as elkjs lays it out: `n1 -->|x| n5`, `n0 --> n1`,
    /// `n2 -->|l| n3`, `n4 --> n5`, `n2 -->|l| n5`, `n0 --> n3`): `n2` beside
    /// `n1`, `n3` beside `n5`.
    #[test]
    fn elk_layering_breaks_ties_as_elk() {
        let lb = Some((30.0, 20.0));
        // n1 n5 n0 n2 n3 n4
        let edges = vec![
            LayoutEdge::new(0, 1, 1, lb),
            edge(2, 0),
            LayoutEdge::new(3, 4, 1, lb),
            edge(5, 1),
            LayoutEdge::new(3, 1, 1, lb),
            edge(2, 4),
        ];
        let out = layout(&elk(input(
            vec![node(None); 6],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        let y = |v: usize| out.nodes[v].y;
        assert!(y(2) < y(0), "{out:?}");
        assert!((y(0) - y(3)).abs() < 1.0, "{out:?}");
        assert!(y(0) < y(5) && y(5) < y(1), "{out:?}");
        assert!((y(1) - y(4)).abs() < 1.0, "{out:?}");
    }

    /// ELK's layers list their nodes as the layering put them there (depth first
    /// over the ports), and a label dummy sits in the layer the layering gave it
    /// until crossing minimisation is over (elkjs: `n0 -->|x| n3`, `n1 --> n2`,
    /// `n1 -->|a longer label| n3`, `n2 -->|x| n3`, LR): `n0` stays above `n2`.
    #[test]
    fn elk_layer_order_and_labels_follow_the_layering() {
        let lb = Some((30.0, 20.0));
        // n0 n3 n1 n2
        let edges = vec![
            LayoutEdge::new(0, 1, 1, lb),
            edge(2, 3),
            LayoutEdge::new(2, 1, 1, Some((80.0, 20.0))),
            LayoutEdge::new(3, 1, 1, lb),
        ];
        let out = layout(&elk(input(
            vec![node(None); 4],
            edges,
            vec![],
            Direction::LeftRight,
        )));
        assert!((out.nodes[0].x - out.nodes[3].x).abs() < 1.0, "{out:?}");
        assert!(out.nodes[0].y < out.nodes[3].y, "{out:?}");
        // The long edge's label ends up half-way, in the layer after its source.
        let label = out.edges[2].label.expect("label");
        assert!(label.x < out.nodes[1].x - 60.0, "{out:?}");
    }

    /// `SortByInputModelProcessor` compares a node by the source of its first
    /// incoming port: before the layer's ports are sorted, the first in edge order;
    /// after, the first clockwise (elkjs, a random flowchart: `n5 --> n7`,
    /// `n1 -->|yes| n5`, `n4 --> n6`, `n0 --> n4`, `n3 -->|yes| n5`,
    /// `n1 -->|a longer label| n7`, `n0 --> n2`, `n4 -->|yes| n7`, `n2 --> n5`).
    #[test]
    fn elk_model_order_sort_reads_ports_as_elk_has_them() {
        let lb = Some((30.0, 20.0));
        // n5 n7 n1 n4 n6 n0 n3 n2
        let edges = vec![
            edge(0, 1),
            LayoutEdge::new(2, 0, 1, lb),
            edge(3, 4),
            edge(5, 3),
            LayoutEdge::new(6, 0, 1, lb),
            LayoutEdge::new(2, 1, 1, Some((80.0, 20.0))),
            edge(5, 7),
            LayoutEdge::new(3, 1, 1, lb),
            edge(7, 0),
        ];
        let out = layout(&elk(input(
            vec![node(None); 8],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        let x = |v: usize| out.nodes[v].x;
        assert!(x(2) < x(6) && x(6) < x(5), "{out:?}");
        assert!(x(7) < x(3), "{out:?}");
        assert!(x(0) < x(4), "{out:?}");
    }

    /// When balancing moves a labelled edge's target down, the label stays in the
    /// layer below the source with its siblings' labels (mermaid's flowchart
    /// example: `C -->|Three| Car` beside `C -->|One| Laptop --> Database`).
    #[test]
    fn elk_label_stays_put_when_its_target_is_balanced_down() {
        let lb = Some((30.0, 20.0));
        let edges = vec![
            LayoutEdge::new(0, 1, 1, lb),
            LayoutEdge::new(0, 2, 1, lb),
            LayoutEdge::new(0, 3, 1, lb),
            edge(1, 4),
            edge(2, 4),
            edge(4, 5),
            edge(3, 5),
        ];
        let out = layout(&elk(input(
            vec![node(None); 6],
            edges,
            vec![],
            Direction::TopBottom,
        )));
        assert!((out.nodes[3].y - out.nodes[4].y).abs() < 1.0, "{out:?}");
        let ly = |e: usize| out.edges[e].label.expect("label").y;
        assert!((ly(2) - ly(0)).abs() < 1.0, "{out:?}");
    }

    /// ELK spreads a node's ports over its side less 10 at each end, and its node
    /// placement lines ports up, not centres: `0 → 1`, `0 → 2`, `3 → 2` in TB puts
    /// node 1 straight under node 0's first port.
    #[test]
    fn elk_aligns_ports_spread_over_the_side() {
        let edges = edges_of(&[(0, 1), (0, 2), (3, 2)]);
        let inp = elk(input(
            vec![node(None); 4],
            edges,
            vec![],
            Direction::TopBottom,
        ));
        let out = layout(&inp);
        let start = |e: usize| out.edges[e].points[0];
        let end = |e: usize| *out.edges[e].points.last().expect("point");
        // Two ports on node 0's 80 wide bottom: 60 to spread, 20 apart.
        assert!((start(1).x - start(0).x - 20.0).abs() < 1e-6, "{out:?}");
        assert!(
            (start(0).x - (out.nodes[0].x - 10.0)).abs() < 1e-6,
            "{out:?}"
        );
        // Edge 0 runs straight into node 1, whose centre is off node 0's.
        assert!((start(0).x - end(0).x).abs() < 1e-6, "{out:?}");
        assert!((out.nodes[1].x - out.nodes[0].x).abs() > 1.0, "{out:?}");
    }
}
