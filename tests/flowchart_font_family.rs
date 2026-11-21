//! A flowchart node's `style` / `classDef` `font-family` reaches its label, and
//! its label is measured in that face (the node's box follows it). The widths
//! are the ones real mmdc 12.3 writes for the same sources.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg_of(source: &str) -> String {
    let r = render::render(source, &MermaidConfig::default()).expect("diagram renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// The widths of the node boxes, in document order.
fn node_widths(svg: &str) -> Vec<f64> {
    svg.split("class=\"basic label-container\"")
        .skip(1)
        .map(|rest| {
            let at = rest.find("width=\"").expect("width") + "width=\"".len();
            let end = at + rest[at..].find('"').expect("closing quote");
            rest[at..end].parse().expect("a number")
        })
        .collect()
}

#[test]
fn style_font_family_reaches_the_label_and_its_measure() {
    let svg = svg_of(
        "flowchart LR\n  A[WWWWWWWWWW iiiiiiiiii]\n  B[WWWWWWWWWW iiiiiiiiii]\n  \
         C[WWWWWWWWWW iiiiiiiiii]\n  D[WWWWWWWWWW iiiiiiiiii]\n  \
         style A font-family:serif\n  style C font-family:NoSuchFontXyz\n  \
         classDef c font-family:Arial\n  class D c",
    );
    assert!(svg.contains("<text class=\"nodeLabel\" style=\"font-family:serif\""));
    assert!(svg.contains("<text class=\"nodeLabel\" style=\"font-family:NoSuchFontXyz\""));
    assert!(svg.contains("<text class=\"nodeLabel\" style=\"font-family:Arial\""));
    // The default is the theme's face (Recursive). Serif, and a family no
    // installed face matches (the browser's default, serif), measure in
    // Liberation Serif; Arial in Liberation Sans.
    let w = node_widths(&svg);
    assert_eq!(w.len(), 4);
    for (got, want) in w
        .iter()
        .zip([167.390625, 169.203125, 167.390625, 168.03125])
    {
        assert!((got - want).abs() < 1e-3, "{got} != {want}");
    }
}

#[test]
fn plain_nodes_keep_the_theme_face() {
    let svg = svg_of("flowchart LR\n  A[Label] --> B[Other]");
    assert!(!svg.contains("class=\"nodeLabel\" style="));
}
