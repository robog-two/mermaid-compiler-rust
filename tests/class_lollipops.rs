//! Class-diagram lollipop interfaces pinned to mermaid 12.3.0's renders in Chromium.
//!
//! classDb's `addRelation` turns a lollipop end (`A ()-- B`, `B --() A`) into an
//! `interfaceN` node: a `rect` with `opacity: 0` labelled with the name written
//! there, which is not added as a class.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// Neo look, HTML labels: squareRect pads the label by 16 / 12 (mermaid: a
/// 74.796875 × 45 rect for `Animal`), and the lollipop is a transparent r 6
/// circle 13.5 past the rect.
#[test]
fn neo_interface_is_an_invisible_padded_rect() {
    let s =
        svg("classDiagram\n  class Animal {\n    +int age\n    +eat()\n  }\n  Animal ()-- Dog\n");
    assert!(s.contains("viewBox=\"4 4 228.359 272\""), "{s}");
    assert!(s.contains(
        "<g id=\"interface0\" class=\"node default\"><rect class=\"basic label-container\" \
         x=\"149.562\" y=\"102\" width=\"74.797\" height=\"45\""
    ));
    assert!(s.contains("opacity=\"0\"/><text class=\"nodeLabel\" x=\"186.961\""));
    assert!(s.contains(
        "<circle class=\"marker\" cx=\"186.961\" cy=\"154.5\" r=\"6\" fill=\"none\" \
         stroke=\"#000000\" stroke-width=\"2\"/>"
    ));
    // `Animal` is a class box and the interface's label: no second class.
    assert_eq!(s.matches("id=\"classId-Animal-").count(), 1);
    assert_eq!(s.matches("id=\"classId-").count(), 2);
}

/// Classic look: the interface sets no `padding` (a 42.796875 × 21 rect), and
/// the circle's own `fill="transparent"` outranks the stylesheet's `mainBkg`.
#[test]
fn classic_interface_has_no_padding_and_a_hollow_marker() {
    let s = svg(
        "---\nconfig:\n  look: classic\n---\nclassDiagram\n  class Animal {\n    +int age\n    \
         +eat()\n  }\n  Animal ()-- Dog\n  Dog --() Pet\n",
    );
    assert!(s.contains("viewBox=\"4 4 200.57 333\""), "{s}");
    assert!(s.contains("x=\"149.562\" y=\"126\" width=\"42.797\" height=\"21\""));
    assert!(
        s.contains("<circle class=\"marker\" cx=\"170.961\" cy=\"154.5\" r=\"6\" fill=\"none\"")
    );
    assert!(
        s.contains("<circle class=\"marker\" cx=\"170.961\" cy=\"300.5\" r=\"6\" fill=\"none\"")
    );
    // The classic stylesheet's `.node path { stroke-width: 2 }` reaches the dividers.
    assert!(s.contains(
        "<path class=\"divider\" d=\"M12,57L109.562,57\" fill=\"none\" stroke=\"#E879F9\" \
         stroke-width=\"2\"/>"
    ));
}

/// SVG labels start at the rect's centre (no `text-anchor`), and the node's DOM
/// box, which reaches the text's end and its glyph box 1px below the rect, is
/// what the layout and the edge clipping take (mermaid: the `Animal` edge leaves
/// half of 17 below the node; the viewBox reaches the text's end).
#[test]
fn svg_interface_label_overhangs_into_the_layout() {
    let s = svg(
        "---\nconfig:\n  look: classic\n  htmlLabels: false\n---\nclassDiagram\n  Animal ()-- Dog\n  \
         Cat --() Pet\n  Dog --() LongInterfaceName\n",
    );
    assert!(s.contains("viewBox=\"4 4 250.182 280.9\""), "{s}");
    assert!(s.contains("x=\"109.596\" y=\"71.5\" width=\"42.797\" height=\"16\""));
    assert!(s.contains("x=\"130.994\" y=\"85.5\" text-anchor=\"start\""));
    assert!(s.contains("<circle class=\"marker\" cx=\"130.994\" cy=\"95.5\" r=\"6\""));
    // Wrapped at `flowchart.wrappingWidth` (120) inside words.
    assert!(s.contains(">LongInterfaceNam</tspan>"));
}

/// Left to right, the edge is clipped at the DOM box's side (mermaid: the
/// lollipop of `IRunnable` at x 145.07).
#[test]
fn svg_interface_clips_edges_at_its_dom_box() {
    let s = svg(
        "---\nconfig:\n  look: neo\n  htmlLabels: false\n  theme: forest\n---\nclassDiagram\n  \
         direction LR\n  IRunnable ()-- Worker\n  Worker --() IStoppable\n",
    );
    assert!(s.contains("viewBox=\"14.262 4 432.219 93\""), "{s}");
    assert!(s.contains("<circle class=\"marker\" cx=\"145.07\" cy=\"50.5\" r=\"6\""));
    assert!(s.contains("<circle class=\"marker\" cx=\"288.914\" cy=\"50.5\" r=\"6\""));
}

/// An HTML interface label wraps at `flowchart.wrappingWidth` (`max-width: 120px`).
#[test]
fn html_interface_label_wraps_at_120() {
    let s = svg(
        "---\nconfig:\n  look: classic\n---\nclassDiagram\n  direction LR\n  \
         VeryLongInterfaceNameForTestingWrap ()-- Impl\n  \
         Impl --() `Some Long Interface Name With Spaces`\n",
    );
    assert!(s.contains("viewBox=\"4 4 518.328 97\""), "{s}");
    assert!(s.contains("x=\"394.328\" y=\"21\" width=\"120\""));
}

/// A lollipop whose other end has a head is an ordinary relation between two classes.
#[test]
fn lollipop_opposite_a_head_joins_two_classes() {
    let s = svg("classDiagram\n  A ()--() B\n  A ()--|> C\n");
    assert!(!s.contains("interface0"));
    for c in ["A", "B", "C"] {
        assert!(s.contains(&format!("id=\"classId-{c}-")), "{c}");
    }
}
