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
/// mermaid's `<i class="fal fa-user">` for a Pro style (`fal`, `fak`), which no
/// CSS styles: an empty, zero-width element. It is followed by its class as tag
/// characters ([`Piece::Raw`]); the SVG output writes the element itself.
const RAW: u32 = 0xFF020;

/// First character of the registered-pack icon range (Supplementary Private Use
/// Area-B): `PACK_BASE + w` is an icon `w` hundredths of an em wide.
const PACK_BASE: u32 = 0x100000;
const PACK_LAST: u32 = 0x10FFFD;
/// Unicode tag characters (U+E0020..=U+E007E) spell an ASCII name without
/// taking any room: a pack icon's character is followed by its `prefix:name`.
const TAG_BASE: u32 = 0xE0000;

/// Whether `c` stands for an icon (Font Awesome's or a registered pack's).
pub fn is_icon(c: char) -> bool {
    (BASE..=RAW).contains(&(c as u32)) || is_pack_icon(c)
}

/// Whether `c` stands for an icon from a registered pack.
pub fn is_pack_icon(c: char) -> bool {
    (PACK_BASE..=PACK_LAST).contains(&(c as u32))
}

/// Whether `c` is a tag character (part of a pack icon's name).
pub fn is_tag(c: char) -> bool {
    (TAG_BASE + 0x20..=TAG_BASE + 0x7E).contains(&(c as u32))
}

/// Advance of an icon character in em.
pub fn advance_em(c: char) -> f64 {
    if is_pack_icon(c) {
        f64::from(c as u32 - PACK_BASE) / 100.0
    } else if c as u32 == RAW {
        0.0
    } else {
        ADVANCE_EM
    }
}

/// The character (and name) a registered pack's icon travels as: mermaid's
/// `getIconSVG(name, undefined, { class: "label-icon" })` gives it Iconify's
/// default size, `height="1em"` and a width of its aspect ratio rounded up to
/// 1/100 em (`calculateSize`).
fn pack_icon_text(name: &str, icon: &crate::iconify::IconData) -> Option<String> {
    let (w, h) = if icon.rotate.rem_euclid(2) == 1 {
        (icon.height, icon.width)
    } else {
        (icon.width, icon.height)
    };
    let ratio = if h > 0.0 { w / h } else { 1.0 };
    let hundredths = if ratio == 1.0 {
        100.0
    } else {
        (ratio * 100.0).ceil()
    };
    if !(1.0..=f64::from(PACK_LAST - PACK_BASE)).contains(&hundredths) || !name.is_ascii() {
        return None;
    }
    let mut out = String::new();
    out.push(char::from_u32(PACK_BASE + hundredths as u32)?);
    for b in name.bytes() {
        out.push(char::from_u32(TAG_BASE + u32::from(b))?);
    }
    Some(out)
}

/// `line` as an HTML label's text: `esc` escapes the plain runs, and each pack
/// icon is drawn inline as mermaid's `getIconSVG(name, undefined, { class:
/// "label-icon" })` gives it (`1em` high, as wide as its advance in em).
pub fn html_with_pack_icons(
    line: &str,
    packs: &crate::iconify::IconPacks,
    esc: impl Fn(&str) -> String,
) -> String {
    let mut out = String::new();
    let mut plain = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if is_tag(c) {
            continue;
        }
        if !is_pack_icon(c) {
            plain.push(c);
            continue;
        }
        let mut name = String::new();
        while let Some(&t) = chars.peek() {
            if !is_tag(t) {
                break;
            }
            name.push(char::from((t as u32 - TAG_BASE) as u8));
            chars.next();
        }
        let Some(icon) = packs.lookup(&name, None) else {
            continue;
        };
        out.push_str(&esc(&std::mem::take(&mut plain)));
        let w = advance_em(c);
        let svg = crate::iconify::icon_svg_sized(&icon, w, 1.0, &format!("label-icon-{name}"));
        let markup = svg
            .markup
            .replacen(
                &format!("width=\"{w}\" height=\"1\""),
                &format!("width=\"{w}em\" height=\"1em\""),
                1,
            )
            .replacen("\">", "\" class=\"label-icon\">", 1);
        out.push_str(&markup);
    }
    out.push_str(&esc(&plain));
    out
}

