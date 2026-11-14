//! Graph IR shared by flowcharts, state diagrams and class diagrams, plus the
//! pure renderer `GraphDiagram -> Scene` (layout → shapes → edges → labels).

use super::elkedges;
use super::markers::{self, End, MarkerColors};
use super::shapes::{self, Boundary, PartStyle, ShapeMetrics};
use crate::config::{Curve, GraphConfig, LayoutAlgorithm, Look as ConfigLook};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::{
    self, Direction, EdgeRouting, LayoutCluster, LayoutEdge, LayoutInput, LayoutNode, OrthoSpacing,
};
use crate::linejump;
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::{ColorSlot, DropShadow, Palette, SlotColors};
use std::collections::HashMap;

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
    /// Flowchart `sm-circ` / `start`: `stateStart.ts` without the state diagram's
    /// `.state-start` CSS, so it takes `.node circle` (`mainBkg` / `nodeBorder`).
    SmallCircle,
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
    /// `datastore` / `data-store`: top and bottom rules only.
    Datastore,
    /// `folder` / `directory`.
    Folder,
    /// `bucket`.
    Bucket,
    /// `console`: terminal window with a `>_` glyph.
    Console,
    /// `browser`: window with a chrome bar.
    Browser,
    /// `person`: head over a rounded body.
    Person,
    /// `bang`.
    Bang,
    /// `cloud`.
    Cloud,
    /// `brace` / `comment` / `brace-l`.
    BraceLeft,
    /// `brace-r`.
    BraceRight,
    /// `braces`.
    Braces,
    /// `bolt` / `com-link` / `lightning-bolt`.
    Bolt,
    /// `docs` / `st-doc` / `stacked-document`.
    StackedDocument,
    /// `bow-rect` / `stored-data`.
    BowTieRect,
    /// `tag-doc` / `tagged-document`.
    TaggedDocument,
    /// `flag` / `paper-tape`.
    PaperTape,
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
    /// `font-style: italic` / `oblique`.
    pub italic: Option<bool>,
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
                "font-style" => st.italic = Some(matches!(v, "italic" | "oblique")),
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
            italic: other.italic.or(self.italic),
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
    /// Width of a label that `createText` wrapped: its `div` is given the whole
    /// wrapping width, however wide the longest line is.
    pub wrapped_width: Option<f64>,
    /// A class box's `«annotation»` (classBox draws only the first one), stacked
    /// above the name in normal weight.
    pub annotation: Option<String>,
    /// A `collapsedGroup` standing in for a folded subgraph: how many expanded
    /// subgraphs `getData` pushes before it (subgraphs come before vertices).
    pub group_seq: Option<usize>,
    /// Where the diagram's `getData` hands this node to the layout, among the
    /// nodes and clusters that carry one (state diagrams: dataFetcher's order).
    pub model_seq: Option<usize>,
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
            wrapped_width: None,
            annotation: None,
            group_seq: None,
            model_seq: None,
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
    /// `noteGroup`: the invisible frame round a state's note. Its label is the
    /// note's text, which ELK still keeps clear above the note (it is the
    /// container's node label) though nothing paints it there.
    NoteGroup,
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
    /// That statement said `TD` (flowDb keeps a subgraph's `TD` as written,
    /// which mermaid's dagre renderer does not take for `TB`).
    pub direction_td: bool,
    pub kind: ClusterKind,
    /// As [`Node::model_seq`].
    pub model_seq: Option<usize>,
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
            direction_td: false,
            kind: ClusterKind::Group,
            model_seq: None,
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

/// The dash pattern of a dotted or dashed edge under the neo look
/// (`generateDashArray`'s dash and gap).
const NEO_DASH: [f64; 2] = [2.0, 2.0];

/// rough.js's stroke width where nothing sets one (`handDrawnShapeStyles`).
const ROUGH_STROKE_WIDTH: f64 = 1.3;

/// Per-diagram visual conventions.
struct Look {
    edge_width: f64,
    thick_width: f64,
    dotted_dash: Vec<f64>,
    dashed_dash: Vec<f64>,
    node_stroke_width: f64,
    /// Corner radius of `Shape::Rounded` (mermaid's `roundedRect`: theme `radius` ?? 5).
    rounded_rx: f64,
    /// Whether the diagram is drawn in the neo look.
    neo: bool,
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
        neo: false,
    }
}

/// Apply the theme's `strokeWidth` / `radius` the way each diagram's stylesheet reads them:
/// flowchart nodes `strokeWidth ?? 1` and edges `?? 2`; class and state `strokeWidth || 1`
/// for both; ER and requirement use `strokeWidth` only under the neo look (1px otherwise).
/// `radius` reaches flowchart rounded nodes (`roundedRect.ts`).
fn themed_look(mut lk: Look, kind: DiagramKind, look: ConfigLook, pal: &Palette) -> Look {
    lk.neo = look == ConfigLook::Neo;
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
    /// A class box's `«annotation»` block, stacked on top of its name (section 0).
    annotation: Option<TextBlock>,
    compartments: Option<CompartmentLayout>,
    table: Option<TableMetrics>,
    label_offset: Point,
    /// The label's width after labelHelper's `minWidth`.
    label_w: f64,
    metrics: ShapeMetrics,
    w: f64,
    h: f64,
}

fn compartment_layout(
    kind: DiagramKind,
    blocks: &[TextBlock],
    annotation: Option<&TextBlock>,
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
    // above and below; an empty members box is 2·GAP (also for empty methods),
    // the methods below it moved down another GAP/2 (`membersGroupHeightForMethods`;
    // the divider between stays at the empty group's height), and a class with
    // neither gets two 1.5·GAP boxes.
    let all_empty = blocks.iter().skip(1).all(|b| b.is_empty());
    let methods_shift = |i: usize| {
        if i == 2 && !all_empty && blocks[1].is_empty() {
            pad / 2.0
        } else {
            0.0
        }
    };
    let annotation_h = annotation.map(content).unwrap_or(0.0);
    let heights: Vec<f64> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match (i, b.is_empty()) {
            (0, _) => annotation_h + content(b) + 2.0 * pad,
            (_, false) => content(b) + 2.0 * pad + methods_shift(i),
            _ if all_empty => 1.5 * pad,
            _ => 2.0 * pad,
        })
        .collect();
    let layout = CompartmentLayout {
        // classBox moves every group by `y + PADDING` from textHelper's frame, where
        // an HTML label's first line starts half a line above its origin: the text
        // sits `2·PADDING - lineHeight/2` below its divider (13.5px, not 12, at
        // 14px). SVG text keeps PADDING (its `-4` and line box cancel out).
        text_top: blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                methods_shift(i)
                    + if b.svg || b.is_empty() {
                        pad
                    } else {
                        2.0 * pad - b.line_height / 2.0
                    }
            })
            .collect(),
        heights,
        inset: pad,
        rx: 0.0,
    };
    let h = layout.heights.iter().sum();
    // textHelper centres the annotation and the name on x = 0 while members and
    // methods start there, so the text bbox reaches half the widest centred label
    // to the left and the farther of that half and the widest member to the right.
    let half = blocks
        .first()
        .into_iter()
        .chain(annotation)
        .map(|b| b.width / 2.0)
        .fold(0.0, f64::max);
    let members = blocks.iter().skip(1).map(|b| b.width).fold(0.0, f64::max);
    let w = if kind == DiagramKind::Class {
        half + half.max(members)
    } else {
        max_w
    };
    (layout, w + 2.0 * pad, h)
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
    wrap_width: f64,
    svg: bool,
    table_scale: f64,
) -> Measured {
    let mut block = TextBlock::label(&node.label, font).html();
    let bold = node.style.bold == Some(true);
    if svg {
        // createText's SVG path: wrapped (markdownAutoWrap or not) and cut inside
        // words by `splitLineToFitWidth`, 1.1em lines.
        let lines = if wrap_width > 0.0 && !block.is_empty() {
            crate::text::svg_wrap_lines(&block.lines, wrap_width, font, bold)
        } else {
            block.lines
        };
        block = TextBlock::svg(lines, font, bold);
    } else if wrap_width > 0.0 && !block.is_empty() {
        let raw = block.lines.clone();
        block = TextBlock::new(wrap_lines(&raw, wrap_width, font, bold), font, bold).html();
        // A label too wide for one line is laid out in a `display: table` box of
        // the wrapping width (or its widest word): the label's box is that wide
        // however short its lines come out.
        let natural = TextBlock::new(raw.clone(), font, bold).html().width;
        if natural > wrap_width {
            block.width = block
                .width
                .max(crate::text::html_wrap_width(&raw, wrap_width, font, bold));
        }
    } else if bold {
        block = TextBlock::new(block.lines, font, true).html();
    }
    // An HTML label takes the node's `font-style`; an SVG one does not (its
    // inner tspans set `font-style="normal"`). Emphasis marks take no width.
    if !svg && node.style.italic == Some(true) {
        use crate::text::mark::{ITALIC_OFF, ITALIC_ON};
        for l in block.lines.iter_mut().filter(|l| !l.is_empty()) {
            *l = format!("{ITALIC_ON}{l}{ITALIC_OFF}");
        }
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
        block.height.max(if block.is_empty() || svg {
            0.0
        } else {
            font * 1.5
        }),
    );
    // Notes carry their own padding: classDb `config.class.padding ?? 6`,
    // state dataFetcher `config.flowchart.padding` (15).
    let node_pad = match (node.shape, kind) {
        (Shape::Note, DiagramKind::Class) => 6.0,
        (Shape::Note, DiagramKind::State) => 15.0,
        _ => pad,
    };
    let metrics = ShapeMetrics::for_look(node.shape, node_pad, look).with_label(tw, th);
    let mut sections = Vec::new();
    let mut annotation = None;
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
                    .enumerate()
                    .map(|(i, s)| match (svg, kind) {
                        (false, _) => TextBlock::new(s.lines.clone(), font, s.bold).html(),
                        // shapeUtil's textHelper: one `<text>` per line, annotation and
                        // name box on box, members and methods 3px apart.
                        (true, DiagramKind::Class) => TextBlock::svg_stacked(
                            s.lines.clone(),
                            font,
                            s.bold,
                            Some(if i == 0 { 0.0 } else { 3.0 }),
                        ),
                        (true, _) => TextBlock::svg(s.lines.clone(), font, s.bold),
                    })
                    .collect();
                annotation = node.annotation.as_ref().map(|a| {
                    let lines = vec![a.clone()];
                    if svg {
                        TextBlock::svg_stacked(lines, font, false, Some(0.0))
                    } else {
                        TextBlock::new(lines, font, false).html()
                    }
                });
                let (layout, w, h) = compartment_layout(kind, &sections, annotation.as_ref(), pad);
                compartments = Some(layout);
                (w, h)
            }
            Shape::EntityTable => {
                let t = table_metrics(node, pad, cell_pad, font, table_scale);
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
        annotation,
        compartments,
        table,
        label_offset,
        label_w: tw,
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
    /// `markdownAutoWrap: false`: markdown-string node and edge labels keep their
    /// lines unwrapped (`markdownToHTML` turns their spaces into `&nbsp;`).
    pub markdown_no_wrap: bool,
    /// ER only: `erBox` scales its paddings by 1.25 (`PADDING *= 1.25`) unless the
    /// root `htmlLabels` is truthy, which it is not by default.
    pub er_scaled_padding: bool,
    /// Padding around the drawing in the viewBox, when it is not `diagram_padding`
    /// (the ER renderer passes a fixed 8 to `setupViewPortForSVG`).
    pub view_padding: Option<f64>,
    /// ER groups under ELK: the width their titles are measured at for the layout
    /// (`labelBBox`, the group's `wrappingWidth`, i.e. `flowchart.wrappingWidth`).
    pub group_title_wrap: f64,
    /// ER under dagre: groups drawn as mermaid's dagre wrapper draws its cluster
    /// nodes (see [`nest_cluster_nodes`]).
    pub cluster_nodes: bool,
}

/// Edge labels wrap at 200px: `insertEdgeLabel` calls `createText` without a
/// width, so its default (`width = 200`) applies to every edge label.
pub(crate) const EDGE_LABEL_WRAP: f64 = 200.0;

/// An edge label measured as `insertEdgeLabel` lays it out (wrapped at
/// [`EDGE_LABEL_WRAP`] by the browser's line breaking).
fn edge_label_block(label: &str, font: f64) -> TextBlock {
    TextBlock::new(
        crate::text::wrap(
            label,
            EDGE_LABEL_WRAP,
            font,
            false,
            crate::text::BreakRule::Browser,
        ),
        font,
        false,
    )
    .html()
}

/// Padding of an SVG edge label's background on each side (`createFormattedText`):
/// the label's bounding box, and so its size in the layout, includes it.
const SVG_EDGE_LABEL_PAD: f64 = 2.0;
/// Where ELK's coordinates start in ours: the root's 12px padding.
const ELK_ORIGIN: f64 = 12.0;

/// [`edge_label_block`] for `htmlLabels: false`: SVG text wrapped at the same 200px.
fn svg_edge_label_block(label: &str, font: f64) -> TextBlock {
    let lines = crate::text::svg_wrap_lines(
        &crate::text::label_lines(label),
        EDGE_LABEL_WRAP,
        font,
        false,
    );
    TextBlock::svg(lines, font, false)
}

