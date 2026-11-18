//! Mindmap under the neo look and the redux themes, pinned to mermaid
//! 12.3.0's renders (Chromium's computed styles of them).

use mmdc::config::MermaidConfig;
use mmdc::output::{EncodeOptions, encode_svg};
use mmdc::render;
use mmdc::scene::{Element, Geometry, Paint, Scene};

fn scene(src: &str) -> Scene {
    render::render(src, &MermaidConfig::default())
        .expect("renders")
        .scene
}

fn with(config: &str, body: &str) -> String {
    format!("---\nconfig:\n{config}\n---\n{body}")
}

const SMALL: &str = "mindmap\n  root((Root))\n    A\n    B\n      C";

/// Every shape, and whether it sits under a group-wide drop shadow.
fn shapes(s: &Scene) -> Vec<(Geometry, Paint, bool)> {
    fn walk(els: &[Element], shadowed: bool, out: &mut Vec<(Geometry, Paint, bool)>) {
        for e in els {
            match e {
                Element::Group { children, .. } => walk(children, shadowed, out),
                Element::Shadowed { children, .. } => walk(children, true, out),
                Element::Shape { geom, paint, .. } => {
                    out.push((geom.clone(), paint.clone(), shadowed))
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&s.elements, false, &mut out);
    out
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn neo_sizes_follow_the_neo_shapes() {
    // circle.ts under neo: r = half the label's diagonal + 16 (not the
    // padding), which grows the drawing by 12 each way.
    let v = scene(&with("  look: neo", SMALL)).view_box;
    assert!(
        close(v.w, 406.8412170410156) && close(v.h, 93.45152282714844),
        "{v:?}"
    );
    let v = scene(&with("  look: neo\n  theme: neo", SMALL)).view_box;
    assert!(
        close(v.w, 397.57080078125) && close(v.h, 88.27485656738281),
        "{v:?}"
    );
    // squareRect.ts (16 / 12 a side), hexagon.ts (h = th + 70, m = h / 3.5,
    // w = tw + 2m + 32): mermaid's viewBoxes.
    let x = "mindmap\n  root[Square root]\n    a(Rounded)\n      a1((Circle))\n      a2))Bang((\n    \
             b{{Hexagon}}\n      b1)Cloud(\n      b2[Square child with a much longer label that \
             wraps]\n    c\n      c1";
    let v = scene(&with("  look: neo", x)).view_box;
    assert!(
        close(v.w, 811.315673828125) && close(v.h, 449.19708251953125),
        "{v:?}"
    );
    let v = scene(&with("  look: neo\n  theme: redux", x)).view_box;
    assert!(
        close(v.w, 779.1390380859375) && close(v.h, 472.86688232421875),
        "{v:?}"
    );
}

#[test]
fn redux_default_nodes_are_rounded_rects_of_the_theme_radius() {
    // getShapeFromType: `rounded` for a default node under a redux theme,
    // `rx` the theme's 12; viewBox `5 4.999998 355.5708 76.27486` (classic).
    let s = scene(&with("  theme: redux", SMALL));
    assert!(close(s.view_box.w, 355.57080078125), "{:?}", s.view_box);
    let rects: Vec<_> = shapes(&s)
        .into_iter()
        .filter_map(|(g, _, _)| match g {
            Geometry::Rect { rect, rx } => Some((rect.w, rect.h, rx)),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), 3);
    assert!(rects.iter().all(|&(_, h, rx)| h == 51.0 && rx == 12.0));
    assert!(close(rects[0].0, 39.34375));
}

#[test]
fn neo_nodes_are_outlined_and_cast_the_themes_shadow() {
    // `[data-look="neo"].mindmap-node.section-N rect { fill; stroke;
    // stroke-width: strokeWidth ?? 2 }`: redux keeps `mainBkg` and outlines
    // in `nodeBorder`; the colour themes fill and outline in `cScale`.
    let s = scene(&with("  look: neo\n  theme: redux", SMALL));
    let sh = shapes(&s);
    let rect = sh
        .iter()
        .find(|(g, _, _)| matches!(g, Geometry::Rect { .. }))
        .expect("a node");
    assert_eq!(rect.1.fill.css(), "#ffffff");
    assert_eq!(rect.1.stroke.css(), "#28253D");
    assert_eq!(rect.1.stroke_width, 2.0);
    // The node group's filter, and the shared `.node rect` rule's own.
    assert!(rect.2 && rect.1.shadow.is_some());
    // Edges in `nodeBorder` under a redux theme, `max(10 - 2·depth, 2)` wide.
    let edge = sh
        .iter()
        .find(|(_, p, _)| p.fill.is_none() && p.stroke_width == 8.0)
        .expect("a depth-1 edge");
    assert_eq!(edge.1.stroke.css(), "#28253D");
    // The classic themes' strokeWidth is 1, their shadow the CSS one.
    let svg = encode_svg(
        &render::render(&with("  look: neo", SMALL), &MermaidConfig::default()).unwrap(),
        &EncodeOptions::default(),
    );
    assert!(svg.contains("style=\"filter:drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))\""));
    assert!(svg.contains("stroke=\"hsl(60, 100%, 73.5294117647%)\" stroke-width=\"1\""));
    // Default nodes: square bottom corners under neo, quadratic ones otherwise.
    let flat = |src: &str| {
        shapes(&scene(src))
            .iter()
            .any(|(g, _, _)| matches!(g, Geometry::Path(c) if c.len() == 8))
    };
    assert!(flat(&with("  look: neo", SMALL)));
    assert!(!flat(SMALL));
}
