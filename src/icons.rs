//! Font Awesome icons in labels (`fa:fa-car`, `fab:fa-github`, ...).
//!
//! mermaid's `createText` turns every `(fa[bklrs]?):fa-([\w-]+)` in an HTML label
//! into `<i class='fa fa-car'></i>`, and mmdc loads Font Awesome Free 7's CSS, so
//! the browser draws the icon glyph inline: an inline-block 1.25em wide (FA 7's
//! fixed width) with the glyph centred in it on the text baseline, at the label's
//! font size and colour. A class Font Awesome does not define leaves the box empty;
//! a known class whose glyph is missing from the style's font (`fa:fa-github`,
//! a brand in the solid font) shows the font's `.notdef` box; `fal` / `fak` (Pro
//! styles, no CSS) give an empty, zero-width element.
//!
//! An icon travels through a label as one private-use character (plane 15, which
//! nothing else in a label produces): [`char_width_em`](crate::text::char_width_em)
//! gives it its 1.25em advance, and the SVG and PDF backends draw its outline
//! ([`outline`]) where it falls on the line ([`line_pieces`]).
//!
//! The outlines and class names come from the Font Awesome Free npm package
//! (icons CC BY 4.0, Fonticons, Inc.; see `LICENSES/`), packed by
//! `tools/gen_fa_icons.py` into `assets/fontawesome/icons.bin`. Without the `icons`
//! feature no data is compiled in and every reference becomes a `•` placeholder.

use crate::geom::PathCmd;

/// Advance of every icon in em: Font Awesome 7's `.fa { width: 1.25em }`.
pub const ADVANCE_EM: f64 = 1.25;
/// Font Awesome's em square and ascent, in font units (y down from the em top).
const UNITS_PER_EM: f64 = 512.0;
const ASCENT: f64 = 448.0;

/// First character of the icon range (Supplementary Private Use Area-A).
const BASE: u32 = 0xF0000;
/// `.notdef` of a style: `NOTDEF + style`.
#[cfg(feature = "icons")]
const NOTDEF: u32 = 0xFF000;
/// An empty box (a class Font Awesome does not define).
const EMPTY: u32 = 0xFF010;

/// Whether `c` stands for an icon.
pub fn is_icon(c: char) -> bool {
    (BASE..=EMPTY).contains(&(c as u32))
}

/// Whether a display line contains icons.
pub fn has_icons(s: &str) -> bool {
    s.chars().any(is_icon)
}

/// A piece of a display line: text (with its emphasis marks balanced, so each
/// piece can be drawn alone) or one icon, with its offset from the line start.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text { x: f64, text: String },
    Icon { x: f64, icon: char },
}

/// Split a display line into text runs and icons, each at its offset (in px) from
/// the start of the line as [`crate::text::line_width`] measures it.
pub fn line_pieces(line: &str, font_size: f64, bold: bool) -> Vec<Piece> {
    let mut texts: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in line.char_indices() {
        if is_icon(c) {
            if i > start {
                texts.push((start, i));
            }
            out.push((i, Some(c)));
            start = i + c.len_utf8();
        }
    }
    if start < line.len() {
        texts.push((start, line.len()));
    }
    let balanced =
        crate::text::balance_marks(texts.iter().map(|&(a, b)| line[a..b].to_string()).collect());
    out.extend(texts.iter().map(|&(a, _)| (a, None)));
    out.sort_by_key(|&(i, _)| i);
    let mut balanced = balanced.into_iter();
    out.into_iter()
        .map(|(i, icon)| {
            let x = crate::text::line_width(&line[..i], font_size, bold);
            match icon {
                Some(icon) => Piece::Icon { x, icon },
                None => Piece::Text {
                    x,
                    text: balanced.next().unwrap_or_default(),
                },
            }
        })
        .collect()
}

/// Where an icon's outline goes: `(tx, ty, scale)` maps outline units to the page
/// (`page = t + unit * scale`) for an icon box starting at `x` on `baseline`.
pub fn placement(icon: char, x: f64, baseline: f64, font_size: f64) -> (f64, f64, f64) {
    let s = font_size / UNITS_PER_EM;
    let adv = glyph(icon).map_or(0.0, |(adv, _)| adv);
    // `text-align: center` in the 1.25em box.
    let left = (ADVANCE_EM * UNITS_PER_EM - adv) / 2.0;
    (x + left * s, baseline - ASCENT * s, s)
}

