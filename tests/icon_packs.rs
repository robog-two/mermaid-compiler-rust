//! Icon packs (`--iconPacks`, `--iconPacksNamesAndUrls`) and images, pinned
//! to mmdc 12 (mermaid 12.3.0, headless Chromium) given the hand-made
//! Iconify set below: where each icon's `<svg>` lands (mermaid centres an
//! icon's drawn content, its `getBBox()`), the viewBoxes that content
//! decides, and images in PNG and PDF output.

use mmdc::config::MermaidConfig;
use mmdc::iconify::IconPacks;
use mmdc::output::{self, EncodeOptions};
use mmdc::render::{self, RenderEnv};

/// An Iconify JSON set exercising set defaults, `left` / `top`, aliases with
/// flips and rotations, ids and an icon drawing past its own box.
pub const TST: &str = r##"{
  "prefix": "tst", "width": 16, "height": 12,
  "icons": {
    "box": { "body": "<rect x=\"2\" y=\"2\" width=\"12\" height=\"8\" fill=\"currentColor\"/>" },
    "arrow": { "body": "<path d=\"M2 6h8V3l4 5l-4 5v-3H2z\" fill=\"#e33\"/>", "left": -2, "top": 1, "width": 18, "height": 14 },
    "grad": { "body": "<defs><linearGradient id=\"g\" x1=\"0\" x2=\"1\" y1=\"0\" y2=\"0\"><stop offset=\"0\" stop-color=\"#f80\"/><stop offset=\"1\" stop-color=\"#08f\"/></linearGradient></defs><ellipse cx=\"8\" cy=\"6\" rx=\"7\" ry=\"5\" fill=\"url(#g)\"/>" },
    "tall": { "body": "<rect x=\"1\" y=\"1\" width=\"6\" height=\"18\" rx=\"2\" fill=\"#a3a\"/>", "width": 8, "height": 20 },
    "cubic": { "body": "<path d=\"M1 11C3 -4 13 -4 15 11S20 15 8 9c-3-2-5 0-7 2Z\" fill=\"currentColor\" fill-opacity=\".6\"/>" }
  },
  "aliases": {
    "arrow-left": { "parent": "arrow", "hFlip": true },
    "arrow-down": { "parent": "arrow", "rotate": 1 },
    "arrow-up-flipped": { "parent": "arrow-down", "vFlip": true, "hFlip": true },
    "arrow-180": { "parent": "arrow", "rotate": 2 },
    "arrow-r3": { "parent": "arrow", "rotate": 3 },
    "arrow-vflip": { "parent": "arrow", "vFlip": true },
    "box-wide": { "parent": "box", "width": 24 },
    "box-alias2": { "parent": "box-wide", "rotate": 1 }
  }
}"##;

fn config() -> MermaidConfig {
    MermaidConfig {
        icon_packs: IconPacks::new(vec![("tst".into(), Ok(TST.into()))]),
        ..MermaidConfig::default()
    }
}

fn svg(src: &str) -> String {
    let r = render::render(src, &config()).expect("renders");
    output::encode_svg(&r, &EncodeOptions::default())
}

fn view_box(s: &str) -> &str {
    let i = s.find("viewBox=\"").expect("viewBox") + 9;
    &s[i..i + s[i..].find('"').expect("closing quote")]
}

