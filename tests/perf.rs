//! Speed tests for diagram rendering.
//!
//! Each test renders a large synthetic diagram and asserts a wall-clock budget.
//! Budgets are set for release builds (`cargo test --release --test perf`) and
//! multiplied by 15 in debug builds, so they catch algorithmic regressions
//! (e.g. accidental O(n²) layout) without flaking on slow CI machines.
//! Scaling tests compare two input sizes to detect super-linear growth directly.

mod common;
use common::synth;

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The tests time themselves, so they take turns: run in parallel they would
/// compete for the same cores and measure each other's work.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn budget(release_ms: u64) -> Duration {
    let factor = if cfg!(debug_assertions) { 15 } else { 1 };
    Duration::from_millis(release_ms * factor)
}

/// Best-of-5 time to render to SVG (min is robust to scheduler noise).
fn time_svg(src: &str) -> Duration {
    let cfg = MermaidConfig::default();
    let enc = EncodeOptions::default();
    (0..5)
        .map(|_| {
            let t = Instant::now();
            let r = render::render(src, &cfg).expect("diagram renders");
            let svg = output::encode_svg(&r, &enc);
            assert!(svg.len() > 100);
            t.elapsed()
        })
        .min()
        .unwrap_or_default()
}

fn assert_within(name: &str, took: Duration, limit: Duration) {
    eprintln!("{name}: {took:?} (budget {limit:?})");
    assert!(took <= limit, "{name} took {took:?}, budget {limit:?}");
}

#[test]
fn small_flowchart_is_fast() {
    let _turn = serial();
    assert_within(
        "flowchart 10",
        time_svg(&synth::flowchart_chain(10)),
        budget(10),
    );
}

#[test]
fn flowchart_500_nodes() {
    let _turn = serial();
    assert_within(
        "flowchart 500/750",
        time_svg(&synth::flowchart_random(500, 750, 3)),
        budget(250),
    );
}

#[test]
fn flowchart_with_subgraphs() {
    let _turn = serial();
    assert_within(
        "subgraphs 30x10",
        time_svg(&synth::flowchart_subgraphs(30, 10)),
        budget(100),
    );
}

#[test]
fn flowchart_labelled_edges() {
    let _turn = serial();
    assert_within(
        "labelled 300",
        time_svg(&synth::flowchart_labelled(300)),
        budget(100),
    );
}

#[test]
fn sequence_2000_messages() {
    let _turn = serial();
    assert_within(
        "sequence 30x2000",
        time_svg(&synth::sequence(30, 2000)),
        budget(100),
    );
}

#[test]
fn state_300() {
    let _turn = serial();
    assert_within("state 300", time_svg(&synth::state(300)), budget(100));
}

#[test]
fn class_200() {
    let _turn = serial();
    assert_within("class 200", time_svg(&synth::class(200)), budget(100));
}

#[test]
fn pie_500_slices() {
    let _turn = serial();
    assert_within("pie 500", time_svg(&synth::pie(500)), budget(50));
}

#[test]
fn pdf_encoding_is_fast() {
    let _turn = serial();
    let r = render::render(
        &synth::flowchart_random(300, 450, 5),
        &MermaidConfig::default(),
    )
    .unwrap();
    let t = Instant::now();
    let pdf = output::encode_pdf(&r, &EncodeOptions::default());
    assert!(pdf.starts_with(b"%PDF"));
    assert_within("pdf 300", t.elapsed(), budget(50));
}

/// A 4× larger graph (whose drawing also grows 4×) must take well under 16× the
/// time, i.e. the layout must not be quadratic. Random graphs with long edges are
/// unsuitable here: their dummy-vertex count, and so the drawing, grows ~n^1.6.
#[test]
fn flowchart_layout_scales_subquadratically() {
    let _turn = serial();
    let small = time_svg(&synth::flowchart_local(500, 11));
    let large = time_svg(&synth::flowchart_local(2000, 11));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    eprintln!("flowchart 500→2000 nodes: {small:?} → {large:?} (×{ratio:.1})");
    assert!(ratio < 10.0, "layout grew ×{ratio:.1} for 4× input");
}

#[test]
fn sequence_scales_linearly() {
    let _turn = serial();
    let small = time_svg(&synth::sequence(20, 1000));
    let large = time_svg(&synth::sequence(20, 4000));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    eprintln!("sequence 1000→4000 messages: {small:?} → {large:?} (×{ratio:.1})");
    assert!(ratio < 8.0, "sequence grew ×{ratio:.1} for 4× input");
}

#[cfg(feature = "png")]
#[test]
fn png_rasterisation() {
    let _turn = serial();
    let r = render::render(
        &synth::flowchart_random(100, 150, 2),
        &MermaidConfig::default(),
    )
    .unwrap();
    let rast = mmdc::output::png::Rasterizer::new();
    let t = Instant::now();
    let png = output::encode_png(&r, &EncodeOptions::default(), &rast).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    assert_within("png 100 nodes", t.elapsed(), budget(1500));
}

/// Every bundled example (all diagram types, including the Taffy-based ones)
/// must render well inside an interactive budget.
#[test]
fn every_example_renders_quickly() {
    let _turn = serial();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/diagrams");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "mmd"))
        .collect();
    paths.sort();
    for p in paths {
        let src = std::fs::read_to_string(&p).unwrap();
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        assert_within(&name, time_svg(&src), budget(25));
    }
}

/// Large inputs for the newer diagram families.
#[test]
fn large_newer_diagrams() {
    let _turn = serial();
    use std::fmt::Write as _;
    let mut gantt = String::from("gantt\ndateFormat YYYY-MM-DD\nsection S\n");
    let _ = writeln!(gantt, "t0 :a0, 2024-01-01, 2d");
    for i in 1..400 {
        let _ = writeln!(gantt, "task {i} :a{i}, after a{}, 1d", i - 1);
    }
    assert_within("gantt 400 tasks", time_svg(&gantt), budget(100));

    let mut er = String::from("erDiagram\n");
    for i in 0..150 {
        let _ = writeln!(er, "E{i} ||--o{{ E{} : has", (i * 7 + 1) % 150);
    }
    assert_within("er 150 entities", time_svg(&er), budget(200));

    let mut kanban = String::from("kanban\n");
    for c in 0..10 {
        let _ = writeln!(kanban, "  col{c}[Column {c}]");
        for i in 0..40 {
            let _ = writeln!(
                kanban,
                "    c{c}i{i}[Card {i} with some longer text to wrap]"
            );
        }
    }
    assert_within("kanban 400 cards (taffy)", time_svg(&kanban), budget(100));

    let mut sankey = String::from("sankey-beta\n");
    for i in 0..60 {
        let _ = writeln!(sankey, "S{},T{},{}", i % 12, i % 17, i + 1);
        let _ = writeln!(sankey, "T{},U{},{}", i % 17, i % 5, i + 2);
    }
    assert_within("sankey 120 links", time_svg(&sankey), budget(100));
}
