//! mermaid's `<style>` text: `getStyles(type, userStyles, themeVariables, svgId)`
//! (`styles.ts` plus each diagram's `styles.ts`) compiled by `stylis`
//! ([`super::stylis`]).
//!
//! The templates are transcribed as CSS text with the JavaScript template
//! expressions replaced by lookups in [`Opts`], the `themeVariables` object the
//! theme produced (an undefined variable prints `undefined`, as in JavaScript).
//! stylis normalises whitespace, so the transcriptions need not preserve it.

use super::{ThemeName, palette, stylis, vars};
use crate::config::Look;
use crate::scene::DiagramKind;
use std::collections::HashMap;
use std::fmt::Write as _;

/// What the stylesheet of one diagram depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpec {
    pub kind: DiagramKind,
    pub theme: ThemeName,
    pub look: Look,
    /// The user's `themeVariables`, in order.
    pub theme_variables: Vec<(String, String)>,
    /// The config's top-level `fontFamily` (`--mermaid-font-family`).
    pub font_family: Option<String>,
}

/// The `options` object of `getStyles`.
pub struct Opts {
    m: HashMap<String, String>,
    /// `#svgId`, the `svgId` of `getStyles`.
    pub svg_sel: String,
    pub theme: ThemeName,
    pub look: Look,
    pal: super::Palette,
}

/// `themeVariables` the theme classes set to constants.
fn constants(theme: ThemeName) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("THEME_COLOR_LIMIT", "12")];
    let (ff, fs, sw) = match theme {
        ThemeName::Neo => ("arial, sans-serif", "14px", "2"),
        ThemeName::NeoDark => ("arial, sans-serif", "14px", "1"),
        ThemeName::Redux
        | ThemeName::ReduxDark
        | ThemeName::ReduxColor
        | ThemeName::ReduxDarkColor => ("\"Recursive Variable\", arial, sans-serif", "14px", "2"),
        _ => ("\"trebuchet ms\", verdana, arial, sans-serif", "16px", "1"),
    };
    v.push(("commitLabelFontSize", "10px"));
    v.push(("tagLabelFontSize", "10px"));
    v.push(("fontFamily", ff));
    v.push(("fontSize", fs));
    v.push(("strokeWidth", sw));
    let (grad, shadow) = match theme {
        ThemeName::Default | ThemeName::Null => {
            ("false", "drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))")
        }
        ThemeName::Base | ThemeName::Dark | ThemeName::Neutral => {
            ("true", "drop-shadow( 1px 2px 2px rgba(185,185,185,1))")
        }
        ThemeName::Forest => ("true", "drop-shadow( 1px 2px 2px rgba(185,185,185,0.5))"),
        ThemeName::Neo => ("true", "drop-shadow( 0px 1px 2px rgba(0, 0, 0, 0.25));"),
        ThemeName::NeoDark => ("true", "drop-shadow( 1px 2px 2px rgba(185,185,185,0.2))"),
        _ => ("false", "url(#drop-shadow)"),
    };
    let redux = matches!(
        theme,
        ThemeName::Redux | ThemeName::ReduxDark | ThemeName::ReduxColor | ThemeName::ReduxDarkColor
    );
    if matches!(
        theme,
        ThemeName::Redux | ThemeName::ReduxDark | ThemeName::ReduxColor | ThemeName::ReduxDarkColor
    ) {
        v.push(("commitLineColor", "#BDBCCC"));
    }
    let weight = if redux { "600" } else { "normal" };
    v.push(("fontWeight", weight));
    v.push(("noteFontWeight", weight));
    v.push(("useGradient", grad));
    v.push(("dropShadow", shadow));
    match theme {
        ThemeName::Default | ThemeName::Null | ThemeName::Forest | ThemeName::Neutral => {
            v.push(("errorBkgColor", "#552222"));
            v.push(("errorTextColor", "#552222"));
        }
        ThemeName::Dark => {
            v.push(("errorBkgColor", "#a44141"));
            v.push(("errorTextColor", "#ddd"));
        }
        _ => {}
    }
    v
}

