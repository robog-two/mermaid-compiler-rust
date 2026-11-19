//! `timeline` diagrams.
//!
//! Layout is a port of mermaid's horizontal `timelineRenderer`: optional section
//! headers on top, one 190px column every 200px per time period, a horizontal
//! arrow axis under the periods and the period's events stacked below it,
//! crossed by a dashed drop line. `timeline TD` ports `timelineRendererVertical`:
//! periods left of a vertical axis, their events to its right.

use crate::color;
use crate::config::MermaidConfig;
use crate::diagram::wardley::ID_MARK;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::output::svg::{BEFORE_STYLE, esc, esc_text, js_num as n};
use crate::scene::{Color, DiagramKind, Element, Geometry, Gradient, Paint, Scene, Shadow, Text};
use crate::text::{self, BreakRule, line_width};
use crate::theme::Palette;
use std::fmt::Write;

/// Wrap a raw label (`<br>` respected) to `max_width` pixels the way the browser
/// lays out an HTML label (spaces, hyphens, CJK); see [`BreakRule::Browser`].
pub(crate) fn wrap_text(raw: &str, max_width: f64, font_size: f64, bold: bool) -> Vec<String> {
    text::wrap(raw, max_width, font_size, bold, BreakRule::Browser)
}

/// `raw.split(/(\s+|<br>)/)`: the words, the whitespace runs between them and
/// the literal `<br>`s, each its own entry (empty strings at the edges too).
fn js_split_words(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            let run: usize = rest
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(char::len_utf8)
                .sum();
            out.push(std::mem::take(&mut cur));
            out.push(rest[..run].to_string());
            rest = &rest[run..];
        } else if let Some(after) = rest.strip_prefix("<br>") {
            out.push(std::mem::take(&mut cur));
            out.push("<br>".to_string());
            rest = after;
        } else {
            cur.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.push(cur);
    out
}

/// svgDraw.js `wrap`: the texts of the `tspan`s it leaves. The words and the
/// whitespace between them are joined with a space each (so a gap is three
/// spaces wide in the DOM, one when painted), a `tspan`'s measured text being
/// `getComputedTextLength` of the trimmed line, whitespace collapsed.
fn wrap_tspans(raw: &str, width: f64, font: f64, bold: bool) -> Vec<String> {
    let trim = |s: String| s.trim().to_string();
    let mut tspans = vec![String::new()];
    let mut line: Vec<String> = Vec::new();
    for word in js_split_words(raw) {
        line.push(word.clone());
        let t = trim(line.join(" "));
        let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
        let over = line_width(&collapsed, font, bold) > width;
        if let Some(last) = tspans.last_mut() {
            *last = t;
        }
        if over || word == "<br>" {
            line.pop();
            if let Some(last) = tspans.last_mut() {
                *last = trim(line.join(" "));
            }
            line = if word == "<br>" {
                vec![String::new()]
            } else {
                vec![word.clone()]
            };
            tspans.push(word);
        }
    }
    tspans
}

/// Darker (light themes) or lighter (dark themes) shade of a CSS colour.
pub(crate) fn shade(c: &Color, dark_theme: bool, amount: f64) -> Color {
    let dl = if dark_theme { amount } else { -amount };
    color::adjust(c.css(), 0.0, 0.0, dl).map_or_else(|| c.clone(), |x| Color::new(x.to_hex()))
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

/// `timeline LR` (the default) or `timeline TD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    LeftRight,
    TopDown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub title: Option<String>,
    pub direction: Direction,
    pub sections: Vec<Section>,
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
        direction: Direction::LeftRight,
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
            tl.direction = match rest {
                "" | "LR" => Direction::LeftRight,
                "TD" => Direction::TopDown,
                _ => {
                    return Err(Error::parse(
                        line_no,
                        format!("{line}\nUnexpected text after 'timeline'"),
                    ));
                }
            };
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

/// What both renderers' pre-pass measures for every period: its node's
/// `descr` is the task object itself, so the text is its string form.
const TASK_OBJECT_TEXT: &str = "[object Object]";

/// Width a node's text wraps at (`node.width`), before its padding.
const NODE_W: f64 = 150.0;
/// `node.padding`.
const NODE_PAD: f64 = 20.0;

/// A node's text: the timeline renderer's SVG `wrap` (greedy, at spaces and
/// `<br>`), lines 1.1em apart, measured `bold` where the redux themes'
/// `.section-N text { font-weight: 600 }` reaches it. Returns the `tspan`
/// texts as the DOM has them and the lines as painted (whitespace collapsed).
fn node_lines(raw: &str, width: f64, font: f64, bold: bool) -> (Vec<String>, Vec<String>) {
    let tspans = wrap_tspans(raw, width, font, bold);
    let lines = tspans
        .iter()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    (tspans, lines)
}

/// `getVirtualNodeHeight` / `drawNode`: the text's bounding box (`getBBox`
/// reads in single precision) plus half a line of the configuration's
/// `fontSize` (`conf.fontSize`, not the theme's) and the padding.
fn padded_node_height(lines: &[String], font: f64, conf_font: f64, pad: f64) -> f64 {
    // The box of the last line, then a line step (single precision) for each
    // line above it, added one after the other.
    let mut bbox = match lines {
        [.., last] => middle_text_height(std::slice::from_ref(last), font) as f32,
        [] => 0.0,
    };
    let step = (1.1 * font) as f32;
    for _ in 1..lines.len() {
        bbox += step;
    }
    f64::from(bbox) + conf_font * 1.1 * 0.5 + pad
}

/// `getBBox().height` of the node text: lines 1.1em apart, `dominant-baseline:
/// middle`. Chromium shifts the baseline by half the x-height (Arial 1082/2048 em) and
/// snaps the box's edges to 1/64 px, a tie moving each edge outwards: at 16px the
/// shift is 270.5/64 px, so a line whose bottom is its font's descent (no deep
/// descender) is 1/64 px taller than [`text::svg_line_box`] (17.015625px,
/// measured for 8 to 36px).
fn middle_text_height(lines: &[String], font: f64) -> f64 {
    let h = text::TextBlock::svg(lines.to_vec(), font, false).height;
    if h == 0.0 {
        return h;
    }
    let last = lines.last().map_or("", String::as_str);
    let shift64 = text::face().x_height_em(false) * font / 2.0 * 64.0;
    let tie = (shift64 - shift64.floor() - 0.5).abs() < 1e-9;
    let at_descent = text::svg_line_box(font, last) == text::svg_line_box(font, "");
    if tie && at_descent { h + 1.0 / 64.0 } else { h }
}

/// The redux themes' node styling (timeline/styles.js `genReduxSections`).
struct Redux {
    /// `isDarkTheme && isColorTheme`: boxes keep `mainBkg`, only outlines
    /// take the slot colour.
    dark_color: bool,
    /// The theme's name has "dark" in it.
    dark: bool,
    /// Under the neo look, `drawNode`'s `{id}-drop-shadow` filter, which the
    /// boxes' `filter: url(…)` then finds (without it, a missing reference
    /// Chromium ignores).
    shadow: Option<Shadow>,
}

/// What `drawNode` needs besides the node itself.
struct NodeStyle<'a> {
    pal: &'a Palette,
    /// The theme's font size (the stylesheet's).
    font: f64,
    /// `conf.fontSize`.
    conf_font: f64,
    redux: Option<Redux>,
    /// The neo look with the theme's `useGradient` (not `neutral`): boxes
    /// filled `mainBkg` and outlined, like their rules, with a 2px
    /// `{id}-gradient` stroke (`.section-N[data-look="neo"]`). The inner
    /// `None` where the renderer defines no such gradient (`timeline TD`):
    /// the outlines refer to nothing, which paints nothing.
    gradient: Option<Option<Gradient>>,
    /// The horizontal renderer passes `drawNode` its `isEvent` (the redux
    /// event text offset); the vertical one never does.
    event_offset: bool,
    /// The neo look (`data-look="neo"` on every node).
    neo: bool,
}

