//! CSS colour parsing and manipulation (hex, rgb[a], hsl[a] and common names).

/// An sRGB colour with alpha in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f64,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 1.0,
    };
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 1.0,
    };

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Components as `0.0..=1.0` floats.
    pub fn unit(self) -> (f64, f64, f64) {
        (
            self.r as f64 / 255.0,
            self.g as f64 / 255.0,
            self.b as f64 / 255.0,
        )
    }

    pub fn to_hsl(self) -> (f64, f64, f64) {
        let (r, g, b) = self.unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f64::EPSILON {
            return (0.0, 0.0, l * 100.0);
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s * 100.0, l * 100.0)
    }

    /// The colour of a khroma HSL result. khroma stringifies it as `hsl(h, s%, l%)` with
    /// each channel clamped (`h % 360`, `s` and `l` to `0..=100`) and rounded to 10
    /// decimals, and the browser turns that string into sRGB ([`Rgba::from_css_hsl`]).
    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Rgba {
        let round10 = |v: f64| js_round(v * 1e10) / 1e10;
        Rgba::from_css_hsl(
            round10(h % 360.0),
            round10(s.clamp(0.0, 100.0)),
            round10(l.clamp(0.0, 100.0)),
            a,
        )
    }

    /// Chromium's `hsl(h, s%, l%)`: the hue wraps into `0..360` and the percentages
    /// clamp in doubles, then `gfx::HSLToSRGB` runs in `float` (CSS Color 4's
    /// `f(n)` formula) and each channel serialises as `round((c + 1e-7f) * 255)`.
    pub fn from_css_hsl(h: f64, s: f64, l: f64, a: f64) -> Rgba {
        let h = if h.is_finite() {
            (h % 360.0 + 360.0) % 360.0
        } else {
            0.0
        };
        let unit = |v: f64| ((v / 100.0 * 100.0) / 100.0).clamp(0.0, 1.0) as f32;
        let (h, s, l) = (h as f32, unit(s), unit(l));
        let f = |n: f32| -> f32 {
            if s == 0.0 {
                return l;
            }
            let k = ((n + h / 30.0) as f64 % 12.0) as f32;
            let a = s * l.min(1.0 - l);
            l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
        };
        let to = |c: f32| {
            (((c + 1e-7) as f64 * 255.0) as f32 as f64)
                .clamp(0.0, 255.0)
                .round() as u8
        };
        Rgba {
            r: to(f(0.0)),
            g: to(f(8.0)),
            b: to(f(4.0)),
            a,
        }
    }

    /// Adjust HSL lightness by `delta` percentage points.
    pub fn lighten(self, delta: f64) -> Rgba {
        let (h, s, l) = self.to_hsl();
        Rgba::from_hsl(h, s, l + delta, self.a)
    }

    /// khroma's `darken(c, amount)`: lower HSL lightness by `amount` points (clamped at 0).
    pub fn darken(self, amount: f64) -> Rgba {
        self.lighten(-amount)
    }

    /// khroma's `adjust(c, { h, s, l })`: shift the HSL channels. Hue wraps, saturation and
    /// lightness clamp to `0..=100` (khroma's channel clamps).
    pub fn adjust_hsl(self, dh: f64, ds: f64, dl: f64) -> Rgba {
        let (h, s, l) = self.to_hsl();
        Rgba::from_hsl(
            h + dh,
            (s + ds).clamp(0.0, 100.0),
            (l + dl).clamp(0.0, 100.0),
            self.a,
        )
    }

    /// Rotate hue by `deg` degrees.
    pub fn rotate_hue(self, deg: f64) -> Rgba {
        let (h, s, l) = self.to_hsl();
        Rgba::from_hsl(h + deg, s, l, self.a)
    }

    /// Perceived darkness (used to pick contrasting text).
    pub fn is_dark(self) -> bool {
        let (r, g, b) = self.unit();
        0.299 * r + 0.587 * g + 0.114 * b < 0.5
    }

    pub fn invert(self) -> Rgba {
        Rgba {
            r: 255 - self.r,
            g: 255 - self.g,
            b: 255 - self.b,
            a: self.a,
        }
    }
}

/// JavaScript's `Math.round`: halves round up.
fn js_round(v: f64) -> f64 {
    let f = v.floor();
    if v - f >= 0.5 { f + 1.0 } else { f }
}

