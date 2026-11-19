//! `journey` (user journey) diagrams.
//!
//! Layout follows mermaid's `journeyRenderer`: an actor legend on the left, then a
//! row of sections (coloured header spanning its tasks), each task a 150×50 box
//! with actor dots on its top edge, a dashed drop line and a face whose height and
//! expression encode the 1–5 score, and an arrowed activity line across them.

use super::timeline::wrap_text;
use crate::config::MermaidConfig;
use crate::diagram::wardley::ID_MARK;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::output::svg::{esc, esc_text, js_num as n};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::Palette;
use std::f64::consts::PI;
use std::fmt::Write;

/// mermaid's default `journey.actorColours`.
const ACTOR_COLORS: [&str; 6] = [
    "#8FBC8F", "#7CFC00", "#00FFFF", "#20B2AA", "#B0E0E6", "#FFFFE0",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub label: String,
    pub score: f64,
    pub actors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Journey {
    pub title: Option<String>,
    pub sections: Vec<Section>,
}

impl Journey {
    /// Distinct actors, sorted (journeyDb.js `[...new Set(actors)].sort()`, which
    /// orders the legend and assigns actor colours).
    pub fn actors(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self
            .sections
            .iter()
            .flat_map(|s| &s.tasks)
            .flat_map(|t| &t.actors)
        {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
        // JS's default sort compares UTF-16 code units; for the BMP that is char order.
        out.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        out
    }
}

/// Parse a journey body.
pub fn parse(body: &str, title: Option<String>) -> Result<Journey> {
    let mut j = Journey {
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
            if line != "journey" {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nExpecting 'journey'"),
                ));
            }
            header = true;
            continue;
        }
        if let Some(t) = line
            .strip_prefix("title")
            .filter(|r| r.is_empty() || r.starts_with([' ', '\t']))
        {
            j.title = Some(t.trim().to_string());
        } else if let Some(s) = line
            .strip_prefix("section")
            .filter(|r| r.starts_with([' ', '\t']))
        {
            j.sections.push(Section {
                name: s.trim().to_string(),
                tasks: Vec::new(),
            });
        } else if let Some(colon) = line.find(':') {
            let label = line[..colon].trim();
            let mut data = line[colon + 1..].splitn(2, ':');
            let score_txt = data.next().unwrap_or("").trim();
            // journeyDb addTask: `Number(score)` — blank is 0, anything else NaN.
            let score: f64 = if score_txt.is_empty() {
                0.0
            } else {
                score_txt.parse().unwrap_or(f64::NAN)
            };
            if label.is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nExpecting a task name"),
                ));
            }
            let actors = data
                .next()
                .map(|a| {
                    a.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if j.sections.is_empty() {
                j.sections.push(Section {
                    name: String::new(),
                    tasks: Vec::new(),
                });
            }
            if let Some(sec) = j.sections.last_mut() {
                sec.tasks.push(Task {
                    label: label.to_string(),
                    score,
                    actors,
                });
            }
        } else {
            return Err(Error::parse(
                line_no,
                format!("{line}\nExpecting 'title', 'section' or 'task: score: actors'"),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'journey'"));
    }
    Ok(j)
}

/// svgDraw `drawFace`: a radius-15 face at `c`, eyes 5 up and to each side, and
/// a mouth by score: smile (> 3, the lower half of a d3 annulus of radii 7.5 and
/// 15/2.2, 2 down), sad (< 3, its upper half, 7 down) or a flat 10px line 7
/// down. The annulus has no fill of its own, so it paints in the root's `fill`
/// (`ink`, the theme's `textColor`) under the `.mouth` stroke (#666, 1px).
fn face(c: Point, score: f64, ink: &Color) -> Vec<Element> {
    let r = 15.0;
    let grey = Color::new("#666666");
    let mut els = vec![
        Element::classed_shape(
            Geometry::Circle { c, r },
            Paint::both(&Color::new("#FFF8DC"), &Color::new("#999999"), 2.0),
            "face",
        ),
        Element::shape(
            Geometry::Circle {
                c: Point::new(c.x - r / 3.0, c.y - r / 3.0),
                r: 1.5,
            },
            Paint::both(&grey, &grey, 2.0),
        ),
        Element::shape(
            Geometry::Circle {
                c: Point::new(c.x + r / 3.0, c.y - r / 3.0),
                r: 1.5,
            },
            Paint::both(&grey, &grey, 2.0),
        ),
    ];
    // d3.arc from `start` to `start + π` (clockwise from 12 o'clock): the outer
    // arc, a radial line in, the inner arc back and close.
    let (outer, inner) = (r / 2.0, r / 2.2);
    let annulus = |center: Point, a0: f64| {
        let a1 = a0 + PI;
        let at = |rad: f64, a: f64| Point::new(center.x + rad * a.cos(), center.y + rad * a.sin());
        let mut cmds = vec![PathCmd::MoveTo(at(outer, a0))];
        cmds.extend(geom::arc_cubics(center, outer, a0, a1));
        cmds.push(PathCmd::LineTo(at(inner, a1)));
        cmds.extend(geom::arc_cubics(center, inner, a1, a0));
        cmds.push(PathCmd::Close);
        cmds
    };
    let (m, paint) = if score > 3.0 {
        (
            annulus(Point::new(c.x, c.y + 2.0), 0.0),
            Paint::both(ink, &grey, 1.0),
        )
    } else if score < 3.0 {
        (
            annulus(Point::new(c.x, c.y + 7.0), PI),
            Paint::both(ink, &grey, 1.0),
        )
    } else {
        (
            vec![
                PathCmd::MoveTo(Point::new(c.x - 5.0, c.y + 7.0)),
                PathCmd::LineTo(Point::new(c.x + 5.0, c.y + 7.0)),
            ],
            Paint::stroke(&grey, 1.0),
        )
    };
    els.push(Element::classed_shape(Geometry::Path(m), paint, "mouth"));
    els
}

/// The journey config mermaid's `svgDraw` / `journeyRenderer` read for the
/// drawing's own attributes (colours, task text and title styling).
struct MarkupConf {
    fills: Vec<String>,
    colours: Vec<String>,
    actor_colours: Vec<String>,
    placement: String,
    task_font_size: String,
    task_font_family: String,
    title_color: String,
    title_font_size: String,
    title_font_family: String,
}

impl MarkupConf {
    fn new(cfg: &MermaidConfig) -> Self {
        let list = |key: &str, default: &[&str]| -> Vec<String> {
            let got: Vec<String> = cfg
                .section("journey", key)
                .and_then(|j| match j {
                    crate::json::Json::Array(a) => Some(
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            if got.is_empty() {
                default.iter().map(|s| s.to_string()).collect()
            } else {
                got
            }
        };
        // A number set through `.style()` is a bare length in a quirks-mode page:
        // it paints as pixels and serialises with `px`.
        let text = |key: &str, default: &str| match cfg.section("journey", key) {
            Some(crate::json::Json::Number(v)) => n(*v),
            Some(crate::json::Json::String(s)) => s.clone(),
            _ => default.to_string(),
        };
        let px = |v: String| {
            if v.parse::<f64>().is_ok() {
                format!("{v}px")
            } else {
                v
            }
        };
        MarkupConf {
            fills: list(
                "sectionFills",
                &[
                    "#191970", "#8B008B", "#4B0082", "#2F4F4F", "#800000", "#8B4513", "#00008B",
                ],
            ),
            colours: list("sectionColours", &["#fff"]),
            actor_colours: list("actorColours", &ACTOR_COLORS),
            placement: text("textPlacement", "fo"),
            task_font_size: px(text("taskFontSize", "14")),
            task_font_family: text("taskFontFamily", "\"Open Sans\", sans-serif"),
            title_color: text("titleColor", ""),
            title_font_size: text("titleFontSize", "4ex"),
            title_font_family: text(
                "titleFontFamily",
                "\"trebuchet ms\", verdana, arial, sans-serif",
            ),
        }
    }

    fn actor_colour(&self, pos: usize) -> String {
        self.actor_colours[pos % self.actor_colours.len()].clone()
    }

    /// svgDraw's `_drawTextCandidateFunc(conf)`: `byFo` (a `foreignObject` with an
    /// HTML label and an SVG-text fallback in a `switch`), `byTspan` or `byText`.
    fn text(&self, mk: &mut String, content: &str, r: Rect, class: &str, colour: &str) {
        let fo = self.placement == "fo";
        if self.placement == "old" {
            let _ = write!(
                mk,
                "<text x=\"{}\" y=\"{}\" class=\"{}\" style=\"text-anchor: middle;\">{}</text>",
                n(r.x + r.w / 2.0),
                n(r.y + r.h / 2.0 + 5.0),
                esc(class),
                esc_text(content),
            );
            return;
        }
        if fo {
            let _ = write!(
                mk,
                "<switch><foreignObject x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"><div class=\"{}\" xmlns=\"http://www.w3.org/1999/xhtml\" style=\"display: table; height: 100%; width: 100%;\"><div class=\"label\" style=\"display: table-cell; text-align: center; vertical-align: middle;\">{}</div></div></foreignObject>",
                n(r.x),
                n(r.y),
                n(r.w),
                n(r.h),
                esc(class),
                esc_text(content),
            );
        }
        let lines = split_br(content);
        let fs = self
            .task_font_size
            .trim_end_matches("px")
            .parse::<f64>()
            .ok();
        for (i, line) in lines.iter().enumerate() {
            let dy = match fs {
                Some(f) => n(i as f64 * f - f * (lines.len() as f64 - 1.0) / 2.0),
                None => "NaN".to_string(),
            };
            let fill = if fo {
                String::new()
            } else {
                format!(" fill=\"{}\"", esc(colour))
            };
            let _ = write!(
                mk,
                "<text x=\"{x}\" y=\"{}\"{fill} dominant-baseline=\"central\" alignment-baseline=\"central\" class=\"{}\" style=\"text-anchor: middle; font-size: {}; font-family: {};\"><tspan x=\"{x}\" dy=\"{dy}\">{}</tspan></text>",
                n(r.y + r.h / 2.0),
                esc(class),
                esc(&self.task_font_size),
                esc(&self.task_font_family),
                esc_text(line),
                x = n(r.x + r.w / 2.0),
            );
        }
        if fo {
            mk.push_str("</switch>");
        }
    }
}

/// `content.split(/<br\s*\/?>/gi)`.
fn split_br(content: &str) -> Vec<String> {
    let lower = content.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        if b[i..].starts_with(b"<br") {
            let mut j = i + 3;
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'/' {
                j += 1;
            }
            if j < b.len() && b[j] == b'>' {
                out.push(content[start..i].to_string());
                start = j + 1;
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push(content[start..].to_string());
    out
}

/// d3's path number: rounded to three decimals, then JavaScript's text.
fn d3n(v: f64) -> String {
    n((v * 1000.0 + 0.5).floor() / 1000.0)
}

/// `d3.arc()` from `start` to `end` (radians clockwise from 12 o'clock, a half
/// turn) between radii `inner` and `outer`, as the path text d3 writes. d3 swaps
/// the radii when the outer is the smaller.
fn d3_half_arc(start: f64, end: f64, inner: f64, outer: f64) -> String {
    let half_pi = PI / 2.0;
    let (r0, r1) = if outer < inner {
        (outer, inner)
    } else {
        (inner, outer)
    };
    let (a0, a1) = (start - half_pi, end - half_pi);
    let large = u8::from(a1 - a0 >= PI);
    format!(
        "M{},{}A{r1o},{r1o},0,{large},1,{},{}L{},{}A{r0o},{r0o},0,{large},0,{},{}Z",
        d3n(r1 * a0.cos()),
        d3n(r1 * a0.sin()),
        d3n(r1 * a1.cos()),
        d3n(r1 * a1.sin()),
        d3n(r0 * a1.cos()),
        d3n(r0 * a1.sin()),
        d3n(r0 * a0.cos()),
        d3n(r0 * a0.sin()),
        r1o = d3n(r1),
        r0o = d3n(r0),
    )
}

/// svgDraw `drawFace` as markup.
fn face_markup(mk: &mut String, cx: f64, cy: f64, score: f64) {
    let r = 15.0;
    let _ = write!(
        mk,
        "<circle cx=\"{}\" cy=\"{}\" class=\"face\" r=\"15\" stroke-width=\"2\" overflow=\"visible\"/><g>",
        n(cx),
        n(cy),
    );
    for dx in [-r / 3.0, r / 3.0] {
        let _ = write!(
            mk,
            "<circle cx=\"{}\" cy=\"{}\" r=\"1.5\" stroke-width=\"2\" fill=\"#666\" stroke=\"#666\"/>",
            n(cx + dx),
            n(cy - r / 3.0),
        );
    }
    let (inner, outer) = (r / 2.0, r / 2.2);
    if score > 3.0 {
        let d = d3_half_arc(PI / 2.0, 3.0 * (PI / 2.0), inner, outer);
        let _ = write!(
            mk,
            "<path class=\"mouth\" d=\"{d}\" transform=\"translate({},{})\"/>",
            n(cx),
            n(cy + 2.0),
        );
    } else if score < 3.0 {
        let d = d3_half_arc(3.0 * PI / 2.0, 5.0 * (PI / 2.0), inner, outer);
        let _ = write!(
            mk,
            "<path class=\"mouth\" d=\"{d}\" transform=\"translate({},{})\"/>",
            n(cx),
            n(cy + 7.0),
        );
    } else {
        let _ = write!(
            mk,
            "<line class=\"mouth\" stroke=\"#666\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"1px\"/>",
            n(cx - 5.0),
            n(cy + 7.0),
            n(cx + 5.0),
            n(cy + 7.0),
        );
    }
    mk.push_str("</g>");
}

/// Lay out and draw a parsed journey.
pub fn draw(j: &Journey, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    // journeyRenderer.ts / svgDraw.js with the journey config defaults.
    let task_w = cfg.section_f64("journey", "width", 150.0);
    let task_h = cfg.section_f64("journey", "height", 50.0);
    let task_margin = cfg.section_f64("journey", "taskMargin", 50.0);
    let margin_x = cfg.section_f64("journey", "diagramMarginX", 50.0);
    let margin_y = cfg.section_f64("journey", "diagramMarginY", 10.0);
    let left_margin = cfg.section_f64("journey", "leftMargin", 150.0);
    let max_label_w = cfg.section_f64("journey", "maxLabelWidth", 360.0);
    // Section and task labels are `byFo` HTML labels in the root font, centred in
    // their box; the legend is root-font SVG text.
    let font = pal.font_size;
    // The labels are HTML (`display: table-cell`) at `line-height: normal`.
    let lh = crate::text::normal_line_height(font);
    let actors = j.actors();
    let conf = MarkupConf::new(cfg);

    let actor_color = |name: &str| {
        let i = actors.iter().position(|a| a == name).unwrap_or(0);
        Color::new(ACTOR_COLORS[i % ACTOR_COLORS.len()])
    };

    let mut back = Vec::new();
    let mut fore = Vec::new();
    // mermaid's own markup, in document order (`<g/>` is the empty group the
    // renderer leaves after the style).
    let mut mk = format!(
        "<g/><defs><marker id=\"{ID_MARK}-arrowhead\" refX=\"5\" refY=\"2\" markerWidth=\"6\" markerHeight=\"4\" orient=\"auto\"><path d=\"M 0,0 V 4 L6,2 Z\"/></marker></defs>"
    );

    // drawActorLegend: circles at (20, 60 + 20·i), names at x = 40 one line (20px)
    // per wrapped line below; a name wider than half the left margin widens it.
    let mut legend_w: f64 = 0.0;
    let mut y = 60.0;
    for (ai, a) in actors.iter().enumerate() {
        let _ = write!(
            mk,
            "<circle cx=\"20\" cy=\"{}\" class=\"actor-{ai}\" fill=\"{}\" stroke=\"#000\" r=\"7\"/>",
            n(y),
            esc(&conf.actor_colour(ai)),
        );
        fore.push(Element::classed_shape(
            Geometry::Circle {
                c: Point::new(20.0, y),
                r: 7.0,
            },
            Paint::both(&actor_color(a), &Color::new("#000000"), 1.0),
            "actor-legend",
        ));
        let lines = if line_width(a, font, false) <= max_label_w {
            vec![a.clone()]
        } else {
            wrap_text(a, max_label_w, font, false)
        };
        for (k, l) in lines.iter().enumerate() {
            let w = line_width(l, font, false);
            if w > legend_w && w > left_margin - w {
                legend_w = w;
            }
            // The `<text x="40">`'s tspan sets `x="50"`.
            let _ = write!(
                mk,
                "<text x=\"40\" y=\"{}\" class=\"legend\"><tspan x=\"50\">{}</tspan></text>",
                n(y + 7.0 + k as f64 * 20.0),
                esc_text(&crate::text::label_lines(l).join(" ")),
            );
            fore.push(Element::classed_text(
                Text {
                    x: 50.0,
                    y: y + 7.0 + k as f64 * 20.0,
                    lines: vec![l.clone()],
                    line_height: 20.0,
                    font_size: font,
                    bold: false,
                    weight: None,
                    anchor: Anchor::Start,
                    fill: pal.text_color.clone(),
                    tspan_central: false,
                    dy_tspans: false,
                    family: None,
                },
                "legend",
            ));
        }
        y += (lines.len() as f64 * 20.0).max(20.0);
    }
    let left = left_margin + legend_w;

    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, left, actors.len() as f64 * 50.0));
    let label = |r: Rect, text: &str, fill: &Color| {
        let lines = wrap_text(text, r.w, font, false);
        Element::classed_text(Text::centered(r.center(), &lines, font, lh, fill), "label")
    };
    let task_y = task_h * 2.0 + margin_y;
    // `maxHeight`: where the drop lines end; faces hang from 300.
    let line_end = 300.0 + 5.0 * 30.0;
    let border = Color::new("#666");
    // drawTasks: the tasks flattened across sections; a section's header is drawn
    // where the section *name* changes (so an unnamed lead section and an empty
    // section draw none, and equal consecutive names share one header).
    let flat: Vec<(&str, &Task)> = j
        .sections
        .iter()
        .flat_map(|s| s.tasks.iter().map(|t| (s.name.as_str(), t)))
        .collect();
    let mut last_section = "";
    let mut section_number = 0usize;
    let mut fill_attr = "#CCC".to_string();
    let mut colour = "black".to_string();
    let mut num = 0usize;
    for (i, &(sec_name, task)) in flat.iter().enumerate() {
        if last_section != sec_name {
            fill_attr = conf.fills[section_number % conf.fills.len()].clone();
            num = section_number % conf.fills.len();
            colour = conf.colours[section_number % conf.colours.len()].clone();
            let count = flat[i..].iter().take_while(|(s, _)| *s == sec_name).count() as f64;
            // `.section-type-N` / `.task-type-N` paint `fillTypeN`.
            let fill = pal.fill_type_color(num);
            let x = i as f64 * (task_margin + task_w) + left;
            let hr = Rect::new(x, 50.0, task_w * count + margin_x * (count - 1.0), task_h);
            let class = format!("journey-section section-type-{num}");
            let _ = write!(
                mk,
                "<g><rect x=\"{}\" y=\"50\" fill=\"{}\" stroke=\"#666\" width=\"{}\" height=\"{}\" rx=\"3\" ry=\"3\" class=\"{class}\"/>",
                n(hr.x),
                esc(&fill_attr),
                n(hr.w),
                n(hr.h),
            );
            conf.text(&mut mk, sec_name, hr, &class, &colour);
            mk.push_str("</g>");
            fore.push(Element::classed_shape(
                Geometry::Rect { rect: hr, rx: 3.0 },
                Paint::both(fill, &border, 1.0),
                "journey-section",
            ));
            fore.push(label(hr, sec_name, &pal.text_color));
            last_section = sec_name;
            section_number += 1;
        }
        let fill = pal.fill_type_color(num);
        let tx = i as f64 * (task_margin + task_w) + left;
        let cx = tx + task_w / 2.0;
        // `line { stroke: textColor }` outranks the `#666` attribute.
        back.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(cx, task_y)),
                PathCmd::LineTo(Point::new(cx, line_end)),
            ]),
            Paint::stroke(&pal.text_color, 1.0).dashed(vec![4.0, 2.0]),
            "task-line",
        ));
        // A NaN score puts upstream's face at `cy = NaN`, i.e. y 0.
        let fy = if task.score.is_nan() {
            0.0
        } else {
            300.0 + (5.0 - task.score) * 30.0
        };
        let _ = write!(
            mk,
            "<g><line id=\"{ID_MARK}-task{i}\" x1=\"{c}\" y1=\"{}\" x2=\"{c}\" y2=\"{}\" class=\"task-line\" stroke-width=\"1px\" stroke-dasharray=\"4 2\" stroke=\"#666\"/>",
            n(task_y),
            n(line_end),
            c = n(cx),
        );
        face_markup(&mut mk, cx, 300.0 + (5.0 - task.score) * 30.0, task.score);
        fore.extend(face(Point::new(cx, fy), task.score, &pal.text_color));
        let tr = Rect::new(tx, task_y, task_w, task_h);
        let _ = write!(
            mk,
            "<rect x=\"{}\" y=\"{}\" fill=\"{}\" stroke=\"#666\" width=\"{}\" height=\"{}\" rx=\"3\" ry=\"3\" class=\"task task-type-{num}\"/>",
            n(tr.x),
            n(tr.y),
            esc(&fill_attr),
            n(tr.w),
            n(tr.h),
        );
        fore.push(Element::classed_shape(
            Geometry::Rect { rect: tr, rx: 3.0 },
            Paint::both(fill, &border, 1.0),
            "task",
        ));
        for (k, a) in task.actors.iter().enumerate() {
            let pos = actors.iter().position(|x| x == a).unwrap_or(0);
            let _ = write!(
                mk,
                "<circle cx=\"{}\" cy=\"{}\" class=\"actor-{pos}\" fill=\"{}\" stroke=\"#000\" r=\"7\"><title>{}</title></circle>",
                n(tx + 14.0 + k as f64 * 10.0),
                n(task_y),
                esc(&conf.actor_colour(pos)),
                esc_text(a),
            );
            fore.push(Element::classed_shape(
                Geometry::Circle {
                    c: Point::new(tx + 14.0 + k as f64 * 10.0, task_y),
                    r: 7.0,
                },
                Paint::both(&actor_color(a), &Color::new("#000000"), 1.0),
                "actor",
            ));
        }
        conf.text(&mut mk, &task.label, tr, "task", &colour);
        mk.push_str("</g>");
        fore.push(label(tr, &task.label, &pal.text_color));
        bounds.add_rect(Rect::new(
            tx,
            task_y,
            margin_x + task_margin,
            line_end - task_y,
        ));
    }
    let content = bounds.rect();
    let height = content.bottom() - content.y + 2.0 * margin_y;
    let width = left + content.right() + 2.0 * margin_x;

    // The activity line: 4px at y = 4·height, its `M 0,0 V 4 L6,2 Z` marker
    // (refX 5) scaled by the stroke width.
    let ay = task_h * 4.0;
    let x2 = width - left - 4.0;
    back.push(Element::line(
        Point::new(left, ay),
        Point::new(x2, ay),
        Paint::stroke(&pal.text_color, 4.0),
    ));
    back.push(Element::classed_shape(
        Geometry::Path(geom::polygon(&[
            Point::new(x2 - 20.0, ay - 8.0),
            Point::new(x2 + 4.0, ay),
            Point::new(x2 - 20.0, ay + 8.0),
        ])),
        // The marker's path has no class: it fills with the svg root's
        // `fill: textColor`.
        Paint::fill(&pal.text_color),
        "arrowhead",
    ));

    let title = j
        .title
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map(|t| crate::text::label_lines(t).join(" "));
    if let Some(t) = &title {
        // `fill` = `journey.titleColor`, default `''`: an empty attribute, so the title
        // inherits the root's `fill: textColor`. `titleFontSize` is `4ex`.
        let title_fill = cfg
            .section("journey", "titleColor")
            .and_then(|j| j.as_str())
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| pal.text_color.clone(), Color::new);
        // `4ex`: four x-heights (1082/2048em).
        let size = 4.0 * crate::text::face().x_height_em(false) * font;
        fore.push(Element::classed_text(
            Text {
                x: left,
                y: 25.0,
                lines: vec![t.clone()],
                line_height: size,
                font_size: size,
                bold: true,
                weight: None,
                anchor: Anchor::Start,
                fill: title_fill,
                tspan_central: false,
                dy_tspans: false,
                family: None,
            },
            "titleText",
        ));
    }
    let extra = if title.is_some() { 70.0 } else { 0.0 };
    if let Some(t) = &title {
        let _ = write!(
            mk,
            "<text x=\"{}\" font-size=\"{}\" font-weight=\"bold\" y=\"25\" fill=\"{}\" font-family=\"{}\">{}</text>",
            n(left),
            esc(&conf.title_font_size),
            esc(&conf.title_color),
            esc(&conf.title_font_family),
            esc_text(t),
        );
    }
    let _ = write!(
        mk,
        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke-width=\"4\" stroke=\"black\" marker-end=\"url(#{ID_MARK}-arrowhead)\"/>",
        n(left),
        n(ay),
        n(x2),
        n(ay),
    );
    Scene {
        kind: DiagramKind::Journey,
        view_box: Rect::new(content.x, -25.0, width, height + extra),
        elements: vec![Element::Markup {
            svg: mk,
            svg_view_box: None,
            children: vec![
                Element::group("lines", back),
                Element::group("journey", fore),
            ],
        }],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Parse and render a `journey` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let j = parse(body, title)?;
    Ok(draw(&j, cfg, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "journey
    title My working day
    section Go to work
      Make tea: 5: Me
      Go upstairs: 3: Me
      Do work: 1: Me, Cat
    section Go home
      Go downstairs: 5: Me
      Sit down: 4
";

    #[test]
    fn parses_sections_tasks_and_actors() {
        let j = parse(SRC, None).unwrap();
        assert_eq!(j.title.as_deref(), Some("My working day"));
        assert_eq!(j.sections.len(), 2);
        assert_eq!(j.sections[0].tasks[2].actors, vec!["Me", "Cat"]);
        assert_eq!(j.sections[0].tasks[1].score, 3.0);
        assert!(j.sections[1].tasks[1].actors.is_empty());
        assert_eq!(j.actors(), vec!["Cat", "Me"]);
    }

    #[test]
    fn errors_have_line_numbers() {
        // `Number("high")` is NaN upstream: accepted, drawn with a neutral face.
        let j = parse("journey\nsection A\n  Task: high: Me", None).unwrap();
        assert!(j.sections[0].tasks[0].score.is_nan());
        let e = parse("journey\n\nnonsense", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        assert!(parse("journeyx", None).is_err());
    }

    #[test]
    fn faces_differ_by_mood() {
        let mouth = |s: f64| {
            face(Point::new(0.0, 0.0), s, &Color::new("#333"))
                .pop()
                .unwrap()
        };
        assert_ne!(mouth(5.0), mouth(1.0));
        assert_ne!(mouth(3.0), mouth(5.0));
    }

    #[test]
    fn smiles_are_d3_annuli() {
        // mermaid 12.3.0: the smile is d3.arc() of radii 7.5 and 15/2.2 from 90°
        // to 270°, 2px below the face centre, filled with the root's colour.
        let Some(Element::Shape {
            geom: Geometry::Path(cmds),
            paint,
            ..
        }) = face(Point::new(0.0, 0.0), 5.0, &Color::new("#333")).pop()
        else {
            panic!("no mouth");
        };
        assert_eq!(paint.fill, Color::new("#333"));
        let pts: Vec<Point> = cmds
            .iter()
            .filter_map(|c| match c {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) => Some(*p),
                PathCmd::Close => None,
                PathCmd::MoveBy(_) | PathCmd::ArcTo { .. } => None,
            })
            .collect();
        let near = |p: Point, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
        assert!(near(pts[0], 7.5, 2.0));
        assert!(pts.iter().any(|&p| near(p, 0.0, 9.5)));
        assert!(pts.iter().any(|&p| near(p, -15.0 / 2.2, 2.0)));
        assert!(pts.iter().any(|&p| near(p, 0.0, 2.0 + 15.0 / 2.2)));
    }

    #[test]
    fn renders() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(SRC, None, &cfg, &pal).unwrap();
        assert!(s.view_box.w > 5.0 * 150.0);
        assert_eq!(s, render(SRC, None, &cfg, &pal).unwrap());
    }

    #[test]
    fn layout_follows_the_journey_renderer() {
        // mermaid 12.3.0, the gallery's journey (7 tasks, title): viewBox
        // `0 -25 1700 540`, the activity line at y = 4·height from leftMargin.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let src = include_str!("../../examples/diagrams/journey.mmd");
        let s = render(src, None, &cfg, &pal).unwrap();
        let v = s.view_box;
        assert_eq!((v.x, v.y, v.w, v.h), (0.0, -25.0, 1700.0, 540.0));
        let Element::Markup { children, .. } = &s.elements[0] else {
            panic!()
        };
        let Element::Group { children, .. } = &children[0] else {
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
        assert_eq!(axis[0], PathCmd::MoveTo(Point::new(150.0, 200.0)));
        assert_eq!(axis[1], PathCmd::LineTo(Point::new(1546.0, 200.0)));
    }
}
