//! `mindmap` diagrams.
//!
//! Syntax: an indentation-based tree, one node per line. Node forms:
//! `text` (default), `id[square]`, `id(rounded)`, `id((circle))`,
//! `id))bang((`, `id)cloud(`, `id{{hexagon}}`; labels may be `"quoted"` or
//! markdown strings (`` "`**bold**`" ``). `::icon(fa fa-book)` and
//! `:::class1 class2` on their own line decorate the previous node. Icons are
//! accepted but not drawn (no icon fonts in this renderer).
//!
//! Layout (deterministic, unlike mermaid's force layout): the root sits in the
//! centre, its children are split into a right and a left group of roughly
//! equal subtree height, and each side is a tidy horizontal tree where every
//! subtree owns a vertical band — so nodes can never overlap.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
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
        width
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

fn measure(n: &Node, font: f64, padding: f64, widths: Widths) -> (Vec<String>, f64, f64) {
    // mindmapRenderer (12.3) zeroes `width` of rect, rounded and hexagon nodes, so
    // labelHelper wraps those at `flowchart.wrappingWidth`; the rest at maxNodeWidth.
    let width = match n.shape {
        NodeShape::Square | NodeShape::Rounded | NodeShape::Hexagon => widths.wrapping,
        _ => widths.max_node,
    };
    let (lines, tw) = wrap(&n.label, font, width);
    let tb = TextBlock::new(lines.clone(), font, false);
    let th = tb.height.max(font * 1.5);
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
        NodeShape::Cloud => (tw + 4.0 * padding, th + 3.0 * padding),
        NodeShape::Bang => (tw + 5.0 * padding, th + 4.0 * padding),
    };
    (lines, w, h)
}

const H_GAP: f64 = 70.0;
const V_GAP: f64 = 18.0;

/// Lay out the tree. Returns one `Placed` per node (same indices as `mm.nodes`).
pub fn layout(mm: &Mindmap, font: f64, padding: f64, widths: Widths) -> Vec<Placed> {
    let n = mm.nodes.len();
    let mut placed: Vec<Placed> = mm
        .nodes
        .iter()
        .map(|nd| {
            let (lines, w, h) = measure(nd, font, padding, widths);
            Placed {
                center: Point::default(),
                w,
                h,
                lines,
                section: None,
            }
        })
        .collect();
    if n == 0 {
        return placed;
    }
    // Subtree band heights (post-order: children always have larger indices).
    let mut band = vec![0.0; n];
    for i in (0..n).rev() {
        let kids = &mm.nodes[i].children;
        let sum: f64 = kids.iter().map(|&k| band[k]).sum::<f64>()
            + V_GAP * kids.len().saturating_sub(1) as f64;
        band[i] = placed[i].h.max(sum);
    }
    // Sections: index of the root child each node descends from.
    for (s, &c) in mm.nodes[0].children.iter().enumerate() {
        let mut stack = vec![c];
        while let Some(k) = stack.pop() {
            placed[k].section = Some(s);
            stack.extend(mm.nodes[k].children.iter().copied());
        }
    }
    // Split root children: right side gets the first ~half of the total height.
    let kids = &mm.nodes[0].children;
    let total: f64 = kids.iter().map(|&k| band[k]).sum();
    let mut right = Vec::new();
    let mut left = Vec::new();
    let mut acc = 0.0;
    for &k in kids {
        if right.is_empty() || acc + band[k] / 2.0 <= total / 2.0 {
            right.push(k);
            acc += band[k];
        } else {
            left.push(k);
        }
    }
    let root_half = placed[0].w / 2.0;
    place_group(mm, &mut placed, &band, &right, root_half, 1.0, 0.0);
    place_group(mm, &mut placed, &band, &left, root_half, -1.0, 0.0);
    placed
}

