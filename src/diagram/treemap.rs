//! `treemap` (`treemap-beta`) diagrams.
//!
//! Syntax: indentation-based hierarchy of `"Section"` and `"Leaf": value`
//! (`,` is accepted instead of `:`), optional `:::class` on any item,
//! `classDef name fill:...,stroke:...,color:...` and `title ...`.
//!
//! Layout is d3's squarified treemap (golden-ratio aspect target) with
//! mermaid's paddings: sections get a 25px header plus 10px inner padding,
//! siblings are separated by `treemap.padding` (default 10). The canvas is
//! `nodeWidth*10 × nodeHeight*10` (1000 × 400 by default).

use crate::config::MermaidConfig;
use crate::diagram::graph::StyleOverrides;
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text::{decode_entities, line_width};
use crate::theme::Palette;

const SECTION_HEADER: f64 = 25.0;
const SECTION_PAD: f64 = 10.0;

/// One node of the hierarchy. Leaves have `children == None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub value: Option<f64>,
    pub class: Option<String>,
    pub children: Option<Vec<Item>>,
}

impl Item {
    /// Sum of leaf values below (or own value for a leaf).
    pub fn total(&self) -> f64 {
        match &self.children {
            Some(c) => c.iter().map(Item::total).sum(),
            None => self.value.unwrap_or(0.0).max(0.0),
        }
    }
}

/// Parsed treemap.
#[derive(Debug, Clone, PartialEq)]
pub struct Treemap {
    pub title: Option<String>,
    pub roots: Vec<Item>,
    pub classes: Vec<(String, StyleOverrides)>,
}

/// Parse a treemap body.
pub fn parse(body: &str, title: Option<String>) -> Result<Treemap> {
    let mut tm = Treemap {
        title,
        roots: Vec::new(),
        classes: Vec::new(),
    };
    let mut header = false;
    // (indent, item) in document order; hierarchy is built afterwards.
    let mut flat: Vec<(usize, Item)> = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = line
                .strip_prefix("treemap-beta")
                .or_else(|| line.strip_prefix("treemap"))
                .ok_or_else(|| Error::parse(line_no, "Expecting 'treemap'"))?;
            if !rest.trim().is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("Unexpected text after 'treemap': {}", rest.trim()),
                ));
            }
            header = true;
            continue;
        }
        if let Some(t) = keyword(line, "title") {
            tm.title = Some(t.to_string());
            continue;
        }
        if let Some(rest) = keyword(line, "classDef") {
            let (name, style) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let st = StyleOverrides::parse(style.trim().trim_end_matches(';'));
            match tm.classes.iter_mut().find(|(n, _)| n == name) {
                Some(c) => c.1 = c.1.merged(&st),
                None => tm.classes.push((name.to_string(), st)),
            }
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        flat.push((indent, parse_item(line, line_no)?));
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'treemap'"));
    }
    tm.roots = build(flat);
    Ok(tm)
}

fn keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

fn parse_item(line: &str, line_no: usize) -> Result<Item> {
    let err = |m: &str| Error::parse(line_no, format!("{line}\n{m}"));
    let q = line.chars().next().unwrap_or(' ');
    if q != '"' && q != '\'' {
        return Err(err("Expecting a quoted name, 'classDef' or 'title'"));
    }
    let close = line[1..]
        .find(q)
        .ok_or_else(|| err("Unterminated string"))?;
    let name = decode_entities(&line[1..1 + close]);
    let mut rest = line[close + 2..].trim();
    let mut class = None;
    if let Some(idx) = rest.find(":::") {
        let c = rest[idx + 3..].trim();
        if c.is_empty() {
            return Err(err("Expecting a class name after ':::'"));
        }
        class = Some(c.to_string());
        rest = rest[..idx].trim();
    }
    if rest.is_empty() {
        return Ok(Item {
            name,
            value: None,
            class,
            children: Some(Vec::new()),
        });
    }
    let v = rest
        .strip_prefix(':')
        .or_else(|| rest.strip_prefix(','))
        .ok_or_else(|| err("Expecting ': <number>'"))?
        .trim();
    // treemap.langium NUMBER2 is `[0-9_\.\,]+`: no sign, no exponent.
    if !v
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | ','))
    {
        return Err(err("Expecting a number value"));
    }
    let value = v
        .replace('_', "")
        .parse::<f64>()
        .map_err(|_| err("Expecting a number value"))?;
    Ok(Item {
        name,
        value: Some(value),
        class,
        children: None,
    })
}

