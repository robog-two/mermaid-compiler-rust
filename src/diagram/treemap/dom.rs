//! mermaid's treemap DOM (`treemap/renderer.ts`), as the serialised svg body.
//!
//! The layout follows d3-hierarchy's `treemap` operation by operation (sum,
//! sort, paddings, squarify, rounding) so the coordinates are mermaid's own.

use super::{Item, Treemap, format_value};
use crate::color;
use crate::diagram::wardley::ID_MARK;
use crate::output::svg::{esc, esc_text, js_num as n};
use crate::text::line_width;
use crate::theme::Palette;
use std::fmt::Write;

const HEADER: f64 = 25.0;
const PAD: f64 = 10.0;

struct Node<'a> {
    name: &'a str,
    class: Option<&'a str>,
    styles: &'a [String],
    own: f64,
    value: f64,
    children: Vec<Node<'a>>,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

fn build<'a>(it: &'a Item, tm: &'a Treemap) -> Node<'a> {
    let styles: &[String] = it
        .class
        .as_ref()
        .and_then(|c| tm.class_styles.iter().find(|(n, _)| n == c))
        .map(|(_, s)| s.as_slice())
        .unwrap_or(&[]);
    Node {
        name: &it.name,
        class: it.class.as_deref(),
        styles,
        own: it.value.unwrap_or(0.0),
        value: 0.0,
        children: it.children.iter().flatten().map(|c| build(c, tm)).collect(),
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    }
}

/// d3 `hierarchy.sum` (own value, then the children from the last) and `.sort`.
fn sum_sort(node: &mut Node) {
    let mut sum = node.own;
    for c in node.children.iter_mut().rev() {
        sum_sort(c);
        sum += c.value;
    }
    node.value = sum;
    node.children.sort_by(|a, b| b.value.total_cmp(&a.value));
}

fn has_kids(node: &Node) -> bool {
    !node.children.is_empty()
}

/// d3's `treemapSquarify` (`squarifyRatio(phi)`) over `node`'s children.
fn tile(node: &mut Node, x0: f64, y0: f64, x1: f64, y1: f64) {
    let ratio = (1.0 + 5f64.sqrt()) / 2.0;
    let (mut x0, mut y0) = (x0, y0);
    let nn = node.children.len();
    let mut value = node.value;
    let (mut i0, mut i1) = (0usize, 0usize);
    while i0 < nn {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let mut sum;
        loop {
            sum = node.children[i1].value;
            i1 += 1;
            if !(sum == 0.0 && i1 < nn) {
                break;
            }
        }
        let mut min_v = sum;
        let mut max_v = sum;
        let alpha = (dy / dx).max(dx / dy) / (value * ratio);
        let mut beta = sum * sum * alpha;
        let mut min_ratio = (max_v / beta).max(beta / min_v);
        while i1 < nn {
            let v = node.children[i1].value;
            sum += v;
            if v < min_v {
                min_v = v;
            }
            if v > max_v {
                max_v = v;
            }
            beta = sum * sum * alpha;
            let new_ratio = (max_v / beta).max(beta / min_v);
            if new_ratio > min_ratio {
                sum -= v;
                break;
            }
            min_ratio = new_ratio;
            i1 += 1;
        }
        let row = &mut node.children[i0..i1];
        if dx < dy {
            let ny = if value != 0.0 {
                y0 + dy * sum / value
            } else {
                y1
            };
            // treemapDice(row, x0, y0, x1, ny)
            let k = if sum != 0.0 { (x1 - x0) / sum } else { 0.0 };
            let mut x = x0;
            for c in row {
                c.y0 = y0;
                c.y1 = ny;
                c.x0 = x;
                x += c.value * k;
                c.x1 = x;
            }
            y0 = ny;
        } else {
            let nx = if value != 0.0 {
                x0 + dx * sum / value
            } else {
                x1
            };
            // treemapSlice(row, x0, y0, nx, y1)
            let k = if sum != 0.0 { (y1 - y0) / sum } else { 0.0 };
            let mut y = y0;
            for c in row {
                c.x0 = x0;
                c.x1 = nx;
                c.y0 = y;
                y += c.value * k;
                c.y1 = y;
            }
            x0 = nx;
        }
        value -= sum;
        i0 = i1;
    }
}

