//! The bundled fonts: the web fonts mmdc loads and TeX Gyre Heros.
//!
//! Layout never reads these files: text is measured with tables generated from
//! the same fonts ([`crate::text`]). They draw the glyphs, so PNG and PDF output
//! looks like mmdc's and works on a machine with no fonts installed:
//!
//! * Recursive Variable (Sans Linear at `wght` 400 and 700, and the slanted
//!   outlines Chromium synthesises for its italic) and Open Sans (400, 400
//!   italic, and the emboldened outlines Chromium synthesises for their bold),
//!   SIL Open Font License 1.1, built by `tools/gen_bundled_fonts.py`
//!   from the fontsource packages mmdc links ([`WEB_FILES`]). The rasteriser
//!   finds them by family name before any system font; the PDF writer embeds
//!   them for text in those families.
//! * TeX Gyre Heros (GUST Font License; see `LICENSES/`), URW's Nimbus Sans (a
//!   Helvetica design) extended by GUST, whose advance widths are Helvetica's,
//!   close to Arial's: the PNG fallback when no font matches a label's
//!   `font-family`, and the PDF face for text outside Windows-1252, which the
//!   standard Helvetica fonts cannot show.
//!
//! Faces are indexed 0..4 for TeX Gyre Heros, `bold as usize | (italic as
//! usize) << 1`, then [`WEB_FILES`]'s order from 4 ([`web_face_index`]).

/// The four faces' OpenType (CFF) files: regular, bold, italic, bold italic.
#[cfg(feature = "fonts")]
pub const FILES: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/texgyreheros-regular.otf"),
    include_bytes!("../assets/fonts/texgyreheros-bold.otf"),
    include_bytes!("../assets/fonts/texgyreheros-italic.otf"),
    include_bytes!("../assets/fonts/texgyreheros-bolditalic.otf"),
];

/// The web fonts' TrueType files: Recursive Variable 400, 700, 400 italic, 700
/// italic (slanted); Open Sans 400, 700 (emboldened), 400 italic, 700 italic
/// (emboldened).
#[cfg(feature = "fonts")]
pub const WEB_FILES: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/recursive-400.ttf"),
    include_bytes!("../assets/fonts/recursive-700.ttf"),
    include_bytes!("../assets/fonts/recursive-400-italic.ttf"),
    include_bytes!("../assets/fonts/recursive-700-italic.ttf"),
    include_bytes!("../assets/fonts/open-sans-400.ttf"),
    include_bytes!("../assets/fonts/open-sans-700.ttf"),
    include_bytes!("../assets/fonts/open-sans-400-italic.ttf"),
    include_bytes!("../assets/fonts/open-sans-700-italic.ttf"),
];

/// The family name the TeX Gyre faces register under.
pub const FAMILY: &str = "TeX Gyre Heros";

/// Number of bundled faces ([`face`] indexes 0 up to this).
pub const FACE_COUNT: usize = 12;

/// The faces' PostScript names, by face index.
pub const POSTSCRIPT_NAMES: [&str; FACE_COUNT] = [
    "TeXGyreHeros-Regular",
    "TeXGyreHeros-Bold",
    "TeXGyreHeros-Italic",
    "TeXGyreHeros-BoldItalic",
    "RecursiveVariable-Regular",
    "RecursiveVariable-Bold",
    "RecursiveVariable-Italic",
    "RecursiveVariable-BoldItalic",
    "OpenSans-Regular",
    "OpenSans-Bold",
    "OpenSans-Italic",
    "OpenSans-BoldItalic",
];

/// The bundled face that draws text measured in `face`, if it is a web font,
/// by weight and slant.
pub fn web_face_index(face: crate::text::Face, bold: bool, italic: bool) -> Option<usize> {
    let style = usize::from(bold) + 2 * usize::from(italic);
    match face {
        crate::text::Face::Arial => None,
        crate::text::Face::Recursive => Some(4 + style),
        crate::text::Face::OpenSans => Some(8 + style),
    }
}

/// The face index for a style.
pub fn face_index(bold: bool, italic: bool) -> usize {
    usize::from(bold) | usize::from(italic) << 1
}

