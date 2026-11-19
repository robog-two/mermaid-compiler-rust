//! Wardley maps (mermaid 12.3 `wardley-beta`).
//!
//! Parsing mirrors the langium grammar (`parser/src/language/wardley`): the line is
//! tokenised with the grammar's terminals in chevrotain order (first match wins, so
//! e.g. `notes` lexes as the `note` keyword exactly like upstream), then each
//! statement is parsed and the model populated in `wardleyParser.ts`'s order.
//!
//! The drawing reproduces `wardleyRenderer.ts`: a fixed `width`×`height` canvas,
//! components projected from `[visibility, evolution]` percentages, stage dividers,
//! pipelines, links with flow markers, evolve trends, sourcing decorations, inertia
//! bars, numbered annotations, notes and (de)accelerator arrows. Upstream removes the
//! diagram's `<style>` element while drawing, so text falls back to the browser's
//! default serif font; we use a serif family too.

use crate::color;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{PathCmd, Point, Rect};
use crate::json::Json;
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::{Palette, ThemeName};

/// mermaid's default evolution stages.
const DEFAULT_STAGES: [&str; 4] = ["Genesis", "Custom Built", "Product", "Commodity"];
/// Browser default (serif) font, which upstream's output ends up using.
const FONT_FAMILY: &str = "\"Times New Roman\", Times, serif";

// ---------------------------------------------------------------- model

/// Sourcing decoration `(build)`, `(buy)`, `(outsource)`, `(market)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Build,
    Buy,
    Outsource,
    Market,
}

/// Direction of a value flow on a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Forward,
    Backward,
    Bidirectional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Anchor,
    Component,
    PipelineComponent,
}

impl NodeKind {
    fn class(self) -> &'static str {
        match self {
            NodeKind::Anchor => "anchor",
            NodeKind::Component => "component",
            NodeKind::PipelineComponent => "pipeline-component",
        }
    }
}

/// A node with coordinates in percent (`x` = evolution, `y` = visibility).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub kind: NodeKind,
    pub label_offset_x: Option<f64>,
    pub label_offset_y: Option<f64>,
    pub in_pipeline: bool,
    pub pipeline_parent: bool,
    pub inertia: bool,
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub source: String,
    pub target: String,
    pub dashed: bool,
    pub label: Option<String>,
    pub flow: Option<Flow>,
}

/// `evolve` arrow: from the node to `target_x` (percent) at `target_y`.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    pub node_id: String,
    pub target_x: f64,
    pub target_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub node_id: String,
    pub component_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub number: i64,
    pub x: f64,
    pub y: f64,
    pub text: String,
}

/// A positioned text (`note`) or force arrow (`accelerator` / `deaccelerator`).
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

/// The populated diagram (upstream `WardleyBuildResult`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Wardley {
    pub title: Option<String>,
    pub size: Option<(f64, f64)>,
    pub stages: Vec<String>,
    pub stage_boundaries: Vec<f64>,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub trends: Vec<Trend>,
    pub pipelines: Vec<Pipeline>,
    pub annotations: Vec<Annotation>,
    pub notes: Vec<Marker>,
    pub accelerators: Vec<Marker>,
    pub deaccelerators: Vec<Marker>,
    pub annotations_box: Option<(f64, f64)>,
}

impl Wardley {
    fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// `WardleyBuilder.addNode`: later declarations replace earlier ones in place,
    /// keeping the class, label offsets and pipeline flags when not given again.
    fn add_node(&mut self, node: Node) {
        match self.node_mut(&node.id) {
            Some(old) => {
                let merged = Node {
                    label_offset_x: node.label_offset_x.or(old.label_offset_x),
                    label_offset_y: node.label_offset_y.or(old.label_offset_y),
                    in_pipeline: old.in_pipeline,
                    pipeline_parent: old.pipeline_parent,
                    ..node
                };
                *old = merged;
            }
            None => self.nodes.push(node),
        }
    }

    /// Exact id, else the first node with that label (pipeline components).
    fn resolve(&self, name: &str) -> String {
        if self.node(name).is_some() {
            return name.to_string();
        }
        self.nodes
            .iter()
            .find(|n| n.label == name)
            .map(|n| n.id.clone())
            .unwrap_or_else(|| name.to_string())
    }
}

// ---------------------------------------------------------------- lexer

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    Wardley,
    Size,
    Evolution,
    Anchor,
    Component,
    Label,
    Inertia,
    Evolve,
    Pipeline,
    Note,
    Annotations,
    Annotation,
    Accelerator,
    Deaccelerator,
}

/// Keyword terminals in grammar order (`annotations` before `annotation`).
const KEYWORDS: [(&str, Kw); 14] = [
    ("wardley-beta", Kw::Wardley),
    ("size", Kw::Size),
    ("evolution", Kw::Evolution),
    ("anchor", Kw::Anchor),
    ("component", Kw::Component),
    ("label", Kw::Label),
    ("inertia", Kw::Inertia),
    ("evolve", Kw::Evolve),
    ("pipeline", Kw::Pipeline),
    ("note", Kw::Note),
    ("annotations", Kw::Annotations),
    ("annotation", Kw::Annotation),
    ("accelerator", Kw::Accelerator),
    ("deaccelerator", Kw::Deaccelerator),
];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `WARDLEY_NUMBER` (decimals only).
    Num(f64),
    /// `INT`.
    Int(f64),
    /// `->`.
    Arrow,
    /// `LINK_PORT`: `+<>`, `+>`, `+<`.
    Port(Flow),
    /// `LINK_ARROW`: `-->`, `-.->`, `>`, `+'label'>` …
    LinkArrow(String),
    /// `LINK_LABEL`: `; text` (already stripped and trimmed).
    LinkLabel(String),
    Strategy(Strategy),
    Kw(Kw),
    /// `NAME_WITH_SPACES`, `ID` or `STRING` (unquoted).
    Name(String),
    Str(String),
    Punct(char),
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Num(v) | Tok::Int(v) => format!("{v}"),
            Tok::Arrow => "->".into(),
            Tok::Port(Flow::Forward) => "+>".into(),
            Tok::Port(Flow::Backward) => "+<".into(),
            Tok::Port(Flow::Bidirectional) => "+<>".into(),
            Tok::LinkArrow(s) => s.clone(),
            Tok::LinkLabel(s) => format!(";{s}"),
            Tok::Strategy(s) => format!("{s:?}").to_lowercase(),
            Tok::Kw(k) => KEYWORDS
                .iter()
                .find(|(_, kk)| kk == k)
                .map(|(s, _)| s.to_string())
                .unwrap_or_default(),
            Tok::Name(s) => s.clone(),
            Tok::Str(s) => format!("\"{s}\""),
            Tok::Punct(c) => c.to_string(),
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '&')
}

/// Length of a `NAME_WITH_SPACES` match at the start of `s`.
fn name_len(s: &str) -> usize {
    let b = s.as_bytes();
    let ws_after = |n: usize| b.get(n).is_none_or(|c| c.is_ascii_whitespace());
    if (s.starts_with("title") && ws_after(5))
        || s.starts_with("accTitle")
        || s.starts_with("accDescr")
    {
        return 0;
    }
    if !b.first().is_some_and(u8::is_ascii_alphabetic) {
        return 0;
    }
    let word = |mut i: usize| {
        while i < b.len() {
            let c = b[i] as char;
            if is_name_char(c) || (c == '-' && b.get(i + 1) != Some(&b'>')) {
                i += 1;
            } else {
                break;
            }
        }
        i
    };
    let mut end = word(1);
    loop {
        let mut j = end;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if j == end || j >= b.len() || !(b[j].is_ascii_alphabetic() || b[j] == b'(') {
            return end;
        }
        end = word(j + 1);
    }
}

/// Length of an `ID` (`[\w]([-\w]*\w)?`) match.
fn id_len(s: &str) -> usize {
    let b = s.as_bytes();
    let w = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    if !b.first().is_some_and(|&c| w(c)) {
        return 0;
    }
    let mut i = 1;
    let mut last_word = 1;
    while i < b.len() && (w(b[i]) || b[i] == b'-') {
        i += 1;
        if w(b[i - 1]) {
            last_word = i;
        }
    }
    last_word
}