/// The pack icons in a display line: `(byte offset of the icon character, the
/// character, its `prefix:name`)`.
pub fn pack_icons(line: &str) -> Vec<(usize, char, String)> {
    let mut out: Vec<(usize, char, String)> = Vec::new();
    for (i, c) in line.char_indices() {
        if is_pack_icon(c) {
            out.push((i, c, String::new()));
        } else if is_tag(c) {
            if let Some(last) = out.last_mut() {
                last.2.push(char::from((c as u32 - TAG_BASE) as u8));
            }
        }
    }
    out
}

/// A display line without its tag characters.
pub fn strip_tags(line: &str) -> String {
    line.chars().filter(|&c| !is_tag(c)).collect()
}

/// Whether a display line contains icons.
pub fn has_icons(s: &str) -> bool {
    s.chars().any(is_icon)
}

/// A piece of a display line: text (with its emphasis marks balanced, so each
/// piece can be drawn alone) or one icon, with its offset from the line start.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text {
        x: f64,
        text: String,
    },
    Icon {
        x: f64,
        icon: char,
    },
    /// mermaid's `<i class="...">` with no glyph (a Pro style): its class.
    Raw {
        x: f64,
        class: String,
    },
}

/// Split a display line into text runs and icons, each at its offset (in px) from
/// the start of the line as [`crate::text::line_width`] measures it.
pub fn line_pieces(line: &str, font_size: f64, bold: bool) -> Vec<Piece> {
    let mut texts: Vec<(usize, usize)> = Vec::new();
    // Each icon with its class: a raw element's class is its tag characters.
    let mut out: Vec<(usize, Option<(char, String)>)> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while let Some(c) = line[i..].chars().next() {
        if !is_icon(c) {
            i += c.len_utf8();
            continue;
        }
        if i > start {
            texts.push((start, i));
        }
        let mut end = i + c.len_utf8();
        let mut class = String::new();
        if c as u32 == RAW {
            for t in line[end..].chars().take_while(|&t| is_tag(t)) {
                class.push(char::from((t as u32 - TAG_BASE) as u8));
                end += t.len_utf8();
            }
        }
        out.push((i, Some((c, class))));
        start = end;
        i = end;
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
                Some((icon, class)) if icon as u32 == RAW => Piece::Raw { x, class },
                Some((icon, _)) => Piece::Icon { x, icon },
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
    replace_references_with(label, &crate::iconify::IconPacks::default())
}

/// [`replace_references`] with icon packs registered: a reference whose
/// `prefix:name` a pack has (`isIconAvailable`; a pack named `fa`, `fab`, …)
/// becomes that pack's icon, drawn by [`place_pack_icons`]; any other is Font
/// Awesome's `<i>` element.
pub fn replace_references_with(label: &str, packs: &crate::iconify::IconPacks) -> String {
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
            let registered = format!("{}:{name}", &at[..prefix_len]);
            let pack = (!packs.is_empty())
                .then(|| packs.lookup(&registered, None))
                .flatten()
                .and_then(|icon| pack_icon_text(&registered, &icon));
            if let Some(p) = pack {
                out.push_str(&p);
            } else if matches!(style, Some(b'l' | b'k')) {
                // `<i class="fal fa-name">`: the class travels as tag characters.
                out.push(char::from_u32(RAW).unwrap_or('\u{FFFD}'));
                for b in format!("{} fa-{name}", &at[..prefix_len]).bytes() {
                    out.push(char::from_u32(TAG_BASE + u32::from(b)).unwrap_or('\u{FFFD}'));
                }
            } else if let Some(c) = icon_char(style, name) {
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

/// Which Font Awesome font draws the icon character `c`: `Some(true)` for the
/// brands font (`fab:`), `Some(false)` for the free one (`fa:`, `fas:`, `far:`,
/// and a class Font Awesome does not define, whose `<i>` still has the free
/// family), `None` when `c` is not a Font Awesome icon (a registered pack's icon
/// is an `<svg>`, not a glyph).
pub fn is_brand_icon(c: char) -> Option<bool> {
    let code = c as u32;
    if !(BASE..=EMPTY).contains(&code) {
        return None;
    }
    #[cfg(feature = "icons")]
    {
        if (NOTDEF..NOTDEF + 3).contains(&code) {
            return Some(code - NOTDEF == 2);
        }
        let d = data::get();
        Some(code != EMPTY && code - BASE >= d.style_base[2])
    }
    #[cfg(not(feature = "icons"))]
    Some(false)
}

/// Advance (font units) and outline of an icon character.
#[cfg(feature = "icons")]
fn glyph(icon: char) -> Option<(f64, Vec<PathCmd>)> {
    if is_pack_icon(icon) {
        // Drawn as its own `<svg>` ([`place_pack_icons`]).
        return None;
    }
    if icon as u32 == RAW {
        // An element with no glyph.
        return None;
    }
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

/// Draw the registered packs' icons in the scene's labels
/// ([`replace_references_with`]): each becomes the pack's `<svg>` after its
/// text (`.label-icon`: an inline-block `1em` high, its bottom `0.125em` below
/// the baseline, as wide as its character's advance), and the text keeps the
/// character as an empty box. In a node label `.node .label-icon path` fills
/// the icon's paths with the label colour. Elements without such icons are
/// returned as they are.
pub fn place_pack_icons(
    elements: Vec<crate::scene::Element>,
    packs: &crate::iconify::IconPacks,
) -> Vec<crate::scene::Element> {
    use crate::scene::Element;
    let mut out = Vec::with_capacity(elements.len());
    let mut n = 0usize;
    for el in elements {
        match el {
            Element::Group {
                id,
                class,
                children,
            } => out.push(Element::Group {
                id,
                class,
                children: place_pack_icons(children, packs),
            }),
            Element::Faded {
                opacity,
                class,
                children,
            } => out.push(Element::Faded {
                opacity,
                class,
                children: place_pack_icons(children, packs),
            }),
            Element::Rotated {
                deg,
                about,
                children,
            } => out.push(Element::Rotated {
                deg,
                about,
                children: place_pack_icons(children, packs),
            }),
            Element::Clipped { rect, children } => out.push(Element::Clipped {
                rect,
                children: place_pack_icons(children, packs),
            }),
            Element::Brightened {
                factor,
                class,
                children,
            } => out.push(Element::Brightened {
                factor,
                class,
                children: place_pack_icons(children, packs),
            }),
            Element::Translated { dx, dy, children } => out.push(Element::Translated {
                dx,
                dy,
                children: place_pack_icons(children, packs),
            }),
            Element::Text { text, class }
                if text.lines.iter().any(|l| l.chars().any(is_pack_icon)) =>
            {
                let node = class.as_deref() == Some("nodeLabel");
                let icons = label_icons(&text, packs, node, &mut n);
                let text = crate::scene::Text {
                    lines: text.lines.iter().map(|l| strip_tags(l)).collect(),
                    ..text
                };
                out.push(Element::Text { text, class });
                out.extend(icons);
            }
            Element::OutlinedText {
                text,
                class,
                color,
                width,
            } if text.lines.iter().any(|l| l.chars().any(is_pack_icon)) => {
                let icons = label_icons(&text, packs, false, &mut n);
                let text = crate::scene::Text {
                    lines: text.lines.iter().map(|l| strip_tags(l)).collect(),
                    ..text
                };
                out.push(Element::OutlinedText {
                    text,
                    class,
                    color,
                    width,
                });
                out.extend(icons);
            }
            el => out.push(el),
        }
    }
    out
}

/// The icon elements of one text's pack icons (see [`place_pack_icons`]).
fn label_icons(
    t: &crate::scene::Text,
    packs: &crate::iconify::IconPacks,
    node: bool,
    n: &mut usize,
) -> Vec<crate::scene::Element> {
    use crate::scene::{Anchor, Element};
    let fs = t.font_size;
    let mut out = Vec::new();
    for (i, line) in t.lines.iter().enumerate() {
        let baseline = t.y + i as f64 * t.line_height;
        let w = crate::text::line_width(line, fs, t.bold);
        let x0 = match t.anchor {
            Anchor::Start => t.x,
            Anchor::Middle => t.x - w / 2.0,
            Anchor::End => t.x - w,
        };
        for (at, c, name) in pack_icons(line) {
            let Some(icon) = packs.lookup(&name, None) else {
                continue;
            };
            let x = x0 + crate::text::line_width(&line[..at], fs, t.bold);
            let (iw, ih) = (advance_em(c) * fs, fs);
            *n += 1;
            let svg = crate::iconify::icon_svg_sized(&icon, iw, ih, &format!("label-icon-{n}"));
            let markup = if node {
                crate::iconify::with_css_rule(&svg.markup, &["path"], "fill", "currentColor", true)
            } else {
                svg.markup
            };
            out.push(Element::Icon {
                rect: crate::geom::Rect::new(x, baseline + 0.125 * fs - ih, iw, ih),
                color: Some(t.fill.clone()),
                svg: markup,
            });
        }
    }
    out
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
        // Unknown classes leave an empty box; Pro styles an empty `<i>` element
        // (`RAW`, then its class as tag characters).
        assert_eq!(replace_references("fa:fa-nonexistent x"), "\u{FF010} x");
        assert_eq!(
            replace_references("fal:fa-car light"),
            "\u{FF020}\u{E0066}\u{E0061}\u{E006C}\u{E0020}\u{E0066}\u{E0061}\u{E002D}\u{E0063}\u{E0061}\u{E0072} light"
        );
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

    #[test]
    fn pro_styles_are_raw_elements_like_mermaid() {
        // mermaid's regex is `(fa[bklrs]?):fa-([\w-]+)`: `fal` and `fak` match and
        // become `<i class="fak fa-foo">`, which no CSS styles (no width, no glyph).
        let label = replace_references("a fak:fa-foo y");
        let p = line_pieces(&label, 10.0, false);
        assert_eq!(p.len(), 3);
        assert!(matches!(&p[0], Piece::Text { text, .. } if text == "a "));
        assert!(matches!(&p[1], Piece::Raw { class, .. } if class == "fak fa-foo"));
        assert!(matches!(&p[2], Piece::Text { text, .. } if text == " y"));
        // The element takes no room: the text after it starts where it would.
        assert!(
            (crate::text::line_width("a ", 10.0, false)
                - crate::text::line_width(&label[..label.find('\u{FF020}').unwrap()], 10.0, false))
            .abs()
                < 1e-9
        );
        // `fal:fa-user` is a reference too; `fal-user` (no `fa-`) is plain text.
        let raw = replace_references("fal:fa-user");
        assert!(
            matches!(&line_pieces(&raw, 10.0, false)[0], Piece::Raw { class, .. } if class == "fal fa-user")
        );
        assert_eq!(replace_references("fa:fal-user x"), "fa:fal-user x");
        // `fa`, `fas`, `fab` and `far` still draw their icons.
        for prefix in ["fa", "fas", "fab", "far"] {
            let s = replace_references(&format!("{prefix}:fa-car"));
            assert!(s.chars().any(is_icon), "{prefix}");
            assert!(!s.chars().any(|c| c == '\u{FF020}'), "{prefix}");
        }
    }
}