/// What the PDF writer needs from an OpenType font: glyph lookup, advances and the
/// font descriptor's metrics, read from the `cmap`, `hmtx`, `hhea`, `head`, `OS/2`
/// and `post` tables.
#[derive(Debug, Clone)]
pub struct Face {
    pub data: &'static [u8],
    /// TrueType outlines (`glyf`), embedded as `FontFile2`; else CFF.
    pub truetype: bool,
    pub units_per_em: u16,
    pub bbox: [i16; 4],
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    pub italic_angle: f64,
    cmap: &'static [u8],
    hmtx: &'static [u8],
    long_metrics: u16,
}

impl Face {
    /// Parse the tables of an OpenType file; `None` if it is malformed.
    pub fn parse(data: &'static [u8]) -> Option<Face> {
        let num_tables = be16(data, 4)?;
        let table = |tag: &[u8; 4]| -> Option<&'static [u8]> {
            (0..num_tables as usize).find_map(|i| {
                let rec = 12 + 16 * i;
                (data.get(rec..rec + 4)? == tag).then(|| {
                    let off = be32(data, rec + 8)? as usize;
                    let len = be32(data, rec + 12)? as usize;
                    data.get(off..off + len)
                })?
            })
        };
        let head = table(b"head")?;
        let hhea = table(b"hhea")?;
        let os2 = table(b"OS/2")?;
        let post = table(b"post")?;
        let cmap_table = table(b"cmap")?;
        let hmtx = table(b"hmtx")?;
        // The Windows Unicode BMP subtable (format 4).
        let cmap = (0..be16(cmap_table, 2)? as usize).find_map(|i| {
            let rec = 4 + 8 * i;
            let (platform, encoding) = (be16(cmap_table, rec)?, be16(cmap_table, rec + 2)?);
            let off = be32(cmap_table, rec + 4)? as usize;
            let sub = cmap_table.get(off..)?;
            (platform == 3 && encoding == 1 && be16(sub, 0)? == 4).then_some(sub)
        })?;
        let fixed = be32(post, 4)? as i32;
        Some(Face {
            data,
            truetype: table(b"glyf").is_some(),
            units_per_em: be16(head, 18)?,
            bbox: [
                be16(head, 36)? as i16,
                be16(head, 38)? as i16,
                be16(head, 40)? as i16,
                be16(head, 42)? as i16,
            ],
            ascent: be16(hhea, 4)? as i16,
            descent: be16(hhea, 6)? as i16,
            cap_height: if be16(os2, 0)? >= 2 {
                be16(os2, 88)? as i16
            } else {
                be16(hhea, 4)? as i16
            },
            italic_angle: f64::from(fixed) / 65536.0,
            cmap,
            hmtx,
            long_metrics: be16(hhea, 34)?,
        })
    }

    /// The glyph for `c`, if the font has one (cmap format 4).
    pub fn glyph(&self, c: char) -> Option<u16> {
        let c = u16::try_from(c as u32).ok()?;
        let t = self.cmap;
        let segs = be16(t, 6)? as usize / 2;
        let ends = 14;
        let starts = ends + 2 * segs + 2;
        let deltas = starts + 2 * segs;
        let ranges = deltas + 2 * segs;
        // Segments are sorted by end code.
        let (mut lo, mut hi) = (0, segs);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if be16(t, ends + 2 * mid)? < c {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let i = lo;
        let start = be16(t, starts + 2 * i)?;
        if i >= segs || c < start {
            return None;
        }
        let delta = be16(t, deltas + 2 * i)?;
        let range = be16(t, ranges + 2 * i)? as usize;
        let g = if range == 0 {
            c.wrapping_add(delta)
        } else {
            let at = ranges + 2 * i + range + 2 * (c - start) as usize;
            match be16(t, at)? {
                0 => 0,
                g => g.wrapping_add(delta),
            }
        };
        (g != 0).then_some(g)
    }

    /// Advance width of a glyph in font units.
    pub fn advance(&self, glyph: u16) -> u16 {
        let i = glyph.min(self.long_metrics.saturating_sub(1)) as usize;
        be16(self.hmtx, 4 * i).unwrap_or(0)
    }
}

/// The glyph a bundled face draws the `liga` ligature `text` with (HarfBuzz's,
/// as Chromium shapes it): Open Sans' ff, fi, fl, ffi and ffl, whose upright
/// files (and the emboldened ones made from them) keep them at glyphs 236 to
/// 240 and whose italic ones at 238 to 242. `None` for any other face or text.
pub fn ligature_glyph(face: usize, text: &str) -> Option<u16> {
    const LIGATURES: [&str; 5] = ["ff", "fi", "fl", "ffi", "ffl"];
    let first = match face {
        8 | 9 => 236,
        10 | 11 => 238,
        _ => return None,
    };
    let k = LIGATURES.iter().position(|&l| l == text)?;
    Some(first + k as u16)
}

