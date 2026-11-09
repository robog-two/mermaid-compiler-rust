//! `block` (`block-beta`) diagrams.
//!
//! Supported syntax: `columns N` / `columns auto`; blocks `id`, `id["label"]`
//! with flowchart shapes (`[ ]`, `( )`, `([ ])`, `[[ ]]`, `[( )]`, `(( ))`,
//! `((( )))`, `> ]`, `{ }`, `{{ }}`, `[/ /]`, `[\ \]`, `[/ \]`, `[\ /]`);
//! column spans `id:3`; `space` / `space:N`; nested `block[:id[:span]] ... end`
//! with their own `columns`; block arrows `id<["label"]>(right|left|up|down|x|y)`;
//! edges `a --> b`, `a --- b`, `a -.-> b`, `a ==> b`, `a <--> b`, `a --x b`,
//! `a --o b`, `a -- "label" --> b`, `a -->|label| b` (chains allowed);
//! `style id css`, `classDef name css`, `class a,b name`, `id:::name`.
//!
//! Layout is CSS grid via Taffy: every container is a grid of equal-width
//! column tracks (the widest block decides, like mermaid), spans are
//! `grid-column: c / span n`, nested blocks are nested grid nodes. Edges are
//! straight lines between block centres clipped to the block outlines.

use crate::config::MermaidConfig;
use crate::diagram::graph::StyleOverrides;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::boxes::{self, BoxNode, Placed, taffy};
use crate::scene::{Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;

/// Block shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockShape {
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
    /// Block arrow pointing in the given directions.
    Arrow(ArrowDir),
}

/// Block-arrow directions (`x`/`y` = double-headed horizontal/vertical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDir {
    Right,
    Left,
    Up,
    Down,
    X,
    Y,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Node {
        label: String,
        shape: BlockShape,
    },
    Space,
    Composite {
        columns: Option<usize>,
        children: Vec<Block>,
    },
}

/// A block in the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: String,
    pub kind: BlockKind,
    pub span: usize,
}

/// Edge line style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Solid,
    Dotted,
    Thick,
}

/// Edge end decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    None,
    Arrow,
    Cross,
    Circle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub line: Line,
    pub start: Head,
    pub end: Head,
}

/// Parsed block diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDiagram {
    pub title: Option<String>,
    pub columns: Option<usize>,
    pub children: Vec<Block>,
    pub edges: Vec<Edge>,
    pub class_defs: Vec<(String, StyleOverrides)>,
    /// Per-block classes and inline styles.
    pub classes: Vec<(String, String)>,
    pub styles: Vec<(String, StyleOverrides)>,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Node {
        id: String,
        shape: Option<(BlockShape, String)>,
        span: Option<usize>,
        classes: Vec<String>,
    },
    Arrow(String),
    Str(String),
    Pipe(String),
}