impl NodeStyle<'_> {
    fn bold(&self) -> bool {
        self.redux.is_some()
    }

    /// A pre-pass `getVirtualNodeHeight`: its text appended to the svg root,
    /// outside any `.section-N`, so never bold.
    fn virtual_height(&self, raw: &str, width: f64, pad: f64) -> f64 {
        let (_, lines) = node_lines(raw, width, self.font, false);
        padded_node_height(&lines, self.font, self.conf_font, pad)
    }
}

/// `THEME_COLOR_LIMIT`: `drawNode` numbers its sections `fullSection % 12 - 1`.
const MAX_SECTIONS: usize = 12;

/// Which wrapper group `drawNode` is called on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A section header: a plain `<g>`.
    Section,
    /// `taskWrapper`.
    Task,
    /// `eventWrapper`, under a brightness filter.
    Event,
}

/// Mermaid's own markup of the drawing (see [`Element::Markup`]): the node
/// groups and lines in document order, `drawNode`'s counter for the box ids.
struct Mk {
    s: String,
    nodes: usize,
    /// What `diagramId` is: the svg's id (the horizontal renderer) or
    /// `undefined` (the vertical one never passes it on).
    id: String,
}

impl Mk {
    fn new(id: &str) -> Self {
        Mk {
            s: String::new(),
            nodes: 0,
            id: id.to_string(),
        }
    }

    /// A drop line (`lineWrapper`): the vertical renderer's name its marker
    /// `arrowhead` alone.
    fn line(&mut self, a: Point, b: Point, width: u32, marker: &str, dashed: bool) {
        let _ = write!(
            self.s,
            "<g class=\"lineWrapper\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"{width}\" stroke=\"black\" marker-end=\"url(#{marker})\"{}/></g>",
            n(a.x),
            n(a.y),
            n(b.x),
            n(b.y),
            if dashed {
                " stroke-dasharray=\"5,5\""
            } else {
                ""
            },
        );
    }
}

/// One `drawNode` call at (x, y): `width` wraps the text and the box is
/// 2·`pad` wider; at least `min_h` tall. `kind` selects the wrapper group, the
/// event text offset and the brightness filter on events. Returns the box.
///
/// Drawn as mermaid nests it: the node translated to (x, y), its box from the
/// origin and its text in a group translated to (w / 2, the text offset).
#[allow(clippy::too_many_arguments)]
fn draw_node(
    out: &mut Vec<Element>,
    mk: &mut Mk,
    st: &NodeStyle,
    x: f64,
    y: f64,
    raw: &str,
    width: f64,
    pad: f64,
    min_h: f64,
    ci: usize,
    kind: Kind,
) -> Rect {
    let event = kind == Kind::Event;
    let font = st.font;
    let bold = st.bold();
    let (tspans, lines) = node_lines(raw, width, font, bold);
    let h = padded_node_height(&lines, font, st.conf_font, pad).max(min_h);
    let w = width + 2.0 * pad;
    let bkg = node_box(Rect::new(0.0, 0.0, w, h), ci, st);
    // The text group sits at (w / 2, padding / 2) (redux: further down), its
    // lines `middle`-aligned from 1em, 1.1em apart.
    let ty = if st.redux.is_some() {
        if event && st.event_offset {
            pad / 2.0 + 3.0
        } else {
            pad
        }
    } else {
        pad / 2.0
    };
    // The markup: the wrapper, the node's group, its box and its text.
    let section = (ci % MAX_SECTIONS) as i64 - 1;
    let wrapper = match kind {
        Kind::Section => String::new(),
        Kind::Task => " class=\"taskWrapper\"".to_string(),
        Kind::Event => " class=\"eventWrapper\"".to_string(),
    };
    let rd = 5.0;
    let d = if st.redux.is_some() {
        format!(
            "M0 {} v{} h{} v{} H0 Z",
            n(h - rd),
            n(-(h - rd)),
            n(w),
            n(h)
        )
    } else {
        format!(
            "M0 {} v{} q0,-5,5,-5 h{} q5,0,5,5 v{} H0 Z",
            n(h - rd),
            n(-h + 2.0 * rd),
            n(w - 2.0 * rd),
            n(h - rd)
        )
    };
    let _ = write!(
        mk.s,
        "<g{wrapper} transform=\"translate({}, {})\"><g class=\"timeline-node section-{section}\"{}><g><path id=\"{}-node-{}\" class=\"node-bkg node-undefined\" d=\"{d}\"/>",
        n(x),
        n(y),
        if st.neo { " data-look=\"neo\"" } else { "" },
        mk.id,
        mk.nodes,
    );
    mk.nodes += 1;
    if st.redux.is_none() {
        let _ = write!(
            mk.s,
            "<line class=\"node-line-{section}\" x1=\"0\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
            n(h),
            n(w),
            n(h),
        );
    }
    let _ = write!(
        mk.s,
        "</g><g transform=\"translate({}, {})\"><text dy=\"1em\" alignment-baseline=\"middle\" dominant-baseline=\"middle\" text-anchor=\"middle\">",
        n(w / 2.0),
        n(ty),
    );
    for (i, t) in tspans.iter().enumerate() {
        let dy = if i == 0 { "1em" } else { "1.1em" };
        if t.is_empty() {
            let _ = write!(mk.s, "<tspan x=\"0\" dy=\"{dy}\"/>");
        } else {
            let _ = write!(mk.s, "<tspan x=\"0\" dy=\"{dy}\">{}</tspan>", esc_text(t));
        }
    }
    mk.s.push_str("</text></g></g></g>");

    let fill = if st.redux.is_some() {
        &st.pal.node_border
    } else {
        st.pal.scale_text_color(ci)
    };
    let t = Text {
        x: 0.0,
        y: 0.0,
        lines,
        line_height: 1.1 * font,
        font_size: font,
        bold,
        weight: bold.then_some(600),
        anchor: crate::scene::Anchor::Middle,
        fill: fill.clone(),
        tspan_central: false,
        dy_tspans: true,
        family: None,
    };
    let label = Element::Translated {
        dx: w / 2.0,
        dy: ty,
        children: vec![Element::classed_text(
            t,
            if event { "eventText" } else { "nodeText" },
        )],
    };
    let node = Element::Translated {
        dx: x,
        dy: y,
        children: vec![bkg, label],
    };
    if event {
        // timeline/styles.js: `.eventWrapper { filter: brightness(120%) }` on
        // the event's whole group, its text's antialiased edges included.
        out.push(Element::Brightened {
            factor: EVENT_BRIGHTNESS,
            class: "eventWrapper".into(),
            children: vec![node],
        });
    } else {
        out.push(node);
    }
    Rect::new(x, y, w, h)
}

