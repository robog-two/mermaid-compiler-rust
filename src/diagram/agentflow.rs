//! `agentflow-beta` diagrams (mermaid 12.3 `diagrams/agentflow`).
//!
//! Agentflow is flowchart's grammar with an agent-oriented vocabulary: `flow … end`
//! containers (nesting to any depth), `global … end` blocks, `connector` declarations,
//! three edge operators (`-->` sequence, `-.-` reference, `--x` failure) and six domain
//! shapes chosen with `@{ shape: … }` (`task`, `tool`, `input`, `decision`, `refdoc`,
//! `action`). Anything else is coerced to the rounded `task` rectangle, as upstream.
//!
//! The parser mirrors `agentflow.jison` + `agentflowDb.ts` into a typed [`Agentflow`]
//! model, including the containment rules (a node belongs to the innermost flow that
//! mentions it first, `global` nodes never belong to a flow, containment cycles are
//! broken) and `@{ view: collapsed }`. [`to_graph`] is the pure equivalent of
//! `getData()`: it turns the model into the shared [`GraphDiagram`] that the
//! flowchart pipeline lays out and draws.

use super::graph::{
    self, Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId,
    RenderOptions, Shape, StyleOverrides,
};
use super::timeline::wrap_text;
use crate::config::{GraphConfig, MermaidConfig};
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::{DiagramKind, Scene};
use crate::theme::{ColorSlot, Palette};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------- model

/// Derived semantic kind of a vertex (`deriveVertexKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexKind {
    Tool,
    Task,
    Decision,
    Input,
    Refdoc,
    Connector,
    Action,
}

impl VertexKind {
    /// `colorSlots.ts` KIND_SLOT: one fixed palette slot per kind.
    fn slot(self) -> usize {
        match self {
            VertexKind::Tool => 0,
            VertexKind::Task => 1,
            VertexKind::Decision => 2,
            VertexKind::Input => 3,
            VertexKind::Refdoc => 4,
            VertexKind::Connector => 5,
            VertexKind::Action => 6,
        }
    }
}

/// Slots reserved for kinds; containers cycle the ones above (`KIND_COUNT`).
const KIND_COUNT: usize = 7;

/// The rendered shape of a vertex after alias resolution and `normaliseNodeShapes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfShape {
    /// `roundedRect` (`task`, and every removed / unsupported shape).
    RoundedRect,
    /// `subroutine` (`tool`).
    Subroutine,
    /// `lean-right` (`input`).
    LeanRight,
    /// `diamond` (`decision`).
    Diamond,
    /// `lin-doc` (`refdoc`).
    LinedDocument,
    /// `hexagon` (`action`).
    Hexagon,
}

/// §5.1 edge semantic of the three operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSemantic {
    /// `-->`
    Sequence,
    /// `-.-`
    Reference,
    /// `--x`
    Failure,
}

/// Arrow type at the end of an edge (`destructEndLink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowType {
    Point,
    Cross,
    Open,
    /// `-- text -.-`-style mixes; drawn as a plain arrow, like upstream.
    Invalid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    pub id: String,
    /// Label as authored (quotes stripped; markdown strings keep their backticks).
    pub label: String,
    /// Authored type: an inline bracket form (`square`, `diamond`, …) or the
    /// alias-resolved `@{ shape }`.
    pub ty: Option<String>,
    pub is_connector: bool,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
    /// Every other `@{ … }` key, carried through untouched.
    pub metadata: Vec<(String, String)>,
}

impl Vertex {
    /// `deriveVertexKind`.
    pub fn kind(&self) -> VertexKind {
        if self.is_connector {
            return VertexKind::Connector;
        }
        let ty = self.ty.as_deref().unwrap_or("");
        if SUBROUTINE_ALIASES.contains(&ty) {
            return VertexKind::Tool;
        }
        match resolve_alias(ty) {
            "hexagon" | "hex" => VertexKind::Action,
            "lean-right" | "lean_right" => VertexKind::Input,
            "lin-doc" | "lined-document" => VertexKind::Refdoc,
            "diamond" => VertexKind::Decision,
            _ => VertexKind::Task,
        }
    }

    /// `getTypeFromVertex` + `normaliseNodeShapes`.
    pub fn shape(&self) -> AfShape {
        match self.ty.as_deref().map(resolve_alias) {
            Some("subroutine" | "subprocess" | "subproc" | "framed-rectangle") => {
                AfShape::Subroutine
            }
            Some("lean-right") => AfShape::LeanRight,
            Some("diamond") => AfShape::Diamond,
            Some("lin-doc" | "lined-document") => AfShape::LinedDocument,
            Some("hexagon" | "hex") => AfShape::Hexagon,
            _ => AfShape::RoundedRect,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AfEdge {
    pub start: String,
    pub end: String,
    pub id: Option<String>,
    pub label: Option<String>,
    pub arrow: ArrowType,
    pub dotted: bool,
    /// Operator length (extra dashes / dots), capped at 10.
    pub length: usize,
    pub semantic: Option<EdgeSemantic>,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
    pub metadata: Vec<(String, String)>,
}

/// A `flow … end` container.
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub id: String,
    pub title: String,
    /// Direct members (vertex, connector and flow ids), after the uniqueness rules.
    pub nodes: Vec<String>,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
    pub metadata: Vec<(String, String)>,
    /// The flow's own `direction` statement (a repeated id takes the last one given).
    pub direction: Option<Direction>,
}

impl Flow {
    pub fn collapsed(&self) -> bool {
        meta_get(&self.metadata, "view") == Some("collapsed")
    }
}

/// A parsed agentflow diagram (the state of `AgentFlowDB` after parsing).
#[derive(Debug, Clone, PartialEq)]
pub struct Agentflow {
    pub direction: Direction,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<AfEdge>,
    /// In completion order: a nested flow precedes the flow that holds it.
    pub flows: Vec<Flow>,
    pub globals: HashSet<String>,
    pub title: Option<String>,
}

fn meta_get<'a>(meta: &'a [(String, String)], key: &str) -> Option<&'a str> {
    meta.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------- shapes

/// §4.3.2 aliases (`SHAPE_ALIASES`).
fn resolve_alias(s: &str) -> &str {
    match s {
        "task" => "roundedRect",
        "tool" => "subroutine",
        "input" | "lean_right" => "lean-right",
        "decision" => "diamond",
        "refdoc" => "lin-doc",
        "action" => "hexagon",
        "round" => "rect",
        other => other,
    }
}

/// `SUBROUTINE_ALIASES`: membership makes a vertex a tool.
const SUBROUTINE_ALIASES: [&str; 5] = [
    "subroutine",
    "subprocess",
    "subproc",
    "framed-rectangle",
    "tool",
];

/// mermaid's `isValidShape`: every registered shape name.
fn is_valid_shape(s: &str) -> bool {
    super::flowchart::shape_by_name(s).is_some()
        || s.starts_with("bpmn-")
        || s.starts_with("usecase")
        || matches!(s, "connector")
}

// ---------------------------------------------------------------- parser

/// Parse a preprocessed `agentflow-beta` body.
pub fn parse(body: &str, title: Option<String>) -> Result<Agentflow> {
    let stmts = statements(body);
    let mut it = stmts.iter().peekable();
    let mut direction = Direction::TopBottom;
    // Header: `agentflow-beta [DIR]`.
    match it.next() {
        Some((line, s)) => {
            let rest = s
                .strip_prefix("agentflow-beta")
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .ok_or_else(|| Error::parse(*line, format!("{s}\nExpecting 'agentflow-beta'")))?;
            let d = rest.trim();
            if !d.is_empty() {
                direction = match d {
                    "TB" | "TD" | "v" => Direction::TopBottom,
                    "BT" | "^" => Direction::BottomTop,
                    "LR" | ">" => Direction::LeftRight,
                    "RL" | "<" => Direction::RightLeft,
                    _ => {
                        return Err(Error::parse(
                            *line,
                            format!("{s}\nInvalid direction '{d}', expecting TB, TD, BT, RL or LR"),
                        ));
                    }
                };
            }
        }
        None => return Err(Error::parse(1, "Expecting 'agentflow-beta'")),
    }
    let mut db = Db::default();
    let rest: Vec<(usize, String)> = it.cloned().collect();
    let mut pos = 0;
    db.document(&rest, &mut pos, Block::Top)?;
    db.apply_styles()?;
    Ok(Agentflow {
        direction,
        vertices: db.vertices,
        edges: db.edges,
        flows: db.flows,
        globals: db.globals,
        title,
    })
}

/// Split the body into statements: newlines and `;` end a statement, except inside
/// quotes or an `@{ … }` block (which may span lines); `%%` starts a comment.
fn statements(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start_line = 1;
    let mut line = 1;
    let mut in_quote = false;
    let mut in_meta = false;
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    let mut flush = |cur: &mut String, start_line: usize| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push((start_line, t.to_string()));
        }
        cur.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        if cur.trim().is_empty() {
            start_line = line;
        }
        match c {
            '"' => in_quote = !in_quote,
            '@' if !in_quote && chars.get(i + 1) == Some(&'{') => in_meta = true,
            '}' if !in_quote && in_meta => in_meta = false,
            '%' if !in_quote && !in_meta && chars.get(i + 1) == Some(&'%') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '\n' | ';' if !in_meta && !(in_quote && c == ';') => {
                if c == '\n' {
                    line += 1;
                    in_quote = false;
                }
                flush(&mut cur, start_line);
                i += 1;
                continue;
            }
            _ => {}
        }
        if c == '\n' {
            line += 1;
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, start_line);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Top,
    Flow,
    Global,
}