/// Langium's `convertString`: strip the quotes and resolve backslash escapes.
fn unquote(s: &str) -> String {
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn lex(line: &str, line_no: usize) -> Result<Vec<Tok>> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let c = rest.chars().next().unwrap_or(' ');
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }
        if rest.starts_with("%%") {
            break;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        // WARDLEY_NUMBER
        if digits > 0 && rest[digits..].starts_with('.') {
            let frac = rest[digits + 1..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            if frac > 0 {
                let n = digits + 1 + frac;
                toks.push(Tok::Num(rest[..n].parse().unwrap_or(0.0)));
                i += n;
                continue;
            }
        }
        // Punctuation keywords; `-` yields to the longer arrow terminals.
        let arrow = ["-->", "-.->", "->"].iter().find(|a| rest.starts_with(**a));
        if let Some(a) = arrow {
            toks.push(if *a == "->" {
                Tok::Arrow
            } else {
                Tok::LinkArrow(a.to_string())
            });
            i += a.len();
            continue;
        }
        if "[],(){}@/-".contains(c) {
            toks.push(Tok::Punct(c));
            i += 1;
            continue;
        }
        // LINK_PORT
        if let Some((p, f)) = [
            ("+<>", Flow::Bidirectional),
            ("+>", Flow::Forward),
            ("+<", Flow::Backward),
        ]
        .iter()
        .find(|(p, _)| rest.starts_with(p))
        {
            toks.push(Tok::Port(*f));
            i += p.len();
            continue;
        }
        // LINK_ARROW
        if c == '>' {
            toks.push(Tok::LinkArrow(">".into()));
            i += 1;
            continue;
        }
        if let Some(body) = rest.strip_prefix("+'") {
            if let Some(q) = body.find('\'') {
                let after = &body[q + 1..];
                let tail = ["<>", "<", ">"].iter().find(|t| after.starts_with(**t));
                if let Some(t) = tail {
                    let n = 2 + q + 1 + t.len();
                    toks.push(Tok::LinkArrow(rest[..n].to_string()));
                    i += n;
                    continue;
                }
            }
        }
        // LINK_LABEL
        if let Some(l) = rest.strip_prefix(';') {
            if !l.is_empty() {
                toks.push(Tok::LinkLabel(l.trim().to_string()));
                break;
            }
        }
        // STRATEGY and keyword terminals (plain prefixes, as in chevrotain).
        if let Some((s, st)) = [
            ("build", Strategy::Build),
            ("buy", Strategy::Buy),
            ("outsource", Strategy::Outsource),
            ("market", Strategy::Market),
        ]
        .iter()
        .find(|(s, _)| rest.starts_with(s))
        {
            toks.push(Tok::Strategy(*st));
            i += s.len();
            continue;
        }
        if let Some((s, k)) = KEYWORDS.iter().find(|(s, _)| rest.starts_with(s)) {
            toks.push(Tok::Kw(*k));
            i += s.len();
            continue;
        }
        let n = name_len(rest);
        if n > 0 {
            toks.push(Tok::Name(rest[..n].to_string()));
            i += n;
            continue;
        }
        // INT
        if digits > 0 {
            let n = if rest.starts_with('0') { 1 } else { digits };
            toks.push(Tok::Int(rest[..n].parse().unwrap_or(0.0)));
            i += n;
            continue;
        }
        // STRING
        if c == '"' || c == '\'' {
            let b = rest.as_bytes();
            let mut j = 1;
            while j < b.len() && b[j] != c as u8 {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            if j < b.len() {
                toks.push(Tok::Str(unquote(&rest[..=j])));
                i += j + 1;
                continue;
            }
        }
        let n = id_len(rest);
        if n > 0 {
            toks.push(Tok::Name(rest[..n].to_string()));
            i += n;
            continue;
        }
        let col = line[..i].chars().count() + 1;
        return Err(Error::parse(
            line_no,
            format!("Lexer error on line {line_no}, column {col}: unexpected character: ->{c}<-"),
        ));
    }
    Ok(toks)
}

// ---------------------------------------------------------------- parser

/// A cursor over one statement's tokens.
struct Cursor<'a> {
    toks: &'a [Tok],
    i: usize,
    line: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.i)
    }
    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.toks.get(self.i);
        self.i += 1;
        t
    }
    fn err(&self, expected: &str) -> Error {
        let found = self
            .toks
            .get(self.i)
            .map(|t| format!("'{}'", t.describe()))
            .unwrap_or_else(|| "end of line".into());
        Error::parse(
            self.line,
            format!(
                "Expecting {expected} but found {found} (line {})",
                self.line
            ),
        )
    }
    fn punct(&mut self, c: char) -> Result<()> {
        match self.peek() {
            Some(Tok::Punct(p)) if *p == c => {
                self.i += 1;
                Ok(())
            }
            _ => Err(self.err(&format!("'{c}'"))),
        }
    }
    fn eat_punct(&mut self, c: char) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(p)) if *p == c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Result<String> {
        match self.peek() {
            Some(Tok::Name(s) | Tok::Str(s)) => {
                self.i += 1;
                Ok(s.clone())
            }
            _ => Err(self.err("a name")),
        }
    }
    fn string(&mut self) -> Result<String> {
        match self.peek() {
            Some(Tok::Str(s)) => {
                self.i += 1;
                Ok(s.clone())
            }
            _ => Err(self.err("a quoted string")),
        }
    }
    /// `WARDLEY_NUMBER` (a decimal such as `0.5`).
    fn decimal(&mut self) -> Result<f64> {
        match self.peek() {
            Some(Tok::Num(v)) => {
                self.i += 1;
                Ok(*v)
            }
            _ => Err(self.err("a decimal number (e.g. 0.5)")),
        }
    }
    fn int(&mut self) -> Result<f64> {
        match self.peek() {
            Some(Tok::Int(v)) => {
                self.i += 1;
                Ok(*v)
            }
            _ => Err(self.err("an integer")),
        }
    }
    /// `CoordinateValue`: decimal or integer.
    fn coord(&mut self) -> Result<f64> {
        match self.peek() {
            Some(Tok::Num(v) | Tok::Int(v)) => {
                self.i += 1;
                Ok(*v)
            }
            _ => Err(self.err("a number")),
        }
    }
    /// `[a, b]` of decimals.
    fn pair(&mut self) -> Result<(f64, f64)> {
        self.punct('[')?;
        let a = self.decimal()?;
        self.punct(',')?;
        let b = self.decimal()?;
        self.punct(']')?;
        Ok((a, b))
    }
    fn end(&self) -> Result<()> {
        if self.i < self.toks.len() {
            Err(self.err("end of line"))
        } else {
            Ok(())
        }
    }
    fn kw(&mut self, k: Kw) -> bool {
        if self.peek() == Some(&Tok::Kw(k)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// `label [x, y]` with optional minus signs.
    fn label(&mut self) -> Result<Option<(f64, f64)>> {
        if !self.kw(Kw::Label) {
            return Ok(None);
        }
        self.punct('[')?;
        let sx = if self.eat_punct('-') { -1.0 } else { 1.0 };
        let x = self.int()? * sx;
        self.punct(',')?;
        let sy = if self.eat_punct('-') { -1.0 } else { 1.0 };
        let y = self.int()? * sy;
        self.punct(']')?;
        Ok(Some((x, y)))
    }
}

/// One evolution stage: name, optional `@boundary`, optional `/ second name`.
#[derive(Debug, Clone, PartialEq)]
struct Stage {
    name: String,
    boundary: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct ComponentDecl {
    name: String,
    vis: f64,
    evo: f64,
    label: Option<(f64, f64)>,
    strategy: Option<Strategy>,
    inertia: bool,
    line: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct LinkDecl {
    from: String,
    to: String,
    from_port: Option<Flow>,
    to_port: Option<Flow>,
    arrow: Option<String>,
    label: Option<String>,
}

/// `component name [evolution] label [x, y]` inside a pipeline, with its line.
type PipelineComponentDecl = (String, f64, Option<(f64, f64)>, usize);

#[derive(Debug, Clone, PartialEq)]
struct PipelineDecl {
    parent: String,
    components: Vec<PipelineComponentDecl>,
    line: usize,
}

/// The grammar's AST: statements grouped by kind, as langium collects them.
#[derive(Debug, Default)]
struct Ast {
    title: Option<String>,
    size: Option<(f64, f64)>,
    evolution: Option<Vec<Stage>>,
    anchors: Vec<(String, f64, f64, usize)>,
    components: Vec<ComponentDecl>,
    links: Vec<LinkDecl>,
    evolves: Vec<(String, f64, usize)>,
    pipelines: Vec<PipelineDecl>,
    notes: Vec<(String, f64, f64, usize)>,
    annotations_boxes: Vec<(f64, f64, usize)>,
    annotation: Vec<(i64, f64, f64, String, usize)>,
    accelerators: Vec<(String, f64, f64, usize)>,
    deaccelerators: Vec<(String, f64, f64, usize)>,
}

/// `title …` value (common `TITLE` terminal): up to a `%%` comment, spaces collapsed.
fn title_value(rest: &str) -> String {
    let v = rest.find("%%").map(|i| &rest[..i]).unwrap_or(rest);
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_stage(c: &mut Cursor) -> Result<Stage> {
    let mut name = c.name()?.trim().to_string();
    let mut boundary = None;
    if c.eat_punct('@') {
        boundary = Some(c.decimal()?);
    }
    if c.eat_punct('/') {
        let second = c.name()?;
        name = format!("{name} / {}", second.trim());
    }
    Ok(Stage { name, boundary })
}

fn parse_statement(c: &mut Cursor, ast: &mut Ast) -> Result<()> {
    match c.peek() {
        Some(Tok::Kw(Kw::Size)) => {
            c.bump();
            c.punct('[')?;
            let w = c.int()?;
            c.punct(',')?;
            let h = c.int()?;
            c.punct(']')?;
            ast.size = Some((w, h));
        }
        Some(Tok::Kw(Kw::Evolution)) => {
            c.bump();
            let mut stages = vec![parse_stage(c)?];
            while c.peek() == Some(&Tok::Arrow) {
                c.bump();
                stages.push(parse_stage(c)?);
            }
            if stages.len() < 2 {
                return Err(c.err("'->'"));
            }
            ast.evolution = Some(stages);
        }
        Some(Tok::Kw(Kw::Anchor)) => {
            c.bump();
            let name = c.name()?;
            let (v, e) = c.pair()?;
            ast.anchors.push((name, v, e, c.line));
        }
        Some(Tok::Kw(Kw::Component)) => {
            c.bump();
            let name = c.name()?;
            let (vis, evo) = c.pair()?;
            let label = c.label()?;
            let mut strategy = None;
            let mut inertia = false;
            if matches!(c.peek(), Some(Tok::Punct('(')))
                && matches!(c.toks.get(c.i + 1), Some(Tok::Strategy(_)))
            {
                c.bump();
                if let Some(Tok::Strategy(s)) = c.bump() {
                    strategy = Some(*s);
                }
                c.punct(')')?;
            }
            if c.kw(Kw::Inertia) {
                inertia = true;
            } else if c.eat_punct('(') {
                if !c.kw(Kw::Inertia) {
                    return Err(c.err("'inertia' or a sourcing strategy"));
                }
                c.punct(')')?;
                inertia = true;
            }
            ast.components.push(ComponentDecl {
                name,
                vis,
                evo,
                label,
                strategy,
                inertia,
                line: c.line,
            });
        }
        Some(Tok::Kw(Kw::Evolve)) => {
            c.bump();
            let name = c.name()?;
            let target = c.decimal()?;
            ast.evolves.push((name, target, c.line));
        }
        Some(Tok::Kw(Kw::Note)) => {
            c.bump();
            let text = c.string()?;
            let (v, e) = c.pair()?;
            ast.notes.push((text, v, e, c.line));
        }
        Some(Tok::Kw(Kw::Annotations)) => {
            c.bump();
            c.punct('[')?;
            let a = c.coord()?;
            c.punct(',')?;
            let b = c.coord()?;
            c.punct(']')?;
            ast.annotations_boxes.push((a, b, c.line));
        }
        Some(Tok::Kw(Kw::Annotation)) => {
            c.bump();
            let n = c.int()?;
            c.punct(',')?;
            c.punct('[')?;
            let a = c.coord()?;
            c.punct(',')?;
            let b = c.coord()?;
            c.punct(']')?;
            let text = c.string()?;
            ast.annotation.push((n as i64, a, b, text, c.line));
        }
        Some(Tok::Kw(k @ (Kw::Accelerator | Kw::Deaccelerator))) => {
            c.bump();
            let name = c.name()?;
            let (a, b) = c.pair()?;
            if *k == Kw::Accelerator {
                ast.accelerators.push((name, a, b, c.line));
            } else {
                ast.deaccelerators.push((name, a, b, c.line));
            }
        }
        Some(Tok::Name(_) | Tok::Str(_)) => {
            let from = c.name()?;
            let from_port = match c.peek() {
                Some(Tok::Port(f)) => {
                    c.bump();
                    Some(*f)
                }
                _ => None,
            };
            let arrow = match c.peek() {
                Some(Tok::Arrow) => {
                    c.bump();
                    Some("->".to_string())
                }
                Some(Tok::LinkArrow(a)) => {
                    c.bump();
                    Some(a.clone())
                }
                _ => None,
            };
            let to = c.name()?;
            let to_port = match c.peek() {
                Some(Tok::Port(f)) => {
                    c.bump();
                    Some(*f)
                }
                _ => None,
            };
            let label = match c.peek() {
                Some(Tok::LinkLabel(l)) => {
                    c.bump();
                    Some(l.clone())
                }
                _ => None,
            };
            ast.links.push(LinkDecl {
                from,
                to,
                from_port,
                to_port,
                arrow,
                label,
            });
        }
        _ => {
            return Err(c.err(
                "a statement ('component', 'anchor', a link, 'evolve', 'pipeline', 'note', 'annotation', ...)",
            ));
        }
    }
    c.end()
}

fn parse_ast(body: &str) -> Result<Ast> {
    let mut ast = Ast::default();
    let lines: Vec<&str> = body.lines().collect();
    let mut header = false;
    let mut idx = 0;
    while idx < lines.len() {
        let line_no = idx + 1;
        let line = lines[idx].trim();
        idx += 1;
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        let mut stmt = line;
        if !header {
            let ok = line.len() >= 12 && line[..12].eq_ignore_ascii_case("wardley-beta");
            if !ok {
                return Err(Error::parse(line_no, "Expecting 'wardley-beta'"));
            }
            header = true;
            stmt = line[12..].trim();
            if stmt.is_empty() {
                continue;
            }
        }
        if let Some(rest) = stmt.strip_prefix("title") {
            if rest.is_empty() || rest.starts_with([' ', '\t']) {
                ast.title = Some(title_value(rest));
                continue;
            }
        }
        let toks = lex(stmt, line_no)?;
        if toks.is_empty() {
            continue;
        }
        let mut c = Cursor {
            toks: &toks,
            i: 0,
            line: line_no,
        };
        if c.kw(Kw::Pipeline) {
            let parent = c.name()?;
            c.punct('{')?;
            c.end()?;
            let mut components = Vec::new();
            let mut closed = false;
            while idx < lines.len() {
                let inner_no = idx + 1;
                let inner = lines[idx].trim();
                idx += 1;
                let toks = lex(inner, inner_no)?;
                if toks.is_empty() {
                    continue;
                }
                let mut ic = Cursor {
                    toks: &toks,
                    i: 0,
                    line: inner_no,
                };
                if ic.eat_punct('}') {
                    ic.end()?;
                    closed = true;
                    break;
                }
                if !ic.kw(Kw::Component) {
                    return Err(ic.err("'component' or '}'"));
                }
                let name = ic.name()?;
                ic.punct('[')?;
                let evo = ic.decimal()?;
                ic.punct(']')?;
                let label = ic.label()?;
                ic.end()?;
                components.push((name, evo, label, inner_no));
            }
            if !closed {
                return Err(Error::parse(
                    lines.len(),
                    format!("Pipeline \"{parent}\" is missing its closing '}}'"),
                ));
            }
            if components.is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("Pipeline \"{parent}\" needs at least one component"),
                ));
            }
            ast.pipelines.push(PipelineDecl {
                parent,
                components,
                line: line_no,
            });
            continue;
        }
        parse_statement(&mut c, &mut ast)?;
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'wardley-beta'"));
    }
    Ok(ast)
}

/// `toPercent`: 0–1 is scaled to percent, 1–100 is used as is.
fn to_percent(v: f64, context: &str, line: usize) -> Result<f64> {
    let n = if v <= 1.0 { v * 100.0 } else { v };
    if !(0.0..=100.0).contains(&n) {
        return Err(Error::parse(
            line,
            format!("{context} must be between 0-1 (decimal) or 0-100 (percentage). Received: {v}"),
        ));
    }
    Ok(n)
}

/// `[visibility, evolution]` → percent `(x, y)`.
fn to_coords(vis: f64, evo: f64, context: &str, line: usize) -> Result<(f64, f64)> {
    Ok((
        to_percent(evo, &format!("{context} evolution"), line)?,
        to_percent(vis, &format!("{context} visibility"), line)?,
    ))
}

fn flow_from_arrow(arrow: &str) -> (Option<Flow>, Option<String>) {
    let Some(rest) = arrow.strip_prefix("+'") else {
        return (None, None);
    };
    let label = rest.find('\'').map(|q| rest[..q].to_string());
    let flow = if arrow.contains("<>") {
        Some(Flow::Bidirectional)
    } else if arrow.contains('<') {
        Some(Flow::Backward)
    } else if arrow.contains('>') {
        Some(Flow::Forward)
    } else {
        None
    };
    (flow, label)
}

/// Parse a `wardley-beta` diagram (`populateDb` order).
pub fn parse(body: &str, title: Option<String>) -> Result<Wardley> {
    let ast = parse_ast(body)?;
    let mut w = Wardley {
        title: ast.title.or(title),
        size: ast.size,
        ..Wardley::default()
    };
    if let Some(stages) = &ast.evolution {
        w.stages = stages.iter().map(|s| s.name.clone()).collect();
        w.stage_boundaries = stages.iter().filter_map(|s| s.boundary).collect();
    }
    let blank = |id: &str, label: &str, x: f64, y: f64, kind: NodeKind| Node {
        id: id.to_string(),
        label: label.to_string(),
        x,
        y,
        kind,
        label_offset_x: None,
        label_offset_y: None,
        in_pipeline: false,
        pipeline_parent: false,
        inertia: false,
        strategy: None,
    };
    for (name, vis, evo, line) in &ast.anchors {
        let (x, y) = to_coords(*vis, *evo, &format!("Anchor \"{name}\""), *line)?;
        w.add_node(blank(name, name, x, y, NodeKind::Anchor));
    }
    for c in &ast.components {
        let (x, y) = to_coords(c.vis, c.evo, &format!("Component \"{}\"", c.name), c.line)?;
        w.add_node(Node {
            label_offset_x: c.label.map(|l| l.0),
            label_offset_y: c.label.map(|l| l.1),
            inertia: c.inertia,
            strategy: c.strategy,
            ..blank(&c.name, &c.name, x, y, NodeKind::Component)
        });
    }
    for (text, vis, evo, line) in &ast.notes {
        let (x, y) = to_coords(*vis, *evo, &format!("Note \"{text}\""), *line)?;
        w.notes.push(Marker {
            text: text.clone(),
            x,
            y,
        });
    }
    for p in &ast.pipelines {
        let parent_y = w.node(&p.parent).map(|n| n.y).ok_or_else(|| {
            Error::parse(
                p.line,
                format!(
                    "Pipeline \"{}\" must reference an existing component with coordinates.",
                    p.parent
                ),
            )
        })?;
        match w.pipelines.iter_mut().find(|q| q.node_id == p.parent) {
            Some(q) => q.component_ids.clear(),
            None => w.pipelines.push(Pipeline {
                node_id: p.parent.clone(),
                component_ids: Vec::new(),
            }),
        }
        if let Some(n) = w.node_mut(&p.parent) {
            n.pipeline_parent = true;
        }
        for (name, evo, label, line) in &p.components {
            let id = format!("{}_{name}", p.parent);
            let ctx = format!("Pipeline component \"{name}\" evolution");
            let x = to_percent(*evo, &ctx, *line)?;
            w.add_node(Node {
                label_offset_x: label.map(|l| l.0),
                label_offset_y: label.map(|l| l.1),
                ..blank(&id, name, x, parent_y, NodeKind::PipelineComponent)
            });
            if let Some(q) = w.pipelines.iter_mut().find(|q| q.node_id == p.parent) {
                q.component_ids.push(id.clone());
            }
            if let Some(n) = w.node_mut(&id) {
                n.in_pipeline = true;
            }
        }
    }
    for l in &ast.links {
        let arrow = l.arrow.as_deref().unwrap_or("");
        let dashed = arrow.contains("-.->") || arrow.contains(".-.");
        let (arrow_flow, flow_label) = flow_from_arrow(arrow);
        let flow = l.from_port.or(l.to_port).or(arrow_flow);
        w.links.push(Link {
            source: w.resolve(&l.from),
            target: w.resolve(&l.to),
            dashed,
            label: flow_label.or_else(|| l.label.clone()),
            flow,
        });
    }
    for (name, target, line) in &ast.evolves {
        if let Some(y) = w.node(name).map(|n| n.y) {
            let ctx = format!("Evolve target for \"{name}\"");
            let target_x = to_percent(*target, &ctx, *line)?;
            let trend = Trend {
                node_id: name.clone(),
                target_x,
                target_y: y,
            };
            match w.trends.iter_mut().find(|t| t.node_id == *name) {
                Some(t) => *t = trend,
                None => w.trends.push(trend),
            }
        }
    }
    if let Some((a, b, line)) = ast.annotations_boxes.first() {
        w.annotations_box = Some(to_coords(*a, *b, "Annotations box", *line)?);
    }
    for (n, a, b, text, line) in &ast.annotation {
        let (x, y) = to_coords(*a, *b, &format!("Annotation {n}"), *line)?;
        w.annotations.push(Annotation {
            number: *n,
            x,
            y,
            text: text.clone(),
        });
    }
    for (name, a, b, line) in &ast.accelerators {
        let (x, y) = to_coords(*a, *b, &format!("Accelerator \"{name}\""), *line)?;
        w.accelerators.push(Marker {
            text: name.clone(),
            x,
            y,
        });
    }
    for (name, a, b, line) in &ast.deaccelerators {
        let (x, y) = to_coords(*a, *b, &format!("Deaccelerator \"{name}\""), *line)?;
        w.deaccelerators.push(Marker {
            text: name.clone(),
            x,
            y,
        });
    }
    Ok(w)
}

// ---------------------------------------------------------------- config & theme

/// `wardley-beta` config (upstream key; `wardley` is accepted too).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WardleyConfig {
    pub width: f64,
    pub height: f64,
    pub padding: f64,
    pub node_radius: f64,
    pub node_label_offset: f64,
    pub axis_font_size: f64,
    pub label_font_size: f64,
    pub show_grid: bool,
    /// `useMaxWidth`: the svg is drawn 100% of mmdc's 784px page wide (never
    /// wider than its own width) while the annotations box is measured.
    pub use_max_width: bool,
}

