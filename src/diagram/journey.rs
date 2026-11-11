//! `journey` (user journey) diagrams.
//!
//! Layout follows mermaid's `journeyRenderer`: an actor legend on the left, then a
//! row of sections (coloured header spanning its tasks), each task a 150×50 box
//! with actor dots on its top edge, a dashed drop line and a face whose height and
//! expression encode the 1–5 score, and an arrowed activity line underneath.
//! Section/task boxes are placed with Taffy ([`crate::layout::boxes`]).

use super::timeline::wrap_text;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::layout::boxes::{self, BoxNode};
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
            let score: f64 = score_txt.parse().map_err(|_| {
                Error::parse(
                    line_no,
                    format!("{line}\nExpecting a numeric score (1-5) after the task name"),
                )
            })?;
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

/// Face drawn in a circle of radius 15 at `c`: smile (> 3), neutral (= 3), sad (< 3).
fn face(c: Point, score: f64, line: &Color) -> Vec<Element> {
    let r = 15.0;
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
            Paint::both(&Color::new("#666666"), &Color::new("#666666"), 2.0),
        ),
        Element::shape(
            Geometry::Circle {
                c: Point::new(c.x + r / 3.0, c.y - r / 3.0),
                r: 1.5,
            },
            Paint::both(&Color::new("#666666"), &Color::new("#666666"), 2.0),
        ),
    ];
    let mouth = Paint::stroke(line, 2.0);
    let arc = |center: Point, rr: f64, a0: f64, a1: f64| {
        let mut cmds = vec![PathCmd::MoveTo(Point::new(
            center.x + rr * a0.cos(),
            center.y + rr * a0.sin(),
        ))];
        cmds.extend(geom::arc_cubics(center, rr, a0, a1));
        cmds
    };
    let m = if score > 3.0 {
        arc(Point::new(c.x, c.y + 1.0), r / 2.0, 0.15 * PI, 0.85 * PI)
    } else if score < 3.0 {
        arc(
            Point::new(c.x, c.y + r * 0.75),
            r / 2.0,
            -0.85 * PI,
            -0.15 * PI,
        )
    } else {
        vec![
            PathCmd::MoveTo(Point::new(c.x - r / 3.0, c.y + r / 3.0 + 1.0)),
            PathCmd::LineTo(Point::new(c.x + r / 3.0, c.y + r / 3.0 + 1.0)),
        ]
    };
    els.push(Element::classed_shape(Geometry::Path(m), mouth, "mouth"));
    els
}

/// Lay out and draw a parsed journey.
/// Length of mermaid's default `journey.sectionFills`.
const SECTION_FILLS: usize = 7;