/// Link token (`-->`, `--x`, `-.-`, `-- text -->`, `-->|text|`, `e1@-->`).
#[derive(Debug, Clone, PartialEq)]
struct Link {
    arrow: ArrowType,
    dotted: bool,
    length: usize,
    text: Option<String>,
    id: Option<String>,
}

/// One `@{ … }` value.
#[derive(Debug, Clone, PartialEq)]
struct MetaValue {
    text: String,
    /// Quoted, or an unquoted value YAML reads as a string (not a number/bool/null).
    is_string: bool,
}

/// Parsing state, mirroring `AgentFlowDB`.
#[derive(Default)]
struct Db {
    vertices: Vec<Vertex>,
    index: HashMap<String, usize>,
    edges: Vec<AfEdge>,
    flows: Vec<Flow>,
    flow_index: HashMap<String, usize>,
    globals: HashSet<String>,
    sub_count: usize,
    /// One slot per open `flow` / `global` block: its last `direction` statement.
    dir_stack: Vec<Option<Direction>>,
    class_defs: HashMap<String, StyleOverrides>,
    link_styles: Vec<(usize, Option<Vec<usize>>, StyleOverrides)>,
}

fn err(line: usize, stmt: &str, col: usize, msg: &str) -> Error {
    Error::parse(line, format!("{stmt}\n{}^\n{msg}", "-".repeat(col)))
}

fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

impl Db {
    /// Parse statements until the block's `end` (or the end of input at top level).
    /// Returns the ids the block mentions, as the grammar's `document` does.
    fn document(
        &mut self,
        stmts: &[(usize, String)],
        pos: &mut usize,
        block: Block,
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        while *pos < stmts.len() {
            let (line, stmt) = (&stmts[*pos].0, stmts[*pos].1.as_str());
            *pos += 1;
            let (kw, rest) = first_word(stmt);
            match kw {
                "end" if rest.is_empty() => {
                    if block == Block::Top {
                        return Err(err(
                            *line,
                            stmt,
                            0,
                            "Unexpected 'end' without matching 'flow' or 'global'",
                        ));
                    }
                    return Ok(ids);
                }
                "flow" => ids.push(self.flow(stmts, pos, *line, stmt, rest)?),
                "global" if rest.is_empty() => {
                    // A `direction` inside `global … end` belongs to no flow.
                    self.dir_stack.push(None);
                    let list = self.document(stmts, pos, Block::Global);
                    self.dir_stack.pop();
                    self.add_global(&list?);
                }
                "connector" => ids.push(self.connector(*line, stmt, rest)?),
                "direction" => {
                    // Only a flow's own `direction` matters (the last one wins).
                    if let (Some(slot), Some(d)) =
                        (self.dir_stack.last_mut(), Direction::parse(rest))
                    {
                        *slot = Some(d);
                    }
                }
                "classDef" => {
                    let (names, style) = first_word(rest);
                    let st = StyleOverrides::parse(style);
                    for n in names.split(',') {
                        let e = self.class_defs.entry(n.trim().to_string()).or_default();
                        *e = e.merged(&st);
                    }
                }
                "class" => {
                    let (targets, class) = first_word(rest);
                    for id in targets.split(',') {
                        self.set_class(id.trim(), class.trim());
                    }
                }
                "style" => {
                    let (id, style) = first_word(rest);
                    let st = StyleOverrides::parse(style);
                    if let Some(&f) = self.flow_index.get(id) {
                        self.flows[f].style = self.flows[f].style.merged(&st);
                    }
                    if let Some(v) = self.vertex_mut(id) {
                        v.style = v.style.merged(&st);
                    }
                }
                "linkStyle" => {
                    let (which, style) = first_word(rest);
                    let style = style.trim();
                    let style = match style.strip_prefix("interpolate") {
                        // `interpolate <curve> [styles]`: the per-edge curve is not drawn.
                        Some(r) => first_word(r).1,
                        None => style,
                    };
                    let idx = if which == "default" {
                        None
                    } else {
                        let parsed: std::result::Result<Vec<usize>, _> =
                            which.split(',').map(|s| s.trim().parse()).collect();
                        Some(parsed.map_err(|_| {
                            err(*line, stmt, 10, "Expecting a link index or 'default'")
                        })?)
                    };
                    self.link_styles
                        .push((*line, idx, StyleOverrides::parse(style)));
                }
                "click" => {}
                _ => ids.extend(self.chain(*line, stmt)?),
            }
        }
        match block {
            Block::Top => Ok(ids),
            Block::Flow | Block::Global => Err(Error::parse(
                stmts.last().map(|s| s.0).unwrap_or(1),
                "Unexpected end of input: missing 'end' for flow",
            )),
        }
    }

    /// `flow [id][\[title\]][@{…}]` … `end`.
    fn flow(
        &mut self,
        stmts: &[(usize, String)],
        pos: &mut usize,
        line: usize,
        stmt: &str,
        rest: &str,
    ) -> Result<String> {
        let mut sc = Scanner::new(rest);
        let (id, title) = if rest.is_empty() {
            (None, None)
        } else {
            let id = sc.header_id();
            let title = if sc.peek() == Some('[') {
                sc.pos += 1;
                let t = sc
                    .until_close("]")
                    .ok_or_else(|| err(line, stmt, 0, "Unterminated flow title: expected ']'"))?;
                Some(t)
            } else {
                None
            };
            (Some(id), title)
        };
        sc.ws();
        let meta = if sc.starts_with("@{") {
            Some(
                sc.shape_data()
                    .ok_or_else(|| err(line, stmt, 0, "Unterminated '@{'"))?,
            )
        } else {
            None
        };
        sc.ws();
        if !sc.done() {
            return Err(err(
                line,
                stmt,
                5 + sc.pos,
                &format!("Unexpected '{}' after flow header", sc.rest()),
            ));
        }
        self.dir_stack.push(None);
        let list = self.document(stmts, pos, Block::Flow);
        let dir = self.dir_stack.pop().flatten();
        let id = self.add_subgraph(id, &list?, title, dir);
        if let Some(m) = meta {
            self.add_vertex(&id, None, None, Some(&m), line, stmt)?;
        }
        Ok(id)
    }

    /// `connector id[\["Title"\]][@{…}]`.
    fn connector(&mut self, line: usize, stmt: &str, rest: &str) -> Result<String> {
        let mut sc = Scanner::new(rest);
        let id = sc.header_id();
        if id.is_empty() {
            return Err(err(line, stmt, 10, "Expecting a connector id"));
        }
        let mut title = String::new();
        if sc.peek() == Some('[') {
            sc.pos += 1;
            title = sc
                .until_close("]")
                .ok_or_else(|| err(line, stmt, 0, "Unterminated connector title: expected ']'"))?;
        }
        sc.ws();
        let meta = if sc.starts_with("@{") {
            sc.shape_data()
        } else {
            None
        };
        let title = title.trim().to_string();
        match self.index.get(&id) {
            Some(&i) if self.vertices[i].is_connector => {
                if !title.is_empty() {
                    self.vertices[i].label = title;
                }
            }
            _ => {
                let label = if title.is_empty() { id.clone() } else { title };
                match self.index.get(&id) {
                    Some(&i) => {
                        let v = &mut self.vertices[i];
                        v.is_connector = true;
                        v.label = label;
                    }
                    None => self.push_vertex(Vertex {
                        id: id.clone(),
                        label,
                        ty: None,
                        is_connector: true,
                        classes: Vec::new(),
                        style: StyleOverrides::default(),
                        metadata: Vec::new(),
                    }),
                }
            }
        }
        if let Some(m) = meta {
            self.add_vertex(&id, None, None, Some(&m), line, stmt)?;
        }
        Ok(id)
    }

    fn push_vertex(&mut self, v: Vertex) {
        self.index.insert(v.id.clone(), self.vertices.len());
        self.vertices.push(v);
    }

    fn vertex_mut(&mut self, id: &str) -> Option<&mut Vertex> {
        self.index.get(id).map(|&i| &mut self.vertices[i])
    }

    /// `setClass`: vertices, edges (by id) and flows.
    fn set_class(&mut self, id: &str, class: &str) {
        if let Some(v) = self.vertex_mut(id) {
            v.classes.push(class.to_string());
        }
        for e in self
            .edges
            .iter_mut()
            .filter(|e| e.id.as_deref() == Some(id))
        {
            e.classes.push(class.to_string());
        }
        if let Some(&f) = self.flow_index.get(id) {
            self.flows[f].classes.push(class.to_string());
        }
    }

