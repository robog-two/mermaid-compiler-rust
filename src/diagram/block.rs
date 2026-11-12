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
//! Layout is a port of mermaid's own block layout (`setBlockSizes` /
//! `layoutBlocks`): all children of a container share the widest per-column
//! width and the tallest height (so a composite makes its siblings as tall as
//! itself), rows of `columns` slots are separated by `block.padding`, and a
//! wider container stretches its children. `space:N` is N one-slot spaces.
//! Edges are straight lines between block centres clipped to the outlines.

use crate::config::MermaidConfig;
use crate::diagram::graph::StyleOverrides;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::boxes::Placed;
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
    Arrow(ArrowDirs),
}

/// One block-arrow direction as written (`x`/`y` = both horizontal/vertical ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowDir {
    Right,
    Left,
    Up,
    Down,
    X,
    Y,
}

impl ArrowDir {
    fn parse(s: &str) -> Option<ArrowDir> {
        Some(match s {
            "right" => ArrowDir::Right,
            "left" => ArrowDir::Left,
            "up" => ArrowDir::Up,
            "down" => ArrowDir::Down,
            "x" => ArrowDir::X,
            "y" => ArrowDir::Y,
            _ => return None,
        })
    }
}

/// The set of directions a block arrow points in (`block.jison`'s `dirList`,
/// expanded and deduplicated as `blockArrow.ts` does).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArrowDirs {
    pub right: bool,
    pub left: bool,
    pub up: bool,
    pub down: bool,
}

impl ArrowDirs {
    pub fn of(dirs: &[ArrowDir]) -> ArrowDirs {
        let mut d = ArrowDirs::default();
        for dir in dirs {
            match dir {
                ArrowDir::Right => d.right = true,
                ArrowDir::Left => d.left = true,
                ArrowDir::Up => d.up = true,
                ArrowDir::Down => d.down = true,
                ArrowDir::X => (d.right, d.left) = (true, true),
                ArrowDir::Y => (d.up, d.down) = (true, true),
            }
        }
        d
    }
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
    (
        "<[",
        "]>",
        BlockShape::Arrow(ArrowDirs {
            right: true,
            left: false,
            up: false,
            down: false,
        }),
    ),
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
                // block.jison ARROW_DIR: one or more `,?\s*(right|left|x|y|up|down)\s*`.
                let mut dirs = Vec::new();
                let mut rest = dir.trim_start();
                while !rest.is_empty() {
                    if !dirs.is_empty() {
                        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
                    }
                    let n = rest
                        .find(|c: char| c == ',' || c.is_whitespace())
                        .unwrap_or(rest.len());
                    let d = ArrowDir::parse(&rest[..n]).ok_or_else(|| {
                        err(&format!("Unknown block arrow direction '{}'", dir.trim()))
                    })?;
                    dirs.push(d);
                    rest = rest[n..].trim_start();
                }
                if dirs.is_empty() {
                    return Err(err("Expecting a block arrow direction like '(right)'"));
                }
                BlockShape::Arrow(ArrowDirs::of(&dirs))
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
            // The lexer only knows `columns auto` / `columns <digits>`; otherwise
            // `columns` is an ordinary block id.
            "columns"
                if rest == "auto"
                    || (!rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())) =>
            {
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
                    if id == "space" && shape.is_none() {
                        prev = None;
                        if !pending.is_empty() {
                            return Err(Error::parse(
                                line_no,
                                format!("{s}\n'space' cannot be linked"),
                            ));
                        }
                        // mermaid expands `space:N` into N one-column spaces.
                        let top = stack.last_mut().expect("the root frame is never popped");
                        for _ in 0..span.unwrap_or(1).max(1) {
                            space_n += 1;
                            top.children.push(Block {
                                id: format!("__space{space_n}"),
                                kind: BlockKind::Space,
                                span: 1,
                            });
                        }
                        continue;
                    }
                    let real_id = id.clone();
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
    /// `block.padding`: gap between blocks and inset of composites.
    pub pad: f64,
}