/// Word-wrap already-decoded label lines at `width` with the browser's line breaking,
/// as labelHelper's `createText` does (mermaid 12 wraps flowchart / state node labels
/// at `wrappingWidth`).
fn wrap_lines(lines: &[String], width: f64, font: f64, bold: bool) -> Vec<String> {
    crate::text::wrap_lines(lines, width, font, bold, crate::text::BreakRule::Browser)
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
    // The class stylesheet paints edge labels `mainBkg`; requirement and ER have their
    // own theme variables (redux / colour themes) over `edgeLabelBackground`.
    let edge_label_bg = match (g.kind, &pal.er_edge_label_background) {
        (DiagramKind::Er, Some(bg)) => bg,
        (DiagramKind::Class, _) => &pal.main_bkg,
        (DiagramKind::Requirement, _) => pal
            .requirement_edge_label_background
            .as_ref()
            .unwrap_or(&pal.edge_label_background),
        _ => &pal.edge_label_background,
    };
    let font = pal.font_size;
    let horizontal = g.direction.is_horizontal();
    let elk = cfg.layout == LayoutAlgorithm::Elk;

    // A cluster with nothing in it is a plain leaf upstream: its title alone, with no
    // visible box. A placeholder node (sized to the title) stands in for it. A
    // flowchart's ELK adapter hands ELK such a group with no size at all (it is not
    // a parent, so it gets neither labels nor a size), and its painter draws the
    // title from that point down; mermaid's dagre renderer paints it as an ordinary
    // `rect` node labelled with the title.
    let flow_leaf = g.kind == DiagramKind::Flowchart;
    let dagre_leaf = flow_leaf && !elk;
    let mut nodes = g.nodes.clone();
    let mut empty_of: Vec<Option<usize>> = vec![None; g.clusters.len()];
    for (ci, c) in g.clusters.iter().enumerate() {
        let has_member = nodes.iter().any(|n| {
            n.cluster
                .map(|nc| is_within(&g.clusters, nc, ClusterId(ci)))
                .unwrap_or(false)
        }) || g.clusters.iter().any(|o| o.parent == Some(ClusterId(ci)));
        if !has_member {
            let mut placeholder = if dagre_leaf {
                let mut n = Node::new(c.id.clone(), c.title.clone(), Shape::Rect);
                n.style = c.style.clone();
                n
            } else {
                Node::new(format!("__empty_{}", c.id), "", Shape::Invisible)
            };
            placeholder.cluster = c.parent;
            empty_of[ci] = Some(nodes.len());
            nodes.push(placeholder);
        }
    }

    let mut cluster_titles: Vec<TextBlock> = g
        .clusters
        .iter()
        .map(|c| {
            let t = TextBlock::label(&c.title, font);
            if cfg.html_edge_labels {
                t.html()
            } else {
                TextBlock::svg(t.lines, font, false)
            }
        })
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
                opts.min_label_width.max(cfg.min_node_width),
                if opts.markdown_no_wrap
                    && cfg.html_labels
                    && crate::text::is_markdown_string(&n.label)
                {
                    0.0
                } else {
                    cfg.wrapping_width
                },
                !cfg.html_labels,
                if opts.er_scaled_padding { 1.25 } else { 1.0 },
            )
        })
        .collect();
    for (ci, p) in empty_of.iter().enumerate() {
        if let Some(p) = *p {
            if dagre_leaf {
                // A plain node, without the vertices' minimum label width.
                measured[p] = measure_node(
                    &nodes[p],
                    g.kind,
                    cfg.padding,
                    cfg.diagram_padding,
                    font,
                    horizontal,
                    cfg.look,
                    0.0,
                    cfg.wrapping_width,
                    !cfg.html_labels,
                    1.0,
                );
            } else if flow_leaf {
                measured[p].w = 0.0;
                measured[p].h = 0.0;
            } else {
                let t = &cluster_titles[ci];
                measured[p].w = t.width + CLUSTER_TITLE_PAD;
                measured[p].h = t.height;
            }
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
    // mermaid's own dagre renderer (see `layout::dagre`) takes such an edge as it
    // is: to the cluster's anchor node, inside it.
    let dagre_engine = !elk
        && matches!(
            g.kind,
            DiagramKind::Flowchart
                | DiagramKind::Agentflow
                | DiagramKind::State
                | DiagramKind::Class
                | DiagramKind::Er
        );
    let nested: Vec<Option<NestedEdge>> = g
        .edges
        .iter()
        .map(|e| nested_edge(&g.clusters, &nodes, &empty_of, e).filter(|_| !dagre_engine))
        .collect();
    // Under ELK a direct member's edge to or from its own container is laid
    // out as it is: inside the container, to or from a port dummy in its first
    // or last layer (`CompoundGraphPreprocessor`), not as a loop.
    let member_edge = |ne: Option<NestedEdge>| {
        elk && ne.is_some_and(|ne| {
            matches!(ne.inner, Endpoint::Node(NodeId(i)) if nodes[i].cluster == Some(ne.cluster))
        })
    };
    // Anything deeper inside runs there too, through its own containers' ports,
    // to or from a port dummy of the container's, which takes the side against
    // the flow (its net flow): the route comes back around inside the frame.
    // A container's edge to itself is ELK's self-loop on the container's node
    // in its parent's graph: on its side across the flow, where its ports are
    // free; with external port dummies inside, its ports keep their sides and
    // the loop runs around from one flow side to the other.
    let deep_edge = |ne: Option<NestedEdge>| elk && !member_edge(ne) && ne.is_some();
    let layout_ends = |ei: usize, e: &Edge| -> ((usize, Option<usize>), (usize, Option<usize>)) {
        match nested[ei] {
            None => (layout_end(e.from, true), layout_end(e.to, false)),
            ne if member_edge(ne) || deep_edge(ne) => {
                (layout_end(e.from, true), layout_end(e.to, false))
            }
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
                .map(|l| {
                    if !cfg.html_edge_labels {
                        svg_edge_label_block(l, font)
                    } else if opts.markdown_no_wrap && crate::text::is_markdown_string(l) {
                        TextBlock::label(l, font).html()
                    } else {
                        edge_label_block(l, font)
                    }
                })
                .filter(|b| !b.is_empty())
        })
        .collect();

    // Model order: the order mermaid hands its nodes to the layout.
    let perm = model_order(g, &nodes, &empty_of);
    let mut pos_of = vec![0; nodes.len()];
    for (k, &old) in perm.iter().enumerate() {
        pos_of[old] = k;
    }
    let mut input = LayoutInput {
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
                    // The ELK adapter hands ELK no edge length (`--->` is as long as `-->`).
                    minlen: if elk { 1 } else { e.minlen },
                    // A cluster's edge to itself is laid out as a loop but drawn
                    // against the frame: under ELK its label reserves no loop room.
                    // One to something inside runs there, its label a layer of its own.
                    label: lb
                        .as_ref()
                        .filter(|_| {
                            !(elk
                                && nested[ei].is_some()
                                && !member_edge(nested[ei])
                                && !deep_edge(nested[ei]))
                        })
                        .map(|b| {
                            // `insertEdgeLabel` sizes the edge by the label's bbox;
                            // an SVG label's includes its background's padding.
                            let pad = if b.svg { 2.0 * SVG_EDGE_LABEL_PAD } else { 0.0 };
                            (b.width + pad, b.height + pad)
                        }),
                    from_cluster,
                    to_cluster,
                    terminals: if elk { terminal_sizes(e) } else { [None; 2] },
                }
            })
            .collect(),
        clusters: g
            .clusters
            .iter()
            .zip(&cluster_titles)
            .map(|(c, t)| LayoutCluster {
                parent: c.parent.map(|p| p.0),
                title_height: cluster_title_height(c.kind, t, elk),
                // A note group reserves no width for its unpainted title
                // (`groupTitleSizeOptions`).
                min_width: if t.is_empty() || c.kind == ClusterKind::NoteGroup {
                    0.0
                } else {
                    t.width + title_pad
                },
                label_width: if t.is_empty() || c.kind == ClusterKind::NoteGroup {
                    0.0
                } else {
                    t.width
                },
                direction: c.direction,
            })
            .collect(),
        direction: g.direction,
        node_sep: layer_spacing(cfg.layout, cfg.node_spacing),
        rank_sep: layer_spacing(cfg.layout, cfg.rank_spacing),
        edge_sep: 20.0,
        cluster_padding: CLUSTER_PADDING,
        inner_node_sep: CLUSTER_NODE_SPACING,
        inner_rank_sep: CLUSTER_RANK_SPACING,
        routing: edge_routing(cfg.layout),
        dagre_groups: !elk,
        dagre: None,
    };
    if dagre_engine {
        input.dagre = Some(dagre_model(g, &nodes, &empty_of, &pos_of));
    }
    // ER group titles wrap at the group's laid-out width. Under ELK mermaid lays
    // out once, with each title measured at the wrapping width, and wraps it again
    // at the frame's width when painting (below); under dagre a probe layout gives
    // the width to wrap at before the real one.
    let er_groups = g.kind == DiagramKind::Er;
    // A composite state's title is a markdown label, which the ELK adapter
    // measures wrapped at the state's `wrappingWidth` (it paints it unwrapped).
    if elk && g.kind == DiagramKind::State {
        for (ci, c) in g.clusters.iter().enumerate() {
            if c.kind != ClusterKind::Composite
                || empty_of[ci].is_some()
                || cluster_titles[ci].is_empty()
            {
                continue;
            }
            let t = wrapped_group_title(&c.title, cfg.wrapping_width, font);
            input.clusters[ci].title_height = cluster_title_height(c.kind, &t, elk);
            input.clusters[ci].min_width = t.width + title_pad;
            input.clusters[ci].label_width = t.width;
        }
    }
    if er_groups && elk {
        for (ci, c) in g.clusters.iter().enumerate() {
            if empty_of[ci].is_some() || cluster_titles[ci].is_empty() {
                continue;
            }
            let t = wrapped_group_title(&c.title, opts.group_title_wrap, font);
            input.clusters[ci].title_height = cluster_title_height(c.kind, &t, elk);
            input.clusters[ci].min_width = t.width + title_pad;
            input.clusters[ci].label_width = t.width;
        }
    } else if er_groups {
        wrap_er_cluster_titles(
            g,
            &mut input,
            &mut cluster_titles,
            &empty_of,
            font,
            title_pad,
        );
    }
    let (raw, extras) = match &input.dagre {
        Some(model) if dagre_engine => layout::dagre::layout_full(&input, model),
        // To mermaid's ELK adapter an empty subgraph is no container at all,
        // only its point (see the placeholders above).
        _ if elk && flow_leaf => (layout_without_empty(&input, &empty_of), Default::default()),
        _ => (layout::layout(&input), Default::default()),
    };
    let mut out = layout::LayoutOutput {
        nodes: (0..nodes.len()).map(|i| raw.nodes[pos_of[i]]).collect(),
        ..raw
    };
    // A fork bar drawn in a graph laid out across the diagram's direction.
    for (i, n) in nodes.iter().enumerate() {
        let dir = extras.node_dirs.get(pos_of[i]).copied().flatten();
        if n.shape == Shape::ForkBar
            && dir.is_some_and(|d| d.is_horizontal() != g.direction.is_horizontal())
        {
            let m = &mut measured[i];
            std::mem::swap(&mut m.w, &mut m.h);
        }
    }
    if elk {
        let sizes: Vec<(f64, f64)> = measured.iter().map(|m| (m.w, m.h)).collect();
        // In the order mermaid's ELK result lists the edges (as handed to
        // ELK), each from the end ELK laid it out from.
        let (handed, turned) = layout::elk_edge_handover(&input);
        let anchors: Vec<(usize, Point)> = handed
            .iter()
            .map(|&ei| (ei, &g.edges[ei], &out.edges[ei]))
            // A container's edge to something inside, laid out as it is, has
            // its inner end where ELK put it.
            .filter(|&(ei, _, r)| {
                (nested[ei].is_none() || member_edge(nested[ei]) || deep_edge(nested[ei]))
                    && r.points.len() >= 2
            })
            .flat_map(|(ei, e, r)| {
                let mut ends = vec![(e.from, r.points[0]), (e.to, r.points[r.points.len() - 1])];
                if turned.get(ei).copied().unwrap_or(false) {
                    ends.reverse();
                }
                if let Some(ne) = nested[ei] {
                    ends.retain(|&(end, _)| end == ne.inner);
                }
                ends.into_iter().filter_map(|(end, p)| match end {
                    Endpoint::Node(NodeId(i)) => Some((i, p)),
                    // An empty subgraph is its point (of no size).
                    Endpoint::Cluster(c) => empty_of[c.0].map(|i| (i, p)),
                })
            })
            .collect();
        align_degenerate_nodes(&mut out.nodes, &sizes, &anchors);
    }

    let mut bounds = Bounds::default();
    for r in &extras.hidden {
        bounds.add_rect(*r);
    }
    let mut cluster_els = Vec::new();
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    let mut node_els = Vec::new();
    // What each drawn element belongs to (cluster, edge, node index), for
    // `nest_cluster_nodes`.
    let mut cluster_tag: Vec<usize> = Vec::new();
    let mut edge_tag: Vec<usize> = Vec::new();
    let mut label_tag: Vec<usize> = Vec::new();
    let mut node_tag: Vec<usize> = Vec::new();

    // Clusters, outermost first.
    let mut cluster_rects = out.clusters.clone();
    for (ci, t) in cluster_titles.iter_mut().enumerate() {
        let r = &mut cluster_rects[ci];
        if let Some(p) = empty_of[ci] {
            *r = Rect::centered(out.nodes[p], measured[p].w, measured[p].h);
            continue;
        }
        if er_groups && elk && !t.is_empty() {
            *t = wrapped_group_title(&g.clusters[ci].title, r.w, font);
        }
        let need = if t.is_empty() || g.clusters[ci].kind == ClusterKind::NoteGroup {
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
        if !t.is_empty()
            && c.kind != ClusterKind::NoteGroup
            && !(dagre_leaf && empty_of[ci].is_some())
        {
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
            if empty_of[ci].is_some() {
                // The title is all there is of it to see.
                bounds.add_rect(Rect::centered(center, t.width, t.height));
            }
            children.push(Element::classed_text(
                t.centered_text(center, &color),
                "cluster-label",
            ));
        }
        cluster_els.push(Element::Group {
            id: Some(format!("cluster-{}", c.id)),
            class: Some("cluster".into()),
            children,
        });
        cluster_tag.push(ci);
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
        let (mut els, boundary) = node_elements(n, m, c, pal, &lk, font, g.kind);
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
            node_tag.push(i);
        }
    }

    // Every rendered edge's points, for ELK's line hops (`applyElkLineJumps`), with the
    // index in `edge_els` of its drawn group (none for an invisible edge, which still
    // takes part: upstream paints it with an invisible stroke).
    let mut hop_edges: Vec<(linejump::EdgeGeom, Option<usize>)> = Vec::new();
    // Per drawn edge (by its element's index) with one: its neo inline pattern.
    let mut neo_dashes: HashMap<usize, Vec<f64>> = HashMap::new();
    // Every edge's clipped route and label position first: under ELK mermaid then
    // straightens the routes' terminal steps and separates opposite labels.
    let mut routed: Vec<Option<(Vec<Point>, Option<Point>)>> = vec![None; g.edges.len()];
    // Per edge: the ports mermaid's clipped route still holds after its first
    // point and before its last, beside the outline points (see `kept_ports`).
    let mut kept: Vec<[Option<Point>; 2]> = vec![[None; 2]; g.edges.len()];
    for (ei, (e, route)) in g.edges.iter().zip(&out.edges).enumerate() {
        let mut pts = route.points.clone();
        let mut label_at = route.label;
        if let Some(ne) = nested[ei].filter(|_| elk) {
            // Under ELK the layout's own route, from (or to) the inner end's
            // border: a member's to a port dummy of its container's in its
            // first or last layer, which lines up with it (`CompoundGraphPreprocessor`).
            if pts.len() < 2 {
                continue;
            }
            // The other end is a cluster, so mermaid leaves `cutter2` out:
            // the node end is cut where the line from the node's centre to
            // its port leaves the outline (`applyStart/EndIntersectionIfNeeded`),
            // the port kept beside it.
            if let Endpoint::Node(NodeId(i)) = ne.inner {
                if ne.inner_is_target {
                    pts.reverse();
                }
                let port = pts[0];
                pts[0] = boundaries[i].clip(port);
                let r = Rect::centered(out.nodes[i], measured[i].w, measured[i].h);
                let neo = cfg.look == ConfigLook::Neo;
                let head = if ne.inner_is_target { e.end } else { e.start };
                let offset = markers::pullback_for(head, neo).max(markers::hidden_end(head, true));
                let k = usize::from(ne.inner_is_target);
                kept[ei][k] = kept_port(port, Some(pts[0]), offset, r, false);
                if ne.inner_is_target {
                    pts.reverse();
                }
                keep_ports_off_the_line(&mut pts, &mut kept[ei]);
            }
        } else if let Some(ne) = nested[ei] {
            let frame = cluster_rects[ne.cluster.0];
            let inner_rect = match ne.inner {
                Endpoint::Node(NodeId(i)) => {
                    Rect::centered(out.nodes[i], measured[i].w, measured[i].h)
                }
                Endpoint::Cluster(c) => cluster_rects[c.0],
            };
            let lw = edge_labels[ei].as_ref().map(|b| b.width + 8.0);
            let title_h = cluster_title_height(
                g.clusters[ne.cluster.0].kind,
                &cluster_titles[ne.cluster.0],
                elk,
            );
            // The flow direction inside the cluster, as the layout used it (a cluster's
            // own `direction` is dropped when an edge crosses its border).
            let dir = out
                .cluster_directions
                .get(ne.cluster.0)
                .copied()
                .unwrap_or(g.direction);
            // The other nodes, which the route keeps clear of where it can.
            let obstacles: Vec<Rect> = (0..nodes.len())
                .filter(|&i| ne.inner != Endpoint::Node(NodeId(i)))
                .map(|i| Rect::centered(out.nodes[i], measured[i].w, measured[i].h))
                .collect();
            let (p, l) = nested_route(ne, frame, inner_rect, dir, title_h, lw, &obstacles);
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
        // (mermaid's dagre renderer loops only an edge written from a thing to
        // itself; one between a cluster and something inside it is routed.)
        let is_loop = if dagre_engine {
            e.from == e.to
        } else {
            nested[ei].is_some() || resolve(e.from, true) == resolve(e.to, false)
        };
        if elk && is_loop && nested[ei].is_none() {
            // An orthogonal self-loop leaves and re-enters through ports on the node's side.
            if let (Endpoint::Node(NodeId(a)), Endpoint::Node(NodeId(b))) = (e.from, e.to) {
                pts[0] = clip_along(&boundaries[a], pts[0], pts[1]);
                pts.reverse();
                pts[0] = clip_target_end(&boundaries[b], &pts, elk);
                pts.reverse();
            }
        }
        if dagre_engine && is_loop && nested[ei].is_none() && pts.len() >= 3 {
            // mermaid's compact loop: its first and last corner are replaced by
            // where the lines from the node's centre to the next ones leave it.
            if let (Endpoint::Node(NodeId(a)), Endpoint::Node(NodeId(b))) = (e.from, e.to) {
                let n = pts.len();
                pts[0] = boundaries[a].clip(pts[1]);
                pts[n - 1] = boundaries[b].clip(pts[n - 2]);
            }
        }
        let raw = extras.raw_points.get(ei).filter(|p| p.len() >= 2);
        // A cluster's node as mermaid cuts against it: dagre's centre, the
        // frame's size.
        let frame = |c: usize| {
            let r = cluster_rects[c];
            let centre = extras
                .cluster_centres
                .get(c)
                .copied()
                .flatten()
                .unwrap_or_else(|| r.center());
            // The shape leaves the node its drawn rect's `getBBox()` height (a
            // subgraph's `rect` its width too): an `SVGRect`, of floats.
            let bbox = geom::chromium_f32;
            let w = if g.clusters[c].kind == ClusterKind::Group {
                bbox(r.w)
            } else {
                r.w
            };
            (centre, w, bbox(r.h))
        };
        let to_cluster = matches!(e.to, Endpoint::Cluster(_));
        let from_cluster = matches!(e.from, Endpoint::Cluster(_));
        if let (true, false, Some(raw)) = (dagre_engine, is_loop, raw) {
            if to_cluster || from_cluster {
                // `insertEdge`: the route clipped at its nodes (an end on a
                // cluster's anchor as dagre left it), then, to a cluster, dagre's
                // own route cut at the frame and, from one, that cut at its frame.
                // (A route from a cluster starts where the line from the frame's
                // centre to its first bend leaves the frame, as mermaid draws it.)
                let n = pts.len();
                match e.from {
                    Endpoint::Node(NodeId(i)) => pts[0] = clip_end(&boundaries[i], &pts, false),
                    Endpoint::Cluster(c) => {
                        let (centre, w, h) = frame(c.0);
                        pts[0] = geom::intersect_box(centre, w, h, pts[1]);
                    }
                }
                // From a cluster laid out on its own (no `intersect`), the
                // other end is left on dagre's box too.
                let unclipped = matches!(e.from, Endpoint::Cluster(c)
                    if extras.extracted.get(c.0).copied().unwrap_or(false));
                if let (Endpoint::Node(NodeId(i)), false) = (e.to, unclipped) {
                    let rev: Vec<Point> = pts.iter().rev().copied().collect();
                    pts[n - 1] = clip_end(&boundaries[i], &rev, false);
                } else {
                    pts[n - 1] = raw[raw.len() - 1];
                }
                if let Endpoint::Cluster(c) = e.to {
                    pts = cut_path_at_intersect(raw, frame(c.0));
                }
                if let Endpoint::Cluster(c) = e.from {
                    pts.reverse();
                    pts = cut_path_at_intersect(&pts, frame(c.0));
                    pts.reverse();
                }
                // A route cut down to one point is drawn as that point (an
                // empty path), its label kept.
                match pts.len() {
                    0 => continue,
                    1 => pts.push(pts[0]),
                    _ => {}
                }
                routed[ei] = Some((pts, label_at));
                continue;
            }
        }
        if !is_loop {
            let ports = (pts[0], pts[pts.len() - 1]);
            // An edge to / from a whole cluster is cut where it crosses the cluster's
            // border (mermaid's `cutPathAtIntersect` for `fromCluster` / `toCluster`).
            // Under ELK, an edge with a cluster at one end leaves `cutter2`
            // out: its node end is cut where the line from the node's centre
            // to its port leaves the outline (`applyStart/EndIntersectionIfNeeded`),
            // not along the edge's last segment, and keeps a tail however short.
            let by_centre = elk && (from_cluster || to_cluster);
            match e.from {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0], elk),
                Endpoint::Node(NodeId(i)) if by_centre => pts[0] = boundaries[i].clip(pts[0]),
                Endpoint::Node(NodeId(i)) => pts[0] = clip_end(&boundaries[i], &pts, elk),
            }
            pts.reverse();
            match e.to {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0], elk),
                Endpoint::Node(NodeId(i)) if by_centre => pts[0] = boundaries[i].clip(pts[0]),
                Endpoint::Node(NodeId(i)) => pts[0] = clip_target_end(&boundaries[i], &pts, elk),
            }
            pts.reverse();
            if pts.len() < 2 {
                continue;
            }
            if elk {
                let neo = cfg.look == ConfigLook::Neo;
                let offset =
                    |h: Head| markers::pullback_for(h, neo).max(markers::hidden_end(h, true));
                let node = |end: Endpoint| match end {
                    Endpoint::Node(NodeId(i)) => Some(i),
                    Endpoint::Cluster(_) => None,
                };
                if let Some(i) = node(e.from) {
                    let r = Rect::centered(out.nodes[i], measured[i].w, measured[i].h);
                    // ELK lays the node out at its size as the browser
                    // measured it (single precision), mermaid clips at the
                    // outline it drew (double): a shape other than a box
                    // meets the port a hair off it.
                    let f32_exact = |v: f64| f64::from(v as f32) == v;
                    let noisy =
                        !matches!(boundaries[i], Boundary::Rect(_) | Boundary::Cylinder(..))
                            && !(f32_exact(measured[i].w) && f32_exact(measured[i].h));
                    let clip = if noisy { None } else { Some(pts[0]) };
                    kept[ei][0] = kept_port(ports.0, clip, offset(e.start), r, false);
                }
                if let Some(i) = node(e.to) {
                    let r = Rect::centered(out.nodes[i], measured[i].w, measured[i].h);
                    kept[ei][1] = kept_port(
                        ports.1,
                        Some(pts[pts.len() - 1]),
                        offset(e.end),
                        r,
                        !by_centre,
                    );
                }
                if by_centre {
                    keep_ports_off_the_line(&mut pts, &mut kept[ei]);
                }
            }
        }
        routed[ei] = Some((pts, label_at));
    }
    if elk {
        let mut routes: Vec<Option<Vec<Point>>> = routed
            .iter()
            .map(|r| r.as_ref().map(|r| r.0.clone()))
            .collect();
        let mut labels: Vec<Option<elkedges::Label>> = routed
            .iter()
            .zip(&edge_labels)
            .map(|(r, b)| {
                let (r, b) = (r.as_ref()?, b.as_ref()?);
                Some(elkedges::Label {
                    center: r.1?,
                    width: b.width,
                    height: b.height,
                })
            })
            .collect();
        // mermaid straightens its own routes, which hold the ports beside the
        // outline points; a route left alone keeps ours.
        let theirs: Vec<Option<Vec<Point>>> = routes
            .iter()
            .zip(&kept)
            .map(|(r, k)| {
                let mut r = r.clone()?;
                if let Some(p) = k[0] {
                    r.insert(1, p);
                }
                if let Some(p) = k[1] {
                    r.insert(r.len() - 1, p);
                }
                Some(r)
            })
            .collect();
        let mut straightened = theirs.clone();
        elkedges::straighten_edge_terminals(&mut straightened, &mut labels, ELK_ORIGIN);
        for (((r, before), after), k) in routes.iter_mut().zip(theirs).zip(straightened).zip(&kept)
        {
            if after != before {
                *r = after.map(|a| without_kept_ports(a, *k));
            }
        }
        let pairs: Vec<(String, String)> = g
            .edges
            .iter()
            .map(|e| {
                let (a, b) = (endpoint_id(g, e.from), endpoint_id(g, e.to));
                if a <= b { (a, b) } else { (b, a) }
            })
            .collect();
        let pair_ids: Vec<usize> = pairs
            .iter()
            .map(|p| pairs.iter().position(|q| q == p).unwrap_or(0))
            .collect();
        elkedges::separate_opposite_edge_labels(&mut labels, &pair_ids);
        for ((r, route), label) in routed.iter_mut().zip(routes).zip(labels) {
            if let (Some(r), Some(route)) = (r.as_mut(), route) {
                r.0 = route;
                if let Some(l) = label {
                    r.1 = Some(l.center);
                }
            }
        }
    }
    // Under ELK the terminal labels (class cardinalities) are ELK's end labels.
    let mut terminal_at: Vec<[Option<Point>; 2]> = vec![[None; 2]; g.edges.len()];
    if elk {
        let mut terms = Vec::new();
        for (ei, e) in g.edges.iter().enumerate() {
            let sizes = terminal_sizes(e);
            for (k, (end, size)) in [e.from, e.to].into_iter().zip(sizes).enumerate() {
                if let (Endpoint::Node(NodeId(node)), Some(_)) = (end, size) {
                    let b = terminal_block(if k == 0 { &e.start_label } else { &e.end_label });
                    terms.push(elkedges::Terminal {
                        edge: ei,
                        start: k == 0,
                        node,
                        // Class relations: `startLabelRight`, `endLabelLeft`.
                        right: k == 0,
                        width: b.width,
                        height: b.height,
                        pad: terminal_pad(if k == 0 { e.start } else { e.end }),
                    });
                }
            }
        }
        if !terms.is_empty() {
            let routes: Vec<Option<Vec<Point>>> = routed
                .iter()
                .map(|r| r.as_ref().map(|r| r.0.clone()))
                .collect();
            let boxes: Vec<Rect> = (0..nodes.len())
                .map(|i| Rect::centered(out.nodes[i], measured[i].w, measured[i].h))
                .collect();
            let centre_labels: Vec<elkedges::Label> = routed
                .iter()
                .zip(&edge_labels)
                .filter_map(|(r, b)| {
                    let (r, b) = (r.as_ref()?, b.as_ref()?);
                    Some(elkedges::Label {
                        center: r.1?,
                        width: b.width,
                        height: b.height,
                    })
                })
                .collect();
            let placed = elkedges::place_terminal_labels(
                &routes,
                &terms,
                &boxes,
                &cluster_rects,
                &centre_labels,
            );
            for (t, at) in terms.iter().zip(placed) {
                terminal_at[t.edge][usize::from(!t.start)] = at;
            }
        }
    }
    for (ei, e) in g.edges.iter().enumerate() {
        edge_tag.resize(edge_els.len(), ei.saturating_sub(1));
        label_tag.resize(label_els.len(), ei.saturating_sub(1));
        let Some((mut pts, mut label_at)) = routed[ei].take() else {
            continue;
        };
        // `insertEdge`'s `points` (the cut route) and the point it probes the
        // drawn path for.
        let cut = dagre_engine.then(|| pts.clone());
        // A neo edge takes the `*-margin` markers (`useMargin`) and the neo
        // barb's pull-back, and leaves its first and last `markerOffsets2` px
        // undrawn by a dash pattern (see `neo_dash`), not by a shorter line.
        let neo = cfg.look == ConfigLook::Neo;
        let trim = |h: Head| markers::pullback_for(h, neo);
        let (start_trim, end_trim) = (trim(e.start), trim(e.end));
        let hidden = (
            markers::hidden_end(e.start, neo),
            markers::hidden_end(e.end, neo),
        );
        if elk {
            hop_edges.push((
                linejump::EdgeGeom {
                    points: pts.clone(),
                    start_offset: start_trim,
                    end_offset: end_trim,
                    rounded: true,
                    corner_radius: EDGE_CORNER_RADIUS,
                },
                (e.line != LineKind::Invisible).then_some(edge_els.len()),
            ));
        }
        if e.line == LineKind::Invisible {
            continue;
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
        if !elk {
            pts = geom::fix_corners(&pts);
        }
        let mut heads = Vec::new();
        let last = pts.len() - 1;
        // A route cut down to one point: d3 draws `M p Z` with the end marker's
        // offset added to x, and the browser orients the marker along +x.
        let degenerate = dagre_engine && pts.len() == 2 && pts[0] == pts[1];
        if degenerate {
            let v = Point::new(pts[0].x + end_trim, pts[0].y);
            pts = vec![v, v];
        }
        let (end_dir, start_dir) = if degenerate {
            (Point::new(1.0, 0.0), Point::new(-1.0, 0.0))
        } else {
            (
                pts[last].minus(pts[last - 1]).norm(),
                pts[0].minus(pts[1]).norm(),
            )
        };
        let end_tip = if degenerate {
            pts[last].plus(end_dir.scale(end_trim))
        } else {
            pts[last]
        };
        let colors = MarkerColors {
            line: &stroke,
            background: &pal.main_bkg,
        };
        heads.extend(markers::elements(
            e.end,
            End::End,
            end_tip,
            end_dir,
            colors,
            width,
            neo,
        ));
        let end_heads = heads.len();
        if !degenerate {
            heads.extend(markers::elements(
                e.start,
                End::Start,
                pts[0],
                start_dir,
                colors,
                width,
                neo,
            ));
        }
        let (end_back, start_back) = (end_trim, start_trim);
        if dagre_engine && !degenerate {
            let keyed = |h: Head, back: f64| markers::has_marker_offset(h).then_some(back);
            pts = line_with_offset(&pts, keyed(e.start, start_back), keyed(e.end, end_back));
            // The browser stops drawing a path at its first NaN: one that starts
            // with one is not drawn at all, nor are its markers.
            if !pts[0].x.is_finite() {
                heads.clear();
            } else if !pts[last].x.is_finite() {
                heads.drain(..end_heads);
            }
        } else if end_back > 0.0 && pts[last].dist(pts[last - 1]) > end_back {
            pts[last] = pts[last].minus(end_dir.scale(end_back));
        }
        if !dagre_engine && start_back > 0.0 && pts[0].dist(pts[1]) > start_back {
            pts[0] = pts[0].minus(start_dir.scale(start_back));
        }
        if !dagre_engine {
            for p in &pts {
                bounds.add_point(*p);
            }
        }
        let cmds = match cfg.curve {
            // d3's line through a single point.
            _ if degenerate => vec![geom::PathCmd::MoveTo(pts[0]), geom::PathCmd::Close],
            // The ELK adapter overrides every routed edge's curve with `rounded`.
            _ if elk => geom::rounded_path(&pts, EDGE_CORNER_RADIUS),
            Curve::Basis => geom::basis_curve(&pts),
            Curve::Linear => geom::polyline(&pts),
            Curve::Step => geom::polyline(&step_points(&pts, horizontal)),
        };
        if dagre_engine {
            // The drawing's bbox holds the curve itself, not its control points.
            if let Some(r) = geom::path_bbox(&cmds) {
                bounds.add_rect(r);
            }
        }
        // `insertEdge`'s `pointsHasChanged`: the route was cut at a frame, or
        // the drawn path does not mention its middle point. `positionEdgeLabel`
        // then moves the label by as much as cutting the route moved the
        // route's midpoint, and places the terminal labels along the cut route
        // (along dagre's own points otherwise).
        let raw = extras.raw_points.get(ei).filter(|p| !p.is_empty());
        let changed = match (&cut, raw) {
            (Some(cut), Some(_)) => {
                let to_frame =
                    matches!(e.from, Endpoint::Cluster(_)) || matches!(e.to, Endpoint::Cluster(_));
                let origin = extras.edge_origins.get(ei).copied().unwrap_or_default();
                to_frame || !label_coordinate_in_path(cut[cut.len() / 2], &cmds, origin)
            }
            _ => false,
        };
        if let (true, Some(cut), Some(at), Some(orig)) = (changed, &cut, label_at, raw) {
            let (a, b) = (calc_label_position(cut), calc_label_position(orig));
            label_at = Some(Point::new(at.x + a.x - b.x, at.y + a.y - b.y));
        }
        let terminal_path = if changed { cut.clone() } else { raw.cloned() };
        let mut paint = Paint::stroke(&stroke, width);
        // A neo edge writes its pattern inline, outranking the stylesheet's
        // (an edge's own style comes after it).
        let patterned = matches!(e.line, LineKind::Dotted | LineKind::Dashed);
        let inline = neo
            .then(|| neo_dash(geom::path_length(&cmds), hidden, patterned))
            .flatten();
        if let Some(d) = &inline {
            neo_dashes.insert(edge_els.len(), d.clone());
        }
        paint.dash = e.style.dash.clone().or(inline).or_else(|| match e.line {
            _ if neo => None,
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
            // An SVG label's background is its text box padded by 2px on every side,
            // half transparent (`.edgeLabel rect { opacity: 0.5 }`).
            let (r, bg) = if block.svg {
                let pad = 2.0 * SVG_EDGE_LABEL_PAD;
                // The state stylesheet fills it with `labelBackgroundColor` (stateBkg).
                let fill = if g.kind == DiagramKind::State {
                    &pal.main_bkg
                } else {
                    edge_label_bg
                };
                (
                    Rect::centered(at, block.width + pad, block.height + pad),
                    Paint::fill(fill).with_opacity(0.5),
                )
            } else {
                // An HTML label's background is its `<p>`'s: the text box itself.
                (
                    Rect::centered(at, block.width, block.height),
                    Paint::fill(edge_label_bg),
                )
            };
            // The drawing's bbox holds an HTML label's `foreignObject`, the text
            // box. (Under dagre, whose output is pinned, 2px wider each side.)
            bounds.add_rect(if block.svg || elk || dagre_engine {
                r
            } else {
                Rect::centered(at, block.width + 4.0, block.height)
            });
            label_els.push(Element::group(
                "edgeLabel",
                vec![
                    Element::rect(r, 0.0, bg),
                    Element::text(block.centered_text(at, &text_color)),
                ],
            ));
        }
        if elk {
            for (txt, at) in [&e.start_label, &e.end_label]
                .into_iter()
                .zip(terminal_at[ei])
            {
                if let (Some(_), Some(at)) = (txt, at) {
                    let b = terminal_block(txt);
                    bounds.add_rect(Rect::centered(at, b.width, b.height));
                    label_els.push(Element::classed_text(
                        Text::centered(at, &b.lines, b.font_size, b.line_height, &text_color),
                        "edgeTerminals",
                    ));
                }
            }
        }
        for (k, (txt, anchor_pt, toward)) in [
            (&e.start_label, pts[0], pts[1]),
            (&e.end_label, pts[last], pts[last - 1]),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(t) = txt.as_ref().filter(|_| !elk) else {
                continue;
            };
            let b = TextBlock::label(t, font * 0.875);
            let at = match &terminal_path {
                // `calcTerminalLabelPosition`, past the end's marker (classDb
                // names one, `none` included, for every relation end).
                Some(path) if path.len() >= 2 => {
                    calc_terminal_label_position(TERMINAL_MARKER_SIZE, k == 1, path)
                }
                _ => {
                    let dir = toward.minus(anchor_pt).norm();
                    let perp = Point::new(-dir.y, dir.x);
                    anchor_pt.plus(dir.scale(16.0)).plus(perp.scale(10.0))
                }
            };
            // `setTerminalWidth`: an HTML label's box is at least 9px a character wide.
            let w = if b.svg {
                b.width
            } else {
                b.width.max(9.0 * t.encode_utf16().count() as f64)
            };
            bounds.add_rect(Rect::new(
                at.x - b.width / 2.0,
                at.y - b.height / 2.0,
                w,
                b.height,
            ));
            label_els.push(Element::classed_text(
                Text::centered(at, &b.lines, b.font_size, b.line_height, &text_color),
                "edgeTerminals",
            ));
        }
    }

    edge_tag.resize(edge_els.len(), g.edges.len().saturating_sub(1));
    if dagre_engine {
        // `getEdgesToRender` draws the merged self-loops after the other edges
        // (their labels keep their places).
        let is_loop = |k: &usize| g.edges.get(*k).is_some_and(|e| e.from == e.to);
        let (mut els, mut tags): (Vec<Element>, Vec<usize>) = (Vec::new(), Vec::new());
        let mut loops: Vec<(Element, usize)> = Vec::new();
        for (el, tag) in edge_els.drain(..).zip(edge_tag.drain(..)) {
            if is_loop(&tag) {
                loops.push((el, tag));
            } else {
                els.push(el);
                tags.push(tag);
            }
        }
        for (el, tag) in loops {
            els.push(el);
            tags.push(tag);
        }
        edge_els = els;
        edge_tag = tags;
    }
    label_tag.resize(label_els.len(), g.edges.len().saturating_sub(1));

    if elk {
        let (geoms, slots): (Vec<_>, Vec<_>) = hop_edges.into_iter().unzip();
        let hopped = linejump::line_hops(&geoms, linejump::ELK_JUMP_RADIUS, cfg.line_hops);
        for (cmds, slot) in hopped.into_iter().zip(slots) {
            if let (Some(cmds), Some(k)) = (cmds, slot) {
                let len = geom::path_length(&cmds);
                set_edge_path(&mut edge_els[k], cmds);
                if let Some(d) = neo_dashes.get(&k) {
                    redash_hopped(&mut edge_els[k], d, len);
                }
            }
        }
    }

    // The layout's origin: mermaid's ELK layout puts the root's contents inside
    // its 12px padding, dagre inside its 8px `marginx` / `marginy`; the viewBox
    // (`setupViewPortForSVG`) then starts at the drawing's bbox less the padding.
    // (The port of mermaid's own dagre renderer already lays out inside them.)
    let origin = if elk {
        ELK_ORIGIN
    } else if dagre_engine {
        0.0
    } else {
        8.0
    };
    let shift = |els: Vec<Element>| -> Vec<Element> {
        els.into_iter()
            .map(|e| e.translated(origin, origin))
            .collect()
    };
    let (cluster_els, edge_els, label_els, node_els) = (
        shift(cluster_els),
        shift(edge_els),
        shift(label_els),
        shift(node_els),
    );
    let mut bounds = {
        let r = bounds.rect();
        let mut b = Bounds::default();
        b.add_rect(Rect::new(r.x + origin, r.y + origin, r.w, r.h));
        b
    };

    // Title above everything: `insertTitle` puts an unstyled `<text>` with its
    // baseline at `y = -titleTopMargin`, centred on the drawing's bbox.
    let mut title_els = Vec::new();
    let title = g.title.as_ref().filter(|t| !t.trim().is_empty());
    if let Some(t) = title {
        let content = bounds.rect();
        let baseline = -cfg.title_top_margin;
        // Flowchart, state and agentflow stylesheets give their title class
        // `font-size: 18px; fill: textColor`; ER's, class's (whose stylesheet
        // styles `.classTitleText` instead) and requirement's are unstyled, so
        // they take the root font.
        let (size, class) = match g.kind {
            DiagramKind::Er => (font, "erDiagramTitleText"),
            DiagramKind::Class => (font, "classDiagramTitleText"),
            DiagramKind::Requirement => (font, "requirementDiagramTitleText"),
            DiagramKind::State => (18.0, "statediagramTitleText"),
            DiagramKind::Agentflow => (18.0, "agentflowTitleText"),
            _ => (18.0, "flowchartTitleText"),
        };
        let tw = crate::text::line_width(t, size, false);
        let ascent = (0.905 * size).round();
        let cx = content.x + content.w / 2.0;
        bounds.add_rect(Rect::new(
            cx - tw / 2.0,
            baseline - ascent,
            tw,
            crate::text::svg_line_box(size, t),
        ));
        let lines = vec![t.clone()];
        let center = Point::new(cx, baseline - size * 0.35);
        title_els.push(Element::classed_text(
            Text::centered(center, &lines, size, size, &pal.text_color),
            class,
        ));
    }

    let mut elements = if opts.cluster_nodes && !elk {
        nest_cluster_nodes(
            g,
            (cluster_els, cluster_tag),
            (edge_els, edge_tag),
            (label_els, label_tag),
            (node_els, node_tag),
        )
    } else {
        vec![
            Element::group("clusters", cluster_els),
            Element::group("edgePaths", edge_els),
            Element::group("edgeLabels", label_els),
            Element::group("nodes", node_els),
        ]
    };
    elements.extend(title_els);
    let view_box = bounds
        .rect()
        .pad(opts.view_padding.unwrap_or(cfg.diagram_padding));
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

/// mermaid's inline `stroke-dasharray` of a neo edge `len` long whose first
/// and last `hidden` px are left undrawn (`markerOffsets2`): `0 s len-s-e e`,
/// or for a dotted or dashed one 2px dashes and gaps between
/// (`generateDashArray`: `0 s 2 2 … 2 2 e`, the pattern repeating after). A
/// solid one hiding nothing draws as it would without; one too short for its
/// ends has a negative length, which makes the browser ignore the pattern.
pub(crate) fn neo_dash(len: f64, (s, e): (f64, f64), patterned: bool) -> Option<Vec<f64>> {
    let middle = len - s - e;
    if patterned {
        let pairs = if middle.is_finite() {
            (middle / 4.0).floor().max(0.0) as usize
        } else {
            0
        };
        let mut out = vec![0.0, s];
        for _ in 0..pairs {
            out.extend(NEO_DASH);
        }
        out.push(e);
        Some(out)
    } else if (s > 0.0 || e > 0.0) && middle >= 0.0 {
        Some(vec![0.0, s, middle, e])
    } else {
        None
    }
}

/// After a neo edge's path is rewritten with line hops (now `len` long),
/// mermaid recomputes its inline pattern (`applyLineJumpsToSvg`): of its
/// first four numbers `0 a b c` it keeps `a` and `c`, makes `b` the rest of
/// the length, and puts that in place of every pattern the edge has.
fn redash_hopped(group: &mut Element, inline: &[f64], len: f64) {
    if let Element::Group { children, .. } = group
        && let Some(Element::Shape { paint, .. }) = children.first_mut()
        && let Some(d) = hopped_dash(inline, len)
    {
        paint.dash = Some(d);
    }
}

/// The pattern `redash_hopped` gives an edge whose inline pattern was
/// `inline`, now `len` long; none for a pattern of fewer than four numbers.
pub(crate) fn hopped_dash(inline: &[f64], len: f64) -> Option<Vec<f64>> {
    match *inline {
        [0.0, a, _, c, ..] => Some(vec![0.0, a, (len - a - c).max(0.0), c]),
        _ => None,
    }
}

/// Replace the path of an `edgePath` group (its first child) with `cmds`.
pub(crate) fn set_edge_path(group: &mut Element, cmds: Vec<PathCmd>) {
    if let Element::Group { children, .. } = group
        && let Some(Element::Shape { geom, .. }) = children.first_mut()
    {
        *geom = Geometry::Path(cmds);
    }
}

/// mermaid's `DEFAULT_EDGE_CORNER_RADIUS` (`rounded` edges).
pub(crate) const EDGE_CORNER_RADIUS: f64 = 5.0;

/// The node and layer spacing of the root graph under ELK. mermaid's adapter never
/// reads the diagram's `nodeSpacing` / `rankSpacing`; it sets `spacing.baseValue: 40`,
/// from which ELK derives `spacing.nodeNode` and `nodeNodeBetweenLayers` as 1× (and
/// the edge spacings as ½×: the option default over `nodeNode`'s default of 20).
pub(crate) const ELK_BASE_SPACING: f64 = 40.0;

/// The root graph's node / rank separation: ELK's, or the configured one under dagre.
pub(crate) fn layer_spacing(layout: LayoutAlgorithm, configured: f64) -> f64 {
    match layout {
        LayoutAlgorithm::Elk => ELK_BASE_SPACING,
        LayoutAlgorithm::Dagre => configured,
    }
}

/// The layered layout's edge routing for a `layout` setting.
pub(crate) fn edge_routing(layout: LayoutAlgorithm) -> EdgeRouting {
    match layout {
        LayoutAlgorithm::Elk => EdgeRouting::Orthogonal(OrthoSpacing::ROOT),
        LayoutAlgorithm::Dagre => EdgeRouting::Splines,
    }
}

/// mermaid's `cutPathAtIntersect`: `points` up to where they first enter the
/// `width` × `height` box centred on `c`, ending at that crossing (by
/// `intersection`).
fn cut_path_at_intersect(points: &[Point], (c, width, height): (Point, f64, f64)) -> Vec<Point> {
    let (w, h) = (width / 2.0, height / 2.0);
    // On the border is outside (give or take the last bits, which our text
    // widths do not reproduce anyway).
    const ULP: f64 = 1e-9;
    let outside = |p: Point| (p.x - c.x).abs() >= w - ULP || (p.y - c.y).abs() >= h - ULP;
    let mut out: Vec<Point> = Vec::new();
    let Some(&first) = points.first() else {
        return out;
    };
    let mut last_outside = first;
    let mut inside = false;
    for &p in points {
        if !outside(p) && !inside {
            let at = rect_intersection(c, w, h, last_outside, p);
            if !out.iter().any(|q| q.x == at.x && q.y == at.y) {
                out.push(at);
            }
            inside = true;
        } else {
            last_outside = p;
            if !inside {
                out.push(p);
            }
        }
    }
    out
}

/// mermaid's `intersection(node, outsidePoint, insidePoint)` for the box of
/// half-size `w` × `h` centred on `c`.
fn rect_intersection(c: Point, w: f64, h: f64, outside: Point, inside: Point) -> Point {
    // An outside point on the border gives an offset of exactly 0 (and then
    // the outside point itself); rounding noise in the last bits counts as 0.
    let snap = |v: f64| if v.abs() < 1e-9 { 0.0 } else { v };
    let q_full = (outside.y - inside.y).abs();
    let r_full = (outside.x - inside.x).abs();
    if (c.y - outside.y).abs() * w > (c.x - outside.x).abs() * h {
        let q = snap(if inside.y < outside.y {
            outside.y - h - c.y
        } else {
            c.y - h - outside.y
        });
        let r = r_full * q / q_full;
        let mut res = Point::new(
            if inside.x < outside.x {
                inside.x + r
            } else {
                inside.x - r_full + r
            },
            if inside.y < outside.y {
                inside.y + q_full - q
            } else {
                inside.y - q_full + q
            },
        );
        if r == 0.0 {
            res = outside;
        }
        if r_full == 0.0 {
            res.x = outside.x;
        }
        if q_full == 0.0 {
            res.y = outside.y;
        }
        res
    } else {
        let r = snap(if inside.x < outside.x {
            outside.x - w - c.x
        } else {
            c.x - w - outside.x
        });
        let q = q_full * r / r_full;
        let mut res = Point::new(
            if inside.x < outside.x {
                inside.x + r_full - r
            } else {
                inside.x - r_full + r
            },
            if inside.y < outside.y {
                inside.y + q
            } else {
                inside.y - q
            },
        );
        if r == 0.0 {
            res = outside;
        }
        if r_full == 0.0 {
            res.x = outside.x;
        }
        if q_full == 0.0 {
            res.y = outside.y;
        }
        res
    }
}

/// `utils.calcLabelPosition`: the point half way along `points`.
fn calc_label_position(points: &[Point]) -> Point {
    if points.len() == 1 {
        return points[0];
    }
    let total: f64 = points.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut remaining = total / 2.0;
    let round5 = |v: f64| (v * 1e5).round() / 1e5;
    for w in points.windows(2) {
        let (prev, p) = (w[0], w[1]);
        let d = p.dist(prev);
        if d == 0.0 {
            return prev;
        }
        if d < remaining {
            remaining -= d;
        } else {
            let r = remaining / d;
            if r <= 0.0 {
                return prev;
            }
            if r >= 1.0 {
                return p;
            }
            return Point::new(
                round5((1.0 - r) * prev.x + r * p.x),
                round5((1.0 - r) * prev.y + r * p.y),
            );
        }
    }
    points.last().copied().unwrap_or_default()
}

/// `getLineFunctionsWithOffset`: the points d3 draws an edge through. Its
/// ends are pulled back along the line by their markers' `markerOffsets`
/// (`start` and `end`, 0 for none), and any point within a marker's height
/// of an end, on both axes, is pushed away from it (a self-loop's ends).
/// An end is `Some` when its arrow type is one of `markerOffsets`' keys: such
/// an end on a zero-length segment gets mermaid's NaN angle, and d3 then
/// writes `NaN` coordinates the browser does not draw.
fn line_with_offset(data: &[Point], start: Option<f64>, end: Option<f64>) -> Vec<Point> {
    let last = data.len() - 1;
    let (first, final_) = (data[0], data[last]);
    let (start_key, end_key) = (start.is_some(), end.is_some());
    let (start, end) = (start.unwrap_or(0.0), end.unwrap_or(0.0));
    // `calculateDeltaAndAngle` from `a` to `b`: the offsets along the line.
    let along = |a: Point, b: Point, h: f64| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let angle = (dy / dx).atan();
        (
            h * angle.cos() * if dx >= 0.0 { 1.0 } else { -1.0 },
            h * angle.sin().abs() * if dy >= 0.0 { 1.0 } else { -1.0 },
        )
    };
    let flip_x = if first.x < final_.x { 1.0 } else { -1.0 };
    let flip_y = if first.y < final_.y { 1.0 } else { -1.0 };
    data.iter()
        .enumerate()
        .map(|(i, &d)| {
            let (mut ox, mut oy) = if i == 0 && start_key {
                along(data[0], data[1], start)
            } else if i == last && end_key {
                along(data[last], data[last - 1], end)
            } else {
                (0.0, 0.0)
            };
            let near = |a: f64, b: f64, h: f64| a < h && a > 0.0 && b < h;
            let (ex, ey) = ((d.x - final_.x).abs(), (d.y - final_.y).abs());
            let (sx, sy) = ((d.x - first.x).abs(), (d.y - first.y).abs());
            if near(ex, ey, end) {
                ox -= (end + 1.0 - ex) * flip_x;
            }
            if near(sx, sy, start) {
                ox += (start + 1.0 - sx) * flip_x;
            }
            if near(ey, ex, end) {
                oy -= (end + 1.0 - ey) * flip_y;
            }
            if near(sy, sx, start) {
                oy += (start + 1.0 - sy) * flip_y;
            }
            Point::new(d.x + ox, d.y + oy)
        })
        .collect()
}

/// The `terminalMarkerSize` mermaid's dagre renderer passes for an end with a marker.
const TERMINAL_MARKER_SIZE: f64 = 10.0;

/// `utils.calcTerminalLabelPosition` for a start label on the right
/// (`startLabelRight`) or an end label on the left (`endLabelLeft`): half way
/// along the route's first (last) `25 + marker` px, `10 + marker / 2` to the
/// side.
fn calc_terminal_label_position(marker: f64, end: bool, path: &[Point]) -> Point {
    let mut points = path.to_vec();
    if end {
        points.reverse();
    }
    let center = calculate_point(&points, 25.0 + marker);
    let d = 10.0 + marker * 0.5;
    let p0 = points[0];
    let angle = (p0.y - center.y).atan2(p0.x - center.x);
    Point::new(
        angle.sin() * d + (p0.x + center.x) / 2.0,
        -angle.cos() * d + (p0.y + center.y) / 2.0,
    )
}

/// `utils.calculatePoint`: the point `distance` along `points`.
fn calculate_point(points: &[Point], distance: f64) -> Point {
    let round5 = |v: f64| (v * 1e5).round() / 1e5;
    let mut remaining = distance;
    for w in points.windows(2) {
        let (prev, p) = (w[0], w[1]);
        let d = p.dist(prev);
        if d == 0.0 {
            return prev;
        }
        if d < remaining {
            remaining -= d;
        } else {
            let r = remaining / d;
            if r <= 0.0 {
                return prev;
            }
            if r >= 1.0 {
                return p;
            }
            return Point::new(
                round5((1.0 - r) * prev.x + r * p.x),
                round5((1.0 - r) * prev.y + r * p.y),
            );
        }
    }
    points.last().copied().unwrap_or_default()
}

/// `utils.isLabelCoordinateInPath`: whether the rounded `x` or `y` of `p`
/// occurs anywhere in the path data (its decimals rounded too), as text.
fn label_coordinate_in_path(p: Point, cmds: &[geom::PathCmd], origin: Point) -> bool {
    // JS `Math.round` (half up) and number printing.
    let js_round = |v: f64| ((v + 0.5).floor() as i64).to_string();
    // Both in the coordinates of the group the path is drawn in.
    let local = |q: Point| q.minus(origin);
    let p = local(p);
    // The `d` d3 writes, each `\d+\.\d+` replaced by its rounded value.
    let n = |v: f64| {
        if v.fract() == 0.0 {
            format!("{}", v as i64)
        } else if v < 0.0 {
            format!("-{}", js_round(-v))
        } else {
            js_round(v)
        }
    };
    let mut d = String::new();
    for c in cmds {
        match *c {
            geom::PathCmd::MoveTo(a) => {
                let a = local(a);
                d += &format!("M{},{}", n(a.x), n(a.y))
            }
            geom::PathCmd::LineTo(a) => {
                let a = local(a);
                d += &format!("L{},{}", n(a.x), n(a.y))
            }
            geom::PathCmd::CubicTo(a, b, e) => {
                let (a, b, e) = (local(a), local(b), local(e));
                d += &format!(
                    "C{},{},{},{},{},{}",
                    n(a.x),
                    n(a.y),
                    n(b.x),
                    n(b.y),
                    n(e.x),
                    n(e.y)
                )
            }
            geom::PathCmd::Close => d += "Z",
        }
    }
    d.contains(&js_round(p.x)) || d.contains(&js_round(p.y))
}

/// The order subgraphs close in (flowDb pushes a subgraph at its `end`): a
/// post-order walk of the subgraph forest, siblings in declaration order.
pub(crate) fn closing_order(clusters: &[Cluster]) -> Vec<usize> {
    fn post(clusters: &[Cluster], parent: Option<ClusterId>, out: &mut Vec<usize>, depth: usize) {
        if depth > clusters.len() {
            return;
        }
        for (ci, c) in clusters.iter().enumerate() {
            if c.parent == parent && Some(ClusterId(ci)) != parent {
                post(clusters, Some(ClusterId(ci)), out, depth + 1);
                out.push(ci);
            }
        }
    }
    let mut out = Vec::new();
    post(clusters, None, &mut out, 0);
    out
}

/// The ids and node order mermaid's dagre renderer keys its graph by: flowDb's
/// `getData` pushes the subgraphs first, last closed first (an empty one as a
/// plain node), then the vertices in the order they were first seen.
fn dagre_model(
    g: &GraphDiagram,
    nodes: &[Node],
    empty_of: &[Option<usize>],
    pos_of: &[usize],
) -> crate::layout::dagre::DagreModel {
    use crate::layout::dagre::{DagreItem, DagreModel};
    let mut node_names = vec![String::new(); nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        node_names[pos_of[i]] = n.id.clone();
    }
    for (ci, p) in empty_of.iter().enumerate() {
        if let Some(p) = *p {
            node_names[pos_of[p]] = g.clusters[ci].id.clone();
        }
    }
    // classDb pads a namespace by `class.padding ?? 16`.
    let cluster_pad = if g.kind == DiagramKind::Class {
        16.0
    } else {
        8.0
    };
    let cluster_pads = vec![cluster_pad; g.clusters.len()];
    let mut turns = vec![false; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        turns[pos_of[i]] = n.shape == Shape::ForkBar;
    }
    // A diagram that says where each node and cluster goes (state diagrams).
    if g.nodes.iter().any(|n| n.model_seq.is_some()) {
        let mut items: Vec<(usize, DagreItem)> = g
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.model_seq.unwrap_or(usize::MAX),
                    DagreItem::Node(pos_of[i]),
                )
            })
            .chain(g.clusters.iter().enumerate().map(|(ci, c)| {
                let item = match empty_of[ci] {
                    Some(p) => DagreItem::Node(pos_of[p]),
                    None => DagreItem::Cluster(ci),
                };
                (c.model_seq.unwrap_or(usize::MAX), item)
            }))
            .collect();
        items.sort_by_key(|&(k, _)| k);
        return DagreModel {
            node_names,
            cluster_names: g.clusters.iter().map(|c| c.id.clone()).collect(),
            order: items.into_iter().map(|(_, item)| item).collect(),
            cluster_td: g.clusters.iter().map(|c| c.direction_td).collect(),
            turns,
            cluster_pads,
        };
    }
    // Collapsed subgraphs take their place among the expanded ones.
    let mut folded: Vec<(usize, usize)> = g
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.group_seq.map(|k| (k, i)))
        .collect();
    folded.sort_by_key(|&(k, _)| k);
    let mut order: Vec<DagreItem> = Vec::new();
    let mut next = folded.iter().peekable();
    for (j, ci) in closing_order(&g.clusters).into_iter().rev().enumerate() {
        while let Some(&&(_, i)) = next.peek().filter(|f| f.0 <= j) {
            order.push(DagreItem::Node(pos_of[i]));
            next.next();
        }
        order.push(match empty_of[ci] {
            Some(p) => DagreItem::Node(pos_of[p]),
            None => DagreItem::Cluster(ci),
        });
    }
    order.extend(next.map(|&(_, i)| DagreItem::Node(pos_of[i])));
    order.extend(
        (0..g.nodes.len())
            .filter(|&i| g.nodes[i].group_seq.is_none())
            .map(|i| DagreItem::Node(pos_of[i])),
    );
    DagreModel {
        node_names,
        cluster_names: g.clusters.iter().map(|c| c.id.clone()).collect(),
        order,
        cluster_td: g.clusters.iter().map(|c| c.direction_td).collect(),
        turns,
        cluster_pads,
    }
}

