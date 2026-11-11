//! Railroad (syntax) diagrams: mermaid 12.3's `railroad-beta`, `railroad-ebnf-beta`,
//! `railroad-abnf-beta` and `railroad-peg-beta`.
//!
//! The four notations are lexed and parsed separately (mirroring the four langium
//! grammars in `packages/parser/src/language/railroad*`) and lowered into one typed
//! expression AST ([`Expr`]), exactly as upstream's `*Parser.ts` transforms do
//! (single-element sequences/choices collapse, EBNF `A - B` becomes the sequence
//! `A "-" B`, PEG predicates become special boxes, ABNF `*1A` becomes an optional…).
//!
//! The layout is a pure port of `railroadRenderer.ts`: every expression yields a
//! box with `width`, `height` and the rail height `up` (`down = height - up`);
//! sequences join children with `horizontalSeparation` rails, choices stack
//! alternatives with `verticalSeparation` and connect them with quarter-circle arcs
//! of `arcRadius`, optionals get a bypass above, repetitions a loop below (plus a
//! bypass for zero-or-more). Rules are stacked vertically with a bold `name =`
//! label, start/end marker dots and the definition on a baseline at least 20px down.

use crate::color::{self, Rgba};
use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{PathCmd, Point, Rect, arc_cubics};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::{Palette, ThemeName};

// ───────────────────────────── AST ─────────────────────────────

/// The four railroad notations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `railroad-beta`: explicit constructors (`sequence(...)`, `choice(...)`, …).
    Ir,
    /// `railroad-ebnf-beta`: W3C / ISO 14977 EBNF.
    Ebnf,
    /// `railroad-abnf-beta`: RFC 5234 ABNF.
    Abnf,
    /// `railroad-peg-beta`: parsing expression grammars.
    Peg,
}

impl Syntax {
    /// Header keywords, longest-prefix-safe order.
    const HEADERS: [(&'static str, Syntax); 4] = [
        ("railroad-ebnf-beta", Syntax::Ebnf),
        ("railroad-abnf-beta", Syntax::Abnf),
        ("railroad-peg-beta", Syntax::Peg),
        ("railroad-beta", Syntax::Ir),
    ];

    fn header(self) -> &'static str {
        Self::HEADERS
            .iter()
            .find(|(_, s)| *s == self)
            .map(|(h, _)| *h)
            .unwrap_or("railroad-beta")
    }

    /// The upstream diagram id (`aria-roledescription`).
    pub fn kind(self) -> DiagramKind {
        match self {
            Syntax::Ir => DiagramKind::Railroad,
            Syntax::Ebnf => DiagramKind::RailroadEbnf,
            Syntax::Abnf => DiagramKind::RailroadAbnf,
            Syntax::Peg => DiagramKind::RailroadPeg,
        }
    }
}

/// A grammar expression (upstream `ASTNode`).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal text, drawn as a rounded box.
    Terminal(String),
    /// Rule reference, drawn as a square box.
    NonTerminal(String),
    /// Elements one after another.
    Sequence(Vec<Expr>),
    /// Alternatives stacked vertically.
    Choice(Vec<Expr>),
    /// Zero or one occurrence (bypass above).
    Optional(Box<Expr>),
    /// `min..=max` occurrences (`max == None` is unbounded). Only `min == 0`
    /// changes the drawing (adds a bypass), as upstream.
    Repetition {
        element: Box<Expr>,
        min: u32,
        max: Option<u32>,
    },
    /// Special sequence, drawn as a dashed box labelled `? text ?`.
    Special(String),
}

impl Expr {
    fn sequence(mut elements: Vec<Expr>) -> Expr {
        if elements.len() == 1 {
            elements.remove(0)
        } else {
            Expr::Sequence(elements)
        }
    }
    fn choice(mut alternatives: Vec<Expr>) -> Expr {
        if alternatives.len() == 1 {
            alternatives.remove(0)
        } else {
            Expr::Choice(alternatives)
        }
    }
    fn repeat(element: Expr, min: u32, max: Option<u32>) -> Expr {
        Expr::Repetition {
            element: Box::new(element),
            min,
            max,
        }
    }
}

/// One grammar rule: `name = definition ;`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub definition: Expr,
}

/// A parsed railroad diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Grammar {
    pub syntax: Syntax,
    /// Parsed but, as upstream, not drawn.
    pub title: Option<String>,
    pub rules: Vec<Rule>,
}