pub fn draw(j: &Journey, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let task_w = cfg.section_f64("journey", "width", 150.0);
    let task_h = cfg.section_f64("journey", "height", 50.0);
    let margin = cfg.section_f64("journey", "taskMargin", 50.0);
    let font = cfg.section_f64("journey", "taskFontSize", 14.0);
    let score_h = 5.0 * 30.0 + 40.0;
    let actors = j.actors();
    let actor_font = 14.0;

    // Legend width decides where the sections start.
    let legend_w = actors
        .iter()
        .map(|a| 40.0 + line_width(a, actor_font, false))
        .fold(0.0, f64::max);
    let title_lines = j
        .title
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map(|t| crate::text::label_lines(t));
    let title_h = if title_lines.is_some() { 50.0 } else { 10.0 };
    let left = 20.0 + legend_w + if actors.is_empty() { 0.0 } else { 30.0 };

    // Measure task labels; tasks share one height.
    let task_lines: Vec<Vec<Vec<String>>> = j
        .sections
        .iter()
        .map(|s| {
            s.tasks
                .iter()
                .map(|t| wrap_text(&t.label, task_w - 16.0, font, false))
                .collect()
        })
        .collect();
    let lh = font * 1.3;
    let task_h = task_lines
        .iter()
        .flatten()
        .map(|l| l.len() as f64 * lh + 16.0)
        .fold(task_h, f64::max);
    let sec_lines: Vec<Vec<String>> = j
        .sections
        .iter()
        .map(|s| wrap_text(&s.name, f64::INFINITY, font, false))
        .collect();
    let sec_h = sec_lines
        .iter()
        .map(|l| l.len() as f64 * lh + 16.0)
        .fold(task_h, f64::max);

    // Box tree: row(sections) → column[header (stretched), row(task columns)].
    let section_nodes: Vec<BoxNode> = j
        .sections
        .iter()
        .zip(&sec_lines)
        .map(|(s, lines)| {
            let min_w = lines
                .iter()
                .map(|l| line_width(l, font, false))
                .fold(0.0, f64::max)
                + 16.0;
            let mut header = boxes::fixed(0.0, sec_h);
            header.size.width = boxes::taffy::style::Dimension::auto();
            header = boxes::min_size(header, min_w, sec_h);
            let tasks = s
                .tasks
                .iter()
                .map(|_| {
                    BoxNode::new(
                        boxes::column(0.0),
                        vec![
                            BoxNode::leaf(task_w, task_h),
                            BoxNode::leaf(task_w, score_h),
                        ],
                    )
                })
                .collect();
            BoxNode::new(
                boxes::stretched(boxes::column(10.0)),
                vec![
                    BoxNode::new(header, vec![]),
                    BoxNode::new(boxes::row(margin), tasks),
                ],
            )
        })
        .collect();
    let placed = boxes::compute(&BoxNode::new(boxes::row(margin), section_nodes));
    let off = Point::new(left, title_h + 10.0);
    let shift = |r: Rect| Rect::new(r.x + off.x, r.y + off.y, r.w, r.h);

    let mut bounds = Bounds::default();
    let mut back = Vec::new();
    let mut fore = Vec::new();
    let body = shift(placed.rect);
    bounds.add_rect(Rect::new(0.0, 0.0, body.right(), body.bottom()));

    let actor_color = |name: &str| {
        let i = actors.iter().position(|a| a == name).unwrap_or(0);
        Color::new(ACTOR_COLORS[i % ACTOR_COLORS.len()])
    };

    for (si, (sec, sp)) in j.sections.iter().zip(&placed.children).enumerate() {
        // `.section-type-N` / `.task-type-N` paint `fillTypeN`, where N wraps at the
        // length of `journey.sectionFills` (7 by default); the outline is the `#666`
        // stroke attribute and the label takes `.label { color: textColor }`.
        let fill = pal.fill_type_color(si % SECTION_FILLS);
        let tfill = &pal.text_color;
        let border = Color::new("#666");
        let hr = shift(sp.children[0].rect);
        fore.push(Element::classed_shape(
            Geometry::Rect { rect: hr, rx: 3.0 },
            Paint::both(fill, &border, 1.0),
            "journey-section",
        ));
        fore.push(Element::classed_text(
            Text::centered(hr.center(), &sec_lines[si], font, lh, tfill),
            "sectionText",
        ));
        for (ti, (task, tp)) in sec.tasks.iter().zip(&sp.children[1].children).enumerate() {
            let tr = shift(tp.children[0].rect);
            let sr = shift(tp.children[1].rect);
            let cx = tr.center().x;
            back.push(Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(cx, tr.bottom())),
                    PathCmd::LineTo(Point::new(cx, sr.bottom())),
                ]),
                Paint::stroke(&Color::new("#666666"), 1.0).dashed(vec![4.0, 2.0]),
                "task-line",
            ));
            fore.push(Element::classed_shape(
                Geometry::Rect { rect: tr, rx: 3.0 },
                Paint::both(fill, &border, 1.0),
                "task",
            ));
            fore.push(Element::classed_text(
                Text::centered(tr.center(), &task_lines[si][ti], font, lh, tfill),
                "taskText",
            ));
            for (k, a) in task.actors.iter().enumerate() {
                let c = actor_color(a);
                fore.push(Element::classed_shape(
                    Geometry::Circle {
                        c: Point::new(tr.x + 12.0 + k as f64 * 14.0, tr.y),
                        r: 7.0,
                    },
                    Paint::both(&c, &Color::new("#000000"), 1.0),
                    "actor",
                ));
            }
            let score = task.score.clamp(1.0, 5.0);
            let fy = sr.y + 30.0 + (5.0 - score) * 30.0;
            fore.extend(face(Point::new(cx, fy), task.score, &pal.line_color));
        }
    }

    // Activity line with an arrowhead.
    if !j.sections.is_empty() {
        let y = body.bottom() + 15.0;
        let (x0, x1) = (left, body.right());
        back.push(Element::line(
            Point::new(x0, y),
            Point::new(x1 - 8.0, y),
            Paint::stroke(&pal.line_color, 4.0),
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

    // Actor legend.
    for (i, a) in actors.iter().enumerate() {
        let y = off.y + 20.0 + i as f64 * 22.0;
        fore.push(Element::classed_shape(
            Geometry::Circle {
                c: Point::new(27.0, y),
                r: 7.0,
            },
            Paint::both(&actor_color(a), &Color::new("#000000"), 1.0),
            "actor-legend",
        ));
        let tw = line_width(a, actor_font, false);
        bounds.add_rect(Rect::new(40.0, y - 10.0, tw, 20.0));
        fore.push(Element::classed_text(
            Text::centered(
                Point::new(0.0, y),
                std::slice::from_ref(a),
                actor_font,
                lh,
                &pal.text_color,
            )
            .anchored(Anchor::Start, 40.0),
            "legend",
        ));
    }

    if let Some(lines) = &title_lines {
        let tw = lines
            .iter()
            .map(|l| line_width(l, 20.0, true))
            .fold(0.0, f64::max);
        bounds.add_rect(Rect::new(left, 10.0, tw, 30.0));
        // `fill` = `journey.titleColor`, default `''`: an empty attribute, so the title
        // inherits the root's `fill: textColor`.
        let title_fill = cfg
            .section("journey", "titleColor")
            .and_then(|j| j.as_str())
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| pal.text_color.clone(), Color::new);
        fore.push(Element::classed_text(
            Text::centered(Point::new(0.0, 28.0), lines, 20.0, 24.0, &title_fill)
                .bold()
                .anchored(Anchor::Start, left),
            "titleText",
        ));
    }

    Scene {
        kind: DiagramKind::Journey,
        view_box: bounds.rect().pad(12.0),
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
        let e = parse("journey\nsection A\n  Task: high: Me", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("journey\n\nnonsense", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        assert!(parse("journeyx", None).is_err());
    }

    #[test]
    fn faces_differ_by_mood() {
        let c = Color::new("#333");
        let mouth = |s: f64| face(Point::new(0.0, 0.0), s, &c).pop().unwrap();
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
}
