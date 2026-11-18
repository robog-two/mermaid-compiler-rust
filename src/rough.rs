//! A port of the rough.js 4.6.6 generator (the version mermaid 12.3.0 bundles,
//! `roughjs@4.6.6` in its lockfile; mermaid's pnpm patch leaves the bundled
//! `rough.esm.js` byte-identical to the published one), with the helpers it
//! bundles: `path-data-parser` 0.1.0, `points-on-curve` 0.2.0,
//! `points-on-path` 0.2.1 and `hachure-fill` 0.5.2.
//!
//! Every function draws exactly what rough.js draws for the same input,
//! options and seed: the same pseudo-random stream consumed in the same order,
//! the same floating-point operations in the same order.
//!
//! The one place this cannot follow rough.js is a zero seed: rough.js then
//! draws from `Math.random()`, so its output differs on every run. A zero seed
//! here stands for [`FALLBACK_SEED`] instead, which keeps the drawing
//! deterministic (see [`Random`]).
//!
//! Random state is shared the way JavaScript objects share it: rough.js keeps
//! its `Random` on the options object (`o.randomizer`), and `Object.assign`
//! copies that reference, so a copy made after the first draw continues the
//! same stream while a copy made before it starts its own. [`Options`] holds
//! the randomizer behind an `Rc` slot so that cloning behaves the same.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

/// The seed a zero seed stands for (rough.js would use `Math.random()`).
pub const FALLBACK_SEED: i32 = 1;

/// JavaScript's `ToInt32`.
fn to_int32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let m = v.trunc().rem_euclid(4_294_967_296.0);
    (m as u64 as u32) as i32
}

/// JavaScript's `Math.round` (halves round up).
pub fn js_round(v: f64) -> f64 {
    let f = v.floor();
    if v - f >= 0.5 { f + 1.0 } else { f }
}

/// rough.js `Random`: a Park–Miller (minimal standard, multiplier 48271)
/// generator over 32-bit integers. A zero seed draws from
/// [`FALLBACK_SEED`] where rough.js would call `Math.random()`.
#[derive(Debug)]
struct Random;

impl Random {
    fn start(seed: f64) -> i32 {
        match to_int32(seed) {
            0 => FALLBACK_SEED,
            s => s,
        }
    }
    fn next(state: &Cell<i32>) -> f64 {
        let s = 48271i32.wrapping_mul(state.get());
        state.set(s);
        f64::from(s & i32::MAX) / 2_147_483_648.0
    }
}

/// A shared random stream (rough.js's `o.randomizer`).
type Stream = Rc<Cell<i32>>;

/// rough.js drawing options (`Options` / `ResolvedOptions`).
#[derive(Debug, Clone)]
pub struct Options {
    pub max_randomness_offset: f64,
    pub roughness: f64,
    pub bowing: f64,
    /// `stroke`: only compared with `"none"` here.
    pub stroke: String,
    pub stroke_width: f64,
    pub curve_tightness: f64,
    pub curve_fitting: f64,
    pub curve_step_count: f64,
    pub fill_style: String,
    pub fill_weight: f64,
    pub hachure_angle: f64,
    pub hachure_gap: f64,
    pub dash_offset: f64,
    pub dash_gap: f64,
    pub zigzag_offset: f64,
    pub seed: f64,
    pub disable_multi_stroke: bool,
    pub disable_multi_stroke_fill: bool,
    pub preserve_vertices: bool,
    pub fill_shape_roughness_gain: f64,
    /// `fill`: `None` when unset (any string, `"none"` included, is truthy).
    pub fill: Option<String>,
    pub simplification: Option<f64>,
    randomizer: RefCell<Option<Stream>>,
}

impl Default for Options {
    /// `RoughGenerator`'s `defaultOptions`.
    fn default() -> Self {
        Options {
            max_randomness_offset: 2.0,
            roughness: 1.0,
            bowing: 1.0,
            stroke: "#000".into(),
            stroke_width: 1.0,
            curve_tightness: 0.0,
            curve_fitting: 0.95,
            curve_step_count: 9.0,
            fill_style: "hachure".into(),
            fill_weight: -1.0,
            hachure_angle: -41.0,
            hachure_gap: -1.0,
            dash_offset: -1.0,
            dash_gap: -1.0,
            zigzag_offset: -1.0,
            seed: 0.0,
            disable_multi_stroke: false,
            disable_multi_stroke_fill: false,
            preserve_vertices: false,
            fill_shape_roughness_gain: 0.8,
            fill: None,
            simplification: None,
            randomizer: RefCell::new(None),
        }
    }
}

impl Options {
    /// A copy that does not share this one's random stream
    /// (`Object.assign({}, o)` made before any draw).
    pub fn fresh(&self) -> Options {
        let o = self.clone();
        *o.randomizer.borrow_mut() = None;
        o
    }
}

/// One drawing operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Move([f64; 2]),
    BCurveTo([f64; 6]),
    LineTo([f64; 2]),
}

/// How an op set is painted (`OpSetType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetKind {
    /// Stroked in `stroke` at `strokeWidth`, unfilled.
    Path,
    /// Filled in `fill`, unstroked.
    FillPath,
    /// Stroked in `fill` at the fill weight (hachure lines).
    FillSketch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpSet {
    pub kind: SetKind,
    pub ops: Vec<Op>,
}

/// What a generator call returns: its op sets in drawing order.
#[derive(Debug, Clone)]
pub struct Drawable {
    pub shape: &'static str,
    pub sets: Vec<OpSet>,
    pub options: Options,
}

impl Drawable {
    /// `fillSketch`'s stroke width: `fillWeight`, or half the stroke width.
    pub fn fill_weight(&self) -> f64 {
        if self.options.fill_weight < 0.0 {
            self.options.stroke_width / 2.0
        } else {
            self.options.fill_weight
        }
    }
}

type P = [f64; 2];

// ---------------------------------------------------------------- random helpers

fn random(o: &Options) -> f64 {
    let s = o
        .randomizer
        .borrow_mut()
        .get_or_insert_with(|| Rc::new(Cell::new(Random::start(o.seed))))
        .clone();
    Random::next(&s)
}

fn offset(min: f64, max: f64, o: &Options, gain: f64) -> f64 {
    o.roughness * gain * ((random(o) * (max - min)) + min)
}

fn offset_opt(x: f64, o: &Options, gain: f64) -> f64 {
    offset(-x, x, o, gain)
}

/// `cloneOptionsAlterSeed`.
fn clone_alter_seed(o: &Options) -> Options {
    let mut r = o.fresh();
    if o.seed != 0.0 {
        r.seed = o.seed + 1.0;
    }
    r
}

// ---------------------------------------------------------------- renderer

fn double_line(x1: f64, y1: f64, x2: f64, y2: f64, o: &Options, filling: bool) -> Vec<Op> {
    let single = if filling {
        o.disable_multi_stroke_fill
    } else {
        o.disable_multi_stroke
    };
    let mut o1 = line_ops(x1, y1, x2, y2, o, true, false);
    if single {
        return o1;
    }
    o1.extend(line_ops(x1, y1, x2, y2, o, true, true));
    o1
}

fn line_ops(x1: f64, y1: f64, x2: f64, y2: f64, o: &Options, mv: bool, overlay: bool) -> Vec<Op> {
    let length_sq = (x1 - x2).powi(2) + (y1 - y2).powi(2);
    let length = length_sq.sqrt();
    let gain = if length < 200.0 {
        1.0
    } else if length > 500.0 {
        0.4
    } else {
        (-0.0016668) * length + 1.233334
    };
    let mut off = o.max_randomness_offset;
    if off * off * 100.0 > length_sq {
        off = length / 10.0;
    }
    let half = off / 2.0;
    let diverge = 0.2 + random(o) * 0.2;
    let mut mid_x = o.bowing * o.max_randomness_offset * (y2 - y1) / 200.0;
    let mut mid_y = o.bowing * o.max_randomness_offset * (x1 - x2) / 200.0;
    mid_x = offset_opt(mid_x, o, gain);
    mid_y = offset_opt(mid_y, o, gain);
    let rh = || offset_opt(half, o, gain);
    let rf = || offset_opt(off, o, gain);
    let pv = o.preserve_vertices;
    let mut ops = Vec::with_capacity(2);
    if mv {
        let (a, b) = if overlay {
            let a = x1 + if pv { 0.0 } else { rh() };
            (a, y1 + if pv { 0.0 } else { rh() })
        } else {
            let a = x1 + if pv { 0.0 } else { offset_opt(off, o, gain) };
            (a, y1 + if pv { 0.0 } else { offset_opt(off, o, gain) })
        };
        ops.push(Op::Move([a, b]));
    }
    let r = if overlay { &rh as &dyn Fn() -> f64 } else { &rf };
    let c0 = mid_x + x1 + (x2 - x1) * diverge + r();
    let c1 = mid_y + y1 + (y2 - y1) * diverge + r();
    let c2 = mid_x + x1 + 2.0 * (x2 - x1) * diverge + r();
    let c3 = mid_y + y1 + 2.0 * (y2 - y1) * diverge + r();
    let c4 = x2 + if pv { 0.0 } else { r() };
    let c5 = y2 + if pv { 0.0 } else { r() };
    ops.push(Op::BCurveTo([c0, c1, c2, c3, c4, c5]));
    ops
}

