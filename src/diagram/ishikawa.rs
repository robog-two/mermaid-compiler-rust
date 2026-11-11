//! `ishikawa` / `ishikawa-beta` fishbone (cause-and-effect) diagrams (mermaid 12.3).
//!
//! The first statement is the effect (drawn as the fish head at the right end of a
//! horizontal spine); its children are cause categories, alternating above and
//! below the spine on 82° bones; deeper causes alternate between horizontal and
//! diagonal sub-bones. Geometry is a port of mermaid's `ishikawaRenderer.ts`.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::Palette;

const SPINE_BASE_LENGTH: f64 = 250.0;
const BONE_STUB: f64 = 30.0;
const BONE_BASE: f64 = 60.0;
const BONE_PER_CHILD: f64 = 5.0;
const ANGLE_DEG: f64 = 82.0;
/// Font size of the head label (`.ishikawa-head-label`).
const HEAD_FONT: f64 = 14.0;

// ───────────────────────────── parser ─────────────────────────────

/// A cause (or the effect, at the root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause {
    pub text: String,
    pub children: Vec<Cause>,
}

impl Cause {
    fn new(text: &str) -> Self {
        Cause {
            text: text.to_string(),
            children: Vec::new(),
        }
    }
    /// Number of descendants (mermaid's `countDescendants`).
    pub fn descendants(&self) -> usize {
        self.children.iter().map(|c| 1 + c.descendants()).sum()
    }
}

/// mermaid's `IshikawaDB.addNode` state.
#[derive(Default)]
struct Builder {
    root: Option<Cause>,
    /// (level, path of child indices from the root)
    stack: Vec<(i64, Vec<usize>)>,
    base_level: Option<i64>,
}

impl Builder {
    fn add(&mut self, raw_level: usize, text: &str) {
        let Some(root) = self.root.as_mut() else {
            self.root = Some(Cause::new(text));
            self.stack = vec![(0, Vec::new())];
            return;
        };
        let raw = raw_level as i64;
        let base = *self.base_level.get_or_insert(raw);
        let level = (raw - base + 1).max(1);
        while self.stack.len() > 1 && self.stack.last().is_some_and(|(l, _)| *l >= level) {
            self.stack.pop();
        }
        let path = self
            .stack
            .last()
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        let mut parent = root;
        for &i in &path {
            parent = &mut parent.children[i];
        }
        parent.children.push(Cause::new(text));
        let mut child_path = path;
        child_path.push(parent.children.len() - 1);
        self.stack.push((level, child_path));
    }
}

fn keyword_len(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("ishikawa-beta") {
        Some(13)
    } else if lower.starts_with("ishikawa") {
        Some(8)
    } else {
        None
    }
}

/// Parse an ishikawa body. Returns the effect (root) with its causes, or `None`
/// for a diagram with only the keyword.
pub fn parse(body: &str) -> Result<Option<Cause>> {
    let mut b = Builder::default();
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line = i + 1;
        let raw = raw.trim_end_matches('\r');
        if raw.trim().is_empty() || raw.trim_start().starts_with("%%") {
            continue;
        }
        let ws = raw.len() - raw.trim_start().len();
        let mut stmt = raw;
        if !header {
            let t = raw.trim_start();
            let Some(k) = keyword_len(t) else {
                return Err(Error::parse(
                    line,
                    format!("Parse error on line {line}:\nExpecting 'ISHIKAWA', got 'TEXT'"),
                ));
            };
            header = true;
            stmt = &t[k..];
            if stmt.trim().is_empty() {
                continue;
            }
        }
        let level = stmt.len() - stmt.trim_start().len();
        let text = stmt.trim();
        // The lexer is case-insensitive and tries the keyword before TEXT.
        if keyword_len(text).is_some() {
            return Err(Error::parse(
                line,
                format!(
                    "Parse error on line {line}:\nExpecting 'SPACELINE', 'SPACELIST', 'TEXT', got 'ISHIKAWA'"
                ),
            ));
        }
        let _ = ws;
        b.add(level, text);
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'ISHIKAWA'"));
    }
    Ok(b.root)
}

