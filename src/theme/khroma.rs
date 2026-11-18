//! khroma 2.1.0 (the colour library mermaid's themes derive their colours with),
//! ported operation for operation on colour strings.
//!
//! Every khroma method parses its input string, changes channels and prints the
//! result again, so a derived colour carries the printing's rounding (to ten
//! decimals) into the next derivation, and its notation decides what the next
//! method reads: a colour last changed in HSL prints as `hsl(..)` and is read back
//! in HSL, an unchanged one keeps its original spelling, and RGB channels that are
//! not whole numbers print as `rgb(..)` (left for the browser to round), others as
//! hex. Matching mermaid's colours to the last 8-bit step needs all of that, e.g.
//! the base theme's `titleColor`, `invert(adjust('#fff4dd', { h: 180, l: 5 }))`, is
//! `rgb(8.5000000002, 5.7500000001, 0)`, which the browser paints as `rgb(9, 6, 0)`.

/// Which channel set a colour was last written in (khroma's `Type`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ty {
    All,
    Rgb,
    Hsl,
}

/// khroma's `Channels`: the parsed channels, the set last written and whether
/// anything changed since parsing (then the original string is printed back).
#[derive(Debug, Clone)]
struct Ch {
    r: Option<f64>,
    g: Option<f64>,
    b: Option<f64>,
    h: Option<f64>,
    s: Option<f64>,
    l: Option<f64>,
    a: f64,
    ty: Ty,
    changed: bool,
    src: String,
}

/// `Math.round`: halves round up.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// khroma's `lang.round`: to ten decimals.
pub(super) fn round10(x: f64) -> f64 {
    let v = js_round(x * 1e10) / 1e10;
    if v == 0.0 { 0.0 } else { v }
}

/// A number as JavaScript prints it (`String(n)`), for the ranges colours use.
fn js_num(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    format!("{x}")
}

fn clamp_rgb(v: f64) -> f64 {
    v.clamp(0.0, 255.0)
}

fn clamp_pct(v: f64) -> f64 {
    v.clamp(0.0, 100.0)
}

