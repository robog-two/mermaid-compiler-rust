//! The viewBox of the layered diagrams: `setupViewPortForSVG` starts it at the
//! drawing's bbox less the diagram padding, and the drawing starts inside the
//! layout's own margin (ELK's 12px root padding, dagre's 8px `marginx` /
//! `marginy`). The expected values are the ones mermaid 12.3.0 renders in
//! Chromium for the same input.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn view_box(src: &str) -> [f64; 4] {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let at = svg.find("viewBox=\"").expect("viewBox") + 9;
    let v: Vec<f64> = svg[at..at + svg[at..].find('"').unwrap()]
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    [v[0], v[1], v[2], v[3]]
}

#[track_caller]
fn assert_view_box(src: &str, expected: [f64; 4], tolerance: f64) {
    let got = view_box(src);
    assert!(
        got.iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() <= tolerance),
        "{got:?} != {expected:?}\n{src}"
    );
}

const DAGRE: &str = "---\nconfig:\n  layout: dagre\n---\n";
const TITLED: &str = "---\ntitle: Hello Title World\n---\n";
const TITLED_DAGRE: &str = "---\ntitle: Hello Title World\nconfig:\n  layout: dagre\n---\n";

const FLOWCHART: &str = "flowchart TD
  A[Start] --> B{Is it?}
  B -->|Yes| C[OK]
  B -->|No| D[End]
";
const CLASS: &str = "classDiagram
  Animal <|-- Duck
  Animal : +int age
  class Duck{
    +swim()
  }
";
const STATE: &str = "stateDiagram-v2
  [*] --> Still
  Still --> Moving
  Moving --> [*]
";
const REQUIREMENT: &str = "requirementDiagram
requirement test_req {
id: 1
text: the test text.
risk: high
verifymethod: test
}
element test_entity {
type: simulation
}
test_entity - satisfies -> test_req
";

#[test]
fn elk_drawings_start_inside_the_12px_root_padding() {
    assert_view_box(FLOWCHART, [4.0, 4.0, 360.0, 418.0], 1e-3);
    assert_view_box(CLASS, [4.0, 4.0, 113.5625, 290.0], 1e-2);
    assert_view_box(STATE, [4.0, 4.0, 152.0, 238.0], 1e-3);
    assert_view_box(REQUIREMENT, [4.0, 4.0, 148.859375, 386.0], 1e-2);
}

#[test]
fn dagre_drawings_start_inside_the_8px_margins() {
    // Edge labels are sized by their bbox alone (`insertEdgeLabel`), and state
    // diagrams keep `rankSpacing` 50.
    let d = |s: &str| format!("{DAGRE}{s}");
    assert_view_box(&d(FLOWCHART), [0.0, 0.0, 370.0, 398.0], 1e-3);
    assert_view_box(&d(CLASS), [0.0, 0.0, 113.5625, 300.0], 1e-2);
    assert_view_box(&d(STATE), [0.0, 0.0, 152.0, 268.0], 1e-3);
    assert_view_box(&d(REQUIREMENT), [0.0, 0.0, 148.859375, 356.0], 1e-2);
}

#[test]
fn dagre_edge_labels_take_no_padding() {
    let src = "---\nconfig:\n  layout: dagre\n  look: classic\n  theme: default\n---\n\
         flowchart LR\n  A[Start] -->|Get money| B{Is it?}\n  B -->|Yes, really long| C[OK]\n";
    assert_view_box(src, [0.0, 0.0, 836.1875, 190.0], 1e-2);
}

#[test]
fn titles_sit_on_a_baseline_at_minus_title_top_margin() {
    // `insertTitle`: baseline at y = -25; flowchart and state titles are 18px
    // (ascent 16), class and requirement titles take the 14px root font
    // (ascent 13).
    assert_view_box(
        &format!("{TITLED}{FLOWCHART}"),
        [4.0, -49.0, 360.0, 471.0],
        1e-3,
    );
    assert_view_box(
        &format!("{TITLED}{STATE}"),
        [4.0, -49.0, 152.0, 291.0],
        1e-3,
    );
    assert_view_box(
        &format!("{TITLED}{CLASS}"),
        [1.9453125, -46.0, 117.671875, 340.0],
        2e-2,
    );
    assert_view_box(
        &format!("{TITLED}{REQUIREMENT}"),
        [4.0, -46.0, 148.859375, 436.0],
        1e-2,
    );
    assert_view_box(
        &format!("{TITLED_DAGRE}{STATE}"),
        [0.0, -49.0, 152.0, 317.0],
        1e-3,
    );
}

#[test]
fn usecase_drawings_start_inside_the_layout_margin() {
    // `diagramPadding` 20 around a drawing at ELK's 12px root padding; the
    // labels' `.labelBkg` padding (2px a side) is part of the size ELK gets.
    assert_view_box(
        "usecase-beta\ndirection LR\nA(\"Alpha\")\nB(\"Beta\")\nA -- \"include\" --> B\n",
        [-8.0, -8.0, 475.703125, 98.0],
        2e-2,
    );
    assert_view_box(
        "usecase-beta\ndirection LR\nactor X\nsystemBoundary S[\"Sys\"]\n  A(\"Alpha\")\n  \
         B(\"Beta\")\nend\nX --> A\nA ..> : include B\n",
        [-8.0, -8.0, 628.8125, 177.5],
        2e-2,
    );
}

#[test]
fn dagre_clusters_take_dagre_frames() {
    let d = |s: &str| format!("{DAGRE}{s}");
    // A cluster no edge crosses is a graph of its own (`recursiveRender`): the
    // other direction, 25 more rank spacing.
    assert_view_box(
        &d(
            "flowchart TD\n  X --> S\n  subgraph S [Group title]\n    A --> B\n    B --> C\n  end\n  S --> Y\n",
        ),
        [0.0, 0.0, 697.0, 321.0],
        1e-3,
    );
    assert_view_box(
        &d(
            "stateDiagram-v2\n  [*] --> M\n  state M {\n    [*] --> a\n    a --> b\n  }\n  M --> [*]\n",
        ),
        [0.0, 0.0, 222.0, 457.0],
        1e-3,
    );
    // One an edge crosses keeps dagre's compound borders: half a node and an
    // edge gap across, half a rank along, the title inside that margin.
    assert_view_box(
        &d("flowchart TD\n  X --> A\n  subgraph S [Group title]\n    A --> B\n  end\n"),
        [0.0, 0.0, 238.0, 301.0],
        1e-3,
    );
    assert_view_box(
        &d("classDiagram\n  namespace N {\n    class A\n    class B\n  }\n  A <|-- B\n  C --> A\n"),
        [0.0, 0.0, 120.125, 409.0],
        1e-2,
    );
}

#[test]
fn git_graph_tags_follow_get_bbox() {
    // The top tag's box is 15px tall (getBBox 11 + 2 * 2), not 16.
    assert_view_box(
        include_str!("../examples/diagrams/gitgraph.mmd"),
        [-149.0625, -56.7, 807.0625, 294.1277770996094],
        1e-2,
    );
}