/// A block node's `padding` (`block.padding`, 8) as the shapes see it.
const NODE_PADDING: f64 = 8.0;

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
        // mermaid `block_arrow`: height = label + 2 * padding(8), width = label + height + 8.
        BlockShape::Arrow(_) => (tw + th + pad + 8.0, th + pad),
        _ => (tw + 2.0 * pad, th + pad),
    };
    (w.max(40.0), h.max(40.0))
}

/// Working copy of a block for mermaid's sizing passes (`block.size`).
#[derive(Debug, Clone)]
struct Sized {
    /// `columns` setting, `-1` for auto (mermaid's encoding).
    columns: i64,
    span: usize,
    space: bool,
    w: f64,
    h: f64,
    x: f64,
    y: f64,
    children: Vec<Sized>,
}

/// Initial `getBBox()` size of each block, before layout.
fn sized_tree(blocks: &[Block], m: &Metrics) -> Vec<Sized> {
    blocks
        .iter()
        .map(|b| {
            let (w, h, columns, children) = match &b.kind {
                BlockKind::Node { label, shape } => {
                    let (w, h) = node_size(label, *shape, m.font);
                    (w, h, -1, Vec::new())
                }
                // Spaces are measured as empty rects; their size is never read.
                BlockKind::Space => (2.0 * m.pad, m.pad, -1, Vec::new()),
                // An unlabelled composite measures 0 wide, i.e. "no size yet".
                BlockKind::Composite { columns, children } => (
                    0.0,
                    0.0,
                    columns.map_or(-1, |c| c as i64),
                    sized_tree(children, m),
                ),
            };
            Sized {
                columns,
                span: b.span.max(1),
                space: matches!(b.kind, BlockKind::Space),
                w,
                h,
                x: 0.0,
                y: 0.0,
                children,
            }
        })
        .collect()
}

/// Port of mermaid's `setBlockSizes` (diagrams/block/layout.ts): every child
/// of a container gets the widest (per column) and tallest child size, the
/// container wraps them in `padding`, and children grow to fill a container
/// that is wider than they need.
fn set_block_sizes(b: &mut Sized, sibling_w: f64, sibling_h: f64, pad: f64) {
    if b.w == 0.0 || b.w.is_nan() {
        b.w = sibling_w;
        b.h = sibling_h;
        b.x = 0.0;
        b.y = 0.0;
    }
    if b.children.is_empty() {
        return;
    }
    for c in &mut b.children {
        set_block_sizes(c, 0.0, 0.0, pad);
    }
    let (mut max_w, mut max_h) = (0.0f64, 0.0f64);
    for c in b.children.iter().filter(|c| !c.space) {
        max_w = max_w.max(c.w / c.span as f64);
        max_h = max_h.max(c.h);
    }
    for c in &mut b.children {
        let span = c.span as f64;
        c.w = max_w * span + pad * (span - 1.0);
        c.h = max_h;
        c.x = 0.0;
        c.y = 0.0;
    }
    for c in &mut b.children {
        set_block_sizes(c, max_w, max_h, pad);
    }
    let num_items: usize = b.children.iter().map(|c| c.span).sum();
    let mut x_size = b.children.len();
    if b.columns > 0 && (b.columns as usize) < num_items {
        x_size = b.columns as usize;
    }
    let y_size = num_items.div_ceil(x_size);
    let (xs, ys) = (x_size as f64, y_size as f64);
    let mut width = xs * (max_w + pad) + pad;
    let mut height = ys * (max_h + pad) + pad;
    if width < sibling_w {
        width = sibling_w;
        height = sibling_h;
        let cw = (sibling_w - xs * pad - pad) / xs;
        let ch = (sibling_h - ys * pad - pad) / ys;
        for c in &mut b.children {
            c.w = cw;
            c.h = ch;
            c.x = 0.0;
            c.y = 0.0;
        }
    }
    if width < b.w {
        width = b.w;
        let num = if b.columns > 0 {
            b.children.len().min(b.columns as usize)
        } else {
            b.children.len()
        };
        if num > 0 {
            let cw = (width - num as f64 * pad - pad) / num as f64;
            for c in &mut b.children {
                c.w = cw;
            }
        }
    }
    b.w = width;
    b.h = height;
    b.x = 0.0;
    b.y = 0.0;
}

