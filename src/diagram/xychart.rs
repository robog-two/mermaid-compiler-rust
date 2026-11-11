//! `xychart-beta` / `xychart` diagrams: bar and line series over a categorical
//! or numeric x-axis, vertical or horizontal.
//!
//! The layout is a port of mermaid's `chartBuilder` (fixed `width × height`
//! canvas, value axis from d3 ticks, legend right of the plot).
//!
//! Also hosts small chart helpers shared by the other chart modules
//! (quadrant, radar, sankey): number formatting,
//! quote-aware list splitting and a Taffy-based legend layout.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Point, Rect};
use crate::json::Json;
use crate::layout::boxes::{self, BoxNode};
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text::{line_width, normalize_label};
use crate::theme::{Palette, ThemeName};

// ---------------------------------------------------------------------------
// Shared chart helpers
// ---------------------------------------------------------------------------

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
// Renderer: a port of mermaid's `xychart/chartBuilder` (orchestrator, axes,
// plots, title, legend) and `xychartRenderer` (data labels).
// ---------------------------------------------------------------------------

/// One axis' settings (`xyChart.xAxis` / `xyChart.yAxis`).
#[derive(Debug, Clone, PartialEq)]
pub struct AxisConfig {
    pub show_label: bool,
    pub label_font: f64,
    pub label_padding: f64,
    pub show_title: bool,
    pub title_font: f64,
    pub title_padding: f64,
    pub show_tick: bool,
    pub tick_length: f64,
    pub tick_width: f64,
    pub show_axis_line: bool,
    pub axis_line_width: f64,
    /// `labelRotation` in degrees; 0 when outside -90..=90 (mermaid's
    /// `normalizedLabelRotationInRad`). Only an axis drawn at the bottom uses it.
    pub label_rotation: f64,
}

impl Default for AxisConfig {
    fn default() -> Self {
        AxisConfig {
            show_label: true,
            label_font: 14.0,
            label_padding: 5.0,
            show_title: true,
            title_font: 16.0,
            title_padding: 5.0,
            show_tick: true,
            tick_length: 5.0,
            tick_width: 2.0,
            show_axis_line: true,
            axis_line_width: 2.0,
            label_rotation: 0.0,
        }
    }
}

impl AxisConfig {
    fn from_config(cfg: &MermaidConfig, axis: &str) -> Self {
        let d = AxisConfig::default();
        let get = |k: &str| cfg.section_nested("xyChart", axis, k);
        let f = |k: &str, v: f64| get(k).and_then(Json::as_f64).unwrap_or(v);
        let b = |k: &str, v: bool| get(k).and_then(Json::as_bool).unwrap_or(v);
        let rot = f("labelRotation", 0.0);
        AxisConfig {
            show_label: b("showLabel", d.show_label),
            label_font: f("labelFontSize", d.label_font),
            label_padding: f("labelPadding", d.label_padding),
            show_title: b("showTitle", d.show_title),
            title_font: f("titleFontSize", d.title_font),
            title_padding: f("titlePadding", d.title_padding),
            show_tick: b("showTick", d.show_tick),
            tick_length: f("tickLength", d.tick_length),
            tick_width: f("tickWidth", d.tick_width),
            show_axis_line: b("showAxisLine", d.show_axis_line),
            axis_line_width: f("axisLineWidth", d.axis_line_width),
            label_rotation: if (-90.0..=90.0).contains(&rot) {
                rot
            } else {
                0.0
            },
        }
    }
}

/// Settings read from the `xyChart` config section (defaults from mermaid's schema).
#[derive(Debug, Clone, PartialEq)]
pub struct XyConfig {
    pub width: f64,
    pub height: f64,
    pub title_font: f64,
    pub title_padding: f64,
    pub show_title: bool,
    pub show_legend: bool,
    pub legend_font: f64,
    pub legend_padding: f64,
    pub show_data_label: bool,
    pub data_label_outside: bool,
    pub horizontal_cfg: bool,
    pub plot_reserved_percent: f64,
    pub x_axis: AxisConfig,
    pub y_axis: AxisConfig,
}

impl Default for XyConfig {
    fn default() -> Self {
        XyConfig {
            width: 700.0,
            height: 500.0,
            title_font: 20.0,
            title_padding: 10.0,
            show_title: true,
            show_legend: true,
            legend_font: 14.0,
            legend_padding: 10.0,
            show_data_label: false,
            data_label_outside: false,
            horizontal_cfg: false,
            plot_reserved_percent: 50.0,
            x_axis: AxisConfig::default(),
            y_axis: AxisConfig::default(),
        }
    }
}

impl XyConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let s = "xyChart";
        let d = XyConfig::default();
        XyConfig {
            width: cfg.section_f64(s, "width", d.width),
            height: cfg.section_f64(s, "height", d.height),
            title_font: cfg.section_f64(s, "titleFontSize", d.title_font),
            title_padding: cfg.section_f64(s, "titlePadding", d.title_padding),
            show_title: cfg.section_bool(s, "showTitle", d.show_title),
            show_legend: cfg.section_bool(s, "showLegend", d.show_legend),
            legend_font: cfg.section_f64(s, "legendFontSize", d.legend_font),
            legend_padding: cfg.section_f64(s, "legendPadding", d.legend_padding),
            show_data_label: cfg.section_bool(s, "showDataLabel", d.show_data_label),
            data_label_outside: cfg.section_bool(s, "showDataLabelOutsideBar", false),
            horizontal_cfg: cfg.section(s, "chartOrientation").and_then(Json::as_str)
                == Some("horizontal"),
            plot_reserved_percent: cfg.section_f64(
                s,
                "plotReservedSpacePercent",
                d.plot_reserved_percent,
            ),
            x_axis: AxisConfig::from_config(cfg, "xAxis"),
            y_axis: AxisConfig::from_config(cfg, "yAxis"),
        }
    }
}

/// Colours of one axis.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisColors {
    pub title: Color,
    pub label: Color,
    pub tick: Color,
    pub line: Color,
}

/// mermaid's `themeVariables.xyChart`.
#[derive(Debug, Clone, PartialEq)]
pub struct XyTheme {
    pub background: Color,
    pub title: Color,
    pub data_label: Color,
    pub legend_text: Color,
    pub x_axis: AxisColors,
    pub y_axis: AxisColors,
    pub plot_colors: Vec<Color>,
}

const PALETTE_DEFAULT: &str =
    "#ECECFF,#8493A6,#FFC3A0,#DCDDE1,#B8E994,#D1A36F,#C3CDE6,#FFB6C1,#496078,#F8F3E3";
const PALETTE_DARK: &str =
    "#3498db,#2ecc71,#e74c3c,#f1c40f,#bdc3c7,#ffffff,#34495e,#9b59b6,#1abc9c,#e67e22";
const PALETTE_FOREST: &str =
    "#CDE498,#FF6B6B,#A0D2DB,#D7BDE2,#F0F0F0,#FFC3A0,#7FD8BE,#FF9A8B,#FAF3E0,#FFF176";
const PALETTE_NEUTRAL: &str =
    "#EEE,#6BB8E4,#8ACB88,#C7ACD6,#E8DCC2,#FFB2A8,#FFF380,#7E8D91,#FFD8B1,#FAF3E0";
const PALETTE_BASE: &str =
    "#FFF4DD,#FFD8B1,#FFA07A,#ECEFF1,#D6DBDF,#C3E0A8,#FFB6A4,#FFD74D,#738FA7,#FFFFF0";

fn split_palette(s: &str) -> Vec<Color> {
    s.split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(Color::new)
        .collect()
}