impl WardleyConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let get = |k: &str| {
            cfg.section("wardley-beta", k)
                .or_else(|| cfg.section("wardley", k))
        };
        let num = |k: &str, d: f64| get(k).and_then(Json::as_f64).unwrap_or(d);
        WardleyConfig {
            width: num("width", 900.0),
            height: num("height", 600.0),
            padding: num("padding", 48.0),
            node_radius: num("nodeRadius", 6.0),
            node_label_offset: num("nodeLabelOffset", 8.0),
            axis_font_size: num("axisFontSize", 12.0),
            label_font_size: num("labelFontSize", 10.0),
            show_grid: get("showGrid").and_then(Json::as_bool).unwrap_or(false),
            use_max_width: get("useMaxWidth").and_then(Json::as_bool).unwrap_or(true),
        }
    }
}

/// The resolved `themeVariables.wardley` colours.
#[derive(Debug, Clone, PartialEq)]
pub struct WardleyColors {
    pub background: Color,
    pub axis: Color,
    pub axis_text: Color,
    pub grid: Color,
    pub component_fill: Color,
    pub component_stroke: Color,
    pub component_label: Color,
    pub link: Color,
    pub evolution: Color,
}

/// Theme colours: the classic themes define `themeVariables.wardley` from
/// `background` / `lineColor` / `primaryTextColor`; neo and redux do not, so the
/// renderer's fallbacks (black strokes, white fills) apply there.
pub fn colors(cfg: &MermaidConfig, pal: &Palette) -> WardleyColors {
    let var = |k: &str| {
        cfg.theme_variables
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim().to_string())
    };
    let inverted = |c: &str| {
        color::parse(c)
            .map(|c| c.invert().to_hex())
            .unwrap_or_else(|| c.to_string())
    };
    let classic = |bg: &str, line: &str, primary: &str, fill: Option<&str>, evo: &str| {
        let bg = var("background").unwrap_or_else(|| bg.to_string());
        let line = var("lineColor").unwrap_or_else(|| line.to_string());
        let text = var("primaryTextColor")
            .unwrap_or_else(|| inverted(&var("primaryColor").unwrap_or_else(|| primary.into())));
        let fill = match fill {
            Some(f) => var("mainBkg")
                .or_else(|| var("primaryColor"))
                .unwrap_or_else(|| f.to_string()),
            None => bg.clone(),
        };
        let evo = var("wardleyEvolutionColor").unwrap_or_else(|| evo.to_string());
        WardleyColors {
            background: Color::new(&bg),
            axis: Color::new(&line),
            axis_text: Color::new(&text),
            grid: Color::new(var("gridColor").unwrap_or_else(|| "lightgrey".into())),
            component_fill: Color::new(fill),
            component_stroke: Color::new(&line),
            component_label: Color::new(&text),
            link: Color::new(&line),
            evolution: Color::new(evo),
        }
    };
    let neo = |bg: &str, text: &str| WardleyColors {
        background: Color::new(var("background").unwrap_or_else(|| bg.into())),
        axis: Color::new("#000"),
        axis_text: Color::new(var("primaryTextColor").unwrap_or_else(|| text.into())),
        grid: Color::new("rgba(100, 100, 100, 0.2)"),
        component_fill: Color::new("#fff"),
        component_stroke: Color::new("#000"),
        component_label: Color::new(var("primaryTextColor").unwrap_or_else(|| text.into())),
        link: Color::new("#000"),
        evolution: Color::new("#dc3545"),
    };
    let mut c = match cfg.theme {
        ThemeName::Default | ThemeName::Null => {
            classic("white", "#333333", "#ECECFF", None, "#dc3545")
        }
        ThemeName::Forest => classic("white", "#000000", "#cde498", None, "#dc3545"),
        ThemeName::Neutral => classic("#ffffff", "#666", "#eee", None, "#dc3545"),
        ThemeName::Dark => classic("#333", "lightgrey", "#1f2020", Some("#1f2020"), "#ff6b6b"),
        ThemeName::Base => {
            let bg = var("background").unwrap_or_else(|| "#f4f4f4".into());
            let line = var("lineColor").unwrap_or_else(|| inverted(&bg));
            let mut c = classic(&bg, &line, "#fff4dd", None, "#dc3545");
            if var("primaryTextColor").is_none() {
                c.axis_text = pal.node_text.clone();
                c.component_label = pal.node_text.clone();
            }
            c
        }
        ThemeName::Neo => neo("#ffffff", "#333"),
        ThemeName::Redux | ThemeName::ReduxColor => neo("#ffffff", "#28253D"),
        ThemeName::NeoDark | ThemeName::ReduxDark | ThemeName::ReduxDarkColor => {
            neo("#333", "#e0dfdf")
        }
    };
    // Explicit `themeVariables.wardley.*` overrides.
    if let Some(Json::Object(pairs)) = cfg.section("themeVariables", "wardley") {
        for (k, v) in pairs {
            let Some(v) = v.as_str() else { continue };
            let v = Color::new(v.trim());
            match k.as_str() {
                "backgroundColor" => c.background = v,
                "axisColor" => c.axis = v,
                "axisTextColor" => c.axis_text = v,
                "gridColor" => c.grid = v,
                "componentFill" => c.component_fill = v,
                "componentStroke" => c.component_stroke = v,
                "componentLabelColor" => c.component_label = v,
                "linkStroke" => c.link = v,
                "evolutionStroke" => c.evolution = v,
                _ => {}
            }
        }
    }
    c
}

// ---------------------------------------------------------------- drawing

/// Text at an SVG `y` with the given `dominant-baseline`.
fn text(
    x: f64,
    y: f64,
    s: &str,
    font: f64,
    baseline: crate::text::Baseline,
    anchor: Anchor,
    fill: &Color,
) -> Text {
    Text {
        x,
        y: y + serif_baseline_shift(baseline, font),
        lines: vec![s.to_string()],
        line_height: font * 1.2,
        font_size: font,
        bold: false,
        weight: None,
        anchor,
        fill: fill.clone(),
        tspan_central: false,
        dy_tspans: false,
        family: None,
    }
}
/// `dominant-baseline: middle`.
const MIDDLE: crate::text::Baseline = crate::text::Baseline::Middle;