/// The `translate(x,y)` and `viewBox` of each inline icon, in order.
fn icons(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("<g transform=\"translate(") {
        rest = &rest[i + 24..];
        let t = rest[..rest.find(')').unwrap()].to_string();
        let head = &rest[..rest.find('>').unwrap()];
        let after = &rest[head.len()..];
        // `<g transform style><g>[<g fill …>]<svg …>`
        let inner = after.trim_start_matches('>').trim_start_matches("<g>");
        let inner = if inner.starts_with("<g fill") {
            &inner[inner.find('>').unwrap() + 1..]
        } else {
            inner
        };
        if let Some(svg) = inner.strip_prefix("<svg ") {
            let vb = &svg[svg.find("viewBox=\"").unwrap() + 9..];
            out.push((t, vb[..vb.find('"').unwrap()].to_string()));
        }
    }
    out
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn flowchart_icons_take_flips_rotations_and_offsets() {
    let s = svg(r#"flowchart LR
  A@{ icon: "tst:arrow", form: "square", label: "arrow" }
  B@{ icon: "tst:arrow-left", form: "square", label: "hFlip" }
  C@{ icon: "tst:arrow-down", form: "square", label: "rot1" }
  D@{ icon: "tst:arrow-up-flipped", form: "square", label: "rot1+hv" }
  E@{ icon: "tst:arrow-180", form: "square", label: "rot2" }
  F@{ icon: "tst:arrow-r3", form: "square", label: "rot3" }
  G@{ icon: "tst:arrow-vflip", form: "square", label: "vFlip" }
  H@{ icon: "tst:box-wide", form: "square", label: "wide" }
  I@{ icon: "tst:box-alias2", form: "square", label: "wide rot" }
  A --> B --> C --> D --> E
  F --> G --> H --> I
"#);
    assert_eq!(view_box(&s), "4 4 491 248");
    assert_eq!(
        icons(&s),
        pairs(&[
            ("16.833,19.5", "-2 1 18 14"),
            ("125.167,19.5", "0 0 18 14"),
            ("225.5,16.833", "1 -2 14 18"),
            ("328.5,22.167", "1 -2 14 18"),
            ("434.167,19.5", "-2 1 18 14"),
            ("19.5,158.167", "1 -2 14 18"),
            ("119.833,155.5", "0 0 18 14"),
            ("233.5,155.5", "0 0 24 12"),
            ("328.5,163.5", "0 0 12 24"),
        ])
    );
    // iconToSVG's transformations, the node's border as `currentColor`.
    assert!(s.contains("<g transform=\"translate(16 -1) scale(-1 1)\">"));
    assert!(s.contains("<g transform=\"rotate(90 8 8)\">"));
    assert!(s.contains("style=\"color: #28253D;\""));
}

#[test]
fn icon_content_sizes_circles_and_node_boxes() {
    // iconCircle rings the drawn content; an icon drawing past its square
    // grows the node's measured box (mermaid lays out the group's getBBox()).
    let s = svg(r#"flowchart LR
  A@{ icon: "tst:tall", form: "circle", label: "Tall" }
  B@{ icon: "tst:cubic", label: "Over", pos: "t" }
  C@{ icon: "tst:box", form: "circle" }
  A --> B --> C
"#);
    assert_eq!(view_box(&s), "4.027 4 336.12 150.094");
    assert_eq!(
        icons(&s),
        pairs(&[
            ("38.574,38.547", "0 0 8 20"),
            ("150.352,62.109", "0 0 16 12"),
            ("262.633,55.047", "0 0 16 12"),
        ])
    );
}

#[test]
fn architecture_icons_from_packs_have_no_background() {
    let s = svg(r#"architecture-beta
    group g(tst:grad)[Group]
    service a(tst:tall)[A] in g
    service b(tst:arrow-left)[B] in g
    service c(database)[C]
    a:R -- L:b
    b:R -- L:c
"#);
    // The viewBox takes in what the icons draw, not their squares.
    assert_eq!(view_box(&s), "-276.33 -65.5 628.659 262");
    assert_eq!(
        icons(&s),
        pairs(&[
            ("-235.33,-24.5", "0 0 16 12"),
            ("-193.83,17", "0 0 8 20"),
            ("6.843,17", "0 0 18 14"),
        ])
    );
    // The built-in database keeps its blue square; the pack icons do not get one.
    assert_eq!(s.matches("fill=\"#087ebf\"").count(), 1);
    // replaceIDs: the gradient's id and its reference are renamed together.
    let id = s.split("<linearGradient id=\"").nth(1).unwrap();
    let id = &id[..id.find('"').unwrap()];
    assert!(id.starts_with("IconifyId"), "{id}");
    assert!(s.contains(&format!("fill=\"url(#{id})\"")));
}

#[test]
fn tree_view_and_usecase_icons() {
    let s = svg(
        "treeView-beta\n├── src/\n│   ├── a.ts icon(tst:box)\n│   └── b.ts icon(tst:arrow-down)\n└── c.md icon(tst:missing)\n",
    );
    assert_eq!(
        icons(&s),
        pairs(&[("35,59.031", "0 0 16 12"), ("35,86.047", "1 -2 14 18")])
    );
    // An icon no pack has is mermaid's unknownIcon.
    assert!(s.contains("#087ebf"));
    let s = svg(
        "usecase-beta\nactor Icon(\"Arrow\")@{ icon: \"tst:arrow\" }\nactor Box(\"Box\")@{ icon: \"tst:box\" }\nIcon --> Manage\nBox --> Manage\n",
    );
    assert_eq!(view_box(&s), "-8 -8 316 322");
    assert_eq!(
        icons(&s),
        pairs(&[("26.667,35", "-2 1 18 14"), ("29,196", "0 0 16 12")])
    );
    // The glyph group's fill and 2px stroke reach the icon's elements.
    assert!(s.contains("<g fill=\"#F5F3FF\" stroke=\"#A78BFA\" stroke-width=\"2\"><svg"));
}

#[test]
fn without_packs_icons_stay_unknown() {
    let src = "flowchart LR\n  A@{ icon: \"tst:box\", form: \"square\", label: \"Box\" }\n";
    let plain = render::render(src, &MermaidConfig::default()).unwrap();
    let s = output::encode_svg(&plain, &EncodeOptions::default());
    assert!(icons(&s).is_empty());
    assert!(s.contains("#087ebf"));
    // A pack that failed to load draws unknown icons too.
    let failed = MermaidConfig {
        icon_packs: IconPacks::new(vec![("tst".into(), Err("offline".into()))]),
        ..MermaidConfig::default()
    };
    let r = render::render(src, &failed).unwrap();
    assert_eq!(output::encode_svg(&r, &EncodeOptions::default()), s);
}

/// A pack registered as `fa`: mermaid's `replaceIconSubstring` draws its icon
/// for `fa:fa-home` (`getIconSVG(…, { class: "label-icon" })`: 1em high, its
/// aspect ratio wide, its bottom 0.125em below the baseline). Checked against
/// mmdc with mdi registered as `fa`: the icon at y 26.75, 14 × 14.
#[test]
fn fa_references_draw_a_pack_named_fa() {
    const FA: &str = r##"{ "prefix": "fa", "width": 24, "height": 24, "icons": {
      "home": { "body": "<path d=\"M10 20v-6h4v6h5v-8h3L12 3L2 12h3v8z\" fill=\"#123456\"/>" },
      "wide": { "body": "<rect width=\"30\" height=\"20\" fill=\"currentColor\"/>", "width": 30, "height": 20 } } }"##;
    let cfg = MermaidConfig {
        icon_packs: IconPacks::new(vec![("fa".into(), Ok(FA.into()))]),
        ..MermaidConfig::default()
    };
    let src = "flowchart LR\n  A[\"fa:fa-home Home\"] -- fa:fa-home edge --> B[\"fa:fa-car car\"]\n  B --> C[\"fa:fa-wide w\"]\n";
    let r = render::render(src, &cfg).unwrap();
    let s = output::encode_svg(&r, &EncodeOptions::default());
    let found = icons(&s);
    assert_eq!(found.len(), 3, "{found:?}");
    assert_eq!(
        found[0],
        ("205.178,26.25".into(), "0 0 24 24".into()),
        "edge"
    );
    assert_eq!(found[1].1, "0 0 24 24");
    assert!(found[1].0.ends_with(",26.75"), "{found:?}");
    assert!(s.contains("width=\"14\" height=\"14\" viewBox=\"0 0 24 24\""));
    // 30 / 20 = 1.5em.
    assert!(s.contains("width=\"21\" height=\"14\" viewBox=\"0 0 30 20\""));
    // `.node .label-icon path { fill: currentColor }`; an edge label keeps the fill.
    assert_eq!(s.matches("fill=\"currentColor\"/></svg>").count(), 2, "{s}");
    assert!(s.contains("fill=\"#123456\"/></svg>"));
    // `fa-car` is not in the pack: Font Awesome's glyph.
    assert!(s.contains("class=\"fa-icon\""));
    // No tag characters reach the output.
    assert!(!s.chars().any(|c| ('\u{E0000}'..='\u{E007F}').contains(&c)));
    // Without the pack the home icon is Font Awesome's.
    let plain = render::render(src, &MermaidConfig::default()).unwrap();
    let p = output::encode_svg(&plain, &EncodeOptions::default());
    assert!(icons(&p).is_empty());
}

/// `.node.usecase-actor .usecase-actor-glyph path, … circle { stroke }` beats
/// an icon's own stroke attribute (and what it inherits), not its inline
/// style; neo strokes its rects in the node border. Computed styles checked
/// in Chromium against mmdc.
#[test]
fn usecase_icon_actor_css_beats_presentation_attributes() {
    const T2: &str = r##"{ "prefix": "t2", "width": 16, "height": 16, "icons": {
      "strk": { "body": "<path d=\"M2 8h12M8 2v12\" fill=\"none\" stroke=\"#e33\" stroke-width=\"2\"/>" },
      "inl": { "body": "<path d=\"M2 8h12M8 2v12\" fill=\"none\" style=\"stroke:#3a3\" stroke-width=\"2\"/>" },
      "grp": { "body": "<g stroke=\"#e33\" stroke-width=\"2\"><path d=\"M2 2L14 14\"/><rect x=\"2\" y=\"10\" width=\"4\" height=\"4\" fill=\"none\"/></g>" } } }"##;
    let cfg = MermaidConfig {
        icon_packs: IconPacks::new(vec![("t2".into(), Ok(T2.into()))]),
        ..MermaidConfig::default()
    };
    let body = "usecase-beta\nactor A(\"S\")@{ icon: \"t2:strk\" }\nactor B(\"I\")@{ icon: \"t2:inl\" }\nactor C(\"G\")@{ icon: \"t2:grp\" }\nA --> U\nB --> U\nC --> U\n";
    let svg_of = |look: &str| {
        let src = format!("---\nconfig:\n  look: {look}\n---\n{body}");
        let r = render::render(&src, &cfg).unwrap();
        output::encode_svg(&r, &EncodeOptions::default())
    };
    let neo = svg_of("neo");
    assert!(neo.contains(
        "<path d=\"M2 8h12M8 2v12\" fill=\"none\" stroke=\"#A78BFA\" stroke-width=\"2\"/>"
    ));
    assert!(neo.contains(
        "<path d=\"M2 8h12M8 2v12\" fill=\"none\" style=\"stroke:#3a3\" stroke-width=\"2\"/>"
    ));
    assert!(neo.contains("<path d=\"M2 2L14 14\" stroke=\"#A78BFA\""));
    assert!(neo.contains("height=\"4\" fill=\"none\" stroke=\"#28253D\"/>"));
    let classic = svg_of("classic");
    assert!(classic.contains("<path d=\"M2 2L14 14\" stroke=\"#A78BFA\"/><rect x=\"2\" y=\"10\" width=\"4\" height=\"4\" fill=\"none\"/>"));
    // handDrawn has no such rule: the icon keeps its own strokes.
    let hand = svg_of("handDrawn");
    assert!(hand.contains("fill=\"none\" stroke=\"#e33\" stroke-width=\"2\"/>"));
    assert!(hand.contains("<path d=\"M2 2L14 14\"/>"));
}

