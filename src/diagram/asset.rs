//! Icon, image and anchor nodes (`@{ icon: … }`, `@{ img: … }`,
//! `@{ shape: anchor }`): mermaid's icon.ts, iconSquare.ts, iconRounded.ts,
//! iconCircle.ts, imageSquare.ts and anchor.ts.
//!
//! mmdc registers no icon packs unless asked (`--iconPacks`, which fetches
//! them), so every `icon:` resolves to mermaid's `unknownIcon`: a blue
//! square with a white `?`, drawn here at the icon's size. An image's
//! natural size, which sizes the node unless `w`, `h` and no `constraint`
//! leave it unused, is read from its `data:` URI (PNG, GIF, JPEG, WebP, or
//! an SVG with a width and height); mermaid's `img.decode()` rejects
//! anything it cannot decode, and with no network a remote URL too, so
//! those fail as mmdc does there.

use crate::geom::{Point, Rect};

use super::graph::{Asset, Shape};

/// `unknownIcon`'s canvas (its `viewBox` is 80 square) and colour.
pub const UNKNOWN_ICON_FILL: &str = "#087ebf";

/// What mermaid throws when `img.decode()` fails.
pub const UNDECODABLE: &str = "The source image cannot be decoded.";

/// How an asset node frames its icon or image.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// icon.ts: an invisible box the icon's size.
    Bare(Rect),
    /// iconSquare.ts / iconRounded.ts: a rounded rectangle (radius 0.1 / 5)
    /// stroked in its own fill.
    Square(Rect, f64),
    /// iconCircle.ts: a circle round the icon's diagonal, 20px clear.
    Circle(Point, f64),
    /// imageSquare.ts: a framed box behind the image.
    Image(Rect),
}

/// An asset node laid out about its centre (mermaid's local frame).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The outer box (`updateNodeBounds` of the transparent outer rect).
    pub w: f64,
    pub h: f64,
    pub frame: Frame,
    /// The unknown icon's square, for an icon node.
    pub icon: Option<Rect>,
    /// The image's box, for an image node.
    pub image: Option<Rect>,
    /// The label box (`bbox`), empty for no label.
    pub label: Rect,
    /// `intersect.polygon`'s points (from the centre, `node.height` = `h`),
    /// or `None` for `intersect.rect`.
    pub clip: Option<Vec<Point>>,
}

/// The size an icon node is given before its label is measured
/// (`node.width = max(iconSize, wrappingWidth)`): the width its label wraps
/// in, less the `p`'s 2px padding a side.
pub fn label_wrap(shape: Shape, a: &Asset, wrapping: f64) -> f64 {
    let outer = match shape {
        Shape::ImageSquare => image_size(a, true, wrapping).0,
        _ => icon_size(a),
    };
    outer.max(wrapping) - 2.0 * LABEL_P_PADDING
}

/// `.icon-shape p, .image-shape p { padding: 2px }`.
pub const LABEL_P_PADDING: f64 = 2.0;

/// `max(assetHeight ?? 48, assetWidth ?? 48)`.
fn icon_size(a: &Asset) -> f64 {
    a.h.unwrap_or(48.0).max(a.w.unwrap_or(48.0))
}

/// imageSquare.ts's image width and height. `labelled`: a non-empty label
/// (which widens the image to the wrapping width).
pub fn image_size(a: &Asset, labelled: bool, wrapping: f64) -> (f64, f64) {
    let (nw, nh) = a.natural.unwrap_or((0.0, 0.0));
    let aspect = nw / nh;
    let raw = (if labelled { wrapping } else { 0.0 }).max(a.w.unwrap_or(nw));
    let w = if a.constraint {
        a.h.map_or(raw, |h| h * aspect)
    } else {
        raw
    };
    let h = if a.constraint {
        w / aspect
    } else {
        a.h.unwrap_or(nh)
    };
    (w, h)
}

/// Whether the image's natural size is needed to lay it out.
pub fn needs_natural(a: &Asset) -> bool {
    a.constraint || a.w.is_none() || a.h.is_none()
}

