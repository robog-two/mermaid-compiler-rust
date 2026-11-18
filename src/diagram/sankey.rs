//! `sankey-beta` diagrams: CSV `source,target,value` rows laid out with a port
//! of the d3-sankey algorithm (longest-path columns, value-proportional node
//! heights, iterative relaxation + collision resolution), drawn as mermaid
//! draws them: stroked `sankeyLinkHorizontal` curves with a gradient each.

use crate::config::MermaidConfig;
use crate::diagram::xychart::keyword;
use crate::error::{Error, Result};
use crate::geom::{Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Gradient, Paint, Scene, Text};
use crate::theme::Palette;

// ---------------------------------------------------------------------------
// IR + parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub source: usize,
    pub target: usize,
    pub value: f64,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sankey {
    /// Node names in order of first appearance.
    pub nodes: Vec<String>,
    pub links: Vec<Link>,
}

/// Split one CSV record (RFC 4180 quoting: `"a, b"`, `""` escapes).
pub fn csv_fields(line: &str, line_no: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        // Skip leading spaces before a field.
        while chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
            chars.next();
        }
        let mut field = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    field.push(c);
                }
            }
            if !closed {
                return Err(Error::parse(line_no, "Unterminated quoted field"));
            }
            while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                chars.next();
            }
            match chars.next() {
                None => {
                    out.push(field);
                    break;
                }
                Some(',') => out.push(field),
                Some(c) => {
                    return Err(Error::parse(
                        line_no,
                        format!("Unexpected '{c}' after quoted field"),
                    ));
                }
            }
        } else {
            let mut ended = true;
            for c in chars.by_ref() {
                if c == ',' {
                    ended = false;
                    break;
                }
                field.push(c);
            }
            out.push(field.trim().to_string());
            if ended {
                break;
            }
        }
    }
    Ok(out)
}

/// Parse a sankey body.
pub fn parse(body: &str) -> Result<Sankey> {
    let mut g = Sankey::default();
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = keyword(line, "sankey-beta")
                .or_else(|| keyword(line, "sankey"))
                .ok_or_else(|| Error::parse(line_no, "Expecting 'sankey-beta'"))?;
            if !rest.is_empty() {
                return Err(Error::parse(line_no, format!("Unexpected '{rest}'")));
            }
            header = true;
            continue;
        }
        let f = csv_fields(line, line_no)?;
        if f.len() != 3 {
            return Err(Error::parse(
                line_no,
                format!(
                    "{line}\nExpecting 3 columns (source,target,value), got {}",
                    f.len()
                ),
            ));
        }
        let value = f[2]
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| {
                Error::parse(
                    line_no,
                    format!("Expecting a non-negative number, got '{}'", f[2]),
                )
            })?;
        if f[0].is_empty() || f[1].is_empty() {
            return Err(Error::parse(line_no, "Node names must not be empty"));
        }
        if f[0] == f[1] {
            return Err(Error::parse(
                line_no,
                format!("Circular link: '{}' links to itself", f[0]),
            ));
        }
        let mut id = |name: &str| match g.nodes.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                g.nodes.push(name.to_string());
                g.nodes.len() - 1
            }
        };
        let (s, t) = (id(&f[0]), id(&f[1]));
        g.links.push(Link {
            source: s,
            target: t,
            value,
            line: line_no,
        });
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'sankey-beta'"));
    }
    // sankey.jison needs at least one record after the header.
    if g.links.is_empty() {
        return Err(Error::parse(1, "Expecting 'NEWLINE', got 'EOF'"));
    }
    Ok(g)
}

