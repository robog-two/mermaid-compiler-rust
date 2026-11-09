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

/// Parse an architecture diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<Architecture> {
    let mut d = Architecture {
        title,
        ..Architecture::default()
    };
    let mut header = false;
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
            "group" => {
                let decl = parse_decl(rest, line, true)?;
                check_new_id(&d, &decl.id, line)?;
                let parent = resolve_group(&d, decl.parent.as_deref(), line)?;
                d.groups.push(Group {
                    id: decl.id,
                    icon: decl.icon.or(decl.icon_text),
                    title: decl.title,
                    parent,
                });
            }
            "service" => {
                let decl = parse_decl(rest, line, true)?;
                check_new_id(&d, &decl.id, line)?;
                let parent = resolve_group(&d, decl.parent.as_deref(), line)?;
                d.nodes.push(ArchNode {
                    id: decl.id,
                    kind: NodeKind::Service {
                        icon: decl.icon,
                        icon_text: decl.icon_text,
                        title: decl.title,
                    },
                    parent,
                });
            }
            "junction" => {
                let decl = parse_decl(rest, line, false)?;
                check_new_id(&d, &decl.id, line)?;
                let parent = resolve_group(&d, decl.parent.as_deref(), line)?;
                d.nodes.push(ArchNode {
                    id: decl.id,
                    kind: NodeKind::Junction,
                    parent,
                });
            }
            _ => {
                let e = parse_edge(&d, s, line)?;
                d.edges.push(e);
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'architecture-beta'"));
    }
    Ok(d)
}

fn check_new_id(d: &Architecture, id: &str, line: usize) -> Result<()> {
    if d.groups.iter().any(|g| g.id == id) || d.nodes.iter().any(|n| n.id == id) {
        return Err(Error::parse(
            line,
            format!("The id '{id}' is already in use"),
        ));
    }
    Ok(())
}

fn resolve_group(d: &Architecture, name: Option<&str>, line: usize) -> Result<Option<usize>> {
    match name {
        None => Ok(None),
        Some(n) => d
            .groups
            .iter()
            .position(|g| g.id == n)
            .map(Some)
            .ok_or_else(|| Error::parse(line, format!("Unknown group '{n}'"))),
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
    let find = |id: &str| {
        d.nodes.iter().position(|n| n.id == id).ok_or_else(|| {
            Error::parse(
                line,
                format!("Unknown service or junction '{id}' (declare it before use)"),
            )
        })
    };
    Ok(ArchEdge {
        lhs: EdgeEnd {
            node: find(lhs_id)?,
            side: lhs_side,
            group: lhs_group,
            arrow: lhs_arrow,
        },
        rhs: EdgeEnd {
            node: find(rhs_id)?,
            side: rhs_side,
            group: rhs_group,
            arrow: rhs_arrow,
        },
        label,
    })
}

// ---------------------------------------------------------------- layout

const ICON: f64 = 80.0;
const GAP: f64 = 70.0;
const GROUP_PAD: f64 = 28.0;
const JUNCTION: f64 = 4.0;

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
}

/// `(lhs item, rhs item, dx, dy)`: rhs sits at lhs + (dx, dy) on the grid.
type Constraint = (Item, Item, i32, i32);