/// The clipped first point of `pts` (which starts at node `b`). A dagre route
/// starts at the node's centre and is cut where the line toward the next point
/// leaves the outline; an orthogonal (ELK) route starts at a port on the node's
/// bounding box and is moved in along its first segment onto the outline.
/// `.edgeTerminals { font-size: 11px }`: the size of a terminal label.
const TERMINAL_FONT_SIZE: f64 = 11.0;
/// `TERMINAL_LABEL_MARKER_CLEARANCE`: half the width of the relation markers,
/// by which mermaid pads a terminal label for ELK past its end's marker.
const TERMINAL_MARKER_CLEARANCE: f64 = 6.0;

fn terminal_block(text: &Option<String>) -> TextBlock {
    TextBlock::label(text.as_deref().unwrap_or(""), TERMINAL_FONT_SIZE)
}

fn terminal_pad(head: Head) -> f64 {
    if head == Head::None {
        0.0
    } else {
        TERMINAL_MARKER_CLEARANCE
    }
}

/// The sizes ELK reserves for an edge's terminal labels (`terminalElkLabels`).
fn terminal_sizes(e: &Edge) -> [Option<(f64, f64)>; 2] {
    let size = |text: &Option<String>, head: Head| {
        let b = terminal_block(text);
        let pad = 2.0 * terminal_pad(head);
        (!b.is_empty()).then_some((b.width + pad, b.height + pad))
    };
    [size(&e.start_label, e.start), size(&e.end_label, e.end)]
}