fn curve_with_offset(points: &[P], off: f64, o: &Options) -> Vec<Op> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut ps: Vec<P> = Vec::with_capacity(points.len() + 2);
    let jitter = |p: P| -> P {
        let a = p[0] + offset_opt(off, o, 1.0);
        [a, p[1] + offset_opt(off, o, 1.0)]
    };
    ps.push(jitter(points[0]));
    ps.push(jitter(points[0]));
    for i in 1..points.len() {
        ps.push(jitter(points[i]));
        if i == points.len() - 1 {
            ps.push(jitter(points[i]));
        }
    }
    curve_ops(&ps, None, o)
}

fn curve_ops(points: &[P], close: Option<P>, o: &Options) -> Vec<Op> {
    let len = points.len();
    let mut ops = Vec::new();
    if len > 3 {
        let s = 1.0 - o.curve_tightness;
        ops.push(Op::Move(points[1]));
        let mut i = 1;
        while i + 2 < len {
            let v = points[i];
            let b1 = [
                v[0] + (s * points[i + 1][0] - s * points[i - 1][0]) / 6.0,
                v[1] + (s * points[i + 1][1] - s * points[i - 1][1]) / 6.0,
            ];
            let b2 = [
                points[i + 1][0] + (s * points[i][0] - s * points[i + 2][0]) / 6.0,
                points[i + 1][1] + (s * points[i][1] - s * points[i + 2][1]) / 6.0,
            ];
            let b3 = points[i + 1];
            ops.push(Op::BCurveTo([b1[0], b1[1], b2[0], b2[1], b3[0], b3[1]]));
            i += 1;
        }
        if let Some(cp) = close {
            let ro = o.max_randomness_offset;
            let a = cp[0] + offset_opt(ro, o, 1.0);
            ops.push(Op::LineTo([a, cp[1] + offset_opt(ro, o, 1.0)]));
        }
    } else if len == 3 {
        ops.push(Op::Move(points[1]));
        ops.push(Op::BCurveTo([
            points[1][0],
            points[1][1],
            points[2][0],
            points[2][1],
            points[2][0],
            points[2][1],
        ]));
    } else if len == 2 {
        ops.extend(line_ops(
            points[0][0],
            points[0][1],
            points[1][0],
            points[1][1],
            o,
            true,
            true,
        ));
    }
    ops
}

/// `linearPath`.
fn linear_path_set(points: &[P], close: bool, o: &Options) -> OpSet {
    let len = points.len();
    let ops = if len > 2 {
        let mut ops = Vec::new();
        for i in 0..len - 1 {
            ops.extend(double_line(
                points[i][0],
                points[i][1],
                points[i + 1][0],
                points[i + 1][1],
                o,
                false,
            ));
        }
        if close {
            ops.extend(double_line(
                points[len - 1][0],
                points[len - 1][1],
                points[0][0],
                points[0][1],
                o,
                false,
            ));
        }
        ops
    } else if len == 2 {
        double_line(points[0][0], points[0][1], points[1][0], points[1][1], o, false)
    } else {
        Vec::new()
    };
    OpSet {
        kind: SetKind::Path,
        ops,
    }
}

struct EllipseParams {
    increment: f64,
    rx: f64,
    ry: f64,
}

fn ellipse_params(width: f64, height: f64, o: &Options) -> EllipseParams {
    let psq = (PI * 2.0 * (((width / 2.0).powi(2) + (height / 2.0).powi(2)) / 2.0).sqrt()).sqrt();
    let step_count = o
        .curve_step_count
        .max((o.curve_step_count / 200f64.sqrt()) * psq)
        .ceil();
    let increment = (PI * 2.0) / step_count;
    let mut rx = (width / 2.0).abs();
    let mut ry = (height / 2.0).abs();
    let fit = 1.0 - o.curve_fitting;
    rx += offset_opt(rx * fit, o, 1.0);
    ry += offset_opt(ry * fit, o, 1.0);
    EllipseParams { increment, rx, ry }
}

