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

#[test]
fn hierarchical_sweeps_draw_like_elk() {
    // Four containers: ELK orders C1, C2 and C3 on their own (in the reverse
    // of a breadth-first walk over the layers), and C0 within the root's
    // sweeps; C1's port for `C1 --> C3` is no node inside C1 (it has no other
    // external port), while C2's port for `c0n1 --> C2` is. With every random
    // draw in ELK's order, c2n0 stands left of c2n1, as in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0 --> c0n1
  end
  subgraph C1
    c1n0 --> c1n1
  end
  subgraph C2
    c2n0 --> c2n1
    c2n1 --> c2n2
  end
  subgraph C3
    c3n0 --> c3n1
    c3n1 --> c3n2
  end
  c2n0 --> c3n1
  c0n0 --> x
  C1 --> C3
  c2n1 --> c3n1
  c0n1 --> C2
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let names = ["c0n0", "c0n1", "c2n0", "c2n1", "c2n2", "c3n0", "c3n1"];
    let cs = centres(&svg, &names, false);
    let x = |n: &str| cs.iter().find(|c| c.0 == n).expect("node").2;
    assert!(x("c0n0") < x("c0n1"));
    assert!(x("c2n0") < x("c2n1"));
    assert!(x("c2n2") < x("c2n1"));
    assert!(x("c3n0") < x("c3n1"));
}

#[test]
fn unconnected_port_of_a_container_with_crossing_edges_is_placed_inside() {
    // C0 has ports for crossing edges, so ELK makes a dummy inside it for the
    // port of `C0 --> C1` too, placed with the rest: the port is not centred
    // on C0's bottom, and C0 (with C2 above it) stands 65px right of C1, as
    // in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0 --> c0n1
    c0n1 --> c0n2
  end
  subgraph C1
    c1n0 --> c1n1
    c1n1 --> c1n2
  end
  subgraph C2
    c2n0 --> c2n1
  end
  C2 --> c0n0
  C0 --> C1
  c0n2 --> C2
  c1n2 --> c1n0
  y --> x
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c0n0", "c1n0", "c2n0"], false);
    let x = |n: &str| cs.iter().find(|c| c.0 == n).expect("node").2;
    assert!((x("c0n0") - x("c1n0") - 65.1).abs() < 1.0, "{cs:?}");
    assert!((x("c2n0") - x("c1n0") - 102.1).abs() < 1.0, "{cs:?}");
}

#[test]
fn external_port_dummies_are_made_in_elks_order() {
    // ELK makes C2's external port dummies child by child (c2n0, c2n1, c2n2),
    // then the one of `C2 --> c1n0`, unconnected inside: not in edge order.
    // From that start the first sweep leaves one crossing, as in ELK, and C2
    // is not mirrored: c2n2 left of c2n0, left of c2n1, as in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0 --> c0n1
    c0n1 --> c0n2
  end
  subgraph C1
    c1n0
  end
  subgraph C2
    c2n0 --> c2n1
    c2n1 --> c2n2
  end
  c2n0 --> x
  c2n2 --> x
  C2 --> c1n0
  c2n1 --> c1n0
  x --> c1n0
  c2n0 --> c2n1
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c2n0", "c2n1", "c2n2", "x", "c0n0"], false);
    let x = |n: &str| cs.iter().find(|c| c.0 == n).expect("node").2;
    assert!(x("c2n2") < x("c2n0") && x("c2n0") < x("c2n1"), "{cs:?}");
    assert!(x("x") < x("c2n0") && x("c2n1") < x("c0n0"), "{cs:?}");
}

#[test]
fn long_edge_dummies_ports_and_jogs_measure_as_in_elk() {
    // Long-edge dummies are as thick as their edge (1px), ports inside a
    // container keep 10.8px from the ends of a side, and a jog of a fraction
    // of a pixel still takes a routing slot: c2n2 sits 48.5px below c2n0 and
    // C3's layers are 224px apart (two slots for the edges into c3n1), as in
    // mermaid's render.
    let src = "flowchart LR
  subgraph C0
    c0n0
  end
  subgraph C1
    c1n0
  end
  subgraph C2
    c2n0 --> c2n1
    c2n1 --> c2n2
  end
  subgraph C3
    c3n0 --> c3n1
  end
  c2n0 --> c3n1
  y --> C1
  x --> C0
  C0 --> c2n2
  c3n1 --> c2n0
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c2n0", "c2n2", "c3n0", "c3n1"], true);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("c2n2").2 - at("c2n0").2 - 48.5).abs() < 0.2, "{cs:?}");
    assert!((at("c3n1").1 - at("c3n0").1 - 224.0).abs() < 0.2, "{cs:?}");
}

