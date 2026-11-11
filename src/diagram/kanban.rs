//! `kanban` diagrams.
//!
//! Columns are the least-indented nodes, items are the nodes indented below
//! them. Items may carry `@{ ticket: X, assigned: 'name', priority: 'High' }`
//! metadata. Rendering follows mermaid's kanban look (coloured section columns,
//! white cards with ticket / assignee footer and a priority stripe); the board is
//! laid out with Taffy: a stretched row of fixed-width flex columns, so every
//! column has the height of the tallest one.

use super::timeline::{shade, wrap_text};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::layout::boxes::{self, BoxNode};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{line_width, normalize_label};
use crate::theme::Palette;
use boxes::taffy::prelude::*;

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
    Some((id, label))
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

const PAD: f64 = 10.0;
const FONT: f64 = 14.0;
const SMALL: f64 = 12.0;

fn card_style(h: f64) -> Style {
    Style {
        size: Size {
            width: auto(),
            height: length(h as f32),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Lay out and draw a parsed board.
pub fn draw(k: &Kanban, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let col_w = cfg.section_f64("kanban", "sectionWidth", 200.0);
    let pad = cfg.section_f64("kanban", "padding", 8.0);
    let linked_tickets = cfg
        .section("kanban", "ticketBaseUrl")
        .and_then(|j| j.as_str())
        .is_some_and(|u| !u.is_empty());
    let lh = FONT * 1.3;
    let text_w = col_w - 2.0 * pad - 2.0 * PAD;

    let col_titles: Vec<Vec<String>> = k
        .columns
        .iter()
        .map(|c| wrap_text(&c.label, col_w - 2.0 * pad, FONT, false))
        .collect();
    let title_h = col_titles
        .iter()
        .map(|l| l.len() as f64 * lh)
        .fold(lh, f64::max)
        + 6.0;
    let cards: Vec<Vec<(Vec<String>, f64)>> = k
        .columns
        .iter()
        .map(|c| {
            c.items
                .iter()
                .map(|it| {
                    let lines = wrap_text(&it.label, text_w, FONT, false);
                    let footer = if it.ticket.is_some() || it.assigned.is_some() {
                        SMALL * 1.4 + 4.0
                    } else {
                        0.0
                    };
                    let h = 2.0 * PAD + lines.len() as f64 * lh + footer;
                    (lines, h)
                })
                .collect()
        })
        .collect();

    let columns: Vec<BoxNode> = cards
        .iter()
        .map(|cs| {
            let mut st =
                boxes::stretched(boxes::padded_with_title(boxes::column(pad), pad, title_h));
            st.size.width = length(col_w as f32);
            st = boxes::min_size(st, col_w, title_h + 3.0 * pad + 30.0);
            st.size.width = length(col_w as f32);
            BoxNode::new(
                st,
                cs.iter()
                    .map(|(_, h)| BoxNode::new(card_style(*h), vec![]))
                    .collect(),
            )
        })
        .collect();
    let placed = boxes::compute(&BoxNode::new(boxes::stretched(boxes::row(10.0)), columns));

    let has_title = k.title.as_ref().is_some_and(|t| !t.trim().is_empty());
    let off = Point::new(0.0, if has_title { 40.0 } else { 0.0 });
    let shift = |r: Rect| Rect::new(r.x + off.x, r.y + off.y, r.w, r.h);

    let mut bounds = Bounds::default();
    bounds.add_rect(shift(placed.rect));
    let mut els = Vec::new();
    let card_fill = pal.background.clone();
    let dark_mode = cfg
        .theme_variables
        .iter()
        .rev()
        .find(|(k, _)| k == "darkMode")
        .is_some_and(|(_, v)| v.trim() == "true");
    let card_text = pal.text_color.clone();
    for (ci, (col, cp)) in k.columns.iter().zip(&placed.children).enumerate() {
        let cr = shift(cp.rect);
        // Column `ci` is `.section-(ci+1)`: fill and stroke `cScale(ci+2)` lightened by
        // 10 (darkened under `darkMode`); the title is an HTML label in regular weight,
        // coloured by `.cluster-label, .label { color: textColor }`; cards are `.node`
        // (`background` fill, `nodeBorder` stroke).
        let fill = shade(pal.scale_color(ci + 2), !dark_mode, 10.0);
        let mut g = vec![
            Element::classed_shape(
                Geometry::Rect { rect: cr, rx: 5.0 },
                Paint::both(&fill, &fill, 1.0),
                "kanban-section",
            ),
            Element::classed_text(
                Text::centered(
                    Point::new(cr.center().x, cr.y + pad + title_h / 2.0),
                    &col_titles[ci],
                    FONT,
                    lh,
                    &pal.text_color,
                ),
                "kanban-section-title",
            ),
        ];
        for ((item, (lines, _)), ip) in col.items.iter().zip(&cards[ci]).zip(&cp.children) {
            let r = shift(ip.rect);
            g.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 5.0 },
                Paint::both(&card_fill, &pal.node_border, 1.0),
                "kanban-item",
            ));
            if let Some(c) = item.priority.and_then(Priority::color) {
                g.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::new(r.x + 2.0, r.y + 2.0, 4.0, r.h - 4.0),
                        rx: 2.0,
                    },
                    Paint::fill(&c),
                    "kanban-priority",
                ));
            }
            g.push(Element::classed_text(
                Text {
                    x: r.x + PAD,
                    y: r.y + PAD + FONT,
                    lines: lines.clone(),
                    line_height: lh,
                    font_size: FONT,
                    bold: false,
                    anchor: Anchor::Start,
                    fill: card_text.clone(),
                },
                "kanban-label",
            ));
            let fy = r.bottom() - PAD - 2.0;
            if let Some(t) = &item.ticket {
                g.push(Element::classed_text(
                    Text {
                        x: r.x + PAD,
                        y: fy,
                        lines: vec![t.clone()],
                        line_height: SMALL * 1.2,
                        font_size: SMALL,
                        bold: false,
                        anchor: Anchor::Start,
                        // An HTML label: the card's text colour, underlined inside the
                        // `.kanban-ticket-link` anchor when `ticketBaseUrl` is set.
                        fill: card_text.clone(),
                    },
                    "kanban-ticket",
                ));
                if linked_tickets {
                    let w = line_width(t, SMALL, false);
                    g.push(Element::line(
                        Point::new(r.x + PAD, fy + 1.5),
                        Point::new(r.x + PAD + w, fy + 1.5),
                        Paint::stroke(&card_text, 1.0),
                    ));
                }
            }
            if let Some(a) = &item.assigned {
                let a = normalize_label(a);
                let tw = line_width(&a, SMALL, false);
                bounds.add_rect(Rect::new(r.right() - PAD - tw, fy - SMALL, tw, SMALL));
                g.push(Element::classed_text(
                    Text {
                        x: r.right() - PAD,
                        y: fy,
                        lines: vec![a],
                        line_height: SMALL * 1.2,
                        font_size: SMALL,
                        bold: false,
                        anchor: Anchor::End,
                        fill: shade(&card_text, !pal.dark, 15.0),
                    },
                    "kanban-assigned",
                ));
            }
        }
        els.push(Element::group("kanban-column", g));
    }
    if let Some(t) = k.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let t = normalize_label(t);
        let tw = line_width(&t, 20.0, true);
        let cx = placed.rect.w.max(tw) / 2.0;
        bounds.add_rect(Rect::new(cx - tw / 2.0, 5.0, tw, 25.0));
        els.push(Element::classed_text(
            Text::centered(Point::new(cx, 18.0), &[t], 20.0, 24.0, &pal.title_color).bold(),
            "titleText",
        ));
    }
    Scene {
        kind: DiagramKind::Kanban,
        view_box: bounds.rect().pad(10.0),
        elements: vec![Element::group("kanban", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
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
    fn columns_share_height() {
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
        assert!(heights.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-6));
    }
}
