//! `mindmap` diagrams.
//!
//! Syntax: an indentation-based tree, one node per line. Node forms:
//! `text` (default), `id[square]`, `id(rounded)`, `id((circle))`,
//! `id))bang((`, `id)cloud(`, `id{{hexagon}}`; labels may be `"quoted"` or
//! markdown strings (`` "`**bold**`" ``). `::icon(fa fa-book)` and
//! `:::class1 class2` on their own line decorate the previous node. `::icon()`
//! is accepted but not drawn, as mermaid 12.3's renderer does not draw it either;
//! `fa:fa-book` in a label is an inline icon ([`crate::icons`]).
//!
//! Layout: mermaid 12.3's own, cytoscape's CoSE-Bilkent force-directed layout
//! (see [`crate::layout::force::cose`]). For a tree it starts from a
//! deterministic radial placement, so this reproduces mermaid's node positions.

use super::graph::Shape;
use super::hand::{self, HandNode, HandStyle};
use super::shapes;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::force::cose;
use crate::scene::{Color, DiagramKind, Element, Geometry, Gradient, Paint, Scene, Shadow, Text};
use crate::text::{BreakRule, TextBlock, line_width};
use crate::theme::Palette;

mod dom;

/// Node shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Default,
    Square,
    Rounded,
    Circle,
    Bang,
    Cloud,
    Hexagon,
}

/// One mindmap node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub icon: Option<String>,
    pub classes: Vec<String>,
    pub level: usize,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

/// Parsed mindmap: nodes in document order, `nodes[0]` is the root.
#[derive(Debug, Clone, PartialEq)]
pub struct Mindmap {
    pub title: Option<String>,
    pub nodes: Vec<Node>,
}

/// Parse a mindmap body.
pub fn parse(body: &str, title: Option<String>) -> Result<Mindmap> {
    let mut mm = Mindmap {
        title,
        nodes: Vec::new(),
    };
    let mut header = false;
    for (line_no, raw) in logical_lines(body) {
        let raw = raw.as_str();
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if !header {
            let rest = t
                .strip_prefix("mindmap")
                .ok_or_else(|| Error::parse(line_no, "Expecting 'mindmap'"))?;
            header = true;
            if !rest.trim().is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("Unexpected text after 'mindmap': {}", rest.trim()),
                ));
            }
            continue;
        }
        let indent: usize = raw
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();
        if let Some(rest) = t.strip_prefix("::icon(") {
            let icon = rest
                .strip_suffix(')')
                .ok_or_else(|| Error::parse(line_no, format!("{t}\nExpecting ')'")))?;
            let last = mm
                .nodes
                .last_mut()
                .ok_or_else(|| Error::parse(line_no, "An icon must follow a node"))?;
            last.icon = Some(icon.trim().to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix(":::") {
            let last = mm
                .nodes
                .last_mut()
                .ok_or_else(|| Error::parse(line_no, "A class must follow a node"))?;
            last.classes
                .extend(rest.split_whitespace().map(str::to_string));
            continue;
        }
        // mindmap.jison lexes `:::` as a CLASS token only where a token may start: after
        // a shape's closing delimiter that is a syntax error, while in a bare id it is
        // just part of the NODE_ID text (`a:::urgent` is a node labelled so).
        if let Some(i) = find_unquoted(t, ":::") {
            let before = t[..i].trim_end();
            if !before.is_empty()
                && matches!(parse_node(before, line_no), Ok((_, _, shape)) if shape != NodeShape::Default)
            {
                return Err(Error::parse(
                    line_no,
                    "Expecting 'SPACELINE', 'NL', 'EOF', got 'CLASS'",
                ));
            }
        }
        // mindmap.jison (12.3) has no `@{ ... }` shape data: the `@{` of a node id is
        // unrecognised text.
        if let Some(at) = t.find("@{") {
            if !t[..at].contains(['[', '(', '{', ')']) {
                return Err(Error::parse(
                    line_no,
                    format!("Lexical error on line {line_no}. Unrecognized text.\n{t}"),
                ));
            }
        }
        let (id, label, shape) = parse_node(t, line_no)?;
        let parent = if mm.nodes.is_empty() {
            None
        } else {
            let p = mm.nodes.iter().rposition(|n| n.level < indent);
            if p.is_none() {
                return Err(Error::parse(
                    line_no,
                    format!(
                        "There can be only one root. No parent could be found for (\"{label}\")"
                    ),
                ));
            }
            p
        };
        let idx = mm.nodes.len();
        if let Some(p) = parent {
            mm.nodes[p].children.push(idx);
        }
        mm.nodes.push(Node {
            id,
            label,
            shape,
            icon: None,
            classes: Vec::new(),
            level: indent,
            children: Vec::new(),
            parent,
        });
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'mindmap'"));
    }
    if mm.nodes.is_empty() {
        return Err(Error::parse(
            body.lines().count().max(1),
            "A mindmap needs a root node",
        ));
    }
    Ok(mm)
}

/// The body's lines with each `"string"` (plain or markdown) that runs over line
/// ends joined into one line, as mindmap.jison's `NSTR` / `NSTR2` states match
/// newlines. Each line keeps its 1-based starting line number. A string never
/// closed is left to fail on its own line.
fn logical_lines(body: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut cur = lines[i].to_string();
        let mut j = i;
        while cur.matches('"').count() % 2 == 1 && j + 1 < lines.len() {
            j += 1;
            cur.push('\n');
            cur.push_str(lines[j]);
        }
        if cur.matches('"').count() % 2 == 1 {
            j = i;
            cur = lines[i].to_string();
        }
        out.push((i + 1, cur));
        i = j + 1;
    }
    out
}

const DELIMS: [(&str, &str, NodeShape); 7] = [
    ("((", "))", NodeShape::Circle),
    ("))", "((", NodeShape::Bang),
    ("{{", "}}", NodeShape::Hexagon),
    ("(", ")", NodeShape::Rounded),
    (")", "(", NodeShape::Cloud),
    ("[", "]", NodeShape::Square),
    ("(-", "-)", NodeShape::Cloud),
];

