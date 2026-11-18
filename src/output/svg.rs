//! `Scene` → SVG text, shaped like mermaid's output (`<svg id="my-svg" ...>`).

use crate::geom::PathCmd;
use crate::scene::{Anchor, DiagramKind, Element, Geometry, Gradient, Paint, Scene, Shadow, Text};
use std::fmt::Write as _;

/// How the root `<svg>` element is sized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SvgSizing {
    /// mermaid's `useMaxWidth: true`: `width="100%"` + `max-width` style.
    MaxWidth,
    /// mmdc `--size`: `max-width`/`max-height` set to the given pixel size.
    Size(u32),
    /// Fixed pixel dimensions (used internally for rasterising).
    Fixed { width: f64, height: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SvgOptions<'a> {
    pub id: &'a str,
    pub background: &'a str,
    pub sizing: SvgSizing,
    /// Raw CSS appended to the generated `<style>` (mermaid `themeCSS`).
    pub theme_css: Option<&'a str>,
    /// Device pixels per user unit when the SVG is only drawn to a raster
    /// (PNG): text Chromium paints off its layout position is written where
    /// it paints at that scale, rather than in mermaid's markup.
    pub raster_scale: Option<f64>,
}

/// Compact, deterministic number formatting.
pub fn num(v: f64) -> String {
    let v = if v.abs() < 5e-4 { 0.0 } else { v };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// XML-escape text content and attribute values.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Path data string for path commands.
pub fn path_data(cmds: &[PathCmd]) -> String {
    use crate::diagram::xychart::js_num;
    // A path with SVG arcs is venn.js's: every number in full.
    let arcs = cmds.iter().any(|c| matches!(c, PathCmd::ArcTo { .. }));
    let mut d = String::new();
    for c in cmds {
        match c {
            PathCmd::MoveTo(p) if arcs => {
                let _ = write!(d, "M {} {} ", js_num(p.x), js_num(p.y));
            }
            PathCmd::MoveTo(p) => {
                let _ = write!(d, "M{},{}", num(p.x), num(p.y));
            }
            PathCmd::LineTo(p) => {
                let _ = write!(d, "L{},{}", num(p.x), num(p.y));
            }
            PathCmd::CubicTo(a, b, p) => {
                let _ = write!(
                    d,
                    "C{},{},{},{},{},{}",
                    num(a.x),
                    num(a.y),
                    num(b.x),
                    num(b.y),
                    num(p.x),
                    num(p.y)
                );
            }
            PathCmd::Close => d.push('Z'),
            // venn.js's spelling (`circlePath`, `intersectionAreaPath`), its
            // numbers as JavaScript prints them, so the browser parses the same
            // floats it parses from mermaid's markup.
            PathCmd::MoveBy(p) => {
                let _ = write!(d, "m {} {} ", js_num(p.x), js_num(p.y));
            }
            PathCmd::ArcTo {
                rx,
                ry,
                large,
                sweep,
                to,
                rel,
            } => {
                let _ = write!(
                    d,
                    "{} {} {} 0 {} {} {} {} ",
                    if *rel { 'a' } else { 'A' },
                    js_num(*rx),
                    js_num(*ry),
                    u8::from(*large),
                    u8::from(*sweep),
                    js_num(to.x),
                    js_num(to.y)
                );
            }
        }
    }
    if arcs {
        d.truncate(d.trim_end().len());
    }
    d
}

/// Half an 8-bit step (0.5 / 255) as a transfer function's intercept.
const HALF_STEP: &str = "0.0019607843";

/// The drop-shadow filters and stroke gradients of a document: its id, the
/// distinct shadows (`filter` ids `{id}-shadow-{index}`) and gradients
/// (`{id}-gradient-{index}`).
struct Filters<'a> {
    id: &'a str,
    /// The drawing's extent: every shadow filter's region (in user space, so a
    /// thin shape's blur is not clipped to a margin around its own box).
    region: crate::geom::Rect,
    shadows: Vec<Shadow>,
    gradients: Vec<Gradient>,
    /// The classes of [`Element::Brightened`] groups and their factors, each
    /// written as one stylesheet rule.
    brightness: Vec<(String, f64)>,
    /// [`SvgOptions::raster_scale`].
    raster_scale: Option<f64>,
}

