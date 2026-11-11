//! Graph IR shared by flowcharts, state diagrams and class diagrams, plus the
//! pure renderer `GraphDiagram -> Scene` (layout → shapes → edges → labels).

use super::markers::{self, End, MarkerColors};
use super::shapes::{self, Boundary, PartStyle, ShapeMetrics};
use crate::config::{Curve, GraphConfig, Look as ConfigLook};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::{self, Direction, LayoutCluster, LayoutEdge, LayoutInput, LayoutNode};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::{ColorSlot, DropShadow, Palette, SlotColors};

/// Index into `GraphDiagram::nodes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

/// Index into `GraphDiagram::clusters`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusterId(pub usize);

/// Edges may attach to nodes or to whole clusters (subgraphs / composite states).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Node(NodeId),
    Cluster(ClusterId),
}

/// Node shapes across the graph-based diagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Rounded,
    /// State-diagram state: `squareRect` sizing with 5px corners (`rect.basic { rx: 5px }`).
    State,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    Asymmetric,
    Diamond,
    Hexagon,
    LeanRight,
    LeanLeft,
    Trapezoid,
    InvTrapezoid,
    StateStart,
    StateEnd,
    ForkBar,
    Choice,
    Note,
    /// Box with horizontal compartments (class boxes, described states).
    Compartments,
    /// Takes up space but draws nothing (placeholder for empty clusters).
    Invisible,
    /// ER entity: `label` is the header, every `Section` is one attribute row
    /// whose `lines` are the cells (type, name, keys, comment).
    EntityTable,
    /// Requirement-diagram box: `sections[0]` holds the «stereotype» lines drawn
    /// above the bold `label`; `sections[1]` (optional) holds the field lines
    /// drawn below a divider.
    Requirement,
    // Extended flowchart shapes (`A@{ shape: … }`), see `shapes.rs`.
    /// `text`: label only, no border.
    Text,
    /// `notch-rect` / `card`.
    Card,
    /// `lin-rect` / `shaded-process`.
    LinedRect,
    /// `delay` / `half-rounded-rectangle`.
    Delay,
    /// `tri` / `extract`.
    Triangle,
    /// `flip-tri` / `manual-file`.
    FlippedTriangle,
    /// `hourglass` / `collate`.
    Hourglass,
    /// `cross-circ` / `summary`.
    CrossedCircle,
    /// `sl-rect` / `manual-input`.
    SlopedRect,
    /// `win-pane` / `internal-storage`.
    WindowPane,
    /// `div-rect` / `divided-process`.
    DividedRect,
    /// `notch-pent` / `loop-limit`.
    NotchedPentagon,
    /// `st-rect` / `processes`.
    StackedRect,
    /// `tag-rect` / `tagged-process`.
    TaggedRect,
    /// `doc` / `document`.
    Document,
    /// `lin-cyl` / `disk`.
    LinedCylinder,
    /// `h-cyl` / `das`.
    HorizontalCylinder,
    /// `curv-trap` / `display`.
    CurvedTrapezoid,
    /// `lin-doc` / `lined-document` (agentflow `refdoc`).
    LinedDocument,
    /// `collapsedGroup`: a folded container (agentflow `@{ view: collapsed }`).
    CollapsedGroup,
}

/// Inline style overrides (`style`, `classDef`, `linkStyle`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleOverrides {
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    pub stroke_width: Option<f64>,
    pub dash: Option<Vec<f64>>,
    pub color: Option<Color>,
    pub bold: Option<bool>,
}

impl StyleOverrides {
    /// Parse `fill:#f9f,stroke:#333,stroke-width:4px,color:#fff,stroke-dasharray: 5 5`.
    pub fn parse(s: &str) -> Self {
        let mut st = StyleOverrides::default();
        for part in split_style(s) {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            let v = v.trim().trim_end_matches("!important").trim();
            match k.trim() {
                "fill" | "background" | "background-color" => st.fill = Some(Color::new(v)),
                "stroke" | "border-color" => st.stroke = Some(Color::new(v)),
                "stroke-width" => st.stroke_width = v.trim_end_matches("px").trim().parse().ok(),
                "stroke-dasharray" => {
                    let d: Vec<f64> = v
                        .split([' ', ','])
                        .filter_map(|x| x.trim_end_matches("px").parse().ok())
                        .collect();
                    st.dash = (!d.is_empty()).then_some(d);
                }
                "color" => st.color = Some(Color::new(v)),
                "font-weight" => {
                    st.bold =
                        Some(v == "bold" || v.parse::<u32>().map(|w| w >= 600).unwrap_or(false))
                }
                _ => {}
            }
        }
        st
    }

    /// `other` wins where it sets a value.
    pub fn merged(&self, other: &StyleOverrides) -> StyleOverrides {
        StyleOverrides {
            fill: other.fill.clone().or_else(|| self.fill.clone()),
            stroke: other.stroke.clone().or_else(|| self.stroke.clone()),
            stroke_width: other.stroke_width.or(self.stroke_width),
            dash: other.dash.clone().or_else(|| self.dash.clone()),
            color: other.color.clone().or_else(|| self.color.clone()),
            bold: other.bold.or(self.bold),
        }
    }
}

/// Split on commas that are not inside parentheses (`rgb(1,2,3)`).
fn split_style(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' | ';' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// One compartment of a `Shape::Compartments` node.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub lines: Vec<String>,
    pub centered: bool,
    pub bold: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    pub cluster: Option<ClusterId>,
    pub style: StyleOverrides,
    pub classes: Vec<String>,
    pub sections: Vec<Section>,
    /// Categorical palette slot (`colorIndex`), painted only by the colour themes.
    pub color_slot: Option<ColorSlot>,
}

impl Node {
    pub fn new(id: impl Into<String>, label: impl Into<String>, shape: Shape) -> Self {
        Node {
            id: id.into(),
            label: label.into(),
            shape,
            cluster: None,
            style: StyleOverrides::default(),
            classes: Vec::new(),
            sections: Vec::new(),
            color_slot: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Solid,
    Dotted,
    /// `edge-pattern-dashed` (class `..`, ER non-identifying, requirement relations).
    Dashed,
    Thick,
    Invisible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    None,
    Arrow,
    Circle,
    Cross,
    /// Hollow triangle (inheritance / realization).
    Triangle,
    /// Hollow diamond (aggregation).
    Diamond,
    /// Filled diamond (composition).
    DiamondFilled,
    /// Open chevron (requirement `requirement_arrow`).
    OpenArrow,
    /// Filled barb (state transitions, `barbEnd`).
    Barb,
    /// Class `<` / `>` (`dependency` marker).
    Dependency,
    /// Class `()` (`lollipop` marker).
    Lollipop,
    /// ER crow's-foot: exactly one (`||`).
    ErOnlyOne,
    /// ER crow's-foot: zero or one (`|o` / `o|`).
    ErZeroOrOne,
    /// ER crow's-foot: one or more (`}|` / `|{`).
    ErOneOrMore,
    /// ER crow's-foot: zero or more (`}o` / `o{`).
    ErZeroOrMore,
    /// Circle with a plus inside (requirement `contains`).
    CirclePlus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: Endpoint,
    pub to: Endpoint,
    pub label: Option<String>,
    pub line: LineKind,
    pub start: Head,
    pub end: Head,
    pub minlen: usize,
    pub style: StyleOverrides,
    /// Text drawn near the start / end (class diagram cardinalities).
    pub start_label: Option<String>,
    pub end_label: Option<String>,
}

impl Edge {
    pub fn new(from: Endpoint, to: Endpoint) -> Self {
        Edge {
            from,
            to,
            label: None,
            line: LineKind::Solid,
            start: Head::None,
            end: Head::Arrow,
            minlen: 1,
            style: StyleOverrides::default(),
            start_label: None,
            end_label: None,
        }
    }
}

/// How a cluster is drawn (mermaid's cluster shapes in `clusters.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClusterKind {
    /// `rect`: flowchart subgraph, class namespace.
    #[default]
    Group,
    /// `roundedWithTitle`: state composite (title strip over an inner body).
    Composite,
    /// `divider`: one concurrency region of a state composite (dashed, untitled).
    Region,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub id: String,
    pub title: String,
    pub parent: Option<ClusterId>,
    pub style: StyleOverrides,
    /// Categorical palette slot (`colorIndex`), painted only by the colour themes.
    pub color_slot: Option<ColorSlot>,
    /// The cluster's own `direction` statement.
    pub direction: Option<Direction>,
    pub kind: ClusterKind,
}

impl Cluster {
    pub fn new(id: impl Into<String>, title: impl Into<String>, parent: Option<ClusterId>) -> Self {
        Cluster {
            id: id.into(),
            title: title.into(),
            parent,
            style: StyleOverrides::default(),
            color_slot: None,
            direction: None,
            kind: ClusterKind::Group,
        }
    }
}

/// A fully parsed graph-shaped diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphDiagram {
    pub kind: DiagramKind,
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
    pub title: Option<String>,
}

/// Per-diagram visual conventions.
struct Look {
    edge_width: f64,
    thick_width: f64,
    dotted_dash: Vec<f64>,
    dashed_dash: Vec<f64>,
    node_stroke_width: f64,
    /// Corner radius of `Shape::Rounded` (mermaid's `roundedRect`: theme `radius` ?? 5).
    rounded_rx: f64,
}