/// Parse a CSS colour. Returns `None` for unsupported syntax (e.g. `var(--x)`).
pub fn parse(input: &str) -> Option<Rgba> {
    let s = input
        .trim()
        .trim_end_matches("!important")
        .trim()
        .to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(args) = func_args(&s, "rgba").or_else(|| func_args(&s, "rgb")) {
        let p = split_args(args);
        if p.len() < 3 {
            return None;
        }
        // Chromium's stylesheet parser: the channel clamps in doubles, is stored as a
        // `float` and serialises rounded (so `127.4999999999` is 128).
        let ch = |v: &str| -> Option<u8> {
            let v = v.trim();
            let c = if let Some(pct) = v.strip_suffix('%') {
                pct.parse::<f64>().ok()? / 100.0 * 255.0
            } else {
                v.parse::<f64>().ok()?
            };
            Some((c.clamp(0.0, 255.0) as f32).round() as u8)
        };
        let a = p.get(3).and_then(|v| parse_alpha(v)).unwrap_or(1.0);
        return Some(Rgba {
            r: ch(p[0])?,
            g: ch(p[1])?,
            b: ch(p[2])?,
            a,
        });
    }
    if s.starts_with("hsl") {
        let (h, sat, l, a) = hsl_args(&s)?;
        return Some(Rgba::from_css_hsl(h, sat, l, a));
    }
    named(&s)
}

/// The channels of an `hsl()`/`hsla()` string, unconverted.
fn hsl_args(s: &str) -> Option<(f64, f64, f64, f64)> {
    let args = func_args(s, "hsla").or_else(|| func_args(s, "hsl"))?;
    let p = split_args(args);
    if p.len() < 3 {
        return None;
    }
    let num = |v: &str| {
        v.trim()
            .trim_end_matches('%')
            .trim_end_matches("deg")
            .parse::<f64>()
            .ok()
    };
    let a = p.get(3).and_then(|v| parse_alpha(v)).unwrap_or(1.0);
    Some((num(p[0])?, num(p[1])?, num(p[2])?, a))
}

/// khroma's `adjust(color, { h, s, l })` on a CSS colour string, as the browser
/// then paints the `hsl()` string khroma returns. khroma keeps an `hsl()` input's
/// own channels (no trip through sRGB bytes), so `darken("hsl(80, 100%,
/// 56.2745098039%)", 30)` is exactly `hsl(80, 100%, 26.2745098039%)`; any other
/// colour is converted from its sRGB channels. `None` when `input` does not parse.
pub fn adjust(input: &str, dh: f64, ds: f64, dl: f64) -> Option<Rgba> {
    let lower = input.trim().to_ascii_lowercase();
    let (h, s, l, a) = match hsl_args(&lower) {
        Some((h, s, l, a)) => (h, s.clamp(0.0, 100.0), l.clamp(0.0, 100.0), a),
        None => {
            let c = parse(input)?;
            let (h, s, l) = c.to_hsl();
            (h, s, l, c.a)
        }
    };
    Some(Rgba::from_hsl(
        h + dh,
        (s + ds).clamp(0.0, 100.0),
        (l + dl).clamp(0.0, 100.0),
        a,
    ))
}

fn parse_alpha(v: &str) -> Option<f64> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        p.parse::<f64>().ok().map(|x| (x / 100.0).clamp(0.0, 1.0))
    } else {
        v.parse::<f64>().ok().map(|x| x.clamp(0.0, 1.0))
    }
}

fn func_args<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn split_args(args: &str) -> Vec<&str> {
    args.split([',', ' ', '/'])
        .filter(|p| !p.trim().is_empty())
        .collect()
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let d = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
    let dd = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => Some(Rgba {
            r: d(0)?,
            g: d(1)?,
            b: d(2)?,
            a: 1.0,
        }),
        4 => Some(Rgba {
            r: d(0)?,
            g: d(1)?,
            b: d(2)?,
            a: d(3)? as f64 / 255.0,
        }),
        6 => Some(Rgba {
            r: dd(0)?,
            g: dd(2)?,
            b: dd(4)?,
            a: 1.0,
        }),
        8 => Some(Rgba {
            r: dd(0)?,
            g: dd(2)?,
            b: dd(4)?,
            a: dd(6)? as f64 / 255.0,
        }),
        _ => None,
    }
}

