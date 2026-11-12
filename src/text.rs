//! Font-free text measurement and label normalisation.
//!
//! mermaid measures text in a real browser. We use Helvetica/Arial advance widths
//! (the same metrics the PDF backend's built-in Helvetica uses), which gives sizes
//! within a few percent of the browser's "trebuchet ms"/arial rendering.

use crate::geom::Point;
use crate::scene::{Color, Text};

/// Advance widths for ASCII 0x20..=0x7E in 1/1000 em (Helvetica AFM).
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278,
    278, // ' '..'/'
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // :..@
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A-Z
    278, 278, 278, 469, 556, 333, // [..`
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a-z
    334, 260, 334, 584, // {..~
];

/// Helvetica-Bold advance widths for ASCII 0x20..=0x7E.
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, //
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, //
    333, 333, 584, 584, 584, 611, 975, //
    722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, //
    333, 278, 333, 584, 556, 333, //
    556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556,
    333, 611, 556, 778, 556, 556, 500, //
    389, 280, 389, 584,
];

/// Inline emphasis inside display lines.
///
/// A label's display lines are plain `String`s that every diagram wraps, measures
/// and hands to [`crate::scene::Text`]. Markdown strings (`` "`**bold** _it_`" ``)
/// and `<b>` / `<i>` tags keep their emphasis as these private-use marks, which take
/// no width, survive wrapping (each wrapped line is re-balanced so it opens and
/// closes its own styles) and are turned into styled runs by the output backends
/// ([`styled_runs`]). Nothing else in a label can produce them.
pub mod mark {
    pub const BOLD_ON: char = '\u{E000}';
    pub const BOLD_OFF: char = '\u{E001}';
    pub const ITALIC_ON: char = '\u{E002}';
    pub const ITALIC_OFF: char = '\u{E003}';

    pub fn is_mark(c: char) -> bool {
        matches!(c, BOLD_ON | BOLD_OFF | ITALIC_ON | ITALIC_OFF)
    }
}

/// A piece of a display line drawn in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

/// Running emphasis state while reading marks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Emphasis {
    bold: u32,
    italic: u32,
}

impl Emphasis {
    fn apply(&mut self, c: char) -> bool {
        match c {
            mark::BOLD_ON => self.bold += 1,
            mark::BOLD_OFF => self.bold = self.bold.saturating_sub(1),
            mark::ITALIC_ON => self.italic += 1,
            mark::ITALIC_OFF => self.italic = self.italic.saturating_sub(1),
            _ => return false,
        }
        true
    }
}

/// Whether a display line carries emphasis marks.
pub fn has_marks(line: &str) -> bool {
    line.chars().any(mark::is_mark)
}

/// A display line without its emphasis marks.
pub fn plain(line: &str) -> String {
    line.chars().filter(|&c| !mark::is_mark(c)).collect()
}

/// Split a display line into runs of one style (`bold` is the text's base weight).
/// Empty runs are dropped; a line without marks is one run.
pub fn styled_runs(line: &str, bold: bool) -> Vec<StyledRun> {
    let mut out: Vec<StyledRun> = Vec::new();
    let mut st = Emphasis::default();
    let mut cur = String::new();
    let mut cur_style = (bold, false);
    for c in line.chars() {
        if st.apply(c) {
            let style = (bold || st.bold > 0, st.italic > 0);
            if style != cur_style {
                if !cur.is_empty() {
                    out.push(StyledRun {
                        text: std::mem::take(&mut cur),
                        bold: cur_style.0,
                        italic: cur_style.1,
                    });
                }
                cur_style = style;
            }
            continue;
        }
        cur.push(c);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(StyledRun {
            text: cur,
            bold: cur_style.0,
            italic: cur_style.1,
        });
    }
    out
}

/// Make every line open the styles still open at the end of the line before it and
/// close its own open styles, so each line can be drawn (and measured) alone.
pub fn balance_marks(lines: Vec<String>) -> Vec<String> {
    if !lines.iter().any(|l| has_marks(l)) {
        return lines;
    }
    let mut carry = Emphasis::default();
    lines
        .into_iter()
        .map(|l| {
            let mut st = carry;
            let mut out = String::new();
            out.extend(std::iter::repeat_n(
                mark::BOLD_ON,
                carry.bold.min(1) as usize,
            ));
            out.extend(std::iter::repeat_n(
                mark::ITALIC_ON,
                carry.italic.min(1) as usize,
            ));
            st.bold = st.bold.min(1);
            st.italic = st.italic.min(1);
            for c in l.chars() {
                if c == mark::BOLD_OFF && st.bold == 0 || c == mark::ITALIC_OFF && st.italic == 0 {
                    continue;
                }
                st.apply(c);
                out.push(c);
            }
            carry = st;
            out.extend(std::iter::repeat_n(mark::ITALIC_OFF, st.italic as usize));
            out.extend(std::iter::repeat_n(mark::BOLD_OFF, st.bold as usize));
            out
        })
        .collect()
}