/// The title: a bold `4ex` text at (`x`, 20). Chromium's `ex` is the font's
/// x-height (the measuring face's) and its text box the rounded ascent and descent. It
/// has no class or fill of its own, so it paints in the diagram root's `fill`,
/// the theme's `textColor`.
fn title_text(t: &str, x: f64, font: f64, pal: &Palette) -> (Element, Rect) {
    let size = 4.0 * crate::text::face().x_height_em(false) * font;
    let line = crate::text::label_lines(t).join(" ");
    let w = line_width(&line, size, true);
    let (asc, desc) = (text::ascent_px(size), text::descent_px(size));
    let el = Element::classed_text(
        Text {
            x,
            y: 20.0,
            lines: vec![line],
            line_height: size,
            font_size: size,
            bold: true,
            weight: None,
            anchor: crate::scene::Anchor::Start,
            fill: pal.text_color.clone(),
            tspan_central: false,
            dy_tspans: false,
            family: None,
        },
        "titleText",
    );
    (el, Rect::new(x, 20.0 - asc, w, asc + desc))
}

/// Lay out and draw a parsed timeline in its direction.
pub fn draw(tl: &Timeline, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    match tl.direction {
        Direction::LeftRight => draw_horizontal(tl, cfg, pal),
        Direction::TopDown => draw_vertical(tl, cfg, pal),
    }
}

/// `drawNode`'s styling for this configuration: the redux themes' sections
/// (any theme whose name has "redux" in it), and under the neo look their
/// drop shadow or, with `useGradient` (not `neutral`), the gradient outline.
fn node_style<'a>(cfg: &MermaidConfig, pal: &'a Palette, vertical: bool) -> NodeStyle<'a> {
    let name = pal.theme.name();
    let neo = cfg.look == crate::config::Look::Neo;
    let dark = name.contains("dark");
    let redux = name.contains("redux").then(|| Redux {
        dark_color: dark && pal.color_slots.is_some(),
        dark,
        // `feDropShadow dx=4 dy=4 stdDeviation=0`, 20% white on a theme with
        // "dark" in its name, 6% black otherwise.
        shadow: neo.then(|| Shadow {
            dx: 4.0,
            dy: 4.0,
            std_dev: 0.0,
            color: Color::new(if dark {
                "rgba(255, 255, 255, 0.2)"
            } else {
                "rgba(0, 0, 0, 0.06)"
            }),
            linear_rgb: true,
            css: false,
        }),
    });
    let gradient = pal
        .node_gradient
        .as_ref()
        .filter(|_| neo && pal.theme != crate::theme::ThemeName::Neutral)
        .map(|(a, b)| (!vertical).then(|| Gradient::across_bbox(a, b)));
    NodeStyle {
        pal,
        // The stylesheet's `font-size` is the theme's: a top-level `fontSize`
        // (which `pal` carries) only sizes `conf.fontSize`'s em.
        font: match cfg.font_size {
            Some(_) => cfg.stylesheet_font_size(),
            None => pal.font_size,
        },
        conf_font: cfg.font_size.unwrap_or(16.0),
        redux,
        gradient,
        event_offset: !vertical,
        neo,
    }
}

/// The `<defs>` of the drawing: the arrowhead marker, then what `drawNode`
/// added once the first node was drawn (the redux themes' drop shadow under
/// the neo look) and, last, the gradient the horizontal renderer defines.
fn defs_markup(st: &NodeStyle, mk: &Mk, marker_id: &str, svg_id: &str, gradient: bool) -> String {
    let mut out = format!(
        "<defs><marker id=\"{marker_id}-arrowhead\" refX=\"5\" refY=\"2\" markerWidth=\"6\" markerHeight=\"4\" orient=\"auto\"><path d=\"M 0,0 V 4 L6,2 Z\"/></marker>"
    );
    if let (true, Some(rx), true) = (st.neo, &st.redux, mk.nodes > 0) {
        let (opacity, colour) = if rx.dark {
            ("0.2", "#FFFFFF")
        } else {
            ("0.06", "#000000")
        };
        let _ = write!(
            out,
            "<filter id=\"{svg_id}-drop-shadow\" height=\"130%\" width=\"130%\"><feDropShadow dx=\"4\" dy=\"4\" stdDeviation=\"0\" flood-opacity=\"{opacity}\" flood-color=\"{colour}\"/></filter>"
        );
    }
    if let (true, Some((a, b))) = (gradient, st.pal.node_gradient.as_ref()) {
        let _ = write!(
            out,
            "<linearGradient id=\"{svg_id}-gradient\" gradientUnits=\"objectBoundingBox\" x1=\"0%\" y1=\"0%\" x2=\"100%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"{}\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"{}\" stop-opacity=\"1\"/></linearGradient>",
            esc(a.css()),
            esc(b.css()),
        );
    }
    out.push_str("</defs>");
    out
}

