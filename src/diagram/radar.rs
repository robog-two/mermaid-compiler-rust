//! `radar-beta` diagrams. Geometry follows mermaid's `radarRenderer.ts`
//! (600×600 plot, 50px margins, closed Catmull-Rom-like curves with tension 0.17).

use crate::config::MermaidConfig;
use crate::diagram::xychart::{keyword, logical_lines, split_top_level, text_at, unquote};
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{Baseline, baseline_shift};
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
            // radar.langium: `axis` takes at least one axis.
            if rest.trim().is_empty() {
                return Err(Error::parse(
                    line_no,
                    format!("{line}\nExpecting an axis id"),
                ));
            }
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
            if n < 1.0 || n.fract() != 0.0 {
                return Err(Error::parse(line_no, "ticks must be a positive integer"));
            }
            // `MAX_TICKS`: mermaid warns and draws 32.
            r.ticks = n.min(32.0) as usize;
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
            // `computeCurveEntries` needs an entry for every axis.
            if let Some(a) = r.axes.iter().find(|a| !m.iter().any(|(k, _)| *k == a.id)) {
                return Err(Error::parse(
                    c.line,
                    format!("Missing entry for axis {}", a.label),
                ));
            }
        }
    }
    Ok(r)
}

impl Radar {
    /// Values of `curve` in axis order (missing entries are 0).
    pub fn values_of(&self, curve: &Curve) -> Vec<f64> {
        match &curve.values {
            CurveValues::List(v) => v.clone(),
            CurveValues::Map(m) => self
                .axes
                .iter()
                .map(|a| m.iter().find(|(k, _)| *k == a.id).map_or(0.0, |(_, v)| *v))
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

/// theme-*.js `radar.graticuleColor` default.
const GRATICULE: &str = "#DEDEDE";

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

/// Lay out and draw a parsed radar chart (`radar/renderer.ts`, `radar/styles.ts`).
pub fn draw(radar: &Radar, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let s = "radar";
    let width = cfg.section_f64(s, "width", 600.0);
    let height = cfg.section_f64(s, "height", 600.0);
    let mt = cfg.section_f64(s, "marginTop", 50.0);
    let mb = cfg.section_f64(s, "marginBottom", 50.0);
    let ml = cfg.section_f64(s, "marginLeft", 50.0);
    let mr = cfg.section_f64(s, "marginRight", 50.0);
    let axis_scale = cfg.section_f64(s, "axisScaleFactor", 1.0);
    let label_factor = cfg.section_f64(s, "axisLabelFactor", 1.05);
    let tension = cfg.section_f64(s, "curveTension", 0.17);
    // `themeVariables.radar`, over the theme's defaults.
    let tv = |k: &str| cfg.section_nested("themeVariables", "radar", k);
    let tv_num = |k: &str, d: f64| {
        tv(k).map_or(d, |j| match j {
            crate::json::Json::String(v) => v.trim().trim_end_matches("px").parse().unwrap_or(d),
            _ => j.as_f64().unwrap_or(d),
        })
    };
    let tv_color = |k: &str, d: &Color| {
        tv(k)
            .and_then(crate::json::Json::as_str)
            .map_or_else(|| d.clone(), |v| Color::new(v.trim()))
    };
    let axis_font = tv_num("axisLabelFontSize", 12.0);
    let legend_font = tv_num("legendFontSize", 12.0);
    let curve_opacity = tv_num("curveOpacity", 0.5);
    let curve_stroke = tv_num("curveStrokeWidth", 2.0);
    let axis_stroke = tv_num("axisStrokeWidth", 2.0);
    let grat_opacity = tv_num("graticuleOpacity", 0.3);
    let grat_stroke = tv_num("graticuleStrokeWidth", 1.0);

    // `drawFrame`: the viewBox is the plot plus its margins; everything is drawn
    // from the plot's centre.
    let total_w = width + ml + mr;
    let total_h = height + mt + mb;
    let center = Point::new(ml + width / 2.0, mt + height / 2.0);
    let radius = width.min(height) / 2.0;
    let n = radar.axes.len();
    let angle = |i: usize| -> f64 {
        2.0 * i as f64 * std::f64::consts::PI / n as f64 - std::f64::consts::FRAC_PI_2
    };
    let at = |a: f64, r: f64| Point::new(center.x + r * a.cos(), center.y + r * a.sin());

    let mut els = Vec::new();
    let axis_c = tv_color("axisColor", &pal.line_color);
    // Unstyled text fills inherit the root's `fill: textColor`.
    let text_c = pal.text_color.clone();

    // `drawGraticule`: `.radarGraticule` fills `graticuleColor` at fill-opacity
    // 0.3 and strokes it 1px, smallest ring first.
    let grat_c = tv_color("graticuleColor", &Color::new(GRATICULE));
    let grat_paint = Paint::both(&grat_c, &grat_c, grat_stroke).with_fill_opacity(grat_opacity);
    for t in 0..radar.ticks {
        let r = radius * (t + 1) as f64 / radar.ticks as f64;
        let geom = match radar.graticule {
            Graticule::Circle => Geometry::Circle { c: center, r },
            Graticule::Polygon => Geometry::Path(geom::polygon(
                &(0..n).map(|j| at(angle(j), r)).collect::<Vec<_>>(),
            )),
        };
        els.push(Element::classed_shape(
            geom,
            grat_paint.clone(),
            "radarGraticule",
        ));
    }

    // `drawAxes`: a line per axis and its label 4px beyond `axisLabelFactor`,
    // anchored and baselined away from the centre.
    for (i, a) in radar.axes.iter().enumerate() {
        let ang = angle(i);
        let (cos_a, sin_a) = (ang.cos(), ang.sin());
        els.push(Element::classed_shape(
            Geometry::Path(geom::polyline(&[
                center,
                Point::new(
                    center.x + radius * axis_scale * cos_a,
                    center.y + radius * axis_scale * sin_a,
                ),
            ])),
            Paint::stroke(&axis_c, axis_stroke),
            "radarAxisLine",
        ));
        let anchor = if cos_a > 0.01 {
            Anchor::Start
        } else if cos_a < -0.01 {
            Anchor::End
        } else {
            Anchor::Middle
        };
        let baseline = if sin_a > 0.01 {
            Baseline::Hanging
        } else if sin_a < -0.01 {
            Baseline::Alphabetic
        } else {
            Baseline::Central
        };
        let x = center.x + radius * label_factor * cos_a + 4.0 * cos_a;
        let y = center.y + radius * label_factor * sin_a + 4.0 * sin_a;
        let t = text_at(
            x,
            y + baseline_shift(baseline, axis_font),
            std::slice::from_ref(&a.label),
            axis_font,
            anchor,
            &text_c,
        );
        els.push(Element::classed_text(t, "radarAxisLabel"));
    }

    // `drawCurves`: curves without an entry per axis are skipped.
    let max = radar.max_value();
    let min = radar.min;
    for (ci, c) in radar.curves.iter().enumerate() {
        let values = radar.values_of(c);
        if values.len() != n {
            continue;
        }
        let col = pal.scale_color(ci).clone();
        let pts: Vec<Point> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let r = radius * (v.max(min).min(max) - min) / (max - min);
                at(
                    2.0 * std::f64::consts::PI * i as f64 / n as f64 - std::f64::consts::FRAC_PI_2,
                    r,
                )
            })
            .collect();
        let path = match radar.graticule {
            Graticule::Circle => closed_round_curve(&pts, tension),
            Graticule::Polygon => geom::polygon(&pts),
        };
        els.push(Element::classed_shape(
            Geometry::Path(path),
            Paint::both(&col, &col, curve_stroke).with_fill_opacity(curve_opacity),
            format!("radarCurve-{ci}"),
        ));
    }

    // `drawLegend`: 12px boxes 20px apart, three quarters of the way to the
    // top-right corner; `hanging` labels 16px to their right.
    if radar.show_legend {
        let lx = center.x + (width / 2.0 + mr) * 3.0 / 4.0;
        let ly = center.y - (height / 2.0 + mt) * 3.0 / 4.0;
        for (i, c) in radar.curves.iter().enumerate() {
            let col = pal.scale_color(i).clone();
            let y = ly + i as f64 * 20.0;
            els.push(Element::classed_shape(
                Geometry::Rect {
                    rect: Rect::new(lx, y, 12.0, 12.0),
                    rx: 0.0,
                },
                Paint::both(&col, &col, 1.0).with_fill_opacity(curve_opacity),
                format!("radarLegendBox-{i}"),
            ));
            let t = text_at(
                lx + 16.0,
                y + baseline_shift(Baseline::Hanging, legend_font),
                std::slice::from_ref(&c.label),
                legend_font,
                Anchor::Start,
                &text_c,
            );
            els.push(Element::classed_text(t, "radarLegendText"));
        }
    }

    // Title: `hanging` from the svg's top edge, centred; `.radarTitle` sets only
    // `color`, so the text keeps the inherited `fill: textColor`.
    if let Some(t) = radar.title.as_ref().filter(|t| !t.is_empty()) {
        let fs = pal.font_size;
        let txt = Text {
            x: center.x,
            y: center.y - height / 2.0 - mt + baseline_shift(Baseline::Hanging, fs),
            lines: vec![t.clone()],
            line_height: fs,
            font_size: fs,
            bold: false,
            weight: None,
            anchor: Anchor::Middle,
            fill: text_c.clone(),
            tspan_central: false,
        };
        els.push(Element::classed_text(txt, "radarTitle"));
    }

    Scene {
        kind: DiagramKind::Radar,
        view_box: Rect::new(0.0, 0.0, total_w, total_h),
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
        let src = "radar-beta\naxis a1, a2, a3\ncurve id1[\"L1\"]{1, 2, 3}, id2{4, 5, 6}\ncurve id4{ a3: 30, a1: 20, a2: 10, a1: 5 }\nshowLegend false\ngraticule polygon\nticks 4\n";
        let r = parse(src, None).unwrap();
        assert_eq!(r.curves.len(), 3);
        assert_eq!(r.curves[1].label, "id2");
        // The first entry for an axis counts (`entries.find`).
        assert_eq!(r.values_of(&r.curves[2]), vec![20.0, 10.0, 30.0]);
        // Every axis needs an entry (`computeCurveEntries`).
        let e = parse("radar-beta\naxis a, b\ncurve c{ a: 1 }\n", None).unwrap_err();
        assert!(e.to_string().contains("Missing entry for axis b"), "{e}");
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

    #[test]
    fn title_takes_no_room() {
        // mermaid 12.3.0: the title sits on the top edge of the fixed-size chart
        // (`dominant-baseline: hanging` at y 0); the drawing does not move.
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let src = "radar-beta\naxis a,b,c\ncurve x{1,2,3}";
        let plain = render(src, None, &cfg, &pal).unwrap();
        let titled = render(src, Some("Grades".into()), &cfg, &pal).unwrap();
        assert_eq!(plain.view_box, titled.view_box);
        // `viewBox="0 0 700 700"`: the plot plus its margins, no extra padding.
        assert_eq!(plain.view_box, Rect::new(0.0, 0.0, 700.0, 700.0));
    }

    fn walk(els: &[Element], f: &mut impl FnMut(&Element)) {
        for e in els {
            f(e);
            if let Element::Group { children, .. } = e {
                walk(children, f);
            }
        }
    }

    #[test]
    fn follows_the_mermaid_renderer() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let src = "radar-beta\naxis a,b,c\ncurve x{1,2,3}\ncurve short{1,2}";
        let s = render(src, Some("T".into()), &cfg, &pal).unwrap();
        let (mut curves, mut texts) = (Vec::new(), Vec::new());
        walk(&s.elements, &mut |e| match e {
            Element::Shape { paint, class, .. }
                if class
                    .as_deref()
                    .is_some_and(|c| c.starts_with("radarCurve")) =>
            {
                curves.push(paint.clone())
            }
            Element::Text { text, .. } => texts.push(text.clone()),
            _ => {}
        });
        // A curve without a value per axis is not drawn; one element per curve,
        // translucent fill under an opaque stroke.
        assert_eq!(curves.len(), 1);
        assert_eq!(curves[0].fill_opacity, Some(0.5));
        assert_eq!(curves[0].opacity, None);
        // Axis `a` points up: its label's baseline is 4px beyond 1.05 r (auto).
        let a = texts.iter().find(|t| t.lines[0] == "a").unwrap();
        assert_eq!(a.y, 350.0 - 300.0 * 1.05 - 4.0);
        // The title hangs from the top edge (16px: 11.203).
        let t = texts.iter().find(|t| t.lines[0] == "T").unwrap();
        assert_eq!(t.y, 11.203125);
        // The legend's labels hang 16px right of their 12px boxes.
        let l = texts.iter().find(|t| t.lines[0] == "x").unwrap();
        assert_eq!((l.x, l.y), (350.0 + 262.5 + 16.0, 350.0 - 262.5 + 8.796875));
    }
}
