//! Under ELK (mermaid's default layout) the order of nodes within each layer
//! follows ELK's layer-sweep crossing minimisation. The expected orders are those
//! mermaid 12.3 renders for these graphs (headless Chromium, elkjs 0.12).

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn attr(tag: &str, name: &str) -> f64 {
    let key = format!(" {name}=\"");
    let at = tag.find(&key).expect("attribute") + key.len();
    let end = at + tag[at..].find('"').expect("closing quote");
    tag[at..end].parse().expect("number")
}

/// Node centres `(name, rank-axis, cross-axis)` from our SVG.
fn centres(svg: &str, nodes: &[&str], horizontal: bool) -> Vec<(String, f64, f64)> {
    nodes
        .iter()
        .map(|n| {
            let key = format!("<g id=\"flowchart-{n}-");
            let at = svg.find(&key).expect("node group");
            let start = at + svg[at..].find("label-container").expect("shape");
            let open = svg[..start].rfind('<').expect("tag start");
            let end = start + svg[start..].find('>').expect("tag end");
            let tag = &svg[open..end];
            let x = attr(tag, "x") + attr(tag, "width") / 2.0;
            let y = attr(tag, "y") + attr(tag, "height") / 2.0;
            let (rank, cross) = if horizontal { (x, y) } else { (y, x) };
            (n.to_string(), rank, cross)
        })
        .collect()
}

/// Our layers (by the rank axis, nodes within 20px together), each ordered
/// along the cross axis.
fn layers(src: &str, nodes: &[&str], horizontal: bool) -> Vec<Vec<String>> {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let mut cs = centres(&svg, nodes, horizontal);
    cs.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut out: Vec<Vec<(String, f64)>> = Vec::new();
    let mut last = f64::NEG_INFINITY;
    for (n, rank, cross) in cs {
        if rank - last > 20.0 || out.is_empty() {
            out.push(Vec::new());
        }
        last = rank;
        out.last_mut().unwrap().push((n, cross));
    }
    out.into_iter()
        .map(|mut l| {
            l.sort_by(|a, b| a.1.total_cmp(&b.1));
            l.into_iter().map(|(n, _)| n).collect()
        })
        .collect()
}

fn expect(src: &str, horizontal: bool, want: &[&[&str]]) {
    let nodes: Vec<&str> = want.iter().flat_map(|l| l.iter().copied()).collect();
    let got = layers(src, &nodes, horizontal);
    let want: Vec<Vec<String>> = want
        .iter()
        .map(|l| l.iter().map(|s| s.to_string()).collect())
        .collect();
    assert_eq!(got, want);
}

#[test]
fn port_barycentres_and_restarts_match_elk() {
    // The dagre-style sweeps order every layer here differently.
    let src = "flowchart TD
  N11 --> N9
  N3 --> N4
  N1 --> N4
  N2 --> N11
  N1 --> N10
  N7 --> N10
  N6 --> N3
  N2 --> N3
  N6 --> N8
  N2 --> N9
  N3 --> N7
  N8 --> N10
  N1 --> N8
  N0 --> N8
  N1 --> N11
  N3 --> N8
";
    expect(
        src,
        false,
        &[
            &["N6", "N2"],
            &["N3", "N0", "N1"],
            &["N7", "N8", "N4", "N11"],
            &["N10", "N9"],
        ],
    );
}

#[test]
fn model_order_and_sweeps_match_elk_left_to_right() {
    let src = "flowchart LR
  N2 --> N3
  N4 --> N8
  N4 --> N9
  N1 --> N7
  N6 --> N10
  N0 --> N4
  N0 --> N5
  N3 --> N6
  N9 --> N10
  N3 --> N9
  N0 --> N10
  N2 --> N9
  N4 --> N5
  N0 --> N9
  N5 --> N8
";
    expect(
        src,
        true,
        &[
            &["N2", "N1", "N0"],
            &["N3", "N7", "N4"],
            &["N6", "N9", "N5"],
            &["N10", "N8"],
        ],
    );
}

#[test]
fn reversed_edges_order_like_elk() {
    let src = "flowchart LR
  N0 --> N1
  N0 --> N3
  N0 --> N2
  N4 --> N5
  N6 --> N3
  N1 --> N5
  N3 --> N6
  N3 --> N0
";
    expect(
        src,
        true,
        &[&["N0"], &["N3", "N2", "N1", "N4"], &["N6", "N5"]],
    );
}