// ───────────────────────────── lexer ─────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Header,
    Title(String),
    /// Keyword or punctuation.
    Kw(&'static str),
    Ident(String),
    Str(String),
    /// EBNF `? … ?` (already trimmed).
    SpecialSeq(String),
    /// ABNF `%x41`, `%d48-57`, …
    NumVal(String),
    /// ABNF `*`, `1*`, `2*4`, `3`.
    Repeat(String),
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Header => "header".into(),
            Tok::Title(t) => format!("title {t}"),
            Tok::Kw(k) => (*k).into(),
            Tok::Ident(s) | Tok::NumVal(s) | Tok::Repeat(s) => s.clone(),
            Tok::Str(s) => format!("\"{s}\""),
            Tok::SpecialSeq(s) => format!("? {s} ?"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    tok: Tok,
    line: usize,
    col: usize,
}

const IR_KEYWORDS: [&str; 13] = [
    "nonterminal",
    "zeroOrMore",
    "oneOrMore",
    "sequence",
    "optional",
    "terminal",
    "special",
    "choice",
    "=",
    ";",
    "(",
    ")",
    ",",
];
const EBNF_KEYWORDS: [&str; 15] = [
    "::=", "=", ";", "|", ",", "(", ")", "[", "]", "{", "}", "?", "*", "+", "-",
];
const ABNF_KEYWORDS: [&str; 7] = ["=", ";", "/", "(", ")", "[", "]"];
const PEG_KEYWORDS: [&str; 11] = ["<-", ";", "/", "&", "!", "?", "*", "+", "(", ")", "."];

fn is_id_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_id_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    line: usize,
    line_start: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src,
            b: src.as_bytes(),
            i: 0,
            line: 1,
            line_start: 0,
        }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.i..]
    }

    /// Advance `n` bytes, tracking line numbers.
    fn bump(&mut self, n: usize) {
        let end = (self.i + n).min(self.b.len());
        for k in self.i..end {
            if self.b[k] == b'\n' {
                self.line += 1;
                self.line_start = k + 1;
            }
        }
        self.i = end;
    }

    fn col(&self) -> usize {
        self.src[self.line_start..self.i].chars().count() + 1
    }

    fn error(&self) -> Error {
        let c = self.rest().chars().next().unwrap_or(' ');
        Error::parse(
            self.line,
            format!(
                "Lexer error on line {}, column {}: unexpected character: ->{c}<- at offset: {}, skipped 1 characters.",
                self.line,
                self.col(),
                self.i
            ),
        )
    }

    /// Length of the rest of the current line (excluding the newline).
    fn line_len(&self) -> usize {
        self.rest().find(['\n', '\r']).unwrap_or(self.rest().len())
    }

    /// Skip whitespace and hidden comments. Returns true when something was skipped.
    fn skip_hidden(&mut self, syntax: Syntax) -> bool {
        let start = self.i;
        loop {
            let r = self.rest();
            let Some(&c) = r.as_bytes().first() else {
                break;
            };
            if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
                self.bump(1);
            } else if r.starts_with("%%") {
                self.bump(self.line_len());
            } else if matches!(syntax, Syntax::Ir | Syntax::Ebnf) && r.starts_with("/*") {
                match r[2..].find("*/") {
                    Some(e) => self.bump(e + 4),
                    None => break,
                }
            } else if syntax == Syntax::Ebnf && r.starts_with("(*") {
                // `(` keyword with the ISO comment as its longer alternative.
                match r[2..].find("*)") {
                    Some(e) => self.bump(e + 4),
                    None => break,
                }
            } else if syntax == Syntax::Peg && c == b'#' {
                self.bump(self.line_len());
            } else if syntax == Syntax::Abnf && c == b';' && self.line_len() > 1 {
                // `;` keyword vs `;[^\n\r]*` comment: the comment wins when longer
                // (chevrotain LONGER_ALT), so `rule = "a" ; note` loses its `;`.
                self.bump(self.line_len());
            } else {
                break;
            }
        }
        self.i > start
    }

    fn header(&self) -> Option<(Syntax, usize)> {
        let r = self.rest();
        Syntax::HEADERS.iter().find_map(|(h, s)| {
            let head = r.get(..h.len())?;
            let after = &r[h.len()..];
            let ends =
                after.starts_with("%%") || after.chars().next().is_none_or(char::is_whitespace);
            (head.eq_ignore_ascii_case(h) && ends).then_some((*s, h.len()))
        })
    }

    /// `title(?:[\t ][^\n\r]*?(?=%%)|[\t ][^\n\r]*|)`; returns (consumed, raw value).
    fn title(&self) -> Option<(usize, String)> {
        let r = self.rest();
        let after = r.strip_prefix("title")?;
        if !after.starts_with([' ', '\t']) {
            return Some((5, String::new()));
        }
        let line = &after[..after.find(['\n', '\r']).unwrap_or(after.len())];
        let line = &line[..line.find("%%").unwrap_or(line.len())];
        let value = line.split_whitespace().collect::<Vec<_>>().join(" ");
        Some((5 + line.len(), value))
    }

    fn keyword(&self, kws: &[&'static str]) -> Option<&'static str> {
        let r = self.rest();
        kws.iter().copied().find(|k| {
            if !r.starts_with(k) {
                return false;
            }
            // Word keywords lose to a longer identifier (`sequenceX`).
            let word = k.as_bytes()[0].is_ascii_alphabetic();
            !word || r.as_bytes().get(k.len()).is_none_or(|&c| !is_id_cont(c))
        })
    }

    fn ident(&self, abnf: bool) -> Option<usize> {
        let b = self.rest().as_bytes();
        let first = *b.first()?;
        let ok = if abnf {
            first.is_ascii_alphabetic()
        } else {
            is_id_start(first)
        };
        if !ok {
            return None;
        }
        let cont = |c: &u8| {
            if abnf {
                c.is_ascii_alphanumeric() || *c == b'-'
            } else {
                is_id_cont(*c)
            }
        };
        Some(1 + b[1..].iter().take_while(|c| cont(c)).count())
    }

    /// `"([^"\\]|\\.)*"|'([^'\\]|\\.)*'` (escapes allowed) or ABNF `"[^"]*"`.
    fn string(&self, escapes: bool) -> Option<usize> {
        let b = self.rest().as_bytes();
        let q = *b.first()?;
        if !(q == b'"' || (escapes && q == b'\'')) {
            return None;
        }
        let mut k = 1;
        while k < b.len() {
            match b[k] {
                b'\\' if escapes => k += 2,
                c if c == q => return Some(k + 1),
                _ => k += 1,
            }
        }
        None
    }

    /// EBNF `\?(?=[^?;]*[^?\s;][^?;]*\?)[^?;]*\?`.
    fn special_seq(&self) -> Option<usize> {
        let r = self.rest();
        let inner = r.strip_prefix('?')?;
        let end = inner.find(['?', ';'])?;
        (inner.as_bytes()[end] == b'?' && !inner[..end].trim().is_empty()).then_some(end + 2)
    }

    /// ABNF `%[xXdDbB][0-9A-Fa-f]+(?:-[0-9A-Fa-f]+|\.[0-9A-Fa-f]+)*`.
    fn numval(&self) -> Option<usize> {
        let b = self.rest().as_bytes();
        if b.first() != Some(&b'%') || !b.get(1).is_some_and(|c| b"xXdDbB".contains(c)) {
            return None;
        }
        let hex = |k: usize| b[k..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let mut k = 2;
        let n = hex(k);
        if n == 0 {
            return None;
        }
        k += n;
        while k < b.len() && (b[k] == b'-' || b[k] == b'.') {
            let n = hex(k + 1);
            if n == 0 {
                break;
            }
            k += 1 + n;
        }
        Some(k)
    }

    /// ABNF `[0-9]*\*[0-9]*` then `[0-9]+`.
    fn repeat(&self) -> Option<usize> {
        let b = self.rest().as_bytes();
        let digits = |k: usize| b[k..].iter().take_while(|c| c.is_ascii_digit()).count();
        let lead = digits(0);
        if b.get(lead) == Some(&b'*') {
            return Some(lead + 1 + digits(lead + 1));
        }
        (lead > 0).then_some(lead)
    }

    fn next_token(&mut self, syntax: Syntax) -> Result<Option<Token>> {
        while self.skip_hidden(syntax) {}
        if self.i >= self.b.len() {
            return Ok(None);
        }
        let (line, col) = (self.line, self.col());
        let (tok, len) = self.scan(syntax).ok_or_else(|| self.error())?;
        self.bump(len);
        Ok(Some(Token { tok, line, col }))
    }

    fn scan(&self, syntax: Syntax) -> Option<(Tok, usize)> {
        if let Some((_, n)) = self.header() {
            return Some((Tok::Header, n));
        }
        // EBNF `?`: the special sequence is the keyword's longer alternative.
        if syntax == Syntax::Ebnf {
            if let Some(n) = self.special_seq() {
                let text = self.rest()[1..n - 1].trim().to_string();
                return Some((Tok::SpecialSeq(text), n));
            }
        }
        let kws: &[&'static str] = match syntax {
            Syntax::Ir => &IR_KEYWORDS,
            Syntax::Ebnf => &EBNF_KEYWORDS,
            Syntax::Abnf => &ABNF_KEYWORDS,
            Syntax::Peg => &PEG_KEYWORDS,
        };
        if let Some(k) = self.keyword(kws) {
            return Some((Tok::Kw(k), k.len()));
        }
        if let Some((n, value)) = self.title() {
            return Some((Tok::Title(value), n));
        }
        let abnf = syntax == Syntax::Abnf;
        if let Some(n) = self.ident(abnf) {
            return Some((Tok::Ident(self.rest()[..n].to_string()), n));
        }
        if let Some(n) = self.string(!abnf) {
            let raw = &self.rest()[..n];
            let value = if abnf {
                raw[1..n - 1].to_string()
            } else {
                decode_escaped(raw)
            };
            return Some((Tok::Str(value), n));
        }
        if abnf {
            if let Some(n) = self.numval() {
                return Some((Tok::NumVal(self.rest()[..n].to_string()), n));
            }
            if let Some(n) = self.repeat() {
                return Some((Tok::Repeat(self.rest()[..n].to_string()), n));
            }
        }
        None
    }
}

/// `decodeEscapedString`: strip the quotes, `\n`/`\r`/`\t` escapes, `\x` → `x`.
fn decode_escaped(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn tokenize(src: &str, syntax: Syntax) -> Result<Vec<Token>> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    while let Some(t) = lx.next_token(syntax)? {
        out.push(t);
    }
    Ok(out)
}

// ───────────────────────────── parser ─────────────────────────────

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    last_line: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }
    fn peek_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Some(Tok::Kw(x)) if *x == k)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t.map(|t| t.tok)
    }
    fn err(&self, expected: &str) -> Error {
        match self.toks.get(self.pos) {
            Some(t) => Error::parse(
                t.line,
                format!(
                    "Parse error on line {}, column {}: Expecting {expected} but found `{}`.",
                    t.line,
                    t.col,
                    t.tok.describe()
                ),
            ),
            None => Error::parse(
                self.last_line,
                format!("Expecting {expected} but found `EOF`."),
            ),
        }
    }
    fn expect_kw(&mut self, k: &str) -> Result<()> {
        if self.peek_kw(k) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("token of type '{k}'")))
        }
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        let hit = self.peek_kw(k);
        if hit {
            self.pos += 1;
        }
        hit
    }
}

/// Parse a railroad diagram of any of the four notations.
pub fn parse(body: &str, title: Option<String>) -> Result<Grammar> {
    let first = body.lines().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let header_line = body.lines().nth(first).unwrap_or("").trim_start();
    let syntax = Syntax::HEADERS
        .iter()
        .find(|(h, _)| {
            header_line
                .get(..h.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(h))
        })
        .map(|(_, s)| *s)
        .ok_or_else(|| Error::parse(first + 1, "Expecting a railroad diagram header"))?;
    let toks = tokenize(body, syntax)?;
    let last_line = toks.last().map_or(1, |t| t.line);
    let mut p = Parser {
        toks,
        pos: 0,
        last_line,
    };
    if p.peek() != Some(&Tok::Header) {
        return Err(p.err(&format!("token of type '{}'", syntax.header())));
    }
    p.pos += 1;
    let mut grammar = Grammar {
        syntax,
        title,
        rules: Vec::new(),
    };
    while let Some(Tok::Title(t)) = p.peek() {
        let t = t.clone();
        p.pos += 1;
        grammar.title = Some(unquote_title(&t, syntax));
    }
    while p.peek().is_some() {
        grammar.rules.push(parse_rule(&mut p, syntax)?);
    }
    Ok(grammar)
}

fn unquote_title(t: &str, syntax: Syntax) -> String {
    let quoted = t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')));
    if !quoted {
        t.to_string()
    } else if syntax == Syntax::Abnf {
        t[1..t.len() - 1].to_string()
    } else {
        decode_escaped(t)
    }
}

fn parse_rule(p: &mut Parser, syntax: Syntax) -> Result<Rule> {
    let name = match p.peek() {
        Some(Tok::Ident(n)) => n.clone(),
        _ => return Err(p.err("a rule name")),
    };
    p.pos += 1;
    let definition = match syntax {
        Syntax::Ir => {
            p.expect_kw("=")?;
            ir_expr(p)?
        }
        Syntax::Ebnf => {
            if !(p.eat_kw("=") || p.eat_kw("::=")) {
                return Err(p.err("one of '=' or '::='"));
            }
            ebnf_choice(p)?
        }
        Syntax::Abnf => {
            p.expect_kw("=")?;
            abnf_alternation(p)?
        }
        Syntax::Peg => {
            p.expect_kw("<-")?;
            peg_choice(p)?
        }
    };
    p.expect_kw(";")?;
    Ok(Rule { name, definition })
}

// IR: explicit constructors.
fn ir_expr(p: &mut Parser) -> Result<Expr> {
    let kw = match p.peek() {
        Some(Tok::Kw(k)) if k.as_bytes()[0].is_ascii_alphabetic() => *k,
        _ => return Err(p.err("a railroad expression")),
    };
    p.pos += 1;
    p.expect_kw("(")?;
    let string = |p: &mut Parser| match p.next() {
        Some(Tok::Str(s)) => Ok(s),
        _ => {
            p.pos -= 1;
            Err(p.err("a string"))
        }
    };
    let list = |p: &mut Parser| -> Result<Vec<Expr>> {
        let mut v = vec![ir_expr(p)?];
        while p.eat_kw(",") {
            v.push(ir_expr(p)?);
        }
        Ok(v)
    };
    let e = match kw {
        "terminal" => Expr::Terminal(string(p)?),
        "nonterminal" => Expr::NonTerminal(string(p)?),
        "special" => Expr::Special(string(p)?),
        "sequence" => Expr::sequence(list(p)?),
        "choice" => Expr::choice(list(p)?),
        "optional" => Expr::Optional(Box::new(ir_expr(p)?)),
        "oneOrMore" => Expr::repeat(ir_expr(p)?, 1, None),
        _ => Expr::repeat(ir_expr(p)?, 0, None), // zeroOrMore
    };
    p.expect_kw(")")?;
    Ok(e)
}

