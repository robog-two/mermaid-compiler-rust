//! Scaling report: layout time and drawing size as the graph grows.
//!
//! `cargo run --release --example scaling`
//!
//! Uses graphs whose edges only reach a few nodes ahead, so the drawing should
//! grow linearly; time per output point exposes algorithmic cost.

#[path = "../tests/common/synth.rs"]
mod synth;

use mmdc::{config::MermaidConfig, output, render};
use std::time::Instant;

fn main() {
    println!(
        "{:>6} {:>12} {:>12} {:>14}",
        "nodes", "render", "svg bytes", "µs per node"
    );
    for n in [250usize, 500, 1000, 2000, 4000] {
        let src = synth::flowchart_local(n, 11);
        let t = Instant::now();
        let r = render::render(&src, &MermaidConfig::default()).expect("renders");
        let took = t.elapsed();
        let svg = output::encode_svg(&r, &output::EncodeOptions::default());
        println!(
            "{n:>6} {:>12.2?} {:>12} {:>14.1}",
            took,
            svg.len(),
            took.as_secs_f64() * 1e6 / n as f64
        );
    }
}
