//! The DOM mermaid 12.3.0's block renderer writes, as text: `insertNode`'s
//! shapes in the three looks, the edges with their markers and labels, and the
//! definitions in front of them. The scene the rasterisers draw is built
//! separately (see [`super::draw`]); this is only what plain SVG output writes.

use super::{Block, BlockShape, Dom, Head, Line, NodeGeom};
use crate::config::Look;
use crate::diagram::hand::HandNode;
use crate::diagram::wardley::ID_MARK;
use crate::geom::{PathCmd, Point};
use crate::output::svg::{esc_text, js_num as n};
use crate::rough::{self, Drawable, Options, SetKind};
use crate::text::mark;
use crate::theme::Palette;
use std::fmt::Write;

/// An attribute value as `XMLSerializer` writes it.
pub(super) fn attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\n', "&#10;")
}

/// A length in a `style` attribute: the six significant digits Chromium
/// serialises.
fn css_px(v: f64) -> String {
    let six = format!("{v:.5e}").parse::<f64>().unwrap_or(v);
    n(six)
}

/// A label's lines as the HTML `markdownToHTML` and `innerHTML` leave: lines
/// joined by `<br />`, emphasis marks as `<strong>` / `<em>`.
fn label_html(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str("<br />");
        }
        let mut plain = String::new();
        let flush = |out: &mut String, plain: &mut String| {
            out.push_str(&esc_text(plain));
            plain.clear();
        };
        for c in line.chars() {
            match c {
                mark::BOLD_ON => {
                    flush(&mut out, &mut plain);
                    out.push_str("<strong>");
                }
                mark::BOLD_OFF => {
                    flush(&mut out, &mut plain);
                    out.push_str("</strong>");
                }
                mark::ITALIC_ON => {
                    flush(&mut out, &mut plain);
                    out.push_str("<em>");
                }
                mark::ITALIC_OFF => {
                    flush(&mut out, &mut plain);
                    out.push_str("</em>");
                }
                mark::RUN => {}
                c => plain.push(c),
            }
        }
        flush(&mut out, &mut plain);
    }
    out
}

/// A style attribute's declarations as Chromium re-serialises them once the
/// `display` and `max-width` properties are set through the style object:
/// `prop: value;` each, colours as `rgb()`.
fn cssom(decls: &str) -> String {
    let mut out = String::new();
    for d in decls.split(';') {
        let Some((prop, value)) = d.split_once(':') else {
            continue;
        };
        let (prop, value) = (prop.trim().to_ascii_lowercase(), value.trim());
        let (value, important) = match value.strip_suffix("!important") {
            Some(v) => (v.trim(), true),
            None => (value, false),
        };
        let _ = write!(
            out,
            "{prop}: {}{}; ",
            css_color(value),
            if important { " !important" } else { "" }
        );
    }
    out
}

/// A hex colour as `rgb()`, anything else as written.
fn css_color(v: &str) -> String {
    let Some(h) = v.strip_prefix('#') else {
        return v.to_string();
    };
    let digits: Option<Vec<u32>> = h.chars().map(|c| c.to_digit(16)).collect();
    match digits.as_deref() {
        Some([r, g, b]) => format!("rgb({}, {}, {})", r * 17, g * 17, b * 17),
        Some([r1, r2, g1, g2, b1, b2]) => {
            format!("rgb({}, {}, {})", r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)
        }
        _ => v.to_string(),
    }
}

/// One `createText` HTML label (`addHtmlSpan`): the `<foreignObject>`.
pub(super) struct Label<'a> {
    pub lines: &'a [String],
    pub w: f64,
    pub h: f64,
    /// `max-width` (the width the label was laid out in).
    pub limit: f64,
    pub table: bool,
    /// An edge label's div is a `labelBkg`; its span an `edgeLabel`.
    pub edge: bool,
    /// The node's label styles (`color:…`), on the span as written and on the
    /// div as the style object re-serialises them.
    pub style: &'a str,
}

