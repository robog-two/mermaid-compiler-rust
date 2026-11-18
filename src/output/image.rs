//! Images for PDF output: what an `<image>` or an inline icon `<svg>` draws,
//! as a PDF image XObject's samples.
//!
//! A JPEG passes through unchanged (`DCTDecode`). Every other image is
//! decoded to 8-bit RGB plus, where it is not opaque, an alpha soft mask, both
//! Flate-compressed: PNG, GIF and WebP at their own pixel size, an SVG (an
//! `image/svg+xml` image, or an icon) rasterised at [`SVG_SCALE`] samples per
//! CSS pixel of the box it is drawn in. Decoding and rasterising use resvg, so
//! without the `png` feature only JPEGs are embedded.

// Without the `png` feature only the JPEG path is live.
#![cfg_attr(not(feature = "png"), allow(dead_code))]

/// Samples per CSS pixel an SVG image or icon is rasterised at.
pub const SVG_SCALE: f64 = 4.0;
/// The largest side, in samples, an SVG is rasterised at.
const MAX_SIDE: f64 = 4096.0;

/// An image's samples, ready for an image XObject.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    /// A baseline or progressive JPEG file, embedded as it is.
    Jpeg {
        data: Vec<u8>,
        width: u32,
        height: u32,
        /// 1 (gray), 3 (RGB) or 4 (CMYK).
        components: u8,
        /// An Adobe CMYK JPEG stores its samples inverted.
        inverted: bool,
    },
    /// Flate-compressed RGB samples and, unless opaque, alpha samples.
    Flate {
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        alpha: Option<Vec<u8>>,
    },
}

/// The samples of the image at `href` (a `data:` URI) drawn over a
/// `w` × `h` CSS-pixel box, or `None` when it cannot be decoded here.
pub fn image(href: &str, w: f64, h: f64) -> Option<Samples> {
    let (mime, bytes) = crate::diagram::asset::data_uri(href)?;
    if let Some(j) = jpeg(&bytes) {
        return Some(j);
    }
    let svg = mime == "image/svg+xml"
        || (!bytes.starts_with(b"\x89PNG")
            && std::str::from_utf8(&bytes).is_ok_and(|s| s.contains("<svg")));
    if svg {
        let text = String::from_utf8(bytes).ok()?;
        return render_svg(&text, w, h);
    }
    raster(href, &bytes)
}

/// Whether the `data:` URI `href` holds an SVG document.
pub fn is_svg(href: &str) -> bool {
    let head = href
        .trim_start()
        .get(..64)
        .unwrap_or(href)
        .to_ascii_lowercase();
    head.starts_with("data:image/svg+xml")
}

/// The samples of an inline `<svg>` element (an icon) drawn over a `w` × `h`
/// box, with `currentColor` as `color`.
pub fn inline_svg(svg: &str, w: f64, h: f64, color: Option<&str>) -> Option<Samples> {
    let color = color
        .map(|c| format!(" color=\"{}\"", super::svg::esc(c)))
        .unwrap_or_default();
    let doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"{color}>{svg}</svg>"
    );
    render_svg(&doc, w, h)
}

/// A JPEG's dimensions and colour components from its frame header.
fn jpeg(b: &[u8]) -> Option<Samples> {
    if !b.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let be16 =
        |i: usize| -> Option<u32> { Some(u32::from(*b.get(i)?) << 8 | u32::from(*b.get(i + 1)?)) };
    let mut adobe = false;
    let mut i = 2;
    while i + 3 < b.len() {
        if b[i] != 0xff {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xff {
            i += 1;
            continue;
        }
        let len = be16(i + 2)? as usize;
        if marker == 0xee && b.get(i + 4..i + 9) == Some(b"Adobe") {
            adobe = true;
        }
        let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if sof {
            let (height, width) = (be16(i + 5)?, be16(i + 7)?);
            let components = *b.get(i + 9)?;
            if width == 0 || height == 0 || !matches!(components, 1 | 3 | 4) {
                return None;
            }
            return Some(Samples::Jpeg {
                data: b.to_vec(),
                width,
                height,
                components,
                inverted: adobe && components == 4,
            });
        }
        i += 2 + len;
    }
    None
}

#[cfg(feature = "png")]
fn raster(href: &str, bytes: &[u8]) -> Option<Samples> {
    // resvg decodes the formats a browser does; drawn 1:1 with nearest-
    // neighbour sampling, each sample is the file's own pixel.
    let (w, h) = crate::diagram::asset::bytes_size(bytes)?;
    let doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"><image href=\"{}\" width=\"{w}\" height=\"{h}\" image-rendering=\"optimizeSpeed\" preserveAspectRatio=\"none\"/></svg>",
        super::svg::esc(href)
    );
    rasterise(&doc, w as u32, h as u32, 1.0)
}

