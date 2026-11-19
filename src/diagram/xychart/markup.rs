//! The DOM mermaid's `xychartRenderer` builds from the chart builder's
//! drawable elements, written as its serialiser prints it: groups created on
//! first use (`getGroup`), text placed by `transform`, paths and numbers as
//! the template literals and d3 print them.

use super::*;
use crate::output::svg::{esc_text, js_num as n};

enum Kid {
    Raw(String),
    Group(Grp),
}

struct Grp {
    class: String,
    kids: Vec<Kid>,
}

impl Grp {
    fn new(class: &str) -> Grp {
        Grp {
            class: class.to_string(),
            kids: Vec::new(),
        }
    }

    /// `getGroup`: the nested `<g class>` for `path`, created on first use.
    fn at(&mut self, path: &[&str]) -> &mut Grp {
        let Some((first, rest)) = path.split_first() else {
            return self;
        };
        let at = self
            .kids
            .iter()
            .position(|k| matches!(k, Kid::Group(g) if g.class == *first))
            .unwrap_or_else(|| {
                self.kids.push(Kid::Group(Grp::new(first)));
                self.kids.len() - 1
            });
        let Kid::Group(g) = &mut self.kids[at] else {
            unreachable!()
        };
        g.at(rest)
    }

    /// Append `items` to the group at `path`; mermaid skips empty shapes.
    fn push_all(&mut self, path: &[&str], items: Vec<String>) {
        if items.is_empty() {
            return;
        }
        let g = self.at(path);
        g.kids.extend(items.into_iter().map(Kid::Raw));
    }

    fn write(&self, out: &mut String) {
        if self.kids.is_empty() {
            out.push_str(&format!("<g class=\"{}\"/>", self.class));
            return;
        }
        out.push_str(&format!("<g class=\"{}\">", self.class));
        for k in &self.kids {
            match k {
                Kid::Raw(s) => out.push_str(s),
                Kid::Group(g) => g.write(out),
            }
        }
        out.push_str("</g>");
    }
}

/// A `TextElem`: `type: 'text'` drawable data.
struct Txt<'a> {
    text: &'a str,
    x: f64,
    y: f64,
    fill: &'a Color,
    font_size: f64,
    top: bool,
    anchor: Anchor,
    rotation: f64,
}

fn text_el(t: &Txt) -> String {
    let base = if t.top { "text-before-edge" } else { "middle" };
    let anchor = match t.anchor {
        Anchor::Start => "start",
        Anchor::End => "end",
        _ => "middle",
    };
    format!(
        "<text x=\"0\" y=\"0\" fill=\"{}\" font-size=\"{}\" dominant-baseline=\"{base}\" text-anchor=\"{anchor}\" transform=\"translate({}, {}) rotate({})\">{}</text>",
        t.fill.css(),
        n(t.font_size),
        n(t.x),
        n(t.y),
        n(t.rotation),
        esc_text(t.text)
    )
}

/// A `type: 'path'` datum (mermaid's template literals, trimmed).
fn path_el(path: &str, stroke: &Color, width: f64) -> String {
    format!(
        "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"/>",
        path.trim(),
        stroke.css(),
        n(width)
    )
}

/// `M a,b L c,d` as the axes and the legend write it.
fn segment(a: Point, b: Point) -> String {
    format!("M {},{} L {},{}", n(a.x), n(a.y), n(b.x), n(b.y))
}

fn rect_el(r: Rect, fill: &Color) -> String {
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"0\"/>",
        n(r.x),
        n(r.y),
        n(r.w),
        n(r.h),
        fill.css(),
        fill.css()
    )
}

/// d3-path's `Math.round(v * 1000) / 1000`.
fn d3_round(v: f64) -> String {
    n((v * 1000.0 + 0.5).floor() / 1000.0)
}

/// d3 `line()` with linear segments.
fn d3_line(points: &[Point]) -> String {
    let mut s = String::new();
    for (i, p) in points.iter().enumerate() {
        s.push(if i == 0 { 'M' } else { 'L' });
        s.push_str(&d3_round(p.x));
        s.push(',');
        s.push_str(&d3_round(p.y));
    }
    if points.len() == 1 {
        s.push('Z');
    }
    s
}

