//! The neo / redux theme family: palette-slot assignment order (mirroring upstream's
//! `colorIndex`), per-diagram theme/look resolution, and end-to-end colour output.

use mmdc::config::MermaidConfig;
use mmdc::diagram::{class, er, flowchart, requirement, state};
use mmdc::render;
use mmdc::scene::{Element, Scene};
use mmdc::theme::{ColorSlot, ThemeName};

fn slots(nodes: impl Iterator<Item = Option<ColorSlot>>) -> Vec<Option<usize>> {
    nodes.map(|s| s.map(|s| s.0)).collect()
}

#[test]
fn flowchart_subgraphs_take_slots_in_declaration_order() {
    // flowDb assigns `declarationIndex` by a pre-order walk: Outer before its children,
    // even though the grammar closes InnerOne/InnerTwo first. Nodes get no slot.
    let g = flowchart::parse(
        "flowchart TD\nsubgraph Outer\n subgraph InnerOne\n a\n end\n subgraph InnerTwo\n b\n end\nend\nsubgraph Sibling\n c\nend",
        None,
    )
    .unwrap();
    let ids: Vec<_> = g.clusters.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["Outer", "InnerOne", "InnerTwo", "Sibling"]);
    assert_eq!(
        slots(g.clusters.iter().map(|c| c.color_slot)),
        [Some(0), Some(1), Some(2), Some(3)]
    );
    assert!(g.nodes.iter().all(|n| n.color_slot.is_none()));
}

#[test]
fn er_entities_take_slots_in_first_mention_order() {
    let d = er::parse(
        "erDiagram\nCUSTOMER ||--o{ ORDER : places\nORDER ||--|{ ITEM : contains\nPRODUCT { string code }",
        None,
    )
    .unwrap();
    let g = er::to_graph(&d);
    let named: Vec<_> = g
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.color_slot.map(|s| s.0)))
        .collect();
    assert_eq!(
        named,
        [
            ("CUSTOMER", Some(0)),
            ("ORDER", Some(1)),
            ("ITEM", Some(2)),
            ("PRODUCT", Some(3))
        ]
    );
}

#[test]
fn class_notes_and_namespaces_do_not_consume_slots() {
    let g = class::parse(
        "classDiagram\nnote \"a note\"\nnamespace Zoo {\n class Animal\n}\nclass Duck\nAnimal <|-- Duck",
        None,
    )
    .unwrap();
    let classes: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.id == "Animal" || n.id == "Duck")
        .map(|n| (n.id.as_str(), n.color_slot.map(|s| s.0)))
        .collect();
    assert_eq!(classes, [("Animal", Some(0)), ("Duck", Some(1))]);
    assert!(
        g.nodes
            .iter()
            .filter(|n| n.id.starts_with("note"))
            .all(|n| n.color_slot.is_none())
    );
    assert!(g.clusters.iter().all(|c| c.color_slot.is_none()));
}

#[test]
fn requirements_number_before_elements() {
    // requirementDb.getData: all requirements, then all elements (`nodes.length`).
    let d = requirement::parse(
        "requirementDiagram\nelement e1 {\ntype: sim\n}\nrequirement r1 {\nid: 1\n}\nelement e2 {\ntype: sim\n}\nrequirement r2 {\nid: 2\n}",
        None,
    )
    .unwrap();
    let g = requirement::to_graph(&d);
    let named: Vec<_> = g
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.color_slot.map(|s| s.0)))
        .collect();
    assert_eq!(
        named,
        [
            ("e1", Some(2)),
            ("r1", Some(0)),
            ("e2", Some(3)),
            ("r2", Some(1))
        ]
    );
}

#[test]
fn state_composites_take_slots_states_do_not() {
    let g = state::parse(
        "stateDiagram-v2\n[*] --> A\nstate A {\n x --> y\n state B {\n  z\n }\n}\nstate C {\n w\n}",
        None,
    )
    .unwrap();
    let named: Vec<_> = g
        .clusters
        .iter()
        .map(|c| (c.id.as_str(), c.color_slot.map(|s| s.0)))
        .collect();
    assert_eq!(named, [("A", Some(0)), ("B", Some(1)), ("C", Some(2))]);
    assert!(g.nodes.iter().all(|n| n.color_slot.is_none()));
}

