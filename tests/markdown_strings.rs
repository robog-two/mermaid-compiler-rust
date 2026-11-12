//! Upstream docs examples with markdown strings, backtick class names and
//! multi-direction block arrows (flowchart_03, flowchart_98, mindmap_11,
//! classDiagram_06 and block_17 of mermaid's docs), rendered end to end.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[test]
fn flowchart_03_markdown_bold_italic_and_new_lines() {
    let s = svg("---
config:
  htmlLabels: false
---
flowchart LR
    markdown[\"`This **is** _Markdown_`\"]
    newLines[\"`Line1
    Line 2
    Line 3`\"]
    markdown --> newLines
");
    assert!(s.contains("This <tspan font-weight=\"bold\">is</tspan> "));
    assert!(s.contains("<tspan font-style=\"italic\">Markdown</tspan>"));
    for l in ["Line1", "Line 2", "Line 3"] {
        assert!(s.contains(&format!(">{l}</tspan>")), "{l}");
    }
    assert!(!s.contains('`') && !s.contains("**"));
}

#[test]
fn flowchart_98_markdown_in_nodes_edge_labels_and_subgraph_titles() {
    let s = svg("---
config:
  htmlLabels: false
---
flowchart LR
subgraph \"One\"
  a(\"`The **cat**
  in the hat`\") -- \"edge label\" --> b{{\"`The **dog** in the hog`\"}}
end
subgraph \"`**Two**`\"
  c(\"`The **cat**
  in the hat`\") -- \"`Bold **edge label**`\" --> d(\"The dog in the hog\")
end
");
    // Two nodes with a bold `cat`, one bold `dog`, the bold edge label and title.
    assert_eq!(count(&s, "<tspan font-weight=\"bold\">cat</tspan>"), 2);
    assert_eq!(count(&s, "<tspan font-weight=\"bold\">dog</tspan>"), 1);
    assert!(s.contains("Bold <tspan font-weight=\"bold\">edge label</tspan>"));
    assert!(s.contains("<tspan font-weight=\"bold\">Two</tspan>"));
    assert_eq!(count(&s, ">in the hat</tspan>"), 2);
    assert!(!s.contains(">**") && !s.contains("**<"));
}

#[test]
fn mindmap_11_multi_line_markdown_wraps_at_the_wrapping_width() {
    let s = svg("mindmap
    id1[\"`**Root** with
a second line
Unicode works too: 🤓`\"]
      id2[\"`The dog in **the** hog... a *very long text* that wraps to a new line`\"]
      id3[Regular labels still works]
");
    assert!(s.contains("<tspan font-weight=\"bold\">Root</tspan> with"));
    assert!(s.contains(">a second line</tspan>"));
    // Rect nodes wrap at `flowchart.wrappingWidth` (120) as mermaid 12.3 does.
    assert!(s.contains(">Unicode works</tspan>"));
    assert!(s.contains(">Regular labels</tspan>"));
    assert!(s.contains("<tspan font-style=\"italic\">very</tspan>"));
    assert!(!s.contains("**") && !s.contains('`'));
}

#[test]
fn class_diagram_06_backtick_class_names() {
    let s = svg("classDiagram
    class `Animal Class!`
    class `Car Class`
    `Animal Class!` --> `Car Class`
");
    assert!(s.contains(">Animal Class!<"));
    assert!(s.contains(">Car Class<"));
    assert!(!s.contains('`'));
}

#[test]
fn block_17_block_arrows_with_direction_lists() {
    let s = svg("block
  blockArrowId<[\"Label\"]>(right)
  blockArrowId2<[\"Label\"]>(left)
  blockArrowId3<[\"Label\"]>(up)
  blockArrowId4<[\"Label\"]>(down)
  blockArrowId5<[\"Label\"]>(x)
  blockArrowId6<[\"Label\"]>(y)
  blockArrowId7<[\"Label\"]>(x, down)
");
    assert_eq!(count(&s, ">Label<"), 7);
}

#[test]
fn pdf_draws_emphasis_with_the_bold_and_oblique_helveticas() {
    let r = render::render(
        "flowchart LR\n  a[\"`**b** *i* ***bi***`\"]",
        &MermaidConfig::default(),
    )
    .unwrap();
    let pdf = output::encode_pdf(&r, &EncodeOptions::default());
    let pdf = String::from_utf8_lossy(&pdf);
    for f in [
        "/F2 1 Tf (b)",
        "/F3 1 Tf (i)",
        "/F4 1 Tf (bi)",
        "/Helvetica-BoldOblique",
    ] {
        assert!(pdf.contains(f), "{f}");
    }
}

#[test]
fn markdown_auto_wrap_false_keeps_markdown_labels_on_one_line() {
    let src = |wrap: bool| {
        format!(
            "---\nconfig:\n  markdownAutoWrap: {wrap}\n---\nflowchart LR\n  \
             a[\"`a **long** markdown label that would wrap at the width`\"] -- \
             \"`a long markdown edge label that would wrap otherwise ok`\" --> \
             b[\"plain label that is long enough to wrap\"]"
        )
    };
    let off = svg(&src(false));
    assert!(off.contains(
        ">a <tspan font-weight=\"bold\">long</tspan> markdown label that would wrap at the width<"
    ));
    assert!(off.contains(">a long markdown edge label that would wrap otherwise ok<"));
    // Plain labels still wrap.
    assert!(!off.contains(">plain label that is long enough to wrap<"));
    let on = svg(&src(true));
    assert!(!on.contains("markdown label that would wrap at the width<"));
}
