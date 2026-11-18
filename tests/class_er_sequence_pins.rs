//! Class, ER and sequence details pinned to mermaid 12.3.0's renders in Chromium.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// classDb's `getData` hands ELK the namespaces first: a namespace with no
/// relations lies left of the classes declared before it (mermaid: the `Zoo`
/// frame at x 12, 106.234375 wide; `Animal` right of it).
#[test]
fn class_namespaces_come_first_under_elk() {
    let s = svg(
        "classDiagram\n  class Animal\n  class Dog\n  Animal <|-- Dog\n  namespace Zoo {\n    class Cage\n  }\n",
    );
    assert!(s.contains("<rect x=\"12\" y=\"12\" width=\"106.234\" height=\"153\""));
    assert!(s.contains("x=\"158.234\" y=\"84\" width=\"70.688\" height=\"81\""));
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