    /// `addVertex(id, text, type, …, shapeData)`.
    fn add_vertex(
        &mut self,
        id: &str,
        text: Option<String>,
        ty: Option<&str>,
        meta: Option<&str>,
        line: usize,
        stmt: &str,
    ) -> Result<()> {
        if id.trim().is_empty() {
            return Ok(());
        }
        let doc = meta.map(parse_meta);
        // Resolve the shape alias, validating what the author typed.
        let mut shape = None;
        if let Some(d) = &doc {
            if let Some((_, v)) = d.iter().rev().find(|(k, _)| k == "shape") {
                if !v.is_string {
                    return Err(err(line, stmt, 0, &format!("No such shape: {}.", v.text)));
                }
                let authored = v.text.as_str();
                if authored != authored.to_lowercase() || authored.contains('_') {
                    return Err(err(
                        line,
                        stmt,
                        0,
                        &format!("No such shape: {authored}. Shape names should be lowercase."),
                    ));
                }
                let resolved = resolve_alias(authored);
                if !is_valid_shape(resolved) {
                    return Err(err(line, stmt, 0, &format!("No such shape: {resolved}.")));
                }
                shape = Some(resolved.to_string());
            }
        }
        let flat = |d: &[(String, MetaValue)]| -> Vec<(String, String)> {
            d.iter()
                .map(|(k, v)| {
                    let v = if k == "shape" {
                        resolve_alias(&v.text).to_string()
                    } else {
                        v.text.clone()
                    };
                    (k.clone(), v)
                })
                .collect()
        };
        // Metadata on a flow container, a connector or an edge (by its id).
        if let (Some(&f), Some(d)) = (self.flow_index.get(id), &doc) {
            self.flows[f].metadata.extend(flat(d));
            return Ok(());
        }
        if let (Some(&i), Some(d)) = (self.index.get(id), &doc) {
            if self.vertices[i].is_connector {
                self.vertices[i].metadata.extend(flat(d));
                return Ok(());
            }
        }
        // `connectors` stays reserved as a declaration-group id.
        if id == "connectors" {
            return Ok(());
        }
        if let Some(e) = self.edges.iter_mut().find(|e| e.id.as_deref() == Some(id)) {
            if let Some(d) = &doc {
                e.metadata.extend(flat(d));
            }
            return Ok(());
        }
        if !self.index.contains_key(id) {
            self.push_vertex(Vertex {
                id: id.to_string(),
                label: id.to_string(),
                ty: None,
                is_connector: false,
                classes: Vec::new(),
                style: StyleOverrides::default(),
                metadata: Vec::new(),
            });
        }
        let v = self.vertex_mut(id).expect("vertex just ensured");
        if let Some(t) = text {
            v.label = t;
        }
        if let Some(t) = ty {
            v.ty = Some(t.to_string());
        }
        if let Some(d) = &doc {
            v.metadata.extend(flat(d));
            if let Some(s) = shape {
                v.ty = Some(s);
            }
            if let Some((_, l)) = d.iter().rev().find(|(k, _)| k == "label") {
                v.label = l.text.clone();
            }
        }
        Ok(())
    }

    /// `addSubGraph`: membership is every id the block mentions, minus the flow
    /// itself, global nodes and ids an earlier-completed flow already holds.
    fn add_subgraph(
        &mut self,
        id: Option<String>,
        list: &[String],
        title: Option<String>,
        dir: Option<Direction>,
    ) -> String {
        let id = id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("subGraph{}", self.sub_count));
        self.sub_count += 1;
        let mut seen = HashSet::new();
        let nodes: Vec<String> = list
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && *s != id && !self.globals.contains(*s))
            .filter(|s| seen.insert(s.to_string()))
            .filter(|s| !self.flows.iter().any(|f| f.nodes.iter().any(|n| n == s)))
            .map(str::to_string)
            .collect();
        let title = title.unwrap_or_default().trim().to_string();
        match self.flow_index.get(&id) {
            Some(&f) => {
                let existing = &mut self.flows[f];
                for n in nodes {
                    if !existing.nodes.contains(&n) {
                        existing.nodes.push(n);
                    }
                }
                if !title.is_empty() {
                    existing.title = title;
                }
                if dir.is_some() {
                    existing.direction = dir;
                }
            }
            None => {
                self.flow_index.insert(id.clone(), self.flows.len());
                self.flows.push(Flow {
                    id: id.clone(),
                    title,
                    nodes,
                    classes: Vec::new(),
                    style: StyleOverrides::default(),
                    metadata: Vec::new(),
                    direction: dir,
                });
            }
        }
        id
    }

    /// `addGlobal`: the ids stay top level wherever they are mentioned.
    fn add_global(&mut self, list: &[String]) {
        for id in list {
            let id = id.trim();
            if !id.is_empty() {
                self.globals.insert(id.to_string());
            }
        }
        for f in &mut self.flows {
            f.nodes.retain(|n| !self.globals.contains(n));
        }
    }

    /// A vertex statement: `a[..]@{..} & b --> c -.- d …`. Returns every node id.
    fn chain(&mut self, line: usize, stmt: &str) -> Result<Vec<String>> {
        let mut sc = Scanner::new(stmt);
        let mut ids = Vec::new();
        let mut prev = self.group(&mut sc, line, stmt)?;
        ids.extend(prev.iter().cloned());
        loop {
            sc.ws();
            if sc.done() {
                break;
            }
            let col = sc.pos;
            let link = sc
                .link()
                .map_err(|m| err(line, stmt, col, &m))?
                .ok_or_else(|| {
                    err(
                        line,
                        stmt,
                        col,
                        &format!(
                            "Expecting '-->', '-.-' or '--x', got '{}'",
                            sc.rest().chars().take(10).collect::<String>()
                        ),
                    )
                })?;
            sc.ws();
            let next = self.group(&mut sc, line, stmt)?;
            self.add_link(&prev, &next, &link);
            ids.extend(next.iter().cloned());
            prev = next;
        }
        Ok(ids)
    }

    /// `node`: one or more `&`-joined vertices, each with optional shape data.
    fn group(&mut self, sc: &mut Scanner, line: usize, stmt: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        loop {
            sc.ws();
            let col = sc.pos;
            let id = sc.node_id();
            if id.is_empty() {
                return Err(err(
                    line,
                    stmt,
                    col,
                    &format!(
                        "Expecting a node id, got '{}'",
                        sc.rest().chars().take(10).collect::<String>()
                    ),
                ));
            }
            let shape = sc.vertex_shape().map_err(|m| err(line, stmt, sc.pos, &m))?;
            match shape {
                Some((ty, text)) => self.add_vertex(&id, Some(text), Some(ty), None, line, stmt)?,
                None => self.add_vertex(&id, None, None, None, line, stmt)?,
            }
            if sc.starts_with(":::") {
                sc.pos += 3;
                let class = sc.take_while(|c| c.is_alphanumeric() || c == '_' || c == '-');
                self.set_class(&id, &class);
            }
            sc.ws();
            if sc.starts_with("@{") {
                let col = sc.pos;
                let meta = sc
                    .shape_data()
                    .ok_or_else(|| err(line, stmt, col, "Unterminated '@{'"))?;
                self.add_vertex(&id, None, None, Some(&meta), line, stmt)?;
            }
            out.push(id);
            sc.ws();
            if sc.peek() == Some('&') {
                sc.pos += 1;
                continue;
            }
            return Ok(out);
        }
    }

    /// `addLink`: every start × every end; a user id goes on the last-start ×
    /// first-end edge only.
    fn add_link(&mut self, from: &[String], to: &[String], link: &Link) {
        let semantic = match (link.arrow, link.dotted) {
            (ArrowType::Point, false) => Some(EdgeSemantic::Sequence),
            (ArrowType::Cross, false) => Some(EdgeSemantic::Failure),
            (ArrowType::Open, true) => Some(EdgeSemantic::Reference),
            _ => None,
        };
        for (i, a) in from.iter().enumerate() {
            for (j, b) in to.iter().enumerate() {
                let user_id = link
                    .id
                    .clone()
                    .filter(|_| i + 1 == from.len() && j == 0)
                    .filter(|id| !self.edges.iter().any(|e| e.id.as_deref() == Some(id)));
                self.edges.push(AfEdge {
                    start: a.clone(),
                    end: b.clone(),
                    id: user_id,
                    label: link.text.clone(),
                    arrow: link.arrow,
                    dotted: link.dotted,
                    length: link.length.min(10),
                    semantic,
                    classes: Vec::new(),
                    style: StyleOverrides::default(),
                    metadata: Vec::new(),
                });
            }
        }
    }

    /// `classDef default` / `node`, `class`, `:::` and `linkStyle`.
    fn apply_styles(&mut self) -> Result<()> {
        let def = |name: &str| self.class_defs.get(name).cloned().unwrap_or_default();
        let base = def("default").merged(&def("node"));
        for v in &mut self.vertices {
            let mut st = base.clone();
            for c in &v.classes {
                st = st.merged(&def(c));
            }
            v.style = st.merged(&v.style);
        }
        for f in &mut self.flows {
            let mut st = StyleOverrides::default();
            for c in &f.classes {
                st = st.merged(&def(c));
            }
            f.style = st.merged(&f.style);
        }
        for e in &mut self.edges {
            let mut st = StyleOverrides::default();
            for c in &e.classes {
                st = st.merged(&def(c));
            }
            e.style = st;
        }
        for (line, idx, st) in &self.link_styles {
            match idx {
                None => {
                    for e in &mut self.edges {
                        e.style = st.merged(&e.style);
                    }
                }
                Some(list) => {
                    for &i in list {
                        let n = self.edges.len();
                        let e = self.edges.get_mut(i).ok_or_else(|| {
                            Error::parse(*line, format!("The index {i} for linkStyle is out of bounds. Valid indices for linkStyle are between 0 and {}. (Help: Ensure that the index is within the range of existing edges.)", n.saturating_sub(1)))
                        })?;
                        e.style = e.style.merged(st);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Parse an `@{ … }` body: YAML flow mapping on one line, block mapping across lines.
/// Nested values are skipped (no key mermaid acts on is nested).
fn parse_meta(body: &str) -> Vec<(String, MetaValue)> {
    let mut entries: Vec<String> = Vec::new();
    if body.contains('\n') {
        let indent = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        for l in body.lines() {
            if l.trim().is_empty() || l.len() - l.trim_start().len() > indent {
                continue;
            }
            entries.extend(split_top(l.trim()));
        }
    } else {
        entries = split_top(body);
    }
    entries
        .iter()
        .filter_map(|e| {
            let (k, v) = split_key(e)?;
            let k = unquote(k.trim()).0;
            if matches!(k.as_str(), "__proto__" | "constructor" | "prototype") || k.is_empty() {
                return None;
            }
            let (text, quoted) = unquote(v.trim());
            let is_string = quoted
                || !(text.is_empty()
                    || text.parse::<f64>().is_ok()
                    || matches!(text.as_str(), "true" | "false" | "null" | "~"));
            Some((k, MetaValue { text, is_string }))
        })
        .collect()
}

/// Split on commas outside quotes and brackets.
fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter().filter(|e| !e.trim().is_empty()).collect()
}

/// `key: value` at the first `:` outside quotes.
fn split_key(e: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    for (i, c) in e.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, ':') => return Some((&e[..i], &e[i + 1..])),
            _ => {}
        }
    }
    None
}

fn unquote(s: &str) -> (String, bool) {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return (s[1..s.len() - 1].to_string(), true);
        }
    }
    (s.to_string(), false)
}