// ---------------------------------------------------------------------------
// Layout (d3-sankey 0.12.3 port, as mermaid bundles it)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutOptions {
    pub extent: Rect,
    pub node_width: f64,
    pub node_padding: f64,
    pub align: Align,
    pub iterations: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeBox {
    pub value: f64,
    pub depth: usize,
    pub height: usize,
    pub layer: usize,
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
    pub source_links: Vec<usize>,
    pub target_links: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkBand {
    pub source: usize,
    pub target: usize,
    pub value: f64,
    pub width: f64,
    /// Centre y of the band at the source / target node.
    pub y0: f64,
    pub y1: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SankeyLayout {
    pub nodes: Vec<NodeBox>,
    pub links: Vec<LinkBand>,
}

use crate::layout::dagre::js_sort;

/// JavaScript's `a - b || tie`: the difference, or the tie-break when it is 0 or
/// NaN (a node not placed yet has an `undefined` breadth).
fn or_tie(d: f64, tie: f64) -> f64 {
    if d == 0.0 || d.is_nan() { tie } else { d }
}

struct Engine {
    nodes: Vec<NodeBox>,
    links: Vec<LinkBand>,
    py: f64,
    y0: f64,
    y1: f64,
}

impl Engine {
    /// `sourceLinks.sort(ascendingTargetBreadth)`.
    fn sort_source_links(&mut self, n: usize) {
        let mut sl = std::mem::take(&mut self.nodes[n].source_links);
        let (nodes, links) = (&self.nodes, &self.links);
        js_sort(&mut sl, |&a, &b| {
            let d = nodes[links[a].target].y0 - nodes[links[b].target].y0;
            or_tie(d, a as f64 - b as f64)
        });
        self.nodes[n].source_links = sl;
    }

    /// `targetLinks.sort(ascendingSourceBreadth)`.
    fn sort_target_links(&mut self, n: usize) {
        let mut tl = std::mem::take(&mut self.nodes[n].target_links);
        let (nodes, links) = (&self.nodes, &self.links);
        js_sort(&mut tl, |&a, &b| {
            let d = nodes[links[a].source].y0 - nodes[links[b].source].y0;
            or_tie(d, a as f64 - b as f64)
        });
        self.nodes[n].target_links = tl;
    }

    /// `reorderNodeLinks`: re-sort the link lists at the far ends of `n`'s links.
    fn reorder_node_links(&mut self, n: usize) {
        for l in self.nodes[n].target_links.clone() {
            self.sort_source_links(self.links[l].source);
        }
        for l in self.nodes[n].source_links.clone() {
            self.sort_target_links(self.links[l].target);
        }
    }

    fn target_top(&self, source: usize, target: usize) -> f64 {
        let s = &self.nodes[source];
        let mut y = s.y0 - (s.source_links.len() as f64 - 1.0) * self.py / 2.0;
        for &l in &s.source_links {
            if self.links[l].target == target {
                break;
            }
            y += self.links[l].width + self.py;
        }
        for &l in &self.nodes[target].target_links {
            if self.links[l].source == source {
                break;
            }
            y -= self.links[l].width;
        }
        y
    }

    fn source_top(&self, source: usize, target: usize) -> f64 {
        let t = &self.nodes[target];
        let mut y = t.y0 - (t.target_links.len() as f64 - 1.0) * self.py / 2.0;
        for &l in &t.target_links {
            if self.links[l].source == source {
                break;
            }
            y += self.links[l].width + self.py;
        }
        for &l in &self.nodes[source].source_links {
            if self.links[l].target == target {
                break;
            }
            y -= self.links[l].width;
        }
        y
    }

    fn shift(&mut self, n: usize, dy: f64) {
        self.nodes[n].y0 += dy;
        self.nodes[n].y1 += dy;
    }

    /// `column.sort(ascendingBreadth)` then `resolveCollisions(column, beta)`.
    fn settle_column(&mut self, col: &mut [usize], beta: f64) {
        let nodes = &self.nodes;
        js_sort(col, |&a, &b| nodes[a].y0 - nodes[b].y0);
        let i = col.len() >> 1;
        let subject = col[i];
        let (sy0, sy1) = (self.nodes[subject].y0, self.nodes[subject].y1);
        self.bottom_to_top(col, sy0 - self.py, i as isize - 1, beta);
        self.top_to_bottom(col, sy1 + self.py, i + 1, beta);
        self.bottom_to_top(col, self.y1, col.len() as isize - 1, beta);
        self.top_to_bottom(col, self.y0, 0, beta);
    }

    fn top_to_bottom(&mut self, col: &[usize], mut y: f64, from: usize, alpha: f64) {
        for &n in col.iter().skip(from) {
            let dy = (y - self.nodes[n].y0) * alpha;
            if dy > 1e-6 {
                self.shift(n, dy);
            }
            y = self.nodes[n].y1 + self.py;
        }
    }

    fn bottom_to_top(&mut self, col: &[usize], mut y: f64, from: isize, alpha: f64) {
        let mut i = from;
        while i >= 0 {
            let n = col[i as usize];
            let dy = (self.nodes[n].y1 - y) * alpha;
            if dy > 1e-6 {
                self.shift(n, -dy);
            }
            y = self.nodes[n].y0 - self.py;
            i -= 1;
        }
    }

    fn relax_left_to_right(&mut self, columns: &mut [Vec<usize>], alpha: f64, beta: f64) {
        for column in columns.iter_mut().skip(1) {
            for &tgt in column.iter() {
                let (mut y, mut w) = (0.0, 0.0);
                for &l in &self.nodes[tgt].target_links {
                    let s = self.links[l].source;
                    let v = self.links[l].value
                        * (self.nodes[tgt].layer as f64 - self.nodes[s].layer as f64);
                    y += self.target_top(s, tgt) * v;
                    w += v;
                }
                // `if (!(w > 0)) continue;`
                if w.is_nan() || w <= 0.0 {
                    continue;
                }
                let dy = (y / w - self.nodes[tgt].y0) * alpha;
                self.shift(tgt, dy);
                self.reorder_node_links(tgt);
            }
            self.settle_column(column, beta);
        }
    }

    fn relax_right_to_left(&mut self, columns: &mut [Vec<usize>], alpha: f64, beta: f64) {
        let n = columns.len();
        for column in columns.iter_mut().take(n.saturating_sub(1)).rev() {
            for &src in column.iter() {
                let (mut y, mut w) = (0.0, 0.0);
                for &l in &self.nodes[src].source_links {
                    let t = self.links[l].target;
                    let v = self.links[l].value
                        * (self.nodes[t].layer as f64 - self.nodes[src].layer as f64);
                    y += self.source_top(src, t) * v;
                    w += v;
                }
                // `if (!(w > 0)) continue;`
                if w.is_nan() || w <= 0.0 {
                    continue;
                }
                let dy = (y / w - self.nodes[src].y0) * alpha;
                self.shift(src, dy);
                self.reorder_node_links(src);
            }
            self.settle_column(column, beta);
        }
    }
}

/// Compute node rectangles and link bands (d3-sankey's `sankey(graph)`).
pub fn layout(g: &Sankey, opt: &LayoutOptions) -> Result<SankeyLayout> {
    let n = g.nodes.len();
    let mut nodes: Vec<NodeBox> = vec![
        NodeBox {
            // `undefined` until `initializeNodeBreadths` places the node.
            y0: f64::NAN,
            y1: f64::NAN,
            ..NodeBox::default()
        };
        n
    ];
    let links: Vec<LinkBand> = g
        .links
        .iter()
        .map(|l| LinkBand {
            source: l.source,
            target: l.target,
            value: l.value,
            width: 0.0,
            y0: 0.0,
            y1: 0.0,
        })
        .collect();
    for (i, l) in g.links.iter().enumerate() {
        if l.source >= n || l.target >= n {
            return Err(Error::parse(l.line, "Link refers to an unknown node"));
        }
        nodes[l.source].source_links.push(i);
        nodes[l.target].target_links.push(i);
    }
    // computeNodeValues
    for nd in &mut nodes {
        let out: f64 = nd.source_links.iter().map(|&l| links[l].value).sum();
        let inn: f64 = nd.target_links.iter().map(|&l| links[l].value).sum();
        nd.value = out.max(inn);
    }
    // computeNodeDepths / computeNodeHeights
    let cycle_err = |frontier: &[usize], nodes: &[NodeBox], forward: bool| {
        let line = frontier
            .iter()
            .flat_map(|&f| {
                if forward {
                    nodes[f].source_links.clone()
                } else {
                    nodes[f].target_links.clone()
                }
            })
            .map(|l| g.links[l].line)
            .min()
            .unwrap_or(1);
        Error::parse(
            line,
            "Circular link detected: sankey diagrams must be acyclic",
        )
    };
    for forward in [true, false] {
        let mut current: Vec<usize> = (0..n).collect();
        let mut x = 0usize;
        while !current.is_empty() {
            let mut next = vec![false; n];
            for &c in &current {
                if forward {
                    nodes[c].depth = x;
                } else {
                    nodes[c].height = x;
                }
                let adj = if forward {
                    &nodes[c].source_links
                } else {
                    &nodes[c].target_links
                };
                for &l in adj {
                    let o = if forward {
                        links[l].target
                    } else {
                        links[l].source
                    };
                    next[o] = true;
                }
            }
            x += 1;
            if x > n {
                return Err(cycle_err(&current, &nodes, forward));
            }
            current = (0..n).filter(|&i| next[i]).collect();
        }
    }
    // computeNodeLayers
    let ext = opt.extent;
    let (x0, x1, y0, y1) = (ext.x, ext.right(), ext.y, ext.bottom());
    let layers = nodes.iter().map(|nd| nd.depth).max().map_or(0, |d| d + 1);
    let kx = (x1 - x0 - opt.node_width) / (layers as f64 - 1.0);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); layers.max(1)];
    for i in 0..n {
        let nd = &nodes[i];
        let raw: i64 = match opt.align {
            Align::Left => nd.depth as i64,
            Align::Right => layers as i64 - 1 - nd.height as i64,
            Align::Justify => {
                if nd.source_links.is_empty() {
                    layers as i64 - 1
                } else {
                    nd.depth as i64
                }
            }
            Align::Center => {
                if !nd.target_links.is_empty() {
                    nd.depth as i64
                } else if !nd.source_links.is_empty() {
                    nd.source_links
                        .iter()
                        .map(|&l| nodes[links[l].target].depth as i64)
                        .min()
                        .unwrap_or(0)
                        - 1
                } else {
                    0
                }
            }
        };
        let layer = raw.min(layers as i64 - 1).max(0) as usize;
        nodes[i].layer = layer;
        nodes[i].x0 = x0 + layer as f64 * kx;
        nodes[i].x1 = nodes[i].x0 + opt.node_width;
        columns[layer].push(i);
    }
    columns.retain(|c| !c.is_empty());

    // computeNodeBreadths
    let max_len = columns.iter().map(Vec::len).max().unwrap_or(1);
    let py = opt.node_padding.min((y1 - y0) / (max_len as f64 - 1.0));
    let ky = columns
        .iter()
        .map(|c| {
            let sum: f64 = c.iter().map(|&i| nodes[i].value).sum();
            (y1 - y0 - (c.len() as f64 - 1.0) * py) / sum
        })
        .filter(|v| !v.is_nan())
        .fold(f64::INFINITY, f64::min);
    let mut eng = Engine {
        nodes,
        links,
        py,
        y0,
        y1,
    };
    // initializeNodeBreadths
    for c in &columns {
        let mut y = y0;
        for &i in c {
            eng.nodes[i].y0 = y;
            eng.nodes[i].y1 = y + eng.nodes[i].value * ky;
            y = eng.nodes[i].y1 + py;
            for l in eng.nodes[i].source_links.clone() {
                eng.links[l].width = eng.links[l].value * ky;
            }
        }
        let spare = (y1 - y + py) / (c.len() as f64 + 1.0);
        for (k, &i) in c.iter().enumerate() {
            eng.shift(i, spare * (k as f64 + 1.0));
        }
        // reorderLinks: nodes of later columns have no breadth yet.
        for &i in c {
            eng.sort_source_links(i);
            eng.sort_target_links(i);
        }
    }
    let iters = opt.iterations;
    for it in 0..iters {
        let alpha = 0.99f64.powf(it as f64);
        let beta = (1.0 - alpha).max((it as f64 + 1.0) / iters as f64);
        eng.relax_right_to_left(&mut columns, alpha, beta);
        eng.relax_left_to_right(&mut columns, alpha, beta);
    }

    // computeLinkBreadths
    for i in 0..n {
        let mut ys = eng.nodes[i].y0;
        for l in eng.nodes[i].source_links.clone() {
            let w = eng.links[l].width;
            eng.links[l].y0 = ys + w / 2.0;
            ys += w;
        }
        let mut yt = eng.nodes[i].y0;
        for l in eng.nodes[i].target_links.clone() {
            let w = eng.links[l].width;
            eng.links[l].y1 = yt + w / 2.0;
            yt += w;
        }
    }
    Ok(SankeyLayout {
        nodes: eng.nodes,
        links: eng.links,
    })
}

