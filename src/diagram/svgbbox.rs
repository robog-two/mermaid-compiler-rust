//! `getBBox()` of an SVG path of elliptical arcs as Chromium computes it.
//!
//! Force-directed layouts (mermaid's mindmaps) feed the boxes the browser
//! reports for the drawn nodes into a chaotic simulation, so the last bit of a
//! bang or cloud node's box matters. Chromium hands each arc to Skia's
//! `SkPath::arcTo` (SVG form), which builds conics in single precision, and
//! `getBBox()` is `SkPath::computeTightBounds()` over them; this replays that
//! arithmetic in `f32`, with Blink's number parser, the C library's `sinf`,
//! `cosf`, `tanf` and `atan2f` (glibc's, see [`libm`]) and the way Chromium
//! turns bounds into a width (see [`span`]).

/// One relative arc command: `a rx,ry phi large,sweep dx,dy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelArc {
    pub rx: f64,
    pub ry: f64,
    pub phi: f64,
    pub large: bool,
    pub sweep: bool,
    pub dx: f64,
    pub dy: f64,
}

/// A number as mermaid writes it into the path (JavaScript's shortest
/// round-trip form) and Chromium reads it back (single precision).
fn js_f32(v: f64) -> f32 {
    let s = if v != 0.0 && (v.abs() < 1e-6 || v.abs() >= 1e21) {
        format!("{v:e}")
    } else {
        format!("{v}")
    };
    blink_parse_number(&s).unwrap_or(v as f32)
}

/// Blink's SVG number parser (`GenericParseNumber` with `float`): the
/// digits are accumulated in single precision, each fraction digit scaled by
/// a running product of `0.1f`, so a long decimal can land an ulp or two away
/// from the nearest float.
fn blink_parse_number(s: &str) -> Option<f32> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut sign = 1.0f32;
    if let Some(&c) = b.first() {
        if c == b'+' || c == b'-' {
            if c == b'-' {
                sign = -1.0;
            }
            i = 1;
        }
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let (mut integer, mut multiplier) = (0.0f32, 1.0f32);
    for &d in b[start..i].iter().rev() {
        integer += multiplier * f32::from(d - b'0');
        multiplier *= 10.0;
    }
    let (mut decimal, mut frac) = (0.0f32, 1.0f32);
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            frac *= 0.1;
            decimal += f32::from(b[i] - b'0') * frac;
            i += 1;
        }
    }
    let mut number = (integer + decimal) * sign;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let exp: i32 = s[i + 1..].parse().ok()?;
        number *= 10f64.powi(exp) as f32;
    } else if i != b.len() {
        return None;
    }
    Some(number)
}

use libm::{atan2f, cosf, sinf, tanf};

/// The single-precision functions Skia calls (`sinf`, `cosf`, `tanf`,
/// `atan2f`) as glibc 2.39 computes them on x86-64 (where Chromium's Skia
/// gets them from): `sinf`/`cosf` are its FMA build of the double-precision
/// polynomial kernels, `tanf` and `atan2f` the fdlibm single-precision code.
/// They are not always correctly rounded, and the last bit shows in the
/// boxes. Arguments are below 120 in magnitude here; larger ones fall back
/// to rounding the double-precision result.
mod libm {
    /// Top 12 bits of the float with the sign bit cleared.
    fn abstop12(x: f32) -> u32 {
        (x.to_bits() >> 20) & 0x7ff
    }

    /// `sincos_t`: the cosine polynomial is negated in the second entry.
    struct Poly {
        c0: f64,
        c1: f64,
        c2: f64,
        c3: f64,
        c4: f64,
        s1: f64,
        s2: f64,
        s3: f64,
    }

