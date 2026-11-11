//! `usecase-beta` diagrams (mermaid 12.3 `diagrams/usecase`).
//!
//! The parser mirrors upstream's chevrotain grammar (`usecase.tokens.ts`,
//! `usecase.parser.ts`) and its semantic pass (`usecase.visitor.ts`,
//! `usecaseModelBuilder.ts`): a hand-written lexer with the same token priorities,
//! a recursive-descent parser over one statement per physical line, then a
//! finalisation step that resolves the diagram-wide ID namespace, infers
//! undeclared relationship endpoints as ellipse use cases, merges repeated
//! declarations and validates relationship kinds, metadata, notes and styles.
//!
//! Rendering follows the unified renderer contract `usecaseDb.getData()` produces:
//! actors (stick, hollow, awesome and icon glyphs with a caption), ellipse / business /
//! rectangular use cases, notes, JSON tables and system boundaries as clusters, laid
//! out by the shared layered layout (upstream defaults to ELK, a layered algorithm too)
//! and drawn with the shapes and the role / palette colours of `usecase/styles.ts`.

use std::collections::HashMap;

use super::graph::{Head, StyleOverrides};
use super::markers::{self, End, MarkerColors};
use super::shapes::{Boundary as Clip, ShapeMetrics};
use crate::config::{Curve, Look, MermaidConfig};
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::json::{self, Json};
use crate::layout::{self, Direction, LayoutCluster, LayoutEdge, LayoutInput, LayoutNode};
use crate::scene::{Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{decode_entities, line_width, normalize_label};
use crate::theme::{ColorSlot, DropShadow, Palette, SlotColors, ThemeName};

// ───────────────────────────── model ─────────────────────────────

/// How a label's text is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    /// Plain text: shown literally (entity codes decoded).
    Text,
    /// A "`…`" Markdown string (may span lines).
    Markdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    pub kind: LabelKind,
}