// ───────────────────────────── text ─────────────────────────────

/// mermaid's `wrapText`: greedy word wrap at `max` characters.
pub fn wrap_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(char::is_whitespace) {
        match lines.last_mut() {
            Some(last) if last.chars().count() + 1 + word.chars().count() <= max => {
                last.push(' ');
                last.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines.join("\n")
}

/// mermaid's `splitLines`: `<br>` variants and newlines.
fn split_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '\n' {
            out.push(std::mem::take(&mut cur));
            rest = &rest[1..];
            continue;
        }
        if c == '<' {
            let lower = rest.get(..3).map(str::to_ascii_lowercase);
            if lower.as_deref() == Some("<br") {
                let after = rest[3..].trim_start();
                let after = after.strip_prefix('/').unwrap_or(after);
                if let Some(a) = after.strip_prefix('>') {
                    out.push(std::mem::take(&mut cur));
                    rest = a;
                    continue;
                }
            }
        }
        cur.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.push(cur);
    out
}

/// Vertical alignment of a text block (`dominant-baseline`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// `middle`: lines centred on `y`.
    Middle,
    /// `baseline` (labels above the spine).
    Alphabetic,
    /// `hanging` (labels below the spine).
    Hanging,
}

/// A positioned multi-line label (mermaid's `drawMultilineText`).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub lines: Vec<String>,
    /// Anchor point of the first line (already shifted up by half the block).
    pub x: f64,
    pub y: f64,
    pub anchor: Anchor,
    pub baseline: Baseline,
    pub font_size: f64,
    pub line_height: f64,
    pub bold: bool,
}

impl Label {
    fn new(text: &str, x: f64, y: f64, anchor: Anchor, baseline: Baseline, fs: f64) -> Self {
        let lines = split_lines(text);
        let lh = fs * 1.05;
        Label {
            y: y - (lines.len() as f64 - 1.0) * lh / 2.0,
            lines,
            x,
            anchor,
            baseline,
            font_size: fs,
            line_height: lh,
            bold: false,
        }
    }

    pub fn width(&self) -> f64 {
        self.lines
            .iter()
            .map(|l| line_width(l, self.font_size, self.bold))
            .fold(0.0, f64::max)
    }

    /// Approximation of the browser's `getBBox()`.
    pub fn bbox(&self) -> Rect {
        let w = self.width();
        let x = match self.anchor {
            Anchor::Start => self.x,
            Anchor::Middle => self.x - w / 2.0,
            Anchor::End => self.x - w,
        };
        let fs = self.font_size;
        let last = self.lines.last().map_or("", String::as_str);
        let th = super::treeview::text_height(last, fs);
        let top = match self.baseline {
            Baseline::Middle => self.y - 0.61 * fs,
            Baseline::Alphabetic => self.y - 0.8 * fs,
            Baseline::Hanging => self.y,
        };
        let h = (self.lines.len() as f64 - 1.0) * self.line_height + th;
        Rect::new(x, top, w, h)
    }

    /// Baseline of the first line.
    fn first_baseline(&self) -> f64 {
        match self.baseline {
            Baseline::Middle => self.y + 0.35 * self.font_size,
            Baseline::Alphabetic => self.y,
            Baseline::Hanging => self.y + 0.75 * self.font_size,
        }
    }
}

// ───────────────────────────── layout ─────────────────────────────

/// Kind of a bone line (stroke width and arrow size differ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoneKind {
    /// Cause category bone (stroke 2).
    Branch,
    /// Nested cause bone (stroke 1).
    SubBranch,
}

/// One drawable item, in mermaid's DOM order.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Spine {
        from: Point,
        to: Point,
    },
    /// The fish head at `(0, y)`: width `w`, height `h`, plus its label.
    Head {
        y: f64,
        w: f64,
        h: f64,
        label: Label,
    },
    /// A bone with an arrowhead at `from` (pointing at its parent).
    Bone {
        kind: BoneKind,
        from: Point,
        to: Point,
    },
    /// A cause category label with its box.
    Category {
        label: Label,
        rect: Rect,
    },
    /// A nested cause label.
    SubLabel(Label),
}