/// (open, close, shape); longest openers first.
const SHAPES: [(&str, &str, BlockShape); 15] = [
    ("(((", ")))", BlockShape::DoubleCircle),
    ("((", "))", BlockShape::Circle),
    ("([", "])", BlockShape::Stadium),
    ("[[", "]]", BlockShape::Subroutine),
    ("[(", ")]", BlockShape::Cylinder),
    ("[/", "/]", BlockShape::LeanRight),
    ("[/", "\\]", BlockShape::Trapezoid),
    ("[\\", "\\]", BlockShape::LeanLeft),
    ("[\\", "/]", BlockShape::InvTrapezoid),
    ("{{", "}}", BlockShape::Hexagon),
    ("<[", "]>", BlockShape::Arrow(ArrowDir::Right)),
    ("{", "}", BlockShape::Diamond),
    ("(", ")", BlockShape::Rounded),
    ("[", "]", BlockShape::Rect),
    (">", "]", BlockShape::Asymmetric),
];

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn tokenize(s: &str, line_no: usize) -> Result<Vec<Tok>> {
    let err = |m: &str| Error::parse(line_no, format!("{s}\n{m}"));
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let rest = |i: usize| -> String { b[i..].iter().collect() };
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' {
            let end = b[i + 1..]
                .iter()
                .position(|&x| x == '"')
                .ok_or_else(|| err("Unterminated string"))?;
            out.push(Tok::Str(b[i + 1..i + 1 + end].iter().collect()));
            i += end + 2;
            continue;
        }
        if c == '|' {
            let end = b[i + 1..]
                .iter()
                .position(|&x| x == '|')
                .ok_or_else(|| err("Unterminated '|label|'"))?;
            out.push(Tok::Pipe(b[i + 1..i + 1 + end].iter().collect()));
            i += end + 2;
            continue;
        }
        if matches!(c, '-' | '=' | '.' | '<') {
            let mut j = i;
            if b[j] == '<' {
                j += 1;
            }
            let body_start = j;
            while j < b.len() && matches!(b[j], '-' | '=' | '.') {
                j += 1;
            }
            if j - body_start >= 2 {
                if j < b.len() && matches!(b[j], '>' | 'x' | 'o') {
                    // `--o`/`--x` only when not followed by an id character.
                    let head_ok = b[j] == '>' || !b.get(j + 1).is_some_and(|&n| is_id_char(n));
                    if head_ok {
                        j += 1;
                    }
                }
                out.push(Tok::Arrow(b[i..j].iter().collect()));
                i = j;
                continue;
            }
            return Err(err(&format!("Unexpected '{}'", rest(i))));
        }
        if !is_id_char(c) {
            return Err(err(&format!("Unexpected character '{c}'")));
        }
        let start = i;
        while i < b.len() && is_id_char(b[i]) {
            i += 1;
        }
        let id: String = b[start..i].iter().collect();
        // Shape.
        let mut shape = None;
        let here = rest(i);
        if let Some(&(open, _, _)) = SHAPES.iter().find(|(o, _, _)| here.starts_with(o)) {
            let inner_start = i + open.chars().count();
            let cands: Vec<&(&str, &str, BlockShape)> =
                SHAPES.iter().filter(|(o, _, _)| *o == open).collect();
            let mut j = inner_start;
            let mut in_q = false;
            let mut found = None;
            while j < b.len() {
                if b[j] == '"' {
                    in_q = !in_q;
                } else if !in_q {
                    let tail = rest(j);
                    if let Some(&&(_, close, sh)) =
                        cands.iter().find(|(_, cl, _)| tail.starts_with(cl))
                    {
                        found = Some((j, close, sh));
                        break;
                    }
                }
                j += 1;
            }
            let (j, close, sh) =
                found.ok_or_else(|| err(&format!("Expecting a closing delimiter for '{open}'")))?;
            let raw: String = b[inner_start..j].iter().collect();
            let raw = raw.trim();
            let label = raw
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .unwrap_or(raw)
                .to_string();
            i = j + close.chars().count();
            let sh = if let BlockShape::Arrow(_) = sh {
                // `(dir)` suffix.
                if b.get(i) != Some(&'(') {
                    return Err(err("Expecting a block arrow direction like '(right)'"));
                }
                let end = b[i..]
                    .iter()
                    .position(|&x| x == ')')
                    .ok_or_else(|| err("Expecting ')'"))?;
                let dir: String = b[i + 1..i + end].iter().collect();
                i += end + 1;
                BlockShape::Arrow(match dir.trim() {
                    "right" => ArrowDir::Right,
                    "left" => ArrowDir::Left,
                    "up" => ArrowDir::Up,
                    "down" => ArrowDir::Down,
                    "x" => ArrowDir::X,
                    "y" => ArrowDir::Y,
                    d => return Err(err(&format!("Unknown block arrow direction '{d}'"))),
                })
            } else {
                sh
            };
            shape = Some((sh, label));
        }
        // `:::class` and `:N` span.
        let mut span = None;
        let mut classes = Vec::new();
        loop {
            let here = rest(i);
            if let Some(after) = here.strip_prefix(":::") {
                let n = after
                    .chars()
                    .take_while(|&c| is_id_char(c) || c == '-')
                    .count();
                classes.push(after.chars().take(n).collect());
                i += 3 + n;
            } else if let Some(after) = here.strip_prefix(':') {
                let n = after.chars().take_while(char::is_ascii_digit).count();
                if n == 0 {
                    return Err(err("Expecting a width after ':'"));
                }
                span = after[..n].parse::<usize>().ok();
                i += 1 + n;
            } else {
                break;
            }
        }
        out.push(Tok::Node {
            id,
            shape,
            span,
            classes,
        });
    }
    Ok(out)
}

fn edge_kind(arrow: &str) -> (Line, Head, Head) {
    let start = if arrow.starts_with('<') {
        Head::Arrow
    } else {
        Head::None
    };
    let end = match arrow.chars().last() {
        Some('>') => Head::Arrow,
        Some('x') => Head::Cross,
        Some('o') => Head::Circle,
        _ => Head::None,
    };
    let line = if arrow.contains('.') {
        Line::Dotted
    } else if arrow.contains('=') {
        Line::Thick
    } else {
        Line::Solid
    };
    (line, start, end)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Frame {
    id: String,
    span: usize,
    columns: Option<usize>,
    children: Vec<Block>,
    line: usize,
}

fn find_mut<'a>(blocks: &'a mut [Block], id: &str) -> Option<&'a mut Block> {
    for b in blocks.iter_mut() {
        if b.id == id {
            return Some(b);
        }
        if let BlockKind::Composite { children, .. } = &mut b.kind {
            if let Some(f) = find_mut(children, id) {
                return Some(f);
            }
        }
    }
    None
}

