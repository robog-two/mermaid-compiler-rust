//! Iconify icon sets: what mmdc's `--iconPacks` / `--iconPacksNamesAndUrls`
//! register with mermaid (`registerIconPacks`), and how mermaid's icons.ts
//! draws an icon from them with `@iconify/utils` (`stringToIcon`,
//! `getIconData`, `iconToSVG`, `replaceIDs`, `iconToHTML`).
//!
//! The library never loads a pack itself: the caller (the CLI shell) reads or
//! fetches each pack's JSON and hands the text in as [`IconPacks`]. A pack that
//! could not be loaded is kept as failed, so its icons draw mermaid's
//! `unknownIcon`, as mermaid falls back when a pack's loader rejects.
//!
//! An icon is drawn as mermaid inserts it: its own `<svg>` element (the body
//! re-serialised, with scripts, event handlers and `<style>` removed as
//! DOMPurify removes them). Where mermaid positions an icon by its
//! `getBBox()`, [`IconSvg::bbox`] gives the drawn content's box.

use std::fmt;
use std::sync::Arc;

use crate::geom::{Bounds, PathCmd, Point, Rect};
use crate::json::Json;

/// One registered pack: its name (the prefix icons are looked up by) and its
/// parsed Iconify JSON, or why it could not be loaded.
#[derive(Debug, Clone, PartialEq)]
struct Pack {
    name: String,
    set: Result<Json, String>,
}

/// The icon packs registered for a render (none by default).
#[derive(Clone, Default)]
pub struct IconPacks(Arc<Vec<Pack>>);

impl fmt::Debug for IconPacks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|p| &p.name))
            .finish()
    }
}

impl PartialEq for IconPacks {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl IconPacks {
    /// Packs from `(name, JSON text or load error)`, in registration order (a
    /// later pack of the same name replaces an earlier one, as mermaid's store
    /// does). JSON that does not parse counts as a failed load.
    pub fn new(packs: Vec<(String, Result<String, String>)>) -> Self {
        let mut out: Vec<Pack> = Vec::new();
        for (name, text) in packs {
            let set = text.and_then(|t| {
                crate::json::parse(&t)
                    .map_err(|e| format!("Failed to parse icon pack JSON: {e}"))
                    .and_then(|j| match j {
                        Json::Object(_) => Ok(j),
                        _ => Err("Icon pack JSON is not an object".to_string()),
                    })
            });
            out.retain(|p| p.name != name);
            out.push(Pack { name, set });
        }
        IconPacks(Arc::new(out))
    }

    /// Whether no pack is registered.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The names of the registered packs.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|p| p.name.as_str())
    }

    /// mermaid's `getRegisteredIconData` over the registered packs: the icon
    /// `name` (`prefix:name`, `@provider:prefix:name` or `prefix-name`; a bare
    /// name takes `fallback_prefix`) with its aliases resolved, or `None` when
    /// no registered pack has it (mermaid then draws `unknownIcon`, or the
    /// diagram's own built-in icon of that name).
    pub fn lookup(&self, name: &str, fallback_prefix: Option<&str>) -> Option<IconData> {
        let (prefix, icon) = icon_prefix(name, fallback_prefix)?;
        let pack = self.0.iter().rev().find(|p| p.name == prefix)?;
        icon_data(pack.set.as_ref().ok()?, &icon)
    }
}

/// `stringToIcon(name, true, fallback !== undefined)` and the prefix mermaid
/// looks the set up by: `(prefix, name)`, or `None` for an invalid name or one
/// without a prefix.
pub fn icon_prefix(name: &str, fallback_prefix: Option<&str>) -> Option<(String, String)> {
    let allow_simple = fallback_prefix.is_some();
    let mut parts: Vec<&str> = name.split(':').collect();
    let mut provider = "";
    if name.starts_with('@') {
        if parts.len() < 2 || parts.len() > 3 {
            return None;
        }
        provider = &parts.remove(0)[1..];
    }
    if parts.len() > 3 || parts.is_empty() {
        return None;
    }
    let (prefix, icon) = if parts.len() > 1 {
        let icon = parts.pop()?;
        let prefix = parts.pop()?;
        (prefix.to_string(), icon.to_string())
    } else {
        let name = parts[0];
        match name.split_once('-') {
            Some((p, rest)) => (p.to_string(), rest.to_string()),
            None if allow_simple && provider.is_empty() => (String::new(), name.to_string()),
            None => return None,
        }
    };
    // validateIconName
    if icon.is_empty() || (prefix.is_empty() && !allow_simple) {
        return None;
    }
    let prefix = if prefix.is_empty() {
        fallback_prefix.unwrap_or("").to_string()
    } else {
        prefix
    };
    (!prefix.is_empty()).then_some((prefix, icon))
}

/// An icon's data with its aliases and the set's defaults merged in (Iconify's
/// `ExtendedIconifyIcon`, defaults filled).
#[derive(Debug, Clone, PartialEq)]
pub struct IconData {
    pub body: String,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    /// Quarter turns clockwise.
    pub rotate: i64,
    pub h_flip: bool,
    pub v_flip: bool,
}

