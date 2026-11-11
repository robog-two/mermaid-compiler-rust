//! mermaid themes and `themeVariables`.

mod classic;
mod neo;

use crate::color::{self, Rgba};
use crate::scene::Color;
use std::str::FromStr;

/// Theme names accepted by mermaid-cli 12 (`-t/--theme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Base,
    Dark,
    Default,
    Forest,
    Neutral,
    Neo,
    NeoDark,
    Redux,
    ReduxDark,
    ReduxColor,
    ReduxDarkColor,
    Null,
}

impl ThemeName {
    /// All names in mermaid-cli's `choices()` order.
    pub const ALL: [(&'static str, ThemeName); 12] = [
        ("base", ThemeName::Base),
        ("dark", ThemeName::Dark),
        ("default", ThemeName::Default),
        ("forest", ThemeName::Forest),
        ("neutral", ThemeName::Neutral),
        ("neo", ThemeName::Neo),
        ("neo-dark", ThemeName::NeoDark),
        ("redux", ThemeName::Redux),
        ("redux-dark", ThemeName::ReduxDark),
        ("redux-color", ThemeName::ReduxColor),
        ("redux-dark-color", ThemeName::ReduxDarkColor),
        ("null", ThemeName::Null),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, t)| *t == self)
            .map(|(n, _)| *n)
            .unwrap_or("default")
    }
}

impl FromStr for ThemeName {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Self::ALL
            .iter()
            .find(|(n, _)| *n == s)
            .map(|(_, t)| *t)
            .ok_or(())
    }
}

/// Resolved colours and fonts used by the renderers.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub font_family: String,
    pub font_size: f64,
    pub background: Color,
    pub main_bkg: Color,
    pub node_border: Color,
    pub node_text: Color,
    pub line_color: Color,
    pub text_color: Color,
    pub title_color: Color,
    pub cluster_bkg: Color,
    pub cluster_border: Color,
    pub edge_label_background: Color,
    pub actor_bkg: Color,
    pub actor_border: Color,
    pub actor_text: Color,
    pub actor_line: Color,
    pub signal_color: Color,
    pub signal_text: Color,
    pub label_box_bkg: Color,
    pub label_box_border: Color,
    pub label_text: Color,
    pub loop_text: Color,
    pub note_bkg: Color,
    pub note_border: Color,
    pub note_text: Color,
    pub activation_bkg: Color,
    pub activation_border: Color,
    pub sequence_number: Color,
    pub special_state: Color,
    pub pie_colors: Vec<Color>,
    pub pie_stroke: Color,
    pub pie_stroke_width: f64,
    pub pie_outer_stroke: Color,
    pub pie_outer_stroke_width: f64,
    pub pie_opacity: f64,
    pub pie_title_text: Color,
    pub pie_section_text: Color,
    pub pie_legend_text: Color,
    /// mermaid's `cScale0..11`: section colours of timeline, mindmap, kanban, treemap, radar.
    pub scale: Vec<Color>,
    /// `cScaleLabel0..11`: text on each `scale` entry.
    pub scale_text: Vec<Color>,
    /// `cScalePeer0..11`: borders of `scale` entries (treemap sections).
    pub scale_peer: Vec<Color>,
    /// `cScaleInv0..11`: the underline of timeline / mindmap / kanban nodes.
    pub scale_inv: Vec<Color>,
    /// `fillType0..7`: user-journey section and task fills.
    pub fill_type: Vec<Color>,
    /// `git0..7`, `gitInv0..7`, `gitBranchLabel0..7` (empty: the git graph derives its own).
    pub git: Vec<Color>,
    pub git_inv: Vec<Color>,
    pub git_branch_label: Vec<Color>,
    /// Whether this is a dark theme.
    pub dark: bool,
    /// Theme `strokeWidth` (neo/redux family). `None` means each diagram's own default.
    pub stroke_width: Option<f64>,
    /// Theme `radius`: corner radius of flowchart rounded nodes (`None` = mermaid's 5) and,
    /// under the neo look, of state composites.
    pub radius: Option<f64>,
    /// Categorical palette of the colour themes (`redux-color`, `redux-dark-color`);
    /// `None` for every other theme (mermaid's `isColorTheme` gate).
    pub color_slots: Option<CategoricalPalette>,
    /// ER attribute-row stripe (`rowOdd`); `None` when the theme leaves rows unfilled.
    pub er_row_odd: Option<Color>,
    /// ER edge-label background used by the colour themes (`erEdgeLabelBackground`).
    pub er_edge_label_background: Option<Color>,
    /// The drop shadow the neo look applies to nodes and clusters (`dropShadow`).
    pub drop_shadow: Option<DropShadow>,
    /// State composites: `compositeTitleBackground` (`None` = `main_bkg`),
    /// `compositeBackground` (`None` = `background`) and `altBackground` (`None` = `#f0f0f0`).
    pub composite_title_background: Option<Color>,
    pub composite_background: Option<Color>,
    pub alt_background: Option<Color>,
}

