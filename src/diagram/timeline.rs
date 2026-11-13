//! `timeline` diagrams.
//!
//! Layout is a port of mermaid's horizontal `timelineRenderer`: optional section
//! headers on top, one 190px column every 200px per time period, a horizontal
//! arrow axis under the periods and the period's events stacked below it,
//! crossed by a dashed drop line.

use crate::color;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{self, BreakRule, line_width};
use crate::theme::Palette;

/// Wrap a raw label (`<br>` respected) to `max_width` pixels the way the browser
/// lays out an HTML label (spaces, hyphens, CJK); see [`BreakRule::Browser`].
pub(crate) fn wrap_text(raw: &str, max_width: f64, font_size: f64, bold: bool) -> Vec<String> {
    text::wrap(raw, max_width, font_size, bold, BreakRule::Browser)
}

/// Greedy word wrap at spaces only, as timeline's SVG `wrap` (`split(/(\s+|<br>)/)`).
fn wrap_words(raw: &str, max_width: f64, font_size: f64, bold: bool) -> Vec<String> {
    text::wrap(raw, max_width, font_size, bold, BreakRule::Spaces)
}

/// Darker (light themes) or lighter (dark themes) shade of a CSS colour.
pub(crate) fn shade(c: &Color, dark_theme: bool, amount: f64) -> Color {
    match color::parse(c.css()) {
        Some(rgb) => Color::new(
            rgb.lighten(if dark_theme { amount } else { -amount })
                .to_hex(),
        ),
        None => c.clone(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    pub label: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// `None` for periods declared before any `section`.
    pub name: Option<String>,
    pub periods: Vec<Period>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub title: Option<String>,
    pub sections: Vec<Section>,
}

impl Timeline {
    fn has_sections(&self) -> bool {
        self.sections.iter().any(|s| s.name.is_some())
    }
}

/// Lex the events of a line, `s` starting at its first `:` (timeline.jison
/// `":"\s(?:[^:\n]|":"(?!\s))+`): each event is a colon, whitespace and at least
/// one character, running until the next colon that is followed by whitespace
/// (so `10:30` stays intact). `None` where the lexer would hit an invalid `:`.
fn lex_events(s: &str) -> Option<Vec<String>> {
    let c: Vec<char> = s.chars().collect();
    let mut events = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] != ':' || !c.get(i + 1).is_some_and(|n| n.is_whitespace()) {
            return None;
        }
        i += 2;
        let start = i;
        while i < c.len() && !(c[i] == ':' && c.get(i + 1).is_none_or(|n| n.is_whitespace())) {
            i += 1;
        }
        if i == start {
            return None;
        }
        events.push(c[start..i].iter().collect::<String>().trim().to_string());
    }
    Some(events)
}

/// Split a line into its period (text before the first `:`) and events.
fn split_events(s: &str) -> Option<(String, Vec<String>)> {
    match s.find(':') {
        Some(at) => Some((s[..at].trim().to_string(), lex_events(&s[at..])?)),
        None => Some((s.trim().to_string(), Vec::new())),
    }
}