    const SIGN: [f64; 4] = [1.0, -1.0, -1.0, 1.0];
    /// 2/π · 2²⁴ and π/2.
    const HPI_INV: f64 = f64::from_bits(0x4164_5f30_6dc9_c883);
    const HPI: f64 = f64::from_bits(0x3ff9_21fb_5444_2d18);
    const C1: f64 = f64::from_bits(0xbfdf_ffff_fd0c_621c);
    const C2: f64 = f64::from_bits(0x3fa5_5553_e106_8f19);
    const C3: f64 = f64::from_bits(0xbf56_c087_e89a_359d);
    const C4: f64 = f64::from_bits(0x3ef9_9343_027b_f8c3);
    const S1: f64 = f64::from_bits(0xbfc5_5554_5995_a603);
    const S2: f64 = f64::from_bits(0x3f81_1076_0523_0bc4);
    const S3: f64 = f64::from_bits(0xbf29_94eb_3774_cf24);
    const TABLE: [Poly; 2] = [
        Poly {
            c0: 1.0,
            c1: C1,
            c2: C2,
            c3: C3,
            c4: C4,
            s1: S1,
            s2: S2,
            s3: S3,
        },
        Poly {
            c0: -1.0,
            c1: -C1,
            c2: -C2,
            c3: -C3,
            c4: -C4,
            s1: S1,
            s2: S2,
            s3: S3,
        },
    ];
    /// `pio4` (0x1.921FB6p-1f).
    const PIO4: f32 = f32::from_bits(0x3f49_0fdb);

    /// `reduce_fast`: `x` modulo π/2 and the quadrant; `fma` as the FMA build
    /// contracts `x - n·hpi`.
    fn reduce_fast(x: f64, fma: bool) -> (f64, i32) {
        let r = x * HPI_INV;
        let n = ((r as i32).wrapping_add(0x80_0000)) >> 24;
        let nf = f64::from(n);
        let red = if fma {
            (-nf).mul_add(HPI, x)
        } else {
            x - nf * HPI
        };
        (red, n)
    }

    /// `sinf_poly` with GCC's contraction into fused multiply-adds.
    fn sinf_poly(x: f64, x2: f64, p: &Poly, n: i32) -> f32 {
        if n & 1 == 0 {
            let x3 = x * x2;
            let s1 = x2.mul_add(p.s3, p.s2);
            let x7 = x3 * x2;
            let s = x3.mul_add(p.s1, x);
            x7.mul_add(s1, s) as f32
        } else {
            let x4 = x2 * x2;
            let c2 = x2.mul_add(p.c4, p.c3);
            let c1 = x2.mul_add(p.c1, p.c0);
            let x6 = x4 * x2;
            let c = x4.mul_add(p.c2, c1);
            x6.mul_add(c2, c) as f32
        }
    }

    pub fn sinf(y: f32) -> f32 {
        let x = f64::from(y);
        if abstop12(y) < abstop12(PIO4) {
            if abstop12(y) < abstop12(f32::from_bits(0x3980_0000)) {
                return y;
            }
            sinf_poly(x, x * x, &TABLE[0], 0)
        } else if abstop12(y) < abstop12(120.0) {
            let (x, n) = reduce_fast(x, true);
            let s = SIGN[(n & 3) as usize];
            let p = &TABLE[usize::from(n & 2 != 0)];
            sinf_poly(x * s, x * x, p, n)
        } else {
            x.sin() as f32
        }
    }

    pub fn cosf(y: f32) -> f32 {
        let x = f64::from(y);
        if abstop12(y) < abstop12(PIO4) {
            if abstop12(y) < abstop12(f32::from_bits(0x3980_0000)) {
                return 1.0;
            }
            sinf_poly(x, x * x, &TABLE[0], 1)
        } else if abstop12(y) < abstop12(120.0) {
            let (x, n) = reduce_fast(x, true);
            let s = SIGN[(n & 3) as usize];
            let p = &TABLE[usize::from(n & 2 != 0)];
            sinf_poly(x * s, x * x, p, n ^ 1)
        } else {
            x.cos() as f32
        }
    }

