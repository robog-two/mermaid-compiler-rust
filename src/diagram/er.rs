//! `erDiagram` diagrams → `GraphDiagram` with entity-table nodes and crow's-foot edges.
//!
//! Supported syntax (mermaid 11): entities with optional aliases (`p[Person]`,
//! `p["Person"]`, quoted names), attribute blocks (`type name PK, FK "comment"`,
//! generic types `type~T~`, back-quoted words), relationships with symbolic
//! (`|o`, `||`, `}o`, `}|`, `o|`, `o{`, `|{`) or word cardinalities (`one or more`,
//! `zero or many`, `only one`, `1+`, `0+`, `many(0)`, `1`, ...), identifying
//! (`--`, `to`) and non-identifying (`..`, `.-`, `-.`, `optionally to`) links,
//! `direction`, `classDef`, `class`, `style`, `:::class` and `title`, and
//! (mermaid 11.17+) `subgraph id` / `subgraph id [title]` … `end` groups, nested,
//! with their own `direction`, styled by `style` / `class`, and usable as
//! relationship endpoints by id.

use super::graph::StyleOverrides;
use super::graph::{
    self, Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Section,
    Shape,
};
use crate::config::{Curve, GraphConfig, MermaidConfig};
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::{DiagramKind, Scene};
use crate::theme::{ColorSlot, Palette};
use std::collections::HashMap;

/// How many instances of an entity may take part in a relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    ZeroOrOne,
    ExactlyOne,
    ZeroOrMore,
    OneOrMore,
    /// `u`: parent in a multi-dimensional relationship (drawn as exactly one).
    MdParent,
}

impl Cardinality {
    fn head(self) -> Head {
        match self {
            Cardinality::ZeroOrOne => Head::ErZeroOrOne,
            Cardinality::ExactlyOne | Cardinality::MdParent => Head::ErOnlyOne,
            Cardinality::ZeroOrMore => Head::ErZeroOrMore,
            Cardinality::OneOrMore => Head::ErOneOrMore,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub ty: String,
    pub name: String,
    pub keys: Vec<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entity {
    pub name: String,
    pub alias: Option<String>,
    pub attributes: Vec<Attribute>,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relationship {
    pub from: String,
    pub to: String,
    /// Cardinality drawn at the `from` entity.
    pub from_card: Cardinality,
    /// Cardinality drawn at the `to` entity.
    pub to_card: Cardinality,
    pub identifying: bool,
    pub label: String,
}

/// A `subgraph … end` group (erDb's `ErSubGraph`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubGraph {
    pub id: String,
    pub title: String,
    /// Names mentioned directly inside the group (entities and nested subgraph ids),
    /// first mention first, without names an earlier-closed group already claimed.
    pub members: Vec<String>,
    /// The group's own `direction` statement (the last one wins).
    pub direction: Option<Direction>,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErDiagram {
    pub title: Option<String>,
    pub direction: Direction,
    pub entities: Vec<Entity>,
    pub relationships: Vec<Relationship>,
    pub class_defs: HashMap<String, StyleOverrides>,
    /// Subgraphs in closing order (inner groups before the groups around them),
    /// as erDb's `addSubGraph` runs on `end`.
    pub subgraphs: Vec<SubGraph>,
}

impl ErDiagram {
    fn entity(&mut self, name: &str) -> &mut Entity {
        let i = match self.entities.iter().position(|e| e.name == name) {
            Some(i) => i,
            None => {
                self.entities.push(Entity {
                    name: name.to_string(),
                    ..Entity::default()
                });
                self.entities.len() - 1
            }
        };
        &mut self.entities[i]
    }

    fn subgraph_index(&self, id: &str) -> Option<usize> {
        // erDb's `subGraphLookup`: a repeated id resolves to the latest group.
        self.subgraphs.iter().rposition(|g| g.id == id)
    }
}

/// A subgraph between its header and its `end`.
struct OpenSubGraph {
    id: String,
    title: String,
    members: Vec<String>,
    direction: Option<Direction>,
    line: usize,
}

/// `subgraph id`, `subgraph "quoted id"` or `subgraph id [title words]`.
fn subgraph_header(rest: &str, line: usize) -> Result<(String, String)> {
    let mut c = Cur::new(rest.trim());
    let id = entity_name(&mut c)
        .ok_or_else(|| Error::parse(line, "Expecting a subgraph id after 'subgraph'"))?;
    c.ws();
    if c.at_end() {
        return Ok((id.clone(), id));
    }
    if !c.eat("[") {
        return Err(Error::parse(
            line,
            format!("Unexpected '{}' after the subgraph id", c.rest().trim()),
        ));
    }
    let r = c.rest();
    let end = r
        .find(']')
        .ok_or_else(|| Error::parse(line, "Expecting ']' to close the subgraph title"))?;
    if !r[end + 1..].trim().is_empty() {
        return Err(Error::parse(
            line,
            format!(
                "Unexpected '{}' after the subgraph title",
                r[end + 1..].trim()
            ),
        ));
    }
    // `subgraphTitle`: entity names separated by whitespace, joined by single spaces.
    let mut words = Vec::new();
    let mut t = Cur::new(&r[..end]);
    loop {
        t.ws();
        if t.at_end() {
            break;
        }
        match entity_name(&mut t) {
            Some(w) => words.push(w),
            None => {
                return Err(Error::parse(
                    line,
                    format!("Unexpected '{}' in the subgraph title", t.rest().trim()),
                ));
            }
        }
    }
    if words.is_empty() {
        return Err(Error::parse(
            line,
            "Expecting a subgraph title inside '[ ]'",
        ));
    }
    Ok((id, words.join(" ")))
}

/// Parse and render a `erDiagram` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let mut g = to_graph(&d);
    wrap_plain_entities(&mut g, cfg.flowchart.wrapping_width, pal.font_size);
    Ok(graph::render(&g, &graph_config(cfg), pal))
}

/// An entity without attributes is drawn by `drawRect`, whose `labelHelper` wraps the
/// name at `flowchart.wrappingWidth` (`DELIVERY-` / `ADDRESS`); a table's name does
/// not wrap.
fn wrap_plain_entities(g: &mut GraphDiagram, width: f64, font: f64) {
    if width <= 0.0 {
        return;
    }
    for n in g.nodes.iter_mut().filter(|n| n.sections.is_empty()) {
        let bold = n.style.bold == Some(true);
        let lines = super::timeline::wrap_text(&n.label, width, font, bold);
        // Only a label that wraps is rewritten (it then holds normalised text).
        if lines.len() > crate::text::split_lines(&n.label).len() {
            n.label = lines.join("<br>");
            n.wrapped_width = Some(width);
        }
    }
}

/// Layout settings from the `er` config section.
fn graph_config(cfg: &MermaidConfig) -> GraphConfig {
    GraphConfig {
        look: cfg.look,
        node_spacing: cfg.section_f64("er", "nodeSpacing", 140.0),
        rank_spacing: cfg.section_f64("er", "rankSpacing", 80.0),
        padding: cfg.section_f64("er", "entityPadding", 15.0),
        diagram_padding: cfg.section_f64("er", "diagramPadding", 20.0),
        curve: Curve::Basis,
        use_max_width: cfg.use_max_width("er"),
        title_top_margin: cfg.section_f64("er", "titleTopMargin", 25.0),
        min_node_width: 0.0,
        wrapping_width: 0.0,
    }
}

// ---------------------------------------------------------------- lexing helpers

/// A tiny cursor over one line.
struct Cur<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Self {
        Cur { s, pos: 0 }
    }
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }
    fn ws(&mut self) {
        let r = self.rest();
        self.pos += r.len() - r.trim_start().len();
    }
    fn eat(&mut self, lit: &str) -> bool {
        if self.rest().starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }
    /// Case-insensitive keyword match; word keywords need a word boundary after them.
    fn eat_kw(&mut self, kw: &str) -> bool {
        let r = self.rest();
        if r.len() < kw.len() || !r.is_char_boundary(kw.len()) {
            return false;
        }
        if !r[..kw.len()].eq_ignore_ascii_case(kw) {
            return false;
        }
        let last_is_word = kw.chars().last().is_some_and(|c| c.is_alphanumeric());
        if last_is_word {
            let next = r[kw.len()..].chars().next();
            if next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                return false;
            }
        }
        self.pos += kw.len();
        true
    }
    fn at_end(&self) -> bool {
        self.rest().trim().is_empty()
    }
}