fn parse_node(t: &str, line_no: usize) -> Result<(String, String, NodeShape)> {
    let t = t.trim();
    // Find the earliest opening delimiter outside quotes.
    let mut best: Option<(usize, &str, &str, NodeShape)> = None;
    for (open, close, shape) in DELIMS {
        if let Some(pos) = find_unquoted(t, open) {
            let better = match best {
                None => true,
                Some((bp, bo, _, _)) => pos < bp || (pos == bp && open.len() > bo.len()),
            };
            if better {
                best = Some((pos, open, close, shape));
            }
        }
    }
    let Some((pos, open, close, shape)) = best else {
        let label = unquote(t);
        return Ok((t.to_string(), label, NodeShape::Default));
    };
    let id = t[..pos].trim().to_string();
    let inner_and_close = &t[pos + open.len()..];
    let inner = inner_and_close.strip_suffix(close).ok_or_else(|| {
        Error::parse(
            line_no,
            format!("{t}\nExpecting '{close}' to close '{open}'"),
        )
    })?;
    let label = unquote(inner.trim());
    let id = if id.is_empty() { label.clone() } else { id };
    Ok((id, label, shape))
}

fn find_unquoted(s: &str, pat: &str) -> Option<usize> {
    let mut in_q = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_q = !in_q;
        } else if !in_q && s[i..].starts_with(pat) {
            return Some(i);
        }
    }
    None
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s);
    let s = s
        .strip_prefix('`')
        .and_then(|x| x.strip_suffix('`'))
        .unwrap_or(s);
    s.to_string()
}

/// Measured and positioned node.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub center: Point,
    pub w: f64,
    pub h: f64,
    pub lines: Vec<String>,
    /// How far left of centre each line is drawn (`break-spaces` keeps the
    /// space a line broke at).
    pub shifts: Vec<f64>,
    /// Colour section (`None` for the root).
    pub section: Option<usize>,
    /// What `getBBox()` reports for the drawn node, relative to its centre (a
    /// bang or cloud outline bulges past its nominal box, off-centre).
    pub bbox: Rect,
    /// Outline of a bang or cloud node around its centre.
    pub outline: Option<Vec<PathCmd>>,
    /// The label's measured size.
    pub label: (f64, f64),
}

impl Placed {
    pub fn rect(&self) -> Rect {
        Rect::centered(self.center, self.w, self.h)
    }
}

/// Wrap a label as labelHelper's `createText` does: mindmap labels are always
/// markdown (`labelType: 'markdown'`), laid out in a `div` of `max-width: width`
/// that is given the whole width once a line had to wrap. Returns the lines and
/// the label's width.
#[cfg(test)]
fn wrap(text: &str, font: f64, width: f64) -> (Vec<String>, f64) {
    let (lines, _, tw) = wrap_shifted(text, font, width);
    (lines, tw)
}

/// [`wrap`], plus how far left of the label's centre each line sits (a line
/// that broke at a space keeps it, see [`crate::text::break_space_shift`]).
fn wrap_shifted(text: &str, font: f64, width: f64) -> (Vec<String>, Vec<f64>, f64) {
    let raw = crate::text::markdown_lines(&crate::icons::replace_references(text));
    let natural = raw
        .iter()
        .map(|l| line_width(l, font, false))
        .fold(0.0, f64::max);
    let (lines, shifts): (Vec<String>, Vec<f64>) =
        crate::text::wrap_lines_flagged(&raw, width, font, false, BreakRule::Browser)
            .into_iter()
            .map(|(l, at_space)| {
                let shift = if at_space {
                    crate::text::break_space_shift(&l, font, false)
                } else {
                    0.0
                };
                (l, shift)
            })
            .unzip();
    let tw = if natural > width {
        crate::text::html_wrap_width(&raw, width, font, false)
    } else {
        // An HTML label's box: layout units (1/64 px).
        TextBlock::new(lines.clone(), font, false).html().width
    };
    (lines, shifts, tw)
}

/// Label widths: `mindmap.maxNodeWidth` and `flowchart.wrappingWidth`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    pub max_node: f64,
    pub wrapping: f64,
}

/// A node's label lines, nominal size, drawn bounding box (around the centre)
/// and, for bang and cloud nodes, outline.
fn measure(n: &Node, font: f64, padding: f64, widths: Widths, neo: bool) -> Placed {
    // mindmapRenderer (12.3) zeroes `width` of rect, rounded and hexagon nodes, so
    // labelHelper wraps those at `flowchart.wrappingWidth`; the rest at maxNodeWidth.
    let width = match n.shape {
        NodeShape::Square | NodeShape::Rounded | NodeShape::Hexagon => widths.wrapping,
        _ => widths.max_node,
    };
    let (lines, shifts, tw) = wrap_shifted(&n.label, font, width);
    let tb = TextBlock::new(lines.clone(), font, false);
    let th = tb.height.max(font * 1.5);
    let mut outline = None;
    let (w, h) = match n.shape {
        // defaultMindmapNode.ts: w = tw + 8·(p/2), h = th + 2·(p/2).
        NodeShape::Default => (tw + 4.0 * padding, th + padding),
        // mindmapRenderer (12.3) sets the padding of rect nodes to 10 and of rounded
        // ones to 15; `rect` is tw + 4p × th + 2p, `rounded` tw + 2p × th + 2p.
        // squareRect.ts under the neo look: 16 and 12 a side instead.
        NodeShape::Square if neo => (tw + 32.0, th + 24.0),
        NodeShape::Square => (tw + 40.0, th + 20.0),
        NodeShape::Rounded => (tw + 30.0, th + 30.0),
        // hexagon.ts under the neo look: h = th + 70, m = h / 3.5, w = tw + 2m + 32.
        NodeShape::Hexagon if neo => {
            let h = th + 70.0;
            (tw + 2.0 * h / 3.5 + 32.0, h)
        }
        // mindmapDb doubles the hexagon's padding: h = th + p', w = tw + p' + h/2.
        NodeShape::Hexagon => {
            let h = th + 2.0 * padding;
            (tw + 2.0 * padding + h / 2.0, h)
        }
        // mindmapCircle → circle.ts with options.padding: r = hypot(tw, th)/2 + p
        // (under the neo look + 16, whatever the padding).
        NodeShape::Circle => {
            let d = tw.hypot(th) + 2.0 * if neo { 16.0 } else { padding };
            (d, d)
        }
        NodeShape::Cloud => (tw + padding, th + padding),
        NodeShape::Bang => (tw + 5.0 * padding, th + 4.0 * padding),
    };
    let mut bbox = Rect::centered(Point::default(), w, h);
    let arc_shape = match n.shape {
        NodeShape::Bang => Some(Shape::Bang),
        NodeShape::Cloud => Some(Shape::Cloud),
        _ => None,
    };
    if let Some((cmds, b)) = arc_shape.and_then(|s| shapes::arc_outline(s, tw, th, padding)) {
        bbox = b;
        outline = Some(cmds);
    }
    Placed {
        center: Point::default(),
        w,
        h,
        lines,
        shifts,
        section: None,
        bbox,
        outline,
        label: (tw, th),
    }
}