/// Parse a timeline body.
pub fn parse(body: &str, title: Option<String>) -> Result<Timeline> {
    let mut tl = Timeline {
        title,
        sections: Vec::new(),
    };
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = line
                .strip_prefix("timeline")
                .ok_or_else(|| Error::parse(line_no, format!("{line}\nExpecting 'timeline'")))?
                .trim();
            if !matches!(rest, "" | "LR" | "TD") {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nUnexpected text after 'timeline'"),
                ));
            }
            header = true;
            continue;
        }
        if let Some(t) = line
            .strip_prefix("title")
            .filter(|r| r.is_empty() || r.starts_with(' '))
        {
            tl.title = Some(t.trim().to_string());
        } else if let Some(s) = line
            .strip_prefix("section")
            .filter(|r| r.starts_with([' ', '\t']))
        {
            tl.sections.push(Section {
                name: Some(s.trim().to_string()),
                periods: Vec::new(),
            });
        } else if line.starts_with(':') {
            // Continuation: more events for the previous period.
            let period = tl
                .sections
                .last_mut()
                .and_then(|s| s.periods.last_mut())
                .ok_or_else(|| {
                    Error::parse(line_no, format!("{line}\nEvent without a time period"))
                })?;
            let events = lex_events(line).ok_or_else(|| {
                Error::parse(line_no, format!("{line}\nExpecting an event after ': '"))
            })?;
            period
                .events
                .extend(events.into_iter().filter(|e| !e.is_empty()));
        } else {
            let (label, events) = split_events(line).ok_or_else(|| {
                Error::parse(line_no, format!("{line}\nExpecting an event after ': '"))
            })?;
            if label.is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nExpecting a time period"),
                ));
            }
            if tl.sections.is_empty() {
                tl.sections.push(Section {
                    name: None,
                    periods: Vec::new(),
                });
            }
            let events: Vec<String> = events.into_iter().filter(|e| !e.is_empty()).collect();
            if let Some(sec) = tl.sections.last_mut() {
                sec.periods.push(Period { label, events });
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'timeline'"));
    }
    Ok(tl)
}

/// Width a node's text wraps at (`node.width`), before its padding.
const NODE_W: f64 = 150.0;
/// `node.padding`.
const NODE_PAD: f64 = 20.0;

/// A node's text: the timeline renderer's SVG `wrap` (greedy, at spaces and
/// `<br>`), lines 1.1em apart.
fn node_lines(raw: &str, width: f64, font: f64) -> Vec<String> {
    wrap_words(raw, width, font, false)
}

/// `getVirtualNodeHeight` / `drawNode`: the text's bounding box plus half a line
/// and the padding.
fn node_height(lines: &[String], font: f64) -> f64 {
    text::TextBlock::svg(lines.to_vec(), font, false).height + font * 1.1 * 0.5 + NODE_PAD
}

