//! The root `<svg>` start tag, byte for byte as mmdc (mermaid 12.3.0) writes it:
//! the attribute set and order per diagram type, `xmlns:xlink`, the
//! full-precision `viewBox` and the `max-width` at six significant digits.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn root_tag(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("diagram renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let end = svg.find('>').expect("root tag ends");
    svg[..=end].to_string()
}

#[test]
fn pie_root_tag_matches_mmdc() {
    let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    primaryColor: \"#00ff00\"\n---\npie showData\n    title Base\n    \"x\" : 2\n    \"y\" : 3\n";
    assert_eq!(
        root_tag(src),
        "<svg id=\"my-svg\" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 544.125 450\" style=\"max-width: 544.125px; background-color: white;\" role=\"graphics-document document\" aria-roledescription=\"pie\">"
    );
}

#[test]
fn treemap_root_tag_matches_mmdc() {
    let src = "treemap-beta\n\"Products\"\n    \"Electronics\"\n        \"Phones\": 50\n        \"Computers\": 30\n        \"Accessories\": 20\n    \"Clothing\"\n        \"Men's\": 40\n        \"Women's\": 40\n";
    assert_eq!(
        root_tag(src),
        "<svg id=\"my-svg\" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"2 27 996 371\" style=\"max-width: 996px; background-color: white;\" class=\"flowchart\" role=\"graphics-document document\" aria-roledescription=\"treemap\">"
    );
}

#[test]
fn radar_root_tag_matches_mmdc() {
    let src = "---\nconfig:\n  theme: forest\n---\nradar-beta\n  axis x[\"X axis\"], y[\"Y axis\"]\n  axis z[\"Z axis\"], w[\"W axis\"], v[\"V axis\"], u[\"U axis\"], t[\"T axis\"]\n  curve a{ x: 1, y: 2, z: 3, w: 4, v: 5, u: 6, t: 7 }\n  ticks 7\n";
    assert_eq!(
        root_tag(src),
        "<svg id=\"my-svg\" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" style=\"max-width: 700px; background-color: white;\" viewBox=\"0 0 700 700\" overflow=\"visible\" role=\"graphics-document document\" aria-roledescription=\"radar\">"
    );
}

#[test]
fn journey_root_tag_matches_mmdc() {
    let src = "journey\n    title My working day\n    section Go to work\n      Make tea: 5: Me\n      Go upstairs: 3: Me\n      Do work: 1: Me, Cat\n    section Go home\n      Go downstairs: 5: Me\n      Sit down: 5: Me\n";
    assert_eq!(
        root_tag(src),
        "<svg id=\"my-svg\" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" style=\"max-width: 1300px; background-color: white;\" viewBox=\"0 -25 1300 540\" preserveAspectRatio=\"xMinYMin meet\" height=\"565\" role=\"graphics-document document\" aria-roledescription=\"journey\">"
    );
}

#[test]
fn usecase_root_tag_matches_mmdc() {
    let src = "---\nconfig:\n  theme: dark\n---\nusecase-beta\nactor Ops\nDeploy(\"Deploy service\")\nOps --> Deploy\nnote for Deploy \"Night only\"\njson Env@{ \"region\": \"eu\" }\nDeploy --> Env\n";
    assert_eq!(
        root_tag(src),
        "<svg id=\"my-svg\" width=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" class=\"usecaseDiagram\" style=\"max-width: 558px; --mermaid-usecase-actor-font-size: 14px; --mermaid-usecase-actor-font-family: &quot;Open Sans&quot;, sans-serif; --mermaid-usecase-actor-font-weight: normal; --mermaid-usecase-font-size: 12px; --mermaid-usecase-font-family: &quot;Open Sans&quot;, sans-serif; --mermaid-usecase-font-weight: normal; background-color: white;\" viewBox=\"-8 -8 558 245\" role=\"graphics-document document\" aria-roledescription=\"usecase\">"
    );
}
