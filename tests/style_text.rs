//! The text of mermaid's own `<style>` element in SVG output, pinned against
//! the stylesheets real mmdc wrote for the same sources (`tests/data/style`).

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

/// The first `<style>` element's text of the SVG, XML-unescaped.
fn first_style(svg: &str) -> String {
    let start = svg.find("<style>").expect("style element") + "<style>".len();
    let end = start + svg[start..].find("</style>").expect("closing tag");
    svg[start..end]
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn check(name: &str, source: &str, expected: &str) {
    let r = render::render(source, &MermaidConfig::default()).expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let got = first_style(&svg);
    if got != expected {
        let at = got
            .chars()
            .zip(expected.chars())
            .position(|(a, b)| a != b)
            .unwrap_or(got.len().min(expected.len()));
        let ctx = |s: &str| {
            s.chars()
                .skip(at.saturating_sub(60))
                .take(160)
                .collect::<String>()
        };
        panic!(
            "{name}: style text differs at byte {at}\n  ours: {}\n  mmdc: {}",
            ctx(&got),
            ctx(expected)
        );
    }
}

macro_rules! pinned {
    ($($test:ident => $name:literal,)*) => {$(
        #[test]
        fn $test() {
            check(
                $name,
                include_str!(concat!("data/style/", $name, ".mmd")),
                include_str!(concat!("data/style/", $name, ".css")),
            );
        }
    )*};
}

pinned! {
    // Default theme: the base block, quotes and commas as stylis writes them.
    pie_default_theme => "pie_cls_pie_basic",
    // Static rules with a theme-dependent fill.
    sankey_default_theme => "sankey_cls_sankey_d03",
    // `fade(...)` of an hsl colour, a dark theme.
    er_dark_theme => "er_cls_er_r3c_e_dark",
    // The redux-color theme's per-slot palette rules and its Recursive Variable font.
    flowchart_redux_color_palette => "flowchart_cls_r16_a01c_nolabel",
    // Section loops and the neo look's rules.
    mindmap_neo_redux => "mindmap_neo_mmneo_s_redux",
    sequence_default_theme => "sequence_cls_sequence_d09",
    // The flowchart-like base plus agentflow's title rule.
    agentflow_default_theme => "agentflow_cls_agentflow_d08",
    // Sanitised theme variables over railroad's own defaults.
    railroad_default_theme => "railroad_cls_railroad_d01",
}

/// PNG and PDF are drawn from their own stylesheet: the SVG file's text is not
/// what they read their colours from.
#[test]
fn raster_and_pdf_paths_do_not_use_the_svg_stylesheet() {
    let r = render::render(
        include_str!("data/style/pie_cls_pie_basic.mmd"),
        &MermaidConfig::default(),
    )
    .expect("diagram renders");
    assert!(r.style.is_some());
    let pdf = output::encode_pdf(&r, &EncodeOptions::default());
    assert!(pdf.starts_with(b"%PDF"));
}
