//! Web fonts embedded in SVG output (mmdc's `fontEmbedder.js`).
//!
//! The expected `(length, FNV-1a hash)` of each embedded `<style>` element was
//! taken from SVGs rendered by mmdc 12.2.0 (mermaid 12.3.0) for the same
//! diagrams, so the CSS and the base64 woff2 data match mmdc byte for byte.
//! mmdc adds its sheets in the order its `<link>`s finish loading, which varies
//! from run to run; these tests compare the set of sheets with mmdc's and
//! pin this writer's own, fixed order.
#![cfg(feature = "font-embed")]

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325u64, |h, &x| {
        (h ^ u64::from(x)).wrapping_mul(0x100000001b3)
    })
}

fn svg(src: &str, enc: &EncodeOptions) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, enc)
}

/// The `<style>` elements after the first (mermaid's own), as
/// `(byte length, hash)` of their text.
fn embedded(svg: &str) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut rest = svg;
    let mut first = true;
    while let Some(i) = rest.find("<style>") {
        let body = &rest[i + 7..];
        let j = body.find("</style>").expect("closed");
        if !first {
            out.push((j, fnv(&body.as_bytes()[..j])));
        }
        first = false;
        rest = &body[j + 8..];
    }
    out
}

// mmdc's sheets (length in bytes, FNV-1a 64 of the text).
const RECURSIVE: (usize, u64) = (167055, 0xdf7962521fdf9d6e);
const OPEN_SANS_ITALIC: (usize, u64) = (182017, 0x7aedf5d6b60a4d81);
const OPEN_SANS: (usize, u64) = (173485, 0x645d3799d4727222);
const FONTAWESOME: (usize, u64) = (101809, 0x86bcf3b2ac46dc7e);
const FA_REGULAR: (usize, u64) = (33930, 0xb107612dfbcac292);
const FA_SOLID: (usize, u64) = (167230, 0xba2bd7e940f8f3cb);
const FA_BRANDS: (usize, u64) = (181922, 0xbb208d6288ade745);

const FLOW: &str = "flowchart LR\n  A[Hello] --> B[World]\n";
const SEQUENCE: &str = "sequenceDiagram\n  Alice->>Bob: Hi\n";
const FA: &str = "flowchart LR\n  A[fa:fa-car Car] --> B[fa:fa-user User]\n";
const FA_ALL: &str =
    "flowchart LR\n  A[fab:fa-github X] --> B[fas:fa-user Y] --> C[far:fa-user Z]\n";
const FA_BRAND: &str = "flowchart LR\n  A[fab:fa-github X]\n";
const C4: &str = "C4Context\n  Person(a, \"Alice\", \"A user\")\n  System(s, \"Sys\", \"desc\")\n  Rel(a, s, \"uses\")\n";
const GANTT: &str = "gantt\n  dateFormat YYYY-MM-DD\n  section A\n  Task one :a1, 2024-01-01, 5d\n";

fn sorted(mut v: Vec<(usize, u64)>) -> Vec<(usize, u64)> {
    v.sort();
    v
}

#[test]
fn flowchart_and_sequence_embed_recursive_variable() {
    let enc = EncodeOptions::default();
    assert_eq!(embedded(&svg(FLOW, &enc)), [RECURSIVE]);
    assert_eq!(embedded(&svg(SEQUENCE, &enc)), [RECURSIVE]);
    // The classic look too.
    let classic = format!("%%{{init: {{\"look\": \"classic\"}}}}%%\n{FLOW}");
    assert_eq!(embedded(&svg(&classic, &enc)), [RECURSIVE]);
}

#[test]
fn recursive_has_four_subsets_in_mmdcs_order() {
    let s = svg(FLOW, &EncodeOptions::default());
    assert_eq!(s.matches("@font-face").count(), 4);
    let ranges: Vec<&str> = s
        .split("unicode-range: ")
        .skip(1)
        .map(|r| r.split(';').next().unwrap())
        .collect();
    assert!(ranges[0].starts_with("U+460-52F"), "cyrillic-ext first");
    assert!(ranges[1].starts_with("U+102-103"), "vietnamese");
    assert!(ranges[2].starts_with("U+100-2BA"), "latin-ext");
    assert!(ranges[3].starts_with("U+0-FF"), "latin last");
    assert_eq!(s.matches("font-family: \"Recursive Variable\"").count(), 4);
    assert_eq!(s.matches("data:font/woff2;base64,").count(), 4);
    assert_eq!(s.matches("format(\"woff2-variations\")").count(), 4);
}

