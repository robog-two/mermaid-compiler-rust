//! `treeView-beta` diagrams (mermaid 12.3 `treeView`): directory-like trees with
//! connector lines, optional file/folder icons, `:::class` highlights and `##`
//! descriptions.
//!
//! Pipeline (all pure): [`preprocess_box_drawing`] (├── / └── input → indents) →
//! [`parse`] (a port of the langium grammar's lexer + parser) → [`layout`]
//! (mermaid's `drawTree` row geometry) → [`render`] (scene).

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{PathCmd, Point, Rect, arc_cubics};
use crate::json::Json;
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::Palette;
use std::f64::consts::{FRAC_PI_2, PI};

const KEYWORD: &str = "treeView-beta";
/// Rendered icon size (mermaid `ICON_SIZE`).
pub const ICON_SIZE: f64 = 14.0;
/// Gap between icon and label (mermaid `ICON_GAP`).
pub const ICON_GAP: f64 = 4.0;
/// Gap between the widest label and the description column (mermaid `DESC_GAP`).
pub const DESC_GAP: f64 = 16.0;
/// Prefix of the built-in icon pack.
const BUILTIN_PACK: &str = "mermaid-treeview";

// ───────────────────────────── model ─────────────────────────────

/// File or directory (a trailing `/` on the label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
}

/// One tree node as stored by mermaid's db.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub level: usize,
    pub name: String,
    pub node_type: NodeType,
    /// `icon(...)` annotation: `Some("none")` hides the icon (also for `icon()`).
    pub icon: Option<String>,
    pub css_class: Option<String>,
    pub description: Option<String>,
    pub children: Vec<Node>,
}

/// A parsed diagram. `root` is mermaid's implicit `/` directory, which is drawn too.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeView {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub root: Node,
}

impl TreeView {
    fn new() -> Self {
        TreeView {
            title: None,
            acc_title: None,
            acc_descr: None,
            root: Node {
                level: 0,
                name: "/".into(),
                node_type: NodeType::Directory,
                icon: None,
                css_class: None,
                description: None,
                children: Vec::new(),
            },
        }
    }

    /// mermaid's `db.addNode`: attach below the nearest preceding node with a lower level.
    fn add_node(&mut self, stack: &mut Vec<i64>, node: Node) {
        let level = node.level as i64;
        while stack.len() > 1 && level <= *stack.last().unwrap_or(&-1) {
            stack.pop();
        }
        let depth = stack.len() - 1;
        let mut parent = &mut self.root;
        for _ in 0..depth {
            parent = parent.children.last_mut().expect("stack mirrors tree");
        }
        parent.children.push(node);
        stack.push(level);
    }
}

// ───────────────────────── box-drawing input ─────────────────────────

fn is_box_char(c: char) -> bool {
    matches!(c, '─' | '━' | '│' | '┃' | '└' | '┗' | '├' | '┣')
}
fn is_branch_char(c: char) -> bool {
    matches!(c, '└' | '┗' | '├' | '┣')
}
fn is_metadata_line(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(r) = t.strip_prefix("title") {
        return r.starts_with([' ', '\t']);
    }
    if let Some(r) = t.strip_prefix("accTitle") {
        return r.trim_start_matches([' ', '\t']).starts_with(':');
    }
    if let Some(r) = t.strip_prefix("accDescr") {
        return r.trim_start_matches([' ', '\t']).starts_with([':', '{']);
    }
    false
}
fn is_decoration_only(line: &str) -> bool {
    !line.is_empty()
        && line
            .chars()
            .all(|c| c.is_whitespace() || c == '│' || c == '┃')
}

/// Result of [`preprocess_box_drawing`].
#[derive(Debug, Clone, PartialEq)]
pub struct Preprocessed {
    pub text: String,
    /// Output line (1-based) → original line (1-based); empty when unchanged.
    pub line_map: Vec<(usize, usize)>,
}