    /// fdlibm's `__kernel_tanf`: tan(x + y), or -1/tan(x + y) for `iy == -1`.
    fn kernel_tanf(mut x: f32, mut y: f32, iy: i32) -> f32 {
        const PIO4: f32 = f32::from_bits(0x3f49_0fda);
        const PIO4LO: f32 = f32::from_bits(0x3322_2168);
        const T: [u32; 13] = [
            0x3eaa_aaab,
            0x3e08_8889,
            0x3d5d_0dd1,
            0x3cb3_27a4,
            0x3c11_371f,
            0x3b6b_6916,
            0x3abe_de48,
            0x3a1a_26c8,
            0x3981_37b9,
            0x38a3_f445,
            0x3895_c07a,
            0xb79b_ae5f,
            0x37d9_5384,
        ];
        let t = |i: usize| f32::from_bits(T[i]);
        let hx = x.to_bits() as i32;
        let ix = hx & 0x7fff_ffff;
        if ix < 0x3900_0000 && x as i32 == 0 {
            return if (ix | (iy + 1)) == 0 {
                1.0 / x.abs()
            } else if iy == 1 {
                x
            } else {
                -1.0 / x
            };
        }
        if ix >= 0x3f2c_a140 {
            if hx < 0 {
                x = -x;
                y = -y;
            }
            let z = PIO4 - x;
            let w = PIO4LO - y;
            x = z + w;
            y = 0.0;
            if x.abs() < f32::from_bits(0x3900_0000) {
                let sign = (1 - ((hx >> 30) & 2)) * iy;
                return sign as f32 * (1.0 - (2 * iy) as f32 * x);
            }
        }
        let z = x * x;
        let w = z * z;
        let mut r = t(1) + w * (t(3) + w * (t(5) + w * (t(7) + w * (t(9) + w * t(11)))));
        let v = z * (t(2) + w * (t(4) + w * (t(6) + w * (t(8) + w * (t(10) + w * t(12))))));
        let s = z * x;
        r = y + z * (s * (r + v) + y);
        r += t(0) * s;
        let w = x + r;
        if ix >= 0x3f2c_a140 {
            let v = iy as f32;
            return (1 - ((hx >> 30) & 2)) as f32 * (v - 2.0 * (x - (w * w / (w + v) - r)));
        }
        if iy == 1 {
            return w;
        }
        let z = f32::from_bits(w.to_bits() & 0xffff_f000);
        let v = r - (z - x);
        let a = -1.0 / w;
        let t = f32::from_bits(a.to_bits() & 0xffff_f000);
        let s = 1.0 + t * z;
        t + a * (s + t * v)
    }

    pub fn tanf(x: f32) -> f32 {
        let ix = x.to_bits() & 0x7fff_ffff;
        if ix <= 0x3f49_0fda {
            return kernel_tanf(x, 0.0, 1);
        }
        if abstop12(x) >= abstop12(120.0) {
            return f64::from(x).tan() as f32;
        }
        let (dx, n) = reduce_fast(f64::from(x), false);
        let y0 = dx as f32;
        let y1 = (dx - f64::from(y0)) as f32;
        kernel_tanf(y0, y1, 1 - ((n & 1) << 1))
    }

    /// fdlibm's `atanf`.
    fn atanf(mut x: f32) -> f32 {
        const ATANHI: [u32; 4] = [0x3eed_6338, 0x3f49_0fda, 0x3f7b_985e, 0x3fc9_0fda];
        const ATANLO: [u32; 4] = [0x31ac_3769, 0x3322_2168, 0x3314_0fb4, 0x33a2_2168];
        const AT: [u32; 11] = [
            0x3eaa_aaab,
            0xbe4c_cccd,
            0x3e12_4925,
            0xbde3_8e38,
            0x3dba_2e6e,
            0xbd9d_8795,
            0x3d88_6b35,
            0xbd6e_f16b,
            0x3d4b_da59,
            0xbd15_a221,
            0x3c85_69d7,
        ];
        let at = |i: usize| f32::from_bits(AT[i]);
        let hx = x.to_bits() as i32;
        let ix = hx & 0x7fff_ffff;
        if ix >= 0x4c00_0000 {
            if ix > 0x7f80_0000 {
                return x + x;
            }
            let v = f32::from_bits(ATANHI[3]) + f32::from_bits(ATANLO[3]);
            return if hx > 0 { v } else { -v };
        }
        let id: i32;
        if ix < 0x3ee0_0000 {
            if ix < 0x3100_0000 {
                return x;
            }
            id = -1;
        } else {
            x = x.abs();
            if ix < 0x3f98_0000 {
                if ix < 0x3f30_0000 {
                    id = 0;
                    x = (2.0 * x - 1.0) / (2.0 + x);
                } else {
                    id = 1;
                    x = (x - 1.0) / (x + 1.0);
                }
            } else if ix < 0x401c_0000 {
                id = 2;
                x = (x - 1.5) / (1.0 + 1.5 * x);
            } else {
                id = 3;
                x = -1.0 / x;
            }
        }
        let z = x * x;
        let w = z * z;
        let s1 = z * (at(0) + w * (at(2) + w * (at(4) + w * (at(6) + w * (at(8) + w * at(10))))));
        let s2 = w * (at(1) + w * (at(3) + w * (at(5) + w * (at(7) + w * at(9)))));
        if id < 0 {
            return x - x * (s1 + s2);
        }
        let i = id as usize;
        let z = f32::from_bits(ATANHI[i]) - ((x * (s1 + s2) - f32::from_bits(ATANLO[i])) - x);
        if hx < 0 { -z } else { z }
    }

