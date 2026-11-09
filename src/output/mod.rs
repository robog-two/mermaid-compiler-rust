//! Output encoders. Everything here is a pure function of the scene and options,
//! except PNG font loading, which `Rasterizer` does once up front.

pub mod pdf;
#[cfg(feature = "png")]
pub mod png;
pub mod svg;

use crate::render::Rendered;
use pdf::{Paper, PdfOptions};
use svg::{SvgOptions, SvgSizing};

/// Image formats mmdc can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Svg,
    Png,
    Pdf,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Png => "png",
            Format::Pdf => "pdf",
        }
    }

    pub fn from_ext(s: &str) -> Option<Format> {
        match s {
            "svg" => Some(Format::Svg),
            "png" => Some(Format::Png),
            "pdf" => Some(Format::Pdf),
            _ => None,
        }
    }
}

/// Options shared by all encoders (mirrors mmdc's flags).
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeOptions {
    pub svg_id: String,
    pub background: String,
    pub size: Option<u32>,
    pub scale: f64,
    pub paper: Option<Paper>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            svg_id: "my-svg".into(),
            background: "white".into(),
            size: None,
            scale: 1.0,
            paper: None,
        }
    }
}

/// Displayed size in CSS pixels, following mmdc's `--size` semantics: the larger
/// dimension becomes `size` (scaling up or down); without it, the natural size.
pub fn css_size(natural_w: f64, natural_h: f64, size: Option<u32>) -> (f64, f64) {
    match size {
        None => (natural_w, natural_h),
        Some(s) => {
            let s = s as f64;
            let aspect = if natural_h > 0.0 {
                natural_w / natural_h
            } else {
                1.0
            };
            if aspect >= 1.0 {
                (s, s / aspect)
            } else {
                ((s * aspect).round(), s)
            }
        }
    }
}

/// SVG document text.
pub fn encode_svg(r: &Rendered, opt: &EncodeOptions) -> String {
    let sizing = match (opt.size, r.use_max_width) {
        (Some(s), _) => SvgSizing::Size(s),
        (None, true) => SvgSizing::MaxWidth,
        (None, false) => SvgSizing::Fixed {
            width: r.scene.view_box.w,
            height: r.scene.view_box.h,
        },
    };
    svg::to_svg(
        &r.scene,
        &SvgOptions {
            id: &opt.svg_id,
            background: &opt.background,
            sizing,
            theme_css: r.theme_css.as_deref(),
        },
    )
}

/// PDF bytes.
pub fn encode_pdf(r: &Rendered, opt: &EncodeOptions) -> Vec<u8> {
    let (w, h) = css_size(r.scene.view_box.w, r.scene.view_box.h, opt.size);
    pdf::to_pdf(
        &r.scene,
        &PdfOptions {
            background: opt.background.clone(),
            width: w,
            height: h,
            paper: opt.paper,
        },
    )
}

/// Pixel dimensions of the PNG for a rendered diagram.
pub fn png_dimensions(r: &Rendered, opt: &EncodeOptions) -> (u32, u32) {
    let (w, h) = css_size(r.scene.view_box.w, r.scene.view_box.h, opt.size);
    (
        (w * opt.scale).ceil().max(1.0) as u32,
        (h * opt.scale).ceil().max(1.0) as u32,
    )
}

/// PNG bytes.
#[cfg(feature = "png")]
pub fn encode_png(
    r: &Rendered,
    opt: &EncodeOptions,
    rast: &png::Rasterizer,
) -> Result<Vec<u8>, String> {
    let vb = r.scene.view_box;
    let svg_text = svg::to_svg(
        &r.scene,
        &SvgOptions {
            id: &opt.svg_id,
            background: &opt.background,
            sizing: SvgSizing::Fixed {
                width: vb.w,
                height: vb.h,
            },
            theme_css: r.theme_css.as_deref(),
        },
    );
    let (w, h) = png_dimensions(r, opt);
    // Puppeteer paints the page white unless the background is transparent.
    let bg = if opt.background.trim() == "transparent" {
        None
    } else {
        Some(crate::color::parse(&opt.background).unwrap_or(crate::color::Rgba::WHITE))
    };
    rast.png(&svg_text, w, h, bg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_semantics() {
        assert_eq!(css_size(200.0, 100.0, None), (200.0, 100.0));
        assert_eq!(css_size(200.0, 100.0, Some(4000)), (4000.0, 2000.0));
        assert_eq!(css_size(100.0, 200.0, Some(400)), (200.0, 400.0));
    }
}