fn clamp_a(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// `hsl2rgb` for a colour held in HSL (channel 0, 1, 2 = r, g, b).
pub(super) fn hsl_to_rgb(h: f64, s: f64, l: f64, ch: usize) -> f64 {
    hsl2rgb(h, s, l, ch)
}

/// `RGB.stringify` of an opaque colour.
pub(super) fn rgb_string(r: f64, g: f64, b: f64) -> String {
    format!(
        "rgb({}, {}, {})",
        js_num(round10(r)),
        js_num(round10(g)),
        js_num(round10(b))
    )
}

/// `channel.hue2rgb`.
fn hue2rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

/// `channel.hsl2rgb` for channel 0 (r), 1 (g) or 2 (b).
fn hsl2rgb(h: f64, s: f64, l: f64, ch: usize) -> f64 {
    if s == 0.0 {
        return l * 2.55;
    }
    let (h, s, l) = (h / 360.0, s / 100.0, l / 100.0);
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    match ch {
        0 => hue2rgb(p, q, h + 1.0 / 3.0) * 255.0,
        1 => hue2rgb(p, q, h) * 255.0,
        _ => hue2rgb(p, q, h - 1.0 / 3.0) * 255.0,
    }
}

/// `channel.rgb2hsl` for channel 0 (h), 1 (s) or 2 (l).
fn rgb2hsl(r: f64, g: f64, b: f64, ch: usize) -> f64 {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if ch == 2 {
        return l * 100.0;
    }
    if max == min {
        return 0.0;
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    if ch == 1 {
        return s * 100.0;
    }
    if max == r {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) * 60.0
    } else if max == g {
        ((b - r) / d + 2.0) * 60.0
    } else {
        ((r - g) / d + 4.0) * 60.0
    }
}

impl Ch {
    fn rgb_data(&mut self) -> (f64, f64, f64) {
        // `_ensureRGB`.
        let (h, s, l) = (
            self.h.unwrap_or(0.0),
            self.s.unwrap_or(0.0),
            self.l.unwrap_or(0.0),
        );
        if self.r.is_none() {
            self.r = Some(hsl2rgb(h, s, l, 0));
        }
        if self.g.is_none() {
            self.g = Some(hsl2rgb(h, s, l, 1));
        }
        if self.b.is_none() {
            self.b = Some(hsl2rgb(h, s, l, 2));
        }
        (self.r.unwrap(), self.g.unwrap(), self.b.unwrap())
    }

    fn hsl_data(&mut self) -> (f64, f64, f64) {
        // `_ensureHSL`.
        let (r, g, b) = (
            self.r.unwrap_or(0.0),
            self.g.unwrap_or(0.0),
            self.b.unwrap_or(0.0),
        );
        if self.h.is_none() {
            self.h = Some(rgb2hsl(r, g, b, 0));
        }
        if self.s.is_none() {
            self.s = Some(rgb2hsl(r, g, b, 1));
        }
        if self.l.is_none() {
            self.l = Some(rgb2hsl(r, g, b, 2));
        }
        (self.h.unwrap(), self.s.unwrap(), self.l.unwrap())
    }

    /// The `r`, `g`, `b` getters.
    fn rgb(&mut self, ch: usize) -> f64 {
        let v = [self.r, self.g, self.b][ch];
        if self.ty != Ty::Hsl
            && let Some(v) = v
        {
            return v;
        }
        let (h, s, l) = self.hsl_data();
        hsl2rgb(h, s, l, ch)
    }

    /// The `h`, `s`, `l` getters.
    fn hsl(&mut self, ch: usize) -> f64 {
        let v = [self.h, self.s, self.l][ch];
        if self.ty != Ty::Rgb
            && let Some(v) = v
        {
            return v;
        }
        let (r, g, b) = self.rgb_data();
        rgb2hsl(r, g, b, ch)
    }

    fn set_hsl(&mut self, ch: usize, v: f64) {
        self.ty = Ty::Hsl;
        self.changed = true;
        match ch {
            0 => self.h = Some(v),
            1 => self.s = Some(v),
            _ => self.l = Some(v),
        }
    }

    /// `Color.stringify`.
    fn stringify(&mut self) -> String {
        if !self.changed && !self.src.is_empty() {
            return self.src.clone();
        }
        if self.ty == Ty::Hsl || self.r.is_none() {
            let (h, s, l) = (self.hsl(0), self.hsl(1), self.hsl(2));
            let (h, s, l) = (js_num(round10(h)), js_num(round10(s)), js_num(round10(l)));
            return if self.a < 1.0 {
                format!("hsla({h}, {s}%, {l}%, {})", js_num(self.a))
            } else {
                format!("hsl({h}, {s}%, {l}%)")
            };
        }
        let (r, g, b) = (self.rgb(0), self.rgb(1), self.rgb(2));
        if self.a < 1.0 || r.fract() != 0.0 || g.fract() != 0.0 || b.fract() != 0.0 {
            let (r, g, b) = (js_num(round10(r)), js_num(round10(g)), js_num(round10(b)));
            if self.a < 1.0 {
                format!("rgba({r}, {g}, {b}, {})", js_num(round10(self.a)))
            } else {
                format!("rgb({r}, {g}, {b})")
            }
        } else {
            let hx = |v: f64| format!("{:02x}", js_round(v) as i64);
            format!("#{}{}{}", hx(r), hx(g), hx(b))
        }
    }
}

fn from_rgb(r: f64, g: f64, b: f64, a: f64, src: &str) -> Ch {
    Ch {
        r: Some(r),
        g: Some(g),
        b: Some(b),
        h: None,
        s: None,
        l: None,
        a,
        ty: Ty::All,
        changed: false,
        src: src.to_string(),
    }
}

fn parse_hex(s: &str) -> Option<Ch> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let dec = u64::from_str_radix(hex, 16).ok()?;
    let len = hex.len();
    let has_alpha = len % 4 == 0;
    let full = len > 4;
    let mul = if full { 1.0 } else { 17.0 };
    let bits = if full { 8 } else { 4 };
    let off: i64 = if has_alpha { 0 } else { -1 };
    let mask = if full { 255 } else { 15 };
    let ch = |k: i64| ((dec >> (bits * (off + k)) as u64) & mask) as f64 * mul;
    let a = if has_alpha {
        (dec & mask) as f64 * mul / 255.0
    } else {
        1.0
    };
    Some(from_rgb(ch(3), ch(2), ch(1), a, s))
}