/// A palette slot (`colorIndex` upstream): the n-th participant of a diagram that takes
/// part in the colour cycle. Wraps modulo the palette length when resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorSlot(pub usize);

/// `borderColorArray` / `bkgColorArray`.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoricalPalette {
    /// Never empty (an empty border palette disables per-item colour upstream).
    pub border: Vec<Color>,
    /// May be empty (`redux-dark-color`): then only outlines take colour.
    pub bkg: Vec<Color>,
}

/// Concrete colours of one slot.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotColors {
    pub border: Color,
    /// `None` when the theme has no background palette (keep the diagram's own fill).
    pub fill: Option<Color>,
}

/// A hard drop shadow: an offset, translucent copy of the shape's silhouette.
///
/// The redux themes' `feDropShadow` has `stdDeviation="0"`, so a solid offset copy
/// reproduces it exactly in every backend. The neo themes use a CSS `drop-shadow()`
/// with a 2px blur, which is approximated without the blur.
#[derive(Debug, Clone, PartialEq)]
pub struct DropShadow {
    pub dx: f64,
    pub dy: f64,
    pub color: Color,
    pub opacity: f64,
}

impl CategoricalPalette {
    /// Colours of `slot`: `colorIndex % len` for each array independently, exactly as
    /// `stampColorSlot` + the per-diagram `genColor` stylesheets (and the sequence
    /// diagram's `paletteColor`) resolve it.
    pub fn slot(&self, slot: ColorSlot) -> SlotColors {
        SlotColors {
            border: self.border[slot.0 % self.border.len()].clone(),
            fill: (!self.bkg.is_empty()).then(|| self.bkg[slot.0 % self.bkg.len()].clone()),
        }
    }
}

fn c(s: &str) -> Color {
    Color::new(s)
}

fn hex(r: Rgba) -> Color {
    Color::new(r.to_hex())
}

