//! C4 diagrams (`C4Context`, `C4Container`, `C4Component`, `C4Dynamic`, `C4Deployment`).
//!
//! Parsing follows mermaid's `c4Diagram.jison` + `c4Db.js`: macro calls with
//! positional or `$named="..."` arguments, boundaries nested with `{ }`,
//! relationships and the `Update*` style/layout statements.
//!
//! Layout mirrors mermaid's row placement (`c4ShapeInRow` shapes per row, then
//! `c4BoundaryInRow` child boundaries per row) but is computed with Taffy: each
//! boundary is a padded flex column with a title band whose children are rows.
//! Relationships are straight lines clipped to element borders with geometric
//! arrowheads and midpoint labels nudged along the line to avoid overlaps.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::boxes::{self, BoxNode, Placed};
use crate::scene::{Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text::{self, TextBlock};
use crate::theme::Palette;
use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// IR
// ---------------------------------------------------------------------------

/// Which C4 diagram keyword introduced the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Context,
    Container,
    Component,
    Dynamic,
    Deployment,
}

/// The C4 abstraction an element card represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Person,
    System,
    Container,
    Component,
}

/// Outline of an element card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Box,
    Db,
    Queue,
}

/// Colour overrides from `UpdateElementStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub bg: Option<String>,
    pub font: Option<String>,
    pub border: Option<String>,
}

/// An element card (person, system, container, component).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub alias: String,
    pub label: String,
    pub kind: ShapeKind,
    pub form: Form,
    pub external: bool,
    pub techn: Option<String>,
    pub descr: Option<String>,
    pub tags: Option<String>,
    pub link: Option<String>,
    pub style: Style,
}

impl Shape {
    /// mermaid's `typeC4Shape` text, e.g. `external_system_db`.
    pub fn type_name(&self) -> String {
        let base = match self.kind {
            ShapeKind::Person => "person",
            ShapeKind::System => "system",
            ShapeKind::Container => "container",
            ShapeKind::Component => "component",
        };
        let form = match self.form {
            Form::Box => "",
            Form::Db => "_db",
            Form::Queue => "_queue",
        };
        let ext = if self.external { "external_" } else { "" };
        format!("{ext}{base}{form}")
    }
}

/// Kind of boundary box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryKind {
    Boundary,
    Enterprise,
    System,
    Container,
    /// `Deployment_Node` / `Node` / `Node_L` / `Node_R` (drawn solid).
    Node,
}

/// A child of the diagram root or of a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Shape(usize),
    Boundary(usize),
}

/// A boundary (or deployment node) grouping nested items.
#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    pub alias: String,
    pub label: String,
    pub kind: BoundaryKind,
    /// Text shown as `[type]` under the label.
    pub type_text: String,
    pub descr: Option<String>,
    pub children: Vec<Item>,
    pub style: Style,
}

/// Relationship flavours. Directional variants are accepted for compatibility;
/// like mermaid, they do not change placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelKind {
    Rel,
    BiRel,
    Up,
    Down,
    Left,
    Right,
    Back,
}

/// A relationship between two elements or boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct Rel {
    pub from: String,
    pub to: String,
    pub label: String,
    pub techn: Option<String>,
    pub descr: Option<String>,
    pub kind: RelKind,
    pub text_color: Option<String>,
    pub line_color: Option<String>,
    pub offset: (f64, f64),
    pub line: usize,
}

/// A parsed C4 diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct C4Diagram {
    pub variant: Variant,
    pub title: Option<String>,
    pub shapes: Vec<Shape>,
    pub boundaries: Vec<Boundary>,
    /// Items outside any boundary, in source order.
    pub root: Vec<Item>,
    pub rels: Vec<Rel>,
    pub shape_in_row: usize,
    pub boundary_in_row: usize,
}

