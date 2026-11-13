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

#[test]
fn a_moved_labels_layer_next_to_a_port_layer_keeps_its_room() {
    // `C2 -->|yes| c3n0` runs back from C2's port dummy in its last layer to
    // C3; LabelDummySwitcher moves the label into C3's layer, and the layer
    // it leaves between C3 and the port dummies still takes its gaps: C2 is
    // 24px wider than without the label (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C2
    c2n0
    subgraph C3
      c3n0 --> c3n1
    end
  end
  x0
  C2 -->|yes| c3n0
  c3n1 --> x0
",
        &[
            ("c2n0", 88.0, 119.0),
            ("c3n0", 0.0, 0.0),
            ("c3n1", 176.0, 0.0),
            ("x0", 460.0, 0.0),
        ],
    );
}

#[test]
fn a_container_laid_out_on_its_own_packs_its_components() {
    // An LR subgraph (`SEPARATE_CHILDREN`) is laid out by ELK one connected
    // component at a time, packed in rows smallest first: the lone c1 and the
    // b chain share the first row, the a chain and the d tree follow, 24px
    // (`spacing.componentComponent`) apart (mermaid 12.3.0).
    expect_centres(
        "flowchart TD
  subgraph S
    direction LR
    a1 --> a2 --> a3
    b1[a longer label] --> b2
    c1
    d1 --> d2
    d1 --> d3
  end
",
        &[
            ("a1", 0.0, 69.0),
            ("a2", 176.0, 69.0),
            ("a3", 352.0, 69.0),
            ("b1", 176.0, 0.0),
            ("b2", 352.0, 0.0),
            ("c1", 0.0, 0.0),
            ("d1", 0.0, 185.5),
            ("d2", 212.0, 138.0),
            ("d3", 212.0, 233.0),
        ],
    );
}

#[test]
fn a_containers_ports_start_in_the_order_elk_makes_them() {
    // C3's port for `C3 --> c0n0` (an edge to C3 itself, which mermaid turns
    // round) is made as ELK imports the edge, before the one for
    // `c1n0 --> c3n0` that `CompoundGraphPreprocessor` adds: listed clockwise,
    // the west side then has the crossing one on top, so C2's initial order is
    // free of crossings and kept (C3 left of c2n0) (mermaid 12.3.0).
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
    end
  end
  x0
  c1n0 --> C2
  C3 --> c0n0
  c2n0 --> x0
  c1n0 --> c3n0
",
        &[
            ("c0n0", 226.0, 24.0),
            ("c1n0", 0.0, 0.0),
            ("c2n0", 226.0, 313.0),
            ("c3n0", 0.0, 289.0),
            ("x0", 226.0, 432.0),
        ],
    );
}

#[test]
fn a_containers_unconnected_ports_are_made_breadth_first() {
    // C6's ports are all for edges to C6 itself, made as ELK imports the
    // edges, those of the root graph (`c2n0 --> C6`) before those of C5
    // (`c5n1 --> C6`, twice) (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
      c1n1
      c1n2
      c1n0 --> c1n1
    end
  end
  subgraph C2
    c2n0
    subgraph C3
      c3n0
      subgraph C4
        c4n0
        c4n1
        c4n0 --> c4n1
      end
    end
  end
  subgraph C5
    c5n0
    c5n1
    subgraph C6
      c6n0
      subgraph C7
        c7n0
      end
    end
    c5n0 --> c5n1
  end
  x0
  C4 --> c5n0
  c5n1 --> C6
  c2n0 --> C6
  c5n1 --> C6
",
        &[
            ("c0n0", 98.0, 800.0),
            ("c0n1", 98.0, 443.0),
            ("c1n0", 10.0, 681.0),
            ("c1n1", 186.0, 681.0),
            ("c1n2", 10.0, 586.0),
            ("c2n0", 224.0, 286.0),
            ("c3n0", 88.0, 0.0),
            ("c4n0", 0.0, 143.0),
            ("c4n1", 176.0, 143.0),
            ("c5n0", 494.0, 144.85),
            ("c5n1", 670.0, 144.85),
            ("c6n0", 930.0, 302.05),
            ("c7n0", 930.0, 183.05),
            ("x0", 98.0, 909.0),
        ],
    );
}