fn default_palette() -> Palette {
    Palette {
        font_family: "\"trebuchet ms\", verdana, arial, sans-serif".into(),
        font_size: 16.0,
        background: c("white"),
        main_bkg: c("#ECECFF"),
        node_border: c("#9370DB"),
        node_text: c("#333"),
        line_color: c("#333333"),
        text_color: c("#333"),
        title_color: c("#333"),
        cluster_bkg: c("#ffffde"),
        cluster_border: c("#aaaa33"),
        edge_label_background: c("rgba(232,232,232, 0.8)"),
        actor_bkg: c("#ECECFF"),
        actor_border: c("hsl(259.6261682243, 59.7765363128%, 87.9019607843%)"),
        actor_text: c("black"),
        actor_line: c("grey"),
        signal_color: c("#333"),
        signal_text: c("#333"),
        label_box_bkg: c("#ECECFF"),
        label_box_border: c("hsl(259.6261682243, 59.7765363128%, 87.9019607843%)"),
        label_text: c("black"),
        loop_text: c("black"),
        note_bkg: c("#fff5ad"),
        note_border: c("#aaaa33"),
        note_text: c("black"),
        activation_bkg: c("#f4f4f4"),
        activation_border: c("#666"),
        sequence_number: c("white"),
        special_state: c("#333333"),
        pie_colors: [
            "#ECECFF",
            "#ffffde",
            "hsl(80, 100%, 56.2745098039%)",
            "hsl(240, 100%, 86.2745098039%)",
            "hsl(60, 100%, 63.5294117647%)",
            "hsl(80, 100%, 76.2745098039%)",
            "hsl(300, 100%, 76.2745098039%)",
            "hsl(180, 100%, 56.2745098039%)",
            "hsl(0, 100%, 56.2745098039%)",
            "hsl(300, 100%, 56.2745098039%)",
            "hsl(150, 100%, 56.2745098039%)",
            "hsl(0, 100%, 66.2745098039%)",
        ]
        .iter()
        .map(|s| c(s))
        .collect(),
        pie_stroke: c("black"),
        pie_stroke_width: 2.0,
        pie_outer_stroke: c("black"),
        pie_outer_stroke_width: 2.0,
        pie_opacity: 0.7,
        pie_title_text: c("black"),
        pie_section_text: c("#333"),
        pie_legend_text: c("black"),
        scale: Vec::new(),
        scale_text: Vec::new(),
        scale_peer: Vec::new(),
        scale_inv: Vec::new(),
        fill_type: Vec::new(),
        git: Vec::new(),
        git_inv: Vec::new(),
        git_branch_label: Vec::new(),
        dark: false,
        stroke_width: None,
        radius: None,
        color_slots: None,
        er_row_odd: Some(c("white")),
        er_edge_label_background: None,
        drop_shadow: None,
        composite_title_background: None,
        composite_background: None,
        alt_background: None,
    }
}

fn forest_palette() -> Palette {
    let mut p = default_palette();
    p.main_bkg = c("#cde498");
    p.node_border = c("#13540c");
    p.node_text = c("#000000");
    p.line_color = c("green");
    p.text_color = c("#000000");
    p.title_color = c("#333");
    p.cluster_bkg = c("#cdffb2");
    p.cluster_border = c("#6eaa49");
    p.edge_label_background = c("#e8e8e8");
    p.actor_bkg = c("#cde498");
    p.actor_border = c("#13540c");
    p.label_box_bkg = c("#cde498");
    p.label_box_border = c("#326932");
    p.signal_color = c("#333");
    p.activation_bkg = c("#f4f4f4");
    p.special_state = c("#13540c");
    // `pie1..12`: see `classic::pie_colors`.
    p
}

fn dark_palette() -> Palette {
    let mut p = default_palette();
    p.alt_background = Some(c("#555"));
    p.background = c("#333");
    p.main_bkg = c("#1f2020");
    p.node_border = c("#ccc");
    p.node_text = c("#ccc");
    p.line_color = c("lightgrey");
    p.text_color = c("#ccc");
    p.title_color = c("#F9FFFE");
    p.cluster_bkg = c("hsl(180, 1.5873015873%, 28.3529411765%)");
    p.cluster_border = c("rgba(255, 255, 255, 0.25)");
    p.edge_label_background = c("hsl(0, 0%, 34.4117647059%)");
    p.actor_bkg = c("#1f2020");
    p.actor_border = c("#ccc");
    p.actor_text = c("lightgrey");
    p.actor_line = c("#ccc");
    p.signal_color = c("lightgrey");
    p.signal_text = c("lightgrey");
    p.label_box_bkg = c("#1f2020");
    p.label_box_border = c("#ccc");
    p.label_text = c("lightgrey");
    p.loop_text = c("lightgrey");
    p.activation_bkg = c("hsl(180, 1.5873015873%, 28.3529411765%)");
    p.activation_border = c("#ccc");
    p.sequence_number = c("black");
    p.special_state = c("#ccc");
    // `pie1..12`: see `classic::pie_colors`.
    // theme-dark.js: `pieStrokeColor ||= 'black'`, `pieOuterStrokeColor ||= 'black'`.
    p.pie_title_text = c("lightgrey");
    p.pie_section_text = c("lightgrey");
    p.pie_legend_text = c("lightgrey");
    p
}