/// `ellipseWithParams`: the outline and the points estimating it.
fn ellipse_with_params(x: f64, y: f64, o: &Options, p: &EllipseParams) -> (Vec<P>, OpSet) {
    let inner = offset(0.4, 1.0, o, 1.0);
    let overlap = p.increment * offset(0.1, inner, o, 1.0);
    let (ap1, cp1) = ellipse_points(p.increment, x, y, p.rx, p.ry, 1.0, overlap, o);
    let mut o1 = curve_ops(&ap1, None, o);
    if !o.disable_multi_stroke && o.roughness != 0.0 {
        let (ap2, _) = ellipse_points(p.increment, x, y, p.rx, p.ry, 1.5, 0.0, o);
        o1.extend(curve_ops(&ap2, None, o));
    }
    (
        cp1,
        OpSet {
            kind: SetKind::Path,
            ops: o1,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn ellipse_points(
    increment: f64,
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    off: f64,
    overlap: f64,
    o: &Options,
) -> (Vec<P>, Vec<P>) {
    let mut core = Vec::new();
    let mut all = Vec::new();
    if o.roughness == 0.0 {
        let increment = increment / 4.0;
        all.push([cx + rx * (-increment).cos(), cy + ry * (-increment).sin()]);
        let mut angle = 0.0;
        while angle <= PI * 2.0 {
            let p = [cx + rx * angle.cos(), cy + ry * angle.sin()];
            core.push(p);
            all.push(p);
            angle += increment;
        }
        all.push([cx + rx * 0f64.cos(), cy + ry * 0f64.sin()]);
        all.push([cx + rx * increment.cos(), cy + ry * increment.sin()]);
    } else {
        let rad = offset_opt(0.5, o, 1.0) - (PI / 2.0);
        let pt = |k: f64, a: f64| -> P {
            let x = offset_opt(off, o, 1.0) + cx + k * rx * a.cos();
            [x, offset_opt(off, o, 1.0) + cy + k * ry * a.sin()]
        };
        all.push(pt(0.9, rad - increment));
        let end = PI * 2.0 + rad - 0.01;
        let mut angle = rad;
        while angle < end {
            let p = pt(1.0, angle);
            core.push(p);
            all.push(p);
            angle += increment;
        }
        all.push(pt(1.0, rad + PI * 2.0 + overlap * 0.5));
        all.push(pt(0.98, rad + overlap));
        all.push(pt(0.9, rad + overlap * 0.5));
    }
    (all, core)
}

#[allow(clippy::too_many_arguments)]
fn arc_ops(
    increment: f64,
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    strt: f64,
    stp: f64,
    off: f64,
    o: &Options,
) -> Vec<Op> {
    let rad = strt + offset_opt(0.1, o, 1.0);
    let mut points: Vec<P> = Vec::new();
    let pt = |k: f64, a: f64| -> P {
        let x = offset_opt(off, o, 1.0) + cx + k * rx * a.cos();
        [x, offset_opt(off, o, 1.0) + cy + k * ry * a.sin()]
    };
    points.push(pt(0.9, rad - increment));
    let mut angle = rad;
    while angle <= stp {
        points.push(pt(1.0, angle));
        angle += increment;
    }
    points.push([cx + rx * stp.cos(), cy + ry * stp.sin()]);
    points.push([cx + rx * stp.cos(), cy + ry * stp.sin()]);
    curve_ops(&points, None, o)
}

fn bezier_to(c: [f64; 6], current: P, o: &Options) -> Vec<Op> {
    let [x1, y1, x2, y2, x, y] = c;
    let mut ops = Vec::new();
    let mro = if o.max_randomness_offset != 0.0 {
        o.max_randomness_offset
    } else {
        1.0
    };
    let ros = [mro, mro + 0.3];
    let iterations = if o.disable_multi_stroke { 1 } else { 2 };
    let pv = o.preserve_vertices;
    for (i, &ro) in ros.iter().enumerate().take(iterations) {
        if i == 0 {
            ops.push(Op::Move(current));
        } else {
            let a = current[0] + if pv { 0.0 } else { offset_opt(ros[0], o, 1.0) };
            let b = current[1] + if pv { 0.0 } else { offset_opt(ros[0], o, 1.0) };
            ops.push(Op::Move([a, b]));
        }
        let f = if pv {
            [x, y]
        } else {
            let a = x + offset_opt(ro, o, 1.0);
            [a, y + offset_opt(ro, o, 1.0)]
        };
        let c0 = x1 + offset_opt(ro, o, 1.0);
        let c1 = y1 + offset_opt(ro, o, 1.0);
        let c2 = x2 + offset_opt(ro, o, 1.0);
        let c3 = y2 + offset_opt(ro, o, 1.0);
        ops.push(Op::BCurveTo([c0, c1, c2, c3, f[0], f[1]]));
    }
    ops
}

/// `svgPath`: every segment of `d` drawn rough.
fn svg_path_set(d: &str, o: &Options) -> OpSet {
    let segs = normalize(&absolutize(&parse_path(d)));
    let mut ops = Vec::new();
    let mut first = [0.0, 0.0];
    let mut current = [0.0, 0.0];
    for s in segs {
        match s {
            Seg::M(p) => {
                current = p;
                first = p;
            }
            Seg::L(p) => {
                ops.extend(double_line(current[0], current[1], p[0], p[1], o, false));
                current = p;
            }
            Seg::C(c) => {
                ops.extend(bezier_to(c, current, o));
                current = [c[4], c[5]];
            }
            Seg::Z => {
                ops.extend(double_line(
                    current[0], current[1], first[0], first[1], o, false,
                ));
                current = first;
            }
        }
    }
    OpSet {
        kind: SetKind::Path,
        ops,
    }
}

fn solid_fill_polygon(polygons: &[Vec<P>], o: &Options) -> OpSet {
    let mut ops = Vec::new();
    for points in polygons {
        if points.len() > 2 {
            let off = o.max_randomness_offset;
            let a = points[0][0] + offset_opt(off, o, 1.0);
            ops.push(Op::Move([a, points[0][1] + offset_opt(off, o, 1.0)]));
            for p in &points[1..] {
                let a = p[0] + offset_opt(off, o, 1.0);
                ops.push(Op::LineTo([a, p[1] + offset_opt(off, o, 1.0)]));
            }
        }
    }
    OpSet {
        kind: SetKind::FillPath,
        ops,
    }
}

// ---------------------------------------------------------------- fillers

/// `polygonHachureLines` (scan-line-hachure.ts).
fn polygon_hachure_lines(polygons: &mut [Vec<P>], o: &Options) -> Vec<[P; 2]> {
    let angle = o.hachure_angle + 90.0;
    let mut gap = o.hachure_gap;
    if gap < 0.0 {
        gap = o.stroke_width * 4.0;
    }
    gap = js_round(gap.max(0.1));
    let mut skip = 1.0;
    if o.roughness >= 1.0 {
        // `o.randomizer?.next() || Math.random()`: the existing stream only.
        let existing = o.randomizer.borrow().clone();
        let r = match existing {
            Some(s) => Random::next(&s),
            None => Random::next(&Cell::new(FALLBACK_SEED)),
        };
        if r > 0.7 {
            skip = gap;
        }
    }
    hachure_lines(polygons, gap, angle, if skip != 0.0 { skip } else { 1.0 })
}

fn rotate_points(points: &mut [P], center: P, degrees: f64) {
    if points.is_empty() {
        return;
    }
    let [cx, cy] = center;
    let angle = (PI / 180.0) * degrees;
    let (cos, sin) = (angle.cos(), angle.sin());
    for p in points {
        let [x, y] = *p;
        p[0] = ((x - cx) * cos) - ((y - cy) * sin) + cx;
        p[1] = ((x - cx) * sin) + ((y - cy) * cos) + cy;
    }
}

/// `hachureLines` (hachure-fill): rotates the polygons in place and back.
fn hachure_lines(polygons: &mut [Vec<P>], gap: f64, angle: f64, step: f64) -> Vec<[P; 2]> {
    let gap = gap.max(0.1);
    let center = [0.0, 0.0];
    if angle != 0.0 {
        for poly in polygons.iter_mut() {
            rotate_points(poly, center, angle);
        }
    }
    let mut lines = straight_hachure_lines(polygons, gap, step);
    if angle != 0.0 {
        for poly in polygons.iter_mut() {
            rotate_points(poly, center, -angle);
        }
        let mut pts: Vec<P> = lines.iter().flat_map(|l| [l[0], l[1]]).collect();
        rotate_points(&mut pts, center, -angle);
        for (i, l) in lines.iter_mut().enumerate() {
            *l = [pts[2 * i], pts[2 * i + 1]];
        }
    }
    lines
}

struct ScanEdge {
    ymin: f64,
    ymax: f64,
    x: f64,
    islope: f64,
}

fn sign_cmp(d: f64) -> std::cmp::Ordering {
    if d < 0.0 {
        std::cmp::Ordering::Less
    } else if d > 0.0 {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

fn straight_hachure_lines(polygons: &[Vec<P>], gap: f64, step: f64) -> Vec<[P; 2]> {
    let mut vertex_array: Vec<Vec<P>> = Vec::new();
    for poly in polygons {
        let mut v = poly.clone();
        if v.is_empty() {
            continue;
        }
        if v[0] != v[v.len() - 1] {
            v.push(v[0]);
        }
        if v.len() > 2 {
            vertex_array.push(v);
        }
    }
    let mut lines = Vec::new();
    let gap = gap.max(0.1);
    let mut edges: Vec<ScanEdge> = Vec::new();
    for v in &vertex_array {
        for i in 0..v.len() - 1 {
            let (p1, p2) = (v[i], v[i + 1]);
            if p1[1] != p2[1] {
                let ymin = p1[1].min(p2[1]);
                edges.push(ScanEdge {
                    ymin,
                    ymax: p1[1].max(p2[1]),
                    x: if ymin == p1[1] { p1[0] } else { p2[0] },
                    islope: (p2[0] - p1[0]) / (p2[1] - p1[1]),
                });
            }
        }
    }
    edges.sort_by(|e1, e2| {
        if e1.ymin < e2.ymin {
            return std::cmp::Ordering::Less;
        }
        if e1.ymin > e2.ymin {
            return std::cmp::Ordering::Greater;
        }
        if e1.x < e2.x {
            return std::cmp::Ordering::Less;
        }
        if e1.x > e2.x {
            return std::cmp::Ordering::Greater;
        }
        sign_cmp(e1.ymax - e2.ymax)
    });
    if edges.is_empty() {
        return lines;
    }
    // Edges not yet active, by index into `edges` (which keeps their state).
    let mut pending: std::collections::VecDeque<usize> = (0..edges.len()).collect();
    let mut active: Vec<usize> = Vec::new();
    let mut y = edges[0].ymin;
    let mut iteration = 0.0f64;
    while !active.is_empty() || !pending.is_empty() {
        if !pending.is_empty() {
            let mut take = 0;
            for &i in &pending {
                if edges[i].ymin > y {
                    break;
                }
                take += 1;
            }
            for _ in 0..take {
                if let Some(i) = pending.pop_front() {
                    active.push(i);
                }
            }
        }
        active.retain(|&i| edges[i].ymax > y);
        active.sort_by(|&a, &b| sign_cmp(edges[a].x - edges[b].x));
        if (step != 1.0 || iteration % gap == 0.0) && active.len() > 1 {
            let mut i = 0;
            while i + 1 < active.len() {
                let (ce, ne) = (&edges[active[i]], &edges[active[i + 1]]);
                lines.push([[js_round(ce.x), y], [js_round(ne.x), y]]);
                i += 2;
            }
        }
        y += step;
        for &i in &active {
            edges[i].x += step * edges[i].islope;
        }
        iteration += 1.0;
    }
    lines
}

fn line_length(l: &[P; 2]) -> f64 {
    ((l[0][0] - l[1][0]).powi(2) + (l[0][1] - l[1][1]).powi(2)).sqrt()
}

fn render_lines(lines: &[[P; 2]], o: &Options) -> Vec<Op> {
    let mut ops = Vec::new();
    for l in lines {
        ops.extend(double_line(l[0][0], l[0][1], l[1][0], l[1][1], o, true));
    }
    ops
}

fn hachure_fill(polygons: &mut [Vec<P>], o: &Options) -> Vec<Op> {
    let lines = polygon_hachure_lines(polygons, o);
    render_lines(&lines, o)
}

/// `patternFillPolygons`: the filler `o.fillStyle` names (`hachure` for
/// unknown names, and for `dots`, whose dots rough.js places with
/// `Math.random()`).
fn pattern_fill_polygons(polygons: &mut [Vec<P>], o: &Options) -> OpSet {
    let ops = match o.fill_style.as_str() {
        "zigzag" => {
            let mut gap = o.hachure_gap;
            if gap < 0.0 {
                gap = o.stroke_width * 4.0;
            }
            gap = gap.max(0.1);
            let o2 = Options {
                hachure_gap: gap,
                ..o.clone()
            };
            let lines = polygon_hachure_lines(polygons, &o2);
            let a = (PI / 180.0) * o.hachure_angle;
            let (dgx, dgy) = (gap * 0.5 * a.cos(), gap * 0.5 * a.sin());
            let mut zz = Vec::new();
            for [p1, p2] in lines {
                if line_length(&[p1, p2]) != 0.0 {
                    zz.push([[p1[0] - dgx, p1[1] + dgy], p2]);
                    zz.push([[p1[0] + dgx, p1[1] - dgy], p2]);
                }
            }
            render_lines(&zz, o)
        }
        "cross-hatch" => {
            let mut ops = hachure_fill(polygons, o);
            let o2 = Options {
                hachure_angle: o.hachure_angle + 90.0,
                ..o.clone()
            };
            ops.extend(hachure_fill(polygons, &o2));
            ops
        }
        "dashed" => {
            let lines = polygon_hachure_lines(polygons, o);
            let fallback = if o.hachure_gap < 0.0 {
                o.stroke_width * 4.0
            } else {
                o.hachure_gap
            };
            let off = if o.dash_offset < 0.0 {
                fallback
            } else {
                o.dash_offset
            };
            let gap = if o.dash_gap < 0.0 { fallback } else { o.dash_gap };
            let mut ops = Vec::new();
            for l in &lines {
                let length = line_length(l);
                let count = (length / (off + gap)).floor();
                let start_off = (length + gap - (count * (off + gap))) / 2.0;
                let (mut p1, mut p2) = (l[0], l[1]);
                if p1[0] > p2[0] {
                    std::mem::swap(&mut p1, &mut p2);
                }
                let alpha = ((p2[1] - p1[1]) / (p2[0] - p1[0])).atan();
                let mut i = 0.0;
                while i < count {
                    let ls = i * (off + gap);
                    let le = ls + off;
                    let s = [
                        p1[0] + (ls * alpha.cos()) + (start_off * alpha.cos()),
                        p1[1] + ls * alpha.sin() + (start_off * alpha.sin()),
                    ];
                    let e = [
                        p1[0] + (le * alpha.cos()) + (start_off * alpha.cos()),
                        p1[1] + (le * alpha.sin()) + (start_off * alpha.sin()),
                    ];
                    ops.extend(double_line(s[0], s[1], e[0], e[1], o, true));
                    i += 1.0;
                }
            }
            ops
        }
        "zigzag-line" => {
            let gap = if o.hachure_gap < 0.0 {
                o.stroke_width * 4.0
            } else {
                o.hachure_gap
            };
            let zo = if o.zigzag_offset < 0.0 {
                gap
            } else {
                o.zigzag_offset
            };
            let o2 = Options {
                hachure_gap: gap + zo,
                ..o.clone()
            };
            let lines = polygon_hachure_lines(polygons, &o2);
            let mut ops = Vec::new();
            for l in &lines {
                let length = line_length(l);
                let count = js_round(length / (2.0 * zo));
                let (mut p1, mut p2) = (l[0], l[1]);
                if p1[0] > p2[0] {
                    std::mem::swap(&mut p1, &mut p2);
                }
                let alpha = ((p2[1] - p1[1]) / (p2[0] - p1[0])).atan();
                let mut i = 0.0;
                while i < count {
                    let ls = i * 2.0 * zo;
                    let le = (i + 1.0) * 2.0 * zo;
                    let dz = (2.0 * zo.powi(2)).sqrt();
                    let s = [p1[0] + (ls * alpha.cos()), p1[1] + ls * alpha.sin()];
                    let e = [p1[0] + (le * alpha.cos()), p1[1] + (le * alpha.sin())];
                    let m = [
                        s[0] + dz * (alpha + PI / 4.0).cos(),
                        s[1] + dz * (alpha + PI / 4.0).sin(),
                    ];
                    ops.extend(double_line(s[0], s[1], m[0], m[1], &o2, true));
                    ops.extend(double_line(m[0], m[1], e[0], e[1], &o2, true));
                    i += 1.0;
                }
            }
            ops
        }
        _ => hachure_fill(polygons, o),
    };
    OpSet {
        kind: SetKind::FillSketch,
        ops,
    }
}

// ---------------------------------------------------------------- generator

fn drawable(shape: &'static str, sets: Vec<OpSet>, o: Options) -> Drawable {
    Drawable {
        shape,
        sets,
        options: o,
    }
}

/// `o.fill` is truthy.
fn fills(o: &Options) -> bool {
    o.fill.as_deref().is_some_and(|f| !f.is_empty())
}

fn has_stroke(o: &Options) -> bool {
    o.stroke != "none"
}

/// `generator.line`.
pub fn line(x1: f64, y1: f64, x2: f64, y2: f64, o: Options) -> Drawable {
    let s = OpSet {
        kind: SetKind::Path,
        ops: double_line(x1, y1, x2, y2, &o, false),
    };
    drawable("line", vec![s], o)
}

/// `generator.rectangle`.
pub fn rectangle(x: f64, y: f64, width: f64, height: f64, o: Options) -> Drawable {
    let points = [
        [x, y],
        [x + width, y],
        [x + width, y + height],
        [x, y + height],
    ];
    let outline = linear_path_set(&points, true, &o);
    let mut sets = Vec::new();
    if fills(&o) {
        let mut polys = vec![points.to_vec()];
        if o.fill_style == "solid" {
            sets.push(solid_fill_polygon(&polys, &o));
        } else {
            sets.push(pattern_fill_polygons(&mut polys, &o));
        }
    }
    if has_stroke(&o) {
        sets.push(outline);
    }
    drawable("rectangle", sets, o)
}

/// `generator.ellipse` (centre `x, y`).
pub fn ellipse(x: f64, y: f64, width: f64, height: f64, o: Options) -> Drawable {
    let params = ellipse_params(width, height, &o);
    let (estimated, outline) = ellipse_with_params(x, y, &o, &params);
    let mut sets = Vec::new();
    if fills(&o) {
        if o.fill_style == "solid" {
            let (_, mut shape) = ellipse_with_params(x, y, &o, &params);
            shape.kind = SetKind::FillPath;
            sets.push(shape);
        } else {
            sets.push(pattern_fill_polygons(&mut [estimated], &o));
        }
    }
    if has_stroke(&o) {
        sets.push(outline);
    }
    drawable("ellipse", sets, o)
}

/// `generator.circle`.
pub fn circle(x: f64, y: f64, diameter: f64, o: Options) -> Drawable {
    let mut d = ellipse(x, y, diameter, diameter, o);
    d.shape = "circle";
    d
}

/// `generator.linearPath`.
pub fn linear_path(points: &[[f64; 2]], o: Options) -> Drawable {
    let s = linear_path_set(points, false, &o);
    drawable("linearPath", vec![s], o)
}

/// `generator.polygon`.
pub fn polygon(points: &[[f64; 2]], o: Options) -> Drawable {
    let outline = linear_path_set(points, true, &o);
    let mut sets = Vec::new();
    if fills(&o) {
        let mut polys = vec![points.to_vec()];
        if o.fill_style == "solid" {
            sets.push(solid_fill_polygon(&polys, &o));
        } else {
            sets.push(pattern_fill_polygons(&mut polys, &o));
        }
    }
    if has_stroke(&o) {
        sets.push(outline);
    }
    drawable("polygon", sets, o)
}

/// `generator.arc` (angles in radians).
#[allow(clippy::too_many_arguments)]
pub fn arc(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    start: f64,
    stop: f64,
    closed: bool,
    o: Options,
) -> Drawable {
    let arc_set = |rough_closure: bool, o: &Options| -> OpSet {
        let (cx, cy) = (x, y);
        let mut rx = (width / 2.0).abs();
        let mut ry = (height / 2.0).abs();
        rx += offset_opt(rx * 0.01, o, 1.0);
        ry += offset_opt(ry * 0.01, o, 1.0);
        let (mut strt, mut stp) = (start, stop);
        while strt < 0.0 {
            strt += PI * 2.0;
            stp += PI * 2.0;
        }
        if stp - strt > PI * 2.0 {
            strt = 0.0;
            stp = PI * 2.0;
        }
        let ellipse_inc = (PI * 2.0) / o.curve_step_count;
        let arc_inc = (ellipse_inc / 2.0).min((stp - strt) / 2.0);
        let mut ops = arc_ops(arc_inc, cx, cy, rx, ry, strt, stp, 1.0, o);
        if !o.disable_multi_stroke {
            ops.extend(arc_ops(arc_inc, cx, cy, rx, ry, strt, stp, 1.5, o));
        }
        if closed {
            if rough_closure {
                ops.extend(double_line(
                    cx,
                    cy,
                    cx + rx * strt.cos(),
                    cy + ry * strt.sin(),
                    o,
                    false,
                ));
                ops.extend(double_line(
                    cx,
                    cy,
                    cx + rx * stp.cos(),
                    cy + ry * stp.sin(),
                    o,
                    false,
                ));
            } else {
                ops.push(Op::LineTo([cx, cy]));
                ops.push(Op::LineTo([cx + rx * strt.cos(), cy + ry * strt.sin()]));
            }
        }
        OpSet {
            kind: SetKind::Path,
            ops,
        }
    };
    let outline = arc_set(true, &o);
    let mut sets = Vec::new();
    if closed && fills(&o) {
        if o.fill_style == "solid" {
            let fo = Options {
                disable_multi_stroke: true,
                ..o.clone()
            };
            let mut shape = arc_set(false, &fo);
            shape.kind = SetKind::FillPath;
            sets.push(shape);
        } else {
            // patternFillArc
            let (cx, cy) = (x, y);
            let mut rx = (width / 2.0).abs();
            let mut ry = (height / 2.0).abs();
            rx += offset_opt(rx * 0.01, &o, 1.0);
            ry += offset_opt(ry * 0.01, &o, 1.0);
            let (mut strt, mut stp) = (start, stop);
            while strt < 0.0 {
                strt += PI * 2.0;
                stp += PI * 2.0;
            }
            if stp - strt > PI * 2.0 {
                strt = 0.0;
                stp = PI * 2.0;
            }
            let increment = (stp - strt) / o.curve_step_count;
            let mut points = Vec::new();
            let mut angle = strt;
            while angle <= stp {
                points.push([cx + rx * angle.cos(), cy + ry * angle.sin()]);
                angle += increment;
            }
            points.push([cx + rx * stp.cos(), cy + ry * stp.sin()]);
            points.push([cx, cy]);
            sets.push(pattern_fill_polygons(&mut [points], &o));
        }
    }
    if has_stroke(&o) {
        sets.push(outline);
    }
    drawable("arc", sets, o)
}

/// rough.js `curve` (renderer): one or several point lists.
fn curve_set(lists: &[Vec<P>], o: &Options) -> OpSet {
    let Some(first) = lists.first() else {
        return OpSet {
            kind: SetKind::Path,
            ops: Vec::new(),
        };
    };
    let mut o1 = curve_with_offset(first, 1.0 * (1.0 + o.roughness * 0.2), o);
    let mut o2 = if o.disable_multi_stroke {
        Vec::new()
    } else {
        curve_with_offset(first, 1.5 * (1.0 + o.roughness * 0.22), &clone_alter_seed(o))
    };
    for points in &lists[1..] {
        if !points.is_empty() {
            let under = curve_with_offset(points, 1.0 * (1.0 + o.roughness * 0.2), o);
            let over = if o.disable_multi_stroke {
                Vec::new()
            } else {
                curve_with_offset(points, 1.5 * (1.0 + o.roughness * 0.22), &clone_alter_seed(o))
            };
            o1.extend(under.into_iter().filter(|op| !matches!(op, Op::Move(_))));
            o2.extend(over.into_iter().filter(|op| !matches!(op, Op::Move(_))));
        }
    }
    o1.extend(o2);
    OpSet {
        kind: SetKind::Path,
        ops: o1,
    }
}

fn merged_shape(ops: Vec<Op>) -> Vec<Op> {
    ops.into_iter()
        .enumerate()
        .filter(|(i, op)| *i == 0 || !matches!(op, Op::Move(_)))
        .map(|(_, op)| op)
        .collect()
}

fn has_fill(o: &Options) -> bool {
    fills(o) && !matches!(o.fill.as_deref(), Some("none" | "transparent"))
}

/// `generator.curve` over one or several point lists.
pub fn curve(lists: &[Vec<[f64; 2]>], o: Options) -> Drawable {
    let outline = curve_set(lists, &o);
    let mut sets = Vec::new();
    if fills(&o) && o.fill.as_deref() != Some("none") {
        if o.fill_style == "solid" {
            let fo = Options {
                disable_multi_stroke: true,
                roughness: if o.roughness != 0.0 {
                    o.roughness + o.fill_shape_roughness_gain
                } else {
                    0.0
                },
                ..o.clone()
            };
            let shape = curve_set(lists, &fo);
            sets.push(OpSet {
                kind: SetKind::FillPath,
                ops: merged_shape(shape.ops),
            });
        } else {
            let mut poly: Vec<P> = Vec::new();
            for points in lists {
                if points.len() < 3 {
                    poly.extend(points.iter().copied());
                } else if points.len() == 3 {
                    poly.extend(points_on_bezier_curves(
                        &curve_to_bezier(&[points[0], points[0], points[1], points[2]], 0.0),
                        10.0,
                        (1.0 + o.roughness) / 2.0,
                    ));
                } else {
                    poly.extend(points_on_bezier_curves(
                        &curve_to_bezier(points, 0.0),
                        10.0,
                        (1.0 + o.roughness) / 2.0,
                    ));
                }
            }
            if !poly.is_empty() {
                sets.push(pattern_fill_polygons(&mut [poly], &o));
            }
        }
    }
    if has_stroke(&o) {
        sets.push(outline);
    }
    drawable("curve", sets, o)
}

/// `generator.path`: SVG path data `d` drawn rough.
pub fn path(d: &str, o: Options) -> Drawable {
    if d.is_empty() {
        return drawable("path", Vec::new(), o);
    }
    // `.replace(/\n/g, ' ').replace(/(-\s)/g, '-')` (the third replace is a no-op).
    let d = d.replace('\n', " ");
    let d = replace_minus_space(&d);
    let fill = has_fill(&o);
    let stroke = has_stroke(&o);
    let simplified = matches!(o.simplification, Some(s) if s != 0.0 && s < 1.0);
    let distance = if simplified {
        4.0 - 4.0 * o.simplification.unwrap_or(1.0)
    } else {
        (1.0 + o.roughness) / 2.0
    };
    let mut sets = points_on_path(&d, 1.0, distance);
    let shape = svg_path_set(&d, &o);
    let mut out = Vec::new();
    if fill {
        if o.fill_style == "solid" {
            if sets.len() == 1 {
                let fo = Options {
                    disable_multi_stroke: true,
                    roughness: if o.roughness != 0.0 {
                        o.roughness + o.fill_shape_roughness_gain
                    } else {
                        0.0
                    },
                    ..o.clone()
                };
                let fs = svg_path_set(&d, &fo);
                out.push(OpSet {
                    kind: SetKind::FillPath,
                    ops: merged_shape(fs.ops),
                });
            } else {
                out.push(solid_fill_polygon(&sets, &o));
            }
        } else {
            out.push(pattern_fill_polygons(&mut sets, &o));
        }
    }
    if stroke {
        if simplified {
            for set in &sets {
                out.push(linear_path_set(set, false, &o));
            }
        } else {
            out.push(shape);
        }
    }
    drawable("path", out, o)
}

fn replace_minus_space(d: &str) -> String {
    let mut out = String::with_capacity(d.len());
    let mut chars = d.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '-' && chars.peek().is_some_and(|n| n.is_whitespace()) {
            chars.next();
        }
    }
    out
}

// ---------------------------------------------------------------- path-data-parser

#[derive(Debug, Clone, PartialEq)]
struct RawSeg {
    key: char,
    data: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Seg {
    M(P),
    L(P),
    C([f64; 6]),
    Z,
}

fn params_of(c: char) -> Option<usize> {
    Some(match c {
        'A' | 'a' => 7,
        'C' | 'c' => 6,
        'H' | 'h' | 'V' | 'v' => 1,
        'L' | 'l' | 'M' | 'm' | 'T' | 't' => 2,
        'Q' | 'q' | 'S' | 's' => 4,
        'Z' | 'z' => 0,
        _ => return None,
    })
}

enum Tok {
    Cmd(char),
    Num(f64),
}

/// `tokenize`: `None` where the data has something else in it.
fn tokenize(d: &str) -> Option<Vec<Tok>> {
    let b = d.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b',') {
            i += 1;
        } else if params_of(c as char).is_some() {
            out.push(Tok::Cmd(c as char));
            i += 1;
        } else {
            // ([-+]?[0-9]+(\.[0-9]*)?|[-+]?\.[0-9]+)([eE][-+]?[0-9]+)?
            let s = i;
            let mut j = i;
            if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                j += 1;
            }
            let ds = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > ds {
                if j < b.len() && b[j] == b'.' {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                }
            } else {
                if !(j < b.len() && b[j] == b'.') {
                    return None;
                }
                j += 1;
                let fs = j;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j == fs {
                    return None;
                }
            }
            if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                let mut k = j + 1;
                if k < b.len() && (b[k] == b'-' || b[k] == b'+') {
                    k += 1;
                }
                let es = k;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > es {
                    j = k;
                }
            }
            out.push(Tok::Num(d[s..j].parse().ok()?));
            i = j;
        }
    }
    Some(out)
}

/// `parsePath` (malformed data ends the path where rough.js would throw).
fn parse_path(d: &str) -> Vec<RawSeg> {
    let Some(toks) = tokenize(d) else {
        return Vec::new();
    };
    let mut segs = Vec::new();
    let mut mode: Option<char> = None;
    let mut i = 0;
    while i < toks.len() {
        let count;
        match (&toks[i], mode) {
            (Tok::Cmd(c), None) => {
                if *c != 'M' && *c != 'm' {
                    return parse_path(&format!("M0,0{d}"));
                }
                i += 1;
                count = 2;
                mode = Some(*c);
            }
            (Tok::Num(_), None) => return parse_path(&format!("M0,0{d}")),
            // (rough.js loops forever on numbers after a `Z`.)
            (Tok::Num(_), Some(m)) => match params_of(m) {
                Some(n) if n > 0 => count = n,
                _ => return segs,
            },
            (Tok::Cmd(c), Some(_)) => {
                i += 1;
                count = params_of(*c).unwrap_or(0);
                mode = Some(*c);
            }
        }
        let m = mode.unwrap_or('M');
        // `(index + paramsCount) < tokens.length`, counting the end token.
        if i + count > toks.len() {
            return segs;
        }
        let mut data = Vec::with_capacity(count);
        for t in &toks[i..i + count] {
            match t {
                Tok::Num(v) => data.push(*v),
                Tok::Cmd(_) => return segs,
            }
        }
        segs.push(RawSeg { key: m, data });
        i += count;
        if m == 'M' {
            mode = Some('L');
        }
        if m == 'm' {
            mode = Some('l');
        }
    }
    segs
}

/// `absolutize`.
fn absolutize(segs: &[RawSeg]) -> Vec<RawSeg> {
    let (mut cx, mut cy, mut subx, mut suby) = (0.0, 0.0, 0.0, 0.0);
    let mut out = Vec::new();
    for s in segs {
        let d = &s.data;
        let push = |out: &mut Vec<RawSeg>, key: char, data: Vec<f64>| out.push(RawSeg { key, data });
        let rel = |d: &[f64], cx: f64, cy: f64| -> Vec<f64> {
            d.iter()
                .enumerate()
                .map(|(i, v)| if i % 2 == 1 { v + cy } else { v + cx })
                .collect()
        };
        match s.key {
            'M' => {
                push(&mut out, 'M', d.clone());
                (cx, cy) = (d[0], d[1]);
                (subx, suby) = (d[0], d[1]);
            }
            'm' => {
                cx += d[0];
                cy += d[1];
                push(&mut out, 'M', vec![cx, cy]);
                (subx, suby) = (cx, cy);
            }
            'L' => {
                push(&mut out, 'L', d.clone());
                (cx, cy) = (d[0], d[1]);
            }
            'l' => {
                cx += d[0];
                cy += d[1];
                push(&mut out, 'L', vec![cx, cy]);
            }
            'C' => {
                push(&mut out, 'C', d.clone());
                (cx, cy) = (d[4], d[5]);
            }
            'c' => {
                let n = rel(d, cx, cy);
                (cx, cy) = (n[4], n[5]);
                push(&mut out, 'C', n);
            }
            'Q' => {
                push(&mut out, 'Q', d.clone());
                (cx, cy) = (d[2], d[3]);
            }
            'q' => {
                let n = rel(d, cx, cy);
                (cx, cy) = (n[2], n[3]);
                push(&mut out, 'Q', n);
            }
            'A' => {
                push(&mut out, 'A', d.clone());
                (cx, cy) = (d[5], d[6]);
            }
            'a' => {
                cx += d[5];
                cy += d[6];
                push(&mut out, 'A', vec![d[0], d[1], d[2], d[3], d[4], cx, cy]);
            }
            'H' => {
                push(&mut out, 'H', d.clone());
                cx = d[0];
            }
            'h' => {
                cx += d[0];
                push(&mut out, 'H', vec![cx]);
            }
            'V' => {
                push(&mut out, 'V', d.clone());
                cy = d[0];
            }
            'v' => {
                cy += d[0];
                push(&mut out, 'V', vec![cy]);
            }
            'S' => {
                push(&mut out, 'S', d.clone());
                (cx, cy) = (d[2], d[3]);
            }
            's' => {
                let n = rel(d, cx, cy);
                (cx, cy) = (n[2], n[3]);
                push(&mut out, 'S', n);
            }
            'T' => {
                push(&mut out, 'T', d.clone());
                (cx, cy) = (d[0], d[1]);
            }
            't' => {
                cx += d[0];
                cy += d[1];
                push(&mut out, 'T', vec![cx, cy]);
            }
            _ => {
                push(&mut out, 'Z', Vec::new());
                (cx, cy) = (subx, suby);
            }
        }
    }
    out
}

/// `normalize`: only M, L, C and Z left.
fn normalize(segs: &[RawSeg]) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut last = ' ';
    let (mut cx, mut cy, mut subx, mut suby, mut lcx, mut lcy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for s in segs {
        let d = &s.data;
        match s.key {
            'M' => {
                out.push(Seg::M([d[0], d[1]]));
                (cx, cy) = (d[0], d[1]);
                (subx, suby) = (d[0], d[1]);
            }
            'C' => {
                out.push(Seg::C([d[0], d[1], d[2], d[3], d[4], d[5]]));
                (cx, cy) = (d[4], d[5]);
                (lcx, lcy) = (d[2], d[3]);
            }
            'L' => {
                out.push(Seg::L([d[0], d[1]]));
                (cx, cy) = (d[0], d[1]);
            }
            'H' => {
                cx = d[0];
                out.push(Seg::L([cx, cy]));
            }
            'V' => {
                cy = d[0];
                out.push(Seg::L([cx, cy]));
            }
            'S' => {
                let (cx1, cy1) = if last == 'C' || last == 'S' {
                    (cx + (cx - lcx), cy + (cy - lcy))
                } else {
                    (cx, cy)
                };
                out.push(Seg::C([cx1, cy1, d[0], d[1], d[2], d[3]]));
                (lcx, lcy) = (d[0], d[1]);
                (cx, cy) = (d[2], d[3]);
            }
            'T' => {
                let (x, y) = (d[0], d[1]);
                let (x1, y1) = if last == 'Q' || last == 'T' {
                    (cx + (cx - lcx), cy + (cy - lcy))
                } else {
                    (cx, cy)
                };
                let cx1 = cx + 2.0 * (x1 - cx) / 3.0;
                let cy1 = cy + 2.0 * (y1 - cy) / 3.0;
                let cx2 = x + 2.0 * (x1 - x) / 3.0;
                let cy2 = y + 2.0 * (y1 - y) / 3.0;
                out.push(Seg::C([cx1, cy1, cx2, cy2, x, y]));
                (lcx, lcy) = (x1, y1);
                (cx, cy) = (x, y);
            }
            'Q' => {
                let (x1, y1, x, y) = (d[0], d[1], d[2], d[3]);
                let cx1 = cx + 2.0 * (x1 - cx) / 3.0;
                let cy1 = cy + 2.0 * (y1 - cy) / 3.0;
                let cx2 = x + 2.0 * (x1 - x) / 3.0;
                let cy2 = y + 2.0 * (y1 - y) / 3.0;
                out.push(Seg::C([cx1, cy1, cx2, cy2, x, y]));
                (lcx, lcy) = (x1, y1);
                (cx, cy) = (x, y);
            }
            'A' => {
                let (r1, r2) = (d[0].abs(), d[1].abs());
                let (angle, large, sweep, x, y) = (d[2], d[3], d[4], d[5], d[6]);
                if r1 == 0.0 || r2 == 0.0 {
                    out.push(Seg::C([cx, cy, x, y, x, y]));
                    (cx, cy) = (x, y);
                } else if cx != x || cy != y {
                    for c in arc_to_cubic(cx, cy, x, y, r1, r2, angle, large, sweep, None) {
                        out.push(Seg::C(c));
                    }
                    (cx, cy) = (x, y);
                }
            }
            _ => {
                out.push(Seg::Z);
                (cx, cy) = (subx, suby);
            }
        }
        last = s.key;
    }
    out
}

fn rotate(x: f64, y: f64, a: f64) -> P {
    [x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos()]
}

/// `arcToCubicCurves`. `rec` carries `[f1, f2, cx, cy]` on recursion, which
/// returns control points instead of finished curves.
#[allow(clippy::too_many_arguments)]
fn arc_to_cubic(
    mut x1: f64,
    mut y1: f64,
    mut x2: f64,
    mut y2: f64,
    mut r1: f64,
    mut r2: f64,
    angle: f64,
    large: f64,
    sweep: f64,
    rec: Option<[f64; 4]>,
) -> Vec<[f64; 6]> {
    let pts = arc_points(&mut x1, &mut y1, &mut x2, &mut y2, &mut r1, &mut r2, angle, large, sweep, rec);
    let a = (PI * angle) / 180.0;
    pts.chunks(3)
        .map(|c| {
            let p1 = rotate(c[0][0], c[0][1], a);
            let p2 = rotate(c[1][0], c[1][1], a);
            let p3 = rotate(c[2][0], c[2][1], a);
            [p1[0], p1[1], p2[0], p2[1], p3[0], p3[1]]
        })
        .collect()
}

/// JavaScript's `parseFloat(v.toFixed(9))`.
fn fixed9(v: f64) -> f64 {
    format!("{v:.9}").parse().unwrap_or(v)
}

#[allow(clippy::too_many_arguments)]
fn arc_points(
    x1: &mut f64,
    y1: &mut f64,
    x2: &mut f64,
    y2: &mut f64,
    r1: &mut f64,
    r2: &mut f64,
    angle: f64,
    large: f64,
    sweep: f64,
    rec: Option<[f64; 4]>,
) -> Vec<P> {
    let ar = (PI * angle) / 180.0;
    let (mut f1, mut f2, cx, cy);
    if let Some([a, b, c, d]) = rec {
        (f1, f2, cx, cy) = (a, b, c, d);
    } else {
        [*x1, *y1] = rotate(*x1, *y1, -ar);
        [*x2, *y2] = rotate(*x2, *y2, -ar);
        let x = (*x1 - *x2) / 2.0;
        let y = (*y1 - *y2) / 2.0;
        let mut h = (x * x) / (*r1 * *r1) + (y * y) / (*r2 * *r2);
        if h > 1.0 {
            h = h.sqrt();
            *r1 *= h;
            *r2 *= h;
        }
        let sign = if large == sweep { -1.0 } else { 1.0 };
        let r1p = *r1 * *r1;
        let r2p = *r2 * *r2;
        let left = r1p * r2p - r1p * y * y - r2p * x * x;
        let right = r1p * y * y + r2p * x * x;
        let k = sign * (left / right).abs().sqrt();
        cx = k * *r1 * y / *r2 + (*x1 + *x2) / 2.0;
        cy = k * -*r2 * x / *r1 + (*y1 + *y2) / 2.0;
        f1 = fixed9((*y1 - cy) / *r2).asin();
        f2 = fixed9((*y2 - cy) / *r2).asin();
        if *x1 < cx {
            f1 = PI - f1;
        }
        if *x2 < cx {
            f2 = PI - f2;
        }
        if f1 < 0.0 {
            f1 += PI * 2.0;
        }
        if f2 < 0.0 {
            f2 += PI * 2.0;
        }
        if sweep != 0.0 && f1 > f2 {
            f1 -= PI * 2.0;
        }
        if sweep == 0.0 && f2 > f1 {
            f2 -= PI * 2.0;
        }
    }
    let mut params: Vec<P> = Vec::new();
    let mut df = f2 - f1;
    if df.abs() > (PI * 120.0 / 180.0) {
        let (f2old, x2old, y2old) = (f2, *x2, *y2);
        if sweep != 0.0 && f2 > f1 {
            f2 = f1 + (PI * 120.0 / 180.0) * 1.0;
        } else {
            f2 = f1 + (PI * 120.0 / 180.0) * -1.0;
        }
        *x2 = cx + *r1 * f2.cos();
        *y2 = cy + *r2 * f2.sin();
        let (mut a, mut b, mut c, mut d, mut e, mut f) = (*x2, *y2, x2old, y2old, *r1, *r2);
        params = arc_points(
            &mut a,
            &mut b,
            &mut c,
            &mut d,
            &mut e,
            &mut f,
            angle,
            0.0,
            sweep,
            Some([f2, f2old, cx, cy]),
        );
    }
    df = f2 - f1;
    let (c1, s1, c2, s2) = (f1.cos(), f1.sin(), f2.cos(), f2.sin());
    let t = (df / 4.0).tan();
    let hx = 4.0 / 3.0 * *r1 * t;
    let hy = 4.0 / 3.0 * *r2 * t;
    let m1 = [*x1, *y1];
    let mut m2 = [*x1 + hx * s1, *y1 - hy * c1];
    let m3 = [*x2 + hx * s2, *y2 - hy * c2];
    let m4 = [*x2, *y2];
    m2[0] = 2.0 * m1[0] - m2[0];
    m2[1] = 2.0 * m1[1] - m2[1];
    let mut out = vec![m2, m3, m4];
    out.extend(params);
    out
}

// ---------------------------------------------------------------- points-on-curve / points-on-path

fn dist_sq(a: P, b: P) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn dist_to_segment_sq(p: P, v: P, w: P) -> f64 {
    let l2 = dist_sq(v, w);
    if l2 == 0.0 {
        return dist_sq(p, v);
    }
    let mut t = ((p[0] - v[0]) * (w[0] - v[0]) + (p[1] - v[1]) * (w[1] - v[1])) / l2;
    t = t.min(1.0).max(0.0);
    dist_sq(p, lerp(v, w, t))
}

fn flatness(p: &[P], o: usize) -> f64 {
    let (p1, p2, p3, p4) = (p[o], p[o + 1], p[o + 2], p[o + 3]);
    let mut ux = 3.0 * p2[0] - 2.0 * p1[0] - p4[0];
    ux *= ux;
    let mut uy = 3.0 * p2[1] - 2.0 * p1[1] - p4[1];
    uy *= uy;
    let mut vx = 3.0 * p3[0] - 2.0 * p4[0] - p1[0];
    vx *= vx;
    let mut vy = 3.0 * p3[1] - 2.0 * p4[1] - p1[1];
    vy *= vy;
    if ux < vx {
        ux = vx;
    }
    if uy < vy {
        uy = vy;
    }
    ux + uy
}

fn bezier_points_split(p: &[P], o: usize, tol: f64, out: &mut Vec<P>) {
    if flatness(p, o) < tol {
        let p0 = p[o];
        if let Some(&last) = out.last() {
            if dist_sq(last, p0).sqrt() > 1.0 {
                out.push(p0);
            }
        } else {
            out.push(p0);
        }
        out.push(p[o + 3]);
    } else {
        let t = 0.5;
        let (p1, p2, p3, p4) = (p[o], p[o + 1], p[o + 2], p[o + 3]);
        let q1 = lerp(p1, p2, t);
        let q2 = lerp(p2, p3, t);
        let q3 = lerp(p3, p4, t);
        let r1 = lerp(q1, q2, t);
        let r2 = lerp(q2, q3, t);
        let red = lerp(r1, r2, t);
        bezier_points_split(&[p1, q1, r1, red], 0, tol, out);
        bezier_points_split(&[red, r2, q3, p4], 0, tol, out);
    }
}

fn simplify_points(p: &[P], start: usize, end: usize, eps: f64, out: &mut Vec<P>) {
    let s = p[start];
    let e = p[end - 1];
    let mut max_sq = 0.0;
    let mut max_i = 1;
    for (i, &q) in p.iter().enumerate().take(end - 1).skip(start + 1) {
        let d = dist_to_segment_sq(q, s, e);
        if d > max_sq {
            max_sq = d;
            max_i = i;
        }
    }
    if max_sq.sqrt() > eps {
        simplify_points(p, start, max_i + 1, eps, out);
        simplify_points(p, max_i, end, eps, out);
    } else {
        if out.is_empty() {
            out.push(s);
        }
        out.push(e);
    }
}

fn simplify(p: &[P], eps: f64) -> Vec<P> {
    let mut out = Vec::new();
    if !p.is_empty() {
        simplify_points(p, 0, p.len(), eps, &mut out);
    }
    out
}

fn points_on_bezier_curves(p: &[P], tol: f64, distance: f64) -> Vec<P> {
    let mut pts = Vec::new();
    let n = (p.len().saturating_sub(1)) / 3;
    for i in 0..n {
        bezier_points_split(p, i * 3, tol, &mut pts);
    }
    if distance > 0.0 && !pts.is_empty() {
        let mut out = Vec::new();
        simplify_points(&pts, 0, pts.len(), distance, &mut out);
        return out;
    }
    pts
}

fn curve_to_bezier(input: &[P], tightness: f64) -> Vec<P> {
    let len = input.len();
    let mut out = Vec::new();
    if len < 3 {
        return out;
    }
    if len == 3 {
        out.extend([input[0], input[1], input[2], input[2]]);
    } else {
        let mut points = vec![input[0], input[0]];
        for i in 1..len {
            points.push(input[i]);
            if i == len - 1 {
                points.push(input[i]);
            }
        }
        let s = 1.0 - tightness;
        out.push(points[0]);
        let mut i = 1;
        while i + 2 < points.len() {
            let v = points[i];
            out.push([
                v[0] + (s * points[i + 1][0] - s * points[i - 1][0]) / 6.0,
                v[1] + (s * points[i + 1][1] - s * points[i - 1][1]) / 6.0,
            ]);
            out.push([
                points[i + 1][0] + (s * points[i][0] - s * points[i + 2][0]) / 6.0,
                points[i + 1][1] + (s * points[i][1] - s * points[i + 2][1]) / 6.0,
            ]);
            out.push(points[i + 1]);
            i += 1;
        }
    }
    out
}

/// `pointsOnPath`: the path flattened to point lists, one per subpath.
fn points_on_path(d: &str, tol: f64, distance: f64) -> Vec<Vec<P>> {
    let segs = normalize(&absolutize(&parse_path(d)));
    let mut sets: Vec<Vec<P>> = Vec::new();
    let mut cur: Vec<P> = Vec::new();
    let mut start = [0.0, 0.0];
    let mut pending: Vec<P> = Vec::new();
    fn flush_curve(pending: &mut Vec<P>, cur: &mut Vec<P>, tol: f64) {
        if pending.len() >= 4 {
            cur.extend(points_on_bezier_curves(pending, tol, 0.0));
        }
        pending.clear();
    }
    for s in segs {
        match s {
            Seg::M(p) => {
                flush_curve(&mut pending, &mut cur, tol);
                if !cur.is_empty() {
                    sets.push(std::mem::take(&mut cur));
                }
                start = p;
                cur.push(start);
            }
            Seg::L(p) => {
                flush_curve(&mut pending, &mut cur, tol);
                cur.push(p);
            }
            Seg::C(c) => {
                if pending.is_empty() {
                    let last = cur.last().copied().unwrap_or(start);
                    pending.push(last);
                }
                pending.push([c[0], c[1]]);
                pending.push([c[2], c[3]]);
                pending.push([c[4], c[5]]);
            }
            Seg::Z => {
                flush_curve(&mut pending, &mut cur, tol);
                cur.push(start);
            }
        }
    }
    flush_curve(&mut pending, &mut cur, tol);
    if !cur.is_empty() {
        sets.push(cur);
    }
    if distance == 0.0 {
        return sets;
    }
    sets.iter()
        .map(|s| simplify(s, distance))
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------- output

/// rough.js's `opsToPath` text (no fixed decimals), for tests and debugging.
pub fn ops_to_path(ops: &[Op]) -> String {
    let mut s = String::new();
    for op in ops {
        match op {
            Op::Move(d) => s.push_str(&format!("M{} {} ", d[0], d[1])),
            Op::BCurveTo(d) => s.push_str(&format!(
                "C{} {}, {} {}, {} {} ",
                d[0], d[1], d[2], d[3], d[4], d[5]
            )),
            Op::LineTo(d) => s.push_str(&format!("L{} {} ", d[0], d[1])),
        }
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `userNodeOverrides`-like options (rough.js 4.6.6 values pinned below).
    fn node_opts(seed: f64) -> Options {
        Options {
            roughness: 0.7,
            fill: Some("#fff".into()),
            fill_weight: 1.5,
            hachure_gap: 1.5,
            stroke: "#333".into(),
            stroke_width: 1.3,
            seed,
            ..Options::default()
        }
    }

    fn close(a: &[f64], b: &[f64]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }

    fn data(op: &Op) -> Vec<f64> {
        match op {
            Op::Move(d) | Op::LineTo(d) => d.to_vec(),
            Op::BCurveTo(d) => d.to_vec(),
        }
    }

    #[test]
    fn park_miller_stream() {
        let s = Cell::new(7);
        let a = Random::next(&s);
        assert_eq!(s.get(), 7 * 48271);
        assert!((a - f64::from(7 * 48271) / 2_147_483_648.0).abs() < 1e-18);
        // A zero seed draws like the fallback seed.
        assert_eq!(Random::start(0.0), FALLBACK_SEED);
        assert_eq!(Random::start(-5.0), -5);
    }

    #[test]
    fn rectangle_matches_roughjs() {
        let d = rectangle(-50.0, -20.0, 100.0, 40.0, node_opts(7.0));
        assert_eq!(d.sets.len(), 2);
        assert_eq!(d.sets[0].kind, SetKind::FillSketch);
        assert_eq!(d.sets[0].ops.len(), 204);
        assert_eq!(d.sets[1].ops.len(), 16);
        close(&data(&d.sets[1].ops[0]), &[-51.39940910618752, -20.87696477789432]);
        close(
            &data(&d.sets[1].ops[1]),
            &[
                -28.76364682558924,
                -19.493477344606074,
                -9.938312210701406,
                -19.670824903342872,
                48.91138618309051,
                -20.877556038089097,
            ],
        );
        close(&data(&d.sets[0].ops[0]), &[-50.19107073324646, -19.780198264928703]);
    }

    #[test]
    fn rounded_path_matches_roughjs() {
        let d = path(
            "M -50 -20 H 50 A 5 5 0 0 1 55 -15 V 15 A 5 5 0 0 1 50 20 H -50 A 5 5 0 0 1 -55 15 V -15 A 5 5 0 0 1 -50 -20 Z",
            node_opts(7.0),
        );
        assert_eq!(d.sets[0].ops.len(), 212);
        assert_eq!(d.sets[1].ops.len(), 36);
        close(&data(&d.sets[1].ops[35]), &[-50.0, -20.0, -50.0, -20.0, -50.0, -20.0]);
    }

    #[test]
    fn circle_matches_roughjs() {
        let d = circle(0.0, 0.0, 30.0, node_opts(7.0));
        assert_eq!(d.sets[0].ops.len(), 60);
        assert_eq!(d.sets[1].ops.len(), 22);
        close(&data(&d.sets[1].ops[0]), &[-3.1204928595096475, -14.777440675258463]);
        close(
            &data(&d.sets[1].ops[1]),
            &[
                -0.4782897023438637,
                -15.715511140893746,
                3.320093335277631,
                -15.384341309120442,
                6.125773607211574,
                -13.840119249486483,
            ],
        );
    }

    #[test]
    fn edge_path_matches_roughjs() {
        let o = Options {
            roughness: 0.3,
            seed: 42.0,
            ..Options::default()
        };
        let d = path("M0,0 L40,0", o);
        let ops: Vec<Vec<f64>> = d.sets[0].ops.iter().map(data).collect();
        close(&ops[0], &[-0.5984805587679147, -0.4550522860139608]);
        close(
            &ops[3],
            &[
                11.708280347473922,
                -0.14323428522795437,
                23.93089993614704,
                -0.12110922586172818,
                40.187283989973366,
                0.18548000436276196,
            ],
        );
    }

    #[test]
    fn path_data_parser_quirks() {
        // A path not starting with a move gets `M0,0` in front.
        assert_eq!(parse_path("L5,5")[0].key, 'M');
        // Numbers after `Z` end the path instead of looping.
        assert_eq!(parse_path("M0,0 L1,1 Z 5 5").len(), 3);
        assert_eq!(replace_minus_space("M- 5,3"), "M-5,3");
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
    }
}