#[test]
fn a_containers_east_ports_start_in_the_order_elk_makes_them() {
    // The east side of a container's port list, too, starts in the order ELK
    // makes the ports (not edge by edge) (mermaid 12.3.0).
    expect_centres(
        "flowchart BT
  subgraph C0
    c0n0
    subgraph C1
      c1n0
      subgraph C2
        c2n0
        c2n1
      end
    end
  end
  subgraph C3
    c3n0
    subgraph C4
      c4n0
    end
  end
  subgraph C5
    c5n0
  end
  x0
  x1
  c0n0 -->|x| c5n0
  x1 --> C2
  c1n0 --> C3
  c1n0 --> x0
  C2 --> C3
",
        &[
            ("c0n0", 893.17, 436.0),
            ("c1n0", 667.17, 484.0),
            ("c2n0", 239.17, 532.0),
            ("c2n1", 441.17, 532.0),
            ("c3n0", 393.5, 203.0),
            ("c4n0", 167.5, 215.0),
            ("c5n0", 0.0, 0.0),
            ("x0", 688.9, 263.0),
            ("x1", 340.17, 709.0),
        ],
    );
}

#[test]
fn external_port_dummies_of_a_childs_ports_follow_their_import_order() {
    // In C3, the dummies of `c1n0 --> c3n1` and `c2n0 --> c3n1` follow
    // c3n1's ports, made as ELK imports the edges: the first edge is the
    // root graph's, so it comes first, though the second comes earlier in
    // the source (mermaid 12.3.0).
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
    c2n1
    c2n2
    subgraph C3
      c3n0
      c3n1
      c3n0 --> c3n1
    end
    c2n0 --> c2n1
    c2n1 --> c2n2
  end
  x0
  x1
  c1n0 --> c2n1
  c2n0 --> c3n1
  c2n1 --> C3
  c1n0 -->|a label| c3n1
  c2n2 --> C0
  C2 --> c0n0
",
        &[
            ("c0n0", 175.6, 24.0),
            ("c1n0", 401.6, 0.0),
            ("c2n0", 292.27, 352.0),
            ("c2n1", 55.37, 457.0),
            ("c2n2", 0.0, 572.0),
            ("c3n0", 226.0, 620.0),
            ("c3n1", 288.5, 725.0),
            ("x0", 641.6, 16.0),
            ("x1", 833.6, 16.0),
        ],
    );
}

#[test]
fn cycle_breaking_leaves_a_container_by_its_ports_in_their_made_order() {
    // C1's ports for `c1n1 --> c0n1` and `c1n0 --> c0n0` are made in the
    // order of their dummies inside C1 (c1n0's first), so the depth-first
    // cycle breaker in C0 reaches c0n0 first and turns `c0n1 --> c0n0`
    // round (mermaid 12.3.0).
    expect_centres(
        "flowchart BT
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
      c1n1
      c1n0 --> c1n1
    end
    c0n0 --> c0n1
  end
  c0n0 --> c0n1
  c1n1 --> c0n1
  c1n0 --> c0n0
  c0n1 --> c0n0
",
        &[
            ("c0n0", 0.0, 105.0),
            ("c0n1", 13.04, 0.0),
            ("c1n0", 44.0, 373.0),
            ("c1n1", 88.5, 268.0),
        ],
    );
}

#[test]
fn a_containers_port_without_edges_outside_holds_its_ports_in_their_made_order() {
    // C0's ports, as made: `C0 --> x0`'s (imported), `c0n0 --> C0`'s (an edge
    // from inside, so none at the root) and `c0n0 --> x0`'s (added by
    // `CompoundGraphPreprocessor`). Sorting by model order would put the
    // earlier edge `c0n0 --> x0` first, but the edgeless port between them
    // compares as smaller than either and either as smaller than it, so the
    // insertion sort leaves the list as made: `C0 --> x0`'s label goes on top
    // and the two edges cross in the channel before the labels, which takes
    // a second lane (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
  end
  x0
  x1
  x0 --> x1
  C0 -->|a label| x0
  c0n0 --> C0
  c0n0 -->|yes| x0
",
        &[("c0n0", 0.0, 0.0), ("x0", 387.27, 6.0), ("x1", 579.27, 6.0)],
    );
}

#[test]
fn a_node_tied_only_to_ports_where_edges_leave_goes_past_a_label_layer() {
    // In C0, C1 meets nothing but the port where `c2n2 --> c3n0` leaves, so
    // ELK layers it on its own and moves it to the last layer: that of the
    // label of `c0n1 --> C0` (an edge to C0's own port, tied to c0n1 only),
    // below c0n1 rather than beside it (mermaid 12.3.0).
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
    c0n1
    subgraph C1
      c1n0
      subgraph C2
        c2n0
        c2n1
        c2n2
        c2n1 --> c2n2
      end
    end
    c0n0 --> c0n1
  end
  subgraph C3
    c3n0
    c3n1
  end
  subgraph C4
    c4n0
    c4n1
    c4n0 --> c4n1
  end
  x0
  x1
  c2n2 -->|x| c3n0
  c0n1 -->|a label| C0
",
        &[
            ("c0n0", 887.0, 0.0),
            ("c0n1", 887.0, 69.0),
            ("c1n0", 264.0, 261.5),
            ("c2n0", 490.0, 234.0),
            ("c2n1", 692.0, 234.0),
            ("c2n2", 692.0, 303.0),
            ("c3n0", 692.0, 609.0),
            ("c3n1", 490.0, 609.0),
            ("c4n0", 0.0, 156.0),
            ("c4n1", 0.0, 225.0),
            ("x0", 1103.0, 178.5),
            ("x1", 1295.0, 178.5),
        ],
    );
}

