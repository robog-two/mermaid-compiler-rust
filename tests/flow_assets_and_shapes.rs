//! Flowchart shapes pinned to mermaid 12.3.0 (headless Chromium): the newer
//! shapes under the classic and neo looks, brace edges and collapsed groups
//! under handDrawn, and the icon, image and anchor nodes. The viewBoxes are
//! mermaid's (`getBBox()` of the drawing, padded), written the way our SVG
//! writes numbers.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn view_box(s: &str) -> &str {
    let i = s.find("viewBox=\"").expect("viewBox") + 9;
    &s[i..i + s[i..].find('"').expect("closing quote")]
}

fn with(look: &str, theme: &str, body: &str) -> String {
    format!("---\nconfig:\n  look: {look}\n  theme: {theme}\n---\n{body}")
}

const HX01: &str = r#"flowchart LR
  a@{ shape: card, label: "Card" }
  b@{ shape: lin-rect, label: "Lined" }
  c@{ shape: delay, label: "Delay" }
  d@{ shape: tri, label: "Tri" }
  e@{ shape: flip-tri, label: "Flip" }
  f@{ shape: hourglass, label: "" }
  g@{ shape: cross-circ, label: "" }
  a --> b --> c
  d --> e
"#;

const HX04: &str = r#"flowchart LR
  a@{ shape: bow-rect, label: "Bow" }
  b@{ shape: bolt, label: "" }
  c@{ shape: brace, label: "Brace" }
  d@{ shape: brace-r, label: "BraceR" }
  e@{ shape: braces, label: "Braces" }
  f@{ shape: f-circ, label: "" }
  g@{ shape: docs, label: "Docs" }
  h@{ shape: tag-doc, label: "TagDoc" }
  i@{ shape: flag, label: "Flag" }
  j@{ shape: fork, label: "" }
  a --> b --> c
  d --> e
"#;

const HX05: &str = r#"flowchart TB
  a@{ shape: bucket, label: "Bucket" }
  b@{ shape: console, label: "Console" }
  c@{ shape: browser, label: "Browser" }
  d@{ shape: person, label: "Person" }
  a --> b
  c --> d
"#;

#[test]
fn classic_delay_is_its_fifty_point_polygon_wide() {
    // halfRoundedRectangle.ts: no point of the round end lies on the axis,
    // so the node is a hair narrower than its nominal width.
    assert_eq!(
        view_box(&svg(&with("classic", "default", HX01))),
        "4 4 576.606 439"
    );
}

#[test]
fn classic_braces_are_filled_and_sized_by_their_hit_polygon() {
    let s = svg(&with("classic", "default", HX04));
    assert_eq!(view_box(&s), "4 4 560.25 686.907");
}

#[test]
fn bucket_drawing_sits_below_its_layout_box() {
    // bucket.ts: the rim bulges below the body by half its height; the
    // viewBox holds the drawing, not the (centred) layout box.
    assert_eq!(
        view_box(&svg(&with("classic", "default", HX05))),
        "4 10 356 241.685"
    );
}

#[test]
fn neo_shadows_fall_on_each_rect_and_circle_of_a_browser_and_person() {
    let s = svg(&with("neo", "redux-color", HX05));
    assert_eq!(view_box(&s), "4 10 356 235.685");
    // Bucket none; console its rect; browser its rect, three dots and the
    // address bar; person its rect and head.
    assert_eq!(s.matches("class=\"drop-shadow\"").count(), 8);
}