/// Indentation stack → tree (mermaid's `buildHierarchy`: leaves never get children).
fn build(flat: Vec<(usize, Item)>) -> Vec<Item> {
    // Arena with parent links, then assemble bottom-up.
    let n = flat.len();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (indent, index)
    for (i, (indent, item)) in flat.iter().enumerate() {
        while stack.last().is_some_and(|&(lvl, _)| lvl >= *indent) {
            stack.pop();
        }
        parent[i] = stack.last().map(|&(_, p)| p);
        if item.children.is_some() {
            stack.push((*indent, i));
        }
    }
    let mut items: Vec<Option<Item>> = flat.into_iter().map(|(_, it)| Some(it)).collect();
    let mut roots = Vec::new();
    for i in (0..n).rev() {
        let Some(item) = items[i].take() else {
            continue;
        };
        match parent[i] {
            Some(p) => {
                if let Some(Some(pi)) = items.get_mut(p) {
                    pi.children.get_or_insert_with(Vec::new).insert(0, item);
                }
            }
            None => roots.insert(0, item),
        }
    }
    roots
}

/// A laid-out node.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub rect: Rect,
    pub name: String,
    pub value: f64,
    pub depth: usize,
    pub section: bool,
    /// Index of the colour section (the parent section for leaves).
    pub color_index: Option<usize>,
    pub class: Option<String>,
}

struct LNode<'a> {
    item: Option<&'a Item>,
    value: f64,
    children: Vec<LNode<'a>>,
}

fn lnode(item: &Item) -> LNode<'_> {
    let mut children: Vec<LNode> = item.children.iter().flatten().map(lnode).collect();
    children.sort_by(|a, b| b.value.total_cmp(&a.value));
    LNode {
        item: Some(item),
        value: item.total(),
        children,
    }
}

