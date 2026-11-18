//! Class, ER and sequence details pinned to mermaid 12.3.0's renders in Chromium.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// ClassMember's classifiers (mermaid 12.3.0 with mmdc's fonts, pixel-compared in Chromium): a
/// static (`$`) member is underlined, an abstract (`*`) one italic where its
/// label is HTML. SVG text paints the underline in the label group's fill
/// (the theme's textColor), `max(1, size / 10)` thick from one pixel under the
/// baseline; HTML in the group's `color`, black, its thickness floored.
#[test]
fn static_members_are_underlined_and_abstract_ones_italic() {
    let body = "classDiagram\n  class Queue~int~ {\n    +List~int~ items$\n    +push(int x)$\n    +pop()* int\n    +count$ int\n  }\n";
    let svg_labels = svg(&format!("---\nconfig:\n  htmlLabels: false\n---\n{body}"));
    assert!(svg_labels.contains(
        "<rect class=\"member-underline\" x=\"24\" y=\"79.5\" width=\"116.06\" height=\"1.4\" fill=\"#28253D\""
    ));
    assert!(svg_labels.contains("y=\"140.5\" width=\"84.7\" height=\"1.4\""));
    // `count$ int` is an attribute ending in `t`: no classifier.
    assert_eq!(svg_labels.matches("member-underline").count(), 2);
    // An SVG label's own `font-style` keeps the abstract member upright.
    assert!(!svg_labels.contains("italic"));
    let html = svg(body);
    assert!(html.contains(
        "<rect class=\"member-underline\" x=\"24\" y=\"86.5\" width=\"116.062\" height=\"1\" fill=\"#000000\""
    ));
    assert!(html.contains("font-style=\"italic\">+pop() : int<"));
    let big = svg(
        "---\nconfig:\n  theme: forest\n  themeVariables:\n    fontSize: 20px\n---\nclassDiagram\n  class Shape {\n    +area()$ double\n    -id$\n    +draw()*\n  }\n",
    );
    assert!(big.contains("x=\"24\" y=\"98\" width=\"22.234\" height=\"2\" fill=\"#000000\""));
}

/// classDb's `getData` hands ELK the namespaces first: a namespace with no
/// relations lies left of the classes declared before it (mermaid: the `Zoo`
/// frame at x 12, 106.234375 wide; `Animal` right of it).
#[test]
fn class_namespaces_come_first_under_elk() {
    let s = svg(
        "classDiagram\n  class Animal\n  class Dog\n  Animal <|-- Dog\n  namespace Zoo {\n    class Cage\n  }\n",
    );
    assert!(s.contains("<rect x=\"12\" y=\"12\" width=\"105.047\" height=\"153\""));
    assert!(s.contains("x=\"157.047\" y=\"84\" width=\"71.75\" height=\"81\""));
}

/// Class labels take `classText`: the default theme's `primaryTextColor`,
/// `invert(#ECECFF)`.
#[test]
fn class_labels_take_class_text() {
    let s = svg("---\nconfig:\n  theme: default\n---\nclassDiagram\n  class Animal\n");
    assert!(s.contains("fill=\"#131300\" font-weight=\"bold\">Animal</text>"));
}

/// The base theme's cluster title, `invert(adjust('#fff4dd', { h: 180, l: 5 }))`,
/// as khroma prints it (Chromium paints rgb(9, 6, 0)), and the outline gradient
/// `primaryBorderColor` → `secondaryBorderColor` exactly as mermaid writes it.
#[test]
fn base_theme_titles_keep_khromas_channels() {
    let s = svg(
        "---\nconfig:\n  theme: base\n---\nclassDiagram\n  namespace Zoo {\n    class Cage\n  }\n",
    );
    assert!(s.contains("fill=\"rgb(8.5000000002, 5.7500000001, 0)\">Zoo</text>"));
    assert!(s.contains("stop-color=\"hsl(40.5882352941, 60%, 83.3333333333%)\""));
    assert!(s.contains("stop-color=\"hsl(-79.4117647059, 60%, 83.3333333333%)\""));
}

/// erDiagram's `.edgeLabel .label { font-size: 14px }` (a 21px label box) on
/// `fade(tertiaryColor, 0.5)`.
#[test]
fn er_relationship_labels_are_14px_on_faded_tertiary() {
    let s =
        svg("---\nconfig:\n  theme: default\n---\nerDiagram\n  CUSTOMER ||--o{ ORDER : places\n");
    assert!(
        s.contains("width=\"40.469\" height=\"21\" fill=\"hsla(80, 100%, 96.2745098039%, 0.5)\"")
    );
}

/// An HTML cell is as wide as its text in layout units (mermaid: 261.5625).
#[test]
fn er_cells_are_measured_in_layout_units() {
    let s = svg(
        "---\nconfig:\n  theme: neo\n---\nerDiagram\n  ORDER {\n    int id PK \"a comment here\"\n  }\n",
    );
    assert!(s.contains("viewBox=\"4 4 261.562 95.5\""), "{s}");
}

/// A typed participant's group shadow is one layer at 6%: overlapping parts
/// (the boundary's bar, arm and circle) do not darken each other.
#[test]
fn sequence_glyph_shadows_are_one_layer() {
    let s = svg(
        "sequenceDiagram\n  participant Alice@{ \"type\" : \"boundary\" }\n  Alice->>Alice: x\n",
    );
    assert!(s.contains("<g class=\"drop-shadow\" opacity=\"0.06\">"));
}