/// Lay out the tree as mermaid 12.3 does: cytoscape's CoSE-Bilkent over the
/// nodes' `getBBox()` sizes (single precision, as SVG reports them). Returns
/// one `Placed` per node (same indices as `mm.nodes`), centred in cytoscape's
/// coordinates.
pub fn layout(mm: &Mindmap, font: f64, padding: f64, widths: Widths) -> Vec<Placed> {
    layout_with(mm, font, padding, widths, None, false)
}

/// [`layout`], under the handDrawn look with `hand`'s rough.js options: a
/// node is as big as its rough drawing.
fn layout_with(
    mm: &Mindmap,
    font: f64,
    padding: f64,
    widths: Widths,
    hand: Option<&HandStyle>,
    neo: bool,
) -> Vec<Placed> {
    let mut placed: Vec<Placed> = mm
        .nodes
        .iter()
        .map(|nd| measure(nd, font, padding, widths, neo))
        .collect();
    if let Some(st) = hand {
        for (nd, p) in mm.nodes.iter().zip(placed.iter_mut()) {
            if let Some(hn) = hand_node(nd.shape, p.label, padding, st) {
                p.bbox = hand_group_box(&hn, p.label).unwrap_or(hn.bbox);
            }
        }
    }
    if placed.is_empty() {
        return placed;
    }
    assign_sections(mm, &mut placed, false);
    let f32_round = |v: f64| f64::from(v as f32);
    let sizes: Vec<(f64, f64)> = placed
        .iter()
        .map(|p| (f32_round(p.bbox.w), f32_round(p.bbox.h)))
        .collect();
    // mindmapDb.generateEdges: parent → child in pre-order, as the nodes are.
    let edges: Vec<(usize, usize)> = mm
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.parent.map(|p| (p, i)))
        .collect();
    let centers = cose::layout_tree(&sizes, &edges);
    for (p, (x, y)) in placed.iter_mut().zip(centers) {
        p.center = Point::new(x, y);
    }
    placed
}

/// `getBBox()` of a hand-drawn node's group as Chromium joins it: the rough
/// paths' group box (see [`crate::diagram::svgbbox::cubic_group_box`]) moved
/// by the shape's translation, then joined with the label's box
/// (`gfx::RectF::Union`: single-precision edges, the width their plain
/// difference).
fn hand_group_box(hn: &HandNode, label: (f64, f64)) -> Option<Rect> {
    let mut acc: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32, w: f32, h: f32| {
        let (r, b) = (x + w, y + h);
        acc = Some(match acc {
            None => (x, y, r, b),
            Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(r), e.max(b)),
        });
    };
    for pc in hn.pieces.iter().filter(|pc| pc.measured) {
        let paths: Vec<Vec<PathCmd>> = pc
            .drawable
            .sets
            .iter()
            .filter(|s| !s.ops.is_empty())
            .map(|s| hand::set_cmds(&s.ops, Point::default()))
            .collect();
        let [x, y, w, h] = crate::diagram::svgbbox::cubic_group_box(&paths)?;
        add(
            x as f32 + pc.at.x as f32,
            y as f32 + pc.at.y as f32,
            w as f32,
            h as f32,
        );
    }
    add(
        (-label.0 / 2.0) as f32,
        (-label.1 / 2.0) as f32,
        label.0 as f32,
        label.1 as f32,
    );
    let (x, y, r, b) = acc?;
    Some(Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(r - x),
        f64::from(b - y),
    ))
}

/// The rough.js drawing of a node of `shape` around a `label`-sized label
/// under the handDrawn look, as the flowchart shapes mindmapRenderer hands
/// it to draw it (`None`: a default node, drawn as in the other looks).
fn hand_node(
    shape: NodeShape,
    label: (f64, f64),
    padding: f64,
    st: &HandStyle,
) -> Option<HandNode> {
    let (graph_shape, p) = match shape {
        NodeShape::Default => return None,
        // squareRect.ts with node.padding 10, roundedRect.ts with 15.
        NodeShape::Square => (Shape::Rect, 10.0),
        NodeShape::Rounded => (Shape::Rounded, 15.0),
        // mindmapCircle: circle.ts with `options.padding` (a whole padding,
        // where circle.ts alone takes half of node.padding).
        NodeShape::Circle => (Shape::Circle, 2.0 * padding),
        // mindmapDb doubles the hexagon's padding.
        NodeShape::Hexagon => (Shape::Hexagon, 2.0 * padding),
        NodeShape::Bang => (Shape::Bang, padding),
        NodeShape::Cloud => (Shape::Cloud, padding),
    };
    let m = shapes::ShapeMetrics::classic(graph_shape, p).with_label(label.0, label.1);
    hand::node(graph_shape, &m, st)
}

/// An edge as cytoscape routes it for mermaid: between the 30px default
/// ellipses around both centres (`rscratch` start, middle and end), drawn by
/// mermaid as a `basis` curve through those three points.
fn edge_points(a: Point, b: Point) -> [Point; 3] {
    // cytoscape's intersectLineEllipse with 15px radii.
    let toward = |c: Point, far: Point| {
        let len = c.minus(far).len() / 15.0;
        if len <= 1.0 {
            return c;
        }
        let k = (len - 1.0) / len;
        Point::new((c.x - far.x) * k + far.x, (c.y - far.y) * k + far.y)
    };
    let s = toward(a, b);
    let e = toward(b, a);
    [s, Point::new((s.x + e.x) / 2.0, (s.y + e.y) / 2.0), e]
}

/// mindmapDb.assignSections: each node's colour section is the index of the root
/// child it descends from, or with `colorBy: depth` its distance from the root
/// less one; either modulo `MAX_SECTIONS - 1` (11).
pub fn assign_sections(mm: &Mindmap, placed: &mut [Placed], by_depth: bool) {
    let Some(root) = mm.nodes.first() else {
        return;
    };
    for (s, &c) in root.children.iter().enumerate() {
        let mut stack = vec![(c, 1usize)];
        while let Some((k, depth)) = stack.pop() {
            placed[k].section = Some(if by_depth { depth - 1 } else { s } % 11);
            stack.extend(mm.nodes[k].children.iter().map(|&ch| (ch, depth + 1)));
        }
    }
}

