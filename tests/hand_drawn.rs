//! The handDrawn look, pinned to mermaid 12.3.0 (headless Chromium): every
//! rough.js path starts where mermaid's does. The expected points are the
//! first points of mermaid's paths, in the SVG's frame, written the way our
//! SVG writes numbers; together they cover the nodes' shapes and sizes (the
//! layout), the edges and the clusters.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn assert_starts(src: &str, starts: &[&str]) {
    let s = svg(src);
    let missing: Vec<&&str> = starts.iter().filter(|p| !s.contains(*p)).collect();
    assert!(missing.is_empty(), "paths not drawn from {missing:?}");
}

#[test]
fn flowchart_clusters() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 42
---
flowchart TB
  subgraph one [First]
    a1[A one] --> a2[A two]
  end
  subgraph two [Second]
    b1[B one] --> b2[B two]
  end
  a2 --> b1
  c1 --> a1
"#;
    assert_starts(
        src,
        &[
            "M11.877,381.328",
            "M10.814,380.284",
            "M11.982,116.417",
            "M10.814,115.493",
            "M126.54,218.336",
            "M126.54,483.126",
            "M126.54,296.731",
            "M126.54,65.94",
            "M37.007,166.244",
            "M35.742,165.031",
            "M37.007,244.639",
            "M35.742,243.427",
            "M37.007,431.035",
            "M35.742,429.822",
            "M37.007,509.43",
            "M35.742,508.217",
            "M37.007,13.848",
            "M35.742,12.636",
        ],
    );
}

#[test]
fn flowchart_seed_one() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 1
---
flowchart LR
  A[Seed zero] --> B(Round)
  B --> C{Q}
"#;
    assert_starts(
        src,
        &[
            "M194.84,98.439",
            "M387.608,98.439",
            "M13.161,72.801",
            "M13.892,73.325",
            "M239.683,75.934",
            "M248.567,73.325",
            "M428.951,97.794",
            "M514.741,184.325",
        ],
    );
}

#[test]
fn flowchart_shapes() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 3
---
flowchart TD
  A[Rect] --> B(Round)
  B --> C([Stadium])
  C --> D[[Sub]]
  D --> E[(Cyl)]
  E --> F((Circ))
  F --> G>Asym]
  G --> H{Dia}
  H --> I{{Hex}}
  I --> J[/LeanR/]
  J --> K[\LeanL\]
  K --> L[/Trap\]
  L --> M[\InvTrap/]
  M --> N(((DblCirc)))
"#;
    assert_starts(
        src,
        &[
            "M112.886,66.272",
            "M112.886,159.855",
            "M113.266,236.15",
            "M113.246,314.388",
            "M112.886,434.468",
            "M113.314,523.563",
            "M113.248,599.488",
            "M113.248,890.924",
            "M113.236,970.015",
            "M113.239,1049.681",
            "M113.236,1129.174",
            "M113.249,1223.848",
            "M23.183,13.953",
            "M22.315,13.026",
            "M42.03,111.18",
            "M49.315,107.12",
            "M46.716,206.472",
            "M58.315,200.539",
            "M37.78,280.075",
            "M36.815,279.259",
            "M179.815,279.259",
            "M44.815,279.259",
            "M47.959,368.558",
            "M180.814,371.724",
            "M45.814,371.724",
            "M95.251,484.793",
            "M108.189,475.227",
            "M41.054,565.389",
            "M40.315,564.314",
            "M28.624,728.451",
            "M112.815,813.532",
            "M45.766,856.581",
            "M44.815,891.75",
            "M45.703,936.036",
            "M26.815,971.132",
            "M27.578,1015.759",
            "M44.815,1050.711",
            "M45.703,1095.195",
            "M26.815,1130.291",
            "M12.971,1174.239",
            "M37.315,1224.644",
            "M86.517,1275.616",
            "M110.595,1267.133",
            "M88.854,1280.297",
            "M110.869,1272.101",
        ],
    );
}

#[test]
fn flowchart_elk_dummies() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 5
---
flowchart LR
  x[Outside] --> p1
  subgraph S [Group]
    p1[Inner one] --> p2[Inner two]
  end
  p2 --> y((Out))
  x --> y
"#;
    assert_starts(
        src,
        &[
            "M244.228,12.227",
            "M244.526,12.29",
            "M194.481,117.258",
            "M450.936,87.473",
            "M657.361,87.473",
            "M194.481,128.743",
            "M13.082,97.492",
            "M13.413,97.915",
            "M269.507,61.878",
            "M269.838,62.301",
            "M475.932,61.878",
            "M476.263,62.301",
            "M742.502,101.855",
            "M758.136,99.292",
        ],
    );
}

#[test]
fn flowchart_dagre_clusters() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 21
  layout: dagre
---
flowchart TB
  c1 --> a1
  subgraph one
    a1 --> a2
  end
  subgraph two
    b1 --> b2
  end
  a2 --> b1
  subgraph three
    direction LR
    t1 --> t2
  end
  b2 --> three