/// Lays `input` out without its empty clusters (`empty_of`: their
/// placeholders), which have no members and so nothing refers to: ELK gets an
/// empty subgraph as a plain node (the placeholder), not as a container. The
/// output has an (empty) rectangle and the root's direction for each of them.
fn layout_without_empty(input: &LayoutInput, empty_of: &[Option<usize>]) -> layout::LayoutOutput {
    if empty_of.iter().all(Option::is_none) {
        return layout::layout(input);
    }
    let mut index = vec![None; input.clusters.len()];
    let mut kept = Vec::new();
    for (ci, slot) in index.iter_mut().enumerate() {
        if empty_of.get(ci).copied().flatten().is_none() {
            *slot = Some(kept.len());
            kept.push(ci);
        }
    }
    let re = |c: Option<usize>| c.and_then(|c| index[c]);
    let mut reduced = input.clone();
    reduced.clusters = kept
        .iter()
        .map(|&ci| {
            let c = &input.clusters[ci];
            LayoutCluster {
                parent: re(c.parent),
                ..c.clone()
            }
        })
        .collect();
    for n in &mut reduced.nodes {
        n.cluster = re(n.cluster);
    }
    for e in &mut reduced.edges {
        e.from_cluster = re(e.from_cluster);
        e.to_cluster = re(e.to_cluster);
    }
    let out = layout::layout(&reduced);
    let mut clusters = vec![Rect::default(); input.clusters.len()];
    let mut directions = vec![input.direction; input.clusters.len()];
    for (k, &ci) in kept.iter().enumerate() {
        clusters[ci] = out.clusters[k];
        if let Some(&d) = out.cluster_directions.get(k) {
            directions[ci] = d;
        }
    }
    layout::LayoutOutput {
        clusters,
        cluster_directions: directions,
        ..out
    }
}

/// mermaid's `alignDegenerateNodeToAnchor` (ELK): ELK keeps a port 10 from
/// either end of its node's side, so on a side shorter than twice
/// `PORTS_SURROUNDING_MARGIN` (a 14px start state's) the edge leaves off the
/// node's middle, and the node, laid out around that port, stands off the
/// edge's line. mermaid moves such a node along the side until its centre is
/// on the anchor of its first edge (in edge order, an edge's start before its
/// end), leaving the edges as routed. `anchors` are the edges' ends at their
/// nodes, in that order.
fn align_degenerate_nodes(centers: &mut [Point], sizes: &[(f64, f64)], anchors: &[(usize, Point)]) {
    const PORTS_SURROUNDING_MARGIN: f64 = 12.0;
    let mut aligned = vec![false; centers.len()];
    for &(i, anchor) in anchors {
        let ((w, h), c) = (sizes[i], centers[i]);
        let (top, bottom) = (c.y - h / 2.0, c.y + h / 2.0);
        let along_width = (anchor.y - top).abs() <= 0.5 || (anchor.y - bottom).abs() <= 0.5;
        if (if along_width { w } else { h }) >= 2.0 * PORTS_SURROUNDING_MARGIN || aligned[i] {
            continue;
        }
        aligned[i] = true;
        if along_width {
            centers[i].x = anchor.x;
        } else {
            centers[i].y = anchor.y;
        }
    }
}

fn clip_end(b: &Boundary, pts: &[Point], elk: bool) -> Point {
    if elk {
        clip_along(b, pts[0], pts[1])
    } else {
        b.clip(pts[1])
    }
}

/// [`clip_end`] at an edge's target (`pts` reversed: from the target's port
/// back). Under ELK mermaid's `cutter2` puts the outline point after the port
/// and then drops a last segment shorter than 2px, so an outline point that
/// close to the port leaves the edge ending at the port, on the bounding box.
pub(crate) fn clip_target_end(b: &Boundary, pts: &[Point], elk: bool) -> Point {
    let p = clip_end(b, pts, elk);
    if elk && p.minus(pts[0]).len() < ELK_MIN_LAST_SEGMENT {
        pts[0]
    } else {
        p
    }
}

/// `cutter2`'s shortest last segment it keeps (the tail point goes otherwise).
const ELK_MIN_LAST_SEGMENT: f64 = 2.0;

/// `MIN_END_MARKER_SEGMENT_LENGTH` of mermaid's ELK adapter.
const ELK_MIN_MARKER_SEGMENT: f64 = 8.0;

/// Whether mermaid's clipped ELK route keeps an end's port beside the point
/// where it meets the node's outline (`clip`), and so holds both. `cutter2`
/// replaces the node's centre by the outline point and leaves the port; a
/// port that coincides with it (to 1e-6) goes as a duplicate, at the target
/// (`at_end`) the outline point goes for one less than 2px from it (see
/// [`clip_target_end`]), and one on the bounding box that leaves too short a
/// segment for the end's marker (`offset`) goes too
/// (`ensureStart/EndMarkerSegmentLength`). `clip`: `None` for an outline
/// point that differs from the port only by rounding.
fn kept_port(
    port: Point,
    clip: Option<Point>,
    offset: f64,
    bounds: Rect,
    at_end: bool,
) -> Option<Point> {
    let clip = match clip {
        Some(c) => {
            let d = port.minus(c);
            if d.x.abs() <= 1e-6 && d.y.abs() <= 1e-6
                || at_end && port.dist(c) < ELK_MIN_LAST_SEGMENT
            {
                return None;
            }
            c
        }
        None => port,
    };
    let tol = 1.0;
    let on_border = (port.x - bounds.x).abs() <= tol
        || (port.x - bounds.right()).abs() <= tol
        || (port.y - bounds.y).abs() <= tol
        || (port.y - bounds.bottom()).abs() <= tol;
    let short = port.dist(clip) < ELK_MIN_MARKER_SEGMENT.max(2.0 * offset);
    (!(offset > 0.0 && short && on_border)).then_some(port)
}

/// Puts the ports [`kept_port`] keeps into the route where the outline point
/// beside one is off the line of its leg (an end cut on the line from the
/// node's centre): mermaid draws the edge through it, its last leg slanted.
fn keep_ports_off_the_line(pts: &mut Vec<Point>, kept: &mut [Option<Point>; 2]) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    if let Some(p) = kept[1]
        && p.dist(pts[n - 1]) > 1e-3
        && !collinear(pts[n - 2], p, pts[n - 1])
    {
        pts.insert(n - 1, p);
        kept[1] = None;
    }
    if let Some(p) = kept[0]
        && p.dist(pts[0]) > 1e-3
        && !collinear(pts[0], p, pts[1])
    {
        pts.insert(1, p);
        kept[0] = None;
    }
}

/// Whether `b` lies on the line through `a` and `c` (to rounding).
fn collinear(a: Point, b: Point, c: Point) -> bool {
    let (u, v) = (b.minus(a), c.minus(b));
    (u.x * v.y - u.y * v.x).abs() <= 1e-9 * (u.len() * v.len()).max(1.0)
}

/// A straightened route without the ports [`kept_port`] put in, where they
/// are still where they were.
fn without_kept_ports(mut pts: Vec<Point>, [start, end]: [Option<Point>; 2]) -> Vec<Point> {
    if let Some(p) = end
        && pts.len() > 2
        && pts[pts.len() - 2] == p
    {
        pts.remove(pts.len() - 2);
    }
    if let Some(p) = start
        && pts.len() > 2
        && pts[1] == p
    {
        pts.remove(1);
    }
    pts
}

/// Where the line from `next` through `port` (a point on the node's bounding box)
/// meets the node's outline, coming from outside: `port` itself for a rectangle,
/// further in for a diamond, circle, ... (as the ELK adapter's `cutter2` clips the
/// first segment). `port` if the line misses the outline.
pub(crate) fn clip_along(b: &Boundary, port: Point, next: Point) -> Point {
    let center = match b {
        Boundary::Rect(r) | Boundary::Cylinder(r, _) => r.center(),
        Boundary::Ellipse(c, _, _) | Boundary::Polygon(c, _) | Boundary::Outline(c, _) => *c,
    };
    let inward = port.minus(next);
    if inward.len() < 1e-9 {
        return port;
    }
    let d = inward.norm();
    let to_center = center.minus(port);
    let depth = to_center.x * d.x + to_center.y * d.y;
    if depth <= 0.0 {
        return port;
    }
    // The point level with the centre on the line, and the way out from it.
    let inner = port.plus(d.scale(depth));
    let out = d.scale(-1.0);
    if let Boundary::Cylinder(..) = b {
        // `outlineAttachPoint` bisects (20 steps) between the two, a point
        // being inside when it is no further from the centre than where the
        // shape's `intersect` puts the ray through it; the cylinder's, the
        // box's crossing moved by the cap, is no outline of its own.
        let inside = |p: Point| {
            let from_centre = p.minus(center).len();
            from_centre < 1e-12 || from_centre <= b.clip(p).minus(center).len() + 1e-9
        };
        if !inside(inner) {
            return port;
        }
        if inside(port) {
            return port;
        }
        let (mut lo, mut hi) = (0.0, depth);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if inside(inner.plus(out.scale(mid))) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        return inner.plus(out.scale(lo));
    }
    let t = match b {
        Boundary::Rect(r) | Boundary::Cylinder(r, _) => {
            let exit = |p: f64, v: f64, lo: f64, hi: f64| {
                if v > 1e-12 {
                    (hi - p) / v
                } else if v < -1e-12 {
                    (lo - p) / v
                } else {
                    f64::INFINITY
                }
            };
            exit(inner.x, out.x, r.x, r.right()).min(exit(inner.y, out.y, r.y, r.bottom()))
        }
        Boundary::Ellipse(c, rx, ry) => {
            // |(inner + t * out - c) / r|^2 = 1, the positive root.
            let (px, py) = ((inner.x - c.x) / rx, (inner.y - c.y) / ry);
            let (vx, vy) = (out.x / rx, out.y / ry);
            let qa = vx * vx + vy * vy;
            let qb = 2.0 * (px * vx + py * vy);
            let qc = px * px + py * py - 1.0;
            let disc = qb * qb - 4.0 * qa * qc;
            if qa < 1e-12 || disc < 0.0 {
                return port;
            }
            (-qb + disc.sqrt()) / (2.0 * qa)
        }
        Boundary::Polygon(_, poly) => {
            let hit = geom::clip_polygon(inner, poly, inner.plus(out));
            hit.minus(inner).len()
        }
        // The crossing nearest the port, as `intersect.polygon` picks it.
        Boundary::Outline(_, pts) => {
            let hit = Boundary::Outline(inner, pts.clone()).clip(port);
            hit.minus(inner).len()
        }
    };
    if !t.is_finite() || t <= 1e-9 || t > depth + 1e-6 {
        return port;
    }
    inner.plus(out.scale(t))
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
        cfg.min_node_width,
        cfg.wrapping_width,
        !cfg.html_labels,
        1.0,
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
        cfg.min_node_width,
        cfg.wrapping_width,
        !cfg.html_labels,
        1.0,
    );
    let slot = slot_colors(n.color_slot, pal);
    let painted = Node {
        style: slot_styled(&n.style, slot.as_ref()),
        ..n.clone()
    };
    let (mut els, boundary) = node_elements(&painted, &m, c, pal, &lk, font, kind);
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
pub(crate) const CLUSTER_PADDING: f64 = 24.0;
/// Node gap inside a cluster laid out on its own (`DEFAULT_SUBGRAPH_NODE_SPACING`).
pub(crate) const CLUSTER_NODE_SPACING: f64 = 50.0;
/// Layer gap inside a cluster laid out on its own (`DEFAULT_SUBGRAPH_SPACING_BASE_VALUE`).
pub(crate) const CLUSTER_RANK_SPACING: f64 = 24.0;
/// Horizontal padding counted into a cluster's title width (the group node's `padding: 8`).
const CLUSTER_TITLE_PAD: f64 = 8.0;

