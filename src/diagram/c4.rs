//! C4 diagrams (`C4Context`, `C4Container`, `C4Component`, `C4Dynamic`, `C4Deployment`).
//!
//! Parsing follows mermaid's `c4Diagram.jison` + `c4Db.js`: macro calls with
//! positional or `$named="..."` arguments, boundaries nested with `{ }`,
//! relationships and the `Update*` style/layout statements.
//!
//! Layout mirrors mermaid's row placement (`c4ShapeInRow` shapes per row, then
//! `c4BoundaryInRow` child boundaries per row) but is computed with Taffy: each
//! boundary is a padded flex column with a title band whose children are rows.
//! Cards are drawn like mermaid 12.3's `c4ShapeAdapter`: unified shapes (person
//! silhouette, rounded box, cylinder, horizontal cylinder, framed rectangle) with a
//! stacked label of bold name, Structurizr type line (`[Container: tech]`) and
//! description. Relationships follow mermaid: the first is straight, later ones are
//! quadratic curves, clipped to the shapes' outlines; labels sit near the midpoint,
//! nudged along the curve to avoid overlaps.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::json::Json;
use crate::layout::boxes::{self, BoxNode, Placed};
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
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

/// Boundary title font (`boundaryFontSize` 14 + 2, bold).
const LABEL_FONT: f64 = 16.0;
const BODY_FONT: f64 = 14.0;
const REL_FONT: f64 = 12.0;
const BOUNDARY_PAD: f64 = 20.0;
const ROW_GAP_X: f64 = 80.0;
const ROW_GAP_Y: f64 = 70.0;
const BOUNDARY_GAP: f64 = 40.0;
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

/// A run of centred text lines inside a boundary title, relative to its top.
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
        let extra = (lines.len().max(1) - 1) as f64;
        // Chromium's SVG text boxes at the default 14px base: name 16px,
        // type 13px, description 12px for one line; 1.1em per extra line.
        let k = base / 14.0;
        let height = match kind {
            SectionKind::Name => 16.0 * k + extra * 1.1 * font,
            SectionKind::Type => 13.0 * k + extra * 1.1 * font,
            SectionKind::Descr => {
                12.0 * k + extra * 1.1 * font + if extra > 0.0 { 0.5 * k } else { 0.0 }
            }
        };
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
    /// Card geometry and label sections, indexed like `shapes`.
    cards: Vec<CardLayout>,
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
pub fn layout(d: &C4Diagram, conf: &C4Conf) -> C4Layout {
    let cards: Vec<CardLayout> = d.shapes.iter().map(|s| measure_shape(s, conf)).collect();
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
        cards,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw_runs(out: &mut Vec<Element>, runs: &[TextRun], cx: f64, top: f64, fill: &Color) {
    for r in runs {
        let c = Point::new(cx, top + r.top + r.height() / 2.0);
        let mut t = Text::centered(c, &r.lines, r.font, r.line_height, fill);
        t.bold = r.bold;
        out.push(Element::text(t));
    }
}

/// A convex piece of a card outline, for ray intersection.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Piece {
    Rect(Rect),
    Ellipse(Point, f64, f64),
}

impl Piece {
    /// Parameter interval `[t_in, t_out]` where `c + t·d` is inside the piece.
    fn interval(&self, c: Point, d: Point) -> Option<(f64, f64)> {
        match *self {
            Piece::Rect(r) => {
                let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
                for (p, dv, a, b) in [(c.x, d.x, r.x, r.right()), (c.y, d.y, r.y, r.bottom())] {
                    if dv.abs() < 1e-12 {
                        if p < a || p > b {
                            return None;
                        }
                    } else {
                        let (t1, t2) = ((a - p) / dv, (b - p) / dv);
                        lo = lo.max(t1.min(t2));
                        hi = hi.min(t1.max(t2));
                    }
                }
                (lo <= hi).then_some((lo, hi))
            }
            Piece::Ellipse(e, a, b) => {
                if a <= 0.0 || b <= 0.0 {
                    return None;
                }
                let (px, py) = ((c.x - e.x) / a, (c.y - e.y) / b);
                let (dx, dy) = (d.x / a, d.y / b);
                let qa = dx * dx + dy * dy;
                let qb = 2.0 * (px * dx + py * dy);
                let qc = px * px + py * py - 1.0;
                let disc = qb * qb - 4.0 * qa * qc;
                if qa < 1e-18 || disc < 0.0 {
                    return None;
                }
                let s = disc.sqrt();
                Some(((-qb - s) / (2.0 * qa), (-qb + s) / (2.0 * qa)))
            }
        }
    }
}

