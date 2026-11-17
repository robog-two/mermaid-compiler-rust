//! SVG → PNG rasterisation with resvg (pure Rust). Enabled by the `png` feature.
//!
//! Text is drawn with the first installed font of a label's `font-family`; when
//! none is installed (a bare server or container), with the bundled TeX Gyre
//! Heros ([`crate::fonts`], the `fonts` feature), so PNG output never depends on
//! what the machine has. Layout is unaffected either way: it is measured with
//! Arial's metrics before any font is chosen.

use resvg::tiny_skia;
use resvg::usvg::{self, fontdb};

/// Holds the loaded font database so multiple diagrams share it.
pub struct Rasterizer {
    opt: usvg::Options<'static>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Rasterizer {
    /// Load system fonts (this is the only I/O in the PNG path) and the bundled
    /// fallback.
    pub fn new() -> Self {
        Self::with_system_fonts(true)
    }

    /// Only the bundled fonts, as on a machine with no fonts installed (and
    /// deterministic across machines). Without the `fonts` feature text is not drawn.
    pub fn bundled_only() -> Self {
        Self::with_system_fonts(false)
    }

    fn with_system_fonts(system: bool) -> Self {
        let mut opt = usvg::Options::default();
        let db = opt.fontdb_mut();
        if system {
            db.load_system_fonts();
        }
        #[cfg(feature = "fonts")]
        for data in crate::fonts::FILES {
            db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(data)));
        }
        opt.font_family = "Arial".into();
        opt.font_resolver.select_font = Box::new(select_font);
        Rasterizer { opt }
    }

    /// Render `svg` (which must carry fixed width/height) to a `width`×`height` PNG,
    /// scaled uniformly to fit (`preserveAspectRatio` `xMidYMid meet` without the
    /// centring: the drawing starts at the top left).
    pub fn png(
        &self,
        svg: &str,
        width: u32,
        height: u32,
        background: Option<crate::color::Rgba>,
    ) -> Result<Vec<u8>, String> {
        self.png_scaled(svg, width, height, None, background)
    }

    /// [`Rasterizer::png`] at `scale` device pixels per SVG pixel. The canvas is
    /// the diagram's size rounded up to whole pixels, as Chromium's screenshot of
    /// the `<svg>` crops it; the drawing is not stretched to fill that rounding.
    pub fn png_scaled(
        &self,
        svg: &str,
        width: u32,
        height: u32,
        scale: Option<f64>,
        background: Option<crate::color::Rgba>,
    ) -> Result<Vec<u8>, String> {
        let tree = usvg::Tree::from_str(svg, &self.opt)
            .map_err(|e| format!("Failed to rasterise SVG: {e}"))?;
        let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1))
            .ok_or("PNG dimensions are too large")?;
        if let Some(bg) = background.filter(|c| c.a > 0.0) {
            pixmap.fill(tiny_skia::Color::from_rgba8(
                bg.r,
                bg.g,
                bg.b,
                (bg.a * 255.0).round() as u8,
            ));
        }
        let size = tree.size();
        let k = scale.map_or_else(
            || {
                let sx = width as f32 / size.width().max(1.0);
                let sy = height as f32 / size.height().max(1.0);
                sx.min(sy)
            },
            |k| k as f32,
        );
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(k, k),
            &mut pixmap.as_mut(),
        );
        pixmap
            .encode_png()
            .map_err(|e| format!("Failed to encode PNG: {e}"))
    }
}

/// The font for a `<text>`: the first installed family of its `font-family`, else
/// the bundled TeX Gyre Heros in the same weight and style, else usvg's default
/// choice (its serif font).
fn select_font(font: &usvg::Font, db: &mut std::sync::Arc<fontdb::Database>) -> Option<fontdb::ID> {
    let families: Vec<fontdb::Family> = font
        .families()
        .iter()
        .map(|f| match f {
            usvg::FontFamily::Serif => fontdb::Family::Serif,
            usvg::FontFamily::SansSerif => fontdb::Family::SansSerif,
            usvg::FontFamily::Cursive => fontdb::Family::Cursive,
            usvg::FontFamily::Fantasy => fontdb::Family::Fantasy,
            usvg::FontFamily::Monospace => fontdb::Family::Monospace,
            usvg::FontFamily::Named(s) => fontdb::Family::Name(s),
        })
        .collect();
    fn query<'a>(font: &usvg::Font, families: &'a [fontdb::Family<'a>]) -> fontdb::Query<'a> {
        fontdb::Query {
            families,
            weight: fontdb::Weight(font.weight()),
            stretch: match font.stretch() {
                usvg::FontStretch::UltraCondensed => fontdb::Stretch::UltraCondensed,
                usvg::FontStretch::ExtraCondensed => fontdb::Stretch::ExtraCondensed,
                usvg::FontStretch::Condensed => fontdb::Stretch::Condensed,
                usvg::FontStretch::SemiCondensed => fontdb::Stretch::SemiCondensed,
                usvg::FontStretch::Normal => fontdb::Stretch::Normal,
                usvg::FontStretch::SemiExpanded => fontdb::Stretch::SemiExpanded,
                usvg::FontStretch::Expanded => fontdb::Stretch::Expanded,
                usvg::FontStretch::ExtraExpanded => fontdb::Stretch::ExtraExpanded,
                usvg::FontStretch::UltraExpanded => fontdb::Stretch::UltraExpanded,
            },
            style: match font.style() {
                usvg::FontStyle::Normal => fontdb::Style::Normal,
                usvg::FontStyle::Italic => fontdb::Style::Italic,
                usvg::FontStyle::Oblique => fontdb::Style::Oblique,
            },
        }
    }
    db.query(&query(font, &families))
        .or_else(|| db.query(&query(font, &[fontdb::Family::Name(crate::fonts::FAMILY)])))
        .or_else(|| (usvg::FontResolver::default_font_selector())(font, db))
}

#[cfg(all(test, feature = "fonts"))]
mod tests {
    use super::*;

    /// A label in fonts the machine does not have still draws (in TeX Gyre Heros).
    #[test]
    fn text_draws_with_no_system_fonts() {
        let r = Rasterizer::bundled_only();
        let svg = |text: &str| {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"40\"><text x=\"10\" y=\"28\" font-family=\"'trebuchet ms', verdana, arial, sans-serif\" font-size=\"20\" font-weight=\"bold\" font-style=\"italic\">{text}</text></svg>"
            )
        };
        let ink = |png: Vec<u8>| png.len();
        let blank = ink(r.png(&svg(""), 120, 40, None).unwrap());
        let drawn = ink(r.png(&svg("Hello Ωж"), 120, 40, None).unwrap());
        assert!(drawn > blank + 200, "{drawn} vs {blank}");
        let tree = usvg::Tree::from_str(&svg("Hello"), &r.opt).unwrap();
        let mut faces = Vec::new();
        collect_fonts(tree.root(), &r.opt, &mut faces);
        assert_eq!(faces, ["TeXGyreHeros-BoldItalic"]);
    }

    fn collect_fonts(g: &usvg::Group, opt: &usvg::Options, out: &mut Vec<String>) {
        for node in g.children() {
            match node {
                usvg::Node::Group(g) => collect_fonts(g, opt, out),
                usvg::Node::Text(t) => {
                    for chunk in t.chunks() {
                        for span in chunk.spans() {
                            let id = (opt.font_resolver.select_font)(
                                span.font(),
                                &mut opt.fontdb.clone(),
                            );
                            if let Some(face) = id.and_then(|id| opt.fontdb.face(id)) {
                                out.push(face.post_script_name.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
