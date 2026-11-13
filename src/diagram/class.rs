//! Class diagrams (`classDiagram`) → `GraphDiagram` with compartment nodes.

use super::graph::{
    Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Section, Shape,
    StyleOverrides,
};
use super::statements_multiline;
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::DiagramKind;
use crate::theme::ColorSlot;
use std::collections::HashMap;

#[derive(Default, Clone)]
struct ClassInfo {
    label: Option<String>,
    generic: Option<String>,
    annotations: Vec<String>,
    attributes: Vec<String>,
    methods: Vec<String>,
    namespace: Option<ClusterId>,
}

#[derive(Default)]
struct Builder {
    order: Vec<String>,
    classes: HashMap<String, ClassInfo>,
    clusters: Vec<Cluster>,
    ns_index: HashMap<String, usize>,
    ns_stack: Vec<ClusterId>,
    /// Per namespace: declared by the author (classDb's `explicit`), not only implied
    /// as the ancestor segment of a dotted name.
    ns_explicit: Vec<bool>,
    relations: Vec<(String, String, Edge)>,
    notes: Vec<(Option<String>, String)>,
    class_defs: HashMap<String, StyleOverrides>,
    css_classes: Vec<(String, String)>,
    styles: Vec<(String, StyleOverrides)>,
    /// Backtick class names (classDiagram.jison `classLiteralName`), by placeholder index.
    literals: Literals,
}

/// Backtick class names (`` `Animal Class!` ``) may hold spaces and punctuation the
/// statement parser splits on, so each is swapped for an opaque placeholder token
/// (private-use characters, which no other syntax uses) before parsing, and the
/// name comes back when the class is built.
#[derive(Default)]
struct Literals(Vec<String>);

impl Literals {
    const MARK: char = '\u{F0000}';

    /// Replace each `` `name` `` outside a `"string"` by its placeholder.
    fn protect(&mut self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_quote = false;
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            if c == '"' {
                in_quote = !in_quote;
            } else if c == '`' && !in_quote {
                if let Some(end) = rest[1..].find('`').filter(|&e| e > 0) {
                    let name = &rest[1..1 + end];
                    let i = match self.0.iter().position(|n| n == name) {
                        Some(i) => i,
                        None => {
                            self.0.push(name.to_string());
                            self.0.len() - 1
                        }
                    };
                    out.push(Self::MARK);
                    out.push_str(&i.to_string());
                    out.push(Self::MARK);
                    rest = &rest[end + 2..];
                    continue;
                }
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
    }

    /// Undo [`Literals::protect`].
    fn restore(&self, s: &str) -> String {
        if !s.contains(Self::MARK) {
            return s.to_string();
        }
        let mut out = String::new();
        for (k, part) in s.split(Self::MARK).enumerate() {
            match part.parse::<usize>().ok().and_then(|i| self.0.get(i)) {
                Some(name) if k % 2 == 1 => out.push_str(name),
                _ => out.push_str(part),
            }
        }
        out
    }
}

/// Config that changes how a class diagram is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// `class.hierarchicalNamespaces` (default true). When false, only declared
    /// namespaces are drawn, flat at the top level and titled by their full dotted
    /// name; members of an implied ancestor go to the nearest declared one.
    pub hierarchical_namespaces: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            hierarchical_namespaces: true,
        }
    }
}

/// Parse a class diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    parse_with(body, title, ParseOptions::default())
}

/// [`parse`] with explicit [`ParseOptions`].
pub fn parse_with(body: &str, title: Option<String>, opts: ParseOptions) -> Result<GraphDiagram> {
    let g = parse_hierarchical(body, title)?;
    Ok(if opts.hierarchical_namespaces {
        g.0
    } else {
        flatten_namespaces(g.0, &g.1)
    })
}

/// classDb's `getData` with `hierarchicalNamespaces: false`.
fn flatten_namespaces(mut g: GraphDiagram, explicit: &[bool]) -> GraphDiagram {
    let n = g.clusters.len();
    let mut map: Vec<Option<usize>> = vec![None; n];
    let mut clusters = Vec::new();
    for (ci, c) in g.clusters.iter().enumerate() {
        if explicit.get(ci).copied().unwrap_or(true) {
            map[ci] = Some(clusters.len());
            clusters.push(Cluster {
                title: c.id.clone(),
                parent: None,
                ..c.clone()
            });
        }
    }
    // Nearest declared namespace at or above `c`.
    let resolve = |c: ClusterId| -> Option<ClusterId> {
        let mut cur = Some(c.0);
        for _ in 0..=n {
            let k = cur?;
            if let Some(m) = map[k] {
                return Some(ClusterId(m));
            }
            cur = g.clusters[k].parent.map(|p| p.0);
        }
        None
    };
    // (Class relations always join classes, never namespaces.)
    for node in &mut g.nodes {
        node.cluster = node.cluster.and_then(resolve);
    }
    g.clusters = clusters;
    g
}

