//! SVG labels (`htmlLabels: false`), statements the grammars take without a
//! keyword, and sequence label markup, pinned to mermaid 12.3.0's renders in
//! Chromium.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// The part of an SVG after its `<style>`, without the ids that vary.
fn body(s: &str) -> String {
    s[s.find("</style>").unwrap()..].to_string()
}

const SVG_LABELS: &str = "---\nconfig:\n  theme: default\n  htmlLabels: false\n---\n";

/// classBox with SVG labels: each text placed by its own bounding box
/// (mermaid: A at y 119.5, its name's baseline 108.5 at x 27, dividers at
/// 122 and 140; B at 341.5, the gap the 11px SVG terminal labels leave), the
/// terminal labels filled with the root `textColor` (#333), the note with
/// `classText`, the edge label on an opaque background, its baseline at the
/// text box's centre (230).
#[test]
fn class_boxes_with_svg_labels() {
    let s = svg(&format!(
        "{SVG_LABELS}classDiagram\n  A \"1\" --> \"*\" B : owns\n  note for A \"hi\"\n"
    ));
    assert!(
        s.contains("x=\"15\" y=\"81\" width=\"36\" height=\"77\""),
        "{s}"
    );
    assert!(s.contains("x=\"27\" y=\"108.5\" text-anchor=\"start\" font-size=\"16\" fill=\"#131300\" font-weight=\"bold\">A</text>"));
    assert!(s.contains("d=\"M15,122L51,122\""));
    assert!(s.contains("d=\"M15,140L51,140\""));
    assert!(s.contains("x=\"15.219\" y=\"303\" width=\"35.562\" height=\"77\""));
    assert!(s.contains("font-size=\"11\" fill=\"#333\">1</text>"));
    assert!(s.contains("font-size=\"11\" fill=\"#333\">*</text>"));
    assert!(s.contains("fill=\"#131300\">hi</text>"));
    assert!(s.contains(
        "<rect x=\"12\" y=\"214\" width=\"41.359\" height=\"21\" fill=\"#ECECFF\" stroke=\"none\"/>"
    ));
    assert!(s.contains(
        "y=\"230\" text-anchor=\"middle\" font-size=\"16\" fill=\"#131300\">owns</text>"
    ));
}

/// An annotation's guillemets reach past their advance (mermaid: the box
/// 83.648 wide, the viewBox 99.648); members 3px apart.
#[test]
fn class_annotation_ink_and_member_rows() {
    let s = svg(
        "---\nconfig:\n  htmlLabels: false\n---\nclassDiagram\n  class A {\n    <<iface>>\n    +int x\n    +int y\n    +foo()\n  }\n",
    );
    assert!(s.contains("viewBox=\"4 4 107.289 176\""), "{s}");
    assert!(s.contains(
        "x=\"24\" y=\"95.5\" text-anchor=\"start\" font-size=\"14\" fill=\"#28253D\"><tspan>+int</tspan><tspan> x</tspan></text>"
    ));
    assert!(s.contains(
        "x=\"24\" y=\"115.5\" text-anchor=\"start\" font-size=\"14\" fill=\"#28253D\"><tspan>+int</tspan><tspan> y</tspan></text>"
    ));
}

/// A class note's SVG label wraps at `flowchart.wrappingWidth` (120).
#[test]
fn class_svg_note_wraps_at_the_flowchart_width() {
    let s = svg(
        "---\nconfig:\n  htmlLabels: false\n---\nclassDiagram\n  A --> B\n  note \"Orders are immutable\"\n",
    );
    assert!(
        s.contains(">Orders are</tspan><tspan x=\"91.109\" y=\"62.7\">immutable</tspan>"),
        "{s}"
    );
}

/// An SVG edge label is centred by its text's box (`computeLabelTransform`):
/// mermaid's `owns` box top is at 90, so its baseline at 107.
#[test]
fn svg_edge_labels_centre_their_text_box() {
    let s = svg(
        "---\nconfig:\n  htmlLabels: false\n---\nflowchart TD\n  A[Alpha] -- owns --> B[Beta]\n",
    );
    assert!(
        s.contains(
            "y=\"108\" text-anchor=\"middle\" font-size=\"14\" fill=\"#28253D\">owns</text>"
        ),
        "{s}"
    );
}