/// Lay out an icon or image node whose label box is `bw`×`bh` (0×0 for no
/// label), `pad` its `padding` (half of it framing a square or rounded icon).
pub fn layout(shape: Shape, a: &Asset, bw: f64, bh: f64, pad: f64, wrapping: f64) -> Layout {
    let labelled = bw > 0.0 || bh > 0.0;
    let lp = if labelled { 8.0 } else { 0.0 };
    let top = a.top;
    // The frame's centre: above the label, or below it.
    let fy = if top {
        bh / 2.0 + lp / 2.0
    } else {
        -bh / 2.0 - lp / 2.0
    };
    let size = icon_size(a);
    let (frame_w, frame_h, frame) = match shape {
        Shape::IconSquare | Shape::IconRounded => {
            let s = size + pad;
            let r = if shape == Shape::IconSquare { 0.1 } else { 5.0 };
            (
                s,
                s,
                Frame::Square(Rect::centered(Point::new(0.0, fy), s, s), r),
            )
        }
        Shape::IconCircle => {
            let d = size * std::f64::consts::SQRT_2 + 40.0;
            (d, d, Frame::Circle(Point::new(0.0, fy), d / 2.0))
        }
        Shape::ImageSquare => {
            let (w, h) = image_size(a, labelled, wrapping);
            (
                w,
                h,
                Frame::Image(Rect::centered(Point::new(0.0, fy), w, h)),
            )
        }
        _ => (
            size,
            size,
            Frame::Bare(Rect::centered(Point::new(0.0, fy), size, size)),
        ),
    };
    let w = frame_w.max(bw);
    let h = frame_h + bh + lp;
    let label_top = if top { -h / 2.0 } else { h / 2.0 - bh };
    let label = Rect::new(-bw / 2.0, label_top, bw, bh);
    let icon =
        (shape != Shape::ImageSquare).then(|| Rect::centered(Point::new(0.0, fy), size, size));
    let image = (shape == Shape::ImageSquare).then(|| {
        let y = if top { h / 2.0 - frame_h } else { -h / 2.0 };
        Rect::new(-frame_w / 2.0, y, frame_w, frame_h)
    });
    let clip = (labelled && shape != Shape::IconCircle).then(|| {
        let p = Point::new;
        let (fw, half) = (frame_w / 2.0, h / 2.0);
        if top {
            let mid = -half + bh + lp;
            vec![
                p(-bw / 2.0, -half),
                p(bw / 2.0, -half),
                p(bw / 2.0, mid),
                p(fw, mid),
                p(fw, half),
                p(-fw, half),
                p(-fw, mid),
                p(-bw / 2.0, mid),
            ]
        } else {
            let mid = -half + frame_h;
            vec![
                p(-fw, -half),
                p(fw, -half),
                p(fw, mid),
                p(bw / 2.0, mid),
                // (sic: a quarter of the label's width)
                p(bw / 2.0 / 2.0, half),
                p(-bw / 2.0, half),
                p(-bw / 2.0, mid),
                p(-fw, mid),
            ]
        }
    });
    Layout {
        w,
        h,
        frame,
        icon,
        image,
        label,
        clip,
    }
}

/// The natural size of the image at `url`, as far as it can be known without
/// fetching: a `data:` URI's PNG, GIF, JPEG or WebP header, or an SVG's
/// `width` and `height`.
pub fn natural_size(url: &str) -> Option<(f64, f64)> {
    let rest = url.trim().strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let bytes = if meta.ends_with(";base64") {
        base64(payload)?
    } else {
        percent_decode(payload)
    };
    raster_size(&bytes).or_else(|| svg_size(&bytes))
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let digits: Vec<u32> = s
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .map(val)
        .collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, d) in chunk.iter().enumerate() {
            n |= d << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn raster_size(b: &[u8]) -> Option<(f64, f64)> {
    let be16 = |i: usize| -> Option<u32> {
        Some(u32::from(b.get(i)?.to_owned()) << 8 | u32::from(*b.get(i + 1)?))
    };
    let le16 =
        |i: usize| -> Option<u32> { Some(u32::from(*b.get(i)?) | u32::from(*b.get(i + 1)?) << 8) };
    let be32 = |i: usize| -> Option<u32> { Some(be16(i)? << 16 | be16(i + 2)?) };
    let le24 = |i: usize| -> Option<u32> { Some(le16(i)? | u32::from(*b.get(i + 2)?) << 16) };
    let pair = |w: u32, h: u32| (w > 0 && h > 0).then(|| (f64::from(w), f64::from(h)));
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        return pair(be32(16)?, be32(20)?);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return pair(le16(6)?, le16(8)?);
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8 " => pair(le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let v = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                pair((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => pair(le24(24)? + 1, le24(27)? + 1),
            _ => None,
        };
    }
    if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                return None;
            }
            let marker = b[i + 1];
            if marker == 0xff {
                i += 1;
                continue;
            }
            let len = be16(i + 2)? as usize;
            let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
            if sof {
                return pair(be16(i + 7)?, be16(i + 5)?);
            }
            i += 2 + len;
        }
    }
    None
}

