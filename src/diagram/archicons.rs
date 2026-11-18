//! The architecture diagram's built-in icons (`database`, `server`, `disk`,
//! `internet`, `cloud`), drawn from mermaid's own SVG bodies, and its
//! `unknownIcon` (a `?`) for any other name.
//!
//! Only what those bodies use is read: `path` (`M L H V C S Z`, absolute or
//! relative), `rect` (with `rx`/`ry`), `line`, `circle` and `ellipse`, each
//! with an inline `fill` / `stroke` / `stroke-width` style.

use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, Element, Geometry, Paint, Text};

/// The elements of icon `name` scaled onto `r` (an 80 × 80 canvas), above the
/// blue background the caller draws; `None` for a name without a built-in icon.
pub fn builtin(name: &str, r: Rect) -> Option<Vec<Element>> {
    let body = ICONS.iter().find(|(n, _)| *n == name)?.1;
    let s = r.w / 80.0;
    Some(
        elements(body)
            .into_iter()
            .filter_map(|(tag, attrs)| element(&tag, &attrs, r, s))
            .collect(),
    )
}

/// mermaid's `unknownIcon`: a white `?` (67.75px Arial) at (21.16, 64.67).
pub fn unknown(r: Rect) -> Element {
    let s = r.w / 80.0;
    Element::text(Text {
        x: r.x + 21.16 * s,
        y: r.y + 64.67 * s,
        lines: vec!["?".into()],
        line_height: 67.75 * s,
        font_size: 67.75 * s,
        bold: false,
        weight: None,
        anchor: Anchor::Start,
        fill: Color::new("#ffffff"),
        tspan_central: false,
        dy_tspans: false,
        family: Some("ArialMT, Arial".into()),
    })
}

/// `(tag, attributes)` of every self-closing element in `body`.
fn elements(body: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let inner = rest[..end].trim_end_matches('/').trim();
        rest = &rest[end.min(rest.len())..];
        if inner.starts_with('/') {
            continue;
        }
        let (tag, mut a) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        let mut attrs = Vec::new();
        while let Some(eq) = a.find("=\"") {
            let name = a[..eq].trim().to_string();
            let v = &a[eq + 2..];
            let close = v.find('"').unwrap_or(v.len());
            attrs.push((name, v[..close].to_string()));
            a = &v[(close + 1).min(v.len())..];
        }
        out.push((tag.to_string(), attrs));
    }
    out
}

fn element(tag: &str, attrs: &[(String, String)], r: Rect, s: f64) -> Option<Element> {
    let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let num = |k: &str| {
        get(k)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let p = |x: f64, y: f64| Point::new(r.x + x * s, r.y + y * s);
    let paint = paint(get("style").unwrap_or(""), s);
    let geom = match tag {
        "path" => Geometry::Path(path(get("d")?, &p)),
        "rect" => {
            let (x, y, w, h) = (num("x"), num("y"), num("width"), num("height"));
            let rx = if get("rx").is_some() {
                num("rx")
            } else {
                num("ry")
            };
            Geometry::Rect {
                rect: Rect::new(r.x + x * s, r.y + y * s, w * s, h * s),
                rx: rx * s,
            }
        }
        "line" => Geometry::Path(vec![
            PathCmd::MoveTo(p(num("x1"), num("y1"))),
            PathCmd::LineTo(p(num("x2"), num("y2"))),
        ]),
        "circle" => Geometry::Circle {
            c: p(num("cx"), num("cy")),
            r: num("r") * s,
        },
        "ellipse" => Geometry::Path(geom::ellipse(
            p(num("cx"), num("cy")),
            num("rx") * s,
            num("ry") * s,
        )),
        _ => return None,
    };
    Some(Element::shape(geom, paint))
}

/// An inline `style`'s fill, stroke and stroke width (scaled by `s`).
fn paint(style: &str, s: f64) -> Paint {
    let mut fill = Color::none();
    let mut stroke = Color::none();
    let mut width = 1.0;
    for decl in style.split(';') {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "fill" => {
                fill = if v == "none" {
                    Color::none()
                } else {
                    Color::new(v)
                }
            }
            "stroke" => {
                stroke = if v == "none" {
                    Color::none()
                } else {
                    Color::new(v)
                }
            }
            "stroke-width" => {
                width = v.trim_end_matches("px").trim().parse().unwrap_or(1.0);
            }
            _ => {}
        }
    }
    if width == 0.0 {
        stroke = Color::none();
    }
    let mut paint = Paint::stroke(&stroke, width * s);
    paint.fill = fill;
    paint
}