// ---------------------------------------------------------------------------
// Drawing (mermaid's `sankeyRenderer.ts`)
// ---------------------------------------------------------------------------

/// d3's `schemeTableau10`, which mermaid uses for sankey nodes.
const TABLEAU10: [&str; 10] = [
    "#4e79a7", "#f28e2c", "#e15759", "#76b7b2", "#59a14f", "#edc949", "#af7aa1", "#ff9da7",
    "#9c755f", "#bab0ab",
];

/// Font size of the `node-labels` group.
const LABEL_FONT: f64 = 14.0;

/// `getBBox()` of a one-line SVG label at `x` (anchored) with baseline `y`, as
/// mermaid measures it while drawing: the run (its advance in layout units)
/// placed by the anchor, extended to the last glyph's ink (`8.86` ends 0.2px
/// past its advance).
fn label_box(t: &Text) -> Rect {
    let line = t.lines.first().map(String::as_str).unwrap_or("");
    let adv = crate::text::layout_unit_ceil(crate::text::line_width(line, t.font_size, false));
    let x = match t.anchor {
        Anchor::Start => t.x,
        Anchor::Middle => t.x - adv / 2.0,
        Anchor::End => t.x - adv,
    };
    let ascent = (0.905 * t.font_size).round();
    Rect::new(
        x,
        t.y - ascent,
        crate::text::svg_bbox_width(line, t.font_size),
        crate::text::svg_line_box(t.font_size, line),
    )
}

