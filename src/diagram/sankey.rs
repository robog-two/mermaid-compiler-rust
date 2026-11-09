//! `sankey-beta` diagrams: CSV `source,target,value` rows laid out with a port
//! of the d3-sankey algorithm (longest-path columns, value-proportional node
//! heights, iterative relaxation + collision resolution), drawn as filled
//! cubic ribbons.

use crate::color::{self, Rgba};
use crate::config::MermaidConfig;
use crate::diagram::xychart::{fmt_num, keyword, text_at, text_bounds};
use crate::error::{Error, Result};
use crate::geom::{Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene};
use crate::text::line_width;
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
    Ok(g)
}

// ---------------------------------------------------------------------------
// Layout (d3-sankey port)
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

struct Engine {
    nodes: Vec<NodeBox>,
    links: Vec<LinkBand>,
    py: f64,
}

impl Engine {
    fn sort_links(&mut self, n: usize) {
        let mut sl = self.nodes[n].source_links.clone();
        sl.sort_by(|&a, &b| {
            let (ta, tb) = (self.links[a].target, self.links[b].target);
            self.nodes[ta]
                .y0
                .total_cmp(&self.nodes[tb].y0)
                .then(a.cmp(&b))
        });
        let mut tl = self.nodes[n].target_links.clone();
        tl.sort_by(|&a, &b| {
            let (sa, sb) = (self.links[a].source, self.links[b].source);
            self.nodes[sa]
                .y0
                .total_cmp(&self.nodes[sb].y0)
                .then(a.cmp(&b))
        });
        self.nodes[n].source_links = sl;
        self.nodes[n].target_links = tl;
    }