/// The optional properties of an icon, alias or set (`left` … `hidden`).
#[derive(Debug, Clone, Default, PartialEq)]
struct Props {
    left: Option<f64>,
    top: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    rotate: Option<f64>,
    v_flip: Option<bool>,
    h_flip: Option<bool>,
    body: Option<String>,
    hidden: Option<bool>,
}

impl Props {
    fn of(j: &Json) -> Props {
        let num = |k: &str| {
            j.get(k).and_then(|v| match v {
                Json::Number(n) => Some(*n),
                _ => None,
            })
        };
        // `key in object`: any value counts, JavaScript truthiness decides.
        let flag = |k: &str| j.get(k).map(Json::js_truthy);
        Props {
            left: num("left"),
            top: num("top"),
            width: num("width"),
            height: num("height"),
            rotate: num("rotate"),
            v_flip: flag("vFlip"),
            h_flip: flag("hFlip"),
            body: j.get("body").and_then(Json::as_str).map(str::to_string),
            hidden: flag("hidden"),
        }
    }
}

/// `mergeIconData(parent, child)`.
fn merge(parent: &Props, child: &Props) -> Props {
    // mergeIconTransformations
    let mut r = Props::default();
    if parent.h_flip.unwrap_or(false) != child.h_flip.unwrap_or(false) {
        r.h_flip = Some(true);
    }
    if parent.v_flip.unwrap_or(false) != child.v_flip.unwrap_or(false) {
        r.v_flip = Some(true);
    }
    let rotate = (parent.rotate.unwrap_or(0.0) + child.rotate.unwrap_or(0.0)) % 4.0;
    if rotate != 0.0 && !rotate.is_nan() {
        r.rotate = Some(rotate);
    }
    // A transformation the parent has and the merge cancelled becomes its default.
    if parent.rotate.is_some() && r.rotate.is_none() {
        r.rotate = Some(0.0);
    }
    if parent.v_flip.is_some() && r.v_flip.is_none() {
        r.v_flip = Some(false);
    }
    if parent.h_flip.is_some() && r.h_flip.is_none() {
        r.h_flip = Some(false);
    }
    r.left = child.left.or(parent.left);
    r.top = child.top.or(parent.top);
    r.width = child.width.or(parent.width);
    r.height = child.height.or(parent.height);
    r.body = child.body.clone().or_else(|| parent.body.clone());
    r.hidden = child.hidden.or(parent.hidden);
    r
}

/// `getIconData(set, name)`: the icon or alias `name`, its parents merged in,
/// then the set's defaults.
fn icon_data(set: &Json, name: &str) -> Option<IconData> {
    let icons = set.get("icons")?;
    let aliases = set.get("aliases");
    fn present(j: Option<&Json>) -> Option<&Json> {
        j.filter(|v| matches!(v, Json::Object(_)))
    }
    let icon = |n: &str| present(icons.get(n));
    let alias = |n: &str| present(aliases.and_then(|a| a.get(n)));
    // getIconsTree: the chain of parents up to an icon (none for a loop).
    let mut chain: Vec<String> = Vec::new();
    if icon(name).is_none() {
        let mut cur = name.to_string();
        loop {
            let parent = alias(&cur)?.get("parent")?.as_str()?.to_string();
            if parent == name || chain.contains(&parent) {
                return None;
            }
            chain.push(parent.clone());
            if icon(&parent).is_some() {
                break;
            }
            cur = parent;
        }
    }
    let mut props = Props::default();
    for n in std::iter::once(name).chain(chain.iter().map(String::as_str)) {
        let own = icon(n).or_else(|| alias(n))?;
        props = merge(&Props::of(own), &props);
    }
    let p = merge(&Props::of(set), &props);
    let rotate = p.rotate.unwrap_or(0.0);
    Some(IconData {
        body: p.body.unwrap_or_default(),
        left: p.left.unwrap_or(0.0),
        top: p.top.unwrap_or(0.0),
        width: p.width.unwrap_or(16.0),
        height: p.height.unwrap_or(16.0),
        rotate: if rotate.is_finite() { rotate as i64 } else { 0 },
        h_flip: p.h_flip.unwrap_or(false),
        v_flip: p.v_flip.unwrap_or(false),
    })
}

/// An icon drawn `size` × `size`: its `<svg>` element and, in that element's
/// own pixels, the box `getBBox()` reports for what it draws.
#[derive(Debug, Clone, PartialEq)]
pub struct IconSvg {
    /// `<svg xmlns=… width height viewBox>…</svg>`.
    pub markup: String,
    pub width: f64,
    pub height: f64,
    /// The drawn content's box (`None` when it draws nothing).
    pub bbox: Option<Rect>,
}

/// JavaScript's `Number.prototype.toString` for the values Iconify writes.
fn js(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e21 {
        return format!("{}", v as i64);
    }
    format!("{v}")
}

