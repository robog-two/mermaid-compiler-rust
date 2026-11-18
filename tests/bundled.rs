//! The bundled assets end to end: Font Awesome icons in labels, the web fonts
//! mmdc loads (Recursive Variable, Open Sans) and the TeX Gyre Heros fallback
//! font in PDF (and PNG) output.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn rendered(src: &str) -> render::Rendered {
    render::render(src, &MermaidConfig::default()).expect("diagram renders")
}

#[cfg(feature = "icons")]
fn svg(src: &str) -> String {
    output::encode_svg(&rendered(src), &EncodeOptions::default())
}

fn pdf(src: &str) -> Vec<u8> {
    output::encode_pdf(&rendered(src), &EncodeOptions::default())
}

#[cfg(feature = "icons")]
#[test]
fn font_awesome_icons_are_drawn_in_html_labels() {
    let s = svg("flowchart LR\n A[fa:fa-car Car] -->|fab:fa-github| B[far:fa-user]");
    assert_eq!(s.matches("class=\"fa-icon\"").count(), 3);
    assert!(!s.contains("fa:fa-car") && !s.contains('\u{2022}'));
    // The icon sits 1.25em wide before the text: 16px · 1.25 at the default 14px is 17.5.
    assert!(s.contains("xml:space=\"preserve\"> Car</text>"));
    // SVG labels keep the reference as text, as mermaid's createText does.
    let s = svg("%%{init: {\"htmlLabels\": false}}%%\nflowchart LR\n A[fa:fa-car Car]");
    assert!(s.contains("fa:fa-car") && !s.contains("fa-icon"));
}

/// Swimlane labels go through the flowchart's `createText`: nodes, edge labels
/// and lane titles draw their icons too.
#[cfg(feature = "icons")]
#[test]
fn font_awesome_icons_are_drawn_in_swimlane_labels() {
    let s = svg(
        "swimlane-beta TB\nsubgraph A[fa:fa-user Customer]\n a[fa:fa-car Drive car]\nend\nsubgraph B\n c[Plain]\nend\na -->|fa:fa-ban Stop| c\n",
    );
    assert_eq!(s.matches("class=\"fa-icon\"").count(), 3);
    assert!(!s.contains("fa:fa-"));
}

#[cfg(feature = "icons")]
#[test]
fn icons_take_their_fixed_width() {
    use mmdc::icons::replace_references;
    use mmdc::text::line_width;
    // Every icon is FA 7's 1.25em box, whatever its glyph: 17.5px at 14px.
    for icon in [
        "fa:fa-car",
        "fab:fa-github",
        "fa:fa-i",
        "fa:fa-nonexistent",
        "far:fa-car",
    ] {
        let w = line_width(&replace_references(&format!("{icon} x")), 14.0, false);
        assert!(
            (w - 17.5 - line_width(" x", 14.0, false)).abs() < 1e-9,
            "{icon}"
        );
    }
    // Pro styles have no CSS: nothing at all.
    assert_eq!(replace_references("fal:fa-car x"), " x");
}

#[cfg(feature = "icons")]
#[test]
fn icons_reach_the_pdf_as_filled_outlines() {
    let p = String::from_utf8_lossy(&pdf(
        "---\nconfig:\n  theme: default\n---\nflowchart LR\n A[fa:fa-car Car]",
    ))
    .into_owned();
    // One transformed outline (the classic theme's 16px: scale 16/512) filled
    // in the text colour.
    assert!(p.contains("q 0.0312500 0 0 0.0312500 "), "{p}");
    assert!(p.contains("/F1 1 Tf ( Car) Tj"));
}

#[cfg(feature = "fonts")]
#[test]
fn pdf_embeds_the_bundled_font_only_for_text_helvetica_lacks() {
    // The classic theme's labels are Arial-measured, drawn in Helvetica.
    let classic = "---\nconfig:\n  theme: default\n---\n";
    let latin = pdf(&format!(
        "{classic}flowchart LR\n A[Café] --> B[“quoted” – ok]"
    ));
    assert!(latin.starts_with(b"%PDF-1.4"));
    assert!(!latin.windows(9).any(|w| w == b"FontFile3"));

    // Polish, Greek and Vietnamese letters (TeX Gyre Heros has no Cyrillic or CJK).
    let bytes = pdf(&format!(
        "{classic}flowchart LR\n A[Łódź → Ω] --> B[\"`**Ωμγα** and *ơi*`\"]"
    ));
    assert!(bytes.starts_with(b"%PDF-1.6"));
    let text = String::from_utf8_lossy(&bytes);
    for needle in [
        "/BaseFont /TeXGyreHeros-Regular",
        "/BaseFont /TeXGyreHeros-Bold",
        "/BaseFont /TeXGyreHeros-Italic",
        "/FontFile3",
        "/Subtype /OpenType",
        "/ToUnicode",
        "/H1 1 Tf <",
        // Latin text in the same label stays in Helvetica.
        "/F3 1 Tf (",
    ] {
        assert!(text.contains(needle), "{needle}");
    }
}

/// The redux themes' labels are Recursive Variable, embedded as TrueType with
/// each glyph at its measured advance; C4's Open Sans too.
#[cfg(feature = "fonts")]
#[test]
fn pdf_embeds_the_web_fonts_for_their_text() {
    let bytes = pdf("flowchart LR\n A[\"`AV **To** *Ty*`\"] --> B[Łódź]");
    assert!(bytes.starts_with(b"%PDF-1.4"));
    let text = String::from_utf8_lossy(&bytes);
    for needle in [
        "/BaseFont /RecursiveVariable-Regular",
        "/BaseFont /RecursiveVariable-Bold",
        "/BaseFont /RecursiveVariable-Italic",
        "/Subtype /CIDFontType2",
        "/CIDToGIDMap /Identity",
        "/FontFile2",
        "] TJ",
    ] {
        assert!(text.contains(needle), "{needle}");
    }
    // Everything Recursive draws is drawn in it: no Helvetica text.
    assert!(!text.contains("/F1 1 Tf ("), "{text}");
    let c4 = String::from_utf8_lossy(&pdf("C4Context\n Person(a, \"Customer\", \"A customer\")"))
        .into_owned();
    assert!(c4.contains("/BaseFont /OpenSans-Bold"));
    assert!(c4.contains("/BaseFont /OpenSans-Regular"));
}

#[cfg(not(feature = "fonts"))]
#[test]
fn without_bundled_fonts_pdf_text_outside_windows_1252_is_replaced() {
    let text = String::from_utf8_lossy(&pdf(
        "---\nconfig:\n  theme: default\n---\nflowchart LR\n A[Łódź]",
    ))
    .into_owned();
    assert!(text.contains("(?\\363d?)"));
}
