//! `erDiagram` diagrams → `GraphDiagram` with entity-table nodes and crow's-foot edges.
//!
//! Supported syntax (mermaid 11): entities with optional aliases (`p[Person]`,
//! `p["Person"]`, quoted names), attribute blocks (`type name PK, FK "comment"`,
//! generic types `type~T~`, back-quoted words), relationships with symbolic
//! (`|o`, `||`, `}o`, `}|`, `o|`, `o{`, `|{`) or word cardinalities (`one or more`,
//! `zero or many`, `only one`, `1+`, `0+`, `many(0)`, `1`, ...), identifying
//! (`--`, `to`) and non-identifying (`..`, `.-`, `-.`, `optionally to`) links,
//! `direction`, `classDef`, `class`, `style`, `:::class` and `title`.

use super::graph::StyleOverrides;
use super::graph::{self, Edge, Endpoint, GraphDiagram, Head, Node, NodeId, Section, Shape};
use crate::config::{Curve, GraphConfig, MermaidConfig};
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::{DiagramKind, Scene};
use crate::theme::Palette;
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

#[derive(Debug, Clone, PartialEq)]
pub struct ErDiagram {
    pub title: Option<String>,
    pub direction: Direction,
    pub entities: Vec<Entity>,
    pub relationships: Vec<Relationship>,
    pub class_defs: HashMap<String, StyleOverrides>,
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
}

/// Parse and render a `erDiagram` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let g = to_graph(&d);
    Ok(graph::render(&g, &graph_config(cfg), pal))
}

/// Layout settings from the `er` config section.
fn graph_config(cfg: &MermaidConfig) -> GraphConfig {
    GraphConfig {
        node_spacing: cfg.section_f64("er", "nodeSpacing", 60.0),
        rank_spacing: cfg.section_f64("er", "rankSpacing", 70.0),
        padding: cfg.section_f64("er", "entityPadding", 12.0),
        diagram_padding: cfg.section_f64("er", "diagramPadding", 20.0),
        curve: Curve::Basis,
        use_max_width: cfg.use_max_width("er"),
        title_top_margin: cfg.section_f64("er", "titleTopMargin", 25.0),
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
    };
    let mut header = false;
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
            if !rest.trim().is_empty() {
                return Err(Error::parse(
                    line,
                    format!("Unexpected '{}' after 'erDiagram'", rest.trim()),
                ));
            }
            continue;
        }
        let lower = s.to_ascii_lowercase();
        if let Some(dir) = lower.strip_prefix("direction ") {
            d.direction = Direction::parse(&dir.trim().to_ascii_uppercase())
                .ok_or_else(|| Error::parse(line, format!("Invalid direction '{}'", dir.trim())))?;
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
        block = statement(&mut d, s, line)?.map(|name| (name, Vec::new(), line));
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
    for (id, cls) in class_assign {
        if let Some(e) = d.entities.iter_mut().find(|e| e.name == id) {
            e.classes.push(cls);
        }
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
    }
    Ok(d)
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

/// One entity/relationship statement. Returns the entity name when it opens a block.
fn statement(d: &mut ErDiagram, s: &str, line: usize) -> Result<Option<String>> {
    let mut c = Cur::new(s);
    let a = entity_ref(&mut c, line)?
        .ok_or_else(|| Error::parse(line, format!("{s}\nExpecting an entity name")))?;
    c.ws();
    if c.at_end() {
        register(d, &a);
        return Ok(None);
    }
    if c.eat("{") {
        register(d, &a);
        return Ok(Some(a.name));
    }
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
    register(d, &a);
    register(d, &b);
    d.relationships.push(Relationship {
        from: a.name,
        to: b.name,
        from_card: left,
        to_card: right,
        identifying,
        label,
    });
    Ok(None)
}

// ---------------------------------------------------------------- graph

/// Lower the ER IR to the shared graph IR.
pub fn to_graph(d: &ErDiagram) -> GraphDiagram {
    let index: HashMap<&str, usize> = d
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i))
        .collect();
    let nodes = d
        .entities
        .iter()
        .map(|e| {
            let mut n = Node::new(
                e.name.clone(),
                e.alias.clone().unwrap_or_else(|| e.name.clone()),
                Shape::EntityTable,
            );
            n.style = e.style.clone();
            n.classes = e.classes.clone();
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
            n
        })
        .collect();
    let edges = d
        .relationships
        .iter()
        .filter_map(|r| {
            let from = *index.get(r.from.as_str())?;
            let to = *index.get(r.to.as_str())?;
            let mut e = Edge::new(Endpoint::Node(NodeId(from)), Endpoint::Node(NodeId(to)));
            e.start = r.from_card.head();
            e.end = r.to_card.head();
            e.label = Some(r.label.clone()).filter(|l| !l.trim().is_empty());
            if !r.identifying {
                e.style.dash = Some(vec![8.0, 8.0]);
            }
            Some(e)
        })
        .collect();
    GraphDiagram {
        kind: DiagramKind::Er,
        direction: d.direction,
        nodes,
        edges,
        clusters: Vec::new(),
        title: d.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "erDiagram\ndirection LR\np[Person] {\n string name\n}\na[\"Customer Account\"]:::hot ||--o{ p : owns\n\"Quoted Name\" { int x }\nclassDef hot fill:#f96\nstyle p stroke:#f00\nclass p hot",
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
}