/// Whether default-shape nodes show their 3px underline: mindmap's styles give
/// `.section-N line` `stroke-width: 0` when the theme's `useGradient` is on, which
/// it is in every theme but `default` (and `base` given a `nodeBorder`) unless
/// themeVariables say otherwise. The redux themes draw no underline at all.
fn underlines(cfg: &MermaidConfig, pal: &Palette) -> bool {
    use crate::theme::ThemeName as T;
    let var = |k: &str| {
        cfg.theme_variables
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim())
    };
    let gradient = match pal.theme {
        T::Default | T::Null => false,
        T::Base => var("nodeBorder").is_none(),
        T::Redux | T::ReduxDark | T::ReduxColor | T::ReduxDarkColor => return false,
        _ => true,
    };
    let gradient = match var("useGradient") {
        Some("true") => true,
        Some("false") => false,
        _ => gradient,
    };
    !gradient
}

/// Render a mindmap.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let mut mm = parse(body, title)?;
    // mindmapDb's getShapeFromType: a default node is `rounded` under any
    // theme whose name has "redux" in it.
    if pal.theme.name().contains("redux") {
        for n in &mut mm.nodes {
            if n.shape == NodeShape::Default {
                n.shape = NodeShape::Rounded;
            }
        }
    }
    let neo = (cfg.look == crate::config::Look::Neo).then(|| Neo::new(pal));
    let padding = cfg.section_f64("mindmap", "padding", 10.0);
    let widths = Widths {
        max_node: cfg.section_f64("mindmap", "maxNodeWidth", 200.0),
        wrapping: cfg.section_f64("flowchart", "wrappingWidth", 120.0),
    };
    let hand = (cfg.look == crate::config::Look::HandDrawn).then(|| HandStyle {
        fill: pal.main_bkg.css().to_string(),
        stroke: pal.node_border.css().to_string(),
        stroke_width: 1.3,
        seed: cfg.hand_drawn_seed(),
        radius: pal.radius.unwrap_or(5.0),
        line: pal.line_color.css().to_string(),
        end_dot: pal.node_border.css().to_string(),
        note_fill: pal.note_bkg.css().to_string(),
        note_stroke: pal.note_border.css().to_string(),
        horizontal: false,
        state: false,
    });
    let mut placed = layout_with(
        &mm,
        pal.font_size,
        padding,
        widths,
        hand.as_ref(),
        neo.is_some(),
    );
    if cfg
        .section("mindmap", "colorBy")
        .and_then(|j| j.as_str())
        .is_some_and(|v| v == "depth")
    {
        assign_sections(&mm, &mut placed, true);
    }
    Ok(draw(
        &mm,
        &placed,
        pal,
        padding,
        underlines(cfg, pal),
        hand.as_ref(),
        neo.as_ref(),
        widths,
    ))
}

/// The neo look's rules (mindmap/styles.ts `[data-look="neo"]`): outlined
/// nodes, other fills and label colours for some themes, thinner and (on
/// some themes) uniformly coloured edges, the theme's drop shadow on every
/// node, and with `useGradient` the gradient outline over a `mainBkg` fill.
pub struct Neo {
    /// `theme === 'redux' || theme === 'redux-dark'`.
    redux_mono: bool,
    /// `theme?.includes('redux')`.
    redux: bool,
    neutral: bool,
    /// `theme?.includes('redux') || theme === 'neo-dark'`: edges in `nodeBorder`.
    edge_border: bool,
    stroke_width: f64,
    shadow: Option<Shadow>,
    gradient: Option<Gradient>,
}

impl Neo {
    pub fn new(pal: &Palette) -> Self {
        use crate::theme::ThemeName as T;
        let name = pal.theme.name();
        // `[data-look="neo"].mindmap-node { filter: dropShadow }`: the redux
        // themes' `url(#drop-shadow)` is insertLookDefs' feDropShadow.
        let shadow = pal.drop_shadow.as_ref().map(|d| d.filter());
        Neo {
            redux_mono: matches!(pal.theme, T::Redux | T::ReduxDark),
            redux: name.contains("redux"),
            neutral: pal.theme == T::Neutral,
            edge_border: name.contains("redux") || pal.theme == T::NeoDark,
            // `strokeWidth ?? 2`: every theme sets it, the classic ones to 1.
            stroke_width: pal.stroke_width.unwrap_or(1.0),
            shadow,
            gradient: pal
                .node_gradient
                .as_ref()
                .map(|(a, b)| Gradient::across_bbox(a, b)),
        }
    }

    /// Fill, outline and label colour of a node of `section` (`None`: the root).
    fn node(&self, pal: &Palette, section: Option<usize>) -> (Color, Color, Color) {
        let i = section.map_or(0, |s| s + 1);
        let fill = match section {
            _ if self.gradient.is_some() => pal.main_bkg.clone(),
            None if self.redux => pal.main_bkg.clone(),
            None => pal
                .git
                .first()
                .unwrap_or_else(|| pal.scale_color(0))
                .clone(),
            Some(_) if self.redux_mono || self.neutral => pal.main_bkg.clone(),
            Some(_) => pal.scale_color(i).clone(),
        };
        let stroke = if self.redux_mono {
            pal.node_border.clone()
        } else {
            pal.scale_color(i).clone()
        };
        let label_i = if self.neutral { 1 } else { i };
        let label = match section {
            None if self.redux => pal.node_border.clone(),
            Some(_) if self.redux_mono => pal.node_border.clone(),
            _ => pal.scale_text_color(label_i).clone(),
        };
        (fill, stroke, label)
    }

    /// Outline and gradient on every shape of a node but its underline;
    /// with `own_shadow` (a `rect`, `circle` or `polygon`, which the shared
    /// `[data-look="neo"].node rect, polygon, circle` rules filter too) the
    /// shadow on the shape as well as on the node's group.
    fn outline(&self, els: &mut [Element], stroke: &Color, own_shadow: bool) {
        for e in els {
            match e {
                Element::Shape { geom, paint, .. } => {
                    if !matches!(geom, Geometry::Line { .. }) {
                        paint.stroke = stroke.clone();
                        paint.stroke_width = self.stroke_width;
                        paint.stroke_gradient = self.gradient.clone();
                        if own_shadow {
                            paint.shadow = self.shadow.clone();
                        }
                    }
                }
                Element::Group { children, .. } => self.outline(children, stroke, own_shadow),
                _ => {}
            }
        }
    }

    /// `.edge-depth-N` under the neo look: `max(10 - 2N, 2)` for the 12
    /// theme colours, `.edge`'s 3 past them.
    fn edge_width(depth: usize) -> f64 {
        if depth + 1 < 12 {
            (10.0 - 2.0 * depth as f64).max(2.0)
        } else {
            3.0
        }
    }
}