impl Label<'_> {
    fn foreign_object(&self) -> String {
        let (display, space, tail) = if self.table {
            (
                "table",
                "break-spaces",
                format!(" width: {}px;", css_px(self.limit)),
            )
        } else {
            ("table-cell", "nowrap", String::new())
        };
        let class = if self.edge { " class=\"labelBkg\"" } else { "" };
        let style = format!(
            "{}display: {display}; white-space: {space}; line-height: 1.5; max-width: {}px; text-align: center;{tail}",
            cssom(self.style),
            css_px(self.limit)
        );
        let span_class = if self.edge { "edgeLabel" } else { "nodeLabel" };
        let span_style = if self.style.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", attr(self.style))
        };
        let body = if self.lines.iter().all(|l| l.trim().is_empty()) {
            String::new()
        } else {
            format!("<p>{}</p>", label_html(self.lines))
        };
        format!(
            "<foreignObject width=\"{}\" height=\"{}\"><div xmlns=\"http://www.w3.org/1999/xhtml\"{class} style=\"{}\"><span{span_style} class=\"{span_class}\">{body}</span></div></foreignObject>",
            n(self.w),
            n(self.h),
            attr(&style)
        )
    }
}

/// `styles2String` of a node: its classes' styles, then its own.
#[derive(Debug, Clone, Default)]
pub(super) struct Styles {
    /// `cssCompiledStyles` (the `classDef`s of the node's classes).
    pub compiled: Vec<String>,
    /// `vertex.styles` (`undefined` as an empty list).
    pub own: Vec<String>,
}

fn is_label_style(k: &str) -> bool {
    matches!(
        k,
        "color"
            | "font-size"
            | "font-family"
            | "font-weight"
            | "font-style"
            | "text-decoration"
            | "text-align"
            | "text-transform"
            | "line-height"
            | "letter-spacing"
            | "word-spacing"
            | "text-shadow"
            | "text-overflow"
            | "white-space"
            | "word-wrap"
            | "word-break"
            | "overflow-wrap"
            | "hyphens"
    )
}

impl Styles {
    /// `styles2Map`: later entries replace earlier values and keep their place.
    fn map(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for s in self.compiled.iter().chain(&self.own) {
            let mut it = s.split(':');
            let key = it.next().unwrap_or("").trim().to_string();
            let value = it.next().map(|v| v.trim().to_string()).unwrap_or_default();
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => *v = value,
                None => out.push((key, value)),
            }
        }
        out
    }

    fn get(&self, key: &str) -> Option<String> {
        self.map()
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .filter(|v| !v.is_empty())
    }

    /// `(labelStyles, nodeStyles)`.
    pub(super) fn strings(&self) -> (String, String) {
        let (mut label, mut node) = (Vec::new(), Vec::new());
        for (k, v) in self.map() {
            let item = format!("{k}:{v} !important");
            if is_label_style(&k) {
                label.push(item);
            } else {
                node.push(item);
            }
        }
        (label.join(";"), node.join(";"))
    }

    /// `node.cssStyles` as an attribute: the array's `toString`.
    fn css_attr(&self) -> String {
        self.own.join(",")
    }
}

/// What a node's drawing needs from the diagram.
pub(super) struct Env<'a> {
    pub look: Look,
    pub radius: f64,
    pub pal: &'a Palette,
    pub seed: f64,
}

/// rough.js's options for the node: `userNodeOverrides`, then the classic and
/// neo looks' own (roughness 0, solid fill) where the shape asks for them.
struct RoughColors {
    fill: String,
    stroke: String,
    stroke_width: String,
    dash: String,
}

fn rough_colors(env: &Env, st: &Styles) -> RoughColors {
    RoughColors {
        fill: st
            .get("fill")
            .unwrap_or_else(|| env.pal.main_bkg.css().to_string()),
        stroke: st
            .get("stroke")
            .unwrap_or_else(|| env.pal.node_border.css().to_string()),
        stroke_width: st
            .get("stroke-width")
            .map(|v| v.replace("px", ""))
            .unwrap_or_else(|| "1.3".into()),
        dash: dash_array(st.get("stroke-dasharray").as_deref()),
    }
}

/// `getStrokeDashArray`, joined as `strokeLineDash.join(' ')`.
fn dash_array(v: Option<&str>) -> String {
    let Some(v) = v else {
        return "0 0".into();
    };
    let nums: Vec<f64> = v
        .split_whitespace()
        .map(|x| x.parse::<f64>().unwrap_or(f64::NAN))
        .collect();
    let num = |x: f64| if x.is_nan() { 0.0 } else { x };
    match nums.as_slice() {
        [] => "0 0".into(),
        [a] => format!("{0} {0}", n(num(*a))),
        [a, b, ..] => format!("{} {}", n(num(*a)), n(num(*b))),
    }
}

