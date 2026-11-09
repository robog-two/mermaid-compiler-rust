//! `xychart-beta` / `xychart` diagrams: bar and line series over a categorical
//! or numeric x-axis, vertical or horizontal.
//!
//! Also hosts small chart helpers shared by the other chart modules
//! (quadrant, radar, sankey): "nice" tick computation, number formatting,
//! quote-aware list splitting and a Taffy-based legend layout.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, Point, Rect};
use crate::layout::boxes::{self, BoxNode};
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text::{TextBlock, line_width, normalize_label};
use crate::theme::{Palette, ThemeName};

// ---------------------------------------------------------------------------
// Shared chart helpers
// ---------------------------------------------------------------------------

/// A "nice" step (1, 2 or 5 × 10ⁿ) close to `span / count`.
pub(crate) fn nice_step(span: f64, count: usize) -> f64 {
    let span = span.abs();
    if !span.is_finite() || span <= 0.0 {
        return 1.0;
    }
    let raw = span / count.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let frac = raw / mag;
    let nice = if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Tick values that are multiples of `step` within `[lo, hi]` (inclusive, with tolerance).
pub(crate) fn ticks_in(lo: f64, hi: f64, step: f64) -> Vec<f64> {
    if step.is_nan() || step <= 0.0 || !lo.is_finite() || !hi.is_finite() || hi < lo {
        return Vec::new();
    }
    let eps = step * 1e-9;
    let first = (lo / step - 1e-9).ceil();
    let mut out = Vec::new();
    let mut k = first;
    while k * step <= hi + eps && out.len() < 1000 {
        let v = k * step;
        // Snap away floating noise (0.30000000000000004 → 0.3).
        out.push(round_to_step(v, step));
        k += 1.0;
    }
    out
}

fn round_to_step(v: f64, step: f64) -> f64 {
    let decimals = (-step.log10().floor()).max(0.0) as i32 + 1;
    let p = 10f64.powi(decimals.min(12));
    let r = (v * p).round() / p;
    if r == 0.0 { 0.0 } else { r }
}

/// Expand `[lo, hi]` outward to multiples of a nice step; returns `(lo, hi, step)`.
pub(crate) fn nice_range(lo: f64, hi: f64, count: usize) -> (f64, f64, f64) {
    let (mut lo, mut hi) = (lo.min(hi), lo.max(hi));
    if (hi - lo).abs() < 1e-12 {
        let d = if lo.abs() < 1e-12 {
            1.0
        } else {
            lo.abs() * 0.1
        };
        lo -= d;
        hi += d;
    }
    let step = nice_step(hi - lo, count);
    let nlo = round_to_step((lo / step + 1e-9).floor() * step, step);
    let nhi = round_to_step((hi / step - 1e-9).ceil() * step, step);
    (nlo, nhi, step)
}

/// Format a number like JavaScript's `String(n)` for typical chart values
/// (no exponent, no trailing zeros, no floating noise).
pub(crate) fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        return format!("{}", v.round() as i64);
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Split on commas outside double quotes and brackets/braces.
pub(crate) fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            '[' | '{' if !in_q => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' if !in_q => {
                depth -= 1;
                cur.push(c);
            }
            ',' if !in_q && depth <= 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() || !out.is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Strip one pair of surrounding double quotes and normalise entities/markup.
pub(crate) fn unquote(s: &str) -> String {
    normalize_label(s.trim())
}

/// Join physical lines while a `[`/`{` opened on an earlier line is still open,
/// so lists may span lines. Yields `(first line number, text)`; blank lines dropped.
pub(crate) fn logical_lines(body: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut pending: Option<(usize, String, i32)> = None;
    for (i, raw) in body.lines().enumerate() {
        let line = raw.trim();
        let mut depth = pending.as_ref().map_or(0, |p| p.2);
        let mut in_q = false;
        for c in line.chars() {
            match c {
                '"' => in_q = !in_q,
                '[' | '{' if !in_q => depth += 1,
                ']' | '}' if !in_q => depth -= 1,
                _ => {}
            }
        }
        match pending.take() {
            Some((n, mut text, _)) => {
                if !line.is_empty() {
                    text.push(' ');
                    text.push_str(line);
                }
                if depth > 0 {
                    pending = Some((n, text, depth));
                } else {
                    out.push((n, text));
                }
            }
            None if line.is_empty() => {}
            None if depth > 0 => pending = Some((i + 1, line.to_string(), depth)),
            None => out.push((i + 1, line.to_string())),
        }
    }
    if let Some((n, text, _)) = pending {
        out.push((n, text));
    }
    out
}

/// Strip a leading keyword followed by whitespace (or end of line).
pub(crate) fn keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// A laid-out legend: a coloured swatch and a label per entry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegendLayout {
    pub swatches: Vec<Rect>,
    /// Baseline-start position of each label.
    pub labels: Vec<Point>,
    pub size: (f64, f64),
}

