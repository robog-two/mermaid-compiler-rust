//! C4 diagrams (`C4Context`, `C4Container`, `C4Component`, `C4Dynamic`, `C4Deployment`).
//!
//! Parsing follows mermaid's `c4Diagram.jison` + `c4Db.js`: macro calls with
//! positional or `$named="..."` arguments, boundaries nested with `{ }`,
//! relationships and the `Update*` style/layout statements.
//!
//! Layout is a port of mermaid 12.3's `c4Renderer`: `drawInsideBoundary` and the
//! `Bounds` row filler (`c4ShapeInRow` shapes per row, wrapping at the screen
//! width, then `c4BoundaryInRow` child boundaries per row), with its quirks kept.
//! Cards are drawn like mermaid 12.3's `c4ShapeAdapter`: unified shapes (person
//! silhouette, rounded box, cylinder, horizontal cylinder, framed rectangle) with a
//! stacked label of bold name, Structurizr type line (`[Container: tech]`) and
//! description. Relationships follow svgDraw: the first is straight, later ones are
//! quadratic curves, clipped by each shape's `intersect`; labels sit at the chord's
//! midpoint shifted right by half their width.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::json::Json;
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text;
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
    /// `$sprite` (only Structurizr shape keywords such as `database` change the drawing).
    pub sprite: Option<String>,
    /// `$shape` (from the element macro or `UpdateElementStyle`).
    pub shape: Option<String>,
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
    // c4Diagram.jison lexes an unquoted attribute as `[^,]+`: when it is the last
    // one it swallows the closing `)` and the statement no longer parses.
    let last = pieces.last().map_or("", |p| p.trim());
    if !last.is_empty() && !last.starts_with(['"', '$']) {
        return Err(Error::parse(
            line,
            format!("Expecting a quoted string, got '{last}' before ')'"),
        ));
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
            Tok::LineStmt { word, rest, line } => {
                if word == "title" {
                    // `"title"\s[^#\n;]+`: a bare `title` is not a token.
                    if rest.trim().is_empty() {
                        return Err(Error::parse(
                            line,
                            "Lexical error: Unrecognized text 'title'",
                        ));
                    }
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
                        sprite: args.get(descr_slot + 1, "sprite"),
                        tags: args.get(descr_slot + 2, "tags"),
                        link: args.get(descr_slot + 3, "link"),
                        shape: args.named("shape").map(str::to_string),
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
                        "UpdateElementStyle" | "UpdateBoundaryStyle" => {
                            let Some(target) = args.get(0, "elementName") else {
                                return Err(Error::parse(
                                    line,
                                    format!("{name} needs an element name"),
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
                                if let Some(v) = args.get(5, "shape") {
                                    s.shape = Some(v);
                                }
                                if let Some(v) = args.get(6, "sprite") {
                                    s.sprite = Some(v);
                                }
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
                        "AddElementTag" | "AddRelTag" | "SHOW_LEGEND" | "LAYOUT_TOP_DOWN"
                        | "LAYOUT_LEFT_RIGHT" | "LAYOUT_WITH_LEGEND" => {}
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

/// `c4.width` default: the target width of an element card.
const SHAPE_W: f64 = 216.0;
/// Gap between the name / type / description sections (`c4LabelHelper`).
const SECTION_GAP: f64 = 3.0;
/// Smallest wrap width `c4LabelHelper` uses.
const MIN_WRAP_WIDTH: f64 = 32.0;

/// Element types, internal and external, that carry per-type config
/// (`C4_ELEMENT_TYPES`) with their default `<type>_bg_color` / `_border_color`.
const ELEMENT_COLORS: [(&str, &str, &str); 20] = [
    ("person", "#08427B", "#073B6F"),
    ("external_person", "#686868", "#8A8A8A"),
    ("system", "#1168BD", "#3C7FC0"),
    ("external_system", "#999999", "#8A8A8A"),
    ("system_db", "#1168BD", "#3C7FC0"),
    ("external_system_db", "#999999", "#8A8A8A"),
    ("system_queue", "#1168BD", "#3C7FC0"),
    ("external_system_queue", "#999999", "#8A8A8A"),
    ("container", "#438DD5", "#3C7FC0"),
    ("external_container", "#B3B3B3", "#A6A6A6"),
    ("container_db", "#438DD5", "#3C7FC0"),
    ("external_container_db", "#B3B3B3", "#A6A6A6"),
    ("container_queue", "#438DD5", "#3C7FC0"),
    ("external_container_queue", "#B3B3B3", "#A6A6A6"),
    ("component", "#85BBF0", "#78A8D8"),
    ("external_component", "#CCCCCC", "#BFBFBF"),
    ("component_db", "#85BBF0", "#78A8D8"),
    ("external_component_db", "#CCCCCC", "#BFBFBF"),
    ("component_queue", "#85BBF0", "#78A8D8"),
    ("external_component_queue", "#CCCCCC", "#BFBFBF"),
];

/// Per-element-type look from the `c4` config.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementLook {
    /// `<type>FontSize`: the label's base size (name; type line 0.75em, description 0.82em).
    pub font_size: f64,
    pub bg: String,
    pub border: String,
}

/// The `c4` config values the element cards depend on.
#[derive(Debug, Clone, PartialEq)]
pub struct C4Conf {
    /// `c4.width`: target card width.
    pub width: f64,
    /// `c4.c4ShapePadding`.
    pub padding: f64,
    /// `c4.wrap`: wrap card text to the card width.
    pub wrap: bool,
    /// Indexed like [`ELEMENT_COLORS`].
    looks: Vec<ElementLook>,
}

impl Default for C4Conf {
    fn default() -> Self {
        C4Conf {
            width: SHAPE_W,
            padding: 20.0,
            wrap: true,
            looks: ELEMENT_COLORS
                .iter()
                .map(|(_, bg, border)| ElementLook {
                    font_size: 14.0,
                    bg: (*bg).to_string(),
                    border: (*border).to_string(),
                })
                .collect(),
        }
    }
}

impl C4Conf {
    /// Read the `c4` section (schema defaults for anything unset).
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let str_of = |key: &str| {
            cfg.section("c4", key)
                .and_then(Json::as_str)
                .map(str::to_string)
        };
        let mut c = C4Conf::default();
        c.width = cfg.section_f64("c4", "width", c.width).max(0.0);
        c.padding = cfg.section_f64("c4", "c4ShapePadding", c.padding).max(0.0);
        c.wrap = cfg.section_bool("c4", "wrap", true);
        for ((ty, _, _), look) in ELEMENT_COLORS.iter().zip(&mut c.looks) {
            look.font_size = cfg
                .section_f64("c4", &format!("{ty}FontSize"), look.font_size)
                .max(1.0);
            if let Some(bg) = str_of(&format!("{ty}_bg_color")) {
                look.bg = bg;
            }
            if let Some(b) = str_of(&format!("{ty}_border_color")) {
                look.border = b;
            }
        }
        c
    }

    /// Look of an element type (`typeC4Shape`), e.g. `external_system_db`.
    pub fn look(&self, type_name: &str) -> &ElementLook {
        let i = ELEMENT_COLORS
            .iter()
            .position(|(t, _, _)| *t == type_name)
            .unwrap_or(2);
        &self.looks[i]
    }
}

/// The unified-renderer shape a card is drawn with (`resolveNodeShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    /// `person`: head circle over a rounded body.
    Person,
    /// `rounded`: a rectangle with 12px corners.
    Rounded,
    /// `cylinder` (database).
    Cylinder,
    /// `h-cyl`: horizontal cylinder (queue).
    HorizontalCylinder,
    /// `fr-rect`: framed rectangle (Structurizr `component`).
    FramedRect,
}

/// Structurizr-style shape keywords accepted via `$shape`, `$sprite` or `$tags`.
fn keyword_shape(v: &str) -> Option<NodeShape> {
    Some(match v.trim().to_ascii_lowercase().as_str() {
        "person" => NodeShape::Person,
        "box" | "rounded" => NodeShape::Rounded,
        "cylinder" | "database" | "db" => NodeShape::Cylinder,
        "queue" | "pipe" => NodeShape::HorizontalCylinder,
        "component" => NodeShape::FramedRect,
        _ => return None,
    })
}

/// The render shape of an element: explicit `$shape`/`$sprite` first, then a
/// recognised `$tags` token, then the element type.
pub fn resolve_shape(s: &Shape) -> NodeShape {
    let explicit = [&s.shape, &s.sprite]
        .into_iter()
        .flatten()
        .find_map(|v| keyword_shape(v));
    if let Some(k) = explicit {
        return k;
    }
    if let Some(k) = s
        .tags
        .as_deref()
        .and_then(|t| t.split(',').find_map(keyword_shape))
    {
        return k;
    }
    match (s.kind, s.form) {
        (ShapeKind::Person, _) => NodeShape::Person,
        (_, Form::Db) => NodeShape::Cylinder,
        (_, Form::Queue) => NodeShape::HorizontalCylinder,
        _ => NodeShape::Rounded,
    }
}

/// Structurizr-style type line, e.g. `[Software System]` or `[Container: Node.js]`.
pub fn stereotype_text(s: &Shape) -> String {
    let name = match s.kind {
        ShapeKind::Person => "Person",
        ShapeKind::System => "Software System",
        ShapeKind::Container => "Container",
        ShapeKind::Component => "Component",
    };
    match s.techn.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => format!("[{name}: {t}]"),
        None => format!("[{name}]"),
    }
}

/// Word wrap to `max` px like mermaid's `createText` (a word wider than
/// `max` is split between characters).
fn wrap(textv: &str, font: f64, bold: bool, max: f64) -> Vec<String> {
    let fits = |s: &str| text::line_width(s, font, bold) <= max;
    let mut out = Vec::new();
    for para in text::label_lines(textv) {
        let mut cur = String::new();
        let mut words: Vec<String> = para.split_whitespace().map(str::to_string).collect();
        words.reverse();
        let first = out.len();
        while let Some(word) = words.pop() {
            let cand = if cur.is_empty() {
                word.clone()
            } else {
                format!("{cur} {word}")
            };
            if fits(&cand) {
                cur = cand;
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                words.push(word);
            } else {
                // Split the word: as many characters as fit (at least one).
                let chars: Vec<char> = word.chars().collect();
                let mut k = 1;
                while k < chars.len() && fits(&chars[..=k].iter().collect::<String>()) {
                    k += 1;
                }
                out.push(chars[..k].iter().collect());
                if k < chars.len() {
                    words.push(chars[k..].iter().collect());
                }
            }
        }
        if !cur.is_empty() || out.len() == first {
            out.push(cur);
        }
    }
    out
}

/// Which `c4LabelHelper` section a text block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Name,
    Type,
    Descr,
}

/// One label section (SVG text, lines 1.1em apart), relative to the label's top.
#[derive(Debug, Clone, PartialEq)]
struct Section {
    kind: SectionKind,
    lines: Vec<String>,
    font: f64,
    bold: bool,
    top: f64,
    height: f64,
    width: f64,
}

impl Section {
    fn new(kind: SectionKind, raw: &str, base: f64, wrap_w: f64) -> Section {
        let (font, bold) = match kind {
            SectionKind::Name => (base, true),
            SectionKind::Type => (base * 0.75, false),
            SectionKind::Descr => (base * 0.82, false),
        };
        // createText measures candidate lines while its tspans still carry
        // `font-weight: normal`, so the bold name wraps at regular-weight widths.
        let lines = wrap(raw, font, false, wrap_w);
        let width = lines
            .iter()
            .map(|l| text::line_width(l, font, bold))
            .fold(0.0, f64::max);
        // The section's SVG text box: rows 1.1em apart, from the first row's top
        // to the last row's bottom, whose descent depends on its glyphs.
        let extra = (lines.len().max(1) - 1) as f64;
        let last = lines.last().map_or("", String::as_str);
        let height = extra * 1.1 * font + text::svg_line_box(font, last);
        Section {
            kind,
            lines,
            font,
            bold,
            top: 0.0,
            height,
            width,
        }
    }
    /// Baseline of the first line below the section top (font ascent).
    fn ascent(&self) -> f64 {
        match self.kind {
            SectionKind::Name => self.font * 13.0 / 14.0,
            SectionKind::Type => self.font * 10.0 / 10.5,
            SectionKind::Descr => self.font * 10.0 / 11.48,
        }
    }
}

/// Shape-specific geometry of a card.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CardGeom {
    Rounded,
    Framed,
    Person {
        head_r: f64,
        overlap: f64,
        body_h: f64,
        body_r: f64,
    },
    Cylinder {
        ry: f64,
    },
    HorizontalCylinder {
        rx: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct CardLayout {
    w: f64,
    h: f64,
    geom: CardGeom,
    sections: Vec<Section>,
    label_h: f64,
    /// Centre of the label block relative to the card centre.
    label_dx: f64,
    label_dy: f64,
}

/// Size a card like the unified shapes do for a `buildC4Node` node
/// (`width = c4.width`, `padding = c4ShapePadding`, no fixed height).
fn measure_shape(s: &Shape, conf: &C4Conf) -> CardLayout {
    let shape = resolve_shape(s);
    let p = conf.padding;
    // Some shapes shrink `node.width` before the label is measured.
    let node_w = match shape {
        NodeShape::Cylinder => (conf.width - p).max(8.0),
        NodeShape::HorizontalCylinder => (conf.width - p / 2.0).max(10.0),
        _ => conf.width,
    };
    let wrap_w = if conf.wrap {
        (node_w - 2.0 * p).max(MIN_WRAP_WIDTH)
    } else {
        f64::INFINITY
    };
    let base = conf.look(&s.type_name()).font_size;
    let mut sections = Vec::new();
    if !s.label.is_empty() {
        sections.push(Section::new(SectionKind::Name, &s.label, base, wrap_w));
    }
    sections.push(Section::new(
        SectionKind::Type,
        &stereotype_text(s),
        base,
        wrap_w,
    ));
    if let Some(d) = s.descr.as_deref().filter(|d| !d.is_empty()) {
        sections.push(Section::new(SectionKind::Descr, d, base, wrap_w));
    }
    let mut y = 0.0;
    for sec in &mut sections {
        sec.top = y;
        y += sec.height + SECTION_GAP;
    }
    let bw = sections.iter().map(|s| s.width).fold(0.0, f64::max);
    let bh = y - SECTION_GAP;
    let (w, h, geom, dx, dy) = match shape {
        NodeShape::Rounded => (
            (bw + 2.0 * p).max(conf.width),
            bh + 2.0 * p,
            CardGeom::Rounded,
            0.0,
            0.0,
        ),
        NodeShape::FramedRect => (
            (bw + 16.0 + p).max(conf.width),
            bh + p,
            CardGeom::Framed,
            0.0,
            0.0,
        ),
        NodeShape::Person => {
            let w = (bw + 2.0 * p).max(conf.width).max(100.0);
            let head_r = (w * 0.23).clamp(16.0, 56.0);
            let overlap = head_r * 0.27;
            let body_h = bh + 2.0 * p;
            let body_r = (w * 0.177).min(body_h * 0.45);
            let total = body_h + 2.0 * head_r - overlap;
            // Label centred on the body.
            let dy = total / 2.0 - body_h / 2.0;
            let geom = CardGeom::Person {
                head_r,
                overlap,
                body_h,
                body_r,
            };
            (w, total, geom, 0.0, dy)
        }
        NodeShape::Cylinder => {
            let w = node_w.max(bw) + p;
            let rx = w / 2.0;
            let ry = rx / (2.5 + w / 50.0);
            let h = bh + p + ry;
            (w, h + 2.0 * ry, CardGeom::Cylinder { ry }, 0.0, p / 1.5)
        }
        NodeShape::HorizontalCylinder => {
            let h = bh + p / 2.0;
            let ry = h / 2.0;
            let rx = ry / (2.5 + h / 50.0);
            let w = node_w.max(bw) + rx + p / 2.0;
            (
                w + 2.0 * rx,
                h,
                CardGeom::HorizontalCylinder { rx },
                -rx,
                0.0,
            )
        }
    };
    CardLayout {
        w,
        h,
        geom,
        sections,
        label_h: bh,
        label_dx: dx,
        label_dy: dy,
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// `screen.availWidth` in mermaid-cli's headless Chromium (no viewport
/// override, so the 800×600 default screen): the width limit of the top row.
pub const SCREEN_WIDTH: f64 = 800.0;

/// The `c4` config values the layout depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConf {
    pub diagram_margin_x: f64,
    pub diagram_margin_y: f64,
    pub shape_margin: f64,
    pub next_line_padding_x: f64,
    pub boundary_font: f64,
    pub message_font: f64,
    /// Boundary titles and relationship texts wrap only with the top-level
    /// `wrap` setting (captured while parsing) and `c4.wrap`.
    pub wrap: bool,
    /// `screen.availWidth`.
    pub screen_width: f64,
}

impl Default for LayoutConf {
    fn default() -> Self {
        LayoutConf {
            diagram_margin_x: 50.0,
            diagram_margin_y: 10.0,
            shape_margin: 50.0,
            next_line_padding_x: 0.0,
            boundary_font: 14.0,
            message_font: 12.0,
            wrap: false,
            screen_width: SCREEN_WIDTH,
        }
    }
}

impl LayoutConf {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let d = LayoutConf::default();
        let f = |key: &str, v: f64| cfg.section_f64("c4", key, v);
        LayoutConf {
            diagram_margin_x: f("diagramMarginX", d.diagram_margin_x),
            diagram_margin_y: f("diagramMarginY", d.diagram_margin_y),
            shape_margin: f("c4ShapeMargin", d.shape_margin),
            next_line_padding_x: f("nextLinePaddingX", d.next_line_padding_x),
            boundary_font: f("boundaryFontSize", d.boundary_font).max(1.0),
            message_font: f("messageFontSize", d.message_font).max(1.0),
            wrap: cfg.top_bool("wrap", false) && cfg.section_bool("c4", "wrap", true),
            screen_width: d.screen_width,
        }
    }
}

/// JavaScript's `Math.round`.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// A text block measured like `calcC4ShapeTextWH`: lines, the widest line's
/// rounded width and the summed per-line box heights.
#[derive(Debug, Clone, PartialEq)]
struct TextWH {
    lines: Vec<String>,
    width: f64,
    height: f64,
    font: f64,
    bold: bool,
    /// Offset of the text from its boundary's top (`label.Y` etc.).
    y: f64,
}

fn calc_text_wh(raw: &str, font: f64, bold: bool, wrap_to: Option<f64>) -> TextWH {
    let lines = match wrap_to {
        Some(w) => wrap(raw, font, bold, w),
        None => text::split_lines(raw),
    };
    let width = match wrap_to {
        Some(w) => w,
        None => lines
            .iter()
            .map(|l| js_round(text::line_width(l, font, bold)))
            .fold(0.0, f64::max),
    };
    let height = lines.iter().map(|l| text::svg_line_box(font, l)).sum();
    TextWH {
        lines,
        width,
        height,
        font,
        bold,
        y: 0.0,
    }
}

/// A boundary's title texts (label, `[type]`, description).
#[derive(Debug, Clone, PartialEq)]
struct BoundaryText {
    label: TextWH,
    kind: Option<TextWH>,
    descr: Option<TextWH>,
}

/// Measure a boundary's title like `drawInsideBoundary`: returns the texts and
/// the height `Y` they take above the boundary's first row.
fn measure_boundary(
    label: &str,
    type_text: &str,
    descr: Option<&str>,
    conf: &LayoutConf,
    width_limit: f64,
) -> (BoundaryText, f64) {
    let wrap_to = conf.wrap.then_some(width_limit);
    let mut label = calc_text_wh(label, conf.boundary_font + 2.0, true, wrap_to);
    label.y = 8.0;
    let mut y = label.y + label.height;
    let kind = (!type_text.is_empty()).then(|| {
        let mut t = calc_text_wh(
            &format!("[{type_text}]"),
            conf.boundary_font,
            false,
            wrap_to,
        );
        t.y = y + 5.0;
        y = t.y + t.height;
        t
    });
    let descr = descr.filter(|t| !t.is_empty()).map(|d| {
        let mut t = calc_text_wh(d, conf.boundary_font - 2.0, false, wrap_to);
        t.y = y + 20.0;
        y = t.y + t.height;
        t
    });
    (BoundaryText { label, kind, descr }, y)
}

/// mermaid's `Bounds`: the extent of what has been placed (`data`) and the
/// row currently being filled (`next`).
#[derive(Debug, Clone, Copy, Default)]
struct Extent {
    startx: f64,
    stopx: f64,
    starty: f64,
    stopy: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct RowBounds {
    data: Extent,
    next: Extent,
    cnt: usize,
    width_limit: f64,
}

impl RowBounds {
    fn set_data(&mut self, x: f64, y: f64) {
        let e = Extent {
            startx: x,
            stopx: x,
            starty: y,
            stopy: y,
        };
        self.data = e;
        self.next = e;
    }

    /// `Bounds.insert`: place a `w`×`h` shape after the previous one, starting
    /// a new row past the width limit or `shape_in_row` shapes.
    fn insert(&mut self, w: f64, h: f64, m: f64, shape_in_row: usize, pad_x: f64) -> Point {
        self.cnt += 1;
        let n = self.next;
        let mut sx = if n.startx == n.stopx {
            n.stopx + m
        } else {
            n.stopx + 2.0 * m
        };
        let mut ex = sx + w;
        let mut sy = n.starty + 2.0 * m;
        let mut ey = sy + h;
        if sx >= self.width_limit || ex >= self.width_limit || self.cnt > shape_in_row {
            sx = n.startx + m + pad_x;
            sy = n.stopy + 2.0 * m;
            ex = sx + w;
            ey = sy + h;
            self.next.stopx = ex;
            self.next.starty = n.stopy;
            self.next.stopy = ey;
            self.cnt = 1;
        }
        for e in [&mut self.data, &mut self.next] {
            e.startx = e.startx.min(sx);
            e.starty = e.starty.min(sy);
            e.stopx = e.stopx.max(ex);
            e.stopy = e.stopy.max(ey);
        }
        Point::new(sx, sy)
    }
}

/// Absolute geometry of a laid-out diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct C4Layout {
    /// Element boxes (top-left corner and size), indexed like the shapes.
    pub shapes: Vec<Rect>,
    /// Boundary boxes, indexed like the boundaries.
    pub boundaries: Vec<Rect>,
    /// Right and bottom edge of everything (`globalBoundaryMaxX/Y`).
    pub max_x: f64,
    pub max_y: f64,
    /// Drawing order: each boundary's shapes, its child boundaries, then itself.
    order: Vec<Item>,
    titles: Vec<Option<BoundaryText>>,
    /// Card geometry and label sections, indexed like `shapes`.
    cards: Vec<CardLayout>,
}

struct LayoutState<'a> {
    d: &'a C4Diagram,
    conf: &'a LayoutConf,
    cards: &'a [CardLayout],
    shapes: Vec<Rect>,
    boundaries: Vec<Rect>,
    titles: Vec<Option<BoundaryText>>,
    order: Vec<Item>,
    max_x: f64,
    max_y: f64,
}

/// What `drawInsideBoundary` iterates over: the implicit `global` boundary
/// (the diagram root) or a declared one.
#[derive(Debug, Clone, Copy)]
enum Container {
    Global,
    Declared(usize),
}

impl LayoutState<'_> {
    fn children(&self, c: Container) -> &[Item] {
        match c {
            Container::Global => &self.d.root,
            Container::Declared(b) => &self.d.boundaries[b].children,
        }
    }

    /// `drawInsideBoundary`: lay out sibling boundaries in rows of
    /// `c4BoundaryInRow`, each with its shapes and nested boundaries. The
    /// siblings share one `Bounds`, so its row counter carries over.
    fn inside(&mut self, parent: &mut RowBounds, list: &[Container]) {
        let (m, mx, my) = (
            self.conf.shape_margin,
            self.conf.diagram_margin_x,
            self.conf.diagram_margin_y,
        );
        let per_row = self.d.boundary_in_row.max(1);
        let mut cur = RowBounds {
            width_limit: parent.width_limit / per_row.min(list.len()) as f64,
            ..RowBounds::default()
        };
        for (i, &c) in list.iter().enumerate() {
            let (label, type_text, descr) = match c {
                Container::Global => ("global", "global", None),
                Container::Declared(b) => {
                    let b = &self.d.boundaries[b];
                    (b.label.as_str(), b.type_text.as_str(), b.descr.as_deref())
                }
            };
            let (title, y_off) =
                measure_boundary(label, type_text, descr, self.conf, cur.width_limit);
            if i % per_row == 0 {
                let x = parent.data.startx + mx;
                let y = parent.data.stopy + my + y_off;
                cur.set_data(x, y);
            } else {
                let x = if cur.data.stopx != cur.data.startx {
                    cur.data.stopx + mx
                } else {
                    cur.data.startx
                };
                let y = cur.data.starty;
                cur.set_data(x, y);
            }
            let items = self.children(c).to_vec();
            let shapes: Vec<usize> = items
                .iter()
                .filter_map(|it| match it {
                    Item::Shape(s) => Some(*s),
                    Item::Boundary(_) => None,
                })
                .collect();
            if !shapes.is_empty() {
                for &s in &shapes {
                    let card = &self.cards[s];
                    let p = cur.insert(
                        card.w,
                        card.h,
                        m,
                        self.d.shape_in_row.max(1),
                        self.conf.next_line_padding_x,
                    );
                    self.shapes[s] = Rect::new(p.x, p.y, card.w, card.h);
                    self.order.push(Item::Shape(s));
                }
                cur.data.stopx += m;
                cur.data.stopy += m;
            }
            let nested: Vec<Container> = items
                .iter()
                .filter_map(|it| match it {
                    Item::Boundary(b) => Some(Container::Declared(*b)),
                    Item::Shape(_) => None,
                })
                .collect();
            if !nested.is_empty() {
                self.inside(&mut cur, &nested);
            }
            if let Container::Declared(b) = c {
                self.boundaries[b] = Rect::new(
                    cur.data.startx,
                    cur.data.starty,
                    cur.data.stopx - cur.data.startx,
                    cur.data.stopy - cur.data.starty,
                );
                self.titles[b] = Some(title);
                self.order.push(Item::Boundary(b));
            }
            parent.data.stopy = parent.data.stopy.max(cur.data.stopy + m);
            parent.data.stopx = parent.data.stopx.max(cur.data.stopx + m);
            self.max_x = self.max_x.max(parent.data.stopx);
            self.max_y = self.max_y.max(parent.data.stopy);
        }
    }
}

/// Lay out cards and boundaries like mermaid's C4 renderer.
pub fn layout(d: &C4Diagram, conf: &C4Conf, lconf: &LayoutConf) -> C4Layout {
    let cards: Vec<CardLayout> = d.shapes.iter().map(|s| measure_shape(s, conf)).collect();
    let mut st = LayoutState {
        d,
        conf: lconf,
        cards: &cards,
        shapes: vec![Rect::default(); d.shapes.len()],
        boundaries: vec![Rect::default(); d.boundaries.len()],
        titles: vec![None; d.boundaries.len()],
        order: Vec::new(),
        max_x: lconf.diagram_margin_x,
        max_y: lconf.diagram_margin_y,
    };
    let mut screen = RowBounds {
        width_limit: lconf.screen_width,
        ..RowBounds::default()
    };
    screen.set_data(lconf.diagram_margin_x, lconf.diagram_margin_y);
    st.inside(&mut screen, &[Container::Global]);
    C4Layout {
        shapes: st.shapes,
        boundaries: st.boundaries,
        max_x: st.max_x,
        max_y: st.max_y,
        order: st.order,
        titles: st.titles,
        cards,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// How far the alphabetic baseline sits below `y` for svgDraw's texts
/// (`dominant-baseline: middle` with `alignment-baseline: mathematical`):
/// Chromium puts it half the rounded font ascent down, measured 6, 7 and 7px
/// at 12, 14 and 16px.
fn baseline_shift(font: f64) -> f64 {
    js_round((0.905 * font).round() / 2.0)
}

/// svgDraw's `byTspan`: one centred `<text>` per line around `y`
/// (`dominant-baseline: middle`).
fn tspan_lines(lines: &[String], x: f64, y: f64, font: f64, bold: bool, fill: &Color) -> Element {
    let n = lines.len() as f64;
    Element::text(Text {
        x,
        y: y - font * (n - 1.0) / 2.0 + baseline_shift(font),
        lines: lines.to_vec(),
        line_height: font,
        font_size: font,
        bold,
        anchor: Anchor::Middle,
        fill: fill.clone(),
    })
}

/// `intersectLine` from mermaid's rendering utilities.
fn intersect_line(p1: Point, p2: Point, q1: Point, q2: Point) -> Option<Point> {
    let a1 = p2.y - p1.y;
    let b1 = p1.x - p2.x;
    let c1 = p2.x * p1.y - p1.x * p2.y;
    let r3 = a1 * q1.x + b1 * q1.y + c1;
    let r4 = a1 * q2.x + b1 * q2.y + c1;
    if r3 != 0.0 && r4 != 0.0 && r3 * r4 > 0.0 {
        return None;
    }
    let a2 = q2.y - q1.y;
    let b2 = q1.x - q2.x;
    let c2 = q2.x * q1.y - q1.x * q2.y;
    let r1 = a2 * p1.x + b2 * p1.y + c2;
    let r2 = a2 * p2.x + b2 * p2.y + c2;
    if r1.abs() < 1e-6 && r2.abs() < 1e-6 && r1 * r2 > 0.0 {
        return None;
    }
    let denom = a1 * b2 - a2 * b1;
    if denom == 0.0 {
        return None;
    }
    Some(Point::new(
        (b1 * c2 - b2 * c1) / denom,
        (a2 * c1 - a1 * c2) / denom,
    ))
}

/// `intersectRect`: where the line from the box centre toward `p` leaves it.
fn intersect_rect(r: Rect, p: Point) -> Point {
    let c = r.center();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    let (mut w, mut h) = (r.w / 2.0, r.h / 2.0);
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
    Point::new(c.x + sx, c.y + sy)
}

/// mermaid's `generateCirclePoints` (points negated, as upstream).
fn circle_points(cx: f64, cy: f64, r: f64, n: usize, start: f64, end: f64) -> Vec<Point> {
    let (a0, a1) = (start.to_radians(), end.to_radians());
    let step = (a1 - a0) / (n as f64 - 1.0);
    (0..n)
        .map(|i| {
            let a = a0 + i as f64 * step;
            Point::new(-(cx + r * a.cos()), -(cy + r * a.sin()))
        })
        .collect()
}

/// The person shape's outline polygon, relative to its centre.
fn person_outline(w: f64, total: f64, head_r: f64, overlap: f64, body_r: f64) -> Vec<Point> {
    let top = -total / 2.0;
    let body_top = top + 2.0 * head_r - overlap;
    let head_cy = top + head_r;
    let phi = ((body_top - head_cy) / head_r).min(1.0).asin().to_degrees();
    let mut v = circle_points(0.0, -head_cy, head_r, 24, 180.0 + phi, -phi);
    let (l, r) = (-(-w / 2.0 + body_r), -(w / 2.0 - body_r));
    let (t, b) = (-(body_top + body_r), -(total / 2.0 - body_r));
    v.extend(circle_points(l, t, body_r, 12, 90.0, 0.0));
    v.extend(circle_points(l, b, body_r, 12, 360.0, 270.0));
    v.extend(circle_points(r, b, body_r, 12, 270.0, 180.0));
    v.extend(circle_points(r, t, body_r, 12, 180.0, 90.0));
    v
}

/// `intersectPolygon`: the polygon is moved so its bounding box's corner sits
/// at the node box's corner; the crossing nearest to `p` wins.
fn intersect_polygon(r: Rect, poly: &[Point], p: Point) -> Point {
    let c = r.center();
    let min_x = poly.iter().map(|q| q.x).fold(f64::INFINITY, f64::min);
    let min_y = poly.iter().map(|q| q.y).fold(f64::INFINITY, f64::min);
    let (left, top) = (r.x - min_x, r.y - min_y);
    let mut best: Option<(f64, Point)> = None;
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let q1 = Point::new(left + a.x, top + a.y);
        let q2 = Point::new(left + b.x, top + b.y);
        if let Some(x) = intersect_line(c, p, q1, q2) {
            let dist = x.minus(p).len();
            if best.is_none_or(|(d, _)| dist < d) {
                best = Some((dist, x));
            }
        }
    }
    best.map_or(c, |(_, x)| x)
}

/// Each unified shape's `intersect` toward `p`.
fn card_intersect(card: &CardLayout, r: Rect, p: Point) -> Point {
    let c = r.center();
    match card.geom {
        CardGeom::Rounded | CardGeom::Framed => intersect_rect(r, p),
        CardGeom::Person {
            head_r,
            overlap,
            body_r,
            ..
        } => intersect_polygon(r, &person_outline(r.w, r.h, head_r, overlap, body_r), p),
        CardGeom::Cylinder { ry } => {
            let rx = r.w / 2.0;
            let mut pos = intersect_rect(r, p);
            let x = pos.x - c.x;
            if rx != 0.0
                && (x.abs() < r.w / 2.0
                    || (x.abs() == r.w / 2.0 && (pos.y - c.y).abs() > r.h / 2.0 - ry))
            {
                let mut y = ry * ry * (1.0 - x * x / (rx * rx));
                if y > 0.0 {
                    y = y.sqrt();
                }
                y = ry - y;
                if p.y - c.y > 0.0 {
                    y = -y;
                }
                pos.y += y;
            }
            pos
        }
        CardGeom::HorizontalCylinder { rx } => {
            let ry = r.h / 2.0;
            let mut pos = intersect_rect(r, p);
            let y = pos.y - c.y;
            if ry != 0.0
                && (y.abs() < r.h / 2.0
                    || (y.abs() == r.h / 2.0 && (pos.x - c.x).abs() > r.w / 2.0 - rx))
            {
                let mut x = rx * rx * (1.0 - y * y / (ry * ry));
                if x != 0.0 {
                    x = x.abs().sqrt();
                }
                x = rx - x;
                if p.x - c.x > 0.0 {
                    x = -x;
                }
                pos.x += x;
            }
            pos
        }
    }
}

/// Draw one element card through its unified shape.
fn draw_shape(s: &Shape, card: &CardLayout, r: Rect, conf: &C4Conf) -> Element {
    let look = conf.look(&s.type_name());
    let fill = Color::new(s.style.bg.clone().unwrap_or_else(|| look.bg.clone()));
    let stroke = Color::new(
        s.style
            .border
            .clone()
            .unwrap_or_else(|| look.border.clone()),
    );
    let font = Color::new(s.style.font.clone().unwrap_or_else(|| "#FFFFFF".into()));
    // `.c4-shape rect/path/circle { stroke-width: 2px }`.
    let paint = Paint::both(&fill, &stroke, 2.0);
    let c = r.center();
    let mut els = Vec::new();
    match card.geom {
        CardGeom::Rounded => els.push(Element::rect(r, 12.0, paint)),
        CardGeom::Framed => {
            els.push(Element::rect(r, 0.0, paint));
            for x in [r.x + 8.0, r.right() - 8.0] {
                els.push(Element::line(
                    Point::new(x, r.y),
                    Point::new(x, r.bottom()),
                    Paint::stroke(&stroke, 2.0),
                ));
            }
        }
        CardGeom::Person {
            head_r,
            body_h,
            body_r,
            ..
        } => {
            // Body first, then the head on top so the full circle shows.
            let body = Rect::new(r.x, r.bottom() - body_h, r.w, body_h);
            els.push(Element::rect(body, body_r, paint.clone()));
            els.push(Element::path(
                geom::ellipse(Point::new(c.x, r.y + head_r), head_r, head_r),
                paint,
            ));
        }
        CardGeom::Cylinder { ry } => {
            let rx = r.w / 2.0;
            let top = Point::new(c.x, r.y + ry);
            let bot = Point::new(c.x, r.bottom() - ry);
            let mut body = vec![PathCmd::MoveTo(Point::new(r.x, top.y))];
            body.extend(geom::elliptic_arc_cubics(top, rx, ry, PI, 2.0 * PI));
            body.push(PathCmd::LineTo(Point::new(r.right(), bot.y)));
            body.extend(geom::elliptic_arc_cubics(bot, rx, ry, 0.0, PI));
            body.push(PathCmd::Close);
            els.push(Element::path(body, paint));
            let mut seam = vec![PathCmd::MoveTo(Point::new(r.x, top.y))];
            seam.extend(geom::elliptic_arc_cubics(top, rx, ry, PI, 0.0));
            els.push(Element::path(seam, Paint::stroke(&stroke, 2.0)));
        }
        CardGeom::HorizontalCylinder { rx } => {
            let ry = r.h / 2.0;
            let left = Point::new(r.x + rx, c.y);
            let right = Point::new(r.right() - rx, c.y);
            let mut body = vec![PathCmd::MoveTo(Point::new(left.x, r.y))];
            body.push(PathCmd::LineTo(Point::new(right.x, r.y)));
            body.extend(geom::elliptic_arc_cubics(
                right,
                rx,
                ry,
                -PI / 2.0,
                PI / 2.0,
            ));
            body.push(PathCmd::LineTo(Point::new(left.x, r.bottom())));
            body.extend(geom::elliptic_arc_cubics(left, rx, ry, PI / 2.0, 1.5 * PI));
            body.push(PathCmd::Close);
            els.push(Element::path(body, paint));
            let mut seam = vec![PathCmd::MoveTo(Point::new(right.x, r.bottom()))];
            seam.extend(geom::elliptic_arc_cubics(right, rx, ry, PI / 2.0, 1.5 * PI));
            els.push(Element::path(seam, Paint::stroke(&stroke, 2.0)));
        }
    }
    // Stacked label: bold name, smaller type line, description.
    let label_top = c.y + card.label_dy - card.label_h / 2.0;
    let x = c.x + card.label_dx;
    for sec in &card.sections {
        let class = match sec.kind {
            SectionKind::Name => "c4-name",
            SectionKind::Type => "c4-type",
            SectionKind::Descr => "c4-descr",
        };
        let t = Text {
            x,
            y: label_top + sec.top + sec.ascent(),
            lines: sec.lines.clone(),
            line_height: 1.1 * sec.font,
            font_size: sec.font,
            bold: sec.bold,
            anchor: Anchor::Middle,
            fill: font.clone(),
        };
        els.push(Element::classed_text(t, class));
    }
    Element::group(format!("c4-shape c4-{}", s.type_name()), els)
}

/// Arrowhead (`M 0 0 L 10 5 L 0 10 z`, 1px past the line end) pointing along `dir` (unit).
fn arrowhead(end: Point, dir: Point, color: &Color) -> Element {
    let tip = end.plus(dir);
    let back = tip.minus(dir.scale(10.0));
    let n = Point::new(-dir.y, dir.x).scale(5.0);
    Element::path(
        geom::polygon(&[tip, back.plus(n), back.minus(n)]),
        Paint::both(color, color, 1.0),
    )
}

/// A relationship end: an element card or a boundary box.
fn end_point(d: &C4Diagram, lay: &C4Layout, alias: &str, toward: Point) -> Option<Point> {
    match d.shape_index(alias) {
        Some(i) => Some(card_intersect(&lay.cards[i], lay.shapes[i], toward)),
        None => d
            .boundary_index(alias)
            .map(|b| intersect_rect(lay.boundaries[b], toward)),
    }
}

fn box_of(d: &C4Diagram, lay: &C4Layout, alias: &str) -> Option<Rect> {
    d.shape_index(alias)
        .map(|i| lay.shapes[i])
        .or_else(|| d.boundary_index(alias).map(|b| lay.boundaries[b]))
}

/// svgDraw `drawBoundary`: a dashed (deployment nodes: solid) box with its
/// centred title texts.
fn draw_boundary(b: &Boundary, r: Rect, title: &BoundaryText) -> Element {
    let ink = Color::new("#444444");
    let stroke = b
        .style
        .border
        .clone()
        .map_or_else(|| ink.clone(), Color::new);
    let fill = Color::new(b.style.bg.clone().unwrap_or_else(|| "none".into()));
    let mut paint = Paint::both(&fill, &stroke, 1.0);
    if b.kind != BoundaryKind::Node {
        paint = paint.dashed(vec![7.0, 7.0]);
    }
    let mut els = vec![Element::rect(r, 2.5, paint)];
    let cx = r.x + r.w / 2.0;
    for t in std::iter::once(&title.label)
        .chain(title.kind.as_ref())
        .chain(title.descr.as_ref())
    {
        els.push(tspan_lines(&t.lines, cx, r.y + t.y, t.font, t.bold, &ink));
    }
    Element::group("c4-boundary", els)
}

/// Render a parsed and laid-out diagram.
pub fn draw(
    d: &C4Diagram,
    lay: &C4Layout,
    conf: &C4Conf,
    lconf: &LayoutConf,
    pal: &Palette,
) -> Result<Scene> {
    let ink = Color::new("#444444");
    // Arrow markers and the title take the svg root's `fill: textColor`.
    let marker = pal.text_color.clone();
    let mut elements = Vec::new();
    for item in &lay.order {
        match *item {
            Item::Shape(i) => {
                elements.push(draw_shape(&d.shapes[i], &lay.cards[i], lay.shapes[i], conf))
            }
            Item::Boundary(b) => {
                if let Some(t) = &lay.titles[b] {
                    elements.push(draw_boundary(&d.boundaries[b], lay.boundaries[b], t));
                }
            }
        }
    }

    let font = lconf.message_font;
    let mut rel_els = Vec::new();
    for (idx, rel) in d.rels.iter().enumerate() {
        let missing = || {
            Error::parse(
                rel.line,
                format!(
                    "C4 rel \"{}\" -> \"{}\" references an unknown shape or boundary",
                    rel.from, rel.to
                ),
            )
        };
        let from_box = box_of(d, lay, &rel.from).ok_or_else(missing)?;
        let to_box = box_of(d, lay, &rel.to).ok_or_else(missing)?;
        let start = end_point(d, lay, &rel.from, to_box.center()).ok_or_else(missing)?;
        let end = end_point(d, lay, &rel.to, from_box.center()).ok_or_else(missing)?;
        let line_color = rel
            .line_color
            .clone()
            .map_or_else(|| ink.clone(), Color::new);
        let text_color = rel
            .text_color
            .clone()
            .map_or_else(|| ink.clone(), Color::new);
        // The first relationship is a straight line, every later one a
        // quadratic curve whose control point sits a quarter of the way along x.
        let ctrl = if idx == 0 {
            start.lerp(end, 0.5)
        } else {
            Point::new(
                start.x + (end.x - start.x) / 2.0 - (end.x - start.x) / 4.0,
                start.y + (end.y - start.y) / 2.0,
            )
        };
        let mut cmds = vec![PathCmd::MoveTo(start)];
        if idx == 0 {
            cmds.push(PathCmd::LineTo(end));
        } else {
            cmds.push(PathCmd::CubicTo(
                start.lerp(ctrl, 2.0 / 3.0),
                end.lerp(ctrl, 2.0 / 3.0),
                end,
            ));
        }
        rel_els.push(Element::path(cmds, Paint::stroke(&line_color, 1.0)));
        // `orient: auto` markers follow the path's end tangents.
        let chord = end.minus(start).norm();
        let tangent = |v: Point| if v.len() > 1e-9 { v.norm() } else { chord };
        if rel.kind != RelKind::Back {
            rel_els.push(arrowhead(end, tangent(end.minus(ctrl)), &marker));
        }
        if matches!(rel.kind, RelKind::BiRel | RelKind::Back) {
            rel_els.push(arrowhead(
                start,
                tangent(ctrl.minus(start)).scale(-1.0),
                &marker,
            ));
        }
        // Labels sit at the chord's midpoint, shifted right by half their width.
        let label = if d.variant == Variant::Dynamic {
            format!("{}: {}", idx + 1, rel.label)
        } else {
            rel.label.clone()
        };
        let lw = calc_text_wh(&label, font, false, None);
        let x = start.x.min(end.x) + (end.x - start.x).abs() / 2.0 + rel.offset.0;
        let y = start.y.min(end.y) + (end.y - start.y).abs() / 2.0 + rel.offset.1;
        rel_els.push(tspan_lines(
            &lw.lines,
            x + lw.width / 2.0,
            y,
            font,
            false,
            &text_color,
        ));
        if let Some(t) = rel.techn.as_ref().filter(|t| !t.is_empty()) {
            let tw = calc_text_wh(t, font, false, None);
            // Drawn `font-style="italic"` (measured upright, as `calculateTextWidth`
            // does).
            let lines = text::balance_marks(text::split_lines(&format!(
                "{}[{t}]{}",
                text::mark::ITALIC_ON,
                text::mark::ITALIC_OFF
            )));
            rel_els.push(tspan_lines(
                &lines,
                x + lw.width.max(tw.width) / 2.0,
                y + font + 5.0,
                font,
                false,
                &text_color,
            ));
        }
    }
    elements.push(Element::group("c4-rels", rel_els));

    let (mx, my) = (lconf.diagram_margin_x, lconf.diagram_margin_y);
    let box_w = lay.max_x - mx;
    let title = d.title.as_ref().map(|t| t.trim()).filter(|t| !t.is_empty());
    if let Some(t) = title {
        elements.push(Element::classed_text(
            Text {
                x: box_w / 2.0 - 4.0 * mx,
                y: my + my,
                lines: vec![t.to_string()],
                line_height: 16.0,
                font_size: 16.0,
                bold: false,
                anchor: Anchor::Start,
                fill: marker.clone(),
            },
            "c4-title",
        ));
    }
    let extra = if title.is_some() { 60.0 } else { 0.0 };
    let width = box_w + 2.0 * mx;
    let height = lay.max_y - my + 2.0 * my;
    Ok(Scene {
        kind: DiagramKind::C4,
        view_box: Rect::new(0.0, -(my + extra), width, height + extra),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    })
}

/// Parse and render a C4 diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    // The rows per line come from the diagram (`UpdateLayoutConfig`) only:
    // mermaid's db starts from 4 and 2 whatever the `c4` config says.
    let d = parse(body, title)?;
    let conf = C4Conf::from_config(cfg);
    let lconf = LayoutConf::from_config(cfg);
    let lay = layout(&d, &conf, &lconf);
    draw(&d, &lay, &conf, &lconf, pal)
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
        let lay = layout(&d, &C4Conf::default(), &LayoutConf::default());
        let r = |i: usize| lay.shapes[i];
        assert!((r(0).y - r(1).y).abs() < 1e-6 && r(1).x > r(0).right());
        assert!(r(3).y > r(2).bottom(), "G wraps under F");
        // A nested boundary list starts a fresh counter, but I would end past
        // the 800px screen width, so it wraps under H too.
        // Top-left corners from mermaid 12.3 (element centres minus 108×36).
        let corners: Vec<(f64, f64)> = lay.shapes.iter().map(|r| (r.x, r.y)).collect();
        assert_eq!(
            corners,
            vec![
                (250.0, 279.0),
                (566.0, 279.0),
                (250.0, 607.0),
                (250.0, 779.0),
                (300.0, 1163.0),
                (300.0, 1335.0)
            ]
        );
        assert_eq!((lay.max_x, lay.max_y), (982.0, 1657.0));
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
        let a = parse_args(r#"a, "Hello, world", , $tags="x,y", bare text,"#, 1).unwrap();
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
            err_line("C4Context\n\nSystem_Boundary(b, \"B\")\nPerson(a, \"A\")"),
            3
        );
        assert_eq!(
            err_line("C4Context\nSystem_Boundary(b, \"B\") {\nPerson(a, \"A\")\n"),
            2
        );
        assert_eq!(err_line("C4Context\n}\n"), 2);
        // An unquoted last attribute swallows the `)` in c4Diagram.jison.
        assert_eq!(err_line("C4Context\nPerson(a, \"A\")\nRel(a, b)"), 3);
        assert_eq!(err_line("C4Context\ntitle"), 2);
        assert_eq!(err_line("C4Context\nPerson(a, \"A\"\n"), 2);
        assert_eq!(
            err_line("C4Context\nPerson(a, \"A\")\n\nRel(a, zz, \"x\")"),
            4
        );
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
        let wide = LayoutConf {
            screen_width: 4000.0,
            ..LayoutConf::default()
        };
        let lay = layout(&d, &C4Conf::default(), &wide);
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
        let lay = layout(&d, &C4Conf::default(), &LayoutConf::default());
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

    fn shape(src: &str) -> Shape {
        p(&format!("C4Context\n{src}")).shapes.remove(0)
    }

    #[test]
    fn adapter_resolves_shapes_and_type_lines() {
        let person = shape(r#"Person_Ext(a, "A")"#);
        assert_eq!(resolve_shape(&person), NodeShape::Person);
        assert_eq!(stereotype_text(&person), "[Person]");
        let db = shape(r#"SystemDb(a, "A")"#);
        assert_eq!(resolve_shape(&db), NodeShape::Cylinder);
        assert_eq!(stereotype_text(&db), "[Software System]");
        let q = shape(r#"ContainerQueue_Ext(a, "A", "Kafka")"#);
        assert_eq!(resolve_shape(&q), NodeShape::HorizontalCylinder);
        assert_eq!(stereotype_text(&q), "[Container: Kafka]");
        let c = shape(r#"Component(a, "A", "Rust")"#);
        assert_eq!(resolve_shape(&c), NodeShape::Rounded);
        assert_eq!(stereotype_text(&c), "[Component: Rust]");
        // $shape / $sprite / $tags keywords override the element type.
        let s = shape(r#"System(a, "A", $shape="queue")"#);
        assert_eq!(resolve_shape(&s), NodeShape::HorizontalCylinder);
        let s = shape(r#"System(a, "A", "d", "database")"#);
        assert_eq!(resolve_shape(&s), NodeShape::Cylinder);
        let s = shape(r#"Container(a, "A", "t", "d", $tags="x, component")"#);
        assert_eq!(resolve_shape(&s), NodeShape::FramedRect);
        let d = p(r#"C4Context
            System(a, "A")
            UpdateElementStyle(a, $shape="person")"#);
        assert_eq!(resolve_shape(&d.shapes[0]), NodeShape::Person);
    }

    #[test]
    fn cards_match_unified_shape_sizes() {
        let conf = C4Conf::default();
        // Person: 216 wide, head r = 0.23w, body = label (16 + 3 + 13) + 2·20.
        let m = measure_shape(&shape(r#"Person(a, "Banking Customer B")"#), &conf);
        assert!((m.w - 216.0).abs() < 1e-9);
        assert!((m.h - (72.0 + 2.0 * 49.68 - 49.68 * 0.27)).abs() < 1e-6);
        // Rounded: name + type, 72 tall.
        let m = measure_shape(&shape(r#"System(a, "Banking System A")"#), &conf);
        assert_eq!((m.w, m.h), (216.0, 72.0));
        // Cylinder: w = 196 + 20, ry = 108 / (2.5 + 216/50).
        let m = measure_shape(&shape(r#"SystemDb(a, "D")"#), &conf);
        let ry = 108.0 / (2.5 + 216.0 / 50.0);
        assert!((m.w - 216.0).abs() < 1e-9);
        assert!((m.h - (32.0 + 20.0 + 3.0 * ry)).abs() < 1e-6);
        // Horizontal cylinder: h = label + 10, w = 206 + rx + 10 plus both caps.
        let m = measure_shape(
            &shape(r#"SystemQueue(a, "Banking System F Queue", "A system of the bank.")"#),
            &conf,
        );
        // The description's last row has a descender: 13px instead of 12px.
        assert!((m.h - 58.0).abs() < 1e-6);
        let rx = 29.0 / (2.5 + 58.0 / 50.0);
        assert!((m.w - (206.0 + 3.0 * rx + 10.0)).abs() < 1e-6);
        // The bold name wraps at regular-weight widths (one line here).
        assert_eq!(m.sections[0].lines.len(), 1);
    }

    #[test]
    fn element_config_and_style_overrides() {
        let mut cfg = MermaidConfig::default();
        cfg = cfg
            .with_json(
                &crate::json::parse(
                    r##"{"c4": {"system_bg_color": "#123456", "systemFontSize": 28}}"##,
                )
                .unwrap(),
            )
            .unwrap();
        let conf = C4Conf::from_config(&cfg);
        assert_eq!(conf.look("system").bg, "#123456");
        assert_eq!(conf.look("system").font_size, 28.0);
        assert_eq!(conf.look("person").bg, "#08427B");
        let d = p(r#"C4Context
            System(a, "A")
            Boundary(b, "B") {
            }
            UpdateBoundaryStyle(b, $bgColor="pink", $borderColor="red")"#);
        assert_eq!(d.boundaries[0].style.bg.as_deref(), Some("pink"));
        assert_eq!(d.boundaries[0].style.border.as_deref(), Some("red"));
    }

    #[test]
    fn person_outline_clips_relationships() {
        let card = measure_shape(&shape(r#"Person(a, "A")"#), &C4Conf::default());
        let r = Rect::new(0.0, 0.0, card.w, card.h);
        // Straight up exits through the head top.
        let p = card_intersect(&card, r, Point::new(r.center().x, -500.0));
        assert!(p.y.abs() < 1e-6);
        // Toward a far corner the rounded body is cut inside the bounding box.
        let p = card_intersect(
            &card,
            r,
            Point::new(r.right() + 1000.0, r.bottom() + 1000.0 * r.h / r.w),
        );
        assert!(p.x < r.right() - 1.0 && p.y < r.bottom() - 1.0);
    }

    #[test]
    fn dynamic_numbers_relationships() {
        let src = "C4Dynamic\nSystem(a, \"A\")\nSystem(b, \"B\")\nRel(a, b, \"Calls\")";
        let s = render(
            src,
            None,
            &MermaidConfig::default(),
            &MermaidConfig::default().palette(),
        )
        .unwrap();
        assert!(format!("{s:?}").contains("1: Calls"));
    }

    fn scene(src: &str) -> Scene {
        let cfg = MermaidConfig::default();
        render(src, None, &cfg, &cfg.palette()).unwrap()
    }

    fn texts(s: &Scene) -> Vec<Text> {
        fn walk(e: &[Element], out: &mut Vec<Text>) {
            for el in e {
                match el {
                    Element::Text { text, .. } => out.push(text.clone()),
                    Element::Group { children, .. } | Element::Rotated { children, .. } => {
                        walk(children, out)
                    }
                    Element::Shape { .. } => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&s.elements, &mut out);
        out
    }

    fn close(a: Rect, b: (f64, f64, f64, f64)) -> bool {
        (a.x - b.0).abs() < 0.01
            && (a.y - b.1).abs() < 0.01
            && (a.w - b.2).abs() < 0.01
            && (a.h - b.3).abs() < 0.01
    }

    #[test]
    fn gallery_diagrams_match_mermaids_view_box() {
        // mermaid 12.3 in mmdc's 800px-wide headless screen.
        let s = scene(include_str!("../../examples/diagrams/c4-container.mmd"));
        assert!(
            close(s.view_box, (0.0, -70.0, 955.489, 1546.478)),
            "{:?}",
            s.view_box
        );
        let s = scene(include_str!("../../examples/diagrams/c4.mmd"));
        assert!(
            close(s.view_box, (0.0, -70.0, 1032.0, 2801.359)),
            "{:?}",
            s.view_box
        );
        // Without a title there is no 60px band.
        let s = scene("C4Context\nSystem(a, \"A\")");
        assert!(
            close(s.view_box, (0.0, -10.0, 516.0, 349.0)),
            "{:?}",
            s.view_box
        );
    }

    #[test]
    fn relationship_and_boundary_texts_follow_svg_draw() {
        // Two stacked systems: the label is centred half its width right of the
        // chord midpoint, the technology 17px lower, baselines 6px below.
        let s = scene(
            "C4Context\nSystem(a, \"A\")\nSystem(c, \"C\")\nRel(a, c, \"HHHH\", \"HHHH\")\nUpdateLayoutConfig($c4ShapeInRow=\"1\")",
        );
        let t = texts(&s);
        let label = t.iter().find(|t| t.lines == ["HHHH"]).unwrap();
        let techn = t
            .iter()
            .find(|t| t.lines.iter().map(|l| text::plain(l)).eq(["[HHHH]"]))
            .unwrap();
        assert_eq!((label.y, techn.y), (295.0, 312.0));
        // The technology is italic.
        assert_eq!(
            text::styled_runs(&techn.lines[0], false),
            [text::StyledRun {
                text: "[HHHH]".into(),
                bold: false,
                italic: true
            }]
        );
        assert!((label.x - techn.x).abs() < 1e-9 && label.x > 258.0);
        // Boundary title: bold label at Y = 8, type 5px under its 17px box.
        let s = scene("C4Context\nBoundary(b, \"HHHH\", \"HHHH\") {\n  System(a, \"A\")\n}");
        let t = texts(&s);
        let label = t.iter().find(|t| t.lines == ["HHHH"]).unwrap();
        let kind = t.iter().find(|t| t.lines == ["[HHHH]"]).unwrap();
        assert_eq!((label.y, kind.y), (138.0, 160.0));
        assert!(label.bold && (label.x - 308.0).abs() < 1e-9);
    }

    #[test]
    fn cylinder_intersect_drops_onto_the_cap() {
        let card = measure_shape(&shape(r#"SystemDb(a, "D")"#), &C4Conf::default());
        let r = Rect::new(0.0, 0.0, card.w, card.h);
        let CardGeom::Cylinder { ry } = card.geom else {
            panic!("not a cylinder")
        };
        // Straight up: the top of the cap; off-centre, the rectangle hit
        // drops vertically onto the ellipse.
        let p = card_intersect(&card, r, Point::new(r.center().x, -1000.0));
        assert!(p.y.abs() < 1e-9);
        let p = card_intersect(&card, r, Point::new(r.right() + 20.0, -1000.0));
        let x = p.x - r.center().x;
        let rx = r.w / 2.0;
        let expect = ry - (ry * ry * (1.0 - x * x / (rx * rx))).sqrt();
        assert!((p.y - expect).abs() < 1e-9);
    }
}
