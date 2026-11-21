//! `render::render_svg` is the library's way to get the SVG file the `mmdc`
//! binary writes: same bytes, numbers printed as JavaScript prints them.

use mmdc::config::MermaidConfig;
use mmdc::output::EncodeOptions;
use mmdc::render::{self, RenderEnv};
use std::process::Command;

fn cli_svg(src: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("mmdc-render-svg-api-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join(format!("{name}.mmd"));
    let output = dir.join(format!("{name}.svg"));
    std::fs::write(&input, src).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_mmdc"))
        .arg("-q")
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read_to_string(&output).unwrap()
}

fn lib_svg(src: &str) -> String {
    let env = RenderEnv {
        now_ms: Some(1_700_000_000_000),
        ..Default::default()
    };
    render::render_svg(
        src,
        &MermaidConfig::default(),
        &env,
        &EncodeOptions::default(),
    )
    .unwrap()
}

#[test]
fn library_svg_matches_cli_for_a_flowchart() {
    // Edge curves and label positions carry fractional coordinates, where the
    // compact and JavaScript number forms differ.
    let src =
        "flowchart LR\n  A[Start] --> B{Is it?}\n  B -- Yes --> C[OK]\n  B -- No --> D[Rethink]\n";
    let lib = lib_svg(src);
    assert_eq!(lib, cli_svg(src, "flow"));
}

#[test]
fn library_svg_matches_cli_for_a_sequence_diagram() {
    let src = "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi there, a longer reply\n";
    assert_eq!(lib_svg(src), cli_svg(src, "seq"));
}
