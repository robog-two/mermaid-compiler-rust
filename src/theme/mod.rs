//! mermaid themes and `themeVariables`.

mod classic;
pub(crate) mod khroma;
mod neo;
mod diagram_styles;
mod tpl;
pub mod styles;
pub mod stylis;
mod vars;

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
    /// The theme this palette was built from (upstream stylesheets gate on its name).
    pub theme: ThemeName,
    pub font_family: String,
    pub font_size: f64,
    pub background: Color,
    pub main_bkg: Color,
    pub node_border: Color,
    pub node_text: Color,
    pub line_color: Color,
    pub text_color: Color,
    pub title_color: Color,
    /// `classText`: class diagram labels (`.nodeLabel, .edgeLabel`). The classic
    /// themes set it to `primaryTextColor`, base and the neo family to `textColor`.
    pub class_text: Color,
    /// `tertiaryColor` (ER relationship-label backgrounds: `fade(tertiaryColor, 0.5)`).
    pub tertiary: Color,
    pub cluster_bkg: Color,
    pub cluster_border: Color,
    /// `flowContainerStroke` (`secondaryBorderColor` unless set): an agentflow
    /// collapsed flow's own stroke, which its separator line keeps.
    pub flow_container_stroke: Color,
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
    /// `pieTitleTextSize`, `pieSectionTextSize`, `pieLegendTextSize` in px.
    pub pie_title_text_size: f64,
    pub pie_section_text_size: f64,
    pub pie_legend_text_size: f64,
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
    /// ER even attribute rows (`rowEven`); set together with `er_row_odd`.
    pub er_row_even: Option<Color>,
    /// ER edge-label background used by the colour themes (`erEdgeLabelBackground`).
    pub er_edge_label_background: Option<Color>,
    /// Requirement edge-label background (`requirementEdgeLabelBackground`, redux themes);
    /// `None` = `edge_label_background`.
    pub requirement_edge_label_background: Option<Color>,
    /// The drop shadow the neo look applies to nodes and clusters (`dropShadow`).
    pub drop_shadow: Option<DropShadow>,
    /// `useGradient`: the neo look strokes nodes and clusters with a horizontal
    /// `gradientStart` → `gradientStop` gradient across each shape's bounding box
    /// (`insertLookDefs`' `{id}-gradient`).
    pub node_gradient: Option<(Color, Color)>,
    /// `nodeShadow` (the redux themes): state start / end circles (and the
    /// flowchart shapes drawn by `stateStart.ts` / `stateEnd.ts`) carry an inline
    /// `filter: url(#{id}-drop-shadow-small)` in every look but hand-drawn.
    pub node_shadow: bool,
    /// `stateBorder`: the inner disc of a state end (`stateBorder ?? nodeBorder`).
    pub state_border: Option<Color>,
    /// A flowchart's node and edge label text (`.label text, span { color:
    /// nodeTextColor || textColor }`): only the base, neo and redux themes set
    /// `nodeTextColor` (to `primaryTextColor`).
    pub flow_text: Color,
    /// `stateLabelColor` (`stateLabelColor || stateBkg || primaryTextColor`):
    /// a state's and a composite's label (`.cluster-label, .nodeLabel`).
    pub state_label: Color,
    /// A `stateBkg` override: a state's fill (`.node rect { fill: stateBkg ||
    /// mainBkg }`).
    pub state_bkg: Option<Color>,
    /// `transitionColor`: a state transition's line and barb (`lineColor`, but
    /// theme-neutral.js `#000`).
    pub transition: Color,
    /// `quadrant1Fill`: the theme's `primaryColor` (the neo / redux light themes use
    /// their gantt-block `#ECECFE`); quadrants 2..4 add 5 per RGB channel each.
    pub quadrant_fill: Color,
    /// `primaryTextColor` (quadrant texts, title and axes).
    pub primary_text: Color,
    /// `primaryBorderColor` (quadrant borders).
    pub primary_border: Color,
    /// State composites: `compositeTitleBackground` (`None` = `main_bkg`),
    /// `compositeBackground` (`None` = `background`) and `altBackground` (`None` = `#f0f0f0`).
    pub composite_title_background: Option<Color>,
    pub composite_background: Option<Color>,
    pub alt_background: Option<Color>,
    /// Git graph commit labels: `commitLabelColor` (`secondaryTextColor`) on
    /// `commitLabelBackground` (`secondaryColor`).
    pub commit_label_color: Color,
    pub commit_label_bkg: Color,
    /// Git graph tags: `tagLabelColor` (`primaryTextColor`) on `tagLabelBackground`
    /// (`primaryColor`), outlined `tagLabelBorder` (`tagBorder || primaryBorderColor`).
    pub tag_label_color: Color,
    pub tag_label_bkg: Color,
    pub tag_label_border: Color,
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

/// The theme's `dropShadow`: the shape's silhouette offset by (`dx`, `dy`),
/// blurred with a Gaussian of standard deviation `std_dev`, in `color` at `opacity`.
///
/// The redux themes' `url(#drop-shadow)` is `insertLookDefs`' `feDropShadow` with
/// `stdDeviation="0"`, which a solid offset copy reproduces exactly in every
/// backend. The classic and neo themes use a CSS `drop-shadow(dx dy blur color)`,
/// whose third length is the Gaussian's standard deviation itself (Filter
/// Effects: unlike `box-shadow`'s blur radius, not twice it; Chromium's
/// rendering of mermaid's 2px matches a standard deviation of 2).
#[derive(Debug, Clone, PartialEq)]
pub struct DropShadow {
    pub dx: f64,
    pub dy: f64,
    pub std_dev: f64,
    pub color: Color,
    pub opacity: f64,
}