/// Advance width of one character in em units (emphasis marks take none).
pub fn char_width_em(c: char, bold: bool) -> f64 {
    let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    let code = c as u32;
    if mark::is_mark(c) {
        return 0.0;
    }
    let units = match code {
        0x20..=0x7E => table[(code - 0x20) as usize] as f64,
        0xA0..=0x24F => 556.0,
        // CJK, Hangul, full-width forms and emoji are roughly square.
        0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFF00..=0xFF60 => {
            1000.0
        }
        0x1F000..=0x1FAFF => 1000.0,
        _ => 600.0,
    };
    units / 1000.0
}

/// Arial's kerning pairs (its `kern` table, in 1/2048 em), which the browser
/// applies to regular text (`font-kerning: auto`): "DEPARTMENT" is 1.3px
/// narrower at 14px than its advance widths (`PA`, `RT`).
const KERNING: &[(&str, i16)] = &[
    (" A", -113),
    (" T", -37),
    (" Y", -37),
    ("11", -152),
    ("A ", -113),
    ("AT", -152),
    ("AV", -152),
    ("AW", -76),
    ("AY", -152),
    ("Av", -37),
    ("Aw", -37),
    ("Ay", -37),
    ("F,", -227),
    ("F.", -227),
    ("FA", -113),
    ("L ", -76),
    ("LT", -152),
    ("LV", -152),
    ("LW", -152),
    ("LY", -152),
    ("Ly", -76),
    ("P ", -37),
    ("P,", -264),
    ("P.", -264),
    ("PA", -152),
    ("RT", -37),
    ("RV", -37),
    ("RW", -37),
    ("RY", -76),
    ("T ", -37),
    ("T,", -227),
    ("T-", -113),
    ("T.", -227),
    ("T:", -227),
    ("T;", -227),
    ("TA", -152),
    ("TO", -37),
    ("Ta", -227),
    ("Tc", -227),
    ("Te", -227),
    ("Ti", -76),
    ("To", -227),
    ("Tr", -76),
    ("Ts", -227),
    ("Tu", -76),
    ("Tw", -113),
    ("Ty", -113),
    ("V ", -37),
    ("V,", -188),
    ("V-", -113),
    ("V.", -188),
    ("V:", -76),
    ("V;", -76),
    ("VA", -152),
    ("Va", -152),
    ("Ve", -113),
    ("Vi", -37),
    ("Vo", -113),
    ("Vr", -76),
    ("Vu", -76),
    ("Vy", -76),
    ("W,", -113),
    ("W-", -37),
    ("W.", -113),
    ("W:", -37),
    ("W;", -37),
    ("WA", -76),
    ("Wa", -76),
    ("We", -37),
    ("Wo", -37),
    ("Wr", -37),
    ("Wu", -37),
    ("Wy", -18),
    ("Y ", -37),
    ("Y,", -264),
    ("Y-", -188),
    ("Y.", -264),
    ("Y:", -113),
    ("Y;", -113),
    ("YA", -152),
    ("Ya", -152),
    ("Ye", -188),
    ("Yi", -76),
    ("Yo", -188),
    ("Yp", -152),
    ("Yq", -188),
    ("Yu", -113),
    ("Yv", -113),
    ("ff", -37),
    ("r,", -113),
    ("r.", -113),
    ("v,", -152),
    ("v.", -152),
    ("w,", -113),
    ("w.", -113),
    ("y,", -152),
    ("y.", -152),
];

/// The kerning between two characters of regular text, in em.
fn kerning_em(a: char, b: char) -> f64 {
    if !a.is_ascii() || !b.is_ascii() {
        return 0.0;
    }
    KERNING
        .iter()
        .find(|(p, _)| p.as_bytes() == [a as u8, b as u8])
        .map_or(0.0, |&(_, k)| f64::from(k) / 2048.0)
}

/// Width of a single line in pixels (regular text kerned as Arial is).
pub fn line_width(text: &str, font_size: f64, bold: bool) -> f64 {
    let mut st = Emphasis::default();
    let mut prev: Option<char> = None;
    text.chars()
        .map(|c| {
            if st.apply(c) {
                // A change of style starts a new run, which is not kerned across.
                prev = None;
                return 0.0;
            }
            let b = bold || st.bold > 0;
            let kern = match prev {
                Some(p) if !b => kerning_em(p, c),
                _ => 0.0,
            };
            prev = Some(c);
            char_width_em(c, b) + kern
        })
        .sum::<f64>()
        * font_size
}

