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

use super::architecture_fcose::{self, Placement};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
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

// mermaid's architecture defaults: `iconSize: 80`, `padding: 40`.
const ICON: f64 = 80.0;
/// `architecture.padding`: space between a group's border and its contents.
const GROUP_PAD: f64 = 40.0;
/// Group icon size: `padding * 0.75`.
const GROUP_ICON: f64 = 30.0;

/// Positioned diagram: full item boxes plus the icon box of each node.
struct Layout {
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
fn arrow(end: Point, dir: Point, color: &Color) -> Element {
    let size = ICON / 6.0;
    let perp = Point::new(-dir.y, dir.x);
    let tip = end.plus(dir.scale(2.0));
    let base = tip.minus(dir.scale(size));
    Element::classed_shape(
        Geometry::Path(geom::polygon(&[
            tip,
            base.plus(perp.scale(size / 2.0)),
            base.minus(perp.scale(size / 2.0)),
        ])),
        Paint::fill(color),
        "arrow",
    )
}

/// White glyph for the built-in icon names, a generic cube otherwise.
fn icon_glyph(icon: Option<&str>, r: Rect) -> Vec<Element> {
    let white = Color::new("#ffffff");
    let s = r.w / 80.0;
    let p = |x: f64, y: f64| Point::new(r.x + x * s, r.y + y * s);
    let stroke = Paint::stroke(&white, 3.0 * s);
    let fill = Paint::fill(&white);
    let name = icon.map(|i| i.rsplit(':').next().unwrap_or(i).to_ascii_lowercase());
    let pi = std::f64::consts::PI;
    match name.as_deref() {
        None | Some("blank") => vec![],
        Some("database") => {
            let (rx, ry) = (20.0 * s, 7.0 * s);
            let mut body = vec![
                PathCmd::MoveTo(p(20.0, 24.0)),
                PathCmd::LineTo(p(20.0, 56.0)),
            ];
            body.extend(geom::elliptic_arc_cubics(p(40.0, 56.0), rx, ry, pi, 0.0));
            body.push(PathCmd::LineTo(p(60.0, 24.0)));
            let mut mid = vec![PathCmd::MoveTo(p(20.0, 40.0))];
            mid.extend(geom::elliptic_arc_cubics(p(40.0, 40.0), rx, ry, pi, 0.0));
            vec![
                Element::path(geom::ellipse(p(40.0, 24.0), rx, ry), stroke.clone()),
                Element::path(body, stroke.clone()),
                Element::path(mid, stroke),
            ]
        }
        Some("server") => {
            let mut v = Vec::new();
            for y in [16.0, 34.0, 52.0] {
                v.push(Element::rect(
                    Rect::new(r.x + 18.0 * s, r.y + y * s, 44.0 * s, 12.0 * s),
                    2.0 * s,
                    stroke.clone(),
                ));
                v.push(Element::shape(
                    Geometry::Circle {
                        c: p(54.0, y + 6.0),
                        r: 2.2 * s,
                    },
                    fill.clone(),
                ));
            }
            v
        }
        Some("disk") => vec![
            Element::shape(
                Geometry::Circle {
                    c: p(40.0, 40.0),
                    r: 22.0 * s,
                },
                stroke.clone(),
            ),
            Element::shape(
                Geometry::Circle {
                    c: p(40.0, 40.0),
                    r: 5.0 * s,
                },
                fill,
            ),
            Element::line(p(46.0, 46.0), p(56.0, 56.0), stroke),
        ],
        Some("internet") => {
            let half = |x: f64| -> f64 { (22.0f64 * 22.0 - x * x).max(0.0).sqrt() };
            let w11 = half(11.0);
            vec![
                Element::shape(
                    Geometry::Circle {
                        c: p(40.0, 40.0),
                        r: 22.0 * s,
                    },
                    stroke.clone(),
                ),
                Element::path(
                    geom::ellipse(p(40.0, 40.0), 9.0 * s, 22.0 * s),
                    stroke.clone(),
                ),
                Element::line(p(18.0, 40.0), p(62.0, 40.0), stroke.clone()),
                Element::line(p(40.0 - w11, 29.0), p(40.0 + w11, 29.0), stroke.clone()),
                Element::line(p(40.0 - w11, 51.0), p(40.0 + w11, 51.0), stroke),
            ]
        }
        Some("cloud") => vec![
            Element::shape(
                Geometry::Circle {
                    c: p(27.0, 46.0),
                    r: 10.0 * s,
                },
                fill.clone(),
            ),
            Element::shape(
                Geometry::Circle {
                    c: p(41.0, 37.0),
                    r: 14.0 * s,
                },
                fill.clone(),
            ),
            Element::shape(
                Geometry::Circle {
                    c: p(55.0, 46.0),
                    r: 10.0 * s,
                },
                fill.clone(),
            ),
            Element::rect(
                Rect::new(r.x + 27.0 * s, r.y + 40.0 * s, 28.0 * s, 16.0 * s),
                0.0,
                fill,
            ),
        ],
        Some(_) => {
            // Generic cube.
            let top = [p(40.0, 18.0), p(60.0, 29.0), p(40.0, 40.0), p(20.0, 29.0)];
            vec![
                Element::path(
                    geom::polygon(&[
                        p(40.0, 18.0),
                        p(60.0, 29.0),
                        p(60.0, 51.0),
                        p(40.0, 62.0),
                        p(20.0, 51.0),
                        p(20.0, 29.0),
                    ]),
                    stroke.clone(),
                ),
                Element::path(geom::polyline(&top[1..]), stroke.clone()),
                Element::line(p(40.0, 40.0), p(40.0, 62.0), stroke),
            ]
        }
    }
}

/// Boxes from mermaid's fcose placement: a service's icon has its top left
/// corner at the node's cytoscape position, and the groups' boxes are drawn
/// shifted by half an icon the same way (svgDraw.ts).
fn layout_from_placement(d: &Architecture, p: &Placement, font: f64) -> Layout {
    let half = ICON / 2.0;
    let icon_box: Vec<Rect> = p
        .node_pos
        .iter()
        .map(|q| Rect::new(q.x, q.y, ICON, ICON))
        .collect();
    let node_box: Vec<Rect> = icon_box
        .iter()
        .enumerate()
        .map(|(i, &ib)| match service_label(d, i, font) {
            Some(b) => {
                let w = ICON.max(b.width + 4.0);
                Rect::new(ib.center().x - w / 2.0, ib.y, w, ICON + 6.0 + b.height)
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
/// drawn from it.
fn layout(d: &Architecture, font: f64) -> (Placement, Layout) {
    // cytoscape measures a label (a service's raw title or an edge's label,
    // one line) on a canvas in its default font, `Helvetica Neue, Helvetica,
    // sans-serif` (Arial's metrics where Helvetica Neue is missing), rounded
    // up; service titles at `fontSize`, edge labels at cytoscape's 16px.
    let measure = |t: &str, edge: bool| {
        crate::text::line_width(t, if edge { 16.0 } else { font }, false).ceil()
    };
    let placement = architecture_fcose::place(d, &measure);
    let lay = layout_from_placement(d, &placement, font);
    (placement, lay)
}

fn draw(d: &Architecture, _cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let font = pal.font_size;
    let (placement, lay) = layout(d, font);
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
        let mut x = r.x + 4.0;
        let mut label_cy = r.y + 12.0;
        if let Some(icon) = &g.icon {
            let ir = Rect::new(r.x + 1.0, r.y + 1.0, GROUP_ICON, GROUP_ICON);
            children.push(Element::rect(ir, 0.0, Paint::fill(&icon_bg)));
            children.extend(icon_glyph(Some(icon), ir));
            x = ir.right() + 3.0;
            label_cy = r.y + 17.0;
        }
        if let Some(t) = g.title.as_ref().map(|t| TextBlock::label(t, font)) {
            if !t.is_empty() {
                let extra = t.line_height * (t.lines.len() as f64 - 1.0) / 2.0;
                let center = Point::new(x, label_cy + extra);
                children.push(Element::classed_text(
                    Text::centered(center, &t.lines, font, t.line_height, &pal.text_color)
                        .anchored(Anchor::Start, x),
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
        let nb = lay.node_box[i];
        bounds.add_rect(nb);
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
                    Geometry::Rect { rect: ib, rx: 6.0 },
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
                if let Some(b) = title.as_ref().map(|t| TextBlock::label(t, font)) {
                    if !b.is_empty() {
                        let center = Point::new(ib.center().x, ib.bottom() + 6.0 + b.height / 2.0);
                        bounds.add_rect(Rect::centered(center, b.width, b.height));
                        v.push(Element::classed_text(
                            Text::centered(center, &b.lines, font, b.line_height, &pal.text_color),
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
        let pts = architecture_fcose::edge_points(d, &placement, e);
        let (p0, p1) = (pts[0], pts[pts.len() - 1]);
        for p in &pts {
            bounds.add_point(*p);
        }
        let mut heads = Vec::new();
        if e.lhs.arrow {
            heads.push(arrow(p0, e.lhs.side.out().scale(-1.0), &pal.line_color));
        }
        if e.rhs.arrow {
            heads.push(arrow(p1, e.rhs.side.out().scale(-1.0), &pal.line_color));
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
        if let Some(b) = e.label.as_ref().map(|l| TextBlock::label(l, font)) {
            if !b.is_empty() {
                // `createText` at the midpoint with `dy: 1em`: the label hangs just
                // below the edge rather than covering it.
                let mid = pts[1];
                let at = Point::new(mid.x, mid.y + b.height / 2.0);
                let r = Rect::centered(at, b.width + 6.0, b.height);
                bounds.add_rect(r);
                label_els.push(Element::group(
                    "edgeLabel",
                    // Upstream's edge labels have no background box.
                    vec![Element::text(Text::centered(
                        at,
                        &b.lines,
                        font,
                        b.line_height,
                        &pal.text_color,
                    ))],
                ));
            }
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
        view_box: bounds.rect().pad(GROUP_PAD),
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
        let (_, lay) = layout(&d, 16.0);
        let (a, b, c) = (lay.icon_box[0], lay.icon_box[1], lay.icon_box[2]);
        assert!(b.x > a.right());
        assert!((a.y - b.y).abs() < 1e-6);
        assert!(c.y > b.bottom());
        assert!((c.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn groups_contain_their_members() {
        let d = parse(SRC, None).unwrap();
        let (_, lay) = layout(&d, 16.0);
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
        let (_, lay) = layout(&d, 16.0);
        let (cloud, private, public) = (lay.group_box[0], lay.group_box[1], lay.group_box[2]);
        assert!(inside(cloud, private, GROUP_PAD));
        assert!(inside(cloud, public, GROUP_PAD));
        assert!(!private.intersects(&public));
        // Icons keep the padding; labels hang into the bottom one.
        assert!(inside(public, lay.icon_box[0], GROUP_PAD));
        assert!(inside(private, lay.icon_box[1], GROUP_PAD));
        assert!(inside(private, lay.icon_box[2], GROUP_PAD));
        assert!(inside(private, lay.node_box[2], 0.0));
        // The ungrouped service stays outside every group.
        assert!(!cloud.intersects(&lay.node_box[3]));
    }

    #[test]
    fn straight_edges_across_group_borders_are_aligned() {
        let d = parse(NESTED, None).unwrap();
        let (_, lay) = layout(&d, 16.0);
        let c = |n: usize| lay.icon_box[n].center();
        // user:R --> L:web is horizontal, web:B --> T:app vertical (fcose aligns both).
        assert!((c(3).y - c(0).y).abs() < 1e-6);
        assert!((c(0).x - c(1).x).abs() < 1e-6);
        // Moving the Public group kept it inside Cloud.
        assert!(inside(lay.group_box[0], lay.group_box[2], GROUP_PAD));
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
        let (_, lay) = layout(&d, 16.0);
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
        let (p, _) = layout(&d, 16.0);
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
                ("s0", 217.6332061971034, 247.1332061971034),
                ("s1", 71.9097292826084, 101.4097292826084),
                ("s2", -73.81965954576471, -44.31965954576471),
                ("s3", -281.1332061971034, -251.6332061971034),
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
                ("s0", 304.25679168541643, 339.44540146717634),
                ("s1", 63.49903969744793, 49.6859856540257),
                ("s2", -32.715413215805015, -29.00875126043576),
                ("s3", -134.57498621106015, -92.21032128678121),
                ("s4", -287.75679168541643, 4.379338781689853),
                ("s5", -18.63878199853069, -154.89711742028635),
                ("s6", -148.23105132567682, 18.259596564597047),
                ("s7", -226.55892931675157, -187.62004156755515),
                ("j0", -193.8681672764819, -303.44540146717634),
            ],
        );
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
                ("s0", -227.68404835199726, -11.856657718654716),
                ("s1", -117.79909556715964, 0.14197922877448832),
                ("s2", -117.79909556715964, 0.14197922877448832),
                ("s3", -227.68404835199726, -213.46552273481612),
                ("s4", 11.97493101455705, -11.856657718654716),
                ("s5", 11.97493101455705, 208.96552273481612),
                ("s6", -227.68404835199726, 208.96552273481612),
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
        let (p, lay) = layout(&d, 16.0);
        let e = &d.edges[0];
        let h = d.groups.iter().position(|g| g.id == "h").unwrap();
        let pts = architecture_fcose::edge_points(&d, &p, e);
        // The `{group}` end starts past H's bottom border, the plain end on
        // the service's top edge.
        assert!(pts[0].y > lay.group_box[h].bottom());
        assert!((pts[2].y - lay.icon_box[e.rhs.node].y).abs() < 1e-9);
    }
}
