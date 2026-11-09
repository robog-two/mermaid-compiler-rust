//! Pie charts. Geometry follows mermaid's `pieRenderer.ts` constants
//! (height 450, margin 40, legend rects 18px with 4px spacing, slices in input order).

use crate::config::PieConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{TextBlock, decode_entities};
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
            match chart.sections.iter_mut().find(|(l, _)| *l == label) {
                Some(s) => s.1 = value,
                None => chart.sections.push((label, value)),
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

/// Render a pie chart.
pub fn render(chart: &PieChart, cfg: &PieConfig, pal: &Palette) -> Scene {
    const MARGIN: f64 = 40.0;
    const LEGEND_RECT: f64 = 18.0;
    const LEGEND_SPACING: f64 = 4.0;
    const HEIGHT: f64 = 450.0;
    let radius = HEIGHT / 2.0 - MARGIN;
    let center = Point::new(HEIGHT / 2.0, HEIGHT / 2.0);
    let sum: f64 = chart.sections.iter().map(|s| s.1).sum();
    let mut els = Vec::new();
    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, HEIGHT + MARGIN, HEIGHT));

    let color_of = |i: usize| pal.pie_colors[i % pal.pie_colors.len()].clone();
    let mut angle = -std::f64::consts::FRAC_PI_2; // d3 pies start at 12 o'clock
    let mut slices = Vec::new();
    let mut labels = Vec::new();
    for (i, (_, value)) in chart.sections.iter().enumerate() {
        if sum <= 0.0 || *value <= 0.0 {
            continue;
        }
        let sweep = value / sum * std::f64::consts::TAU;
        let (a0, a1) = (angle, angle + sweep);
        angle = a1;
        let mut cmds = if (sweep - std::f64::consts::TAU).abs() < 1e-9 {
            vec![PathCmd::MoveTo(Point::new(
                center.x + radius * a0.cos(),
                center.y + radius * a0.sin(),
            ))]
        } else {
            vec![
                PathCmd::MoveTo(center),
                PathCmd::LineTo(Point::new(
                    center.x + radius * a0.cos(),
                    center.y + radius * a0.sin(),
                )),
            ]
        };
        cmds.extend(geom::arc_cubics(center, radius, a0, a1));
        cmds.push(PathCmd::Close);
        let paint = Paint::both(&color_of(i), &pal.pie_stroke, pal.pie_stroke_width)
            .with_opacity(pal.pie_opacity);
        slices.push(Element::classed_shape(
            Geometry::Path(cmds),
            paint,
            "pieCircle",
        ));
        let pct = value / sum * 100.0;
        if format!("{pct:.0}") != "0" {
            let mid = (a0 + a1) / 2.0;
            let r = radius * cfg.text_position;
            let at = Point::new(center.x + r * mid.cos(), center.y + r * mid.sin());
            let t = Text::centered(
                at,
                &[format!("{pct:.0}%")],
                17.0,
                17.0,
                &pal.pie_section_text,
            );
            labels.push(Element::classed_text(t, "slice"));
        }
    }
    els.push(Element::classed_shape(
        Geometry::Circle {
            c: center,
            r: radius + pal.pie_outer_stroke_width / 2.0,
        },
        crate::scene::Paint::stroke(&pal.pie_outer_stroke, pal.pie_outer_stroke_width),
        "pieOuterCircle",
    ));
    els.extend(slices);
    els.extend(labels);

    if let Some(t) = chart.title.as_ref().filter(|t| !t.is_empty()) {
        let y = center.y - (HEIGHT - 50.0) / 2.0;
        let tb = TextBlock::new(vec![t.clone()], 25.0, false);
        bounds.add_rect(Rect::centered(Point::new(center.x, y), tb.width, tb.height));
        els.push(Element::classed_text(
            Text::centered(
                Point::new(center.x, y),
                &tb.lines,
                25.0,
                25.0,
                &pal.pie_title_text,
            ),
            "pieTitleText",
        ));
    }

    // Legend, vertically centred on the pie's right.
    let legend_h = LEGEND_RECT + LEGEND_SPACING;
    let n = chart.sections.len() as f64;
    let lx = center.x + 12.0 * LEGEND_RECT;
    let mut longest: f64 = 0.0;
    for (i, (label, value)) in chart.sections.iter().enumerate() {
        let ly = center.y + i as f64 * legend_h - legend_h * n / 2.0;
        els.push(Element::rect(
            Rect::new(lx, ly, LEGEND_RECT, LEGEND_RECT),
            0.0,
            Paint::both(&color_of(i), &color_of(i), 1.0),
        ));
        let text = if chart.show_data {
            format!("{label} [{}]", fmt_value(*value))
        } else {
            label.clone()
        };
        let tb = TextBlock::new(vec![text], 17.0, false);
        longest = longest.max(tb.width);
        let t = Text {
            x: lx + LEGEND_RECT + LEGEND_SPACING,
            y: ly + LEGEND_RECT - LEGEND_SPACING,
            lines: tb.lines,
            line_height: 17.0,
            font_size: 17.0,
            bold: false,
            anchor: Anchor::Start,
            fill: pal.pie_legend_text.clone(),
        };
        els.push(Element::classed_text(t, "legend"));
        bounds.add_rect(Rect::new(
            lx,
            ly,
            LEGEND_RECT + LEGEND_SPACING + tb.width,
            LEGEND_RECT,
        ));
    }
    let mut vb = bounds.rect();
    vb.w =
        vb.w.max(HEIGHT + MARGIN + LEGEND_RECT + LEGEND_SPACING + longest);
    Scene {
        kind: DiagramKind::Pie,
        view_box: vb,
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

    #[test]
    fn rejects_negative_and_garbage() {
        assert!(parse("pie\n\"A\" : -1", None).is_err());
        assert!(parse("pie\nnonsense", None).is_err());
    }
}