/// ER groups render their title with `createText` at the group's laid-out width
/// (`node.width`), so a title wider than the group's content wraps onto more lines
/// and the frame is that width plus the label padding. A probe layout without the
/// titles' minimum widths gives each group's content width.
fn wrap_er_cluster_titles(
    g: &GraphDiagram,
    input: &mut LayoutInput,
    titles: &mut [TextBlock],
    empty_of: &[Option<usize>],
    font: f64,
    title_pad: f64,
) {
    let mut probe = input.clone();
    probe.clusters.iter_mut().for_each(|c| c.min_width = 0.0);
    let widths: Vec<f64> = layout::layout(&probe)
        .clusters
        .iter()
        .map(|r| r.w)
        .collect();
    for (ci, c) in g.clusters.iter().enumerate() {
        let w = widths.get(ci).copied().unwrap_or(0.0);
        if empty_of[ci].is_some() || titles[ci].is_empty() || w <= 0.0 || titles[ci].width <= w {
            continue;
        }
        let lines = crate::text::wrap(&c.title, w, font, false, crate::text::BreakRule::Browser);
        let mut t = TextBlock::new(lines, font, false);
        // The wrapped `div` is given exactly the wrapping width.
        t.width = t.width.max(w);
        input.clusters[ci].title_height = cluster_title_height(c.kind, &t, false);
        input.clusters[ci].min_width = t.width + title_pad;
        titles[ci] = t;
    }
}

/// An ER group title as `createText` lays it out at `width`: one line when it
/// fits, else wrapped in a `div` exactly that wide (or as wide as its widest word).
fn wrapped_group_title(title: &str, width: f64, font: f64) -> TextBlock {
    let t = TextBlock::label(title, font);
    if width <= 0.0 || t.width <= width {
        return t;
    }
    let lines = crate::text::wrap(title, width, font, false, crate::text::BreakRule::Browser);
    let mut t = TextBlock::new(lines, font, false);
    t.width = t.width.max(width);
    t
}