#[test]
fn an_edgeless_port_on_a_containers_west_side_splits_its_sort() {
    // C2's west ports, as made: `c0n0 --> C2`'s, `C2 --> C3`'s (an edge to
    // something inside, so none at the root) and `c0n2 --> c3n1`'s. The
    // edgeless one keeps the other two in their made order, listed up the
    // west side, so they start crossing the edges from C0; the sweep that
    // undoes it puts c2n0 above C3 (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
    c0n2
    subgraph C1
      c1n0
    end
    c0n0 --> c0n1
  end
  subgraph C2
    c2n0
    subgraph C3
      c3n0
      c3n1
      c3n0 --> c3n1
    end
  end
  subgraph C4
    c4n0
    c4n1
    subgraph C5
      c5n0
      subgraph C6
        c6n0
        c6n1
      end
    end
    c4n0 --> c4n1
  end
  x0
  c0n0 --> C2
  c6n1 --> x0
  c0n2 --> c3n1
  C3 --> C6
  C2 --> C3
",
        &[
            ("c0n0", 24.0, 241.38),
            ("c0n1", 236.0, 259.12),
            ("c0n2", 236.0, 354.12),
            ("c1n0", 0.0, 384.38),
            ("c2n0", 631.0, 156.0),
            ("c3n0", 530.0, 299.0),
            ("c3n1", 742.0, 316.25),
            ("c4n0", 1142.0, 0.0),
            ("c4n1", 1328.0, 0.0),
            ("c5n0", 1089.0, 143.0),
            ("c6n0", 1084.0, 286.0),
            ("c6n1", 1084.0, 381.0),
            ("x0", 1554.0, 381.0),
        ],
    );
}

#[test]
fn a_crossed_subgraph_inside_a_directed_one_is_a_run_of_its_own() {
    // `b00n0 --> c00n0` makes B00 `INCLUDE_CHILDREN`, but its parent A0 has a
    // direction of its own (`SEPARATE_CHILDREN`): ELK lays B00 out as the root
    // of a compound run of its own, with ELK's default direction (RIGHT), not
    // A0's BT (mermaid 12.3.0).
    expect_centres(
        "flowchart BT
  subgraph A0
    direction BT
    subgraph B00
      b00n0
      subgraph C00
        c00n0
      end
    end
  end
  b00n0 --> c00n0
  b00n0 --> C00
",
        &[("b00n0", 0.0, 6.0), ("c00n0", 246.0, 0.0)],
    );
}

#[test]
fn a_crossed_subgraphs_own_run_is_not_split_into_components() {
    // B00 and B10 are compound runs of their own (an edge from each one's
    // child into its subgraph C), laid out whole (`doCompoundLayout` packs no
    // components): b00n1 stays in B00's layering rather than packed apart
    // (mermaid 12.3.0).
    expect_centres(
        "flowchart TD
  subgraph A0
    direction LR
    subgraph B00
      b00n0
      b00n1
      subgraph C00
        c00n0
        c00n1
        c00n0 --> c00n1
      end
    end
  end
  subgraph A1
    direction LR
    a1n0
    subgraph B10
      b10n0
      subgraph C10
        c10n0
      end
    end
  end
  c00n1 --> b00n0
  b00n0 --> c00n0
  b10n0 --> c10n0
  b10n0 --> c10n0
",
        &[
            ("a1n0", 0.0, 0.0),
            ("b00n0", 1124.0, 172.75),
            ("b00n1", 777.0, 22.5),
            ("b10n0", 24.0, 165.0),
            ("c00n0", 666.0, 172.75),
            ("c00n1", 878.0, 190.5),
            ("c10n0", 330.0, 165.0),
        ],
    );
}

#[test]
fn a_loop_on_a_container_with_port_dummies_goes_round_it() {
    // A has port dummies inside (`A --> a1`, `a2 --> A`), so its ports keep
    // their sides: `A --> A` leaves at the bottom and comes back round to the
    // top, 20 off each, which takes room in the layer gap above A (and A's
    // ports get unconnected dummies inside, beside a1's) (mermaid 12.3.0).
    expect_centres(
        "flowchart TB
subgraph A
a1 --> a2
end
start --> A
A --> A
A --> a1
a2 --> A
",
        &[("a1", 0.0, 153.0), ("a2", 0.0, 222.0), ("start", 12.0, 0.0)],
    );
}