/// The `<path>`s of one rough.js drawing (`RoughSVG.draw` without its `<g>`).
fn rough_paths(d: &Drawable, c: &RoughColors, path_style: Option<&str>) -> String {
    let style = path_style.map_or(String::new(), |s| format!(" style=\"{}\"", attr(s)));
    let mut out = String::new();
    for set in &d.sets {
        let data = path_data(&set.ops);
        match set.kind {
            SetKind::Path => {
                let _ = write!(
                    out,
                    "<path d=\"{data}\" stroke=\"{}\" stroke-width=\"{}\" fill=\"none\" stroke-dasharray=\"{}\"{style}/>",
                    attr(&c.stroke),
                    attr(&c.stroke_width),
                    c.dash
                );
            }
            SetKind::FillPath => {
                let _ = write!(
                    out,
                    "<path d=\"{data}\" stroke=\"none\" stroke-width=\"0\" fill=\"{}\"{style}/>",
                    attr(&c.fill)
                );
            }
            SetKind::FillSketch => {
                let _ = write!(
                    out,
                    "<path d=\"{data}\" stroke=\"{}\" stroke-width=\"{}\" fill=\"none\" stroke-dasharray=\"0 0\"{style}/>",
                    attr(&c.fill),
                    n(d.fill_weight())
                );
            }
        }
    }
    out
}

/// `opsToPath`.
fn path_data(ops: &[rough::Op]) -> String {
    let mut d = String::new();
    for op in ops {
        match op {
            rough::Op::Move(p) => {
                let _ = write!(d, "M{} {} ", n(p[0]), n(p[1]));
            }
            rough::Op::LineTo(p) => {
                let _ = write!(d, "L{} {} ", n(p[0]), n(p[1]));
            }
            rough::Op::BCurveTo(p) => {
                let _ = write!(
                    d,
                    "C{} {}, {} {}, {} {} ",
                    n(p[0]),
                    n(p[1]),
                    n(p[2]),
                    n(p[3]),
                    n(p[4]),
                    n(p[5])
                );
            }
        }
    }
    d.trim().to_string()
}

/// The path as d3 writes it (every coordinate to three decimals).
pub(super) fn d3_cmds(cmds: &[PathCmd]) -> Vec<PathCmd> {
    let r = |v: f64| rough::js_round(v * 1000.0) / 1000.0;
    let p = |a: Point| Point::new(r(a.x), r(a.y));
    cmds.iter()
        .map(|c| match *c {
            PathCmd::MoveTo(a) => PathCmd::MoveTo(p(a)),
            PathCmd::LineTo(a) => PathCmd::LineTo(p(a)),
            PathCmd::CubicTo(a, b, e) => PathCmd::CubicTo(p(a), p(b), p(e)),
            other => other,
        })
        .collect()
}

/// The path d3's `line` writes (every coordinate to three decimals).
fn d3_path(cmds: &[PathCmd]) -> String {
    let r = |v: f64| n(rough::js_round(v * 1000.0) / 1000.0);
    let mut d = String::new();
    for c in cmds {
        match *c {
            PathCmd::MoveTo(a) => {
                let _ = write!(d, "M{},{}", r(a.x), r(a.y));
            }
            PathCmd::LineTo(a) => {
                let _ = write!(d, "L{},{}", r(a.x), r(a.y));
            }
            PathCmd::CubicTo(a, b, e) => {
                let _ = write!(
                    d,
                    "C{},{},{},{},{},{}",
                    r(a.x),
                    r(a.y),
                    r(b.x),
                    r(b.y),
                    r(e.x),
                    r(e.y)
                );
            }
            PathCmd::Close => d.push('Z'),
            PathCmd::MoveBy(_) | PathCmd::ArcTo { .. } => {}
        }
    }
    d
}

/// The look's `data-look` value.
fn look_name(look: Look) -> &'static str {
    match look {
        Look::Classic => "classic",
        Look::Neo => "neo",
        Look::HandDrawn => "handDrawn",
    }
}

