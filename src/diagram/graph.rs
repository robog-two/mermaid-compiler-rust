//! Graph IR shared by flowcharts, state diagrams and class diagrams, plus the
//! pure renderer `GraphDiagram -> Scene` (layout → shapes → edges → labels).

use crate::config::{Curve, GraphConfig};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::{self, Direction, LayoutCluster, LayoutEdge, LayoutInput, LayoutNode};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;

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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Solid,
    Dotted,
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
    /// Open `>` arrow (association / lollipop-free dependency).
    OpenArrow,
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

#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub id: String,
    pub title: String,
    pub parent: Option<ClusterId>,
    pub style: StyleOverrides,
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
    node_stroke_width: f64,
}

fn look(kind: DiagramKind) -> Look {
    match kind {
        DiagramKind::Flowchart => Look {
            edge_width: 2.0,
            thick_width: 3.5,
            dotted_dash: vec![3.0],
            node_stroke_width: 1.0,
        },
        _ => Look {
            edge_width: 1.0,
            thick_width: 2.5,
            dotted_dash: vec![3.0],
            node_stroke_width: 1.0,
        },
    }
}

/// Clipping boundary of a drawn node.
enum Boundary {
    Rect(Rect),
    Ellipse(Point, f64, f64),
    Polygon(Point, Vec<Point>),
}

impl Boundary {
    fn clip(&self, toward: Point) -> Point {
        match self {
            Boundary::Rect(r) => geom::clip_rect(r, toward),
            Boundary::Ellipse(c, rx, ry) => geom::clip_ellipse(*c, *rx, *ry, toward),
            Boundary::Polygon(c, pts) => geom::clip_polygon(*c, pts, toward),
        }
    }
}

struct Measured {
    block: TextBlock,
    sections: Vec<TextBlock>,
    w: f64,
    h: f64,
}

fn measure_node(node: &Node, pad: f64, font: f64, horizontal: bool) -> Measured {
    let mut block = TextBlock::label(&node.label, font);
    if node.style.bold == Some(true) {
        block = TextBlock::new(block.lines, font, true);
    }
    let (tw, th) = (
        block.width,
        block
            .height
            .max(if block.is_empty() { 0.0 } else { font * 1.5 }),
    );
    let mut sections = Vec::new();
    let (w, h) = match node.shape {
        Shape::Rect | Shape::Rounded | Shape::Note => (tw + 2.0 * pad, th + 2.0 * pad),
        Shape::Stadium => (tw + th + 2.0 * pad, th + 2.0 * pad),
        Shape::Subroutine => (tw + 2.0 * pad + 16.0, th + 2.0 * pad),
        Shape::Cylinder => {
            let w = tw + 2.0 * pad;
            let ry = w / 2.0 / (2.5 + w / 50.0);
            (w, th + 2.0 * pad + 2.0 * ry)
        }
        Shape::Circle => {
            let d = tw.max(th) + pad;
            (d, d)
        }
        Shape::DoubleCircle => {
            let d = tw.max(th) + pad + 10.0;
            (d, d)
        }
        Shape::Asymmetric => {
            let h = th + 2.0 * pad;
            (tw + 2.0 * pad + h / 4.0, h)
        }
        Shape::Diamond => {
            let s = tw + th + 2.0 * pad;
            (s, s)
        }
        Shape::Hexagon => {
            let h = th + 2.0 * pad;
            (tw + 2.0 * pad + h / 2.0, h)
        }
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid | Shape::InvTrapezoid => {
            let h = th + 2.0 * pad;
            (tw + 2.0 * pad + 2.0 * h / 3.0, h)
        }
        Shape::StateStart | Shape::StateEnd => (14.0, 14.0),
        Shape::ForkBar => {
            if horizontal {
                (8.0, 70.0)
            } else {
                (70.0, 8.0)
            }
        }
        Shape::Choice => (28.0, 28.0),
        Shape::Compartments => {
            sections = node
                .sections
                .iter()
                .map(|s| TextBlock::new(s.lines.clone(), font, s.bold))
                .collect();
            let w = sections.iter().map(|b| b.width).fold(tw, f64::max) + 2.0 * pad;
            let h: f64 = sections
                .iter()
                .map(|b| b.height.max(font * 0.5) + pad)
                .sum::<f64>()
                .max(th + pad);
            (w.max(60.0), h.max(30.0))
        }
        Shape::Invisible => (tw.max(20.0), th.max(10.0)),
        Shape::EntityTable => {
            let t = table_metrics(node, pad, font);
            (t.w, t.h)
        }
        Shape::Requirement => {
            let r = requirement_metrics(node, pad, font);
            (r.w, r.h)
        }
    };
    Measured {
        block,
        sections,
        w,
        h,
    }
}