// EBNF.
fn ebnf_choice(p: &mut Parser) -> Result<Expr> {
    let mut alts = vec![ebnf_sequence(p)?];
    while p.eat_kw("|") {
        alts.push(ebnf_sequence(p)?);
    }
    Ok(Expr::choice(alts))
}

fn ebnf_starts_primary(t: Option<&Tok>) -> bool {
    matches!(
        t,
        Some(Tok::Str(_) | Tok::SpecialSeq(_) | Tok::Ident(_) | Tok::Kw("(" | "[" | "{"))
    )
}

fn ebnf_sequence(p: &mut Parser) -> Result<Expr> {
    let mut elements = vec![ebnf_term(p)?];
    loop {
        if p.eat_kw(",") || ebnf_starts_primary(p.peek()) {
            elements.push(ebnf_term(p)?);
        } else {
            break;
        }
    }
    Ok(Expr::sequence(elements))
}

fn ebnf_term(p: &mut Parser) -> Result<Expr> {
    let mut node = ebnf_primary(p)?;
    loop {
        node = if p.eat_kw("?") {
            Expr::Optional(Box::new(node))
        } else if p.eat_kw("*") {
            Expr::repeat(node, 0, None)
        } else if p.eat_kw("+") {
            Expr::repeat(node, 1, None)
        } else if p.eat_kw("-") {
            let except = ebnf_primary(p)?;
            Expr::Sequence(vec![node, Expr::Terminal("-".into()), except])
        } else {
            return Ok(node);
        }
    }
}

fn ebnf_primary(p: &mut Parser) -> Result<Expr> {
    let e = match p.peek().cloned() {
        Some(Tok::Str(s)) => Expr::Terminal(s),
        Some(Tok::SpecialSeq(s)) => Expr::Special(s),
        Some(Tok::Ident(s)) => Expr::NonTerminal(s),
        Some(Tok::Kw(open @ ("(" | "[" | "{"))) => {
            p.pos += 1;
            let inner = ebnf_choice(p)?;
            let (close, e) = match open {
                "(" => (")", inner),
                "[" => ("]", Expr::Optional(Box::new(inner))),
                _ => ("}", Expr::repeat(inner, 0, None)),
            };
            p.expect_kw(close)?;
            return Ok(e);
        }
        _ => return Err(p.err("an EBNF primary")),
    };
    p.pos += 1;
    Ok(e)
}

// ABNF.
fn abnf_alternation(p: &mut Parser) -> Result<Expr> {
    let mut alts = vec![abnf_concatenation(p)?];
    while p.eat_kw("/") {
        alts.push(abnf_concatenation(p)?);
    }
    Ok(Expr::choice(alts))
}

fn abnf_concatenation(p: &mut Parser) -> Result<Expr> {
    let mut elements = vec![abnf_element(p)?];
    while matches!(
        p.peek(),
        Some(Tok::Repeat(_) | Tok::Str(_) | Tok::NumVal(_) | Tok::Ident(_) | Tok::Kw("(" | "["))
    ) {
        elements.push(abnf_element(p)?);
    }
    Ok(Expr::sequence(elements))
}

/// `parseRepeat`: `*` → 0..∞, `1*` → 1..∞, `2*4` → 2..4, `3` → exactly 3.
fn abnf_repeat(r: &str) -> (u32, Option<u32>) {
    let num = |s: &str| s.parse::<u32>().unwrap_or(u32::MAX);
    match r.split_once('*') {
        Some((lo, hi)) => (
            if lo.is_empty() { 0 } else { num(lo) },
            (!hi.is_empty()).then(|| num(hi)),
        ),
        None => (num(r), Some(num(r))),
    }
}

fn abnf_element(p: &mut Parser) -> Result<Expr> {
    let repeat = match p.peek() {
        Some(Tok::Repeat(r)) => {
            let r = abnf_repeat(r);
            p.pos += 1;
            Some(r)
        }
        _ => None,
    };
    let inner = abnf_primary(p)?;
    Ok(match repeat {
        None => inner,
        Some((0, Some(1))) => Expr::Optional(Box::new(inner)),
        Some((min, max)) => Expr::repeat(inner, min, max),
    })
}

fn abnf_primary(p: &mut Parser) -> Result<Expr> {
    let e = match p.peek().cloned() {
        Some(Tok::Str(s) | Tok::NumVal(s)) => Expr::Terminal(s),
        Some(Tok::Ident(s)) => Expr::NonTerminal(s),
        Some(Tok::Kw(open @ ("(" | "["))) => {
            p.pos += 1;
            let inner = abnf_alternation(p)?;
            if open == "(" {
                p.expect_kw(")")?;
                return Ok(inner);
            }
            p.expect_kw("]")?;
            return Ok(Expr::Optional(Box::new(inner)));
        }
        _ => return Err(p.err("an ABNF element")),
    };
    p.pos += 1;
    Ok(e)
}

// PEG.
fn peg_choice(p: &mut Parser) -> Result<Expr> {
    let mut alts = vec![peg_sequence(p)?];
    while p.eat_kw("/") {
        alts.push(peg_sequence(p)?);
    }
    Ok(Expr::choice(alts))
}

fn peg_sequence(p: &mut Parser) -> Result<Expr> {
    let mut elements = vec![peg_prefix(p)?];
    while matches!(
        p.peek(),
        Some(Tok::Str(_) | Tok::Ident(_) | Tok::Kw("&" | "!" | "(" | "."))
    ) {
        elements.push(peg_prefix(p)?);
    }
    Ok(Expr::sequence(elements))
}

/// Label of a predicate operand (`nodeToLabel`).
fn peg_label(e: &Expr) -> String {
    match e {
        Expr::Terminal(v) => format!("\"{v}\""),
        Expr::NonTerminal(n) => n.clone(),
        Expr::Special(t) => t.clone(),
        _ => "(...)".into(),
    }
}

fn peg_prefix(p: &mut Parser) -> Result<Expr> {
    let op = if p.eat_kw("&") {
        Some('&')
    } else if p.eat_kw("!") {
        Some('!')
    } else {
        None
    };
    let inner = peg_suffix(p)?;
    Ok(match op {
        Some(op) => Expr::Special(format!("{op}{}", peg_label(&inner))),
        None => inner,
    })
}

fn peg_suffix(p: &mut Parser) -> Result<Expr> {
    let inner = peg_primary(p)?;
    Ok(if p.eat_kw("?") {
        Expr::Optional(Box::new(inner))
    } else if p.eat_kw("*") {
        Expr::repeat(inner, 0, None)
    } else if p.eat_kw("+") {
        Expr::repeat(inner, 1, None)
    } else {
        inner
    })
}

fn peg_primary(p: &mut Parser) -> Result<Expr> {
    let e = match p.peek().cloned() {
        Some(Tok::Str(s)) => Expr::Terminal(s),
        Some(Tok::Ident(s)) => Expr::NonTerminal(s),
        Some(Tok::Kw(".")) => Expr::Special(".".into()),
        Some(Tok::Kw("(")) => {
            p.pos += 1;
            let inner = peg_choice(p)?;
            p.expect_kw(")")?;
            return Ok(inner);
        }
        _ => return Err(p.err("a PEG primary")),
    };
    p.pos += 1;
    Ok(e)
}

// ───────────────────────────── layout ─────────────────────────────

/// Geometry settings (`railroad` config, upstream defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub padding: f64,
    pub vertical_separation: f64,
    pub horizontal_separation: f64,
    pub arc_radius: f64,
    pub font_size: f64,
    pub marker_radius: f64,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            padding: 10.0,
            vertical_separation: 8.0,
            horizontal_separation: 10.0,
            arc_radius: 10.0,
            font_size: 16.0,
            marker_radius: 5.0,
        }
    }
}

impl Metrics {
    /// `getBBox()` of the label: Arial-metric advance widths, and the browser's
    /// (hinted) line box height measured for common sizes.
    fn measure(&self, text: &str) -> (f64, f64) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let w = line_width(&display_text(text), self.font_size, false);
        let fs = self.font_size;
        let h = match fs as i64 {
            10 | 12 | 14 | 16 | 18 | 20 | 24 | 32 if fs.fract() == 0.0 => match fs as i64 {
                10 => 11.0,
                12 => 14.0,
                14 => 16.0,
                16 => 17.0,
                18 => 20.0,
                20 => 22.0,
                24 => 27.0,
                _ => 36.0,
            },
            _ => (fs * 1.125).round(),
        };
        // The hinted ink of deep descenders overflows that box by a pixel at the
        // sizes measured (16px: `g j p q y ( ) [ ] { } | _ Q ç`; 24px: `| ç`).
        let deep = match fs {
            16.0 => "gjpqy()[]{}|_Qç",
            24.0 => "|ç",
            _ => "",
        };
        let h = if text.chars().any(|c| deep.contains(c)) {
            h + 1.0
        } else {
            h
        };
        (w, h)
    }
}

/// SVG collapses control whitespace in `<text>`.
fn display_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect()
}

/// Kind of labelled box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Terminal,
    NonTerminal,
    Special,
}

/// One rail path segment (SVG path command).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move(Point),
    Line(Point),
    /// Small circular arc of `r` to `to`; `sweep` as SVG's sweep flag.
    Arc {
        to: Point,
        r: f64,
        sweep: bool,
    },
}