/// The title's text as the DOM holds it: `sanitizeText`'s DOMPurify output is
/// HTML text, which `.text()` then escapes once more on serialisation.
fn title_markup_text(t: &str) -> String {
    let plain = crate::text::label_lines(t).join(" ");
    esc_text(&esc_text(&plain))
}

/// The `.5em + 20` and `2em` offsets of the vertical axis are in
/// `conf.fontSize`'s em; `getBBox` numbers are single precision.
fn f32d(v: f64) -> f64 {
    f64::from(v as f32)
}

/// The tasks of the sections named `name` (upstream matches them by name, so
/// two sections of one name each get both's tasks).
fn tasks_named<'a>(tl: &'a Timeline, name: &str) -> Vec<&'a Period> {
    tl.sections
        .iter()
        .filter(|s| s.name.as_deref() == Some(name))
        .flat_map(|s| s.periods.iter())
        .collect()
}

/// The `.lineWrapper line` rules: the last section's `cScaleLabel` over the
/// lines' own 2px (events) and 4px (axis) widths; the redux themes'
/// `nodeBorder` at their `strokeWidth`, the axis too.
fn line_paint(st: &NodeStyle, own_width: f64) -> Paint {
    match &st.redux {
        Some(_) => Paint::stroke(&st.pal.node_border, st.pal.stroke_width.unwrap_or(2.0)),
        None => Paint::stroke(st.pal.scale_text_color(11), own_width),
    }
}