#[test]
fn placement_follows_the_port_order_crossing_minimisation_leaves() {
    // C1's ports towards C0 stay in the order they were made (the root's
    // model order has no crossing, so crossing minimisation keeps it): the one
    // of `C1 --> c0n0` first, although C1's contents put it right of the one
    // of `c1n0 --> c0n1`. Brandes-Koepf takes edges in that order: C0 stands
    // 10px left of C1 (centres of c0n0 and c1n0), and two edges cross between
    // them, as in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0 --> c0n1
    c0n1 --> c0n2
  end
  subgraph C1
    c1n0
  end
  subgraph C2
    c2n0
  end
  y --> c1n0
  c0n2 --> c0n1
  C0 --> C1
  c0n0 --> C1
  x --> C1
  c1n0 --> c0n1
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c0n0", "c1n0", "y"], false);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("c1n0").2 - at("c0n0").2 - 10.0).abs() < 1.0, "{cs:?}");
    assert!((at("c1n0").2 - at("y").2 - 56.0).abs() < 1.0, "{cs:?}");
    assert!((at("c0n0").1 - at("c1n0").1 - 197.0).abs() < 1.0, "{cs:?}");
}

#[test]
fn port_list_sorter_reverses_all_but_the_last_west_port() {
    // ELK's `PortListSorter` reverses a west side of four or more ports but for
    // its last one; s0n1's ports stand [s0n0, x0, x0, s0n0], so the edges from
    // x0 pass on both sides of s0n0 and s0n1 sits 8.7px left of it, x0 0.5px,
    // as in mermaid's render.
    let src = "flowchart TD
  subgraph S0
    s0n0
    s0n1
    s0n0 --> s0n1
    s0n0 --> s0n1
  end
  x0 --> s0n1
  x0 --> s0n1
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["s0n0", "s0n1", "x0"], false);
    let x = |n: &str| cs.iter().find(|c| c.0 == n).expect("node").2;
    assert!((x("s0n0") - x("s0n1") - 8.7).abs() < 0.5, "{cs:?}");
    assert!((x("s0n0") - x("x0") - 0.5).abs() < 0.5, "{cs:?}");
}

#[test]
fn ports_of_edges_into_a_container_itself_follow_the_crossing_minimisation() {
    // C0's ports are all on edges to C0 itself: no dummies inside, so its
    // parent orders them (c2n1's above y's) and the two jogs share a slot:
    // c0n0 stands 250px right of c2n1, as in mermaid's render.
    let src = "flowchart LR
  subgraph C0
    c0n0 --> c0n1
  end
  subgraph C1
    c1n0
  end
  subgraph C2
    c2n0 --> c2n1
  end
  y --> C0
  c2n1 --> C0
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c2n1", "c0n0"], true);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("c0n0").1 - at("c2n1").1 - 250.0).abs() < 0.5, "{cs:?}");
}

#[test]
fn a_container_without_port_dummies_leaves_its_port_order_free() {
    // C1's two edges start at C1 itself: its graph has no external port
    // dummies, so ELK does not fix C1's port order and the parent's sweeps
    // order its ports by C0's; C1's right port lines up with C0's: c1n0 sits
    // 23.8px right of c0n2, as in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0 --> c0n1
    c0n1 --> c0n2
  end
  subgraph C1
    c1n0 --> c1n1
  end
  C1 --> c0n2
  C1 --> c0n2
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c0n2", "c1n0"], false);
    let x = |n: &str| cs.iter().find(|c| c.0 == n).expect("node").2;
    assert!((x("c1n0") - x("c0n2") - 23.8).abs() < 0.5, "{cs:?}");
}

#[test]
fn ports_reaching_a_container_keep_the_crossing_minimisation_order() {
    // x2's ports stay in the order crossing minimisation left them (s0n2's
    // edge first, from S0's ports as made), although S0's contents place
    // s0n1's port left of s0n2's: the edges cross below S0 and x2 is centred
    // under it (balanced between the alignments), as in mermaid's render.
    let src = "flowchart TD
  subgraph S0
    s0n0
    s0n1
    s0n2
    s0n0 -->|x| s0n1
  end
  x2 --> x1
  s0n2 --> x2
  s0n1 --> x2
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["s0n1", "s0n2", "x2"], false);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    let mid = (at("s0n1").2 + at("s0n2").2) / 2.0;
    assert!((at("x2").2 - mid).abs() < 0.5, "{cs:?}");
    assert!((at("x2").1 - at("s0n1").1 - 139.0).abs() < 0.5, "{cs:?}");
}

