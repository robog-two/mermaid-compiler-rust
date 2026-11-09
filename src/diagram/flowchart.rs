//! Flowchart (`graph` / `flowchart`) parser → `GraphDiagram`.
//!
//! Hand-written scanner following mermaid's `flow.jison` token rules for links
//! (`-->`, `---`, `-.->`, `==>`, `--o`, `--x`, `<-->`, `~~~`, `-- text -->`,
//! `-->|text|`, extra dashes for longer edges), node shapes, `&` groups,
//! `:::class`, subgraphs, `classDef`, `class`, `style` and `linkStyle`.

use super::graph::{
    Cluster, ClusterId, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Shape,
    StyleOverrides,
};
use super::statements;
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::DiagramKind;
use std::collections::HashMap;

/// Parse a preprocessed flowchart body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    let mut b = Builder::default();
    let mut header_seen = false;
    let mut direction = Direction::TopBottom;
    for (line, stmt) in statements(body) {
        if !header_seen {
            let mut words = stmt.split_whitespace();
            let kw = words.next().unwrap_or("");
            if !matches!(kw, "graph" | "flowchart" | "flowchart-v2" | "flowchart-elk") {
                return Err(Error::parse(
                    line,
                    format!("{stmt}\nExpecting 'graph' or 'flowchart'"),
                ));
            }
            if let Some(d) = words.next() {
                direction = Direction::parse(d).ok_or_else(|| {
                    Error::parse(
                        line,
                        format!("{stmt}\nInvalid direction '{d}', expecting TB, TD, BT, RL or LR"),
                    )
                })?;
            }
            header_seen = true;
            continue;
        }
        b.statement(line, &stmt)?;
    }
    if !b.stack.is_empty() {
        return Err(Error::parse(
            body.lines().count(),
            "Unexpected end of input: missing 'end' for subgraph",
        ));
    }
    b.finish(direction, title)
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    index: HashMap<String, usize>,
    clusters: Vec<Cluster>,
    cluster_index: HashMap<String, usize>,
    stack: Vec<ClusterId>,
    edges: Vec<Edge>,
    class_defs: HashMap<String, StyleOverrides>,
    node_styles: Vec<(String, StyleOverrides)>,
    class_assignments: Vec<(String, String)>,
    link_styles: Vec<(usize, Option<Vec<usize>>, StyleOverrides)>,
    auto_subgraph: usize,
}

fn err_at(line: usize, stmt: &str, col: usize, msg: &str) -> Error {
    Error::parse(line, format!("{stmt}\n{}^\n{msg}", "-".repeat(col)))
}

impl Builder {
    fn statement(&mut self, line: usize, stmt: &str) -> Result<()> {
        let (kw, rest) = match stmt.find(char::is_whitespace) {
            Some(i) => (&stmt[..i], stmt[i..].trim()),
            None => (stmt, ""),
        };
        match kw {
            "subgraph" => self.subgraph(line, rest),
            "end" if rest.is_empty() => self.stack.pop().map(|_| ()).ok_or_else(|| {
                err_at(
                    line,
                    stmt,
                    0,
                    "Unexpected 'end' without matching 'subgraph'",
                )
            }),
            "direction" => Ok(()),
            "classDef" => {
                let (names, style) = split_first_word(rest);
                let st = StyleOverrides::parse(style);
                for n in names.split(',') {
                    let e = self.class_defs.entry(n.trim().to_string()).or_default();
                    *e = e.merged(&st);
                }
                Ok(())
            }
            "class" => {
                let (ids, class) = split_first_word(rest);
                for id in ids.split(',') {
                    self.class_assignments
                        .push((id.trim().to_string(), class.trim().to_string()));
                }
                Ok(())
            }
            "style" => {
                let (id, style) = split_first_word(rest);
                self.node_styles
                    .push((id.to_string(), StyleOverrides::parse(style)));
                Ok(())
            }
            "linkStyle" => {
                let (which, style) = split_first_word(rest);
                let st = StyleOverrides::parse(style.trim_start_matches("interpolate").trim());
                let idx = if which == "default" {
                    None
                } else {
                    let parsed: std::result::Result<Vec<usize>, _> = which
                        .split(',')
                        .map(|s| s.trim().parse::<usize>())
                        .collect();
                    Some(parsed.map_err(|_| {
                        err_at(line, stmt, 10, "Expecting a link index or 'default'")
                    })?)
                };
                self.link_styles.push((line, idx, st));
                Ok(())
            }
            "click" | "href" | "callback" => Ok(()),
            _ => self.chain(line, stmt),
        }
    }