/// Fill colours of every node's outline (first non-shadow shape of each `node` group).
fn node_fills(scene: &Scene) -> Vec<String> {
    fn walk(els: &[Element], out: &mut Vec<String>) {
        for e in els {
            if let Element::Group {
                class, children, ..
            } = e
            {
                if class.as_deref().is_some_and(|c| c.starts_with("node ")) {
                    let fill = children.iter().find_map(|c| match c {
                        Element::Shape { paint, class, .. }
                            if class.as_deref() != Some("drop-shadow") =>
                        {
                            Some(paint.fill.css().to_ascii_lowercase())
                        }
                        _ => None,
                    });
                    out.extend(fill);
                } else {
                    walk(children, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&scene.elements, &mut out);
    out
}

fn cfg(theme: Option<ThemeName>) -> MermaidConfig {
    match theme {
        Some(t) => MermaidConfig::default().with_cli_theme(t),
        None => MermaidConfig::default(),
    }
}

#[test]
fn redux_color_er_gives_every_entity_its_own_fill() {
    let src = "erDiagram\nA ||--o{ B : r\nB ||--|{ C : s\nC }|--|| D : t";
    let r = render::render(src, &cfg(Some(ThemeName::ReduxColor))).unwrap();
    let fills = node_fills(&r.scene);
    assert_eq!(fills, ["#fdf4ff", "#f0fdfa", "#fff7ed", "#ecfeff"]);
    // The classic default theme paints them all alike.
    let r = render::render(src, &cfg(Some(ThemeName::Default))).unwrap();
    let fills = node_fills(&r.scene);
    assert!(fills.iter().all(|f| f == &fills[0]));
}

#[test]
fn redux_color_flowchart_colours_subgraphs_not_nodes() {
    let src = "flowchart LR\nsubgraph one\n a --> b\nend\nsubgraph two\n c\nend\nb --> c";
    let r = render::render(src, &cfg(Some(ThemeName::ReduxColor))).unwrap();
    let svg = mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default());
    // Subgraph `one` is slot 0 (fuchsia), `two` is slot 1 (teal).
    assert!(svg.contains("#FDF4FF") && svg.contains("#E879F9"), "{svg}");
    assert!(svg.contains("#F0FDFA") && svg.contains("#2DD4BF"));
    // Nodes keep the uniform mainBkg.
    assert!(node_fills(&r.scene).iter().all(|f| f == "#ffffff"));
}

#[test]
fn mermaid_12_defaults_to_redux_color_for_six_diagram_types() {
    // No -t: flowchart / ER / class / state / sequence / requirement resolve to
    // redux-color + neo, exactly what `mmdc -i x.mmd` renders upstream.
    let er_src = "erDiagram\nA ||--o{ B : r";
    let r = render::render(er_src, &cfg(None)).unwrap();
    assert_eq!(node_fills(&r.scene), ["#fdf4ff", "#f0fdfa"]);
    assert_eq!(
        r.scene.font_family,
        "\"Recursive Variable\", arial, sans-serif"
    );
    // The neo look draws the redux drop shadow (offset 4,4, 6% black).
    let svg = mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default());
    assert!(svg.contains("opacity=\"0.06\""));

    // An explicit theme wins over the diagram default, from any layer.
    let r = render::render(er_src, &cfg(Some(ThemeName::Default))).unwrap();
    assert_eq!(node_fills(&r.scene), ["#ececff", "#ececff"]);
    let r = render::render(
        &format!("%%{{init: {{'theme':'forest'}}}}%%\n{er_src}"),
        &cfg(None),
    )
    .unwrap();
    assert_eq!(node_fills(&r.scene), ["#cde498", "#cde498"]);

    // Other diagram types keep `default`.
    let r = render::render("pie\n\"a\": 1\n\"b\": 2", &cfg(None)).unwrap();
    assert_eq!(
        r.scene.font_family,
        "\"trebuchet ms\", verdana, arial, sans-serif"
    );
}

#[test]
fn redux_dark_color_outlines_only() {
    let r = render::render(
        "erDiagram\nA ||--o{ B : r",
        &cfg(Some(ThemeName::ReduxDarkColor)),
    )
    .unwrap();
    // Empty bkgColorArray: entities keep mainBkg, borders take the palette.
    assert_eq!(node_fills(&r.scene), ["#111113", "#111113"]);
    let svg = mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default());
    assert!(svg.contains("#E879F9") && svg.contains("#2DD4BF"));
}

#[test]
fn sequence_actors_cycle_through_the_palette() {
    let r = render::render(
        "sequenceDiagram\nparticipant A\nparticipant B\nparticipant C\nA->>B: hi\nB->>C: hi",
        &cfg(Some(ThemeName::ReduxColor)),
    )
    .unwrap();
    let svg = mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default());
    for c in [
        "#FDF4FF", "#F0FDFA", "#FFF7ED", "#E879F9", "#2DD4BF", "#FB923C",
    ] {
        assert!(svg.contains(c), "{c} missing");
    }
}

