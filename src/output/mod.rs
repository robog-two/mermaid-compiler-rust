//! Output encoders. Everything here is a pure function of the scene and options,
//! except PNG font loading, which `Rasterizer` does once up front.

mod image;
pub mod pdf;
#[cfg(feature = "png")]
pub mod png;
mod shadow;
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

/// Puppeteer's page: the body's 8px margin, the default 800px-wide viewport.
const BODY_MARGIN: f64 = 8.0;
const DEFAULT_VIEWPORT_WIDTH: f64 = 800.0;

/// A CSS length as Chromium lays it out: whole 1/64 px (`LayoutUnit`), truncated.
fn layout_unit(v: f64) -> f64 {
    (v * 64.0).floor() / 64.0
}

/// JavaScript's `Math.round`.
fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// The `<svg>` element's CSS box (width, height) on mmdc's page when the body
/// is `body_w` wide.
///
/// mermaid's `useMaxWidth` svg is `width="100%"` with `max-width` its natural
/// width, its height following the viewBox's aspect ratio; otherwise it has
/// fixed `width`/`height`. mmdc's `--size` sets `max-width` and `max-height`
/// to the size and drops the `height` attribute; for the PNG, a portrait
/// diagram's `max-width` becomes `round(size × aspect)` (`aspect_fix`).
fn svg_box(r: &Rendered, size: Option<u32>, body_w: f64, aspect_fix: bool) -> (f64, f64) {
    let vb = r.scene.view_box;
    let ratio = if vb.w > 0.0 { vb.h / vb.w } else { 0.0 };
    let natural_w = layout_unit(vb.w);
    match size {
        None if r.use_max_width => {
            let w = body_w.min(natural_w).max(0.0);
            (w, layout_unit(w * ratio))
        }
        None => (natural_w, layout_unit(vb.h)),
        Some(s) => {
            let s = f64::from(s);
            let max_w = if aspect_fix && vb.h > 0.0 && vb.w < vb.h {
                js_round(s * vb.w / vb.h)
            } else {
                s
            };
            let basis = if r.use_max_width { body_w } else { natural_w };
            let w = basis.min(max_w).max(0.0);
            (w, layout_unit(w * ratio).min(s))
        }
    }
}

/// The body's width on mmdc's first page: the 800px viewport, or `size + 16`.
fn first_body_width(size: Option<u32>) -> f64 {
    size.map_or(DEFAULT_VIEWPORT_WIDTH, |s| f64::from(s) + 2.0 * BODY_MARGIN) - 2.0 * BODY_MARGIN
}

/// How a viewBox `vw`×`vh` is drawn in a `w`×`h` box with `preserveAspectRatio`
/// `xMidYMid meet`: the scale, and the offset of the drawing in the box.
pub(crate) fn fit_meet(vw: f64, vh: f64, w: f64, h: f64) -> (f64, f64, f64) {
    let k = match (vw > 0.0, vh > 0.0) {
        (true, true) => (w / vw).min(h / vh),
        (true, false) => w / vw,
        (false, true) => h / vh,
        (false, false) => 1.0,
    };
    (k, (w - vw * k) / 2.0, (h - vh * k) / 2.0)
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
            raster_scale: None,
        },
    )
}

/// The svg's box in mmdc's PDF. Without a paper format it is the box on the
/// first page (a `useMaxWidth` diagram is at most 784px wide there), which the
/// page is then sized to; with one, the box on the paper's page.
fn pdf_box(r: &Rendered, opt: &EncodeOptions) -> (f64, f64) {
    match opt.paper {
        None => svg_box(r, opt.size, first_body_width(opt.size), false),
        Some(p) => {
            let body = p.size_pt().0 / 0.75 - 2.0 * BODY_MARGIN;
            svg_box(r, opt.size, body, false)
        }
    }
}

/// PDF bytes.
pub fn encode_pdf(r: &Rendered, opt: &EncodeOptions) -> Vec<u8> {
    let (w, h) = pdf_box(r, opt);
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

/// Where mmdc's PNG draws the diagram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PngLayout {
    /// Image size in device pixels.
    pub width: u32,
    pub height: u32,
    /// Device pixels per viewBox unit.
    pub scale: f64,
    /// The drawing's top-left corner in device pixels.
    pub dx: f64,
    pub dy: f64,
}