    /// fdlibm's `atan2f`.
    pub fn atan2f(y: f32, x: f32) -> f32 {
        const PI_O_4: f32 = f32::from_bits(0x3f49_0fdb);
        const PI_O_2: f32 = f32::from_bits(0x3fc9_0fdb);
        const PI: f32 = f32::from_bits(0x4049_0fdb);
        const PI_LO: f32 = f32::from_bits(0xb3bb_bd2e);
        let hx = x.to_bits() as i32;
        let ix = hx & 0x7fff_ffff;
        let hy = y.to_bits() as i32;
        let iy = hy & 0x7fff_ffff;
        if ix > 0x7f80_0000 || iy > 0x7f80_0000 {
            return x + y;
        }
        if hx == 0x3f80_0000 {
            return atanf(y);
        }
        let m = ((hy >> 31) & 1) | ((hx >> 30) & 2);
        if iy == 0 {
            return match m {
                0 | 1 => y,
                2 => PI,
                _ => -PI,
            };
        }
        if ix == 0 {
            return if hy < 0 { -PI_O_2 } else { PI_O_2 };
        }
        if ix == 0x7f80_0000 {
            return if iy == 0x7f80_0000 {
                match m {
                    0 => PI_O_4,
                    1 => -PI_O_4,
                    2 => 3.0 * PI_O_4,
                    _ => -3.0 * PI_O_4,
                }
            } else {
                match m {
                    0 => 0.0,
                    1 => -0.0,
                    2 => PI,
                    _ => -PI,
                }
            };
        }
        if iy == 0x7f80_0000 {
            return if hy < 0 { -PI_O_2 } else { PI_O_2 };
        }
        let k = (iy - ix) >> 23;
        let z = if k > 60 {
            PI_O_2 + 0.5 * PI_LO
        } else if hx < 0 && k < -60 {
            0.0
        } else {
            atanf((y / x).abs())
        };
        match m {
            0 => z,
            1 => -z,
            2 => PI - (z - PI_LO),
            _ => (z - PI_LO) - PI,
        }
    }
}

/// `SK_ScalarNearlyZero`.
const NEARLY_ZERO: f32 = 1.0 / 4096.0;

fn sin_snap(x: f32) -> f32 {
    let v = sinf(x);
    if v.abs() <= NEARLY_ZERO { 0.0 } else { v }
}
fn cos_snap(x: f32) -> f32 {
    let v = cosf(x);
    if v.abs() <= NEARLY_ZERO { 0.0 } else { v }
}

/// An affine `SkMatrix` without translation: `x' = sx·x + kx·y`, `y' = ky·x + sy·y`.
#[derive(Clone, Copy)]
struct Mat {
    sx: f32,
    kx: f32,
    ky: f32,
    sy: f32,
}