    fn reorder_node_links(&mut self, n: usize) {
        let tl = self.nodes[n].target_links.clone();
        for l in tl {
            let s = self.links[l].source;
            self.sort_links(s);
        }
        let sl = self.nodes[n].source_links.clone();
        for l in sl {
            let t = self.links[l].target;
            self.sort_links(t);
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

    fn resolve_collisions(&mut self, col: &[usize], alpha: f64, y0: f64, y1: f64) {
        if col.is_empty() {
            return;
        }
        let i = col.len() >> 1;
        let subject = col[i];
        let (sy0, sy1) = (self.nodes[subject].y0, self.nodes[subject].y1);
        self.bottom_to_top(col, sy0 - self.py, i as isize - 1, alpha);
        self.top_to_bottom(col, sy1 + self.py, i + 1, alpha);
        self.bottom_to_top(col, y1, col.len() as isize - 1, alpha);
        self.top_to_bottom(col, y0, 0, alpha);
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

    fn sort_column(&self, col: &mut [usize]) {
        col.sort_by(|&a, &b| self.nodes[a].y0.total_cmp(&self.nodes[b].y0));
    }
}

/// Compute node rectangles and link bands.
pub fn layout(g: &Sankey, opt: &LayoutOptions) -> Result<SankeyLayout> {
    let n = g.nodes.len();
    let mut nodes: Vec<NodeBox> = vec![NodeBox::default(); n];
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
    // Node values.
    for nd in &mut nodes {
        let out: f64 = nd.source_links.iter().map(|&l| links[l].value).sum();
        let inn: f64 = nd.target_links.iter().map(|&l| links[l].value).sum();
        nd.value = out.max(inn);
    }
    // Depths (longest path from sources) and heights (to sinks).
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
    // Layers.
    let layers = nodes.iter().map(|nd| nd.depth).max().map_or(0, |d| d + 1);
    let ext = opt.extent;
    let kx = if layers > 1 {
        (ext.w - opt.node_width) / (layers - 1) as f64
    } else {
        0.0
    };
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); layers.max(1)];
    for i in 0..n {
        let nd = &nodes[i];
        let raw = match opt.align {
            Align::Left => nd.depth,
            Align::Right => (layers - 1).saturating_sub(nd.height),
            Align::Justify => {
                if nd.source_links.is_empty() {
                    layers - 1
                } else {
                    nd.depth
                }
            }
            Align::Center => {
                if !nd.target_links.is_empty() {
                    nd.depth
                } else if !nd.source_links.is_empty() {
                    nd.source_links
                        .iter()
                        .map(|&l| nodes[links[l].target].depth)
                        .min()
                        .unwrap_or(1)
                        .saturating_sub(1)
                } else {
                    0
                }
            }
        };
        let layer = raw.min(layers.saturating_sub(1));
        nodes[i].layer = layer;
        nodes[i].x0 = ext.x + layer as f64 * kx;
        nodes[i].x1 = nodes[i].x0 + opt.node_width;
        columns[layer].push(i);
    }
    columns.retain(|c| !c.is_empty());

    // Initial breadths.
    let max_len = columns.iter().map(Vec::len).max().unwrap_or(1);
    let py = if max_len > 1 {
        opt.node_padding.min(ext.h / (max_len - 1) as f64)
    } else {
        opt.node_padding
    };
    let ky = columns
        .iter()
        .map(|c| {
            let sum: f64 = c.iter().map(|&i| nodes[i].value).sum();
            if sum > 0.0 {
                (ext.h - (c.len() as f64 - 1.0) * py) / sum
            } else {
                f64::INFINITY
            }
        })
        .fold(f64::INFINITY, f64::min);
    let ky = if ky.is_finite() { ky.max(0.0) } else { 0.0 };
    let mut eng = Engine { nodes, links, py };
    for l in &mut eng.links {
        l.width = l.value * ky;
    }
    for c in &columns {
        let mut y = ext.y;
        for &i in c {
            eng.nodes[i].y0 = y;
            eng.nodes[i].y1 = y + eng.nodes[i].value * ky;
            y = eng.nodes[i].y1 + py;
        }
        let spare = (ext.bottom() - y + py) / (c.len() as f64 + 1.0);
        for (k, &i) in c.iter().enumerate() {
            eng.shift(i, spare * (k as f64 + 1.0));
        }
    }
    for i in 0..n {
        eng.sort_links(i);
    }

    // Relaxation.
    let iters = opt.iterations;
    for it in 0..iters {
        let alpha = 0.99f64.powi(it as i32);
        let beta = (1.0 - alpha).max((it as f64 + 1.0) / iters as f64);
        // Right to left.
        for ci in (0..columns.len().saturating_sub(1)).rev() {
            let col = columns[ci].clone();
            for &src in &col {
                let (mut y, mut w) = (0.0, 0.0);
                for &l in &eng.nodes[src].source_links.clone() {
                    let t = eng.links[l].target;
                    let v = eng.links[l].value
                        * (eng.nodes[t].layer as f64 - eng.nodes[src].layer as f64);
                    y += eng.source_top(src, t) * v;
                    w += v;
                }
                if w > 0.0 {
                    let dy = (y / w - eng.nodes[src].y0) * alpha;
                    eng.shift(src, dy);
                }
                eng.reorder_node_links(src);
            }
            let mut col = columns[ci].clone();
            eng.sort_column(&mut col);
            eng.resolve_collisions(&col, beta, ext.y, ext.bottom());
            columns[ci] = col;
        }
        // Left to right.
        for column in columns.iter_mut().skip(1) {
            let col = column.clone();
            for &tgt in &col {
                let (mut y, mut w) = (0.0, 0.0);
                for &l in &eng.nodes[tgt].target_links.clone() {
                    let s = eng.links[l].source;
                    let v = eng.links[l].value
                        * (eng.nodes[tgt].layer as f64 - eng.nodes[s].layer as f64);
                    y += eng.target_top(s, tgt) * v;
                    w += v;
                }
                if w > 0.0 {
                    let dy = (y / w - eng.nodes[tgt].y0) * alpha;
                    eng.shift(tgt, dy);
                }
                eng.reorder_node_links(tgt);
            }
            let mut col = column.clone();
            eng.sort_column(&mut col);
            eng.resolve_collisions(&col, beta, ext.y, ext.bottom());
            *column = col;
        }
    }

    // Link breadths.
    for i in 0..n {
        eng.sort_links(i);
    }
    for i in 0..n {
        let mut y0 = eng.nodes[i].y0;
        for l in eng.nodes[i].source_links.clone() {
            let w = eng.links[l].width;
            eng.links[l].y0 = y0 + w / 2.0;
            y0 += w;
        }
        let mut y1 = eng.nodes[i].y0;
        for l in eng.nodes[i].target_links.clone() {
            let w = eng.links[l].width;
            eng.links[l].y1 = y1 + w / 2.0;
            y1 += w;
        }
    }
    Ok(SankeyLayout {
        nodes: eng.nodes,
        links: eng.links,
    })
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// d3's `schemeTableau10`, which mermaid uses for sankey nodes.
const TABLEAU10: [&str; 10] = [
    "#4e79a7", "#f28e2c", "#e15759", "#76b7b2", "#59a14f", "#edc949", "#af7aa1", "#ff9da7",
    "#9c755f", "#bab0ab",
];

#[derive(Debug, Clone, PartialEq)]
enum LinkColor {
    Source,
    Target,
    Gradient,
    Fixed(String),
}

type Cubic = [Point; 4];

fn cubic_at(c: &Cubic, t: f64) -> Point {
    let u = 1.0 - t;
    c[0].scale(u * u * u)
        .plus(c[1].scale(3.0 * u * u * t))
        .plus(c[2].scale(3.0 * u * t * t))
        .plus(c[3].scale(t * t * t))
}

/// The part of cubic `c` between parameters `t0 < t1` (de Casteljau).
fn sub_cubic(c: &Cubic, t0: f64, t1: f64) -> Cubic {
    let split_left = |c: &Cubic, t: f64| -> Cubic {
        let p01 = c[0].lerp(c[1], t);
        let p12 = c[1].lerp(c[2], t);
        let p23 = c[2].lerp(c[3], t);
        let a = p01.lerp(p12, t);
        let b = p12.lerp(p23, t);
        [c[0], p01, a, a.lerp(b, t)]
    };
    let split_right = |c: &Cubic, t: f64| -> Cubic {
        let p01 = c[0].lerp(c[1], t);
        let p12 = c[1].lerp(c[2], t);
        let p23 = c[2].lerp(c[3], t);
        let a = p01.lerp(p12, t);
        let b = p12.lerp(p23, t);
        [a.lerp(b, t), b, p23, c[3]]
    };
    let left = split_left(c, t1);
    if t1 <= 0.0 {
        return [c[0]; 4];
    }
    split_right(&left, t0 / t1)
}

/// Filled ribbon between two horizontal-tangent cubics, restricted to `[t0, t1]`.
pub fn ribbon(x0: f64, y0: f64, x1: f64, y1: f64, w: f64, t0: f64, t1: f64) -> Vec<PathCmd> {
    let xm = (x0 + x1) / 2.0;
    let edge = |dy: f64| -> Cubic {
        [
            Point::new(x0, y0 + dy),
            Point::new(xm, y0 + dy),
            Point::new(xm, y1 + dy),
            Point::new(x1, y1 + dy),
        ]
    };
    let top = sub_cubic(&edge(-w / 2.0), t0, t1);
    let bot = sub_cubic(&edge(w / 2.0), t0, t1);
    vec![
        PathCmd::MoveTo(top[0]),
        PathCmd::CubicTo(top[1], top[2], top[3]),
        PathCmd::LineTo(bot[3]),
        PathCmd::CubicTo(bot[2], bot[1], bot[0]),
        PathCmd::Close,
    ]
}

fn mix(a: &Color, b: &Color, t: f64) -> Color {
    match (color::parse(a.css()), color::parse(b.css())) {
        (Some(x), Some(y)) => {
            let m = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * t).round() as u8;
            Color::new(
                Rgba {
                    r: m(x.r, y.r),
                    g: m(x.g, y.g),
                    b: m(x.b, y.b),
                    a: 1.0,
                }
                .to_hex(),
            )
        }
        _ => a.clone(),
    }
}

/// Render a `sankey-beta` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let g = parse(body)?;
    let s = "sankey";
    let width = cfg.section_f64(s, "width", 600.0).max(50.0);
    let height = cfg.section_f64(s, "height", 400.0).max(50.0);
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
    let link_color = match str_opt("linkColor").as_deref() {
        Some("source") => LinkColor::Source,
        Some("target") => LinkColor::Target,
        None | Some("gradient") => LinkColor::Gradient,
        Some(other) => LinkColor::Fixed(other.to_string()),
    };
    let node_width = cfg.section_f64(s, "nodeWidth", 10.0);
    let node_padding = cfg.section_f64(
        s,
        "nodePadding",
        10.0 + if show_values { 15.0 } else { 0.0 },
    );