/// Where the ray from `c` toward `toward` leaves the union of `pieces`
/// (`None` when `c` is outside all of them).
fn union_exit(pieces: &[Piece], c: Point, toward: Point) -> Option<Point> {
    let d = toward.minus(c);
    if d.len() < 1e-9 {
        return Some(c);
    }
    let spans: Vec<(f64, f64)> = pieces.iter().filter_map(|p| p.interval(c, d)).collect();
    let mut t = 0.0;
    let mut inside = false;
    loop {
        let next = spans
            .iter()
            .filter(|(lo, hi)| *lo <= t + 1e-9 && *hi > t + 1e-9)
            .map(|&(_, hi)| hi)
            .fold(None, |m: Option<f64>, h| Some(m.map_or(h, |m| m.max(h))));
        match next {
            Some(h) => {
                t = h;
                inside = true;
            }
            None => break,
        }
    }
    inside.then(|| c.plus(d.scale(t)))
}

/// Pieces of a rounded rectangle (two cross rectangles and four corner discs).
fn rounded_rect_pieces(r: Rect, radius: f64) -> Vec<Piece> {
    let k = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let mut v = vec![
        Piece::Rect(Rect::new(r.x + k, r.y, r.w - 2.0 * k, r.h)),
        Piece::Rect(Rect::new(r.x, r.y + k, r.w, r.h - 2.0 * k)),
    ];
    for (x, y) in [
        (r.x + k, r.y + k),
        (r.right() - k, r.y + k),
        (r.x + k, r.bottom() - k),
        (r.right() - k, r.bottom() - k),
    ] {
        v.push(Piece::Ellipse(Point::new(x, y), k, k));
    }
    v
}

/// Outline of a card as convex pieces, matching each unified shape's `intersect`.
fn card_pieces(card: &CardLayout, r: Rect) -> Vec<Piece> {
    match card.geom {
        CardGeom::Rounded | CardGeom::Framed => vec![Piece::Rect(r)],
        CardGeom::Person {
            head_r,
            body_h,
            body_r,
            ..
        } => {
            let mut v =
                rounded_rect_pieces(Rect::new(r.x, r.bottom() - body_h, r.w, body_h), body_r);
            v.push(Piece::Ellipse(
                Point::new(r.center().x, r.y + head_r),
                head_r,
                head_r,
            ));
            v
        }
        CardGeom::Cylinder { ry } => vec![
            Piece::Rect(Rect::new(r.x, r.y + ry, r.w, r.h - 2.0 * ry)),
            Piece::Ellipse(Point::new(r.center().x, r.y + ry), r.w / 2.0, ry),
            Piece::Ellipse(Point::new(r.center().x, r.bottom() - ry), r.w / 2.0, ry),
        ],
        CardGeom::HorizontalCylinder { rx } => vec![
            Piece::Rect(Rect::new(r.x + rx, r.y, r.w - 2.0 * rx, r.h)),
            Piece::Ellipse(Point::new(r.x + rx, r.center().y), rx, r.h / 2.0),
            Piece::Ellipse(Point::new(r.right() - rx, r.center().y), rx, r.h / 2.0),
        ],
    }
}