/// Variables the theme classes derive after `updateColors()` that the
/// ThemeVars port does not carry (`||=`: an override or an earlier value wins).
const EXTRA: &str = r##"
archEdgeColor ||= lineColor
archEdgeArrowColor ||= lineColor
archEdgeWidth ||= "3"
archGroupBorderColor ||= primaryBorderColor
archGroupBorderWidth ||= "2px"
vennTitleTextColor ||= titleColor
vennSetTextColor ||= textColor
cynefin.domainFontSize ||= "16"
cynefin.itemFontSize ||= "12"
cynefin.boundaryColor ||= lineColor
cynefin.boundaryWidth ||= "2"
cynefin.cliffColor ||= "#8B0000"
cynefin.cliffWidth ||= "4"
cynefin.arrowColor ||= lineColor
cynefin.arrowWidth ||= "2"
cynefin.textColor ||= textColor
cynefin.labelColor ||= primaryTextColor
radar.axisColor ||= lineColor
radar.axisStrokeWidth ||= "2"
radar.axisLabelFontSize ||= "12"
radar.curveOpacity ||= "0.5"
radar.curveStrokeWidth ||= "2"
radar.graticuleColor ||= "#DEDEDE"
radar.graticuleStrokeWidth ||= "1"
radar.graticuleOpacity ||= "0.3"
radar.legendBoxSize ||= "12"
radar.legendFontSize ||= "12"
"##;

const EXTRA_BASE: &str = r##"
archEdgeColor ||= "#777"
archEdgeArrowColor ||= "#777"
archEdgeWidth ||= "3"
archGroupBorderColor ||= "#000"
archGroupBorderWidth ||= "2px"
vennTitleTextColor ||= titleColor
vennSetTextColor ||= textColor
cynefin.domainFontSize ||= "16"
cynefin.itemFontSize ||= "12"
cynefin.boundaryColor ||= lineColor
cynefin.boundaryWidth ||= "2"
cynefin.cliffColor ||= "#8B0000"
cynefin.cliffWidth ||= "4"
cynefin.arrowColor ||= lineColor
cynefin.arrowWidth ||= "2"
cynefin.textColor ||= textColor
cynefin.labelColor ||= primaryTextColor
radar.axisColor ||= lineColor
radar.axisStrokeWidth ||= "2"
radar.axisLabelFontSize ||= "12"
radar.curveOpacity ||= "0.5"
radar.curveStrokeWidth ||= "2"
radar.graticuleColor ||= "#DEDEDE"
radar.graticuleStrokeWidth ||= "1"
radar.graticuleOpacity ||= "0.3"
radar.legendBoxSize ||= "12"
radar.legendFontSize ||= "12"
"##;

