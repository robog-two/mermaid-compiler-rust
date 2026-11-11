//! Plain geometry: points, rectangles, curves and boundary clipping.

/// A 2-D point in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
    pub fn plus(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
    pub fn minus(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
    pub fn scale(self, k: f64) -> Point {
        Point::new(self.x * k, self.y * k)
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: Point) -> f64 {
        self.minus(o).len()
    }
    /// Unit vector (zero vector stays zero).
    pub fn norm(self) -> Point {
        let l = self.len();
        if l < 1e-12 {
            Point::new(0.0, 0.0)
        } else {
            self.scale(1.0 / l)
        }
    }
    pub fn lerp(self, o: Point, t: f64) -> Point {
        Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
    /// This point rotated by `deg` degrees (clockwise on screen, like SVG
    /// `rotate(deg cx cy)`) about `about`.
    pub fn rotated(self, about: Point, deg: f64) -> Point {
        let (s, c) = deg.to_radians().sin_cos();
        let d = self.minus(about);
        Point::new(about.x + d.x * c - d.y * s, about.y + d.x * s + d.y * c)
    }
}

/// Axis-aligned rectangle (`x`,`y` = top-left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }
    pub fn centered(c: Point, w: f64, h: f64) -> Self {
        Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }
    pub fn pad(&self, p: f64) -> Rect {
        Rect::new(self.x - p, self.y - p, self.w + 2.0 * p, self.h + 2.0 * p)
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// Accumulates a bounding box.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bounds(Option<Rect>);

impl Bounds {
    pub fn add_rect(&mut self, r: Rect) {
        self.0 = Some(match self.0 {
            Some(b) => b.union(&r),
            None => r,
        });
    }
    pub fn add_point(&mut self, p: Point) {
        self.add_rect(Rect::new(p.x, p.y, 0.0, 0.0));
    }
    /// Add the corners of `r` rotated by `deg` degrees about `about`.
    pub fn add_rotated_rect(&mut self, r: Rect, about: Point, deg: f64) {
        for p in [
            Point::new(r.x, r.y),
            Point::new(r.right(), r.y),
            Point::new(r.x, r.bottom()),
            Point::new(r.right(), r.bottom()),
        ] {
            self.add_point(p.rotated(about, deg));
        }
    }
    pub fn rect(&self) -> Rect {
        self.0.unwrap_or_default()
    }
}

/// Path commands understood by every output backend. Arcs are pre-converted to cubics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// Closed polygon path.
pub fn polygon(points: &[Point]) -> Vec<PathCmd> {
    let mut cmds = Vec::with_capacity(points.len() + 1);
    for (i, p) in points.iter().enumerate() {
        cmds.push(if i == 0 {
            PathCmd::MoveTo(*p)
        } else {
            PathCmd::LineTo(*p)
        });
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// Open polyline path.
pub fn polyline(points: &[Point]) -> Vec<PathCmd> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i == 0 {
                PathCmd::MoveTo(*p)
            } else {
                PathCmd::LineTo(*p)
            }
        })
        .collect()
}

/// Uniform cubic B-spline through `points` (equivalent to d3's `curveBasis`,
/// mermaid's default edge curve). Endpoints are interpolated exactly.
pub fn basis_curve(points: &[Point]) -> Vec<PathCmd> {
    if points.len() < 3 {
        return polyline(points);
    }
    let mut out = vec![PathCmd::MoveTo(points[0])];
    let (mut x0, mut y0, mut x1, mut y1) = (points[0].x, points[0].y, points[0].x, points[0].y);
    let bezier = |out: &mut Vec<PathCmd>, x0: f64, y0: f64, x1: f64, y1: f64, x: f64, y: f64| {
        out.push(PathCmd::CubicTo(
            Point::new((2.0 * x0 + x1) / 3.0, (2.0 * y0 + y1) / 3.0),
            Point::new((x0 + 2.0 * x1) / 3.0, (y0 + 2.0 * y1) / 3.0),
            Point::new((x0 + 4.0 * x1 + x) / 6.0, (y0 + 4.0 * y1 + y) / 6.0),
        ));
    };
    for (i, p) in points.iter().enumerate().skip(1) {
        if i == 1 {
            out.push(PathCmd::LineTo(Point::new(
                (5.0 * x0 + p.x) / 6.0,
                (5.0 * y0 + p.y) / 6.0,
            )));
        } else {
            bezier(&mut out, x0, y0, x1, y1, p.x, p.y);
        }
        (x0, y0, x1, y1) = (x1, y1, p.x, p.y);
    }
    bezier(&mut out, x0, y0, x1, y1, x1, y1);
    out.push(PathCmd::LineTo(Point::new(x1, y1)));
    out
}

/// Cubic segments approximating a circular arc (angles in radians, SVG y-down,
/// 0 = east, increasing clockwise on screen). Does not emit the initial MoveTo.
pub fn arc_cubics(c: Point, r: f64, a0: f64, a1: f64) -> Vec<PathCmd> {
    elliptic_arc_cubics(c, r, r, a0, a1)
}

/// Axis-aligned elliptical arc as cubic segments (≤ 90° each).
pub fn elliptic_arc_cubics(c: Point, rx: f64, ry: f64, a0: f64, a1: f64) -> Vec<PathCmd> {
    let total = a1 - a0;
    let n = ((total.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).max(1);
    let step = total / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |a: f64| Point::new(c.x + rx * a.cos(), c.y + ry * a.sin());
    let d = |a: f64| Point::new(-rx * a.sin(), ry * a.cos());
    (0..n)
        .map(|i| {
            let s = a0 + step * i as f64;
            let e = s + step;
            PathCmd::CubicTo(at(s).plus(d(s).scale(k)), at(e).minus(d(e).scale(k)), at(e))
        })
        .collect()
}

/// Full ellipse as a closed path.
pub fn ellipse(c: Point, rx: f64, ry: f64) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(Point::new(c.x + rx, c.y))];
    cmds.extend(elliptic_arc_cubics(c, rx, ry, 0.0, std::f64::consts::TAU));
    cmds.push(PathCmd::Close);
    cmds
}

/// Where the ray from the centre of `r` toward `toward` exits the rectangle.
pub fn clip_rect(r: &Rect, toward: Point) -> Point {
    let c = r.center();
    let d = toward.minus(c);
    if d.x.abs() < 1e-9 && d.y.abs() < 1e-9 {
        return c;
    }
    let sx = if d.x.abs() > 1e-9 {
        (r.w / 2.0) / d.x.abs()
    } else {
        f64::INFINITY
    };
    let sy = if d.y.abs() > 1e-9 {
        (r.h / 2.0) / d.y.abs()
    } else {
        f64::INFINITY
    };
    c.plus(d.scale(sx.min(sy)))
}

/// Exit point of the ray from `c` toward `toward` through an ellipse.
pub fn clip_ellipse(c: Point, rx: f64, ry: f64, toward: Point) -> Point {
    let d = toward.minus(c);
    let denom = ((d.x / rx).powi(2) + (d.y / ry).powi(2)).sqrt();
    if denom < 1e-12 {
        c
    } else {
        c.plus(d.scale(1.0 / denom))
    }
}

/// Exit point of the ray from `c` toward `toward` through a convex polygon around `c`.
pub fn clip_polygon(c: Point, poly: &[Point], toward: Point) -> Point {
    let d = toward.minus(c);
    if d.len() < 1e-12 {
        return c;
    }
    let mut best: Option<f64> = None;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let e = b.minus(a);
        let den = d.x * e.y - d.y * e.x;
        if den.abs() < 1e-12 {
            continue;
        }
        let ac = a.minus(c);
        let t = (ac.x * e.y - ac.y * e.x) / den;
        let u = (ac.x * d.y - ac.y * d.x) / den;
        if t > 0.0 && (-1e-9..=1.0 + 1e-9).contains(&u) {
            best = Some(best.map_or(t, |bt: f64| bt.min(t)));
        }
    }
    best.map_or(c, |t| c.plus(d.scale(t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_clipping_hits_border() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let p = clip_rect(&r, Point::new(50.0, 500.0));
        assert!((p.y - 50.0).abs() < 1e-9 && (p.x - 50.0).abs() < 1e-9);
    }

    #[test]
    fn polygon_clipping_diamond() {
        let c = Point::new(0.0, 0.0);
        let poly = [
            Point::new(0.0, -10.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0),
            Point::new(-10.0, 0.0),
        ];
        let p = clip_polygon(c, &poly, Point::new(100.0, 0.0));
        assert!((p.x - 10.0).abs() < 1e-9);
    }

    #[test]
    fn basis_curve_endpoints() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(20.0, 0.0),
        ];
        let c = basis_curve(&pts);
        assert_eq!(c.first(), Some(&PathCmd::MoveTo(pts[0])));
        assert_eq!(c.last(), Some(&PathCmd::LineTo(pts[2])));
    }
}