/// Multi-line text with its measured extent.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub lines: Vec<String>,
    pub font_size: f64,
    pub line_height: f64,
    pub bold: bool,
    pub width: f64,
    pub height: f64,
    /// Laid out as SVG `<text>` (`htmlLabels: false`, see [`TextBlock::svg`]) rather
    /// than as an HTML label: [`TextBlock::centered_text`] places its baselines the
    /// way `createFormattedText` does.
    pub svg: bool,
}

/// Line pitch of SVG text labels in em (`createFormattedText`'s `lineHeight`).
pub const SVG_LINE_HEIGHT: f64 = 1.1;

/// Whether a glyph reaches below the font's descent line, which grows the browser's
/// `getBBox()` of an SVG text line (`g`, `p`, brackets, `_`, ...).
fn has_deep_descender(c: char) -> bool {
    matches!(
        c,
        'g' | 'j'
            | 'p'
            | 'q'
            | 'y'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '|'
            | '_'
            | '@'
            | '$'
            | 'ç'
            | 'Ç'
            | 'µ'
            | 'þ'
            | 'ÿ'
            | 'ý'
    )
}

/// Height of one SVG text line's bounding box, as Chromium's `getBBox()` reports it
/// for mermaid's fonts (Arial metrics, ascent 0.905em and descent 0.212em, each
/// rounded to whole pixels; a deep descender rounds the descent up instead):
/// 16px at 14px, 17px (18px with a descender) at 16px.
pub fn svg_line_box(font_size: f64, line: &str) -> f64 {
    let ascent = (0.905 * font_size).round();
    let descent = (0.212 * font_size).round();
    let descent = if line.chars().any(has_deep_descender) {
        descent.max((0.212 * font_size - 1e-6).ceil())
    } else {
        descent
    };
    ascent + descent
}

impl TextBlock {
    /// Lines laid out as mermaid's SVG text labels (`createText` with
    /// `useHtmlLabels: false`): baselines 1.1em apart, the block as tall as the
    /// browser's text bounding box (one line box plus 1.1em per further line).
    pub fn svg(lines: Vec<String>, font_size: f64, bold: bool) -> Self {
        Self::svg_stacked(lines, font_size, bold, None)
    }

    /// [`TextBlock::svg`] for lines that are separate `<text>` elements stacked
    /// box on box with `gap` pixels between them (class members, `shapeUtil`'s
    /// `textHelper`) instead of tspans 1.1em apart.
    pub fn svg_stacked(lines: Vec<String>, font_size: f64, bold: bool, gap: Option<f64>) -> Self {
        let mut b = Self::new(lines, font_size, bold);
        b.svg = true;
        let n = b.lines.len();
        b.line_height = match gap {
            Some(g) => svg_line_box(font_size, "") + g,
            None => font_size * SVG_LINE_HEIGHT,
        };
        if b.height > 0.0 {
            let last = b.lines.last().map(String::as_str).unwrap_or("");
            b.height = match gap {
                Some(g) => {
                    b.lines
                        .iter()
                        .map(|l| svg_line_box(font_size, l))
                        .sum::<f64>()
                        + g * (n - 1) as f64
                }
                None => b.line_height * (n - 1) as f64 + svg_line_box(font_size, last),
            };
        }
        b
    }

    /// The block drawn centred on `center`: an HTML label's lines centred in their
    /// 1.5em line boxes, or an SVG label's first baseline 1em below the box top
    /// (`createTspan`'s `y = -0.1em, dy = 1.1em` in a label shifted up by half its
    /// height).
    pub fn centered_text(&self, center: Point, fill: &Color) -> Text {
        if !self.svg {
            return Text::centered(center, &self.lines, self.font_size, self.line_height, fill);
        }
        Text {
            y: center.y - self.height / 2.0 + self.font_size,
            ..Text::centered(center, &self.lines, self.font_size, self.line_height, fill)
        }
    }

    /// Measure already-split lines.
    pub fn new(lines: Vec<String>, font_size: f64, bold: bool) -> Self {
        let line_height = font_size * 1.5;
        let width = lines
            .iter()
            .map(|l| line_width(l, font_size, bold))
            .fold(0.0, f64::max);
        let height = if lines.iter().all(|l| l.is_empty()) {
            0.0
        } else {
            line_height * lines.len() as f64
        };
        TextBlock {
            lines,
            font_size,
            line_height,
            bold,
            width,
            height,
            svg: false,
        }
    }

    /// Normalise a raw mermaid label and measure it.
    pub fn label(raw: &str, font_size: f64) -> Self {
        Self::new(label_lines(raw), font_size, false)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }
}

/// Where a line of wrapped text may break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakRule {
    /// Only at spaces (mermaid's own SVG `wrap` helpers split on `\s+`).
    Spaces,
    /// Like the browser lays out an HTML label (`white-space: break-spaces` in a
    /// `max-width` div): at spaces, after a hyphen inside a word, and between CJK
    /// characters (UAX #14 ID class, keeping closing punctuation off a line start).
    /// A word with no break opportunity overflows rather than being split.
    Browser,
}

