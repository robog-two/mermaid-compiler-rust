//! SVG → PNG rasterisation with resvg (pure Rust). Enabled by the `png` feature.

use resvg::{tiny_skia, usvg};

/// Holds the loaded system font database so multiple diagrams share it.
pub struct Rasterizer {
    opt: usvg::Options<'static>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Rasterizer {
    /// Load system fonts (this is the only I/O in the PNG path).
    pub fn new() -> Self {
        let mut opt = usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        opt.font_family = "Arial".into();
        Rasterizer { opt }
    }

    /// Render `svg` (which must carry fixed width/height) to a `width`×`height` PNG.
    pub fn png(
        &self,
        svg: &str,
        width: u32,
        height: u32,
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
        let sx = width as f32 / size.width().max(1.0);
        let sy = height as f32 / size.height().max(1.0);
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(sx, sy),
            &mut pixmap.as_mut(),
        );
        pixmap
            .encode_png()
            .map_err(|e| format!("Failed to encode PNG: {e}"))
    }
}