/// Where a relationship toward `toward` meets a card's outline.
fn card_exit(card: &CardLayout, r: Rect, toward: Point) -> Point {
    union_exit(&card_pieces(card, r), r.center(), toward)
        .unwrap_or_else(|| geom::clip_rect(&r, toward))
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

fn overlap_area(a: &Rect, b: &Rect) -> f64 {
    let w = a.right().min(b.right()) - a.x.max(b.x);
    let h = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// Render a parsed and laid-out diagram.
pub fn draw(d: &C4Diagram, lay: &C4Layout, conf: &C4Conf, pal: &Palette) -> Scene {
    // mermaid's C4 renderer uses a fixed scheme regardless of theme.
    let ink = Color::new("#444444");
    let label_bg = Color::new("white");
    // Arrow markers and the title take the svg's default text fill (`#333`),
    // not the relationship colour.
    let marker = Color::new("#333333");
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
    for ((s, card), r) in d.shapes.iter().zip(&lay.cards).zip(&lay.shapes) {
        bounds.add_rect(*r);
        obstacles.push(r.pad(4.0));
        shape_els.push(draw_shape(s, card, *r, conf));
    }

    let rect_of = |alias: &str| -> Option<Rect> {
        d.shape_index(alias)
            .map(|i| lay.shapes[i])
            .or_else(|| d.boundary_index(alias).map(|b| lay.boundaries[b]))
    };
    // Where a relationship toward `toward` meets an element's (or boundary's) outline.
    let exit_of = |alias: &str, toward: Point| -> Option<Point> {
        match d.shape_index(alias) {
            Some(i) => Some(card_exit(&lay.cards[i], lay.shapes[i], toward)),
            None => d
                .boundary_index(alias)
                .map(|b| geom::clip_rect(&lay.boundaries[b], toward)),
        }
    };
    let ends_of = |rel: &Rel| -> Option<(Point, Point)> {
        let (fr, tr) = (rect_of(&rel.from)?, rect_of(&rel.to)?);
        Some((
            exit_of(&rel.from, tr.center())?,
            exit_of(&rel.to, fr.center())?,
        ))
    };
    // Keep labels off arrowheads when possible (weaker than covering a card).
    let mut heads: Vec<Rect> = Vec::new();
    for rel in d.rels.iter().filter(|r| r.from != r.to) {
        if let Some((s, e)) = ends_of(rel) {
            heads.push(Rect::centered(s, 24.0, 24.0));
            heads.push(Rect::centered(e, 24.0, 24.0));
        }
    }
    let mut line_els = Vec::new();
    let mut label_els = Vec::new();
    let mut placed_labels: Vec<Rect> = Vec::new();
    for (idx, rel) in d.rels.iter().enumerate() {
        let Some(fr) = rect_of(&rel.from) else {
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
        // The label runs along a quadratic curve (start, control, end).
        let curve = if rel.from == rel.to {
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
            line_els.push(arrowhead(b, Point::new(-1.0, 0.0), &marker));
            bounds.add_point(Point::new(fr.right() + 40.0, a.y));
            (pts[1], pts[1].lerp(pts[2], 0.5), pts[2])
        } else {
            let Some((start, end)) = ends_of(rel) else {
                continue;
            };
            // mermaid draws the first relationship straight and every later one
            // as a quadratic curve bowed toward the start.
            let ctrl = if idx == 0 {
                start.lerp(end, 0.5)
            } else {
                Point::new(
                    start.x + (end.x - start.x) / 4.0,
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
                bounds.add_point(ctrl.lerp(start.lerp(end, 0.5), 0.5));
            }
            line_els.push(Element::path(cmds, Paint::stroke(&line_color, 1.0)));
            // Tangents at the ends (fall back to the chord for degenerate curves).
            let chord = end.minus(start).norm();
            let tan_end = match end.minus(ctrl).norm() {
                v if v.len() > 0.5 => v,
                _ => chord,
            };
            let tan_start = match ctrl.minus(start).norm() {
                v if v.len() > 0.5 => v,
                _ => chord,
            };
            if rel.kind != RelKind::Back {
                line_els.push(arrowhead(end, tan_end, &marker));
            }
            if matches!(rel.kind, RelKind::BiRel | RelKind::Back) {
                line_els.push(arrowhead(start, tan_start.scale(-1.0), &marker));
            }
            (start, ctrl, end)
        };
        // Label block: label lines + [technology]. C4Dynamic numbers its steps.
        let label = if d.variant == Variant::Dynamic {
            format!("{}: {}", idx + 1, rel.label)
        } else {
            rel.label.clone()
        };
        // Like mermaid, relationship text only breaks at explicit line breaks.
        let mut lines = text::label_lines(&label);
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
        let (p0, q, p1) = curve;
        let mut best: Option<(f64, Rect)> = None;
        let perp = h / 2.0 + 6.0;
        for t in [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8, 0.12, 0.88] {
            let on = p0.lerp(q, t).lerp(q.lerp(p1, t), t);
            let dir = q.lerp(p1, t).minus(p0.lerp(q, t)).norm();
            let normal = Point::new(-dir.y, dir.x);
            for k in [0.0, 1.0, -1.0] {
                let c = on
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
        let tb = TextBlock::new(vec![t.trim().to_string()], 16.0, false);
        let c = Point::new(content.x + content.w / 2.0, lay.title_band / 2.0 - 4.0);
        let r = Rect::centered(c, tb.width, tb.height);
        bounds.add_rect(r);
        title_els.push(Element::classed_text(
            Text::centered(c, &tb.lines, 16.0, 24.0, &marker),
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
    let conf = C4Conf::from_config(cfg);
    let lay = layout(&d, &conf);
    Ok(draw(&d, &lay, &conf, pal))
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
        let lay = layout(&d, &C4Conf::default());
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
        let lay = layout(&d, &C4Conf::default());
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
        let lay = layout(&d, &C4Conf::default());
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
        assert!((m.h - 57.0).abs() < 1e-6);
        let rx = 28.5 / (2.5 + 57.0 / 50.0);
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
        let p = card_exit(&card, r, Point::new(r.center().x, -500.0));
        assert!(p.y.abs() < 1e-6);
        // Toward a far corner the rounded body is cut inside the bounding box.
        let p = card_exit(
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
}