/// Squarified treemap layout (d3 `treemapSquarify` + mermaid paddings),
/// returned in pre-order. The virtual root is not included.
pub fn layout(tm: &Treemap, width: f64, height: f64, inner: f64) -> Vec<Cell> {
    let mut roots: Vec<LNode> = tm.roots.iter().map(lnode).collect();
    roots.sort_by(|a, b| b.value.total_cmp(&a.value));
    let root = LNode {
        item: None,
        value: roots.iter().map(|r| r.value).sum(),
        children: roots,
    };
    let mut out = Vec::new();
    let mut section_counter = 0usize;
    // Section colour indices follow breadth-first order like mermaid's ordinal scale.
    let mut colors: Vec<(*const Item, usize)> = Vec::new();
    let mut queue: Vec<&LNode> = vec![&root];
    while !queue.is_empty() {
        let mut next = Vec::new();
        for n in queue {
            for c in &n.children {
                if !c.children.is_empty() {
                    if let Some(it) = c.item {
                        colors.push((it as *const Item, section_counter));
                        section_counter += 1;
                    }
                }
                next.push(c);
            }
        }
        queue = next;
    }
    let color_of = |it: Option<&Item>| {
        it.and_then(|it| {
            colors
                .iter()
                .find(|(p, _)| std::ptr::eq(*p, it))
                .map(|(_, i)| *i)
        })
    };
    position(
        &root,
        Rect::new(0.0, 0.0, width, height),
        0.0,
        inner,
        0,
        None,
        &color_of,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn position(
    n: &LNode,
    r: Rect,
    pad: f64,
    inner: f64,
    depth: usize,
    parent: Option<&Item>,
    color_of: &dyn Fn(Option<&Item>) -> Option<usize>,
    out: &mut Vec<Cell>,
) {
    let (mut x0, mut y0, mut x1, mut y1) =
        (r.x + pad, r.y + pad, r.right() - pad, r.bottom() - pad);
    if x1 < x0 {
        x0 = (x0 + x1) / 2.0;
        x1 = x0;
    }
    if y1 < y0 {
        y0 = (y0 + y1) / 2.0;
        y1 = y0;
    }
    let is_section = !n.children.is_empty();
    if let Some(it) = n.item {
        let rect = Rect::new(x0.round(), y0.round(), 0.0, 0.0);
        let rect = Rect::new(rect.x, rect.y, x1.round() - rect.x, y1.round() - rect.y);
        out.push(Cell {
            rect,
            name: it.name.clone(),
            value: n.value,
            depth,
            section: is_section,
            color_index: if is_section {
                color_of(Some(it))
            } else {
                color_of(parent)
            },
            class: it.class.clone(),
        });
    }
    if !is_section {
        return;
    }
    let p = inner / 2.0;
    let (mut cx0, mut cy0, mut cx1, mut cy1) = (
        x0 + SECTION_PAD - p,
        y0 + SECTION_HEADER + SECTION_PAD - p,
        x1 - (SECTION_PAD - p),
        y1 - (SECTION_PAD - p),
    );
    if cx1 < cx0 {
        cx0 = (cx0 + cx1) / 2.0;
        cx1 = cx0;
    }
    if cy1 < cy0 {
        cy0 = (cy0 + cy1) / 2.0;
        cy1 = cy0;
    }
    let values: Vec<f64> = n.children.iter().map(|c| c.value).collect();
    let rects = squarify(&values, Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0));
    for (c, cr) in n.children.iter().zip(rects) {
        position(c, cr, p, inner, depth + 1, n.item, color_of, out);
    }
}

/// d3's `squarifyRatio(phi)`: tile `area` with rectangles proportional to `values`
/// (assumed sorted descending), keeping aspect ratios close to the golden ratio.
pub fn squarify(values: &[f64], area: Rect) -> Vec<Rect> {
    let ratio = (1.0 + 5f64.sqrt()) / 2.0;
    let n = values.len();
    let mut out = vec![Rect::new(area.x, area.y, 0.0, 0.0); n];
    let (mut x0, mut y0, x1, y1) = (area.x, area.y, area.right(), area.bottom());
    let mut value: f64 = values.iter().sum();
    let (mut i0, mut i1) = (0usize, 0usize);
    while i0 < n {
        if value <= 0.0 {
            break; // remaining (zero-valued) children keep empty rects
        }
        let (dx, dy) = (x1 - x0, y1 - y0);
        let mut sum;
        loop {
            sum = values[i1];
            i1 += 1;
            if sum > 0.0 || i1 >= n {
                break;
            }
        }
        let (mut min_v, mut max_v) = (sum, sum);
        let alpha = (dy / dx).max(dx / dy) / (value * ratio);
        let mut beta = sum * sum * alpha;
        let mut min_ratio = (max_v / beta).max(beta / min_v);
        while i1 < n {
            let v = values[i1];
            sum += v;
            min_v = min_v.min(v);
            max_v = max_v.max(v);
            beta = sum * sum * alpha;
            let new_ratio = (max_v / beta).max(beta / min_v);
            if new_ratio > min_ratio || !new_ratio.is_finite() {
                sum -= v;
                break;
            }
            min_ratio = new_ratio;
            i1 += 1;
        }
        let dice = dx < dy;
        if dice {
            // Row spans the full width; children laid out left to right.
            let ny = if value > 0.0 {
                y0 + dy * sum / value
            } else {
                y1
            };
            let mut x = x0;
            for i in i0..i1 {
                let w = if sum > 0.0 {
                    (x1 - x0) * values[i] / sum
                } else {
                    0.0
                };
                out[i] = Rect::new(x, y0, w, ny - y0);
                x += w;
            }
            y0 = ny;
        } else {
            let nx = if value > 0.0 {
                x0 + dx * sum / value
            } else {
                x1
            };
            let mut y = y0;
            for i in i0..i1 {
                let h = if sum > 0.0 {
                    (y1 - y0) * values[i] / sum
                } else {
                    0.0
                };
                out[i] = Rect::new(x0, y, nx - x0, h);
                y += h;
            }
            x0 = nx;
        }
        value -= sum;
        i0 = i1;
    }
    out
}

/// d3-format subset: optional `$` prefix, `,` grouping, `.N` precision, `f`/`%`/`d`.
pub fn format_value(v: f64, spec: &str) -> String {
    let mut s = spec.trim();
    let dollar = s.starts_with('$');
    if dollar {
        s = &s[1..];
    }
    if s == "0,0" {
        s = ",";
    }
    let group = s.contains(',');
    let percent = s.ends_with('%');
    let precision = s.find('.').and_then(|i| {
        s[i + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<usize>()
            .ok()
    });
    let x = if percent { v * 100.0 } else { v };
    let mut num = match precision {
        Some(p) => format!("{:.*}", p, x.abs()),
        None if x.fract().abs() < 1e-9 => format!("{}", x.abs().round()),
        None => {
            let t = format!("{:.6}", x.abs());
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    };
    if group {
        let (int, frac) = match num.find('.') {
            Some(i) => (num[..i].to_string(), num[i..].to_string()),
            None => (num.clone(), String::new()),
        };
        let mut g = String::new();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(c);
        }
        num = g + &frac;
    }
    format!(
        "{}{}{}{}",
        if x < 0.0 { "-" } else { "" },
        if dollar { "$" } else { "" },
        num,
        if percent { "%" } else { "" }
    )
}

/// Render a treemap.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let tm = parse(body, title)?;
    let width = cfg.section_f64("treemap", "nodeWidth", 100.0) * 10.0;
    let height = cfg.section_f64("treemap", "nodeHeight", 40.0) * 10.0;
    let inner = cfg.section_f64("treemap", "padding", 10.0);
    let show_values = cfg.section_bool("treemap", "showValues", true);
    let fmt = cfg
        .section("treemap", "valueFormat")
        .and_then(|j| j.as_str())
        .unwrap_or(",")
        .to_string();
    let pad = cfg.section_f64("treemap", "diagramPadding", 8.0);
    Ok(draw(
        &tm,
        &layout(&tm, width, height, inner),
        width,
        show_values,
        &fmt,
        pad,
        pal,
    ))
}

/// Draw laid-out cells.
pub fn draw(
    tm: &Treemap,
    cells: &[Cell],
    width: f64,
    show_values: bool,
    fmt: &str,
    pad: f64,
    pal: &Palette,
) -> Scene {
    let title_h = if tm.title.as_ref().is_some_and(|t| !t.is_empty()) {
        30.0
    } else {
        0.0
    };
    let mut bounds = Bounds::default();
    let mut els = Vec::new();
    if let Some(t) = tm.title.as_ref().filter(|t| !t.is_empty()) {
        let c = Point::new(width / 2.0, title_h / 2.0);
        bounds.add_rect(Rect::centered(c, line_width(t, 14.0, false), 20.0));
        els.push(Element::classed_text(
            Text::centered(c, std::slice::from_ref(t), 14.0, 21.0, &pal.title_color),
            "treemapTitle",
        ));
    }
    let class_style = |c: &Option<String>| {
        c.as_ref()
            .and_then(|c| tm.classes.iter().find(|(n, _)| n == c))
            .map(|(_, s)| s.clone())
            .unwrap_or_default()
    };
    // mermaid's ordinal scales: `colorScale` / `colorScalePeer` range over
    // `['transparent', cScale0..11]` and see the root first, so section k (breadth-first)
    // is entry k + 1 (mod 13); `colorScaleLabel` ranges over `cScaleLabel0..11` and sees
    // the sections first, then the leaves in order (keyed by name).
    let ordinal = |set: &[Color], i: Option<usize>| -> Option<Color> {
        let k = (i? + 1) % (set.len() + 1);
        (k > 0).then(|| set[k - 1].clone())
    };
    let transparent = Color::new("transparent");
    let mut label_domain: Vec<&str> = Vec::new();
    let mut sorted: Vec<&Cell> = cells.iter().filter(|c| c.section).collect();
    sorted.sort_by_key(|c| c.color_index);
    for c in sorted.iter().chain(
        cells
            .iter()
            .filter(|c| !c.section)
            .collect::<Vec<_>>()
            .iter(),
    ) {
        if !label_domain.contains(&c.name.as_str()) {
            label_domain.push(&c.name);
        }
    }
    let label_of = |name: &str| -> Color {
        let i = label_domain.iter().position(|n| *n == name).unwrap_or(0);
        pal.scale_text_color(i).clone()
    };
    let mut sections = Vec::new();
    let mut leaves = Vec::new();
    for cell in cells {
        let r = Rect::new(cell.rect.x, cell.rect.y + title_h, cell.rect.w, cell.rect.h);
        bounds.add_rect(r);
        let st = class_style(&cell.class);
        let base = ordinal(&pal.scale, cell.color_index).unwrap_or_else(|| transparent.clone());
        let label_color = st.color.clone().unwrap_or_else(|| label_of(&cell.name));
        if cell.section {
            let fill = st.fill.clone().unwrap_or_else(|| base.clone());
            let stroke = st.stroke.clone().unwrap_or_else(|| {
                ordinal(&pal.scale_peer, cell.color_index).unwrap_or_else(|| transparent.clone())
            });
            let mut g = vec![
                Element::classed_shape(
                    crate::scene::Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::fill(&fill).with_opacity(0.6),
                    "treemapSection",
                ),
                Element::classed_shape(
                    crate::scene::Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::stroke(&stroke, st.stroke_width.unwrap_or(2.0)).with_opacity(0.4),
                    "treemapSectionBorder",
                ),
            ];
            let value = if show_values && cell.value > 0.0 {
                format_value(cell.value, fmt)
            } else {
                String::new()
            };
            let value_w = line_width(&value, 10.0, false);
            let avail = if value.is_empty() {
                r.w - 12.0
            } else {
                r.w - 16.0 - value_w - 10.0
            };
            let name = ellipsize(&cell.name, 12.0, true, avail.max(15.0));
            let cy = r.y + SECTION_HEADER / 2.0;
            if !name.is_empty() {
                g.push(Element::classed_text(
                    Text::centered(Point::new(0.0, cy), &[name], 12.0, 18.0, &label_color)
                        .bold()
                        .anchored(Anchor::Start, r.x + 6.0),
                    "treemapSectionLabel",
                ));
            }
            if !value.is_empty() && r.w > value_w + 20.0 {
                g.push(Element::classed_text(
                    Text::centered(Point::new(0.0, cy), &[value], 10.0, 15.0, &label_color)
                        .anchored(Anchor::End, r.right() - 10.0),
                    "treemapSectionValue",
                ));
            }
            sections.push(Element::group("treemapSection", g));
        } else {
            let fill = st.fill.clone().unwrap_or_else(|| base.clone());
            let stroke = st.stroke.clone().unwrap_or_else(|| base.clone());
            let mut g = vec![
                Element::classed_shape(
                    crate::scene::Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::fill(&fill).with_opacity(0.3),
                    "treemapLeaf",
                ),
                Element::classed_shape(
                    crate::scene::Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::stroke(&stroke, st.stroke_width.unwrap_or(3.0)),
                    "treemapLeafBorder",
                ),
            ];
            g.extend(leaf_labels(cell, r, show_values, fmt, &label_color));
            let class = match &cell.class {
                Some(c) => format!("treemapNode treemapLeafGroup {c}"),
                None => "treemapNode treemapLeafGroup".to_string(),
            };
            leaves.push(Element::group(class, g));
        }
    }
    els.extend(sections);
    els.extend(leaves);
    Scene {
        kind: DiagramKind::Treemap,
        view_box: bounds.rect().pad(pad),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// mermaid's leaf label fitting: start at 38px, shrink to fit (min 8px), value
/// at 60% of the label size below it; hide what still does not fit.
fn leaf_labels(cell: &Cell, r: Rect, show_values: bool, fmt: &str, color: &Color) -> Vec<Element> {
    let avail_w = r.w - 8.0;
    let avail_h = r.h - 8.0;
    if avail_w < 10.0 || avail_h < 10.0 {
        return Vec::new();
    }
    let value = if show_values && cell.value > 0.0 {
        format_value(cell.value, fmt)
    } else {
        String::new()
    };
    let value_size = |f: f64| (f * 0.6).round().clamp(6.0, 28.0);
    let mut font: f64 = 38.0;
    while line_width(&cell.name, font, false) > avail_w && font > 8.0 {
        font -= 1.0;
    }
    let combined = |f: f64| {
        if value.is_empty() {
            f
        } else {
            f + 2.0 + value_size(f)
        }
    };
    while combined(font) > avail_h && font > 8.0 {
        font -= 1.0;
    }
    if line_width(&cell.name, font, false) > avail_w || avail_h < font {
        return Vec::new();
    }
    let mut out = Vec::new();
    let c = r.center();
    let vs = value_size(font);
    let show_value =
        !value.is_empty() && line_width(&value, vs, false) <= avail_w && combined(font) <= avail_h;
    let label_cy = if show_value {
        c.y - (vs + 2.0) / 2.0
    } else {
        c.y
    };
    out.push(Element::classed_text(
        Text::centered(
            Point::new(c.x, label_cy),
            std::slice::from_ref(&cell.name),
            font,
            font,
            color,
        ),
        "treemapLabel",
    ));
    if show_value {
        out.push(Element::classed_text(
            Text::centered(
                Point::new(c.x, label_cy + font / 2.0 + 2.0 + vs / 2.0),
                &[value],
                vs,
                vs,
                color,
            ),
            "treemapValue",
        ));
    }
    out
}

fn ellipsize(s: &str, font: f64, bold: bool, max: f64) -> String {
    if line_width(s, font, bold) <= max {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    for n in (0..chars.len()).rev() {
        let t: String = chars[..n].iter().collect::<String>() + "...";
        if line_width(&t, font, bold) <= max {
            return if n == 0 { String::new() } else { t };
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "treemap-beta\n\"Section 1\"\n    \"Leaf 1.1\": 12\n    \"Section 1.2\":::hot\n      \"Leaf 1.2.1\": 12\n\"Section 2\"\n    \"Leaf 2.1\": 20:::hot\n    \"Leaf 2.2\", 25\nclassDef hot fill:#f96,color:#000";

    #[test]
    fn parses_hierarchy() {
        let t = parse(SRC, None).unwrap();
        assert_eq!(t.roots.len(), 2);
        let s1 = &t.roots[0];
        assert_eq!(s1.name, "Section 1");
        let kids = s1.children.as_ref().unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[1].class.as_deref(), Some("hot"));
        assert_eq!(kids[1].children.as_ref().unwrap()[0].value, Some(12.0));
        assert_eq!(s1.total(), 24.0);
        assert_eq!(t.roots[1].total(), 45.0);
        assert_eq!(
            t.roots[1].children.as_ref().unwrap()[0].class.as_deref(),
            Some("hot")
        );
        assert_eq!(t.classes.len(), 1);
    }

    #[test]
    fn parse_errors_have_lines() {
        let e = parse("treemap\n\"a\"\n  \"b\": x", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("treemap\n\n\"open", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("treemap\nfoo", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
    }

    #[test]
    fn squarify_conserves_area() {
        let vals = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let rs = squarify(&vals, area);
        let total: f64 = rs.iter().map(|r| r.w * r.h).sum();
        assert!((total - 240_000.0).abs() < 1e-6);
        for (r, v) in rs.iter().zip(vals) {
            assert!((r.w * r.h - v / 24.0 * 240_000.0).abs() < 1e-6);
        }
        assert_no_overlap(&rs);
    }

    fn assert_no_overlap(rs: &[Rect]) {
        for (i, a) in rs.iter().enumerate() {
            for b in &rs[i + 1..] {
                let ox = a.x.max(b.x) < a.right().min(b.right()) - 1e-6;
                let oy = a.y.max(b.y) < a.bottom().min(b.bottom()) - 1e-6;
                assert!(!(ox && oy), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn layout_nests_without_overlap() {
        let t = parse(SRC, None).unwrap();
        let cells = layout(&t, 1000.0, 400.0, 10.0);
        assert_eq!(cells.len(), 7);
        let leaves: Vec<Rect> = cells
            .iter()
            .filter(|c| !c.section)
            .map(|c| c.rect)
            .collect();
        assert_eq!(leaves.len(), 4);
        assert_no_overlap(&leaves);
        let tops: Vec<Rect> = cells
            .iter()
            .filter(|c| c.depth == 1)
            .map(|c| c.rect)
            .collect();
        assert_no_overlap(&tops);
        // Every cell lies within the canvas and children within their section.
        for c in &cells {
            assert!(c.rect.x >= 0.0 && c.rect.right() <= 1000.0);
            assert!(c.rect.y >= 0.0 && c.rect.bottom() <= 400.0);
        }
        let s2 = cells.iter().find(|c| c.name == "Section 2").unwrap();
        let l = cells.iter().find(|c| c.name == "Leaf 2.2").unwrap();
        assert!(l.rect.x >= s2.rect.x && l.rect.right() <= s2.rect.right());
        assert!(l.rect.y >= s2.rect.y + SECTION_HEADER);
        // Larger section first.
        assert_eq!(cells[0].name, "Section 2");
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_value(1234567.0, ","), "1,234,567");
        assert_eq!(format_value(1234.5, "$,.2f"), "$1,234.50");
        assert_eq!(format_value(0.25, ".0%"), "25%");
        assert_eq!(format_value(42.0, "$0,0"), "$42");
    }
}