#[test]
fn console_prompt_is_monospace() {
    let s = svg(&with("classic", "default", HX05));
    assert!(s.contains(r#"class="console-glyph" style="font-family:monospace""#));
}

#[test]
fn hand_drawn_brace_edges_reach_the_outline_elk_bisects_to() {
    // outlineAttachPoint's probes see the doubled-back hit polygon's nearest
    // side: the edges end at 419.026 and 235.159 (their markers' tips).
    let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 15\n---\n".to_string() + HX04;
    let s = svg(&src);
    assert!(s.contains("L419.057,48.006L"), "b --> c marker");
    assert!(s.contains("L235.166,148.729L"), "d --> e marker");
    assert_eq!(view_box(&s), "4.371 4.142 565.145 685.958");
}

#[test]
fn hand_drawn_collapsed_subgraph_is_a_rough_box() {
    let src = r#"---
config:
  look: handDrawn
  theme: default
  handDrawnSeed: 7
---
flowchart LR
  a[Start] --> grp
  subgraph grp [Processing stage]
    b[Validate] --> c[Enrich]
  end
  grp --> d[Done]
  subgraph two [Second]
    e[E1]
  end
  d --> two
  grp@{ view: collapsed }
  two@{ view: collapsed }
"#;
    let s = svg(src);
    assert_eq!(view_box(&s), "4.098 3.864 722.78 111.054");
    // The rough-node misses `.node .collapsed-separator`: 1px, not 0.75.
    assert!(!s.contains("stroke-width=\"0.75\""));
}

const FORMS: &str = r#"flowchart LR
  A@{ icon: "fa:user", form: "square", label: "User" }
  B@{ icon: "fa:user", form: "circle", label: "Circle" }
  C@{ icon: "fa:user", form: "rounded", label: "Rounded" }
  D@{ icon: "fa:user", label: "Plain" }
  A --> B --> C --> D
"#;

#[test]
fn icon_nodes_draw_mermaid_s_unknown_icon() {
    let s = svg(&with("classic", "redux-color", FORMS));
    assert_eq!(view_box(&s), "4 4 417.94 156.882");
    assert_eq!(s.matches("style=\"font-family:ArialMT, Arial\"").count(), 4);
    assert_eq!(s.matches("fill=\"#087ebf\"").count(), 4);
}

#[test]
fn hand_drawn_icon_nodes_are_measured_by_their_strokes() {
    let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 4\n---\n".to_string() + FORMS;
    assert_eq!(view_box(&svg(&src)), "3.932 4.275 431.375 160.126");
}

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAJElEQVR4nGM4YWMzIIhh1OJRi0ctHrV41OJRi0ctHrV45FgMAGQL6C7l1KfNAAAAAElFTkSuQmCC";

#[test]
fn image_nodes_size_from_the_data_uri() {
    let src = format!(
        r#"flowchart LR
  A@{{ img: "data:image/png;base64,{PNG}", label: "Image", w: 60, h: 60, constraint: "on" }}
  B@{{ img: "data:image/png;base64,{PNG}", label: "Free", w: 80, h: 30 }}
  C@{{ img: "data:image/png;base64,{PNG}", label: "Natural" }}
  D@{{ img: "data:image/png;base64,{PNG}", label: "Top", pos: "t", h: 40, constraint: "on" }}
  A --> B --> C --> D
"#
    );
    let s = svg(&src);
    assert_eq!(view_box(&s), "4 4 576 109");
    // A label widens the image to the wrapping width; `constraint` keeps a
    // 40×20 picture's aspect ratio.
    for (w, h) in [(120, 60), (120, 30), (120, 20), (80, 40)] {
        assert!(s.contains(&format!(
            "width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"none\""
        )));
    }
}

#[test]
fn undecodable_images_fail_as_mermaid_does() {
    for img in ["data:image/png;base64,AAAA", "https://example.com/a.png"] {
        let src = format!("flowchart LR\n  A@{{ img: \"{img}\", label: \"X\" }}\n");
        let err = render::render(&src, &MermaidConfig::default()).expect_err("fails");
        assert_eq!(err.to_string(), "The source image cannot be decoded.");
    }
    // A remote image sized by `w` and `h` alone needs no decoding here.
    let src = "flowchart LR\n  A@{ img: \"https://example.com/a.png\", w: 60, h: 40 }\n";
    assert!(render::render(src, &MermaidConfig::default()).is_ok());
}

#[test]
fn anchors_are_two_pixel_dots_without_labels() {
    let src = with(
        "classic",
        "redux-color",
        "flowchart TD\n  A[Start] --> X@{ shape: anchor }\n  X --> B[End]\n",
    );
    let s = svg(&src);
    assert_eq!(view_box(&s), "4 4 196 200");
    assert!(!s.contains(">X<"));
    let dark = with(
        "classic",
        "dark",
        "flowchart LR\n  A[One] --> X@{ shape: anchor }\n  B[Two] --> X\n  X --> C[Three]\n  X --> D[Four]\n",
    );
    let s = svg(&dark);
    assert_eq!(view_box(&s), "4 4 498.002 164");
    // ELK spaces the ports past the dot: the edge keeps its port and slants
    // to the dot's outline (mermaid's 252.836, 88.014), less the marker.
    assert!(s.contains("L252.178,84.068"));
    // The dot's side is too short for its ports' insets, so ELK crams them in
    // reversed: X->C leaves at y 89 below X->D at 83, though first in the port
    // list. Its routing segments come in that list's order, and the tie between
    // the two crossing routes keeps X->D's vertical left (mermaid: X->D bends
    // at x 274, X->C at 294).
    let edge = |from: &str, to: &str| {
        let key = format!("LS-{from} LE-{to}\" d=\"");
        let i = s.find(&key).expect("edge") + key.len();
        s[i..i + s[i..].find('"').unwrap()].to_string()
    };
    assert!(
        edge("X", "C").contains("L294.002,46.071"),
        "{}",
        edge("X", "C")
    );
    assert!(
        edge("X", "D").contains("L274.002,125.929"),
        "{}",
        edge("X", "D")
    );
}

/// question.ts draws the diamond half a pixel right of its node, and the
/// viewBox (the drawing's `getBBox()`) holds it there: mermaid 12.3.0's
/// `4 4 569.5 187` for a diamond at the right edge.
#[test]
fn view_box_holds_the_diamond_where_it_is_drawn() {
    let s = svg("flowchart LR\n  A[Hello world] --> B(Second node)\n  B --> C{Decide}\n");
    assert_eq!(view_box(&s), "4 4 569.5 187");
}
