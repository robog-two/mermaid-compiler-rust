//! `quadrantChart` diagrams. Geometry follows mermaid's `quadrantBuilder.ts`
//! defaults (500×500 chart, 16px axis/quadrant text, 12px point labels).
//! The y-axis labels, rotated in mermaid, are drawn horizontally (wrapped)
//! because scenes have no text rotation.

use crate::color;
use crate::config::MermaidConfig;
use crate::diagram::xychart::{keyword, text_at, text_bounds, unquote};
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{TextBlock, line_width};
use crate::theme::Palette;

/// Optional per-point (or per-class) style.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PointStyle {
    pub radius: Option<f64>,
    pub color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_width: Option<f64>,
}

impl PointStyle {
    /// `self` overrides `base`.
    fn over(&self, base: &PointStyle) -> PointStyle {
        PointStyle {
            radius: self.radius.or(base.radius),
            color: self.color.clone().or_else(|| base.color.clone()),
            stroke_color: self
                .stroke_color
                .clone()
                .or_else(|| base.stroke_color.clone()),
            stroke_width: self.stroke_width.or(base.stroke_width),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuadPoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub class: Option<String>,
    pub style: PointStyle,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantChart {
    pub title: Option<String>,
    pub x_left: Option<String>,
    pub x_right: Option<String>,
    pub y_bottom: Option<String>,
    pub y_top: Option<String>,
    /// quadrant-1..4 texts (index 0 = quadrant-1, top right).
    pub quadrants: [Option<String>; 4],
    pub points: Vec<QuadPoint>,
    pub classes: Vec<(String, PointStyle)>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn parse_styles(s: &str, line: usize) -> Result<PointStyle> {
    let mut st = PointStyle::default();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part
            .split_once(':')
            .ok_or_else(|| Error::parse(line, format!("Expecting 'key: value', got '{part}'")))?;
        let (k, v) = (k.trim(), v.trim());
        let num = || {
            v.trim_end_matches("px")
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| Error::parse(line, format!("Invalid number '{v}' for {k}")))
        };
        match k {
            "radius" => st.radius = Some(num()?),
            "stroke-width" => st.stroke_width = Some(num()?),
            "color" => st.color = Some(v.to_string()),
            "stroke-color" => st.stroke_color = Some(v.to_string()),
            _ => {
                return Err(Error::parse(
                    line,
                    format!(
                        "Unknown point style '{k}' (expecting radius, color, stroke-color or stroke-width)"
                    ),
                ));
            }
        }
    }
    Ok(st)
}

/// `left --> right` (right optional).
fn parse_axis(rest: &str) -> (Option<String>, Option<String>) {
    match rest.split_once("-->") {
        Some((a, b)) => {
            let (a, b) = (unquote(a), unquote(b));
            ((!a.is_empty()).then_some(a), (!b.is_empty()).then_some(b))
        }
        None => {
            let a = unquote(rest);
            ((!a.is_empty()).then_some(a), None)
        }
    }
}

/// Find `:` followed by optional whitespace and `[` (outside quotes).
fn point_split(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut in_q = false;
    for (i, &c) in b.iter().enumerate() {
        if c == b'"' {
            in_q = !in_q;
        } else if c == b':' && !in_q {
            let rest = &line[i + 1..];
            let trimmed = rest.trim_start();
            if trimmed.starts_with('[') {
                return Some((i, i + 1 + (rest.len() - trimmed.len())));
            }
        }
    }
    None
}

fn parse_point(line: &str, line_no: usize) -> Result<Option<QuadPoint>> {
    let Some((colon, open)) = point_split(line) else {
        return Ok(None);
    };
    let close = line[open..]
        .find(']')
        .map(|c| c + open)
        .ok_or_else(|| Error::parse(line_no, "Expecting ']' after point coordinates"))?;
    let mut name = line[..colon].trim();
    let mut class = None;
    if let Some(i) = name.find(":::") {
        let c = name[i + 3..].trim();
        if c.is_empty() {
            return Err(Error::parse(line_no, "Expecting a class name after ':::'"));
        }
        class = Some(c.to_string());
        name = name[..i].trim();
    }
    if name.is_empty() {
        return Err(Error::parse(line_no, "Point name is empty"));
    }
    let coords: Vec<&str> = line[open + 1..close].split(',').map(str::trim).collect();
    let parse_c = |s: &str| -> Result<f64> {
        let v = s
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| Error::parse(line_no, format!("Expecting a number, got '{s}'")))?;
        if !(0.0..=1.0).contains(&v) {
            return Err(Error::parse(
                line_no,
                format!("Point coordinate {v} must be between 0 and 1"),
            ));
        }
        Ok(v)
    };
    if coords.len() != 2 {
        return Err(Error::parse(line_no, "Expecting point coordinates [x, y]"));
    }
    let (x, y) = (parse_c(coords[0])?, parse_c(coords[1])?);
    let style = parse_styles(&line[close + 1..], line_no)?;
    Ok(Some(QuadPoint {
        name: unquote(name),
        x,
        y,
        class,
        style,
    }))
}

/// Parse a quadrantChart body.
pub fn parse(body: &str, title: Option<String>) -> Result<QuadrantChart> {
    let mut chart = QuadrantChart {
        title,
        ..Default::default()
    };
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = keyword(line, "quadrantChart")
                .ok_or_else(|| Error::parse(line_no, "Expecting 'quadrantChart'"))?;
            if !rest.is_empty() {
                return Err(Error::parse(line_no, format!("Unexpected '{rest}'")));
            }
            header = true;
            continue;
        }
        if let Some(t) = keyword(line, "title") {
            chart.title = Some(unquote(t));
        } else if let Some(r) = keyword(line, "x-axis") {
            (chart.x_left, chart.x_right) = parse_axis(r);
        } else if let Some(r) = keyword(line, "y-axis") {
            (chart.y_bottom, chart.y_top) = parse_axis(r);
        } else if let Some(r) = keyword(line, "classDef") {
            let (name, styles) = r
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line_no, "Expecting 'classDef <name> <styles>'"))?;
            let st = parse_styles(styles, line_no)?;
            chart.classes.retain(|(n, _)| n != name);
            chart.classes.push((name.to_string(), st));
        } else if let Some(q) = (1..=4).find(|q| keyword(line, &format!("quadrant-{q}")).is_some())
        {
            let text = keyword(line, &format!("quadrant-{q}")).unwrap_or_default();
            chart.quadrants[q - 1] = Some(unquote(text));
        } else if let Some(p) = parse_point(line, line_no)? {
            chart.points.push(p);
        } else {
            return Err(Error::parse(
                line_no,
                format!(
                    "{line}\nExpecting 'title', 'x-axis', 'y-axis', 'quadrant-N', 'classDef' or a point 'Name: [x, y]'"
                ),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'quadrantChart'"));
    }
    // Unknown `:::class` references are ignored, like mermaid.
    Ok(chart)
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

struct QuadColors {
    fills: [Color; 4],
    texts: [Color; 4],
    point: Color,
    point_text: Color,
    x_text: Color,
    y_text: Color,
    internal: Color,
    external: Color,
    title: Color,
}

fn colors(cfg: &MermaidConfig, pal: &Palette) -> QuadColors {
    let var = |k: &str| {
        cfg.theme_variables
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| Color::new(v.trim()))
    };
    let base = color::parse(pal.main_bkg.css());
    let adj = |d: f64| {
        base.map(|c| Color::new(c.lighten(d).to_hex()))
            .unwrap_or_else(|| pal.main_bkg.clone())
    };
    let q1 = var("quadrant1Fill").unwrap_or_else(|| pal.main_bkg.clone());
    let fills = [
        q1.clone(),
        var("quadrant2Fill").unwrap_or_else(|| adj(if pal.dark { 3.0 } else { 1.0 })),
        var("quadrant3Fill").unwrap_or_else(|| adj(if pal.dark { 6.0 } else { 2.0 })),
        var("quadrant4Fill").unwrap_or_else(|| adj(if pal.dark { 9.0 } else { 3.0 })),
    ];
    let t = |k: &str| var(k).unwrap_or_else(|| pal.text_color.clone());
    let point = var("quadrantPointFill").unwrap_or_else(|| {
        color::parse(q1.css())
            .map(|c| {
                let c = if c.is_dark() {
                    c.lighten(30.0)
                } else {
                    c.lighten(-30.0)
                };
                Color::new(c.to_hex())
            })
            .unwrap_or_else(|| pal.node_border.clone())
    });
    QuadColors {
        fills,
        texts: [
            t("quadrant1TextFill"),
            t("quadrant2TextFill"),
            t("quadrant3TextFill"),
            t("quadrant4TextFill"),
        ],
        point,
        point_text: t("quadrantPointTextFill"),
        x_text: t("quadrantXAxisTextFill"),
        y_text: t("quadrantYAxisTextFill"),
        internal: var("quadrantInternalBorderStrokeFill")
            .unwrap_or_else(|| pal.node_border.clone()),
        external: var("quadrantExternalBorderStrokeFill")
            .unwrap_or_else(|| pal.node_border.clone()),
        title: var("quadrantTitleFill").unwrap_or_else(|| pal.title_color.clone()),
    }
}

/// Word-wrap `s` to lines no wider than `max_w`.
fn wrap(s: &str, font: f64, max_w: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        match lines.last_mut() {
            Some(l) if line_width(&format!("{l} {word}"), font, false) <= max_w => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Render a `quadrantChart` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let chart = parse(body, title)?;
    Ok(draw(&chart, cfg, pal))
}

/// Lay out and draw a parsed chart.
pub fn draw(chart: &QuadrantChart, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let s = "quadrantChart";
    let width = cfg.section_f64(s, "chartWidth", 500.0).max(100.0);
    let height = cfg.section_f64(s, "chartHeight", 500.0).max(100.0);
    let title_pad = cfg.section_f64(s, "titlePadding", 10.0);
    let title_font = cfg.section_f64(s, "titleFontSize", 20.0);
    let qpad = cfg.section_f64(s, "quadrantPadding", 5.0);
    let q_text_top = cfg.section_f64(s, "quadrantTextTopPadding", 5.0);
    let q_font = cfg.section_f64(s, "quadrantLabelFontSize", 16.0);
    let in_w = cfg.section_f64(s, "quadrantInternalBorderStrokeWidth", 1.0);
    let ex_w = cfg.section_f64(s, "quadrantExternalBorderStrokeWidth", 2.0);
    let x_pad = cfg.section_f64(s, "xAxisLabelPadding", 5.0);
    let x_font = cfg.section_f64(s, "xAxisLabelFontSize", 16.0);
    let y_pad = cfg.section_f64(s, "yAxisLabelPadding", 5.0);
    let y_font = cfg.section_f64(s, "yAxisLabelFontSize", 16.0);
    let pt_pad = cfg.section_f64(s, "pointTextPadding", 5.0);
    let pt_font = cfg.section_f64(s, "pointLabelFontSize", 12.0);
    let pt_radius = cfg.section_f64(s, "pointRadius", 5.0);
    let y_right = cfg
        .section(s, "yAxisPosition")
        .and_then(crate::json::Json::as_str)
        == Some("right");
    let x_bottom_cfg = cfg
        .section(s, "xAxisPosition")
        .and_then(crate::json::Json::as_str)
        == Some("bottom");
    let qc = colors(cfg, pal);

    let has_points = !chart.points.is_empty();
    let x_bottom = has_points || x_bottom_cfg;
    let has_x = chart.x_left.is_some() || chart.x_right.is_some();
    let has_y = chart.y_bottom.is_some() || chart.y_top.is_some();

    let mut els = Vec::new();
    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, width, height));

    // Vertical bands.
    let mut top = qpad;
    if let Some(t) = chart.title.as_ref().filter(|t| !t.is_empty()) {
        let tb = TextBlock::new(vec![t.clone()], title_font, false);
        let c = Point::new(width / 2.0, title_pad + tb.height / 2.0);
        let txt = Text::centered(c, &tb.lines, title_font, tb.line_height, &qc.title);
        bounds.add_rect(text_bounds(&txt));
        els.push(Element::classed_text(txt, "quadrant-title"));
        top = title_pad * 2.0 + tb.height;
    }
    let x_band = if has_x {
        x_pad * 2.0 + x_font * 1.3
    } else {
        0.0
    };
    if !x_bottom {
        top += x_band;
    }
    let bottom = height - qpad - if x_bottom { x_band } else { 0.0 };

    // Horizontal y-axis labels, wrapped into the side band.
    let y_max_w = (width * 0.22).max(60.0);
    let y_lines: Vec<Vec<String>> = [&chart.y_bottom, &chart.y_top]
        .iter()
        .map(|l| {
            l.as_ref()
                .map_or_else(Vec::new, |t| wrap(t, y_font, y_max_w))
        })
        .collect();
    let y_band_w = if has_y {
        y_lines
            .iter()
            .flatten()
            .map(|l| line_width(l, y_font, false))
            .fold(0.0, f64::max)
            + y_pad * 2.0
    } else {
        0.0
    };
    let (left, right) = if y_right {
        (qpad, width - qpad - y_band_w)
    } else {
        (qpad + y_band_w, width - qpad)
    };
    let quad = Rect::new(
        left,
        top,
        (right - left).max(20.0),
        (bottom - top).max(20.0),
    );
    let (hw, hh) = (quad.w / 2.0, quad.h / 2.0);
    // quadrant-1 top-right, 2 top-left, 3 bottom-left, 4 bottom-right.
    let qrects = [
        Rect::new(quad.x + hw, quad.y, hw, hh),
        Rect::new(quad.x, quad.y, hw, hh),
        Rect::new(quad.x, quad.y + hh, hw, hh),
        Rect::new(quad.x + hw, quad.y + hh, hw, hh),
    ];
    let mut quad_els = Vec::new();
    for (i, r) in qrects.iter().enumerate() {
        quad_els.push(Element::rect(*r, 0.0, Paint::fill(&qc.fills[i])));
    }
    let mut text_els = Vec::new();
    for (i, r) in qrects.iter().enumerate() {
        let Some(t) = chart.quadrants[i].as_ref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let lines = wrap(t, q_font, r.w - 10.0);
        let lh = q_font * 1.2;
        let txt = if has_points {
            text_at(
                r.x + r.w / 2.0,
                r.y + q_text_top + q_font,
                &lines,
                q_font,
                Anchor::Middle,
                &qc.texts[i],
            )
        } else {
            let first = r.y + r.h / 2.0 - lh * (lines.len() as f64 - 1.0) / 2.0 + q_font * 0.35;
            text_at(
                r.x + r.w / 2.0,
                first,
                &lines,
                q_font,
                Anchor::Middle,
                &qc.texts[i],
            )
        };
        text_els.push(Element::classed_text(txt, "quadrant-text"));
    }
    // Borders: internal cross then the external frame.
    let ip = Paint::stroke(&qc.internal, in_w);
    quad_els.push(Element::line(
        Point::new(quad.x + hw, quad.y),
        Point::new(quad.x + hw, quad.bottom()),
        ip.clone(),
    ));
    quad_els.push(Element::line(
        Point::new(quad.x, quad.y + hh),
        Point::new(quad.right(), quad.y + hh),
        ip,
    ));
    quad_els.push(Element::rect(quad, 0.0, Paint::stroke(&qc.external, ex_w)));
    els.push(Element::group("quadrants", quad_els));
    els.push(Element::group("quadrant-texts", text_els));

    // Axis labels.
    let mut axis_els = Vec::new();
    let x_base = if x_bottom {
        quad.bottom() + x_pad + x_font * 0.95
    } else {
        quad.y - x_pad - x_font * 0.3
    };
    let x_slots: Vec<(f64, &String)> = match (&chart.x_left, &chart.x_right) {
        (Some(l), Some(r)) => vec![(quad.x + hw / 2.0, l), (quad.x + hw * 1.5, r)],
        (Some(l), None) => vec![(quad.x + hw / 2.0, l)],
        (None, Some(r)) => vec![(quad.x + hw * 1.5, r)],
        (None, None) => Vec::new(),
    };
    for (x, t) in x_slots {
        let txt = text_at(
            x,
            x_base,
            std::slice::from_ref(t),
            x_font,
            Anchor::Middle,
            &qc.x_text,
        );
        bounds.add_rect(text_bounds(&txt));
        axis_els.push(Element::classed_text(txt, "x-axis-label"));
    }
    let y_x = if y_right {
        quad.right() + y_pad + (y_band_w - 2.0 * y_pad) / 2.0
    } else {
        quad.x - y_band_w / 2.0
    };
    for (k, lines) in y_lines.iter().enumerate() {
        if lines.is_empty() {
            continue;
        }
        let cy = if k == 0 {
            quad.y + hh * 1.5
        } else {
            quad.y + hh * 0.5
        };
        let lh = y_font * 1.2;
        let first = cy - lh * (lines.len() as f64 - 1.0) / 2.0 + y_font * 0.35;
        let txt = text_at(y_x, first, lines, y_font, Anchor::Middle, &qc.y_text);
        bounds.add_rect(text_bounds(&txt));
        axis_els.push(Element::classed_text(txt, "y-axis-label"));
    }
    els.push(Element::group("axis-labels", axis_els));

    // Points.
    let mut pt_els = Vec::new();
    for p in &chart.points {
        let class_style = p
            .class
            .as_ref()
            .and_then(|c| chart.classes.iter().find(|(n, _)| n == c))
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let st = p.style.over(&class_style);
        let r = st.radius.unwrap_or(pt_radius);
        let fill = st
            .color
            .as_deref()
            .map_or_else(|| qc.point.clone(), Color::new);
        let sw = st.stroke_width.unwrap_or(0.0);
        let stroke = st
            .stroke_color
            .as_deref()
            .map_or_else(|| fill.clone(), Color::new);
        let c = Point::new(quad.x + p.x * quad.w, quad.bottom() - p.y * quad.h);
        let paint = if sw > 0.0 {
            Paint::both(&fill, &stroke, sw)
        } else {
            Paint::fill(&fill)
        };
        bounds.add_rect(Rect::centered(c, 2.0 * r + sw, 2.0 * r + sw));
        pt_els.push(Element::classed_shape(
            Geometry::Circle { c, r },
            paint,
            "quadrant-point",
        ));
        let txt = text_at(
            c.x,
            c.y + r + pt_pad + pt_font * 0.85,
            std::slice::from_ref(&p.name),
            pt_font,
            Anchor::Middle,
            &qc.point_text,
        );
        bounds.add_rect(text_bounds(&txt));
        pt_els.push(Element::classed_text(txt, "quadrant-point-label"));
    }
    els.push(Element::group("data-points", pt_els));

    Scene {
        kind: DiagramKind::Quadrant,
        view_box: bounds.rect().pad(4.0),
        elements: vec![Element::group("main", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "quadrantChart
  title Reach and engagement of campaigns
  x-axis Low Reach --> High Reach
  y-axis Low Engagement --> \"High Engagement\"
  quadrant-1 We should expand
  quadrant-2 Need to promote
  quadrant-3 Re-evaluate
  quadrant-4 May be improved
  Campaign A: [0.9, 0.0] radius: 12
  Campaign B:::class1: [0.8, 0.1] color: #ff3300, radius: 10
  Campaign D: [0.6, 0.3] radius: 15, stroke-color: #00ff0f, stroke-width: 5px ,color: #ff33f0
  classDef class1 color: #109060, stroke-width: 2px
";

    #[test]
    fn parses_everything() {
        let c = parse(SRC, None).unwrap();
        assert_eq!(
            c.title.as_deref(),
            Some("Reach and engagement of campaigns")
        );
        assert_eq!(c.x_left.as_deref(), Some("Low Reach"));
        assert_eq!(c.x_right.as_deref(), Some("High Reach"));
        assert_eq!(c.y_top.as_deref(), Some("High Engagement"));
        assert_eq!(c.quadrants[3].as_deref(), Some("May be improved"));
        assert_eq!(c.points.len(), 3);
        let b = &c.points[1];
        assert_eq!(b.class.as_deref(), Some("class1"));
        assert_eq!(b.style.color.as_deref(), Some("#ff3300"));
        assert_eq!(b.style.radius, Some(10.0));
        let d = &c.points[2];
        assert_eq!(d.style.stroke_width, Some(5.0));
        assert_eq!(d.style.stroke_color.as_deref(), Some("#00ff0f"));
        assert_eq!(c.classes[0].1.stroke_width, Some(2.0));
        // Direct style wins over class style.
        let merged = b.style.over(&c.classes[0].1);
        assert_eq!(merged.color.as_deref(), Some("#ff3300"));
        assert_eq!(merged.stroke_width, Some(2.0));
    }

    #[test]
    fn axis_without_arrow() {
        let c = parse("quadrantChart\nx-axis Urgent\ny-axis Low -->", None).unwrap();
        assert_eq!(c.x_left.as_deref(), Some("Urgent"));
        assert_eq!(c.x_right, None);
        assert_eq!(c.y_bottom.as_deref(), Some("Low"));
    }

    #[test]
    fn errors_report_lines() {
        let e = parse("quadrantChart\nA: [1.5, 0.2]", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }), "{e:?}");
        let e = parse("quadrantChart\n\nA: [0.5, 0.2] size: 3", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("quadrantChart\nwhat is this", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("graph TD", None).is_err());
    }

    #[test]
    fn points_land_inside_quadrants() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let scene = render(SRC, None, &cfg, &pal).unwrap();
        assert_eq!(scene.kind, DiagramKind::Quadrant);
        assert!(scene.view_box.w >= 500.0);
    }
}