/// mermaid's `calculateBlockPosition`: grid cell of the `position`-th column slot.
fn block_position(columns: i64, position: usize) -> (usize, usize) {
    if columns < 0 {
        (position, 0)
    } else if columns == 1 {
        (0, position)
    } else {
        let c = columns as usize;
        (position % c, position / c)
    }
}

/// Port of mermaid's `layoutBlocks`: children flow left to right in rows of
/// `columns` slots, each row as tall as its tallest child; a new row restarts
/// at the container's left edge. Centres are absolute (`size.x/y`).
fn layout_blocks(b: &mut Sized, pad: f64) {
    if b.children.is_empty() {
        return;
    }
    let columns = b.columns;
    let filled = |span: usize, pos: usize| {
        if columns > 0 {
            span.min(columns as usize - pos % columns as usize)
        } else {
            span
        }
    };
    let mut row_h: std::collections::BTreeMap<usize, f64> = Default::default();
    let mut pos = 0usize;
    for c in &b.children {
        let (_, py) = block_position(columns, pos);
        let e = row_h.entry(py).or_insert(0.0);
        *e = e.max(c.h);
        pos += filled(c.span, pos);
    }
    let mut row_off = std::collections::BTreeMap::new();
    let mut off = 0.0;
    for (row, h) in &row_h {
        row_off.insert(*row, off);
        off += h + pad;
    }
    // `block.size.x ? x - width / 2 : -padding` (a container centred at 0 starts at -padding).
    let row_start = if b.x != 0.0 { b.x - b.w / 2.0 } else { -pad };
    let (by, bh) = (b.y, b.h);
    let mut start_x = row_start;
    let mut row_pos = 0usize;
    let mut pos = 0usize;
    for c in &mut b.children {
        let (_, py) = block_position(columns, pos);
        if py != row_pos {
            row_pos = py;
            start_x = row_start;
        }
        let half = c.w / 2.0;
        c.x = start_x + pad + half;
        start_x = c.x + half;
        let rh = row_h.get(&py).copied().unwrap_or(c.h);
        c.y = by - bh / 2.0 + row_off.get(&py).copied().unwrap_or(0.0) + rh / 2.0 + pad;
        layout_blocks(c, pad);
        pos += filled(c.span, pos);
    }
}

fn to_placed(s: &Sized) -> Placed {
    Placed {
        rect: Rect::centered(Point::new(s.x, s.y), s.w, s.h),
        children: s.children.iter().map(to_placed).collect(),
    }
}

/// Lay out the diagram exactly like mermaid's block layout; the result
/// mirrors `d.children` (the root rect is the diagram's bounds).
pub fn layout(d: &BlockDiagram, m: &Metrics) -> Placed {
    let mut root = Sized {
        columns: d.columns.map_or(-1, |c| c as i64),
        span: 1,
        space: false,
        w: 0.0,
        h: 0.0,
        x: 0.0,
        y: 0.0,
        children: sized_tree(&d.children, m),
    };
    set_block_sizes(&mut root, 0.0, 0.0, m.pad);
    layout_blocks(&mut root, m.pad);
    let children: Vec<Placed> = root.children.iter().map(to_placed).collect();
    let mut bounds = Rect::new(0.0, 0.0, 0.0, 0.0);
    fn grow(b: &mut Rect, p: &Placed) {
        *b = b.union(&p.rect);
        for c in &p.children {
            grow(b, c);
        }
    }
    for c in &children {
        grow(&mut bounds, c);
    }
    Placed {
        rect: bounds,
        children,
    }
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
        BlockShape::Arrow(dirs) => poly(block_arrow(r, dirs, NODE_PADDING)),
    }
}