fn is_name_char(c: char) -> bool {
    !c.is_ascii() || c.is_alphanumeric() || c == '_' || c == '-' || c == '*' || c == '.'
}

/// Entity name: `"quoted name"` or a run of word characters.
fn entity_name(c: &mut Cur) -> Option<String> {
    let r = c.rest();
    if let Some(q) = r.strip_prefix('"') {
        let end = q.find('"')?;
        c.pos += end + 2;
        return Some(q[..end].to_string());
    }
    let len: usize = r
        .chars()
        .take_while(|&ch| is_name_char(ch))
        .map(char::len_utf8)
        .sum();
    // A trailing `-` / `.` belongs to a relationship operator (`A--`), not the name.
    let mut name = &r[..len];
    while name.ends_with("--")
        || name.ends_with("..")
        || name.ends_with(".-")
        || name.ends_with("-.")
    {
        name = &name[..name.len() - 2];
    }
    if name.is_empty() {
        return None;
    }
    c.pos += name.len();
    Some(name.to_string())
}

/// `name`, `name[Alias]`, `name["Alias"]`, each optionally followed by `:::cls`.
struct EntityRef {
    name: String,
    alias: Option<String>,
    classes: Vec<String>,
}

fn entity_ref(c: &mut Cur, line: usize) -> Result<Option<EntityRef>> {
    let Some(name) = entity_name(c) else {
        return Ok(None);
    };
    let mut alias = None;
    let save = c.pos;
    c.ws();
    if c.eat("[") {
        let r = c.rest();
        let end = r
            .find(']')
            .ok_or_else(|| Error::parse(line, "Expecting ']' to close the entity alias"))?;
        alias = Some(r[..end].trim().trim_matches('"').to_string());
        c.pos += end + 1;
    } else {
        c.pos = save;
    }
    let mut classes = Vec::new();
    if c.eat(":::") {
        let r = c.rest();
        let len: usize = r
            .chars()
            .take_while(|&ch| is_name_char(ch) || ch == ',')
            .map(char::len_utf8)
            .sum();
        classes = r[..len]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        c.pos += len;
    }
    Ok(Some(EntityRef {
        name,
        alias,
        classes,
    }))
}

fn cardinality(c: &mut Cur) -> Option<Cardinality> {
    use Cardinality::*;
    const TABLE: &[(&str, Cardinality)] = &[
        ("only one", ExactlyOne),
        ("one or zero", ZeroOrOne),
        ("zero or one", ZeroOrOne),
        ("one or more", OneOrMore),
        ("one or many", OneOrMore),
        ("zero or more", ZeroOrMore),
        ("zero or many", ZeroOrMore),
        ("many(0)", ZeroOrMore),
        ("many(1)", OneOrMore),
        ("many", ZeroOrMore),
        ("one", ExactlyOne),
        ("1+", OneOrMore),
        ("0+", ZeroOrMore),
        ("|o", ZeroOrOne),
        ("o|", ZeroOrOne),
        ("}o", ZeroOrMore),
        ("o{", ZeroOrMore),
        ("}|", OneOrMore),
        ("|{", OneOrMore),
        ("||", ExactlyOne),
        ("1", ExactlyOne),
    ];
    for (kw, card) in TABLE {
        if c.eat_kw(kw) {
            return Some(*card);
        }
    }
    let r = c.rest();
    if (r.starts_with('u') || r.starts_with('U')) && r[1..].starts_with(['.', '-', '|']) {
        c.pos += 1;
        return Some(MdParent);
    }
    None
}

/// `true` for identifying.
fn rel_type(c: &mut Cur) -> Option<bool> {
    if c.eat_kw("optionally to") {
        return Some(false);
    }
    if c.eat_kw("to") || c.eat("--") {
        return Some(true);
    }
    if c.eat("..") || c.eat(".-") || c.eat("-.") {
        return Some(false);
    }
    None
}

// ---------------------------------------------------------------- attribute blocks

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Key(String),
    Comma,
    Comment(String),
    Close,
}