/// classDiagram.jison has no `title`: a bare class name (its words run
/// together) is a statement that adds nothing, so `title My Title` is dropped,
/// and in a relation `title A` names the class `titleA`.
#[test]
fn class_title_lines_are_dropped() {
    let plain = svg("classDiagram\n  A --> B\n");
    for src in [
        "classDiagram\n  title My Title\n  A --> B\n",
        "classDiagram\n  A --> B\n  title Late\n",
        "classDiagram\n  Foo\n  A --> B\n",
    ] {
        assert_eq!(body(&svg(src)), body(&plain), "{src}");
    }
    let s = svg("classDiagram\n  title A --> B\n");
    assert!(s.contains("id=\"classId-titleA-0\""), "{s}");
    // A note on an undeclared class stands alone (no class, no edge).
    let s = svg("classDiagram\n  A\n  note for A \"hi\"\n");
    assert!(!s.contains("classId-A-"), "{s}");
}

/// stateDiagram and erDiagram take a line of ids as one state or entity each:
/// `title My Title` makes three (mermaid: `state-title`, `state-My`,
/// `state-Title`; `entity-title`, `entity-My`, `entity-Title`), with no title.
#[test]
fn state_and_er_title_lines_are_ids() {
    let s = svg("stateDiagram-v2\n  title My Title\n  A --> B\n");
    for id in [
        "state-title-",
        "state-My-",
        "state-Title-",
        "state-A-",
        "state-B-",
    ] {
        assert!(s.contains(id), "{id}: {s}");
    }
    let s = svg("erDiagram\n  title My Title\n  A ||--o{ B : has\n");
    for id in ["entity-title-", "entity-My-", "entity-Title-"] {
        assert!(s.contains(id), "{id}: {s}");
    }
    assert!(!s.contains("erDiagramTitleText"), "{s}");
}

/// An empty graph diagram's viewBox is the padded origin.
#[test]
fn empty_class_diagram_view_box() {
    assert!(svg("classDiagram\n").contains("viewBox=\"-8 -8 16 16\""));
    assert!(svg("flowchart TD\n").contains("viewBox=\"-8 -8 16 16\""));
}

/// byTspan's markup: central baseline with the words in a `<tspan dy="0">`,
/// which Chromium paints a pixel lower than an alphabetic baseline at the
/// shift its `getBBox()` implies.
#[test]
fn sequence_actor_labels_keep_mermaids_markup() {
    let s = svg(
        "sequenceDiagram\n    participant Alice@{ \"type\" : \"collections\" }\n    participant Bob\n    Alice->>Bob: Collections request\n    Bob->>Alice: Collections response\n",
    );
    assert!(s.contains("x=\"278\" y=\"36.5\" dominant-baseline=\"central\" alignment-baseline=\"central\" text-anchor=\"middle\" font-size=\"16\" fill=\"#28253D\"><tspan x=\"278\" dy=\"0\">Bob</tspan></text>"), "{s}");
    assert_eq!(mmdc::text::painted_tspan_central(36.5, 16.0), 43.0);
}

/// The control glyph's arrowhead is clipped to its marker's 20×28 viewport
/// (×2 for the line's width), rotated by the marker's 172.5°.
#[test]
fn sequence_control_arrowhead_is_clipped_to_its_marker() {
    let s = svg(
        "---\nconfig:\n  look: classic\n---\nsequenceDiagram\n    participant A@{ \"type\" : \"control\" }\n    participant B\n    A->>B: Query\n",
    );
    assert!(s.contains("<g transform=\"rotate(172.5 75 10)\"><svg x=\"53\" y=\"-1.6\" width=\"40\" height=\"56\" viewBox=\"53 -1.6 40 56\"><path d=\"M81.8,9.6L67.4,19.2L70.6,9.6L67.4,0Z\""), "{s}");
}