/// mmdc's PNG geometry. It measures the svg's box on the first page and clips
/// the screenshot to it rounded up (`x` = 8, the body margin), but first
/// resizes the viewport to `clip.x + clip.width`: the body is then 8px
/// narrower than the clip, so a `useMaxWidth` svg reflows narrower, its drawing
/// shrunk by `(clip.width − 8) / width` and blank at the right and bottom.
/// The image is the clip at the device scale factor, rounded.
pub fn png_layout(r: &Rendered, opt: &EncodeOptions) -> PngLayout {
    let (w1, h1) = svg_box(r, opt.size, first_body_width(opt.size), true);
    let (clip_w, clip_h) = (w1.ceil(), h1.ceil());
    let body = (clip_w - BODY_MARGIN).max(0.0);
    let (w2, h2) = svg_box(r, opt.size, body, true);
    let vb = r.scene.view_box;
    let (k, dx, dy) = fit_meet(vb.w, vb.h, w2, h2);
    let s = opt.scale;
    PngLayout {
        width: js_round(clip_w * s).max(1.0) as u32,
        height: js_round(clip_h * s).max(1.0) as u32,
        scale: k * s,
        dx: dx * s,
        dy: dy * s,
    }
}

/// Pixel dimensions of the PNG for a rendered diagram.
pub fn png_dimensions(r: &Rendered, opt: &EncodeOptions) -> (u32, u32) {
    let l = png_layout(r, opt);
    (l.width, l.height)
}