impl Label {
    fn text(s: &str) -> Label {
        Label {
            text: s.to_string(),
            kind: LabelKind::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorType {
    Normal,
    Hollow,
    Awesome,
    Icon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseCaseShape {
    Ellipse,
    Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryType {
    Rect,
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipType {
    Association,
    Include,
    Extend,
    Generalization,
}

/// Association operators (`ARROW_TYPE` upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowType {
    /// `-->`
    SolidArrow,
    /// `<--`
    BackArrow,
    /// `--`
    LineSolid,
    /// `--o`
    CircleArrow,
    /// `--x`
    CrossArrow,
    /// `o--`
    CircleArrowReversed,
    /// `x--`
    CrossArrowReversed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Animation {
    Fast,
    Slow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Actor {
    pub id: String,
    pub label: Label,
    pub kind: ActorType,
    pub icon: Option<String>,
    pub business: bool,
    pub stereotype: Option<String>,
    pub parent: Option<String>,
    pub classes: Vec<String>,
    pub styles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseCase {
    pub id: String,
    pub label: Label,
    pub shape: UseCaseShape,
    pub business: bool,
    pub stereotype: Option<String>,
    pub parent: Option<String>,
    pub classes: Vec<String>,
    pub styles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemBoundary {
    pub id: String,
    pub label: Label,
    pub kind: BoundaryType,
    pub members: Vec<String>,
    pub classes: Vec<String>,
    pub styles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relationship {
    pub id: String,
    pub explicit_id: bool,
    pub source: String,
    pub target: String,
    pub kind: RelationshipType,
    pub arrow: ArrowType,
    pub label: Option<Label>,
    pub minlen: usize,
    pub classes: Vec<String>,
    pub styles: Vec<String>,
    pub animate: bool,
    pub animation: Option<Animation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub target: String,
    pub label: Label,
}

/// One row of a flattened JSON table.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRow {
    /// Visible key cell (blank for scalar-array items after the first).
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonTable {
    pub id: String,
    pub rows: Vec<JsonRow>,
    pub classes: Vec<String>,
    pub styles: Vec<String>,
}

/// A fully resolved use case diagram (the committed `UsecaseFields` upstream).
#[derive(Debug, Clone, PartialEq)]
pub struct UsecaseDiagram {
    pub direction: Direction,
    pub actors: Vec<Actor>,
    pub use_cases: Vec<UseCase>,
    pub boundaries: Vec<SystemBoundary>,
    pub relationships: Vec<Relationship>,
    pub notes: Vec<Note>,
    pub json_tables: Vec<JsonTable>,
    /// `classDef` name → style declarations (later definitions replace earlier ones).
    pub class_defs: Vec<(String, Vec<String>)>,
    pub title: Option<String>,
}

// ───────────────────────────── lexer ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    Usecase,
    Actor,
    SystemBoundary,
    End,
    Direction,
    Td,
    Tb,
    Bt,
    Lr,
    Rl,
    Note,
    For,
    Json,
    ClassDef,
    Class,
    Style,
    Include,
    Extend,
    True,
    False,
}

/// Keyword tokens in upstream's priority order; the flag marks `/…/i` patterns.
const KEYWORDS: [(&str, Kw, bool); 20] = [
    ("usecase-beta", Kw::Usecase, false),
    ("actor", Kw::Actor, false),
    ("systemBoundary", Kw::SystemBoundary, false),
    ("end", Kw::End, false),
    ("direction", Kw::Direction, false),
    ("TD", Kw::Td, false),
    ("TB", Kw::Tb, false),
    ("BT", Kw::Bt, false),
    ("LR", Kw::Lr, false),
    ("RL", Kw::Rl, false),
    ("note", Kw::Note, false),
    ("for", Kw::For, false),
    ("json", Kw::Json, false),
    ("classDef", Kw::ClassDef, false),
    ("class", Kw::Class, false),
    ("style", Kw::Style, false),
    ("include", Kw::Include, true),
    ("extend", Kw::Extend, true),
    ("true", Kw::True, false),
    ("false", Kw::False, false),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum T {
    Newline,
    Comment,
    AccTitle,
    AccDescr,
    Markdown,
    Plain,
    JsonStart,
    JsonObject,
    Kw(Kw),
    Ident,
    Number,
    CssIdent,
    Generalization,
    Dependency,
    StereoStart,
    StereoText,
    StereoEnd,
    ClassSep,
    FwdSolid,
    BackSolid,
    FwdCircle,
    BackCircle,
    FwdCross,
    BackCross,
    Markerless,
    MetaStart,
    At,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Comma,
    Colon,
    Hash,
    EscComma,
    Dash,
    Dot,
    Percent,
    CssPunct,
    LabelPunct,
    LabelSymbol,
    Eof,
}

impl T {
    /// Members of upstream's `LABEL_TEXT` category (unquoted label material).
    fn is_label_text(self) -> bool {
        matches!(
            self,
            T::Kw(_)
                | T::Ident
                | T::Number
                | T::At
                | T::Comma
                | T::Colon
                | T::Hash
                | T::CssIdent
                | T::EscComma
                | T::Dash
                | T::Dot
                | T::Percent
                | T::CssPunct
                | T::LabelPunct
                | T::LabelSymbol
        )
    }
    fn is_label(self) -> bool {
        self.is_label_text() || matches!(self, T::Plain | T::Markdown)
    }
    fn is_word(self) -> bool {
        matches!(self, T::Kw(_) | T::Ident)
    }
    fn is_arrow(self) -> bool {
        matches!(
            self,
            T::Dependency
                | T::Generalization
                | T::FwdSolid
                | T::BackSolid
                | T::Markerless
                | T::FwdCircle
                | T::BackCircle
                | T::FwdCross
                | T::BackCross
        )
    }
    fn name(self) -> String {
        match self {
            T::Kw(k) => format!("{k:?}").to_uppercase(),
            T::Newline => "NEWLINE".into(),
            T::Eof => "EOF".into(),
            T::Ident => "IDENTIFIER".into(),
            other => format!("{other:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Tok<'a> {
    t: T,
    text: &'a str,
    start: usize,
    end: usize,
}

/// Source position (byte offset, 1-based line and column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Loc {
    offset: usize,
    line: usize,
    col: usize,
}

impl Loc {
    fn at(&self) -> String {
        format!("line {}, column {}", self.line, self.col)
    }
}

struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(src: &str) -> Self {
        let b = src.as_bytes();
        let mut starts = vec![0];
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\r' if b.get(i + 1) == Some(&b'\n') => {
                    starts.push(i + 2);
                    i += 1;
                }
                b'\n' | b'\r' => starts.push(i + 1),
                _ => {}
            }
            i += 1;
        }
        Lines { starts }
    }
    fn loc(&self, src: &str, offset: usize) -> Loc {
        let idx = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.starts[idx];
        let col = src.get(start..offset).map_or(1, |s| s.chars().count() + 1);
        Loc {
            offset,
            line: idx + 1,
            col,
        }
    }
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn ident_len(b: &[u8], i: usize) -> usize {
    b[i..].iter().take_while(|c| is_word_byte(**c)).count()
}

/// `/(?:\d+\.\d+|\d+|\.\d+)(?:[A-Za-z]+)?/`
fn number_len(b: &[u8], i: usize) -> usize {
    let digits = |j: usize| b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut j = i;
    let d = digits(j);
    if d > 0 {
        j += d;
        if b.get(j) == Some(&b'.') && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
            j += 1 + digits(j + 1);
        }
    } else if b.get(j) == Some(&b'.') && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
        j += 1 + digits(j + 1);
    } else {
        return 0;
    }
    j += b[j..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    j - i
}

/// `/--[A-Z_a-z][\w-]*|[A-Z_a-z]\w*(?:-\w+)+/`
fn css_ident_len(b: &[u8], i: usize) -> usize {
    let alpha = |c: Option<&u8>| c.is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_');
    if b[i..].starts_with(b"--") && alpha(b.get(i + 2)) {
        let rest = b[i + 3..]
            .iter()
            .take_while(|c| is_word_byte(**c) || **c == b'-')
            .count();
        return 3 + rest;
    }
    if !alpha(b.get(i)) {
        return 0;
    }
    let mut j = i + ident_len(b, i);
    let mut groups = 0;
    while b.get(j) == Some(&b'-') && b.get(j + 1).is_some_and(|c| is_word_byte(*c)) {
        j += 1 + ident_len(b, j + 1);
        groups += 1;
    }
    if groups == 0 { 0 } else { j - i }
}

/// Only spaces / tabs between the previous line break and `i`.
fn line_prefix_blank(b: &[u8], i: usize) -> bool {
    b[..i]
        .iter()
        .rev()
        .take_while(|c| **c != b'\n' && **c != b'\r')
        .all(|c| *c == b' ' || *c == b'\t')
}

fn eol(b: &[u8], i: usize) -> usize {
    i + b[i..]
        .iter()
        .take_while(|c| **c != b'\n' && **c != b'\r')
        .count()
}

/// `accTitle: …`, `accDescr: …` and `accDescr { … }` at the start of a line.
fn acc_token(b: &[u8], i: usize) -> Option<(T, usize)> {
    if !line_prefix_blank(b, i) {
        return None;
    }
    let after = |kw: &[u8]| -> Option<usize> {
        if !b[i..].starts_with(kw) {
            return None;
        }
        let j = i + kw.len();
        Some(
            j + b[j..]
                .iter()
                .take_while(|c| **c == b' ' || **c == b'\t')
                .count(),
        )
    };
    if let Some(j) = after(b"accDescr").filter(|&j| b.get(j) == Some(&b'{')) {
        let close = b[j..].iter().position(|c| *c == b'}')?;
        return Some((T::AccDescr, j + close + 1 - i));
    }
    for (kw, t) in [(&b"accTitle"[..], T::AccTitle), (b"accDescr", T::AccDescr)] {
        if after(kw).is_some_and(|j| b.get(j) == Some(&b':')) {
            return Some((t, eol(b, i) - i));
        }
    }
    None
}

/// Balanced `{…}` with JSON string awareness (`matchJsonObject`).
fn json_object_len(b: &[u8], i: usize) -> Option<usize> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (k, &c) in b[i..].iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                quoted = false;
            }
            continue;
        }
        match c {
            b'"' => quoted = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `/json[\t ]+\w+[\t ]*@[\t ]*(?={)/`
fn json_start_len(b: &[u8], i: usize) -> Option<usize> {
    if !b[i..].starts_with(b"json") {
        return None;
    }
    let ws = |j: usize| {
        b[j..]
            .iter()
            .take_while(|c| **c == b' ' || **c == b'\t')
            .count()
    };
    let mut j = i + 4;
    let w = ws(j);
    if w == 0 {
        return None;
    }
    j += w;
    let id = ident_len(b, j);
    if id == 0 {
        return None;
    }
    j += id;
    j += ws(j);
    if b.get(j) != Some(&b'@') {
        return None;
    }
    j += 1;
    j += ws(j);
    (b.get(j) == Some(&b'{')).then_some(j - i)
}

fn lex(src: &str) -> Result<Vec<Tok<'_>>> {
    let b = src.as_bytes();
    let lines = Lines::new(src);
    let mut out = Vec::new();
    let mut i = 0;
    let err = |i: usize, what: &str| {
        let loc = lines.loc(src, i);
        Error::parse(
            loc.line,
            format!("Error lexing usecase diagram: {what} at {}", loc.at()),
        )
    };
    while i < b.len() {
        let c = b[i];
        if c == b' ' || c == b'\t' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        macro_rules! emit {
            ($t:expr, $len:expr) => {{
                let len = $len;
                out.push(Tok {
                    t: $t,
                    text: &src[i..i + len],
                    start: i,
                    end: i + len,
                });
                i += len;
            }};
        }
        // Markdown strings ("`…`"), which may span lines.
        if rest.starts_with(b"\"`") {
            let close =
                (i + 2..b.len().saturating_sub(1)).find(|&k| b[k] == b'`' && b[k + 1] == b'"');
            match close {
                Some(k) => emit!(T::Markdown, k + 2 - i),
                None => return Err(err(i, "unclosed Markdown string")),
            }
            continue;
        }
        if rest.starts_with(b"%%") && line_prefix_blank(b, i) {
            emit!(T::Comment, eol(b, i) - i);
            continue;
        }
        if rest.starts_with(b"\r\n") {
            emit!(T::Newline, 2);
            continue;
        }
        if c == b'\n' || c == b'\r' {
            emit!(T::Newline, 1);
            continue;
        }
        if let Some((t, len)) = acc_token(b, i) {
            emit!(t, len);
            continue;
        }
        if let Some(len) = json_start_len(b, i) {
            emit!(T::JsonStart, len);
            match json_object_len(b, i) {
                Some(len) => {
                    out.push(Tok {
                        t: T::JsonObject,
                        text: &src[i..i + len],
                        start: i,
                        end: i + len,
                    });
                    i += len;
                }
                None => return Err(err(i, "unclosed JSON object")),
            }
            continue;
        }
        // Keywords, with IDENTIFIER as their longer alternative.
        let id_len = ident_len(b, i);
        let kw = KEYWORDS.iter().find(|(k, _, ci)| {
            let k = k.as_bytes();
            rest.len() >= k.len()
                && if *ci {
                    rest[..k.len()].eq_ignore_ascii_case(k)
                } else {
                    &rest[..k.len()] == k
                }
        });
        if let Some((k, kw, _)) = kw {
            if id_len > k.len() {
                emit!(T::Ident, id_len);
            } else {
                emit!(T::Kw(*kw), k.len());
            }
            continue;
        }
        let dashes = |j: usize| b[j..].iter().take_while(|c| **c == b'-').count();
        let fixed: &[(&[u8], T)] = &[(b"--|>", T::Generalization), (b"..>", T::Dependency)];
        if let Some((s, t)) = fixed.iter().find(|(s, _)| rest.starts_with(s)) {
            emit!(*t, s.len());
            continue;
        }
        if rest.starts_with(b"<<") {
            emit!(T::StereoStart, 2);
            // Stereotype mode: `>>`, or text up to `>>` on the same line.
            if b[i..].starts_with(b">>") {
                emit!(T::StereoEnd, 2);
                continue;
            }
            let line_end = eol(b, i);
            let close = src[i..line_end].find(">>").map(|k| i + k);
            match close {
                Some(k) if !src[i..k].trim().is_empty() => {
                    out.push(Tok {
                        t: T::StereoText,
                        text: &src[i..k],
                        start: i,
                        end: k,
                    });
                    out.push(Tok {
                        t: T::StereoEnd,
                        text: &src[k..k + 2],
                        start: k,
                        end: k + 2,
                    });
                    i = k + 2;
                }
                _ => return Err(err(i, "unclosed stereotype")),
            }
            continue;
        }
        if rest.starts_with(b":::") {
            emit!(T::ClassSep, 3);
            continue;
        }
        let d = dashes(i);
        if d >= 2 && b.get(i + d) == Some(&b'>') {
            emit!(T::FwdSolid, d + 1);
            continue;
        }
        if c == b'<' && dashes(i + 1) >= 2 {
            emit!(T::BackSolid, 1 + dashes(i + 1));
            continue;
        }
        let ops: &[(&[u8], T)] = &[
            (b"--o", T::FwdCircle),
            (b"o--", T::BackCircle),
            (b"--x", T::FwdCross),
            (b"x--", T::BackCross),
        ];
        if let Some((s, t)) = ops.iter().find(|(s, _)| rest.starts_with(s)) {
            emit!(*t, s.len());
            continue;
        }
        if d >= 2 {
            emit!(T::Markerless, d);
            continue;
        }
        if rest.starts_with(b"@{") {
            emit!(T::MetaStart, 2);
            continue;
        }
        let single = match c {
            b'@' => Some(T::At),
            b'{' => Some(T::LBrace),
            b'}' => Some(T::RBrace),
            b'[' => Some(T::LBracket),
            b']' => Some(T::RBracket),
            b'(' => Some(T::LParen),
            b')' => Some(T::RParen),
            b',' => Some(T::Comma),
            b':' => Some(T::Colon),
            _ => None,
        };
        if let Some(t) = single {
            emit!(t, 1);
            continue;
        }
        if c == b'#' {
            let hex = b[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if hex > 0 {
                emit!(T::Hash, 1 + hex);
                continue;
            }
        }
        if c == b'"' || c == b'\'' {
            let close = b[i + 1..]
                .iter()
                .position(|x| *x == c || *x == b'\n' || *x == b'\r');
            match close {
                Some(k) if b[i + 1 + k] == c => {
                    emit!(T::Plain, k + 2);
                    continue;
                }
                _ => return Err(err(i, "unclosed string")),
            }
        }
        let css = css_ident_len(b, i);
        if css > 0 {
            emit!(T::CssIdent, css);
            continue;
        }
        if id_len > 0 {
            let num = number_len(b, i);
            if num > id_len {
                emit!(T::Number, num);
            } else {
                emit!(T::Ident, id_len);
            }
            continue;
        }
        let num = number_len(b, i);
        if num > 0 {
            emit!(T::Number, num);
            continue;
        }
        if rest.starts_with(b"\\,") {
            emit!(T::EscComma, 2);
            continue;
        }
        let punct = match c {
            b'-' => Some(T::Dash),
            b'.' => Some(T::Dot),
            b'%' => Some(T::Percent),
            b'!' | b'#' | b'$' | b'&' | b'*' | b'+' | b'/' | b'=' | b'?' | b'^' | b'_' | b'|'
            | b'~' => Some(T::CssPunct),
            b';' | b'<' | b'>' | b'\\' | b'`' => Some(T::LabelPunct),
            _ => None,
        };
        if let Some(t) = punct {
            emit!(t, 1);
            continue;
        }
        if c >= 0x80 {
            let len = src[i..]
                .char_indices()
                .find(|(_, ch)| ch.is_ascii())
                .map_or(src.len() - i, |(k, _)| k);
            emit!(T::LabelSymbol, len);
            continue;
        }
        let ch = src[i..].chars().next().unwrap_or('?');
        return Err(err(i, &format!("unexpected character '{ch}'")));
    }
    out.push(Tok {
        t: T::Eof,
        text: "",
        start: src.len(),
        end: src.len(),
    });
    Ok(out)
}

// ───────────────────────────── parser ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Actor,
    UseCase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymKind {
    Actor,
    UseCase,
    Boundary,
    Json,
    Edge,
}

impl SymKind {
    fn name(self) -> &'static str {
        match self {
            SymKind::Actor => "actor",
            SymKind::UseCase => "usecase",
            SymKind::Boundary => "boundary",
            SymKind::Json => "json",
            SymKind::Edge => "edge",
        }
    }
}

impl From<Kind> for SymKind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Actor => SymKind::Actor,
            Kind::UseCase => SymKind::UseCase,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PropValue {
    Str(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
struct Prop {
    key: String,
    value: PropValue,
    loc: Loc,
}

/// An actor or use case as written (`actorItem` / `entityName`).
#[derive(Debug, Clone)]
struct Entity {
    id: String,
    label: Label,
    loc: Loc,
    generated: bool,
    shape: Option<UseCaseShape>,
    metadata: Option<Vec<Prop>>,
    stereotype: Option<String>,
    classes: Vec<String>,
    explicit: bool,
}

#[derive(Debug, Clone)]
struct ElementDraft {
    kind: Kind,
    entity: Entity,
    parent: Option<(String, Loc)>,
}

#[derive(Debug, Clone)]
struct Endpoint {
    id: String,
    label: Label,
    loc: Loc,
    generated: bool,
    declaration: bool,
    classes_on_ref: bool,
}

#[derive(Debug, Clone)]
struct Arrow {
    kind: RelationshipType,
    arrow: ArrowType,
    label: Option<Label>,
    minlen: usize,
}

#[derive(Debug, Clone)]
struct RelationDraft {
    source: Endpoint,
    target: Endpoint,
    loc: Loc,
    explicit_id: Option<(String, Loc)>,
    arrow: Arrow,
}

#[derive(Debug, Clone)]
struct BoundaryDraft {
    id: String,
    label: Label,
    loc: Loc,
    generated: bool,
    classes: Vec<String>,
}

#[derive(Debug, Clone)]
struct JsonDraft {
    id: String,
    rows: Vec<JsonRow>,
    loc: Loc,
    classes: Vec<String>,
}

#[derive(Debug, Clone)]
struct NoteDraft {
    target: String,
    target_loc: Loc,
    label: Label,
}

#[derive(Debug, Clone)]
struct MetaAssign {
    target: String,
    loc: Loc,
    props: Vec<Prop>,
}

/// `class a, b c1, c2`: the targets (with their positions) and the classes.
type ClassAssign = (Vec<(String, Loc)>, Vec<String>);

/// Everything the grammar pass collects, in source order (the visitor's drafts).
#[derive(Debug, Default)]
struct Drafts {
    elements: Vec<ElementDraft>,
    relations: Vec<RelationDraft>,
    boundaries: Vec<BoundaryDraft>,
    json: Vec<JsonDraft>,
    notes: Vec<NoteDraft>,
    meta: Vec<MetaAssign>,
    class_defs: Vec<(Vec<String>, Vec<String>)>,
    class_assigns: Vec<ClassAssign>,
    style_assigns: Vec<(String, Loc, Vec<String>)>,
    directions: Vec<Direction>,
}

const FORBIDDEN_PLANTUML: [&str; 5] = [
    "allowmixing",
    "newpage",
    "package",
    "rectangle",
    "skinparam",
];

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Tok<'a>>,
    pos: usize,
    lines: Lines,
    d: Drafts,
    parent: Option<(String, Loc)>,
}

/// `label.replace(/\W/g, '_')` (per UTF-16 code unit, as JavaScript does).
fn generate_id(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('_', c.len_utf16()));
        }
    }
    out
}

/// `max(1, dashes - 1)` for solid association operators.
fn solid_minlen(op: &str) -> usize {
    op.matches('-').count().saturating_sub(1).max(1)
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Result<Self> {
        Ok(Parser {
            src,
            toks: lex(src)?,
            pos: 0,
            lines: Lines::new(src),
            d: Drafts::default(),
            parent: None,
        })
    }

    fn la(&self, k: usize) -> Tok<'a> {
        let i = (self.pos + k - 1).min(self.toks.len() - 1);
        self.toks[i]
    }
    fn peek(&self) -> T {
        self.la(1).t
    }
    fn loc(&self, offset: usize) -> Loc {
        self.lines.loc(self.src, offset)
    }
    fn bump(&mut self) -> Tok<'a> {
        let t = self.la(1);
        if t.t != T::Eof {
            self.pos += 1;
        }
        t
    }
    fn error_at(&self, tok: Tok<'_>, msg: &str) -> Error {
        let loc = self.loc(tok.start);
        let found = if tok.t == T::Eof {
            "end of input".to_string()
        } else {
            format!("'{}'", tok.text.escape_debug())
        };
        Error::parse(loc.line, format!("{msg} but found {found} at {}", loc.at()))
    }
    fn expect(&mut self, t: T) -> Result<Tok<'a>> {
        if self.peek() == t {
            Ok(self.bump())
        } else {
            Err(self.error_at(self.la(1), &format!("Expecting {}", t.name())))
        }
    }
    fn line_end(&mut self) -> Result<()> {
        match self.peek() {
            T::Newline => {
                self.bump();
                Ok(())
            }
            T::Eof => Ok(()),
            _ => Err(self.error_at(
                self.la(1),
                "Expecting a new line or end of input after the statement",
            )),
        }
    }
    fn skip_newlines(&mut self) {
        while self.peek() == T::Newline {
            self.bump();
        }
    }

    fn token_label(&self, tok: Tok<'_>) -> Label {
        match tok.t {
            T::Markdown => Label {
                text: tok.text[2..tok.text.len() - 2].to_string(),
                kind: LabelKind::Markdown,
            },
            T::Plain => Label::text(&tok.text[1..tok.text.len() - 1]),
            _ => Label::text(tok.text),
        }
    }

    fn parse(mut self) -> Result<Drafts> {
        // Front-matter and directives leave blank lines before the header.
        while matches!(self.peek(), T::Newline | T::Comment) {
            self.bump();
        }
        self.expect(T::Kw(Kw::Usecase))?;
        self.line_end()?;
        while self.peek() != T::Eof {
            match self.peek() {
                T::Newline => {
                    self.bump();
                }
                T::Comment => {
                    self.bump();
                    self.line_end()?;
                }
                _ => self.statement()?,
            }
        }
        Ok(self.d)
    }

    fn statement(&mut self) -> Result<()> {
        match self.peek() {
            T::AccTitle | T::AccDescr => {
                // Already extracted by the shared preprocessor.
                self.bump();
                self.line_end()
            }
            T::Kw(Kw::Direction) => self.direction(),
            T::Kw(Kw::Actor) => self.actor_statement(),
            T::Kw(Kw::SystemBoundary) => self.boundary(),
            T::Kw(Kw::Note) => self.note(),
            T::JsonStart => self.json(),
            T::Kw(Kw::ClassDef) => self.class_def(),
            T::Kw(Kw::Class) => self.class_statement(),
            T::Kw(Kw::Style) => self.style_statement(),
            T::Ident | T::Plain | T::Markdown if self.la(2).t == T::MetaStart => {
                self.metadata_assignment()
            }
            T::Ident
                if FORBIDDEN_PLANTUML.contains(&self.la(1).text.to_ascii_lowercase().as_str()) =>
            {
                Err(self.error_at(
                    self.la(1),
                    "Expecting a usecase statement (PlantUML syntax is not supported)",
                ))
            }
            T::Ident | T::Plain | T::Markdown => self.entity_statement(),
            _ => Err(self.error_at(self.la(1), "Expecting a statement")),
        }
    }

    fn direction(&mut self) -> Result<()> {
        self.bump();
        let tok = self.bump();
        let dir = match tok.t {
            T::Kw(Kw::Td | Kw::Tb) => Direction::TopBottom,
            T::Kw(Kw::Bt) => Direction::BottomTop,
            T::Kw(Kw::Lr) => Direction::LeftRight,
            T::Kw(Kw::Rl) => Direction::RightLeft,
            _ => return Err(self.error_at(tok, "Expecting one of TD, TB, BT, LR, RL")),
        };
        self.d.directions.push(dir);
        self.line_end()
    }

    /// `nodeLabel` / `edgeLabel`: a quoted string or a run of unquoted label tokens,
    /// rebuilt from the source slice so spacing is kept.
    fn node_label(&mut self) -> Result<Label> {
        match self.peek() {
            T::Plain | T::Markdown => {
                let tok = self.bump();
                Ok(self.token_label(tok))
            }
            t if t.is_label_text() => {
                let first = self.bump();
                let mut last = first;
                while self.peek().is_label_text() {
                    last = self.bump();
                }
                Ok(Label::text(&self.src[first.start..last.end]))
            }
            _ => Err(self.error_at(self.la(1), "Expecting a label")),
        }
    }

    fn metadata(&mut self) -> Result<Vec<Prop>> {
        self.expect(T::MetaStart)?;
        self.skip_newlines();
        let mut props = Vec::new();
        if self.peek() != T::RBrace {
            props.push(self.property()?);
            loop {
                match self.peek() {
                    T::Comma => {
                        self.bump();
                        self.skip_newlines();
                    }
                    T::Newline => {
                        self.skip_newlines();
                        if self.peek() == T::Comma {
                            self.bump();
                        }
                        self.skip_newlines();
                    }
                    _ => break,
                }
                if self.peek() == T::RBrace {
                    break;
                }
                props.push(self.property()?);
            }
        }
        self.expect(T::RBrace)?;
        Ok(props)
    }

    fn property(&mut self) -> Result<Prop> {
        let key_tok = self.bump();
        let key = match key_tok.t {
            T::Ident => key_tok.text.to_string(),
            T::Plain => key_tok.text[1..key_tok.text.len() - 1].to_string(),
            _ => return Err(self.error_at(key_tok, "Expecting a metadata key")),
        };
        self.expect(T::Colon)?;
        let v = self.bump();
        let value = match v.t {
            T::Ident => PropValue::Str(v.text.to_string()),
            T::Plain => PropValue::Str(v.text[1..v.text.len() - 1].to_string()),
            T::Kw(Kw::True) => PropValue::Bool(true),
            T::Kw(Kw::False) => PropValue::Bool(false),
            _ => return Err(self.error_at(v, "Expecting a metadata value")),
        };
        Ok(Prop {
            key,
            value,
            loc: self.loc(key_tok.start),
        })
    }

    fn stereotype(&mut self) -> Result<String> {
        self.expect(T::StereoStart)?;
        let text = self.expect(T::StereoText)?.text.trim().to_string();
        self.expect(T::StereoEnd)?;
        Ok(text)
    }

    fn class_suffix(&mut self) -> Result<Vec<String>> {
        self.expect(T::ClassSep)?;
        let mut classes = vec![self.expect(T::Ident)?.text.to_string()];
        while self.peek() == T::Comma && self.la(2).t == T::Ident {
            self.bump();
            classes.push(self.bump().text.to_string());
        }
        Ok(classes)
    }

    /// The optional `@{…}`, `<<…>>` and `:::…` suffixes, in that order.
    fn suffixes(&mut self, e: &mut Entity) -> Result<()> {
        if self.peek() == T::MetaStart {
            e.metadata = Some(self.metadata()?);
        }
        if self.peek() == T::StereoStart {
            e.stereotype = Some(self.stereotype()?);
        }
        if self.peek() == T::ClassSep {
            e.classes = self.class_suffix()?;
        }
        Ok(())
    }

    /// `actorName` (`allow_rect = false`) or `entityName` (`true`), without suffixes.
    fn name(&mut self, allow_rect: bool) -> Result<Entity> {
        let tok = self.bump();
        let loc = self.loc(tok.start);
        let entity = |id: String, label: Label, generated: bool, shape| Entity {
            id,
            label,
            loc,
            generated,
            shape,
            metadata: None,
            stereotype: None,
            classes: Vec::new(),
            explicit: false,
        };
        match tok.t {
            T::Ident => {
                let (label, shape) = match self.peek() {
                    T::LParen => {
                        self.bump();
                        let l = self.node_label()?;
                        self.expect(T::RParen)?;
                        (l, Some(UseCaseShape::Ellipse))
                    }
                    T::LBracket if allow_rect => {
                        self.bump();
                        let l = self.node_label()?;
                        self.expect(T::RBracket)?;
                        (l, Some(UseCaseShape::Rect))
                    }
                    _ => (Label::text(tok.text), None),
                };
                Ok(entity(tok.text.to_string(), label, false, shape))
            }
            T::Plain | T::Markdown => {
                let label = self.token_label(tok);
                Ok(entity(generate_id(&label.text), label, true, None))
            }
            _ => Err(self.error_at(tok, "Expecting an identifier or a string")),
        }
    }

    fn entity_name(&mut self) -> Result<Entity> {
        let mut e = self.name(true)?;
        self.suffixes(&mut e)?;
        e.explicit = e.shape.is_some() || e.metadata.is_some() || e.stereotype.is_some();
        Ok(e)
    }

    fn actor_item(&mut self) -> Result<Entity> {
        let mut e = self.name(false)?;
        // An actor's parenthesised label does not select a shape.
        e.shape = None;
        self.suffixes(&mut e)?;
        e.explicit = true;
        Ok(e)
    }

    fn element(&mut self, kind: Kind, entity: Entity) {
        self.d.elements.push(ElementDraft {
            kind,
            entity,
            parent: self.parent.clone(),
        });
    }

    fn relation_start(&self) -> bool {
        self.peek().is_arrow() || (self.peek() == T::Ident && self.la(2).t == T::At)
    }

    fn endpoint(e: &Entity, declaration: bool) -> Endpoint {
        Endpoint {
            id: e.id.clone(),
            label: e.label.clone(),
            loc: e.loc,
            generated: e.generated,
            declaration,
            classes_on_ref: !e.classes.is_empty(),
        }
    }

    /// `relationTail` after `source`; records the relationship (and an explicitly
    /// declared target).
    fn relation_tail(&mut self, source: Endpoint) -> Result<()> {
        let start = self.la(1).start;
        let explicit_id = if self.peek() == T::Ident && self.la(2).t == T::At {
            let id = self.bump();
            self.bump();
            Some((id.text.to_string(), self.loc(id.start)))
        } else {
            None
        };
        let arrow = self.arrow()?;
        let target = self.entity_name()?;
        if target.explicit {
            let mut declared = target.clone();
            declared.shape.get_or_insert(UseCaseShape::Ellipse);
            self.element(Kind::UseCase, declared);
        }
        let target = Self::endpoint(&target, target.explicit);
        self.d.relations.push(RelationDraft {
            source,
            target,
            loc: self.loc(start),
            explicit_id,
            arrow,
        });
        Ok(())
    }

    fn has_labeled_right(&self, allowed: &[T]) -> bool {
        if !self.peek().is_label() {
            return false;
        }
        let mut k = 2;
        loop {
            let t = self.la(k).t;
            if allowed.contains(&t) {
                return true;
            }
            if !t.is_label() {
                return false;
            }
            k += 1;
        }
    }

    fn arrow(&mut self) -> Result<Arrow> {
        let tok = self.bump();
        let assoc = |arrow, minlen, label| Arrow {
            kind: RelationshipType::Association,
            arrow,
            label,
            minlen,
        };
        Ok(match tok.t {
            T::Dependency => {
                self.expect(T::Colon)?;
                let k = self.bump();
                let kind = match k.t {
                    T::Kw(Kw::Include) => RelationshipType::Include,
                    T::Kw(Kw::Extend) => RelationshipType::Extend,
                    _ => return Err(self.error_at(k, "Expecting include or extend")),
                };
                Arrow {
                    kind,
                    arrow: ArrowType::SolidArrow,
                    label: None,
                    minlen: 1,
                }
            }
            T::Generalization => Arrow {
                kind: RelationshipType::Generalization,
                arrow: ArrowType::SolidArrow,
                label: None,
                minlen: 1,
            },
            T::FwdSolid => assoc(ArrowType::SolidArrow, solid_minlen(tok.text), None),
            T::BackSolid => {
                if tok.text == "<--" && self.has_labeled_right(&[T::Markerless]) {
                    let label = self.node_label()?;
                    let m = self.expect(T::Markerless)?;
                    assoc(ArrowType::BackArrow, solid_minlen(m.text), Some(label))
                } else {
                    assoc(ArrowType::BackArrow, solid_minlen(tok.text), None)
                }
            }
            T::Markerless => {
                let allowed = [T::FwdSolid, T::Markerless, T::FwdCircle, T::FwdCross];
                if tok.text == "--" && self.has_labeled_right(&allowed) {
                    let label = self.node_label()?;
                    let last = self.bump();
                    let (arrow, minlen) = match last.t {
                        T::FwdSolid => (ArrowType::SolidArrow, solid_minlen(last.text)),
                        T::FwdCircle => (ArrowType::CircleArrow, 1),
                        T::FwdCross => (ArrowType::CrossArrow, 1),
                        _ => (ArrowType::LineSolid, solid_minlen(last.text)),
                    };
                    assoc(arrow, minlen, Some(label))
                } else {
                    assoc(ArrowType::LineSolid, solid_minlen(tok.text), None)
                }
            }
            T::FwdCircle => assoc(ArrowType::CircleArrow, 1, None),
            T::FwdCross => assoc(ArrowType::CrossArrow, 1, None),
            T::BackCircle | T::BackCross => {
                let arrow = if tok.t == T::BackCircle {
                    ArrowType::CircleArrowReversed
                } else {
                    ArrowType::CrossArrowReversed
                };
                let label = if self.has_labeled_right(&[T::Markerless]) {
                    let l = self.node_label()?;
                    self.expect(T::Markerless)?;
                    Some(l)
                } else {
                    None
                };
                assoc(arrow, 1, label)
            }
            _ => return Err(self.error_at(tok, "Expecting a relationship operator")),
        })
    }

    fn actor_statement(&mut self) -> Result<()> {
        self.bump();
        let mut items = vec![self.actor_item()?];
        let relation = if self.peek() == T::Comma {
            while self.peek() == T::Comma {
                self.bump();
                items.push(self.actor_item()?);
            }
            false
        } else {
            self.relation_start()
        };
        let source = Self::endpoint(&items[0], true);
        for item in items {
            self.element(Kind::Actor, item);
        }
        if relation {
            self.relation_tail(source)?;
        }
        self.line_end()
    }

    fn entity_statement(&mut self) -> Result<()> {
        let mut source = self.entity_name()?;
        if !self.relation_start() {
            source.explicit = true;
            source.shape.get_or_insert(UseCaseShape::Ellipse);
            self.element(Kind::UseCase, source);
            return self.line_end();
        }
        if source.explicit {
            let mut declared = source.clone();
            declared.shape.get_or_insert(UseCaseShape::Ellipse);
            self.element(Kind::UseCase, declared);
        }
        let endpoint = Self::endpoint(&source, source.explicit);
        self.relation_tail(endpoint)?;
        self.line_end()
    }

    fn boundary(&mut self) -> Result<()> {
        self.bump();
        let tok = self.bump();
        let loc = self.loc(tok.start);
        let (id, label, generated) = match tok.t {
            T::Ident => {
                let label = match self.peek() {
                    T::LParen | T::LBracket => {
                        let close = if self.bump().t == T::LParen {
                            T::RParen
                        } else {
                            T::RBracket
                        };
                        let l = self.node_label()?;
                        self.expect(close)?;
                        l
                    }
                    _ => Label::text(tok.text),
                };
                (tok.text.to_string(), label, false)
            }
            T::Plain | T::Markdown => {
                let label = self.token_label(tok);
                (generate_id(&label.text), label, true)
            }
            _ => return Err(self.error_at(tok, "Expecting a system boundary name")),
        };
        let metadata = if self.peek() == T::MetaStart {
            Some(self.metadata()?)
        } else {
            None
        };
        let classes = if self.peek() == T::ClassSep {
            self.class_suffix()?
        } else {
            Vec::new()
        };
        self.line_end()?;
        self.d.boundaries.push(BoundaryDraft {
            id: id.clone(),
            label,
            loc,
            generated,
            classes,
        });
        let previous = self.parent.replace((id.clone(), loc));
        loop {
            match self.peek() {
                T::Newline => {
                    self.bump();
                }
                T::Comment => {
                    self.bump();
                    self.line_end()?;
                }
                T::Kw(Kw::End) => break,
                T::Kw(Kw::Actor) => {
                    self.bump();
                    let first = self.actor_item()?;
                    self.element(Kind::Actor, first);
                    while self.peek() == T::Comma {
                        self.bump();
                        let item = self.actor_item()?;
                        self.element(Kind::Actor, item);
                    }
                    self.line_end()?;
                }
                T::Ident | T::Plain | T::Markdown => {
                    let mut e = self.entity_name()?;
                    e.explicit = true;
                    e.shape.get_or_insert(UseCaseShape::Ellipse);
                    self.element(Kind::UseCase, e);
                    self.line_end()?;
                }
                _ => {
                    return Err(self.error_at(
                        self.la(1),
                        "Expecting an actor or use case declaration, or 'end'",
                    ));
                }
            }
        }
        self.expect(T::Kw(Kw::End))?;
        self.parent = previous;
        if let Some(props) = metadata {
            // Inline metadata resolves exactly like a later `id@{ … }` statement.
            self.d.meta.push(MetaAssign {
                target: id,
                loc,
                props,
            });
        }
        self.line_end()
    }

    fn note(&mut self) -> Result<()> {
        self.bump();
        self.expect(T::Kw(Kw::For))?;
        let target = self.expect(T::Ident)?;
        let label = self.node_label()?;
        self.d.notes.push(NoteDraft {
            target: target.text.to_string(),
            target_loc: self.loc(target.start),
            label,
        });
        self.line_end()
    }

    fn json(&mut self) -> Result<()> {
        let start = self.bump();
        let after_kw = start.text[4..].trim_start_matches([' ', '\t']);
        let id: String = after_kw
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let loc = self.loc(start.start + (start.text.len() - after_kw.len()));
        let literal = self.expect(T::JsonObject)?;
        let value = json::parse(literal.text).map_err(|e| {
            let at = self.loc(literal.start + e.offset.min(literal.text.len()));
            Error::parse(
                at.line,
                format!("Invalid JSON for '{id}': {} at {}", e.message, at.at()),
            )
        })?;
        if value.as_object().is_none() {
            return Err(Error::parse(
                loc.line,
                format!("JSON '{id}' must be an object at {}", loc.at()),
            ));
        }
        let classes = if self.peek() == T::ClassSep {
            self.class_suffix()?
        } else {
            Vec::new()
        };
        self.d.json.push(JsonDraft {
            id,
            rows: flatten_json(&value),
            loc,
            classes,
        });
        self.line_end()
    }

    fn id_list(&mut self) -> Result<Vec<Tok<'a>>> {
        let mut ids = vec![self.expect(T::Ident)?];
        while self.peek() == T::Comma && self.la(2).t == T::Ident {
            self.bump();
            ids.push(self.bump());
        }
        Ok(ids)
    }