#[test]
fn inverted_ports_of_a_container_nest_their_in_layer_edges() {
    // The three edges from x0 into C0 run back up: C0's ports for them stay
    // on its top, and in-layer dummies beside C0 take them on. ELK makes the
    // dummies in C0's port order and its port distributor nests their in-layer
    // edges, so all three loop round on one side, three routing slots deep:
    // x0 stands 108.6px right of c0n0 and 149px below it, as in mermaid's render.
    let src = "flowchart TD
  subgraph C0
    c0n0
  end
  x0
  x0 --> C0
  C0 --> x0
  x0 --> c0n0
  x0 --> c0n0
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["c0n0", "x0"], false);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("x0").2 - at("c0n0").2 - 108.6).abs() < 0.5, "{cs:?}");
    assert!((at("x0").1 - at("c0n0").1 - 149.0).abs() < 0.5, "{cs:?}");
}

#[test]
fn feedback_edges_around_nested_containers_order_like_elk() {
    // c1n0 --> x0 and x1 --> c0n0 run against the flow; with their in-layer
    // dummies weighed in port order, x1 ends up 185.25px below x0 and 212px
    // right of it, as in mermaid's render.
    let src = "flowchart LR
  subgraph C0
    c0n0
    subgraph C1
      c1n0
    end
  end
  x0
  x1
  x1 --> c0n0
  x0 --> x1
  c0n0 --> x1
  c1n0 --> x0
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["x0", "x1"], true);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("x1").1 - at("x0").1 - 212.0).abs() < 0.5, "{cs:?}");
    assert!((at("x1").2 - at("x0").2 - 185.25).abs() < 0.5, "{cs:?}");
}

#[test]
fn a_members_edges_to_its_own_container_run_to_port_dummies_inside_it() {
    // `a --> C` leaves through a port dummy in C's last layer, `C --> b` comes
    // in from one in its first: b's dummy starts C's first layer, so b stands
    // left of a, 202px apart, as in mermaid's render (not a loop on each).
    let src = "flowchart TD
  subgraph C
    a
    b
  end
  a --> C
  C --> b
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let cs = centres(&svg, &["a", "b"], false);
    let at = |n: &str| cs.iter().find(|c| c.0 == n).expect("node");
    assert!((at("a").2 - at("b").2 - 202.0).abs() < 0.5, "{cs:?}");
    assert!((at("a").1 - at("b").1).abs() < 0.5, "{cs:?}");
    // No loop room: b's edge out to C leaves C's frame as tall as without it.
    let height = |src: &str| {
        let r = render::render(src, &MermaidConfig::default()).expect("renders");
        let svg = output::encode_svg(&r, &EncodeOptions::default());
        let at = svg.find("id=\"cluster-C\"").expect("cluster");
        let rect = at + svg[at..].find("<rect x=").expect("frame");
        let end = rect + svg[rect..].find('>').expect("tag end");
        attr(&svg[rect..end], "height")
    };
    let with = height("flowchart TD\n  subgraph C\n    x --> y\n  end\n  y --> C\n");
    let without = height("flowchart TD\n  subgraph C\n    x --> y\n  end\n");
    assert!((with - without).abs() < 0.5, "{with} {without}");
}

/// Every node's centre, relative to the least x and y among them, within
/// half a pixel of mermaid's render.
fn expect_centres(src: &str, want: &[(&str, f64, f64)]) {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let names: Vec<&str> = want.iter().map(|w| w.0).collect();
    let cs = centres(&svg, &names, true);
    let x0 = cs.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    let y0 = cs.iter().map(|c| c.2).fold(f64::INFINITY, f64::min);
    for ((n, x, y), &(_, wx, wy)) in cs.iter().zip(want) {
        assert!(
            (x - x0 - wx).abs() < 0.5 && (y - y0 - wy).abs() < 0.5,
            "{n}: {} {} vs {wx} {wy}\n{cs:?}",
            x - x0,
            y - y0
        );
    }
}

#[test]
fn an_edge_to_an_ancestor_container_runs_back_to_a_port_against_the_flow() {
    // `c1n0 --> C0`: C0's port counts the edge as incoming (its source is no
    // child of C0), so its dummy is in C0's first layer: the edge leaves C1 at
    // the bottom and comes back up inside C0, which grows to make room.
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
    end
    c0n0 --> c0n1
  end
  x0
  x1
  c1n0 -->|x| x0
  x1 -->|x| C0
  c1n0 --> C0
  c0n1 --> c0n0
",
        &[
            ("c0n0", 0.0, 266.0),
            ("c0n1", 0.0, 381.0),
            ("c1n0", 226.0, 242.0),
            ("x0", 204.27, 561.0),
            ("x1", 350.0, 0.0),
        ],
    );
}