"#;
    assert_starts(
        src,
        &[
            "M140.635,371.355",
            "M139.565,371.417",
            "M140.4,112.773",
            "M139.565,112.564",
            "M266.767,62.441",
            "M266.767,191.868",
            "M266.767,450.72",
            "M266.767,296.294",
            "M266.767,555.147",
            "M7.862,630.291",
            "M7.441,630.199",
            "M227.695,692.367",
            "M46.56,666.996",
            "M45.293,667.014",
            "M303.942,666.996",
            "M302.675,667.014",
            "M176.761,9.864",
            "M175.494,9.882",
            "M176.761,139.29",
            "M175.494,139.309",
            "M176.761,243.717",
            "M175.494,243.735",
            "M176.761,398.143",
            "M175.494,398.161",
            "M176.761,502.569",
            "M175.494,502.588",
        ],
    );
}

#[test]
fn state_composite() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 13
---
stateDiagram-v2
  state ch <<choice>>
  state fork_state <<fork>>
  [*] --> First
  state First {
    [*] --> second
    second --> [*]
  }
  First --> ch
  ch --> A: yes
  ch --> B: no
  A --> fork_state
  B --> fork_state
  fork_state --> C
  note right of C : a note here
  C --> [*]
"#;
    assert_starts(
        src,
        &[
            "M107.158,65.207",
            "M104.113,65.849",
            "M93.907,88.959",
            "M93.856,89.008",
            "M187.561,25.89",
            "M187.561,127.367",
            "M187.31,191.037",
            "M187.31,253.089",
            "M185.861,320.314",
            "M189.205,318.216",
            "M98.546,462.844",
            "M276.255,462.844",
            "M187.31,515.329",
            "M167.782,595.196",
            "M207.019,595.196",
            "M173.687,307.235",
            "M186.973,321.742",
            "M152.69,503.814",
            "M151.967,504.327",
            "M188.928,25.098",
            "M188.444,12.489",
            "M188.928,126.576",
            "M188.444,113.967",
            "M123.079,155.934",
            "M130.967,152.788",
            "M180.66,219.876",
            "M188.444,215.654",
            "M188.21,219.726",
            "M188.041,220.011",
            "M34.224,427.798",
            "M42.113,424.652",
            "M211.933,427.798",
            "M219.822,424.652",
            "M123.079,560.149",
            "M130.967,557.003",
            "M37.351,695.041",
            "M36.779,695.381",
            "M255.849,650.091",
            "M263.633,645.869",
            "M263.399,649.941",
            "M263.23,650.226",
        ],
    );
}

#[test]
fn state_regions_dark() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 99
  theme: dark
---
stateDiagram-v2
  direction LR
  [*] --> Idle
  Idle --> Running : start
  Running --> Idle : stop
  state Running {
    [*] --> Fast
    Fast --> Slow
    --
    [*] --> Hot
  }
  classDef warn fill:#fa0,stroke:#333
  class Idle warn
"#;
    assert_starts(
        src,
        &[
            "M326.757,9.79",
            "M325.928,11.647",
            "M316.964,36.74",
            "M316.128,37.515",
            "M340.179,216.72",
            "M339.829,217.062",
            "M340.179,62.37",
            "M339.829,62.712",
            "M26.503,225.208",
            "M204.438,221.425",
            "M315.656,293.001",
            "M432.415,256.111",
            "M432.669,323.527",
            "M432.415,101.761",
            "M22.454,231.153",
            "M18.634,218.705",
            "M68.499,207.1",
            "M72.864,204.977",
            "M435.741,254.767",
            "M431.921,242.319",
            "M365.651,283.888",
            "M370.017,281.766",
            "M365.651,350.949",
            "M370.017,348.827",
            "M435.741,100.417",
            "M431.921,87.97",
            "M365.651,129.539",
            "M370.017,127.417",
        ],
    );
}

#[test]
fn class_dark() {
    let src = r#"---
config:
  look: handDrawn
  handDrawnSeed: 1
  theme: dark
---
classDiagram
  class Shape {
    <<interface>>
    +area() double
  }
  Shape <|.. Circle
  Circle *-- Point
  Circle o-- Style
"#;
    assert_starts(
        src,
        &[
            "M102.661,183.193",
            "M93.795,310.05",
            "M111.048,310.05",
            "M14.206,13.483",
            "M14.508,14.502",
            "M13.137,86.502",
            "M13.137,110.502",
            "M68.013,207.397",
            "M68.769,207.892",
            "M67.723,255.892",
            "M67.723,273.892",
            "M17.442,334.19",
            "M17.848,335.088",
            "M16.496,383.088",
            "M16.496,401.088",
            "M124.252,334.553",
            "M125.034,335.018",
            "M123.962,383.018",
            "M123.962,401.018",
        ],
    );
}

/// mermaid's `handDrawnSeed` defaults to 0, for which rough.js draws from
/// `Math.random()`; a zero seed draws exactly what seed 1 draws.
#[test]
fn zero_seed_draws_as_seed_one() {
    let body = "flowchart LR\n  A[Seed zero] --> B(Round)\n  B --> C{Q}\n";
    let zero = svg(&format!("---\nconfig:\n  look: handDrawn\n---\n{body}"));
    let one = svg(&format!(
        "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 1\n---\n{body}"
    ));
    assert_eq!(zero, one);
}

/// The other looks keep their plain shapes.
#[test]
fn classic_look_draws_no_rough_paths() {
    let s = svg("flowchart LR\n  A --> B\n");
    assert!(s.contains("<rect"), "classic nodes stay rects");
}
