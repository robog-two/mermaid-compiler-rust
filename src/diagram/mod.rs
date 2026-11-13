//! Diagram detection, shared preprocessing and per-diagram parsers/renderers.

pub mod agentflow;
pub mod architecture;
pub mod architecture_fcose;
pub mod block;
pub mod c4;
pub mod class;
pub mod cynefin;
pub mod elkedges;
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
pub mod svgbbox;
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
    let src = normalize_newlines(src);
    let mut config_layers = Vec::new();
    let mut title = None;

    // Front-matter: only at the very start (optionally indented), as mermaid's
    // `frontMatterRegex` requires; its lines (and the blank lines after it) are blanked.
    let mut lines: Vec<String> = src.split('\n').map(str::to_string).collect();
    let chars: Vec<char> = src.chars().collect();
    if let Some(end) = front_matter_end(&chars) {
        let fm_lines = chars[..end].iter().filter(|&&c| c == '\n').count();
        let indent = lines[0].len() - lines[0].trim_start().len();
        let close = (1..fm_lines)
            .rev()
            .find(|&i| lines[i].trim() == "---")
            .unwrap_or(fm_lines.saturating_sub(1));
        let yaml_src: Vec<&str> = lines[1..close]
            .iter()
            .map(|l| {
                l.get(indent..)
                    .filter(|_| l.starts_with(&lines[0][..indent]))
                    .unwrap_or(l)
            })
            .collect();
        let yaml = parse_yaml_subset(&yaml_src.join("\n"));
        title = yaml.get("title").and_then(Json::to_scalar_string);
        if let Some(cfg) = yaml.get("config").filter(|c| c.as_object().is_some()) {
            config_layers.push(cfg.clone());
        }
        for l in &mut lines[..fm_lines] {
            l.clear();
        }
    }
    let text = lines.join("\n");

    // Directives (`directiveRegex`; may span lines, and an unclosed `%%{` is removed
    // up to where the regex stops rather than being an error).
    let mut chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 2 < chars.len() {
        if chars[i] == '%' && chars[i + 1] == '%' && chars[i + 2] == '{' {
            if let Some(end) = directive_end(&chars, i) {
                if chars[..end].ends_with(&['}', '%', '%']) {
                    let inner: String = chars[i + 3..end - 3].iter().collect();
                    if let Some(layer) = parse_directive(inner.trim()) {
                        config_layers.push(layer);
                    }
                }
                for c in &mut chars[i..end] {
                    if *c != '\n' {
                        *c = ' ';
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    let text: String = chars.into_iter().collect();

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
    let text = normalize_newlines(src);
    // Each char tagged with its (0-based) source line; removals filter this sequence
    // exactly as mermaid's string replacements remove text.
    let mut seq: Vec<(char, usize)> = Vec::with_capacity(text.len());
    let mut line = 0;
    for c in text.chars() {
        seq.push((c, line));
        if c == '\n' {
            line += 1;
        }
    }
    let n_lines = line + 1;
    let as_string = |seq: &[(char, usize)]| seq.iter().map(|&(c, _)| c).collect::<Vec<char>>();
    // extractFrontMatter.
    let chars = as_string(&seq);
    if let Some(end) = front_matter_end(&chars) {
        seq.drain(..end);
    }
    // removeDirectives.
    let chars = as_string(&seq);
    let mut keep = vec![true; chars.len()];
    let mut i = 0;
    while i + 2 < chars.len() {
        if chars[i] == '%' && chars[i + 1] == '%' && chars[i + 2] == '{' {
            if let Some(end) = directive_end(&chars, i) {
                keep[i..end].iter_mut().for_each(|k| *k = false);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    let mut it = keep.iter();
    seq.retain(|_| *it.next().unwrap_or(&true));
    // cleanupComments: `/^\s*%%(?!{)[^\n]+\n?/gm`, then `trimStart()`.
    let chars = as_string(&seq);
    let mut keep = vec![true; chars.len()];
    let mut st = 0;
    while st < chars.len() {
        let mut p = st;
        while p < chars.len() && chars[p].is_whitespace() {
            p += 1;
        }
        let is_comment = chars.get(p) == Some(&'%')
            && chars.get(p + 1) == Some(&'%')
            && chars.get(p + 2).is_some_and(|&c| c != '{' && c != '\n');
        if is_comment {
            let mut e = p + 2;
            while e < chars.len() && chars[e] != '\n' {
                e += 1;
            }
            if e < chars.len() {
                e += 1;
            }
            keep[st..e].iter_mut().for_each(|k| *k = false);
            st = e;
            continue;
        }
        // Next line start (`^` with the m flag).
        match chars[st..].iter().position(|&c| c == '\n') {
            Some(k) => st += k + 1,
            None => break,
        }
    }
    let mut it = keep.iter();
    seq.retain(|_| *it.next().unwrap_or(&true));
    let lead = seq.iter().take_while(|&&(c, _)| c.is_whitespace()).count();
    seq.drain(..lead);
    // A source line maps to the cleaned line holding its first surviving char.
    let mut out = vec![None; n_lines];
    let mut cleaned_line = 1;
    for &(c, l) in &seq {
        if out[l].is_none() {
            out[l] = Some(cleaned_line);
        }
        if c == '\n' {
            cleaned_line += 1;
        }
    }
    out
}

/// mermaid's `cleanupText` line endings: `\r\n` and lone `\r` become `\n`.
pub fn normalize_newlines(src: &str) -> String {
    src.replace("\r\n", "\n").replace('\r', "\n")
}

/// End (char index) of a front-matter block at the very start of `s`, per
/// `frontMatterRegex = /^([^\S\n\r]*)-{3}\s*[\n\r](.*?)[\n\r]\1-{3}\s*[\n\r]+/s`.
fn front_matter_end(s: &[char]) -> Option<usize> {
    let indent = s
        .iter()
        .take_while(|&&c| c.is_whitespace() && c != '\n')
        .count();
    let mut p = indent;
    if s.get(p..p + 3) != Some(&['-', '-', '-'][..]) {
        return None;
    }
    p += 3;
    // `\s*[\n\r]`: the body starts after the last newline of the whitespace run.
    let run_end = p + s[p..].iter().take_while(|c| c.is_whitespace()).count();
    let body = s[p..run_end].iter().rposition(|&c| c == '\n')? + p + 1;
    let mut k = body.max(p);
    // Lazy body: the first `\n<indent>---` followed by whitespace holding a newline.
    while k < s.len() {
        if s[k] == '\n' && s[k + 1..].starts_with(&s[..indent]) {
            let q = k + 1 + indent;
            if s.get(q..q + 3) == Some(&['-', '-', '-'][..]) {
                let r = q + 3;
                let ws_end = r + s[r..].iter().take_while(|c| c.is_whitespace()).count();
                if let Some(last_nl) = s[r..ws_end].iter().rposition(|&c| c == '\n') {
                    return Some(r + last_nl + 1);
                }
            }
        }
        k += 1;
    }
    None
}

/// End (char index) of the directive starting at `start` (`%%{`), per mermaid's
/// `directiveRegex = /%{2}{\s*(?:(\w+)\s*:|(\w+))\s*(?:(\w+)|((?:(?!}%{2}).|\r?\n)*))?\s*(?:}%{2})?/gi`
/// (the closing `}%%` is optional). `None` when there is no directive keyword.
fn directive_end(s: &[char], start: usize) -> Option<usize> {
    let word = |p: usize| {
        s[p..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
            .count()
    };
    let ws = |p: usize| s[p..].iter().take_while(|c| c.is_whitespace()).count();
    let mut p = start + 3;
    p += ws(p);
    let w = word(p);
    if w == 0 {
        return None;
    }
    p += w;
    let after = p + ws(p);
    if s.get(after) == Some(&':') {
        p = after + 1;
    }
    p += ws(p);
    let w = word(p);
    if w > 0 {
        p += w;
    } else {
        while p < s.len()
            && !(s[p] == '}' && s.get(p + 1) == Some(&'%') && s.get(p + 2) == Some(&'%'))
        {
            p += 1;
        }
    }
    p += ws(p);
    if s[p..].starts_with(&['}', '%', '%']) {
        p += 3;
    }
    Some(p)
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
    statements_multiline(body, |_| false)
}

/// [`statements`] where a `"…"` string opened by a statement for which `spans(stmt)`
/// holds (`stmt` is the statement text so far, trimmed) may run on across lines, as
/// the jison string states allow (`["]` … `[^"]+` matches newlines): the newlines stay
/// in the statement, which keeps the line number of its first line. A string that is
/// never closed does not swallow the rest of the input: its line is split on its own
/// (as by [`statements`]) so the parser reports the error where the string starts.
pub fn statements_multiline(body: &str, spans: impl Fn(&str) -> bool) -> Vec<(usize, String)> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // Lines whose open string must not carry on (it is never closed).
    let mut no_carry = std::collections::HashSet::new();
    'outer: while i < lines.len() {
        let mut cur = String::new();
        let mut in_quote = false;
        let start = i;
        let mut stmt_line = i;
        let mut j = i;
        let emitted = out.len();
        loop {
            for c in lines[j].chars() {
                match c {
                    '"' => {
                        in_quote = !in_quote;
                        cur.push(c);
                    }
                    ';' if !in_quote => {
                        if !cur.trim().is_empty() {
                            out.push((stmt_line + 1, cur.trim().to_string()));
                        }
                        cur.clear();
                        stmt_line = j;
                    }
                    _ => cur.push(c),
                }
            }
            let carry = in_quote && !no_carry.contains(&stmt_line) && spans(cur.trim());
            if !carry {
                break;
            }
            if j + 1 == lines.len() {
                // Unterminated string: redo from `start` without carrying from `j`.
                no_carry.insert(stmt_line);
                out.truncate(emitted);
                i = start;
                continue 'outer;
            }
            cur.push('\n');
            j += 1;
        }
        if !cur.trim().is_empty() {
            out.push((stmt_line + 1, cur.trim().to_string()));
        }
        i = j + 1;
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
        assert_eq!(m[7], None, "a directive's text is removed");
        assert_eq!(m[8], Some(3), "what follows it stays on one line");
        assert_eq!(m[9], Some(4));
        // Without front-matter or comments the numbering is unchanged.
        assert_eq!(
            mermaid_line_numbers("graph TD\nA-->B"),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn error_lines_follow_mermaids_regexes() {
        let line_of = |src: &str, needle: &str| {
            let idx = src.split('\n').position(|l| l.contains(needle)).unwrap();
            mermaid_line_numbers(src)[idx]
        };
        // `\s*` in the comment regex also takes the blank lines before a comment.
        let s = "flowchart TD\n A --> B\n\n\n %% c\n C --> --> D";
        assert_eq!(line_of(s, "C -->"), Some(3));
        // A multi-line directive at the top is trimmed away entirely.
        let s = "%%{init: {\n 'theme':'dark'\n}}%%\nflowchart TD\n A --> B\n C --> --> D";
        assert_eq!(line_of(s, "C -->"), Some(3));
        // Indented front-matter.
        let s = "  ---\n  title: x\n  ---\nflowchart TD\n A-->B\n C --> --> D";
        assert_eq!(line_of(s, "C -->"), Some(3));
        // Lone CR line endings count as newlines.
        assert_eq!(mermaid_line_numbers("graph TD\rA-->B").len(), 2);
    }

    #[test]
    fn front_matter_and_directives_like_mermaid() {
        let p = preprocess("  ---\n  title: Hi\n  ---\ngraph TD\nA-->B").unwrap();
        assert_eq!(p.title.as_deref(), Some("Hi"));
        assert_eq!(detect(&p.body).unwrap(), DiagramType::Flowchart);
        // Front-matter after a blank line is not front-matter for mermaid.
        let p = preprocess("\n---\ntitle: Hi\n---\ngraph TD").unwrap();
        assert_eq!(p.title, None);
        // An unclosed directive is removed up to where the regex stops, not an error.
        let p = preprocess("flowchart TD\n A --> B\n%%{ todo fix this\n B --> C").unwrap();
        assert!(p.body.contains("this"), "{:?}", p.body);
        assert!(!p.body.contains("todo"));
    }

    #[test]
    fn multiline_strings_join_lines_and_keep_the_first_line_number() {
        let all = |_: &str| true;
        let s = statements_multiline("graph TD\nA[\"one\ntwo\"] --> B; C\nD", all);
        assert_eq!(
            s,
            vec![
                (1, "graph TD".into()),
                (2, "A[\"one\ntwo\"] --> B".into()),
                (3, "C".into()),
                (4, "D".into())
            ]
        );
        // `;` inside the multi-line string does not split it.
        let s = statements_multiline("A[\"a;\nb\"];B", all);
        assert_eq!(s, vec![(1, "A[\"a;\nb\"]".into()), (2, "B".into())]);
        // An unterminated string falls back to one statement per line.
        let s = statements_multiline("graph TD\nA[\"open\nB --> C", all);
        assert_eq!(
            s,
            vec![
                (1, "graph TD".into()),
                (2, "A[\"open".into()),
                (3, "B --> C".into())
            ]
        );
        // Statements the predicate rejects never carry.
        let s = statements_multiline("S1 : 5\" screen\nS2 : 7\" screen", |s| {
            s.starts_with("state ")
        });
        assert_eq!(s.len(), 2);
        assert_eq!(statements("A[\"x\ny\"]").len(), 2);
    }
}
