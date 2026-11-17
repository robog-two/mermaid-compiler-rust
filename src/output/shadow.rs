//! A blurred drop shadow as an alpha raster, for outputs without filters (PDF).
//!
//! CSS `drop-shadow()` is the shape's silhouette (its alpha: fill and stroke),
//! offset and blurred with a Gaussian. This rasterises the silhouette
//! (supersampled), then blurs it with the exact Gaussian (separable, ±4σ), so the
//! PDF can paint the shadow colour through it as a soft mask.

use crate::geom::{PathCmd, Point, Rect};

/// Raster samples per user unit (CSS pixel) of the shadow image.
pub const RES: f64 = 2.0;
/// Coverage samples per raster pixel and axis.
const SS: usize = 4;

/// The shadow's alpha raster: `w`×`h` bytes, row 0 at the top, covering `rect`
/// (user space).
pub struct AlphaImage {
    pub rect: Rect,
    pub w: usize,
    pub h: usize,
    pub alpha: Vec<u8>,
}

/// One outline of a shape: its points and whether it closes.
pub struct Subpath {
    pub pts: Vec<Point>,
    pub closed: bool,
}

/// Flatten path commands into polylines (cubics in 16 chords each, well under
/// a raster pixel for node-sized curves).
pub fn flatten(cmds: &[PathCmd]) -> Vec<Subpath> {
    let mut out: Vec<Subpath> = Vec::new();
    let mut cur = Point::default();
    for c in crate::geom::flatten_arcs(cmds).iter() {
        match *c {
            PathCmd::MoveTo(p) => {
                out.push(Subpath {
                    pts: vec![p],
                    closed: false,
                });
                cur = p;
            }
            PathCmd::LineTo(p) => {
                if out.is_empty() {
                    out.push(Subpath {
                        pts: vec![cur],
                        closed: false,
                    });
                }
                if let Some(s) = out.last_mut() {
                    s.pts.push(p);
                }
                cur = p;
            }
            PathCmd::CubicTo(a, b, p) => {
                if out.is_empty() {
                    out.push(Subpath {
                        pts: vec![cur],
                        closed: false,
                    });
                }
                let p0 = cur;
                if let Some(s) = out.last_mut() {
                    for i in 1..=16 {
                        let t = i as f64 / 16.0;
                        let u = 1.0 - t;
                        let f = |q0: f64, q1: f64, q2: f64, q3: f64| {
                            u * u * u * q0
                                + 3.0 * u * u * t * q1
                                + 3.0 * u * t * t * q2
                                + t * t * t * q3
                        };
                        s.pts
                            .push(Point::new(f(p0.x, a.x, b.x, p.x), f(p0.y, a.y, b.y, p.y)));
                    }
                }
                cur = p;
            }
            // `flatten_arcs` leaves neither.
            PathCmd::MoveBy(_) | PathCmd::ArcTo { .. } => {}
            PathCmd::Close => {
                if let Some(s) = out.last_mut() {
                    s.closed = true;
                    if let Some(&first) = s.pts.first() {
                        cur = first;
                    }
                }
            }
        }
    }
    out
}