/// A laid-out diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub marks: Vec<Mark>,
    /// Union of everything's bounding box (mermaid's `svg.getBBox()`).
    pub bbox: Rect,
}

struct Stats {
    total: usize,
    max: usize,
}

fn side_stats<'a>(nodes: impl Iterator<Item = &'a Cause>) -> Stats {
    nodes.fold(Stats { total: 0, max: 0 }, |mut s, n| {
        let d = n.descendants();
        s.total += d;
        s.max = s.max.max(d);
        s
    })
}

struct Entry {
    text: String,
    depth: usize,
    parent: Option<usize>,
    child_count: usize,
}

/// mermaid's `flattenTree`: even depths pre-order, odd depths post-order.
fn flatten(children: &[Cause], dir: f64) -> (Vec<Entry>, Vec<usize>) {
    fn walk(
        nodes: &[Cause],
        pid: Option<usize>,
        depth: usize,
        dir: f64,
        entries: &mut Vec<Entry>,
        order: &mut Vec<usize>,
    ) {
        let ordered: Vec<&Cause> = if dir < 0.0 {
            nodes.iter().rev().collect()
        } else {
            nodes.iter().collect()
        };
        for child in ordered {
            let idx = entries.len();
            entries.push(Entry {
                text: wrap_text(&child.text, 15),
                depth,
                parent: pid,
                child_count: child.children.len(),
            });
            if depth % 2 == 0 {
                order.push(idx);
                walk(&child.children, Some(idx), depth + 1, dir, entries, order);
            } else {
                walk(&child.children, Some(idx), depth + 1, dir, entries, order);
                order.push(idx);
            }
        }
    }
    let mut entries = Vec::new();
    let mut order = Vec::new();
    walk(children, None, 2, dir, &mut entries, &mut order);
    (entries, order)
}

