//! `Scene` → SVG text, shaped like mermaid's output (`<svg id="my-svg" ...>`).

use crate::geom::PathCmd;
use crate::scene::{Anchor, Element, Geometry, Paint, Scene, Text};
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
    let mut d = String::new();
    for c in cmds {
        match c {
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
        }
    }
    d
}

fn paint_attrs(p: &Paint) -> String {
    let mut s = format!(
        " fill=\"{}\" stroke=\"{}\"",
        esc(p.fill.css()),
        esc(p.stroke.css())
    );
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
        if t.bold { " font-weight=\"bold\"" } else { "" }
    );
    if t.lines.len() == 1 {
        out.push_str(&esc(&t.lines[0]));
    } else {
        for (i, line) in t.lines.iter().enumerate() {
            let _ = write!(
                out,
                "<tspan x=\"{}\" y=\"{}\">{}</tspan>",
                num(t.x),
                num(t.y + i as f64 * t.line_height),
                esc(line)
            );
        }
    }
    out.push_str("</text>");
}

fn write_element(out: &mut String, el: &Element) {
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
                write_element(out, c);
            }
            out.push_str("</g>");
        }
        Element::Shape { geom, paint, class } => {
            let attrs = paint_attrs(paint);
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
            }
        }
        Element::Text { text, class } => write_text(out, text, class),
        Element::Rotated {
            degrees,
            origin,
            children,
        } => {
            let _ = write!(
                out,
                "<g transform=\"rotate({} {} {})\">",
                num(*degrees),
                num(origin.x),
                num(origin.y)
            );
            for c in children {
                write_element(out, c);
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
    out.push_str("<g>");
    for el in &scene.elements {
        write_element(&mut out, el);
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
}
