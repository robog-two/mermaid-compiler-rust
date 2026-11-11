//! `venn-beta` diagrams (mermaid 12.3 `diagrams/venn`).
//!
//! The grammar follows `venn.jison` (first-match lexer, indented `text` lines attach to the
//! most recent `set` / `union`). Layout is a port of `@upsetjs/venn.js` 2.0, which mermaid
//! uses unchanged: circle radii from set sizes, a greedy initial placement solving pairwise
//! overlap distances by bisection (plus constrained MDS for 4+ sets), Nelder–Mead
//! refinement of the overlap loss, normalisation (orientation + disjoint clusters),
//! scaling into the viewBox and label centres maximising the distance to circle edges.
//! Drawing follows `vennRenderer.ts` (800×450 canvas scaled from a 1600px reference).

use crate::color::{self, Rgba};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::theme::{Palette, ThemeName};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One `set` or `union` statement (`VennData`).
#[derive(Debug, Clone, PartialEq)]
pub struct Subset {
    /// Member set identifiers, sorted.
    pub sets: Vec<String>,
    pub size: f64,
    pub label: Option<String>,
}

/// A `text` node placed inside a set or union region (`VennTextData`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextNode {
    /// The region's member sets, sorted.
    pub sets: Vec<String>,
    pub id: String,
    pub label: Option<String>,
}

/// A `style` statement (`VennStyleData`).
#[derive(Debug, Clone, PartialEq)]
pub struct StyleEntry {
    /// Sorted targets: one set, a union's members, or a text node id.
    pub targets: Vec<String>,
    pub styles: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Venn {
    pub title: Option<String>,
    pub subsets: Vec<Subset>,
    pub texts: Vec<TextNode>,
    pub styles: Vec<StyleEntry>,
}

// ---------------------------------------------------------------------------
// Lexer (venn.jison, first matching rule wins, case-insensitive)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Title(String),
    Venn,
    Set,
    Union,
    Text,
    IndentText,
    Style,
    Bracket(String),
    Numeric(String),
    Hex(String),
    Rgba(String),
    Rgb(String),
    Ident(String),
    Str(String),
    Comma,
    Colon,
}

impl Tok {
    fn describe(&self) -> &'static str {
        match self {
            Tok::Title(_) => "TITLE",
            Tok::Venn => "VENN",
            Tok::Set => "SET",
            Tok::Union => "UNION",
            Tok::Text => "TEXT",
            Tok::IndentText => "INDENT_TEXT",
            Tok::Style => "STYLE",
            Tok::Bracket(_) => "BRACKET_LABEL",
            Tok::Numeric(_) => "NUMERIC",
            Tok::Hex(_) => "HEXCOLOR",
            Tok::Rgba(_) => "RGBACOLOR",
            Tok::Rgb(_) => "RGBCOLOR",
            Tok::Ident(_) => "IDENTIFIER",
            Tok::Str(_) => "STRING",
            Tok::Comma => "COMMA",
            Tok::Colon => "COLON",
        }
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `[+-]?(\d+(\.\d+)?|\.\d+)`: byte length of the match.
fn match_numeric(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let digits = |from: usize| s[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let d = digits(i);
    if d > 0 {
        i += d;
        if i + 1 < s.len() && s[i] == b'.' && s[i + 1].is_ascii_digit() {
            i += 1 + digits(i + 1);
        }
        return Some(i);
    }
    if i + 1 < s.len() && s[i] == b'.' && s[i + 1].is_ascii_digit() {
        return Some(i + 1 + digits(i + 1));
    }
    None
}

/// `rgba?\(\s*n\s*,\s*n\s*,\s*n(\s*,\s*n)?\s*\)` with `n = [0-9.]+`.
fn match_rgb(s: &[u8], alpha: bool) -> Option<usize> {
    let head = if alpha { "rgba(" } else { "rgb(" };
    let st = std::str::from_utf8(s).ok()?;
    if !starts_with_ci(st, head) {
        return None;
    }
    let mut i = head.len();
    let ws = |i: &mut usize| {
        while *i < s.len() && (s[*i] == b' ' || s[*i] == b'\t') {
            *i += 1;
        }
    };
    let count = if alpha { 4 } else { 3 };
    for k in 0..count {
        ws(&mut i);
        let n = s[i..]
            .iter()
            .take_while(|c| c.is_ascii_digit() || **c == b'.')
            .count();
        if n == 0 {
            return None;
        }
        i += n;
        ws(&mut i);
        if k + 1 < count {
            if s.get(i) != Some(&b',') {
                return None;
            }
            i += 1;
        }
    }
    (s.get(i) == Some(&b')')).then_some(i + 1)
}

/// Tokenise one source line (after the `<bol>` handling).
fn lex_line(line: &str, line_no: usize, out: &mut Vec<Tok>) -> Result<()> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &line[i..];
        let rb = rest.as_bytes();
        // Comments: `%%(?!\{)...` and `[^}]%%...` (the latter eats the preceding char).
        if rest.starts_with("%%") && !rest.starts_with("%%{") {
            break;
        }
        if rb.len() >= 3 && rb[0] != b'}' && &rb[1..3] == b"%%" {
            break;
        }
        if rb[0] == b' ' || rb[0] == b'\t' {
            i += 1;
            continue;
        }
        // "title"\s[^#\n;]+
        if starts_with_ci(rest, "title")
            && rb.len() > 6
            && (rb[5] as char).is_whitespace()
            && !matches!(rb[6], b'#' | b';')
        {
            let end = rest[6..].find(['#', ';']).map_or(rest.len(), |p| p + 6);
            out.push(Tok::Title(rest[6..end].to_string()));
            i += end;
            continue;
        }
        let kw = [
            ("venn-beta", Tok::Venn),
            ("set", Tok::Set),
            ("union", Tok::Union),
            ("text", Tok::Text),
            ("style", Tok::Style),
        ];
        if let Some((k, t)) = kw.iter().find(|(k, _)| starts_with_ci(rest, k)) {
            out.push(t.clone());
            i += k.len();
            continue;
        }
        if rb[0] == b'[' {
            if rb.len() > 1 && rb[1] == b'"' {
                if let Some(close) = rest[2..].find('"') {
                    if rb.get(2 + close + 1) == Some(&b']') {
                        out.push(Tok::Bracket(rest[2..2 + close].to_string()));
                        i += 2 + close + 2;
                        continue;
                    }
                }
            }
            if let Some(close) = rest[1..].find([']', '"']) {
                if close > 0 && rb[1 + close] == b']' {
                    out.push(Tok::Bracket(rest[1..1 + close].trim().to_string()));
                    i += close + 2;
                    continue;
                }
            }
        }
        if let Some(n) = match_numeric(rb) {
            out.push(Tok::Numeric(rest[..n].to_string()));
            i += n;
            continue;
        }
        if rb[0] == b'#' {
            let n = rb[1..]
                .iter()
                .take(8)
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if n >= 3 {
                out.push(Tok::Hex(rest[..1 + n].to_string()));
                i += 1 + n;
                continue;
            }
        }
        if let Some(n) = match_rgb(rb, true) {
            out.push(Tok::Rgba(rest[..n].to_string()));
            i += n;
            continue;
        }
        if let Some(n) = match_rgb(rb, false) {
            out.push(Tok::Rgb(rest[..n].to_string()));
            i += n;
            continue;
        }
        if rb[0].is_ascii_alphabetic() || rb[0] == b'_' {
            let n = rb
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b'_')
                .count();
            out.push(Tok::Ident(rest[..n].to_string()));
            i += n;
            continue;
        }
        if rb[0] == b'"' {
            if let Some(close) = rest[1..].find('"') {
                out.push(Tok::Str(rest[..close + 2].to_string()));
                i += close + 2;
                continue;
            }
        }
        match rb[0] {
            b',' => out.push(Tok::Comma),
            b':' => out.push(Tok::Colon),
            _ => {
                return Err(Error::parse(
                    line_no,
                    format!("Lexical error on line {line_no}. Unrecognized text.\n{rest}"),
                ));
            }
        }
        i += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parser (statement grammar of venn.jison + vennDB semantics)
// ---------------------------------------------------------------------------

/// `normalizeText`: trim, then strip one pair of surrounding double quotes.
fn normalize_text(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    line: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn unexpected(&self, expecting: &str) -> Error {
        let got = self.peek().map_or("NEWLINE", Tok::describe);
        Error::parse(
            self.line,
            format!(
                "Parse error on line {}: Expecting {expecting}, got '{got}'",
                self.line
            ),
        )
    }

    fn identifier(&mut self) -> Result<String> {
        match self.peek() {
            Some(Tok::Ident(s) | Tok::Str(s)) => {
                let s = normalize_text(s);
                self.pos += 1;
                Ok(s)
            }
            _ => Err(self.unexpected("'IDENTIFIER', 'STRING'")),
        }
    }

    fn identifier_list(&mut self) -> Result<Vec<String>> {
        let mut v = vec![self.identifier()?];
        while self.peek() == Some(&Tok::Comma) {
            self.pos += 1;
            v.push(self.identifier()?);
        }
        Ok(v)
    }

    /// Optional `BRACKET_LABEL` then optional `: NUMERIC`.
    fn label_and_size(&mut self) -> Result<(Option<String>, Option<f64>)> {
        let mut label = None;
        if let Some(Tok::Bracket(l)) = self.peek() {
            label = Some(l.clone());
            self.pos += 1;
        }
        let mut size = None;
        if self.peek() == Some(&Tok::Colon) {
            self.pos += 1;
            match self.peek() {
                Some(Tok::Numeric(n)) => {
                    size = Some(n.parse::<f64>().unwrap_or(f64::NAN));
                    self.pos += 1;
                }
                _ => return Err(self.unexpected("'NUMERIC'")),
            }
        }
        Ok((label, size))
    }

    /// The text node id (+ optional bracket label) of a `text` statement.
    fn text_tail(&mut self) -> Result<(String, Option<String>)> {
        let (id, labelled) = match self.peek() {
            Some(Tok::Ident(s) | Tok::Str(s)) => (s.clone(), true),
            Some(Tok::Numeric(s)) => (s.clone(), false),
            _ => return Err(self.unexpected("'IDENTIFIER', 'STRING', 'NUMERIC'")),
        };
        self.pos += 1;
        let mut label = None;
        if labelled {
            if let Some(Tok::Bracket(l)) = self.peek() {
                label = Some(l.clone());
                self.pos += 1;
            }
        }
        Ok((id, label))
    }

    fn style_fields(&mut self) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        loop {
            let key = match self.peek() {
                Some(Tok::Ident(k)) => k.clone(),
                _ => return Err(self.unexpected("'IDENTIFIER'")),
            };
            self.pos += 1;
            if self.peek() != Some(&Tok::Colon) {
                return Err(self.unexpected("'COLON'"));
            }
            self.pos += 1;
            let value = if let Some(Tok::Str(s)) = self.peek() {
                let s = s.clone();
                self.pos += 1;
                s
            } else {
                let mut parts = Vec::new();
                while let Some(
                    Tok::Ident(v) | Tok::Numeric(v) | Tok::Hex(v) | Tok::Rgb(v) | Tok::Rgba(v),
                ) = self.peek()
                {
                    parts.push(v.clone());
                    self.pos += 1;
                }
                if parts.is_empty() {
                    return Err(self.unexpected("'STRING', 'IDENTIFIER', 'NUMERIC', 'HEXCOLOR'"));
                }
                parts.join(" ")
            };
            out.push((key, normalize_text(&value)));
            if self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
                continue;
            }
            return Ok(out);
        }
    }
}

