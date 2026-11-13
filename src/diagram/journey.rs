//! `journey` (user journey) diagrams.
//!
//! Layout follows mermaid's `journeyRenderer`: an actor legend on the left, then a
//! row of sections (coloured header spanning its tasks), each task a 150×50 box
//! with actor dots on its top edge, a dashed drop line and a face whose height and
//! expression encode the 1–5 score, and an arrowed activity line across them.

use super::timeline::wrap_text;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::Palette;
use std::f64::consts::PI;

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
/// a mouth by score: smile (> 3, a lower half arc 2 down), sad (< 3, an upper
/// half arc 7 down) or a flat 10px line 7 down. The arcs are d3 annuli of radii
/// 7.5 and 6.8, drawn here as their centre line.
fn face(c: Point, score: f64) -> Vec<Element> {
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
    let mid = (r / 2.0 + r / 2.2) / 2.0;
    let arc = |center: Point, a0: f64, a1: f64| {
        let mut cmds = vec![PathCmd::MoveTo(Point::new(
            center.x + mid * a0.cos(),
            center.y + mid * a0.sin(),
        ))];
        cmds.extend(geom::arc_cubics(center, mid, a0, a1));
        cmds
    };
    let (m, w) = if score > 3.0 {
        (arc(Point::new(c.x, c.y + 2.0), 0.0, PI), 1.7)
    } else if score < 3.0 {
        (arc(Point::new(c.x, c.y + 7.0), -PI, 0.0), 1.7)
    } else {
        (
            vec![
                PathCmd::MoveTo(Point::new(c.x - 5.0, c.y + 7.0)),
                PathCmd::LineTo(Point::new(c.x + 5.0, c.y + 7.0)),
            ],
            1.0,
        )
    };
    els.push(Element::classed_shape(
        Geometry::Path(m),
        Paint::stroke(&grey, w),
        "mouth",
    ));
    els
}

/// Lay out and draw a parsed journey.
/// Length of mermaid's default `journey.sectionFills`.
const SECTION_FILLS: usize = 7;

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
    let lh = font * 1.15;
    let actors = j.actors();

    let actor_color = |name: &str| {
        let i = actors.iter().position(|a| a == name).unwrap_or(0);
        Color::new(ACTOR_COLORS[i % ACTOR_COLORS.len()])
    };

    let mut back = Vec::new();
    let mut fore = Vec::new();

    // drawActorLegend: circles at (20, 60 + 20·i), names at x = 40 one line (20px)
    // per wrapped line below; a name wider than half the left margin widens it.
    let mut legend_w: f64 = 0.0;
    let mut y = 60.0;
    for a in &actors {
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
            fore.push(Element::classed_text(
                Text {
                    x: 40.0,
                    y: y + 7.0 + k as f64 * 20.0,
                    lines: vec![l.clone()],
                    line_height: 20.0,
                    font_size: font,
                    bold: false,
                    anchor: Anchor::Start,
                    fill: pal.text_color.clone(),
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
    let mut i = 0usize;
    for (si, sec) in j.sections.iter().enumerate() {
        // `.section-type-N` / `.task-type-N` paint `fillTypeN`, where N wraps at the
        // length of `journey.sectionFills` (7 by default).
        let fill = pal.fill_type_color(si % SECTION_FILLS);
        let n = sec.tasks.len() as f64;
        let x = i as f64 * (task_margin + task_w) + left;
        let hr = Rect::new(x, 50.0, task_w * n + margin_x * (n - 1.0), task_h);
        fore.push(Element::classed_shape(
            Geometry::Rect { rect: hr, rx: 3.0 },
            Paint::both(fill, &border, 1.0),
            "journey-section",
        ));
        fore.push(label(hr, &sec.name, &pal.text_color));
        for task in &sec.tasks {
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
            fore.extend(face(Point::new(cx, fy), task.score));
            let tr = Rect::new(tx, task_y, task_w, task_h);
            fore.push(Element::classed_shape(
                Geometry::Rect { rect: tr, rx: 3.0 },
                Paint::both(fill, &border, 1.0),
                "task",
            ));
            for (k, a) in task.actors.iter().enumerate() {
                fore.push(Element::classed_shape(
                    Geometry::Circle {
                        c: Point::new(tx + 14.0 + k as f64 * 10.0, task_y),
                        r: 7.0,
                    },
                    Paint::both(&actor_color(a), &Color::new("#000000"), 1.0),
                    "actor",
                ));
            }
            fore.push(label(tr, &task.label, &pal.text_color));
            bounds.add_rect(Rect::new(
                tx,
                task_y,
                margin_x + task_margin,
                line_end - task_y,
            ));
            i += 1;
        }
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
        Paint::fill(&Color::new("#000000")),
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
        let size = 4.0 * 0.52 * font;
        fore.push(Element::classed_text(
            Text {
                x: left,
                y: 25.0,
                lines: vec![t.clone()],
                line_height: size,
                font_size: size,
                bold: true,
                anchor: Anchor::Start,
                fill: title_fill,
            },
            "titleText",
        ));
    }
    let extra = if title.is_some() { 70.0 } else { 0.0 };
    Scene {
        kind: DiagramKind::Journey,
        view_box: Rect::new(content.x, -25.0, width, height + extra),
        elements: vec![
            Element::group("lines", back),
            Element::group("journey", fore),
        ],
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
        let mouth = |s: f64| face(Point::new(0.0, 0.0), s).pop().unwrap();
        assert_ne!(mouth(5.0), mouth(1.0));
        assert_ne!(mouth(3.0), mouth(5.0));
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
        assert_eq!(axis[0], PathCmd::MoveTo(Point::new(150.0, 200.0)));
        assert_eq!(axis[1], PathCmd::LineTo(Point::new(1546.0, 200.0)));
    }
}
