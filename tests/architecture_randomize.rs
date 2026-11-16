//! `architecture.randomize: true`: fcose starts from its spectral draft
//! instead of the origin. The expected icon positions are the `translate`s
//! mermaid 12.3 renders for these diagrams (headless Chromium), written
//! the way our SVG writes numbers.

use mmdc::config::MermaidConfig;
use mmdc::output::svg::num;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

const BODY: &str = "architecture-beta
    group g1(cloud)[G1]
    group g2(cloud)[G2]
    service a(server)[A] in g1
    service b(server)[B] in g1
    service c(server)[C] in g2
    service d(server)[D] in g2
    service e(server)[E]
    a:R -- L:b
    c:T -- B:d
";

/// mermaid's positions for `BODY` with `randomize: true`.
const RANDOMIZED: [(&str, f64, f64); 5] = [
    ("a", 40.351070802823415, 14.00674526082635),
    ("b", 241.0376365640085, 14.00674526082635),
    ("c", -241.0376365640085, 51.606121790450715),
    ("d", -241.0376365640085, -149.08044397073428),
    ("e", -41.48505813605246, 222.58044397073428),
];

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

/// The icon's top left corner of service `id`: `(x, y)` as written.
fn icon(svg: &str, id: &str) -> (String, String) {
    let key =
        format!("<g id=\"service-{id}\" class=\"architecture-service\"><rect class=\"node-bkg\" ");
    let at = svg.find(&key).expect("service") + key.len();
    let tag = &svg[at..at + svg[at..].find('>').expect("tag end")];
    let attr = |name: &str| {
        let k = format!("{name}=\"");
        let s = tag.find(&k).expect("attribute") + k.len();
        tag[s..s + tag[s..].find('"').expect("quote")].to_string()
    };
    (attr("x"), attr("y"))
}

fn assert_randomized(svg: &str) {
    for (id, x, y) in RANDOMIZED {
        assert_eq!(icon(svg, id), (num(x), num(y)), "service {id}");
    }
}

#[test]
fn frontmatter_randomize_lays_out_like_mermaid() {
    let src = format!("---\nconfig:\n  architecture:\n    randomize: true\n---\n{BODY}");
    assert_randomized(&svg(&src));
}

#[test]
fn init_directive_randomize_lays_out_like_mermaid() {
    let src = format!("%%{{init: {{\"architecture\": {{\"randomize\": true}}}}}}%%\n{BODY}");
    assert_randomized(&svg(&src));
}

#[test]
fn site_config_randomize_lays_out_like_mermaid() {
    let j = mmdc::json::parse(r#"{"architecture": {"randomize": true}}"#).unwrap();
    let cfg = MermaidConfig::default().with_json(&j).unwrap();
    let r = render::render(BODY, &cfg).expect("renders");
    assert_randomized(&output::encode_svg(&r, &EncodeOptions::default()));
}

#[test]
fn randomize_defaults_to_false() {
    let default = svg(BODY);
    let off = svg(&format!(
        "---\nconfig:\n  architecture:\n    randomize: false\n---\n{BODY}"
    ));
    let strip = |s: &str| s[s.find("<g>").expect("body")..].to_string();
    assert_eq!(strip(&default), strip(&off));
    // mermaid's default layout of BODY puts `a` elsewhere.
    assert_ne!(
        icon(&default, "a"),
        (num(RANDOMIZED[0].1), num(RANDOMIZED[0].2))
    );
}