impl XyTheme {
    /// Per-theme `xyChart` defaults (`background`, `primaryTextColor` and the
    /// theme's `plotColorPalette`, as mermaid 12.3 resolves them), then the flat
    /// `background` / `primaryTextColor` variables, then `themeVariables.xyChart.*`.
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        use ThemeName as T;
        let (bg, text, palette) = match cfg.theme {
            T::Dark => ("#333", "#e0dfdf", PALETTE_DARK),
            T::Forest => ("white", "#321b67", PALETTE_FOREST),
            T::Neutral => ("#ffffff", "#111111", PALETTE_NEUTRAL),
            T::Base => ("#f4f4f4", "#333", PALETTE_BASE),
            T::Neo => ("#ffffff", "#333", PALETTE_BASE),
            T::Redux | T::ReduxColor => ("#ffffff", "#28253D", PALETTE_BASE),
            T::NeoDark | T::ReduxDark | T::ReduxDarkColor => ("#333", "#e0dfdf", PALETTE_BASE),
            T::Default | T::Null => ("white", "#131300", PALETTE_DEFAULT),
        };
        let flat = |k: &str| {
            cfg.theme_variables
                .iter()
                .rev()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.trim().to_string())
        };
        let bg = flat("background").unwrap_or_else(|| bg.to_string());
        let text = flat("primaryTextColor").unwrap_or_else(|| text.to_string());
        let var = |k: &str, d: &str| {
            cfg.section_nested("themeVariables", "xyChart", k)
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map_or_else(|| Color::new(d), Color::new)
        };
        let axis = |p: &str| AxisColors {
            title: var(&format!("{p}AxisTitleColor"), &text),
            label: var(&format!("{p}AxisLabelColor"), &text),
            tick: var(&format!("{p}AxisTickColor"), &text),
            line: var(&format!("{p}AxisLineColor"), &text),
        };
        let mut plot_colors = split_palette(var("plotColorPalette", palette).css());
        if plot_colors.is_empty() {
            plot_colors = split_palette(palette);
        }
        XyTheme {
            background: var("backgroundColor", &bg),
            title: var("titleColor", &text),
            data_label: var("dataLabelColor", &text),
            legend_text: var("legendTextColor", &text),
            x_axis: axis("x"),
            y_axis: axis("y"),
            plot_colors,
        }
    }
}

// --- Chart data (mermaid's `xychartDb`) ------------------------------------

/// A data point's x value: a category of a band axis or a number of a linear one.
#[derive(Debug, Clone, PartialEq)]
pub enum XKey {
    Cat(String),
    Num(f64),
}

/// The data of an axis after `xychartDb` has resolved it.
#[derive(Debug, Clone, PartialEq)]
pub enum AxisData {
    Band(Vec<String>),
    Linear(f64, f64),
}

/// One plot after `xychartDb` has resolved it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotData {
    pub kind: SeriesKind,
    /// `""` when the plot has no title (no legend entry).
    pub title: String,
    pub color: Color,
    /// `(x, y)`; `y` is `None` where a band axis has more categories than values.
    pub data: Vec<(XKey, Option<f64>)>,
    pub point_labels: Option<Vec<String>>,
}

/// mermaid's `XYChartData`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub x: AxisData,
    pub x_title: String,
    pub y: (f64, f64),
    pub y_title: String,
    pub title: String,
    pub plots: Vec<PlotData>,
}

/// JavaScript's `Number.prototype.toString` for chart values.
pub(crate) fn js_num(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v.is_finite() && v == v.trunc() && v.abs() < 1e21 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// `xychartDb`: axis ranges from the data, band truncation, numeric x values
/// spread over a linear x-axis and palette colours by plot index.
pub fn chart_data(chart: &XyChart, palette: &[Color]) -> ChartData {
    let mut x = match &chart.x_axis {
        XAxis::Categories(c) => Some(AxisData::Band(c.clone())),
        XAxis::Range(a, b) => Some(AxisData::Linear(*a, *b)),
        XAxis::Auto => None,
    };
    let mut y = chart.y_range;
    let has_set_y = y.is_some();
    let mut y_data = (f64::INFINITY, f64::NEG_INFINITY);
    let mut plots = Vec::new();
    for (i, s) in chart.series.iter().enumerate() {
        let mut values: &[f64] = &s.values;
        let mut data = Vec::new();
        if !values.is_empty() {
            let xa = x.get_or_insert(AxisData::Linear(1.0, values.len() as f64));
            if let AxisData::Band(c) = xa {
                values = &values[..values.len().min(c.len())];
            }
            if !has_set_y {
                for v in values {
                    y_data = (y_data.0.min(*v), y_data.1.max(*v));
                }
            }
            data = match xa {
                AxisData::Band(c) => c
                    .iter()
                    .enumerate()
                    .map(|(k, c)| (XKey::Cat(c.clone()), values.get(k).copied()))
                    .collect(),
                AxisData::Linear(min, max) if values.len() == 1 => {
                    vec![(XKey::Num(*min), Some(values[0]))]
                }
                AxisData::Linear(min, max) => {
                    let step = (*max - *min) / (values.len() - 1) as f64;
                    values
                        .iter()
                        .enumerate()
                        .map(|(k, v)| (XKey::Num(*min + k as f64 * step), Some(*v)))
                        .collect()
                }
            };
        }
        let labels: Vec<String> = s
            .labels
            .iter()
            .map(|l| l.clone().unwrap_or_default())
            .collect();
        let point_labels =
            (s.kind == SeriesKind::Line && labels.iter().any(|l| !l.is_empty())).then_some(labels);
        plots.push(PlotData {
            kind: s.kind,
            title: s.name.clone().unwrap_or_default(),
            color: palette
                .get(i % palette.len().max(1))
                .cloned()
                .unwrap_or_else(|| Color::new("#ECECFF")),
            data,
            point_labels,
        });
    }
    if !has_set_y {
        y = Some(y_data);
    }
    ChartData {
        x: x.unwrap_or(AxisData::Band(Vec::new())),
        x_title: chart.x_title.clone().unwrap_or_default(),
        y: y.unwrap_or((0.0, 1.0)),
        y_title: chart.y_title.clone().unwrap_or_default(),
        title: chart.title.clone().unwrap_or_default(),
        plots,
    }
}

// --- Text measurement ------------------------------------------------------

/// Chromium's `getBoundingClientRect().height` of one `<tspan>` at integer font
/// sizes 6..=48 (mermaid's `computeDimensionOfText`), without and with glyphs
/// that reach below the baseline. Measured in the reference renderer.
const TSPAN_HEIGHTS: [(u8, u8); 43] = [
    (6, 7),
    (7, 8),
    (9, 9),
    (10, 10),
    (11, 12),
    (12, 13),
    (14, 14),
    (15, 15),
    (16, 16),
    (17, 18),
    (17, 18),
    (19, 19),
    (20, 20),
    (21, 21),
    (22, 23),
    (23, 24),
    (25, 25),
    (26, 26),
    (27, 27),
    (28, 29),
    (30, 30),
    (30, 30),
    (31, 31),
    (32, 33),
    (33, 34),
    (35, 35),
    (36, 36),
    (37, 37),
    (38, 39),
    (39, 40),
    (41, 41),
    (41, 41),
    (42, 42),
    (43, 44),
    (44, 45),
    (46, 46),
    (47, 47),
    (48, 48),
    (49, 50),
    (51, 51),
    (52, 52),
    (53, 53),
    (53, 53),
];

/// Height of one line of chart text as the browser measures it.
pub(crate) fn tspan_height(s: &str, font_size: f64) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let desc = s.chars().any(|c| "gjpqy(),;[]{}|_@Q$".contains(c));
    if font_size.fract() == 0.0 && (6.0..=48.0).contains(&font_size) {
        let (a, b) = TSPAN_HEIGHTS[font_size as usize - 6];
        return f64::from(if desc { b } else { a });
    }
    font_size * 1.115 + if desc { 0.6 } else { 0.0 }
}

/// mermaid's `getMaxDimension`: the widest and tallest of `texts`.
fn max_dimension<S: AsRef<str>>(texts: &[S], font_size: f64) -> (f64, f64) {
    texts.iter().fold((0.0, 0.0), |(w, h), t| {
        let t = t.as_ref();
        (
            f64::max(w, line_width(t, font_size, false)),
            f64::max(h, tspan_height(t, font_size)),
        )
    })
}

// --- Drawing helpers -------------------------------------------------------

/// Alphabetic-baseline offsets for SVG `dominant-baseline` values.
/// (`middle` sits half an x-height above the baseline, not at `central`.)
const BASELINE_MIDDLE: f64 = 0.27;
const BASELINE_BEFORE_EDGE: f64 = 0.9;
const BASELINE_HANGING: f64 = 0.72;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VPos {
    Top,
    Middle,
}

