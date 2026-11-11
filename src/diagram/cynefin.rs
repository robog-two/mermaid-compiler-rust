//! `cynefin-beta` diagrams (mermaid 12.3 `diagrams/cynefin`, langium grammar
//! `packages/parser/src/language/cynefin`).
//!
//! Four fixed quadrants (Complex top-left, Complicated top-right, Chaotic bottom-left,
//! Clear bottom-right) on an 800×600 canvas, wavy dashed boundaries generated from a seeded
//! mulberry32 PRNG, the Clear/Chaotic "cliff", the Confusion ellipse in the centre, item
//! badges below each domain's label and curved transition arrows between domain centres.
//! Geometry and constants follow `cynefinRenderer.ts` / `cynefinBoundaries.ts`.

use crate::color;
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::json::Json;
use crate::scene::{Anchor, Color, DiagramKind, Element, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::{Palette, ThemeName};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Complex,
    Complicated,
    Clear,
    Chaotic,
    Confusion,
}

impl Domain {
    pub const ALL: [Domain; 5] = [
        Domain::Complex,
        Domain::Complicated,
        Domain::Chaotic,
        Domain::Clear,
        Domain::Confusion,
    ];
    /// The four quadrants, in drawing order.
    pub const QUADRANTS: [Domain; 4] = [
        Domain::Complex,
        Domain::Complicated,
        Domain::Chaotic,
        Domain::Clear,
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            Domain::Complex => "complex",
            Domain::Complicated => "complicated",
            Domain::Clear => "clear",
            Domain::Chaotic => "chaotic",
            Domain::Confusion => "confusion",
        }
    }

    fn from_keyword(s: &str) -> Option<Domain> {
        Domain::ALL.into_iter().find(|d| d.keyword() == s)
    }

    /// Capitalised display name.
    pub fn title(self) -> &'static str {
        match self {
            Domain::Complex => "Complex",
            Domain::Complicated => "Complicated",
            Domain::Clear => "Clear",
            Domain::Chaotic => "Chaotic",
            Domain::Confusion => "Confusion",
        }
    }

    /// `DOMAIN_META`: decision model and practice type.
    pub fn meta(self) -> (&'static str, &'static str) {
        match self {
            Domain::Complex => (
                "Probe \u{2192} Sense \u{2192} Respond",
                "Emergent Practices",
            ),
            Domain::Complicated => ("Sense \u{2192} Analyse \u{2192} Respond", "Good Practices"),
            Domain::Clear => (
                "Sense \u{2192} Categorise \u{2192} Respond",
                "Best Practices",
            ),
            Domain::Chaotic => ("Act \u{2192} Sense \u{2192} Respond", "Novel Practices"),
            Domain::Confusion => ("", "Disorder"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainBlock {
    pub domain: Domain,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub from: Domain,
    pub to: Domain,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cynefin {
    pub title: Option<String>,
    /// One entry per declared domain (a later block replaces an earlier one in place).
    pub domains: Vec<DomainBlock>,
    /// Transitions, self-loops dropped.
    pub transitions: Vec<Transition>,
}

impl Cynefin {
    pub fn domain(&self, d: Domain) -> Option<&DomainBlock> {
        self.domains.iter().find(|b| b.domain == d)
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Header,
    Title(String),
    Domain(Domain),
    Arrow,
    Colon,
    Str(String),
    Newline,
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "end of input".into(),
        Some(Tok::Header) => "'cynefin-beta'".into(),
        Some(Tok::Title(_)) => "title".into(),
        Some(Tok::Domain(d)) => format!("'{}'", d.keyword()),
        Some(Tok::Arrow) => "'-->'".into(),
        Some(Tok::Colon) => "':'".into(),
        Some(Tok::Str(s)) => format!("\"{s}\""),
        Some(Tok::Newline) => "newline".into(),
    }
}

/// Langium's STRING value conversion: drop the quotes, resolve backslash escapes.
fn unquote(s: &str) -> String {
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn lex(body: &str) -> Result<Vec<(Tok, usize)>> {
    let mut out = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        let ln = idx + 1;
        let mut rest = line;
        loop {
            rest = rest.trim_start_matches([' ', '\t']);
            if rest.is_empty() || rest.starts_with("%%") {
                break;
            }
            let word_len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(rest.len());
            let word = &rest[..word_len];
            if let Some(after) = rest.strip_prefix("cynefin-beta") {
                out.push((Tok::Header, ln));
                rest = after.strip_prefix(':').unwrap_or(after);
                continue;
            }
            if word == "title" && (rest.len() == 5 || rest[5..].starts_with([' ', '\t'])) {
                let t = rest[5..].split("%%").next().unwrap_or("").trim();
                out.push((Tok::Title(t.to_string()), ln));
                break;
            }
            if let Some(d) = Domain::from_keyword(word) {
                out.push((Tok::Domain(d), ln));
                rest = &rest[word_len..];
                continue;
            }
            if let Some(after) = rest.strip_prefix("-->") {
                out.push((Tok::Arrow, ln));
                rest = after;
                continue;
            }
            if let Some(after) = rest.strip_prefix(':') {
                out.push((Tok::Colon, ln));
                rest = after;
                continue;
            }
            let q = rest.as_bytes()[0];
            if q == b'"' || q == b'\'' {
                let b = rest.as_bytes();
                let mut i = 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                if i < b.len() {
                    out.push((Tok::Str(unquote(&rest[..=i])), ln));
                    rest = &rest[i + 1..];
                    continue;
                }
            }
            let shown: String = rest.chars().take(20).collect();
            return Err(Error::parse(
                ln,
                format!(
                    "Unexpected character(s) '{shown}': expecting a domain name, a quoted item, '-->' or 'title'"
                ),
            ));
        }
        out.push((Tok::Newline, ln));
    }
    Ok(out)
}

/// Parse a `cynefin-beta` definition (body as produced by `diagram::preprocess`).
pub fn parse(body: &str, title: Option<String>) -> Result<Cynefin> {
    let toks = lex(body)?;
    let mut d = Cynefin {
        title,
        ..Cynefin::default()
    };
    let mut i = 0;
    let line_of = |i: usize| toks.get(i).or(toks.last()).map_or(1, |t| t.1);
    while matches!(toks.get(i), Some((Tok::Newline, _))) {
        i += 1;
    }
    if !matches!(toks.get(i), Some((Tok::Header, _))) {
        return Err(Error::parse(line_of(i), "Expecting 'cynefin-beta'"));
    }
    i += 1;
    let mut current: Option<usize> = None;
    while i < toks.len() {
        let line = toks[i].1;
        match &toks[i].0 {
            Tok::Newline => i += 1,
            Tok::Title(t) => {
                d.title = Some(t.clone());
                current = None;
                i += 1;
            }
            Tok::Domain(from) => {
                if matches!(toks.get(i + 1), Some((Tok::Arrow, _))) {
                    let Some((Tok::Domain(to), _)) = toks.get(i + 2) else {
                        return Err(Error::parse(
                            line,
                            format!(
                                "Expecting a domain name after '-->', got {}",
                                describe(toks.get(i + 2).map(|t| &t.0))
                            ),
                        ));
                    };
                    let mut j = i + 3;
                    let mut label = None;
                    if matches!(toks.get(j), Some((Tok::Colon, _))) {
                        match toks.get(j + 1) {
                            Some((Tok::Str(s), _)) => label = Some(s.clone()),
                            other => {
                                return Err(Error::parse(
                                    line,
                                    format!(
                                        "Expecting a quoted label after ':', got {}",
                                        describe(other.map(|t| &t.0))
                                    ),
                                ));
                            }
                        }
                        j += 2;
                    }
                    if !matches!(toks.get(j), None | Some((Tok::Newline, _))) {
                        return Err(Error::parse(
                            line,
                            format!(
                                "Expecting end of line after transition, got {}",
                                describe(toks.get(j).map(|t| &t.0))
                            ),
                        ));
                    }
                    if from != to {
                        d.transitions.push(Transition {
                            from: *from,
                            to: *to,
                            label: label.filter(|l| !l.is_empty()),
                        });
                    }
                    current = None;
                    i = j;
                } else {
                    let block = DomainBlock {
                        domain: *from,
                        items: Vec::new(),
                    };
                    let pos = match d.domains.iter().position(|b| b.domain == *from) {
                        Some(p) => {
                            d.domains[p] = block;
                            p
                        }
                        None => {
                            d.domains.push(block);
                            d.domains.len() - 1
                        }
                    };
                    current = Some(pos);
                    i += 1;
                }
            }
            Tok::Str(s) => {
                let Some(p) = current else {
                    return Err(Error::parse(
                        line,
                        format!(
                            "Item \"{s}\" must follow a domain name (complex, complicated, clear, chaotic or confusion)"
                        ),
                    ));
                };
                d.domains[p].items.push(s.clone());
                i += 1;
            }
            other => {
                return Err(Error::parse(
                    line,
                    format!("Unexpected {}", describe(Some(other))),
                ));
            }
        }
    }
    Ok(d)
}

// ---------------------------------------------------------------------------
// Boundaries (cynefinBoundaries.ts)
// ---------------------------------------------------------------------------

/// JavaScript `ToInt32`.
fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    x.trunc().rem_euclid(4_294_967_296.0) as u32 as i32
}

/// `seededRandom`: one mulberry32 step, in `[0, 1)`.
pub fn seeded_random(seed: f64) -> f64 {
    let mut t = to_int32(seed + 1_831_565_813.0) as u32;
    t = (t ^ (t >> 15)).wrapping_mul(t | 1);
    t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
    (t ^ (t >> 14)) as f64 / 4_294_967_296.0
}

/// `hashString`: Java-style 31× string hash over UTF-16 code units.
pub fn hash_string(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| {
        h.wrapping_shl(5).wrapping_sub(h).wrapping_add(c as i32)
    })
}

/// `resolveSeed`: a finite non-zero configured seed, else the hash of the SVG id.
pub fn resolve_seed(configured: Option<f64>, id: &str) -> f64 {
    match configured {
        Some(s) if s.is_finite() && s != 0.0 => s,
        _ => hash_string(id) as f64,
    }
}

/// `generateFoldPath`: the vertical wavy boundary (7 cubic segments).
pub fn fold_path(width: f64, height: f64, seed: f64, amplitude: f64) -> Vec<PathCmd> {
    let cx = width / 2.0;
    let seg = height / 7.0;
    let pts: Vec<Point> = (0..=7)
        .map(|i| {
            let j = seeded_random(seed + i as f64 * 17.0) * amplitude * 2.0 - amplitude;
            Point::new(cx + j, i as f64 * seg)
        })
        .collect();
    let mut cmds = vec![PathCmd::MoveTo(pts[0])];
    for i in 0..7 {
        let (p0, p1) = (pts[i], pts[i + 1]);
        let mid_y = (p0.y + p1.y) / 2.0;
        let dir = if i % 2 == 0 { 1.0 } else { -1.0 };
        let off = amplitude * 1.5 * dir * seeded_random(seed + i as f64 * 31.0 + 7.0);
        cmds.push(PathCmd::CubicTo(
            Point::new(p0.x + off, mid_y),
            Point::new(p1.x - off, mid_y),
            p1,
        ));
    }
    cmds
}

/// `generateHorizontalBoundary`: the horizontal wavy boundary.
pub fn horizontal_path(width: f64, height: f64, seed: f64, amplitude: f64) -> Vec<PathCmd> {
    let cy = height / 2.0;
    let seg = width / 7.0;
    let pts: Vec<Point> = (0..=7)
        .map(|i| {
            let j = seeded_random(seed + i as f64 * 23.0) * amplitude * 2.0 - amplitude;
            Point::new(i as f64 * seg, cy + j)
        })
        .collect();
    let mut cmds = vec![PathCmd::MoveTo(pts[0])];
    for i in 0..7 {
        let (p0, p1) = (pts[i], pts[i + 1]);
        let mid_x = (p0.x + p1.x) / 2.0;
        let dir = if i % 2 == 0 { 1.0 } else { -1.0 };
        let off = amplitude * 1.5 * dir * seeded_random(seed + i as f64 * 37.0 + 11.0);
        cmds.push(PathCmd::CubicTo(
            Point::new(mid_x, p0.y + off),
            Point::new(mid_x, p1.y - off),
            p1,
        ));
    }
    cmds
}

/// `generateCliffPath`: the steep S-curve from the centre down to the bottom edge.
pub fn cliff_path(width: f64, height: f64) -> Vec<PathCmd> {
    let cx = width / 2.0;
    let top = height * 0.5;
    let bottom = height;
    let a = width * 0.03;
    let h = bottom - top;
    vec![
        PathCmd::MoveTo(Point::new(cx, top)),
        PathCmd::CubicTo(
            Point::new(cx + a, top + h * 0.2),
            Point::new(cx - a * 1.5, top + h * 0.55),
            Point::new(cx + a * 0.5, top + h * 0.75),
        ),
        PathCmd::CubicTo(
            Point::new(cx - a, top + h * 0.85),
            Point::new(cx + a * 0.3, top + h * 0.95),
            Point::new(cx, bottom),
        ),
    ]
}

/// `generateConfusionPath`: an ellipse starting at its left end, drawn clockwise.
pub fn confusion_path(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<PathCmd> {
    let c = Point::new(cx, cy);
    let mut cmds = vec![PathCmd::MoveTo(Point::new(cx - rx, cy))];
    cmds.extend(geom::elliptic_arc_cubics(
        c,
        rx,
        ry,
        std::f64::consts::PI,
        3.0 * std::f64::consts::PI,
    ));
    cmds.push(PathCmd::Close);
    cmds
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A domain's region (`DomainLayout`): centre and box, in unpadded coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DomainLayout {
    pub center: Point,
    pub rect: Rect,
}

/// `getDomainLayouts`.
pub fn domain_layout(d: Domain, width: f64, height: f64) -> DomainLayout {
    let (hw, hh) = (width / 2.0, height / 2.0);
    let (cx, cy, rect) = match d {
        Domain::Complex => (hw / 2.0, hh / 2.0, Rect::new(0.0, 0.0, hw, hh)),
        Domain::Complicated => (hw + hw / 2.0, hh / 2.0, Rect::new(hw, 0.0, hw, hh)),
        Domain::Chaotic => (hw / 2.0, hh + hh / 2.0, Rect::new(0.0, hh, hw, hh)),
        Domain::Clear => (hw + hw / 2.0, hh + hh / 2.0, Rect::new(hw, hh, hw, hh)),
        Domain::Confusion => (hw, hh, Rect::new(hw * 0.7, hh * 0.7, hw * 0.6, hh * 0.6)),
    };
    DomainLayout {
        center: Point::new(cx, cy),
        rect,
    }
}

/// Items shown inside the confusion ellipse before the `+N more` badge.
pub const MAX_CONFUSION_ITEMS: usize = 3;
const ITEM_HEIGHT: f64 = 26.0;
const ITEM_PADDING_X: f64 = 10.0;

/// One item badge, in unpadded coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub domain: Domain,
    pub label: String,
    pub rect: Rect,
    /// The `+N more` overflow badge.
    pub overflow: bool,
}

/// Item badges: centred on the domain's x, stacked below its label block.
pub fn badges(d: &Cynefin, width: f64, height: f64, show_desc: bool, item_font: f64) -> Vec<Badge> {
    let mut out = Vec::new();
    for dom in Domain::ALL {
        let Some(block) = d.domain(dom).filter(|b| !b.items.is_empty()) else {
            continue;
        };
        let l = domain_layout(dom, width, height);
        let confusion = dom == Domain::Confusion;
        let mut items: Vec<&String> = block.items.iter().collect();
        let mut overflow = 0;
        if confusion && items.len() > MAX_CONFUSION_ITEMS {
            overflow = items.len() - MAX_CONFUSION_ITEMS;
            items.truncate(MAX_CONFUSION_ITEMS);
        }
        let start_y = l.center.y
            + match (confusion, show_desc) {
                (true, true) => 22.0,
                (true, false) => 14.0,
                (false, true) => 25.0,
                (false, false) => 15.0,
            };
        let badge = |label: String, k: usize, overflow: bool| {
            let w = line_width(&label, item_font, false) + ITEM_PADDING_X * 2.0;
            Badge {
                domain: dom,
                rect: Rect::new(
                    l.center.x - w / 2.0,
                    start_y + k as f64 * (ITEM_HEIGHT + 4.0),
                    w,
                    ITEM_HEIGHT,
                ),
                label,
                overflow,
            }
        };
        for (k, it) in items.iter().enumerate() {
            out.push(badge((*it).clone(), k, false));
        }
        if overflow > 0 {
            out.push(badge(format!("+{overflow} more"), items.len(), true));
        }
    }
    out
}

/// A transition arrow: quadratic curve from `from` to `to` bowed by 15% of its length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowGeom {
    pub from: Point,
    pub control: Point,
    pub to: Point,
}

pub fn arrow_geom(t: &Transition, width: f64, height: f64) -> ArrowGeom {
    let a = domain_layout(t.from, width, height).center;
    let b = domain_layout(t.to, width, height).center;
    let m = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let off = len * 0.15;
    let (nx, ny) = if len > 0.0 {
        (-dy / len, dx / len)
    } else {
        (0.0, 0.0)
    };
    ArrowGeom {
        from: a,
        control: Point::new(m.x + nx * off, m.y + ny * off),
        to: b,
    }
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// `themeVariables.cynefin`.
#[derive(Debug, Clone, PartialEq)]
pub struct CynefinTheme {
    pub domain_font_size: f64,
    pub item_font_size: f64,
    pub boundary_color: Color,
    pub boundary_width: f64,
    pub cliff_color: Color,
    pub cliff_width: f64,
    pub arrow_color: Color,
    pub arrow_width: f64,
    pub complex_bg: Color,
    pub complicated_bg: Color,
    pub chaotic_bg: Color,
    pub clear_bg: Color,
    pub confusion_bg: Color,
    pub text_color: Color,
    pub label_color: Color,
}

impl CynefinTheme {
    pub fn bg(&self, d: Domain) -> &Color {
        match d {
            Domain::Complex => &self.complex_bg,
            Domain::Complicated => &self.complicated_bg,
            Domain::Chaotic => &self.chaotic_bg,
            Domain::Clear => &self.clear_bg,
            Domain::Confusion => &self.confusion_bg,
        }
    }
}

fn var<'a>(cfg: &'a MermaidConfig, key: &str) -> Option<&'a str> {
    cfg.theme_variables
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// The cynefin block of the active theme. The renderer merges the current theme's
/// variables over the `default` theme's, so the neo/redux themes (which define no
/// cynefin block) draw with the default theme's colours.
pub fn theme(cfg: &MermaidConfig) -> CynefinTheme {
    let c = |s: &str| Color::new(s);
    // (line, text, label, cliff, [complex, complicated, chaotic, clear, confusion])
    let (line, text, label, cliff, bgs) = match cfg.theme {
        ThemeName::Base => (
            "#0b0b0b",
            "#333",
            "#333",
            "#8B0000",
            ["#E8F5E9", "#E3F2FD", "#FBE9E7", "#FFF8E1", "#F3E5F5"],
        ),
        ThemeName::Dark => (
            "lightgrey",
            "#ccc",
            "#e0dfdf",
            "#FF6B6B",
            ["#1B5E20", "#0D47A1", "#BF360C", "#F57F17", "#4A148C"],
        ),
        ThemeName::Forest => (
            "#000000",
            "#000000",
            "#321b67",
            "#8B4513",
            ["#C8E6C9", "#DCEDC8", "#FFE0B2", "#FFF9C4", "#D7CCC8"],
        ),
        ThemeName::Neutral => (
            "#666",
            "#000000",
            "#111111",
            "#8B0000",
            ["#E8F5E9", "#E3F2FD", "#FBE9E7", "#FFF8E1", "#F3E5F5"],
        ),
        _ => (
            "#333333",
            "#333",
            "#131300",
            "#8B0000",
            ["#E8F5E9", "#E3F2FD", "#FBE9E7", "#FFF8E1", "#F3E5F5"],
        ),
    };
    let classic = matches!(
        cfg.theme,
        ThemeName::Base
            | ThemeName::Dark
            | ThemeName::Forest
            | ThemeName::Neutral
            | ThemeName::Default
            | ThemeName::Null
    );
    // Variables only re-derive the block for themes that compute one themselves.
    let derived = |k: &str, dflt: &str| -> Color {
        match var(cfg, k) {
            Some(v) if classic => c(v),
            _ => c(dflt),
        }
    };
    let mut t = CynefinTheme {
        domain_font_size: 16.0,
        item_font_size: 12.0,
        boundary_color: derived("lineColor", line),
        boundary_width: 2.0,
        cliff_color: c(cliff),
        cliff_width: 4.0,
        arrow_color: derived("lineColor", line),
        arrow_width: 2.0,
        complex_bg: c(bgs[0]),
        complicated_bg: c(bgs[1]),
        chaotic_bg: c(bgs[2]),
        clear_bg: c(bgs[3]),
        confusion_bg: c(bgs[4]),
        text_color: derived("textColor", text),
        label_color: derived("primaryTextColor", label),
    };
    if let Some(Json::Object(o)) = cfg.section("themeVariables", "cynefin") {
        for (k, v) in o {
            let num = v
                .as_f64()
                .or_else(|| {
                    v.as_str()
                        .and_then(|s| s.trim_end_matches("px").parse().ok())
                })
                .filter(|n: &f64| *n != 0.0);
            let col = v
                .to_scalar_string()
                .filter(|s| !s.is_empty())
                .map(Color::new);
            match k.as_str() {
                "domainFontSize" => t.domain_font_size = num.unwrap_or(t.domain_font_size),
                "itemFontSize" => t.item_font_size = num.unwrap_or(t.item_font_size),
                "boundaryWidth" => t.boundary_width = num.unwrap_or(t.boundary_width),
                "cliffWidth" => t.cliff_width = num.unwrap_or(t.cliff_width),
                "arrowWidth" => t.arrow_width = num.unwrap_or(t.arrow_width),
                "boundaryColor" => t.boundary_color = col.unwrap_or(t.boundary_color),
                "cliffColor" => t.cliff_color = col.unwrap_or(t.cliff_color),
                "arrowColor" => t.arrow_color = col.unwrap_or(t.arrow_color),
                "complexBg" => t.complex_bg = col.unwrap_or(t.complex_bg),
                "complicatedBg" => t.complicated_bg = col.unwrap_or(t.complicated_bg),
                "chaoticBg" => t.chaotic_bg = col.unwrap_or(t.chaotic_bg),
                "clearBg" => t.clear_bg = col.unwrap_or(t.clear_bg),
                "confusionBg" => t.confusion_bg = col.unwrap_or(t.confusion_bg),
                "textColor" => t.text_color = col.unwrap_or(t.text_color),
                "labelColor" => t.label_color = col.unwrap_or(t.label_color),
                _ => {}
            }
        }
    }
    t
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The SVG id mermaid-cli renders with; the boundary seed is derived from it.
const SVG_ID: &str = "my-svg";

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

fn middle_text(p: Point, s: &str, font: f64, fill: &Color) -> Text {
    Text::centered(p, &[s.to_string()], font, font, fill)
}

/// Quadratic Bézier as an exact cubic.
fn quad(p0: Point, c: Point, p1: Point) -> PathCmd {
    PathCmd::CubicTo(
        p0.plus(c.minus(p0).scale(2.0 / 3.0)),
        p1.plus(c.minus(p1).scale(2.0 / 3.0)),
        p1,
    )
}

/// The `cynefin-arrow` marker (`M0,0 L10,5 L0,10 z`, refX 9, 6×6 in stroke-width units)
/// placed at `tip`, pointing along `dir`.
fn arrow_head(tip: Point, dir: Point, stroke_width: f64) -> Vec<PathCmd> {
    let s = 6.0 * stroke_width / 10.0;
    let d = dir.norm();
    let n = Point::new(-d.y, d.x);
    let at = |u: f64, v: f64| {
        tip.plus(d.scale((u - 9.0) * s))
            .plus(n.scale((v - 5.0) * s))
    };
    geom::polygon(&[at(0.0, 0.0), at(10.0, 5.0), at(0.0, 10.0)])
}

/// Parse, lay out and draw a `cynefin` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    Ok(draw(&d, cfg, pal))
}

/// Draw a parsed diagram.
pub fn draw(d: &Cynefin, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let s = "cynefin";
    let width = cfg.section_f64(s, "width", 800.0).max(1.0);
    let height = cfg.section_f64(s, "height", 600.0).max(1.0);
    let padding = cfg.section_f64(s, "padding", 40.0).max(0.0);
    let show_desc = cfg.section_bool(s, "showDomainDescriptions", true);
    let amplitude = cfg.section_f64(s, "boundaryAmplitude", 8.0);
    let seed = resolve_seed(cfg.section(s, "seed").and_then(Json::as_f64), SVG_ID);
    let t = theme(cfg);
    let sub_font = t.item_font_size - 1.0;
    let mut els = Vec::new();

    // 1. Domain backgrounds.
    let bgs = Domain::QUADRANTS
        .iter()
        .map(|&dom| {
            Element::classed_shape(
                crate::scene::Geometry::Rect {
                    rect: domain_layout(dom, width, height).rect,
                    rx: 0.0,
                },
                Paint::fill(&with_alpha(t.bg(dom), 0.4)),
                "cynefinDomain",
            )
        })
        .collect();
    els.push(Element::group("cynefin-backgrounds", bgs));

    // 2–3. Wavy boundaries and the cliff.
    let boundary = Paint::stroke(&t.boundary_color, t.boundary_width).dashed(vec![6.0, 3.0]);
    els.push(Element::group(
        "cynefin-boundaries",
        vec![
            Element::path(fold_path(width, height, seed, amplitude), boundary.clone()),
            Element::path(
                horizontal_path(width, height, seed + 100.0, amplitude),
                boundary,
            ),
            Element::path(
                cliff_path(width, height),
                Paint::stroke(&t.cliff_color, t.cliff_width),
            ),
        ],
    ));

    // 4. Confusion ellipse.
    let ell = confusion_path(width / 2.0, height / 2.0, width * 0.15, height * 0.15);
    els.push(Element::group(
        "cynefinConfusion",
        vec![
            Element::path(ell.clone(), Paint::fill(&with_alpha(&t.confusion_bg, 0.5))),
            Element::path(
                ell,
                Paint::stroke(&t.boundary_color, 1.5).dashed(vec![4.0, 2.0]),
            ),
        ],
    ));

    // 5. Domain labels.
    let mut labels = Vec::new();
    for dom in Domain::QUADRANTS {
        let c = domain_layout(dom, width, height).center;
        let y = if show_desc { c.y - 30.0 } else { c.y };
        labels.push(Element::classed_text(
            middle_text(
                Point::new(c.x, y),
                dom.title(),
                t.domain_font_size,
                &t.label_color,
            )
            .bold(),
            "cynefinDomainLabel",
        ));
    }
    let cy = if show_desc {
        height / 2.0 - 10.0
    } else {
        height / 2.0
    };
    labels.push(Element::classed_text(
        middle_text(
            Point::new(width / 2.0, cy),
            "Confusion",
            t.domain_font_size,
            &t.label_color,
        )
        .bold(),
        "cynefinDomainLabel",
    ));
    els.push(Element::group("cynefin-labels", labels));

    // 6. Decision model / practice subtitles.
    if show_desc {
        let mut subs = Vec::new();
        for dom in Domain::QUADRANTS {
            let c = domain_layout(dom, width, height).center;
            let (model, practice) = dom.meta();
            for (text, y) in [(model, c.y - 10.0), (practice, c.y + 5.0)] {
                subs.push(Element::classed_text(
                    middle_text(Point::new(c.x, y), text, sub_font, &t.text_color),
                    "cynefinSubtitle",
                ));
            }
        }
        subs.push(Element::classed_text(
            middle_text(
                Point::new(width / 2.0, height / 2.0 + 8.0),
                Domain::Confusion.meta().1,
                sub_font,
                &t.text_color,
            ),
            "cynefinSubtitle",
        ));
        els.push(Element::group("cynefin-subtitles", subs));
    }

    // 7. Item badges.
    let mut items = Vec::new();
    for b in badges(d, width, height, show_desc, t.item_font_size) {
        let bg = t.bg(b.domain);
        let geom = crate::scene::Geometry::Rect {
            rect: b.rect,
            rx: 4.0,
        };
        let (alpha, class) = if b.overflow {
            (0.6, "cynefinItemOverflow")
        } else {
            (0.95, "cynefinItem")
        };
        let mut stroke = Paint::stroke(&t.boundary_color, 1.0);
        if b.overflow {
            stroke = stroke.dashed(vec![3.0, 2.0]);
        }
        items.push(Element::group(
            class,
            vec![
                Element::shape(geom.clone(), Paint::fill(&with_alpha(bg, alpha))),
                Element::shape(geom, stroke),
                Element::classed_text(
                    middle_text(b.rect.center(), &b.label, t.item_font_size, &t.text_color),
                    "cynefinItemText",
                ),
            ],
        ));
    }
    if !items.is_empty() {
        els.push(Element::group("cynefin-items", items));
    }

    // 8. Transition arrows.
    if !d.transitions.is_empty() {
        let mut arrows = Vec::new();
        for tr in &d.transitions {
            let a = arrow_geom(tr, width, height);
            arrows.push(Element::path(
                vec![PathCmd::MoveTo(a.from), quad(a.from, a.control, a.to)],
                Paint::stroke(&t.arrow_color, t.arrow_width),
            ));
            arrows.push(Element::path(
                arrow_head(a.to, a.to.minus(a.control), t.arrow_width),
                Paint::fill(&t.arrow_color),
            ));
            if let Some(l) = &tr.label {
                arrows.push(Element::classed_text(
                    Text {
                        x: a.control.x,
                        y: a.control.y - 6.0,
                        lines: vec![l.clone()],
                        line_height: sub_font,
                        font_size: sub_font,
                        bold: false,
                        anchor: Anchor::Middle,
                        fill: t.text_color.clone(),
                    },
                    "cynefinArrowLabel",
                ));
            }
        }
        els.push(Element::group("cynefin-arrows", arrows));
    }

    // Title, centred in the top padding band.
    if let Some(title) = d.title.as_ref().filter(|t| !t.is_empty()) {
        els.push(Element::classed_text(
            middle_text(
                Point::new(width / 2.0, -padding / 2.0),
                title,
                t.domain_font_size + 2.0,
                &t.label_color,
            )
            .bold(),
            "cynefinTitle",
        ));
    }

    let els = els
        .into_iter()
        .map(|e| translate(e, padding, padding))
        .collect();
    Scene {
        kind: DiagramKind::Cynefin,
        view_box: Rect::new(0.0, 0.0, width + 2.0 * padding, height + 2.0 * padding),
        elements: vec![Element::group("cynefin", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: ".cynefinSubtitle{font-style:italic;}".to_string(),
    }
}

fn translate(e: Element, dx: f64, dy: f64) -> Element {
    match e {
        Element::Group {
            id,
            class,
            children,
        } => Element::Group {
            id,
            class,
            children: children.into_iter().map(|c| translate(c, dx, dy)).collect(),
        },
        Element::Shape { geom, paint, class } => Element::Shape {
            geom: geom.translated(dx, dy),
            paint,
            class,
        },
        Element::Text { mut text, class } => {
            text.x += dx;
            text.y += dy;
            Element::Text { text, class }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_domains_and_items() {
        let d = parse(
            "cynefin-beta\n  complex\n    \"Emergent practice\"\n    \"Probe-sense-respond\"\n  clear\n    \"Best practice\"\n",
            None,
        )
        .unwrap();
        assert_eq!(d.domains.len(), 2);
        assert_eq!(
            d.domain(Domain::Complex).unwrap().items,
            vec!["Emergent practice", "Probe-sense-respond"]
        );
        assert_eq!(
            d.domain(Domain::Clear).unwrap().items,
            vec!["Best practice"]
        );
    }

    #[test]
    fn parses_all_domains_empty_blocks_and_header_colon() {
        let d = parse(
            "cynefin-beta:\n complex\n \"A\"\n complicated\n \"B\"\n clear\n \"C\"\n chaotic\n \"D\"\n confusion\n \"E\"\n",
            None,
        )
        .unwrap();
        assert_eq!(d.domains.len(), 5);
        let d = parse("cynefin-beta\n  complex\n", None).unwrap();
        assert!(d.domain(Domain::Complex).unwrap().items.is_empty());
        // Several items on one line are allowed; single quotes too.
        let d = parse("cynefin-beta\ncomplex \"a\" 'b'\n", None).unwrap();
        assert_eq!(d.domain(Domain::Complex).unwrap().items, vec!["a", "b"]);
    }

    #[test]
    fn parses_transitions_and_drops_self_loops() {
        let d = parse(
            "cynefin-beta\n complex\n \"A\"\n complex --> complicated : \"Analyzed\"\n complicated --> clear\n chaotic --> complex : \"Stabilized\"\n clear --> clear\n",
            None,
        )
        .unwrap();
        assert_eq!(d.transitions.len(), 3);
        assert_eq!(d.transitions[0].from, Domain::Complex);
        assert_eq!(d.transitions[0].to, Domain::Complicated);
        assert_eq!(d.transitions[0].label.as_deref(), Some("Analyzed"));
        assert_eq!(d.transitions[1].label, None);
        assert_eq!(d.transitions[2].from, Domain::Chaotic);
        // Transitions don't swallow following items into a domain.
        assert_eq!(d.domain(Domain::Complex).unwrap().items, vec!["A"]);
    }

    #[test]
    fn parses_title_and_comments() {
        let d = parse(
            "cynefin-beta\n  title Incident Response\n  %% comment\n  complex\n    \"Item\" %% trailing\n",
            None,
        )
        .unwrap();
        assert_eq!(d.title.as_deref(), Some("Incident Response"));
        assert_eq!(d.domain(Domain::Complex).unwrap().items, vec!["Item"]);
    }

    #[test]
    fn parse_errors() {
        assert!(parse("cynefin-beta\n \"orphan\"\n", None).is_err());
        assert!(parse("cynefin-beta\n complexity\n", None).is_err());
        assert!(parse("cynefin-beta\n complex --> \n", None).is_err());
        assert!(parse("cynefin-beta\n complex --> clear : label\n", None).is_err());
        assert!(parse("complex\n", None).is_err());
    }

    #[test]
    fn prng_matches_upstream() {
        // mulberry32 one-step values computed with the JS implementation.
        assert_eq!(seeded_random(42.0), seeded_random(42.0));
        for s in [0.0, 1.0, 100.0, 999_999.0, -50.0] {
            let v = seeded_random(s);
            assert!((0.0..1.0).contains(&v));
        }
        assert_eq!(hash_string("cynefin"), hash_string("cynefin"));
        assert_ne!(hash_string("complex"), hash_string("chaotic"));
        // "abc" → 96354 in Java/JS 31-hash.
        assert_eq!(hash_string("abc"), 96354);
        assert_eq!(resolve_seed(Some(42.0), "x"), 42.0);
        assert_eq!(
            resolve_seed(Some(0.0), "my-svg"),
            hash_string("my-svg") as f64
        );
        assert_eq!(resolve_seed(Some(f64::NAN), "a"), hash_string("a") as f64);
    }

    #[test]
    fn seeded_random_reference_values() {
        // Values from mermaid's seededRandom (node): seededRandom(1) and seededRandom(42).
        assert!((seeded_random(1.0) - 0.627_073_940_588_161_3).abs() < 1e-15);
        assert!((seeded_random(42.0) - 0.601_103_751_920_163_6).abs() < 1e-15);
        assert!((seeded_random(-50.0) - 0.231_296_549_318_358_3).abs() < 1e-15);
        assert_eq!(hash_string("my-svg"), -1_061_188_925);
    }

    #[test]
    fn boundaries_stay_within_amplitude() {
        let p = fold_path(800.0, 600.0, 42.0, 8.0);
        assert_eq!(p.len(), 8);
        let PathCmd::MoveTo(start) = p[0] else {
            panic!()
        };
        assert!((start.x - 400.0).abs() <= 8.0 && start.y == 0.0);
        let PathCmd::CubicTo(_, _, end) = p[7] else {
            panic!()
        };
        assert!((end.y - 600.0).abs() < 1e-9 && (end.x - 400.0).abs() <= 8.0);
        let h = horizontal_path(800.0, 600.0, 142.0, 0.0);
        for c in &h {
            if let PathCmd::CubicTo(a, b, e) = c {
                assert_eq!((a.y, b.y, e.y), (300.0, 300.0, 300.0));
            }
        }
        let cl = cliff_path(800.0, 600.0);
        assert_eq!(cl[0], PathCmd::MoveTo(Point::new(400.0, 300.0)));
    }

    #[test]
    fn layout_quadrants_and_badges() {
        let c = domain_layout(Domain::Clear, 800.0, 600.0);
        assert_eq!(c.center, Point::new(600.0, 450.0));
        assert_eq!(c.rect, Rect::new(400.0, 300.0, 400.0, 300.0));
        let d = parse(
            "cynefin-beta\nconfusion\n\"A\"\n\"B\"\n\"C\"\n\"D\"\n\"E\"\ncomplex\n\"x\"\n\"y\"\n",
            None,
        )
        .unwrap();
        let b = badges(&d, 800.0, 600.0, true, 12.0);
        let conf: Vec<&Badge> = b.iter().filter(|b| b.domain == Domain::Confusion).collect();
        assert_eq!(conf.len(), 4);
        assert!(conf[3].overflow);
        assert_eq!(conf[3].label, "+2 more");
        assert_eq!(conf[0].rect.y, 300.0 + 22.0);
        let cx: Vec<&Badge> = b.iter().filter(|b| b.domain == Domain::Complex).collect();
        assert_eq!(cx[0].rect.y, 150.0 + 25.0);
        assert_eq!(cx[1].rect.y, 150.0 + 25.0 + 30.0);
        assert!((cx[0].rect.center().x - 200.0).abs() < 1e-9);
    }

    #[test]
    fn arrow_bows_left_of_travel() {
        let a = arrow_geom(
            &Transition {
                from: Domain::Complex,
                to: Domain::Complicated,
                label: None,
            },
            800.0,
            600.0,
        );
        // Complex (200,150) → Complicated (600,150): normal (0, 1) · 60.
        assert_eq!(a.control, Point::new(400.0, 210.0));
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default().scoped("cynefin");
        let pal = cfg.palette();
        let scene = render(
            "cynefin-beta\n title T\n complex\n \"a\"\n complex --> clear : \"go\"\n",
            None,
            &cfg,
            &pal,
        )
        .unwrap();
        assert_eq!(scene.view_box, Rect::new(0.0, 0.0, 880.0, 680.0));
        assert!(format!("{scene:?}").contains("Probe \u{2192} Sense \u{2192} Respond"));
    }

    #[test]
    fn theme_overrides() {
        let cfg = MermaidConfig::default()
            .with_json(
                &crate::json::parse(
                    r##"{"themeVariables":{"cynefin":{"complexBg":"#ff0000","cliffWidth":6}}}"##,
                )
                .unwrap(),
            )
            .unwrap()
            .scoped("cynefin");
        let t = theme(&cfg);
        assert_eq!(t.complex_bg.css(), "#ff0000");
        assert_eq!(t.cliff_width, 6.0);
        assert_eq!(t.label_color.css(), "#131300");
    }
}