/// A positioned drawing primitive.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Box {
        kind: BoxKind,
        rect: Rect,
        label: String,
    },
    Rail(Vec<Seg>),
}

impl Prim {
    fn shifted(mut self, dx: f64, dy: f64) -> Prim {
        let mv = |p: &mut Point| {
            p.x += dx;
            p.y += dy;
        };
        match &mut self {
            Prim::Box { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
            Prim::Rail(segs) => {
                for s in segs {
                    match s {
                        Seg::Move(p) | Seg::Line(p) | Seg::Arc { to: p, .. } => mv(p),
                    }
                }
            }
        }
        self
    }
}

/// A laid-out expression in local coordinates (`(0,0)` top-left); the rail
/// enters at `(0, up)` and leaves at `(width, up)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub width: f64,
    pub height: f64,
    pub up: f64,
    pub prims: Vec<Prim>,
}

impl Laid {
    pub fn down(&self) -> f64 {
        self.height - self.up
    }
    fn place(&mut self, child: Laid, dx: f64, dy: f64) {
        self.prims
            .extend(child.prims.into_iter().map(|p| p.shifted(dx, dy)));
    }
}

/// Builder mirroring upstream's `PathBuilder`.
struct Path(Vec<Seg>);

impl Path {
    fn m(x: f64, y: f64) -> Path {
        Path(vec![Seg::Move(Point::new(x, y))])
    }
    fn l(mut self, x: f64, y: f64) -> Path {
        self.0.push(Seg::Line(Point::new(x, y)));
        self
    }
    fn a(mut self, r: f64, sweep: bool, x: f64, y: f64) -> Path {
        self.0.push(Seg::Arc {
            to: Point::new(x, y),
            r,
            sweep,
        });
        self
    }
    fn prim(self) -> Prim {
        Prim::Rail(self.0)
    }
}

fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Prim {
    Path::m(x1, y1).l(x2, y2).prim()
}

fn layout_box(kind: BoxKind, label: String, m: &Metrics) -> Laid {
    let (tw, th) = m.measure(&label);
    let width = tw + m.padding * 2.0;
    let height = th + m.padding * 2.0;
    Laid {
        width,
        height,
        up: height / 2.0,
        prims: vec![Prim::Box {
            kind,
            rect: Rect::new(0.0, 0.0, width, height),
            label,
        }],
    }
}

/// Lay out one expression (`renderExpression`).
pub fn layout(e: &Expr, m: &Metrics) -> Laid {
    match e {
        Expr::Terminal(v) => layout_box(BoxKind::Terminal, v.clone(), m),
        Expr::NonTerminal(n) => layout_box(BoxKind::NonTerminal, n.clone(), m),
        Expr::Special(t) => layout_box(BoxKind::Special, format!("? {t} ?"), m),
        Expr::Sequence(es) => layout_sequence(es, m),
        Expr::Choice(alts) => layout_choice(alts, m),
        Expr::Optional(inner) => layout_optional(inner, m),
        Expr::Repetition { element, min, .. } => layout_repetition(element, *min == 0, m),
    }
}

fn layout_sequence(es: &[Expr], m: &Metrics) -> Laid {
    let kids: Vec<Laid> = es.iter().map(|e| layout(e, m)).collect();
    let gap = m.horizontal_separation;
    let max_up = kids.iter().map(|k| k.up).fold(0.0, f64::max);
    let max_down = kids.iter().map(Laid::down).fold(0.0, f64::max);
    let width =
        kids.iter().map(|k| k.width).sum::<f64>() + kids.len().saturating_sub(1) as f64 * gap;
    let mut out = Laid {
        width,
        height: max_up + max_down,
        up: max_up,
        prims: Vec::new(),
    };
    let n = kids.len();
    let mut x = 0.0;
    for (i, k) in kids.into_iter().enumerate() {
        let w = k.width;
        let y = max_up - k.up;
        out.place(k, x, y);
        if i + 1 < n {
            out.prims.push(line(x + w, max_up, x + w + gap, max_up));
        }
        x += w + gap;
    }
    out
}

fn layout_choice(alts: &[Expr], m: &Metrics) -> Laid {
    let kids: Vec<Laid> = alts.iter().map(|e| layout(e, m)).collect();
    let r = m.arc_radius;
    let max_w = kids.iter().map(|k| k.width).fold(0.0, f64::max);
    let total_h = kids.iter().map(|k| k.height).sum::<f64>()
        + kids.len().saturating_sub(1) as f64 * m.vertical_separation;
    let total_w = max_w + r * 4.0;
    let cy = total_h / 2.0;
    let mut out = Laid {
        width: total_w,
        height: total_h,
        up: cy,
        prims: Vec::new(),
    };
    let mut y = 0.0;
    for k in kids {
        let (w, h) = (k.width, k.height);
        let ecy = y + k.up;
        let ex = r * 2.0 + (max_w - w) / 2.0;
        out.place(k, ex, y);
        let below = ecy > cy;
        let s = if below { 1.0 } else { -1.0 };
        let left = if ecy == cy {
            Path::m(0.0, cy).l(ex, ecy)
        } else {
            Path::m(0.0, cy)
                .a(r, below, r, cy + s * r)
                .l(r, ecy - s * r)
                .a(r, !below, r * 2.0, ecy)
                .l(ex, ecy)
        };
        out.prims.push(left.prim());
        let rs = ex + w;
        let lane = total_w - r * 2.0;
        let right = if ecy == cy {
            Path::m(rs, ecy).l(total_w, cy)
        } else {
            Path::m(rs, ecy)
                .l(lane, ecy)
                .a(r, !below, total_w - r, ecy - s * r)
                .l(total_w - r, cy + s * r)
                .a(r, below, total_w, cy)
        };
        out.prims.push(right.prim());
        y += h + m.vertical_separation;
    }
    out
}

/// Bypass rail over the top of a `width`-wide construct whose rail is at `cy`.
fn bypass(width: f64, cy: f64, r: f64) -> Prim {
    Path::m(0.0, cy)
        .a(r, false, r, cy - r)
        .l(r, r)
        .a(r, true, r * 2.0, 0.0)
        .l(width - r * 2.0, 0.0)
        .a(r, true, width - r, r)
        .l(width - r, cy - r)
        .a(r, false, width, cy)
        .prim()
}

fn layout_optional(inner: &Expr, m: &Metrics) -> Laid {
    let k = layout(inner, m);
    let r = m.arc_radius;
    let width = k.width + r * 4.0;
    let height = k.height + r * 2.0;
    let (ex, ey) = (r * 2.0, r * 2.0);
    let cy = ey + k.up;
    let kw = k.width;
    let mut out = Laid {
        width,
        height,
        up: cy,
        prims: Vec::new(),
    };
    out.place(k, ex, ey);
    out.prims.push(line(0.0, cy, r * 2.0, cy));
    out.prims.push(line(ex + kw, cy, width, cy));
    out.prims.push(bypass(width, cy, r));
    out
}

fn layout_repetition(inner: &Expr, has_bypass: bool, m: &Metrics) -> Laid {
    let k = layout(inner, m);
    let r = m.arc_radius;
    let width = k.width + r * 4.0;
    let height = k.height + r * 2.0 + if has_bypass { r * 2.0 } else { 0.0 };
    let ex = r * 2.0;
    let ey = if has_bypass { r * 2.0 } else { 0.0 };
    let cy = ey + k.up;
    let (kw, kh) = (k.width, k.height);
    let mut out = Laid {
        width,
        height,
        up: cy,
        prims: Vec::new(),
    };
    out.place(k, ex, ey);
    out.prims.push(line(0.0, cy, r * 2.0, cy));
    out.prims.push(line(ex + kw, cy, width, cy));
    let loop_y = ey + kh + r;
    out.prims.push(
        Path::m(ex + kw, cy)
            .a(r, true, ex + kw + r, cy + r)
            .l(ex + kw + r, loop_y)
            .a(r, true, ex + kw, loop_y + r)
            .l(r * 2.0, loop_y + r)
            .a(r, true, r, loop_y)
            .l(r, cy + r)
            .a(r, true, r * 2.0, cy)
            .prim(),
    );
    if has_bypass {
        out.prims.push(bypass(width, cy, r));
    }
    out
}

/// A laid-out rule, relative to the rule's own top-left.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleLayout {
    pub label: String,
    pub baseline: f64,
    pub start: Point,
    pub end: Point,
    pub prims: Vec<Prim>,
    pub width: f64,
    pub height: f64,
}

/// `renderRule`.
pub fn layout_rule(rule: &Rule, m: &Metrics) -> RuleLayout {
    let label = format!("{} =", rule.name);
    let name_w = m.measure(&label).0 + 20.0;
    let def_x = name_w + 20.0;
    let def = layout(&rule.definition, m);
    let baseline = def.up.max(20.0);
    let def_y = baseline - def.up;
    let (w, h) = (def.width, def.height);
    let mut prims: Vec<Prim> = def
        .prims
        .into_iter()
        .map(|p| p.shifted(def_x, def_y))
        .collect();
    let mr = m.marker_radius;
    prims.push(line(name_w + mr, baseline, def_x, baseline));
    prims.push(line(def_x + w, baseline, def_x + w + 10.0 - mr, baseline));
    RuleLayout {
        label,
        baseline,
        start: Point::new(name_w, baseline),
        end: Point::new(def_x + w + 10.0, baseline),
        prims,
        width: def_x + w + 10.0 + mr,
        height: (def_y + h + m.padding * 2.0).max(40.0),
    }
}