fn parse_hierarchical(body: &str, title: Option<String>) -> Result<(GraphDiagram, Vec<bool>)> {
    let mut b = Builder::default();
    let mut direction = Direction::TopBottom;
    let mut header = false;
    let mut open_class: Option<String> = None;
    // A `class A["…"]` label may span lines (the jison string matches newlines).
    for (line, s) in statements_multiline(body, |s| s.starts_with("class ")) {
        if !header {
            if !s.starts_with("classDiagram") {
                return Err(Error::parse(line, format!("{s}\nExpecting 'classDiagram'")));
            }
            header = true;
            continue;
        }
        if let Some(cls) = open_class.clone() {
            if s == "}" {
                open_class = None;
            } else if s.starts_with("<<") {
                b.class(&cls)
                    .annotations
                    .push(s.trim_matches(['<', '>']).trim().to_string());
            } else {
                b.add_member(&cls, &s);
            }
            continue;
        }
        let s = b.literals.protect(&s);
        if s == "}" {
            b.ns_stack
                .pop()
                .ok_or_else(|| Error::parse(line, "Unexpected '}'"))?;
            continue;
        }
        if let Some(d) = s.strip_prefix("direction ") {
            direction = Direction::parse(d.trim())
                .ok_or_else(|| Error::parse(line, format!("Invalid direction '{}'", d.trim())))?;
            continue;
        }
        if let Some(rest) = s.strip_prefix("namespace ") {
            let head = rest.trim_end_matches('{').trim();
            let (name, label) = match head.find('[') {
                Some(i) => (
                    head[..i].trim(),
                    Some(
                        head[i + 1..]
                            .trim_end_matches(']')
                            .trim()
                            .trim_matches('"')
                            .to_string(),
                    ),
                ),
                None => (head, None),
            };
            if name.is_empty() {
                return Err(Error::parse(line, "Expecting a namespace name"));
            }
            let c = b.namespace(name, label);
            b.ns_stack.push(c);
            continue;
        }
        if let Some(rest) = s.strip_prefix("class ") {
            let rest = rest.trim();
            let opens = rest.ends_with('{');
            let decl = rest.trim_end_matches('{').trim();
            let (decl, css) = match decl.split_once(":::") {
                Some((_, c)) if c.trim().is_empty() => {
                    return Err(Error::parse(
                        line,
                        format!("{s}\nExpecting a class name after ':::'"),
                    ));
                }
                Some((d, c)) => (d.trim(), Some(c.trim().to_string())),
                None => (decl, None),
            };
            let (name, label) = match decl.find('[') {
                Some(i) => (
                    decl[..i].trim(),
                    Some(
                        decl[i + 1..]
                            .trim_end_matches(']')
                            .trim()
                            .trim_matches('"')
                            .to_string(),
                    ),
                ),
                None => (decl, None),
            };
            // `~` opens a generic that a second `~` must close (classDiagram.jison
            // GENERICTYPE).
            if name.matches('~').count() == 1 {
                return Err(Error::parse(
                    line,
                    format!("{s}\nUnterminated generic: expecting '~'"),
                ));
            }
            let (name, generic) = split_generic(name);
            if name.is_empty() {
                return Err(Error::parse(line, "Expecting a class name"));
            }
            let info = b.class(&name);
            if label.is_some() {
                info.label = label;
            }
            if generic.is_some() {
                info.generic = generic;
            }
            if let Some(c) = css {
                b.css_classes.push((name.clone(), c));
            }
            // `class X` inside a namespace body moves X there (classDb's
            // `addClassesToNamespace`, the last namespace wins).
            if let Some(&ns) = b.ns_stack.last() {
                b.class(&name).namespace = Some(ns);
            }
            if opens {
                open_class = Some(name);
            }
            continue;
        }
        if s.starts_with("<<") {
            if let Some(end) = s.find(">>") {
                let ann = s[2..end].trim().to_string();
                let name = s[end + 2..].trim().to_string();
                b.class(&name).annotations.push(ann);
                continue;
            }
        }
        if let Some(rest) = s.strip_prefix("note ") {
            let rest = rest.trim();
            let (target, text) = if let Some(r) = rest.strip_prefix("for ") {
                let r = r.trim();
                let (t, txt) = r.split_once(char::is_whitespace).unwrap_or((r, ""));
                (
                    Some(t.to_string()),
                    txt.trim().trim_matches('"').to_string(),
                )
            } else {
                (None, rest.trim_matches('"').to_string())
            };
            // The note text (a STR token) is mandatory.
            if text.is_empty() {
                return Err(Error::parse(line, format!("{s}\nExpecting the note text")));
            }
            b.notes.push((target, text.replace("\\n", "\n")));
            continue;
        }
        if let Some(rest) = s.strip_prefix("classDef ") {
            let (names, style) = rest
                .trim()
                .split_once(char::is_whitespace)
                .unwrap_or((rest.trim(), ""));
            for n in names.split(',') {
                b.class_defs
                    .insert(n.trim().to_string(), StyleOverrides::parse(style));
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("cssClass ") {
            let rest = rest.trim();
            if let Some(end) = rest[1..].find('"') {
                let ids = &rest[1..end + 1];
                let class = rest[end + 2..].trim();
                for id in ids.split(',') {
                    b.css_classes
                        .push((id.trim().to_string(), class.to_string()));
                }
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("style ") {
            let (id, st) = rest
                .trim()
                .split_once(char::is_whitespace)
                .unwrap_or((rest.trim(), ""));
            b.styles.push((id.to_string(), StyleOverrides::parse(st)));
            continue;
        }
        if s.starts_with("link ") || s.starts_with("click ") || s.starts_with("callback ") {
            continue;
        }
        if let Some(rel) = parse_relation(&s) {
            let (from, to, edge) = rel;
            b.class(&from);
            b.class(&to);
            b.relations.push((from, to, edge));
            continue;
        }
        if let Some((cls, member)) = s.split_once(':') {
            let (cls, _) = split_generic(cls.trim());
            b.add_member(&cls, member.trim());
            continue;
        }
        let (name, _) = split_generic(s.trim());
        if name.contains(char::is_whitespace) {
            return Err(Error::parse(line, format!("{s}\nUnexpected statement")));
        }
        b.class(&name);
    }
    if open_class.is_some() || !b.ns_stack.is_empty() {
        return Err(Error::parse(
            body.lines().count(),
            "Unexpected end of input: missing '}'",
        ));
    }
    let explicit = b.ns_explicit.clone();
    Ok((b.finish(direction, title), explicit))
}

fn split_generic(name: &str) -> (String, Option<String>) {
    match name.find('~') {
        Some(i) => (
            name[..i].trim().to_string(),
            Some(name[i + 1..].trim_end_matches('~').to_string()),
        ),
        None => (name.trim().to_string(), None),
    }
}

fn generic_text(s: &str) -> String {
    // `List~int~` → `List<int>`
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

/// `A "1" <|-- "*" B : label`
fn parse_relation(s: &str) -> Option<(String, String, Edge)> {
    let (op_pos, dotted) = find_operator(s)?;
    let left = s[..op_pos].trim_end();
    let mut right = s[op_pos + 2..].trim_start();
    let (left, start) = if let Some(l) = left.strip_suffix("<|") {
        (l, Head::Triangle)
    } else if let Some(l) = left.strip_suffix("()") {
        (l, Head::Lollipop)
    } else if let Some(l) = left.strip_suffix('*') {
        (l, Head::DiamondFilled)
    } else if let Some(l) = left.strip_suffix('<') {
        (l, Head::Dependency)
    } else if left.len() > 1 && left.ends_with('o') && left[..left.len() - 1].ends_with([' ', '"'])
    {
        (&left[..left.len() - 1], Head::Diamond)
    } else {
        (left, Head::None)
    };
    let end = if let Some(r) = right.strip_prefix("|>") {
        right = r;
        Head::Triangle
    } else if let Some(r) = right.strip_prefix("()") {
        right = r;
        Head::Lollipop
    } else if let Some(r) = right.strip_prefix('*') {
        right = r;
        Head::DiamondFilled
    } else if let Some(r) = right.strip_prefix('>') {
        right = r;
        Head::Dependency
    } else if right.starts_with("o ") || right.starts_with("o\"") {
        right = &right[1..];
        Head::Diamond
    } else {
        Head::None
    };
    let (left_name, left_card) = take_cardinality(left.trim(), true);
    let (rhs, label) = match right.split_once(':') {
        Some((r, l)) => (
            r.trim(),
            Some(l.trim().to_string()).filter(|l| !l.is_empty()),
        ),
        None => (right.trim(), None),
    };
    let (right_name, right_card) = take_cardinality(rhs, false);
    if left_name.is_empty()
        || right_name.is_empty()
        || left_name.contains(' ')
        || right_name.contains(' ')
    {
        return None;
    }
    let mut e = Edge::new(Endpoint::Node(NodeId(0)), Endpoint::Node(NodeId(0)));
    e.line = if dotted {
        LineKind::Dashed
    } else {
        LineKind::Solid
    };
    e.start = start;
    e.end = end;
    e.label = label;
    e.start_label = left_card;
    e.end_label = right_card;
    Some((split_generic(&left_name).0, split_generic(&right_name).0, e))
}

fn find_operator(s: &str) -> Option<(usize, bool)> {
    let b = s.as_bytes();
    let mut in_quote = false;
    for i in 0..b.len().saturating_sub(1) {
        if b[i] == b'"' {
            in_quote = !in_quote;
        }
        if !in_quote && ((b[i] == b'-' && b[i + 1] == b'-') || (b[i] == b'.' && b[i + 1] == b'.')) {
            return Some((i, b[i] == b'.'));
        }
    }
    None
}

/// Split `Name "1"` (left) or `"*" Name` (right) into the name and its cardinality.
fn take_cardinality(s: &str, left: bool) -> (String, Option<String>) {
    let s = s.trim();
    if left {
        if let Some(body) = s.strip_suffix('"') {
            if let Some(start) = body.rfind('"') {
                return (
                    body[..start].trim().to_string(),
                    Some(body[start + 1..].to_string()),
                );
            }
        }
    } else if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (
                rest[end + 1..].trim().to_string(),
                Some(rest[..end].to_string()),
            );
        }
    }
    (s.to_string(), None)
}

impl Builder {
    /// classDb's `addNamespace`: the id is qualified by the enclosing namespace, and a
    /// dotted id creates one nested namespace per segment, each titled by its segment.
    /// A repeated id reuses the existing namespace.
    fn namespace(&mut self, name: &str, label: Option<String>) -> ClusterId {
        let qualified = match self.ns_stack.last() {
            Some(p) => format!("{}.{name}", self.clusters[p.0].id),
            None => name.to_string(),
        };
        let parts: Vec<&str> = qualified.split('.').collect();
        let mut parent: Option<ClusterId> = None;
        let mut id = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                id.push('.');
            }
            id.push_str(part);
            let leaf = i + 1 == parts.len();
            let c = match self.ns_index.get(&id) {
                Some(&c) => c,
                None => {
                    // Namespaces are not part of the colour cycle.
                    self.clusters.push(Cluster::new(id.clone(), *part, parent));
                    self.ns_explicit.push(false);
                    self.ns_index.insert(id.clone(), self.clusters.len() - 1);
                    self.clusters.len() - 1
                }
            };
            if leaf {
                self.ns_explicit[c] = true;
                if let Some(l) = &label {
                    self.clusters[c].title = l.clone();
                }
            }
            parent = Some(ClusterId(c));
        }
        parent.expect("a namespace has at least one segment")
    }

    fn class(&mut self, name: &str) -> &mut ClassInfo {
        if !self.classes.contains_key(name) {
            self.order.push(name.to_string());
            let ns = self.ns_stack.last().copied();
            self.classes.insert(
                name.to_string(),
                ClassInfo {
                    namespace: ns,
                    ..Default::default()
                },
            );
        }
        self.classes.get_mut(name).expect("inserted above")
    }

    fn add_member(&mut self, cls: &str, member: &str) {
        let m = member.trim();
        if m.is_empty() {
            return;
        }
        let text = generic_text(m.trim_end_matches(['$', '*']));
        let info = self.class(cls);
        if m.contains('(') {
            info.methods.push(text);
        } else {
            info.attributes.push(text);
        }
    }

    fn finish(self, direction: Direction, title: Option<String>) -> GraphDiagram {
        let mut nodes = Vec::new();
        let mut index = HashMap::new();
        for name in &self.order {
            let info = &self.classes[name];
            let mut header: Vec<String> = Vec::new();
            let id = self.literals.restore(name);
            let display = info.label.clone().unwrap_or_else(|| match &info.generic {
                Some(g) => format!("{id}<{}>", generic_text(g)),
                None => id.clone(),
            });
            // A label spanning lines breaks there (`first<br>second`).
            header.extend(display.split('\n').map(|l| l.trim().to_string()));
            let mut n = Node::new(id, String::new(), Shape::Compartments);
            // classBox draws only the first annotation.
            n.annotation = info.annotations.first().map(|a| format!("«{a}»"));
            // Only classes consume a palette slot (classDb: `classColorIndex++`); notes do not.
            n.color_slot = Some(ColorSlot(nodes.len()));
            n.cluster = info.namespace;
            n.sections = vec![
                Section {
                    lines: header,
                    centered: true,
                    bold: true,
                },
                Section {
                    lines: info.attributes.clone(),
                    centered: false,
                    bold: false,
                },
                Section {
                    lines: info.methods.clone(),
                    centered: false,
                    bold: false,
                },
            ];
            index.insert(name.clone(), nodes.len());
            nodes.push(n);
        }
        let mut edges: Vec<Edge> = self
            .relations
            .into_iter()
            .map(|(a, b, mut e)| {
                e.from = Endpoint::Node(NodeId(index[&a]));
                e.to = Endpoint::Node(NodeId(index[&b]));
                e
            })
            .collect();
        for (i, (target, text)) in self.notes.into_iter().enumerate() {
            let mut n = Node::new(format!("note{i}"), text, Shape::Note);
            if let Some(t) = target.as_ref().and_then(|t| index.get(t)) {
                n.cluster = nodes[*t].cluster;
                let mut e = Edge::new(
                    Endpoint::Node(NodeId(nodes.len())),
                    Endpoint::Node(NodeId(*t)),
                );
                e.line = LineKind::Dotted;
                e.end = Head::None;
                edges.push(e);
            }
            nodes.push(n);
        }
        for (id, class) in &self.css_classes {
            if let (Some(&i), Some(def)) = (index.get(id), self.class_defs.get(class)) {
                nodes[i].style = nodes[i].style.merged(def);
                nodes[i].classes.push(class.clone());
            }
        }
        for (id, st) in &self.styles {
            if let Some(&i) = index.get(id) {
                nodes[i].style = nodes[i].style.merged(st);
            }
        }
        GraphDiagram {
            kind: DiagramKind::Class,
            direction,
            nodes,
            edges,
            clusters: self.clusters,
            title,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_incomplete_class_statements() {
        assert!(parse("classDiagram\nclass A~T", None).is_err());
        assert!(parse("classDiagram\nclass A:::", None).is_err());
        assert!(parse("classDiagram\nnote for A", None).is_err());
        assert!(parse("classDiagram\nclass A~T~\nnote for A \"n\"", None).is_ok());
    }

    #[test]
    fn classes_members_relations() {
        let g = parse(
            "classDiagram\nclass Animal {\n  <<abstract>>\n  +String name\n  +makeSound() void\n}\nAnimal <|-- Duck\nDuck : +swim()\nZoo \"1\" *-- \"many\" Animal : contains\nA ..> B\nC o-- D",
            None,
        )
        .unwrap();
        let animal = &g.nodes[0];
        assert_eq!(animal.annotation.as_deref(), Some("«abstract»"));
        assert_eq!(animal.sections[0].lines, vec!["Animal"]);
        assert_eq!(animal.sections[1].lines, vec!["+String name"]);
        assert_eq!(animal.sections[2].lines, vec!["+makeSound() void"]);
        assert_eq!(g.edges[0].start, Head::Triangle);
        let zoo = &g.edges[1];
        assert_eq!(zoo.start, Head::DiamondFilled);
        assert_eq!(zoo.start_label.as_deref(), Some("1"));
        assert_eq!(zoo.end_label.as_deref(), Some("many"));
        assert_eq!(zoo.label.as_deref(), Some("contains"));
        assert_eq!(
            (g.edges[2].line, g.edges[2].end),
            (LineKind::Dashed, Head::Dependency)
        );
        assert_eq!(g.edges[3].start, Head::Diamond);
    }

    #[test]
    fn generics_and_namespaces() {
        let g = parse(
            "classDiagram\nnamespace Shapes {\n class Square~Shape~\n}\nSquare : -List~int~ ids",
            None,
        )
        .unwrap();
        assert_eq!(g.clusters.len(), 1);
        assert_eq!(g.nodes[0].sections[0].lines, vec!["Square<Shape>"]);
        assert_eq!(g.nodes[0].sections[1].lines, vec!["-List<int> ids"]);
        assert!(g.nodes[0].cluster.is_some());
    }

    #[test]
    fn dotted_and_nested_namespaces() {
        let g = parse(
            "classDiagram\nnamespace Company.Project {\n class A\n}\nnamespace Company {\n namespace Other[\"Elsewhere\"] {\n  class B\n }\n}\nclass C",
            None,
        )
        .unwrap();
        let ids: Vec<&str> = g.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["Company", "Company.Project", "Company.Other"]);
        let titles: Vec<&str> = g.clusters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Company", "Project", "Elsewhere"]);
        assert_eq!(g.clusters[1].parent, Some(ClusterId(0)));
        assert_eq!(g.clusters[2].parent, Some(ClusterId(0)));
        let of = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().cluster;
        assert_eq!(of("A"), Some(ClusterId(1)));
        assert_eq!(of("B"), Some(ClusterId(2)));
        assert_eq!(of("C"), None);
        assert!(g.clusters.iter().all(|c| c.color_slot.is_none()));
    }

    #[test]
    fn class_moves_into_a_later_namespace() {
        let g = parse("classDiagram\nA --> B\nnamespace N {\n class A\n}", None).unwrap();
        let a = g.nodes.iter().find(|n| n.id == "A").unwrap();
        assert_eq!(a.cluster, Some(ClusterId(0)));
        let b = g.nodes.iter().find(|n| n.id == "B").unwrap();
        assert_eq!(b.cluster, None);
    }

    #[test]
    fn flat_namespaces_keep_only_declared_ones() {
        let src = "classDiagram\nnamespace Company.Project {\n class A\n}\nnamespace Company {\n namespace Other[\"Elsewhere\"] {\n  class B\n }\n class D\n}\nnamespace X.Y.Z {\n class E\n}\nclass C";
        let g = parse_with(
            src,
            None,
            ParseOptions {
                hierarchical_namespaces: false,
            },
        )
        .unwrap();
        // `Company` is declared too (the second block), `X` and `X.Y` are only implied.
        let ids: Vec<&str> = g.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            ["Company", "Company.Project", "Company.Other", "X.Y.Z"]
        );
        let titles: Vec<&str> = g.clusters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ids, "flat namespaces are titled by their full id");
        assert!(g.clusters.iter().all(|c| c.parent.is_none()));
        let of = |id: &str| {
            g.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .cluster
                .map(|c| g.clusters[c.0].id.as_str())
        };
        assert_eq!(of("A"), Some("Company.Project"));
        assert_eq!(of("B"), Some("Company.Other"));
        assert_eq!(of("D"), Some("Company"));
        assert_eq!(of("E"), Some("X.Y.Z"));
        assert_eq!(of("C"), None);
        // The default stays hierarchical.
        assert_eq!(parse(src, None).unwrap().clusters.len(), 6);
    }

    #[test]
    fn backtick_class_names_may_hold_spaces_and_punctuation() {
        let src = "classDiagram
    class `Animal Class!` {
      +int age
    }
    class `Car Class`~T~
    `Animal Class!` <|-- `Car Class` : drives
    `Animal Class!` : +eat()
    note for `Car Class` \"a note\"
    class B[\"`md` label\"]";
        let g = parse(src, None).unwrap();
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["Animal Class!", "Car Class", "B", "note0"]);
        assert_eq!(g.nodes[0].sections[0].lines, ["Animal Class!"]);
        assert_eq!(g.nodes[0].sections[1].lines, ["+int age"]);
        assert_eq!(g.nodes[0].sections[2].lines, ["+eat()"]);
        assert_eq!(g.nodes[1].sections[0].lines, ["Car Class<T>"]);
        // Backticks inside a string are left alone.
        assert_eq!(g.nodes[2].sections[0].lines, ["`md` label"]);
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].label.as_deref(), Some("drives"));
    }

    #[test]
    fn class_labels_may_span_lines() {
        let g = parse("classDiagram\nclass A[\"first\nsecond\"]\nA --> B", None).unwrap();
        assert_eq!(g.nodes[0].sections[0].lines, vec!["first", "second"]);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn class_box_width_follows_text_helpers_bbox() {
        // textHelper centres the annotation and name on x = 0 while members start
        // there: the box is half the widest centred label plus the farther of that
        // half and the widest member, plus 2 * PADDING. Reference (mermaid 12.3.0):
        // Animal 139.58, Duck 153.57, Shape 138.80 wide.
        let src = "classDiagram\nAnimal <|-- Duck\nAnimal : +int age\nAnimal : +String gender\nAnimal: +isMammal()\nAnimal: +mate()\nclass Duck{\n+String beakColor\n+swim()\n+quack()\n}\nclass Shape{\n<<interface>>\n<<second>>\nnoOfVertices\ndraw()\n}";
        let g = parse(src, None).unwrap();
        let shape = g.nodes.iter().find(|n| n.id == "Shape").unwrap();
        // classBox draws only the first annotation, apart from the bold name.
        assert_eq!(shape.annotation.as_deref(), Some("«interface»"));
        assert_eq!(shape.sections[0].lines, ["Shape"]);
        let cfg = crate::config::MermaidConfig::default().scoped("class");
        let pal = crate::theme::palette(cfg.theme, &[]);
        let width = |id: &str| {
            let n = g.nodes.iter().find(|n| n.id == id).unwrap();
            super::super::graph::node_size(n, DiagramKind::Class, &cfg.class, &pal, false).0
        };
        let text = |s: &str, bold: bool| {
            crate::text::TextBlock::new(vec![s.into()], 14.0, bold)
                .html()
                .width
        };
        let expect = |half: f64, widest: f64| half + half.max(widest) + 24.0;
        let animal = expect(text("Animal", true) / 2.0, text("+String gender", false));
        assert!((width("Animal") - animal).abs() < 1e-6);
        assert!(
            (width("Animal") - 139.578).abs() < 1.0,
            "{}",
            width("Animal")
        );
        assert!((width("Duck") - 153.570).abs() < 1.0, "{}", width("Duck"));
        let shape_w = expect(
            text("«interface»", false) / 2.0,
            text("noOfVertices", false),
        );
        assert!((width("Shape") - shape_w).abs() < 1e-6);
        assert!((width("Shape") - 138.797).abs() < 1.0, "{}", width("Shape"));
    }

    #[test]
    fn annotation_is_drawn_in_normal_weight_above_the_bold_name() {
        use crate::geom::PathCmd;
        use crate::scene::{Element, Geometry, Text};
        let g = parse(
            "classDiagram\nclass Shape{\n<<interface>>\nnoOfVertices\n}",
            None,
        )
        .unwrap();
        let cfg = crate::config::MermaidConfig::default().scoped("class");
        let pal = crate::theme::palette(cfg.theme, &[]);
        let scene = super::super::graph::render(&g, &cfg.class, &pal);
        let (mut texts, mut tops, mut dividers) = (Vec::<Text>::new(), Vec::new(), Vec::new());
        fn walk(e: &Element, t: &mut Vec<Text>, r: &mut Vec<f64>, d: &mut Vec<f64>) {
            match e {
                Element::Text { text, .. } => t.push(text.clone()),
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    ..
                } => r.push(rect.y),
                Element::Shape {
                    geom: Geometry::Path(p),
                    ..
                } => {
                    if let [PathCmd::MoveTo(a), PathCmd::LineTo(b)] = p.as_slice() {
                        if a.y == b.y {
                            d.push(a.y);
                        }
                    }
                }
                Element::Group { children, .. } => children.iter().for_each(|c| walk(c, t, r, d)),
                _ => {}
            }
        }
        scene
            .elements
            .iter()
            .for_each(|e| walk(e, &mut texts, &mut tops, &mut dividers));
        let find = |s: &str| texts.iter().find(|t| t.lines == [s]).unwrap();
        let (ann, name) = (find("«interface»"), find("Shape"));
        assert!(!ann.bold && name.bold);
        // One 21px line apart, the annotation's top 13.5px (2 * PADDING - 10.5)
        // below the box: classBox's `y + PADDING` shift of textHelper's frame.
        assert!((name.y - ann.y - 21.0).abs() < 1e-6);
        let top = tops.iter().copied().fold(f64::INFINITY, f64::min);
        let first_divider = dividers.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            (first_divider - top - 66.0).abs() < 1e-6,
            "{first_divider} {top}"
        );
    }
}