/// The theme's `radius` reaches flowchart rounded nodes (`roundedRect.ts`), through
/// the shape metrics: 12 under the mermaid 12 default (`redux-color`), 5 classically.
#[test]
fn rounded_node_radius_follows_the_theme() {
    let svg_with = |cfg: &MermaidConfig| {
        let r = render::render("flowchart LR\n  A(round)\n", cfg).expect("renders");
        mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default())
    };
    let neo = svg_with(&MermaidConfig::default());
    assert!(neo.contains("rx=\"12\""), "redux-color radius");
    let classic = svg_with(&MermaidConfig::default().with_cli_theme(ThemeName::Default));
    assert!(classic.contains("rx=\"5\"") && !classic.contains("rx=\"12\""));
}

#[test]
fn er_table_lines_take_the_neo_stroke_width() {
    // `[data-look="neo"].node path { stroke-width: strokeWidth }` reaches the
    // rows' and columns' lines as well as the outline (2px in mermaid 12.3.0's
    // default theme); in the classic look they keep erBox's rough.js 1.3px.
    let src = "erDiagram\n  CUSTOMER {\n    string id PK\n    string name\n  }\n";
    let widths = |src: &str| {
        let r = render::render(src, &MermaidConfig::default()).expect("renders");
        let svg = mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default());
        svg.split("<path ")
            .skip(1)
            .map(|p| {
                let at = p.find("stroke-width=\"").expect("width") + 14;
                p[at..at + p[at..].find('"').unwrap()].to_string()
            })
            .collect::<Vec<_>>()
    };
    let neo = widths(src);
    assert!(!neo.is_empty() && neo.iter().all(|w| w == "2"), "{neo:?}");
    let classic = widths(&format!("---\nconfig:\n  look: classic\n---\n{src}"));
    assert!(classic.iter().all(|w| w == "1.3"), "{classic:?}");
}

#[test]
fn collapsed_subgraph_markers_take_the_slot_colour() {
    // flowchart/styles.ts: a collapsed subgraph's separator and dots are
    // clusterBorder (dots at 60%), or its palette slot's border colour.
    let render = |src: &str| {
        let r = render::render(src, &MermaidConfig::default()).expect("renders");
        mmdc::output::encode_svg(&r, &mmdc::output::EncodeOptions::default())
    };
    let src = "flowchart LR\n  subgraph a [A]\n    x\n  end\n  subgraph b [B]\n    y\n  end\n  b@{ view: collapsed }\n";
    let svg = render(src);
    let dot = svg
        .split("class=\"collapsed-indicator\"")
        .nth(1)
        .expect("dot");
    assert!(
        dot.contains("fill=\"#2DD4BF\"") && dot.contains("opacity=\"0.6\""),
        "{dot:.120}"
    );
    let sep = svg
        .split("class=\"collapsed-separator\"")
        .nth(1)
        .expect("separator");
    assert!(sep.contains("stroke=\"#2DD4BF\""), "{sep:.120}");
    let classic = render(&format!("---\nconfig:\n  theme: default\n---\n{src}"));
    let dot = classic
        .split("class=\"collapsed-indicator\"")
        .nth(1)
        .expect("dot");
    assert!(
        !dot.contains("#2DD4BF") && dot.contains("opacity=\"0.6\""),
        "{dot:.120}"
    );
}