/// Tokenise the inside of an attribute block line. Returns the tokens and the
/// remainder after a closing `}` (if any).
fn block_tokens(s: &str, line: usize) -> Result<(Vec<Tok>, Option<String>)> {
    let mut out = Vec::new();
    let mut c = Cur::new(s);
    loop {
        c.ws();
        let r = c.rest();
        let Some(ch) = r.chars().next() else {
            return Ok((out, None));
        };
        match ch {
            '}' => {
                out.push(Tok::Close);
                return Ok((out, Some(r[1..].to_string())));
            }
            ',' => {
                out.push(Tok::Comma);
                c.pos += 1;
            }
            '"' => {
                let end = r[1..]
                    .find('"')
                    .ok_or_else(|| Error::parse(line, "Unterminated attribute comment"))?;
                out.push(Tok::Comment(r[1..1 + end].to_string()));
                c.pos += end + 2;
            }
            '`' => {
                let end = r[1..]
                    .find('`')
                    .ok_or_else(|| Error::parse(line, "Unterminated back-quoted attribute"))?;
                out.push(Tok::Word(r[1..1 + end].to_string()));
                c.pos += end + 2;
            }
            _ => {
                let key = ["PK", "FK", "UK"].iter().find(|k| {
                    r.len() >= 2
                        && r[..2].eq_ignore_ascii_case(k)
                        && !r[2..]
                            .chars()
                            .next()
                            .is_some_and(|n| n.is_alphanumeric() || n == '_')
                });
                if let Some(k) = key {
                    out.push(Tok::Key(k.to_string()));
                    c.pos += 2;
                    continue;
                }
                if !(ch == '*' || ch == '_' || ch.is_alphabetic() || !ch.is_ascii()) {
                    return Err(Error::parse(
                        line,
                        format!("Unexpected character '{ch}' in attribute block"),
                    ));
                }
                let len: usize = r
                    .chars()
                    .take_while(|&x| !x.is_whitespace() && x != '"' && x != '}' && x != '{')
                    .map(char::len_utf8)
                    .sum();
                out.push(Tok::Word(r[..len].to_string()));
                c.pos += len;
            }
        }
    }
}

/// `type~T~` → `type<T>`.
fn generic_text(s: &str) -> String {
    let mut out = String::new();
    let mut open = true;
    for c in s.chars() {
        if c == '~' {
            out.push(if open { '<' } else { '>' });
            open = !open;
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_attributes(toks: &[(usize, Tok)]) -> Result<Vec<Attribute>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let (line, ty) = match &toks[i] {
            (l, Tok::Word(w)) => (*l, w.clone()),
            (l, t) => {
                return Err(Error::parse(
                    *l,
                    format!("Expecting an attribute type, got {}", describe(t)),
                ));
            }
        };
        i += 1;
        let name = match toks.get(i) {
            Some((_, Tok::Word(w))) => w.clone(),
            Some((l, t)) => {
                return Err(Error::parse(
                    *l,
                    format!("Expecting an attribute name, got {}", describe(t)),
                ));
            }
            None => return Err(Error::parse(line, "Expecting an attribute name")),
        };
        i += 1;
        let mut keys = Vec::new();
        while let Some((_, Tok::Key(k))) = toks.get(i) {
            keys.push(k.clone());
            i += 1;
            if let Some((l, Tok::Comma)) = toks.get(i) {
                i += 1;
                if !matches!(toks.get(i), Some((_, Tok::Key(_)))) {
                    return Err(Error::parse(*l, "Expecting PK, FK or UK after ','"));
                }
            }
        }
        let mut comment = None;
        if let Some((_, Tok::Comment(c))) = toks.get(i) {
            comment = Some(c.clone());
            i += 1;
        }
        if let Some((l, Tok::Comma)) = toks.get(i) {
            return Err(Error::parse(*l, "Unexpected ','"));
        }
        out.push(Attribute {
            ty: generic_text(&ty),
            name,
            keys,
            comment,
        });
    }
    Ok(out)
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Word(w) => format!("'{w}'"),
        Tok::Key(k) => format!("key '{k}'"),
        Tok::Comma => "','".into(),
        Tok::Comment(_) => "a comment".into(),
        Tok::Close => "'}'".into(),
    }
}

// ---------------------------------------------------------------- statements

