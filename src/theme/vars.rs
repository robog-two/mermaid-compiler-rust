//! mermaid's theme objects (`theme-{base,default,dark,forest,neutral}.js`) for the
//! variables the renderers read: the constructor, `calculate(overrides)` (copy the
//! overrides in, `updateColors()`, copy them in again) and `updateColors()`,
//! transcribed statement by statement in the order upstream runs them, with
//! khroma's string arithmetic ([`super::khroma`]).
//!
//! The order matters as much as the formulas: a classic theme's `updateColors`
//! assigns most variables unconditionally from a handful of others (`border1`,
//! `secondBkg`, `textColor`, `mainContrastColor`, ...), so overriding
//! `primaryColor` does not recolour `mainBkg`, while overriding `textColor`
//! recolours the title and the sequence signals; base derives everything with
//! `||=` from the overridden primary colours.
//!
//! The statements are kept as text close to the JavaScript (`this.` dropped):
//! `name = expr`, `name ||= expr`, a leading `?dark` / `?light` for the
//! `darkMode` branches, and expressions of quoted literals, variables,
//! `a || b`, `c ? a : b` on `darkMode` (`darkMode ? a : b`) and the khroma calls
//! `lighten`, `darken`, `adjust(c, h, s, l)`, `invert` and `mkBorder`.

use super::ThemeName;
use super::khroma;
use std::collections::HashMap;

/// A theme object's string variables (absent = `undefined`).
pub(crate) struct ThemeVars {
    map: HashMap<String, String>,
}

impl ThemeVars {
    pub(crate) fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(String::as_str)
    }

    /// Runs more statements of the same language over the variables.
    pub(crate) fn extend(&mut self, program: &str) {
        self.run(program);
    }

    /// Every defined variable.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    fn truthy(&self, k: &str) -> bool {
        self.get(k)
            .is_some_and(|v| !v.is_empty() && v != "false" && v != "0")
    }

    fn run(&mut self, program: &str) {
        for line in program.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let line = if let Some(rest) = line.strip_prefix("?dark ") {
                if !self.truthy("darkMode") {
                    continue;
                }
                rest
            } else if let Some(rest) = line.strip_prefix("?calculated ") {
                // `x = x === "calculated" ? expr : x`.
                if self.get(rest.split(' ').next().unwrap_or("")) != Some("calculated") {
                    continue;
                }
                rest
            } else if let Some(rest) = line.strip_prefix("?light ") {
                if self.truthy("darkMode") {
                    continue;
                }
                rest
            } else {
                line
            };
            let (name, guarded, expr) = match line.split_once(" ||= ") {
                Some((n, e)) => (n, true, e),
                None => {
                    let (n, e) = line.split_once(" = ").expect("statement");
                    (n, false, e)
                }
            };
            if guarded && self.get(name).is_some_and(|v| !v.is_empty()) {
                continue;
            }
            match self.eval(expr) {
                Some(v) => {
                    self.map.insert(name.to_string(), v);
                }
                None => {
                    self.map.remove(name);
                }
            }
        }
    }

    /// `a || b || ...`, `darkMode ? a : b`.
    fn eval(&self, expr: &str) -> Option<String> {
        if let Some(rest) = expr.strip_prefix("darkMode ? ") {
            let (a, b) = split_top(rest, " : ").expect("ternary");
            return if self.truthy("darkMode") {
                self.eval(a)
            } else {
                self.eval(b)
            };
        }
        if let Some((a, b)) = split_top(expr, " || ") {
            return self
                .atom(a)
                .filter(|v| !v.is_empty())
                .or_else(|| self.eval(b));
        }
        self.atom(expr)
    }

    fn atom(&self, a: &str) -> Option<String> {
        let a = a.trim();
        if let Some(lit) = a.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            return Some(lit.to_string());
        }
        if let Some((f, rest)) = a.split_once('(') {
            let args_s = rest.strip_suffix(')').expect("call");
            let args: Vec<&str> = split_args(args_s);
            let c = self.eval(args[0])?;
            let n = |i: usize| {
                args.get(i)
                    .map_or(0.0, |s| s.trim().parse::<f64>().unwrap())
            };
            return Some(match f {
                "lighten" => khroma::lighten(&c, n(1)),
                "darken" => khroma::darken(&c, n(1)),
                "adjust" => khroma::adjust(&c, n(1), n(2), n(3)),
                "invert" => khroma::invert(&c),
                "mkBorder" => khroma::mk_border(&c, self.truthy("darkMode")),
                _ => panic!("unknown function {f}"),
            });
        }
        self.get(a).map(str::to_string)
    }
}