/// `renderDiagram`: rules stacked with `verticalSeparation`; returns each rule's
/// y offset and the canvas size.
pub fn layout_diagram(rules: &[Rule], m: &Metrics) -> (Vec<(f64, RuleLayout)>, f64, f64) {
    if rules.is_empty() {
        return (Vec::new(), 200.0, 100.0);
    }
    let mut y = m.padding;
    let mut max_w: f64 = 0.0;
    let mut out = Vec::with_capacity(rules.len());
    for r in rules {
        let rl = layout_rule(r, m);
        let h = rl.height;
        max_w = max_w.max(rl.width);
        out.push((y, rl));
        y += h + m.vertical_separation;
    }
    (out, max_w + m.padding * 2.0, y + m.padding)
}

// ───────────────────────────── style ─────────────────────────────

/// Resolved colours and strokes (`buildRailroadStyleOptions`).
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub terminal_fill: Color,
    pub terminal_stroke: Color,
    pub terminal_text: Color,
    pub non_terminal_fill: Color,
    pub non_terminal_stroke: Color,
    pub non_terminal_text: Color,
    pub line: Color,
    pub marker: Color,
    pub special_fill: Color,
    pub special_stroke: Color,
    pub rule_name: Color,
    pub stroke_width: f64,
    pub font_family: String,
}

fn norm(s: &str) -> Color {
    match color::parse(s) {
        Some(c) if c.a >= 1.0 => Color::new(c.to_hex()),
        _ => Color::new(s),
    }
}

/// Theme-derived defaults for the classic and neo/redux themes, as mermaid 12.3
/// resolves them (`secondBkg`, `secondaryBorderColor`, `primaryBorderColor`, …).
/// Order: terminal fill/stroke/text, non-terminal fill/stroke/text, line,
/// special fill/stroke, rule name.
fn theme_preset(theme: ThemeName) -> [&'static str; 10] {
    match theme {
        ThemeName::Dark => [
            "hsl(180, 1.5873015873%, 28.3529411765%)",
            "hsl(180, 0%, 18.3529411765%)",
            "rgb(183.8476190475, 181.5523809523, 181.5523809523)",
            "#1f2020",
            "#cccccc",
            "#e0dfdf",
            "lightgrey",
            "hsl(20, 1.5873015873%, 12.3529411765%)",
            "hsl(20, 0%, 2.3529411765%)",
            "#F9FFFE",
        ],
        ThemeName::Forest => [
            "#cdffb2",
            "hsl(98.961038961, 60%, 74.9019607843%)",
            "#32004d",
            "#cde498",
            "hsl(78.1578947368, 18.4615384615%, 64.5098039216%)",
            "#321b67",
            "#000000",
            "hsl(78.1578947368, 58.4615384615%, 84.5098039216%)",
            "hsl(78.1578947368, 18.4615384615%, 74.5098039216%)",
            "#333",
        ],
        ThemeName::Neutral => [
            "hsl(0, 0%, 98.9215686275%)",
            "hsl(0, 0%, 88.9215686275%)",
            "rgb(2.7499999999, 2.7499999999, 2.7499999999)",
            "#eee",
            "hsl(0, 0%, 83.3333333333%)",
            "#111111",
            "#666",
            "hsl(-160, 0%, 93.3333333333%)",
            "hsl(-160, 0%, 83.3333333333%)",
            "#333",
        ],
        ThemeName::Neo => [
            "#ffffde",
            "hsl(-120, 0%, 70%)",
            "#333333",
            "#ffffff",
            "hsl(0, 0%, 70%)",
            "#333",
            "#000000",
            "#ffffff",
            "hsl(0, 0%, 90%)",
            "#000000",
        ],
        ThemeName::Redux | ThemeName::ReduxColor => [
            "#ffffde",
            "hsl(-120, 0%, 70%)",
            "#333333",
            "#ffffff",
            "hsl(247.5, 0%, 9.2156862745%)",
            "#28253D",
            "#000000",
            "#ffffff",
            "hsl(0, 0%, 90%)",
            "#000000",
        ],
        ThemeName::NeoDark => [
            "hsl(0, 13.5135135135%, 30.5098039216%)",
            "hsl(180, 0%, 18.3529411765%)",
            "rgb(183.8476190475, 181.5523809523, 181.5523809523)",
            "#2a2020",
            "#cccccc",
            "#e0dfdf",
            "#cccccc",
            "hsl(20, 1.5873015873%, 12.3529411765%)",
            "hsl(20, 0%, 2.3529411765%)",
            "rgb(222.9999999999, 223.6666666666, 223.9999999999)",
        ],
        ThemeName::ReduxDark | ThemeName::ReduxDarkColor => [
            "hsl(240, 5.5555555556%, 23.0588235294%)",
            "hsl(180, 0%, 18.3529411765%)",
            "rgb(183.8476190475, 181.5523809523, 181.5523809523)",
            "#111113",
            "#cccccc",
            "#e0dfdf",
            "#cccccc",
            "hsl(20, 1.5873015873%, 12.3529411765%)",
            "hsl(20, 0%, 2.3529411765%)",
            "rgb(222.9999999999, 223.6666666666, 223.9999999999)",
        ],
        ThemeName::Default | ThemeName::Null | ThemeName::Base => [
            "#ffffde",
            "hsl(60, 60%, 83.5294117647%)",
            "#000021",
            "#ECECFF",
            "hsl(240, 60%, 86.2745098039%)",
            "#131300",
            "#333333",
            "hsl(80, 100%, 96.2745098039%)",
            "hsl(80, 60%, 86.2745098039%)",
            "#333",
        ],
    }
}

/// `theme-base`'s derivations from `primaryColor` (`mkBorder`, `invert`, …).
fn base_theme(var: &dyn Fn(&str) -> Option<Rgba>, dark: bool) -> [Color; 10] {
    let mk_border = |c: Rgba| {
        if dark {
            c.adjust_hsl(0.0, -40.0, 10.0)
        } else {
            c.adjust_hsl(0.0, -40.0, -10.0)
        }
    };
    let h = |c: Rgba| Color::new(c.to_hex());
    let primary = var("primaryColor").unwrap_or(Rgba {
        r: 0xff,
        g: 0xf4,
        b: 0xdd,
        a: 1.0,
    });
    let background = var("background").unwrap_or(Rgba {
        r: 0xf4,
        g: 0xf4,
        b: 0xf4,
        a: 1.0,
    });
    let secondary = var("secondaryColor").unwrap_or(primary.adjust_hsl(-120.0, 0.0, 0.0));
    let tertiary = var("tertiaryColor").unwrap_or(primary.adjust_hsl(180.0, 0.0, 5.0));
    let tertiary_text = var("tertiaryTextColor").unwrap_or(tertiary.invert());
    [
        // secondBkg is not a base-theme variable: the default theme's value shows through.
        Color::new("#ffffde"),
        h(var("secondaryBorderColor").unwrap_or(mk_border(secondary))),
        h(var("secondaryTextColor").unwrap_or(secondary.invert())),
        h(primary),
        h(var("primaryBorderColor").unwrap_or(mk_border(primary))),
        Color::new(if dark { "#eee" } else { "#333" }),
        h(var("lineColor").unwrap_or(background.invert())),
        h(tertiary),
        h(var("tertiaryBorderColor").unwrap_or(mk_border(tertiary))),
        h(tertiary_text),
    ]
}

impl Style {
    /// Theme defaults, then `themeVariables`, then the `railroad` config block.
    pub fn resolve(cfg: &MermaidConfig, pal: &Palette) -> Style {
        let vars = &cfg.theme_variables;
        let raw = |k: &str| {
            vars.iter()
                .rev()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.trim().to_string())
        };
        let mut c: [Color; 10] = if cfg.theme == ThemeName::Base {
            let dark = raw("darkMode").is_some_and(|v| v == "true");
            base_theme(&|k| raw(k).and_then(|v| color::parse(&v)), dark)
        } else {
            theme_preset(cfg.theme).map(norm)
        };
        // Direct theme-variable overrides (the variables the style builder reads).
        let theme_keys: [&[&str]; 10] = [
            &["secondBkg"],
            &["secondaryBorderColor"],
            &["secondaryTextColor"],
            &["mainBkg", "primaryColor"],
            &["primaryBorderColor"],
            &["primaryTextColor"],
            &["lineColor"],
            &["tertiaryColor"],
            &["tertiaryBorderColor"],
            &["titleColor"],
        ];
        if cfg.theme != ThemeName::Base {
            for (slot, keys) in c.iter_mut().zip(theme_keys) {
                if let Some(v) = keys.iter().find_map(|k| raw(k)) {
                    *slot = Color::new(v);
                }
            }
        }
        let [tf, ts, tt, nf, ns, nt, line, sf, ss, rn] = c;
        let mut st = Style {
            terminal_fill: tf,
            terminal_stroke: ts,
            terminal_text: tt,
            non_terminal_fill: nf,
            non_terminal_stroke: ns,
            non_terminal_text: nt,
            marker: line.clone(),
            line,
            special_fill: sf,
            special_stroke: ss,
            rule_name: rn,
            stroke_width: 2.0,
            font_family: pal.font_family.clone(),
        };
        let opt = |k: &str| {
            cfg.section("railroad", k)
                .and_then(|j| j.as_str().map(str::trim).map(str::to_string))
                .filter(|v| is_color_value(v))
                .map(Color::new)
        };
        let set = |slot: &mut Color, k: &str| {
            if let Some(v) = opt(k) {
                *slot = v;
            }
        };
        set(&mut st.terminal_fill, "terminalFill");
        set(&mut st.terminal_stroke, "terminalStroke");
        set(&mut st.terminal_text, "terminalTextColor");
        set(&mut st.non_terminal_fill, "nonTerminalFill");
        set(&mut st.non_terminal_stroke, "nonTerminalStroke");
        set(&mut st.non_terminal_text, "nonTerminalTextColor");
        set(&mut st.line, "lineColor");
        set(&mut st.marker, "markerFill");
        set(&mut st.special_fill, "specialFill");
        set(&mut st.special_stroke, "specialStroke");
        set(&mut st.rule_name, "ruleNameColor");
        st.stroke_width = non_negative(cfg, "strokeWidth").unwrap_or(2.0);
        if let Some(f) = cfg
            .section("railroad", "fontFamily")
            .and_then(|j| j.as_str())
            .map(str::trim)
            .filter(|f| {
                !f.is_empty()
                    && f.chars()
                        .all(|c| c.is_alphanumeric() || " _\"',.-".contains(c))
            })
        {
            st.font_family = f.to_string();
        }
        st
    }
}