#[test]
fn a_containers_port_without_an_outside_edge_counts_for_the_sweep_type() {
    // C1's own port for `C1 --> c2n0` has no edge in C0's graph, but with the
    // port for `c2n0 --> x0` it makes two on C1's side: C1 is swept within C0
    // (not bottom-up), and its dummy comes first on that side.
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
    subgraph C1
      c1n0
      subgraph C2
        c2n0
      end
    end
  end
  x0
  c2n0 --> x0
  C1 --> c2n0
",
        &[
            ("c0n0", 0.0, 0.0),
            ("c1n0", 226.0, 22.0),
            ("c2n0", 426.0, 34.0),
            ("x0", 426.0, 221.0),
        ],
    );
}

#[test]
fn a_containers_unconnected_port_dummy_goes_last_on_its_west_side() {
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
    subgraph C1
      c1n0
    end
  end
  subgraph C2
    c2n0
    subgraph C3
      c3n0
      c3n1
      c3n2
      subgraph C4
        c4n0
        c4n1
        c4n2
        c4n1 --> c4n2
      end
      c3n1 --> c3n2
    end
  end
  x0
  x1
  C2 --> c3n0
  x1 --> C3
  x1 --> c3n1
  c4n0 --> C3
",
        &[
            ("c0n0", 242.0, 12.0),
            ("c1n0", 16.0, 24.0),
            ("c2n0", 0.0, 444.0),
            ("c3n0", 200.0, 333.0),
            ("c3n1", 402.0, 408.5),
            ("c3n2", 402.0, 589.0),
            ("c4n0", 830.0, 450.0),
            ("c4n1", 628.0, 381.0),
            ("c4n2", 628.0, 450.0),
            ("x0", 458.0, 0.0),
            ("x1", 650.0, 72.0),
        ],
    );
}

#[test]
fn labels_of_edges_to_ancestor_containers_take_their_layers() {
    // `c3n0 -->|yes| C2`: the label dummy stays in C2's layering, tied to C3,
    // when the edge to the port dummy is hidden from it.
    expect_centres(
        "flowchart BT
  subgraph C0
    c0n0
  end
  subgraph C1
    c1n0
  end
  subgraph C2
    c2n0
    subgraph C3
      c3n0
      c3n1
      c3n2
      c3n0 --> c3n1
      c3n1 --> c3n2
    end
  end
  x0
  x1
  c0n0 -->|yes| c2n0
  x0 --> C0
  c3n0 -->|yes| C2
  c0n0 --> x0
",
        &[
            ("c0n0", 314.53, 529.0),
            ("c1n0", 62.53, 524.0),
            ("c2n0", 292.8, 267.0),
            ("c3n0", 44.0, 174.0),
            ("c3n1", 0.0, 69.0),
            ("c3n2", 0.0, 0.0),
            ("x0", 419.59, 386.0),
            ("x1", 537.59, 512.0),
        ],
    );
    // `C0 -->|yes| C2`: the label moves into C1's layer, to the dummy of
    // C1's inverted port (`LabelDummySwitcher`), above C1.
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
      c1n1
      subgraph C2
        c2n0
      end
      c1n0 --> c1n1
    end
    c0n0 --> c0n1
  end
  x0
  x1
  c0n1 -->|x| x0
  C1 -->|a label| C2
  C0 -->|yes| C2
",
        &[
            ("c0n0", 248.0, 238.0),
            ("c0n1", 424.0, 238.0),
            ("c1n0", 0.0, 119.0),
            ("c1n1", 176.0, 119.0),
            ("c2n0", 200.0, 0.0),
            ("x0", 697.0, 238.0),
            ("x1", 185.0, 347.0),
        ],
    );
}

#[test]
fn a_containers_ports_feed_its_channel_in_port_list_order() {
    // C0's four edges into the channel above it (BT) cross pairwise with
    // equal penalties; ELK breaks those ties by the order of its hyperedge
    // segments, which follows C0's port list (edge order here), not where
    // the ports ended up from C0's contents: three routing slots, not two.
    expect_centres(
        "flowchart BT
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
    end
  end
  subgraph C2
    c2n0
    c2n1
    c2n2
    c2n0 --> c2n1
    c2n1 --> c2n2
  end
  x0
  x1
  c0n1 --> c2n1
  c2n2 --> c1n0
  c0n0 --> x1
  c0n1 --> x1
",
        &[
            ("c0n0", 304.0, 437.0),
            ("c0n1", 506.0, 437.0),
            ("c1n0", 78.0, 485.0),
            ("c2n0", 0.0, 210.0),
            ("c2n1", 44.0, 105.0),
            ("c2n2", 88.0, 0.0),
            ("x0", 722.0, 451.0),
            ("x1", 559.73, 234.0),
        ],
    );
}
