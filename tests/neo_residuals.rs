//! Details of the neo look and the ELK layout pinned to what mermaid 12.3.0
//! draws (headless Chromium with mmdc's fonts): entity rows, shadows, the
//! markers of animated edges, a container's loops and the end of an edge
//! whose port mermaid finds inside its node.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// The group `<g id="{id}" ...>` up to its first 600 bytes.
fn group<'a>(svg: &'a str, id: &str) -> &'a str {
    let key = format!("<g id=\"{id}\"");
    let at = svg.find(&key).unwrap_or_else(|| panic!("group {id}"));
    &svg[at..(at + 600).min(svg.len())]
}

fn view_box(svg: &str) -> String {
    let at = svg.find("viewBox=\"").expect("viewBox") + 9;
    svg[at..at + svg[at..].find('"').expect("quote")].to_string()
}

/// The colour themes' `[data-color-id].node path` rule fills every path of an
/// entity, its rows' included, with the slot fill (`#FDF4FF` for color-0).
#[test]
fn entity_rows_take_the_slot_fill() {
    let s = svg("---\nconfig:\n  layout: dagre\n---\nerDiagram\n  direction RL\n  E1 {\n    \
         date created\n    string name\n    string name\n  }\n  E2 {\n    string name\n    \
         string name\n    string email UK\n  }\n  E1 }o--|| E0 : \"is part of\"\n  \
         E3 ||--|| E1 : contains\n  E2 }o--o{ E1 : \"is part of\"\n");
    for class in ["row-rect-odd", "row-rect-even"] {
        let at = s.find(&format!("class=\"{class}\"")).expect("row");
        let tag = &s[at..at + s[at..].find('>').expect("tag end")];
        assert!(tag.contains("fill=\"#FDF4FF\""), "{tag}");
    }
}

/// A region's `rect.divider` is in no `.cluster`: the neo look casts no
/// shadow from it (mermaid draws none; the composite's `rect.outer` keeps one).
#[test]
fn regions_cast_no_shadow() {
    let s = svg("stateDiagram-v2\n  direction LR\n  [*] --> A\n  state A {\n    a1 --> a2\n    \
         --\n    b1 --> b2\n  }\n  A --> [*]\n");
    let g = group(&s, "cluster-divider-id-1");
    let first = &g[..g.find("</g>").expect("end")];
    assert!(!first.contains("drop-shadow"), "{first}");
    assert!(group(&s, "cluster-A").contains("drop-shadow"));
    // stateEnd's rough.js circle measures 14.0133 wide: the end state sits
    // at x 345.0066 and the view box is 356.01995849609375 wide.
    assert_eq!(view_box(&s), "4 4 356.02 404");
}

/// A subgraph nested in a lane is a `.cluster rect` and casts the shadow;
/// only the lanes reset it (`.swimlane.cluster rect { filter: none }`).
#[test]
fn groups_in_lanes_cast_the_shadow() {
    let s = svg(r#"swimlane-beta TB
  subgraph L0[Lane 0]
    n00((Ship))
    n01[(Review deploy)]
    subgraph S0[Group 0]
      s00[Approve]
      s01[Submit]
    end
  end
  subgraph L1[Lane 1]
    n10[/Notify/]
    n11[Test]
  end
  subgraph L2[Lane 2]
    n20([Notify])
    n21{{Pay}}
  end
  subgraph L3
    n30>Archive]
    n31[(Check ship)]
    n32((Plan))
  end
  subgraph L4[Lane 4]
    n40[[Plan]]
    n41[Approve]
  end
  subgraph L5[Lane 5]
    n50[Deploy]
    subgraph S5[Group 5]
      s50[Ship]
    end
  end
  n32 --> n20
  s01 --> n01
  n31 --o n40
  n32 --> n11
  s01 --o n32
  n11 --x n10
  n40 ---|Archive 1| n21
  n32 --> n00
  n11 --- n10
  s01 --x n21
  s50 --> n30
  n00 ---|Test 2| n40
  n40 -.-> n01
  n11 --o|Pay 3| n01
  s01 ==>|Archive 4| n01
  n00 -.-> n50
  n11 -.-> n21
  classDef hot fill:#f96,stroke:#333,stroke-width:2px
  class s00 hot
"#);
    assert!(
        group(&s, "S0").starts_with("<g id=\"S0\" class=\"cluster\"><rect class=\"drop-shadow\""),
        "{}",
        group(&s, "S0")
    );
}

/// Under ELK an empty subgraph is a point: the cluster shape draws it as its
/// title's width plus the padding, 0 high (`82.625` for "Inner title").
#[test]
fn an_empty_subgraph_under_elk_is_a_flat_frame() {
    let s = svg("flowchart LR\nsubgraph outer\nsubgraph inner[Inner title]\nend\nX\nend\n\
         A --> inner\nclassDef hot fill:#f96\nclass inner hot\n");
    let g = group(&s, "cluster-inner");
    assert!(
        g.contains("<rect x=\"196.688\" y=\"165.8\" width=\"82.625\" height=\"0\" fill=\"#f96\""),
        "{g}"
    );
    assert!(g.contains("class=\"drop-shadow\""), "{g}");
}

/// An animated edge leaves the neo look's `*-margin` markers out
/// (`useMargin = !animatedEdge && look === "neo"`): its arrow is the 8×8
/// `pointEnd`, centred on the line's end.
#[test]
fn animated_edges_take_the_plain_markers() {
    let s = svg(r#"---
config:
  layout: dagre
---
flowchart LR
    subgraph ingest [Ingest]
        src[Sources] --> parse[Parse] --> validate[Validate]
    end
    subgraph store [Storage]
        direction TB
        raw[(Raw)] --> clean[(Clean)]
    end
    subgraph observability [Observability]
        metrics[Metrics] --> alerts[Alerts]
    end
    user((User)) e1@--> src
    validate --> raw
    validate --> observability
    clean --> store
    observability@{ view: collapsed }
    e1@{ animate: true }
"#);
    let g = group(&s, "L-user-src-3");
    assert!(
        g.contains("<path class=\"marker\" d=\"M144.858,125.287L152.858,129.287L144.858,133.287Z\""),
        "{g}"
    );
}

/// A container's loops count in its margins: ELK moves the root's content
/// to start at its padding, the outermost leg included (view box at 4, not
/// -16).
#[test]
fn container_loops_count_in_the_root_offset() {
    let s = svg("flowchart BT\n  subgraph C0\n    c0n0\n    c0n1\n    c0n0 --> c0n1\n  end\n  \
         x0\n  C0 --> c0n0\n  C0 --> C0\n  C0 -->|lab1| C0\n  C0 --> C0\n");
    assert_eq!(view_box(&s), "4 4 497.969 282");
}