impl Mat {
    fn scale(sx: f32, sy: f32) -> Mat {
        Mat {
            sx,
            kx: 0.0,
            ky: 0.0,
            sy,
        }
    }
    /// `setRotate(degrees)`.
    fn rotate(deg: f32) -> Mat {
        let rad = deg * (std::f32::consts::PI / 180.0);
        let (s, c) = (sin_snap(rad), cos_snap(rad));
        Mat {
            sx: c,
            kx: -s,
            ky: s,
            sy: c,
        }
    }
    /// `this · o` (`setConcat`, products summed in double).
    fn pre(self, o: Mat) -> Mat {
        let mam = |a: f32, b: f32, c: f32, d: f32| {
            (f64::from(a) * f64::from(b) + f64::from(c) * f64::from(d)) as f32
        };
        Mat {
            sx: mam(self.sx, o.sx, self.kx, o.ky),
            kx: mam(self.sx, o.kx, self.kx, o.sy),
            ky: mam(self.ky, o.sx, self.sy, o.ky),
            sy: mam(self.ky, o.kx, self.sy, o.sy),
        }
    }
    fn map(self, p: (f32, f32)) -> (f32, f32) {
        (p.0 * self.sx + p.1 * self.kx, p.0 * self.ky + p.1 * self.sy)
    }
}

/// A conic segment: control point, end point and weight.
type Conic = ((f32, f32), (f32, f32), f32);

