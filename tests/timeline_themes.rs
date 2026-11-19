//! Timeline under the redux themes and the neo look, and its text markup,
//! pinned to mermaid 12.3.0's renders (and Chromium's computed styles of them).

use mmdc::config::MermaidConfig;
use mmdc::output::{EncodeOptions, encode_svg};
use mmdc::render;
use mmdc::scene::{Element, Geometry, Paint, Scene, Text};

fn rendered(src: &str) -> render::Rendered {
    render::render(src, &MermaidConfig::default()).expect("renders")
}

fn svg(src: &str) -> String {
    encode_svg(&rendered(src), &EncodeOptions::default())
}

fn view_box(svg: &str) -> &str {
    let at = svg.find("viewBox=\"").expect("viewBox") + 9;
    &svg[at..at + svg[at..].find('"').unwrap()]
}

fn with(config: &str, body: &str) -> String {
    format!("---\nconfig:\n{config}\n---\n{body}")
}

const HORIZONTAL: &str = "timeline
    title History of Social Media Platform
    section Early
      2002 : LinkedIn
      2004 : Facebook : Google
    section Later
      2005 : YouTube with a much longer event label that wraps
      2006 : Twitter";

const VERTICAL: &str = "timeline TD
    title Vertical
    2002 : LinkedIn
    2004 : Facebook : Google
    2005 : YouTube";

/// Every shape and text, with the translations of the groups it sits in added.
type Placed<T> = Vec<(T, (f64, f64))>;

#[allow(clippy::type_complexity)]
fn flat(s: &Scene) -> (Vec<(Geometry, Paint, (f64, f64))>, Placed<Text>) {
    fn walk(
        els: &[Element],
        o: (f64, f64),
        shapes: &mut Vec<(Geometry, Paint, (f64, f64))>,
        texts: &mut Vec<(Text, (f64, f64))>,
    ) {
        for e in els {
            match e {
                Element::Group { children, .. }
                | Element::Brightened { children, .. }
                | Element::Markup { children, .. }
                | Element::Clipped { children, .. } => walk(children, o, shapes, texts),
                Element::Translated { dx, dy, children } => {
                    walk(children, (o.0 + dx, o.1 + dy), shapes, texts)
                }
                Element::Shape { geom, paint, .. } => shapes.push((geom.clone(), paint.clone(), o)),
                Element::Text { text, .. } => texts.push((text.clone(), o)),
                _ => {}
            }
        }
    }
    let (mut shapes, mut texts) = (Vec::new(), Vec::new());
    walk(&s.elements, (0.0, 0.0), &mut shapes, &mut texts);
    (shapes, texts)
}

#[test]
fn redux_layouts_take_the_configs_font_size_for_the_half_line() {
    // `node.height = bbox.height + conf.fontSize * 1.1 * 0.5 + padding`:
    // `conf.fontSize` is 16 while the redux stylesheet sets 14px.
    for theme in ["redux", "redux-dark", "redux-color", "redux-dark-color"] {
        let s = svg(&with(&format!("  theme: '{theme}'"), HORIZONTAL));
        assert_eq!(view_box(&s), "100 -58 1190 641.2000122070312", "{theme}");
        let s = svg(&with(&format!("  theme: '{theme}'"), VERTICAL));
        assert_eq!(view_box(&s), "95 -58 745 420", "{theme}");
    }
    // A top-level `fontSize` sizes only that half line (and the vertical
    // axis' ends); the stylesheet keeps the theme's 16px.
    let s = svg(&with(
        "  fontSize: 20",
        "timeline\n    title T\n    2002 : LinkedIn",
    ));
    assert_eq!(view_box(&s), "-105 -61 795 478.015625");
    let s = svg(&with(
        "  fontSize: 20\n  theme: redux",
        "timeline\n    title T\n    2002 : LinkedIn",
    ));
    assert_eq!(view_box(&s), "-105 -58 795 474");
}

#[test]
fn redux_sections_are_outlined_boxes_in_the_node_border() {
    // genReduxSections: `fill: isDark && isColor ? mainBkg : slot ?? mainBkg`,
    // `stroke: slot ?? nodeBorder`, `stroke-width: strokeWidth`; the text in
    // `nodeBorder` at weight 600; `.lineWrapper line` in `nodeBorder`, 2px.
    let cases = [
        ("redux", "#ffffff", "#28253D"),
        ("redux-dark", "#111113", "#FFFFFF"),
        ("redux-color", "#2DD4BF", "#2DD4BF"),
        ("redux-dark-color", "#111113", "#2DD4BF"),
    ];
    for (theme, fill, stroke) in cases {
        let r = rendered(&with(&format!("  theme: '{theme}'"), HORIZONTAL));
        let (shapes, texts) = flat(&r.scene);
        // The second section, `section-0`.
        let later = texts
            .iter()
            .find(|(t, _)| t.lines == ["Later"])
            .expect("section label");
        assert_eq!(later.0.weight, Some(600), "{theme}");
        assert!(later.0.bold && later.0.dy_tspans);
        let bkg = shapes
            .iter()
            .find(|(g, p, o)| {
                matches!(g, Geometry::Path(_))
                    && p.stroke_width == 2.0
                    && o.0 == 600.0
                    && o.1 == 50.0
            })
            .expect("section box");
        assert_eq!(bkg.1.fill.css(), fill, "{theme}");
        assert_eq!(bkg.1.stroke.css(), stroke, "{theme}");
        // The axis: 2px, its arrowhead scaled by 2 (12px long).
        let axis = shapes
            .iter()
            .find(|(g, _, _)| matches!(g, Geometry::Line { a, b } if a.y == b.y))
            .expect("axis");
        assert_eq!(axis.1.stroke_width, 2.0, "{theme}");
    }
}