/// The blurred silhouette of `subpaths`: filled with alpha `fill_a` (every
/// subpath implicitly closed, nonzero winding) and stroked `stroke_w` wide with
/// alpha `stroke_a` (round joins), blurred with standard deviation `std_dev`.
/// `None` when nothing is painted.
pub fn silhouette(
    subpaths: &[Subpath],
    fill_a: f64,
    stroke_a: f64,
    stroke_w: f64,
    std_dev: f64,
) -> Option<AlphaImage> {
    let mut b: Option<Rect> = None;
    for p in subpaths.iter().flat_map(|s| &s.pts) {
        let r = Rect::new(p.x, p.y, 0.0, 0.0);
        b = Some(b.map_or(r, |b| b.union(&r)));
    }
    let half = if stroke_a > 0.0 { stroke_w / 2.0 } else { 0.0 };
    let margin = half + 4.0 * std_dev + 1.0 / RES;
    let b = b?.pad(margin);
    // Whole raster pixels, aligned to the user-space grid.
    let x0 = (b.x * RES).floor() / RES;
    let y0 = (b.y * RES).floor() / RES;
    let w = ((b.right() - x0) * RES).ceil() as usize;
    let h = ((b.bottom() - y0) * RES).ceil() as usize;
    if w == 0 || h == 0 || w * h > 16_000_000 {
        return None;
    }
    let (sw, sh) = (w * SS, h * SS);
    let step = 1.0 / (RES * SS as f64);
    let sample = |i: usize, origin: f64| origin + (i as f64 + 0.5) * step;
    let mut fill = vec![false; sw * sh];
    if fill_a > 0.0 {
        // Nonzero winding along each sample row.
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for j in 0..sh {
            let y = sample(j, y0);
            xs.clear();
            for s in subpaths {
                let n = s.pts.len();
                for k in 0..n {
                    let (a, c) = (s.pts[k], s.pts[(k + 1) % n]);
                    if (a.y <= y) != (c.y <= y) {
                        let x = a.x + (y - a.y) / (c.y - a.y) * (c.x - a.x);
                        xs.push((x, if c.y > a.y { 1 } else { -1 }));
                    }
                }
            }
            xs.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut wind = 0;
            for k in 0..xs.len() {
                wind += xs[k].1;
                if wind != 0 && k + 1 < xs.len() {
                    let lo = ((xs[k].0 - x0) / step - 0.5).ceil().max(0.0) as usize;
                    let hi = ((xs[k + 1].0 - x0) / step - 0.5).floor();
                    if hi >= 0.0 {
                        let hi = (hi as usize).min(sw - 1);
                        for i in lo..=hi {
                            fill[j * sw + i] = true;
                        }
                    }
                }
            }
        }
    }
    let mut stroke = vec![false; if half > 0.0 { sw * sh } else { 0 }];
    if half > 0.0 {
        for s in subpaths {
            let n = s.pts.len();
            let segs = if s.closed { n } else { n.saturating_sub(1) };
            for k in 0..segs.max(usize::from(n == 1)) {
                let a = s.pts[k];
                let c = s.pts[(k + 1) % n];
                let ix = |x: f64| ((x - x0) / step - 0.5).clamp(0.0, sw as f64 - 1.0);
                let iy = |y: f64| ((y - y0) / step - 0.5).clamp(0.0, sh as f64 - 1.0);
                let (i0, i1) = (
                    ix(a.x.min(c.x) - half).floor() as usize,
                    ix(a.x.max(c.x) + half).ceil() as usize,
                );
                let (j0, j1) = (
                    iy(a.y.min(c.y) - half).floor() as usize,
                    iy(a.y.max(c.y) + half).ceil() as usize,
                );
                let (dx, dy) = (c.x - a.x, c.y - a.y);
                let len2 = dx * dx + dy * dy;
                for j in j0..=j1 {
                    let y = sample(j, y0);
                    for i in i0..=i1 {
                        let x = sample(i, x0);
                        let t = if len2 > 0.0 {
                            (((x - a.x) * dx + (y - a.y) * dy) / len2).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let (px, py) = (a.x + t * dx - x, a.y + t * dy - y);
                        if px * px + py * py <= half * half {
                            stroke[j * sw + i] = true;
                        }
                    }
                }
            }
        }
    }
    // Pixel coverage: the stroke painted over the fill.
    let mut cov = vec![0.0f64; w * h];
    let per = (SS * SS) as f64;
    for (j, row) in cov.chunks_mut(w).enumerate() {
        for (i, px) in row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for v in 0..SS {
                for u in 0..SS {
                    let k = (j * SS + v) * sw + i * SS + u;
                    let s = if half > 0.0 && stroke[k] {
                        stroke_a
                    } else {
                        0.0
                    };
                    let f = if fill[k] { fill_a } else { 0.0 };
                    sum += s + f * (1.0 - s);
                }
            }
            *px = sum / per;
        }
    }
    let blurred = gaussian(&cov, w, h, std_dev * RES);
    Some(AlphaImage {
        rect: Rect::new(x0, y0, w as f64 / RES, h as f64 / RES),
        w,
        h,
        alpha: blurred
            .iter()
            .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect(),
    })
}