/// A mermaid `TextElem`: `translate(x, y) rotate(rotation)` with the given
/// `dominant-baseline` and `text-anchor`.
#[allow(clippy::too_many_arguments)]
fn text_elem(
    s: &str,
    x: f64,
    y: f64,
    fs: f64,
    fill: &Color,
    vpos: VPos,
    anchor: Anchor,
    rotation: f64,
) -> Element {
    let off = match vpos {
        VPos::Top => BASELINE_BEFORE_EDGE,
        VPos::Middle => BASELINE_MIDDLE,
    };
    let t = text_at(x, y + off * fs, &[s.to_string()], fs, anchor, fill);
    let deg = rotation.rem_euclid(360.0);
    let deg = if deg > 180.0 { deg - 360.0 } else { deg };
    if deg == 0.0 {
        Element::text(t)
    } else {
        Element::Rotated {
            deg,
            about: Point::new(x, y),
            children: vec![Element::text(t)],
        }
    }
}

fn path_line(a: Point, b: Point, c: &Color, w: f64) -> Element {
    Element::line(a, b, Paint::stroke(c, w))
}

/// Push a `<g class>` unless it would be empty (mermaid skips empty shapes).
fn push_group(out: &mut Vec<Element>, class: &str, children: Vec<Element>) {
    if !children.is_empty() {
        out.push(Element::group(class, children));
    }
}

// --- d3 scales -------------------------------------------------------------

/// d3-array's `tickSpec`.
fn tick_spec(start: f64, stop: f64, count: f64) -> (f64, f64, f64) {
    let js_round = |v: f64| (v + 0.5).floor();
    let step = (stop - start) / count.max(0.0);
    let power = step.log10().floor();
    let error = step / 10f64.powi(power as i32);
    let factor = if error >= 50f64.sqrt() {
        10.0
    } else if error >= 10f64.sqrt() {
        5.0
    } else if error >= 2f64.sqrt() {
        2.0
    } else {
        1.0
    };
    let (mut i1, mut i2, inc);
    if power < 0.0 {
        let p = 10f64.powi(-power as i32) / factor;
        i1 = js_round(start * p);
        i2 = js_round(stop * p);
        if i1 / p < start {
            i1 += 1.0;
        }
        if i2 / p > stop {
            i2 -= 1.0;
        }
        inc = -p;
    } else {
        inc = 10f64.powi(power as i32) * factor;
        i1 = js_round(start / inc);
        i2 = js_round(stop / inc);
        if i1 * inc < start {
            i1 += 1.0;
        }
        if i2 * inc > stop {
            i2 -= 1.0;
        }
    }
    if i2 < i1 && (0.5..2.0).contains(&count) {
        return tick_spec(start, stop, count * 2.0);
    }
    (i1, i2, inc)
}

/// d3-array's `ticks(start, stop, count)` (what `scaleLinear().ticks()` uses).
pub(crate) fn d3_ticks(start: f64, stop: f64, count: f64) -> Vec<f64> {
    if count.is_nan() || count <= 0.0 || !start.is_finite() || !stop.is_finite() {
        return Vec::new();
    }
    if start == stop {
        return vec![start];
    }
    let reverse = stop < start;
    let (i1, i2, inc) = if reverse {
        tick_spec(stop, start, count)
    } else {
        tick_spec(start, stop, count)
    };
    if i2.is_nan() || i1.is_nan() || i2 < i1 {
        return Vec::new();
    }
    let n = (i2 - i1 + 1.0) as usize;
    (0..n.min(10_000))
        .map(|i| {
            let k = if reverse {
                i2 - i as f64
            } else {
                i1 + i as f64
            };
            if inc < 0.0 { k / -inc } else { k * inc }
        })
        .collect()
}

// --- Axes ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisPos {
    Left,
    Bottom,
    Top,
}

#[derive(Debug, Clone, PartialEq)]
enum AxisKind {
    /// Categories as given (tick labels) and their de-duplicated domain.
    Band {
        categories: Vec<String>,
        domain: Vec<String>,
    },
    Linear {
        domain: (f64, f64),
    },
}

const BAR_WIDTH_TO_TICK_WIDTH_RATIO: f64 = 0.7;
const MAX_OUTER_PADDING_PERCENT_FOR_WRT_LABEL: f64 = 0.2;

/// mermaid's `BaseAxis` with its `BandAxis` / `LinearAxis` scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    cfg: AxisConfig,
    colors: AxisColors,
    title: String,
    kind: AxisKind,
    pub pos: AxisPos,
    range: (f64, f64),
    pub bbox: Rect,
    show_title: bool,
    show_label: bool,
    show_tick: bool,
    show_axis_line: bool,
    pub outer_padding: f64,
    title_text_height: f64,
    rot: f64,
}

impl Axis {
    pub fn new(data: &AxisData, title: &str, cfg: &AxisConfig, colors: &AxisColors) -> Axis {
        let kind = match data {
            AxisData::Band(c) => {
                let mut domain: Vec<String> = Vec::new();
                for x in c {
                    if !domain.contains(x) {
                        domain.push(x.clone());
                    }
                }
                AxisKind::Band {
                    categories: c.clone(),
                    domain,
                }
            }
            AxisData::Linear(a, b) => AxisKind::Linear { domain: (*a, *b) },
        };
        Axis {
            cfg: cfg.clone(),
            colors: colors.clone(),
            title: title.to_string(),
            kind,
            pos: AxisPos::Left,
            range: (0.0, 10.0),
            bbox: Rect::new(0.0, 0.0, 0.0, 0.0),
            show_title: false,
            show_label: false,
            show_tick: false,
            show_axis_line: false,
            outer_padding: 0.0,
            title_text_height: 0.0,
            rot: cfg.label_rotation.to_radians(),
        }
    }

    fn vertical(&self) -> bool {
        self.pos == AxisPos::Left
    }

    pub fn set_range(&mut self, a: f64, b: f64) {
        self.range = (a, b);
        if self.vertical() {
            self.bbox.h = b - a;
        } else {
            self.bbox.w = b - a;
        }
    }

    /// The range inset by the outer padding.
    pub fn get_range(&self) -> (f64, f64) {
        (
            self.range.0 + self.outer_padding,
            self.range.1 - self.outer_padding,
        )
    }

    pub fn set_position(&mut self, pos: AxisPos) {
        self.pos = pos;
        self.set_range(self.range.0, self.range.1);
    }

    fn linear_domain(&self) -> Option<(f64, f64)> {
        match self.kind {
            AxisKind::Linear { domain: (a, b) } if self.pos == AxisPos::Left => Some((b, a)),
            AxisKind::Linear { domain } => Some(domain),
            AxisKind::Band { .. } => None,
        }
    }

    /// Tick values as `(position, label)`.
    pub fn ticks(&self) -> Vec<(f64, String)> {
        match &self.kind {
            AxisKind::Band { categories, .. } => categories
                .iter()
                .map(|c| (self.scale_cat(c), c.clone()))
                .collect(),
            AxisKind::Linear { .. } => {
                let (a, b) = self.linear_domain().unwrap_or((0.0, 1.0));
                d3_ticks(a, b, 10.0)
                    .into_iter()
                    .map(|v| (self.scale_num(v), js_num(v)))
                    .collect()
            }
        }
    }

    fn tick_count(&self) -> usize {
        match &self.kind {
            AxisKind::Band { categories, .. } => categories.len(),
            AxisKind::Linear { .. } => {
                let (a, b) = self.linear_domain().unwrap_or((0.0, 1.0));
                d3_ticks(a, b, 10.0).len()
            }
        }
    }

    fn tick_labels(&self) -> Vec<String> {
        self.ticks().into_iter().map(|t| t.1).collect()
    }

    /// d3 `scaleBand().paddingInner(1).paddingOuter(0).align(0.5)`.
    fn scale_cat(&self, c: &str) -> f64 {
        let (r0, r1) = self.get_range();
        let AxisKind::Band { domain, .. } = &self.kind else {
            return c.parse().map_or(r0, |v| self.scale_num(v));
        };
        let Some(i) = domain.iter().position(|d| d == c) else {
            return r0;
        };
        let n = domain.len() as f64;
        let step = (r1 - r0) / f64::max(1.0, n - 1.0);
        let start = r0 + (r1 - r0 - step * (n - 1.0)) * 0.5;
        start + step * i as f64
    }

    /// d3 `scaleLinear()` (domain reversed for a left axis).
    fn scale_num(&self, v: f64) -> f64 {
        let (r0, r1) = self.get_range();
        let Some((d0, d1)) = self.linear_domain() else {
            return self.scale_cat(&js_num(v));
        };
        let span = d1 - d0;
        let t = if span != 0.0 && span.is_finite() {
            (v - d0) / span
        } else {
            0.5
        };
        r0 + t * (r1 - r0)
    }