/// Lay out legend entries with Taffy, either as a wrapping row (max width `max_w`)
/// or as a single column. Coordinates are relative to the legend's top-left.
pub(crate) fn legend_layout(
    labels: &[String],
    font: f64,
    swatch: f64,
    vertical: bool,
    max_w: f64,
) -> LegendLayout {
    let line_h = font * 1.4;
    let item_h = line_h.max(swatch);
    let items: Vec<BoxNode> = labels
        .iter()
        .map(|l| {
            BoxNode::new(
                boxes::row(6.0),
                vec![
                    BoxNode::leaf(swatch, item_h),
                    BoxNode::leaf(line_width(l, font, false), item_h),
                ],
            )
        })
        .collect();
    let root = if vertical {
        BoxNode::new(boxes::column(4.0), items)
    } else {
        BoxNode::new(boxes::wrap_row(18.0, max_w.max(swatch * 4.0)), items)
    };
    let placed = boxes::compute(&root);
    let mut sw = Vec::new();
    let mut lp = Vec::new();
    for item in &placed.children {
        let (Some(b), Some(t)) = (item.children.first(), item.children.get(1)) else {
            continue;
        };
        sw.push(Rect::new(
            b.rect.x,
            b.rect.y + (item_h - swatch) / 2.0,
            swatch,
            swatch,
        ));
        lp.push(Point::new(t.rect.x, t.rect.y + item_h / 2.0 + font * 0.35));
    }
    LegendLayout {
        swatches: sw,
        labels: lp,
        size: (placed.rect.w, placed.rect.h),
    }
}

/// Plain text element helper.
pub(crate) fn text_at(
    x: f64,
    baseline: f64,
    lines: &[String],
    size: f64,
    anchor: Anchor,
    fill: &Color,
) -> Text {
    Text {
        x,
        y: baseline,
        lines: lines.to_vec(),
        line_height: size * 1.2,
        font_size: size,
        bold: false,
        anchor,
        fill: fill.clone(),
    }
}

/// Bounding box of a [`Text`] (approximate, from measured widths).
pub(crate) fn text_bounds(t: &Text) -> Rect {
    let w = t
        .lines
        .iter()
        .map(|l| line_width(l, t.font_size, t.bold))
        .fold(0.0, f64::max);
    let h = t.line_height * t.lines.len().max(1) as f64;
    let x = match t.anchor {
        Anchor::Start => t.x,
        Anchor::Middle => t.x - w / 2.0,
        Anchor::End => t.x - w,
    };
    let top = t.y - t.font_size * 0.85;
    Rect::new(x, top, w, h - (t.line_height - t.font_size))
}