/// The requirement and state variables of the theme classes (`updateColors()`),
/// which the ThemeVars port does not carry.
fn theme_extras(theme: ThemeName) -> String {
    use ThemeName as T;
    let relation_bkg = match theme {
        T::Default | T::Null => "labelBackground",
        T::Forest | T::Neutral => "edgeLabelBackground",
        _ => "darkMode ? darken(secondaryColor, 30) : secondaryColor",
    };
    let transition = if theme == T::Neutral {
        "\"#000\""
    } else {
        "lineColor"
    };
    let inner_end = match theme {
        T::Dark | T::Forest | T::Neutral => "primaryBorderColor",
        _ => "nodeBorder",
    };
    let composite_bg = match theme {
        T::ReduxDark | T::ReduxDarkColor => "\"#16141F\"",
        _ => "background || tertiaryColor",
    };
    let composite_title = match theme {
        T::Redux | T::ReduxColor => "\"#F9F9FB\"",
        T::ReduxDark | T::ReduxDarkColor => "\"#16141F\"",
        _ => "mainBkg",
    };
    let radius = match theme {
        T::Neo | T::NeoDark => "3",
        T::Redux | T::ReduxColor | T::ReduxDark | T::ReduxDarkColor => "12",
        _ => "5",
    };
    let requirement_bkg = match theme {
        T::Neo | T::Redux | T::ReduxColor => "\"#ECECFE\"",
        _ => "primaryColor",
    };
    let alt = match theme {
        T::Base => "tertiaryColor",
        T::Dark => "\"#555\"",
        T::Neutral => "\"#f4f4f4\"",
        T::Redux | T::ReduxColor => "\"#F9F9FB\"",
        T::ReduxDark | T::ReduxDarkColor => "\"#16141F\"",
        _ => "\"#f0f0f0\"",
    };
    let state_border = match theme {
        T::Neo => "stateBorder ||= \"#000000\"\n",
        T::Neutral => "stateBorder ||= \"#000\"\n",
        T::Redux | T::ReduxColor => "stateBorder ||= \"#28253D\"\n",
        T::ReduxDark | T::ReduxDarkColor => "stateBorder ||= \"#FFFFFF\"\n",
        _ => "",
    };
    let req_edge = match theme {
        T::Redux | T::ReduxColor => "requirementEdgeLabelBackground ||= \"#FFFFFF\"\n",
        T::ReduxDark | T::ReduxDarkColor => "requirementEdgeLabelBackground ||= \"#16141F\"\n",
        _ => "",
    };
    format!(
        "{state_border}{req_edge}personBorder ||= primaryBorderColor\npersonBkg ||= mainBkg\nrequirementBackground ||= {requirement_bkg}\n\
         altBackground ||= {alt}\n\
         requirementBorderColor ||= primaryBorderColor\n\
         requirementBorderSize ||= \"1\"\n\
         requirementTextColor ||= primaryTextColor\n\
         relationColor ||= lineColor\n\
         relationLabelBackground ||= {relation_bkg}\n\
         relationLabelColor ||= actorTextColor\n\
         transitionColor ||= {transition}\n\
         transitionLabelColor ||= textColor\n\
         stateLabelColor ||= stateBkg || primaryTextColor\n\
         stateBkg ||= mainBkg\n\
         labelBackgroundColor ||= stateBkg\n\
         compositeBackground ||= {composite_bg}\n\
         compositeTitleBackground ||= {composite_title}\n\
         innerEndBackground ||= {inner_end}\n\
         radius ||= \"{radius}\"\n"
    )
}

impl Opts {
    /// The palette the options were derived with.
    pub(super) fn pal(&self) -> &super::Palette {
        &self.pal
    }

    pub fn new(theme: ThemeName, look: Look, overrides: &[(String, String)]) -> Opts {
        let mut m: HashMap<String, String> = HashMap::new();
        for (k, v) in constants(theme) {
            m.insert(k.into(), v.into());
        }
        if let Some(mut tv) = vars::resolve(theme, overrides) {
            tv.extend(if theme == ThemeName::Base {
                EXTRA_BASE
            } else {
                EXTRA
            });
            tv.extend(match theme {
                ThemeName::Default | ThemeName::Null | ThemeName::Neutral => {
                    "arrowheadColor ||= \"#333333\""
                }
                ThemeName::Forest => "arrowheadColor ||= \"green\"",
                ThemeName::Dark => "arrowheadColor ||= mainContrastColor",
                _ => "arrowheadColor ||= invert(background)",
            });
            tv.extend(&theme_extras(theme));
            for (k, v) in tv.entries() {
                m.insert(k.into(), v.into());
            }
        }
        for (k, v) in overrides {
            m.insert(k.clone(), v.trim().to_string());
        }
        for (k, from) in [
            ("errorBkgColor", "tertiaryColor"),
            ("errorTextColor", "tertiaryTextColor"),
        ] {
            if !m.contains_key(k) {
                if let Some(v) = m.get(from).cloned() {
                    m.insert(k.into(), v);
                }
            }
        }
        let pal = palette(theme, overrides);
        for (k, v) in crate::diagram::gantt::style_vars(&pal, overrides) {
            m.entry(k.to_string()).or_insert(v);
        }
        let mut o = Opts {
            m,
            svg_sel: String::new(),
            theme,
            look,
            pal,
        };
        o.materialise();
        o
    }

