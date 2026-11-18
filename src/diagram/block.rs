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
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
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
#[derive(Debug, Clone)]
pub struct Metrics {
    pub font: f64,
    /// `block.padding`: gap between blocks and inset of composites.
    pub pad: f64,
    /// The neo look, whose shapes pad their labels by fixed amounts of their own.
    pub neo: bool,
    /// Under the handDrawn look: rough.js's options for the nodes (each
    /// block's own style applied in [`hand_style`]).
    pub hand: Option<super::hand::HandStyle>,
}

/// The primitive a block shape draws, around the node centre.
#[derive(Debug, Clone, PartialEq)]
enum Prim {
    Rect {
        w: f64,
        h: f64,
        rx: f64,
    },
    /// Polygon points relative to the node centre.
    Poly(Vec<Point>),
    Circle(f64),
    /// Inner and outer radius.
    DoubleCircle(f64, f64),
    /// Width, height between the cap centres, and the caps' radii.
    Cylinder {
        w: f64,
        h: f64,
        rx: f64,
        ry: f64,
    },
    /// Outer width (frame included) and height.
    Subroutine {
        w: f64,
        h: f64,
    },
    Stadium {
        w: f64,
        h: f64,
    },
}

/// A block node as mermaid's rendering-util shape functions build it: the shape
/// around the node centre and its HTML label box.
#[derive(Debug, Clone, PartialEq)]
struct NodeGeom {
    prim: Prim,
    /// Label box centre relative to the node centre.
    label_off: Point,
    /// Label box size: the text's own width, clipped to the width the shape
    /// passes to `labelHelper`, and its height.
    label_w: f64,
    label_h: f64,
    /// The label's lines as laid out, and whether its box cuts the text off.
    lines: Vec<String>,
    clipped: bool,
    /// What the shape's handDrawn branch draws (`None`: no rough variant).
    rough: Option<super::hand::Sized>,
}

/// A block label laid out in at most `limit` pixels (`createText`'s
/// `addHtmlSpan`).
struct Fitted {
    lines: Vec<String>,
    w: f64,
    h: f64,
    clipped: bool,
}

/// `addHtmlSpan`: a label wider than `limit` gets `max-width: limit`. Its box
/// then measures `limit` in layout units (1/64 px); only when that equals
/// `limit` exactly does mermaid switch the box to `display: table` with
/// `break-spaces` wrapping (a word that does not fit widens it). Otherwise the
/// single line overflows the box and is cut off.
fn fit_label(tb: &TextBlock, limit: f64) -> Fitted {
    if tb.width <= limit {
        return Fitted {
            lines: tb.lines.clone(),
            w: tb.width,
            h: tb.height,
            clipped: false,
        };
    }
    let snapped = (limit * 64.0).floor() / 64.0;
    if snapped != limit {
        return Fitted {
            lines: tb.lines.clone(),
            w: snapped.max(0.0),
            h: tb.height,
            clipped: true,
        };
    }
    let lines = crate::text::wrap_lines(
        &tb.lines,
        limit,
        tb.font_size,
        tb.bold,
        crate::text::BreakRule::Browser,
    );
    let wrapped = TextBlock::new(lines, tb.font_size, tb.bold).html();
    Fitted {
        w: wrapped.width.max(limit),
        h: wrapped.height,
        lines: wrapped.lines,
        clipped: false,
    }
}

/// Width of the frame `subroutine` adds on each side.
const SUBROUTINE_FRAME: f64 = 8.0;

/// Cap radius of a cylinder `w` wide (`rx / (2.5 + w / 50)`).
fn cylinder_ry(w: f64) -> f64 {
    w / 2.0 / (2.5 + w / 50.0)
}