impl DropShadow {
    /// `insertLookDefs`' `feDropShadow` filters: `{id}-drop-shadow` (`offset` 4)
    /// and `{id}-drop-shadow-small` (2), unblurred, 6% of white for a theme with
    /// "dark" in its name and of black otherwise.
    pub fn look_defs(theme: ThemeName, offset: f64) -> Self {
        DropShadow {
            dx: offset,
            dy: offset,
            std_dev: 0.0,
            color: c(if theme.name().contains("dark") {
                "#FFFFFF"
            } else {
                "#000000"
            }),
            opacity: 0.06,
        }
    }

    /// The CSS `drop-shadow()` of the classic and neo themes.
    fn css(dx: f64, dy: f64, blur: f64, rgb: &str, opacity: f64) -> Self {
        DropShadow {
            dx,
            dy,
            std_dev: blur,
            color: c(rgb),
            opacity,
        }
    }

    /// Parse a CSS `drop-shadow(<dx> <dy> [<blur>] [<color>])` (lengths in px, the
    /// colour first or last); `None` for anything else.
    pub fn parse_css(s: &str) -> Option<Self> {
        let inner = s
            .trim()
            .trim_end_matches(';')
            .trim()
            .strip_prefix("drop-shadow(")?
            .strip_suffix(')')?;
        // Split on whitespace outside parentheses.
        let mut parts: Vec<String> = Vec::new();
        let mut depth = 0usize;
        let mut cur = String::new();
        for ch in inner.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if ch.is_whitespace() && depth == 0 {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            parts.push(cur);
        }
        let len = |p: &str| -> Option<f64> {
            match p.strip_suffix("px") {
                Some(v) => v.parse().ok(),
                None if p == "0" => Some(0.0),
                None => None,
            }
        };
        // `[ <color>? && <length>{2,3} ]`: the colour before or after the lengths.
        let (col, lens) = match parts.as_slice() {
            [first, rest @ ..] if len(first).is_none() => (Some(first), rest),
            [rest @ .., last] if len(last).is_none() => (Some(last), rest),
            all => (None, all),
        };
        let lens: Vec<f64> = lens.iter().map(|p| len(p)).collect::<Option<_>>()?;
        if !(2..=3).contains(&lens.len()) || lens.get(2).is_some_and(|b| *b < 0.0) {
            return None;
        }
        let col = match col {
            Some(p) => Some(color::parse(p)?),
            None => None,
        };
        // An omitted colour is `currentcolor`, which on an SVG shape is black
        // unless a `color` is set.
        let col = col.unwrap_or(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 1.0,
        });
        Some(DropShadow {
            dx: lens[0],
            dy: lens[1],
            std_dev: lens.get(2).copied().unwrap_or(0.0),
            color: hex(Rgba { a: 1.0, ..col }),
            opacity: col.a,
        })
    }

    /// This shadow as the filter a stylesheet's `filter:` declaration
    /// casts: a hard one is the redux themes' `url(#…-drop-shadow)`
    /// (`feDropShadow`, in linear RGB), any other the CSS `drop-shadow()`.
    pub fn filter(&self) -> crate::scene::Shadow {
        let c = color::parse(self.color.css()).unwrap_or(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 1.0,
        });
        crate::scene::Shadow {
            dx: self.dx,
            dy: self.dy,
            std_dev: self.std_dev,
            color: Color::new(format!(
                "rgba({}, {}, {}, {})",
                c.r,
                c.g,
                c.b,
                crate::output::svg::num(c.a * self.opacity)
            )),
            linear_rgb: self.std_dev == 0.0,
            css: self.std_dev > 0.0,
        }
    }

    /// The blurred shadow as a scene paint shadow (`None` for a hard one, which
    /// is drawn as an offset copy instead).
    pub fn blurred(&self) -> Option<crate::scene::Shadow> {
        (self.std_dev > 0.0).then(|| crate::scene::Shadow {
            dx: self.dx,
            dy: self.dy,
            std_dev: self.std_dev,
            color: match color::parse(self.color.css()) {
                Some(c) => Color::new(format!(
                    "rgba({}, {}, {}, {})",
                    c.r,
                    c.g,
                    c.b,
                    crate::output::svg::num(c.a * self.opacity)
                )),
                None => self.color.clone(),
            },
            linear_rgb: false,
            css: false,
        })
    }
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
        theme: ThemeName::Default,
        font_family: "\"trebuchet ms\", verdana, arial, sans-serif".into(),
        font_size: 16.0,
        background: c("white"),
        main_bkg: c("#ECECFF"),
        node_border: c("#9370DB"),
        node_text: c("#333"),
        line_color: c("#333333"),
        text_color: c("#333"),
        title_color: c("#333"),
        class_text: c("#131300"),
        tertiary: c("hsl(80, 100%, 96.2745098039%)"),
        cluster_bkg: c("#ffffde"),
        cluster_border: c("#aaaa33"),
        flow_container_stroke: c("hsl(60, 60%, 83.5294117647%)"),
        edge_label_background: c("rgba(232,232,232, 0.8)"),
        actor_bkg: c("#ECECFF"),
        // theme-default.js resolves `actorBorder` / `actorLineColor` to `border1`.
        actor_border: c("#9370DB"),
        actor_text: c("black"),
        actor_line: c("#9370DB"),
        signal_color: c("#333"),
        signal_text: c("#333"),
        label_box_bkg: c("#ECECFF"),
        label_box_border: c("#9370DB"),
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
        pie_title_text_size: 25.0,
        pie_section_text_size: 17.0,
        pie_legend_text_size: 17.0,
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
        er_row_even: None,
        er_edge_label_background: None,
        requirement_edge_label_background: None,
        // theme-default.js: `dropShadow = 'drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))'`,
        // `useGradient = false`.
        drop_shadow: Some(DropShadow::css(1.0, 2.0, 2.0, "#b9b9b9", 1.0)),
        node_gradient: None,
        node_shadow: false,
        state_border: None,
        flow_text: c("#333"),
        state_label: c("#131300"),
        state_bkg: None,
        transition: c("#333333"),
        quadrant_fill: c("#ECECFF"),
        // invert(#ECECFF); mkBorder(#ECECFF) = adjust(s: -40, l: -10).
        primary_text: c("#131300"),
        primary_border: c("hsl(240, 60%, 86.2745098039%)"),
        composite_title_background: None,
        composite_background: None,
        alt_background: None,
        commit_label_color: c("#000021"),
        commit_label_bkg: c("#ffffde"),
        tag_label_color: c("#131300"),
        tag_label_bkg: c("#ECECFF"),
        tag_label_border: c("hsl(240, 60%, 86.2745098039%)"),
    }
}

