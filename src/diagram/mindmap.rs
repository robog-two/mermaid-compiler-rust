//! `mindmap` diagrams.
//!
//! Syntax: an indentation-based tree, one node per line. Node forms:
//! `text` (default), `id[square]`, `id(rounded)`, `id((circle))`,
//! `id))bang((`, `id)cloud(`, `id{{hexagon}}`; labels may be `"quoted"` or
//! markdown strings (`` "`**bold**`" ``). `::icon(fa fa-book)` and
//! `:::class1 class2` on their own line decorate the previous node. Icons are
//! accepted but not drawn (no icon fonts in this renderer).
//!
//! Layout: mermaid 12.3's own, cytoscape's CoSE-Bilkent force-directed layout
//! (see [`crate::layout::force::cose`]). For a tree it starts from a
//! deterministic radial placement, so this reproduces mermaid's node positions.

use super::graph::Shape;
use super::shapes;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::force::cose;
use crate::scene::{Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{BreakRule, TextBlock, line_width};
use crate::theme::Palette;

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
    /// Colour section (`None` for the root).
    pub section: Option<usize>,
    /// What `getBBox()` reports for the drawn node, relative to its centre (a
    /// bang or cloud outline bulges past its nominal box, off-centre).
    pub bbox: Rect,
    /// Outline of a bang or cloud node around its centre.
    pub outline: Option<Vec<PathCmd>>,
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
fn wrap(text: &str, font: f64, width: f64) -> (Vec<String>, f64) {
    let raw = crate::text::markdown_lines(text);
    let natural = raw
        .iter()
        .map(|l| line_width(l, font, false))
        .fold(0.0, f64::max);
    let lines = crate::text::wrap_lines(&raw, width, font, false, BreakRule::Browser);
    let tw = if natural > width {
        crate::text::html_wrap_width(&raw, width, font, false)
    } else {
        // An HTML label's box: layout units (1/64 px).
        TextBlock::new(lines.clone(), font, false).html().width
    };
    (lines, tw)
}

/// Label widths: `mindmap.maxNodeWidth` and `flowchart.wrappingWidth`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    pub max_node: f64,
    pub wrapping: f64,
}