fn look(kind: DiagramKind) -> Look {
    // Global styles.ts: `.edge-thickness-normal { stroke-width: strokeWidth (1) }`,
    // `.edge-thickness-thick { 3.5px }`, `.edge-pattern-dashed { 3 }`,
    // `.edge-pattern-dotted { 2 }`; diagram styles override the patterns.
    let (dotted_dash, dashed_dash) = match kind {
        // er/styles.ts: `.edge-pattern-dashed { stroke-dasharray: 8,8 }`.
        DiagramKind::Er => (vec![2.0], vec![8.0, 8.0]),
        // state/styles.js: `.note-edge { stroke-dasharray: 5 }`.
        DiagramKind::State => (vec![5.0], vec![3.0]),
        _ => (vec![2.0], vec![3.0]),
    };
    Look {
        edge_width: 1.0,
        thick_width: 3.5,
        dotted_dash,
        dashed_dash,
        node_stroke_width: 1.0,
        rounded_rx: 5.0,
    }
}

/// Apply the theme's `strokeWidth` / `radius` the way each diagram's stylesheet reads them:
/// flowchart nodes `strokeWidth ?? 1` and edges `?? 2`; class and state `strokeWidth || 1`
/// for both; ER and requirement use `strokeWidth` only under the neo look (1px otherwise).
/// `radius` reaches flowchart rounded nodes (`roundedRect.ts`).
fn themed_look(mut lk: Look, kind: DiagramKind, look: ConfigLook, pal: &Palette) -> Look {
    let Some(sw) = pal.stroke_width else {
        if matches!(kind, DiagramKind::Flowchart | DiagramKind::Agentflow) {
            lk.rounded_rx = pal.radius.unwrap_or(lk.rounded_rx);
        }
        return lk;
    };
    match kind {
        DiagramKind::Flowchart | DiagramKind::Agentflow => {
            lk.node_stroke_width = sw;
            lk.edge_width = sw;
            lk.rounded_rx = pal.radius.unwrap_or(lk.rounded_rx);
        }
        DiagramKind::Class | DiagramKind::State => {
            lk.node_stroke_width = sw;
            lk.edge_width = sw;
        }
        DiagramKind::Er | DiagramKind::Requirement if look == ConfigLook::Neo => {
            lk.node_stroke_width = sw;
            lk.edge_width = sw;
        }
        _ => {}
    }
    lk
}

/// The palette slot colours of an item, if the theme is a colour theme.
fn slot_colors(slot: Option<ColorSlot>, pal: &Palette) -> Option<SlotColors> {
    Some(pal.color_slots.as_ref()?.slot(slot?))
}

/// `style` with the slot's border / fill filled in where the author set none: user
/// `style` / `classDef` declarations are inline and keep winning over the theme palette.
fn slot_styled(style: &StyleOverrides, slot: Option<&SlotColors>) -> StyleOverrides {
    let Some(sc) = slot else {
        return style.clone();
    };
    StyleOverrides {
        stroke: style.stroke.clone().or_else(|| Some(sc.border.clone())),
        fill: style.fill.clone().or_else(|| sc.fill.clone()),
        ..style.clone()
    }
}

/// The neo look's drop shadow: an offset translucent copy of `el`'s silhouette.
fn drop_shadow(el: &Element, sh: &DropShadow) -> Option<Element> {
    let Element::Shape { geom, paint, .. } = el else {
        return None;
    };
    if paint.fill.is_none() && paint.stroke.is_none() {
        return None;
    }
    Some(Element::classed_shape(
        geom.translated(sh.dx, sh.dy),
        Paint::fill(&sh.color).with_opacity(sh.opacity),
        "drop-shadow",
    ))
}

/// Rows of a `Shape::Compartments` node: height of every section, where its
/// text block starts inside it, and the inset of left-aligned text.
struct CompartmentLayout {
    heights: Vec<f64>,
    text_top: Vec<f64>,
    inset: f64,
    rx: f64,
}

struct Measured {
    block: TextBlock,
    sections: Vec<TextBlock>,
    compartments: Option<CompartmentLayout>,
    table: Option<TableMetrics>,
    label_offset: Point,
    metrics: ShapeMetrics,
    w: f64,
    h: f64,
}

fn compartment_layout(
    kind: DiagramKind,
    blocks: &[TextBlock],
    pad: f64,
) -> (CompartmentLayout, f64, f64) {
    let content = |b: &TextBlock| if b.is_empty() { 0.0 } else { b.height };
    let max_w = blocks.iter().map(|b| b.width).fold(0.0, f64::max);
    if kind == DiagramKind::State {
        // rectWithTitle.ts: box = labels + padding; title 3px below the top,
        // divider `title + padding/2` down, description 5px below it.
        let hp = pad / 2.0;
        let title = blocks.first().map(content).unwrap_or(0.0);
        let rest: f64 = blocks.iter().skip(1).map(content).sum();
        let layout = CompartmentLayout {
            heights: vec![title + hp, rest + 5.0 + pad],
            text_top: vec![3.0, hp + 5.0 + 3.0],
            inset: hp,
            rx: 5.0,
        };
        let h = layout.heights.iter().sum();
        return (layout, max_w + pad, h);
    }
    // classBox.ts / shapeUtil.ts: every compartment is its text plus PADDING
    // above and below; an empty members box is GAP/2 + 2·GAP (2·GAP for empty
    // methods), and a class with neither gets two 1.5·GAP boxes.
    let all_empty = blocks.iter().skip(1).all(|b| b.is_empty());
    let heights: Vec<f64> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match (i, b.is_empty()) {
            (0, _) | (_, false) => content(b) + 2.0 * pad,
            _ if all_empty => 1.5 * pad,
            (1, true) => 2.5 * pad,
            _ => 2.0 * pad,
        })
        .collect();
    let layout = CompartmentLayout {
        text_top: vec![pad; heights.len()],
        heights,
        inset: pad,
        rx: 0.0,
    };
    let h = layout.heights.iter().sum();
    (layout, max_w + 2.0 * pad, h)
}

#[allow(clippy::too_many_arguments)]
fn measure_node(
    node: &Node,
    kind: DiagramKind,
    pad: f64,
    cell_pad: f64,
    font: f64,
    horizontal: bool,
    look: ConfigLook,
    min_label_width: f64,
) -> Measured {
    let mut block = TextBlock::label(&node.label, font);
    if node.style.bold == Some(true) {
        block = TextBlock::new(block.lines, font, true);
    }
    // labelHelper: `minWidth` widens non-empty labels; circles and collapsed
    // containers size without it.
    let widen = !block.is_empty()
        && !matches!(
            node.shape,
            Shape::Circle | Shape::DoubleCircle | Shape::CollapsedGroup
        );
    let (tw, th) = (
        if widen {
            block.width.max(min_label_width)
        } else {
            block.width
        },
        block
            .height
            .max(if block.is_empty() { 0.0 } else { font * 1.5 }),
    );
    // Notes carry their own padding: classDb `config.class.padding ?? 6`,
    // state dataFetcher `config.flowchart.padding` (15).
    let node_pad = match (node.shape, kind) {
        (Shape::Note, DiagramKind::Class) => 6.0,
        (Shape::Note, DiagramKind::State) => 15.0,
        _ => pad,
    };
    let metrics = ShapeMetrics::for_look(node.shape, node_pad, look);
    let mut sections = Vec::new();
    let mut compartments = None;
    let mut table = None;
    let mut label_offset = Point::default();
    let (w, h) = match shapes::size(node.shape, tw, th, &metrics, horizontal) {
        Some(s) => {
            label_offset = s.label_offset;
            (s.w, s.h)
        }
        None => match node.shape {
            Shape::Compartments => {
                sections = node
                    .sections
                    .iter()
                    .map(|s| TextBlock::new(s.lines.clone(), font, s.bold))
                    .collect();
                let (layout, w, h) = compartment_layout(kind, &sections, pad);
                compartments = Some(layout);
                (w, h)
            }
            Shape::EntityTable => {
                let t = table_metrics(node, pad, cell_pad, font);
                let size = (t.w, t.h);
                table = Some(t);
                size
            }
            Shape::Requirement => {
                let r = requirement_metrics(node, font);
                (r.w, r.h)
            }
            _ => (tw.max(20.0), th.max(10.0)),
        },
    };
    Measured {
        block,
        sections,
        compartments,
        table,
        label_offset,
        metrics,
        w,
        h,
    }
}

/// Diagram-specific options of [`render_with`] that are not part of [`GraphConfig`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderOptions {
    /// mermaid's `minNodeWidth`: non-empty node labels are widened to at least this.
    pub min_label_width: f64,
    /// `flowchart.subGraphTitleMargin` (top, bottom). As under mermaid's ELK layout it
    /// moves a `rect` cluster's title down by `top` and every edge label down by
    /// `(top + bottom) / 2` (`positionEdgeLabel`); the layout itself is unchanged.
    pub subgraph_title_margin: (f64, f64),
}

/// Pure renderer for graph diagrams.
pub fn render(g: &GraphDiagram, cfg: &GraphConfig, pal: &Palette) -> Scene {
    render_with(g, cfg, pal, &RenderOptions::default())
}

