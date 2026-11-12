//! `htmlLabels: false`: labels laid out as mermaid 12.3's SVG `<text>` labels. The
//! sizes below are the ones mermaid 12.3.0 renders in Chromium for the same input.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// `(width, height)` of every `basic label-container` rect, in document order.
fn node_rects(svg: &str) -> Vec<(String, String)> {
    svg.split("<rect class=\"basic label-container\"")
        .skip(1)
        .map(|r| {
            let attr = |name: &str| {
                let key = format!(" {name}=\"");
                let at = r.find(&key).expect("attribute") + key.len();
                r[at..at + r[at..].find('"').unwrap()].to_string()
            };
            (attr("width"), attr("height"))
        })
        .collect()
}

fn rect(w: &str, h: &str) -> (String, String) {
    (w.to_string(), h.to_string())
}

const FLOWCHART_03: &str = "flowchart LR
    markdown[\"`This **is** _Markdown_`\"]
    newLines[\"`Line1
    Line 2
    Line 3`\"]
    markdown --> newLines
";

#[test]
fn svg_labels_use_1_1em_lines_and_the_text_box_height() {
    let s = svg(&format!(
        "---\nconfig:\n  htmlLabels: false\n---\n{FLOWCHART_03}"
    ));
    // One line: a 16px text box plus 2·12 padding; three lines add 2·1.1em.
    assert_eq!(
        node_rects(&s),
        [rect("152", "40"), rect("152", "70.8")],
        "{s}"
    );
    // First baseline 1em below the top of the text box.
    assert!(s.contains("y=\"41.4\""), "{s}");
}

#[test]
fn html_labels_true_is_the_default() {
    let explicit = svg(&format!(
        "---\nconfig:\n  htmlLabels: true\n---\n{FLOWCHART_03}"
    ));
    assert_eq!(explicit, svg(FLOWCHART_03));
    assert_ne!(node_rects(&explicit)[1], rect("152", "70.8"));
}

#[test]
fn svg_labels_wrap_and_cut_long_words() {
    let s = svg("---
config:
  htmlLabels: false
  layout: dagre
  look: classic
  theme: default
---
flowchart LR
    a[ggg]
    b[a very long label that has to wrap around in svg text mode for sure ok]
    c[Supercalifragilisticexpialidocious_and_more_characters]
");
    // 16px: a 17px text box, 18px with a descender; 1.1em = 17.6px per line.
    assert_eq!(
        node_rects(&s),
        [rect("180", "48"), rect("180", "117.4"), rect("180", "99.8")]
    );
    for line in [
        "Supercalifragilist",
        "icexpialidocious",
        "_and_more_cha",
        "racters",
    ] {
        assert!(s.contains(&format!(">{line}<")), "{line}");
    }
}

#[test]
fn flowchart_html_labels_only_changes_edge_labels_and_titles() {
    let src = "flowchart TD
    A[Christmas] -->|Get money| B(Go shopping)
    subgraph S [Sub title]
      K[Inside]
    end
";
    let html = svg(src);
    let s = svg(&format!(
        "---\nconfig:\n  flowchart:\n    htmlLabels: false\n---\n{src}"
    ));
    // labelHelper reads only the top-level key: node labels stay HTML-sized.
    assert_eq!(node_rects(&s), node_rects(&html));
    // The edge label is SVG text: its box plus 2px all round, half transparent.
    assert!(s.contains("height=\"20\" fill=\"#cccccc\" stroke=\"none\" opacity=\"0.5\""));
    assert!(!html.contains("opacity=\"0.5\""));
}

#[test]
fn class_members_stack_text_boxes_3px_apart() {
    let s = svg("---
config:
  htmlLabels: false
---
classDiagram
    Animal <|-- Duck : extends
    Animal : +int age
    Animal : +String gender
    Animal: +isMammal()
    Animal: +mate()
    class Duck{
      +String beakColor
      +swim()
      +quack()
    }
    class Shape{
      <<interface>>
      noOfVertices
      draw()
    }
");
    let heights: Vec<String> = node_rects(&s).into_iter().map(|r| r.1).collect();
    assert_eq!(heights, ["158", "139", "136"]);
}

#[test]
fn state_labels_start_at_the_left_of_their_box() {
    let s = svg("---
config:
  htmlLabels: false
---
stateDiagram-v2
    [*] --> Still
    Still --> Moving : push it
");
    assert!(s.contains("text-anchor=\"start\""), "{s}");
    let html = svg("stateDiagram-v2
    [*] --> Still
    Still --> Moving : push it
");
    // 5px shorter: a 16px text box instead of a 21px HTML line.
    let (sh, hh): (f64, f64) = (
        node_rects(&s)[0].1.parse().unwrap(),
        node_rects(&html)[0].1.parse().unwrap(),
    );
    assert_eq!(hh - sh, 5.0);
}