/// Build `shape` for a label `lw × lh`. `cell` is `None` while mermaid measures
/// the block (`calculateBlockSize`: no `node.width/height`) and the laid-out
/// size once it draws it (`insertBlockPositioned`), which some shapes fill and
/// others ignore. `span` is the block's column span (block arrows stretch over
/// several columns only). `p` is `block.padding`; under the neo look (`neo`)
/// most shapes pad their labels by fixed amounts instead.
fn node_geom(
    shape: BlockShape,
    label: &TextBlock,
    cell: Option<(f64, f64)>,
    span: usize,
    (p, neo): (f64, bool),
) -> NodeGeom {
    // Per look: the classic value, else the neo one.
    let pick = |classic: f64, neo_value: f64| if neo { neo_value } else { classic };
    let (nw, nh) = cell.unwrap_or((0.0, 0.0));
    // The width `labelHelper` lays the label out in (`node.width`, or no limit).
    let wrap = |w: f64| if cell.is_some() { w } else { f64::INFINITY };
    let mut label_off = Point::new(0.0, 0.0);
    use super::hand::Sized;
    let mut rough = None;
    let (prim, fitted) = match shape {
        BlockShape::Rect | BlockShape::Rounded => {
            // squareRect / drawRect: labelPaddingX = 2p, labelPaddingY = p (neo:
            // 16 and 12), each side; a rounded one is `roundedRect`, p each
            // side in either look.
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let (rx, pad_x, pad_y) = if shape == BlockShape::Rounded {
                (5.0, p, p)
            } else {
                (0.0, pick(2.0 * p, 16.0), pick(p, 12.0))
            };
            let w = (lw + 2.0 * pad_x).max(nw);
            let h = (lh + 2.0 * pad_y).max(nh);
            rough = Some(Sized::Rect { w, h, rx });
            (Prim::Rect { w, h, rx }, f)
        }
        BlockShape::Diamond => {
            // question: a square diamond of side (label + padding) summed, its
            // polygon shifted right by 0.5.
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let s = lw + p + lh + p;
            let (d, at) = super::hand::question_path(s);
            rough = Some(Sized::Path { d, at });
            let pts = [
                (s / 2.0, 0.0),
                (s, -s / 2.0),
                (s / 2.0, -s),
                (0.0, -s / 2.0),
            ];
            let pts = pts
                .iter()
                .map(|&(x, y)| Point::new(x - s / 2.0 + 0.5, y + s / 2.0))
                .collect();
            (Prim::Poly(pts), f)
        }
        BlockShape::Hexagon => {
            // hexagon (f = 4, neo 3.5): a laid-out hexagon fills its cell, the
            // label limited to what the slanted ends leave. Neo pads the
            // height by 70 and the width by 32 (`labelPaddingX` / `Y`, swapped).
            let slant = pick(4.0, 3.5);
            let (pad_h, pad_w) = (pick(p, 70.0), pick(p, 32.0));
            let (h, label_limit) = match cell {
                Some((w, h)) => (h, w - 2.0 * (h / slant) - pad_w),
                None => (label.height + pad_h, f64::INFINITY),
            };
            let f = fit_label(label, label_limit);
            let lw = f.w;
            let m = h / slant;
            let w = if cell.is_some() {
                nw
            } else {
                lw + 2.0 * m + pad_w
            };
            let (d, at) = super::hand::hexagon_path(w, h, m);
            rough = Some(Sized::Path { d, at });
            let pts = [
                (m, 0.0),
                (w - m, 0.0),
                (w, -h / 2.0),
                (w - m, -h),
                (m, -h),
                (0.0, -h / 2.0),
            ];
            (Prim::Poly(shift(&pts, w, h)), f)
        }
        BlockShape::Arrow(dirs) => {
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let h = lh + 2.0 * p;
            let natural = lw + h + p;
            let w = if cell.is_some() && span > 1 && nw > natural {
                nw
            } else {
                natural
            };
            let r = Rect::new(-w / 2.0, -h / 2.0, w, h);
            (Prim::Poly(block_arrow(r, dirs, p)), f)
        }
        BlockShape::Asymmetric => {
            // rect_left_inv_arrow: the notch is a quarter of the height; polygon
            // and label move right by half the notch.
            // Neo pads the label by 21 and 12 on each side.
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let h = (lh + pick(p, 24.0)).max(nh);
            let notch = h / 4.0;
            let w = (lw + pick(p, 42.0)).max(nw - notch);
            let dx = notch / 2.0;
            let local = [
                Point::new(-w / 2.0 - notch, -h / 2.0),
                Point::new(-w / 2.0, 0.0),
                Point::new(-w / 2.0 - notch, h / 2.0),
                Point::new(w / 2.0, h / 2.0),
                Point::new(w / 2.0, -h / 2.0),
            ];
            rough = Some(Sized::Path {
                d: super::hand::path_from_points(&local),
                at: Point::new(dx, 0.0),
            });
            let pts = vec![
                Point::new(-w / 2.0 - notch + dx, -h / 2.0),
                Point::new(-w / 2.0 + dx, 0.0),
                Point::new(-w / 2.0 - notch + dx, h / 2.0),
                Point::new(w / 2.0 + dx, h / 2.0),
                Point::new(w / 2.0 + dx, -h / 2.0),
            ];
            label_off = Point::new(dx, 0.0);
            (Prim::Poly(pts), f)
        }
        BlockShape::LeanRight
        | BlockShape::LeanLeft
        | BlockShape::Trapezoid
        | BlockShape::InvTrapezoid => {
            // The slanted ends add half the height on each side; `inv_trapezoid`
            // pads its label on both sides. Neo pads the width twice as much.
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let k = if shape == BlockShape::InvTrapezoid {
                2.0
            } else {
                1.0
            };
            let h = (lh + k * p).max(nh);
            let w = (lw + k * pick(p, 2.0 * p)).max(nw - h);
            let s = 3.0 * h / 6.0;
            let pts = match shape {
                BlockShape::LeanRight => [(-s, 0.0), (w, 0.0), (w + s, -h), (0.0, -h)],
                BlockShape::LeanLeft => [(0.0, 0.0), (w + s, 0.0), (w, -h), (-s, -h)],
                BlockShape::Trapezoid => [(-s, 0.0), (w + s, 0.0), (w, -h), (0.0, -h)],
                _ => [(0.0, 0.0), (w, 0.0), (w + s, -h), (-s, -h)],
            };
            let local: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
            rough = Some(Sized::Path {
                d: super::hand::path_from_points(&local),
                at: Point::new(-w / 2.0, h / 2.0),
            });
            (Prim::Poly(shift(&pts, w, h)), f)
        }
        BlockShape::Circle => {
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let r = lw.hypot(lh) / 2.0 + pick(p / 2.0, 16.0);
            rough = Some(Sized::Circle(r));
            (Prim::Circle(r), f)
        }
        BlockShape::DoubleCircle => {
            // doublecircle: a laid-out one takes half the cell width as its
            // label radius, so it outgrows its cell by the padding and gap
            // (neo: 16 and 12).
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let inner = if cell.is_some() {
                nw / 2.0
            } else {
                lw.hypot(lh) / 2.0
            } + pick(p, 16.0);
            let outer = inner + pick(5.0, 12.0);
            rough = Some(Sized::DoubleCircle { inner, outer });
            (Prim::DoubleCircle(inner, outer), f)
        }
        BlockShape::Stadium => {
            // stadiumDimensions: the arc is a 50-point polygon, so the caps are a
            // hair narrower than the height. Neo pads by 40 and 24.
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            let (px, py) = (pick(p, 40.0), pick(p, 24.0));
            let h = lh + py;
            let inscribed = h * (std::f64::consts::PI / (2.0 * 49.0)).cos();
            let cap = (inscribed * inscribed - lh * lh).max(0.0).sqrt();
            let w = (lw + h / 4.0 + px)
                .max(1.5 * h + (h - inscribed))
                .max(lw + px + h - cap);
            rough = Some(Sized::Path {
                d: super::hand::path_from_points(&stadium_points(w, h)),
                at: Point::default(),
            });
            (Prim::Stadium { w, h }, f)
        }
        BlockShape::Subroutine => {
            let f = fit_label(label, wrap(nw));
            let (lw, lh) = (f.w, f.h);
            // Neo pads by 28 and 12.
            let w = (lw + 2.0 * SUBROUTINE_FRAME + pick(p, 28.0)).max(nw);
            let h = (lh + pick(p, 12.0)).max(nh);
            rough = Some(Sized::Subroutine { w, h });
            (Prim::Subroutine { w, h }, f)
        }
        BlockShape::Cylinder => {
            // cylinder: a laid-out one gives the label its cell less the padding
            // (neo: 24) and three cap radii, and grows back to fill the cell.
            let q = pick(p, 24.0);
            let (label_limit, min_h) = match cell {
                Some((w, h)) => ((w - q).max(8.0), (h - q - 3.0 * cylinder_ry(w)).max(8.0)),
                None => (f64::INFINITY, 0.0),
            };
            let f = fit_label(label, label_limit);
            let (lw, lh) = (f.w, f.h);
            let w = if cell.is_some() {
                label_limit.max(lw)
            } else {
                lw
            } + q;
            let rx = w / 2.0;
            let ry = cylinder_ry(w);
            let h = min_h.max(lh) + q + ry;
            label_off = Point::new(0.0, p / 1.5);
            rough = Some(Sized::Cylinder { w, h, rx, ry });
            (Prim::Cylinder { w, h, rx, ry }, f)
        }
    };
    NodeGeom {
        prim,
        label_off,
        label_w: fitted.w,
        label_h: fitted.h,
        lines: fitted.lines,
        clipped: fitted.clipped,
        rough,
    }
}

/// `insertPolygonShape`'s `translate(-w / 2, h / 2)` of points given from the
/// bottom-left corner.
fn shift(pts: &[(f64, f64)], w: f64, h: f64) -> Vec<Point> {
    pts.iter()
        .map(|&(x, y)| Point::new(x - w / 2.0, y + h / 2.0))
        .collect()
}

/// stadium.ts's outline about the centre: the straight top, then 50-point
/// arcs of radius `h / 2` (`generateCirclePoints`, negated) for the caps,
/// which never reach the cap's outermost point.
fn stadium_points(w: f64, h: f64) -> Vec<Point> {
    let r = h / 2.0;
    let arc = |cx: f64, a0: f64, a1: f64| {
        let pi = std::f64::consts::PI;
        let (s, e) = (a0 * pi / 180.0, a1 * pi / 180.0);
        let step = (e - s) / 49.0;
        (0..50).map(move |i| {
            let a = s + f64::from(i) * step;
            Point::new(-(cx + r * a.cos()), -(r * a.sin()))
        })
    };
    let mut pts = vec![
        Point::new(-w / 2.0 + r, -h / 2.0),
        Point::new(w / 2.0 - r, -h / 2.0),
    ];
    pts.extend(arc(-w / 2.0 + r, 90.0, 270.0));
    pts.push(Point::new(w / 2.0 - r, h / 2.0));
    pts.extend(arc(w / 2.0 - r, 270.0, 450.0));
    pts
}