#[test]
fn node_text_is_written_as_wraps_dy_tspans() {
    // `<g transform="translate(95, 13)"><text dy="1em" … dominant-baseline=
    // "middle"><tspan x="0" dy="1em">…</tspan><tspan x="0" dy="1.1em">…`.
    let s = svg(&with("  theme: redux", HORIZONTAL));
    assert!(
        s.contains("<g class=\"eventWrapper\" transform=\"translate(600, 365.8)\">"),
        "{s}"
    );
    // The words and the whitespace between them are joined by a space each.
    assert!(s.contains(
        "<g transform=\"translate(95, 13)\"><text dy=\"1em\" \
         alignment-baseline=\"middle\" dominant-baseline=\"middle\" text-anchor=\"middle\">\
         <tspan x=\"0\" dy=\"1em\">YouTube   with   a   much</tspan><tspan x=\"0\" dy=\"1.1em\">\
         longer   event   label</tspan><tspan x=\"0\" dy=\"1.1em\">that   wraps</tspan></text></g>"
    ));
    // Section and period labels sit `padding` down, events `padding / 2 + 3`.
    assert!(s.contains("<g transform=\"translate(195, 20)\"><text dy=\"1em\""));
}

#[test]
fn wrapped_lines_sit_where_chromium_accumulates_their_dy() {
    // Chromium resolves `1em` / `1.1em` and sums them in single precision:
    // 16, 33.5999985, 51.1999969 below the text's `y`, plus the `middle`
    // baseline shift (270/64 px at 16px).
    let r =
        rendered("timeline\n2005 : YouTube with a much longer event label that wraps over lines");
    let (_, texts) = flat(&r.scene);
    let (t, _) = texts
        .iter()
        .find(|(t, _)| t.lines.len() >= 3)
        .expect("wrapped");
    let shift = 270.0 / 64.0;
    assert_eq!(t.line_y(0), 16.0 + shift);
    assert_eq!(t.line_y(1), f64::from(16f32 + 1.1f32 * 16f32) + shift);
    assert_eq!(
        t.line_y(2),
        f64::from(16f32 + 1.1f32 * 16f32 + 1.1f32 * 16f32) + shift
    );
    // getStartPositionOfChar(0).y of the three tspans in Chromium: 16,
    // 33.599998474121094, 51.19999694824219.
    assert_eq!(t.line_y(1), 33.599998474121094 + shift);
    assert_eq!(t.line_y(2), 51.19999694824219 + shift);
}

#[test]
fn neo_redux_boxes_cast_drawnodes_hard_shadow() {
    // drawNode: `<filter id="{id}-drop-shadow" height="130%" width="130%">
    // <feDropShadow dx="4" dy="4" stdDeviation="0" flood-opacity flood-color>`,
    // 6% black, or 20% white on a theme with "dark" in its name.
    let s = svg(&with("  look: neo\n  theme: redux", HORIZONTAL));
    assert!(s.contains(
        "<filter id=\"my-svg-drop-shadow\" height=\"130%\" width=\"130%\"><feDropShadow dx=\"4\" \
         dy=\"4\" stdDeviation=\"0\" flood-opacity=\"0.06\" flood-color=\"#000000\"/></filter>"
    ));
    let s = svg(&with("  look: neo\n  theme: redux-dark", HORIZONTAL));
    assert!(s.contains("flood-opacity=\"0.2\" flood-color=\"#FFFFFF\""));
    // Without the neo look the boxes' `filter: url(…)` finds nothing.
    let s = svg(&with("  theme: redux", HORIZONTAL));
    assert!(!s.contains("<filter"));
}

#[test]
fn neo_moves_the_title_and_outlines_boxes_with_the_gradient() {
    // `x = look === 'neo' ? box.x * 2 + LEFT_MARGIN : box.width / 2 - LEFT_MARGIN`.
    let title_x = |src: &str| {
        let r = rendered(src);
        let (_, texts) = flat(&r.scene);
        texts
            .iter()
            .find(|(t, _)| t.lines == ["History of Social Media Platform"])
            .map(|(t, o)| t.x + o.0)
            .expect("title")
    };
    assert_eq!(
        title_x(&with("  look: neo\n  theme: redux", HORIZONTAL)),
        550.0
    );
    assert_eq!(title_x(&with("  theme: redux", HORIZONTAL)), 245.0);
    // `useGradient` themes: `.section-N[data-look="neo"] path { fill: mainBkg;
    // stroke: url(#{id}-gradient); stroke-width: 2 }`; `timeline TD` defines
    // no gradient, so its outlines paint nothing.
    let r = rendered(&with("  look: neo\n  theme: neo", HORIZONTAL));
    let (shapes, _) = flat(&r.scene);
    let boxes: Vec<_> = shapes
        .iter()
        .filter(|(g, _, _)| matches!(g, Geometry::Path(c) if c.len() > 4))
        .collect();
    assert!(!boxes.is_empty());
    for (_, p, _) in &boxes {
        assert_eq!(p.fill.css(), "#ffffff");
        assert!(p.stroke_gradient.is_some() && p.stroke_width == 2.0);
    }
    let r = rendered(&with("  look: neo\n  theme: neo", VERTICAL));
    let (shapes, _) = flat(&r.scene);
    for (g, p, _) in &shapes {
        if matches!(g, Geometry::Path(c) if c.len() > 4) {
            assert!(p.stroke.is_none() && p.stroke_gradient.is_none());
        }
    }
}
