//! `requirementDiagram` diagrams → `GraphDiagram` with requirement boxes.
//!
//! Supported syntax (mermaid 11): the six requirement types with `id`, `text`,
//! `risk` and `verifymethod` fields, `element` blocks with `type` and `docref`,
//! relationships in both directions (`a - satisfies -> b`, `b <- copies - a`) for
//! `contains`, `copies`, `derives`, `satisfies`, `verifies`, `refines`, `traces`,
//! quoted names/values, `direction`, `classDef`, `class`, `style` and `:::class`.

use super::graph::StyleOverrides;
use super::graph::{
    self, Edge, Endpoint, GraphDiagram, Head, LineKind, Node, NodeId, Section, Shape,
};
use crate::config::{Curve, GraphConfig, MermaidConfig};
use crate::error::{Error, Result};
use crate::layout::Direction;
use crate::scene::{DiagramKind, Scene};
use crate::theme::{ColorSlot, Palette};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementType {
    Requirement,
    Functional,
    Interface,
    Performance,
    Physical,
    DesignConstraint,
}

impl RequirementType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "requirement" => RequirementType::Requirement,
            "functionalrequirement" => RequirementType::Functional,
            "interfacerequirement" => RequirementType::Interface,
            "performancerequirement" => RequirementType::Performance,
            "physicalrequirement" => RequirementType::Physical,
            "designconstraint" => RequirementType::DesignConstraint,
            _ => return None,
        })
    }
    pub fn display(self) -> &'static str {
        match self {
            RequirementType::Requirement => "Requirement",
            RequirementType::Functional => "Functional Requirement",
            RequirementType::Interface => "Interface Requirement",
            RequirementType::Performance => "Performance Requirement",
            RequirementType::Physical => "Physical Requirement",
            RequirementType::DesignConstraint => "Design Constraint",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMethod {
    Analysis,
    Demonstration,
    Inspection,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelKind {
    Contains,
    Copies,
    Derives,
    Satisfies,
    Verifies,
    Refines,
    Traces,
}

impl RelKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "contains" => RelKind::Contains,
            "copies" => RelKind::Copies,
            "derives" => RelKind::Derives,
            "satisfies" => RelKind::Satisfies,
            "verifies" => RelKind::Verifies,
            "refines" => RelKind::Refines,
            "traces" => RelKind::Traces,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            RelKind::Contains => "contains",
            RelKind::Copies => "copies",
            RelKind::Derives => "derives",
            RelKind::Satisfies => "satisfies",
            RelKind::Verifies => "verifies",
            RelKind::Refines => "refines",
            RelKind::Traces => "traces",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Requirement {
        ty: RequirementType,
        id: Option<String>,
        text: Option<String>,
        risk: Option<Risk>,
        verify: Option<VerifyMethod>,
    },
    Element {
        ty: Option<String>,
        docref: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub classes: Vec<String>,
    pub style: StyleOverrides,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    pub src: String,
    pub dst: String,
    pub kind: RelKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequirementDiagram {
    pub title: Option<String>,
    pub direction: Direction,
    pub items: Vec<Item>,
    pub relations: Vec<Relation>,
}

/// Parse and render a `requirementDiagram` diagram body (already preprocessed).
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let d = parse(body, title)?;
    let g = to_graph(&d);
    let gc = GraphConfig {
        look: cfg.look,
        node_spacing: cfg.section_f64("requirement", "nodeSpacing", 50.0),
        rank_spacing: cfg.section_f64("requirement", "rankSpacing", 50.0),
        padding: cfg.section_f64("requirement", "padding", 14.0),
        diagram_padding: cfg.section_f64("requirement", "diagramPadding", 20.0),
        curve: Curve::Basis,
        use_max_width: cfg.use_max_width("requirement"),
        title_top_margin: 25.0,
        min_node_width: 0.0,
        wrapping_width: 0.0,
        layout: cfg.layout,
        line_hops: cfg.elk_line_hops(),
        html_labels: true,
        html_edge_labels: true,
    };
    Ok(graph::render(&g, &gc, pal))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

fn split_ids(s: &str) -> Vec<String> {
    s.split(',')
        .map(unquote)
        .filter(|x| !x.is_empty())
        .collect()
}

/// `name`, `"quoted name"`, optionally followed by `:::cls`, then `{`.
fn block_header(rest: &str, line: usize, what: &str) -> Result<(String, Vec<String>)> {
    let head = rest
        .trim()
        .strip_suffix('{')
        .ok_or_else(|| Error::parse(line, format!("Expecting '{{' after the {what} name")))?
        .trim();
    let (name, classes) = match head.split_once(":::") {
        Some((n, c)) => (n, split_ids(c)),
        None => (head, Vec::new()),
    };
    let name = unquote(name);
    if name.is_empty() {
        return Err(Error::parse(line, format!("Expecting a {what} name")));
    }
    Ok((name, classes))
}

struct Open {
    idx: usize,
    line: usize,
}

/// Parse a requirement diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<RequirementDiagram> {
    let mut d = RequirementDiagram {
        title,
        direction: Direction::TopBottom,
        items: Vec::new(),
        relations: Vec::new(),
    };
    let mut header = false;
    let mut open: Option<Open> = None;
    let mut class_defs: HashMap<String, StyleOverrides> = HashMap::new();
    let mut class_assign: Vec<(String, String)> = Vec::new();
    let mut styles: Vec<(String, StyleOverrides)> = Vec::new();

    for (idx, raw) in body.lines().enumerate() {
        let line = idx + 1;
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        if !header {
            if !(s == "requirementDiagram" || s == "requirement") {
                return Err(Error::parse(
                    line,
                    format!("{s}\nExpecting 'requirementDiagram'"),
                ));
            }
            header = true;
            continue;
        }
        if let Some(o) = &open {
            if s == "}" {
                open = None;
                continue;
            }
            let (key, value) = s.split_once(':').ok_or_else(|| {
                Error::parse(line, format!("{s}\nExpecting '<field>: <value>' or '}}'"))
            })?;
            let key = key.trim().to_ascii_lowercase();
            let value = unquote(value);
            let item = &mut d.items[o.idx];
            match &mut item.kind {
                ItemKind::Requirement {
                    id,
                    text,
                    risk,
                    verify,
                    ..
                } => match key.as_str() {
                    "id" => *id = Some(value),
                    "text" => *text = Some(value),
                    "risk" => {
                        *risk = Some(match value.to_ascii_lowercase().as_str() {
                            "low" => Risk::Low,
                            "medium" => Risk::Medium,
                            "high" => Risk::High,
                            _ => {
                                return Err(Error::parse(
                                    line,
                                    format!(
                                        "Invalid risk '{value}': expecting low, medium or high"
                                    ),
                                ));
                            }
                        })
                    }
                    "verifymethod" => {
                        *verify = Some(match value.to_ascii_lowercase().as_str() {
                            "analysis" => VerifyMethod::Analysis,
                            "demonstration" => VerifyMethod::Demonstration,
                            "inspection" => VerifyMethod::Inspection,
                            "test" => VerifyMethod::Test,
                            _ => {
                                return Err(Error::parse(
                                    line,
                                    format!(
                                        "Invalid verifymethod '{value}': expecting analysis, demonstration, inspection or test"
                                    ),
                                ));
                            }
                        })
                    }
                    _ => {
                        return Err(Error::parse(
                            line,
                            format!(
                                "Unknown requirement field '{key}': expecting id, text, risk or verifymethod"
                            ),
                        ));
                    }
                },
                ItemKind::Element { ty, docref } => match key.as_str() {
                    "type" => *ty = Some(value),
                    "docref" => *docref = Some(value),
                    _ => {
                        return Err(Error::parse(
                            line,
                            format!("Unknown element field '{key}': expecting type or docref"),
                        ));
                    }
                },
            }
            continue;
        }
        let (first, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        if first.eq_ignore_ascii_case("direction") {
            d.direction = Direction::parse(&rest.trim().to_ascii_uppercase()).ok_or_else(|| {
                Error::parse(line, format!("Invalid direction '{}'", rest.trim()))
            })?;
            continue;
        }
        if first == "title" {
            d.title = Some(rest.trim().to_string()).filter(|t| !t.is_empty());
            continue;
        }
        if first == "classDef" {
            let (names, st) = rest
                .trim()
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting styles after classDef names"))?;
            for n in split_ids(names) {
                class_defs.insert(n, StyleOverrides::parse(st));
            }
            continue;
        }
        if first == "class" {
            let (ids, cls) = rest
                .trim()
                .rsplit_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting 'class <ids> <className>'"))?;
            for id in split_ids(ids) {
                class_assign.push((id, cls.trim().to_string()));
            }
            continue;
        }
        if first == "style" {
            let (ids, st) = rest
                .trim()
                .split_once(char::is_whitespace)
                .ok_or_else(|| Error::parse(line, "Expecting styles after style ids"))?;
            for id in split_ids(ids) {
                styles.push((id, StyleOverrides::parse(st)));
            }
            continue;
        }
        let kind = if first.eq_ignore_ascii_case("element") {
            Some(ItemKind::Element {
                ty: None,
                docref: None,
            })
        } else {
            RequirementType::parse(first).map(|ty| ItemKind::Requirement {
                ty,
                id: None,
                text: None,
                risk: None,
                verify: None,
            })
        };
        if let Some(kind) = kind {
            let what = if matches!(kind, ItemKind::Element { .. }) {
                "element"
            } else {
                "requirement"
            };
            let (name, classes) = block_header(rest, line, what)?;
            let idx = match d.items.iter().position(|i| i.name == name) {
                Some(i) => {
                    d.items[i].kind = kind;
                    d.items[i].classes.extend(classes);
                    i
                }
                None => {
                    d.items.push(Item {
                        name,
                        kind,
                        classes,
                        style: StyleOverrides::default(),
                    });
                    d.items.len() - 1
                }
            };
            open = Some(Open { idx, line });
            continue;
        }
        if let Some(rel) = parse_relation(s) {
            d.relations.push(rel);
            continue;
        }
        return Err(Error::parse(
            line,
            format!("{s}\nExpecting a requirement, element, relationship or style statement"),
        ));
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'requirementDiagram'"));
    }
    if let Some(o) = open {
        return Err(Error::parse(
            o.line,
            format!(
                "Unterminated block for '{}': missing '}}'",
                d.items[o.idx].name
            ),
        ));
    }
    for (id, cls) in class_assign {
        if let Some(i) = d.items.iter_mut().find(|i| i.name == id) {
            i.classes.push(cls);
        }
    }
    for i in d.items.iter_mut() {
        let mut st = class_defs.get("default").cloned().unwrap_or_default();
        for c in &i.classes {
            if let Some(def) = class_defs.get(c) {
                st = st.merged(def);
            }
        }
        i.style = st;
    }
    for (id, st) in styles {
        if let Some(i) = d.items.iter_mut().find(|i| i.name == id) {
            i.style = i.style.merged(&st);
        }
    }
    Ok(d)
}

/// `a - satisfies -> b` or `b <- satisfies - a` (source is always the `-` side).
fn parse_relation(s: &str) -> Option<Relation> {
    if let Some((lhs, rhs)) = s.split_once("->") {
        let (src, kind) = lhs.trim().rsplit_once(" - ")?;
        let kind = RelKind::parse(kind.trim())?;
        let (src, dst) = (unquote(src), unquote(rhs));
        return (!src.is_empty() && !dst.is_empty()).then_some(Relation { src, dst, kind });
    }
    if let Some((lhs, rhs)) = s.split_once("<-") {
        let (kind, src) = rhs.trim().split_once(" - ")?;
        let kind = RelKind::parse(kind.trim())?;
        let (src, dst) = (unquote(src), unquote(lhs));
        return (!src.is_empty() && !dst.is_empty()).then_some(Relation { src, dst, kind });
    }
    None
}

/// Palette slots in requirementDb's `getData` order: every requirement first (in
/// declaration order), then every element, each taking `nodes.length`.
fn requirement_slots(items: &[Item]) -> Vec<ColorSlot> {
    let is_req = |i: &Item| matches!(i.kind, ItemKind::Requirement { .. });
    let reqs = items.iter().filter(|i| is_req(i)).count();
    let (mut r, mut e) = (0, reqs);
    items
        .iter()
        .map(|i| {
            let n = if is_req(i) { &mut r } else { &mut e };
            *n += 1;
            ColorSlot(*n - 1)
        })
        .collect()
}

/// Lower to the shared graph IR. Relationship endpoints that were never defined
/// become plain elements, as in mermaid.
pub fn to_graph(d: &RequirementDiagram) -> GraphDiagram {
    let mut items = d.items.clone();
    for r in &d.relations {
        for name in [&r.src, &r.dst] {
            if !items.iter().any(|i| &i.name == name) {
                items.push(Item {
                    name: name.clone(),
                    kind: ItemKind::Element {
                        ty: None,
                        docref: None,
                    },
                    classes: Vec::new(),
                    style: StyleOverrides::default(),
                });
            }
        }
    }
    let slots = requirement_slots(&items);
    let nodes = items
        .iter()
        .zip(slots)
        .map(|(i, slot)| {
            let mut n = Node::new(i.name.clone(), i.name.clone(), Shape::Requirement);
            n.color_slot = Some(slot);
            n.style = i.style.clone();
            n.classes = i.classes.clone();
            let (stereo, fields) = match &i.kind {
                ItemKind::Requirement {
                    ty,
                    id,
                    text,
                    risk,
                    verify,
                } => {
                    let mut f = Vec::new();
                    if let Some(v) = id {
                        f.push(format!("ID: {v}"));
                    }
                    if let Some(v) = text {
                        f.push(format!("Text: {v}"));
                    }
                    if let Some(v) = risk {
                        f.push(format!("Risk: {v:?}"));
                    }
                    if let Some(v) = verify {
                        f.push(format!("Verification: {v:?}"));
                    }
                    (format!("<<{}>>", ty.display()), f)
                }
                ItemKind::Element { ty, docref } => {
                    let mut f = Vec::new();
                    if let Some(v) = ty {
                        f.push(format!("Type: {v}"));
                    }
                    if let Some(v) = docref {
                        f.push(format!("Doc Ref: {v}"));
                    }
                    ("<<Element>>".to_string(), f)
                }
            };
            n.sections = vec![
                Section {
                    lines: vec![stereo],
                    centered: true,
                    bold: false,
                },
                Section {
                    lines: fields,
                    centered: true,
                    bold: false,
                },
            ];
            n
        })
        .collect();
    let index = |name: &str| items.iter().position(|i| i.name == name);
    let edges = d
        .relations
        .iter()
        .filter_map(|r| {
            let from = index(&r.src)?;
            let to = index(&r.dst)?;
            let mut e = Edge::new(Endpoint::Node(NodeId(from)), Endpoint::Node(NodeId(to)));
            e.label = Some(format!("&lt;&lt;{}&gt;&gt;", r.kind.name()));
            if r.kind == RelKind::Contains {
                e.start = Head::CirclePlus;
                e.end = Head::None;
            } else {
                e.end = Head::OpenArrow;
                e.line = LineKind::Dashed;
            }
            Some(e)
        })
        .collect();
    GraphDiagram {
        kind: DiagramKind::Requirement,
        direction: d.direction,
        nodes,
        edges,
        clusters: Vec::new(),
        title: d.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "requirementDiagram

    requirement test_req {
    id: 1
    text: the test text.
    risk: high
    verifymethod: test
    }

    functionalRequirement \"quoted req\":::important {
    id: 1.1
    text: \"quoted: text\"
    risk: Low
    verifyMethod: Inspection
    }

    element test_entity {
    type: simulation
    docref: reqs/test_entity
    }

    test_entity - satisfies -> test_req
    test_req <- copies - test_entity
    test_req - contains -> \"quoted req\"
    classDef important fill:#f96
    style test_req stroke:#f00
";

    #[test]
    fn parses_requirements_elements_and_relations() {
        let d = parse(SRC, None).unwrap();
        assert_eq!(d.items.len(), 3);
        match &d.items[0].kind {
            ItemKind::Requirement {
                ty,
                id,
                risk,
                verify,
                ..
            } => {
                assert_eq!(*ty, RequirementType::Requirement);
                assert_eq!(id.as_deref(), Some("1"));
                assert_eq!(*risk, Some(Risk::High));
                assert_eq!(*verify, Some(VerifyMethod::Test));
            }
            k => panic!("unexpected {k:?}"),
        }
        assert_eq!(d.items[1].name, "quoted req");
        assert_eq!(d.items[1].classes, vec!["important"]);
        assert_eq!(d.items[1].style.fill.as_ref().unwrap().css(), "#f96");
        match &d.items[1].kind {
            ItemKind::Requirement { text, ty, .. } => {
                assert_eq!(text.as_deref(), Some("quoted: text"));
                assert_eq!(*ty, RequirementType::Functional);
            }
            k => panic!("unexpected {k:?}"),
        }
        assert!(
            matches!(&d.items[2].kind, ItemKind::Element { ty: Some(t), .. } if t == "simulation")
        );
        assert_eq!(d.relations.len(), 3);
        assert_eq!(d.relations[0].src, "test_entity");
        assert_eq!(d.relations[0].dst, "test_req");
        assert_eq!(d.relations[1].src, "test_entity");
        assert_eq!(d.relations[1].dst, "test_req");
        assert_eq!(d.relations[1].kind, RelKind::Copies);
        assert_eq!(d.relations[2].dst, "quoted req");
        assert_eq!(d.items[0].style.stroke.as_ref().unwrap().css(), "#f00");
    }

    #[test]
    fn graph_has_heads_and_labels() {
        let g = to_graph(&parse(SRC, None).unwrap());
        assert_eq!(g.edges[0].end, Head::OpenArrow);
        assert_eq!(g.edges[2].start, Head::CirclePlus);
        assert_eq!(g.nodes[0].sections[0].lines[0], "<<Requirement>>");
        assert_eq!(g.nodes[0].sections[1].lines[2], "Risk: High");
    }

    fn err_line(src: &str) -> usize {
        match parse(src, None) {
            Err(Error::Parse { line, .. }) => line,
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn reports_errors_with_lines() {
        assert_eq!(
            err_line("requirementDiagram\nrequirement a {\nrisk: huge\n}"),
            3
        );
        assert_eq!(
            err_line("requirementDiagram\nrequirement a {\ncolour: red\n}"),
            3
        );
        assert_eq!(
            err_line("requirementDiagram\n\nrequirement a {\nid: 1\n"),
            3
        );
        assert_eq!(err_line("requirementDiagram\na - likes -> b"), 2);
        assert_eq!(err_line("requirementDiagram\nelement e\n"), 2);
        assert_eq!(err_line("graph\n"), 1);
    }

    #[test]
    fn renders_scene() {
        let cfg = MermaidConfig::default();
        let pal = cfg.palette();
        let s = render(SRC, None, &cfg, &pal).unwrap();
        assert_eq!(s.kind, DiagramKind::Requirement);
    }
}