fn named(s: &str) -> Option<Rgba> {
    const TABLE: &[(&str, u32)] = &[
        ("black", 0x000000),
        ("white", 0xffffff),
        ("red", 0xff0000),
        ("green", 0x008000),
        ("blue", 0x0000ff),
        ("yellow", 0xffff00),
        ("orange", 0xffa500),
        ("purple", 0x800080),
        ("gray", 0x808080),
        ("grey", 0x808080),
        ("lightgrey", 0xd3d3d3),
        ("lightgray", 0xd3d3d3),
        ("darkgray", 0xa9a9a9),
        ("darkgrey", 0xa9a9a9),
        ("silver", 0xc0c0c0),
        ("pink", 0xffc0cb),
        ("cyan", 0x00ffff),
        ("aqua", 0x00ffff),
        ("magenta", 0xff00ff),
        ("fuchsia", 0xff00ff),
        ("brown", 0xa52a2a),
        ("navy", 0x000080),
        ("teal", 0x008080),
        ("lime", 0x00ff00),
        ("olive", 0x808000),
        ("maroon", 0x800000),
        ("gold", 0xffd700),
        ("coral", 0xff7f50),
        ("salmon", 0xfa8072),
        ("beige", 0xf5f5dc),
        ("ivory", 0xfffff0),
        ("khaki", 0xf0e68c),
        ("lavender", 0xe6e6fa),
        ("violet", 0xee82ee),
        ("indigo", 0x4b0082),
        ("tomato", 0xff6347),
        ("wheat", 0xf5deb3),
        ("crimson", 0xdc143c),
        ("darkblue", 0x00008b),
        ("darkgreen", 0x006400),
        ("darkred", 0x8b0000),
        ("lightblue", 0xadd8e6),
        ("lightgreen", 0x90ee90),
        ("lightyellow", 0xffffe0),
        ("whitesmoke", 0xf5f5f5),
        ("dimgray", 0x696969),
        ("steelblue", 0x4682b4),
        ("skyblue", 0x87ceeb),
        ("orchid", 0xda70d6),
        ("plum", 0xdda0dd),
        ("tan", 0xd2b48c),
        ("chocolate", 0xd2691e),
        ("firebrick", 0xb22222),
        ("seagreen", 0x2e8b57),
        ("forestgreen", 0x228b22),
        ("royalblue", 0x4169e1),
        ("slategray", 0x708090),
        ("aliceblue", 0xf0f8ff),
        ("honeydew", 0xf0fff0),
        ("mintcream", 0xf5fffa),
    ];
    if s == "transparent" || s == "none" {
        return Some(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        });
    }
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, v)| Rgba {
        r: (v >> 16) as u8,
        g: (v >> 8) as u8,
        b: *v as u8,
        a: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse("#fff"), Some(Rgba::WHITE));
        assert_eq!(parse("#ECECFF").unwrap().to_hex(), "#ececff");
        assert_eq!(parse("rgb(255, 0, 0)").unwrap().to_hex(), "#ff0000");
        assert!((parse("rgba(232,232,232, 0.8)").unwrap().a - 0.8).abs() < 1e-9);
        assert_eq!(parse("hsl(0, 100%, 50%)").unwrap().to_hex(), "#ff0000");
        assert_eq!(parse("transparent").unwrap().a, 0.0);
        assert_eq!(parse("Red").unwrap().to_hex(), "#ff0000");
        assert_eq!(parse("var(--x)"), None);
    }

    #[test]
    fn hsl_strings_resolve_as_chromium_does() {
        // getComputedStyle() in headless Chromium (14824 strings checked, in a
        // stylesheet, a style attribute, a fill attribute and element.style).
        let rgb = |s: &str| {
            let c = parse(s).unwrap();
            [c.r, c.g, c.b]
        };
        assert_eq!(rgb("hsl(0, 50%, 1.1764705882%)"), [5, 2, 2]);
        assert_eq!(rgb("hsl(0, 50%, 20.0%)"), [77, 26, 26]);
        assert_eq!(rgb("hsl(0, 50%, 27.0588235294%)"), [104, 35, 35]);
        assert_eq!(rgb("hsl(270, 100%, 76.2745098039%)"), [195, 134, 255]);
        assert_eq!(rgb("hsl(80, 100%, 26.2745098039%)"), [89, 134, 0]);
        // A float channel rounds as the stylesheet's float does.
        assert_eq!(rgb("rgb(127.4999999999, 10.49999999, 3.5)"), [128, 11, 4]);
    }

    #[test]
    fn adjust_keeps_an_hsl_strings_channels() {
        // venn: darken("hsl(80, 100%, 56.2745098039%)", 30) paints rgb(89, 134, 0);
        // a trip through sRGB bytes would give 90.
        let c = adjust("hsl(80, 100%, 56.2745098039%)", 0.0, 0.0, -30.0).unwrap();
        assert_eq!([c.r, c.g, c.b], [89, 134, 0]);
        let c = adjust("#ECECFF", 0.0, 0.0, -10.0).unwrap();
        assert_eq!(c, Rgba::from_hsl(240.0, 100.0, 86.2745098039, 1.0));
    }

    #[test]
    fn hsl_roundtrip() {
        let c = parse("#9370db").unwrap();
        let (h, s, l) = c.to_hsl();
        assert_eq!(Rgba::from_hsl(h, s, l, 1.0).to_hex(), "#9370db");
    }
}
