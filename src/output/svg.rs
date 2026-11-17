//! `Scene` → SVG text, shaped like mermaid's output (`<svg id="my-svg" ...>`).

use crate::geom::PathCmd;
use crate::scene::{Anchor, Element, Geometry, Gradient, Paint, Scene, Shadow, Text};
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

/// The drop-shadow filters and stroke gradients of a document: its id, the
/// distinct shadows (`filter` ids `{id}-shadow-{index}`) and gradients
/// (`{id}-gradient-{index}`).
struct Filters<'a> {
    id: &'a str,
    shadows: Vec<Shadow>,
    gradients: Vec<Gradient>,
}

impl Filters<'_> {
    fn collect(&mut self, els: &[Element]) {
        for el in els {
            match el {
                Element::Group { children, .. }
                | Element::Rotated { children, .. }
                | Element::Clipped { children, .. } => {
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
                Element::Text { .. } | Element::OutlinedText { .. } => {}
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
            .and_then(|sh| self.shadows.iter().position(|s| s == sh))
            .map(|k| format!(" filter=\"url(#{}-shadow-{k})\"", self.id))
            .unwrap_or_default()
    }

    /// `<defs>` with a `feDropShadow` per shadow (the flood split into an
    /// opaque colour and its opacity, which every renderer reads).
    fn defs(&self) -> String {
        if self.shadows.is_empty() && self.gradients.is_empty() {
            return String::new();
        }
        let mut out = String::from("<defs>");
        for (k, g) in self.gradients.iter().enumerate() {
            let _ = write!(
                out,
                "<linearGradient id=\"{}-gradient-{k}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" x2=\"{}\" y1=\"0\" y2=\"0\"><stop offset=\"0%\" stop-color=\"{}\"/><stop offset=\"100%\" stop-color=\"{}\"/></linearGradient>",
                self.id,
                num(g.x1),
                num(g.x2),
                esc(g.from.css()),
                esc(g.to.css())
            );
        }
        for (k, sh) in self.shadows.iter().enumerate() {
            let (color, alpha) = match crate::color::parse(sh.color.css()) {
                Some(c) => (format!("rgb({},{},{})", c.r, c.g, c.b), c.a),
                None => (sh.color.css().to_string(), 1.0),
            };
            let _ = write!(
                out,
                "<filter id=\"{}-shadow-{k}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\"><feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-color=\"{}\" flood-opacity=\"{}\"/></filter>",
                self.id,
                num(sh.dx),
                num(sh.dy),
                num(sh.std_dev),
                esc(&color),
                num(alpha)
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

fn write_text(out: &mut String, t: &Text, class: &Option<String>) {
    if t.lines.iter().any(|l| crate::icons::has_icons(l)) {
        return write_text_with_icons(out, t, class);
    }
    let anchor = match t.anchor {
        Anchor::Start => "start",
        Anchor::Middle => "middle",
        Anchor::End => "end",
    };
    let _ = write!(
        out,
        "<text{} x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" font-size=\"{}\" fill=\"{}\"{}>",
        class_attr(class),
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
    for run in crate::text::styled_runs(line, false) {
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
            write_text(out, text, class)
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
    }
}

/// Serialise a scene to an SVG document string.
pub fn to_svg(scene: &Scene, opt: &SvgOptions) -> String {
    let vb = scene.view_box;
    let id = esc(opt.id);
    let mut out = String::with_capacity(4096 + scene.element_count() * 96);
    let _ = write!(out, "<svg id=\"{id}\"");
    match opt.sizing {
        SvgSizing::MaxWidth => {
            let _ = write!(
                out,
                " width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"max-width: {}px; background-color: {};\"",
                scene.kind.svg_class(),
                num(vb.w),
                esc(opt.background)
            );
        }
        SvgSizing::Size(s) => {
            let _ = write!(
                out,
                " width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"max-width: {s}px; background-color: {}; max-height: {s}px;\"",
                scene.kind.svg_class(),
                esc(opt.background)
            );
        }
        SvgSizing::Fixed { width, height } => {
            let _ = write!(
                out,
                " width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" class=\"{}\" style=\"background-color: {};\"",
                num(width),
                num(height),
                scene.kind.svg_class(),
                esc(opt.background)
            );
        }
    }
    let _ = write!(
        out,
        " viewBox=\"{} {} {} {}\" role=\"graphics-document document\" aria-roledescription=\"{}\"",
        num(vb.x),
        num(vb.y),
        num(vb.w),
        num(vb.h),
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
    let _ = write!(
        out,
        "<style>#{id}{{font-family:{};font-size:16px;}}#{id} text{{font-family:{};}}{}{}</style>",
        esc(&scene.font_family),
        esc(&scene.font_family),
        esc(&scene.css),
        opt.theme_css.map(esc).unwrap_or_default()
    );
    let mut fx = Filters {
        id: &id,
        shadows: Vec::new(),
        gradients: Vec::new(),
    };
    fx.collect(&scene.elements);
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
        };
        let paint = Paint::fill(&Color::new("#fff")).with_shadow(Some(sh));
        let els = vec![
            Element::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, paint.clone()),
            Element::rect(Rect::new(20.0, 0.0, 10.0, 10.0), 0.0, paint),
        ];
        let mut out = String::new();
        let mut fx = Filters {
            id: "d",
            shadows: Vec::new(),
            gradients: Vec::new(),
        };
        fx.collect(&els);
        for e in &els {
            write_element(&mut out, e, &fx);
        }
        assert_eq!(out.matches("filter=\"url(#d-shadow-0)\"").count(), 2);
        assert_eq!(
            fx.defs(),
            "<defs><filter id=\"d-shadow-0\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">\
             <feDropShadow dx=\"1\" dy=\"2\" stdDeviation=\"1\" flood-color=\"rgb(185,185,185)\" \
             flood-opacity=\"0.5\"/></filter></defs>"
        );
    }
}