impl Filters<'_> {
    fn collect(&mut self, els: &[Element]) {
        for el in els {
            match el {
                Element::Group { children, .. }
                | Element::Rotated { children, .. }
                | Element::Translated { children, .. }
                | Element::Faded { children, .. }
                | Element::Clipped { children, .. } => {
                    self.collect(children);
                }
                Element::Shadowed { shadow, children } => {
                    if !self.shadows.contains(shadow) {
                        self.shadows.push(shadow.clone());
                    }
                    self.collect(children);
                }
                Element::Brightened {
                    factor,
                    class,
                    children,
                } => {
                    let rule = (class.clone(), *factor);
                    if !self.brightness.contains(&rule) {
                        self.brightness.push(rule);
                    }
                    self.collect(children);
                }
                Element::Shape { paint, .. } => {
                    if let Some(sh) = &paint.shadow
                        && !self.shadows.contains(sh)
                    {
                        self.shadows.push(sh.clone());
                    }
                    if let Some(g) = &paint.stroke_gradient
                        && !self.gradients.contains(g)
                    {
                        self.gradients.push(g.clone());
                    }
                }
                Element::Text { .. }
                | Element::OutlinedText { .. }
                | Element::Image { .. }
                | Element::Icon { .. } => {}
            }
        }
    }

    /// The `url(#...)` a gradient-stroked shape's `stroke` refers to.
    fn stroke_url(&self, p: &Paint) -> Option<String> {
        p.stroke_gradient
            .as_ref()
            .and_then(|g| self.gradients.iter().position(|x| x == g))
            .map(|k| format!("url(#{}-gradient-{k})", self.id))
    }

    fn attr(&self, p: &Paint) -> String {
        p.shadow
            .as_ref()
            .map(|sh| self.filter_attr(sh))
            .unwrap_or_default()
    }

    /// The attribute that casts `sh`: its `<filter>`, or, for a CSS shadow in
    /// SVG output, the CSS function itself.
    fn filter_attr(&self, sh: &Shadow) -> String {
        if sh.css && self.raster_scale.is_none() {
            return format!(" style=\"filter:{}\"", esc(&sh.css_function()));
        }
        self.shadows
            .iter()
            .position(|s| s == sh)
            .map(|k| format!(" filter=\"url(#{}-shadow-{k})\"", self.id))
            .unwrap_or_default()
    }

    /// mermaid's rule for each brightened class (timeline/styles.js
    /// `.eventWrapper { filter: brightness(120%) }`), scoped to the document.
    /// None when rasterising: the groups then refer to [`Filters::defs`]'
    /// equivalent `<filter>` instead.
    fn css(&self) -> String {
        if self.raster_scale.is_some() {
            return String::new();
        }
        self.brightness
            .iter()
            .map(|(class, k)| {
                format!(
                    "#{} .{}{{filter:brightness({}%);}}",
                    self.id,
                    esc(class),
                    num(k * 100.0)
                )
            })
            .collect()
    }

    /// The `filter` a brightened group carries when rasterising.
    fn brightness_attr(&self, class: &str, factor: f64) -> String {
        if self.raster_scale.is_none() {
            return String::new();
        }
        self.brightness
            .iter()
            .position(|(c, k)| c == class && *k == factor)
            .map(|k| format!(" filter=\"url(#{}-brightness-{k})\"", self.id))
            .unwrap_or_default()
    }

    /// `<defs>` with a `feDropShadow` per shadow (the flood split into an
    /// opaque colour and its opacity, which every renderer reads).
    fn defs(&self) -> String {
        let brightness = if self.raster_scale.is_some() {
            &self.brightness[..]
        } else {
            &[]
        };
        // A CSS shadow is written in SVG output as the function itself.
        let svg_css = |sh: &Shadow| sh.css && self.raster_scale.is_none();
        if self.shadows.iter().all(svg_css) && self.gradients.is_empty() && brightness.is_empty() {
            return String::new();
        }
        let mut out = String::from("<defs>");
        // CSS `brightness(k)` as the filter element it stands for (Filter
        // Effects 1): a linear transfer of slope k on each colour channel, in
        // sRGB. resvg truncates the result to 8 bits where Chromium rounds
        // it, so the intercept adds the half step.
        for (k, (_, factor)) in brightness.iter().enumerate() {
            let f = |c: char| {
                format!(
                    "<feFunc{c} type=\"linear\" slope=\"{}\" intercept=\"{}\"/>",
                    num(*factor),
                    HALF_STEP
                )
            };
            let _ = write!(
                out,
                "<filter id=\"{}-brightness-{k}\" color-interpolation-filters=\"sRGB\"><feComponentTransfer>{}{}{}</feComponentTransfer></filter>",
                self.id,
                f('R'),
                f('G'),
                f('B'),
            );
        }
        for (k, g) in self.gradients.iter().enumerate() {
            let (units, x1, x2) = if g.bbox {
                (
                    "objectBoundingBox",
                    format!("{}%", num(g.x1 * 100.0)),
                    format!("{}%", num(g.x2 * 100.0)),
                )
            } else {
                ("userSpaceOnUse", num(g.x1), num(g.x2))
            };
            let _ = write!(
                out,
                "<linearGradient id=\"{}-gradient-{k}\" gradientUnits=\"{units}\" x1=\"{x1}\" x2=\"{x2}\" y1=\"0\" y2=\"0\"><stop offset=\"0%\" stop-color=\"{}\"/><stop offset=\"100%\" stop-color=\"{}\"/></linearGradient>",
                self.id,
                esc(g.from.css()),
                esc(g.to.css())
            );
        }
        // A CSS `drop-shadow()` spelt out: the alpha in the shadow colour (a colour
        // matrix rather than `feFlood`, which resvg mistakes for linear RGB under
        // `color-interpolation-filters="sRGB"`), blurred, offset, under the shape.
        // sRGB, as CSS filter functions work.
        let r = self.region;
        let m = r.w.max(r.h);
        for (k, sh) in self.shadows.iter().enumerate() {
            if svg_css(sh) {
                continue;
            }
            let c = crate::color::parse(sh.color.css()).unwrap_or(crate::color::Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 1.0,
            });
            if sh.linear_rgb && self.raster_scale.is_none() {
                // As mermaid writes it, its region the default one, 130% wide
                // and tall.
                let _ = write!(
                    out,
                    "<filter id=\"{}-shadow-{k}\" height=\"130%\" width=\"130%\"><feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-opacity=\"{}\" flood-color=\"#{:02X}{:02X}{:02X}\"/></filter>",
                    self.id,
                    num(sh.dx),
                    num(sh.dy),
                    num(sh.std_dev),
                    num(c.a),
                    c.r,
                    c.g,
                    c.b,
                );
                continue;
            }
            // An `feDropShadow` works in linear RGB: its colour converted.
            let unit = |v: u8| {
                let v = f64::from(v) / 255.0;
                num(match sh.linear_rgb {
                    false => v,
                    true if v <= 0.04045 => v / 12.92,
                    true => ((v + 0.055) / 1.055).powf(2.4),
                })
            };
            let space = if sh.linear_rgb { "linearRGB" } else { "sRGB" };
            let _ = write!(
                out,
                "<filter id=\"{}-shadow-{k}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" color-interpolation-filters=\"{space}\"><feColorMatrix in=\"SourceAlpha\" type=\"matrix\" values=\"0 0 0 0 {} 0 0 0 0 {} 0 0 0 0 {} 0 0 0 {} 0\"/><feGaussianBlur stdDeviation=\"{}\"/><feOffset dx=\"{}\" dy=\"{}\"/><feMerge><feMergeNode/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>",
                self.id,
                num(r.x - m),
                num(r.y - m),
                num(r.w + 2.0 * m),
                num(r.h + 2.0 * m),
                unit(c.r),
                unit(c.g),
                unit(c.b),
                num(c.a),
                num(sh.std_dev),
                num(sh.dx),
                num(sh.dy),
            );
        }
        out.push_str("</defs>");
        out
    }
}