/// `COLOR_VALUE_PATTERN`: hex, functional notation or a plain colour name.
fn is_color_value(v: &str) -> bool {
    if let Some(h) = v.strip_prefix('#') {
        return matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit());
    }
    if let Some(open) = v.find('(') {
        let f = v[..open].to_ascii_lowercase();
        let args = &v[open + 1..];
        return [
            "rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch",
        ]
        .contains(&f.as_str())
            && args.ends_with(')')
            && args[..args.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || " \t%+,./-".contains(c));
    }
    !v.is_empty() && v.chars().all(|c| c.is_ascii_alphabetic())
}

/// `sanitizeNumberValue`: finite and `>= 0`, else the default.
fn non_negative(cfg: &MermaidConfig, key: &str) -> Option<f64> {
    let j = cfg.section("railroad", key)?;
    let v = j.as_f64().or_else(|| {
        j.as_str().and_then(|s| {
            let s = s.trim();
            let end = s
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
                .unwrap_or(s.len());
            s[..end].parse::<f64>().ok()
        })
    })?;
    (v.is_finite() && v >= 0.0).then_some(v)
}

impl Metrics {
    /// Geometry from the `railroad` config block over the theme font size.
    pub fn from_config(cfg: &MermaidConfig, pal: &Palette) -> Metrics {
        let d = Metrics::default();
        let font_size = if pal.font_size > 0.0 {
            pal.font_size
        } else {
            d.font_size
        };
        Metrics {
            padding: non_negative(cfg, "padding").unwrap_or(d.padding),
            vertical_separation: non_negative(cfg, "verticalSeparation")
                .unwrap_or(d.vertical_separation),
            horizontal_separation: non_negative(cfg, "horizontalSeparation")
                .unwrap_or(d.horizontal_separation),
            arc_radius: non_negative(cfg, "arcRadius").unwrap_or(d.arc_radius),
            font_size: non_negative(cfg, "fontSize").unwrap_or(font_size),
            marker_radius: non_negative(cfg, "markerRadius").unwrap_or(d.marker_radius),
        }
    }
}

// ───────────────────────────── drawing ─────────────────────────────

/// Cubic approximation of an SVG small-arc command (`A r r 0 0 sweep x y`).
fn arc_cmds(from: Point, to: Point, r: f64, sweep: bool) -> Vec<PathCmd> {
    let d = to.minus(from);
    let len = d.len();
    if len == 0.0 {
        return Vec::new();
    }
    if r <= 0.0 {
        return vec![PathCmd::LineTo(to)];
    }
    let half = len / 2.0;
    let r = r.max(half);
    let h = (r * r - half * half).max(0.0).sqrt();
    let mid = from.lerp(to, 0.5);
    let n = Point::new(-d.y / len, d.x / len);
    let norm_angle = |mut a: f64| {
        use std::f64::consts::PI;
        while a > PI {
            a -= 2.0 * PI;
        }
        while a <= -PI {
            a += 2.0 * PI;
        }
        a
    };
    for c in [mid.plus(n.scale(h)), mid.minus(n.scale(h))] {
        let a0 = (from.y - c.y).atan2(from.x - c.x);
        let a1 = (to.y - c.y).atan2(to.x - c.x);
        let delta = norm_angle(a1 - a0);
        if (delta > 0.0) == sweep {
            let mut cmds = arc_cubics(c, r, a0, a0 + delta);
            // Land exactly on the target.
            if let Some(PathCmd::CubicTo(_, _, p)) = cmds.last_mut() {
                *p = to;
            }
            return cmds;
        }
    }
    vec![PathCmd::LineTo(to)]
}

fn rail_cmds(segs: &[Seg]) -> Vec<PathCmd> {
    let mut cur = Point::new(0.0, 0.0);
    let mut out = Vec::with_capacity(segs.len() * 2);
    for s in segs {
        match *s {
            Seg::Move(p) => {
                out.push(PathCmd::MoveTo(p));
                cur = p;
            }
            Seg::Line(p) => {
                out.push(PathCmd::LineTo(p));
                cur = p;
            }
            Seg::Arc { to, r, sweep } => {
                out.extend(arc_cmds(cur, to, r, sweep));
                cur = to;
            }
        }
    }
    out
}

fn draw_prim(p: &Prim, st: &Style, m: &Metrics) -> Element {
    match p {
        Prim::Rail(segs) => Element::classed_shape(
            Geometry::Path(rail_cmds(segs)),
            Paint::stroke(&st.line, st.stroke_width),
            "railroad-line",
        ),
        Prim::Box { kind, rect, label } => {
            let (class, fill, stroke, text, rx) = match kind {
                BoxKind::Terminal => (
                    "railroad-terminal",
                    &st.terminal_fill,
                    &st.terminal_stroke,
                    &st.terminal_text,
                    10.0,
                ),
                BoxKind::NonTerminal => (
                    "railroad-nonterminal",
                    &st.non_terminal_fill,
                    &st.non_terminal_stroke,
                    &st.non_terminal_text,
                    0.0,
                ),
                BoxKind::Special => (
                    "railroad-special",
                    &st.special_fill,
                    &st.special_stroke,
                    &st.non_terminal_text,
                    0.0,
                ),
            };
            let mut paint = Paint::both(fill, stroke, st.stroke_width);
            if *kind == BoxKind::Special {
                paint = paint.dashed(vec![5.0, 3.0]);
            }
            let lh = m.font_size * 1.5;
            Element::group(
                class,
                vec![
                    Element::shape(Geometry::Rect { rect: *rect, rx }, paint),
                    Element::text(Text::centered(
                        rect.center(),
                        &[display_text(label)],
                        m.font_size,
                        lh,
                        text,
                    )),
                ],
            )
        }
    }
}

/// Draw a parsed grammar.
pub fn draw(g: &Grammar, m: &Metrics, st: &Style) -> Scene {
    let (rules, width, height) = layout_diagram(&g.rules, m);
    let mut els = Vec::with_capacity(rules.len());
    for (y, rl) in &rules {
        let mut kids: Vec<Element> = rl
            .prims
            .iter()
            .map(|p| draw_prim(&p.clone().shifted(0.0, *y), st, m))
            .collect();
        kids.push(Element::classed_text(
            Text {
                x: 0.0,
                y: y + rl.baseline,
                lines: vec![display_text(&rl.label)],
                line_height: m.font_size * 1.5,
                font_size: m.font_size,
                bold: true,
                anchor: Anchor::Start,
                fill: st.rule_name.clone(),
            },
            "railroad-rule-name",
        ));
        for (pt, class) in [(rl.start, "railroad-start"), (rl.end, "railroad-end")] {
            kids.push(Element::group(
                class,
                vec![Element::shape(
                    Geometry::Circle {
                        c: Point::new(pt.x, pt.y + y),
                        r: m.marker_radius,
                    },
                    Paint::fill(&st.marker),
                )],
            ));
        }
        els.push(Element::group("railroad-rule", kids));
    }
    Scene {
        kind: g.syntax.kind(),
        view_box: Rect::new(0.0, 0.0, width, height),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: st.font_family.clone(),
        css: String::new(),
    }
}