/// The icon's outline in font units (y down from the em top), or nothing for an
/// empty box.
pub fn outline(icon: char) -> Vec<PathCmd> {
    glyph(icon).map(|(_, p)| p).unwrap_or_default()
}

/// Replace every Font Awesome reference in an HTML label with its icon character
/// (mermaid's `replaceIconSubstring`; the regex has no word boundary, so `sofa:fa-x`
/// matches from its `fa`).
pub fn replace_references(label: &str) -> String {
    if !label.contains(":fa-") {
        return label.to_string();
    }
    let mut out = String::with_capacity(label.len());
    let mut rest = label;
    while let Some(i) = rest.find("fa") {
        let (before, at) = rest.split_at(i);
        out.push_str(before);
        let style = match at.as_bytes().get(2) {
            Some(&b) if b"bklrs".contains(&b) && at.as_bytes().get(3) == Some(&b':') => Some(b),
            _ => None,
        };
        let prefix_len = if style.is_some() { 3 } else { 2 };
        let tail = &at[prefix_len..];
        let name_len = tail
            .strip_prefix(":fa-")
            .map(|t| {
                t.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(t.len())
            })
            .unwrap_or(0);
        if name_len > 0 {
            let name = &tail[4..4 + name_len];
            if let Some(c) = icon_char(style, name) {
                out.push(c);
            }
            rest = &tail[4 + name_len..];
        } else {
            out.push_str(&at[..2]);
            rest = &at[2..];
        }
    }
    out.push_str(rest);
    out
}

/// The character for `fa{style}:fa-{name}`; `None` draws nothing at all.
#[cfg(feature = "icons")]
fn icon_char(style: Option<u8>, name: &str) -> Option<char> {
    let style = match style {
        None | Some(b's') => 0,
        Some(b'r') => 1,
        Some(b'b') => 2,
        // `fal` / `fak`: Pro styles, no CSS gives the element a box or a glyph.
        _ => return None,
    };
    let d = data::get();
    let code = match d.classes.binary_search_by(|(n, _)| (*n).cmp(name)) {
        Err(_) => EMPTY,
        Ok(k) => {
            let cp = d.classes[k].1;
            match d.styles[style].binary_search_by_key(&cp, |g| g.cp) {
                Ok(i) => BASE + d.style_base[style] + i as u32,
                Err(_) => NOTDEF + style as u32,
            }
        }
    };
    char::from_u32(code)
}

#[cfg(not(feature = "icons"))]
fn icon_char(style: Option<u8>, _name: &str) -> Option<char> {
    match style {
        Some(b'l' | b'k') => None,
        _ => Some('\u{2022}'),
    }
}

/// Advance (font units) and outline of an icon character.
#[cfg(feature = "icons")]
fn glyph(icon: char) -> Option<(f64, Vec<PathCmd>)> {
    let code = icon as u32;
    let d = data::get();
    let g = if (NOTDEF..NOTDEF + 3).contains(&code) {
        // The style's font's `.notdef` box.
        &d.notdef[(code - NOTDEF) as usize]
    } else {
        let mut i = code.checked_sub(BASE)? as usize;
        let mut found = None;
        for glyphs in &d.styles {
            if let Some(g) = glyphs.get(i) {
                found = Some(g);
                break;
            }
            i -= glyphs.len();
        }
        found?
    };
    Some((f64::from(g.adv), data::decode(&d.paths[g.off as usize..])))
}

#[cfg(not(feature = "icons"))]
fn glyph(_icon: char) -> Option<(f64, Vec<PathCmd>)> {
    None
}

#[cfg(feature = "icons")]
mod data {
    use crate::geom::{PathCmd, Point};
    use std::sync::OnceLock;

    static BYTES: &[u8] = include_bytes!("../assets/fontawesome/icons.bin");

    pub(super) struct Glyph {
        pub cp: u32,
        pub adv: u16,
        pub off: u32,
    }

