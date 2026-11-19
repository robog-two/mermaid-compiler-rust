//! A mindmap as mermaid's own DOM: the unified renderer's `insertNode` shapes
//! (`defaultMindmapNode`, `squareRect`, `roundedRect`, `mindmapCircle`, `bang`,
//! `cloud`, `hexagon`, and their rough.js versions), `insertEdge` paths and
//! labels, and the look definitions the cose-bilkent layout leaves in the root.

use super::*;
use crate::diagram::kanban::{Box32, attr};
use crate::diagram::svgbbox::{cubic_group_box, js_f32};
use crate::diagram::wardley::ID_MARK;
use crate::output::svg::{esc_text, js_num as n};
use std::fmt::Write as _;

/// What the markup of one drawing depends on.
pub(super) struct Dom<'a> {
    pub look: &'static str,
    pub pal: &'a Palette,
    pub padding: f64,
    pub widths: Widths,
    pub hand: Option<&'a HandStyle>,
    pub neo: bool,
}

/// Standard base64 (`btoa`).
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let v = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(v >> 18) as usize & 63] as char);
        out.push(A[(v >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(v >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[v as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A label's markdown as `markdownToHTML` writes it: one paragraph, its lines
/// joined by `<br/>`.
fn label_html(label: &str) -> String {
    use crate::text::mark;
    let raw = crate::text::markdown_lines(&crate::icons::replace_references(label));
    if raw.iter().all(|l| l.trim().is_empty()) {
        return String::new();
    }
    let mut out = String::from("<p>");
    for (i, l) in raw.iter().enumerate() {
        if i > 0 {
            out.push_str("<br/>");
        }
        let mut plain = String::new();
        for c in l.chars() {
            let tag = match c {
                mark::BOLD_ON => "<strong>",
                mark::BOLD_OFF => "</strong>",
                mark::ITALIC_ON => "<em>",
                mark::ITALIC_OFF => "</em>",
                mark::RUN => "",
                _ => {
                    plain.push(c);
                    continue;
                }
            };
            out.push_str(&esc_text(&std::mem::take(&mut plain)));
            out.push_str(tag);
        }
        out.push_str(&esc_text(&plain));
    }
    out.push_str("</p>");
    out
}

/// The label `<g>` `labelHelper` builds: an empty `rect` and the HTML label's
/// `foreignObject`, centred on the node.
fn label_group(nd: &Node, p: &Placed, width: f64) -> String {
    let (tw, th) = p.label;
    // addHtmlSpan turns a label as wide as its box into a `table`.
    let (display, space, tail) = if tw == width {
        ("table", "break-spaces", format!(" width: {}px;", n(width)))
    } else {
        ("table-cell", "nowrap", String::new())
    };
    // `addSvgBackground: !!node.icon`.
    let class = if nd.icon.as_deref().is_some_and(|i| !i.is_empty()) {
        " class=\"labelBkg\""
    } else {
        ""
    };
    format!(
        "<g class=\"label\" style=\"\" transform=\"translate({}, {})\"><rect/><foreignObject width=\"{}\" height=\"{}\"><div xmlns=\"http://www.w3.org/1999/xhtml\"{class} style=\"display: {display}; white-space: {space}; line-height: 1.5; max-width: {}px; text-align: center;{tail}\"><span class=\"nodeLabel markdown-node-label\">{}</span></div></foreignObject></g>",
        n(-tw / 2.0),
        n(-th / 2.0),
        n(tw),
        n(th),
        n(width),
        label_html(&nd.label),
    )
}

/// The width a node's label is wrapped at (see [`measure`]).
fn wrap_width(shape: NodeShape, widths: Widths) -> f64 {
    match shape {
        NodeShape::Square | NodeShape::Rounded | NodeShape::Hexagon => widths.wrapping,
        _ => widths.max_node,
    }
}

/// A path's data as rough.js writes it.
fn rough_d(ops: &[crate::rough::Op]) -> String {
    let mut d = String::new();
    for op in ops {
        match op {
            crate::rough::Op::Move(p) => {
                let _ = write!(d, "M{} {} ", n(p[0]), n(p[1]));
            }
            crate::rough::Op::LineTo(p) => {
                let _ = write!(d, "L{} {} ", n(p[0]), n(p[1]));
            }
            crate::rough::Op::BCurveTo(p) => {
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

/// The `<path>` elements rough.js's `svg` renderer makes of a drawing.
fn rough_paths(d: &crate::rough::Drawable) -> String {
    let o = &d.options;
    let mut out = String::new();
    for set in d.sets.iter().filter(|s| !s.ops.is_empty()) {
        let (stroke, width, fill) = match set.kind {
            crate::rough::SetKind::Path => {
                (o.stroke.clone(), n(o.stroke_width), "none".to_string())
            }
            crate::rough::SetKind::FillSketch => (
                o.fill.clone().unwrap_or_default(),
                n(d.fill_weight()),
                "none".to_string(),
            ),
            crate::rough::SetKind::FillPath => (
                "none".to_string(),
                "0".to_string(),
                o.fill.clone().unwrap_or_default(),
            ),
        };
        let _ = write!(
            out,
            "<path d=\"{}\" stroke=\"{}\" stroke-width=\"{width}\" fill=\"{}\" stroke-dasharray=\"0 0\"/>",
            attr(&rough_d(&set.ops)),
            attr(&stroke),
            attr(&fill),
        );
    }
    out
}

/// `SVGPathElement.getTotalLength()` of a path of lines and cubics in single
/// precision, as Blink's `PathTraversalState` computes it: each cubic is
/// halved until its control polygon is as long as its chord (to a relative
/// 1e-8), and the polygons' lengths are added.
fn path_length(cmds: &[PathCmd]) -> f32 {
    type P = [f32; 2];
    fn dist(a: P, b: P) -> f32 {
        let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
        (dx * dx + dy * dy).sqrt() as f32
    }
    fn mid(a: P, b: P) -> P {
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    }
    fn dot(p: P) -> f64 {
        f64::from(p[0]) * f64::from(p[0]) + f64::from(p[1]) * f64::from(p[1])
    }
    #[derive(Clone, Copy)]
    struct Curve {
        p: [P; 4],
        depth: u16,
    }
    fn cubic(c: Curve) -> f32 {
        let scale = (dot(c.p[0]) + dot(c.p[1]) + dot(c.p[2]) + dot(c.p[3])) / 16.0;
        if scale < 1e-16 {
            return 0.0;
        }
        let mut stack = vec![c];
        let mut cur = c;
        let mut total = 0.0f32;
        loop {
            let [s, c1, c2, e] = cur.p;
            let length = dist(s, c1) + dist(c1, c2) + dist(c2, e);
            let disc = f64::from(length - dist(s, e));
            if disc * disc / scale > 1e-16 && cur.depth < 20 {
                let s2 = mid(c1, c2);
                let (l1, r2) = (mid(s, c1), mid(c2, e));
                let (l2, r1) = (mid(l1, s2), mid(r2, s2));
                let m = mid(l2, r1);
                let right = Curve {
                    p: [m, r1, r2, e],
                    depth: cur.depth + 1,
                };
                cur = Curve {
                    p: [s, l1, l2, m],
                    depth: cur.depth + 1,
                };
                stack.push(right);
            } else {
                total += length;
                cur = stack.pop().expect("the curve itself is on the stack");
                if stack.is_empty() {
                    return total;
                }
            }
        }
    }
    let f = |p: Point| -> P { [js_f32(p.x), js_f32(p.y)] };
    let mut total = 0.0f32;
    let mut cur: P = [0.0, 0.0];
    for c in cmds {
        match *c {
            PathCmd::MoveTo(p) => cur = f(p),
            PathCmd::LineTo(p) => {
                let q = f(p);
                total += dist(cur, q);
                cur = q;
            }
            PathCmd::CubicTo(a, b, e) => {
                let (a, b, e) = (f(a), f(b), f(e));
                total += cubic(Curve {
                    p: [cur, a, b, e],
                    depth: 0,
                });
                cur = e;
            }
            _ => {}
        }
    }
    total
}

/// `Math.round(v * 1000) / 1000`, as d3-path writes a coordinate.
fn round3(v: f64) -> f64 {
    crate::rough::js_round(v * 1000.0) / 1000.0
}

/// `<g class="tag">` or, empty, `<g class="tag"/>`.
fn group(class: &str, inner: &str) -> String {
    if inner.is_empty() {
        format!("<g class=\"{class}\"/>")
    } else {
        format!("<g class=\"{class}\">{inner}</g>")
    }
}

/// `markers.point`: the four arrow heads `insertMarkers` puts in the root.
fn markers() -> String {
    format!(
        "<marker id=\"{ID_MARK}_mindmap-pointEnd\" class=\"marker mindmap\" viewBox=\"0 0 10 10\" refX=\"5\" refY=\"5\" markerUnits=\"userSpaceOnUse\" markerWidth=\"8\" markerHeight=\"8\" orient=\"auto\"><path d=\"M 0 0 L 10 5 L 0 10 z\" class=\"arrowMarkerPath\" style=\"stroke-width: 1; stroke-dasharray: 1, 0;\"/></marker>\
<marker id=\"{ID_MARK}_mindmap-pointStart\" class=\"marker mindmap\" viewBox=\"0 0 10 10\" refX=\"4.5\" refY=\"5\" markerUnits=\"userSpaceOnUse\" markerWidth=\"8\" markerHeight=\"8\" orient=\"auto\"><path d=\"M 0 5 L 10 10 L 10 0 z\" class=\"arrowMarkerPath\" style=\"stroke-width: 1; stroke-dasharray: 1, 0;\"/></marker>\
<marker id=\"{ID_MARK}_mindmap-pointEnd-margin\" class=\"marker mindmap\" viewBox=\"0 0 11.5 14\" refX=\"11.5\" refY=\"7\" markerUnits=\"userSpaceOnUse\" markerWidth=\"10.5\" markerHeight=\"14\" orient=\"auto\"><path d=\"M 0 0 L 11.5 7 L 0 14 z\" class=\"arrowMarkerPath\" style=\"stroke-width: 0; stroke-dasharray: 1, 0;\"/></marker>\
<marker id=\"{ID_MARK}_mindmap-pointStart-margin\" class=\"marker mindmap\" viewBox=\"0 0 11.5 14\" refX=\"1\" refY=\"7\" markerUnits=\"userSpaceOnUse\" markerWidth=\"11.5\" markerHeight=\"14\" orient=\"auto\"><polygon points=\"0,7 11.5,14 11.5,0\" class=\"arrowMarkerPath\" style=\"stroke-width: 0; stroke-dasharray: 1, 0;\"/></marker>"
    )
}

/// One edge label: always empty for a mindmap.
fn edge_label(edge_id: &str) -> String {
    format!(
        "<g class=\"edgeLabel\"><g class=\"label\" data-id=\"{edge_id}\" transform=\"translate(0, 0)\"><foreignObject width=\"0\" height=\"0\"><div xmlns=\"http://www.w3.org/1999/xhtml\" class=\"labelBkg\" style=\"display: table-cell; white-space: nowrap; line-height: 1.5; max-width: 200px; text-align: center;\"><span class=\"edgeLabel\"></span></div></foreignObject></g></g>"
    )
}

impl Dom<'_> {
    /// `insertLookDefs` and the gradient the mindmap renderer adds after it.
    fn defs(&self) -> String {
        let flood = if self.pal.theme.name().contains("dark") {
            "#FFFFFF"
        } else {
            "#000000"
        };
        let mut out = String::new();
        for (name, size, off) in [
            ("drop-shadow", "130%", "4"),
            ("drop-shadow-small", "150%", "2"),
        ] {
            let _ = write!(
                out,
                "<defs><filter id=\"{ID_MARK}-{name}\" height=\"{size}\" width=\"{size}\"><feDropShadow dx=\"{off}\" dy=\"{off}\" stdDeviation=\"0\" flood-opacity=\"0.06\" flood-color=\"{flood}\"/></filter></defs>"
            );
        }
        if let Some((a, b)) = &self.pal.node_gradient {
            let g = format!(
                "<linearGradient id=\"{ID_MARK}-gradient\" gradientUnits=\"objectBoundingBox\" x1=\"0%\" y1=\"0%\" x2=\"100%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"{}\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"{}\" stop-opacity=\"1\"/></linearGradient>",
                attr(a.css()),
                attr(b.css())
            );
            let _ = write!(out, "{g}<defs>{g}</defs>");
        }
        out
    }

    /// The rough.js group of a node shape: `class` and `transform` as the
    /// shape file sets them.
    fn rough_group(&self, hn: &HandNode, class: bool, transform: bool) -> String {
        let pc = &hn.pieces[0];
        let class = if class {
            " class=\"basic label-container\""
        } else {
            ""
        };
        let tf = if transform {
            format!(" transform=\"translate({}, {})\"", n(pc.at.x), n(pc.at.y))
        } else {
            String::new()
        };
        // hexagon.ts puts the transform before the style, the others after.
        if class.is_empty() {
            format!("<g{tf} style=\"\">{}</g>", rough_paths(&pc.drawable))
        } else {
            format!("<g{class} style=\"\"{tf}>{}</g>", rough_paths(&pc.drawable))
        }
    }

    /// A node's shape elements and the box of everything it draws, in the
    /// node's own frame.
    fn shape(&self, i: usize, nd: &Node, p: &Placed) -> (String, Rect) {
        let (tw, th) = p.label;
        let padding = self.padding;
        let neo = self.neo;
        let rough = self
            .hand
            .and_then(|st| hand_node(nd.shape, p.label, padding, st));
        let local = match &rough {
            Some(hn) => hn.drawn_bbox(),
            None => p.bbox,
        };
        let style = "style=\"\"";
        let mk = match nd.shape {
            NodeShape::Default => {
                let hp = padding / 2.0;
                let (w, h, rd) = (tw + 8.0 * hp, th + 2.0 * hp, 5.0);
                let (x, y) = (n(-w / 2.0), n(h / 2.0 - rd));
                let d = if neo {
                    format!(
                        "M{x} {y}\n    v{}\n    q0,-{} {},-{}\n    h{}\n    q{},0 {},{}\n    v{}\n    H{}\n    Z",
                        n(-h + 2.0 * rd),
                        n(rd),
                        n(rd),
                        n(rd),
                        n(w - 2.0 * rd),
                        n(rd),
                        n(rd),
                        n(rd),
                        n(h - rd),
                        n(-w / 2.0),
                    )
                } else {
                    format!(
                        "M{x} {y}\n    v{}\n    q0,-{} {},-{}\n    h{}\n    q{},0 {},{}\n    v{}\n    q0,{} {},{}\n    h{}\n    q{},0 {},{}\n    Z",
                        n(-h + 2.0 * rd),
                        n(rd),
                        n(rd),
                        n(rd),
                        n(w - 2.0 * rd),
                        n(rd),
                        n(rd),
                        n(rd),
                        n(h - 2.0 * rd),
                        n(rd),
                        n(-rd),
                        n(rd),
                        n(-(w - 2.0 * rd)),
                        n(-rd),
                        n(-rd),
                        n(-rd),
                    )
                };
                format!(
                    "<path id=\"{ID_MARK}-node_{i}\" class=\"node-bkg node-0\" {style} d=\"{}\"/><line class=\"node-line-\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
                    attr(&d),
                    n(-w / 2.0),
                    n(h / 2.0),
                    n(w / 2.0),
                    n(h / 2.0),
                )
            }
            NodeShape::Square | NodeShape::Rounded => {
                let (rx, lpx, lpy) = if nd.shape == NodeShape::Rounded {
                    (self.pal.radius.unwrap_or(5.0), 15.0, 15.0)
                } else if neo {
                    (0.0, 16.0, 12.0)
                } else {
                    (0.0, 20.0, 10.0)
                };
                let (w, h) = (tw + lpx * 2.0, th + lpy * 2.0);
                if let Some(hn) = &rough {
                    self.rough_group(hn, true, false)
                } else {
                    let r = if rx != 0.0 {
                        format!(" rx=\"{}\" ry=\"{}\"", n(rx), n(rx))
                    } else {
                        String::new()
                    };
                    format!(
                        "<rect class=\"basic label-container\" {style}{r} x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                        n(-w / 2.0),
                        n(-h / 2.0),
                        n(w),
                        n(h)
                    )
                }
            }
            NodeShape::Circle => {
                if let Some(hn) = &rough {
                    self.rough_group(hn, true, false)
                } else {
                    let radius =
                        (tw * tw + th * th).sqrt() / 2.0 + if neo { 16.0 } else { padding };
                    format!(
                        "<circle class=\"basic label-container\" {style} r=\"{}\" cx=\"0\" cy=\"0\"/>",
                        n(radius)
                    )
                }
            }
            NodeShape::Hexagon => {
                let f = if neo { 3.5 } else { 4.0 };
                let (lpx, lpy) = if neo {
                    (70.0, 32.0)
                } else {
                    (2.0 * padding, 2.0 * padding)
                };
                let h = th + lpx;
                let m = h / f;
                let w = tw + 2.0 * m + lpy;
                if let Some(hn) = &rough {
                    self.rough_group(hn, false, true)
                } else {
                    let pts = [
                        (m, 0.0),
                        (w - m, 0.0),
                        (w, -h / 2.0),
                        (w - m, -h),
                        (m, -h),
                        (0.0, -h / 2.0),
                    ];
                    let pts: Vec<String> = pts
                        .iter()
                        .map(|&(x, y)| format!("{},{}", n(x), n(y)))
                        .collect();
                    format!(
                        "<polygon points=\"{}\" class=\"label-container\" transform=\"translate({},{})\"/>",
                        pts.join(" "),
                        n(-w / 2.0),
                        n(h / 2.0)
                    )
                }
            }
            NodeShape::Bang => {
                let hp = padding / 2.0;
                let (w, h) = (tw + 10.0 * hp, th + 8.0 * hp);
                let r = 0.15 * w;
                let (ew, eh) = (w.max(tw + 20.0), h.max(th + 20.0));
                if let Some(hn) = &rough {
                    self.rough_group(hn, true, true)
                } else {
                    let (r, r8) = (n(r), n(r * 0.8));
                    let a = |v: f64| n(v);
                    let d = format!(
                        "M0 0 \n    a{r},{r} 1 0,0 {},{}\n    a{r},{r} 1 0,0 {},{}\n    a{r},{r} 1 0,0 {},{}\n    a{r},{r} 1 0,0 {},{}\n\n    a{r},{r} 1 0,0 {},{}\n    a{r8},{r8} 1 0,0 0,{}\n    a{r},{r} 1 0,0 {},{}\n\n    a{r},{r} 1 0,0 {},{}\n    a{r},{r} 1 0,0 {},0\n    a{r},{r} 1 0,0 {},0\n    a{r},{r} 1 0,0 {},{}\n\n    a{r},{r} 1 0,0 {},{}\n    a{r8},{r8} 1 0,0 0,{}\n    a{r},{r} 1 0,0 {},{}\n  H0 V0 Z",
                        a(ew * 0.25),
                        a(-eh * 0.1),
                        a(ew * 0.25),
                        a(0.0),
                        a(ew * 0.25),
                        a(0.0),
                        a(ew * 0.25),
                        a(eh * 0.1),
                        a(ew * 0.15),
                        a(eh * 0.33),
                        a(eh * 0.34),
                        a(-ew * 0.15),
                        a(eh * 0.33),
                        a(-ew * 0.25),
                        a(eh * 0.15),
                        a(-ew * 0.25),
                        a(-ew * 0.25),
                        a(-ew * 0.25),
                        a(-eh * 0.15),
                        a(-ew * 0.1),
                        a(-eh * 0.33),
                        a(-eh * 0.34),
                        a(ew * 0.1),
                        a(-eh * 0.33),
                    );
                    format!(
                        "<path class=\"basic label-container\" {style} d=\"{}\" transform=\"translate({}, {})\"/>",
                        attr(&d),
                        n(-ew / 2.0),
                        n(-eh / 2.0)
                    )
                }
            }
            NodeShape::Cloud => {
                let hp = padding / 2.0;
                let (w, h) = (tw + 2.0 * hp, th + 2.0 * hp);
                if let Some(hn) = &rough {
                    self.rough_group(hn, true, true)
                } else {
                    let (r1, r2, r3, r4) = (0.15 * w, 0.25 * w, 0.35 * w, 0.2 * w);
                    let a = |v: f64| n(v);
                    let d = format!(
                        "M0 0 \n    a{r1},{r1} 0 0,1 {},{}\n    a{r3},{r3} 1 0,1 {},{}\n    a{r2},{r2} 1 0,1 {},{}\n\n    a{r1},{r1} 1 0,1 {},{}\n    a{r4},{r4} 1 0,1 {},{}\n\n    a{r2},{r1} 1 0,1 {},{}\n    a{r3},{r3} 1 0,1 {},0\n    a{r1},{r1} 1 0,1 {},{}\n\n    a{r1},{r1} 1 0,1 {},{}\n    a{r4},{r4} 1 0,1 {},{}\n  H0 V0 Z",
                        a(w * 0.25),
                        a(-w * 0.1),
                        a(w * 0.4),
                        a(-w * 0.1),
                        a(w * 0.35),
                        a(w * 0.2),
                        a(w * 0.15),
                        a(h * 0.35),
                        a(-w * 0.15),
                        a(h * 0.65),
                        a(-w * 0.25),
                        a(w * 0.15),
                        a(-w * 0.5),
                        a(-w * 0.25),
                        a(-w * 0.15),
                        a(-w * 0.1),
                        a(-h * 0.35),
                        a(w * 0.1),
                        a(-h * 0.65),
                    );
                    format!(
                        "<path class=\"basic label-container\" {style} d=\"{}\" transform=\"translate({}, {})\"/>",
                        attr(&d),
                        n(-w / 2.0),
                        n(-h / 2.0)
                    )
                }
            }
        };
        (mk, local)
    }

    /// One node: its group and the box of everything in it, in the node's frame.
    fn node(&self, i: usize, nd: &Node, p: &Placed) -> (String, Rect) {
        let (shape, local) = self.shape(i, nd, p);
        let mut css = String::from("mindmap-node");
        match p.section {
            None => css.push_str(" section-root section--1"),
            Some(s) => {
                let _ = write!(css, " section-{s}");
            }
        }
        for c in &nd.classes {
            css.push(' ');
            css.push_str(c);
        }
        let base = if self.hand.is_some() {
            "rough-node"
        } else {
            "node"
        };
        let mk = format!(
            "<g class=\"{base} {}\" id=\"{ID_MARK}-node_{i}\" data-look=\"{}\" transform=\"translate({}, {})\">{shape}{}</g>",
            attr(&css),
            self.look,
            n(p.center.x),
            n(p.center.y),
            label_group(nd, p, wrap_width(nd.shape, self.widths)),
        );
        (mk, local)
    }

    /// One edge's path, its label and the path's box.
    fn edge(
        &self,
        (a, b): (usize, usize),
        pts: [Point; 3],
        depth: usize,
        section: usize,
    ) -> (String, String, Rect) {
        let id = format!("edge_{a}_{b}");
        let cmds = geom::basis_curve(&pts);
        let d3 = super::super::graph::d3_path_data(&Geometry::Path(cmds.clone()));
        let json = format!(
            "[{}]",
            pts.iter()
                .map(|q| format!("{{\"x\":{},\"y\":{}}}", n(q.x), n(q.y)))
                .collect::<Vec<_>>()
                .join(",")
        );
        let data = format!(
            "data-edge=\"true\" data-et=\"edge\" data-id=\"{id}\" data-points=\"{}\" data-look=\"{}\"",
            base64(json.as_bytes()),
            self.look
        );
        let classes = format!("edge section-edge-{section} edge-depth-{depth}");
        let (mk, bx) = if let Some(st) = self.hand {
            let o = crate::rough::Options {
                roughness: 0.3,
                seed: st.seed,
                ..Default::default()
            };
            let drawn = crate::rough::path(&d3, o);
            let set = drawn.sets.iter().find(|s| !s.ops.is_empty());
            let ops: &[crate::rough::Op] = set.map_or(&[], |s| &s.ops);
            let bx = cubic_group_box(&[hand::set_cmds(ops, Point::default())])
                .map_or(Rect::default(), |[x, y, w, h]| Rect::new(x, y, w, h));
            (
                format!(
                    "<path d=\"{}\" stroke=\"{}\" stroke-width=\"{}\" fill=\"none\" id=\"{ID_MARK}-{id}\" class=\"edge-thickness-normal edge-pattern-solid transition {classes}\" style=\";undefined\" {data}/>",
                    attr(&rough_d(ops)),
                    attr(&drawn.options.stroke),
                    n(drawn.options.stroke_width),
                ),
                bx,
            )
        } else {
            let rounded: Vec<PathCmd> = cmds
                .iter()
                .map(|c| {
                    let r = |q: Point| Point::new(round3(q.x), round3(q.y));
                    match *c {
                        PathCmd::MoveTo(q) => PathCmd::MoveTo(r(q)),
                        PathCmd::LineTo(q) => PathCmd::LineTo(r(q)),
                        PathCmd::CubicTo(x, y, z) => PathCmd::CubicTo(r(x), r(y), r(z)),
                        other => other,
                    }
                })
                .collect();
            let dash = if self.neo {
                // `0 ${start offset} ${length - offsets} ${end offset}`.
                let len = f64::from(path_length(&rounded));
                format!("stroke-dasharray: 0 0 {} 0; stroke-dashoffset: 0;", n(len))
            } else {
                String::new()
            };
            let bx = cubic_group_box(&[rounded])
                .map_or(Rect::default(), |[x, y, w, h]| Rect::new(x, y, w, h));
            (
                format!(
                    "<path d=\"{d3}\" id=\"{ID_MARK}-{id}\" class=\"edge-thickness-normal edge-pattern-solid {classes}\" style=\"{dash}undefined;;;undefined\" {data}/>"
                ),
                bx,
            )
        };
        (mk, edge_label(&id), bx)
    }
}

/// The whole drawing: mermaid's markup and the box `setupViewPortForSVG`
/// measures.
pub(super) fn markup(
    mm: &Mindmap,
    placed: &[Placed],
    dom: &Dom,
    padding: f64,
) -> (String, Option<Rect>) {
    let (mut paths, mut labels, mut nodes) = (String::new(), String::new(), String::new());
    let (mut edge_box, mut node_box) = (Box32::default(), Box32::default());
    for (i, nd) in mm.nodes.iter().enumerate() {
        let p = &placed[i];
        if let Some(par) = nd.parent {
            let pts = edge_points(placed[par].center, p.center);
            let depth = mm.nodes[par].level - mm.nodes[0].level + 1;
            let (path, label, bx) = dom.edge((par, i), pts, depth, p.section.unwrap_or(0));
            paths.push_str(&path);
            labels.push_str(&label);
            edge_box.add(bx);
        }
        let (mk, local) = dom.node(i, nd, p);
        nodes.push_str(&mk);
        // Chromium moves the node's box by its translation in single precision.
        let (x, y) = (
            local.x as f32 + p.center.x as f32,
            local.y as f32 + p.center.y as f32,
        );
        node_box.add(Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(local.w as f32),
            f64::from(local.h as f32),
        ));
    }
    let svg = format!(
        "<g>{}{}{}{}{}</g>{}",
        markers(),
        "<g class=\"subgraphs\"/>",
        group("edgePaths", &paths),
        group("edgeLabels", &labels),
        group("nodes", &nodes),
        dom.defs(),
    );
    (svg, edge_box.join(&node_box).view_box(padding))
}