#[test]
fn c4_embeds_open_sans_italic_then_normal() {
    // C4 text is Open Sans (the root family stays trebuchet ms).
    let s = svg(C4, &EncodeOptions::default());
    assert_eq!(embedded(&s), [OPEN_SANS_ITALIC, OPEN_SANS]);
    assert_eq!(s.matches("@font-face").count(), 20);
    assert!(!s.contains("Recursive Variable"));
}

#[test]
fn only_used_families_are_embedded() {
    // Gantt text is trebuchet ms: no stylesheet of mmdc's has it.
    assert!(embedded(&svg(GANTT, &EncodeOptions::default())).is_empty());
}

#[test]
fn font_awesome_sheets_follow_the_icon_style() {
    let enc = EncodeOptions::default();
    // `fa` / `fas` / `far`: the free family (both its faces) and fontawesome.css.
    let free = svg(FA, &enc);
    assert_eq!(
        sorted(embedded(&free)),
        sorted(vec![RECURSIVE, FONTAWESOME, FA_REGULAR, FA_SOLID])
    );
    // `fab`: the brands family, but not the free one.
    assert_eq!(
        sorted(embedded(&svg(FA_BRAND, &enc))),
        sorted(vec![RECURSIVE, FONTAWESOME, FA_BRANDS])
    );
    let all = svg(FA_ALL, &enc);
    assert_eq!(
        sorted(embedded(&all)),
        sorted(vec![
            RECURSIVE,
            FONTAWESOME,
            FA_REGULAR,
            FA_SOLID,
            FA_BRANDS
        ])
    );
    // The writer's own order: the reverse of the order mmdc requests the sheets in.
    assert_eq!(
        embedded(&all),
        [RECURSIVE, FONTAWESOME, FA_SOLID, FA_REGULAR, FA_BRANDS]
    );
}

#[test]
fn the_font_css_follows_mermaids_style_element() {
    let s = svg(FLOW, &EncodeOptions::default());
    let first = s.find("</style>").unwrap() + "</style>".len();
    assert!(s[first..].starts_with("<style>/* Copyright 2019 The Recursive Project Authors"));
    // The licence comment, then the rules; `&` is escaped as in mmdc's XML.
    assert!(s.contains("PERMISSION &amp; CONDITIONS"));
    // Everything else follows: the `<defs>` / `<g>` of the drawing.
    let end = s[first..].find("</style>").unwrap() + first + "</style>".len();
    assert!(!s[end..].starts_with("<style>"));
}

#[test]
fn no_font_embed_leaves_exactly_the_font_css_out() {
    for src in [FLOW, SEQUENCE, FA_ALL, C4, GANTT] {
        let on = svg(src, &EncodeOptions::default());
        let off = svg(
            src,
            &EncodeOptions {
                font_embed: false,
                ..EncodeOptions::default()
            },
        );
        assert!(!off.contains("@font-face") && !off.contains("data:font"));
        // Cut the added `<style>` elements out of the embedded SVG: the rest is
        // byte for byte the SVG without them.
        let mut stripped = on.clone();
        let first = stripped.find("</style>").unwrap() + "</style>".len();
        let mut tail = first;
        while stripped[tail..].starts_with("<style>/*") {
            tail += stripped[tail..].find("</style>").unwrap() + "</style>".len();
        }
        stripped.replace_range(first..tail, "");
        assert_eq!(stripped, off, "{src}");
        assert!(on.len() >= off.len());
    }
}

#[test]
fn pdf_output_is_not_affected() {
    let r = render::render(FA, &MermaidConfig::default()).unwrap();
    let off = EncodeOptions {
        font_embed: false,
        ..EncodeOptions::default()
    };
    assert_eq!(
        output::encode_pdf(&r, &EncodeOptions::default()),
        output::encode_pdf(&r, &off)
    );
}

#[cfg(feature = "png")]
#[test]
fn png_output_is_not_affected() {
    let r = render::render(FA, &MermaidConfig::default()).unwrap();
    let off = EncodeOptions {
        font_embed: false,
        ..EncodeOptions::default()
    };
    let rast = output::png::Rasterizer::new();
    let a = output::encode_png(&r, &EncodeOptions::default(), &rast).unwrap();
    let b = output::encode_png(&r, &off, &rast).unwrap();
    assert_eq!(a, b);
}