    pub fn scale(&self, x: &XKey) -> f64 {
        match x {
            XKey::Cat(c) => self.scale_cat(c),
            XKey::Num(v) => self.scale_num(*v),
        }
    }

    pub fn tick_distance(&self) -> f64 {
        let (a, b) = self.get_range();
        (a - b).abs() / self.tick_count() as f64
    }

    fn label_dimension(&self) -> (f64, f64) {
        max_dimension(&self.tick_labels(), self.cfg.label_font)
    }

    pub fn recalculate_outer_padding_to_draw_bar(&mut self) {
        let d = BAR_WIDTH_TO_TICK_WIDTH_RATIO * self.tick_distance();
        if d > self.outer_padding * 2.0 {
            self.outer_padding = (d / 2.0).floor();
        }
    }

    fn space_horizontal(&mut self, aw: f64, ah: f64) {
        let c = self.cfg.clone();
        let mut avail = ah;
        if c.show_axis_line && avail > c.axis_line_width {
            avail -= c.axis_line_width;
            self.show_axis_line = true;
        }
        if c.show_label {
            let (lw, lh) = self.label_dimension();
            self.outer_padding = f64::min(lw / 2.0, MAX_OUTER_PADDING_PERCENT_FOR_WRT_LABEL * aw);
            let mut need = lh;
            if self.pos == AxisPos::Bottom && self.rot != 0.0 {
                need = need.max((self.rot.sin() * lw).abs() + (self.rot.cos() * lh).abs());
            }
            need += c.label_padding * 2.0;
            if need <= avail {
                avail -= need;
                self.show_label = true;
            }
        }
        if c.show_tick && avail >= c.tick_length {
            self.show_tick = true;
            avail -= c.tick_length;
        }
        if c.show_title && !self.title.is_empty() {
            let (_, th) = max_dimension(&[&self.title], c.title_font);
            let need = th + c.title_padding * 2.0;
            self.title_text_height = th;
            if need <= avail {
                avail -= need;
                self.show_title = true;
            }
        }
        self.bbox.w = aw;
        self.bbox.h = ah - avail;
    }

    fn space_vertical(&mut self, aw: f64, ah: f64) {
        let c = self.cfg.clone();
        let mut avail = aw;
        if c.show_axis_line && avail > c.axis_line_width {
            avail -= c.axis_line_width;
            self.show_axis_line = true;
        }
        if c.show_label {
            let (lw, lh) = self.label_dimension();
            self.outer_padding = f64::min(lh / 2.0, MAX_OUTER_PADDING_PERCENT_FOR_WRT_LABEL * ah);
            let need = lw + c.label_padding * 2.0;
            if need <= avail {
                avail -= need;
                self.show_label = true;
            }
        }
        if c.show_tick && avail >= c.tick_length {
            self.show_tick = true;
            avail -= c.tick_length;
        }
        if c.show_title && !self.title.is_empty() {
            let (_, th) = max_dimension(&[&self.title], c.title_font);
            let need = th + c.title_padding * 2.0;
            self.title_text_height = th;
            if need <= avail {
                avail -= need;
                self.show_title = true;
            }
        }
        self.bbox.w = aw - avail;
        self.bbox.h = ah;
    }

    /// Space the axis needs out of `(aw, ah)`.
    pub fn calculate_space(&mut self, aw: f64, ah: f64) -> (f64, f64) {
        if self.vertical() {
            self.space_vertical(aw, ah);
        } else {
            self.space_horizontal(aw, ah);
        }
        (self.bbox.w, self.bbox.h)
    }

    pub fn set_xy(&mut self, x: f64, y: f64) {
        self.bbox.x = x;
        self.bbox.y = y;
    }

    fn offset_by_rotation(&self, use_height: bool) -> f64 {
        if self.rot == 0.0 {
            return 0.0;
        }
        let (w, h) = self.label_dimension();
        self.rot.sin() * if use_height { h } else { w } / 2.0
    }

    fn draw_left(&self) -> Vec<Element> {
        let (c, k, b) = (&self.cfg, &self.colors, self.bbox);
        let line_w = if self.show_axis_line {
            c.axis_line_width
        } else {
            0.0
        };
        let tick_l = if self.show_tick { c.tick_length } else { 0.0 };
        let mut out = Vec::new();
        if self.show_axis_line {
            let x = b.x + b.w - c.axis_line_width / 2.0;
            let l = path_line(
                Point::new(x, b.y),
                Point::new(x, b.y + b.h),
                &k.line,
                c.axis_line_width,
            );
            out.push(Element::group("axisl-line", vec![l]));
        }
        let ticks = self.ticks();
        if self.show_label {
            let x = b.x + b.w - c.label_padding - tick_l - line_w;
            let els = ticks
                .iter()
                .map(|(p, s)| {
                    text_elem(
                        s,
                        x,
                        *p,
                        c.label_font,
                        &k.label,
                        VPos::Middle,
                        Anchor::End,
                        0.0,
                    )
                })
                .collect();
            push_group(&mut out, "label", els);
        }
        if self.show_tick {
            let x = b.x + b.w - line_w;
            let els = ticks
                .iter()
                .map(|(p, _)| {
                    path_line(
                        Point::new(x, *p),
                        Point::new(x - c.tick_length, *p),
                        &k.tick,
                        c.tick_width,
                    )
                })
                .collect();
            push_group(&mut out, "ticks", els);
        }
        if self.show_title {
            let t = text_elem(
                &self.title,
                b.x + c.title_padding,
                b.y + b.h / 2.0,
                c.title_font,
                &k.title,
                VPos::Top,
                Anchor::Middle,
                270.0,
            );
            out.push(Element::group("title", vec![t]));
        }
        out
    }

    fn draw_bottom(&self) -> Vec<Element> {
        let (c, k, b) = (&self.cfg, &self.colors, self.bbox);
        let line_w = if self.show_axis_line {
            c.axis_line_width
        } else {
            0.0
        };
        let tick_l = if self.show_tick { c.tick_length } else { 0.0 };
        let mut out = Vec::new();
        if self.show_axis_line {
            let y = b.y + c.axis_line_width / 2.0;
            let l = path_line(
                Point::new(b.x, y),
                Point::new(b.x + b.w, y),
                &k.line,
                c.axis_line_width,
            );
            out.push(Element::group("axis-line", vec![l]));
        }
        let ticks = self.ticks();
        if self.show_label {
            let dx = self.offset_by_rotation(true);
            let y = b.y + c.label_padding + tick_l + line_w + self.offset_by_rotation(false).abs();
            let deg = self.rot.to_degrees();
            let els = ticks
                .iter()
                .map(|(p, s)| {
                    text_elem(
                        s,
                        p + dx,
                        y,
                        c.label_font,
                        &k.label,
                        VPos::Top,
                        Anchor::Middle,
                        deg,
                    )
                })
                .collect();
            push_group(&mut out, "label", els);
        }
        if self.show_tick {
            let y = b.y + line_w;
            let els = ticks
                .iter()
                .map(|(p, _)| {
                    path_line(
                        Point::new(*p, y),
                        Point::new(*p, y + c.tick_length),
                        &k.tick,
                        c.tick_width,
                    )
                })
                .collect();
            push_group(&mut out, "ticks", els);
        }
        if self.show_title {
            let t = text_elem(
                &self.title,
                self.range.0 + (self.range.1 - self.range.0) / 2.0,
                b.y + b.h - c.title_padding - self.title_text_height,
                c.title_font,
                &k.title,
                VPos::Top,
                Anchor::Middle,
                0.0,
            );
            out.push(Element::group("title", vec![t]));
        }
        out
    }