impl NodeGeom {
    /// The node's size as `calculateBlockSize` takes it: its group's
    /// `getBBox()`. A cylinder's path is drawn about its own corner and moved
    /// into place by a transform, and the browser measures it in single
    /// precision there, which the rows' heights carry.
    /// [`NodeGeom::measured_size`], drawn by rough.js under the handDrawn look.
    fn measured_size_in(&self, hand: Option<&super::hand::HandStyle>) -> (f64, f64) {
        if let Some(hn) = self.hand_node(hand) {
            let b =
                hn.drawn_bbox()
                    .union(&Rect::centered(self.label_off, self.label_w, self.label_h));
            return (b.w, b.h);
        }
        if let Prim::Cylinder { w, h, rx, ry } = self.prim {
            let l = Rect::centered(self.label_off, self.label_w, self.label_h);
            let [_, _, bw, bh] =
                super::svgbbox::cylinder_node_box(w, h, rx, ry, [l.x, l.y, l.w, l.h]);
            return (bw, bh);
        }
        let b = self.bbox();
        (b.w, b.h)
    }

    /// The rough.js drawing of the shape under the handDrawn look.
    fn hand_node(&self, hand: Option<&super::hand::HandStyle>) -> Option<super::hand::HandNode> {
        let (st, shape) = (hand?, self.rough.as_ref()?);
        Some(super::hand::sized_node(shape, st))
    }

    /// The node's bounding box around its centre (shape and label, as `getBBox`).
    fn bbox(&self) -> Rect {
        let shape = match &self.prim {
            Prim::Rect { w, h, .. } | Prim::Subroutine { w, h } => {
                Rect::centered(Point::new(0.0, 0.0), *w, *h)
            }
            Prim::Stadium { w, h } => {
                let mut b = Bounds::default();
                for p in stadium_points(*w, *h) {
                    b.add_point(p);
                }
                b.rect()
            }
            Prim::Poly(pts) => {
                let mut b = Bounds::default();
                for p in pts {
                    b.add_point(*p);
                }
                b.rect()
            }
            Prim::Circle(r) | Prim::DoubleCircle(_, r) => {
                Rect::centered(Point::new(0.0, 0.0), 2.0 * r, 2.0 * r)
            }
            Prim::Cylinder { w, h, ry, .. } => {
                Rect::centered(Point::new(0.0, 0.0), *w, h + 2.0 * ry)
            }
        };
        shape.union(&Rect::centered(self.label_off, self.label_w, self.label_h))
    }

    /// The shape drawn around `c`, and its outline for clipping edges.
    fn draw(&self, c: Point, paint: &Paint) -> (Vec<Element>, Outline) {
        let at = |p: &Point| c.plus(*p);
        match &self.prim {
            Prim::Rect { w, h, rx } => {
                let r = Rect::centered(c, *w, *h);
                (
                    vec![Element::classed_shape(
                        Geometry::Rect { rect: r, rx: *rx },
                        paint.clone(),
                        "label-container",
                    )],
                    Outline::Rect(r),
                )
            }
            Prim::Stadium { w, h } => {
                // A polygon (rough.js draws its straight segments, roughness 0).
                let pts: Vec<Point> = stadium_points(*w, *h).iter().map(at).collect();
                (
                    vec![Element::classed_shape(
                        Geometry::Path(geom::polygon(&pts)),
                        paint.clone(),
                        "label-container",
                    )],
                    Outline::Polygon(c, pts),
                )
            }
            Prim::Subroutine { w, h } => {
                // subroutine.ts: one polygon, the inner frame's rectangle then the
                // outer one (both edges of the frame drawn by its outline).
                let r = Rect::centered(c, *w, *h);
                let (x0, x1) = (r.x + SUBROUTINE_FRAME, r.right() - SUBROUTINE_FRAME);
                let (top, bot) = (r.y, r.bottom());
                let pts = [
                    (x0, bot),
                    (x1, bot),
                    (x1, top),
                    (x0, top),
                    (x0, bot),
                    (r.x, bot),
                    (r.right(), bot),
                    (r.right(), top),
                    (r.x, top),
                    (r.x, bot),
                ]
                .map(|(x, y)| Point::new(x, y));
                (
                    vec![Element::classed_shape(
                        Geometry::Path(geom::polygon(&pts)),
                        paint.clone(),
                        "label-container",
                    )],
                    Outline::Rect(r),
                )
            }
            Prim::Poly(pts) => {
                let pts: Vec<Point> = pts.iter().map(at).collect();
                (
                    vec![Element::classed_shape(
                        Geometry::Path(geom::polygon(&pts)),
                        paint.clone(),
                        "label-container",
                    )],
                    Outline::Polygon(c, pts),
                )
            }
            Prim::Circle(r) => (
                vec![Element::classed_shape(
                    Geometry::Circle { c, r: *r },
                    paint.clone(),
                    "label-container",
                )],
                Outline::Ellipse(c, *r, *r),
            ),
            Prim::DoubleCircle(inner, outer) => (
                vec![
                    Element::classed_shape(
                        Geometry::Circle { c, r: *outer },
                        paint.clone(),
                        "outer-circle",
                    ),
                    Element::classed_shape(
                        Geometry::Circle { c, r: *inner },
                        paint.clone(),
                        "inner-circle",
                    ),
                ],
                Outline::Ellipse(c, *outer, *outer),
            ),
            Prim::Cylinder { w, h, rx, ry } => {
                // createCylinderPathD about the shape's corner: the top ellipse
                // (front half, then back), down the left side, the bottom's
                // front half, up the right side; one path, filled and stroked,
                // moved into place by `translate(-w / 2, -(h / 2 + ry))`.
                let arc = |dx: f64| PathCmd::ArcTo {
                    rx: *rx,
                    ry: *ry,
                    large: false,
                    sweep: false,
                    to: Point::new(dx, 0.0),
                    rel: true,
                };
                let body = vec![
                    PathCmd::MoveTo(Point::new(0.0, *ry)),
                    arc(*w),
                    arc(-w),
                    PathCmd::LineTo(Point::new(0.0, ry + h)),
                    arc(*w),
                    PathCmd::LineTo(Point::new(*w, *ry)),
                ];
                // `updateNodeBounds`: the node's size is the path's getBBox,
                // in that frame and single precision; whether an edge lands on
                // a side or a cap hangs on it.
                let (bw, bh) = super::svgbbox::cylinder_box(*w, *h, *rx, *ry);
                (
                    vec![Element::Translated {
                        dx: c.x - w / 2.0,
                        dy: c.y - (h / 2.0 + ry),
                        children: vec![Element::classed_shape(
                            Geometry::Path(body),
                            paint.clone(),
                            "label-container",
                        )],
                    }],
                    Outline::Cylinder(c, bw, bh, *rx, *ry),
                )
            }
        }
    }
}

/// A block's HTML label box (layout units, one 1.5em line box per line).
fn label_box(label: &str, font: f64) -> TextBlock {
    TextBlock::label(&crate::icons::replace_references(label), font).html()
}

