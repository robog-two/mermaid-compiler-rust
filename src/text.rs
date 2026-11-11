//! Font-free text measurement and label normalisation.
//!
//! mermaid measures text in a real browser. We use Helvetica/Arial advance widths
//! (the same metrics the PDF backend's built-in Helvetica uses), which gives sizes
//! within a few percent of the browser's "trebuchet ms"/arial rendering.

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

/// Advance width of one character in em units.
pub fn char_width_em(c: char, bold: bool) -> f64 {
    let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    let code = c as u32;
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

/// Width of a single line in pixels.
pub fn line_width(text: &str, font_size: f64, bold: bool) -> f64 {
    text.chars().map(|c| char_width_em(c, bold)).sum::<f64>() * font_size
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
}

impl TextBlock {
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
            if !cur.trim().is_empty() && line_width(cand.trim_end(), font_size, bold) > max_width {
                out.push(std::mem::replace(&mut cur, seg).trim_end().to_string());
            } else {
                cur = cand;
            }
        }
        out.push(cur.trim_end().to_string());
    }
    out
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
    decode_entities(&strip_markup(raw))
}

/// A label's display lines: split on `\n` / `<br>` *before* decoding entities, so an
/// escaped `&lt;br&gt;` stays literal text as in mermaid's HTML labels.
pub fn label_lines(raw: &str) -> Vec<String> {
    split_lines(&strip_markup(raw))
        .iter()
        .map(|l| decode_entities(l))
        .collect()
}

/// [`normalize_label`] without the entity decoding.
fn strip_markup(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = s[1..s.len() - 1].to_string();
    }
    if s.len() >= 2 && s.starts_with('`') && s.ends_with('`') {
        // A markdown string: blank lines only separate paragraphs (`<p>` without
        // margins), single newlines are line breaks.
        let body: Vec<&str> = s[1..s.len() - 1]
            .split('\n')
            .filter(|l| !l.trim().is_empty())
            .collect();
        s = strip_emphasis(&body.join("\n"));
    }
    strip_tags(&s)
}

/// Remove the `*` / `_` delimiters that markdown (marked, CommonMark rules) turns
/// into `<em>` / `<strong>`, keeping the ones that stay literal: `snake_case`,
/// `2 * 3`, unmatched runs. Text is drawn without the emphasis styles.
pub fn strip_emphasis(s: &str) -> String {
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
        .zip(&removed)
        .filter(|(_, r)| !**r)
        .map(|(c, _)| c)
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
            (true, Some(end)) => rest = &tail[end + 1..],
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