fn neutral_palette() -> Palette {
    let mut p = default_palette();
    p.alt_background = Some(c("#f4f4f4"));
    p.main_bkg = c("#eee");
    p.node_border = c("#999");
    p.node_text = c("#333");
    p.line_color = c("#666");
    p.text_color = c("#333");
    p.cluster_bkg = c("hsl(0, 0%, 96.0784313725%)");
    p.cluster_border = c("#aaa");
    p.edge_label_background = c("white");
    p.actor_bkg = c("#eee");
    p.actor_border = c("#999");
    p.actor_line = c("#666");
    p.label_box_bkg = c("#eee");
    p.label_box_border = c("#999");
    p.signal_color = c("#333");
    p.note_bkg = c("#666");
    p.note_border = c("#999");
    p.note_text = c("#fff");
    p.special_state = c("#222");
    p.pie_colors = [
        "#F4F4F4", "#555", "#BBB", "#777", "#999", "#DDD", "#FFF", "#DDD", "#BBB", "#999", "#777",
        "#555",
    ]
    .iter()
    .map(|s| c(s))
    .collect();
    p
}

/// `base` theme: everything derives from `primaryColor` (mermaid's documented model).
fn base_palette(vars: &[(String, String)]) -> Palette {
    let get = |k: &str| {
        vars.iter()
            .rev()
            .find(|(n, _)| n == k)
            .and_then(|(_, v)| color::parse(v))
    };
    let primary = get("primaryColor").unwrap_or(Rgba {
        r: 0xff,
        g: 0xf4,
        b: 0xdd,
        a: 1.0,
    });
    let dark_mode = vars.iter().any(|(k, v)| k == "darkMode" && v == "true");
    let secondary = get("secondaryColor").unwrap_or(primary.rotate_hue(-120.0));
    let tertiary = get("tertiaryColor").unwrap_or(primary.rotate_hue(180.0).lighten(5.0));
    let border =
        get("primaryBorderColor").unwrap_or(primary.lighten(if dark_mode { 10.0 } else { -40.0 }));
    let text = get("primaryTextColor").unwrap_or(if primary.is_dark() {
        Rgba::WHITE
    } else {
        Rgba {
            r: 0x33,
            g: 0x33,
            b: 0x33,
            a: 1.0,
        }
    });
    let line = get("lineColor").unwrap_or(if primary.is_dark() {
        Rgba::WHITE
    } else {
        Rgba {
            r: 0x33,
            g: 0x33,
            b: 0x33,
            a: 1.0,
        }
    });
    let mut p = default_palette();
    p.main_bkg = hex(primary);
    p.node_border = hex(border);
    p.node_text = hex(text);
    p.text_color = hex(text);
    p.title_color = hex(text);
    p.line_color = hex(line);
    p.cluster_bkg = hex(secondary);
    p.cluster_border = hex(secondary.lighten(-40.0));
    p.edge_label_background = hex(secondary);
    p.actor_bkg = hex(primary);
    p.actor_border = hex(border);
    p.actor_text = hex(text);
    p.signal_color = hex(line);
    p.signal_text = hex(text);
    p.label_box_bkg = hex(primary);
    p.label_box_border = hex(border);
    p.note_bkg = hex(tertiary);
    p.note_border = hex(tertiary.lighten(-40.0));
    p.special_state = hex(line);
    p
}