    fn draw_top(&self) -> Vec<Element> {
        let (c, k, b) = (&self.cfg, &self.colors, self.bbox);
        let line_w = if self.show_axis_line {
            c.axis_line_width
        } else {
            0.0
        };
        let mut out = Vec::new();
        if self.show_axis_line {
            let y = b.y + b.h - c.axis_line_width / 2.0;
            let l = path_line(
                Point::new(b.x, y),
                Point::new(b.x + b.w, y),
                &k.line,
                c.axis_line_width,
            );
            out.push(Element::group("axis-line", vec![l]));
        }
        let ticks = self.ticks();
        if self.show_label {
            let title_h = if self.show_title {
                self.title_text_height + c.title_padding * 2.0
            } else {
                0.0
            };
            let y = b.y + title_h + c.label_padding;
            let els = ticks
                .iter()
                .map(|(p, s)| {
                    text_elem(
                        s,
                        *p,
                        y,
                        c.label_font,
                        &k.label,
                        VPos::Top,
                        Anchor::Middle,
                        0.0,
                    )
                })
                .collect();
            push_group(&mut out, "label", els);
        }
        if self.show_tick {
            let y0 = b.y + b.h - line_w;
            let els = ticks
                .iter()
                .map(|(p, _)| {
                    path_line(
                        Point::new(*p, y0),
                        Point::new(*p, y0 - c.tick_length),
                        &k.tick,
                        c.tick_width,
                    )
                })
                .collect();
            push_group(&mut out, "ticks", els);
        }
        if self.show_title {
            let t = text_elem(
                &self.title,
                b.x + b.w / 2.0,
                b.y + c.title_padding,
                c.title_font,
                &k.title,
                VPos::Top,
                Anchor::Middle,
                0.0,
            );
            out.push(Element::group("title", vec![t]));
        }
        out
    }

    /// The axis' drawing, wrapped in its `left-axis` / `bottom-axis` / `top-axis` group.
    pub fn draw(&self) -> Element {
        let (class, els) = match self.pos {
            AxisPos::Left => ("left-axis", self.draw_left()),
            AxisPos::Bottom => ("bottom-axis", self.draw_bottom()),
            AxisPos::Top => ("top-axis", self.draw_top()),
        };
        Element::group(class, els)
    }
}

// --- Title and legend --------------------------------------------------------

/// mermaid's `ChartTitle`.
#[derive(Debug, Clone, PartialEq)]
struct ChartTitle {
    bbox: Rect,
    show: bool,
    width: f64,
}

impl ChartTitle {
    fn calculate_space(&mut self, data: &ChartData, c: &XyConfig, aw: f64, ah: f64) -> (f64, f64) {
        let (tw, th) = max_dimension(&[&data.title], c.title_font);
        let need_h = th + 2.0 * c.title_padding;
        if need_h <= ah && c.show_title && !data.title.is_empty() {
            self.bbox.w = tw.max(aw);
            self.bbox.h = need_h;
            self.width = tw;
            self.show = true;
        }
        (self.bbox.w, self.bbox.h)
    }

    /// Centre over `[start, end]`, kept inside the chart.
    fn align_to(&mut self, c: &XyConfig, start: f64, end: f64) {
        let half = self.width.min(c.width) / 2.0;
        let center = ((start + end) / 2.0).max(half).min(c.width - half);
        self.bbox.x = center - half;
        self.bbox.w = half * 2.0;
    }
}

const LEGEND_MARKER_TO_FONT_RATIO: f64 = 0.75;
const LEGEND_ITEM_SPACING_TO_FONT_RATIO: f64 = 0.5;
const LEGEND_MARKER_SPACING_TO_FONT_RATIO: f64 = 0.35;

/// mermaid's `ChartLegend`: one row per titled plot, right of the plot area.
#[derive(Debug, Clone, PartialEq)]
struct Legend {
    bbox: Rect,
    visible: Vec<usize>,
}