#[derive(Clone, Copy)]
struct Bone {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    child_count: usize,
    drawn: usize,
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// mermaid's `drawBranch`; returns the marks and the labels' left-most x.
fn branch(node: &Cause, start: Point, dir: f64, length: f64, fs: f64, out: &mut Vec<Mark>) -> f64 {
    let (sin_a, cos_a) = ANGLE_DEG.to_radians().sin_cos();
    let mut left = f64::INFINITY;
    let line_len = length * if node.children.is_empty() { 0.2 } else { 1.0 };
    let dx = -cos_a * line_len;
    let dy = sin_a * line_len * dir;
    let end = Point::new(start.x + dx, start.y + dy);
    out.push(Mark::Bone {
        kind: BoneKind::Branch,
        from: start,
        to: end,
    });
    let label = Label::new(
        &node.text,
        end.x,
        end.y + 11.0 * dir,
        Anchor::Middle,
        Baseline::Middle,
        fs,
    );
    let tb = label.bbox();
    left = left.min(tb.x);
    out.push(Mark::Category {
        rect: Rect::new(tb.x - 20.0, tb.y - 2.0, tb.w + 40.0, tb.h + 4.0),
        label,
    });
    if node.children.is_empty() {
        return left;
    }
    let (entries, order) = flatten(&node.children, dir);
    let n = entries.len();
    let mut ys = vec![0.0; n];
    for (slot, &idx) in order.iter().enumerate() {
        ys[idx] = start.y + dy * ((slot as f64 + 1.0) / (n as f64 + 1.0));
    }
    // bones[0] is the category bone; bones[i + 1] belongs to entry i.
    let mut bones: Vec<Option<Bone>> = vec![None; n + 1];
    bones[0] = Some(Bone {
        x0: start.x,
        y0: start.y,
        x1: end.x,
        y1: end.y,
        child_count: node.children.len(),
        drawn: 0,
    });
    let diag_x = -cos_a;
    let diag_y = sin_a * dir;
    for (i, e) in entries.iter().enumerate() {
        let y = ys[i];
        let pslot = e.parent.map_or(0, |p| p + 1);
        let Some(par) = bones[pslot].as_mut() else {
            continue;
        };
        let (bx0, by0, bx1, label) = if e.depth % 2 == 0 {
            let dyp = par.y1 - par.y0;
            let t = if dyp != 0.0 { (y - par.y0) / dyp } else { 0.5 };
            let bx0 = lerp(par.x0, par.x1, t);
            let len = if e.child_count > 0 {
                BONE_BASE + e.child_count as f64 * BONE_PER_CHILD
            } else {
                BONE_STUB
            };
            let bx1 = bx0 - len;
            let l = Label::new(&e.text, bx1, y, Anchor::End, Baseline::Middle, fs);
            (bx0, y, bx1, l)
        } else {
            let k = par.drawn;
            par.drawn += 1;
            let bx0 = lerp(
                par.x0,
                par.x1,
                (par.child_count as f64 - k as f64) / (par.child_count as f64 + 1.0),
            );
            let by0 = par.y0;
            let bx1 = bx0 + diag_x * ((y - by0) / diag_y);
            let bl = if dir < 0.0 {
                Baseline::Alphabetic
            } else {
                Baseline::Hanging
            };
            let l = Label::new(&e.text, bx1, y, Anchor::End, bl, fs);
            (bx0, by0, bx1, l)
        };
        out.push(Mark::Bone {
            kind: BoneKind::SubBranch,
            from: Point::new(bx0, by0),
            to: Point::new(bx1, y),
        });
        left = left.min(label.bbox().x);
        out.push(Mark::SubLabel(label));
        if e.child_count > 0 {
            bones[i + 1] = Some(Bone {
                x0: bx0,
                y0: by0,
                x1: bx1,
                y1: y,
                child_count: e.child_count,
                drawn: 0,
            });
        }
    }
    left
}

/// mermaid's `draw` geometry. `fs` is the configured `fontSize`.
pub fn layout(root: &Cause, fs: f64) -> Layout {
    let mut marks = Vec::new();
    // Head.
    let max_chars = ((110.0 / (fs * 0.6)).floor() as usize).max(6);
    let mut head_label = Label::new(
        &wrap_text(&root.text, max_chars),
        0.0,
        0.0,
        Anchor::Middle,
        Baseline::Middle,
        HEAD_FONT,
    );
    head_label.line_height = fs * 1.05;
    head_label.bold = true;
    head_label.y = -(head_label.lines.len() as f64 - 1.0) * head_label.line_height / 2.0;
    let tb = head_label.bbox();
    let w = (tb.w + 6.0).max(60.0);
    let h = (tb.h * 2.0 + 40.0).max(40.0);
    head_label.x = w / 2.0 + 3.0;
    // Centre the block vertically on the spine (mermaid's -tb.y - tb.height/2).
    head_label.y -= tb.y + tb.h / 2.0;

    let causes = &root.children;
    let mut spine_x = 0.0;
    let mut spine_y = SPINE_BASE_LENGTH;
    let mut pair_marks = Vec::new();
    if !causes.is_empty() {
        spine_x = -20.0;
        let upper = side_stats(causes.iter().step_by(2));
        let lower = side_stats(causes.iter().skip(1).step_by(2));
        let total = upper.total + lower.total;
        let (mut up_len, mut low_len) = (SPINE_BASE_LENGTH, SPINE_BASE_LENGTH);
        if total > 0 {
            let pool = SPINE_BASE_LENGTH * 2.0;
            let min_len = SPINE_BASE_LENGTH * 0.3;
            up_len = min_len.max(pool * upper.total as f64 / total as f64);
            low_len = min_len.max(pool * lower.total as f64 / total as f64);
        }
        let min_spacing = fs * 2.0;
        up_len = up_len.max(upper.max as f64 * min_spacing);
        low_len = low_len.max(lower.max as f64 * min_spacing);
        spine_y = up_len.max(SPINE_BASE_LENGTH);
        for pair in causes.chunks(2) {
            let mut left = f64::INFINITY;
            for (cause, dir, len) in pair
                .iter()
                .zip([(-1.0, up_len), (1.0, low_len)])
                .map(|(c, (d, l))| (c, d, l))
            {
                let l = branch(
                    cause,
                    Point::new(spine_x, spine_y),
                    dir,
                    len,
                    fs,
                    &mut pair_marks,
                );
                left = left.min(l);
            }
            spine_x = left;
        }
    }
    head_label.y += spine_y;
    marks.push(Mark::Spine {
        from: Point::new(spine_x, spine_y),
        to: Point::new(0.0, spine_y),
    });
    marks.push(Mark::Head {
        y: spine_y,
        w,
        h,
        label: head_label,
    });
    marks.extend(pair_marks);

    let mut b = Bounds::default();
    for m in &marks {
        match m {
            Mark::Spine { from, to } | Mark::Bone { from, to, .. } => {
                b.add_point(*from);
                b.add_point(*to);
            }
            Mark::Head { y, w, h, label } => {
                b.add_rect(Rect::new(0.0, y - h / 2.0, 1.2 * w, *h));
                b.add_rect(label.bbox());
            }
            Mark::Category { label, rect } => {
                b.add_rect(*rect);
                b.add_rect(label.bbox());
            }
            Mark::SubLabel(l) => b.add_rect(l.bbox()),
        }
    }
    Layout {
        marks,
        bbox: b.rect(),
    }
}

// ───────────────────────────── drawing ─────────────────────────────

fn text_el(l: &Label, fill: &Color, class: &str) -> Element {
    Element::classed_text(
        Text {
            x: l.x,
            y: l.first_baseline(),
            lines: l.lines.clone(),
            line_height: l.line_height,
            font_size: l.font_size,
            bold: l.bold,
            anchor: l.anchor,
            fill: fill.clone(),
        },
        class,
    )
}

/// mermaid's `ishikawa-arrow` marker (`M10 0 L0 5 L10 10Z`, 6×6 in stroke-width
/// units, `refX=0`) placed at the start of a bone.
fn arrow(from: Point, to: Point, stroke: f64) -> Vec<PathCmd> {
    let s = 0.6 * stroke;
    let u = to.minus(from).norm();
    let n = Point::new(-u.y, u.x);
    let base = from.plus(u.scale(10.0 * s));
    vec![
        PathCmd::MoveTo(from),
        PathCmd::LineTo(base.plus(n.scale(5.0 * s))),
        PathCmd::LineTo(base.minus(n.scale(5.0 * s))),
        PathCmd::Close,
    ]
}

/// Draw a laid-out diagram.
pub fn draw(lay: &Layout, padding: f64, pal: &Palette) -> Scene {
    let line = &pal.line_color;
    let bkg = &pal.main_bkg;
    let text = &pal.text_color;
    let mut els = Vec::new();
    for m in &lay.marks {
        match m {
            Mark::Spine { from, to } => els.push(Element::classed_shape(
                Geometry::Path(vec![PathCmd::MoveTo(*from), PathCmd::LineTo(*to)]),
                Paint::stroke(line, 2.0),
                "ishikawa-spine",
            )),
            Mark::Head { y, w, h, label } => {
                // M 0 -h/2 L 0 h/2 Q 2.4w 0 0 -h/2 Z, with the quadratic as a cubic.
                let p0 = Point::new(0.0, y + h / 2.0);
                let q = Point::new(2.4 * w, *y);
                let p1 = Point::new(0.0, y - h / 2.0);
                let c1 = p0.plus(q.minus(p0).scale(2.0 / 3.0));
                let c2 = p1.plus(q.minus(p1).scale(2.0 / 3.0));
                els.push(Element::group(
                    "ishikawa-head-group",
                    vec![
                        Element::classed_shape(
                            Geometry::Path(vec![
                                PathCmd::MoveTo(p1),
                                PathCmd::LineTo(p0),
                                PathCmd::CubicTo(c1, c2, p1),
                                PathCmd::Close,
                            ]),
                            Paint::both(bkg, line, 2.0),
                            "ishikawa-head",
                        ),
                        text_el(label, text, "ishikawa-head-label"),
                    ],
                ));
            }
            Mark::Bone { kind, from, to } => {
                let (w, class) = match kind {
                    BoneKind::Branch => (2.0, "ishikawa-branch"),
                    BoneKind::SubBranch => (1.0, "ishikawa-sub-branch"),
                };
                els.push(Element::classed_shape(
                    Geometry::Path(vec![PathCmd::MoveTo(*from), PathCmd::LineTo(*to)]),
                    Paint::stroke(line, w),
                    class,
                ));
                els.push(Element::classed_shape(
                    Geometry::Path(arrow(*from, *to, w)),
                    Paint::fill(line),
                    "ishikawa-arrow",
                ));
            }
            Mark::Category { label, rect } => els.push(Element::group(
                "ishikawa-label-group",
                vec![
                    Element::classed_shape(
                        Geometry::Rect {
                            rect: *rect,
                            rx: 0.0,
                        },
                        Paint::both(bkg, line, 2.0),
                        "ishikawa-label-box",
                    ),
                    text_el(label, text, "ishikawa-label cause"),
                ],
            )),
            Mark::SubLabel(l) => els.push(text_el(l, text, "ishikawa-label")),
        }
    }
    let view_box = if lay.marks.is_empty() {
        Rect::new(0.0, 0.0, 2.0 * padding, 2.0 * padding)
    } else {
        lay.bbox.pad(padding)
    };
    Scene {
        kind: DiagramKind::Ishikawa,
        view_box,
        elements: vec![Element::group("ishikawa", els)],
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// Parse, lay out and draw a `ishikawa` diagram.
pub fn render(
    body: &str,
    _title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let root = parse(body)?;
    let padding = cfg.section_f64("ishikawa", "diagramPadding", 20.0).max(0.0);
    let lay = match &root {
        Some(r) => layout(r, pal.font_size),
        None => Layout {
            marks: Vec::new(),
            bbox: Rect::new(0.0, 0.0, 0.0, 0.0),
        },
    };
    Ok(draw(&lay, padding, pal))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLURRY: &str = "ishikawa-beta\n    Blurry Photo\n    Process\n        Out of focus\n        Shutter speed too slow\n        Protective film not removed\n        Beautification filter applied\n    User\n        Shaky hands\n    Equipment\n        LENS\n            Inappropriate lens\n            Damaged lens\n            Dirty lens\n        SENSOR\n            Damaged sensor\n            Dirty sensor\n    Environment\n        Subject moved too quickly\n        Too dark\n";

    fn texts(c: &Cause) -> Vec<&str> {
        c.children.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn parses_hierarchy() {
        let r = parse(BLURRY).unwrap().unwrap();
        assert_eq!(r.text, "Blurry Photo");
        assert_eq!(
            texts(&r),
            vec!["Process", "User", "Equipment", "Environment"]
        );
        assert_eq!(texts(&r.children[2]), vec!["LENS", "SENSOR"]);
        assert_eq!(r.children[2].children[0].children.len(), 3);
        assert_eq!(r.children[2].descendants(), 7);
    }

    #[test]
    fn keyword_variants_and_empty() {
        assert!(parse("ISHIKAWA\n  Effect").unwrap().is_some());
        assert!(parse("Ishikawa-Beta\n").unwrap().is_none());
        assert_eq!(parse("ishikawa Effect").unwrap().unwrap().text, "Effect");
        assert!(parse("fishbone\n a").is_err());
    }

    #[test]
    fn base_level_comes_from_first_cause() {
        // Upstream spec: the effect may be indented more than the causes.
        let r = parse("ishikawa\n        Effect\n    A\n        a1\n    B\n")
            .unwrap()
            .unwrap();
        assert_eq!(texts(&r), vec!["A", "B"]);
        assert_eq!(texts(&r.children[0]), vec!["a1"]);
        // Dedenting below the base level clamps to level 1.
        let r = parse("ishikawa\n  E\n      A\n          a1\n    B\n  C\n")
            .unwrap()
            .unwrap();
        assert_eq!(texts(&r), vec!["A", "B", "C"]);
        assert_eq!(texts(&r.children[0]), vec!["a1"]);
    }

    #[test]
    fn wraps_like_mermaid() {
        assert_eq!(
            wrap_text("Shutter speed too slow", 15),
            "Shutter speed\ntoo slow"
        );
        assert_eq!(wrap_text("Out of focus", 15), "Out of focus");
        assert_eq!(
            wrap_text(
                "Website is down because the database cluster failed over",
                11
            ),
            "Website is\ndown\nbecause the\ndatabase\ncluster\nfailed over"
        );
        assert_eq!(split_lines("a<br/>b<BR>c\nd"), vec!["a", "b", "c", "d"]);
    }

    fn bones(l: &Layout, kind: BoneKind) -> Vec<(Point, Point)> {
        l.marks
            .iter()
            .filter_map(|m| match m {
                Mark::Bone { kind: k, from, to } if *k == kind => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn layout_matches_reference_geometry() {
        let r = parse(BLURRY).unwrap().unwrap();
        let l = layout(&r, 16.0);
        // spineY = max(upper share of 500, 250) = 500 * 11/14
        let Mark::Spine { from, to } = &l.marks[0] else {
            panic!()
        };
        assert!((from.y - 392.857_142_857).abs() < 1e-6);
        assert_eq!(to.x, 0.0);
        let br = bones(&l, BoneKind::Branch);
        assert_eq!(br.len(), 4);
        // Process: up, 82°, from (-20, spineY) to (-74.675, 3.823) as in mermaid's SVG.
        assert!((br[0].0.x + 20.0).abs() < 1e-9);
        assert!((br[0].1.x + 74.675_146_8).abs() < 1e-6);
        assert!((br[0].1.y - 3.823_258_7).abs() < 1e-6);
        // User (leaf-only, lower): 0.2 × lowerLen(107.14)
        assert!((br[1].1.y - 498.957_293).abs() < 1e-5);
        // Upper bones go up, lower bones go down; later pairs start further left.
        assert!(br[0].1.y < from.y && br[1].1.y > from.y);
        assert!(br[2].0.x < br[0].0.x && br[2].0.x == br[3].0.x);
        // Process' first sub-bone ("Beautification filter applied", reversed order).
        let sub = bones(&l, BoneKind::SubBranch);
        assert!((sub[0].0.x + 30.935_029_36).abs() < 1e-6);
        assert!((sub[0].0.y - 315.050_366_03).abs() < 1e-6);
        assert!((sub[0].1.x - (sub[0].0.x - BONE_STUB)).abs() < 1e-9);
        // spine reaches the left-most label
        let min_label = l
            .marks
            .iter()
            .filter_map(|m| match m {
                Mark::SubLabel(t) | Mark::Category { label: t, .. } => Some(t.bbox().x),
                _ => None,
            })
            .fold(f64::INFINITY, f64::min);
        assert!(from.x <= min_label + 1e-9);
        assert!(l.bbox.x <= from.x && l.bbox.right() >= 0.0);
    }

    #[test]
    fn nested_odd_depth_bones_are_diagonal_at_82_degrees() {
        let r = parse(BLURRY).unwrap().unwrap();
        let l = layout(&r, 16.0);
        let sub = bones(&l, BoneKind::SubBranch);
        // "SENSOR" horizontal, then "Dirty sensor" diagonal starting on it.
        let diag: Vec<_> = sub
            .iter()
            .filter(|(a, b)| (a.y - b.y).abs() > 1e-9)
            .collect();
        assert_eq!(diag.len(), 5);
        for (a, b) in diag {
            let ang = ((a.y - b.y).abs() / (a.x - b.x).abs()).atan().to_degrees();
            assert!((ang - 82.0).abs() < 1e-6);
        }
    }

    #[test]
    fn head_only() {
        let cfg = MermaidConfig::default();
        let s = render("ishikawa\n  Effect\n", None, &cfg, &cfg.palette()).unwrap();
        assert_eq!(s.kind, DiagramKind::Ishikawa);
        // head 60 wide → curve reaches x = 72; h = 2 × text height + 40; padded by 20
        assert!((s.view_box.right() - 92.0).abs() < 1e-9);
        let th = crate::diagram::treeview::text_height("Effect", HEAD_FONT);
        assert!((s.view_box.h - (th * 2.0 + 40.0 + 40.0)).abs() < 1e-9);
    }
}
