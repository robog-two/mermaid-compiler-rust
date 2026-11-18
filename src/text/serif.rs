//! Text measured in the browser's default serif font.
//!
//! Some of mermaid's measurements (`utils.calculateTextDimensions` with a font family
//! the page does not apply, as the sequence diagram's are) fall back to the page's
//! default font, "Times New Roman", which the container renders with Liberation
//! Serif (metric-compatible). These are its metrics, checked against headless
//! Chromium's `getBBox()` for 1500 random ASCII strings at 12 to 20px.

/// Width of an SVG `<text>`'s `getBBox()` in the default serif font: the kerned
/// advance (rounded up to 1/64 px) united with each glyph's ink box rounded out to
/// whole pixels from the glyph's origin, as [`super::svg_bbox_width`] does for
/// Arial. Characters outside ASCII count by Arial's advance alone.
pub fn bbox_width(text: &str, font_size: f64) -> f64 {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let em = |u: f64| u / 2048.0 * font_size;
    let mut x = 0.0f64;
    let (mut left, mut right) = (0.0f64, 0.0f64);
    let mut prev: Option<char> = None;
    for c in text.chars().filter(|&c| !super::mark::is_mark(c)) {
        if let Some(p) = prev {
            x += em(kerning(p, c));
        }
        match glyph(c) {
            Some((adv, lo, hi, _)) => {
                if c != ' ' {
                    left = left.min(x + (em(f64::from(lo)) + 0.005).floor());
                    right = right.max(x + (em(f64::from(hi)) - 0.005).ceil());
                }
                x += em(f64::from(adv));
            }
            None => x += super::arial_char_width_em(c, false) * font_size,
        }
        prev = Some(c);
    }
    right.max(super::layout_unit_ceil(x)) - left
}

/// Height of one line's `getBBox()` in the default serif font: the ascent
/// (1825/2048 em) and descent (443/2048 em) each rounded to whole pixels, the
/// descent rounded up instead to reach the lowest glyph's ink.
pub fn line_box(font_size: f64, line: &str) -> f64 {
    let ascent = (1825.0 / 2048.0 * font_size).round();
    let descent = line
        .chars()
        .filter_map(glyph)
        .map(|(_, _, _, y_min)| (-f64::from(y_min) / 2048.0 * font_size).ceil())
        .fold((443.0 / 2048.0 * font_size).round(), f64::max);
    ascent + descent
}

fn glyph(c: char) -> Option<(u16, i16, i16, i16)> {
    match c as u32 {
        code @ 0x20..=0x7E => Some(SERIF[(code - 0x20) as usize]),
        _ => None,
    }
}

fn kerning(a: char, b: char) -> f64 {
    if !a.is_ascii() || !b.is_ascii() {
        return 0.0;
    }
    SERIF_KERNING
        .iter()
        .find(|(p, _)| p.as_bytes() == [a as u8, b as u8])
        .map_or(0.0, |&(_, k)| f64::from(k))
}

/// Liberation Serif (the browser's default serif font, metric-compatible with Times New Roman)
/// for ASCII 0x20..=0x7E, in 1/2048 em: advance width, ink `xMin`, `xMax` and `yMin`
/// (`hmtx` and `glyf`).
const SERIF: [(u16, i16, i16, i16); 95] = [
    (512, 0, 0, 0),
    (682, 219, 461, -29),
    (836, 127, 709, 860),
    (1024, 37, 987, 0),
    (1024, 88, 961, -156),
    (1706, 70, 1636, -27),
    (1593, 78, 1556, -20),
    (369, 86, 283, 860),
    (682, 90, 616, -436),
    (682, 66, 592, -436),
    (1024, 100, 926, 590),
    (1155, 102, 1055, 203),
    (512, 78, 383, -315),
    (682, 76, 608, 406),
    (512, 135, 377, -29),
    (569, 0, 569, -20),
    (1024, 78, 946, -20),
    (1024, 180, 901, 0),
    (1024, 90, 911, 0),
    (1024, 98, 944, -20),
    (1024, 40, 992, 0),
    (1024, 119, 944, -20),
    (1024, 88, 963, -20),
    (1024, 135, 965, 0),
    (1024, 78, 946, -20),
    (1024, 66, 940, -20),
    (569, 162, 403, -29),
    (569, 106, 412, -315),
    (1155, 102, 1055, 186),
    (1155, 102, 1055, 424),
    (1155, 104, 1057, 186),
    (909, 106, 825, -29),
    (1886, 109, 1778, -389),
    (1479, 20, 1464, 0),
    (1366, 59, 1266, -6),
    (1366, 84, 1253, -20),
    (1479, 59, 1397, -4),
    (1251, 59, 1149, 0),
    (1139, 59, 1065, 0),
    (1479, 84, 1415, -20),
    (1479, 59, 1419, 0),
    (682, 74, 610, 0),
    (797, 43, 754, -20),
    (1479, 59, 1458, 0),
    (1251, 59, 1128, 0),
    (1821, 59, 1761, 0),
    (1479, 59, 1432, 0),
    (1479, 84, 1395, -20),
    (1139, 59, 1057, 0),
    (1479, 84, 1444, -332),
    (1366, 59, 1363, 0),
    (1139, 137, 1012, -20),
    (1251, 37, 1217, 0),
    (1479, 43, 1436, -20),
    (1479, 23, 1456, -31),
    (1933, 2, 1929, -31),
    (1479, 45, 1442, 0),
    (1479, 23, 1427, 0),
    (1251, 98, 1139, 0),
    (682, 152, 608, -274),
    (569, 0, 569, -20),
    (682, 74, 530, -274),
    (961, 29, 934, 389),
    (1024, -16, 1040, -264),
    (682, 119, 528, 1119),
    (909, 72, 881, -20),
    (1024, 0, 946, -20),
    (909, 78, 846, -20),
    (1024, 74, 999, -20),
    (909, 80, 838, -20),
    (682, 63, 682, 0),
    (1024, 88, 985, -442),
    (1024, 20, 997, 0),
    (569, 43, 530, 0),
    (569, -59, 393, -436),
    (1024, 39, 1024, 0),
    (569, 41, 528, 0),
    (1593, 43, 1561, 0),
    (1024, 47, 993, 0),
    (1024, 78, 946, -20),
    (1024, 33, 944, -436),
    (1024, 74, 987, -436),
    (682, 41, 664, 0),
    (797, 84, 723, -20),
    (569, 20, 557, -20),
    (1024, 27, 989, -20),
    (1024, 0, 1024, -20),
    (1479, 2, 1470, -20),
    (1024, 18, 999, 0),
    (1024, 25, 1016, -442),
    (909, 55, 852, 0),
    (983, 186, 797, -274),
    (410, 154, 256, -442),
    (983, 186, 797, -274),
    (1108, 57, 1049, 502),
];