/// `calculateBlockSize`: the bounding box of the block as drawn unpositioned.
fn node_size(label: &str, shape: BlockShape, span: usize, m: &Metrics) -> (f64, f64) {
    let tb = label_box(label, m.font);
    node_geom(shape, &tb, None, span, (m.pad, m.neo)).measured_size_in(m.hand.as_ref())
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
                    let (w, h) = node_size(label, *shape, b.span.max(1), m);
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
    /// `intersect.polygon` as written: of the crossings of the segment from
    /// the centre to the point, the one nearest the point (a polygon that
    /// doubles back, as subroutine's frame, has several).
    NearestPolygon(Point, Vec<Point>),
    /// A cylinder's centre, bounding box size and cap radii.
    Cylinder(Point, f64, f64, f64, f64),
}

impl Outline {
    pub(crate) fn clip(&self, toward: Point) -> Point {
        match self {
            Outline::Rect(r) => geom::clip_rect(r, toward),
            Outline::Ellipse(c, rx, ry) => geom::clip_ellipse(*c, *rx, *ry, toward),
            Outline::Polygon(c, pts) => geom::clip_polygon(*c, pts, toward),
            Outline::NearestPolygon(c, pts) => {
                let d = toward.minus(*c);
                let mut best: Option<f64> = None;
                for (i, &a) in pts.iter().enumerate() {
                    let e = pts[(i + 1) % pts.len()].minus(a);
                    let den = d.x * e.y - d.y * e.x;
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let ac = a.minus(*c);
                    let t = (ac.x * e.y - ac.y * e.x) / den;
                    let u = (ac.x * d.y - ac.y * d.x) / den;
                    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                        best = Some(best.map_or(t, |bt: f64| bt.max(t)));
                    }
                }
                best.map_or(*c, |t| c.plus(d.scale(t)))
            }
            &Outline::Cylinder(c, bw, bh, rx, ry) => {
                // cylinder.ts `intersect`: the box's, moved onto the cap's
                // ellipse when it lands on the top or bottom side.
                // intersect.rect, operation for operation: whether the point
                // is on a side (`|x| == width / 2`) is an exact comparison.
                let (dx, dy) = (toward.x - c.x, toward.y - c.y);
                let (mut w, mut h) = (bw / 2.0, bh / 2.0);
                let (sx, sy) = if dy.abs() * w > dx.abs() * h {
                    if dy < 0.0 {
                        h = -h;
                    }
                    (if dy == 0.0 { 0.0 } else { h * dx / dy }, h)
                } else {
                    if dx < 0.0 {
                        w = -w;
                    }
                    (w, if dx == 0.0 { 0.0 } else { w * dy / dx })
                };
                let mut pos = Point::new(c.x + sx, c.y + sy);
                let x = pos.x - c.x;
                #[allow(clippy::float_cmp)]
                if rx != 0.0
                    && (x.abs() < bw / 2.0
                        || x.abs() == bw / 2.0 && (pos.y - c.y).abs() > bh / 2.0 - ry)
                {
                    let mut y = ry * ry * (1.0 - x * x / (rx * rx));
                    if y > 0.0 {
                        y = y.sqrt();
                    }
                    y = ry - y;
                    if toward.y - c.y > 0.0 {
                        y = -y;
                    }
                    pos.y += y;
                }
                pos
            }
        }
    }
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
        neo: cfg.look == crate::config::Look::Neo,
        hand: (cfg.look == crate::config::Look::HandDrawn).then(|| super::hand::HandStyle {
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
        }),
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
                // The shape built again with its laid-out size: some shapes fill
                // their cell, others keep their natural size (or outgrow it).
                let tb = label_box(label, m.font);
                let g = node_geom(*shape, &tb, Some((r.w, r.h)), b.span.max(1), (m.pad, m.neo));
                let c = r.center();
                let hand = m.hand.as_ref().map(|h| hand_style(h, &st));
                let rough = g.hand_node(hand.as_ref());
                let bb = match &rough {
                    Some(hn) => hn.drawn_bbox().union(&g.bbox()),
                    None => g.bbox(),
                };
                bounds.add_rect(Rect::new(bb.x + c.x, bb.y + c.y, bb.w, bb.h));
                let mut paint = Paint::both(
                    st.fill.as_ref().unwrap_or(&pal.main_bkg),
                    st.stroke.as_ref().unwrap_or(&pal.node_border),
                    // `.node rect, circle, polygon, path { stroke-width: strokeWidth }`.
                    st.stroke_width.unwrap_or(pal.stroke_width.unwrap_or(1.0)),
                );
                paint.dash = st.dash.clone();
                let (mut els, mut outline) = g.draw(c, &paint);
                if let Some(hn) = &rough {
                    els = super::graph::hand_elements(hn, c, st.dash.as_ref());
                    // The shapes' `intersect`s see the node as big as its
                    // rough drawing (`updateNodeBounds`).
                    let (w, h) = (hn.bbox.w, hn.bbox.h);
                    outline = match (&g.prim, outline) {
                        (Prim::Rect { .. }, _) => Outline::Rect(Rect::centered(c, w, h)),
                        // subroutine.ts: the frame's polygon.
                        (Prim::Subroutine { w: ow, h: oh }, _) => {
                            let (iw, ih) = (ow - 2.0 * SUBROUTINE_FRAME, *oh);
                            let local = [
                                (0.0, 0.0),
                                (iw, 0.0),
                                (iw, -ih),
                                (0.0, -ih),
                                (0.0, 0.0),
                                (-8.0, 0.0),
                                (iw + 8.0, 0.0),
                                (iw + 8.0, -ih),
                                (-8.0, -ih),
                                (-8.0, 0.0),
                            ]
                            .map(|(x, y)| Point::new(x, y))
                            .to_vec();
                            Outline::NearestPolygon(c, super::hand::in_box(c, w, h, local))
                        }
                        // question.ts: a diamond as wide as the box.
                        (Prim::Poly(_), _) if *shape == BlockShape::Diamond => {
                            let local = vec![
                                Point::new(w / 2.0, 0.0),
                                Point::new(w, -w / 2.0),
                                Point::new(w / 2.0, -w),
                                Point::new(0.0, -w / 2.0),
                            ];
                            Outline::Polygon(c, super::hand::in_box(c, w, h, local))
                        }
                        // cylinder.ts: the box's crossing moved by the nominal
                        // caps, the box the measured outline.
                        (Prim::Cylinder { rx, ry, .. }, _) => Outline::Cylinder(c, w, h, *rx, *ry),
                        (Prim::Poly(_) | Prim::Stadium { .. }, Outline::Polygon(_, pts)) => {
                            let local = pts.iter().map(|q| q.minus(c)).collect();
                            Outline::Polygon(c, super::hand::in_box(c, w, h, local))
                        }
                        (_, o) => o,
                    };
                }
                if m.neo {
                    neo_node(&mut els, *shape, &st, pal);
                }
                // `.label text, span { color: nodeTextColor || textColor }`.
                let color: Color = st
                    .color
                    .clone()
                    .unwrap_or_else(|| pal.node_text_or_text().clone());
                let center = c.plus(g.label_off);
                let mut t = Text::centered(center, &g.lines, m.font, tb.line_height, &color);
                if g.clipped {
                    t = t.anchored(Anchor::Start, center.x - g.label_w / 2.0);
                }
                if st.bold == Some(true) {
                    t = t.bold();
                }
                let label = Element::classed_text(t, "label");
                els.push(if g.clipped {
                    // A label wider than the width the shape gave it overflows its
                    // `foreignObject` to the right, which cuts it off (mid-glyph).
                    let h = tb.line_height * g.lines.len() as f64;
                    Element::Clipped {
                        rect: Rect::new(
                            center.x - g.label_w / 2.0,
                            center.y - h / 2.0,
                            g.label_w,
                            h,
                        ),
                        children: vec![label],
                    }
                } else {
                    label
                });
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

