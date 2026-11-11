//! `timeline` diagrams.
//!
//! Layout mirrors mermaid's `timelineRenderer`: optional section headers on top,
//! one column per time period (150px wide), a horizontal arrow axis under the
//! periods and the period's events stacked below it, joined by a dashed line.
//! Columns, headers and event stacks are placed with Taffy ([`crate::layout::boxes`]).

use crate::color;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::boxes::{self, BoxNode, Placed};
use crate::scene::{Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{self, BreakRule, line_width};
use crate::theme::Palette;
use boxes::taffy::prelude::*;

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

const PERIOD_W: f64 = 150.0;
const PAD: f64 = 10.0;
const GAP: f64 = 10.0;
const AXIS_GAP: f64 = 50.0;
const FONT: f64 = 14.0;
const HEADER_FONT: f64 = 16.0;

fn auto_width_fixed_height(h: f64, min_w: f64) -> Style {
    Style {
        size: Size {
            width: auto(),
            height: length(h as f32),
        },
        min_size: Size {
            width: length(min_w as f32),
            height: auto(),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

struct Measured {
    header: Option<Vec<String>>,
    periods: Vec<(Vec<String>, Vec<Vec<String>>)>,
}

/// Lay out and draw a parsed timeline.
pub fn draw(tl: &Timeline, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let multicolor = !cfg.section_bool("timeline", "disableMulticolor", false);
    let text_w = PERIOD_W - 2.0 * PAD;
    let lh = FONT * 1.3;
    let sections: Vec<Measured> = tl
        .sections
        .iter()
        .map(|s| Measured {
            header: s
                .name
                .as_ref()
                .map(|n| wrap_words(n, f64::INFINITY, HEADER_FONT, true)),
            periods: s
                .periods
                .iter()
                .map(|p| {
                    (
                        wrap_words(&p.label, text_w, FONT, true),
                        p.events
                            .iter()
                            .map(|e| wrap_words(e, text_w, FONT, false))
                            .collect(),
                    )
                })
                .collect(),
        })
        .collect();
    let box_h = |n: usize| n as f64 * lh + 2.0 * PAD;
    let period_h = sections
        .iter()
        .flat_map(|s| &s.periods)
        .map(|(l, _)| box_h(l.len()))
        .fold(box_h(1), f64::max);
    let header_h = sections
        .iter()
        .filter_map(|s| s.header.as_ref())
        .map(|l| l.len() as f64 * HEADER_FONT * 1.3 + 2.0 * PAD)
        .fold(0.0, f64::max);

    // Box tree: row of sections → [header, row of periods → column(period, axis gap, events)].
    let section_nodes: Vec<BoxNode> = sections
        .iter()
        .map(|s| {
            let periods = s
                .periods
                .iter()
                .map(|(_, events)| {
                    let mut kids = vec![
                        BoxNode::leaf(PERIOD_W, period_h),
                        BoxNode::leaf(PERIOD_W, AXIS_GAP),
                    ];
                    kids.extend(
                        events
                            .iter()
                            .map(|e| BoxNode::leaf(PERIOD_W, box_h(e.len()))),
                    );
                    BoxNode::new(boxes::column(GAP), kids)
                })
                .collect();
            let mut prow = boxes::row(GAP);
            prow.justify_content = JustifyContent::CENTER;
            let mut kids = Vec::new();
            if let Some(h) = &s.header {
                let w = h
                    .iter()
                    .map(|l| line_width(l, HEADER_FONT, true))
                    .fold(0.0, f64::max)
                    + 2.0 * PAD;
                kids.push(BoxNode::new(auto_width_fixed_height(header_h, w), vec![]));
            } else if header_h > 0.0 {
                kids.push(BoxNode::new(auto_width_fixed_height(header_h, 0.0), vec![]));
            }
            kids.push(BoxNode::new(prow, periods));
            BoxNode::new(boxes::stretched(boxes::column(GAP * 2.0)), kids)
        })
        .collect();
    let body = BoxNode::new(boxes::row(GAP * 2.0), section_nodes);
    let placed = boxes::compute(&body);

    let title_h = tl
        .title
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map_or(0.0, |_| 50.0);
    let off = Point::new(20.0, 20.0 + title_h);
    let shift = |r: Rect| Rect::new(r.x + off.x, r.y + off.y, r.w, r.h);

    let mut bounds = Bounds::default();
    let mut back = Vec::new();
    let mut nodes = Vec::new();
    let has_sections = tl.has_sections();
    let mut period_index = 0usize;
    let mut axis_y: Option<f64> = None;
    let body_rect = shift(placed.rect);
    bounds.add_rect(body_rect);

    for (si, (sec, sp)) in sections.iter().zip(&placed.children).enumerate() {
        let has_header = header_h > 0.0;
        if let (Some(lines), true) = (&sec.header, has_header) {
            let r = shift(sp.children[0].rect);
            nodes.push(node_box(r, si, pal, 1.0));
            nodes.push(Element::classed_text(
                Text::centered(
                    r.center(),
                    lines,
                    HEADER_FONT,
                    HEADER_FONT * 1.3,
                    pal.scale_text_color(si),
                )
                .bold(),
                "sectionTitle",
            ));
        }
        let prow: &Placed = &sp.children[usize::from(has_header)];
        for ((label, events), pp) in sec.periods.iter().zip(&prow.children) {
            let ci = if has_sections {
                si
            } else if multicolor {
                period_index
            } else {
                0
            };
            period_index += 1;
            let tfill = pal.scale_text_color(ci);
            let pr = shift(pp.children[0].rect);
            let gap_r = shift(pp.children[1].rect);
            axis_y = Some(gap_r.y + gap_r.h / 2.0);
            nodes.push(node_box(pr, ci, pal, 1.0));
            nodes.push(Element::classed_text(
                Text::centered(pr.center(), label, FONT, lh, tfill).bold(),
                "periodText",
            ));
            let last = pp.children.last().map_or(pr, |c| shift(c.rect));
            let bottom = if events.is_empty() {
                gap_r.bottom()
            } else {
                last.bottom()
            };
            back.push(Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(pr.center().x, pr.bottom())),
                    PathCmd::LineTo(Point::new(pr.center().x, bottom)),
                ]),
                Paint::stroke(&pal.line_color, 2.0).dashed(vec![5.0, 5.0]),
                "eventLine",
            ));
            for (lines, ep) in events.iter().zip(pp.children.iter().skip(2)) {
                let er = shift(ep.rect);
                // `.eventWrapper { filter: brightness(120%) }`.
                nodes.push(node_box(er, ci, pal, EVENT_BRIGHTNESS));
                nodes.push(Element::classed_text(
                    Text::centered(
                        er.center(),
                        lines,
                        FONT,
                        lh,
                        &brightness(tfill, EVENT_BRIGHTNESS),
                    ),
                    "eventText",
                ));
            }
        }
    }

    if let Some(y) = axis_y {
        let x0 = body_rect.x - 10.0;
        let x1 = body_rect.right() + 20.0;
        back.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(x0, y)),
                PathCmd::LineTo(Point::new(x1 - 8.0, y)),
            ]),
            Paint::stroke(&pal.line_color, 4.0),
            "axis",
        ));
        back.push(Element::classed_shape(
            Geometry::Path(geom::polygon(&[
                Point::new(x1 - 12.0, y - 8.0),
                Point::new(x1 + 4.0, y),
                Point::new(x1 - 12.0, y + 8.0),
            ])),
            Paint::fill(&pal.line_color),
            "arrowhead",
        ));
        bounds.add_rect(Rect::new(x0, y - 8.0, x1 + 4.0 - x0, 16.0));
    }

    if let Some(t) = tl.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let lines = crate::text::label_lines(t);
        let w = lines
            .iter()
            .map(|l| line_width(l, 24.0, true))
            .fold(0.0, f64::max);
        let cx = body_rect.x + body_rect.w.max(w) / 2.0;
        let c = Point::new(cx, 20.0 + title_h / 2.0 - 5.0);
        bounds.add_rect(Rect::centered(c, w, 30.0));
        nodes.push(Element::classed_text(
            Text::centered(c, &lines, 24.0, 30.0, &pal.title_color).bold(),
            "titleText",
        ));
    }

    Scene {
        kind: DiagramKind::Timeline,
        view_box: bounds.rect().pad(15.0),
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

/// mermaid's timeline node: rounded box with a darker rule along its bottom edge.
/// A node of section colour `ci`: `cScale` fill with a `cScaleInv` underline
/// (`.section-N rect` / `.section-N line`).
/// `light` is a CSS `brightness()` factor applied to the whole node.
fn node_box(r: Rect, ci: usize, pal: &Palette, light: f64) -> Element {
    let fill = brightness(pal.scale_color(ci), light);
    let edge = brightness(pal.scale_inv_color(ci), light);
    Element::group(
        "node",
        vec![
            Element::rect(r, 6.0, Paint::fill(&fill)),
            Element::line(
                Point::new(r.x + 3.0, r.bottom()),
                Point::new(r.right() - 3.0, r.bottom()),
                Paint::stroke(&edge, 3.0),
            ),
        ],
    )
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
        assert!(a.view_box.w > 4.0 * PERIOD_W);
    }

    #[test]
    fn events_are_brightened() {
        // `.eventWrapper { filter: brightness(120%) }`: #d7ff86 -> #ffffa1.
        let c = brightness(&Color::new("#d7ff86"), EVENT_BRIGHTNESS);
        assert_eq!(c.css(), "#ffffa1");
        assert_eq!(brightness(&Color::new("#000000"), 1.2).css(), "#000000");
    }
}