/// [`render`] with extra [`RenderOptions`].
pub fn render_with(
    g: &GraphDiagram,
    cfg: &GraphConfig,
    pal: &Palette,
    opts: &RenderOptions,
) -> Scene {
    let lk = themed_look(look(g.kind), g.kind, cfg.look, pal);
    let shadow = pal
        .drop_shadow
        .as_ref()
        .filter(|_| cfg.look == ConfigLook::Neo);
    let edge_label_bg = match (g.kind, &pal.er_edge_label_background) {
        (DiagramKind::Er, Some(bg)) => bg,
        _ => &pal.edge_label_background,
    };
    let font = pal.font_size;
    let horizontal = g.direction.is_horizontal();

    // A cluster with nothing in it is a plain leaf upstream: its title alone, with no
    // visible box. A placeholder node (sized to the title) stands in for it.
    let mut nodes = g.nodes.clone();
    let mut empty_of: Vec<Option<usize>> = vec![None; g.clusters.len()];
    for (ci, c) in g.clusters.iter().enumerate() {
        let has_member = nodes.iter().any(|n| {
            n.cluster
                .map(|nc| is_within(&g.clusters, nc, ClusterId(ci)))
                .unwrap_or(false)
        }) || g.clusters.iter().any(|o| o.parent == Some(ClusterId(ci)));
        if !has_member {
            let mut placeholder = Node::new(format!("__empty_{}", c.id), "", Shape::Invisible);
            placeholder.cluster = c.parent;
            empty_of[ci] = Some(nodes.len());
            nodes.push(placeholder);
        }
    }

    let cluster_titles: Vec<TextBlock> = g
        .clusters
        .iter()
        .map(|c| TextBlock::label(&c.title, font))
        .collect();
    let mut measured: Vec<Measured> = nodes
        .iter()
        .map(|n| {
            measure_node(
                n,
                g.kind,
                cfg.padding,
                cfg.diagram_padding,
                font,
                horizontal,
                cfg.look,
                opts.min_label_width,
            )
        })
        .collect();
    for (ci, p) in empty_of.iter().enumerate() {
        if let Some(p) = *p {
            let t = &cluster_titles[ci];
            measured[p].w = t.width + CLUSTER_TITLE_PAD;
            measured[p].h = t.height;
        }
    }
    // ELK reserves the painted title width (label + the group's padding) as the
    // frame's minimum width; class namespaces pad with `class.padding`.
    let title_pad = if g.kind == DiagramKind::Class {
        cfg.padding
    } else {
        CLUSTER_TITLE_PAD
    };

    // An edge to / from a whole cluster is laid out against one member node: a member
    // without incoming edges from inside the cluster for edges into it, and (the last)
    // one without outgoing inside edges for edges out of it, so that `A --> B` between
    // clusters ranks B's content after A's, as dagre's cluster borders do.
    let representative = |c: ClusterId, outgoing: bool| -> usize {
        let members: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.cluster
                    .map(|nc| is_within(&g.clusters, nc, c))
                    .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();
        let inside = |i: usize| {
            g.edges.iter().any(|e| match (e.from, e.to) {
                (Endpoint::Node(NodeId(a)), Endpoint::Node(NodeId(b))) if a != b => {
                    let (me, other) = if outgoing { (a, b) } else { (b, a) };
                    me == i && members.contains(&other)
                }
                _ => false,
            })
        };
        let pick = if outgoing {
            members.iter().rev().find(|&&i| !inside(i))
        } else {
            members.iter().find(|&&i| !inside(i))
        };
        pick.or(members.first()).copied().unwrap_or(0)
    };
    // Layout endpoint of an edge end: the node, or a member of the whole cluster.
    let layout_end = |e: Endpoint, outgoing: bool| -> (usize, Option<usize>) {
        match e {
            Endpoint::Node(NodeId(i)) => (i, None),
            Endpoint::Cluster(c) => match empty_of[c.0] {
                Some(p) => (p, None),
                None => (representative(c, outgoing), Some(c.0)),
            },
        }
    };
    let resolve = |e: Endpoint, outgoing: bool| layout_end(e, outgoing).0;
    // Edges between a cluster and itself or something inside it. They are laid out as
    // a loop on one inner node (constraining nothing) and drawn against the frame.
    let nested: Vec<Option<NestedEdge>> = g
        .edges
        .iter()
        .map(|e| nested_edge(&g.clusters, &nodes, &empty_of, e))
        .collect();
    let layout_ends = |ei: usize, e: &Edge| -> ((usize, Option<usize>), (usize, Option<usize>)) {
        match nested[ei] {
            None => (layout_end(e.from, true), layout_end(e.to, false)),
            Some(ne) => {
                let v = match ne.inner {
                    Endpoint::Node(NodeId(i)) => i,
                    Endpoint::Cluster(c) => resolve(Endpoint::Cluster(c), true),
                };
                ((v, None), (v, None))
            }
        }
    };
    let edge_labels: Vec<Option<TextBlock>> = g
        .edges
        .iter()
        .map(|e| {
            e.label
                .as_ref()
                .map(|l| TextBlock::label(l, font))
                .filter(|b| !b.is_empty())
        })
        .collect();

    // Model order: the order mermaid hands its nodes to the layout.
    let perm = model_order(g, &nodes, &empty_of);
    let mut pos_of = vec![0; nodes.len()];
    for (k, &old) in perm.iter().enumerate() {
        pos_of[old] = k;
    }
    let input = LayoutInput {
        nodes: perm
            .iter()
            .map(|&i| LayoutNode {
                width: measured[i].w,
                height: measured[i].h,
                cluster: nodes[i].cluster.map(|c| c.0),
            })
            .collect(),
        edges: g
            .edges
            .iter()
            .zip(&edge_labels)
            .enumerate()
            .map(|(ei, (e, lb))| {
                let ((from, from_cluster), (to, to_cluster)) = layout_ends(ei, e);
                LayoutEdge {
                    from: pos_of[from],
                    to: pos_of[to],
                    minlen: e.minlen,
                    label: lb.as_ref().map(|b| (b.width + 8.0, b.height + 4.0)),
                    from_cluster,
                    to_cluster,
                }
            })
            .collect(),
        clusters: g
            .clusters
            .iter()
            .zip(&cluster_titles)
            .map(|(c, t)| LayoutCluster {
                parent: c.parent.map(|p| p.0),
                title_height: cluster_title_height(c.kind, t),
                min_width: if t.is_empty() {
                    0.0
                } else {
                    t.width + title_pad
                },
                direction: c.direction,
            })
            .collect(),
        direction: g.direction,
        node_sep: cfg.node_spacing,
        rank_sep: cfg.rank_spacing,
        edge_sep: 20.0,
        cluster_padding: CLUSTER_PADDING,
        inner_node_sep: CLUSTER_NODE_SPACING,
        inner_rank_sep: CLUSTER_RANK_SPACING,
    };
    let raw = layout::layout(&input);
    let out = layout::LayoutOutput {
        nodes: (0..nodes.len()).map(|i| raw.nodes[pos_of[i]]).collect(),
        ..raw
    };

    let mut bounds = Bounds::default();
    let mut cluster_els = Vec::new();
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    let mut node_els = Vec::new();

    // Clusters, outermost first.
    let mut cluster_rects = out.clusters.clone();
    for (ci, t) in cluster_titles.iter().enumerate() {
        let r = &mut cluster_rects[ci];
        if let Some(p) = empty_of[ci] {
            *r = Rect::centered(out.nodes[p], measured[p].w, measured[p].h);
            continue;
        }
        let need = if t.is_empty() {
            0.0
        } else {
            t.width + title_pad
        };
        if r.w < need {
            r.x -= (need - r.w) / 2.0;
            r.w = need;
        }
    }
    let mut order: Vec<usize> = (0..g.clusters.len()).collect();
    order.sort_by_key(|&c| depth(&g.clusters, ClusterId(c)));
    for ci in order {
        let c = &g.clusters[ci];
        let r = cluster_rects[ci];
        bounds.add_rect(r);
        let t = &cluster_titles[ci];
        let mut children: Vec<Element> = Vec::new();
        if empty_of[ci].is_none() {
            let alt = depth(&g.clusters, ClusterId(ci)) % 2 == 1;
            let shapes = cluster_shapes(c, r, t, alt, pal, &lk, cfg.look, g.kind);
            if let Some(s) = shadow.and_then(|sh| shapes.first().and_then(|e| drop_shadow(e, sh))) {
                children.push(s);
            }
            children.extend(shapes);
        }
        if !t.is_empty() {
            let color = c
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.title_color.clone());
            // Flush with the top edge (`translate(x, y + subGraphTitleTopMargin)`);
            // a composite's label sits 1px lower (`roundedWithTitle`).
            let top = match (empty_of[ci], c.kind) {
                (Some(_), _) => r.y,
                (None, ClusterKind::Composite) => r.y + 1.0,
                (None, ClusterKind::Group) => r.y + opts.subgraph_title_margin.0,
                _ => r.y,
            };
            let center = Point::new(r.x + r.w / 2.0, top + t.height / 2.0);
            children.push(Element::classed_text(
                Text::centered(center, &t.lines, font, t.line_height, &color),
                "cluster-label",
            ));
        }
        cluster_els.push(Element::Group {
            id: Some(format!("cluster-{}", c.id)),
            class: Some("cluster".into()),
            children,
        });
    }

    // Node geometry (needed for edge clipping).
    let mut boundaries = Vec::with_capacity(nodes.len());
    for (i, (n, m)) in nodes.iter().zip(&measured).enumerate() {
        let c = out.nodes[i];
        let slot = slot_colors(n.color_slot, pal);
        let painted;
        let n = if slot.is_some() {
            painted = Node {
                style: slot_styled(&n.style, slot.as_ref()),
                ..n.clone()
            };
            &painted
        } else {
            n
        };
        let (mut els, boundary) = node_elements(n, m, c, pal, &lk, font);
        if let Some(sh) = shadow.filter(|_| n.shape != Shape::Invisible) {
            if let Some(s) = els.first().and_then(|e| drop_shadow(e, sh)) {
                els.insert(0, s);
            }
        }
        bounds.add_rect(Rect::centered(c, m.w, m.h));
        boundaries.push(boundary);
        if n.shape != Shape::Invisible {
            let class = std::iter::once("node".to_string())
                .chain(std::iter::once("default".into()))
                .chain(n.classes.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            node_els.push(Element::Group {
                id: Some(format!("{}-{}-{}", node_prefix(g.kind), n.id, i)),
                class: Some(class),
                children: els,
            });
        }
    }

    for (ei, (e, route)) in g.edges.iter().zip(&out.edges).enumerate() {
        if e.line == LineKind::Invisible {
            continue;
        }
        let mut pts = route.points.clone();
        let mut label_at = route.label;
        if let Some(ne) = nested[ei] {
            let frame = cluster_rects[ne.cluster.0];
            let inner_rect = match ne.inner {
                Endpoint::Node(NodeId(i)) => {
                    Rect::centered(out.nodes[i], measured[i].w, measured[i].h)
                }
                Endpoint::Cluster(c) => cluster_rects[c.0],
            };
            let lw = edge_labels[ei].as_ref().map(|b| b.width + 8.0);
            let title_h =
                cluster_title_height(g.clusters[ne.cluster.0].kind, &cluster_titles[ne.cluster.0]);
            // The flow direction inside the cluster: its own, else the nearest ancestor's.
            let mut dir = g.direction;
            let mut cur = Some(ne.cluster);
            for _ in 0..=g.clusters.len() {
                let Some(c) = cur else { break };
                if let Some(d) = g.clusters[c.0].direction {
                    dir = d;
                    break;
                }
                cur = g.clusters[c.0].parent;
            }
            let (p, l) = nested_route(ne, frame, inner_rect, dir, title_h, lw);
            pts = p;
            if let Endpoint::Node(NodeId(i)) = ne.inner {
                // Clip at the node's own outline (diamonds, circles, …).
                let k = if ne.inner_is_target { pts.len() - 1 } else { 0 };
                let toward = pts[if k == 0 { 1 } else { k - 1 }];
                pts[k] = boundaries[i].clip(toward);
            }
            label_at = edge_labels[ei].as_ref().map(|_| l);
        } else if pts.len() < 2 {
            continue;
        }
        let is_loop = nested[ei].is_some() || resolve(e.from, true) == resolve(e.to, false);
        if !is_loop {
            // An edge to / from a whole cluster is cut where it crosses the cluster's
            // border (mermaid's `cutPathAtIntersect` for `fromCluster` / `toCluster`).
            match e.from {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0]),
                Endpoint::Node(NodeId(i)) => pts[0] = boundaries[i].clip(pts[1]),
            }
            pts.reverse();
            match e.to {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0]),
                Endpoint::Node(NodeId(i)) => pts[0] = boundaries[i].clip(pts[1]),
            }
            pts.reverse();
            if pts.len() < 2 {
                continue;
            }
        }
        let stroke = e
            .style
            .stroke
            .clone()
            .unwrap_or_else(|| pal.line_color.clone());
        let width = e
            .style
            .stroke_width
            .unwrap_or(if e.line == LineKind::Thick {
                lk.thick_width
            } else {
                lk.edge_width
            });
        let mut heads = Vec::new();
        let last = pts.len() - 1;
        let end_dir = pts[last].minus(pts[last - 1]).norm();
        let start_dir = pts[0].minus(pts[1]).norm();
        let colors = MarkerColors {
            line: &stroke,
            background: &pal.main_bkg,
        };
        heads.extend(markers::elements(
            e.end,
            End::End,
            pts[last],
            end_dir,
            colors,
            width,
            cfg.look == ConfigLook::Neo,
        ));
        heads.extend(markers::elements(
            e.start,
            End::Start,
            pts[0],
            start_dir,
            colors,
            width,
            cfg.look == ConfigLook::Neo,
        ));
        let (end_back, start_back) = (markers::pullback(e.end), markers::pullback(e.start));
        if end_back > 0.0 && pts[last].dist(pts[last - 1]) > end_back {
            pts[last] = pts[last].minus(end_dir.scale(end_back));
        }
        if start_back > 0.0 && pts[0].dist(pts[1]) > start_back {
            pts[0] = pts[0].minus(start_dir.scale(start_back));
        }
        for p in &pts {
            bounds.add_point(*p);
        }
        let cmds = match cfg.curve {
            Curve::Basis => geom::basis_curve(&pts),
            Curve::Linear => geom::polyline(&pts),
            Curve::Step => geom::polyline(&step_points(&pts, horizontal)),
        };
        let mut paint = Paint::stroke(&stroke, width);
        paint.dash = e.style.dash.clone().or_else(|| match e.line {
            LineKind::Dotted => Some(lk.dotted_dash.clone()),
            LineKind::Dashed => Some(lk.dashed_dash.clone()),
            _ => None,
        });
        let from_id = endpoint_id(g, e.from);
        let to_id = endpoint_id(g, e.to);
        let mut children = vec![Element::classed_shape(
            Geometry::Path(cmds),
            paint,
            format!("flowchart-link LS-{from_id} LE-{to_id}"),
        )];
        children.extend(heads);
        edge_els.push(Element::Group {
            id: Some(format!("L-{from_id}-{to_id}-{ei}")),
            class: Some("edgePath".into()),
            children,
        });

        let text_color = e
            .style
            .color
            .clone()
            .unwrap_or_else(|| pal.text_color.clone());
        let (mt, mb) = opts.subgraph_title_margin;
        let label_at = label_at.map(|p| Point::new(p.x, p.y + (mt + mb) / 2.0));
        if let (Some(block), Some(at)) = (&edge_labels[ei], label_at) {
            let r = Rect::centered(at, block.width + 4.0, block.height);
            bounds.add_rect(r);
            label_els.push(Element::group(
                "edgeLabel",
                vec![
                    Element::rect(r, 0.0, Paint::fill(edge_label_bg)),
                    Element::text(Text::centered(
                        at,
                        &block.lines,
                        font,
                        block.line_height,
                        &text_color,
                    )),
                ],
            ));
        }
        for (txt, anchor_pt, toward) in [
            (&e.start_label, pts[0], pts[1]),
            (&e.end_label, pts[last], pts[last - 1]),
        ] {
            if let Some(t) = txt {
                let dir = toward.minus(anchor_pt).norm();
                let perp = Point::new(-dir.y, dir.x);
                let at = anchor_pt.plus(dir.scale(16.0)).plus(perp.scale(10.0));
                let b = TextBlock::label(t, font * 0.875);
                bounds.add_rect(Rect::centered(at, b.width, b.height));
                label_els.push(Element::classed_text(
                    Text::centered(at, &b.lines, b.font_size, b.line_height, &text_color),
                    "edgeTerminals",
                ));
            }
        }
    }

    // Title above everything.
    let mut title_els = Vec::new();
    if let Some(t) = g.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], font * 1.125, false);
        let center = Point::new(
            content.x + content.w / 2.0,
            content.y - cfg.title_top_margin + tb.height / 2.0 - tb.height,
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
            if g.kind == DiagramKind::Agentflow {
                "agentflowTitleText"
            } else {
                "flowchartTitleText"
            },
        ));
    }

    let mut elements = vec![
        Element::group("clusters", cluster_els),
        Element::group("edgePaths", edge_els),
        Element::group("edgeLabels", label_els),
        Element::group("nodes", node_els),
    ];
    elements.extend(title_els);
    let view_box = bounds.rect().pad(cfg.diagram_padding);
    Scene {
        kind: g.kind,
        view_box,
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// A node drawn by a layout other than the layered one (swimlanes): its size, as
/// [`render`] measures it, for the caller to place.
pub fn node_size(
    n: &Node,
    kind: DiagramKind,
    cfg: &GraphConfig,
    pal: &Palette,
    horizontal: bool,
) -> (f64, f64) {
    let m = measure_node(
        n,
        kind,
        cfg.padding,
        cfg.diagram_padding,
        pal.font_size,
        horizontal,
        cfg.look,
        0.0,
    );
    (m.w, m.h)
}

/// Draw node `index` of a graph centred on `c`, exactly as [`render`] would (palette
/// slot, neo drop shadow, `node` group); returns the group and its clipping boundary.
pub fn draw_node(
    n: &Node,
    index: usize,
    kind: DiagramKind,
    cfg: &GraphConfig,
    pal: &Palette,
    horizontal: bool,
    c: Point,
) -> (Element, Boundary) {
    let lk = themed_look(look(kind), kind, cfg.look, pal);
    let font = pal.font_size;
    let m = measure_node(
        n,
        kind,
        cfg.padding,
        cfg.diagram_padding,
        font,
        horizontal,
        cfg.look,
        0.0,
    );
    let slot = slot_colors(n.color_slot, pal);
    let painted = Node {
        style: slot_styled(&n.style, slot.as_ref()),
        ..n.clone()
    };
    let (mut els, boundary) = node_elements(&painted, &m, c, pal, &lk, font);
    let shadow = pal
        .drop_shadow
        .as_ref()
        .filter(|_| cfg.look == ConfigLook::Neo && n.shape != Shape::Invisible);
    if let Some(s) = shadow.and_then(|sh| els.first().and_then(|e| drop_shadow(e, sh))) {
        els.insert(0, s);
    }
    let class = ["node", "default"]
        .into_iter()
        .map(str::to_string)
        .chain(n.classes.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let group = Element::Group {
        id: Some(format!("{}-{}-{}", node_prefix(kind), n.id, index)),
        class: Some(class),
        children: els,
    };
    (group, boundary)
}

/// Padding between a cluster frame and its children (mermaid's ELK `SUBGRAPH_PADDING`).
const CLUSTER_PADDING: f64 = 24.0;
/// Node gap inside a cluster laid out on its own (`DEFAULT_SUBGRAPH_NODE_SPACING`).
const CLUSTER_NODE_SPACING: f64 = 50.0;
/// Layer gap inside a cluster laid out on its own (`DEFAULT_SUBGRAPH_SPACING_BASE_VALUE`).
const CLUSTER_RANK_SPACING: f64 = 24.0;
/// Horizontal padding counted into a cluster's title width (the group node's `padding: 8`).
const CLUSTER_TITLE_PAD: f64 = 8.0;

/// Height of the band reserved above a cluster's children for its title: ELK keeps
/// the label plus its label spacing clear (21px of text → 24); a state composite's
/// body starts 2px under its label (`roundedWithTitle`); regions have no title.
fn cluster_title_height(kind: ClusterKind, t: &TextBlock) -> f64 {
    if t.is_empty() {
        return 0.0;
    }
    match kind {
        ClusterKind::Group => t.height + 3.0,
        ClusterKind::Composite => t.height + 2.0,
        ClusterKind::Region => 0.0,
    }
}

/// The frame of one cluster (without its label), outermost shape first.
#[allow(clippy::too_many_arguments)]
fn cluster_shapes(
    c: &Cluster,
    r: Rect,
    t: &TextBlock,
    alt: bool,
    pal: &Palette,
    lk: &Look,
    look: ConfigLook,
    kind: DiagramKind,
) -> Vec<Element> {
    let slot = slot_colors(c.color_slot, pal);
    match c.kind {
        // Agentflow's `flowGroup` (clusters.js + agentflow styles.ts): rounded, unfilled
        // unless a palette slot or style fills it, 0.75px border.
        ClusterKind::Group if kind == DiagramKind::Agentflow => {
            let style = slot_styled(&c.style, slot.as_ref());
            let fill = style.fill.clone().unwrap_or_else(Color::none);
            let stroke = style
                .stroke
                .clone()
                .unwrap_or_else(|| pal.cluster_border.clone());
            let mut paint = Paint::both(&fill, &stroke, style.stroke_width.unwrap_or(0.75));
            paint.dash = style.dash.clone();
            vec![Element::rect(r, 10.0, paint)]
        }
        ClusterKind::Group => {
            let style = slot_styled(&c.style, slot.as_ref());
            let fill = style
                .fill
                .clone()
                .unwrap_or_else(|| pal.cluster_bkg.clone());
            // The neo look outlines clusters with the node border
            // (`[data-look="neo"].cluster rect`), unless a palette slot or style says otherwise.
            let stroke = style.stroke.clone().unwrap_or_else(|| match look {
                ConfigLook::Neo => pal.node_border.clone(),
                _ => pal.cluster_border.clone(),
            });
            let mut paint = Paint::both(&fill, &stroke, style.stroke_width.unwrap_or(1.0));
            paint.dash = style.dash.clone();
            vec![Element::rect(r, 0.0, paint)]
        }
        ClusterKind::Composite => {
            // `rect.outer` spans the composite and shows as the title strip; `rect.inner`
            // covers the body. The palette tints the strip and colours both outlines.
            let stroke = c
                .style
                .stroke
                .clone()
                .or_else(|| slot.as_ref().map(|s| s.border.clone()))
                .unwrap_or_else(|| pal.node_border.clone());
            let title_bkg = c
                .style
                .fill
                .clone()
                .or_else(|| slot.as_ref().and_then(|s| s.fill.clone()))
                .or_else(|| pal.composite_title_background.clone())
                .unwrap_or_else(|| pal.main_bkg.clone());
            let body = if alt {
                pal.alt_background
                    .clone()
                    .unwrap_or_else(|| Color::new("#f0f0f0"))
            } else {
                pal.composite_background
                    .clone()
                    .unwrap_or_else(|| pal.background.clone())
            };
            let width = c.style.stroke_width.unwrap_or(lk.node_stroke_width);
            let rx = match look {
                ConfigLook::Neo => pal.radius.unwrap_or(5.0),
                _ => 5.0,
            };
            let mut outer = Paint::both(&title_bkg, &stroke, width);
            outer.dash = c.style.dash.clone();
            let mut inner_paint = Paint::both(&body, &stroke, width);
            inner_paint.dash = c.style.dash.clone();
            let th = if t.is_empty() { 0.0 } else { t.height };
            let inner = Rect::new(r.x, r.y + th + 2.0, r.w, (r.h - th - 6.0).max(0.0));
            vec![
                Element::rect(r, rx, outer),
                Element::rect(inner, 0.0, inner_paint),
            ]
        }
        ClusterKind::Region => {
            let stroke = slot
                .as_ref()
                .map(|s| s.border.clone())
                .unwrap_or_else(|| pal.node_border.clone());
            let fill = slot
                .as_ref()
                .and_then(|s| s.fill.clone())
                .or_else(|| pal.alt_background.clone())
                .unwrap_or_else(|| Color::new("#efefef"));
            let mut paint = Paint::both(&fill, &stroke, lk.node_stroke_width);
            paint.dash = Some(vec![10.0, 10.0]);
            vec![Element::rect(r, 0.0, paint)]
        }
    }
}

/// The order in which mermaid hands nodes to the layout. flowDb pushes its subgraphs
/// last-declared first and the vertices after them, so at every level the child
/// subgraphs come in reverse, before the level's own nodes; ELK keeps that model order.
/// Other diagrams keep declaration order.
fn model_order(g: &GraphDiagram, nodes: &[Node], empty_of: &[Option<usize>]) -> Vec<usize> {
    if g.kind != DiagramKind::Flowchart || g.clusters.is_empty() {
        return (0..nodes.len()).collect();
    }
    let placeholder: Vec<bool> = {
        let mut v = vec![false; nodes.len()];
        for p in empty_of.iter().flatten() {
            v[*p] = true;
        }
        v
    };
    fn visit(
        scope: Option<ClusterId>,
        g: &GraphDiagram,
        nodes: &[Node],
        empty_of: &[Option<usize>],
        placeholder: &[bool],
        out: &mut Vec<usize>,
        depth: usize,
    ) {
        if depth > g.clusters.len() {
            return;
        }
        for ci in (0..g.clusters.len()).rev() {
            if g.clusters[ci].parent != scope || Some(ClusterId(ci)) == scope {
                continue;
            }
            match empty_of[ci] {
                Some(p) => out.push(p),
                None => visit(
                    Some(ClusterId(ci)),
                    g,
                    nodes,
                    empty_of,
                    placeholder,
                    out,
                    depth + 1,
                ),
            }
        }
        for (i, n) in nodes.iter().enumerate() {
            if n.cluster == scope && !placeholder[i] {
                out.push(i);
            }
        }
    }
    let mut out = Vec::with_capacity(nodes.len());
    visit(None, g, nodes, empty_of, &placeholder, &mut out, 0);
    // Anything unreachable (a malformed parent chain) keeps its place at the end.
    let mut seen = vec![false; nodes.len()];
    for &i in &out {
        seen[i] = true;
    }
    out.extend((0..nodes.len()).filter(|&i| !seen[i]));
    out.dedup();
    out
}

/// Drop the leading points of `pts` that lie inside `r` and start the path where it
/// leaves `r` instead. Paths that never leave `r` are returned unchanged.
fn cut_at_rect(pts: &[Point], r: &Rect) -> Vec<Point> {
    let inside = |p: Point| p.x > r.x && p.x < r.right() && p.y > r.y && p.y < r.bottom();
    let Some(k) = pts.iter().position(|&p| !inside(p)) else {
        return pts.to_vec();
    };
    if k == 0 {
        return pts.to_vec();
    }
    // Bisect the crossing segment for the border point.
    let (mut a, mut b) = (pts[k - 1], pts[k]);
    for _ in 0..48 {
        let m = a.lerp(b, 0.5);
        if inside(m) {
            a = m;
        } else {
            b = m;
        }
    }
    // The representative member may sit anywhere inside the cluster; leave from the
    // side that faces the next outside point, straight across from it, so the edge
    // reads as leaving the cluster rather than one of its members.
    let next = pts[k];
    let gap_x = (r.x - next.x).max(next.x - r.right()).max(0.0);
    let gap_y = (r.y - next.y).max(next.y - r.bottom()).max(0.0);
    let inset = |lo: f64, len: f64, v: f64| v.clamp(lo + len * 0.1, lo + len * 0.9);
    let b = if gap_y > 0.0 && gap_y >= gap_x {
        let y = if next.y > r.bottom() { r.bottom() } else { r.y };
        Point::new(inset(r.x, r.w, next.x), y)
    } else if gap_x > 0.0 {
        let x = if next.x > r.right() { r.right() } else { r.x };
        Point::new(x, inset(r.y, r.h, next.y))
    } else {
        b
    };
    let mut out = vec![b];
    out.extend_from_slice(&pts[k..]);
    if out.len() >= 2 && out[0].dist(out[1]) < 1e-6 {
        out.remove(0);
    }
    if out.len() < 2 {
        return pts.to_vec();
    }
    out
}

fn node_prefix(kind: DiagramKind) -> &'static str {
    match kind {
        DiagramKind::State => "state",
        DiagramKind::Class => "classId",
        DiagramKind::Er => "entity",
        DiagramKind::Requirement => "requirement",
        DiagramKind::Agentflow => "agentflow",
        _ => "flowchart",
    }
}

fn endpoint_id(g: &GraphDiagram, e: Endpoint) -> String {
    match e {
        Endpoint::Node(NodeId(i)) => g.nodes[i].id.clone(),
        Endpoint::Cluster(ClusterId(c)) => g.clusters[c].id.clone(),
    }
}

/// An edge between a (non-empty) cluster and itself or something inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NestedEdge {
    cluster: ClusterId,
    /// The other end: a node or cluster inside `cluster`, or `cluster` itself (a loop).
    inner: Endpoint,
    /// The edge runs from the cluster into `inner` (else out of `inner` to the cluster).
    inner_is_target: bool,
}