/// Vertical radius of a cylinder's end caps.
fn cylinder_ry(r: Rect) -> f64 {
    (r.w / 2.0 / (2.5 + r.w / 50.0)).min(r.h / 5.0)
}

/// Polygon of a block arrow in `r` (`blockArrow.ts` `getArrowPoints`): `pad` is the
/// node padding; points are laid out from the bottom-left corner upwards, and the
/// polygon of each direction set is upstream's (some, such as `up`, `down` or
/// `right|left|down`, are not arrow-like there either).
fn block_arrow(r: Rect, dirs: ArrowDirs, pad: f64) -> Vec<Point> {
    let p = pad / 2.0;
    let (h, w) = (r.h, r.w);
    let m = h / 2.0;
    let key = (dirs.right, dirs.left, dirs.up, dirs.down);
    let pts: Vec<(f64, f64)> = match key {
        (true, true, true, true) => vec![
            (0.0, 0.0),
            (m, 0.0),
            (w / 2.0, 2.0 * p),
            (w - m, 0.0),
            (w, 0.0),
            (w, -h / 3.0),
            (w + 2.0 * p, -h / 2.0),
            (w, -2.0 * h / 3.0),
            (w, -h),
            (w - m, -h),
            (w / 2.0, -h - 2.0 * p),
            (m, -h),
            (0.0, -h),
            (0.0, -2.0 * h / 3.0),
            (-2.0 * p, -h / 2.0),
            (0.0, -h / 3.0),
        ],
        (true, true, true, false) => vec![
            (m, 0.0),
            (w - m, 0.0),
            (w, -h / 2.0),
            (w - m, -h),
            (m, -h),
            (0.0, -h / 2.0),
        ],
        (true, true, false, true) => vec![(0.0, 0.0), (m, -h), (w - m, -h), (w, 0.0)],
        (true, false, true, true) => vec![(0.0, 0.0), (w, -m), (w, -h + m), (0.0, -h)],
        (false, true, true, true) => vec![(w, 0.0), (0.0, -m), (0.0, -h + m), (w, -h)],
        (true, true, false, false) => vec![
            (m, 0.0),
            (m, -p),
            (w - m, -p),
            (w - m, 0.0),
            (w, -h / 2.0),
            (w - m, -h),
            (w - m, -h + p),
            (m, -h + p),
            (m, -h),
            (0.0, -h / 2.0),
        ],
        (false, false, true, true) => vec![
            (w / 2.0, 0.0),
            (0.0, -p),
            (m, -p),
            (m, -h + p),
            (0.0, -h + p),
            (w / 2.0, -h),
            (w, -h + p),
            (w - m, -h + p),
            (w - m, -p),
            (w, -p),
        ],
        (true, false, true, false) => vec![(0.0, 0.0), (w, -m), (0.0, -h)],
        (true, false, false, true) => vec![(0.0, 0.0), (w, 0.0), (0.0, -h)],
        (false, true, true, false) => vec![(w, 0.0), (0.0, -m), (w, -h)],
        (false, true, false, true) => vec![(w, 0.0), (0.0, 0.0), (w, -h)],
        (true, false, false, false) => vec![
            (m, -p),
            (m, -p),
            (w - m, -p),
            (w - m, 0.0),
            (w, -h / 2.0),
            (w - m, -h),
            (w - m, -h + p),
            (m, -h + p),
            (m, -h + p),
        ],
        (false, true, false, false) => vec![
            (m, 0.0),
            (m, -p),
            (w - m, -p),
            (w - m, -h + p),
            (m, -h + p),
            (m, -h),
            (0.0, -h / 2.0),
        ],
        (false, false, true, false) => vec![
            (m, -p),
            (m, -h + p),
            (0.0, -h + p),
            (w / 2.0, -h),
            (w, -h + p),
            (w - m, -h + p),
            (w - m, -p),
        ],
        (false, false, false, true) => vec![
            (w / 2.0, 0.0),
            (0.0, -p),
            (m, -p),
            (m, -h + p),
            (w - m, -h + p),
            (w - m, -p),
            (w, -p),
        ],
        (false, false, false, false) => vec![(0.0, 0.0)],
    };
    pts.into_iter()
        .map(|(x, y)| Point::new(r.x + x, r.bottom() + y))
        .collect()
}

