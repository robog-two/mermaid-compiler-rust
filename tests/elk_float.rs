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

/// The on-curve points of the path of edge `from` → `to` (our first such edge).
fn edge_points(svg: &str, from: &str, to: &str) -> Vec<(f64, f64)> {
    let key = format!("<g id=\"L-{from}-{to}-");
    let at = svg.find(&key).expect("edge group");
    let d_at = at + svg[at..].find(" d=\"").expect("path data") + 4;
    let d = &svg[d_at..d_at + svg[d_at..].find('"').expect("end of data")];
    let mut pts = Vec::new();
    for cmd in d
        .split(|c: char| c.is_ascii_alphabetic())
        .filter(|s| !s.is_empty())
    {
        let nums: Vec<f64> = cmd
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().expect("number"))
            .collect();
        if nums.len() >= 2 {
            pts.push((nums[nums.len() - 2], nums[nums.len() - 1]));
        }
    }
    pts
}

fn near(p: (f64, f64), x: f64, y: f64) -> bool {
    (p.0 - x).abs() < 0.01 && (p.1 - y).abs() < 0.01
}

/// ELK carries g0n0's port out of G0 into the reference graph, G4's, and
/// from there to the root (`(75.58333333333333 + 493.875) + 622`): it lands
/// at 1191.4583333333335, an ulp inside the asymmetric node mermaid places
/// at 1191.4583333333333 (summed down from the root). mermaid's `cutter2`
/// then cuts the edge where the line from the node's centre to the next
/// point leaves the outline, a slanted last step into the notch.
#[test]
fn port_an_ulp_inside_its_node_is_cut_further_on() {
    let src = "flowchart LR
  subgraph G0[\"`**Box** G0`\"]
    g0n0>\"Merge\"]
    subgraph G1
      g1n0
    end
    g0n0 ==> g0n1
  end
  subgraph G4[Group G4]
    g4n0
    g4n1
  end
  g3n0 e0@===> g4n1
  g4n0 ---->|longer edge label text| g0n0
  g2n1 --> g3n0
  g0n0 ----> g3n0
  g2n0 --> g0n2
  g1n0 === g4n1
  g0n2 --x|longer edge label text| g4n1
";
    let pts = edge_points(&svg(src), "g4n0", "g0n0");
    let last = pts[pts.len() - 1];
    assert!(near(last, 1197.632, 85.398), "{pts:?}");
}

/// A cylinder's east side as ELK and mermaid sum it, in a container in a
/// container: the ports' 422.66666666666663 against the side's
/// 422.6666666666667, the ports inside.
#[test]
fn ports_inside_a_nested_cylinder() {
    let src = "flowchart LR
    subgraph G2[Group G2]
      subgraph G3[Group G3]
        g3n0[(\"Save\")]
        g3n1
      end
    end
    subgraph G5
        g6n0
      g5n0 --o g5n1
    end
  g5n1 --x g3n0
  g0n1 & g6n0 ----> g3n0
  g0n1 --- g3n0
  G1 --> g3n0 --> g0n0
  g0n0 --> g0n1 --> g5n0
";
    let svg = svg(src);
    let into = edge_points(&svg, "g5n1", "g3n0");
    assert!(near(into[into.len() - 1], 422.667, 230.815), "{into:?}");
    let out = edge_points(&svg, "g3n0", "g0n0");
    assert!(near(out[0], 422.667, 260.334), "{out:?}");
}

/// A container laid out right inside a graph laid out down: its position
/// across the root's flow (103.00000000000006) carries into b10n0's corner
/// and its port alike, in different orders.
#[test]
fn port_inside_a_node_of_a_turned_container() {
    let src = "flowchart TD
  subgraph A1
    direction LR
    subgraph B10
      b10n0
      subgraph C10
        c10n0
      end
    end
    a1n0 --> a1n1
  end
  b01n1 --> c01n0
  b01n0 --> c01n0
  c10n0 --> b10n0
  b10n0 --> c10n0
  B10 --> a1n1
  x0 --> A0
  x0 --> A1
  A0 --> A1
";
    let pts = edge_points(&svg(src), "b10n0", "c10n0");
    assert!(near(pts[0], 481.0, 345.704), "{pts:?}");
}
