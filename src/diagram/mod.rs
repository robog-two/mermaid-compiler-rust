//! Diagram detection, shared preprocessing and per-diagram parsers/renderers.

pub mod architecture;
pub mod block;
pub mod c4;
pub mod class;
pub mod er;
pub mod flowchart;
pub mod gantt;
pub mod gitgraph;
pub mod graph;
pub mod info;
pub mod journey;
pub mod kanban;
pub mod markers;
pub mod mindmap;
pub mod packet;
pub mod pie;
pub mod quadrant;
pub mod radar;
pub mod requirement;
pub mod sankey;
pub mod sequence;
pub mod shapes;
pub mod state;
pub mod timeline;
pub mod treemap;
pub mod xychart;
pub mod zenuml;

use crate::config::parse_yaml_subset;
use crate::error::{Error, Result};
use crate::json::{self, Json};

/// Diagram source after mermaid's generic preprocessing. Removed lines are
/// blanked rather than deleted so parser line numbers match the input.
#[derive(Debug, Clone, PartialEq)]
pub struct Preprocessed {
    pub body: String,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    /// Config layers in application order (front-matter, then directives).
    pub config_layers: Vec<Json>,
}

/// Supported diagram families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramType {
    Flowchart,
    Sequence,
    Pie,
    State,
    Class,
    Er,
    Gantt,
    Journey,
    GitGraph,
    Mindmap,
    Timeline,
    Quadrant,
    Requirement,
    C4,
    Sankey,
    XyChart,
    Block,
    Packet,
    Architecture,
    Kanban,
    Radar,
    Treemap,
    ZenUml,
    Info,
}

/// Strip front-matter, directives, comments and accessibility statements.
pub fn preprocess(src: &str) -> Result<Preprocessed> {
    let src = src.replace("\r\n", "\n");
    let mut config_layers = Vec::new();
    let mut title = None;

    // Front-matter: must be the first non-blank content.
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    if let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) {
        if lines[first].trim() == "---" {
            if let Some(end) = lines
                .iter()
                .skip(first + 1)
                .position(|l| l.trim() == "---")
                .map(|p| p + first + 1)
            {
                let yaml = parse_yaml_subset(&lines[first + 1..end].join("\n"));
                title = yaml.get("title").and_then(Json::to_scalar_string);
                if let Some(cfg) = yaml.get("config").filter(|c| c.as_object().is_some()) {
                    config_layers.push(cfg.clone());
                }
                if let Some(mode) = yaml.get("displayMode").and_then(Json::as_str) {
                    let _ = mode; // gantt-only option, accepted for compatibility
                }
                for l in &mut lines[first..=end] {
                    l.clear();
                }
            }
        }
    }
    let mut text = lines.join("\n");

    // Directives: %%{ init: {...} }%% (may span lines).
    while let Some(start) = text.find("%%{") {
        let Some(rel_end) = text[start..].find("}%%") else {
            let line = text[..start].matches('\n').count() + 1;
            return Err(Error::parse(line, "Unterminated directive: expected '}%%'"));
        };
        let end = start + rel_end + 3;
        let inner = text[start + 3..end - 3].trim();
        if let Some(layer) = parse_directive(inner) {
            config_layers.push(layer);
        }
        let blank: String = text[start..end]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        text.replace_range(start..end, &blank);
    }

    // Comments and accessibility statements.
    let mut acc_title = None;
    let mut acc_descr = None;
    let mut out: Vec<String> = Vec::new();
    let mut in_descr_block: Option<Vec<String>> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(buf) = in_descr_block.as_mut() {
            if let Some(idx) = t.find('}') {
                buf.push(t[..idx].trim().to_string());
                acc_descr = Some(
                    buf.iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                in_descr_block = None;
            } else {
                buf.push(t.to_string());
            }
            out.push(String::new());
            continue;
        }
        if t.starts_with("%%") {
            out.push(String::new());
            continue;
        }
        if let Some(rest) = t.strip_prefix("accTitle") {
            if let Some(v) = rest.trim_start().strip_prefix(':') {
                acc_title = Some(v.trim().to_string());
                out.push(String::new());
                continue;
            }
        }
        if let Some(rest) = t.strip_prefix("accDescr") {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix(':') {
                acc_descr = Some(v.trim().to_string());
                out.push(String::new());
                continue;
            }
            if let Some(v) = rest.strip_prefix('{') {
                if let Some(idx) = v.find('}') {
                    acc_descr = Some(v[..idx].trim().to_string());
                } else {
                    in_descr_block = Some(vec![v.trim().to_string()]);
                }
                out.push(String::new());
                continue;
            }
        }
        out.push(line.to_string());
    }
    Ok(Preprocessed {
        body: out.join("\n"),
        title,
        acc_title,
        acc_descr,
        config_layers,
    })
}

