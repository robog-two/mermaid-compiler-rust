//! Sequence diagrams: parser, layout and renderer.

use crate::color;
use crate::config::{Look, SequenceConfig};
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::{ColorSlot, Palette, SlotColors};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantKind {
    Participant,
    Actor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub id: String,
    pub label: String,
    pub kind: ParticipantKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxGroup {
    pub label: String,
    pub color: Option<String>,
    pub members: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgHead {
    None,
    Filled,
    Cross,
    Async,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub dotted: bool,
    pub head: MsgHead,
    pub bidirectional: bool,
    /// `+` activates the target, `-` deactivates the source.
    pub activation: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePlacement {
    LeftOf(usize),
    RightOf(usize),
    Over(usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Loop,
    Alt,
    Opt,
    Par,
    Critical,
    Break,
    Rect,
}

impl BlockKind {
    fn word(self) -> &'static str {
        match self {
            BlockKind::Loop => "loop",
            BlockKind::Alt => "alt",
            BlockKind::Opt => "opt",
            BlockKind::Par => "par",
            BlockKind::Critical => "critical",
            BlockKind::Break => "break",
            BlockKind::Rect => "rect",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Message(Message),
    Note {
        placement: NotePlacement,
        text: String,
    },
    Activate(usize),
    Deactivate(usize),
    BlockStart {
        kind: BlockKind,
        label: String,
    },
    BlockSection {
        label: String,
    },
    BlockEnd,
    Autonumber {
        on: bool,
        start: Option<u32>,
        step: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDiagram {
    pub title: Option<String>,
    pub participants: Vec<Participant>,
    pub boxes: Vec<BoxGroup>,
    pub events: Vec<Event>,
}

const ARROWS: &[(&str, bool, MsgHead, bool)] = &[
    ("<<-->>", true, MsgHead::Filled, true),
    ("<<->>", false, MsgHead::Filled, true),
    ("-->>", true, MsgHead::Filled, false),
    ("->>", false, MsgHead::Filled, false),
    ("--x", true, MsgHead::Cross, false),
    ("-x", false, MsgHead::Cross, false),
    ("--)", true, MsgHead::Async, false),
    ("-)", false, MsgHead::Async, false),
    ("-->", true, MsgHead::None, false),
    ("->", false, MsgHead::None, false),
];

#[derive(Default)]
struct Builder {
    participants: Vec<Participant>,
    index: HashMap<String, usize>,
    boxes: Vec<BoxGroup>,
    events: Vec<Event>,
    open: Vec<bool>, // true = box, false = block
}

impl Builder {
    fn actor(&mut self, id: &str) -> usize {
        let id = id.trim();
        if let Some(&i) = self.index.get(id) {
            return i;
        }
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
            kind: ParticipantKind::Participant,
        });
        self.index
            .insert(id.to_string(), self.participants.len() - 1);
        let i = self.participants.len() - 1;
        if let Some(b) = self.current_box() {
            self.boxes[b].members.push(i);
        }
        i
    }

    fn current_box(&self) -> Option<usize> {
        if self.open.last() == Some(&true) {
            Some(self.boxes.len() - 1)
        } else {
            None
        }
    }
}

/// Parse a sequence diagram body.
pub fn parse(body: &str, title: Option<String>) -> Result<SequenceDiagram> {
    let mut b = Builder::default();
    let mut title = title;
    let mut header = false;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        if !header {
            if !s.starts_with("sequenceDiagram") {
                return Err(Error::parse(line_no, "Expecting 'sequenceDiagram'"));
            }
            header = true;
            continue;
        }
        let (kw, rest) = match s.find(char::is_whitespace) {
            Some(p) => (&s[..p], s[p..].trim()),
            None => (s, ""),
        };
        let lower = kw.to_ascii_lowercase();
        match lower.as_str() {
            "participant" | "actor" | "create" => {
                let (kind_word, rest) = if lower == "create" {
                    rest.split_once(char::is_whitespace)
                        .map(|(k, r)| (k.to_ascii_lowercase(), r.trim()))
                        .unwrap_or(("participant".into(), rest))
                } else {
                    (lower.clone(), rest)
                };
                let rest = match rest.find("@{") {
                    Some(p) => rest[..p].trim(),
                    None => rest,
                };
                let (id, label) = match rest.find(" as ") {
                    Some(p) => (rest[..p].trim(), Some(rest[p + 4..].trim())),
                    None => (rest, None),
                };
                if id.is_empty() {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\nExpecting a participant name"),
                    ));
                }
                let idx = b.actor(id);
                if let Some(l) = label {
                    b.participants[idx].label = l.to_string();
                }
                if kind_word == "actor" {
                    b.participants[idx].kind = ParticipantKind::Actor;
                }
            }
            "destroy" | "link" | "links" | "properties" | "details" => {}
            "autonumber" => {
                let nums: Vec<u32> = rest
                    .split_whitespace()
                    .filter_map(|w| w.parse().ok())
                    .collect();
                b.events.push(Event::Autonumber {
                    on: rest != "off",
                    start: nums.first().copied(),
                    step: nums.get(1).copied(),
                });
            }
            "title" | "title:" => title = Some(rest.trim_start_matches(':').trim().to_string()),
            _ if s.starts_with("title:") => title = Some(s[6..].trim().to_string()),
            "activate" => {
                let a = b.actor(rest);
                b.events.push(Event::Activate(a));
            }
            "deactivate" => {
                let a = b.actor(rest);
                b.events.push(Event::Deactivate(a));
            }
            "note" => {
                let (head, text) = rest.split_once(':').ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting ':' after note placement"))
                })?;
                let head = head.trim();
                let placement = if let Some(a) = head.strip_prefix("left of") {
                    NotePlacement::LeftOf(b.actor(a))
                } else if let Some(a) = head.strip_prefix("right of") {
                    NotePlacement::RightOf(b.actor(a))
                } else if let Some(a) = head.strip_prefix("over") {
                    let mut parts = a.split(',');
                    let first = b.actor(parts.next().unwrap_or(""));
                    let second = parts.next().map(|p| b.actor(p)).unwrap_or(first);
                    NotePlacement::Over(first, second)
                } else {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\nExpecting 'left of', 'right of' or 'over'"),
                    ));
                };
                b.events.push(Event::Note {
                    placement,
                    text: text.trim().to_string(),
                });
            }
            "box" => {
                let (color, label) = split_box_color(rest);
                b.boxes.push(BoxGroup {
                    label,
                    color,
                    members: Vec::new(),
                });
                b.open.push(true);
            }
            "loop" | "alt" | "opt" | "par" | "par_over" | "critical" | "break" | "rect" => {
                let kind = match lower.as_str() {
                    "loop" => BlockKind::Loop,
                    "alt" => BlockKind::Alt,
                    "opt" => BlockKind::Opt,
                    "par" | "par_over" => BlockKind::Par,
                    "critical" => BlockKind::Critical,
                    "break" => BlockKind::Break,
                    _ => BlockKind::Rect,
                };
                b.events.push(Event::BlockStart {
                    kind,
                    label: rest.to_string(),
                });
                b.open.push(false);
            }
            "else" | "and" | "option" => {
                if b.open.last() != Some(&false) {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\n'{kw}' outside of a block"),
                    ));
                }
                b.events.push(Event::BlockSection {
                    label: rest.to_string(),
                });
            }
            "end" => match b.open.pop() {
                Some(true) => {}
                Some(false) => b.events.push(Event::BlockEnd),
                None => return Err(Error::parse(line_no, "Unexpected 'end'")),
            },
            _ => {
                let m = parse_message(s, &mut b).ok_or_else(|| {
                    Error::parse(
                        line_no,
                        format!("{s}\nExpecting a message like 'Alice->>Bob: Hello'"),
                    )
                })?;
                b.events.push(Event::Message(m));
            }
        }
    }
    if !b.open.is_empty() {
        return Err(Error::parse(
            body.lines().count(),
            "Unexpected end of input: missing 'end'",
        ));
    }
    Ok(SequenceDiagram {
        title,
        participants: b.participants,
        boxes: b.boxes,
        events: b.events,
    })
}