    /// Adds the palette's indexed colour sets under their mermaid names, in the
    /// spelling the theme file would print: the palette holds normalised colours
    /// (`#ececff`), while a variable copied from a literal keeps its text
    /// (`#ECECFF`, `black`).
    fn materialise(&mut self) {
        let mut spelled: Vec<(String, crate::color::Rgba)> = self
            .m
            .iter()
            .filter(|(_, v)| !v.contains('(') && !v.contains(' '))
            .filter_map(|(k, v)| Some((format!("{k}\0{v}"), crate::color::parse(v)?)))
            .collect();
        spelled.sort_by(|a, b| a.0.cmp(&b.0));
        let respell = |c: &str| -> String {
            match crate::color::parse(c) {
                Some(rgba) if !c.contains('(') => spelled
                    .iter()
                    .find(|(_, x)| *x == rgba)
                    .and_then(|(k, _)| k.split_once('\0').map(|(_, v)| v.to_string()))
                    .unwrap_or_else(|| c.to_string()),
                _ => c.to_string(),
            }
        };
        let p = &self.pal;
        let sets: [(&str, &Vec<crate::scene::Color>); 8] = [
            ("cScale", &p.scale),
            ("cScaleLabel", &p.scale_text),
            ("cScalePeer", &p.scale_peer),
            ("cScaleInv", &p.scale_inv),
            ("fillType", &p.fill_type),
            ("git", &p.git),
            ("gitInv", &p.git_inv),
            ("gitBranchLabel", &p.git_branch_label),
        ];
        let mut add: Vec<(String, String)> = Vec::new();
        let classic_default = matches!(self.theme, ThemeName::Default | ThemeName::Null);
        for (name, set) in sets {
            for (i, c) in set.iter().enumerate() {
                // Copies of a literal keep its spelling; derived colours (`invert`,
                // `adjust`, ...) print as khroma does, whichever literal they equal.
                let derived = matches!(name, "gitInv" | "cScaleInv" | "cScalePeer")
                    || (name == "gitBranchLabel"
                        && matches!(i, 0 | 3)
                        && matches!(
                            self.theme,
                            ThemeName::Default | ThemeName::Null | ThemeName::Forest | ThemeName::Dark
                        ))
                    || (classic_default && name == "cScaleLabel" && matches!(i, 0 | 3));
                let text = if derived {
                    c.css().to_string()
                } else {
                    respell(c.css())
                };
                add.push((format!("{name}{i}"), text));
            }
        }
        for (i, c) in p.pie_colors.iter().enumerate() {
            add.push((format!("pie{}", i + 1), respell(c.css())));
        }
        let opt = |k: &str, c: &Option<crate::scene::Color>, add: &mut Vec<(String, String)>| {
            if let Some(c) = c {
                add.push((k.to_string(), respell(c.css())));
            }
        };
        opt("erEdgeLabelBackground", &p.er_edge_label_background, &mut add);
        opt(
            "requirementEdgeLabelBackground",
            &p.requirement_edge_label_background,
            &mut add,
        );
        opt("commitLabelColor", &Some(p.commit_label_color.clone()), &mut add);
        opt("commitLabelBackground", &Some(p.commit_label_bkg.clone()), &mut add);
        opt("tagLabelColor", &Some(p.tag_label_color.clone()), &mut add);
        opt("tagLabelBackground", &Some(p.tag_label_bkg.clone()), &mut add);
        opt("tagLabelBorder", &Some(p.tag_label_border.clone()), &mut add);
        opt("altBackground", &p.alt_background, &mut add);
        opt("stateBorder", &p.state_border, &mut add);
        opt("compositeBackground", &p.composite_background, &mut add);
        opt(
            "compositeTitleBackground",
            &p.composite_title_background,
            &mut add,
        );
        for (k, v) in add {
            self.m.entry(k).or_insert(v);
        }
    }

    /// `borderColorArray` and `bkgColorArray` of a colour theme (`isColorTheme`):
    /// `None` for every other theme.
    pub(super) fn color_arrays(&self) -> Option<(Vec<String>, Vec<String>)> {
        let c = self.pal.color_slots.as_ref()?;
        if !matches!(
            self.theme,
            ThemeName::ReduxColor | ThemeName::ReduxDarkColor
        ) || c.border.is_empty()
        {
            return None;
        }
        let f = |v: &Vec<crate::scene::Color>| v.iter().map(|c| c.css().to_string()).collect();
        Some((f(&c.border), f(&c.bkg)))
    }