fn forest_palette() -> Palette {
    let mut p = default_palette();
    p.main_bkg = c("#cde498");
    p.quadrant_fill = c("#cde498");
    p.primary_text = c("#321b67");
    p.primary_border = c("hsl(78.1578947368, 18.4615384615%, 64.5098039216%)");
    p.flow_container_stroke = c("hsl(98.961038961, 60%, 74.9019607843%)");
    p.node_border = c("#13540c");
    p.node_text = c("#000000");
    p.line_color = c("#000000"); // theme-forest.js: `lineColor = invert(background)`
    p.text_color = c("#000000");
    p.title_color = c("#333");
    p.tertiary = c("hsl(78.1578947368, 58.4615384615%, 84.5098039216%)");
    p.cluster_bkg = c("#cdffb2");
    p.cluster_border = c("#6eaa49");
    p.edge_label_background = c("#e8e8e8");
    p.actor_bkg = c("#cde498");
    p.actor_border = c("hsl(78.1578947368, 58.4615384615%, 54.5098039216%)");
    p.actor_line = c("hsl(78.1578947368, 58.4615384615%, 54.5098039216%)");
    p.note_border = c("#6eaa49");
    p.label_box_bkg = c("#cde498");
    p.label_box_border = c("#326932");
    p.signal_color = c("#333");
    p.activation_bkg = c("#f4f4f4");
    p.special_state = c("#000000"); // `lineColor`
    // `pie1..12`: see `classic::pie_colors`.
    p.pie_section_text = p.text_color.clone(); // `pieSectionTextColor ||= textColor`
    p
}

fn dark_palette() -> Palette {
    let mut p = default_palette();
    p.alt_background = Some(c("#555"));
    p.flow_container_stroke = c("hsl(180, 0%, 18.3529411765%)");
    p.background = c("#333");
    p.main_bkg = c("#1f2020");
    p.quadrant_fill = c("#1f2020");
    p.primary_text = c("#e0dfdf");
    p.primary_border = c("#cccccc");
    p.node_border = c("#ccc");
    p.node_text = c("#ccc");
    p.line_color = c("lightgrey");
    p.text_color = c("#ccc");
    p.title_color = c("#F9FFFE");
    p.tertiary = c("hsl(20, 1.5873015873%, 12.3529411765%)");
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
    p.special_state = c("#f4f4f4");
    // theme-dark.js: notes are `secondBkg` / `secondaryBorderColor` / `secondaryTextColor`.
    p.note_bkg = c("hsl(180, 1.5873015873%, 28.3529411765%)");
    p.note_border = c("hsl(180, 0%, 18.3529411765%)");
    p.note_text = c("#b8b6b6");
    // `pie1..12`: see `classic::pie_colors`.
    // theme-dark.js: `pieStrokeColor ||= 'black'`, `pieOuterStrokeColor ||= 'black'`.
    p.pie_title_text = c("lightgrey");
    p.pie_section_text = c("#ccc"); // `textColor`
    p.pie_legend_text = c("lightgrey");
    p
}