/// Character scanner over one statement.
struct Scanner {
    c: Vec<char>,
    pos: usize,
}

impl Scanner {
    fn new(s: &str) -> Self {
        Scanner {
            c: s.chars().collect(),
            pos: 0,
        }
    }
    fn peek(&self) -> Option<char> {
        self.c.get(self.pos).copied()
    }
    fn at(&self, i: usize) -> Option<char> {
        self.c.get(i).copied()
    }
    fn done(&self) -> bool {
        self.pos >= self.c.len()
    }
    fn rest(&self) -> String {
        self.c[self.pos.min(self.c.len())..].iter().collect()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }
    fn starts_with(&self, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(i, ch)| self.at(self.pos + i) == Some(ch))
    }
    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let s = self.pos;
        while self.peek().is_some_and(&f) {
            self.pos += 1;
        }
        self.c[s..self.pos].iter().collect()
    }

    /// `textNoTags` of a `flow` / `connector` header: everything up to `[` or `@{`
    /// (it may contain spaces), or one quoted string.
    fn header_id(&mut self) -> String {
        self.ws();
        if self.peek() == Some('"') {
            self.pos += 1;
            let s = self.take_while(|c| c != '"');
            self.pos += 1;
            return s.trim_matches('`').to_string();
        }
        let s = self.pos;
        while !self.done() && self.peek() != Some('[') && !self.starts_with("@{") {
            self.pos += 1;
        }
        self.c[s..self.pos]
            .iter()
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Text up to `close`; a quoted label must be followed by the closer. Quotes are
    /// stripped, markdown strings keep their backticks for the label normaliser.
    fn until_close(&mut self, close: &str) -> Option<String> {
        self.ws();
        if self.peek() == Some('"') {
            let start = self.pos + 1;
            let end = (start..self.c.len()).find(|&i| self.c[i] == '"')?;
            let save = self.pos;
            self.pos = end + 1;
            self.ws();
            if self.starts_with(close) {
                self.pos += close.chars().count();
                return Some(self.c[start..end].iter().collect());
            }
            self.pos = save;
        }
        let start = self.pos;
        let first = close.chars().next()?;
        let end = (start..self.c.len()).find(|&i| {
            self.c[i] == first
                && close
                    .chars()
                    .enumerate()
                    .all(|(k, ch)| self.at(i + k) == Some(ch))
        })?;
        self.pos = end + close.chars().count();
        Some(
            self.c[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        )
    }

    fn is_link_start(&self, i: usize) -> bool {
        matches!((self.at(i), self.at(i + 1)), (Some('-'), Some('-' | '.')))
    }

    /// `idString`: NODE_STRING characters, `-` not starting a link, unicode letters.
    fn node_id(&mut self) -> String {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            let ok = ch.is_alphanumeric()
                || matches!(
                    ch,
                    '_' | '!' | '#' | '$' | '\'' | '*' | '+' | '?' | '\\' | '/'
                )
                || (ch == '-' && !self.is_link_start(self.pos))
                || (ch == '.' && self.at(self.pos + 1) != Some('-'))
                || (ch == ':' && !self.starts_with(":::"))
                || (ch == '@' && !self.starts_with("@{") && !self.link_id_ahead());
            if !ok {
                break;
            }
            self.pos += 1;
        }
        self.c[start..self.pos].iter().collect()
    }

    /// `[^\s"]+@` immediately before a link: an edge id, not part of a node id.
    fn link_id_ahead(&self) -> bool {
        self.at(self.pos) == Some('@') && self.is_link_start(self.pos + 1)
    }

    /// Inline vertex shapes (`[..]`, `(..)`, `{..}`, …) → (jison type, text).
    fn vertex_shape(&mut self) -> std::result::Result<Option<(&'static str, String)>, String> {
        const OPENERS: &[(&str, &[(&str, &str)])] = &[
            ("(((", &[(")))", "doublecircle")]),
            ("((", &[("))", "circle")]),
            ("(-", &[("-)", "ellipse")]),
            ("([", &[("])", "stadium")]),
            ("(", &[(")", "round")]),
            ("[[", &[("]]", "subroutine")]),
            ("[(", &[(")]", "cylinder")]),
            ("[/", &[("/]", "lean_right"), ("\\]", "trapezoid")]),
            ("[\\", &[("\\]", "lean_left"), ("/]", "inv_trapezoid")]),
            ("[", &[("]", "square")]),
            ("{{", &[("}}", "hexagon")]),
            ("{", &[("}", "diamond")]),
            (">", &[("]", "odd")]),
        ];
        for (open, closers) in OPENERS {
            if !self.starts_with(open) {
                continue;
            }
            self.pos += open.chars().count();
            let body_start = self.pos;
            // A quoted label runs to its closing quote.
            let mut probe = self.pos;
            while self.at(probe).is_some_and(char::is_whitespace) {
                probe += 1;
            }
            let mut search_from = self.pos;
            let mut quoted = None;
            if self.at(probe) == Some('"') {
                if let Some(end) = (probe + 1..self.c.len()).find(|&i| self.c[i] == '"') {
                    quoted = Some(self.c[probe + 1..end].iter().collect::<String>());
                    search_from = end + 1;
                }
            }
            for i in search_from..self.c.len() {
                for (close, ty) in closers.iter() {
                    if close
                        .chars()
                        .enumerate()
                        .all(|(k, ch)| self.at(i + k) == Some(ch))
                    {
                        let between: String = self.c[search_from..i].iter().collect();
                        let text = match &quoted {
                            Some(q) if between.trim().is_empty() => q.clone(),
                            _ => self.c[body_start..i]
                                .iter()
                                .collect::<String>()
                                .trim()
                                .to_string(),
                        };
                        self.pos = i + close.chars().count();
                        return Ok(Some((ty, text)));
                    }
                }
            }
            return Err(format!(
                "Unterminated node shape: expected '{}'",
                closers[0].0
            ));
        }
        Ok(None)
    }

    /// `@{ … }` at the cursor: the body text (newlines in strings become `<br/>`).
    fn shape_data(&mut self) -> Option<String> {
        self.pos += 2;
        let start = self.pos;
        let mut in_q = false;
        let mut i = start;
        while i < self.c.len() {
            match self.c[i] {
                '"' => in_q = !in_q,
                '}' if !in_q => break,
                _ => {}
            }
            i += 1;
        }
        if i >= self.c.len() {
            return None;
        }
        let mut body = String::new();
        let mut in_q = false;
        let mut k = start;
        while k < i {
            let ch = self.c[k];
            if ch == '"' {
                in_q = !in_q;
            }
            if in_q && ch == '\n' {
                body.push_str("<br/>");
                k += 1;
                while k < i && self.c[k].is_whitespace() {
                    k += 1;
                }
                continue;
            }
            body.push(ch);
            k += 1;
        }
        self.pos = i + 1;
        Some(
            body.trim_matches(|c: char| c == ' ' || c == '\t')
                .to_string(),
        )
    }

    /// A link at the cursor (see [`Link`]); `Ok(None)` when none starts here.
    fn link(&mut self) -> std::result::Result<Option<Link>, String> {
        let save = self.pos;
        // LINK_ID: `e1@-->`.
        let mut id = None;
        let id_end = (self.pos..self.c.len())
            .take_while(|&i| !self.c[i].is_whitespace() && self.c[i] != '"')
            .find(|&i| self.c[i] == '@');
        if let Some(at) = id_end {
            if at > self.pos && self.is_link_start(at + 1) {
                id = Some(self.c[self.pos..at].iter().collect::<String>());
                self.pos = at + 1;
            }
        }
        if let Some(mut l) = self.full_link() {
            l.id = id;
            l.text = self.pipe_label()?;
            return Ok(Some(l));
        }
        // START_LINK `--` edge text, closed by `--+[x>]`.
        if self.starts_with("--") {
            self.pos += 2;
            let text_start = self.pos;
            while !self.done() {
                let here = self.pos;
                if self.peek() == Some('-') && self.at(here + 1) == Some('-') {
                    let mut k = here;
                    while self.at(k) == Some('-') {
                        k += 1;
                    }
                    if let Some(end @ ('x' | '>')) = self.at(k) {
                        let text: String = self.c[text_start..here].iter().collect();
                        let text = text.trim();
                        let text = unquote(text).0;
                        self.pos = k + 1;
                        return Ok(Some(Link {
                            arrow: if end == 'x' {
                                ArrowType::Cross
                            } else {
                                ArrowType::Point
                            },
                            dotted: false,
                            length: k - here - 1,
                            text: Some(text).filter(|t| !t.is_empty()),
                            id,
                        }));
                    }
                    return Err(
                        "Expecting '-->' or '--x' to close the edge label (agentflow has no \
                         '---', '==>', '-.->', '~~~' or circle-ended edges)"
                            .into(),
                    );
                }
                self.pos += 1;
            }
            return Err("Unterminated edge label: expected '-->' or '--x'".into());
        }
        self.pos = save;
        Ok(None)
    }

    /// LINK: `--+[x>]` or `-.+-`.
    fn full_link(&mut self) -> Option<Link> {
        let mut k = self.pos;
        while self.at(k) == Some('-') {
            k += 1;
        }
        let dashes = k - self.pos;
        if dashes >= 2 {
            if let Some(end @ ('x' | '>')) = self.at(k) {
                self.pos = k + 1;
                return Some(Link {
                    arrow: if end == 'x' {
                        ArrowType::Cross
                    } else {
                        ArrowType::Point
                    },
                    dotted: false,
                    length: dashes - 1,
                    text: None,
                    id: None,
                });
            }
            return None;
        }
        if dashes == 1 && self.at(k) == Some('.') {
            let mut p = k;
            while self.at(p) == Some('.') {
                p += 1;
            }
            if self.at(p) == Some('-') {
                let dots = p - k;
                self.pos = p + 1;
                return Some(Link {
                    arrow: ArrowType::Open,
                    dotted: true,
                    length: dots,
                    text: None,
                    id: None,
                });
            }
        }
        None
    }

    /// `|text|` after a link.
    fn pipe_label(&mut self) -> std::result::Result<Option<String>, String> {
        let save = self.pos;
        self.ws();
        if self.peek() != Some('|') {
            self.pos = save;
            return Ok(None);
        }
        let start = self.pos + 1;
        let end = (start..self.c.len())
            .find(|&i| self.c[i] == '|')
            .ok_or("Unterminated edge label: expected '|'")?;
        self.pos = end + 1;
        let t: String = self.c[start..end].iter().collect();
        Ok(Some(unquote(t.trim()).0).filter(|t| !t.is_empty()))
    }
}