impl Legend {
    fn calculate_space(&mut self, data: &ChartData, c: &XyConfig, aw: f64, ah: f64) -> (f64, f64) {
        self.visible = if c.show_legend {
            (0..data.plots.len())
                .filter(|&i| !data.plots[i].title.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        self.bbox.w = 0.0;
        self.bbox.h = 0.0;
        if self.visible.is_empty() {
            return (0.0, 0.0);
        }
        let fs = c.legend_font;
        let titles: Vec<&str> = self
            .visible
            .iter()
            .map(|&i| data.plots[i].title.as_str())
            .collect();
        let (tw, _) = max_dimension(&titles, fs);
        let n = self.visible.len() as f64;
        let w = c.legend_padding * 2.0
            + fs * LEGEND_MARKER_TO_FONT_RATIO
            + fs * LEGEND_MARKER_SPACING_TO_FONT_RATIO
            + tw;
        let h =
            c.legend_padding * 2.0 + n * fs + (n - 1.0) * fs * LEGEND_ITEM_SPACING_TO_FONT_RATIO;
        if w <= aw && h <= ah {
            self.bbox.w = w;
            self.bbox.h = h;
        } else {
            self.visible.clear();
        }
        (self.bbox.w, self.bbox.h)
    }

    fn draw(&self, data: &ChartData, c: &XyConfig, theme: &XyTheme) -> Vec<Element> {
        if self.visible.is_empty() {
            return Vec::new();
        }
        let fs = c.legend_font;
        let marker = fs * LEGEND_MARKER_TO_FONT_RATIO;
        let spacing = fs * LEGEND_MARKER_SPACING_TO_FONT_RATIO;
        let row = fs + fs * LEGEND_ITEM_SPACING_TO_FONT_RATIO;
        let x0 = self.bbox.x + c.legend_padding;
        let y0 = self.bbox.y + c.legend_padding;
        let mut rects = Vec::new();
        let mut lines = Vec::new();
        let mut labels = Vec::new();
        for (k, &i) in self.visible.iter().enumerate() {
            let p = &data.plots[i];
            let y = y0 + k as f64 * row;
            match p.kind {
                SeriesKind::Bar => rects.push(Element::rect(
                    Rect::new(x0, y, marker, marker),
                    0.0,
                    Paint::fill(&p.color),
                )),
                SeriesKind::Line => lines.push(path_line(
                    Point::new(x0, y + marker / 2.0),
                    Point::new(x0 + marker, y + marker / 2.0),
                    &p.color,
                    LINE_WIDTH,
                )),
            }
            labels.push(text_elem(
                &p.title,
                x0 + marker + spacing,
                y + marker / 2.0,
                fs,
                &theme.legend_text,
                VPos::Middle,
                Anchor::Start,
                0.0,
            ));
        }
        rects.extend(lines);
        let mut out = Vec::new();
        push_group(&mut out, "markers", rects);
        push_group(&mut out, "label", labels);
        vec![Element::group("legend", out)]
    }
}

// --- Orchestrator ------------------------------------------------------------

/// Stroke width of line plots (`xychartDb.setLineData`).
const LINE_WIDTH: f64 = 2.0;

/// The laid-out chart: every component positioned like mermaid's `Orchestrator`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLayout {
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_axis: Axis,
    title: ChartTitle,
    legend: Legend,
}

/// mermaid's `Orchestrator.calculateSpace` (vertical and horizontal variants).
pub fn layout(data: &ChartData, c: &XyConfig, theme: &XyTheme, horizontal: bool) -> ChartLayout {
    let mut x_axis = Axis::new(&data.x, &data.x_title, &c.x_axis, &theme.x_axis);
    let y_data = AxisData::Linear(data.y.0, data.y.1);
    let mut y_axis = Axis::new(&y_data, &data.y_title, &c.y_axis, &theme.y_axis);
    let mut title = ChartTitle {
        bbox: Rect::new(0.0, 0.0, 0.0, 0.0),
        show: false,
        width: 0.0,
    };
    let mut legend = Legend {
        bbox: Rect::new(0.0, 0.0, 0.0, 0.0),
        visible: Vec::new(),
    };
    let mut aw = c.width;
    let mut ah = c.height;
    let mut chart_w = (aw * c.plot_reserved_percent / 100.0).floor();
    let mut chart_h = (ah * c.plot_reserved_percent / 100.0).floor();
    aw -= chart_w;
    ah -= chart_h;
    let t = title.calculate_space(data, c, c.width, ah);
    ah -= t.1;
    let (plot_x, plot_y, legend_space);
    if horizontal {
        let title_y_end = t.1;
        x_axis.set_position(AxisPos::Left);
        let s = x_axis.calculate_space(aw, ah);
        aw -= s.0;
        plot_x = s.0;
        y_axis.set_position(AxisPos::Top);
        let s = y_axis.calculate_space(aw, ah);
        ah -= s.1;
        plot_y = title_y_end + s.1;
        legend_space = legend.calculate_space(data, c, aw, chart_h);
        aw -= legend_space.0;
        if aw > 0.0 {
            chart_w += aw;
        }
        if ah > 0.0 {
            chart_h += ah;
        }
        y_axis.set_range(plot_x, plot_x + chart_w);
        y_axis.set_xy(plot_x, title_y_end);
        x_axis.set_range(plot_y, plot_y + chart_h);
        x_axis.set_xy(0.0, plot_y);
    } else {
        plot_y = t.1;
        x_axis.set_position(AxisPos::Bottom);
        let s = x_axis.calculate_space(aw, ah);
        ah -= s.1;
        y_axis.set_position(AxisPos::Left);
        let s = y_axis.calculate_space(aw, ah);
        plot_x = s.0;
        aw -= s.0;
        legend_space = legend.calculate_space(data, c, aw, chart_h);
        aw -= legend_space.0;
        if aw > 0.0 {
            chart_w += aw;
        }
        if ah > 0.0 {
            chart_h += ah;
        }
        x_axis.set_range(plot_x, plot_x + chart_w);
        x_axis.set_xy(plot_x, plot_y + chart_h);
        y_axis.set_range(plot_y, plot_y + chart_h);
        y_axis.set_xy(0.0, plot_y);
    }
    title.align_to(c, plot_x, plot_x + chart_w);
    legend.bbox.x = plot_x + chart_w;
    legend.bbox.y = plot_y + ((chart_h - legend_space.1) / 2.0).max(0.0);
    if data.plots.iter().any(|p| p.kind == SeriesKind::Bar) {
        x_axis.recalculate_outer_padding_to_draw_bar();
    }
    ChartLayout {
        plot: Rect::new(plot_x, plot_y, chart_w, chart_h),
        x_axis,
        y_axis,
        title,
        legend,
    }
}

// --- Plots -----------------------------------------------------------------

/// `xychartRenderer`'s data labels for one bar plot (values of the first plot,
/// as mermaid does).
fn bar_data_labels(
    rects: &[(Rect, usize)],
    labels: &[Option<f64>],
    c: &XyConfig,
    theme: &XyTheme,
    horizontal: bool,
) -> Vec<Element> {
    const CHAR_W: f64 = 0.7;
    const MARGIN: f64 = 10.0;
    let valid: Vec<(Rect, String)> = rects
        .iter()
        .filter(|(r, _)| r.w > 0.0 && r.h > 0.0)
        .filter_map(|(r, i)| Some((*r, js_num(labels.get(*i).copied().flatten()?))))
        .collect();
    let len = |s: &str| s.encode_utf16().count() as f64;
    let sizes = valid.iter().map(|(r, s)| {
        let n = len(s);
        if horizontal {
            let mut fs = r.h * 0.7;
            while fs * n * CHAR_W > r.w - MARGIN && fs > 0.0 {
                fs -= 1.0;
            }
            fs
        } else {
            let fits = |fs: f64| {
                let tw = fs * n * CHAR_W;
                let cx = r.x + r.w / 2.0;
                cx - tw / 2.0 >= r.x && cx + tw / 2.0 <= r.x + r.w && r.y + MARGIN + fs <= r.y + r.h
            };
            let mut fs = r.w / (n * CHAR_W);
            while !fits(fs) && fs > 0.0 {
                fs -= 1.0;
            }
            fs
        }
    });
    let fs = sizes.fold(f64::INFINITY, f64::min).floor();
    if !(fs > 0.0 && fs.is_finite()) {
        return Vec::new();
    }
    let out = c.data_label_outside;
    valid
        .iter()
        .map(|(r, s)| {
            let t = if horizontal {
                let (x, anchor) = if out {
                    (r.x + r.w + MARGIN, Anchor::Start)
                } else {
                    (r.x + r.w - MARGIN, Anchor::End)
                };
                text_at(
                    x,
                    r.y + r.h / 2.0 + BASELINE_MIDDLE * fs,
                    std::slice::from_ref(s),
                    fs,
                    anchor,
                    &theme.data_label,
                )
            } else {
                let y = if out {
                    r.y - MARGIN
                } else {
                    r.y + MARGIN + BASELINE_HANGING * fs
                };
                text_at(
                    r.x + r.w / 2.0,
                    y,
                    std::slice::from_ref(s),
                    fs,
                    Anchor::Middle,
                    &theme.data_label,
                )
            };
            Element::text(t)
        })
        .collect()
}

/// mermaid's `BasePlot`: bar and line plots in declaration order.
fn draw_plots(
    data: &ChartData,
    l: &ChartLayout,
    c: &XyConfig,
    theme: &XyTheme,
    horizontal: bool,
) -> Vec<Element> {
    let (xa, ya, plot) = (&l.x_axis, &l.y_axis, l.plot);
    let label_data: Vec<Option<f64>> = data
        .plots
        .first()
        .map(|p| p.data.iter().map(|d| d.1).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (i, p) in data.plots.iter().enumerate() {
        let pts: Vec<(usize, f64, f64)> = p
            .data
            .iter()
            .enumerate()
            .filter_map(|(k, (x, y))| Some((k, xa.scale(x), ya.scale_num((*y)?))))
            .collect();
        match p.kind {
            SeriesKind::Bar => {
                let bw = (xa.outer_padding * 2.0).min(xa.tick_distance()) * (1.0 - 0.05);
                let rects: Vec<(Rect, usize)> = pts
                    .iter()
                    .map(|&(k, xp, yp)| {
                        let r = if horizontal {
                            Rect::new(plot.x, xp - bw / 2.0, yp - plot.x, bw)
                        } else {
                            Rect::new(xp - bw / 2.0, yp, bw, plot.y + plot.h - yp)
                        };
                        (r, k)
                    })
                    .collect();
                let mut els: Vec<Element> = rects
                    .iter()
                    .map(|(r, _)| Element::rect(*r, 0.0, Paint::fill(&p.color)))
                    .collect();
                if c.show_data_label && !els.is_empty() {
                    els.extend(bar_data_labels(&rects, &label_data, c, theme, horizontal));
                }
                push_group(&mut out, &format!("bar-plot-{i}"), els);
            }
            SeriesKind::Line => {
                let points: Vec<Point> = pts
                    .iter()
                    .map(|&(_, xp, yp)| {
                        if horizontal {
                            Point::new(yp, xp)
                        } else {
                            Point::new(xp, yp)
                        }
                    })
                    .collect();
                if points.is_empty() {
                    continue;
                }
                let mut els = vec![Element::path(
                    geom::polyline(&points),
                    Paint::stroke(&p.color, LINE_WIDTH),
                )];
                if let Some(labels) = &p.point_labels {
                    // `LinePlot`: 12px labels 10px above (or right of) each point.
                    let texts: Vec<Element> = pts
                        .iter()
                        .zip(&points)
                        .filter_map(|(&(k, _, _), pt)| {
                            let s = labels.get(k).filter(|s| !s.is_empty())?;
                            Some(if horizontal {
                                text_elem(
                                    s,
                                    pt.x + 10.0,
                                    pt.y,
                                    12.0,
                                    &p.color,
                                    VPos::Middle,
                                    Anchor::Start,
                                    0.0,
                                )
                            } else {
                                text_elem(
                                    s,
                                    pt.x,
                                    pt.y - 10.0,
                                    12.0,
                                    &p.color,
                                    VPos::Middle,
                                    Anchor::Middle,
                                    0.0,
                                )
                            })
                        })
                        .collect();
                    push_group(&mut els, "labels", texts);
                }
                out.push(Element::group(format!("line-plot-{i}"), els));
            }
        }
    }
    out
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
    if chart.series.is_empty() {
        return Err(Error::parse(
            1,
            "No Plot to render, please provide a plot with some data",
        ));
    }
    let theme = XyTheme::from_config(cfg);
    Ok(draw(&chart, &xc, &theme, &pal.font_family))
}

/// Lay out and draw a parsed chart on mermaid's fixed `width × height` canvas.
pub fn draw(chart: &XyChart, xc: &XyConfig, theme: &XyTheme, font_family: &str) -> Scene {
    let horizontal = chart.orientation == Orientation::Horizontal;
    let data = chart_data(chart, &theme.plot_colors);
    let l = layout(&data, xc, theme, horizontal);
    let mut els = vec![Element::classed_shape(
        crate::scene::Geometry::Rect {
            rect: Rect::new(0.0, 0.0, xc.width, xc.height),
            rx: 0.0,
        },
        Paint::fill(&theme.background),
        "background",
    )];
    if l.title.show {
        let b = l.title.bbox;
        els.push(Element::group(
            "chart-title",
            vec![text_elem(
                &data.title,
                b.x + b.w / 2.0,
                b.y + b.h / 2.0,
                xc.title_font,
                &theme.title,
                VPos::Middle,
                Anchor::Middle,
                0.0,
            )],
        ));
    }
    els.push(Element::group(
        "plot",
        draw_plots(&data, &l, xc, theme, horizontal),
    ));
    els.extend(l.legend.draw(&data, xc, theme));
    els.push(l.x_axis.draw());
    els.push(l.y_axis.draw());
    Scene {
        kind: DiagramKind::XyChart,
        view_box: Rect::new(0.0, 0.0, xc.width, xc.height),
        elements: vec![Element::group("main", els)],
        acc_title: None,
        acc_descr: None,
        font_family: font_family.to_string(),
        css: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn base_chart() -> XyChart {
        parse(
            "xychart-beta\n    title \"Sales Revenue\"\n    x-axis \"Month of year\" [jan, feb, mar, apr, may, jun, jul, aug, sep, oct, nov, dec]\n    y-axis \"Revenue (in $)\" 4000 --> 11000\n    bar \"2024\" [5000, 6000, 7500, 8200, 9500, 10500, 11000, 10200, 9200, 8500, 7000, 6000]\n    bar \"2025\" [5400, 6300, 7100, 8800, 9900, 10800, 10600, 9800, 9000, 8300, 7400, 6600]\n    line \"Target\" [5000, 6200, 7000, 8000, 9000, 10000, 10500, 10000, 9500, 8800, 7600, 6400]",
            None,
        )
        .unwrap()
    }

    fn default_theme() -> XyTheme {
        XyTheme::from_config(&MermaidConfig::default())
    }

    #[test]
    fn d3_ticks_match_d3() {
        let t = d3_ticks(4000.0, 11000.0, 10.0);
        assert_eq!(t.len(), 15);
        assert_eq!((t[0], t[1], t[14]), (4000.0, 4500.0, 11000.0));
        assert_eq!(
            d3_ticks(0.0, 1.0, 10.0),
            vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]
        );
        assert_eq!(
            d3_ticks(-20.0, 48.0, 10.0),
            (-4..=9).map(|k| k as f64 * 5.0).collect::<Vec<_>>()
        );
        // Reversed domains give descending ticks (left axes).
        assert_eq!(d3_ticks(5.0, 1.0, 10.0)[0], 5.0);
        assert_eq!(d3_ticks(1.0, 5.0, 10.0).len(), 9);
        assert_eq!(d3_ticks(3.0, 3.0, 10.0), vec![3.0]);
        assert!(d3_ticks(f64::INFINITY, f64::NEG_INFINITY, 10.0).is_empty());
        assert_eq!(js_num(0.30000000000000004), "0.30000000000000004");
        assert_eq!(js_num(11000.0), "11000");
        assert_eq!(js_num(-0.0), "0");
        assert_eq!(js_num(-5.5), "-5.5");
    }

    #[test]
    fn band_scale_has_points_at_range_ends() {
        let cfg = AxisConfig::default();
        let colors = default_theme().x_axis;
        let data = AxisData::Band(vec!["a".into(), "b".into(), "c".into(), "a".into()]);
        let mut ax = Axis::new(&data, "", &cfg, &colors);
        ax.set_position(AxisPos::Bottom);
        ax.set_range(0.0, 100.0);
        // Duplicates collapse in the domain but stay as tick labels.
        let ticks = ax.ticks();
        assert_eq!(ticks.len(), 4);
        assert_eq!((ticks[0].0, ticks[1].0, ticks[2].0), (0.0, 50.0, 100.0));
        assert_eq!(ticks[3].0, 0.0);
        assert_eq!(ax.tick_distance(), 25.0);
        ax.recalculate_outer_padding_to_draw_bar();
        assert_eq!(ax.outer_padding, 8.0); // floor(0.7 × 25 / 2)
        assert_eq!(ax.scale(&XKey::Cat("b".into())), 50.0);
        assert_eq!(ax.scale(&XKey::Cat("zzz".into())), 8.0);
        // A single category sits in the middle.
        let mut one = Axis::new(&AxisData::Band(vec!["x".into()]), "", &cfg, &colors);
        one.set_position(AxisPos::Bottom);
        one.set_range(10.0, 30.0);
        assert_eq!(one.ticks()[0].0, 20.0);
    }

    #[test]
    fn linear_left_axis_is_reversed() {
        let cfg = AxisConfig::default();
        let colors = default_theme().y_axis;
        let mut ax = Axis::new(&AxisData::Linear(0.0, 10.0), "", &cfg, &colors);
        ax.set_position(AxisPos::Left);
        ax.set_range(0.0, 100.0);
        assert_eq!(ax.scale_num(10.0), 0.0);
        assert_eq!(ax.scale_num(0.0), 100.0);
        ax.set_position(AxisPos::Top);
        assert_eq!(ax.scale_num(10.0), 100.0);
        // A degenerate domain maps to the middle, like d3.
        let mut flat = Axis::new(&AxisData::Linear(5.0, 5.0), "", &cfg, &colors);
        flat.set_position(AxisPos::Top);
        flat.set_range(0.0, 100.0);
        assert_eq!(flat.scale_num(5.0), 50.0);
    }

    #[test]
    fn chart_data_mirrors_xychart_db() {
        let pal = default_theme().plot_colors;
        // No x-axis: linear 1..n from the first plot; y range from data (no zero).
        let c = parse("xychart-beta\nline [3, 7, 12]\nbar [1, 2]", None).unwrap();
        let d = chart_data(&c, &pal);
        assert_eq!(d.x, AxisData::Linear(1.0, 3.0));
        assert_eq!(d.y, (1.0, 12.0));
        assert_eq!(d.plots[0].data[1], (XKey::Num(2.0), Some(7.0)));
        assert_eq!(d.plots[1].data[1], (XKey::Num(3.0), Some(2.0)));
        assert_eq!(d.plots[1].color.css(), "#8493A6");
        // Band axes truncate longer data (and the y range) and leave gaps.
        let c = parse(
            "xychart-beta\nx-axis [a, b]\nbar [1, 2, 99]\nline [5]",
            None,
        )
        .unwrap();
        let d = chart_data(&c, &pal);
        assert_eq!(d.y, (1.0, 5.0));
        assert_eq!(d.plots[0].data.len(), 2);
        assert_eq!(d.plots[1].data[1], (XKey::Cat("b".into()), None));
        // An explicit range wins and is kept as written.
        let d = chart_data(&base_chart(), &pal);
        assert_eq!(d.y, (4000.0, 11000.0));
        assert_eq!(d.plots[2].title, "Target");
    }

    #[test]
    fn vertical_layout_matches_mermaid() {
        let xc = XyConfig::default();
        let theme = default_theme();
        let d = chart_data(&base_chart(), &theme.plot_colors);
        let l = layout(&d, &xc, &theme, false);
        // Title row: 22px text + 2 × 10 padding.
        assert_eq!(l.plot.y, 42.0);
        // Bottom axis: line 2 + label 16 + 2 × 5 + tick 5 + title 18 + 2 × 5.
        assert_eq!(l.x_axis.bbox.h, 61.0);
        assert_eq!(l.plot.y + l.plot.h, 439.0);
        assert_eq!(l.x_axis.pos, AxisPos::Bottom);
        assert_eq!(l.y_axis.pos, AxisPos::Left);
        assert_eq!(l.y_axis.bbox.x, 0.0);
        // Value axis inset by half a label height; 15 ticks of 500.
        let yt = l.y_axis.ticks();
        assert_eq!(yt.len(), 15);
        assert_eq!((yt[0].1.as_str(), yt[0].0), ("11000", 50.0));
        assert_eq!(yt[14].0, 431.0);
        // Category axis padded for bars: floor(0.7 × tickDistance / 2).
        // (15 in Chromium; our label widths are approximate.)
        let op = l.x_axis.outer_padding;
        assert_eq!(op, op.floor());
        assert!((14.0..=15.0).contains(&op), "{op}");
        let xt = l.x_axis.ticks();
        assert_eq!(xt[0].0, l.plot.x + op);
        assert!((xt[11].0 - (l.plot.x + l.plot.w - op)).abs() < 1e-9);
        // Legend right of the plot, vertically centred.
        assert_eq!(l.legend.bbox.x, l.plot.x + l.plot.w);
        assert!(l.legend.bbox.y > l.plot.y);
        assert!(l.plot.x + l.plot.w + l.legend.bbox.w <= 700.0 + 1e-9);
    }

    /// Every `(class, rects)` of bar plot groups.
    fn bar_groups(els: &[Element]) -> Vec<(String, Vec<Rect>)> {
        let mut out = Vec::new();
        for e in els {
            if let Element::Group {
                class, children, ..
            } = e
            {
                let class = class.clone().unwrap_or_default();
                if class.starts_with("bar-plot-") {
                    let rs = children
                        .iter()
                        .filter_map(|c| match c {
                            Element::Shape {
                                geom: crate::scene::Geometry::Rect { rect, .. },
                                ..
                            } => Some(*rect),
                            _ => None,
                        })
                        .collect();
                    out.push((class, rs));
                } else {
                    out.extend(bar_groups(children));
                }
            }
        }
        out
    }

    #[test]
    fn bars_of_several_series_overlap() {
        let theme = default_theme();
        let s = draw(&base_chart(), &XyConfig::default(), &theme, "x");
        let d = chart_data(&base_chart(), &theme.plot_colors);
        let l = layout(&d, &XyConfig::default(), &theme, false);
        let bw = (l.x_axis.outer_padding * 2.0).min(l.x_axis.tick_distance()) * 0.95;
        let bars = bar_groups(&s.elements);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].0, "bar-plot-0");
        for (a, b) in bars[0].1.iter().zip(&bars[1].1) {
            assert_eq!((a.x, a.w), (b.x, b.w));
            assert_eq!(a.w, bw); // min(2 × outerPadding, tickDistance) × 0.95
            assert_eq!(a.y + a.h, 439.0); // bars start at the plot bottom
        }
        assert_eq!(s.view_box, Rect::new(0.0, 0.0, 700.0, 500.0));
    }