/// Normalise a raw label and wrap it into lines no wider than `max_width` (where
/// break opportunities allow). Explicit `<br>` / newlines always break.
pub fn wrap(raw: &str, max_width: f64, font_size: f64, bold: bool, rule: BreakRule) -> Vec<String> {
    wrap_lines(&label_lines(raw), max_width, font_size, bold, rule)
}

/// [`wrap`] for lines that are already split and entity-decoded.
pub fn wrap_lines(
    lines: &[String],
    max_width: f64,
    font_size: f64,
    bold: bool,
    rule: BreakRule,
) -> Vec<String> {
    let mut out = Vec::new();
    for para in lines {
        let para = para.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut cur = String::new();
        for seg in segments(&para, rule) {
            let cand = format!("{cur}{seg}");
            if !plain(&cur).trim().is_empty()
                && line_width(cand.trim_end(), font_size, bold) > max_width
            {
                out.push(std::mem::replace(&mut cur, seg).trim_end().to_string());
            } else {
                cur = cand;
            }
        }
        out.push(cur.trim_end().to_string());
    }
    balance_marks(out)
}

/// Wrap already-split lines as mermaid's SVG text labels do (`createFormattedText`
/// with `splitLineToFitWidth`): a line that fits stays whole; otherwise words are
/// added while the line fits, and a word too wide on its own is cut between
/// characters (the browser's HTML line breaking would let it overflow instead).
pub fn svg_wrap_lines(lines: &[String], width: f64, font_size: f64, bold: bool) -> Vec<String> {
    let fits = |s: &str| line_width(s, font_size, bold) <= width;
    let mut out = Vec::new();
    for para in lines {
        let mut queue: std::collections::VecDeque<String> =
            para.split_whitespace().map(str::to_string).collect();
        let joined = queue
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        if fits(&joined) {
            out.push(joined);
            continue;
        }
        let mut cur: Vec<String> = Vec::new();
        while let Some(word) = queue.pop_front() {
            let cand = if cur.is_empty() {
                word.clone()
            } else {
                format!("{} {word}", cur.join(" "))
            };
            if fits(&cand) {
                cur.push(word);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur).join(" "));
                queue.push_front(word);
            } else {
                let chars: Vec<char> = word.chars().collect();
                let mut k = 0;
                while k < chars.len() && fits(&chars[..=k].iter().collect::<String>()) {
                    k += 1;
                }
                let k = k.max(1);
                out.push(chars[..k].iter().collect());
                if k < chars.len() {
                    queue.push_front(chars[k..].iter().collect());
                }
            }
        }
        if !cur.is_empty() {
            out.push(cur.join(" "));
        }
    }
    balance_marks(out)
}

/// CJK ideographs, kana, Hangul and full-width forms: each is its own word to the
/// browser's line breaker.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FFF | 0x3000..=0x303F | 0x3040..=0x30FF
        | 0x3100..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF)
}

/// No line may start with these (closing punctuation, small kana; UAX #14 CL/NS).
fn no_break_before(c: char) -> bool {
    matches!(
        c,
        '、' | '。'
            | '，'
            | '．'
            | '：'
            | '；'
            | '！'
            | '？'
            | '）'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '》'
            | '〕'
            | '］'
            | '｝'
            | '〙'
            | '〗'
            | '・'
            | 'ー'
            | '々'
            | '〜'
            | 'ぁ'
            | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | ','
            | '.'
            | ')'
            | ']'
            | '}'
            | '!'
            | '?'
            | ':'
            | ';'
    )
}

/// No line may end with these (opening punctuation; UAX #14 OP).
fn no_break_after(c: char) -> bool {
    matches!(
        c,
        '（' | '「'
            | '『'
            | '【'
            | '〈'
            | '《'
            | '〔'
            | '［'
            | '｛'
            | '〘'
            | '〖'
            | '('
            | '['
            | '{'
    )
}