    fn styles(&mut self) -> Result<Vec<String>> {
        let mut out = vec![self.style_value()?];
        while self.peek() == T::Comma {
            self.bump();
            out.push(self.style_value()?);
        }
        Ok(out)
    }

    fn style_value(&mut self) -> Result<String> {
        let first = self.la(1);
        match first.t {
            t if t.is_word() || t == T::CssIdent => {
                self.bump();
            }
            T::Markerless if self.la(2).t.is_word() => {
                self.bump();
                self.bump();
            }
            _ => return Err(self.error_at(first, "Expecting a CSS property")),
        }
        self.expect(T::Colon)?;
        let component = |t: T| {
            t.is_word()
                || matches!(
                    t,
                    T::Plain
                        | T::Number
                        | T::Hash
                        | T::CssIdent
                        | T::EscComma
                        | T::Dash
                        | T::Dot
                        | T::Percent
                        | T::CssPunct
                        | T::Colon
                        | T::LParen
                        | T::RParen
                        | T::LBracket
                        | T::RBracket
                        | T::LBrace
                        | T::RBrace
                        | T::At
                        | T::Markerless
                )
        };
        if !component(self.peek()) {
            return Err(self.error_at(self.la(1), "Expecting a CSS value"));
        }
        let mut last = self.bump();
        while component(self.peek()) {
            last = self.bump();
        }
        Ok(self.src[first.start..last.end].replace("\\,", ","))
    }

    fn class_def(&mut self) -> Result<()> {
        self.bump();
        let ids = self.id_list()?;
        let styles = self.styles()?;
        self.d
            .class_defs
            .push((ids.iter().map(|t| t.text.to_string()).collect(), styles));
        self.line_end()
    }

    fn class_statement(&mut self) -> Result<()> {
        self.bump();
        let targets = self.id_list()?;
        if self.peek() != T::Ident {
            return Err(self.error_at(self.la(1), "Expecting a class name"));
        }
        let classes = self.id_list()?;
        let targets = targets
            .iter()
            .map(|t| (t.text.to_string(), self.loc(t.start)))
            .collect();
        self.d.class_assigns.push((
            targets,
            classes.iter().map(|t| t.text.to_string()).collect(),
        ));
        self.line_end()
    }

    fn style_statement(&mut self) -> Result<()> {
        self.bump();
        let target = self.expect(T::Ident)?;
        let styles = self.styles()?;
        self.d
            .style_assigns
            .push((target.text.to_string(), self.loc(target.start), styles));
        self.line_end()
    }

    fn metadata_assignment(&mut self) -> Result<()> {
        let tok = self.bump();
        let target = match tok.t {
            T::Ident => tok.text.to_string(),
            _ => generate_id(&self.token_label(tok).text),
        };
        let props = self.metadata()?;
        self.d.meta.push(MetaAssign {
            target,
            loc: self.loc(tok.start),
            props,
        });
        self.line_end()
    }
}

/// `flattenJsonRows`: object properties in source order (a repeated key keeps its first
/// position and its last value), arrays in index order, leaves as `a.b` / `a[0].b` paths.
fn flatten_json(value: &Json) -> Vec<JsonRow> {
    fn scalar(v: &Json) -> Option<String> {
        Some(match v {
            Json::Null => "null".into(),
            Json::Bool(b) => b.to_string(),
            Json::Number(n) => format!("{n}"),
            Json::String(s) => s.clone(),
            _ => return None,
        })
    }
    fn row(key: &str, value: String) -> JsonRow {
        JsonRow {
            key: key.to_string(),
            value,
        }
    }
    fn visit(v: &Json, path: &str, out: &mut Vec<JsonRow>) {
        match v {
            Json::Array(items) if items.is_empty() => out.push(row(path, "[]".into())),
            Json::Array(items) if items.iter().all(|i| scalar(i).is_some()) => {
                for (k, item) in items.iter().enumerate() {
                    let key = if k == 0 { path } else { "" };
                    out.push(row(key, scalar(item).unwrap_or_default()));
                }
            }
            Json::Array(items) => {
                for (k, item) in items.iter().enumerate() {
                    visit(item, &format!("{path}[{k}]"), out);
                }
            }
            Json::Object(pairs) => {
                let mut merged: Vec<(&str, &Json)> = Vec::new();
                for (k, v) in pairs {
                    match merged.iter_mut().find(|(mk, _)| mk == k) {
                        Some(slot) => slot.1 = v,
                        None => merged.push((k, v)),
                    }
                }
                if merged.is_empty() {
                    out.push(row(path, "{}".into()));
                }
                for (k, child) in merged {
                    let p = if path.is_empty() {
                        k.to_string()
                    } else {
                        format!("{path}.{k}")
                    };
                    visit(child, &p, out);
                }
            }
            leaf => out.push(row(path, scalar(leaf).unwrap_or_default())),
        }
    }
    let mut out = Vec::new();
    visit(value, "", &mut out);
    out
}

// ───────────────────────────── semantic pass ─────────────────────────────

#[derive(Debug, Clone)]
struct Symbol {
    kind: SymKind,
    loc: Loc,
    generated: bool,
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct ElementState {
    kind: Kind,
    id: String,
    label: Label,
    loc: Loc,
    shape: Option<UseCaseShape>,
    parent: Option<(String, Loc)>,
    stereotype: Option<String>,
    classes: Vec<String>,
    styles: Vec<String>,
    actor_type: Option<ActorType>,
    icon: Option<String>,
    business: Option<bool>,
}

#[derive(Debug, Clone)]
struct BoundaryState {
    id: String,
    label: Label,
    loc: Loc,
    kind: Option<BoundaryType>,
    classes: Vec<String>,
    styles: Vec<String>,
}

fn label_suffix(label: Option<&str>) -> String {
    label.map_or(String::new(), |l| format!(" (from label '{l}')"))
}

fn conflict(
    msg: &str,
    current: Loc,
    previous: Loc,
    current_label: Option<&str>,
    previous_label: Option<&str>,
) -> Error {
    Error::parse(
        current.line,
        format!(
            "{msg} at {}{}; previous declaration at {}{}",
            current.at(),
            label_suffix(current_label),
            previous.at(),
            label_suffix(previous_label)
        ),
    )
}

fn at_error(loc: Loc, msg: &str) -> Error {
    Error::parse(loc.line, format!("{msg} at {}", loc.at()))
}

fn invalid_metadata(id: &str, kind: SymKind, p: &Prop) -> Error {
    at_error(
        p.loc,
        &format!(
            "Metadata property '{}' is invalid for {} '{id}'",
            p.key,
            kind.name()
        ),
    )
}

fn push_unique(target: &mut Vec<String>, values: &[String]) {
    for v in values {
        if !target.contains(v) {
            target.push(v.clone());
        }
    }
}

#[derive(Default)]
struct Builder {
    symbols: HashMap<String, Symbol>,
    states: Vec<ElementState>,
    state_idx: HashMap<String, usize>,
    boundaries: Vec<BoundaryState>,
    boundary_idx: HashMap<String, usize>,
    first: HashMap<String, usize>,
}

fn generated_label(s: &Symbol) -> Option<&str> {
    if s.generated {
        s.label.as_deref()
    } else {
        None
    }
}

impl Builder {
    fn record_first(&mut self, id: &str, offset: usize) {
        let e = self.first.entry(id.to_string()).or_insert(offset);
        *e = (*e).min(offset);
    }

