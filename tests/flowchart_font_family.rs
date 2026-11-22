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

/// Node `width` and `height` pairs, in document order.
fn node_sizes(svg: &str) -> Vec<(f64, f64)> {
    let attr = |rest: &str, name: &str| -> f64 {
        let key = format!("{name}=\"");
        let at = rest.find(&key).expect(name) + key.len();
        let end = at + rest[at..].find('"').expect("closing quote");
        rest[at..end].parse().expect("a number")
    };
    svg.split("class=\"basic label-container\"")
        .skip(1)
        .map(|rest| (attr(rest, "width"), attr(rest, "height")))
        .collect()
}

#[test]
fn monospace_families_measure_in_dejavu_and_liberation_mono() {
    let w15 = "WWWWWWWWWWWWWWW";
    let src = format!(
        "flowchart LR\n  A[{w15}]\n  B[{w15}]\n  C[{w15}]\n  D[**{w15}**]\n  E[*{w15}*]\n  \
         F[{w15}]\n  G[ünïcødé→✓ünïcødé]\n  H[{w15}]\n  \
         style A font-family:monospace\n  style B font-family:Courier New\n  \
         style C font-family:courier\n  style D font-family:monospace\n  \
         style E font-family:monospace\n  style F font-family:Liberation Mono\n  \
         style G font-family:monospace\n  style H font-family:DejaVu Sans Mono"
    );
    // Real mmdc 12.2.0: the label's width (every character 1233/2048 em in
    // DejaVu Sans Mono, 1229/2048 in Liberation Mono at 14px) plus 32.
    let want = [
        158.4375, 158.03125, 158.03125, 192.15625, 175.296875, 158.03125, 166.859375, 158.4375,
    ];
    let got = node_sizes(&svg_of(&src));
    assert_eq!(got.len(), want.len());
    for ((w, h), want) in got.iter().zip(want) {
        // (The library's SVG rounds to three decimals.)
        assert!(
            (w - want).abs() < 1e-3 && *h == 45.0,
            "{w}x{h} != {want}x45"
        );
    }
}

#[test]
fn monospace_label_wraps_at_the_minimum_box() {
    let l = "WWWWWWWWWW iiiiiiiiii";
    let src = format!(
        "flowchart LR\n  A[{l}]\n  B[{l}]\n  C[{l}]\n  F[{l}]\n  \
         style A font-family:monospace\n  style B font-family:Courier New\n  \
         style C font-family:DejaVu Sans Mono\n  style F font-family:Liberation Mono"
    );
    for size in node_sizes(&svg_of(&src)) {
        assert_eq!(size, (152.0, 66.0));
    }
}

#[cfg(all(feature = "png", feature = "fonts"))]
#[test]
fn monospace_labels_render_to_png_and_pdf() {
    let r = render::render(
        "flowchart LR\n  A[monospace → ✓] --> B[courier]\n  \
         style A font-family:monospace\n  style B font-family:Courier New",
        &MermaidConfig::default(),
    )
    .expect("diagram renders");
    let opt = EncodeOptions::default();
    let png = output::encode_png(&r, &opt, &output::png::Rasterizer::bundled_only());
    assert!(png.is_ok_and(|p| p.starts_with(b"\x89PNG")));
    assert!(output::encode_pdf(&r, &opt).starts_with(b"%PDF"));
}