/// A 40 × 20 PNG of one colour.
fn png(rgb: [u8; 3]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let (w, h) = (40u32, 20u32);
    // Each row: filter type 0, then the pixels.
    let row: Vec<u8> = std::iter::once(0).chain((0..w).flat_map(|_| rgb)).collect();
    let raw: Vec<u8> = (0..h).flat_map(|_| row.iter().copied()).collect();
    // A stored (uncompressed) zlib stream.
    let mut z = vec![0x78, 0x01, 1];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn base64(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            s.push(if i <= c.len() {
                A[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    s
}

#[test]
fn remote_images_are_sized_and_drawn_from_fetched_bytes() {
    let url = "https://example.com/a.png";
    let src = format!("flowchart LR\n  A@{{ img: \"{url}\", label: \"X\" }}\n");
    assert_eq!(
        render::remote_images(&src, &MermaidConfig::default()),
        vec![url.to_string()]
    );
    // Not fetched (no network): mermaid's `img.decode()` rejects it.
    let err = render::render(&src, &MermaidConfig::default()).expect_err("fails");
    assert_eq!(err.to_string(), "The source image cannot be decoded.");
    let env = RenderEnv {
        images: std::sync::Arc::new([(url.to_string(), png([200, 60, 60]))].into()),
        ..Default::default()
    };
    let r = render::render_with_env(&src, &MermaidConfig::default(), &env).unwrap();
    let s = output::encode_svg(&r, &EncodeOptions::default());
    // The SVG keeps the link; the natural 40 × 20 widens to the label width (120).
    assert!(s.contains(&format!(
        "<image href=\"{url}\" x=\"12\" y=\"12\" width=\"120\" height=\"20\""
    )));
    // PDF output embeds the fetched pixels.
    let pdf = output::encode_pdf(&r, &EncodeOptions::default());
    let text = String::from_utf8_lossy(&pdf);
    if cfg!(feature = "png") {
        assert!(
            text.contains("/Subtype /Image /Width 40 /Height 20"),
            "{text}"
        );
    }
}

#[test]
fn pdf_embeds_jpeg_as_dct_and_dedupes_images() {
    // A minimal baseline JPEG header (SOI, SOF0 2 × 1 RGB, EOI): passed through.
    let jpeg = [
        0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x01, 0x00, 0x02, 0x03, 0x01, 0x11, 0x00,
        0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xff, 0xd9,
    ];
    let uri = format!("data:image/jpeg;base64,{}", base64(&jpeg));
    let src = format!(
        "flowchart LR\n  A@{{ img: \"{uri}\", w: 40, h: 20 }}\n  B@{{ img: \"{uri}\", w: 80, h: 40 }}\n  A --> B\n"
    );
    let r = render::render(&src, &MermaidConfig::default()).unwrap();
    let pdf = output::encode_pdf(&r, &EncodeOptions::default());
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("/Filter /DCTDecode").count(), 1, "{text}");
    assert!(text.contains("/Width 2 /Height 1 /ColorSpace /DeviceRGB"));
    assert_eq!(text.matches("/Im0 Do").count(), 2);
}

#[cfg(feature = "png")]
#[test]
fn png_output_draws_images_and_icons() {
    let uri = format!("data:image/png;base64,{}", base64(&png([200, 60, 60])));
    let src = format!(
        "flowchart LR\n  A@{{ img: \"{uri}\", w: 80, h: 40 }}\n  B@{{ icon: \"tst:box\", label: \"\" }}\n  A --> B\n"
    );
    let r = render::render(&src, &config()).unwrap();
    let rast = output::png::Rasterizer::bundled_only();
    let bytes = output::encode_png(&r, &EncodeOptions::default(), &rast).unwrap();
    let pix = resvg_pixels(&bytes);
    let (w, h) = (pix.0 as usize, pix.1 as usize);
    let at = |x: usize, y: usize| &pix.2[(y * w + x) * 4..(y * w + x) * 4 + 3];
    let _ = h;
    // The image fills its box (x 8..88, y 8..48 in the PNG).
    assert_eq!(at(48, 28), [200, 60, 60]);
    // The icon's rect is drawn in the node border colour (#28253D).
    let s = output::encode_svg(&r, &EncodeOptions::default());
    let (tx, ty) = {
        let t = icons(&s)[0].0.clone();
        let (a, b) = t.split_once(',').unwrap();
        (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap())
    };
    let vb: Vec<f64> = view_box(&s)
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    let (x, y) = ((tx - vb[0] + 24.0) as usize, (ty - vb[1] + 24.0) as usize);
    assert_eq!(at(x, y), [0x28, 0x25, 0x3d]);
}

#[cfg(feature = "png")]
fn resvg_pixels(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let p = resvg::tiny_skia::Pixmap::decode_png(png).unwrap();
    (p.width(), p.height(), p.data().to_vec())
}
