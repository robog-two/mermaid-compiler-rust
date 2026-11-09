//! Rendering benchmarks without external crates: `cargo bench`.
//!
//! Prints the median and min wall-clock time per diagram for parse+layout+SVG,
//! plus PDF encoding, across sizes, so regressions in the layout engine show up
//! as super-linear growth.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;
use std::hint::black_box;
use std::time::{Duration, Instant};

#[path = "../tests/common/synth.rs"]
mod generate;

fn measure(name: &str, iters: usize, mut f: impl FnMut()) {
    f(); // warm-up
    let mut samples: Vec<Duration> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    samples.sort();
    println!(
        "{name:<44} median {:>10.3?}   min {:>10.3?}   ({iters} iters)",
        samples[samples.len() / 2],
        samples[0]
    );
}

fn main() {
    let cfg = MermaidConfig::default();
    let enc = EncodeOptions::default();
    let cases: Vec<(String, String, usize)> = vec![
        (
            "flowchart chain 100".into(),
            generate::flowchart_chain(100),
            30,
        ),
        (
            "flowchart random 200 nodes / 300 edges".into(),
            generate::flowchart_random(200, 300, 7),
            20,
        ),
        (
            "flowchart random 1000 nodes / 1500 edges".into(),
            generate::flowchart_random(1000, 1500, 7),
            5,
        ),
        (
            "flowchart 20 subgraphs x 10 nodes".into(),
            generate::flowchart_subgraphs(20, 10),
            10,
        ),
        (
            "flowchart labelled edges 300".into(),
            generate::flowchart_labelled(300),
            10,
        ),
        (
            "sequence 20 actors / 500 messages".into(),
            generate::sequence(20, 500),
            20,
        ),
        (
            "sequence 50 actors / 5000 messages".into(),
            generate::sequence(50, 5000),
            5,
        ),
        ("state 300 states".into(), generate::state(300), 10),
        ("class 150 classes".into(), generate::class(150), 10),
        ("pie 100 slices".into(), generate::pie(100), 50),
    ];
    for (name, src, iters) in &cases {
        measure(&format!("{name} [svg]"), *iters, || {
            let r = render::render(black_box(src), &cfg).expect("render");
            black_box(output::encode_svg(&r, &enc));
        });
    }
    let (_, big, _) = &cases[2];
    let r = render::render(big, &cfg).expect("render");
    measure("encode pdf (1000-node flowchart)", 10, || {
        black_box(output::encode_pdf(&r, &enc));
    });
}
