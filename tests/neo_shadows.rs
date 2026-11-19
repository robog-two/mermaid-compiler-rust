//! The shared neo-look stylesheet of mermaid 12.3.0 (`getStyles`): the theme's
//! `dropShadow` filter and `useGradient` strokes on `[data-look="neo"]` nodes and
//! clusters, pinned to Chromium's computed styles of mermaid's own renders.

use mmdc::config::MermaidConfig;
use mmdc::output::{EncodeOptions, encode_pdf, encode_svg};
use mmdc::render;
use mmdc::scene::{Element, Geometry, Paint, Scene, Shadow};
use mmdc::theme::{DropShadow, ThemeName, palette};

fn scene(src: &str) -> Scene {
    render::render(src, &MermaidConfig::default())
        .expect("renders")
        .scene
}

/// Every shape with the class of the group it sits in (`id` or `class`).
fn shapes(s: &Scene) -> Vec<(String, Geometry, Paint)> {
    fn walk(els: &[Element], group: &str, out: &mut Vec<(String, Geometry, Paint)>) {
        for e in els {
            match e {
                Element::Group {
                    id,
                    class,
                    children,
                } => {
                    let g = id.clone().or_else(|| class.clone()).unwrap_or_default();
                    walk(children, &g, out);
                }
                Element::Rotated { children, .. }
                | Element::Translated { children, .. }
                | Element::Markup { children, .. }
                | Element::Clipped { children, .. } => walk(children, group, out),
                Element::Shape { geom, paint, .. } => {
                    out.push((group.to_string(), geom.clone(), paint.clone()))
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&s.elements, "", &mut out);
    out
}

fn themed(theme: &str, body: &str) -> String {
    format!("---\nconfig:\n  theme: {theme}\n---\n{body}")
}

const FLOW: &str = "flowchart LR\n  A[Start] --> B((Circle))\n  B --> C{Choice}";

/// `drop-shadow(1px 2px 2px rgba(185, 185, 185, 1))`: Chromium blurs with a
/// standard deviation of 2 (the length itself, not half of it).
fn css_drop_shadow(opacity: f64) -> DropShadow {
    DropShadow {
        dx: 1.0,
        dy: 2.0,
        std_dev: 2.0,
        color: mmdc::scene::Color::new("#b9b9b9"),
        opacity,
    }
}

/// [`css_drop_shadow`] as the blurred filter drawn under a shape.
fn css_shadow(opacity: f64) -> Shadow {
    css_drop_shadow(opacity).blurred().expect("blurred")
}

#[test]
fn classic_themes_cast_the_css_drop_shadow_under_neo() {
    for (theme, opacity) in [
        ("default", 1.0),
        ("dark", 1.0),
        ("forest", 0.5),
        ("neutral", 1.0),
    ] {
        let s = scene(&themed(theme, FLOW));
        let nodes: Vec<_> = shapes(&s)
            .into_iter()
            .filter(|(g, _, _)| g.starts_with("flowchart-"))
            .collect();
        assert_eq!(nodes.len(), 3, "{theme}");
        for (g, _, p) in &nodes {
            // Written in SVG as the CSS function itself.
            let sh = css_drop_shadow(opacity).filter();
            assert!(sh.css && !sh.linear_rgb);
            assert_eq!(p.shadow, Some(sh), "{theme} {g}");
        }
    }
    // The classic look has none of it.
    let classic = format!("---\nconfig:\n  theme: dark\n  look: classic\n---\n{FLOW}");
    assert!(
        shapes(&scene(&classic))
            .iter()
            .all(|(_, _, p)| p.shadow.is_none())
    );
}

#[test]
fn use_gradient_strokes_nodes_across_their_box() {
    // theme-dark.js: `gradientStart = primaryBorderColor` (#cccccc),
    // `gradientStop = secondaryBorderColor`; theme-default.js: `useGradient = false`.
    let s = scene(&themed("dark", FLOW));
    let (_, _, p) = shapes(&s)
        .into_iter()
        .find(|(g, _, _)| g.starts_with("flowchart-A"))
        .expect("node A");
    let g = p.stroke_gradient.expect("a gradient");
    assert!(g.bbox && g.x1 == 0.0 && g.x2 == 1.0);
    assert_eq!(g.from.css(), "#cccccc");
    assert_eq!(g.to.css(), "hsl(180, 0%, 18.3529411765%)");
    let s = scene(&themed("default", FLOW));
    assert!(
        shapes(&s)
            .iter()
            .all(|(_, _, p)| p.stroke_gradient.is_none())
    );
    let svg = encode_svg(
        &render::render(&themed("dark", FLOW), &MermaidConfig::default()).unwrap(),
        &EncodeOptions::default(),
    );
    assert!(svg.contains(
        "<linearGradient id=\"my-svg-gradient-0\" gradientUnits=\"objectBoundingBox\" x1=\"0%\" x2=\"100%\""
    ));
}

#[test]
fn theme_variables_steer_the_gradient_and_the_shadow() {
    let vars = |kv: &[(&str, &str)]| -> Vec<(String, String)> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    // theme-base.js: a custom `nodeBorder` turns the gradient off ...
    assert!(
        palette(ThemeName::Base, &vars(&[("nodeBorder", "#ff0000")]))
            .node_gradient
            .is_none()
    );
    // ... and the gradient follows the overridden border colours.
    let p = palette(
        ThemeName::Base,
        &vars(&[
            ("primaryBorderColor", "#00aa00"),
            ("secondaryBorderColor", "#aa00aa"),
        ]),
    );
    let (a, b) = p.node_gradient.expect("gradient");
    assert_eq!((a.css(), b.css()), ("#00aa00", "#aa00aa"));
    // A frontmatter `dropShadow` is emptied by sanitizeDirective (its hyphen):
    // mermaid then writes `filter: none`.
    let src = "---\nconfig:\n  theme: dark\n  themeVariables:\n    dropShadow: 'drop-shadow(3px 3px 5px red)'\n---\nflowchart LR\n  A";
    assert!(
        shapes(&scene(src))
            .iter()
            .all(|(_, _, p)| p.shadow.is_none())
    );
    let d = DropShadow::parse_css("drop-shadow( 0px 1px 2px rgba(0, 0, 0, 0.25));").unwrap();
    assert_eq!((d.dx, d.dy, d.std_dev, d.opacity), (0.0, 1.0, 2.0, 0.25));
}

#[test]
fn shadows_follow_the_shapes_mermaid_filters() {
    // `dbl-circ`: both `circle`s cast one; `fork` and `f-circ` are rough.js paths
    // in plain groups and cast none (mermaid 12.3.0, theme dark).
    let src = themed(
        "dark",
        "flowchart LR\n  a --> d@{ shape: dbl-circ, label: \"D\" }\n  a --> f@{ shape: fork }\n  a --> fc@{ shape: f-circ }",
    );
    let s = shapes(&scene(&src));
    let count = |id: &str| {
        s.iter()
            .filter(|(g, _, p)| g.starts_with(id) && p.shadow.is_some())
            .count()
    };
    assert_eq!(count("flowchart-d-"), 2);
    assert_eq!(count("flowchart-f-"), 0);
    assert_eq!(count("flowchart-fc-"), 0);
    // A state choice casts none either.
    let st = shapes(&scene(&themed(
        "dark",
        "stateDiagram-v2\n  state c <<choice>>\n  [*] --> c\n  c --> A",
    )));
    assert!(
        st.iter()
            .filter(|(g, _, _)| g.contains("-c-"))
            .all(|(_, _, p)| p.shadow.is_none())
    );
}

#[test]
fn redux_state_circles_take_the_small_hard_shadow() {
    // `nodeShadow`: an inline `filter: url(#<id>-drop-shadow-small)` (2px,
    // stdDeviation 0, 6% black), in the classic look too.
    let src = "---\nconfig:\n  theme: redux\n  look: classic\n---\nstateDiagram-v2\n  [*] --> A\n  A --> [*]";
    let s = shapes(&scene(src));
    let shadowed: Vec<_> = s
        .iter()
        .filter(|(g, geom, p)| {
            g.contains("root_")
                && matches!(geom, Geometry::Circle { r, .. } if *r == 7.0)
                && p.shadow.as_ref().is_some_and(|sh| {
                    sh.linear_rgb && sh.dx == 2.0 && sh.color.css() == "rgba(0, 0, 0, 0.06)"
                })
        })
        .collect();
    assert_eq!(shadowed.len(), 2);
    // Written as mermaid's own filter (in linear RGB, which the shape it
    // draws passes through as well).
    let svg = mmdc::output::encode_svg(
        &render::render(src, &MermaidConfig::default()).unwrap(),
        &EncodeOptions::default(),
    );
    assert!(svg.contains(
        "height=\"150%\" width=\"150%\"><feDropShadow dx=\"2\" dy=\"2\" stdDeviation=\"0\" \
         flood-opacity=\"0.06\" flood-color=\"#000000\"/>"
    ));
}

#[test]
fn block_shapes_cast_the_shadow_under_neo() {
    let src = "---\nconfig:\n  look: neo\n---\nblock-beta\n  a[(\"Store\")] b[\"Rect\"]";
    let s = shapes(&scene(src));
    assert_eq!(s.iter().filter(|(_, _, p)| p.shadow.is_some()).count(), 2);
}

#[test]
fn pdf_paints_blurred_shadows_through_soft_masks() {
    let r = render::render(&themed("dark", FLOW), &MermaidConfig::default()).unwrap();
    let pdf = encode_pdf(&r, &EncodeOptions::default());
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/Sh0 Do"));
    assert!(text.contains(
        "/ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true /Filter /RunLengthDecode"
    ));
    assert!(text.contains("/SMask "));
    assert!(text.contains("/ShadingType 2"));
}

#[test]
fn usecase_notes_and_tables_take_the_gradient_and_the_shadow() {
    // mermaid 12.3.0, theme dark: the note's rough.js paths and the JSON table's
    // rects are `.node path` / `.node rect` (gradient strokes, CSS shadow); the
    // use-case ellipse is neither.
    let src = themed(
        "dark",
        "usecase-beta\nactor Ops\nDeploy(\"Deploy\")\nOps --> Deploy\nnote for Deploy \"Night\"\njson Env@{ \"region\": \"eu\" }\nDeploy --> Env",
    );
    let s = shapes(&scene(&src));
    let grads = s
        .iter()
        .filter(|(_, _, p)| p.stroke_gradient.is_some())
        .count();
    // Note: fill path and outline; table: border, title cell, key and value cells.
    assert_eq!(grads, 6);
    let shadows = s
        .iter()
        .filter(|(_, _, p)| p.shadow == Some(css_shadow(1.0)))
        .count();
    assert_eq!(shadows, 5);
}

#[test]
fn redux_nodes_pass_through_their_linear_rgb_drop_shadow() {
    // `[data-look="neo"].node rect { filter: url(#my-svg-drop-shadow) }`:
    // insertLookDefs' feDropShadow works in linear RGB, so the node itself
    // is drawn through it (Chromium paints redux-dark's #111113 fill as
    // #0D0D16), which a filter on the shape reproduces and an offset copy
    // of its silhouette did not.
    let src =
        "---\nconfig:\n  look: neo\n  theme: redux-dark\n---\nflowchart LR\n  A[Start] --> B[End]";
    let s = scene(src);
    let nodes: Vec<_> = shapes(&s)
        .into_iter()
        .filter(|(g, _, _)| g.starts_with("flowchart-"))
        .collect();
    assert!(!nodes.is_empty());
    for (g, _, p) in &nodes {
        let sh = p.shadow.as_ref().expect(g);
        assert!(sh.linear_rgb && sh.dx == 4.0 && sh.std_dev == 0.0, "{g}");
    }
    let svg = mmdc::output::encode_svg(
        &render::render(src, &MermaidConfig::default()).unwrap(),
        &EncodeOptions::default(),
    );
    assert!(svg.contains(
        "height=\"130%\" width=\"130%\"><feDropShadow dx=\"4\" dy=\"4\" stdDeviation=\"0\" \
         flood-opacity=\"0.06\" flood-color=\"#FFFFFF\"/>"
    ));
    assert!(!svg.contains("class=\"drop-shadow\""));
}