/// mermaid's `getIconSVG(name, { width: size, height: size })` for a resolved
/// icon: `iconToSVG`, `replaceIDs` (ids prefixed from `uniq`, which should
/// differ between icons of one document) and `iconToHTML`.
pub fn icon_svg(icon: &IconData, size: f64, uniq: &str) -> IconSvg {
    let mut b = (icon.left, icon.top, icon.width, icon.height);
    let mut body = icon.body.clone();
    // iconToSVG: the icon's own transformations (the customisations have none).
    let mut transforms: Vec<String> = Vec::new();
    let mut rotation = icon.rotate;
    if icon.h_flip {
        if icon.v_flip {
            rotation += 2;
        } else {
            transforms.push(format!("translate({} {})", js(b.2 + b.0), js(0.0 - b.1)));
            transforms.push("scale(-1 1)".into());
            b.0 = 0.0;
            b.1 = 0.0;
        }
    } else if icon.v_flip {
        transforms.push(format!("translate({} {})", js(0.0 - b.0), js(b.3 + b.1)));
        transforms.push("scale(1 -1)".into());
        b.0 = 0.0;
        b.1 = 0.0;
    }
    rotation = rotation.rem_euclid(4);
    match rotation {
        1 => {
            let t = b.3 / 2.0 + b.1;
            transforms.insert(0, format!("rotate(90 {} {})", js(t), js(t)));
        }
        2 => transforms.insert(
            0,
            format!(
                "rotate(180 {} {})",
                js(b.2 / 2.0 + b.0),
                js(b.3 / 2.0 + b.1)
            ),
        ),
        3 => {
            let t = b.2 / 2.0 + b.0;
            transforms.insert(0, format!("rotate(-90 {} {})", js(t), js(t)));
        }
        _ => {}
    }
    if rotation % 2 == 1 {
        b = (b.1, b.0, b.3, b.2);
    }
    if !transforms.is_empty() {
        body = wrap_content(
            &body,
            &format!("<g transform=\"{}\">", transforms.join(" ")),
            "</g>",
        );
    }
    let body = replace_ids(&body, &format!("IconifyId{:x}c", fnv(uniq)));
    let tree = parse(&body);
    let mut inner = String::new();
    for n in &tree {
        serialize(n, &mut inner);
    }
    let xlink = if inner.contains("xlink:") {
        " xmlns:xlink=\"http://www.w3.org/1999/xlink\""
    } else {
        ""
    };
    let s = js(size);
    let markup = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"{xlink} width=\"{s}\" height=\"{s}\" viewBox=\"{} {} {} {}\">{inner}</svg>",
        js(b.0),
        js(b.1),
        js(b.2),
        js(b.3)
    );
    // The viewport maps the viewBox in (`xMidYMid meet`).
    let k = if b.2 > 0.0 && b.3 > 0.0 {
        (size / b.2).min(size / b.3)
    } else {
        0.0
    };
    let tx = (size - b.2 * k) / 2.0 - b.0 * k;
    let ty = (size - b.3 * k) / 2.0 - b.1 * k;
    let bbox = content_box(&tree).map(|r| Rect::new(tx + r.x * k, ty + r.y * k, r.w * k, r.h * k));
    IconSvg {
        markup,
        width: size,
        height: size,
        bbox,
    }
}

/// FNV-1a, for deterministic id prefixes.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `wrapSVGContent`: wrap the body in `start` … `end`, keeping its `<defs>`
/// outside.
fn wrap_content(body: &str, start: &str, end: &str) -> String {
    // splitSVGDefs (its `index` is computed once, as in @iconify/utils).
    let mut defs = String::new();
    let mut content = body.to_string();
    let index = content.find("<defs");
    if let Some(index) = index {
        loop {
            let Some(s) = content
                .get(index..)
                .and_then(|c| c.find('>'))
                .map(|i| i + index)
            else {
                break;
            };
            let Some(e) = content.find("</defs") else {
                break;
            };
            let Some(ee) = content[e..].find('>').map(|i| i + e) else {
                break;
            };
            if s + 1 <= e {
                defs.push_str(content[s + 1..e].trim());
            }
            content = format!("{}{}", content[..index].trim(), &content[ee + 1..]);
        }
    }
    let wrapped = format!("{start}{content}{end}");
    if defs.is_empty() {
        wrapped
    } else {
        format!("<defs>{defs}</defs>{wrapped}")
    }
}