/// Parse a block diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<BlockDiagram> {
    let mut d = BlockDiagram {
        title,
        columns: None,
        children: Vec::new(),
        edges: Vec::new(),
        class_defs: Vec::new(),
        classes: Vec::new(),
        styles: Vec::new(),
    };
    let mut stack: Vec<Frame> = vec![Frame {
        id: String::new(),
        span: 1,
        columns: None,
        children: Vec::new(),
        line: 0,
    }];
    let mut header = false;
    let mut anon = 0usize;
    let mut space_n = 0usize;
    for (line_no, stmt) in super::statements(body) {
        let s = stmt.as_str();
        if !header {
            let rest = s
                .strip_prefix("block-beta")
                .or_else(|| s.strip_prefix("block"))
                .filter(|r| {
                    r.is_empty() || r.starts_with(char::is_whitespace) || r.starts_with(':')
                })
                .ok_or_else(|| Error::parse(line_no, "Expecting 'block-beta'"))?;
            if !rest.trim().trim_start_matches(':').trim().is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("Unexpected text after 'block-beta': {}", rest.trim()),
                ));
            }
            header = true;
            continue;
        }
        let (word, rest) = match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], s[i..].trim()),
            None => (s, ""),
        };
        let frame_count = stack.len();
        let top = &mut stack[frame_count - 1];
        match word {
            "columns" => {
                top.columns = if rest == "auto" {
                    None
                } else {
                    Some(
                        rest.parse::<usize>()
                            .ok()
                            .filter(|&n| n > 0)
                            .ok_or_else(|| {
                                Error::parse(
                                    line_no,
                                    format!("{s}\nExpecting a column count or 'auto'"),
                                )
                            })?,
                    )
                };
                continue;
            }
            "end" => {
                if stack.len() == 1 {
                    return Err(Error::parse(line_no, "'end' without a matching 'block'"));
                }
                let f = stack.pop().expect("the root frame is never popped");
                let parent = stack.last_mut().expect("the root frame is never popped");
                parent.children.push(Block {
                    id: f.id,
                    kind: BlockKind::Composite {
                        columns: f.columns,
                        children: f.children,
                    },
                    span: f.span,
                });
                continue;
            }
            "title" => {
                d.title = Some(rest.to_string());
                continue;
            }
            "classDef" => {
                let (name, css) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                d.class_defs.push((
                    name.to_string(),
                    StyleOverrides::parse(css.trim().trim_end_matches(';')),
                ));
                continue;
            }
            "class" => {
                let (ids, name) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting 'class <ids> <className>'"))
                })?;
                for id in ids.split(',') {
                    d.classes
                        .push((id.trim().to_string(), name.trim().to_string()));
                }
                continue;
            }
            "style" => {
                let (ids, css) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting 'style <id> <css>'"))
                })?;
                let st = StyleOverrides::parse(css.trim().trim_end_matches(';'));
                for id in ids.split(',') {
                    d.styles.push((id.trim().to_string(), st.clone()));
                }
                continue;
            }
            _ => {}
        }
        if word == "block" || word.starts_with("block:") {
            let mut parts = word.splitn(3, ':');
            parts.next();
            let id = match parts.next().filter(|p| !p.is_empty()) {
                Some(id) => id.to_string(),
                None => {
                    anon += 1;
                    format!("__block{anon}")
                }
            };
            let span = match parts.next() {
                Some(w) => w.parse::<usize>().ok().filter(|&n| n > 0).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting a width after ':'"))
                })?,
                None => 1,
            };
            stack.push(Frame {
                id,
                span,
                columns: None,
                children: Vec::new(),
                line: line_no,
            });
            if !rest.is_empty() {
                // Allow `block:x columns 2`-style trailing content? Treat as an error like mermaid.
                return Err(Error::parse(
                    line_no,
                    format!("{s}\nUnexpected text after block declaration"),
                ));
            }
            continue;
        }

        let toks = tokenize(s, line_no)?;
        // Declarations and edges.
        let mut prev: Option<String> = None;
        let mut pending: Vec<Tok> = Vec::new();
        for t in toks {
            match t {
                Tok::Node {
                    id,
                    shape,
                    span,
                    classes,
                } => {
                    let real_id;
                    if id == "space" && shape.is_none() {
                        prev = None;
                        if !pending.is_empty() {
                            return Err(Error::parse(
                                line_no,
                                format!("{s}\n'space' cannot be linked"),
                            ));
                        }
                        space_n += 1;
                        real_id = format!("__space{space_n}");
                        let top = stack.last_mut().expect("the root frame is never popped");
                        top.children.push(Block {
                            id: real_id,
                            kind: BlockKind::Space,
                            span: span.unwrap_or(1).max(1),
                        });
                        continue;
                    }
                    real_id = id.clone();
                    for c in classes {
                        d.classes.push((id.clone(), c));
                    }
                    // Declare or update.
                    let exists = stack
                        .iter_mut()
                        .any(|f| find_mut(&mut f.children, &id).is_some());
                    if exists {
                        for f in stack.iter_mut() {
                            if let Some(b) = find_mut(&mut f.children, &id) {
                                if let Some((sh, label)) = &shape {
                                    b.kind = BlockKind::Node {
                                        label: label.clone(),
                                        shape: *sh,
                                    };
                                }
                                if let Some(sp) = span {
                                    b.span = sp.max(1);
                                }
                            }
                        }
                    } else {
                        let (sh, label) = shape.unwrap_or((BlockShape::Rect, id.clone()));
                        let top = stack.last_mut().expect("the root frame is never popped");
                        top.children.push(Block {
                            id: id.clone(),
                            kind: BlockKind::Node { label, shape: sh },
                            span: span.unwrap_or(1).max(1),
                        });
                    }
                    if !pending.is_empty() {
                        let from = prev.take().ok_or_else(|| {
                            Error::parse(line_no, format!("{s}\nAn edge needs a source block"))
                        })?;
                        d.edges
                            .push(make_edge(from, real_id.clone(), &pending, line_no, s)?);
                        pending.clear();
                    }
                    prev = Some(real_id);
                }
                other => {
                    if prev.is_none() {
                        return Err(Error::parse(
                            line_no,
                            format!("{s}\nAn edge needs a source block"),
                        ));
                    }
                    pending.push(other);
                }
            }
        }
        if !pending.is_empty() {
            return Err(Error::parse(
                line_no,
                format!("{s}\nAn edge needs a target block"),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'block-beta'"));
    }
    if stack.len() > 1 {
        let f = stack.last().map(|f| f.line).unwrap_or(1);
        return Err(Error::parse(f, "Unclosed 'block': expecting 'end'"));
    }
    let root = stack.pop().expect("the root frame is never popped");
    d.columns = root.columns;
    d.children = root.children;
    Ok(d)
}

fn make_edge(from: String, to: String, toks: &[Tok], line_no: usize, s: &str) -> Result<Edge> {
    let err = || Error::parse(line_no, format!("{s}\nExpecting '-->' between blocks"));
    let arrows: Vec<&str> = toks
        .iter()
        .filter_map(|t| match t {
            Tok::Arrow(a) => Some(a.as_str()),
            _ => None,
        })
        .collect();
    let label = toks.iter().find_map(|t| match t {
        Tok::Str(x) | Tok::Pipe(x) => Some(x.trim().to_string()),
        _ => None,
    });
    let last = arrows.last().ok_or_else(err)?;
    if arrows.len() > 2 {
        return Err(err());
    }
    let (line, mut start, end) = edge_kind(last);
    if arrows.len() == 2 && arrows[0].starts_with('<') {
        start = Head::Arrow;
    }
    Ok(Edge {
        from,
        to,
        label: label.filter(|l| !l.is_empty()),
        line,
        start,
        end,
    })
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Sizes used by the layout.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub font: f64,
    pub gap: f64,
    pub pad: f64,
}

fn node_size(label: &str, shape: BlockShape, font: f64) -> (f64, f64) {
    let tb = TextBlock::label(label, font);
    let (tw, th) = (tb.width, tb.height.max(font * 1.5));
    let pad = 16.0;
    let (w, h) = match shape {
        BlockShape::Circle => {
            let d = tw.max(th) + pad;
            (d, d)
        }
        BlockShape::DoubleCircle => {
            let d = tw.max(th) + pad + 10.0;
            (d, d)
        }
        BlockShape::Diamond => {
            let s = tw + th + pad;
            (s, s * 0.75)
        }
        BlockShape::Hexagon => (tw + 2.0 * pad + th, th + pad),
        BlockShape::Stadium => (tw + th + 2.0 * pad, th + pad),
        BlockShape::Cylinder => (tw + 2.0 * pad, th + pad + 20.0),
        BlockShape::LeanLeft
        | BlockShape::LeanRight
        | BlockShape::Trapezoid
        | BlockShape::InvTrapezoid => (tw + 2.0 * pad + th, th + pad),
        BlockShape::Asymmetric => (tw + 2.0 * pad + th / 2.0, th + pad),
        BlockShape::Arrow(d) => match d {
            ArrowDir::Up | ArrowDir::Down | ArrowDir::Y => (tw + pad, th + 2.0 * pad + 30.0),
            _ => (tw + 2.0 * pad + th, th + pad + 16.0),
        },
        _ => (tw + 2.0 * pad, th + pad),
    };
    (w.max(40.0), h.max(40.0))
}

fn columns_of(cols: Option<usize>, children: &[Block]) -> usize {
    cols.unwrap_or_else(|| children.iter().map(|c| c.span).sum::<usize>())
        .max(1)
}

/// Natural (minimum) width of a block.
fn natural_width(b: &Block, m: &Metrics) -> f64 {
    match &b.kind {
        BlockKind::Node { label, shape } => node_size(label, *shape, m.font).0,
        BlockKind::Space => 0.0,
        BlockKind::Composite { columns, children } => {
            let cols = columns_of(*columns, children);
            let track = track_width(children, cols, m);
            cols as f64 * track + (cols - 1) as f64 * m.gap + 2.0 * m.pad
        }
    }
}

fn track_width(children: &[Block], cols: usize, m: &Metrics) -> f64 {
    children
        .iter()
        .map(|c| {
            let span = c.span.min(cols).max(1) as f64;
            (natural_width(c, m) - m.gap * (span - 1.0)) / span
        })
        .fold(0.0, f64::max)
}

/// Row-major placement with wrapping: (row, column) 1-based per child.
pub fn place(children: &[Block], cols: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut row, mut col) = (1usize, 0usize);
    for c in children {
        let span = c.span.min(cols).max(1);
        if col + span > cols && col > 0 {
            row += 1;
            col = 0;
        }
        out.push((row, col + 1));
        col += span;
        if col >= cols {
            row += 1;
            col = 0;
        }
    }
    out
}

