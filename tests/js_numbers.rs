//! The CLI writes its SVG numbers as JavaScript prints them (shortest digits
//! that read back, no rounding to three decimals), the way mermaid's DOM does.
//! Rendering and encoding both happen inside `with_js_numbers`.

use mmdc::config::MermaidConfig;
use mmdc::output::svg::with_js_numbers;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str, js: bool) -> String {
    let go = || {
        let r = render::render(src, &MermaidConfig::default()).expect("renders");
        output::encode_svg(&r, &EncodeOptions::default())
    };
    if js { with_js_numbers(go) } else { go() }
}

/// Every number in the `d` attributes of `svg`, as written.
fn numbers(svg: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for d in svg.split(" d=\"").skip(1) {
        let d = &d[..d.find('"').unwrap_or(d.len())];
        numbers_in(d, &mut out);
    }
    out
}

fn numbers_in<'a>(svg: &'a str, out: &mut Vec<&'a str>) {
    let b = svg.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let s = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push(&svg[s..i]);
        } else {
            i += 1;
        }
    }
}

const SRC: &str = "flowchart LR\n  a[Alpha] --> b{Beta}\n  b --> c((Gamma))\n";

#[test]
fn numbers_keep_every_digit_javascript_would_print() {
    let js = svg(SRC, true);
    let plain = svg(SRC, false);
    let long = |s: &str| {
        numbers(s)
            .into_iter()
            .any(|n| n.split_once('.').is_some_and(|(_, f)| f.len() > 3))
    };
    assert!(long(&js), "{:?}", numbers(&js));
    assert!(!long(&plain), "{:?}", numbers(&plain));
}

#[test]
fn javascript_numbers_round_trip_to_the_plain_ones() {
    let js = svg(SRC, true);
    let plain = svg(SRC, false);
    let (a, b) = (numbers(&js), numbers(&plain));
    assert!(!a.is_empty());
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
        assert!((x - y).abs() <= 5e-4, "{x} vs {y}");
    }
}

#[test]
fn the_switch_is_scoped_to_the_closure() {
    let _ = svg(SRC, true);
    assert_eq!(mmdc::output::svg::num(1.23456), "1.235");
}
