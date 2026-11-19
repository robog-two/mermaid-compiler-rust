//! gitGraph under the neo look and the neo / redux themes, pinned to mermaid
//! 12.3.0's renders.

use mmdc::config::MermaidConfig;
use mmdc::geom::Rect;
use mmdc::output::{EncodeOptions, encode_svg};
use mmdc::render;
use mmdc::scene::{Element, Geometry, Paint, Scene};

fn scene(src: &str) -> Scene {
    render::render(src, &MermaidConfig::default())
        .expect("renders")
        .scene
}

fn source(look: &str, theme: &str) -> String {
    format!(
        "---\nconfig:\n  look: {look}\n  theme: '{theme}'\n---\n{}\n  commit id: \"a\"\n  \
         commit id: \"b\" tag: \"v1.0\"\n  branch dev\n  commit id: \"c\"\n  checkout main\n  \
         commit id: \"d\"\n  merge dev id: \"m\"\n",
        ["git", "Graph"].concat()
    )
}

/// Every shape with its class.
fn shapes(s: &Scene) -> Vec<(String, Geometry, Paint)> {
    fn walk(els: &[Element], out: &mut Vec<(String, Geometry, Paint)>) {
        for e in els {
            match e {
                Element::Group { children, .. }
                | Element::Markup { children, .. }
                | Element::Rotated { children, .. }
                | Element::Translated { children, .. } => walk(children, out),
                Element::Shape { geom, paint, class } => out.push((
                    class.clone().unwrap_or_default(),
                    geom.clone(),
                    paint.clone(),
                )),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&s.elements, &mut out);
    out
}

fn main_label_box(s: &Scene) -> (Rect, Paint) {
    shapes(s)
        .into_iter()
        .find_map(|(c, g, p)| match g {
            Geometry::Rect { rect, .. } if c == "branchLabelBkg label0" => Some((rect, p)),
            _ => None,
        })
        .expect("main's label box")
}

#[test]
fn branch_labels_take_the_root_font_of_the_theme() {
    // The neo and redux themes' 14px root font sizes "main" (getBBox 30.36 ×
    // 16 under neo; 32.69 wide, bold, under redux); the classic themes' 16px.
    let (r, _) = main_label_box(&scene(&source("classic", "neo")));
    assert_eq!((r.w, r.h), (48.359375, 20.0));
    let (r, _) = main_label_box(&scene(&source("classic", "redux")));
    assert_eq!((r.w, r.h), (67.46875, 33.0));
    let (r, _) = main_label_box(&scene(&source("neo", "default")));
    assert_eq!((r.w, r.h), (52.6875, 21.0));
    // viewBox `-94.46875 -28.7 352.46875 163.078`: the redux commit labels
    // are measured at weight 600 too.
    let v = scene(&source("classic", "redux")).view_box;
    assert_eq!((v.x, v.w), (-94.46875, 352.46875));
}

#[test]
fn neo_branch_labels_cast_the_themes_shadow() {
    // `style="filter:drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))"`, and
    // under redux `filter:url(#{id}-drop-shadow)` (feDropShadow 4 4, 6%).
    let svg = |look: &str, theme: &str| {
        let full = encode_svg(
            &render::render(&source(look, theme), &MermaidConfig::default()).unwrap(),
            &EncodeOptions::default(),
        );
        // Not mermaid's own stylesheet, which spells the neo rules in any look.
        match (full.find("<style>"), full.find("</style>")) {
            (Some(a), Some(b)) if a < b => format!("{}{}", &full[..a], &full[b..]),
            _ => full,
        }
    };
    let s = svg("neo", "default");
    assert!(s.contains("style=\"filter:drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))\""));
    assert!(!svg("classic", "default").contains("drop-shadow"));
    let s = svg("neo", "redux");
    assert!(s.contains("<feDropShadow dx=\"4\" dy=\"4\" stdDeviation=\"0\" flood-opacity=\"0.06\" flood-color=\"#000000\"/>"));
    let (_, p) = main_label_box(&scene(&source("neo", "redux")));
    assert!(p.shadow.as_ref().is_some_and(|sh| sh.linear_rgb));
    // The neo themes' tags take `.tag-label-bkg { filter: dropShadow }` from
    // the stylesheet in any look (no style attribute); their label boxes are
    // outlined with the gradient.
    let s = svg("classic", "neo");
    assert!(!s.contains("style=\"filter"));
    let (_, p) = main_label_box(&scene(&source("classic", "neo")));
    assert!(p.stroke_gradient.is_some());
}