/// SVG path data (`M L H V C S Z`, absolute and relative) through `p`.
fn path(d: &str, p: &dyn Fn(f64, f64) -> Point) -> Vec<PathCmd> {
    // Tokens: command letters and numbers ("-" or a second "." starts a number).
    let mut toks: Vec<Result<char, f64>> = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, toks: &mut Vec<Result<char, f64>>| {
        if !num.is_empty() {
            toks.push(Err(num.parse().unwrap_or(0.0)));
            num.clear();
        }
    };
    for c in d.chars() {
        match c {
            'A'..='Z' | 'a'..='z' if c != 'e' && c != 'E' => {
                flush(&mut num, &mut toks);
                toks.push(Ok(c));
            }
            '-' if !num.ends_with(['e', 'E']) => {
                flush(&mut num, &mut toks);
                num.push(c);
            }
            '.' if num.contains('.') => {
                flush(&mut num, &mut toks);
                num.push(c);
            }
            ',' | ' ' | '\t' | '\n' => flush(&mut num, &mut toks),
            _ => num.push(c),
        }
    }
    flush(&mut num, &mut toks);

    let mut out = Vec::new();
    let (mut cur, mut start) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64));
    let mut last_ctrl: Option<(f64, f64)> = None;
    let mut cmd = 'M';
    let mut i = 0;
    let nums = |i: &mut usize, n: usize| -> Option<Vec<f64>> {
        let v: Vec<f64> = toks[*i..].iter().take(n).map_while(|t| t.err()).collect();
        (v.len() == n).then(|| {
            *i += n;
            v
        })
    };
    while i < toks.len() {
        if let Ok(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                out.push(PathCmd::Close);
                cur = start;
                last_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { cur } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' | 'L' => {
                let Some(v) = nums(&mut i, 2) else { break };
                cur = (ox + v[0], oy + v[1]);
                if cmd.eq_ignore_ascii_case(&'M') {
                    out.push(PathCmd::MoveTo(p(cur.0, cur.1)));
                    start = cur;
                    // Further pairs are line-tos.
                    cmd = if rel { 'l' } else { 'L' };
                } else {
                    out.push(PathCmd::LineTo(p(cur.0, cur.1)));
                }
                last_ctrl = None;
            }
            'H' => {
                let Some(v) = nums(&mut i, 1) else { break };
                cur.0 = ox + v[0];
                out.push(PathCmd::LineTo(p(cur.0, cur.1)));
                last_ctrl = None;
            }
            'V' => {
                let Some(v) = nums(&mut i, 1) else { break };
                cur.1 = oy + v[0];
                out.push(PathCmd::LineTo(p(cur.0, cur.1)));
                last_ctrl = None;
            }
            'C' => {
                let Some(v) = nums(&mut i, 6) else { break };
                let c1 = (ox + v[0], oy + v[1]);
                let c2 = (ox + v[2], oy + v[3]);
                cur = (ox + v[4], oy + v[5]);
                out.push(PathCmd::CubicTo(
                    p(c1.0, c1.1),
                    p(c2.0, c2.1),
                    p(cur.0, cur.1),
                ));
                last_ctrl = Some(c2);
            }
            'S' => {
                let Some(v) = nums(&mut i, 4) else { break };
                let c1 = last_ctrl.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                let c2 = (ox + v[0], oy + v[1]);
                cur = (ox + v[2], oy + v[3]);
                out.push(PathCmd::CubicTo(
                    p(c1.0, c1.1),
                    p(c2.0, c2.1),
                    p(cur.0, cur.1),
                ));
                last_ctrl = Some(c2);
            }
            _ => break,
        }
    }
    out
}