/// Liberation Serif's kerning pairs (1/2048 em) as Chromium applies them, measured in
/// headless Chromium.
const SERIF_KERNING: &[(&str, i16)] = &[
    (" A", -113),
    (" T", -37),
    (" V", -37),
    (" W", -37),
    (" Y", -76),
    ("11", -76),
    ("A ", -113),
    ("AT", -227),
    ("AV", -264),
    ("AW", -164),
    ("AY", -188),
    ("Av", -152),
    ("Aw", -188),
    ("Ay", -188),
    ("F,", -164),
    ("F.", -164),
    ("FA", -152),
    ("L ", -76),
    ("LT", -188),
    ("LV", -188),
    ("LW", -152),
    ("LY", -205),
    ("Ly", -113),
    ("P ", -76),
    ("P,", -227),
    ("P.", -227),
    ("PA", -188),
    ("RT", -123),
    ("RV", -164),
    ("RW", -113),
    ("RY", -113),
    ("Ry", -82),
    ("T ", -37),
    ("T,", -152),
    ("T-", -188),
    ("T.", -152),
    ("T:", -102),
    ("T;", -113),
    ("TA", -164),
    ("TO", -37),
    ("Ta", -143),
    ("Tc", -143),
    ("Te", -143),
    ("Ti", -72),
    ("To", -143),
    ("Tr", -72),
    ("Ts", -143),
    ("Tu", -72),
    ("Tw", -143),
    ("Ty", -143),
    ("V ", -37),
    ("V,", -264),
    ("V-", -188),
    ("V.", -264),
    ("V:", -152),
    ("V;", -152),
    ("VA", -264),
    ("Va", -227),
    ("Ve", -227),
    ("Vi", -123),
    ("Vo", -264),
    ("Vr", -123),
    ("Vu", -123),
    ("Vy", -227),
    ("W ", -37),
    ("W,", -188),
    ("W-", -113),
    ("W.", -188),
    ("W:", -76),
    ("W;", -76),
    ("WA", -227),
    ("Wa", -164),
    ("We", -164),
    ("Wi", -82),
    ("Wo", -164),
    ("Wr", -82),
    ("Wu", -82),
    ("Wy", -123),
    ("Y ", -76),
    ("Y,", -264),
    ("Y-", -227),
    ("Y.", -264),
    ("Y:", -188),
    ("Y;", -188),
    ("YA", -227),
    ("Ya", -205),
    ("Ye", -205),
    ("Yi", -113),
    ("Yo", -205),
    ("Yp", -188),
    ("Yq", -227),
    ("Yu", -227),
    ("Yv", -205),
    ("ff", -37),
    ("r,", -82),
    ("r-", -41),
    ("r.", -113),
    ("rg", -37),
    ("v,", -133),
    ("v.", -133),
    ("w,", -133),
    ("w.", -133),
    ("y,", -133),
    ("y.", -133),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_heights_as_chromium_measures_them() {
        // getBBox() in headless Chromium, "Times New Roman" (Liberation Serif).
        assert_eq!(bbox_width("Hello John, how are you?", 16.0), 163.96875);
        assert_eq!(bbox_width("Great!", 16.0), 40.859375);
        assert_eq!(bbox_width("See you later!", 16.0), 88.859375);
        assert_eq!(bbox_width("John", 16.0), 30.234375);
        assert_eq!(bbox_width("Alice", 16.0), 34.65625);
        assert_eq!(line_box(16.0, "Hello John, how are you?"), 18.0);
        assert_eq!(line_box(16.0, "Great!"), 17.0);
        assert_eq!(line_box(14.0, "Typography"), 16.0);
        assert_eq!(line_box(14.0, "(paren)x"), 15.0);
    }
}