/// Height of the band reserved above a cluster's children for its title: ELK keeps
/// the label plus its label spacing clear (21px of text → 24), a state
/// composite's as any other's; under dagre a composite's body starts 2px under
/// its label (`roundedWithTitle`); regions have no title.
fn cluster_title_height(kind: ClusterKind, t: &TextBlock, elk: bool) -> f64 {
    if t.is_empty() {
        return 0.0;
    }
    match kind {
        ClusterKind::Group => t.height + 3.0,
        ClusterKind::Composite | ClusterKind::NoteGroup if elk => t.height + 3.0,
        ClusterKind::Composite => t.height + 2.0,
        ClusterKind::Region | ClusterKind::NoteGroup => 0.0,
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
        // `noteGroup`: an unfilled, unstroked rect.
        ClusterKind::NoteGroup => Vec::new(),
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
/// erDb's `getData` does the same with its groups (kept here in closing order).
/// Other diagrams keep declaration order.
fn model_order(g: &GraphDiagram, nodes: &[Node], empty_of: &[Option<usize>]) -> Vec<usize> {
    if !matches!(g.kind, DiagramKind::Flowchart | DiagramKind::Er) || g.clusters.is_empty() {
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
/// An orthogonal (ELK) route is cut exactly where it crosses the border, keeping its
/// right angles; a dagre route is moved to leave straight across from the next point.
fn cut_at_rect(pts: &[Point], r: &Rect, elk: bool) -> Vec<Point> {
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
    let b = if elk {
        b
    } else if gap_y > 0.0 && gap_y >= gap_x {
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

/// Drawn elements, each with the index of the cluster, edge or node it draws.
type Tagged = (Vec<Element>, Vec<usize>);

/// mermaid's dagre wrapper lays out a cluster no edge crosses (none joins a
/// node or cluster inside it to one outside; edges to the cluster itself do
/// not count) as a graph of its own (`adjustClustersAndEdges` / `extractor`),
/// and draws it as one node of its parent: its frame and contents in the
/// parent's `nodes`, over the parent's edges. Edges to it are cut at its
/// frame, so the part of an end marker that reaches inside is hidden. The
/// layers here follow that: per such graph its other clusters, edges (those
/// with both ends inside it), edge labels, and nodes with its cluster nodes.
fn nest_cluster_nodes(
    g: &GraphDiagram,
    clusters: Tagged,
    edges: Tagged,
    labels: Tagged,
    nodes: Tagged,
) -> Vec<Element> {
    let nc = g.clusters.len();
    let chain = |mut c: Option<ClusterId>| {
        let mut out = Vec::new();
        while let Some(ClusterId(i)) = c {
            if out.len() > nc || out.contains(&i) {
                break;
            }
            out.push(i);
            c = g.clusters[i].parent;
        }
        out
    };
    // Per endpoint: the clusters it lies in (a cluster counts as inside itself
    // for where its edges go, not for crossing).
    let within = |e: Endpoint| match e {
        Endpoint::Node(NodeId(i)) => (chain(g.nodes[i].cluster), None),
        Endpoint::Cluster(ClusterId(c)) => (chain(g.clusters[c].parent), Some(c)),
    };
    let mut own = vec![false; nc];
    for n in &g.nodes {
        for c in chain(n.cluster) {
            own[c] = true;
        }
    }
    let mut crossed = vec![false; nc];
    for e in &g.edges {
        let ((a, _), (b, _)) = (within(e.from), within(e.to));
        for (c, x) in crossed.iter_mut().enumerate() {
            *x |= a.contains(&c) != b.contains(&c);
        }
    }
    let node_cluster: Vec<bool> = (0..nc).map(|c| own[c] && !crossed[c]).collect();
    // The graph each item is drawn in: its nearest cluster node (`nc`: the root).
    let level_of = |inside: &[usize]| {
        inside
            .iter()
            .copied()
            .find(|&c| node_cluster[c])
            .unwrap_or(nc)
    };
    let cluster_level: Vec<usize> = (0..nc)
        .map(|c| level_of(&chain(g.clusters[c].parent)))
        .collect();
    let edge_level: Vec<usize> = g
        .edges
        .iter()
        .map(|e| {
            let side = |x: Endpoint| {
                let (mut cs, me) = within(x);
                if let Some(c) = me {
                    cs.insert(0, c);
                }
                cs
            };
            let (a, b) = (side(e.from), side(e.to));
            a.iter()
                .copied()
                .find(|c| node_cluster[*c] && b.contains(c))
                .unwrap_or(nc)
        })
        .collect();
    let node_level: Vec<usize> = g
        .nodes
        .iter()
        .map(|n| level_of(&chain(n.cluster)))
        .collect();
    let mut by_level: Vec<[Vec<Element>; 4]> = (0..=nc).map(|_| Default::default()).collect();
    let mut frames: Vec<Option<Element>> = (0..nc).map(|_| None).collect();
    for (el, c) in clusters.0.into_iter().zip(clusters.1) {
        if node_cluster[c] {
            frames[c] = Some(el);
        } else {
            by_level[cluster_level[c]][0].push(el);
        }
    }
    for (el, e) in edges.0.into_iter().zip(edges.1) {
        by_level[edge_level[e]][1].push(el);
    }
    for (el, e) in labels.0.into_iter().zip(labels.1) {
        by_level[edge_level[e]][2].push(el);
    }
    // Nodes and cluster nodes in the order mermaid's graph lists them: a
    // cluster node where its first member would be.
    let mut first_member = vec![usize::MAX; nc];
    for (i, n) in g.nodes.iter().enumerate() {
        for c in chain(n.cluster) {
            first_member[c] = first_member[c].min(i);
        }
    }
    fn build(
        level: usize,
        by_level: &mut [[Vec<Element>; 4]],
        frames: &mut [Option<Element>],
        kids: &[Vec<usize>],
        first_member: &[usize],
        nodes_at: &mut [Vec<(usize, usize, Element)>],
    ) -> Vec<Element> {
        let mut layer = std::mem::take(&mut nodes_at[level]);
        for &c in &kids[level] {
            let mut inner = Vec::new();
            if let Some(f) = frames[c].take() {
                inner.push(f);
            }
            let parts = build(c, by_level, frames, kids, first_member, nodes_at);
            let mut parts = parts.into_iter();
            if let Some(Element::Group { children, .. }) = parts.next() {
                inner.extend(children);
            }
            let mut children = vec![Element::group("clusters", inner)];
            children.extend(parts);
            layer.push((first_member[c], 1, Element::group("root", children)));
        }
        layer.sort_by_key(|t| (t.0, t.1));
        let [c, e, l, _] = std::mem::take(&mut by_level[level]);
        vec![
            Element::group("clusters", c),
            Element::group("edgePaths", e),
            Element::group("edgeLabels", l),
            Element::group("nodes", layer.into_iter().map(|t| t.2).collect()),
        ]
    }
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); nc + 1];
    for c in (0..nc).filter(|&c| node_cluster[c]) {
        kids[cluster_level[c]].push(c);
    }
    let mut nodes_at: Vec<Vec<(usize, usize, Element)>> = (0..=nc).map(|_| Vec::new()).collect();
    for (el, i) in nodes.0.into_iter().zip(nodes.1) {
        nodes_at[node_level[i]].push((i, 0, el));
    }
    build(
        nc,
        &mut by_level,
        &mut frames,
        &kids,
        &first_member,
        &mut nodes_at,
    )
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
    obstacles: &[Rect],
) -> (Vec<Point>, Point) {
    if ne.inner == Endpoint::Cluster(ne.cluster) {
        let ext = 22.0;
        let (cx, cy) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
        let (top, right) = (frame.y, frame.right());
        // The outer corners are repeated so the basis curve runs out to them instead of
        // cutting back across the frame's corner.
        let (c1, c2) = (
            Point::new(right + ext, top - ext),
            Point::new(right + ext, cy),
        );
        let pts = vec![
            Point::new(cx + frame.w / 4.0, top),
            Point::new(cx + frame.w / 4.0, top - ext),
            c1,
            c1,
            c2,
            c2,
            Point::new(right, cy),
        ];
        let label = Point::new(right + ext + label_w.unwrap_or(0.0) / 2.0 + 4.0, top - ext);
        return (pts, label);
    }
    let (cx, cy) = (inner.x + inner.w / 2.0, inner.y + inner.h / 2.0);
    // The edge runs in the lane the frame's padding keeps free along one side (clear
    // of the nodes and of the title) and turns into / out of the inner end sideways,
    // so it does not cut through the nodes before / after it in the flow. If that
    // sideways run would cross another node, the lane on the other side is tried,
    // then a straight run along the flow, then a run around the inner end through
    // the lane on its far side; failing all, the route crossing the fewest nodes.
    let lane = CLUSTER_PADDING / 2.0;
    let (near_x, far_x) = (frame.x + lane, frame.right() - lane);
    let (near_y, far_y) = (frame.y + title_h + lane, frame.bottom() - lane);
    let p = Point::new;
    let candidates: [Vec<Point>; 5] = if ne.inner_is_target {
        // In from the entry side.
        match dir {
            Direction::TopBottom | Direction::BottomTop => {
                let (entry, face) = if dir == Direction::TopBottom {
                    (frame.y, inner.y)
                } else {
                    (frame.bottom(), inner.bottom())
                };
                let (back, back_face) = if dir == Direction::TopBottom {
                    (far_y, inner.bottom())
                } else {
                    (near_y, inner.y)
                };
                [
                    vec![p(near_x, entry), p(near_x, cy), p(inner.x, cy)],
                    vec![p(far_x, entry), p(far_x, cy), p(inner.right(), cy)],
                    vec![p(cx, entry), p(cx, face)],
                    vec![
                        p(near_x, entry),
                        p(near_x, back),
                        p(cx, back),
                        p(cx, back_face),
                    ],
                    vec![
                        p(far_x, entry),
                        p(far_x, back),
                        p(cx, back),
                        p(cx, back_face),
                    ],
                ]
            }
            Direction::LeftRight | Direction::RightLeft => {
                let (entry, face) = if dir == Direction::LeftRight {
                    (frame.x, inner.x)
                } else {
                    (frame.right(), inner.right())
                };
                let (back, back_face) = if dir == Direction::LeftRight {
                    (far_x, inner.right())
                } else {
                    (near_x, inner.x)
                };
                [
                    vec![p(entry, near_y), p(cx, near_y), p(cx, inner.y)],
                    vec![p(entry, far_y), p(cx, far_y), p(cx, inner.bottom())],
                    vec![p(entry, cy), p(face, cy)],
                    vec![
                        p(entry, near_y),
                        p(back, near_y),
                        p(back, cy),
                        p(back_face, cy),
                    ],
                    vec![
                        p(entry, far_y),
                        p(back, far_y),
                        p(back, cy),
                        p(back_face, cy),
                    ],
                ]
            }
        }
    } else {
        // Out to the exit side.
        match dir {
            Direction::TopBottom | Direction::BottomTop => {
                let (exit, face) = if dir == Direction::TopBottom {
                    (frame.bottom(), inner.bottom())
                } else {
                    (frame.y, inner.y)
                };
                let (back, back_face) = if dir == Direction::TopBottom {
                    (near_y, inner.y)
                } else {
                    (far_y, inner.bottom())
                };
                [
                    vec![p(inner.right(), cy), p(far_x, cy), p(far_x, exit)],
                    vec![p(inner.x, cy), p(near_x, cy), p(near_x, exit)],
                    vec![p(cx, face), p(cx, exit)],
                    vec![
                        p(cx, back_face),
                        p(cx, back),
                        p(far_x, back),
                        p(far_x, exit),
                    ],
                    vec![
                        p(cx, back_face),
                        p(cx, back),
                        p(near_x, back),
                        p(near_x, exit),
                    ],
                ]
            }
            Direction::LeftRight | Direction::RightLeft => {
                let (exit, face) = if dir == Direction::LeftRight {
                    (frame.right(), inner.right())
                } else {
                    (frame.x, inner.x)
                };
                let (back, back_face) = if dir == Direction::LeftRight {
                    (near_x, inner.x)
                } else {
                    (far_x, inner.right())
                };
                [
                    vec![p(cx, inner.bottom()), p(cx, far_y), p(exit, far_y)],
                    vec![p(cx, inner.y), p(cx, near_y), p(exit, near_y)],
                    vec![p(face, cy), p(exit, cy)],
                    vec![
                        p(back_face, cy),
                        p(back, cy),
                        p(back, far_y),
                        p(exit, far_y),
                    ],
                    vec![
                        p(back_face, cy),
                        p(back, cy),
                        p(back, near_y),
                        p(exit, near_y),
                    ],
                ]
            }
        }
    };
    // How many nodes the route crosses.
    let hits = |pts: &[Point]| {
        obstacles
            .iter()
            .filter(|o| {
                pts.windows(2).any(|w| {
                    let (a, b) = (w[0], w[1]);
                    let seg = Rect::new(
                        a.x.min(b.x),
                        a.y.min(b.y),
                        (a.x - b.x).abs(),
                        (a.y - b.y).abs(),
                    );
                    seg.intersects(&o.pad(-1.0))
                })
            })
            .count()
    };
    let pts = candidates
        .iter()
        .min_by_key(|c| hits(c))
        .cloned()
        .unwrap_or_default();
    // The label sits on the run along the frame side (in) / to the frame side (out).
    let label = if ne.inner_is_target {
        pts[0].lerp(pts[1], 0.5)
    } else {
        pts[pts.len() - 2].lerp(pts[pts.len() - 1], 0.5)
    };
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
/// slack) and `cell_pad` is `er.diagramPadding` (PADDING, per-column slack). A
/// table scales both by `scale` (1.25 without root `htmlLabels`); the plain box is
/// drawn before that scaling.
fn table_metrics(node: &Node, text_pad: f64, cell_pad: f64, font: f64, scale: f64) -> TableMetrics {
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
            w: (label.width.max(node.wrapped_width.unwrap_or(0.0)) + 2.0 * cell_pad).max(100.0),
            h: label.height.max(line_h) + 3.0 * cell_pad,
        };
    }
    let (text_pad, cell_pad) = (text_pad * scale, cell_pad * scale);
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
    let name = TextBlock::new(crate::text::label_lines(&node.label), font, true);
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
    kind: DiagramKind,
) -> (Vec<Element>, Boundary) {
    let (w, h) = (m.w, m.h);
    // Flowchart CSS (`.node circle, .node path { fill: mainBkg; stroke: nodeBorder;
    // stroke-width: 2px }`) overrides the state shapes' own attributes; `f-circ` keeps
    // its inline `fill: nodeBorder !important`.
    let flow_css = kind == DiagramKind::Flowchart;
    let r = Rect::centered(c, w, h);
    let (x0, y0, x1) = (r.x, r.y, r.right());
    let default_fill = match n.shape {
        Shape::Note => &pal.note_bkg,
        Shape::StateStart if flow_css => &pal.node_border,
        Shape::ForkBar if flow_css => &pal.main_bkg,
        Shape::StateStart | Shape::ForkBar => &pal.special_state,
        _ => &pal.main_bkg,
    };
    let default_stroke = match n.shape {
        Shape::Note => &pal.note_border,
        Shape::StateStart | Shape::StateEnd | Shape::ForkBar if flow_css => &pal.node_border,
        Shape::StateStart | Shape::StateEnd | Shape::ForkBar => &pal.special_state,
        _ => &pal.node_border,
    };
    let fill = n.style.fill.clone().unwrap_or_else(|| default_fill.clone());
    let stroke = n
        .style
        .stroke
        .clone()
        .unwrap_or_else(|| default_stroke.clone());
    // A state's choice and note outlines are rough.js paths no stylesheet rule
    // reaches: `strokeWidth` falls back to rough's 1.3 (`userNodeOverrides`).
    let default_width = match (kind, n.shape) {
        (DiagramKind::State, Shape::Choice | Shape::Note) => ROUGH_STROKE_WIDTH,
        _ => lk.node_stroke_width,
    };
    let mut paint = Paint::both(
        &fill,
        &stroke,
        n.style.stroke_width.unwrap_or(default_width),
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
                PartStyle::Fill => els.push(Element::classed_shape(
                    part.geom,
                    Paint::fill(&fill),
                    "basic label-container",
                )),
                PartStyle::AccentLine | PartStyle::AccentFaint => {
                    let mut accent = Paint::stroke(&pal.node_border, 1.0);
                    if part.style == PartStyle::AccentFaint {
                        accent.opacity = Some(0.6);
                    }
                    els.push(Element::shape(part.geom, accent));
                }
                PartStyle::AccentDot => {
                    els.push(Element::shape(part.geom, Paint::fill(&pal.node_border)))
                }
            }
        }
        for g in o.glyphs {
            let text = Text {
                x: g.at.x,
                y: g.at.y,
                lines: vec![g.text.to_string()],
                line_height: g.font_size,
                font_size: g.font_size,
                bold: g.bold,
                anchor: Anchor::Start,
                fill: pal.node_border.clone(),
            };
            els.push(Element::classed_text(text, "console-glyph"));
        }
        o.boundary
    } else {
        match n.shape {
            // stateStart.ts: r = 7.
            Shape::StateStart => {
                els.push(Element::shape(
                    Geometry::Circle { c, r: 7.0 },
                    Paint::both(&fill, &stroke, if flow_css { 2.0 } else { 1.0 }),
                ));
                Boundary::Ellipse(c, 7.0, 7.0)
            }
            Shape::SmallCircle => {
                els.push(Element::shape(
                    Geometry::Circle { c, r: 7.0 },
                    Paint::both(&fill, &stroke, pal.stroke_width.unwrap_or(1.0)),
                ));
                Boundary::Ellipse(c, 7.0, 7.0)
            }
            // stateEnd.ts: rough circles of diameter 14 and 14·5/14 = 5, both
            // stroked 2px.
            Shape::StateEnd => {
                let inner = if flow_css {
                    n.style.fill.as_ref().unwrap_or(&pal.main_bkg)
                } else {
                    &stroke
                };
                els.push(Element::shape(
                    Geometry::Circle { c, r: 7.0 },
                    Paint::both(&pal.background, &stroke, 2.0),
                ));
                els.push(Element::shape(
                    Geometry::Circle { c, r: 2.5 },
                    Paint::both(inner, &stroke, 2.0),
                ));
                Boundary::Ellipse(c, 7.0, 7.0)
            }
            Shape::ForkBar => {
                els.push(Element::shape(
                    Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::both(&fill, &stroke, if flow_css { 2.0 } else { 1.0 }),
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
                    let mut top = y + layout.text_top.get(i).copied().unwrap_or(0.0);
                    if let Some(a) = m.annotation.as_ref().filter(|_| i == 0) {
                        // shapeUtil: the annotation sits on the name, without its bold.
                        let center = Point::new(c.x, top + a.height / 2.0);
                        let t = a.centered_text(center, &text_color);
                        els.push(Element::classed_text(t, "label"));
                        top += a.height;
                    }
                    if !block.is_empty() {
                        let center = Point::new(c.x, top + block.height / 2.0);
                        let mut t = block.centered_text(center, &text_color);
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
                    // erBox's dividers are rough.js polygons stroked at rough's 1.3;
                    // only neo's `.node path { stroke-width }` rule restyles them.
                    line_width: if lk.neo {
                        lk.node_stroke_width
                    } else {
                        ROUGH_STROKE_WIDTH
                    },
                    stroke: &stroke,
                    text: &text_color,
                    // Rows are striped with `rowOdd`; the colour themes paint every row
                    // with the slot fill instead (`er/styles.ts` `.node path`): no stripe.
                    odd_row: pal.er_row_odd.as_ref().filter(|_| {
                        slot_colors(n.color_slot, pal)
                            .and_then(|s| s.fill)
                            .is_none()
                    }),
                    even_row: pal.er_row_even.as_ref().filter(|_| {
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
        // dots centred in the 20px indicator row. flowchart/styles.ts paints both
        // clusterBorder (the dots at 60%), or the palette slot's border colour.
        let marker =
            slot_colors(n.color_slot, pal).map_or(pal.cluster_border.clone(), |s| s.border);
        let sep_y = y0 + shapes::COLLAPSED_PAD + m.block.height + 8.0;
        let mut sep = Paint::stroke(&marker, 0.75);
        sep.dash = Some(vec![3.0, 3.0]);
        els.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(x0 + 8.0, sep_y)),
                PathCmd::LineTo(Point::new(x1 - 8.0, sep_y)),
            ]),
            sep,
            "collapsed-separator",
        ));
        let mut dot = Paint::fill(&marker);
        dot.opacity = Some(0.6);
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
        let mut at = c.plus(m.label_offset);
        if m.block.svg && corrects_glyph_offset(n.shape) {
            // These shapes subtract the text's `getBBox().y` when placing the label,
            // so the glyph box (not the text origin) sits where the HTML box would.
            at.y -= font - (0.905 * font).round();
        }
        let mut t = m.block.centered_text(at, &text_color);
        if m.block.svg && kind == DiagramKind::State {
            // State nodes centre their label box (`centerLabel`), but the state
            // stylesheet has no `text-anchor: middle`: SVG text starts at its left.
            t = t.anchored(Anchor::Start, at.x - m.label_w / 2.0);
        }
        t.bold = m.block.bold;
        els.push(Element::classed_text(t, "nodeLabel"));
    }
    (els, boundary)
}

/// Shapes whose label transform subtracts the label's `bbox.y` (`- (bbox.y -
/// (bbox.top ?? 0))` in mermaid's shape code), which only matters for SVG labels.
fn corrects_glyph_offset(shape: Shape) -> bool {
    matches!(
        shape,
        Shape::Browser
            | Shape::Bucket
            | Shape::Console
            | Shape::BraceLeft
            | Shape::BraceRight
            | Shape::Braces
            | Shape::Cylinder
            | Shape::DividedRect
            | Shape::FlippedTriangle
            | Shape::Folder
            | Shape::Delay
            | Shape::LinedCylinder
            | Shape::LinedDocument
            | Shape::StackedRect
            | Shape::StackedDocument
            | Shape::Note
            | Shape::Person
            | Shape::Asymmetric
            | Shape::LinedRect
            | Shape::SlopedRect
            | Shape::TaggedDocument
            | Shape::HorizontalCylinder
            | Shape::Triangle
            | Shape::Document
            | Shape::WindowPane
    )
}

/// Paints of an entity table.
struct TableColors<'a> {
    paint: &'a Paint,
    /// The rows' and columns' lines: `[data-look="neo"].node path` gives them
    /// the theme's stroke width, as the outline; otherwise 1px.
    line_width: f64,
    stroke: &'a Color,
    text: &'a Color,
    odd_row: Option<&'a Color>,
    /// `rowEven` (`erBox.ts` fills each row with `rowOdd` / `rowEven`).
    even_row: Option<&'a Color>,
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
    let line_paint = Paint::stroke(colors.stroke, colors.line_width);
    for (i, row) in n.sections.iter().enumerate() {
        let ry = y0 + t.header_h + t.row_h * i as f64;
        let (row_fill, class) = if i % 2 == 0 {
            (colors.odd_row, "row-rect-odd")
        } else {
            (colors.even_row, "row-rect-even")
        };
        if let Some(fill) = row_fill {
            els.push(Element::classed_shape(
                Geometry::Rect {
                    rect: Rect::new(x0, ry, r.w, t.row_h),
                    rx: 0.0,
                },
                Paint::fill(fill),
                class,
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
    fn label_position_is_half_way_along_and_stops_at_a_repeat() {
        let p = Point::new;
        let mid = calc_label_position(&[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]);
        assert_eq!(mid, p(10.0, 0.0));
        let mid = calc_label_position(&[p(0.0, 0.0), p(0.0, 0.0), p(0.0, 10.0)]);
        assert_eq!(mid, p(0.0, 0.0), "a zero-length step returns its start");
    }

    /// `body` rendered as the command line does (mermaid's defaults: ELK).
    fn render_cli(body: &str) -> crate::scene::Scene {
        crate::render::render(body, &crate::config::MermaidConfig::default())
            .unwrap()
            .scene
    }

    fn render_dagre(body: &str) -> crate::scene::Scene {
        let src = format!("---\nconfig:\n  layout: dagre\n---\n{body}");
        crate::render::render(&src, &crate::config::MermaidConfig::default())
            .unwrap()
            .scene
    }

    fn walk<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
        out.push(e);
        if let Element::Group { children, .. } = e {
            children.iter().for_each(|c| walk(c, out));
        }
    }

    /// mermaid 12.3.0: a route's points on a frame's border fall inside or
    /// outside it by the float32 sizes `getBBox()` hands back: the nodes'
    /// sizes dagre lays out, and the frame's own. Here the route's third point
    /// lies on C1's top: inside the float32 frame, so the route is cut at its
    /// second (and drawn ending in mid-air).
    #[test]
    fn dagre_frames_and_nodes_are_float32_boxes() {
        let scene = render_dagre(
            "flowchart TD\n  subgraph C0\n    C0_0\n  end\n  subgraph C1\n    C1_0\n    C1_1\n  \
             end\n  x0[[\"A much longer node text here\"]]\n  x0 -->|longer edge label| C1\n  \
             C1 --o|no| C1_0\n  C1_0 -->|no| C1_0\n  x0 --o|yes| C1_0\n",
        );
        let mut all = Vec::new();
        scene.elements.iter().for_each(|e| walk(e, &mut all));
        let d = all.iter().find_map(|e| match e {
            Element::Group {
                id: Some(i),
                children,
                ..
            } if i.starts_with("L-x0-C1-") => children.iter().find_map(|c| match c {
                Element::Shape {
                    geom: Geometry::Path(cmds),
                    ..
                } => Some(crate::output::svg::path_data(cmds)),
                _ => None,
            }),
            _ => None,
        });
        assert_eq!(d.as_deref(), Some("M385.374,92.5L428.039,155.193"));
    }

    /// mermaid 12.3.0: a stadium is as wide as its drawn polygon, whose
    /// 50-point arcs miss the circles' extremes (`Start`: 171.227, not
    /// 171.25), which decides where the route from C1 to x0 is cut at C1's
    /// border and so how far its label moves.
    #[test]
    fn dagre_stadiums_are_as_wide_as_their_polygon() {
        let scene = render_dagre(
            "flowchart LR\n  subgraph C0\n    C0_0\n  end\n  subgraph C1\n    C1_0\n    C1_1\n    \
             C1_0 --> C0_0\n  end\n  subgraph C2\n    C2_0\n    C2_1\n    C2_2\n  end\n  \
             x0(\"Decision?\")\n  x1([\"Start\"])\n  x2[(\"ok\")]\n  C1_0 --x|two<br>lines| C1\n  \
             C1_0 ==> C2\n  C1 -.->|yes| x0\n  x1 -.-> C2_0\n  x1 --> C1_1\n  C1_0 --> C0_0\n  \
             C0_0 --> x1\n",
        );
        let mut all = Vec::new();
        scene.elements.iter().for_each(|e| walk(e, &mut all));
        let centres: Vec<Point> = all
            .iter()
            .filter_map(|e| match e {
                Element::Group {
                    class: Some(c),
                    children,
                    ..
                } if c == "edgeLabel" => children.iter().find_map(|c| match c {
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        ..
                    } => Some(rect.center()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        let at = Point::new(987.4221854345703, 434.0);
        assert!(centres.iter().any(|c| c.dist(at) < 1e-3), "{centres:?}");
    }

    /// mermaid 12.3.0: under dagre `getEdgesToRender` draws the self-loops
    /// after the other edges; ELK keeps the edges' order.
    #[test]
    fn dagre_draws_self_loops_last() {
        fn edge_ids(e: &Element, out: &mut Vec<String>) {
            if let Element::Group {
                id,
                class,
                children,
            } = e
            {
                match (id, class.as_deref()) {
                    (Some(i), Some("edgePath")) => out.push(i.clone()),
                    _ => children.iter().for_each(|c| edge_ids(c, out)),
                }
            }
        }
        let order = |layout: &str| {
            let src = format!(
                "---\nconfig:\n  layout: {layout}\n---\nflowchart TD\n  A --> A\n  A --> B\n  \
                 B --> B\n  B --> C\n"
            );
            let scene = crate::render::render(&src, &crate::config::MermaidConfig::default())
                .unwrap()
                .scene;
            let mut ids = Vec::new();
            scene.elements.iter().for_each(|e| edge_ids(e, &mut ids));
            ids
        };
        assert_eq!(order("dagre"), ["L-A-B-1", "L-B-C-3", "L-A-A-0", "L-B-B-2"]);
        assert_eq!(order("elk"), ["L-A-A-0", "L-A-B-1", "L-B-B-2", "L-B-C-3"]);
    }

    /// mermaid 12.3.0 (class diagram under dagre): `0..1` at both ends of a
    /// route through (35.21, 89), (40.70, 124.5), (35.21, 160).
    #[test]
    fn terminal_labels_sit_beside_the_routes_ends() {
        let p = Point::new;
        let path = [
            p(35.21463815789474, 89.0),
            p(40.703125, 124.5),
            p(35.21463815789474, 160.0),
        ];
        let close = |a: Point, b: Point| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9;
        let start = calc_terminal_label_position(TERMINAL_MARKER_SIZE, false, &path);
        assert!(
            close(start, p(23.064584056151183, 108.58637601386286)),
            "{start:?}"
        );
        let end = calc_terminal_label_position(TERMINAL_MARKER_SIZE, true, &path);
        assert!(
            close(end, p(52.71234410174355, 144.99732601386287)),
            "{end:?}"
        );
    }

    /// `calculateDeltaAndAngle` on a zero-length first segment is `atan(0/0)`:
    /// a start with a `markerOffsets` key (even `arrow_barb`'s 0) becomes NaN,
    /// one without (`arrow_circle`) stays where it is.
    #[test]
    fn line_offsets_on_a_zero_length_end_segment_are_nan() {
        let p = Point::new;
        let data = [
            p(10.0, 474.0),
            p(10.0, 474.0),
            p(10.0, 499.0),
            p(40.0, 520.0),
        ];
        let out = line_with_offset(&data, Some(4.0), Some(4.0));
        assert!(out[0].x.is_nan() && out[0].y.is_nan(), "{:?}", out[0]);
        assert!(out[3].x.is_finite());
        let out = line_with_offset(&data, Some(0.0), None);
        assert!(out[0].x.is_nan());
        let out = line_with_offset(&data, None, Some(4.0));
        assert_eq!(out[0], data[0]);
    }

    /// `getLineFunctionsWithOffset` (mermaid 12.3.0, a class self-loop with an
    /// `extension` end): the loop's first point, 16.8 above its end, is pushed
    /// up to clear the end's marker; the end is pulled back along its line.
    #[test]
    fn line_offsets_clear_a_nearby_end_marker() {
        let p = Point::new;
        let data = [
            p(288.6156250014901, 420.68501042864625),
            p(264.6156250014901, 411.0750000011176),
            p(264.6156250014901, 447.0750000011176),
            p(288.6156250014901, 437.4649895735889),
        ];
        let out = line_with_offset(&data, None, Some(17.25));
        assert!((out[0].y - 419.215).abs() < 1e-3, "{:?}", out[0]);
        assert_eq!(out[0].x, data[0].x);
        assert_eq!(&out[1..3], &data[1..3]);
        let back = out[3].minus(data[3]);
        assert!((back.len() - 17.25).abs() < 1e-9);
    }

    /// `isLabelCoordinateInPath` reads the `d` of a path drawn in a
    /// translated group (a cluster's own graph): in its local coordinates.
    #[test]
    fn label_coordinate_is_looked_up_in_local_coordinates() {
        let p = Point::new;
        let d = [
            geom::PathCmd::MoveTo(p(100.0, 0.0)),
            geom::PathCmd::LineTo(p(300.0, 0.0)),
        ];
        // "M100,0L300,0" has no 50 or 77; locally "M50,0L250,0" has the 0.
        assert!(!label_coordinate_in_path(
            p(50.0, 77.0),
            &d,
            Point::default()
        ));
        assert!(label_coordinate_in_path(p(50.0, 77.0), &d, p(50.0, 0.0)));
    }

    #[test]
    fn label_coordinate_is_looked_up_as_text() {
        let p = Point::new;
        let d = [
            geom::PathCmd::MoveTo(p(12.4, 150.6)),
            geom::PathCmd::LineTo(p(-3.5, 40.0)),
        ];
        // "M12,151L-4,40": 15 is inside "151", 4 inside "-4".
        assert!(label_coordinate_in_path(
            p(15.2, 999.0),
            &d,
            Point::default()
        ));
        assert!(label_coordinate_in_path(
            p(999.0, 3.6),
            &d,
            Point::default()
        ));
        assert!(!label_coordinate_in_path(
            p(77.0, 99.0),
            &d,
            Point::default()
        ));
    }

    #[test]
    fn cut_path_stops_at_the_frame() {
        let p = Point::new;
        let r = (p(50.0, 50.0), 100.0, 100.0);
        // `intersection` measures its offset from the inside point's side.
        let cut = cut_path_at_intersect(&[p(80.0, 160.0), p(50.0, 50.0)], r);
        assert_eq!(cut[0], p(80.0, 160.0));
        assert!((cut[1].x - (50.0 + 30.0 * 60.0 / 110.0)).abs() < 1e-9 && cut[1].y == 100.0);
        // Met straight on, it returns the outside point (`r === 0`): no new one.
        let cut = cut_path_at_intersect(&[p(50.0, 200.0), p(50.0, 150.0), p(50.0, 50.0)], r);
        assert_eq!(cut, vec![p(50.0, 200.0), p(50.0, 150.0)]);
        // A point on the border counts as outside.
        let cut = cut_path_at_intersect(&[p(50.0, 150.0), p(50.0, 100.0), p(50.0, 50.0)], r);
        assert_eq!(cut, vec![p(50.0, 150.0), p(50.0, 100.0)]);
    }

    #[test]
    fn empty_members_keep_their_divider_two_gaps_down() {
        // mermaid 12.3.0, `class Keeper { <<interface>> +feed(Animal a) }`: box
        // 141 high, dividers at -4.5 and 19.5 from its centre, the method's
        // label 19.5 below the second.
        let t = |s: &str| TextBlock::new(vec![s.into()], 14.0, false).html();
        let blocks = [
            t("Keeper"),
            TextBlock::new(vec![], 14.0, false),
            t("+feed(Animal a)"),
        ];
        let (l, _, h) = compartment_layout(DiagramKind::Class, &blocks, Some(&t("x")), 12.0);
        assert_eq!(h, 141.0);
        assert_eq!(l.heights, vec![66.0, 24.0, 51.0]);
        assert_eq!(l.text_top[2], 19.5);
    }

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
        let cut = cut_at_rect(&pts, &r, false);
        assert_eq!(cut[0], Point::new(60.0, 100.0));
        assert_eq!(&cut[1..], &pts[2..]);
        // A path that never leaves the box is left alone.
        assert_eq!(cut_at_rect(&pts[..2], &r, false), pts[..2].to_vec());
        // An orthogonal route is cut where it crosses, keeping its right angles
        // (moving the cut under the next point would make the last leg diagonal).
        let pts = vec![
            Point::new(50.0, 50.0),
            Point::new(50.0, 120.0),
            Point::new(-30.0, 120.0),
            Point::new(-30.0, 200.0),
        ];
        let cut = cut_at_rect(&pts, &r, true);
        assert!((cut[0].x - 50.0).abs() < 1e-9 && (cut[0].y - 100.0).abs() < 1e-6);
        assert_eq!(&cut[1..], &pts[1..]);
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

    fn frame_rect(els: &[Element]) -> Rect {
        els.iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    ..
                } => Some(*rect),
                _ => None,
            })
            .unwrap_or_else(|| panic!("frame rect: {els:?}"))
    }

    /// mermaid 12.3.0 (ELK): an empty subgraph is laid out as a point, its
    /// title painted from there down: a frame around only that is 48 wide.
    #[test]
    fn elk_empty_nested_subgraph_is_a_point() {
        let (_, scene) =
            render_flowchart("flowchart TB\nsubgraph outer\nsubgraph inner\nend\nend\nA --> B");
        let outer = frame_rect(cluster_group(&scene, "outer"));
        let Some(Element::Text { text: ot, .. }) = cluster_group(&scene, "outer")
            .iter()
            .find(|e| matches!(e, Element::Text { .. }))
        else {
            panic!("outer title");
        };
        let font = ot.font_size;
        let title = TextBlock::label("outer", font).html();
        // Two paddings around a point; the title is narrower.
        assert!(title.width + CLUSTER_TITLE_PAD < 48.0);
        assert!((outer.w - 48.0).abs() < 1e-6, "{outer:?}");
        let inner = cluster_group(&scene, "inner");
        let Some(Element::Text { text: t, .. }) = inner.first() else {
            panic!("title only: {inner:?}");
        };
        assert!((t.x - outer.center().x).abs() < 1e-6);
        // The title's top on the point, the padding (24) above the frame's
        // bottom.
        let top = t.y - font * 0.35 - title.height / 2.0;
        assert!(
            (top - (outer.bottom() - 24.0)).abs() < 1e-6,
            "{top} {outer:?}"
        );
    }

    /// The centre x of the text `label` (a node's label) in `scene`.
    fn label_x(scene: &Scene, label: &str) -> f64 {
        let mut all = Vec::new();
        scene.elements.iter().for_each(|e| walk(e, &mut all));
        all.iter()
            .find_map(|e| match e {
                Element::Text { text, .. } if text.lines.iter().any(|l| l == label) => Some(text.x),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no label {label}"))
    }

    /// mermaid 12.3.0 (ELK): an empty subgraph is a plain node of no size to
    /// ELK, not a container: inside a container laid out on its own it is a
    /// connected component of its own, packed in a row of its own (no row is
    /// narrower than the component spacing, 24): above the node here (one
    /// spacing over it), not laid out as a nested box.
    #[test]
    fn elk_empty_subgraph_is_a_component_of_its_parent() {
        let scene =
            render_cli("flowchart LR\nsubgraph G2\ndirection BT\ng2n0\nsubgraph G3\nend\nend");
        let f = frame_of(&scene, "G2");
        // The point under the title and padding; the node 24 below it; 24
        // under that (mermaid's frame: 141 high).
        let title = cluster_title_height(
            ClusterKind::Group,
            &TextBlock::label("G2", 14.0).html(),
            true,
        );
        assert!(
            (f.h - (24.0 + title + 24.0 + 45.0 + 24.0)).abs() < 1e-6,
            "{f:?} {title}"
        );
        assert!((f.h - 141.0).abs() < 1e-6, "{f:?}");
        // Two points only: one spacing wide between the paddings.
        let scene = render_cli(
            "flowchart BT\nsubgraph G1\ndirection LR\nsubgraph G2\nsubgraph G3\nend\nsubgraph G4\nend\nend\nend",
        );
        let f = frame_of(&scene, "G2");
        assert!((f.w - 72.0).abs() < 1e-6, "{f:?}");
    }

    /// The title of empty subgraph `id`: its text element.
    fn empty_title(scene: &Scene, id: &str) -> Text {
        match cluster_group(scene, id) {
            [Element::Text { text, .. }] => text.clone(),
            other => panic!("title only: {other:?}"),
        }
    }

    /// mermaid 12.3.0 (ELK): `alignDegenerateNodeToAnchor` moves an empty
    /// subgraph's point (no size) onto the anchor of its first edge, its
    /// container's edge to it included; its title is painted from there.
    /// The diagram's bounds take the title in.
    #[test]
    fn elk_empty_subgraph_point_moves_onto_its_edge() {
        let top = |t: &Text| t.y - t.font_size * 0.35 - t.line_height / 2.0;
        // Across the flow: the container's edge enters the point from its side.
        let scene =
            render_cli("flowchart LR\nsubgraph G0[A title]\nsubgraph G1\nend\na\nend\nG0 --> G1");
        let end = *edge_points(&scene, "L-G0-G1-0").last().expect("route");
        let t = empty_title(&scene, "G1");
        assert!((top(&t) - end.y).abs() < 1e-3, "{t:?} {end:?}");
        // Along the flow: the edge leaves the point's bottom.
        let scene = render_cli(
            "flowchart TD\nsubgraph G1[Outer]\nsubgraph G2[Inner]\nend\na --> b\nend\nG2 --> b\nx --> G0\nsubgraph G0[Last]\nend",
        );
        let start = edge_points(&scene, "L-G2-b-1")[0];
        let t = empty_title(&scene, "G2");
        assert!((t.x - start.x).abs() < 1e-3, "{t:?} {start:?}");
        let last = empty_title(&scene, "G0");
        let bottom = top(&last) + last.line_height;
        assert!(
            scene.view_box.bottom() >= bottom + 8.0 - 1e-3,
            "{:?} {last:?}",
            scene.view_box
        );
    }

    /// mermaid 12.3.0 (ELK): two ports crammed onto a point's side (6.67
    /// apart, in reverse past its middle) keep their port list's order for
    /// Brandes–Köpf, so the four alignments differ and balance the point over
    /// the middle of the node it has a two-cycle with. The point then moves
    /// onto the anchor of its first edge as handed to ELK: an edge from inside
    /// a subgraph goes before the one from that subgraph's border.
    #[test]
    fn elk_point_ports_keep_their_list_order() {
        let scene = render_cli("flowchart TD\nsubgraph G0\nend\na\nG0 --> a\na --> G0");
        let t = empty_title(&scene, "G0");
        let a = label_x(&scene, "a");
        let start = edge_points(&scene, "L-G0-a-0")[0];
        assert!((t.x - start.x).abs() < 1e-3, "{t:?} {start:?}");
        assert!((start.x - (a + 10.0 / 3.0)).abs() < 1e-3, "{start:?} {a}");
        let scene = render_cli(
            "flowchart TD\nsubgraph G0\nend\nsubgraph G1\ng1n0\nend\nG0 --> G1\ng1n0 --> G0",
        );
        let t = empty_title(&scene, "G0");
        let end = *edge_points(&scene, "L-g1n0-G0-1").last().expect("route");
        assert!((t.x - end.x).abs() < 1e-3, "{t:?} {end:?}");
    }

    /// mermaid 12.3.0 (ELK): a self-loop's two ports crammed onto a point's
    /// side come out the other way round, 4 either side of it (inside the
    /// 12px surrounding): the loop leaves 4 past the point, which moves onto
    /// that, and its legs count in the point's margins along the flow, which
    /// hold the nodes of its layer that lean towards their edges' side.
    #[test]
    fn elk_loop_on_a_point_stands_beside_it() {
        let scene = render_cli("flowchart TD\nsubgraph G0[A title]\nend\nx0\nG0 --> G0");
        let route = edge_points(&scene, "L-G0-G0-0");
        let t = empty_title(&scene, "G0");
        let top = t.y - t.font_size * 0.35 - t.line_height / 2.0;
        assert!((top - route[0].y).abs() < 1e-3, "{t:?} {route:?}");
        assert!(
            (route[0].y - route[route.len() - 1].y - 8.0).abs() < 1e-3,
            "{route:?}"
        );
        let scene = render_cli(
            "flowchart LR\nsubgraph G0\nend\nx1{\"Load\"}\nx2{{\"Done\"}}\nG0 -.-> G0\nx1 -.-> x0",
        );
        let (x1, x2) = (label_x(&scene, "Load"), label_x(&scene, "Done"));
        assert!((x1 - x2 - 12.5).abs() < 0.01, "{x1} {x2}");
    }

    /// mermaid 12.3.0 (ELK): a container laid out on its own is as wide as its
    /// title label (`NODE_LABELS`, 5 either side), its contents at its left;
    /// `evenGroupFrames` then pulls the frame in to the title's floor (label +
    /// 8) — unless it holds only points, which it does not pull in at all.
    #[test]
    fn elk_separate_container_holds_its_title_label() {
        let long = "A really very much longer title for this subgraph here";
        let title = TextBlock::label(long, 14.0).html();
        let scene = render_cli(&format!(
            "flowchart TD\nsubgraph S[{long}]\ndirection LR\na\nend\nb --> S"
        ));
        // (The frame itself, after its drop shadow.)
        let f = cluster_group(&scene, "S")
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    ..
                } => Some(*rect),
                _ => None,
            })
            .next_back()
            .expect("frame");
        assert!(
            (f.w - (title.width + CLUSTER_TITLE_PAD)).abs() < 1e-3,
            "{f:?}"
        );
        let a = label_x(&scene, "a");
        assert!((a - (f.x + 24.0 + 76.0)).abs() < 1.0, "{a} {f:?}");
        let scene = render_cli(&format!(
            "flowchart TD\nsubgraph S[{long}]\ndirection BT\nsubgraph T\nend\nend\nb --> S"
        ));
        let f = frame_of(&scene, "S");
        assert!((f.w - (title.width + 10.0)).abs() < 1e-3, "{f:?}");
    }

    /// mermaid 12.3.0 (dagre): an empty subgraph is an ordinary `rect` node
    /// labelled with its title (no minimum label width), its classes applied.
    #[test]
    fn dagre_empty_subgraph_is_a_plain_node() {
        let scene = render_dagre(
            "flowchart LR\nsubgraph outer\nsubgraph inner[Inner title]\nend\nX\nend\nA --> inner\nclassDef hot fill:#f96\nclass inner hot",
        );
        let mut all = Vec::new();
        scene.elements.iter().for_each(|e| walk(e, &mut all));
        let boxes: Vec<(Rect, &crate::scene::Paint)> = all
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    paint,
                    ..
                } => Some((*rect, paint)),
                _ => None,
            })
            .collect();
        let inner = boxes
            .iter()
            .find(|(_, p)| p.fill.0 == "#f96")
            .unwrap_or_else(|| panic!("no node box for the subgraph: {boxes:?}"));
        let label = all
            .iter()
            .find_map(|e| match e {
                Element::Text { text, .. } if text.lines.iter().any(|l| l == "Inner title") => {
                    Some(text)
                }
                _ => None,
            })
            .expect("the title as the node's label");
        let title = TextBlock::label("Inner title", label.font_size).html();
        assert!((inner.0.w - (title.width + 32.0)).abs() < 1e-6, "{inner:?}");
        assert!(
            (inner.0.h - (title.height + 24.0)).abs() < 1e-6,
            "{inner:?}"
        );
        assert_eq!(inner.1.fill.0, "#f96");
        let labels = all
            .iter()
            .filter(|e| matches!(e, Element::Text { text, .. } if text.lines.iter().any(|l| l.contains("Inner title"))))
            .count();
        assert_eq!(labels, 1, "the title is the node's label, painted once");
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

    /// The dash pattern of an edge's line.
    fn edge_dash(scene: &Scene, id: &str) -> Option<Vec<f64>> {
        let Element::Group { children, .. } = &scene.elements[1] else {
            panic!("edgePaths second");
        };
        children
            .iter()
            .find_map(|c| match c {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i == id => match children.first() {
                    Some(Element::Shape { paint, .. }) => Some(paint.dash.clone()),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_else(|| panic!("no edge {id}"))
    }

    #[test]
    fn neo_edges_hide_their_ends_under_the_markers_by_a_pattern() {
        // mermaid 12.3.0: A --> B ends 4 short of B (`markerOffsets`) and hides
        // 4 more (`markerOffsets2`): `0 0 185.67 4` along its rounded path;
        // A --o C ends on C and hides 12.5 (`0 0 27.5 12.5`); the dotted
        // A -.-> D hides 4 after its 2px dashes and gaps.
        let src = "flowchart TD\n  A --> B\n  A --o C\n  A -.-> D\n";
        let scene_of = |src: &str| {
            crate::render::render(src, &crate::config::MermaidConfig::default())
                .expect("renders")
                .scene
        };
        let scene = scene_of(src);
        let ab = edge_points(&scene, "L-A-B-0");
        let ac = edge_points(&scene, "L-A-C-1");
        let (b_end, c_end) = (*ab.last().unwrap(), *ac.last().unwrap());
        assert!((c_end.y - b_end.y - 4.0).abs() < 1e-6, "{ab:?} {ac:?}");
        assert!((c_end.y - ac[0].y - 40.0).abs() < 1e-6, "{ac:?}");
        let d = edge_dash(&scene, "L-A-B-0").expect("pattern");
        assert_eq!((d.len(), d[0], d[1], d[3]), (4, 0.0, 0.0, 4.0), "{d:?}");
        assert!((d[2] - 185.67).abs() < 0.01, "{d:?}");
        assert_eq!(
            edge_dash(&scene, "L-A-C-1"),
            Some(vec![0.0, 0.0, 27.5, 12.5])
        );
        let d = edge_dash(&scene, "L-A-D-2").expect("pattern");
        let n = d.len();
        assert_eq!((d[0], d[1], d[n - 1]), (0.0, 0.0, 4.0), "{d:?}");
        assert!(d[2..n - 1].iter().all(|&x| x == 2.0), "{d:?}");
        // As many 2 + 2 pairs as fit in its 185.67 between the ends (46).
        assert_eq!((n - 3) / 2, 46, "{d:?}");
        // The classic look ends its line at the same pull-back, all drawn.
        let classic = format!("---\nconfig:\n  look: classic\n---\n{src}");
        let scene = scene_of(&classic);
        let classic_ab = edge_points(&scene, "L-A-B-0");
        let end = *classic_ab.last().unwrap();
        assert!((end.y - classic_ab[0].y - (b_end.y - ab[0].y)).abs() < 1e-6);
        assert_eq!(edge_dash(&scene, "L-A-B-0"), None);
    }

    #[test]
    fn neo_dash_patterns_follow_generate_dash_array() {
        assert_eq!(neo_dash(100.0, (0.0, 0.0), false), None);
        assert_eq!(
            neo_dash(100.0, (0.0, 4.0), false),
            Some(vec![0.0, 0.0, 96.0, 4.0])
        );
        // Too short for its ends: the browser ignores the negative pattern.
        assert_eq!(neo_dash(10.0, (12.5, 12.5), false), None);
        assert_eq!(
            neo_dash(13.0, (0.0, 4.0), true),
            Some(vec![0.0, 0.0, 2.0, 2.0, 2.0, 2.0, 4.0])
        );
        assert_eq!(neo_dash(3.0, (0.0, 4.0), true), Some(vec![0.0, 0.0, 4.0]));
    }

    #[test]
    fn a_hopped_neo_edge_is_dashed_again_for_its_new_length() {
        // `applyLineJumpsToSvg`: `0 a b c …` becomes `0 a (len - a - c) c`,
        // in place of whatever pattern the edge had; a pattern of fewer than
        // four numbers is left alone.
        let edge = |dash: Option<Vec<f64>>| {
            let mut paint = Paint::stroke(&Color::new("#000"), 2.0);
            paint.dash = dash;
            Element::Group {
                id: None,
                class: None,
                children: vec![Element::shape(Geometry::Path(Vec::new()), paint)],
            }
        };
        let dash = |el: &Element| match el {
            Element::Group { children, .. } => match &children[0] {
                Element::Shape { paint, .. } => paint.dash.clone(),
                _ => None,
            },
            _ => None,
        };
        let mut el = edge(Some(vec![0.0, 0.0, 90.0, 4.0]));
        redash_hopped(&mut el, &[0.0, 0.0, 90.0, 4.0], 106.0);
        assert_eq!(dash(&el), Some(vec![0.0, 0.0, 102.0, 4.0]));
        let mut el = edge(Some(vec![5.0, 5.0]));
        redash_hopped(&mut el, &[0.0, 0.0, 2.0, 2.0, 2.0, 2.0, 4.0], 50.0);
        assert_eq!(dash(&el), Some(vec![0.0, 0.0, 48.0, 2.0]));
        let mut el = edge(None);
        redash_hopped(&mut el, &[0.0, 0.0, 4.0], 50.0);
        assert_eq!(dash(&el), None);
    }

    #[test]
    fn elk_edge_from_a_nested_cluster_leaves_its_own_border() {
        // mermaid 12.3.0: C2 --> x starts on C2's bottom (177) and runs on
        // through C1's (201), not from C1's border alone.
        let (_, scene) = render_flowchart(
            "flowchart TD\n  subgraph C1\n    b\n    subgraph C2\n      a\n    end\n  end\n  C2 --> x\n",
        );
        let (c1, c2) = (frame_of(&scene, "C1"), frame_of(&scene, "C2"));
        let pts = edge_points(&scene, "L-C2-x-0");
        assert!((pts[0].y - c2.bottom()).abs() < 1e-6, "{pts:?} {c2:?}");
        assert!(pts[0].x > c2.x && pts[0].x < c2.right(), "{pts:?} {c2:?}");
        assert!(pts.iter().any(|p| p.y > c1.bottom()), "{pts:?} {c1:?}");
    }

    #[test]
    fn elk_member_edges_from_their_container_run_straight_from_own_ports() {
        // mermaid 12.3.0: C --> a twice, from C's top straight down into a,
        // each from a port of its own (the first on the right: x 133.7 and
        // 90.3 about a's 112).
        let (_, scene) = render_flowchart(
            "flowchart TD\n  subgraph C\n    a --> b\n  end\n  C --> a\n  C --> a\n",
        );
        let frame = frame_of(&scene, "C");
        let (p, q) = (
            edge_points(&scene, "L-C-a-1"),
            edge_points(&scene, "L-C-a-2"),
        );
        for pts in [&p, &q] {
            assert_eq!(pts.len(), 2, "{pts:?}");
            assert!((pts[0].y - frame.y).abs() < 1e-6, "{pts:?} {frame:?}");
            assert!((pts[0].x - pts[1].x).abs() < 1e-6, "{pts:?}");
        }
        assert!(p[0].x > q[0].x + 20.0, "{p:?} {q:?}");
    }

    #[test]
    fn elk_container_holds_its_last_layer_of_port_dummies() {
        // mermaid 12.3.0 (ELK): c1n0 --> C0 runs back up inside C0 through a
        // channel below C1; a loop on C0 gives C0 a port dummy in a last
        // layer of its own, 30 past that channel: C0 is 259 tall (its frame
        // drawn 189 without the loop, pulled in to its contents).
        let src = "flowchart TD\n  subgraph C0\n    c0n0\n    subgraph C1\n      c1n0\n    end\n  end\n  x0\n  c1n0 --> C0\n";
        let h = |src: &str| {
            let r = crate::render::render(src, &crate::config::MermaidConfig::default())
                .expect("renders");
            frame_of(&r.scene, "C0").h
        };
        let looped = format!("{src}  C0 --> C0\n");
        assert_eq!((h(&looped), h(src)), (259.0, 189.0));
    }

    #[test]
    fn elk_antiparallel_edges_take_their_ports_from_crossing_minimisation() {
        // Between two nodes, a --> b and b --> a leave and reach the same
        // nodes; ELK orders their ports by barycentre, a tie keeping the
        // order its sweeps left them in. At the root (`considerModelOrder`)
        // a --> b runs left (mermaid: x 66 and 110 about 88); inside a
        // container, which has no model order, right (134 and 90 about 112).
        let xs = |src: &str| {
            let (_, scene) = render_flowchart(src);
            let ab = edge_points(&scene, "L-a-b-0");
            let ba = edge_points(&scene, "L-b-a-1");
            (ab[0].x, ba[0].x)
        };
        let (ab, ba) = xs("flowchart TD\n  a --> b\n  b --> a\n");
        assert!(ab + 40.0 < ba, "{ab} {ba}");
        let (ab, ba) = xs("flowchart TD\n  subgraph C\n    a --> b\n    b --> a\n  end\n");
        assert!(ba + 40.0 < ab, "{ab} {ba}");
    }

    /// Loops round a container whose ports keep their sides (mermaid 12.3.0):
    /// they nest by ELK's routing slots, each leg `spacing.edgeEdge` (20)
    /// beyond the one inside it; loops round opposite sides nest apart; a
    /// label stands on its loop's leg.
    #[test]
    fn elk_loops_round_a_container_nest() {
        fn find<'a>(e: &'a Element, id: &str) -> Option<&'a [Element]> {
            match e {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i == id => Some(children.as_slice()),
                Element::Group { children, .. } => children.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        let group = |scene: &Scene, id: &str| -> Vec<Element> {
            scene
                .elements
                .iter()
                .find_map(|e| find(e, id))
                .unwrap_or_else(|| panic!("no {id}"))
                .to_vec()
        };
        let frame = |scene: &Scene| -> Rect {
            group(scene, "cluster-C0")
                .iter()
                .find_map(|e| match e {
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        class,
                        ..
                    } if class.as_deref() != Some("drop-shadow") => Some(*rect),
                    _ => None,
                })
                .expect("frame")
        };
        // Leftmost, rightmost and lowest point of a loop's path.
        let extent = |scene: &Scene, id: &str| -> (f64, f64, f64) {
            let els = group(scene, id);
            let Some(Element::Shape {
                geom: Geometry::Path(cmds),
                ..
            }) = els.first()
            else {
                panic!("path {id}");
            };
            let pts: Vec<Point> = cmds
                .iter()
                .filter_map(|c| match c {
                    PathCmd::MoveTo(p) | PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) => Some(*p),
                    _ => None,
                })
                .collect();
            let xs = pts.iter().map(|p| p.x);
            (
                xs.clone().fold(f64::INFINITY, f64::min),
                xs.fold(f64::NEG_INFINITY, f64::max),
                pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
            )
        };
        let render = |src: &str| {
            crate::render::render(src, &crate::config::MermaidConfig::default())
                .expect("renders")
                .scene
        };
        let near = |a: f64, b: f64| (a - b).abs() < 0.5;
        let base = "  subgraph C0\n    c0n0\n  end\n  x0\n  x0 --> C0\n  c0n0 --> x0\n  \
                    C0 --> c0n0\n  c0n0 --> C0\n";
        // Two loops left to right, round the bottom: the second, whose ports
        // are the lower ones, inside (mermaid: frame 254..514 x 12..144; legs
        // at 554 / 534 east, 204 / 224 west, 196 / 176 below).
        let scene = render(&format!("flowchart LR\n{base}  C0 --> C0\n  C0 --> C0\n"));
        let f = frame(&scene);
        let (w0, e0, b0) = extent(&scene, "L-C0-C0-4");
        let (w1, e1, b1) = extent(&scene, "L-C0-C0-5");
        assert!(
            near(e0 - f.right(), 40.0) && near(e1 - f.right(), 20.0),
            "{e0} {e1} {f:?}"
        );
        assert!(
            near(f.x - w0, 50.0) && near(f.x - w1, 30.0),
            "{w0} {w1} {f:?}"
        );
        assert!(
            near(b0 - f.bottom(), 52.0) && near(b1 - f.bottom(), 32.0),
            "{b0} {b1} {f:?}"
        );
        // Labelled, each stands on its leg's middle, the leg one half its
        // label further out (mermaid: 231.5 and 186.5).
        let scene = render(&format!(
            "flowchart LR\n{base}  C0 -->|one| C0\n  C0 -->|two| C0\n"
        ));
        let f = frame(&scene);
        let (_, _, b0) = extent(&scene, "L-C0-C0-4");
        let (_, _, b1) = extent(&scene, "L-C0-C0-5");
        assert!(
            near(b0 - f.bottom(), 87.5) && near(b1 - f.bottom(), 42.5),
            "{b0} {b1} {f:?}"
        );
        // Top down, one loop round the left and two round the right: the left
        // one's legs stand as the innermost right one's (mermaid: frame
        // 32..232 x 147..333; legs below at 353, 373 and 353).
        let scene = render(
            "flowchart TD\n  subgraph C0\n    c0n0 --> c0n1\n  end\n  x0 --> c0n0\n  \
             C0 --> C0\n  C0 --> C0\n  C0 --> C0\n",
        );
        let f = frame(&scene);
        let below: Vec<f64> = (2..5)
            .map(|i| extent(&scene, &format!("L-C0-C0-{i}")).2 - f.bottom())
            .collect();
        assert!(
            near(below[0], 20.0) && near(below[1], 40.0) && near(below[2], 20.0),
            "{below:?}"
        );
    }

    #[test]
    fn elk_loop_on_a_cluster_stands_off_its_side_across_the_flow() {
        // As ELK's self-loop on the container's node (mermaid: frame 32,12
        // 200x186, loop 32,78 -> 12,78 -> 12,132 -> 28,132): ports a third of
        // the side apart inside the 12px surrounding, the leg 20 off the side,
        // the frame no wider than without the loop.
        let (_, scene) =
            render_flowchart("flowchart TD\n  subgraph A\n    a --> b\n  end\n  A --> A\n");
        let frame = frame_of(&scene, "A");
        let (_, plain) = render_flowchart("flowchart TD\n  subgraph A\n    a --> b\n  end\n");
        assert!(
            (frame.w - frame_of(&plain, "A").w).abs() < 1e-6,
            "{frame:?}"
        );
        let lp = edge_points(&scene, "L-A-A-1");
        let gap = (frame.h - 24.0) / 3.0;
        assert!((lp[0].x - frame.x).abs() < 1e-6, "{lp:?} {frame:?}");
        assert!(
            (lp[0].y - (frame.y + 12.0 + gap)).abs() < 0.5,
            "{lp:?} {frame:?}"
        );
        let end = *lp.last().unwrap();
        assert!((end.y - (frame.y + 12.0 + 2.0 * gap)).abs() < 0.5, "{lp:?}");
        assert!(end.x < frame.x && end.x > frame.x - 10.0, "{lp:?}");
        let out = lp.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        assert!((out - (frame.x - 20.0)).abs() < 0.5, "{lp:?}");
        // Laid out to the right, it stands on the north side.
        let (_, scene) =
            render_flowchart("flowchart LR\n  subgraph A\n    a --> b\n  end\n  A --> A\n");
        let frame = frame_of(&scene, "A");
        let lp = edge_points(&scene, "L-A-A-1");
        assert!((lp[0].y - frame.y).abs() < 1e-6, "{lp:?} {frame:?}");
        let top = lp.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        assert!((top - (frame.y - 20.0)).abs() < 0.5, "{lp:?}");
        assert!(lp.last().unwrap().x > lp[0].x, "{lp:?}");
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
        // A --> A: A's ports keep their sides (it has port dummies inside), so
        // the loop leaves at the bottom and comes back round the left to the
        // top, 20 off each side (mermaid: 144,303 -> 144,323 -> 12,323 ->
        // 12,97 -> 156,97 -> 156,117 around the frame at 32,117 200x186 under
        // the neo look).
        let lp = edge_points(&scene, "L-A-A-2");
        assert!((lp[0].y - frame.bottom()).abs() < eps, "{lp:?} {frame:?}");
        let (min_x, min_y, max_y) = lp.iter().fold(
            (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY),
            |(a, b, c), p| (a.min(p.x), b.min(p.y), c.max(p.y)),
        );
        assert!((min_x - (frame.x - 20.0)).abs() < 0.5, "{lp:?}");
        assert!((min_y - (frame.y - 20.0)).abs() < 0.5, "{lp:?}");
        assert!((max_y - (frame.bottom() + 20.0)).abs() < 0.5, "{lp:?}");
        let end = *lp.last().unwrap();
        // Its ports where A's unconnected port dummies stand, right of a1's:
        // the way back in the further right.
        let mid = frame.x + frame.w / 2.0;
        assert!(
            lp[0].x > mid && end.x > lp[0].x && end.y < frame.y,
            "{lp:?}"
        );
        // A --> a1: under ELK a port dummy in A's first layer lines up with a1:
        // straight down from the frame's top edge into a1 (mermaid: 132,117 to
        // 132,161), not between a1 and a2.
        let into = edge_points(&scene, "L-A-a1-3");
        assert!((into[0].y - frame.y).abs() < eps, "{into:?}");
        let a1 = scene_node_center(&g, &scene, "a1");
        let end = *into.last().unwrap();
        assert!(
            (end.x - a1.x).abs() < 1.0 && end.y < a1.y,
            "{end:?} vs {a1:?}"
        );
        assert!(into.iter().all(|p| (p.x - a1.x).abs() < 1.0), "{into:?}");
        // a2 --> A: from a2 straight out to the frame's bottom edge (not a
        // self-loop on a2), through a dummy in A's last layer.
        let out = edge_points(&scene, "L-a2-A-4");
        let a2 = scene_node_center(&g, &scene, "a2");
        assert!((out[0].x - a2.x).abs() < 1.0 && out[0].y > a2.y, "{out:?}");
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
        // fc_edge_to_ancestor: x --> outer must not become a loop on x. Under
        // ELK outer's port counts the edge as incoming (x is no child of
        // outer): its dummy is in outer's first layer, so the edge leaves x
        // and inner along the flow and runs back to outer's left side (as in
        // mermaid's render: from x's right side to outer's left frame).
        let src = "flowchart LR\nsubgraph outer\nsubgraph inner\nx --> z\nend\nend\nx --> outer\nouter --> y";
        let (g, scene) = render_flowchart(src);
        let frame = frame_of(&scene, "outer");
        let inner = frame_of(&scene, "inner");
        let pts = edge_points(&scene, "L-x-outer-1");
        let x = scene_node_center(&g, &scene, "x");
        assert!(
            pts[0].x > x.x + 50.0 && (pts[0].y - x.y).abs() < 10.0,
            "{pts:?}"
        );
        assert!(pts.iter().any(|p| p.x > inner.right()), "{pts:?}");
        let end = *pts.last().unwrap();
        assert!((end.x - frame.x).abs() < 8.0, "{end:?} {frame:?}");
        assert!(end.y < inner.y, "{end:?} {inner:?}");
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

    #[test]
    fn labels_take_min_node_width_and_wrap_at_wrapping_width() {
        // mermaid 12 labelHelper: `minNodeWidth` 120 widens short labels, `wrappingWidth`
        // 120 wraps long ones; classic rect padding 2*30 gives 180 wide (reference SVG).
        let cfg = crate::config::MermaidConfig::default().scoped("flowchart");
        let mut fc = cfg.flowchart.clone();
        fc.look = ConfigLook::Classic;
        let pal = cfg.palette();
        let size = |fc: &GraphConfig, label: &str, shape: Shape| {
            node_size(
                &Node::new("n", label, shape),
                DiagramKind::Flowchart,
                fc,
                &pal,
                false,
            )
        };
        let short = size(&fc, "Hi", Shape::Rect);
        let long = size(&fc, "A much longer label here", Shape::Rect);
        assert_eq!(short.0, 180.0);
        assert_eq!(long.0, 180.0);
        assert!(long.1 > short.1, "the long label wraps onto a second line");
        // Circles size from both axes and opt out of the minimum width.
        assert!(size(&fc, "x", Shape::Circle).0 < 120.0);
        fc.min_node_width = 0.0;
        fc.wrapping_width = 0.0;
        assert!(size(&fc, "Hi", Shape::Rect).0 < 120.0);
    }

    #[test]
    fn edge_labels_wrap_at_200px() {
        let b = edge_label_block(
            "a really quite long relationship label that keeps going on",
            14.0,
        );
        // mermaid (14px): two lines in a 200px wide label.
        assert_eq!(b.lines.len(), 2, "{:?}", b.lines);
        assert!(b.width <= EDGE_LABEL_WRAP);
        assert_eq!(edge_label_block("has", 16.0).lines, ["has"]);
    }

    fn edge_paths(scene: &Scene) -> Vec<Vec<PathCmd>> {
        let Element::Group { children, .. } = &scene.elements[1] else {
            panic!("edgePaths second");
        };
        children
            .iter()
            .filter_map(|c| match c {
                Element::Group { children, .. } => match children.first() {
                    Some(Element::Shape {
                        geom: Geometry::Path(cmds),
                        ..
                    }) => Some(cmds.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    const FAN: &str = "flowchart TD\n A --> B & C & D\n B --> E\n C --> E\n D --> E\n A --> E";

    /// Under ELK (the default `layout`) every edge is a run of horizontal and vertical
    /// lines whose bends are radius-5 rounded corners (`generateRoundedPath`).
    #[test]
    fn elk_edges_are_orthogonal_with_rounded_corners() {
        let (_, scene) = render_flowchart(FAN);
        let mut corners = 0;
        for cmds in edge_paths(&scene) {
            let mut at = match cmds[0] {
                PathCmd::MoveTo(p) => p,
                ref c => panic!("starts with {c:?}"),
            };
            for c in &cmds[1..] {
                match *c {
                    PathCmd::LineTo(p) => {
                        assert!(
                            (p.x - at.x).abs() < 1e-6 || (p.y - at.y).abs() < 1e-6,
                            "diagonal line {at:?} -> {p:?} in {cmds:?}"
                        );
                        at = p;
                    }
                    PathCmd::CubicTo(c1, _, p) => {
                        // A quarter turn: the corner is cut 5 / sin(45°) back on each leg.
                        let corner = at.plus(c1.minus(at).scale(1.5));
                        assert!((corner.dist(at) - 5.0 * 2f64.sqrt()).abs() < 1e-6);
                        assert!((corner.dist(p) - 5.0 * 2f64.sqrt()).abs() < 1e-6);
                        corners += 1;
                        at = p;
                    }
                    _ => panic!("unexpected {c:?}"),
                }
            }
        }
        assert!(corners >= 4, "fan-out / fan-in edges bend");
    }

    /// `n1 -- lbl --> g0` where the cluster constraints contradict (`n2 --o n1` runs
    /// from inside g0 back to n1): the edge is ranked to its plain end deep inside
    /// g0 and cut at g0's border, and its label must stay on the drawn part instead
    /// of floating inside the cluster.
    #[test]
    fn elk_label_of_an_edge_to_a_cluster_stays_on_the_drawn_edge() {
        let (_, scene) = render_flowchart(
            "flowchart BT\nn0[(N0)]\nn1[/N1/]\nn2(N2)\nn3[(N3)]\nsubgraph g0\ndirection BT\n\
             n2\nn3\nend\nn2 --o n1\nn2 ==> n2\nn1 -- lbl --> g0\nn0 ==> n3\nn1 --- n0",
        );
        let label = scene
            .elements
            .iter()
            .find_map(|e| match e {
                Element::Group {
                    class: Some(c),
                    children,
                    ..
                } if c == "edgeLabels" => Some(children),
                _ => None,
            })
            .and_then(|ls| {
                ls.iter().find_map(|l| match l {
                    Element::Group { children, .. } => match children.first() {
                        Some(Element::Shape {
                            geom: Geometry::Rect { rect, .. },
                            ..
                        }) => Some(rect.center()),
                        _ => None,
                    },
                    _ => None,
                })
            })
            .expect("a label");
        let on_edge = edge_paths(&scene).iter().any(|cmds| {
            let mut at = Point::new(f64::NAN, f64::NAN);
            cmds.iter().any(|c| match *c {
                PathCmd::MoveTo(p) => {
                    at = p;
                    false
                }
                PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) => {
                    let hit = Rect::new(
                        at.x.min(p.x),
                        at.y.min(p.y),
                        (at.x - p.x).abs(),
                        (at.y - p.y).abs(),
                    )
                    .pad(1.0)
                    .contains(label);
                    at = p;
                    hit
                }
                _ => false,
            })
        });
        assert!(on_edge, "label at {label:?} is off its edge");
    }

    /// `n2 --> g0` (LR) out of g0's member n2, with nodes below, above and after n2
    /// in g0: the three routes tried first all crossed a node, and the first was
    /// drawn through it. The route now goes round n2's back through the lane on
    /// that side.
    #[test]
    fn member_to_own_cluster_edge_goes_round_blocked_sides() {
        let frame = Rect::new(0.0, 0.0, 500.0, 270.0);
        let inner = Rect::new(40.0, 110.0, 150.0, 50.0);
        let obstacles = [
            Rect::new(30.0, 200.0, 170.0, 40.0),
            Rect::new(60.0, 50.0, 80.0, 30.0),
            Rect::new(300.0, 120.0, 170.0, 45.0),
        ];
        for inner_is_target in [false, true] {
            let ne = NestedEdge {
                cluster: ClusterId(0),
                inner: Endpoint::Node(NodeId(0)),
                inner_is_target,
            };
            let dir = if inner_is_target {
                Direction::RightLeft
            } else {
                Direction::LeftRight
            };
            let (pts, _) = nested_route(ne, frame, inner, dir, 24.0, None, &obstacles);
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let seg = Rect::new(
                    a.x.min(b.x),
                    a.y.min(b.y),
                    (a.x - b.x).abs(),
                    (a.y - b.y).abs(),
                );
                for o in &obstacles {
                    assert!(!seg.intersects(o), "{dir:?}: {pts:?} crosses {o:?}");
                }
            }
        }
    }

    /// An edge from a cluster to its own member is drawn in the direction the layout
    /// really used inside the cluster: an ancestor's `direction` that an edge across
    /// its border cancelled does not apply, so the route comes in from the top (TB)
    /// instead of across the frame and through the member above (as LR would).
    #[test]
    fn cluster_to_member_edge_follows_the_layout_direction() {
        let (g, scene) = render_flowchart(
            "flowchart TB\nsubgraph S1\ndirection LR\nsubgraph S2\nb1 --> b2\nend\nend\nX --> b1\nS2 --> b2",
        );
        let ei = g
            .edges
            .iter()
            .position(|e| matches!(e.from, Endpoint::Cluster(_)))
            .unwrap();
        let frame = cluster_group(&scene, "S2")
            .iter()
            .rev()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    ..
                } => Some(*rect),
                _ => None,
            })
            .unwrap();
        let cmds = &edge_paths(&scene)[ei];
        let PathCmd::MoveTo(start) = cmds[0] else {
            panic!("{cmds:?}");
        };
        assert!(
            (start.y - frame.y).abs() < 1e-6,
            "{start:?} not on the top of {frame:?}"
        );
    }

    /// `layout: dagre` keeps mermaid's dagre rendering: the configured (basis) curve.
    #[test]
    fn dagre_layout_keeps_the_basis_curve() {
        let g = crate::diagram::flowchart::parse(FAN, None).unwrap();
        let cfg = GraphConfig {
            layout: LayoutAlgorithm::Dagre,
            ..crate::config::MermaidConfig::default().flowchart
        };
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let scene = render(&g, &cfg, &pal);
        let paths = edge_paths(&scene);
        // A basis spline: M, L, cubics, L.
        assert!(
            paths
                .iter()
                .all(|c| matches!(c[1], PathCmd::LineTo(_)) && matches!(c[2], PathCmd::CubicTo(..)))
        );
        let diagonal = paths.iter().any(|cmds| {
            cmds.windows(2).any(|w| match (&w[0], &w[1]) {
                (PathCmd::CubicTo(_, _, a), PathCmd::CubicTo(_, _, b)) => {
                    (a.x - b.x).abs() > 1.0 && (a.y - b.y).abs() > 1.0
                }
                _ => false,
            })
        });
        assert!(diagonal, "dagre's S curves are not right-angled");
    }

    /// ELK gives every edge end its own port, spread evenly over the node's side.
    #[test]
    fn elk_ports_spread_over_the_node_side() {
        let (_, scene) = render_flowchart(FAN);
        let starts: Vec<Point> = edge_paths(&scene)
            .iter()
            .filter_map(|c| match c[0] {
                PathCmd::MoveTo(p) => Some(p),
                _ => None,
            })
            .collect();
        // A's four outgoing edges leave its bottom side at four distinct ports.
        let y = starts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let mut xs: Vec<f64> = starts
            .iter()
            .filter(|p| (p.y - y).abs() < 1e-6)
            .map(|p| p.x)
            .collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert_eq!(xs.len(), 4, "{starts:?}");
        let gaps: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-6),
            "evenly spread: {gaps:?}"
        );
    }

    /// ELK's line hops (`elk.lineHops`): arcs by default, gaps on request, none when
    /// off or under dagre. Every edge of K3,3 crosses another.
    #[test]
    fn elk_line_hops_at_crossings() {
        use crate::linejump::LineHops;
        let g =
            crate::diagram::flowchart::parse("flowchart TD\n a1 & a2 & a3 --> b1 & b2 & b3", None)
                .unwrap();
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let base = crate::config::MermaidConfig::default().flowchart;
        let paths = |line_hops, layout| {
            let cfg = GraphConfig {
                line_hops,
                layout,
                ..base.clone()
            };
            edge_paths(&render(&g, &cfg, &pal))
        };
        let elk = LayoutAlgorithm::Elk;
        let (off, arc, gap) = (
            paths(LineHops::Off, elk),
            paths(LineHops::Arc, elk),
            paths(LineHops::Gap, elk),
        );
        let moves = |cmds: &[PathCmd]| {
            cmds.iter()
                .filter(|c| matches!(c, PathCmd::MoveTo(_)))
                .count()
        };
        // Plain paths are single runs; some edges gain arcs (two cubics each) ...
        assert!(off.iter().chain(&arc).all(|c| moves(c) == 1));
        let extra: usize = arc.iter().zip(&off).map(|(a, o)| a.len() - o.len()).sum();
        assert!(extra >= 3 && extra % 3 == 0, "{extra}");
        // ... or gaps (a line to the hop, a move past it), at the same crossings.
        let gaps: usize = gap.iter().map(|c| moves(c) - 1).sum();
        assert_eq!(gaps * 3, extra);
        // The crossings stay put: every path still starts and ends where it did.
        for (a, o) in arc.iter().zip(&off) {
            assert_eq!(a.first(), o.first());
            assert_eq!(a.last(), o.last());
        }
        let dagre = LayoutAlgorithm::Dagre;
        assert_eq!(paths(LineHops::Arc, dagre), paths(LineHops::Off, dagre));
    }

    /// mermaid's ELK adapter hands ELK the label's own bbox (`edge.width/height`),
    /// without the padding dagre's label node gets. Reference (mermaid 12.3.0,
    /// default theme, classic look): A→B centres 188 apart, C→D 94.
    #[test]
    fn elk_edge_labels_take_their_own_size() {
        let (g, scene) = render_flowchart("flowchart TD\nA -->|Get money| B\nC --> D");
        let gap = |a: &str, b: &str| {
            scene_node_center(&g, &scene, b).y - scene_node_center(&g, &scene, a).y
        };
        assert!((gap("A", "B") - 188.0).abs() < 1e-6, "{}", gap("A", "B"));
        assert!((gap("C", "D") - 94.0).abs() < 1e-6, "{}", gap("C", "D"));
    }

    /// `font-style: italic` (classDef / style) italicises an HTML node label.
    #[test]
    fn italic_font_style_italicises_html_labels() {
        assert_eq!(
            StyleOverrides::parse("font-style:italic").italic,
            Some(true)
        );
        assert_eq!(
            StyleOverrides::parse("font-style: normal").italic,
            Some(false)
        );
        let (_, scene) = render_flowchart(
            "flowchart LR\nA[Slanted]:::it\nB[Upright]\nclassDef it font-style:italic",
        );
        fn lines(els: &[Element], out: &mut Vec<String>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => lines(children, out),
                    Element::Text { text, .. } => out.extend(text.lines.iter().cloned()),
                    _ => {}
                }
            }
        }
        let mut all = Vec::new();
        lines(&scene.elements, &mut all);
        let runs = |s: &str| {
            let l = all.iter().find(|l| crate::text::plain(l) == s).unwrap();
            crate::text::styled_runs(l, false)
        };
        assert!(runs("Slanted").iter().all(|r| r.italic));
        assert!(runs("Upright").iter().all(|r| !r.italic));
    }

    /// mermaid's ELK adapter never reads `nodeSpacing` / `rankSpacing`; dagre does.
    #[test]
    fn elk_ignores_the_configured_spacing() {
        let g = crate::diagram::flowchart::parse(FAN, None).unwrap();
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let base = crate::config::MermaidConfig::default().flowchart;
        let wide = GraphConfig {
            node_spacing: 300.0,
            rank_spacing: 300.0,
            ..base.clone()
        };
        let size = |c: &GraphConfig| {
            let v = render(&g, c, &pal).view_box;
            (v.w, v.h)
        };
        assert_eq!(size(&base), size(&wide));
        let dagre = |c: &GraphConfig| GraphConfig {
            layout: LayoutAlgorithm::Dagre,
            ..c.clone()
        };
        let (a, b) = (size(&dagre(&base)), size(&dagre(&wide)));
        assert!(b.0 > a.0 + 100.0 && b.1 > a.1 + 100.0);
    }

    #[test]
    fn clipping_along_the_axis_meets_the_outline() {
        let c = Point::new(0.0, 0.0);
        // A diamond of half-size 50: the vertical through x = 20 leaves it at y = 30.
        let diamond = Boundary::Polygon(
            c,
            vec![
                Point::new(0.0, -50.0),
                Point::new(50.0, 0.0),
                Point::new(0.0, 50.0),
                Point::new(-50.0, 0.0),
            ],
        );
        let p = clip_along(&diamond, Point::new(20.0, 50.0), Point::new(20.0, 80.0));
        assert!(p.dist(Point::new(20.0, 30.0)) < 1e-6, "{p:?}");
        let circle = Boundary::Ellipse(c, 10.0, 10.0);
        let p = clip_along(&circle, Point::new(6.0, 10.0), Point::new(6.0, 30.0));
        assert!(p.dist(Point::new(6.0, 8.0)) < 1e-6, "{p:?}");
        let rect = Boundary::Rect(Rect::new(-10.0, -5.0, 20.0, 10.0));
        let p = clip_along(&rect, Point::new(4.0, 5.0), Point::new(4.0, 20.0));
        assert!(p.dist(Point::new(4.0, 5.0)) < 1e-6, "{p:?}");
        // A cylinder's port off its middle, at the bottom: mermaid bisects
        // with its `intersect` (the box's crossing moved by the cap) as the
        // inside test, which lands inside the box, where the ray through the
        // point meets that `intersect` (mermaid 12.3.0: 1.1px in).
        let cyl = Boundary::Cylinder(Rect::new(-50.0, -40.0, 100.0, 80.0), 10.0);
        let p = clip_along(&cyl, Point::new(30.0, 40.0), Point::new(30.0, 80.0));
        assert!(p.x == 30.0 && p.y < 40.0 && p.y > 30.0, "{p:?}");
        let on = cyl.clip(p).minus(c).len() - p.minus(c).len();
        assert!(on.abs() < 1e-3, "{p:?} {on}");
        // Straight down its middle the port is on the cap.
        let p = clip_along(&cyl, Point::new(0.0, 40.0), Point::new(0.0, 80.0));
        assert!(p.dist(Point::new(0.0, 40.0)) < 1e-3, "{p:?}");
    }

    #[test]
    fn elk_routes_keep_ports_beside_their_outline_points_as_mermaid_does() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 40.0);
        let port = Point::new(100.0, 20.0);
        // At the source a port on the outline goes as a duplicate; one off
        // it (or off only by rounding) stays, unless a start marker needs
        // the room.
        assert_eq!(kept_port(port, Some(port), 0.0, bounds, false), None);
        let inside = Point::new(95.0, 20.0);
        assert_eq!(
            kept_port(port, Some(inside), 0.0, bounds, false),
            Some(port)
        );
        assert_eq!(kept_port(port, None, 0.0, bounds, false), Some(port));
        assert_eq!(kept_port(port, Some(inside), 4.0, bounds, false), None);
        // At the target an outline point within 2px goes for the port.
        let near = Point::new(98.5, 20.0);
        assert_eq!(kept_port(port, Some(near), 0.0, bounds, true), None);
        assert_eq!(kept_port(port, Some(inside), 0.0, bounds, true), Some(port));
        // Those still in place go again after straightening.
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
        ];
        let kept = [Some(pts[1]), Some(Point::new(10.0, 5.0))];
        assert_eq!(
            without_kept_ports(pts.clone(), kept),
            vec![pts[0], pts[2], pts[3]]
        );
    }

    #[test]
    fn elk_target_end_keeps_a_port_within_two_pixels_of_the_outline() {
        // A 14px end circle entered 2px off its centre: the outline lies 0.29px
        // inside the box, so the edge ends on the box (the port); 6px off
        // centre it lies 3.4px in, and the edge ends on the circle. The source
        // end is always clipped.
        let circle = Boundary::Ellipse(Point::new(0.0, 0.0), 7.0, 7.0);
        let near = [Point::new(2.0, -7.0), Point::new(2.0, -40.0)];
        assert_eq!(clip_target_end(&circle, &near, true), near[0]);
        assert!(clip_end(&circle, &near, true).dist(Point::new(2.0, -45f64.sqrt())) < 1e-9);
        let far = [Point::new(6.0, -7.0), Point::new(6.0, -40.0)];
        let p = clip_target_end(&circle, &far, true);
        assert!(p.dist(Point::new(6.0, -13f64.sqrt())) < 1e-9, "{p:?}");
    }
}
