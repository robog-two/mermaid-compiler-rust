//! Font-free text measurement and label normalisation.
//!
//! mermaid measures text in a real browser. We use the metrics of the fonts
//! mmdc's page draws it in: Arial's (Liberation Sans, which `"trebuchet ms",
//! verdana, arial` resolves to) by default, and the web fonts mmdc loads,
//! Recursive Variable and Open Sans ([`webfonts`]), for text whose
//! `font-family` names them. Which one applies is the measuring face ([`face`]),
//! set for the extent of a diagram's layout by [`with_face`].

pub mod inter;
pub mod serif;
mod webfont_data;
mod webfont_synthetic;
pub mod webfonts;

pub use webfonts::Face;

thread_local! {
    static FACE: std::cell::Cell<Face> = const { std::cell::Cell::new(Face::Arial) };
}

/// The face text is measured in: the one set by the innermost [`with_face`]
/// on this thread, Arial outside any.
pub fn face() -> Face {
    FACE.with(std::cell::Cell::get)
}

/// Run `f` with text measured in `face` (the previous face comes back
/// afterwards, also when `f` panics). A diagram's layout runs inside one,
/// with the face its theme's `fontFamily` names.
pub fn with_face<R>(face: Face, f: impl FnOnce() -> R) -> R {
    struct Restore(Face);
    impl Drop for Restore {
        fn drop(&mut self) {
            FACE.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(FACE.with(|c| c.replace(face)));
    f()
}

/// [`with_face`] for a CSS `font-family` list ([`Face::of_family`]).
pub fn with_family<R>(font_family: &str, f: impl FnOnce() -> R) -> R {
    with_face(Face::of_family(font_family), f)
}

use crate::geom::Point;
use crate::scene::{Color, Text};

/// Advance widths for ASCII 0x20..=0x7E in 1/2048 em: Arial's (its `hmtx`
/// table), which the browser renders mermaid's labels in (`"trebuchet ms",
/// verdana, arial` falls back to it). Helvetica's AFM widths are these rounded
/// to 1/1000 em, which drifts by a few hundredths of a pixel over a long label.
const ARIAL: [u16; 95] = [
    569, 569, 727, 1139, 1139, 1821, 1366, 391, 682, 682, 797, 1196, 569, 682, 569,
    569, // ' '..'/'
    1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, // 0-9
    569, 569, 1196, 1196, 1196, 1139, 2079, // :..@
    1366, 1366, 1479, 1479, 1366, 1251, 1593, 1479, 569, 1024, 1366, 1139, 1706, 1479, 1593, 1366,
    1593, 1479, 1366, 1251, 1479, 1366, 1933, 1366, 1366, 1251, // A-Z
    569, 569, 569, 961, 1139, 682, // [..`
    1139, 1139, 1024, 1139, 1139, 569, 1139, 1139, 455, 455, 1024, 455, 1706, 1139, 1139, 1139,
    1139, 682, 1024, 569, 1139, 1024, 1479, 1024, 1024, 1024, // a-z
    684, 532, 684, 1196, // {..~
];

/// Arial Bold's advance widths for ASCII 0x20..=0x7E, in 1/2048 em.
const ARIAL_BOLD: [u16; 95] = [
    569, 682, 971, 1139, 1139, 1821, 1479, 487, 682, 682, 797, 1196, 569, 682, 569,
    569, // ' '..'/'
    1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, 1139, // 0-9
    682, 682, 1196, 1196, 1196, 1251, 1997, // :..@
    1479, 1479, 1479, 1479, 1366, 1251, 1593, 1479, 569, 1139, 1479, 1251, 1706, 1479, 1593, 1366,
    1593, 1479, 1366, 1251, 1479, 1366, 1933, 1366, 1366, 1251, // A-Z
    682, 569, 682, 1196, 1139, 682, // [..`
    1139, 1251, 1139, 1251, 1139, 682, 1251, 1251, 569, 569, 1139, 569, 1821, 1251, 1251, 1251,
    1251, 797, 1139, 682, 1251, 1139, 1593, 1139, 1139, 1024, // a-z
    797, 573, 797, 1196, // {..~
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
    /// Starts a new run (its own `<tspan>`) in the same style, as mermaid's
    /// `createText` puts each word of an SVG label in a tspan of its own. Placed
    /// in finished display lines only; it changes neither measurement nor
    /// kerning.
    pub const RUN: char = '\u{E004}';

    pub fn is_mark(c: char) -> bool {
        matches!(c, BOLD_ON | BOLD_OFF | ITALIC_ON | ITALIC_OFF | RUN)
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

/// A display line split into one run per word ([`mark::RUN`] before each space),
/// as mermaid's `createText` draws SVG labels (see [`svg_word_runs_box`]).
pub fn word_runs(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    for (i, c) in line.chars().enumerate() {
        if c == ' ' && i > 0 {
            out.push(mark::RUN);
        }
        out.push(c);
    }
    out
}

/// Split a display line into runs of one style (`bold` is the text's base weight).
/// Empty runs are dropped; a line without marks is one run.
pub fn styled_runs(line: &str, bold: bool) -> Vec<StyledRun> {
    let mut out: Vec<StyledRun> = Vec::new();
    let mut st = Emphasis::default();
    let mut cur = String::new();
    let mut cur_style = (bold, false);
    for c in line.chars() {
        if c == mark::RUN {
            if !cur.is_empty() {
                out.push(StyledRun {
                    text: std::mem::take(&mut cur),
                    bold: cur_style.0,
                    italic: cur_style.1,
                });
            }
            continue;
        }
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

/// Advance width of one character in em units (emphasis marks take none), in
/// the measuring face ([`face`]); a character the face lacks takes Arial's.
pub fn char_width_em(c: char, bold: bool) -> f64 {
    char_width_em_styled(c, bold, false)
}

/// [`char_width_em`] upright or italic (Open Sans' italic face has advances
/// of its own; Arial's and Recursive's slanted text keeps the upright ones).
pub fn char_width_em_styled(c: char, bold: bool, italic: bool) -> f64 {
    if mark::is_mark(c) || crate::icons::is_tag(c) {
        return 0.0;
    }
    if let Some(w) = face().data(italic).and_then(|d| d.advance_em(c)) {
        return w;
    }
    if face() == Face::Serif {
        if let Some(w) = serif::advance_em(c) {
            return w;
        }
    }
    if let Some(w) = face().mono_advance_em() {
        return w;
    }
    arial_char_width_em(c, bold)
}

/// Arial's advance width of one character in em units, whatever the measuring
/// face.
pub fn arial_char_width_em(c: char, bold: bool) -> f64 {
    let table = if bold { &ARIAL_BOLD } else { &ARIAL };
    let code = c as u32;
    if mark::is_mark(c) || crate::icons::is_tag(c) {
        return 0.0;
    }
    if crate::icons::is_icon(c) {
        return crate::icons::advance_em(c);
    }
    let units = match code {
        0x20..=0x7E => f64::from(table[(code - 0x20) as usize]) * 1000.0 / 2048.0,
        // A no-break space is as wide as a space.
        0xA0 => f64::from(table[0]) * 1000.0 / 2048.0,
        0xA1..=0x24F => 556.0,
        // CJK, Hangul, full-width forms and emoji are roughly square.
        0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFF00..=0xFF60 => {
            1000.0
        }
        0x1F000..=0x1FAFF => 1000.0,
        _ => 600.0,
    };
    units / 1000.0
}

/// Arial's kerning pairs (in 1/2048 em) as Chromium applies them to regular
/// text (`font-kerning: auto`, measured in headless Chromium): "DEPARTMENT" is
/// 1.3px narrower at 14px than its advance widths (`PA`, `RT`).
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
    ("RY", -37),
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
    ("Y;", -133),
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

/// Arial Bold's kerning pairs (in 1/2048 em), measured as for [`KERNING`]:
/// bold labels (class and entity names, group titles in bold) are kerned too.
const KERNING_BOLD: &[(&str, i16)] = &[
    (" A", -76),
    (" Y", -37),
    ("11", -113),
    ("A ", -76),
    ("AT", -152),
    ("AV", -152),
    ("AW", -113),
    ("AY", -188),
    ("Av", -76),
    ("Aw", -37),
    ("Ay", -76),
    ("F,", -227),
    ("F.", -227),
    ("FA", -113),
    ("L ", -37),
    ("LT", -152),
    ("LV", -152),
    ("LW", -113),
    ("LY", -188),
    ("Ly", -76),
    ("P ", -37),
    ("P,", -264),
    ("P.", -264),
    ("PA", -152),
    ("RV", -37),
    ("RW", -37),
    ("RY", -76),
    ("T,", -227),
    ("T-", -113),
    ("T.", -227),
    ("T:", -227),
    ("T;", -227),
    ("TA", -152),
    ("TO", -37),
    ("Ta", -152),
    ("Tc", -152),
    ("Te", -152),
    ("Ti", -37),
    ("To", -152),
    ("Tr", -113),
    ("Ts", -152),
    ("Tu", -152),
    ("Tw", -152),
    ("Ty", -152),
    ("V,", -188),
    ("V-", -113),
    ("V.", -188),
    ("V:", -113),
    ("V;", -113),
    ("VA", -152),
    ("Va", -113),
    ("Ve", -113),
    ("Vi", -37),
    ("Vo", -152),
    ("Vr", -113),
    ("Vu", -76),
    ("Vy", -76),
    ("W,", -113),
    ("W-", -41),
    ("W.", -113),
    ("W:", -37),
    ("W;", -37),
    ("WA", -113),
    ("Wa", -76),
    ("We", -37),
    ("Wi", -18),
    ("Wo", -37),
    ("Wr", -37),
    ("Wu", -37),
    ("Wy", -37),
    ("Y ", -37),
    ("Y,", -227),
    ("Y-", -113),
    ("Y.", -227),
    ("Y:", -152),
    ("Y;", -152),
    ("YA", -188),
    ("Ya", -113),
    ("Ye", -113),
    ("Yi", -76),
    ("Yo", -152),
    ("Yp", -113),
    ("Yq", -152),
    ("Yu", -113),
    ("Yv", -113),
    ("r,", -113),
    ("r.", -113),
    ("v,", -152),
    ("v.", -152),
    ("w,", -76),
    ("w.", -76),
    ("y,", -152),
    ("y.", -152),
];

/// The kerning between two characters, in em: the measuring face's when it
/// draws both, none across a font fallback, Arial's between two characters
/// Arial draws.
fn kerning_em(a: char, b: char, bold: bool) -> f64 {
    if let Some(d) = face().data(false) {
        match (d.has(a), d.has(b)) {
            (true, true) => return d.kern_em(a, b),
            (false, false) => {}
            _ => return 0.0,
        }
    }
    if face().mono_advance_em().is_some() {
        return 0.0;
    }
    if face() == Face::Serif && a.is_ascii() && b.is_ascii() {
        return serif::kerning_em(a, b);
    }
    if !a.is_ascii() || !b.is_ascii() {
        return 0.0;
    }
    let table = if bold { KERNING_BOLD } else { KERNING };
    table
        .iter()
        .find(|(p, _)| p.as_bytes() == [a as u8, b as u8])
        .map_or(0.0, |&(_, k)| f64::from(k) / 2048.0)
}

/// Width of a single line in pixels in the measuring face ([`face`]): its
/// advances, kerning and ligatures (Open Sans' `fi`, `ffl`, ...), each styled
/// run in its own weight and slant.
pub fn line_width(text: &str, font_size: f64, bold: bool) -> f64 {
    let chars: Vec<char> = text.chars().filter(|&c| c != mark::RUN).collect();
    let mut st = Emphasis::default();
    let mut prev: Option<char> = None;
    let mut px = 0.0;
    let mut i = 0;
    let pos = |em: f64| glyph_px(em, font_size);
    while i < chars.len() {
        let c = chars[i];
        if st.apply(c) {
            // A change of style starts a new run, which is not kerned across.
            prev = None;
            i += 1;
            continue;
        }
        let b = bold || st.bold > 0;
        let italic = st.italic > 0;
        if let Some(p) = prev {
            px += pos(kerning_em(p, c, b));
        }
        if let Some((n, w)) = face().data(italic).and_then(|d| d.ligature_at(&chars[i..])) {
            px += pos(w);
            prev = Some(chars[i + n - 1]);
            i += n;
            continue;
        }
        px += pos(char_width_em_styled(c, b, italic));
        prev = Some(c);
        i += 1;
    }
    px
}

/// One glyph's advance (or kerning) of `em` at `font_size` as HarfBuzz hands
/// it to Blink: in a web font, rounded to its 16.16 fixed point (1/65536 px;
/// Recursive's 1000-unit em makes 9.6px digits 9.6000061px, so ten of them
/// round up to 96.015625px as a layout unit). Arial's 2048-unit advances are
/// left exact, as its tables were fitted.
fn glyph_px(em: f64, font_size: f64) -> f64 {
    let v = em * font_size;
    if matches!(face(), Face::Arial | Face::Serif | Face::Mono | Face::Courier) {
        v
    } else {
        (v * 65536.0).round() / 65536.0
    }
}

/// Horizontal ink extents (`xMin`, `xMax`) of ASCII 0x21..=0x7E in 1/2048 em:
/// Liberation Sans' (`glyf`), the metric-compatible Arial the browser falls back
/// to. Advance widths are [`ARIAL`]'s.
const ARIAL_INK: [(i16, i16); 94] = [
    (185, 383),
    (87, 640),
    (9, 1129),
    (22, 1106),
    (73, 1748),
    (72, 1334),
    (104, 288),
    (127, 670),
    (12, 555),
    (33, 765),
    (100, 1095),
    (184, 385),
    (91, 591),
    (187, 382),
    (0, 569),
    (80, 1059),
    (156, 1039),
    (103, 1036),
    (78, 1049),
    (47, 1079),
    (82, 1053),
    (104, 1049),
    (105, 1036),
    (89, 1050),
    (96, 1042),
    (187, 382),
    (184, 385),
    (101, 1096),
    (100, 1095),
    (101, 1096),
    (84, 1063),
    (161, 1902),
    (4, 1362),
    (168, 1258),
    (104, 1401),
    (168, 1381),
    (168, 1278),
    (168, 1169),
    (103, 1440),
    (168, 1312),
    (189, 380),
    (32, 872),
    (168, 1343),
    (168, 1071),
    (168, 1538),
    (168, 1312),
    (97, 1495),
    (168, 1258),
    (97, 1495),
    (168, 1384),
    (93, 1272),
    (46, 1204),
    (158, 1321),
    (9, 1357),
    (9, 1926),
    (46, 1323),
    (45, 1321),
    (65, 1187),
    (146, 553),
    (0, 569),
    (16, 423),
    (10, 951),
    (-31, 1162),
    (106, 530),
    (87, 1139),
    (132, 1053),
    (87, 970),
    (86, 1007),
    (87, 1048),
    (29, 572),
    (86, 1007),
    (142, 1006),
    (137, 317),
    (-50, 317),
    (138, 1027),
    (138, 318),
    (136, 1571),
    (136, 1006),
    (86, 1053),
    (132, 1053),
    (86, 1008),
    (136, 648),
    (57, 950),
    (31, 554),
    (133, 1003),
    (7, 1017),
    (-3, 1484),
    (23, 1002),
    (5, 1020),
    (83, 922),
    (34, 648),
    (183, 349),
    (34, 647),
    (92, 1104),
];

/// [`ARIAL_INK`] for Liberation Sans Italic, whose advance widths are the
/// regular face's.
const ARIAL_ITALIC_INK: [(i16, i16); 94] = [
    (77, 549),
    (187, 803),
    (39, 1159),
    (-12, 1170),
    (146, 1733),
    (33, 1256),
    (202, 449),
    (96, 855),
    (-199, 560),
    (120, 852),
    (130, 1125),
    (37, 331),
    (105, 636),
    (80, 318),
    (-116, 745),
    (89, 1122),
    (53, 966),
    (-12, 1109),
    (48, 1121),
    (13, 1062),
    (46, 1144),
    (115, 1139),
    (212, 1202),
    (54, 1127),
    (70, 1097),
    (81, 487),
    (38, 500),
    (131, 1126),
    (130, 1125),
    (131, 1126),
    (159, 1133),
    (194, 1935),
    (-101, 1256),
    (63, 1296),
    (113, 1490),
    (63, 1413),
    (63, 1385),
    (63, 1347),
    (101, 1505),
    (63, 1481),
    (81, 546),
    (-5, 1040),
    (63, 1441),
    (63, 996),
    (63, 1706),
    (63, 1480),
    (111, 1536),
    (63, 1353),
    (101, 1526),
    (63, 1416),
    (58, 1344),
    (184, 1372),
    (153, 1489),
    (177, 1525),
    (177, 2094),
    (-39, 1430),
    (213, 1489),
    (-40, 1308),
    (-39, 738),
    (146, 472),
    (-169, 608),
    (40, 981),
    (-160, 1018),
    (130, 503),
    (46, 1069),
    (29, 1076),
    (67, 1001),
    (69, 1193),
    (69, 1063),
    (69, 746),
    (4, 1121),
    (34, 1045),
    (33, 501),
    (-229, 502),
    (34, 1110),
    (33, 501),
    (34, 1611),
    (34, 1046),
    (67, 1074),
    (-51, 1073),
    (69, 1125),
    (34, 751),
    (5, 982),
    (93, 640),
    (86, 1098),
    (112, 1122),
    (102, 1589),
    (-82, 1084),
    (-116, 1127),
    (-12, 996),
    (21, 833),
    (213, 379),
    (-159, 653),
    (122, 1134),
];

/// [`ARIAL_INK`] for Arial Bold (Liberation Sans Bold's `glyf`).
const ARIAL_BOLD_INK: [(i16, i16); 94] = [
    (193, 487),
    (135, 836),
    (35, 1106),
    (27, 1110),
    (51, 1767),
    (90, 1415),
    (109, 381),
    (102, 680),
    (2, 580),
    (6, 795),
    (86, 1113),
    (139, 432),
    (80, 600),
    (139, 428),
    (20, 549),
    (81, 1055),
    (129, 1082),
    (71, 1057),
    (47, 1065),
    (31, 1128),
    (63, 1082),
    (75, 1065),
    (88, 1049),
    (65, 1076),
    (71, 1063),
    (197, 485),
    (195, 487),
    (86, 1113),
    (85, 1112),
    (86, 1113),
    (94, 1133),
    (117, 1878),
    (51, 1425),
    (137, 1386),
    (84, 1423),
    (137, 1393),
    (137, 1286),
    (137, 1176),
    (84, 1466),
    (137, 1341),
    (137, 432),
    (31, 999),
    (137, 1460),
    (137, 1188),
    (137, 1569),
    (137, 1341),
    (84, 1507),
    (137, 1296),
    (84, 1507),
    (137, 1437),
    (59, 1286),
    (23, 1229),
    (123, 1354),
    (14, 1352),
    (2, 1931),
    (18, 1348),
    (35, 1333),
    (61, 1192),
    (115, 657),
    (21, 550),
    (25, 567),
    (45, 1151),
    (-20, 1157),
    (66, 575),
    (60, 1152),
    (135, 1167),
    (80, 1079),
    (84, 1116),
    (80, 1069),
    (35, 686),
    (84, 1114),
    (143, 1124),
    (143, 424),
    (-32, 425),
    (143, 1141),
    (143, 424),
    (135, 1694),
    (135, 1124),
    (80, 1171),
    (135, 1167),
    (84, 1114),
    (135, 766),
    (72, 1055),
    (25, 657),
    (127, 1116),
    (8, 1130),
    (-6, 1597),
    (14, 1124),
    (16, 1128),
    (82, 950),
    (33, 754),
    (156, 418),
    (43, 766),
    (81, 1119),
];

/// Width of an SVG `<text>`'s `getBBox()` in Chromium (regular weight): the run's
/// advance (kerned, rounded up to 1/64 px) united with each glyph's ink box,
/// which Skia rounds out to whole pixels from the glyph's origin. So a label
/// usually ends on its last glyph's ink rounded up (`e` at 16px: 9, not its
/// 8.898 advance). White space collapses as SVG's does, and none takes room at
/// either end. Characters outside ASCII count by their advance alone.
pub fn svg_bbox_width(text: &str, font_size: f64) -> f64 {
    let (left, right) = svg_bbox_extent(text, font_size, false);
    right - left
}

/// [`svg_bbox_width`] in regular or bold weight.
pub fn svg_bbox_width_weight(text: &str, font_size: f64, bold: bool) -> f64 {
    let (left, right) = svg_bbox_extent_styled(text, font_size, bold, false);
    right - left
}

/// The left and right edges of an SVG `<text>`'s `getBBox()` relative to its
/// start (see [`svg_bbox_width`]), upright or italic (`font-style: italic`,
/// whose glyphs lean past their advance).
pub fn svg_bbox_extent(text: &str, font_size: f64, italic: bool) -> (f64, f64) {
    svg_bbox_extent_styled(text, font_size, false, italic)
}

/// The ink extent of `c` in one of the ink tables: ASCII, and the guillemets
/// that wrap a class annotation (`«interface»`), whose ink reaches past their
/// advance (Liberation Sans' `glyf`; other characters count by their advance).
fn glyph_ink(table: &[(i16, i16); 94], c: char) -> Option<(i16, i16)> {
    let face = if std::ptr::eq(table, &ARIAL_BOLD_INK) {
        1
    } else if std::ptr::eq(table, &ARIAL_ITALIC_INK) {
        2
    } else {
        0
    };
    match c {
        '!'..='~' => Some(table[(c as u32 - 0x21) as usize]),
        '«' => Some([(83, 1056), (92, 1047), (76, 1131)][face]),
        '»' => Some([(83, 1056), (93, 1048), (5, 1061)][face]),
        _ => None,
    }
}

/// The horizontal ink extent of `c` in em (`xMin`, `xMax`) in the measuring
/// face, or Arial's for characters it lacks (`None`: count by the advance).
fn ink_em(c: char, bold: bool, italic: bool) -> Option<(f64, f64)> {
    if let Some(d) = face().data(italic) {
        if let Some((lo, hi, _, _)) = d.ink_em(c, bold) {
            // (Open Sans' synthetic bold: `Face::synthetic_ink_px`.)
            return Some((lo, hi));
        }
    }
    if face().mono_advance_em().is_some() {
        return None;
    }
    let table = if bold {
        &ARIAL_BOLD_INK
    } else if italic {
        &ARIAL_ITALIC_INK
    } else {
        &ARIAL_INK
    };
    glyph_ink(table, c).map(|(lo, hi)| (f64::from(lo) / 2048.0, f64::from(hi) / 2048.0))
}

/// A glyph's ink edges (in em) as Skia rounds them out to whole pixels from
/// the glyph's origin. FreeType first loads the outline in 26.6 fixed point,
/// so an edge 0.006px past a pixel (Open Sans' `S` at 12px) stays on it;
/// Arial's edges keep the 0.005px tolerance they were fitted with.
fn ink_px(lo: f64, hi: f64, size: f64) -> (f64, f64) {
    if face() == Face::Arial {
        ((lo * size + 0.005).floor(), (hi * size - 0.005).ceil())
    } else {
        let q = |v: f64| (v * size * 64.0).round() / 64.0;
        (q(lo).floor(), q(hi).ceil())
    }
}

/// [`svg_bbox_extent`] for any weight: bold (`font-weight` 600 and up, drawn
/// with the bold face) takes the bold advances and ink.
pub fn svg_bbox_extent_styled(text: &str, font_size: f64, bold: bool, italic: bool) -> (f64, f64) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = text.chars().filter(|&c| !mark::is_mark(c)).collect();
    let mut x = 0.0f64;
    let (mut left, mut right) = (0.0f64, 0.0f64);
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(p) = prev {
            x += glyph_px(kerning_em(p, c, bold), font_size);
        }
        // A `liga` ligature (Open Sans' ff, fi, fl, ffi, ffl) is one glyph,
        // with its own advance and ink.
        let lig = face()
            .data(italic)
            .and_then(|d| d.ligature_at(&chars[i..]))
            .and_then(|(k, adv)| {
                let s: String = chars[i..i + k].iter().collect();
                let size = if bold {
                    shaping_size(font_size)
                } else {
                    font_size
                };
                Some((k, adv, face().ligature_ink_px(&s, bold, italic, size)?))
            });
        let (k, advance, ink) = match lig {
            Some((k, adv, ink)) => (k, adv, Some(ink)),
            None => {
                let ink = (c != ' ')
                    .then(|| {
                        face()
                            .synthetic_ink_px(c, bold, italic, shaping_size(font_size))
                            .or_else(|| {
                                ink_em(c, bold, italic).map(|(lo, hi)| ink_px(lo, hi, font_size))
                            })
                    })
                    .flatten();
                (1, char_width_em_styled(c, bold, italic), ink)
            }
        };
        if let Some((lo, hi)) = ink {
            left = left.min(x + lo);
            right = right.max(x + hi);
        }
        x += glyph_px(advance, font_size);
        prev = Some(chars[i + k - 1]);
        i += k;
    }
    (left, right.max(layout_unit_ceil(x)))
}

/// Width of one HTML label line: its styled runs, each rounded up to a layout unit.
pub fn html_line_width(line: &str, font_size: f64, bold: bool) -> f64 {
    styled_runs(line, bold)
        .iter()
        .map(|r| html_run_width(r, font_size))
        .sum()
}

/// One run's HTML width: its text rounded up to the layout unit. A pack icon
/// is an inline `<svg>` box instead, snapped to the nearest layout unit, and
/// splits the text around it.
fn html_run_width(r: &StyledRun, font_size: f64) -> f64 {
    if !r.text.chars().any(crate::icons::is_pack_icon) {
        return layout_unit_ceil(run_width(r, font_size));
    }
    let mut total = 0.0;
    let mut text = String::new();
    for c in r.text.chars() {
        if crate::icons::is_pack_icon(c) {
            let seg = StyledRun {
                text: std::mem::take(&mut text),
                ..r.clone()
            };
            total += layout_unit_ceil(run_width(&seg, font_size));
            total += (crate::icons::advance_em(c) * font_size * 64.0).round() / 64.0;
        } else {
            text.push(c);
        }
    }
    let seg = StyledRun { text, ..r.clone() };
    total + layout_unit_ceil(run_width(&seg, font_size))
}

/// Width of one styled run ([`styled_runs`]) in its weight and slant.
pub fn run_width(r: &StyledRun, font_size: f64) -> f64 {
    if r.italic {
        let marked = format!("{}{}{}", mark::ITALIC_ON, r.text, mark::ITALIC_OFF);
        line_width(&marked, font_size, r.bold)
    } else {
        line_width(&r.text, font_size, r.bold)
    }
}

/// `w` rounded up to Chromium's layout unit (1/64 px), as an HTML box's
/// width from its text is (a hair of float error is not rounded up).
pub fn layout_unit_ceil(w: f64) -> f64 {
    ((w * 64.0) - 1e-6).ceil().max(0.0) / 64.0
}

/// The font size Chromium shapes text at. Blink's
/// `FontDescription::EffectiveFontSize()` cuts the computed size (a `float`)
/// to 1/100 px in single precision, `floorf(size * 100) / 100`, and Skia hands
/// that to FreeType in 26.6 fixed point, `(int)(size * 64)`: 11.48px is laid
/// out at 11.46875px, and 9.9px (`9.9f * 100` is 989.99994) at 9.875px. Fitted
/// exactly to the advances Chromium measures for 9401 sizes from 6 to 60px.
/// (Blink's font cache keys sizes by `(unsigned)(effective * 100)`, so two
/// effective sizes a hundredth apart can share the first one's font; a page
/// that uses only one of them is not affected.)
pub fn shaping_size(font_size: f64) -> f64 {
    let effective = (font_size as f32 * 100.0).floor() / 100.0;
    f64::from((effective * 64.0) as i32) / 64.0
}

/// Screen pixels per user unit of a `width` × `height` viewBox drawn as mmdc
/// draws a `useMaxWidth` svg: 100% of its 784px page wide (never wider than
/// `width`), its height following the aspect ratio cut down to a layout unit,
/// and the viewBox fitted into that box (`meet`, the smaller of the two scales).
/// Text an svg measures while on the page is laid out at its screen size.
pub fn mmdc_page_scale(width: f64, height: f64) -> f64 {
    let w = width.min(784.0);
    if w <= 0.0 || height <= 0.0 {
        return 1.0;
    }
    let h = (w * height / width * 64.0).floor() / 64.0;
    (w / width).min(h / height)
}

/// The box of one SVG text line drawn as mermaid's `createText` draws SVG labels,
/// one `<tspan>` per word (each after the first keeps its leading space), with
/// `text-anchor: start` at 0: `(x, width, advance)` of its `getBBox()` and its
/// advance. Chromium kerns across the tspans (a pair's kerning is part of the
/// first glyph's advance) but lays each tspan out as a fragment whose width is
/// rounded up to 1/64 px, so the line is a little wider than one run; the box
/// unites that advance with each glyph's ink, rounded out to whole pixels from
/// the glyph's origin, as [`svg_bbox_width`] does. Characters outside ASCII
/// count by their advance alone.
pub fn svg_word_runs_box(line: &str, font_size: f64, bold: bool) -> (f64, f64, f64) {
    let size = shaping_size(font_size);
    let chars: Vec<char> = line.chars().filter(|&c| !mark::is_mark(c)).collect();
    let (mut x, mut run) = (0.0f64, 0.0f64);
    let (mut left, mut right) = (0.0f64, 0.0f64);
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && i > 0 {
            x += layout_unit_ceil(run);
            run = 0.0;
        }
        if c != ' ' {
            if let Some((lo, hi)) = face()
                .synthetic_ink_px(c, bold, false, size)
                .or_else(|| ink_em(c, bold, false).map(|(lo, hi)| ink_px(lo, hi, size)))
            {
                left = left.min(x + run + lo);
                right = right.max(x + run + hi);
            }
        }
        let kern = chars.get(i + 1).map_or(0.0, |&n| kerning_em(c, n, bold));
        run += glyph_px(char_width_em(c, bold), size) + glyph_px(kern, size);
    }
    x += layout_unit_ceil(run);
    (left, right.max(x) - left, x)
}
/// An SVG `dominant-baseline` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    Alphabetic,
    Middle,
    Central,
    Hanging,
    TextBeforeEdge,
    TextAfterEdge,
    Mathematical,
    Ideographic,
}

/// The measuring face's ascent above the baseline in whole pixels, as Blink
/// rounds it (Arial: 0.905em, 13px at 14px; Recursive: 0.95em; Open Sans:
/// 2189/2048em).
pub fn ascent_px(font_size: f64) -> f64 {
    (face().vertical_em().0 * font_size).round()
}

/// The measuring face's descent below the baseline in whole pixels (Arial:
/// 0.212em; Recursive: 0.25em; Open Sans: 600/2048em).
pub fn descent_px(font_size: f64) -> f64 {
    (face().vertical_em().1 * font_size).round()
}

/// How far below its `y` Chromium draws the alphabetic baseline of SVG text with
/// `dominant-baseline: b` in the measuring face (Liberation Sans, the Arial the
/// browser falls back to, unless a web font applies). Blink takes the font's
/// ascent and descent rounded to whole pixels (0.905em and 0.212em), its
/// x-height (1082/2048 em) and the hanging baseline (0.8 × ascent) to the
/// 1/64 px layout unit; `middle` is half the x-height, a tie rounding down.
/// Measured in Chromium for 6–60px.
pub fn baseline_shift(b: Baseline, font_size: f64) -> f64 {
    baseline_shift_weight(b, font_size, false)
}

/// [`baseline_shift`] for regular or bold text: `middle` takes the x-height
/// of the weight (Recursive's `MVAR` raises it at `wght` 700).
pub fn baseline_shift_weight(b: Baseline, font_size: f64, bold: bool) -> f64 {
    let ascent = ascent_px(font_size);
    let descent = descent_px(font_size);
    match b {
        Baseline::Alphabetic => 0.0,
        Baseline::Middle => (font_size * face().x_height_em(bold) / 2.0 * 64.0 - 0.5).ceil() / 64.0,
        Baseline::Central => (ascent - descent) / 2.0,
        Baseline::Hanging => (0.8 * ascent * 64.0).round() / 64.0,
        Baseline::TextBeforeEdge => ascent,
        Baseline::TextAfterEdge | Baseline::Ideographic => -descent,
        Baseline::Mathematical => ascent / 2.0,
    }
}

/// Where Chromium paints, on a 1:1 device pixel grid, the alphabetic
/// baseline of text written as `<text y="y" dominant-baseline="central">
/// <tspan dy="0">`: it snaps the tspan's position to a whole pixel and then
/// the shifted baseline, halves rounding up (measured for 12-24px at whole
/// and half pixel positions, and with the `dy` of further lines). Its
/// `getBBox()` (and our `y`) keeps `y` + the central shift unsnapped.
pub fn painted_tspan_central(y: f64, font_size: f64) -> f64 {
    let up = |v: f64| (v + 0.5).floor();
    up(up(y) + baseline_shift(Baseline::Central, font_size))
}

/// An HTML line box's `line-height: normal` in mermaid's font (Liberation
/// Sans): its ascent, descent and line gap (67/2048em), each rounded to
/// whole pixels (18px at 16px).
pub fn normal_line_height(font_size: f64) -> f64 {
    let (a, d, g) = face().vertical_em();
    (a * font_size).round() + (d * font_size).round() + (g * font_size).round()
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
    /// Per line, how far left of centre it is drawn: a line an HTML label
    /// wrapped at a space keeps that space (`white-space: break-spaces`, see
    /// [`break_space_shift`]). Empty: no line is shifted.
    pub shifts: Vec<f64>,
}

/// Line pitch of SVG text labels in em (`createFormattedText`'s `lineHeight`).
pub const SVG_LINE_HEIGHT: f64 = 1.1;

/// How far below the baseline a glyph's ink reaches, in em, for the glyphs that
/// can reach past the font's rounded descent (0 for the rest). Measured in
/// Chromium as the font sizes (6..40px) at which a lone glyph's `getBBox()`
/// grows: `g`, `p`, brackets, ... from about 0.2075em, `|` and the cedillas
/// further, `Q` and `_` less (only where the descent rounds down by more than
/// 0.01em); `@` and `$` never.
fn descender_depth(c: char) -> f64 {
    if let Some(d) = face().data(false) {
        if let Some((_, _, y0, _)) = d.ink_em(c, false) {
            return (-y0).max(0.0);
        }
    }
    match c {
        '|' | 'ç' | 'Ç' => 0.22,
        'g' | 'j' | 'p' | 'q' | 'y' | '(' | ')' | '[' | ']' | '{' | '}' | 'µ' | 'þ' | 'ÿ' | 'ý' => {
            0.2075
        }
        'Q' | '_' => 0.195,
        _ => 0.0,
    }
}

/// Height of one SVG text line's bounding box, as Chromium's `getBBox()` reports it
/// for mermaid's fonts (Arial metrics, ascent 0.905em and descent 0.212em, each
/// rounded to whole pixels; a glyph whose ink reaches further down extends it to
/// that ink, rounded out): 16px at 14px, 17px (18px with a `g`) at 16px.
pub fn svg_line_box(font_size: f64, line: &str) -> f64 {
    let ascent = ascent_px(font_size);
    let descent = descent_px(font_size);
    let ink = line.chars().map(descender_depth).fold(0.0, f64::max);
    ascent + descent.max((ink * font_size - 1e-9).ceil())
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

    /// An SVG label drawn centred on `center` by its text's bounding box, as
    /// `computeLabelTransform` centres edge and terminal labels (it takes the
    /// `getBBox()` origin into account, unlike `labelHelper`): the box top is
    /// the first line's ascent above its baseline, 1em below the label's
    /// origin, and a centred line's ink need not be symmetric about its anchor.
    /// HTML blocks are drawn as [`TextBlock::centered_text`] draws them.
    pub fn bbox_centered_text(&self, center: Point, fill: &Color) -> Text {
        if !self.svg {
            return self.centered_text(center, fill);
        }
        let (left, right) = self.svg_ink_extent();
        let ascent = ascent_px(self.font_size);
        Text {
            x: center.x - (left + right) / 2.0,
            y: center.y + ascent - self.height / 2.0,
            ..Text::centered(center, &self.lines, self.font_size, self.line_height, fill)
        }
    }

    /// The horizontal extent of an SVG block's `getBBox()` about its anchor,
    /// its lines centred (`text-anchor: middle`).
    pub fn svg_ink_extent(&self) -> (f64, f64) {
        self.lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let (x, w, advance) = svg_word_runs_box(l, self.font_size, self.bold);
                (x - advance / 2.0, x + w - advance / 2.0)
            })
            .fold(None, |acc: Option<(f64, f64)>, (a, b)| {
                Some(acc.map_or((a, b), |(l, r)| (l.min(a), r.max(b))))
            })
            .unwrap_or((0.0, 0.0))
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
            shifts: Vec::new(),
        }
    }

    /// Normalise a raw mermaid label and measure it.
    pub fn label(raw: &str, font_size: f64) -> Self {
        Self::new(label_lines(raw), font_size, false)
    }

    /// The block as an HTML label's box measures it: Chromium sizes a box
    /// in layout units (1/64 px), its content's width rounded up to one.
    /// (SVG text's `getBBox` is not snapped.)
    pub fn html(mut self) -> Self {
        self.width = layout_unit_ceil(self.width);
        self
    }

    /// [`TextBlock::html`] for lines with emphasis: Chromium sizes each styled run
    /// (a `<strong>` or `<em>` and the text between them) in layout units, so a
    /// line is the sum of its runs, each rounded up.
    pub fn html_runs(mut self) -> Self {
        self.width = self
            .lines
            .iter()
            .map(|l| html_line_width(l, self.font_size, self.bold))
            .fold(0.0, f64::max);
        self
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
///
/// [`BreakRule::Browser`] follows `createText`'s HTML label: laid out on one
/// line (`white-space: nowrap`) while that fits `max_width`; otherwise in a
/// `display: table` box of that width with `white-space: break-spaces`, where
/// a space stays at the end of its line and must fit too, and the box widens
/// to its widest unbreakable piece (see [`html_wrap_width`]).
pub fn wrap_lines(
    lines: &[String],
    max_width: f64,
    font_size: f64,
    bold: bool,
    rule: BreakRule,
) -> Vec<String> {
    wrap_lines_flagged(lines, max_width, font_size, bold, rule)
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

/// [`wrap_lines`], each line paired with whether it broke at a space. Under
/// `break-spaces` that space stays at the line's end and keeps its width, so a
/// centred line sits half a space left of centre (see [`break_space_shift`]).
pub fn wrap_lines_flagged(
    lines: &[String],
    max_width: f64,
    font_size: f64,
    bold: bool,
    rule: BreakRule,
) -> Vec<(String, bool)> {
    let paras: Vec<String> = lines
        .iter()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let width = match rule {
        BreakRule::Spaces => max_width,
        BreakRule::Browser => {
            let natural = paras
                .iter()
                .map(|p| line_width(p, font_size, bold))
                .fold(0.0, f64::max);
            if natural <= max_width {
                return balance_marks(paras)
                    .into_iter()
                    .map(|l| (l, false))
                    .collect();
            }
            html_wrap_width(&paras, max_width, font_size, bold)
        }
    };
    let mut out = Vec::new();
    let mut at_space = Vec::new();
    for para in &paras {
        let mut cur = String::new();
        for seg in segments(para, rule) {
            let cand = format!("{cur}{seg}");
            let used = match rule {
                BreakRule::Spaces => line_width(cand.trim_end(), font_size, bold),
                BreakRule::Browser => line_width(&cand, font_size, bold),
            };
            if !plain(&cur).trim().is_empty() && used > width {
                at_space.push(cur.ends_with(' '));
                out.push(std::mem::replace(&mut cur, seg).trim_end().to_string());
            } else {
                cur = cand;
            }
        }
        at_space.push(false);
        out.push(cur.trim_end().to_string());
    }
    balance_marks(out).into_iter().zip(at_space).collect()
}

/// How far left of centre a centred `break-spaces` line that broke at a space
/// sits: half the width its kept trailing space adds.
pub fn break_space_shift(line: &str, font_size: f64, bold: bool) -> f64 {
    (line_width(&format!("{line} "), font_size, bold) - line_width(line, font_size, bold)) / 2.0
}

/// The width an HTML label wrapped at `max_width` takes ([`BreakRule::Browser`]):
/// `max_width`, or its widest unbreakable piece (a word with the space after
/// it) when that is wider, since a table box cannot shrink below it. Chromium
/// measures a piece between its start and end offsets in the paragraph, each
/// rounded to the nearest layout unit (1/64 px).
pub fn html_wrap_width(lines: &[String], max_width: f64, font_size: f64, bold: bool) -> f64 {
    let lu = |w: f64| (w * 64.0 + 0.5).floor();
    let mut min_content = 0.0f64;
    for para in lines {
        let para = para.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut prefix = String::new();
        for seg in segments(&para, BreakRule::Browser) {
            let start = lu(line_width(&prefix, font_size, bold));
            prefix.push_str(&seg);
            let end = lu(line_width(&prefix, font_size, bold));
            min_content = min_content.max((end - start) / 64.0);
        }
    }
    max_width.max(min_content)
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
    // A pack icon's name (tag characters) stays with its icon.
    if crate::icons::is_tag(c) {
        return true;
    }
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
        } else if (is_cjk(c)
            || is_cjk(n)
            || crate::icons::is_icon(c)
            || crate::icons::is_tag(c)
            || crate::icons::is_icon(n))
            && !no_break_before(n)
            && !no_break_after(c)
        {
            // An icon is an inline-block: the line may break on either side of it.
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
    fn baseline_shifts_match_chromium() {
        // getBBox().y with each dominant-baseline minus the alphabetic one, in Chromium.
        use Baseline::*;
        let cases: [(f64, [f64; 6]); 7] = [
            (8.0, [2.109375, 2.5, 5.59375, 7.0, -2.0, 3.5]),
            (12.0, [3.171875, 4.0, 8.796875, 11.0, -3.0, 5.5]),
            (14.0, [3.703125, 5.0, 10.40625, 13.0, -3.0, 6.5]),
            (16.0, [4.21875, 5.5, 11.203125, 14.0, -3.0, 7.0]),
            (20.0, [5.28125, 7.0, 14.40625, 18.0, -4.0, 9.0]),
            (25.0, [6.609375, 9.0, 18.40625, 23.0, -5.0, 11.5]),
            (48.0, [12.671875, 16.5, 34.40625, 43.0, -10.0, 21.5]),
        ];
        for (fs, want) in cases {
            let got = [
                baseline_shift(Middle, fs),
                baseline_shift(Central, fs),
                baseline_shift(Hanging, fs),
                baseline_shift(TextBeforeEdge, fs),
                baseline_shift(TextAfterEdge, fs),
                baseline_shift(Mathematical, fs),
            ];
            assert_eq!(got, want, "{fs}px");
        }
    }

    #[test]
    fn html_labels_snap_up_to_layout_units() {
        assert_eq!(layout_unit_ceil(10.0), 10.0);
        assert_eq!(layout_unit_ceil(10.0 + 1e-9), 10.0);
        assert_eq!(layout_unit_ceil(10.001), 10.015625);
        // Edge and state labels as mermaid 12.3.0 sizes their foreignObjects.
        for (s, w) in [
            ("some edge", 69.28125),
            ("yes indeed", 67.71875),
            ("start moving now", 105.828125),
            ("oh no wobble", 83.296875),
        ] {
            let b = TextBlock::label(s, 14.0).html();
            assert_eq!(b.width, w, "{s}");
        }
        // SVG text is measured as is.
        let svg = TextBlock::svg(vec!["some edge".into()], 14.0, false);
        assert!(svg.width < 69.28125);
    }

    /// Chromium's `getBBox()` of slanted Recursive (mmdc's fonts, no italic
    /// face): the right edge of each glyph's sheared ink rounded up from its
    /// origin. Single glyphs whose outermost point lands a hair past a pixel
    /// (`k` at 12px: 8.0004 by the unrounded outline, 9 in Chromium; at 18px
    /// 12.0007, 12 in Chromium) pin the 26.6 rounding before the shear.
    #[test]
    fn slanted_recursive_boxes_follow_chromium() {
        with_face(Face::Recursive, || {
            let right =
                |s: &str, size: f64, bold: bool| svg_bbox_extent_styled(s, size, bold, true).1;
            for (s, size, bold, want) in [
                ("k", 12.0, false, 9.0),
                ("k", 18.0, false, 12.0),
                ("k", 24.0, false, 17.0),
                ("E", 12.0, false, 9.0),
                ("E", 18.0, false, 13.0),
                ("t", 14.0, true, 9.0),
                ("U", 16.0, true, 13.0),
                ("<", 18.0, true, 13.0),
                ("`", 14.0, true, 6.0),
            ] {
                assert_eq!(right(s, size, bold), want, "{s} {size} {bold}");
            }
            // Whole strings (probe: 74.79971313476562, 25.519927978515625).
            assert!((right("dash name", 14.0, false) - 74.7997).abs() < 1e-3);
            assert!((right("yes", 14.0, false) - 25.5199).abs() < 1e-3);
            assert_eq!(svg_bbox_extent_styled(")", 14.0, false, true).0, -1.0);
        });
    }

    /// Chromium's `getBBox()` of emboldened Open Sans (mmdc loads no bold
    /// face): each glyph's outline stroked `size * k` wide, the probes'
    /// `(x, x + width)` (13.3px is laid out at its shaping size, 13.296875).
    #[test]
    fn emboldened_open_sans_boxes_follow_chromium() {
        with_face(Face::OpenSans, || {
            for (s, size, italic, want) in [
                ("www", 14.0, false, (-1.0, 33.697265625)),
                ("iwB", 18.0, false, (0.0, 30.125)),
                ("yes", 13.296875, false, (-1.0, 21.1279296875)),
                ("Alice", 11.0, false, (-1.0, 23.96875)),
                ("AV To Ty", 11.0, false, (-1.0, 43.962890625)),
                ("fi fl ffi", 14.0, false, (-1.0, 36.76171875)),
                ("State1", 16.0, true, (0.0, 44.4375)),
                ("yes", 13.296875, true, (-2.0, 18.745010375976562)),
                ("Alice", 11.0, true, (-2.0, 22.7041015625)),
                ("fi fl ffi", 14.0, true, (-2.0, 36.064453125)),
            ] {
                let (l, r) = svg_bbox_extent_styled(s, size, true, italic);
                assert!(
                    l == want.0 && (r - want.1).abs() < 1e-3,
                    "{s} {size} {italic}: {l} {r}"
                );
            }
        });
    }

    #[test]
    fn svg_bbox_width_follows_chromium_get_bbox() {
        // Chromium 16px arial `getBBox().width`.
        for (t, w) in [
            ("true", 27.671875),
            ("false", 33.8984375),
            ("element", 57.4765625),
            ("member", 59.3515625),
            ("{", 6.0),
            (",", 4.453125),
            ("e ", 9.0),
            ("  a  b", 22.34375),
            ("AVx", 28.15625),
            ("Tox", 24.90625),
            ("WW", 31.1015625),
            ("u", 8.90625),
        ] {
            assert!(
                (svg_bbox_width(t, 16.0) - w).abs() < 1e-6,
                "{t}: {}",
                svg_bbox_width(t, 16.0)
            );
        }
        // Other sizes: a glyph's ink is rounded out at its own size.
        assert!((svg_bbox_width("F", 14.0) - 8.5625).abs() < 1e-9);
        assert!((svg_bbox_width("W", 18.0) - 17.0).abs() < 1e-9);
        assert!((svg_bbox_width("/", 18.0) - 5.015625).abs() < 1e-9);
    }

    #[test]
    fn tspan_central_paints_where_chromium_does() {
        // Chromium (1:1), `<text dominant-baseline=central><tspan dy=0>`: the
        // whole-pixel alphabetic baseline it paints, for y and font size.
        for (y, size, painted) in [
            (36.5, 16.0, 43.0),
            (50.0, 16.0, 56.0),
            (50.5, 16.0, 57.0),
            (50.0, 12.0, 54.0),
            (50.5, 12.0, 55.0),
            (50.5, 14.0, 56.0),
            (50.5, 18.0, 57.0),
            (50.5, 20.0, 58.0),
            (50.0, 24.0, 59.0),
            (50.5, 24.0, 60.0),
            (42.5, 16.0, 49.0),
        ] {
            assert_eq!(painted_tspan_central(y, size), painted, "{y} {size}");
        }
    }

    #[test]
    fn guillemets_reach_past_their_advance() {
        // mermaid 12.3.0: `«iface»` at 14px, translate(-22.6792) (half its box).
        let (l, r) = svg_bbox_extent("«iface»", 14.0, false);
        assert!((r - l - 45.358_398_437_5).abs() < 0.02, "{}", r - l);
    }

    #[test]
    fn svg_line_box_follows_chromium_get_bbox() {
        assert_eq!(svg_line_box(14.0, "Christmas"), 16.0);
        assert_eq!(svg_line_box(14.0, "ggg"), 16.0);
        assert_eq!(svg_line_box(16.0, "Christmas"), 17.0);
        assert_eq!(svg_line_box(16.0, "Laptop"), 18.0);
        assert_eq!(svg_line_box(16.0, "(x)"), 18.0);
        // Chromium (mermaid's fonts): `_` and `Q` reach below the rounded descent
        // at 16px but not at 10px; `@` and `$` never; `g` at 10px does.
        assert_eq!(svg_line_box(10.0, "RC_1"), 11.0);
        assert_eq!(svg_line_box(16.0, "RC_1"), 18.0);
        assert_eq!(svg_line_box(11.0, "Q"), svg_line_box(11.0, "g"));
        assert_eq!(svg_line_box(16.0, "@$"), 17.0);
        assert_eq!(svg_line_box(10.0, "g"), 12.0);
        // `|` also at 19px, where `g` stays within.
        assert_eq!(svg_line_box(19.0, "|"), svg_line_box(19.0, "x") + 1.0);
        assert_eq!(svg_line_box(19.0, "g"), svg_line_box(19.0, "x"));
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
        // Bold text is kerned with Arial Bold's pairs ("Te": -152, not -227).
        let unkerned = (char_width_em('T', true) + char_width_em('e', true)) * 14.0;
        let kerned = unkerned - 152.0 / 2048.0 * 14.0;
        assert!((line_width("Te", 14.0, true) - kerned).abs() < 1e-9);
        assert!(line_width("Te", 14.0, false) < line_width("Tn", 14.0, false));
        // Chromium (exact, before its layout rounds up to 1/64px): 210.5879px
        // bold, and the 52-character namespace title 405.1729px regular.
        assert!((line_width("DELIVERY-ADDRESS Type Year", 14.0, true) - 210.5879).abs() < 1e-3);
        let ns = "AVeryLongNamespaceNameThatGoesOnAndOnForeverAndEver";
        assert!((line_width(ns, 14.0, false) - 405.1729).abs() < 1e-3);
        // Advance widths in Arial's 1/2048 em.
        let units = 1479.0 + 1139.0 + 455.0 + 455.0 + 1139.0;
        assert!((w - 16.0 * units / 2048.0).abs() < 1e-9);
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

    #[test]
    fn browser_wrapping_keeps_the_space_on_its_line() {
        // `white-space: break-spaces`: the space after a line's last word
        // takes room ("...kappa leaf " is 202.77px), as Chromium wraps it.
        let w = |s: &str, max: f64| wrap(s, max, 16.0, false, BreakRule::Browser);
        assert_eq!(
            w("rho W Long-term kappa leaf alpha lambda Long-term i", 200.0),
            ["rho W Long-term kappa", "leaf alpha lambda Long-", "term i"]
        );
        assert_eq!(
            w("iiii map omega kappa map node epsilon node", 120.0),
            ["iiii map omega", "kappa map", "node epsilon", "node"]
        );
        // A label that fits stays on one line (`nowrap`), even when its last
        // word only fits without a space after it.
        assert_eq!(w("kappa map node", 120.0), ["kappa map node"]);
    }

    #[test]
    fn browser_wrapping_widens_to_the_longest_word() {
        // The table box cannot be narrower than "internationalization "
        // (137.87px), so the other lines wrap at that width too.
        let text = "research nu iiii branch lambda kappa internationalization eta mind l omicron xi research Long-term";
        let lines = wrap(text, 120.0, 16.0, false, BreakRule::Browser);
        assert_eq!(
            lines,
            [
                "research nu iiii",
                "branch lambda",
                "kappa",
                "internationalization",
                "eta mind l omicron",
                "xi research Long-",
                "term"
            ]
        );
        // "internationalization " is 137.8671875px, half a layout unit: it
        // starts half a unit in here (rounded up), so it spans 8823/64 px...
        let paras = [text.to_string()];
        assert_eq!(html_wrap_width(&paras, 120.0, 16.0, false), 137.859375);
        assert_eq!(html_wrap_width(&paras, 200.0, 16.0, false), 200.0);
        // ... and 8824/64 px from a whole unit (as Chromium sizes both boxes).
        let paras = ["theta internationalization eta alpha lambda".to_string()];
        assert_eq!(html_wrap_width(&paras, 120.0, 16.0, false), 137.875);
    }

    #[test]
    fn word_runs_box_as_chromium_lays_out_create_text_tspans() {
        // getBBox() of one tspan per word in headless Chromium (Liberation Sans):
        // (x, width, advance) with text-anchor start.
        let b = |s: &str, f: f64| svg_word_runs_box(s, f, false);
        assert_eq!(b("Stores all of the core banking", 11.48).1, 148.609375);
        assert_eq!(b("A customer of the bank, with", 11.48).1, 144.140625);
        assert_eq!(b("A customer of the bank, with", 16.0).1, 201.03125);
        assert_eq!(b("A customer of the bank, with", 10.5).2, 131.953125);
        assert_eq!(b("[Software System]", 10.5).1, 85.283447265625);
        assert_eq!(b("Banking System G Queue", 16.0).1, 185.1015625);
        // One run: no per-tspan rounding, the kerned advance.
        assert_eq!(b("[Person]", 11.48).1, 42.71875);
        assert_eq!(shaping_size(11.48), 11.46875);
        assert_eq!(shaping_size(14.0), 14.0);
    }

    #[test]
    fn shaping_size_follows_blinks_effective_font_size() {
        // 64× the size Chromium's advances scale with (getComputedTextLength of
        // a long string against its length at 2048px), each in a fresh page.
        for (size, fdot6) in [
            (9.6, 614),
            (9.9, 632),
            (10.0, 640),
            (11.48, 734),
            (12.6, 806),
            (14.4, 921),
            (15.9, 1017),
            (16.22, 1037),
            (16.8, 1074),
            (17.3, 1106),
            (40.1, 2565),
            (8.15625, 521),
        ] {
            assert_eq!(shaping_size(size) * 64.0, f64::from(fdot6), "{size}px");
        }
    }

    #[test]
    fn run_marks_split_tspans_but_not_measurement() {
        let l = word_runs("A customer of");
        assert_eq!(plain(&l), "A customer of");
        assert_eq!(
            line_width(&l, 16.0, false),
            line_width("A customer of", 16.0, false)
        );
        let runs: Vec<String> = styled_runs(&l, false).into_iter().map(|r| r.text).collect();
        assert_eq!(runs, ["A", " customer", " of"]);
    }
}
