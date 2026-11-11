//! Diagram detection, shared preprocessing and per-diagram parsers/renderers.

pub mod agentflow;
pub mod architecture;
pub mod block;
pub mod c4;
pub mod class;
pub mod cynefin;
pub mod er;
pub mod eventmodeling;
pub mod flowchart;
pub mod gantt;
pub mod gitgraph;
pub mod graph;
pub mod info;
pub mod ishikawa;
pub mod journey;
pub mod kanban;
pub mod markers;
pub mod mindmap;
pub mod packet;
pub mod pie;
pub mod quadrant;
pub mod radar;
pub mod railroad;
pub mod requirement;
pub mod sankey;
pub mod sequence;
pub mod shapes;
pub mod state;
pub mod swimlane;
pub mod timeline;
pub mod treemap;
pub mod treeview;
pub mod usecase;
pub mod venn;
pub mod wardley;
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
    Agentflow,
    Swimlane,
    EventModeling,
    TreeView,
    Ishikawa,
    Railroad,
    Venn,
    Wardley,
    Cynefin,
    Usecase,
}

impl DiagramType {
    /// The `MermaidConfig` section holding this diagram's settings (mermaid's
    /// `getDiagramConfigKey`), which also scopes its `theme` / `look` resolution.
    pub fn config_key(self) -> &'static str {
        match self {
            DiagramType::Flowchart => "flowchart",
            DiagramType::Sequence => "sequence",
            DiagramType::Pie => "pie",
            DiagramType::State => "state",
            DiagramType::Class => "class",
            DiagramType::Er => "er",
            DiagramType::Gantt => "gantt",
            DiagramType::Journey => "journey",
            DiagramType::GitGraph => "gitGraph",
            DiagramType::Mindmap => "mindmap",
            DiagramType::Timeline => "timeline",
            DiagramType::Quadrant => "quadrantChart",
            DiagramType::Requirement => "requirement",
            DiagramType::C4 => "c4",
            DiagramType::Sankey => "sankey",
            DiagramType::XyChart => "xyChart",
            DiagramType::Block => "block",
            DiagramType::Packet => "packet",
            DiagramType::Architecture => "architecture",
            DiagramType::Kanban => "kanban",
            DiagramType::Radar => "radar",
            DiagramType::Treemap => "treemap",
            DiagramType::ZenUml => "zenuml",
            DiagramType::Info => "info",
            DiagramType::Agentflow => "agentflow",
            DiagramType::Swimlane => "swimlane",
            DiagramType::EventModeling => "eventmodeling",
            DiagramType::TreeView => "treeView",
            DiagramType::Ishikawa => "ishikawa",
            DiagramType::Railroad => "railroad",
            DiagramType::Venn => "venn",
            DiagramType::Wardley => "wardley",
            DiagramType::Cynefin => "cynefin",
            DiagramType::Usecase => "usecase",
        }
    }
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