/// Render a `sankey-beta` diagram body (already preprocessed). mermaid draws no
/// title for sankey diagrams.
pub fn render(
    body: &str,
    _title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let g = parse(body)?;
    let s = "sankey";
    let width = cfg.section_f64(s, "width", 600.0);
    let height = cfg.section_f64(s, "height", 400.0);
    let show_values = cfg.section_bool(s, "showValues", true);
    let str_opt = |k: &str| {
        cfg.section(s, k)
            .and_then(crate::json::Json::as_str)
            .map(str::to_string)
    };
    let prefix = str_opt("prefix").unwrap_or_default();
    let suffix = str_opt("suffix").unwrap_or_default();
    let align = match str_opt("nodeAlignment").as_deref() {
        Some("left") => Align::Left,
        Some("right") => Align::Right,
        Some("center") => Align::Center,
        _ => Align::Justify,
    };
    let outlined = str_opt("labelStyle").as_deref() == Some("outlined");
    let link_color = str_opt("linkColor").unwrap_or_else(|| "gradient".to_string());
    let node_width = cfg.section_f64(s, "nodeWidth", 10.0);
    let node_padding =
        cfg.section_f64(s, "nodePadding", 12.0) + if show_values { 15.0 } else { 0.0 };
    let opt = LayoutOptions {
        extent: Rect::new(0.0, 0.0, width, height),
        node_width,
        node_padding,
        align,
        iterations: 6,
    };
    let lay = layout(&g, &opt)?;

    // `nodeColors[id] ?? ordinal(schemeTableau10)(id)`: the scale hands out colours
    // in the order it first sees ids (the nodes, in order; overridden ones never ask).
    let overrides = cfg.section(s, "nodeColors");
    let mut scheme = 0usize;
    let colors: Vec<Color> = g
        .nodes
        .iter()
        .map(|id| {
            match overrides
                .and_then(|o| o.get(id))
                .and_then(|c| c.to_scalar_string())
            {
                Some(c) => Color::new(c),
                None => {
                    scheme += 1;
                    Color::new(TABLEAU10[(scheme - 1) % TABLEAU10.len()])
                }
            }
        })
        .collect();

    let mut bounds = Bounds::default();
    // Nodes.
    let mut node_els = Vec::new();
    for (i, nd) in lay.nodes.iter().enumerate() {
        let r = Rect::new(nd.x0, nd.y0, nd.x1 - nd.x0, nd.y1 - nd.y0);
        bounds.add_rect(r);
        node_els.push(Element::classed_shape(
            Geometry::Rect { rect: r, rx: 0.0 },
            Paint::fill(&colors[i]),
            "node",
        ));
    }

    // Labels: `${id}\n${prefix}${round2(value)}${suffix}`, one SVG line (white
    // space collapses), on the node's middle (`dy="0em"`; without values 0.35em
    // lower).
    let central_layer = {
        let (mut max_value, mut layer) = (0.0, 0usize);
        for nd in &lay.nodes {
            if nd.value > max_value {
                max_value = nd.value;
                layer = nd.layer;
            }
        }
        layer
    };
    let mut label_els = Vec::new();
    let classes: &[&str] = if outlined {
        &["sankey-label-bg", "sankey-label-fg"]
    } else {
        &["node-label"]
    };
    for class in classes {
        for (i, nd) in lay.nodes.iter().enumerate() {
            let raw = if show_values {
                let v = (nd.value * 100.0 + 0.5).floor() / 100.0;
                format!("{}\n{prefix}{v}{suffix}", g.nodes[i])
            } else {
                g.nodes[i].clone()
            };
            let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            let start = if outlined {
                nd.layer >= central_layer
            } else {
                nd.x0 < width / 2.0
            };
            let (x, anchor) = if start {
                (nd.x1 + 6.0, Anchor::Start)
            } else {
                (nd.x0 - 6.0, Anchor::End)
            };
            let dy = if show_values { 0.0 } else { 0.35 * LABEL_FONT };
            let t = Text {
                x,
                y: (nd.y1 + nd.y0) / 2.0 + dy,
                lines: vec![line],
                line_height: LABEL_FONT * 1.2,
                font_size: LABEL_FONT,
                bold: false,
                weight: None,
                anchor,
                fill: pal.text_color.clone(),
                tspan_central: false,
            };
            bounds.add_rect(label_box(&t));
            label_els.push(if *class == "sankey-label-bg" {
                // `.sankey-label-bg` in the CSS below; PDF output strokes it itself.
                Element::OutlinedText {
                    text: t,
                    class: Some((*class).to_string()),
                    color: pal.main_bkg.clone(),
                    width: 4.0,
                }
            } else {
                Element::classed_text(t, *class)
            });
        }
    }

    // Links: `sankeyLinkHorizontal()` stroked `max(1, width)` wide at
    // `stroke-opacity: 0.5`, each with its own source-to-target gradient.
    let mut link_els = Vec::new();
    for l in &lay.links {
        let (x0, x1) = (lay.nodes[l.source].x1, lay.nodes[l.target].x0);
        let xm = (x0 + x1) / 2.0;
        let d = vec![
            PathCmd::MoveTo(Point::new(x0, l.y0)),
            PathCmd::CubicTo(
                Point::new(xm, l.y0),
                Point::new(xm, l.y1),
                Point::new(x1, l.y1),
            ),
        ];
        bounds.add_rect(Rect::new(
            x0.min(x1),
            l.y0.min(l.y1),
            (x1 - x0).abs(),
            (l.y1 - l.y0).abs(),
        ));
        let (sc, tc) = (&colors[l.source], &colors[l.target]);
        let w = l.width.max(1.0);
        let paint = match link_color.as_str() {
            "gradient" => Paint::stroke(sc, w).with_stroke_gradient(Gradient {
                x1: x0,
                x2: x1,
                from: sc.clone(),
                to: tc.clone(),
                bbox: false,
            }),
            "source" => Paint::stroke(sc, w),
            "target" => Paint::stroke(tc, w),
            other => Paint::stroke(&Color::new(other), w),
        };
        link_els.push(Element::classed_shape(
            Geometry::Path(d),
            paint.with_opacity(0.5).with_multiply(),
            "link",
        ));
    }

    let mut css =
        String::from("rect.node{shape-rendering:crispEdges;}.link{mix-blend-mode:multiply;}");
    if outlined {
        css.push_str(&format!(
            ".sankey-label-bg{{stroke:{};stroke-width:4px;stroke-linejoin:round;paint-order:stroke;}}",
            pal.main_bkg.css()
        ));
    }
    let els = vec![
        Element::group("nodes", node_els),
        Element::group("node-labels", label_els),
        Element::group("links", link_els),
    ];
    Ok(Scene {
        kind: DiagramKind::Sankey,
        view_box: bounds.rect(),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "sankey-beta\n\nAgricultural 'waste',Bio-conversion,124.729\nBio-conversion,Liquid,0.597\nBio-conversion,Losses,26.862\nBio-conversion,Solid,280.322\nBio-conversion,Gas,81.144\nBiofuel imports,Liquid,35\nBiomass imports,Solid,35\nCoal imports,Coal,11.606\nCoal reserves,Coal,63.965\nCoal,Solid,75.571\nGas,Heat,40\nSolid,Heat,300\nLiquid,Transport,30\n";

    fn opts() -> LayoutOptions {
        LayoutOptions {
            extent: Rect::new(0.0, 0.0, 600.0, 400.0),
            node_width: 10.0,
            node_padding: 12.0,
            align: Align::Justify,
            iterations: 6,
        }
    }

    #[test]
    fn csv_quoting() {
        let f = csv_fields(r#""Hello, world",target,"say ""hi""""#, 1).unwrap();
        assert_eq!(f, vec!["Hello, world", "target", "say \"hi\""]);
        assert!(csv_fields(r#""open,b,1"#, 4).is_err());
        let g = parse("sankey-beta\n\"a, b\",c,1.5\n").unwrap();
        assert_eq!(g.nodes, vec!["a, b", "c"]);
        assert_eq!(g.links[0].value, 1.5);
    }

    #[test]
    fn parse_errors() {
        let e = parse("sankey-beta\n\na,b\n").unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("sankey-beta\na,b,x\n").unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("sankey-beta\na,b,-1\n").unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("graph\n").is_err());
    }

    #[test]
    fn cycles_rejected() {
        let g = parse("sankey-beta\na,b,1\nb,c,1\nc,a,1\n").unwrap();
        assert!(matches!(layout(&g, &opts()), Err(Error::Parse { .. })));
    }

    #[test]
    fn columns_follow_longest_path() {
        let g = parse("sankey-beta\na,b,1\nb,c,1\na,c,1\n").unwrap();
        let l = layout(&g, &opts()).unwrap();
        assert_eq!(
            l.nodes.iter().map(|n| n.layer).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn layout_invariants() {
        let g = parse(SRC).unwrap();
        let o = opts();
        let l = layout(&g, &o).unwrap();
        // Flow conservation: node value = max(in, out); intermediate nodes balance here.
        for (i, n) in l.nodes.iter().enumerate() {
            let inn: f64 = l
                .links
                .iter()
                .filter(|k| k.target == i)
                .map(|k| k.value)
                .sum();
            let out: f64 = l
                .links
                .iter()
                .filter(|k| k.source == i)
                .map(|k| k.value)
                .sum();
            assert!((n.value - inn.max(out)).abs() < 1e-9);
            // Node height proportional to value; link widths sum to node height on each side.
            let ky = (n.y1 - n.y0) / n.value;
            let w_out: f64 = l
                .links
                .iter()
                .filter(|k| k.source == i)
                .map(|k| k.width)
                .sum();
            let w_in: f64 = l
                .links
                .iter()
                .filter(|k| k.target == i)
                .map(|k| k.width)
                .sum();
            assert!((w_out - out * ky).abs() < 1e-6);
            assert!((w_in - inn * ky).abs() < 1e-6);
            assert!(n.y0 >= o.extent.y - 1e-6 && n.y1 <= o.extent.bottom() + 1e-6);
        }
        // Same ky everywhere.
        let kys: Vec<f64> = l.nodes.iter().map(|n| (n.y1 - n.y0) / n.value).collect();
        assert!(kys.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9));
        // No overlaps within a column.
        for a in 0..l.nodes.len() {
            for b in 0..l.nodes.len() {
                let (na, nb) = (&l.nodes[a], &l.nodes[b]);
                if a != b && na.layer == nb.layer {
                    assert!(
                        na.y1 <= nb.y0 + 1e-6 || nb.y1 <= na.y0 + 1e-6,
                        "nodes {a} and {b} overlap"
                    );
                }
            }
        }
        // Sinks are justified to the last column.
        let last = l.nodes.iter().map(|n| n.layer).max().unwrap();
        let heat = g.nodes.iter().position(|n| n == "Heat").unwrap();
        assert_eq!(l.nodes[heat].layer, last);
        // Links attach within their nodes' extents.
        for k in &l.links {
            let (s, t) = (&l.nodes[k.source], &l.nodes[k.target]);
            assert!(k.y0 - k.width / 2.0 >= s.y0 - 1e-6 && k.y0 + k.width / 2.0 <= s.y1 + 1e-6);
            assert!(k.y1 - k.width / 2.0 >= t.y0 - 1e-6 && k.y1 + k.width / 2.0 <= t.y1 + 1e-6);
        }
    }

    const EXAMPLE: &str = "sankey-beta\nAgricultural 'waste',Bio-conversion,124.729\nBio-conversion,Liquid,0.597\nBio-conversion,Losses,26.862\nBio-conversion,Solid,280.322\nBio-conversion,Gas,81.144\nBiofuel imports,Liquid,35\nBiomass imports,Solid,35\nCoal imports,Coal,11.606\nCoal reserves,Coal,63.965\nCoal,Solid,75.571\nGas,Heating and cooling - commercial,22.567\nGas,\"Heating and cooling, homes\",46.477\nGas,Industry,30.1\nLiquid,Industry,10.6\nLiquid,Transport,25.6\nSolid,Industry,280.0\nSolid,Thermal generation,111.893\nThermal generation,Electricity grid,82.0\nThermal generation,Losses,30.0\nElectricity grid,Industry,40.0\nElectricity grid,\"Heating and cooling, homes\",42.0\n\"Wind \"\"offshore\"\"\",Electricity grid,35.0\n";

    /// Node positions mermaid 12.3.0 draws for its docs example (800×460,
    /// values shown: padding 12 + 15): `initializeNodeBreadths` sorts a column's
    /// source links while later columns have no breadth yet, relaxation re-sorts
    /// only the far ends' lists, and the link breadths are not sorted again.
    #[test]
    fn layout_matches_mermaid_example() {
        let g = parse(EXAMPLE).unwrap();
        let l = layout(
            &g,
            &LayoutOptions {
                extent: Rect::new(0.0, 0.0, 800.0, 460.0),
                node_width: 10.0,
                node_padding: 27.0,
                align: Align::Justify,
                iterations: 6,
            },
        )
        .unwrap();
        let want = [
            (0.0, 0.0),
            (158.0, 0.0),
            (316.0, 377.1007611750894),
            (790.0, 256.09604010061236),
            (316.0, 0.0),
            (316.0, 275.90805224050257),
            (0.0, 378.8304175720801),
            (0.0, 161.27637176759225),
            (0.0, 259.7363740050274),
            (158.0, 282.1699232142087),
            (0.0, 294.1078423759942),
            (790.0, 402.40707246042086),
            (790.0, 319.21153506097005),
            (790.0, 0.0),
            (790.0, 443.74034203888084),
            (474.0, 191.45859362317756),
            (632.0, 167.17724187977333),
            (0.0, 210.5063728863098),
        ];
        assert_eq!(l.nodes.len(), want.len());
        for (i, (n, (x, y))) in l.nodes.iter().zip(want).enumerate() {
            assert!(
                (n.x0 - x).abs() < 1e-9 && (n.y0 - y).abs() < 1e-9,
                "node {i}: ({}, {}) want ({x}, {y})",
                n.x0,
                n.y0
            );
        }
    }

    /// mermaid 12.3.0, `showValues: false` (padding 12) with three sinks.
    #[test]
    fn single_source_fan_out_matches_mermaid() {
        let g = parse("sankey\nElectricity grid,Heating and cooling - homes,113.726\nElectricity grid,Industry,342.165\nElectricity grid,Losses,56.691\n").unwrap();
        let l = layout(&g, &opts()).unwrap();
        let ys: Vec<(f64, f64)> = l.nodes.iter().map(|n| (n.y0, n.y1 - n.y0)).collect();
        let want = [
            (9.673843260105073, 376.00000000000006),
            (0.0, 83.42270309921143),
            (95.42270309921143, 250.99211443242257),
            (358.414817531634, 41.58518246836604),
        ];
        for (got, want) in ys.iter().zip(want) {
            assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9);
        }
        // Link centres where mermaid strokes them.
        assert!((l.links[0].y0 - 51.3851948097108).abs() < 1e-9);
        assert!((l.links[2].y1 - 379.20740876581704).abs() < 1e-9);
    }

    #[test]
    fn links_are_gradient_strokes_and_overridden_nodes_skip_the_scheme() {
        let cfg = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(r##"{"sankey":{"nodeColors":{"a":"#123456"}}}"##).unwrap(),
            )
            .unwrap();
        let s = render("sankey\na,b,1\nb,c,1\n", None, &cfg, &cfg.palette()).unwrap();
        let mut fills = Vec::new();
        let mut strokes = Vec::new();
        fn walk(els: &[Element], f: &mut Vec<String>, st: &mut Vec<Paint>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => walk(children, f, st),
                    Element::Shape { paint, class, .. } if class.as_deref() == Some("node") => {
                        f.push(paint.fill.css().to_string())
                    }
                    Element::Shape { paint, class, .. } if class.as_deref() == Some("link") => {
                        st.push(paint.clone())
                    }
                    _ => {}
                }
            }
        }
        walk(&s.elements, &mut fills, &mut strokes);
        // `b` gets the scheme's first colour: `a` never asked the scale.
        assert_eq!(fills, ["#123456", "#4e79a7", "#f28e2c"]);
        let g = strokes[0].stroke_gradient.as_ref().unwrap();
        assert_eq!((g.from.css(), g.to.css()), ("#123456", "#4e79a7"));
        assert_eq!((g.x1, g.x2), (10.0, 295.0));
        assert!(strokes[0].fill.is_none());
        // No padding around the drawing: the viewBox is the content's bounding box.
        assert_eq!(s.view_box.x, 0.0);
        assert_eq!(s.view_box.w, 600.0);
    }

    #[test]
    fn renders() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(SRC, None, &cfg, &pal).unwrap();
        assert_eq!(s.kind, DiagramKind::Sankey);
    }

    #[test]
    fn labels_are_one_line_on_the_node_middle() {
        // `${id}\n${value}` renders as one SVG text line with `dy="0em"`.
        let cfg = MermaidConfig::default();
        let s = render(
            "sankey-beta\nAgricultural waste,Bio-conversion,124.729",
            None,
            &cfg,
            &cfg.palette(),
        )
        .unwrap();
        fn labels(els: &[Element], out: &mut Vec<crate::scene::Text>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => labels(children, out),
                    Element::Text {
                        text,
                        class: Some(c),
                    } if c == "node-label" => out.push(text.clone()),
                    _ => {}
                }
            }
        }
        let mut t = Vec::new();
        labels(&s.elements, &mut t);
        assert_eq!(t[0].lines, ["Agricultural waste 124.73"]);
        // A single link fills the height: the node spans 0..400.
        assert!((t[0].y - 200.0).abs() < 1e-9, "{}", t[0].y);
    }
}
