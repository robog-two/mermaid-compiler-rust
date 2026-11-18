//! SVG → PNG rasterisation with resvg (pure Rust). Enabled by the `png` feature.
//!
//! Text is drawn with the first available font of a label's `font-family`. The
//! web fonts mmdc loads, Recursive Variable and Open Sans, are bundled
//! ([`crate::fonts`], the `fonts` feature) and found before any system font of
//! the same name, so the redux themes' labels look as mmdc draws them; for
//! other families the machine's fonts serve, and when none matches (a bare
//! server or container), the bundled TeX Gyre Heros, so PNG output never
//! depends on what the machine has. Layout is unaffected either way: it is
//! measured from tables before any font is chosen.

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
        // The web fonts first: a query takes the first face that matches.
        #[cfg(feature = "fonts")]
        for data in crate::fonts::WEB_FILES {
            db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(data)));
        }
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
        self.render(svg, width, height, None, background)
    }

    /// Render `svg` to a `width`×`height` PNG with its drawing at `scale`
    /// device pixels per SVG pixel and its top left at (`dx`, `dy`), as
    /// [`crate::output::png_layout`] places mmdc's.
    pub fn png_placed(
        &self,
        svg: &str,
        width: u32,
        height: u32,
        (scale, dx, dy): (f64, f64, f64),
        background: Option<crate::color::Rgba>,
    ) -> Result<Vec<u8>, String> {
        self.render(svg, width, height, Some((scale, dx, dy)), background)
    }

    fn render(
        &self,
        svg: &str,
        width: u32,
        height: u32,
        placed: Option<(f64, f64, f64)>,
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
        let (k, dx, dy) = placed.map_or_else(
            || {
                let sx = width as f32 / size.width().max(1.0);
                let sy = height as f32 / size.height().max(1.0);
                (sx.min(sy), 0.0, 0.0)
            },
            |(k, dx, dy)| (k as f32, dx as f32, dy as f32),
        );
        resvg::render(
            &tree,
            tiny_skia::Transform::from_row(k, 0.0, 0.0, k, dx, dy),
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

    /// The redux themes' and C4's families draw in the bundled web fonts, by
    /// weight and slant (Open Sans' bold is the emboldened regular outline).
    #[test]
    fn web_font_families_draw_in_the_bundled_faces() {
        let r = Rasterizer::bundled_only();
        let faces = |family: &str, weight: &str, style: &str| {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"40\"><text x=\"10\" y=\"28\" font-family=\"{family}\" font-weight=\"{weight}\" font-style=\"{style}\">Hello</text></svg>"
            );
            let tree = usvg::Tree::from_str(&svg, &r.opt).unwrap();
            let mut out = Vec::new();
            collect_fonts(tree.root(), &r.opt, &mut out);
            out
        };
        let rec = "&quot;Recursive Variable&quot;, arial, sans-serif";
        assert_eq!(
            faces(rec, "normal", "normal"),
            ["RecursiveVariable-Regular"]
        );
        assert_eq!(faces(rec, "bold", "normal"), ["RecursiveVariable-Bold"]);
        assert_eq!(faces(rec, "normal", "italic"), ["RecursiveVariable-Italic"]);
        let os = "&quot;Open Sans&quot;, sans-serif";
        assert_eq!(faces(os, "normal", "normal"), ["OpenSans-Regular"]);
        assert_eq!(faces(os, "bold", "italic"), ["OpenSans-BoldItalic"]);
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
