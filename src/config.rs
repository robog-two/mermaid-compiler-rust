//! Typed mermaid configuration, layered exactly like mermaid-cli + mermaid:
//! `-t theme` → `--configFile` (shallow-assigned over it) → `--cssFile` (themeCSS)
//! → diagram front-matter `config:` → `%%{init: ...}%%` directives.

use crate::json::Json;
use crate::theme::{self, Palette, ThemeName};

/// Edge interpolation (`flowchart.curve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Basis,
    Linear,
    Step,
}

impl Curve {
    fn from_name(s: &str) -> Curve {
        match s {
            "linear" | "natural" => Curve::Linear,
            "step" | "stepBefore" | "stepAfter" => Curve::Step,
            _ => Curve::Basis,
        }
    }
}

/// mermaid's `look`. `HandDrawn` is accepted but drawn like `Classic` (no rough.js).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    Classic,
    Neo,
    HandDrawn,
}

impl Look {
    /// Parse a schema value (`classic` / `neo` / `handDrawn`); anything else is not usable.
    pub fn from_name(s: &str) -> Option<Look> {
        match s {
            "classic" => Some(Look::Classic),
            "neo" => Some(Look::Neo),
            "handDrawn" => Some(Look::HandDrawn),
            _ => None,
        }
    }
}

/// `theme` / `look` given by one config section (top level or one diagram's section).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Appearance {
    pub theme: Option<ThemeName>,
    pub look: Option<Look>,
}

impl Appearance {
    /// Fold the usable `theme` / `look` of `j` over `self`. Unusable values are passed
    /// over (mermaid's `isUsableAppearance`); an explicit `null` clears the key.
    fn merge(&mut self, j: &Json) {
        match j.get("theme") {
            Some(Json::String(s)) => {
                if let Ok(t) = s.parse() {
                    self.theme = Some(t);
                }
            }
            Some(Json::Null) => self.theme = None,
            _ => {}
        }
        match j.get("look") {
            Some(Json::String(s)) => {
                if let Some(l) = Look::from_name(s) {
                    self.look = Some(l);
                }
            }
            Some(Json::Null) => self.look = None,
            _ => {}
        }
    }
}

/// The appearance settings of one config layer (mermaid's directives, or the site config
/// mermaid-cli hands to `initialize`): the top-level keys plus every diagram section's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppearanceLayer {
    pub global: Appearance,
    pub sections: Vec<(String, Appearance)>,
}

/// Diagram sections whose schema defaults are `theme: redux-color`, `look: neo`
/// (mermaid 12, `config.schema.yaml`; of those, the ones this implementation renders).
pub const NEO_DEFAULT_SECTIONS: [&str; 8] = [
    "flowchart",
    "sequence",
    "class",
    "state",
    "er",
    "requirement",
    "agentflow",
    "swimlane",
];

impl AppearanceLayer {
    /// Deep-merge `j` (assignWithDepth semantics for these keys).
    pub fn merge(&mut self, j: &Json) {
        self.global.merge(j);
        for (k, v) in j.as_object().into_iter().flatten() {
            if v.as_object().is_none() || (v.get("theme").is_none() && v.get("look").is_none()) {
                continue;
            }
            match self.sections.iter_mut().find(|(n, _)| n == k) {
                Some((_, a)) => a.merge(v),
                None => {
                    let mut a = Appearance::default();
                    a.merge(v);
                    self.sections.push((k.clone(), a));
                }
            }
        }
    }

    fn section(&self, key: &str) -> Appearance {
        self.sections
            .iter()
            .find(|(n, _)| n == key)
            .map(|(_, a)| *a)
            .unwrap_or_default()
    }

    /// mermaid's `defaultConfig`: `default` / `classic`, except the sections listed in
    /// [`NEO_DEFAULT_SECTIONS`].
    pub fn defaults() -> AppearanceLayer {
        AppearanceLayer {
            global: Appearance {
                theme: Some(ThemeName::Default),
                look: Some(Look::Classic),
            },
            sections: NEO_DEFAULT_SECTIONS
                .iter()
                .map(|k| {
                    (
                        k.to_string(),
                        Appearance {
                            theme: Some(ThemeName::ReduxColor),
                            look: Some(Look::Neo),
                        },
                    )
                })
                .collect(),
        }
    }
}