fn nested_edge(
    clusters: &[Cluster],
    nodes: &[Node],
    empty_of: &[Option<usize>],
    e: &Edge,
) -> Option<NestedEdge> {
    let inside = |x: Endpoint, c: ClusterId| match x {
        Endpoint::Node(NodeId(i)) => nodes[i]
            .cluster
            .is_some_and(|nc| is_within(clusters, nc, c)),
        Endpoint::Cluster(d) => is_within(clusters, d, c),
    };
    let solid = |c: ClusterId| empty_of.get(c.0).is_some_and(Option::is_none);
    match (e.from, e.to) {
        (Endpoint::Cluster(c), to) if solid(c) && inside(to, c) => Some(NestedEdge {
            cluster: c,
            inner: to,
            inner_is_target: true,
        }),
        (from, Endpoint::Cluster(c)) if solid(c) && inside(from, c) => Some(NestedEdge {
            cluster: c,
            inner: from,
            inner_is_target: false,
        }),
        _ => None,
    }
}

/// The path of a [`NestedEdge`] and its label position: a loop on the frame's top-right
/// corner for a cluster's edge to itself, otherwise a straight run between the inner
/// end and the frame side it faces in the flow (the side the flow enters through for
/// an edge into the cluster, the side it leaves through for one out of it).
fn nested_route(
    ne: NestedEdge,
    frame: Rect,
    inner: Rect,
    dir: Direction,
    title_h: f64,
    label_w: Option<f64>,
) -> (Vec<Point>, Point) {
    if ne.inner == Endpoint::Cluster(ne.cluster) {
        let ext = 22.0;
        let (cx, cy) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
        let (top, right) = (frame.y, frame.right());
        let pts = vec![
            Point::new(cx + frame.w / 4.0, top),
            Point::new(cx + frame.w / 4.0, top - ext),
            Point::new(right + ext, top - ext),
            Point::new(right + ext, cy),
            Point::new(right, cy),
        ];
        let label = Point::new(right + ext + label_w.unwrap_or(0.0) / 2.0 + 4.0, top - ext);
        return (pts, label);
    }
    let (cx, cy) = (inner.x + inner.w / 2.0, inner.y + inner.h / 2.0);
    // The edge runs in the lane the frame's padding keeps free along one side (clear
    // of the nodes and of the title) and turns into / out of the inner end sideways,
    // so it does not cut through the nodes before / after it in the flow.
    let lane = CLUSTER_PADDING / 2.0;
    if ne.inner_is_target {
        // In from the entry side, down the lane on the cross axis' near side.
        let pts = match dir {
            Direction::TopBottom | Direction::BottomTop => {
                let x = frame.x + lane;
                let entry = if dir == Direction::TopBottom {
                    frame.y
                } else {
                    frame.bottom()
                };
                vec![
                    Point::new(x, entry),
                    Point::new(x, cy),
                    Point::new(inner.x, cy),
                ]
            }
            Direction::LeftRight | Direction::RightLeft => {
                let y = frame.y + title_h + lane;
                let entry = if dir == Direction::LeftRight {
                    frame.x
                } else {
                    frame.right()
                };
                vec![
                    Point::new(entry, y),
                    Point::new(cx, y),
                    Point::new(cx, inner.y),
                ]
            }
        };
        let label = pts[0].lerp(pts[1], 0.5);
        return (pts, label);
    }
    // Out of the cluster: sideways into the far lane, then along it to the exit side.
    let pts = match dir {
        Direction::TopBottom | Direction::BottomTop => {
            let x = frame.right() - lane;
            let exit = if dir == Direction::TopBottom {
                frame.bottom()
            } else {
                frame.y
            };
            vec![
                Point::new(inner.right(), cy),
                Point::new(x, cy),
                Point::new(x, exit),
            ]
        }
        Direction::LeftRight | Direction::RightLeft => {
            let y = frame.bottom() - lane;
            let exit = if dir == Direction::LeftRight {
                frame.right()
            } else {
                frame.x
            };
            vec![
                Point::new(cx, inner.bottom()),
                Point::new(cx, y),
                Point::new(exit, y),
            ]
        }
    };
    let label = pts[1].lerp(pts[2], 0.5);
    (pts, label)
}