fn neutral_palette() -> Palette {
    let mut p = default_palette();
    p.alt_background = Some(c("#f4f4f4"));
    p.flow_container_stroke = c("hsl(0, 0%, 88.9215686275%)");
    p.main_bkg = c("#eee");
    p.quadrant_fill = c("#eee");
    p.primary_text = c("#111111");
    p.primary_border = c("hsl(0, 0%, 83.3333333333%)");
    p.node_border = c("#999");
    p.node_text = c("#333");
    p.line_color = c("#666");
    p.cluster_bkg = c("hsl(0, 0%, 98.9215686275%)");
    p.tertiary = c("hsl(-160, 0%, 93.3333333333%)");
    p.cluster_border = c("#707070");
    // theme-neutral.js: `actorBorder = lighten(border1, 23)`, texts `text` (#333).
    p.actor_text = c("#333");
    p.label_text = c("#333");
    p.loop_text = c("#333");
    p.edge_label_background = c("white");
    p.actor_bkg = c("#eee");
    p.actor_border = c("hsl(0, 0%, 83%)");
    p.actor_line = c("hsl(0, 0%, 83%)");
    p.label_box_bkg = c("#eee");
    p.label_box_border = c("hsl(0, 0%, 83%)");
    p.signal_color = c("#333");
    p.note_bkg = c("#666");
    p.note_border = c("#999");
    p.note_text = c("#fff");
    p.special_state = c("#222");
    // theme-neutral.js: `textColor = invert(background)`; the pie title and legend use
    // `taskTextDarkColor = text` (#333), the slice labels `textColor`.
    p.text_color = c("#000000");
    p.pie_title_text = c("#333");
    p.pie_legend_text = c("#333");
    p.pie_section_text = c("#000000");
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
    // theme-helpers.js `mkBorder(c, darkMode)`.
    let mk_border = |c: Rgba| c.adjust_hsl(0.0, -40.0, if dark_mode { 10.0 } else { -10.0 });
    let border = get("primaryBorderColor").unwrap_or(mk_border(primary));
    // theme-base.js: `primaryTextColor ||= darkMode ? '#eee' : '#333'` (however dark
    // `primaryColor` is).
    let text = get("primaryTextColor").unwrap_or(if dark_mode {
        Rgba {
            r: 0xee,
            g: 0xee,
            b: 0xee,
            a: 1.0,
        }
    } else {
        Rgba {
            r: 0x33,
            g: 0x33,
            b: 0x33,
            a: 1.0,
        }
    });
    // theme-base.js: `lineColor ||= invert(background)`, background `#f4f4f4`.
    let line = get("lineColor").unwrap_or_else(|| {
        get("background")
            .unwrap_or(Rgba {
                r: 0xf4,
                g: 0xf4,
                b: 0xf4,
                a: 1.0,
            })
            .invert()
    });
    let mut p = default_palette();
    p.main_bkg = hex(primary);
    p.quadrant_fill = hex(primary);
    p.primary_text = hex(text);
    p.primary_border = hex(border);
    p.node_border = hex(border);
    p.node_text = hex(text);
    p.text_color = hex(text);
    p.line_color = hex(line);
    // theme-base.js: clusters on `tertiaryColor`, notes on a fixed `#fff5ad`, every
    // border `mkBorder(..)` and every sequence text `textColor`.
    p.cluster_bkg = hex(tertiary);
    p.cluster_border = hex(mk_border(tertiary));
    p.flow_container_stroke = hex(mk_border(secondary));
    p.edge_label_background = hex(secondary);
    p.actor_bkg = hex(primary);
    p.actor_border = hex(border);
    p.actor_line = hex(border);
    p.actor_text = hex(text);
    p.signal_color = hex(text);
    p.signal_text = hex(text);
    p.label_box_bkg = hex(primary);
    p.label_box_border = hex(border);
    p.label_text = hex(text);
    p.loop_text = hex(text);
    let note = get("noteBkgColor").unwrap_or(Rgba {
        r: 0xff,
        g: 0xf5,
        b: 0xad,
        a: 1.0,
    });
    p.note_bkg = hex(note);
    p.note_border = hex(mk_border(note));
    p.note_text = hex(text);
    p.activation_bkg = hex(secondary);
    p.activation_border = hex(secondary.darken(10.0));
    p.sequence_number = hex(line.invert());
    // `titleColor ||= tertiaryTextColor = invert(tertiaryColor)`.
    p.title_color = hex(get("titleColor").unwrap_or(tertiary.invert()));
    p.special_state = hex(line);
    // theme-base.js: `pieTitleTextColor` / `pieLegendTextColor ||= taskTextDarkColor`
    // (= `textColor`), `pieSectionTextColor ||= textColor`.
    p.pie_title_text = hex(text);
    p.pie_legend_text = hex(text);
    p.pie_section_text = hex(text);
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
    p.theme = theme;
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
    // `dropShadow`, `useGradient`, `gradientStart` / `gradientStop` as each theme's
    // constructor sets them (base: in `updateColors`, so from the overridden
    // border colours).
    let var = |k: &str| {
        vars.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim())
    };
    let (mut use_gradient, mut gradient) = match theme {
        ThemeName::Default | ThemeName::Null => (
            false,
            (p.primary_border.clone(), p.flow_container_stroke.clone()),
        ),
        ThemeName::Dark | ThemeName::Forest | ThemeName::Neutral | ThemeName::Base => {
            let stop = match (theme, var("secondaryBorderColor")) {
                (ThemeName::Base, Some(v)) => c(v),
                _ => p.flow_container_stroke.clone(),
            };
            (true, (p.primary_border.clone(), stop))
        }
        ThemeName::Neo | ThemeName::NeoDark => (true, (c("#0042eb"), c("#eb0042"))),
        ThemeName::Redux
        | ThemeName::ReduxDark
        | ThemeName::ReduxColor
        | ThemeName::ReduxDarkColor => (false, (c("#0042eb"), c("#eb0042"))),
    };
    match theme {
        ThemeName::Dark | ThemeName::Neutral | ThemeName::Base => {
            p.drop_shadow = Some(DropShadow::css(1.0, 2.0, 2.0, "#b9b9b9", 1.0));
        }
        ThemeName::Forest => p.drop_shadow = Some(DropShadow::css(1.0, 2.0, 2.0, "#b9b9b9", 0.5)),
        _ => {}
    }
    if let Some(v) = var("useGradient") {
        use_gradient = v == "true";
    } else if theme == ThemeName::Base && var("nodeBorder").is_some() {
        // theme-base.js: a custom `nodeBorder` turns the gradient off unless
        // `useGradient` is given too.
        use_gradient = false;
    }
    if let Some(v) = var("gradientStart") {
        gradient.0 = c(v);
    }
    if let Some(v) = var("gradientStop") {
        gradient.1 = c(v);
    }
    p.node_gradient = use_gradient.then_some(gradient);
    p.node_shadow = match var("nodeShadow") {
        Some(v) => v == "true",
        None => matches!(
            theme,
            ThemeName::Redux
                | ThemeName::ReduxDark
                | ThemeName::ReduxColor
                | ThemeName::ReduxDarkColor
        ),
    };
    p.state_border = match (var("stateBorder"), theme) {
        (Some(v), _) => Some(c(v)),
        (None, ThemeName::Neutral) => Some(c("#000")),
        (None, ThemeName::Neo) => Some(c("#000000")),
        (None, ThemeName::Redux | ThemeName::ReduxColor) => Some(c("#28253D")),
        (None, ThemeName::ReduxDark | ThemeName::ReduxDarkColor) => Some(c("#FFFFFF")),
        (None, _) => None,
    };
    if let Some(v) = var("dropShadow") {
        p.drop_shadow = if v.contains("url(#drop-shadow)") {
            Some(DropShadow::look_defs(theme, 4.0))
        } else {
            DropShadow::parse_css(v)
        };
    }
    let git = classic::git_label_colors(theme, vars);
    p.commit_label_color = git.commit_text;
    p.commit_label_bkg = git.commit_bkg;
    p.tag_label_color = git.tag_text;
    p.tag_label_bkg = git.tag_bkg;
    p.tag_label_border = git.tag_border;
    for (k, v) in vars {
        apply_variable(&mut p, k, v);
    }
    // The classic themes' variables as their theme objects resolve them, the
    // overrides included (`vars`): these replace the hand-derived values above.
    let resolved = vars::resolve(theme, vars);
    if let Some(tv) = &resolved {
        let fields: [(&str, &mut Color); 34] = [
            ("background", &mut p.background),
            ("mainBkg", &mut p.main_bkg),
            ("nodeBorder", &mut p.node_border),
            ("lineColor", &mut p.line_color),
            ("textColor", &mut p.text_color),
            ("titleColor", &mut p.title_color),
            ("tertiaryColor", &mut p.tertiary),
            ("clusterBkg", &mut p.cluster_bkg),
            ("clusterBorder", &mut p.cluster_border),
            ("flowContainerStroke", &mut p.flow_container_stroke),
            ("edgeLabelBackground", &mut p.edge_label_background),
            ("actorBkg", &mut p.actor_bkg),
            ("actorBorder", &mut p.actor_border),
            ("actorTextColor", &mut p.actor_text),
            ("actorLineColor", &mut p.actor_line),
            ("signalColor", &mut p.signal_color),
            ("signalTextColor", &mut p.signal_text),
            ("labelBoxBkgColor", &mut p.label_box_bkg),
            ("labelBoxBorderColor", &mut p.label_box_border),
            ("labelTextColor", &mut p.label_text),
            ("loopTextColor", &mut p.loop_text),
            ("noteBkgColor", &mut p.note_bkg),
            ("noteBorderColor", &mut p.note_border),
            ("noteTextColor", &mut p.note_text),
            ("activationBkgColor", &mut p.activation_bkg),
            ("activationBorderColor", &mut p.activation_border),
            ("sequenceNumberColor", &mut p.sequence_number),
            ("specialStateColor", &mut p.special_state),
            ("primaryTextColor", &mut p.primary_text),
            ("primaryBorderColor", &mut p.primary_border),
            ("pieTitleTextColor", &mut p.pie_title_text),
            ("pieSectionTextColor", &mut p.pie_section_text),
            ("pieLegendTextColor", &mut p.pie_legend_text),
            ("classText", &mut p.class_text),
        ];
        for (k, f) in fields {
            if let Some(v) = tv.get(k) {
                *f = c(v);
            }
        }
        // The classic themes' `gradientStart` / `gradientStop` (their constructors',
        // base's `updateColors`').
        if let (Some(g), Some(a), Some(b)) = (
            p.node_gradient.as_mut(),
            tv.get("gradientStart"),
            tv.get("gradientStop"),
        ) {
            *g = (c(a), c(b));
        }
        if theme == ThemeName::Base
            && let Some(v) = tv.get("nodeTextColor")
        {
            p.node_text = c(v);
        }
    }
    // Each theme's `updateColors`, after the overrides.
    let sets_node_text = matches!(
        theme,
        ThemeName::Base
            | ThemeName::Neo
            | ThemeName::NeoDark
            | ThemeName::Redux
            | ThemeName::ReduxDark
            | ThemeName::ReduxColor
            | ThemeName::ReduxDarkColor
    );
    p.flow_text = match var("nodeTextColor") {
        Some(v) => c(v),
        None if sets_node_text => p.primary_text.clone(),
        None => p.text_color.clone(),
    };
    // (Under the default theme mermaid 12.3.0 keeps its own `#131300`
    // whatever `stateBkg` or `primaryTextColor` say: measured, every other
    // theme follows them.)
    let state_bkg = var("stateBkg").filter(|_| theme != ThemeName::Default);
    p.state_label = match var("stateLabelColor").or(state_bkg) {
        Some(v) => c(v),
        None if theme == ThemeName::Default => c("#131300"),
        None => p.primary_text.clone(),
    };
    p.state_bkg = var("stateBkg").map(c);
    p.transition = match (var("transitionColor"), theme) {
        (Some(v), _) => c(v),
        (None, ThemeName::Neutral) => c("#000"),
        (None, _) => p.line_color.clone(),
    };
    // theme-base.js: `compositeBackground ||= background || tertiaryColor`, its
    // background `#f4f4f4` (the others' composites take the canvas's).
    if theme == ThemeName::Base && p.composite_background.is_none() {
        p.composite_background = Some(c(var("background").unwrap_or("#f4f4f4")));
    }
    // `classText`: `this.classText = this.primaryTextColor` in the classic themes'
    // `updateColors` (rerun after the overrides are copied in), `classText ||
    // textColor` in base and the neo family.
    if resolved.is_none() {
        p.class_text = match var("classText") {
            Some(v) => c(v),
            None => match theme {
                ThemeName::Default
                | ThemeName::Null
                | ThemeName::Dark
                | ThemeName::Forest
                | ThemeName::Neutral => p.primary_text.clone(),
                _ => p.text_color.clone(),
            },
        };
    }
    finish_rows(theme, p, vars)
}