/// [`crate::text::baseline_shift`] for the page's default serif font, which
/// wardley's unstyled text falls back to (Liberation Serif: ascent 0.891em,
/// descent 0.216em, x-height 940/2048em).
fn serif_baseline_shift(b: crate::text::Baseline, font: f64) -> f64 {
    use crate::text::Baseline;
    let ascent = (0.891 * font).round();
    let descent = (0.216 * font).round();
    match b {
        Baseline::Middle => (font * 940.0 / 4096.0 * 64.0 - 0.5).ceil() / 64.0,
        Baseline::Central => (ascent - descent) / 2.0,
        Baseline::Alphabetic => 0.0,
        other => crate::text::baseline_shift(other, font),
    }
}
/// `dominant-baseline: central`.
const CENTRAL: crate::text::Baseline = crate::text::Baseline::Central;

fn line(a: Point, b: Point, stroke: &Color, w: f64) -> Element {
    Element::line(a, b, Paint::stroke(stroke, w))
}

fn circle(c: Point, r: f64, paint: Paint) -> Element {
    Element::shape(Geometry::Circle { c, r }, paint)
}

/// A `viewBox="0 0 10 10"` marker path placed at `at`, oriented along `dir`.
fn marker(at: Point, dir: Point, scale: f64, refx: f64, pts: &[(f64, f64)]) -> Vec<PathCmd> {
    let perp = Point::new(-dir.y, dir.x);
    let mut cmds: Vec<PathCmd> = pts
        .iter()
        .enumerate()
        .map(|(i, &(px, py))| {
            let p = at
                .plus(dir.scale((px - refx) * scale))
                .plus(perp.scale((py - 5.0) * scale));
            if i == 0 {
                PathCmd::MoveTo(p)
            } else {
                PathCmd::LineTo(p)
            }
        })
        .collect();
    cmds.push(PathCmd::Close);
    cmds
}

const ARROW_END: [(f64, f64); 3] = [(0.0, 0.0), (10.0, 5.0), (0.0, 10.0)];
const ARROW_START: [(f64, f64); 3] = [(10.0, 0.0), (0.0, 5.0), (10.0, 10.0)];

/// Unit vector from `a` to `b` (zero when they coincide).
fn unit(a: Point, b: Point) -> Point {
    let d = b.minus(a);
    let len = d.len();
    if len > 0.0 {
        d.scale(1.0 / len)
    } else {
        Point::new(0.0, 0.0)
    }
}

// ---------------------------------------------------------------- mermaid's DOM

/// Stands for the svg's id in the markup; the writer puts the real one in.
pub const ID_MARK: &str = "\u{1}SVGID\u{1}";

/// An attribute value as `XMLSerializer` writes it.
fn attr_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// A number as JavaScript prints it.
fn jn(v: f64) -> String {
    crate::output::svg::js_num(v)
}

