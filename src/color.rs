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

    pub fn from_hsl(h: f64, s: f64, l: f64, a: f64) -> Rgba {
        let h = h.rem_euclid(360.0) / 360.0;
        let s = (s / 100.0).clamp(0.0, 1.0);
        let l = (l / 100.0).clamp(0.0, 1.0);
        let to = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if s == 0.0 {
            return Rgba {
                r: to(l),
                g: to(l),
                b: to(l),
                a,
            };
        }
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        let hue = |mut t: f64| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        Rgba {
            r: to(hue(h + 1.0 / 3.0)),
            g: to(hue(h)),
            b: to(hue(h - 1.0 / 3.0)),
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
        let ch = |v: &str| -> Option<u8> {
            let v = v.trim();
            Some(if let Some(pct) = v.strip_suffix('%') {
                (pct.parse::<f64>().ok()? * 2.55).round().clamp(0.0, 255.0) as u8
            } else {
                v.parse::<f64>().ok()?.round().clamp(0.0, 255.0) as u8
            })
        };
        let a = p.get(3).and_then(|v| parse_alpha(v)).unwrap_or(1.0);
        return Some(Rgba {
            r: ch(p[0])?,
            g: ch(p[1])?,
            b: ch(p[2])?,
            a,
        });
    }
    if let Some(args) = func_args(&s, "hsla").or_else(|| func_args(&s, "hsl")) {
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
        return Some(Rgba::from_hsl(num(p[0])?, num(p[1])?, num(p[2])?, a));
    }
    named(&s)
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
    fn hsl_roundtrip() {
        let c = parse("#9370db").unwrap();
        let (h, s, l) = c.to_hsl();
        assert_eq!(Rgba::from_hsl(h, s, l, 1.0).to_hex(), "#9370db");
    }
}