/// Port of mermaid's `preprocessBoxDrawing`: converts `├──`/`└──`/`│` (and the heavy
/// variants) to 4-space indentation; indent-based input is returned unchanged.
pub fn preprocess_box_drawing(input: &str) -> Result<Preprocessed> {
    let lines: Vec<&str> = input.split('\n').collect();
    let unchanged = || Preprocessed {
        text: input.to_string(),
        line_map: Vec::new(),
    };
    let Some(kw) = lines.iter().position(|l| l.trim() == KEYWORD) else {
        return Ok(unchanged());
    };
    let skip = |l: &str| {
        l.trim().is_empty()
            || l.trim_start().starts_with("%%")
            || is_metadata_line(l)
            || is_decoration_only(l)
    };
    let content: Vec<String> = lines[kw + 1..]
        .iter()
        .filter(|l| !skip(l))
        .map(|l| l.replace('\t', "    "))
        .collect();
    if !content.iter().any(|l| l.chars().any(is_box_char)) {
        return Ok(unchanged());
    }
    // Segment width: column (in UTF-16 units, as in JS) of the first branch char past column 0.
    let segment = content
        .iter()
        .find_map(|l| {
            let col = l.chars().position(is_branch_char)?;
            (col > 0).then_some(col)
        })
        .unwrap_or(4);

    let mut out: Vec<String> = Vec::new();
    let mut map = Vec::new();
    let mut push = |out: &mut Vec<String>, s: String, orig: usize| {
        out.push(s);
        map.push((out.len(), orig));
    };
    for (i, l) in lines.iter().enumerate().take(kw + 1) {
        push(&mut out, l.to_string(), i + 1);
    }
    for (i, line) in lines.iter().enumerate().skip(kw + 1) {
        let orig = i + 1;
        if line.trim().is_empty() || line.trim_start().starts_with("%%") || is_metadata_line(line) {
            push(&mut out, line.to_string(), orig);
            continue;
        }
        if is_decoration_only(line) {
            continue;
        }
        let norm: Vec<char> = line.replace('\t', "    ").chars().collect();
        if let Some(col) = norm.iter().position(|&c| is_branch_char(c)) {
            let depth = (col as f64 / segment as f64).round() as usize + 1;
            let mut pos = col + 1;
            while pos < norm.len() && matches!(norm[pos], '─' | '━') {
                pos += 1;
            }
            while pos < norm.len() && norm[pos] == ' ' {
                pos += 1;
            }
            let content: String = norm[pos..].iter().collect();
            let content = content.trim_end();
            if content.is_empty() {
                return Err(Error::parse(
                    orig,
                    format!(
                        "Line {orig}: Empty node — expected a filename or directory name after the box-drawing prefix"
                    ),
                ));
            }
            push(&mut out, format!("{}{content}", "    ".repeat(depth)), orig);
        } else if norm.iter().all(|&c| c.is_whitespace() || is_box_char(c)) {
            continue;
        } else if norm.iter().any(|&c| is_box_char(c)) {
            push(&mut out, line.to_string(), orig);
        } else if norm.first().is_some_and(|c| c.is_whitespace()) {
            return Err(Error::parse(
                orig,
                format!(
                    "Line {orig}: Unexpected indentation without box-drawing characters. \
                     In box-drawing format, use ├── or └── prefixes for indented nodes."
                ),
            ));
        } else {
            push(&mut out, line.to_string(), orig);
        }
    }
    Ok(Preprocessed {
        text: out.join("\n"),
        line_map: map,
    })
}

// ───────────────────────────── lexer ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Title(String),
    AccTitle(String),
    AccDescr(String),
    Class(String),
    Icon(String),
    Desc(String),
    Indent(usize),
    Name(String),
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn lead_ws(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}
fn rest_of_line(s: &str) -> usize {
    s.find(['\n', '\r']).unwrap_or(s.len())
}

/// `[ \t]+:::[ \t]*[A-Za-z_][\w-]*`
fn m_class(s: &str) -> Option<usize> {
    let w = lead_ws(s);
    let r = s[w..].strip_prefix(":::")?;
    let r2 = r.trim_start_matches([' ', '\t']);
    let first = r2.chars().next()?;
    if w == 0 || !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let name = 1 + r2[1..]
        .find(|c: char| !(is_word(c) || c == '-'))
        .unwrap_or(r2.len() - 1);
    Some(s.len() - r2.len() + name)
}
/// `[ \t]+icon\([\w-]*(?::[\w-]+)?\)`
fn m_icon(s: &str) -> Option<usize> {
    let w = lead_ws(s);
    let r = s[w..].strip_prefix("icon(")?;
    if w == 0 {
        return None;
    }
    let id = |t: &str| {
        t.find(|c: char| !(is_word(c) || c == '-'))
            .unwrap_or(t.len())
    };
    let mut n = id(r);
    if r[n..].starts_with(':') {
        let m = id(&r[n + 1..]);
        if m > 0 {
            n += 1 + m;
        }
    }
    r[n..].starts_with(')').then(|| s.len() - r.len() + n + 1)
}
/// `[ \t]+##[^\n\r]*`
fn m_desc(s: &str) -> Option<usize> {
    let w = lead_ws(s);
    (w > 0 && s[w..].starts_with("##")).then(|| rest_of_line(s))
}
/// Whether an annotation starts at `s` (the BARE_NAME stop lookahead).
fn annotation_ahead(s: &str) -> bool {
    let w = lead_ws(s);
    if w == 0 {
        return false;
    }
    let r = &s[w..];
    if let Some(r) = r.strip_prefix(":::") {
        let r = r.trim_start_matches([' ', '\t']);
        return r.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    }
    r.starts_with("icon(") || r.starts_with("##")
}
/// BARE_NAME.
fn m_bare(s: &str) -> Option<usize> {
    let first = s.chars().next()?;
    if matches!(first, ' ' | '\t' | '\n' | '\r' | '"' | '\'')
        || s.starts_with(":::")
        || s.starts_with("icon(")
        || s.starts_with("##")
    {
        return None;
    }
    let mut end = first.len_utf8();
    while end < s.len() {
        let r = &s[end..];
        if r.starts_with(['\n', '\r']) || annotation_ahead(r) {
            break;
        }
        end += r.chars().next().map_or(1, char::len_utf8);
    }
    Some(end)
}
/// `[\t ]*title(?:[\t ]…)?` — only when `title` is a whole word (see module docs).
fn m_meta(s: &str) -> Option<(usize, Tok)> {
    let w = lead_ws(s);
    let r = &s[w..];
    let line = rest_of_line(s);
    let value = |from: usize| {
        let v = &s[from..line];
        let v = v.find("%%").map_or(v, |i| &v[..i]);
        v.trim().to_string()
    };
    if let Some(t) = r.strip_prefix("accTitle") {
        let t2 = t.trim_start_matches([' ', '\t']);
        if let Some(v) = t2.strip_prefix(':') {
            let from = s.len() - v.len();
            return Some((line, Tok::AccTitle(value(from))));
        }
    }
    if let Some(t) = r.strip_prefix("accDescr") {
        let t2 = t.trim_start_matches([' ', '\t']);
        if let Some(v) = t2.strip_prefix(':') {
            let from = s.len() - v.len();
            return Some((line, Tok::AccDescr(value(from))));
        }
        let t3 = t.trim_start();
        if let Some(v) = t3.strip_prefix('{') {
            let close = v.find('}')?;
            let end = s.len() - v.len() + close + 1;
            return Some((end, Tok::AccDescr(v[..close].trim().to_string())));
        }
    }
    if let Some(t) = r.strip_prefix("title") {
        if t.is_empty() || t.starts_with([' ', '\t', '\n', '\r']) {
            let from = s.len() - t.len();
            return Some((line.max(from), Tok::Title(value(from.min(line)))));
        }
    }
    None
}