    pub(super) struct Data {
        /// `fa-<name>` class (without `fa-`) to codepoint, sorted by name.
        pub classes: Vec<(&'static str, u32)>,
        /// Solid, regular and brands glyphs, each sorted by codepoint.
        pub styles: [Vec<Glyph>; 3],
        /// Index of each style's first glyph in the icon range.
        pub style_base: [u32; 3],
        /// Each style's `.notdef` glyph (`cp` unused).
        pub notdef: [Glyph; 3],
        pub paths: &'static [u8],
    }

    struct Reader {
        at: usize,
    }

    impl Reader {
        fn bytes(&mut self, n: usize) -> &'static [u8] {
            let b = &BYTES[self.at..self.at + n];
            self.at += n;
            b
        }
        fn u8(&mut self) -> u8 {
            self.bytes(1)[0]
        }
        fn u16(&mut self) -> u16 {
            let b = self.bytes(2);
            u16::from_le_bytes([b[0], b[1]])
        }
        fn u32(&mut self) -> u32 {
            let b = self.bytes(4);
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
        fn str(&mut self) -> &'static str {
            let n = self.u8() as usize;
            std::str::from_utf8(self.bytes(n)).unwrap_or("")
        }
    }

    pub(super) fn get() -> &'static Data {
        static DATA: OnceLock<Data> = OnceLock::new();
        DATA.get_or_init(|| {
            let mut r = Reader { at: 0 };
            assert_eq!(r.bytes(4), b"FAI1", "bad icon data");
            let _version = r.str();
            let n = r.u32() as usize;
            let classes = (0..n)
                .map(|_| {
                    let name = r.str();
                    (name, r.u32())
                })
                .collect();
            let mut style = || {
                let m = r.u32() as usize;
                (0..m)
                    .map(|_| Glyph {
                        cp: r.u32(),
                        adv: r.u16(),
                        off: r.u32(),
                    })
                    .collect::<Vec<_>>()
            };
            let styles = [style(), style(), style()];
            let mut notdef = || Glyph {
                cp: 0,
                adv: r.u16(),
                off: r.u32(),
            };
            let notdef = [notdef(), notdef(), notdef()];
            let len = r.u32() as usize;
            let paths = r.bytes(len);
            let style_base = [
                0,
                styles[0].len() as u32,
                (styles[0].len() + styles[1].len()) as u32,
            ];
            Data {
                classes,
                styles,
                style_base,
                notdef,
                paths,
            }
        })
    }

    /// Decode one path (see `tools/gen_fa_icons.py`) to absolute commands in font
    /// units, arcs as cubics.
    pub(super) fn decode(mut b: &[u8]) -> Vec<PathCmd> {
        fn varint(b: &mut &[u8]) -> u64 {
            let mut v = 0u64;
            let mut shift = 0;
            while let Some((&x, rest)) = b.split_first() {
                *b = rest;
                v |= u64::from(x & 0x7F) << shift;
                shift += 7;
                if x & 0x80 == 0 {
                    break;
                }
            }
            v
        }
        // Coordinates add up in whole tenths of a unit, so they stay exact.
        fn tenths(b: &mut &[u8]) -> i64 {
            let v = varint(b);
            (v >> 1) as i64 ^ -((v & 1) as i64)
        }
        let pt = |(x, y): (i64, i64)| Point::new(x as f64 / 10.0, y as f64 / 10.0);
        let mut out = Vec::new();
        let mut cur = (0i64, 0i64);
        let mut start = cur;
        let next = |b: &mut &[u8], cur: (i64, i64)| (cur.0 + tenths(b), cur.1 + tenths(b));
        while let Some((&op, rest)) = b.split_first() {
            b = rest;
            match op {
                1 => {
                    cur = next(&mut b, cur);
                    start = cur;
                    out.push(PathCmd::MoveTo(pt(cur)));
                }
                2 => {
                    cur = next(&mut b, cur);
                    out.push(PathCmd::LineTo(pt(cur)));
                }
                3 => {
                    let (c1, c2, to) = (next(&mut b, cur), next(&mut b, cur), next(&mut b, cur));
                    out.push(PathCmd::CubicTo(pt(c1), pt(c2), pt(to)));
                    cur = to;
                }
                4 => {
                    let (rx, ry, phi) = (tenths(&mut b), tenths(&mut b), tenths(&mut b));
                    let flags = varint(&mut b);
                    let to = next(&mut b, cur);
                    let arc = crate::diagram::shapes::Arc {
                        rx: rx as f64 / 10.0,
                        ry: ry as f64 / 10.0,
                        phi: phi as f64 / 10.0,
                        large: flags & 2 != 0,
                        sweep: flags & 1 != 0,
                    };
                    crate::diagram::shapes::svg_arc(&mut out, pt(cur), arc, pt(to));
                    cur = to;
                }
                5 => {
                    out.push(PathCmd::Close);
                    cur = start;
                }
                _ => break,
            }
        }
        out
    }
}