impl Axis {
    /// `getDrawableElements` of the axis.
    fn drawables(&self, g: &mut Grp) {
        let class = match self.pos {
            AxisPos::Left => "left-axis",
            AxisPos::Bottom => "bottom-axis",
            AxisPos::Top => "top-axis",
        };
        let (c, k, b) = (&self.cfg, &self.colors, self.bbox);
        let if_line = if self.show_axis_line {
            c.axis_line_width
        } else {
            0.0
        };
        let if_tick = if self.show_tick { c.tick_length } else { 0.0 };
        let ticks = self.ticks();
        let text = |s: &str, x: f64, y: f64, top: bool, anchor: Anchor, rotation: f64, title| {
            let (fill, font_size) = if title {
                (&k.title, c.title_font)
            } else {
                (&k.label, c.label_font)
            };
            text_el(&Txt {
                text: s,
                x,
                y,
                fill,
                font_size,
                top,
                anchor,
                rotation,
            })
        };
        let line_path = |p: String| vec![path_el(&p, &k.line, c.axis_line_width)];
        let tick_paths = |f: &dyn Fn(f64) -> String| -> Vec<String> {
            ticks
                .iter()
                .map(|(p, _)| path_el(&f(*p), &k.tick, c.tick_width))
                .collect()
        };
        match self.pos {
            AxisPos::Left => {
                if self.show_axis_line {
                    let x = b.x + b.w - c.axis_line_width / 2.0;
                    let p = segment(Point::new(x, b.y), Point::new(x, b.y + b.h));
                    g.push_all(&[class, "axisl-line"], line_path(p));
                }
                if self.show_label {
                    let x = b.x + b.w - c.label_padding - if_tick - if_line;
                    let items = ticks
                        .iter()
                        .map(|(p, s)| text(s, x, *p, false, Anchor::End, 0.0, false))
                        .collect();
                    g.push_all(&[class, "label"], items);
                }
                if self.show_tick {
                    let x = b.x + b.w - if_line;
                    let items = tick_paths(&|p| {
                        segment(Point::new(x, p), Point::new(x - c.tick_length, p))
                    });
                    g.push_all(&[class, "ticks"], items);
                }
                if self.show_title {
                    let t = text(
                        &self.title,
                        b.x + c.title_padding,
                        b.y + b.h / 2.0,
                        true,
                        Anchor::Middle,
                        270.0,
                        true,
                    );
                    g.push_all(&[class, "title"], vec![t]);
                }
            }
            AxisPos::Bottom => {
                if self.show_axis_line {
                    let y = b.y + c.axis_line_width / 2.0;
                    let p = segment(Point::new(b.x, y), Point::new(b.x + b.w, y));
                    g.push_all(&[class, "axis-line"], line_path(p));
                }
                if self.show_label {
                    let dx = self.offset_by_rotation(true);
                    let y = b.y
                        + c.label_padding
                        + if_tick
                        + if_line
                        + self.offset_by_rotation(false).abs();
                    let rotation = self.rot * 180.0 / std::f64::consts::PI;
                    let items = ticks
                        .iter()
                        .map(|(p, s)| text(s, p + dx, y, true, Anchor::Middle, rotation, false))
                        .collect();
                    g.push_all(&[class, "label"], items);
                }
                if self.show_tick {
                    let y = b.y + if_line;
                    let items = tick_paths(&|p| {
                        segment(Point::new(p, y), Point::new(p, y + c.tick_length))
                    });
                    g.push_all(&[class, "ticks"], items);
                }
                if self.show_title {
                    let t = text(
                        &self.title,
                        self.range.0 + (self.range.1 - self.range.0) / 2.0,
                        b.y + b.h - c.title_padding - self.title_text_height,
                        true,
                        Anchor::Middle,
                        0.0,
                        true,
                    );
                    g.push_all(&[class, "title"], vec![t]);
                }
            }
            AxisPos::Top => {
                if self.show_axis_line {
                    let y = b.y + b.h - c.axis_line_width / 2.0;
                    let p = segment(Point::new(b.x, y), Point::new(b.x + b.w, y));
                    g.push_all(&[class, "axis-line"], line_path(p));
                }
                if self.show_label {
                    let title_h = if self.show_title {
                        self.title_text_height + c.title_padding * 2.0
                    } else {
                        0.0
                    };
                    let y = b.y + title_h + c.label_padding;
                    let items = ticks
                        .iter()
                        .map(|(p, s)| text(s, *p, y, true, Anchor::Middle, 0.0, false))
                        .collect();
                    g.push_all(&[class, "label"], items);
                }
                if self.show_tick {
                    let y = b.y;
                    let items = tick_paths(&|p| {
                        segment(
                            Point::new(p, y + b.h - if_line),
                            Point::new(p, y + b.h - c.tick_length - if_line),
                        )
                    });
                    g.push_all(&[class, "ticks"], items);
                }
                if self.show_title {
                    let t = text(
                        &self.title,
                        b.x + b.w / 2.0,
                        b.y + c.title_padding,
                        true,
                        Anchor::Middle,
                        0.0,
                        true,
                    );
                    g.push_all(&[class, "title"], vec![t]);
                }
            }
        }
    }
}

