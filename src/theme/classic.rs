//! The colour sets of mermaid's classic themes (`theme-{default,forest,dark,neutral,base}.js`):
//! `cScale0..11` with their `cScalePeer`, `cScaleInv` and `cScaleLabel` companions, the
//! journey `fillType0..7` and the git-graph `git0..7` / `gitInv0..7` / `gitBranchLabel0..7`.
//!
//! Each theme's `updateColors()` is replayed with khroma semantics, including the
//! upstream quirks that make the result differ from what the formulas suggest:
//!
//! * `getThemeVariables` runs `updateColors()` after copying the user's overrides onto
//!   the theme and copies them again afterwards, so an explicit `cScaleN`, `fillTypeN`, ...
//!   always wins;
//! * the default theme's constructor already calls `updateColors()`, so `calculate()`
//!   runs it a second time: `cScaleN` is darkened twice (10 + 10 points), the git colours
//!   twice (25 + 25), and the `||`-guarded values (`cScalePeerN`, `cScaleInvN`) keep what
//!   the first run derived from the stock `primaryColor`;
//! * colours keep khroma's HSL channels between operations, so e.g. the base theme's
//!   `adjust(primaryColor, { h: 210, l: 150 })` (white, at full lightness) darkens back
//!   into a hue instead of into grey.

use super::ThemeName;
use crate::color::{self, Rgba};
use crate::scene::Color;
use std::collections::HashMap;

/// `THEME_COLOR_LIMIT`.
const LIMIT: usize = 12;

/// A colour as khroma holds it: whichever channel set the last operation produced.
#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    Rgb(f64, f64, f64),
    Hsl(f64, f64, f64),
}

impl K {
    fn parse(s: &str) -> Option<K> {
        color::parse(s).map(K::from)
    }