/// PNG bytes.
#[cfg(feature = "png")]
pub fn encode_png(
    r: &Rendered,
    opt: &EncodeOptions,
    rast: &png::Rasterizer,
) -> Result<Vec<u8>, String> {
    let vb = r.scene.view_box;
    let l = png_layout(r, opt);
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
            raster_scale: Some(l.scale),
        },
    );
    // Puppeteer paints the page white unless the background is transparent.
    let bg = if opt.background.trim() == "transparent" {
        None
    } else {
        Some(crate::color::parse(&opt.background).unwrap_or(crate::color::Rgba::WHITE))
    };
    rast.png_placed(&svg_text, l.width, l.height, (l.scale, l.dx, l.dy), bg)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::MermaidConfig;

    fn rendered(src: &str) -> Rendered {
        crate::render::render(src, &MermaidConfig::default()).unwrap()
    }

    const TD: &str =
        "graph TD\n  A[Top] --> B[Middle]\n  B --> C[Bottom]\n  B --> D[Other]\n  C --> E[End]\n";
    const WIDE: &str = "graph LR\n  A[Alpha node] --> B[Beta node] --> C[Gamma node] --> D[Delta node] --> E[Epsilon node] --> F[Zeta node] --> G[Eta node] --> H[Theta node]\n";
    const FIXED: &str = "%%{init: {\"flowchart\": {\"useMaxWidth\": false}}}%%\ngraph LR\n  A[Alpha node] --> B[Beta node] --> C[Gamma node] --> D[Delta node] --> E[Epsilon node] --> F[Zeta node] --> G[Eta node] --> H[Theta node]\n";
    const STATE: &str = "stateDiagram-v2\n  [*] --> Still\n  Still --> Moving\n  Moving --> [*]\n";

    fn opts(scale: f64, size: Option<u32>) -> EncodeOptions {
        EncodeOptions {
            scale,
            size,
            ..EncodeOptions::default()
        }
    }

    /// Sizes and drawing scales of real mmdc PNGs (Chromium's layout of the
    /// same svgs: 360×316 → 352px wide after the viewport resize, etc.).
    #[test]
    fn png_layout_follows_the_viewport_resize() {
        let r = rendered(TD);
        assert_eq!(r.scene.view_box.w, 360.0);
        let l = png_layout(&r, &opts(1.0, None));
        assert_eq!((l.width, l.height), (360, 316));
        assert!((l.scale - 352.0 / 360.0).abs() < 1e-3, "{l:?}");
        // Box 352 × floor64(352 × 316/360) = 308.96875: drawn from the top left.
        assert!(l.dx.abs() < 0.05 && l.dy.abs() < 0.05, "{l:?}");
        let l = png_layout(&r, &opts(2.0, None));
        assert_eq!((l.width, l.height), (720, 632));
        assert!((l.scale - 2.0 * 352.0 / 360.0).abs() < 2e-3);
        // --size 500: 500 × 438.875, clip 500 × 439, reflowed to 492 wide.
        let l = png_layout(&r, &opts(1.0, Some(500)));
        assert_eq!((l.width, l.height), (500, 439));
        assert!((l.scale - 492.0 / 360.0).abs() < 1e-3);
        // A portrait diagram under --size: max-width round(500 × 152/238) = 319.
        let l = png_layout(&rendered(STATE), &opts(1.0, Some(500)));
        assert_eq!((l.width, l.height), (319, 500));
        assert!((l.scale - 311.0 / 152.0).abs() < 1e-3);
        // Wider than the 784px body: 784 × 31.625 → 784 × 32, then 776 wide.
        let r = rendered(WIDE);
        assert_eq!(r.scene.view_box.w, 1512.0);
        let l = png_layout(&r, &opts(1.0, None));
        assert_eq!((l.width, l.height), (784, 32));
        // (31.296875 / 61 is the smaller scale: `meet` centres it 0.13px in.)
        assert!((l.scale - 31.296875 / 61.0).abs() < 1e-9 && (l.dx - 0.13).abs() < 0.01);
        // useMaxWidth false: fixed size, nothing reflows.
        let l = png_layout(&rendered(FIXED), &opts(1.0, None));
        assert_eq!((l.width, l.height), (1512, 61));
        assert_eq!(l.scale, 1.0);
        // A fractional scale rounds the clip: 360 × 1.3 = 468, 316 × 1.3 = 410.8.
        let l = png_layout(&rendered(TD), &opts(1.3, None));
        assert_eq!((l.width, l.height), (468, 411));
    }

    fn media_box(pdf: &[u8]) -> String {
        let s = String::from_utf8_lossy(pdf);
        let i = s.find("/MediaBox [").unwrap() + 11;
        s[i..i + s[i..].find(']').unwrap()].to_string()
    }

    /// MediaBoxes of real mmdc PDFs of the same diagrams.
    #[test]
    fn pdf_page_follows_chromium() {
        let o = EncodeOptions::default();
        assert_eq!(media_box(&encode_pdf(&rendered(TD), &o)), "0 0 282 249.12");
        assert_eq!(media_box(&encode_pdf(&rendered(WIDE), &o)), "0 0 600 36");
        assert_eq!(
            media_box(&encode_pdf(&rendered(FIXED), &o)),
            "0 0 1146 58.08"
        );
        let a4 = EncodeOptions {
            paper: Some(Paper::A4),
            ..EncodeOptions::default()
        };
        assert_eq!(
            media_box(&encode_pdf(&rendered(TD), &a4)),
            "0 0 595.92 841.92"
        );
    }

    #[test]
    fn chromium_page_sides() {
        // (requested CSS px, device units of 1/300 inch) from Chromium.
        for (px, units) in [
            (652.0, 2037.0),
            (203.0, 638.0),
            (164.0, 513.0),
            (20.0, 62.0),
            (28.0, 88.0),
            (800.0, 2500.0),
            (3334.0, 10421.0),
        ] {
            assert!(
                (pdf::page_side_pt(px / 96.0) - units * 0.24).abs() < 1e-9,
                "{px}px"
            );
        }
        let num = svg::num;
        let pt = |p: Paper| {
            let (w, h) = p.size_pt();
            (num(w), num(h))
        };
        assert_eq!(pt(Paper::A0), (num(2383.92), num(3371.04)));
        assert_eq!(pt(Paper::A3), (num(841.92), num(1191.12)));
        assert_eq!(pt(Paper::A6), (num(298.08), num(420.0)));
        assert_eq!(pt(Paper::Letter), (num(612.0), num(792.0)));
    }
}