    let mut top = 0.0;
    let mut els = Vec::new();
    let mut bounds = Bounds::default();
    if let Some(t) = title.as_ref().filter(|t| !t.is_empty()) {
        let txt = text_at(
            width / 2.0,
            pal.font_size * 1.2,
            std::slice::from_ref(t),
            pal.font_size * 1.2,
            Anchor::Middle,
            &pal.title_color,
        );
        bounds.add_rect(text_bounds(&txt));
        els.push(Element::classed_text(txt, "sankey-title"));
        top = pal.font_size * 2.4;
    }
    bounds.add_rect(Rect::new(0.0, 0.0, width, height + top));
    let opt = LayoutOptions {
        extent: Rect::new(0.0, top, width, height),
        node_width,
        node_padding,
        align,
        iterations: 6,
    };
    let lay = layout(&g, &opt)?;
    let node_color = |i: usize| Color::new(TABLEAU10[i % TABLEAU10.len()]);

    // Links (drawn first, under the nodes).
    let mut link_els = Vec::new();
    for l in &lay.links {
        if l.width <= 0.0 {
            continue;
        }
        let (sn, tn) = (&lay.nodes[l.source], &lay.nodes[l.target]);
        let (x0, x1) = (sn.x1, tn.x0);
        let (sc, tc) = (node_color(l.source), node_color(l.target));
        let full = |c: &Color| {
            Element::classed_shape(
                crate::scene::Geometry::Path(ribbon(x0, l.y0, x1, l.y1, l.width, 0.0, 1.0)),
                Paint::fill(c).with_opacity(0.5),
                "link",
            )
        };
        match &link_color {
            LinkColor::Source => link_els.push(full(&sc)),
            LinkColor::Target => link_els.push(full(&tc)),
            LinkColor::Fixed(c) => link_els.push(full(&Color::new(c.clone()))),
            LinkColor::Gradient => {
                // Approximate the SVG gradient with colour-interpolated slices. Slices
                // are opaque, toned toward the background (≈ 50% alpha), so their
                // slight overlaps leave no seams.
                const SLICES: usize = 16;
                let mut parts = Vec::new();
                for k in 0..SLICES {
                    let (t0, t1) = (k as f64 / SLICES as f64, (k + 1) as f64 / SLICES as f64);
                    // Slight overlap hides anti-aliasing seams.
                    let t1o = (t1 + 0.004).min(1.0);
                    let x_mid = cubic_at(
                        &[
                            Point::new(x0, 0.0),
                            Point::new((x0 + x1) / 2.0, 0.0),
                            Point::new((x0 + x1) / 2.0, 0.0),
                            Point::new(x1, 0.0),
                        ],
                        (t0 + t1) / 2.0,
                    )
                    .x;
                    let f = if (x1 - x0).abs() < 1e-9 {
                        0.5
                    } else {
                        (x_mid - x0) / (x1 - x0)
                    };
                    parts.push(Element::shape(
                        crate::scene::Geometry::Path(ribbon(x0, l.y0, x1, l.y1, l.width, t0, t1o)),
                        Paint::fill(&mix(&mix(&sc, &tc, f), &pal.background, 0.5)),
                    ));
                }
                link_els.push(Element::group("link", parts));
            }
        }
    }
    els.push(Element::group("links", link_els));