#[test]
fn a_containers_loop_stands_off_its_crossing_edges_ports() {
    // C0's ports for `c0n0 --> x0` (crossing its border) stand 10 outside it;
    // the loop's leg below stands 20 beyond those (mermaid 12.3.0).
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
  end
  x0
  C0 --> x0
  c0n0 --> x0
  c0n0 -->|yes| x0
  C0 --> C0
",
        &[("c0n0", 21.73, 0.0), ("x0", 0.0, 220.0)],
    );
}

/// A container's frame `(x, y, w, h)` and the extent `(min x, max x, min y,
/// max y)` of the path of its `n`th loop, from our SVG.
type Extent = (f64, f64, f64, f64);
fn frame_and_loop(src: &str, c: &str, n: usize) -> (Extent, Extent) {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let frame = frame_of(&svg, c);
    let key = format!("<g id=\"L-{c}-{c}-");
    let at = svg.match_indices(&key).nth(n).expect("loop").0;
    let d = at + svg[at..].find(" d=\"").expect("path") + 4;
    let d = &svg[d..d + svg[d..].find('"').expect("path end")];
    let nums: Vec<f64> = d
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().expect("number"))
        .collect();
    let (xs, ys): (Vec<f64>, Vec<f64>) = nums.chunks(2).map(|p| (p[0], p[1])).unzip();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (frame, (min(&xs), max(&xs), min(&ys), max(&ys)))
}

/// A container's frame `(x, y, w, h)` in our SVG.
fn frame_of(svg: &str, c: &str) -> Extent {
    let at = svg
        .find(&format!("<g id=\"cluster-{c}\" class=\"cluster\">"))
        .expect("cluster");
    let rect = at + svg[at..].find("<rect x=").expect("frame");
    let tag = &svg[rect..rect + svg[rect..].find('>').expect("tag end")];
    (
        attr(tag, "x"),
        attr(tag, "y"),
        attr(tag, "width"),
        attr(tag, "height"),
    )
}

/// A container's frame `(x, y, w, h)` as we render `src`.
fn frame(src: &str, c: &str) -> Extent {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    frame_of(&output::encode_svg(&r, &EncodeOptions::default()), c)
}

#[test]
fn a_containers_loop_goes_round_the_side_passing_fewer_ports() {
    // Neither way round C0 passes a port (its port for `c0n0 --> x0` stands
    // on the side the loop leaves from, before it): ELK's `RoutingDirector`
    // then goes clockwise, from the bottom round the right to the top
    // (mermaid 12.3.0: 114.9,390 -> 114.9,410 -> 256,410 -> 256,243 ->
    // 169.7,243 -> 169.7,273 round C0 at 36,273 200x117).
    let ((x, y, w, h), (lx, hx, ly, hy)) = frame_and_loop(
        "flowchart TD
  subgraph C0
    c0n0
  end
  x0
  x1
  x0 --> x1
  x1 -->|x| c0n0
  c0n0 --> x0
  C0 --> C0
",
        "C0",
        0,
    );
    assert!(lx > x - 0.5, "{lx} {x}");
    assert!((hx - (x + w + 20.0)).abs() < 0.5, "{hx} {x} {w}");
    assert!((ly - (y - 30.0)).abs() < 0.5, "{ly} {y}");
    assert!((hy - (y + h + 20.0)).abs() < 0.5, "{hy} {y} {h}");
}

#[test]
fn a_containers_loop_counts_its_ports_in_the_order_they_were_made() {
    // Nothing sweeps the root graph (its model order has no crossing), so
    // C0's ports stay as made: `C0 --> x0`'s, the loop's, then those of the
    // edges crossing C0's border. Round the right the loop would pass those
    // two (penalty 6), round the left `C0 --> x0`'s (3) (mermaid 12.3.0:
    // round the left, 20 off C0 at 32,32 200x117).
    let ((x, _, w, _), (lx, hx, _, _)) = frame_and_loop(
        "flowchart TD
  subgraph C0
    c0n0
  end
  x0
  C0 --> x0
  c0n0 --> x0
  c0n0 -->|yes| x0
  C0 --> C0
",
        "C0",
        0,
    );
    assert!((lx - (x - 20.0)).abs() < 0.5, "{lx} {x}");
    assert!(hx < x + w, "{hx} {x} {w}");
}