#[cfg(not(feature = "png"))]
fn raster(_href: &str, _bytes: &[u8]) -> Option<Samples> {
    None
}

#[cfg(feature = "png")]
fn render_svg(doc: &str, w: f64, h: f64) -> Option<Samples> {
    let k = SVG_SCALE.min(MAX_SIDE / w.max(h).max(1.0));
    let (pw, ph) = ((w * k).ceil().max(1.0), (h * k).ceil().max(1.0));
    // The document's own size is stretched over the box, as an `<image>`
    // with `preserveAspectRatio="none"` stretches it.
    let tree = resvg::usvg::Tree::from_str(doc, &options()).ok()?;
    let size = tree.size();
    let (sx, sy) = (pw as f32 / size.width(), ph as f32 / size.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw as u32, ph as u32)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(sx, sy),
        &mut pixmap.as_mut(),
    );
    Some(samples(&pixmap))
}

#[cfg(not(feature = "png"))]
fn render_svg(_doc: &str, _w: f64, _h: f64) -> Option<Samples> {
    None
}

#[cfg(feature = "png")]
fn rasterise(doc: &str, w: u32, h: u32, k: f32) -> Option<Samples> {
    let tree = resvg::usvg::Tree::from_str(doc, &options()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(k, k),
        &mut pixmap.as_mut(),
    );
    Some(samples(&pixmap))
}

/// usvg options for images: the bundled fonts only (no I/O).
#[cfg(feature = "png")]
fn options() -> resvg::usvg::Options<'static> {
    #[allow(unused_mut)]
    let mut opt = resvg::usvg::Options::default();
    #[cfg(feature = "fonts")]
    for data in crate::fonts::WEB_FILES.iter().chain(&crate::fonts::FILES) {
        opt.fontdb_mut()
            .load_font_source(resvg::usvg::fontdb::Source::Binary(std::sync::Arc::new(
                data,
            )));
    }
    opt
}

/// A pixmap's (premultiplied) pixels as compressed RGB and alpha samples.
#[cfg(feature = "png")]
fn samples(p: &resvg::tiny_skia::Pixmap) -> Samples {
    let mut rgb = Vec::with_capacity(p.data().len() / 4 * 3);
    let mut alpha = Vec::with_capacity(p.data().len() / 4);
    for px in p.pixels() {
        let c = px.demultiply();
        rgb.extend_from_slice(&[c.red(), c.green(), c.blue()]);
        alpha.push(c.alpha());
    }
    let opaque = alpha.iter().all(|&a| a == 255);
    Samples::Flate {
        width: p.width(),
        height: p.height(),
        rgb: deflate(&rgb),
        alpha: (!opaque).then(|| deflate(&alpha)),
    }
}

/// zlib-wrapped deflate (`FlateDecode`).
#[cfg(feature = "png")]
pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_frames_pass_through() {
        // SOI, an Adobe APP14, SOF0 for a 3 × 2 CMYK image, EOI.
        let mut b = vec![0xff, 0xd8, 0xff, 0xee, 0x00, 0x07];
        b.extend_from_slice(b"Adobe");
        b.extend_from_slice(&[0xff, 0xc0, 0x00, 0x08, 0x08, 0x00, 0x02, 0x00, 0x03, 0x04]);
        b.extend_from_slice(&[0xff, 0xd9]);
        match jpeg(&b) {
            Some(Samples::Jpeg {
                width,
                height,
                components,
                inverted,
                ..
            }) => assert_eq!((width, height, components, inverted), (3, 2, 4, true)),
            other => panic!("{other:?}"),
        }
    }

    #[cfg(feature = "png")]
    #[test]
    fn png_pixels_decode_exactly() {
        // 2 × 1: opaque red, half-transparent blue.
        let uri = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADklEQVR4nGP4z8AAQg0AD3oDfnfpf5cAAAAASUVORK5CYII=";
        match image(uri, 2.0, 1.0) {
            Some(Samples::Flate {
                width,
                height,
                rgb,
                alpha,
            }) => {
                assert_eq!((width, height), (2, 1));
                let inflate = |d: &[u8]| miniz_oxide::inflate::decompress_to_vec_zlib(d).unwrap();
                assert_eq!(inflate(&rgb), [255, 0, 0, 0, 0, 255]);
                assert_eq!(inflate(&alpha.unwrap()), [255, 128]);
            }
            other => panic!("{other:?}"),
        }
    }
}
