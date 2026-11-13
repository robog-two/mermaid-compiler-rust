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

/// An example's source with `layout: dagre` added to its front matter.
fn under_dagre(src: &str) -> String {
    match src.strip_prefix("---\n") {
        Some(rest) => format!("---\nconfig:\n  layout: dagre\n{rest}"),
        None => format!("{DAGRE}{src}"),
    }
}

#[test]
fn gallery_examples_under_dagre_match_mermaid() {
    // mermaid 12.3.0 in Chromium, the examples with `layout: dagre`.
    let cases: [(&str, [f64; 4]); 6] = [
        (
            include_str!("../examples/diagrams/subgraphs.mmd"),
            [0.0, 0.0, 1674.26220703125, 304.75],
        ),
        (
            include_str!("../examples/diagrams/flowchart.mmd"),
            [0.0, -49.0, 533.64453125, 825.66015625],
        ),
        (
            include_str!("../examples/diagrams/shapes.mmd"),
            [0.0, -49.0, 1344.2384033203125, 1889.31005859375],
        ),
        (
            include_str!("../examples/diagrams/agentflow.mmd"),
            [0.0, 0.0, 483.1999816894531, 881.9920043945312],
        ),
        (
            include_str!("../examples/diagrams/usecase.mmd"),
            [-12.0, -12.0, 853.265625, 648.75],
        ),
        (
            include_str!("../examples/diagrams/subgraph-collapse.mmd"),
            [0.0, 0.0, 1178.26220703125, 287.398681640625],
        ),
    ];
    for (src, expected) in cases {
        assert_view_box(&under_dagre(src), expected, 2e-2);
    }
}

/// The centres of the edge labels' background boxes, in drawing order.
fn edge_label_centres(src: &str) -> Vec<(f64, f64)> {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let attr = |s: &str, k: &str| -> f64 {
        let at = s.find(&format!(" {k}=\"")).expect("attribute") + k.len() + 3;
        s[at..at + s[at..].find('"').unwrap()].parse().unwrap()
    };
    svg.split("<g class=\"edgeLabel\"><rect")
        .skip(1)
        .map(|s| {
            let s = &s[..s.find('>').unwrap()];
            (
                attr(s, "x") + attr(s, "width") / 2.0,
                attr(s, "y") + attr(s, "height") / 2.0,
            )
        })
        .collect()
}

#[track_caller]
fn assert_label_at(src: &str, expected: (f64, f64), tolerance: f64) {
    let got = edge_label_centres(src);
    assert!(
        got.iter().any(|p| (p.0 - expected.0).abs() <= tolerance
            && (p.1 - expected.1).abs() <= tolerance),
        "{got:?} has no label at {expected:?}"
    );
}

#[test]
fn dagre_labels_move_with_their_cut_routes() {
    let d = |s: &str| format!("{DAGRE}{s}");
    // `positionEdgeLabel`: the route's middle point is not in the drawn path,
    // so the label moves as far as clipping moved the route's midpoint.
    let flat = d(
        "flowchart LR\n  n0[[\"Decision?\"]]\n  n1>\"Process data\"]\n  n2[\"End\"]\n  \
         n3(\"ok\")\n  n4([\"A much longer node text here\"])\n  n6 -.-> n2\n  n2 ==>|yes| n4\n  \
         n7 -.->|yes| n2\n  n2 ==> n8\n  n5 --> n2\n  n5 ==> n0\n  n7 ==> n8\n  n5 ---|no| n6\n  \
         n1 ==> n4\n  n6 --> n3\n  n7 ==> n0\n  n5 --> n8\n",
    );
    // (The text widths differ from Chromium's by a few hundredths.)
    assert_label_at(&flat, (660.7725875, 19.54185), 0.1);
    assert_view_box(
        &flat,
        [0.0, 1.041849136352539, 878.887939453125, 418.2081604003906],
        5e-2,
    );
    // From a cluster: the route leaves the frame where the line from its
    // centre to the first bend does, and a route cut at a frame always counts
    // as moved.
    assert_label_at(
        &d(
            "flowchart TD\n  subgraph C0\n    c0n0\n  end\n  x0\n  x1\n  C0 -->|x| x1\n  \
            c0n0 --> x1\n  C0 --> x0\n  C0 --> x1\n",
        ),
        (107.5, 123.31887),
        1e-3,
    );
    // Between a cluster and one inside it: routed (not a loop), and cut down
    // to a single point.
    let nested = d(
        "flowchart LR\n  subgraph C0\n    c0n0\n    c0n1\n    c0n0 --> c0n1\n  end\n  \
         subgraph C1\n    c1n0\n    subgraph C2\n      c2n0\n    end\n  end\n  subgraph C3\n    \
         c3n0\n    c3n1\n    c3n2\n    c3n0 --> c3n1\n    c3n1 --> c3n2\n  end\n  x0\n  \
         C1 -->|a label| C2\n  c3n1 --> c1n0\n  C2 -->|x| C3\n",
    );
    assert_label_at(&nested, (271.12814249999997, 88.08064516129032), 1e-2);
    assert_label_at(&nested, (275.69647, 117.75), 1e-2);
    assert_view_box(&nested, [0.0, 0.0, 967.265625, 576.0], 1e-2);
}

#[test]
fn dagre_routes_leave_cylinders_as_mermaid_clips_them() {
    // `cylinder.ts`'s intersect moves the box's crossing by the cap, even on a
    // side: the edge from `n3` starts 13.38 below where the box would put it.
    let r = render::render(
        &format!(
            "{DAGRE}flowchart LR\n  n1{{{{\"Decision?\"}}}}\n  n2{{{{\"A much longer node text here\"}}}}\n  \
             n3[(\"Load\")]\n  n1 --> n3\n  n3 --- n0\n  n3 -.-> n1\n  n0 --> n0\n  n2 --> n0\n  \
             n3 ==> n0\n  n0 --> n3\n"
        ),
        &MermaidConfig::default(),
    )
    .expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    assert!(svg.contains("d=\"M442,191.155L452.167,184.011"), "{svg}");
}
