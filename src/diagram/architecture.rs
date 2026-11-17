//! `architecture-beta` diagrams.
//!
//! Supported syntax (mermaid 11): `group id(icon)[Title] in parent`,
//! `service id(icon)[Title] in group`, text icons `service id("txt")[Title]`,
//! `junction id in group`, edges `a:R -- L:b`, arrows `a:R --> L:b`,
//! `a:R <--> L:b`, labelled edges `a:R -[Label]- L:b`, group modifiers
//! `a{group}:B --> T:b{group}`, and `title` (parsed; mermaid 12.3 does not
//! draw it).
//!
//! Layout: mermaid's own: cytoscape-fcose constrained by the port sides
//! (see [`super::architecture_fcose`]), which reproduces its node positions;
//! edges are drawn as svgDraw.ts does, from cytoscape's endpoints and
//! midpoints. Icons are drawn as simple generic glyphs.

use super::architecture_fcose::{self, ArchConfig, Placement};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, Point, Rect};
use crate::json::Json;
use crate::layout::force::fcose;
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;

/// A port side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    L,
    R,
    T,
    B,
}

impl Side {
    fn parse(c: char) -> Option<Side> {
        match c {
            'L' => Some(Side::L),
            'R' => Some(Side::R),
            'T' => Some(Side::T),
            'B' => Some(Side::B),
            _ => None,
        }
    }
    /// Outward unit vector.
    fn out(self) -> Point {
        match self {
            Side::L => Point::new(-1.0, 0.0),
            Side::R => Point::new(1.0, 0.0),
            Side::T => Point::new(0.0, -1.0),
            Side::B => Point::new(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Service {
        icon: Option<String>,
        icon_text: Option<String>,
        title: Option<String>,
    },
    Junction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArchNode {
    pub id: String,
    pub kind: NodeKind,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeEnd {
    pub node: usize,
    pub side: Side,
    /// `{group}` modifier: attach to the enclosing group instead of the node.
    pub group: bool,
    /// Arrowhead pointing into this end.
    pub arrow: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArchEdge {
    pub lhs: EdgeEnd,
    pub rhs: EdgeEnd,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Architecture {
    pub title: Option<String>,
    pub groups: Vec<Group>,
    pub nodes: Vec<ArchNode>,
    pub edges: Vec<ArchEdge>,
    /// `align row|column a b c` layout hints.
    pub aligns: Vec<Align>,
}

/// `align row a b ...` (same y) or `align column a b ...` (same x).
#[derive(Debug, Clone, PartialEq)]
pub struct Align {
    pub row: bool,
    pub members: Vec<usize>,
}

/// Parse and render an `architecture-beta` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    Ok(draw(&d, cfg, pal))
}

// ---------------------------------------------------------------- parsing

/// `id`, `(icon)`, `[title]`, `in parent` after a `group` / `service` keyword.
struct Decl {
    id: String,
    icon: Option<String>,
    icon_text: Option<String>,
    title: Option<String>,
    parent: Option<String>,
}

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn parse_decl(s: &str, raw: &str, line: usize, allow_icon: bool) -> Result<Decl> {
    let len: usize = s
        .chars()
        .take_while(|&c| is_id_char(c))
        .map(char::len_utf8)
        .sum();
    if len == 0 {
        return Err(Error::parse(line, format!("Expecting an id, got '{s}'")));
    }
    let id = s[..len].to_string();
    let mut rest = s[len..].trim_start();
    let (mut icon, mut title, mut parent) = (None, None, None);
    let icon_text = None;
    if allow_icon {
        if let Some(r) = rest.strip_prefix('(') {
            let end = r
                .find(')')
                .ok_or_else(|| Error::parse(line, "Expecting ')' to close the icon"))?;
            let inner = &r[..end];
            // arch.langium: `terminal ARCH_ICON: /\([\w-:]+\)/`. Anything else (e.g. the
            // older quoted text icons) leaves `(` unmatched, which the lexer skips.
            if inner.is_empty()
                || !inner
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':')
            {
                let col = raw.find(s).map_or(0, |o| o + len) + 1;
                return Err(Error::parse(
                    line,
                    format!(
                        "Lexer error on line {line}, column {col}: unexpected character: ->(<-"
                    ),
                ));
            }
            icon = Some(inner.to_string());
            rest = r[end + 1..].trim_start();
        }
        if let Some(r) = rest.strip_prefix('[') {
            let end = r
                .rfind(']')
                .ok_or_else(|| Error::parse(line, "Expecting ']' to close the title"))?;
            let t = r[..end].trim();
            let t = t
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(t);
            title = Some(t.to_string());
            rest = r[end + 1..].trim_start();
        }
    }
    if let Some(r) = rest.strip_prefix("in") {
        if r.starts_with(char::is_whitespace) {
            let p = r.trim();
            if p.is_empty() || !p.chars().all(is_id_char) {
                return Err(Error::parse(line, format!("Invalid parent group '{p}'")));
            }
            parent = Some(p.to_string());
            rest = "";
        }
    }
    if !rest.trim().is_empty() {
        return Err(Error::parse(
            line,
            format!("Unexpected '{}' in declaration of '{id}'", rest.trim()),
        ));
    }
    Ok(Decl {
        id,
        icon,
        icon_text,
        title,
        parent,
    })
}

/// Which declaration kind a statement is (for mermaid's error wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclKind {
    Group,
    Service,
    Junction,
}

impl DeclKind {
    fn name(self) -> &'static str {
        match self {
            DeclKind::Group => "group",
            DeclKind::Service => "service",
            DeclKind::Junction => "junction",
        }
    }
}

/// Parse an architecture diagram body.
///
/// Like mermaid's langium-based parser, statements are collected first and
/// then applied by kind: all groups, then services, junctions, edges and
/// `align` hints. So a service may sit `in` a group declared further down,
/// and an edge may name a service declared after it.
pub fn parse(body: &str, title: Option<String>) -> Result<Architecture> {
    let mut d = Architecture {
        title,
        ..Architecture::default()
    };
    let mut header = false;
    let mut decls: Vec<(DeclKind, Decl, usize)> = Vec::new();
    let mut edges: Vec<(&str, usize)> = Vec::new();
    let mut aligns: Vec<(bool, Vec<&str>, usize)> = Vec::new();
    for (idx, raw) in body.lines().enumerate() {
        let line = idx + 1;
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        if !header {
            if s != "architecture-beta" && s != "architecture" {
                return Err(Error::parse(
                    line,
                    format!("{s}\nExpecting 'architecture-beta'"),
                ));
            }
            header = true;
            continue;
        }
        let (kw, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        let rest = rest.trim();
        match kw {
            "title" => {
                d.title = Some(rest.to_string()).filter(|t| !t.is_empty());
            }
            "group" => decls.push((DeclKind::Group, parse_decl(rest, raw, line, true)?, line)),
            "service" => decls.push((DeclKind::Service, parse_decl(rest, raw, line, true)?, line)),
            "junction" => decls.push((
                DeclKind::Junction,
                parse_decl(rest, raw, line, false)?,
                line,
            )),
            "align" => {
                let mut words = rest.split_whitespace();
                let row = match words.next() {
                    Some("row") => true,
                    Some("column") => false,
                    other => {
                        return Err(Error::parse(
                            line,
                            format!(
                                "{s}\nExpecting 'row' or 'column' after 'align', got '{}'",
                                other.unwrap_or("")
                            ),
                        ));
                    }
                };
                aligns.push((row, words.collect(), line));
            }
            _ => edges.push((s, line)),
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'architecture-beta'"));
    }
    for want in [DeclKind::Group, DeclKind::Service, DeclKind::Junction] {
        for (kind, decl, line) in decls.iter().filter(|x| x.0 == want) {
            add_decl(&mut d, *kind, decl, *line)?;
        }
    }
    for (s, line) in edges {
        let e = parse_edge(&d, s, line)?;
        d.edges.push(e);
    }
    for (row, members, line) in aligns {
        let dir = if row { "row" } else { "column" };
        if members.len() < 2 {
            return Err(Error::parse(
                line,
                format!(
                    "An align directive requires at least two members; got {}",
                    members.len()
                ),
            ));
        }
        let mut ids: Vec<usize> = Vec::new();
        for m in members {
            let n = d.nodes.iter().position(|n| n.id == m).ok_or_else(|| {
                Error::parse(
                    line,
                    format!("align {dir} references [{m}], which is not a service or junction"),
                )
            })?;
            if ids.contains(&n) {
                return Err(Error::parse(
                    line,
                    format!("align {dir} lists [{m}] more than once"),
                ));
            }
            ids.push(n);
        }
        d.aligns.push(Align { row, members: ids });
    }
    Ok(d)
}

fn add_decl(d: &mut Architecture, kind: DeclKind, decl: &Decl, line: usize) -> Result<()> {
    let what = kind.name();
    let id = &decl.id;
    if let Some(other) = registered(d, id) {
        return Err(Error::parse(
            line,
            format!("The {what} id [{id}] is already in use by another {other}"),
        ));
    }
    let parent = match decl.parent.as_deref() {
        None => None,
        Some(p) if p == id => {
            return Err(Error::parse(
                line,
                format!("The {what} [{id}] cannot be placed within itself"),
            ));
        }
        Some(p) => match registered(d, p) {
            None => {
                return Err(Error::parse(
                    line,
                    format!(
                        "The {what} [{id}]'s parent does not exist. Please make sure the parent is created before this {what}"
                    ),
                ));
            }
            Some("node") => {
                return Err(Error::parse(
                    line,
                    format!("The {what} [{id}]'s parent is not a group"),
                ));
            }
            Some(_) => d.groups.iter().position(|g| g.id == p),
        },
    };
    match kind {
        DeclKind::Group => d.groups.push(Group {
            id: id.clone(),
            icon: decl.icon.clone().or(decl.icon_text.clone()),
            title: decl.title.clone(),
            parent,
        }),
        DeclKind::Service => d.nodes.push(ArchNode {
            id: id.clone(),
            kind: NodeKind::Service {
                icon: decl.icon.clone(),
                icon_text: decl.icon_text.clone(),
                title: decl.title.clone(),
            },
            parent,
        }),
        DeclKind::Junction => d.nodes.push(ArchNode {
            id: id.clone(),
            kind: NodeKind::Junction,
            parent,
        }),
    }
    Ok(())
}

/// mermaid's `registeredIds` kind of an id: `"group"` or `"node"`.
fn registered(d: &Architecture, id: &str) -> Option<&'static str> {
    if d.groups.iter().any(|g| g.id == id) {
        Some("group")
    } else if d.nodes.iter().any(|n| n.id == id) {
        Some("node")
    } else {
        None
    }
}

/// `lhs{group}?:S <?(--|-[label]-)>? S:rhs{group}?`
fn parse_edge(d: &Architecture, s: &str, line: usize) -> Result<ArchEdge> {
    let bad = || {
        Error::parse(
            line,
            format!("{s}\nExpecting a group, service, junction or edge (e.g. 'a:R --> L:b')"),
        )
    };
    let (lhs, rest) = s.split_once(':').ok_or_else(bad)?;
    let lhs = lhs.trim();
    let (lhs_id, lhs_group) = match lhs.strip_suffix("{group}") {
        Some(x) => (x.trim(), true),
        None => (lhs, false),
    };
    let mut chars = rest.trim_start();
    let lhs_side = chars.chars().next().and_then(Side::parse).ok_or_else(|| {
        Error::parse(
            line,
            format!("{s}\nExpecting a side (L, R, T or B) after ':'"),
        )
    })?;
    chars = chars[1..].trim_start();
    let lhs_arrow = if let Some(r) = chars.strip_prefix('<') {
        chars = r;
        true
    } else {
        false
    };
    let mut label = None;
    if let Some(r) = chars.strip_prefix("--") {
        chars = r;
    } else if let Some(r) = chars.strip_prefix("-[") {
        let end = r
            .find("]-")
            .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting ']-' after the label")))?;
        let t = r[..end].trim();
        label = Some(
            t.strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .unwrap_or(t)
                .to_string(),
        );
        chars = &r[end + 2..];
    } else {
        return Err(Error::parse(
            line,
            format!("{s}\nExpecting '--' or '-[label]-'"),
        ));
    }
    let rhs_arrow = if let Some(r) = chars.strip_prefix('>') {
        chars = r;
        true
    } else {
        false
    };
    chars = chars.trim_start();
    let rhs_side = chars.chars().next().and_then(Side::parse).ok_or_else(|| {
        Error::parse(
            line,
            format!("{s}\nExpecting a side (L, R, T or B) before ':'"),
        )
    })?;
    let rhs = chars[1..]
        .trim_start()
        .strip_prefix(':')
        .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting ':' after the side")))?
        .trim();
    let (rhs_id, rhs_group) = match rhs.strip_suffix("{group}") {
        Some(x) => (x.trim(), true),
        None => (rhs, false),
    };
    let find = |id: &str, hand: &str| {
        d.nodes.iter().position(|n| n.id == id).ok_or_else(|| {
            Error::parse(
                line,
                format!(
                    "The {hand} id [{id}] does not yet exist. Please create the service/group before declaring an edge to it."
                ),
            )
        })
    };
    let (lhs_node, rhs_node) = (find(lhs_id, "left-hand")?, find(rhs_id, "right-hand")?);
    let (gl, gr) = (d.nodes[lhs_node].parent, d.nodes[rhs_node].parent);
    let same_group = gl.is_some() && gl == gr;
    for (flag, hand, id) in [
        (lhs_group, "left-hand", lhs_id),
        (rhs_group, "right-hand", rhs_id),
    ] {
        if flag && same_group {
            return Err(Error::parse(
                line,
                format!(
                    "The {hand} id [{id}] is modified to traverse the group boundary, but the edge does not pass through two groups."
                ),
            ));
        }
    }
    Ok(ArchEdge {
        lhs: EdgeEnd {
            node: lhs_node,
            side: lhs_side,
            group: lhs_group,
            arrow: lhs_arrow,
        },
        rhs: EdgeEnd {
            node: rhs_node,
            side: rhs_side,
            group: rhs_group,
            arrow: rhs_arrow,
        },
        label,
    })
}

// ---------------------------------------------------------------- layout

/// Positioned diagram: full item boxes plus the icon box of each node.
struct Layout {
    /// Icon plus title: what the tests check against the groups (drawing
    /// takes the titles' text boxes instead).
    #[cfg_attr(not(test), allow(dead_code))]
    node_box: Vec<Rect>,
    icon_box: Vec<Rect>,
    group_box: Vec<Rect>,
}

fn service_label(d: &Architecture, n: usize, font: f64) -> Option<TextBlock> {
    match &d.nodes[n].kind {
        NodeKind::Service { title: Some(t), .. } => {
            Some(TextBlock::label(t, font)).filter(|b| !b.is_empty())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- drawing

/// `archGroupBorderColor`: the theme constructor's `primaryBorderColor` (the dark
/// themes' `invert(background)`), and `#000` in theme-base.js.
fn group_border_color(pal: &Palette) -> Color {
    if pal.theme == crate::theme::ThemeName::Base {
        return Color::new("#000");
    }
    pal.primary_border.clone()
}

/// svgDraw.ts: arrowSize = iconSize / 6, base as wide as it is long, tip 2px
/// past the edge end (`ArchitectureDirectionArrowShift`).
/// The arrow's corners also widen the drawing's box (`getBBox`).
/// drawEdges' label: `createText` wrapped at a width from the edge's extent,
/// its tspans 1em, 2.1em, ... below the midpoint. An edge between two
/// horizontal sides keeps it level, between two vertical sides turns it
/// -90° about the midpoint, and an elbow (one of each) tilts it 45° and
/// moves it by its screen boxes before and after the turn
/// (`getBoundingClientRect`, at scale 1), with `dominant-baseline: auto`.
fn edge_label(
    label: &str,
    pts: &[Point],
    from: Side,
    to: Side,
    font: f64,
    fill: &Color,
    bounds: &mut Bounds,
) -> Vec<Element> {
    let horizontal = |s: Side| matches!(s, Side::L | Side::R);
    let (start, mid, end) = (pts[0], pts[1], pts[pts.len() - 1]);
    let width = match (horizontal(from), horizontal(to)) {
        (true, true) => (start.x - end.x).abs(),
        (false, false) => (start.y - end.y).abs() / 1.5,
        _ => (start.x - end.x).abs() / 2.0,
    };
    let lines = crate::text::svg_wrap_lines(&crate::text::label_lines(label), width, font, false);
    let b = TextBlock::svg(lines, font, false);
    if b.is_empty() {
        return Vec::new();
    }
    let middle = crate::text::baseline_shift(crate::text::Baseline::Middle, font);
    let ascent = (0.905 * font).round();
    // The text box (as getBBox / getBoundingClientRect see it) about the
    // group's origin: from the first line's ascent to the block's height.
    // The text box about the anchor (lines centred on it).
    let ink = b
        .lines
        .iter()
        .map(|l| {
            let half = crate::text::line_width(l, font, false) / 2.0;
            let (lo, hi) = crate::text::svg_bbox_extent(l, font, false);
            (lo - half, hi - half)
        })
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, (lo, hi)| {
            (a.0.min(lo), a.1.max(hi))
        });
    let text = |x: f64, y: f64| Text {
        x,
        y,
        lines: b.lines.clone(),
        line_height: font * crate::text::SVG_LINE_HEIGHT,
        font_size: font,
        bold: false,
        weight: None,
        anchor: Anchor::Middle,
        fill: fill.clone(),
    };
    // Corners of the text box placed with its origin at `o`, turned by `deg`
    // about `about`.
    let mut add = |o: Point, baseline: f64, deg: f64, about: Point| {
        let (x0, x1) = (o.x + ink.0, o.x + ink.1);
        let top = o.y + baseline - ascent;
        let (sin, cos) = deg.to_radians().sin_cos();
        for (x, y) in [
            (x0, top),
            (x1, top),
            (x0, top + b.height),
            (x1, top + b.height),
        ] {
            let (dx, dy) = (x - about.x, y - about.y);
            bounds.add_point(Point::new(
                about.x + dx * cos - dy * sin,
                about.y + dx * sin + dy * cos,
            ));
        }
    };
    match (horizontal(from), horizontal(to)) {
        (true, true) => {
            add(mid, font + middle, 0.0, mid);
            vec![Element::text(text(mid.x, mid.y + font + middle))]
        }
        (false, false) => {
            add(mid, font + middle, -90.0, mid);
            vec![Element::Rotated {
                deg: -90.0,
                about: mid,
                children: vec![Element::text(text(mid.x, mid.y + font + middle))],
            }]
        }
        _ => {
            // getArchitectureDirectionXYFactors
            let (fx, fy) = match (from, to) {
                (Side::L, Side::T) | (Side::T, Side::L) => (1.0, 1.0),
                (Side::B, Side::L) | (Side::L, Side::B) => (1.0, -1.0),
                (Side::B, Side::R) | (Side::R, Side::B) => (-1.0, -1.0),
                _ => (-1.0, 1.0),
            };
            let (w0, h0) = (ink.1 - ink.0, b.height);
            // The box turned 45°: (w + h)·cos 45° each way.
            let turned = (w0 + h0) * std::f64::consts::FRAC_1_SQRT_2;
            let deg = -fx * fy * 45.0;
            let o = Point::new(
                mid.x + fx * turned / 2.0,
                mid.y - h0 / 2.0 + fy * turned / 2.0,
            );
            let about = Point::new(o.x, o.y + h0 / 2.0);
            // `dominant-baseline: auto`: the first baseline 1em down.
            add(o, font, deg, about);
            vec![Element::Rotated {
                deg,
                about,
                children: vec![Element::text(text(o.x, o.y + font))],
            }]
        }
    }
}

fn arrow(end: Point, dir: Point, color: &Color, icon: f64, bounds: &mut Bounds) -> Element {
    let size = icon / 6.0;
    let perp = Point::new(-dir.y, dir.x);
    let tip = end.plus(dir.scale(2.0));
    let base = tip.minus(dir.scale(size));
    let pts = [
        tip,
        base.plus(perp.scale(size / 2.0)),
        base.minus(perp.scale(size / 2.0)),
    ];
    for p in pts {
        bounds.add_point(p);
    }
    Element::classed_shape(
        Geometry::Path(geom::polygon(&pts)),
        Paint::fill(color),
        "arrow",
    )
}

/// The icon drawn on a service's (or group's) blue square: mermaid's own SVG
/// for a built-in name (`database`, `server`, `disk`, `internet`, `cloud`, also
/// as `mermaid-architecture:<name>`), nothing for `blank`, and `unknownIcon`'s
/// `?` for any other name (an icon pack mmdc was not given).
fn icon_glyph(icon: Option<&str>, r: Rect) -> Vec<Element> {
    let Some(icon) = icon else {
        return Vec::new();
    };
    let name = icon.strip_prefix("mermaid-architecture:").unwrap_or(icon);
    if name == "blank" {
        return Vec::new();
    }
    super::archicons::builtin(name, r).unwrap_or_else(|| vec![super::archicons::unknown(r)])
}

/// Boxes from mermaid's fcose placement: a service's icon has its top left
/// corner at the node's cytoscape position, and the groups' boxes are drawn
/// shifted by half an icon the same way (svgDraw.ts).
fn layout_from_placement(d: &Architecture, p: &Placement, font: f64, icon: f64) -> Layout {
    let half = icon / 2.0;
    let icon_box: Vec<Rect> = p
        .node_pos
        .iter()
        .map(|q| Rect::new(q.x, q.y, icon, icon))
        .collect();
    let node_box: Vec<Rect> = icon_box
        .iter()
        .enumerate()
        .map(|(i, &ib)| match service_label(d, i, font) {
            Some(b) => {
                let w = icon.max(b.width + 4.0);
                Rect::new(ib.center().x - w / 2.0, ib.y, w, icon + 6.0 + b.height)
            }
            None => ib,
        })
        .collect();
    let group_box: Vec<Rect> = p
        .group_bbox
        .iter()
        .map(|r| Rect::new(r.x + half, r.y + half, r.w, r.h))
        .collect();
    Layout {
        node_box,
        icon_box,
        group_box,
    }
}

/// mermaid's placement (see [`architecture_fcose::place`]) and the boxes
/// drawn from it; `font` is the labels' (the theme's) font size.
fn layout(d: &Architecture, font: f64, c: &ArchConfig) -> (Placement, Layout) {
    // cytoscape measures a label (a service's raw title or an edge's label,
    // one line) on a canvas in its default font, `Helvetica Neue, Helvetica,
    // sans-serif` (Inter, as mermaid-cli's environment serves Helvetica
    // Neue), rounded up; service titles at `architecture.fontSize`, edge
    // labels at cytoscape's 16px.
    let measure = |t: &str, edge: bool| {
        crate::text::inter::canvas_width(t, if edge { 16.0 } else { c.font_size }).ceil()
    };
    let placement = architecture_fcose::place(d, &measure, c);
    let lay = layout_from_placement(d, &placement, font, c.icon_size);
    (placement, lay)
}

/// The `architecture` config fields mermaid reads with `getConfigField`
/// (`ArchitectureDB.getConfig`: the defaults under the merged site config,
/// frontmatter and directives, latest first). fcose tests `randomize` for
/// JavaScript truthiness; `withSeededRandom` takes `seed >>> 0`.
pub fn arch_config(cfg: &MermaidConfig) -> ArchConfig {
    let d = ArchConfig::default();
    let num = |k: &str| cfg.section("architecture", k).and_then(Json::as_f64);
    // ECMAScript ToUint32.
    let uint32 = |v: f64| {
        if v.is_finite() {
            v.trunc().rem_euclid(4_294_967_296.0) as u32
        } else {
            0
        }
    };
    ArchConfig {
        icon_size: num("iconSize").unwrap_or(d.icon_size),
        padding: num("padding").unwrap_or(d.padding),
        font_size: num("fontSize").unwrap_or(d.font_size),
        ideal_mult: num("idealEdgeLengthMultiplier").unwrap_or(d.ideal_mult),
        elasticity: num("edgeElasticity").unwrap_or(d.elasticity),
        fcose: fcose::Options {
            randomize: cfg
                .section("architecture", "randomize")
                .map_or(d.fcose.randomize, Json::js_truthy),
            node_separation: num("nodeSeparation").unwrap_or(d.fcose.node_separation),
            seed: num("seed").map_or(d.fcose.seed, uint32),
            num_iter: num("numIter").map_or(d.fcose.num_iter, |n| n.max(0.0) as usize),
        },
    }
}

fn draw(d: &Architecture, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let font = pal.font_size;
    let ac = arch_config(cfg);
    let (placement, lay) = layout(d, font, &ac);
    let group_icon = ac.padding * 0.75;
    let icon_bg = Color::new("#087ebf");
    let mut bounds = Bounds::default();
    let mut group_els = Vec::new();
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    let mut node_els = Vec::new();

    // mermaid: `.node-bkg { stroke: archGroupBorderColor (= primaryBorderColor);
    // stroke-width: 2px; stroke-dasharray: 8 }`.
    let group_border = group_border_color(pal);
    for (gi, g) in d.groups.iter().enumerate() {
        let r = lay.group_box[gi];
        bounds.add_rect(r);
        let mut children = vec![Element::classed_shape(
            Geometry::Rect { rect: r, rx: 0.0 },
            Paint::stroke(&group_border, 2.0).dashed(vec![8.0]),
            "node-bkg",
        )];
        // Icon (padding * 0.75 square) in the top-left corner, title after it.
        // The title is SVG text (`createText`, tspans 1.1em apart, the first
        // baseline 1em down) from 2px below the box's top, alphabetic
        // (`dominant-baseline: start` is no value).
        let mut x = r.x + 4.0;
        let mut baseline = r.y + 2.0 + font;
        if let Some(icon) = &g.icon {
            let ir = Rect::new(r.x + 1.0, r.y + 1.0, group_icon, group_icon);
            children.push(Element::rect(ir, 0.0, Paint::fill(&icon_bg)));
            children.extend(icon_glyph(Some(icon), ir));
            x = ir.right() + 3.0;
            // svgDraw.ts: the title moves down `fontSize / 2 - 3` past an icon.
            baseline += ac.font_size / 2.0 - 3.0;
        }
        // createText wraps the title at the group's width.
        let wrapped = |t: &str, w: f64| {
            let lines = crate::text::svg_wrap_lines(&crate::text::label_lines(t), w, font, false);
            TextBlock::new(lines, font, false)
        };
        if let Some(t) = g.title.as_ref().map(|t| wrapped(t, r.w)) {
            if !t.is_empty() {
                children.push(Element::classed_text(
                    Text {
                        x,
                        y: baseline,
                        lines: t.lines.clone(),
                        line_height: font * crate::text::SVG_LINE_HEIGHT,
                        font_size: font,
                        bold: false,
                        weight: None,
                        anchor: Anchor::Start,
                        fill: pal.text_color.clone(),
                    },
                    "architecture-group-label",
                ));
            }
        }
        group_els.push(Element::Group {
            id: Some(format!("group-{}", g.id)),
            class: Some("architecture-group".into()),
            children,
        });
    }

    for (i, n) in d.nodes.iter().enumerate() {
        let ib = lay.icon_box[i];
        // The title (when any) adds its own text box below.
        bounds.add_rect(ib);
        let children = match &n.kind {
            // mermaid draws junctions as fully transparent icon-sized rects.
            NodeKind::Junction => vec![Element::shape(
                Geometry::Rect { rect: ib, rx: 0.0 },
                Paint::fill(&pal.line_color).with_opacity(0.0),
            )],
            NodeKind::Service {
                icon,
                icon_text,
                title,
            } => {
                let mut v = vec![Element::classed_shape(
                    // wrapIcon's `<rect width="80" height="80">`: square corners.
                    Geometry::Rect { rect: ib, rx: 0.0 },
                    Paint::fill(&icon_bg),
                    "node-bkg",
                )];
                if let Some(t) = icon_text {
                    let b = TextBlock::label(t, font);
                    v.push(Element::text(Text::centered(
                        ib.center(),
                        &b.lines,
                        font,
                        b.line_height,
                        &Color::new("#ffffff"),
                    )));
                } else {
                    v.extend(icon_glyph(icon.as_deref(), ib));
                }
                // createText wraps the title at 1.5 icon widths.
                let wrap = 1.5 * ac.icon_size;
                if let Some(b) = title.as_ref().map(|t| {
                    let lines = crate::text::svg_wrap_lines(
                        &crate::text::label_lines(t),
                        wrap,
                        font,
                        false,
                    );
                    TextBlock::new(lines, font, false)
                }) {
                    if !b.is_empty() {
                        // `createText` under the icon's bottom centre: tspans
                        // 1.1em apart from 1em down, on the group's inherited
                        // `dominant-baseline: middle`.
                        let lh = font * crate::text::SVG_LINE_HEIGHT;
                        let first = ib.bottom()
                            + font
                            + crate::text::baseline_shift(crate::text::Baseline::Middle, font);
                        let last = b.lines.last().map_or("", String::as_str);
                        let ascent = (0.905 * font).round();
                        // The viewBox is measured before it is set (at scale 1):
                        // Chromium boxes the text from its single-precision row
                        // positions (row `i` at `y = (1.1i - 0.1)em`, `dy =
                        // 1.1em`) and the unrounded `middle` shift (half the
                        // x-height), rounded out to the 1/64px layout unit
                        // (17.015625 tall at 16px, where the drawn text's box is
                        // 17); a descender's ink reaches whole pixels below the
                        // baseline as drawn (the shift rounded).
                        let row = |i: usize, shift: f64| {
                            let em = |k: f64| k as f32 * font as f32;
                            f64::from(em(i as f64 * 1.1 - 0.1) + em(1.1)) + shift
                        };
                        let raw = font * 1082.0 / 4096.0;
                        let drawn = first - ib.bottom() - font;
                        let lu = |v: f64, up: bool| {
                            let u = v * 64.0;
                            (if up { u.ceil() } else { u.floor() }) / 64.0
                        };
                        let n = b.lines.len() - 1;
                        let descent = (0.212 * font).round();
                        let below = crate::text::svg_line_box(font, last) - ascent;
                        let top = ib.bottom() + lu(row(0, raw) - ascent, false);
                        let bottom = ib.bottom()
                            + lu(row(n, raw) + descent, true).max(row(n, drawn) + below);
                        bounds.add_rect(Rect::new(
                            ib.center().x - b.width / 2.0,
                            top,
                            b.width,
                            bottom - top,
                        ));
                        v.push(Element::classed_text(
                            Text {
                                x: ib.center().x,
                                y: first,
                                lines: b.lines.clone(),
                                line_height: lh,
                                font_size: font,
                                bold: false,
                                weight: None,
                                anchor: Anchor::Middle,
                                fill: pal.text_color.clone(),
                            },
                            "architecture-service-label",
                        ));
                    }
                }
                v
            }
        };
        node_els.push(Element::Group {
            id: Some(format!("service-{}", n.id)),
            class: Some("architecture-service".into()),
            children,
        });
    }

    for (ei, e) in d.edges.iter().enumerate() {
        let pts = architecture_fcose::edge_points(d, &placement, e, &ac);
        let (p0, p1) = (pts[0], pts[pts.len() - 1]);
        for p in &pts {
            bounds.add_point(*p);
        }
        let mut heads = Vec::new();
        if e.lhs.arrow {
            heads.push(arrow(
                p0,
                e.lhs.side.out().scale(-1.0),
                &pal.line_color,
                ac.icon_size,
                &mut bounds,
            ));
        }
        if e.rhs.arrow {
            heads.push(arrow(
                p1,
                e.rhs.side.out().scale(-1.0),
                &pal.line_color,
                ac.icon_size,
                &mut bounds,
            ));
        }
        let mut children = vec![Element::classed_shape(
            Geometry::Path(geom::polyline(&pts)),
            // `archEdgeWidth: 3`
            Paint::stroke(&pal.line_color, 3.0),
            "edge",
        )];
        children.extend(heads);
        edge_els.push(Element::Group {
            id: Some(format!(
                "L-{}-{}-{ei}",
                d.nodes[e.lhs.node].id, d.nodes[e.rhs.node].id
            )),
            class: Some("architecture-edge".into()),
            children,
        });
        if let Some(label) = e.label.as_deref().filter(|l| !l.trim().is_empty()) {
            label_els.push(Element::group(
                "edgeLabel",
                // Upstream's edge labels have no background box.
                edge_label(
                    label,
                    &pts,
                    e.lhs.side,
                    e.rhs.side,
                    font,
                    &pal.text_color,
                    &mut bounds,
                ),
            ));
        }
    }

    let elements = vec![
        Element::group("architecture-groups", group_els),
        Element::group("architecture-edges", edge_els),
        Element::group("architecture-services", node_els),
        Element::group("edgeLabels", label_els),
    ];
    Scene {
        kind: DiagramKind::Architecture,
        // setupGraphViewbox: the drawing's box plus `architecture.padding`.
        view_box: bounds.rect().pad(ac.padding),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_config_reads_the_architecture_config() {
        let base = MermaidConfig::default();
        let fcose_options = |c: &MermaidConfig| arch_config(c).fcose;
        assert_eq!(arch_config(&base), ArchConfig::default());
        assert!(!fcose_options(&base).randomize);
        assert_eq!(fcose_options(&base).node_separation, 75.0);
        assert_eq!(
            (fcose_options(&base).seed, fcose_options(&base).num_iter),
            (1, 2500)
        );
        let with = |site: &str, directive: &str| {
            let mut c = MermaidConfig::default();
            if !site.is_empty() {
                c = c.with_json(&crate::json::parse(site).unwrap()).unwrap();
            }
            if !directive.is_empty() {
                c = c
                    .with_directive(&crate::json::parse(directive).unwrap())
                    .unwrap();
            }
            arch_config(&c)
        };
        let on = with(r#"{"architecture": {"randomize": true}}"#, "").fcose;
        assert!(on.randomize);
        // The diagram's directive wins over the site config.
        assert!(
            !with(
                r#"{"architecture": {"randomize": true}}"#,
                r#"{"architecture": {"randomize": false}}"#
            )
            .fcose
            .randomize
        );
        // fcose tests `options.randomize` for JavaScript truthiness.
        for (v, want) in [
            ("1", true),
            ("0", false),
            ("\"\"", false),
            ("\"no\"", true),
            ("null", false),
        ] {
            let j = format!(r#"{{"architecture": {{"randomize": {v}}}}}"#);
            assert_eq!(with("", &j).fcose.randomize, want, "{v}");
        }
        let sep = with("", r#"{"architecture": {"nodeSeparation": 40}}"#).fcose;
        assert_eq!((sep.randomize, sep.node_separation), (false, 40.0));
        let all = with(
            r#"{"architecture": {"iconSize": 60, "padding": 10, "seed": 7}}"#,
            r#"{"architecture": {"fontSize": 12, "idealEdgeLengthMultiplier": 2,
                "edgeElasticity": 0.2, "numIter": 300, "seed": -1}}"#,
        );
        assert_eq!(
            (
                all.icon_size,
                all.padding,
                all.font_size,
                all.ideal_mult,
                all.elasticity
            ),
            (60.0, 10.0, 12.0, 2.0, 0.2)
        );
        // `seed >>> 0`; the directive's seed wins over the site config's.
        assert_eq!((all.fcose.seed, all.fcose.num_iter), (u32::MAX, 300));
    }

    const SRC: &str = "architecture-beta
    group api(cloud)[API]

    service db(database)[Database] in api
    service disk1(disk)[Storage] in api
    service disk2(disk)[Storage] in api
    service server(server)[Server] in api
    junction j in api

    db:L -- R:server
    disk1:T -- B:server
    disk2:T --> B:db
    server:T <-[calls]-> T:db
    out:L -- R:server{group}
    service out(internet)[Out]
";

    #[test]
    fn parses_groups_services_edges() {
        let d = parse(SRC, None).unwrap();
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.groups[0].icon.as_deref(), Some("cloud"));
        assert_eq!(d.groups[0].title.as_deref(), Some("API"));
        assert_eq!(d.nodes.len(), 6);
        assert_eq!(d.nodes[0].parent, Some(0));
        // Junctions are applied after every service (mermaid's statement order).
        assert!(matches!(d.nodes[5].kind, NodeKind::Junction));
        assert_eq!(d.nodes[4].id, "out");
        let e = &d.edges[0];
        assert_eq!((e.lhs.side, e.rhs.side), (Side::L, Side::R));
        assert!(!e.lhs.arrow && !e.rhs.arrow);
        assert!(d.edges[2].rhs.arrow);
        let e = &d.edges[3];
        assert!(!e.lhs.group && e.lhs.arrow && e.rhs.arrow);
        assert_eq!(e.label.as_deref(), Some("calls"));
        assert!(d.edges[4].rhs.group && !d.edges[4].lhs.group);
    }

    #[test]
    fn parses_icons_and_nested_groups_and_rejects_quoted_icons() {
        let e = parse("architecture-beta\n    service s(\"TXT\")[S]", None).unwrap_err();
        assert_eq!(
            e.to_string(),
            "Parsing failed: Lexer error on line 2, column 14: unexpected character: ->(<-"
        );
        let d = parse(
            "architecture-beta\ngroup outer[Outer]\ngroup inner(server)[\"Inner [x]\"] in outer\nservice s(disk)[S] in inner\nservice t(logos:aws-s3) in outer",
            None,
        )
        .unwrap();
        assert_eq!(d.groups[1].parent, Some(0));
        assert_eq!(d.groups[1].title.as_deref(), Some("Inner [x]"));
        assert!(
            matches!(&d.nodes[1].kind, NodeKind::Service { icon: Some(i), title: None, .. } if i == "logos:aws-s3")
        );
    }

    #[test]
    fn side_hints_drive_placement() {
        let d = parse(
            "architecture-beta\nservice a(server)[A]\nservice b(server)[B]\nservice c(server)[C]\na:R -- L:b\nb:B -- T:c",
            None,
        )
        .unwrap();
        let (_, lay) = layout(&d, 16.0, &ArchConfig::default());
        let (a, b, c) = (lay.icon_box[0], lay.icon_box[1], lay.icon_box[2]);
        assert!(b.x > a.right());
        assert!((a.y - b.y).abs() < 1e-6);
        assert!(c.y > b.bottom());
        assert!((c.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn groups_contain_their_members() {
        let d = parse(SRC, None).unwrap();
        let (_, lay) = layout(&d, 16.0, &ArchConfig::default());
        let g = lay.group_box[0];
        // (fcose lets the ungrouped `out` overlap the group, as in mermaid.)
        for (n, nb) in d.nodes.iter().zip(&lay.node_box) {
            if n.parent.is_none() {
                continue;
            }
            assert!(
                g.x <= nb.x && nb.right() <= g.right() && g.y <= nb.y && nb.bottom() <= g.bottom()
            );
        }
    }

    const NESTED: &str = "architecture-beta
    group cloud(cloud)[Cloud]
    group private(server)[Private] in cloud
    group public(internet)[Public] in cloud
    service web(internet)[Web] in public
    service app(server)[App] in private
    service db(database)[DB] in private
    service user(internet)[User]
    user:R --> L:web
    web{group}:B --> T:app{group}
    app:R -- L:db
";

    fn inside(outer: Rect, inner: Rect, pad: f64) -> bool {
        outer.x + pad <= inner.x + 1e-6
            && inner.right() <= outer.right() - pad + 1e-6
            && outer.y + pad <= inner.y + 1e-6
            && inner.bottom() <= outer.bottom() - pad + 1e-6
    }

    #[test]
    fn nested_groups_contain_children_with_padding() {
        let d = parse(NESTED, None).unwrap();
        let (_, lay) = layout(&d, 16.0, &ArchConfig::default());
        let (cloud, private, public) = (lay.group_box[0], lay.group_box[1], lay.group_box[2]);
        assert!(inside(cloud, private, 40.0));
        assert!(inside(cloud, public, 40.0));
        assert!(!private.intersects(&public));
        // Icons keep the padding; labels hang into the bottom one.
        assert!(inside(public, lay.icon_box[0], 40.0));
        assert!(inside(private, lay.icon_box[1], 40.0));
        assert!(inside(private, lay.icon_box[2], 40.0));
        assert!(inside(private, lay.node_box[2], 0.0));
        // The ungrouped service stays outside every group.
        assert!(!cloud.intersects(&lay.node_box[3]));
    }

    #[test]
    fn straight_edges_across_group_borders_are_aligned() {
        let d = parse(NESTED, None).unwrap();
        let (_, lay) = layout(&d, 16.0, &ArchConfig::default());
        let c = |n: usize| lay.icon_box[n].center();
        // user:R --> L:web is horizontal, web:B --> T:app vertical (fcose aligns both).
        assert!((c(3).y - c(0).y).abs() < 1e-6);
        assert!((c(0).x - c(1).x).abs() < 1e-6);
        // Moving the Public group kept it inside Cloud.
        assert!(inside(lay.group_box[0], lay.group_box[2], 40.0));
    }

    #[test]
    fn statements_apply_groups_first_like_mermaid() {
        // Service before its group and an edge before its target: both fine upstream.
        let d = parse(
            "architecture-beta\nservice a(server)[A] in g\na:R -- L:b\nservice b(disk)[B]\ngroup g[G]",
            None,
        )
        .unwrap();
        assert_eq!(d.nodes[0].parent, Some(0));
        assert_eq!(d.edges.len(), 1);
    }

    #[test]
    fn align_hints() {
        let d = parse(
            "architecture-beta\ngroup g[G]\nservice a(server)[A] in g\nservice b(server)[B] in g\nservice c(server)[C]\nalign row a b c",
            None,
        )
        .unwrap();
        assert_eq!(
            d.aligns,
            vec![Align {
                row: true,
                members: vec![0, 1, 2]
            }]
        );
        let (_, lay) = layout(&d, 16.0, &ArchConfig::default());
        let (a, b, c) = (lay.icon_box[0], lay.icon_box[1], lay.icon_box[2]);
        assert!(a.right() < b.x && b.right() < c.x);
        assert!((a.y - b.y).abs() < 1e-6 && (b.y - c.y).abs() < 1e-6);
        assert!(!lay.group_box[0].intersects(&c));
    }

    #[test]
    fn group_errors_match_mermaid() {
        let msg = |src: &str| match parse(src, None) {
            Err(Error::Parse { message, .. }) => message,
            other => panic!("expected error, got {other:?}"),
        };
        assert!(
            msg("architecture-beta\ngroup g[G]\nservice a in g\nservice b in g\na{group}:R -- L:b")
                .contains("modified to traverse the group boundary")
        );
        assert!(
            msg("architecture-beta\nservice a\nservice b in a").contains("parent is not a group")
        );
        assert!(msg("architecture-beta\ngroup g in g").contains("cannot be placed within itself"));
        assert!(msg("architecture-beta\ngroup g in h\ngroup h").contains("parent does not exist"));
        assert!(msg("architecture-beta\nservice a\nalign row a").contains("at least two members"));
        assert!(
            msg("architecture-beta\ngroup g\nservice a\nalign column a g")
                .contains("not a service or junction")
        );
    }

    /// mermaid 12.3.0's viewBox, absolute offset included: cytoscape centres the
    /// layout on its labels measured in Inter (the canvas's Helvetica Neue), and
    /// the viewBox takes "Processor"'s box at scale 1 (17.015625 tall, rounded out
    /// to layout units), not the 17 the scaled drawing shows.
    #[test]
    fn view_box_matches_mermaids_absolutely() {
        let src = "architecture-beta\n    service src1(server)[Source 1]\n    service src2(server)[Source 2]\n    service src3(server)[Source 3]\n    service proc(server)[Processor]\n    src1:B --> T:proc\n    src2:B --> T:proc\n    src3:B --> T:proc\n    align row src1 src2 src3\n";
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        let v = r.scene.view_box;
        let want = [-166.108_894, -103.600_822, 414.717_804, 369.936_035];
        for (got, want) in [v.x, v.y, v.w, v.h].into_iter().zip(want) {
            assert!((got - want).abs() < 2e-3, "{v:?}");
        }
    }

    /// createText wraps a group's title at the group's width and a service's at
    /// 1.5 icon widths (mermaid 12.3.0: "Backend Services" in two lines).
    #[test]
    fn titles_wrap_like_create_text() {
        let src = "---\nconfig:\n  architecture:\n    padding: 20\n---\narchitecture-beta\n    group api(cloud)[API]\n    group back(server)[Backend Services] in api\n\n    service db(database)[Database] in back\n    service disk1(disk)[Storage] in back\n    service server(server)[Server] in api\n    service gw(internet)[Gateway]\n    junction j1\n\n    db:L -- R:server\n    disk1:T -- B:db\n    server:T -- B:j1\n    j1:R --> L:gw\n    gw:B -- T:disk1\n";
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        fn walk(els: &[Element], out: &mut Vec<Vec<String>>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => walk(children, out),
                    Element::Text { text, .. } => out.push(text.lines.clone()),
                    _ => {}
                }
            }
        }
        let mut lines = Vec::new();
        walk(&r.scene.elements, &mut lines);
        assert!(
            lines.contains(&vec!["Backend".to_string(), "Services".to_string()]),
            "{lines:?}"
        );
    }

    /// drawEdges turns a label on a vertical edge -90° about the midpoint and
    /// wraps it at its width (mermaid 12.3.0: "calls" at translate(-187.184,
    /// -72.661) rotate(-90); "replicates data" in two lines), and tilts an
    /// elbow's 45°.
    #[test]
    fn edge_labels_turn_and_wrap_like_draw_edges() {
        let src = "architecture-beta\n    group g2(cloud)[Group 2]\n    service s0(server)[DB]\n    service s1(internet)[Service number 1]\n    service s2(cloud)[Svc 2]\n    service s3(server)[DB]\n    service s4(disk)[Service number 4]\n    service s5(cloud)[DB]\n    service s6(server)[x6] in g2\n    s0:T -[calls]- B:s3\n    s6{group}:T -[calls]- B:s4\n    s5:B -[replicates data]- L:s2\n    s6:T --> B:s0\n    s5:R -[calls]- B:s1\n    s0:R --> L:s4\n    s5:T --> B:s4\n";
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        fn walk(els: &[Element], deg: f64, out: &mut Vec<(f64, Vec<String>)>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => walk(children, deg, out),
                    Element::Rotated { deg, children, .. } => walk(children, *deg, out),
                    Element::Text { text, .. } => out.push((deg, text.lines.clone())),
                    _ => {}
                }
            }
        }
        let mut texts = Vec::new();
        walk(&r.scene.elements, 0.0, &mut texts);
        let calls = |d: f64| {
            texts
                .iter()
                .filter(|t| t.0 == d && t.1 == ["calls"])
                .count()
        };
        assert_eq!(calls(-90.0), 2, "{texts:?}");
        assert_eq!(calls(-45.0) + calls(45.0), 1, "{texts:?}");
        assert!(
            texts.iter().any(|t| t.1 == ["replicates", "data"]),
            "{texts:?}"
        );
    }

    fn err_line(src: &str) -> usize {
        match parse(src, None) {
            Err(Error::Parse { line, .. }) => line,
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn reports_errors_with_lines() {
        assert_eq!(
            err_line("architecture-beta\nservice a(server)[A] in nowhere"),
            2
        );
        assert_eq!(err_line("architecture-beta\nservice a\nservice a"), 3);
        assert_eq!(
            err_line("architecture-beta\nservice a\nservice b\na:X -- L:b"),
            4
        );
        assert_eq!(err_line("architecture-beta\nservice a\n\na:R -- L:zzz"), 4);
        assert_eq!(
            err_line("architecture-beta\nservice a\nservice b\na:R == L:b"),
            4
        );
        assert_eq!(err_line("architecture-beta\nfoo bar"), 2);
        assert_eq!(err_line("service a"), 1);
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(SRC, Some("T".into()), &cfg, &pal).unwrap();
        assert_eq!(s.kind, DiagramKind::Architecture);
        assert!(s.view_box.w > 80.0);
    }

    /// Our service positions relative to the first one against mermaid
    /// 12.3.0's (Chromium, `Helvetica` resolving to Arial's metrics).
    fn assert_positions(src: &str, want: &[(&str, f64, f64)]) {
        let d = parse(src, None).unwrap();
        let (p, _) = layout(&d, 16.0, &ArchConfig::default());
        let at = |id: &str| p.node_pos[d.nodes.iter().position(|n| n.id == id).unwrap()];
        let (o, (_, wx, wy)) = (at(want[0].0), want[0]);
        for &(id, x, y) in want {
            let q = at(id);
            assert!(
                ((q.x - o.x) - (x - wx)).abs() < 1e-6 && ((q.y - o.y) - (y - wy)).abs() < 1e-6,
                "{id}: {q:?} vs ({x}, {y})"
            );
        }
    }

    #[test]
    fn unconstrained_diagrams_run_fcose_like_mermaid() {
        // No edges at all: fcose still runs (mermaid always passes an
        // alignment constraint object, so nothing is tiled).
        assert_positions(
            "architecture-beta
    group g0(cloud)[Group 0]
    service s0(internet)[Svc 0] in g0
    service s1(internet)[DB] in g0
    service s2(cloud)[x2] in g0
    service s3(database)[Service number 3]",
            &[
                ("s0", 215.3832061971034, 247.1332061971034),
                ("s1", 69.6597292826084, 101.4097292826084),
                ("s2", -76.06965954576471, -44.31965954576471),
                ("s3", -283.3832061971034, -251.6332061971034),
            ],
        );
        // Only same-side edges (no spatial constraints); their endpoints and
        // the wide titles move the centre fcose starts from.
        assert_positions(
            "architecture-beta
    service s0(server)[x0]
    service s1(server)[DB]
    service s2(cloud)[Service number 2]
    service s3(cloud)[DB]
    service s4(disk)[x4]
    service s5(cloud)[Service number 5]
    service s6(cloud)[DB]
    service s7(database)[Service number 7]
    junction j0
    s7:L --> L:s5
    s5:R -- R:j0
    s6:L -- L:s1
    s5:R --> R:s1
    s7:T --> T:s4
    s7:R -- R:s6",
            &[
                ("s0", 319.31681913528405, 295.99704392807337),
                ("s1", 83.03645205703171, 63.55882156462545),
                ("s2", 8.899067305369137, -17.219013064145784),
                ("s3", -76.1052684035935, -86.58799880124188),
                ("s4", -82.38634223695406, -180.5985121527668),
                ("s5", -162.19937875824058, -35.96895990063015),
                ("s6", 21.275730629731136, -165.66812751389176),
                ("s7", -218.01050802472713, -234.99110642402042),
            ],
        );
    }

    /// With `fontSize: 24` the services' titles, not their icons, set the
    /// width of the group around them: mermaid 12.3.0's positions and group
    /// rects (Chromium, the canvas's `Helvetica Neue` resolving to Inter).
    #[test]
    fn font_size_widens_groups_by_their_titles_like_cytoscape() {
        let d = parse(
            "architecture-beta
    group api(cloud)[API]
    group back(server)[Backend Services] in api

    service db(database)[Database] in back
    service disk1(disk)[Storage] in back
    service server(server)[Server] in api
    service gw(internet)[Gateway]
    junction j1

    db:L -- R:server
    disk1:T -- B:db
    server:T -- B:j1
    j1:R --> L:gw
    gw:B -- T:disk1",
            None,
        )
        .unwrap();
        let c = ArchConfig {
            font_size: 24.0,
            ..ArchConfig::default()
        };
        let (p, _) = layout(&d, 16.0, &c);
        let at = |id: &str| p.node_pos[d.nodes.iter().position(|n| n.id == id).unwrap()];
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let (db, server) = (at("db"), at("server"));
        assert!(close(db.x, 89.95813219223501) && close(db.y, 24.550458108513606));
        assert!(close(server.x, -113.45813219223498));
        // The drawn rect is cytoscape's box shifted by half an icon.
        let back = p.group_bbox[1];
        assert!(close(back.x + 40.0, 32.45813219223501), "{back:?}");
        assert!(close(back.w, 195.0) && close(back.h, 390.89304507517085));
        let api = p.group_bbox[0];
        assert!(close(api.x + 40.0, -155.95813219223498) && close(api.w, 424.91626438446997));
    }

    #[test]
    fn edge_labels_widen_the_centre_box_like_cytoscape() {
        assert_positions(
            "architecture-beta
    group g0(cloud)[Group 0]
    group g1(cloud)[Group 1]
    group g2(cloud)[Group 2]
    service s0(server)[DB]
    service s1(internet)[Service number 1]
    service s2(cloud)[Svc 2]
    service s3(server)[DB]
    service s4(disk)[Service number 4]
    service s5(cloud)[DB]
    service s6(server)[x6] in g2
    s0:T -[calls]- B:s3
    s6{group}:T -[calls]- B:s4
    s5:B -[replicates data]- L:s2
    s6:T --> B:s0
    s5:R -[calls]- B:s1
    s0:R --> L:s4
    s5:T --> B:s4",
            &[
                ("s0", -227.18422118380988, -11.856549900874967),
                ("s1", -117.29888344527353, 0.14185992296433625),
                ("s2", -117.29888344527353, 0.14185992296433625),
                ("s3", -227.18422118380988, -213.4654128644585),
                ("s4", 12.475676629678059, -11.856549900874967),
                ("s5", 12.475676629678059, 208.96541286445853),
                ("s6", -227.18422118380988, 208.96541286445853),
            ],
        );
    }

    #[test]
    fn group_edge_leaves_the_nodes_innermost_group() {
        let d = parse(
            "architecture-beta\ngroup g(cloud)[G]\ngroup h(cloud)[H] in g\nservice a(server)[A] in h\nservice c(disk)[C]\na{group}:B -- T:c",
            None,
        )
        .unwrap();
        let (p, lay) = layout(&d, 16.0, &ArchConfig::default());
        let e = &d.edges[0];
        let h = d.groups.iter().position(|g| g.id == "h").unwrap();
        let pts = architecture_fcose::edge_points(&d, &p, e, &ArchConfig::default());
        // The `{group}` end starts past H's bottom border, the plain end on
        // the service's top edge.
        assert!(pts[0].y > lay.group_box[h].bottom());
        assert!((pts[2].y - lay.icon_box[e.rhs.node].y).abs() < 1e-9);
    }
}