// The built-in architecture icons, as mermaid 12.3.0 defines them
// (`architectureIcons`, without ids), each drawn on an 80 x 80 canvas.
pub const ICONS: [(&str, &str); 5] = [
    (
        "database",
        r##"<path d="m20,57.86c0,3.94,8.95,7.14,20,7.14s20-3.2,20-7.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><path d="m20,45.95c0,3.94,8.95,7.14,20,7.14s20-3.2,20-7.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><path d="m20,34.05c0,3.94,8.95,7.14,20,7.14s20-3.2,20-7.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="40" cy="22.14" rx="20" ry="7.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="20" y1="57.86" x2="20" y2="22.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="60" y1="57.86" x2="60" y2="22.14" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/>"##,
    ),
    (
        "server",
        r##"<rect x="17.5" y="17.5" width="45" height="45" rx="2" ry="2" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="17.5" y1="32.5" x2="62.5" y2="32.5" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="17.5" y1="47.5" x2="62.5" y2="47.5" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><g><path d="m56.25,25c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: #fff; stroke-width: 0px;"/><path d="m56.25,25c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: none; stroke: #fff; stroke-miterlimit: 10;"/></g><g><path d="m56.25,40c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: #fff; stroke-width: 0px;"/><path d="m56.25,40c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: none; stroke: #fff; stroke-miterlimit: 10;"/></g><g><path d="m56.25,55c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: #fff; stroke-width: 0px;"/><path d="m56.25,55c0,.27-.45.5-1,.5h-10.5c-.55,0-1-.23-1-.5s.45-.5,1-.5h10.5c.55,0,1,.23,1,.5Z" style="fill: none; stroke: #fff; stroke-miterlimit: 10;"/></g><g><circle cx="32.5" cy="25" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="27.5" cy="25" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="22.5" cy="25" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/></g><g><circle cx="32.5" cy="40" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="27.5" cy="40" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="22.5" cy="40" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/></g><g><circle cx="32.5" cy="55" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="27.5" cy="55" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/><circle cx="22.5" cy="55" r=".75" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10;"/></g>"##,
    ),
    (
        "disk",
        r##"<rect x="20" y="15" width="40" height="50" rx="1" ry="1" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="24" cy="19.17" rx=".8" ry=".83" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="56" cy="19.17" rx=".8" ry=".83" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="24" cy="60.83" rx=".8" ry=".83" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="56" cy="60.83" rx=".8" ry=".83" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="40" cy="33.75" rx="14" ry="14.58" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><ellipse cx="40" cy="33.75" rx="4" ry="4.17" style="fill: #fff; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><path d="m37.51,42.52l-4.83,13.22c-.26.71-1.1,1.02-1.76.64l-4.18-2.42c-.66-.38-.81-1.26-.33-1.84l9.01-10.8c.88-1.05,2.56-.08,2.09,1.2Z" style="fill: #fff; stroke-width: 0px;"/>"##,
    ),
    (
        "internet",
        r##"<circle cx="40" cy="40" r="22.5" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="40" y1="17.5" x2="40" y2="62.5" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="17.5" y1="40" x2="62.5" y2="40" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><path d="m39.99,17.51c-15.28,11.1-15.28,33.88,0,44.98" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><path d="m40.01,17.51c15.28,11.1,15.28,33.88,0,44.98" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="19.75" y1="30.1" x2="60.25" y2="30.1" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/><line x1="19.75" y1="49.9" x2="60.25" y2="49.9" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/>"##,
    ),
    (
        "cloud",
        r##"<path d="m65,47.5c0,2.76-2.24,5-5,5H20c-2.76,0-5-2.24-5-5,0-1.87,1.03-3.51,2.56-4.36-.04-.21-.06-.42-.06-.64,0-2.6,2.48-4.74,5.65-4.97,1.65-4.51,6.34-7.76,11.85-7.76.86,0,1.69.08,2.5.23,2.09-1.57,4.69-2.5,7.5-2.5,6.1,0,11.19,4.38,12.28,10.17,2.14.56,3.72,2.51,3.72,4.83,0,.03,0,.07-.01.1,2.29.46,4.01,2.48,4.01,4.9Z" style="fill: none; stroke: #fff; stroke-miterlimit: 10; stroke-width: 2px;"/>"##,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_parse_into_their_shapes() {
        let r = Rect::new(0.0, 0.0, 80.0, 80.0);
        let count = |n: &str| builtin(n, r).unwrap().len();
        // database: three arcs, the top ellipse and the two sides; server: frame,
        // 2 lines, 3 bars and 3 × 3 lights.
        assert_eq!(count("database"), 6);
        assert!(count("server") >= 9);
        assert!(builtin("nope", r).is_none());
        // mermaid's server frame: rect 17.5,17.5 45 × 45, rx 2, a 2px white stroke.
        let els = builtin("server", Rect::new(10.0, 20.0, 40.0, 40.0)).unwrap();
        let Element::Shape {
            geom: Geometry::Rect { rect, rx },
            paint,
            ..
        } = &els[0]
        else {
            panic!("{:?}", els[0])
        };
        assert_eq!((rect.x, rect.y, rect.w, *rx), (18.75, 28.75, 22.5, 1.0));
        assert_eq!((paint.stroke.css(), paint.stroke_width), ("#fff", 1.0));
        assert!(paint.fill.is_none());
    }

    #[test]
    fn relative_and_smooth_path_commands() {
        let p = |x: f64, y: f64| Point::new(x, y);
        let cmds = path("m20,57.86c0,3.94,8.95,7.14,20,7.14s20-3.2,20-7.14", &p);
        assert_eq!(cmds[0], PathCmd::MoveTo(p(20.0, 57.86)));
        let PathCmd::CubicTo(a, b, c) = cmds[2] else {
            panic!("{cmds:?}")
        };
        let close = |u: Point, v: Point| (u.x - v.x).abs() < 1e-9 && (u.y - v.y).abs() < 1e-9;
        assert!(close(a, p(51.05, 65.0)) && close(b, p(60.0, 61.8)) && close(c, p(60.0, 57.86)));
        let cmds = path("m56.25,25c0,.27-.45.5-1,.5h-10.5Z", &p);
        let PathCmd::CubicTo(a, b, c) = cmds[1] else {
            panic!("{cmds:?}")
        };
        assert!(close(a, p(56.25, 25.27)) && close(b, p(55.8, 25.5)) && close(c, p(55.25, 25.5)));
        let PathCmd::LineTo(l) = cmds[2] else {
            panic!("{cmds:?}")
        };
        assert!(close(l, p(44.75, 25.5)));
    }
}