/// The ER row stripes, unless overridden (the neo family has none).
fn finish_rows(theme: ThemeName, mut p: Palette, vars: &[(String, String)]) -> Palette {
    if p.er_row_odd.is_some() {
        let (odd, even) = er_rows(theme, &p, vars);
        let set = |k: &str| vars.iter().any(|(n, _)| n == k);
        if !set("rowOdd") {
            p.er_row_odd = Some(odd);
        }
        if !set("rowEven") {
            p.er_row_even = Some(even);
        }
    }
    p
}

/// `rowOdd` / `rowEven` of the classic themes, from the (overridden) `mainBkg`:
/// `lighten(mainBkg, 75)` and a theme-specific shade (`theme-*.js` `updateColors`).
fn er_rows(theme: ThemeName, p: &Palette, vars: &[(String, String)]) -> (Color, Color) {
    let m = p.main_bkg.css();
    let dark_mode = vars
        .iter()
        .any(|(k, v)| k == "darkMode" && v.trim() == "true");
    let (odd, even) = match theme {
        ThemeName::Dark => (khroma::lighten(m, 5.0), khroma::darken(m, 10.0)),
        ThemeName::Neutral => (khroma::lighten(m, 75.0), "#f4f4f4".to_string()),
        ThemeName::Forest => (khroma::lighten(m, 75.0), khroma::lighten(m, 20.0)),
        ThemeName::Base if dark_mode => (khroma::darken(m, 5.0), khroma::darken(m, 10.0)),
        ThemeName::Base => (khroma::lighten(m, 75.0), khroma::lighten(m, 5.0)),
        _ => (khroma::lighten(m, 75.0), khroma::lighten(m, 1.0)),
    };
    (c(&odd), c(&even))
}