struct Grid {
    cells: Vec<(Item, i32, i32)>,
    col_w: Vec<f64>,
    row_h: Vec<f64>,
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

fn group_title_size(g: &Group, font: f64) -> (f64, f64) {
    let t = g
        .title
        .as_ref()
        .map(|t| TextBlock::label(t, font))
        .filter(|b| !b.is_empty());
    let icon_w = if g.icon.is_some() { 26.0 } else { 0.0 };
    match t {
        Some(b) => (b.width + icon_w + 12.0, b.height.max(24.0) + 6.0),
        None if g.icon.is_some() => (icon_w + 12.0, 30.0),
        None => (0.0, 0.0),
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

    // Size items bottom-up: groups are declared after their parents, so
    // iterating in reverse visits children before parents.
    let mut sizes: HashMap<Item, (f64, f64)> = HashMap::new();
    for (i, n) in d.nodes.iter().enumerate() {
        let s = match n.kind {
            NodeKind::Junction => (JUNCTION, JUNCTION),
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
        let (tw, th) = group_title_size(&d.groups[gi], font);
        let w = (grid.w + 2.0 * GROUP_PAD)
            .max(tw + 2.0 * GROUP_PAD)
            .max(ICON);
        let h = grid.h + 2.0 * GROUP_PAD + th;
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
    };
    place(
        d,
        None,
        Rect::new(0.0, 0.0, 0.0, 0.0),
        &grids,
        &sizes,
        &mut out,
        font,
    );
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
    let mut cells = Vec::new();
    for &it in items {
        let Some(&(x, y)) = pos.get(&it) else {
            continue;
        };
        let (cx, cy) = (x - min_x, y - min_y);
        let (w, h) = sizes.get(&it).copied().unwrap_or((0.0, 0.0));
        col_w[cx as usize] = col_w[cx as usize].max(w);
        row_h[cy as usize] = row_h[cy as usize].max(h);
        cells.push((it, cx, cy));
    }
    let span = |v: &[f64]| v.iter().sum::<f64>() + GAP * (v.len().saturating_sub(1)) as f64;
    Grid {
        w: span(&col_w),
        h: span(&row_h),
        cells,
        col_w,
        row_h,
    }
}

/// Assign absolute rectangles. `frame` is the container's box (unused for root).
#[allow(clippy::too_many_arguments)]
fn place(
    d: &Architecture,
    container: Option<usize>,
    frame: Rect,
    grids: &HashMap<Option<usize>, Grid>,
    sizes: &HashMap<Item, (f64, f64)>,
    out: &mut Layout,
    font: f64,
) {
    let Some(grid) = grids.get(&container) else {
        return;
    };
    let origin = match container {
        None => Point::new(0.0, 0.0),
        Some(g) => {
            let (_, th) = group_title_size(&d.groups[g], font);
            Point::new(frame.x + (frame.w - grid.w) / 2.0, frame.y + th + GROUP_PAD)
        }
    };
    let col_x: Vec<f64> = grid
        .col_w
        .iter()
        .scan(origin.x, |x, w| {
            let at = *x;
            *x += w + GAP;
            Some(at)
        })
        .collect();
    let row_y: Vec<f64> = grid
        .row_h
        .iter()
        .scan(origin.y, |y, h| {
            let at = *y;
            *y += h + GAP;
            Some(at)
        })
        .collect();
    for &(it, cx, cy) in &grid.cells {
        let (w, h) = sizes.get(&it).copied().unwrap_or((0.0, 0.0));
        let (cw, ch) = (grid.col_w[cx as usize], grid.row_h[cy as usize]);
        let (x0, y0) = (col_x[cx as usize], row_y[cy as usize]);
        match it {
            Item::Node(n) => {
                // Align icons along the row's top band so equal-height icons line up.
                let icon_h = if matches!(d.nodes[n].kind, NodeKind::Junction) {
                    JUNCTION
                } else {
                    ICON
                };
                let band = grid
                    .cells
                    .iter()
                    .filter(|c| c.2 == cy)
                    .filter_map(|c| match c.0 {
                        Item::Node(m) if !matches!(d.nodes[m].kind, NodeKind::Junction) => {
                            Some(ICON)
                        }
                        _ => None,
                    })
                    .fold(0.0f64, f64::max);
                let top = if band > 0.0 && h < ch {
                    // Centre the icon on the band of service icons in this row.
                    let band_top = y0 + (ch - sizes_row_max(grid, sizes, cy)) / 2.0;
                    band_top + (band - icon_h) / 2.0
                } else {
                    y0 + (ch - h) / 2.0
                };
                let r = Rect::new(x0 + (cw - w) / 2.0, top, w, h);
                out.node_box[n] = r;
                out.icon_box[n] =
                    Rect::new(r.x + (w - icon_h.min(w)) / 2.0, r.y, icon_h.min(w), icon_h);
                if matches!(d.nodes[n].kind, NodeKind::Junction) {
                    out.icon_box[n] = Rect::centered(
                        Point::new(x0 + cw / 2.0, top + JUNCTION / 2.0),
                        JUNCTION,
                        JUNCTION,
                    );
                }
            }
            Item::Group(g) => {
                let r = Rect::new(x0 + (cw - w) / 2.0, y0 + (ch - h) / 2.0, w, h);
                out.group_box[g] = r;
                place(d, Some(g), r, grids, sizes, out, font);
            }
        }
    }
}

/// Tallest service node in a grid row (icons are aligned to its top).
fn sizes_row_max(grid: &Grid, sizes: &HashMap<Item, (f64, f64)>, row: i32) -> f64 {
    grid.cells
        .iter()
        .filter(|c| c.2 == row && matches!(c.0, Item::Node(_)))
        .filter_map(|c| sizes.get(&c.0).map(|s| s.1))
        .filter(|h| *h > JUNCTION)
        .fold(0.0f64, f64::max)
}

// ---------------------------------------------------------------- drawing

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

fn arrow(tip: Point, dir: Point, color: &Color) -> Element {
    let perp = Point::new(-dir.y, dir.x);
    let base = tip.minus(dir.scale(12.0));
    Element::classed_shape(
        Geometry::Path(geom::polygon(&[
            tip,
            base.plus(perp.scale(6.0)),
            base.minus(perp.scale(6.0)),
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

    for (gi, g) in d.groups.iter().enumerate() {
        let r = lay.group_box[gi];
        bounds.add_rect(r);
        let mut children = vec![Element::classed_shape(
            Geometry::Rect { rect: r, rx: 0.0 },
            Paint::stroke(&pal.cluster_border, 2.0).dashed(vec![8.0, 4.0]),
            "node-bkg",
        )];
        let mut x = r.x + 8.0;
        let ty = r.y + 6.0;
        if let Some(icon) = &g.icon {
            let ir = Rect::new(x, ty, 22.0, 22.0);
            children.push(Element::rect(ir, 3.0, Paint::fill(&icon_bg)));
            children.extend(icon_glyph(Some(icon), ir));
            x += 28.0;
        }
        if let Some(t) = g.title.as_ref().map(|t| TextBlock::label(t, font)) {
            if !t.is_empty() {
                let center = Point::new(x, ty + 11.0);
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
            NodeKind::Junction => vec![Element::shape(
                Geometry::Circle {
                    c: ib.center(),
                    r: 2.0,
                },
                Paint::fill(&pal.line_color),
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
        // Bottom ports of services sit below the label so the line does not cross it.
        let end_point = |end: &EdgeEnd, r: &Rect| {
            if end.group {
                port(r, end.side)
            } else if junction(end.node) {
                r.center()
            } else if end.side == Side::B {
                Point::new(r.center().x, lay.node_box[end.node].bottom() + 2.0)
            } else {
                port(r, end.side)
            }
        };
        let p0 = end_point(&e.lhs, &ra);
        let p1 = end_point(&e.rhs, &rb);
        let mut pts = route(p0, e.lhs.side, p1, e.rhs.side);
        for p in &pts {
            bounds.add_point(*p);
        }
        let mut heads = Vec::new();
        if e.lhs.arrow {
            heads.push(arrow(p0, e.lhs.side.out().scale(-1.0), &pal.line_color));
            pts[0] = p0.plus(e.lhs.side.out().scale(8.0));
        }
        if e.rhs.arrow {
            let last = pts.len() - 1;
            heads.push(arrow(p1, e.rhs.side.out().scale(-1.0), &pal.line_color));
            pts[last] = p1.plus(e.rhs.side.out().scale(8.0));
        }
        let mut children = vec![Element::classed_shape(
            Geometry::Path(geom::polyline(&pts)),
            Paint::stroke(&pal.line_color, 2.0),
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
    server{group}:R <-[calls]-> L:db{group}
";

    #[test]
    fn parses_groups_services_edges() {
        let d = parse(SRC, None).unwrap();
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.groups[0].icon.as_deref(), Some("cloud"));
        assert_eq!(d.groups[0].title.as_deref(), Some("API"));
        assert_eq!(d.nodes.len(), 5);
        assert_eq!(d.nodes[0].parent, Some(0));
        assert!(matches!(d.nodes[4].kind, NodeKind::Junction));
        let e = &d.edges[0];
        assert_eq!((e.lhs.side, e.rhs.side), (Side::L, Side::R));
        assert!(!e.lhs.arrow && !e.rhs.arrow);
        assert!(d.edges[2].rhs.arrow);
        let e = &d.edges[3];
        assert!(e.lhs.group && e.rhs.group && e.lhs.arrow && e.rhs.arrow);
        assert_eq!(e.label.as_deref(), Some("calls"));
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
        for nb in &lay.node_box {
            assert!(
                g.x <= nb.x && nb.right() <= g.right() && g.y <= nb.y && nb.bottom() <= g.bottom()
            );
        }
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