fn is_within(clusters: &[Cluster], mut c: ClusterId, ancestor: ClusterId) -> bool {
    for _ in 0..=clusters.len() {
        if c == ancestor {
            return true;
        }
        match clusters[c.0].parent {
            Some(p) => c = p,
            None => return false,
        }
    }
    false
}

fn depth(clusters: &[Cluster], c: ClusterId) -> usize {
    let mut d = 0;
    let mut cur = clusters[c.0].parent;
    while let Some(p) = cur {
        d += 1;
        if d > clusters.len() {
            break;
        }
        cur = clusters[p.0].parent;
    }
    d
}

fn step_points(pts: &[Point], horizontal: bool) -> Vec<Point> {
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if horizontal {
            let mx = (a.x + b.x) / 2.0;
            out.extend([Point::new(mx, a.y), Point::new(mx, b.y), b]);
        } else {
            let my = (a.y + b.y) / 2.0;
            out.extend([Point::new(a.x, my), Point::new(b.x, my), b]);
        }
    }
    out
}

/// Column geometry of a `Shape::EntityTable` node.
struct TableMetrics {
    cols: Vec<f64>,
    header_h: f64,
    row_h: f64,
    /// Left inset of cell text (`PADDING / 2`).
    text_inset: f64,
    w: f64,
    h: f64,
}