/// 1-based line and column of byte offset `off`.
fn line_col(src: &str, off: usize) -> (usize, usize) {
    let before = &src[..off];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// Tokenise everything after the keyword (`start` is a byte offset into `src`).
fn lex(src: &str, start: usize) -> Result<Vec<(usize, Tok)>> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos < src.len() {
        let s = &src[pos..];
        if let Some((n, t)) = m_meta(s) {
            out.push((pos, t));
            pos += n.max(1);
            continue;
        }
        if let Some(n) = m_class(s) {
            out.push((pos, Tok::Class(s[..n].trim()[3..].trim().to_string())));
            pos += n;
            continue;
        }
        if let Some(n) = m_icon(s) {
            let t = s[..n].trim();
            out.push((pos, Tok::Icon(t[5..t.len() - 1].to_string())));
            pos += n;
            continue;
        }
        if let Some(n) = m_desc(s) {
            out.push((pos, Tok::Desc(s[..n].trim()[2..].trim().to_string())));
            pos += n;
            continue;
        }
        let w = lead_ws(s);
        if w > 0 {
            out.push((pos, Tok::Indent(w)));
            pos += w;
            continue;
        }
        let q = s.chars().next().unwrap_or(' ');
        if q == '"' || q == '\'' {
            if let Some(close) = s[1..].find(q) {
                out.push((pos, Tok::Name(s[1..1 + close].to_string())));
                pos += close + 2;
                continue;
            }
        }
        if s.starts_with("%%") {
            pos += rest_of_line(s);
            continue;
        }
        if s.starts_with("\r\n") {
            pos += 2;
            continue;
        }
        if s.starts_with('\n') {
            pos += 1;
            continue;
        }
        if let Some(n) = m_bare(s) {
            let name = s[..n].trim_end_matches([' ', '\t']).to_string();
            out.push((pos, Tok::Name(name)));
            pos += n;
            continue;
        }
        let (line, col) = line_col(src, pos);
        return Err(Error::parse(
            line,
            format!(
                "Lexer error on line {line}, column {col}: unexpected character: ->{q}<- at offset: {pos}, skipped 1 characters."
            ),
        ));
    }
    Ok(out)
}

// ───────────────────────────── parser ─────────────────────────────

fn tok_text(src: &str, off: usize) -> String {
    src[off..off + rest_of_line(&src[off..])].to_string()
}

/// Parse a `treeView-beta` body (box-drawing or indentation format).
pub fn parse(body: &str) -> Result<TreeView> {
    let pre = preprocess_box_drawing(body)?;
    let remap = |line: usize| {
        pre.line_map
            .iter()
            .find(|(o, _)| *o == line)
            .map_or(line, |(_, orig)| *orig)
    };
    parse_indented(&pre.text).map_err(|e| match e {
        Error::Parse { line, message } => Error::parse(remap(line), message),
        other => other,
    })
}

fn parse_indented(src: &str) -> Result<TreeView> {
    let lead = src.len() - src.trim_start().len();
    let rest = &src[lead..];
    let kw_ok = rest
        .strip_prefix(KEYWORD)
        .is_some_and(|r| r.is_empty() || r.starts_with("%%") || r.starts_with(char::is_whitespace));
    if !kw_ok {
        let (line, col) = line_col(src, lead);
        return Err(Error::parse(
            line,
            format!("Parse error on line {line}, column {col}: Expecting 'treeView-beta'"),
        ));
    }
    let toks = lex(src, lead + KEYWORD.len())?;

    let mut tv = TreeView::new();
    let mut stack: Vec<i64> = vec![-1];
    let mut pending: Option<Node> = None;
    let mut seen_node = false;
    let mut i = 0;
    let unexpected = |off: usize, what: &str| {
        let (line, col) = line_col(src, off);
        Error::parse(
            line,
            format!(
                "Parse error on line {line}, column {col}: {what} but found `{}`.",
                tok_text(src, off)
            ),
        )
    };
    while i < toks.len() {
        let (off, tok) = &toks[i];
        match tok {
            Tok::Title(_) | Tok::AccTitle(_) | Tok::AccDescr(_) if seen_node => {
                return Err(unexpected(*off, "Expecting end of file"));
            }
            Tok::Title(t) => tv.title = Some(t.clone()),
            Tok::AccTitle(t) => tv.acc_title = Some(t.clone()),
            Tok::AccDescr(t) => tv.acc_descr = Some(t.clone()),
            Tok::Class(_) | Tok::Icon(_) | Tok::Desc(_) => {
                let Some(n) = pending.as_mut() else {
                    return Err(unexpected(*off, "Expecting a node name"));
                };
                match tok {
                    Tok::Class(c) => n.css_class = Some(c.clone()).filter(|c| !c.is_empty()),
                    Tok::Icon(c) => {
                        n.icon = Some(if c.is_empty() {
                            "none".into()
                        } else {
                            c.clone()
                        })
                    }
                    Tok::Desc(d) => n.description = Some(d.clone()).filter(|d| !d.is_empty()),
                    _ => {}
                }
            }
            Tok::Indent(_) | Tok::Name(_) => {
                let (level, name) = match tok {
                    Tok::Indent(level) => match toks.get(i + 1) {
                        Some((_, Tok::Name(name))) => {
                            i += 1;
                            (*level, name.clone())
                        }
                        Some((o, _)) => {
                            return Err(unexpected(*o, "Expecting a node name"));
                        }
                        None => break,
                    },
                    Tok::Name(name) => (0, name.clone()),
                    _ => unreachable!(),
                };
                if let Some(n) = pending.take() {
                    tv.add_node(&mut stack, n);
                }
                seen_node = true;
                let (name, node_type) = match name.strip_suffix('/') {
                    Some(n) => (n.to_string(), NodeType::Directory),
                    None => (name, NodeType::File),
                };
                pending = Some(Node {
                    level,
                    name,
                    node_type,
                    icon: None,
                    css_class: None,
                    description: None,
                    children: Vec::new(),
                });
            }
        }
        i += 1;
    }
    if let Some(n) = pending.take() {
        tv.add_node(&mut stack, n);
    }
    Ok(tv)
}

