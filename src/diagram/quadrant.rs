//! `quadrantChart` diagrams. Geometry follows mermaid's `quadrantBuilder.ts`
//! defaults (500×500 chart, 16px axis/quadrant text, 12px point labels);
//! the y-axis labels are rotated -90° in a `yAxisLabelPadding * 2 +
//! yAxisLabelFontSize` side band, as in mermaid.

use crate::color;
use crate::config::MermaidConfig;
use crate::diagram::xychart::{keyword, text_at, text_bounds, unquote};
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene};
use crate::text::line_width;
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
    // The theme files: `quadrantNFill = adjust(primaryColor, { r, g, b: 5(N-1) })`,
    // `quadrantNTextFill = adjust(primaryTextColor, { r, g, b: -5(N-1) })`.
    let rgb_step = |c: &Color, d: f64| -> Color {
        color::parse(c.css()).map_or_else(
            || c.clone(),
            |x| {
                let ch = |v: u8| (v as f64 + d).round().clamp(0.0, 255.0) as u8;
                Color::new(
                    color::Rgba {
                        r: ch(x.r),
                        g: ch(x.g),
                        b: ch(x.b),
                        a: x.a,
                    }
                    .to_hex(),
                )
            },
        )
    };
    let q1 = var("quadrant1Fill").unwrap_or_else(|| pal.quadrant_fill.clone());
    let fills = [
        q1,
        var("quadrant2Fill").unwrap_or_else(|| rgb_step(&pal.quadrant_fill, 5.0)),
        var("quadrant3Fill").unwrap_or_else(|| rgb_step(&pal.quadrant_fill, 10.0)),
        var("quadrant4Fill").unwrap_or_else(|| rgb_step(&pal.quadrant_fill, 15.0)),
    ];
    let text = &pal.primary_text;
    let t = |k: &str, d: f64| var(k).unwrap_or_else(|| rgb_step(text, d));
    // `quadrantPointFill = x || isDark(q1) ? lighten(q1) : darken(q1)` parses as
    // `(x || isDark(q1)) ? ...`, and khroma's lighten/darken without an amount yield
    // `hsl(.., NaN%)`: an invalid fill attribute, so the point inherits the root's
    // `fill: textColor`. A user-supplied value is copied back over it afterwards.
    let point = var("quadrantPointFill").unwrap_or_else(|| pal.text_color.clone());
    let border = |k: &str| var(k).unwrap_or_else(|| pal.primary_border.clone());
    QuadColors {
        fills,
        texts: [
            t("quadrant1TextFill", 0.0),
            t("quadrant2TextFill", -5.0),
            t("quadrant3TextFill", -10.0),
            t("quadrant4TextFill", -15.0),
        ],
        point,
        point_text: t("quadrantPointTextFill", 0.0),
        x_text: t("quadrantXAxisTextFill", 0.0),
        y_text: t("quadrantYAxisTextFill", 0.0),
        internal: border("quadrantInternalBorderStrokeFill"),
        external: border("quadrantExternalBorderStrokeFill"),
        title: t("quadrantTitleFill", 0.0),
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

/// Alphabetic baseline offset for SVG `dominant-baseline: hanging` text
/// (Blink places the hanging baseline at 0.8 × ascent; Arial ascent ≈ 0.905em).
const HANGING: f64 = 0.72;
/// Alphabetic baseline offset for `dominant-baseline: middle` text.
const MIDDLE: f64 = 0.35;

/// Lay out and draw a parsed chart, following `quadrantBuilder.ts` (space
/// calculation, label anchors) and `quadrantRenderer.ts` (draw order; texts at
/// `translate(x, y) rotate(r)` with `hanging`/`middle` baselines).
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

    let nonempty = |t: &Option<String>| t.as_ref().is_some_and(|t| !t.is_empty());
    let has_points = !chart.points.is_empty();
    let x_bottom = has_points || x_bottom_cfg;
    let show_x = nonempty(&chart.x_left) || nonempty(&chart.x_right);
    let show_y = nonempty(&chart.y_bottom) || nonempty(&chart.y_top);
    let title = chart.title.as_ref().filter(|t| !t.is_empty());

    // `calculateSpace`.
    let x_space = if show_x { x_pad * 2.0 + x_font } else { 0.0 };
    let (x_top, x_bot) = if x_bottom {
        (0.0, x_space)
    } else {
        (x_space, 0.0)
    };
    let y_space = if show_y { y_pad * 2.0 + y_font } else { 0.0 };
    let (y_left, y_rgt) = if y_right {
        (0.0, y_space)
    } else {
        (y_space, 0.0)
    };
    let title_top = if title.is_some() {
        title_font + title_pad * 2.0
    } else {
        0.0
    };
    let quad = Rect::new(
        qpad + y_left,
        qpad + x_top + title_top,
        (width - qpad * 2.0 - y_left - y_rgt).max(20.0),
        (height - qpad * 2.0 - x_top - x_bot - title_top).max(20.0),
    );
    let (hw, hh) = (quad.w / 2.0, quad.h / 2.0);

    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, width, height));

    // Quadrants: 1 top-right, 2 top-left, 3 bottom-left, 4 bottom-right.
    let qrects = [
        Rect::new(quad.x + hw, quad.y, hw, hh),
        Rect::new(quad.x, quad.y, hw, hh),
        Rect::new(quad.x, quad.y + hh, hw, hh),
        Rect::new(quad.x + hw, quad.y + hh, hw, hh),
    ];
    let mut quad_els = Vec::new();
    for (i, r) in qrects.iter().enumerate() {
        let mut q = vec![Element::rect(*r, 0.0, Paint::fill(&qc.fills[i]))];
        if let Some(t) = chart.quadrants[i].as_ref().filter(|t| !t.is_empty()) {
            // mermaid never wraps; we only wrap texts wider than the quadrant.
            let lines = wrap(t, q_font, r.w - 10.0);
            let lh = q_font * 1.2;
            let first = if has_points {
                r.y + q_text_top + q_font * HANGING
            } else {
                r.y + r.h / 2.0 - lh * (lines.len() as f64 - 1.0) / 2.0 + q_font * MIDDLE
            };
            let txt = text_at(
                r.x + r.w / 2.0,
                first,
                &lines,
                q_font,
                Anchor::Middle,
                &qc.texts[i],
            );
            q.push(Element::classed_text(txt, "quadrant-text"));
        }
        quad_els.push(Element::group("quadrant", q));
    }

    // `getBorders`: external frame then the internal cross.
    let he = ex_w / 2.0;
    let ext = Paint::stroke(&qc.external, ex_w);
    let int = Paint::stroke(&qc.internal, in_w);
    let (l, t, r, b) = (quad.x, quad.y, quad.right(), quad.bottom());
    let border_els = vec![
        Element::line(Point::new(l - he, t), Point::new(r + he, t), ext.clone()),
        Element::line(Point::new(r, t + he), Point::new(r, b - he), ext.clone()),
        Element::line(Point::new(l - he, b), Point::new(r + he, b), ext.clone()),
        Element::line(Point::new(l, t + he), Point::new(l, b - he), ext),
        Element::line(
            Point::new(l + hw, t + he),
            Point::new(l + hw, b - he),
            int.clone(),
        ),
        Element::line(Point::new(l + he, t + hh), Point::new(r - he, t + hh), int),
    ];

    // Points (labels sit `pointTextPadding` below the centre, hanging).
    let mut pt_els = Vec::new();
    for p in &chart.points {
        let class_style = p
            .class
            .as_ref()
            .and_then(|c| chart.classes.iter().find(|(n, _)| n == c))
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let st = p.style.over(&class_style);
        let rad = st.radius.unwrap_or(pt_radius);
        let fill = st
            .color
            .as_deref()
            .map_or_else(|| qc.point.clone(), Color::new);
        let sw = st.stroke_width.unwrap_or(0.0);
        let stroke = st
            .stroke_color
            .as_deref()
            .map_or_else(|| qc.point.clone(), Color::new);
        let c = Point::new(quad.x + p.x * quad.w, quad.bottom() - p.y * quad.h);
        let paint = if sw > 0.0 {
            Paint::both(&fill, &stroke, sw)
        } else {
            Paint::fill(&fill)
        };
        bounds.add_rect(Rect::centered(c, 2.0 * rad + sw, 2.0 * rad + sw));
        let txt = text_at(
            c.x,
            c.y + pt_pad + pt_font * HANGING,
            std::slice::from_ref(&p.name),
            pt_font,
            Anchor::Middle,
            &qc.point_text,
        );
        bounds.add_rect(text_bounds(&txt));
        pt_els.push(Element::group(
            "data-point",
            vec![
                Element::classed_shape(Geometry::Circle { c, r: rad }, paint, "quadrant-point"),
                Element::classed_text(txt, "quadrant-point-label"),
            ],
        ));
    }

    // `getAxisLabels`.
    let mut label_els = Vec::new();
    if show_x {
        let mid = nonempty(&chart.x_right);
        let anchor = if mid { Anchor::Middle } else { Anchor::Start };
        let y = if x_bottom {
            x_pad + quad.bottom() + qpad
        } else {
            x_pad + title_top
        };
        let off = if mid { hw / 2.0 } else { 0.0 };
        for (x, label) in [
            (quad.x + off, &chart.x_left),
            (quad.x + hw + off, &chart.x_right),
        ] {
            let Some(label) = label.as_ref().filter(|t| !t.is_empty()) else {
                continue;
            };
            let txt = text_at(
                x,
                y + x_font * HANGING,
                std::slice::from_ref(label),
                x_font,
                anchor,
                &qc.x_text,
            );
            bounds.add_rect(text_bounds(&txt));
            label_els.push(Element::classed_text(txt, "x-axis-label"));
        }
    }
    if show_y {
        let mid = nonempty(&chart.y_top);
        let anchor = if mid { Anchor::Middle } else { Anchor::Start };
        let x = if y_right {
            y_pad + quad.right() + qpad
        } else {
            y_pad
        };
        let off = if mid { hh / 2.0 } else { 0.0 };
        for (y, label) in [
            (quad.bottom() - off, &chart.y_bottom),
            (quad.y + hh - off, &chart.y_top),
        ] {
            let Some(label) = label.as_ref().filter(|t| !t.is_empty()) else {
                continue;
            };
            // `translate(x, y) rotate(-90)`: the text runs upwards from (x, y)
            // and its hanging top edge faces left.
            let about = Point::new(x, y);
            let txt = text_at(
                x,
                y + y_font * HANGING,
                std::slice::from_ref(label),
                y_font,
                anchor,
                &qc.y_text,
            );
            bounds.add_rotated_rect(text_bounds(&txt), about, -90.0);
            label_els.push(Element::Rotated {
                deg: -90.0,
                about,
                children: vec![Element::classed_text(txt, "y-axis-label")],
            });
        }
    }

    let mut els = vec![
        Element::group("quadrants", quad_els),
        Element::group("border", border_els),
        Element::group("data-points", pt_els),
        Element::group("labels", label_els),
    ];
    if let Some(t) = title {
        let txt = text_at(
            width / 2.0,
            title_pad + title_font * HANGING,
            std::slice::from_ref(t),
            title_font,
            Anchor::Middle,
            &qc.title,
        );
        bounds.add_rect(text_bounds(&txt));
        els.push(Element::group(
            "title",
            vec![Element::classed_text(txt, "quadrant-title")],
        ));
    }

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

    use crate::scene::Text;

    /// All `Rotated` elements (deg, about, first child text) in the tree.
    fn rotated(els: &[Element]) -> Vec<(f64, Point, Text)> {
        let mut out = Vec::new();
        for e in els {
            match e {
                Element::Group { children, .. } => out.extend(rotated(children)),
                Element::Rotated {
                    deg,
                    about,
                    children,
                } => {
                    if let Some(Element::Text { text, .. }) = children.first() {
                        out.push((*deg, *about, text.clone()));
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn y_axis_labels_are_rotated_like_mermaid() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let scene = render(SRC, None, &cfg, &pal).unwrap();
        let rot = rotated(&scene.elements);
        assert_eq!(rot.len(), 2, "{rot:?}");
        // Title space 40, x-axis at the bottom (points exist) → quadrant top 45,
        // height 500 - 10 - 26 - 40 = 424; y band = 5·2 + 16 → quadrant left 31.
        let (top, h) = (45.0, 424.0);
        let (deg, about, t) = &rot[0];
        assert_eq!(*deg, -90.0);
        assert_eq!(t.lines, vec!["Low Engagement".to_string()]);
        assert_eq!(t.anchor, Anchor::Middle);
        assert!((about.x - 5.0).abs() < 1e-9);
        assert!((about.y - (top + h - h / 4.0)).abs() < 1e-9);
        assert_eq!(rot[1].2.lines, vec!["High Engagement".to_string()]);
        assert!((rot[1].1.y - (top + h / 4.0)).abs() < 1e-9);
        // Without a top label the bottom one starts at the quadrant's bottom.
        let c = parse("quadrantChart\ny-axis Only", None).unwrap();
        let scene = draw(&c, &cfg, &pal);
        let rot = rotated(&scene.elements);
        assert_eq!(rot.len(), 1);
        assert_eq!(rot[0].2.anchor, Anchor::Start);
        assert!((rot[0].1.y - 495.0).abs() < 1e-9);
    }

    #[test]
    fn right_y_axis_band() {
        let cfg = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(r#"{"quadrantChart":{"yAxisPosition":"right"}}"#).unwrap(),
            )
            .unwrap();
        let pal = cfg.palette();
        let scene = render(SRC, None, &cfg, &pal).unwrap();
        let rot = rotated(&scene.elements);
        // Quadrant spans 5..469 (500 - 10 - 26); labels at 5 + 469 + 5.
        assert_eq!(rot.len(), 2);
        assert!(
            rot.iter().all(|(_, a, _)| (a.x - 479.0).abs() < 1e-9),
            "{rot:?}"
        );
    }

    #[test]
    fn theme_colors_follow_upstream() {
        let css = |c: &Color| c.css().to_ascii_lowercase();
        let cfg = MermaidConfig::default();
        let pal = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let q = colors(&cfg, &pal);
        // adjust(primaryColor #ECECFF, { r, g, b: +5 }), adjust(#131300, -5).
        assert_eq!(css(&q.fills[1]), "#f1f1ff");
        assert_eq!(css(&q.texts[3]), "#040400");
        // The NaN `quadrantPointFill` falls back to the root `textColor`.
        assert_eq!(css(&q.point), "#333");
        assert_eq!(css(&q.external), "hsl(240, 60%, 86.2745098039%)");
        let rc = crate::theme::palette(crate::theme::ThemeName::ReduxColor, &[]);
        let q = colors(&cfg, &rc);
        assert_eq!(css(&q.fills[0]), "#ececfe");
        assert_eq!(css(&q.internal), "#181818"); // hsl(247.5, 0%, 9.2157%)
        assert_eq!(css(&q.point), "#28253d");
    }
}