/// `erBox.ts`: `text_pad` is `er.entityPadding` (TEXT_PADDING, row/header
/// slack) and `cell_pad` is `er.diagramPadding` (PADDING, per-column slack).
fn table_metrics(node: &Node, text_pad: f64, cell_pad: f64, font: f64) -> TableMetrics {
    let ncols = node
        .sections
        .iter()
        .map(|s| s.lines.len())
        .max()
        .unwrap_or(0);
    let label = TextBlock::label(&node.label, font);
    let line_h = font * 1.5;
    if node.sections.is_empty() {
        // drawRect with labelPaddingX = PADDING, labelPaddingY = 1.5·PADDING,
        // widened to er.minEntityWidth (100).
        return TableMetrics {
            cols: Vec::new(),
            header_h: 0.0,
            row_h: 0.0,
            text_inset: cell_pad / 2.0,
            w: (label.width + 2.0 * cell_pad).max(100.0),
            h: label.height.max(line_h) + 3.0 * cell_pad,
        };
    }
    // Type and name columns are always present; keys / comment only when used.
    let mut cols: Vec<f64> = (0..ncols)
        .map(|j| {
            let used = node
                .sections
                .iter()
                .any(|r| r.lines.get(j).is_some_and(|c| !c.is_empty()));
            if j < 2 || used { cell_pad } else { 0.0 }
        })
        .collect();
    let mut widest: f64 = label.width;
    for row in &node.sections {
        for (j, cell) in row.lines.iter().enumerate() {
            if !cell.is_empty() {
                let tw = crate::text::line_width(cell, font, false);
                widest = widest.max(tw);
                cols[j] = cols[j].max(tw + cell_pad);
            }
        }
    }
    let sum: f64 = cols.iter().sum();
    let used = cols.iter().filter(|c| **c > 0.0).count();
    let need = label.width + 2.0 * cell_pad;
    if used > 0 && need > sum {
        let extra = (need - sum) / used as f64;
        for c in cols.iter_mut().filter(|c| **c > 0.0) {
            *c += extra;
        }
    }
    let sum: f64 = cols.iter().sum();
    let header_h = label.height.max(line_h) + text_pad;
    let row_h = line_h + text_pad;
    TableMetrics {
        w: sum.max(widest + 2.0 * cell_pad),
        h: header_h + row_h * node.sections.len() as f64,
        cols,
        header_h,
        row_h,
        text_inset: cell_pad / 2.0,
    }
}

/// `requirementBox.ts`: fixed `padding = 20` and `gap = 20`.
const REQ_PADDING: f64 = 20.0;
const REQ_GAP: f64 = 20.0;

/// Text blocks and size of a `Shape::Requirement` node.
struct RequirementMetrics {
    stereo: TextBlock,
    name: TextBlock,
    body: TextBlock,
    /// Top of the first text line below the box top (and below the divider).
    top: f64,
    w: f64,
    h: f64,
}

fn requirement_metrics(node: &Node, font: f64) -> RequirementMetrics {
    let section = |i: usize| -> Vec<String> {
        node.sections
            .get(i)
            .map(|s| s.lines.clone())
            .unwrap_or_default()
    };
    let stereo = TextBlock::new(section(0), font, false);
    let name = TextBlock::new(
        crate::text::split_lines(&crate::text::normalize_label(&node.label)),
        font,
        true,
    );
    let body = TextBlock::new(section(1), font, false);
    let w = stereo.width.max(name.width).max(body.width) + REQ_PADDING;
    // Labels are centred on their y offset, so the content box starts half a
    // line above the first one; the box adds `padding` to that content box.
    let mut h = stereo.height + name.height + REQ_GAP;
    if !body.is_empty() {
        h += body.height + REQ_PADDING;
    }
    RequirementMetrics {
        top: REQ_PADDING - font * 0.75,
        stereo,
        name,
        body,
        w,
        h,
    }
}