/// mermaid's `resolveAppearance` / `readAppearance`: for each key, walk the layers from
/// highest to lowest and, within a layer, take the diagram section's value before the
/// top-level one. The first usable value wins. `layers` should end with
/// [`AppearanceLayer::defaults`]; if nothing is found the result is `default` / `classic`.
pub fn resolve_appearance(section: &str, layers: &[&AppearanceLayer]) -> (ThemeName, Look) {
    let theme = layers
        .iter()
        .find_map(|l| l.section(section).theme.or(l.global.theme))
        .unwrap_or(ThemeName::Default);
    let look = layers
        .iter()
        .find_map(|l| l.section(section).look.or(l.global.look))
        .unwrap_or(Look::Classic);
    (theme, look)
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphConfig {
    /// Resolved `look` for the diagram being rendered.
    pub look: Look,
    pub node_spacing: f64,
    pub rank_spacing: f64,
    pub padding: f64,
    pub diagram_padding: f64,
    pub curve: Curve,
    pub use_max_width: bool,
    pub title_top_margin: f64,
}

impl GraphConfig {
    fn flowchart() -> Self {
        GraphConfig {
            look: Look::Classic,
            node_spacing: 50.0,
            rank_spacing: 50.0,
            padding: 15.0,
            diagram_padding: 8.0,
            curve: Curve::Basis,
            use_max_width: true,
            title_top_margin: 25.0,
        }
    }
    fn state() -> Self {
        GraphConfig {
            padding: 8.0,
            rank_spacing: 70.0,
            ..Self::flowchart()
        }
    }
    fn class() -> Self {
        GraphConfig {
            padding: 12.0,
            ..Self::flowchart()
        }
    }

    fn apply(&mut self, j: &Json) {
        set_f64(j, "nodeSpacing", &mut self.node_spacing);
        set_f64(j, "rankSpacing", &mut self.rank_spacing);
        set_f64(j, "padding", &mut self.padding);
        set_f64(j, "diagramPadding", &mut self.diagram_padding);
        set_f64(j, "titleTopMargin", &mut self.title_top_margin);
        set_bool(j, "useMaxWidth", &mut self.use_max_width);
        if let Some(c) = j.get("curve").and_then(Json::as_str) {
            self.curve = Curve::from_name(c);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceConfig {
    /// Resolved `look` for the diagram being rendered.
    pub look: Look,
    pub diagram_margin_x: f64,
    pub diagram_margin_y: f64,
    pub actor_margin: f64,
    pub width: f64,
    pub height: f64,
    pub box_margin: f64,
    pub box_text_margin: f64,
    pub note_margin: f64,
    pub message_margin: f64,
    pub mirror_actors: bool,
    pub show_sequence_numbers: bool,
    pub use_max_width: bool,
    pub message_font_size: f64,
    pub note_font_size: f64,
    pub actor_font_size: f64,
    pub wrap_padding: f64,
}

impl Default for SequenceConfig {
    fn default() -> Self {
        SequenceConfig {
            look: Look::Classic,
            diagram_margin_x: 50.0,
            diagram_margin_y: 10.0,
            actor_margin: 50.0,
            width: 150.0,
            height: 65.0,
            box_margin: 10.0,
            box_text_margin: 5.0,
            note_margin: 10.0,
            message_margin: 35.0,
            mirror_actors: true,
            show_sequence_numbers: false,
            use_max_width: true,
            message_font_size: 16.0,
            note_font_size: 16.0,
            actor_font_size: 16.0,
            wrap_padding: 10.0,
        }
    }
}

impl SequenceConfig {
    fn apply(&mut self, j: &Json) {
        set_f64(j, "diagramMarginX", &mut self.diagram_margin_x);
        set_f64(j, "diagramMarginY", &mut self.diagram_margin_y);
        set_f64(j, "actorMargin", &mut self.actor_margin);
        set_f64(j, "width", &mut self.width);
        set_f64(j, "height", &mut self.height);
        set_f64(j, "boxMargin", &mut self.box_margin);
        set_f64(j, "boxTextMargin", &mut self.box_text_margin);
        set_f64(j, "noteMargin", &mut self.note_margin);
        set_f64(j, "messageMargin", &mut self.message_margin);
        set_f64(j, "messageFontSize", &mut self.message_font_size);
        set_f64(j, "noteFontSize", &mut self.note_font_size);
        set_f64(j, "actorFontSize", &mut self.actor_font_size);
        set_f64(j, "wrapPadding", &mut self.wrap_padding);
        set_bool(j, "mirrorActors", &mut self.mirror_actors);
        set_bool(j, "showSequenceNumbers", &mut self.show_sequence_numbers);
        set_bool(j, "useMaxWidth", &mut self.use_max_width);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PieConfig {
    pub text_position: f64,
    pub use_max_width: bool,
}

impl Default for PieConfig {
    fn default() -> Self {
        PieConfig {
            text_position: 0.75,
            use_max_width: true,
        }
    }
}

/// The full configuration relevant to this implementation. Unknown keys are ignored,
/// as mermaid does.
#[derive(Debug, Clone, PartialEq)]
pub struct MermaidConfig {
    /// The effective theme. Outside a diagram scope this is the top-level setting (or
    /// `default`); [`MermaidConfig::scoped`] resolves it per diagram type.
    pub theme: ThemeName,
    /// The effective look (see `theme`).
    pub look: Look,
    /// Appearance given by the site config (`-t`, then `--configFile`).
    pub site: AppearanceLayer,
    /// Appearance given by the diagram's front-matter and `%%{init}%%` directives.
    pub directives: AppearanceLayer,
    pub theme_variables: Vec<(String, String)>,
    pub theme_css: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub flowchart: GraphConfig,
    pub state: GraphConfig,
    pub class: GraphConfig,
    pub sequence: SequenceConfig,
    pub pie: PieConfig,
    /// Every JSON layer applied, in order, for diagram-specific options that
    /// have no typed field (read with [`MermaidConfig::section`]).
    pub raw: Vec<Json>,
}

impl Default for MermaidConfig {
    fn default() -> Self {
        MermaidConfig {
            theme: ThemeName::Default,
            look: Look::Classic,
            site: AppearanceLayer::default(),
            directives: AppearanceLayer::default(),
            theme_variables: Vec::new(),
            theme_css: None,
            font_family: None,
            font_size: None,
            flowchart: GraphConfig::flowchart(),
            state: GraphConfig::state(),
            class: GraphConfig::class(),
            sequence: SequenceConfig::default(),
            pie: PieConfig::default(),
            raw: Vec::new(),
        }
    }
}

fn set_f64(j: &Json, key: &str, slot: &mut f64) {
    if let Some(v) = j.get(key).and_then(Json::as_f64) {
        *slot = v;
    }
}

fn set_bool(j: &Json, key: &str, slot: &mut bool) {
    if let Some(v) = j.get(key).and_then(Json::as_bool) {
        *slot = v;
    }
}

impl MermaidConfig {
    /// Return a new config with `j` layered on top. Unknown theme names are an error
    /// (mermaid silently falls back; we warn the user instead of guessing).
    /// This is a *site* layer (mermaid-cli's `--configFile`); see
    /// [`MermaidConfig::with_directive`] for front-matter and `%%{init}%%`.
    pub fn with_json(self, j: &Json) -> Result<Self, String> {
        self.with_layer(j, false)
    }

    /// Layer a front-matter `config:` block or an `%%{init}%%` directive on top.
    pub fn with_directive(self, j: &Json) -> Result<Self, String> {
        self.with_layer(j, true)
    }

    /// `-t/--theme`: the site config's top-level theme.
    pub fn with_cli_theme(mut self, t: ThemeName) -> Self {
        self.site.global.theme = Some(t);
        self.refresh_global_appearance();
        self
    }

    /// Resolve `theme` and `look` for one diagram section (mermaid's `resolveAppearance`)
    /// and hand the look to that diagram's typed config.
    pub fn scoped(mut self, section: &str) -> Self {
        let (theme, look) = resolve_appearance(
            section,
            &[&self.directives, &self.site, &AppearanceLayer::defaults()],
        );
        self.theme = theme;
        self.look = look;
        self.flowchart.look = look;
        self.state.look = look;
        self.class.look = look;
        self.sequence.look = look;
        self
    }

    fn refresh_global_appearance(&mut self) {
        let (theme, look) = resolve_appearance(
            "",
            &[
                &self.directives,
                &self.site,
                &AppearanceLayer {
                    global: AppearanceLayer::defaults().global,
                    sections: Vec::new(),
                },
            ],
        );
        self.theme = theme;
        self.look = look;
    }

    fn with_layer(mut self, j: &Json, directive: bool) -> Result<Self, String> {
        if j.as_object().is_none() {
            return Err("configuration must be a JSON object".into());
        }
        match j.get("theme") {
            None | Some(Json::Null) => {}
            Some(Json::String(s)) => {
                s.parse::<ThemeName>()
                    .map_err(|_| format!("unknown theme \"{s}\""))?;
            }
            _ => return Err("\"theme\" must be a string".into()),
        }
        if directive {
            self.directives.merge(j);
        } else {
            self.site.merge(j);
        }
        self.refresh_global_appearance();
        if let Some(vars) = j.get("themeVariables").and_then(Json::as_object) {
            for (k, v) in vars {
                if let Some(s) = v.to_scalar_string() {
                    self.theme_variables.push((k.clone(), s));
                }
            }
        }
        if let Some(css) = j.get("themeCSS").and_then(Json::as_str) {
            self.theme_css = Some(css.to_string());
        }
        if let Some(f) = j.get("fontFamily").and_then(Json::as_str) {
            self.font_family = Some(f.to_string());
        }
        if let Some(f) = j.get("fontSize").and_then(Json::as_f64) {
            self.font_size = Some(f);
        }
        if let Some(f) = j.get("flowchart") {
            self.flowchart.apply(f);
        }
        if let Some(f) = j.get("state") {
            self.state.apply(f);
        }
        if let Some(f) = j.get("class") {
            self.class.apply(f);
        }
        if let Some(f) = j.get("sequence") {
            self.sequence.apply(f);
        }
        self.raw.push(j.clone());
        if let Some(f) = j.get("pie") {
            set_f64(f, "textPosition", &mut self.pie.text_position);
            set_bool(f, "useMaxWidth", &mut self.pie.use_max_width);
        }
        Ok(self)
    }

    /// Latest value of `section.key` across all layers (e.g. `("gantt", "barHeight")`).
    pub fn section(&self, section: &str, key: &str) -> Option<&Json> {
        self.raw
            .iter()
            .rev()
            .find_map(|j| j.get(section).and_then(|s| s.get(key)))
    }

    /// Numeric `section.key`, or `default`.
    pub fn section_f64(&self, section: &str, key: &str, default: f64) -> f64 {
        self.section(section, key)
            .and_then(Json::as_f64)
            .unwrap_or(default)
    }

    /// Boolean `section.key`, or `default`.
    pub fn section_bool(&self, section: &str, key: &str, default: bool) -> bool {
        self.section(section, key)
            .and_then(Json::as_bool)
            .unwrap_or(default)
    }

    /// mermaid's per-diagram `useMaxWidth` (default true).
    pub fn use_max_width(&self, section: &str) -> bool {
        self.section_bool(section, "useMaxWidth", true)
    }

    /// Resolve theme + variables + top-level font overrides into concrete colours.
    pub fn palette(&self) -> Palette {
        let mut p = theme::palette(self.theme, &self.theme_variables);
        if let Some(f) = &self.font_family {
            p.font_family = f.clone();
        }
        if let Some(s) = self.font_size {
            p.font_size = s;
        }
        p
    }
}

/// Parse the YAML subset used in mermaid front-matter (nested maps of scalars).
/// Lists and anchors are ignored rather than rejected.
pub fn parse_yaml_subset(src: &str) -> Json {
    let lines: Vec<(usize, &str)> = src
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| (l.len() - l.trim_start().len(), l.trim()))
        .collect();
    let mut idx = 0;
    yaml_map(&lines, &mut idx, 0)
}

fn yaml_map(lines: &[(usize, &str)], idx: &mut usize, indent: usize) -> Json {
    let mut pairs = Vec::new();
    while *idx < lines.len() {
        let (ind, text) = lines[*idx];
        if ind < indent {
            break;
        }
        *idx += 1;
        let Some((k, v)) = text.split_once(':') else {
            continue;
        };
        let key = unquote(k.trim()).to_string();
        let v = v.trim();
        if v.is_empty() {
            let child_indent = lines.get(*idx).map(|(i, _)| *i).unwrap_or(0);
            if child_indent > ind {
                pairs.push((key, yaml_map(lines, idx, child_indent)));
            } else {
                pairs.push((key, Json::Null));
            }
        } else {
            pairs.push((key, yaml_scalar(v)));
        }
    }
    Json::Object(pairs)
}

fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn yaml_scalar(v: &str) -> Json {
    let v = if !v.starts_with(['"', '\'']) {
        v.split(" #").next().unwrap_or(v).trim()
    } else {
        v
    };
    match v {
        "true" => Json::Bool(true),
        "false" => Json::Bool(false),
        "null" | "~" => Json::Null,
        _ => {
            if v.starts_with('{') {
                if let Ok(j) = crate::json::parse_lenient(v) {
                    return j;
                }
            }
            v.parse::<f64>()
                .map(Json::Number)
                .unwrap_or_else(|_| Json::String(unquote(v).to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layering_overrides_fields() {
        let j = crate::json::parse(r##"{"theme":"forest","flowchart":{"nodeSpacing":80,"curve":"linear"},"themeVariables":{"primaryColor":"#f00"}}"##).unwrap();
        let c = MermaidConfig::default().with_json(&j).unwrap();
        assert_eq!(c.theme, ThemeName::Forest);
        assert_eq!(c.flowchart.node_spacing, 80.0);
        assert_eq!(c.flowchart.curve, Curve::Linear);
        assert_eq!(c.palette().main_bkg.css(), "#f00");
    }

    fn json(s: &str) -> Json {
        crate::json::parse(s).unwrap()
    }

    #[test]
    fn appearance_defaults_per_diagram() {
        let c = MermaidConfig::default();
        for key in NEO_DEFAULT_SECTIONS {
            let s = c.clone().scoped(key);
            assert_eq!(
                (s.theme, s.look),
                (ThemeName::ReduxColor, Look::Neo),
                "{key}"
            );
        }
        let s = c.clone().scoped("flowchart");
        assert_eq!((s.flowchart.look, s.sequence.look), (Look::Neo, Look::Neo));
        for key in ["pie", "gantt", "mindmap", "zenuml", "info"] {
            let s = c.clone().scoped(key);
            assert_eq!(
                (s.theme, s.look),
                (ThemeName::Default, Look::Classic),
                "{key}"
            );
        }
    }

    #[test]
    fn appearance_layers_resolve_like_mermaid() {
        // -t default wins over the diagram default; look is untouched by it.
        let s = MermaidConfig::default()
            .with_cli_theme(ThemeName::Default)
            .scoped("er");
        assert_eq!((s.theme, s.look), (ThemeName::Default, Look::Neo));

        // Within one layer the diagram section beats the top level...
        let site = MermaidConfig::default()
            .with_json(&json(
                r#"{"theme":"dark","look":"classic","er":{"theme":"neutral"}}"#,
            ))
            .unwrap();
        let s = site.clone().scoped("er");
        assert_eq!((s.theme, s.look), (ThemeName::Neutral, Look::Classic));
        assert_eq!(site.clone().scoped("class").theme, ThemeName::Dark);

        // ...but a directive's top-level value beats the site's section value.
        let s = site
            .clone()
            .with_directive(&json(r#"{"theme":"forest"}"#))
            .unwrap()
            .scoped("er");
        assert_eq!(s.theme, ThemeName::Forest);

        // Unusable values are passed over; null clears an earlier value.
        let s = MermaidConfig::default()
            .with_directive(&json(r#"{"look":"sketchy","flowchart":{"theme":"nope"}}"#))
            .unwrap()
            .scoped("flowchart");
        assert_eq!((s.theme, s.look), (ThemeName::ReduxColor, Look::Neo));
        let s = MermaidConfig::default()
            .with_cli_theme(ThemeName::Dark)
            .with_json(&json(r#"{"theme":null}"#))
            .unwrap()
            .scoped("state");
        assert_eq!(s.theme, ThemeName::ReduxColor);

        // A handDrawn look is accepted.
        let s = MermaidConfig::default()
            .with_directive(&json(r#"{"look":"handDrawn"}"#))
            .unwrap()
            .scoped("pie");
        assert_eq!(s.look, Look::HandDrawn);
    }

    #[test]
    fn rejects_unknown_theme() {
        let j = crate::json::parse(r#"{"theme":"sparkly"}"#).unwrap();
        assert!(MermaidConfig::default().with_json(&j).is_err());
    }

    #[test]
    fn yaml_front_matter() {
        let y = parse_yaml_subset(
            "title: \"Hello: world\"\nconfig:\n  theme: dark\n  flowchart:\n    nodeSpacing: 10\n",
        );
        assert_eq!(y.get("title").unwrap().as_str(), Some("Hello: world"));
        let cfg = y.get("config").unwrap();
        assert_eq!(cfg.get("theme").unwrap().as_str(), Some("dark"));
        assert_eq!(
            cfg.get("flowchart")
                .unwrap()
                .get("nodeSpacing")
                .unwrap()
                .as_f64(),
            Some(10.0)
        );
    }
}