fn paint_attrs(p: &Paint, stroke_url: Option<String>) -> String {
    let stroke = stroke_url.unwrap_or_else(|| esc(p.stroke.css()));
    let mut s = format!(" fill=\"{}\" stroke=\"{stroke}\"", esc(p.fill.css()));
    if !p.stroke.is_none() {
        let _ = write!(s, " stroke-width=\"{}\"", num(p.stroke_width));
    }
    if let Some(d) = &p.dash {
        let _ = write!(
            s,
            " stroke-dasharray=\"{}\"",
            d.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
        );
    }
    if let Some(o) = p.fill_opacity {
        let _ = write!(s, " fill-opacity=\"{}\"", num(o));
    }
    if let Some(o) = p.opacity {
        let _ = write!(s, " opacity=\"{}\"", num(o));
    }
    s
}

fn class_attr(c: &Option<String>) -> String {
    c.as_ref()
        .map(|c| format!(" class=\"{}\"", esc(c)))
        .unwrap_or_default()
}

/// A text's inline `font-family` (a style, so that it beats the stylesheet's
/// `text` rule).
fn family_attr(t: &Text) -> String {
    t.family
        .map(|f| format!(" style=\"font-family:{}\"", esc(f)))
        .unwrap_or_default()
}

fn write_text(out: &mut String, t: &Text, class: &Option<String>, raster: Option<f64>) {
    if t.lines.iter().any(|l| crate::icons::has_icons(l)) {
        return write_text_with_icons(out, t, class);
    }
    let anchor = match t.anchor {
        Anchor::Start => "start",
        Anchor::Middle => "middle",
        Anchor::End => "end",
    };
    if t.tspan_central && t.lines.len() == 1 {
        let central =
            t.y - crate::text::baseline_shift(crate::text::Baseline::Central, t.font_size);
        match raster {
            // At other scales Chromium's snapping is not modelled: the layout's.
            Some(k) if (k - 1.0).abs() < 1e-9 => {
                let y = crate::text::painted_tspan_central(central, t.font_size);
                return write_text(
                    out,
                    &Text {
                        y,
                        tspan_central: false,
                        dy_tspans: false,
                        family: None,
                        ..t.clone()
                    },
                    class,
                    None,
                );
            }
            Some(_) => {
                return write_text(
                    out,
                    &Text {
                        tspan_central: false,
                        dy_tspans: false,
                        family: None,
                        ..t.clone()
                    },
                    class,
                    None,
                );
            }
            None => {}
        }
        let _ = write!(
            out,
            "<text{}{} x=\"{}\" y=\"{}\" dominant-baseline=\"central\" alignment-baseline=\"central\" text-anchor=\"{anchor}\" font-size=\"{}\" fill=\"{}\"{}><tspan x=\"{}\" dy=\"0\">",
            class_attr(class),
            family_attr(t),
            num(t.x),
            num(central),
            num(t.font_size),
            esc(t.fill.css()),
            weight_attr(t),
            num(t.x),
        );
        write_line(out, &t.lines[0], t);
        out.push_str("</tspan></text>");
        return;
    }
    if t.dy_tspans && raster.is_none() {
        let _ = write!(
            out,
            "<text{}{} x=\"{}\" y=\"{}\" dy=\"1em\" alignment-baseline=\"middle\" dominant-baseline=\"middle\" text-anchor=\"{anchor}\" font-size=\"{}\" fill=\"{}\"{}>",
            class_attr(class),
            family_attr(t),
            num(t.x),
            num(t.y),
            num(t.font_size),
            esc(t.fill.css()),
            weight_attr(t)
        );
        let step = num(t.line_height / t.font_size);
        for (i, line) in t.lines.iter().enumerate() {
            let dy = if i == 0 { "1" } else { &step };
            let _ = write!(out, "<tspan x=\"{}\" dy=\"{dy}em\">", num(t.x));
            write_line(out, line, t);
            out.push_str("</tspan>");
        }
        out.push_str("</text>");
        return;
    }
    if t.dy_tspans {
        // Rasterising: each line at the baseline Chromium computes for it.
        use crate::diagram::xychart::js_num;
        let _ = write!(
            out,
            "<text{}{} x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" font-size=\"{}\" fill=\"{}\"{}>",
            class_attr(class),
            family_attr(t),
            num(t.x),
            js_num(t.line_y(0)),
            num(t.font_size),
            esc(t.fill.css()),
            weight_attr(t)
        );
        for (i, line) in t.lines.iter().enumerate() {
            let _ = write!(
                out,
                "<tspan x=\"{}\" y=\"{}\">",
                num(t.x),
                js_num(t.line_y(i))
            );
            write_line(out, line, t);
            out.push_str("</tspan>");
        }
        out.push_str("</text>");
        return;
    }
    let _ = write!(
        out,
        "<text{}{} x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" font-size=\"{}\" fill=\"{}\"{}>",
        class_attr(class),
        family_attr(t),
        num(t.x),
        num(t.y),
        num(t.font_size),
        esc(t.fill.css()),
        weight_attr(t)
    );
    if t.lines.len() == 1 {
        write_line(out, &t.lines[0], t);
    } else {
        for (i, line) in t.lines.iter().enumerate() {
            let _ = write!(
                out,
                "<tspan x=\"{}\" y=\"{}\">",
                num(t.x),
                num(t.y + i as f64 * t.line_height),
            );
            write_line(out, line, t);
            out.push_str("</tspan>");
        }
    }
    out.push_str("</text>");
}