fn parse_directive(inner: &str) -> Option<Json> {
    let (kind, rest) = match inner.find(':') {
        Some(i) => (
            inner[..i].trim().trim_matches(['"', '\'']),
            inner[i + 1..].trim(),
        ),
        None => (inner, ""),
    };
    match kind {
        "init" | "initialize" | "config" => json::parse_lenient(rest).ok(),
        _ => None, // `wrap` and unknown directives are ignored, like mermaid
    }
}

/// Detect the diagram type from the first meaningful line.
pub fn detect(body: &str) -> Result<DiagramType> {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let word: String = first
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ';' && *c != ':')
        .collect();
    match word.as_str() {
        "graph" | "flowchart" | "flowchart-v2" | "flowchart-elk" => Ok(DiagramType::Flowchart),
        "sequenceDiagram" => Ok(DiagramType::Sequence),
        "pie" => Ok(DiagramType::Pie),
        "stateDiagram" | "stateDiagram-v2" => Ok(DiagramType::State),
        "classDiagram" | "classDiagram-v2" => Ok(DiagramType::Class),
        "erDiagram" => Ok(DiagramType::Er),
        "gantt" => Ok(DiagramType::Gantt),
        "journey" => Ok(DiagramType::Journey),
        "gitGraph" => Ok(DiagramType::GitGraph),
        "mindmap" => Ok(DiagramType::Mindmap),
        "timeline" => Ok(DiagramType::Timeline),
        "quadrantChart" => Ok(DiagramType::Quadrant),
        "requirementDiagram" | "requirement" => Ok(DiagramType::Requirement),
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment" => {
            Ok(DiagramType::C4)
        }
        "sankey-beta" | "sankey" => Ok(DiagramType::Sankey),
        "xychart-beta" | "xychart" => Ok(DiagramType::XyChart),
        "block-beta" | "block" => Ok(DiagramType::Block),
        "packet-beta" | "packet" => Ok(DiagramType::Packet),
        "architecture-beta" | "architecture" => Ok(DiagramType::Architecture),
        "kanban" => Ok(DiagramType::Kanban),
        "radar-beta" | "radar" => Ok(DiagramType::Radar),
        "treemap-beta" | "treemap" => Ok(DiagramType::Treemap),
        "zenuml" => Ok(DiagramType::ZenUml),
        "info" => Ok(DiagramType::Info),
        _ => Err(Error::UnknownDiagram {
            text: body.trim().to_string(),
        }),
    }
}

/// Split a diagram body into statements (newline or `;` separated) keeping
/// 1-based line numbers. Semicolons inside quotes are preserved.
pub fn statements(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let mut cur = String::new();
        let mut in_quote = false;
        for c in line.chars() {
            match c {
                '"' => {
                    in_quote = !in_quote;
                    cur.push(c);
                }
                ';' if !in_quote => {
                    if !cur.trim().is_empty() {
                        out.push((i + 1, cur.trim().to_string()));
                    }
                    cur.clear();
                }
                _ => cur.push(c),
            }
        }
        if !cur.trim().is_empty() {
            out.push((i + 1, cur.trim().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_front_matter_and_directives() {
        let src = "---\ntitle: Hi\nconfig:\n  theme: dark\n---\n%%{init: {'theme':'forest'}}%%\n%% comment\ngraph TD\naccTitle: T\naccDescr { multi\nline }\nA-->B";
        let p = preprocess(src).unwrap();
        assert_eq!(p.title.as_deref(), Some("Hi"));
        assert_eq!(p.config_layers.len(), 2);
        assert_eq!(p.acc_title.as_deref(), Some("T"));
        assert_eq!(p.acc_descr.as_deref(), Some("multi\nline"));
        assert_eq!(detect(&p.body).unwrap(), DiagramType::Flowchart);
        // Line numbers preserved: "A-->B" is on line 12.
        assert_eq!(p.body.lines().nth(11), Some("A-->B"));
    }

    #[test]
    fn detects_types() {
        assert_eq!(
            detect("  sequenceDiagram\n").unwrap(),
            DiagramType::Sequence
        );
        assert_eq!(detect("pie title X").unwrap(), DiagramType::Pie);
        assert_eq!(detect("graph;A-->B").unwrap(), DiagramType::Flowchart);
        assert_eq!(detect("gantt\n").unwrap(), DiagramType::Gantt);
        assert_eq!(detect("C4Container\n").unwrap(), DiagramType::C4);
        assert_eq!(detect("gitGraph:\n").unwrap(), DiagramType::GitGraph);
        assert_eq!(detect("xychart-beta\n").unwrap(), DiagramType::XyChart);
        assert!(matches!(detect("hello"), Err(Error::UnknownDiagram { .. })));
    }

    #[test]
    fn statement_splitting() {
        let s = statements("graph TD;A-->B;\nC[\"a;b\"]");
        assert_eq!(
            s,
            vec![
                (1, "graph TD".into()),
                (1, "A-->B".into()),
                (2, "C[\"a;b\"]".into())
            ]
        );
    }
}