fn split_box_color(rest: &str) -> (Option<String>, String) {
    let lower = rest.to_ascii_lowercase();
    if lower.starts_with("rgb") || lower.starts_with("hsl") {
        if let Some(end) = rest.find(')') {
            return (
                Some(rest[..=end].to_string()),
                rest[end + 1..].trim().to_string(),
            );
        }
    }
    let (first, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if !first.is_empty() && (first == "transparent" || color::parse(first).is_some()) {
        (Some(first.to_string()), tail.trim().to_string())
    } else {
        (None, rest.to_string())
    }
}

fn parse_message(s: &str, b: &mut Builder) -> Option<Message> {
    let start = s.find(['-', '<'])?;
    let (arrow, dotted, head, bidi) = ARROWS.iter().find(|(a, ..)| s[start..].starts_with(a))?;
    let from = s[..start].trim();
    let mut rest = &s[start + arrow.len()..];
    let activation = match rest.chars().next() {
        Some('+') => {
            rest = &rest[1..];
            Some(true)
        }
        Some('-') => {
            rest = &rest[1..];
            Some(false)
        }
        _ => None,
    };
    let (to, text) = match rest.split_once(':') {
        Some((t, x)) => (t.trim(), x.trim()),
        None => (rest.trim(), ""),
    };
    // sequenceDiagram.jison's message ACTOR token excludes `<`, `>` and `+`, so e.g.
    // `A-->>>B` (actor `>B`) is a syntax error upstream.
    let bad = |a: &str| a.is_empty() || a.contains(['<', '>', '+']);
    if bad(from) || bad(to) {
        return None;
    }
    let from = b.actor(from);
    let to = b.actor(to);
    Some(Message {
        from,
        to,
        text: text.to_string(),
        dotted: *dotted,
        head: *head,
        bidirectional: *bidi,
        activation,
    })
}

// ---------------------------------------------------------------------------
// Layout + render
// ---------------------------------------------------------------------------

struct Frame {
    kind: BlockKind,
    label: String,
    top: f64,
    sections: Vec<(f64, String)>,
    min_x: f64,
    max_x: f64,
    color: Option<String>,
}

/// Render a sequence diagram.
pub fn render(d: &SequenceDiagram, cfg: &SequenceConfig, pal: &Palette) -> Scene {
    let n = d.participants.len();
    let font = cfg.message_font_size;
    let labels: Vec<TextBlock> = d
        .participants
        .iter()
        .map(|p| TextBlock::label(&p.label, cfg.actor_font_size))
        .collect();
    let widths: Vec<f64> = labels
        .iter()
        .map(|l| cfg.width.max(l.width + 2.0 * cfg.wrap_padding))
        .collect();
    let actor_h = labels
        .iter()
        .map(|l| l.height + 2.0 * cfg.box_margin)
        .fold(cfg.height, f64::max);

    // Horizontal spacing.
    let mut gaps: Vec<f64> = (0..n.saturating_sub(1))
        .map(|i| widths[i] / 2.0 + cfg.actor_margin + widths[i + 1] / 2.0)
        .collect();
    let note_width = |text: &str| {
        cfg.width
            .max(TextBlock::label(text, cfg.note_font_size).width + 2.0 * cfg.note_margin)
    };
    let mut spans: Vec<(usize, usize, f64)> = Vec::new();
    for ev in &d.events {
        match ev {
            Event::Message(m) if m.from != m.to => {
                let mut w = TextBlock::label(&m.text, font).width + 2.0 * cfg.wrap_padding;
                // mermaid sizes the margin between neighbours as message + actorMargin.
                if m.from.abs_diff(m.to) == 1 {
                    w += cfg.actor_margin;
                }
                spans.push((m.from.min(m.to), m.from.max(m.to), w));
            }
            Event::Message(m) => {
                let w = TextBlock::label(&m.text, font).width;
                if m.from + 1 < n {
                    spans.push((
                        m.from,
                        m.from + 1,
                        w / 2.0 + 60.0 + widths[m.from + 1] / 2.0,
                    ));
                }
            }
            Event::Note {
                placement: NotePlacement::RightOf(a),
                text,
            } if a + 1 < n => {
                spans.push((
                    *a,
                    a + 1,
                    25.0 + note_width(text) + 10.0 + widths[a + 1] / 2.0 - 0.0,
                ));
            }
            Event::Note {
                placement: NotePlacement::LeftOf(a),
                text,
            } if *a > 0 => {
                spans.push((
                    a - 1,
                    *a,
                    25.0 + note_width(text) + 10.0 + widths[a - 1] / 2.0,
                ));
            }
            Event::Note {
                placement: NotePlacement::Over(a, b),
                text,
            } if a != b => {
                spans.push(((*a).min(*b), (*a).max(*b), note_width(text) - 50.0));
            }
            _ => {}
        }
    }
    spans.sort_by_key(|s| s.1 - s.0);
    for (lo, hi, need) in spans {
        let have: f64 = gaps[lo..hi].iter().sum();
        if have < need {
            gaps[hi - 1] += need - have;
        }
    }

    // Participant boxes (`box ... end`), as in mermaid's `calculateActorMargins`
    // and `addActorRenderingData`: each box gets an inner `margin`
    // (`boxTextMargin`, more if its title is wider than its members) on both
    // sides, and `boxMargin` more separates it from what follows.
    let box_of: Vec<Option<usize>> = (0..n)
        .map(|i| d.boxes.iter().position(|b| b.members.contains(&i)))
        .collect();
    let box_title_h = d
        .boxes
        .iter()
        .filter(|b| !b.label.is_empty())
        .map(|b| TextBlock::label(&b.label, font).height)
        .fold(0.0f64, f64::max);
    let box_inner: Vec<f64> = d
        .boxes
        .iter()
        .map(|b| {
            let total: f64 = b
                .members
                .iter()
                .map(|&m| {
                    let margin = if m + 1 < n {
                        gaps[m] - widths[m] / 2.0 - widths[m + 1] / 2.0
                    } else {
                        cfg.actor_margin
                    };
                    widths[m] + margin
                })
                .sum::<f64>()
                + 8.0 * cfg.box_margin
                - 2.0 * cfg.box_text_margin;
            let title_w = if b.label.is_empty() {
                0.0
            } else {
                TextBlock::label(&b.label, font).width
            };
            let min_w = total.max(title_w + 2.0 * cfg.wrap_padding);
            cfg.box_text_margin + (min_w - total).max(0.0) / 2.0
        })
        .collect();
    for i in 0..n.saturating_sub(1) {
        if box_of[i] != box_of[i + 1] {
            if let Some(b) = box_of[i] {
                gaps[i] += cfg.box_margin + box_inner[b];
            }
            if let Some(c) = box_of[i + 1] {
                gaps[i] += box_inner[c];
            }
        }
    }
    let lead = box_of
        .first()
        .copied()
        .flatten()
        .map_or(0.0, |b| box_inner[b]);
    let has_box_labels = d.boxes.iter().any(|b| !b.label.is_empty());
    let title_band = if d.title.as_ref().is_some_and(|t| !t.is_empty()) {
        40.0
    } else {
        0.0
    };
    // Boxes start `boxMargin / 2` above the diagram top; actors follow after
    // `boxMargin` plus the tallest box title.
    let box_band = if d.boxes.is_empty() {
        0.0
    } else if has_box_labels {
        1.5 * cfg.box_margin + box_title_h
    } else {
        1.5 * cfg.box_margin
    };
    let top = cfg.diagram_margin_y + title_band + box_band;
    let mut xs = Vec::with_capacity(n);
    let mut x = cfg.diagram_margin_x + lead + widths.first().copied().unwrap_or(0.0) / 2.0;
    for i in 0..n {
        if i > 0 {
            x += gaps[i - 1];
        }
        xs.push(x);
    }

    let mut bounds = Bounds::default();
    let mut background = Vec::new();
    let mut activations_el = Vec::new();
    let mut frames_el = Vec::new();
    let mut messages_el = Vec::new();
    let mut notes_el = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut active: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut autonumber: Option<(u32, u32)> = None;
    let mut cursor = top + actor_h;

    let extend_frames = |frames: &mut Vec<Frame>, lo: f64, hi: f64| {
        for f in frames.iter_mut() {
            f.min_x = f.min_x.min(lo);
            f.max_x = f.max_x.max(hi);
        }
    };
    let act_offset = |stack: &Vec<f64>| {
        if stack.is_empty() {
            0.0
        } else {
            5.0 + (stack.len() - 1) as f64 * 5.0
        }
    };

    let push_activation =
        |els: &mut Vec<Element>, a: usize, depth: usize, y0: f64, y1: f64, xs: &[f64]| {
            let r = Rect::new(
                xs[a] - 5.0 + depth as f64 * 5.0,
                y0,
                10.0,
                (y1 - y0).max(4.0),
            );
            // Colour themes paint an activation in its actor's slot, falling back to
            // `mainBkg` for the fill (svgDraw.js `drawActivation`).
            let paint = match actor_slot(pal, a) {
                Some(sc) => Paint::both(sc.fill.as_ref().unwrap_or(&pal.main_bkg), &sc.border, 1.0),
                None => Paint::both(&pal.activation_bkg, &pal.activation_border, 1.0),
            };
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx: 0.0 },
                paint,
                format!("activation{}", depth % 3),
            ));
        };

    for ev in &d.events {
        match ev {
            Event::Autonumber { on, start, step } => {
                autonumber = if *on {
                    Some((start.unwrap_or(1), step.unwrap_or(1)))
                } else {
                    None
                };
            }
            Event::Activate(a) => active[*a].push(cursor),
            Event::Deactivate(a) => {
                if let Some(y0) = active[*a].pop() {
                    push_activation(&mut activations_el, *a, active[*a].len(), y0, cursor, &xs);
                }
            }
            Event::Message(m) => {
                let block = TextBlock::label(&m.text, font);
                let line_y = cursor + block.height + 18.0;
                let (x1, x2) = (xs[m.from], xs[m.to]);
                let paint = {
                    let p = Paint::stroke(&pal.signal_color, 1.5);
                    if m.dotted {
                        p.dashed(vec![3.0, 3.0])
                    } else {
                        p
                    }
                };
                let mut els = Vec::new();
                if m.from == m.to {
                    let xa = x1 + act_offset(&active[m.from]);
                    let cmds = vec![
                        PathCmd::MoveTo(Point::new(xa, line_y)),
                        PathCmd::CubicTo(
                            Point::new(xa + 60.0, line_y - 10.0),
                            Point::new(xa + 60.0, line_y + 40.0),
                            Point::new(xa, line_y + 30.0),
                        ),
                    ];
                    els.push(Element::classed_shape(
                        Geometry::Path(cmds),
                        paint,
                        "messageLine0",
                    ));
                    els.extend(msg_head(
                        m.head,
                        Point::new(xa, line_y + 30.0),
                        Point::new(-1.0, 0.0),
                        &pal.signal_color,
                    ));
                    let tx = xa + 30.0;
                    els.push(Element::classed_text(
                        Text::centered(
                            Point::new(tx, line_y - 6.0 - block.height / 2.0),
                            &block.lines,
                            font,
                            block.line_height,
                            &pal.signal_text,
                        ),
                        "messageText",
                    ));
                    extend_frames(
                        &mut frames,
                        x1 - 10.0,
                        tx.max(xa + 60.0) + block.width / 2.0,
                    );
                    bounds.add_rect(Rect::new(
                        tx - block.width / 2.0,
                        line_y - 6.0 - block.height,
                        block.width,
                        block.height,
                    ));
                    bounds.add_point(Point::new(xa + 60.0, line_y + 30.0));
                    cursor = line_y + 30.0 + 2.0;
                } else {
                    let dir = if x2 > x1 { 1.0 } else { -1.0 };
                    let from_x = x1 + dir * act_offset(&active[m.from]);
                    // Activating the target this message: the arrow lands on the new bar.
                    let target_depth = active[m.to].len() + usize::from(m.activation == Some(true));
                    let to_off = if target_depth == 0 {
                        0.0
                    } else {
                        5.0 + (target_depth - 1) as f64 * 5.0
                    };
                    let to_x = x2 - dir * to_off;
                    let tip = Point::new(to_x, line_y);
                    let back = Point::new(from_x, line_y);
                    let d_vec = Point::new(dir, 0.0);
                    let end_pull = if m.head == MsgHead::Filled { 8.0 } else { 0.0 };
                    let start_pull = if m.bidirectional { 8.0 } else { 0.0 };
                    els.push(Element::classed_shape(
                        Geometry::Path(vec![
                            PathCmd::MoveTo(back.plus(d_vec.scale(start_pull))),
                            PathCmd::LineTo(tip.minus(d_vec.scale(end_pull))),
                        ]),
                        paint,
                        if m.dotted {
                            "messageLine1"
                        } else {
                            "messageLine0"
                        },
                    ));
                    els.extend(msg_head(m.head, tip, d_vec, &pal.signal_color));
                    if m.bidirectional {
                        els.extend(msg_head(m.head, back, d_vec.scale(-1.0), &pal.signal_color));
                    }
                    let mid = (from_x + to_x) / 2.0;
                    if !block.is_empty() {
                        els.push(Element::classed_text(
                            Text::centered(
                                Point::new(mid, line_y - 6.0 - block.height / 2.0),
                                &block.lines,
                                font,
                                block.line_height,
                                &pal.signal_text,
                            ),
                            "messageText",
                        ));
                        bounds.add_rect(Rect::new(
                            mid - block.width / 2.0,
                            line_y - 6.0 - block.height,
                            block.width,
                            block.height,
                        ));
                    }
                    extend_frames(&mut frames, x1.min(x2), x1.max(x2));
                    cursor = line_y + 2.0;
                }
                if let Some((num, step)) = autonumber.as_mut() {
                    let c = Point::new(xs[m.from], line_y);
                    els.push(Element::classed_shape(
                        Geometry::Circle { c, r: 9.0 },
                        Paint::both(&pal.signal_color, &pal.signal_color, 1.0),
                        "sequenceNumber",
                    ));
                    els.push(Element::text(Text::centered(
                        c,
                        &[num.to_string()],
                        12.0,
                        12.0,
                        &pal.sequence_number,
                    )));
                    *num += *step;
                }
                messages_el.push(Element::group("message", els));
                match m.activation {
                    Some(true) => active[m.to].push(line_y),
                    Some(false) => {
                        if let Some(y0) = active[m.from].pop() {
                            push_activation(
                                &mut activations_el,
                                m.from,
                                active[m.from].len(),
                                y0,
                                line_y,
                                &xs,
                            );
                        }
                    }
                    None => {}
                }
            }
            Event::Note { placement, text } => {
                let block = TextBlock::label(text, cfg.note_font_size);
                let w = note_width(text);
                let h = block.height + 2.0 * cfg.note_margin;
                let y = cursor + 10.0;
                let x0 = match *placement {
                    NotePlacement::RightOf(a) => xs[a] + 25.0,
                    NotePlacement::LeftOf(a) => xs[a] - 25.0 - w,
                    NotePlacement::Over(a, b) if a == b => xs[a] - w / 2.0,
                    NotePlacement::Over(a, b) => {
                        let (l, r) = (xs[a].min(xs[b]) - 25.0, xs[a].max(xs[b]) + 25.0);
                        let ww = (r - l).max(w);
                        (l + r) / 2.0 - ww / 2.0
                    }
                };
                let w = match *placement {
                    NotePlacement::Over(a, b) if a != b => ((xs[a] - xs[b]).abs() + 50.0).max(w),
                    _ => w,
                };
                let r = Rect::new(x0, y, w, h);
                bounds.add_rect(r);
                extend_frames(&mut frames, r.x, r.right());
                notes_el.push(Element::group(
                    "note",
                    vec![
                        Element::classed_shape(
                            Geometry::Rect { rect: r, rx: 0.0 },
                            Paint::both(&pal.note_bkg, &pal.note_border, 1.0),
                            "note",
                        ),
                        Element::classed_text(
                            Text::centered(
                                r.center(),
                                &block.lines,
                                cfg.note_font_size,
                                block.line_height,
                                &pal.note_text,
                            ),
                            "noteText",
                        ),
                    ],
                ));
                cursor = y + h;
            }
            Event::BlockStart { kind, label } => {
                let block = TextBlock::label(label, font);
                let has_title = !label.is_empty();
                let top_y = cursor + cfg.box_margin;
                let (color, label) = if *kind == BlockKind::Rect {
                    (Some(label.clone()), String::new())
                } else {
                    (None, label.clone())
                };
                frames.push(Frame {
                    kind: *kind,
                    label,
                    top: top_y,
                    sections: Vec::new(),
                    min_x: f64::INFINITY,
                    max_x: f64::NEG_INFINITY,
                    color,
                });
                // mermaid `adjustLoopHeightForWrap`: after the frame top come
                // `boxMargin + boxTextMargin` plus the title (at least
                // `labelBoxHeight`); a `rect` only gets `boxMargin`.
                cursor = top_y
                    + if *kind == BlockKind::Rect {
                        cfg.box_margin
                    } else if !has_title {
                        cfg.box_margin + cfg.box_text_margin
                    } else {
                        cfg.box_margin + cfg.box_text_margin + block.height.max(LABEL_BOX_H)
                    };
            }
            Event::BlockSection { label } => {
                let block = TextBlock::label(label, font);
                // Sections: `boxMargin + boxTextMargin` before the divider,
                // `boxMargin` plus the (non-empty) title after it.
                let y = cursor + cfg.box_margin + cfg.box_text_margin;
                if let Some(f) = frames.last_mut() {
                    f.sections.push((y, label.clone()));
                }
                cursor = y
                    + cfg.box_margin
                    + if label.is_empty() {
                        0.0
                    } else {
                        block.height.max(LABEL_BOX_H)
                    };
            }
            Event::BlockEnd => {
                cursor += 10.0;
                let Some(mut f) = frames.pop() else { continue };
                if !f.min_x.is_finite() {
                    f.min_x = xs.first().copied().unwrap_or(0.0) - 50.0;
                    f.max_x = xs.last().copied().unwrap_or(0.0) + 50.0;
                }
                let label_w = TextBlock::label(&f.label, font).width;
                let mut left = f.min_x - 11.0;
                let mut right = f.max_x + 11.0;
                let need = 50.0 + label_w + 30.0;
                if right - left < need {
                    let c = (left + right) / 2.0;
                    left = c - need / 2.0;
                    right = c + need / 2.0;
                }
                let r = Rect::new(left, f.top, right - left, cursor - f.top);
                bounds.add_rect(r);
                // Frames sit 11px outside their content and each enclosing
                // level adds `boxMargin` (10) more, as in mermaid's nested loops.
                for parent in frames.iter_mut() {
                    parent.min_x = parent.min_x.min(left + 11.0 - cfg.box_margin);
                    parent.max_x = parent.max_x.max(right - 11.0 + cfg.box_margin);
                }
                if f.kind == BlockKind::Rect {
                    let fill =
                        Color::new(f.color.clone().unwrap_or_else(|| "rgba(0,0,0,0.1)".into()));
                    // mermaid lowers each finished rect below the earlier ones, so
                    // an enclosing rect (which ends later) stays behind nested ones.
                    background.insert(
                        0,
                        Element::classed_shape(
                            Geometry::Rect { rect: r, rx: 0.0 },
                            Paint::fill(&fill),
                            "rect",
                        ),
                    );
                } else {
                    frames_el.push(frame_elements(&f, r, pal, font, cfg.look == Look::Neo));
                }
                cursor += 2.0;
            }
        }
    }
    cursor += 10.0;
    for (a, stack) in active.iter_mut().enumerate() {
        while let Some(y0) = stack.pop() {
            push_activation(&mut activations_el, a, stack.len(), y0, cursor - 10.0, &xs);
        }
    }

    // Actors, lifelines and boxes.
    let bottom_top = cursor;
    let mut actors_el = Vec::new();
    let mut lifelines = Vec::new();
    for (i, p) in d.participants.iter().enumerate() {
        let r_top = Rect::new(xs[i] - widths[i] / 2.0, top, widths[i], actor_h);
        bounds.add_rect(r_top);
        lifelines.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(xs[i], top + actor_h)),
                PathCmd::LineTo(Point::new(xs[i], bottom_top)),
            ]),
            Paint::stroke(&pal.actor_line, 0.5),
            "actor-line",
        ));
        actors_el.push(actor_elements(p, i, r_top, &labels[i], pal, cfg));
        if cfg.mirror_actors {
            let r_bot = Rect::new(r_top.x, bottom_top, widths[i], actor_h);
            bounds.add_rect(r_bot);
            actors_el.push(actor_elements(p, i, r_bot, &labels[i], pal, cfg));
        }
    }
    let mut boxes_el = Vec::new();
    let content_bottom = if cfg.mirror_actors {
        bottom_top + actor_h
    } else {
        bottom_top
    };
    for (bi, bx) in d.boxes.iter().enumerate() {
        if bx.members.is_empty() {
            continue;
        }
        // mermaid pads the box by `2 * boxMargin` beyond its inner margin.
        let pad = box_inner[bi] + 2.0 * cfg.box_margin;
        let l = bx
            .members
            .iter()
            .map(|&m| xs[m] - widths[m] / 2.0)
            .fold(f64::INFINITY, f64::min)
            - pad;
        let r = bx
            .members
            .iter()
            .map(|&m| xs[m] + widths[m] / 2.0)
            .fold(f64::NEG_INFINITY, f64::max)
            + pad;
        let rect = Rect::new(
            l,
            top - box_band,
            r - l,
            content_bottom - top + box_band + 2.0 * cfg.box_margin,
        );
        bounds.add_rect(rect);
        let fill = Color::new(bx.color.clone().unwrap_or_else(|| "none".into()));
        let mut els = vec![Element::shape(
            Geometry::Rect { rect, rx: 0.0 },
            Paint::both(&fill, &Color::new("rgba(0,0,0,0.5)"), 1.0),
        )];
        if !bx.label.is_empty() {
            let t = TextBlock::label(&bx.label, font);
            els.push(Element::text(Text::centered(
                Point::new(
                    rect.center().x,
                    rect.y + 0.5 * cfg.box_margin + cfg.box_text_margin + box_title_h / 2.0,
                ),
                &t.lines,
                font,
                t.line_height,
                &pal.text_color,
            )));
        }
        boxes_el.push(Element::group("box", els));
    }

    let mut title_el = Vec::new();
    if let Some(t) = d.title.as_ref().filter(|t| !t.is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], 18.0, true);
        let c = Point::new(content.x + content.w / 2.0, cfg.diagram_margin_y + 14.0);
        bounds.add_rect(Rect::centered(c, tb.width, tb.height));
        title_el.push(Element::classed_text(
            Text::centered(c, &tb.lines, 18.0, 27.0, &pal.text_color).bold(),
            "titleText",
        ));
    }

    let content = bounds.rect();
    let view_box = Rect::new(
        content.x - cfg.diagram_margin_x,
        content.y - cfg.diagram_margin_y,
        content.w + 2.0 * cfg.diagram_margin_x,
        content.h + 2.0 * cfg.diagram_margin_y,
    );
    let elements = vec![
        Element::group("boxes", boxes_el),
        Element::group("rects", background),
        Element::group("lifelines", lifelines),
        Element::group("activations", activations_el),
        Element::group("frames", frames_el),
        Element::group("messages", messages_el),
        Element::group("notes", notes_el),
        Element::group("actors", actors_el),
        Element::group("title", title_el),
    ];
    Scene {
        kind: DiagramKind::Sequence,
        view_box,
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn msg_head(head: MsgHead, tip: Point, dir: Point, color: &Color) -> Vec<Element> {
    let perp = Point::new(-dir.y, dir.x);
    let at = |back: f64, side: f64| tip.minus(dir.scale(back)).plus(perp.scale(side));
    match head {
        MsgHead::None => vec![],
        MsgHead::Filled => vec![Element::classed_shape(
            Geometry::Path(geom::polygon(&[tip, at(10.0, 5.0), at(10.0, -5.0)])),
            Paint::both(color, color, 1.0),
            "arrowhead",
        )],
        MsgHead::Cross => {
            let c = at(6.0, 0.0);
            let p = Paint::stroke(color, 2.0);
            vec![
                Element::line(
                    c.plus(Point::new(-5.0, -5.0)),
                    c.plus(Point::new(5.0, 5.0)),
                    p.clone(),
                ),
                Element::line(
                    c.plus(Point::new(-5.0, 5.0)),
                    c.plus(Point::new(5.0, -5.0)),
                    p,
                ),
            ]
        }
        MsgHead::Async => vec![Element::path(
            vec![
                PathCmd::MoveTo(at(9.0, 5.0)),
                PathCmd::LineTo(tip),
                PathCmd::LineTo(at(9.0, -5.0)),
            ],
            Paint::stroke(color, 1.5),
        )],
    }
}

/// `labelBoxHeight` (sequence config default).
const LABEL_BOX_H: f64 = 20.0;
/// `labelBoxWidth` (sequence config default).
const LABEL_BOX_W: f64 = 50.0;

fn frame_elements(f: &Frame, r: Rect, pal: &Palette, font: f64, neo: bool) -> Element {
    let line = Paint::stroke(&pal.label_box_border, 2.0).dashed(vec![2.0, 2.0]);
    let mut els = vec![Element::classed_shape(
        Geometry::Rect { rect: r, rx: 0.0 },
        line.clone(),
        "loopLine",
    )];
    let word = f.kind.word();
    // svgDraw `drawLabel`: a 50-wide tag (`labelBoxHeight`, +15 in the neo
    // look) with its bottom-right corner cut by 7, label in the message font.
    let word_w = LABEL_BOX_W.max(crate::text::line_width(word, font, false) + 10.0);
    let tag_h = LABEL_BOX_H + if neo { 15.0 } else { 0.0 };
    let cut = 7.0;
    let (x, y) = (r.x, r.y);
    let tag = geom::polygon(&[
        Point::new(x, y),
        Point::new(x + word_w, y),
        Point::new(x + word_w, y + tag_h - cut),
        Point::new(x + word_w - cut * 1.2, y + tag_h),
        Point::new(x, y + tag_h),
    ]);
    els.push(Element::classed_shape(
        Geometry::Path(tag),
        Paint::both(&pal.label_box_bkg, &pal.label_box_border, 1.0),
        "labelBox",
    ));
    let lh = TextBlock::label(word, font).line_height;
    els.push(Element::classed_text(
        Text::centered(
            Point::new(x + word_w / 2.0, y + tag_h / 2.0),
            &[word.to_string()],
            font,
            lh,
            &pal.label_text,
        ),
        "labelText",
    ));
    if !f.label.is_empty() {
        let t = TextBlock::label(&format!("[{}]", f.label), font);
        let cx = (r.x + word_w + r.right()) / 2.0;
        els.push(Element::classed_text(
            Text::centered(
                Point::new(cx, y + 2.0 + t.height / 2.0),
                &t.lines,
                font,
                t.line_height,
                &pal.loop_text,
            ),
            "loopText",
        ));
    }
    for (sy, label) in &f.sections {
        els.push(Element::line(
            Point::new(r.x, *sy),
            Point::new(r.right(), *sy),
            line.clone(),
        ));
        if !label.is_empty() {
            let t = TextBlock::label(&format!("[{label}]"), font);
            els.push(Element::classed_text(
                Text::centered(
                    Point::new(r.center().x, sy + 2.0 + t.height / 2.0),
                    &t.lines,
                    font,
                    t.line_height,
                    &pal.loop_text,
                ),
                "loopText",
            ));
        }
    }
    Element::group("frame", els)
}

/// Palette slot of the `index`-th participant (svgDraw.js `paletteColor(palette,
/// actorIndexMap.get(name))`, the index being the participant's declaration order).
fn actor_slot(pal: &Palette, index: usize) -> Option<SlotColors> {
    Some(pal.color_slots.as_ref()?.slot(ColorSlot(index)))
}

fn actor_elements(
    p: &Participant,
    index: usize,
    r: Rect,
    label: &TextBlock,
    pal: &Palette,
    cfg: &SequenceConfig,
) -> Element {
    let mut els = Vec::new();
    // `.actor { stroke-width: strokeWidth ?? 1 }`; an empty background palette leaves the
    // stylesheet's `actorBkg` in charge (d3 removes the inline fill).
    let slot = actor_slot(pal, index);
    let actor_border = slot.as_ref().map_or(&pal.actor_border, |s| &s.border);
    let actor_bkg = slot
        .as_ref()
        .and_then(|s| s.fill.as_ref())
        .unwrap_or(&pal.actor_bkg);
    let box_width = pal.stroke_width.unwrap_or(1.0);
    let rx = if cfg.look == Look::Neo { 6.0 } else { 3.0 };
    match p.kind {
        ParticipantKind::Participant => {
            // The neo look gives actor boxes the theme's drop shadow.
            if let Some(sh) = pal.drop_shadow.as_ref().filter(|_| cfg.look == Look::Neo) {
                els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::new(r.x + sh.dx, r.y + sh.dy, r.w, r.h),
                        rx,
                    },
                    Paint::fill(&sh.color).with_opacity(sh.opacity),
                    "drop-shadow",
                ));
            }
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx },
                Paint::both(actor_bkg, actor_border, box_width),
                "actor",
            ));
            els.push(Element::classed_text(
                Text::centered(
                    r.center(),
                    &label.lines,
                    cfg.actor_font_size,
                    label.line_height,
                    &pal.actor_text,
                ),
                "actor",
            ));
        }
        ParticipantKind::Actor => {
            let cx = r.center().x;
            let y = r.y;
            let stroke = Paint::both(actor_bkg, actor_border, 2.0);
            let line = Paint::stroke(actor_border, 2.0);
            els.push(Element::shape(
                Geometry::Circle {
                    c: Point::new(cx, y + 8.0),
                    r: 8.0,
                },
                stroke,
            ));
            els.push(Element::line(
                Point::new(cx, y + 16.0),
                Point::new(cx, y + 32.0),
                line.clone(),
            ));
            els.push(Element::line(
                Point::new(cx - 12.0, y + 21.0),
                Point::new(cx + 12.0, y + 21.0),
                line.clone(),
            ));
            els.push(Element::line(
                Point::new(cx, y + 32.0),
                Point::new(cx - 10.0, y + 44.0),
                line.clone(),
            ));
            els.push(Element::line(
                Point::new(cx, y + 32.0),
                Point::new(cx + 10.0, y + 44.0),
                line,
            ));
            let ty = y + 44.0 + (r.h - 44.0) / 2.0;
            let mut t = Text::centered(
                Point::new(cx, ty),
                &label.lines,
                cfg.actor_font_size,
                label.line_height,
                &pal.actor_text,
            );
            t.anchor = Anchor::Middle;
            els.push(Element::classed_text(t, "actor"));
        }
    }
    Element::group("actor", els)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages_and_blocks() {
        let d = parse(
            "sequenceDiagram\nparticipant A as Alice\nactor B\nautonumber\nA->>+B: Hello\nB-->>-A: Hi\nloop Every minute\n  A-)B: ping\nend\nalt ok\n A-xB: x\nelse fail\n A--xB: y\nend\nNote over A,B: shared\nA<<->>B: both",
            None,
        )
        .unwrap();
        assert_eq!(d.participants.len(), 2);
        assert_eq!(d.participants[0].label, "Alice");
        assert_eq!(d.participants[1].kind, ParticipantKind::Actor);
        let msgs: Vec<&Message> = d
            .events
            .iter()
            .filter_map(|e| {
                if let Event::Message(m) = e {
                    Some(m)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(msgs[0].activation, Some(true));
        assert!(msgs[1].dotted && msgs[1].activation == Some(false));
        assert_eq!(msgs[2].head, MsgHead::Async);
        assert_eq!(msgs[3].head, MsgHead::Cross);
        assert!(msgs[4].dotted && msgs[4].head == MsgHead::Cross);
        assert!(msgs[5].bidirectional);
    }

    #[test]
    fn boxes_group_participants() {
        let d = parse("sequenceDiagram\nbox rgb(200, 220, 255) Team\nparticipant A\nparticipant B\nend\nA->>C: hi", None).unwrap();
        assert_eq!(d.boxes[0].color.as_deref(), Some("rgb(200, 220, 255)"));
        assert_eq!(d.boxes[0].label, "Team");
        assert_eq!(d.boxes[0].members, vec![0, 1]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("sequenceDiagram\nloop x\nA->>B: hi", None).is_err());
        assert!(parse("sequenceDiagram\nend", None).is_err());
        assert!(parse("sequenceDiagram\nthis is not a message", None).is_err());
    }

    /// Top-level group `class` of a scene, by class name.
    fn scene_group<'a>(s: &'a Scene, class: &str) -> &'a [Element] {
        s.elements
            .iter()
            .find_map(|e| match e {
                Element::Group {
                    class: Some(c),
                    children,
                    ..
                } if c == class => Some(children.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    fn first_rect(e: &Element) -> Option<Rect> {
        match e {
            Element::Shape {
                geom: Geometry::Rect { rect, .. },
                ..
            } => Some(*rect),
            Element::Group { children, .. } => children.iter().find_map(first_rect),
            _ => None,
        }
    }

    fn lifeline_xs(s: &Scene) -> Vec<f64> {
        scene_group(s, "lifelines")
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Path(cmds),
                    ..
                } => match cmds.first() {
                    Some(PathCmd::MoveTo(p)) => Some(p.x),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn participant_boxes_follow_mermaid_margins() {
        let d = parse(
            "sequenceDiagram\nbox Aqua One\nparticipant A\nend\nbox Two\nparticipant B\nend\nparticipant C",
            None,
        )
        .unwrap();
        let p = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let cfg = SequenceConfig::default();
        let s = render(&d, &cfg, &p);
        let xs = lifeline_xs(&s);
        let boxes: Vec<Rect> = scene_group(&s, "boxes")
            .iter()
            .filter_map(first_rect)
            .collect();
        assert_eq!(boxes.len(), 2);
        // Box = actor (150) + boxTextMargin (5) + 2 * boxMargin (20) on each side.
        assert!(
            (boxes[0].x - (xs[0] - 100.0)).abs() < 1e-6,
            "{boxes:?} {xs:?}"
        );
        assert!((boxes[0].right() - (xs[0] + 100.0)).abs() < 1e-6);
        // Neighbouring boxes: actorMargin + boxMargin + 2 * boxTextMargin - 2 * 25 apart.
        assert!((boxes[1].x - boxes[0].right() - 20.0).abs() < 1e-6);
        // The participant after a box is pushed out by boxMargin + boxTextMargin.
        assert!((xs[2] - xs[1] - (150.0 + 50.0 + 15.0)).abs() < 1e-6);
        // Both boxes start `boxMargin / 2` above the band holding the titles.
        assert!((boxes[0].y - boxes[1].y).abs() < 1e-6);
        assert!((boxes[0].y - cfg.diagram_margin_y).abs() < 1e-6);
    }

    #[test]
    fn nested_rects_paint_inner_on_top() {
        let d = parse(
            "sequenceDiagram\nA->>B: x\nrect rgb(1, 2, 3)\nA->>B: y\nrect rgb(4, 5, 6)\nA->>B: z\nend\nend",
            None,
        )
        .unwrap();
        let p = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let s = render(&d, &SequenceConfig::default(), &p);
        let rects: Vec<(Rect, String)> = scene_group(&s, "rects")
            .iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    paint,
                    ..
                } => Some((*rect, paint.fill.0.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 2);
        // Outer first (painted below), inner inset by boxMargin on each side.
        assert_eq!(rects[0].1, "rgb(1, 2, 3)");
        assert!((rects[1].0.x - rects[0].0.x - 10.0).abs() < 1e-6);
        assert!(rects[1].0.y > rects[0].0.y && rects[1].0.bottom() < rects[0].0.bottom());
    }

    #[test]
    fn frame_spacing_follows_mermaid() {
        // loop title: boxMargin before the frame, boxMargin + boxTextMargin + max(text, 20) after.
        let d = parse("sequenceDiagram\nA->>B: x\nloop L\nA->>B: y\nend", None).unwrap();
        let d2 = parse("sequenceDiagram\nA->>B: x\nloop\nA->>B: y\nend", None).unwrap();
        let p = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let cfg = SequenceConfig::default();
        let h = |d: &SequenceDiagram| render(d, &cfg, &p).view_box.h;
        let title_h = TextBlock::label("[L]", cfg.message_font_size)
            .height
            .max(LABEL_BOX_H);
        assert!((h(&d) - h(&d2) - title_h).abs() < 1e-6);
    }

    #[test]
    fn long_messages_widen_gaps() {
        let short = parse("sequenceDiagram\nA->>B: hi", None).unwrap();
        let long = parse(
            "sequenceDiagram\nA->>B: a very very very very very very very long message text",
            None,
        )
        .unwrap();
        let p = crate::theme::palette(crate::theme::ThemeName::Default, &[]);
        let cfg = SequenceConfig::default();
        assert!(render(&long, &cfg, &p).view_box.w > render(&short, &cfg, &p).view_box.w);
    }

    #[test]
    fn actor_names_cannot_contain_arrow_characters() {
        assert!(parse("sequenceDiagram\nA-->>>B: bad", None).is_err());
        assert!(parse("sequenceDiagram\nA-->>B: ok", None).is_ok());
    }
}
