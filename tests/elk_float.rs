//! Layouts where a decision of ELK's hangs on the last bits of a coordinate:
//! our arithmetic has to produce the same doubles as elkjs at that point. The
//! expected values are those mermaid 12.3 renders for these graphs (headless
//! Chromium, elkjs 0.12).

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn attr(tag: &str, name: &str) -> f64 {
    let key = format!(" {name}=\"");
    let at = tag.find(&key).expect("attribute") + key.len();
    let end = at + tag[at..].find('"').expect("closing quote");
    tag[at..end].parse().expect("number")
}

/// The frame of cluster `id`: `(x, y, width, height)`.
fn cluster(svg: &str, id: &str) -> (f64, f64, f64, f64) {
    let key = format!("<g id=\"cluster-{id}\" class=\"cluster\">");
    let at = svg.find(&key).expect("cluster group") + key.len();
    let mut rest = &svg[at..];
    if rest.starts_with("<rect class=\"drop-shadow\"") {
        rest = &rest[rest.find('>').expect("tag end") + 1..];
    }
    let tag = &rest[..rest.find('>').expect("tag end")];
    (
        attr(tag, "x"),
        attr(tag, "y"),
        attr(tag, "width"),
        attr(tag, "height"),
    )
}

/// Two vertical segments of a channel in G5 stand exactly half the edge
/// spacing apart: ELK spreads g5n1's four ports by adding its step to the
/// one before (`15.480000000000002, 20.160000000000004, ...`), which puts
/// them a hair under 6 apart, a conflict: the channel takes one more slot
/// and G5 is 12 wider.
#[test]
fn port_spread_rounds_as_elk_does() {
    let src = "flowchart LR
  subgraph G0
    g0n1
    subgraph G1[Group G1]
      subgraph G3[Group G3]
        g3n0 --> g3n1
      end
      g1n0 --> g1n1
    end
      subgraph G5[Group G5]
        g5n0[/\"Parse input\"/]
        g5n2
        g5n0 --o g5n1
      end
  end
  G3 --> g0n1 --> g5n1
  g5n0 -...-> g4n0
  g5n1 === g3n0
  g0n0 --o|a label| g5n0
  g5n1 ~~~ g1n0
  g5n0 e1@--x g1n2
  e2@{ animate: true }
  g2n0 --> g5n2
  g6n0 -...-> g5n0 & x1
  g5n2 & x2 ~~~ g4n0 & g0n0
";
    let (_, _, w, h) = cluster(&svg(src), "G5");
    assert!((w - 482.0).abs() < 1e-3, "G5 width {w}");
    assert!((h - 256.86).abs() < 1e-3, "G5 height {h}");
}
