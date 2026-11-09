//! Class diagrams (`classDiagram`) → `GraphDiagram` with compartment nodes.

use super::graph::{
    Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Section, Shape,
    StyleOverrides,
};
use super::statements;
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
    ns_stack: Vec<ClusterId>,
    relations: Vec<(String, String, Edge)>,
    notes: Vec<(Option<String>, String)>,
    class_defs: HashMap<String, StyleOverrides>,
    css_classes: Vec<(String, String)>,
    styles: Vec<(String, StyleOverrides)>,
}

/// Parse a class diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    let mut b = Builder::default();
    let mut direction = Direction::TopBottom;
    let mut header = false;
    let mut open_class: Option<String> = None;
    for (line, s) in statements(body) {
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
            let name = rest.trim_end_matches('{').trim().to_string();
            b.clusters.push(Cluster {
                id: name.clone(),
                title: name,
                parent: b.ns_stack.last().copied(),
                style: StyleOverrides::default(),
                color_slot: None, // namespaces are not part of the colour cycle
            });
            b.ns_stack.push(ClusterId(b.clusters.len() - 1));
            continue;
        }
        if let Some(rest) = s.strip_prefix("class ") {
            let rest = rest.trim();
            let opens = rest.ends_with('{');
            let decl = rest.trim_end_matches('{').trim();
            let (decl, css) = match decl.split_once(":::") {
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
    Ok(b.finish(direction, title))
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
        (l, Head::Circle)
    } else if let Some(l) = left.strip_suffix('*') {
        (l, Head::DiamondFilled)
    } else if let Some(l) = left.strip_suffix('<') {
        (l, Head::Arrow)
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
        Head::Circle
    } else if let Some(r) = right.strip_prefix('*') {
        right = r;
        Head::DiamondFilled
    } else if let Some(r) = right.strip_prefix('>') {
        right = r;
        Head::Arrow
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
        LineKind::Dotted
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
            let mut header: Vec<String> =
                info.annotations.iter().map(|a| format!("«{a}»")).collect();
            let display = info.label.clone().unwrap_or_else(|| match &info.generic {
                Some(g) => format!("{name}<{}>", generic_text(g)),
                None => name.clone(),
            });
            header.push(display);
            let mut n = Node::new(name.clone(), String::new(), Shape::Compartments);
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
    fn classes_members_relations() {
        let g = parse(
            "classDiagram\nclass Animal {\n  <<abstract>>\n  +String name\n  +makeSound() void\n}\nAnimal <|-- Duck\nDuck : +swim()\nZoo \"1\" *-- \"many\" Animal : contains\nA ..> B\nC o-- D",
            None,
        )
        .unwrap();
        let animal = &g.nodes[0];
        assert_eq!(animal.sections[0].lines, vec!["«abstract»", "Animal"]);
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
            (LineKind::Dotted, Head::Arrow)
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
}