fn grid_node(children: &[Block], cols: usize, track: f64, m: &Metrics) -> Vec<BoxNode> {
    let pos = place(children, cols);
    children
        .iter()
        .zip(pos)
        .map(|(c, (row, col))| {
            let span = c.span.min(cols).max(1);
            let width = span as f64 * track + (span - 1) as f64 * m.gap;
            let mut style = match &c.kind {
                BlockKind::Node { label, shape } => {
                    let (_, h) = node_size(label, *shape, m.font);
                    boxes::min_size(taffy::Style::default(), 0.0, h)
                }
                BlockKind::Space => taffy::Style::default(),
                BlockKind::Composite { .. } => taffy::Style::default(),
            };
            style = boxes::grid_span(style, col as i16, span as u16);
            style.grid_row = taffy::Line {
                start: taffy::style_helpers::line(row as i16),
                end: taffy::style_helpers::span(1),
            };
            match &c.kind {
                BlockKind::Composite { columns, children } => {
                    let inner_cols = columns_of(*columns, children);
                    let inner_track = ((width - 2.0 * m.pad - (inner_cols - 1) as f64 * m.gap)
                        / inner_cols as f64)
                        .max(track_width(children, inner_cols, m));
                    let mut g =
                        boxes::padded(boxes::uniform_grid(inner_cols, inner_track, m.gap), m.pad);
                    g.grid_column = style.grid_column;
                    g.grid_row = style.grid_row;
                    BoxNode::new(g, grid_node(children, inner_cols, inner_track, m))
                }
                _ => BoxNode::new(style, Vec::new()),
            }
        })
        .collect()
}