/// A node's label lines, nominal size, drawn bounding box (around the centre)
/// and, for bang and cloud nodes, outline.
fn measure(n: &Node, font: f64, padding: f64, widths: Widths) -> Placed {
    // mindmapRenderer (12.3) zeroes `width` of rect, rounded and hexagon nodes, so
    // labelHelper wraps those at `flowchart.wrappingWidth`; the rest at maxNodeWidth.
    let width = match n.shape {
        NodeShape::Square | NodeShape::Rounded | NodeShape::Hexagon => widths.wrapping,
        _ => widths.max_node,
    };
    let (lines, tw) = wrap(&n.label, font, width);
    let tb = TextBlock::new(lines.clone(), font, false);
    let th = tb.height.max(font * 1.5);
    let mut outline = None;
    let (w, h) = match n.shape {
        // defaultMindmapNode.ts: w = tw + 8·(p/2), h = th + 2·(p/2).
        NodeShape::Default => (tw + 4.0 * padding, th + padding),
        // mindmapRenderer (12.3) sets the padding of rect nodes to 10 and of rounded
        // ones to 15; `rect` is tw + 4p × th + 2p, `rounded` tw + 2p × th + 2p.
        NodeShape::Square => (tw + 40.0, th + 20.0),
        NodeShape::Rounded => (tw + 30.0, th + 30.0),
        // mindmapDb doubles the hexagon's padding: h = th + p', w = tw + p' + h/2.
        NodeShape::Hexagon => {
            let h = th + 2.0 * padding;
            (tw + 2.0 * padding + h / 2.0, h)
        }
        // mindmapCircle → circle.ts with options.padding: r = hypot(tw, th)/2 + p.
        NodeShape::Circle => {
            let d = tw.hypot(th) + 2.0 * padding;
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
        section: None,
        bbox,
        outline,
    }
}

/// Lay out the tree as mermaid 12.3 does: cytoscape's CoSE-Bilkent over the
/// nodes' `getBBox()` sizes (single precision, as SVG reports them). Returns
/// one `Placed` per node (same indices as `mm.nodes`), centred in cytoscape's
/// coordinates.
pub fn layout(mm: &Mindmap, font: f64, padding: f64, widths: Widths) -> Vec<Placed> {
    let mut placed: Vec<Placed> = mm
        .nodes
        .iter()
        .map(|nd| measure(nd, font, padding, widths))
        .collect();
    if placed.is_empty() {
        return placed;
    }
    // Sections: index of the root child each node descends from.
    for (s, &c) in mm.nodes[0].children.iter().enumerate() {
        let mut stack = vec![c];
        while let Some(k) = stack.pop() {
            placed[k].section = Some(s);
            stack.extend(mm.nodes[k].children.iter().copied());
        }
    }
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

/// Render a mindmap.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let mm = parse(body, title)?;
    let padding = cfg.section_f64("mindmap", "padding", 10.0);
    let widths = Widths {
        max_node: cfg.section_f64("mindmap", "maxNodeWidth", 200.0),
        wrapping: cfg.section_f64("flowchart", "wrappingWidth", 120.0),
    };
    let placed = layout(&mm, pal.font_size, padding, widths);
    Ok(draw(&mm, &placed, pal, padding))
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
pub fn draw(mm: &Mindmap, placed: &[Placed], pal: &Palette, padding: f64) -> Scene {
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
            edges.push(Element::classed_shape(
                Geometry::Path(geom::basis_curve(&pts)),
                Paint::stroke(&c, edge_width(depth)),
                format!(
                    "edge section-edge-{} edge-depth-{depth}",
                    p.section.unwrap_or(0)
                ),
            ));
        }
        let (fill, text_color, line) = section_colors(pal, p.section);
        let r = p.rect();
        let b = p.bbox;
        bounds.add_rect(Rect::new(p.center.x + b.x, p.center.y + b.y, b.w, b.h));
        let mut els = match &p.outline {
            Some(cmds) => vec![Element::shape(
                Geometry::Path(cmds.clone()).translated(p.center.x, p.center.y),
                Paint::fill(&fill),
            )],
            None => shape_elements(nd.shape, r, &fill, &line),
        };
        els.push(Element::text(Text::centered(
            r.center(),
            &p.lines,
            font,
            font * 1.5,
            &text_color,
        )));
        let section_class = match p.section {
            None => "section-root".to_string(),
            Some(s) => format!("section-{s}"),
        };
        let mut class = format!("mindmap-node {section_class}");
        for c in &nd.classes {
            class.push(' ');
            class.push_str(c);
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
    Scene {
        kind: DiagramKind::Mindmap,
        // setupViewPortForSVG: the drawing's bounding box plus `mindmap.padding`.
        view_box: bounds.rect().pad(padding),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Stroke width of `.edge-depth-{depth}` (classic look: `17 - 3·(depth + 1)` for
/// the 12 theme colours). Past them, or where that width turns negative and the
/// declaration is dropped, `.edge`'s 3 applies.
fn edge_width(depth: usize) -> f64 {
    let i = depth + 1;
    let w = 17.0 - 3.0 * i as f64;
    if i < 12 && w > 0.0 { w } else { 3.0 }
}

fn shape_elements(shape: NodeShape, r: Rect, fill: &Color, line: &Color) -> Vec<Element> {
    let c = r.center();
    let paint = Paint::fill(fill);
    match shape {
        NodeShape::Default => {
            let rr = 5.0;
            let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
            let pi = std::f64::consts::PI;
            let mut cmds = vec![
                PathCmd::MoveTo(Point::new(x0, y1)),
                PathCmd::LineTo(Point::new(x0, y0 + rr)),
            ];
            cmds.extend(geom::arc_cubics(
                Point::new(x0 + rr, y0 + rr),
                rr,
                pi,
                1.5 * pi,
            ));
            cmds.push(PathCmd::LineTo(Point::new(x1 - rr, y0)));
            cmds.extend(geom::arc_cubics(
                Point::new(x1 - rr, y0 + rr),
                rr,
                1.5 * pi,
                2.0 * pi,
            ));
            cmds.push(PathCmd::LineTo(Point::new(x1, y1)));
            cmds.push(PathCmd::Close);
            vec![
                Element::classed_shape(Geometry::Path(cmds), paint, "node-bkg node-no-border"),
                Element::line(
                    Point::new(x0, y1),
                    Point::new(x1, y1),
                    Paint::stroke(line, 3.0),
                ),
            ]
        }
        // Bang and cloud nodes are drawn from their outline.
        NodeShape::Square | NodeShape::Bang | NodeShape::Cloud => {
            vec![Element::rect(r, 0.0, paint)]
        }
        NodeShape::Rounded => vec![Element::rect(r, 5.0, paint)],
        NodeShape::Circle => vec![Element::shape(Geometry::Circle { c, r: r.w / 2.0 }, paint)],
        NodeShape::Hexagon => {
            let m = r.h / 4.0;
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
}
