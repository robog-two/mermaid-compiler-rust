//! `Scene` → SVG text, shaped like mermaid's output (`<svg id="my-svg" ...>`).

use crate::geom::{PathCmd, Rect};
use crate::scene::{Anchor, DiagramKind, Element, Geometry, Gradient, Paint, Scene, Shadow, Text};
use std::fmt::Write as _;

/// In a [`Element::Markup`] string: what comes before it belongs ahead of the
/// root's `<style>` (a drawing that lowers an element to be the root's first
/// child), what follows it where the markup normally goes.
pub const BEFORE_STYLE: &str = "\u{1}BEFORE_STYLE\u{1}";

/// How the root `<svg>` element is sized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SvgSizing {
    /// mermaid's `useMaxWidth: true`: `width="100%"` + `max-width` style.
    MaxWidth,
    /// mmdc `--size`: `max-width`/`max-height` set to the given pixel size.
    Size(u32),
    /// `--size` on a `useMaxWidth: false` diagram: mermaid's fixed `width` stays.
    SizeFixed { size: u32, width: f64 },
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
    /// mermaid's own stylesheet for the diagram. Only the SVG file carries it:
    /// the PNG and PDF paths leave it out and keep the hand-written rules their
    /// output was built against.
    pub style: Option<&'a crate::theme::styles::StyleSpec>,
}

thread_local! {
    /// Whether [`num`] prints numbers as JavaScript does. Only the SVG file
    /// path turns it on; the raster and PDF paths keep the rounded form their
    /// output was built against.
    static JS_NUMBERS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with [`num`] printing numbers the way mermaid's SVG does (`js_num`).
pub fn with_js_numbers<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            JS_NUMBERS.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(JS_NUMBERS.with(|c| c.replace(true)));
    f()
}