/// `s` split at the first `sep` outside parentheses and quotes.
fn split_top<'a>(s: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let (mut depth, mut quoted) = (0i32, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            _ if depth == 0 && !quoted && s[i..].starts_with(sep) => {
                return Some((&s[..i], &s[i + sep.len()..]));
            }
            _ => {}
        }
    }
    None
}

fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some((a, b)) = split_top(rest, ", ") {
        out.push(a);
        rest = b;
    }
    out.push(rest);
    out
}

const BASE_CONSTRUCTOR: &str = r##"
background = "#f4f4f4"
primaryColor = "#fff4dd"
noteBkgColor = "#fff5ad"
noteTextColor = "#333"
"##;

const BASE_UPDATE: &str = r##"
primaryTextColor ||= darkMode ? "#eee" : "#333"
secondaryColor ||= adjust(primaryColor, -120, 0, 0)
tertiaryColor ||= adjust(primaryColor, 180, 0, 5)
primaryBorderColor ||= mkBorder(primaryColor)
secondaryBorderColor ||= mkBorder(secondaryColor)
tertiaryBorderColor ||= mkBorder(tertiaryColor)
noteBorderColor ||= mkBorder(noteBkgColor)
noteBkgColor ||= "#fff5ad"
noteTextColor ||= "#333"
secondaryTextColor ||= invert(secondaryColor)
tertiaryTextColor ||= invert(tertiaryColor)
lineColor ||= invert(background)
arrowheadColor ||= invert(background)
textColor ||= primaryTextColor
border2 ||= tertiaryBorderColor
nodeBkg ||= primaryColor
mainBkg ||= primaryColor
nodeBorder ||= primaryBorderColor
clusterBkg ||= tertiaryColor
clusterBorder ||= tertiaryBorderColor
defaultLinkColor ||= lineColor
titleColor ||= tertiaryTextColor
edgeLabelBackground ||= darkMode ? darken(secondaryColor, 30) : secondaryColor
flowContainerStroke ||= secondaryBorderColor
nodeTextColor ||= primaryTextColor
actorBorder ||= primaryBorderColor
actorBkg ||= mainBkg
actorTextColor ||= primaryTextColor
actorLineColor ||= actorBorder
labelBoxBkgColor ||= actorBkg
signalColor ||= textColor
signalTextColor ||= textColor
labelBoxBorderColor ||= actorBorder
labelTextColor ||= actorTextColor
loopTextColor ||= actorTextColor
activationBorderColor ||= darken(secondaryColor, 10)
activationBkgColor ||= secondaryColor
sequenceNumberColor ||= invert(lineColor)
taskTextDarkColor ||= textColor
specialStateColor = lineColor
classText ||= textColor
pieTitleTextColor ||= taskTextDarkColor
pieSectionTextColor ||= textColor
pieLegendTextColor ||= taskTextDarkColor
gradientStart = primaryBorderColor
gradientStop = secondaryBorderColor
"##;