#[test]
fn a_containers_ports_without_edges_outside_keep_their_order_inside() {
    // C0's ports for its edges into itself have no edges outside: the port
    // distributor keeps them as `PortListSorter` left them, and their dummies
    // inside follow, the loop's port on the top side third from the left of
    // four. Round the left the loop passes one, round the right two (mermaid
    // 12.3.0: round the left, 20 off C0 at 32,32 267.3x327).
    let ((x, _, w, _), (lx, hx, _, _)) = frame_and_loop(
        "flowchart BT
  subgraph C0
    c0n0
    c0n1
    c0n2
    c0n0 --> c0n1
    c0n1 --> c0n2
  end
  C0 --> c0n1
  C0 --> c0n2
  C0 --> c0n1
  c0n2 --> c0n0
  C0 --> C0
",
        "C0",
        0,
    );
    assert!((lx - (x - 20.0)).abs() < 0.5, "{lx} {x}");
    assert!(hx < x + w, "{hx} {x} {w}");
}

#[test]
fn loops_in_a_containers_graph_stand_off_by_its_spacing() {
    // In P's graph (`spacing.baseValue` 24) C's first loop stands 12 off it
    // (`spacing.nodeSelfLoop`), the second 20 beyond (`spacing.edgeEdge`)
    // (mermaid 12.3.0: legs at 56 and 36, C at 68).
    let src = "flowchart TD
  subgraph P
    subgraph C
      a
    end
    b
  end
  C --> C
  C --> C
";
    let ((x, _, _, _), (lx, _, _, _)) = frame_and_loop(src, "C", 0);
    assert!((lx - (x - 12.0)).abs() < 0.5, "{lx} {x}");
    let ((x, _, _, _), (lx, _, _, _)) = frame_and_loop(src, "C", 1);
    assert!((lx - (x - 32.0)).abs() < 0.5, "{lx} {x}");
}

#[test]
fn a_container_with_only_a_loop_and_port_edges_is_pinned_like_elk() {
    // C1's loop is no edge of C0's graph when ELK layers it: C1, with edges
    // only to C0's border, goes next to C0's port dummies, after c0n0
    // (mermaid 12.3.0).
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
    c2n1
    subgraph C3
      c3n0
    end
    c2n0 --> c2n1
  end
  x0
  c1n0 --> c3n0
  x0 --> C3
  c1n0 --> x0
  C2 -->|x| x0
  c2n1 -->|a label| x0
  C1 --> C1
  C3 --> C3
",
        &[
            ("c0n0", 78.0, 0.0),
            ("c1n0", 304.0, 7.0),
            ("c2n0", 0.0, 324.0),
            ("x0", 154.78, 605.0),
        ],
    );
}

#[test]
fn a_containers_unconnected_ports_keep_the_order_its_contents_gave_them() {
    // The forward sweep diving into C0 orders its east ports by their dummies
    // there: `c0n0 --> C0`'s (beside c0n0) first, then the loops' (without a
    // barycentre, kept after it). The backward sweep takes that order back
    // into C0, so the loops' dummies stand below c0n0's and C0 grows for
    // them (mermaid 12.3.0: C0 260x225.5).
    let (_, _, w, h) = frame(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
  end
  x0
  c0n0 --> C0
  x0 --> c0n0
  C0 --> c0n0
  x1 --> c0n1
  C0 -->|lab0| C0
  C0 --> C0
  C0 --> C0
",
        "C0",
    );
    assert!(
        (w - 260.0).abs() < 0.5 && (h - 225.5).abs() < 0.5,
        "{w} {h}"
    );
}

#[test]
fn a_containers_model_order_sorts_its_ports_as_port_list_sorter_left_them() {
    // `PortListSorter` turns C0's west side round (but for its last port)
    // before the model order sorts it: x0's and x1's ports end up side by
    // side, no loop port between them, and sort by their sources. The model
    // order has no crossing, nothing is swept, and C0's port dummies keep
    // the order they were made in: the loops' after x1's, below c0n0
    // (mermaid 12.3.0: C0 200x142.5).
    let (_, _, w, h) = frame(
        "flowchart RL
  subgraph C0
    c0n0
  end
  x0
  x1 --> c0n0
  x0 --> C0
  C0 --> C0
  C0 -->|lab1| C0
  C0 -->|lab2| C0
",
        "C0",
    );
    assert!(
        (w - 200.0).abs() < 0.5 && (h - 142.5).abs() < 0.5,
        "{w} {h}"
    );
}