    fn hsl(self) -> (f64, f64, f64) {
        match self {
            K::Hsl(h, s, l) => (h, s, l),
            K::Rgb(r, g, b) => {
                let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
                let max = r.max(g).max(b);
                let min = r.min(g).min(b);
                let l = (max + min) / 2.0;
                if max == min {
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
        }
    }

    fn rgb(self) -> (f64, f64, f64) {
        match self {
            K::Rgb(r, g, b) => (r, g, b),
            K::Hsl(h, s, l) => {
                let (h, s, l) = (h.rem_euclid(360.0) / 360.0, s / 100.0, l / 100.0);
                if s == 0.0 {
                    let v = l * 255.0;
                    return (v, v, v);
                }
                let q = if l < 0.5 {
                    l * (1.0 + s)
                } else {
                    l + s - l * s
                };
                let p = 2.0 * l - q;
                let f = |mut t: f64| {
                    if t < 0.0 {
                        t += 1.0;
                    }
                    if t > 1.0 {
                        t -= 1.0;
                    }
                    let v = if t < 1.0 / 6.0 {
                        p + (q - p) * 6.0 * t
                    } else if t < 0.5 {
                        q
                    } else if t < 2.0 / 3.0 {
                        p + (q - p) * (2.0 / 3.0 - t) * 6.0
                    } else {
                        p
                    };
                    v * 255.0
                };
                (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
            }
        }
    }

    /// khroma `adjust(c, { h, s, l })`: hue is taken `% 360`, saturation and lightness
    /// clamp to `0..=100`.
    fn adjust(self, dh: f64, ds: f64, dl: f64) -> K {
        let (h, s, l) = self.hsl();
        K::Hsl(
            (h + dh) % 360.0,
            (s + ds).clamp(0.0, 100.0),
            (l + dl).clamp(0.0, 100.0),
        )
    }

    fn hue(self, dh: f64) -> K {
        self.adjust(dh, 0.0, 0.0)
    }

    fn lighten(self, by: f64) -> K {
        self.adjust(0.0, 0.0, by)
    }

    fn darken(self, by: f64) -> K {
        self.adjust(0.0, 0.0, -by)
    }

    fn invert(self) -> K {
        let (r, g, b) = self.rgb();
        K::Rgb(255.0 - r, 255.0 - g, 255.0 - b)
    }

    fn to_color(self) -> Color {
        let (r, g, b) = self.rgb();
        let ch = |v: f64| v.round().clamp(0.0, 255.0) as u8;
        Color::new(
            Rgba {
                r: ch(r),
                g: ch(g),
                b: ch(b),
                a: 1.0,
            }
            .to_hex(),
        )
    }
}

impl From<Rgba> for K {
    fn from(c: Rgba) -> K {
        K::Rgb(c.r as f64, c.g as f64, c.b as f64)
    }
}

/// The section colour sets of a classic theme.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SectionColors {
    pub scale: Vec<Color>,
    pub scale_peer: Vec<Color>,
    pub scale_inv: Vec<Color>,
    pub scale_label: Vec<Color>,
    pub fill_type: Vec<Color>,
    pub git: Vec<Color>,
    pub git_inv: Vec<Color>,
    pub git_branch_label: Vec<Color>,
    /// `pie1..12` where the classic palette table does not already match upstream
    /// (forest, dark, base).
    pub pie: Option<Vec<Color>>,
}

/// The theme object's colour variables (`undefined` / `'calculated'` = absent).
struct Vars {
    map: HashMap<String, K>,
    dark_mode: bool,
}

impl Vars {
    fn get(&self, k: &str) -> Option<K> {
        self.map.get(k).copied()
    }
    /// A variable every code path defines before it is read.
    fn c(&self, k: &str) -> K {
        self.get(k).unwrap_or(K::Rgb(0.0, 0.0, 0.0))
    }
    fn set(&mut self, k: impl Into<String>, v: K) {
        self.map.insert(k.into(), v);
    }
    /// `this.k = this.k || f()`.
    fn or(&mut self, k: impl Into<String>, f: impl FnOnce(&Vars) -> K) {
        let k = k.into();
        if !self.map.contains_key(&k) {
            let v = f(self);
            self.map.insert(k, v);
        }
    }
    fn n(&self, prefix: &str, i: usize) -> K {
        self.c(&format!("{prefix}{i}"))
    }
}

fn hex(s: &str) -> K {
    K::parse(s).unwrap_or(K::Rgb(0.0, 0.0, 0.0))
}

/// Hue offsets of `cScale3..11` from `primaryColor`.
const SCALE_HUES: [f64; 9] = [30.0, 60.0, 90.0, 120.0, 150.0, 210.0, 270.0, 300.0, 330.0];

/// `cScale0..11 ||= primary, secondary, tertiary, primary + 30°, ...`.
fn scale_defaults(v: &mut Vars, base_l8: bool) {
    let p = v.c("primaryColor");
    v.or("cScale0", |_| p);
    v.or("cScale1", |v| v.c("secondaryColor"));
    v.or("cScale2", |v| v.c("tertiaryColor"));
    for (i, h) in SCALE_HUES.iter().enumerate() {
        // theme-base.js: `cScale8 = adjust(primaryColor, { h: 210, l: 150 })`.
        let l = if base_l8 && i + 3 == 8 { 150.0 } else { 0.0 };
        v.or(format!("cScale{}", i + 3), |_| p.adjust(*h, 0.0, l));
    }
}

/// theme-default.js / theme-forest.js: darken the scale by 10, peers by 25 more,
/// inverses by hue rotation.
fn scale_darken_peer_rotate(v: &mut Vars) {
    v.or("cScalePeer1", |v| v.c("secondaryColor").darken(45.0));
    v.or("cScalePeer2", |v| v.c("tertiaryColor").darken(40.0));
    for i in 0..LIMIT {
        let s = v.n("cScale", i).darken(10.0);
        v.set(format!("cScale{i}"), s);
        v.or(format!("cScalePeer{i}"), |_| s.darken(25.0));
    }
    for i in 0..LIMIT {
        v.or(format!("cScaleInv{i}"), |v| v.n("cScale", i).hue(180.0));
    }
}

/// `cScaleInvN ||= invert(cScaleN)`, `cScalePeerN ||= lighten/darken(cScaleN, 10)`.
fn scale_invert_peer(v: &mut Vars, peer_lighten: bool) {
    for i in 0..LIMIT {
        v.or(format!("cScaleInv{i}"), |v| v.n("cScale", i).invert());
    }
    for i in 0..LIMIT {
        let s = v.n("cScale", i);
        v.or(format!("cScalePeer{i}"), |_| {
            if peer_lighten {
                s.lighten(10.0)
            } else {
                s.darken(10.0)
            }
        });
    }
}

/// `cScaleLabelN ||= scaleLabelColor`.
fn scale_labels(v: &mut Vars, label: K) {
    for i in 0..LIMIT {
        v.or(format!("cScaleLabel{i}"), |_| label);
    }
}

/// The journey `fillType0..7`; `guarded` = `||` (base) instead of plain assignment.
fn fill_types(v: &mut Vars, guarded: bool) {
    let p = v.c("primaryColor");
    let s = v.c("secondaryColor");
    let vals = [
        p,
        s,
        p.hue(64.0),
        s.hue(64.0),
        p.hue(-64.0),
        s.hue(-64.0),
        p.hue(128.0),
        s.hue(128.0),
    ];
    for (i, c) in vals.into_iter().enumerate() {
        if guarded {
            v.or(format!("fillType{i}"), |_| c);
        } else {
            v.set(format!("fillType{i}"), c);
        }
    }
}

/// `gitN ||= primary, secondary, tertiary, primary -30°, -60°, -90°, +60°, +120°`,
/// then lightened (darkMode) or darkened by 25.
fn git_from_primary(v: &mut Vars) {
    let p = v.c("primaryColor");
    v.or("git0", |_| p);
    v.or("git1", |v| v.c("secondaryColor"));
    v.or("git2", |v| v.c("tertiaryColor"));
    for (i, h) in [-30.0, -60.0, -90.0, 60.0, 120.0].iter().enumerate() {
        v.or(format!("git{}", i + 3), |_| p.hue(*h));
    }
    for i in 0..8 {
        let g = v.n("git", i);
        let g = if v.dark_mode {
            g.lighten(25.0)
        } else {
            g.darken(25.0)
        };
        v.set(format!("git{i}"), g);
    }
}

fn git_inverses(v: &mut Vars) {
    for i in 0..8 {
        v.or(format!("gitInv{i}"), |v| v.n("git", i).invert());
    }
}

/// `gitBranchLabel0/3 ||= invert(labelTextColor)`, the rest `||= labelTextColor`.
fn git_labels_inverted(v: &mut Vars) {
    let lt = v.c("labelTextColor");
    for i in 0..8 {
        v.or(format!("gitBranchLabel{i}"), |_| {
            if i == 0 || i == 3 { lt.invert() } else { lt }
        });
    }
}

fn update_default(v: &mut Vars) {
    scale_defaults(v, false);
    scale_darken_peer_rotate(v);
    if let Some(lt) = v.get("labelTextColor") {
        v.or("cScaleLabel0", |_| lt.invert());
        v.or("cScaleLabel3", |_| lt.invert());
        scale_labels(v, lt);
    }
    let actor_text = v.c("actorTextColor");
    v.set("labelTextColor", actor_text);
    fill_types(v, false);
    git_from_primary(v);
    v.or("gitInv0", |v| v.c("git0").invert().darken(25.0));
    git_inverses(v);
    git_labels_inverted(v);
}

fn update_forest(v: &mut Vars) {
    let actor_text = v.c("actorTextColor");
    v.set("labelTextColor", actor_text);
    scale_defaults(v, false);
    scale_darken_peer_rotate(v);
    let label = v.get("scaleLabelColor").unwrap_or(actor_text);
    scale_labels(v, label);
    fill_types(v, false);
    git_from_primary(v);
    git_inverses(v);
    git_labels_inverted(v);
}

fn update_dark(v: &mut Vars) {
    let contrast = v.c("mainContrastColor");
    v.set("labelTextColor", contrast);
    fill_types(v, false);
    for (i, h) in [
        "#0b0000", "#4d1037", "#3f5258", "#4f2f1b", "#6e0a0a", "#3b0048", "#995a01", "#154706",
        "#161722", "#00296f", "#01629c",
    ]
    .iter()
    .enumerate()
    {
        v.or(format!("cScale{}", i + 1), |_| hex(h));
    }
    scale_defaults(v, false);
    scale_invert_peer(v, true);
    let label =
        v.get("scaleLabelColor")
            .unwrap_or(if v.dark_mode { hex("black") } else { contrast });
    scale_labels(v, label);
    // `pieN = cScaleN`, and the git colours are read off the pie slots.
    let pie = |v: &Vars, i: usize| v.n("cScale", i);
    let git = [
        v.c("secondaryColor").lighten(20.0),
        pie(v, 2).lighten(20.0),
        pie(v, 3).lighten(20.0),
        pie(v, 4).lighten(20.0),
        pie(v, 5).lighten(20.0),
        pie(v, 6).lighten(10.0),
        pie(v, 7).lighten(10.0),
        pie(v, 8).lighten(20.0),
    ];
    for (i, g) in git.into_iter().enumerate() {
        v.set(format!("git{i}"), g);
    }
    git_inverses(v);
    git_labels_inverted(v);
}

fn update_neutral(v: &mut Vars) {
    let text = v.c("text");
    v.set("labelTextColor", text);
    for (i, h) in [
        "#555", "#F4F4F4", "#555", "#BBB", "#777", "#999", "#DDD", "#FFF", "#DDD", "#BBB", "#999",
        "#777",
    ]
    .iter()
    .enumerate()
    {
        v.or(format!("cScale{i}"), |_| hex(h));
    }
    scale_invert_peer(v, v.dark_mode);
    let label = v
        .get("scaleLabelColor")
        .unwrap_or(if v.dark_mode { hex("black") } else { text });
    v.or("cScaleLabel0", |v| v.c("cScale1"));
    v.or("cScaleLabel2", |v| v.c("cScale1"));
    scale_labels(v, label);
    fill_types(v, false);
    // `pieN = cScaleN`; `git0 = darken(pie1, 25)`, `gitN = pie(N+1)`.
    v.set("git0", v.c("cScale1").darken(25.0));
    for i in 1..8 {
        let g = v.n("cScale", i + 1);
        v.set(format!("git{i}"), g);
    }
    git_inverses(v);
    let branch = v.get("branchLabelColor").unwrap_or(text);
    for i in 0..8 {
        let c = if i == 1 || i == 3 {
            hex("white")
        } else {
            branch
        };
        v.set(format!("gitBranchLabel{i}"), c);
    }
}

fn update_base(v: &mut Vars) {
    let p = v.c("primaryColor");
    let dm = v.dark_mode;
    v.or("primaryTextColor", |_| {
        hex(if dm { "#eee" } else { "#333" })
    });
    v.or("secondaryColor", |_| p.hue(-120.0));
    v.or("tertiaryColor", |_| p.adjust(180.0, 0.0, 5.0));
    v.or("actorTextColor", |v| v.c("primaryTextColor"));
    v.or("labelTextColor", |v| v.c("actorTextColor"));
    scale_defaults(v, true);
    for i in 0..LIMIT {
        let s = v.n("cScale", i).darken(if dm { 75.0 } else { 25.0 });
        v.set(format!("cScale{i}"), s);
    }
    scale_invert_peer(v, dm);
    let label = v
        .get("scaleLabelColor")
        .unwrap_or_else(|| v.c("labelTextColor"));
    scale_labels(v, label);
    fill_types(v, true);
    git_from_primary(v);
    git_inverses(v);
    let branch = v.get("branchLabelColor").unwrap_or_else(|| {
        if dm {
            hex("black")
        } else {
            v.c("labelTextColor")
        }
    });
    for i in 0..8 {
        v.or(format!("gitBranchLabel{i}"), |_| branch);
    }
}

/// `pie1..12` of forest (`l` steps of 30 / 40 / 50), base (10 / 20) and dark
/// (`pieN = cScaleN`; `pie12` is left undefined upstream, `cScale0` stands in).
fn pie_colors(theme: ThemeName, v: &Vars) -> Option<Vec<Color>> {
    let (p, s, t) = (
        v.c("primaryColor"),
        v.c("secondaryColor"),
        v.c("tertiaryColor"),
    );
    let list: Vec<K> = match theme {
        ThemeName::Forest => vec![
            p,
            s,
            t,
            p.adjust(0.0, 0.0, -30.0),
            s.adjust(0.0, 0.0, -30.0),
            t.adjust(40.0, 0.0, -40.0),
            p.adjust(60.0, 0.0, -10.0),
            p.adjust(-60.0, 0.0, -10.0),
            p.adjust(120.0, 0.0, 0.0),
            p.adjust(60.0, 0.0, -50.0),
            p.adjust(-60.0, 0.0, -50.0),
            p.adjust(120.0, 0.0, -50.0),
        ],
        ThemeName::Base => vec![
            p,
            s,
            t,
            p.adjust(0.0, 0.0, -10.0),
            s.adjust(0.0, 0.0, -10.0),
            t.adjust(0.0, 0.0, -10.0),
            p.adjust(60.0, 0.0, -10.0),
            p.adjust(-60.0, 0.0, -10.0),
            p.adjust(120.0, 0.0, 0.0),
            p.adjust(60.0, 0.0, -20.0),
            p.adjust(-60.0, 0.0, -20.0),
            p.adjust(120.0, 0.0, -10.0),
        ],
        ThemeName::Dark => (1..=LIMIT).map(|i| v.n("cScale", i % LIMIT)).collect(),
        _ => return None,
    };
    Some(list.into_iter().map(K::to_color).collect())
}

/// The section colour sets of a classic theme, `themeVariables` applied; `None` for the
/// neo / redux family.
pub(super) fn section_colors(theme: ThemeName, vars: &[(String, String)]) -> Option<SectionColors> {
    let mut v = Vars {
        map: HashMap::new(),
        dark_mode: false,
    };
    let init: &[(&str, &str)] = match theme {
        ThemeName::Default | ThemeName::Null => &[
            ("primaryColor", "#ECECFF"),
            ("secondaryColor", "#ffffde"),
            ("actorTextColor", "black"),
        ],
        ThemeName::Forest => &[
            ("primaryColor", "#cde498"),
            ("secondaryColor", "#cdffb2"),
            ("actorTextColor", "black"),
        ],
        ThemeName::Dark => &[
            ("primaryColor", "#1f2020"),
            ("mainContrastColor", "lightgrey"),
        ],
        ThemeName::Neutral => &[("primaryColor", "#eee"), ("text", "#333")],
        ThemeName::Base => &[("primaryColor", "#fff4dd")],
        _ => return None,
    };
    for (k, c) in init {
        v.set(*k, hex(c));
    }
    match theme {
        ThemeName::Default | ThemeName::Null => {
            let t = v.c("primaryColor").hue(-160.0);
            v.set("tertiaryColor", t);
        }
        ThemeName::Forest => v.set("tertiaryColor", hex("#cde498").lighten(10.0)),
        ThemeName::Dark => {
            let p = v.c("primaryColor");
            v.set("secondaryColor", p.lighten(16.0));
            v.set("tertiaryColor", p.hue(-160.0));
        }
        ThemeName::Neutral => {
            v.set("secondaryColor", hex("#707070").lighten(55.0));
            let t = v.c("primaryColor").hue(-160.0);
            v.set("tertiaryColor", t);
        }
        _ => {}
    }
    let update: fn(&mut Vars) = match theme {
        ThemeName::Forest => update_forest,
        ThemeName::Dark => update_dark,
        ThemeName::Neutral => update_neutral,
        ThemeName::Base => update_base,
        _ => update_default,
    };
    // Only the default theme's constructor runs `updateColors()` itself.
    if matches!(theme, ThemeName::Default | ThemeName::Null) {
        update(&mut v);
    }
    let overrides: Vec<(String, K)> = vars
        .iter()
        .filter_map(|(k, val)| K::parse(val).map(|c| (k.clone(), c)))
        .collect();
    if let Some((_, d)) = vars.iter().rev().find(|(k, _)| k == "darkMode") {
        v.dark_mode = d.trim() == "true";
    }
    for (k, c) in &overrides {
        v.set(k.clone(), *c);
    }
    update(&mut v);
    for (k, c) in &overrides {
        v.set(k.clone(), *c);
    }
    let list = |v: &Vars, prefix: &str, n: usize| -> Vec<Color> {
        (0..n).map(|i| v.n(prefix, i).to_color()).collect()
    };
    let pie = pie_colors(theme, &v);
    Some(SectionColors {
        pie,
        scale: list(&v, "cScale", LIMIT),
        scale_peer: list(&v, "cScalePeer", LIMIT),
        scale_inv: list(&v, "cScaleInv", LIMIT),
        scale_label: list(&v, "cScaleLabel", LIMIT),
        fill_type: list(&v, "fillType", 8),
        git: list(&v, "git", 8),
        git_inv: list(&v, "gitInv", 8),
        git_branch_label: list(&v, "gitBranchLabel", 8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(theme: ThemeName, vars: &[(&str, &str)]) -> SectionColors {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        section_colors(theme, &vars).unwrap()
    }

    /// `(key, hex)` pairs checked against mermaid 12.3.0's computed `themeVariables`.
    fn check(theme: ThemeName, vars: &[(&str, &str)], expect: &[(&str, &str)]) {
        let s = sc(theme, vars);
        for (key, want) in expect {
            let (prefix, i) = key.split_at(key.find(|c: char| c.is_ascii_digit()).unwrap());
            let i: usize = i.parse().unwrap();
            let set = match prefix {
                "cScale" => &s.scale,
                "cScalePeer" => &s.scale_peer,
                "cScaleInv" => &s.scale_inv,
                "cScaleLabel" => &s.scale_label,
                "fillType" => &s.fill_type,
                "git" => &s.git,
                "gitInv" => &s.git_inv,
                "gitBranchLabel" => &s.git_branch_label,
                _ => unreachable!(),
            };
            assert_eq!(set[i].css(), *want, "{theme:?} {key}");
        }
    }

    #[test]
    fn default_theme_darkens_twice() {
        check(
            ThemeName::Default,
            &[],
            &[
                ("cScale0", "#8686ff"),
                ("cScale1", "#ffff78"),
                ("cScale2", "#d7ff86"),
                ("cScalePeer2", "#b5ff20"),
                ("cScaleInv0", "#ffffb9"),
                ("cScaleLabel0", "#ffffff"),
                ("cScaleLabel1", "#000000"),
                ("cScaleLabel3", "#ffffff"),
                ("fillType0", "#ececff"),
                ("fillType2", "#ffecfe"),
                ("fillType5", "#ffdee0"),
                ("git0", "#0000ec"),
                ("gitBranchLabel0", "#ffffff"),
            ],
        );
    }

    #[test]
    fn forest_theme() {
        check(
            ThemeName::Forest,
            &[],
            &[
                ("cScale0", "#b9d970"),
                ("cScale1", "#acff7f"),
                ("cScale8", "#c570d9"),
                ("cScalePeer2", "#8cb42f"),
                ("cScaleInv0", "#9070d9"),
                ("cScaleLabel0", "#000000"),
                ("fillType2", "#98e4b4"),
                ("git0", "#9bc834"),
            ],
        );
    }

    #[test]
    fn dark_theme() {
        check(
            ThemeName::Dark,
            &[],
            &[
                ("cScale0", "#1f2020"),
                ("cScale1", "#0b0000"),
                ("cScale8", "#154706"),
                ("cScalePeer2", "#771955"),
                ("cScaleInv0", "#e0dfdf"),
                ("cScaleLabel1", "#d3d3d3"),
                ("fillType5", "#474947"),
                ("git0", "#797d7d"),
                ("gitBranchLabel0", "#2c2c2c"),
            ],
        );
    }

    #[test]
    fn neutral_theme() {
        check(
            ThemeName::Neutral,
            &[],
            &[
                ("cScale0", "#555555"),
                ("cScale1", "#f4f4f4"),
                ("cScalePeer2", "#3b3b3b"),
                ("cScaleInv0", "#aaaaaa"),
                ("cScaleLabel0", "#f4f4f4"),
                ("cScaleLabel1", "#333333"),
                ("fillType5", "#fcfcfc"),
                ("git0", "#b4b4b4"),
                ("gitBranchLabel1", "#ffffff"),
            ],
        );
    }

    #[test]
    fn base_theme_keeps_hue_through_full_lightness() {
        check(
            ThemeName::Base,
            &[],
            &[
                ("cScale0", "#ffcb5d"),
                ("cScale1", "#cb5dff"),
                ("cScale2", "#77a3ff"),
                // adjust(primary, { h: 210, l: 150 }) is white with a hue; darken keeps it.
                ("cScale8", "#9680ff"),
                ("cScalePeer2", "#4480ff"),
                ("cScaleInv0", "#0034a2"),
                ("cScaleLabel0", "#333333"),
                ("fillType2", "#e6ffdd"),
                ("git0", "#ffcb5d"),
            ],
        );
    }

    #[test]
    fn pie_slots() {
        let pie = |t| sc(t, &[]).pie.unwrap();
        // theme-dark.js: `pieN = cScaleN`.
        assert_eq!(pie(ThemeName::Dark)[0].css(), "#0b0000");
        assert_eq!(pie(ThemeName::Dark)[1].css(), "#4d1037");
        // theme-forest.js: pie4 = adjust(primaryColor, { l: -30 }).
        assert_eq!(pie(ThemeName::Forest)[3].css(), "#8cb42f");
        assert!(sc(ThemeName::Default, &[]).pie.is_none());
    }

    #[test]
    fn overrides() {
        // An explicit cScaleN / fillTypeN wins unchanged (re-applied after updateColors).
        check(
            ThemeName::Default,
            &[("cScale3", "#123456"), ("fillType1", "#abcdef")],
            &[("cScale3", "#123456"), ("fillType1", "#abcdef")],
        );
        // The default theme's scale ignores primaryColor (derived in the constructor)...
        check(
            ThemeName::Default,
            &[("primaryColor", "#ff0000")],
            &[("cScale0", "#8686ff"), ("fillType0", "#ff0000")],
        );
        // ...while the base theme derives everything from it.
        check(
            ThemeName::Base,
            &[("primaryColor", "#ff0000")],
            &[("cScale0", "#800000"), ("fillType0", "#ff0000")],
        );
        // forest derives its scale from the overridden primary (single updateColors run).
        check(
            ThemeName::Forest,
            &[("primaryColor", "#ff0000")],
            &[("cScale0", "#cc0000")],
        );
    }

    #[test]
    fn dump_for_comparison() {
        if std::env::var("DUMP_SECTION_COLORS").is_err() {
            return;
        }
        for t in ["default", "forest", "dark", "neutral", "base"] {
            let s = sc(t.parse().unwrap(), &[]);
            for (name, set) in [
                ("cScale", &s.scale),
                ("cScalePeer", &s.scale_peer),
                ("cScaleInv", &s.scale_inv),
                ("cScaleLabel", &s.scale_label),
                ("fillType", &s.fill_type),
                ("git", &s.git),
                ("gitInv", &s.git_inv),
                ("gitBranchLabel", &s.git_branch_label),
            ] {
                for (i, c) in set.iter().enumerate() {
                    println!("{t} {name}{i} {}", c.css());
                }
            }
        }
    }
}
