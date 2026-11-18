//! `swimlane-beta` under its own layout engine, pinned to what mermaid 12.3.0
//! renders for these diagrams (headless Chromium, with the fonts mmdc loads):
//! coordinates as mermaid
//! writes them, to its 3 decimals.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;

fn svg(src: &str) -> String {
    let r = render::render(src, &MermaidConfig::default()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn view_box(svg: &str) -> &str {
    let at = svg.find("viewBox=\"").expect("viewBox") + 9;
    &svg[at..at + svg[at..].find('"').expect("closing quote")]
}

/// The path of the first edge drawn from `from` to `to`, and its dash pattern.
fn edge(svg: &str, from: &str, to: &str) -> (String, Option<String>) {
    let key = format!("<g id=\"L-{from}-{to}-");
    let at = svg.find(&key).expect("edge group");
    let tag_at = at + svg[at..].find("<path").expect("path");
    let tag = &svg[tag_at..tag_at + svg[tag_at..].find('>').expect("tag end")];
    let attr = |name: &str| {
        let key = format!(" {name}=\"");
        tag.find(&key).map(|i| {
            let s = &tag[i + key.len()..];
            s[..s.find('"').expect("closing quote")].to_string()
        })
    };
    (attr("d").expect("d"), attr("stroke-dasharray"))
}

const SUPPORT: &str = r#"swimlane-beta LR
  subgraph Customer
    request[Request service]
    receive[Receive update]
  end
  subgraph Support
    triage[Triage request]
    answer[Send answer]
  end
  subgraph Engineering
    investigate[Investigate issue]
    fix[Prepare fix]
  end
  request --> triage
  triage -->|Known issue| answer
  triage -->|Needs code change| investigate
  investigate --> fix --> answer
  answer --> receive
"#;

/// Lanes stacked top to bottom with rotated titles; hand-offs run straight
/// across the lanes, and each arrow ends 4px short of its node.
#[test]
fn lr_lanes_and_routes() {
    let s = svg(SUPPORT);
    assert_eq!(view_box(&s), "-94 -66.5 818.34 817.5");
    assert_eq!(edge(&s, "request", "triage").0, "M36,22.5L36,225.5");
    assert_eq!(edge(&s, "triage", "answer").0, "M112,252L550.34,252");
    assert_eq!(edge(&s, "fix", "answer").0, "M630.34,651.5L630.34,278.5");
    assert_eq!(edge(&s, "answer", "receive").0, "M630.34,229.5L630.34,26.5");
}

/// Under BT the lane titles get no strip (height 0), and their labels stand
/// half above the lanes: the label boxes count towards the viewBox
/// (`getBBox`), whose top is the label's top less the padding.
#[test]
fn a_lane_label_outside_its_squeezed_title_strip_counts_in_the_view_box() {
    let s = svg(r#"---
config:
  look: neo
---
swimlane-beta BT
  subgraph L0[Lane 0]
    n00[Pay plan]
    n01[Plan test]
    n02[/Deploy/]
  end
  subgraph L1[Lane 1]
    n10[/Plan/]
  end
  subgraph L2[Lane 2]
    n20{Approve}
    subgraph S2[Group 2]
      s20[Pay]
      s21[Deploy]
      subgraph T2[Inner 2]
        t2[Submit]
      end
    end
  end
  subgraph L3[Lane 3]
    n30[[Ship build]]
    n31[[Archive]]
  end
  n01 ==> n10
  n10 --> t2
  t2 --> s20
  n01 --- s21
  t2 ==> n02
  n01 --> s21
  n02 --> n31
  n02 --> t2
  n31 ==> t2
  n02 --> n30
  classDef hot fill:#f96,stroke:#333,stroke-width:2px
  class n01 hot
"#);
    assert_eq!(view_box(&s), "-561.5 4.5 1123 558.5");
    assert!(s.contains(r#"<rect class="swimlane-title" x="289.5" y="23" width="264" height="0""#));
}

/// The untitled default lane is drawn as mermaid draws it: a body and a title
/// strip of height 0.
#[test]
fn the_default_lane_has_a_zero_height_title_strip() {
    let s = svg("swimlane-beta\nsubgraph A\na\nend\na-->b\n");
    assert!(s.contains(
        r#"<g id="__swimlane_default__" class="cluster swimlane"><rect class="swimlane-body" x="-252" y="-36" width="252" height="117""#
    ));
    assert!(s.contains(r#"<rect class="swimlane-title" x="-252" y="-36" width="252" height="0""#));
}

/// `applyMarkerOffsetsToPoints` moves an arrow's end 4px back along its last
/// segment however short that is: here 2.5px, so the line overshoots back past
/// the bend before it.
#[test]
fn an_arrow_end_moves_back_past_a_short_last_segment() {
    let s = svg(r#"swimlane-beta TB
  subgraph L0[Lane 0]
    n00((Ship))
    n01[(Review deploy)]
    subgraph S0[Group 0]
      s00[Approve]
      s01[Submit]
    end
  end
  subgraph L1[Lane 1]
    n10[/Notify/]
    n11[Test]
  end
  subgraph L2[Lane 2]
    n20([Notify])
    n21{{Pay}}
  end
  subgraph L3
    n30>Archive]
    n31[(Check ship)]
    n32((Plan))
  end
  subgraph L4[Lane 4]
    n40[[Plan]]
    n41[Approve]
  end
  subgraph L5[Lane 5]
    n50[Deploy]
    subgraph S5[Group 5]
      s50[Ship]
    end
  end
  n32 --> n20
  s01 --> n01
  n31 --o n40
  n32 --> n11
  s01 --o n32
  n11 --x n10
  n40 ---|Archive 1| n21
  n32 --> n00
  n11 --- n10
  s01 --x n21
  s50 --> n30
  n00 ---|Test 2| n40
  n40 -.-> n01
  n11 --o|Pay 3| n01
  s01 ==>|Archive 4| n01
  n00 -.-> n50
  n11 -.-> n21
  classDef hot fill:#f96,stroke:#333,stroke-width:2px
  class s00 hot
"#);
    let (d, _) = edge(&s, "n32", "n11");
    assert!(d.starts_with("M294.444,202.372L294.644,203.057C"), "{d}");
    assert!(d.ends_with("L-455.875,149.446L-455.875,150.946"), "{d}");
}

/// A nearly level edge (from a label's anchor) hops the vertical edge it
/// crosses, an arc between the points 6px either side of the crossing.
#[test]
fn a_slanted_segment_hops_a_crossing() {
    let s = svg(r#"swimlane-beta LR
  subgraph L0[Lane 0]
    a00>Submit]
    a01>Approve notify]
    a02[Check]
    a03{{Submit}}
    subgraph G0[Group 0]
      g00[Pay]
      g01[Approve]
    end
  end
  subgraph L1[Lane 1]
    a10[[Plan]]
    a11[[Pay]]
    a12([Check])
    a13[Pay test]
    subgraph G1[Group 1]
      g10[Build]
      g11[Archive]
      g12[Notify]
    end
  end
  subgraph L2[Lane 2]
    a20[\Pay\]
  end
  subgraph L3[Lane 3]
    a30[Test archive]
    a31[Review]
    a32[/Ship\]
    a33[[Review approve]]
  end
  g11 -->|Deploy 1| a12
  a20 --> a02
  a03 --> a30
  g10 ~~~ g12
  a10 ==> g10
  a11 --o g00
  g01 --o a30
  g00 --x a10
  a03 -->|Submit the check step 3 now| a13
  a02 --x a30
  a30 ---|Build 4| g00
  g10 --o a00
  a30 -.->|Approve 5| a33
  g10 <--> a12
  a32 --> a13 --> a20
  g01 -->|Build 6| a13
  a00 --x|Escalate 7| a02
  a33 ==> g10
  g11 -->|Archive 8| a33
  g11 --> a33
  a03 -->|Plan the ship step 9 now| g01
  a03 --> g01
  a33 --> a01
"#);
    let (d, _) = edge(&s, "a03", "g01");
    assert!(d.starts_with("M132.589,-9.469L749.174,-9.938C"), "{d}");
    assert!(d.ends_with("761.174,-9.948L826.175,-9.997"), "{d}");
}

/// mermaid writes a hop as `A5.009,5.009 0 0 1 523.435,508.569` from
/// `513.418,508.569`: rounded to 3 decimals, its ends are a hair closer than
/// its diameter, so the browser draws a little less than a semicircle (apex
/// at 503.631, not 503.56), and the neo dash pattern is measured along that.
#[test]
fn a_hop_is_drawn_from_its_rounded_numbers() {
    let s = svg(r#"swimlane-beta RL
  subgraph L0[Lane 0]
    a00((Check review))
    a01(((Plan)))
    a02>Approve]
  end
  subgraph L1[Lane 1]
    a10{Submit}
    a11((Submit))
  end
  loose(Loose)
  a00 --> a10
  a10 --o|Plan 1| a00
  a01 --x|Submit 2| loose
  a10 --> a00 --> a02
  a10 --x loose
  a00 --> a02 --> a11
  a10 --- loose
  a00 -.-> a01
"#);
    let (d, dash) = edge(&s, "a10", "a00");
    assert!(
        d.contains("L511.163,525.386C511.163,522.675,513.36,520.478,516.071,520.478C518.782,520.478,520.979,522.675,520.979,525.386L522.979,525.386"),
        "{d}"
    );
    // Chromium measures 221.93884 (its float path measure; mermaid 12.3.0 with
    // mmdc's fonts).
    assert_eq!(dash.as_deref(), Some("0 0 221.938 12.5"));
}

/// Neo heads hide the line's end under them by an inline pattern: 12.5px for a
/// circle or cross, 4px for an arrow, with 2px dashes for a dotted line.
#[test]
fn neo_heads_hide_the_line_end_by_a_pattern() {
    let s = svg(r#"---
config:
  look: neo
---
swimlane-beta
subgraph L
  a
  b
end
a --o b
b --x c
c -.-> a
"#);
    assert_eq!(
        edge(&s, "a", "b"),
        ("M126,45L126,95".into(), Some("0 0 37.5 12.5".into()))
    );
    assert_eq!(
        edge(&s, "b", "c"),
        ("M50,117.5L-50,117.5".into(), Some("0 0 87.5 12.5".into()))
    );
    let dash = edge(&s, "c", "a").1.expect("a pattern");
    assert!(
        dash.starts_with("0 0 2 2 2 ") && dash.ends_with(" 2 4"),
        "{dash}"
    );
}

/// mermaid 12.3.0, `swimlane-beta` with `layout: dagre`: the flowchart
/// renderer under dagre, the lanes as subgraphs.
#[test]
fn under_dagre_the_lanes_are_subgraphs() {
    let s = svg("---\nconfig:\n  layout: dagre\n---\nswimlane-beta LR\n  \
                 subgraph Applicant\n    apply[Submit application]\n    sign[Sign agreement]\n  end\n  \
                 subgraph Reviewer\n    screen[Screen application]\n    decide{Approved?}\n  end\n  \
                 subgraph System\n    create[Create account]\n    notify[Send welcome email]\n  end\n  \
                 apply -->|Application received| screen\n  screen --> decide\n  \
                 decide -->|Approved| create --> notify --> sign\n  \
                 decide -->|Needs changes| apply\n");
    assert_eq!(
        edge(&s, "screen", "decide").0,
        "M554.391,128.5L558.557,128.5C562.724,128.5,571.057,128.5,578.724,128.5C586.391,128.5,593.391,128.5,596.891,128.5L600.391,128.5"
    );
}