/// Lay out the diagram; the result mirrors `d.children` (root is a padded grid).
pub fn layout(d: &BlockDiagram, m: &Metrics) -> Placed {
    let cols = columns_of(d.columns, &d.children);
    let track = track_width(&d.children, cols, m);
    let root = BoxNode::new(
        boxes::uniform_grid(cols, track, m.gap),
        grid_node(&d.children, cols, track, m),
    );
    boxes::compute(&root)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Outline of a drawn block, used to clip edges.
#[derive(Debug, Clone)]
pub(crate) enum Outline {
    Rect(Rect),
    Ellipse(Point, f64, f64),
    Polygon(Point, Vec<Point>),
}

impl Outline {
    pub(crate) fn clip(&self, toward: Point) -> Point {
        match self {
            Outline::Rect(r) => geom::clip_rect(r, toward),
            Outline::Ellipse(c, rx, ry) => geom::clip_ellipse(*c, *rx, *ry, toward),
            Outline::Polygon(c, pts) => geom::clip_polygon(*c, pts, toward),
        }
    }
}

/// Draw `shape` inside `r` (shared helper for box-like shapes).
pub(crate) fn shape_elements(shape: BlockShape, r: Rect, paint: &Paint) -> (Vec<Element>, Outline) {
    let c = r.center();
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let poly = |pts: Vec<Point>| {
        (
            vec![Element::classed_shape(
                Geometry::Path(geom::polygon(&pts)),
                paint.clone(),
                "label-container",
            )],
            Outline::Polygon(c, pts),
        )
    };
    let rect = |rx: f64| {
        (
            vec![Element::classed_shape(
                Geometry::Rect { rect: r, rx },
                paint.clone(),
                "label-container",
            )],
            Outline::Rect(r),
        )
    };
    match shape {
        BlockShape::Rect => rect(0.0),
        BlockShape::Rounded => rect(5.0),
        BlockShape::Stadium => rect(r.h / 2.0),
        BlockShape::Subroutine => {
            let (mut els, o) = rect(0.0);
            let lp = Paint::stroke(&paint.stroke, paint.stroke_width);
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
            (els, o)
        }
        BlockShape::Cylinder => {
            let rx = r.w / 2.0;
            let ry = cylinder_ry(r);
            let pi = std::f64::consts::PI;
            let top = Point::new(c.x, y0 + ry);
            let bot = Point::new(c.x, y1 - ry);
            let mut body = vec![
                PathCmd::MoveTo(Point::new(x0, y0 + ry)),
                PathCmd::LineTo(Point::new(x0, y1 - ry)),
            ];
            body.extend(geom::elliptic_arc_cubics(bot, rx, ry, pi, 0.0));
            body.push(PathCmd::LineTo(Point::new(x1, y0 + ry)));
            body.extend(geom::elliptic_arc_cubics(top, rx, ry, 0.0, -pi));
            body.push(PathCmd::Close);
            let mut front = vec![PathCmd::MoveTo(Point::new(x0, y0 + ry))];
            front.extend(geom::elliptic_arc_cubics(top, rx, ry, pi, 0.0));
            (
                vec![
                    Element::classed_shape(Geometry::Path(body), paint.clone(), "label-container"),
                    Element::path(front, Paint::stroke(&paint.stroke, paint.stroke_width)),
                ],
                Outline::Rect(r),
            )
        }
        BlockShape::Circle | BlockShape::DoubleCircle => {
            let rad = r.w.min(r.h) / 2.0;
            let mut els = vec![Element::classed_shape(
                Geometry::Circle { c, r: rad },
                paint.clone(),
                "label-container",
            )];
            if shape == BlockShape::DoubleCircle {
                els.push(Element::shape(
                    Geometry::Circle {
                        c,
                        r: (rad - 5.0).max(1.0),
                    },
                    Paint::stroke(&paint.stroke, paint.stroke_width),
                ));
            }
            (els, Outline::Ellipse(c, rad, rad))
        }
        BlockShape::Diamond => poly(vec![
            Point::new(c.x, y0),
            Point::new(x1, c.y),
            Point::new(c.x, y1),
            Point::new(x0, c.y),
        ]),
        BlockShape::Hexagon => {
            let m = (r.h / 4.0).min(r.w / 4.0);
            poly(vec![
                Point::new(x0 + m, y0),
                Point::new(x1 - m, y0),
                Point::new(x1, c.y),
                Point::new(x1 - m, y1),
                Point::new(x0 + m, y1),
                Point::new(x0, c.y),
            ])
        }
        BlockShape::LeanRight
        | BlockShape::LeanLeft
        | BlockShape::Trapezoid
        | BlockShape::InvTrapezoid => {
            let k = (r.h / 2.0).min(r.w / 4.0);
            let pts = match shape {
                BlockShape::LeanRight => vec![
                    Point::new(x0 + k, y0),
                    Point::new(x1, y0),
                    Point::new(x1 - k, y1),
                    Point::new(x0, y1),
                ],
                BlockShape::LeanLeft => vec![
                    Point::new(x0, y0),
                    Point::new(x1 - k, y0),
                    Point::new(x1, y1),
                    Point::new(x0 + k, y1),
                ],
                BlockShape::Trapezoid => vec![
                    Point::new(x0 + k, y0),
                    Point::new(x1 - k, y0),
                    Point::new(x1, y1),
                    Point::new(x0, y1),
                ],
                _ => vec![
                    Point::new(x0, y0),
                    Point::new(x1, y0),
                    Point::new(x1 - k, y1),
                    Point::new(x0 + k, y1),
                ],
            };
            poly(pts)
        }
        BlockShape::Asymmetric => {
            let k = r.h / 4.0;
            poly(vec![
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
                Point::new(x0 + k, c.y),
            ])
        }
        BlockShape::Arrow(dir) => poly(block_arrow(r, dir)),
    }
}

/// Vertical radius of a cylinder's end caps.
fn cylinder_ry(r: Rect) -> f64 {
    (r.w / 2.0 / (2.5 + r.w / 50.0)).min(r.h / 5.0)
}

/// Polygon of a block arrow filling `r`.
fn block_arrow(r: Rect, dir: ArrowDir) -> Vec<Point> {
    let c = r.center();
    // Horizontal template: shaft half-height `s`, head length `hl`.
    let horiz = |double: bool, flip: bool| {
        let hl = (r.h / 2.0).min(r.w / 3.0);
        let s = r.h * 0.3;
        let (x0, x1) = (r.x, r.right());
        let mut pts = if double {
            vec![
                Point::new(x0, c.y),
                Point::new(x0 + hl, r.y),
                Point::new(x0 + hl, c.y - s),
                Point::new(x1 - hl, c.y - s),
                Point::new(x1 - hl, r.y),
                Point::new(x1, c.y),
                Point::new(x1 - hl, r.bottom()),
                Point::new(x1 - hl, c.y + s),
                Point::new(x0 + hl, c.y + s),
                Point::new(x0 + hl, r.bottom()),
            ]
        } else {
            vec![
                Point::new(x0, c.y - s),
                Point::new(x1 - hl, c.y - s),
                Point::new(x1 - hl, r.y),
                Point::new(x1, c.y),
                Point::new(x1 - hl, r.bottom()),
                Point::new(x1 - hl, c.y + s),
                Point::new(x0, c.y + s),
            ]
        };
        if flip {
            for p in &mut pts {
                p.x = 2.0 * c.x - p.x;
            }
        }
        pts
    };
    let vert = |double: bool, flip: bool| {
        let hl = (r.w / 2.0).min(r.h / 3.0);
        let s = r.w * 0.3;
        let (y0, y1) = (r.y, r.bottom());
        let mut pts = if double {
            vec![
                Point::new(c.x, y0),
                Point::new(r.right(), y0 + hl),
                Point::new(c.x + s, y0 + hl),
                Point::new(c.x + s, y1 - hl),
                Point::new(r.right(), y1 - hl),
                Point::new(c.x, y1),
                Point::new(r.x, y1 - hl),
                Point::new(c.x - s, y1 - hl),
                Point::new(c.x - s, y0 + hl),
                Point::new(r.x, y0 + hl),
            ]
        } else {
            vec![
                Point::new(c.x - s, y0),
                Point::new(c.x + s, y0),
                Point::new(c.x + s, y1 - hl),
                Point::new(r.right(), y1 - hl),
                Point::new(c.x, y1),
                Point::new(r.x, y1 - hl),
                Point::new(c.x - s, y1 - hl),
            ]
        };
        if flip {
            for p in &mut pts {
                p.y = 2.0 * c.y - p.y;
            }
        }
        pts
    };
    match dir {
        ArrowDir::Right => horiz(false, false),
        ArrowDir::Left => horiz(false, true),
        ArrowDir::X => horiz(true, false),
        ArrowDir::Down => vert(false, false),
        ArrowDir::Up => vert(false, true),
        ArrowDir::Y => vert(true, false),
    }
}

/// Render a block diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let gap = if d.edges.is_empty() { 8.0 } else { 24.0 };
    let m = Metrics {
        font: pal.font_size,
        gap: cfg.section_f64("block", "padding", gap).max(gap),
        pad: 8.0,
    };
    let placed = layout(&d, &m);
    Ok(draw(&d, &placed, &m, pal))
}