fn position(node: &mut Node, p: f64, inner: f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (node.x0 + p, node.y0 + p, node.x1 - p, node.y1 - p);
    if x1 < x0 {
        x0 = (x0 + x1) / 2.0;
        x1 = x0;
    }
    if y1 < y0 {
        y0 = (y0 + y1) / 2.0;
        y1 = y0;
    }
    node.x0 = x0;
    node.y0 = y0;
    node.x1 = x1;
    node.y1 = y1;
    if has_kids(node) {
        let p = inner / 2.0;
        x0 += PAD - p;
        y0 += HEADER + PAD - p;
        x1 -= PAD - p;
        y1 -= PAD - p;
        if x1 < x0 {
            x0 = (x0 + x1) / 2.0;
            x1 = x0;
        }
        if y1 < y0 {
            y0 = (y0 + y1) / 2.0;
            y1 = y0;
        }
        tile(node, x0, y0, x1, y1);
        for c in &mut node.children {
            position(c, p, inner);
        }
    }
}

fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

fn round_all(node: &mut Node) {
    node.x0 = js_round(node.x0);
    node.y0 = js_round(node.y0);
    node.x1 = js_round(node.x1);
    node.y1 = js_round(node.y1);
    for c in &mut node.children {
        round_all(c);
    }
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

/// `styles2String`: (label styles, node styles, border styles).
fn styles_to_strings(styles: &[String]) -> (String, String, Vec<String>) {
    let mut map: Vec<(String, Option<String>)> = Vec::new();
    for s in styles {
        let mut parts = s.split(':');
        let key = parts.next().unwrap_or("").trim().to_string();
        let val = parts.next().map(|v| v.trim().to_string());
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 = val,
            None => map.push((key, val)),
        }
    }
    let (mut label, mut node, mut border) = (Vec::new(), Vec::new(), Vec::new());
    for (k, v) in &map {
        let joined = format!("{k}:{} !important", v.as_deref().unwrap_or(""));
        if is_label_style(k) {
            label.push(joined);
        } else {
            node.push(joined.clone());
            if k.contains("stroke") {
                border.push(joined);
            }
        }
    }
    (label.join(";"), node.join(";"), border)
}