impl Palette {
    /// `nodeTextColor || textColor`: the classic themes define no
    /// `nodeTextColor` (whatever `node_text` holds for other uses).
    pub fn node_text_or_text(&self) -> &Color {
        match self.theme {
            ThemeName::Default
            | ThemeName::Null
            | ThemeName::Dark
            | ThemeName::Forest
            | ThemeName::Neutral => &self.text_color,
            _ => &self.node_text,
        }
    }

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
        "flowContainerStroke" | "secondaryBorderColor" => p.flow_container_stroke = v,
        "primaryColor" | "mainBkg" => {
            // The neo / redux light themes derive `quadrant1Fill` from a local constant.
            if key == "primaryColor"
                && !matches!(
                    p.theme,
                    ThemeName::Neo | ThemeName::Redux | ThemeName::ReduxColor
                )
            {
                p.quadrant_fill = v.clone();
            }
            p.main_bkg = v.clone();
            p.actor_bkg = v.clone();
            p.label_box_bkg = v;
        }
        "primaryBorderColor" | "nodeBorder" => {
            if key == "primaryBorderColor" {
                p.primary_border = v.clone();
            }
            p.node_border = v.clone();
            p.actor_border = v;
        }
        "primaryTextColor" | "nodeTextColor" => {
            if key == "primaryTextColor" {
                p.primary_text = v.clone();
            }
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
        "tertiaryColor" => p.tertiary = v,
        "edgeLabelBackground" => p.edge_label_background = v,
        "rowOdd" => p.er_row_odd = Some(v),
        "rowEven" => p.er_row_even = Some(v),
        "erEdgeLabelBackground" => p.er_edge_label_background = Some(v),
        "requirementEdgeLabelBackground" => p.requirement_edge_label_background = Some(v),
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
        "pieTitleTextSize" => p.pie_title_text_size = num().unwrap_or(p.pie_title_text_size),
        "pieSectionTextSize" => p.pie_section_text_size = num().unwrap_or(p.pie_section_text_size),
        "pieLegendTextSize" => p.pie_legend_text_size = num().unwrap_or(p.pie_legend_text_size),
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
        // mermaid 12.3.0: the default theme's constructor sets `mainBkg`, which
        // `updateColors` does not derive again from an overridden `primaryColor`.
        assert_eq!(p.main_bkg.css(), "#ECECFF");
        assert_eq!(p.pie_colors[2].css(), "red");
        let p = palette(ThemeName::Default, &[("mainBkg".into(), "#ff0000".into())]);
        assert_eq!(p.main_bkg.css(), "#ff0000");
        assert_eq!(p.actor_bkg.css(), "#ff0000");
    }

    #[test]
    fn base_derives_from_primary() {
        let p = palette(
            ThemeName::Base,
            &[("primaryColor".into(), "#003366".into())],
        );
        assert_eq!(p.main_bkg.css(), "#003366");
        // mermaid 12.3.0 `theme-base.js`: `primaryTextColor ||= darkMode ? '#eee' : '#333'`,
        // whatever the primary colour.
        assert_eq!(p.node_text.css(), "#333");
        let p = palette(
            ThemeName::Base,
            &[
                ("primaryColor".into(), "#003366".into()),
                ("darkMode".into(), "true".into()),
            ],
        );
        assert_eq!(p.node_text.css(), "#eee");
    }