// ---------------------------------------------------------------- getData

/// Shared options of the conversion (agentflow config + palette length).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvertOptions {
    /// `agentflow.wrappingWidth`: node labels wrap at this width.
    pub wrapping_width: f64,
    pub font_size: f64,
    /// Length of the theme's categorical palette (0 when the theme has none).
    pub palette_len: usize,
    /// `flowchart.inheritDir` (agentflow's `addSubGraph` reads the flowchart section):
    /// a flow without its own `direction` takes the diagram's.
    pub inherit_dir: bool,
}

/// The pure equivalent of `AgentFlowDB.getData()`: collapse handling, containment
/// (cycle-safe), palette slots, shapes, and edge redirection to collapsed containers.
pub fn to_graph(af: &Agentflow, opts: &ConvertOptions) -> GraphDiagram {
    let flow_of: HashMap<&str, usize> = af
        .flows
        .iter()
        .enumerate()
        .map(|(i, f)| (f.id.as_str(), i))
        .collect();

    // Ids hidden by collapsed flows, mapped to their outermost collapsed ancestor.
    let mut hidden: HashSet<String> = HashSet::new();
    let mut collapsed_ancestor: HashMap<String, String> = HashMap::new();
    fn collect(
        af: &Agentflow,
        flow_of: &HashMap<&str, usize>,
        sg: &str,
        ancestor: &str,
        seen: &mut HashSet<String>,
        hidden: &mut HashSet<String>,
        map: &mut HashMap<String, String>,
    ) {
        if !seen.insert(sg.to_string()) {
            return;
        }
        let Some(&fi) = flow_of.get(sg) else {
            return;
        };
        for child in &af.flows[fi].nodes {
            if child == ancestor || seen.contains(child) {
                continue;
            }
            hidden.insert(child.clone());
            map.insert(child.clone(), ancestor.to_string());
            collect(af, flow_of, child, ancestor, seen, hidden, map);
        }
    }
    for f in &af.flows {
        if f.collapsed() && !hidden.contains(&f.id) {
            collect(
                af,
                &flow_of,
                &f.id,
                &f.id,
                &mut HashSet::new(),
                &mut hidden,
                &mut collapsed_ancestor,
            );
        }
    }

    // Parents: later-completed (outer) flows first, earlier ones overwrite; a nesting
    // that would close a containment cycle is dropped.
    let mut parent: HashMap<String, String> = HashMap::new();
    for f in af.flows.iter().rev() {
        if hidden.contains(&f.id) {
            continue;
        }
        for id in &f.nodes {
            let mut cur = Some(f.id.as_str());
            let mut seen = HashSet::new();
            let mut cycle = false;
            while let Some(c) = cur {
                if !seen.insert(c.to_string()) {
                    break;
                }
                if c == id {
                    cycle = true;
                    break;
                }
                cur = parent.get(c).map(String::as_str);
            }
            if !cycle {
                parent.insert(id.clone(), f.id.clone());
            }
        }
    }

    // Declaration order of containers: pre-order walk of the containment forest.
    let mut order: HashMap<&str, usize> = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for f in &af.flows {
        if let Some(p) = parent.get(&f.id) {
            children.entry(p.as_str()).or_default().push(&f.id);
        }
    }
    fn walk<'a>(
        id: &'a str,
        children: &HashMap<&str, Vec<&'a str>>,
        order: &mut HashMap<&'a str, usize>,
    ) {
        if order.contains_key(id) {
            return;
        }
        let n = order.len();
        order.insert(id, n);
        for c in children.get(id).into_iter().flatten() {
            walk(c, children, order);
        }
    }
    for f in &af.flows {
        if !parent.contains_key(&f.id) {
            walk(&f.id, &children, &mut order);
        }
    }
    let mut fallback = order.len();
    let mut container_slot = |id: &str| {
        let n = order.get(id).copied().unwrap_or_else(|| {
            fallback += 1;
            fallback - 1
        });
        let count = opts.palette_len.saturating_sub(KIND_COUNT).max(1);
        let slot = KIND_COUNT + n % count;
        ColorSlot(if opts.palette_len > 0 {
            slot % opts.palette_len
        } else {
            slot
        })
    };

    // Clusters (visible, expanded flows) in declaration order.
    let mut visible: Vec<&Flow> = af
        .flows
        .iter()
        .filter(|f| !hidden.contains(&f.id) && !f.collapsed())
        .collect();
    visible.sort_by_key(|f| order.get(f.id.as_str()).copied().unwrap_or(usize::MAX));
    let cluster_ix: HashMap<&str, usize> = visible
        .iter()
        .enumerate()
        .map(|(i, f)| (f.id.as_str(), i))
        .collect();
    let cluster_parent = |id: &str| {
        parent
            .get(id)
            .and_then(|p| cluster_ix.get(p.as_str()))
            .map(|&c| ClusterId(c))
    };
    let clusters: Vec<Cluster> = visible
        .iter()
        .map(|f| Cluster {
            style: f.style.clone(),
            color_slot: Some(container_slot(&f.id)),
            direction: f.direction.or(opts.inherit_dir.then_some(af.direction)),
            ..Cluster::new(f.id.clone(), f.title.clone(), cluster_parent(&f.id))
        })
        .collect();

    let wrap = |label: &str| -> String {
        if opts.wrapping_width <= 0.0 {
            return label.to_string();
        }
        wrap_text(label, opts.wrapping_width, opts.font_size, false).join("<br>")
    };

    // Nodes: collapsed containers (reverse completion order, as getData pushes them),
    // then vertices in first-mention order.
    let mut nodes = Vec::new();
    let mut node_ix: HashMap<String, usize> = HashMap::new();
    for f in af.flows.iter().rev() {
        if hidden.contains(&f.id) || !f.collapsed() {
            continue;
        }
        let mut n = Node::new(f.id.clone(), f.title.clone(), Shape::CollapsedGroup);
        n.cluster = cluster_parent(&f.id);
        n.style = f.style.clone();
        n.color_slot = Some(container_slot(&f.id));
        node_ix.insert(f.id.clone(), nodes.len());
        nodes.push(n);
    }
    for v in &af.vertices {
        if hidden.contains(&v.id) || flow_of.contains_key(v.id.as_str()) {
            continue;
        }
        let shape = match v.shape() {
            AfShape::RoundedRect => Shape::Rounded,
            AfShape::Subroutine => Shape::Subroutine,
            AfShape::LeanRight => Shape::LeanRight,
            AfShape::Diamond => Shape::Diamond,
            AfShape::LinedDocument => Shape::LinedDocument,
            AfShape::Hexagon => Shape::Hexagon,
        };
        let mut n = Node::new(v.id.clone(), wrap(&v.label), shape);
        n.cluster = cluster_parent(&v.id);
        n.style = v.style.clone();
        n.classes = std::iter::once(format!("af-kind-{}", kind_name(v.kind())))
            .chain(v.classes.iter().cloned())
            .collect();
        n.color_slot = Some(ColorSlot(v.kind().slot()));
        node_ix.insert(v.id.clone(), nodes.len());
        nodes.push(n);
    }
    // `style` on a flow id styles the cluster, also when given via the vertex record.
    let mut clusters = clusters;
    for v in &af.vertices {
        if let Some(&c) = cluster_ix.get(v.id.as_str()) {
            clusters[c].style = clusters[c].style.merged(&v.style);
        }
    }

    let endpoint = |id: &str| -> Option<Endpoint> {
        node_ix
            .get(id)
            .map(|&i| Endpoint::Node(NodeId(i)))
            .or_else(|| cluster_ix.get(id).map(|&c| Endpoint::Cluster(ClusterId(c))))
    };
    let mut edges = Vec::new();
    for e in &af.edges {
        let start = collapsed_ancestor.get(&e.start).unwrap_or(&e.start);
        let end = collapsed_ancestor.get(&e.end).unwrap_or(&e.end);
        // A self-loop produced by a collapse is dropped; an authored one is kept.
        if start == end && e.start != e.end {
            continue;
        }
        let (Some(from), Some(to)) = (endpoint(start), endpoint(end)) else {
            continue;
        };
        let mut g = Edge::new(from, to);
        g.end = match e.arrow {
            ArrowType::Open => Head::None,
            ArrowType::Cross => Head::Cross,
            ArrowType::Point | ArrowType::Invalid => Head::Arrow,
        };
        g.line = if e.dotted {
            LineKind::Dotted
        } else {
            LineKind::Solid
        };
        g.minlen = e.length.max(1);
        g.label = e.label.clone();
        g.style = e.style.clone();
        // `animate: true` / `animation: fast|slow` → `.edge-animation-*` (9,5 dashes).
        if meta_get(&e.metadata, "animate") == Some("true")
            || meta_get(&e.metadata, "animation").is_some_and(|v| !v.is_empty())
        {
            g.style.dash = Some(super::flowchart::ANIMATED_DASH.to_vec());
        }
        edges.push(g);
    }

    GraphDiagram {
        kind: DiagramKind::Agentflow,
        direction: af.direction,
        nodes,
        edges,
        clusters,
        title: af.title.clone(),
    }
}

