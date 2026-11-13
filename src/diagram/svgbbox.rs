//! `getBBox()` of an SVG path of elliptical arcs as Chromium computes it.
//!
//! Force-directed layouts (mermaid's mindmaps) feed the boxes the browser
//! reports for the drawn nodes into a chaotic simulation, so the last bit of a
//! bang or cloud node's box matters. Chromium hands each arc to Skia's
//! `SkPath::arcTo` (SVG form), which builds conics in single precision, and
//! `getBBox()` is `SkPath::computeTightBounds()` over them; this replays that
//! arithmetic in `f32`. It matches Chromium's box exactly in most cases and is
//! otherwise one float ulp narrower (see `chromium_box`).

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
    format!("{v}").parse::<f32>().unwrap_or(v as f32)
}

fn sinf(x: f32) -> f32 {
    f64::from(x).sin() as f32
}
fn cosf(x: f32) -> f32 {
    f64::from(x).cos() as f32
}
fn tanf(x: f32) -> f32 {
    f64::from(x).tan() as f32
}
fn atan2f(y: f32, x: f32) -> f32 {
    f64::from(y).atan2(f64::from(x)) as f32
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

/// Width of a box from its edges the way Chromium reports it: never so narrow
/// that `left + width` falls short of `right`.
fn span(lo: f32, hi: f32) -> f32 {
    let w = hi - lo;
    if lo + w < hi {
        f32::from_bits(w.to_bits() + 1)
    } else {
        w
    }
}

/// `getBBox()` of a node group holding mermaid's `M0 0 <arcs> H0 V0 Z` path
/// under `translate(tx, ty)` and a `label_w`×`label_h` label centred on the
/// origin, as `(x, y, width, height)`.
pub fn chromium_box(translate: (f64, f64), arcs: &[RelArc], label: (f64, f64)) -> [f64; 4] {
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
        f64::from(r - l),
        f64::from(b - t),
    ]
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

    #[test]
    fn widths_never_fall_short_of_the_right_edge() {
        assert_eq!(span(0.0, 1.0), 1.0);
        let (lo, hi) = (-0.1_f32, 100.3_f32);
        let w = span(lo, hi);
        assert!(lo + w >= hi);
    }
}