/// An SVG document's `width` and `height` (plain numbers or px).
fn svg_size(b: &[u8]) -> Option<(f64, f64)> {
    let s = std::str::from_utf8(b).ok()?;
    let start = s.find("<svg")?;
    let tag = &s[start..start + s[start..].find('>')?];
    let attr = |name: &str| -> Option<f64> {
        let mut rest = tag;
        loop {
            let i = rest.find(name)?;
            let before = rest[..i].chars().last();
            let after = rest[i + name.len()..].trim_start();
            rest = &rest[i + name.len()..];
            if !before.is_some_and(char::is_whitespace) {
                continue;
            }
            let v = after.strip_prefix('=')?.trim_start();
            let q = v.chars().next()?;
            let v = &v[1..v[1..].find(q)? + 1];
            return v.trim().trim_end_matches("px").parse().ok();
        }
    };
    let (w, h) = (attr("width")?, attr("height")?);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> Asset {
        Asset {
            icon: Some("fa:user".into()),
            ..Asset::default()
        }
    }

    #[test]
    fn icon_square_lays_out_as_icon_square_ts() {
        // mermaid 12.3.0, `A@{ icon: "fa:user", form: "square", label: "User" }`
        // (14px label, 33.5625 × 25 with the p's padding): a 63px square
        // 16.5 above the centre, the node 63 × 96.
        let l = layout(Shape::IconSquare, &asset(), 33.5625, 25.0, 15.0, 120.0);
        assert_eq!((l.w, l.h), (63.0, 96.0));
        assert_eq!(
            l.frame,
            Frame::Square(Rect::new(-31.5, -48.0, 63.0, 63.0), 0.1)
        );
        assert_eq!(l.label, Rect::new(-33.5625 / 2.0, 23.0, 33.5625, 25.0));
        assert_eq!(l.icon, Some(Rect::new(-24.0, -40.5, 48.0, 48.0)));
    }

    #[test]
    fn icon_circle_rings_the_icon_s_diagonal() {
        // iconCircle.ts: diameter 48·√2 + 40 = 107.882…, radius 53.941125496954285.
        let l = layout(Shape::IconCircle, &asset(), 0.0, 0.0, 15.0, 120.0);
        match l.frame {
            Frame::Circle(c, r) => {
                assert_eq!(c, Point::new(0.0, 0.0));
                assert!((r - 53.941125496954285).abs() < 1e-12);
            }
            other => panic!("{other:?}"),
        }
        assert!(l.clip.is_none());
    }

    #[test]
    fn images_widen_to_the_wrapping_width_when_labelled() {
        // imageSquare.ts (mermaid 12.3.0): a 40×20 PNG; `w: 60, h: 60` and a
        // label give 120 × 60; `constraint: on` with `h: 40` 80 × 40.
        let mut a = Asset {
            img: Some("x".into()),
            natural: Some((40.0, 20.0)),
            w: Some(60.0),
            h: Some(60.0),
            ..Asset::default()
        };
        assert_eq!(image_size(&a, true, 120.0), (120.0, 60.0));
        a.constraint = true;
        a.w = None;
        a.h = Some(40.0);
        assert_eq!(image_size(&a, true, 120.0), (80.0, 40.0));
    }

    #[test]
    fn natural_sizes_come_from_data_uri_headers() {
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAJElEQVR4nGM4YWMzIIhh1OJRi0ctHrV41OJRi0ctHrV45FgMAGQL6C7l1KfNAAAAAElFTkSuQmCC";
        assert_eq!(natural_size(png), Some((40.0, 20.0)));
        let svg = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='30'%20height='10'%3E%3C/svg%3E";
        assert_eq!(natural_size(svg), Some((30.0, 10.0)));
        assert_eq!(natural_size("https://example.com/a.png"), None);
        assert_eq!(natural_size("data:image/png;base64,AAAA"), None);
    }
}
