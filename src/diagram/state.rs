//! State diagrams (`stateDiagram`, `stateDiagram-v2`) → `GraphDiagram`.
//!
//! Composite states become clusters; `[*]` resolves to a start or end pseudo-state
//! scoped to the enclosing composite, as in mermaid.

use super::graph::{
    Cluster, ClusterId, ClusterKind, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId,
    Section, Shape, StyleOverrides,
};
use super::statements_multiline;
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
    /// Composites whose body is split into concurrency regions by `--`.
    divided: HashSet<String>,
    stack: Vec<ClusterId>,
    /// Palette slots handed out so far (regions reuse their composite's).
    composite_slots: usize,
    /// stateDb's `divider-id-N` counter.
    dividers: usize,
    edges: Vec<Edge>,
    descriptions: HashMap<String, Vec<String>>,
    class_defs: HashMap<String, StyleOverrides>,
    class_assignments: Vec<(String, String)>,
    notes: usize,
}

/// Parse a state diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    // A `state "…"` description may span lines (STATE_STRING matches newlines).
    let stmts = statements_multiline(body, |s| s.starts_with("state "));
    let mut b = Builder::default();
    // Pass 1: which ids are composite states, and which of them hold `--` regions.
    let mut open: Vec<String> = Vec::new();
    for (_, s) in &stmts {
        if let Some(rest) = s.strip_prefix("state ") {
            if rest.trim_end().ends_with('{') {
                let id = state_decl_id(rest.trim_end().trim_end_matches('{').trim());
                b.composite_ids.insert(id.clone());
                open.push(id);
            }
        } else if s == "}" {
            open.pop();
        } else if s == "--" {
            if let Some(id) = open.last() {
                b.divided.insert(id.clone());
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
            let d = Direction::parse(d.trim())
                .ok_or_else(|| Error::parse(line, format!("Invalid direction '{}'", d.trim())))?;
            match b.composite_on_top() {
                // A composite's own `direction` (stateDb keeps it as the state's `dir`).
                Some(c) => b.clusters[c.0].direction = Some(d),
                None => direction = d,
            }
            continue;
        }
        if s == "}" {
            // An unmatched `}` is accepted (and ignored) upstream.
            let Some(c) = b.stack.pop() else {
                continue;
            };
            if b.clusters[c.0].kind == ClusterKind::Region {
                b.stack.pop();
            }
            continue;
        }
        if s == "--" {
            // Close the current region of the composite and open the next one.
            if let Some(&c) = b.stack.last() {
                if b.clusters[c.0].kind == ClusterKind::Region {
                    b.stack.pop();
                    if let Some(&parent) = b.stack.last() {
                        b.open_region(parent);
                    }
                }
            }
            continue;
        }
        if s.starts_with("hide ") || s.starts_with("scale ") || s.starts_with("click ") {
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
                // An empty single-line note has no text token: mermaid 12.3 fails to
                // render it ("No such shape: undefined").
                Some((_, text)) if text.trim().is_empty() => {
                    return Err(Error::parse(line, format!("{s}\nExpecting the note text")));
                }
                Some((head, text)) => (note_target(head), text.trim().to_string()),
                None => {
                    let mut lines = Vec::new();
                    let mut closed = false;
                    for (_, l) in iter.by_ref() {
                        if l.trim() == "end note" {
                            closed = true;
                            break;
                        }
                        lines.push(l.trim().to_string());
                    }
                    // The lexer's note state needs `end note`: without it the
                    // `note …` line itself is unrecognised.
                    if !closed {
                        return Err(Error::parse(
                            line,
                            format!("Lexical error on line {line}. Unrecognized text.\n{s}"),
                        ));
                    }
                    (note_target(rest), lines.join("\n"))
                }
            };
            let t = b.touch(&target);
            b.notes += 1;
            // dataFetcher wraps the note in an invisible `noteGroup` at the top level,
            // whatever composite the annotated state lives in: the note sits outside
            // it, and its edge crosses the composite's border (and the group's).
            let id = format!("note{}", b.notes);
            let mut group = Cluster::new(format!("{id}----parent"), text.clone(), None);
            group.kind = ClusterKind::NoteGroup;
            b.clusters.push(group);
            let mut note = Node::new(id, text, Shape::Note);
            note.cluster = Some(ClusterId(b.clusters.len() - 1));
            b.nodes.push(note);
            let n = b.nodes.len() - 1;
            let (from, to) = if rest.starts_with("left of") {
                (n, t)
            } else {
                (t, n)
            };
            let mut e = Edge::new(Endpoint::Node(NodeId(from)), Endpoint::Node(NodeId(to)));
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
            e.end = Head::Barb;
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

/// `g` without its note groups: their notes stand on their own (the layouts
/// other than ELK do not model the groups yet).
pub fn without_note_groups(mut g: GraphDiagram) -> GraphDiagram {
    let keep: Vec<bool> = g
        .clusters
        .iter()
        .map(|c| c.kind != ClusterKind::NoteGroup)
        .collect();
    let mut remap = vec![None; keep.len()];
    let mut next = 0;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            remap[i] = Some(ClusterId(next));
            next += 1;
        }
    }
    let fix = |c: Option<ClusterId>| c.and_then(|c| remap[c.0]);
    for n in &mut g.nodes {
        n.cluster = fix(n.cluster);
    }
    let mut k = keep.iter();
    g.clusters.retain(|_| *k.next().unwrap_or(&true));
    for c in &mut g.clusters {
        c.parent = fix(c.parent);
    }
    for e in &mut g.edges {
        for end in [&mut e.from, &mut e.to] {
            if let Endpoint::Cluster(c) = *end
                && let Some(c) = remap[c.0]
            {
                *end = Endpoint::Cluster(c);
            }
        }
    }
    g
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
        // dataFetcher's `rect` with `rx`/`ry` set: nodes.ts draws it as `roundedRect`
        // (padding 8, the CSS `rx: 5px`).
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
        let mut c = Cluster::new(id, id, self.stack.last().copied());
        c.kind = ClusterKind::Composite;
        // Composites take slots in the order dataFetcher inserts them (pre-order).
        c.color_slot = Some(ColorSlot(self.composite_slots));
        self.composite_slots += 1;
        self.clusters.push(c);
        self.cluster_index
            .insert(id.to_string(), self.clusters.len() - 1);
        ClusterId(self.clusters.len() - 1)
    }

    /// A concurrency region of `parent` (stateDb's `divider` state): untitled, and
    /// painted with its composite's slot.
    fn open_region(&mut self, parent: ClusterId) {
        self.dividers += 1;
        let mut r = Cluster::new(format!("divider-id-{}", self.dividers), "", Some(parent));
        r.kind = ClusterKind::Region;
        r.color_slot = self.clusters[parent.0].color_slot;
        self.clusters.push(r);
        self.stack.push(ClusterId(self.clusters.len() - 1));
    }

    /// The innermost open composite (skipping the region it may be split into).
    fn composite_on_top(&self) -> Option<ClusterId> {
        self.stack
            .iter()
            .rev()
            .copied()
            .find(|c| self.clusters[c.0].kind == ClusterKind::Composite)
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
            if self.divided.contains(&id) {
                self.open_region(c);
            }
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
        // dataFetcher.ts `getDir(item, DEFAULT_NESTED_DOC_DIR = 'TB')`: a composite with no
        // `direction` of its own is laid out top to bottom, whatever the diagram's direction.
        for c in &mut self.clusters {
            c.direction.get_or_insert(Direction::TopBottom);
        }
        // dataFetcher: a state's description starts as its display name (or id). The
        // first `id : text` replaces a bare id and otherwise appends; one entry keeps
        // the plain shape with that entry as its label, more make a `rectWithTitle`
        // titled by the first entry over the rest, both centred.
        for (id, descs) in &self.descriptions {
            if let Some(&i) = self.index.get(id) {
                let n = &mut self.nodes[i];
                if n.shape != Shape::Rounded {
                    continue;
                }
                let mut all = Vec::with_capacity(descs.len() + 1);
                if n.label != n.id {
                    all.push(n.label.clone());
                }
                all.extend(descs.iter().cloned());
                if let [only] = all.as_slice() {
                    n.label = only.clone();
                    continue;
                }
                n.shape = Shape::Compartments;
                n.sections = vec![
                    Section {
                        lines: vec![all[0].clone()],
                        centered: true,
                        bold: false,
                    },
                    Section {
                        lines: all[1..].to_vec(),
                        centered: true,
                        bold: false,
                    },
                ];
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
        // The composite and the note's group.
        assert_eq!(g.clusters.len(), 2);
        assert_eq!(g.clusters[0].title, "Big one");
        assert_eq!(g.clusters[1].kind, ClusterKind::NoteGroup);
        assert!(g.edges.iter().any(|e| matches!(e.to, Endpoint::Cluster(_))));
        assert!(g.nodes.iter().any(|n| n.shape == Shape::ForkBar));
        // A single description replaces the label of the plain shape.
        let inner = g.nodes.iter().find(|n| n.id == "Inner").unwrap();
        assert_eq!(
            (inner.shape, inner.label.as_str()),
            (Shape::Rounded, "does work")
        );
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
        // An unmatched `}` is ignored upstream.
        assert!(parse("stateDiagram-v2\n}", None).is_ok());
        // Regions do not change the brace count.
        assert!(parse("stateDiagram-v2\nstate A {\nB\n--\nC\n}\nD", None).is_ok());
    }

    #[test]
    fn concurrency_regions() {
        let g = parse(
            "stateDiagram-v2\nstate Active {\n[*] --> A1\n--\n[*] --> B1\n--\n[*] --> C1\n}\nActive --> Done",
            None,
        )
        .unwrap();
        let regions: Vec<&Cluster> = g
            .clusters
            .iter()
            .filter(|c| c.kind == ClusterKind::Region)
            .collect();
        assert_eq!(regions.len(), 3);
        let active = &g.clusters[0];
        assert_eq!(active.kind, ClusterKind::Composite);
        for r in &regions {
            assert_eq!(r.parent, Some(ClusterId(0)));
            assert!(r.title.is_empty());
            assert_eq!(
                r.color_slot, active.color_slot,
                "regions share the composite's slot"
            );
        }
        // Each region has its own start state and its own member.
        let starts: Vec<&Node> = g
            .nodes
            .iter()
            .filter(|n| n.shape == Shape::StateStart)
            .collect();
        assert_eq!(starts.len(), 3);
        let mut owners: Vec<_> = starts.iter().map(|n| n.cluster).collect();
        owners.dedup();
        assert_eq!(owners.len(), 3);
        for id in ["A1", "B1", "C1"] {
            let n = g.nodes.iter().find(|n| n.id == id).unwrap();
            assert_eq!(g.clusters[n.cluster.unwrap().0].kind, ClusterKind::Region);
        }
        // `Done` is outside, and a later composite takes the next slot.
        let g2 = parse(
            "stateDiagram-v2\nstate A {\nx\n--\ny\n}\nstate B {\nz\n}",
            None,
        )
        .unwrap();
        let b = g2.clusters.iter().find(|c| c.id == "B").unwrap();
        assert_eq!(b.color_slot, Some(ColorSlot(1)));
    }

    #[test]
    fn unterminated_multiline_note_is_a_lexical_error() {
        let e = parse(
            "stateDiagram-v2\n  A --> B\n  note left of A\n    text",
            None,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e}");
        assert!(e.to_string().contains("Lexical error on line 3"));
    }

    #[test]
    fn composite_direction_and_notes() {
        let g = parse(
            "stateDiagram-v2\ndirection LR\nstate B {\ndirection TB\na --> b\nnote left of b : hi\n}",
            None,
        )
        .unwrap();
        assert_eq!(g.direction, Direction::LeftRight);
        assert_eq!(g.clusters[0].direction, Some(Direction::TopBottom));
        // dataFetcher puts the note's group at the top level, outside the composite,
        // and a `left of` note's edge runs from the note to the state.
        let note = g.nodes.iter().position(|n| n.shape == Shape::Note).unwrap();
        let group = g.nodes[note].cluster.expect("a note group");
        assert_eq!(g.clusters[group.0].kind, ClusterKind::NoteGroup);
        assert_eq!(g.clusters[group.0].parent, None);
        assert_eq!(g.clusters[group.0].title, "hi");
        let e = g.edges.last().unwrap();
        assert_eq!(e.from, Endpoint::Node(NodeId(note)));
        // The layouts other than ELK go without the groups.
        let flat = without_note_groups(g.clone());
        assert_eq!(flat.clusters.len(), g.clusters.len() - 1);
        assert!(
            flat.clusters
                .iter()
                .all(|c| c.kind != ClusterKind::NoteGroup)
        );
        assert_eq!(flat.nodes[note].cluster, None);
        let b = flat.nodes.iter().position(|n| n.id == "b").unwrap();
        assert_eq!(flat.clusters[flat.nodes[b].cluster.unwrap().0].id, "B");
    }

    /// The first rect drawn in the group with id `id` (or whose id starts so),
    /// other than a drop shadow: `Some(None)` for a group drawing none.
    fn drawn_rect(scene: &crate::scene::Scene, id: &str) -> Option<Option<crate::geom::Rect>> {
        use crate::scene::{Element, Geometry};
        fn find(e: &Element, id: &str) -> Option<Option<crate::geom::Rect>> {
            match e {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i.starts_with(id) => Some(children.iter().find_map(|c| match c {
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        class,
                        ..
                    } if class.as_deref() != Some("drop-shadow") => Some(*rect),
                    _ => None,
                })),
                Element::Group { children, .. } => children.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        scene.elements.iter().find_map(|e| find(e, id))
    }

    /// mermaid 12.3.0 under ELK: a note sits in its invisible `noteGroup`,
    /// which ELK pads (24) and tops with the note's text as its label (24),
    /// so the state after the annotated one, beside the group, comes 125
    /// below it (B at 205.5 from A's 80.5); a composite's children start
    /// below its title band as a subgraph's do (24, not 23).
    #[test]
    fn elk_notes_stand_in_their_groups_and_composites_keep_their_title_band() {
        let render = |src: &str| {
            crate::render::render(src, &crate::config::MermaidConfig::default())
                .expect("renders")
                .scene
        };
        let scene = render(
            "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]\n  note right of A : a note\n",
        );
        assert_eq!(drawn_rect(&scene, "cluster-note1----parent"), Some(None));
        let (a, b) = (
            drawn_rect(&scene, "state-A-").flatten().expect("A"),
            drawn_rect(&scene, "state-B-").flatten().expect("B"),
        );
        assert!((b.y - a.y - 125.0).abs() < 1e-6, "{a:?} {b:?}");
        let scene = render("stateDiagram-v2\n  state M {\n    a --> b\n  }\n");
        let m = drawn_rect(&scene, "cluster-M").flatten().expect("M");
        let a = drawn_rect(&scene, "state-a-").flatten().expect("a");
        assert!((a.y - m.y - 48.0).abs() < 1e-6, "{m:?} {a:?}");
    }

    /// The centre of the first (non-shadow) circle drawn in group `id…`.
    fn drawn_circle(scene: &crate::scene::Scene, id: &str) -> Option<crate::geom::Point> {
        use crate::scene::{Element, Geometry};
        fn find(e: &Element, id: &str) -> Option<crate::geom::Point> {
            match e {
                Element::Group {
                    id: Some(i),
                    children,
                    ..
                } if i.starts_with(id) => children.iter().find_map(|c| match c {
                    Element::Shape {
                        geom: Geometry::Circle { c, .. },
                        class,
                        ..
                    } if class.as_deref() != Some("drop-shadow") => Some(*c),
                    _ => None,
                }),
                Element::Group { children, .. } => children.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        scene.elements.iter().find_map(|e| find(e, id))
    }

    /// mermaid 12.3.0 under ELK (left to right): ELK keeps a port 10 from the
    /// ends of its side, so a 14px start or end state's edge leaves 10 from its
    /// top, and the node is laid out around that; mermaid then centres the node
    /// on the edge (`alignDegenerateNodeToAnchor`). The end state, below its
    /// note's group, comes 49.75 below C (we had it 3 short), and everything
    /// else 1.5 lower than we had it, as the balanced placement averages it.
    #[test]
    fn elk_start_and_end_states_stand_on_their_ports() {
        let src = "stateDiagram-v2\n  direction LR\n  [*] --> A\n  A --> B\n  B --> C\n  \
                   note left of B : left note\n  note right of C : right\n  C --> [*]\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .expect("renders")
            .scene;
        let a = drawn_rect(&scene, "state-A-").flatten().expect("A");
        let c = drawn_rect(&scene, "state-C-").flatten().expect("C");
        let note = drawn_rect(&scene, "state-note2-").flatten().expect("note");
        let start = drawn_circle(&scene, "state-root_start-").expect("start");
        let end = drawn_circle(&scene, "state-root_end-").expect("end");
        let mid = |r: crate::geom::Rect| r.y + r.h / 2.0;
        assert!((start.y - mid(a)).abs() < 1e-6, "{start:?} {a:?}");
        assert!((end.y - mid(c) - 49.75).abs() < 1e-6, "{end:?} {c:?}");
        // mermaid: A at 69.25, the right note at 85.5.
        assert!((mid(note) - mid(a) - 16.25).abs() < 1e-6, "{note:?} {a:?}");
    }

    /// stateRenderer-v3-unified: `rankSpacing: conf?.rankSpacing || 50` (the
    /// state schema has no default), which dagre takes.
    #[test]
    fn dagre_ranks_are_fifty_apart() {
        let src = "---\nconfig:\n  layout: dagre\n---\nstateDiagram-v2\n  A --> B\n";
        let scene = crate::render::render(src, &crate::config::MermaidConfig::default())
            .expect("renders")
            .scene;
        let (a, b) = (
            drawn_rect(&scene, "state-A-").flatten().expect("A"),
            drawn_rect(&scene, "state-B-").flatten().expect("B"),
        );
        assert!((b.y - a.bottom() - 50.0).abs() < 1e-6, "{a:?} {b:?}");
    }

    #[test]
    fn composites_default_to_top_bottom_like_upstream() {
        let g = parse(
            "stateDiagram-v2\ndirection LR\nstate B {\nb1 --> b2\n}\nA --> B",
            None,
        )
        .unwrap();
        assert_eq!(g.direction, Direction::LeftRight);
        assert_eq!(g.clusters[0].direction, Some(Direction::TopBottom));
    }

    #[test]
    fn state_descriptions_may_span_lines() {
        let g = parse(
            "stateDiagram-v2\nstate \"multi\nline\" as S5\nS5 --> S6",
            None,
        )
        .unwrap();
        let s5 = g.nodes.iter().find(|n| n.id == "S5").unwrap();
        assert_eq!(
            crate::text::TextBlock::label(&s5.label, 16.0).lines,
            ["multi", "line"]
        );
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn descriptions_follow_the_data_fetcher() {
        // Mirrors mermaid 12.3.0's output: the id is dropped by the first description,
        // a display name is kept as the title, and further entries form the body.
        let g = parse(
            "stateDiagram-v2\nstate \"Display name\" as A\nA : one desc\nB : d1\nB : d2\nB : d3\nC --> A",
            None,
        )
        .unwrap();
        let node = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        let sections = |id: &str| -> Vec<Vec<String>> {
            node(id).sections.iter().map(|s| s.lines.clone()).collect()
        };
        assert_eq!(node("A").shape, Shape::Compartments);
        assert_eq!(sections("A"), [vec!["Display name"], vec!["one desc"]]);
        assert_eq!(sections("B"), [vec!["d1"], vec!["d2", "d3"]]);
        assert!(node("B").sections.iter().all(|s| s.centered));
        assert_eq!(
            (node("C").shape, node("C").label.as_str()),
            (Shape::Rounded, "C")
        );
    }

    #[test]
    fn state_shapes_match_mermaid_sizes() {
        // Reference SVG (mermaid 12.3.0, default config): a plain state is a
        // roundedRect with padding 8 around its 120x21 label, 136x37; a two-entry
        // description box is the label bbox plus 8 (title 21 + 9 + body 21 = 51);
        // a note pads its label by 15, a fork bar is 70x10 and a choice 28x28.
        let src = "stateDiagram-v2\n[*] --> Still\nStill --> D\nD : first\nD : second\nnote right of Still : A note here\nstate f <<fork>>\nstate c <<choice>>\nD --> f\nf --> c";
        let g = parse(src, None).unwrap();
        let cfg = crate::config::MermaidConfig::default().scoped("state");
        let pal = crate::theme::palette(cfg.theme, &[]);
        let size = |n: &Node| {
            super::super::graph::node_size(n, DiagramKind::State, &cfg.state, &pal, false)
        };
        let by_id = |id: &str| size(g.nodes.iter().find(|n| n.id == id).unwrap());
        let close = |(w, h): (f64, f64), (ew, eh): (f64, f64)| {
            assert!(
                (w - ew).abs() < 1e-6 && (h - eh).abs() < 1e-6,
                "{w}x{h} vs {ew}x{eh}"
            );
        };
        close(by_id("Still"), (136.0, 37.0));
        assert!((by_id("D").1 - 59.0).abs() < 1e-6, "{:?}", by_id("D"));
        close(by_id("f"), (70.0, 10.0));
        close(by_id("c"), (28.0, 28.0));
        close(by_id("root_start"), (14.0, 14.0));
        let note = g.nodes.iter().find(|n| n.shape == Shape::Note).unwrap();
        close(size(note), (150.0, 51.0));
    }

    /// Every stroked shape of a rendered state diagram with the id of the group
    /// holding it.
    fn stroked(src: &str) -> Vec<(String, crate::scene::Paint)> {
        use crate::scene::Element;
        fn walk(e: &Element, group: &str, out: &mut Vec<(String, crate::scene::Paint)>) {
            match e {
                Element::Group { id, children, .. } => {
                    let g = id.as_deref().unwrap_or(group);
                    for c in children {
                        walk(c, g, out);
                    }
                }
                Element::Shape { paint, .. } if paint.stroke_width > 0.0 => {
                    out.push((group.to_string(), paint.clone()));
                }
                _ => {}
            }
        }
        let r =
            crate::render::render(src, &crate::config::MermaidConfig::default()).expect("renders");
        let mut out = Vec::new();
        for e in &r.scene.elements {
            walk(e, "", &mut out);
        }
        out
    }

    const NOTED: &str = "stateDiagram-v2\n  [*] --> A\n  state c <<choice>>\n  A --> c\n  c --> A : yes\n  note right of A : a note";

    /// The note edge: 2px dashes under the (default) neo look, which writes the
    /// pattern inline (`generateDashArray`: `0 0 2 2 … 2 2 0`, mermaid 12.3.0);
    /// `.note-edge`'s 5 under the classic look.
    #[test]
    fn note_edge_dashes_follow_the_look() {
        let dash = |src: &str| {
            stroked(src)
                .into_iter()
                .filter(|(g, _)| g.starts_with("L-"))
                .find_map(|(_, p)| p.dash)
        };
        let neo = dash(NOTED).expect("a pattern");
        let n = neo.len();
        assert!(n > 5 && n % 2 == 1, "{neo:?}");
        assert_eq!(neo[..2], [0.0, 0.0]);
        assert_eq!(neo[n - 1], 0.0);
        assert!(neo[2..n - 1].iter().all(|&d| d == 2.0), "{neo:?}");
        let classic = format!("---\nconfig:\n  look: classic\n---\n{NOTED}");
        assert_eq!(dash(&classic), Some(vec![5.0]));
    }

    /// Choice and note outlines are rough.js paths at its 1.3px.
    #[test]
    fn choice_and_note_outlines_are_rough_width() {
        let shapes = stroked(NOTED);
        let width = |part: &str| {
            shapes
                .iter()
                .find(|(g, _)| g.starts_with("state-") && g.contains(part))
                .map(|(_, p)| p.stroke_width)
        };
        assert_eq!(width("-c-"), Some(1.3));
        assert_eq!(width("-note"), Some(1.3));
        assert_ne!(width("-A-"), Some(1.3));
    }
}