#[derive(Default)]
struct Db {
    venn: Venn,
    known: Vec<String>,
    current: Option<Vec<String>>,
    indent_mode: bool,
}

impl Db {
    fn add_subset(&mut self, ids: Vec<String>, label: Option<String>, size: Option<f64>) {
        let n = ids.len() as f64;
        let mut sets = ids;
        sets.sort();
        if sets.len() == 1 && !self.known.contains(&sets[0]) {
            self.known.push(sets[0].clone());
        }
        self.current = Some(sets.clone());
        self.venn.subsets.push(Subset {
            sets,
            size: size.unwrap_or(10.0 / (n * n)),
            label: label.map(|l| normalize_text(&l)),
        });
        self.indent_mode = true;
    }

    fn add_text(&mut self, ids: Vec<String>, id: &str, label: Option<String>) {
        let mut sets = ids;
        sets.sort();
        self.venn.texts.push(TextNode {
            sets,
            id: normalize_text(id),
            label: label.map(|l| normalize_text(&l)),
        });
    }

    fn statement(&mut self, p: &mut Parser) -> Result<()> {
        let Some(tok) = p.peek().cloned() else {
            return Ok(());
        };
        p.pos += 1;
        match tok {
            Tok::Title(t) => self.venn.title = Some(t.trim().to_string()),
            Tok::Set => {
                let id = p.identifier()?;
                if p.peek() == Some(&Tok::Comma) {
                    return Err(p.unexpected("'BRACKET_LABEL', 'COLON', NEWLINE"));
                }
                let (label, size) = p.label_and_size()?;
                self.add_subset(vec![id], label, size);
            }
            Tok::Union => {
                let ids = p.identifier_list()?;
                let (label, size) = p.label_and_size()?;
                if ids.len() < 2 {
                    return Err(Error::parse(p.line, "union requires multiple identifiers"));
                }
                let unknown: Vec<&str> = ids
                    .iter()
                    .filter(|i| !self.known.contains(i))
                    .map(String::as_str)
                    .collect();
                if !unknown.is_empty() {
                    return Err(Error::parse(
                        p.line,
                        format!("unknown set identifier: {}", unknown.join(", ")),
                    ));
                }
                self.add_subset(ids, label, size);
            }
            Tok::Text => {
                let ids = p.identifier_list()?;
                let (id, label) = p.text_tail()?;
                self.add_text(ids, &id, label);
            }
            Tok::IndentText => {
                let (id, label) = p.text_tail()?;
                let Some(sets) = self.current.clone() else {
                    return Err(Error::parse(p.line, "text requires set"));
                };
                self.add_text(sets, &id, label);
            }
            Tok::Style => {
                let mut targets = p.identifier_list()?;
                targets.sort();
                let styles = p.style_fields()?;
                self.venn.styles.push(StyleEntry { targets, styles });
            }
            other => {
                p.pos -= 1;
                let _ = other;
                return Err(p.unexpected("'TITLE', 'SET', 'UNION', 'TEXT', 'STYLE', NEWLINE"));
            }
        }
        Ok(())
    }
}

/// Parse a `venn-beta` definition (body as produced by `diagram::preprocess`).
pub fn parse(body: &str, title: Option<String>) -> Result<Venn> {
    let mut db = Db::default();
    db.venn.title = title;
    let mut header_seen = false;
    for (idx, raw) in body.lines().enumerate() {
        let line_no = idx + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let mut toks = Vec::new();
        let starts_indented = raw.starts_with([' ', '\t']);
        let mut content = raw;
        if header_seen {
            if starts_indented {
                let trimmed = raw.trim_start_matches([' ', '\t']);
                let tb = trimmed.as_bytes();
                if db.indent_mode
                    && starts_with_ci(trimmed, "text")
                    && (tb.len() == 4 || !is_word(tb[4]))
                {
                    toks.push(Tok::IndentText);
                    content = &trimmed[4..];
                }
            } else {
                db.indent_mode = false;
            }
        }
        lex_line(content, line_no, &mut toks)?;
        if toks.is_empty() {
            continue;
        }
        let mut start = 0;
        if !header_seen {
            if toks[0] != Tok::Venn {
                return Err(Error::parse(
                    line_no,
                    format!(
                        "Parse error on line {line_no}: Expecting 'VENN', got '{}'",
                        toks[0].describe()
                    ),
                ));
            }
            header_seen = true;
            start = 1;
        }
        let mut p = Parser {
            toks: &toks[start..],
            pos: 0,
            line: line_no,
        };
        while p.pos < p.toks.len() {
            db.statement(&mut p)?;
        }
    }
    if !header_seen {
        return Err(Error::parse(1, "Expecting 'VENN'"));
    }
    Ok(db.venn)
}

