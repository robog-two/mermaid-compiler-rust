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

#[derive(Debug, Clone, PartialEq)]
pub struct GraphConfig {
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
    pub theme: ThemeName,
    pub theme_variables: Vec<(String, String)>,
    pub theme_css: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub flowchart: GraphConfig,
    pub state: GraphConfig,
    pub class: GraphConfig,
    pub sequence: SequenceConfig,
    pub pie: PieConfig,
}

impl Default for MermaidConfig {
    fn default() -> Self {
        MermaidConfig {
            theme: ThemeName::Default,
            theme_variables: Vec::new(),
            theme_css: None,
            font_family: None,
            font_size: None,
            flowchart: GraphConfig::flowchart(),
            state: GraphConfig::state(),
            class: GraphConfig::class(),
            sequence: SequenceConfig::default(),
            pie: PieConfig::default(),
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
    pub fn with_json(mut self, j: &Json) -> Result<Self, String> {
        if j.as_object().is_none() {
            return Err("configuration must be a JSON object".into());
        }
        if let Some(t) = j.get("theme") {
            match t {
                Json::Null => self.theme = ThemeName::Default,
                Json::String(s) => {
                    self.theme = s.parse().map_err(|_| format!("unknown theme \"{s}\""))?;
                }
                _ => return Err("\"theme\" must be a string".into()),
            }
        }
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
        if let Some(f) = j.get("pie") {
            set_f64(f, "textPosition", &mut self.pie.text_position);
            set_bool(f, "useMaxWidth", &mut self.pie.use_max_width);
        }
        Ok(self)
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