// ---------------------------------------------------------------------------
// IR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum XAxis {
    /// No `x-axis` data given: categories are 1..n.
    Auto,
    Categories(Vec<String>),
    Range(f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesKind {
    Bar,
    Line,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub kind: SeriesKind,
    pub name: Option<String>,
    pub values: Vec<f64>,
    /// Optional per-point labels (`line [540 "PaLM", ...]`).
    pub labels: Vec<Option<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XyChart {
    pub orientation: Orientation,
    pub title: Option<String>,
    pub x_title: Option<String>,
    pub x_axis: XAxis,
    pub y_title: Option<String>,
    pub y_range: Option<(f64, f64)>,
    pub series: Vec<Series>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn parse_number(s: &str, line: usize) -> Result<f64> {
    let t = s.trim();
    t.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::parse(line, format!("Expecting a number, got '{t}'")))
}

/// Split `"quoted title" rest` or `title rest-starting-with-[` into (title, rest).
fn split_title(rest: &str) -> (Option<String>, &str) {
    let rest = rest.trim();
    if let Some(q) = rest.strip_prefix('"') {
        if let Some(end) = q.find('"') {
            let t = unquote(&q[..end]);
            return (Some(t), q[end + 1..].trim());
        }
    }
    (None, rest)
}

/// Parse `title? (min --> max | [list])?` for an axis.
fn parse_axis(rest: &str, line: usize, allow_categories: bool) -> Result<(Option<String>, XAxis)> {
    let (quoted, after) = split_title(rest);
    let mut title = quoted;
    let mut body = after;
    if let Some(open) = body.find('[') {
        if !allow_categories {
            return Err(Error::parse(line, "y-axis does not accept categories"));
        }
        let head = body[..open].trim();
        if !head.is_empty() {
            if title.is_some() {
                return Err(Error::parse(
                    line,
                    format!("Unexpected '{head}' before '['"),
                ));
            }
            title = Some(unquote(head));
        }
        let inner = body[open + 1..]
            .strip_suffix(']')
            .or_else(|| body[open + 1..].trim_end().strip_suffix(']'))
            .ok_or_else(|| Error::parse(line, "Expecting ']' to close the category list"))?;
        let cats: Vec<String> = split_top_level(inner)
            .iter()
            .filter(|c| !c.is_empty())
            .map(|c| unquote(c))
            .collect();
        return Ok((title, XAxis::Categories(cats)));
    }
    if let Some(arrow) = body.find("-->") {
        let left = body[..arrow].trim();
        let right = body[arrow + 3..].trim();
        let (head, min_s) = match left.rfind(char::is_whitespace) {
            Some(i) => (left[..i].trim(), left[i..].trim()),
            None => ("", left),
        };
        if !head.is_empty() {
            if title.is_some() {
                return Err(Error::parse(line, format!("Unexpected '{head}'")));
            }
            title = Some(unquote(head));
        }
        let min = parse_number(min_s, line)?;
        let max = parse_number(right, line)?;
        return Ok((title, XAxis::Range(min, max)));
    }
    body = body.trim();
    if !body.is_empty() {
        if title.is_some() {
            return Err(Error::parse(line, format!("Unexpected '{body}'")));
        }
        title = Some(unquote(body));
    }
    Ok((title, XAxis::Auto))
}

fn parse_series(kind: SeriesKind, rest: &str, line: usize) -> Result<Series> {
    let (quoted, after) = split_title(rest);
    let open = after
        .find('[')
        .ok_or_else(|| Error::parse(line, "Expecting a data list like [1, 2, 3]"))?;
    let head = after[..open].trim();
    let name = match (quoted, head.is_empty()) {
        (Some(q), true) => Some(q),
        (None, false) => Some(unquote(head)),
        (None, true) => None,
        (Some(_), false) => return Err(Error::parse(line, format!("Unexpected '{head}'"))),
    };
    let inner = after[open + 1..]
        .trim_end()
        .strip_suffix(']')
        .ok_or_else(|| Error::parse(line, "Expecting ']' to close the data list"))?;
    let mut values = Vec::new();
    let mut labels = Vec::new();
    for item in split_top_level(inner) {
        if item.is_empty() {
            return Err(Error::parse(line, "Empty value in data list"));
        }
        let (num, label) = match item.find('"') {
            Some(q) => (item[..q].trim(), Some(unquote(&item[q..]))),
            None => (item.as_str(), None),
        };
        values.push(parse_number(num, line)?);
        labels.push(label);
    }
    Ok(Series {
        kind,
        name,
        values,
        labels,
    })
}

/// Parse an xychart body.
pub fn parse(body: &str, title: Option<String>) -> Result<XyChart> {
    let mut chart = XyChart {
        orientation: Orientation::Vertical,
        title,
        x_title: None,
        x_axis: XAxis::Auto,
        y_title: None,
        y_range: None,
        series: Vec::new(),
    };
    let mut header = false;
    for (line_no, line) in logical_lines(body) {
        let line = line.trim().trim_end_matches(';').trim();
        if !header {
            let rest = keyword(line, "xychart-beta")
                .or_else(|| keyword(line, "xychart"))
                .ok_or_else(|| Error::parse(line_no, "Expecting 'xychart-beta'"))?;
            header = true;
            match rest {
                "" | "vertical" => {}
                "horizontal" => chart.orientation = Orientation::Horizontal,
                other => {
                    return Err(Error::parse(
                        line_no,
                        format!("Unexpected '{other}' after 'xychart-beta'"),
                    ));
                }
            }
            continue;
        }
        if line == "horizontal" {
            chart.orientation = Orientation::Horizontal;
        } else if line == "vertical" {
            chart.orientation = Orientation::Vertical;
        } else if let Some(t) = keyword(line, "title") {
            chart.title = Some(unquote(t));
        } else if let Some(rest) = keyword(line, "x-axis") {
            let (t, axis) = parse_axis(rest, line_no, true)?;
            chart.x_title = t.or(chart.x_title.take());
            if axis != XAxis::Auto {
                chart.x_axis = axis;
            }
        } else if let Some(rest) = keyword(line, "y-axis") {
            let (t, axis) = parse_axis(rest, line_no, false)?;
            chart.y_title = t.or(chart.y_title.take());
            if let XAxis::Range(a, b) = axis {
                chart.y_range = Some((a, b));
            }
        } else if let Some(rest) = keyword(line, "bar") {
            chart
                .series
                .push(parse_series(SeriesKind::Bar, rest, line_no)?);
        } else if let Some(rest) = keyword(line, "line") {
            chart
                .series
                .push(parse_series(SeriesKind::Line, rest, line_no)?);
        } else {
            return Err(Error::parse(
                line_no,
                format!(
                    "{line}\nExpecting 'title', 'x-axis', 'y-axis', 'bar', 'line' or 'horizontal'"
                ),
            ));
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'xychart-beta'"));
    }
    Ok(chart)
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// Settings read from the `xyChart` config section.
#[derive(Debug, Clone, PartialEq)]
pub struct XyConfig {
    pub width: f64,
    pub height: f64,
    pub title_font: f64,
    pub title_padding: f64,
    pub show_title: bool,
    pub show_legend: bool,
    pub legend_font: f64,
    pub label_font: f64,
    pub axis_title_font: f64,
    pub tick_length: f64,
    pub show_data_label: bool,
    pub data_label_outside: bool,
    pub horizontal_cfg: bool,
}

impl XyConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let s = "xyChart";
        let axis_f64 = |key: &str, d: f64| {
            cfg.section(s, "xAxis")
                .and_then(|a| a.get(key))
                .and_then(crate::json::Json::as_f64)
                .unwrap_or(d)
        };
        XyConfig {
            width: cfg.section_f64(s, "width", 700.0),
            height: cfg.section_f64(s, "height", 500.0),
            title_font: cfg.section_f64(s, "titleFontSize", 20.0),
            title_padding: cfg.section_f64(s, "titlePadding", 10.0),
            show_title: cfg.section_bool(s, "showTitle", true),
            show_legend: cfg.section_bool(s, "showLegend", true),
            legend_font: cfg.section_f64(s, "legendFontSize", 14.0),
            label_font: axis_f64("labelFontSize", 14.0),
            axis_title_font: axis_f64("titleFontSize", 16.0),
            tick_length: axis_f64("tickLength", 5.0),
            show_data_label: cfg.section_bool(s, "showDataLabel", false),
            data_label_outside: cfg.section_bool(s, "showDataLabelOutsideBar", false),
            horizontal_cfg: cfg
                .section(s, "chartOrientation")
                .and_then(crate::json::Json::as_str)
                == Some("horizontal"),
        }
    }
}

/// Series colours: `themeVariables.xyChart.plotColorPalette`, else a theme default.
fn plot_colors(cfg: &MermaidConfig, pal: &Palette) -> Vec<Color> {
    if let Some(list) = cfg
        .section("themeVariables", "xyChart")
        .and_then(|x| x.get("plotColorPalette"))
        .and_then(crate::json::Json::as_str)
    {
        let v: Vec<Color> = list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Color::new)
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    if cfg.theme == ThemeName::Default {
        return [
            "#ECECFF", "#8493A6", "#FFC3A0", "#DCDDE1", "#B8E994", "#D6A2E8", "#FFC300", "#F8B4B6",
            "#AEC6CF", "#A8D8EA",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect();
    }
    if pal.dark {
        return [
            "#5b8ff9", "#5ad8a6", "#f6bd16", "#e86452", "#6dc8ec", "#945fb9", "#ff9845", "#1e9493",
            "#ff99c3", "#5d7092",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect();
    }
    let mut v = vec![pal.main_bkg.clone(), pal.node_border.clone()];
    v.extend(pal.pie_colors.iter().skip(2).cloned());
    v
}

/// Value axis after range resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueScale {
    pub lo: f64,
    pub hi: f64,
    pub ticks: Vec<f64>,
}

impl ValueScale {
    /// Resolve the value axis: explicit range keeps its bounds, otherwise the data
    /// range (including 0 when there are bars) is widened to nice ticks.
    pub fn resolve(chart: &XyChart, target_ticks: usize) -> ValueScale {
        if let Some((a, b)) = chart.y_range {
            let (lo, hi) = (a.min(b), a.max(b));
            let hi = if (hi - lo).abs() < 1e-12 {
                lo + 1.0
            } else {
                hi
            };
            let step = nice_step(hi - lo, target_ticks);
            return ValueScale {
                lo,
                hi,
                ticks: ticks_in(lo, hi, step),
            };
        }
        let all = chart.series.iter().flat_map(|s| s.values.iter().copied());
        let (mut lo, mut hi) = all.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
        if !lo.is_finite() {
            lo = 0.0;
            hi = 1.0;
        }
        if chart.series.iter().any(|s| s.kind == SeriesKind::Bar) {
            lo = lo.min(0.0);
            hi = hi.max(0.0);
        }
        let (lo, hi, step) = nice_range(lo, hi, target_ticks);
        ValueScale {
            lo,
            hi,
            ticks: ticks_in(lo, hi, step),
        }
    }

    fn frac(&self, v: f64) -> f64 {
        if (self.hi - self.lo).abs() < 1e-12 {
            0.0
        } else {
            (v - self.lo) / (self.hi - self.lo)
        }
    }
}

/// Wrap a category label onto lines no wider than `max_w` (word boundaries).
fn wrap_words(s: &str, font: f64, max_w: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        match lines.last_mut() {
            Some(last) if line_width(&format!("{last} {word}"), font, false) <= max_w => {
                last.push(' ');
                last.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Render an `xychart-beta` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let mut chart = parse(body, title)?;
    let xc = XyConfig::from_config(cfg);
    if xc.horizontal_cfg {
        chart.orientation = Orientation::Horizontal;
    }
    Ok(draw(&chart, &xc, &plot_colors(cfg, pal), pal))
}

/// Lay out and draw a parsed chart.
pub fn draw(chart: &XyChart, xc: &XyConfig, colors: &[Color], pal: &Palette) -> Scene {
    let text_c = pal.text_color.clone();
    let axis_c = pal.text_color.clone();
    let color_of = |i: usize| colors[i % colors.len().max(1)].clone();
    let w = xc.width.max(200.0);
    let h = xc.height.max(150.0);
    let horizontal = chart.orientation == Orientation::Horizontal;
    let n_points = chart
        .series
        .iter()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0);

    let categories: Vec<String> = match &chart.x_axis {
        XAxis::Categories(c) => c.clone(),
        XAxis::Auto => (1..=n_points.max(1)).map(|i| i.to_string()).collect(),
        XAxis::Range(..) => Vec::new(),
    };
    let n_slots = if categories.is_empty() {
        n_points.max(1)
    } else {
        categories.len().max(n_points).max(1)
    };

    let mut els: Vec<Element> = Vec::new();
    let mut bounds = Bounds::default();
    bounds.add_rect(Rect::new(0.0, 0.0, w, h));

    // --- Title band.
    let mut top = 10.0;
    if let Some(t) = chart
        .title
        .as_ref()
        .filter(|t| xc.show_title && !t.is_empty())
    {
        let tb = TextBlock::new(vec![t.clone()], xc.title_font, false);
        top = xc.title_padding;
        let c = Point::new(w / 2.0, top + tb.height / 2.0);
        els.push(Element::classed_text(
            Text::centered(
                c,
                &tb.lines,
                xc.title_font,
                tb.line_height,
                &pal.title_color,
            ),
            "chart-title",
        ));
        top += tb.height + xc.title_padding;
    }

    // --- Legend band (only when some series is named).
    let legend_names: Vec<(usize, String)> = chart
        .series
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.name.clone().map(|n| (i, n)))
        .collect();
    if xc.show_legend && !legend_names.is_empty() {
        let labels: Vec<String> = legend_names.iter().map(|(_, n)| n.clone()).collect();
        let lg = legend_layout(&labels, xc.legend_font, 12.0, false, w - 40.0);
        let ox = (w - lg.size.0) / 2.0;
        let mut items = Vec::new();
        for (k, (si, name)) in legend_names.iter().enumerate() {
            let (Some(sw), Some(lp)) = (lg.swatches.get(k), lg.labels.get(k)) else {
                continue;
            };
            let r = Rect::new(sw.x + ox, sw.y + top, sw.w, sw.h);
            let col = color_of(*si);
            if chart.series[*si].kind == SeriesKind::Line {
                let col = line_color(&col, pal.dark);
                let y = r.y + r.h / 2.0;
                items.push(Element::line(
                    Point::new(r.x - 2.0, y),
                    Point::new(r.right() + 2.0, y),
                    Paint::stroke(&col, 3.0),
                ));
            } else {
                items.push(Element::rect(r, 2.0, Paint::both(&col, &axis_c, 0.5)));
            }
            items.push(Element::text(text_at(
                lp.x + ox,
                lp.y + top,
                std::slice::from_ref(name),
                xc.legend_font,
                Anchor::Start,
                &text_c,
            )));
        }
        els.push(Element::group("legend", items));
        top += lg.size.1 + 10.0;
    }

    // --- Axes geometry.
    let lf = xc.label_font;
    let tf = xc.axis_title_font;
    let tick = xc.tick_length;
    let (cat_title, val_title) = (&chart.x_title, &chart.y_title);
    // Title of whichever axis sits on the left goes above the plot (no rotated text).
    let left_title = if horizontal { cat_title } else { val_title };
    let bottom_title = if horizontal { val_title } else { cat_title };
    if left_title.as_ref().is_some_and(|t| !t.is_empty()) {
        top += tf * 1.5 + 4.0;
    }
    let plot_top = top + lf * 0.6;
    let mut bottom_band = tick + 5.0 + lf * 1.3 + 10.0;
    if bottom_title.as_ref().is_some_and(|t| !t.is_empty()) {
        bottom_band += tf * 1.5 + 5.0;
    }

    // Value scale (tick density depends on axis length; estimate it first).
    let est_len = if horizontal {
        w * 0.7
    } else {
        h - plot_top - bottom_band
    };
    let target = ((est_len / if horizontal { 80.0 } else { 50.0 }).round() as usize).clamp(2, 10);
    let vs = ValueScale::resolve(chart, target);
    let tick_labels: Vec<String> = vs.ticks.iter().map(|v| fmt_num(*v)).collect();

    // Category / numeric x labels.
    let x_ticks: Vec<(f64, String)> = match chart.x_axis {
        XAxis::Range(a, b) => {
            let step = nice_step((b - a).abs(), 8);
            ticks_in(a.min(b), a.max(b), step)
                .into_iter()
                .map(|v| (v, fmt_num(v)))
                .collect()
        }
        _ => Vec::new(),
    };

    let left_band;
    let plot_left;
    let plot_right = w - 20.0;
    let mut cat_lines: Vec<Vec<String>> = Vec::new();
    if horizontal {
        let max_cat_w = categories
            .iter()
            .chain(x_ticks.iter().map(|t| &t.1))
            .map(|c| line_width(c, lf, false))
            .fold(0.0, f64::max)
            .min(w * 0.3);
        left_band = 10.0 + max_cat_w + 5.0 + tick;
        plot_left = left_band;
        for c in &categories {
            cat_lines.push(wrap_words(c, lf, w * 0.3));
        }
    } else {
        let max_tick_w = tick_labels
            .iter()
            .map(|c| line_width(c, lf, false))
            .fold(0.0, f64::max);
        left_band = 10.0 + max_tick_w + 5.0 + tick;
        plot_left = left_band;
        let band = (plot_right - plot_left) / n_slots as f64;
        for c in &categories {
            cat_lines.push(wrap_words(c, lf, (band - 4.0).max(20.0)));
        }
        let extra = cat_lines.iter().map(Vec::len).max().unwrap_or(1).max(1) - 1;
        bottom_band += extra as f64 * lf * 1.2;
    }
    let plot_bottom = h - bottom_band;
    let plot = Rect::new(
        plot_left,
        plot_top,
        (plot_right - plot_left).max(10.0),
        (plot_bottom - plot_top).max(10.0),
    );

    // Category axis: position along its own direction.
    let cat_len = if horizontal { plot.h } else { plot.w };
    let band = cat_len / n_slots as f64;
    let cat_start = if horizontal { plot.y } else { plot.x };
    let slot_pos = |i: usize| -> f64 {
        match chart.x_axis {
            XAxis::Range(a, b) if n_slots > 1 => {
                let v = a + (b - a) * i as f64 / (n_slots - 1) as f64;
                range_pos(v, a, b, cat_start, cat_len, band)
            }
            _ => cat_start + band * (i as f64 + 0.5),
        }
    };
    // Value axis: v → coordinate.
    let val_pos = |v: f64| -> f64 {
        let f = vs.frac(v);
        if horizontal {
            plot.x + f * plot.w
        } else {
            plot.bottom() - f * plot.h
        }
    };

    // --- Axes drawing.
    let mut axis_els = Vec::new();
    let line_paint = Paint::stroke(&axis_c, 2.0);
    let tick_paint = Paint::stroke(&axis_c, 2.0);
    // Light grid lines along value ticks for readability.
    let grid_paint = Paint::stroke(&axis_c, 0.5).with_opacity(0.15);
    for (v, label) in vs.ticks.iter().zip(&tick_labels) {
        let p = val_pos(*v);
        if horizontal {
            axis_els.push(Element::line(
                Point::new(p, plot.y),
                Point::new(p, plot.bottom()),
                grid_paint.clone(),
            ));
            axis_els.push(Element::line(
                Point::new(p, plot.bottom()),
                Point::new(p, plot.bottom() + tick),
                tick_paint.clone(),
            ));
            let t = text_at(
                p,
                plot.bottom() + tick + 5.0 + lf * 0.85,
                std::slice::from_ref(label),
                lf,
                Anchor::Middle,
                &text_c,
            );
            bounds.add_rect(text_bounds(&t));
            axis_els.push(Element::text(t));
        } else {
            axis_els.push(Element::line(
                Point::new(plot.x, p),
                Point::new(plot.right(), p),
                grid_paint.clone(),
            ));
            axis_els.push(Element::line(
                Point::new(plot.x - tick, p),
                Point::new(plot.x, p),
                tick_paint.clone(),
            ));
            let t = text_at(
                plot.x - tick - 5.0,
                p + lf * 0.35,
                std::slice::from_ref(label),
                lf,
                Anchor::End,
                &text_c,
            );
            bounds.add_rect(text_bounds(&t));
            axis_els.push(Element::text(t));
        }
    }
    // Category / x ticks.
    let cat_ticks: Vec<(f64, Vec<String>)> = if x_ticks.is_empty() {
        cat_lines
            .iter()
            .enumerate()
            .map(|(i, l)| (slot_pos(i), l.clone()))
            .collect()
    } else if let XAxis::Range(a, b) = chart.x_axis {
        x_ticks
            .iter()
            .map(|(v, l)| {
                (
                    range_pos(*v, a, b, cat_start, cat_len, band),
                    vec![l.clone()],
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    for (p, lines) in &cat_ticks {
        if horizontal {
            axis_els.push(Element::line(
                Point::new(plot.x - tick, *p),
                Point::new(plot.x, *p),
                tick_paint.clone(),
            ));
            let lh = lf * 1.2;
            let first = *p - lh * (lines.len() as f64 - 1.0) / 2.0 + lf * 0.35;
            let t = text_at(plot.x - tick - 5.0, first, lines, lf, Anchor::End, &text_c);
            bounds.add_rect(text_bounds(&t));
            axis_els.push(Element::text(t));
        } else {
            axis_els.push(Element::line(
                Point::new(*p, plot.bottom()),
                Point::new(*p, plot.bottom() + tick),
                tick_paint.clone(),
            ));
            let t = text_at(
                *p,
                plot.bottom() + tick + 5.0 + lf * 0.85,
                lines,
                lf,
                Anchor::Middle,
                &text_c,
            );
            bounds.add_rect(text_bounds(&t));
            axis_els.push(Element::text(t));
        }
    }
    // Axis lines (left and bottom).
    axis_els.push(Element::line(
        Point::new(plot.x, plot.y),
        Point::new(plot.x, plot.bottom()),
        line_paint.clone(),
    ));
    axis_els.push(Element::line(
        Point::new(plot.x, plot.bottom()),
        Point::new(plot.right(), plot.bottom()),
        line_paint,
    ));
    // Axis titles.
    if let Some(t) = left_title.as_ref().filter(|t| !t.is_empty()) {
        let tt = text_at(
            plot.x - left_band + 10.0,
            plot.y - lf * 0.6 - 6.0,
            std::slice::from_ref(t),
            tf,
            Anchor::Start,
            &text_c,
        );
        bounds.add_rect(text_bounds(&tt));
        axis_els.push(Element::classed_text(tt, "axis-title"));
    }
    if let Some(t) = bottom_title.as_ref().filter(|t| !t.is_empty()) {
        let extra = if horizontal {
            0.0
        } else {
            (cat_lines.iter().map(Vec::len).max().unwrap_or(1).max(1) - 1) as f64 * lf * 1.2
        };
        let tt = text_at(
            plot.x + plot.w / 2.0,
            plot.bottom() + tick + 5.0 + lf * 1.3 + extra + 5.0 + tf,
            std::slice::from_ref(t),
            tf,
            Anchor::Middle,
            &text_c,
        );
        bounds.add_rect(text_bounds(&tt));
        axis_els.push(Element::classed_text(tt, "axis-title"));
    }

    // --- Plots.
    let mut plot_els = Vec::new();
    let mut label_els = Vec::new();
    let bars: Vec<usize> = chart
        .series
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SeriesKind::Bar)
        .map(|(i, _)| i)
        .collect();
    let group_w = band * 0.8;
    let bar_w = group_w / bars.len().max(1) as f64;
    let base = val_pos(0.0f64.clamp(vs.lo, vs.hi));
    for (bk, &si) in bars.iter().enumerate() {
        let s = &chart.series[si];
        let col = color_of(si);
        for (i, v) in s.values.iter().enumerate().take(n_slots) {
            let c = slot_pos(i);
            let off = c - group_w / 2.0 + bar_w * bk as f64;
            let vp = val_pos(v.clamp(vs.lo, vs.hi));
            let r = if horizontal {
                Rect::new(base.min(vp), off, (vp - base).abs(), bar_w)
            } else {
                Rect::new(off, base.min(vp), bar_w, (vp - base).abs())
            };
            plot_els.push(Element::rect(
                r,
                0.0,
                Paint::both(&col, &outline_of(&col), 1.0),
            ));
            if xc.show_data_label {
                let label = fmt_num(*v);
                let fs = (bar_w * 0.5).clamp(8.0, 14.0);
                let lw = line_width(&label, fs, false);
                let t = if horizontal {
                    let inside = !xc.data_label_outside && r.w > lw + 8.0;
                    let (x, anchor) = if inside {
                        (r.right() - 4.0, Anchor::End)
                    } else {
                        (r.right() + 4.0, Anchor::Start)
                    };
                    text_at(
                        x,
                        r.y + r.h / 2.0 + fs * 0.35,
                        &[label],
                        fs,
                        anchor,
                        &text_c,
                    )
                } else {
                    let inside = !xc.data_label_outside && r.h > fs + 8.0;
                    let y = if inside { r.y + fs + 2.0 } else { r.y - 4.0 };
                    text_at(r.x + r.w / 2.0, y, &[label], fs, Anchor::Middle, &text_c)
                };
                bounds.add_rect(text_bounds(&t));
                label_els.push(Element::text(t));
            }
        }
    }
    for (si, s) in chart.series.iter().enumerate() {
        if s.kind != SeriesKind::Line {
            continue;
        }
        let col = line_color(&color_of(si), pal.dark);
        let pts: Vec<Point> = s
            .values
            .iter()
            .enumerate()
            .take(n_slots)
            .map(|(i, v)| {
                let (a, b) = (slot_pos(i), val_pos(v.clamp(vs.lo, vs.hi)));
                if horizontal {
                    Point::new(b, a)
                } else {
                    Point::new(a, b)
                }
            })
            .collect();
        if pts.len() == 1 {
            plot_els.push(Element::shape(
                crate::scene::Geometry::Circle { c: pts[0], r: 3.0 },
                Paint::fill(&col),
            ));
        } else {
            plot_els.push(Element::path(
                geom::polyline(&pts),
                Paint::stroke(&col, 2.0),
            ));
        }
        for (i, p) in pts.iter().enumerate() {
            let label = match s.labels.get(i).cloned().flatten() {
                Some(l) => Some(l),
                None if xc.show_data_label => Some(fmt_num(s.values[i])),
                None => None,
            };
            if let Some(l) = label {
                plot_els.push(Element::shape(
                    crate::scene::Geometry::Circle { c: *p, r: 3.0 },
                    Paint::fill(&col),
                ));
                let t = text_at(p.x, p.y - 8.0, &[l], 12.0, Anchor::Middle, &text_c);
                bounds.add_rect(text_bounds(&t));
                label_els.push(Element::text(t));
            }
        }
    }

    els.push(Element::group("plot", plot_els));
    els.push(Element::group("axes", axis_els));
    els.push(Element::group("data-labels", label_els));
    Scene {
        kind: DiagramKind::XyChart,
        view_box: bounds.rect().pad(4.0),
        elements: vec![Element::group("main", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Lines are thin, so a near-background colour (e.g. `#ECECFF` on white) is
/// pushed toward the foreground to stay visible.
fn line_color(c: &Color, dark: bool) -> Color {
    match crate::color::parse(c.css()) {
        Some(rgba) => {
            let (h, sat, l) = rgba.to_hsl();
            if !dark && l > 80.0 {
                Color::new(crate::color::Rgba::from_hsl(h, sat.min(45.0), 50.0, 1.0).to_hex())
            } else if dark && l < 25.0 {
                Color::new(rgba.lighten(45.0 - l).to_hex())
            } else {
                c.clone()
            }
        }
        None => c.clone(),
    }
}

/// A slightly darker outline so very light bar colours stay visible on white.
pub(crate) fn outline_of(c: &Color) -> Color {
    crate::color::parse(c.css())
        .map(|rgba| Color::new(rgba.lighten(-12.0).to_hex()))
        .unwrap_or_else(|| c.clone())
}

/// Position of numeric x value `v` on an axis spanning `[a, b]`, inset by half a band.
fn range_pos(v: f64, a: f64, b: f64, start: f64, len: f64, band: f64) -> f64 {
    let f = if (b - a).abs() < 1e-12 {
        0.5
    } else {
        (v - a) / (b - a)
    };
    start + band / 2.0 + f * (len - band)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(10.0, 10), 1.0);
        assert_eq!(nice_step(7000.0, 7), 1000.0);
        assert_eq!(nice_step(15.0, 10), 2.0);
        assert_eq!(nice_step(0.3, 10), 0.05);
        assert_eq!(nice_step(40.0, 10), 5.0);
        for (span, n) in [(123.4, 6), (0.07, 5), (98765.0, 9), (3.0, 3)] {
            let s = nice_step(span, n);
            let mant = s / 10f64.powf(s.log10().floor());
            assert!(
                [1.0, 2.0, 5.0].iter().any(|m| (mant - m).abs() < 1e-9),
                "{s} is not 1/2/5 × 10^n"
            );
            assert!(span / s <= n as f64 + 1e-9);
        }
    }

    #[test]
    fn nice_range_covers_data() {
        let (lo, hi, step) = nice_range(-3.4, 45.0, 10);
        assert!(lo <= -3.4 && hi >= 45.0);
        assert_eq!(step, 5.0);
        assert_eq!((lo, hi), (-5.0, 45.0));
        assert_eq!(nice_range(0.0, 540.0, 6), (0.0, 600.0, 100.0));
        assert_eq!(nice_range(0.0, 500.0, 5), (0.0, 500.0, 100.0));
        let t = ticks_in(0.0, 0.3, 0.1);
        assert_eq!(t, vec![0.0, 0.1, 0.2, 0.3]);
        assert_eq!(fmt_num(0.1 + 0.2), "0.3");
        assert_eq!(fmt_num(4000.0), "4000");
        assert_eq!(fmt_num(-0.34), "-0.34");
    }

    #[test]
    fn parses_full_chart() {
        let src = "xychart-beta horizontal\n  title \"Sales Revenue\"\n  x-axis \"Month of year\" [jan, \"feb x\", mar]\n  y-axis \"Revenue (in $)\" 4000 --> 11000\n  bar [5000, 6000, 7500]\n  line \"trend\" [5000, 6000, 7500]\n";
        let c = parse(src, None).unwrap();
        assert_eq!(c.orientation, Orientation::Horizontal);
        assert_eq!(c.title.as_deref(), Some("Sales Revenue"));
        assert_eq!(c.x_title.as_deref(), Some("Month of year"));
        assert_eq!(
            c.x_axis,
            XAxis::Categories(vec!["jan".into(), "feb x".into(), "mar".into()])
        );
        assert_eq!(c.y_title.as_deref(), Some("Revenue (in $)"));
        assert_eq!(c.y_range, Some((4000.0, 11000.0)));
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[1].name.as_deref(), Some("trend"));
    }

    #[test]
    fn parses_numeric_axis_and_labels() {
        let src =
            "xychart\nx-axis score 1 --> 10\ny-axis Count\nline [+1.3, .6, 2.4 \"peak\", -.34]\n";
        let c = parse(src, None).unwrap();
        assert_eq!(c.x_axis, XAxis::Range(1.0, 10.0));
        assert_eq!(c.x_title.as_deref(), Some("score"));
        assert_eq!(c.y_title.as_deref(), Some("Count"));
        assert_eq!(c.series[0].values, vec![1.3, 0.6, 2.4, -0.34]);
        assert_eq!(c.series[0].labels[2].as_deref(), Some("peak"));
    }

    #[test]
    fn multiline_lists() {
        let c = parse("xychart-beta\nbar [1,\n 2,\n 3]\nline [3, 2, 1]", None).unwrap();
        assert_eq!(c.series[0].values, vec![1.0, 2.0, 3.0]);
        assert_eq!(c.series.len(), 2);
    }

    #[test]
    fn parse_errors_have_line_numbers() {
        let e = parse("xychart-beta\n\nbar [1, x]", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("xychart-beta\nfoo", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("xychart-beta\ny-axis t [a]", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        assert!(parse("pie\n", None).is_err());
    }

    #[test]
    fn auto_scale_includes_zero_for_bars() {
        let c = parse("xychart-beta\nbar [3, 7, 12]", None).unwrap();
        let vs = ValueScale::resolve(&c, 5);
        assert_eq!(vs.lo, 0.0);
        assert!(vs.hi >= 12.0);
        assert!(vs.ticks.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(
            "xychart-beta\ntitle T\nx-axis [a, b]\nbar \"s\" [1, 2]\nline [2, 1]",
            None,
            &cfg,
            &pal,
        )
        .unwrap();
        assert_eq!(s.kind, DiagramKind::XyChart);
        assert!(s.view_box.w >= 700.0 && s.view_box.h >= 500.0);
    }
}