/// Pure renderer for graph diagrams.
pub fn render(g: &GraphDiagram, cfg: &GraphConfig, pal: &Palette) -> Scene {
    let lk = look(g.kind);
    let font = pal.font_size;
    let horizontal = g.direction.is_horizontal();

    // Placeholder nodes for clusters with no member nodes, so they still get a box.
    let mut nodes = g.nodes.clone();
    for (ci, c) in g.clusters.iter().enumerate() {
        let has_member = nodes.iter().any(|n| {
            n.cluster
                .map(|nc| is_within(&g.clusters, nc, ClusterId(ci)))
                .unwrap_or(false)
        });
        if !has_member {
            let mut placeholder = Node::new(format!("__empty_{}", c.id), "", Shape::Invisible);
            placeholder.cluster = Some(ClusterId(ci));
            nodes.push(placeholder);
        }
    }

    let measured: Vec<Measured> = nodes
        .iter()
        .map(|n| measure_node(n, cfg.padding, font, horizontal))
        .collect();
    let cluster_titles: Vec<TextBlock> = g
        .clusters
        .iter()
        .map(|c| TextBlock::label(&c.title, font))
        .collect();

    let representative = |c: ClusterId| -> usize {
        nodes
            .iter()
            .position(|n| {
                n.cluster
                    .map(|nc| is_within(&g.clusters, nc, c))
                    .unwrap_or(false)
            })
            .unwrap_or(0)
    };
    let resolve = |e: Endpoint| match e {
        Endpoint::Node(NodeId(i)) => i,
        Endpoint::Cluster(c) => representative(c),
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

    let input = LayoutInput {
        nodes: nodes
            .iter()
            .zip(&measured)
            .map(|(n, m)| LayoutNode {
                width: m.w,
                height: m.h,
                cluster: n.cluster.map(|c| c.0),
            })
            .collect(),
        edges: g
            .edges
            .iter()
            .zip(&edge_labels)
            .map(|(e, lb)| LayoutEdge {
                from: resolve(e.from),
                to: resolve(e.to),
                minlen: e.minlen,
                label: lb.as_ref().map(|b| (b.width + 8.0, b.height + 4.0)),
            })
            .collect(),
        clusters: g
            .clusters
            .iter()
            .zip(&cluster_titles)
            .map(|(c, t)| LayoutCluster {
                parent: c.parent.map(|p| p.0),
                title_height: if t.is_empty() { 0.0 } else { t.height + 4.0 },
            })
            .collect(),
        direction: g.direction,
        node_sep: cfg.node_spacing,
        rank_sep: cfg.rank_spacing,
        edge_sep: 20.0,
        cluster_padding: 8.0,
    };
    let out = layout::layout(&input);

    let mut bounds = Bounds::default();
    let mut cluster_els = Vec::new();
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    let mut node_els = Vec::new();

    // Clusters, outermost first.
    let mut cluster_rects = out.clusters.clone();
    for (ci, t) in cluster_titles.iter().enumerate() {
        let r = &mut cluster_rects[ci];
        let need = t.width + 16.0;
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
        let fill = c
            .style
            .fill
            .clone()
            .unwrap_or_else(|| pal.cluster_bkg.clone());
        let stroke = c
            .style
            .stroke
            .clone()
            .unwrap_or_else(|| pal.cluster_border.clone());
        let mut paint = Paint::both(&fill, &stroke, c.style.stroke_width.unwrap_or(1.0));
        paint.dash = c.style.dash.clone();
        let rx = if g.kind == DiagramKind::State {
            5.0
        } else {
            0.0
        };
        let mut children = vec![Element::rect(r, rx, paint)];
        let t = &cluster_titles[ci];
        if !t.is_empty() {
            let color = c
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.title_color.clone());
            let center = Point::new(r.x + r.w / 2.0, r.y + 4.0 + t.height / 2.0);
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
        let (els, boundary) = node_elements(n, m, c, pal, &lk, cfg.padding, font);
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
        if pts.len() < 2 {
            continue;
        }
        let clip_end = |ep: Endpoint, toward: Point| -> Point {
            match ep {
                Endpoint::Node(NodeId(i)) => boundaries[i].clip(toward),
                Endpoint::Cluster(c) => geom::clip_rect(&cluster_rects[c.0], toward),
            }
        };
        let is_loop = resolve(e.from) == resolve(e.to);
        if !is_loop {
            // Cluster endpoints start from the cluster box rather than the representative.
            if let Endpoint::Cluster(c) = e.from {
                pts[0] = cluster_rects[c.0].center();
            }
            if let Endpoint::Cluster(c) = e.to {
                let last = pts.len() - 1;
                pts[last] = cluster_rects[c.0].center();
            }
            pts[0] = clip_end(e.from, pts[1]);
            let last = pts.len() - 1;
            pts[last] = clip_end(e.to, pts[last - 1]);
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
        let (end_els, end_back) =
            head_elements(e.end, pts[last], end_dir, &stroke, &pal.background, width);
        heads.extend(end_els);
        let start_dir = pts[0].minus(pts[1]).norm();
        let (start_els, start_back) =
            head_elements(e.start, pts[0], start_dir, &stroke, &pal.background, width);
        heads.extend(start_els);
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
        paint.dash = e
            .style
            .dash
            .clone()
            .or_else(|| (e.line == LineKind::Dotted).then(|| lk.dotted_dash.clone()));
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
        if let (Some(block), Some(at)) = (&edge_labels[ei], route.label) {
            let r = Rect::centered(at, block.width + 4.0, block.height);
            bounds.add_rect(r);
            label_els.push(Element::group(
                "edgeLabel",
                vec![
                    Element::rect(r, 0.0, Paint::fill(&pal.edge_label_background)),
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
            "flowchartTitleText",
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

fn node_prefix(kind: DiagramKind) -> &'static str {
    match kind {
        DiagramKind::State => "state",
        DiagramKind::Class => "classId",
        DiagramKind::Er => "entity",
        DiagramKind::Requirement => "requirement",
        _ => "flowchart",
    }
}

fn endpoint_id(g: &GraphDiagram, e: Endpoint) -> String {
    match e {
        Endpoint::Node(NodeId(i)) => g.nodes[i].id.clone(),
        Endpoint::Cluster(ClusterId(c)) => g.clusters[c].id.clone(),
    }
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

/// Arrowhead geometry with its tip at `tip`, pointing along `dir`.
/// Returns the elements and how far the line should be pulled back.
fn head_elements(
    head: Head,
    tip: Point,
    dir: Point,
    stroke: &Color,
    bg: &Color,
    width: f64,
) -> (Vec<Element>, f64) {
    let perp = Point::new(-dir.y, dir.x);
    let at = |back: f64, side: f64| tip.minus(dir.scale(back)).plus(perp.scale(side));
    match head {
        Head::None => (vec![], 0.0),
        Head::Arrow => {
            let (len, hw) = (8.0 + width, 4.0 + width / 2.0);
            (
                vec![Element::classed_shape(
                    Geometry::Path(geom::polygon(&[tip, at(len, hw), at(len, -hw)])),
                    Paint::fill(stroke),
                    "arrowheadPath",
                )],
                len * 0.8,
            )
        }
        Head::Circle => {
            let r = 4.5;
            (
                vec![Element::shape(
                    Geometry::Circle { c: at(r, 0.0), r },
                    Paint::both(stroke, stroke, 1.0),
                )],
                2.0 * r,
            )
        }
        Head::Cross => {
            let s = 4.5;
            let c = at(s, 0.0);
            let p = Paint::stroke(stroke, 2.0);
            let d1 = dir.plus(perp).scale(s / 2f64.sqrt());
            let d2 = dir.minus(perp).scale(s / 2f64.sqrt());
            (
                vec![
                    Element::line(c.minus(d1), c.plus(d1), p.clone()),
                    Element::line(c.minus(d2), c.plus(d2), p),
                ],
                s,
            )
        }
        Head::Triangle => {
            let (len, hw) = (16.0, 8.0);
            (
                vec![Element::shape(
                    Geometry::Path(geom::polygon(&[tip, at(len, hw), at(len, -hw)])),
                    Paint::both(bg, stroke, 1.0),
                )],
                len,
            )
        }
        Head::Diamond | Head::DiamondFilled => {
            let (len, hw) = (18.0, 6.0);
            let fill = if head == Head::DiamondFilled {
                stroke
            } else {
                bg
            };
            (
                vec![Element::shape(
                    Geometry::Path(geom::polygon(&[
                        tip,
                        at(len / 2.0, hw),
                        at(len, 0.0),
                        at(len / 2.0, -hw),
                    ])),
                    Paint::both(fill, stroke, 1.0),
                )],
                len,
            )
        }
        Head::OpenArrow => {
            let (len, hw) = (10.0, 6.0);
            let cmds = vec![
                PathCmd::MoveTo(at(len, hw)),
                PathCmd::LineTo(tip),
                PathCmd::LineTo(at(len, -hw)),
            ];
            (vec![Element::path(cmds, Paint::stroke(stroke, 1.0))], 0.0)
        }
        Head::ErOnlyOne | Head::ErZeroOrOne | Head::ErOneOrMore | Head::ErZeroOrMore => {
            let hw = 8.0;
            let p = Paint::stroke(stroke, width.max(1.0));
            let bar = |back: f64| Element::line(at(back, hw), at(back, -hw), p.clone());
            let foot = |apex: f64| {
                Element::path(
                    vec![
                        PathCmd::MoveTo(at(0.0, hw)),
                        PathCmd::LineTo(at(apex, 0.0)),
                        PathCmd::LineTo(at(0.0, -hw)),
                    ],
                    p.clone(),
                )
            };
            let ring = |back: f64| {
                Element::shape(
                    Geometry::Circle {
                        c: at(back, 0.0),
                        r: 5.0,
                    },
                    Paint::both(bg, stroke, width.max(1.0)),
                )
            };
            let els = match head {
                Head::ErOnlyOne => vec![bar(8.0), bar(14.0)],
                Head::ErZeroOrOne => vec![bar(8.0), ring(19.0)],
                Head::ErOneOrMore => vec![foot(15.0), bar(19.0)],
                _ => vec![foot(15.0), ring(25.0)],
            };
            (els, 0.0)
        }
        Head::CirclePlus => {
            let r = 7.0;
            let c = at(r, 0.0);
            let p = Paint::stroke(stroke, 1.0);
            (
                vec![
                    Element::shape(Geometry::Circle { c, r }, Paint::both(bg, stroke, 1.0)),
                    Element::line(c.minus(dir.scale(r)), c.plus(dir.scale(r)), p.clone()),
                    Element::line(c.minus(perp.scale(r)), c.plus(perp.scale(r)), p),
                ],
                2.0 * r,
            )
        }
    }
}

/// Column geometry of a `Shape::EntityTable` node.
struct TableMetrics {
    cols: Vec<f64>,
    header_h: f64,
    row_h: f64,
    w: f64,
    h: f64,
}

/// Horizontal padding inside an entity-table cell.
const CELL_PAD: f64 = 8.0;

fn table_metrics(node: &Node, pad: f64, font: f64) -> TableMetrics {
    let ncols = node
        .sections
        .iter()
        .map(|s| s.lines.len())
        .max()
        .unwrap_or(0);
    let mut cols = vec![0.0f64; ncols];
    for row in &node.sections {
        for (j, cell) in row.lines.iter().enumerate() {
            if !cell.is_empty() {
                cols[j] = cols[j].max(crate::text::line_width(cell, font, false) + 2.0 * CELL_PAD);
            }
        }
    }
    let label = TextBlock::label(&node.label, font);
    let line_h = font * 1.5;
    let header_h = label.height.max(line_h) + pad;
    let row_h = line_h + 6.0;
    let header_w = label.width + 2.0 * pad;
    let sum: f64 = cols.iter().sum();
    let w = sum.max(header_w).max(100.0);
    let used = cols.iter().filter(|c| **c > 0.0).count();
    if used > 0 && w > sum {
        let extra = (w - sum) / used as f64;
        for c in cols.iter_mut().filter(|c| **c > 0.0) {
            *c += extra;
        }
    }
    let h = if node.sections.is_empty() {
        (label.height + 2.0 * pad).max(60.0)
    } else {
        header_h + row_h * node.sections.len() as f64
    };
    TableMetrics {
        cols,
        header_h,
        row_h,
        w,
        h,
    }
}

/// Text blocks and size of a `Shape::Requirement` node.
struct RequirementMetrics {
    stereo: TextBlock,
    name: TextBlock,
    body: TextBlock,
    w: f64,
    h: f64,
}

fn requirement_metrics(node: &Node, pad: f64, font: f64) -> RequirementMetrics {
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
    let w = stereo.width.max(name.width).max(body.width) + 2.0 * pad;
    let mut h = pad + stereo.height + name.height + pad;
    if !body.is_empty() {
        h += body.height + pad / 2.0;
    }
    RequirementMetrics {
        stereo,
        name,
        body,
        w: w.max(120.0),
        h,
    }
}

fn node_elements(
    n: &Node,
    m: &Measured,
    c: Point,
    pal: &Palette,
    lk: &Look,
    pad: f64,
    font: f64,
) -> (Vec<Element>, Boundary) {
    let (w, h) = (m.w, m.h);
    let r = Rect::centered(c, w, h);
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
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
    let poly = |pts: Vec<Point>, paint: &Paint| {
        (
            Element::classed_shape(
                Geometry::Path(geom::polygon(&pts)),
                paint.clone(),
                "label-container",
            ),
            Boundary::Polygon(c, pts),
        )
    };

    let mut els = Vec::new();
    let boundary = match n.shape {
        Shape::Rect | Shape::Note | Shape::Invisible => {
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 0.0 },
                paint.clone(),
                "basic label-container",
            ));
            Boundary::Rect(r)
        }
        Shape::Rounded => {
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 5.0 },
                paint.clone(),
                "basic label-container",
            ));
            Boundary::Rect(r)
        }
        Shape::Stadium => {
            els.push(Element::classed_shape(
                Geometry::Rect {
                    rect: r,
                    rx: h / 2.0,
                },
                paint.clone(),
                "basic label-container",
            ));
            Boundary::Rect(r)
        }
        Shape::Subroutine => {
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 0.0 },
                paint.clone(),
                "basic label-container",
            ));
            let lp = Paint::stroke(&stroke, paint.stroke_width);
            els.push(Element::line(
                Point::new(x0 + 8.0, y0),
                Point::new(x0 + 8.0, y1),
                lp.clone(),
            ));
            els.push(Element::line(
                Point::new(x1 - 8.0, y0),
                Point::new(x1 - 8.0, y1),
                lp,
            ));
            Boundary::Rect(r)
        }
        Shape::Cylinder => {
            let rx = w / 2.0;
            let ry = rx / (2.5 + w / 50.0);
            let pi = std::f64::consts::PI;
            let top_c = Point::new(c.x, y0 + ry);
            let bot_c = Point::new(c.x, y1 - ry);
            let mut body = vec![
                PathCmd::MoveTo(Point::new(x0, y0 + ry)),
                PathCmd::LineTo(Point::new(x0, y1 - ry)),
            ];
            body.extend(geom::elliptic_arc_cubics(bot_c, rx, ry, pi, 0.0));
            body.push(PathCmd::LineTo(Point::new(x1, y0 + ry)));
            body.extend(geom::elliptic_arc_cubics(top_c, rx, ry, 0.0, -pi));
            body.push(PathCmd::Close);
            els.push(Element::classed_shape(
                Geometry::Path(body),
                paint.clone(),
                "basic label-container",
            ));
            let mut front = vec![PathCmd::MoveTo(Point::new(x0, y0 + ry))];
            front.extend(geom::elliptic_arc_cubics(top_c, rx, ry, pi, 0.0));
            els.push(Element::path(
                front,
                Paint::stroke(&stroke, paint.stroke_width),
            ));
            Boundary::Rect(r)
        }
        Shape::Circle => {
            els.push(Element::classed_shape(
                Geometry::Circle { c, r: w / 2.0 },
                paint.clone(),
                "basic label-container",
            ));
            Boundary::Ellipse(c, w / 2.0, h / 2.0)
        }
        Shape::DoubleCircle => {
            els.push(Element::classed_shape(
                Geometry::Circle { c, r: w / 2.0 },
                paint.clone(),
                "outer-circle",
            ));
            els.push(Element::classed_shape(
                Geometry::Circle {
                    c,
                    r: w / 2.0 - 5.0,
                },
                paint.clone(),
                "inner-circle",
            ));
            Boundary::Ellipse(c, w / 2.0, h / 2.0)
        }
        Shape::Asymmetric => {
            let (e, b) = poly(
                vec![
                    Point::new(x0, y0),
                    Point::new(x1, y0),
                    Point::new(x1, y1),
                    Point::new(x0, y1),
                    Point::new(x0 + h / 4.0, c.y),
                ],
                &paint,
            );
            els.push(e);
            b
        }
        Shape::Diamond | Shape::Choice => {
            let (e, b) = poly(
                vec![
                    Point::new(c.x, y0),
                    Point::new(x1, c.y),
                    Point::new(c.x, y1),
                    Point::new(x0, c.y),
                ],
                &paint,
            );
            els.push(e);
            b
        }
        Shape::Hexagon => {
            let m4 = h / 4.0;
            let (e, b) = poly(
                vec![
                    Point::new(x0 + m4, y0),
                    Point::new(x1 - m4, y0),
                    Point::new(x1, c.y),
                    Point::new(x1 - m4, y1),
                    Point::new(x0 + m4, y1),
                    Point::new(x0, c.y),
                ],
                &paint,
            );
            els.push(e);
            b
        }
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid | Shape::InvTrapezoid => {
            let s = h / 3.0;
            let pts = match n.shape {
                Shape::LeanRight => vec![
                    Point::new(x0 + s, y0),
                    Point::new(x1, y0),
                    Point::new(x1 - s, y1),
                    Point::new(x0, y1),
                ],
                Shape::LeanLeft => vec![
                    Point::new(x0, y0),
                    Point::new(x1 - s, y0),
                    Point::new(x1, y1),
                    Point::new(x0 + s, y1),
                ],
                Shape::Trapezoid => vec![
                    Point::new(x0 + s, y0),
                    Point::new(x1 - s, y0),
                    Point::new(x1, y1),
                    Point::new(x0, y1),
                ],
                _ => vec![
                    Point::new(x0, y0),
                    Point::new(x1, y0),
                    Point::new(x1 - s, y1),
                    Point::new(x0 + s, y1),
                ],
            };
            let (e, b) = poly(pts, &paint);
            els.push(e);
            b
        }
        Shape::StateStart => {
            els.push(Element::shape(
                Geometry::Circle { c, r: 7.0 },
                Paint::both(&fill, &stroke, 1.0),
            ));
            Boundary::Ellipse(c, 7.0, 7.0)
        }
        Shape::StateEnd => {
            els.push(Element::shape(
                Geometry::Circle { c, r: 7.0 },
                Paint::both(&pal.background, &stroke, 1.5),
            ));
            els.push(Element::shape(
                Geometry::Circle { c, r: 4.0 },
                Paint::fill(&stroke),
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
            els.push(Element::classed_shape(
                Geometry::Rect {
                    rect: r,
                    rx: if lk.edge_width > 1.5 { 0.0 } else { 3.0 },
                },
                paint.clone(),
                "basic label-container",
            ));
            let mut y = y0;
            for (i, (sec, block)) in n.sections.iter().zip(&m.sections).enumerate() {
                let sh = block.height.max(font * 0.5) + pad;
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
                    let center = Point::new(c.x, y + sh / 2.0);
                    let mut t =
                        Text::centered(center, &block.lines, font, block.line_height, &text_color);
                    t.bold = sec.bold;
                    if !sec.centered {
                        t = t.anchored(Anchor::Start, x0 + pad);
                    }
                    els.push(Element::classed_text(t, "label"));
                }
                y += sh;
            }
            return (els, Boundary::Rect(r));
        }
        Shape::EntityTable => {
            let t = table_metrics(n, pad, font);
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 0.0 },
                paint.clone(),
                "basic label-container",
            ));
            let header_center = if n.sections.is_empty() {
                c
            } else {
                Point::new(c.x, y0 + t.header_h / 2.0)
            };
            els.push(Element::classed_text(
                Text::centered(
                    header_center,
                    &m.block.lines,
                    font,
                    m.block.line_height,
                    &text_color,
                ),
                "nodeLabel",
            ));
            let line_paint = Paint::stroke(&stroke, 1.0);
            let odd = pal.background.clone();
            for (i, row) in n.sections.iter().enumerate() {
                let ry = y0 + t.header_h + t.row_h * i as f64;
                if i % 2 == 0 {
                    els.push(Element::classed_shape(
                        Geometry::Rect {
                            rect: Rect::new(x0, ry, w, t.row_h),
                            rx: 0.0,
                        },
                        Paint::fill(&odd),
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
                            &text_color,
                        )
                        .anchored(Anchor::Start, x + CELL_PAD);
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
                    Paint::stroke(&stroke, paint.stroke_width),
                ));
            }
            return (els, Boundary::Rect(r));
        }
        Shape::Requirement => {
            let rm = requirement_metrics(n, pad, font);
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 0.0 },
                paint.clone(),
                "basic label-container",
            ));
            let mut y = y0 + pad;
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
                y += pad / 2.0;
                els.push(Element::classed_shape(
                    Geometry::Path(vec![
                        PathCmd::MoveTo(Point::new(x0, y)),
                        PathCmd::LineTo(Point::new(x1, y)),
                    ]),
                    Paint::stroke(&stroke, 1.0),
                    "divider",
                ));
                y += pad / 2.0;
                let centered = n.sections.get(1).map(|s| s.centered).unwrap_or(true);
                let mut t = Text::centered(
                    Point::new(c.x, y + rm.body.height / 2.0),
                    &rm.body.lines,
                    font,
                    rm.body.line_height,
                    &text_color,
                );
                if !centered {
                    t = t.anchored(Anchor::Start, x0 + pad);
                }
                els.push(Element::classed_text(t, "label"));
            }
            return (els, Boundary::Rect(r));
        }
    };
    if !m.block.is_empty()
        && !matches!(
            n.shape,
            Shape::StateStart | Shape::StateEnd | Shape::ForkBar | Shape::Choice | Shape::Invisible
        )
    {
        let mut t = Text::centered(c, &m.block.lines, font, m.block.line_height, &text_color);
        t.bold = m.block.bold;
        els.push(Element::classed_text(t, "nodeLabel"));
    }
    (els, boundary)
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
}
