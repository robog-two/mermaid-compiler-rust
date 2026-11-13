//! Rectangle geometry shared by the force-directed layouts, transcribed from
//! layout-base's `RectangleD` and `IGeometry` (identical in layout-base 1.0.2
//! and 2.0.1). The floating-point operations keep upstream's order so results
//! match bit for bit.

/// `RectangleD`: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectD {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectD {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center_x(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }
    /// `RectangleD.intersects`: touching rectangles intersect.
    pub fn intersects(&self, a: &RectD) -> bool {
        !(self.right() < a.x || self.bottom() < a.y || a.right() < self.x || a.bottom() < self.y)
    }
}

/// `IMath.sign`.
pub fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// `IGeometry.calcSeparationAmount`: how far apart overlapping `a` and `b` must
/// move, returned as the `[x, y]` amount for `a` (upstream's `overlapAmount`).
pub fn separation_amount(a: &RectD, b: &RectD, buffer: f64) -> [f64; 2] {
    let dir_x = if a.center_x() < b.center_x() {
        -1.0
    } else {
        1.0
    };
    let dir_y = if a.center_y() < b.center_y() {
        -1.0
    } else {
        1.0
    };
    let mut o0 = a.right().min(b.right()) - a.x.max(b.x);
    let mut o1 = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if a.x <= b.x && a.right() >= b.right() {
        o0 += (b.x - a.x).min(a.right() - b.right());
    } else if b.x <= a.x && b.right() >= a.right() {
        o0 += (a.x - b.x).min(b.right() - a.right());
    }
    if a.y <= b.y && a.bottom() >= b.bottom() {
        o1 += (b.y - a.y).min(a.bottom() - b.bottom());
    } else if b.y <= a.y && b.bottom() >= a.bottom() {
        o1 += (a.y - b.y).min(b.bottom() - a.bottom());
    }
    let mut slope = ((b.center_y() - a.center_y()) / (b.center_x() - a.center_x())).abs();
    if b.center_y() == a.center_y() && b.center_x() == a.center_x() {
        slope = 1.0;
    }
    let mut move_y = slope * o0;
    let mut move_x = o1 / slope;
    if o0 < move_x {
        move_x = o0;
    } else {
        move_y = o1;
    }
    [
        -dir_x * (move_x / 2.0 + buffer),
        -dir_y * (move_y / 2.0 + buffer),
    ]
}

/// `IGeometry.getCardinalDirection`.
fn cardinal(slope: f64, slope_prime: f64, line: u8) -> u8 {
    if slope > slope_prime {
        line
    } else {
        1 + line % 4
    }
}