/// `SkPath::arcTo(rx, ry, angle, large, sweep, x, y)` from `cur`: the conics
/// `(control, end, weight)`, or `None` where Skia draws a line.
#[allow(clippy::too_many_arguments)]
fn arc_conics(
    cur: (f32, f32),
    rx: f32,
    ry: f32,
    angle: f32,
    large: bool,
    sweep: bool,
    x: f32,
    y: f32,
) -> Option<Vec<Conic>> {
    if rx == 0.0 || ry == 0.0 || (cur.0 == x && cur.1 == y) {
        return None;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let mid = ((cur.0 - x) * 0.5, (cur.1 - y) * 0.5);
    let tm = if angle != 0.0 {
        Mat::rotate(-angle).map(mid)
    } else {
        mid
    };
    let radii_scale = tm.0 * tm.0 / (rx * rx) + tm.1 * tm.1 / (ry * ry);
    if radii_scale > 1.0 {
        let s = radii_scale.sqrt();
        rx *= s;
        ry *= s;
    }
    let mut to_unit = Mat::scale(1.0 / rx, 1.0 / ry);
    if angle != 0.0 {
        to_unit = to_unit.pre(Mat::rotate(-angle));
    }
    let u0 = to_unit.map(cur);
    let u1 = to_unit.map((x, y));
    let (mut dx, mut dy) = (u1.0 - u0.0, u1.1 - u0.1);
    let d = dx * dx + dy * dy;
    let mut scale_factor = (1.0 / d - 0.25).max(0.0).sqrt();
    // Chromium maps sweep 1 to clockwise.
    if !sweep != large {
        scale_factor = -scale_factor;
    }
    dx *= scale_factor;
    dy *= scale_factor;
    let mut c = ((u0.0 + u1.0) * 0.5, (u0.1 + u1.1) * 0.5);
    c = (c.0 + -dy, c.1 + dx);
    let theta1 = atan2f(u0.1 - c.1, u0.0 - c.0);
    let theta2 = atan2f(u1.1 - c.1, u1.0 - c.0);
    let pi = std::f32::consts::PI;
    let mut theta_arc = theta2 - theta1;
    if theta_arc < 0.0 && sweep {
        theta_arc += pi * 2.0;
    } else if theta_arc > 0.0 && !sweep {
        theta_arc -= pi * 2.0;
    }
    if theta_arc.abs() < pi / (1000.0 * 1000.0) {
        return None;
    }
    let mut from_unit = if angle != 0.0 {
        Mat::rotate(angle)
    } else {
        Mat::scale(1.0, 1.0)
    };
    from_unit.sx *= rx;
    from_unit.ky *= rx;
    from_unit.kx *= ry;
    from_unit.sy *= ry;
    let segments = (theta_arc / (2.0 * pi / 3.0)).abs().ceil() as usize;
    let width = theta_arc / segments as f32;
    let t = tanf(0.5 * width);
    if !t.is_finite() {
        return Some(Vec::new());
    }
    let w = (0.5 + cosf(width) * 0.5).sqrt();
    let is_int = |v: f32| v == v.floor();
    let expect_integers = (pi / 2.0 - width.abs()).abs() <= NEARLY_ZERO
        && is_int(rx)
        && is_int(ry)
        && is_int(x)
        && is_int(y);
    let round = |v: f32| (v + 0.5).floor();
    let mut start = theta1;
    let mut out = Vec::with_capacity(segments);
    for _ in 0..segments {
        let end = start + width;
        let (se, ce) = (sin_snap(end), cos_snap(end));
        let p1 = (ce + c.0, se + c.1);
        let p0 = (p1.0 + t * se, p1.1 + -t * ce);
        let (mut m0, mut m1) = (from_unit.map(p0), from_unit.map(p1));
        if expect_integers {
            m0 = (round(m0.0), round(m0.1));
            m1 = (round(m1.0), round(m1.1));
        }
        out.push((m0, m1, w));
        start = end;
    }
    if let Some(last) = out.last_mut() {
        last.1 = (x, y);
    }
    Some(out)
}

/// `SkFindUnitQuadRoots`.
fn unit_quad_roots(a: f32, b: f32, c: f32) -> Vec<f32> {
    let unit_divide = |mut numer: f32, mut denom: f32| -> Option<f32> {
        if numer < 0.0 {
            numer = -numer;
            denom = -denom;
        }
        if denom == 0.0 || numer == 0.0 || numer >= denom {
            return None;
        }
        let r = numer / denom;
        if r.is_nan() || r == 0.0 {
            None
        } else {
            Some(r)
        }
    };
    if a == 0.0 {
        return unit_divide(-c, b).into_iter().collect();
    }
    let dr = f64::from(b) * f64::from(b) - 4.0 * f64::from(a) * f64::from(c);
    if dr < 0.0 {
        return Vec::new();
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return Vec::new();
    }
    let q = if b < 0.0 {
        -(b - r) / 2.0
    } else {
        -(b + r) / 2.0
    };
    let mut roots: Vec<f32> = unit_divide(q, a).into_iter().collect();
    roots.extend(unit_divide(c, q));
    if roots.len() == 2 {
        if roots[0] > roots[1] {
            roots.swap(0, 1);
        } else if roots[0] == roots[1] {
            roots.pop();
        }
    }
    roots
}

/// Parameters of a conic's extrema along one axis (`SkConic::findXExtrema`).
fn conic_extrema(a: f32, b: f32, c: f32, w: f32) -> Vec<f32> {
    let p20 = c - a;
    let p10 = b - a;
    let wp10 = w * p10;
    unit_quad_roots(w * p20 - p20, p20 - 2.0 * wp10, wp10)
}

/// `SkConic::evalAt` along one axis.
fn conic_eval(p0: f32, p1: f32, p2: f32, w: f32, t: f32) -> f32 {
    let p1w = p1 * w;
    let (na, nb, nc) = (p2 - 2.0 * p1w + p0, 2.0 * (p1w - p0), p0);
    let db = 2.0 * (w - 1.0);
    let (da, dc) = (0.0 - db, 1.0);
    ((na * t + nb) * t + nc) / ((da * t + db) * t + dc)
}

/// Width of a box from its edges the way Chromium reports it: the
/// difference, one float step wider whenever `left + width` does not land
/// exactly on `right` (measured on 425 single-line paths).
fn span(lo: f32, hi: f32) -> f32 {
    let w = hi - lo;
    if lo + w != hi {
        f32::from_bits(w.to_bits() + 1)
    } else {
        w
    }
}

/// `getBBox()` of a node group holding mermaid's `M0 0 <arcs> H0 V0 Z` path
/// under `translate(tx, ty)` and a `label_w`×`label_h` label centred on the
/// origin, as `(x, y, width, height)`.
pub fn chromium_box(translate: (f64, f64), arcs: &[RelArc], label: (f64, f64)) -> [f64; 4] {
    let (lo, hi) = path_bounds(arcs);
    let (w, h) = (span(lo.0, hi.0), span(lo.1, hi.1));
    let (x, y) = (lo.0 + js_f32(translate.0), lo.1 + js_f32(translate.1));
    // Joined with the label's box.
    let (lw, lh) = (js_f32(label.0), js_f32(label.1));
    let (lx, ly) = (js_f32(-label.0 / 2.0), js_f32(-label.1 / 2.0));
    let (l, t) = (x.min(lx), y.min(ly));
    let (r, b) = ((x + w).max(lx + lw), (y + h).max(ly + lh));
    [
        f64::from(l),
        f64::from(t),
        f64::from(span(l, r)),
        f64::from(span(t, b)),
    ]
}

/// `SkPath::computeTightBounds()` of `M0 0 <arcs> H0 V0 Z`: its corners.
fn path_bounds(arcs: &[RelArc]) -> ((f32, f32), (f32, f32)) {
    let mut cur = (0.0_f32, 0.0_f32);
    let (mut lo, mut hi) = (cur, cur);
    let mut add = |p: (f32, f32)| {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    };
    for a in arcs {
        let to = (cur.0 + js_f32(a.dx), cur.1 + js_f32(a.dy));
        match arc_conics(
            cur,
            js_f32(a.rx),
            js_f32(a.ry),
            js_f32(a.phi),
            a.large,
            a.sweep,
            to.0,
            to.1,
        ) {
            None => add(to),
            Some(conics) => {
                let mut p0 = cur;
                for (p1, p2, w) in conics {
                    let mut ts = conic_extrema(p0.0, p1.0, p2.0, w);
                    ts.extend(conic_extrema(p0.1, p1.1, p2.1, w));
                    for t in ts {
                        add((
                            conic_eval(p0.0, p1.0, p2.0, w, t),
                            conic_eval(p0.1, p1.1, p2.1, w, t),
                        ));
                    }
                    add(p2);
                    p0 = p2;
                }
            }
        }
        cur = to;
    }
    // H0 V0 Z.
    add((0.0, cur.1));
    add((0.0, 0.0));
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(rx: f64, ry: f64, phi: f64, sweep: bool, dx: f64, dy: f64) -> RelArc {
        RelArc {
            rx,
            ry,
            phi,
            large: false,
            sweep,
            dx,
            dy,
        }
    }

    #[test]
    fn cloud_box_matches_chromium() {
        // cloud.ts around a 41.8125×24 label with half padding 5 (w = 51.8125,
        // h = 34); Chromium reports this box for the drawn node.
        let (w, h) = (51.8125_f64, 34.0_f64);
        let (r1, r2, r3, r4) = (0.15 * w, 0.25 * w, 0.35 * w, 0.2 * w);
        let arcs = [
            arc(r1, r1, 0.0, true, w * 0.25, -w * 0.1),
            arc(r3, r3, 1.0, true, w * 0.4, -w * 0.1),
            arc(r2, r2, 1.0, true, w * 0.35, w * 0.2),
            arc(r1, r1, 1.0, true, w * 0.15, h * 0.35),
            arc(r4, r4, 1.0, true, -w * 0.15, h * 0.65),
            arc(r2, r1, 1.0, true, -w * 0.25, w * 0.15),
            arc(r3, r3, 1.0, true, -w * 0.5, 0.0),
            arc(r1, r1, 1.0, true, -w * 0.25, -w * 0.15),
            arc(r1, r1, 1.0, true, -w * 0.1, -h * 0.35),
            arc(r4, r4, 1.0, true, w * 0.1, -h * 0.65),
        ];
        assert_eq!(
            chromium_box((-w / 2.0, -h / 2.0), &arcs, (41.8125, 24.0)),
            [
                -39.84649658203125,
                -28.68901824951172,
                81.35205841064453,
                58.90385437011719
            ]
        );
    }

    /// cloud.ts's arcs for a `w`×`h` node.
    fn cloud(w: f64, h: f64) -> Vec<RelArc> {
        let (r1, r2, r3, r4) = (0.15 * w, 0.25 * w, 0.35 * w, 0.2 * w);
        vec![
            arc(r1, r1, 0.0, true, w * 0.25, -w * 0.1),
            arc(r3, r3, 1.0, true, w * 0.4, -w * 0.1),
            arc(r2, r2, 1.0, true, w * 0.35, w * 0.2),
            arc(r1, r1, 1.0, true, w * 0.15, h * 0.35),
            arc(r4, r4, 1.0, true, -w * 0.15, h * 0.65),
            arc(r2, r1, 1.0, true, -w * 0.25, w * 0.15),
            arc(r3, r3, 1.0, true, -w * 0.5, 0.0),
            arc(r1, r1, 1.0, true, -w * 0.25, -w * 0.15),
            arc(r1, r1, 1.0, true, -w * 0.1, -h * 0.35),
            arc(r4, r4, 1.0, true, w * 0.1, -h * 0.65),
        ]
    }

    #[test]
    fn cloud_paths_match_chromium_exactly() {
        // getBBox() of the bare path in Chromium, for label sizes plus
        // padding where the conic maths, Blink's parser or the width rule
        // each used to cost an ulp.
        let cases = [
            (
                (123.4375, 24.0, 5.0),
                [
                    -13.34375,
                    -30.103803634643555,
                    166.796875,
                    98.13717651367188,
                ],
            ),
            (
                (137.71875, 48.0, 7.5),
                [
                    -18.185091018676758,
                    -34.453697204589844,
                    195.83299255371094,
                    136.40480041503906,
                ],
            ),
            (
                (175.28125, 24.0, 7.5),
                [
                    -19.028125762939453,
                    -42.92789077758789,
                    237.8515625,
                    130.4593505859375,
                ],
            ),
            (
                (150.828125, 72.0, 10.0),
                [
                    -29.097837448120117,
                    -38.539215087890625,
                    237.3015899658203,
                    174.10911560058594,
                ],
            ),
            (
                (104.140625, 72.0, 10.0),
                [
                    -36.7445068359375,
                    -28.006406784057617,
                    201.5117645263672,
                    151.66860961914062,
                ],
            ),
        ];
        for ((bw, bh, pad), want) in cases {
            let (lo, hi) = path_bounds(&cloud(bw + 2.0 * pad, bh + 2.0 * pad));
            let got = [lo.0, lo.1, span(lo.0, hi.0), span(lo.1, hi.1)].map(f64::from);
            assert_eq!(got, want, "{bw} x {bh}");
        }
    }

    #[test]
    fn numbers_parse_as_blink_does() {
        // The fraction is accumulated in floats, not rounded to the nearest.
        assert_eq!(
            f64::from(js_f32(2.503_125_000_000_000_3)),
            2.503_125_190_734_863_3
        );
        assert_eq!(f64::from(js_f32(1.933_593_75)), 1.933_593_869_209_289_6);
        assert_eq!(
            f64::from(js_f32(13.649_999_999_999_999)),
            13.649_999_618_530_273
        );
        assert_eq!(f64::from(js_f32(-19.028125)), -19.028_125_762_939_453);
        assert_eq!(js_f32(1.5e-7), 1.5e-7_f32);
        assert_eq!(blink_parse_number("12x"), None);
    }

    #[test]
    fn libm_matches_glibc() {
        // glibc 2.39 (x86-64) results, some of them not correctly rounded.
        assert_eq!(libm::atan2f(18.104_332, 45.151_306), 0.381_342_53);
        assert_eq!(libm::atan2f(7.361_224, 20.089_615), 0.351_226_78);
        assert_eq!(libm::atan2f(116.380_04, 43.806_58), 1.210_790_4);
        assert_eq!(libm::sinf(0.5), 0.479_425_55);
        assert_eq!(libm::cosf(2.0), -0.416_146_84);
        assert_eq!(libm::tanf(1.0), 1.557_407_7);
        assert_eq!(libm::tanf(-0.3), -0.309_336_26);
    }

    #[test]
    fn widths_never_fall_short_of_the_right_edge() {
        assert_eq!(span(0.0, 1.0), 1.0);
        let (lo, hi) = (-0.1_f32, 100.3_f32);
        let w = span(lo, hi);
        assert!(lo + w >= hi);
        // Chromium widens by an ulp whenever left + width misses right, in
        // either direction (getBBox of `M0 lo L0 hi`).
        let (lo, hi) = (
            -59.738_693_237_304_69_f64 as f32,
            28.215_816_497_802_734_f64 as f32,
        );
        assert_eq!(f64::from(span(lo, hi)), 87.954_521_179_199_22);
    }
}