type Attrs<'a> = &'a [(&'a str, String)];

/// `<tag a="v" ...>`, or `<tag .../>` when `close`.
fn open(o: &mut String, tag: &str, attrs: Attrs, close: bool) {
    o.push('<');
    o.push_str(tag);
    for (k, v) in attrs {
        o.push(' ');
        o.push_str(k);
        o.push_str("=\"");
        o.push_str(v);
        o.push('"');
    }
    o.push_str(if close { "/>" } else { ">" });
}

/// A childless element, or one holding `text` (an empty text is no child).
fn leaf(o: &mut String, tag: &str, attrs: Attrs, text: &str) {
    if text.is_empty() {
        open(o, tag, attrs, true);
    } else {
        open(o, tag, attrs, false);
        o.push_str(&crate::output::svg::esc_text(text));
        o.push_str("</");
        o.push_str(tag);
        o.push('>');
    }
}

/// A `<g class=...>` around what `f` writes (`<g class=.../>` when nothing).
fn group(o: &mut String, class: &str, f: impl FnOnce(&mut String)) {
    let at = [("class", attr_esc(class))];
    let mut inner = String::new();
    f(&mut inner);
    if inner.is_empty() {
        open(o, "g", &at, true);
    } else {
        open(o, "g", &at, false);
        o.push_str(&inner);
        o.push_str("</g>");
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

fn css(v: &Color) -> String {
    attr_esc(v.css())
}

/// The DOM `wardleyRenderer.ts` builds, as mmdc serialises it (d3 `attr`
/// order, JavaScript's arithmetic order and number text). [`draw`] builds
/// the same map as scene elements for the rasterisers.
fn markup(w: &Wardley, wc: &WardleyConfig, col: &WardleyColors) -> String {
    use std::f64::consts::PI;
    let (width, height) = w.size.unwrap_or((wc.width, wc.height));
    let pad = wc.padding;
    let r = wc.node_radius;
    let square = r * 1.6;
    let cw = width - pad * 2.0;
    let ch = height - pad * 2.0;
    let px = |v: f64| pad + (v / 100.0) * cw;
    let py = |v: f64| height - pad - (v / 100.0) * ch;
    let id = ID_MARK;
    let mut o = String::new();

    group(&mut o, "wardley-map", |o| {
        open(
            o,
            "rect",
            &[
                ("class", s("wardley-background")),
                ("width", jn(width)),
                ("height", jn(height)),
                ("fill", css(&col.background)),
            ],
            true,
        );
        if let Some(t) = w.title.as_ref().filter(|t| !t.is_empty()) {
            leaf(
                o,
                "text",
                &[
                    ("class", s("wardley-title")),
                    ("x", jn(width / 2.0)),
                    ("y", jn(pad / 2.0)),
                    ("fill", css(&col.axis_text)),
                    ("font-size", jn(wc.axis_font_size * 1.05)),
                    ("font-weight", s("bold")),
                    ("text-anchor", s("middle")),
                    ("dominant-baseline", s("middle")),
                ],
                t,
            );
        }

        // Axes.
        group(o, "wardley-axes", |o| {
            for (x1, x2, y1, y2) in [
                (pad, width - pad, height - pad, height - pad),
                (pad, pad, pad, height - pad),
            ] {
                open(
                    o,
                    "line",
                    &[
                        ("x1", jn(x1)),
                        ("x2", jn(x2)),
                        ("y1", jn(y1)),
                        ("y2", jn(y2)),
                        ("stroke", css(&col.axis)),
                        ("stroke-width", s("1")),
                    ],
                    true,
                );
            }
            let (yx, yy) = (pad / 3.0, pad + ch / 2.0);
            leaf(
                o,
                "text",
                &[
                    ("class", s("wardley-axis-label wardley-axis-label-x")),
                    ("x", jn(pad + cw / 2.0)),
                    ("y", jn(height - pad / 4.0)),
                    ("fill", css(&col.axis_text)),
                    ("font-size", jn(wc.axis_font_size)),
                    ("font-weight", s("bold")),
                    ("text-anchor", s("middle")),
                ],
                "Evolution",
            );
            leaf(
                o,
                "text",
                &[
                    ("class", s("wardley-axis-label wardley-axis-label-y")),
                    ("x", jn(yx)),
                    ("y", jn(yy)),
                    ("fill", css(&col.axis_text)),
                    ("font-size", jn(wc.axis_font_size)),
                    ("font-weight", s("bold")),
                    ("text-anchor", s("middle")),
                    ("transform", format!("rotate(-90 {} {})", jn(yx), jn(yy))),
                ],
                "Visibility",
            );
        });

        // Evolution stages.
        let stages: Vec<String> = if w.stages.is_empty() {
            DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
        } else {
            w.stages.clone()
        };
        let spans: Vec<(f64, f64)> = if w.stage_boundaries.len() == stages.len() {
            let mut prev = 0.0;
            w.stage_boundaries
                .iter()
                .map(|&b| {
                    let s = (prev, b);
                    prev = b;
                    s
                })
                .collect()
        } else {
            let sw = 1.0 / stages.len() as f64;
            (0..stages.len())
                .map(|i| (i as f64 * sw, (i + 1) as f64 * sw))
                .collect()
        };
        group(o, "wardley-stages", |o| {
            for (i, (stage, (a, b))) in stages.iter().zip(&spans).enumerate() {
                let sx = pad + a * cw;
                let ex = pad + b * cw;
                if i > 0 {
                    open(
                        o,
                        "line",
                        &[
                            ("x1", jn(sx)),
                            ("x2", jn(sx)),
                            ("y1", jn(pad)),
                            ("y2", jn(height - pad)),
                            ("stroke", s("#000")),
                            ("stroke-width", s("1")),
                            ("stroke-dasharray", s("5 5")),
                            ("opacity", s("0.8")),
                        ],
                        true,
                    );
                }
                leaf(
                    o,
                    "text",
                    &[
                        ("class", s("wardley-stage-label")),
                        ("x", jn((sx + ex) / 2.0)),
                        ("y", jn(height - pad / 1.5)),
                        ("fill", css(&col.axis_text)),
                        ("font-size", jn(wc.axis_font_size - 2.0)),
                        ("text-anchor", s("middle")),
                    ],
                    stage,
                );
            }
        });

        if wc.show_grid {
            group(o, "wardley-grid", |o| {
                for i in 1..4 {
                    let ratio = i as f64 / 4.0;
                    let x = pad + cw * ratio;
                    let y = height - pad - ch * ratio;
                    for (x1, x2, y1, y2) in [(x, x, pad, height - pad), (pad, width - pad, y, y)] {
                        open(
                            o,
                            "line",
                            &[
                                ("x1", jn(x1)),
                                ("x2", jn(x2)),
                                ("y1", jn(y1)),
                                ("y2", jn(y2)),
                                ("stroke", css(&col.grid)),
                                ("stroke-dasharray", s("2 6")),
                            ],
                            true,
                        );
                    }
                }
            });
        }

        // Projected positions (pipeline parents move onto their box below).
        let mut pos: Vec<(f64, f64)> = w.nodes.iter().map(|k| (px(k.x), py(k.y))).collect();
        let index = |id: &str| w.nodes.iter().position(|k| k.id == id);

        if !w.pipelines.is_empty() {
            let mut boxes = String::new();
            let mut plinks = String::new();
            for p in &w.pipelines {
                if p.component_ids.is_empty() {
                    continue;
                }
                let mut comps: Vec<usize> =
                    p.component_ids.iter().filter_map(|k| index(k)).collect();
                comps.sort_by(|a, b| w.nodes[*a].x.total_cmp(&w.nodes[*b].x));
                for pair in comps.windows(2) {
                    let (a, b) = (pos[pair[0]], pos[pair[1]]);
                    open(
                        &mut plinks,
                        "line",
                        &[
                            ("class", s("wardley-pipeline-evolution-link")),
                            ("x1", jn(a.0)),
                            ("y1", jn(a.1)),
                            ("x2", jn(b.0)),
                            ("y2", jn(b.1)),
                            ("stroke", css(&col.link)),
                            ("stroke-width", s("1")),
                            ("stroke-dasharray", s("4 4")),
                        ],
                        true,
                    );
                }
                let mut min_x = f64::INFINITY;
                let mut max_x = f64::NEG_INFINITY;
                let mut y = 0.0;
                for k in &p.component_ids {
                    if let Some(i) = index(k) {
                        min_x = min_x.min(pos[i].0);
                        max_x = max_x.max(pos[i].0);
                        y = pos[i].1;
                    }
                }
                if min_x != f64::INFINITY && max_x != f64::NEG_INFINITY {
                    let bpad = 15.0;
                    let bh = r * 4.0;
                    let top = y - bh / 2.0;
                    if let Some(pi) = index(&p.node_id) {
                        pos[pi] = ((min_x + max_x) / 2.0, top - square / 6.0);
                    }
                    open(
                        &mut boxes,
                        "rect",
                        &[
                            ("class", s("wardley-pipeline-box")),
                            ("x", jn(min_x - bpad)),
                            ("y", jn(top)),
                            ("width", jn(max_x - min_x + bpad * 2.0)),
                            ("height", jn(bh)),
                            ("fill", s("none")),
                            ("stroke", css(&col.axis)),
                            ("stroke-width", s("1.5")),
                            ("rx", s("4")),
                            ("ry", s("4")),
                        ],
                        true,
                    );
                }
            }
            group(o, "wardley-pipelines", |o| o.push_str(&boxes));
            group(o, "wardley-pipeline-links", |o| o.push_str(&plinks));
        }

        // Links (minus pipeline component -> own parent), then their labels.
        group(o, "wardley-links", |o| {
            let mut labels = String::new();
            for l in &w.links {
                let (Some(si), Some(ti)) = (index(&l.source), index(&l.target)) else {
                    continue;
                };
                if w.pipelines
                    .iter()
                    .any(|p| p.node_id == l.target && p.component_ids.contains(&l.source))
                {
                    continue;
                }
                let (sp, tp) = (pos[si], pos[ti]);
                let rad = |i: usize| {
                    if w.nodes[i].pipeline_parent {
                        square / 2f64.sqrt()
                    } else {
                        r
                    }
                };
                let (dx, dy) = (tp.0 - sp.0, tp.1 - sp.1);
                let dist = (dx * dx + dy * dy).sqrt();
                let (bx, by) = (sp.0 - tp.0, sp.1 - tp.1);
                let bdist = (bx * bx + by * by).sqrt();
                let mut a = vec![
                    (
                        "class",
                        if l.dashed {
                            s("wardley-link wardley-link--dashed")
                        } else {
                            s("wardley-link")
                        },
                    ),
                    ("x1", jn(sp.0 + (dx / dist) * rad(si))),
                    ("y1", jn(sp.1 + (dy / dist) * rad(si))),
                    ("x2", jn(tp.0 + (bx / bdist) * rad(ti))),
                    ("y2", jn(tp.1 + (by / bdist) * rad(ti))),
                    ("stroke", css(&col.link)),
                    ("stroke-width", s("1")),
                ];
                if l.dashed {
                    a.push(("stroke-dasharray", s("6 6")));
                }
                if matches!(l.flow, Some(Flow::Forward | Flow::Bidirectional)) {
                    a.push(("marker-end", format!("url(#link-arrow-end-{id})")));
                }
                if matches!(l.flow, Some(Flow::Backward | Flow::Bidirectional)) {
                    a.push(("marker-start", format!("url(#link-arrow-start-{id})")));
                }
                open(o, "line", &a, true);
                if let Some(label) = l.label.as_ref().filter(|t| !t.is_empty()) {
                    let mid = ((sp.0 + tp.0) / 2.0, (sp.1 + tp.1) / 2.0);
                    let lx = mid.0 + (dy / dist) * 8.0;
                    let ly = mid.1 + (-dx / dist) * 8.0;
                    let mut angle = (dy.atan2(dx) * 180.0) / PI;
                    if !(-90.0..=90.0).contains(&angle) {
                        angle += 180.0;
                    }
                    leaf(
                        &mut labels,
                        "text",
                        &[
                            ("class", s("wardley-link-label")),
                            ("x", jn(lx)),
                            ("y", jn(ly)),
                            ("fill", css(&col.axis_text)),
                            ("font-size", jn(wc.label_font_size)),
                            ("text-anchor", s("middle")),
                            ("dominant-baseline", s("middle")),
                            (
                                "transform",
                                format!("rotate({} {} {})", jn(angle), jn(lx), jn(ly)),
                            ),
                        ],
                        label,
                    );
                }
            }
            o.push_str(&labels);
        });

        // Evolve trends.
        group(o, "wardley-trends", |o| {
            for t in &w.trends {
                let Some(i) = index(&t.node_id) else { continue };
                let origin = pos[i];
                let (tx, ty) = (px(t.target_x), py(t.target_y));
                let (dx, dy) = (tx - origin.0, ty - origin.1);
                let dist = (dx * dx + dy * dy).sqrt();
                let shorten = r + 2.0;
                let (ex, ey) = if dist > shorten {
                    (tx - (dx / dist) * shorten, ty - (dy / dist) * shorten)
                } else {
                    (tx, ty)
                };
                open(
                    o,
                    "line",
                    &[
                        ("class", s("wardley-trend")),
                        ("x1", jn(origin.0)),
                        ("y1", jn(origin.1)),
                        ("x2", jn(ex)),
                        ("y2", jn(ey)),
                        ("stroke", css(&col.evolution)),
                        ("stroke-width", s("1")),
                        ("stroke-dasharray", s("4 4")),
                        ("marker-end", format!("url(#arrow-{id})")),
                    ],
                    true,
                );
            }
        });

        // Nodes.
        group(o, "wardley-nodes", |o| {
            for (i, k) in w.nodes.iter().enumerate() {
                let (x, y) = pos[i];
                let class = format!("wardley-node wardley-node--{}", k.kind.class());
                group(o, &class, |o| {
                    let ring = |o: &mut String, cls: &str, fill: &str, stroke: String| {
                        open(
                            o,
                            "circle",
                            &[
                                ("class", s(cls)),
                                ("cx", jn(x)),
                                ("cy", jn(y)),
                                ("r", jn(r * 2.0)),
                                ("fill", s(fill)),
                                ("stroke", stroke),
                                ("stroke-width", s("1")),
                            ],
                            true,
                        );
                    };
                    match k.strategy {
                        Some(Strategy::Outsource) => ring(
                            o,
                            "wardley-outsource-overlay",
                            "#666",
                            css(&col.component_stroke),
                        ),
                        Some(Strategy::Buy) => {
                            ring(o, "wardley-buy-overlay", "#ccc", css(&col.component_stroke))
                        }
                        Some(Strategy::Build) => {
                            ring(o, "wardley-build-overlay", "#eee", s("#000"))
                        }
                        Some(Strategy::Market) => ring(
                            o,
                            "wardley-market-overlay",
                            "white",
                            css(&col.component_stroke),
                        ),
                        None => {}
                    }
                    if !k.pipeline_parent
                        && k.strategy != Some(Strategy::Market)
                        && k.kind != NodeKind::Anchor
                    {
                        open(
                            o,
                            "circle",
                            &[
                                ("cx", jn(x)),
                                ("cy", jn(y)),
                                ("r", jn(r)),
                                ("fill", css(&col.component_fill)),
                                ("stroke", css(&col.component_stroke)),
                                ("stroke-width", s("1")),
                            ],
                            true,
                        );
                    }
                    if k.strategy == Some(Strategy::Market) {
                        let tr = r * 1.2;
                        let (cos30, sin30) = ((PI / 6.0).cos(), (PI / 6.0).sin());
                        let top = (x, y - tr);
                        let bl = (x - tr * cos30, y + tr * sin30);
                        let br = (x + tr * cos30, y + tr * sin30);
                        for (a, b) in [(top, bl), (bl, br), (br, top)] {
                            open(
                                o,
                                "line",
                                &[
                                    ("class", s("wardley-market-line")),
                                    ("x1", jn(a.0)),
                                    ("y1", jn(a.1)),
                                    ("x2", jn(b.0)),
                                    ("y2", jn(b.1)),
                                    ("stroke", css(&col.component_stroke)),
                                    ("stroke-width", s("1")),
                                ],
                                true,
                            );
                        }
                        for d in [top, bl, br] {
                            open(
                                o,
                                "circle",
                                &[
                                    ("class", s("wardley-market-dot")),
                                    ("cx", jn(d.0)),
                                    ("cy", jn(d.1)),
                                    ("r", jn(r * 0.7)),
                                    ("fill", s("white")),
                                    ("stroke", css(&col.component_stroke)),
                                    ("stroke-width", s("2")),
                                ],
                                true,
                            );
                        }
                    }
                    if k.pipeline_parent {
                        open(
                            o,
                            "rect",
                            &[
                                ("x", jn(x - square / 2.0)),
                                ("y", jn(y - square / 2.0)),
                                ("width", jn(square)),
                                ("height", jn(square)),
                                ("fill", css(&col.component_fill)),
                                ("stroke", css(&col.component_stroke)),
                                ("stroke-width", s("1")),
                            ],
                            true,
                        );
                    }
                    if k.inertia {
                        let mut off = if k.pipeline_parent {
                            square / 2.0 + 15.0
                        } else {
                            r + 15.0
                        };
                        if k.strategy.is_some() {
                            off += r + 10.0;
                        }
                        let lh = if k.pipeline_parent { square } else { r * 2.0 };
                        open(
                            o,
                            "line",
                            &[
                                ("class", s("wardley-inertia")),
                                ("x1", jn(x + off)),
                                ("y1", jn(y - lh / 2.0)),
                                ("x2", jn(x + off)),
                                ("y2", jn(y + lh / 2.0)),
                                ("stroke", css(&col.component_stroke)),
                                ("stroke-width", s("6")),
                            ],
                            true,
                        );
                    }
                    let anchor = k.kind == NodeKind::Anchor;
                    let lx = if anchor {
                        k.label_offset_x.map_or(x, |d| x + d)
                    } else {
                        let mut d = wc.node_label_offset;
                        if k.strategy.is_some() && k.label_offset_x.is_none() {
                            d += 10.0;
                        }
                        x + k.label_offset_x.unwrap_or(d)
                    };
                    let ly = if anchor {
                        k.label_offset_y.map_or(y - 3.0, |d| y + d)
                    } else {
                        let mut d = -wc.node_label_offset;
                        if k.strategy.is_some() && k.label_offset_y.is_none() {
                            d -= 10.0;
                        }
                        y + k.label_offset_y.unwrap_or(d)
                    };
                    leaf(
                        o,
                        "text",
                        &[
                            ("x", jn(lx)),
                            ("y", jn(ly)),
                            ("class", s("wardley-node-label")),
                            (
                                "fill",
                                if anchor {
                                    s("#000")
                                } else {
                                    css(&col.component_label)
                                },
                            ),
                            ("font-size", jn(wc.label_font_size)),
                            ("font-weight", s(if anchor { "bold" } else { "normal" })),
                            ("text-anchor", s(if anchor { "middle" } else { "start" })),
                            (
                                "dominant-baseline",
                                s(if anchor { "middle" } else { "auto" }),
                            ),
                        ],
                        &k.label,
                    );
                });
            }
        });

        // Numbered annotations and their legend box.
        if !w.annotations.is_empty() {
            group(o, "wardley-annotations", |o| {
                for a in &w.annotations {
                    let (x, y) = (px(a.x), py(a.y));
                    group(o, "wardley-annotation", |o| {
                        open(
                            o,
                            "circle",
                            &[
                                ("cx", jn(x)),
                                ("cy", jn(y)),
                                ("r", s("10")),
                                ("fill", s("white")),
                                ("stroke", css(&col.axis)),
                                ("stroke-width", s("1.5")),
                            ],
                            true,
                        );
                        leaf(
                            o,
                            "text",
                            &[
                                ("x", jn(x)),
                                ("y", jn(y)),
                                ("text-anchor", s("middle")),
                                ("dominant-baseline", s("central")),
                                ("font-size", s("10")),
                                ("fill", css(&col.axis_text)),
                                ("font-weight", s("bold")),
                            ],
                            &a.number.to_string(),
                        );
                    });
                }
                if let Some((bx, by)) = w.annotations_box {
                    group(o, "wardley-annotations-box", |o| {
                        let mut sorted: Vec<&Annotation> = w
                            .annotations
                            .iter()
                            .filter(|a| !a.text.is_empty())
                            .collect();
                        sorted.sort_by_key(|a| a.number);
                        if sorted.is_empty() {
                            return;
                        }
                        let (bpad, lh, fs) = (10.0, 16.0, 11.0);
                        let lines: Vec<String> = sorted
                            .iter()
                            .map(|a| format!("{}. {}", a.number, a.text))
                            .collect();
                        let (max_w, max_h) = annotation_text_metrics(&lines, fs, wc, width, height);
                        let bw = max_w + bpad * 2.0 + 105.0;
                        let bh = sorted.len() as f64 * lh + bpad * 2.0 + max_h / 2.0;
                        let x = px(bx).min(width - pad - bw).max(pad);
                        let y = py(by).min(height - pad - bh).max(pad);
                        open(
                            o,
                            "rect",
                            &[
                                ("x", jn(x)),
                                ("y", jn(y)),
                                ("width", jn(bw)),
                                ("height", jn(bh)),
                                ("fill", s("white")),
                                ("stroke", css(&col.axis)),
                                ("stroke-width", s("1.5")),
                                ("rx", s("4")),
                                ("ry", s("4")),
                            ],
                            true,
                        );
                        for (k, l) in lines.iter().enumerate() {
                            leaf(
                                o,
                                "text",
                                &[
                                    ("x", jn(x + bpad)),
                                    ("y", jn(y + bpad + (k + 1) as f64 * lh)),
                                    ("font-size", jn(fs)),
                                    ("fill", css(&col.axis_text)),
                                    ("text-anchor", s("start")),
                                    ("dominant-baseline", s("middle")),
                                ],
                                l,
                            );
                        }
                    });
                }
            });
        }

        if !w.notes.is_empty() {
            group(o, "wardley-notes", |o| {
                for nt in &w.notes {
                    leaf(
                        o,
                        "text",
                        &[
                            ("x", jn(px(nt.x))),
                            ("y", jn(py(nt.y))),
                            ("text-anchor", s("start")),
                            ("font-size", s("11")),
                            ("fill", css(&col.axis_text)),
                            ("font-weight", s("bold")),
                        ],
                        &nt.text,
                    );
                }
            });
        }

        // Accelerator (right) and deaccelerator (left) arrows.
        let force = |o: &mut String, ms: &[Marker], right: bool| {
            for m in ms {
                let (x, y) = (px(m.x), py(m.y));
                let (aw, ah, head) = (60.0, 30.0, 20.0);
                let p = |a: f64, b: f64| format!("{} {}", jn(a), jn(b));
                let pts = if right {
                    [
                        p(x, y - ah / 2.0),
                        p(x + aw - head, y - ah / 2.0),
                        p(x + aw - head, y - ah / 2.0 - 8.0),
                        p(x + aw, y),
                        p(x + aw - head, y + ah / 2.0 + 8.0),
                        p(x + aw - head, y + ah / 2.0),
                        p(x, y + ah / 2.0),
                    ]
                } else {
                    [
                        p(x + aw, y - ah / 2.0),
                        p(x + head, y - ah / 2.0),
                        p(x + head, y - ah / 2.0 - 8.0),
                        p(x, y),
                        p(x + head, y + ah / 2.0 + 8.0),
                        p(x + head, y + ah / 2.0),
                        p(x + aw, y + ah / 2.0),
                    ]
                };
                // The template literal's own line breaks and indentation,
                // trimmed at both ends as mermaid's `.trim()` leaves it.
                let d = format!(
                    "M {}\n        L {}\n        L {}\n        L {}\n        L {}\n        L {}\n        L {}\n        Z",
                    pts[0], pts[1], pts[2], pts[3], pts[4], pts[5], pts[6]
                );
                open(
                    o,
                    "path",
                    &[
                        ("d", attr_esc(&d)),
                        ("fill", s("white")),
                        ("stroke", css(&col.component_stroke)),
                        ("stroke-width", s("1")),
                    ],
                    true,
                );
                leaf(
                    o,
                    "text",
                    &[
                        ("x", jn(x + aw / 2.0)),
                        ("y", jn(y + ah / 2.0 + 15.0)),
                        ("text-anchor", s("middle")),
                        ("font-size", s("10")),
                        ("fill", css(&col.axis_text)),
                        ("font-weight", s("bold")),
                    ],
                    &m.text,
                );
            }
        };
        if !w.accelerators.is_empty() {
            group(o, "wardley-accelerators", |o| {
                force(o, &w.accelerators, true)
            });
        }
        if !w.deaccelerators.is_empty() {
            group(o, "wardley-deaccelerators", |o| {
                force(o, &w.deaccelerators, false)
            });
        }
    });

    // Arrow markers.
    let marker = |o: &mut String,
                  mid: String,
                  ref_x: &str,
                  size: &str,
                  orient: &str,
                  d: &str,
                  fill: &Color| {
        open(
            o,
            "marker",
            &[
                ("id", mid),
                ("viewBox", s("0 0 10 10")),
                ("refX", s(ref_x)),
                ("refY", s("5")),
                ("markerWidth", s(size)),
                ("markerHeight", s(size)),
                ("orient", s(orient)),
            ],
            false,
        );
        open(
            o,
            "path",
            &[("d", s(d)), ("fill", css(fill)), ("stroke", s("none"))],
            true,
        );
        o.push_str("</marker>");
    };
    o.push_str("<defs>");
    marker(
        &mut o,
        format!("arrow-{id}"),
        "9",
        "6",
        "auto-start-reverse",
        "M 0 0 L 10 5 L 0 10 z",
        &col.evolution,
    );
    marker(
        &mut o,
        format!("link-arrow-end-{id}"),
        "9",
        "5",
        "auto",
        "M 0 0 L 10 5 L 0 10 z",
        &col.link,
    );
    marker(
        &mut o,
        format!("link-arrow-start-{id}"),
        "1",
        "5",
        "auto",
        "M 10 0 L 0 5 L 10 10 z",
        &col.link,
    );
    o.push_str("</defs>");
    o
}

/// `getComputedTextLength()` and `getBBox()` of the annotation box's lines,
/// as `(widest, tallest)` in svg units. They run while the texts still
/// inherit the container's "trebuchet ms", verdana, arial (Arial) family,
/// before they end up in the serif face they are drawn in, and at the svg's
/// screen scale: Chromium lays the text out at the screen size cut to 26.6
/// fixed point, its advance rounded up to 1/64 screen px, its box the rounded
/// ascent and descent. Both come back as single precision (`SVGRect` and
/// `getComputedTextLength` are `float`s).
fn annotation_text_metrics(
    lines: &[String],
    fs: f64,
    wc: &WardleyConfig,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let scale = if wc.use_max_width {
        crate::text::mmdc_page_scale(width, height)
    } else {
        1.0
    };
    let size = crate::text::shaping_size(fs * scale);
    let max_w = lines
        .iter()
        .map(|l| f64::from((crate::text::layout_unit_ceil(line_width(l, size, false)) / scale) as f32))
        .fold(0.0, f64::max);
    let max_h = lines
        .iter()
        .map(|l| f64::from((crate::text::svg_line_box(size, l) / scale) as f32))
        .fold(0.0, f64::max);
    (max_w, max_h)
}

/// Lay out and draw a parsed map.
pub fn draw(w: &Wardley, wc: &WardleyConfig, col: &WardleyColors) -> Scene {
    let (width, height) = w.size.unwrap_or((wc.width, wc.height));
    let pad = wc.padding;
    let r = wc.node_radius;
    let square = r * 1.6;
    let cw = width - pad * 2.0;
    let ch = height - pad * 2.0;
    let px = |v: f64| pad + v / 100.0 * cw;
    let py = |v: f64| height - pad - v / 100.0 * ch;
    let mut root = vec![Element::classed_shape(
        Geometry::Rect {
            rect: Rect::new(0.0, 0.0, width, height),
            rx: 0.0,
        },
        Paint::fill(&col.background),
        "wardley-background",
    )];

    if let Some(t) = w.title.as_ref().filter(|t| !t.is_empty()) {
        let fs = wc.axis_font_size * 1.05;
        let txt = text(
            width / 2.0,
            pad / 2.0,
            t,
            fs,
            MIDDLE,
            Anchor::Middle,
            &col.axis_text,
        )
        .bold();
        root.push(Element::classed_text(txt, "wardley-title"));
    }

    // Axes.
    let xlab = text(
        pad + cw / 2.0,
        height - pad / 4.0,
        "Evolution",
        wc.axis_font_size,
        crate::text::Baseline::Alphabetic,
        Anchor::Middle,
        &col.axis_text,
    )
    .bold();
    let yc = Point::new(pad / 3.0, pad + ch / 2.0);
    let ylab = text(
        yc.x,
        yc.y,
        "Visibility",
        wc.axis_font_size,
        crate::text::Baseline::Alphabetic,
        Anchor::Middle,
        &col.axis_text,
    )
    .bold();
    root.push(Element::group(
        "wardley-axes",
        vec![
            line(
                Point::new(pad, height - pad),
                Point::new(width - pad, height - pad),
                &col.axis,
                1.0,
            ),
            line(
                Point::new(pad, pad),
                Point::new(pad, height - pad),
                &col.axis,
                1.0,
            ),
            Element::classed_text(xlab, "wardley-axis-label wardley-axis-label-x"),
            Element::Rotated {
                deg: -90.0,
                about: yc,
                children: vec![Element::classed_text(
                    ylab,
                    "wardley-axis-label wardley-axis-label-y",
                )],
            },
        ],
    ));

    // Evolution stages.
    let stages: Vec<String> = if w.stages.is_empty() {
        DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
    } else {
        w.stages.clone()
    };
    let spans: Vec<(f64, f64)> = if w.stage_boundaries.len() == stages.len() {
        let mut prev = 0.0;
        w.stage_boundaries
            .iter()
            .map(|&b| {
                let s = (prev, b);
                prev = b;
                s
            })
            .collect()
    } else {
        let sw = 1.0 / stages.len() as f64;
        (0..stages.len())
            .map(|i| (i as f64 * sw, (i + 1) as f64 * sw))
            .collect()
    };
    let mut stage_els = Vec::new();
    for (i, (stage, (s, e))) in stages.iter().zip(&spans).enumerate() {
        let sx = pad + s * cw;
        let ex = pad + e * cw;
        if i > 0 {
            stage_els.push(Element::line(
                Point::new(sx, pad),
                Point::new(sx, height - pad),
                Paint::stroke(&Color::new("#000"), 1.0)
                    .dashed(vec![5.0, 5.0])
                    .with_opacity(0.8),
            ));
        }
        stage_els.push(Element::classed_text(
            text(
                (sx + ex) / 2.0,
                height - pad / 1.5,
                stage,
                wc.axis_font_size - 2.0,
                crate::text::Baseline::Alphabetic,
                Anchor::Middle,
                &col.axis_text,
            ),
            "wardley-stage-label",
        ));
    }
    root.push(Element::group("wardley-stages", stage_els));

    if wc.show_grid {
        let mut grid = Vec::new();
        for i in 1..4 {
            let ratio = i as f64 / 4.0;
            let x = pad + cw * ratio;
            let y = height - pad - ch * ratio;
            let paint = Paint::stroke(&col.grid, 1.0).dashed(vec![2.0, 6.0]);
            grid.push(Element::line(
                Point::new(x, pad),
                Point::new(x, height - pad),
                paint.clone(),
            ));
            grid.push(Element::line(
                Point::new(pad, y),
                Point::new(width - pad, y),
                paint,
            ));
        }
        root.push(Element::group("wardley-grid", grid));
    }

    // Projected positions (pipeline parents move onto their box below).
    let mut pos: Vec<Point> = w
        .nodes
        .iter()
        .map(|n| Point::new(px(n.x), py(n.y)))
        .collect();
    let index = |id: &str| w.nodes.iter().position(|n| n.id == id);

    if !w.pipelines.is_empty() {
        let mut boxes = Vec::new();
        let mut plinks = Vec::new();
        for p in &w.pipelines {
            let mut comps: Vec<usize> = p.component_ids.iter().filter_map(|c| index(c)).collect();
            if comps.is_empty() {
                continue;
            }
            comps.sort_by(|a, b| w.nodes[*a].x.total_cmp(&w.nodes[*b].x));
            for pair in comps.windows(2) {
                plinks.push(Element::classed_shape(
                    Geometry::Path(vec![
                        PathCmd::MoveTo(pos[pair[0]]),
                        PathCmd::LineTo(pos[pair[1]]),
                    ]),
                    Paint::stroke(&col.link, 1.0).dashed(vec![4.0, 4.0]),
                    "wardley-pipeline-evolution-link",
                ));
            }
            let min_x = comps
                .iter()
                .map(|&i| pos[i].x)
                .fold(f64::INFINITY, f64::min);
            let max_x = comps
                .iter()
                .map(|&i| pos[i].x)
                .fold(f64::NEG_INFINITY, f64::max);
            let y = p
                .component_ids
                .iter()
                .rev()
                .find_map(|c| index(c))
                .map(|i| pos[i].y)
                .unwrap_or(0.0);
            let bpad = 15.0;
            let bh = r * 4.0;
            let top = y - bh / 2.0;
            if let Some(pi) = index(&p.node_id) {
                pos[pi] = Point::new((min_x + max_x) / 2.0, top - square / 6.0);
            }
            boxes.push(Element::classed_shape(
                Geometry::Rect {
                    rect: Rect::new(min_x - bpad, top, max_x - min_x + bpad * 2.0, bh),
                    rx: 4.0,
                },
                Paint::stroke(&col.axis, 1.5),
                "wardley-pipeline-box",
            ));
        }
        root.push(Element::group("wardley-pipelines", boxes));
        root.push(Element::group("wardley-pipeline-links", plinks));
    }

    // Links (minus pipeline component → own parent).
    let mut link_lines = Vec::new();
    let mut link_labels = Vec::new();
    for l in &w.links {
        let (Some(si), Some(ti)) = (index(&l.source), index(&l.target)) else {
            continue;
        };
        if w.pipelines
            .iter()
            .any(|p| p.node_id == l.target && p.component_ids.contains(&l.source))
        {
            continue;
        }
        let (s, t) = (pos[si], pos[ti]);
        let rad = |i: usize| {
            if w.nodes[i].pipeline_parent {
                square / 2f64.sqrt()
            } else {
                r
            }
        };
        let dir = unit(s, t);
        let a = s.plus(dir.scale(rad(si)));
        let b = t.minus(dir.scale(rad(ti)));
        let mut paint = Paint::stroke(&col.link, 1.0);
        if l.dashed {
            paint = paint.dashed(vec![6.0, 6.0]);
        }
        let mut cls = "wardley-link".to_string();
        if l.dashed {
            cls.push_str(" wardley-link--dashed");
        }
        link_lines.push(Element::classed_shape(
            Geometry::Path(vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)]),
            paint,
            cls,
        ));
        let dir_ab = unit(a, b);
        if matches!(l.flow, Some(Flow::Forward | Flow::Bidirectional)) {
            link_lines.push(Element::path(
                marker(b, dir_ab, 0.5, 9.0, &ARROW_END),
                Paint::fill(&col.link),
            ));
        }
        if matches!(l.flow, Some(Flow::Backward | Flow::Bidirectional)) {
            link_lines.push(Element::path(
                marker(a, dir_ab, 0.5, 1.0, &ARROW_START),
                Paint::fill(&col.link),
            ));
        }
        if let Some(label) = l.label.as_ref().filter(|s| !s.is_empty()) {
            let mid = s.lerp(t, 0.5);
            let at = mid.plus(Point::new(dir.y, -dir.x).scale(8.0));
            let d = t.minus(s);
            let mut angle = d.y.atan2(d.x).to_degrees();
            if !(-90.0..=90.0).contains(&angle) {
                angle += 180.0;
            }
            link_labels.push(Element::Rotated {
                deg: angle,
                about: at,
                children: vec![Element::classed_text(
                    text(
                        at.x,
                        at.y,
                        label,
                        wc.label_font_size,
                        MIDDLE,
                        Anchor::Middle,
                        &col.axis_text,
                    ),
                    "wardley-link-label",
                )],
            });
        }
    }
    link_lines.extend(link_labels);
    root.push(Element::group("wardley-links", link_lines));

    // Evolve trends.
    let mut trends = Vec::new();
    for t in &w.trends {
        let Some(i) = index(&t.node_id) else { continue };
        let origin = pos[i];
        let target = Point::new(px(t.target_x), py(t.target_y));
        let dist = target.dist(origin);
        let shorten = r + 2.0;
        let dir = unit(origin, target);
        let end = if dist > shorten {
            target.minus(dir.scale(shorten))
        } else {
            target
        };
        trends.push(Element::classed_shape(
            Geometry::Path(vec![PathCmd::MoveTo(origin), PathCmd::LineTo(end)]),
            Paint::stroke(&col.evolution, 1.0).dashed(vec![4.0, 4.0]),
            "wardley-trend",
        ));
        trends.push(Element::path(
            marker(end, unit(origin, end), 0.6, 9.0, &ARROW_END),
            Paint::fill(&col.evolution),
        ));
    }
    root.push(Element::group("wardley-trends", trends));

    // Nodes.
    let mut nodes = Vec::new();
    for (i, n) in w.nodes.iter().enumerate() {
        let p = pos[i];
        let mut els = Vec::new();
        let overlay = |fill: &str, stroke: &Color, cls: &str| {
            Element::classed_shape(
                Geometry::Circle { c: p, r: r * 2.0 },
                Paint::both(&Color::new(fill), stroke, 1.0),
                cls,
            )
        };
        match n.strategy {
            Some(Strategy::Outsource) => els.push(overlay(
                "#666",
                &col.component_stroke,
                "wardley-outsource-overlay",
            )),
            Some(Strategy::Buy) => els.push(overlay(
                "#ccc",
                &col.component_stroke,
                "wardley-buy-overlay",
            )),
            Some(Strategy::Build) => els.push(overlay(
                "#eee",
                &Color::new("#000"),
                "wardley-build-overlay",
            )),
            Some(Strategy::Market) => els.push(overlay(
                "white",
                &col.component_stroke,
                "wardley-market-overlay",
            )),
            None => {}
        }
        if !n.pipeline_parent && n.strategy != Some(Strategy::Market) && n.kind != NodeKind::Anchor
        {
            els.push(circle(
                p,
                r,
                Paint::both(&col.component_fill, &col.component_stroke, 1.0),
            ));
        }
        if n.strategy == Some(Strategy::Market) {
            let tr = r * 1.2;
            let (s30, c30) = (std::f64::consts::PI / 6.0).sin_cos();
            let top = Point::new(p.x, p.y - tr);
            let bl = Point::new(p.x - tr * c30, p.y + tr * s30);
            let br = Point::new(p.x + tr * c30, p.y + tr * s30);
            for (a, b) in [(top, bl), (bl, br), (br, top)] {
                els.push(line(a, b, &col.component_stroke, 1.0));
            }
            for c in [top, bl, br] {
                els.push(Element::classed_shape(
                    Geometry::Circle { c, r: r * 0.7 },
                    Paint::both(&Color::new("white"), &col.component_stroke, 2.0),
                    "wardley-market-dot",
                ));
            }
        }
        if n.pipeline_parent {
            els.push(Element::rect(
                Rect::new(p.x - square / 2.0, p.y - square / 2.0, square, square),
                0.0,
                Paint::both(&col.component_fill, &col.component_stroke, 1.0),
            ));
        }
        if n.inertia {
            let mut off = if n.pipeline_parent {
                square / 2.0 + 15.0
            } else {
                r + 15.0
            };
            if n.strategy.is_some() {
                off += r + 10.0;
            }
            let lh = if n.pipeline_parent { square } else { r * 2.0 };
            els.push(Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(p.x + off, p.y - lh / 2.0)),
                    PathCmd::LineTo(Point::new(p.x + off, p.y + lh / 2.0)),
                ]),
                Paint::stroke(&col.component_stroke, 6.0),
                "wardley-inertia",
            ));
        }
        let label = if n.kind == NodeKind::Anchor {
            text(
                p.x + n.label_offset_x.unwrap_or(0.0),
                p.y + n.label_offset_y.unwrap_or(-3.0),
                &n.label,
                wc.label_font_size,
                MIDDLE,
                Anchor::Middle,
                &Color::new("#000"),
            )
            .bold()
        } else {
            let extra = if n.strategy.is_some() { 10.0 } else { 0.0 };
            text(
                p.x + n.label_offset_x.unwrap_or(wc.node_label_offset + extra),
                p.y + n.label_offset_y.unwrap_or(-wc.node_label_offset - extra),
                &n.label,
                wc.label_font_size,
                crate::text::Baseline::Alphabetic,
                Anchor::Start,
                &col.component_label,
            )
        };
        els.push(Element::classed_text(label, "wardley-node-label"));
        nodes.push(Element::group(
            format!("wardley-node wardley-node--{}", n.kind.class()),
            els,
        ));
    }
    root.push(Element::group("wardley-nodes", nodes));

    // Numbered annotations and their legend box.
    if !w.annotations.is_empty() {
        let mut ann = Vec::new();
        for a in &w.annotations {
            let c = Point::new(px(a.x), py(a.y));
            ann.push(Element::group(
                "wardley-annotation",
                vec![
                    circle(c, 10.0, Paint::both(&Color::new("white"), &col.axis, 1.5)),
                    Element::text(
                        text(
                            c.x,
                            c.y,
                            &a.number.to_string(),
                            10.0,
                            CENTRAL,
                            Anchor::Middle,
                            &col.axis_text,
                        )
                        .bold(),
                    ),
                ],
            ));
        }
        if let Some((bx, by)) = w.annotations_box {
            let mut sorted: Vec<&Annotation> = w
                .annotations
                .iter()
                .filter(|a| !a.text.is_empty())
                .collect();
            sorted.sort_by_key(|a| a.number);
            if !sorted.is_empty() {
                let (bpad, lh, fs) = (10.0, 16.0, 11.0);
                let lines: Vec<String> = sorted
                    .iter()
                    .map(|a| format!("{}. {}", a.number, a.text))
                    .collect();
                let (max_w, max_h) = annotation_text_metrics(&lines, fs, wc, width, height);
                let bw = max_w + bpad * 2.0 + 105.0;
                let bh = sorted.len() as f64 * lh + bpad * 2.0 + max_h / 2.0;
                let x = px(bx).min(width - pad - bw).max(pad);
                let y = py(by).min(height - pad - bh).max(pad);
                let mut els = vec![Element::rect(
                    Rect::new(x, y, bw, bh),
                    4.0,
                    Paint::both(&Color::new("white"), &col.axis, 1.5),
                )];
                for (k, l) in lines.iter().enumerate() {
                    els.push(Element::text(text(
                        x + bpad,
                        y + bpad + (k + 1) as f64 * lh,
                        l,
                        fs,
                        MIDDLE,
                        Anchor::Start,
                        &col.axis_text,
                    )));
                }
                ann.push(Element::group("wardley-annotations-box", els));
            }
        }
        root.push(Element::group("wardley-annotations", ann));
    }

    if !w.notes.is_empty() {
        let notes = w
            .notes
            .iter()
            .map(|n| {
                Element::text(
                    text(
                        px(n.x),
                        py(n.y),
                        &n.text,
                        11.0,
                        crate::text::Baseline::Alphabetic,
                        Anchor::Start,
                        &col.axis_text,
                    )
                    .bold(),
                )
            })
            .collect();
        root.push(Element::group("wardley-notes", notes));
    }

    // Accelerator (right) and deaccelerator (left) arrows.
    let force = |m: &Marker, right: bool| {
        let (x, y) = (px(m.x), py(m.y));
        let (aw, ah, head) = (60.0, 30.0, 20.0);
        // Local outline for a right-pointing arrow; mirrored for deaccelerators.
        let local = [
            (0.0, -ah / 2.0),
            (aw - head, -ah / 2.0),
            (aw - head, -ah / 2.0 - 8.0),
            (aw, 0.0),
            (aw - head, ah / 2.0 + 8.0),
            (aw - head, ah / 2.0),
            (0.0, ah / 2.0),
        ];
        let pts: Vec<Point> = local
            .iter()
            .map(|&(dx, dy)| Point::new(if right { x + dx } else { x + aw - dx }, y + dy))
            .collect();
        vec![
            Element::path(
                crate::geom::polygon(&pts),
                Paint::both(&Color::new("white"), &col.component_stroke, 1.0),
            ),
            Element::text(
                text(
                    x + aw / 2.0,
                    y + ah / 2.0 + 15.0,
                    &m.text,
                    10.0,
                    crate::text::Baseline::Alphabetic,
                    Anchor::Middle,
                    &col.axis_text,
                )
                .bold(),
            ),
        ]
    };
    if !w.accelerators.is_empty() {
        let els = w.accelerators.iter().flat_map(|m| force(m, true)).collect();
        root.push(Element::group("wardley-accelerators", els));
    }
    if !w.deaccelerators.is_empty() {
        let els = w
            .deaccelerators
            .iter()
            .flat_map(|m| force(m, false))
            .collect();
        root.push(Element::group("wardley-deaccelerators", els));
    }

    Scene {
        kind: DiagramKind::Wardley,
        view_box: Rect::new(0.0, 0.0, width, height),
        elements: vec![Element::Markup {
            svg: markup(w, wc, col),
            children: vec![Element::group("wardley-map", root)],
        }],
        acc_title: None,
        acc_descr: None,
        font_family: FONT_FAMILY.to_string(),
        css: String::new(),
    }
}