// ───────────────────────────── icons ─────────────────────────────

/// Which icon a node shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconRef {
    /// Built-in `mermaid-treeview:file`.
    File,
    /// Built-in `mermaid-treeview:folder`.
    Folder,
    /// Any other iconify reference (`pack:name`). No packs are bundled, so these
    /// draw mermaid's "unknown icon" placeholder, as mermaid-cli does without packs.
    External(String),
}

/// Icon-related `treeView` config.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IconConfig {
    pub show_icons: bool,
    pub default_icon_pack: String,
    pub filename_icons: Vec<(String, String)>,
    pub extension_icons: Vec<(String, String)>,
}

fn qualify_icon(icon: &str, pack: &str) -> IconRef {
    let full = if icon.contains(':') {
        icon.to_string()
    } else if icon == "file" || icon == "folder" || pack.is_empty() {
        format!("{BUILTIN_PACK}:{icon}")
    } else {
        format!("{pack}:{icon}")
    };
    match full.as_str() {
        "mermaid-treeview:file" => IconRef::File,
        "mermaid-treeview:folder" => IconRef::Folder,
        _ => IconRef::External(full),
    }
}

/// mermaid's `detectIcon`: exact filename first, then the (lower-cased) extension.
pub fn detect_icon(name: &str, cfg: &IconConfig) -> Option<String> {
    if let Some((_, v)) = cfg.filename_icons.iter().rev().find(|(k, _)| k == name) {
        if !v.is_empty() {
            return Some(v.clone());
        }
    }
    let dot = name.rfind('.').filter(|&i| i > 0)?;
    let ext = name[dot..].to_lowercase();
    let find = |k: &str| {
        cfg.extension_icons
            .iter()
            .rev()
            .find(|(e, _)| e == k)
            .map(|(_, v)| v.clone())
    };
    find(&ext).or_else(|| find(&ext[1..]))
}

/// mermaid's `getNodeIcon`.
pub fn node_icon(node: &Node, cfg: &IconConfig) -> Option<IconRef> {
    match node.icon.as_deref() {
        Some("none") => return None,
        Some(i) if !i.is_empty() => return Some(qualify_icon(i, &cfg.default_icon_pack)),
        _ => {}
    }
    if !cfg.show_icons {
        return None;
    }
    if node.node_type == NodeType::File {
        match detect_icon(&node.name, cfg).as_deref() {
            Some("none") => return None,
            Some(d) if !d.is_empty() => return Some(qualify_icon(d, &cfg.default_icon_pack)),
            _ => {}
        }
    }
    Some(match node.node_type {
        NodeType::Directory => IconRef::Folder,
        NodeType::File => IconRef::File,
    })
}

// ───────────────────────────── layout ─────────────────────────────

/// Geometry config (`treeView` section) plus the label font size.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConfig {
    pub row_indent: f64,
    pub padding_x: f64,
    pub padding_y: f64,
    pub line_thickness: f64,
    pub font_size: f64,
    pub icons: IconConfig,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            row_indent: 10.0,
            padding_x: 5.0,
            padding_y: 5.0,
            line_thickness: 1.0,
            font_size: 16.0,
            icons: IconConfig::default(),
        }
    }
}

/// A positioned row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub name: String,
    pub is_dir: bool,
    pub classes: Vec<String>,
    pub icon: Option<IconRef>,
    pub description: Option<String>,
    /// mermaid's `node.BBox`.
    pub bbox: Rect,
    pub label_x: f64,
    pub label_right: f64,
    pub center_y: f64,
    /// Highlight background (`:::highlight`).
    pub highlight: Option<Rect>,
}

/// A laid-out tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub rows: Vec<Row>,
    /// Connector lines in drawing order.
    pub lines: Vec<(Point, Point)>,
    /// x of the description column, when any node has a description.
    pub desc_x: Option<f64>,
    pub width: f64,
    pub height: f64,
}