/// Build the palette for a theme, then apply `themeVariables` overrides.
pub fn palette(theme: ThemeName, vars: &[(String, String)]) -> Palette {
    let mut p = match theme {
        ThemeName::Base => base_palette(vars),
        ThemeName::Dark => dark_palette(),
        ThemeName::Forest => forest_palette(),
        ThemeName::Neutral => neutral_palette(),
        ThemeName::Default | ThemeName::Null => default_palette(),
        ThemeName::Neo
        | ThemeName::NeoDark
        | ThemeName::Redux
        | ThemeName::ReduxDark
        | ThemeName::ReduxColor
        | ThemeName::ReduxDarkColor => neo::palette(theme, default_palette()),
    };
    if p.er_row_odd.is_some() {
        // The classic themes stripe odd ER rows with (nearly) the canvas colour.
        p.er_row_odd = Some(p.background.clone());
    }
    p.dark = matches!(
        theme,
        ThemeName::Dark | ThemeName::NeoDark | ThemeName::ReduxDark | ThemeName::ReduxDarkColor
    ) || color::parse(p.background.css()).is_some_and(Rgba::is_dark);
    if let Some(sc) = classic::section_colors(theme, vars) {
        // Every override is already folded in, exactly as upstream re-applies them.
        p.scale = sc.scale;
        p.scale_text = sc.scale_label;
        p.scale_peer = sc.scale_peer;
        p.scale_inv = sc.scale_inv;
        p.fill_type = sc.fill_type;
        p.git = sc.git;
        p.git_inv = sc.git_inv;
        p.git_branch_label = sc.git_branch_label;
        if let Some(pie) = sc.pie {
            p.pie_colors = pie;
        }
    }
    for (k, v) in vars {
        apply_variable(&mut p, k, v);
    }
    // The neo / redux family: companions not ported from those themes.
    if p.scale_text.is_empty() {
        p.scale_text = p
            .scale
            .iter()
            .map(|c| match color::parse(c.css()) {
                Some(rgb) if rgb.is_dark() => Color::new("#ffffff"),
                _ => Color::new("#333333"),
            })
            .collect();
    }
    let derived = |f: fn(Rgba) -> Rgba, from: &[Color]| -> Vec<Color> {
        from.iter()
            .map(|c| color::parse(c.css()).map_or_else(|| c.clone(), |x| hex(f(x))))
            .collect()
    };
    if p.scale_peer.is_empty() {
        // `cScalePeerN ||= darken(cScaleN, 10)` (none of the family sets darkMode).
        p.scale_peer = derived(|x| x.darken(10.0), &p.scale);
    }
    if p.scale_inv.is_empty() {
        p.scale_inv = derived(Rgba::invert, &p.scale);
    }
    p
}

impl Palette {
    /// Section colour `i` (`cScale`, wraps around).
    pub fn scale_color(&self, i: usize) -> &Color {
        &self.scale[i % self.scale.len()]
    }
    /// Text colour on section colour `i` (`cScaleLabel`).
    pub fn scale_text_color(&self, i: usize) -> &Color {
        &self.scale_text[i % self.scale_text.len()]
    }
    /// Border colour of section colour `i` (`cScalePeer`).
    pub fn scale_peer_color(&self, i: usize) -> &Color {
        &self.scale_peer[i % self.scale_peer.len()]
    }
    /// Inverse of section colour `i` (`cScaleInv`).
    pub fn scale_inv_color(&self, i: usize) -> &Color {
        &self.scale_inv[i % self.scale_inv.len()]
    }
    /// User-journey section / task fill `i` (`fillType`, wraps around); falls back to the
    /// section scale for a palette without journey colours.
    pub fn fill_type_color(&self, i: usize) -> &Color {
        if self.fill_type.is_empty() {
            return self.scale_color(i);
        }
        &self.fill_type[i % self.fill_type.len()]
    }
}