/// Number formatting for SVG text: JavaScript's within [`with_js_numbers`],
/// else compact and deterministic (three decimals).
pub fn num(v: f64) -> String {
    if JS_NUMBERS.with(|c| c.get()) {
        return js_num(v);
    }
    let v = if v.abs() < 5e-4 { 0.0 } else { v };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// `Number.prototype.toString` (ECMA-262 Number::toString, radix 10): the
/// shortest digits that read back as `v`, laid out without an exponent for
/// 1e-7 < |v| < 1e21.
pub fn js_num(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let a = v.abs();
    // Shortest round-trip digits. Where two shortest strings are equally near
    // the exact value, JavaScript takes the even one, which is what the
    // correctly rounded form with as many digits is.
    let e = format!("{a:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let mut digits: String = mant.chars().filter(|c| *c != '.').collect();
    let even = format!("{:.*e}", digits.len() - 1, a);
    if even.parse::<f64>() == Ok(a)
        && let Some((m2, e2)) = even.split_once('e')
        && e2 == exp
    {
        digits = m2.chars().filter(|c| *c != '.').collect();
    }
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let es = if e < 0 {
            format!("-{}", -e)
        } else {
            format!("+{e}")
        };
        if k == 1 {
            format!("{digits}e{es}")
        } else {
            format!("{}.{}e{es}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// The root `max-width` as mmdc writes it: the drawing's width to six
/// significant digits (`Number(w.toPrecision(6))`), shortest form.
fn max_width(w: f64) -> String {
    let six = format!("{w:.5e}").parse::<f64>().unwrap_or(w);
    crate::diagram::xychart::js_num(six)
}

/// XML-escape text content and attribute values.
/// Character data as `XMLSerializer` writes it: only `&`, `<` and `>` escaped.
pub fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

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
    /// Rasterising: how many per-shape shadow filters have been written.
    local: std::cell::Cell<usize>,
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
                // Plain SVG writes the markup's own string, with no shared
                // filters or gradients.
                Element::Markup { children, .. } => {
                    if self.raster_scale.is_some() {
                        self.collect(children);
                    }
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

    /// The `filter` attribute of a shape casting `sh` when rasterising, its
    /// filter written here over the shape's own box (grown by its stroke, the
    /// blur and the offset) rather than the whole drawing: a renderer
    /// allocates and blurs a filter's whole region for every shape using it.
    /// Where the shape has no box, the shared filter.
    fn local_shadow(&self, out: &mut String, sh: &Shadow, geom: &Geometry, p: &Paint) -> String {
        let Some(bb) = geom.bbox() else {
            return self.filter_attr(sh);
        };
        let grow = 2.0 * p.stroke_width + 4.0 * sh.std_dev + sh.dx.abs().max(sh.dy.abs()) + 2.0;
        let k = self.local.get();
        self.local.set(k + 1);
        let id = format!("{}-shape-shadow-{k}", self.id);
        out.push_str("<defs>");
        write_shadow_filter(
            out,
            &id,
            sh,
            Rect::new(
                bb.x - grow,
                bb.y - grow,
                bb.w + 2.0 * grow,
                bb.h + 2.0 * grow,
            ),
        );
        out.push_str("</defs>");
        format!(" filter=\"url(#{id})\"")
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
                // As mermaid writes it: insertLookDefs' region is 130% wide
                // and tall for the 4px shadow, 150% for the small 2px one.
                let size = if sh.dx == 2.0 { 150 } else { 130 };
                let _ = write!(
                    out,
                    "<filter id=\"{}-shadow-{k}\" height=\"{size}%\" width=\"{size}%\"><feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-opacity=\"{}\" flood-color=\"#{:02X}{:02X}{:02X}\"/></filter>",
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
            write_shadow_filter(
                &mut out,
                &format!("{}-shadow-{k}", self.id),
                sh,
                Rect::new(r.x - m, r.y - m, r.w + 2.0 * m, r.h + 2.0 * m),
            );
        }
        out.push_str("</defs>");
        out
    }
}

/// A shadow's filter, written as its parts: the shadow colour from the alpha
/// (a colour matrix; an `feDropShadow` works in linear RGB, its colour
/// converted), blurred, offset, under the source, over `region`.
fn write_shadow_filter(out: &mut String, id: &str, sh: &Shadow, region: Rect) {
    let c = crate::color::parse(sh.color.css()).unwrap_or(crate::color::Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 1.0,
    });
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
        "<filter id=\"{id}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" color-interpolation-filters=\"{space}\"><feColorMatrix in=\"SourceAlpha\" type=\"matrix\" values=\"0 0 0 0 {} 0 0 0 0 {} 0 0 0 0 {} 0 0 0 {} 0\"/><feGaussianBlur stdDeviation=\"{}\"/><feOffset dx=\"{}\" dy=\"{}\"/><feMerge><feMergeNode/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>",
        num(region.x),
        num(region.y),
        num(region.w),
        num(region.h),
        unit(c.r),
        unit(c.g),
        unit(c.b),
        num(c.a),
        num(sh.std_dev),
        num(sh.dx),
        num(sh.dy),
    );
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
        .as_ref()
        .map(|f| format!(" style=\"font-family:{}\"", esc(f)))
        .unwrap_or_default()
}

/// Run `f` measuring in the face `t` is drawn in: its own `font-family`, else
/// the scene's (set around the whole drawing by [`to_svg`]).
fn in_face<R>(t: &Text, f: impl FnOnce() -> R) -> R {
    match &t.family {
        Some(fam) => crate::text::with_family(fam, f),
        None => f(),
    }
}

fn write_text(out: &mut String, t: &Text, class: &Option<String>, raster: Option<f64>) {
    in_face(t, || write_text_in_face(out, t, class, raster));
}

fn write_text_in_face(out: &mut String, t: &Text, class: &Option<String>, raster: Option<f64>) {
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
            let shadow_attr = match &paint.shadow {
                Some(sh) if fx.raster_scale.is_some() => fx.local_shadow(out, sh, geom, paint),
                _ => fx.attr(paint),
            };
            let attrs = paint_attrs(paint, fx.stroke_url(paint)) + &shadow_attr;
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
        Element::Markup { svg, children, .. } => {
            if fx.raster_scale.is_some() {
                children.iter().for_each(|c| write_element(out, c, fx));
            } else {
                out.push_str(&svg.replace(crate::diagram::wardley::ID_MARK, &esc(fx.id)));
            }
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
    crate::text::with_family(&scene.font_family, || to_svg_in_face(scene, opt))
}

fn to_svg_in_face(scene: &Scene, opt: &SvgOptions) -> String {
    let vb = match (opt.raster_scale, scene.elements.as_slice()) {
        (
            None,
            [
                Element::Markup {
                    svg_view_box: Some(v),
                    ..
                },
            ],
        ) => *v,
        _ => scene.view_box,
    };
    let id = esc(opt.id);
    let mut out = String::with_capacity(4096 + scene.element_count() * 96);
    let _ = write!(out, "<svg id=\"{id}\"");
    // journeyRenderer.ts sets `preserveAspectRatio="xMinYMin meet"` and a height
    // 25px over the viewBox's after configureSvgSize (mmdc drops the height only
    // with `--size`). Not for the rasterisers, which size the image themselves.
    let journey_height =
        (scene.kind == DiagramKind::Journey && opt.raster_scale.is_none()).then_some(vb.h + 25.0);
    let kind = scene.kind;
    let xlink = " xmlns:xlink=\"http://www.w3.org/1999/xlink\"";
    let class_attr = |c: Option<&str>| c.map(|c| format!(" class=\"{c}\"")).unwrap_or_default();
    // The viewBox as mermaid writes it: the drawing's bounding box in full.
    // The rasterisers keep the compact form.
    let vb_num = |v: f64| match opt.sizing {
        SvgSizing::Fixed { .. } if kind != DiagramKind::Timeline => num(v),
        SvgSizing::Fixed { .. } => crate::diagram::xychart::js_num(v),
        // ishikawa's box is single-precision numbers plus its padding, summed
        // in doubles: written as they are.
        _ if kind == DiagramKind::Ishikawa => crate::diagram::xychart::js_num(v),
        // A board's box is a float's plus the padding: a double, in full.
        _ if kind == DiagramKind::Kanban && opt.raster_scale.is_none() => {
            crate::diagram::xychart::js_num(v)
        }
        _ => crate::diagram::xychart::js_num(f64::from(v as f32)),
    };
    let viewbox = format!(
        " viewBox=\"{} {} {} {}\"",
        vb_num(vb.x),
        vb_num(vb.y),
        vb_num(vb.w),
        vb_num(vb.h)
    );
    // Attributes mermaid's renderers set after the viewBox, before the role.
    let par = if journey_height.is_some() {
        " preserveAspectRatio=\"xMinYMin meet\""
    } else {
        ""
    };
    let mut tail = String::new();
    if opt.sizing == SvgSizing::MaxWidth {
        tail.push_str(par);
        if let Some(h) = journey_height {
            let _ = write!(tail, " height=\"{}\"", num(h));
        }
    }
    if kind == DiagramKind::Radar {
        tail.push_str(" overflow=\"visible\"");
    }
    let vars = kind.root_style_vars();
    let bg = esc(opt.background);
    match opt.sizing {
        SvgSizing::MaxWidth | SvgSizing::Size(_) => {
            let style = match opt.sizing {
                SvgSizing::Size(s) => format!(
                    " style=\"max-width: {s}px;{vars} background-color: {bg}; max-height: {s}px;\"{par}"
                ),
                _ => format!(
                    " style=\"max-width: {}px;{vars} background-color: {bg};\"",
                    max_width(vb.w)
                ),
            };
            let (class, vb_first, class_last) = kind.root_layout();
            out.push_str(" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\"");
            out.push_str(xlink);
            if !class_last {
                out.push_str(&class_attr(class));
            }
            if vb_first {
                out.push_str(&viewbox);
                out.push_str(&style);
            } else {
                out.push_str(&style);
                out.push_str(&viewbox);
            }
            if class_last {
                out.push_str(&class_attr(class));
            }
            out.push_str(&tail);
        }
        SvgSizing::SizeFixed { size: s, width } => {
            let _ = write!(
                out,
                " width=\"{}\" xmlns=\"http://www.w3.org/2000/svg\"{xlink}{} style=\"{}background-color: {bg}; max-width: {s}px; max-height: {s}px;\"{par}{viewbox}{tail}",
                num(width),
                class_attr(kind.root_layout().0),
                vars.trim_start().to_owned() + if vars.is_empty() { "" } else { " " },
            );
        }
        SvgSizing::Fixed { width, height } => {
            let _ = write!(
                out,
                " width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"background-color: {bg};\"{par}{viewbox}",
                num(width),
                num(journey_height.unwrap_or(height)),
                kind.svg_class(),
            );
        }
    }
    out.push_str(" role=\"graphics-document document\"");
    let _ = write!(out, " aria-roledescription=\"{}\"", kind.role());
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
        local: Default::default(),
    };
    fx.collect(&scene.elements);
    // A drawing whose markup is mermaid's own: no wrapper of ours.
    let own_markup = match (opt.raster_scale, scene.elements.as_slice()) {
        (None, [Element::Markup { svg, .. }]) => {
            Some(svg.replace(crate::diagram::wardley::ID_MARK, &id))
        }
        _ => None,
    };
    let own_markup = own_markup.map(|m| match m.split_once(BEFORE_STYLE) {
        Some((first, rest)) => {
            out.push_str(first);
            rest.to_string()
        }
        None => m,
    });
    let faithful = opt
        .style
        .and_then(|spec| crate::theme::styles::style_text(spec, &id));
    match faithful {
        Some(crate::theme::styles::Sheet::Text(css)) => {
            let _ = write!(out, "<style>{}</style>", esc_text(&css));
        }
        Some(crate::theme::styles::Sheet::Omit) => {}
        None => {
            let _ = write!(
                out,
                "<style>#{id}{{font-family:{};font-size:16px;}}#{id} text{{font-family:{};}}{}{}{}</style>",
                esc(&scene.font_family),
                esc(&scene.font_family),
                esc(&scene.css),
                fx.css(),
                opt.theme_css.map(esc).unwrap_or_default()
            );
        }
    }
    out.push_str(&fx.defs());
    if let Some(markup) = own_markup {
        out.push_str(&markup);
        out.push_str("</svg>");
        return out;
    }
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
    fn js_number_text() {
        for (v, want) in [
            (0.0, "0"),
            (-0.0, "0"),
            (0.75, "0.75"),
            (57.34375, "57.34375"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (-2.5, "-2.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (636.2000122070312, "636.2000122070312"),
            (f64::from(636.2f32), "636.2000122070312"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(js_num(v), want, "{v:e}");
        }
        assert_eq!(with_js_numbers(|| num(0.0001234)), "0.0001234");
        assert_eq!(num(0.0001234), "0");
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
            local: Default::default(),
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
            style: None,
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
                .png_placed(
                    &svg,
                    40,
                    20,
                    (1.0, 0.0, 0.0),
                    Some(crate::color::Rgba::WHITE),
                )
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