/// `replaceIDs`: every `id="…"` renamed `prefix0`, `prefix1`, …, and the
/// references to it (`#id`, `;id`, `"id"` followed by `"`, `)` or `.x`).
fn replace_ids(body: &str, prefix: &str) -> String {
    let mut ids: Vec<String> = Vec::new();
    let mut rest = body;
    // /\sid="(\S+)"/g
    while let Some(i) = rest.find("id=\"") {
        let before = rest[..i].chars().last();
        let after = &rest[i + 4..];
        rest = after;
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let run_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let run = &after[..run_end];
        if let Some(q) = run.rfind('"').filter(|&q| q > 0) {
            ids.push(run[..q].to_string());
            rest = &after[q + 1..];
        }
    }
    if ids.is_empty() {
        return body.to_string();
    }
    const MARK: &str = "\u{1}\u{2}";
    let mut out = body.to_string();
    for (n, id) in ids.iter().enumerate() {
        let new_id = format!("{prefix}{n}{MARK}");
        let mut res = String::with_capacity(out.len());
        let mut i = 0;
        let bytes = out.as_bytes();
        while i < out.len() {
            if matches!(bytes[i], b'#' | b';' | b'"') && out[i + 1..].starts_with(id.as_str()) {
                let after = &out[i + 1 + id.len()..];
                let ok = after.starts_with('"')
                    || after.starts_with(')')
                    || (after.starts_with('.')
                        && after[1..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_lowercase()));
                if ok {
                    res.push(bytes[i] as char);
                    res.push_str(&new_id);
                    i += 1 + id.len();
                    continue;
                }
            }
            let c = out[i..].chars().next().unwrap_or(' ');
            res.push(c);
            i += c.len_utf8();
        }
        out = res;
    }
    out.replace(MARK, "")
}

// ───────────────────────────── markup ─────────────────────────────

/// A parsed element of an icon body.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<Node>,
    },
    Text(String),
}

/// Elements DOMPurify removes (with their content) from an icon.
fn forbidden(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "script" | "style" | "foreignobject" | "iframe" | "object" | "embed" | "noscript"
    )
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A forgiving parse of an icon body into elements and text: unclosed
/// elements close at their parent's end, stray end tags are dropped, comments,
/// processing instructions and doctypes are skipped.
fn parse(body: &str) -> Vec<Node> {
    // Stack of open elements: (tag, attrs, children).
    let mut stack: Vec<(String, Vec<(String, String)>, Vec<Node>)> =
        vec![(String::new(), Vec::new(), Vec::new())];
    let mut rest = body;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut stack, rest);
            break;
        };
        push_text(&mut stack, &rest[..lt]);
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let e = r.find("]]>").unwrap_or(r.len());
            if let Some(top) = stack.last_mut() {
                top.2.push(Node::Text(r[..e].to_string()));
            }
            rest = r.get(e + 3..).unwrap_or("");
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        if let Some(r) = rest.strip_prefix("</") {
            let e = r.find('>').unwrap_or(r.len());
            let tag = r[..e].trim();
            rest = r.get(e + 1..).unwrap_or("");
            if let Some(pos) = stack
                .iter()
                .rposition(|(t, _, _)| t == tag)
                .filter(|&p| p > 0)
            {
                while stack.len() > pos {
                    close(&mut stack);
                }
            }
            continue;
        }
        // A start tag: name, then attributes up to `>` (quotes may hold `>`).
        let s = &rest[1..];
        let name_end = s
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(s.len());
        let tag = s[..name_end].to_string();
        if tag.is_empty() {
            push_text(&mut stack, "<");
            rest = s;
            continue;
        }
        let mut a = &s[name_end..];
        let mut attrs = Vec::new();
        let mut self_closing = false;
        loop {
            a = a.trim_start();
            if a.is_empty() {
                break;
            }
            if let Some(r) = a.strip_prefix("/>") {
                self_closing = true;
                a = r;
                break;
            }
            if let Some(r) = a.strip_prefix('>') {
                a = r;
                break;
            }
            if let Some(r) = a.strip_prefix('/') {
                a = r;
                continue;
            }
            let ne = a
                .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
                .unwrap_or(a.len())
                .max(1);
            let name = a[..ne].to_string();
            a = a[ne..].trim_start();
            let mut value = String::new();
            if let Some(r) = a.strip_prefix('=') {
                let r = r.trim_start();
                if let Some(q) = r.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    let v = &r[1..];
                    let e = v.find(q).unwrap_or(v.len());
                    value = decode_entities(&v[..e]);
                    a = v.get(e + 1..).unwrap_or("");
                } else {
                    let e = r
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(r.len());
                    value = decode_entities(&r[..e]);
                    a = &r[e..];
                }
            }
            if !attrs.iter().any(|(n, _): &(String, String)| *n == name) {
                attrs.push((name, value));
            }
        }
        rest = a;
        stack.push((tag, attrs, Vec::new()));
        if self_closing {
            close(&mut stack);
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().map(|(_, _, c)| c).unwrap_or_default()
}

fn push_text(stack: &mut [(String, Vec<(String, String)>, Vec<Node>)], t: &str) {
    if t.is_empty() {
        return;
    }
    if let Some(top) = stack.last_mut() {
        top.2.push(Node::Text(decode_entities(t)));
    }
}

fn close(stack: &mut Vec<(String, Vec<(String, String)>, Vec<Node>)>) {
    if let Some((tag, attrs, children)) = stack.pop() {
        if forbidden(&tag) {
            return;
        }
        let attrs = attrs
            .into_iter()
            .filter(|(n, v)| {
                let n = n.to_ascii_lowercase();
                let v = v.trim().to_ascii_lowercase();
                !n.starts_with("on")
                    && !((n == "href" || n.ends_with(":href")) && v.starts_with("javascript:"))
            })
            .collect();
        if let Some(top) = stack.last_mut() {
            top.2.push(Node::Element {
                tag,
                attrs,
                children,
            });
        }
    }
}