impl C4Diagram {
    fn shape_index(&self, alias: &str) -> Option<usize> {
        self.shapes.iter().position(|s| s.alias == alias)
    }
    fn boundary_index(&self, alias: &str) -> Option<usize> {
        self.boundaries.iter().position(|b| b.alias == alias)
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// One macro argument: positional (`"text"`, `bare`, empty) or `$key="value"`.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Pos(String),
    Named(String, String),
}

/// A macro's argument list. A named argument occupies its slot, as in mermaid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args(pub Vec<Arg>);

impl Args {
    /// Non-empty positional value at `slot`.
    pub fn pos(&self, slot: usize) -> Option<&str> {
        match self.0.get(slot) {
            Some(Arg::Pos(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }
    /// Value of `$key=...` anywhere in the list.
    pub fn named(&self, key: &str) -> Option<&str> {
        self.0.iter().find_map(|a| match a {
            Arg::Named(k, v) if k == key && !v.is_empty() => Some(v.as_str()),
            _ => None,
        })
    }
    /// Named value, else positional value at `slot`.
    pub fn get(&self, slot: usize, key: &str) -> Option<String> {
        self.named(key)
            .or_else(|| self.pos(slot))
            .map(str::to_string)
    }
    fn drop_first(mut self) -> Self {
        if !self.0.is_empty() {
            self.0.remove(0);
        }
        self
    }
}

/// Parse the text between the parentheses of a macro call. Commas inside
/// double quotes do not split.
pub fn parse_args(inner: &str, line: usize) -> Result<Args> {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in inner.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ',' if !in_quote => pieces.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if in_quote {
        return Err(Error::parse(line, "Unterminated string in argument list"));
    }
    pieces.push(cur);
    if pieces.len() == 1 && pieces[0].trim().is_empty() {
        return Ok(Args::default());
    }
    let unquote = |s: &str| -> String {
        let s = s.trim();
        if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
            s[1..s.len() - 1].to_string()
        } else {
            s.to_string()
        }
    };
    let mut out = Vec::new();
    for p in pieces {
        let t = p.trim();
        if let Some(rest) = t.strip_prefix('$') {
            let Some(eq) = rest.find('=') else {
                return Err(Error::parse(
                    line,
                    format!("Expected '=' after named argument '${}'", rest.trim()),
                ));
            };
            let key = rest[..eq].trim().to_string();
            if key.is_empty() {
                return Err(Error::parse(line, "Empty named argument key"));
            }
            out.push(Arg::Named(key, unquote(&rest[eq + 1..])));
        } else {
            out.push(Arg::Pos(unquote(t)));
        }
    }
    Ok(Args(out))
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `Name(args)` or a bare word (`C4Context`, `direction`).
    Call {
        name: String,
        args: Option<String>,
        line: usize,
    },
    /// `title ...` / `direction ...` rest-of-line statements.
    LineStmt {
        word: String,
        rest: String,
        line: usize,
    },
    Open(usize),
    Close(usize),
}

fn tokenize(body: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = body.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() || c == ';' => i += 1,
            '{' => {
                toks.push(Tok::Open(line));
                i += 1;
            }
            '}' => {
                toks.push(Tok::Close(line));
                i += 1;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                if matches!(name.as_str(), "title" | "direction") {
                    let s = i;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    let rest: String = chars[s..i].iter().collect();
                    toks.push(Tok::LineStmt {
                        word: name,
                        rest: rest.trim().to_string(),
                        line,
                    });
                    continue;
                }
                let mut j = i;
                while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '(' {
                    let call_line = line;
                    let mut k = j + 1;
                    let mut in_quote = false;
                    let mut found = None;
                    while k < chars.len() {
                        match chars[k] {
                            '"' => in_quote = !in_quote,
                            ')' if !in_quote => {
                                found = Some(k);
                                break;
                            }
                            '\n' => line += 1,
                            _ => {}
                        }
                        k += 1;
                    }
                    let Some(end) = found else {
                        return Err(Error::parse(
                            call_line,
                            format!("Missing ')' to close '{name}('"),
                        ));
                    };
                    let inner: String = chars[j + 1..end].iter().collect();
                    toks.push(Tok::Call {
                        name,
                        args: Some(inner),
                        line: call_line,
                    });
                    i = end + 1;
                } else {
                    toks.push(Tok::Call {
                        name,
                        args: None,
                        line,
                    });
                }
            }
            other => {
                return Err(Error::parse(
                    line,
                    format!("Unexpected character '{other}'"),
                ));
            }
        }
    }
    Ok(toks)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn shape_macro(name: &str) -> Option<(ShapeKind, Form, bool)> {
    use Form::*;
    use ShapeKind::*;
    Some(match name {
        "Person" => (Person, Box, false),
        "Person_Ext" => (Person, Box, true),
        "System" => (System, Box, false),
        "SystemDb" => (System, Db, false),
        "SystemQueue" => (System, Queue, false),
        "System_Ext" => (System, Box, true),
        "SystemDb_Ext" => (System, Db, true),
        "SystemQueue_Ext" => (System, Queue, true),
        "Container" => (Container, Box, false),
        "ContainerDb" => (Container, Db, false),
        "ContainerQueue" => (Container, Queue, false),
        "Container_Ext" => (Container, Box, true),
        "ContainerDb_Ext" => (Container, Db, true),
        "ContainerQueue_Ext" => (Container, Queue, true),
        "Component" => (Component, Box, false),
        "ComponentDb" => (Component, Db, false),
        "ComponentQueue" => (Component, Queue, false),
        "Component_Ext" => (Component, Box, true),
        "ComponentDb_Ext" => (Component, Db, true),
        "ComponentQueue_Ext" => (Component, Queue, true),
        _ => return None,
    })
}

fn rel_macro(name: &str) -> Option<RelKind> {
    Some(match name {
        "Rel" | "RelIndex" => RelKind::Rel,
        "BiRel" => RelKind::BiRel,
        "Rel_U" | "Rel_Up" => RelKind::Up,
        "Rel_D" | "Rel_Down" => RelKind::Down,
        "Rel_L" | "Rel_Left" => RelKind::Left,
        "Rel_R" | "Rel_Right" => RelKind::Right,
        "Rel_Back" => RelKind::Back,
        _ => return None,
    })
}

#[derive(Default)]
struct Parser {
    shapes: Vec<Shape>,
    boundaries: Vec<Boundary>,
    root: Vec<Item>,
    rels: Vec<Rel>,
    /// Open boundaries (indexes) with the line that opened them.
    stack: Vec<(usize, usize)>,
}

impl Parser {
    fn push_item(&mut self, item: Item) {
        match self.stack.last() {
            Some(&(b, _)) => self.boundaries[b].children.push(item),
            None => self.root.push(item),
        }
    }
}

/// Parse a C4 diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<C4Diagram> {
    let toks = tokenize(body)?;
    let mut it = toks.into_iter().peekable();
    let variant = match it.next() {
        Some(Tok::Call {
            name, args: None, ..
        }) => match name.as_str() {
            "C4Context" => Variant::Context,
            "C4Container" => Variant::Container,
            "C4Component" => Variant::Component,
            "C4Dynamic" => Variant::Dynamic,
            "C4Deployment" => Variant::Deployment,
            _ => {
                return Err(Error::parse(
                    1,
                    format!("Expected a C4 keyword, got '{name}'"),
                ));
            }
        },
        _ => return Err(Error::parse(1, "Expected a C4 diagram keyword")),
    };
    let mut d = C4Diagram {
        variant,
        title,
        shapes: Vec::new(),
        boundaries: Vec::new(),
        root: Vec::new(),
        rels: Vec::new(),
        shape_in_row: 4,
        boundary_in_row: 2,
    };
    let mut p = Parser::default();
    while let Some(tok) = it.next() {
        match tok {
            Tok::LineStmt { word, rest, .. } => {
                if word == "title" {
                    d.title = Some(rest);
                }
            }
            Tok::Open(line) => return Err(Error::parse(line, "Unexpected '{'")),
            Tok::Close(line) => {
                if p.stack.pop().is_none() {
                    return Err(Error::parse(line, "Unmatched '}'"));
                }
            }
            Tok::Call {
                name,
                args: None,
                line,
            } => {
                return Err(Error::parse(line, format!("Expected '(' after '{name}'")));
            }
            Tok::Call {
                name,
                args: Some(raw),
                line,
            } => {
                let args = parse_args(&raw, line)?;
                let need_alias = |a: &Args| -> Result<String> {
                    a.get(0, "alias")
                        .ok_or_else(|| Error::parse(line, format!("'{name}' needs an alias")))
                };
                if let Some((kind, form, external)) = shape_macro(&name) {
                    let alias = need_alias(&args)?;
                    let has_techn = matches!(kind, ShapeKind::Container | ShapeKind::Component);
                    let (techn, descr_slot) = if has_techn {
                        (args.get(2, "techn"), 3)
                    } else {
                        (args.named("techn").map(str::to_string), 2)
                    };
                    let shape = Shape {
                        label: args.get(1, "label").unwrap_or_default(),
                        kind,
                        form,
                        external,
                        techn,
                        descr: args.get(descr_slot, "descr"),
                        tags: args.get(descr_slot + 2, "tags"),
                        link: args.get(descr_slot + 3, "link"),
                        style: Style::default(),
                        alias,
                    };
                    if let Some(i) = p.shapes.iter().position(|s| s.alias == shape.alias) {
                        p.shapes[i] = shape;
                    } else {
                        p.shapes.push(shape);
                        p.push_item(Item::Shape(p.shapes.len() - 1));
                    }
                } else if let Some(kind) = match name.as_str() {
                    "Boundary" => Some(BoundaryKind::Boundary),
                    "Enterprise_Boundary" => Some(BoundaryKind::Enterprise),
                    "System_Boundary" => Some(BoundaryKind::System),
                    "Container_Boundary" => Some(BoundaryKind::Container),
                    "Deployment_Node" | "Node" | "Node_L" | "Node_R" => Some(BoundaryKind::Node),
                    _ => None,
                } {
                    let alias = need_alias(&args)?;
                    let fixed = match kind {
                        BoundaryKind::Enterprise => Some("ENTERPRISE"),
                        BoundaryKind::System => Some("SYSTEM"),
                        BoundaryKind::Container => Some("CONTAINER"),
                        _ => None,
                    };
                    let type_text = match fixed {
                        Some(t) => args.named("type").unwrap_or(t).to_string(),
                        None => args.get(2, "type").unwrap_or_else(|| {
                            if kind == BoundaryKind::Node {
                                "node".into()
                            } else {
                                "system".into()
                            }
                        }),
                    };
                    let descr = if kind == BoundaryKind::Node {
                        args.get(3, "descr")
                    } else {
                        args.named("descr").map(str::to_string)
                    };
                    let b = Boundary {
                        alias,
                        label: args.get(1, "label").unwrap_or_default(),
                        kind,
                        type_text,
                        descr,
                        children: Vec::new(),
                        style: Style::default(),
                    };
                    let idx = p.boundaries.len();
                    p.boundaries.push(b);
                    p.push_item(Item::Boundary(idx));
                    match it.next() {
                        Some(Tok::Open(_)) => p.stack.push((idx, line)),
                        _ => {
                            return Err(Error::parse(
                                line,
                                format!("Expected '{{' after '{name}(...)'"),
                            ));
                        }
                    }
                } else if let Some(kind) = rel_macro(&name) {
                    let args = if name == "RelIndex" {
                        args.drop_first()
                    } else {
                        args
                    };
                    let from = args.get(0, "from");
                    let to = args.get(1, "to");
                    let (Some(from), Some(to)) = (from, to) else {
                        return Err(Error::parse(
                            line,
                            format!("'{name}' needs a source and a target"),
                        ));
                    };
                    let rel = Rel {
                        label: args.get(2, "label").unwrap_or_default(),
                        techn: args.get(3, "techn"),
                        descr: args.get(4, "descr"),
                        kind,
                        text_color: None,
                        line_color: None,
                        offset: (0.0, 0.0),
                        line,
                        from,
                        to,
                    };
                    if let Some(old) = p
                        .rels
                        .iter_mut()
                        .find(|r| r.from == rel.from && r.to == rel.to)
                    {
                        *old = rel;
                    } else {
                        p.rels.push(rel);
                    }
                } else {
                    match name.as_str() {
                        "UpdateElementStyle" => {
                            let Some(target) = args.get(0, "elementName") else {
                                return Err(Error::parse(
                                    line,
                                    "UpdateElementStyle needs an element name",
                                ));
                            };
                            let style = Style {
                                bg: args.get(1, "bgColor"),
                                font: args.get(2, "fontColor"),
                                border: args.get(3, "borderColor"),
                            };
                            let merge = |s: &mut Style| {
                                if style.bg.is_some() {
                                    s.bg.clone_from(&style.bg);
                                }
                                if style.font.is_some() {
                                    s.font.clone_from(&style.font);
                                }
                                if style.border.is_some() {
                                    s.border.clone_from(&style.border);
                                }
                            };
                            if let Some(s) = p.shapes.iter_mut().find(|s| s.alias == target) {
                                merge(&mut s.style);
                            } else if let Some(b) =
                                p.boundaries.iter_mut().find(|b| b.alias == target)
                            {
                                merge(&mut b.style);
                            }
                        }
                        "UpdateRelStyle" => {
                            let (Some(from), Some(to)) = (args.get(0, "from"), args.get(1, "to"))
                            else {
                                return Err(Error::parse(
                                    line,
                                    "UpdateRelStyle needs a source and a target",
                                ));
                            };
                            let num = |v: Option<String>| -> Result<Option<f64>> {
                                v.map(|s| {
                                    s.trim().parse::<f64>().map_err(|_| {
                                        Error::parse(line, format!("Expected a number, got '{s}'"))
                                    })
                                })
                                .transpose()
                            };
                            let dx = num(args.get(4, "offsetX"))?;
                            let dy = num(args.get(5, "offsetY"))?;
                            if let Some(r) =
                                p.rels.iter_mut().find(|r| r.from == from && r.to == to)
                            {
                                if let Some(c) = args.get(2, "textColor") {
                                    r.text_color = Some(c);
                                }
                                if let Some(c) = args.get(3, "lineColor") {
                                    r.line_color = Some(c);
                                }
                                if let Some(x) = dx {
                                    r.offset.0 = x;
                                }
                                if let Some(y) = dy {
                                    r.offset.1 = y;
                                }
                            }
                        }
                        "UpdateLayoutConfig" => {
                            let count = |v: Option<String>| -> Result<Option<usize>> {
                                v.map(|s| {
                                    s.trim()
                                        .parse::<usize>()
                                        .ok()
                                        .filter(|n| *n > 0)
                                        .ok_or_else(|| {
                                            Error::parse(
                                                line,
                                                format!("Expected a positive integer, got '{s}'"),
                                            )
                                        })
                                })
                                .transpose()
                            };
                            if let Some(n) = count(args.get(0, "c4ShapeInRow"))? {
                                d.shape_in_row = n;
                            }
                            if let Some(n) = count(args.get(1, "c4BoundaryInRow"))? {
                                d.boundary_in_row = n;
                            }
                        }
                        // Accepted and ignored, like mermaid's legend/tag helpers.
                        "AddElementTag"
                        | "AddRelTag"
                        | "UpdateBoundaryStyle"
                        | "SHOW_LEGEND"
                        | "LAYOUT_TOP_DOWN"
                        | "LAYOUT_LEFT_RIGHT"
                        | "LAYOUT_WITH_LEGEND" => {}
                        _ => {
                            return Err(Error::parse(
                                line,
                                format!("Unknown C4 statement '{name}'"),
                            ));
                        }
                    }
                }
            }
        }
    }
    if let Some(&(b, line)) = p.stack.last() {
        return Err(Error::parse(
            line,
            format!(
                "Boundary '{}' is missing its closing '}}'",
                p.boundaries[b].alias
            ),
        ));
    }
    d.shapes = p.shapes;
    d.boundaries = p.boundaries;
    d.root = p.root;
    d.rels = p.rels;
    for r in &d.rels {
        for end in [&r.from, &r.to] {
            if d.shape_index(end).is_none() && d.boundary_index(end).is_none() {
                return Err(Error::parse(
                    r.line,
                    format!("Relationship refers to unknown element '{end}'"),
                ));
            }
        }
    }
    Ok(d)
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

const SHAPE_W: f64 = 216.0;
const SHAPE_MIN_H: f64 = 60.0;
const SHAPE_PAD: f64 = 20.0;
const TYPE_FONT: f64 = 12.0;
const LABEL_FONT: f64 = 16.0;
const BODY_FONT: f64 = 14.0;
const REL_FONT: f64 = 12.0;
const DB_RY: f64 = 8.0;
const QUEUE_RX: f64 = 8.0;
const BOUNDARY_PAD: f64 = 20.0;
const ROW_GAP_X: f64 = 80.0;
const ROW_GAP_Y: f64 = 70.0;
const BOUNDARY_GAP: f64 = 40.0;

/// Greedy word wrap to `max` px (a single over-long word stays on its line).
fn wrap(textv: &str, font: f64, bold: bool, max: f64) -> Vec<String> {
    let mut out = Vec::new();
    for para in text::split_lines(&text::normalize_label(textv)) {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() {
                word.to_string()
            } else {
                format!("{cur} {word}")
            };
            if !cur.is_empty() && text::line_width(&cand, font, bold) > max {
                out.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = cand;
            }
        }
        out.push(cur);
    }
    out
}

/// A run of centred text lines inside a card, relative to the card's top.
#[derive(Debug, Clone, PartialEq)]
struct TextRun {
    top: f64,
    lines: Vec<String>,
    font: f64,
    line_height: f64,
    bold: bool,
}

impl TextRun {
    fn height(&self) -> f64 {
        self.line_height * self.lines.len() as f64
    }
    fn width(&self) -> f64 {
        self.lines
            .iter()
            .map(|l| text::line_width(l, self.font, self.bold))
            .fold(0.0, f64::max)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CardLayout {
    w: f64,
    h: f64,
    runs: Vec<TextRun>,
    /// Top of the 48px person glyph, relative to the card top.
    glyph: Option<f64>,
}

fn measure_shape(s: &Shape) -> CardLayout {
    let limit = SHAPE_W - 2.0 * SHAPE_PAD;
    let mut y = 6.0 + if s.form == Form::Db { 2.0 * DB_RY } else { 0.0 };
    let mut runs = Vec::new();
    let mut push = |y: &mut f64, lines: Vec<String>, font: f64, lh: f64, bold: bool, gap: f64| {
        *y += gap;
        let r = TextRun {
            top: *y,
            lines,
            font,
            line_height: lh,
            bold,
        };
        *y += r.height();
        runs.push(r);
    };
    push(
        &mut y,
        vec![format!("\u{ab}{}\u{bb}", s.type_name())],
        TYPE_FONT,
        15.0,
        false,
        0.0,
    );
    let mut glyph = None;
    if s.kind == ShapeKind::Person {
        glyph = Some(y + 2.0);
        y += 50.0;
    }
    push(
        &mut y,
        wrap(&s.label, LABEL_FONT, true, limit),
        LABEL_FONT,
        20.0,
        true,
        4.0,
    );
    if let Some(t) = s.techn.as_ref().filter(|t| !t.is_empty()) {
        push(
            &mut y,
            wrap(&format!("[{t}]"), BODY_FONT, false, limit),
            BODY_FONT,
            17.0,
            false,
            2.0,
        );
    }
    if let Some(dsc) = s.descr.as_ref().filter(|t| !t.is_empty()) {
        push(
            &mut y,
            wrap(dsc, BODY_FONT, false, limit),
            BODY_FONT,
            17.0,
            false,
            10.0,
        );
    }
    y += 12.0 + if s.form == Form::Db { DB_RY } else { 0.0 };
    let text_w = runs.iter().map(TextRun::width).fold(0.0, f64::max);
    let extra = if s.form == Form::Queue {
        2.0 * QUEUE_RX
    } else {
        0.0
    };
    CardLayout {
        w: SHAPE_W.max(text_w + 2.0 * SHAPE_PAD / 2.0 + extra),
        h: SHAPE_MIN_H.max(y),
        runs,
        glyph,
    }
}

fn measure_boundary_title(b: &Boundary, max: f64) -> (Vec<TextRun>, f64) {
    let mut runs = Vec::new();
    let mut y = 8.0;
    let label = TextRun {
        top: y,
        lines: wrap(&b.label, LABEL_FONT, true, max),
        font: LABEL_FONT,
        line_height: 20.0,
        bold: true,
    };
    y += label.height();
    runs.push(label);
    if !b.type_text.is_empty() {
        let r = TextRun {
            top: y + 2.0,
            lines: vec![format!("[{}]", b.type_text)],
            font: BODY_FONT,
            line_height: 17.0,
            bold: false,
        };
        y = r.top + r.height();
        runs.push(r);
    }
    if let Some(dsc) = b.descr.as_ref().filter(|t| !t.is_empty()) {
        let r = TextRun {
            top: y + 2.0,
            lines: wrap(dsc, 12.0, false, max),
            font: 12.0,
            line_height: 15.0,
            bold: false,
        };
        y = r.top + r.height();
        runs.push(r);
    }
    (runs, y)
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Absolute geometry of a laid-out diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct C4Layout {
    pub shapes: Vec<Rect>,
    pub boundaries: Vec<Rect>,
    /// Height of the title band at the top (0 without a title).
    pub title_band: f64,
}

/// Shapes already counted into the current row when a boundary's shapes
/// start, per boundary.
///
/// mermaid's `drawInsideBoundary` places sibling boundaries with one shared
/// `Bounds` whose row counter (`nextData.cnt`) is never reset, so a boundary's
/// first shape row is shortened by the shapes of the siblings drawn before it
/// (e.g. with `c4ShapeInRow=3`, two shapes in one boundary leave room for one
/// in the next sibling's first row). Nested boundaries get a fresh counter.
fn row_carry(d: &C4Diagram) -> Vec<usize> {
    fn visit(d: &C4Diagram, items: &[Item], carry: &mut Vec<usize>) {
        let n = d.shape_in_row.max(1);
        let mut cnt = 0usize;
        for it in items {
            if let Item::Boundary(b) = *it {
                carry[b] = cnt;
                for c in &d.boundaries[b].children {
                    if matches!(c, Item::Shape(_)) {
                        cnt += 1;
                        if cnt > n {
                            cnt = 1;
                        }
                    }
                }
                visit(d, &d.boundaries[b].children, carry);
            }
        }
    }
    let mut carry = vec![0; d.boundaries.len()];
    visit(d, &d.root, &mut carry);
    carry
}

/// Rows of a container: shapes in rows of `shape_in_row` (the first row
/// shortened by `carry`, see [`row_carry`]), then child boundaries in rows of
/// `boundary_in_row` (mermaid's order).
fn rows_of(d: &C4Diagram, items: &[Item], carry: usize) -> Vec<Vec<Item>> {
    let n = d.shape_in_row.max(1);
    let shapes: Vec<Item> = items
        .iter()
        .copied()
        .filter(|i| matches!(i, Item::Shape(_)))
        .collect();
    let bounds: Vec<Item> = items
        .iter()
        .copied()
        .filter(|i| matches!(i, Item::Boundary(_)))
        .collect();
    let first = if carry >= n { n } else { n - carry };
    let mut rows: Vec<Vec<Item>> = Vec::new();
    let mut rest: &[Item] = &shapes;
    if !rest.is_empty() {
        let k = first.min(rest.len());
        rows.push(rest[..k].to_vec());
        rest = &rest[k..];
    }
    rows.extend(rest.chunks(n).map(<[Item]>::to_vec));
    rows.extend(
        bounds
            .chunks(d.boundary_in_row.max(1))
            .map(<[Item]>::to_vec),
    );
    rows
}

fn container_node(
    d: &C4Diagram,
    items: &[Item],
    cards: &[CardLayout],
    style: boxes::taffy::Style,
    carry_in: usize,
    carry: &[usize],
) -> BoxNode {
    let rows = rows_of(d, items, carry_in)
        .into_iter()
        .map(|row| {
            let gap = if matches!(row.first(), Some(Item::Boundary(_))) {
                BOUNDARY_GAP
            } else {
                ROW_GAP_X
            };
            BoxNode::new(
                boxes::row(gap),
                row.iter()
                    .map(|it| item_node(d, *it, cards, carry))
                    .collect(),
            )
        })
        .collect();
    BoxNode::new(style, rows)
}

fn item_node(d: &C4Diagram, item: Item, cards: &[CardLayout], carry: &[usize]) -> BoxNode {
    match item {
        Item::Shape(i) => BoxNode::leaf(cards[i].w, cards[i].h),
        Item::Boundary(b) => {
            let bd = &d.boundaries[b];
            let (runs, title_h) = measure_boundary_title(bd, 2.0 * SHAPE_W);
            let title_w = runs.iter().map(TextRun::width).fold(0.0, f64::max);
            let style = boxes::min_size(
                boxes::padded_with_title(boxes::column(ROW_GAP_Y), BOUNDARY_PAD, title_h),
                title_w + 2.0 * BOUNDARY_PAD,
                title_h + 2.0 * BOUNDARY_PAD,
            );
            container_node(d, &bd.children, cards, style, carry[b], carry)
        }
    }
}

fn read_container(
    d: &C4Diagram,
    items: &[Item],
    carry_in: usize,
    carry: &[usize],
    placed: &Placed,
    shapes: &mut [Rect],
    bounds: &mut [Rect],
) {
    for (row, prow) in rows_of(d, items, carry_in).iter().zip(&placed.children) {
        for (item, p) in row.iter().zip(&prow.children) {
            match *item {
                Item::Shape(i) => shapes[i] = p.rect,
                Item::Boundary(b) => {
                    bounds[b] = p.rect;
                    let children = &d.boundaries[b].children;
                    read_container(d, children, carry[b], carry, p, shapes, bounds);
                }
            }
        }
    }
}

fn title_band(d: &C4Diagram) -> f64 {
    if d.title.as_ref().is_some_and(|t| !t.trim().is_empty()) {
        40.0
    } else {
        0.0
    }
}

/// Lay out cards and boundaries with Taffy.
pub fn layout(d: &C4Diagram) -> C4Layout {
    let cards: Vec<CardLayout> = d.shapes.iter().map(measure_shape).collect();
    let band = title_band(d);
    let carry = row_carry(d);
    let root = container_node(d, &d.root, &cards, boxes::column(ROW_GAP_Y), 0, &carry);
    let placed = boxes::compute(&root);
    let mut shapes = vec![Rect::default(); d.shapes.len()];
    let mut bounds = vec![Rect::default(); d.boundaries.len()];
    read_container(d, &d.root, 0, &carry, &placed, &mut shapes, &mut bounds);
    let shift = |r: &mut Rect| r.y += band;
    shapes.iter_mut().for_each(shift);
    bounds.iter_mut().for_each(shift);
    C4Layout {
        shapes,
        boundaries: bounds,
        title_band: band,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn colors(s: &Shape) -> (&'static str, &'static str) {
    match (s.kind, s.external) {
        (ShapeKind::Person, false) => ("#08427B", "#073B6F"),
        (ShapeKind::Person, true) => ("#686868", "#8A8A8A"),
        (ShapeKind::System, false) => ("#1168BD", "#3C7FC0"),
        (ShapeKind::System, true) => ("#999999", "#8A8A8A"),
        (ShapeKind::Container, false) => ("#438DD5", "#3C7FC0"),
        (ShapeKind::Container, true) => ("#B3B3B3", "#A6A6A6"),
        (ShapeKind::Component, false) => ("#85BBF0", "#78A8D8"),
        (ShapeKind::Component, true) => ("#CCCCCC", "#BFBFBF"),
    }
}

fn draw_runs(out: &mut Vec<Element>, runs: &[TextRun], cx: f64, top: f64, fill: &Color) {
    for r in runs {
        let c = Point::new(cx, top + r.top + r.height() / 2.0);
        let mut t = Text::centered(c, &r.lines, r.font, r.line_height, fill);
        t.bold = r.bold;
        out.push(Element::text(t));
    }
}

fn person_glyph(cx: f64, top: f64, fill: &Color) -> Vec<Element> {
    let head = Element::path(
        geom::ellipse(Point::new(cx, top + 11.0), 10.0, 10.0),
        Paint::fill(fill),
    );
    let sy = top + 36.0;
    let mut body = vec![PathCmd::MoveTo(Point::new(cx - 19.0, top + 46.0))];
    body.push(PathCmd::LineTo(Point::new(cx - 19.0, sy)));
    body.extend(geom::elliptic_arc_cubics(
        Point::new(cx, sy),
        19.0,
        13.0,
        PI,
        2.0 * PI,
    ));
    body.push(PathCmd::LineTo(Point::new(cx + 19.0, top + 46.0)));
    body.push(PathCmd::Close);
    vec![head, Element::path(body, Paint::fill(fill))]
}

fn draw_shape(s: &Shape, card: &CardLayout, r: Rect) -> Element {
    let (bg, border) = colors(s);
    let fill = Color::new(s.style.bg.clone().unwrap_or_else(|| bg.into()));
    let stroke = Color::new(s.style.border.clone().unwrap_or_else(|| border.into()));
    let font = Color::new(s.style.font.clone().unwrap_or_else(|| "#FFFFFF".into()));
    let paint = Paint::both(&fill, &stroke, 0.5);
    let mut els = Vec::new();
    match s.form {
        Form::Box => els.push(Element::rect(r, 2.5, paint)),
        Form::Db => {
            let cx = r.center().x;
            let rx = r.w / 2.0;
            let top = Point::new(cx, r.y + DB_RY);
            let bot = Point::new(cx, r.bottom() - DB_RY);
            let mut body = vec![PathCmd::MoveTo(Point::new(r.x, top.y))];
            body.extend(geom::elliptic_arc_cubics(top, rx, DB_RY, PI, 2.0 * PI));
            body.push(PathCmd::LineTo(Point::new(r.right(), bot.y)));
            body.extend(geom::elliptic_arc_cubics(bot, rx, DB_RY, 0.0, PI));
            body.push(PathCmd::Close);
            els.push(Element::path(body, paint));
            let mut seam = vec![PathCmd::MoveTo(Point::new(r.x, top.y))];
            seam.extend(geom::elliptic_arc_cubics(top, rx, DB_RY, PI, 0.0));
            els.push(Element::path(seam, Paint::stroke(&stroke, 0.5)));
        }
        Form::Queue => {
            let cy = r.center().y;
            let ry = r.h / 2.0;
            let left = Point::new(r.x + QUEUE_RX, cy);
            let right = Point::new(r.right() - QUEUE_RX, cy);
            let mut body = vec![PathCmd::MoveTo(Point::new(left.x, r.y))];
            body.push(PathCmd::LineTo(Point::new(right.x, r.y)));
            body.extend(geom::elliptic_arc_cubics(
                right,
                QUEUE_RX,
                ry,
                -PI / 2.0,
                PI / 2.0,
            ));
            body.push(PathCmd::LineTo(Point::new(left.x, r.bottom())));
            body.extend(geom::elliptic_arc_cubics(
                left,
                QUEUE_RX,
                ry,
                PI / 2.0,
                1.5 * PI,
            ));
            body.push(PathCmd::Close);
            els.push(Element::path(body, paint));
            let mut seam = vec![PathCmd::MoveTo(Point::new(right.x, r.bottom()))];
            seam.extend(geom::elliptic_arc_cubics(
                right,
                QUEUE_RX,
                ry,
                PI / 2.0,
                1.5 * PI,
            ));
            els.push(Element::path(seam, Paint::stroke(&stroke, 0.5)));
        }
    }
    if let Some(g) = card.glyph {
        els.extend(person_glyph(r.center().x, r.y + g, &font));
    }
    // Centre the text block vertically when the card was stretched taller.
    draw_runs(&mut els, &card.runs, r.center().x, r.y, &font);
    Element::group("c4-shape", els)
}

/// Arrowhead polygon with its tip at `tip`, pointing along `dir` (unit).
fn arrowhead(tip: Point, dir: Point, color: &Color) -> Element {
    let back = tip.minus(dir.scale(10.0));
    let n = Point::new(-dir.y, dir.x).scale(4.0);
    Element::path(
        geom::polygon(&[tip, back.plus(n), back.minus(n)]),
        Paint::both(color, color, 1.0),
    )
}

fn overlap_area(a: &Rect, b: &Rect) -> f64 {
    let w = a.right().min(b.right()) - a.x.max(b.x);
    let h = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// Render a parsed and laid-out diagram.
pub fn draw(d: &C4Diagram, lay: &C4Layout, pal: &Palette) -> Scene {
    // mermaid's C4 renderer uses a fixed scheme regardless of theme.
    let ink = Color::new("#444444");
    let label_bg = Color::new("white");
    let cards: Vec<CardLayout> = d.shapes.iter().map(measure_shape).collect();
    let mut bounds = Bounds::default();
    let mut boundary_els = Vec::new();
    let mut boundary_title_els = Vec::new();
    let mut obstacles: Vec<Rect> = Vec::new();
    for (b, r) in d.boundaries.iter().zip(&lay.boundaries) {
        bounds.add_rect(*r);
        let stroke = Color::new(
            b.style
                .border
                .clone()
                .unwrap_or_else(|| ink.css().to_string()),
        );
        let fill = Color::new(b.style.bg.clone().unwrap_or_else(|| "none".into()));
        let mut paint = Paint::both(&fill, &stroke, 1.0);
        if b.kind != BoundaryKind::Node {
            paint = paint.dashed(vec![7.0, 7.0]);
        }
        let els = vec![Element::rect(*r, 2.5, paint)];
        let (runs, title_h) = measure_boundary_title(b, (r.w - 2.0 * BOUNDARY_PAD).max(SHAPE_W));
        let font = b.style.font.clone().map_or_else(|| ink.clone(), Color::new);
        let tw = runs.iter().map(TextRun::width).fold(0.0, f64::max);
        let title_rect = Rect::new(
            r.center().x - tw / 2.0 - 4.0,
            r.y + 6.0,
            tw + 8.0,
            title_h - 4.0,
        );
        obstacles.push(title_rect.pad(2.0));
        // Titles sit above relationship lines, on a light backing.
        let backing = b
            .style
            .bg
            .clone()
            .map_or_else(|| label_bg.clone(), Color::new);
        let mut title = vec![Element::rect(
            title_rect,
            2.0,
            Paint::fill(&backing).with_opacity(0.85),
        )];
        draw_runs(&mut title, &runs, r.center().x, r.y, &font);
        boundary_title_els.push(Element::group("c4-boundary-title", title));
        boundary_els.push(Element::group("c4-boundary", els));
    }
    let mut shape_els = Vec::new();
    for ((s, card), r) in d.shapes.iter().zip(&cards).zip(&lay.shapes) {
        bounds.add_rect(*r);
        obstacles.push(r.pad(4.0));
        shape_els.push(draw_shape(s, card, *r));
    }

    let rect_of = |alias: &str| -> Option<Rect> {
        d.shape_index(alias)
            .map(|i| lay.shapes[i])
            .or_else(|| d.boundary_index(alias).map(|b| lay.boundaries[b]))
    };
    // Keep labels off arrowheads when possible (weaker than covering a card).
    let mut heads: Vec<Rect> = Vec::new();
    for rel in d.rels.iter().filter(|r| r.from != r.to) {
        if let (Some(fr), Some(tr)) = (rect_of(&rel.from), rect_of(&rel.to)) {
            heads.push(Rect::centered(
                geom::clip_rect(&fr, tr.center()),
                24.0,
                24.0,
            ));
            heads.push(Rect::centered(
                geom::clip_rect(&tr, fr.center()),
                24.0,
                24.0,
            ));
        }
    }
    let mut line_els = Vec::new();
    let mut label_els = Vec::new();
    let mut placed_labels: Vec<Rect> = Vec::new();
    for rel in &d.rels {
        let (Some(fr), Some(tr)) = (rect_of(&rel.from), rect_of(&rel.to)) else {
            continue;
        };
        let line_color = rel
            .line_color
            .clone()
            .map_or_else(|| ink.clone(), Color::new);
        let text_color = rel
            .text_color
            .clone()
            .map_or_else(|| ink.clone(), Color::new);
        let (start, end, mid_line) = if rel.from == rel.to {
            // Self relationship: a loop off the right edge.
            let a = Point::new(fr.right(), fr.y + fr.h * 0.3);
            let b = Point::new(fr.right(), fr.y + fr.h * 0.7);
            let pts = [
                a,
                Point::new(fr.right() + 40.0, a.y),
                Point::new(fr.right() + 40.0, b.y),
                b,
            ];
            line_els.push(Element::path(
                geom::polyline(&pts),
                Paint::stroke(&line_color, 1.0),
            ));
            line_els.push(arrowhead(b, Point::new(-1.0, 0.0), &line_color));
            bounds.add_point(Point::new(fr.right() + 40.0, a.y));
            (a, b, (pts[1], pts[2]))
        } else {
            let start = geom::clip_rect(&fr, tr.center());
            let end = geom::clip_rect(&tr, fr.center());
            let dir = end.minus(start).norm();
            let head_start = matches!(rel.kind, RelKind::BiRel | RelKind::Back);
            let head_end = rel.kind != RelKind::Back;
            let a = if head_start {
                start.plus(dir.scale(9.0))
            } else {
                start
            };
            let b = if head_end {
                end.minus(dir.scale(9.0))
            } else {
                end
            };
            line_els.push(Element::line(a, b, Paint::stroke(&line_color, 1.0)));
            if head_end {
                line_els.push(arrowhead(end, dir, &line_color));
            }
            if head_start {
                line_els.push(arrowhead(start, dir.scale(-1.0), &line_color));
            }
            (start, end, (start, end))
        };
        let _ = (start, end);
        // Label block: label lines + [technology].
        let mut lines = wrap(&rel.label, REL_FONT, false, 120.0);
        if let Some(t) = rel.techn.as_ref().filter(|t| !t.is_empty()) {
            lines.push(format!("[{t}]"));
        }
        lines.retain(|l| !l.is_empty());
        if lines.is_empty() {
            continue;
        }
        let tb = TextBlock::new(lines, REL_FONT, false);
        let lh = REL_FONT * 1.25;
        let (w, h) = (tb.width + 6.0, lh * tb.lines.len() as f64 + 4.0);
        let (p0, p1) = mid_line;
        let mut best: Option<(f64, Rect)> = None;
        let dir = p1.minus(p0).norm();
        let normal = Point::new(-dir.y, dir.x);
        let perp = h / 2.0 + 6.0;
        for t in [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8, 0.12, 0.88] {
            for k in [0.0, 1.0, -1.0] {
                let c = p0
                    .lerp(p1, t)
                    .plus(normal.scale(k * perp))
                    .plus(Point::new(rel.offset.0, rel.offset.1));
                let r = Rect::centered(c, w, h);
                let overlap: f64 = obstacles
                    .iter()
                    .chain(&placed_labels)
                    .map(|o| overlap_area(&r, o))
                    .sum::<f64>()
                    + 0.3 * heads.iter().map(|o| overlap_area(&r, o)).sum::<f64>();
                // Prefer the midpoint on the line when nothing is in the way.
                let cost = overlap + 40.0 * (t - 0.5_f64).abs() + 8.0 * k.abs();
                if best.as_ref().is_none_or(|(bc, _)| cost < *bc - 1e-6) {
                    best = Some((cost, r));
                }
            }
        }
        let Some((_, r)) = best else { continue };
        placed_labels.push(r.pad(2.0));
        bounds.add_rect(r);
        label_els.push(Element::rect(
            r,
            2.0,
            Paint::fill(&label_bg).with_opacity(0.85),
        ));
        label_els.push(Element::text(Text::centered(
            r.center(),
            &tb.lines,
            REL_FONT,
            lh,
            &text_color,
        )));
    }

    let mut title_els = Vec::new();
    if let Some(t) = d.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.trim().to_string()], 18.0, true);
        let c = Point::new(content.x + content.w / 2.0, lay.title_band / 2.0 - 4.0);
        let r = Rect::centered(c, tb.width, tb.height);
        bounds.add_rect(r);
        title_els.push(Element::classed_text(
            Text::centered(c, &tb.lines, 18.0, 27.0, &ink).bold(),
            "c4-title",
        ));
    }

    let view_box = bounds.rect().pad(20.0);
    Scene {
        kind: DiagramKind::C4,
        view_box,
        elements: vec![
            Element::group("c4-boundaries", boundary_els),
            Element::group("c4-rels", line_els),
            Element::group("c4-boundary-titles", boundary_title_els),
            Element::group("c4-shapes", shape_els),
            Element::group("c4-rel-labels", label_els),
            Element::group("c4-title", title_els),
        ],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Parse and render a C4 diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let mut d = parse(body, title)?;
    // Config values apply unless the diagram set them with UpdateLayoutConfig.
    let defaults = (4, 2);
    if d.shape_in_row == defaults.0 {
        d.shape_in_row = cfg.section_f64("c4", "c4ShapeInRow", 4.0).max(1.0) as usize;
    }
    if d.boundary_in_row == defaults.1 {
        d.boundary_in_row = cfg.section_f64("c4", "c4BoundaryInRow", 2.0).max(1.0) as usize;
    }
    let lay = layout(&d);
    Ok(draw(&d, &lay, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> C4Diagram {
        parse(src, None).unwrap()
    }

    #[test]
    fn sibling_boundaries_share_mermaids_row_counter() {
        // With c4ShapeInRow=3, b2's two shapes leave one slot in b3's first row,
        // so F and G stack (mermaid never resets `nextData.cnt` between siblings).
        let d = p(r#"C4Context
Boundary(b1, "B1") {
  Boundary(b2, "B2") {
    System(A, "A")
    System(B, "B")
  }
  Boundary(b3, "B3") {
    System(F, "F")
    System(G, "G")
  }
  Boundary(b4, "B4") {
    Boundary(b5, "B5") {
      System(H, "H")
      System(I, "I")
    }
  }
}
UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="1")"#);
        assert_eq!(row_carry(&d), vec![0, 0, 2, 1, 0]);
        let lay = layout(&d);
        let r = |i: usize| lay.shapes[i];
        assert!((r(0).y - r(1).y).abs() < 1e-6 && r(1).x > r(0).right());
        assert!(r(3).y > r(2).bottom(), "G wraps under F");
        // A nested boundary list starts a fresh counter: H and I share a row.
        assert!((r(4).y - r(5).y).abs() < 1e-6);
        // Every shape stays inside its boundary.
        for (b, s) in [(1, 0), (1, 1), (2, 2), (2, 3), (4, 4), (4, 5)] {
            let (o, i) = (lay.boundaries[b], r(s));
            assert!(o.x <= i.x && i.right() <= o.right() && o.y <= i.y && i.bottom() <= o.bottom());
        }
    }

    fn err_line(src: &str) -> usize {
        match parse(src, None) {
            Err(Error::Parse { line, .. }) => line,
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn args_handle_quotes_commas_and_named() {
        let a = parse_args(r#"a, "Hello, world", , $tags="x,y", bare text"#, 1).unwrap();
        assert_eq!(a.pos(0), Some("a"));
        assert_eq!(a.pos(1), Some("Hello, world"));
        assert_eq!(a.pos(2), None);
        assert_eq!(a.named("tags"), Some("x,y"));
        assert_eq!(a.pos(4), Some("bare text"));
        assert_eq!(a.get(3, "tags").as_deref(), Some("x,y"));
        assert!(parse_args("a, \"oops", 3).is_err());
        assert!(matches!(
            parse_args("$link", 7),
            Err(Error::Parse { line: 7, .. })
        ));
    }

    #[test]
    fn parses_elements_and_types() {
        let d = p(r#"C4Container
            title Demo
            Person(u, "User", "A user, of things")
            Container(api, "API", "Rust, axum", "Serves JSON")
            ContainerDb_Ext(db, "DB", "Postgres")
            SystemQueue(q, "Queue")
            Component(c, "Comp", $descr="named descr")"#);
        assert_eq!(d.variant, Variant::Container);
        assert_eq!(d.title.as_deref(), Some("Demo"));
        assert_eq!(d.shapes.len(), 5);
        assert_eq!(d.shapes[0].descr.as_deref(), Some("A user, of things"));
        assert_eq!(d.shapes[1].techn.as_deref(), Some("Rust, axum"));
        assert_eq!(d.shapes[1].descr.as_deref(), Some("Serves JSON"));
        assert_eq!(d.shapes[2].type_name(), "external_container_db");
        assert_eq!(d.shapes[3].form, Form::Queue);
        assert_eq!(d.shapes[4].descr.as_deref(), Some("named descr"));
    }

    #[test]
    fn boundaries_nest() {
        let d = p(r#"C4Context
            Person(a, "A")
            Enterprise_Boundary(e, "Corp") {
              System(s1, "S1")
              System_Boundary(sb, "Inner") {
                System(s2, "S2")
              }
              Boundary(b, "Generic", "team")
              {
              }
            }
            Deployment_Node(n, "Node", "Ubuntu") {
            }
            Rel(a, s2, "Uses", "HTTPS")"#);
        assert_eq!(
            d.root,
            vec![Item::Shape(0), Item::Boundary(0), Item::Boundary(3)]
        );
        let e = &d.boundaries[0];
        assert_eq!(e.type_text, "ENTERPRISE");
        assert_eq!(
            e.children,
            vec![Item::Shape(1), Item::Boundary(1), Item::Boundary(2)]
        );
        assert_eq!(d.boundaries[1].children, vec![Item::Shape(2)]);
        assert_eq!(d.boundaries[2].type_text, "team");
        assert_eq!(d.boundaries[3].kind, BoundaryKind::Node);
        assert_eq!(d.boundaries[3].type_text, "Ubuntu");
        assert_eq!(d.rels[0].techn.as_deref(), Some("HTTPS"));
    }

    #[test]
    fn relationships_and_updates() {
        let d = p(r#"C4Dynamic
            Person(a, "A")
            System(b, "B")
            RelIndex(1, a, b, "first")
            BiRel(b, a, "both")
            Rel_Back(a, b, "back")
            UpdateRelStyle(b, a, $textColor="red", $offsetY="-10")
            UpdateElementStyle(a, $bgColor="grey")
            UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="1")"#);
        // Rel_Back(a,b) replaces RelIndex(a,b) like mermaid (same from/to).
        assert_eq!(d.rels.len(), 2);
        assert_eq!(d.rels[0].kind, RelKind::Back);
        assert_eq!(d.rels[0].label, "back");
        assert_eq!(d.rels[1].text_color.as_deref(), Some("red"));
        assert_eq!(d.rels[1].offset, (0.0, -10.0));
        assert_eq!(d.shapes[0].style.bg.as_deref(), Some("grey"));
        assert_eq!((d.shape_in_row, d.boundary_in_row), (3, 1));
    }

    #[test]
    fn parse_errors_report_lines() {
        assert_eq!(err_line("C4Context\nPerson(a, \"A\")\nFoo(x)"), 3);
        assert_eq!(
            err_line("C4Context\n\nSystem_Boundary(b, \"B\")\nPerson(a)"),
            3
        );
        assert_eq!(
            err_line("C4Context\nSystem_Boundary(b, \"B\") {\nPerson(a)\n"),
            2
        );
        assert_eq!(err_line("C4Context\n}\n"), 2);
        assert_eq!(err_line("C4Context\nPerson(a, \"A\"\n"), 2);
        assert_eq!(err_line("C4Context\nPerson(a)\n\nRel(a, zz, \"x\")"), 4);
        assert_eq!(err_line("C4Context\nPerson(, \"A\")"), 2);
        assert_eq!(
            err_line("C4Context\nUpdateLayoutConfig($c4ShapeInRow=\"x\")"),
            2
        );
    }

    #[test]
    fn shapes_wrap_into_rows() {
        let mut src = String::from("C4Context\n");
        for i in 0..6 {
            src.push_str(&format!("System(s{i}, \"S{i}\")\n"));
        }
        src.push_str("UpdateLayoutConfig($c4ShapeInRow=\"4\")\n");
        let d = p(&src);
        let lay = layout(&d);
        assert_eq!(lay.shapes[0].y, lay.shapes[3].y);
        assert!(lay.shapes[4].y > lay.shapes[0].bottom());
        assert_eq!(lay.shapes[4].x, lay.shapes[0].x);
    }

    #[test]
    fn boundaries_contain_children_and_do_not_overlap() {
        let d = p(r#"C4Container
            title T
            Person(u, "User")
            System_Boundary(a, "A") {
              Container(a1, "A1", "Rust", "Long description text that wraps over several lines in the card")
              Container(a2, "A2")
            }
            System_Boundary(b, "B") {
              ContainerDb(b1, "B1")
            }
            System_Boundary(c, "C") {
              ContainerQueue(c1, "C1")
            }"#);
        let lay = layout(&d);
        let inside = |outer: &Rect, inner: &Rect| {
            inner.x >= outer.x
                && inner.y >= outer.y
                && inner.right() <= outer.right()
                && inner.bottom() <= outer.bottom()
        };
        assert!(inside(&lay.boundaries[0], &lay.shapes[1]));
        assert!(inside(&lay.boundaries[0], &lay.shapes[2]));
        assert!(inside(&lay.boundaries[2], &lay.shapes[4]));
        assert!(!lay.boundaries[0].intersects(&lay.boundaries[1]));
        // Two boundaries per row: the third starts a new row.
        assert_eq!(lay.boundaries[0].y, lay.boundaries[1].y);
        assert!(lay.boundaries[2].y >= lay.boundaries[0].bottom());
        // Title band pushes content down.
        assert!(lay.shapes[0].y >= 40.0);
    }

    #[test]
    fn renders_scene() {
        let src = "C4Context\ntitle X\nPerson(a, \"A\", \"d\")\nSystem_Ext(b, \"B\")\nRel(a, b, \"Uses\", \"HTTPS\")\nBiRel(b, b, \"self\")";
        let s = render(
            src,
            None,
            &MermaidConfig::default(),
            &MermaidConfig::default().palette(),
        )
        .unwrap();
        assert_eq!(s.kind, DiagramKind::C4);
        assert!(s.view_box.w > 400.0 && s.view_box.h > 60.0);
        assert!(s.element_count() > 10);
    }
}