const DEFAULT_CONSTRUCTOR: &str = r##"
background = "#f4f4f4"
primaryColor = "#ECECFF"
secondaryColor = adjust(primaryColor, 120, 0, 0)
secondaryColor = "#ffffde"
tertiaryColor = adjust(primaryColor, -160, 0, 0)
primaryBorderColor = mkBorder(primaryColor)
secondaryBorderColor = mkBorder(secondaryColor)
tertiaryBorderColor = mkBorder(tertiaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(tertiaryColor)
lineColor = invert(background)
textColor = invert(background)
background = "white"
mainBkg = "#ECECFF"
secondBkg = "#ffffde"
lineColor = "#333333"
border1 = "#9370DB"
primaryBorderColor = mkBorder(primaryColor)
border2 = "#aaaa33"
labelBackground = "rgba(232,232,232, 0.8)"
textColor = "#333"
actorTextColor = "black"
noteBkgColor = "#fff5ad"
activationBorderColor = "#666"
activationBkgColor = "#f4f4f4"
sequenceNumberColor = "white"
taskTextDarkColor = "black"
gradientStart = primaryBorderColor
gradientStop = secondaryBorderColor
"##;

const DEFAULT_UPDATE: &str = r##"
nodeBkg = mainBkg
nodeBorder = border1
clusterBkg = secondBkg
clusterBorder = border2
titleColor = textColor
edgeLabelBackground = labelBackground
flowContainerStroke ||= secondaryBorderColor
actorBorder = border1
actorBkg = mainBkg
labelBoxBkgColor = actorBkg
signalColor = textColor
signalTextColor = textColor
labelBoxBorderColor = actorBorder
labelTextColor = actorTextColor
loopTextColor = actorTextColor
noteBorderColor = border2
noteTextColor = actorTextColor
actorLineColor = actorBorder
specialStateColor = lineColor
classText = primaryTextColor
pieTitleTextColor ||= taskTextDarkColor
pieSectionTextColor ||= textColor
pieLegendTextColor ||= taskTextDarkColor
"##;

const DARK_CONSTRUCTOR: &str = r##"
background = "#333"
primaryColor = "#1f2020"
secondaryColor = lighten(primaryColor, 16)
tertiaryColor = adjust(primaryColor, -160, 0, 0)
primaryBorderColor = invert(background)
secondaryBorderColor = mkBorder(secondaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(tertiaryColor)
lineColor = invert(background)
textColor = invert(background)
mainBkg = "#1f2020"
mainContrastColor = "lightgrey"
border1 = "#ccc"
border2 = "rgba(255, 255, 255, 0.25)"
labelBackground = "#181818"
textColor = "#ccc"
titleColor = "#F9FFFE"
noteBkgColor = "#fff5ad"
sequenceNumberColor = "black"
gradientStart = primaryBorderColor
gradientStop = secondaryBorderColor
"##;

const DARK_UPDATE: &str = r##"
secondBkg = lighten(mainBkg, 16)
lineColor = mainContrastColor
nodeBorder = border1
clusterBkg = secondBkg
clusterBorder = border2
edgeLabelBackground = lighten(labelBackground, 25)
flowContainerStroke ||= secondaryBorderColor
actorBorder = border1
actorBkg = mainBkg
actorTextColor = mainContrastColor
actorLineColor = actorBorder
signalColor = mainContrastColor
signalTextColor = mainContrastColor
labelBoxBkgColor = actorBkg
labelBoxBorderColor = actorBorder
labelTextColor = mainContrastColor
loopTextColor = mainContrastColor
noteBorderColor = secondaryBorderColor
noteBkgColor = secondBkg
noteTextColor = secondaryTextColor
activationBorderColor = border1
activationBkgColor = secondBkg
specialStateColor = "#f4f4f4"
pieTitleTextColor ||= mainContrastColor
pieSectionTextColor ||= textColor
pieLegendTextColor ||= mainContrastColor
classText = primaryTextColor
"##;

const FOREST_CONSTRUCTOR: &str = r##"
background = "#f4f4f4"
primaryColor = "#cde498"
secondaryColor = "#cdffb2"
background = "white"
mainBkg = "#cde498"
secondBkg = "#cdffb2"
lineColor = "green"
border1 = "#13540c"
border2 = "#6eaa49"
tertiaryColor = lighten("#cde498", 10)
primaryBorderColor = mkBorder(primaryColor)
secondaryBorderColor = mkBorder(secondaryColor)
tertiaryBorderColor = mkBorder(tertiaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(primaryColor)
lineColor = invert(background)
textColor = invert(background)
titleColor = "#333"
edgeLabelBackground = "#e8e8e8"
actorTextColor = "black"
signalColor = "#333"
signalTextColor = "#333"
labelBoxBorderColor = "#326932"
noteBkgColor = "#fff5ad"
activationBorderColor = "#666"
activationBkgColor = "#f4f4f4"
sequenceNumberColor = "white"
taskTextDarkColor = "black"
gradientStart = primaryBorderColor
gradientStop = secondaryBorderColor
"##;

const FOREST_UPDATE: &str = r##"
actorBorder = darken(mainBkg, 20)
actorBkg = mainBkg
labelBoxBkgColor = actorBkg
labelTextColor = actorTextColor
loopTextColor = actorTextColor
noteBorderColor = border2
noteTextColor = actorTextColor
actorLineColor = actorBorder
nodeBkg = mainBkg
nodeBorder = border1
clusterBkg = secondBkg
clusterBorder = border2
flowContainerStroke ||= secondaryBorderColor
specialStateColor = lineColor
classText = primaryTextColor
pieTitleTextColor ||= taskTextDarkColor
pieSectionTextColor ||= textColor
pieLegendTextColor ||= taskTextDarkColor
"##;

const NEUTRAL_CONSTRUCTOR: &str = r##"
contrast = "#707070"
primaryColor = "#eee"
secondaryColor = lighten(contrast, 55)
background = "#ffffff"
tertiaryColor = adjust(primaryColor, -160, 0, 0)
primaryBorderColor = mkBorder(primaryColor)
secondaryBorderColor = mkBorder(secondaryColor)
tertiaryBorderColor = mkBorder(tertiaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(tertiaryColor)
lineColor = invert(background)
textColor = invert(background)
mainBkg = "#eee"
lineColor = "#666"
border1 = "#999"
text = "#333"
edgeLabelBackground = "white"
activationBorderColor = "#666"
activationBkgColor = "#f4f4f4"
sequenceNumberColor = "white"
gradientStart = primaryBorderColor
gradientStop = secondaryBorderColor
"##;

const NEUTRAL_UPDATE: &str = r##"
secondBkg = lighten(contrast, 55)
border2 = contrast
actorBorder = lighten(border1, 23)
actorBkg = mainBkg
actorTextColor = text
actorLineColor = actorBorder
signalColor = text
signalTextColor = text
labelBoxBkgColor = actorBkg
labelBoxBorderColor = actorBorder
labelTextColor = text
loopTextColor = text
noteBorderColor = "#999"
noteBkgColor = "#666"
noteTextColor = "#fff"
nodeBkg = mainBkg
nodeBorder = border1
clusterBkg = secondBkg
clusterBorder = border2
titleColor = text
flowContainerStroke ||= secondaryBorderColor
taskTextDarkColor = text
specialStateColor = "#222"
classText = primaryTextColor
pieTitleTextColor ||= taskTextDarkColor
pieSectionTextColor ||= textColor
pieLegendTextColor ||= taskTextDarkColor
"##;

const NEO_UPDATE: &str = r##"
primaryTextColor ||= darkMode ? "#eee" : "@TEXT"
secondaryColor ||= adjust(primaryColor, -120, 0, 0)
tertiaryColor ||= adjust(primaryColor, 180, 0, 5)
primaryBorderColor ||= mkBorder(primaryColor)
secondaryBorderColor ||= mkBorder(secondaryColor)
tertiaryBorderColor ||= mkBorder(tertiaryColor)
noteBorderColor ||= mkBorder(noteBkgColor)
noteBkgColor ||= "@NOTEBKG"
noteTextColor ||= "@NOTETEXT"
secondaryTextColor ||= invert(secondaryColor)
tertiaryTextColor ||= invert(tertiaryColor)
lineColor ||= invert(background)
arrowheadColor ||= invert(background)
textColor ||= primaryTextColor
border2 ||= tertiaryBorderColor
nodeBkg ||= primaryColor
mainBkg ||= primaryColor
@SECONDBKG
nodeBorder ||= @NODEBORDER
clusterBkg ||= tertiaryColor
clusterBorder ||= tertiaryBorderColor
defaultLinkColor ||= lineColor
titleColor ||= tertiaryTextColor
edgeLabelBackground ||= darkMode ? darken(secondaryColor, 30) : secondaryColor
nodeTextColor ||= primaryTextColor
flowContainerStroke ||= secondaryBorderColor
@REDUXDARK
actorBorder ||= primaryBorderColor
actorBkg ||= mainBkg
actorTextColor ||= primaryTextColor
actorLineColor ||= actorBorder
labelBoxBkgColor ||= actorBkg
signalColor ||= textColor
signalTextColor ||= textColor
labelBoxBorderColor ||= actorBorder
labelTextColor ||= actorTextColor
loopTextColor ||= actorTextColor
activationBorderColor ||= darken(secondaryColor, 10)
activationBkgColor ||= secondaryColor
sequenceNumberColor ||= invert(lineColor)
taskTextDarkColor ||= textColor
specialStateColor = lineColor
classText ||= textColor
pieTitleTextColor ||= taskTextDarkColor
pieSectionTextColor ||= @PIESECTION
pieLegendTextColor ||= taskTextDarkColor
"##;

const NEO_CONSTRUCTOR: &str = r##"
background = "#ffffff"
primaryColor = "#cccccc"
mainBkg = "#ffffff"
noteBkgColor = "#fff5ad"
noteTextColor = "#333"
primaryBorderColor = mkBorder(primaryColor)
nodeBorder = "#000000"
tertiaryColor = "#ffffff"
"##;

const REDUX_CONSTRUCTOR: &str = r##"
background = "#ffffff"
primaryColor = "#cccccc"
mainBkg = "#ffffff"
noteBkgColor = "#fff5ad"
noteTextColor = "#28253D"
primaryBorderColor = mkBorder("#28253D")
nodeBorder = "#28253D"
tertiaryColor = "#ffffff"
clusterBkg = "#F9F9FB"
clusterBorder = "#BDBCCC"
noteBorderColor = "#FACC15"
actorBorder = "#28253D"
"##;

const NEO_DARK_CONSTRUCTOR: &str = r##"
background = "#333"
primaryColor = "#1f2020"
secondaryColor = lighten(primaryColor, 16)
tertiaryColor = adjust(primaryColor, -160, 0, 0)
primaryBorderColor = invert(background)
secondaryBorderColor = mkBorder(secondaryColor)
tertiaryBorderColor = mkBorder(tertiaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(tertiaryColor)
mainBkg = "#2a2020"
secondBkg = "calculated"
mainContrastColor = "lightgrey"
border1 = "#ccc"
border2 = "rgba(255, 255, 255, 0.25)"
arrowheadColor = invert(background)
labelBackground = "#181818"
textColor = "#ccc"
noteBkgColor = "#fff5ad"
noteTextColor = "#333"
"##;

const REDUX_DARK_CONSTRUCTOR: &str = r##"
background = "#333"
primaryColor = "#1f2020"
secondaryColor = lighten(primaryColor, 16)
tertiaryColor = adjust(primaryColor, -160, 0, 0)
primaryBorderColor = invert(background)
secondaryBorderColor = mkBorder(secondaryColor)
tertiaryBorderColor = mkBorder(tertiaryColor)
primaryTextColor = invert(primaryColor)
secondaryTextColor = invert(secondaryColor)
tertiaryTextColor = invert(tertiaryColor)
mainBkg = "#111113"
secondBkg = "calculated"
mainContrastColor = "lightgrey"
border1 = "#ccc"
border2 = "rgba(255, 255, 255, 0.25)"
arrowheadColor = invert(background)
labelBackground = "#111113"
textColor = "#ccc"
noteBkgColor ||= "#FEF9C3"
noteTextColor ||= "#28253D"
nodeBorder = "#FFFFFF"
clusterBkg = "#1E1A2E"
clusterBorder = "#BDBCCC"
noteBorderColor = "#FACC15"
"##;

/// The neo family's shared `updateColors()` with its per-theme differences
/// (the dark variants' `secondBkg` and `nodeBorder`, the redux dark ones'
/// fixed actor and signal colours, ...).
fn neo_update(theme: ThemeName) -> String {
    let dark = matches!(
        theme,
        ThemeName::NeoDark | ThemeName::ReduxDark | ThemeName::ReduxDarkColor
    );
    let redux_dark = matches!(theme, ThemeName::ReduxDark | ThemeName::ReduxDarkColor);
    let (text, note_bkg, note_text) = match theme {
        ThemeName::Redux => ("#28253D", "#FEF9C3", "#28253D"),
        ThemeName::ReduxColor => ("#28253D", "#fff5ad", "#28253D"),
        ThemeName::ReduxDark | ThemeName::ReduxDarkColor => ("#FFFFFF", "#fff5ad", "#FFFFFF"),
        _ => ("#333", "#fff5ad", "#333"),
    };
    NEO_UPDATE
        .replace("@TEXT", text)
        .replace("@NOTEBKG", note_bkg)
        .replace("@NOTETEXT", note_text)
        // `secondBkg === "calculated" ? lighten(mainBkg, 16) : secondBkg`.
        .replace(
            "@SECONDBKG",
            if dark {
                "?calculated secondBkg = lighten(mainBkg, 16)"
            } else {
                ""
            },
        )
        .replace("@NODEBORDER", if dark { "border1" } else { "primaryBorderColor" })
        .replace(
            "@REDUXDARK",
            if redux_dark {
                "actorBorder = \"#FFFFFF\"\nsignalColor = \"#FFFFFF\"\nlabelBoxBorderColor = \"#BDBCCC\""
            } else {
                ""
            },
        )
        .replace(
            "@PIESECTION",
            if theme == ThemeName::ReduxDarkColor {
                "\"#28253D\""
            } else {
                "textColor"
            },
        )
}

/// The resolved variables of a theme, with
/// `themeVariables` overrides as mermaid applies them.
pub(crate) fn resolve(theme: ThemeName, overrides: &[(String, String)]) -> Option<ThemeVars> {
    let neo_program;
    let (ctor, update, ctor_updates) = match theme {
        ThemeName::Base => (BASE_CONSTRUCTOR, BASE_UPDATE, false),
        ThemeName::Default | ThemeName::Null => (DEFAULT_CONSTRUCTOR, DEFAULT_UPDATE, true),
        ThemeName::Dark => (DARK_CONSTRUCTOR, DARK_UPDATE, false),
        ThemeName::Forest => (FOREST_CONSTRUCTOR, FOREST_UPDATE, false),
        ThemeName::Neutral => (NEUTRAL_CONSTRUCTOR, NEUTRAL_UPDATE, false),
        ThemeName::Neo
        | ThemeName::NeoDark
        | ThemeName::Redux
        | ThemeName::ReduxDark
        | ThemeName::ReduxColor
        | ThemeName::ReduxDarkColor => {
            neo_program = neo_update(theme);
            let ctor = match theme {
                ThemeName::Neo => NEO_CONSTRUCTOR,
                ThemeName::NeoDark => NEO_DARK_CONSTRUCTOR,
                ThemeName::Redux | ThemeName::ReduxColor => REDUX_CONSTRUCTOR,
                _ => REDUX_DARK_CONSTRUCTOR,
            };
            (ctor, neo_program.as_str(), false)
        }
    };
    let mut v = ThemeVars {
        map: HashMap::new(),
    };
    v.run(ctor);
    // Only the default theme's constructor runs `updateColors()` itself.
    if ctor_updates {
        v.run(update);
    }
    // `calculate`: the overrides, `updateColors()`, the overrides again. Later
    // duplicates win, as in an object literal.
    let mut last: Vec<(&str, &str)> = Vec::new();
    for (k, val) in overrides {
        last.retain(|(n, _)| *n != k);
        last.push((k, val.trim()));
    }
    for (k, val) in &last {
        v.map.insert(k.to_string(), val.to_string());
    }
    v.run(update);
    for (k, val) in &last {
        v.map.insert(k.to_string(), val.to_string());
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(t: ThemeName, vars: &[(&str, &str)], k: &str) -> String {
        let o: Vec<(String, String)> = vars
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        resolve(t, &o).unwrap().get(k).unwrap().to_string()
    }

    /// Values mermaid 12.3.0 resolves (`getConfig().themeVariables`).
    #[test]
    fn classic_themes_resolve_as_mermaid() {
        assert_eq!(
            get(ThemeName::Base, &[], "titleColor"),
            "rgb(8.5000000002, 5.7500000001, 0)"
        );
        assert_eq!(
            get(ThemeName::Base, &[], "clusterBorder"),
            "hsl(220.5882352941, 60%, 88.3333333333%)"
        );
        assert_eq!(
            get(
                ThemeName::Base,
                &[("darkMode", "true"), ("primaryColor", "#00ff00")],
                "edgeLabelBackground"
            ),
            "hsl(0, 100%, 20%)"
        );
        assert_eq!(
            get(ThemeName::Base, &[("lineColor", "#0000aa")], "signalColor"),
            "#333"
        );
        assert_eq!(
            get(
                ThemeName::Default,
                &[("primaryColor", "#4a90e2")],
                "mainBkg"
            ),
            "#ECECFF"
        );
        assert_eq!(
            get(
                ThemeName::Default,
                &[("textColor", "#550055")],
                "titleColor"
            ),
            "#550055"
        );
        assert_eq!(
            get(
                ThemeName::Default,
                &[("lineColor", "#0000aa")],
                "specialStateColor"
            ),
            "#0000aa"
        );
        assert_eq!(
            get(ThemeName::Dark, &[("mainBkg", "#fedcba")], "noteBkgColor"),
            "hsl(30, 97.1428571429%, 100%)"
        );
        assert_eq!(
            get(ThemeName::Forest, &[("mainBkg", "#fedcba")], "actorBorder"),
            "hsl(30, 97.1428571429%, 66.2745098039%)"
        );
        assert_eq!(
            get(ThemeName::Neutral, &[], "actorBorder"),
            "hsl(0, 0%, 83%)"
        );
        assert_eq!(get(ThemeName::Neutral, &[], "classText"), "#111111");
    }
}