/// `userNodeOverrides` for a block styled `st`.
fn hand_style(base: &super::hand::HandStyle, st: &StyleOverrides) -> super::hand::HandStyle {
    super::hand::HandStyle {
        fill: st
            .fill
            .as_ref()
            .map_or(base.fill.clone(), |c| c.css().to_string()),
        stroke: st
            .stroke
            .as_ref()
            .map_or(base.stroke.clone(), |c| c.css().to_string()),
        stroke_width: st.stroke_width.unwrap_or(base.stroke_width),
        ..base.clone()
    }
}

/// The shared stylesheet's `[data-look="neo"].node` rules on a block's shape
/// (mermaid 12.3.0 draws blocks with the flowchart shapes): the theme's
/// `dropShadow` on its outline (each ring of a double circle), and every outline
/// in `nodeBorder` or, with `useGradient`, the `{id}-gradient`, unless a `style`
/// stroke (inline) wins.
fn neo_node(els: &mut Vec<Element>, shape: BlockShape, st: &StyleOverrides, pal: &Palette) {
    fn outlines(els: &mut [Element], f: &mut dyn FnMut(&mut Element)) {
        for el in els {
            match el {
                Element::Group { children, .. } | Element::Translated { children, .. } => {
                    outlines(children, f)
                }
                Element::Shape { .. } => f(el),
                _ => {}
            }
        }
    }
    /// The vector holding the first shape (inside any wrapping groups).
    fn first_level(els: &mut Vec<Element>) -> &mut Vec<Element> {
        if !matches!(
            els.first(),
            Some(Element::Group { .. } | Element::Translated { .. })
        ) {
            return els;
        }
        match &mut els[0] {
            Element::Group { children, .. } | Element::Translated { children, .. } => {
                first_level(children)
            }
            _ => unreachable!("checked above"),
        }
    }
    if st.stroke.is_none() {
        let gradient = pal
            .node_gradient
            .as_ref()
            .map(|(from, to)| crate::scene::Gradient::across_bbox(from, to));
        outlines(els, &mut |el| {
            if let Element::Shape { paint, .. } = el
                && !paint.stroke.is_none()
            {
                paint.stroke = pal.node_border.clone();
                paint.stroke_gradient = gradient.clone();
            }
        });
    }
    let Some(sh) = pal.drop_shadow.as_ref() else {
        return;
    };
    if shape == BlockShape::DoubleCircle {
        let level = first_level(els);
        let mut out = Vec::with_capacity(level.len() + 2);
        for el in level.drain(..) {
            if matches!(
                &el,
                Element::Shape {
                    geom: Geometry::Circle { .. },
                    ..
                }
            ) {
                let mut one = vec![el];
                super::graph::cast_shadow(&mut one, sh);
                out.extend(one);
            } else {
                out.push(el);
            }
        }
        *level = out;
    } else {
        super::graph::cast_shadow(first_level(els), sh);
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
        // blockRenderer: the edge runs through the blocks' centres and the
        // point half-way between them; insertEdge cuts its ends at the
        // outlines toward that middle point and curves it (curveBasis).
        let ca = outline_center(a);
        let cb = outline_center(b);
        let mid = Point::new(ca.x + (cb.x - ca.x) / 2.0, ca.y + (cb.y - ca.y) / 2.0);
        let pa = a.clip(mid);
        let pb = b.clip(mid);
        // `.edge-thickness-normal` is the theme's `strokeWidth` (1px by default)
        // in the shared stylesheet, `-thick` 3.5px; `.edge-pattern-dotted` dashes 2.
        let width = match e.line {
            Line::Thick => 3.5,
            _ => pal.stroke_width.unwrap_or(1.0),
        };
        let mut paint = Paint::stroke(&pal.line_color, width);
        if e.line == Line::Dotted {
            paint = paint.dashed(vec![2.0]);
        }
        // The markers turn with the line's first and last segments (orient
        // auto); Chromium turns a marker on a zero-length segment to angle 0.
        let toward = |from: Point, to: Point| {
            let d = to.minus(from);
            if d.len() < 1e-12 {
                Point::new(1.0, 0.0)
            } else {
                d.norm()
            }
        };
        // getLineFunctionsWithOffset: an arrow end (`arrow_point`) is pulled
        // back 4px toward its neighbour, and a point within 4px of it on both
        // axes is pushed clear of it.
        let offset = |h: Head| (h == Head::Arrow).then_some(4.0);
        let line =
            crate::diagram::graph::line_with_offset(&[pa, mid, pb], offset(e.start), offset(e.end));
        let path = geom::basis_curve(&line);
        if let Some(bb) = geom::path_bbox(&path) {
            bounds.add_rect(bb);
        }
        // curveBasis starts with a line toward the second point and ends with
        // one from the second-to-last: the markers' directions.
        let (out_a, in_b) = (toward(line[0], line[1]), toward(line[1], line[2]));
        edges.push(Element::classed_shape(Geometry::Path(path), paint, "edge"));
        edges.extend(marker(
            e.end,
            line[line.len() - 1],
            in_b,
            false,
            &pal.line_color,
        ));
        edges.extend(marker(e.start, line[0], out_a, true, &pal.line_color));
        bounds.add_point(pa);
        bounds.add_point(pb);
        if let Some(l) = &e.label {
            // The label sits on the edge's middle point, half-way between the
            // blocks' centres, its `labelBkg` box exactly the label's.
            let tb = label_box(l, m.font);
            let mid = ca.lerp(cb, 0.5);
            let bg = Rect::centered(mid, tb.width, tb.height);
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
        // blockRenderer: the rendered bounding box with 5px around it.
        view_box: bounds.rect().pad(5.0),
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
        Outline::Ellipse(c, _, _)
        | Outline::Polygon(c, _)
        | Outline::NearestPolygon(c, _)
        | Outline::Cylinder(c, ..) => *c,
    }
}

fn marker(h: Head, at: Point, d: Point, start: bool, color: &Color) -> Vec<Element> {
    // mermaid's block markers (`markers.ts`), userSpaceOnUse with orient
    // auto: a point `(mx, my)` of the marker's viewBox lands at
    // `at + s * ((mx - refX) * d + (my - refY) * n)`, `s` the viewBox scale.
    let n = Point::new(-d.y, d.x);
    let (s, (rx, ry)) = match h {
        Head::None => return Vec::new(),
        // pointEnd / pointStart: viewBox 10, drawn 8.
        Head::Arrow => (0.8, if start { (4.5, 5.0) } else { (5.0, 5.0) }),
        // circleEnd / circleStart: viewBox 10, drawn 11.
        Head::Circle => (1.1, if start { (-1.0, 5.0) } else { (11.0, 5.0) }),
        // crossEnd / crossStart: viewBox 11, drawn 11.
        Head::Cross => (1.0, if start { (-1.0, 5.2) } else { (12.0, 5.2) }),
    };
    let pt = |mx: f64, my: f64| at.plus(d.scale(s * (mx - rx))).plus(n.scale(s * (my - ry)));
    match h {
        Head::None => Vec::new(),
        Head::Arrow => {
            // `M 0 0 L 10 5 L 0 10 z` (start: `M 0 5 L 10 10 L 10 0 z`), its
            // 1px stroke scaled to 0.8.
            let pts = if start {
                [pt(0.0, 5.0), pt(10.0, 10.0), pt(10.0, 0.0)]
            } else {
                [pt(0.0, 0.0), pt(10.0, 5.0), pt(0.0, 10.0)]
            };
            vec![Element::path(
                geom::polygon(&pts),
                Paint::both(color, color, s),
            )]
        }
        // `<circle cx=5 cy=5 r=5>`, its 1px stroke scaled.
        Head::Circle => vec![Element::shape(
            Geometry::Circle {
                c: pt(5.0, 5.0),
                r: 5.0 * s,
            },
            Paint::both(color, color, s),
        )],
        // `M 1,1 l 9,9 M 10,1 l -9,9`, stroke 2.
        Head::Cross => vec![Element::path(
            vec![
                PathCmd::MoveTo(pt(1.0, 1.0)),
                PathCmd::LineTo(pt(10.0, 10.0)),
                PathCmd::MoveTo(pt(10.0, 1.0)),
                PathCmd::LineTo(pt(1.0, 10.0)),
            ],
            Paint::stroke(color, 2.0 * s),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overlapping blocks: a short cylinder sizes the rows, then fills its
    /// two-column-wide cell and reaches past the middle point of its edges.
    const OVERLAP: &str = "block-beta\n  columns 2\n  x{{\"A rather long hexagon label\"}} y[(\"DB\")]\n  z([\"Stadium\"]) w[[\"Sub\"]]\n  x <--> y\n  y --> w\n  z --o x\n  w --x z\n";

    /// Every top-level shape of the edges group.
    fn edge_shapes(src: &str) -> Vec<Element> {
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        let Element::Group { children, .. } = &r.scene.elements[1] else {
            panic!()
        };
        children.clone()
    }

    fn poly(e: &Element) -> Vec<Point> {
        match e {
            Element::Shape {
                geom: Geometry::Path(c),
                ..
            } => c
                .iter()
                .filter_map(|c| match c {
                    PathCmd::MoveTo(p) | PathCmd::LineTo(p) => Some(*p),
                    _ => None,
                })
                .collect(),
            _ => panic!("{e:?}"),
        }
    }

    fn near_all(a: &[Point], b: &[(f64, f64)]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(p, q)| (p.x - q.0).abs() < 1e-6 && (p.y - q.1).abs() < 1e-6)
    }

    /// mermaid 12.3.0 draws `createCylinderPathD` about the shape's corner
    /// (`M0,ry a…`) under `translate(-w / 2, -(h / 2 + ry))`, and the
    /// browser measures it there: the rows are 46.60408401489258 tall
    /// (centres ±27.30204200744629), not the exact 46.6074484.
    #[test]
    fn cylinders_are_drawn_and_measured_about_their_corner() {
        let r = crate::render::render(OVERLAP, &MermaidConfig::default()).unwrap();
        let mut found = Vec::new();
        fn walk(els: &[Element], out: &mut Vec<(f64, f64, Vec<PathCmd>)>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => walk(children, out),
                    Element::Translated { dx, dy, children } => {
                        if let Element::Shape {
                            geom: Geometry::Path(c),
                            ..
                        } = &children[0]
                        {
                            out.push((*dx, *dy, c.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(&r.scene.elements, &mut found);
        assert_eq!(found.len(), 1);
        let (dx, dy, cmds) = &found[0];
        // translate(336.21875, -27.30204200744629) translate(-109.40625, -39.866115251772406)
        assert!((dx - (336.218_75 - 109.406_25)).abs() < 1e-9, "{dx}");
        assert!(
            (dy - (-27.302_042_007_446_29 - 39.866_115_251_772_406)).abs() < 1e-9,
            "{dy}"
        );
        let ry = 15.910_743_501_181_605;
        assert!(matches!(cmds[0], PathCmd::MoveTo(p) if p.x == 0.0 && (p.y - ry).abs() < 1e-12));
        assert!(matches!(
            cmds[1],
            PathCmd::ArcTo { rel: true, to, .. } if to.x == 218.8125 && to.y == 0.0
        ));
    }

    /// Where the overlapping blocks' edges put their markers in mermaid
    /// 12.3.0 (markers centred, turned by the line's first or last segment,
    /// angle 0 when that segment has no length).
    #[test]
    fn markers_sit_where_mermaids_do() {
        let els = edge_shapes(OVERLAP);
        // x <--> y: every point of the line is 222.8125,-27.302…; Chromium
        // still draws both markers there, at angle 0.
        let y0 = -27.302_042_007_446_29;
        let end = poly(&els[1]);
        assert!(
            near_all(
                &end,
                &[(218.8125, y0 - 4.0), (226.8125, y0), (218.8125, y0 + 4.0),]
            ),
            "{end:?}"
        );
        let start = poly(&els[2]);
        assert!(
            near_all(
                &start,
                &[(219.2125, y0), (227.2125, y0 + 4.0), (227.2125, y0 - 4.0),]
            ),
            "{start:?}"
        );
        // y --> w: the line runs from 12.564077 up to the middle point 0,
        // the outline's 4 being behind it; the arrow is centred on 0, at angle 0.
        let PathCmd::MoveTo(a) = (match &els[3] {
            Element::Shape {
                geom: Geometry::Path(c),
                ..
            } => c[0],
            e => panic!("{e:?}"),
        }) else {
            panic!()
        };
        assert!(
            a.dist(Point::new(336.218_75, 12.564_077_377_319_336)) < 1e-6,
            "{a:?}"
        );
        let arrow = poly(&els[4]);
        assert!(
            near_all(
                &arrow,
                &[(332.218_75, -4.0), (340.218_75, 0.0), (332.218_75, 4.0),]
            ),
            "{arrow:?}"
        );
        // z --o x ends at 109.40625,-4 going up: circleEnd (refX 11 of a
        // 10 wide viewBox drawn 11) is centred 6.6 back, radius 5.5.
        let Element::Shape {
            geom: Geometry::Circle { c, r },
            ..
        } = &els[6]
        else {
            panic!("{:?}", els[6])
        };
        assert!(c.dist(Point::new(109.406_25, 2.6)) < 1e-6 && (r - 5.5).abs() < 1e-12);
        // w --x z: crossEnd (refX 12, refY 5.2) 6.5 back and 0.3 aside.
        let cross = poly(&els[8]);
        assert!(
            (cross[0].x - (148.179_018_300_387_36 + 6.5 + 4.5)).abs() < 1e-6,
            "{cross:?}"
        );
        assert!(
            (cross[0].y - (27.302_042_007_446_29 - 0.3 + 4.5)).abs() < 1e-6,
            "{cross:?}"
        );
    }

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
            neo: false,
            hand: None,
        }
    }

    #[test]
    fn node_width_is_the_html_label_box() {
        // mermaid 12.3.0: the label's foreignObject is 135.171875 wide (layout
        // units), the block 32 wider.
        assert_eq!(
            node_size("Gamma three wide", BlockShape::Rect, 1, &m()).0,
            167.171875
        );
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

    fn scene(src: &str) -> Scene {
        let d = parse(src, None).unwrap();
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        draw(&d, &layout(&d, &m()), &m(), &pal)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn neo_shapes_pad_their_labels_like_mermaid() {
        // mermaid 12.3.0 with `look: neo`: the shapes' own label paddings.
        let neo = (8.0, true);
        let tb = label_box("C", 16.0);
        // circle: half the label's diagonal plus 16 (r = 29.320016950533507).
        match node_geom(BlockShape::Circle, &tb, None, 1, neo).prim {
            Prim::Circle(r) => assert!((r - 29.320016950533507).abs() < 1e-6, "{r}"),
            other => panic!("{other:?}"),
        }
        let tb = label_box("Square", 16.0);
        let b = node_geom(BlockShape::Rect, &tb, None, 1, neo).bbox();
        assert!(
            close(b.w, tb.width + 32.0) && close(b.h, tb.height + 24.0),
            "{b:?}"
        );
        // hexagon: the height padded by 70, f = 3.5, the width by 32.
        let tb = label_box("Hex", 16.0);
        let b = node_geom(BlockShape::Hexagon, &tb, None, 1, neo).bbox();
        let h = tb.height + 70.0;
        assert!(
            close(b.h, h) && close(b.w, tb.width + 2.0 * h / 3.5 + 32.0),
            "{b:?}"
        );
        // doublecircle: 16 inside the inner ring, 12 between the rings.
        match node_geom(BlockShape::DoubleCircle, &tb, None, 1, neo).prim {
            Prim::DoubleCircle(inner, outer) => {
                assert!(close(inner, tb.width.hypot(tb.height) / 2.0 + 16.0));
                assert!(close(outer, inner + 12.0));
            }
            other => panic!("{other:?}"),
        }
        // subroutine 28 x 12, rect_left_inv_arrow 42 x 24.
        let b = node_geom(BlockShape::Subroutine, &tb, None, 1, neo).bbox();
        assert!(close(b.w, tb.width + 16.0 + 28.0) && close(b.h, tb.height + 12.0));
        let g = node_geom(BlockShape::Asymmetric, &tb, None, 1, neo);
        match g.prim {
            Prim::Poly(pts) => {
                let ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
                let h = ys.iter().copied().fold(f64::MIN, f64::max)
                    - ys.iter().copied().fold(f64::MAX, f64::min);
                assert!(close(h, tb.height + 24.0), "{h}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_rounded_block_pads_its_label_once_in_either_look() {
        // `roundedRect`, not `squareRect`: p on each side (mermaid 12.3.0:
        // "Rounded node", label 104.984375 wide, block 120.984375).
        let tb = label_box("Rounded node", 16.0);
        for neo in [false, true] {
            let b = node_geom(BlockShape::Rounded, &tb, None, 1, (8.0, neo)).bbox();
            assert!(
                close(b.w, tb.width + 16.0) && close(b.h, tb.height + 16.0),
                "{b:?}"
            );
        }
    }

    #[test]
    fn shapes_size_like_mermaid_rendering_util() {
        // mermaid 12.3.0, every block shape in a 4-column grid: the drawn bounding
        // box (the viewBox less 5px) comes out exactly as in the browser.
        let s = scene(concat!(
            "block-beta\ncolumns 4\n",
            "a[\"Rect\"] b(\"Round\") c([\"Stadium\"]) d[[\"Subroutine\"]]\n",
            "e[(\"Cylinder\")] f((\"Circle\")) g(((\"Dbl\"))) h>\"Odd\"]\n",
            "i{\"Diamond\"} j{{\"Hexagon\"}} k[/\"LeanR\"/] l[\\\"LeanL\"\\]\n",
            "m[/\"Trap\"\\] n[\\\"InvTrap\"/] o<[\"Arrow\"]>(right) p<[\"Up\"]>(up)\n",
            "q<[\"Both ways\"]>(x):2 r[\"Wide label here\"]:2\na --> b\n",
        ));
        let v = s.view_box;
        assert!(close(v.x, -22.34375) && close(v.y, -281.078125), "{v:?}");
        assert!(close(v.w, 513.40625) && close(v.h, 562.15625), "{v:?}");
        // The gallery's block: a double circle (half a cell wide plus 13) outgrows
        // its cell, a hexagon fills it.
        let g = scene(include_str!("../../examples/diagrams/block.mmd")).view_box;
        assert!(close(g.x, -18.0) && close(g.y, -278.890625), "{g:?}");
        assert!(
            close(g.w, 431.140625) && (g.h - 537.4036865234375).abs() < 1e-3,
            "{g:?}"
        );
    }

    #[test]
    fn laid_out_shapes_fill_or_ignore_their_cell() {
        let tb = label_box("API Gateway", 16.0);
        let cell = Some((130.71354166666666, 103.15625));
        let dc = node_geom(BlockShape::DoubleCircle, &tb, cell, 1, (8.0, false));
        assert_eq!(
            dc.prim,
            Prim::DoubleCircle(65.35677083333333 + 8.0, 65.35677083333333 + 13.0)
        );
        // The hexagon fills the cell; its label gets what the ends leave, and as
        // that is off the 1/64px grid the text is cut off rather than wrapped.
        let hx = node_geom(BlockShape::Hexagon, &tb, cell, 1, (8.0, false));
        let b = hx.bbox();
        assert!(close(b.w, 130.71354166666666) && close(b.h, 103.15625));
        assert!(hx.clipped);
        assert_eq!(hx.label_w, 71.125);
        // A stadium keeps its natural size: its caps are a 50-point polygon.
        let st = node_geom(
            BlockShape::Stadium,
            &label_box("Browser", 16.0),
            cell,
            1,
            (8.0, false),
        );
        match st.prim {
            Prim::Stadium { w, h } => {
                assert_eq!(h, 32.0);
                assert!((w - 77.546).abs() < 1e-2, "{w}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn labels_wrap_only_on_the_layout_grid() {
        let tb = label_box("Hexagon wide", 16.0);
        // 49.640625 is a whole number of 1/64px: `display: table` + wrapping.
        let f = fit_label(&tb, 49.640625);
        assert!(!f.clipped);
        assert_eq!(f.lines.len(), 2);
        assert_eq!(f.h, 48.0);
        assert!(f.w >= 49.640625);
        let f = fit_label(&tb, 49.64);
        assert!(f.clipped);
        assert_eq!((f.w, f.h), (49.625, 24.0));
    }

    /// The gallery example (mermaid 12.3.0 reference: `ex_block`).
    const EX_BLOCK: &str = "block-beta\n  columns 3\n  client([\"Browser\"]) space gw{{\"API Gateway\"}}\n  block:services:3\n    columns 3\n    auth[\"Auth service\"] orders[\"Orders\"] search[/\"Search\"/]\n  end\n  queue[[\"Event queue\"]] arrow<[\"publish\"]>(right) db[(\"Database\")]\n  space:2 decide{\"Healthy?\"}\n  ok(((\"OK\"))) space alert>\"Page on-call\"]\n\n  client -- \"HTTPS\" --> gw\n  gw --> auth\n  gw --> orders\n  orders --> queue\n  db -.-> search\n  ok --> alert\n  queue ==> db\n";

    /// Every element of a render, groups flattened.
    fn flat(src: &str) -> (Rect, Vec<Element>) {
        fn walk(els: &[Element], out: &mut Vec<Element>) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Translated { children, .. } => {
                        walk(children, out)
                    }
                    other => out.push(other.clone()),
                }
            }
        }
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        let mut out = Vec::new();
        walk(&r.scene.elements, &mut out);
        (r.scene.view_box, out)
    }

    fn container(els: &[Element]) -> Vec<&Geometry> {
        els.iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom,
                    class: Some(c),
                    ..
                } if c == "label-container" => Some(geom),
                _ => None,
            })
            .collect()
    }

    /// mermaid 12.3.0 draws a subroutine as one polygon (inner frame, then the
    /// outer rectangle), a cylinder as one path of SVG arcs, a stadium as a
    /// polygon of 50-point caps (viewBox 205.8267822 wide, not 205.843).
    #[test]
    fn shapes_are_single_elements_like_mermaids() {
        let (_, els) = flat("block\n    id1[[\"This is the text in the box\"]]\n");
        let g = container(&els);
        assert_eq!(g.len(), 1);
        let Geometry::Path(cmds) = g[0] else {
            panic!("{g:?}")
        };
        assert_eq!(cmds.len(), 11);
        assert_eq!(
            els.iter()
                .filter(|e| matches!(e, Element::Shape { .. }))
                .count(),
            1
        );
        let (_, els) = flat("block\n    id1[(\"Database\")]\n");
        let g = container(&els);
        let Geometry::Path(cmds) = g[0] else {
            panic!("{g:?}")
        };
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(c, PathCmd::ArcTo { .. }))
                .count(),
            3
        );
        assert_eq!(
            els.iter()
                .filter(|e| matches!(e, Element::Shape { .. }))
                .count(),
            1
        );
        let (vb, _) = flat("block\n    id1([\"This is the text in the box\"])\n");
        assert!((vb.w - 205.826_782_2).abs() < 1e-4, "{vb:?}");
    }

    /// A label too wide for its hexagon is cut by its foreignObject, mid-glyph:
    /// the whole text inside a clip of the 71.125px label box.
    #[test]
    fn overflowing_label_is_clipped_not_cut_short() {
        let (_, els) = flat(EX_BLOCK);
        let clip = els
            .iter()
            .find_map(|e| match e {
                Element::Clipped { rect, children } => Some((rect, children)),
                _ => None,
            })
            .unwrap();
        assert_eq!(clip.0.w, 71.125);
        let Element::Text { text, .. } = &clip.1[0] else {
            panic!()
        };
        assert_eq!(text.lines, ["API Gateway"]);
    }

    /// blockRenderer runs an edge through the middle point between the blocks'
    /// centres; a short edge from a cylinder curves back (mermaid 12.3.0:
    /// `M339.643,-98.72L339.722,-97.424C...`, data-points start
    /// (339.6428, -98.7196), the cap's ellipse, not the box's top -98.734).
    #[test]
    fn edges_curve_through_the_centres_midpoint() {
        let src = EX_BLOCK;
        let (_, els) = flat(src);
        let dotted = els
            .iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(c),
                    paint,
                    class: Some(k),
                } if k == "edge" && paint.dash.is_some() => Some(c.clone()),
                _ => None,
            })
            .unwrap();
        let PathCmd::MoveTo(a) = dotted[0] else {
            panic!()
        };
        assert!(
            a.dist(Point::new(339.642_848_9, -98.719_612_1)) < 1e-3,
            "{a:?}"
        );
        assert!(dotted.iter().any(|c| matches!(c, PathCmd::CubicTo(..))));
    }

    #[test]
    fn no_break_spaces_are_space_wide() {
        // mermaid 12.3.0: a down block arrow labelled with three &nbsp; is
        // 13.34 (three space widths) + 48 = 61.34 wide.
        use crate::scene::{Element, Geometry};
        fn walk(els: &[Element], out: &mut Vec<Element>) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Translated { children, .. } => {
                        walk(children, out)
                    }
                    other => out.push(other.clone()),
                }
            }
        }
        let r = crate::render::render(
            "block\ncolumns 1\n  a[\"&nbsp;&nbsp;&nbsp;\"]\n  arrow<[\"&nbsp;&nbsp;&nbsp;\"]>(down)\n",
            &MermaidConfig::default(),
        )
        .unwrap();
        let mut els = Vec::new();
        walk(&r.scene.elements, &mut els);
        let widths: Vec<f64> = els
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(c),
                    class: Some(k),
                    ..
                } if k == "label-container" => crate::geom::path_bbox(c).map(|b| b.w),
                _ => None,
            })
            .collect();
        assert!(
            widths.iter().any(|w| (w - 61.34375).abs() < 1e-3),
            "{widths:?}"
        );
        assert_eq!(
            crate::text::line_width("\u{a0}", 16.0, false),
            crate::text::line_width(" ", 16.0, false)
        );
    }

    /// The classic themes define no `nodeTextColor`: a block's label takes
    /// `textColor` (`.label text, span`), black under neutral (mermaid 12.3.0).
    #[test]
    fn neutral_labels_are_the_text_colour() {
        let src = "---\nconfig:\n  theme: neutral\n---\nblock\n  A[\"One\"] B[\"Two\"]\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .unwrap()
            .scene;
        fn walk(els: &[Element], out: &mut Vec<String>) {
            for el in els {
                match el {
                    Element::Group { children, .. } => walk(children, out),
                    Element::Text { text, .. } => out.push(text.fill.css().to_string()),
                    _ => {}
                }
            }
        }
        let mut fills = Vec::new();
        walk(&scene.elements, &mut fills);
        assert_eq!(fills, ["#000000", "#000000"]);
    }

    /// mermaid 12.3.0 under `look: handDrawn` (seed 19): blocks are sized by
    /// their rough drawings, then drawn into their cells by rough.js again
    /// (Alpha's fill sketch from (0.265, -102.482), Beta's circle from
    /// (136.368, -66.521)).
    #[test]
    fn hand_drawn_blocks_match_mermaid() {
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 19\n---\nblock\n  \
                   columns 3\n  a[\"Alpha\"] b((\"Beta\")) c{\"Gamma\"}\n  d([\"Delta\"]) \
                   e[\"Epsilon\"] f(\"Zeta\")\n  a --> b\n  b --> d\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .unwrap()
            .scene;
        fn walk(els: &[Element], out: &mut Vec<String>) {
            for el in els {
                match el {
                    Element::Group { children, .. } => walk(children, out),
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
        for start in ["M0.265,-102.482C", "M136.368,-66.521C"] {
            assert!(paths.iter().any(|d| d.starts_with(start)), "{start}");
        }
        // Seed 12: a cylinder is cylinder.ts's rough outline (moved to its
        // corner) and unfilled front half; the database's outline starts at
        // (2.723, -67.767).
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 12\n---\nblock\n  \
                   columns 3\n  db[(\"Database\")] a[\"App\"] c[(\"Cache\")]\n  \
                   wide[(\"A wide cylinder\")]:2 x[\"X\"]\n  a --> db\n  a --> c\n  x --> wide\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .unwrap()
            .scene;
        let mut paths = Vec::new();
        walk(&scene.elements, &mut paths);
        assert!(
            paths.iter().any(|d| d.starts_with("M2.723,-67.767C")),
            "{paths:?}"
        );
    }
}
