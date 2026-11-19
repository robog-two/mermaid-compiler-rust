//! Quadrant charts as mmdc (mermaid 12.3.0) writes them, byte for byte: the DOM
//! `quadrantRenderer.ts` builds (presentation attributes, inline border styles,
//! points in reverse order), with only the embedded font payloads stripped.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg_without_font_data(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let mut out = String::new();
    let mut rest = svg.as_str();
    while let Some(i) = rest.find(";base64,") {
        let start = i + ";base64,".len();
        out.push_str(&rest[..start]);
        out.push_str("FONTDATA");
        let tail = &rest[start..];
        let n = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || "+/=".contains(c)))
            .unwrap_or(tail.len());
        rest = &tail[n..];
    }
    out.push_str(rest);
    out.trim_end().to_string()
}

fn pinned(name: &str) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/quadrant");
    let src = std::fs::read_to_string(format!("{dir}/{name}.mmd")).unwrap();
    let want = std::fs::read_to_string(format!("{dir}/{name}.svg")).unwrap();
    assert_eq!(svg_without_font_data(&src), want.trim_end());
}

#[test]
fn quadrant_with_points_classes_and_styles_matches_mmdc() {
    pinned("ex_quadrant");
}

#[test]
fn quadrant_in_dark_theme_matches_mmdc() {
    pinned("c_dark");
}

#[test]
fn quadrant_with_theme_variables_matches_mmdc() {
    pinned("forest_vars");
}
