//! `architecture-beta` diagrams.
//!
//! Supported syntax (mermaid 11): `group id(icon)[Title] in parent`,
//! `service id(icon)[Title] in group`, text icons `service id("txt")[Title]`,
//! `junction id in group`, edges `a:R -- L:b`, arrows `a:R --> L:b`,
//! `a:R <--> L:b`, labelled edges `a:R -[Label]- L:b`, group modifiers
//! `a{group}:B --> T:b{group}`, and `title`.
//!
//! Layout: mermaid uses a force layout constrained by the port sides; we instead
//! place every container's children (services, junctions, nested groups) on an
//! integer grid derived from the side hints (`a:R -- L:b` puts `b` one cell right
//! of `a`), size grid rows/columns to their contents, and route edges
//! orthogonally between the named sides. Icons are drawn as simple generic glyphs.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;
use std::collections::HashMap;

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
    fn horizontal(self) -> bool {
        matches!(self, Side::L | Side::R)
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

fn parse_decl(s: &str, line: usize, allow_icon: bool) -> Result<Decl> {
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
    let (mut icon, mut icon_text, mut title, mut parent) = (None, None, None, None);
    if allow_icon {
        if let Some(r) = rest.strip_prefix('(') {
            let end = r
                .find(')')
                .ok_or_else(|| Error::parse(line, "Expecting ')' to close the icon"))?;
            let inner = r[..end].trim();
            if let Some(t) = inner.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                icon_text = Some(t.to_string());
            } else if inner.is_empty() {
                return Err(Error::parse(line, "Empty icon name"));
            } else {
                icon = Some(inner.to_string());
            }
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
            "group" => decls.push((DeclKind::Group, parse_decl(rest, line, true)?, line)),
            "service" => decls.push((DeclKind::Service, parse_decl(rest, line, true)?, line)),
            "junction" => decls.push((DeclKind::Junction, parse_decl(rest, line, false)?, line)),
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

// Sizes follow mermaid's architecture defaults (`iconSize: 80`, `padding: 40`,
// ideal same-group edge length `1.5 * iconSize`), measured against fcose output.
const ICON: f64 = 80.0;
/// Horizontal gap between two columns holding only services/junctions.
const GAP_X: f64 = 120.0;
/// Vertical gap between two rows of services (the service label hangs into it).
const GAP_Y: f64 = 96.0;
/// Gaps next to a nested group (fcose's cross-group edges are shorter).
const GROUP_GAP_X: f64 = 100.0;
const GROUP_GAP_Y: f64 = 75.0;
/// `architecture.padding`: space between a group's border and its contents.
const GROUP_PAD: f64 = 40.0;
/// Group icon size: `padding * 0.75`.
const GROUP_ICON: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    Node(usize),
    Group(usize),
}

/// Positioned diagram: full item boxes plus the icon box of each node.
struct Layout {
    node_box: Vec<Rect>,
    icon_box: Vec<Rect>,
    group_box: Vec<Rect>,
    /// Grid cell each node was placed in (bounds for alignment nudges).
    cell_box: Vec<Rect>,
    /// Grid cell each group was placed in.
    group_cell: Vec<Rect>,
}

/// `(lhs item, rhs item, dx, dy)`: rhs sits at lhs + (dx, dy) on the grid.
type Constraint = (Item, Item, i32, i32);

struct Grid {
    cells: Vec<(Item, i32, i32)>,
    col_w: Vec<f64>,
    row_h: Vec<f64>,
    /// Gap after each column / row (last entry unused).
    col_gap: Vec<f64>,
    row_gap: Vec<f64>,
    w: f64,
    h: f64,
}

fn group_chain(d: &Architecture, mut g: Option<usize>) -> Vec<Option<usize>> {
    // Outermost first, starting with the root (None).
    let mut chain = vec![];
    let mut guard = 0;
    while let Some(i) = g {
        chain.push(Some(i));
        g = d.groups[i].parent;
        guard += 1;
        if guard > d.groups.len() {
            break;
        }
    }
    chain.push(None);
    chain.reverse();
    chain
}

/// Deepest common container of two nodes and each node's child item in it.
fn common_items(d: &Architecture, a: usize, b: usize) -> (Option<usize>, Item, Item) {
    let ca = group_chain(d, d.nodes[a].parent);
    let cb = group_chain(d, d.nodes[b].parent);
    let mut k = 0;
    while k + 1 < ca.len() && k + 1 < cb.len() && ca[k + 1] == cb[k + 1] {
        k += 1;
    }
    let item = |chain: &[Option<usize>], n: usize| match chain.get(k + 1) {
        Some(Some(g)) => Item::Group(*g),
        _ => Item::Node(n),
    };
    (ca[k], item(&ca, a), item(&cb, b))
}

/// Grid offset of `rhs` relative to `lhs` implied by the edge's sides.
fn offset(e: &ArchEdge) -> (i32, i32) {
    let dx = match (e.lhs.side, e.rhs.side) {
        (Side::R, _) => 1,
        (Side::L, _) => -1,
        (_, Side::L) => 1,
        (_, Side::R) => -1,
        _ => 0,
    };
    let dy = match (e.lhs.side, e.rhs.side) {
        (Side::B, _) => 1,
        (Side::T, _) => -1,
        (_, Side::T) => 1,
        (_, Side::B) => -1,
        _ => 0,
    };
    (dx, dy)
}

fn service_label(d: &Architecture, n: usize, font: f64) -> Option<TextBlock> {
    match &d.nodes[n].kind {
        NodeKind::Service { title: Some(t), .. } => {
            Some(TextBlock::label(t, font)).filter(|b| !b.is_empty())
        }
        _ => None,
    }
}

/// Width the group's header (icon + title) needs inside its border.
fn group_title_width(g: &Group, font: f64) -> f64 {
    let t = g
        .title
        .as_ref()
        .map(|t| TextBlock::label(t, font))
        .filter(|b| !b.is_empty());
    let icon_w = if g.icon.is_some() {
        GROUP_ICON + 4.0
    } else {
        4.0
    };
    match t {
        Some(b) => icon_w + b.width + 8.0,
        None if g.icon.is_some() => icon_w,
        None => 0.0,
    }
}

fn layout(d: &Architecture, font: f64) -> Layout {
    let ng = d.groups.len();
    // Children of each container, in declaration order (nodes, then groups).
    let mut children: HashMap<Option<usize>, Vec<Item>> = HashMap::new();
    for (i, n) in d.nodes.iter().enumerate() {
        children.entry(n.parent).or_default().push(Item::Node(i));
    }
    for (i, g) in d.groups.iter().enumerate() {
        children.entry(g.parent).or_default().push(Item::Group(i));
    }
    let mut constraints: HashMap<Option<usize>, Vec<Constraint>> = HashMap::new();
    for e in &d.edges {
        let (c, ia, ib) = common_items(d, e.lhs.node, e.rhs.node);
        if ia != ib {
            let (dx, dy) = offset(e);
            constraints.entry(c).or_default().push((ia, ib, dx, dy));
        }
    }
    // `align` hints rank after edges: consecutive members side by side.
    for a in &d.aligns {
        for w in a.members.windows(2) {
            let (c, ia, ib) = common_items(d, w[0], w[1]);
            if ia != ib {
                let step = if a.row { (1, 0) } else { (0, 1) };
                constraints
                    .entry(c)
                    .or_default()
                    .push((ia, ib, step.0, step.1));
            }
        }
    }

    // Size items bottom-up: groups are declared after their parents, so
    // iterating in reverse visits children before parents.
    let mut sizes: HashMap<Item, (f64, f64)> = HashMap::new();
    for (i, n) in d.nodes.iter().enumerate() {
        let s = match n.kind {
            // Junctions are invisible icon-sized boxes in mermaid.
            NodeKind::Junction => (ICON, ICON),
            NodeKind::Service { .. } => match service_label(d, i, font) {
                Some(b) => (ICON.max(b.width + 4.0), ICON + 6.0 + b.height),
                None => (ICON, ICON),
            },
        };
        sizes.insert(Item::Node(i), s);
    }
    let mut grids: HashMap<Option<usize>, Grid> = HashMap::new();
    for gi in (0..ng).rev() {
        let grid = grid_for(
            children.get(&Some(gi)).map(Vec::as_slice).unwrap_or(&[]),
            constraints.get(&Some(gi)).map(Vec::as_slice).unwrap_or(&[]),
            &sizes,
        );
        // The header sits inside the top padding, like mermaid's group label.
        let w = (grid.w + 2.0 * GROUP_PAD)
            .max(group_title_width(&d.groups[gi], font))
            .max(ICON);
        let h = grid.h + 2.0 * GROUP_PAD;
        sizes.insert(Item::Group(gi), (w, h));
        grids.insert(Some(gi), grid);
    }
    let root = grid_for(
        children.get(&None).map(Vec::as_slice).unwrap_or(&[]),
        constraints.get(&None).map(Vec::as_slice).unwrap_or(&[]),
        &sizes,
    );
    grids.insert(None, root);

    let mut out = Layout {
        node_box: vec![Rect::default(); d.nodes.len()],
        icon_box: vec![Rect::default(); d.nodes.len()],
        group_box: vec![Rect::default(); ng],
        cell_box: vec![Rect::default(); d.nodes.len()],
        group_cell: vec![Rect::default(); ng],
    };
    place(
        None,
        Rect::new(0.0, 0.0, 0.0, 0.0),
        &grids,
        &sizes,
        &mut out,
    );
    align_cross_group_edges(d, &mut out);
    out
}

fn grid_for(items: &[Item], cons: &[Constraint], sizes: &HashMap<Item, (f64, f64)>) -> Grid {
    let mut pos: HashMap<Item, (i32, i32)> = HashMap::new();
    let mut taken: HashMap<(i32, i32), Item> = HashMap::new();
    let free_from = |taken: &HashMap<(i32, i32), Item>, mut p: (i32, i32), step: (i32, i32)| {
        let step = if step == (0, 0) { (1, 0) } else { step };
        while taken.contains_key(&p) {
            p = (p.0 + step.0, p.1 + step.1);
        }
        p
    };
    for &start in items {
        if pos.contains_key(&start) {
            continue;
        }
        // New component: to the right of everything placed so far.
        let x = taken.keys().map(|p| p.0 + 1).max().unwrap_or(0);
        let p = free_from(&taken, (x, 0), (0, 1));
        pos.insert(start, p);
        taken.insert(p, start);
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(cur) = queue.pop_front() {
            let here = pos[&cur];
            for &(a, b, dx, dy) in cons {
                let (other, step) = if a == cur {
                    (b, (dx, dy))
                } else if b == cur {
                    (a, (-dx, -dy))
                } else {
                    continue;
                };
                if pos.contains_key(&other) || !items.contains(&other) {
                    continue;
                }
                let p = free_from(&taken, (here.0 + step.0, here.1 + step.1), step);
                pos.insert(other, p);
                taken.insert(p, other);
                queue.push_back(other);
            }
        }
    }
    let min_x = pos.values().map(|p| p.0).min().unwrap_or(0);
    let min_y = pos.values().map(|p| p.1).min().unwrap_or(0);
    let max_x = pos.values().map(|p| p.0).max().unwrap_or(-1);
    let max_y = pos.values().map(|p| p.1).max().unwrap_or(-1);
    let ncols = (max_x - min_x + 1).max(0) as usize;
    let nrows = (max_y - min_y + 1).max(0) as usize;
    let mut col_w = vec![0.0f64; ncols];
    let mut row_h = vec![0.0f64; nrows];
    let mut col_group = vec![false; ncols];
    let mut row_group = vec![false; nrows];
    let mut cells = Vec::new();
    for &it in items {
        let Some(&(x, y)) = pos.get(&it) else {
            continue;
        };
        let (cx, cy) = ((x - min_x) as usize, (y - min_y) as usize);
        let (w, h) = sizes.get(&it).copied().unwrap_or((0.0, 0.0));
        col_w[cx] = col_w[cx].max(w);
        row_h[cy] = row_h[cy].max(h);
        if matches!(it, Item::Group(_)) {
            col_group[cx] = true;
            row_group[cy] = true;
        }
        cells.push((it, cx as i32, cy as i32));
    }
    let gaps = |grp: &[bool], plain: f64, group: f64| -> Vec<f64> {
        (0..grp.len())
            .map(|i| {
                if grp[i] || grp.get(i + 1).copied().unwrap_or(false) {
                    group
                } else {
                    plain
                }
            })
            .collect()
    };
    let col_gap = gaps(&col_group, GAP_X, GROUP_GAP_X);
    let row_gap = gaps(&row_group, GAP_Y, GROUP_GAP_Y);
    let span = |v: &[f64], g: &[f64]| {
        v.iter().sum::<f64>() + g.iter().take(v.len().saturating_sub(1)).sum::<f64>()
    };
    Grid {
        w: span(&col_w, &col_gap),
        h: span(&row_h, &row_gap),
        cells,
        col_w,
        row_h,
        col_gap,
        row_gap,
    }
}

/// Assign absolute rectangles. `frame` is the container's box (unused for root).
fn place(
    container: Option<usize>,
    frame: Rect,
    grids: &HashMap<Option<usize>, Grid>,
    sizes: &HashMap<Item, (f64, f64)>,
    out: &mut Layout,
) {
    let Some(grid) = grids.get(&container) else {
        return;
    };
    let origin = match container {
        None => Point::new(0.0, 0.0),
        Some(_) => Point::new(frame.x + (frame.w - grid.w) / 2.0, frame.y + GROUP_PAD),
    };
    let starts = |sizes: &[f64], gaps: &[f64], o: f64| -> Vec<f64> {
        let mut at = o;
        sizes
            .iter()
            .zip(gaps)
            .map(|(s, g)| {
                let here = at;
                at += s + g;
                here
            })
            .collect()
    };
    let col_x = starts(&grid.col_w, &grid.col_gap, origin.x);
    let row_y = starts(&grid.row_h, &grid.row_gap, origin.y);
    for &(it, cx, cy) in &grid.cells {
        let (w, h) = sizes.get(&it).copied().unwrap_or((0.0, 0.0));
        let (cw, ch) = (grid.col_w[cx as usize], grid.row_h[cy as usize]);
        let (x0, y0) = (col_x[cx as usize], row_y[cy as usize]);
        match it {
            Item::Node(n) => {
                // Icons of a row share their top edge (labels hang below).
                let top = y0 + (ch - sizes_row_max(grid, sizes, cy)) / 2.0;
                let r = Rect::new(x0 + (cw - w) / 2.0, top, w, h);
                out.node_box[n] = r;
                out.icon_box[n] = Rect::new(r.x + (w - ICON) / 2.0, r.y, ICON, ICON);
                out.cell_box[n] = Rect::new(x0, y0, cw, ch);
            }
            Item::Group(g) => {
                let r = Rect::new(x0 + (cw - w) / 2.0, y0 + (ch - h) / 2.0, w, h);
                out.group_box[g] = r;
                out.group_cell[g] = Rect::new(x0, y0, cw, ch);
                place(Some(g), r, grids, sizes, out);
            }
        }
    }
}

/// Tallest service/junction in a grid row (icons are aligned to its top).
fn sizes_row_max(grid: &Grid, sizes: &HashMap<Item, (f64, f64)>, row: i32) -> f64 {
    grid.cells
        .iter()
        .filter(|c| c.2 == row && matches!(c.0, Item::Node(_)))
        .filter_map(|c| sizes.get(&c.0).map(|s| s.1))
        .fold(0.0f64, f64::max)
}

/// fcose aligns the two ends of a straight edge even when it crosses a group
/// border. Our grid only aligns the items of one container, so slide one of
/// the two items holding the edge's ends (a node or a whole nested group)
/// across its grid cell to line the ends up, when it fits in the cell and no
/// other straight edge along the same axis pins that item.
fn align_cross_group_edges(d: &Architecture, lay: &mut Layout) {
    // (a, b, horizontal): pairs of nodes that should share a y (or an x).
    let mut pairs: Vec<(usize, usize, bool)> = d
        .edges
        .iter()
        .filter(|e| e.lhs.side.horizontal() == e.rhs.side.horizontal())
        .map(|e| (e.lhs.node, e.rhs.node, e.lhs.side.horizontal()))
        .collect();
    for a in &d.aligns {
        pairs.extend(a.members.windows(2).map(|w| (w[0], w[1], a.row)));
    }
    for (pi, &(na, nb, horizontal)) in pairs.iter().enumerate() {
        let (_, ia, ib) = common_items(d, na, nb);
        if ia == ib || (matches!(ia, Item::Node(_)) && matches!(ib, Item::Node(_))) {
            continue;
        }
        let (want, have) = (lay.icon_box[nb].center(), lay.icon_box[na].center());
        let delta = if horizontal {
            Point::new(0.0, want.y - have.y)
        } else {
            Point::new(want.x - have.x, 0.0)
        };
        if delta.len() < 0.5 {
            continue;
        }
        // Prefer moving a plain node, then the smaller group.
        let area = |it: Item| match it {
            Item::Node(_) => 0.0,
            Item::Group(g) => lay.group_box[g].w * lay.group_box[g].h,
        };
        let mut order = [(ia, delta), (ib, delta.scale(-1.0))];
        if area(ib) < area(ia) {
            order.swap(0, 1);
        }
        for (mover, dv) in order {
            let pinned = pairs.iter().enumerate().any(|(pj, &(fa, fb, fh))| {
                let (_, fa, fb) = common_items(d, fa, fb);
                pj != pi && fh == horizontal && fa != fb && (fa == mover || fb == mover)
            });
            let (r, cell) = match mover {
                Item::Node(n) => (lay.node_box[n], lay.cell_box[n]),
                Item::Group(g) => (lay.group_box[g], lay.group_cell[g]),
            };
            let moved = Rect::new(r.x + dv.x, r.y + dv.y, r.w, r.h);
            let fits = moved.x >= cell.x - 0.5
                && moved.right() <= cell.right() + 0.5
                && moved.y >= cell.y - 0.5
                && moved.bottom() <= cell.bottom() + 0.5;
            if !pinned && fits {
                shift_item(d, lay, mover, dv);
                break;
            }
        }
    }
}

/// Translate a node, or a group with everything inside it.
fn shift_item(d: &Architecture, lay: &mut Layout, it: Item, dv: Point) {
    let mv = |r: &mut Rect| {
        r.x += dv.x;
        r.y += dv.y;
    };
    let inside = |chain: Vec<Option<usize>>, g: usize| chain.contains(&Some(g));
    match it {
        Item::Node(n) => {
            mv(&mut lay.node_box[n]);
            mv(&mut lay.icon_box[n]);
        }
        Item::Group(g) => {
            for gi in 0..d.groups.len() {
                if inside(group_chain(d, Some(gi)), g) {
                    mv(&mut lay.group_box[gi]);
                    mv(&mut lay.group_cell[gi]);
                }
            }
            for n in 0..d.nodes.len() {
                if inside(group_chain(d, d.nodes[n].parent), g) {
                    mv(&mut lay.node_box[n]);
                    mv(&mut lay.icon_box[n]);
                    mv(&mut lay.cell_box[n]);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- drawing

/// `archGroupBorderColor`: the theme's `primaryBorderColor`, i.e. mermaid's
/// `mkBorder(primaryColor)` (saturation -40, lightness -10 / +10 when dark).
fn group_border_color(pal: &Palette) -> Color {
    match crate::color::parse(&pal.main_bkg.0) {
        Some(c) => Color::new(
            c.adjust_hsl(0.0, -40.0, if pal.dark { 10.0 } else { -10.0 })
                .to_hex(),
        ),
        None => pal.node_border.clone(),
    }
}

fn port(r: &Rect, side: Side) -> Point {
    let c = r.center();
    match side {
        Side::L => Point::new(r.x, c.y),
        Side::R => Point::new(r.right(), c.y),
        Side::T => Point::new(c.x, r.y),
        Side::B => Point::new(c.x, r.bottom()),
    }
}

fn end_rect(d: &Architecture, lay: &Layout, e: &EdgeEnd, other: usize) -> Rect {
    if e.group {
        let (_, ia, _) = common_items(d, e.node, other);
        if let Item::Group(g) = ia {
            return lay.group_box[g];
        }
        if let Some(g) = d.nodes[e.node].parent {
            return lay.group_box[g];
        }
    }
    lay.icon_box[e.node]
}

fn route(p0: Point, s0: Side, p1: Point, s1: Side) -> Vec<Point> {
    let mut pts = vec![p0];
    match (s0.horizontal(), s1.horizontal()) {
        (true, true) => {
            if (p0.y - p1.y).abs() > 0.5 {
                let mx = (p0.x + p1.x) / 2.0;
                pts.extend([Point::new(mx, p0.y), Point::new(mx, p1.y)]);
            }
        }
        (false, false) => {
            if (p0.x - p1.x).abs() > 0.5 {
                let my = (p0.y + p1.y) / 2.0;
                pts.extend([Point::new(p0.x, my), Point::new(p1.x, my)]);
            }
        }
        (true, false) => pts.push(Point::new(p1.x, p0.y)),
        (false, true) => pts.push(Point::new(p0.x, p1.y)),
    }
    pts.push(p1);
    pts
}

/// Point halfway along a polyline.
fn midpoint(pts: &[Point]) -> Point {
    let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut left = total / 2.0;
    for w in pts.windows(2) {
        let l = w[0].dist(w[1]);
        if l >= left && l > 0.0 {
            return w[0].lerp(w[1], left / l);
        }
        left -= l;
    }
    pts[0]
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

fn draw(d: &Architecture, _cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let font = pal.font_size;
    let lay = layout(d, font);
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
        let ra = end_rect(d, &lay, &e.lhs, e.rhs.node);
        let rb = end_rect(d, &lay, &e.rhs, e.lhs.node);
        let junction = |n: usize| matches!(d.nodes[n].kind, NodeKind::Junction);
        let end_point = |end: &EdgeEnd, r: &Rect| {
            if end.group {
                // Leave the group border level with the node (mermaid shifts
                // the node's port by `padding + 4` towards the border).
                let c = lay.icon_box[end.node].center();
                let p = port(r, end.side);
                if end.side.horizontal() {
                    Point::new(p.x, c.y.clamp(r.y, r.bottom()))
                } else {
                    Point::new(c.x.clamp(r.x, r.right()), p.y)
                }
            } else if junction(end.node) {
                r.center()
            } else {
                port(r, end.side)
            }
        };
        let p0 = end_point(&e.lhs, &ra);
        let p1 = end_point(&e.rhs, &rb);
        let pts = route(p0, e.lhs.side, p1, e.rhs.side);
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
                let at = midpoint(&pts);
                let r = Rect::centered(at, b.width + 6.0, b.height);
                bounds.add_rect(r);
                label_els.push(Element::group(
                    "edgeLabel",
                    vec![
                        Element::rect(r, 0.0, Paint::fill(&pal.background)),
                        Element::text(Text::centered(
                            at,
                            &b.lines,
                            font,
                            b.line_height,
                            &pal.text_color,
                        )),
                    ],
                ));
            }
        }
    }

    let mut title_els = Vec::new();
    if let Some(t) = d.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], font * 1.125, false);
        let center = Point::new(
            content.x + content.w / 2.0,
            content.y - 16.0 - tb.height / 2.0,
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
            "architectureTitleText",
        ));
    }
    let mut elements = vec![
        Element::group("architecture-groups", group_els),
        Element::group("architecture-edges", edge_els),
        Element::group("architecture-services", node_els),
        Element::group("edgeLabels", label_els),
    ];
    elements.extend(title_els);
    Scene {
        kind: DiagramKind::Architecture,
        view_box: bounds.rect().pad(20.0),
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
    fn parses_text_icons_and_nested_groups() {
        let d = parse(
            "architecture-beta\ngroup outer[Outer]\ngroup inner(server)[\"Inner [x]\"] in outer\nservice s(\"TXT\")[S] in inner\nservice t(logos:aws-s3) in outer",
            None,
        )
        .unwrap();
        assert_eq!(d.groups[1].parent, Some(0));
        assert_eq!(d.groups[1].title.as_deref(), Some("Inner [x]"));
        assert!(
            matches!(&d.nodes[0].kind, NodeKind::Service { icon_text: Some(t), .. } if t == "TXT")
        );
        assert!(
            matches!(&d.nodes[1].kind, NodeKind::Service { icon: Some(i), title: None, .. } if i == "logos:aws-s3")
        );
    }

    #[test]
    fn side_hints_drive_grid_placement() {
        let d = parse(
            "architecture-beta\nservice a(server)[A]\nservice b(server)[B]\nservice c(server)[C]\na:R -- L:b\nb:B -- T:c",
            None,
        )
        .unwrap();
        let lay = layout(&d, 16.0);
        let (a, b, c) = (lay.icon_box[0], lay.icon_box[1], lay.icon_box[2]);
        assert!(b.x > a.right());
        assert!((a.y - b.y).abs() < 1e-6);
        assert!(c.y > b.bottom());
        assert!((c.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn groups_contain_their_members() {
        let d = parse(SRC, None).unwrap();
        let lay = layout(&d, 16.0);
        let g = lay.group_box[0];
        for (n, nb) in d.nodes.iter().zip(&lay.node_box) {
            if n.parent.is_none() {
                assert!(!g.intersects(nb));
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
        let lay = layout(&d, 16.0);
        let (cloud, private, public) = (lay.group_box[0], lay.group_box[1], lay.group_box[2]);
        assert!(inside(cloud, private, GROUP_PAD));
        assert!(inside(cloud, public, GROUP_PAD));
        assert!(!private.intersects(&public));
        assert!(inside(public, lay.node_box[0], GROUP_PAD));
        assert!(inside(private, lay.node_box[1], GROUP_PAD));
        assert!(inside(private, lay.node_box[2], GROUP_PAD));
        // The ungrouped service stays outside every group.
        assert!(!cloud.intersects(&lay.node_box[3]));
    }

    #[test]
    fn straight_edges_across_group_borders_are_aligned() {
        let d = parse(NESTED, None).unwrap();
        let lay = layout(&d, 16.0);
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
        let lay = layout(&d, 16.0);
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
}