    fn register_unique(&mut self, id: &str, kind: SymKind, loc: Loc) -> Result<()> {
        if let Some(prev) = self.symbols.get(id) {
            return Err(conflict(
                &format!(
                    "ID '{id}' is declared more than once ({} and {})",
                    prev.kind.name(),
                    kind.name()
                ),
                loc,
                prev.loc,
                None,
                generated_label(prev),
            ));
        }
        self.symbols.insert(
            id.to_string(),
            Symbol {
                kind,
                loc,
                generated: false,
                label: None,
            },
        );
        Ok(())
    }

    fn actor_property(state: &mut ElementState, p: &Prop, replace: bool) -> Result<()> {
        let conflicting = |what: &str| {
            conflict(
                &format!("Actor '{}' has conflicting {what} metadata", state.id),
                p.loc,
                state.loc,
                None,
                None,
            )
        };
        match (p.key.as_str(), &p.value) {
            ("type", PropValue::Str(v))
                if matches!(v.as_str(), "normal" | "hollow" | "awesome") =>
            {
                let t = match v.as_str() {
                    "hollow" => ActorType::Hollow,
                    "awesome" => ActorType::Awesome,
                    _ => ActorType::Normal,
                };
                if !replace && state.actor_type.is_some_and(|o| o != t) {
                    return Err(conflicting("type"));
                }
                state.actor_type = Some(t);
            }
            ("icon", PropValue::Str(v)) => {
                if !replace && state.icon.as_ref().is_some_and(|o| o != v) {
                    return Err(conflicting("icon"));
                }
                state.icon = Some(v.clone());
            }
            ("business", PropValue::Bool(v)) => {
                if !replace && state.business.is_some_and(|o| o != *v) {
                    return Err(conflicting("business"));
                }
                state.business = Some(*v);
            }
            _ => return Err(invalid_metadata(&state.id, SymKind::Actor, p)),
        }
        Ok(())
    }

    fn element_metadata(state: &mut ElementState, props: &[Prop], replace: bool) -> Result<()> {
        for p in props {
            if state.kind == Kind::Actor {
                Self::actor_property(state, p, replace)?;
                continue;
            }
            match (p.key.as_str(), &p.value) {
                ("business", PropValue::Bool(v)) => {
                    if !replace && state.business.is_some_and(|o| o != *v) {
                        return Err(conflict(
                            &format!("Use case '{}' has conflicting business metadata", state.id),
                            p.loc,
                            state.loc,
                            None,
                            None,
                        ));
                    }
                    state.business = Some(*v);
                }
                _ => return Err(invalid_metadata(&state.id, SymKind::UseCase, p)),
            }
        }
        Ok(())
    }

    /// Duplicate-ID checks shared by elements and boundaries.
    fn check_origin(
        &self,
        id: &str,
        kind: SymKind,
        loc: Loc,
        generated: bool,
        label: &str,
    ) -> Result<()> {
        let Some(origin) = self.symbols.get(id) else {
            return Ok(());
        };
        let draft_label = generated.then_some(label);
        if origin.kind != kind {
            return Err(conflict(
                &format!(
                    "ID '{id}' is declared as both {} and {}",
                    origin.kind.name(),
                    kind.name()
                ),
                loc,
                origin.loc,
                draft_label,
                generated_label(origin),
            ));
        }
        if origin.generated || generated {
            return Err(conflict(
                &format!("Generated ID '{id}' collides with another declaration"),
                loc,
                origin.loc,
                draft_label,
                generated_label(origin),
            ));
        }
        Ok(())
    }

    fn collect_element(&mut self, draft: &ElementDraft) -> Result<()> {
        let e = &draft.entity;
        let kind: SymKind = draft.kind.into();
        self.check_origin(&e.id, kind, e.loc, e.generated, &e.label.text)?;
        let Some(&idx) = self.state_idx.get(&e.id) else {
            let mut state = ElementState {
                kind: draft.kind,
                id: e.id.clone(),
                label: e.label.clone(),
                loc: e.loc,
                shape: e.shape,
                parent: draft.parent.clone(),
                stereotype: e.stereotype.clone(),
                classes: e.classes.clone(),
                styles: Vec::new(),
                actor_type: None,
                icon: None,
                business: None,
            };
            if let Some(m) = &e.metadata {
                Self::element_metadata(&mut state, m, false)?;
            }
            self.state_idx.insert(e.id.clone(), self.states.len());
            self.states.push(state);
            self.symbols.insert(
                e.id.clone(),
                Symbol {
                    kind,
                    loc: e.loc,
                    generated: e.generated,
                    label: Some(e.label.text.clone()),
                },
            );
            return Ok(());
        };
        let existing = &mut self.states[idx];
        let clash = |msg: String, cur: Loc, prev: Loc| Err(conflict(&msg, cur, prev, None, None));
        if existing.label != e.label {
            return clash(
                format!("ID '{}' has conflicting labels", e.id),
                e.loc,
                existing.loc,
            );
        }
        if let (Some(a), Some(b)) = (e.shape, existing.shape) {
            if a != b {
                return clash(
                    format!("Use case '{}' has conflicting shapes", e.id),
                    e.loc,
                    existing.loc,
                );
            }
        }
        if let (Some((a, al)), Some((b, bl))) = (&draft.parent, &existing.parent) {
            if a != b {
                return clash(
                    format!(
                        "Element '{}' belongs to more than one system boundary",
                        e.id
                    ),
                    *al,
                    *bl,
                );
            }
        }
        if let (Some(a), Some(b)) = (&e.stereotype, &existing.stereotype) {
            if a != b {
                return clash(
                    format!("Element '{}' has conflicting stereotypes", e.id),
                    e.loc,
                    existing.loc,
                );
            }
        }
        existing.shape = existing.shape.or(e.shape);
        if existing.parent.is_none() {
            existing.parent = draft.parent.clone();
        }
        if existing.stereotype.is_none() {
            existing.stereotype = e.stereotype.clone();
        }
        push_unique(&mut existing.classes, &e.classes);
        if let Some(m) = &e.metadata {
            Self::element_metadata(existing, m, false)?;
        }
        Ok(())
    }

    fn collect_boundary(&mut self, draft: &BoundaryDraft) -> Result<()> {
        self.check_origin(
            &draft.id,
            SymKind::Boundary,
            draft.loc,
            draft.generated,
            &draft.label.text,
        )?;
        if let Some(&idx) = self.boundary_idx.get(&draft.id) {
            let existing = &mut self.boundaries[idx];
            if existing.label != draft.label {
                return Err(conflict(
                    &format!("Boundary '{}' has conflicting titles", draft.id),
                    draft.loc,
                    existing.loc,
                    None,
                    None,
                ));
            }
            push_unique(&mut existing.classes, &draft.classes);
            return Ok(());
        }
        self.boundary_idx
            .insert(draft.id.clone(), self.boundaries.len());
        self.boundaries.push(BoundaryState {
            id: draft.id.clone(),
            label: draft.label.clone(),
            loc: draft.loc,
            kind: None,
            classes: draft.classes.clone(),
            styles: Vec::new(),
        });
        self.symbols.insert(
            draft.id.clone(),
            Symbol {
                kind: SymKind::Boundary,
                loc: draft.loc,
                generated: draft.generated,
                label: Some(draft.label.text.clone()),
            },
        );
        Ok(())
    }

    /// The endpoint's kind; an undeclared endpoint becomes an ellipse use case.
    fn resolve_endpoint(&mut self, ep: &Endpoint) -> Result<SymKind> {
        if ep.classes_on_ref && !ep.declaration {
            return Err(at_error(
                ep.loc,
                &format!(
                    "Relationship endpoint '{}' uses ::: without declaring the node",
                    ep.id
                ),
            ));
        }
        if let Some(s) = self.symbols.get(&ep.id) {
            return Ok(s.kind);
        }
        self.state_idx.insert(ep.id.clone(), self.states.len());
        self.states.push(ElementState {
            kind: Kind::UseCase,
            id: ep.id.clone(),
            label: ep.label.clone(),
            loc: ep.loc,
            shape: Some(UseCaseShape::Ellipse),
            parent: None,
            stereotype: None,
            classes: Vec::new(),
            styles: Vec::new(),
            actor_type: None,
            icon: None,
            business: None,
        });
        self.symbols.insert(
            ep.id.clone(),
            Symbol {
                kind: SymKind::UseCase,
                loc: ep.loc,
                generated: ep.generated,
                label: Some(ep.label.text.clone()),
            },
        );
        self.record_first(&ep.id, ep.loc.offset);
        Ok(SymKind::UseCase)
    }