fn node_elements(
    n: &Node,
    m: &Measured,
    c: Point,
    pal: &Palette,
    lk: &Look,
    font: f64,
) -> (Vec<Element>, Boundary) {
    let (w, h) = (m.w, m.h);
    let r = Rect::centered(c, w, h);
    let (x0, y0, x1) = (r.x, r.y, r.right());
    let default_fill = match n.shape {
        Shape::Note => &pal.note_bkg,
        Shape::StateStart | Shape::ForkBar => &pal.special_state,
        _ => &pal.main_bkg,
    };
    let default_stroke = match n.shape {
        Shape::Note => &pal.note_border,
        Shape::StateStart | Shape::StateEnd | Shape::ForkBar => &pal.special_state,
        _ => &pal.node_border,
    };
    let fill = n.style.fill.clone().unwrap_or_else(|| default_fill.clone());
    let stroke = n
        .style
        .stroke
        .clone()
        .unwrap_or_else(|| default_stroke.clone());
    let mut paint = Paint::both(
        &fill,
        &stroke,
        n.style.stroke_width.unwrap_or(lk.node_stroke_width),
    );
    paint.dash = n.style.dash.clone();
    let text_color = n.style.color.clone().unwrap_or_else(|| {
        if n.shape == Shape::Note {
            pal.note_text.clone()
        } else {
            pal.node_text.clone()
        }
    });

    let mut els = Vec::new();
    let boundary = if let Some(o) = shapes::outline(
        n.shape,
        c,
        w,
        h,
        &ShapeMetrics {
            corner_radius: lk.rounded_rx,
            ..m.metrics
        },
    ) {
        for part in o.parts {
            match part.style {
                PartStyle::Body => {
                    let class = match part.geom {
                        Geometry::Path(_) if n.shape != Shape::Cylinder => "label-container",
                        _ => "basic label-container",
                    };
                    els.push(Element::classed_shape(part.geom, paint.clone(), class));
                }
                PartStyle::Line => els.push(Element::shape(
                    part.geom,
                    Paint::stroke(&stroke, paint.stroke_width),
                )),
                PartStyle::Hidden => {}
            }
        }
        o.boundary
    } else {
        match n.shape {
            // stateStart.ts: r = 7.
            Shape::StateStart => {
                els.push(Element::shape(
                    Geometry::Circle { c, r: 7.0 },
                    Paint::both(&fill, &stroke, 1.0),
                ));
                Boundary::Ellipse(c, 7.0, 7.0)
            }
            // stateEnd.ts: rough circles of diameter 14 and 14·5/14 = 5, both
            // stroked 2px.
            Shape::StateEnd => {
                els.push(Element::shape(
                    Geometry::Circle { c, r: 7.0 },
                    Paint::both(&pal.background, &stroke, 2.0),
                ));
                els.push(Element::shape(
                    Geometry::Circle { c, r: 2.5 },
                    Paint::both(&stroke, &stroke, 2.0),
                ));
                Boundary::Ellipse(c, 7.0, 7.0)
            }
            Shape::ForkBar => {
                els.push(Element::shape(
                    Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::both(&fill, &stroke, 1.0),
                ));
                Boundary::Rect(r)
            }
            Shape::Compartments => {
                let Some(layout) = &m.compartments else {
                    return (els, Boundary::Rect(r));
                };
                els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: r,
                        rx: layout.rx,
                    },
                    paint.clone(),
                    "basic label-container",
                ));
                let mut y = y0;
                for (i, (sec, block)) in n.sections.iter().zip(&m.sections).enumerate() {
                    let sh = layout.heights.get(i).copied().unwrap_or(0.0);
                    if i > 0 {
                        els.push(Element::classed_shape(
                            Geometry::Path(vec![
                                PathCmd::MoveTo(Point::new(x0, y)),
                                PathCmd::LineTo(Point::new(x1, y)),
                            ]),
                            Paint::stroke(&stroke, 1.0),
                            "divider",
                        ));
                    }
                    if !block.is_empty() {
                        let top = y + layout.text_top.get(i).copied().unwrap_or(0.0);
                        let center = Point::new(c.x, top + block.height / 2.0);
                        let mut t = Text::centered(
                            center,
                            &block.lines,
                            font,
                            block.line_height,
                            &text_color,
                        );
                        t.bold = sec.bold;
                        if !sec.centered {
                            t = t.anchored(Anchor::Start, x0 + layout.inset);
                        }
                        els.push(Element::classed_text(t, "label"));
                    }
                    y += sh;
                }
                return (els, Boundary::Rect(r));
            }
            Shape::EntityTable => {
                let colors = TableColors {
                    paint: &paint,
                    stroke: &stroke,
                    text: &text_color,
                    // Rows are striped with `rowOdd`; the colour themes paint every row
                    // with the slot fill instead (`er/styles.ts` `.node path`): no stripe.
                    odd_row: pal.er_row_odd.as_ref().filter(|_| {
                        slot_colors(n.color_slot, pal)
                            .and_then(|s| s.fill)
                            .is_none()
                    }),
                };
                return (
                    entity_table_elements(n, m, r, colors, font),
                    Boundary::Rect(r),
                );
            }
            Shape::Requirement => {
                let rm = requirement_metrics(n, font);
                els.push(Element::classed_shape(
                    Geometry::Rect { rect: r, rx: 0.0 },
                    paint.clone(),
                    "basic label-container",
                ));
                let mut y = y0 + rm.top;
                for (block, class) in [(&rm.stereo, "stereotype"), (&rm.name, "nodeLabel")] {
                    if !block.is_empty() {
                        let mut t = Text::centered(
                            Point::new(c.x, y + block.height / 2.0),
                            &block.lines,
                            font,
                            block.line_height,
                            &text_color,
                        );
                        t.bold = block.bold;
                        els.push(Element::classed_text(t, class));
                    }
                    y += block.height;
                }
                if !rm.body.is_empty() {
                    let divider = y0 + rm.stereo.height + rm.name.height + REQ_GAP;
                    els.push(Element::classed_shape(
                        Geometry::Path(vec![
                            PathCmd::MoveTo(Point::new(x0, divider)),
                            PathCmd::LineTo(Point::new(x1, divider)),
                        ]),
                        Paint::stroke(&stroke, 1.0),
                        "divider",
                    ));
                    // Body lines are left-aligned `padding / 2` in from the edge.
                    let top = divider + rm.top;
                    let t = Text::centered(
                        Point::new(c.x, top + rm.body.height / 2.0),
                        &rm.body.lines,
                        font,
                        rm.body.line_height,
                        &text_color,
                    )
                    .anchored(Anchor::Start, x0 + REQ_PADDING / 2.0);
                    els.push(Element::classed_text(t, "label"));
                }
                return (els, Boundary::Rect(r));
            }
            _ => Boundary::Rect(r),
        }
    };
    if n.shape == Shape::CollapsedGroup {
        // collapsedGroup.ts: a dashed separator 8px below the title, inset 8px, and three
        // dots centred in the 20px indicator row (`.collapsed-indicator`: nodeBorder, 50%).
        let sep_y = y0 + shapes::COLLAPSED_PAD + m.block.height + 8.0;
        let mut sep = Paint::stroke(&pal.cluster_border, 0.75);
        sep.dash = Some(vec![3.0, 3.0]);
        els.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(x0 + 8.0, sep_y)),
                PathCmd::LineTo(Point::new(x1 - 8.0, sep_y)),
            ]),
            sep,
            "collapsed-separator",
        ));
        let mut dot = Paint::fill(&pal.node_border);
        dot.opacity = Some(0.5);
        for i in [-1.0, 0.0, 1.0] {
            els.push(Element::classed_shape(
                Geometry::Circle {
                    c: Point::new(c.x + 10.0 * i, sep_y + 10.0),
                    r: 2.5,
                },
                dot.clone(),
                "collapsed-indicator",
            ));
        }
    }
    if !m.block.is_empty() && shapes::shows_label(n.shape) {
        let mut t = Text::centered(
            c.plus(m.label_offset),
            &m.block.lines,
            font,
            m.block.line_height,
            &text_color,
        );
        t.bold = m.block.bold;
        els.push(Element::classed_text(t, "nodeLabel"));
    }
    (els, boundary)
}

/// Paints of an entity table.
struct TableColors<'a> {
    paint: &'a Paint,
    stroke: &'a Color,
    text: &'a Color,
    odd_row: Option<&'a Color>,
}