#[test]
fn a_containers_margin_ends_at_its_outer_loops_label() {
    // The outer loop's label (inline, on its leg) is the last thing round
    // C0: its margin ends there, without `spacing.edgeLabel` beyond it, and
    // x0 (a component of its own) stands that much nearer (mermaid 12.3.0).
    expect_centres(
        "flowchart BT
  subgraph C0
    c0n0
    c0n1
    c0n0 --> c0n1
  end
  x0
  C0 --> c0n0
  C0 --> C0
  C0 -->|lab1| C0
  C0 --> C0
",
        &[
            ("c0n0", 0.0, 69.0),
            ("c0n1", 0.0, 0.0),
            ("x0", 282.47, 22.5),
        ],
    );
    // So does a reversed edge's dummy below C0 (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
  end
  x0
  C0 --> x0
  C0 --> c0n0
  x0 --> C0
  c0n1 --> x1
  C0 -->|lab0| C0
  C0 --> C0
",
        &[
            ("c0n0", 0.0, 95.0),
            ("c0n1", 0.0, 0.0),
            ("x0", 266.0, 231.83),
            ("x1", 266.0, 0.0),
        ],
    );
}

#[test]
fn loop_labels_in_a_containers_graph_keep_its_edge_label_spacing() {
    // In A's graph (`spacing.baseValue` 24) the loop beyond lab0's stands
    // `spacing.edgeLabel` 2.4 (not the root's 4) past the label, and A
    // holds it (mermaid 12.3.0: A 529.84 wide).
    let (_, _, w, _) = frame(
        "flowchart BT
  subgraph A
    subgraph C0
      c0n0
    end
    a1
  end
  x0
  c0n0 --> x0
  C0 --> x0
  c0n0 --> C0
  x0 --> C0
  C0 -->|lab0| C0
  C0 -->|lab1| C0
",
        "A",
    );
    assert!((w - 529.84).abs() < 0.5, "{w}");
}

#[test]
fn a_loop_label_in_a_containers_graph_stands_its_edge_label_spacing_off() {
    // D0's loop in C0's graph: its label stands `spacing.edgeLabel` 2.4 off
    // the loop's leg, and C0 holds it (mermaid 12.3.0: C0 283.4 high).
    let (_, _, _, h) = frame(
        "flowchart RL
  subgraph C0
    c0n0
    c0n1
    c0n2
    c0n0 --> c0n1
    subgraph D0
      d0n0
    end
  end
  x0
  C0 -->|lab1| x0
  c0n1 --> D0
  c0n1 -->|lab2| D0
  D0 -->|lab3| D0
  C0 --> C0
  C0 --> C0
",
        "C0",
    );
    assert!((h - 283.4).abs() < 0.5, "{h}");
}

#[test]
fn a_containers_loops_take_the_lowest_free_slot_on_each_side_with_ports() {
    // Two loops round C0's high side, one round its low side: on the low
    // and high sides they nest by their routing slots, but on the sides with
    // ports each moves in to the lowest slot free at the ports it spans
    // (`RoutingSlotAssigner.shiftTowardsNode`), and C0's margins hold the
    // outermost legs there (mermaid 12.3.0).
    expect_centres(
        "flowchart TD
  subgraph C0
    c0n0
  end
  x0
  x2 --> C0
  C0 --> x0
  c0n0 -->|lab1| x0
  x2 -->|lab2| x1
  C0 -->|lab3| C0
  C0 --> C0
  C0 --> C0
",
        &[
            ("c0n0", 0.0, 173.0),
            ("x0", 6.0, 457.0),
            ("x2", 93.98, 0.0),
            ("x1", 219.97, 372.0),
        ],
    );
    // Round opposite sides, two loops leave from C0's ports apart but come
    // back to crossing ones: they take different slots there, and C0's
    // margin holds both (mermaid 12.3.0).
    expect_centres(
        "flowchart LR
  subgraph C0
    c0n0
    c0n1
    c0n2
  end
  x0
  c0n0 --> c0n1
  c0n0 -->|lab1| x1
  C0 --> c0n2
  C0 --> C0
  C0 --> C0
",
        &[
            ("c0n0", 0.0, 95.0),
            ("c0n1", 212.0, 112.75),
            ("c0n2", 0.0, 0.0),
            ("x0", 101.0, 241.75),
            ("x1", 524.47, 77.25),
        ],
    );
}

#[test]
fn a_label_dummy_left_by_a_hidden_port_edge_keeps_its_layer() {
    // `d0n0 -->|lab1| C0` runs back to C0's port in its first layer; with
    // that port hidden for the layering its label dummy, tied to D0 alone,
    // takes a layer of its own above D0. c0n0 (joined only to ports where
    // edges leave) then moves to the last layer, and the label's layer stays
    // (`LayerConstraintPostprocessor` drops only empty layers): C0 is
    // 462x304, the label above D0, not beside it (mermaid 12.3.0).
    let (_, _, w, h) = frame(
        "flowchart TD
  subgraph C0
    c0n0
    subgraph D0
      d0n0
    end
  end
  x0
  C0 --> x0
  x0 --> c0n0
  d0n0 -->|lab1| C0
",
        "C0",
    );
    assert!(
        (w - 462.0).abs() < 0.5 && (h - 304.0).abs() < 0.5,
        "{w}x{h}"
    );
}