fn kind_name(k: VertexKind) -> &'static str {
    match k {
        VertexKind::Tool => "tool",
        VertexKind::Task => "task",
        VertexKind::Decision => "decision",
        VertexKind::Input => "input",
        VertexKind::Refdoc => "refdoc",
        VertexKind::Connector => "connector",
        VertexKind::Action => "action",
    }
}

/// Layout settings from the `agentflow` section (with flowchart's `padding` and `curve`,
/// which upstream reads from the flowchart namespace).
fn graph_config(cfg: &MermaidConfig) -> GraphConfig {
    GraphConfig {
        look: cfg.look,
        node_spacing: cfg.section_f64("agentflow", "nodeSpacing", 50.0),
        rank_spacing: cfg.section_f64("agentflow", "rankSpacing", 50.0),
        padding: cfg.flowchart.padding,
        diagram_padding: cfg.section_f64("agentflow", "diagramPadding", 8.0),
        curve: cfg.flowchart.curve,
        use_max_width: cfg.use_max_width("agentflow"),
        title_top_margin: cfg.section_f64("agentflow", "titleTopMargin", 25.0),
        min_node_width: 0.0,
        wrapping_width: 0.0,
        layout: cfg.layout,
        line_hops: cfg.elk_line_hops(),
    }
}