/// `(fill, text, underline)` of a node: section `s` is `.section-s` (`cScale(s+1)`,
/// `cScaleLabel(s+1)`, `cScaleInv(s+1)`); the root is `.section-root` (`git0`,
/// `gitBranchLabel0`) and also `.section--1` (whose `line` is `cScaleInv0`).
fn section_colors(pal: &Palette, section: Option<usize>) -> (Color, Color, Color) {
    match section {
        None => {
            let fill = pal.git.first().unwrap_or_else(|| pal.scale_color(0));
            let text = pal
                .git_branch_label
                .first()
                .unwrap_or_else(|| pal.scale_text_color(0));
            (fill.clone(), text.clone(), pal.scale_inv_color(0).clone())
        }
        Some(s) => (
            pal.scale_color(s + 1).clone(),
            pal.scale_text_color(s + 1).clone(),
            pal.scale_inv_color(s + 1).clone(),
        ),
    }
}

/// Draw a laid-out mindmap.
///
/// `underline` draws the line under default-shape nodes (see `underlines`).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mm: &Mindmap,
    placed: &[Placed],
    pal: &Palette,
    padding: f64,
    underline: bool,
    hand: Option<&HandStyle>,
    neo: Option<&Neo>,
    widths: Widths,
) -> Scene {
    let font = pal.font_size;
    let mut bounds = Bounds::default();
    let mut edges = Vec::new();
    let mut nodes = Vec::new();
    for (i, nd) in mm.nodes.iter().enumerate() {
        let p = &placed[i];
        if let Some(par) = nd.parent {
            let pts = edge_points(placed[par].center, p.center);
            for q in pts {
                bounds.add_point(q);
            }
            // mindmapDb: `edge-depth-{parent level + 1}`, the level counted in
            // indentation characters from the root's.
            let depth = mm.nodes[par].level - mm.nodes[0].level + 1;
            let (c, _, _) = section_colors(pal, p.section);
            let mut cmds = geom::basis_curve(&pts);
            if let Some(st) = hand {
                // insertEdge under the handDrawn look: the curve's path data
                // through rough.js at roughness 0.3.
                let d = super::graph::d3_path_data(&Geometry::Path(cmds));
                let o = crate::rough::Options {
                    roughness: 0.3,
                    seed: st.seed,
                    ..Default::default()
                };
                let drawn = crate::rough::path(&d, o);
                let ops: Vec<_> = drawn.sets.into_iter().flat_map(|s| s.ops).collect();
                cmds = hand::set_cmds(&ops, Point::default());
                if let Some(b) = geom::path_bbox(&cmds) {
                    bounds.add_rect(b);
                }
            }
            let paint = match neo {
                Some(n) => Paint::stroke(
                    if n.edge_border { &pal.node_border } else { &c },
                    Neo::edge_width(depth),
                ),
                None => Paint::stroke(&c, edge_width(depth)),
            };
            edges.push(Element::classed_shape(
                Geometry::Path(cmds),
                paint,
                format!(
                    "edge section-edge-{} edge-depth-{depth}",
                    p.section.unwrap_or(0)
                ),
            ));
        }
        let (mut fill, mut text_color, line) = section_colors(pal, p.section);
        let mut stroke = None;
        if let Some(n) = neo {
            let (f, s, t) = n.node(pal, p.section);
            (fill, text_color, stroke) = (f, t, Some(s));
        }
        let r = p.rect();
        let b = p.bbox;
        bounds.add_rect(Rect::new(p.center.x + b.x, p.center.y + b.y, b.w, b.h));
        let rough = hand.and_then(|st| hand_node(nd.shape, p.label, padding, st));
        let mut els = match &p.outline {
            _ if rough.is_some() => {
                let hn = rough.as_ref().expect("checked");
                let b = hn.drawn_bbox();
                bounds.add_rect(Rect::new(p.center.x + b.x, p.center.y + b.y, b.w, b.h));
                hand_elements(hn, p.center, &fill)
            }
            Some(cmds) => vec![Element::shape(
                Geometry::Path(cmds.clone()).translated(p.center.x, p.center.y),
                Paint::fill(&fill),
            )],
            None => shape_elements(
                nd.shape,
                r,
                &fill,
                underline.then_some(&line),
                pal.radius.unwrap_or(5.0),
                neo.is_some(),
            ),
        };
        if let (Some(n), Some(stroke)) = (neo, &stroke) {
            let own_shadow = matches!(
                nd.shape,
                NodeShape::Square | NodeShape::Rounded | NodeShape::Circle | NodeShape::Hexagon
            );
            n.outline(&mut els, stroke, own_shadow);
        }
        let block = Text::centered(r.center(), &p.lines, font, font * 1.5, &text_color);
        if p.shifts.iter().all(|&s| s == 0.0) {
            els.push(Element::text(block));
        } else {
            // A line that broke at a space keeps it (`break-spaces`): one text
            // per line, each centred with its own offset.
            for (i, (line, shift)) in p.lines.iter().zip(&p.shifts).enumerate() {
                els.push(Element::text(Text {
                    x: block.x - shift,
                    y: block.y + i as f64 * block.line_height,
                    lines: vec![line.clone()],
                    ..block.clone()
                }));
            }
        }
        let section_class = match p.section {
            None => "section-root".to_string(),
            Some(s) => format!("section-{s}"),
        };
        let mut class = format!("mindmap-node {section_class}");
        for c in &nd.classes {
            class.push(' ');
            class.push_str(c);
        }
        // The neo look's shadow: a filter on the node's whole group.
        if let Some(shadow) = neo.and_then(|n| n.shadow.clone()) {
            els = vec![Element::Shadowed {
                shadow,
                children: els,
            }];
        }
        nodes.push(Element::group(class, els));
    }
    let mut els = vec![Element::group("mindmap-edges", edges)];
    els.push(Element::group("mindmap-nodes", nodes));
    if let Some(t) = mm.title.as_ref().filter(|t| !t.is_empty()) {
        let r = bounds.rect();
        let c = Point::new(r.center().x, r.y - 24.0);
        let tb = TextBlock::new(vec![t.clone()], 18.0, false);
        bounds.add_rect(Rect::centered(c, tb.width, tb.height));
        els.push(Element::classed_text(
            Text::centered(c, &tb.lines, 18.0, tb.line_height, &pal.title_color),
            "mindmapTitle",
        ));
    }
    // Plain SVG output is mermaid's own markup (which has no title).
    let dom = dom::Dom {
        look: match (hand.is_some(), neo.is_some()) {
            (true, _) => "handDrawn",
            (_, true) => "neo",
            _ => "classic",
        },
        pal,
        padding,
        widths,
        hand,
        neo: neo.is_some(),
    };
    let (svg, svg_view_box) = dom::markup(mm, placed, &dom, padding);
    Scene {
        kind: DiagramKind::Mindmap,
        // setupViewPortForSVG: the drawing's bounding box plus `mindmap.padding`.
        view_box: bounds.rect().pad(padding),
        elements: vec![Element::Markup {
            svg,
            children: els,
            svg_view_box,
        }],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// A hand-drawn node's rough.js paths centred on `c`: hachures stroked in the
/// fill colour, outlines in the stroke colour, and every path filled with
/// its section's colour (`.section-N path { fill }` outranks rough.js's
/// `fill` attributes).
fn hand_elements(hn: &HandNode, c: Point, section_fill: &Color) -> Vec<Element> {
    let mut out = Vec::new();
    for pc in &hn.pieces {
        let o = &pc.drawable.options;
        let at = c.plus(pc.at);
        let mut kids = Vec::new();
        for set in pc.drawable.sets.iter().filter(|s| !s.ops.is_empty()) {
            let mut paint = match set.kind {
                crate::rough::SetKind::Path => {
                    Paint::stroke(&Color::new(o.stroke.clone()), o.stroke_width)
                }
                crate::rough::SetKind::FillSketch => Paint::stroke(
                    &Color::new(o.fill.clone().unwrap_or_default()),
                    pc.drawable.fill_weight(),
                ),
                crate::rough::SetKind::FillPath => {
                    Paint::fill(&Color::new(o.fill.clone().unwrap_or_default()))
                }
            };
            paint.fill = section_fill.clone();
            kids.push(Element::shape(
                Geometry::Path(hand::set_cmds(&set.ops, at)),
                paint,
            ));
        }
        out.push(Element::group("basic label-container", kids));
    }
    out
}

/// Stroke width of `.edge-depth-{depth}` (classic look: `17 - 3·(depth + 1)` for
/// the 12 theme colours). Past them, or where that width turns negative and the
/// declaration is dropped, `.edge`'s 3 applies.
fn edge_width(depth: usize) -> f64 {
    let i = depth + 1;
    let w = 17.0 - 3.0 * i as f64;
    if i < 12 && w > 0.0 { w } else { 3.0 }
}

/// A quadratic Bézier from `a` via `c` to `b` as a cubic.
fn quad(a: Point, c: Point, b: Point) -> PathCmd {
    let k = 2.0 / 3.0;
    PathCmd::CubicTo(
        Point::new(a.x + k * (c.x - a.x), a.y + k * (c.y - a.y)),
        Point::new(b.x + k * (c.x - b.x), b.y + k * (c.y - b.y)),
        b,
    )
}

/// A node's shapes in `r`: `radius` is a rounded node's corner
/// (`themeVariables.radius ?? 5`).
fn shape_elements(
    shape: NodeShape,
    r: Rect,
    fill: &Color,
    line: Option<&Color>,
    radius: f64,
    neo: bool,
) -> Vec<Element> {
    let c = r.center();
    let paint = Paint::fill(fill);
    match shape {
        NodeShape::Default => {
            // defaultMindmapNode.ts: corners rounded by quadratic curves of 5,
            // the bottom ones square under the neo look.
            let rr = 5.0;
            let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
            let p = Point::new;
            let mut cmds = vec![
                PathCmd::MoveTo(p(x0, y1 - rr)),
                PathCmd::LineTo(p(x0, y0 + rr)),
                quad(p(x0, y0 + rr), p(x0, y0), p(x0 + rr, y0)),
                PathCmd::LineTo(p(x1 - rr, y0)),
                quad(p(x1 - rr, y0), p(x1, y0), p(x1, y0 + rr)),
            ];
            if neo {
                cmds.extend([PathCmd::LineTo(p(x1, y1)), PathCmd::LineTo(p(x0, y1))]);
            } else {
                cmds.extend([
                    PathCmd::LineTo(p(x1, y1 - rr)),
                    quad(p(x1, y1 - rr), p(x1, y1), p(x1 - rr, y1)),
                    PathCmd::LineTo(p(x0 + rr, y1)),
                    quad(p(x0 + rr, y1), p(x0, y1), p(x0, y1 - rr)),
                ]);
            }
            cmds.push(PathCmd::Close);
            let mut els = vec![Element::classed_shape(
                Geometry::Path(cmds),
                paint,
                "node-bkg node-no-border",
            )];
            if let Some(line) = line {
                els.push(Element::shape(
                    Geometry::Line {
                        a: Point::new(x0, y1),
                        b: Point::new(x1, y1),
                    },
                    Paint::stroke(line, 3.0),
                ));
            }
            els
        }
        // Bang and cloud nodes are drawn from their outline.
        NodeShape::Square | NodeShape::Bang | NodeShape::Cloud => {
            vec![Element::rect(r, 0.0, paint)]
        }
        NodeShape::Rounded => vec![Element::rect(r, radius, paint)],
        NodeShape::Circle => vec![Element::shape(Geometry::Circle { c, r: r.w / 2.0 }, paint)],
        NodeShape::Hexagon => {
            let m = r.h / if neo { 3.5 } else { 4.0 };
            let pts = [
                Point::new(r.x + m, r.y),
                Point::new(r.right() - m, r.y),
                Point::new(r.right(), c.y),
                Point::new(r.right() - m, r.bottom()),
                Point::new(r.x + m, r.bottom()),
                Point::new(r.x, c.y),
            ];
            vec![Element::shape(Geometry::Path(geom::polygon(&pts)), paint)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Widths = Widths {
        max_node: 200.0,
        wrapping: 120.0,
    };

    const SRC: &str = "mindmap
  root((mindmap))
    Origins
      Long history
      ::icon(fa fa-book)
      Popularisation
        British popular psychology author Tony Buzan
    Research
      On effectiveness<br/>and features
      On Automatic creation
        Uses
            Creative techniques
            Strategic planning
            Argument mapping
    Tools
      Pen and paper
      Mermaid
    id1[Square]
    id2(Rounded)
    :::urgent
    id3))Bang((
    id4)Cloud(
    id5{{Hexagon}}
    :::large big";

    #[test]
    fn label_width_is_the_html_label_box() {
        // mermaid 12.3.0 sizes this label's foreignObject 107.609375 wide.
        assert_eq!(wrap("Origins of thing", 16.0, 200.0).1, 107.609375);
    }

    #[test]
    fn wrapped_labels_follow_break_spaces() {
        // mermaid 12.3.0: three lines (the space after "leaf" must fit too).
        let (lines, w) = wrap(
            "rho W Long-term kappa leaf alpha lambda Long-term i",
            16.0,
            200.0,
        );
        assert_eq!(
            lines,
            ["rho W Long-term kappa", "leaf alpha lambda Long-", "term i"]
        );
        assert_eq!(w, 200.0);
        // The box widens to its longest word: foreignObject 137.875 wide.
        let (lines, w) = wrap("theta internationalization eta alpha lambda", 16.0, 120.0);
        assert_eq!(lines, ["theta", "internationalization", "eta alpha lambda"]);
        assert_eq!(w, 137.875);
    }

    #[test]
    fn parses_tree_and_shapes() {
        let mm = parse(SRC, None).unwrap();
        assert_eq!(mm.nodes[0].shape, NodeShape::Circle);
        assert_eq!(mm.nodes[0].label, "mindmap");
        assert_eq!(mm.nodes[0].children.len(), 8);
        let find = |l: &str| mm.nodes.iter().find(|n| n.label == l).unwrap();
        assert_eq!(find("Long history").icon.as_deref(), Some("fa fa-book"));
        assert_eq!(find("Square").shape, NodeShape::Square);
        assert_eq!(find("Rounded").shape, NodeShape::Rounded);
        assert_eq!(find("Bang").shape, NodeShape::Bang);
        assert_eq!(find("Cloud").shape, NodeShape::Cloud);
        assert_eq!(find("Hexagon").shape, NodeShape::Hexagon);
        assert_eq!(find("Hexagon").classes, vec!["large", "big"]);
        assert_eq!(find("Uses").children.len(), 3);
        assert_eq!(find("Square").id, "id1");
    }

    #[test]
    fn rejects_second_root() {
        let e = parse("mindmap\n  a\n    b\n  c", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 4, .. }), "{e:?}");
        let e = parse("mindmap\n  a[unclosed", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("notmindmap", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 1, .. }));
    }

    #[test]
    fn quoted_labels_keep_delimiters() {
        let mm = parse("mindmap\n  r[\"a (b) c\"]\n    \"`**md**`\"", None).unwrap();
        assert_eq!(mm.nodes[0].label, "a (b) c");
        assert_eq!(mm.nodes[1].label, "**md**");
    }

    /// Node centres mermaid 12.3.0 (Chromium) placed, as `translate(x, y)`.
    fn assert_centres(src: &str, want: &[(f64, f64)]) {
        let mm = parse(src, None).unwrap();
        let p = layout(&mm, 16.0, 10.0, W);
        assert_eq!(p.len(), want.len());
        for (i, (q, &(x, y))) in p.iter().zip(want).enumerate() {
            assert!(
                (q.center.x - x).abs() < 1e-9 && (q.center.y - y).abs() < 1e-9,
                "node {i}: {:?} vs ({x}, {y})",
                q.center
            );
        }
    }

    #[test]
    fn layout_reproduces_mermaids_cose_bilkent_positions() {
        let src = "mindmap
  root((Iota))
    n1[theta mind leaf]
      eta mind
      n3[alpha leaf sigma omicron]
    n4(beta beta beta)
    xi branch beta tools
      n6((omicron psi omicron omicron))
      n7{{root}}
        n8(mu)
        n9{{chi}}";
        assert_centres(
            src,
            &[
                (459.7011144692349, 320.2904984040083),
                (578.6254360771309, 382.3714601896676),
                (755.8466086387257, 390.4253680479784),
                (566.5135219778385, 492.35753168954545),
                (340.0547411674238, 391.8483283499353),
                (384.65177812032584, 233.84471747236842),
                (127.83968353271484, 214.04117727364735),
                (399.8008617011801, 141.01901835154922),
                (358.37459270495606, 42.0),
                (513.0454034525198, 77.3043028507235),
            ],
        );
    }

    #[test]
    fn bang_and_cloud_nodes_are_sized_as_chromium_measures_them() {
        // The gallery example: its bang and cloud boxes come from Chromium's
        // getBBox(), and the whole layout lands on mermaid's coordinates.
        let src = "mindmap
  root((Mind maps))
    Origins
      Long history
      ::icon(fa fa-book)
      Popularisation
        British popular psychology author Tony Buzan
    Research
      On effectiveness<br/>and features
      On Automatic creation
        Uses
            Creative techniques
            Strategic planning
            Argument mapping
    Tools
      Pen and paper
      Mermaid
    Shapes
      sq[Square]
      rd(Rounded)
      :::urgent
      bg))Bang((
      cl)Cloud(
      hx{{Hexagon}}
      ci((Circle))";
        let mm = parse(src, None).unwrap();
        let p = layout(&mm, 16.0, 10.0, W);
        let b = p[18].bbox;
        assert_eq!(
            (b.x, b.y, b.w, b.h),
            (-52.42499923706055, -38.400001525878906, 109.21875, 80.0)
        );
        let c = p[19].bbox;
        assert_eq!(
            (c.x, c.y, c.w, c.h),
            (
                -39.84649658203125,
                -28.68901824951172,
                81.35205841064453,
                58.90385437011719
            )
        );
        assert_centres(
            src,
            &[
                (376.02877940564906, 274.793203217614),
                (515.8012353497398, 340.03645740829575),
                (679.0366506190506, 312.35133743067513),
                (589.1333470814394, 412.4675479061931),
                (832.7309064827283, 459.7936765609388),
                (324.40705202960044, 394.678576868105),
                (137.67134467629774, 449.83750700246514),
                (379.51712179140054, 474.0447749215424),
                (387.93854444419037, 562.4312802608188),
                (571.6201593732293, 562.4068470123025),
                (444.39007767472333, 640.8665068520803),
                (207.36077093263282, 593.1195835199801),
                (229.4707583317255, 280.5035211979048),
                (87.4921875, 327.26420596966216),
                (117.3488523603902, 220.43102844541636),
                (446.80971661538786, 148.4397586028635),
                (502.2908890730308, 232.05678228580848),
                (309.1631144287787, 82.01175287927248),
                (615.0491765087016, 155.8046398879663),
                (428.40113049698687, 44.451927185058594),
                (290.8940286231008, 169.79414833556586),
                (534.4257624888925, 54.24852623732278),
            ],
        );
    }

    #[test]
    fn edges_run_between_the_15px_circles_around_the_centres() {
        let [s, m, e] = edge_points(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert_eq!(
            (s, m, e),
            (
                Point::new(15.0, 0.0),
                Point::new(50.0, 0.0),
                Point::new(85.0, 0.0)
            )
        );
        // `.edge-depth-N` widths, falling back to `.edge`'s 3 once negative.
        assert_eq!([1, 3, 4, 5, 11].map(edge_width), [11.0, 5.0, 2.0, 3.0, 3.0]);
    }

    #[test]
    fn markdown_strings_may_span_lines_and_wrap_at_the_wrapping_width() {
        let src = "mindmap
    id1[\"`**Root** with
a second line
Unicode works too: 🤓`\"]
      id2[\"`The dog in **the** hog... a *very long text* that wraps to a new line`\"]
      id3[Regular labels still works]
      id4)\"multi
line\"(";
        let mm = parse(src, None).unwrap();
        assert_eq!(mm.nodes.len(), 4);
        assert_eq!(
            mm.nodes[0].label,
            "**Root** with\na second line\nUnicode works too: 🤓"
        );
        assert_eq!(mm.nodes[2].parent, Some(0));
        assert_eq!(mm.nodes[3].label, "multi\nline");
        let p = layout(&mm, 16.0, 10.0, W);
        let plain = |i: usize| -> Vec<String> {
            p[i].lines.iter().map(|l| crate::text::plain(l)).collect()
        };
        // Rect nodes wrap at 120 and take the whole width (+ 4 × 10 padding).
        assert_eq!(
            plain(0),
            ["Root with", "a second line", "Unicode works", "too: 🤓"]
        );
        assert_eq!(plain(2), ["Regular labels", "still works"]);
        assert_eq!((p[2].w, p[2].h), (160.0, 68.0));
        assert!(crate::text::has_marks(&p[0].lines[0]));
        // A cloud wraps at maxNodeWidth and is not widened.
        assert_eq!(plain(3), ["multi", "line"]);
        assert!(p[3].w < 120.0);
        // An unclosed string still fails on its own line.
        let e = parse("mindmap\n  root\n    a[\"open\n    b", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
    }

    #[test]
    fn class_on_the_node_line_is_a_syntax_error_like_mermaid() {
        let e = parse("mindmap\n  root\n    a(A):::urgent", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let mm = parse("mindmap\n  root\n    a(A)\n    :::urgent", None).unwrap();
        assert_eq!(mm.nodes[1].classes, vec!["urgent"]);
        // In a bare id, `:::` is just text.
        let mm = parse("mindmap\n  root:::x\n    a:::urgent", None).unwrap();
        assert_eq!(mm.nodes[0].label, "root:::x");
        assert_eq!(mm.nodes[1].label, "a:::urgent");
    }

    #[test]
    fn a_line_broken_at_a_space_keeps_it() {
        // mermaid 12.3.0: "I am a rounded square" wraps after "rounded " in a
        // `break-spaces` div, so the centred first line ends with the space and its
        // glyphs start 2.234px left of the centred text (x 28.953 vs 31.188).
        let (lines, shifts, _) = wrap_shifted("I am a rounded square", 16.0, 120.0);
        assert_eq!(lines, ["I am a rounded", "square"]);
        assert!((shifts[0] - 2.2227).abs() < 1e-3, "{shifts:?}");
        assert_eq!(shifts[1], 0.0);
        // A break after a hyphen keeps no space.
        let (lines, shifts, _) = wrap_shifted(
            "rho W Long-term kappa leaf alpha lambda Long-term i",
            16.0,
            200.0,
        );
        assert_eq!(lines[1], "leaf alpha lambda Long-");
        assert!(shifts[0] > 2.0 && shifts[1] == 0.0 && shifts[2] == 0.0);
    }

    #[test]
    fn color_by_depth_and_gradient_underlines() {
        let src = "---\nconfig:\n  theme: base\n  mindmap:\n    colorBy: depth\n---\nmindmap\n  root((mindmap))\n    a[A]\n      b\n    c[C]\n      d\n";
        let mm = parse(
            "mindmap\n  root((mindmap))\n    a[A]\n      b\n    c[C]\n      d\n",
            None,
        )
        .unwrap();
        let mut p = layout(
            &mm,
            16.0,
            10.0,
            Widths {
                max_node: 200.0,
                wrapping: 120.0,
            },
        );
        let sections = |p: &[Placed]| p.iter().map(|p| p.section).collect::<Vec<_>>();
        assert_eq!(sections(&p), [None, Some(0), Some(0), Some(1), Some(1)]);
        assert_using_depth(&mm, &mut p);
        // mermaid 12.3.0 under `theme: base` (useGradient): `.section-N line`
        // has stroke-width 0, so default nodes show no underline.
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        fn lines(els: &[Element]) -> usize {
            els.iter()
                .map(|e| match e {
                    Element::Group { children, .. } | Element::Markup { children, .. } => {
                        lines(children)
                    }
                    Element::Shape {
                        geom: Geometry::Line { .. },
                        class: None,
                        ..
                    } => 1,
                    _ => 0,
                })
                .sum()
        }
        assert_eq!(lines(&r.scene.elements), 0);
        let r = crate::render::render(
            &src.replace("theme: base", "theme: default"),
            &MermaidConfig::default(),
        )
        .unwrap();
        assert_eq!(lines(&r.scene.elements), 2);
    }

    fn assert_using_depth(mm: &Mindmap, p: &mut [Placed]) {
        assign_sections(mm, p, true);
        let s: Vec<_> = p.iter().map(|p| p.section).collect();
        assert_eq!(s, [None, Some(0), Some(1), Some(0), Some(1)]);
    }

    /// mermaid 12.3.0 under `look: handDrawn` (seed 13): the shaped nodes are
    /// the flowchart's rough.js shapes, laid out at their drawings' size (the
    /// root circle's outline from (188.324, 221.661), the bang's from
    /// (26.913, 430.009)), and every edge is a rough path.
    #[test]
    fn hand_drawn_nodes_and_edges_match_mermaid() {
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 13\n---\nmindmap\n  \
                   root((Root))\n    A[Square]\n      B(Rounded)\n      C))Bang((\n    \
                   D)Cloud(\n      E{{Hexagon}}\n    F\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .expect("renders")
            .scene;
        fn walk(els: &[Element], out: &mut Vec<String>) {
            for el in els {
                match el {
                    Element::Group { children, .. } | Element::Markup { children, .. } => {
                        walk(children, out)
                    }
                    Element::Shape {
                        geom: Geometry::Path(cmds),
                        ..
                    } => out.push(crate::output::svg::path_data(cmds)),
                    _ => {}
                }
            }
        }
        let mut paths = Vec::new();
        walk(&scene.elements, &mut paths);
        for start in ["M188.324,221.661C", "M26.913,430.009C"] {
            assert!(paths.iter().any(|d| d.starts_with(start)), "{start}");
        }
        // Six rough edges: each segment drawn twice.
        let rough_edges = paths.iter().filter(|d| d.matches('M').count() >= 6).count();
        assert!(rough_edges >= 6, "{paths:?}");
    }
}