#[test]
fn a_ports_last_layer_comes_after_a_label_layer_beyond_all_nodes() {
    // `C0 -->|lab3| cb0` runs back from CB to C0's port in its last layer,
    // its label dummy in a layer after CB, the last of C0's nodes: the port
    // comes after that layer, so the label stays in it, beside nothing
    // (mermaid 12.3.0: C0 783.4x516, CB at x 559.4).
    let (_, _, w, h) = frame(
        "flowchart TD
  subgraph C0
    c0
    subgraph CA
      ca0
    end
    subgraph CB
      cb0
    end
  end
  C0 -->|lab1| ca0
  cb0 -->|lab2| C0
  C0 -->|lab3| cb0
",
        "C0",
    );
    assert!(
        (w - 783.44).abs() < 0.5 && (h - 516.0).abs() < 0.5,
        "{w}x{h}"
    );
}

#[test]
fn an_inverted_ports_dummy_left_by_a_switched_label_has_no_size() {
    // `B -->|lab| ba0` leaves BA backwards; `LabelDummySwitcher` moves the
    // label into BA's layer and the inverted port's dummy into the label's,
    // beside BB: a long-edge dummy, which stands the edge spacing (12) off
    // BB, not the node spacing of a label (mermaid 12.3.0: B 754x379).
    let (_, _, w, h) = frame(
        "flowchart LR
  subgraph B
    b0
    subgraph BA
      ba0
    end
    subgraph BB
      bb0
    end
  end
  x0
  B -->|lab| ba0
  b0 --> ba0
  BB --> x0
",
        "B",
    );
    assert!(
        (w - 754.0).abs() < 0.5 && (h - 379.0).abs() < 0.5,
        "{w}x{h}"
    );
}

#[test]
fn a_containers_ports_for_inverted_edges_are_made_as_its_contents_made_them() {
    // Both edges leave C0's port dummies backwards into C0A, through
    // inverted ports whose dummies stand in C0A's layer. They cross C0A's
    // border, so C0A's ports are made in the order C0A's own port dummies
    // were (c0an0's first), not in edge order: the dummies follow them and
    // the initial order has no crossing, which ELK keeps. Both edges come
    // round C0A's right (mermaid 12.3.0: C0 482x241).
    let (_, _, w, h) = frame(
        "flowchart TD
  subgraph C0
    subgraph C0A
      c0an0
      c0an1
    end
  end
  C0 --> c0an1
  C0 --> c0an0
",
        "C0",
    );
    assert!(
        (w - 482.0).abs() < 0.5 && (h - 241.0).abs() < 0.5,
        "{w}x{h}"
    );
}

/// The smallest and largest x of the edge `L-{from}-{to}-*` in our SVG.
fn edge_x_range(svg: &str, from: &str, to: &str) -> (f64, f64) {
    let key = format!("<g id=\"L-{from}-{to}-");
    let at = svg.find(&key).expect("edge group");
    let d = at + svg[at..].find(" d=\"").expect("path") + 4;
    let end = d + svg[d..].find('"').expect("closing quote");
    let nums: Vec<f64> = svg[d..end]
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().expect("number"))
        .collect();
    let xs = nums.iter().step_by(2);
    (
        xs.clone().copied().fold(f64::INFINITY, f64::min),
        xs.copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

#[test]
fn inverted_ports_feed_their_channel_in_their_containers_port_list_order() {
    // Both edges leave C0's east side back into C0A through inverted ports on
    // C0A's west side; their legs before C0A tie in that channel, and ELK
    // makes the segments in C0A's port list order (clockwise: bottom up there,
    // c0aan0's first), not top down by position: c0an1's leg goes outside
    // (mermaid 12.3.0: 12px left of c0aan0's).
    let src = "flowchart LR
  subgraph C0
    c0n0
    c0n1
    subgraph C0A
      c0an0
      c0an1
      subgraph C0AA
        c0aan0
      end
    end
  end
  x0
  C0 -->|lab1| c0aan0
  c0an0 --> C0
  c0an1 -->|lab2| C0
  c0an0 --> x0
  C0AA --> C0
  C0 --> c0an1
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let (deep, _) = edge_x_range(&svg, "C0", "c0aan0");
    let (near, _) = edge_x_range(&svg, "C0", "c0an1");
    assert!((deep - near - 12.0).abs() < 0.5, "{deep} {near}");
}

#[test]
fn an_inverted_ports_dummy_block_straightens_towards_its_in_layer_edge() {
    // `C0 --> c0an0` leaves C0's bottom and runs back up beside C0A to its
    // inverted port on top. Laid out leftwards and upwards, the block of the
    // port's dummy and C0's port dummy holds only dummies (ELK's `od` never
    // looks at the block's root), so BK's threshold strategy takes the
    // in-layer edge to C0A into account and moves the block up against C0A:
    // the balanced layout keeps the edge's leg the edge-node spacing right
    // of C0A (mermaid 12.3.0), not midway to the next dummy.
    let src = "flowchart TD
  subgraph C0
    c0n0
    c0n1
    subgraph C0A
      c0an0
    end
  end
  x0
  C0 --> c0an0
  c0an0 --> C0
  c0an0 --> C0
  x1 --> c0n1
  x1 --> x0
  c0n1 --> C0A
  x1 --> c0n0
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let (x, _, w, _) = frame_of(&svg, "C0A");
    let (_, leg) = edge_x_range(&svg, "C0", "c0an0");
    assert!((leg - (x + w) - 12.0).abs() < 0.5, "{leg} {}", x + w);
}

/// The first `n` points of the edge `L-{from}-{to}-*`'s path in our SVG.
fn edge_start(svg: &str, from: &str, to: &str, n: usize) -> Vec<(f64, f64)> {
    let key = format!("<g id=\"L-{from}-{to}-");
    let at = svg.find(&key).expect("edge group");
    let d = at + svg[at..].find(" d=\"").expect("path") + 4;
    let end = d + svg[d..].find('"').expect("closing quote");
    let nums: Vec<f64> = svg[d..end]
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().expect("number"))
        .collect();
    nums.chunks(2).take(n).map(|p| (p[0], p[1])).collect()
}