/// An inline style attribute after a script touched it (CSSOM serialisation).
fn reserialize(style: &str, set: &[(&str, String)]) -> String {
    let mut decls: Vec<(String, String)> = Vec::new();
    for d in style.split(';') {
        let Some((k, v)) = d.split_once(':') else {
            continue;
        };
        let (k, mut v) = (k.trim().to_string(), v.trim().to_string());
        let important = v.ends_with("!important");
        if important {
            v = v.trim_end_matches("!important").trim().to_string();
        }
        if v.starts_with('#')
            && let Some(c) = color::parse(&v)
        {
            v = if c.a < 1.0 {
                format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, n(c.a))
            } else {
                format!("rgb({}, {}, {})", c.r, c.g, c.b)
            };
        }
        if important {
            v.push_str(" !important");
        }
        decls.retain(|(x, _)| *x != k);
        decls.push((k, v));
    }
    for (k, v) in set {
        match decls.iter_mut().find(|(x, _)| x == k) {
            Some(e) => e.1 = v.clone(),
            None => decls.push((k.to_string(), v.clone())),
        }
    }
    decls
        .iter()
        .map(|(k, v)| format!("{k}: {v};"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// mermaid's `valueFormat` selection over d3-format.
fn value_text(v: f64, spec: &str) -> String {
    if v == 0.0 || v.is_nan() {
        return String::new();
    }
    if spec == "$0,0" {
        return format!("${}", format_value(v, ","));
    }
    if spec.starts_with('$') && spec.contains(',') {
        let prec = spec.find('.').map(|i| {
            let d: String = spec[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            format!(".{d}")
        });
        let prec = prec.filter(|p| p.len() > 1).unwrap_or_default();
        return format!("${}", format_value(v, &format!(",{prec}")));
    }
    format_value(v, spec)
}

/// d3 ordinal scale with an implicit domain (keyed by name).
struct Ordinal {
    domain: Vec<String>,
}

impl Ordinal {
    fn index(&mut self, name: &str) -> usize {
        match self.domain.iter().position(|d| d == name) {
            Some(i) => i,
            None => {
                self.domain.push(name.to_string());
                self.domain.len() - 1
            }
        }
    }
}

/// `cScaleLabelN`. The themes spell their black `black` (the palette keeps
/// it as hex).
fn label_color(pal: &Palette, i: usize) -> String {
    match pal.scale_text_color(i).css() {
        "#000000" => "black".to_string(),
        c => c.to_string(),
    }
}

struct Flat<'a> {
    node: &'a Node<'a>,
    depth: usize,
    parent: Option<&'a str>,
}

fn open_text(out: &mut String, attrs: &str, text: &str, style: &str) {
    let _ = write!(out, "<text {attrs} style=\"{}\"", esc(style));
    if text.is_empty() {
        out.push_str("/>");
    } else {
        let _ = write!(out, ">{}</text>", esc_text(text));
    }
}

pub fn markup(
    tm: &Treemap,
    width: f64,
    height: f64,
    inner: f64,
    show_values: bool,
    fmt: &str,
    pal: &Palette,
) -> String {
    let mut root = Node {
        name: "",
        class: None,
        styles: &[],
        own: 0.0,
        value: 0.0,
        children: tm.roots.iter().map(|r| build(r, tm)).collect(),
        x0: 0.0,
        y0: 0.0,
        x1: width,
        y1: height,
    };
    sum_sort(&mut root);
    position(&mut root, 0.0, inner);
    round_all(&mut root);

    // d3 `descendants()` (breadth first) filtered to branches; `leaves()` in pre-order.
    let mut branches: Vec<Flat> = Vec::new();
    let mut queue: Vec<Flat> = vec![Flat {
        node: &root,
        depth: 0,
        parent: None,
    }];
    while !queue.is_empty() {
        let mut next = Vec::new();
        for f in queue {
            if has_kids(f.node) {
                for c in &f.node.children {
                    next.push(Flat {
                        node: c,
                        depth: f.depth + 1,
                        parent: Some(f.node.name),
                    });
                }
                branches.push(f);
            }
        }
        queue = next;
    }
    let mut leaves: Vec<Flat> = Vec::new();
    fn walk<'a>(f: Flat<'a>, out: &mut Vec<Flat<'a>>) {
        if has_kids(f.node) {
            for c in &f.node.children {
                walk(
                    Flat {
                        node: c,
                        depth: f.depth + 1,
                        parent: Some(f.node.name),
                    },
                    out,
                );
            }
        } else {
            out.push(f);
        }
    }
    walk(
        Flat {
            node: &root,
            depth: 0,
            parent: None,
        },
        &mut leaves,
    );

    let title = tm.title.as_deref().filter(|t| !t.is_empty());
    let title_h = if title.is_some() { 30.0 } else { 0.0 };

    let mut fill_scale = Ordinal { domain: vec![] };
    let mut label_scale = Ordinal { domain: vec![] };
    let range = |set: &[crate::scene::Color], i: usize| -> String {
        let k = i % (set.len() + 1);
        if k == 0 {
            "transparent".to_string()
        } else {
            set[k - 1].css().to_string()
        }
    };

    let mut out = String::from("<g/>");
    if let Some(t) = title {
        let _ = write!(
            out,
            "<text x=\"{}\" y=\"{}\" class=\"treemapTitle\" text-anchor=\"middle\" dominant-baseline=\"middle\">{}</text>",
            n(width / 2.0),
            n(title_h / 2.0),
            esc_text(t)
        );
    }
    let _ = write!(
        out,
        "<g transform=\"translate(0, {})\" class=\"treemapContainer\">",
        n(title_h)
    );

    for f in &branches {
        let _ = fill_scale.index(f.node.name);
    }
    for (i, f) in branches.iter().enumerate() {
        let d = f.node;
        let w = d.x1 - d.x0;
        let hidden = f.depth == 0;
        let (ls, ns, bs) = styles_to_strings(d.styles);
        let ls = ls.replacen("color:", "fill:", 1);
        let fi = fill_scale.index(d.name);
        let fill = range(&pal.scale, fi);
        let stroke = range(&pal.scale_peer, fi);
        let lc = if hidden {
            String::new()
        } else {
            let k = label_scale.index(d.name);
            label_color(pal, k)
        };
        let _ = write!(
            out,
            "<g class=\"treemapSection\" transform=\"translate({},{})\">",
            n(d.x0),
            n(d.y0)
        );
        let _ = write!(
            out,
            "<rect width=\"{}\" height=\"25\" class=\"treemapSectionHeader\" fill=\"none\" fill-opacity=\"0.6\" stroke-width=\"0.6\" style=\"{}\"/>",
            n(w),
            if hidden { "display: none;" } else { "" }
        );
        let _ = write!(
            out,
            "<clipPath id=\"clip-section-{ID_MARK}-{i}\"><rect width=\"{}\" height=\"25\"/></clipPath>",
            n((w - 12.0).max(0.0))
        );
        let _ = write!(
            out,
            "<rect width=\"{}\" height=\"{}\" class=\"treemapSection section{i}\" fill=\"{}\" fill-opacity=\"0.6\" stroke=\"{}\" stroke-width=\"2\" stroke-opacity=\"0.4\" style=\"{}\"/>",
            n(w),
            n(d.y1 - d.y0),
            esc(&fill),
            esc(&stroke),
            esc(&if hidden {
                "display: none;".to_string()
            } else {
                format!("{ns};{}", bs.join(";"))
            })
        );
        // the label, fitted into the header
        let mut label = if hidden { "" } else { d.name }.to_string();
        if !hidden {
            let space = if show_values && d.value != 0.0 {
                (w - 10.0) - 30.0 - 10.0 - 6.0
            } else {
                w - 6.0 - 6.0
            };
            let avail = space.max(15.0);
            if line_width(&label, 12.0, true) > avail {
                let chars: Vec<char> = d.name.chars().collect();
                let mut cur = chars.len();
                while cur > 0 {
                    cur -= 1;
                    if cur == 0 {
                        label = "...".to_string();
                        if line_width(&label, 12.0, true) > avail {
                            label.clear();
                        }
                        break;
                    }
                    label = chars[..cur].iter().collect::<String>() + "...";
                    if line_width(&label, 12.0, true) <= avail {
                        break;
                    }
                }
            }
        }
        let attrs = format!(
            "class=\"treemapSectionLabel\" x=\"6\" y=\"12.5\" dominant-baseline=\"middle\" font-weight=\"bold\" clip-path=\"url(#clip-section-{ID_MARK}-{i})\""
        );
        let style = if hidden {
            "display: none;".to_string()
        } else {
            format!(
                "dominant-baseline: middle; font-size: 12px; fill:{lc}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;{ls}"
            )
        };
        open_text(&mut out, &attrs, &label, &style);
        if show_values {
            let attrs = format!(
                "class=\"treemapSectionValue\" x=\"{}\" y=\"12.5\" text-anchor=\"end\" dominant-baseline=\"middle\" font-style=\"italic\"",
                n(w - 10.0)
            );
            let style = if hidden {
                "display: none;".to_string()
            } else {
                format!(
                    "text-anchor: end; dominant-baseline: middle; font-size: 10px; fill:{lc}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;{ls}"
                )
            };
            open_text(&mut out, &attrs, &value_text(d.value, fmt), &style);
        }
        out.push_str("</g>");
    }

    let complex = leaves.len() > 20;
    let (base_label, base_value, min_label, min_value, lpad, min_disp, spacing): (
        f64,
        f64,
        f64,
        f64,
        f64,
        f64,
        f64,
    ) = if complex {
        (16.0, 14.0, 4.0, 4.0, 2.0, 8.0, 1.0)
    } else {
        (38.0, 28.0, 8.0, 6.0, 4.0, 10.0, 2.0)
    };
    for (i, f) in leaves.iter().enumerate() {
        let d = f.node;
        let (w, h) = (d.x1 - d.x0, d.y1 - d.y0);
        let (ls, ns, _) = styles_to_strings(d.styles);
        let ls = ls.replacen("color:", "fill:", 1);
        let color_name = f.parent.unwrap_or(d.name);
        let fill = range(&pal.scale, fill_scale.index(color_name));
        let lc = label_color(pal, label_scale.index(d.name));
        let cls = d.class.map(|c| format!(" {c}")).unwrap_or_default();
        let _ = write!(
            out,
            "<g class=\"treemapNode treemapLeafGroup leaf{i}{}x\" transform=\"translate({},{})\">",
            esc(&cls),
            n(d.x0),
            n(d.y0)
        );
        let _ = write!(
            out,
            "<rect width=\"{}\" height=\"{}\" class=\"treemapLeaf\" fill=\"{}\" style=\"{}\" fill-opacity=\"0.3\" stroke=\"{}\" stroke-width=\"3\"/>",
            n(w),
            n(h),
            esc(&fill),
            esc(&ns),
            esc(&fill)
        );
        let _ = write!(
            out,
            "<clipPath id=\"clip-{ID_MARK}-{i}\"><rect width=\"{}\" height=\"{}\"/></clipPath>",
            n((w - 4.0).max(0.0)),
            n((h - 4.0).max(0.0))
        );

        // label fitting
        let avail_w = w - 2.0 * lpad;
        let avail_h = h - 2.0 * lpad;
        let mut fs = base_label;
        let mut hide = false;
        let scale_v = |fs: f64| min_value.max(base_value.min(js_round(fs * 0.6)));
        if avail_w < min_disp || avail_h < min_disp {
            hide = true;
        } else {
            while line_width(d.name, fs, false) > avail_w && fs > min_label {
                fs -= 1.0;
            }
            let mut pv = scale_v(fs);
            let mut combined = fs + spacing + pv;
            while combined > avail_h && fs > min_label {
                fs -= 1.0;
                pv = scale_v(fs);
                if pv < min_value && fs == min_label {
                    break;
                }
                combined = fs + spacing + pv;
            }
            if complex {
                if fs < min_label || avail_h < min_label {
                    hide = true;
                }
            } else if line_width(d.name, fs, false) > avail_w || fs < min_label || avail_h < fs {
                hide = true;
            }
        }
        let init = format!(
            "text-anchor: middle; dominant-baseline: middle; font-size: {}px;fill:{lc};{ls}",
            n(base_label)
        );
        let mut set: Vec<(&str, String)> = Vec::new();
        if fs != base_label {
            set.push(("font-size", format!("{}px", n(fs))));
        }
        if hide {
            set.push(("display", "none".into()));
        }
        let style = if set.is_empty() {
            init
        } else {
            reserialize(&init, &set)
        };
        let clip = format!("url(#clip-{ID_MARK}-{i})");
        let attrs = format!(
            "class=\"treemapLabel\" x=\"{}\" y=\"{}\"",
            n(w / 2.0),
            n(h / 2.0)
        );
        let _ = write!(
            out,
            "<text {attrs} style=\"{}\" clip-path=\"{clip}\"",
            esc(&style)
        );
        if d.name.is_empty() {
            out.push_str("/>");
        } else {
            let _ = write!(out, ">{}</text>", esc_text(d.name));
        }
        if show_values {
            let vt = value_text(d.value, fmt);
            let init = format!(
                "text-anchor: middle; dominant-baseline: hanging; font-size: {}px;fill:{lc};{ls}",
                n(base_value)
            );
            let mut y = h / 2.0;
            let mut set: Vec<(&str, String)> = Vec::new();
            if hide {
                set.push(("display", "none".into()));
            } else {
                let avs = scale_v(fs);
                if avs != base_value {
                    set.push(("font-size", format!("{}px", n(avs))));
                }
                y = h / 2.0 + fs / 2.0 + spacing;
                let aw = w - 2.0 * lpad;
                if line_width(&vt, avs, false) > aw || y + avs > h - 4.0 || avs < min_value {
                    set.push(("display", "none".into()));
                }
            }
            let style = if set.is_empty() {
                init
            } else {
                reserialize(&init, &set)
            };
            let _ = write!(
                out,
                "<text class=\"treemapValue\" x=\"{}\" y=\"{}\" style=\"{}\" clip-path=\"{clip}\"",
                n(w / 2.0),
                n(y),
                esc(&style)
            );
            if vt.is_empty() {
                out.push_str("/>");
            } else {
                let _ = write!(out, ">{}</text>", esc_text(&vt));
            }
        }
        out.push_str("</g>");
    }
    out.push_str("</g>");
    out
}