/// Parse, lay out and draw a railroad diagram (any of the four notations).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let g = parse(body, title)?;
    Ok(draw(
        &g,
        &Metrics::from_config(cfg, pal),
        &Style::resolve(cfg, pal),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(src: &str) -> Vec<Rule> {
        parse(src, None).unwrap().rules
    }
    fn def(src: &str) -> Expr {
        rules(src).remove(0).definition
    }
    fn t(s: &str) -> Expr {
        Expr::Terminal(s.into())
    }
    fn nt(s: &str) -> Expr {
        Expr::NonTerminal(s.into())
    }

    // ── IR (railroadDiagram.spec.ts) ──
    #[test]
    fn ir_primitives() {
        assert_eq!(
            def("railroad-beta\nrule = terminal(\"hello\") ;"),
            t("hello")
        );
        assert_eq!(
            def("railroad-beta\nrule = nonterminal(\"expression\") ;"),
            nt("expression")
        );
        assert_eq!(
            def("railroad-beta\nr = special(\"any character\") ;"),
            Expr::Special("any character".into())
        );
        assert_eq!(
            def("railroad-beta\nr = sequence(terminal(\"a\"), terminal(\"b\")) ;"),
            Expr::Sequence(vec![t("a"), t("b")])
        );
        // Single-element sequences and choices collapse.
        assert_eq!(
            def("railroad-beta\nr = sequence(terminal(\"a\")) ;"),
            t("a")
        );
        assert_eq!(def("railroad-beta\nr = choice(terminal(\"a\")) ;"), t("a"));
        assert_eq!(
            def("railroad-beta\nr = choice(terminal('a'), terminal('b')) ;"),
            Expr::Choice(vec![t("a"), t("b")])
        );
        assert_eq!(
            def("railroad-beta\nr = optional(terminal(\"a\")) ;"),
            Expr::Optional(Box::new(t("a")))
        );
        assert_eq!(
            def("railroad-beta\nr = zeroOrMore(terminal(\"a\")) ;"),
            Expr::repeat(t("a"), 0, None)
        );
        assert_eq!(
            def("railroad-beta\nr = oneOrMore(terminal(\"a\")) ;"),
            Expr::repeat(t("a"), 1, None)
        );
    }

    #[test]
    fn ir_document() {
        let g = parse(
            "railroad-beta\ntitle \"Test Grammar\"\nrule1 = terminal(\"a\") ;\n/* note */ my_rule = terminal(\"hello world\") ;\nrule123 = zeroOrMore(choice(terminal(\"x\"), nonterminal(\"y\"))) ; %% c\n",
            None,
        )
        .unwrap();
        assert_eq!(g.syntax, Syntax::Ir);
        assert_eq!(g.title.as_deref(), Some("Test Grammar"));
        let names: Vec<_> = g.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["rule1", "my_rule", "rule123"]);
        assert_eq!(g.rules[1].definition, t("hello world"));
        assert!(matches!(
            &g.rules[2].definition,
            Expr::Repetition { element, min: 0, .. } if matches!(**element, Expr::Choice(_))
        ));
        // Empty grammar with a title.
        let g = parse("railroad-beta\ntitle Empty Grammar\n", None).unwrap();
        assert_eq!(g.title.as_deref(), Some("Empty Grammar"));
        assert!(g.rules.is_empty());
        // Escapes and case-insensitive header.
        assert_eq!(
            def("RAILROAD-BETA\nr = terminal(\"a\\\"b\\n\") ;"),
            t("a\"b\n")
        );
    }

    #[test]
    fn ir_errors() {
        assert!(parse("rule = terminal(\"a\") ;", None).is_err());
        assert!(parse("railroad-beta\nrule = terminal(\"a\")", None).is_err());
        assert!(parse("railroad-beta\nrule = terminal(\"a) ;", None).is_err());
        // Keywords are not rule names.
        assert!(parse("railroad-beta\nchoice = terminal(\"a\") ;", None).is_err());
        // …but longer identifiers are.
        assert_eq!(
            rules("railroad-beta\nchoices = terminal(\"a\") ;")[0].name,
            "choices"
        );
        let e = parse(
            "railroad-beta\nr = terminal(\"a\")\ns = terminal(\"b\") ;",
            None,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
    }

    // ── EBNF (ebnfDiagram.spec.ts) ──
    #[test]
    fn ebnf_basics() {
        let h = "railroad-ebnf-beta\n";
        assert_eq!(def(&format!("{h}rule = \"terminal\" ;")), t("terminal"));
        assert_eq!(def(&format!("{h}rule = other_rule ;")), nt("other_rule"));
        assert_eq!(
            def(&format!("{h}rule = \"a\" \"b\" , \"c\" ;")),
            Expr::Sequence(vec![t("a"), t("b"), t("c")])
        );
        assert_eq!(
            def(&format!("{h}rule ::= \"a\" | \"b\" | \"c\" ;")),
            Expr::Choice(vec![t("a"), t("b"), t("c")])
        );
        let opt = Expr::Optional(Box::new(t("a")));
        assert_eq!(def(&format!("{h}rule = \"a\"? ;")), opt);
        assert_eq!(def(&format!("{h}rule = [ \"a\" ] ;")), opt);
        assert_eq!(
            def(&format!("{h}rule = \"a\"* ;")),
            Expr::repeat(t("a"), 0, None)
        );
        assert_eq!(
            def(&format!("{h}rule = \"a\"+ ;")),
            Expr::repeat(t("a"), 1, None)
        );
        assert_eq!(
            def(&format!("{h}rule = {{ \"a\" }} ;")),
            Expr::repeat(t("a"), 0, None)
        );
        assert_eq!(
            def(&format!("{h}rule = ? special ? ;")),
            Expr::Special("special".into())
        );
        assert_eq!(
            def(&format!("{h}rule = letter - digit ;")),
            Expr::Sequence(vec![nt("letter"), t("-"), nt("digit")])
        );
        // Grouping collapses, comments are hidden.
        assert_eq!(
            def(&format!("{h}r = (* iso *) ( \"a\" ) /* w3c */ ;")),
            t("a")
        );
        // `?` after a primary is the postfix, not a special sequence opener.
        assert_eq!(
            def(&format!("{h}number = sign? digit+ ;")),
            Expr::Sequence(vec![
                Expr::Optional(Box::new(nt("sign"))),
                Expr::repeat(nt("digit"), 1, None)
            ])
        );
    }

    #[test]
    fn ebnf_grammar_and_errors() {
        let g = parse(
            "railroad-ebnf-beta\ntitle \"Simple Expression\"\nexpression = term ( \"+\" term | \"-\" term )* ;\nterm = factor ( \"*\" factor | \"/\" factor )* ;\nfactor = number | \"(\" expression \")\" ;\nnumber = digit+ ;\ndigit = \"0\" | \"1\" | \"2\" ;\n",
            None,
        )
        .unwrap();
        assert_eq!(g.rules.len(), 5);
        assert_eq!(g.title.as_deref(), Some("Simple Expression"));
        assert_eq!(g.syntax.kind().role(), "railroadEbnf");
        assert!(parse("rule = \"a\" ;", None).is_err());
        assert!(parse("railroad-ebnf-beta\nrule = \"a\"", None).is_err());
    }

    // ── ABNF (abnfDiagram.spec.ts) ──
    #[test]
    fn abnf_basics() {
        let h = "railroad-abnf-beta\n";
        assert_eq!(def(&format!("{h}rule = \"hello\" ;")), t("hello"));
        assert_eq!(def(&format!("{h}rule = %x41 ;")), t("%x41"));
        assert_eq!(def(&format!("{h}rule = %x30-39 ;")), t("%x30-39"));
        assert_eq!(def(&format!("{h}rule = other-rule ;")), nt("other-rule"));
        assert_eq!(
            def(&format!("{h}rule = \"a\" \"b\" \"c\" ;")),
            Expr::Sequence(vec![t("a"), t("b"), t("c")])
        );
        assert_eq!(
            def(&format!("{h}rule = \"a\" / \"b\" / \"c\" ;")),
            Expr::Choice(vec![t("a"), t("b"), t("c")])
        );
        assert_eq!(
            def(&format!("{h}rule = [ \"a\" ] ;")),
            Expr::Optional(Box::new(t("a")))
        );
        assert_eq!(
            def(&format!("{h}rule = *\"a\" ;")),
            Expr::repeat(t("a"), 0, None)
        );
        assert_eq!(
            def(&format!("{h}rule = 1*\"a\" ;")),
            Expr::repeat(t("a"), 1, None)
        );
        assert_eq!(
            def(&format!("{h}rule = 2*4\"a\" ;")),
            Expr::repeat(t("a"), 2, Some(4))
        );
        assert_eq!(
            def(&format!("{h}rule = 3DIGIT ;")),
            Expr::repeat(nt("DIGIT"), 3, Some(3))
        );
        assert_eq!(
            def(&format!("{h}rule = *1\"a\" ;")),
            Expr::Optional(Box::new(t("a")))
        );
    }

    #[test]
    fn abnf_comments_and_errors() {
        let g = parse(
            "railroad-abnf-beta\ntitle \"URI\"\n; leading comment\nURI = scheme \":\" hier-part ;\nscheme = ALPHA *( ALPHA / DIGIT / \"+\" / \"-\" / \".\" ) ;\nhier-part = \"//\" authority path-abempty ;\n",
            None,
        )
        .unwrap();
        let names: Vec<_> = g.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["URI", "scheme", "hier-part"]);
        assert_eq!(g.title.as_deref(), Some("URI"));
        // A `;` followed by text on the same line is a comment (as upstream), so
        // the rule is left unterminated.
        assert!(parse("railroad-abnf-beta\na = \"x\" ; note\nb = \"y\" ;", None).is_err());
        assert!(parse("rule = \"a\" ;", None).is_err());
        assert!(parse("railroad-abnf-beta\nrule = \"a\"", None).is_err());
    }

    // ── PEG (pegDiagram.spec.ts) ──
    #[test]
    fn peg_basics() {
        let h = "railroad-peg-beta\n";
        assert_eq!(def(&format!("{h}rule <- \"hello\" ;")), t("hello"));
        assert_eq!(def(&format!("{h}rule <- other ;")), nt("other"));
        assert_eq!(
            def(&format!("{h}rule <- \"a\" \"b\" ;")),
            Expr::Sequence(vec![t("a"), t("b")])
        );
        assert_eq!(
            def(&format!("{h}rule <- \"a\" / \"b\" ;")),
            Expr::Choice(vec![t("a"), t("b")])
        );
        assert_eq!(
            def(&format!("{h}rule <- \"a\"? ;")),
            Expr::Optional(Box::new(t("a")))
        );
        assert_eq!(
            def(&format!("{h}rule <- \"a\"* ;")),
            Expr::repeat(t("a"), 0, None)
        );
        assert_eq!(
            def(&format!("{h}rule <- \"a\"+ ;")),
            Expr::repeat(t("a"), 1, None)
        );
        assert_eq!(def(&format!("{h}rule <- . ;")), Expr::Special(".".into()));
        assert_eq!(
            def(&format!("{h}rule <- &\"a\" ;")),
            Expr::Special("&\"a\"".into())
        );
        assert_eq!(
            def(&format!("{h}rule <- !\"a\" ;")),
            Expr::Special("!\"a\"".into())
        );
        assert_eq!(
            def(&format!("{h}rule <- !(\"a\" \"b\") ;")),
            Expr::Special("!(...)".into())
        );
        assert_eq!(def(&format!("{h}# c\nrule <- x # trailing\n;")), nt("x"));
        assert!(parse("rule <- \"a\" ;", None).is_err());
        assert!(parse("railroad-peg-beta\nrule <- \"a\"", None).is_err());
    }

    // ── layout ──
    fn m() -> Metrics {
        Metrics::default()
    }

    fn check_bounds(l: &Laid) {
        assert!((l.up + l.down() - l.height).abs() < 1e-9);
        assert!(l.up >= 0.0 && l.down() >= 0.0);
        for p in &l.prims {
            match p {
                Prim::Box { rect, .. } => {
                    assert!(
                        rect.x >= -1e-9 && rect.right() <= l.width + 1e-9,
                        "{rect:?}"
                    );
                    assert!(
                        rect.y >= -1e-9 && rect.bottom() <= l.height + 1e-9,
                        "{rect:?}"
                    );
                }
                Prim::Rail(segs) => {
                    for s in segs {
                        let (Seg::Move(q) | Seg::Line(q) | Seg::Arc { to: q, .. }) = s;
                        assert!(
                            q.x >= -1e-9 && q.x <= l.width + 1e-9,
                            "{q:?} in {}",
                            l.width
                        );
                        assert!(q.y >= -1e-9 && q.y <= l.height + 1e-9, "{q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn box_sizes_match_upstream() {
        // `term` at 16px: 32px text + 2×10 padding, 17px bbox + 20.
        let l = layout(&nt("term"), &m());
        assert_eq!((l.width, l.height, l.up), (52.0, 37.0, 18.5));
        let s = layout(&Expr::Special("c".into()), &m());
        assert!((s.width - 54.7).abs() < 0.5, "{}", s.width);
    }

    #[test]
    fn sequence_and_choice_geometry() {
        let mm = m();
        let a = layout(&t("+"), &mm);
        let seq = layout(&Expr::Sequence(vec![t("+"), nt("term")]), &mm);
        assert_eq!(seq.width, a.width + 52.0 + mm.horizontal_separation);
        check_bounds(&seq);
        // Choice: alternatives stacked with verticalSeparation, rails 4r wider.
        let ch = layout(&Expr::Choice(vec![t("+"), t("-")]), &mm);
        assert_eq!(ch.height, 37.0 * 2.0 + mm.vertical_separation);
        assert_eq!(ch.up, ch.height / 2.0);
        let wmax = layout(&t("+"), &mm).width.max(layout(&t("-"), &mm).width);
        assert_eq!(ch.width, wmax + 4.0 * mm.arc_radius);
        check_bounds(&ch);
        // Every alternative contributes one entry and one exit rail.
        let rails = ch
            .prims
            .iter()
            .filter(|p| matches!(p, Prim::Rail(_)))
            .count();
        assert_eq!(rails, 4);
        // Entry rails start at (0, up), exit rails end at (width, up).
        for p in &ch.prims {
            if let Prim::Rail(segs) = p {
                let first = match segs[0] {
                    Seg::Move(q) => q,
                    _ => panic!(),
                };
                let last = match *segs.last().unwrap() {
                    Seg::Move(q) | Seg::Line(q) | Seg::Arc { to: q, .. } => q,
                };
                assert!(
                    first == Point::new(0.0, ch.up) || last == Point::new(ch.width, ch.up),
                    "{segs:?}"
                );
            }
        }
    }

    #[test]
    fn optional_and_repetition_geometry() {
        let mm = m();
        let r = mm.arc_radius;
        let inner = layout(&nt("x"), &mm);
        let o = layout(&Expr::Optional(Box::new(nt("x"))), &mm);
        assert_eq!(o.width, inner.width + 4.0 * r);
        assert_eq!(o.height, inner.height + 2.0 * r);
        assert_eq!(o.up, 2.0 * r + inner.up);
        check_bounds(&o);
        let plus = layout(&Expr::repeat(nt("x"), 1, None), &mm);
        assert_eq!(plus.height, inner.height + 2.0 * r);
        assert_eq!(plus.up, inner.up);
        check_bounds(&plus);
        let star = layout(&Expr::repeat(nt("x"), 0, None), &mm);
        assert_eq!(star.height, inner.height + 4.0 * r);
        assert_eq!(star.up, 2.0 * r + inner.up);
        assert_eq!(star.prims.len(), plus.prims.len() + 1);
        check_bounds(&star);
    }

    #[test]
    fn nested_layout_stays_in_bounds() {
        let g = parse(
            "railroad-ebnf-beta\njson = \"{\" [ member ( \",\" member )* ] \"}\" | { ? any ? }+ - x ;",
            None,
        )
        .unwrap();
        check_bounds(&layout(&g.rules[0].definition, &m()));
    }

    #[test]
    fn rule_and_diagram_layout() {
        let mm = m();
        // `expression = sequence(term, zeroOrMore(...))` from the docs: upstream puts the
        // definition at x = 131.17 and the baseline at y = 61.
        let g = parse(
            "railroad-beta\nexpression = sequence(nonterminal(\"term\"), zeroOrMore(sequence(choice(terminal(\"+\"), terminal(\"-\")), nonterminal(\"term\")))) ;",
            None,
        )
        .unwrap();
        let rl = layout_rule(&g.rules[0], &mm);
        assert_eq!(rl.baseline, 61.0);
        assert!((rl.start.x + 20.0 - 131.17).abs() < 3.0, "{}", rl.start.x);
        assert!(rl.end.x > rl.start.x);
        // Short rules keep the 20px minimum baseline and 40px minimum height.
        let small = Rule {
            name: "r".into(),
            definition: t("a"),
        };
        let rl = layout_rule(&small, &mm);
        assert_eq!(rl.baseline, 20.0);
        assert_eq!(rl.height, 40.0_f64.max(1.5 + 37.0 + 20.0));
        let (placed, w, h) = layout_diagram(&[small.clone(), small], &mm);
        assert_eq!(placed[0].0, mm.padding);
        assert_eq!(placed[1].0, mm.padding + rl.height + mm.vertical_separation);
        assert_eq!(
            h,
            placed[1].0 + rl.height + mm.vertical_separation + mm.padding
        );
        assert_eq!(w, rl.width + 2.0 * mm.padding);
        assert_eq!(layout_diagram(&[], &mm).1, 200.0);
    }

    #[test]
    fn arcs_are_quarter_circles() {
        let cmds = arc_cmds(Point::new(0.0, 10.0), Point::new(10.0, 0.0), 10.0, false);
        // Sweep 0 from (0,10) to (10,0) is centred at (0,0): leaves heading right.
        assert!(
            matches!(cmds.last(), Some(PathCmd::CubicTo(_, _, e)) if *e == Point::new(10.0, 0.0))
        );
        if let PathCmd::CubicTo(c1, _, _) = cmds[0] {
            assert!((c1.y - 10.0).abs() < 1e-9 && c1.x > 0.0, "{c1:?}");
        } else {
            panic!();
        }
        let cw = arc_cmds(Point::new(0.0, 10.0), Point::new(10.0, 0.0), 10.0, true);
        if let PathCmd::CubicTo(c1, _, _) = cw[0] {
            // Centred at (10,10): leaves (0,10) heading straight up.
            assert!(c1.x.abs() < 1e-9 && c1.y < 10.0, "{c1:?}");
        }
    }

    #[test]
    fn styles_follow_theme_and_config() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let st = Style::resolve(&cfg, &pal);
        assert_eq!(st.terminal_fill.css(), "#ffffde");
        assert_eq!(st.non_terminal_fill.css(), "#ececff");
        assert_eq!(st.line.css(), "#333333");
        let j = crate::json::parse(
            r##"{"theme":"base","themeVariables":{"primaryColor":"#ff0000"},"railroad":{"terminalFill":"#123456","lineColor":"url(evil)","arcRadius":4,"padding":-3}}"##,
        )
        .unwrap();
        let cfg = MermaidConfig::default().with_json(&j).unwrap();
        let pal = cfg.palette();
        let st = Style::resolve(&cfg, &pal);
        assert_eq!(st.terminal_fill.css(), "#123456");
        assert_eq!(st.non_terminal_fill.css(), "#ff0000");
        // mkBorder(red) = hsl(0, 60%, 40%), invert(#0000ff) = #ffff00, invert(#f4f4f4).
        assert_eq!(st.non_terminal_stroke.css(), "#a32929");
        assert_eq!(st.terminal_text.css(), "#ffff00");
        assert_eq!(st.line.css(), "#0b0b0b");
        let mt = Metrics::from_config(&cfg, &pal);
        assert_eq!(mt.arc_radius, 4.0);
        assert_eq!(mt.padding, 10.0);
    }

    #[test]
    fn renders_all_syntaxes() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        for (src, kind) in [
            ("railroad-beta\nr = terminal(\"a\") ;", "railroad"),
            ("railroad-ebnf-beta\nr = \"a\" ;", "railroadEbnf"),
            ("railroad-abnf-beta\nr = \"a\" ;", "railroadAbnf"),
            ("railroad-peg-beta\nr <- \"a\" ;", "railroadPeg"),
        ] {
            let s = render(src, None, &cfg, &pal).unwrap();
            assert_eq!(s.kind.role(), kind);
            assert!(s.element_count() > 5);
        }
        let empty = render("railroad-beta\n", None, &cfg, &pal).unwrap();
        assert_eq!((empty.view_box.w, empty.view_box.h), (200.0, 100.0));
    }
}