    /// `options[k]`, `None` for `undefined`.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.m
            .get(k)
            .map(String::as_str)
            .or_else(|| self.derived(k))
    }

    /// `${options[k]}`.
    pub fn s(&self, k: &str) -> String {
        self.get(k).unwrap_or_else(|| {
            if std::env::var_os("MMDC_STYLE_DEBUG").is_some() {
                eprintln!("undefined option {k} ({:?})", self.theme);
            }
            "undefined"
        }).to_string()
    }

    /// `a || b` over variable names (the first truthy one).
    pub fn or(&self, ks: &[&str]) -> String {
        for k in ks {
            if let Some(v) = self.get(k) {
                if !v.is_empty() && v != "0" && v != "false" {
                    return v.to_string();
                }
            }
        }
        self.s(ks[ks.len() - 1])
    }

    /// The variables the palette holds under another spelling.
    fn derived(&self, k: &str) -> Option<&str> {
        let p = &self.pal;
        let idx = |prefix: &str| -> Option<usize> { k.strip_prefix(prefix)?.parse().ok() };
        match k {
            "pieStrokeColor" => Some(p.pie_stroke.css()),
            "pieOuterStrokeColor" => Some(p.pie_outer_stroke.css()),
            "pieTitleTextColor" => Some(p.pie_title_text.css()),
            "pieSectionTextColor" => Some(p.pie_section_text.css()),
            "pieLegendTextColor" => Some(p.pie_legend_text.css()),
            _ => {
                if let Some(i) = idx("cScaleLabel") {
                    return pick(&p.scale_text, i);
                }
                if let Some(i) = idx("cScalePeer") {
                    return pick(&p.scale_peer, i);
                }
                if let Some(i) = idx("cScaleInv") {
                    return pick(&p.scale_inv, i);
                }
                if let Some(i) = idx("cScale") {
                    return pick(&p.scale, i);
                }
                if let Some(i) = idx("fillType") {
                    return pick(&p.fill_type, i);
                }
                if let Some(i) = idx("gitInv") {
                    return pick(&p.git_inv, i);
                }
                if let Some(i) = idx("gitBranchLabel") {
                    return pick(&p.git_branch_label, i);
                }
                if let Some(i) = idx("git") {
                    return pick(&p.git, i);
                }
                if let Some(i) = idx("pie") {
                    return i.checked_sub(1).and_then(|i| pick(&p.pie_colors, i));
                }
                None
            }
        }
    }

    /// A pixel quantity of the palette (`2px`).
    pub(super) fn px(n: f64) -> String {
        format!("{}px", crate::theme::styles::js_num(n))
    }
}

fn pick(v: &[crate::scene::Color], i: usize) -> Option<&str> {
    v.get(i).map(crate::scene::Color::css)
}

