//! State diagrams (`stateDiagram`, `stateDiagram-v2`) → `GraphDiagram`.
//!
//! Composite states become clusters; `[*]` resolves to a start or end pseudo-state
//! scoped to the enclosing composite, as in mermaid.

use super::graph::{
    Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Section, Shape,
    StyleOverrides,
};
use super::statements;
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::DiagramKind;
use crate::theme::ColorSlot;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    index: HashMap<String, usize>,
    clusters: Vec<Cluster>,
    cluster_index: HashMap<String, usize>,
    composite_ids: HashSet<String>,
    stack: Vec<ClusterId>,
    edges: Vec<Edge>,
    descriptions: HashMap<String, Vec<String>>,
    class_defs: HashMap<String, StyleOverrides>,
    class_assignments: Vec<(String, String)>,
    notes: usize,
}

/// Parse a state diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    let stmts = statements(body);
    let mut b = Builder::default();
    // Pass 1: which ids are composite states.
    for (_, s) in &stmts {
        if let Some(rest) = s.strip_prefix("state ") {
            if rest.trim_end().ends_with('{') {
                b.composite_ids
                    .insert(state_decl_id(rest.trim_end().trim_end_matches('{').trim()));
            }
        }
    }
    let mut direction = Direction::TopBottom;
    let mut header = false;
    let mut iter = stmts.into_iter().peekable();
    while let Some((line, s)) = iter.next() {
        if !header {
            if !s.starts_with("stateDiagram") {
                return Err(Error::parse(line, format!("{s}\nExpecting 'stateDiagram'")));
            }
            header = true;
            continue;
        }
        if let Some(d) = s.strip_prefix("direction ") {
            if b.stack.is_empty() {
                direction = Direction::parse(d.trim()).ok_or_else(|| {
                    Error::parse(line, format!("Invalid direction '{}'", d.trim()))
                })?;
            }
            continue;
        }
        if s == "}" {
            b.stack
                .pop()
                .ok_or_else(|| Error::parse(line, "Unexpected '}'"))?;
            continue;
        }
        if s == "--" || s.starts_with("hide ") || s.starts_with("scale ") || s.starts_with("click ")
        {
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
        if let Some(rest) = s.strip_prefix("class ") {
            let (ids, class) = rest
                .trim()
                .split_once(char::is_whitespace)
                .unwrap_or((rest.trim(), ""));
            for id in ids.split(',') {
                b.class_assignments
                    .push((id.trim().to_string(), class.trim().to_string()));
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("note ") {
            let rest = rest.trim();
            let (target, text) = match rest.split_once(':') {
                Some((head, text)) => (note_target(head), text.trim().to_string()),
                None => {
                    let mut lines = Vec::new();
                    for (_, l) in iter.by_ref() {
                        if l.trim() == "end note" {
                            break;
                        }
                        lines.push(l.trim().to_string());
                    }
                    (note_target(rest), lines.join("\n"))
                }
            };
            let t = b.touch(&target);
            b.notes += 1;
            let mut note = Node::new(format!("note{}", b.notes), text, Shape::Note);
            note.cluster = b.stack.last().copied();
            b.nodes.push(note);
            let n = b.nodes.len() - 1;
            let mut e = Edge::new(Endpoint::Node(NodeId(t)), Endpoint::Node(NodeId(n)));
            e.line = LineKind::Dotted;
            e.end = Head::None;
            b.edges.push(e);
            continue;
        }
        if let Some(rest) = s.strip_prefix("state ") {
            b.state_decl(line, rest.trim())?;
            continue;
        }
        if let Some(arrow) = s.find("-->") {
            let from = s[..arrow].trim();
            let (to, label) = match s[arrow + 3..].split_once(':') {
                Some((t, l)) => (
                    t.trim(),
                    Some(l.trim().to_string()).filter(|l| !l.is_empty()),
                ),
                None => (s[arrow + 3..].trim(), None),
            };
            if from.is_empty() || to.is_empty() {
                return Err(Error::parse(
                    line,
                    format!("{s}\nExpecting a state on both sides of '-->'"),
                ));
            }
            let a = b.endpoint(from, true);
            let c = b.endpoint(to, false);
            let mut e = Edge::new(a, c);
            e.label = label;
            b.edges.push(e);
            continue;
        }
        if let Some((id, desc)) = s.split_once(':') {
            let id = b.strip_class(id.trim());
            b.touch(&id);
            b.descriptions
                .entry(id)
                .or_default()
                .push(desc.trim().to_string());
            continue;
        }
        let id = b.strip_class(s.trim());
        if id.contains(char::is_whitespace) {
            return Err(Error::parse(line, format!("{s}\nUnexpected statement")));
        }
        b.touch(&id);
    }
    if !b.stack.is_empty() {
        return Err(Error::parse(
            body.lines().count(),
            "Unexpected end of input: missing '}'",
        ));
    }
    Ok(b.finish(direction, title))
}

fn note_target(head: &str) -> String {
    let words: Vec<&str> = head.split_whitespace().collect();
    // "right of X" / "left of X"
    words.get(2).map(|s| s.to_string()).unwrap_or_default()
}

/// `"Long name" as Id` → Id; `Id` → Id; `Id <<fork>>` → Id
fn state_decl_id(rest: &str) -> String {
    if let Some(i) = rest.find(" as ") {
        let (a, b) = (rest[..i].trim(), rest[i + 4..].trim());
        return if a.starts_with('"') {
            b.trim_matches('"').to_string()
        } else {
            a.to_string()
        };
    }
    rest.split_whitespace().next().unwrap_or("").to_string()
}

impl Builder {
    fn strip_class(&mut self, id: &str) -> String {
        match id.split_once(":::") {
            Some((i, c)) => {
                self.class_assignments
                    .push((i.trim().to_string(), c.trim().to_string()));
                i.trim().to_string()
            }
            None => id.to_string(),
        }
    }

    fn touch(&mut self, id: &str) -> usize {
        if let Some(&i) = self.index.get(id) {
            return i;
        }
        let mut n = Node::new(id, id, Shape::Rounded);
        n.cluster = self.stack.last().copied();
        self.nodes.push(n);
        self.index.insert(id.to_string(), self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    fn ensure_cluster(&mut self, id: &str) -> ClusterId {
        if let Some(&c) = self.cluster_index.get(id) {
            return ClusterId(c);
        }
        self.clusters.push(Cluster {
            id: id.to_string(),
            title: id.to_string(),
            parent: self.stack.last().copied(),
            style: StyleOverrides::default(),
            // Composites take slots in the order dataFetcher inserts them (pre-order).
            color_slot: Some(ColorSlot(self.clusters.len())),
        });
        self.cluster_index
            .insert(id.to_string(), self.clusters.len() - 1);
        ClusterId(self.clusters.len() - 1)
    }

    fn endpoint(&mut self, raw: &str, is_source: bool) -> Endpoint {
        let raw = self.strip_class(raw);
        if raw == "[*]" {
            let scope = self
                .stack
                .last()
                .map(|c| self.clusters[c.0].id.clone())
                .unwrap_or_else(|| "root".into());
            let (key, shape) = if is_source {
                (format!("{scope}_start"), Shape::StateStart)
            } else {
                (format!("{scope}_end"), Shape::StateEnd)
            };
            let i = self.touch(&key);
            self.nodes[i].shape = shape;
            self.nodes[i].label = String::new();
            return Endpoint::Node(NodeId(i));
        }
        if self.composite_ids.contains(&raw) {
            return Endpoint::Cluster(self.ensure_cluster(&raw));
        }
        Endpoint::Node(NodeId(self.touch(&raw)))
    }

    fn state_decl(&mut self, line: usize, rest: &str) -> Result<()> {
        let opens = rest.ends_with('{');
        let rest = rest.trim_end_matches('{').trim();
        let id = state_decl_id(rest);
        if id.is_empty() {
            return Err(Error::parse(line, "Expecting a state id after 'state'"));
        }
        let display = rest.find(" as ").map(|i| {
            let (a, b) = (rest[..i].trim(), rest[i + 4..].trim());
            if a.starts_with('"') {
                a.trim_matches('"').to_string()
            } else {
                b.trim_matches('"').to_string()
            }
        });
        if opens {
            let c = self.ensure_cluster(&id);
            if let Some(d) = display {
                self.clusters[c.0].title = d;
            }
            self.stack.push(c);
            return Ok(());
        }
        let i = self.touch(&id);
        if let Some(d) = display {
            self.nodes[i].label = d;
        }
        if rest.contains("<<fork>>") || rest.contains("<<join>>") {
            self.nodes[i].shape = Shape::ForkBar;
            self.nodes[i].label = String::new();
        } else if rest.contains("<<choice>>") {
            self.nodes[i].shape = Shape::Choice;
            self.nodes[i].label = String::new();
        } else if let Some((_, desc)) = rest.split_once(':') {
            self.descriptions
                .entry(id)
                .or_default()
                .push(desc.trim().to_string());
        }
        Ok(())
    }

    fn finish(mut self, direction: Direction, title: Option<String>) -> GraphDiagram {
        for (id, lines) in &self.descriptions {
            if let Some(&i) = self.index.get(id) {
                let n = &mut self.nodes[i];
                if n.shape == Shape::Rounded {
                    n.shape = Shape::Compartments;
                    n.sections = vec![
                        Section {
                            lines: vec![n.label.clone()],
                            centered: true,
                            bold: false,
                        },
                        Section {
                            lines: lines.clone(),
                            centered: false,
                            bold: false,
                        },
                    ];
                }
            }
        }
        for (id, class) in &self.class_assignments {
            let def = self.class_defs.get(class).cloned().unwrap_or_default();
            if let Some(&i) = self.index.get(id) {
                self.nodes[i].style = self.nodes[i].style.merged(&def);
                self.nodes[i].classes.push(class.clone());
            }
        }
        // Drop nodes accidentally created for composite ids (e.g. by descriptions).
        let composite: Vec<bool> = self
            .nodes
            .iter()
            .map(|n| self.cluster_index.contains_key(&n.id))
            .collect();
        let mut remap = vec![None; self.nodes.len()];
        let mut next = 0;
        for (i, c) in composite.iter().enumerate() {
            if !c {
                remap[i] = Some(next);
                next += 1;
            }
        }
        let cluster_of: Vec<Option<usize>> = self
            .nodes
            .iter()
            .map(|n| self.cluster_index.get(&n.id).copied())
            .collect();
        let nodes = self
            .nodes
            .into_iter()
            .zip(&composite)
            .filter(|(_, c)| !**c)
            .map(|(n, _)| n)
            .collect();
        let edges = self
            .edges
            .into_iter()
            .filter_map(|mut e| {
                let fix = |ep: Endpoint| match ep {
                    Endpoint::Node(NodeId(i)) => match remap[i] {
                        Some(j) => Some(Endpoint::Node(NodeId(j))),
                        None => cluster_of[i].map(|c| Endpoint::Cluster(ClusterId(c))),
                    },
                    c => Some(c),
                };
                e.from = fix(e.from)?;
                e.to = fix(e.to)?;
                Some(e)
            })
            .collect();
        GraphDiagram {
            kind: DiagramKind::State,
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
    fn simple_states() {
        let g = parse(
            "stateDiagram-v2\n[*] --> Still\nStill --> Moving : push\nMoving --> [*]",
            None,
        )
        .unwrap();
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.nodes[0].shape, Shape::StateStart);
        assert_eq!(g.edges[1].label.as_deref(), Some("push"));
        assert_eq!(g.nodes[3].shape, Shape::StateEnd);
    }

    #[test]
    fn composite_and_special_states() {
        let g = parse(
            "stateDiagram-v2\ndirection LR\nstate \"Big one\" as Big {\n  [*] --> Inner\n  Inner --> [*]\n}\n[*] --> Big\nstate f <<fork>>\nBig --> f\nInner : does work\nnote right of Inner : a note",
            None,
        )
        .unwrap();
        assert_eq!(g.direction, Direction::LeftRight);
        assert_eq!(g.clusters.len(), 1);
        assert_eq!(g.clusters[0].title, "Big one");
        assert!(g.edges.iter().any(|e| matches!(e.to, Endpoint::Cluster(_))));
        assert!(g.nodes.iter().any(|n| n.shape == Shape::ForkBar));
        assert!(g.nodes.iter().any(|n| n.shape == Shape::Compartments));
        assert!(g.nodes.iter().any(|n| n.shape == Shape::Note));
        // Inner [*] nodes live in the cluster.
        assert!(
            g.nodes
                .iter()
                .filter(|n| n.shape == Shape::StateStart)
                .count()
                == 2
        );
    }

    #[test]
    fn unbalanced_braces() {
        assert!(parse("stateDiagram-v2\nstate A {\nB-->C", None).is_err());
        assert!(parse("stateDiagram-v2\n}", None).is_err());
    }
}