/// A node group and its contents.
pub(super) struct NodeIn<'a> {
    pub b: &'a Block,
    pub shape: BlockShape,
    pub g: &'a NodeGeom,
    pub centre: (f64, f64),
    pub classes: Vec<String>,
    pub styles: Styles,
    /// rough.js's drawing of the shape (handDrawn look).
    pub hand: Option<&'a HandNode>,
}

/// `base64`'s alphabet.
fn btoa(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        out.push(A[(b[0] >> 2) as usize] as char);
        out.push(A[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(b[2] & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `getStylesFromArray`'s `style` and `labelStyle` of a node's own styles.
pub(super) fn plain_styles(own: &[String]) -> (String, String) {
    let (mut style, mut label) = (String::new(), String::new());
    for e in own {
        if e.starts_with("color:") || e.starts_with("text-align:") {
            label.push_str(e);
            label.push(';');
        } else {
            style.push_str(e);
            style.push(';');
        }
    }
    (style, label)
}

fn polygon(points: &[(f64, f64)], tr: &str, style: Option<&str>) -> String {
    let pts: Vec<String> = points
        .iter()
        .map(|&(x, y)| format!("{},{}", n(x), n(y)))
        .collect();
    let style = style.map_or(String::new(), |s| format!(" style=\"{}\"", attr(s)));
    format!(
        "<polygon points=\"{}\" class=\"label-container\" transform=\"{tr}\"{style}/>",
        pts.join(" ")
    )
}

fn translate(t: (f64, f64), spaced: bool) -> String {
    format!(
        "translate({},{}{})",
        n(t.0),
        if spaced { " " } else { "" },
        n(t.1)
    )
}

/// `insertNode`'s group for one block node.
pub(super) fn node(env: &Env, a: &NodeIn) -> String {
    let hand = env.look == Look::HandDrawn;
    let (label_styles, node_styles) = a.styles.strings();
    let g = a.g;
    // The arrow shape keeps the label styles as `getStylesFromArray` split them.
    let is_arrow = matches!(a.shape, BlockShape::Arrow(_));
    let (arrow_style, arrow_label) = plain_styles(&a.styles.own);
    let label_style = if is_arrow { arrow_label } else { label_styles };
    let class_list = if a.classes.is_empty() {
        "default".to_string()
    } else {
        a.classes.join(" ")
    };
    let group_class = format!(
        "{} {class_list} flowchart-label",
        if hand { "rough-node" } else { "node" }
    );
    let colors = rough_colors(env, &a.styles);
    let css_attr = a.styles.css_attr();
    // The one style a shape's `if (nodeStyles) el.attr('style', nodeStyles)` leaves.
    let style_after = |css: &str| {
        if node_styles.is_empty() {
            css.to_string()
        } else {
            node_styles.clone()
        }
    };

    let mut shape = String::new();
    match (&a.hand, hand && !is_arrow) {
        (Some(hn), true) => {
            hand_shape(
                &mut shape,
                a,
                hn,
                &colors,
                &css_attr,
                &node_styles,
                &group_class,
                &class_list,
            );
        }
        _ => match &g.dom {
            Dom::Rect { w, h, rx } => {
                let rx = if a.shape == BlockShape::Rounded {
                    env.radius
                } else {
                    *rx
                };
                let _ = write!(
                    shape,
                    "<rect class=\"basic label-container\" style=\"{}\" rx=\"{}\" ry=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                    attr(&node_styles),
                    n(rx),
                    n(rx),
                    n(-w / 2.0),
                    n(-h / 2.0),
                    n(*w),
                    n(*h)
                );
            }
            Dom::Polygon {
                points,
                translate: t,
                spaced,
            } => {
                let tr = translate(*t, *spaced);
                let style = if is_arrow {
                    Some(arrow_style.as_str())
                } else {
                    (!node_styles.is_empty()).then_some(node_styles.as_str())
                };
                shape.push_str(&polygon(points, &tr, style));
            }
            Dom::Circle(r) => {
                let _ = write!(
                    shape,
                    "<circle class=\"basic label-container\" style=\"{}\" r=\"{}\" cx=\"0\" cy=\"0\"/>",
                    attr(&node_styles),
                    n(*r)
                );
            }
            Dom::DoubleCircle { inner, outer } => {
                let ns = attr(&node_styles);
                let _ = write!(
                    shape,
                    "<g class=\"basic label-container\" style=\"{ns}\"><circle class=\"outer-circle\" style=\"{ns}\" r=\"{}\" cx=\"0\" cy=\"0\"/><circle class=\"inner-circle\" style=\"{ns}\" r=\"{}\" cx=\"0\" cy=\"0\"/></g>",
                    n(*outer),
                    n(*inner)
                );
            }
            Dom::Cylinder { w, h, rx, ry } => {
                let d = format!(
                    "M0,{} a{},{} 0,0,0 {},0 a{},{} 0,0,0 {},0 l0,{} a{},{} 0,0,0 {},0 l0,{}",
                    n(*ry),
                    n(*rx),
                    n(*ry),
                    n(*w),
                    n(*rx),
                    n(*ry),
                    n(-w),
                    n(*h),
                    n(*rx),
                    n(*ry),
                    n(*w),
                    n(-h)
                );
                let _ = write!(
                    shape,
                    "<path d=\"{d}\" class=\"basic label-container outer-path\" style=\"{}\" transform=\"translate({}, {})\"/>",
                    attr(&node_styles),
                    n(-w / 2.0),
                    n(-(h / 2.0 + ry))
                );
            }
            Dom::Rough { d, shift } => {
                let o = Options {
                    roughness: 0.0,
                    fill: Some(colors.fill.clone()),
                    fill_style: "solid".into(),
                    fill_weight: 1.5,
                    hachure_gap: 1.5,
                    stroke: colors.stroke.clone(),
                    seed: env.seed,
                    stroke_width: colors.stroke_width.parse().unwrap_or(1.3),
                    ..Options::default()
                };
                let drawn = rough::path(d, o);
                let tr = shift.map_or(String::new(), |s| {
                    format!(" transform=\"{}\"", translate(s, false))
                });
                let _ = write!(
                    shape,
                    "<g class=\"basic label-container outer-path\"{tr}>{}</g>",
                    rough_paths(&drawn, &colors, Some(&style_after(&css_attr)))
                );
            }
        },
    }

    let (lx, ly) = (
        g.label_off.x - g.label_w / 2.0,
        g.label_off.y - g.label_h / 2.0,
    );
    let label = Label {
        lines: &g.lines,
        w: g.label_w,
        h: g.label_h,
        limit: g.limit,
        table: g.table,
        edge: false,
        style: &label_style,
    };
    format!(
        "<g class=\"{}\" id=\"{ID_MARK}-{}\" data-look=\"{}\" transform=\"translate({}, {})\">{shape}<g class=\"label\" style=\"{}\" transform=\"translate({}, {})\"><rect/>{}</g></g>",
        attr(&group_class),
        attr(&a.b.id),
        look_name(env.look),
        n(a.centre.0),
        n(a.centre.1),
        attr(&label_style),
        n(lx),
        n(ly),
        label.foreign_object()
    )
}

/// The handDrawn look's shapes.
#[allow(clippy::too_many_arguments)]
fn hand_shape(
    out: &mut String,
    a: &NodeIn,
    hn: &HandNode,
    c: &RoughColors,
    css: &str,
    node_styles: &str,
    _group_class: &str,
    class_list: &str,
) {
    // `if (nodeStyles) el.attr('style', nodeStyles)` after `cssStyles`.
    let style = if node_styles.is_empty() {
        css
    } else {
        node_styles
    };
    let group = |piece: usize, attrs: &str| {
        let pc = &hn.pieces[piece];
        format!("<g{attrs}>{}</g>", rough_paths(&pc.drawable, c, None))
    };
    let tr = |at: Point, spaced: bool| translate((at.x, at.y), spaced);
    match a.shape {
        BlockShape::Rect | BlockShape::Rounded | BlockShape::Circle => {
            let _ = write!(
                out,
                "{}",
                group(
                    0,
                    &format!(" class=\"basic label-container\" style=\"{}\"", attr(css))
                )
            );
        }
        BlockShape::Diamond
        | BlockShape::Hexagon
        | BlockShape::LeanRight
        | BlockShape::LeanLeft
        | BlockShape::Trapezoid
        | BlockShape::InvTrapezoid => {
            let at = hn.pieces[0].at;
            let _ = write!(
                out,
                "{}",
                group(
                    0,
                    &format!(" transform=\"{}\" style=\"{}\"", tr(at, true), attr(style))
                )
            );
        }
        BlockShape::Stadium => {
            let _ = write!(
                out,
                "{}",
                group(0, " class=\"basic label-container outer-path\"")
            );
        }
        BlockShape::Asymmetric => {
            let at = hn.pieces[0].at;
            let _ = write!(
                out,
                "{}",
                group(
                    0,
                    &format!(
                        " class=\"basic label-container outer-path\" transform=\"{}\"",
                        tr(at, false)
                    )
                )
            );
        }
        BlockShape::DoubleCircle => {
            let _ = write!(
                out,
                "<g class=\"{}\" style=\"{}\">{}{}</g>",
                attr(&format!("{class_list} flowchart-label")),
                attr(css),
                group(0, ""),
                group(1, "")
            );
        }
        BlockShape::Subroutine => {
            let _ = write!(
                out,
                "{}{}{}",
                group(
                    0,
                    &format!(" class=\"basic label-container\" style=\"{}\"", attr(css))
                ),
                group(1, ""),
                group(2, "")
            );
        }
        BlockShape::Cylinder => {
            let at = hn.pieces[0].at;
            let _ = write!(
                out,
                "{}{}",
                group(
                    0,
                    &format!(
                        " class=\"basic label-container\" style=\"{}\" transform=\"{}\"",
                        attr(css),
                        tr(at, true)
                    )
                ),
                group(1, "")
            );
        }
        BlockShape::Arrow(_) => {}
    }
}

/// A composite block: `composite`'s `rect` and its (empty) label.
pub(super) fn composite(
    env: &Env,
    b: &Block,
    classes: &[String],
    rect: (f64, f64, f64, f64),
    centre: (f64, f64),
    own_styles: &[String],
) -> String {
    let (style, _) = plain_styles(own_styles);
    let class_list = if classes.is_empty() {
        "default".to_string()
    } else {
        classes.join(" ")
    };
    let group_class = format!("node {class_list} flowchart-label {class_list} flowchart-label");
    let (x, y, w, h) = rect;
    let label = Label {
        lines: &[],
        w: 0.0,
        h: 0.0,
        limit: w,
        table: false,
        edge: false,
        style: "",
    };
    format!(
        "<g class=\"{}\" id=\"{ID_MARK}-{}\" data-look=\"{}\" transform=\"translate({}, {})\"><rect class=\"basic cluster composite label-container\" style=\"{}\" rx=\"0\" ry=\"0\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/><g class=\"label\" style=\"\" transform=\"translate(0, 0)\"><rect/>{}</g></g>",
        attr(&group_class),
        attr(&b.id),
        look_name(env.look),
        n(centre.0),
        n(centre.1),
        attr(&style),
        n(x),
        n(y),
        n(w),
        n(h),
        label.foreign_object()
    )
}

/// An edge's path, and its label.
pub(super) struct EdgeIn<'a> {
    /// `{count}-{from}-{to}`.
    pub id: String,
    pub line: Line,
    pub start: Head,
    pub end: Head,
    /// The points after clipping (`data-points`).
    pub points: [Point; 3],
    /// The path d3 draws.
    pub path: &'a [PathCmd],
    /// Lines, size, `max-width`, and whether the box is a `display: table`.
    pub label: Option<(&'a [String], f64, f64, f64, bool)>,
    pub mid: Point,
}

fn marker_id(h: Head, start: bool) -> Option<&'static str> {
    Some(match (h, start) {
        (Head::None, _) => return None,
        (Head::Arrow, false) => "pointEnd",
        (Head::Arrow, true) => "pointStart",
        (Head::Circle, false) => "circleEnd",
        (Head::Circle, true) => "circleStart",
        (Head::Cross, false) => "crossEnd",
        (Head::Cross, true) => "crossStart",
    })
}

pub(super) fn edge(e: &EdgeIn) -> String {
    let (thickness, pattern) = match e.line {
        Line::Thick => ("edge-thickness-thick", "edge-pattern-solid"),
        Line::Dotted => ("edge-thickness-normal", "edge-pattern-dotted"),
        Line::Solid => ("edge-thickness-normal", "edge-pattern-solid"),
    };
    let json = format!(
        "[{}]",
        e.points
            .iter()
            .map(|p| format!("{{\"x\":{},\"y\":{}}}", n(p.x), n(p.y)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut out = format!(
        "<path d=\"{}\" id=\"{ID_MARK}-{ID_MARK}-{}\" class=\"{thickness} {pattern} {thickness} {pattern} flowchart-link LS-a1 LE-b1\" style=\"undefined;;;undefined\" data-edge=\"true\" data-et=\"edge\" data-id=\"{ID_MARK}-{}\" data-points=\"{}\"",
        d3_path(e.path),
        attr(&e.id),
        attr(&e.id),
        btoa(json.as_bytes())
    );
    if let Some(m) = marker_id(e.start, true) {
        let _ = write!(out, " marker-start=\"url(#{ID_MARK}_block-{m})\"");
    }
    if let Some(m) = marker_id(e.end, false) {
        let _ = write!(out, " marker-end=\"url(#{ID_MARK}_block-{m})\"");
    }
    out.push_str("/>");
    if let Some((lines, w, h, limit, table)) = e.label {
        let label = Label {
            lines,
            w,
            h,
            limit,
            table,
            edge: true,
            style: "",
        };
        let _ = write!(
            out,
            "<g class=\"edgeLabel\" transform=\"translate({}, {})\"><g class=\"label\" data-id=\"{ID_MARK}-{}\" transform=\"translate({}, {})\">{}</g></g>",
            n(e.mid.x),
            n(e.mid.y),
            attr(&e.id),
            n(-w / 2.0),
            n(-h / 2.0),
            label.foreign_object()
        );
    }
    out
}

/// Everything in front of the blocks: the look's definitions and the markers.
pub(super) fn defs(pal: &Palette, dark: bool) -> String {
    let flood = if dark { "#FFFFFF" } else { "#000000" };
    let mut s = String::from("<g/>");
    for (id, size, off) in [
        ("drop-shadow", "130%", "4"),
        ("drop-shadow-small", "150%", "2"),
    ] {
        let _ = write!(
            s,
            "<defs><filter id=\"{ID_MARK}-{id}\" height=\"{size}\" width=\"{size}\"><feDropShadow dx=\"{off}\" dy=\"{off}\" stdDeviation=\"0\" flood-opacity=\"0.06\" flood-color=\"{flood}\"/></filter></defs>"
        );
    }
    if let Some((a, b)) = pal.node_gradient.as_ref() {
        let _ = write!(
            s,
            "<linearGradient id=\"{ID_MARK}-gradient\" gradientUnits=\"objectBoundingBox\" x1=\"0%\" y1=\"0%\" x2=\"100%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"{}\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"{}\" stop-opacity=\"1\"/></linearGradient>",
            attr(a.css()),
            attr(b.css())
        );
    }
    s.push_str(&MARKERS.replace("{ID}", ID_MARK));
    s
}

/// `insertMarkers(svg, ['point', 'circle', 'cross'], 'block', id)`.
const MARKERS: &str = include_str!("markers.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_like_btoa() {
        assert_eq!(btoa(b"a"), "YQ==");
        assert_eq!(btoa(b"ab"), "YWI=");
        assert_eq!(btoa(b"abc"), "YWJj");
        assert_eq!(btoa(br#"[{"x":1,"y":-2}]"#), "W3sieCI6MSwieSI6LTJ9XQ==");
    }

    /// Chromium serialises a length with six significant digits.
    #[test]
    fn lengths_are_six_significant_digits() {
        assert_eq!(css_px(198.037_216_186_523_44), "198.037");
        assert_eq!(css_px(466.0625), "466.062");
        assert_eq!(css_px(90.167_4), "90.1674");
        assert_eq!(css_px(200.0), "200");
    }

    #[test]
    fn style_objects_reserialise_colours() {
        assert_eq!(
            cssom("color:#fff !important;font-size:12px"),
            "color: rgb(255, 255, 255) !important; font-size: 12px; "
        );
    }

    #[test]
    fn styles_map_replaces_values_in_place() {
        let st = Styles {
            compiled: vec!["fill:#bbf".into(), "stroke:#f66".into()],
            own: vec!["fill:#f9f".into(), "color:#fff".into()],
        };
        let (label, node) = st.strings();
        assert_eq!(label, "color:#fff !important");
        assert_eq!(node, "fill:#f9f !important;stroke:#f66 !important");
    }
}