fn split_ids(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Open attribute block: entity name, tokens so far, opening line.
type OpenBlock = (String, Vec<(usize, Tok)>, usize);

/// Parse an ER diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<ErDiagram> {
    let mut d = ErDiagram {
        title,
        direction: Direction::TopBottom,
        entities: Vec::new(),
        relationships: Vec::new(),
        class_defs: HashMap::new(),
        subgraphs: Vec::new(),
    };
    let mut header = false;
    let mut open: Vec<OpenSubGraph> = Vec::new();
    // Open attribute block: entity name, tokens so far, opening line.
    let mut block: Option<OpenBlock> = None;
    let mut styles: Vec<(String, StyleOverrides)> = Vec::new();
    let mut class_assign: Vec<(String, String)> = Vec::new();

    for (idx, raw) in body.lines().enumerate() {
        let line = idx + 1;
        let mut text = raw.trim().to_string();
        if let Some((name, toks, _)) = block.as_mut() {
            let (more, rest) = block_tokens(&text, line)?;
            let closed = rest.is_some();
            toks.extend(
                more.into_iter()
                    .filter(|t| *t != Tok::Close)
                    .map(|t| (line, t)),
            );
            if !closed {
                continue;
            }
            let attrs = parse_attributes(toks)?;
            let name = name.clone();
            d.entity(&name).attributes.extend(attrs);
            block = None;
            text = rest.unwrap_or_default().trim().to_string();
        }
        let s = text.trim().trim_end_matches(';').trim();
        if s.is_empty() {
            continue;
        }
        if !header {
            let rest = s
                .strip_prefix("erDiagram")
                .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting 'erDiagram'")))?;
            header = true;
            // The grammar is token-based: a statement may follow on the same line.
            if rest.trim().is_empty() || !rest.starts_with(char::is_whitespace) {
                if !rest.trim().is_empty() {
                    return Err(Error::parse(
                        line,
                        format!("Unexpected '{}' after 'erDiagram'", rest.trim()),
                    ));
                }
                continue;
            }
            text = rest.trim().to_string();
        }
        let s = text.trim().trim_end_matches(';').trim();
        let lower = s.to_ascii_lowercase();
        if let Some(dir) = lower.strip_prefix("direction ") {
            let dir = Direction::parse(&dir.trim().to_ascii_uppercase())
                .ok_or_else(|| Error::parse(line, format!("Invalid direction '{}'", dir.trim())))?;
            // Inside a subgraph `direction` belongs to the innermost open group.
            match open.last_mut() {
                Some(g) => g.direction = Some(dir),
                None => d.direction = dir,
            }
            continue;
        }
        if lower == "end" {
            let g = open
                .pop()
                .ok_or_else(|| Error::parse(line, "Unexpected 'end' outside a subgraph"))?;
            close_subgraph(&mut d, g);
            if let Some(parent) = open.last_mut() {
                // `subgraphHeader document END` yields the id into the enclosing document.
                let id = d.subgraphs.last().map(|g| g.id.clone()).unwrap_or_default();
                parent.members.push(id);
            }
            continue;
        }
        if lower == "subgraph"
            || (lower.starts_with("subgraph") && s[8..].starts_with(char::is_whitespace))
        {
            let (id, title) = subgraph_header(&s[8..], line)?;
            open.push(OpenSubGraph {
                id,
                title,
                members: Vec::new(),
                direction: None,
                line,
            });
            continue;
        }
        if lower.starts_with("title ") || lower == "title" {
            d.title = Some(s[5..].trim().to_string()).filter(|t| !t.is_empty());
            continue;
        }
        if let Some(rest) = s.strip_prefix("classDef ") {
            let (names, st) = rest
                .trim()
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting styles after classDef names"))?;
            for n in split_ids(names) {
                d.class_defs.insert(n, StyleOverrides::parse(st));
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("class ") {
            let (ids, cls) = rest
                .trim()
                .rsplit_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting 'class <ids> <className>'"))?;
            for id in split_ids(ids) {
                class_assign.push((id, cls.trim().to_string()));
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("style ") {
            let (ids, st) = rest
                .trim()
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting styles after style ids"))?;
            for id in split_ids(ids) {
                styles.push((id, StyleOverrides::parse(st)));
            }
            continue;
        }
        let (names, opened) = statement(&mut d, s, line)?;
        if let Some(g) = open.last_mut() {
            g.members.extend(names);
        }
        block = opened.map(|name| (name, Vec::new(), line));
        // A block may open and close on the same line: `A { string x }`.
        if let Some((name, toks, _)) = block.as_mut() {
            let after = s.split_once('{').map(|x| x.1).unwrap_or("");
            let (more, rest) = block_tokens(after, line)?;
            let closed = rest.is_some();
            toks.extend(
                more.into_iter()
                    .filter(|t| *t != Tok::Close)
                    .map(|t| (line, t)),
            );
            if closed {
                if let Some(r) = rest.filter(|r| !r.trim().is_empty()) {
                    return Err(Error::parse(
                        line,
                        format!("Unexpected '{}' after '}}'", r.trim()),
                    ));
                }
                let attrs = parse_attributes(toks)?;
                let name = name.clone();
                d.entity(&name).attributes.extend(attrs);
                block = None;
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'erDiagram'"));
    }
    if let Some((name, _, open)) = block {
        return Err(Error::parse(
            open,
            format!("Unterminated attribute block for entity '{name}': missing '}}'"),
        ));
    }
    if let Some(g) = open.last() {
        return Err(Error::parse(
            g.line,
            format!("Unterminated subgraph '{}': missing 'end'", g.id),
        ));
    }
    for (id, cls) in class_assign {
        if let Some(e) = d.entities.iter_mut().find(|e| e.name == id) {
            e.classes.push(cls.clone());
        }
        if let Some(i) = d.subgraph_index(&id) {
            d.subgraphs[i].classes.push(cls);
        }
    }
    // Subgraphs take only their own classes (no `default`), then `style` statements.
    for g in d.subgraphs.iter_mut() {
        let mut st = StyleOverrides::default();
        for c in &g.classes {
            if let Some(def) = d.class_defs.get(c) {
                st = st.merged(def);
            }
        }
        g.style = st;
    }
    for e in d.entities.iter_mut() {
        let mut st = d.class_defs.get("default").cloned().unwrap_or_default();
        for c in &e.classes {
            if let Some(def) = d.class_defs.get(c) {
                st = st.merged(def);
            }
        }
        e.style = st;
    }
    for (id, st) in styles {
        if let Some(e) = d.entities.iter_mut().find(|e| e.name == id) {
            e.style = e.style.merged(&st);
        }
        if let Some(i) = d.subgraph_index(&id) {
            d.subgraphs[i].style = d.subgraphs[i].style.merged(&st);
        }
    }
    Ok(d)
}

/// erDb's `addSubGraph`: dedupe the members and drop names an earlier-closed group
/// already holds (`makeUniq`), so every entity belongs to at most one group.
fn close_subgraph(d: &mut ErDiagram, g: OpenSubGraph) {
    let mut members: Vec<String> = Vec::new();
    for m in g.members {
        let m = m.trim().to_string();
        if m.is_empty() || members.contains(&m) {
            continue;
        }
        if d.subgraphs.iter().any(|o| o.members.contains(&m)) {
            continue;
        }
        members.push(m);
    }
    d.subgraphs.push(SubGraph {
        id: g.id,
        title: g.title,
        members,
        direction: g.direction,
        ..SubGraph::default()
    });
}

fn register(d: &mut ErDiagram, r: &EntityRef) {
    let e = d.entity(&r.name);
    if r.alias.is_some() {
        e.alias = r.alias.clone();
    }
    for c in &r.classes {
        if !e.classes.contains(c) {
            e.classes.push(c.clone());
        }
    }
}

/// One entity/relationship statement. Returns the entity names it mentions (what the
/// grammar yields into an enclosing subgraph) and the entity name when it opens a block.
fn statement(d: &mut ErDiagram, s: &str, line: usize) -> Result<(Vec<String>, Option<String>)> {
    let mut c = Cur::new(s);
    let a = entity_ref(&mut c, line)?
        .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting an entity name")))?;
    c.ws();
    if c.at_end() {
        register(d, &a);
        return Ok((vec![a.name], None));
    }
    if c.eat("{") {
        register(d, &a);
        return Ok((vec![a.name.clone()], Some(a.name)));
    }
    // erDiagram.jison only takes `name[Alias]` on an entity declaration (with or
    // without a block), not as a relationship end.
    let no_alias = |r: &EntityRef| -> Result<()> {
        match r.alias {
            Some(_) => Err(Error::parse(
                line,
                format!("{s}\nAn entity alias is only allowed when declaring the entity"),
            )),
            None => Ok(()),
        }
    };
    no_alias(&a)?;
    let left = cardinality(&mut c).ok_or_else(|| {
        Error::parse(
            line,
            format!(
                "{s}\nExpecting a cardinality (e.g. '||', '|o', '}}o', '}}|') or '{{', got '{}'",
                c.rest().trim()
            ),
        )
    })?;
    c.ws();
    let identifying = rel_type(&mut c).ok_or_else(|| {
        Error::parse(
            line,
            format!("{s}\nExpecting '--', '..', 'to' or 'optionally to'"),
        )
    })?;
    c.ws();
    let right = cardinality(&mut c).ok_or_else(|| {
        Error::parse(
            line,
            format!("{s}\nExpecting a cardinality after the relationship type"),
        )
    })?;
    c.ws();
    let b = entity_ref(&mut c, line)?
        .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting the second entity name")))?;
    no_alias(&b)?;
    c.ws();
    if !c.eat(":") {
        return Err(Error::parse(
            line,
            format!("{s}\nExpecting ':' followed by a relationship label"),
        ));
    }
    let label = c.rest().trim();
    let label = label
        .strip_prefix('"')
        .and_then(|l| l.strip_suffix('"'))
        .unwrap_or(label)
        .to_string();
    // erDb's `addRelationship`: an endpoint naming a (closed) subgraph is that group.
    for r in [&a, &b] {
        if d.subgraph_index(&r.name).is_none() {
            register(d, r);
        }
    }
    let names = vec![a.name.clone(), b.name.clone()];
    d.relationships.push(Relationship {
        from: a.name,
        to: b.name,
        from_card: left,
        to_card: right,
        identifying,
        label,
    });
    Ok((names, None))
}

// ---------------------------------------------------------------- graph

/// Lower the ER IR to the shared graph IR. Subgraphs become clusters (erDb's
/// `getData`): an entity or group sits in the group whose member list names it, and a
/// relationship whose endpoint names a group attaches to that group's frame.
pub fn to_graph(d: &ErDiagram) -> GraphDiagram {
    // `parentDB`: member name -> index of the group holding it.
    let mut parent_of: HashMap<&str, usize> = HashMap::new();
    for (gi, g) in d.subgraphs.iter().enumerate().rev() {
        for m in &g.members {
            parent_of.insert(m.as_str(), gi);
        }
    }
    let cluster_of = |name: &str| parent_of.get(name).map(|&gi| ClusterId(gi));
    let mut clusters: Vec<Cluster> = Vec::with_capacity(d.subgraphs.len());
    for g in &d.subgraphs {
        let mut c = Cluster::new(g.id.clone(), g.title.clone(), None);
        c.direction = g.direction;
        c.style = g.style.clone();
        clusters.push(c);
    }
    for gi in 0..clusters.len() {
        let id = &d.subgraphs[gi].id;
        // Only the latest group of a repeated id is the one its name resolves to.
        if d.subgraph_index(id) != Some(gi) {
            continue;
        }
        let Some(p) = cluster_of(id) else { continue };
        // A group cannot sit inside itself (a name yielded into its own descendant).
        let mut cur = Some(p);
        let mut cyclic = false;
        for _ in 0..=clusters.len() {
            match cur {
                Some(c) if c.0 == gi => {
                    cyclic = true;
                    break;
                }
                Some(c) => cur = clusters[c.0].parent,
                None => break,
            }
        }
        if !cyclic {
            clusters[gi].parent = Some(p);
        }
    }

    // Entities named like a group are that group, not a node of their own.
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    for e in &d.entities {
        if d.subgraph_index(&e.name).is_some() {
            continue;
        }
        let mut n = Node::new(
            e.name.clone(),
            e.alias.clone().unwrap_or_else(|| e.name.clone()),
            Shape::EntityTable,
        );
        // erDb: every entity takes the next slot, in first-mention order; groups
        // take none.
        n.color_slot = Some(ColorSlot(nodes.len()));
        n.style = e.style.clone();
        n.classes = e.classes.clone();
        n.cluster = cluster_of(&e.name);
        n.sections = e
            .attributes
            .iter()
            .map(|a| Section {
                lines: vec![
                    a.ty.clone(),
                    a.name.clone(),
                    a.keys.join(", "),
                    a.comment.clone().unwrap_or_default(),
                ],
                centered: false,
                bold: false,
            })
            .collect();
        index.insert(e.name.as_str(), nodes.len());
        nodes.push(n);
    }
    let endpoint = |name: &str| -> Option<Endpoint> {
        match d.subgraph_index(name) {
            Some(gi) => Some(Endpoint::Cluster(ClusterId(gi))),
            None => index.get(name).map(|&i| Endpoint::Node(NodeId(i))),
        }
    };
    let edges = d
        .relationships
        .iter()
        .filter_map(|r| {
            let mut e = Edge::new(endpoint(&r.from)?, endpoint(&r.to)?);
            e.start = r.from_card.head();
            e.end = r.to_card.head();
            e.label = Some(r.label.clone()).filter(|l| !l.trim().is_empty());
            if !r.identifying {
                e.line = LineKind::Dashed;
            }
            Some(e)
        })
        .collect();
    GraphDiagram {
        kind: DiagramKind::Er,
        direction: d.direction,
        nodes,
        edges,
        clusters,
        title: d.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{PathCmd, Point, Rect};
    use crate::scene::{Element, Geometry};

    fn err_line(src: &str) -> usize {
        match parse(src, None) {
            Err(Error::Parse { line, .. }) => line,
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn parses_relationships_and_cardinalities() {
        let d = parse(
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE-ITEM : contains\n  CUSTOMER }|..|{ DELIVERY-ADDRESS : uses\n  A |o--o| B : \"two words\"",
            None,
        )
        .unwrap();
        assert_eq!(d.entities.len(), 6);
        let r = &d.relationships;
        assert_eq!(r[0].from_card, Cardinality::ExactlyOne);
        assert_eq!(r[0].to_card, Cardinality::ZeroOrMore);
        assert!(r[0].identifying);
        assert_eq!(r[1].to, "LINE-ITEM");
        assert_eq!(r[1].to_card, Cardinality::OneOrMore);
        assert_eq!(r[2].from_card, Cardinality::OneOrMore);
        assert!(!r[2].identifying);
        assert_eq!(r[3].from_card, Cardinality::ZeroOrOne);
        assert_eq!(r[3].to_card, Cardinality::ZeroOrOne);
        assert_eq!(r[3].label, "two words");
    }

    #[test]
    fn parses_word_cardinalities() {
        let d = parse(
            "erDiagram\nCAR 1 to zero or more NAMED-DRIVER : allows\nPERSON many(0) optionally to 1+ X : has\nA one or zero -- only one B : x\nC||--||D:y",
            None,
        )
        .unwrap();
        let r = &d.relationships;
        assert_eq!(
            (r[0].from_card, r[0].to_card, r[0].identifying),
            (Cardinality::ExactlyOne, Cardinality::ZeroOrMore, true)
        );
        assert_eq!(
            (r[1].from_card, r[1].to_card, r[1].identifying),
            (Cardinality::ZeroOrMore, Cardinality::OneOrMore, false)
        );
        assert_eq!(r[2].from_card, Cardinality::ZeroOrOne);
        assert_eq!(r[2].to_card, Cardinality::ExactlyOne);
        assert_eq!((r[3].from.as_str(), r[3].to.as_str()), ("C", "D"));
    }

    #[test]
    fn parses_attributes_keys_and_comments() {
        let d = parse(
            "erDiagram\nCAR {\n  string registrationNumber PK\n  string make \"maker\"\n  int ownerId FK, UK \"the owner\"\n  decimal(10,2) price\n  type~T~ generic\n}\nX { string a string b }",
            None,
        )
        .unwrap();
        let car = &d.entities[0];
        assert_eq!(car.attributes.len(), 5);
        assert_eq!(car.attributes[0].keys, vec!["PK"]);
        assert_eq!(car.attributes[1].comment.as_deref(), Some("maker"));
        assert_eq!(car.attributes[2].keys, vec!["FK", "UK"]);
        assert_eq!(car.attributes[2].comment.as_deref(), Some("the owner"));
        assert_eq!(car.attributes[3].ty, "decimal(10,2)");
        assert_eq!(car.attributes[4].ty, "type<T>");
        assert_eq!(d.entities[1].attributes.len(), 2);
    }

    #[test]
    fn parses_aliases_classes_and_styles() {
        let d = parse(
            "erDiagram\ndirection LR\np[Person] {\n string name\n}\na[\"Customer Account\"]:::hot\na ||--o{ p : owns\n\"Quoted Name\" { int x }\nclassDef hot fill:#f96\nstyle p stroke:#f00\nclass p hot",
            None,
        )
        .unwrap();
        assert_eq!(d.direction, Direction::LeftRight);
        assert_eq!(d.entities[0].alias.as_deref(), Some("Person"));
        assert_eq!(d.entities[1].alias.as_deref(), Some("Customer Account"));
        assert_eq!(d.entities[1].classes, vec!["hot"]);
        assert_eq!(d.entities[1].style.fill.as_ref().unwrap().css(), "#f96");
        assert_eq!(d.entities[0].style.stroke.as_ref().unwrap().css(), "#f00");
        assert_eq!(d.entities[0].style.fill.as_ref().unwrap().css(), "#f96");
        assert_eq!(d.entities[2].name, "Quoted Name");
    }

    #[test]
    fn reports_errors_with_lines() {
        assert_eq!(err_line("erDiagram\nA ||--o{ B : ok\nA ??--o{ B : x"), 3);
        assert_eq!(err_line("erDiagram\n\nA ||--o{ B"), 3);
        assert_eq!(err_line("erDiagram\nA ||-- B : x"), 2);
        assert_eq!(err_line("erDiagram\nA {\n string\n}"), 3);
        assert_eq!(err_line("erDiagram\nA {\n string x\n"), 2);
        assert_eq!(err_line("erDiagram\nA {\n string x PK,\n}"), 3);
        assert_eq!(err_line("flowchart\nA"), 1);
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(
            "erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER { string name PK }",
            None,
            &cfg,
            &pal,
        )
        .unwrap();
        assert_eq!(s.kind, DiagramKind::Er);
        assert!(s.view_box.w > 100.0 && s.view_box.h > 100.0);
    }

    // ------------------------------------------------------------ subgraphs
    // Ported from upstream `parser/subgraph.spec.js`.

    fn sg<'a>(d: &'a ErDiagram, id: &str) -> &'a SubGraph {
        d.subgraphs.iter().find(|g| g.id == id).unwrap()
    }

    #[test]
    fn subgraph_simple_nodes_and_id_title() {
        let d = parse("erDiagram\nsubgraph Group\nA\nB\nend", None).unwrap();
        assert_eq!(d.subgraphs.len(), 1);
        let g = &d.subgraphs[0];
        assert_eq!(g.members, vec!["A", "B"]);
        assert_eq!((g.id.as_str(), g.title.as_str()), ("Group", "Group"));
    }

    #[test]
    fn subgraph_quoted_and_explicit_titles() {
        let d = parse("erDiagram\nsubgraph \"Some Title\"\nA\nB\nend", None).unwrap();
        assert_eq!(d.subgraphs[0].id, "Some Title");
        assert_eq!(d.subgraphs[0].title, "Some Title");
        let d = parse("erDiagram\nsubgraph id [Title]\nA\nB\nend", None).unwrap();
        assert_eq!(
            (d.subgraphs[0].id.as_str(), d.subgraphs[0].title.as_str()),
            ("id", "Title")
        );
        let d = parse("erDiagram\nsubgraph id1 [title 1]\nCUSTOMER\nend", None).unwrap();
        assert_eq!(d.subgraphs[0].title, "title 1");
        let d = parse("erDiagram\nsubgraph \"Group 1.5\"\nA\nend", None).unwrap();
        assert_eq!(d.subgraphs[0].id, "Group 1.5");
        assert_eq!(d.subgraphs[0].members, vec!["A"]);
    }

    #[test]
    fn subgraph_blank_lines_trailing_space_and_empty() {
        let d = parse("erDiagram\nsubgraph Group\n\nA\n\nB\n\nend", None).unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A", "B"]);
        let d = parse("erDiagram\nsubgraph Group\nA\nend   \n", None).unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A"]);
        let d = parse("erDiagram\nsubgraph empty\nend", None).unwrap();
        assert!(d.subgraphs[0].members.is_empty());
        assert_eq!(d.subgraphs[0].title, "empty");
    }

    #[test]
    fn subgraph_direction_on_header_line() {
        let d = parse("erDiagram subgraph WithRL\ndirection RL\nA\nB\nend", None).unwrap();
        let g = &d.subgraphs[0];
        assert_eq!(g.direction, Some(Direction::RightLeft));
        assert_eq!(g.members, vec!["A", "B"]);
        assert_eq!(d.direction, Direction::TopBottom);
    }

    #[test]
    fn subgraph_relationship_endpoints() {
        // Both endpoints of a relationship inside a group are members.
        let d = parse(
            "erDiagram\nsubgraph Group\nA }|--|| B : \"relates to\"\nend",
            None,
        )
        .unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A", "B"]);
        assert_eq!(d.relationships.len(), 1);
        // Entity inside, other outside.
        let d = parse(
            "erDiagram\nsubgraph Group\nA\nend\nA }|--|| B : \"relates to\"\n",
            None,
        )
        .unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A"]);
        // Between groups: the endpoints are the groups, no entities are made for them.
        let d = parse(
            "erDiagram\nsubgraph G1\n  A\nend\nsubgraph G2\n  B\nend\nG1 }|--|| G2 : \"relates to\"\n",
            None,
        )
        .unwrap();
        assert_eq!(d.entities.len(), 2);
        let g = to_graph(&d);
        assert_eq!(g.edges[0].from, Endpoint::Cluster(ClusterId(0)));
        assert_eq!(g.edges[0].to, Endpoint::Cluster(ClusterId(1)));
        // Entities in different groups.
        let d = parse(
            "erDiagram\nsubgraph G1\n  A\nend\nsubgraph G2\n  B\nend\nA }|--|| B : \"relates to\"\n",
            None,
        )
        .unwrap();
        assert_eq!(
            (
                sg(&d, "G1").members[0].as_str(),
                sg(&d, "G2").members[0].as_str()
            ),
            ("A", "B")
        );
        let g = to_graph(&d);
        assert_eq!(g.nodes[0].cluster, Some(ClusterId(0)));
        assert_eq!(g.nodes[1].cluster, Some(ClusterId(1)));
        // Entity and group.
        let d = parse(
            "erDiagram\nsubgraph Group\nA\nend\nGroup }|--|| B : \"relates to\"\n",
            None,
        )
        .unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A"]);
        let g = to_graph(&d);
        assert_eq!(g.edges[0].from, Endpoint::Cluster(ClusterId(0)));
        assert_eq!(g.edges[0].to, Endpoint::Node(NodeId(1)));
        // Multiple relationships inside one group.
        let d = parse(
            "erDiagram\nsubgraph Group\n  A }|--|| B : \"r1\"\n  B }|--|| C : \"r2\"\nend",
            None,
        )
        .unwrap();
        assert_eq!(d.subgraphs[0].members, vec!["A", "B", "C"]);
        assert_eq!(d.relationships.len(), 2);
    }

    #[test]
    fn nested_subgraphs() {
        let d = parse(
            "erDiagram\nsubgraph outer\n  A\n  subgraph inner\n    B\n  end\nend",
            None,
        )
        .unwrap();
        // Closing order: the inner group first; its id is a member of the outer one.
        assert_eq!(d.subgraphs[0].id, "inner");
        assert_eq!(sg(&d, "outer").members, vec!["A", "inner"]);
        assert_eq!(sg(&d, "inner").members, vec!["B"]);
        let g = to_graph(&d);
        assert_eq!(g.clusters[0].parent, Some(ClusterId(1)));
        assert_eq!(g.clusters[1].parent, None);
        let d = parse(
            "erDiagram\nsubgraph outer\n    A\n      B\n  subgraph inner\n        C\n  end\nend",
            None,
        )
        .unwrap();
        assert_eq!(sg(&d, "outer").members, vec!["A", "B", "inner"]);
        assert_eq!(sg(&d, "inner").members, vec!["C"]);
        let d = parse(
            "erDiagram\nsubgraph outer\n  subgraph inner\n    A }|--|| B : \"relates to\"\n  end\nend",
            None,
        )
        .unwrap();
        assert_eq!(sg(&d, "outer").members, vec!["inner"]);
        assert_eq!(sg(&d, "inner").members, vec!["A", "B"]);
    }

    #[test]
    fn entity_in_two_subgraphs_stays_in_the_first_closed() {
        let d = parse(
            "erDiagram\nsubgraph G1\nA\nB\nend\nsubgraph G2\nB\nC\nend",
            None,
        )
        .unwrap();
        assert_eq!(sg(&d, "G1").members, vec!["A", "B"]);
        assert_eq!(sg(&d, "G2").members, vec!["C"]);
        // A nested group closes first, so it keeps a name the outer group repeats.
        let d = parse("erDiagram\nsubgraph O\nX\nsubgraph I\nX\nend\nend", None).unwrap();
        assert_eq!(sg(&d, "I").members, vec!["X"]);
        assert_eq!(sg(&d, "O").members, vec!["I"]);
    }

    #[test]
    fn subgraph_styles_classes_and_colour_slots() {
        let d = parse(
            "erDiagram\nsubgraph G\nA\nend\nG ||--|| B : r\nstyle G fill:#ffe,stroke:#c90\nclassDef hot stroke:#f00,color:#00f\nclassDef default fill:#f9f\nclass G hot",
            None,
        )
        .unwrap();
        let g = to_graph(&d);
        let c = &g.clusters[0];
        assert_eq!(c.style.fill.as_ref().unwrap().css(), "#ffe");
        assert_eq!(c.style.stroke.as_ref().unwrap().css(), "#c90");
        assert_eq!(c.color_slot, None);
        // Entities take consecutive slots; groups take none.
        let slots: Vec<_> = g.nodes.iter().map(|n| n.color_slot).collect();
        assert_eq!(slots, vec![Some(ColorSlot(0)), Some(ColorSlot(1))]);
    }

    #[test]
    fn subgraph_errors() {
        assert_eq!(err_line("erDiagram\nA\nend"), 3);
        assert_eq!(err_line("erDiagram\nsubgraph G\nA\n"), 2);
        assert_eq!(err_line("erDiagram\nsubgraph G [x\nend"), 2);
        assert_eq!(err_line("erDiagram\nsubgraph\nend"), 2);
    }

    #[test]
    fn subgraph_layout_contains_members() {
        let src = "erDiagram\n direction LR\n subgraph L1 [Level A]\n  A\n  subgraph L2 [Level B]\n   direction TB\n   B\n   subgraph L3\n    C ||--o{ D : has\n   end\n  end\n end\n A ||--|{ B : owns\n X }o--|| L3 : refs\n subgraph EMPTY\n end";
        let d = parse(src, None).unwrap();
        let g = to_graph(&d);
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let scene = graph::render(&g, &graph_config(&cfg), &pal);
        let mut clusters = Vec::new();
        boxes(&scene.elements, "cluster-", &mut clusters);
        let mut nodes = Vec::new();
        boxes(&scene.elements, "entity-", &mut nodes);
        let cl = |id: &str| clusters.iter().find(|(c, _)| c == id).unwrap().1;
        let nd = |id: &str| {
            nodes
                .iter()
                .find(|(n, _)| n.rsplit_once('-').map(|(a, _)| a) == Some(id))
                .unwrap()
                .1
        };
        for n in &g.nodes {
            match n.cluster {
                Some(c) => assert!(contains(&cl(&g.clusters[c.0].id), &nd(&n.id)), "{}", n.id),
                None => {
                    // The empty group draws only its title (no frame).
                    for c in g.clusters.iter().filter(|c| c.id != "EMPTY") {
                        assert!(!overlaps(&cl(&c.id), &nd(&n.id)), "{} in {}", n.id, c.id);
                    }
                }
            }
        }
        assert!(clusters.iter().all(|(c, _)| c != "EMPTY"));
        for c in &g.clusters {
            if let Some(p) = c.parent {
                assert!(contains(&cl(&g.clusters[p.0].id), &cl(&c.id)), "{}", c.id);
            }
        }
        assert_eq!(
            g.clusters.iter().find(|c| c.id == "L2").unwrap().direction,
            Some(Direction::TopBottom)
        );
    }

    fn bbox(el: &Element, out: &mut Option<Rect>) {
        let mut add = |p: Point| {
            let r = out.get_or_insert(Rect::new(p.x, p.y, 0.0, 0.0));
            let (x0, y0) = (r.x.min(p.x), r.y.min(p.y));
            let (x1, y1) = (r.right().max(p.x), r.bottom().max(p.y));
            *r = Rect::new(x0, y0, x1 - x0, y1 - y0);
        };
        match el {
            Element::Group { children, .. } => children.iter().for_each(|c| bbox(c, out)),
            Element::Shape { geom, .. } => match geom {
                Geometry::Rect { rect, .. } => {
                    add(Point::new(rect.x, rect.y));
                    add(Point::new(rect.right(), rect.bottom()));
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

    #[test]
    fn aliases_are_rejected_on_relationship_ends_like_mermaid() {
        assert!(parse("erDiagram\np[Person] ||--o{ q : x", None).is_err());
        assert!(parse("erDiagram\np ||--o{ q[Q] : x", None).is_err());
        let d = parse("erDiagram\np[Person]\np ||--o{ q : x", None).unwrap();
        assert_eq!(d.entities[0].alias.as_deref(), Some("Person"));
    }

    #[test]
    fn plain_entity_names_wrap_at_the_flowchart_wrapping_width() {
        let d = parse(
            "erDiagram\nCUSTOMER }|..|{ DELIVERY-ADDRESS : uses\nLINE-ITEM { int qty }",
            None,
        )
        .unwrap();
        let mut g = to_graph(&d);
        wrap_plain_entities(&mut g, 120.0, 16.0);
        let label = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().label.clone();
        // mermaid: `DELIVERY-` / `ADDRESS` (the browser breaks after the hyphen).
        assert_eq!(label("DELIVERY-ADDRESS"), "DELIVERY-<br>ADDRESS");
        let wrapped = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().wrapped_width;
        assert_eq!(wrapped("DELIVERY-ADDRESS"), Some(120.0));
        assert_eq!(wrapped("CUSTOMER"), None);
        assert_eq!(label("CUSTOMER"), "CUSTOMER");
        // A table's name is not wrapped.
        let mut g2 = to_graph(&parse("erDiagram\nDELIVERY-ADDRESS { int x }", None).unwrap());
        wrap_plain_entities(&mut g2, 50.0, 16.0);
        assert_eq!(g2.nodes[0].label, "DELIVERY-ADDRESS");
    }

    #[test]
    fn group_titles_wrap_at_the_group_width() {
        let title = "Customer delivery addresses and their very long ordering group name";
        let src =
            format!("erDiagram\nsubgraph G [{title}]\n  A\nend\nsubgraph S [Short]\n  B\nend");
        let g = to_graph(&parse(&src, None).unwrap());
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let scene = graph::render(&g, &graph_config(&cfg), &pal);
        let mut clusters = Vec::new();
        boxes(&scene.elements, "cluster-", &mut clusters);
        let cl = |id: &str| clusters.iter().find(|(c, _)| c == id).unwrap().1;
        let (long, short) = (cl("G"), cl("S"));
        let full = crate::text::line_width(title, pal.font_size, false);
        // mermaid (148 / 156 px wide): the title wraps instead of widening the frame.
        assert!(long.w < full / 2.0, "{long:?}");
        assert!(long.w >= short.w, "{long:?} {short:?}");
        assert!(
            long.h > short.h + 40.0,
            "four title lines: {long:?} {short:?}"
        );
    }
}
