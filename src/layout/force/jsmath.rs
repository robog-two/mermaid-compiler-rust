//! `Math.sin`, `Math.cos`, `Math.log` and `Math.pow` as Chromium computes them.
//!
//! V8 takes sin, cos and log from LLVM's libc, whose results are correctly
//! rounded, while the platform libm behind `f64::sin` & co. is only faithful
//! (glibc is off by one ulp for about one input in a thousand) and differs
//! between platforms. The force-directed layouts amplify such a last-bit
//! difference into a visibly different drawing, so they use these instead:
//! each evaluates the function in double-double arithmetic (~106 bits) and
//! rounds once, which is correctly rounded except for inputs closer to a
//! rounding boundary than 2^-100 relative. `Math.pow` is the exception (see
//! [`pow`]).

/// Double-double: `hi + lo` with `|lo| <= ulp(hi) / 2`.
#[derive(Debug, Clone, Copy)]
struct Dd {
    hi: f64,
    lo: f64,
}

fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    Dd {
        hi: s,
        lo: (a - (s - bb)) + (b - bb),
    }
}

fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd {
        hi: s,
        lo: b - (s - a),
    }
}

/// Dekker's split of `a` into two 26-bit halves.
fn split(a: f64) -> (f64, f64) {
    let c = 134_217_729.0 * a;
    let hi = c - (c - a);
    (hi, a - hi)
}

fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    Dd {
        hi: p,
        lo: ((ah * bh - p) + ah * bl + al * bh) + al * bl,
    }
}

impl Dd {
    const fn new(hi: f64, lo: f64) -> Dd {
        Dd { hi, lo }
    }
    fn from(a: f64) -> Dd {
        Dd { hi: a, lo: 0.0 }
    }
    fn add(self, o: Dd) -> Dd {
        let s = two_sum(self.hi, o.hi);
        let t = two_sum(self.lo, o.lo);
        let s = quick_two_sum(s.hi, s.lo + t.hi);
        quick_two_sum(s.hi, s.lo + t.lo)
    }
    fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }
    fn sub(self, o: Dd) -> Dd {
        self.add(o.neg())
    }
    fn mul(self, o: Dd) -> Dd {
        let p = two_prod(self.hi, o.hi);
        quick_two_sum(p.hi, p.lo + (self.hi * o.lo + self.lo * o.hi))
    }
    fn mul_f(self, b: f64) -> Dd {
        let p = two_prod(self.hi, b);
        quick_two_sum(p.hi, p.lo + self.lo * b)
    }
    fn div(self, o: Dd) -> Dd {
        let q1 = self.hi / o.hi;
        let r = self.sub(o.mul_f(q1));
        let q2 = r.hi / o.hi;
        let r = r.sub(o.mul_f(q2));
        let q3 = r.hi / o.hi;
        quick_two_sum(q1, q2).add(Dd::from(q3))
    }
    fn div_f(self, b: f64) -> Dd {
        self.div(Dd::from(b))
    }
    /// Round to the nearest double.
    fn round(self) -> f64 {
        self.hi + self.lo
    }
}

/// π/2 as a sum of four doubles.
const PIO2: [f64; 4] = [
    std::f64::consts::FRAC_PI_2,
    6.123233995736766e-17,
    -1.4973849048591698e-33,
    5.562271104316826e-50,
];
/// ln 2 as a sum of three doubles.
const LN2: [f64; 3] = [
    std::f64::consts::LN_2,
    2.3190468138462996e-17,
    5.707708438416212e-34,
];

/// `x - k·π/2` in double-double.
fn reduce_pio2(x: f64, k: f64) -> Dd {
    let p = two_prod(k, PIO2[0]);
    let r = two_sum(x, -p.hi);
    let r = r.sub(Dd::from(p.lo));
    let r = r.sub(two_prod(k, PIO2[1]));
    let r = r.sub(two_prod(k, PIO2[2]));
    r.sub(Dd::from(k * PIO2[3]))
}

/// Taylor series of sin (`odd`) or cos around 0 for `|r| <= π/4`.
fn taylor(r: Dd, odd: bool) -> Dd {
    let r2 = r.mul(r);
    // Terms r^n / n! for n = start, start + 2, ... until below 2^-110.
    let mut n = if odd { 1.0 } else { 0.0 };
    let mut term = if odd { r } else { Dd::from(1.0) };
    let mut sum = term;
    loop {
        term = term.mul(r2).div_f((n + 1.0) * (n + 2.0)).neg();
        n += 2.0;
        if term.hi.abs() < 1e-34 * sum.hi.abs().max(1e-300) {
            break;
        }
        sum = sum.add(term);
        if n > 60.0 {
            break;
        }
    }
    sum
}