/// A separable Gaussian blur of standard deviation `sigma` (in pixels), the
/// kernel cut at ±4σ and normalised; edges read as transparent.
fn gaussian(src: &[f64], w: usize, h: usize, sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 {
        return src.to_vec();
    }
    let r = (4.0 * sigma).ceil() as isize;
    let mut k: Vec<f64> = (-r..=r)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= total);
    let pass = |src: &[f64], horizontal: bool| -> Vec<f64> {
        let mut out = vec![0.0; w * h];
        for j in 0..h {
            for i in 0..w {
                let mut acc = 0.0;
                for (t, kv) in k.iter().enumerate() {
                    let d = t as isize - r;
                    let (x, y) = if horizontal {
                        (i as isize + d, j as isize)
                    } else {
                        (i as isize, j as isize + d)
                    };
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        acc += kv * src[y as usize * w + x as usize];
                    }
                }
                out[j * w + i] = acc;
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

/// PDF `RunLengthDecode` encoding of `data`.
pub fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 2);
    let mut i = 0;
    while i < data.len() {
        // A run of 2..128 equal bytes.
        let mut run = 1;
        while i + run < data.len() && run < 128 && data[i + run] == data[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(data[i]);
            i += run;
            continue;
        }
        // Literal bytes up to the next run of 2 (or 128 bytes).
        let start = i;
        while i < data.len() && i - start < 128 && !(i + 1 < data.len() && data[i + 1] == data[i]) {
            i += 1;
        }
        if i == start {
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&data[start..i]);
    }
    out.push(128);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(rle: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while rle[i] != 128 {
            let n = rle[i] as usize;
            if n < 128 {
                out.extend_from_slice(&rle[i + 1..i + 2 + n]);
                i += n + 2;
            } else {
                out.extend(std::iter::repeat_n(rle[i + 1], 257 - n));
                i += 2;
            }
        }
        out
    }

    #[test]
    fn run_length_round_trips() {
        let mut data = vec![0u8; 300];
        data.extend((0..=255u8).cycle().take(400));
        data.extend([7, 7, 8, 9, 9, 9, 1]);
        assert_eq!(decode(&run_length(&data)), data);
        assert_eq!(decode(&run_length(&[5])), vec![5]);
    }

    /// A filled square's edge blurs to the normal CDF: half alpha on the edge,
    /// ~16% one standard deviation outside, ~84% one inside.
    #[test]
    fn blurred_edge_follows_the_gaussian() {
        let sq = flatten(&crate::geom::polygon(&[
            Point::new(0.0, 0.0),
            Point::new(40.0, 0.0),
            Point::new(40.0, 40.0),
            Point::new(0.0, 40.0),
        ]));
        let img = silhouette(&sq, 1.0, 0.0, 0.0, 2.0).expect("an image");
        let at = |x: f64, y: f64| {
            let i = ((x - img.rect.x) * RES) as usize;
            let j = ((y - img.rect.y) * RES) as usize;
            f64::from(img.alpha[j * img.w + i]) / 255.0
        };
        // Pixel centres 0.25 inside/outside the right edge, and ±2 (1σ).
        assert!((at(39.75, 20.0) - 0.55).abs() < 0.02, "{}", at(39.75, 20.0));
        assert!((at(40.25, 20.0) - 0.45).abs() < 0.02);
        assert!((at(42.25, 20.0) - 0.13).abs() < 0.02, "{}", at(42.25, 20.0));
        assert!((at(37.75, 20.0) - 0.87).abs() < 0.02);
        assert!(at(20.0, 20.0) > 0.99);
    }

    /// An unfilled ring casts the shadow of its stroke only.
    #[test]
    fn a_stroke_alone_casts_a_ring() {
        let sq = flatten(&crate::geom::polygon(&[
            Point::new(0.0, 0.0),
            Point::new(40.0, 0.0),
            Point::new(40.0, 40.0),
            Point::new(0.0, 40.0),
        ]));
        let img = silhouette(&sq, 0.0, 1.0, 2.0, 0.5).expect("an image");
        let at = |x: f64, y: f64| {
            let i = ((x - img.rect.x) * RES) as usize;
            let j = ((y - img.rect.y) * RES) as usize;
            img.alpha[j * img.w + i]
        };
        assert!(at(20.0, 20.0) < 3);
        assert!(at(0.25, 20.0) > 200);
    }
}