impl Legend {
    /// `ChartLegend.getDrawableElements`.
    fn drawables(&self, data: &ChartData, c: &XyConfig, theme: &XyTheme, g: &mut Grp) {
        if self.visible.is_empty() {
            return;
        }
        let fs = c.legend_font;
        let marker = fs * LEGEND_MARKER_TO_FONT_RATIO;
        let spacing = fs * LEGEND_MARKER_SPACING_TO_FONT_RATIO;
        let row = fs + fs * LEGEND_ITEM_SPACING_TO_FONT_RATIO;
        let x0 = self.bbox.x + c.legend_padding;
        let y0 = self.bbox.y + c.legend_padding;
        let (mut rects, mut paths, mut labels) = (Vec::new(), Vec::new(), Vec::new());
        for (k, &i) in self.visible.iter().enumerate() {
            let p = &data.plots[i];
            match p.kind {
                SeriesKind::Bar => rects.push(rect_el(
                    Rect::new(x0, y0 + k as f64 * row, marker, marker),
                    &p.color,
                )),
                SeriesKind::Line => {
                    let y = y0 + k as f64 * row + marker / 2.0;
                    let d = segment(Point::new(x0, y), Point::new(x0 + marker, y));
                    paths.push(path_el(&d, &p.color, LINE_WIDTH));
                }
            }
            labels.push(text_el(&Txt {
                text: &p.title,
                x: x0 + marker + spacing,
                y: y0 + k as f64 * row + marker / 2.0,
                fill: &theme.legend_text,
                font_size: fs,
                top: false,
                anchor: Anchor::Start,
                rotation: 0.0,
            }));
        }
        g.push_all(&["legend", "markers"], rects);
        g.push_all(&["legend", "markers"], paths);
        g.push_all(&["legend", "label"], labels);
    }
}

/// Mermaid's DOM for a laid-out chart, from `<g/>` to the temporary group.
pub(super) fn markup(
    data: &ChartData,
    l: &ChartLayout,
    xc: &XyConfig,
    theme: &XyTheme,
    horizontal: bool,
) -> String {
    let mut main = Grp::new("main");
    main.kids.push(Kid::Raw(format!(
        "<rect width=\"{}\" height=\"{}\" class=\"background\" fill=\"{}\"/>",
        n(xc.width),
        n(xc.height),
        theme.background.css()
    )));
    if l.title.show {
        let b = l.title.bbox;
        let t = text_el(&Txt {
            text: &data.title,
            x: b.x + b.w / 2.0,
            y: b.y + b.h / 2.0,
            fill: &theme.title,
            font_size: xc.title_font,
            top: false,
            anchor: Anchor::Middle,
            rotation: 0.0,
        });
        main.push_all(&["chart-title"], vec![t]);
    }
    plots(data, l, xc, theme, horizontal, &mut main);
    l.legend.drawables(data, xc, theme, &mut main);
    l.x_axis.drawables(&mut main);
    l.y_axis.drawables(&mut main);
    let mut out = String::from("<g/>");
    main.write(&mut out);
    out.push_str("<g class=\"mermaid-tmp-group\"/>");
    out
}