    #[test]
    fn horizontal_puts_value_axis_on_top() {
        let mut c = base_chart();
        c.orientation = Orientation::Horizontal;
        let theme = default_theme();
        let xc = XyConfig::default();
        let d = chart_data(&c, &theme.plot_colors);
        let l = layout(&d, &xc, &theme, true);
        assert_eq!(l.x_axis.pos, AxisPos::Left);
        assert_eq!(l.y_axis.pos, AxisPos::Top);
        assert_eq!(l.x_axis.bbox.x, 0.0);
        assert_eq!(l.y_axis.bbox.y, 42.0);
        assert_eq!(l.plot.y, 42.0 + l.y_axis.bbox.h);
        // Categories run top to bottom, values left to right.
        let xt = l.x_axis.ticks();
        assert!(xt[0].0 < xt[1].0);
        let yt = l.y_axis.ticks();
        assert_eq!(yt[0].1, "4000");
        assert!(yt[0].0 < yt[1].0);
        let bars = bar_groups(&draw(&c, &xc, &theme, "x").elements);
        assert!(bars[0].1.iter().all(|r| r.x == l.plot.x));
    }

    #[test]
    fn toggles_hide_parts() {
        let cfg = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(
                    r#"{"xyChart":{"showTitle":false,"showLegend":false,"xAxis":{"showTick":false},"yAxis":{"showLabel":false,"showAxisLine":false}}}"#,
                )
                .unwrap(),
            )
            .unwrap();
        let xc = XyConfig::from_config(&cfg);
        assert!(!xc.show_title && !xc.show_legend);
        assert!(!xc.x_axis.show_tick && xc.x_axis.show_label);
        assert!(!xc.y_axis.show_label && !xc.y_axis.show_axis_line);
        let theme = default_theme();
        let d = chart_data(&base_chart(), &theme.plot_colors);
        let l = layout(&d, &xc, &theme, false);
        assert_eq!(l.plot.y, 0.0);
        assert!(l.legend.visible.is_empty());
        // Only tick (5) + title (18 + 10) remain on the left.
        assert_eq!(l.y_axis.bbox.w, 33.0);
    }

    #[test]
    fn theme_colors() {
        let t = default_theme();
        assert_eq!(t.title.css(), "#131300");
        assert_eq!(t.plot_colors[2].css(), "#FFC3A0");
        let dark = MermaidConfig::default()
            .with_directive(&crate::json::parse(r#"{"theme":"dark"}"#).unwrap())
            .unwrap();
        let t = XyTheme::from_config(&dark);
        assert_eq!(t.background.css(), "#333");
        assert_eq!(t.plot_colors[0].css(), "#3498db");
        let custom = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(
                    r##"{"themeVariables":{"xyChart":{"plotColorPalette":"#f00, #0f0","titleColor":"#123456"}}}"##,
                )
                .unwrap(),
            )
            .unwrap();
        let t = XyTheme::from_config(&custom);
        assert_eq!(t.plot_colors.len(), 2);
        assert_eq!(t.plot_colors[1].css(), "#0f0");
        assert_eq!(t.title.css(), "#123456");
    }

    #[test]
    fn renders_scene_and_rejects_empty_charts() {
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
        assert_eq!((s.view_box.w, s.view_box.h), (700.0, 500.0));
        assert!(render("xychart-beta\ntitle T", None, &cfg, &pal).is_err());
    }

    /// `(deg, about, text)` of every `Rotated` element holding a text.
    fn rotated_texts(els: &[Element]) -> Vec<(f64, Point, Text)> {
        let mut out = Vec::new();
        for e in els {
            match e {
                Element::Group { children, .. } => out.extend(rotated_texts(children)),
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
    fn left_axis_title_is_rotated() {
        let src =
            "xychart-beta\n  x-axis \"Month\" [a, b]\n  y-axis \"Revenue\" 0 --> 10\n  bar [1, 2]";
        let cfg = MermaidConfig::default();
        let s = render(src, None, &cfg, &cfg.palette()).unwrap();
        let rot = rotated_texts(&s.elements);
        assert_eq!(rot.len(), 1, "{rot:?}");
        let (deg, about, t) = &rot[0];
        assert_eq!(*deg, -90.0);
        assert_eq!(t.lines, vec!["Revenue".to_string()]);
        assert_eq!(t.anchor, Anchor::Middle);
        // titlePadding (5) from the chart's left edge.
        assert!((about.x - 5.0).abs() < 1e-9);
        // Horizontal charts rotate the category (left) axis title instead.
        let s = render(
            &src.replace("xychart-beta", "xychart-beta horizontal"),
            None,
            &cfg,
            &cfg.palette(),
        )
        .unwrap();
        let rot = rotated_texts(&s.elements);
        assert_eq!(rot.len(), 1);
        assert_eq!(rot[0].2.lines, vec!["Month".to_string()]);
    }

    #[test]
    fn x_label_rotation_turns_bottom_labels() {
        let src = "xychart-beta\n  x-axis [alpha, beta]\n  bar [1, 2]";
        let cfg = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(r#"{"xyChart":{"xAxis":{"labelRotation":-45}}}"#).unwrap(),
            )
            .unwrap();
        let xc = XyConfig::from_config(&cfg);
        assert_eq!(xc.x_axis.label_rotation, -45.0);
        let s = render(src, None, &cfg, &cfg.palette()).unwrap();
        let rot = rotated_texts(&s.elements);
        assert_eq!(rot.len(), 2, "{rot:?}");
        assert!(rot.iter().all(|(d, _, _)| *d == -45.0));
        // Shifted left by sin(r)·labelHeight/2 from the tick position.
        let theme = default_theme();
        let c = parse(src, None).unwrap();
        let l = layout(&chart_data(&c, &theme.plot_colors), &xc, &theme, false);
        let tick = l.x_axis.ticks()[0].0;
        let shift = (-45f64).to_radians().sin() * tspan_height("alpha", 14.0) / 2.0;
        assert!((rot[0].1.x - (tick + shift)).abs() < 1e-9);
        assert!((rot[0].2.x - rot[0].1.x).abs() < 1e-9);
        // Out-of-range rotations are ignored, like mermaid.
        let cfg = MermaidConfig::default()
            .with_directive(
                &crate::json::parse(r#"{"xyChart":{"xAxis":{"labelRotation":120}}}"#).unwrap(),
            )
            .unwrap();
        assert_eq!(XyConfig::from_config(&cfg).x_axis.label_rotation, 0.0);
        let s = render(src, None, &cfg, &cfg.palette()).unwrap();
        assert!(rotated_texts(&s.elements).is_empty());
    }
}