    fn subgraph(&mut self, _line: usize, rest: &str) -> Result<()> {
        let rest = rest.trim();
        let (id, title) = if let Some(q) = rest.strip_prefix('"') {
            self.auto_subgraph += 1;
            (
                format!("subGraph{}", self.auto_subgraph - 1),
                q.trim_end_matches('"').to_string(),
            )
        } else if let Some(open) = rest.find('[') {
            let id = rest[..open].trim().to_string();
            let t = rest[open + 1..].trim_end().trim_end_matches(']');
            (id, t.trim().trim_matches('"').to_string())
        } else if rest.is_empty() {
            self.auto_subgraph += 1;
            (format!("subGraph{}", self.auto_subgraph - 1), String::new())
        } else {
            (rest.to_string(), rest.to_string())
        };
        let parent = self.stack.last().copied();
        let ci = match self.cluster_index.get(&id) {
            Some(&ci) => ci,
            None => {
                self.clusters.push(Cluster {
                    id: id.clone(),
                    title,
                    parent,
                    style: StyleOverrides::default(),
                });
                self.cluster_index.insert(id, self.clusters.len() - 1);
                self.clusters.len() - 1
            }
        };
        self.stack.push(ClusterId(ci));
        Ok(())
    }

    /// Register / update a node and place it in the current subgraph.
    fn touch(&mut self, id: &str, shape: Option<(Shape, String)>, class: Option<String>) -> usize {
        let idx = match self.index.get(id) {
            Some(&i) => i,
            None => {
                self.nodes.push(Node::new(id, id, Shape::Rect));
                self.index.insert(id.to_string(), self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        if let Some((s, label)) = shape {
            self.nodes[idx].shape = s;
            self.nodes[idx].label = label;
        }
        if let Some(c) = class {
            self.class_assignments.push((id.to_string(), c));
        }
        if let Some(&cur) = self.stack.last() {
            let deeper = match self.nodes[idx].cluster {
                None => true,
                Some(existing) => self.is_ancestor(existing, cur),
            };
            if deeper {
                self.nodes[idx].cluster = Some(cur);
            }
        }
        idx
    }

    fn is_ancestor(&self, anc: ClusterId, mut c: ClusterId) -> bool {
        for _ in 0..=self.clusters.len() {
            if c == anc {
                return true;
            }
            match self.clusters[c.0].parent {
                Some(p) => c = p,
                None => return false,
            }
        }
        false
    }

    fn chain(&mut self, line: usize, stmt: &str) -> Result<()> {
        let mut sc = Scanner::new(stmt);
        let mut prev = self.group(&mut sc, line, stmt)?;
        loop {
            sc.ws();
            if sc.done() {
                break;
            }
            let col = sc.pos;
            let link = sc.link().ok_or_else(|| {
                err_at(
                    line,
                    stmt,
                    col,
                    &format!("Expecting a link (e.g. '-->'), got '{}'", sc.rest()),
                )
            })?;
            sc.ws();
            let next = self.group(&mut sc, line, stmt)?;
            for (_, a) in &prev {
                for (_, b) in &next {
                    let mut e = Edge::new(Endpoint::Node(NodeId(*a)), Endpoint::Node(NodeId(*b)));
                    e.line = link.line;
                    e.start = link.start;
                    e.end = link.end;
                    e.minlen = link.minlen;
                    e.label = link.label.clone();
                    self.edges.push(e);
                }
            }
            prev = next;
        }
        Ok(())
    }

    fn group(&mut self, sc: &mut Scanner, line: usize, stmt: &str) -> Result<Vec<(String, usize)>> {
        let mut out = Vec::new();
        loop {
            sc.ws();
            let col = sc.pos;
            let Some(r) = sc.node_ref().map_err(|m| err_at(line, stmt, sc.pos, &m))? else {
                return Err(err_at(
                    line,
                    stmt,
                    col,
                    &format!("Expecting a node id, got '{}'", sc.rest()),
                ));
            };
            let idx = self.touch(&r.id, r.shape, r.class);
            out.push((r.id, idx));
            sc.ws();
            if sc.peek() == Some('&') {
                sc.pos += 1;
                continue;
            }
            return Ok(out);
        }
    }

    fn finish(mut self, direction: Direction, title: Option<String>) -> Result<GraphDiagram> {
        let default_def = self.class_defs.get("default").cloned().unwrap_or_default();
        for n in &mut self.nodes {
            n.style = default_def.clone();
        }
        for (id, class) in &self.class_assignments {
            let def = self.class_defs.get(class).cloned().unwrap_or_default();
            if let Some(&i) = self.index.get(id) {
                self.nodes[i].style = self.nodes[i].style.merged(&def);
                self.nodes[i].classes.push(class.clone());
            }
            if let Some(&c) = self.cluster_index.get(id) {
                self.clusters[c].style = self.clusters[c].style.merged(&def);
            }
        }
        for (id, st) in &self.node_styles {
            if let Some(&i) = self.index.get(id) {
                self.nodes[i].style = self.nodes[i].style.merged(st);
            }
            if let Some(&c) = self.cluster_index.get(id) {
                self.clusters[c].style = self.clusters[c].style.merged(st);
            }
        }
        for (line, idx, st) in &self.link_styles {
            match idx {
                None => {
                    for e in &mut self.edges {
                        e.style = e.style.merged(st);
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
        // Subgraph ids used as plain nodes would otherwise render as stray boxes.
        let cluster_only: Vec<bool> = self
            .nodes
            .iter()
            .map(|n| {
                self.cluster_index.contains_key(&n.id) && n.label == n.id && n.shape == Shape::Rect
            })
            .collect();
        let remap: Vec<Option<usize>> = {
            let mut next = 0;
            cluster_only
                .iter()
                .map(|&skip| {
                    if skip {
                        None
                    } else {
                        next += 1;
                        Some(next - 1)
                    }
                })
                .collect()
        };
        let as_cluster: Vec<Option<usize>> = self
            .nodes
            .iter()
            .map(|n| self.cluster_index.get(&n.id).copied())
            .collect();
        let nodes: Vec<Node> = self
            .nodes
            .into_iter()
            .zip(&cluster_only)
            .filter(|(_, s)| !**s)
            .map(|(n, _)| n)
            .collect();
        let edges = self
            .edges
            .into_iter()
            .filter_map(|mut e| {
                let fix = |ep: Endpoint| match ep {
                    Endpoint::Node(NodeId(i)) => match remap[i] {
                        Some(j) => Some(Endpoint::Node(NodeId(j))),
                        None => as_cluster[i].map(|c| Endpoint::Cluster(ClusterId(c))),
                    },
                    c => Some(c),
                };
                e.from = fix(e.from)?;
                e.to = fix(e.to)?;
                Some(e)
            })
            .collect();
        Ok(GraphDiagram {
            kind: DiagramKind::Flowchart,
            direction,
            nodes,
            edges,
            clusters: self.clusters,
            title,
        })
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

struct NodeRef {
    id: String,
    shape: Option<(Shape, String)>,
    class: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Link {
    line: LineKind,
    start: Head,
    end: Head,
    minlen: usize,
    label: Option<String>,
}

struct Scanner {
    c: Vec<char>,
    pos: usize,
}

fn head_of(c: char) -> Head {
    match c {
        '>' | '<' => Head::Arrow,
        'x' => Head::Cross,
        'o' => Head::Circle,
        _ => Head::None,
    }
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

    fn is_link_start(&self, i: usize) -> bool {
        let (a, b) = (self.at(i), self.at(i + 1));
        matches!(
            (a, b),
            (Some('-'), Some('-' | '.' | '>'))
                | (Some('='), Some('=' | '>'))
                | (Some('~'), Some('~'))
                | (Some('.'), Some('-'))
        )
    }

    fn node_ref(&mut self) -> std::result::Result<Option<NodeRef>, String> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            let ok = ch.is_alphanumeric()
                || ch == '_'
                || ch == '$'
                || ch == '\''
                || (ch == '-' && !self.is_link_start(self.pos))
                || (ch == '.'
                    && !self.is_link_start(self.pos)
                    && self.at(self.pos + 1).is_some_and(char::is_alphanumeric));
            if !ok {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Ok(None);
        }
        let id: String = self.c[start..self.pos].iter().collect();
        let shape = self.shape()?;
        let class = if self.starts_with(":::") {
            self.pos += 3;
            let s = self.pos;
            while self
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                self.pos += 1;
            }
            Some(self.c[s..self.pos].iter().collect())
        } else {
            None
        };
        Ok(Some(NodeRef { id, shape, class }))
    }

    fn shape(&mut self) -> std::result::Result<Option<(Shape, String)>, String> {
        if self.starts_with("@{") {
            return self.shape_attrs().map(Some);
        }
        const OPENERS: &[(&str, &[(&str, Shape)])] = &[
            ("(((", &[(")))", Shape::DoubleCircle)]),
            ("((", &[("))", Shape::Circle)]),
            ("([", &[("])", Shape::Stadium)]),
            ("(", &[(")", Shape::Rounded)]),
            ("[[", &[("]]", Shape::Subroutine)]),
            ("[(", &[(")]", Shape::Cylinder)]),
            ("[/", &[("/]", Shape::LeanRight), ("\\]", Shape::Trapezoid)]),
            (
                "[\\",
                &[("\\]", Shape::LeanLeft), ("/]", Shape::InvTrapezoid)],
            ),
            ("[", &[("]", Shape::Rect)]),
            ("{{", &[("}}", Shape::Hexagon)]),
            ("{", &[("}", Shape::Diamond)]),
            (">", &[("]", Shape::Asymmetric)]),
        ];
        for (open, closers) in OPENERS {
            if !self.starts_with(open) {
                continue;
            }
            // `>` is only a shape opener when not part of a link like `-->`.
            let save = self.pos;
            self.pos += open.chars().count();
            let text_start = self.pos;
            let mut quoted: Option<String> = None;
            {
                let mut p = self.pos;
                while self.at(p).is_some_and(char::is_whitespace) {
                    p += 1;
                }
                if self.at(p) == Some('"') {
                    if let Some(end) = (p + 1..self.c.len()).find(|&i| self.c[i] == '"') {
                        quoted = Some(self.c[p + 1..end].iter().collect());
                        self.pos = end + 1;
                        while self.peek().is_some_and(char::is_whitespace) {
                            self.pos += 1;
                        }
                    }
                }
            }
            let search_from = self.pos;
            let mut found: Option<(usize, &str, Shape)> = None;
            for i in search_from..self.c.len() {
                for (close, shape) in closers.iter() {
                    if close
                        .chars()
                        .enumerate()
                        .all(|(k, ch)| self.at(i + k) == Some(ch))
                    {
                        found = Some((i, close, *shape));
                        break;
                    }
                }
                if found.is_some() || quoted.is_some() {
                    break;
                }
            }
            match found {
                Some((i, close, shape)) if quoted.is_none() || i == search_from => {
                    let text = quoted.unwrap_or_else(|| {
                        self.c[text_start..i]
                            .iter()
                            .collect::<String>()
                            .trim()
                            .to_string()
                    });
                    self.pos = i + close.chars().count();
                    let text = if text.starts_with('`') {
                        format!("\"{text}\"")
                    } else {
                        text
                    };
                    return Ok(Some((shape, text)));
                }
                _ => {
                    if *open == ">" {
                        self.pos = save;
                        return Ok(None);
                    }
                    return Err(format!(
                        "Unterminated node shape: expected '{}'",
                        closers[0].0
                    ));
                }
            }
        }
        Ok(None)
    }

    /// `A@{ shape: diamond, label: "Decide" }` (mermaid 11 expanded syntax).
    fn shape_attrs(&mut self) -> std::result::Result<(Shape, String), String> {
        self.pos += 1;
        let start = self.pos;
        let end = (start..self.c.len())
            .find(|&i| self.c[i] == '}')
            .ok_or("Unterminated '@{'")?;
        let body: String = self.c[start + 1..end].iter().collect();
        self.pos = end + 1;
        let mut shape = Shape::Rect;
        let mut label = String::new();
        for part in body.split(',') {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            let v = v.trim().trim_matches(['"', '\'']);
            match k.trim() {
                "shape" => {
                    shape = match v {
                        "rect" | "proc" | "process" | "rectangle" => Shape::Rect,
                        "rounded" | "event" => Shape::Rounded,
                        "stadium" | "pill" | "terminal" => Shape::Stadium,
                        "subproc" | "subprocess" | "subroutine" | "fr-rect"
                        | "framed-rectangle" => Shape::Subroutine,
                        "cyl" | "cylinder" | "database" | "db" => Shape::Cylinder,
                        "circle" | "circ" => Shape::Circle,
                        "dbl-circ" | "double-circle" => Shape::DoubleCircle,
                        "diam" | "diamond" | "decision" | "question" => Shape::Diamond,
                        "hex" | "hexagon" | "prepare" => Shape::Hexagon,
                        "lean-r" | "lean-right" | "in-out" => Shape::LeanRight,
                        "lean-l" | "lean-left" | "out-in" => Shape::LeanLeft,
                        "trap-b" | "trapezoid" | "priority" | "trapezoid-bottom" => {
                            Shape::Trapezoid
                        }
                        "trap-t" | "inv-trapezoid" | "manual" | "trapezoid-top" => {
                            Shape::InvTrapezoid
                        }
                        "odd" => Shape::Asymmetric,
                        "f-circ" | "filled-circle" | "junction" | "sm-circ" | "small-circle"
                        | "start" => Shape::StateStart,
                        "fr-circ" | "framed-circle" | "stop" => Shape::StateEnd,
                        "fork" | "join" => Shape::ForkBar,
                        "doc" | "document" | "notch-rect" | "card" | "notched-rectangle" => {
                            Shape::Rect
                        }
                        other => return Err(format!("Unknown shape '{other}'")),
                    }
                }
                "label" => label = v.to_string(),
                _ => {}
            }
        }
        Ok((shape, label))
    }

    /// Scan a link token at the cursor.
    fn link(&mut self) -> Option<Link> {
        let save = self.pos;
        if self.starts_with("~~~") {
            while self.peek() == Some('~') {
                self.pos += 1;
            }
            return Some(Link {
                line: LineKind::Invisible,
                start: Head::None,
                end: Head::None,
                minlen: 1,
                label: self.pipe_label(),
            });
        }
        let start_head = match self.peek() {
            Some(c @ ('<' | 'o' | 'x'))
                if matches!(self.at(self.pos + 1), Some('-' | '='))
                    || (c != '<' && self.at(self.pos + 1) == Some('.')) =>
            {
                self.pos += 1;
                head_of(c)
            }
            _ => Head::None,
        };
        if let Some(mut l) = self.full_link(start_head) {
            l.label = self.pipe_label();
            return Some(l);
        }
        // START_LINK: `-- text -->`, `== text ==>`, `-. text .->`
        let kind = if self.starts_with("--") {
            LineKind::Solid
        } else if self.starts_with("==") {
            LineKind::Thick
        } else if self.starts_with("-.") {
            LineKind::Dotted
        } else {
            self.pos = save;
            return None;
        };
        self.pos += 2;
        let text_start = self.pos;
        while !self.done() {
            let here = self.pos;
            let close_start = if matches!(self.peek(), Some('x' | 'o' | '<'))
                && self
                    .at(here + 1)
                    .is_some_and(|c| c == '-' || c == '=' || c == '.')
            {
                here + 1
            } else {
                here
            };
            self.pos = close_start;
            let closing = match kind {
                LineKind::Dotted => self.dotted_close(),
                _ => self.full_link(Head::None).filter(|l| l.line == kind),
            };
            if let Some(mut l) = closing {
                let text: String = self.c[text_start..here].iter().collect();
                l.start = start_head;
                l.label = Some(text.trim().to_string()).filter(|t| !t.is_empty());
                if let Some(p) = self.pipe_label() {
                    l.label = Some(p);
                }
                return Some(l);
            }
            self.pos = here + 1;
        }
        self.pos = save;
        None
    }

    /// `--+[-xo>]`, `==+[=xo>]` or `-?.+-[xo>]?` at the cursor.
    fn full_link(&mut self, start: Head) -> Option<Link> {
        let save = self.pos;
        for (ch, kind) in [('-', LineKind::Solid), ('=', LineKind::Thick)] {
            let mut k = 0;
            while self.at(self.pos + k) == Some(ch) {
                k += 1;
            }
            if k >= 2 {
                if let Some(end) = self
                    .at(self.pos + k)
                    .filter(|c| matches!(c, 'x' | 'o' | '>'))
                {
                    // `--x`/`--o` followed by an identifier character is still a head
                    // in mermaid's lexer; mirror that.
                    self.pos += k + 1;
                    return Some(Link {
                        line: kind,
                        start,
                        end: head_of(end),
                        minlen: k - 1,
                        label: None,
                    });
                }
                if k >= 3 {
                    self.pos += k;
                    return Some(Link {
                        line: kind,
                        start,
                        end: Head::None,
                        minlen: k - 2,
                        label: None,
                    });
                }
            }
        }
        // dotted
        let mut p = self.pos;
        if self.at(p) == Some('-') {
            p += 1;
        }
        let dots_start = p;
        while self.at(p) == Some('.') {
            p += 1;
        }
        let dots = p - dots_start;
        if dots >= 1 && self.at(p) == Some('-') {
            p += 1;
            let end = match self.at(p) {
                Some(c @ ('x' | 'o' | '>')) => {
                    p += 1;
                    head_of(c)
                }
                _ => Head::None,
            };
            self.pos = p;
            return Some(Link {
                line: LineKind::Dotted,
                start,
                end,
                minlen: dots,
                label: None,
            });
        }
        self.pos = save;
        None
    }

    fn dotted_close(&mut self) -> Option<Link> {
        let save = self.pos;
        let mut p = self.pos;
        let mut dots = 0;
        while self.at(p) == Some('.') {
            p += 1;
            dots += 1;
        }
        if dots >= 1 && self.at(p) == Some('-') {
            p += 1;
            let end = match self.at(p) {
                Some(c @ ('x' | 'o' | '>')) => {
                    p += 1;
                    head_of(c)
                }
                _ => Head::None,
            };
            self.pos = p;
            return Some(Link {
                line: LineKind::Dotted,
                start: Head::None,
                end,
                minlen: dots,
                label: None,
            });
        }
        self.pos = save;
        None
    }

    fn pipe_label(&mut self) -> Option<String> {
        let save = self.pos;
        self.ws();
        if self.peek() != Some('|') {
            self.pos = save;
            return None;
        }
        let start = self.pos + 1;
        let end = (start..self.c.len()).find(|&i| self.c[i] == '|')?;
        self.pos = end + 1;
        Some(
            self.c[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> GraphDiagram {
        parse(src, None).unwrap()
    }

    #[test]
    fn basic_chain_and_shapes() {
        let g = p("graph LR\nA[Start] --> B{Is it?}\nB -->|Yes| C((Done))\nB -- No --> D[(DB)]");
        assert_eq!(g.direction, Direction::LeftRight);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.nodes[1].shape, Shape::Diamond);
        assert_eq!(g.nodes[2].shape, Shape::Circle);
        assert_eq!(g.nodes[3].shape, Shape::Cylinder);
        assert_eq!(g.edges[1].label.as_deref(), Some("Yes"));
        assert_eq!(g.edges[2].label.as_deref(), Some("No"));
    }

    #[test]
    fn link_varieties() {
        let g = p(
            "flowchart TD\nA --- B\nA -.-> C\nA ==> D\nA --o E\nA --x F\nA <--> G\nA ~~~ H\nA ----> I\nA -. dotted .-> J\nA == thick ==> K",
        );
        let e = &g.edges;
        assert_eq!((e[0].line, e[0].end), (LineKind::Solid, Head::None));
        assert_eq!((e[1].line, e[1].end), (LineKind::Dotted, Head::Arrow));
        assert_eq!((e[2].line, e[2].end), (LineKind::Thick, Head::Arrow));
        assert_eq!(e[3].end, Head::Circle);
        assert_eq!(e[4].end, Head::Cross);
        assert_eq!((e[5].start, e[5].end), (Head::Arrow, Head::Arrow));
        assert_eq!(e[6].line, LineKind::Invisible);
        assert_eq!(e[7].minlen, 3);
        assert_eq!(
            (e[8].line, e[8].label.as_deref()),
            (LineKind::Dotted, Some("dotted"))
        );
        assert_eq!(
            (e[9].line, e[9].label.as_deref()),
            (LineKind::Thick, Some("thick"))
        );
    }

    #[test]
    fn ampersand_groups_and_chains() {
        let g = p("graph TD\nA & B --> C & D --> E");
        assert_eq!(g.edges.len(), 6);
    }

    #[test]
    fn subgraphs_and_styles() {
        let g = p(
            "graph TB\nsubgraph one [First]\na1-->a2\nend\nsubgraph two\nb1-->b2\nend\none --> two\nclassDef hot fill:#f00\nclass a1 hot\nb1:::hot\nstyle a2 stroke:#0f0\nlinkStyle 0 stroke:#00f",
        );
        assert_eq!(g.clusters.len(), 2);
        assert_eq!(g.clusters[0].title, "First");
        assert_eq!(g.nodes.len(), 4, "subgraph ids are not drawn as nodes");
        assert!(matches!(g.edges[2].from, Endpoint::Cluster(_)));
        let a1 = g.nodes.iter().find(|n| n.id == "a1").unwrap();
        assert_eq!(a1.style.fill.as_ref().unwrap().css(), "#f00");
        let b1 = g.nodes.iter().find(|n| n.id == "b1").unwrap();
        assert_eq!(b1.style.fill.as_ref().unwrap().css(), "#f00");
        assert_eq!(g.edges[0].style.stroke.as_ref().unwrap().css(), "#00f");
    }

    #[test]
    fn quoted_and_special_labels() {
        let g = p(
            "graph TD\nA[\"Hello [world] (x)\"] --> B>flag]\nC[/lean/] --> D[\\alt\\]\nE[/trap\\] --> F{{hex}}",
        );
        assert_eq!(g.nodes[0].label, "Hello [world] (x)");
        assert_eq!(g.nodes[1].shape, Shape::Asymmetric);
        assert_eq!(g.nodes[2].shape, Shape::LeanRight);
        assert_eq!(g.nodes[3].shape, Shape::LeanLeft);
        assert_eq!(g.nodes[4].shape, Shape::Trapezoid);
        assert_eq!(g.nodes[5].shape, Shape::Hexagon);
    }

    #[test]
    fn errors_have_line_numbers() {
        let e = parse("graph TD\nA --> B\nA -->", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("graph TD\nsubgraph x\nA", None).unwrap_err();
        assert!(matches!(e, Error::Parse { .. }));
        let e = parse("graph TD\nA-->B\nlinkStyle 3 stroke:red", None).unwrap_err();
        assert!(e.to_string().contains("out of bounds"));
    }

    #[test]
    fn dashed_ids_and_semicolons() {
        let g = p("graph TD;my-node-->other_node;");
        assert_eq!(g.nodes[0].id, "my-node");
        assert_eq!(g.nodes[1].id, "other_node");
    }

    #[test]
    fn expanded_shape_syntax() {
        let g = p("flowchart TD\nA@{ shape: diam, label: \"Decide\" } --> B");
        assert_eq!(g.nodes[0].shape, Shape::Diamond);
        assert_eq!(g.nodes[0].label, "Decide");
    }
}