fn style_for(d: &BlockDiagram, id: &str) -> StyleOverrides {
    let mut st = StyleOverrides::default();
    for (bid, cls) in &d.classes {
        if bid == id {
            if let Some((_, s)) = d.class_defs.iter().find(|(n, _)| n == cls) {
                st = st.merged(s);
            }
        }
    }
    for (bid, s) in &d.styles {
        if bid == id {
            st = st.merged(s);
        }
    }
    st
}

struct Drawn {
    outlines: Vec<(String, Outline)>,
}

#[allow(clippy::too_many_arguments)]
fn draw_blocks(
    d: &BlockDiagram,
    blocks: &[Block],
    placed: &[Placed],
    m: &Metrics,
    pal: &Palette,
    bounds: &mut Bounds,
    out: &mut Vec<Element>,
    drawn: &mut Drawn,
) {
    for (b, p) in blocks.iter().zip(placed) {
        let r = p.rect;
        let st = style_for(d, &b.id);
        match &b.kind {
            BlockKind::Space => {}
            BlockKind::Composite { children, .. } => {
                bounds.add_rect(r);
                let mut paint = Paint::both(
                    st.fill.as_ref().unwrap_or(&pal.cluster_bkg),
                    st.stroke.as_ref().unwrap_or(&pal.cluster_border),
                    st.stroke_width.unwrap_or(1.0),
                );
                paint.dash = st.dash.clone();
                let mut g = vec![Element::classed_shape(
                    Geometry::Rect { rect: r, rx: 4.0 },
                    paint,
                    "composite",
                )];
                draw_blocks(d, children, &p.children, m, pal, bounds, &mut g, drawn);
                drawn.outlines.push((b.id.clone(), Outline::Rect(r)));
                out.push(Element::group(format!("block composite {}", b.id), g));
            }
            BlockKind::Node { label, shape } => {
                // Circles and diamonds keep their natural size, centred in the cell.
                let (nw, nh) = node_size(label, *shape, m.font);
                let rr = match shape {
                    BlockShape::Circle | BlockShape::DoubleCircle => {
                        let s = r.w.min(r.h).min(nw.max(nh));
                        Rect::centered(r.center(), s, s)
                    }
                    BlockShape::Diamond | BlockShape::Arrow(_) => {
                        Rect::centered(r.center(), r.w.min(nw.max(r.w * 0.6)), r.h.min(nh.max(r.h)))
                    }
                    _ => r,
                };
                bounds.add_rect(rr);
                let mut paint = Paint::both(
                    st.fill.as_ref().unwrap_or(&pal.main_bkg),
                    st.stroke.as_ref().unwrap_or(&pal.node_border),
                    st.stroke_width.unwrap_or(1.0),
                );
                paint.dash = st.dash.clone();
                let (mut els, outline) = shape_elements(*shape, rr, &paint);
                let tb = TextBlock::label(label, m.font);
                let color: Color = st.color.clone().unwrap_or_else(|| pal.node_text.clone());
                let center = if *shape == BlockShape::Cylinder {
                    rr.center().plus(Point::new(0.0, cylinder_ry(rr) / 2.0))
                } else {
                    rr.center()
                };
                let mut t = Text::centered(center, &tb.lines, m.font, tb.line_height, &color);
                if st.bold == Some(true) {
                    t = t.bold();
                }
                els.push(Element::classed_text(t, "label"));
                let mut class = format!("node {}", b.id);
                for (bid, c) in &d.classes {
                    if bid == &b.id {
                        class.push(' ');
                        class.push_str(c);
                    }
                }
                drawn.outlines.push((b.id.clone(), outline));
                out.push(Element::group(class, els));
            }
        }
    }
}

