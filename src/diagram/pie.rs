//! Pie charts. Geometry follows mermaid's `pieRenderer.ts` constants
//! (height 450, margin 40, legend rects 18px with 4px spacing, slices in input order).

use crate::config::PieConfig;
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::{Anchor, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{decode_entities, svg_bbox_width};
use crate::theme::Palette;

#[derive(Debug, Clone, PartialEq)]
pub struct PieChart {
    pub title: Option<String>,
    pub show_data: bool,
    pub sections: Vec<(String, f64)>,
}

/// Parse a pie chart body.
pub fn parse(body: &str, title: Option<String>) -> Result<PieChart> {
    let mut chart = PieChart {
        title,
        show_data: false,
        sections: Vec::new(),
    };
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let mut line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            line = line
                .strip_prefix("pie")
                .ok_or_else(|| Error::parse(line_no, "Expecting 'pie'"))?
                .trim();
            header = true;
            if let Some(rest) = line.strip_prefix("showData") {
                chart.show_data = true;
                line = rest.trim();
            }
            if line.is_empty() {
                continue;
            }
        }
        if line == "showData" {
            chart.show_data = true;
        } else if let Some(t) = line.strip_prefix("title") {
            chart.title = Some(t.trim().to_string());
        } else if let Some(quoted) = line.strip_prefix('"') {
            let close = quoted
                .find('"')
                .ok_or_else(|| Error::parse(line_no, format!("{line}\nUnterminated label")))?;
            let label = decode_entities(&quoted[..close]);
            let rest = quoted[close + 1..].trim();
            let value = rest
                .strip_prefix(':')
                .map(str::trim)
                // pie.langium NUMBER_PIE: `-?digits(.digits)?` only (no exponent).
                .filter(|v| {
                    let d = v.strip_prefix('-').unwrap_or(v);
                    !d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.')
                })
                .and_then(|v| v.parse::<f64>().ok())
                .ok_or_else(|| Error::parse(line_no, format!("{line}\nExpecting ': <number>'")))?;
            if value < 0.0 {
                return Err(Error::parse(
                    line_no,
                    format!(
                        "\"{label}\" has invalid value: {value}. Negative values are not allowed in pie charts. All slice values must be >= 0."
                    ),
                ));
            }
            // `addSection` keeps the first value of a repeated label.
            if !chart.sections.iter().any(|(l, _)| *l == label) {
                chart.sections.push((label, value));
            }
        } else {
            return Err(Error::parse(
                line_no,
                format!("{line}\nExpecting a quoted label, 'title' or 'showData'"),
            ));
        }
    }
    Ok(chart)
}

/// `Number.prototype.toFixed(0)` for a non-negative percentage: halves round up
/// (Rust's formatter rounds them to even).
fn percent_label(pct: f64) -> String {
    format!("{}", pct.round() as i64)
}

/// An arc as cubic segments of at most 22.5°, so the drawn curve stays within a
/// thousandth of a pixel of the circle d3's `A` command draws.
fn fine_arc(c: Point, r: f64, a0: f64, a1: f64) -> Vec<PathCmd> {
    let n = ((a1 - a0).abs() / (std::f64::consts::PI / 8.0))
        .ceil()
        .max(1.0) as usize;
    let step = (a1 - a0) / n as f64;
    (0..n)
        .flat_map(|i| geom::arc_cubics(c, r, a0 + step * i as f64, a0 + step * (i + 1) as f64))
        .collect()
}