/// `timelineRendererVertical.ts`: periods in a column left of a vertical axis,
/// their events stacked to its right, each joined to the axis by a dashed
/// line, section headers spanning both. Upstream's lines reference a marker id
/// that does not exist (`#arrowhead`), so they have no arrowheads.
fn draw_vertical(tl: &Timeline, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    const NODE_WIDTH: f64 = 200.0;
    const PAD: f64 = 5.0;
    const EVENT_WIDTH: f64 = NODE_WIDTH + 100.0;
    const EVENT_SPACING: f64 = 10.0;
    const SECTION_TASK_GAP: f64 = 20.0;
    const TASK_AXIS_GAP: f64 = 20.0;
    const TASK_VERTICAL_GAP: f64 = 30.0;
    const EVENT_AXIS_GAP: f64 = 50.0;
    let multicolor = !cfg.section_bool("timeline", "disableMulticolor", false);
    let left_margin = cfg.section_f64("timeline", "leftMargin", 150.0);
    let padding = cfg.section_f64("timeline", "padding", 50.0);
    let style = node_style(cfg, pal, true);
    let names: Vec<&str> = tl
        .sections
        .iter()
        .filter_map(|s| s.name.as_deref())
        .collect();
    let has_sections = !names.is_empty();
    let tasks: Vec<&Period> = tl.sections.iter().flat_map(|s| s.periods.iter()).collect();

    let master_x = 50.0 + left_margin;
    let left_w = NODE_WIDTH + 2.0 * PAD + TASK_AXIS_GAP;
    let right_w = EVENT_WIDTH + 2.0 * PAD + EVENT_AXIS_GAP;
    let axis_x = master_x + left_w;
    let section_w = (left_w + right_w - 2.0 * PAD).max(50.0);
    let height = |raw: &str, width: f64| style.virtual_height(raw, width, PAD);

    let max_section_h = names
        .iter()
        .map(|n| height(n, section_w))
        .fold(0.0, f64::max);
    let max_task_h = if tasks.is_empty() {
        0.0
    } else {
        height(TASK_OBJECT_TEXT, NODE_WIDTH)
    };
    let max_events_h = tasks
        .iter()
        .map(|t| {
            let n = t.events.len() as f64;
            t.events.iter().map(|e| height(e, EVENT_WIDTH)).sum::<f64>()
                + if n > 0.0 {
                    (n - 1.0) * EVENT_SPACING
                } else {
                    0.0
                }
        })
        .fold(0.0, f64::max);
    let task_spacing = max_task_h.max(max_events_h) + TASK_VERTICAL_GAP;

    let mut bounds = Bounds::default();
    let mut nodes = Vec::new();
    let mut lines = Vec::new();
    let mut mk = Mk::new("undefined");
    let draw_tasks = |nodes: &mut Vec<Element>,
                      lines: &mut Vec<Element>,
                      mk: &mut Mk,
                      bounds: &mut Bounds,
                      periods: &[&Period],
                      mut color: usize,
                      mut y: f64,
                      without_sections: bool| {
        let mut cur_max = max_task_h;
        for p in periods {
            let w = NODE_WIDTH + 2.0 * PAD;
            let r = draw_node(
                nodes,
                mk,
                &style,
                axis_x - TASK_AXIS_GAP - w,
                y,
                &p.label,
                NODE_WIDTH,
                PAD,
                cur_max,
                color,
                Kind::Task,
            );
            bounds.add_rect(r);
            cur_max = cur_max.max(r.h);
            let mut ey = y;
            for e in &p.events {
                let ex = axis_x + EVENT_AXIS_GAP;
                let r = draw_node(
                    nodes,
                    mk,
                    &style,
                    ex,
                    ey,
                    e,
                    EVENT_WIDTH,
                    PAD,
                    0.0,
                    color,
                    Kind::Event,
                );
                bounds.add_rect(r);
                let ly = ey + r.h / 2.0;
                mk.line(
                    Point::new(axis_x, ly),
                    Point::new(ex, ly),
                    2,
                    "arrowhead",
                    true,
                );
                lines.push(Element::classed_shape(
                    Geometry::Line {
                        a: Point::new(axis_x, ly),
                        b: Point::new(ex, ly),
                    },
                    line_paint(&style, 2.0).dashed(vec![5.0, 5.0]),
                    "eventLine",
                ));
                ey = ey + r.h + EVENT_SPACING;
            }
            y += task_spacing;
            if without_sections && multicolor {
                color += 1;
            }
        }
    };

    let mut master_y = 50.0;
    if has_sections {
        for (si, name) in names.iter().enumerate() {
            let r = draw_node(
                &mut nodes,
                &mut mk,
                &style,
                axis_x - left_w,
                master_y,
                name,
                section_w,
                PAD,
                max_section_h,
                si,
                Kind::Section,
            );
            bounds.add_rect(r);
            let periods = tasks_named(tl, name);
            if !periods.is_empty() {
                draw_tasks(
                    &mut nodes,
                    &mut lines,
                    &mut mk,
                    &mut bounds,
                    &periods,
                    si,
                    master_y + r.h + SECTION_TASK_GAP,
                    false,
                );
            }
            let n = periods.len();
            master_y += r.h + SECTION_TASK_GAP + task_spacing * n.max(1) as f64
                - if n > 0 { 2.0 * TASK_VERTICAL_GAP } else { 0.0 };
        }
    } else {
        draw_tasks(
            &mut nodes,
            &mut lines,
            &mut mk,
            &mut bounds,
            &tasks,
            0,
            master_y,
            true,
        );
    }

    let mut content = bounds.rect();
    let mut title_mk = String::new();
    if let Some(t) = tl.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let (el, r) = title_text(t, content.w / 2.0 - left_margin, style.font, pal);
        let _ = write!(
            title_mk,
            "<text x=\"{}\" font-size=\"4ex\" font-weight=\"bold\" y=\"20\">{}</text>",
            n(f32d(content.w) / 2.0 - left_margin),
            title_markup_text(t),
        );
        bounds.add_rect(r);
        nodes.push(el);
        content = bounds.rect();
    }
    // The axis: from 2em above the content top to 0.5em + 20 below its
    // bottom, in `conf.fontSize`'s em.
    let em = style.conf_font;
    let (y1, y2) = (50.0 - 2.0 * em, content.bottom() + 0.5 * em + 20.0);
    let y2_markup = f32d(content.y) + f32d(content.h) + (em * 0.5 + 20.0);
    let axis_mk = format!(
        "<g class=\"lineWrapper\"><line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\" stroke-width=\"4\" stroke=\"black\" marker-end=\"url(#arrowhead)\"/></g>",
        n(y1),
        n(y2_markup),
        x = n(axis_x),
    );
    let mut back = vec![Element::classed_shape(
        Geometry::Line {
            a: Point::new(axis_x, y1),
            b: Point::new(axis_x, y2),
        },
        line_paint(&style, 4.0),
        "axis",
    )];
    bounds.add_rect(Rect::new(axis_x, y1, 0.0, y2 - y1));
    back.extend(lines);
    // The axis is lowered to be the root's first child, ahead of the styles.
    let svg = format!(
        "{axis_mk}{BEFORE_STYLE}<g/><g/>{}{}{title_mk}",
        defs_markup(&style, &mk, "undefined", ID_MARK, false),
        mk.s,
    );
    Scene {
        kind: DiagramKind::Timeline,
        view_box: graph_view_box(bounds.rect(), padding),
        elements: vec![Element::Markup {
            svg,
            svg_view_box: None,
            children: vec![
                Element::group("lines", back),
                Element::group("nodes", nodes),
            ],
        }],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// `timelineRenderer.ts` (the horizontal layout) and `svgDraw.js`'s `drawNode`.
fn draw_horizontal(tl: &Timeline, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let multicolor = !cfg.section_bool("timeline", "disableMulticolor", false);
    let left_margin = cfg.section_f64("timeline", "leftMargin", 150.0);
    let padding = cfg.section_f64("timeline", "padding", 50.0);
    let style = node_style(cfg, pal, false);
    let neo = cfg.look == crate::config::Look::Neo;
    let names: Vec<&str> = tl
        .sections
        .iter()
        .filter_map(|s| s.name.as_deref())
        .collect();
    let has_sections = !names.is_empty();
    // The pre-pass looks at every task, those before the first section too.
    let all: Vec<&Period> = tl.sections.iter().flat_map(|s| s.periods.iter()).collect();

    // The pre-pass: tallest section, tallest period, tallest event stack.
    let max_section_h = names
        .iter()
        .map(|n| style.virtual_height(n, NODE_W, NODE_PAD) + 20.0)
        .fold(0.0, f64::max);
    let max_task_h = if all.is_empty() {
        0.0
    } else {
        style.virtual_height(TASK_OBJECT_TEXT, NODE_W, NODE_PAD) + 20.0
    };
    let max_event_len = all
        .iter()
        .map(|p| {
            let n = p.events.len() as f64;
            p.events
                .iter()
                .map(|e| style.virtual_height(e, NODE_W, NODE_PAD))
                .sum::<f64>()
                + if n > 0.0 { (n - 1.0) * 10.0 } else { 0.0 }
        })
        .fold(0.0, f64::max);

    let mut bounds = Bounds::default();
    // In upstream's document order: per period its node, its (empty, filled
    // later) `lineWrapper`, its events; then the title and the axis on top.
    let mut nodes = Vec::new();
    let mut mk = Mk::new(ID_MARK);
    // The arrowhead marker's path has no class: it fills with the svg root's
    // `fill: textColor`.
    let arrow_fill = pal.text_color.clone();

    // drawTasks: the periods of one section (or all, without sections), from
    // (master_x, master_y), `maxTaskHeight` growing with each node.
    let draw_tasks = |nodes: &mut Vec<Element>,
                      mk: &mut Mk,
                      bounds: &mut Bounds,
                      periods: &[&Period],
                      mut color: usize,
                      master_x: f64,
                      master_y: f64,
                      without_sections: bool| {
        let mut cur_max = max_task_h;
        let mut x = master_x;
        for p in periods {
            let r = draw_node(
                nodes,
                mk,
                &style,
                x,
                master_y,
                &p.label,
                NODE_W,
                NODE_PAD,
                cur_max,
                color,
                Kind::Task,
            );
            bounds.add_rect(r);
            cur_max = cur_max.max(r.h);
            // The dashed drop line with its arrowhead (marker scaled by 2),
            // under the events.
            let lx = x + 190.0 / 2.0;
            let y1 = master_y + cur_max;
            let y2 = y1 + 100.0 + max_event_len + 100.0;
            mk.line(
                Point::new(lx, y1),
                Point::new(lx, y2),
                2,
                &format!("{ID_MARK}-arrowhead"),
                true,
            );
            nodes.push(Element::classed_shape(
                Geometry::Line {
                    a: Point::new(lx, y1),
                    b: Point::new(lx, y2),
                },
                line_paint(&style, 2.0).dashed(vec![5.0, 5.0]),
                "eventLine",
            ));
            // `M 0,0 V 4 L6,2 Z` with refX 5, refY 2, scaled by 2: from 10px
            // behind the end to 2px past it, 8px wide.
            nodes.push(marker(
                &[
                    Point::new(lx - 4.0, y2 - 10.0),
                    Point::new(lx, y2 + 2.0),
                    Point::new(lx + 4.0, y2 - 10.0),
                ],
                &arrow_fill,
            ));
            bounds.add_rect(Rect::new(lx - 1.0, y1, 2.0, y2 - y1));
            let mut ey = master_y + 100.0;
            ey += 100.0;
            for e in &p.events {
                let r = draw_node(
                    nodes,
                    mk,
                    &style,
                    x,
                    ey,
                    e,
                    NODE_W,
                    NODE_PAD,
                    50.0,
                    color,
                    Kind::Event,
                );
                bounds.add_rect(r);
                ey = ey + 10.0 + r.h;
            }
            x += 200.0;
            if without_sections && multicolor {
                color += 1;
            }
        }
    };

    let mut master_x = 50.0 + left_margin;
    let section_y = 50.0;
    if has_sections {
        for (si, name) in names.iter().enumerate() {
            let periods = tasks_named(tl, name);
            let count = periods.len().max(1) as f64;
            let r = draw_node(
                &mut nodes,
                &mut mk,
                &style,
                master_x,
                section_y,
                name,
                200.0 * count - 50.0,
                NODE_PAD,
                max_section_h,
                si,
                Kind::Section,
            );
            bounds.add_rect(r);
            if !periods.is_empty() {
                let master_y = 50.0 + max_section_h + 50.0;
                draw_tasks(
                    &mut nodes,
                    &mut mk,
                    &mut bounds,
                    &periods,
                    si,
                    master_x,
                    master_y,
                    false,
                );
            }
            master_x += 200.0 * count;
        }
    } else {
        draw_tasks(
            &mut nodes,
            &mut mk,
            &mut bounds,
            &all,
            0,
            master_x,
            50.0,
            true,
        );
    }

    let content = bounds.rect();
    let mut title_mk = String::new();
    if let Some(t) = tl.title.as_ref().filter(|t| !t.trim().is_empty()) {
        // At (width / 2 - leftMargin, 20); under the neo look at
        // (2·x + leftMargin, 20), x the drawing's left edge.
        let x = if neo {
            2.0 * content.x + left_margin
        } else {
            content.w / 2.0 - left_margin
        };
        let x_markup = if neo {
            f32d(content.x) * 2.0 + left_margin
        } else {
            f32d(content.w) / 2.0 - left_margin
        };
        let (el, r) = title_text(t, x, style.font, pal);
        let _ = write!(
            title_mk,
            "<text x=\"{}\" font-size=\"4ex\" font-weight=\"bold\" y=\"20\">{}</text>",
            n(x_markup),
            title_markup_text(t),
        );
        bounds.add_rect(r);
        nodes.push(el);
    }

    // The axis, drawn last: 4px (redux: the theme's `strokeWidth`) from
    // leftMargin to the drawing's width + 3·leftMargin, its marker scaled by
    // the stroke's width.
    let depth_y = if has_sections {
        max_section_h + max_task_h + 150.0
    } else {
        max_task_h + 100.0
    };
    let x2 = content.w + 3.0 * left_margin;
    let axis_mk = format!(
        "<g class=\"lineWrapper\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"4\" stroke=\"black\" marker-end=\"url(#{ID_MARK}-arrowhead)\"/></g>",
        n(left_margin),
        n(depth_y),
        n(f32d(content.w) + 3.0 * left_margin),
        n(depth_y),
    );
    let axis = line_paint(&style, 4.0);
    // `M 0,0 V 4 L6,2 Z`, refX 5, refY 2.
    let k = axis.stroke_width;
    let back = vec![
        Element::classed_shape(
            Geometry::Line {
                a: Point::new(left_margin, depth_y),
                b: Point::new(x2, depth_y),
            },
            axis,
            "axis",
        ),
        marker(
            &[
                Point::new(x2 - 5.0 * k, depth_y - 2.0 * k),
                Point::new(x2 + k, depth_y),
                Point::new(x2 - 5.0 * k, depth_y + 2.0 * k),
            ],
            &arrow_fill,
        ),
    ];
    bounds.add_rect(Rect::new(left_margin, depth_y - 2.0, x2 - left_margin, 4.0));

    // setupGraphViewbox with `timeline.padding`.
    let view_box = graph_view_box(bounds.rect(), padding);
    let mut elements = vec![
        Element::group("nodes", nodes),
        Element::group("lines", back),
    ];
    snap_marker_clips(&mut elements, view_box);
    let svg = format!(
        "<g/><g/>{}{}{title_mk}{axis_mk}",
        defs_markup(&style, &mk, ID_MARK, ID_MARK, style.gradient.is_some()),
        mk.s,
    );
    Scene {
        kind: DiagramKind::Timeline,
        view_box,
        elements: vec![Element::Markup {
            svg,
            svg_view_box: None,
            children: elements,
        }],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// The arrowhead marker's triangle in user space, clipped to the marker's
/// viewport (`markerWidth` 6 × `markerHeight` 4, which the path fills);
/// [`snap_marker_clips`] then puts the clip's edges where Chromium does.
fn marker(pts: &[Point], fill: &Color) -> Element {
    let mut b = Bounds::default();
    for p in pts {
        b.add_point(*p);
    }
    Element::Clipped {
        rect: b.rect(),
        children: vec![Element::path(geom::polygon(pts), Paint::fill(fill))],
    }
}

/// Chromium clips a marker to its viewport with the edges snapped to the
/// nearest device pixel: drawn 1:1, a whole unit from the view box's origin.
fn snap_marker_clips(els: &mut [Element], vb: Rect) {
    for e in els {
        match e {
            Element::Clipped { rect, .. } => {
                let x0 = (rect.x - vb.x).round() + vb.x;
                let y0 = (rect.y - vb.y).round() + vb.y;
                let x1 = (rect.right() - vb.x).round() + vb.x;
                let y1 = (rect.bottom() - vb.y).round() + vb.y;
                *rect = Rect::new(x0, y0, x1 - x0, y1 - y0);
            }
            Element::Group { children, .. } => snap_marker_clips(children, vb),
            _ => {}
        }
    }
}

/// `setupGraphViewbox`: the drawing's `getBBox()`, each of its numbers a
/// single-precision float, grown by `padding` on every side (the SVG writer
/// prints a timeline's view box in full).
fn graph_view_box(r: Rect, padding: f64) -> Rect {
    let f = |v: f64| f64::from(v as f32);
    Rect::new(f(r.x), f(r.y), f(r.w), f(r.h)).pad(padding)
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

/// `drawNode`'s background (`defaultBkg`), from the node's origin: a box with
/// rounded top corners (square under the redux themes) and, outside redux, a
/// `cScaleInv` rule along its bottom (`.section-N path` / `.section-N line`).
fn node_box(r: Rect, ci: usize, st: &NodeStyle) -> Element {
    let pal = st.pal;
    let (w, h) = (r.w, r.h);
    let p = |x: f64, y: f64| Point::new(r.x + x, r.y + y);
    // `M0 {h - 5}` and up, as mermaid's path runs.
    let mut d = vec![PathCmd::MoveTo(p(0.0, h - 5.0))];
    if st.redux.is_some() {
        d.extend([
            PathCmd::LineTo(p(0.0, 0.0)),
            PathCmd::LineTo(p(w, 0.0)),
            PathCmd::LineTo(p(w, h)),
        ]);
    } else {
        d.extend([
            PathCmd::LineTo(p(0.0, 5.0)),
            quad(p(0.0, 5.0), p(0.0, 0.0), p(5.0, 0.0)),
            PathCmd::LineTo(p(w - 5.0, 0.0)),
            quad(p(w - 5.0, 0.0), p(w, 0.0), p(w, 5.0)),
            PathCmd::LineTo(p(w, h)),
        ]);
    }
    d.extend([PathCmd::LineTo(p(0.0, h)), PathCmd::Close]);
    let slot = ci % pal.scale.len().max(1);
    let mut paint = match &st.redux {
        Some(rx) => {
            let slot = pal
                .color_slots
                .as_ref()
                .map(|cp| cp.border[slot % cp.border.len()].clone());
            let fill = match &slot {
                Some(c) if !rx.dark_color => c.clone(),
                _ => pal.main_bkg.clone(),
            };
            let stroke = slot.unwrap_or_else(|| pal.node_border.clone());
            Paint::both(&fill, &stroke, pal.stroke_width.unwrap_or(2.0))
                .with_shadow(rx.shadow.clone())
        }
        None => Paint::fill(pal.scale_color(ci)),
    };
    if let Some(g) = &st.gradient {
        let shadow = paint.shadow.take();
        paint = match g {
            Some(g) => {
                Paint::both(&pal.main_bkg, &pal.main_bkg, 2.0).with_stroke_gradient(g.clone())
            }
            None => Paint::fill(&pal.main_bkg),
        }
        .with_shadow(shadow);
    }
    let mut els = vec![Element::classed_shape(Geometry::Path(d), paint, "node-bkg")];
    if st.redux.is_none() {
        // Under the gradient rule (`stroke: url(#{id}-gradient)`) the line
        // paints nothing: the gradient spans its empty bounding box, or
        // (`timeline TD`) is not defined.
        let paint = match &st.gradient {
            None => Paint::stroke(pal.scale_inv_color(ci), 3.0),
            Some(Some(g)) => Paint::stroke(&Color::none(), 2.0).with_stroke_gradient(g.clone()),
            // Written with a gradient too: the outline is there, painting nothing.
            Some(None) => match pal.node_gradient.as_ref() {
                Some((a, b)) => Paint::stroke(&Color::none(), 2.0)
                    .with_stroke_gradient(Gradient::across_bbox(a, b)),
                None => Paint::stroke(&Color::none(), 2.0),
            },
        };
        els.push(Element::shape(
            Geometry::Line {
                a: p(0.0, h),
                b: p(w, h),
            },
            paint,
        ));
    }
    Element::group("node", els)
}

/// timeline/styles.js: `.eventWrapper { filter: brightness(120%) }`.
const EVENT_BRIGHTNESS: f64 = 1.2;

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
    fn node_text_boxes_as_chromium_measures_middle_baselines() {
        // getBBox() in headless Chromium of the node text (dy 1em, then 1.1em,
        // dominant-baseline middle) at 16px: 17.015625 for a line ending on the
        // font's descent, 18 with a descender, 1.1em more per line.
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(middle_text_height(&l(&["xxxx"]), 16.0), 17.015625);
        assert_eq!(middle_text_height(&l(&["gggg"]), 16.0), 18.0);
        assert!((middle_text_height(&l(&["a", "b"]), 16.0) - 34.615625).abs() < 1e-9);
        assert_eq!(middle_text_height(&l(&["xxxx"]), 14.0), 16.0);
    }

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
        let c = Color::new("#d7ff86").brightened(EVENT_BRIGHTNESS);
        assert_eq!(c.css(), "#ffffa1");
        assert_eq!(Color::new("#000000").brightened(1.2).css(), "#000000");
        // Each event is an `eventWrapper` group carrying the filter over its
        // box and text in their own (unbrightened) section colours.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render("timeline\n2002 : LinkedIn : Facebook", None, &cfg, &pal).unwrap();
        let Element::Markup { children: top, .. } = &s.elements[0] else {
            panic!()
        };
        let Element::Group { children, .. } = &top[0] else {
            panic!()
        };
        let events: Vec<&Vec<Element>> = children
            .iter()
            .filter_map(|e| match e {
                Element::Brightened {
                    factor,
                    class,
                    children,
                } if *factor == 1.2 && class == "eventWrapper" => Some(children),
                _ => None,
            })
            .collect();
        assert_eq!(events.len(), 2);
        // The node translated to its place, its box and its text's group.
        let [Element::Translated { children: node, .. }] = &events[0][..] else {
            panic!()
        };
        let Element::Group { children: bkg, .. } = &node[0] else {
            panic!()
        };
        let Element::Shape { paint, .. } = &bkg[0] else {
            panic!()
        };
        assert_eq!(&paint.fill, pal.scale_color(0));
        let Element::Translated {
            children: label, ..
        } = &node[1]
        else {
            panic!()
        };
        let Element::Text { text, class } = &label[0] else {
            panic!()
        };
        assert_eq!(class.as_deref(), Some("eventText"));
        assert_eq!(&text.fill, pal.scale_text_color(0));
        // The period's node is not brightened.
        assert!(matches!(&children[0], Element::Translated { .. }));
    }

    #[test]
    fn layout_follows_the_horizontal_renderer() {
        // mermaid 12.3.0, the gallery's timeline: viewBox `100 -61 1390 722.6`
        // sections 66.8 tall at y 50, periods at y 166.8, events from y 366.8,
        // the axis at y 283.6.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let src = include_str!("../../examples/diagrams/timeline.mmd");
        let s = render(src, None, &cfg, &pal).unwrap();
        let v = s.view_box;
        assert_eq!((v.x, v.y, v.w), (100.0, -61.0, 1390.0));
        assert!((v.bottom() - 661.6).abs() < 0.1, "{v:?}");
        // The axis is drawn last, over the drop lines.
        let Element::Markup { children: top, .. } = &s.elements[0] else {
            panic!()
        };
        let Element::Group { children, .. } = top.last().unwrap() else {
            panic!()
        };
        let axis = children
            .iter()
            .find_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Line { a, b },
                    paint,
                    ..
                } if paint.stroke_width == 4.0 => Some((*a, *b)),
                _ => None,
            })
            .unwrap();
        assert_eq!(axis.0, Point::new(150.0, 283.6));
        assert_eq!(axis.1, Point::new(1440.0, 283.6));
    }

    /// mermaid 12.3.0 (a timeline with periods 2002 and 2004): the markers'
    /// `M 0,0 V 4 L6,2 Z` (refX 5) fill with the root's `fill: #333` (the
    /// theme's textColor); a drop line's (stroke 2) runs from 10px before its
    /// end (y 419.415625) to 2px past it, 8px wide; each drop line follows its
    /// period's node and comes before its events; the axis comes last.
    #[test]
    fn arrowheads_and_drawing_order_follow_upstream() {
        fn walk<'a>(e: &'a [Element], out: &mut Vec<&'a Element>) {
            for x in e {
                match x {
                    Element::Group { children, .. }
                    | Element::Markup { children, .. }
                    | Element::Brightened { children, .. }
                    | Element::Clipped { children, .. }
                    | Element::Translated { children, .. } => walk(children, out),
                    _ => out.push(x),
                }
            }
        }
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(
            "timeline\ntitle History\n2002 : LinkedIn\n2004 : Facebook : Google",
            None,
            &cfg,
            &pal,
        )
        .unwrap();
        let mut flat = Vec::new();
        walk(&s.elements, &mut flat);
        let heads: Vec<&Vec<PathCmd>> = flat
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(c),
                    paint,
                    ..
                } if paint.fill.css() == pal.text_color.css() => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(heads.len(), 3, "two drop lines and the axis");
        assert_eq!(pal.text_color.css(), "#333");
        let y_end = 419.415625;
        let PathCmd::MoveTo(p) = heads[0][0] else {
            panic!()
        };
        assert_eq!(p.x, 291.0);
        assert!((p.y - (y_end - 10.0)).abs() < 1e-3, "{p:?}");
        let PathCmd::LineTo(p) = heads[0][1] else {
            panic!()
        };
        assert!((p.y - (y_end + 2.0)).abs() < 1e-3, "{p:?}");
        let pos = |pred: &dyn Fn(&Element) -> bool| flat.iter().position(|e| pred(e)).unwrap();
        let text = |t: &'static str| move |e: &Element| matches!(e, Element::Text { text, .. } if text.lines.iter().any(|l| l == t));
        let line =
            |e: &Element| matches!(e, Element::Shape { class: Some(c), .. } if c == "eventLine");
        assert!(pos(&text("2002")) < pos(&line));
        assert!(pos(&line) < pos(&text("LinkedIn")));
        let axis = |e: &Element| matches!(e, Element::Shape { class: Some(c), .. } if c == "axis");
        assert_eq!(pos(&axis), flat.len() - 2, "the axis and its head last");
    }

    fn view_box(src: &str) -> Rect {
        let cfg = MermaidConfig::default();
        render(src, None, &cfg, &cfg.palette()).unwrap().view_box
    }

    #[test]
    fn period_heights_are_measured_from_the_task_object() {
        // Upstream's pre-pass measures "[object Object]" (with descenders) for
        // every period, so short labels still get 18px text boxes: mermaid's
        // viewBox here is `100 -61 1190 530.4`.
        let v = view_box(
            "timeline\ntitle History of Social Media Platform\n2002 : LinkedIn\n2004 : Facebook : Google\n2005 : YouTube\n2006 : Twitter",
        );
        assert_eq!((v.x, v.y, v.w), (100.0, -61.0, 1190.0));
        assert!((v.h - 530.4).abs() < 0.05, "{v:?}");
    }

    #[test]
    fn top_down_follows_the_vertical_renderer() {
        assert_eq!(
            parse("timeline TD\n2002 : a", None).unwrap().direction,
            Direction::TopDown
        );
        assert_eq!(
            parse("timeline\n2002 : a", None).unwrap().direction,
            Direction::LeftRight
        );
        // mermaid 12.3.0 viewBoxes. The gallery timeline as `timeline TD`:
        // `95 -61 745 873` (sections overlap the previous section's last
        // events, as upstream's section height subtracts two task gaps).
        let src = include_str!("../../examples/diagrams/timeline.mmd").replacen(
            "timeline",
            "timeline TD",
            1,
        );
        let v = view_box(&src);
        assert_eq!((v.x, v.y, v.w, v.h), (95.0, -61.0, 745.0, 873.0));
        // Without sections: periods left of the axis at x 430, events right.
        let v = view_box(
            "timeline TD\n2002 : LinkedIn\n2004 : Facebook : Google\n2005 : YouTube\n2006 : Twitter with a much longer event text that will need to wrap over several lines here",
        );
        assert_eq!((v.x, v.y, v.w), (150.0, -32.0, 690.0));
        assert!((v.h - 517.247).abs() < 0.1, "{v:?}");
        let v = view_box(
            "timeline TD\ntitle My day\nsection Morning\nWake up : Coffee\nsection Evening\nSleep",
        );
        assert_eq!((v.x, v.y, v.w), (95.0, -61.0, 745.0));
        assert!((v.h - 326.2).abs() < 0.01, "{v:?}");
    }
}