/// Parse, lay out and draw a `wardley-beta` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let w = parse(body, title)?;
    Ok(draw(
        &w,
        &WardleyConfig::from_config(cfg),
        &colors(cfg, pal),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Wardley {
        parse(src, None).unwrap()
    }

    fn node<'a>(w: &'a Wardley, label: &str) -> &'a Node {
        w.nodes.iter().find(|n| n.label == label).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn components_links_and_title() {
        let w = p(
            "wardley-beta\ntitle Example\ncomponent Alpha [0.2, 0.1]\ncomponent Beta [0.4, 0.3]\nAlpha -> Beta",
        );
        assert_eq!(w.nodes.len(), 2);
        assert_eq!(w.links.len(), 1);
        assert_eq!(w.title.as_deref(), Some("Example"));
        let a = node(&w, "Alpha");
        assert!(close(a.x, 10.0) && close(a.y, 20.0));
    }

    #[test]
    fn header_is_case_insensitive_and_frontmatter_title_is_fallback() {
        let w = parse("Wardley-Beta\ncomponent A [0.5, 0.5]", Some("FM".into())).unwrap();
        assert_eq!(w.title.as_deref(), Some("FM"));
        assert!(parse("wardley\ncomponent A [0.5, 0.5]", None).is_err());
    }

    #[test]
    fn evolution_stages() {
        let w = p("wardley-beta\nevolution Genesis -> Custom -> Product -> Commodity");
        assert_eq!(w.stages, ["Genesis", "Custom", "Product", "Commodity"]);
        let w = p(
            "wardley-beta\nevolution Genesis / Concept -> Custom / Emerging -> Product / Converging -> Commodity / Accepted",
        );
        assert_eq!(
            w.stages,
            [
                "Genesis / Concept",
                "Custom / Emerging",
                "Product / Converging",
                "Commodity / Accepted"
            ]
        );
        let w =
            p("wardley-beta\nevolution Genesis@0.3 -> Custom@0.6 -> Product@0.85 -> Commodity@1.0");
        assert_eq!(w.stages, ["Genesis", "Custom", "Product", "Commodity"]);
        assert_eq!(w.stage_boundaries, [0.3, 0.6, 0.85, 1.0]);
    }

    #[test]
    fn pipelines_inherit_visibility() {
        let w = p("wardley-beta
component Kettle [0.45, 0.57]
pipeline Kettle {
  component Campfire Kettle [0.35] label [-60, 35]
  component Electric Kettle [0.53]
}");
        let k = node(&w, "Kettle");
        let c = node(&w, "Campfire Kettle");
        let e = node(&w, "Electric Kettle");
        assert!(k.pipeline_parent);
        assert_eq!(c.id, "Kettle_Campfire Kettle");
        assert!(c.in_pipeline && c.kind == NodeKind::PipelineComponent);
        assert!(close(c.y, k.y) && close(e.y, k.y));
        assert!(close(c.x, 35.0) && close(e.x, 53.0));
        assert_eq!(
            (c.label_offset_x, c.label_offset_y),
            (Some(-60.0), Some(35.0))
        );
        assert_eq!(w.pipelines[0].component_ids.len(), 2);
    }

    #[test]
    fn pipeline_needs_existing_parent() {
        let e = parse(
            "wardley-beta\npipeline Nope {\n  component A [0.3]\n}",
            None,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("must reference an existing component")
        );
    }

    #[test]
    fn notes_annotations_anchors_and_evolve() {
        let w = p("wardley-beta
anchor Business [0.95, 0.63]
anchor Public [0.95, 0.78]
component Kettle [0.35, 0.43]
evolve Kettle 0.62
note \"Critical decision point\" [0.65, 0.55]
annotations [0.1, 0.9]
annotation 1,[0.6, 0.65] \"Critical component\"
annotation 2,[0.5, 0.5] \"Performance layer\"");
        let anchors: Vec<_> = w
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Anchor)
            .collect();
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].label, "Business");
        assert_eq!(w.trends.len(), 1);
        assert!(close(w.trends[0].target_x, 62.0));
        assert_eq!(w.notes[0].text, "Critical decision point");
        assert_eq!(w.annotations.len(), 2);
        assert_eq!(w.annotations[1].text, "Performance layer");
        assert_eq!(w.annotations_box, Some((90.0, 10.0)));
    }

    #[test]
    fn decorators_size_flows_and_coordinates() {
        let w = p("wardley-beta
title Coordinate Handling
size [1200, 900]
component \"Mobile App\" [0.2, 0.4] (build) (inertia)
component API [0.3, 0.5] (buy)
component Cache [0.5, 0.6] (outsource)
component Cloud [0.5, 0.6] (market) inertia
\"Mobile App\" +<> API; constraint
note \"Check dependencies\" [0.25, 0.45]
annotations [0.10, 0.90]
annotation 1,[0.60, 0.65] \"Critical component\"
accelerator \"Cloud Native\" [0.20, 0.85]
deaccelerator \"Legacy Data\" [0.40, 0.35]");
        assert_eq!(w.size, Some((1200.0, 900.0)));
        let m = node(&w, "Mobile App");
        assert!(close(m.x, 40.0) && close(m.y, 20.0));
        assert_eq!(m.strategy, Some(Strategy::Build));
        assert!(m.inertia);
        assert_eq!(node(&w, "API").strategy, Some(Strategy::Buy));
        assert_eq!(node(&w, "Cache").strategy, Some(Strategy::Outsource));
        assert!(node(&w, "Cloud").inertia);
        assert_eq!(w.links[0].flow, Some(Flow::Bidirectional));
        assert_eq!(w.links[0].label.as_deref(), Some("constraint"));
        assert!(close(w.notes[0].x, 45.0) && close(w.notes[0].y, 25.0));
        assert!(close(w.annotations[0].x, 65.0) && close(w.annotations[0].y, 60.0));
        assert_eq!(w.accelerators[0].text, "Cloud Native");
        assert!(close(w.accelerators[0].x, 85.0) && close(w.accelerators[0].y, 20.0));
        assert!(close(w.deaccelerators[0].x, 35.0) && close(w.deaccelerators[0].y, 40.0));
    }

    #[test]
    fn link_arrow_variants() {
        let w = p("wardley-beta
component A [0.1, 0.1]
component B [0.2, 0.2]
A->B
A --> B
A -.-> B
A +> B
A +< B
A +'backup'> B; ignored
A > B; note");
        let l = &w.links;
        assert_eq!(l.len(), 7);
        assert!(l.iter().all(|l| l.source == "A" && l.target == "B"));
        assert_eq!(l[0].flow, None);
        assert!(l[2].dashed && !l[1].dashed);
        assert_eq!(l[3].flow, Some(Flow::Forward));
        assert_eq!(l[4].flow, Some(Flow::Backward));
        assert_eq!(l[5].flow, Some(Flow::Forward));
        assert_eq!(l[5].label.as_deref(), Some("backup"));
        assert_eq!(l[6].label.as_deref(), Some("note"));
    }

    #[test]
    fn hyphenated_names() {
        let w = p("wardley-beta
component real-time processing [0.5, 0.5]
component end-user [0.8, 0.9]
component foo--bar [0.3, 0.4]
component foo- [0.2, 0.3]
anchor on-call engineer [0.9, 0.95]
real-time processing -> end-user
foo--bar->end-user");
        assert!(close(node(&w, "end-user").x, 90.0));
        assert!(close(node(&w, "foo--bar").y, 30.0));
        assert!(close(node(&w, "foo-").x, 30.0));
        assert_eq!(node(&w, "on-call engineer").kind, NodeKind::Anchor);
        assert_eq!(w.links[0].source, "real-time processing");
        assert_eq!(w.links[1].source, "foo--bar");
        assert_eq!(w.links[1].target, "end-user");
    }

    #[test]
    fn links_resolve_pipeline_labels() {
        let w = p("wardley-beta
component Data Store [0.5, 0.5]
component User [0.9, 0.9]
pipeline Data Store {
  component real-time queue [0.3] label [-40, 20]
  component batch-loader [0.7]
}
User -> batch-loader");
        assert_eq!(w.links[0].target, "Data Store_batch-loader");
        assert_eq!(node(&w, "real-time queue").label_offset_x, Some(-40.0));
    }

    #[test]
    fn errors() {
        // Integers are not valid component coordinates (WARDLEY_NUMBER is decimal-only).
        assert!(parse("wardley-beta\ncomponent A [1, 0.5]", None).is_err());
        assert!(parse("wardley-beta\nnote unquoted [0.1, 0.2]", None).is_err());
        let e = parse("wardley-beta\ncomponent A [0.5, 150.0]", None).unwrap_err();
        assert!(e.to_string().contains("must be between 0-1"), "{e}");
        assert!(e.to_string().contains("line 2"), "{e}");
        assert!(parse("wardley-beta\ncomponent A [0.5, 0.5] $", None).is_err());
        assert!(parse("wardley-beta\npipeline A {\n component B [0.2]", None).is_err());
    }

    #[test]
    fn comments_and_redeclaration() {
        let w = p("wardley-beta
component A [0.3, 0.2] label [5, 5] %% trailing comment
component A [0.6, 0.7]");
        assert_eq!(w.nodes.len(), 1);
        let a = node(&w, "A");
        assert!(close(a.x, 70.0));
        assert_eq!(a.label_offset_x, Some(5.0));
    }

    fn scene_elements(src: &str) -> (Scene, Vec<Element>) {
        fn flatten(els: &[Element], out: &mut Vec<Element>) {
            for e in els {
                match e {
                    Element::Group { children, .. }
                    | Element::Markup { children, .. }
                    | Element::Rotated { children, .. } => flatten(children, out),
                    other => out.push(other.clone()),
                }
            }
        }
        let cfg = MermaidConfig::default();
        let s = render(src, None, &cfg, &cfg.palette()).unwrap();
        let mut els = Vec::new();
        flatten(&s.elements, &mut els);
        (s, els)
    }

    fn rects(els: &[Element]) -> Vec<(Rect, f64, Option<String>)> {
        els.iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, rx },
                    class,
                    ..
                } => Some((*rect, *rx, class.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn projection_matches_upstream_geometry() {
        let (s, els) = scene_elements("wardley-beta\ncomponent A [0.5, 0.25]");
        assert_eq!(s.view_box, Rect::new(0.0, 0.0, 900.0, 600.0));
        // padding 48: x = 48 + 0.25*804, y = 600 - 48 - 0.5*504.
        assert!(els.iter().any(|e| matches!(e,
            Element::Shape { geom: Geometry::Circle { c, r }, .. }
                if close(c.x, 249.0) && close(c.y, 300.0) && close(*r, 6.0))));
        // Default stages: three dividers.
        let dividers = els
            .iter()
            .filter(
                |e| matches!(e, Element::Shape { paint, .. } if paint.dash == Some(vec![5.0, 5.0])),
            )
            .count();
        assert_eq!(dividers, 3);
    }

    #[test]
    fn pipeline_parent_moves_onto_box() {
        let (_, els) = scene_elements(
            "wardley-beta
size [1000, 600]
component K [0.5, 0.5]
pipeline K {
  component A [0.2]
  component B [0.6]
}",
        );
        let rs = rects(&els);
        // Chart 904 wide: A at 48+180.8, B at 48+542.4; a 24px box around y=300.
        let (bx, rx, _) = rs
            .iter()
            .find(|r| r.2.as_deref() == Some("wardley-pipeline-box"))
            .unwrap();
        assert!(close(bx.x, 228.8 - 15.0) && close(bx.y, 288.0) && close(*rx, 4.0));
        assert!(close(bx.w, 361.6 + 30.0) && close(bx.h, 24.0));
        // The parent square sits on the box top, 2/3 outside.
        let (sq, _, _) = rs.iter().find(|r| close(r.0.w, 9.6)).unwrap();
        assert!(close(sq.center().x, 409.6));
        assert!(close(sq.center().y, 288.0 - 1.6));
    }

    #[test]
    fn annotations_box_is_clamped_inside_the_map() {
        let (_, els) = scene_elements(
            "wardley-beta
annotations [0.0, 1.0]
annotation 1,[0.5, 0.5] \"First\"",
        );
        let (r, _, _) = rects(&els).into_iter().find(|r| close(r.1, 4.0)).unwrap();
        assert!(r.right() <= 900.0 - 48.0 + 1e-9);
        assert!(r.bottom() <= 600.0 - 48.0 + 1e-9);
    }

    #[test]
    fn annotations_box_is_measured_in_the_containers_font() {
        // mermaid 12.3.0 (the gallery map, 1100 × 800): the box is 219.6425 wide,
        // the widest line's getComputedTextLength() in Arial (the texts are drawn
        // in Times later) plus 2·10 padding and 105. Chromium measures it at the
        // svg's screen scale (784px page): laid out at 7.828125px, its advance
        // rounded up to 1/64 screen px; the plain Arial advance is 94.773. The
        // box is 74.3139 tall: the line box is 9 screen px, 12.628 units.
        let (_, els) = scene_elements(
            "wardley-beta
size [1100, 800]
annotations [0.10, 0.20]
annotation 1,[0.78, 0.82] \"User touchpoints\"
annotation 2,[0.70, 0.60] \"Integration layer\"
annotation 3,[0.50, 0.40] \"Data persistence\"",
        );
        let (r, _, _) = rects(&els).into_iter().find(|r| close(r.1, 4.0)).unwrap();
        assert!((r.w - 219.642_517).abs() < 1e-3, "{r:?}");
        assert!((r.h - 74.313_886).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn theme_colours() {
        let pal = MermaidConfig::default().palette();
        let mut cfg = MermaidConfig::default();
        let c = colors(&cfg, &pal);
        assert_eq!(c.axis_text.css(), "#131300");
        assert_eq!(c.axis.css(), "#333333");
        cfg.theme = ThemeName::Dark;
        let c = colors(&cfg, &pal);
        assert_eq!(c.component_fill.css(), "#1f2020");
        assert_eq!(c.evolution.css(), "#ff6b6b");
        cfg.theme = ThemeName::Redux;
        let c = colors(&cfg, &pal);
        assert_eq!(c.axis.css(), "#000");
        assert_eq!(c.axis_text.css(), "#28253D");
    }
}
