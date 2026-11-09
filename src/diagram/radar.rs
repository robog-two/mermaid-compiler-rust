//! `radar-beta` diagrams. Geometry follows mermaid's `radarRenderer.ts`
//! (600×600 plot, 50px margins, closed Catmull-Rom-like curves with tension 0.17).

use crate::config::MermaidConfig;
use crate::diagram::xychart::{
    keyword, legend_layout, logical_lines, split_top_level, text_at, text_bounds, unquote,
};
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graticule {
    Circle,
    Polygon,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CurveValues {
    List(Vec<f64>),
    Map(Vec<(String, f64)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub id: String,
    pub label: String,
    pub values: CurveValues,
    /// Source line, for errors detected after parsing (unknown axis ids).
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Radar {
    pub title: Option<String>,
    pub axes: Vec<Axis>,
    pub curves: Vec<Curve>,
    pub show_legend: bool,
    pub max: Option<f64>,
    pub min: f64,
    pub graticule: Graticule,
    pub ticks: usize,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Split `id["Label"]rest` into (id, label, rest).
fn id_label(s: &str, line: usize) -> Result<(String, Option<String>, &str)> {
    let s = s.trim();
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(s.len());
    let id = &s[..end];
    if id.is_empty() {
        return Err(Error::parse(
            line,
            format!("Expecting an identifier, got '{s}'"),
        ));
    }
    let rest = s[end..].trim_start();
    if let Some(r) = rest.strip_prefix('[') {
        let close = r
            .find(']')
            .ok_or_else(|| Error::parse(line, "Expecting ']' after label"))?;
        return Ok((
            id.to_string(),
            Some(unquote(&r[..close])),
            r[close + 1..].trim(),
        ));
    }
    Ok((id.to_string(), None, rest))
}

fn number(s: &str, line: usize) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::parse(line, format!("Expecting a number, got '{}'", s.trim())))
}

fn parse_curve(item: &str, line: usize) -> Result<Curve> {
    let (id, label, rest) = id_label(item, line)?;
    let inner = rest
        .strip_prefix('{')
        .and_then(|r| r.trim_end().strip_suffix('}'))
        .ok_or_else(|| Error::parse(line, format!("Expecting '{{values}}' for curve '{id}'")))?;
    let parts: Vec<String> = split_top_level(inner)
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let values = if parts.iter().any(|p| p.contains(':')) {
        let mut m = Vec::new();
        for p in &parts {
            let (k, v) = p
                .split_once(':')
                .ok_or_else(|| Error::parse(line, format!("Expecting 'axis: value', got '{p}'")))?;
            m.push((k.trim().to_string(), number(v, line)?));
        }
        CurveValues::Map(m)
    } else {
        CurveValues::List(
            parts
                .iter()
                .map(|p| number(p, line))
                .collect::<Result<_>>()?,
        )
    };
    Ok(Curve {
        label: label.unwrap_or_else(|| id.clone()),
        id,
        values,
        line,
    })
}

/// Parse a radar body.
pub fn parse(body: &str, title: Option<String>) -> Result<Radar> {
    let mut r = Radar {
        title,
        axes: Vec::new(),
        curves: Vec::new(),
        show_legend: true,
        max: None,
        min: 0.0,
        graticule: Graticule::Circle,
        ticks: 5,
    };
    let mut header = false;
    for (line_no, line) in logical_lines(body) {
        let line = line.trim();
        if !header {
            let rest = keyword(line, "radar-beta")
                .or_else(|| keyword(line, "radar"))
                .or_else(|| line.strip_prefix("radar-beta:").map(str::trim))
                .ok_or_else(|| Error::parse(line_no, "Expecting 'radar-beta'"))?;
            header = true;
            if rest.is_empty() {
                continue;
            }
            return Err(Error::parse(line_no, format!("Unexpected '{rest}'")));
        }
        if let Some(t) = keyword(line, "title") {
            r.title = Some(unquote(t));
        } else if let Some(rest) = keyword(line, "axis") {
            for item in split_top_level(rest) {
                if item.is_empty() {
                    continue;
                }
                let (id, label, tail) = id_label(&item, line_no)?;
                if !tail.is_empty() {
                    return Err(Error::parse(line_no, format!("Unexpected '{tail}'")));
                }
                if r.axes.iter().any(|a| a.id == id) {
                    return Err(Error::parse(line_no, format!("Duplicate axis '{id}'")));
                }
                r.axes.push(Axis {
                    label: label.unwrap_or_else(|| id.clone()),
                    id,
                });
            }
        } else if let Some(rest) = keyword(line, "curve") {
            for item in split_top_level(rest) {
                if !item.is_empty() {
                    r.curves.push(parse_curve(&item, line_no)?);
                }
            }
        } else if let Some(v) = keyword(line, "showLegend") {
            r.show_legend = match v {
                "" | "true" => true,
                "false" => false,
                _ => return Err(Error::parse(line_no, "Expecting 'true' or 'false'")),
            };
        } else if let Some(v) = keyword(line, "max") {
            r.max = Some(number(v, line_no)?);
        } else if let Some(v) = keyword(line, "min") {
            r.min = number(v, line_no)?;
        } else if let Some(v) = keyword(line, "ticks") {
            let n = number(v, line_no)?;
            if n < 1.0 || n.fract() != 0.0 || n > 100.0 {
                return Err(Error::parse(line_no, "ticks must be a positive integer"));
            }
            r.ticks = n as usize;
        } else if let Some(v) = keyword(line, "graticule") {
            r.graticule = match v {
                "circle" => Graticule::Circle,
                "polygon" => Graticule::Polygon,
                _ => {
                    return Err(Error::parse(
                        line_no,
                        "graticule must be 'circle' or 'polygon'",
                    ));
                }
            };
        } else {
            return Err(Error::parse(
                line_no,
                format!(
                    "{line}\nExpecting 'title', 'axis', 'curve', 'showLegend', 'max', 'min', 'ticks' or 'graticule'"
                ),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'radar-beta'"));
    }
    // Validate map keys against axes.
    for c in &r.curves {
        if let CurveValues::Map(m) = &c.values {
            for (k, _) in m {
                if !r.axes.iter().any(|a| &a.id == k) {
                    return Err(Error::parse(
                        c.line,
                        format!("Curve '{}' refers to unknown axis '{k}'", c.id),
                    ));
                }
            }
        }
    }
    Ok(r)
}

impl Radar {
    /// Values of `curve` in axis order (missing entries are 0).
    pub fn values_of(&self, curve: &Curve) -> Vec<f64> {
        match &curve.values {
            CurveValues::List(v) => (0..self.axes.len())
                .map(|i| v.get(i).copied().unwrap_or(0.0))
                .collect(),
            CurveValues::Map(m) => self
                .axes
                .iter()
                .map(|a| {
                    m.iter()
                        .rev()
                        .find(|(k, _)| *k == a.id)
                        .map_or(0.0, |(_, v)| *v)
                })
                .collect(),
        }
    }

    /// Scale maximum: explicit `max`, else the largest value.
    pub fn max_value(&self) -> f64 {
        self.max.unwrap_or_else(|| {
            self.curves
                .iter()
                .flat_map(|c| self.values_of(c))
                .fold(f64::NEG_INFINITY, f64::max)
                .max(self.min + 1e-9)
        })
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// mermaid's `closedRoundCurve`: a closed cubic through every point.
pub fn closed_round_curve(pts: &[Point], tension: f64) -> Vec<PathCmd> {
    let n = pts.len();
    if n < 3 {
        return geom::polygon(pts);
    }
    let mut cmds = vec![PathCmd::MoveTo(pts[0])];
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let p1 = pts[i];
        let p2 = pts[(i + 1) % n];
        let p3 = pts[(i + 2) % n];
        let c1 = p1.plus(p2.minus(p0).scale(tension));
        let c2 = p2.minus(p3.minus(p1).scale(tension));
        cmds.push(PathCmd::CubicTo(c1, c2, p2));
    }
    cmds.push(PathCmd::Close);
    cmds
}

fn darker(c: &Color, dark: bool) -> Color {
    crate::color::parse(c.css())
        .map(|rgba| Color::new(rgba.lighten(if dark { 15.0 } else { -25.0 }).to_hex()))
        .unwrap_or_else(|| c.clone())
}

/// Render a `radar-beta` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let radar = parse(body, title)?;
    Ok(draw(&radar, cfg, pal))
}

/// Lay out and draw a parsed radar chart.
pub fn draw(radar: &Radar, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let s = "radar";
    let width = cfg.section_f64(s, "width", 600.0).max(100.0);
    let height = cfg.section_f64(s, "height", 600.0).max(100.0);
    let mt = cfg.section_f64(s, "marginTop", 50.0);
    let mb = cfg.section_f64(s, "marginBottom", 50.0);
    let ml = cfg.section_f64(s, "marginLeft", 50.0);
    let mr = cfg.section_f64(s, "marginRight", 50.0);
    let axis_scale = cfg.section_f64(s, "axisScaleFactor", 1.0);
    let label_factor = cfg.section_f64(s, "axisLabelFactor", 1.05);
    let tension = cfg.section_f64(s, "curveTension", 0.17);
    let axis_font = 12.0;
    let legend_font = 14.0;
    let legend_box = 10.0;
    let curve_opacity = 0.5;

    let total_w = width + ml + mr;
    let has_title = radar.title.as_ref().is_some_and(|t| !t.is_empty());
    let title_band = if has_title {
        pal.font_size * 1.5 + 10.0
    } else {
        0.0
    };
    let total_h = height + mt + mb + title_band;
    let center = Point::new(ml + width / 2.0, mt + title_band + height / 2.0);
    let radius = width.min(height) / 2.0;
    let n = radar.axes.len();
    let angle = |i: usize| -> f64 {
        std::f64::consts::TAU * i as f64 / n.max(1) as f64 - std::f64::consts::FRAC_PI_2
    };
    let at = |i: usize, r: f64| {
        let a = angle(i);
        Point::new(center.x + r * a.cos(), center.y + r * a.sin())
    };

    let mut els = Vec::new();
    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, total_w, total_h));
    let axis_c = pal.line_color.clone();

    // Graticule.
    let mut grat = Vec::new();
    let gp = Paint::both(&pal.main_bkg, &axis_c, 1.0).with_opacity(0.3);
    for t in (1..=radar.ticks).rev() {
        let r = radius * t as f64 / radar.ticks as f64;
        let geom = match radar.graticule {
            Graticule::Circle => Geometry::Circle { c: center, r },
            Graticule::Polygon if n >= 3 => {
                Geometry::Path(geom::polygon(&(0..n).map(|i| at(i, r)).collect::<Vec<_>>()))
            }
            Graticule::Polygon => Geometry::Circle { c: center, r },
        };
        grat.push(Element::classed_shape(geom, gp.clone(), "radarGraticule"));
    }
    els.push(Element::group("graticule", grat));

    // Axes + labels.
    let mut axes = Vec::new();
    for (i, a) in radar.axes.iter().enumerate() {
        axes.push(Element::classed_shape(
            Geometry::Path(geom::polyline(&[center, at(i, radius * axis_scale)])),
            Paint::stroke(&axis_c, 1.0),
            "radarAxisLine",
        ));
        let p = at(i, radius * axis_scale * label_factor);
        let ang = angle(i);
        let (cx, sy) = (ang.cos(), ang.sin());
        let anchor = if cx > 0.2 {
            Anchor::Start
        } else if cx < -0.2 {
            Anchor::End
        } else {
            Anchor::Middle
        };
        // Push labels away from the circle vertically near the poles.
        let baseline = p.y + axis_font * 0.35 + sy * axis_font * 0.6;
        let t = text_at(
            p.x,
            baseline,
            std::slice::from_ref(&a.label),
            axis_font,
            anchor,
            &pal.text_color,
        );
        bounds.add_rect(text_bounds(&t));
        axes.push(Element::classed_text(t, "radarAxisLabel"));
    }
    els.push(Element::group("axes", axes));

    // Curves.
    let max = radar.max_value();
    let min = radar.min;
    let span = (max - min).abs().max(1e-12);
    let mut curves = Vec::new();
    for (ci, c) in radar.curves.iter().enumerate() {
        let col = pal.scale_color(ci).clone();
        let pts: Vec<Point> = radar
            .values_of(c)
            .iter()
            .enumerate()
            .map(|(i, v)| {
                at(
                    i,
                    (v.clamp(min.min(max), max.max(min)) - min) / span * radius,
                )
            })
            .collect();
        if pts.is_empty() {
            continue;
        }
        let path = match radar.graticule {
            Graticule::Circle => closed_round_curve(&pts, tension),
            Graticule::Polygon => geom::polygon(&pts),
        };
        curves.push(Element::classed_shape(
            Geometry::Path(path.clone()),
            Paint::fill(&col).with_opacity(curve_opacity),
            format!("radarCurve-{ci}"),
        ));
        curves.push(Element::shape(
            Geometry::Path(path),
            Paint::stroke(&darker(&col, pal.dark), 2.0),
        ));
    }
    els.push(Element::group("curves", curves));

    // Legend (top right, Taffy column).
    if radar.show_legend && !radar.curves.is_empty() {
        let labels: Vec<String> = radar.curves.iter().map(|c| c.label.clone()).collect();
        let lg = legend_layout(&labels, legend_font, legend_box * 1.2, true, f64::INFINITY);
        let ox = center.x + (width / 2.0 + mr) * 0.75;
        let oy = center.y - (height / 2.0 + mt) * 0.75;
        let mut items = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            let (Some(sw), Some(lp)) = (lg.swatches.get(i), lg.labels.get(i)) else {
                continue;
            };
            let col = pal.scale_color(i).clone();
            let r = Rect::new(sw.x + ox, sw.y + oy, sw.w, sw.h);
            items.push(Element::rect(
                r,
                0.0,
                Paint::both(&col, &darker(&col, pal.dark), 1.0),
            ));
            let t = text_at(
                lp.x + ox,
                lp.y + oy,
                std::slice::from_ref(label),
                legend_font,
                Anchor::Start,
                &pal.text_color,
            );
            bounds.add_rect(text_bounds(&t));
            items.push(Element::text(t));
        }
        els.push(Element::group("legend", items));
    }

    // Title.
    if let Some(t) = radar.title.as_ref().filter(|t| !t.is_empty()) {
        let tb = TextBlock::new(vec![t.clone()], pal.font_size, false);
        let txt = Text::centered(
            Point::new(center.x, mt / 2.0 + tb.height / 2.0),
            &tb.lines,
            pal.font_size,
            tb.line_height,
            &pal.title_color,
        );
        bounds.add_rect(text_bounds(&txt));
        els.push(Element::classed_text(txt, "radarTitle"));
    }

    Scene {
        kind: DiagramKind::Radar,
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

    #[test]
    fn parses_docs_example() {
        let src = "radar-beta\n  axis m[\"Math\"], s[\"Science\"], e[\"English\"]\n  axis h[\"History\"], g[\"Geography\"], a[\"Art\"]\n  curve a[\"Alice\"]{85, 90, 80, 70, 75, 90}\n  curve b[\"Bob\"]{70, 75, 85, 80, 90, 85}\n\n  max 100\n  min 0\n";
        let r = parse(src, Some("Grades".into())).unwrap();
        assert_eq!(r.axes.len(), 6);
        assert_eq!(r.axes[0].label, "Math");
        assert_eq!(r.curves[1].label, "Bob");
        assert_eq!(r.max, Some(100.0));
        assert_eq!(r.values_of(&r.curves[0])[5], 90.0);
    }

    #[test]
    fn parses_maps_and_options() {
        let src = "radar-beta\naxis a1, a2, a3\ncurve id1[\"L1\"]{1, 2, 3}, id2{4, 5, 6}\ncurve id4{ a3: 30, a1: 20 }\nshowLegend false\ngraticule polygon\nticks 4\n";
        let r = parse(src, None).unwrap();
        assert_eq!(r.curves.len(), 3);
        assert_eq!(r.curves[1].label, "id2");
        assert_eq!(r.values_of(&r.curves[2]), vec![20.0, 0.0, 30.0]);
        assert!(!r.show_legend);
        assert_eq!(r.graticule, Graticule::Polygon);
        assert_eq!(r.ticks, 4);
        assert_eq!(r.max_value(), 30.0);
    }

    #[test]
    fn multiline_curve() {
        let r = parse("radar-beta\naxis a, b\ncurve c{\n  a: 1,\n  b: 2\n}", None).unwrap();
        assert_eq!(r.values_of(&r.curves[0]), vec![1.0, 2.0]);
    }

    #[test]
    fn errors() {
        let e = parse("radar-beta\naxis a\ncurve c{ z: 1 }", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("radar-beta\n\ngraticule star", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("radar-beta\ncurve c{1, x}", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("radar-beta\nbogus", None).is_err());
    }

    #[test]
    fn closed_curve_passes_through_points() {
        let pts = [
            Point::new(0.0, -10.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0),
        ];
        let c = closed_round_curve(&pts, 0.17);
        assert_eq!(c.len(), 5);
        assert!(matches!(c[1], PathCmd::CubicTo(_, _, p) if p == pts[1]));
        assert!(matches!(c[3], PathCmd::CubicTo(_, _, p) if p == pts[0]));
    }

    #[test]
    fn renders() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render("radar-beta\naxis a,b,c\ncurve x{1,2,3}", None, &cfg, &pal).unwrap();
        assert_eq!(s.kind, DiagramKind::Radar);
    }
}