/// Browser `getBBox().height` of one line of text: 17.02px at 16px, one pixel more
/// when a glyph descends below the baseline (measured from Chromium).
pub fn text_height(s: &str, font_size: f64) -> f64 {
    let desc = s.chars().any(|c| "gjpqy(),;[]{}|_@Q".contains(c));
    font_size * (17.015625 + if desc { 0.984375 } else { 0.0 }) / 16.0
}

/// mermaid's `drawTree`: one row per node in pre-order.
pub fn layout(tv: &TreeView, cfg: &LayoutConfig) -> Layout {
    struct St<'a> {
        cfg: &'a LayoutConfig,
        rows: Vec<Row>,
        lines: Vec<(Point, Point)>,
        total_w: f64,
        total_h: f64,
    }
    fn walk(st: &mut St, node: &Node, depth: usize) {
        let c = st.cfg;
        let x = depth as f64 * (c.row_indent + c.padding_x);
        let y = st.total_h;
        let icon = node_icon(node, &c.icons);
        let icon_off = if icon.is_some() {
            ICON_SIZE + ICON_GAP
        } else {
            0.0
        };
        let is_dir = node.node_type == NodeType::Directory;
        let label_w = line_width(&node.name, c.font_size, is_dir);
        let h = text_height(&node.name, c.font_size) + c.padding_y * 2.0;
        let label_x = x + c.padding_x + icon_off;
        let w = label_w + c.padding_x * 2.0 + icon_off;
        let classes: Vec<String> = node
            .css_class
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let cy = y + h / 2.0;
        st.rows.push(Row {
            depth,
            name: node.name.clone(),
            is_dir,
            highlight: classes
                .iter()
                .any(|c| c == "highlight")
                .then(|| Rect::new(x, y + 1.0, 0.0, h - 2.0)),
            classes,
            icon,
            description: node.description.clone(),
            bbox: Rect::new(x, y, w, h),
            label_x,
            label_right: label_x + label_w,
            center_y: cy,
        });
        st.lines
            .push((Point::new(x - c.row_indent, cy), Point::new(x, cy)));
        st.total_w = st.total_w.max(x + w);
        st.total_h += h;
        let mut last: Option<Rect> = None;
        for child in &node.children {
            let idx = st.rows.len();
            walk(st, child, depth + 1);
            last = Some(st.rows[idx].bbox);
        }
        if let Some(end) = last {
            let lx = x + c.padding_x;
            st.lines.push((
                Point::new(lx, y + h),
                Point::new(lx, end.y + end.h / 2.0 + c.line_thickness / 2.0),
            ));
        }
    }
    let mut st = St {
        cfg,
        rows: Vec::new(),
        lines: Vec::new(),
        total_w: 0.0,
        total_h: 0.0,
    };
    walk(&mut st, &tv.root, 0);

    let mut desc_x = None;
    if st.rows.iter().any(|r| r.description.is_some()) {
        let max_right = st
            .rows
            .iter()
            .map(|r| r.label_right)
            .fold(f64::MIN, f64::max);
        let dx = max_right + DESC_GAP;
        desc_x = Some(dx);
        for r in &st.rows {
            if let Some(d) = &r.description {
                st.total_w = st
                    .total_w
                    .max(dx + line_width(d, cfg.font_size, false) + cfg.padding_x);
            }
        }
    }
    for r in &mut st.rows {
        if let Some(hl) = r.highlight.as_mut() {
            hl.w = st.total_w - r.bbox.x + 8.0;
            st.total_w = st.total_w.max(r.bbox.x + hl.w + 2.0);
        }
    }
    Layout {
        rows: st.rows,
        lines: st.lines,
        desc_x,
        width: st.total_w,
        height: st.total_h,
    }
}

// ───────────────────────────── drawing ─────────────────────────────

/// Colours from `themeVariables.treeView` (mermaid's style defaults otherwise).
#[derive(Debug, Clone, PartialEq)]
pub struct Colors {
    pub label: Color,
    pub line: Color,
    pub icon: Color,
    pub description: Color,
    pub highlight_bg: Color,
    pub highlight_stroke: Color,
}

fn theme_var(cfg: &MermaidConfig, key: &str) -> Option<String> {
    cfg.raw.iter().rev().find_map(|j| {
        j.get("themeVariables")?
            .get("treeView")?
            .get(key)?
            .to_scalar_string()
    })
}