    /// `commitLabelColor`, `commitLabelBackground`, `tagLabelColor`,
    /// `tagLabelBackground` and `tagLabelBorder` of mermaid 12.3.0's resolved
    /// `themeVariables` (as the browser reads them: `rgb()` / `hsl()` with
    /// fractional channels round to 8 bits).
    #[test]
    fn git_label_colours_match_upstream() {
        let rgb = |c: &Color| {
            let k = color::parse(c.css()).unwrap();
            format!("{:02x}{:02x}{:02x}", k.r, k.g, k.b)
        };
        let vars = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        type Case<'a> = (ThemeName, &'a [(&'a str, &'a str)], [&'a str; 5]);
        let cases: &[Case] = &[
            (
                ThemeName::Default,
                &[],
                [
                    "#000021",
                    "#ffffde",
                    "#131300",
                    "#ECECFF",
                    "hsl(240, 60%, 86.2745098039%)",
                ],
            ),
            (
                ThemeName::Base,
                &[],
                [
                    "rgb(11, 34, 0)",
                    "hsl(-79.4117647059, 100%, 93.3333333333%)",
                    "#333",
                    "#fff4dd",
                    "hsl(40.5882352941, 60%, 83.3333333333%)",
                ],
            ),
            (
                ThemeName::Dark,
                &[],
                [
                    "rgb(184, 182, 182)",
                    "hsl(180, 1.5873015873%, 28.3529411765%)",
                    "#e0dfdf",
                    "#1f2020",
                    "#cccccc",
                ],
            ),
            (
                ThemeName::Forest,
                &[],
                [
                    "#32004d",
                    "#cdffb2",
                    "#321b67",
                    "#cde498",
                    "hsl(78.1578947368, 18.4615384615%, 64.5098039216%)",
                ],
            ),
            (
                ThemeName::Neutral,
                &[],
                [
                    "rgb(3, 3, 3)",
                    "hsl(0, 0%, 98.9215686275%)",
                    "#111111",
                    "#eee",
                    "hsl(0, 0%, 83.3333333333%)",
                ],
            ),
            (
                ThemeName::Neo,
                &[],
                [
                    "#333333",
                    "hsl(-120, 0%, 80%)",
                    "#333",
                    "#cccccc",
                    "hsl(0, 0%, 70%)",
                ],
            ),
            (
                ThemeName::NeoDark,
                &[],
                [
                    "rgb(184, 182, 182)",
                    "hsl(180, 1.5873015873%, 28.3529411765%)",
                    "#e0dfdf",
                    "#1f2020",
                    "#cccccc",
                ],
            ),
            (
                ThemeName::Redux,
                &[],
                [
                    "#333333",
                    "hsl(-120, 0%, 80%)",
                    "#28253D",
                    "#cccccc",
                    "hsl(247.5, 0%, 9.2156862745%)",
                ],
            ),
            (
                ThemeName::ReduxDarkColor,
                &[],
                [
                    "rgb(184, 182, 182)",
                    "hsl(180, 1.5873015873%, 28.3529411765%)",
                    "#e0dfdf",
                    "#1f2020",
                    "#cccccc",
                ],
            ),
            // Overrides: `default` derived everything but the border in its
            // constructor; the others re-derive what they compute in `updateColors`.
            (
                ThemeName::Default,
                &[
                    ("primaryColor", "#ff0000"),
                    ("secondaryColor", "#00ff00"),
                    ("primaryBorderColor", "#ff0000"),
                ],
                ["#000021", "#ffffde", "#131300", "#ECECFF", "#ff0000"],
            ),
            (
                ThemeName::Base,
                &[("primaryColor", "#ff0000")],
                [
                    "#ffff00",
                    "hsl(-120, 100%, 50%)",
                    "#333",
                    "#ff0000",
                    "hsl(0, 60%, 40%)",
                ],
            ),
            (
                ThemeName::Base,
                &[("darkMode", "true"), ("primaryColor", "#202040")],
                [
                    "rgb(223, 191, 223)",
                    "hsl(120, 33.3333333333%, 18.8235294118%)",
                    "#eee",
                    "#202040",
                    "hsl(240, 0%, 28.8235294118%)",
                ],
            ),
            (
                ThemeName::Dark,
                &[("secondaryColor", "#00ff00")],
                [
                    "rgb(184, 182, 182)",
                    "#00ff00",
                    "#e0dfdf",
                    "#1f2020",
                    "#cccccc",
                ],
            ),
            (
                ThemeName::Forest,
                &[("secondaryColor", "#00ff00"), ("primaryColor", "#0000ff")],
                [
                    "#32004d",
                    "#00ff00",
                    "#321b67",
                    "#0000ff",
                    "hsl(78.1578947368, 18.4615384615%, 64.5098039216%)",
                ],
            ),
            (
                ThemeName::Neo,
                &[("primaryColor", "#ff0000")],
                [
                    "#ffff00",
                    "hsl(-120, 100%, 50%)",
                    "#333",
                    "#ff0000",
                    "hsl(0, 0%, 70%)",
                ],
            ),
            (
                ThemeName::Base,
                &[
                    ("tagLabelBorder", "#123456"),
                    ("commitLabelColor", "#abcdef"),
                ],
                [
                    "#abcdef",
                    "hsl(-79.4117647059, 100%, 93.3333333333%)",
                    "#333",
                    "#fff4dd",
                    "#123456",
                ],
            ),
            (
                ThemeName::Default,
                &[("tagBorder", "#00ff00")],
                ["#000021", "#ffffde", "#131300", "#ECECFF", "#00ff00"],
            ),
        ];
        for (theme, v, want) in cases {
            let p = palette(*theme, &vars(v));
            let got = [
                &p.commit_label_color,
                &p.commit_label_bkg,
                &p.tag_label_color,
                &p.tag_label_bkg,
                &p.tag_label_border,
            ];
            for (g, w) in got.iter().zip(want) {
                assert_eq!(
                    rgb(g),
                    rgb(&Color::new(*w)),
                    "{theme:?} {v:?}: {} vs {w}",
                    g.css()
                );
            }
        }
    }

    /// Values checked against mermaid 12.3.0's resolved `themeVariables`.
    #[test]
    fn classic_variables_match_upstream() {
        // As the browser paints them.
        let css = |c: &Color| color::parse(c.css()).unwrap().to_hex();
        let base = palette(ThemeName::Base, &[]);
        assert_eq!(css(&base.pie_legend_text), "#333333");
        assert_eq!(css(&base.pie_title_text), "#333333");
        // `titleColor = invert(tertiaryColor)` is rgb(8.5000000002, 5.75.., 0).
        assert_eq!(css(&base.title_color), "#090600");
        assert_eq!(css(&base.background), "#f4f4f4");
        assert_eq!(css(&base.line_color), "#0b0b0b"); // invert(#f4f4f4)
        assert_eq!(css(&base.note_bkg), "#fff5ad");
        let neutral = palette(ThemeName::Neutral, &[]);
        assert_eq!(css(&neutral.pie_legend_text), "#333333");
        assert_eq!(css(&neutral.text_color), "#000000");
        let dark = palette(ThemeName::Dark, &[]);
        assert_eq!(css(&dark.special_state), "#f4f4f4");
        assert_eq!(css(&dark.pie_section_text), "#cccccc");
        // rowOdd = lighten(#1f2020, 5), rowEven = darken(#1f2020, 10).
        assert_eq!(dark.er_row_odd.as_ref().map(css), Some("#2c2d2d".into()));
        assert_eq!(dark.er_row_even.as_ref().map(css), Some("#060606".into()));
        let default = palette(ThemeName::Default, &[]);
        assert_eq!(css(&default.actor_border), "#9370db");
        assert_eq!(css(&default.primary_text), "#131300");
        let forest = palette(ThemeName::Forest, &[]);
        assert_eq!(css(&forest.line_color), "#000000");
        assert_eq!(css(&forest.pie_section_text), "#000000");
        // primaryBorderColor of the family: mkBorder(#28253D) for redux.
        assert_eq!(
            css(&palette(ThemeName::Redux, &[]).primary_border),
            "#181818"
        );
        assert_eq!(
            css(&palette(ThemeName::NeoDark, &[]).primary_border),
            "#cccccc"
        );
    }

    /// `DUMP_PALETTE=1 cargo test dump_palette -- --nocapture`: our resolved values under
    /// mermaid's variable names, for diffing against the reference's `themeVariables`.
    #[test]
    fn dump_palette() {
        let Ok(path) = std::env::var("DUMP_PALETTE") else {
            return;
        };
        // `DUMP_PALETTE=<cases.json>`: `[{"name", "theme", "vars": {..}}]`; otherwise
        // every theme without overrides.
        type Case = (String, ThemeName, Vec<(String, String)>);
        let cases: Vec<Case> = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| crate::json::parse(&s).ok())
        {
            Some(crate::json::Json::Array(items)) => items
                .iter()
                .map(|c| {
                    let name = c.get("name").and_then(|v| v.as_str()).unwrap().to_string();
                    let theme = c.get("theme").and_then(|v| v.as_str()).unwrap();
                    let vars = c
                        .get("vars")
                        .and_then(|v| v.as_object())
                        .unwrap_or(&[])
                        .iter()
                        .map(|(k, v)| (k.clone(), v.to_scalar_string().unwrap()))
                        .collect();
                    (name, theme.parse().unwrap(), vars)
                })
                .collect(),
            _ => ThemeName::ALL
                .iter()
                .map(|&(n, t)| (n.to_string(), t, Vec::new()))
                .collect(),
        };
        for (name, t, vars) in cases {
            let p = palette(t, &vars);
            let one = |k: &str, c: &Color| println!("{name} {k} {}", c.css());
            for (prefix, set) in [
                ("cScale", &p.scale),
                ("cScalePeer", &p.scale_peer),
                ("cScaleInv", &p.scale_inv),
                ("cScaleLabel", &p.scale_text),
                ("fillType", &p.fill_type),
                ("git", &p.git),
                ("gitInv", &p.git_inv),
                ("gitBranchLabel", &p.git_branch_label),
            ] {
                for (i, c) in set.iter().enumerate() {
                    one(&format!("{prefix}{i}"), c);
                }
            }
            for (i, c) in p.pie_colors.iter().enumerate() {
                one(&format!("pie{}", i + 1), c);
            }
            for (k, c) in [
                ("pieTitleTextColor", &p.pie_title_text),
                ("pieSectionTextColor", &p.pie_section_text),
                ("pieLegendTextColor", &p.pie_legend_text),
                ("pieStrokeColor", &p.pie_stroke),
                ("pieOuterStrokeColor", &p.pie_outer_stroke),
                ("textColor", &p.text_color),
                ("titleColor", &p.title_color),
                ("classText", &p.class_text),
                ("primaryTextColor", &p.primary_text),
                ("primaryBorderColor", &p.primary_border),
                ("flowContainerStroke", &p.flow_container_stroke),
                ("mainBkg", &p.main_bkg),
                ("nodeBorder", &p.node_border),
                ("nodeTextColor", &p.node_text),
                ("lineColor", &p.line_color),
                ("clusterBkg", &p.cluster_bkg),
                ("clusterBorder", &p.cluster_border),
                ("edgeLabelBackground", &p.edge_label_background),
                ("actorBkg", &p.actor_bkg),
                ("actorBorder", &p.actor_border),
                ("actorTextColor", &p.actor_text),
                ("actorLineColor", &p.actor_line),
                ("signalColor", &p.signal_color),
                ("signalTextColor", &p.signal_text),
                ("labelBoxBkgColor", &p.label_box_bkg),
                ("labelBoxBorderColor", &p.label_box_border),
                ("labelTextColor", &p.label_text),
                ("loopTextColor", &p.loop_text),
                ("noteBkgColor", &p.note_bkg),
                ("noteBorderColor", &p.note_border),
                ("noteTextColor", &p.note_text),
                ("activationBkgColor", &p.activation_bkg),
                ("activationBorderColor", &p.activation_border),
                ("sequenceNumberColor", &p.sequence_number),
                ("specialStateColor", &p.special_state),
                ("background", &p.background),
            ] {
                one(k, c);
            }
        }
    }
}