/// d3's `arc()` for one slice (angles clockwise from 3 o'clock, in radians).
fn slice_path(c: Point, r0: f64, r1: f64, a0: f64, a1: f64) -> Vec<PathCmd> {
    let at = |r: f64, a: f64| Point::new(c.x + r * a.cos(), c.y + r * a.sin());
    let full = a1 - a0 > std::f64::consts::TAU - 1e-12;
    let mut cmds = vec![PathCmd::MoveTo(at(r1, a0))];
    cmds.extend(fine_arc(c, r1, a0, a1));
    if r0 > 1e-12 {
        cmds.push(if full {
            PathCmd::MoveTo(at(r0, a1))
        } else {
            PathCmd::LineTo(at(r0, a1))
        });
        cmds.extend(fine_arc(c, r0, a1, a0));
    } else if !full {
        cmds.push(PathCmd::LineTo(c));
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// Render a pie chart (`pieRenderer.ts`).
pub fn render(chart: &PieChart, cfg: &PieConfig, pal: &Palette) -> Scene {
    const MARGIN: f64 = 40.0;
    const LEGEND_RECT: f64 = 18.0;
    const LEGEND_SPACING: f64 = 4.0;
    const HEIGHT: f64 = 450.0;
    let tau = std::f64::consts::TAU;
    let radius = HEIGHT / 2.0 - MARGIN;
    let origin = Point::new(HEIGHT / 2.0, HEIGHT / 2.0);
    let inner = if cfg.donut_hole > 0.0 && cfg.donut_hole <= 0.9 {
        cfg.donut_hole * radius
    } else {
        0.0
    };
    let sum: f64 = chart.sections.iter().map(|s| s.1).sum();
    let color_of = |i: usize| pal.pie_colors[i % pal.pie_colors.len()].clone();

    // Legend entries and their measured width (`getBoundingClientRect`).
    let legend_size = pal.pie_legend_text_size;
    let entries: Vec<String> = chart
        .sections
        .iter()
        .map(|(label, value)| {
            if chart.show_data {
                format!("{label} [{}]", fmt_value(*value))
            } else {
                label.clone()
            }
        })
        .collect();
    let longest = entries
        .iter()
        .map(|t| svg_bbox_width(t, legend_size))
        .fold(0.0, f64::max);
    let legend_h = LEGEND_RECT + LEGEND_SPACING;
    let total_legend = chart.sections.len() as f64 * legend_h;
    let mut height = HEIGHT;
    let mut width = HEIGHT + MARGIN;
    let mut pie_shift = (0.0, 0.0);
    let centred_x = -longest / 2.0 - legend_h;
    let legend_at: Box<dyn Fn(f64) -> (f64, f64)> = match cfg.legend_position.as_str() {
        "center" => Box::new(move |i| (centred_x, i * legend_h - total_legend / 2.0)),
        "top" => {
            height += total_legend;
            pie_shift = (0.0, total_legend + legend_h);
            Box::new(move |i| (centred_x, i * legend_h - radius))
        }
        "bottom" => {
            height += total_legend;
            Box::new(move |i| (centred_x, i * legend_h + radius + legend_h))
        }
        "left" => {
            width += legend_h + longest;
            pie_shift = (longest + legend_h, 0.0);
            Box::new(move |i| (-radius - legend_h, i * legend_h - total_legend / 2.0))
        }
        _ => {
            width += legend_h + longest;
            Box::new(move |i| (12.0 * LEGEND_RECT, i * legend_h - total_legend / 2.0))
        }
    };

    // d3.pie over the slices of at least 1% (`createPieArcs`), in input order.
    let shown: Vec<(usize, f64)> = chart
        .sections
        .iter()
        .enumerate()
        .filter(|(_, (_, v))| sum > 0.0 && v / sum * 100.0 >= 1.0)
        .map(|(i, (_, v))| (i, *v))
        .collect();
    let shown_sum: f64 = shown.iter().map(|s| s.1).sum();
    let center = Point::new(origin.x + pie_shift.0, origin.y + pie_shift.1);
    let mut slices = Vec::new();
    let mut labels = Vec::new();
    let mut start = 0.0;
    for &(i, value) in &shown {
        let end = start + value * tau / shown_sum;
        let (a0, a1) = (start - tau / 4.0, end - tau / 4.0);
        start = end;
        let pct = value / sum * 100.0;
        let mut cmds = slice_path(center, inner, radius, a0, a1);
        let mut paint = Paint::both(&color_of(i), &pal.pie_stroke, pal.pie_stroke_width)
            .with_opacity(pal.pie_opacity);
        if cfg.highlight_slice != "hover" && cfg.highlight_slice == chart.sections[i].0 {
            // `.pieCircle.highlighted { scale: 1.05; opacity: 1 }`, about the pie's centre.
            let k = 1.05;
            let sc = |p: &Point| {
                Point::new(
                    center.x + (p.x - center.x) * k,
                    center.y + (p.y - center.y) * k,
                )
            };
            cmds = cmds
                .iter()
                .map(|c| match c {
                    PathCmd::MoveTo(p) => PathCmd::MoveTo(sc(p)),
                    PathCmd::LineTo(p) => PathCmd::LineTo(sc(p)),
                    PathCmd::CubicTo(a, b, p) => PathCmd::CubicTo(sc(a), sc(b), sc(p)),
                    PathCmd::Close => PathCmd::Close,
                    PathCmd::MoveBy(d) => PathCmd::MoveBy(d.scale(k)),
                    &PathCmd::ArcTo {
                        rx,
                        ry,
                        large,
                        sweep,
                        to,
                        rel,
                    } => PathCmd::ArcTo {
                        rx: rx * k,
                        ry: ry * k,
                        large,
                        sweep,
                        to: if rel { to.scale(k) } else { sc(&to) },
                        rel,
                    },
                })
                .collect();
            paint.stroke_width *= k;
            paint.opacity = Some(1.0);
        }
        slices.push(Element::classed_shape(
            Geometry::Path(cmds),
            paint,
            "pieCircle",
        ));
        // The label's baseline sits on the centroid of the label arc.
        let mid = (a0 + a1) / 2.0;
        let r = radius * cfg.text_position;
        labels.push(Element::classed_text(
            Text {
                x: center.x + r * mid.cos(),
                y: center.y + r * mid.sin(),
                lines: vec![format!("{}%", percent_label(pct))],
                line_height: pal.pie_section_text_size,
                font_size: pal.pie_section_text_size,
                bold: false,
                weight: None,
                anchor: Anchor::Middle,
                fill: pal.pie_section_text.clone(),
                tspan_central: false,
                dy_tspans: false,
                family: None,
            },
            "slice",
        ));
    }
    let mut els = vec![Element::classed_shape(
        Geometry::Circle {
            c: center,
            r: radius + pal.pie_outer_stroke_width / 2.0,
        },
        Paint::stroke(&pal.pie_outer_stroke, pal.pie_outer_stroke_width),
        "pieOuterCircle",
    )];
    els.extend(slices);
    els.extend(labels);

    let title = chart.title.clone().unwrap_or_default();
    let title_size = pal.pie_title_text_size;
    let title_w = svg_bbox_width(&title, title_size);
    if !title.trim().is_empty() {
        els.push(Element::classed_text(
            Text {
                x: origin.x,
                y: origin.y - (HEIGHT - 50.0) / 2.0,
                lines: vec![title],
                line_height: title_size,
                font_size: title_size,
                bold: false,
                weight: None,
                anchor: Anchor::Middle,
                fill: pal.pie_title_text.clone(),
                tspan_central: false,
                dy_tspans: false,
                family: None,
            },
            "pieTitleText",
        ));
    }

    for (i, text) in entries.into_iter().enumerate() {
        let (dx, dy) = legend_at(i as f64);
        let (lx, ly) = (origin.x + dx, origin.y + dy);
        els.push(Element::rect(
            Rect::new(lx, ly, LEGEND_RECT, LEGEND_RECT),
            0.0,
            Paint::both(&color_of(i), &color_of(i), 1.0),
        ));
        let t = Text {
            x: lx + LEGEND_RECT + LEGEND_SPACING,
            y: ly + LEGEND_RECT - LEGEND_SPACING,
            lines: vec![text],
            line_height: legend_size,
            font_size: legend_size,
            bold: false,
            weight: None,
            anchor: Anchor::Start,
            fill: pal.pie_legend_text.clone(),
            tspan_central: false,
            dy_tspans: false,
            family: None,
        };
        els.push(Element::classed_text(t, "legend"));
    }
    // The viewBox grows to the title when it is wider than the chart.
    let left = (HEIGHT / 2.0 - title_w / 2.0).min(0.0);
    let right = width.max(HEIGHT / 2.0 + title_w / 2.0);
    Scene {
        kind: DiagramKind::Pie,
        view_box: Rect::new(left, 0.0, right - left, height),
        elements: vec![Element::group("pie", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn fmt_value(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pie() {
        let c = parse(
            "pie showData title Pets\n\"Dogs\" : 386\n\"Cats\" : 85.5\n",
            None,
        )
        .unwrap();
        assert!(c.show_data);
        assert_eq!(c.title.as_deref(), Some("Pets"));
        assert_eq!(
            c.sections,
            vec![("Dogs".into(), 386.0), ("Cats".into(), 85.5)]
        );
    }

    fn draw(src: &str, cfg: &PieConfig) -> Scene {
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        render(&parse(src, None).unwrap(), cfg, &pal)
    }

    fn texts(s: &Scene) -> Vec<Text> {
        fn walk(els: &[Element], out: &mut Vec<Text>) {
            for e in els {
                match e {
                    Element::Text { text, .. } => out.push(text.clone()),
                    Element::Group { children, .. } => walk(children, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&s.elements, &mut out);
        out
    }

    #[test]
    fn labels_sit_on_their_baselines_and_round_halves_up() {
        // mermaid 12.3.0: the title's baseline at y=25, a slice label's on the label
        // arc's centroid, `toFixed(0)` turns 40.5% into 41%.
        let s = draw(
            "pie title T\n\"a\" : 40.5\n\"b\" : 59.5\n",
            &PieConfig::default(),
        );
        let t = texts(&s);
        let title = t.iter().find(|t| t.lines[0] == "T").unwrap();
        assert_eq!((title.x, title.y), (225.0, 25.0));
        let a = t.iter().find(|t| t.lines[0] == "41%").unwrap();
        let mid = (0.405 * std::f64::consts::PI) - std::f64::consts::FRAC_PI_2;
        assert!((a.x - (225.0 + 138.75 * mid.cos())).abs() < 1e-9);
        assert!((a.y - (225.0 + 138.75 * mid.sin())).abs() < 1e-9);
        // The legend is as wide as the browser's box of its widest label
        // (`b` at 17px: 10px of ink, not its 9.45px advance).
        assert_eq!(s.view_box, Rect::new(0.0, 0.0, 522.0, 450.0));
    }

    #[test]
    fn drops_slices_under_one_percent_and_keeps_first_duplicate() {
        let s = draw(
            "pie\n\"Big\" : 1000\n\"Tiny\" : 1\n\"Big\" : 5\n",
            &PieConfig::default(),
        );
        let t: Vec<String> = texts(&s).into_iter().map(|t| t.lines[0].clone()).collect();
        assert_eq!(t, vec!["100%", "Big", "Tiny"]);
    }

    #[test]
    fn legend_positions_resize_the_view_box() {
        let src = "pie\n\"A\" : 1\n\"B\" : 2\n";
        let cfg = |p: &str| PieConfig {
            legend_position: p.into(),
            ..PieConfig::default()
        };
        assert_eq!(draw(src, &cfg("top")).view_box.h, 450.0 + 44.0);
        assert_eq!(draw(src, &cfg("bottom")).view_box.h, 450.0 + 44.0);
        assert_eq!(draw(src, &cfg("center")).view_box.w, 490.0);
        assert_eq!(draw(src, &cfg("left")).view_box.w, 490.0 + 22.0 + 12.0);
        // A title wider than the chart widens the viewBox on both sides.
        let wide = draw(
            &format!("pie title {}\n\"A\" : 1\n", "W".repeat(40)),
            &PieConfig::default(),
        );
        assert!(wide.view_box.x < 0.0);
    }

    #[test]
    fn rejects_negative_and_garbage() {
        assert!(parse("pie\n\"A\" : -1", None).is_err());
        assert!(parse("pie\nnonsense", None).is_err());
    }
}
