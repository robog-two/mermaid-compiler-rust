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
    /// Width of a label that `createText` wrapped: its `div` is given the whole
    /// wrapping width, however wide the longest line is.
    pub wrapped_width: Option<f64>,
    /// A class box's `«annotation»` (classBox draws only the first one), stacked
    /// above the name in normal weight.
    pub annotation: Option<String>,
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
    // above and below; an empty members box is GAP/2 + 2·GAP (2·GAP for empty
    // methods), and a class with neither gets two 1.5·GAP boxes.
    let all_empty = blocks.iter().skip(1).all(|b| b.is_empty());
    let annotation_h = annotation.map(content).unwrap_or(0.0);
    let heights: Vec<f64> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match (i, b.is_empty()) {
            (0, _) => annotation_h + content(b) + 2.0 * pad,
            (_, false) => content(b) + 2.0 * pad,
            _ if all_empty => 1.5 * pad,
            (1, true) => 2.5 * pad,
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
            .map(|b| {
                if b.svg || b.is_empty() {
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
    let mut block = TextBlock::label(&node.label, font);
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
        block = TextBlock::new(wrap_lines(&block.lines, wrap_width, font, bold), font, bold);
    } else if bold {
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
                        (false, _) => TextBlock::new(s.lines.clone(), font, s.bold),
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
                        TextBlock::new(lines, font, false)
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
}

/// Padding of an SVG edge label's background on each side (`createFormattedText`):
/// the label's bounding box, and so its size in the layout, includes it.
const SVG_EDGE_LABEL_PAD: f64 = 2.0;

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

    let mut cluster_titles: Vec<TextBlock> = g
        .clusters
        .iter()
        .map(|c| {
            let t = TextBlock::label(&c.title, font);
            if cfg.html_edge_labels {
                t
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
                .map(|l| {
                    if !cfg.html_edge_labels {
                        svg_edge_label_block(l, font)
                    } else if opts.markdown_no_wrap && crate::text::is_markdown_string(l) {
                        TextBlock::label(l, font)
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
                    // A cluster's edge to its own member is laid out as a loop but drawn
                    // against the frame: under ELK its label reserves no loop room.
                    label: lb
                        .as_ref()
                        .filter(|_| !(elk && nested[ei].is_some()))
                        .map(|b| {
                            // An SVG label's box includes its background's padding.
                            let pad = if b.svg { 2.0 * SVG_EDGE_LABEL_PAD } else { 0.0 };
                            if elk {
                                (b.width + pad, b.height + pad)
                            } else {
                                (b.width + pad + 8.0, b.height + pad + 4.0)
                            }
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
        node_sep: layer_spacing(cfg.layout, cfg.node_spacing),
        rank_sep: layer_spacing(cfg.layout, cfg.rank_spacing),
        edge_sep: 20.0,
        cluster_padding: CLUSTER_PADDING,
        inner_node_sep: CLUSTER_NODE_SPACING,
        inner_rank_sep: CLUSTER_RANK_SPACING,
        routing: edge_routing(cfg.layout),
    };
    // ER group titles wrap at the group's laid-out width. Under ELK mermaid lays
    // out once, with each title measured at the wrapping width, and wraps it again
    // at the frame's width when painting (below); under dagre a probe layout gives
    // the width to wrap at before the real one.
    let er_groups = g.kind == DiagramKind::Er;
    if er_groups && elk {
        for (ci, c) in g.clusters.iter().enumerate() {
            if empty_of[ci].is_some() || cluster_titles[ci].is_empty() {
                continue;
            }
            let t = wrapped_group_title(&c.title, opts.group_title_wrap, font);
            input.clusters[ci].title_height = cluster_title_height(c.kind, &t);
            input.clusters[ci].min_width = t.width + title_pad;
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
    for (ci, t) in cluster_titles.iter_mut().enumerate() {
        let r = &mut cluster_rects[ci];
        if let Some(p) = empty_of[ci] {
            *r = Rect::centered(out.nodes[p], measured[p].w, measured[p].h);
            continue;
        }
        if er_groups && elk && !t.is_empty() {
            *t = wrapped_group_title(&g.clusters[ci].title, r.w, font);
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
                t.centered_text(center, &color),
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
        }
    }

    // Every rendered edge's points, for ELK's line hops (`applyElkLineJumps`), with the
    // index in `edge_els` of its drawn group (none for an invisible edge, which still
    // takes part: upstream paints it with an invisible stroke).
    let mut hop_edges: Vec<(linejump::EdgeGeom, Option<usize>)> = Vec::new();
    // Every edge's clipped route and label position first: under ELK mermaid then
    // straightens the routes' terminal steps and separates opposite labels.
    let mut routed: Vec<Option<(Vec<Point>, Option<Point>)>> = vec![None; g.edges.len()];
    for (ei, (e, route)) in g.edges.iter().zip(&out.edges).enumerate() {
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
            if elk {
                pts.dedup_by(|a, b| a.dist(*b) < 1e-9);
            }
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
        if elk && is_loop && nested[ei].is_none() {
            // An orthogonal self-loop leaves and re-enters through ports on the node's side.
            if let (Endpoint::Node(NodeId(a)), Endpoint::Node(NodeId(b))) = (e.from, e.to) {
                let n = pts.len();
                pts[0] = clip_along(&boundaries[a], pts[0], pts[1]);
                pts[n - 1] = clip_along(&boundaries[b], pts[n - 1], pts[n - 2]);
            }
        }
        if !is_loop {
            // An edge to / from a whole cluster is cut where it crosses the cluster's
            // border (mermaid's `cutPathAtIntersect` for `fromCluster` / `toCluster`).
            match e.from {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0], elk),
                Endpoint::Node(NodeId(i)) => pts[0] = clip_end(&boundaries[i], &pts, elk),
            }
            pts.reverse();
            match e.to {
                Endpoint::Cluster(c) => pts = cut_at_rect(&pts, &cluster_rects[c.0], elk),
                Endpoint::Node(NodeId(i)) => pts[0] = clip_end(&boundaries[i], &pts, elk),
            }
            pts.reverse();
            if pts.len() < 2 {
                continue;
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
        elkedges::straighten_edge_terminals(&mut routes, &mut labels);
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
        let Some((mut pts, label_at)) = routed[ei].take() else {
            continue;
        };
        // A neo edge takes the `*-margin` markers (`useMargin`), the neo barb's
        // pull-back, and leaves its last `markerOffsets2` px undrawn.
        let neo = cfg.look == ConfigLook::Neo;
        let trim = |h: Head| markers::pullback_for(h, neo) + markers::hidden_end(h, neo);
        let (start_trim, end_trim) = (trim(e.start), trim(e.end));
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
            neo,
        ));
        heads.extend(markers::elements(
            e.start,
            End::Start,
            pts[0],
            start_dir,
            colors,
            width,
            neo,
        ));
        let (end_back, start_back) = (end_trim, start_trim);
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
            // The ELK adapter overrides every routed edge's curve with `rounded`.
            _ if elk => geom::rounded_path(&pts, EDGE_CORNER_RADIUS),
            Curve::Basis => geom::basis_curve(&pts),
            Curve::Linear => geom::polyline(&pts),
            Curve::Step => geom::polyline(&step_points(&pts, horizontal)),
        };
        let mut paint = Paint::stroke(&stroke, width);
        paint.dash = e.style.dash.clone().or_else(|| match e.line {
            // A neo edge writes its pattern inline (`generateDashArray`): 2px
            // dashes and gaps, dotted or dashed, outranking the stylesheet's.
            LineKind::Dotted | LineKind::Dashed if neo => Some(NEO_DASH.to_vec()),
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
                (
                    Rect::centered(at, block.width + 4.0, block.height),
                    Paint::fill(edge_label_bg),
                )
            };
            // The drawing's bbox holds an HTML label's `foreignObject`, which is the
            // text box itself: the 2px background margin stays outside it. (Kept
            // under dagre, whose output is pinned.)
            bounds.add_rect(if block.svg || !elk {
                r
            } else {
                Rect::centered(at, block.width, block.height)
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
        for (txt, anchor_pt, toward) in [
            (&e.start_label, pts[0], pts[1]),
            (&e.end_label, pts[last], pts[last - 1]),
        ] {
            if let Some(t) = txt.as_ref().filter(|_| !elk) {
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

    if elk {
        let (geoms, slots): (Vec<_>, Vec<_>) = hop_edges.into_iter().unzip();
        let hopped = linejump::line_hops(&geoms, linejump::ELK_JUMP_RADIUS, cfg.line_hops);
        for (cmds, slot) in hopped.into_iter().zip(slots) {
            if let (Some(cmds), Some(k)) = (cmds, slot) {
                set_edge_path(&mut edge_els[k], cmds);
            }
        }
    }

    // Title above everything.
    let mut title_els = Vec::new();
    let title = g.title.as_ref().filter(|t| !t.trim().is_empty());
    if let Some(t) = title.filter(|_| g.kind == DiagramKind::Er) {
        // `insertTitle`: an unstyled `<text>` (root font size) with its baseline at
        // `y = -titleTopMargin` in layout space, whose top margin is ELK's 12px
        // padding or dagre's 8px `marginy`; centred on the drawing's bbox.
        let content = bounds.rect();
        let margin = if elk { 12.0 } else { 8.0 };
        let baseline = content.y - margin - cfg.title_top_margin;
        let tw = crate::text::line_width(t, font, false);
        let ascent = (0.905 * font).round();
        let cx = content.x + content.w / 2.0;
        bounds.add_rect(Rect::new(
            cx - tw / 2.0,
            baseline - ascent,
            tw,
            crate::text::svg_line_box(font, t),
        ));
        let lines = vec![t.clone()];
        let center = Point::new(cx, baseline - font * 0.35);
        title_els.push(Element::classed_text(
            Text::centered(center, &lines, font, font, &pal.text_color),
            "erDiagramTitleText",
        ));
    } else if let Some(t) = title {
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
                // `.flowchartTitleText` (and the class / state / agentflow
                // equivalents): `fill: textColor`.
                &pal.text_color,
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

fn clip_end(b: &Boundary, pts: &[Point], elk: bool) -> Point {
    if elk {
        clip_along(b, pts[0], pts[1])
    } else {
        b.clip(pts[1])
    }
}

/// Where the line from `next` through `port` (a point on the node's bounding box)
/// meets the node's outline, coming from outside: `port` itself for a rectangle,
/// further in for a diamond, circle, ... (as the ELK adapter's `cutter2` clips the
/// first segment). `port` if the line misses the outline.
pub(crate) fn clip_along(b: &Boundary, port: Point, next: Point) -> Point {
    let center = match b {
        Boundary::Rect(r) => r.center(),
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
    let t = match b {
        Boundary::Rect(r) => {
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
const CLUSTER_PADDING: f64 = 24.0;
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
        input.clusters[ci].title_height = cluster_title_height(c.kind, &t);
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
    let line_paint = Paint::stroke(colors.stroke, 1.0);
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
                .any(|p| p.x > frame.right() + 10.0 && p.y < frame.y),
            "{lp:?} {frame:?}"
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
    }
}