/// `IGeometry.getIntersection2`: the points where the segment between the two
/// centres leaves `a` and enters `b`, as `[ax, ay, bx, by]`, and whether the
/// rectangles intersect (then the centres are returned).
pub fn clip_points(a: &RectD, b: &RectD) -> ([f64; 4], bool) {
    let mut r = [0.0; 4];
    let (p1x, p1y) = (a.center_x(), a.center_y());
    let (p2x, p2y) = (b.center_x(), b.center_y());
    if a.intersects(b) {
        return ([p1x, p1y, p2x, p2y], true);
    }
    let (top_left_ax, top_left_ay) = (a.x, a.y);
    let top_right_ax = a.right();
    let (bottom_left_ax, bottom_left_ay) = (a.x, a.bottom());
    let bottom_right_ax = a.right();
    let (half_wa, half_ha) = (a.w / 2.0, a.h / 2.0);
    let (top_left_bx, top_left_by) = (b.x, b.y);
    let top_right_bx = b.right();
    let (bottom_left_bx, bottom_left_by) = (b.x, b.bottom());
    let bottom_right_bx = b.right();
    let (half_wb, half_hb) = (b.w / 2.0, b.h / 2.0);
    if p1x == p2x {
        if p1y > p2y {
            return ([p1x, top_left_ay, p2x, bottom_left_by], false);
        } else if p1y < p2y {
            return ([p1x, bottom_left_ay, p2x, top_left_by], false);
        }
        return (r, false);
    }
    if p1y == p2y {
        if p1x > p2x {
            return ([top_left_ax, p1y, top_right_bx, p2y], false);
        } else if p1x < p2x {
            return ([top_right_ax, p1y, top_left_bx, p2y], false);
        }
        return (r, false);
    }
    let slope_a = a.h / a.w;
    let slope_b = b.h / b.w;
    let slope_prime = (p2y - p1y) / (p2x - p1x);
    let mut found_a = false;
    let mut found_b = false;
    if -slope_a == slope_prime {
        if p1x > p2x {
            r[0] = bottom_left_ax;
            r[1] = bottom_left_ay;
        } else {
            r[0] = top_right_ax;
            r[1] = top_left_ay;
        }
        found_a = true;
    } else if slope_a == slope_prime {
        if p1x > p2x {
            r[0] = top_left_ax;
            r[1] = top_left_ay;
        } else {
            r[0] = bottom_right_ax;
            r[1] = bottom_left_ay;
        }
        found_a = true;
    }
    if -slope_b == slope_prime {
        if p2x > p1x {
            r[2] = bottom_left_bx;
            r[3] = bottom_left_by;
        } else {
            r[2] = top_right_bx;
            r[3] = top_left_by;
        }
        found_b = true;
    } else if slope_b == slope_prime {
        if p2x > p1x {
            r[2] = top_left_bx;
            r[3] = top_left_by;
        } else {
            r[2] = bottom_right_bx;
            r[3] = bottom_left_by;
        }
        found_b = true;
    }
    if found_a && found_b {
        return (r, false);
    }
    let (dir_a, dir_b) = if p1x > p2x {
        if p1y > p2y {
            (
                cardinal(slope_a, slope_prime, 4),
                cardinal(slope_b, slope_prime, 2),
            )
        } else {
            (
                cardinal(-slope_a, slope_prime, 3),
                cardinal(-slope_b, slope_prime, 1),
            )
        }
    } else if p1y > p2y {
        (
            cardinal(-slope_a, slope_prime, 1),
            cardinal(-slope_b, slope_prime, 3),
        )
    } else {
        (
            cardinal(slope_a, slope_prime, 2),
            cardinal(slope_b, slope_prime, 4),
        )
    };
    if !found_a {
        match dir_a {
            1 => {
                r[0] = p1x + -half_ha / slope_prime;
                r[1] = top_left_ay;
            }
            2 => {
                r[0] = bottom_right_ax;
                r[1] = p1y + half_wa * slope_prime;
            }
            3 => {
                r[0] = p1x + half_ha / slope_prime;
                r[1] = bottom_left_ay;
            }
            _ => {
                r[0] = bottom_left_ax;
                r[1] = p1y + -half_wa * slope_prime;
            }
        }
    }
    if !found_b {
        match dir_b {
            1 => {
                r[2] = p2x + -half_hb / slope_prime;
                r[3] = top_left_by;
            }
            2 => {
                r[2] = bottom_right_bx;
                r[3] = p2y + half_wb * slope_prime;
            }
            3 => {
                r[2] = p2x + half_hb / slope_prime;
                r[3] = bottom_left_by;
            }
            _ => {
                r[2] = bottom_left_bx;
                r[3] = p2y + -half_wb * slope_prime;
            }
        }
    }
    (r, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_points_leave_and_enter_the_boxes() {
        let a = RectD {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
        };
        let b = RectD {
            x: 100.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
        };
        assert_eq!(clip_points(&a, &b), ([20.0, 5.0, 100.0, 5.0], false));
        // Diagonal: a's right side, b's left side.
        let b = RectD { y: 40.0, ..b };
        let (r, overlap) = clip_points(&a, &b);
        assert!(!overlap);
        assert_eq!((r[0], r[2]), (20.0, 100.0));
        assert!((r[1] - (5.0 + 10.0 * 0.4)).abs() < 1e-12);
        // Overlapping boxes report their centres.
        let c = RectD { x: 10.0, ..a };
        assert_eq!(clip_points(&a, &c), ([10.0, 5.0, 20.0, 5.0], true));
    }

    #[test]
    fn separation_pushes_apart_along_the_cheaper_axis() {
        let a = RectD {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 20.0,
        };
        let b = RectD { x: 15.0, ..a };
        // Overlap 5 in x; same centre y → slope 0 → moveByX = 20/0 = ∞ → x wins
        // and y only gets the buffer.
        let o = separation_amount(&a, &b, 25.0);
        assert_eq!(o, [27.5, -25.0]);
    }
}