/// The numbers of a functional notation's arguments, with whether each ends in `%`.
fn args(s: &str, names: &[&str]) -> Option<Vec<(f64, bool, String)>> {
    let lower = s.to_ascii_lowercase();
    let inner = names
        .iter()
        .find_map(|n| lower.strip_prefix(n).and_then(|r| r.strip_prefix('(')))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    parts
        .iter()
        .map(|p| {
            let p = p.trim_start_matches('+');
            let (num, pct) = match p.strip_suffix('%') {
                Some(n) => (n, true),
                None => (p, false),
            };
            let digits = num.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let unit = num[digits.len()..].to_string();
            digits.parse::<f64>().ok().map(|v| (v, pct, unit))
        })
        .collect()
}

fn parse_rgb(s: &str) -> Option<Ch> {
    if !s.starts_with(['r', 'R']) {
        return None;
    }
    let v = args(s, &["rgba", "rgb"])?;
    let c = |i: usize| clamp_rgb(if v[i].1 { v[i].0 * 2.55 } else { v[i].0 });
    let a = v
        .get(3)
        .map_or(1.0, |x| clamp_a(if x.1 { x.0 / 100.0 } else { x.0 }));
    Some(from_rgb(c(0), c(1), c(2), a, s))
}

fn parse_hsl(s: &str) -> Option<Ch> {
    if !s.starts_with(['h', 'H']) {
        return None;
    }
    let v = args(s, &["hsla", "hsl"])?;
    if !v[1].1 || !v[2].1 {
        return None;
    }
    let h = match v[0].2.as_str() {
        "grad" => v[0].0 * 0.9,
        "rad" => v[0].0 * 180.0 / std::f64::consts::PI,
        "turn" => v[0].0 * 360.0,
        _ => v[0].0,
    } % 360.0;
    let a = v
        .get(3)
        .map_or(1.0, |x| clamp_a(if x.1 { x.0 / 100.0 } else { x.0 }));
    Some(Ch {
        r: None,
        g: None,
        b: None,
        h: Some(h),
        s: Some(clamp_pct(v[1].0)),
        l: Some(clamp_pct(v[2].0)),
        a,
        ty: Ty::All,
        changed: false,
        src: s.to_string(),
    })
}

fn parse(s: &str) -> Option<Ch> {
    parse_hex(s)
        .or_else(|| parse_rgb(s))
        .or_else(|| parse_hsl(s))
        .or_else(|| {
            // Keywords (the subset of names the crate knows; `transparent` too).
            if !s.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            let c = crate::color::parse(&s.to_ascii_lowercase())?;
            Some(from_rgb(c.r as f64, c.g as f64, c.b as f64, c.a, s))
        })
}

/// khroma `adjust(c, { h, s, l })`: zero deltas are skipped; an unparseable
/// colour comes back unchanged.
pub fn adjust(c: &str, dh: f64, ds: f64, dl: f64) -> String {
    let Some(mut ch) = parse(c) else {
        return c.to_string();
    };
    let mut changes = Vec::new();
    for (i, d) in [dh, ds, dl].into_iter().enumerate() {
        if d != 0.0 {
            changes.push((i, ch.hsl(i) + d));
        }
    }
    // `change`: parse afresh and set each channel, clamped.
    let mut ch = parse(c).unwrap();
    for (i, v) in changes {
        let v = if i == 0 { v % 360.0 } else { clamp_pct(v) };
        ch.set_hsl(i, v);
    }
    ch.stringify()
}

