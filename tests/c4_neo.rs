//! C4 under the neo look and the neo / redux themes, pinned to mermaid 12.3.0.

use mmdc::config::MermaidConfig;
use mmdc::render;
use mmdc::scene::{Element, Scene};

const SRC: &str = "C4Dynamic
    title Dynamic diagram for Internet Banking System - API Application
    ContainerDb(c4, \"Database\", \"Relational Database Schema\", \"Stores user registration information, hashed authentication credentials, access logs, etc.\")
    Container(c1, \"Single-Page Application\", \"JavaScript and Angular\", \"Provides all of the Internet banking functionality to customers via their web browser.\")
    Rel(c1, c4, \"Reads\", \"JDBC\")";

fn scene(config: &str) -> Scene {
    let src = format!("---\nconfig:\n{config}\n---\n{SRC}");
    render::render(&src, &MermaidConfig::default())
        .expect("renders")
        .scene
}

#[test]
fn neo_cylinders_pad_their_labels_by_24() {
    // cylinder.ts under neo: labelPaddingX / Y 24 for the shape's 20 (the
    // database card 4px taller, so the drawing too).
    let classic = scene("  look: classic").view_box;
    let neo = scene("  look: neo").view_box;
    assert_eq!(neo.w, classic.w);
    assert!(
        (neo.h - classic.h - 4.0).abs() < 1e-6,
        "{classic:?} {neo:?}"
    );
}

#[test]
fn the_title_takes_the_themes_root_font() {
    // `<text x y>` with no class: the root's 14px under the neo and redux
    // themes.
    let title_size = |s: &Scene| {
        fn find(els: &[Element]) -> Option<f64> {
            els.iter().find_map(|e| match e {
                Element::Text { text, class } if class.as_deref() == Some("c4-title") => {
                    Some(text.font_size)
                }
                Element::Group { children, .. } => find(children),
                _ => None,
            })
        }
        find(&s.elements).expect("title")
    };
    assert_eq!(title_size(&scene("  theme: neo")), 14.0);
    assert_eq!(title_size(&scene("  theme: redux")), 14.0);
    assert_eq!(title_size(&scene("  theme: default")), 16.0);
}