/// For each 1-based source line, the line number mermaid reports for it in parse
/// errors (index `line - 1`), or `None` for lines mermaid deletes before parsing.
///
/// mermaid's `preprocessDiagram` parses text from which it has removed the front-matter
/// (and the blank lines straight after it), every `%%{...}%%` directive (a directive
/// spanning lines collapses onto one), every `%% comment` line (`cleanupComments`), and
/// then leading whitespace (`trimStart`). Error positions count lines of that text.
pub fn mermaid_line_numbers(src: &str) -> Vec<Option<usize>> {
    let src = src.replace("\r\n", "\n");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut keep = vec![true; lines.len()];
    // Front-matter: `---` on the very first line, up to the closing `---` and the
    // blank lines after it.
    if lines.first().map(|l| l.trim_end()) == Some("---") {
        if let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---") {
            let mut last = end + 1;
            while last + 1 < lines.len() && lines[last + 1].trim().is_empty() {
                last += 1;
            }
            keep[..=last].iter_mut().for_each(|k| *k = false);
        }
    }
    // Directives: lines after the first of a multi-line directive merge into it.
    let mut i = 0;
    while i < lines.len() {
        if keep[i] {
            if let Some(start) = lines[i].find("%%{") {
                if !lines[i][start..].contains("}%%") {
                    let mut j = i + 1;
                    while j < lines.len() && !lines[j].contains("}%%") {
                        keep[j] = false;
                        j += 1;
                    }
                    if j < lines.len() {
                        keep[j] = false;
                    }
                    i = j;
                }
            }
        }
        i += 1;
    }
    // Comment lines (`/^\s*%%(?!{)[^\n]+\n?/gm`).
    for (k, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("%%") {
            if !rest.is_empty() && !rest.starts_with('{') {
                keep[k] = false;
            }
        }
    }
    // `trimStart`: leading lines that are blank (or held only a directive) go too.
    for k in 0..lines.len() {
        if !keep[k] {
            continue;
        }
        let blank = lines[k].trim().is_empty()
            || (lines[k].trim_start().starts_with("%%{") && lines[k].trim_end().ends_with("}%%"));
        if !blank {
            break;
        }
        keep[k] = false;
    }
    let mut n = 0;
    keep.iter()
        .map(|&k| {
            k.then(|| {
                n += 1;
                n
            })
        })
        .collect()
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
    // Newer detectors are regex prefixes (`/^\s*venn-beta/`), some case-insensitive,
    // so they are checked on the raw first line before the keyword match.
    let lower = first.to_ascii_lowercase();
    let word_end = |rest: &str, extra: &[char]| {
        rest.chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_') || extra.contains(&c))
    };
    if let Some(rest) = first.strip_prefix("agentflow-beta") {
        if word_end(rest, &[]) {
            return Ok(DiagramType::Agentflow);
        }
    }
    if let Some(rest) = first.strip_prefix("swimlane-beta") {
        if word_end(rest, &[]) {
            return Ok(DiagramType::Swimlane);
        }
    }
    if [
        "railroad-beta",
        "railroad-ebnf-beta",
        "railroad-abnf-beta",
        "railroad-peg-beta",
    ]
    .iter()
    .any(|k| lower.starts_with(k))
    {
        return Ok(DiagramType::Railroad);
    }
    if first.starts_with("eventmodeling") {
        return Ok(DiagramType::EventModeling);
    }
    if first.starts_with("treeView-beta") {
        return Ok(DiagramType::TreeView);
    }
    for k in ["ishikawa-beta", "ishikawa"] {
        if let Some(rest) = lower.strip_prefix(k) {
            if word_end(rest, &[]) && !rest.starts_with('-') {
                return Ok(DiagramType::Ishikawa);
            }
        }
    }
    if first.starts_with("venn-beta") {
        return Ok(DiagramType::Venn);
    }
    if lower.starts_with("wardley-beta") {
        return Ok(DiagramType::Wardley);
    }
    if let Some(rest) = first.strip_prefix("cynefin-beta") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with(':') {
            return Ok(DiagramType::Cynefin);
        }
    }
    if let Some(rest) = first.strip_prefix("usecase-beta") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return Ok(DiagramType::Usecase);
        }
    }
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

    #[test]
    fn detects_mermaid_12_3_types_like_their_regex_detectors() {
        let cases = [
            ("agentflow-beta\n", DiagramType::Agentflow),
            ("swimlane-beta LR\n", DiagramType::Swimlane),
            ("eventmodeling\n", DiagramType::EventModeling),
            ("treeView-beta\n", DiagramType::TreeView),
            ("ishikawa\n", DiagramType::Ishikawa),
            ("Ishikawa-beta\n", DiagramType::Ishikawa),
            ("railroad-beta\n", DiagramType::Railroad),
            ("railroad-EBNF-beta\n", DiagramType::Railroad),
            ("railroad-abnf-beta\n", DiagramType::Railroad),
            ("railroad-peg-beta\n", DiagramType::Railroad),
            ("venn-beta\n", DiagramType::Venn),
            ("Wardley-beta\n", DiagramType::Wardley),
            ("cynefin-beta: t\n", DiagramType::Cynefin),
            ("usecase-beta\n", DiagramType::Usecase),
        ];
        for (src, kind) in cases {
            assert_eq!(detect(src).unwrap(), kind, "{src:?}");
        }
        assert!(detect("usecase-betax\n").is_err());
        assert!(detect("cynefin-betax\n").is_err());
    }

    #[test]
    fn error_lines_count_like_mermaid_after_preprocessing() {
        let src = "---\ntitle: T\n---\n\nerDiagram\n    %% a comment\n    A ||--o{ B : x\n%%{init: {\n 'theme':'dark'}}%%\n    bad line";
        let m = mermaid_line_numbers(src);
        assert_eq!(&m[..5], &[None, None, None, None, Some(1)]);
        assert_eq!(m[5], None, "comment line removed");
        assert_eq!(m[6], Some(2));
        assert_eq!(m[7], Some(3), "a directive collapses onto its first line");
        assert_eq!(m[8], None);
        assert_eq!(m[9], Some(4));
        // Without front-matter or comments the numbering is unchanged.
        assert_eq!(
            mermaid_line_numbers("graph TD\nA-->B"),
            vec![Some(1), Some(2)]
        );
    }
}