fn string_map(j: Option<&Json>) -> Vec<(String, String)> {
    j.and_then(Json::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.to_scalar_string()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the `treeView` config section and theme variables.
pub fn config(cfg: &MermaidConfig) -> (LayoutConfig, Colors) {
    let d = LayoutConfig::default();
    let f = |k: &str, dv: f64| cfg.section_f64("treeView", k, dv).max(0.0);
    let font_size = theme_var(cfg, "labelFontSize")
        .and_then(|s| Json::String(s).as_f64())
        .unwrap_or(d.font_size);
    let lc = LayoutConfig {
        row_indent: f("rowIndent", d.row_indent),
        padding_x: f("paddingX", d.padding_x),
        padding_y: f("paddingY", d.padding_y),
        line_thickness: f("lineThickness", d.line_thickness),
        font_size,
        icons: IconConfig {
            show_icons: cfg.section_bool("treeView", "showIcons", false),
            default_icon_pack: cfg
                .section("treeView", "defaultIconPack")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
            filename_icons: string_map(cfg.section("treeView", "filenameIcons")),
            extension_icons: string_map(cfg.section("treeView", "extensionIcons")),
        },
    };
    let col = |k: &str, dv: &str| Color::new(theme_var(cfg, k).unwrap_or_else(|| dv.into()));
    let colors = Colors {
        label: col("labelColor", "black"),
        line: col("lineColor", "black"),
        icon: col("iconColor", "#546e7a"),
        description: col("descriptionColor", "#6a9955"),
        highlight_bg: col("highlightBg", "rgba(255, 193, 7, 0.15)"),
        highlight_stroke: col("highlightStroke", "#ffc107"),
    };
    (lc, colors)
}

/// The built-in icons (24×24 artwork) scaled to `ICON_SIZE` at `(x, y)`.
fn builtin_icon(kind: &IconRef, x: f64, y: f64) -> Vec<PathCmd> {
    let k = ICON_SIZE / 24.0;
    let p = |px: f64, py: f64| Point::new(x + px * k, y + py * k);
    let arc = |cx: f64, cy: f64, r: f64, a0: f64, a1: f64| arc_cubics(p(cx, cy), r * k, a0, a1);
    let mut v = Vec::new();
    match kind {
        IconRef::Folder => {
            v.push(PathCmd::MoveTo(p(10.59, 4.59)));
            v.push(PathCmd::LineTo(p(9.17, 4.0)));
            v.push(PathCmd::LineTo(p(4.0, 4.0)));
            v.extend(arc(4.0, 6.0, 2.0, -FRAC_PI_2, -PI));
            v.push(PathCmd::LineTo(p(2.0, 18.0)));
            v.extend(arc(4.0, 18.0, 2.0, PI, FRAC_PI_2));
            v.push(PathCmd::LineTo(p(20.0, 20.0)));
            v.extend(arc(20.0, 18.0, 2.0, FRAC_PI_2, 0.0));
            v.push(PathCmd::LineTo(p(22.0, 8.0)));
            v.extend(arc(20.0, 8.0, 2.0, 0.0, -FRAC_PI_2));
            v.push(PathCmd::LineTo(p(12.83, 6.0)));
            v.push(PathCmd::Close);
        }
        _ => {
            v.push(PathCmd::MoveTo(p(6.0, 2.0)));
            v.extend(arc(6.0, 4.0, 2.0, -FRAC_PI_2, -PI));
            v.push(PathCmd::LineTo(p(4.0, 20.0)));
            v.extend(arc(6.0, 20.0, 2.0, PI, FRAC_PI_2));
            v.push(PathCmd::LineTo(p(18.0, 22.0)));
            v.extend(arc(18.0, 20.0, 2.0, FRAC_PI_2, 0.0));
            v.push(PathCmd::LineTo(p(20.0, 8.83)));
            v.push(PathCmd::LineTo(p(19.41, 7.41)));
            v.push(PathCmd::LineTo(p(14.59, 2.59)));
            v.push(PathCmd::LineTo(p(13.17, 2.0)));
            v.push(PathCmd::Close);
            // The folded corner, wound the other way so it cuts a hole.
            v.push(PathCmd::MoveTo(p(13.5, 3.9)));
            v.push(PathCmd::LineTo(p(18.1, 8.5)));
            v.push(PathCmd::LineTo(p(14.5, 8.5)));
            v.extend(arc(14.5, 7.5, 1.0, FRAC_PI_2, PI));
            v.push(PathCmd::Close);
        }
    }
    v
}

fn icon_elements(icon: &IconRef, x: f64, y: f64, color: &Color) -> Vec<Element> {
    match icon {
        IconRef::File | IconRef::Folder => vec![Element::classed_shape(
            Geometry::Path(builtin_icon(icon, x, y)),
            Paint::fill(color),
            "treeView-node-icon",
        )],
        IconRef::External(_) => {
            // mermaid's `unknownIcon` (80×80 artwork): a blue square with a white "?".
            let k = ICON_SIZE / 80.0;
            vec![Element::group(
                "treeView-node-icon",
                vec![
                    Element::rect(
                        Rect::new(x, y, ICON_SIZE, ICON_SIZE),
                        0.0,
                        Paint::fill(&Color::new("#087ebf")),
                    ),
                    Element::text(Text {
                        x: x + 21.16 * k,
                        y: y + 64.67 * k,
                        lines: vec!["?".into()],
                        line_height: 67.75 * k,
                        font_size: 67.75 * k,
                        bold: false,
                        anchor: Anchor::Start,
                        fill: Color::new("#fff"),
                    }),
                ],
            )]
        }
    }
}

/// Draw a laid-out tree.
pub fn draw(lay: &Layout, cfg: &LayoutConfig, colors: &Colors, pal: &Palette) -> Scene {
    let fs = cfg.font_size;
    let label_text = |x: f64, cy: f64, s: &str, fill: &Color| Text {
        x,
        y: cy + fs * 0.35,
        lines: vec![s.to_string()],
        line_height: fs * 1.5,
        font_size: fs,
        bold: false,
        anchor: Anchor::Start,
        fill: fill.clone(),
    };
    let line_paint = Paint::stroke(&colors.line, cfg.line_thickness);
    let mut els = Vec::new();
    // mermaid appends each node's group, then its connector, then (after the
    // children) the vertical trunk; order only matters for overlap, so keep it.
    for row in &lay.rows {
        let mut g = Vec::new();
        if let Some(hl) = row.highlight {
            g.push(Element::classed_shape(
                Geometry::Rect { rect: hl, rx: 3.0 },
                Paint::both(&colors.highlight_bg, &colors.highlight_stroke, 1.0),
                "treeView-highlight-bg",
            ));
        }
        if let Some(icon) = &row.icon {
            g.extend(icon_elements(
                icon,
                row.bbox.x + cfg.padding_x,
                row.bbox.y + cfg.padding_y,
                &colors.icon,
            ));
        }
        let mut class = String::from("treeView-node-label");
        if row.is_dir {
            class.push_str(" treeView-node-dir");
        }
        for c in &row.classes {
            class.push(' ');
            class.push_str(c);
        }
        let mut t = label_text(row.label_x, row.center_y, &row.name, &colors.label);
        t.bold = row.is_dir;
        g.push(Element::classed_text(t, class));
        if let (Some(d), Some(dx)) = (&row.description, lay.desc_x) {
            g.push(Element::classed_text(
                label_text(dx, row.center_y, d, &colors.description),
                "treeView-node-description",
            ));
        }
        els.push(Element::group("treeView-node", g));
    }
    for (a, b) in &lay.lines {
        els.push(Element::classed_shape(
            Geometry::Path(vec![PathCmd::MoveTo(*a), PathCmd::LineTo(*b)]),
            line_paint.clone(),
            "treeView-node-line",
        ));
    }
    Scene {
        kind: DiagramKind::TreeView,
        view_box: Rect::new(
            -cfg.line_thickness / 2.0,
            0.0,
            lay.width.max(1.0),
            lay.height.max(1.0),
        ),
        elements: vec![Element::group("tree-view", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: ".treeView-node-label,.treeView-node-description{white-space:pre;}\
              .treeView-node-description{font-style:italic;}"
            .into(),
    }
}

/// Parse, lay out and draw a `treeView` diagram.
pub fn render(
    body: &str,
    _title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let tv = parse(body)?;
    let (lc, colors) = config(cfg);
    let lay = layout(&tv, &lc);
    let mut scene = draw(&lay, &lc, &colors, pal);
    scene.acc_title = tv.acc_title;
    scene.acc_descr = tv.acc_descr;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &Node) -> Vec<String> {
        n.children.iter().map(|c| c.name.clone()).collect()
    }

    #[test]
    fn parses_indented_hierarchy() {
        let tv = parse(
            "treeView-beta\n    my-project/\n        src/\n            index.js\n        package.json\n        README.md\n",
        )
        .unwrap();
        assert_eq!(names(&tv.root), vec!["my-project"]);
        let p = &tv.root.children[0];
        assert_eq!(p.node_type, NodeType::Directory);
        assert_eq!(names(p), vec!["src", "package.json", "README.md"]);
        assert_eq!(names(&p.children[0]), vec!["index.js"]);
        assert_eq!(p.children[0].children[0].node_type, NodeType::File);
    }

    #[test]
    fn quoted_names_and_inner_spaces() {
        let tv = parse(
            "treeView-beta\n    \"my project\"\n        'folder with spaces/'\n        b   c  \n",
        )
        .unwrap();
        let p = &tv.root.children[0];
        assert_eq!(p.name, "my project");
        assert_eq!(p.children[0].name, "folder with spaces");
        assert_eq!(p.children[0].node_type, NodeType::Directory);
        assert_eq!(p.children[1].name, "b   c");
    }

    #[test]
    fn annotations_in_any_order() {
        let tv = parse(
            "treeView-beta\n  App.tsx :::highlight icon(logos:react) ## main component\n  b ## x ## y\n  c icon() :::foo\n  d icon(none)\n",
        )
        .unwrap();
        let c = &tv.root.children;
        assert_eq!(c[0].css_class.as_deref(), Some("highlight"));
        assert_eq!(c[0].icon.as_deref(), Some("logos:react"));
        assert_eq!(c[0].description.as_deref(), Some("main component"));
        assert_eq!(c[1].description.as_deref(), Some("x ## y"));
        assert_eq!(c[2].icon.as_deref(), Some("none"));
        assert_eq!(c[2].css_class.as_deref(), Some("foo"));
        assert_eq!(c[3].icon.as_deref(), Some("none"));
    }

    #[test]
    fn dedent_pops_to_matching_parent() {
        // Mirrors upstream's tc: "e" (indent 1) and "f" (indent 0) are root children.
        let tv = parse("treeView-beta\n  a\n    b\n  d\n e\nf\n").unwrap();
        assert_eq!(names(&tv.root), vec!["a", "d", "e", "f"]);
        assert_eq!(names(&tv.root.children[0]), vec!["b"]);
    }

    #[test]
    fn title_and_acc_before_nodes_only() {
        let tv = parse("treeView-beta\ntitle Hello\n  a\n").unwrap();
        assert_eq!(tv.title.as_deref(), Some("Hello"));
        assert!(matches!(
            parse("treeView-beta\n  a\n  title x\n"),
            Err(Error::Parse { line: 3, .. })
        ));
    }

    #[test]
    fn errors_mirror_upstream() {
        // lexer error: second ::: without whitespace
        assert!(matches!(
            parse("treeView-beta\n  d :::foo:::bar\n"),
            Err(Error::Parse { line: 2, .. })
        ));
        // whitespace-only line followed by an indented node
        assert!(matches!(
            parse("treeView-beta\n    src/\n   \n        index.js\n"),
            Err(Error::Parse { line: 4, .. })
        ));
        assert!(parse("treeview-beta\n a").is_err());
    }

    #[test]
    fn box_drawing_input() {
        let src = "treeView-beta\n├── packages/\n│   ├── mermaid/\n│   │   └── src/\n│   └── parser/\n└── README.md\n";
        let pre = preprocess_box_drawing(src).unwrap();
        assert_eq!(
            pre.text,
            "treeView-beta\n    packages/\n        mermaid/\n            src/\n        parser/\n    README.md\n"
        );
        let tv = parse(src).unwrap();
        assert_eq!(names(&tv.root), vec!["packages", "README.md"]);
        assert_eq!(names(&tv.root.children[0]), vec!["mermaid", "parser"]);
        // heavy variant and annotations
        let tv = parse("treeView-beta\n┣━━ a ## x\n┗━━ b :::highlight\n").unwrap();
        assert_eq!(tv.root.children[0].description.as_deref(), Some("x"));
        assert_eq!(tv.root.children[1].css_class.as_deref(), Some("highlight"));
    }

    #[test]
    fn box_drawing_errors_use_original_lines() {
        let e = parse("treeView-beta\n├── a\n├──\n").unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("treeView-beta\n├── a\n    b\n").unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        // no box chars → unchanged
        let src = "treeView-beta\n  a\n";
        assert_eq!(preprocess_box_drawing(src).unwrap().text, src);
    }

    #[test]
    fn icon_resolution() {
        let n = |name: &str, t: NodeType, icon: Option<&str>| Node {
            level: 0,
            name: name.into(),
            node_type: t,
            icon: icon.map(str::to_string),
            css_class: None,
            description: None,
            children: vec![],
        };
        let mut cfg = IconConfig::default();
        assert_eq!(node_icon(&n("a.ts", NodeType::File, None), &cfg), None);
        assert_eq!(
            node_icon(&n("a", NodeType::File, Some("folder")), &cfg),
            Some(IconRef::Folder)
        );
        cfg.show_icons = true;
        cfg.default_icon_pack = "mit".into();
        cfg.filename_icons = vec![("Dockerfile".into(), "docker".into())];
        cfg.extension_icons = vec![
            (".ts".into(), "typescript".into()),
            ("txt".into(), "none".into()),
        ];
        assert_eq!(
            node_icon(&n("Dockerfile", NodeType::File, None), &cfg),
            Some(IconRef::External("mit:docker".into()))
        );
        assert_eq!(
            node_icon(&n("A.TS", NodeType::File, None), &cfg),
            Some(IconRef::External("mit:typescript".into()))
        );
        assert_eq!(node_icon(&n("n.txt", NodeType::File, None), &cfg), None);
        assert_eq!(
            node_icon(&n("x", NodeType::File, None), &cfg),
            Some(IconRef::File)
        );
        assert_eq!(
            node_icon(&n("d", NodeType::Directory, None), &cfg),
            Some(IconRef::Folder)
        );
        assert_eq!(
            node_icon(&n("d", NodeType::Directory, Some("none")), &cfg),
            None
        );
    }

    #[test]
    fn layout_matches_mermaid_geometry() {
        let tv = parse("treeView-beta\n    src/\n        App.tsx :::highlight ## main\n        index.js\n    a\n").unwrap();
        let cfg = LayoutConfig::default();
        let lay = layout(&tv, &cfg);
        assert_eq!(lay.rows.len(), 5);
        assert_eq!(lay.rows[0].name, "/");
        // rows stack without gaps; depth indent is rowIndent + paddingX
        for w in lay.rows.windows(2) {
            assert!((w[0].bbox.bottom() - w[1].bbox.y).abs() < 1e-9);
        }
        for r in &lay.rows {
            assert!((r.bbox.x - r.depth as f64 * 15.0).abs() < 1e-9);
            assert!(r.bbox.h >= 27.0 && r.bbox.h <= 28.0 + 1e-9);
        }
        assert!((lay.height - lay.rows.iter().map(|r| r.bbox.h).sum::<f64>()).abs() < 1e-9);
        // one horizontal connector per row + one trunk per parent (/, src)
        assert_eq!(lay.lines.len(), 5 + 2);
        let trunk = lay
            .lines
            .iter()
            .find(|(a, b)| a.x == b.x && a.x == 20.0)
            .unwrap();
        assert!((trunk.1.y - (lay.rows[3].center_y + 0.5)).abs() < 1e-9);
        // descriptions start DESC_GAP after the widest label
        let max_right = lay.rows.iter().map(|r| r.label_right).fold(0.0, f64::max);
        assert_eq!(lay.desc_x, Some(max_right + DESC_GAP));
        // highlight spans to the right edge
        let hl = lay.rows[2].highlight.unwrap();
        assert!((hl.right() + 2.0 - lay.width).abs() < 1e-9);
    }

    #[test]
    fn config_and_theme_variables() {
        let j = crate::json::parse(
            r##"{"treeView":{"rowIndent":80,"lineThickness":3},"themeVariables":{"treeView":{"labelFontSize":"20px","labelColor":"#FF0000"}}}"##,
        )
        .unwrap();
        let cfg = MermaidConfig::default().with_json(&j).unwrap();
        let (lc, colors) = config(&cfg);
        assert_eq!(lc.row_indent, 80.0);
        assert_eq!(lc.line_thickness, 3.0);
        assert_eq!(lc.font_size, 20.0);
        assert_eq!(colors.label.css(), "#FF0000");
        assert_eq!(colors.line.css(), "black");
        let s = render("treeView-beta\n  a\n", None, &cfg, &cfg.palette()).unwrap();
        assert_eq!(s.kind, DiagramKind::TreeView);
        assert_eq!(s.view_box.x, -1.5);
    }
}