fn escape(s: &str, attr: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&#160;"),
            _ => out.push(c),
        }
    }
}

/// `XMLSerializer` form: empty elements self-close.
fn serialize(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) => escape(t, false, out),
        Node::Element {
            tag,
            attrs,
            children,
        } => {
            out.push('<');
            out.push_str(tag);
            for (k, v) in attrs {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                escape(v, true, out);
                out.push('"');
            }
            if children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for c in children {
                    serialize(c, out);
                }
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}

// ───────────────────────────── getBBox ─────────────────────────────

/// A 2D affine transform `[a b c d e f]` (SVG `matrix`).
type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn map_rect(m: &Matrix, r: Rect) -> Rect {
    let mut b = Bounds::default();
    for (x, y) in [
        (r.x, r.y),
        (r.right(), r.y),
        (r.x, r.bottom()),
        (r.right(), r.bottom()),
    ] {
        b.add_point(Point::new(
            m[0] * x + m[2] * y + m[4],
            m[1] * x + m[3] * y + m[5],
        ));
    }
    b.rect()
}

/// The numbers in an attribute or function argument list.
fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut p = NumParser {
        s: s.as_bytes(),
        i: 0,
    };
    loop {
        p.skip_sep();
        match p.number() {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out
}

/// A `transform` attribute as one matrix (`None` when it does not parse).
pub fn parse_transform(s: &str) -> Option<Matrix> {
    let mut m = IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let close = rest[open..].find(')')? + open;
        let a = numbers(&rest[open + 1..close]);
        let t: Matrix = match (name, a.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => [*a, *b, *c, *d, *e, *f],
            ("translate", [x]) => [1.0, 0.0, 0.0, 1.0, *x, 0.0],
            ("translate", [x, y]) => [1.0, 0.0, 0.0, 1.0, *x, *y],
            ("scale", [x]) => [*x, 0.0, 0.0, *x, 0.0, 0.0],
            ("scale", [x, y]) => [*x, 0.0, 0.0, *y, 0.0, 0.0],
            ("rotate", [deg]) | ("rotate", [deg, _, _]) => {
                let (s, c) = deg.to_radians().sin_cos();
                let r = [c, s, -s, c, 0.0, 0.0];
                if let [_, cx, cy] = a.as_slice() {
                    mul(
                        &mul(&[1.0, 0.0, 0.0, 1.0, *cx, *cy], &r),
                        &[1.0, 0.0, 0.0, 1.0, -cx, -cy],
                    )
                } else {
                    r
                }
            }
            ("skewX", [deg]) => [1.0, 0.0, deg.to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewY", [deg]) => [1.0, deg.to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => return None,
        };
        m = mul(&m, &t);
        rest = rest[close + 1..]
            .trim_start()
            .trim_start_matches(',')
            .trim_start();
    }
    Some(m)
}

/// Blink's SVG number grammar, for path data, point lists and lengths.
struct NumParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl NumParser<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn number(&mut self) -> Option<f64> {
        let s = self.s;
        let start = self.i;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        let digits = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        let mut any = i > digits;
        if i < s.len() && s[i] == b'.' {
            i += 1;
            let f = i;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
            any |= i > f;
        }
        if !any {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            let e = j;
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
            if j > e {
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }
    /// An arc flag: a single `0` or `1`.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// SVG path data as absolute moves, lines and cubics (quadratics raised,
/// arcs split into cubics of at most a quarter turn). Parsing stops at the
/// first error, keeping what came before, as browsers do.
pub fn parse_path(d: &str) -> Vec<PathCmd> {
    let mut p = NumParser {
        s: d.as_bytes(),
        i: 0,
    };
    let mut out = Vec::new();
    let (mut cur, mut start) = (Point::default(), Point::default());
    let mut last_c: Option<Point> = None; // last cubic control point
    let mut last_q: Option<Point> = None; // last quadratic control point
    let mut cmd = 0u8;
    loop {
        p.skip_ws();
        let Some(&c) = p.s.get(p.i) else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            p.i += 1;
        } else if cmd == 0 {
            break;
        } else {
            p.skip_sep();
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { Point::default() };
        let pt = |p: &mut NumParser, base: Point| -> Option<Point> {
            p.skip_sep();
            let x = p.number()?;
            p.skip_sep();
            let y = p.number()?;
            Some(Point::new(base.x + x, base.y + y))
        };
        let (mut nc, mut nq) = (None, None);
        match cmd.to_ascii_uppercase() {
            b'Z' => {
                out.push(PathCmd::Close);
                cur = start;
                cmd = 0;
                last_c = None;
                last_q = None;
                // A number after `z` is an error; another command may follow.
                p.skip_ws();
                match p.s.get(p.i) {
                    Some(c) if c.is_ascii_alphabetic() => continue,
                    _ => break,
                }
            }
            b'M' => {
                let Some(q) = pt(&mut p, base) else { break };
                out.push(PathCmd::MoveTo(q));
                cur = q;
                start = q;
                // Later pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let Some(q) = pt(&mut p, base) else { break };
                out.push(PathCmd::LineTo(q));
                cur = q;
            }
            b'H' => {
                p.skip_sep();
                let Some(x) = p.number() else { break };
                cur = Point::new(if rel { cur.x + x } else { x }, cur.y);
                out.push(PathCmd::LineTo(cur));
            }
            b'V' => {
                p.skip_sep();
                let Some(y) = p.number() else { break };
                cur = Point::new(cur.x, if rel { cur.y + y } else { y });
                out.push(PathCmd::LineTo(cur));
            }
            b'C' => {
                let (Some(a), Some(b), Some(q)) =
                    (pt(&mut p, base), pt(&mut p, base), pt(&mut p, base))
                else {
                    break;
                };
                out.push(PathCmd::CubicTo(a, b, q));
                nc = Some(b);
                cur = q;
            }
            b'S' => {
                let (Some(b), Some(q)) = (pt(&mut p, base), pt(&mut p, base)) else {
                    break;
                };
                let a = last_c.map_or(cur, |c| Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y));
                out.push(PathCmd::CubicTo(a, b, q));
                nc = Some(b);
                cur = q;
            }
            b'Q' => {
                let (Some(c1), Some(q)) = (pt(&mut p, base), pt(&mut p, base)) else {
                    break;
                };
                out.push(quad(cur, c1, q));
                nq = Some(c1);
                cur = q;
            }
            b'T' => {
                let Some(q) = pt(&mut p, base) else { break };
                let c1 = last_q.map_or(cur, |c| Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y));
                out.push(quad(cur, c1, q));
                nq = Some(c1);
                cur = q;
            }
            b'A' => {
                p.skip_sep();
                let Some(rx) = p.number() else { break };
                p.skip_sep();
                let Some(ry) = p.number() else { break };
                p.skip_sep();
                let Some(phi) = p.number() else { break };
                let (Some(large), Some(sweep)) = (p.flag(), p.flag()) else {
                    break;
                };
                let Some(q) = pt(&mut p, base) else { break };
                out.extend(arc(cur, rx, ry, phi, large, sweep, q));
                cur = q;
            }
            _ => break,
        }
        last_c = nc;
        last_q = nq;
    }
    out
}

fn quad(p0: Point, c: Point, p: Point) -> PathCmd {
    PathCmd::CubicTo(
        Point::new(
            p0.x + 2.0 / 3.0 * (c.x - p0.x),
            p0.y + 2.0 / 3.0 * (c.y - p0.y),
        ),
        Point::new(p.x + 2.0 / 3.0 * (c.x - p.x), p.y + 2.0 / 3.0 * (c.y - p.y)),
        p,
    )
}

/// An SVG elliptical arc as cubics (SVG 1.1 F.6 endpoint-to-centre conversion).
fn arc(p0: Point, rx: f64, ry: f64, phi: f64, large: bool, sweep: bool, p: Point) -> Vec<PathCmd> {
    if p0 == p {
        return Vec::new();
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        return vec![PathCmd::LineTo(p)];
    }
    let (s, c) = phi.to_radians().sin_cos();
    let dx = (p0.x - p.x) / 2.0;
    let dy = (p0.y - p.y) / 2.0;
    let x1 = c * dx + s * dy;
    let y1 = -s * dx + c * dy;
    let l = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if l > 1.0 {
        rx *= l.sqrt();
        ry *= l.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = if den == 0.0 {
        0.0
    } else {
        (num / den).max(0.0).sqrt()
    };
    if large == sweep {
        k = -k;
    }
    let cx1 = k * rx * y1 / ry;
    let cy1 = -k * ry * x1 / rx;
    let cx = c * cx1 - s * cy1 + (p0.x + p.x) / 2.0;
    let cy = s * cx1 + c * cy1 + (p0.y + p.y) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let t1 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = angle(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    }
    let n = (dt.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = dt / n as f64;
    let alpha = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |t: f64| {
        let (st, ct) = t.sin_cos();
        (
            Point::new(
                cx + c * rx * ct - s * ry * st,
                cy + s * rx * ct + c * ry * st,
            ),
            Point::new(-c * rx * st - s * ry * ct, -s * rx * st + c * ry * ct),
        )
    };
    let mut out = Vec::with_capacity(n);
    let mut t = t1;
    for i in 0..n {
        let (a, da) = at(t);
        let t2 = t + step;
        let (mut b, db) = at(t2);
        if i + 1 == n {
            b = p;
        }
        out.push(PathCmd::CubicTo(
            Point::new(a.x + alpha * da.x, a.y + alpha * da.y),
            Point::new(b.x - alpha * db.x, b.y - alpha * db.y),
            b,
        ));
        t = t2;
    }
    out
}

/// Elements that draw nothing themselves (or only through a reference).
fn not_rendered(tag: &str) -> bool {
    matches!(
        tag,
        "defs"
            | "clipPath"
            | "mask"
            | "pattern"
            | "linearGradient"
            | "radialGradient"
            | "marker"
            | "symbol"
            | "filter"
            | "title"
            | "desc"
            | "metadata"
            | "style"
            | "script"
            | "stop"
    )
}

fn attr<'a>(attrs: &'a [(String, String)], k: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn display_none(attrs: &[(String, String)]) -> bool {
    attr(attrs, "display").is_some_and(|v| v.trim() == "none")
        || attr(attrs, "style").is_some_and(|s| {
            s.split(';').any(|d| {
                d.split_once(':')
                    .is_some_and(|(k, v)| k.trim() == "display" && v.trim() == "none")
            })
        })
}

/// `getBBox()` of the body's content, in the icon's own (viewBox) units: each
/// element's box mapped through its `transform` and united, as Blink unites
/// the children of a container.
fn content_box(nodes: &[Node]) -> Option<Rect> {
    let mut b = Bounds::default();
    for n in nodes {
        if let Some(r) = node_box(n, nodes, 0) {
            b.add_rect(r);
        }
    }
    b.get()
}

fn node_box(n: &Node, root: &[Node], depth: usize) -> Option<Rect> {
    let Node::Element {
        tag,
        attrs,
        children,
    } = n
    else {
        return None;
    };
    if depth > 16 || not_rendered(tag) || display_none(attrs) {
        return None;
    }
    let num = |k: &str| attr(attrs, k).and_then(|v| numbers(v).first().copied());
    let n0 = |k: &str| num(k).unwrap_or(0.0);
    let own = match tag.as_str() {
        "g" | "a" | "switch" | "svg" => {
            let mut b = Bounds::default();
            for c in children {
                if let Some(r) = node_box(c, root, depth + 1) {
                    b.add_rect(r);
                }
            }
            let r = b.get()?;
            if tag == "svg" {
                Some(Rect::new(r.x + n0("x"), r.y + n0("y"), r.w, r.h))
            } else {
                Some(r)
            }
        }
        "path" => crate::geom::path_bbox(&parse_path(attr(attrs, "d")?)),
        "rect" | "image" => {
            let (w, h) = (n0("width"), n0("height"));
            (w > 0.0 && h > 0.0).then(|| Rect::new(n0("x"), n0("y"), w, h))
        }
        "circle" => {
            let r = n0("r");
            (r > 0.0).then(|| Rect::new(n0("cx") - r, n0("cy") - r, 2.0 * r, 2.0 * r))
        }
        "ellipse" => {
            let (rx, ry) = (n0("rx"), n0("ry"));
            (rx > 0.0 && ry > 0.0)
                .then(|| Rect::new(n0("cx") - rx, n0("cy") - ry, 2.0 * rx, 2.0 * ry))
        }
        "line" => {
            let mut b = Bounds::default();
            b.add_point(Point::new(n0("x1"), n0("y1")));
            b.add_point(Point::new(n0("x2"), n0("y2")));
            b.get()
        }
        "polyline" | "polygon" => {
            let v = numbers(attr(attrs, "points")?);
            let mut b = Bounds::default();
            for p in v.chunks_exact(2) {
                b.add_point(Point::new(p[0], p[1]));
            }
            b.get()
        }
        "use" => {
            let href = attr(attrs, "href").or_else(|| attr(attrs, "xlink:href"))?;
            let id = href.strip_prefix('#')?;
            let target = find_id(root, id)?;
            let r = match target {
                Node::Element { tag, children, .. } if tag == "symbol" => {
                    let mut b = Bounds::default();
                    for c in children {
                        if let Some(r) = node_box(c, root, depth + 1) {
                            b.add_rect(r);
                        }
                    }
                    b.get()?
                }
                t => node_box(t, root, depth + 1)?,
            };
            Some(Rect::new(r.x + n0("x"), r.y + n0("y"), r.w, r.h))
        }
        _ => None,
    }?;
    Some(match attr(attrs, "transform").and_then(parse_transform) {
        Some(m) => map_rect(&m, own),
        None => own,
    })
}

fn find_id<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    for n in nodes {
        if let Node::Element {
            attrs, children, ..
        } = n
        {
            if attr(attrs, "id") == Some(id) {
                return Some(n);
            }
            if let Some(f) = find_id(children, id) {
                return Some(f);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packs(json: &str) -> IconPacks {
        IconPacks::new(vec![("tst".into(), Ok(json.into()))])
    }

    const SET: &str = r##"{
      "prefix": "tst", "width": 16, "height": 12,
      "icons": {
        "box": { "body": "<rect x=\"2\" y=\"2\" width=\"12\" height=\"8\" fill=\"currentColor\"/>" },
        "arrow": { "body": "<path d=\"M2 6h8V3l4 5l-4 5v-3H2z\"/>", "left": -2, "top": 1, "width": 18, "height": 14 },
        "grad": { "body": "<defs><linearGradient id=\"g\"><stop offset=\"0\"/></linearGradient></defs><rect width=\"4\" height=\"4\" fill=\"url(#g)\"/>" }
      },
      "aliases": {
        "arrow-left": { "parent": "arrow", "hFlip": true },
        "arrow-down": { "parent": "arrow", "rotate": 1 },
        "arrow-up": { "parent": "arrow-down", "vFlip": true, "hFlip": true },
        "box-wide": { "parent": "box", "width": 24 },
        "a": { "parent": "b" }, "b": { "parent": "a" }
      }
    }"##;

    #[test]
    fn names_resolve_as_string_to_icon_does() {
        assert_eq!(
            icon_prefix("tst:box", None),
            Some(("tst".into(), "box".into()))
        );
        assert_eq!(
            icon_prefix("@api:tst:box", None),
            Some(("tst".into(), "box".into()))
        );
        assert_eq!(
            icon_prefix("tst-box-x", None),
            Some(("tst".into(), "box-x".into()))
        );
        assert_eq!(icon_prefix("box", None), None);
        assert_eq!(
            icon_prefix("box", Some("fa")),
            Some(("fa".into(), "box".into()))
        );
        assert_eq!(icon_prefix("box", Some("")), None);
        assert_eq!(icon_prefix("a:b:c:d", None), None);
    }

    #[test]
    fn aliases_merge_dimensions_and_transformations() {
        let p = packs(SET);
        let b = p.lookup("tst:box", None).unwrap();
        assert_eq!((b.width, b.height, b.left), (16.0, 12.0, 0.0));
        assert_eq!(p.lookup("tst:box-wide", None).unwrap().width, 24.0);
        let l = p.lookup("tst:arrow-left", None).unwrap();
        assert!(l.h_flip && !l.v_flip);
        assert_eq!((l.left, l.top, l.width), (-2.0, 1.0, 18.0));
        let u = p.lookup("tst:arrow-up", None).unwrap();
        assert_eq!((u.rotate, u.h_flip, u.v_flip), (1, true, true));
        assert!(p.lookup("tst:a", None).is_none());
        assert!(p.lookup("tst:nope", None).is_none());
        assert!(p.lookup("other:box", None).is_none());
        let failed = IconPacks::new(vec![("tst".into(), Err("offline".into()))]);
        assert!(failed.lookup("tst:box", None).is_none());
    }

    #[test]
    fn svg_markup_follows_icon_to_svg() {
        let p = packs(SET);
        let s = icon_svg(&p.lookup("tst:arrow-left", None).unwrap(), 48.0, "n1");
        assert_eq!(
            s.markup,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"48\" height=\"48\" viewBox=\"0 0 18 14\"><g transform=\"translate(16 -1) scale(-1 1)\"><path d=\"M2 6h8V3l4 5l-4 5v-3H2z\"/></g></svg>"
        );
        let s = icon_svg(&p.lookup("tst:arrow-down", None).unwrap(), 48.0, "n1");
        assert!(s.markup.contains("viewBox=\"1 -2 14 18\""), "{}", s.markup);
        assert!(s.markup.contains("<g transform=\"rotate(90 8 8)\">"));
        let g = icon_svg(&p.lookup("tst:grad", None).unwrap(), 48.0, "n1");
        let id = format!("IconifyId{:x}c0", fnv("n1"));
        assert!(g.markup.contains(&format!("id=\"{id}\"")), "{}", g.markup);
        assert!(g.markup.contains(&format!("url(#{id})")));
    }

    #[test]
    fn content_box_is_the_drawn_extent() {
        let p = packs(SET);
        // box: 12 × 8 at (2, 2) in a 16 × 12 view, 3px per unit, 6px down.
        let s = icon_svg(&p.lookup("tst:box", None).unwrap(), 48.0, "x");
        assert_eq!(s.bbox, Some(Rect::new(6.0, 12.0, 36.0, 24.0)));
        let arc = parse_path("M0 0a5 5 0 0 1 10 0");
        let r = crate::geom::path_bbox(&arc).unwrap();
        assert!(
            (r.y + 5.0).abs() < 1e-9 && (r.w - 10.0).abs() < 1e-9,
            "{r:?}"
        );
        let t = parse_transform("translate(1 1) scale(0.5) rotate(90)").unwrap();
        let r = map_rect(&t, Rect::new(0.0, 0.0, 2.0, 4.0));
        assert!(
            (r.x + 1.0).abs() < 1e-9 && (r.w - 2.0).abs() < 1e-9,
            "{r:?}"
        );
    }

    #[test]
    fn scripts_and_handlers_are_dropped() {
        let nodes = parse(
            "<g onclick=\"x()\"><script>alert(1)</script><a href=\"javascript:x\"><path d=\"M0 0\"/></a></g>",
        );
        let mut s = String::new();
        for n in &nodes {
            serialize(n, &mut s);
        }
        assert_eq!(s, "<g><a><path d=\"M0 0\"/></a></g>");
    }
}
