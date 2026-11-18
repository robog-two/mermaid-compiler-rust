//! Web fonts embedded in SVG output, as mmdc embeds them.
//!
//! mmdc loads the stylesheets of Recursive Variable, Open Sans 400 / 400 italic
//! and Font Awesome Free into its page; unless `--no-font-embed`, its
//! `fontEmbedder.js` then adds one `<style>` element straight after mermaid's own
//! for every stylesheet that has an `@font-face` for a font family the drawing
//! uses (the first family of an element that has text, or a Font Awesome icon
//! class), or a rule that matches an element of the drawing (Font Awesome's
//! `.fa` rules). Each holds the stylesheet's licence as a comment and all its
//! rules, the used faces' sources replaced by `data:font/woff2` URLs. A family
//! that is not used leaves its stylesheet out, so a flowchart carries Recursive
//! Variable (4 subsets, ~160 KB) and a C4 diagram, whose text is Open Sans,
//! Open Sans (2 x 10 subsets, ~350 KB); `fa:fa-car` adds Font Awesome's.
//!
//! `assets/fontembed` holds the stylesheets as mmdc serialises them (with
//! `@@FONT@@` where the data URL goes) and the woff2 files, byte for byte what
//! mmdc embeds (`tools/gen_font_embed.py`). mmdc adds its sheets in the order
//! its page's `<link>`s finished loading, which varies from run to run; this
//! writes them in the order that is the reverse of the order mmdc requests them
//! in, which is the commonest.

use crate::scene::{DiagramKind, Element, Scene};
use std::sync::OnceLock;

/// One embedded stylesheet: its CSS with a `@@FONT@@` for each woff2 file.
struct Sheet {
    css: &'static str,
    fonts: &'static [&'static [u8]],
}

macro_rules! sheet {
    ($name:literal; $($n:literal),*) => {
        Sheet {
            css: include_str!(concat!("../../assets/fontembed/", $name, ".css")),
            fonts: &[$(include_bytes!(concat!("../../assets/fontembed/", $name, "-", $n, ".woff2"))),*],
        }
    };
}

