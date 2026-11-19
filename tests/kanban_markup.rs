//! Kanban boards as mmdc (mermaid 12.3.0) writes them, byte for byte, with
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
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/kanban");
    let src = std::fs::read_to_string(format!("{dir}/{name}.mmd")).unwrap();
    let want = std::fs::read_to_string(format!("{dir}/{name}.svg")).unwrap();
    assert_eq!(svg_without_font_data(&src), want.trim_end());
}

#[test]
fn kanban_default_look_matches_mmdc() {
    pinned("kanban_cls_kanban_d02");
}

#[test]
fn kanban_hand_drawn_matches_mmdc() {
    pinned("kanban_cls_handkanban_hk03_forest_width");
}
