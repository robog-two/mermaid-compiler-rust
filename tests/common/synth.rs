//! Deterministic synthetic diagram generators shared by perf tests and benches.
#![allow(dead_code)]

use std::fmt::Write as _;

/// Tiny xorshift PRNG so inputs are reproducible without a `rand` dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub fn flowchart_chain(n: usize) -> String {
    let mut s = String::from("flowchart TD\n");
    for i in 0..n {
        let _ = writeln!(s, "  N{i}[Node {i}] --> N{}[Node {}]", i + 1, i + 1);
    }
    s
}

pub fn flowchart_random(nodes: usize, edges: usize, seed: u64) -> String {
    let mut r = Rng::new(seed);
    let shapes = [
        ("[", "]"),
        ("(", ")"),
        ("{", "}"),
        ("([", "])"),
        ("[(", ")]"),
        ("((", "))"),
        ("{{", "}}"),
    ];
    let mut s = String::from("flowchart LR\n");
    for i in 0..nodes {
        let (o, c) = shapes[i % shapes.len()];
        let _ = writeln!(s, "  n{i}{o}Label {i}{c}");
    }
    for _ in 0..edges {
        let a = r.below(nodes);
        let b = r.below(nodes);
        let link = ["-->", "-.->", "==>", "---"][r.below(4)];
        let _ = writeln!(s, "  n{a} {link} n{b}");
    }
    s
}

pub fn flowchart_subgraphs(groups: usize, per: usize) -> String {
    let mut s = String::from("flowchart TB\n");
    for g in 0..groups {
        let _ = writeln!(s, "  subgraph g{g}[Group {g}]");
        for i in 0..per {
            if i + 1 < per {
                let _ = writeln!(s, "    g{g}n{i} --> g{g}n{}", i + 1);
            }
        }
        let _ = writeln!(s, "  end");
        if g > 0 {
            let _ = writeln!(s, "  g{}n{} --> g{g}n0", g - 1, per - 1);
        }
    }
    s
}

pub fn flowchart_labelled(n: usize) -> String {
    let mut s = String::from("graph TD\n");
    for i in 0..n {
        let _ = writeln!(s, "  a{i} -->|step {i}| a{}", (i * 3 + 1) % n);
    }
    s
}

pub fn sequence(actors: usize, messages: usize) -> String {
    let mut r = Rng::new(42);
    let mut s = String::from("sequenceDiagram\n  autonumber\n");
    for a in 0..actors {
        let _ = writeln!(s, "  participant P{a} as Participant {a}");
    }
    for m in 0..messages {
        if m % 50 == 0 {
            let _ = writeln!(s, "  loop Batch {m}");
        }
        let a = r.below(actors);
        let b = r.below(actors);
        let arrow = ["->>", "-->>", "-x", "-)"][r.below(4)];
        let _ = writeln!(s, "  P{a}{arrow}P{b}: message number {m}");
        if m % 50 == 49 {
            let _ = writeln!(s, "  end");
        }
    }
    if messages % 50 != 0 {
        s.push_str("  end\n");
    }
    s
}

pub fn state(n: usize) -> String {
    let mut s = String::from("stateDiagram-v2\n  [*] --> S0\n");
    for i in 0..n {
        let _ = writeln!(s, "  S{i} --> S{} : go {i}", (i + 1) % n);
        if i % 7 == 0 {
            let _ = writeln!(s, "  S{i} --> S{} : skip", (i + 5) % n);
        }
    }
    s
}

pub fn class(n: usize) -> String {
    let mut s = String::from("classDiagram\n");
    for i in 0..n {
        let _ = writeln!(
            s,
            "  class C{i} {{\n    +int field{i}\n    +method{i}() void\n  }}"
        );
        if i > 0 {
            let rel = ["<|--", "*--", "o--", "-->", "..>"][i % 5];
            let _ = writeln!(s, "  C{} {rel} C{i}", i / 2);
        }
    }
    s
}

pub fn pie(n: usize) -> String {
    let mut s = String::from("pie title Big pie\n");
    for i in 0..n {
        let _ = writeln!(s, "  \"Slice {i}\" : {}", i + 1);
    }
    s
}

/// Random graph whose edges only reach a few nodes ahead, so the drawing size
/// (nodes + dummy vertices) grows linearly with `n` — isolates algorithmic cost.
pub fn flowchart_local(n: usize, seed: u64) -> String {
    let mut r = Rng::new(seed);
    let mut s = String::from("flowchart TD\n");
    for i in 0..n {
        let _ = writeln!(s, "  n{i}[Node {i}]");
        for _ in 0..2 {
            let j = i + 1 + r.below(4);
            if j < n {
                let _ = writeln!(s, "  n{i} --> n{j}");
            }
        }
    }
    s
}
