//! Graphs mermaid 12.3 cannot draw. Under `layout: dagre`, an edge from an
//! empty subgraph to itself makes mermaid throw "Cannot read properties of
//! undefined (reading 'node')". Under ELK, an edge between a subgraph and its
//! parent inside a subgraph with its own `direction` makes elkjs throw
//! `UnsupportedGraphException` (the two ends land in separate layout runs).
//! We draw them on purpose (see docs/DIAGRAM_GUIDE.md, "Deliberate
//! differences from mermaid"), so these tests pin that they keep rendering:
//! every subgraph is drawn, and every coordinate is finite.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    // Without the fonts: their base64 data may spell "NaN" or "inf".
    let enc = EncodeOptions {
        font_embed: false,
        ..EncodeOptions::default()
    };
    output::encode_svg(&r, &enc)
}

fn check(src: &str, clusters: &[&str]) {
    let out = svg(src);
    assert!(
        !out.contains("NaN") && !out.contains("inf"),
        "non-finite coordinate"
    );
    // Under dagre an empty subgraph is drawn as a node, as mermaid does.
    for id in clusters {
        assert!(
            out.contains(&format!("<g id=\"cluster-{id}\" class=\"cluster\">"))
                || out.contains(&format!("<g id=\"flowchart-{id}-")),
            "subgraph {id} missing"
        );
    }
}

#[test]
fn dagre_self_loop_on_top_level_empty_subgraph() {
    check(
        "---\nconfig:\n  layout: dagre\n---\nflowchart RL\n  subgraph G0[Group G0]\n  end\n  x0\n  G0 --> G0\n  x0 ==> G0\n",
        &["G0"],
    );
}

#[test]
fn dagre_two_way_self_loop_on_empty_subgraph() {
    check(
        "---\nconfig:\n  layout: dagre\n---\nflowchart BT\n  subgraph G0[Group G0]\n  end\n  subgraph G1[Group G1]\n    direction TB\n  end\n  x0(\"Start\")\n  G1 <--> G1\n  x0 -->|lbl| x0\n",
        &["G0", "G1"],
    );
}

#[test]
fn dagre_self_loop_on_nested_empty_subgraph() {
    check(
        "---\nconfig:\n  layout: dagre\n---\nflowchart LR\n  subgraph G0\n    g0n0(\"Save\")\n    g0n1{\"Save\"}\n    subgraph G1\n    end\n    subgraph G2\n      direction TB\n      g2n0\n      g2n1\n      subgraph G3\n        direction RL\n        subgraph G4[Group G4]\n        end\n      end\n      g2n0 --> g2n1\n    end\n  end\n  x0\n  G4 ---> G4\n  g0n0 --> g0n1\n  g2n0 --- g0n0\n",
        &["G0", "G1", "G2", "G3", "G4"],
    );
}

#[test]
fn elk_edge_to_parent_inside_a_directed_subgraph() {
    check(
        "flowchart TD\n  subgraph G0\n  direction LR\n  subgraph G2\n    subgraph G3\n    end\n  end\n  end\n  G3 --> G2\n",
        &["G0", "G2", "G3"],
    );
}
