//! `ishikawa` / `ishikawa-beta` fishbone (cause-and-effect) diagrams (mermaid 12.3).
//!
//! The first statement is the effect (drawn as the fish head at the right end of a
//! horizontal spine); its children are cause categories, alternating above and
//! below the spine on 82° bones; deeper causes alternate between horizontal and
//! diagonal sub-bones. Geometry is a port of mermaid's `ishikawaRenderer.ts`.

use crate::config::MermaidConfig;
use crate::diagram::wardley::ID_MARK;
use crate::error::{Error, Result};
use crate::geom::{Bounds, PathCmd, Point, Rect};
use crate::output::svg::{esc, esc_text, js_num as n};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{Baseline as TextBaseline, baseline_shift, line_width};
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

    /// Approximation of the browser's `getBBox()`: each line's advance, anchored,
    /// united with its glyphs' ink rounded out to whole pixels
    /// ([`crate::text::svg_bbox_extent_styled`]).
    pub fn bbox(&self) -> Rect {
        self.bbox_impl(false)
    }

    /// [`Label::bbox`]; with `quantised`, the line's advance is rounded up to
    /// 1/64 px as the browser's inline size is, which moves a centred line.
    fn bbox_impl(&self, quantised: bool) -> Rect {
        let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
        for l in &self.lines {
            let mut adv = line_width(l, self.font_size, self.bold);
            if quantised {
                adv = (adv * 64.0 - 1e-6).ceil() / 64.0;
            }
            let start = match self.anchor {
                Anchor::Start => self.x,
                Anchor::Middle => self.x - adv / 2.0,
                Anchor::End => self.x - adv,
            };
            let (lo, hi) = crate::text::svg_bbox_extent_styled(l, self.font_size, self.bold, false);
            x0 = x0.min(start + lo);
            x1 = x1.max(start + hi);
        }
        let (x, w) = if x0.is_finite() {
            (x0, x1 - x0)
        } else {
            (self.x, 0.0)
        };
        let fs = self.font_size;
        let last = self.lines.last().map_or("", String::as_str);
        let th = super::treeview::text_height(last, fs);
        // The glyph box starts the font's ascent (rounded to whole pixels)
        // above the first baseline.
        let top = self.first_baseline() - crate::text::ascent_px(fs);
        let h = (self.lines.len() as f64 - 1.0) * self.line_height + th;
        Rect::new(x, top, w, h)
    }

    /// [`Label::bbox`] as `getBBox()` reports it: single-precision floats,
    /// the far edges summed in single precision too.
    fn bbox_f32(&self) -> Rect {
        let r = self.bbox_impl(true);
        let (x, y) = (r.x as f32, r.y as f32);
        let (w, h) = (((x + r.w as f32) - x), ((y + r.h as f32) - y));
        Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h))
    }

    /// Baseline of the first line.
    fn first_baseline(&self) -> f64 {
        match self.baseline {
            Baseline::Middle => self.y + baseline_shift(TextBaseline::Middle, self.font_size),
            Baseline::Alphabetic => self.y,
            Baseline::Hanging => self.y + baseline_shift(TextBaseline::Hanging, self.font_size),
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
        /// The label's own `y` before its `translate(tx, ty)`.
        attr_y: f64,
        tx: f64,
        ty: f64,
        /// The label's own box (`getBBox()`, before its translate).
        tb: Rect,
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

/// The bones' angle in radians. mermaid writes `82 * Math.PI / 180`; the
/// scene keeps `to_radians`, whose last bit differs.
fn angle(exact: bool) -> f64 {
    if exact {
        ANGLE_DEG * std::f64::consts::PI / 180.0
    } else {
        ANGLE_DEG.to_radians()
    }
}

/// mermaid's `drawBranch`; returns the marks and the labels' left-most x.
fn branch(
    node: &Cause,
    start: Point,
    dir: f64,
    length: f64,
    fs: f64,
    exact: bool,
    out: &mut Vec<Mark>,
) -> f64 {
    let (sin_a, cos_a) = angle(exact).sin_cos();
    let bbox = |l: &Label| if exact { l.bbox_f32() } else { l.bbox() };
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
    let tb = bbox(&label);
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
        left = left.min(bbox(&label).x);
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
    layout_with(root, fs, false)
}

/// [`layout`]; with `exact`, mermaid's own operation order and the
/// browser's single-precision `getBBox()` numbers (what the SVG markup
/// writes), not the scene's.
fn layout_with(root: &Cause, fs: f64, exact: bool) -> Layout {
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
    let attr_y = head_label.y;
    let mut tb = head_label.bbox_impl(exact);
    // drawHead measures the label before the svg is sized, where Chromium's text
    // box is the rounded ascent plus descent (16px at 14px, as `svg_line_box`).
    let last = head_label.lines.last().map_or("", String::as_str);
    tb.h = (head_label.lines.len() as f64 - 1.0) * head_label.line_height
        + crate::text::svg_line_box(HEAD_FONT, last);
    if exact {
        tb = Rect::new(
            f64::from(tb.x as f32),
            f64::from(tb.y as f32),
            f64::from(tb.w as f32),
            f64::from(tb.h as f32),
        );
    }
    let w = (tb.w + 6.0).max(60.0);
    let h = (tb.h * 2.0 + 40.0).max(40.0);
    // translate((w - tb.width) / 2 - tb.x + 3): the label was laid out at x 0.
    let tx = (w - tb.w) / 2.0 - tb.x + 3.0;
    // Centre the block vertically on the spine (mermaid's -tb.y - tb.height/2).
    let ty = -tb.y - tb.h / 2.0;
    head_label.x = tx;
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
                    exact,
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
        attr_y,
        tx,
        ty,
        tb,
    });
    marks.extend(pair_marks);

    let mut b = Bounds::default();
    for m in &marks {
        match m {
            Mark::Spine { from, to } | Mark::Bone { from, to, .. } => {
                b.add_point(*from);
                b.add_point(*to);
            }
            Mark::Head { y, w, h, label, .. } => {
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
            // The only bold label is the head's: `.ishikawa-head-label` is 600.
            weight: l.bold.then_some(600),
            anchor: l.anchor,
            fill: fill.clone(),
            tspan_central: false,
            dy_tspans: false,
            family: None,
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

/// The rough.js drawing of `d` as elements, and its box.
fn rough_els(d: &crate::rough::Drawable, at: Point) -> (Vec<Element>, Option<Rect>) {
    let hn = super::hand::measured_node(vec![super::hand::Piece {
        drawable: d.clone(),
        at,
        measured: true,
    }]);
    let els = super::graph::hand_elements(&hn, Point::default(), None);
    let b = super::hand::drawable_bbox(d).map(|b| Rect::new(b.x + at.x, b.y + at.y, b.w, b.h));
    (els, b)
}

/// `drawLine`'s rough.js options.
fn stroke2_options(seed: f64, line: &str) -> crate::rough::Options {
    crate::rough::Options {
        roughness: 1.5,
        seed,
        stroke: line.to_string(),
        stroke_width: 2.0,
        ..Default::default()
    }
}

/// The head's and the label boxes' rough.js options.
fn boxed_options(seed: f64, line: &str, bkg: &str) -> crate::rough::Options {
    crate::rough::Options {
        fill: Some(bkg.to_string()),
        fill_style: "hachure".into(),
        fill_weight: 2.5,
        hachure_gap: 5.0,
        ..stroke2_options(seed, line)
    }
}

/// `drawArrowMarker`'s rough.js options.
fn arrow_options(seed: f64, line: &str) -> crate::rough::Options {
    crate::rough::Options {
        roughness: 1.0,
        seed,
        fill: Some(line.to_string()),
        fill_style: "solid".into(),
        stroke: line.to_string(),
        stroke_width: 1.0,
        ..Default::default()
    }
}

/// The handDrawn look: every mark drawn by rough.js (mermaid's
/// `roughContext`), the spine last; and the box of the strokes as drawn.
fn draw_hand(lay: &Layout, seed: f64, pal: &Palette) -> (Vec<Element>, Bounds) {
    use crate::rough;
    let line = pal.line_color.0.clone();
    let bkg = pal.main_bkg.0.clone();
    let text = &pal.text_color;
    let stroke2 = || stroke2_options(seed, &line);
    let boxed = || boxed_options(seed, &line, &bkg);
    let mut els = Vec::new();
    let mut b = Bounds::default();
    let add =
        |els: &mut Vec<Element>, b: &mut Bounds, d: rough::Drawable, at: Point, class: &str| {
            let (e, bb) = rough_els(&d, at);
            if let Some(bb) = bb {
                b.add_rect(bb);
            }
            els.push(Element::group(class, e));
        };
    let mut spine = None;
    for m in &lay.marks {
        match m {
            Mark::Spine { from, to } => spine = Some((*from, *to)),
            Mark::Head { y, w, h, label, .. } => {
                let d = format!(
                    "M 0 {} L 0 {} Q {} 0 0 {} Z",
                    -h / 2.0,
                    h / 2.0,
                    w * 2.4,
                    -h / 2.0
                );
                let mut g = Vec::new();
                add(
                    &mut g,
                    &mut b,
                    rough::path(&d, boxed()),
                    Point::new(0.0, *y),
                    "ishikawa-head",
                );
                b.add_rect(label.bbox());
                g.push(text_el(label, text, "ishikawa-head-label"));
                els.push(Element::group("ishikawa-head-group", g));
            }
            Mark::Bone { kind, from, to } => {
                let class = match kind {
                    BoneKind::Branch => "ishikawa-branch",
                    BoneKind::SubBranch => "ishikawa-sub-branch",
                };
                add(
                    &mut els,
                    &mut b,
                    rough::line(from.x, from.y, to.x, to.y, stroke2()),
                    Point::default(),
                    class,
                );
                // drawArrowMarker: a solid rough triangle (roughness 1) at the
                // bone's start, pointing away from its end.
                let (dx, dy) = (from.x - to.x, from.y - to.y);
                let len = (dx * dx + dy * dy).sqrt();
                if len != 0.0 {
                    let (ux, uy, s) = (dx / len, dy / len, 6.0);
                    let (px, py) = (-uy * s, ux * s);
                    let (tx, ty) = (from.x, from.y);
                    let d = format!(
                        "M {} {} L {} {} L {} {} Z",
                        tx,
                        ty,
                        tx - ux * s * 2.0 + px,
                        ty - uy * s * 2.0 + py,
                        tx - ux * s * 2.0 - px,
                        ty - uy * s * 2.0 - py
                    );
                    let o = arrow_options(seed, &line);
                    add(&mut els, &mut b, rough::path(&d, o), Point::default(), "");
                }
            }
            Mark::Category { label, rect } => {
                let mut g = Vec::new();
                add(
                    &mut g,
                    &mut b,
                    rough::rectangle(rect.x, rect.y, rect.w, rect.h, boxed()),
                    Point::default(),
                    "ishikawa-label-box",
                );
                b.add_rect(label.bbox());
                g.push(text_el(label, text, "ishikawa-label cause"));
                els.push(Element::group("ishikawa-label-group", g));
            }
            Mark::SubLabel(l) => {
                b.add_rect(l.bbox());
                els.push(text_el(l, text, "ishikawa-label"));
            }
        }
    }
    if let Some((from, to)) = spine {
        add(
            &mut els,
            &mut b,
            rough::line(from.x, from.y, to.x, to.y, stroke2()),
            Point::default(),
            "ishikawa-spine",
        );
    }
    (els, b)
}

/// Draw a laid-out diagram (`hand`: the handDrawn look's seed).
pub fn draw(lay: &Layout, padding: f64, pal: &Palette, hand: Option<f64>) -> Scene {
    if let (Some(seed), false) = (hand, lay.marks.is_empty()) {
        let (els, b) = draw_hand(lay, seed, pal);
        return Scene {
            kind: DiagramKind::Ishikawa,
            view_box: b.rect().pad(padding),
            elements: vec![Element::group("ishikawa", els)],
            acc_title: None,
            acc_descr: None,
            font_family: pal.font_family.clone(),
            css: String::new(),
        };
    }
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
            Mark::Head { y, w, h, label, .. } => {
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

// ───────────────────────────── mermaid's markup ─────────────────────────────

/// The rough.js `<g>` of `d` (`RoughSVG.draw`), with `class` when set.
fn rough_g(d: &crate::rough::Drawable, class: Option<&str>) -> String {
    use crate::rough::{Op, SetKind};
    let o = &d.options;
    let mut s = match class {
        Some(c) => format!("<g class=\"{c}\">"),
        None => "<g>".to_string(),
    };
    for set in &d.sets {
        let mut data = String::new();
        for op in &set.ops {
            match op {
                Op::Move(p) => data.push_str(&format!("M{} {} ", n(p[0]), n(p[1]))),
                Op::BCurveTo(p) => data.push_str(&format!(
                    "C{} {}, {} {}, {} {} ",
                    n(p[0]),
                    n(p[1]),
                    n(p[2]),
                    n(p[3]),
                    n(p[4]),
                    n(p[5])
                )),
                Op::LineTo(p) => data.push_str(&format!("L{} {} ", n(p[0]), n(p[1]))),
            }
        }
        let data = data.trim();
        let fill = o.fill.clone().unwrap_or_default();
        match set.kind {
            SetKind::Path => s.push_str(&format!(
                "<path d=\"{data}\" stroke=\"{}\" stroke-width=\"{}\" fill=\"none\"/>",
                esc(&o.stroke),
                n(o.stroke_width)
            )),
            SetKind::FillPath => s.push_str(&format!(
                "<path d=\"{data}\" stroke=\"none\" stroke-width=\"0\" fill=\"{}\"/>",
                esc(&fill)
            )),
            SetKind::FillSketch => s.push_str(&format!(
                "<path d=\"{data}\" stroke=\"{}\" stroke-width=\"{}\" fill=\"none\"/>",
                esc(&fill),
                n(d.fill_weight())
            )),
        }
    }
    s.push_str("</g>");
    s
}

/// `drawMultilineText`: the `<text>` and its `<tspan>`s. Text is DOMPurify's
/// output, which `.text()` escapes once more.
fn text_markup(l: &Label, class: &str, anchor: &str, transform: Option<(f64, f64)>) -> String {
    let mut s = format!(
        "<text class=\"{class}\" text-anchor=\"{anchor}\" x=\"{}\" y=\"{}\"",
        n(l.x),
        n(l.y)
    );
    if let Some((tx, ty)) = transform {
        s.push_str(&format!(" transform=\"translate({},{})\"", n(tx), n(ty)));
    }
    s.push('>');
    for (i, line) in l.lines.iter().enumerate() {
        let dy = if i == 0 { 0.0 } else { l.line_height };
        s.push_str(&format!(
            "<tspan x=\"{}\" dy=\"{}\">{}</tspan>",
            n(l.x),
            n(dy),
            esc_text(&esc_text(line))
        ));
    }
    s.push_str("</text>");
    s
}

/// A box's size from its edges, in single precision: the difference, one
/// float step wider when `lo + size` would fall short of `hi`.
fn span(lo: f32, hi: f32) -> f32 {
    let w = hi - lo;
    if lo + w < hi {
        f32::from_bits(w.to_bits() + 1)
    } else {
        w
    }
}

/// A single-precision box, as the browser's `getBBox()` holds and joins them.
#[derive(Clone, Copy)]
struct R32 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl R32 {
    fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        R32 {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        }
    }
    /// A box from its four edges (a path's bounds).
    fn edges(l: f64, t: f64, r: f64, b: f64) -> Self {
        let (l, t, r, b) = (l as f32, t as f32, r as f32, b as f32);
        R32 {
            x: l,
            y: t,
            w: span(l, r),
            h: span(t, b),
        }
    }
    fn union(self, o: R32) -> R32 {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        let (r, b) = (
            (self.x + self.w).max(o.x + o.w),
            (self.y + self.h).max(o.y + o.h),
        );
        R32 {
            x,
            y,
            w: span(x, r),
            h: span(y, b),
        }
    }
    /// Through `translate(dx, dy)`: the origin moves, the size stays.
    fn moved(self, dx: f64, dy: f64) -> R32 {
        R32 {
            x: self.x + dx as f32,
            y: self.y + dy as f32,
            ..self
        }
    }
    fn rect(self) -> Rect {
        Rect::new(
            f64::from(self.x),
            f64::from(self.y),
            f64::from(self.w),
            f64::from(self.h),
        )
    }
}

/// Join `r` into an accumulated box.
fn join(acc: &mut Option<R32>, r: Option<R32>) {
    if let Some(r) = r {
        *acc = Some(acc.map_or(r, |a| a.union(r)));
    }
}

/// A rough.js drawing's markup and the box of its group.
fn rough_part(d: &crate::rough::Drawable, class: Option<&str>) -> (String, Option<R32>) {
    let b = super::hand::drawable_bbox(d).map(|b| R32::new(b.x, b.y, b.w, b.h));
    (rough_g(d, class), b)
}

/// `drawArrowMarker`'s rough triangle at `(x, y)`, pointing along `(dx, dy)`.
fn rough_arrow(x: f64, y: f64, dx: f64, dy: f64, seed: f64, line: &str) -> (String, Option<R32>) {
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return (String::new(), None);
    }
    let (ux, uy) = (dx / len, dy / len);
    let s = 6.0;
    let (px, py) = (-uy * s, ux * s);
    let d = format!(
        "M {} {} L {} {} L {} {} Z",
        n(x),
        n(y),
        n(x - ux * s * 2.0 + px),
        n(y - uy * s * 2.0 + py),
        n(x - ux * s * 2.0 - px),
        n(y - uy * s * 2.0 - py)
    );
    rough_part(&crate::rough::path(&d, arrow_options(seed, line)), None)
}

fn line_markup(class: &str, from: Point, to: Point, marker: bool) -> String {
    let marker = if marker {
        format!(" marker-start=\"url(#ishikawa-arrow-{ID_MARK})\"")
    } else {
        String::new()
    };
    format!(
        "<line class=\"{class}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{marker}/>",
        n(from.x),
        n(from.y),
        n(to.x),
        n(to.y)
    )
}

/// A `<line>`'s box.
fn line_box(from: Point, to: Point) -> R32 {
    R32::edges(
        from.x.min(to.x),
        from.y.min(to.y),
        from.x.max(to.x),
        from.y.max(to.y),
    )
}

/// mermaid's DOM for the drawing (inside the root, after its `<style>`) and
/// the `getBBox()` of the whole.
fn markup(lay: &Layout, hand: Option<f64>, pal: &Palette) -> (String, Rect) {
    use crate::rough;
    let line = pal.line_color.0.as_str();
    let bkg = pal.main_bkg.0.as_str();
    let mut s = String::from("<g/><g class=\"ishikawa\">");
    // The boxes, joined as the DOM nests them.
    let mut all: Option<R32> = None;
    let mut pair: Option<R32> = None;
    if hand.is_none() {
        s.push_str(&format!(
            "<defs><marker id=\"ishikawa-arrow-{ID_MARK}\" viewBox=\"0 0 10 10\" refX=\"0\" refY=\"5\" markerWidth=\"6\" markerHeight=\"6\" orient=\"auto\"><path d=\"M 10 0 L 0 5 L 10 10 Z\" class=\"ishikawa-arrow\"/></marker></defs>"
        ));
    }
    let mut spine = None;
    let mut open_pair = false;
    let mut branches = 0usize;
    let mut i = 0;
    while i < lay.marks.len() {
        match &lay.marks[i] {
            Mark::Spine { from, to } => {
                spine = Some((*from, *to));
                if hand.is_none() {
                    s.push_str(&line_markup("ishikawa-spine", *from, *to, false));
                    join(&mut all, Some(line_box(*from, *to)));
                }
            }
            Mark::Head {
                y,
                w,
                h,
                label,
                attr_y,
                tx,
                ty,
                tb,
            } => {
                let mut hg: Option<R32> = None;
                s.push_str(&format!(
                    "<g class=\"ishikawa-head-group\" transform=\"translate(0,{})\">",
                    n(*y)
                ));
                let d = format!(
                    "M 0 {} L 0 {} Q {} 0 0 {} Z",
                    n(-h / 2.0),
                    n(h / 2.0),
                    n(w * 2.4),
                    n(-h / 2.0)
                );
                match hand {
                    Some(seed) => {
                        let dr = rough::path(&d, boxed_options(seed, line, bkg));
                        let (m, b) = rough_part(&dr, Some("ishikawa-head"));
                        s.push_str(&m);
                        join(&mut hg, b);
                    }
                    None => {
                        s.push_str(&format!("<path class=\"ishikawa-head\" d=\"{d}\"/>"));
                        // A quadratic's extreme x is half its control point's.
                        let right = f64::from((w * 2.4) as f32 / 2.0);
                        join(&mut hg, Some(R32::edges(0.0, -h / 2.0, right, h / 2.0)));
                    }
                }
                let mut l = label.clone();
                l.y = *attr_y;
                l.x = 0.0;
                s.push_str(&text_markup(
                    &l,
                    "ishikawa-head-label",
                    "start",
                    Some((*tx, *ty)),
                ));
                join(
                    &mut hg,
                    Some(R32::new(tb.x, tb.y, tb.w, tb.h).moved(*tx, *ty)),
                );
                s.push_str("</g>");
                join(&mut all, hg.map(|b| b.moved(0.0, *y)));
            }
            Mark::Bone {
                kind: BoneKind::Branch,
                from,
                to,
            } => {
                if branches % 2 == 0 {
                    if open_pair {
                        s.push_str("</g>");
                        join(&mut all, pair.take());
                    }
                    s.push_str("<g class=\"ishikawa-pair\">");
                    open_pair = true;
                }
                branches += 1;
                match hand {
                    Some(seed) => {
                        let dr =
                            rough::line(from.x, from.y, to.x, to.y, stroke2_options(seed, line));
                        let (m, b) = rough_part(&dr, Some("ishikawa-branch"));
                        s.push_str(&m);
                        join(&mut pair, b);
                        let (m, b) =
                            rough_arrow(from.x, from.y, from.x - to.x, from.y - to.y, seed, line);
                        s.push_str(&m);
                        join(&mut pair, b);
                    }
                    None => {
                        s.push_str(&line_markup("ishikawa-branch", *from, *to, true));
                        join(&mut pair, Some(line_box(*from, *to)));
                    }
                }
            }
            Mark::Bone { from, to, .. } => {
                // A sub-group: the bone and its label.
                let mut sg: Option<R32> = None;
                s.push_str("<g class=\"ishikawa-sub-group\">");
                match hand {
                    Some(seed) => {
                        let dr =
                            rough::line(from.x, from.y, to.x, to.y, stroke2_options(seed, line));
                        let (m, b) = rough_part(&dr, Some("ishikawa-sub-branch"));
                        s.push_str(&m);
                        join(&mut sg, b);
                        let (dx, dy) = if from.y == to.y {
                            (1.0, 0.0)
                        } else {
                            (from.x - to.x, from.y - to.y)
                        };
                        let (m, b) = rough_arrow(from.x, from.y, dx, dy, seed, line);
                        s.push_str(&m);
                        join(&mut sg, b);
                    }
                    None => {
                        s.push_str(&line_markup("ishikawa-sub-branch", *from, *to, true));
                        join(&mut sg, Some(line_box(*from, *to)));
                    }
                }
                if let Some(Mark::SubLabel(l)) = lay.marks.get(i + 1) {
                    let class = match l.baseline {
                        Baseline::Middle => "ishikawa-label align",
                        Baseline::Alphabetic => "ishikawa-label up",
                        Baseline::Hanging => "ishikawa-label down",
                    };
                    s.push_str(&text_markup(l, class, "end", None));
                    let t = l.bbox_f32();
                    join(&mut sg, Some(R32::new(t.x, t.y, t.w, t.h)));
                    i += 1;
                }
                s.push_str("</g>");
                join(&mut pair, sg);
            }
            Mark::Category { label, rect } => {
                let mut lg: Option<R32> = None;
                s.push_str("<g class=\"ishikawa-label-group\">");
                match hand {
                    Some(seed) => {
                        let dr = rough::rectangle(
                            rect.x,
                            rect.y,
                            rect.w,
                            rect.h,
                            boxed_options(seed, line, bkg),
                        );
                        let (m, b) = rough_part(&dr, Some("ishikawa-label-box"));
                        s.push_str(&m);
                        join(&mut lg, b);
                    }
                    None => {
                        s.push_str(&format!(
                            "<rect class=\"ishikawa-label-box\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                            n(rect.x),
                            n(rect.y),
                            n(rect.w),
                            n(rect.h)
                        ));
                        join(&mut lg, Some(R32::new(rect.x, rect.y, rect.w, rect.h)));
                    }
                }
                s.push_str(&text_markup(label, "ishikawa-label cause", "middle", None));
                let t = label.bbox_f32();
                join(&mut lg, Some(R32::new(t.x, t.y, t.w, t.h)));
                s.push_str("</g>");
                join(&mut pair, lg);
            }
            Mark::SubLabel(_) => {}
        }
        i += 1;
    }
    if open_pair {
        s.push_str("</g>");
        join(&mut all, pair.take());
    }
    if let (Some(seed), Some((from, to))) = (hand, spine) {
        let dr = rough::line(from.x, from.y, to.x, to.y, stroke2_options(seed, line));
        let (m, b) = rough_part(&dr, Some("ishikawa-spine"));
        s.push_str(&m);
        join(&mut all, b);
    }
    s.push_str("</g>");
    (s, all.map_or(Rect::new(0.0, 0.0, 0.0, 0.0), R32::rect))
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
    let hand = (cfg.look == crate::config::Look::HandDrawn).then(|| cfg.hand_drawn_seed());
    let mut scene = draw(&lay, padding, pal, hand);
    if let Some(r) = &root {
        // The plain SVG is mermaid's own DOM, from the exact layout; the
        // rasterisers keep the scene.
        let exact = layout_with(r, pal.font_size, true);
        let (svg, b) = markup(&exact, hand, pal);
        let pad = cfg.section_f64("ishikawa", "diagramPadding", 20.0);
        let svg_view_box = Some(Rect::new(
            b.x - pad,
            b.y - pad,
            b.w + pad * 2.0,
            b.h + pad * 2.0,
        ));
        scene.elements = vec![Element::Markup {
            svg,
            children: std::mem::take(&mut scene.elements),
            svg_view_box,
        }];
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLURRY: &str = "ishikawa-beta\n    Blurry Photo\n    Process\n        Out of focus\n        Shutter speed too slow\n        Protective film not removed\n        Beautification filter applied\n    User\n        Shaky hands\n    Equipment\n        LENS\n            Inappropriate lens\n            Damaged lens\n            Dirty lens\n        SENSOR\n            Damaged sensor\n            Dirty sensor\n    Environment\n        Subject moved too quickly\n        Too dark\n";

    /// mermaid 12.3.0, `look: handDrawn`, seed 23: the head, bones, arrow
    /// tips and label boxes are rough.js drawings; the head's outline starts
    /// at (0.099, 197.086), the "Tools" box's at (-115.39, 579.424).
    #[test]
    fn hand_drawn_marks_match_mermaid() {
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 23\n  theme: dark\n---\n\
                   ishikawa-beta\n    Late delivery\n    People\n        Understaffed\n    \
                   Process\n        No tracking\n            Manual entry\n    Tools\n";
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        fn walk(els: &[Element], out: &mut Vec<String>) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Markup { children, .. } => {
                        walk(children, out)
                    }
                    Element::Shape {
                        geom: Geometry::Path(cmds),
                        ..
                    } => out.push(crate::output::svg::path_data(cmds)),
                    _ => {}
                }
            }
        }
        let mut paths = Vec::new();
        walk(&r.scene.elements, &mut paths);
        for start in ["M0.099,197.086C", "M-115.39,579.424C"] {
            assert!(paths.iter().any(|d| d.starts_with(start)), "{start}");
        }
    }

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

    /// mermaid 12.3.0's `.ishikawa-head-label { font-weight: 600 }`: the head
    /// label's computed weight is 600, not bold's 700.
    #[test]
    fn head_label_weight_is_600() {
        let cfg = MermaidConfig::default();
        let s = render(
            "ishikawa\n  Blurry Photo\n    Cause\n",
            None,
            &cfg,
            &cfg.palette(),
        )
        .unwrap();
        fn walk(els: &[Element], out: &mut Vec<(Option<String>, Option<u16>)>) {
            for e in els {
                match e {
                    Element::Group { children, .. }
                    | Element::Markup { children, .. }
                    | Element::Rotated { children, .. } => walk(children, out),
                    Element::Text { text, class } => out.push((class.clone(), text.weight)),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&s.elements, &mut out);
        assert!(
            out.contains(&(Some("ishikawa-head-label".into()), Some(600))),
            "{out:?}"
        );
        assert!(out.iter().filter(|o| o.1.is_some()).count() == 1, "{out:?}");
    }

    #[test]
    fn head_only() {
        let cfg = MermaidConfig::default();
        let s = render("ishikawa\n  Effect\n", None, &cfg, &cfg.palette()).unwrap();
        assert_eq!(s.kind, DiagramKind::Ishikawa);
        // head 60 wide → curve reaches x = 72; h = 2 × text height + 40; padded by
        // 20. mermaid 12.3.0: viewBox "-20 194 112 112" (the 14px label box is 16).
        assert!((s.view_box.right() - 92.0).abs() < 1e-9);
        assert_eq!((s.view_box.y, s.view_box.h), (194.0, 112.0));
    }

    #[test]
    fn label_boxes_and_head_follow_get_bbox() {
        // mermaid 12.3.0 (the gallery chart): "Environment" / "User" boxes are
        // 130.375 / 74.453125 wide: their texts' getBBox() (ink rounded out to
        // pixels, here past the advance) plus 40.
        let cfg = MermaidConfig::default();
        let s = render(
            "ishikawa\n  Blurry Photo\n    Environment\n      Low light\n    User\n      Shaky hands\n",
            None,
            &cfg,
            &cfg.palette(),
        )
        .unwrap();
        fn rects(els: &[Element], out: &mut Vec<f64>) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Markup { children, .. } => {
                        rects(children, out)
                    }
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        class: Some(c),
                        ..
                    } if c == "ishikawa-label-box" => out.push(rect.w),
                    _ => {}
                }
            }
        }
        let mut w = Vec::new();
        rects(&s.elements, &mut w);
        w.sort_by(f64::total_cmp);
        assert!(
            (w[0] - 74.453125).abs() < 1e-3 && (w[1] - 130.375).abs() < 1e-3,
            "{w:?}"
        );
    }
}
