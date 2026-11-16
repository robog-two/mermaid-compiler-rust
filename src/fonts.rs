//! The bundled TeX Gyre Heros fonts (GUST Font License; see `LICENSES/`).
//!
//! Layout never reads these: text is measured with Arial's metrics
//! ([`crate::text`]). They only draw glyphs where nothing else can, so PNG and PDF
//! output works on a machine with no fonts installed:
//!
//! * the PNG rasteriser falls back to them when no system font matches a label's
//!   `font-family` (`output::png`);
//! * the PDF writer embeds a face for text outside Windows-1252, which the
//!   standard Helvetica fonts cannot show (`output::pdf`).
//!
//! TeX Gyre Heros is URW's Nimbus Sans (a Helvetica design) extended by GUST: its
//! advance widths are Helvetica's, close to the Arial metrics the layout uses.
//! Faces are indexed `bold as usize | (italic as usize) << 1`.

/// The four faces' OpenType (CFF) files: regular, bold, italic, bold italic.
#[cfg(feature = "fonts")]
pub const FILES: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/texgyreheros-regular.otf"),
    include_bytes!("../assets/fonts/texgyreheros-bold.otf"),
    include_bytes!("../assets/fonts/texgyreheros-italic.otf"),
    include_bytes!("../assets/fonts/texgyreheros-bolditalic.otf"),
];

/// The family name the faces register under.
pub const FAMILY: &str = "TeX Gyre Heros";

/// The faces' PostScript names.
pub const POSTSCRIPT_NAMES: [&str; 4] = [
    "TeXGyreHeros-Regular",
    "TeXGyreHeros-Bold",
    "TeXGyreHeros-Italic",
    "TeXGyreHeros-BoldItalic",
];

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

/// The bundled face for a style, parsed once.
#[cfg(feature = "fonts")]
pub fn face(index: usize) -> Option<&'static Face> {
    use std::sync::OnceLock;
    static FACES: OnceLock<[Option<Face>; 4]> = OnceLock::new();
    FACES
        .get_or_init(|| FILES.map(Face::parse))
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
}