const OPEN_SANS_ITALIC: Sheet = sheet!("open-sans-italic"; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
const OPEN_SANS: Sheet = sheet!("open-sans"; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
const RECURSIVE: Sheet = sheet!("recursive"; 0, 1, 2, 3);
const FONTAWESOME: Sheet = Sheet {
    css: include_str!("../../assets/fontembed/fontawesome.css"),
    fonts: &[],
};
const FA_SOLID: Sheet = sheet!("fa-solid"; 0);
const FA_REGULAR: Sheet = sheet!("fa-regular"; 0);
const FA_BRANDS: Sheet = sheet!("fa-brands"; 0);

/// The stylesheets in the order they are written, each with a cache of its
/// `<style>` element (the base64 of a few hundred KB is not worth redoing).
const SHEETS: [&Sheet; 7] = [
    &OPEN_SANS_ITALIC,
    &OPEN_SANS,
    &RECURSIVE,
    &FONTAWESOME,
    &FA_SOLID,
    &FA_REGULAR,
    &FA_BRANDS,
];

/// The font families (of the ones mmdc loads) a drawing uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Used {
    pub recursive: bool,
    pub open_sans: bool,
    /// A Font Awesome icon in the free (solid / regular) font.
    pub fa_free: bool,
    /// A Font Awesome icon in the brands font.
    pub fa_brands: bool,
}

impl Used {
    fn any(self) -> bool {
        self.recursive || self.open_sans || self.fa_free || self.fa_brands
    }

    /// Which of [`SHEETS`] mmdc embeds.
    fn wants(self) -> [bool; 7] {
        let fa = self.fa_free || self.fa_brands;
        [
            self.open_sans,
            self.open_sans,
            self.recursive,
            // `.fa`, `.fas`, `.far` and `.fab` all have a rule in fontawesome.css.
            fa,
            self.fa_free,
            self.fa_free,
            self.fa_brands,
        ]
    }
}

/// The first family of a CSS `font-family` list, unquoted (the computed style
/// mmdc reads; case is kept, as `@font-face` names match exactly).
fn first_family(list: &str) -> String {
    let list = list.trim_start();
    let mut chars = list.chars();
    match chars.next() {
        Some(q @ ('"' | '\'')) => chars.take_while(|&c| c != q).collect(),
        _ => list
            .split(',')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn note_family(used: &mut Used, list: &str) {
    match first_family(list).as_str() {
        "Recursive Variable" => used.recursive = true,
        "Open Sans" => used.open_sans = true,
        _ => {}
    }
}

fn walk(els: &[Element], root: &str, used: &mut Used) {
    for el in els {
        match el {
            Element::Group { children, .. }
            | Element::Rotated { children, .. }
            | Element::Translated { children, .. }
            | Element::Faded { children, .. }
            | Element::Clipped { children, .. }
            | Element::Shadowed { children, .. }
            | Element::Brightened { children, .. } => walk(children, root, used),
            Element::Text { text, .. } | Element::OutlinedText { text, .. } => {
                if text.lines.iter().any(|l| !l.is_empty()) {
                    note_family(used, text.family.as_deref().unwrap_or(root));
                }
                for c in text.lines.iter().flat_map(|l| l.chars()) {
                    match crate::icons::is_brand_icon(c) {
                        Some(true) => used.fa_brands = true,
                        Some(false) => used.fa_free = true,
                        None => {}
                    }
                }
            }
            Element::Shape { .. } | Element::Image { .. } | Element::Icon { .. } => {}
        }
    }
}

/// The font families `scene`'s text uses.
pub fn used(scene: &Scene) -> Used {
    let mut used = Used::default();
    let mut texts = Used::default();
    walk(&scene.elements, &scene.font_family, &mut used);
    // A flowchart's nodes have HTML labels even when empty (`A@{ shape: sm-circ }`),
    // and those carry the root family.
    if used == texts && scene.kind == DiagramKind::Flowchart && !scene.elements.is_empty() {
        note_family(&mut texts, &scene.font_family);
        return texts;
    }
    used
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8], out: &mut String) {
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
}

/// A sheet as the `<style>` element mmdc's page serialises.
fn style_element(sheet: &Sheet) -> String {
    let mut out = String::with_capacity(
        sheet.css.len()
            + 40
            + sheet
                .fonts
                .iter()
                .map(|f| f.len() * 4 / 3 + 4)
                .sum::<usize>(),
    );
    out.push_str("<style>");
    let mut parts = sheet.css.split("@@FONT@@");
    out.push_str(parts.next().unwrap_or(""));
    for (part, font) in parts.zip(sheet.fonts) {
        out.push_str("data:font/woff2;base64,");
        base64(font, &mut out);
        out.push_str(part);
    }
    out.push_str("</style>");
    out
}

/// The `<style>` elements mmdc embeds for a drawing that uses `used`.
pub fn style_elements(used: Used) -> String {
    static CACHE: [OnceLock<String>; 7] = [const { OnceLock::new() }; 7];
    if !used.any() {
        return String::new();
    }
    let mut out = String::new();
    for ((sheet, cache), want) in SHEETS.iter().zip(&CACHE).zip(used.wants()) {
        if want {
            out.push_str(cache.get_or_init(|| style_element(sheet)));
        }
    }
    out
}

/// Put the fonts `scene` uses into `svg` (what [`super::svg::to_svg`] wrote),
/// right after its first `<style>` element, where mmdc puts them.
pub fn embed(scene: &Scene, svg: &mut String) {
    let css = style_elements(used(scene));
    if css.is_empty() {
        return;
    }
    if let Some(at) = svg.find("</style>") {
        svg.insert_str(at + "</style>".len(), &css);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_family_of_a_css_list() {
        assert_eq!(
            first_family("\"Recursive Variable\", arial, sans-serif"),
            "Recursive Variable"
        );
        assert_eq!(first_family("'Open Sans', sans-serif"), "Open Sans");
        assert_eq!(first_family("Open  Sans, sans-serif"), "Open Sans");
        assert_eq!(first_family("\"trebuchet ms\",verdana"), "trebuchet ms");
        assert_eq!(first_family(""), "");
    }

    #[test]
    fn base64_pads() {
        let enc = |d: &[u8]| {
            let mut s = String::new();
            base64(d, &mut s);
            s
        };
        assert_eq!(enc(b""), "");
        assert_eq!(enc(b"f"), "Zg==");
        assert_eq!(enc(b"fo"), "Zm8=");
        assert_eq!(enc(b"foo"), "Zm9v");
        assert_eq!(enc(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn sheets_hold_one_marker_per_font() {
        for s in SHEETS {
            assert_eq!(s.css.matches("@@FONT@@").count(), s.fonts.len());
        }
    }
}