fn place_group(
    mm: &Mindmap,
    placed: &mut [Placed],
    band: &[f64],
    group: &[usize],
    edge_x: f64,
    dir: f64,
    center_y: f64,
) {
    let total: f64 =
        group.iter().map(|&k| band[k]).sum::<f64>() + V_GAP * group.len().saturating_sub(1) as f64;
    let mut y = center_y - total / 2.0;
    for &k in group {
        let cy = y + band[k] / 2.0;
        let cx = (edge_x + H_GAP + placed[k].w / 2.0) * dir;
        placed[k].center = Point::new(cx, cy);
        let next_edge = edge_x + H_GAP + placed[k].w;
        let kids = mm.nodes[k].children.clone();
        place_group(mm, placed, band, &kids, next_edge, dir, cy);
        y += band[k] + V_GAP;
    }
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
    Ok(draw(&mm, &placed, pal))
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
pub fn draw(mm: &Mindmap, placed: &[Placed], pal: &Palette) -> Scene {
    let font = pal.font_size;
    let mut bounds = Bounds::default();
    let mut edges = Vec::new();
    let mut nodes = Vec::new();
    for (i, nd) in mm.nodes.iter().enumerate() {
        let p = &placed[i];
        if let Some(par) = nd.parent {
            let a = placed[par].center;
            let b = p.center;
            let mx = (a.x + b.x) / 2.0;
            let cmds = vec![
                PathCmd::MoveTo(a),
                PathCmd::CubicTo(Point::new(mx, a.y), Point::new(mx, b.y), b),
            ];
            let depth = depth_of(mm, i);
            let width = (17.0 - 3.0 * (depth as f64 + 1.0)).max(2.0);
            let (c, _, _) = section_colors(pal, p.section);
            edges.push(Element::classed_shape(
                Geometry::Path(cmds),
                Paint::stroke(&c, width),
                format!(
                    "edge section-edge-{} edge-depth-{depth}",
                    p.section.unwrap_or(0)
                ),
            ));
        }
        let (fill, text_color, line) = section_colors(pal, p.section);
        let r = p.rect();
        bounds.add_rect(r.pad(4.0));
        let mut els = shape_elements(nd.shape, r, &fill, &line, pal);
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
        view_box: bounds.rect().pad(12.0),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn depth_of(mm: &Mindmap, mut i: usize) -> usize {
    let mut d = 0;
    while let Some(p) = mm.nodes[i].parent {
        d += 1;
        i = p;
    }
    d
}

fn shape_elements(
    shape: NodeShape,
    r: Rect,
    fill: &Color,
    line: &Color,
    pal: &Palette,
) -> Vec<Element> {
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
        NodeShape::Square => vec![Element::rect(r, 0.0, paint)],
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
        NodeShape::Cloud => vec![Element::shape(Geometry::Path(cloud(r)), paint)],
        NodeShape::Bang => {
            let _ = pal;
            vec![Element::shape(Geometry::Path(bang(r)), paint)]
        }
    }
}

/// Scalloped outline: bumps (semicircles) along each edge of `r`.
fn cloud(r: Rect) -> Vec<PathCmd> {
    let pi = std::f64::consts::PI;
    let nx = ((r.w / 30.0).round() as usize).max(2);
    let ny = ((r.h / 30.0).round() as usize).max(1);
    let (sx, sy) = (r.w / nx as f64, r.h / ny as f64);
    let mut cmds = vec![PathCmd::MoveTo(Point::new(r.x, r.y))];
    for i in 0..nx {
        let cx = r.x + sx * (i as f64 + 0.5);
        cmds.extend(geom::elliptic_arc_cubics(
            Point::new(cx, r.y),
            sx / 2.0,
            sx / 3.0,
            pi,
            2.0 * pi,
        ));
    }
    for i in 0..ny {
        let cy = r.y + sy * (i as f64 + 0.5);
        cmds.extend(geom::elliptic_arc_cubics(
            Point::new(r.right(), cy),
            sy / 3.0,
            sy / 2.0,
            -0.5 * pi,
            0.5 * pi,
        ));
    }
    for i in (0..nx).rev() {
        let cx = r.x + sx * (i as f64 + 0.5);
        cmds.extend(geom::elliptic_arc_cubics(
            Point::new(cx, r.bottom()),
            sx / 2.0,
            sx / 3.0,
            0.0,
            pi,
        ));
    }
    for i in (0..ny).rev() {
        let cy = r.y + sy * (i as f64 + 0.5);
        cmds.extend(geom::elliptic_arc_cubics(
            Point::new(r.x, cy),
            sy / 3.0,
            sy / 2.0,
            0.5 * pi,
            1.5 * pi,
        ));
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// Explosion outline: inward-curving spikes around the box.
fn bang(r: Rect) -> Vec<PathCmd> {
    let c = r.center();
    let n = 16;
    let (rx, ry) = (r.w / 2.0, r.h / 2.0);
    let pt = |a: f64, k: f64| Point::new(c.x + rx * k * a.cos(), c.y + ry * k * a.sin());
    let step = std::f64::consts::TAU / n as f64;
    let mut cmds = vec![PathCmd::MoveTo(pt(0.0, 1.0))];
    for i in 0..n {
        let a0 = step * i as f64;
        let a1 = a0 + step;
        let mid = pt((a0 + a1) / 2.0, 0.78);
        cmds.push(PathCmd::CubicTo(mid, mid, pt(a1, 1.0)));
    }
    cmds.push(PathCmd::Close);
    cmds
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

    #[test]
    fn layout_has_no_overlaps_and_both_sides() {
        let mm = parse(SRC, None).unwrap();
        let p = layout(&mm, 16.0, 10.0, W);
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                let (a, b) = (p[i].rect(), p[j].rect());
                assert!(!a.intersects(&b), "{i} {:?} overlaps {j} {:?}", a, b);
            }
        }
        let right = mm.nodes[0]
            .children
            .iter()
            .filter(|&&k| p[k].center.x > 0.0)
            .count();
        let left = mm.nodes[0]
            .children
            .iter()
            .filter(|&&k| p[k].center.x < 0.0)
            .count();
        assert!(right > 0 && left > 0);
        // Wrapped long label stays within maxNodeWidth (+ padding).
        assert!(p.iter().all(|n| n.w <= 200.0 + 60.0));
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
