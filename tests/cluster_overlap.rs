//! Fuzzer: random flowcharts with sibling (and nested) subgraphs, edges between
//! members and edges to whole subgraphs (contradictory ones included), under ELK
//! and dagre, in every direction. Sibling cluster frames (padding and title
//! included) must never overlap, and no node may fall inside the frame of a
//! cluster it does not belong to.

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;
use std::collections::HashMap;

/// A small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

#[derive(Clone, Copy)]
struct Frame {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Frame {
    fn overlaps(&self, o: &Frame) -> bool {
        let ox = self.x1.min(o.x1) - self.x0.max(o.x0);
        let oy = self.y1.min(o.y1) - self.y0.max(o.y0);
        ox > 0.5 && oy > 0.5
    }
}

fn attr(tag: &str, name: &str) -> f64 {
    let key = format!(" {name}=\"");
    let at = tag.find(&key).expect("attribute") + key.len();
    let end = at + tag[at..].find('"').expect("closing quote");
    tag[at..end].parse().expect("number")
}

/// The first `<rect ...>` after `from` that has `marker` in it.
fn rect_after(svg: &str, from: usize, marker: &str) -> Frame {
    let mut at = from;
    loop {
        let start = at + svg[at..].find("<rect ").expect("rect");
        let end = start + svg[start..].find('>').expect("tag end");
        let tag = &svg[start..end];
        if tag.contains(marker) && !tag.contains("drop-shadow") {
            let (x, y) = (attr(tag, "x"), attr(tag, "y"));
            return Frame {
                x0: x,
                y0: y,
                x1: x + attr(tag, "width"),
                y1: y + attr(tag, "height"),
            };
        }
        at = end;
    }
}

struct Case {
    src: String,
    /// Cluster -> parent cluster.
    parent: HashMap<String, Option<String>>,
    /// Node -> innermost cluster.
    home: HashMap<String, Option<String>>,
}

fn generate(rng: &mut Rng, dagre: bool) -> Case {
    let dir = *rng.pick(&["TD", "LR", "BT", "RL"]);
    let mut lines = Vec::new();
    if dagre {
        lines.push("---\nconfig:\n  layout: dagre\n---".to_string());
    }
    lines.push(format!("flowchart {dir}"));
    let mut parent = HashMap::new();
    let mut home: HashMap<String, Option<String>> = HashMap::new();
    for loose in ["x", "y", "z"] {
        lines.push(format!("  {loose}"));
        home.insert(loose.to_string(), None);
    }
    let nc = 2 + rng.below(3);
    let mut members: Vec<(String, Vec<String>)> = Vec::new();
    for c in 0..nc {
        let name = format!("C{c}");
        // Sometimes nest inside an earlier cluster, which must still be open:
        // emit clusters as separate blocks, nesting only one level.
        let nested = c > 0 && rng.below(4) == 0;
        let ms: Vec<String> = (0..1 + rng.below(3)).map(|i| format!("c{c}n{i}")).collect();
        let mut block = vec![format!("  subgraph {name}")];
        for w in ms.windows(2) {
            if rng.below(3) > 0 {
                block.push(format!("    {} --> {}", w[0], w[1]));
            }
        }
        for m in &ms {
            block.push(format!("    {m}"));
        }
        let mut owner = None;
        if nested {
            let k = rng.below(c);
            let host = format!("C{k}");
            if parent.get(&host) == Some(&None) {
                owner = Some(host);
            }
        }
        block.push("  end".to_string());
        match &owner {
            Some(host) => {
                // Insert just before the host's `end`.
                let end = lines
                    .iter()
                    .position(|l| l == &format!("  subgraph {host}"))
                    .and_then(|s| lines[s..].iter().position(|l| l == "  end").map(|e| s + e))
                    .expect("host block");
                let inner: Vec<String> = block.iter().map(|l| format!("  {l}")).collect();
                lines.splice(end..end, inner);
            }
            None => lines.extend(block),
        }
        parent.insert(name.clone(), owner);
        for m in &ms {
            home.insert(m.clone(), Some(name.clone()));
        }
        members.push((name, ms));
    }
    let nodes: Vec<String> = home.keys().cloned().collect::<Vec<_>>();
    let mut nodes = nodes;
    nodes.sort();
    let mut ends = nodes.clone();
    ends.extend(members.iter().map(|(c, _)| c.clone()));
    let within = |n: &str, c: &str| {
        let mut at = home
            .get(n)
            .cloned()
            .flatten()
            .or_else(|| parent.get(n).cloned().flatten());
        if n == c {
            return true;
        }
        while let Some(a) = at {
            if a == c {
                return true;
            }
            at = parent.get(&a).cloned().flatten();
        }
        false
    };
    for _ in 0..2 + rng.below(6) {
        let a = rng.pick(&ends).clone();
        let b = rng.pick(&ends).clone();
        if a != b && !within(&a, &b) && !within(&b, &a) {
            lines.push(format!("  {a} --> {b}"));
        }
    }
    Case {
        src: lines.join("\n") + "\n",
        parent,
        home,
    }
}

/// Overlaps found in one rendered case.
fn check(case: &Case) -> Vec<String> {
    let cfg = MermaidConfig::default();
    let r = render::render(&case.src, &cfg).expect("renders");
    let svg = output::encode_svg(&r, &EncodeOptions::default());
    let mut frames = HashMap::new();
    for c in case.parent.keys() {
        let key = format!("<g id=\"cluster-{c}\" class=\"cluster\">");
        let Some(at) = svg.find(&key) else { continue };
        frames.insert(c.clone(), rect_after(&svg, at, " x=\""));
    }
    let mut nodes = HashMap::new();
    for n in case.home.keys() {
        let key = format!("<g id=\"flowchart-{n}-");
        let Some(at) = svg.find(&key) else { continue };
        nodes.insert(n.clone(), rect_after(&svg, at, "label-container"));
    }
    let inside = |n: &str, c: &str| {
        let mut at = case.home[n].clone();
        while let Some(a) = at {
            if a == c {
                return true;
            }
            at = case.parent[&a].clone();
        }
        false
    };
    let mut bad = Vec::new();
    let mut names: Vec<&String> = frames.keys().collect();
    names.sort();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            if case.parent[*a] == case.parent[*b] && frames[*a].overlaps(&frames[*b]) {
                bad.push(format!("clusters {a} and {b} overlap"));
            }
        }
    }
    for (n, f) in &nodes {
        for c in &names {
            if !inside(n, c) && frames[*c].overlaps(f) {
                bad.push(format!("node {n} inside cluster {c}"));
            }
        }
    }
    bad
}

fn run(dagre: bool, seed: u64, count: usize) {
    let mut rng = Rng(seed);
    let mut failures = Vec::new();
    for k in 0..count {
        let case = generate(&mut rng, dagre);
        let bad = check(&case);
        if !bad.is_empty() {
            failures.push(format!("case {k}: {}\n{}", bad.join(", "), case.src));
        }
    }
    eprintln!(
        "{} overlapping of {count} ({})",
        failures.len(),
        if dagre { "dagre" } else { "elk" }
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn elk_sibling_clusters_never_overlap() {
    run(false, 1, 500);
}

#[test]
fn dagre_sibling_clusters_never_overlap() {
    run(true, 2, 500);
}