/// A bundled face by index, parsed once.
#[cfg(feature = "fonts")]
pub fn face(index: usize) -> Option<&'static Face> {
    use std::sync::OnceLock;
    static FACES: OnceLock<Vec<Option<Face>>> = OnceLock::new();
    FACES
        .get_or_init(|| {
            FILES
                .iter()
                .chain(&WEB_FILES)
                .map(|d| Face::parse(d))
                .collect()
        })
        .get(index)?
        .as_ref()
}

/// Without the `fonts` feature nothing is bundled.
#[cfg(not(feature = "fonts"))]
pub fn face(_index: usize) -> Option<&'static Face> {
    None
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

#[cfg(all(test, feature = "fonts"))]
mod tests {
    use super::*;

    #[test]
    fn faces_parse_with_helvetica_metrics() {
        for i in 0..4 {
            let f = face(i).expect("bundled face parses");
            assert_eq!(f.units_per_em, 1000);
            // Helvetica's advances: space 278, `A` 667 (722 bold), `i` 222 (278 bold).
            let w = |c| f.advance(f.glyph(c).unwrap());
            assert_eq!(w(' '), 278);
            assert_eq!(w('A'), if i & 1 == 1 { 722 } else { 667 });
            assert_eq!(w('i'), if i & 1 == 1 { 278 } else { 222 });
            // Beyond Windows-1252: Latin Extended, Vietnamese, basic Greek, arrows.
            for c in ['Ł', 'ő', 'ế', 'ơ', 'Ω', 'μ', '→', '≤'] {
                assert!(f.glyph(c).is_some(), "{c} in face {i}");
            }
            // No Cyrillic or CJK.
            assert!(f.glyph('Ж').is_none() && f.glyph('中').is_none());
            assert_eq!(f.italic_angle != 0.0, i & 2 == 2);
        }
    }

    #[test]
    fn web_faces_carry_the_measured_advances() {
        use crate::text::Face as F;
        for (face, bold, italic) in [
            (F::Recursive, false, false),
            (F::Recursive, true, false),
            (F::Recursive, false, true),
            (F::Recursive, true, true),
            (F::OpenSans, false, false),
            (F::OpenSans, true, false),
            (F::OpenSans, false, true),
            (F::OpenSans, true, true),
        ] {
            let f = super::face(web_face_index(face, bold, italic).unwrap()).unwrap();
            assert!(f.truetype);
            let data = face.data(italic).unwrap();
            assert_eq!(f64::from(f.units_per_em), data.upem);
            // Every character the tables measure is drawn with that advance
            // (the emboldened Open Sans keeps the regular advances).
            for c in ['a', 'W', 'é', 'ế', '1', ' '] {
                let g = f.glyph(c).unwrap_or_else(|| panic!("{c} in {face:?}"));
                let adv = f64::from(f.advance(g)) / data.upem;
                assert_eq!(
                    Some(adv),
                    data.advance_em(c),
                    "{c} in {face:?} {bold} {italic}"
                );
            }
        }
    }

    /// The ligature glyphs [`ligature_glyph`] names have the advances the
    /// text tables measure the ligatures by.
    #[test]
    fn ligature_glyphs_carry_the_measured_advances() {
        use crate::text::Face as F;
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            let index = web_face_index(F::OpenSans, bold, italic).unwrap();
            let f = super::face(index).unwrap();
            let data = F::OpenSans.data(italic).unwrap();
            for lig in ["ff", "fi", "fl", "ffi", "ffl"] {
                let chars: Vec<char> = lig.chars().collect();
                let (n, adv) = data.ligature_at(&chars).unwrap();
                assert_eq!(n, chars.len());
                let g = ligature_glyph(index, lig).unwrap();
                assert_eq!(
                    f64::from(f.advance(g)) / data.upem,
                    adv,
                    "{lig} {bold} {italic}"
                );
            }
        }
        assert_eq!(
            ligature_glyph(web_face_index(F::Recursive, false, false).unwrap(), "fi"),
            None
        );
    }
}