/// khroma `adjustChannel(c, 'l', amount)`: a clamped value equal to the current
/// one leaves the colour (and its spelling) as it was.
fn adjust_l(c: &str, amount: f64) -> String {
    let Some(mut ch) = parse(c) else {
        return c.to_string();
    };
    let cur = ch.hsl(2);
    let next = clamp_pct(cur + amount);
    if cur != next {
        ch.set_hsl(2, next);
    }
    ch.stringify()
}

pub fn lighten(c: &str, amount: f64) -> String {
    adjust_l(c, amount)
}

pub fn darken(c: &str, amount: f64) -> String {
    adjust_l(c, -amount)
}

/// khroma `invert(c)`: `mix(inverse, c, 100)`, i.e. the inverse channels as
/// `rgba(r, g, b, a)`.
pub fn invert(c: &str) -> String {
    let Some(mut ch) = parse(c) else {
        return c.to_string();
    };
    let inv: Vec<f64> = (0..3).map(|i| 255.0 - ch.rgb(i)).collect();
    // `mix` with weight 100: weight1 = 1, weight2 = 0.
    let (w1, w2) = (1.0, 0.0);
    let orig: Vec<f64> = (0..3).map(|i| ch.rgb(i)).collect();
    let v: Vec<f64> = (0..3)
        .map(|i| clamp_rgb(inv[i] * w1 + orig[i] * w2))
        .collect();
    let a = clamp_a(ch.a);
    let mut out = from_rgb(v[0], v[1], v[2], a, "");
    out.changed = true;
    out.stringify()
}

/// khroma `rgba(c, a)` (mermaid's `fade`): the colour with its alpha set.
pub fn with_alpha(c: &str, a: f64) -> String {
    let Some(mut ch) = parse(c) else {
        return c.to_string();
    };
    ch.a = clamp_a(a);
    ch.changed = true;
    ch.stringify()
}

/// theme-helpers.js `mkBorder(c, darkMode)`.
pub fn mk_border(c: &str, dark_mode: bool) -> String {
    adjust(c, 0.0, -40.0, if dark_mode { 10.0 } else { -10.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values mermaid 12.3.0 resolves (`getConfig().themeVariables`).
    #[test]
    fn matches_mermaids_theme_variables() {
        // theme-base.js from `primaryColor: #fff4dd`.
        let t = adjust("#fff4dd", 180.0, 0.0, 5.0);
        assert_eq!(t, "hsl(220.5882352941, 100%, 98.3333333333%)");
        assert_eq!(invert(&t), "rgb(8.5000000002, 5.7500000001, 0)");
        assert_eq!(
            mk_border(&t, false),
            "hsl(220.5882352941, 60%, 88.3333333333%)"
        );
        assert_eq!(
            adjust("#fff4dd", -120.0, 0.0, 0.0),
            "hsl(-79.4117647059, 100%, 93.3333333333%)"
        );
        assert_eq!(invert("#f4f4f4"), "#0b0b0b");
        // darkMode: `primaryColor: #00ff00`.
        assert_eq!(
            invert(&adjust("#00ff00", 180.0, 0.0, 5.0)),
            "rgb(0, 229.5, 0)"
        );
        assert_eq!(
            darken(&adjust("#00ff00", -120.0, 0.0, 0.0), 30.0),
            "hsl(0, 100%, 20%)"
        );
        // An unchanged colour keeps its spelling; a clamped no-op too.
        assert_eq!(lighten("white", 10.0), "white");
        assert_eq!(adjust("#ECECFF", 0.0, 0.0, 0.0), "#ECECFF");
        assert_eq!(
            with_alpha("hsl(80, 100%, 96.2745098039%)", 0.5),
            "hsla(80, 100%, 96.2745098039%, 0.5)"
        );
    }
}