    fn validate_relationship(&self, r: &RelationDraft, src: SymKind, dst: SymKind) -> Result<()> {
        let allowed = |k: SymKind| matches!(k, SymKind::Actor | SymKind::UseCase | SymKind::Json);
        for (ep, kind, role) in [(&r.source, src, "source"), (&r.target, dst, "target")] {
            if !allowed(kind) {
                let prev = self.symbols.get(&ep.id).map_or(ep.loc, |s| s.loc);
                return Err(conflict(
                    &format!("Relationship {role} '{}' cannot be {}", ep.id, kind.name()),
                    ep.loc,
                    prev,
                    None,
                    None,
                ));
            }
        }
        match r.arrow.kind {
            RelationshipType::Include | RelationshipType::Extend
                if src != SymKind::UseCase || dst != SymKind::UseCase =>
            {
                let name = if r.arrow.kind == RelationshipType::Include {
                    "include"
                } else {
                    "extend"
                };
                Err(at_error(
                    r.loc,
                    &format!("{name} relationship requires use-case endpoints"),
                ))
            }
            RelationshipType::Generalization
                if !matches!(src, SymKind::Actor | SymKind::UseCase) || src != dst =>
            {
                Err(at_error(
                    r.loc,
                    "Generalization requires actor-to-actor or use-case-to-use-case endpoints",
                ))
            }
            RelationshipType::Association
                if (src == SymKind::Json || dst == SymKind::Json)
                    && !matches!(
                        r.arrow.arrow,
                        ArrowType::SolidArrow | ArrowType::BackArrow | ArrowType::LineSolid
                    ) =>
            {
                Err(at_error(
                    r.loc,
                    &format!(
                        "JSON relationship '{}' to '{}' permits only point, reversed-point, or markerless solid association",
                        r.source.id, r.target.id
                    ),
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Which model entity a `class` / `style` / metadata target names.
#[derive(Debug, Clone, Copy)]
enum Target {
    Element(usize),
    Boundary(usize),
    Json(usize),
    Edge(usize),
}

/// Resolve the drafts into the committed model (`UsecaseModelBuilder.finalize`).
fn finalize(d: Drafts, title: Option<String>) -> Result<UsecaseDiagram> {
    let mut b = Builder::default();
    for r in &d.relations {
        b.record_first(&r.source.id, r.source.loc.offset);
        b.record_first(&r.target.id, r.target.loc.offset);
    }
    for e in &d.elements {
        b.record_first(&e.entity.id, e.entity.loc.offset);
    }
    for x in &d.boundaries {
        b.record_first(&x.id, x.loc.offset);
    }
    for x in &d.json {
        b.record_first(&x.id, x.loc.offset);
    }

    // Declarations claim IDs in source order.
    enum Decl<'a> {
        Element(&'a ElementDraft),
        Boundary(&'a BoundaryDraft),
        Json(&'a JsonDraft),
        Edge(&'a str, Loc),
    }
    let mut decls: Vec<(usize, Decl<'_>)> = Vec::new();
    decls.extend(
        d.elements
            .iter()
            .map(|e| (e.entity.loc.offset, Decl::Element(e))),
    );
    decls.extend(
        d.boundaries
            .iter()
            .map(|x| (x.loc.offset, Decl::Boundary(x))),
    );
    decls.extend(d.json.iter().map(|x| (x.loc.offset, Decl::Json(x))));
    decls.extend(
        d.relations
            .iter()
            .filter_map(|r| r.explicit_id.as_ref())
            .map(|(id, loc)| (loc.offset, Decl::Edge(id, *loc))),
    );
    decls.sort_by_key(|(o, _)| *o);
    for (_, decl) in &decls {
        match decl {
            Decl::Element(e) => b.collect_element(e)?,
            Decl::Boundary(x) => b.collect_boundary(x)?,
            Decl::Json(x) => b.register_unique(&x.id, SymKind::Json, x.loc)?,
            Decl::Edge(id, loc) => b.register_unique(id, SymKind::Edge, *loc)?,
        }
    }

    let mut relationships = Vec::new();
    let mut anonymous = 0;
    for r in &d.relations {
        let src = b.resolve_endpoint(&r.source)?;
        let dst = b.resolve_endpoint(&r.target)?;
        b.validate_relationship(r, src, dst)?;
        let id = match &r.explicit_id {
            Some((id, _)) => id.clone(),
            None => {
                anonymous += 1;
                format!("edge-{}", anonymous - 1)
            }
        };
        relationships.push(Relationship {
            id,
            explicit_id: r.explicit_id.is_some(),
            source: r.source.id.clone(),
            target: r.target.id.clone(),
            kind: r.arrow.kind,
            arrow: r.arrow.arrow,
            label: r.arrow.label.clone(),
            minlen: r.arrow.minlen,
            classes: Vec::new(),
            styles: Vec::new(),
            animate: false,
            animation: None,
        });
    }

    // Standalone (and inline boundary) metadata, in source order.
    for m in &d.meta {
        let Some(kind) = b.symbols.get(&m.target).map(|s| s.kind) else {
            return Err(at_error(
                m.loc,
                &format!("Metadata target '{}' is unresolved", m.target),
            ));
        };
        match kind {
            SymKind::Actor | SymKind::UseCase => {
                let idx = b.state_idx[&m.target];
                Builder::element_metadata(&mut b.states[idx], &m.props, true)?;
            }
            SymKind::Boundary => {
                let idx = b.boundary_idx[&m.target];
                for p in &m.props {
                    let t = match (p.key.as_str(), &p.value) {
                        ("type", PropValue::Str(v)) if v == "rect" => BoundaryType::Rect,
                        ("type", PropValue::Str(v)) if v == "package" => BoundaryType::Package,
                        _ => return Err(invalid_metadata(&m.target, kind, p)),
                    };
                    b.boundaries[idx].kind = Some(t);
                }
            }
            SymKind::Edge => {
                let Some(rel) = relationships.iter_mut().find(|r| r.id == m.target) else {
                    return Err(at_error(
                        m.loc,
                        &format!("Metadata target '{}' is not an explicit edge", m.target),
                    ));
                };
                for p in &m.props {
                    match (p.key.as_str(), &p.value) {
                        ("animate", PropValue::Bool(v)) => rel.animate = *v,
                        ("animation", PropValue::Str(v)) if v == "fast" || v == "slow" => {
                            rel.animation = Some(if v == "fast" {
                                Animation::Fast
                            } else {
                                Animation::Slow
                            });
                            rel.animate = true;
                        }
                        _ => return Err(invalid_metadata(&m.target, kind, p)),
                    }
                }
            }
            SymKind::Json => {
                if let Some(p) = m.props.first() {
                    return Err(invalid_metadata(&m.target, kind, p));
                }
            }
        }
    }
    for r in &mut relationships {
        if r.animation.is_some() {
            r.animate = true;
        }
    }

    // Element validation.
    for s in &b.states {
        if s.kind == Kind::Actor {
            if let (Some(_), Some(t)) = (&s.icon, s.actor_type) {
                if t != ActorType::Normal {
                    let name = if t == ActorType::Hollow {
                        "hollow"
                    } else {
                        "awesome"
                    };
                    return Err(at_error(
                        s.loc,
                        &format!("Actor '{}' cannot combine icon with type '{name}'", s.id),
                    ));
                }
            }
            let awesome = s.actor_type == Some(ActorType::Awesome);
            if s.business == Some(true) && (s.icon.is_some() || awesome) {
                return Err(at_error(
                    s.loc,
                    &format!(
                        "Business actor '{}' must use normal or hollow geometry",
                        s.id
                    ),
                ));
            }
        } else if s.shape == Some(UseCaseShape::Rect) && s.business == Some(true) {
            return Err(at_error(
                s.loc,
                &format!(
                    "Rectangular use case '{}' cannot be a business use case",
                    s.id
                ),
            ));
        }
    }

    // Boundary membership, in source order.
    let mut members: Vec<Vec<String>> = vec![Vec::new(); b.boundaries.len()];
    let mut owned: Vec<&ElementDraft> = d.elements.iter().filter(|e| e.parent.is_some()).collect();
    owned.sort_by_key(|e| e.entity.loc.offset);
    for e in owned {
        if let Some((pid, _)) = &e.parent {
            let list = &mut members[b.boundary_idx[pid]];
            if !list.contains(&e.entity.id) {
                list.push(e.entity.id.clone());
            }
        }
    }

    // Notes.
    let mut notes = Vec::new();
    for (n, draft) in d.notes.iter().enumerate() {
        let Some(origin) = b.symbols.get(&draft.target) else {
            return Err(at_error(
                draft.target_loc,
                &format!("Note target '{}' is unresolved", draft.target),
            ));
        };
        if !matches!(origin.kind, SymKind::Actor | SymKind::UseCase) {
            return Err(conflict(
                &format!(
                    "Note target '{}' must be an actor or use case, not {}",
                    draft.target,
                    origin.kind.name()
                ),
                draft.target_loc,
                origin.loc,
                None,
                None,
            ));
        }
        notes.push(Note {
            id: format!("note-{n}"),
            target: draft.target.clone(),
            label: draft.label.clone(),
        });
    }

    // Class definitions (a later definition of a name replaces the earlier one).
    let mut class_defs: Vec<(String, Vec<String>)> = Vec::new();
    for (ids, styles) in &d.class_defs {
        for id in ids {
            match class_defs.iter_mut().find(|(n, _)| n == id) {
                Some(slot) => slot.1 = styles.clone(),
                None => class_defs.push((id.clone(), styles.clone())),
            }
        }
    }

    let mut json_tables: Vec<JsonTable> = d
        .json
        .iter()
        .map(|j| JsonTable {
            id: j.id.clone(),
            rows: j.rows.clone(),
            classes: j.classes.clone(),
            styles: Vec::new(),
        })
        .collect();

    // `class` and `style` statements.
    let resolve = |b: &Builder, json: &[JsonTable], rels: &[Relationship], id: &str| match b
        .symbols
        .get(id)
        .map(|s| s.kind)?
    {
        SymKind::Actor | SymKind::UseCase => Some(Target::Element(b.state_idx[id])),
        SymKind::Boundary => Some(Target::Boundary(b.boundary_idx[id])),
        SymKind::Json => json.iter().position(|j| j.id == id).map(Target::Json),
        SymKind::Edge => rels.iter().position(|r| r.id == id).map(Target::Edge),
    };
    let unresolved = |id: &str, loc: Loc| {
        at_error(
            loc,
            &format!("Class/style target '{id}' is unresolved or anonymous"),
        )
    };
    let mut assignments: Vec<(Target, &[String], bool)> = Vec::new();
    for (targets, classes) in &d.class_assigns {
        for (id, loc) in targets {
            let t = resolve(&b, &json_tables, &relationships, id)
                .ok_or_else(|| unresolved(id, *loc))?;
            assignments.push((t, classes, true));
        }
    }
    for (id, loc, styles) in &d.style_assigns {
        let t =
            resolve(&b, &json_tables, &relationships, id).ok_or_else(|| unresolved(id, *loc))?;
        assignments.push((t, styles, false));
    }
    for (t, values, is_class) in assignments {
        let (classes, styles) = match t {
            Target::Element(i) => {
                let s = &mut b.states[i];
                (&mut s.classes, &mut s.styles)
            }
            Target::Boundary(i) => {
                let s = &mut b.boundaries[i];
                (&mut s.classes, &mut s.styles)
            }
            Target::Json(i) => {
                let s = &mut json_tables[i];
                (&mut s.classes, &mut s.styles)
            }
            Target::Edge(i) => {
                let s = &mut relationships[i];
                (&mut s.classes, &mut s.styles)
            }
        };
        if is_class {
            push_unique(classes, values);
        } else {
            styles.extend(values.iter().cloned());
        }
    }

    // Final ordering: first occurrence anywhere in the source.
    let first = |id: &str| b.first.get(id).copied().unwrap_or(0);
    let mut states = b.states.clone();
    states.sort_by_key(|s| first(&s.id));
    let mut actors = Vec::new();
    let mut use_cases = Vec::new();
    for s in states {
        let parent = s.parent.map(|(p, _)| p);
        match s.kind {
            Kind::Actor => actors.push(Actor {
                kind: if s.icon.is_some() {
                    ActorType::Icon
                } else {
                    s.actor_type.unwrap_or(ActorType::Normal)
                },
                id: s.id,
                label: s.label,
                icon: s.icon,
                business: s.business.unwrap_or(false),
                stereotype: s.stereotype,
                parent,
                classes: s.classes,
                styles: s.styles,
            }),
            Kind::UseCase => use_cases.push(UseCase {
                id: s.id,
                label: s.label,
                shape: s.shape.unwrap_or(UseCaseShape::Ellipse),
                business: s.business.unwrap_or(false),
                stereotype: s.stereotype,
                parent,
                classes: s.classes,
                styles: s.styles,
            }),
        }
    }
    let mut order: Vec<usize> = (0..b.boundaries.len()).collect();
    order.sort_by_key(|&i| first(&b.boundaries[i].id));
    let boundaries = order
        .into_iter()
        .map(|i| {
            let x = &b.boundaries[i];
            SystemBoundary {
                id: x.id.clone(),
                label: x.label.clone(),
                kind: x.kind.unwrap_or(BoundaryType::Rect),
                members: members[i].clone(),
                classes: x.classes.clone(),
                styles: x.styles.clone(),
            }
        })
        .collect();
    json_tables.sort_by_key(|j| first(&j.id));

    Ok(UsecaseDiagram {
        direction: d.directions.last().copied().unwrap_or(Direction::LeftRight),
        actors,
        use_cases,
        boundaries,
        relationships,
        notes,
        json_tables,
        class_defs,
        title,
    })
}

/// Parse a `usecase-beta` definition (header line included).
pub fn parse(src: &str, title: Option<String>) -> Result<UsecaseDiagram> {
    let drafts = Parser::new(src)?.parse()?;
    finalize(drafts, title)
}

// ───────────────────────────── rendering ─────────────────────────────

/// `ACTOR_FIGURE_WIDTH` / `ACTOR_FIGURE_HEIGHT` / `ACTOR_LABEL_GAP` (`usecaseActor.ts`).
const FIGURE_W: f64 = 56.0;
const FIGURE_H: f64 = 72.0;
const LABEL_GAP: f64 = 8.0;
const STEREOTYPE_GAP: f64 = 2.0;
/// `usecaseDb.getData`: actor / rect / note / JSON padding 10, ellipse 20, boundary 20.
const NODE_PADDING: f64 = 10.0;
const BOUNDARY_PADDING: f64 = 20.0;
/// `usecaseJsonTable.ts` cell paddings.
const CELL_PAD_X: f64 = 8.0;
const CELL_PAD_Y: f64 = 4.0;
/// `.relationship-label, .edgeLabel { font-size: 10px }`.
const EDGE_LABEL_FONT: f64 = 10.0;

const STICK_FIGURE: &str = "M 0 -12 C 6.627 -12 12 -17.373 12 -24 C 12 -30.627 6.627 -36 0 -36 \
    C -6.627 -36 -12 -30.627 -12 -24 C -12 -17.373 -6.627 -12 0 -12 Z M 0 -12 V 8 M -17 -5 H 17 \
    M 0 8 L -15 28 M 0 8 L 15 28";
const HOLLOW_BODY: &str =
    "M -22 -10 H 22 V 0 H 6 L 22 17 L 13 28 L 0 13 L -13 28 L -22 17 L -6 0 H -22 Z";
const AWESOME_SILHOUETTE: &str = "M 0 -34 C 7.18 -34 13 -28.18 13 -21 C 13 -13.82 7.18 -8 0 -8 \
    C -7.18 -8 -13 -13.82 -13 -21 C -13 -28.18 -7.18 -34 0 -34 Z M -24 25 C -24 7 -14 -3 0 -3 \
    C 14 -3 24 7 24 25 C 24 28 21 30 18 30 H -18 C -21 30 -24 28 -24 25 Z";

/// Absolute `M`/`L`/`H`/`V`/`C`/`Z` path data, translated by `o`.
fn svg_path(d: &str, o: Point) -> Vec<PathCmd> {
    let mut cmds = Vec::new();
    let mut cur = Point::default();
    let mut op = 'M';
    let mut nums: Vec<f64> = Vec::new();
    let flush = |op: char, nums: &mut Vec<f64>, cur: &mut Point, cmds: &mut Vec<PathCmd>| {
        let p = |x: f64, y: f64| Point::new(x + o.x, y + o.y);
        match op {
            'M' | 'L' if nums.len() >= 2 => {
                *cur = Point::new(nums[0], nums[1]);
                cmds.push(if op == 'M' {
                    PathCmd::MoveTo(p(cur.x, cur.y))
                } else {
                    PathCmd::LineTo(p(cur.x, cur.y))
                });
            }
            'H' if !nums.is_empty() => {
                cur.x = nums[0];
                cmds.push(PathCmd::LineTo(p(cur.x, cur.y)));
            }
            'V' if !nums.is_empty() => {
                cur.y = nums[0];
                cmds.push(PathCmd::LineTo(p(cur.x, cur.y)));
            }
            'C' if nums.len() >= 6 => {
                *cur = Point::new(nums[4], nums[5]);
                cmds.push(PathCmd::CubicTo(
                    p(nums[0], nums[1]),
                    p(nums[2], nums[3]),
                    p(cur.x, cur.y),
                ));
            }
            _ => {}
        }
        nums.clear();
    };
    for tok in d.split_whitespace() {
        match tok.parse::<f64>() {
            Ok(n) => nums.push(n),
            Err(_) => {
                flush(op, &mut nums, &mut cur, &mut cmds);
                op = tok.chars().next().unwrap_or('M');
                if op == 'Z' {
                    cmds.push(PathCmd::Close);
                }
            }
        }
    }
    flush(op, &mut nums, &mut cur, &mut cmds);
    cmds
}

/// `businessMarkerPathForCircle`: a chord across the actor's head.
fn business_marker(center_y: f64, radius: f64) -> (Point, Point) {
    let offset = radius * 0.6;
    let half = radius * (1.0 - 0.36f64).sqrt();
    let angle = std::f64::consts::PI / 3.0;
    let (dx, dy) = (angle.cos(), -angle.sin());
    let cx = offset * -dy;
    let cy = center_y + offset * dx;
    (
        Point::new(cx - half * dx, cy - half * dy),
        Point::new(cx + half * dx, cy + half * dy),
    )
}

/// One colour per kind of element (`roleColors` in `usecase/styles.ts`).
#[derive(Debug, Clone, PartialEq)]
struct RoleColors {
    actor_bkg: Color,
    actor_border: Color,
    bkg: Color,
    border: Color,
    boundary_bkg: Color,
    boundary_border: Color,
    include_line: Color,
    extend_line: Color,
}

fn role_colors(theme: ThemeName, vars: &[(String, String)], pal: &Palette) -> RoleColors {
    // The usecase tokens the colour themes declare (`theme-redux-color.js`,
    // `theme-redux-dark-color.js`).
    let declared: &[(&str, &str)] = match theme {
        ThemeName::ReduxColor => &[
            ("usecaseActorBorder", "#A78BFA"),
            ("usecaseActorBkg", "#F5F3FF"),
            ("usecaseBorder", "#2DD4BF"),
            ("usecaseBkg", "#F0FDFA"),
            ("usecaseBoundaryBorder", "#BDBCCC"),
            ("usecaseBoundaryBkg", "#FAFAFC"),
            ("usecaseIncludeLine", "#38BDF8"),
            ("usecaseExtendLine", "#FB923C"),
        ],
        ThemeName::ReduxDarkColor => &[
            ("usecaseActorBorder", "#A78BFA"),
            ("usecaseBorder", "#2DD4BF"),
            ("usecaseBoundaryBorder", "#BDBCCC"),
            ("usecaseIncludeLine", "#38BDF8"),
            ("usecaseExtendLine", "#FB923C"),
        ],
        _ => &[],
    };
    let var = |key: &str, fallback: &Color| -> Color {
        vars.iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| Color::new(v.trim()))
            .or_else(|| {
                declared
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| Color::new(*v))
            })
            .unwrap_or_else(|| fallback.clone())
    };
    // Themes whose `actorBorder` is their primary border colour (`border1`).
    let actor_border = match theme {
        ThemeName::Forest | ThemeName::Neutral => &pal.actor_border,
        _ => &pal.node_border,
    };
    RoleColors {
        actor_bkg: var("usecaseActorBkg", &pal.actor_bkg),
        actor_border: var("usecaseActorBorder", actor_border),
        bkg: var("usecaseBkg", &pal.main_bkg),
        border: var("usecaseBorder", &pal.node_border),
        boundary_bkg: var("usecaseBoundaryBkg", &pal.cluster_bkg),
        boundary_border: var("usecaseBoundaryBorder", &pal.cluster_border),
        include_line: var("usecaseIncludeLine", &pal.line_color),
        extend_line: var("usecaseExtendLine", &pal.line_color),
    }
}

/// Resolved `usecase` configuration.
struct Options {
    look: Look,
    actor_font: f64,
    actor_bold: bool,
    usecase_font: f64,
    usecase_bold: bool,
    node_spacing: f64,
    rank_spacing: f64,
    padding: f64,
    wrap: f64,
    min_width: f64,
    rotate: bool,
    curve: Curve,
    role: RoleColors,
}

fn is_bold(weight: Option<&str>) -> bool {
    weight
        .is_some_and(|w| matches!(w, "bold" | "bolder") || w.parse::<u32>().is_ok_and(|n| n >= 600))
}

impl Options {
    fn new(cfg: &MermaidConfig, pal: &Palette) -> Self {
        let num = |k: &str, d: f64| cfg.section_f64("usecase", k, d);
        let s = |k: &str| cfg.section("usecase", k).and_then(Json::as_str);
        Options {
            look: cfg.look,
            actor_font: num("actorFontSize", 14.0),
            actor_bold: is_bold(s("actorFontWeight")),
            usecase_font: num("usecaseFontSize", 12.0),
            usecase_bold: is_bold(s("usecaseFontWeight")),
            node_spacing: num("nodeSpacing", 50.0),
            rank_spacing: num("rankSpacing", 50.0),
            padding: num("diagramPadding", 20.0),
            wrap: num("wrappingWidth", 120.0),
            min_width: num("minNodeWidth", 120.0),
            rotate: s("colorScheme") == Some("rotate"),
            curve: cfg.flowchart.curve,
            role: role_colors(cfg.theme, &cfg.theme_variables, pal),
        }
    }
    fn neo(&self) -> bool {
        self.look == Look::Neo
    }
}

/// Display lines of a label: entity codes decoded; Markdown strings lose their markers
/// and keep their physical line breaks.
fn label_lines(label: &Label) -> Vec<String> {
    match label.kind {
        LabelKind::Text => vec![decode_entities(label.text.trim())],
        LabelKind::Markdown => normalize_label(&format!("`{}`", label.text))
            .split('\n')
            .map(|l| l.trim().to_string())
            .collect(),
    }
}

/// Greedy word wrap of every line to `max` pixels (the label `max-width`).
fn wrap_lines(lines: &[String], font: f64, bold: bool, max: f64) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        let mut cur = String::new();
        for word in line.split_whitespace() {
            let cand = if cur.is_empty() {
                word.to_string()
            } else {
                format!("{cur} {word}")
            };
            if !cur.is_empty() && line_width(&cand, font, bold) > max {
                out.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = cand;
            }
        }
        out.push(cur);
    }
    out
}

/// A measured text block.
#[derive(Debug, Clone)]
struct Block {
    lines: Vec<String>,
    font: f64,
    bold: bool,
    w: f64,
    h: f64,
}

impl Block {
    fn new(lines: Vec<String>, font: f64, bold: bool) -> Block {
        let w = lines
            .iter()
            .map(|l| line_width(l, font, bold))
            .fold(0.0, f64::max);
        let h = if lines.iter().all(|l| l.trim().is_empty()) {
            0.0
        } else {
            lines.len() as f64 * font * 1.5
        };
        Block {
            lines,
            font,
            bold,
            w,
            h,
        }
    }
    fn wrapped(lines: &[String], font: f64, bold: bool, max: f64) -> Block {
        Block::new(wrap_lines(lines, font, bold, max), font, bold)
    }
    fn text(&self, center: Point, color: &Color) -> Element {
        let mut t = Text::centered(center, &self.lines, self.font, self.font * 1.5, color);
        t.bold = self.bold;
        Element::text(t)
    }
}

#[derive(Debug, Clone)]
enum Shape {
    Actor {
        kind: ActorType,
        business: bool,
        label: Block,
        stereotype: Option<Block>,
    },
    Ellipse {
        label: Block,
        rx: f64,
        ry: f64,
    },
    Business {
        label: Block,
        stereotype: Option<Block>,
        label_w: f64,
        label_h: f64,
        rx: f64,
        ry: f64,
    },
    Rect {
        label: Block,
    },
    Note {
        label: Block,
    },
    Json {
        title: Block,
        rows: Vec<(Block, Block)>,
        key_w: f64,
        value_w: f64,
        title_h: f64,
        row_h: Vec<f64>,
        border: f64,
    },
}

/// A measured node, ready for layout.
#[derive(Debug, Clone)]
struct VNode {
    id: String,
    kind: &'static str,
    shape: Shape,
    w: f64,
    h: f64,
    cluster: Option<usize>,
    style: StyleOverrides,
    slot: Option<SlotColors>,
}

/// `getCompiledStyles` (the `default` class, then the named classes, later declarations
/// winning per property) under the inline `style` declarations.
fn compiled_style(
    class_defs: &[(String, Vec<String>)],
    classes: &[String],
    inline: &[String],
) -> StyleOverrides {
    let mut compiled: Vec<(String, String)> = Vec::new();
    for name in std::iter::once("default").chain(classes.iter().map(String::as_str)) {
        let Some((_, styles)) = class_defs.iter().find(|(n, _)| n == name) else {
            continue;
        };
        for s in styles {
            let s = s.trim();
            let prop = s.split(':').next().unwrap_or("").trim().to_string();
            if prop.is_empty() {
                continue;
            }
            match compiled.iter_mut().find(|(p, _)| *p == prop) {
                Some(slot) => slot.1 = s.to_string(),
                None => compiled.push((prop, s.to_string())),
            }
        }
    }
    let joined: Vec<&str> = compiled.iter().map(|(_, s)| s.as_str()).collect();
    StyleOverrides::parse(&joined.join(",")).merged(&StyleOverrides::parse(&inline.join(",")))
}

fn measure_actor(a: &Actor, o: &Options) -> (Shape, f64, f64) {
    let label = Block::wrapped(&label_lines(&a.label), o.actor_font, o.actor_bold, o.wrap);
    let stereotype = a.stereotype.as_ref().map(|s| {
        Block::wrapped(
            &[format!("«{}»", decode_entities(s))],
            o.actor_font,
            o.actor_bold,
            o.wrap,
        )
    });
    let sh = stereotype.as_ref().map_or(0.0, |s| s.h + STEREOTYPE_GAP);
    let content_h = FIGURE_H + LABEL_GAP + label.h + sh;
    let w = FIGURE_W
        .max(label.w)
        .max(stereotype.as_ref().map_or(0.0, |s| s.w))
        + 2.0 * NODE_PADDING;
    let h = content_h + 2.0 * NODE_PADDING;
    (
        Shape::Actor {
            kind: a.kind,
            business: a.business,
            label,
            stereotype,
        },
        w,
        h,
    )
}

fn measure_use_case(u: &UseCase, o: &Options) -> (Shape, f64, f64) {
    let (font, bold) = (o.usecase_font, o.usecase_bold);
    let text = label_lines(&u.label);
    if u.business && u.shape == UseCaseShape::Ellipse {
        // usecaseBusiness.ts: the stereotype keeps its own label above the main one.
        let label = Block::wrapped(&text, font, bold, o.wrap);
        let stereotype = u
            .stereotype
            .as_ref()
            .map(|s| Block::wrapped(&[format!("«{}»", decode_entities(s))], font, bold, o.wrap));
        let label_w = label
            .w
            .max(o.min_width)
            .max(stereotype.as_ref().map_or(0.0, |s| s.w));
        let label_h = label.h + stereotype.as_ref().map_or(0.0, |s| s.h + STEREOTYPE_GAP);
        let (rx, ry) = (label_w / 2.0 + NODE_PADDING, label_h / 2.0 + NODE_PADDING);
        return (
            Shape::Business {
                label,
                stereotype,
                label_w,
                label_h,
                rx,
                ry,
            },
            2.0 * rx,
            2.0 * ry,
        );
    }
    // `prepareUsecaseLayoutData` folds the stereotype into the label: «S»<br/>label.
    let mut lines = Vec::new();
    if let Some(s) = &u.stereotype {
        lines.push(format!("«{}»", decode_entities(s)));
    }
    lines.extend(text);
    let label = Block::wrapped(&lines, font, bold, o.wrap);
    let lw = label.w.max(o.min_width);
    let lh = label.h.max(font * 1.5);
    match u.shape {
        UseCaseShape::Ellipse => {
            // usecaseEllipse.ts: radius = label / 2 + 2 · halfPadding (padding 20).
            let (rx, ry) = (lw / 2.0 + 20.0, lh / 2.0 + 20.0);
            (Shape::Ellipse { label, rx, ry }, 2.0 * rx, 2.0 * ry)
        }
        UseCaseShape::Rect => {
            let m = ShapeMetrics::for_look(super::graph::Shape::Rect, NODE_PADDING, o.look);
            (Shape::Rect { label }, lw + m.pad.x, lh + m.pad.y)
        }
    }
}

fn measure_note(n: &Note, o: &Options, font: f64) -> (Shape, f64, f64) {
    let label = Block::wrapped(&label_lines(&n.label), font, false, o.wrap);
    let w = label.w.max(o.min_width) + 2.0 * NODE_PADDING;
    let h = label.h.max(font * 1.5) + 2.0 * NODE_PADDING;
    (Shape::Note { label }, w, h)
}

fn measure_json(
    j: &JsonTable,
    o: &Options,
    font: f64,
    style: &StyleOverrides,
) -> (Shape, f64, f64) {
    let title = Block::new(vec![j.id.clone()], font, false);
    let cell = |s: &str| Block::new(vec![s.to_string()], font, false);
    let rows: Vec<(Block, Block)> = j
        .rows
        .iter()
        .map(|r| (cell(&r.key), cell(&r.value)))
        .collect();
    let key_w = rows.iter().map(|(k, _)| k.w).fold(0.0, f64::max) + 2.0 * CELL_PAD_X;
    let measured_value = rows.iter().map(|(_, v)| v.w).fold(0.0, f64::max) + 2.0 * CELL_PAD_X;
    let title_w = title.w.max(o.min_width) + 2.0 * CELL_PAD_X;
    let inner_w = title_w.max(key_w + measured_value);
    let value_w = inner_w - key_w;
    let line = font * 1.5;
    let title_h = title.h.max(line) + 2.0 * CELL_PAD_Y;
    let row_h: Vec<f64> = rows
        .iter()
        .map(|(k, v)| k.h.max(v.h).max(line) + 2.0 * CELL_PAD_Y)
        .collect();
    let inner_h = title_h + row_h.iter().sum::<f64>();
    let border = style.stroke_width.filter(|w| *w >= 0.0).unwrap_or(1.0);
    (
        Shape::Json {
            title,
            rows,
            key_w,
            value_w,
            title_h,
            row_h,
            border,
        },
        inner_w + 2.0 * border,
        inner_h + 2.0 * border,
    )
}

/// The neo look's drop shadow under a filled shape.
fn shadow(geom: &Geometry, sh: Option<&DropShadow>) -> Option<Element> {
    let sh = sh?;
    Some(Element::classed_shape(
        geom.translated(sh.dx, sh.dy),
        Paint::fill(&sh.color).with_opacity(sh.opacity),
        "drop-shadow",
    ))
}

fn painted(
    style: &StyleOverrides,
    slot: Option<&SlotColors>,
    fill: &Color,
    stroke: &Color,
    width: f64,
) -> Paint {
    let fill = style
        .fill
        .clone()
        .or_else(|| slot.and_then(|s| s.fill.clone()))
        .unwrap_or_else(|| fill.clone());
    let stroke = style
        .stroke
        .clone()
        .or_else(|| slot.map(|s| s.border.clone()))
        .unwrap_or_else(|| stroke.clone());
    let mut p = Paint::both(&fill, &stroke, style.stroke_width.unwrap_or(width));
    p.dash = style.dash.clone();
    p
}

/// Draw one node centred on `c`; returns its elements and its clipping boundary.
fn draw_node(
    n: &VNode,
    c: Point,
    o: &Options,
    pal: &Palette,
    sh: Option<&DropShadow>,
) -> (Vec<Element>, Clip) {
    let mut els = Vec::new();
    let r = Rect::centered(c, n.w, n.h);
    let slot = n.slot.as_ref();
    let role = &o.role;
    match &n.shape {
        Shape::Actor {
            kind,
            business,
            label,
            stereotype,
        } => {
            let paint = painted(&n.style, slot, &role.actor_bkg, &role.actor_border, 2.0);
            let line = Paint::stroke(&paint.stroke, paint.stroke_width);
            let content_h = n.h - 2.0 * NODE_PADDING;
            let top = c.y - content_h / 2.0;
            let g = Point::new(c.x, top + FIGURE_H / 2.0);
            match kind {
                ActorType::Normal => els.push(Element::classed_shape(
                    Geometry::Path(svg_path(STICK_FIGURE, g)),
                    paint.clone(),
                    "usecase-actor-stick",
                )),
                ActorType::Hollow => {
                    els.push(Element::classed_shape(
                        Geometry::Circle {
                            c: Point::new(g.x, g.y - 23.0),
                            r: 9.0,
                        },
                        line.clone(),
                        "usecase-actor-hollow-head",
                    ));
                    els.push(Element::classed_shape(
                        Geometry::Path(svg_path(HOLLOW_BODY, g)),
                        line.clone(),
                        "usecase-actor-hollow-body",
                    ));
                }
                ActorType::Awesome => els.push(Element::classed_shape(
                    Geometry::Path(svg_path(AWESOME_SILHOUETTE, g)),
                    paint.clone(),
                    "usecase-actor-awesome-silhouette",
                )),
                ActorType::Icon => {
                    // Icon packs are not bundled: draw mermaid's unknown-icon fallback.
                    els.push(Element::classed_shape(
                        Geometry::Rect {
                            rect: Rect::new(g.x - 26.0, g.y - 28.0, 52.0, 52.0),
                            rx: 4.0,
                        },
                        paint.clone(),
                        "usecase-actor-icon-frame",
                    ));
                    els.push(Element::classed_shape(
                        Geometry::Rect {
                            rect: Rect::new(g.x - 21.0, g.y - 23.0, 42.0, 42.0),
                            rx: 0.0,
                        },
                        Paint::fill(&Color::new("#087ebf")),
                        "usecase-actor-icon-fallback",
                    ));
                    let q = Block::new(vec!["?".into()], 35.57, false);
                    els.push(q.text(Point::new(g.x, g.y - 1.5), &Color::new("#fff")));
                }
            }
            if *business {
                let (a, b) = match kind {
                    ActorType::Normal => business_marker(-24.0, 12.0),
                    ActorType::Hollow => business_marker(-23.0, 9.0),
                    ActorType::Awesome => business_marker(-21.0, 13.0),
                    ActorType::Icon => (Point::new(12.0, -8.0), Point::new(26.0, -26.0)),
                };
                els.push(Element::classed_shape(
                    Geometry::Path(vec![PathCmd::MoveTo(g.plus(a)), PathCmd::LineTo(g.plus(b))]),
                    line,
                    "usecase-business-marker",
                ));
            }
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.actor_text.clone());
            let mut y = top + FIGURE_H + LABEL_GAP;
            if let Some(s) = stereotype {
                els.push(s.text(Point::new(c.x, y + s.h / 2.0), &color));
                y += s.h + STEREOTYPE_GAP;
            }
            els.push(label.text(Point::new(c.x, y + label.h / 2.0), &color));
            (els, Clip::Rect(r))
        }
        Shape::Ellipse { label, rx, ry } => {
            let paint = painted(&n.style, slot, &role.bkg, &role.border, 2.0);
            els.push(Element::classed_shape(
                Geometry::Path(geom::ellipse(c, *rx, *ry)),
                paint,
                "basic label-container",
            ));
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.node_text.clone());
            els.push(label.text(c, &color));
            (els, Clip::Ellipse(c, *rx, *ry))
        }
        Shape::Business {
            label,
            stereotype,
            label_w,
            label_h,
            rx,
            ry,
        } => {
            let paint = painted(&n.style, slot, &role.bkg, &role.border, 2.0);
            let line = Paint::stroke(&paint.stroke, paint.stroke_width);
            els.push(Element::classed_shape(
                Geometry::Path(geom::ellipse(c, *rx, *ry)),
                paint,
                "basic label-container usecase-business-ellipse",
            ));
            let inset = (NODE_PADDING / 5.0).clamp(1.0, NODE_PADDING / 2.0);
            let (sx, ex) = (label_w / 2.0 + inset, rx - inset);
            let on = |x: f64| ry * (1.0 - (x / rx).powi(2)).max(0.0).sqrt();
            els.push(Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(c.x + sx, c.y + on(sx))),
                    PathCmd::LineTo(Point::new(c.x + ex, c.y - on(ex))),
                ]),
                line,
                "usecase-business-marker",
            ));
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.node_text.clone());
            if let Some(s) = stereotype {
                els.push(s.text(Point::new(c.x, c.y - label_h / 2.0 + s.h / 2.0), &color));
            }
            els.push(label.text(Point::new(c.x, c.y + label_h / 2.0 - label.h / 2.0), &color));
            (els, Clip::Ellipse(c, *rx, *ry))
        }
        Shape::Rect { label } => {
            let paint = painted(&n.style, slot, &role.bkg, &role.border, 2.0);
            let geom = Geometry::Rect { rect: r, rx: 0.0 };
            els.extend(shadow(&geom, sh));
            els.push(Element::classed_shape(geom, paint, "basic label-container"));
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.node_text.clone());
            els.push(label.text(c, &color));
            (els, Clip::Rect(r))
        }
        Shape::Note { label } => {
            // Under neo the `.node path` rule repaints the note outline with nodeBorder.
            let (stroke, width) = if o.neo() {
                (&pal.node_border, pal.stroke_width.unwrap_or(1.0))
            } else {
                (&pal.note_border, 1.0)
            };
            let paint = painted(&n.style, None, &pal.note_bkg, stroke, width);
            let geom = Geometry::Rect { rect: r, rx: 0.0 };
            els.extend(shadow(&geom, sh));
            els.push(Element::classed_shape(geom, paint, "basic label-container"));
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.note_text.clone());
            els.push(label.text(c, &color));
            (els, Clip::Rect(r))
        }
        Shape::Json {
            title,
            rows,
            key_w,
            value_w,
            title_h,
            row_h,
            border,
        } => {
            let mut paint = painted(&n.style, None, &pal.main_bkg, &pal.node_border, 1.0);
            paint.stroke_width = *border;
            let outer = Geometry::Rect { rect: r, rx: 0.0 };
            els.extend(shadow(&outer, sh));
            els.push(Element::classed_shape(
                outer,
                paint.clone(),
                "label-container usecase-json-border",
            ));
            let color = n
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.node_text.clone());
            let inner_w = key_w + value_w;
            let (left, mut y) = (r.x + border, r.y + border);
            let cell = |x: f64, y: f64, w: f64, h: f64| {
                Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::new(x, y, w, h),
                        rx: 0.0,
                    },
                    paint.clone(),
                    "usecase-json-cell",
                )
            };
            els.push(cell(left, y, inner_w, *title_h));
            els.push(title.text(Point::new(left + inner_w / 2.0, y + title_h / 2.0), &color));
            y += title_h;
            for ((k, v), h) in rows.iter().zip(row_h) {
                els.push(cell(left, y, *key_w, *h));
                els.push(cell(left + key_w, y, *value_w, *h));
                let cy = y + h / 2.0;
                els.push(k.text(Point::new(left + key_w / 2.0, cy), &color));
                els.push(v.text(Point::new(left + key_w + value_w / 2.0, cy), &color));
                y += h;
            }
            (els, Clip::Rect(r))
        }
    }
}