/// Parse, lay out and draw a `agentflow` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let af = parse(body, title)?;
    let opts = ConvertOptions {
        wrapping_width: cfg.section_f64("agentflow", "wrappingWidth", 120.0),
        font_size: pal.font_size,
        palette_len: pal.color_slots.as_ref().map_or(0, |p| p.border.len()),
        inherit_dir: cfg.section_bool("flowchart", "inheritDir", false),
    };
    let g = to_graph(&af, &opts);
    let ro = RenderOptions {
        min_label_width: cfg.section_f64("agentflow", "minNodeWidth", 120.0),
        subgraph_title_margin: cfg.subgraph_title_margin(),
    };
    Ok(graph::render_with(&g, &graph_config(cfg), pal, &ro))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{PathCmd, Rect};
    use crate::scene::{Element, Geometry};

    fn p(src: &str) -> Agentflow {
        parse(src, None).unwrap()
    }

    fn graph(src: &str) -> GraphDiagram {
        to_graph(
            &p(src),
            &ConvertOptions {
                wrapping_width: 0.0,
                font_size: 16.0,
                palette_len: 12,
                inherit_dir: false,
            },
        )
    }

    fn vertex<'a>(af: &'a Agentflow, id: &str) -> &'a Vertex {
        af.vertices.iter().find(|v| v.id == id).unwrap()
    }

    fn flow<'a>(af: &'a Agentflow, id: &str) -> &'a Flow {
        af.flows.iter().find(|f| f.id == id).unwrap()
    }

    /// Cluster id of a node in the converted graph.
    fn parent_of(g: &GraphDiagram, id: &str) -> Option<String> {
        let n = g.nodes.iter().find(|n| n.id == id)?;
        n.cluster.map(|c| g.clusters[c.0].id.clone())
    }

    fn cluster_parent(g: &GraphDiagram, id: &str) -> Option<String> {
        let c = g.clusters.iter().find(|c| c.id == id)?;
        c.parent.map(|p| g.clusters[p.0].id.clone())
    }

    fn edge_names(g: &GraphDiagram) -> Vec<(String, String)> {
        let name = |ep: Endpoint| match ep {
            Endpoint::Node(NodeId(i)) => g.nodes[i].id.clone(),
            Endpoint::Cluster(ClusterId(c)) => g.clusters[c].id.clone(),
        };
        g.edges.iter().map(|e| (name(e.from), name(e.to))).collect()
    }

    #[test]
    fn smoke_two_nodes_and_an_edge() {
        let af = p("agentflow-beta\n");
        assert!(af.vertices.is_empty());
        let af = p("agentflow-beta TB\n  a[\"Alpha\"] --> b[\"Beta\"]");
        assert_eq!(af.direction, Direction::TopBottom);
        assert_eq!(vertex(&af, "a").label, "Alpha");
        assert_eq!(af.edges.len(), 1);
        assert_eq!(
            p("agentflow-beta LR\na --> b").direction,
            Direction::LeftRight
        );
        assert!(parse("graph TD\na-->b", None).is_err());
    }

    #[test]
    fn edge_operators_map_to_semantics() {
        let af =
            p("agentflow-beta\na --> b\nb -.- c\nc --x d\nd ----> e\nf -- yes --> g\ng -->|no| h");
        use EdgeSemantic::*;
        let sem: Vec<_> = af.edges.iter().map(|e| e.semantic).collect();
        assert_eq!(
            sem,
            vec![
                Some(Sequence),
                Some(Reference),
                Some(Failure),
                Some(Sequence),
                Some(Sequence),
                Some(Sequence)
            ]
        );
        assert_eq!(af.edges[1].arrow, ArrowType::Open);
        assert!(af.edges[1].dotted);
        assert_eq!(af.edges[3].length, 3);
        assert_eq!(af.edges[4].label.as_deref(), Some("yes"));
        assert_eq!(af.edges[5].label.as_deref(), Some("no"));
        let g = graph("agentflow-beta\na --> b\nb -.- c\nc --x d");
        let heads: Vec<_> = g.edges.iter().map(|e| (e.end, e.line)).collect();
        assert_eq!(
            heads,
            vec![
                (Head::Arrow, LineKind::Solid),
                (Head::None, LineKind::Dotted),
                (Head::Cross, LineKind::Solid)
            ]
        );
    }

    #[test]
    fn removed_operators_are_rejected() {
        for op in ["==>", "-.->", "---", "~~~", "--o"] {
            assert!(
                parse(&format!("agentflow-beta\na {op} b"), None).is_err(),
                "{op}"
            );
        }
    }

    #[test]
    fn shape_aliases_and_kinds() {
        let af = p("agentflow-beta
  t[\"T\"]@{ shape: tool }
  a[\"A\"]@{ shape: action }
  i[\"I\"]@{ shape: input }
  r[\"R\"]@{ shape: refdoc }
  d[\"D\"]@{ shape: decision }
  d2{\"inline\"}
  k[\"K\"]@{ shape: task }
  plain
  connector llm[\"LLM API\"]");
        use VertexKind::*;
        let kinds: Vec<_> = ["t", "a", "i", "r", "d", "d2", "k", "plain", "llm"]
            .iter()
            .map(|id| vertex(&af, id).kind())
            .collect();
        assert_eq!(
            kinds,
            vec![
                Tool, Action, Input, Refdoc, Decision, Decision, Task, Task, Connector
            ]
        );
        assert_eq!(vertex(&af, "t").shape(), AfShape::Subroutine);
        assert_eq!(vertex(&af, "r").shape(), AfShape::LinedDocument);
        assert_eq!(vertex(&af, "llm").shape(), AfShape::RoundedRect);
        assert_eq!(vertex(&af, "llm").label, "LLM API");
    }

    #[test]
    fn removed_and_unsupported_shapes_fall_back_to_rounded() {
        let af =
            p("agentflow-beta\nstore@{ shape: \"cylinder\" }\nc((circle))\nx@{ shape: cloud }");
        for id in ["store", "c", "x"] {
            assert_eq!(vertex(&af, id).shape(), AfShape::RoundedRect, "{id}");
        }
    }

    #[test]
    fn unknown_or_uppercase_shapes_are_errors() {
        let e = parse("agentflow-beta\na@{ shape: nope }", None).unwrap_err();
        assert!(e.to_string().contains("No such shape: nope."), "{e}");
        let e = parse("agentflow-beta\na@{ shape: roundedRect }", None).unwrap_err();
        assert!(e.to_string().contains("should be lowercase"), "{e}");
    }

    #[test]
    fn metadata_label_and_multiline_blocks() {
        let af = p("agentflow-beta
fetch@{
  shape: tool
  label: \"Fetch it\"
  params: \"city :: String\",
  retry: 2
}
fetch --> x");
        let v = vertex(&af, "fetch");
        assert_eq!(v.kind(), VertexKind::Tool);
        assert_eq!(v.label, "Fetch it");
        assert_eq!(meta_get(&v.metadata, "params"), Some("city :: String"));
        assert_eq!(af.edges.len(), 1);
        // Prototype-shaped keys are stripped.
        let af = p("agentflow-beta\na@{ __proto__: 1, model: \"m\" }");
        let v = vertex(&af, "a");
        assert_eq!(meta_get(&v.metadata, "__proto__"), None);
        assert_eq!(meta_get(&v.metadata, "model"), Some("m"));
    }

    #[test]
    fn flows_with_and_without_labels_and_nesting() {
        let src = "agentflow-beta
flow pipeline[\"Pipeline\"]
  a --> b
end
flow bare
  c
end
flow
  d
end
flow outer[\"Outer\"]
  flow inner[\"Inner\"]
    e --> f
  end
end";
        let af = p(src);
        assert_eq!(flow(&af, "pipeline").title, "Pipeline");
        assert_eq!(flow(&af, "pipeline").nodes, vec!["a", "b"]);
        assert_eq!(flow(&af, "bare").title, "");
        assert_eq!(flow(&af, "subGraph2").nodes, vec!["d"]);
        assert_eq!(flow(&af, "outer").nodes, vec!["inner"]);
        let g = graph(src);
        assert_eq!(cluster_parent(&g, "inner").as_deref(), Some("outer"));
        assert_eq!(parent_of(&g, "e").as_deref(), Some("inner"));
    }

    #[test]
    fn unterminated_flow_and_stray_end_are_errors() {
        assert!(parse("agentflow-beta\nflow a\n  b\n", None).is_err());
        assert!(parse("agentflow-beta\nend\n", None).is_err());
    }

    #[test]
    fn innermost_flow_claims_a_node_first() {
        // `city` is mentioned in the outer flow before the inner one, but the inner
        // flow completes first and keeps it (the docs' worked example).
        let g = graph(
            "agentflow-beta
flow team[\"Team\"]
  city[\"city\"]
  flow researcher[\"Researcher\"]
    city --> research
  end
  flow designer[\"Designer\"]
    render
  end
  researcher --> designer
end",
        );
        assert_eq!(parent_of(&g, "city").as_deref(), Some("researcher"));
        assert_eq!(parent_of(&g, "render").as_deref(), Some("designer"));
        assert_eq!(cluster_parent(&g, "researcher").as_deref(), Some("team"));
        assert_eq!(cluster_parent(&g, "designer").as_deref(), Some("team"));
        assert!(edge_names(&g).contains(&("researcher".into(), "designer".into())));
        assert!(
            g.edges
                .iter()
                .any(|e| matches!((e.from, e.to), (Endpoint::Cluster(_), Endpoint::Cluster(_))))
        );
    }

    #[test]
    fn global_nodes_never_join_a_flow() {
        let src = "agentflow-beta
flow f[\"Flow\"]
  global
    A
  end
  A --> B
end";
        let af = p(src);
        assert_eq!(flow(&af, "f").nodes, vec!["B"]);
        let g = graph(src);
        assert_eq!(parent_of(&g, "A"), None);
        assert_eq!(parent_of(&g, "B").as_deref(), Some("f"));
        // Order independent: a later global block releases the node.
        let af = p("agentflow-beta\nflow f\n  A --> X\nend\nglobal\n  X\nend");
        assert_eq!(flow(&af, "f").nodes, vec!["A"]);
        // Labels may contain the keyword.
        let af =
            p("agentflow-beta\nflow f[\"global settings\"]\n  a[\"a global thing\"] --> b\nend");
        assert_eq!(flow(&af, "f").title, "global settings");
    }

    #[test]
    fn a_flow_is_never_its_own_member() {
        let g = graph("agentflow-beta\nflow myFlow\n  a --> myFlow\nend");
        assert_eq!(cluster_parent(&g, "myFlow"), None);
        assert_eq!(parent_of(&g, "a").as_deref(), Some("myFlow"));
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn mutual_containment_keeps_the_first_nesting() {
        let g = graph(
            "agentflow-beta TB
  flow A[\"Flow A\"]
    a1[\"Task A1\"]
    a1 --> B
  end
  flow B[\"Flow B\"]
    b1[\"Task B1\"]
    b1 --> A
  end",
        );
        assert_eq!(cluster_parent(&g, "A").as_deref(), Some("B"));
        assert_eq!(cluster_parent(&g, "B"), None);
        assert_eq!(parent_of(&g, "a1").as_deref(), Some("A"));
        assert_eq!(parent_of(&g, "b1").as_deref(), Some("B"));
    }

    #[test]
    fn flow_direction_statements() {
        let src = "agentflow-beta TB
flow f [Steps]
  direction LR
  a --> b
  flow g
    direction BT
    c
  end
  global
    direction RL
    z
  end
end
flow h
  d
end
flow f
  e
end";
        let af = p(src);
        assert_eq!(flow(&af, "f").direction, Some(Direction::LeftRight));
        assert_eq!(flow(&af, "g").direction, Some(Direction::BottomTop));
        assert_eq!(flow(&af, "h").direction, None);
        let g = graph(src);
        let dir = |id: &str| g.clusters.iter().find(|c| c.id == id).unwrap().direction;
        assert_eq!(dir("f"), Some(Direction::LeftRight));
        assert_eq!(dir("h"), None);
        let inherited = to_graph(
            &af,
            &ConvertOptions {
                wrapping_width: 0.0,
                font_size: 16.0,
                palette_len: 12,
                inherit_dir: true,
            },
        );
        let dir = |id: &str| {
            inherited
                .clusters
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .direction
        };
        assert_eq!(dir("h"), Some(Direction::TopBottom));
        assert_eq!(dir("g"), Some(Direction::BottomTop));
        // A repeated id keeps the last direction given.
        let af = p("agentflow-beta\nflow f\ndirection LR\na\nend\nflow f\ndirection RL\nb\nend");
        assert_eq!(flow(&af, "f").direction, Some(Direction::RightLeft));
    }

    #[test]
    fn animated_edges_are_dashed() {
        let g = graph(
            "agentflow-beta\na e1@--> b\nb e2@--> c\nc e3@--> d\ne1@{ animate: true }\ne2@{ animation: slow }",
        );
        let dash: Vec<Option<Vec<f64>>> = g.edges.iter().map(|e| e.style.dash.clone()).collect();
        assert_eq!(dash, [Some(vec![9.0, 5.0]), Some(vec![9.0, 5.0]), None]);
    }

    #[test]
    fn collapsed_flow_folds_to_a_node_and_redirects_edges() {
        let g = graph(
            "agentflow-beta TB
  flow intake[\"Intake\"]
    receive[\"Receive request\"]@{ shape: input }
  end
  flow processing[\"Processing\"]
    validate --> enrich
  end
  processing@{ view: \"collapsed\" }
  receive --> validate
  enrich --> publish",
        );
        let n = g.nodes.iter().find(|n| n.id == "processing").unwrap();
        assert_eq!(n.shape, Shape::CollapsedGroup);
        assert!(
            g.nodes
                .iter()
                .all(|n| n.id != "validate" && n.id != "enrich")
        );
        assert!(g.clusters.iter().all(|c| c.id != "processing"));
        // validate --> enrich became a self-loop on the collapsed node and is dropped.
        let ids = edge_names(&g);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&("receive".into(), "processing".into())));
        assert!(ids.contains(&("processing".into(), "publish".into())));
        // Authored self-loops stay.
        assert_eq!(graph("agentflow-beta\na --> a").edges.len(), 1);
    }

    #[test]
    fn fan_out_and_edge_ids() {
        let af = p("agentflow-beta\nq --> o --> s & a & v\nx e1@--> y\ne1@{ curve: linear }");
        assert_eq!(af.edges.len(), 5);
        let e = af
            .edges
            .iter()
            .find(|e| e.id.as_deref() == Some("e1"))
            .unwrap();
        assert_eq!((e.start.as_str(), e.end.as_str()), ("x", "y"));
        assert_eq!(meta_get(&e.metadata, "curve"), Some("linear"));
        assert!(af.vertices.iter().all(|v| v.id != "e1"));
    }

    #[test]
    fn connector_metadata_does_not_change_its_shape() {
        let af = p("agentflow-beta LR
  connector github[\"GitHub API\"]
  github@{ protocol: \"http\", shape: tool }
  connector bare");
        let v = vertex(&af, "github");
        assert_eq!(v.kind(), VertexKind::Connector);
        assert_eq!(v.shape(), AfShape::RoundedRect);
        assert_eq!(meta_get(&v.metadata, "protocol"), Some("http"));
        assert_eq!(vertex(&af, "bare").label, "bare");
    }

    #[test]
    fn styles_classes_and_comments() {
        let af = p("agentflow-beta
classDef hot fill:#f00
a:::hot --> b %% trailing comment
style b stroke:#0f0
linkStyle 0 stroke:#00f");
        assert!(vertex(&af, "a").style.fill.is_some());
        assert!(vertex(&af, "b").style.stroke.is_some());
        assert!(af.edges[0].style.stroke.is_some());
        assert!(parse("agentflow-beta\na --> b\nlinkStyle 3 stroke:red", None).is_err());
    }

    #[test]
    fn palette_slots_follow_kind_and_container_order() {
        let g = graph("agentflow-beta\nflow one\n  t@{ shape: tool }\nend\nflow two\n  k\nend");
        let slot = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().color_slot;
        assert_eq!(slot("t"), Some(ColorSlot(0)));
        assert_eq!(slot("k"), Some(ColorSlot(1)));
        let cs: Vec<_> = g
            .clusters
            .iter()
            .map(|c| (c.id.as_str(), c.color_slot))
            .collect();
        assert_eq!(
            cs,
            vec![("one", Some(ColorSlot(7))), ("two", Some(ColorSlot(8)))]
        );
    }

    #[test]
    fn agentflow_defaults_to_redux_color_neo() {
        let cfg = MermaidConfig::default().scoped("agentflow");
        assert_eq!(cfg.theme, crate::theme::ThemeName::ReduxColor);
        assert_eq!(cfg.look, crate::config::Look::Neo);
    }

    // ------------------------------------------------------------ layout invariants

    fn bbox(el: &Element, out: &mut Option<Rect>) {
        let mut add = |p: crate::geom::Point| {
            let r = out.get_or_insert(Rect::new(p.x, p.y, 0.0, 0.0));
            let (x0, y0) = (r.x.min(p.x), r.y.min(p.y));
            let (x1, y1) = (r.right().max(p.x), r.bottom().max(p.y));
            *r = Rect::new(x0, y0, x1 - x0, y1 - y0);
        };
        match el {
            Element::Group { children, .. } => children.iter().for_each(|c| bbox(c, out)),
            Element::Shape { geom, .. } => match geom {
                Geometry::Rect { rect, .. } => {
                    add(crate::geom::Point::new(rect.x, rect.y));
                    add(crate::geom::Point::new(rect.right(), rect.bottom()));
                }
                Geometry::Circle { c, .. } => add(*c),
                Geometry::Path(cmds) => {
                    for c in cmds {
                        match c {
                            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => add(*p),
                            PathCmd::CubicTo(_, _, p) => add(*p),
                            PathCmd::Close => {}
                        }
                    }
                }
            },
            Element::Text { .. } | Element::Rotated { .. } => {}
        }
    }

    /// Bounding boxes of every group whose id starts with `prefix`, keyed by the rest.
    fn boxes(els: &[Element], prefix: &str, out: &mut Vec<(String, Rect)>) {
        for el in els {
            if let Element::Group { id, children, .. } = el {
                if let Some(rest) = id.as_deref().and_then(|i| i.strip_prefix(prefix)) {
                    let mut r = None;
                    bbox(el, &mut r);
                    if let Some(r) = r {
                        out.push((rest.to_string(), r));
                    }
                }
                boxes(children, prefix, out);
            }
        }
    }

    fn contains(outer: &Rect, inner: &Rect) -> bool {
        inner.x >= outer.x - 0.5
            && inner.y >= outer.y - 0.5
            && inner.right() <= outer.right() + 0.5
            && inner.bottom() <= outer.bottom() + 0.5
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.x < b.right() - 0.5
            && b.x < a.right() - 0.5
            && a.y < b.bottom() - 0.5
            && b.y < a.bottom() - 0.5
    }

    const WORKED: &str = "agentflow-beta TB
  connector llm[\"LLM API\"]
  flow coffee_team[\"Coffee Team\"]
    city[\"city\"]@{ shape: input, value: \"Stockholm\" }
    flow researcher[\"Researcher\"]
      research[\"research_location\"]@{ shape: tool }
      write[\"write_copy\"]@{ shape: tool }
      city --> research --> write
    end
    flow designer[\"Designer\"]
      render[\"generate_html\"]@{ shape: tool }
      brand[\"Nordic Brand Guide\"]@{ shape: refdoc }
      render -.- brand
    end
    researcher --> designer
  end
  outside[\"Outside\"] --> coffee_team";

    #[test]
    fn layout_nests_members_inside_their_flows() {
        let cfg = MermaidConfig::default().scoped("agentflow");
        let pal = cfg.palette();
        let scene = render(WORKED, None, &cfg, &pal).unwrap();
        assert_eq!(scene.kind, DiagramKind::Agentflow);
        let mut clusters = Vec::new();
        boxes(&scene.elements, "cluster-", &mut clusters);
        let mut nodes = Vec::new();
        boxes(&scene.elements, "agentflow-", &mut nodes);
        let cl = |id: &str| clusters.iter().find(|(c, _)| c == id).unwrap().1;
        let nd = |id: &str| {
            nodes
                .iter()
                .find(|(n, _)| n.rsplit_once('-').map(|(a, _)| a) == Some(id))
                .unwrap()
                .1
        };
        let af = p(WORKED);
        let g = to_graph(
            &af,
            &ConvertOptions {
                wrapping_width: 120.0,
                font_size: pal.font_size,
                palette_len: 12,
                inherit_dir: false,
            },
        );
        // Every node sits inside its cluster and every cluster inside its parent.
        for n in &g.nodes {
            if let Some(c) = n.cluster {
                assert!(contains(&cl(&g.clusters[c.0].id), &nd(&n.id)), "{}", n.id);
            } else {
                for c in &g.clusters {
                    assert!(!overlaps(&cl(&c.id), &nd(&n.id)), "{} in {}", n.id, c.id);
                }
            }
        }
        for c in &g.clusters {
            if let Some(p) = c.parent {
                assert!(contains(&cl(&g.clusters[p.0].id), &cl(&c.id)), "{}", c.id);
            }
        }
        // Sibling flows do not overlap, and `researcher --> designer` (TB) puts the
        // designer flow below the researcher flow.
        let (r, d) = (cl("researcher"), cl("designer"));
        assert!(!overlaps(&r, &d));
        assert!(d.y >= r.bottom() - 0.5);
        // Nodes never overlap.
        for (i, (a, ra)) in nodes.iter().enumerate() {
            for (b, rb) in &nodes[i + 1..] {
                assert!(!overlaps(ra, rb), "{a} / {b}");
            }
        }
        // minNodeWidth: every node label is at least 120 wide.
        assert!(nd("city").w >= 120.0);
    }
}