/// Split a single-spaced paragraph into unbreakable pieces; a piece keeps the space
/// that follows it.
fn segments(para: &str, rule: BreakRule) -> Vec<String> {
    let chars: Vec<char> = para.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        let next = chars.get(i + 1).copied();
        let Some(n) = next else { break };
        let brk = if c == ' ' {
            n != ' '
        } else if rule == BreakRule::Spaces || n == ' ' {
            false
        } else if (is_cjk(c) || is_cjk(n)) && !no_break_before(n) && !no_break_after(c) {
            true
        } else {
            // `DELIVERY-ADDRESS` breaks after the hyphen (UAX #14 HY before a letter).
            c == '-' && n.is_alphabetic() && i > 0 && chars[i - 1].is_alphanumeric()
        };
        if brk {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Split on `\n` and every `<br>` variant.
pub fn split_lines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(len) = br_tag_len(rest) {
            out.push(std::mem::take(&mut cur));
            rest = &rest[len..];
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        if c == '\n' {
            out.push(std::mem::take(&mut cur));
        } else if c != '\r' {
            cur.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out.push(cur);
    out.into_iter().map(|l| l.trim().to_string()).collect()
}

fn br_tag_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 4
        || b[0] != b'<'
        || !b[1].eq_ignore_ascii_case(&b'b')
        || !b[2].eq_ignore_ascii_case(&b'r')
    {
        return None;
    }
    let close = s.find('>')?;
    let inner = s[3..close].trim();
    (inner.is_empty() || inner == "/").then_some(close + 1)
}

/// Decode mermaid entity codes, strip markdown-string backticks, emphasis
/// markers and inline HTML tags other than `<br>`.
pub fn normalize_label(raw: &str) -> String {
    plain(&decode_entities(&strip_markup(raw)))
}

/// A label's display lines: split on `\n` / `<br>` *before* decoding entities, so an
/// escaped `&lt;br&gt;` stays literal text as in mermaid's HTML labels. Emphasis
/// from markdown strings and `<b>` / `<i>` tags is kept as [`mark`]s.
pub fn label_lines(raw: &str) -> Vec<String> {
    to_lines(&strip_markup(raw))
}

/// Display lines of text that is markdown whether or not it is wrapped in
/// backticks (mindmap and kanban labels are `labelType: 'markdown'`).
pub fn markdown_lines(text: &str) -> Vec<String> {
    to_lines(&strip_tags(&markdown_body(text.trim())))
}

fn to_lines(marked: &str) -> Vec<String> {
    balance_marks(
        split_lines(marked)
            .iter()
            .map(|l| decode_entities(l))
            .collect(),
    )
}

/// Whether a raw label is a markdown string (`` `…` ``, optionally in quotes).
pub fn is_markdown_string(raw: &str) -> bool {
    let s = raw.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s);
    s.len() >= 2 && s.starts_with('`') && s.ends_with('`')
}

/// [`normalize_label`] without the entity decoding (emphasis kept as marks).
fn strip_markup(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = s[1..s.len() - 1].to_string();
    }
    if s.len() >= 2 && s.starts_with('`') && s.ends_with('`') {
        s = markdown_body(&s[1..s.len() - 1]);
    }
    strip_tags(&s)
}

/// A markdown string's body as mermaid's `markdownToHTML` / `markdownToLines`
/// lays it out: blank lines only separate paragraphs (`<p>` without margins),
/// single newlines are line breaks, each line's indentation is dropped, and
/// `**strong**` / `*em*` become emphasis marks.
fn markdown_body(body: &str) -> String {
    let body: Vec<&str> = body
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    emphasize(&body.join("\n"))
}

/// Remove the `*` / `_` delimiters that markdown (marked, CommonMark rules) turns
/// into `<em>` / `<strong>`, keeping the ones that stay literal: `snake_case`,
/// `2 * 3`, unmatched runs.
pub fn strip_emphasis(s: &str) -> String {
    plain(&emphasize(s))
}