/// A number as JavaScript prints it.
pub fn js_num(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `${(options.strokeWidth ?? 1)}`-style numeric option.
pub(super) fn width_or(o: &Opts, default: &str) -> String {
    o.get("strokeWidth").unwrap_or(default).to_string()
}

/// styles.ts: the rules every diagram gets around its own.
fn base(o: &Opts, id_sel: &str, diagram: &str, user: &str) -> String {
    let ff = o.s("fontFamily");
    let fs = o.s("fontSize");
    let line = o.s("lineColor");
    let node_border = o.s("nodeBorder");
    let sw = width_or(o, "1");
    let use_gradient = o.get("useGradient").is_some_and(|v| v == "true");
    let stroke = if use_gradient {
        format!("url({id_sel}-gradient)")
    } else {
        node_border.clone()
    };
    let shadow = match o.get("dropShadow").filter(|s| !s.is_empty()) {
        Some(d) => d.replace("url(#drop-shadow)", &format!("url({id_sel}-drop-shadow)")),
        None => "none".to_string(),
    };
    let mut s = String::new();
    let _ = write!(
        s,
        "& {{ font-family: {ff}; font-size: {fs}; fill: {} }}\n\
         @keyframes edge-animation-frame {{ from {{ stroke-dashoffset: 0; }} }}\n\
         @keyframes dash {{ to {{ stroke-dashoffset: 0; }} }}\n\
         & .edge-animation-slow {{ stroke-dasharray: 9,5 !important; stroke-dashoffset: 900; animation: dash 50s linear infinite; stroke-linecap: round; }}\n\
         & .edge-animation-fast {{ stroke-dasharray: 9,5 !important; stroke-dashoffset: 900; animation: dash 20s linear infinite; stroke-linecap: round; }}\n\
         & .error-icon {{ fill: {}; }}\n\
         & .error-text {{ fill: {}; stroke: {}; }}\n\
         & .edge-thickness-normal {{ stroke-width: {sw}px; }}\n\
         & .edge-thickness-thick {{ stroke-width: 3.5px }}\n\
         & .edge-pattern-solid {{ stroke-dasharray: 0; }}\n\
         & .edge-thickness-invisible {{ stroke-width: 0; fill: none; }}\n\
         & .edge-pattern-dashed{{ stroke-dasharray: 3; }}\n\
         .edge-pattern-dotted {{ stroke-dasharray: 2; }}\n\
         & .marker {{ fill: {line}; stroke: {line}; }}\n\
         & .marker.cross {{ stroke: {line}; }}\n\
         & svg {{ font-family: {ff}; font-size: {fs}; }}\n\
         & p {{ margin: 0 }}\n\
         {diagram}\n\
         .node .neo-node {{ stroke: {node_border}; }}\n\
         [data-look=\"neo\"].node rect, [data-look=\"neo\"].cluster rect, [data-look=\"neo\"].node polygon {{ stroke: {stroke}; filter: {shadow}; }}\n\
         [data-look=\"neo\"].swimlane.cluster rect {{ filter: none; }}\n\
         [data-look=\"neo\"].node path {{ stroke: {stroke}; stroke-width: {sw}px; }}\n\
         [data-look=\"neo\"].node .outer-path {{ filter: {shadow}; }}\n\
         [data-look=\"neo\"].node .neo-line path {{ stroke: {node_border}; filter: none; }}\n\
         [data-look=\"neo\"].node circle{{ stroke: {stroke}; filter: {shadow}; }}\n\
         [data-look=\"neo\"].node circle .state-start{{ fill: #000000; }}\n\
         [data-look=\"neo\"].icon-shape .icon {{ fill: {stroke}; filter: {shadow}; }}\n\
         [data-look=\"neo\"].icon-shape .icon-neo path {{ stroke: {stroke}; filter: {shadow}; }}\n\
         {user}\n",
        o.s("textColor"),
        o.s("errorBkgColor"),
        o.s("errorTextColor"),
        o.s("errorTextColor"),
    );
    s
}

/// What the diagram's first `<style>` element is.
#[derive(Debug, Clone, PartialEq)]
pub enum Sheet {
    /// Element text (unescaped).
    Text(String),
    /// mermaid writes no `<style>` for the diagram.
    Omit,
}

/// The first `<style>` for `spec`, `None` where it is not transcribed.
pub fn style_text(spec: &StyleSpec, id: &str) -> Option<Sheet> {
    match spec.kind {
        // Drawn by the wardley renderer without mermaid's stylesheet.
        DiagramKind::Wardley => return Some(Sheet::Omit),
        // The first style of a ZenUML svg is the plugin's own, a fixed text.
        DiagramKind::ZenUml => {
            return Some(Sheet::Text(include_str!("css/zenuml.css").to_string()));
        }
        _ => {}
    }
    let mut o = Opts::new(spec.theme, spec.look, &spec.theme_variables);
    o.svg_sel = format!("#{id}");
    let diagram_css = super::diagram_styles::diagram(spec.kind, &o)?;
    let id_sel = format!("#{id}");
    let root_ff = spec
        .font_family
        .clone()
        .unwrap_or_else(|| "\"trebuchet ms\", verdana, arial, sans-serif".into());
    let user = format!(":root {{ --mermaid-font-family: {root_ff}}}");
    Some(Sheet::Text(stylis::compile(
        &id_sel,
        &base(&o, &id_sel, &diagram_css, &user),
    )))
}