/// `BarPlot` / `LinePlot` drawables and `xychartRenderer`'s data labels.
fn plots(
    data: &ChartData,
    l: &ChartLayout,
    c: &XyConfig,
    theme: &XyTheme,
    horizontal: bool,
    g: &mut Grp,
) {
    let (xa, ya, plot) = (&l.x_axis, &l.y_axis, l.plot);
    let label_data: Vec<Option<f64>> = data
        .plots
        .first()
        .map(|p| p.data.iter().map(|d| d.1).collect())
        .unwrap_or_default();
    for (i, p) in data.plots.iter().enumerate() {
        let pts: Vec<(usize, f64, f64)> = p
            .data
            .iter()
            .enumerate()
            .map(|(k, (x, y))| (k, xa.scale(x), y.map_or(f64::NAN, |y| ya.scale_num(y))))
            .collect();
        match p.kind {
            SeriesKind::Bar => {
                let bw = (xa.outer_padding * 2.0).min(xa.tick_distance()) * (1.0 - 0.05);
                let bw_half = bw / 2.0;
                let rects: Vec<(Rect, usize)> = pts
                    .iter()
                    .map(|&(k, xp, yp)| {
                        let r = if horizontal {
                            Rect::new(plot.x, xp - bw_half, yp - plot.x, bw)
                        } else {
                            Rect::new(xp - bw_half, yp, bw, plot.y + plot.h - yp)
                        };
                        (r, k)
                    })
                    .collect();
                let class = format!("bar-plot-{i}");
                let path = ["plot", class.as_str()];
                g.push_all(
                    &path,
                    rects.iter().map(|(r, _)| rect_el(*r, &p.color)).collect(),
                );
                if c.show_data_label && !rects.is_empty() {
                    let labels = data_label_els(&rects, &label_data, c, theme, horizontal);
                    g.push_all(&path, labels);
                }
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
                let class = format!("line-plot-{i}");
                g.push_all(
                    &["plot", &class],
                    vec![path_el(&d3_line(&points), &p.color, LINE_WIDTH)],
                );
                if let Some(labels) = &p.point_labels {
                    let items: Vec<String> = points
                        .iter()
                        .enumerate()
                        .filter_map(|(k, pt)| {
                            let s = labels.get(k).filter(|s| !s.is_empty())?;
                            let (x, y, anchor) = if horizontal {
                                (pt.x + 10.0, pt.y, Anchor::Start)
                            } else {
                                (pt.x, pt.y - 10.0, Anchor::Middle)
                            };
                            Some(text_el(&Txt {
                                text: s,
                                x,
                                y,
                                fill: &p.color,
                                font_size: 12.0,
                                top: false,
                                anchor,
                                rotation: 0.0,
                            }))
                        })
                        .collect();
                    g.push_all(&["plot", &class, "labels"], items);
                }
            }
        }
    }
}

/// The data labels `xychartRenderer` appends to a bar plot's group.
fn data_label_els(
    rects: &[(Rect, usize)],
    labels: &[Option<f64>],
    c: &XyConfig,
    theme: &XyTheme,
    horizontal: bool,
) -> Vec<String> {
    const MARGIN: f64 = 10.0;
    let valid = valid_labels(rects, labels);
    if valid.is_empty() {
        return Vec::new();
    }
    let fs = uniform_font_size(&valid, horizontal);
    let out = c.data_label_outside;
    valid
        .iter()
        .map(|(r, s)| {
            let (x, y, anchor, base) = if horizontal {
                if out {
                    (r.x + r.w + MARGIN, r.y + r.h / 2.0, "start", "middle")
                } else {
                    (r.x + r.w - MARGIN, r.y + r.h / 2.0, "end", "middle")
                }
            } else if out {
                (r.x + r.w / 2.0, r.y - MARGIN, "middle", "auto")
            } else {
                (r.x + r.w / 2.0, r.y + MARGIN, "middle", "hanging")
            };
            format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" dominant-baseline=\"{base}\" fill=\"{}\" font-size=\"{}px\">{}</text>",
                n(x),
                n(y),
                theme.data_label.css(),
                n(fs),
                esc_text(s)
            )
        })
        .collect()
}