/// Measured system boundary title.
struct VCluster {
    id: String,
    kind: BoundaryType,
    title: Block,
    style: StyleOverrides,
    slot: Option<SlotColors>,
}

/// Parse, lay out and draw a `usecase` diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    Ok(draw(&d, &Options::new(cfg, pal), pal))
}

fn draw(d: &UsecaseDiagram, o: &Options, pal: &Palette) -> Scene {
    let font = pal.font_size;
    let shadow_of = pal.drop_shadow.as_ref().filter(|_| o.neo());
    let palette_slot = |i: usize| pal.color_slots.as_ref().map(|p| p.slot(ColorSlot(i)));
    let boundary_index: HashMap<&str, usize> = d
        .boundaries
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.as_str(), i))
        .collect();
    let cluster_of = |p: &Option<String>| p.as_deref().and_then(|p| boundary_index.get(p).copied());

    // Nodes in `getData` order: actors, use cases, notes, JSON tables.
    let mut nodes: Vec<VNode> = Vec::new();
    let mut color_index = 0;
    let mut next_slot = || {
        color_index += 1;
        if o.rotate {
            palette_slot(color_index - 1)
        } else {
            None
        }
    };
    for a in &d.actors {
        let (shape, w, h) = measure_actor(a, o);
        nodes.push(VNode {
            id: a.id.clone(),
            kind: "actor",
            shape,
            w,
            h,
            cluster: cluster_of(&a.parent),
            style: compiled_style(&d.class_defs, &a.classes, &a.styles),
            slot: next_slot(),
        });
    }
    for u in &d.use_cases {
        let (shape, w, h) = measure_use_case(u, o);
        nodes.push(VNode {
            id: u.id.clone(),
            kind: "usecase",
            shape,
            w,
            h,
            cluster: cluster_of(&u.parent),
            style: compiled_style(&d.class_defs, &u.classes, &u.styles),
            slot: next_slot(),
        });
    }
    for n in &d.notes {
        let (shape, w, h) = measure_note(n, o, font);
        nodes.push(VNode {
            id: n.id.clone(),
            kind: "note",
            shape,
            w,
            h,
            cluster: None,
            style: compiled_style(&d.class_defs, &[], &[]),
            slot: None,
        });
    }
    for j in &d.json_tables {
        let style = compiled_style(&d.class_defs, &j.classes, &j.styles);
        let (shape, w, h) = measure_json(j, o, font, &style);
        nodes.push(VNode {
            id: j.id.clone(),
            kind: "json",
            shape,
            w,
            h,
            cluster: None,
            style,
            slot: None,
        });
    }
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let clusters: Vec<VCluster> = d
        .boundaries
        .iter()
        .enumerate()
        .map(|(i, b)| VCluster {
            id: b.id.clone(),
            kind: b.kind,
            title: Block::new(label_lines(&b.label), font, false),
            style: compiled_style(&d.class_defs, &b.classes, &b.styles),
            slot: palette_slot(i),
        })
        .collect();

    // Relationships, then the note connectors.
    struct VEdge {
        from: usize,
        to: usize,
        label: Option<Block>,
        minlen: usize,
        kind: Option<RelationshipType>,
        start: Head,
        end: Head,
        style: StyleOverrides,
        id: String,
    }
    let mut edges: Vec<VEdge> = Vec::new();
    for r in &d.relationships {
        let (Some(&from), Some(&to)) = (index.get(r.source.as_str()), index.get(r.target.as_str()))
        else {
            continue;
        };
        let (start, end) = match r.kind {
            RelationshipType::Include | RelationshipType::Extend => (Head::None, Head::Arrow),
            RelationshipType::Generalization => (Head::None, Head::Triangle),
            RelationshipType::Association => match r.arrow {
                ArrowType::SolidArrow => (Head::None, Head::Arrow),
                ArrowType::BackArrow => (Head::Arrow, Head::None),
                ArrowType::LineSolid => (Head::None, Head::None),
                ArrowType::CircleArrow => (Head::None, Head::Circle),
                ArrowType::CrossArrow => (Head::None, Head::Cross),
                ArrowType::CircleArrowReversed => (Head::Circle, Head::None),
                ArrowType::CrossArrowReversed => (Head::Cross, Head::None),
            },
        };
        let text = match r.kind {
            RelationshipType::Include => Some(vec!["«include»".to_string()]),
            RelationshipType::Extend => Some(vec!["«extend»".to_string()]),
            RelationshipType::Generalization => None,
            RelationshipType::Association => r.label.as_ref().map(label_lines),
        };
        edges.push(VEdge {
            from,
            to,
            label: text
                .map(|t| Block::new(t, EDGE_LABEL_FONT, false))
                .filter(|b| b.h > 0.0),
            minlen: r.minlen,
            kind: Some(r.kind),
            start,
            end,
            style: compiled_style(&d.class_defs, &r.classes, &r.styles),
            id: r.id.clone(),
        });
    }
    for n in &d.notes {
        if let (Some(&from), Some(&to)) = (index.get(n.id.as_str()), index.get(n.target.as_str())) {
            edges.push(VEdge {
                from,
                to,
                label: None,
                minlen: 1,
                kind: None,
                start: Head::None,
                end: Head::None,
                style: compiled_style(&d.class_defs, &[], &[]),
                id: format!("{}-edge", n.id),
            });
        }
    }

    let title_band = |c: &VCluster| match c.kind {
        BoundaryType::Rect => c.title.h + 8.0,
        BoundaryType::Package => c.title.h + 10.0 + 8.0,
    };
    let input = LayoutInput {
        nodes: nodes
            .iter()
            .map(|n| LayoutNode {
                width: n.w,
                height: n.h,
                cluster: n.cluster,
            })
            .collect(),
        edges: edges
            .iter()
            .map(|e| LayoutEdge {
                from: e.from,
                to: e.to,
                minlen: e.minlen,
                label: e.label.as_ref().map(|b| (b.w + 8.0, b.h + 4.0)),
            })
            .collect(),
        clusters: clusters
            .iter()
            .map(|c| LayoutCluster {
                parent: None,
                title_height: title_band(c),
            })
            .collect(),
        direction: d.direction,
        node_sep: o.node_spacing,
        rank_sep: o.rank_spacing,
        edge_sep: 20.0,
        cluster_padding: BOUNDARY_PADDING,
    };
    let out = layout::layout(&input);

    let mut bounds = Bounds::default();

    // System boundaries (`usecaseSystemBoundary` in clusters.js).
    let mut cluster_els = Vec::new();
    for (ci, c) in clusters.iter().enumerate() {
        let lr = out.clusters[ci];
        let package = c.kind == BoundaryType::Package;
        let label_pad = if package { 20.0 } else { BOUNDARY_PADDING };
        let tab_h = if package { c.title.h + 10.0 } else { 0.0 };
        let w = lr.w.max(c.title.w + label_pad);
        let h = if package {
            lr.h.max(tab_h + 2.0 * BOUNDARY_PADDING)
        } else {
            lr.h
        };
        let r = Rect::centered(lr.center(), w, h);
        bounds.add_rect(r);
        let paint = painted(
            &c.style,
            c.slot.as_ref(),
            &o.role.boundary_bkg,
            &o.role.boundary_border,
            1.0,
        );
        let mut children = Vec::new();
        let body = Geometry::Rect {
            rect: Rect::new(r.x, r.y + tab_h, w, h - tab_h),
            rx: 0.0,
        };
        let title_color = c
            .style
            .color
            .clone()
            .unwrap_or_else(|| pal.title_color.clone());
        if package {
            let tab_w = w.min((c.title.w + 20.0).max(80.0));
            let tab = Geometry::Rect {
                rect: Rect::new(r.x, r.y, tab_w, tab_h),
                rx: 0.0,
            };
            children.extend(shadow(&tab, shadow_of));
            children.push(Element::classed_shape(
                tab,
                paint.clone(),
                "boundary-tab system-boundary-package-tab",
            ));
            children.extend(shadow(&body, shadow_of));
            children.push(Element::classed_shape(body, paint, "boundary-body"));
            children.push(c.title.text(
                Point::new(r.x + tab_w / 2.0, r.y + tab_h / 2.0),
                &title_color,
            ));
        } else {
            children.extend(shadow(&body, shadow_of));
            children.push(Element::classed_shape(body, paint, "boundary-body"));
            children.push(c.title.text(
                Point::new(r.center().x, r.y + c.title.h / 2.0),
                &title_color,
            ));
        }
        cluster_els.push(Element::Group {
            id: Some(format!("usecase-{}", c.id)),
            class: Some(format!(
                "cluster usecase-system-boundary system-boundary system-boundary-{}",
                if package { "package" } else { "rect" }
            )),
            children,
        });
    }

    // Nodes.
    let mut node_els = Vec::new();
    let mut clips = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let c = out.nodes[i];
        let (els, clip) = draw_node(n, c, o, pal, shadow_of);
        bounds.add_rect(Rect::centered(c, n.w, n.h));
        clips.push(clip);
        node_els.push(Element::Group {
            id: Some(format!("usecase-{}", n.id)),
            class: Some(format!("node default usecase-{}", n.kind)),
            children: els,
        });
    }

    // Edges.
    let neo = o.neo();
    let edge_width = if neo {
        pal.stroke_width.unwrap_or(1.0)
    } else {
        1.0
    };
    let mut edge_els = Vec::new();
    let mut label_els = Vec::new();
    for (e, route) in edges.iter().zip(&out.edges) {
        let mut pts = route.points.clone();
        if pts.len() < 2 {
            continue;
        }
        if e.from != e.to {
            pts[0] = clips[e.from].clip(pts[1]);
            let last = pts.len() - 1;
            pts[last] = clips[e.to].clip(pts[last - 1]);
        }
        let line_color = match e.kind {
            Some(RelationshipType::Include) => &o.role.include_line,
            Some(RelationshipType::Extend) => &o.role.extend_line,
            _ => &pal.line_color,
        };
        let stroke = e.style.stroke.clone().unwrap_or_else(|| line_color.clone());
        let width = e.style.stroke_width.unwrap_or(edge_width);
        let marker_color = e
            .style
            .stroke
            .clone()
            .unwrap_or_else(|| pal.line_color.clone());
        let colors = MarkerColors {
            line: &marker_color,
            background: &pal.main_bkg,
        };
        let last = pts.len() - 1;
        let end_dir = pts[last].minus(pts[last - 1]).norm();
        let start_dir = pts[0].minus(pts[1]).norm();
        let mut heads = markers::elements(e.end, End::End, pts[last], end_dir, colors, width, neo);
        heads.extend(markers::elements(
            e.start,
            End::Start,
            pts[0],
            start_dir,
            colors,
            width,
            neo,
        ));
        let (end_back, start_back) = (markers::pullback(e.end), markers::pullback(e.start));
        if end_back > 0.0 && pts[last].dist(pts[last - 1]) > end_back {
            pts[last] = pts[last].minus(end_dir.scale(end_back));
        }
        if start_back > 0.0 && pts[0].dist(pts[1]) > start_back {
            pts[0] = pts[0].minus(start_dir.scale(start_back));
        }
        for p in &pts {
            bounds.add_point(*p);
        }
        let cmds = match o.curve {
            Curve::Linear | Curve::Step => geom::polyline(&pts),
            Curve::Basis => geom::basis_curve(&pts),
        };
        let mut paint = Paint::stroke(&stroke, width);
        let dotted = !matches!(
            e.kind,
            Some(RelationshipType::Association | RelationshipType::Generalization)
        );
        paint.dash = e
            .style
            .dash
            .clone()
            .or_else(|| dotted.then(|| vec![if neo { 2.0 } else { 3.0 }]));
        let kind = match e.kind {
            Some(RelationshipType::Association) => "association",
            Some(RelationshipType::Include) => "include",
            Some(RelationshipType::Extend) => "extend",
            Some(RelationshipType::Generalization) => "generalization",
            None => "note",
        };
        let mut children = vec![Element::classed_shape(
            Geometry::Path(cmds),
            paint,
            format!("relationship relationship-{kind}"),
        )];
        children.extend(heads);
        edge_els.push(Element::Group {
            id: Some(format!("usecase-{}", e.id)),
            class: Some("edgePath".into()),
            children,
        });
        if let (Some(b), Some(at)) = (&e.label, route.label) {
            let r = Rect::centered(at, b.w + 4.0, b.h);
            bounds.add_rect(r);
            let color = e
                .style
                .color
                .clone()
                .unwrap_or_else(|| pal.node_text.clone());
            label_els.push(Element::group(
                "edgeLabel",
                vec![
                    Element::rect(r, 0.0, Paint::fill(&pal.edge_label_background)),
                    b.text(at, &color),
                ],
            ));
        }
    }

    let mut elements = vec![
        Element::group("clusters", cluster_els),
        Element::group("edgePaths", edge_els),
        Element::group("edgeLabels", label_els),
        Element::group("nodes", node_els),
    ];
    if let Some(t) = d.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = Block::new(vec![t.clone()], font * 1.125, false);
        let center = Point::new(content.x + content.w / 2.0, content.y - tb.h / 2.0 - 10.0);
        bounds.add_rect(Rect::centered(center, tb.w, tb.h));
        let mut text = Text::centered(center, &tb.lines, tb.font, tb.font * 1.5, &pal.title_color);
        text.bold = false;
        elements.push(Element::classed_text(text, "usecaseDiagramTitleText"));
    }
    Scene {
        kind: DiagramKind::Usecase,
        view_box: bounds.rect().pad(o.padding),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(body: &str) -> UsecaseDiagram {
        parse(&format!("usecase-beta\n{body}"), None).unwrap()
    }
    fn err(body: &str) -> String {
        match parse(&format!("usecase-beta\n{body}"), None) {
            Err(Error::Parse { message, .. }) => message,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_and_final_statement_without_newline() {
        let d = p("");
        assert!(d.actors.is_empty() && d.use_cases.is_empty());
        assert_eq!(d.direction, Direction::LeftRight);
        let d = p("actor A\nA --> B");
        assert_eq!(d.relationships.len(), 1);
    }

    #[test]
    fn unresolved_endpoints_become_ellipse_use_cases() {
        let d = p("Left --> Right");
        assert!(d.actors.is_empty());
        let ids: Vec<_> = d.use_cases.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["Left", "Right"]);
        assert!(d.use_cases.iter().all(|u| u.shape == UseCaseShape::Ellipse));
        assert_eq!(d.use_cases[0].label, Label::text("Left"));
    }

    #[test]
    fn actors_are_never_inferred_and_order_follows_first_occurrence() {
        let d = p(
            "Customer --> Login\nactor Customer\nactor Admin(\"Main administrator\")\nLogin(\"Sign in\")\nReport[Generate report]\n\"Reset password\"\nAdmin --> Report",
        );
        let actors: Vec<_> = d.actors.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(actors, ["Customer", "Admin"]);
        assert_eq!(d.actors[1].label.text, "Main administrator");
        let ucs: Vec<_> = d
            .use_cases
            .iter()
            .map(|u| (u.id.as_str(), u.shape))
            .collect();
        assert_eq!(
            ucs,
            [
                ("Login", UseCaseShape::Ellipse),
                ("Report", UseCaseShape::Rect),
                ("Reset_password", UseCaseShape::Ellipse),
            ]
        );
        assert_eq!(d.use_cases[0].label.text, "Sign in");
    }

    #[test]
    fn merges_equivalent_repeated_declarations() {
        let d = p(
            "systemBoundary Auth\nactor User(\"Person\")@{ type: hollow } <<Human>>\nactor User(\"Person\")@{ type: hollow } <<Human>>\nLogin[Sign in] <<Primary>>\nLogin[Sign in] <<Primary>>\nend",
        );
        assert_eq!(d.actors.len(), 1);
        let a = &d.actors[0];
        assert_eq!(
            (a.kind, a.stereotype.as_deref(), a.parent.as_deref()),
            (ActorType::Hollow, Some("Human"), Some("Auth"))
        );
        let u = &d.use_cases[0];
        assert_eq!(
            (u.shape, u.stereotype.as_deref()),
            (UseCaseShape::Rect, Some("Primary"))
        );
        assert_eq!(d.boundaries[0].members, ["User", "Login"]);
    }

    #[test]
    fn rejects_conflicting_declarations() {
        for (body, id) in [
            ("actor Shared\nShared", "Shared"),
            ("Login(Sign in)\nLogin[Sign in]", "Login"),
            ("Login(First label)\nLogin(Second label)", "Login"),
            ("Login <<Primary>>\nLogin <<Secondary>>", "Login"),
            (
                "systemBoundary First\nLogin\nend\nsystemBoundary Second\nLogin\nend",
                "Login",
            ),
            ("\"A-B\"\n\"A B\"", "A_B"),
            ("\"A B\"\nA_B(Explicit)", "A_B"),
            ("actor Shared\njson Shared@{}", "Shared"),
            ("actor link\nA link@--> B", "link"),
            ("A link@--> B\nB link@--> C", "link"),
            ("note for Ghost \"Missing\"", "Ghost"),
            ("class Ghost important", "Ghost"),
            ("style Ghost fill:red", "Ghost"),
            ("Ghost@{ type: package }", "Ghost"),
        ] {
            let m = err(body);
            assert!(m.contains(id), "{body:?}: {m}");
        }
    }

    #[test]
    fn anonymous_edge_and_note_ids_are_sequential() {
        let d = p(
            "actor User\nUser --> Login\nnote for User \"first\"\nLogin --> Logout\nnote for Logout \"second\"",
        );
        let e: Vec<_> = d.relationships.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(e, ["edge-0", "edge-1"]);
        let n: Vec<_> = d.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(n, ["note-0", "note-1"]);
    }

    #[test]
    fn labels_keep_entities_markdown_and_literal_text() {
        let d = p(
            "actor User(\"`**User**\nrole`\") <<Human>>\nEscaped(\"Open #40;safe#41;\")\nLiteral(\"First\\nSecond\")\nPriced[Costs 19.95 EUR, incl. VAT!]\nProgress(Ärende — 50% klart)\nUser -- opens **form** --> Priced",
        );
        assert_eq!(
            d.actors[0].label,
            Label {
                text: "**User**\nrole".into(),
                kind: LabelKind::Markdown
            }
        );
        let label = |id: &str| {
            d.use_cases
                .iter()
                .find(|u| u.id == id)
                .unwrap()
                .label
                .text
                .clone()
        };
        assert_eq!(label("Escaped"), "Open #40;safe#41;");
        assert_eq!(label("Literal"), "First\\nSecond");
        assert_eq!(label("Priced"), "Costs 19.95 EUR, incl. VAT!");
        assert_eq!(label("Progress"), "Ärende — 50% klart");
        assert_eq!(
            d.relationships[0].label.as_ref().unwrap().text,
            "opens **form**"
        );
        assert_eq!(label_lines(&d.actors[0].label), ["User", "role"]);
        assert_eq!(
            label_lines(&Label::text("Open #40;safe#41;")),
            ["Open (safe)"]
        );
    }

    #[test]
    fn rejects_reserved_material_in_unquoted_labels() {
        for body in [
            "Literal(Literal (nested) markers)",
            "Literal[Literal [nested] markers]",
            "Literal(Literal {nested} markers)",
            "Literal(Literal -- markers)",
            "Literal(Literal ::: markers)",
            "Literal(Literal @{ markers)",
            "Literal(Literal \"markers)",
            "Literal(Reviewer's markers)",
            "Broken(\"First\nSecond\")",
        ] {
            assert!(
                parse(&format!("usecase-beta\n{body}"), None).is_err(),
                "{body}"
            );
        }
    }

    #[test]
    fn association_operators_markers_and_minlen() {
        let d = p(
            "actor User\nactor Support\nStart\nFinish\nUser --> Start\nStart <-- Support\nStart -- Finish\nUser --o Start\nStart o-- Support\nUser --x Finish\nFinish x-- Support\nUser ----> Finish\nUser -- \"longer\" ----> Start",
        );
        let arrows: Vec<_> = d
            .relationships
            .iter()
            .map(|r| (r.arrow, r.minlen))
            .collect();
        assert_eq!(
            arrows,
            [
                (ArrowType::SolidArrow, 1),
                (ArrowType::BackArrow, 1),
                (ArrowType::LineSolid, 1),
                (ArrowType::CircleArrow, 1),
                (ArrowType::CircleArrowReversed, 1),
                (ArrowType::CrossArrow, 1),
                (ArrowType::CrossArrowReversed, 1),
                (ArrowType::SolidArrow, 3),
                (ArrowType::SolidArrow, 3),
            ]
        );
        assert!(
            d.relationships
                .iter()
                .all(|r| r.kind == RelationshipType::Association)
        );
        assert_eq!(d.relationships[8].label.as_ref().unwrap().text, "longer");
    }

    #[test]
    fn include_extend_and_generalization() {
        let d = p(
            "actor Admin\nactor Person\nCheckout\nPayment\nApplyCoupon\nAdmin --|> Person\nCheckout ..> : include Payment\nApplyCoupon ..> : extend Checkout\nApplyCoupon --|> Checkout\nUser -- \"include account details\" --> Checkout",
        );
        let kinds: Vec<_> = d.relationships.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RelationshipType::Generalization,
                RelationshipType::Include,
                RelationshipType::Extend,
                RelationshipType::Generalization,
                RelationshipType::Association,
            ]
        );
        assert!(
            err("actor A\nB\nA ..> : include B")
                .contains("include relationship requires use-case endpoints")
        );
        assert!(err("actor A\nB\nA --|> B").contains("Generalization requires"));
        assert!(err("json J@{}\nA --o J").contains("permits only point"));
    }

    #[test]
    fn metadata_validation() {
        assert!(
            err("actor A@{ fillColor: red }")
                .contains("Metadata property 'fillColor' is invalid for actor 'A'")
        );
        assert!(err("actor A@{ icon: \"fa:user\", type: hollow }").contains("cannot combine icon"));
        assert!(err("actor A@{ type: awesome, business: true }").contains("Business actor 'A'"));
        assert!(err("R[Rect]@{ business: true }").contains("Rectangular use case 'R'"));
        let d = p(
            "actor I@{ icon: \"fa:user\" }\nactor S@{ business: true }\nQ(\"Quote\")@{ business: true }",
        );
        assert_eq!(d.actors[0].kind, ActorType::Icon);
        assert_eq!(d.actors[0].icon.as_deref(), Some("fa:user"));
        assert!(d.actors[1].business && d.use_cases[0].business);
    }

    #[test]
    fn edge_ids_animation_classes_and_styles() {
        let d = p(
            "actor Customer\nCheckout\nPayment\nCustomer opens@-- \"starts checkout\" ---> Checkout\nCheckout payment@..> : include Payment\nclassDef emphasized stroke:#d33,stroke-width:3px\nclass opens,payment emphasized\nstyle opens stroke:#06c,stroke-width:4px\nopens@{ animation: fast }\npayment@{ animate: false }",
        );
        let r = &d.relationships;
        assert_eq!(
            (r[0].id.as_str(), r[0].explicit_id, r[0].minlen),
            ("opens", true, 2)
        );
        assert_eq!(
            (r[0].animate, r[0].animation),
            (true, Some(Animation::Fast))
        );
        assert!(!r[1].animate);
        assert_eq!(r[0].classes, ["emphasized"]);
        assert_eq!(r[0].styles, ["stroke:#06c", "stroke-width:4px"]);
        assert_eq!(
            d.class_defs[0],
            (
                "emphasized".into(),
                vec!["stroke:#d33".into(), "stroke-width:3px".into()]
            )
        );
        assert!(err("A x@--> B\nx@{ animation: medium }").contains("invalid for edge"));
    }

    #[test]
    fn boundaries_with_titles_metadata_and_classes() {
        let d = p(
            "systemBoundary sb1[\"Payment service\"]@{ type: package }:::system\n  actor Clerk(\"Payment clerk\")\n  Authorize(\"Authorize payment\")\nend\nClerk --> Authorize",
        );
        let b = &d.boundaries[0];
        assert_eq!(
            (b.id.as_str(), b.label.text.as_str(), b.kind),
            ("sb1", "Payment service", BoundaryType::Package)
        );
        assert_eq!(b.classes, ["system"]);
        assert_eq!(b.members, ["Clerk", "Authorize"]);
        assert_eq!(d.actors[0].parent.as_deref(), Some("sb1"));
        let d = p(
            "systemBoundary \"Payment service\"\n  Authorize\nend\nPayment_service@{ type: package }",
        );
        assert_eq!(
            (d.boundaries[0].id.as_str(), d.boundaries[0].kind),
            ("Payment_service", BoundaryType::Package)
        );
        let d = p("systemBoundary B@{ type: package }\nX\nend\nB@{ type: rect }");
        assert_eq!(d.boundaries[0].kind, BoundaryType::Rect);
        assert!(err("systemBoundary B@{ type: oval }\nend").contains("invalid for boundary"));
        assert!(parse("usecase-beta\nsystemBoundary B\nA --> C\nend", None).is_err());
        assert!(
            parse(
                "usecase-beta\nsystemBoundary B:::c@{ type: package }\nend",
                None
            )
            .is_err()
        );
    }

    #[test]
    fn notes_and_their_targets() {
        let d = p(
            "note for Login \"`Requires an **active session**`\"\nactor User\nLogin(\"Sign in\")\nUser --> Login\nnote for Literal Read the guide; then sign off.\nLiteral",
        );
        assert_eq!(d.notes[0].target, "Login");
        assert_eq!(d.notes[0].label.kind, LabelKind::Markdown);
        assert_eq!(d.notes[1].label.text, "Read the guide; then sign off.");
        assert!(
            err("json J@{}\nnote for J \"x\"").contains("must be an actor or use case, not json")
        );
        assert!(err("systemBoundary B\nend\nnote for B \"x\"").contains("not boundary"));
    }

    #[test]
    fn json_tables_flatten_in_source_order() {
        let d = p(
            "json Payload@{\n  \"2\": \"second in source\",\n  \"1\": \"first after 2\",\n  \"enabled\": true,\n  \"count\": 3,\n  \"missing\": null,\n  \"colors\": [\"Red\", \"Green\"],\n  \"address\": { \"city\": \"Oslo\" },\n  \"items\": [{ \"name\": \"Book\" }],\n  \"emptyObject\": {},\n  \"emptyArray\": [],\n  \"count\": 4\n}:::data",
        );
        let rows: Vec<_> = d.json_tables[0]
            .rows
            .iter()
            .map(|r| (r.key.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("2", "second in source"),
                ("1", "first after 2"),
                ("enabled", "true"),
                ("count", "4"),
                ("missing", "null"),
                ("colors", "Red"),
                ("", "Green"),
                ("address.city", "Oslo"),
                ("items[0].name", "Book"),
                ("emptyObject", "{}"),
                ("emptyArray", "[]"),
            ]
        );
        assert_eq!(d.json_tables[0].classes, ["data"]);
        assert!(parse("usecase-beta\njson P@{ \"a\": }", None).is_err());
        assert!(parse("usecase-beta\nsystemBoundary B\njson P@{}\nend", None).is_err());
    }

    #[test]
    fn statements_comments_direction_and_plantuml_rejections() {
        let d = p(
            "%% comment\ndirection TB\nactor User\n\n  %% indented\nUser --> Login\ndirection RL",
        );
        assert_eq!(d.direction, Direction::RightLeft);
        assert!(parse("usecase-beta\nactor A actor B", None).is_err());
        assert!(parse("usecase-beta\nA --> B; C --> D", None).is_err());
        for s in [
            "skinparam x",
            "package P",
            "rectangle R",
            "newpage",
            "A -left-> B",
        ] {
            assert!(parse(&format!("usecase-beta\n{s}"), None).is_err(), "{s}");
        }
    }

    #[test]
    fn numeric_ids_and_multi_word_styles() {
        let d = p(
            "actor 1\n1mg --> 3rd\nclassDef c border:1px solid red,stroke-width:1.5px\nclass 1mg c\nstyle 3rd font-family:a\\, b",
        );
        assert_eq!(d.actors[0].id, "1");
        assert_eq!(d.use_cases[0].id, "1mg");
        assert_eq!(
            d.class_defs[0].1,
            ["border:1px solid red", "stroke-width:1.5px"]
        );
        assert_eq!(d.use_cases[1].styles, ["font-family:a, b"]);
    }

    #[test]
    fn generated_ids_follow_javascript_word_rule() {
        assert_eq!(generate_id("Reset password"), "Reset_password");
        assert_eq!(generate_id("Ärende 1"), "_rende_1");
    }

    fn scene(src: &str) -> Scene {
        let cfg = MermaidConfig::default().scoped("usecase");
        render(src, None, &cfg, &cfg.palette()).unwrap()
    }

    fn find_group<'a>(els: &'a [Element], id: &str) -> Option<&'a [Element]> {
        els.iter().find_map(|e| match e {
            Element::Group {
                id: gid, children, ..
            } if gid.as_deref() == Some(id) => Some(children.as_slice()),
            Element::Group { children, .. } => find_group(children, id),
            _ => None,
        })
    }

    /// Extent of the visible shapes of a group (drop shadows excluded).
    fn extent(els: &[Element]) -> Rect {
        let mut b = Bounds::default();
        for e in els {
            let Element::Shape { geom, class, .. } = e else {
                continue;
            };
            if class.as_deref() == Some("drop-shadow") {
                continue;
            }
            match geom {
                Geometry::Rect { rect, .. } => b.add_rect(*rect),
                Geometry::Circle { c, r } => b.add_rect(Rect::centered(*c, 2.0 * r, 2.0 * r)),
                Geometry::Path(cmds) => {
                    for c in cmds {
                        if let PathCmd::MoveTo(p) | PathCmd::LineTo(p) | PathCmd::CubicTo(_, _, p) =
                            c
                        {
                            b.add_point(*p);
                        }
                    }
                }
            }
        }
        b.rect()
    }

    #[test]
    fn use_cases_lie_inside_their_boundary() {
        let s = scene(
            "usecase-beta\ndirection LR\nactor Customer\nactor Support\nsystemBoundary Storefront\n  Browse(\"Browse catalogue\")\n  Checkout(\"Checkout\")\nend\nsystemBoundary Fulfilment\n  actor Clerk\n  Track(\"Track delivery\")\nend\nCustomer --> Browse\nCustomer --> Checkout\nCustomer --> Track\nSupport --> Track\nClerk --> Track\nCheckout ..> : include Browse",
        );
        let ext = |id: &str| extent(find_group(&s.elements, &format!("usecase-{id}")).unwrap());
        let inside = |inner: Rect, outer: Rect| {
            inner.x >= outer.x - 0.5
                && inner.y >= outer.y - 0.5
                && inner.right() <= outer.right() + 0.5
                && inner.bottom() <= outer.bottom() + 0.5
        };
        let (store, fulfil) = (ext("Storefront"), ext("Fulfilment"));
        for id in ["Browse", "Checkout"] {
            assert!(inside(ext(id), store), "{id} in Storefront");
            assert!(!ext(id).intersects(&fulfil), "{id} outside Fulfilment");
        }
        for id in ["Track", "Clerk"] {
            assert!(inside(ext(id), fulfil), "{id} in Fulfilment");
        }
        for id in ["Customer", "Support"] {
            assert!(
                !ext(id).intersects(&store) && !ext(id).intersects(&fulfil),
                "{id}"
            );
        }
        assert!(!store.intersects(&fulfil));
    }

    #[test]
    fn upstream_node_geometry() {
        let cfg = MermaidConfig::default().scoped("usecase");
        let o = Options::new(&cfg, &cfg.palette());
        let u = |shape, stereotype: Option<&str>| UseCase {
            id: "U".into(),
            label: Label::text("Browse"),
            shape,
            business: false,
            stereotype: stereotype.map(str::to_string),
            parent: None,
            classes: vec![],
            styles: vec![],
        };
        // usecaseEllipse: label widened to minWidth 120, radius = label / 2 + 20.
        let (_, w, h) = measure_use_case(&u(UseCaseShape::Ellipse, None), &o);
        assert_eq!((w, h), (160.0, 58.0));
        let (_, _, h) = measure_use_case(&u(UseCaseShape::Ellipse, Some("Core")), &o);
        assert_eq!(h, 76.0);
        // neo squareRect: 16 / 12 padding per side.
        let (_, w, h) = measure_use_case(&u(UseCaseShape::Rect, None), &o);
        assert_eq!((w, h), (152.0, 42.0));
        // usecaseActor: 72px figure + 8px gap + 21px label + 2 · 10 padding.
        let a = Actor {
            id: "A".into(),
            label: Label::text("Customer"),
            kind: ActorType::Normal,
            icon: None,
            business: false,
            stereotype: None,
            parent: None,
            classes: vec![],
            styles: vec![],
        };
        assert_eq!(measure_actor(&a, &o).2, 121.0);
    }

    #[test]
    fn usecase_defaults_to_redux_color_and_neo() {
        let cfg = MermaidConfig::default().scoped("usecase");
        assert_eq!((cfg.theme, cfg.look), (ThemeName::ReduxColor, Look::Neo));
        let role = role_colors(cfg.theme, &[], &cfg.palette());
        assert_eq!(role.actor_border, Color::new("#A78BFA"));
        assert_eq!(role.include_line, Color::new("#38BDF8"));
        let vars = vec![("usecaseBkg".to_string(), "#123456".to_string())];
        assert_eq!(
            role_colors(ThemeName::Default, &vars, &cfg.palette()).bkg,
            Color::new("#123456")
        );
    }

    #[test]
    fn renders_the_documented_examples() {
        for src in [
            "usecase-beta\ndirection LR\nactor Customer(\"Customer\")\nsystemBoundary \"Order system\"\n  Checkout(\"Place order\")\nend\nCustomer --> Checkout",
            "usecase-beta\nactor Normal(\"Normal actor\")\nactor Hollow(\"Hollow actor\")@{ type: hollow }\nactor Awesome(\"Awesome actor\")@{ type: awesome }\nactor Icon(\"Registered icon\")@{ icon: \"fa:user\" }\nNormal --> Manage\nHollow --> Manage\nAwesome --> Manage\nIcon --> Manage",
            "usecase-beta\nactor SalesAgent(\"Sales agent\")@{ business: true } <<Employee>>\nQuote(\"Prepare quote\")@{ business: true } <<Core>>\nArchive[Archive quote] <<Record>>\nSalesAgent --> Quote\nQuote --> Archive",
        ] {
            let s = scene(src);
            assert_eq!(s.kind, DiagramKind::Usecase);
            assert!(s.view_box.w > 0.0 && s.view_box.h > 0.0);
        }
    }
}