#[test]
fn a_shapes_port_beside_its_outline_point_lets_the_far_step_straighten() {
    // `n10` is a stadium whose width the browser measures in single
    // precision: ELK's port for `n10 --> n11` stands a hair off the outline
    // mermaid clips at, so mermaid's route keeps both, and with that fifth
    // point the step at the far end straightens (`straightenEdgeTerminals`),
    // pulling the channel onto the target's row: the edge rises at once
    // (mermaid 12.3.0), not after a 30px run.
    let src = "---
config:
  layout: elk
---
swimlane-beta LR
  subgraph L0[Lane 0]
    n00[Approve]
    n01{Submit build}
  end
  subgraph L1[Lane 1]
    n10([Pay])
    n11(Build check)
    n12([Pay])
    subgraph S1[Group 1]
      s1[Submit]
    end
  end
  s1 --> n01
  n00 --> n01
  n10 --> n11
  n00 --> s1
  n12 --> n10
  n12 --> n11
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let p = edge_start(&svg, "n10", "n11", 2);
    assert!((p[0].0 - p[1].0).abs() < 0.01, "{p:?}");
    assert!((p[0].1 - p[1].1).abs() > 5.0, "{p:?}");
}

#[test]
fn a_loop_on_a_container_does_not_count_for_its_alignment_in_the_layer() {
    // C0AA and C0AB share a layer, which C0AB's port for `c0abn0 --> C0`
    // widens. C0AA's only edge leaves it, so ELK aligns it to the side its
    // edges leave from (`placeNodesHorizontally`); its loop, taken off its
    // ports while the layer is placed, does not count, and C0AA lines up
    // with C0AB (mermaid 12.3.0) instead of a third of the way back.
    let src = "flowchart BT
  subgraph C0
    c0n0
    subgraph C0A
      c0an0
      subgraph C0AA
        c0aan0
        c0aan1
      end
      subgraph C0AB
        c0abn0
        c0abn1
      end
    end
  end
  x0
  C0AA --> C0
  c0abn0 --> C0
  C0AA -->|lab1| C0AA
";
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let (_, a, _, _) = frame_of(&svg, "C0AA");
    let (_, b, _, _) = frame_of(&svg, "C0AB");
    assert!((a - b).abs() < 0.01, "{a} {b}");
}

#[test]
fn a_loops_label_across_a_horizontal_layout_is_no_thicker_than_itself() {
    // C0A's loop goes round its high side with its label on its leg; the
    // label is the last thing in C0, which ends there. A label dummy is the
    // edge's thickness taller than its label across a horizontal layout,
    // but a loop's label has none (mermaid 12.3.0 draws C0's frame 262.5
    // high).
    let (_, _, _, h) = frame(
        "flowchart RL
  subgraph C0
    c0n0
    subgraph C0A
      c0an0
    end
  end
  x0
  C0 --> c0an0
  x0 --> C0A
  c0an0 --> x0
  c0n0 --> c0an0
  C0A -->|lab1| C0A
  C0 --> C0
",
        "C0",
    );
    assert!((h - 262.5).abs() < 0.01, "{h}");
}