fn entity_table_elements(
    n: &Node,
    m: &Measured,
    r: Rect,
    colors: TableColors<'_>,
    font: f64,
) -> Vec<Element> {
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let mut els = vec![Element::classed_shape(
        Geometry::Rect { rect: r, rx: 0.0 },
        colors.paint.clone(),
        "basic label-container",
    )];
    let Some(t) = &m.table else {
        return els;
    };
    let header_center = if n.sections.is_empty() {
        r.center()
    } else {
        Point::new(r.center().x, y0 + t.header_h / 2.0)
    };
    els.push(Element::classed_text(
        Text::centered(
            header_center,
            &m.block.lines,
            font,
            m.block.line_height,
            colors.text,
        ),
        "nodeLabel",
    ));
    let line_paint = Paint::stroke(colors.stroke, 1.0);
    for (i, row) in n.sections.iter().enumerate() {
        let ry = y0 + t.header_h + t.row_h * i as f64;
        if let Some(odd) = colors.odd_row.filter(|_| i % 2 == 0) {
            els.push(Element::classed_shape(
                Geometry::Rect {
                    rect: Rect::new(x0, ry, r.w, t.row_h),
                    rx: 0.0,
                },
                Paint::fill(odd),
                "row-rect-odd",
            ));
        }
        els.push(Element::line(
            Point::new(x0, ry),
            Point::new(x1, ry),
            line_paint.clone(),
        ));
        let mut x = x0;
        for (j, cw) in t.cols.iter().enumerate() {
            if *cw <= 0.0 {
                continue;
            }
            if let Some(cell) = row.lines.get(j).filter(|s| !s.is_empty()) {
                let center = Point::new(x, ry + t.row_h / 2.0);
                let tx = Text::centered(
                    center,
                    std::slice::from_ref(cell),
                    font,
                    font * 1.5,
                    colors.text,
                )
                .anchored(Anchor::Start, x + t.text_inset);
                els.push(Element::classed_text(tx, "label"));
            }
            x += cw;
        }
    }
    if !n.sections.is_empty() {
        // Column dividers below the header.
        let used: Vec<f64> = t.cols.iter().copied().filter(|c| *c > 0.0).collect();
        let mut x = x0;
        for cw in used.iter().take(used.len().saturating_sub(1)) {
            x += cw;
            els.push(Element::line(
                Point::new(x, y0 + t.header_h),
                Point::new(x, y1),
                line_paint.clone(),
            ));
        }
        // Redraw the outline on top of the row fills.
        els.push(Element::shape(
            Geometry::Rect { rect: r, rx: 0.0 },
            Paint::stroke(colors.stroke, colors.paint.stroke_width),
        ));
    }
    els
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_styles() {
        let s = StyleOverrides::parse(
            "fill:#f9f,stroke:rgb(1,2,3),stroke-width:4px,color:#fff,stroke-dasharray: 5 5",
        );
        assert_eq!(s.fill.unwrap().css(), "#f9f");
        assert_eq!(s.stroke.unwrap().css(), "rgb(1,2,3)");
        assert_eq!(s.stroke_width, Some(4.0));
        assert_eq!(s.dash, Some(vec![5.0, 5.0]));
    }

    #[test]
    fn whole_cluster_edges_are_cut_at_the_border() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Leaves through the bottom, straight below the next outside point.
        let pts = vec![
            Point::new(20.0, 20.0),
            Point::new(30.0, 80.0),
            Point::new(60.0, 150.0),
            Point::new(60.0, 200.0),
        ];
        let cut = cut_at_rect(&pts, &r);
        assert_eq!(cut[0], Point::new(60.0, 100.0));
        assert_eq!(&cut[1..], &pts[2..]);
        // A path that never leaves the box is left alone.
        assert_eq!(cut_at_rect(&pts[..2], &r), pts[..2].to_vec());
    }

    fn render_flowchart(src: &str) -> (GraphDiagram, Scene) {
        let g = crate::diagram::flowchart::parse(src, None).unwrap();
        let cfg = crate::config::MermaidConfig::default().flowchart;
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let scene = render(&g, &cfg, &pal);
        (g, scene)
    }

    fn cluster_group<'a>(scene: &'a Scene, id: &str) -> &'a [Element] {
        let Element::Group { children, .. } = &scene.elements[0] else {
            panic!("clusters group first");
        };
        children
            .iter()
            .find_map(|c| match c {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i == &format!("cluster-{id}") => Some(children.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no cluster {id}"))
    }

    #[test]
    fn empty_subgraph_draws_only_its_title() {
        let (_, scene) =
            render_flowchart("flowchart TB\nsubgraph empty\nend\nsubgraph full\na\nend");
        let empty = cluster_group(&scene, "empty");
        assert_eq!(empty.len(), 1, "{empty:?}");
        assert!(matches!(empty[0], Element::Text { .. }));
        let full = cluster_group(&scene, "full");
        assert!(full.iter().any(|e| matches!(e, Element::Shape { .. })));
    }

    #[test]
    fn subgraph_title_is_flush_with_the_top_and_widens_the_frame() {
        let (_, scene) = render_flowchart(
            "flowchart TB\nsubgraph s [A title much wider than the node inside it]\na\nend",
        );
        let els = cluster_group(&scene, "s");
        let Some(Element::Shape {
            geom: Geometry::Rect { rect, .. },
            ..
        }) = els.iter().find(|e| matches!(e, Element::Shape { .. }))
        else {
            panic!("frame rect: {els:?}");
        };
        let Some(Element::Text { text: t, .. }) =
            els.iter().find(|e| matches!(e, Element::Text { .. }))
        else {
            panic!("title: {els:?}");
        };
        let title = TextBlock::label("A title much wider than the node inside it", 16.0);
        assert!(rect.w >= title.width + CLUSTER_TITLE_PAD - 1e-6);
        assert!((t.y - 16.0 * 0.35 - (rect.y + title.height / 2.0)).abs() < 1e-6);
    }

    fn frame_of(scene: &Scene, id: &str) -> Rect {
        cluster_group(scene, id)
            .iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    ..
                } => Some(*rect),
                _ => None,
            })
            .expect("frame rect")
    }

    /// Points (path vertices and curve end points) of the edge drawn with group id `id`.
    fn edge_points(scene: &Scene, id: &str) -> Vec<Point> {
        let Element::Group { children, .. } = &scene.elements[1] else {
            panic!("edgePaths second");
        };
        let path = children
            .iter()
            .find_map(|c| match c {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i == id => children.first(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no edge {id}: {children:?}"));
        let Element::Shape {
            geom: Geometry::Path(cmds),
            ..
        } = path
        else {
            panic!("path: {path:?}");
        };
        cmds.iter()
            .filter_map(|c| match c {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) => Some(*p),
                PathCmd::Close => None,
            })
            .collect()
    }

    #[test]
    fn edges_between_a_cluster_and_its_own_contents() {
        let src =
            "flowchart TB\nsubgraph A\na1 --> a2\nend\nstart --> A\nA --> A\nA --> a1\na2 --> A";
        let (g, scene) = render_flowchart(src);
        let n = |id: &str| NodeId(g.nodes.iter().position(|n| n.id == id).unwrap());
        let ne: Vec<Option<NestedEdge>> = g
            .edges
            .iter()
            .map(|e| nested_edge(&g.clusters, &g.nodes, &[None], e))
            .collect();
        let a = ClusterId(0);
        assert_eq!(ne[1], None, "start --> A is an ordinary cluster edge");
        assert_eq!(
            ne[2],
            Some(NestedEdge {
                cluster: a,
                inner: Endpoint::Cluster(a),
                inner_is_target: true
            })
        );
        assert_eq!(
            ne[3].map(|x| (x.inner, x.inner_is_target)),
            Some((Endpoint::Node(n("a1")), true))
        );
        assert_eq!(
            ne[4].map(|x| (x.inner, x.inner_is_target)),
            Some((Endpoint::Node(n("a2")), false))
        );

        let frame = frame_of(&scene, "A");
        let eps = 1e-6;
        // A --> A: a loop outside the frame, from its top back to its right side.
        let lp = edge_points(&scene, "L-A-A-2");
        assert!((lp[0].y - frame.y).abs() < eps, "{lp:?} {frame:?}");
        assert!(
            lp.iter()
                .any(|p| p.x > frame.right() + 10.0 && p.y < frame.y)
        );
        assert!((lp.last().unwrap().x - frame.right()).abs() < 10.0);
        // A --> a1: from the frame's top edge into a1 (not between a1 and a2).
        let into = edge_points(&scene, "L-A-a1-3");
        assert!((into[0].y - frame.y).abs() < eps, "{into:?}");
        let a1 = scene_node_center(&g, &scene, "a1");
        let end = *into.last().unwrap();
        assert!(
            (end.y - a1.y).abs() < 1.0 && end.x < a1.x,
            "{end:?} vs {a1:?}"
        );
        // a2 --> A: from a2 out to the frame's bottom edge (not a self-loop on a2).
        let out = edge_points(&scene, "L-a2-A-4");
        let a2 = scene_node_center(&g, &scene, "a2");
        assert!((out[0].y - a2.y).abs() < 1.0 && out[0].x > a2.x, "{out:?}");
        assert!(
            out.last().unwrap().y > frame.bottom() - 8.0,
            "{out:?} {frame:?}"
        );
    }

    /// Centre of the node `id`'s label-container shape.
    fn scene_node_center(g: &GraphDiagram, scene: &Scene, id: &str) -> Point {
        let i = g.nodes.iter().position(|n| n.id == id).unwrap();
        let Element::Group { children, .. } = &scene.elements[3] else {
            panic!("nodes fourth");
        };
        let prefix = format!("{}-{id}-{i}", node_prefix(g.kind));
        let shape = children
            .iter()
            .find_map(|c| match c {
                Element::Group {
                    id: Some(x),
                    children,
                    ..
                } if *x == prefix => children.iter().find_map(|e| match e {
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        ..
                    } => Some(*rect),
                    _ => None,
                }),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no node {prefix}"));
        Point::new(shape.x + shape.w / 2.0, shape.y + shape.h / 2.0)
    }

    #[test]
    fn edge_to_an_ancestor_cluster_leaves_the_node() {
        // fc_edge_to_ancestor: x --> outer must not become a loop on x.
        let src = "flowchart LR\nsubgraph outer\nsubgraph inner\nx --> z\nend\nend\nx --> outer\nouter --> y";
        let (g, scene) = render_flowchart(src);
        let frame = frame_of(&scene, "outer");
        let pts = edge_points(&scene, "L-x-outer-1");
        let x = scene_node_center(&g, &scene, "x");
        assert!((pts[0].x - x.x).abs() < 1.0 && pts[0].y > x.y, "{pts:?}");
        let end = *pts.last().unwrap();
        assert!((end.x - frame.right()).abs() < 8.0, "{end:?} {frame:?}");
    }

    #[test]
    fn subgraph_title_margin_moves_titles_and_edge_labels() {
        let g =
            crate::diagram::flowchart::parse("flowchart TB\nsubgraph S\na -->|lbl| b\nend", None)
                .unwrap();
        let cfg = crate::config::MermaidConfig::default().flowchart;
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let texts = |scene: &Scene, group: usize| -> Vec<f64> {
            let Element::Group { children, .. } = &scene.elements[group] else {
                panic!()
            };
            let mut ys = Vec::new();
            fn walk(e: &Element, ys: &mut Vec<f64>) {
                match e {
                    Element::Text { text, .. } => ys.push(text.y),
                    Element::Group { children, .. } => children.iter().for_each(|c| walk(c, ys)),
                    _ => {}
                }
            }
            children.iter().for_each(|c| walk(c, &mut ys));
            ys
        };
        let plain = render(&g, &cfg, &pal);
        let opts = RenderOptions {
            subgraph_title_margin: (20.0, 10.0),
            ..Default::default()
        };
        let moved = render_with(&g, &cfg, &pal, &opts);
        let (t0, t1) = (texts(&plain, 0), texts(&moved, 0));
        assert!(
            (t1[0] - t0[0] - 20.0).abs() < 1e-6,
            "title {t0:?} -> {t1:?}"
        );
        let (l0, l1) = (texts(&plain, 2), texts(&moved, 2));
        assert!(
            (l1[0] - l0[0] - 15.0).abs() < 1e-6,
            "label {l0:?} -> {l1:?}"
        );
        assert_eq!(
            frame_of(&plain, "S"),
            frame_of(&moved, "S"),
            "layout unchanged"
        );
    }
}