/// Text with Font Awesome icons ([`crate::icons`]): each line's text runs as
/// start-anchored `<text>` and each icon as a filled `<path>`, at the offsets the
/// layout measured, grouped in one `<g>`.
fn write_text_with_icons(out: &mut String, t: &Text, class: &Option<String>) {
    use crate::icons::Piece;
    let _ = write!(out, "<g{}>", class_attr(class));
    let weight = weight_attr(t);
    for (i, line) in t.lines.iter().enumerate() {
        let y = t.y + i as f64 * t.line_height;
        let w = crate::text::line_width(line, t.font_size, t.bold);
        let x0 = match t.anchor {
            Anchor::Start => t.x,
            Anchor::Middle => t.x - w / 2.0,
            Anchor::End => t.x - w,
        };
        for piece in crate::icons::line_pieces(line, t.font_size, t.bold) {
            match piece {
                Piece::Text { x, text } => {
                    if crate::text::plain(&text).is_empty() {
                        continue;
                    }
                    let _ = write!(
                        out,
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\"{weight} xml:space=\"preserve\">",
                        num(x0 + x),
                        num(y),
                        num(t.font_size),
                        esc(t.fill.css()),
                    );
                    write_line(out, &text, t);
                    out.push_str("</text>");
                }
                Piece::Icon { x, icon } => {
                    let d = crate::icons::outline(icon);
                    if d.is_empty() {
                        continue;
                    }
                    let (tx, ty, s) = crate::icons::placement(icon, x0 + x, y, t.font_size);
                    let _ = write!(
                        out,
                        "<path class=\"fa-icon\" transform=\"translate({} {}) scale({})\" d=\"{}\" fill=\"{}\"/>",
                        num(tx),
                        num(ty),
                        fmt_scale(s),
                        path_data(&d),
                        esc(t.fill.css()),
                    );
                }
            }
        }
    }
    out.push_str("</g>");
}

