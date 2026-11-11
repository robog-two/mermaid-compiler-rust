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
use super::statements_multiline;
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::DiagramKind;
use crate::theme::ColorSlot;
use std::collections::HashMap;

/// Dash pattern of an animated edge (flowchart `styles.ts`: `.edge-animation-fast`,
/// `.edge-animation-slow { stroke-dasharray: 9,5 !important }`).
pub(crate) const ANIMATED_DASH: [f64; 2] = [9.0, 5.0];

/// Config that changes how a flowchart is parsed (flowDb reads it while parsing).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// `flowchart.inheritDir`: a subgraph without its own `direction` takes the
    /// diagram's, instead of having none.
    pub inherit_dir: bool,
}

/// Parse a preprocessed flowchart body.
pub fn parse(body: &str, title: Option<String>) -> Result<GraphDiagram> {
    parse_with(body, title, ParseOptions::default())
}

/// [`parse`] with explicit [`ParseOptions`].
pub fn parse_with(body: &str, title: Option<String>, opts: ParseOptions) -> Result<GraphDiagram> {
    let mut b = Builder {
        inherit_dir: opts.inherit_dir,
        ..Builder::default()
    };
    let mut header_seen = false;
    let mut direction = Direction::TopBottom;
    // flow.jison's string states match newlines: quoted labels may span lines.
    for (line, stmt) in statements_multiline(body, |_| true) {
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
                // flowDb's `getDirection()` is unset without a header direction.
                b.root_dir = Some(direction);
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
    /// Subgraphs on the stack whose id is generated when they close (`subGraphN`).
    unnamed: Vec<ClusterId>,
    /// flowDb's `subCount`: bumped by every subgraph as it closes.
    sub_count: usize,
    /// Per cluster: closed at least once (only then is its id in flowDb's
    /// `subGraphLookup`, so `id@{ … }` addresses the subgraph rather than a vertex).
    closed: Vec<bool>,
    /// Per cluster: `@{ view: collapsed }` (the last `view` given wins).
    collapsed: Vec<bool>,
    /// The diagram direction from the header (`inheritDir` hands it to subgraphs).
    root_dir: Option<Direction>,
    inherit_dir: bool,
    /// User edge ids (`A e1@--> B`).
    edge_ids: HashMap<String, Vec<usize>>,
    /// Edges with `animate: true` / `animation: …` metadata (indices into `edges`).
    animated_edges: Vec<usize>,
    /// `classDef`s that declare an `animation`.
    animated_classes: std::collections::HashSet<String>,
    edges: Vec<Edge>,
    class_defs: HashMap<String, StyleOverrides>,
    node_styles: Vec<(String, StyleOverrides)>,
    class_assignments: Vec<(String, String)>,
    link_styles: Vec<(usize, Option<Vec<usize>>, StyleOverrides)>,
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
            "end" if rest.is_empty() => {
                let c = self.stack.pop().ok_or_else(|| {
                    err_at(
                        line,
                        stmt,
                        0,
                        "Unexpected 'end' without matching 'subgraph'",
                    )
                })?;
                self.close_subgraph(c);
                Ok(())
            }
            "direction" => {
                // Only a subgraph's own `direction` matters (the last one wins). A
                // repeated id keeps the direction of its first declaration.
                if let (Some(&c), Some(d)) = (self.stack.last(), Direction::parse(rest)) {
                    if !self.closed[c.0] {
                        self.clusters[c.0].direction = Some(d);
                    }
                }
                Ok(())
            }
            "classDef" if rest.is_empty() => Err(err_at(
                line,
                stmt,
                stmt.len(),
                "Expecting a class name after 'classDef'",
            )),
            "classDef" => {
                let (names, style) = split_first_word(rest);
                let st = StyleOverrides::parse(style);
                let animated = style.split([',', ';']).any(|kv| {
                    kv.split(':')
                        .next()
                        .is_some_and(|k| k.trim() == "animation")
                });
                for n in names.split(',') {
                    let e = self.class_defs.entry(n.trim().to_string()).or_default();
                    *e = e.merged(&st);
                    if animated {
                        self.animated_classes.insert(n.trim().to_string());
                    }
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
        let (id, title) = subgraph_header(rest);
        let parent = self.stack.last().copied();
        let existing = id
            .as_ref()
            .and_then(|id| self.cluster_index.get(id).copied());
        let ci = match existing {
            // A repeated id adds to the first declaration, which keeps its title.
            Some(ci) => ci,
            None => {
                let mut c = Cluster::new(id.clone().unwrap_or_default(), title, parent);
                // Subgraphs take palette slots in declaration (pre-order) order,
                // as flowDb's `declarationIndex` assigns `colorIndex`.
                c.color_slot = Some(ColorSlot(self.clusters.len()));
                self.clusters.push(c);
                self.closed.push(false);
                self.collapsed.push(false);
                let ci = self.clusters.len() - 1;
                match id {
                    Some(id) => {
                        self.cluster_index.insert(id, ci);
                    }
                    None => self.unnamed.push(ClusterId(ci)),
                }
                ci
            }
        };
        self.stack.push(ClusterId(ci));
        Ok(())
    }

    /// `end`: flowDb's `addSubGraph`, which runs as the subgraph closes, so a generated
    /// id numbers the subgraphs in closing order (inner ones first).
    fn close_subgraph(&mut self, c: ClusterId) {
        if let Some(pos) = self.unnamed.iter().position(|&u| u == c) {
            self.unnamed.remove(pos);
            let id = format!("subGraph{}", self.sub_count);
            self.clusters[c.0].id = id.clone();
            self.cluster_index.entry(id).or_insert(c.0);
        }
        if !self.closed[c.0] {
            self.closed[c.0] = true;
            if self.inherit_dir && self.clusters[c.0].direction.is_none() {
                self.clusters[c.0].direction = self.root_dir;
            }
        }
        self.sub_count += 1;
    }

    /// Register / update a node and place it in the current subgraph.
    fn touch(
        &mut self,
        id: &str,
        shape: Option<(Shape, String)>,
        data: Option<ShapeData>,
        class: Option<String>,
    ) -> usize {
        // flowDb's `addVertex`: metadata on a declared subgraph id is the subgraph's
        // (`sub@{ view: collapsed }`) and does not make it a vertex.
        let mut shape = shape;
        if let Some(d) = data {
            match self.cluster_index.get(id) {
                Some(&c) if self.closed[c] => {
                    if let Some(v) = d.view {
                        self.collapsed[c] = v == "collapsed";
                    }
                }
                _ => {
                    let label = d.label.unwrap_or_else(|| {
                        self.index
                            .get(id)
                            .map_or_else(|| id.to_string(), |&i| self.nodes[i].label.clone())
                    });
                    let s = d.shape.unwrap_or_else(|| {
                        self.index
                            .get(id)
                            .map_or(Shape::Rect, |&i| self.nodes[i].shape)
                    });
                    shape = Some((s, label));
                }
            }
        }
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
            // A closed subgraph named inside another one becomes its child, unless it
            // already belongs to a subgraph (flowDb's `makeUniq`).
            if let Some(&sc) = self.cluster_index.get(id) {
                let open = self.stack.contains(&ClusterId(sc));
                if !open
                    && self.clusters[sc].parent.is_none()
                    && !self.is_ancestor(ClusterId(sc), cur)
                {
                    self.clusters[sc].parent = Some(cur);
                }
            }
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
        // `e1@{ animate: true }` on a known edge id is edge metadata (flowDb's
        // `addVertex` routes it to the edge); `animate` / `animation` are drawn, a
        // per-edge `curve` is not.
        if let Some((id, body)) = stmt.split_once("@{") {
            if let Some(ix) = self.edge_ids.get(id.trim()).cloned() {
                let body = body.trim_end().trim_end_matches('}');
                let animated = body.split(',').any(|kv| {
                    kv.split_once(':').is_some_and(|(k, v)| {
                        let v = v.trim().trim_matches(['"', '\'']);
                        match k.trim() {
                            "animate" => v == "true",
                            "animation" => !v.is_empty(),
                            _ => false,
                        }
                    })
                });
                if animated {
                    self.animated_edges.extend(ix);
                }
                return Ok(());
            }
        }
        let mut sc = Scanner::new(stmt);
        let mut prev = self.group(&mut sc, line, stmt)?;
        loop {
            sc.ws();
            if sc.done() {
                break;
            }
            let edge_id = sc.link_id();
            if let Some(id) = &edge_id {
                self.edge_ids.entry(id.clone()).or_default();
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
                    if let Some(id) = &edge_id {
                        self.edge_ids
                            .entry(id.clone())
                            .or_default()
                            .push(self.edges.len());
                    }
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
            let idx = self.touch(&r.id, r.shape, r.data, r.class);
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
        // Animated edges (`animate` / `animation` metadata, or a class with an
        // `animation`) take `.edge-animation-*`'s `stroke-dasharray: 9,5 !important`.
        for (id, class) in &self.class_assignments {
            if self.animated_classes.contains(class) {
                if let Some(ix) = self.edge_ids.get(id) {
                    self.animated_edges.extend(ix.iter().copied());
                }
            }
        }
        for &i in &self.animated_edges {
            if let Some(e) = self.edges.get_mut(i) {
                e.style.dash = Some(ANIMATED_DASH.to_vec());
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
        let g = GraphDiagram {
            kind: DiagramKind::Flowchart,
            direction,
            nodes,
            edges,
            clusters: self.clusters,
            title,
        };
        Ok(collapse(g, &self.collapsed))
    }
}

/// flowDb's `getData` for `@{ view: collapsed }` subgraphs: the outermost collapsed
/// subgraph of a chain becomes one `collapsedGroup` node (its title, style and palette
/// slot), everything inside it disappears, and edges that cross its border are
/// redirected to it. An edge that turns into a self-loop only because of that is
/// dropped; an authored self-loop on the collapsed subgraph is kept.
fn collapse(g: GraphDiagram, collapsed: &[bool]) -> GraphDiagram {
    if !collapsed.iter().any(|&c| c) {
        return g;
    }
    let n_clusters = g.clusters.len();
    // Outermost collapsed ancestor (or self) of every cluster.
    let folded_into: Vec<Option<usize>> = (0..n_clusters)
        .map(|c| {
            let mut out = None;
            let mut cur = Some(c);
            for _ in 0..=n_clusters {
                let Some(k) = cur else { break };
                if collapsed.get(k).copied().unwrap_or(false) {
                    out = Some(k);
                }
                cur = g.clusters[k].parent.map(|p| p.0);
            }
            out
        })
        .collect();
    // Surviving clusters, renumbered.
    let mut cluster_map: Vec<Option<usize>> = vec![None; n_clusters];
    let mut clusters = Vec::new();
    for (ci, c) in g.clusters.iter().enumerate() {
        if folded_into[ci].is_none() {
            cluster_map[ci] = Some(clusters.len());
            clusters.push(c.clone());
        }
    }
    for c in &mut clusters {
        c.parent = c.parent.and_then(|p| cluster_map[p.0]).map(ClusterId);
    }
    // getData pushes subgraph nodes (last declared first) before the vertices.
    let mut nodes = Vec::new();
    let mut collapsed_node: Vec<Option<usize>> = vec![None; n_clusters];
    for ci in (0..n_clusters).rev() {
        if folded_into[ci] != Some(ci) {
            continue;
        }
        let c = &g.clusters[ci];
        let mut n = Node::new(c.id.clone(), c.title.clone(), Shape::CollapsedGroup);
        n.cluster = c.parent.and_then(|p| cluster_map[p.0]).map(ClusterId);
        n.style = c.style.clone();
        n.color_slot = c.color_slot;
        collapsed_node[ci] = Some(nodes.len());
        nodes.push(n);
    }
    let mut node_map: Vec<Option<usize>> = vec![None; g.nodes.len()];
    // Hidden nodes, by the collapsed node standing in for them.
    let mut hidden_in: Vec<Option<usize>> = vec![None; g.nodes.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        match n.cluster.and_then(|c| folded_into[c.0]) {
            Some(a) => hidden_in[i] = collapsed_node[a],
            None => {
                let mut n = n.clone();
                n.cluster = n.cluster.and_then(|c| cluster_map[c.0]).map(ClusterId);
                node_map[i] = Some(nodes.len());
                nodes.push(n);
            }
        }
    }
    // (new endpoint, whether the original end was hidden by a collapse)
    let end = |e: Endpoint| -> Option<(Endpoint, bool)> {
        match e {
            Endpoint::Node(NodeId(i)) => match node_map[i] {
                Some(j) => Some((Endpoint::Node(NodeId(j)), false)),
                None => hidden_in[i].map(|j| (Endpoint::Node(NodeId(j)), true)),
            },
            Endpoint::Cluster(ClusterId(c)) => match folded_into[c] {
                None => cluster_map[c].map(|k| (Endpoint::Cluster(ClusterId(k)), false)),
                Some(a) => collapsed_node[a].map(|j| (Endpoint::Node(NodeId(j)), a != c)),
            },
        }
    };
    let edges = g
        .edges
        .into_iter()
        .filter_map(|mut e| {
            let (from, hid_a) = end(e.from)?;
            let (to, hid_b) = end(e.to)?;
            if from == to && (hid_a || hid_b) {
                return None;
            }
            e.from = from;
            e.to = to;
            Some(e)
        })
        .collect();
    GraphDiagram {
        nodes,
        edges,
        clusters,
        ..g
    }
}

/// `subgraph` header → (explicit id, title), after flow.jison's three forms:
/// `subgraph id [title]` / `subgraph id["title"]`, `subgraph text` (a title with
/// whitespace gets a generated id, otherwise the text is both) and a bare `subgraph`.
fn subgraph_header(rest: &str) -> (Option<String>, String) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, String::new());
    }
    let unquote = |s: &str| -> String {
        let s = s.trim();
        let inner = s
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .unwrap_or(s);
        // A markdown string keeps its quotes so the label renders it as markdown.
        if inner.starts_with('`') && inner.ends_with('`') && inner.len() > 1 {
            format!("\"{inner}\"")
        } else {
            inner.to_string()
        }
    };
    let quoted = rest.starts_with('"');
    let bracket = if quoted {
        // `"id"[title]` is unusual; a quoted head is a title unless brackets follow it.
        rest[1..]
            .find('"')
            .map(|q| q + 2)
            .filter(|&e| rest[e..].trim_start().starts_with('['))
    } else {
        rest.find('[')
    };
    if let Some(open) = bracket {
        let open = open + rest[open..].find('[').unwrap_or(0);
        if rest.ends_with(']') {
            let id = unquote(&rest[..open]);
            let title = unquote(&rest[open + 1..rest.len() - 1]);
            return (Some(id).filter(|i| !i.is_empty()), title);
        }
    }
    let title = unquote(rest);
    let plain = title.trim_matches('"');
    if plain.contains(char::is_whitespace) {
        (None, title)
    } else {
        (Some(plain.to_string()), title)
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
    /// The `@{ … }` metadata block, if the reference carries one.
    data: Option<ShapeData>,
    class: Option<String>,
}

/// The keys of an `@{ … }` block this renderer reads.
#[derive(Debug, Clone, Default, PartialEq)]
struct ShapeData {
    shape: Option<Shape>,
    label: Option<String>,
    /// `view: collapsed` on a declared subgraph id folds that subgraph.
    view: Option<String>,
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

    /// flow.jison's `LINK_ID`: `id@` right before a link (not `@{` / `@"`). Consumed
    /// and returned when present.
    fn link_id(&mut self) -> Option<String> {
        let start = self.pos;
        let mut p = start;
        while let Some(ch) = self.at(p) {
            if ch.is_whitespace() || "\"[](){}|@".contains(ch) {
                break;
            }
            p += 1;
        }
        let ok = p > start
            && self.at(p) == Some('@')
            && !matches!(self.at(p + 1), Some('{' | '"') | None)
            && (self.is_link_start(p + 1)
                || (matches!(self.at(p + 1), Some('<' | 'x' | 'o')) && self.is_link_start(p + 2)));
        if !ok {
            return None;
        }
        self.pos = p + 1;
        Some(self.c[start..p].iter().collect())
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
        let (shape, data) = if self.starts_with("@{") {
            (None, Some(self.shape_attrs()?))
        } else {
            (self.shape()?, None)
        };
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
        Ok(Some(NodeRef {
            id,
            shape,
            data,
            class,
        }))
    }

    fn shape(&mut self) -> std::result::Result<Option<(Shape, String)>, String> {
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
    fn shape_attrs(&mut self) -> std::result::Result<ShapeData, String> {
        self.pos += 1;
        let start = self.pos;
        let end = (start..self.c.len())
            .find(|&i| self.c[i] == '}')
            .ok_or("Unterminated '@{'")?;
        let body: String = self.c[start + 1..end].iter().collect();
        self.pos = end + 1;
        let mut data = ShapeData::default();
        for part in body.split(',') {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            let v = v.trim().trim_matches(['"', '\'']);
            match k.trim() {
                "shape" => {
                    data.shape =
                        Some(shape_by_name(v).ok_or_else(|| format!("No such shape: {v}."))?);
                }
                "label" => data.label = Some(v.to_string()),
                "view" => data.view = Some(v.to_string()),
                _ => {}
            }
        }
        Ok(data)
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

/// `@{ shape: … }` names → shapes, following the alias table in mermaid's
/// `rendering-util/rendering-elements/shapes.ts`. Valid names we do not draw
/// yet fall back to a rectangle; unknown names are an error, as upstream.
pub(crate) fn shape_by_name(name: &str) -> Option<Shape> {
    Some(match name {
        "rect" | "proc" | "process" | "rectangle" | "squareRect" => Shape::Rect,
        "rounded" | "event" | "roundedRect" => Shape::Rounded,
        "stadium" | "pill" | "terminal" => Shape::Stadium,
        "fr-rect" | "subproc" | "subprocess" | "subroutine" | "framed-rectangle" => {
            Shape::Subroutine
        }
        "cyl" | "cylinder" | "database" | "db" => Shape::Cylinder,
        "circle" | "circ" => Shape::Circle,
        "dbl-circ" | "double-circle" | "doublecircle" => Shape::DoubleCircle,
        "diam" | "diamond" | "decision" | "question" => Shape::Diamond,
        "hex" | "hexagon" | "prepare" => Shape::Hexagon,
        "lean-r" | "lean-right" | "in-out" | "lean_right" => Shape::LeanRight,
        "lean-l" | "lean-left" | "out-in" | "lean_left" => Shape::LeanLeft,
        "trap-b" | "trapezoid" | "priority" | "trapezoid-bottom" => Shape::Trapezoid,
        "trap-t" | "inv-trapezoid" | "manual" | "trapezoid-top" | "inv_trapezoid" => {
            Shape::InvTrapezoid
        }
        "odd" | "rect_left_inv_arrow" => Shape::Asymmetric,
        "f-circ" | "filled-circle" | "junction" => Shape::StateStart,
        "sm-circ" | "small-circle" | "start" | "stateStart" => Shape::SmallCircle,
        "fr-circ" | "framed-circle" | "stop" | "stateEnd" => Shape::StateEnd,
        "fork" | "join" | "forkJoin" => Shape::ForkBar,
        "text" => Shape::Text,
        "notch-rect" | "card" | "notched-rectangle" => Shape::Card,
        "lin-rect" | "lined-rectangle" | "lined-process" | "lin-proc" | "shaded-process" => {
            Shape::LinedRect
        }
        "delay" | "half-rounded-rectangle" => Shape::Delay,
        "tri" | "extract" | "triangle" => Shape::Triangle,
        "flip-tri" | "manual-file" | "flipped-triangle" => Shape::FlippedTriangle,
        "hourglass" | "collate" => Shape::Hourglass,
        "cross-circ" | "summary" | "crossed-circle" => Shape::CrossedCircle,
        "sl-rect" | "manual-input" | "sloped-rectangle" => Shape::SlopedRect,
        "win-pane" | "internal-storage" | "window-pane" => Shape::WindowPane,
        "div-rect" | "div-proc" | "divided-rectangle" | "divided-process" => Shape::DividedRect,
        "notch-pent" | "loop-limit" | "notched-pentagon" => Shape::NotchedPentagon,
        "st-rect" | "procs" | "processes" | "stacked-rectangle" => Shape::StackedRect,
        "tag-rect" | "tagged-rectangle" | "tag-proc" | "tagged-process" => Shape::TaggedRect,
        "doc" | "document" => Shape::Document,
        "lin-cyl" | "disk" | "lined-cylinder" => Shape::LinedCylinder,
        "h-cyl" | "das" | "horizontal-cylinder" => Shape::HorizontalCylinder,
        "curv-trap" | "curved-trapezoid" | "display" => Shape::CurvedTrapezoid,
        "datastore" | "data-store" => Shape::Datastore,
        "folder" | "directory" => Shape::Folder,
        "bucket" => Shape::Bucket,
        "console" => Shape::Console,
        "browser" => Shape::Browser,
        "person" => Shape::Person,
        "bang" => Shape::Bang,
        "cloud" => Shape::Cloud,
        "brace" | "comment" | "brace-l" => Shape::BraceLeft,
        "brace-r" => Shape::BraceRight,
        "braces" => Shape::Braces,
        "bolt" | "com-link" | "lightning-bolt" => Shape::Bolt,
        "docs" | "documents" | "st-doc" | "stacked-document" => Shape::StackedDocument,
        "bow-rect" | "stored-data" | "bow-tie-rectangle" => Shape::BowTieRect,
        "tag-doc" | "tagged-document" => Shape::TaggedDocument,
        "flag" | "paper-tape" => Shape::PaperTape,
        "lin-doc" | "lined-document" => Shape::LinedDocument,
        "state" => Shape::State,
        "choice" => Shape::Choice,
        "note" => Shape::Note,
        "collapsedGroup" => Shape::CollapsedGroup,
        // Valid upstream shapes (other diagrams' internals) drawn as rectangles.
        "composite"
        | "rectWithTitle"
        | "labelRect"
        | "block_arrow"
        | "iconSquare"
        | "iconCircle"
        | "icon"
        | "iconRounded"
        | "imageSquare"
        | "anchor"
        | "kanbanItem"
        | "mindmapCircle"
        | "defaultMindmapNode"
        | "classBox"
        | "erBox"
        | "requirementBox"
        | "usecaseActor"
        | "usecaseActorHollow"
        | "usecaseActorAwesome"
        | "usecaseActorIcon"
        | "usecaseBusiness"
        | "usecaseEllipse"
        | "usecaseJsonTable"
        | "bpmn-start"
        | "bpmn-intermediate"
        | "bpmn-boundary"
        | "bpmn-end"
        | "bpmn-gateway"
        | "bpmn-activity"
        | "bpmn-data"
        | "bpmn-data-store"
        | "bpmn-annotation" => Shape::Rect,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> GraphDiagram {
        parse(src, None).unwrap()
    }

    #[test]
    fn every_upstream_shape_name_is_accepted() {
        // shapes.ts: every shortName and alias of the documented shapes.
        let drawn = [
            ("datastore", Shape::Datastore),
            ("data-store", Shape::Datastore),
            ("directory", Shape::Folder),
            ("bucket", Shape::Bucket),
            ("console", Shape::Console),
            ("browser", Shape::Browser),
            ("person", Shape::Person),
            ("bang", Shape::Bang),
            ("cloud", Shape::Cloud),
            ("comment", Shape::BraceLeft),
            ("brace-r", Shape::BraceRight),
            ("braces", Shape::Braces),
            ("lightning-bolt", Shape::Bolt),
            ("st-doc", Shape::StackedDocument),
            ("stored-data", Shape::BowTieRect),
            ("tagged-document", Shape::TaggedDocument),
            ("paper-tape", Shape::PaperTape),
            ("lin-doc", Shape::LinedDocument),
            ("lined-document", Shape::LinedDocument),
        ];
        for (name, shape) in drawn {
            assert_eq!(shape_by_name(name), Some(shape), "{name}");
        }
        assert_eq!(shape_by_name("bpmn-gateway"), Some(Shape::Rect));
        assert_eq!(shape_by_name("bogus"), None);
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

    fn cluster<'a>(g: &'a GraphDiagram, id: &str) -> &'a Cluster {
        g.clusters
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no cluster {id}: {:?}", g.clusters))
    }

    #[test]
    fn subgraph_ids_and_titles() {
        // flowDb: a title with whitespace gets `subGraph<subCount>`, counted by every
        // subgraph as it closes; a single word is both id and title.
        let g = p(concat!(
            "flowchart TB
",
            "subgraph one [First title]
a1
end
",
            "subgraph \"Quoted title\"
b1
end
",
            "subgraph three[\"`**md**`\"]
c1
end
",
            "subgraph Title with spaces
d1
end
",
            "subgraph \"Word\"
e1
end
",
            "subgraph\nf1\nend\n",
        ));
        assert_eq!(cluster(&g, "one").title, "First title");
        assert_eq!(cluster(&g, "subGraph1").title, "Quoted title");
        assert_eq!(cluster(&g, "three").title, "\"`**md**`\"");
        assert_eq!(cluster(&g, "subGraph3").title, "Title with spaces");
        assert_eq!(cluster(&g, "Word").title, "Word");
        assert_eq!(cluster(&g, "subGraph5").title, "");
    }

    #[test]
    fn nested_subgraph_ids_number_in_closing_order() {
        let g = p("flowchart TB
subgraph Outer box
subgraph Inner box
a
end
end");
        let inner = cluster(&g, "subGraph0");
        let outer = cluster(&g, "subGraph1");
        assert_eq!(inner.title, "Inner box");
        assert_eq!(outer.title, "Outer box");
        assert_eq!(g.clusters[inner.parent.unwrap().0].id, "subGraph1");
    }

    #[test]
    fn subgraph_membership_follows_flowdb() {
        // The first subgraph to close keeps a node listed by a later one.
        let g = p("flowchart TB
a --> b
subgraph S1
b
end
subgraph S2
c
b
end");
        let of = |id: &str| {
            let n = g.nodes.iter().find(|n| n.id == id).unwrap();
            n.cluster.map(|c| g.clusters[c.0].id.clone())
        };
        assert_eq!(of("a"), None);
        assert_eq!(of("b").as_deref(), Some("S1"));
        assert_eq!(of("c").as_deref(), Some("S2"));
        // An inner subgraph wins over its parent, whichever mentions the node first.
        let g = p("flowchart TB
subgraph O
x
subgraph I
x
end
end");
        let x = g.nodes.iter().find(|n| n.id == "x").unwrap();
        assert_eq!(g.clusters[x.cluster.unwrap().0].id, "I");
    }

    #[test]
    fn closed_subgraph_named_inside_another_is_nested() {
        let g = p("flowchart TB
subgraph A
a
end
subgraph B
A --> b
end");
        assert_eq!(
            cluster(&g, "A")
                .parent
                .map(|c| g.clusters[c.0].id.clone())
                .as_deref(),
            Some("B")
        );
        assert!(matches!(g.edges[0].from, Endpoint::Cluster(_)));
        // Mentioned at the top level, it stays a sibling.
        let g = p("flowchart TB
subgraph A
a
end
subgraph B
b
end
A --> B");
        assert!(cluster(&g, "A").parent.is_none());
    }

    #[test]
    fn subgraph_direction_and_repeated_ids() {
        let g = p("flowchart LR
subgraph S
direction TB
a
end
subgraph T
b
end
subgraph S
c
end");
        assert_eq!(cluster(&g, "S").direction, Some(Direction::TopBottom));
        assert_eq!(cluster(&g, "T").direction, None);
        assert_eq!(
            g.clusters.len(),
            2,
            "a repeated id merges into the first subgraph"
        );
        let c = g.nodes.iter().find(|n| n.id == "c").unwrap();
        assert_eq!(g.clusters[c.cluster.unwrap().0].id, "S");
    }

    fn node<'a>(g: &'a GraphDiagram, id: &str) -> Option<&'a Node> {
        g.nodes.iter().find(|n| n.id == id)
    }

    fn edge_ids(g: &GraphDiagram) -> Vec<(String, String)> {
        let name = |e: Endpoint| match e {
            Endpoint::Node(NodeId(i)) => g.nodes[i].id.clone(),
            Endpoint::Cluster(ClusterId(c)) => format!("[{}]", g.clusters[c].id),
        };
        g.edges.iter().map(|e| (name(e.from), name(e.to))).collect()
    }

    #[test]
    fn collapsed_subgraph_folds_to_one_node() {
        // subgraph.spec.js: `one@{ view: collapsed }`.
        let g = p("graph TD
subgraph one [My Group]
A --> B
end
C --> A
one@{ view: collapsed }");
        assert!(g.clusters.is_empty());
        let one = node(&g, "one").expect("collapsed node");
        assert_eq!(one.shape, Shape::CollapsedGroup);
        assert_eq!(one.label, "My Group");
        assert_eq!(one.color_slot, Some(ColorSlot(0)), "keeps its palette slot");
        assert!(node(&g, "A").is_none() && node(&g, "B").is_none());
        // C --> A is redirected, A --> B (internal) is dropped.
        assert_eq!(edge_ids(&g), [("C".to_string(), "one".to_string())]);
    }

    #[test]
    fn nested_collapse_redirects_to_the_outermost() {
        let g = p("flowchart LR
subgraph outer
subgraph inner
A
end
B
end
C --> A
inner --> C
outer --> outer
D --> inner
outer@{ view: collapsed }
inner@{ view: \"collapsed\" }
classDef hot fill:#f00
class outer hot");
        assert!(g.clusters.is_empty());
        assert!(node(&g, "inner").is_none());
        let outer = node(&g, "outer").unwrap();
        assert_eq!(outer.shape, Shape::CollapsedGroup);
        assert_eq!(
            outer.style.fill.as_ref().map(|c| c.0.as_str()),
            Some("#f00")
        );
        let e: Vec<(String, String)> = edge_ids(&g);
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            e,
            [
                s("C", "outer"),
                s("outer", "C"),
                s("outer", "outer"),
                s("D", "outer")
            ],
            "an authored self-loop stays"
        );
    }

    #[test]
    fn collapsed_child_stays_inside_its_expanded_parent() {
        let g = p("flowchart TB
subgraph P
subgraph K
x --> y
end
z --> K
end
K@{ view: collapsed }");
        assert_eq!(g.clusters.len(), 1);
        let k = node(&g, "K").unwrap();
        assert_eq!(k.cluster, Some(ClusterId(0)));
        assert_eq!(node(&g, "z").unwrap().cluster, Some(ClusterId(0)));
        assert_eq!(edge_ids(&g), [("z".to_string(), "K".to_string())]);
    }

    #[test]
    fn collapse_applies_to_a_repeated_subgraph() {
        // flowDb.spec.ts: whichever declaration the metadata follows.
        for src in [
            "flowchart LR\nsubgraph S\n  x\nend\nsubgraph S\n  y\nend\nS@{ view: collapsed }",
            "flowchart LR\nsubgraph S\n  x\nend\nS@{ view: collapsed }\nsubgraph S\n  y\nend",
        ] {
            let g = p(src);
            assert!(g.clusters.is_empty(), "{src}");
            assert_eq!(g.nodes.len(), 1, "{src}");
            assert_eq!(g.nodes[0].shape, Shape::CollapsedGroup);
        }
        // Metadata without `view` (or another view) leaves the subgraph expanded and
        // never turns the subgraph id into a vertex.
        let g = p("flowchart LR\nsubgraph S\n  x\nend\nS@{ algorithm: elk.box }");
        assert_eq!(g.clusters.len(), 1);
        assert!(node(&g, "S").is_none());
    }

    #[test]
    fn shape_data_without_a_label_keeps_the_id() {
        let g = p("flowchart TD\nA@{ shape: diam }\nB[Bee]\nB@{ shape: circle }");
        assert_eq!(node(&g, "A").unwrap().label, "A");
        assert_eq!(node(&g, "A").unwrap().shape, Shape::Diamond);
        assert_eq!(node(&g, "B").unwrap().label, "Bee");
        assert_eq!(node(&g, "B").unwrap().shape, Shape::Circle);
    }

    #[test]
    fn edge_ids_and_edge_metadata() {
        let g = p("flowchart LR
subgraph S
a
end
A e1@--> B
B e2@<--> S
e1@{ animate: true }");
        assert_eq!(g.nodes.len(), 3, "{:?}", g.nodes);
        let e = edge_ids(&g);
        assert_eq!(e[0], ("A".to_string(), "B".to_string()));
        assert_eq!(e[1], ("B".to_string(), "[S]".to_string()));
        assert_eq!(g.edges[1].start, Head::Arrow);
    }

    #[test]
    fn animated_edges_take_the_animation_dash() {
        let g = p("flowchart LR
A e1@--> B
B e2@-.-> C
C e3@--> D
D e4@--> E
E --> F
e1@{ animate: true }
e2@{ animation: fast }
classDef anim animation: slow, stroke: #f00
class e3 anim
linkStyle 0,4 stroke-dasharray: 2 2");
        let dash: Vec<Option<Vec<f64>>> = g.edges.iter().map(|e| e.style.dash.clone()).collect();
        let anim = Some(vec![9.0, 5.0]);
        assert_eq!(
            dash,
            [anim.clone(), anim.clone(), anim, None, Some(vec![2.0, 2.0])],
            "animation wins over linkStyle and the dotted pattern"
        );
    }

    #[test]
    fn inherit_dir_gives_undirected_subgraphs_the_diagram_direction() {
        let src = "flowchart LR
subgraph A
direction TB
a
end
subgraph B
b
end";
        let off = p(src);
        assert_eq!(cluster(&off, "A").direction, Some(Direction::TopBottom));
        assert_eq!(cluster(&off, "B").direction, None);
        let on = parse_with(src, None, ParseOptions { inherit_dir: true }).unwrap();
        assert_eq!(cluster(&on, "A").direction, Some(Direction::TopBottom));
        assert_eq!(cluster(&on, "B").direction, Some(Direction::LeftRight));
    }

    #[test]
    fn quoted_and_markdown_labels_may_span_lines() {
        let g = p(
            "flowchart TD\n  A[\"line1\nline3\"] --> B[\"`md1\nmd2`\"]\n  B -- \"edge\nlabel\" --> C\n  C --> D",
        );
        assert_eq!(g.nodes[0].label, "line1\nline3");
        assert!(
            g.nodes[1].label.contains("md1\nmd2"),
            "{}",
            g.nodes[1].label
        );
        assert_eq!(g.edges[1].label.as_deref(), Some("\"edge\nlabel\""));
        assert_eq!(g.nodes.len(), 4);
        // An unclosed string still reports the line it starts on.
        let e = parse("graph TD\nA[\"open\nB --> C", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }), "{e:?}");
        // Errors after a multi-line statement keep their own line numbers.
        let e = parse("graph TD\nA[\"a\nb\"]\nA -->", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 4, .. }), "{e:?}");
    }
}