/// Draw a laid-out block diagram.
pub fn draw(d: &BlockDiagram, placed: &Placed, m: &Metrics, pal: &Palette) -> Scene {
    let mut bounds = Bounds::default();
    let mut blocks = Vec::new();
    let mut drawn = Drawn {
        outlines: Vec::new(),
    };
    draw_blocks(
        d,
        &d.children,
        &placed.children,
        m,
        pal,
        &mut bounds,
        &mut blocks,
        &mut drawn,
    );
    let mut edges = Vec::new();
    let mut labels = Vec::new();
    for e in &d.edges {
        let find = |id: &str| drawn.outlines.iter().find(|(i, _)| i == id).map(|(_, o)| o);
        let (Some(a), Some(b)) = (find(&e.from), find(&e.to)) else {
            continue;
        };
        let ca = outline_center(a);
        let cb = outline_center(b);
        let pa = a.clip(cb);
        let pb = b.clip(ca);
        let width = match e.line {
            Line::Thick => 3.5,
            _ => 2.0,
        };
        let mut paint = Paint::stroke(&pal.line_color, width);
        if e.line == Line::Dotted {
            paint = paint.dashed(vec![3.0]);
        }
        let dir = pb.minus(pa).norm();
        let trim = |h: Head| if h == Head::Arrow { 8.0 } else { 0.0 };
        let sa = pa.plus(dir.scale(trim(e.start)));
        let sb = pb.minus(dir.scale(trim(e.end)));
        edges.push(Element::classed_shape(
            Geometry::Path(vec![PathCmd::MoveTo(sa), PathCmd::LineTo(sb)]),
            paint,
            "edge",
        ));
        edges.extend(head(e.end, pb, dir, &pal.line_color));
        edges.extend(head(e.start, pa, dir.scale(-1.0), &pal.line_color));
        bounds.add_point(pa);
        bounds.add_point(pb);
        if let Some(l) = &e.label {
            let tb = TextBlock::label(l, m.font);
            let mid = pa.lerp(pb, 0.5);
            let bg = Rect::centered(mid, tb.width + 6.0, tb.height + 2.0);
            bounds.add_rect(bg);
            labels.push(Element::rect(
                bg,
                0.0,
                Paint::fill(&pal.edge_label_background),
            ));
            labels.push(Element::classed_text(
                Text::centered(mid, &tb.lines, m.font, tb.line_height, &pal.text_color),
                "edgeLabel",
            ));
        }
    }
    let mut els = vec![
        Element::group("blocks", blocks),
        Element::group("edges", edges),
        Element::group("edgeLabels", labels),
    ];
    if let Some(t) = d.title.as_ref().filter(|t| !t.is_empty()) {
        let r = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], 18.0, false);
        let c = Point::new(r.center().x, r.y - 20.0);
        bounds.add_rect(Rect::centered(c, tb.width, tb.height));
        els.push(Element::classed_text(
            Text::centered(c, &tb.lines, 18.0, tb.line_height, &pal.title_color),
            "blockTitle",
        ));
    }
    Scene {
        kind: DiagramKind::Block,
        view_box: bounds.rect().pad(8.0),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn outline_center(o: &Outline) -> Point {
    match o {
        Outline::Rect(r) => r.center(),
        Outline::Ellipse(c, _, _) | Outline::Polygon(c, _) => *c,
    }
}

fn head(h: Head, tip: Point, dir: Point, color: &Color) -> Vec<Element> {
    let n = Point::new(-dir.y, dir.x);
    match h {
        Head::None => Vec::new(),
        Head::Arrow => {
            let base = tip.minus(dir.scale(10.0));
            vec![Element::path(
                geom::polygon(&[tip, base.plus(n.scale(5.0)), base.minus(n.scale(5.0))]),
                Paint::both(color, color, 1.0),
            )]
        }
        Head::Circle => vec![Element::shape(
            Geometry::Circle {
                c: tip.minus(dir.scale(5.0)),
                r: 5.0,
            },
            Paint::both(color, color, 1.0),
        )],
        Head::Cross => {
            let c = tip.minus(dir.scale(6.0));
            let a = dir.plus(n).scale(4.5);
            let b = dir.minus(n).scale(4.5);
            vec![Element::path(
                vec![
                    PathCmd::MoveTo(c.minus(a)),
                    PathCmd::LineTo(c.plus(a)),
                    PathCmd::MoveTo(c.minus(b)),
                    PathCmd::LineTo(c.plus(b)),
                ],
                Paint::stroke(color, 2.0),
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"block-beta
columns 3
a["Frontend"] b:2
block:backend:2
  columns 2
  api(("API")) db[("DB")]
end
c{"Decide"}
space:2 arrow<["go"]>(down)
a --> api
api -- "query" --> db
c -.-> a
style a fill:#f9f
classDef hot fill:#f66
class c hot
"#;

    fn m() -> Metrics {
        Metrics {
            font: 16.0,
            gap: 24.0,
            pad: 8.0,
        }
    }

    #[test]
    fn parses_blocks_and_edges() {
        let d = parse(SRC, None).unwrap();
        assert_eq!(d.columns, Some(3));
        assert_eq!(d.children.len(), 6);
        assert_eq!(d.children[1].span, 2);
        let BlockKind::Composite { columns, children } = &d.children[2].kind else {
            panic!("expected composite");
        };
        assert_eq!(*columns, Some(2));
        assert_eq!(children.len(), 2);
        assert_eq!(
            children[0].kind,
            BlockKind::Node {
                label: "API".into(),
                shape: BlockShape::Circle
            }
        );
        assert_eq!(
            children[1].kind,
            BlockKind::Node {
                label: "DB".into(),
                shape: BlockShape::Cylinder
            }
        );
        assert_eq!(d.children[2].span, 2);
        assert!(matches!(d.children[4].kind, BlockKind::Space));
        assert_eq!(d.children[4].span, 2);
        assert_eq!(
            d.children[5].kind,
            BlockKind::Node {
                label: "go".into(),
                shape: BlockShape::Arrow(ArrowDir::Down)
            }
        );
        assert_eq!(d.edges.len(), 3);
        assert_eq!(d.edges[1].label.as_deref(), Some("query"));
        assert_eq!(d.edges[2].line, Line::Dotted);
        assert_eq!(d.styles.len(), 1);
        assert_eq!(d.classes, vec![("c".to_string(), "hot".to_string())]);
    }

    #[test]
    fn tokenizes_shapes() {
        let cases = [
            ("a[/x/]", BlockShape::LeanRight),
            ("a[\\x\\]", BlockShape::LeanLeft),
            ("a[/x\\]", BlockShape::Trapezoid),
            ("a[\\x/]", BlockShape::InvTrapezoid),
            ("a{{x}}", BlockShape::Hexagon),
            ("a>x]", BlockShape::Asymmetric),
            ("a(((x)))", BlockShape::DoubleCircle),
            ("a([x])", BlockShape::Stadium),
            ("a[[x]]", BlockShape::Subroutine),
            ("a(x)", BlockShape::Rounded),
        ];
        for (src, want) in cases {
            let t = tokenize(src, 1).unwrap();
            match &t[0] {
                Tok::Node {
                    shape: Some((s, l)),
                    ..
                } => {
                    assert_eq!(*s, want, "{src}");
                    assert_eq!(l, "x");
                }
                other => panic!("{src}: {other:?}"),
            }
        }
        let t = tokenize("a -->|lbl| b <--> c --x d", 1).unwrap();
        assert_eq!(t.len(), 8);
    }

    #[test]
    fn parse_errors_have_lines() {
        let e = parse("block-beta\na[\"x\"\n", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }), "{e:?}");
        let e = parse("block-beta\nblock:x\n a\n", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }), "{e:?}");
        let e = parse("block-beta\na\nend", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("block-beta\na -->", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("block-beta\ncolumns x", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("block-beta\na<[\"x\"]>(sideways)", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
    }

    #[test]
    fn grid_placement_wraps_rows() {
        let d = parse(SRC, None).unwrap();
        let pos = place(&d.children, 3);
        assert_eq!(pos, vec![(1, 1), (1, 2), (2, 1), (2, 3), (3, 1), (3, 3)]);
    }

    #[test]
    fn layout_spans_and_no_overlap() {
        let d = parse(SRC, None).unwrap();
        let p = layout(&d, &m());
        let r: Vec<Rect> = p.children.iter().map(|c| c.rect).collect();
        // Equal tracks: b spans two tracks plus one gap.
        let track = r[0].w;
        assert!((r[1].w - (2.0 * track + 24.0)).abs() < 0.5, "{r:?}");
        assert!((r[2].w - r[1].w).abs() < 0.5);
        for i in 0..r.len() {
            for j in i + 1..r.len() {
                if i == 4 || j == 4 {
                    continue; // space
                }
                assert!(!r[i].intersects(&r[j]), "{i} {:?} vs {j} {:?}", r[i], r[j]);
            }
        }
        // Nested children lie inside their composite.
        let comp = &p.children[2];
        for c in &comp.children {
            assert!(c.rect.x >= comp.rect.x && c.rect.right() <= comp.rect.right() + 0.5);
            assert!(c.rect.y >= comp.rect.y && c.rect.bottom() <= comp.rect.bottom() + 0.5);
        }
        assert!(!comp.children[0].rect.intersects(&comp.children[1].rect));
    }

    #[test]
    fn renders_edges() {
        let d = parse(SRC, None).unwrap();
        let p = layout(&d, &m());
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let s = draw(&d, &p, &m(), &pal);
        assert!(s.view_box.w > 100.0 && s.view_box.h > 100.0);
    }
}