#[cfg(all(test, feature = "icons"))]
mod tests {
    use super::*;

    #[test]
    fn classes_resolve_per_style() {
        let car = replace_references("fa:fa-car");
        assert_eq!(car.chars().count(), 1);
        assert!(is_icon(car.chars().next().unwrap()));
        // An alias is the same glyph; `fas` is `fa`.
        assert_eq!(replace_references("fa:fa-automobile"), car);
        assert_eq!(replace_references("fas:fa-car"), car);
        // The regular font has no car: its `.notdef`. Brands have GitHub.
        assert_eq!(replace_references("far:fa-car"), "\u{FF001}");
        assert_eq!(replace_references("fa:fa-github"), "\u{FF000}");
        assert!(glyph(replace_references("fab:fa-github").chars().next().unwrap()).is_some());
        // Unknown classes leave an empty box; Pro styles nothing at all.
        assert_eq!(replace_references("fa:fa-nonexistent x"), "\u{FF010} x");
        assert_eq!(replace_references("fal:fa-car light"), " light");
        assert_eq!(replace_references("fak:fa-car"), "");
    }

    #[test]
    fn references_are_found_like_the_regex() {
        let car = replace_references("fa:fa-car");
        assert_eq!(replace_references("a fa:fa-car b"), format!("a {car} b"));
        assert_eq!(replace_references("sofa:fa-car"), format!("so{car}"));
        assert_eq!(
            replace_references("fake fa: fa-x fas:fb faz:fa-car"),
            "fake fa: fa-x fas:fb faz:fa-car"
        );
        assert_eq!(replace_references("fa:fa-car!"), format!("{car}!"));
    }

    #[test]
    fn outlines_decode_inside_the_em() {
        let car = replace_references("fa:fa-car").chars().next().unwrap();
        let (adv, path) = glyph(car).unwrap();
        assert_eq!(adv, 512.0);
        assert!(
            matches!(path[0], PathCmd::MoveTo(p) if (p.x - 135.2).abs() < 1e-9 && (p.y - 117.4).abs() < 1e-9)
        );
        for c in &path {
            if let PathCmd::LineTo(p) | PathCmd::MoveTo(p) = c {
                assert!(
                    (0.0..=512.0).contains(&p.x) && (0.0..=512.0).contains(&p.y),
                    "{p:?}"
                );
            }
        }
        // A 512-wide glyph is centred in the 640-unit box.
        let (tx, ty, s) = placement(car, 10.0, 100.0, 16.0);
        assert!((s - 16.0 / 512.0).abs() < 1e-12);
        assert!((tx - (10.0 + 64.0 * s)).abs() < 1e-9);
        assert!((ty - (100.0 - 14.0)).abs() < 1e-9);
    }

    #[test]
    fn every_glyph_decodes() {
        let d = data::get();
        for g in d.styles.iter().flatten() {
            let p = data::decode(&d.paths[g.off as usize..]);
            assert!(matches!(p.first(), Some(PathCmd::MoveTo(_))));
            assert!(g.adv > 0 && g.adv <= 640);
        }
    }

    #[test]
    fn pieces_carry_offsets_and_emphasis() {
        let car = replace_references("fa:fa-car");
        let line = format!("\u{E000}a{car}b\u{E001}");
        let p = line_pieces(&line, 10.0, false);
        assert_eq!(p.len(), 3);
        assert!(
            matches!(&p[0], Piece::Text { x, text } if *x == 0.0 && text == "\u{E000}a\u{E001}")
        );
        assert!(matches!(&p[2], Piece::Text { text, .. } if text == "\u{E000}b\u{E001}"));
        if let Piece::Icon { x, .. } = p[1] {
            assert!((x - crate::text::line_width("\u{E000}a", 10.0, false)).abs() < 1e-9);
        }
    }
}