/// `ensurePairwiseSubsets`: for every union of 3+ sets, add the missing pairwise overlaps
/// (size = min of the two set sizes / 4, or 2.5) so the layout gives it a visible region.
pub fn ensure_pairwise_subsets(subsets: &[Subset]) -> Vec<Subset> {
    let mut keys: Vec<String> = subsets.iter().map(|s| s.sets.join("|")).collect();
    let size_of = |id: &str| {
        subsets
            .iter()
            .rev()
            .find(|s| s.sets.len() == 1 && s.sets[0] == id)
            .map(|s| s.size)
    };
    let mut out = subsets.to_vec();
    for s in subsets.iter().filter(|s| s.sets.len() >= 3) {
        let mut m = s.sets.clone();
        m.sort();
        for i in 0..m.len() {
            for j in i + 1..m.len() {
                let key = format!("{}|{}", m[i], m[j]);
                if keys.contains(&key) {
                    continue;
                }
                keys.push(key);
                let size = match (size_of(&m[i]), size_of(&m[j])) {
                    (Some(a), Some(b)) => a.min(b) / 4.0,
                    _ => 2.5,
                };
                out.push(Subset {
                    sets: vec![m[i].clone(), m[j].clone()],
                    size,
                    label: Some(String::new()),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// venn.js geometry
// ---------------------------------------------------------------------------

const SMALL: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
}

impl Circle {
    fn center(&self) -> Point {
        Point::new(self.x, self.y)
    }
}

fn dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)).sqrt()
}

/// Area of a circular segment of width `width` in a circle of radius `r`.
pub fn circle_area(r: f64, width: f64) -> f64 {
    r * r * (1.0 - width / r).acos() - (r - width) * (width * (2.0 * r - width)).sqrt()
}

/// Overlap area of two circles of radii `r1`, `r2` whose centres are `d` apart.
pub fn circle_overlap(r1: f64, r2: f64, d: f64) -> f64 {
    if d >= r1 + r2 {
        return 0.0;
    }
    if d <= (r1 - r2).abs() {
        return PI * r1.min(r2) * r1.min(r2);
    }
    let w1 = r1 - (d * d - r2 * r2 + r1 * r1) / (2.0 * d);
    let w2 = r2 - (d * d - r1 * r1 + r2 * r2) / (2.0 * d);
    circle_area(r1, w1) + circle_area(r2, w2)
}

/// Intersection points of two circle outlines (none when disjoint or nested).
pub fn circle_circle_intersection(p1: &Circle, p2: &Circle) -> Vec<Point> {
    let d = dist(p1.center(), p2.center());
    let (r1, r2) = (p1.radius, p2.radius);
    if d >= r1 + r2 || d <= (r1 - r2).abs() {
        return Vec::new();
    }
    let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
    let h = (r1 * r1 - a * a).sqrt();
    let x0 = p1.x + a * (p2.x - p1.x) / d;
    let y0 = p1.y + a * (p2.y - p1.y) / d;
    let rx = -(p2.y - p1.y) * (h / d);
    let ry = -(p2.x - p1.x) * (h / d);
    vec![Point::new(x0 + rx, y0 - ry), Point::new(x0 - rx, y0 + ry)]
}

fn get_center(points: &[Point]) -> Point {
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    Point::new(sx / n, sy / n)
}

/// One boundary arc of an intersection region (drawn from `p2` to `p1`, sweep = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    pub circle: Circle,
    pub width: f64,
    pub p1: Point,
    pub p2: Point,
    pub large: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AreaStats {
    pub area: f64,
    pub arcs: Vec<Arc>,
}

/// `intersectionArea`: area (and boundary arcs) of the region common to all `circles`.
pub fn intersection_area(circles: &[Circle]) -> AreaStats {
    struct Ip {
        p: Point,
        parents: [usize; 2],
        angle: f64,
    }
    let mut ips = Vec::new();
    for i in 0..circles.len() {
        for j in i + 1..circles.len() {
            for p in circle_circle_intersection(&circles[i], &circles[j]) {
                ips.push(Ip {
                    p,
                    parents: [i, j],
                    angle: 0.0,
                });
            }
        }
    }
    let mut inner: Vec<Ip> = ips
        .into_iter()
        .filter(|ip| {
            circles
                .iter()
                .all(|c| dist(ip.p, c.center()) < c.radius + SMALL)
        })
        .collect();
    let mut arc_area = 0.0;
    let mut polygon_area = 0.0;
    let mut arcs = Vec::new();
    if inner.len() > 1 {
        let center = get_center(&inner.iter().map(|i| i.p).collect::<Vec<_>>());
        for ip in &mut inner {
            ip.angle = (ip.p.x - center.x).atan2(ip.p.y - center.y);
        }
        inner.sort_by(|a, b| {
            b.angle
                .partial_cmp(&a.angle)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut p2 = inner.len() - 1;
        for i in 0..inner.len() {
            let (a, b) = (&inner[i], &inner[p2]);
            polygon_area += (b.p.x + a.p.x) * (a.p.y - b.p.y);
            let mid = Point::new((a.p.x + b.p.x) / 2.0, (a.p.y + b.p.y) / 2.0);
            let mut arc: Option<Arc> = None;
            for &pi in &a.parents {
                if !b.parents.contains(&pi) {
                    continue;
                }
                let c = circles[pi];
                let a1 = (a.p.x - c.x).atan2(a.p.y - c.y);
                let a2 = (b.p.x - c.x).atan2(b.p.y - c.y);
                let mut diff = a2 - a1;
                if diff < 0.0 {
                    diff += TAU;
                }
                let am = a2 - diff / 2.0;
                let mut width = dist(
                    mid,
                    Point::new(c.x + c.radius * am.sin(), c.y + c.radius * am.cos()),
                );
                if width > c.radius * 2.0 {
                    width = c.radius * 2.0;
                }
                if arc.is_none_or(|x| x.width > width) {
                    arc = Some(Arc {
                        circle: c,
                        width,
                        p1: a.p,
                        p2: b.p,
                        large: width > c.radius,
                    });
                }
            }
            if let Some(arc) = arc {
                arc_area += circle_area(arc.circle.radius, arc.width);
                arcs.push(arc);
                p2 = i;
            }
        }
    } else if let Some(first) = circles.first() {
        let mut smallest = *first;
        for c in &circles[1..] {
            if c.radius < smallest.radius {
                smallest = *c;
            }
        }
        let disjoint = circles
            .iter()
            .any(|c| dist(c.center(), smallest.center()) > (smallest.radius - c.radius).abs());
        if !disjoint {
            arc_area = smallest.radius * smallest.radius * PI;
            arcs.push(Arc {
                circle: smallest,
                p1: Point::new(smallest.x, smallest.y + smallest.radius),
                p2: Point::new(smallest.x - SMALL, smallest.y + smallest.radius),
                width: smallest.radius * 2.0,
                large: true,
            });
        }
    }
    polygon_area /= 2.0;
    AreaStats {
        area: arc_area + polygon_area,
        arcs,
    }
}

fn bisect(f: impl Fn(f64) -> f64, mut a: f64, b: f64) -> f64 {
    let (max_iterations, tolerance) = (100, 1e-10);
    let fa = f(a);
    let fb = f(b);
    let mut delta = b - a;
    if fa * fb > 0.0 {
        // venn.js throws here; callers only bracket sign changes.
        return a;
    }
    if fa == 0.0 {
        return a;
    }
    if fb == 0.0 {
        return b;
    }
    for _ in 0..max_iterations {
        delta /= 2.0;
        let mid = a + delta;
        let fm = f(mid);
        if fm * fa >= 0.0 {
            a = mid;
        }
        if delta.abs() < tolerance || fm == 0.0 {
            return mid;
        }
    }
    a + delta
}

/// Centre distance at which two circles overlap by `overlap`.
pub fn distance_from_intersect_area(r1: f64, r2: f64, overlap: f64) -> f64 {
    if r1.min(r2) * r1.min(r2) * PI <= overlap + SMALL {
        return (r1 - r2).abs();
    }
    bisect(|d| circle_overlap(r1, r2, d) - overlap, 0.0, r1 + r2)
}

fn weighted_sum(ret: &mut [f64], w1: f64, v1: &[f64], w2: f64, v2: &[f64]) {
    for j in 0..ret.len() {
        ret[j] = w1 * v1[j] + w2 * v2[j];
    }
}

struct NmParams {
    max_iterations: usize,
    min_error_delta: f64,
    min_tolerance: f64,
}

/// fmin's Nelder–Mead (as bundled in venn.js), including its quirks.
fn nelder_mead(f: impl Fn(&[f64]) -> f64, x0: &[f64], prm: &NmParams) -> (Vec<f64>, f64) {
    let (rho, chi, psi, sigma) = (1.0, 2.0, -0.5, 0.5);
    let (non_zero_delta, zero_delta) = (1.05, 1e-3);
    let n = x0.len();
    let mut simplex: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n + 1);
    simplex.push((x0.to_vec(), f(x0)));
    for i in 0..n {
        let mut p = x0.to_vec();
        p[i] = if p[i] != 0.0 {
            p[i] * non_zero_delta
        } else {
            zero_delta
        };
        let fx = f(&p);
        simplex.push((p, fx));
    }
    let sort = |s: &mut Vec<(Vec<f64>, f64)>| {
        s.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    };
    let mut centroid = x0.to_vec();
    let mut reflected = x0.to_vec();
    let mut contracted = x0.to_vec();
    let mut expanded = x0.to_vec();
    for _ in 0..prm.max_iterations {
        sort(&mut simplex);
        let mut max_diff: f64 = 0.0;
        for i in 0..n {
            max_diff = max_diff.max((simplex[0].0[i] - simplex[1].0[i]).abs());
        }
        if (simplex[0].1 - simplex[n].1).abs() < prm.min_error_delta && max_diff < prm.min_tolerance
        {
            break;
        }
        for (i, c) in centroid.iter_mut().enumerate() {
            *c = simplex[..n].iter().map(|s| s.0[i]).sum::<f64>() / n as f64;
        }
        let worst = simplex[n].clone();
        weighted_sum(&mut reflected, 1.0 + rho, &centroid, -rho, &worst.0);
        let fr = f(&reflected);
        if fr < simplex[0].1 {
            weighted_sum(&mut expanded, 1.0 + chi, &centroid, -chi, &worst.0);
            let fe = f(&expanded);
            simplex[n] = if fe < fr {
                (expanded.clone(), fe)
            } else {
                (reflected.clone(), fr)
            };
        } else if fr >= simplex[n - 1].1 {
            let mut reduce = false;
            if fr > worst.1 {
                weighted_sum(&mut contracted, 1.0 + psi, &centroid, -psi, &worst.0);
                let fc = f(&contracted);
                if fc < worst.1 {
                    simplex[n] = (contracted.clone(), fc);
                } else {
                    reduce = true;
                }
            } else {
                weighted_sum(
                    &mut contracted,
                    1.0 - psi * rho,
                    &centroid,
                    psi * rho,
                    &worst.0,
                );
                let fc = f(&contracted);
                if fc < fr {
                    simplex[n] = (contracted.clone(), fc);
                } else {
                    reduce = true;
                }
            }
            if reduce {
                let best = simplex[0].0.clone();
                for s in simplex.iter_mut().skip(1) {
                    let cur = s.0.clone();
                    weighted_sum(&mut s.0, 1.0 - sigma, &best, sigma, &cur);
                    s.1 = f(&s.0);
                }
            }
        } else {
            simplex[n] = (reflected.clone(), fr);
        }
    }
    sort(&mut simplex);
    let (x, fx) = simplex.swap_remove(0);
    (x, fx)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm2(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

#[derive(Clone)]
struct CgPoint {
    x: Vec<f64>,
    fx: f64,
    fxprime: Vec<f64>,
}

type GradFn<'a> = dyn Fn(&[f64], &mut [f64]) -> f64 + 'a;

fn wolfe_line_search(
    f: &GradFn,
    pk: &[f64],
    current: &CgPoint,
    next: &mut CgPoint,
    a0: f64,
) -> f64 {
    let (c1, c2) = (1e-6, 0.1);
    let phi0 = current.fx;
    let phi_prime0 = dot(&current.fxprime, pk);
    let mut phi_old = phi0;
    let mut a_prev = 0.0;
    let mut a = if a0 == 0.0 { 1.0 } else { a0 };
    let eval = |next: &mut CgPoint, a: f64| -> (f64, f64) {
        weighted_sum(&mut next.x, 1.0, &current.x, a, pk);
        let mut g = std::mem::take(&mut next.fxprime);
        next.fx = f(&next.x, &mut g);
        next.fxprime = g;
        (next.fx, dot(&next.fxprime, pk))
    };
    let zoom = |next: &mut CgPoint, mut lo: f64, mut hi: f64, mut phi_lo: f64| -> f64 {
        for _ in 0..16 {
            let a = (lo + hi) / 2.0;
            let (phi, phi_prime) = eval(next, a);
            if phi > phi0 + c1 * a * phi_prime0 || phi >= phi_lo {
                hi = a;
            } else {
                if phi_prime.abs() <= -c2 * phi_prime0 {
                    return a;
                }
                if phi_prime * (hi - lo) >= 0.0 {
                    hi = lo;
                }
                lo = a;
                phi_lo = phi;
            }
        }
        0.0
    };
    for iteration in 0..10 {
        let (phi, phi_prime) = eval(next, a);
        if phi > phi0 + c1 * a * phi_prime0 || (iteration > 0 && phi >= phi_old) {
            return zoom(next, a_prev, a, phi_old);
        }
        if phi_prime.abs() <= -c2 * phi_prime0 {
            return a;
        }
        if phi_prime >= 0.0 {
            return zoom(next, a, a_prev, phi);
        }
        phi_old = phi;
        a_prev = a;
        a *= 2.0;
    }
    a
}

fn conjugate_gradient(f: &GradFn, initial: &[f64], max_iterations: usize) -> CgPoint {
    let mut current = CgPoint {
        x: initial.to_vec(),
        fx: 0.0,
        fxprime: initial.to_vec(),
    };
    let mut next = current.clone();
    let mut yk = initial.to_vec();
    let mut g = current.fxprime.clone();
    current.fx = f(&current.x, &mut g);
    current.fxprime = g;
    let mut pk: Vec<f64> = current.fxprime.iter().map(|v| -v).collect();
    let mut a = 1.0;
    for _ in 0..max_iterations {
        a = wolfe_line_search(f, &pk, &current, &mut next, a);
        if a == 0.0 {
            pk = current.fxprime.iter().map(|v| -v).collect();
        } else {
            weighted_sum(&mut yk, 1.0, &next.fxprime, -1.0, &current.fxprime);
            let delta_k = dot(&current.fxprime, &current.fxprime);
            let beta_k = (dot(&yk, &next.fxprime) / delta_k).max(0.0);
            let old = pk.clone();
            weighted_sum(&mut pk, beta_k, &old, -1.0, &next.fxprime);
            std::mem::swap(&mut current, &mut next);
        }
        if norm2(&current.fxprime) <= 1e-5 {
            break;
        }
    }
    current
}

/// An area of the layout problem with set indices resolved.
#[derive(Debug, Clone, PartialEq)]
struct IArea {
    sets: Vec<usize>,
    size: f64,
}

/// `lossFunction`: sum of squared differences between wanted and actual overlap areas.
fn loss_function(circles: &[Circle], areas: &[IArea]) -> f64 {
    let mut out = 0.0;
    for a in areas {
        let overlap = match a.sets.len() {
            0 | 1 => continue,
            2 => {
                let (l, r) = (circles[a.sets[0]], circles[a.sets[1]]);
                circle_overlap(l.radius, r.radius, dist(l.center(), r.center()))
            }
            _ => {
                let cs: Vec<Circle> = a.sets.iter().map(|&i| circles[i]).collect();
                intersection_area(&cs).area
            }
        };
        out += (overlap - a.size) * (overlap - a.size);
    }
    out
}

/// `addMissingAreas`: every pair of sets without a stated overlap gets size 0.
fn add_missing_areas(areas: &[IArea], n_sets: usize, names: &[String]) -> Vec<IArea> {
    let mut r = areas.to_vec();
    let mut ids: Vec<usize> = areas
        .iter()
        .filter(|a| a.sets.len() == 1)
        .map(|a| a.sets[0])
        .collect();
    let mut pairs = std::collections::HashSet::new();
    for a in areas.iter().filter(|a| a.sets.len() == 2) {
        pairs.insert((a.sets[0], a.sets[1]));
        pairs.insert((a.sets[1], a.sets[0]));
    }
    ids.sort_by(|a, b| names[*a].cmp(&names[*b]));
    debug_assert!(ids.iter().all(|i| *i < n_sets));
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if !pairs.contains(&(ids[i], ids[j])) {
                r.push(IArea {
                    sets: vec![ids[i], ids[j]],
                    size: 0.0,
                });
            }
        }
    }
    r
}

/// `greedyLayout`: place the most-overlapped set first, then each next set at the
/// candidate point (pairwise distance solutions) with the lowest pairwise loss.
fn greedy_layout(areas: &[IArea], sizes: &[f64]) -> Vec<Circle> {
    let n = sizes.len();
    let mut circles: Vec<Circle> = sizes
        .iter()
        .map(|s| Circle {
            x: 1e10,
            y: 1e10,
            radius: (s / PI).sqrt(),
        })
        .collect();
    let pairs: Vec<IArea> = areas
        .iter()
        .filter(|a| a.sets.len() == 2)
        .cloned()
        .collect();
    // (other set, size, weight)
    let mut overlaps: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for a in &pairs {
        let (l, r) = (a.sets[0], a.sets[1]);
        let weight = if a.size + SMALL >= sizes[l].min(sizes[r]) {
            0.0
        } else {
            1.0
        };
        overlaps[l].push((r, a.size, weight));
        overlaps[r].push((l, a.size, weight));
    }
    let mut most: Vec<(usize, f64)> = (0..n)
        .map(|i| (i, overlaps[i].iter().map(|o| o.1 * o.2).sum()))
        .collect();
    most.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut positioned = vec![false; n];
    if let Some(&(first, _)) = most.first() {
        circles[first].x = 0.0;
        circles[first].y = 0.0;
        positioned[first] = true;
    }
    for &(idx, _) in most.iter().skip(1) {
        let mut ov: Vec<(usize, f64, f64)> = overlaps[idx]
            .iter()
            .filter(|o| positioned[o.0])
            .copied()
            .collect();
        ov.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let r = circles[idx].radius;
        let mut points = Vec::new();
        for j in 0..ov.len() {
            let p1 = circles[ov[j].0];
            let d1 = distance_from_intersect_area(r, p1.radius, ov[j].1);
            points.push(Point::new(p1.x + d1, p1.y));
            points.push(Point::new(p1.x - d1, p1.y));
            points.push(Point::new(p1.x, p1.y + d1));
            points.push(Point::new(p1.x, p1.y - d1));
            for o2 in &ov[j + 1..] {
                let p2 = circles[o2.0];
                let d2 = distance_from_intersect_area(r, p2.radius, o2.1);
                points.extend(circle_circle_intersection(
                    &Circle {
                        x: p1.x,
                        y: p1.y,
                        radius: d1,
                    },
                    &Circle {
                        x: p2.x,
                        y: p2.y,
                        radius: d2,
                    },
                ));
            }
        }
        // venn.js throws "missing pairwise overlap information" here; addMissingAreas
        // guarantees every pair exists, so there is always a candidate.
        let mut best_loss = 1e50;
        let mut best = points.first().copied().unwrap_or(Point::new(0.0, 0.0));
        for p in &points {
            circles[idx].x = p.x;
            circles[idx].y = p.y;
            let l = loss_function(&circles, &pairs);
            if l < best_loss {
                best_loss = l;
                best = *p;
            }
        }
        circles[idx].x = best.x;
        circles[idx].y = best.y;
        positioned[idx] = true;
    }
    circles
}

/// Deterministic stand-in for `Math.random` in the constrained-MDS restarts.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

/// `constrainedMDSLayout`: MDS on the pairwise target distances, where subset/disjoint
/// pairs only constrain one side. venn.js restarts from `Math.random()`; we use a fixed
/// seed so output is reproducible.
fn constrained_mds_layout(areas: &[IArea], sizes: &[f64]) -> Vec<Circle> {
    let n = sizes.len();
    let mut distances = vec![vec![0.0; n]; n];
    let mut constraints = vec![vec![0.0; n]; n];
    for a in areas.iter().filter(|a| a.sets.len() == 2) {
        let (l, r) = (a.sets[0], a.sets[1]);
        let r1 = (sizes[l] / PI).sqrt();
        let r2 = (sizes[r] / PI).sqrt();
        let d = distance_from_intersect_area(r1, r2, a.size);
        distances[l][r] = d;
        distances[r][l] = d;
        let c = if a.size + 1e-10 >= sizes[l].min(sizes[r]) {
            1.0
        } else if a.size <= 1e-10 {
            -1.0
        } else {
            0.0
        };
        constraints[l][r] = c;
        constraints[r][l] = c;
    }
    let row_norms: Vec<f64> = distances.iter().map(|r| norm2(r)).collect();
    let norm = norm2(&row_norms) / n as f64;
    if norm > 0.0 {
        for row in &mut distances {
            for v in row.iter_mut() {
                *v /= norm;
            }
        }
    }
    let obj = |x: &[f64], fxprime: &mut [f64]| -> f64 {
        fxprime.iter_mut().for_each(|v| *v = 0.0);
        let mut loss = 0.0;
        for i in 0..n {
            let (xi, yi) = (x[2 * i], x[2 * i + 1]);
            for j in i + 1..n {
                let (xj, yj) = (x[2 * j], x[2 * j + 1]);
                let dij = distances[i][j];
                let c = constraints[i][j];
                let sq = (xj - xi) * (xj - xi) + (yj - yi) * (yj - yi);
                let d = sq.sqrt();
                let delta = sq - dij * dij;
                if (c > 0.0 && d <= dij) || (c < 0.0 && d >= dij) {
                    continue;
                }
                loss += 2.0 * delta * delta;
                fxprime[2 * i] += 4.0 * delta * (xi - xj);
                fxprime[2 * i + 1] += 4.0 * delta * (yi - yj);
                fxprime[2 * j] += 4.0 * delta * (xj - xi);
                fxprime[2 * j + 1] += 4.0 * delta * (yj - yi);
            }
        }
        loss
    };
    let mut rng = Rng(0x5eed);
    let mut best: Option<CgPoint> = None;
    for _ in 0..10 {
        let initial: Vec<f64> = (0..2 * n).map(|_| rng.next()).collect();
        let cur = conjugate_gradient(&obj, &initial, 500);
        if best.as_ref().is_none_or(|b| cur.fx < b.fx) {
            best = Some(cur);
        }
    }
    let x = best.map(|b| b.x).unwrap_or_else(|| vec![0.0; 2 * n]);
    (0..n)
        .map(|i| Circle {
            x: x[2 * i] * norm,
            y: x[2 * i + 1] * norm,
            radius: (sizes[i] / PI).sqrt(),
        })
        .collect()
}

/// Order of an object's keys in JavaScript: canonical array indices ascending first,
/// then the other keys in insertion order. Returns positions into `names`.
fn js_key_order(names: &[&str]) -> Vec<usize> {
    let index = |s: &str| -> Option<u64> {
        if s.is_empty()
            || (s.len() > 1 && s.starts_with('0'))
            || !s.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        s.parse::<u64>().ok().filter(|v| *v < u32::MAX as u64)
    };
    let mut ints: Vec<(u64, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| index(n).map(|v| (v, i)))
        .collect();
    ints.sort();
    let mut out: Vec<usize> = ints.into_iter().map(|(_, i)| i).collect();
    out.extend((0..names.len()).filter(|i| index(names[*i]).is_none()));
    out
}

/// A positioned circle for one set (`setid` = index into the set-name table).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetCircle {
    pub set: usize,
    pub circle: Circle,
}

/// `venn()`: solve circle positions. The result is in the circles object's key order.
fn venn_solve(areas: &[IArea], sizes: &[f64], names: &[String]) -> Vec<Circle> {
    let areas = add_missing_areas(areas, sizes.len(), names);
    let mut circles = greedy_layout(&areas, sizes);
    if areas.len() >= 8 {
        let constrained = constrained_mds_layout(&areas, sizes);
        if loss_function(&constrained, &areas) + 1e-8 < loss_function(&circles, &areas) {
            circles = constrained;
        }
    }
    let initial: Vec<f64> = circles.iter().flat_map(|c| [c.x, c.y]).collect();
    let radii: Vec<f64> = circles.iter().map(|c| c.radius).collect();
    let (pos, _) = nelder_mead(
        |v| {
            let cur: Vec<Circle> = radii
                .iter()
                .enumerate()
                .map(|(i, &r)| Circle {
                    x: v[2 * i],
                    y: v[2 * i + 1],
                    radius: r,
                })
                .collect();
            loss_function(&cur, &areas)
        },
        &initial,
        &NmParams {
            max_iterations: 500,
            min_error_delta: 1e-6,
            min_tolerance: 1e-5,
        },
    );
    for (i, c) in circles.iter_mut().enumerate() {
        c.x = pos[2 * i];
        c.y = pos[2 * i + 1];
    }
    circles
}

struct Bounds2 {
    x: (f64, f64),
    y: (f64, f64),
}

fn bounding_box(cs: &[SetCircle]) -> Bounds2 {
    let mut b = Bounds2 {
        x: (f64::INFINITY, f64::NEG_INFINITY),
        y: (f64::INFINITY, f64::NEG_INFINITY),
    };
    for c in cs {
        let c = c.circle;
        b.x.0 = b.x.0.min(c.x - c.radius);
        b.x.1 = b.x.1.max(c.x + c.radius);
        b.y.0 = b.y.0.min(c.y - c.radius);
        b.y.1 = b.y.1.max(c.y + c.radius);
    }
    b
}

fn orientate_circles(cs: &mut [SetCircle], orientation: f64) {
    cs.sort_by(|a, b| {
        b.circle
            .radius
            .partial_cmp(&a.circle.radius)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(first) = cs.first().copied() {
        for c in cs.iter_mut() {
            c.circle.x -= first.circle.x;
            c.circle.y -= first.circle.y;
        }
    }
    if cs.len() == 2 {
        let d = dist(cs[0].circle.center(), cs[1].circle.center());
        if d < (cs[1].circle.radius - cs[0].circle.radius).abs() {
            cs[1].circle.x = cs[0].circle.x + cs[0].circle.radius - cs[1].circle.radius - 1e-10;
            cs[1].circle.y = cs[0].circle.y;
        }
    }
    if cs.len() > 1 {
        let rotation = cs[1].circle.x.atan2(cs[1].circle.y) - orientation;
        let (c, s) = (rotation.cos(), rotation.sin());
        for k in cs.iter_mut() {
            let (x, y) = (k.circle.x, k.circle.y);
            k.circle.x = c * x - s * y;
            k.circle.y = s * x + c * y;
        }
    }
    if cs.len() > 2 {
        let mut angle = cs[2].circle.x.atan2(cs[2].circle.y) - orientation;
        while angle < 0.0 {
            angle += TAU;
        }
        while angle > TAU {
            angle -= TAU;
        }
        if angle > PI {
            let slope = cs[1].circle.y / (1e-10 + cs[1].circle.x);
            for k in cs.iter_mut() {
                let d = (k.circle.x + slope * k.circle.y) / (1.0 + slope * slope);
                k.circle.x = 2.0 * d - k.circle.x;
                k.circle.y = 2.0 * d * slope - k.circle.y;
            }
        }
    }
}

/// Reorder a solution array into JS object-key order (`toObjectNotation` round trip).
fn object_order(cs: Vec<SetCircle>, names: &[String]) -> Vec<SetCircle> {
    // Later duplicates overwrite earlier keys but keep the first insertion position.
    let mut uniq: Vec<SetCircle> = Vec::new();
    for c in cs {
        match uniq.iter_mut().find(|u| u.set == c.set) {
            Some(u) => *u = c,
            None => uniq.push(c),
        }
    }
    let keys: Vec<&str> = uniq.iter().map(|c| names[c.set].as_str()).collect();
    js_key_order(&keys).into_iter().map(|i| uniq[i]).collect()
}

/// `normalizeSolution`: orient each disjoint cluster (largest circle at the origin, second
/// one at `orientation`), then pack the clusters left-to-right / top-to-bottom.
fn normalize_solution(
    solution: &[SetCircle],
    orientation: f64,
    names: &[String],
) -> Vec<SetCircle> {
    let n = solution.len();
    // disjointCluster: union-find in array order.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (solution[i].circle, solution[j].circle);
            if dist(a.center(), b.center()) + 1e-10 < a.radius + b.radius {
                let xr = find(&mut parent, j);
                let yr = find(&mut parent, i);
                parent[xr] = yr;
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    let mut clusters: Vec<Vec<SetCircle>> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        let key = solution[r].set;
        match roots.iter().position(|k| *k == key) {
            Some(p) => clusters[p].push(solution[i]),
            None => {
                roots.push(key);
                clusters.push(vec![solution[i]]);
            }
        }
    }
    let mut sized: Vec<(Vec<SetCircle>, f64, Bounds2)> = clusters
        .into_iter()
        .map(|mut c| {
            orientate_circles(&mut c, orientation);
            let b = bounding_box(&c);
            let size = (b.x.1 - b.x.0) * (b.y.1 - b.y.0);
            (c, size, b)
        })
        .collect();
    sized.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut iter = sized.into_iter();
    let Some((mut circles, _, mut ret_bounds)) = iter.next() else {
        return Vec::new();
    };
    let spacing = (ret_bounds.x.1 - ret_bounds.x.0) / 50.0;
    let rest: Vec<(Vec<SetCircle>, f64, Bounds2)> = iter.collect();
    let add = |circles: &mut Vec<SetCircle>,
               ret: &Bounds2,
               cluster: Option<&(Vec<SetCircle>, f64, Bounds2)>,
               right: bool,
               bottom: bool| {
        let Some((cl, _, b)) = cluster else {
            return;
        };
        let x_off = if right {
            ret.x.1 - b.x.0 + spacing
        } else {
            let mut o = ret.x.1 - b.x.1;
            let centring = (b.x.1 - b.x.0) / 2.0 - (ret.x.1 - ret.x.0) / 2.0;
            if centring < 0.0 {
                o += centring;
            }
            o
        };
        let y_off = if bottom {
            ret.y.1 - b.y.0 + spacing
        } else {
            let mut o = ret.y.1 - b.y.1;
            let centring = (b.y.1 - b.y.0) / 2.0 - (ret.y.1 - ret.y.0) / 2.0;
            if centring < 0.0 {
                o += centring;
            }
            o
        };
        for c in cl {
            let mut c = *c;
            c.circle.x += x_off;
            c.circle.y += y_off;
            circles.push(c);
        }
    };
    let mut index = 0;
    while index < rest.len() {
        add(&mut circles, &ret_bounds, rest.get(index), true, false);
        add(&mut circles, &ret_bounds, rest.get(index + 1), false, true);
        add(&mut circles, &ret_bounds, rest.get(index + 2), true, true);
        index += 3;
        ret_bounds = bounding_box(&circles);
    }
    object_order(circles, names)
}

/// `scaleSolution`: fit the circles into `width × height` minus `padding` on each side.
fn scale_solution(solution: &[SetCircle], width: f64, height: f64, padding: f64) -> Vec<SetCircle> {
    let width = width - 2.0 * padding;
    let height = height - 2.0 * padding;
    let b = bounding_box(solution);
    if b.x.1 == b.x.0 || b.y.1 == b.y.0 {
        return solution.to_vec();
    }
    let scaling = (height / (b.y.1 - b.y.0)).min(width / (b.x.1 - b.x.0));
    let x_off = (width - (b.x.1 - b.x.0) * scaling) / 2.0;
    let y_off = (height - (b.y.1 - b.y.0) * scaling) / 2.0;
    solution
        .iter()
        .map(|c| SetCircle {
            set: c.set,
            circle: Circle {
                radius: scaling * c.circle.radius,
                x: padding + x_off + (c.circle.x - b.x.0) * scaling,
                y: padding + y_off + (c.circle.y - b.y.0) * scaling,
            },
        })
        .collect()
}

fn circle_margin(p: Point, interior: &[Circle], exterior: &[Circle]) -> f64 {
    let mut margin = interior[0].radius - dist(interior[0].center(), p);
    for c in &interior[1..] {
        let m = c.radius - dist(c.center(), p);
        if m <= margin {
            margin = m;
        }
    }
    for c in exterior {
        let m = dist(c.center(), p) - c.radius;
        if m <= margin {
            margin = m;
        }
    }
    margin
}

/// `computeTextCentre`: the point inside every `interior` circle and outside every
/// `exterior` one that is furthest from all their edges. `None` = region not drawn.
pub fn compute_text_centre(interior: &[Circle], exterior: &[Circle]) -> Option<Point> {
    let first = interior.first()?;
    let mut points = Vec::new();
    for c in interior {
        points.push(Point::new(c.x, c.y));
        points.push(Point::new(c.x + c.radius / 2.0, c.y));
        points.push(Point::new(c.x - c.radius / 2.0, c.y));
        points.push(Point::new(c.x, c.y + c.radius / 2.0));
        points.push(Point::new(c.x, c.y - c.radius / 2.0));
    }
    let mut initial = points[0];
    let mut margin = circle_margin(points[0], interior, exterior);
    for p in &points[1..] {
        let m = circle_margin(*p, interior, exterior);
        if m >= margin {
            initial = *p;
            margin = m;
        }
    }
    let (sol, _) = nelder_mead(
        |p| -circle_margin(Point::new(p[0], p[1]), interior, exterior),
        &[initial.x, initial.y],
        &NmParams {
            max_iterations: 500,
            min_error_delta: 1e-10,
            min_tolerance: 1e-10,
        },
    );
    let ret = Point::new(sol[0], sol[1]);
    let valid = interior.iter().all(|c| dist(ret, c.center()) <= c.radius)
        && exterior.iter().all(|c| dist(ret, c.center()) >= c.radius);
    if valid {
        return Some(ret);
    }
    if interior.len() == 1 {
        return Some(first.center());
    }
    let stats = intersection_area(interior);
    match stats.arcs.len() {
        0 => None,
        1 => Some(stats.arcs[0].circle.center()),
        _ if !exterior.is_empty() => compute_text_centre(interior, &[]),
        _ => Some(get_center(
            &stats.arcs.iter().map(|a| a.p1).collect::<Vec<_>>(),
        )),
    }
}

/// `computeTextCentres` for each area (sets as indices into the set-name table).
fn compute_text_centres(circles: &[SetCircle], areas: &[Vec<usize>]) -> Vec<Option<Point>> {
    // getOverlappingCircles: for each circle, the circles that fully contain it.
    let mut containers: Vec<Vec<usize>> = vec![Vec::new(); circles.len()];
    for i in 0..circles.len() {
        for j in i + 1..circles.len() {
            let (a, b) = (circles[i].circle, circles[j].circle);
            let d = dist(a.center(), b.center());
            if d + b.radius <= a.radius + 1e-10 {
                containers[j].push(circles[i].set);
            } else if d + a.radius <= b.radius + 1e-10 {
                containers[i].push(circles[j].set);
            }
        }
    }
    areas
        .iter()
        .map(|sets| {
            let mut exclude: Vec<usize> = Vec::new();
            for s in sets {
                if let Some(k) = circles.iter().position(|c| c.set == *s) {
                    exclude.extend(&containers[k]);
                }
            }
            let mut interior = Vec::new();
            let mut exterior = Vec::new();
            for c in circles {
                if sets.contains(&c.set) {
                    interior.push(c.circle);
                } else if !exclude.contains(&c.set) {
                    exterior.push(c.circle);
                }
            }
            compute_text_centre(&interior, &exterior)
        })
        .collect()
}

/// A solved venn layout for one canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct VennLayout {
    /// Set names (`setid`s).
    pub names: Vec<String>,
    /// Scaled circles in the order venn.js iterates them.
    pub circles: Vec<SetCircle>,
    /// Label centre of each input area (same order as the input), `None` if not drawn.
    pub text_centres: Vec<Option<Point>>,
}

impl VennLayout {
    pub fn circle_of(&self, name: &str) -> Option<Circle> {
        let i = self.names.iter().position(|n| n == name)?;
        self.circles.iter().find(|c| c.set == i).map(|c| c.circle)
    }
}

/// The solved (unscaled, normalised) positions: shared by every canvas size.
#[derive(Debug, Clone, PartialEq)]
pub struct VennSolution {
    pub names: Vec<String>,
    pub circles: Vec<SetCircle>,
    areas: Vec<Vec<usize>>,
}

/// Solve and normalise the circle layout for `data` (sets of size 0 must already be
/// filtered out, as `VennDiagram` does).
pub fn solve(data: &[Subset]) -> VennSolution {
    let mut names: Vec<String> = Vec::new();
    let mut sizes: Vec<f64> = Vec::new();
    for s in data.iter().filter(|s| s.sets.len() == 1) {
        match names.iter().position(|n| *n == s.sets[0]) {
            Some(i) => sizes[i] = s.size,
            None => {
                names.push(s.sets[0].clone());
                sizes.push(s.size);
            }
        }
    }
    // Index the circles in the circles object's key order.
    let order = js_key_order(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let names: Vec<String> = order.iter().map(|&i| names[i].clone()).collect();
    let sizes: Vec<f64> = order.iter().map(|&i| sizes[i]).collect();
    let idx = |n: &str| names.iter().position(|x| x == n);
    let areas: Vec<Vec<usize>> = data
        .iter()
        .map(|s| s.sets.iter().filter_map(|n| idx(n)).collect())
        .collect();
    let iareas: Vec<IArea> = data
        .iter()
        .zip(&areas)
        .filter(|(s, a)| a.len() == s.sets.len())
        .map(|(s, a)| IArea {
            sets: a.clone(),
            size: s.size,
        })
        .collect();
    if names.is_empty() {
        return VennSolution {
            names,
            circles: Vec::new(),
            areas,
        };
    }
    let solved = venn_solve(&iareas, &sizes, &names);
    let circles: Vec<SetCircle> = solved
        .into_iter()
        .enumerate()
        .map(|(set, circle)| SetCircle { set, circle })
        .collect();
    let circles = normalize_solution(&circles, PI / 2.0, &names);
    VennSolution {
        names,
        circles,
        areas,
    }
}

impl VennSolution {
    /// `scaleSolution` + `computeTextCentres` for one canvas.
    pub fn scaled(&self, width: f64, height: f64, padding: f64) -> VennLayout {
        let circles = scale_solution(&self.circles, width, height, padding);
        let text_centres = compute_text_centres(&circles, &self.areas);
        VennLayout {
            names: self.names.clone(),
            circles,
            text_centres,
        }
    }
}

/// Boundary of the region common to `circles` as path commands (`intersectionAreaPath`).
pub fn intersection_path(circles: &[Circle]) -> Vec<PathCmd> {
    if circles.is_empty() {
        return Vec::new();
    }
    let arcs = intersection_area(circles).arcs;
    match arcs.len() {
        0 => Vec::new(),
        1 => {
            let c = arcs[0].circle;
            geom::ellipse(c.center(), c.radius, c.radius)
        }
        _ => {
            let mut cmds = vec![PathCmd::MoveTo(arcs[0].p2)];
            let mut cur = arcs[0].p2;
            for a in &arcs {
                let c = a.circle.center();
                let a0 = (cur.y - c.y).atan2(cur.x - c.x);
                let a1 = (a.p1.y - c.y).atan2(a.p1.x - c.x);
                let mut delta = (a1 - a0).rem_euclid(TAU);
                if delta < 1e-12 && a.large {
                    delta = TAU;
                }
                cmds.extend(geom::arc_cubics(c, a.circle.radius, a0, a0 + delta));
                cur = a.p1;
            }
            cmds.push(PathCmd::Close);
            cmds
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering (vennRenderer.ts)
// ---------------------------------------------------------------------------

/// Reference width the renderer's sizes are expressed in.
const REFERENCE_WIDTH: f64 = 1600.0;

fn var<'a>(cfg: &'a MermaidConfig, key: &str) -> Option<&'a str> {
    cfg.theme_variables
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn hsl(h: f64, s: f64, l: f64) -> Color {
    Color::new(format!("hsl({h}, {s}%, {l}%)"))
}

/// The theme's `venn1..venn8` colours (empty when the theme defines none).
pub fn theme_colors(cfg: &MermaidConfig, pal: &Palette) -> Vec<Color> {
    let parse = |k: &str| var(cfg, k).and_then(color::parse);
    let adj = |c: Rgba, dh: f64, dl: f64| -> Color {
        let (h, s, l) = c.to_hsl();
        hsl(h + dh, s, (l + dl).clamp(0.0, 100.0))
    };
    let base: Vec<Color> = match cfg.theme {
        ThemeName::Base => {
            let primary = parse("primaryColor").unwrap_or(Rgba {
                r: 0xff,
                g: 0xf4,
                b: 0xdd,
                a: 1.0,
            });
            let secondary = parse("secondaryColor").unwrap_or(primary.rotate_hue(-120.0));
            let tertiary = parse("tertiaryColor").unwrap_or(primary.adjust_hsl(180.0, 0.0, 5.0));
            vec![
                adj(primary, 0.0, -30.0),
                adj(secondary, 0.0, -30.0),
                adj(tertiary, 0.0, -30.0),
                adj(primary, 60.0, -30.0),
                adj(primary, -60.0, -30.0),
                adj(secondary, 60.0, -30.0),
                adj(primary, 120.0, -30.0),
                adj(secondary, 120.0, -30.0),
            ]
        }
        ThemeName::Default | ThemeName::Null => [
            "hsl(240, 100%, 66.2745098039%)",
            "hsl(60, 100%, 63.5294117647%)",
            "hsl(80, 100%, 56.2745098039%)",
            "hsl(300, 100%, 66.2745098039%)",
            "hsl(180, 100%, 66.2745098039%)",
            "hsl(120, 100%, 63.5294117647%)",
            "hsl(0, 100%, 66.2745098039%)",
            "hsl(180, 100%, 63.5294117647%)",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect(),
        ThemeName::Dark => [
            "hsl(180, 1.5873015873%, 42.3529411765%)",
            "hsl(0, 100%, 32.1568627451%)",
            "hsl(321.6393442623, 65.5913978495%, 48.2352941176%)",
            "hsl(194.4, 16.5562913907%, 59.6078431373%)",
            "hsl(23.0769230769, 49.0566037736%, 50.7843137255%)",
            "hsl(0, 83.3333333333%, 53.5294117647%)",
            "hsl(289.1666666667, 100%, 44.1176470588%)",
            "hsl(35.1315789474, 98.7012987013%, 60.1960784314%)",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect(),
        ThemeName::Forest => [
            "hsl(78.1578947368, 58.4615384615%, 44.5098039216%)",
            "hsl(98.961038961, 100%, 54.9019607843%)",
            "hsl(78.1578947368, 58.4615384615%, 54.5098039216%)",
            "hsl(138.1578947368, 58.4615384615%, 44.5098039216%)",
            "hsl(18.1578947368, 58.4615384615%, 44.5098039216%)",
            "hsl(158.961038961, 100%, 54.9019607843%)",
            "hsl(198.1578947368, 58.4615384615%, 44.5098039216%)",
            "hsl(218.961038961, 100%, 54.9019607843%)",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect(),
        ThemeName::Neutral => [
            "#555", "#F4F4F4", "#555", "#BBB", "#777", "#999", "#DDD", "#FFF",
        ]
        .iter()
        .map(|s| Color::new(*s))
        .collect(),
        ThemeName::ReduxColor | ThemeName::ReduxDarkColor => pal
            .color_slots
            .as_ref()
            .map(|p| p.border.iter().take(8).cloned().collect())
            .unwrap_or_default(),
        ThemeName::Neo | ThemeName::NeoDark | ThemeName::Redux | ThemeName::ReduxDark => Vec::new(),
    };
    let mut out: Vec<Option<Color>> = (0..8).map(|i| base.get(i).cloned()).collect();
    for (i, slot) in out.iter_mut().enumerate() {
        if let Some(v) = var(cfg, &format!("venn{}", i + 1)) {
            *slot = Some(Color::new(v));
        }
    }
    out.into_iter().flatten().collect()
}

/// `primaryColor` of the theme: the fallback circle colour of themes without `venn*`.
fn primary_color(cfg: &MermaidConfig, pal: &Palette) -> Color {
    if let Some(v) = var(cfg, "primaryColor") {
        return Color::new(v);
    }
    match cfg.theme {
        ThemeName::Neo | ThemeName::Redux | ThemeName::ReduxColor => Color::new("#cccccc"),
        ThemeName::NeoDark | ThemeName::ReduxDark | ThemeName::ReduxDarkColor => {
            Color::new("#1f2020")
        }
        _ => pal.main_bkg.clone(),
    }
}

/// `color` with its alpha multiplied by `alpha`, as an `rgba()` string.
fn with_alpha(c: &Color, alpha: f64) -> Color {
    match color::parse(c.css()) {
        Some(rgb) => Color::new(format!(
            "rgba({}, {}, {}, {})",
            rgb.r,
            rgb.g,
            rgb.b,
            (rgb.a * alpha).clamp(0.0, 1.0)
        )),
        None => c.clone(),
    }
}

fn shift_lightness(c: &Color, delta: f64) -> Color {
    match color::parse(c.css()) {
        Some(rgb) => {
            let (h, s, l) = rgb.to_hsl();
            Color::new(Rgba::from_hsl(h, s, (l + delta).clamp(0.0, 100.0), rgb.a).to_hex())
        }
        None => c.clone(),
    }
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("px").trim().parse::<f64>().ok()
}

/// Styles keyed by the joined sorted target list (later statements merge over earlier).
fn style_map(entries: &[StyleEntry]) -> HashMap<String, Vec<(String, String)>> {
    let mut m: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for e in entries {
        let slot = m.entry(e.targets.join("|")).or_default();
        for (k, v) in &e.styles {
            match slot.iter_mut().find(|(sk, _)| sk == k) {
                Some(x) => x.1 = v.clone(),
                None => slot.push((k.clone(), v.clone())),
            }
        }
    }
    m
}

fn style_get<'a>(
    m: &'a HashMap<String, Vec<(String, String)>>,
    key: &str,
    prop: &str,
) -> Option<&'a str> {
    m.get(key)?
        .iter()
        .find(|(k, _)| k == prop)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// Parse, lay out and draw a `venn` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let venn = parse(body, title)?;
    Ok(draw(&venn, cfg, pal))
}

/// Draw a parsed diagram.
pub fn draw(venn: &Venn, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let s = "venn";
    let width = cfg.section_f64(s, "width", 800.0).max(1.0);
    let height = cfg.section_f64(s, "height", 450.0).max(1.0);
    let padding = cfg.section_f64(s, "padding", 8.0).max(0.0);
    let debug = cfg.section_bool(s, "useDebugLayout", false);
    let scale = width / REFERENCE_WIDTH;
    let title = venn.title.as_ref().filter(|t| !t.is_empty());
    let title_height = if title.is_some() { 48.0 * scale } else { 0.0 };
    let styles = style_map(&venn.styles);
    let palette = theme_colors(cfg, pal);
    let primary = primary_color(cfg, pal);
    let set_text = var(cfg, "vennSetTextColor").map_or_else(|| pal.text_color.clone(), Color::new);
    let title_color =
        var(cfg, "vennTitleTextColor").map_or_else(|| pal.title_color.clone(), Color::new);

    // VennDiagram drops sets of size 0 and every area that mentions them.
    let render_sets = ensure_pairwise_subsets(&venn.subsets);
    let removed: Vec<&String> = render_sets
        .iter()
        .filter(|d| d.size == 0.0 && d.sets.len() == 1)
        .map(|d| &d.sets[0])
        .collect();
    let data: Vec<Subset> = render_sets
        .iter()
        .filter(|d| !d.sets.iter().any(|s| removed.contains(&s)))
        .cloned()
        .collect();

    let canvas_h = height - title_height;
    let solution = solve(&data);
    // The chart itself uses venn.js's default padding (15); mermaid's `padding` only
    // reaches the second `venn.layout` call that positions the text nodes.
    let chart = solution.scaled(width, canvas_h, 15.0);
    let text_layout = solution.scaled(width, canvas_h, padding);

    let mut els = Vec::new();
    if let Some(t) = title {
        els.push(Element::classed_text(
            Text::centered(
                Point::new(width / 2.0, 32.0 * scale),
                std::slice::from_ref(t),
                32.0,
                32.0,
                &title_color,
            ),
            "venn-title",
        ));
    }

    let label_font = 48.0 * scale;
    let mut areas_els = Vec::new();
    let mut circle_index = 0;
    for (k, d) in data.iter().enumerate() {
        let key = d.sets.join("|");
        let circles: Vec<Circle> = d.sets.iter().filter_map(|n| chart.circle_of(n)).collect();
        if circles.len() != d.sets.len() {
            continue;
        }
        let mut group = Vec::new();
        let text_fill = if d.sets.len() == 1 {
            let i = circle_index;
            circle_index += 1;
            let base = style_get(&styles, &key, "fill")
                .map(Color::new)
                .or_else(|| (!palette.is_empty()).then(|| palette[i % palette.len()].clone()))
                .unwrap_or_else(|| primary.clone());
            let fill_opacity = style_get(&styles, &key, "fill-opacity")
                .and_then(parse_num)
                .unwrap_or(0.1);
            let stroke =
                style_get(&styles, &key, "stroke").map_or_else(|| base.clone(), Color::new);
            let stroke_w = style_get(&styles, &key, "stroke-width")
                .and_then(parse_num)
                .unwrap_or(5.0 * scale);
            let c = circles[0];
            let geom = Geometry::Path(geom::ellipse(c.center(), c.radius, c.radius));
            group.push(Element::shape(
                geom.clone(),
                Paint::fill(&with_alpha(&base, fill_opacity)),
            ));
            group.push(Element::shape(
                geom,
                Paint::stroke(&with_alpha(&stroke, 0.95), stroke_w),
            ));
            style_get(&styles, &key, "color").map_or_else(
                || shift_lightness(&base, if pal.dark { 30.0 } else { -30.0 }),
                Color::new,
            )
        } else {
            if let Some(fill) = style_get(&styles, &key, "fill") {
                let path = intersection_path(&circles);
                if !path.is_empty() {
                    group.push(Element::path(path, Paint::fill(&Color::new(fill))));
                }
            }
            style_get(&styles, &key, "color").map_or_else(|| set_text.clone(), Color::new)
        };
        let label = d
            .label
            .clone()
            .filter(|l| !l.is_empty())
            .or_else(|| (d.sets.len() == 1).then(|| d.sets[0].clone()));
        if let (Some(label), Some(Some(c))) = (label, chart.text_centres.get(k)) {
            // venn.js floors the centre and offsets the baseline by `dy=.35em`.
            group.push(Element::text(Text {
                x: c.x.floor(),
                y: c.y.floor() + 0.35 * label_font,
                lines: vec![label],
                line_height: label_font * 1.1,
                font_size: label_font,
                bold: false,
                anchor: Anchor::Middle,
                fill: text_fill,
            }));
        }
        let class = if d.sets.len() == 1 {
            "venn-area venn-circle"
        } else {
            "venn-area venn-intersection"
        };
        areas_els.push(Element::group(class, group));
    }
    let text_nodes = draw_text_nodes(venn, &data, &text_layout, scale, &styles, &set_text, debug);
    if !text_nodes.is_empty() {
        areas_els.push(Element::group("venn-text-nodes", text_nodes));
    }
    let shifted: Vec<Element> = areas_els
        .into_iter()
        .map(|e| translate_element(e, title_height))
        .collect();
    els.extend(shifted);

    Scene {
        kind: DiagramKind::Venn,
        view_box: Rect::new(0.0, 0.0, width, height),
        elements: vec![Element::group("venn", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn translate_element(e: Element, dy: f64) -> Element {
    if dy == 0.0 {
        return e;
    }
    match e {
        Element::Group {
            id,
            class,
            children,
        } => Element::Group {
            id,
            class,
            children: children
                .into_iter()
                .map(|c| translate_element(c, dy))
                .collect(),
        },
        Element::Shape { geom, paint, class } => Element::Shape {
            geom: geom.translated(0.0, dy),
            paint,
            class,
        },
        Element::Text { mut text, class } => {
            text.y += dy;
            Element::Text { text, class }
        }
    }
}

/// `renderTextNodes`: text nodes in a grid inside their region's inscribed circle.
fn draw_text_nodes(
    venn: &Venn,
    data: &[Subset],
    layout: &VennLayout,
    scale: f64,
    styles: &HashMap<String, Vec<(String, String)>>,
    set_text: &Color,
    debug: bool,
) -> Vec<Element> {
    let mut groups: Vec<(String, Vec<&TextNode>)> = Vec::new();
    for n in &venn.texts {
        let key = n.sets.join("|");
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(n),
            None => groups.push((key, vec![n])),
        }
    }
    let font = 40.0 * scale;
    let line_h = font * 1.15;
    let mut out = Vec::new();
    for (key, nodes) in groups {
        // Later areas with the same key win (a Map built in data order).
        let Some(k) = data.iter().rposition(|d| d.sets.join("|") == key) else {
            continue;
        };
        let Some(Some(centre)) = layout.text_centres.get(k) else {
            continue;
        };
        let circles: Vec<Circle> = data[k]
            .sets
            .iter()
            .filter_map(|n| layout.circle_of(n))
            .collect();
        if circles.is_empty() {
            continue;
        }
        let min_r = circles
            .iter()
            .map(|c| c.radius)
            .fold(f64::INFINITY, f64::min);
        let raw = circles
            .iter()
            .map(|c| c.radius - dist(*centre, c.center()))
            .fold(f64::INFINITY, f64::min);
        let mut inner = if raw.is_finite() { raw.max(0.0) } else { 0.0 };
        if inner == 0.0 && min_r.is_finite() {
            inner = min_r * 0.6;
        }
        let inner_w = (80.0 * scale).max(inner * 2.0 * 0.95);
        let inner_h = (60.0 * scale).max(inner * 2.0 * 0.95);
        let has_label = data[k].label.as_ref().is_some_and(|l| !l.is_empty());
        let base = if has_label {
            (32.0 * scale).min(inner * 0.25)
        } else {
            0.0
        };
        let label_offset = base + if nodes.len() <= 2 { 30.0 * scale } else { 0.0 };
        let start_x = centre.x - inner_w / 2.0;
        let start_y = centre.y - inner_h / 2.0 + label_offset;
        let cols = ((nodes.len() as f64).sqrt().ceil() as usize).max(1);
        let rows = nodes.len().div_ceil(cols).max(1);
        let cell_w = inner_w / cols as f64;
        let cell_h = inner_h / rows as f64;
        let mut area = Vec::new();
        if debug {
            area.push(Element::path(
                geom::ellipse(*centre, inner, inner),
                Paint::stroke(&Color::new("purple"), 1.5 * scale)
                    .dashed(vec![6.0 * scale, 4.0 * scale]),
            ));
        }
        for (i, node) in nodes.iter().enumerate() {
            let (col, row) = (i % cols, i / cols);
            let x = start_x + cell_w * (col as f64 + 0.5);
            let y = start_y + cell_h * (row as f64 + 0.5);
            if debug {
                area.push(Element::rect(
                    Rect::new(
                        start_x + cell_w * col as f64,
                        start_y + cell_h * row as f64,
                        cell_w,
                        cell_h,
                    ),
                    0.0,
                    Paint::stroke(&Color::new("teal"), scale)
                        .dashed(vec![4.0 * scale, 3.0 * scale]),
                ));
            }
            let label = node.label.clone().unwrap_or_else(|| node.id.clone());
            let lines = crate::diagram::timeline::wrap_text(&label, cell_w * 0.9, font, false);
            let fill =
                style_get(styles, &node.id, "color").map_or_else(|| set_text.clone(), Color::new);
            let lines: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
            if lines.is_empty() {
                continue;
            }
            area.push(Element::classed_text(
                Text::centered(Point::new(x, y), &lines, font, line_h, &fill),
                "venn-text-node",
            ));
        }
        out.push(Element::group("venn-text-area", area));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subsets(v: &Venn) -> Vec<(Vec<&str>, f64, Option<&str>)> {
        v.subsets
            .iter()
            .map(|s| {
                (
                    s.sets.iter().map(String::as_str).collect(),
                    s.size,
                    s.label.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn simple() {
        let v = parse(
            "venn-beta\n      title foo bar\n      set A\n      set B\n      union A,B\n",
            None,
        )
        .unwrap();
        assert_eq!(v.title.as_deref(), Some("foo bar"));
        assert_eq!(
            subsets(&v),
            vec![
                (vec!["A"], 10.0, None),
                (vec!["B"], 10.0, None),
                (vec!["A", "B"], 2.5, None)
            ]
        );
    }

    #[test]
    fn bracket_labels_quoted_and_unquoted() {
        for src in [
            "venn-beta\n set A[\"Alpha\"]\n set B[\"Beta\"]\n union A,B[\"AB\"]\n",
            "venn-beta\n set A[Alpha]\n set B[Beta]\n union A,B[AB]\n",
        ] {
            let v = parse(src, None).unwrap();
            assert_eq!(
                subsets(&v),
                vec![
                    (vec!["A"], 10.0, Some("Alpha")),
                    (vec!["B"], 10.0, Some("Beta")),
                    (vec!["A", "B"], 2.5, Some("AB"))
                ]
            );
        }
    }

    #[test]
    fn sizes_and_labels() {
        let v = parse(
            "venn-beta\n title foo bar\n set A[\"Alpha\"]:20\n set B[\"Beta\"]:12\n set C[\"Gamma\"]:30\n union A,B[\"AB\"]:5.3\n union C,A,B:1\n",
            None,
        )
        .unwrap();
        assert_eq!(
            subsets(&v),
            vec![
                (vec!["A"], 20.0, Some("Alpha")),
                (vec!["B"], 12.0, Some("Beta")),
                (vec!["C"], 30.0, Some("Gamma")),
                (vec!["A", "B"], 5.3, Some("AB")),
                (vec!["A", "B", "C"], 1.0, None),
            ]
        );
    }

    #[test]
    fn text_nodes() {
        let v = parse(
            "venn-beta\n  set A\n    text A1[\"foo bar\"]\n  set B\n    text B1[\"hello, world\"]\n  union A,B\n    text AB1[\"shared note\"]\n",
            None,
        )
        .unwrap();
        let t: Vec<(Vec<&str>, &str, Option<&str>)> = v
            .texts
            .iter()
            .map(|t| {
                (
                    t.sets.iter().map(String::as_str).collect(),
                    t.id.as_str(),
                    t.label.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            t,
            vec![
                (vec!["A"], "A1", Some("foo bar")),
                (vec!["B"], "B1", Some("hello, world")),
                (vec!["A", "B"], "AB1", Some("shared note")),
            ]
        );
        // Non-indented form names the region explicitly.
        let v = parse("venn-beta\nset A\nset B\ntext A,B note[\"x\"]\n", None).unwrap();
        assert_eq!(v.texts[0].sets, vec!["A", "B"]);
        assert_eq!(v.texts[0].id, "note");
    }

    #[test]
    fn indented_text_without_label() {
        let v = parse(
            "venn-beta\n  set A[\"Frontend\"]\n    text A1\n    text A2\n  set B[\"Backend\"]\n    text B1\n  union A,B[\"APIs\"]\n",
            None,
        )
        .unwrap();
        let ids: Vec<(&str, &str)> = v
            .texts
            .iter()
            .map(|t| (t.sets[0].as_str(), t.id.as_str()))
            .collect();
        assert_eq!(ids, vec![("A", "A1"), ("A", "A2"), ("B", "B1")]);
        assert!(v.texts.iter().all(|t| t.label.is_none()));
    }

    #[test]
    fn styles() {
        let v = parse(
            "venn-beta\n set A\n set B\n union A,B\n style A fill:#ff6b6b, color:#333\n style B,A color:#333\n style A fill:rgb(255, 0, 128)\n style B fill:rgba(255, 0, 128, 0.5)\n style A stroke-width:5 px\n",
            None,
        )
        .unwrap();
        assert_eq!(v.styles[0].targets, vec!["A"]);
        assert_eq!(
            v.styles[0].styles,
            vec![
                ("fill".to_string(), "#ff6b6b".to_string()),
                ("color".to_string(), "#333".to_string())
            ]
        );
        assert_eq!(v.styles[1].targets, vec!["A", "B"]);
        assert_eq!(v.styles[2].styles[0].1, "rgb(255, 0, 128)");
        assert_eq!(v.styles[3].styles[0].1, "rgba(255, 0, 128, 0.5)");
        assert_eq!(v.styles[4].styles[0].1, "5 px");
    }

    #[test]
    fn errors() {
        assert!(parse("venn-beta\n set A,B\n", None).is_err());
        let e = parse("venn-beta\n union A\n", None).unwrap_err();
        assert!(
            e.to_string()
                .contains("union requires multiple identifiers")
        );
        let e = parse("venn-beta\n set Foo\n union Foo,Buz\n", None).unwrap_err();
        assert!(e.to_string().contains("unknown set identifier: Buz"));
        assert!(parse("vennx\n", None).is_err());
        assert!(parse("venn-beta\n set A\n set B ?\n", None).is_err());
    }

    #[test]
    fn quoted_identifiers_sorted() {
        let v = parse(
            "venn-beta\n set \"Foo Bar\"\n set Buz\n union \"Foo Bar\",Buz\n",
            None,
        )
        .unwrap();
        assert_eq!(v.subsets[0].sets, vec!["Foo Bar"]);
        assert_eq!(v.subsets[2].sets, vec!["Buz", "Foo Bar"]);
        assert_eq!(v.subsets[2].size, 2.5);
    }

    #[test]
    fn first_match_lexer_quirks() {
        // `set` is matched before IDENTIFIER, so `settings` lexes as SET + `tings`.
        assert!(parse("venn-beta\n set settings\n", None).is_err());
        // Comments run to the end of the line.
        let v = parse("venn-beta\n set A %% note\n", None).unwrap();
        assert_eq!(v.subsets.len(), 1);
        // Case-insensitive keywords.
        let v = parse("venn-beta\n SET A\n Set B\n UNION A,B\n", None).unwrap();
        assert_eq!(v.subsets.len(), 3);
    }

    #[test]
    fn pairwise_subsets() {
        let s = |sets: &[&str], size: f64| Subset {
            sets: sets.iter().map(|x| x.to_string()).collect(),
            size,
            label: None,
        };
        assert!(ensure_pairwise_subsets(&[]).is_empty());
        let two = vec![s(&["A"], 10.0), s(&["B"], 10.0), s(&["A", "B"], 2.5)];
        assert_eq!(ensure_pairwise_subsets(&two), two);
        let three = vec![
            s(&["A"], 10.0),
            s(&["B"], 8.0),
            s(&["C"], 12.0),
            s(&["A", "B"], 1.0),
            s(&["A", "B", "C"], 1.0),
        ];
        let out = ensure_pairwise_subsets(&three);
        assert_eq!(out.len(), 7);
        assert_eq!(out[5].sets, vec!["A", "C"]);
        assert_eq!(out[5].size, 2.5);
        assert_eq!(out[6].sets, vec!["B", "C"]);
        assert_eq!(out[6].size, 2.0);
        let unknown = vec![s(&["A", "B", "C"], 1.0)];
        assert!(
            ensure_pairwise_subsets(&unknown)[1..]
                .iter()
                .all(|x| x.size == 2.5)
        );
    }

    #[test]
    fn overlap_math() {
        assert!((circle_overlap(1.0, 1.0, 0.0) - PI).abs() < 1e-12);
        assert_eq!(circle_overlap(1.0, 1.0, 2.0), 0.0);
        for &(r1, r2, o) in &[(2.0, 1.5, 1.0), (3.0, 3.0, 5.0), (1.0, 4.0, 0.5)] {
            let d = distance_from_intersect_area(r1, r2, o);
            assert!(
                (circle_overlap(r1, r2, d) - o).abs() < 1e-6,
                "{r1} {r2} {o}"
            );
        }
        // A pairwise intersection region has the same area as the analytic overlap.
        let a = Circle {
            x: 0.0,
            y: 0.0,
            radius: 2.0,
        };
        let b = Circle {
            x: 2.5,
            y: 0.3,
            radius: 1.5,
        };
        let stats = intersection_area(&[a, b]);
        let expect = circle_overlap(2.0, 1.5, dist(a.center(), b.center()));
        assert!((stats.area - expect).abs() < 1e-9);
        assert_eq!(stats.arcs.len(), 2);
        // Nested circles: the smaller circle is the intersection.
        let inner = Circle {
            x: 0.2,
            y: 0.0,
            radius: 0.5,
        };
        assert!((intersection_area(&[a, inner]).area - PI * 0.25).abs() < 1e-9);
    }

    fn layout_of(src: &str) -> (Vec<Subset>, VennLayout) {
        let v = parse(src, None).unwrap();
        let data = ensure_pairwise_subsets(&v.subsets);
        let l = solve(&data).scaled(800.0, 450.0, 15.0);
        (data, l)
    }

    #[test]
    fn two_set_layout_matches_reference() {
        // Docs example: mermaid draws r = 210 at x = 266.71 / 533.29, y = 225.
        let (_, l) = layout_of("venn-beta\n set A\n set B\n union A,B\n");
        let a = l.circle_of("A").unwrap();
        let b = l.circle_of("B").unwrap();
        assert!((a.radius - 210.0).abs() < 1e-6);
        assert!((a.x - 266.712).abs() < 0.01, "{a:?}");
        assert!((b.x - 533.288).abs() < 0.01, "{b:?}");
        assert!((a.y - 225.0).abs() < 1e-6);
        // The overlap keeps the requested proportion: 2.5 of 10.
        let ov = circle_overlap(a.radius, b.radius, dist(a.center(), b.center()));
        assert!((ov / (PI * a.radius * a.radius) - 0.25).abs() < 1e-3);
    }

    #[test]
    fn three_set_layout_matches_reference() {
        let (data, l) = layout_of(
            "venn-beta\n set Desirable\n set Feasible\n set Viable\n union Desirable,Feasible[\"Buildable\"]\n union Feasible,Viable[\"Sustainable\"]\n union Desirable,Viable[\"Marketable\"]\n union Desirable,Feasible,Viable[\"Ship it\"]\n",
        );
        // mermaid (in the group below the 24px title band): Desirable (319.16, 283.01), r = 127.99.
        let d = l.circle_of("Desirable").unwrap();
        let v = l.circle_of("Viable").unwrap();
        let canvas = solve(&data).scaled(800.0, 426.0, 15.0);
        let d2 = canvas.circle_of("Desirable").unwrap();
        assert!((d2.radius - 127.991_910_052_646_6).abs() < 1e-6, "{d2:?}");
        assert!((d2.x - 319.160_735_870_201_4).abs() < 1e-6, "{d2:?}");
        assert!((d2.y - 283.008_089_947_353_4).abs() < 1e-6, "{d2:?}");
        assert!(v.y < d.y, "Viable is on top");
        // Every label lies in its region.
        for (k, area) in data.iter().enumerate() {
            let p = l.text_centres[k].unwrap();
            for name in &area.sets {
                let c = l.circle_of(name).unwrap();
                assert!(dist(p, c.center()) <= c.radius + 1e-6);
            }
        }
    }

    #[test]
    fn disjoint_clusters_are_packed() {
        let (_, l) = layout_of("venn-beta\n set A\n set B\n set C:4\n union A,B\n");
        let c = l.circle_of("C").unwrap();
        let b = l.circle_of("B").unwrap();
        let a = l.circle_of("A").unwrap();
        assert!(dist(c.center(), b.center()) > c.radius + b.radius);
        assert!(dist(c.center(), a.center()) > c.radius + a.radius);
        // Packed to the right of the main cluster.
        assert!(c.x > a.x.max(b.x));
        for c in &l.circles {
            assert!(c.circle.x - c.circle.radius >= 15.0 - 1e-6);
            assert!(c.circle.x + c.circle.radius <= 785.0 + 1e-6);
        }
    }

    #[test]
    fn many_sets_are_deterministic_and_finite() {
        let src = "venn-beta\n set A:10\n set B:10\n set C:5\n set D:3\n set E:2\n union A,B:2\n union B,C:1\n union C,D:1\n union A,C:1\n union D,E:0.5\n union A,B,C:0.5\n";
        let (_, l1) = layout_of(src);
        let (_, l2) = layout_of(src);
        assert_eq!(l1, l2);
        assert!(
            l1.circles
                .iter()
                .all(|c| c.circle.x.is_finite() && c.circle.y.is_finite())
        );
    }

    #[test]
    fn js_key_order_puts_indices_first() {
        assert_eq!(
            js_key_order(&["b", "2", "a", "1", "01"]),
            vec![3, 1, 0, 2, 4]
        );
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default().scoped("venn");
        let pal = cfg.palette();
        let scene = render(
            "venn-beta\n title T\n set A[\"Alpha\"]:20\n   text A1[\"React\"]\n set B:12\n union A,B[\"AB\"]:3\n style A fill:#ff6b6b\n style A,B fill:#ffcc00\n style A1 color:red\n",
            None,
            &cfg,
            &pal,
        )
        .unwrap();
        assert_eq!(scene.view_box, Rect::new(0.0, 0.0, 800.0, 450.0));
        let svg = format!("{scene:?}");
        assert!(svg.contains("rgba(255, 107, 107, 0.1)"));
        assert!(svg.contains("#ffcc00"));
        assert!(svg.contains("\"red\""));
    }

    #[test]
    fn theme_palettes() {
        let mut cfg = MermaidConfig::default().scoped("venn");
        let pal = cfg.palette();
        assert_eq!(cfg.theme, ThemeName::ReduxColor);
        let c = theme_colors(&cfg, &pal);
        assert_eq!(c.len(), 8);
        assert_eq!(c[0].css().to_ascii_uppercase(), "#E879F9");
        cfg.theme = ThemeName::Neo;
        assert!(theme_colors(&cfg, &pal).is_empty());
        cfg.theme_variables.push(("venn2".into(), "#123456".into()));
        assert_eq!(theme_colors(&cfg, &pal), vec![Color::new("#123456")]);
    }
}