/// A scale factor with enough digits for a 512-unit glyph to land within 0.001px.
fn fmt_scale(s: f64) -> String {
    let s = format!("{s:.7}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A text's `font-weight` attribute: its own CSS weight, else bold or none.
fn weight_attr(t: &Text) -> String {
    match t.weight {
        Some(w) => format!(" font-weight=\"{w}\""),
        None if t.bold => " font-weight=\"bold\"".into(),
        None => String::new(),
    }
}

/// CSS `bolder`: the weight of a `<strong>` inside text of weight `w`.
fn bolder(w: u16) -> u16 {
    match w {
        0..350 => 400,
        350..550 => 700,
        _ => 900,
    }
}

/// One display line: plain text, or one `<tspan>` per run of markdown emphasis.
/// A run marked bold inside bold text with its own CSS weight ([`Text::weight`],
/// an HTML label's) is that weight made `bolder`, as `<strong>` is.
fn write_line(out: &mut String, line: &str, t: &Text) {
    if !crate::text::has_marks(line) {
        out.push_str(&esc(line));
        return;
    }
    let bold = t.bold;
    let word_runs = line.contains(crate::text::mark::RUN);
    // Only text with its own CSS weight tells a marked run inside bold text
    // apart; other text takes its runs merged with its weight.
    let base = bold && t.weight.is_none();
    for run in crate::text::styled_runs(line, base) {
        let strong = (run.bold && bold).then_some(t.weight).flatten();
        if (run.bold || bold) == bold && !run.italic && !word_runs && strong.is_none() {
            out.push_str(&esc(&run.text));
            continue;
        }
        out.push_str("<tspan");
        // Runs are never lighter than the text's own weight.
        if let Some(w) = strong {
            let _ = write!(out, " font-weight=\"{}\"", bolder(w));
        } else if run.bold && !bold {
            out.push_str(" font-weight=\"bold\"");
        }
        if run.italic {
            out.push_str(" font-style=\"italic\"");
        }
        let _ = write!(out, ">{}</tspan>", esc(&run.text));
    }
}

fn write_element(out: &mut String, el: &Element, fx: &Filters) {
    match el {
        Element::Group {
            id,
            class,
            children,
        } => {
            if children.is_empty() {
                return;
            }
            out.push_str("<g");
            if let Some(id) = id {
                let _ = write!(out, " id=\"{}\"", esc(id));
            }
            out.push_str(&class_attr(class));
            out.push('>');
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
        Element::Shape { geom, paint, class } => {
            let attrs = paint_attrs(paint, fx.stroke_url(paint)) + &fx.attr(paint);
            let cls = class_attr(class);
            match geom {
                Geometry::Rect { rect, rx } => {
                    let _ = write!(
                        out,
                        "<rect{cls} x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
                        num(rect.x),
                        num(rect.y),
                        num(rect.w),
                        num(rect.h)
                    );
                    if *rx > 0.0 {
                        let _ = write!(out, " rx=\"{0}\" ry=\"{0}\"", num(*rx));
                    }
                    let _ = write!(out, "{attrs}/>");
                }
                Geometry::Circle { c, r } => {
                    let _ = write!(
                        out,
                        "<circle{cls} cx=\"{}\" cy=\"{}\" r=\"{}\"{attrs}/>",
                        num(c.x),
                        num(c.y),
                        num(*r)
                    );
                }
                Geometry::Path(cmds) => {
                    let _ = write!(out, "<path{cls} d=\"{}\"{attrs}/>", path_data(cmds));
                }
                Geometry::Line { a, b } => {
                    let _ = write!(
                        out,
                        "<line{cls} x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{attrs}/>",
                        num(a.x),
                        num(a.y),
                        num(b.x),
                        num(b.y)
                    );
                }
            }
        }
        Element::Text { text, class } | Element::OutlinedText { text, class, .. } => {
            write_text(out, text, class, fx.raster_scale)
        }
        Element::Image { rect, href, inline } => {
            // A rasteriser draws the fetched bytes of a remote image.
            let href = match (fx.raster_scale, inline) {
                (Some(_), Some(data)) => data,
                _ => href,
            };
            let _ = write!(
                out,
                "<image href=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"/>",
                esc(href),
                num(rect.x),
                num(rect.y),
                num(rect.w),
                num(rect.h)
            );
        }
        Element::Icon { rect, color, svg } => {
            let style = color
                .as_ref()
                .map(|c| format!(" style=\"color: {};\"", esc(c.css())))
                .unwrap_or_default();
            let _ = write!(
                out,
                "<g transform=\"translate({},{})\"{style}><g>{svg}</g></g>",
                num(rect.x),
                num(rect.y)
            );
        }
        Element::Brightened {
            factor,
            class,
            children,
        } => {
            if children.is_empty() {
                return;
            }
            let _ = write!(
                out,
                "<g class=\"{}\"{}>",
                esc(class),
                fx.brightness_attr(class, *factor)
            );
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
        Element::Shadowed { shadow, children } => {
            let _ = write!(out, "<g{}>", fx.filter_attr(shadow));
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
        Element::Faded {
            opacity,
            class,
            children,
        } => {
            if children.is_empty() {
                return;
            }
            let cls = class
                .as_ref()
                .map(|c| format!(" class=\"{}\"", esc(c)))
                .unwrap_or_default();
            let _ = write!(out, "<g{cls} opacity=\"{}\">", num(*opacity));
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
        Element::Clipped { rect, children } => {
            // A nested viewport over the same user coordinates clips its content.
            let (x, y, w, h) = (num(rect.x), num(rect.y), num(rect.w), num(rect.h));
            let _ = write!(
                out,
                "<svg x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {w} {h}\">"
            );
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</svg>");
        }
        Element::Rotated {
            deg,
            about,
            children,
        } => {
            if children.is_empty() {
                return;
            }
            let _ = write!(
                out,
                "<g transform=\"rotate({} {} {})\">",
                num(*deg),
                num(about.x),
                num(about.y)
            );
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
        Element::Translated { dx, dy, children } => {
            if children.is_empty() {
                return;
            }
            // In full, as mermaid writes it: the browser reads the offset
            // and the local coordinates in single precision each.
            use crate::diagram::xychart::js_num;
            let _ = write!(
                out,
                "<g transform=\"translate({}, {})\">",
                js_num(*dx),
                js_num(*dy)
            );
            for c in children {
                write_element(out, c, fx);
            }
            out.push_str("</g>");
        }
    }
}

/// Serialise a scene to an SVG document string.
pub fn to_svg(scene: &Scene, opt: &SvgOptions) -> String {
    let vb = scene.view_box;
    let id = esc(opt.id);
    let mut out = String::with_capacity(4096 + scene.element_count() * 96);
    let _ = write!(out, "<svg id=\"{id}\"");
    // journeyRenderer.ts sets `preserveAspectRatio="xMinYMin meet"` and a height
    // 25px over the viewBox's after configureSvgSize (mmdc drops the height only
    // with `--size`). Not for the rasterisers, which size the image themselves.
    let journey_height =
        (scene.kind == DiagramKind::Journey && opt.raster_scale.is_none()).then_some(vb.h + 25.0);
    let journey_par = if journey_height.is_some() {
        " preserveAspectRatio=\"xMinYMin meet\""
    } else {
        ""
    };
    match opt.sizing {
        SvgSizing::MaxWidth => {
            let _ = write!(
                out,
                " width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"max-width: {}px; background-color: {};\"{journey_par}",
                scene.kind.svg_class(),
                num(vb.w),
                esc(opt.background)
            );
            if let Some(h) = journey_height {
                let _ = write!(out, " height=\"{}\"", num(h));
            }
        }
        SvgSizing::Size(s) => {
            let _ = write!(
                out,
                " width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"max-width: {s}px; background-color: {}; max-height: {s}px;\"{journey_par}",
                scene.kind.svg_class(),
                esc(opt.background)
            );
        }
        SvgSizing::Fixed { width, height } => {
            let _ = write!(
                out,
                " width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"background-color: {};\"{journey_par}",
                num(width),
                num(journey_height.unwrap_or(height)),
                scene.kind.svg_class(),
                esc(opt.background)
            );
        }
    }
    // A timeline's in full, as mermaid writes it (a browser sizes the
    // drawing from it).
    let vb_num = |v: f64| {
        if scene.kind == DiagramKind::Timeline {
            crate::diagram::xychart::js_num(v)
        } else {
            num(v)
        }
    };
    let _ = write!(
        out,
        " viewBox=\"{} {} {} {}\" role=\"graphics-document document\" aria-roledescription=\"{}\"",
        vb_num(vb.x),
        vb_num(vb.y),
        vb_num(vb.w),
        vb_num(vb.h),
        scene.kind.role()
    );
    if scene.acc_title.is_some() {
        let _ = write!(out, " aria-labelledby=\"chart-title-{id}\"");
    }
    if scene.acc_descr.is_some() {
        let _ = write!(out, " aria-describedby=\"chart-desc-{id}\"");
    }
    out.push('>');
    if let Some(t) = &scene.acc_title {
        let _ = write!(out, "<title id=\"chart-title-{id}\">{}</title>", esc(t));
    }
    if let Some(d) = &scene.acc_descr {
        let _ = write!(out, "<desc id=\"chart-desc-{id}\">{}</desc>", esc(d));
    }
    let mut fx = Filters {
        id: &id,
        region: scene.view_box,
        shadows: Vec::new(),
        gradients: Vec::new(),
        brightness: Vec::new(),
        raster_scale: opt.raster_scale,
    };
    fx.collect(&scene.elements);
    let _ = write!(
        out,
        "<style>#{id}{{font-family:{};font-size:16px;}}#{id} text{{font-family:{};}}{}{}{}</style>",
        esc(&scene.font_family),
        esc(&scene.font_family),
        esc(&scene.css),
        fx.css(),
        opt.theme_css.map(esc).unwrap_or_default()
    );
    out.push_str(&fx.defs());
    out.push_str("<g>");
    for el in &scene.elements {
        write_element(&mut out, el, &fx);
    }
    out.push_str("</g></svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tspan_central_text_as_mermaid_writes_it_and_as_chromium_paints_it() {
        let t = Text {
            x: 278.0,
            // Central at 36.5, 5.5 above the alphabetic baseline at 16px.
            y: 42.0,
            lines: vec!["Bob".into()],
            line_height: 16.0,
            font_size: 16.0,
            bold: false,
            weight: None,
            anchor: Anchor::Middle,
            fill: crate::scene::Color::new("#28253D"),
            tspan_central: true,
            dy_tspans: false,
            family: None,
        };
        let mut out = String::new();
        write_text(&mut out, &t, &None, None);
        assert_eq!(
            out,
            "<text x=\"278\" y=\"36.5\" dominant-baseline=\"central\" alignment-baseline=\"central\" text-anchor=\"middle\" font-size=\"16\" fill=\"#28253D\"><tspan x=\"278\" dy=\"0\">Bob</tspan></text>"
        );
        // Drawn 1:1 to a raster: where Chromium paints it (mermaid 12.3.0, d09).
        let mut out = String::new();
        write_text(&mut out, &t, &None, Some(1.0));
        assert_eq!(
            out,
            "<text x=\"278\" y=\"43\" text-anchor=\"middle\" font-size=\"16\" fill=\"#28253D\">Bob</text>"
        );
        // At another scale, the layout's baseline.
        let mut out = String::new();
        write_text(&mut out, &t, &None, Some(2.0));
        assert!(out.contains(" y=\"42\" "), "{out}");
    }

    #[test]
    fn number_formatting() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.23456), "1.235");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(-2.5), "-2.5");
    }

    #[test]
    fn escaping() {
        assert_eq!(esc("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
    }

    #[test]
    fn emphasis_marks_become_styled_tspans() {
        let mut out = String::new();
        let plain = Text::middle(
            crate::geom::Point::default(),
            "",
            16.0,
            &crate::scene::Color::new("#000"),
        );
        write_line(
            &mut out,
            &crate::text::label_lines("`a **b** *c<d*`")[0],
            &plain,
        );
        assert_eq!(
            out,
            "a <tspan font-weight=\"bold\">b</tspan> <tspan font-style=\"italic\">c&lt;d</tspan>"
        );
        // Text that is bold already needs no bold tspans.
        let mut out = String::new();
        let bold = plain.clone().bold();
        write_line(&mut out, &crate::text::label_lines("<b>x</b>y")[0], &bold);
        assert_eq!(out, "xy");
        // Unless it is an HTML label's: `<strong>` in bold is `bolder`.
        let html = Text {
            weight: Some(700),
            ..bold
        };
        let mut out = String::new();
        write_line(&mut out, &crate::text::label_lines("<b>x</b>y")[0], &html);
        assert_eq!(out, "<tspan font-weight=\"900\">x</tspan>y");
    }

    #[test]
    fn shadows_become_one_drop_shadow_filter_each() {
        use crate::geom::Rect;
        use crate::scene::Color;
        let sh = Shadow {
            dx: 1.0,
            dy: 2.0,
            std_dev: 1.0,
            color: Color::new("rgba(185, 185, 185, 0.5)"),
            linear_rgb: false,
            css: false,
        };
        let paint = Paint::fill(&Color::new("#fff")).with_shadow(Some(sh));
        let els = vec![
            Element::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, paint.clone()),
            Element::rect(Rect::new(20.0, 0.0, 10.0, 10.0), 0.0, paint),
        ];
        let mut out = String::new();
        let mut fx = Filters {
            id: "d",
            region: Rect::new(0.0, 0.0, 30.0, 10.0),
            shadows: Vec::new(),
            gradients: Vec::new(),
            brightness: Vec::new(),
            raster_scale: None,
        };
        fx.collect(&els);
        for e in &els {
            write_element(&mut out, e, &fx);
        }
        assert_eq!(out.matches("filter=\"url(#d-shadow-0)\"").count(), 2);
        assert_eq!(
            fx.defs(),
            "<defs><filter id=\"d-shadow-0\" filterUnits=\"userSpaceOnUse\" x=\"-30\" y=\"-30\" \
             width=\"90\" height=\"70\" color-interpolation-filters=\"sRGB\"><feColorMatrix \
             in=\"SourceAlpha\" type=\"matrix\" values=\"0 0 0 0 0.725 0 0 0 0 0.725 0 0 0 0 0.725 \
             0 0 0 0.5 0\"/><feGaussianBlur stdDeviation=\"1\"/><feOffset dx=\"1\" dy=\"2\"/>\
             <feMerge><feMergeNode/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter></defs>"
        );
    }

    /// A scene with one timeline-style event: a box and its text in a group
    /// brightened by 120%.
    fn brightened_scene(fill: &str) -> Scene {
        use crate::geom::{Point, Rect};
        use crate::scene::Color;
        let text = Text::centered(
            Point::new(20.0, 14.0),
            &["Hi".to_string()],
            16.0,
            17.6,
            &Color::new("#ffffff"),
        );
        Scene {
            kind: DiagramKind::Timeline,
            view_box: Rect::new(0.0, 0.0, 40.0, 20.0),
            elements: vec![Element::Brightened {
                factor: 1.2,
                class: "eventWrapper".into(),
                children: vec![
                    Element::rect(
                        Rect::new(0.0, 0.0, 40.0, 20.0),
                        0.0,
                        Paint::fill(&Color::new(fill)),
                    ),
                    Element::classed_text(text, "eventText"),
                ],
            }],
            acc_title: None,
            acc_descr: None,
            font_family: "sans-serif".into(),
            css: String::new(),
        }
    }

    fn opts(raster_scale: Option<f64>) -> SvgOptions<'static> {
        SvgOptions {
            id: "my-svg",
            background: "white",
            sizing: SvgSizing::Fixed {
                width: 40.0,
                height: 20.0,
            },
            theme_css: None,
            raster_scale,
        }
    }

    #[test]
    fn a_brightened_group_is_mermaids_class_and_stylesheet_rule() {
        // timeline/styles.js: `.eventWrapper { filter: brightness(120%) }`, the
        // colours inside as they are.
        let out = to_svg(&brightened_scene("#868686"), &opts(None));
        assert!(
            out.contains("#my-svg .eventWrapper{filter:brightness(120%);}</style>"),
            "{out}"
        );
        assert!(
            out.contains("<g><g class=\"eventWrapper\"><rect x=\"0\" y=\"0\" width=\"40\" height=\"20\" fill=\"#868686\""),
            "{out}"
        );
        assert!(!out.contains("<filter"), "{out}");
    }

    #[test]
    fn a_rasterised_brightened_group_refers_to_an_srgb_transfer_filter() {
        // For resvg: the filter element CSS `brightness()` stands for, in sRGB,
        // with the half step that makes resvg round as Chromium does; no rule.
        let out = to_svg(&brightened_scene("#868686"), &opts(Some(1.0)));
        assert!(!out.contains("brightness("), "{out}");
        assert!(
            out.contains(
                "<defs><filter id=\"my-svg-brightness-0\" color-interpolation-filters=\"sRGB\">\
                 <feComponentTransfer><feFuncR type=\"linear\" slope=\"1.2\" intercept=\"0.0019607843\"/>\
                 <feFuncG type=\"linear\" slope=\"1.2\" intercept=\"0.0019607843\"/>\
                 <feFuncB type=\"linear\" slope=\"1.2\" intercept=\"0.0019607843\"/>\
                 </feComponentTransfer></filter></defs>"
            ),
            "{out}"
        );
        assert!(
            out.contains("<g class=\"eventWrapper\" filter=\"url(#my-svg-brightness-0)\">"),
            "{out}"
        );
    }

    /// Chromium's `brightness(120%)` is `min(255, round(1.2 · v))` on every
    /// pixel of the group's layer, antialiased text edges included (measured
    /// on mermaid's timelines). resvg's own filter truncates instead; with the
    /// half-step intercept it rounds: here #868686 (134) becomes 161, not 160.
    #[cfg(feature = "png")]
    #[test]
    fn rasterised_brightness_rounds_each_pixel_of_the_layer_as_chromium_does() {
        use resvg::tiny_skia::Pixmap;
        let rast = crate::output::png::Rasterizer::bundled_only();
        let draw = |scene: &Scene| {
            let svg = to_svg(scene, &opts(Some(1.0)));
            let png = rast
                .png_scaled(&svg, 40, 20, Some(1.0), Some(crate::color::Rgba::WHITE))
                .unwrap();
            Pixmap::decode_png(&png).unwrap().data().to_vec()
        };
        let filtered = brightened_scene("#868686");
        let Element::Brightened { children, .. } = &filtered.elements[0] else {
            panic!()
        };
        let plain = Scene {
            elements: vec![Element::group("eventWrapper", children.clone())],
            ..filtered.clone()
        };
        let (got, under) = (draw(&filtered), draw(&plain));
        assert_eq!(&got[..4], &[161, 161, 161, 255]);
        for (k, (g, u)) in got.iter().zip(&under).enumerate() {
            let want = if k % 4 == 3 {
                *u
            } else {
                (*u as f64 * 1.2).round().min(255.0) as u8
            };
            assert_eq!(*g, want, "byte {k}");
        }
    }
}