/// [`strip_emphasis`], with each matched delimiter pair replaced by emphasis
/// [`mark`]s (`**`/`__` bold, `*`/`_` italic).
fn emphasize(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
    let space = |c: Option<char>| c.is_none_or(char::is_whitespace);
    struct Run {
        ch: char,
        start: usize,
        len: usize,
        /// Delimiters still unused (removed from the end nearest the text).
        left: usize,
        open: bool,
        close: bool,
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '*' && c != '_' {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] == c {
            i += 1;
        }
        let before = start.checked_sub(1).map(|k| chars[k]);
        let after = chars.get(i).copied();
        let left_flanking = !space(after) && (!punct(after) || space(before) || punct(before));
        let right_flanking = !space(before) && (!punct(before) || space(after) || punct(after));
        let (open, close) = if c == '*' {
            (left_flanking, right_flanking)
        } else {
            (
                left_flanking && (!right_flanking || punct(before)),
                right_flanking && (!left_flanking || punct(after)),
            )
        };
        runs.push(Run {
            ch: c,
            start,
            len: i - start,
            left: i - start,
            open,
            close,
        });
    }
    // CommonMark "process emphasis": match each closer with the nearest opener.
    let mut removed = vec![false; chars.len()];
    // The mark that takes a removed delimiter's place (opening one first, closing
    // one last, so nested pairs nest).
    let mut marks: Vec<Option<char>> = vec![None; chars.len()];
    for ci in 0..runs.len() {
        if !runs[ci].close {
            continue;
        }
        while runs[ci].left > 0 {
            let opener = (0..ci).rev().find(|&oi| {
                let (o, c) = (&runs[oi], &runs[ci]);
                o.ch == c.ch
                    && o.open
                    && o.left > 0
                    // "Rule of 3": a run that can both open and close only pairs when
                    // the lengths don't sum to a multiple of 3 (unless both are).
                    && !((o.close || c.open)
                        && (o.len + c.len) % 3 == 0
                        && !(o.len % 3 == 0 && c.len % 3 == 0))
            });
            let Some(oi) = opener else { break };
            let used = if runs[oi].left >= 2 && runs[ci].left >= 2 {
                2
            } else {
                1
            };
            // Openers lose delimiters from their end, closers from their start.
            let o_end = runs[oi].start + runs[oi].left;
            removed[o_end - used..o_end].fill(true);
            let c_start = runs[ci].start + (runs[ci].len - runs[ci].left);
            removed[c_start..c_start + used].fill(true);
            let (on, off) = if used == 2 {
                (mark::BOLD_ON, mark::BOLD_OFF)
            } else {
                (mark::ITALIC_ON, mark::ITALIC_OFF)
            };
            marks[o_end - used] = Some(on);
            marks[c_start] = Some(off);
            runs[oi].left -= used;
            runs[ci].left -= used;
            // Runs between the pair can no longer match anything.
            for r in &mut runs[oi + 1..ci] {
                r.left = 0;
            }
        }
    }
    chars
        .iter()
        .zip(removed.iter().zip(&marks))
        .filter_map(|(&c, (&r, &m))| if r { m } else { Some(c) })
        .collect()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(len) = br_tag_len(tail) {
            out.push_str(&tail[..len]);
            rest = &tail[len..];
            continue;
        }
        let looks_like_tag = tail[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/');
        match (looks_like_tag, tail.find('>')) {
            (true, Some(end)) => {
                // `<b>` / `<strong>` / `<i>` / `<em>` style the text in an HTML label.
                let inner = tail[1..end].trim().to_ascii_lowercase();
                let (close, name) = match inner.strip_prefix('/') {
                    Some(n) => (true, n.trim()),
                    None => (false, inner.split_whitespace().next().unwrap_or("")),
                };
                let m = match (name, close) {
                    ("b" | "strong", false) => Some(mark::BOLD_ON),
                    ("b" | "strong", true) => Some(mark::BOLD_OFF),
                    ("i" | "em", false) => Some(mark::ITALIC_ON),
                    ("i" | "em", true) => Some(mark::ITALIC_OFF),
                    _ => None,
                };
                out.extend(m);
                rest = &tail[end + 1..];
            }
            _ => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `#quot;`, `#35;`, `&amp;`, `&#9829;` …
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(['#', '&']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail[1..]
            .find(';')
            .filter(|&e| e > 0 && e <= 10)
            .and_then(|e| {
                let name = &tail[1..1 + e];
                let ch = if let Some(num) = name.strip_prefix('#') {
                    parse_codepoint(num)
                } else if tail.starts_with('#') {
                    parse_codepoint(name).or_else(|| named_entity(name))
                } else {
                    named_entity(name)
                }?;
                Some((ch, e + 2))
            });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push_str(&tail[..1]);
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_codepoint(num: &str) -> Option<char> {
    let n = if let Some(h) = num.strip_prefix(['x', 'X']) {
        u32::from_str_radix(h, 16).ok()?
    } else {
        num.parse().ok()?
    };
    char::from_u32(n)
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "quot" => '"',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "hearts" => '♥',
        "infin" => '∞',
        "rarr" => '→',
        "larr" => '←',
        "deg" => '°',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_line_box_follows_chromium_get_bbox() {
        assert_eq!(svg_line_box(14.0, "Christmas"), 16.0);
        assert_eq!(svg_line_box(14.0, "ggg"), 16.0);
        assert_eq!(svg_line_box(16.0, "Christmas"), 17.0);
        assert_eq!(svg_line_box(16.0, "Laptop"), 18.0);
        assert_eq!(svg_line_box(16.0, "(x)"), 18.0);
    }

    #[test]
    fn svg_blocks_grow_by_1_1em_per_line() {
        let b = TextBlock::svg(
            vec!["Line1".into(), "Line 2".into(), "Line 3".into()],
            14.0,
            false,
        );
        assert!(b.svg);
        assert!((b.line_height - 15.4).abs() < 1e-9);
        assert!((b.height - 46.8).abs() < 1e-9);
        // The last line's descender decides the bottom.
        let up = TextBlock::svg(vec!["Ag".into(), "aa".into()], 16.0, false);
        let down = TextBlock::svg(vec!["aa".into(), "Ag".into()], 16.0, false);
        assert!((up.height - 34.6).abs() < 1e-9);
        assert!((down.height - 35.6).abs() < 1e-9);
        // Class members: separate text boxes 3px apart.
        let m = TextBlock::svg_stacked(vec!["+a".into(), "+b".into()], 14.0, false, Some(3.0));
        assert_eq!(m.height, 35.0);
        assert_eq!(TextBlock::svg(vec![String::new()], 14.0, false).height, 0.0);
    }

    #[test]
    fn svg_text_baseline_sits_1em_below_the_box_top() {
        let c = Color::new("#000");
        let b = TextBlock::svg(vec!["x".into()], 14.0, false);
        assert_eq!(b.centered_text(Point::new(0.0, 0.0), &c).y, -8.0 + 14.0);
        // HTML blocks keep `Text::centered`.
        let h = TextBlock::new(vec!["x".into()], 14.0, false);
        assert_eq!(
            h.centered_text(Point::new(1.0, 2.0), &c),
            Text::centered(Point::new(1.0, 2.0), &h.lines, 14.0, h.line_height, &c)
        );
    }

    #[test]
    fn svg_wrapping_fills_lines_and_cuts_words() {
        let lines = |s: &str| vec![s.to_string()];
        assert_eq!(
            svg_wrap_lines(&lines("short"), 120.0, 16.0, false),
            ["short"]
        );
        assert_eq!(
            svg_wrap_lines(
                &lines("a very long label that has to wrap"),
                120.0,
                16.0,
                false
            ),
            ["a very long label", "that has to wrap"]
        );
        let cut = svg_wrap_lines(
            &lines("Supercalifragilisticexpialidocious"),
            120.0,
            16.0,
            false,
        );
        assert_eq!(cut, ["Supercalifragilist", "icexpialidocious"]);
        // A character wider than the width still makes progress.
        assert_eq!(svg_wrap_lines(&lines("WW"), 1.0, 16.0, false), ["W", "W"]);
        // Emphasis survives a cut line.
        let bold = format!("{}aaaa bbbb{}", mark::BOLD_ON, mark::BOLD_OFF);
        let w = svg_wrap_lines(&[bold], 40.0, 16.0, false);
        assert_eq!(w.len(), 2);
        assert!(
            w.iter()
                .all(|l| l.starts_with(mark::BOLD_ON) && l.ends_with(mark::BOLD_OFF))
        );
    }

    #[test]
    fn splits_br_variants() {
        assert_eq!(
            split_lines("a<br>b<br/>c<BR />d\ne"),
            vec!["a", "b", "c", "d", "e"]
        );
    }

    #[test]
    fn normalizes_labels() {
        assert_eq!(
            normalize_label("\"A #quot;quoted#quot; &amp; #35;\""),
            "A \"quoted\" & #"
        );
        assert_eq!(normalize_label("<b>bold</b> text"), "bold text");
        assert_eq!(normalize_label("\"`**md** string`\""), "md string");
        assert_eq!(normalize_label("a < b"), "a < b");
        assert_eq!(normalize_label("C#"), "C#");
    }

    #[test]
    fn measures_widths() {
        let w = line_width("Hello", 16.0, false);
        // Kerned as the browser does (mermaid: "DEPARTMENT" 95.16px at 14px).
        assert!((line_width("DEPARTMENT", 14.0, false) - 95.16).abs() < 0.05);
        // Only regular text is kerned here.
        let unkerned = (char_width_em('T', true) + char_width_em('e', true)) * 14.0;
        assert!((line_width("Te", 14.0, true) - unkerned).abs() < 1e-9);
        assert!(line_width("Te", 14.0, false) < line_width("Tn", 14.0, false));
        assert!((w - 16.0 * (0.722 + 0.556 + 0.222 + 0.222 + 0.556)).abs() < 1e-9);
        let b = TextBlock::label("a<br>bb", 16.0);
        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.height, 48.0);
    }

    #[test]
    fn escaped_br_is_text_not_a_line_break() {
        assert_eq!(label_lines("a &lt;br&gt; b"), vec!["a <br> b"]);
        assert_eq!(label_lines("a<br>b &amp; c"), vec!["a", "b & c"]);
        assert_eq!(normalize_label("&lt;tag&gt;"), "<tag>");
    }

    #[test]
    fn markdown_emphasis_follows_commonmark_delimiter_rules() {
        let n = |s: &str| normalize_label(&format!("`{s}`"));
        assert_eq!(n("**Bold** and _italic_ text"), "Bold and italic text");
        assert_eq!(n("snake_case_word and _it_"), "snake_case_word and it");
        assert_eq!(n("2 * 3 * 4 and *a*b"), "2 * 3 * 4 and ab");
        assert_eq!(n("__strong__ ***both*** ~~del~~"), "strong both ~~del~~");
        assert_eq!(n("**unclosed"), "**unclosed");
        assert_eq!(strip_emphasis("**a*"), "*a");
        // Blank lines only separate paragraphs; single newlines break lines.
        assert_eq!(n("md1\n\nmd3"), "md1\nmd3");
        // Plain strings keep `_` and `*`, and their blank lines.
        assert_eq!(normalize_label("_x_ *y*\n\nz"), "_x_ *y*\n\nz");
    }

    fn runs(line: &str) -> Vec<(String, bool, bool)> {
        styled_runs(line, false)
            .into_iter()
            .map(|r| (r.text, r.bold, r.italic))
            .collect()
    }

    #[test]
    fn markdown_strings_keep_bold_and_italic_as_styled_runs() {
        let l = label_lines("\"`This **is** _Markdown_`\"");
        assert_eq!(l.len(), 1);
        assert_eq!(plain(&l[0]), "This is Markdown");
        assert_eq!(
            runs(&l[0]),
            [
                ("This ".into(), false, false),
                ("is".into(), true, false),
                (" ".into(), false, false),
                ("Markdown".into(), false, true),
            ]
        );
        // `***x***` is both; nested pairs nest.
        let l = label_lines("`***both*** __under__`");
        assert_eq!(runs(&l[0])[0], ("both".into(), true, true));
        assert_eq!(runs(&l[0])[2], ("under".into(), true, false));
        // Indentation of continuation lines is dropped; emphasis may span lines.
        let l = label_lines("`The **cat\n    in** the hat`");
        assert_eq!(
            l.iter().map(|l| plain(l)).collect::<Vec<_>>(),
            ["The cat", "in the hat"]
        );
        assert_eq!(runs(&l[1])[0], ("in".into(), true, false));
        assert_eq!(runs(&l[1])[1], (" the hat".into(), false, false));
        // Plain strings do not interpret markdown; normalize_label stays plain.
        assert!(!has_marks(&label_lines("**not md**")[0]));
        assert_eq!(normalize_label("`**md**`"), "md");
        assert!(is_markdown_string("\"`x`\"") && is_markdown_string("`x`"));
        assert!(!is_markdown_string("\"x\"") && !is_markdown_string("`"));
    }

    #[test]
    fn html_emphasis_tags_become_styled_runs() {
        let l = label_lines("<b>html</b> <i>tags</i> and <strong>x</STRONG><em>y</em>");
        assert_eq!(plain(&l[0]), "html tags and xy");
        assert_eq!(
            runs(&l[0]),
            [
                ("html".into(), true, false),
                (" ".into(), false, false),
                ("tags".into(), false, true),
                (" and ".into(), false, false),
                ("x".into(), true, false),
                ("y".into(), false, true),
            ]
        );
    }

    #[test]
    fn emphasis_is_measured_bold_and_survives_wrapping() {
        let bold = label_lines("`**Wide**`");
        assert_eq!(
            line_width(&bold[0], 16.0, false),
            line_width("Wide", 16.0, true)
        );
        let l = wrap(
            "`A **long bold markdown label** that wraps`",
            120.0,
            16.0,
            false,
            BreakRule::Browser,
        );
        assert!(l.len() > 2, "{l:?}");
        // Every line opens and closes its own styles.
        for line in &l {
            let mut st = Emphasis::default();
            line.chars().for_each(|c| {
                st.apply(c);
            });
            assert_eq!(st, Emphasis::default(), "{line:?}");
        }
        assert!(runs(&l[1]).iter().all(|r| r.1), "{:?}", runs(&l[1]));
        // A stray closing tag is dropped, an unclosed one carries to the next line.
        let l = label_lines("a</b> <b>b<br>c");
        assert_eq!(
            runs(&l[0]),
            [("a ".into(), false, false), ("b".into(), true, false)]
        );
        assert_eq!(runs(&l[1]), [("c".into(), true, false)]);
    }

    #[test]
    fn browser_wrapping_breaks_cjk_and_hyphens_but_not_long_words() {
        let w = |s: &str, max: f64| wrap(s, max, 16.0, false, BreakRule::Browser);
        // CJK breaks between any two characters (16px each at 16px).
        let cjk = w("这是一段很长的中文文本用于测试", 120.0);
        assert_eq!(cjk, ["这是一段很长的", "中文文本用于测", "试"]);
        // ... but no line starts with closing punctuation.
        assert_eq!(w("一二三四五六七。", 112.0), ["一二三四五六", "七。"]);
        assert_eq!(w("DELIVERY-ADDRESS", 120.0), ["DELIVERY-", "ADDRESS"]);
        // A word without break opportunities overflows.
        let long = "Supercalifragilisticexpialidocious";
        assert_eq!(w(long, 120.0), [long]);
        assert_eq!(w("a - b", 1000.0), ["a - b"]);
        assert_eq!(w("x<br>y z", 1000.0), ["x", "y z"]);
        // Space-only breaking keeps CJK and hyphenated words whole.
        let s = wrap(
            "中文文本 DELIVERY-ADDRESS",
            40.0,
            16.0,
            false,
            BreakRule::Spaces,
        );
        assert_eq!(s, ["中文文本", "DELIVERY-ADDRESS"]);
    }
}