    // Nodes and labels.
    let mut node_els = Vec::new();
    let mut label_els = Vec::new();
    let font = 14.0;
    let layers = lay.nodes.iter().map(|n| n.layer).max().unwrap_or(0);
    let label_room = width / layers.max(1) as f64 - node_width - 16.0;
    for (i, nd) in lay.nodes.iter().enumerate() {
        let r = Rect::new(nd.x0, nd.y0, nd.x1 - nd.x0, (nd.y1 - nd.y0).max(0.5));
        node_els.push(Element::classed_shape(
            crate::scene::Geometry::Rect { rect: r, rx: 0.0 },
            Paint::fill(&node_color(i)),
            "node",
        ));
        let value = format!(
            "{prefix}{}{suffix}",
            fmt_num((nd.value * 100.0).round() / 100.0)
        );
        let one_line = if show_values {
            format!("{} {value}", g.nodes[i])
        } else {
            g.nodes[i].clone()
        };
        // Put the value on its own line when the label would run into the next column.
        let lines = if show_values
            && line_width(&one_line, font, false) > label_room
            && nd.y1 - nd.y0 >= font * 0.5
        {
            vec![g.nodes[i].clone(), value]
        } else {
            vec![one_line]
        };
        let (x, anchor) = if nd.x0 < width / 2.0 {
            (nd.x1 + 6.0, Anchor::Start)
        } else {
            (nd.x0 - 6.0, Anchor::End)
        };
        let lh = font * 1.2;
        let t = text_at(
            x,
            (nd.y0 + nd.y1) / 2.0 - lh * (lines.len() as f64 - 1.0) / 2.0 + font * 0.35,
            &lines,
            font,
            anchor,
            &pal.text_color,
        );
        // A soft backdrop keeps labels legible where they cross ribbons.
        let tb = text_bounds(&t);
        label_els.push(Element::rect(
            tb.pad(2.0),
            3.0,
            Paint::fill(&pal.background).with_opacity(0.55),
        ));
        bounds.add_rect(text_bounds(&t));
        label_els.push(Element::classed_text(t, "node-label"));
    }
    els.push(Element::group("nodes", node_els));
    els.push(Element::group("node-labels", label_els));

    Ok(Scene {
        kind: DiagramKind::Sankey,
        view_box: bounds.rect().pad(4.0),
        elements: vec![Element::group("main", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
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

    #[test]
    fn sub_cubic_matches_curve() {
        let c: Cubic = [
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(5.0, 10.0),
            Point::new(10.0, 10.0),
        ];
        let s = sub_cubic(&c, 0.25, 0.75);
        let (a, b) = (cubic_at(&c, 0.25), cubic_at(&c, 0.75));
        assert!(s[0].dist(a) < 1e-9 && s[3].dist(b) < 1e-9);
        assert!(cubic_at(&s, 0.5).dist(cubic_at(&c, 0.5)) < 1e-9);
    }

    #[test]
    fn renders() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(SRC, None, &cfg, &pal).unwrap();
        assert_eq!(s.kind, DiagramKind::Sankey);
    }
}