/// Lay out and draw a parsed timeline: a port of `timelineRenderer.ts` (the
/// horizontal layout) and `svgDraw.js`'s `drawNode`.
pub fn draw(tl: &Timeline, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let multicolor = !cfg.section_bool("timeline", "disableMulticolor", false);
    let left_margin = cfg.section_f64("timeline", "leftMargin", 150.0);
    let padding = cfg.section_f64("timeline", "padding", 50.0);
    let font = pal.font_size;
    let redux = matches!(
        cfg.theme,
        crate::theme::ThemeName::Redux
            | crate::theme::ThemeName::ReduxDark
            | crate::theme::ThemeName::ReduxColor
            | crate::theme::ThemeName::ReduxDarkColor
    );
    let has_sections = tl.has_sections();
    let sections: Vec<&Section> = if has_sections {
        tl.sections.iter().filter(|s| s.name.is_some()).collect()
    } else {
        tl.sections.iter().collect()
    };
    let periods = || sections.iter().flat_map(|s| s.periods.iter());

    // The pre-pass: tallest section, tallest period, tallest event stack.
    let max_section_h = sections
        .iter()
        .filter_map(|s| s.name.as_ref())
        .map(|n| node_height(&node_lines(n, NODE_W, font), font) + 20.0)
        .fold(0.0, f64::max);
    let max_task_h = periods()
        .map(|p| node_height(&node_lines(&p.label, NODE_W, font), font) + 20.0)
        .fold(0.0, f64::max);
    let max_event_len = periods()
        .map(|p| {
            let n = p.events.len() as f64;
            p.events
                .iter()
                .map(|e| node_height(&node_lines(e, NODE_W, font), font))
                .sum::<f64>()
                + if n > 0.0 { (n - 1.0) * 10.0 } else { 0.0 }
        })
        .fold(0.0, f64::max);

    let mut bounds = Bounds::default();
    let mut back = Vec::new();
    let mut nodes = Vec::new();
    let line_color = pal.scale_text_color(11).clone();
    let black = Color::new("#000000");

    // drawNode at (x, y): `width` wraps the text, the box is 2·padding wider.
    let mut node = |x: f64, y: f64, raw: &str, width: f64, min_h: f64, ci: usize, event: bool| {
        let lines = node_lines(raw, width, font);
        let h = node_height(&lines, font).max(min_h);
        let w = width + 2.0 * NODE_PAD;
        let r = Rect::new(x, y, w, h);
        let light = if event { EVENT_BRIGHTNESS } else { 1.0 };
        nodes.push(node_box(r, ci, pal, light, redux));
        let tfill = brightness(pal.scale_text_color(ci), light);
        // The text group sits at (w / 2, padding / 2) (redux: further down), the
        // lines `middle`-aligned from 1em, 1.1em apart.
        let ty = y + if redux {
            if event {
                NODE_PAD / 2.0 + 3.0
            } else {
                NODE_PAD
            }
        } else {
            NODE_PAD / 2.0
        };
        let mut t = Text::centered(
            Point::new(x + w / 2.0, 0.0),
            &lines,
            font,
            1.1 * font,
            &tfill,
        );
        t.y = ty + font + 0.27 * font;
        nodes.push(Element::classed_text(
            t,
            if event { "eventText" } else { "nodeText" },
        ));
        (h, r)
    };

    let mut master_x = 50.0 + left_margin;
    let section_y = 50.0;
    let mut color = 0usize;
    for (si, sec) in sections.iter().enumerate() {
        let mut master_y = 50.0;
        let n = sec.periods.len().max(1) as f64;
        if let Some(name) = sec.name.as_ref().filter(|_| has_sections) {
            let (_, r) = node(
                master_x,
                section_y,
                name,
                200.0 * n - 50.0,
                max_section_h,
                si,
                false,
            );
            bounds.add_rect(r);
            master_y += max_section_h + 50.0;
        }
        let mut x = master_x;
        for p in &sec.periods {
            let ci = if has_sections { si } else { color };
            let (th, r) = node(x, master_y, &p.label, NODE_W, max_task_h, ci, false);
            bounds.add_rect(r);
            let task_h = max_task_h.max(th);
            if !p.events.is_empty() {
                let mut ey = master_y + 200.0;
                for e in &p.events {
                    let (eh, r) = node(x, ey, e, NODE_W, 50.0, ci, true);
                    bounds.add_rect(r);
                    ey += 10.0 + eh;
                }
            }
            // The dashed drop line with its arrowhead (marker scaled by 2).
            let lx = x + 190.0 / 2.0;
            let y1 = master_y + task_h;
            let y2 = y1 + 100.0 + max_event_len + 100.0;
            back.push(Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(lx, y1)),
                    PathCmd::LineTo(Point::new(lx, y2)),
                ]),
                Paint::stroke(&line_color, 2.0).dashed(vec![5.0, 5.0]),
                "eventLine",
            ));
            back.push(Element::path(
                geom::polygon(&[
                    Point::new(lx - 4.0, y2 - 8.0),
                    Point::new(lx, y2 + 2.0),
                    Point::new(lx + 4.0, y2 - 8.0),
                ]),
                Paint::fill(&black),
            ));
            bounds.add_rect(Rect::new(lx - 1.0, y1, 2.0, y2 - y1));
            x += 200.0;
            if !has_sections && multicolor {
                color += 1;
            }
        }
        master_x += 200.0 * n;
    }

    // The axis: 4px from leftMargin to the drawing's width + 3·leftMargin, its
    // marker scaled by 4.
    let content = bounds.rect();
    let depth_y = if has_sections {
        max_section_h + max_task_h + 150.0
    } else {
        max_task_h + 100.0
    };
    let x2 = content.w + 3.0 * left_margin;
    back.push(Element::classed_shape(
        Geometry::Path(vec![
            PathCmd::MoveTo(Point::new(left_margin, depth_y)),
            PathCmd::LineTo(Point::new(x2, depth_y)),
        ]),
        Paint::stroke(&line_color, 4.0),
        "axis",
    ));
    back.push(Element::path(
        geom::polygon(&[
            Point::new(x2 - 20.0, depth_y - 8.0),
            Point::new(x2 + 4.0, depth_y),
            Point::new(x2 - 20.0, depth_y + 8.0),
        ]),
        Paint::fill(&black),
    ));
    bounds.add_rect(Rect::new(left_margin, depth_y - 2.0, x2 - left_margin, 4.0));

    if let Some(t) = tl.title.as_ref().filter(|t| !t.trim().is_empty()) {
        // A bold `4ex` text at (width / 2 - leftMargin, 20).
        let size = 4.0 * 0.52 * font;
        let line = crate::text::label_lines(t).join(" ");
        let x = content.w / 2.0 - left_margin;
        let w = line_width(&line, size, true);
        bounds.add_rect(Rect::new(x, 20.0 - 0.905 * size, w, 1.117 * size));
        nodes.push(Element::classed_text(
            Text {
                x,
                y: 20.0,
                lines: vec![line],
                line_height: size,
                font_size: size,
                bold: true,
                anchor: crate::scene::Anchor::Start,
                fill: pal.title_color.clone(),
            },
            "titleText",
        ));
    }

    Scene {
        kind: DiagramKind::Timeline,
        // setupGraphViewbox with `timeline.padding`.
        view_box: bounds.rect().pad(padding),
        elements: vec![
            Element::group("lines", back),
            Element::group("nodes", nodes),
        ],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// A quadratic Bézier from `a` via `c` to `b` as a cubic.
fn quad(a: Point, c: Point, b: Point) -> PathCmd {
    let k = 2.0 / 3.0;
    PathCmd::CubicTo(
        Point::new(a.x + k * (c.x - a.x), a.y + k * (c.y - a.y)),
        Point::new(b.x + k * (c.x - b.x), b.y + k * (c.y - b.y)),
        b,
    )
}

/// `drawNode`'s background: a box with rounded top corners (square under the
/// redux themes) and, outside redux, a `cScaleInv` rule along its bottom
/// (`.section-N path` / `.section-N line`). `light` is a CSS `brightness()`
/// factor applied to the whole node.
fn node_box(r: Rect, ci: usize, pal: &Palette, light: f64, redux: bool) -> Element {
    let fill = brightness(pal.scale_color(ci), light);
    let edge = brightness(pal.scale_inv_color(ci), light);
    let rr = if redux { 0.0 } else { 5.0 };
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let mut d = vec![PathCmd::MoveTo(Point::new(x0, y1))];
    d.push(PathCmd::LineTo(Point::new(x0, y0 + rr)));
    if rr > 0.0 {
        d.push(quad(
            Point::new(x0, y0 + rr),
            Point::new(x0, y0),
            Point::new(x0 + rr, y0),
        ));
    }
    d.push(PathCmd::LineTo(Point::new(x1 - rr, y0)));
    if rr > 0.0 {
        d.push(quad(
            Point::new(x1 - rr, y0),
            Point::new(x1, y0),
            Point::new(x1, y0 + rr),
        ));
    }
    d.push(PathCmd::LineTo(Point::new(x1, y1)));
    d.push(PathCmd::Close);
    let mut els = vec![Element::classed_shape(
        Geometry::Path(d),
        Paint::fill(&fill),
        "node-bkg",
    )];
    if !redux {
        els.push(Element::line(
            Point::new(x0, y1),
            Point::new(x1, y1),
            Paint::stroke(&edge, 3.0),
        ));
    }
    Element::group("node", els)
}

/// timeline/styles.js: `.eventWrapper { filter: brightness(120%) }`.
const EVENT_BRIGHTNESS: f64 = 1.2;

/// CSS `filter: brightness(k)`: every channel scaled by `k`, clamped.
fn brightness(c: &Color, k: f64) -> Color {
    if k == 1.0 {
        return c.clone();
    }
    match crate::color::parse(c.css()) {
        Some(x) => {
            let ch = |v: u8| (v as f64 * k).round().clamp(0.0, 255.0) as u8;
            Color::new(
                crate::color::Rgba {
                    r: ch(x.r),
                    g: ch(x.g),
                    b: ch(x.b),
                    a: x.a,
                }
                .to_hex(),
            )
        }
        None => c.clone(),
    }
}

/// Parse and render a `timeline` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let tl = parse(body, title)?;
    Ok(draw(&tl, cfg, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "timeline
    title History of Social Media
    2002 : LinkedIn
    2004 : Facebook : Google
         : Orkut
    section Later
    2005 : YouTube
    Slot : Meeting at 10:30
";

    #[test]
    fn colons_follow_the_jison_lexer() {
        // A period cannot contain `:`; an event needs `: ` plus text, and a `:`
        // not followed by whitespace stays inside the event.
        assert!(parse("timeline\n10:30 slot : x", None).is_err());
        assert!(parse("timeline\n2020 :", None).is_err());
        assert!(parse("timeline\n2020 :: a", None).is_err());
        assert!(parse("timeline\n2020 : a : : b", None).is_err());
        let t = parse("timeline\n2020 : a :b : c", None).unwrap();
        assert_eq!(t.sections[0].periods[0].events, vec!["a :b", "c"]);
    }

    #[test]
    fn parses_periods_events_and_sections() {
        let t = parse(SRC, None).unwrap();
        assert_eq!(t.title.as_deref(), Some("History of Social Media"));
        assert_eq!(t.sections.len(), 2);
        assert_eq!(t.sections[0].name, None);
        let p = &t.sections[0].periods;
        assert_eq!(p[1].label, "2004");
        assert_eq!(p[1].events, vec!["Facebook", "Google", "Orkut"]);
        assert_eq!(t.sections[1].name.as_deref(), Some("Later"));
        assert_eq!(t.sections[1].periods[1].label, "Slot");
        assert_eq!(t.sections[1].periods[1].events, vec!["Meeting at 10:30"]);
    }

    #[test]
    fn errors_have_line_numbers() {
        let e = parse("timeline\n\n: orphan", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("timeline\n : \n", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("timeline nonsense", None).is_err());
        assert!(parse("graph TD", None).is_err());
    }

    #[test]
    fn wraps_long_text() {
        let lines = wrap_text("one two three four five six seven eight", 60.0, 14.0, false);
        assert!(lines.len() > 2);
        assert!(
            lines
                .iter()
                .all(|l| line_width(l, 14.0, false) <= 60.0 || !l.contains(' '))
        );
        assert_eq!(wrap_text("a<br>b", 100.0, 14.0, false), vec!["a", "b"]);
    }

    #[test]
    fn layout_is_deterministic_and_non_overlapping() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let a = render(SRC, None, &cfg, &pal).unwrap();
        assert_eq!(a, render(SRC, None, &cfg, &pal).unwrap());
        assert!(a.view_box.w > 4.0 * NODE_W);
    }

    #[test]
    fn events_are_brightened() {
        // `.eventWrapper { filter: brightness(120%) }`: #d7ff86 -> #ffffa1.
        let c = brightness(&Color::new("#d7ff86"), EVENT_BRIGHTNESS);
        assert_eq!(c.css(), "#ffffa1");
        assert_eq!(brightness(&Color::new("#000000"), 1.2).css(), "#000000");
    }

    #[test]
    fn layout_follows_the_horizontal_renderer() {
        // mermaid 12.3.0, the gallery's timeline: viewBox `100 -61 1390 722.6`
        // (the title's box differs by a fraction), sections 66.8 tall at y 50,
        // periods at y 166.8, events from y 366.8, the axis at y 283.6.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let src = include_str!("../../examples/diagrams/timeline.mmd");
        let s = render(src, None, &cfg, &pal).unwrap();
        let v = s.view_box;
        assert_eq!((v.x, v.w), (100.0, 1390.0));
        assert!((v.bottom() - 661.6).abs() < 0.1, "{v:?}");
        let Element::Group { children, .. } = &s.elements[0] else {
            panic!()
        };
        let axis = children
            .iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(c),
                    paint,
                    ..
                } if paint.stroke_width == 4.0 => Some(c.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(axis[0], PathCmd::MoveTo(Point::new(150.0, 283.6)));
        assert_eq!(axis[1], PathCmd::LineTo(Point::new(1440.0, 283.6)));
    }
}