fn apply_variable(p: &mut Palette, key: &str, value: &str) {
    let v = Color::new(value.trim());
    let num = || value.trim().trim_end_matches("px").parse::<f64>().ok();
    match key {
        "fontFamily" => p.font_family = value.to_string(),
        "fontSize" => {
            if let Some(n) = num() {
                p.font_size = n;
            }
        }
        "background" => p.background = v,
        "primaryColor" | "mainBkg" => {
            p.main_bkg = v.clone();
            p.actor_bkg = v.clone();
            p.label_box_bkg = v;
        }
        "primaryBorderColor" | "nodeBorder" => {
            p.node_border = v.clone();
            p.actor_border = v;
        }
        "primaryTextColor" | "nodeTextColor" => {
            p.node_text = v.clone();
            p.actor_text = v;
        }
        "lineColor" => {
            p.line_color = v.clone();
            p.signal_color = v;
        }
        "textColor" => {
            p.text_color = v.clone();
            p.signal_text = v;
        }
        "titleColor" => p.title_color = v,
        "secondaryColor" | "clusterBkg" => p.cluster_bkg = v,
        "clusterBorder" => p.cluster_border = v,
        "compositeTitleBackground" => p.composite_title_background = Some(v),
        "compositeBackground" => p.composite_background = Some(v),
        "altBackground" => p.alt_background = Some(v),
        "tertiaryColor" => {}
        "edgeLabelBackground" => p.edge_label_background = v,
        "actorBkg" => p.actor_bkg = v,
        "actorBorder" => p.actor_border = v,
        "actorTextColor" => p.actor_text = v,
        "actorLineColor" => p.actor_line = v,
        "signalColor" => p.signal_color = v,
        "signalTextColor" => p.signal_text = v,
        "labelBoxBkgColor" => p.label_box_bkg = v,
        "labelBoxBorderColor" => p.label_box_border = v,
        "labelTextColor" => p.label_text = v,
        "loopTextColor" => p.loop_text = v,
        "noteBkgColor" => p.note_bkg = v,
        "noteBorderColor" => p.note_border = v,
        "noteTextColor" => p.note_text = v,
        "activationBkgColor" => p.activation_bkg = v,
        "activationBorderColor" => p.activation_border = v,
        "sequenceNumberColor" => p.sequence_number = v,
        "specialStateColor" => p.special_state = v,
        "pieStrokeColor" => p.pie_stroke = v,
        "pieOuterStrokeColor" => p.pie_outer_stroke = v,
        "pieTitleTextColor" => p.pie_title_text = v,
        "pieSectionTextColor" => p.pie_section_text = v,
        "pieLegendTextColor" => p.pie_legend_text = v,
        "pieStrokeWidth" => p.pie_stroke_width = num().unwrap_or(p.pie_stroke_width),
        "pieOuterStrokeWidth" => {
            p.pie_outer_stroke_width = num().unwrap_or(p.pie_outer_stroke_width)
        }
        "pieOpacity" => p.pie_opacity = num().unwrap_or(p.pie_opacity),
        "strokeWidth" => p.stroke_width = num().or(p.stroke_width),
        "radius" => p.radius = num().or(p.radius),
        k => {
            if let Some(i) = k.strip_prefix("pie").and_then(|n| n.parse::<usize>().ok()) {
                if (1..=12).contains(&i) {
                    p.pie_colors[i - 1] = v;
                }
                return;
            }
            // Indexed colour sets: `cScaleN`, `cScaleLabelN`, ... (`gitInv` before `git`).
            let sets: [(&str, &mut Vec<Color>); 8] = [
                ("cScaleLabel", &mut p.scale_text),
                ("cScalePeer", &mut p.scale_peer),
                ("cScaleInv", &mut p.scale_inv),
                ("cScale", &mut p.scale),
                ("fillType", &mut p.fill_type),
                ("gitBranchLabel", &mut p.git_branch_label),
                ("gitInv", &mut p.git_inv),
                ("git", &mut p.git),
            ];
            for (prefix, set) in sets {
                if let Some(i) = k.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok()) {
                    if let Some(slot) = set.get_mut(i) {
                        *slot = v;
                    }
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_roundtrip() {
        for (n, t) in ThemeName::ALL {
            assert_eq!(n.parse::<ThemeName>(), Ok(t));
            assert_eq!(t.name(), n);
        }
        assert!("nope".parse::<ThemeName>().is_err());
    }

    #[test]
    fn variables_override() {
        let p = palette(
            ThemeName::Default,
            &[
                ("primaryColor".into(), "#ff0000".into()),
                ("pie3".into(), "red".into()),
            ],
        );
        assert_eq!(p.main_bkg.css(), "#ff0000");
        assert_eq!(p.pie_colors[2].css(), "red");
    }

    #[test]
    fn base_derives_from_primary() {
        let p = palette(
            ThemeName::Base,
            &[("primaryColor".into(), "#003366".into())],
        );
        assert_eq!(p.main_bkg.css(), "#003366");
        assert_eq!(p.node_text.css(), "#ffffff");
    }
}
