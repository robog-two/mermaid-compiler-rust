//! Flowchart and state-diagram details pinned to mermaid 12.3.0 renders (the
//! computed styles and geometry Chromium reports for mermaid's own SVG): label
//! colours per theme, the theme variables a directive may set, cylinders as
//! one path, the state stylesheet's borders and fills, and the edges into a
//! state end.

use mmdc::config::MermaidConfig;
use mmdc::geom::PathCmd;
use mmdc::render;
use mmdc::scene::{Element, Geometry, Paint, Scene, Text};

fn scene(src: &str) -> Scene {
    render::render(src, &MermaidConfig::default())
        .expect("renders")
        .scene
}

fn themed(vars: &str, body: &str) -> String {
    format!("---\nconfig:\n{vars}---\n{body}")
}

/// Every shape and text with the id (or class) of the group it sits in, and
/// whether it sits in a translated frame (with that frame's offset).
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Item {
    Shape(String, Geometry, Paint, Option<(f64, f64)>),
    Text(Text),
}

fn items(s: &Scene) -> Vec<Item> {
    fn walk(els: &[Element], group: &str, frame: Option<(f64, f64)>, out: &mut Vec<Item>) {
        for e in els {
            match e {
                Element::Group {
                    id,
                    class,
                    children,
                } => {
                    let g = id.clone().or_else(|| class.clone()).unwrap_or_default();
                    let g = if g.is_empty() { group.to_string() } else { g };
                    walk(children, &g, frame, out);
                }
                Element::Translated { dx, dy, children } => {
                    walk(children, group, Some((*dx, *dy)), out)
                }
                Element::Rotated { children, .. } | Element::Clipped { children, .. } => {
                    walk(children, group, frame, out)
                }
                Element::Shape { geom, paint, .. } => out.push(Item::Shape(
                    group.to_string(),
                    geom.clone(),
                    paint.clone(),
                    frame,
                )),
                Element::Text { text, .. } => out.push(Item::Text(text.clone())),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&s.elements, "", None, &mut out);
    out
}

fn text_fill(s: &Scene, label: &str) -> String {
    items(s)
        .into_iter()
        .find_map(|i| match i {
            Item::Text(t) if t.lines.iter().any(|l| l == label) => Some(t.fill.css().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text {label}"))
}

fn text_of(s: &Scene, label: &str) -> Text {
    items(s)
        .into_iter()
        .find_map(|i| match i {
            Item::Text(t) if t.lines.iter().any(|l| l == label) => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no text {label}"))
}

/// A shape, its paint and the offset of the translated frame it sits in.
type Placed = (Geometry, Paint, Option<(f64, f64)>);

fn shapes_in(s: &Scene, group: &str) -> Vec<Placed> {
    items(s)
        .into_iter()
        .filter_map(|i| match i {
            Item::Shape(g, geom, p, f) if g.starts_with(group) => Some((geom, p, f)),
            _ => None,
        })
        .collect()
}

const FLOW: &str = "flowchart LR\n  A[Start] -->|yes| B(Round)\n  B --> C[(Store)]\n";

#[test]
fn flowchart_labels_take_node_text_or_text_color() {
    // `.label text, span { color: nodeTextColor || textColor }`: only the base,
    // neo and redux themes set `nodeTextColor` (to `primaryTextColor`); edge
    // labels are spans too.
    for (theme, color) in [
        ("neutral", "#000000"),
        ("default", "#333"),
        ("dark", "#ccc"),
        ("neo-dark", "#e0dfdf"),
        ("redux-dark", "#e0dfdf"),
    ] {
        let s = scene(&themed(&format!("  theme: {theme}\n"), FLOW));
        for label in ["Start", "Store", "yes"] {
            let fill = text_fill(&s, label);
            assert_eq!(
                fill.to_ascii_lowercase(),
                color.to_ascii_lowercase(),
                "{theme} {label}"
            );
        }
    }
    // A default-theme `primaryTextColor` leaves them alone ...
    let s = scene(&themed(
        "  theme: default\n  themeVariables:\n    primaryTextColor: '#0000aa'\n",
        FLOW,
    ));
    assert_eq!(text_fill(&s, "Start"), "#333");
    // ... and a neo `textColor` too (its labels follow `primaryTextColor`).
    let s = scene(&themed(
        "  theme: neo\n  themeVariables:\n    textColor: '#00aa00'\n",
        FLOW,
    ));
    assert_eq!(text_fill(&s, "yes"), "#333");
}

#[test]
fn a_directive_cannot_set_theme_variables_the_default_config_lacks() {
    // `sanitizeDirective` deletes every key outside `configKeys`:
    // `nodeTextColor` never reaches the theme from front matter or `%%{init}%%`.
    for src in [
        themed(
            "  theme: neutral\n  themeVariables:\n    nodeTextColor: '#aa0000'\n",
            FLOW,
        ),
        format!(
            "%%{{init: {{'theme':'neutral','themeVariables':{{'nodeTextColor':'#aa0000'}}}}}}%%\n{FLOW}"
        ),
    ] {
        let s = scene(&src);
        assert_eq!(text_fill(&s, "Start"), "#000000");
    }
    // `textColor` is a known key.
    let s = scene(&themed(
        "  theme: neutral\n  themeVariables:\n    textColor: '#00aa00'\n",
        FLOW,
    ));
    assert_eq!(text_fill(&s, "Start"), "#00aa00");
}

fn arcs(cmds: &[PathCmd]) -> usize {
    cmds.iter()
        .filter(|c| matches!(c, PathCmd::ArcTo { .. }))
        .count()
}

#[test]
fn cylinders_are_one_arc_path_in_their_own_frame() {
    // cylinder.ts `createCylinderPathD`: one filled and stroked path (no
    // separate rim line), drawn about the shape's corner and translated
    // into place. A 144 x 85.149 neo cylinder at (984.024, 102.574).
    let s = scene(&themed(
        "  theme: neo\n",
        "flowchart LR\n  A[Start] --> B(Round)\n  B --> C{Choice}\n  C -->|yes| D([Stadium])\n  C --> E((Circle))\n  D --> F[(Store)]\n  E --> G{{Hex}}\n  subgraph S [Group]\n    F\n    G\n  end\n",
    ));
    let f: Vec<_> = shapes_in(&s, "flowchart-F-")
        .into_iter()
        .filter(|(g, p, _)| matches!(g, Geometry::Path(_)) && !p.stroke.is_none())
        .collect();
    assert_eq!(f.len(), 1, "{f:?}");
    let (Geometry::Path(cmds), _, Some((dx, dy))) = &f[0] else {
        panic!("{f:?}")
    };
    assert_eq!(arcs(cmds), 3);
    assert!((dx - (984.0237579345703 - 72.0)).abs() < 1e-6, "{dx}");
    assert!(
        (dy - (102.57434844970703 - 42.57434944237918)).abs() < 1e-3,
        "{dy}"
    );
    // The local frame starts at (0, ry) as mermaid's `M0,13.38`.
    let PathCmd::MoveTo(p) = cmds[0] else {
        panic!("{cmds:?}")
    };
    assert!(
        p.x.abs() < 1e-9 && (p.y - 13.382899628252789).abs() < 1e-3,
        "{p:?}"
    );

    // linedCylinder.ts adds its second rim as a subpath; tiltedCylinder.ts
    // draws the right cap's inner half as one.
    let s = scene(
        "flowchart TD\n  C@{ shape: lin-cyl, label: \"Lined\" } --> D@{ shape: h-cyl, label: \"Horiz\" }\n",
    );
    for (id, n) in [("flowchart-C-", 4), ("flowchart-D-", 3)] {
        let paths: Vec<_> = shapes_in(&s, id)
            .into_iter()
            .filter(|(g, p, _)| matches!(g, Geometry::Path(_)) && !p.stroke.is_none())
            .collect();
        assert_eq!(paths.len(), 1, "{id} {paths:?}");
        let Geometry::Path(cmds) = &paths[0].0 else {
            unreachable!()
        };
        assert_eq!(arcs(cmds), n, "{id}");
    }
}

#[test]
fn translucent_edge_label_backgrounds_layer_over_label_bkg() {
    // The default theme's `edgeLabelBackground` is rgba(232,232,232,0.8): the
    // `<p>` paints it over the `div.labelBkg`'s `fade(.., 0.5)`.
    let s = scene(&themed("  theme: default\n", FLOW));
    let rects: Vec<String> = items(&s)
        .into_iter()
        .filter_map(|i| match i {
            Item::Shape(g, Geometry::Rect { .. }, p, _) if g == "edgeLabel" => {
                Some(p.fill.css().to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        rects,
        vec!["rgba(232, 232, 232, 0.5)", "rgba(232,232,232, 0.8)"]
    );
    // An opaque one hides it: no extra layer.
    let s = scene(&themed("  theme: neutral\n", FLOW));
    let n = items(&s)
        .into_iter()
        .filter(|i| matches!(i, Item::Shape(g, Geometry::Rect { .. }, _, _) if g == "edgeLabel"))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn document_label_sits_padding_in_from_the_corner() {
    // waveEdgedRectangle.ts (neo): the label box at `(-w/2 + padding, -h/2 +
    // padding - amp)`, 1px left of centre and 9px above the node's origin
    // (forest, node at (106.5, 47.9957); its 120px label box at x 45.5).
    let s = scene(&themed(
        "  theme: forest\n",
        "flowchart TD\n  a@{ shape: doc, label: \"Doc\" } --> b@{ shape: cyl, label: \"Cyl\" }\n  b --> c@{ shape: tri, label: \"Tri\" }\n  c --> d@{ shape: dbl-circ, label: \"Dbl\" }\n  d --> e@{ shape: fork }\n  e --> f@{ shape: sm-circ }\n  f --> g@{ shape: lean-r, label: \"Lean\" }\n  g --> h@{ shape: notch-rect, label: \"Card\" }\n  h --> i[/Para/]\n  i --> j>Flag]\n",
    ));
    let t = text_of(&s, "Doc");
    assert!((t.x - 105.5).abs() < 1e-3, "{}", t.x);
}

const STATE: &str = "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go\n  state Busy {\n    [*] --> Work\n    Work --> [*]\n  }\n  Busy --> [*]\n  note right of Idle : a note\n";

#[test]
fn state_labels_take_state_label_color() {
    // `.cluster-label, .nodeLabel { color: stateLabelColor }`, which is
    // `primaryTextColor` (composite titles included).
    for (theme, color) in [
        ("default", "#131300"),
        ("dark", "#e0dfdf"),
        ("neutral", "#111111"),
        ("forest", "#321b67"),
    ] {
        let s = scene(&themed(&format!("  theme: {theme}\n"), STATE));
        for label in ["Idle", "Work", "Busy"] {
            assert_eq!(
                text_fill(&s, label).to_ascii_lowercase(),
                color,
                "{theme} {label}"
            );
        }
    }
    // A `stateBkg` override fills the states and, outside the default theme,
    // colours their labels.
    let s = scene(&themed(
        "  theme: neutral\n  themeVariables:\n    stateBkg: '#ffeeee'\n",
        STATE,
    ));
    assert_eq!(text_fill(&s, "Idle"), "#ffeeee");
    let s = scene(&themed(
        "  theme: default\n  themeVariables:\n    stateBkg: '#ffeeee'\n    primaryTextColor: '#00aa00'\n",
        STATE,
    ));
    assert_eq!(text_fill(&s, "Idle"), "#131300");
    let idle = shapes_in(&s, "state-Idle-");
    assert_eq!(idle[0].1.fill.css(), "#ffeeee");
}

#[test]
fn neutral_state_transitions_are_black_and_classic_borders_state_border() {
    // theme-neutral.js: `transitionColor = '#000'` (its `lineColor` is #666).
    let s = scene(&themed("  theme: neutral\n", STATE));
    let edges: Vec<_> = items(&s)
        .into_iter()
        .filter_map(|i| match i {
            Item::Shape(g, Geometry::Path(_), p, _) if g.starts_with("L-") => Some(p),
            _ => None,
        })
        .collect();
    assert!(!edges.is_empty());
    for p in &edges {
        assert!(
            p.stroke.css() == "#000" || p.stroke.is_none(),
            "{:?}",
            p.stroke
        );
    }
    // Classic: `.node rect`, `.statediagram-cluster rect { stroke: stateBorder
    // || nodeBorder }`; a state end's rough circle is stroked `lineColor`.
    let s = scene(&themed("  theme: neutral\n  look: classic\n", STATE));
    assert_eq!(shapes_in(&s, "state-Idle-")[0].1.stroke.css(), "#000");
    let end = shapes_in(&s, "state-root_end-");
    assert_eq!(end[0].1.stroke.css(), "#666");
}

#[test]
fn an_edge_from_a_composite_steps_back_out_to_the_end_circle() {
    // ELK leaves the edge's port on the end state's 14px box (y 556); the
    // circle mermaid intersects has half the width of the rough.js outline's
    // bbox (14.0133): `M104,516 L104,556 L104,555.9933533668518`, the
    // marker turned up.
    let s = scene(&themed("  theme: default\n  look: classic\n", STATE));
    let path = items(&s)
        .into_iter()
        .find_map(|i| match i {
            Item::Shape(g, Geometry::Path(cmds), _, _) if g.starts_with("L-Busy-root_end") => {
                Some(cmds)
            }
            _ => None,
        })
        .expect("edge");
    let ys: Vec<f64> = path
        .iter()
        .filter_map(|c| match c {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => Some(p.y),
            _ => None,
        })
        .collect();
    assert_eq!(ys.len(), 3, "{ys:?}");
    assert!((ys[1] - 556.0).abs() < 1e-9, "{ys:?}");
    assert!((ys[2] - 555.9933533668518).abs() < 1e-9, "{ys:?}");
}

#[test]
fn described_states_forks_and_dividers() {
    let src = "stateDiagram-v2\n  state c <<choice>>\n  state f <<fork>>\n  [*] --> A\n  A --> c\n  c --> f\n  f --> B\n  f --> C\n  state D {\n    E : described\n    E : more\n  }\n  B --> D\n  D --> [*]\n";
    // rectWithTitle.ts: the description `title + padding/2 + 5` below the
    // title's top (mermaid: its box at y 431, 24 tall).
    let s = scene(&themed("  theme: neutral\n  look: classic\n", src));
    let more = text_of(&s, "more");
    let described = text_of(&s, "described");
    assert!((more.y - described.y - 33.0).abs() < 1e-9);
    // forkJoin.ts: a rough.js rectangle in `lineColor`, rough's 1.3px wide.
    let fork = shapes_in(&s, "state-f-");
    assert_eq!(fork[0].1.fill.css(), "#666");
    assert_eq!(fork[0].1.stroke_width, 1.3);
    // `<line class="divider">` under neo: `stateBorder`, no gradient.
    let s = scene(&themed("  theme: neutral\n", src));
    let divider: Vec<_> = shapes_in(&s, "state-E-")
        .into_iter()
        .filter(|(g, _, _)| matches!(g, Geometry::Line { .. }))
        .collect();
    assert_eq!(divider.len(), 1);
    assert_eq!(divider[0].1.stroke.css(), "#000");
    assert!(divider[0].1.stroke_gradient.is_none());
}

#[test]
fn classic_business_use_case_marker_is_one_pixel() {
    // The classic usecase stylesheet strokes `.usecase-business-marker`
    // without a width: SVG's initial 1px (mermaid 12.3.0, a business use
    // case in `#2DD4BF`); the business actor's own marker inherits 2px.
    let s = scene(
        "---\nconfig:\n  look: classic\n---\nusecase-beta\nactor Clerk@{ business: true }\nOrder(\"Place order\")@{ business: true } <<Core>>\nClerk --> Order\n",
    );
    let markers: Vec<(String, f64)> = items(&s)
        .into_iter()
        .filter_map(|i| match i {
            Item::Shape(_, Geometry::Path(cmds), p, _)
                if cmds.len() == 2 && p.fill.is_none() && !p.stroke.is_none() =>
            {
                Some((p.stroke.css().to_string(), p.stroke_width))
            }
            _ => None,
        })
        .collect();
    assert!(markers.contains(&("#2DD4BF".into(), 1.0)), "{markers:?}");
    assert!(markers.contains(&("#A78BFA".into(), 2.0)), "{markers:?}");
}