/// Quadrant `k mod 4` and reduced argument of `x`.
fn quadrant(x: f64) -> (i64, Dd) {
    let k = (x * (2.0 / std::f64::consts::PI)).round();
    let r = reduce_pio2(x, k);
    (k.rem_euclid(4.0) as i64, r)
}

/// Correctly rounded `sin`.
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() || x.abs() > 1e9 {
        return x.sin();
    }
    if x.abs() < 1e-8 {
        // sin x = x - x³/6: rounds to x.
        return x;
    }
    let (q, r) = quadrant(x);
    match q {
        0 => taylor(r, true).round(),
        1 => taylor(r, false).round(),
        2 => -taylor(r, true).round(),
        _ => -taylor(r, false).round(),
    }
}

/// Correctly rounded `cos`.
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() || x.abs() > 1e9 {
        return x.cos();
    }
    if x.abs() < 1e-9 {
        return 1.0;
    }
    let (q, r) = quadrant(x);
    match q {
        0 => taylor(r, false).round(),
        1 => -taylor(r, true).round(),
        2 => -taylor(r, false).round(),
        _ => taylor(r, true).round(),
    }
}

/// Natural logarithm of a positive finite `x` in double-double.
fn ln_dd(x: f64) -> Dd {
    // x = m · 2^e with m in [√½, √2).
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if e == -1023 {
        // Subnormal: normalise first.
        let n = x * 2f64.powi(54);
        let b = n.to_bits();
        e = ((b >> 52) & 0x7ff) as i64 - 1023 - 54;
        m = f64::from_bits((b & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    }
    if m > std::f64::consts::SQRT_2 {
        m /= 2.0;
        e += 1;
    }
    // ln m = 2 atanh(u), u = (m - 1) / (m + 1).
    let u = Dd::from(m - 1.0).div(two_sum(m, 1.0));
    let u2 = u.mul(u);
    let mut term = u;
    let mut sum = u;
    let mut k = 1.0;
    loop {
        term = term.mul(u2);
        k += 2.0;
        let t = term.div_f(k);
        if t.hi.abs() < 1e-34 * sum.hi.abs().max(1e-300) || k > 99.0 {
            break;
        }
        sum = sum.add(t);
    }
    let ln2 = Dd::new(LN2[0], LN2[1]).add(Dd::from(LN2[2]));
    sum.mul_f(2.0).add(ln2.mul_f(e as f64))
}

/// Correctly rounded `ln` (`Math.log`).
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 || !x.is_finite() {
        return x.ln();
    }
    if x == 1.0 {
        return 0.0;
    }
    ln_dd(x).round()
}

/// `Math.pow`. Chromium's is not correctly rounded: it agrees with glibc's
/// (ARM optimized-routines) on every input sampled, so this defers to the
/// platform `powf`.
pub fn pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_chromium_where_glibc_is_off_by_one_ulp() {
        // Values printed by Chromium (V8 on LLVM libc) that glibc 2.39 misses.
        for (x, want) in [
            (1.9658802692497868, 0.922964259367253),
            (3.0104509657321037, 0.13076611206208924),
            (2.965252274248037, 0.1754278879056179),
            (3.3029978798967625, -0.16070532660753942),
        ] {
            assert_eq!(sin(x), want, "sin {x}");
        }
        for (x, want) in [
            (1.8586589070496746, -0.28390340408895526),
            (4.810842384038421, 0.09829442805501364),
            (4.442512474670682, -0.2666124144564878),
            (1.0361575833452423, 0.5095302284893409),
        ] {
            assert_eq!(cos(x), want, "cos {x}");
        }
        for (x, want) in [
            (2.184824889352417, 0.781535683163191),
            (3.1437007003673543, 1.1454006729512551),
            (1.1940328403723452, 0.17733651909162018),
            (2.8091417750223684, 1.0328790185545293),
        ] {
            assert_eq!(ln(x), want, "ln {x}");
        }
        let cases = [
            (0.0, 0.0, 1.0),
            (std::f64::consts::FRAC_PI_2, 1.0, 6.123233995736766e-17),
            (std::f64::consts::PI, 1.2246467991473532e-16, -1.0),
        ];
        for (x, s, c) in cases {
            assert_eq!(sin(x), s, "sin {x}");
            assert_eq!(cos(x), c, "cos {x}");
        }
    }

    #[test]
    fn log_and_pow_are_correctly_rounded() {
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(ln(std::f64::consts::E), 1.0);
        assert_eq!(ln(2.0), std::f64::consts::LN_2);
        assert_eq!(ln(10.0), std::f64::consts::LN_10);
        assert_eq!(pow(2.0, 10.0), 1024.0);
    }
}
