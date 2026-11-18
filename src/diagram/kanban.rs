//! `kanban` diagrams.
//!
//! Columns are the least-indented nodes, items are the nodes indented below
//! them. Items may carry `@{ ticket: X, assigned: 'name', priority: 'High' }`
//! metadata. Rendering follows mermaid's kanban look (coloured section columns,
//! white cards with ticket / assignee footer and a priority stripe), laid out as
//! `kanbanRenderer.ts` does: fixed-width columns, each as tall as its own cards.

use super::timeline::{shade, wrap_text};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{line_width, normalize_label};
use crate::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    VeryHigh,
    High,
    Medium,
    Low,
    VeryLow,
}

impl Priority {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "Very High" => Priority::VeryHigh,
            "High" => Priority::High,
            "Medium" => Priority::Medium,
            "Low" => Priority::Low,
            "Very Low" => Priority::VeryLow,
            _ => return None,
        })
    }
    fn color(self) -> Option<Color> {
        Some(Color::new(match self {
            Priority::VeryHigh => "red",
            Priority::High => "orange",
            Priority::Medium => return None,
            Priority::Low => "blue",
            Priority::VeryLow => "lightblue",
        }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub ticket: Option<String>,
    pub assigned: Option<String>,
    pub priority: Option<Priority>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub id: String,
    pub label: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Kanban {
    pub title: Option<String>,
    pub columns: Vec<Column>,
}

/// Split `id[label]` / `id(label)` / `[label]` / `plain text` into `(id, label)`.
fn parse_node(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let Some(open) = text.find(['[', '(', '{', '>']) else {
        return Some((text.to_string(), text.to_string()));
    };
    let id = text[..open].trim().to_string();
    let inner = &text[open..];
    let opens: &[char] = &['[', '(', '{', '>'];
    let closes: &[char] = &[']', ')', '}'];
    let body = inner.trim_start_matches(opens);
    let close = body.rfind(closes)?;
    // Nothing may follow the closing delimiter (kanban.jison expects a newline).
    if !body[close + 1..].trim().is_empty() {
        return None;
    }
    let body = body[..close].trim_end_matches(closes);
    let label = normalize_label(body);
    let id = if id.is_empty() { label.clone() } else { id };
    Some((id, crate::icons::replace_references(&label)))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['\'', '"'] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Parse `key: value, key: 'value'` pairs.
fn parse_metadata(s: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut parts = Vec::new();
    for c in s.chars() {
        match (c, quote) {
            ('\'' | '"', None) => {
                quote = Some(c);
                cur.push(c);
            }
            (q, Some(open)) if q == open => {
                quote = None;
                cur.push(c);
            }
            (',' | '\n', None) => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        // A bare `key` is a flow-mapping key with a null value for js-yaml (or a
        // plain scalar document): valid, and it sets nothing.
        let Some((k, v)) = p.split_once(':') else {
            if p.starts_with(['\'', '"']) && !(p.len() > 1 && p.ends_with(&p[..1])) {
                return None;
            }
            continue;
        };
        out.push((k.trim().to_string(), unquote(v)));
    }
    Some(out)
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Parse a kanban body.
pub fn parse(body: &str, title: Option<String>) -> Result<Kanban> {
    let mut k = Kanban {
        title,
        columns: Vec::new(),
    };
    let lines: Vec<&str> = body.lines().collect();
    let mut header = false;
    let mut column_indent: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line_no = i + 1;
        let raw = lines[i];
        i += 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            if line != "kanban" {
                return Err(Error::parse(line_no, format!("{line}\nExpecting 'kanban'")));
            }
            header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            k.title = Some(t.trim().to_string());
            continue;
        }
        // Gather a possibly multi-line `@{ ... }` block.
        let mut stmt = line.to_string();
        if let Some(at) = stmt.find("@{") {
            while !stmt[at..].contains('}') {
                if i >= lines.len() {
                    return Err(Error::parse(line_no, format!("{line}\nUnterminated '@{{'")));
                }
                stmt.push('\n');
                stmt.push_str(lines[i].trim());
                i += 1;
            }
        }
        let (node_txt, meta) = match stmt.find("@{") {
            Some(at) => {
                let close = stmt[at..].find('}').map_or(stmt.len(), |c| at + c);
                if !stmt[close + 1..].trim().is_empty() {
                    return Err(Error::parse(
                        line_no,
                        format!("{line}\nUnexpected text after metadata"),
                    ));
                }
                let m = parse_metadata(&stmt[at + 2..close]).ok_or_else(|| {
                    Error::parse(
                        line_no,
                        format!("{line}\nInvalid metadata: expected 'key: value' pairs"),
                    )
                })?;
                (stmt[..at].to_string(), m)
            }
            None => (stmt.clone(), Vec::new()),
        };
        let (id, label) = parse_node(&node_txt)
            .ok_or_else(|| Error::parse(line_no, format!("{line}\nUnterminated node shape")))?;
        if label.is_empty() && meta.is_empty() {
            return Err(Error::parse(line_no, format!("{line}\nEmpty node")));
        }
        let ind = indent_of(raw);
        let is_column = match column_indent {
            None => true,
            Some(ci) => ind <= ci,
        };
        if is_column {
            column_indent = Some(column_indent.map_or(ind, |c| c.min(ind)));
            k.columns.push(Column {
                id,
                label,
                items: Vec::new(),
            });
            continue;
        }
        let mut item = Item {
            id,
            label,
            ticket: None,
            assigned: None,
            priority: None,
        };
        for (key, v) in meta {
            match key.as_str() {
                "ticket" => item.ticket = Some(v),
                "assigned" => item.assigned = Some(v),
                // kanbanItem.ts colorFromPriority: an unknown (or differently cased)
                // value draws no stripe.
                "priority" => item.priority = Priority::parse(&v),
                "label" => item.label = v,
                _ => {}
            }
        }
        if let Some(col) = k.columns.last_mut() {
            col.items.push(item);
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'kanban'"));
    }
    Ok(k)
}

/// Lay out and draw a parsed board: a port of `kanbanRenderer.ts` and the
/// `kanbanItem` shape.
pub fn draw(k: &Kanban, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let col_w = cfg.section_f64("kanban", "sectionWidth", 200.0);
    let linked_tickets = cfg
        .section("kanban", "ticketBaseUrl")
        .and_then(|j| j.as_str())
        .is_some_and(|u| !u.is_empty());
    let font = pal.font_size;
    let lh = font * 1.5;
    let padding = 10.0;
    // Items are `sectionWidth - 1.5·padding` wide, their title wraps 10px narrower.
    let item_w = col_w - 1.5 * padding;
    let text_w = item_w - 10.0;

    let col_titles: Vec<Vec<String>> = k
        .columns
        .iter()
        .map(|c| crate::text::label_lines(&normalize_label(&c.label)))
        .collect();
    let max_label_h = col_titles
        .iter()
        .map(|l| l.len() as f64 * lh)
        .fold(25.0, f64::max);

    let mut bounds = Bounds::default();
    let mut els = Vec::new();
    let card_fill = pal.background.clone();
    let dark_mode = cfg
        .theme_variables
        .iter()
        .rev()
        .find(|(k, _)| k == "darkMode")
        .is_some_and(|(_, v)| v.trim() == "true");
    let card_text = pal.text_color.clone();
    let top = -col_w * 3.0 / 2.0 + max_label_h;
    let hand_seed = (cfg.look == crate::config::Look::HandDrawn).then(|| cfg.hand_drawn_seed());
    for (ci, col) in k.columns.iter().enumerate() {
        let cnt = (ci + 1) as f64;
        let cx = col_w * cnt + (cnt - 1.0) * padding / 2.0;
        let mut g = Vec::new();
        let mut y = top;
        for item in &col.items {
            let lines = wrap_text(&item.label, text_w, font, false);
            let bh = lines.len().max(1) as f64 * lh;
            let has_footer = item.ticket.is_some() || item.assigned.is_some();
            let adj = if has_footer { lh / 2.0 } else { 0.0 };
            let h = bh + 2.0 * padding + adj;
            let cy = y + h / 2.0;
            let r = Rect::new(cx - item_w / 2.0, y, item_w, h);
            bounds.add_rect(r);
            g.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 5.0 },
                Paint::both(&card_fill, &pal.node_border, 1.0),
                "kanban-item",
            ));
            if let Some(c) = item.priority.and_then(Priority::color) {
                g.push(Element::line(
                    Point::new(r.x + 2.0, r.y + 2.0),
                    Point::new(r.x + 2.0, r.bottom() - 2.0),
                    Paint::stroke(&c, 4.0),
                ));
            }
            let lx = r.x + padding;
            let label_top = cy - adj - bh / 2.0;
            g.push(Element::classed_text(
                Text::centered(
                    Point::new(0.0, label_top + bh / 2.0),
                    &lines,
                    font,
                    lh,
                    &card_text,
                )
                .anchored(Anchor::Start, lx),
                "kanban-label",
            ));
            let fy = cy - adj + bh / 2.0 + lh / 2.0;
            if let Some(t) = &item.ticket {
                let text = Text::centered(
                    Point::new(0.0, fy),
                    std::slice::from_ref(t),
                    font,
                    lh,
                    &card_text,
                )
                .anchored(Anchor::Start, lx);
                let baseline = text.y;
                g.push(Element::classed_text(text, "kanban-ticket"));
                if linked_tickets {
                    // An HTML label underlined inside the `.kanban-ticket-link` anchor:
                    // Chromium paints the 1px underline on the pixel row just below
                    // the one under the baseline (measured at 16px), in the anchor's
                    // `color`, the page's black: the label's `#333` is only its fill.
                    // As wide as the HTML text: its advance in layout units.
                    let w = crate::text::html_line_width(t, font, false);
                    let base = baseline + 1.5;
                    g.push(Element::line(
                        Point::new(lx, base),
                        Point::new(lx + w, base),
                        Paint::stroke(&Color::new("#000000"), 1.0),
                    ));
                }
            }
            if let Some(a) = &item.assigned {
                let a = normalize_label(a);
                let tw = line_width(&a, font, false);
                let ax = padding + r.right() - tw - 2.0 * padding;
                bounds.add_rect(Rect::new(ax, fy - lh / 2.0, tw, lh));
                g.push(Element::classed_text(
                    Text::centered(Point::new(0.0, fy), &[a], font, lh, &card_text)
                        .anchored(Anchor::Start, ax),
                    "kanban-assigned",
                ));
            }
            y = cy + h / 2.0 + padding / 2.0;
        }
        // Column `ci` is `.section-(ci+1)`: fill and stroke `cScale(ci+2)` lightened by
        // 10 (darkened under `darkMode`); the title is an HTML label in regular weight,
        // coloured by `.cluster-label, .label { color: textColor }`, at the top centre.
        let height = (y - top + 3.0 * padding).max(50.0) + (max_label_h - 25.0);
        let cr = Rect::new(cx - col_w / 2.0, -col_w * 3.0 / 2.0, col_w, height);
        let fill = shade(pal.scale_color(ci + 2), !dark_mode, 10.0);
        let frame = hand_seed.map(|seed| hand_section(cx, col_w, seed, pal, &fill));
        match &frame {
            Some((_, b)) => bounds.add_rect(*b),
            None => bounds.add_rect(cr),
        }
        let title_h = col_titles[ci].len() as f64 * lh;
        g.insert(
            0,
            Element::classed_text(
                Text::centered(
                    Point::new(cx, cr.y + title_h / 2.0),
                    &col_titles[ci],
                    font,
                    lh,
                    &pal.text_color,
                ),
                "kanban-section-title",
            ),
        );
        let section = match frame {
            Some((el, _)) => el,
            None => Element::classed_shape(
                Geometry::Rect { rect: cr, rx: 5.0 },
                Paint::both(&fill, &fill, 1.0),
                "kanban-section",
            ),
        };
        g.insert(0, section);
        els.push(Element::group("kanban-column", g));
    }
    // setupGraphViewbox with the (mindmap) padding, 10.
    let pad = cfg.section_f64("mindmap", "padding", 10.0);
    Scene {
        kind: DiagramKind::Kanban,
        view_box: bounds.rect().pad(pad),
        elements: vec![Element::group("kanban", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// clusters.js `kanbanSection` under the handDrawn look: the section as a
/// rough path of its rounded rectangle (`rx` 5) at its first size,
/// `sectionWidth` × 3·`sectionWidth` round (cx, 0) (kanbanRenderer then
/// looks for a `rect` to shorten and finds none), hachured at `fillWeight`
/// 4; the section stylesheet fills and strokes both paths in its colour.
/// Returns the element and the drawing's box.
fn hand_section(cx: f64, w: f64, seed: f64, pal: &Palette, fill: &Color) -> (Element, Rect) {
    let h = w * 3.0;
    let (x, y) = (cx - w / 2.0, 0.0 - h / 2.0);
    let o = crate::rough::Options {
        roughness: 0.7,
        fill: Some(pal.cluster_bkg.css().to_string()),
        fill_style: "hachure".into(),
        fill_weight: 4.0,
        hachure_gap: 1.5,
        stroke: pal.cluster_border.css().to_string(),
        seed,
        stroke_width: 1.3,
        ..Default::default()
    };
    let d = crate::rough::path(&super::hand::rounded_rect_path(x, y, w, h, 5.0), o);
    let mut kids = Vec::new();
    let mut paths = Vec::new();
    for set in d.sets.iter().filter(|s| !s.ops.is_empty()) {
        let cmds = super::hand::set_cmds(&set.ops, Point::default());
        let width = match set.kind {
            crate::rough::SetKind::FillSketch => d.fill_weight(),
            _ => d.options.stroke_width,
        };
        paths.push(cmds.clone());
        kids.push(Element::shape(
            Geometry::Path(cmds),
            Paint::both(fill, fill, width),
        ));
    }
    let b = super::svgbbox::cubic_group_box(&paths)
        .map_or(Rect::new(x, y, w, h), |[x, y, w, h]| Rect::new(x, y, w, h));
    (Element::group("kanban-section", kids), b)
}

/// Parse and render a `kanban` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let k = parse(body, title)?;
    Ok(draw(&k, cfg, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "kanban
  Todo
    [Create Documentation]
    docs[Create Blog about the new diagram]
  [In progress]
    id6[Create renderer]
  id10[Ready for test]
    id4[Create parsing tests]@{ ticket: MC-2038, assigned: 'K.Sveidqvist', priority: 'High' }
    id66[last item]@{ priority: 'Very Low', assigned: \"knsv, jr\" }
  id11[Done]
";

    #[test]
    fn parses_columns_items_and_metadata() {
        let k = parse(SRC, None).unwrap();
        let names: Vec<&str> = k.columns.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(names, vec!["Todo", "In progress", "Ready for test", "Done"]);
        assert_eq!(k.columns[0].items.len(), 2);
        assert_eq!(k.columns[0].items[1].id, "docs");
        let it = &k.columns[2].items[0];
        assert_eq!(it.ticket.as_deref(), Some("MC-2038"));
        assert_eq!(it.assigned.as_deref(), Some("K.Sveidqvist"));
        assert_eq!(it.priority, Some(Priority::High));
        assert_eq!(k.columns[2].items[1].assigned.as_deref(), Some("knsv, jr"));
        assert_eq!(k.columns[2].items[1].priority, Some(Priority::VeryLow));
        assert!(k.columns[3].items.is_empty());
    }

    #[test]
    fn multiline_metadata() {
        let k = parse(
            "kanban\n  c[Col]\n    a[A]@{\n      ticket: T-1,\n      priority: 'Low'\n    }\n",
            None,
        )
        .unwrap();
        assert_eq!(k.columns[0].items[0].ticket.as_deref(), Some("T-1"));
        assert_eq!(k.columns[0].items[0].priority, Some(Priority::Low));
    }

    #[test]
    fn errors_have_line_numbers() {
        // Unknown priorities and bare keys are accepted upstream (no stripe, no value).
        let k = parse("kanban\n  c[Col]\n    a[A]@{ priority: 'Urgent' }", None).unwrap();
        assert_eq!(k.columns[0].items[0].priority, None);
        let k = parse(
            "kanban\n  c[Col]\n\n    a[A]@{ ticket }\n    b@{ bogus }",
            None,
        )
        .unwrap();
        assert_eq!(k.columns[0].items.len(), 2);
        assert_eq!(k.columns[0].items[0].ticket, None);
        let e = parse("kanban\n  c[Col]\n    a[A]@{ \"ticket }", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e}");
        let e = parse("kanban\n  c[Col]\n    a[A]@{ ticket: 1", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        assert!(parse("kanban\n  c[Col\n", None).is_err());
        assert!(parse("flowchart", None).is_err());
    }

    #[test]
    fn columns_fit_their_cards() {
        // kanbanRenderer: each column is its cards (title + 2·padding tall, 5
        // apart) plus 3·padding, at least 50; mermaid 12.3.0's gallery board
        // matches its viewBox exactly.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let k = parse(SRC, None).unwrap();
        let s = draw(&k, &cfg, &pal);
        let heights: Vec<f64> = match &s.elements[0] {
            Element::Group { children, .. } => children
                .iter()
                .filter_map(|c| match c {
                    Element::Group { children, .. } => match &children[0] {
                        Element::Shape {
                            geom: Geometry::Rect { rect, .. },
                            ..
                        } => Some(rect.h),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        assert_eq!(heights.len(), 4);
        assert!(heights.iter().all(|h| *h >= 50.0));
        let src = include_str!("../../examples/diagrams/kanban.mmd");
        // Past the front matter (its `ticketBaseUrl` only underlines tickets).
        let src = src.splitn(3, "---").nth(2).unwrap();
        let g = draw(&parse(src, None).unwrap(), &cfg, &pal).view_box;
        assert_eq!((g.x, g.y, g.w, g.h), (90.0, -310.0, 1245.0, 293.0));
    }

    /// Every text and shape of a full render, groups flattened.
    fn flat(src: &str) -> (crate::scene::Scene, Vec<crate::scene::Element>) {
        fn walk(els: &[crate::scene::Element], out: &mut Vec<crate::scene::Element>) {
            for e in els {
                match e {
                    crate::scene::Element::Group { children, .. }
                    | crate::scene::Element::Rotated { children, .. } => walk(children, out),
                    other => out.push(other.clone()),
                }
            }
        }
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        let mut out = Vec::new();
        walk(&r.scene.elements, &mut out);
        (r.scene, out)
    }

    #[test]
    fn assignee_and_ticket_link_paint() {
        // mermaid 12.3.0: the assignee is a plain `.label` (#333); a linked
        // ticket's underline is the anchor's black text-decoration, on the pixel
        // row from baseline + 1 (a 1px line centred 1.5 below the baseline).
        let src = "---\nconfig:\n  kanban:\n    ticketBaseUrl: 'https://x/#TICKET#'\n---\nkanban\n  c[Col]\n    a[Task]@{ ticket: MC-1, assigned: 'knsv' }\n";
        let (_, els) = flat(src);
        let text = |cls: &str| {
            els.iter()
                .find_map(|e| match e {
                    Element::Text {
                        text,
                        class: Some(c),
                    } if c == cls => Some(text.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(text("kanban-assigned").fill.css(), "#333");
        let ticket = text("kanban-ticket");
        let underline = els
            .iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(c),
                    paint,
                    class: None,
                } if paint.stroke_width == 1.0 && c.len() == 2 => Some((c.clone(), paint.clone())),
                _ => None,
            })
            .unwrap();
        let crate::geom::PathCmd::MoveTo(a) = underline.0[0] else {
            panic!()
        };
        assert_eq!(a.y, ticket.y + 1.5);
        assert_eq!(underline.1.stroke.css(), "#000000");
    }

    /// mermaid 12.3.0 under `look: handDrawn` (seed 17): each section is a
    /// rough path 600px tall (kanbanRenderer finds no `rect` to shorten),
    /// its fill sketch from (101.547, -298.630) and outline from
    /// (104.001, -299.730) for the first, filled and stroked in the section
    /// colour; the cards stay classic.
    #[test]
    fn hand_drawn_sections_match_mermaid() {
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 17\n---\nkanban\n  \
                   todo[Todo]\n    id1[Write the spec]\n    id2[Review it]@{ ticket: MC-1, \
                   assigned: 'ann', priority: 'High' }\n  doing[Doing]\n    id3[Build]\n";
        let (scene, flat) = flat(src);
        let paths: Vec<String> = flat
            .iter()
            .filter_map(|e| match e {
                crate::scene::Element::Shape {
                    geom: crate::scene::Geometry::Path(cmds),
                    ..
                } => Some(crate::output::svg::path_data(cmds)),
                _ => None,
            })
            .collect();
        for start in [
            "M101.547,-298.63C",
            "M104.001,-299.73C",
            "M306.224,-298.259C",
        ] {
            assert!(paths.iter().any(|d| d.starts_with(start)), "{start}");
        }
        assert!(scene.view_box.h > 600.0, "{:?}", scene.view_box);
    }
}
