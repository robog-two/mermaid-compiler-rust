//! Journey diagrams as mmdc (mermaid 12.3.0) writes them, byte for byte, with
//! only the embedded font payloads stripped.

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
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/journey");
    let src = std::fs::read_to_string(format!("{dir}/{name}.mmd")).unwrap();
    let want = std::fs::read_to_string(format!("{dir}/{name}.svg")).unwrap();
    assert_eq!(svg_without_font_data(&src), want.trim_end());
}

#[test]
fn journey_default_look_matches_mmdc() {
    pinned("journey_cls_journey_d00");
}

#[test]
fn journey_with_lunch_section_and_actors_matches_mmdc() {
    pinned("journey_cls_journey_ex_journey");
}

#[test]
fn journey_neo_look_matches_mmdc() {
    pinned("journey_neoinj_journey_d00");
}

#[test]
fn journey_neo_look_with_lunch_section_matches_mmdc() {
    pinned("journey_neoinj_journey_ex_journey");
}