/// Render a block diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let m = Metrics {
        font: pal.font_size,
        pad: cfg.section_f64("block", "padding", 8.0),
    };
    let placed = layout(&d, &m);
    Ok(draw(&d, &placed, &m, pal))
}

/// khroma's `fade(c, alpha)`: the colour with its alpha scaled to `alpha`.
fn fade(c: &Color, alpha: f64) -> Color {
    match crate::color::parse(&c.0) {
        Some(rgba) => Color::new(format!(
            "rgba({}, {}, {}, {})",
            rgba.r,
            rgba.g,
            rgba.b,
            rgba.a * alpha
        )),
        None => c.clone(),
    }
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
                // `.node .cluster { fill: fade(clusterBkg, 0.5); stroke: fade(clusterBorder, 0.2) }`
                let mut paint = Paint::both(
                    &st.fill
                        .clone()
                        .unwrap_or_else(|| fade(&pal.cluster_bkg, 0.5)),
                    &st.stroke
                        .clone()
                        .unwrap_or_else(|| fade(&pal.cluster_border, 0.2)),
                    st.stroke_width.unwrap_or(1.0),
                );
                paint.dash = st.dash.clone();
                let mut g = vec![Element::classed_shape(
                    Geometry::Rect { rect: r, rx: 0.0 },
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
                    // Block arrows keep their natural size unless they span columns.
                    BlockShape::Arrow(_) => {
                        Rect::centered(r.center(), if b.span > 1 { r.w.max(nw) } else { nw }, nh)
                    }
                    BlockShape::Diamond => {
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
            Text::centered(c, &tb.lines, 18.0, tb.line_height, &pal.text_color),
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
            pad: 8.0,
        }
    }

    #[test]
    fn parses_blocks_and_edges() {
        let d = parse(SRC, None).unwrap();
        assert_eq!(d.columns, Some(3));
        assert_eq!(d.children.len(), 7);
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
        assert!(matches!(d.children[5].kind, BlockKind::Space));
        assert_eq!(
            d.children[6].kind,
            BlockKind::Node {
                label: "go".into(),
                shape: BlockShape::Arrow(ArrowDirs::of(&[ArrowDir::Down]))
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
        // `columns x` is not a columns statement but two blocks, as upstream.
        let d = parse("block-beta\ncolumns x", None).unwrap();
        assert_eq!(d.children.len(), 2);
        let e = parse("block-beta\na<[\"x\"]>(sideways)", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
    }

    #[test]
    fn space_n_expands_to_n_spaces() {
        let d = parse("block-beta\nspace:3 a", None).unwrap();
        assert_eq!(d.children.len(), 4);
        assert!(
            d.children[..3]
                .iter()
                .all(|c| c.kind == BlockKind::Space && c.span == 1)
        );
    }

    /// Geometry of mermaid 12.3.0's rendering of this diagram (centre x, y, w, h).
    #[test]
    fn nested_layout_matches_mermaid() {
        let src = "block-beta\ncolumns 3\na b c\nblock:outer:3\ncolumns 2\nblock:inner1\nx y\nend\nblock:inner2\ncolumns 1\nz w\nend\nend\nspace:2 d";
        let d = parse(src, None).unwrap();
        let p = layout(&d, &m());
        let check = |r: Rect, cx: f64, cy: f64, w: f64, h: f64| {
            let c = r.center();
            assert!(
                (c.x - cx).abs() < 0.01 && (c.y - cy).abs() < 0.01,
                "{r:?} vs centre ({cx}, {cy})"
            );
            assert!(
                (r.w - w).abs() < 0.01 && (r.h - h).abs() < 0.01,
                "{r:?} vs {w}x{h}"
            );
        };
        let third = 232.0 / 3.0;
        // Siblings of a composite take its height (mermaid's quirk).
        check(p.children[0].rect, third / 2.0, -128.0, third, 120.0);
        check(p.children[1].rect, 124.0, -128.0, third, 120.0);
        let outer = &p.children[3];
        check(outer.rect, 124.0, 0.0, 248.0, 120.0);
        check(outer.children[0].rect, 64.0, 0.0, 112.0, 56.0);
        check(outer.children[0].children[0].rect, 36.0, 0.0, 40.0, 40.0);
        check(outer.children[1].rect, 184.0, 0.0, 112.0, 104.0);
        check(outer.children[1].children[0].rect, 180.0, -24.0, 88.0, 40.0);
        check(outer.children[1].children[1].rect, 180.0, 24.0, 88.0, 40.0);
        check(p.children[6].rect, 124.0 + third + 8.0, 128.0, third, 120.0);
    }

    #[test]
    fn layout_spans_and_no_overlap() {
        let d = parse(SRC, None).unwrap();
        let p = layout(&d, &m());
        let r: Vec<Rect> = p.children.iter().map(|c| c.rect).collect();
        // Equal tracks: b spans two tracks plus one padding.
        let track = r[0].w;
        assert!((r[1].w - (2.0 * track + 8.0)).abs() < 0.5, "{r:?}");
        assert!((r[2].w - r[1].w).abs() < 0.5);
        for i in 0..r.len() {
            for j in i + 1..r.len() {
                if matches!(d.children[i].kind, BlockKind::Space)
                    || matches!(d.children[j].kind, BlockKind::Space)
                {
                    continue;
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
    fn block_arrows_take_a_list_of_directions() {
        let d = parse(
            "block\n  a<[\"A\"]>(x, down)\n  b<[\"B\"]>( y )\n  c<[\"C\"]>(right,left up)",
            None,
        )
        .unwrap();
        let dirs = |i: usize| match &d.children[i].kind {
            BlockKind::Node {
                shape: BlockShape::Arrow(a),
                ..
            } => *a,
            k => panic!("{k:?}"),
        };
        let all = |r, l, u, dn| ArrowDirs {
            right: r,
            left: l,
            up: u,
            down: dn,
        };
        assert_eq!(dirs(0), all(true, true, false, true));
        assert_eq!(dirs(1), all(false, false, true, true));
        assert_eq!(dirs(2), all(true, true, true, false));
        assert!(parse("block\n  a<[\"A\"]>(sideways)", None).is_err());
        assert!(parse("block\n  a<[\"A\"]>()", None).is_err());
    }

    #[test]
    fn block_arrow_polygons_follow_get_arrow_points() {
        // `x, down` is upstream's trapezoid; points run from the bottom-left corner.
        let r = Rect::new(0.0, 0.0, 87.15625, 40.0);
        let one = |r: bool, l: bool, u: bool, d: bool| {
            block_arrow(
                Rect::new(0.0, 0.0, 87.15625, 40.0),
                ArrowDirs {
                    right: r,
                    left: l,
                    up: u,
                    down: d,
                },
                8.0,
            )
        };
        let p = one(true, true, false, true);
        let xy: Vec<(f64, f64)> = p.iter().map(|p| (p.x, p.y)).collect();
        assert_eq!(
            xy,
            [(0.0, 40.0), (20.0, 0.0), (67.15625, 0.0), (87.15625, 40.0)]
        );
        // `right`: shaft inset by padding / 2, head from `width - midpoint`.
        let p = one(true, false, false, false);
        assert_eq!((p[2].x, p[2].y), (67.15625, 36.0));
        assert_eq!((p[4].x, p[4].y), (r.w, 20.0));
        // `up|down` (`y`) is a double-headed vertical arrow.
        let p = one(false, false, true, true);
        assert_eq!((p[0].x, p[0].y), (r.w / 2.0, 40.0));
        assert_eq!((p[5].x, p[5].y), (r.w / 2.0, 0.0));
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
