//! Sequence diagrams: parser, layout and renderer.

use crate::color;
use crate::config::{Look, SequenceConfig};
use crate::error::{Error, Result};
use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{label_lines, svg_line_box};
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
    /// sequenceDb addSignal(ACTIVE_END): deactivating a participant whose
    /// activations are all closed is an error.
    fn check_active(&self, a: usize, line: usize) -> Result<()> {
        let count: i64 = self
            .events
            .iter()
            .map(|e| match e {
                Event::Activate(x) if *x == a => 1,
                Event::Deactivate(x) if *x == a => -1,
                Event::Message(m) if m.activation == Some(true) && m.to == a => 1,
                Event::Message(m) if m.activation == Some(false) && m.from == a => -1,
                _ => 0,
            })
            .sum();
        if count < 1 {
            return Err(Error::parse(
                line,
                format!(
                    "Trying to inactivate an inactive participant ({})",
                    self.participants[a].id
                ),
            ));
        }
        Ok(())
    }

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
                let words: Vec<&str> = rest.split_whitespace().collect();
                let nums: Vec<u32> = words.iter().filter_map(|w| w.parse().ok()).collect();
                // sequenceDiagram.jison: `autonumber [off | NUM [NUM]]`.
                if !(rest == "off" || (words.len() <= 2 && nums.len() == words.len())) {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\nExpecting 'off' or up to two numbers after 'autonumber'"),
                    ));
                }
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
                b.check_active(a, line_no)?;
                b.events.push(Event::Deactivate(a));
            }
            "note" => {
                let (head, text) = rest.split_once(':').ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting ':' after note placement"))
                })?;
                let head = head.trim();
                let actors = ["left of", "right of", "over"]
                    .iter()
                    .find_map(|p| head.strip_prefix(p))
                    .unwrap_or(head);
                if actors.split(',').any(|a| a.trim().is_empty()) {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\nExpecting a participant for the note"),
                    ));
                }
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
                if m.activation == Some(false) {
                    b.check_active(m.from, line_no)?;
                }
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
    // The message text (`TXT`, starting at ':') is mandatory upstream.
    let (to, text) = rest.split_once(':').map(|(t, x)| (t.trim(), x.trim()))?;
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
// Layout + render: a port of mermaid's `sequenceRenderer.ts` / `svgDraw.js` /
// `actorBands.ts` (12.3.0) for the constructs this parser supports.
// ---------------------------------------------------------------------------

/// `activationWidth`.
const ACTIVATION_W: f64 = 10.0;
/// `labelBoxHeight` (sequence config default).
const LABEL_BOX_H: f64 = 20.0;
/// `labelBoxWidth` (sequence config default).
const LABEL_BOX_W: f64 = 50.0;
/// `bottomMarginAdj`.
const BOTTOM_MARGIN_ADJ: f64 = 1.0;
/// actorBands.ts: the glyph band (a 22 unit icon plus its underline), and the air
/// between glyph and label and between label and lifeline datum.
const GLYPH_BAND_H: f64 = 44.0;
const GLYPH_LABEL_GAP: f64 = 6.0;
const LABEL_LIFELINE_GAP: f64 = 6.0;
/// svgDraw.js: the stick figure spans `ACTOR_GLYPH_TOP..ACTOR_GLYPH_BOTTOM` below
/// its classic box top, its arms `ACTOR_TYPE_WIDTH` wide.
const ACTOR_GLYPH_TOP: f64 = -5.0;
const ACTOR_GLYPH_BOTTOM: f64 = 60.0;
const ACTOR_TYPE_W: f64 = 36.0;
/// The autonumber marker's circle radius, before the marker's `strokeWidth`
/// scaling (the stylesheet's bare `line { stroke-width: 2px }` doubles it).
const SEQ_NUM_R: f64 = 6.0;
/// Alphabetic-baseline offsets (in em) of `dominant-baseline: middle` (half the
/// x-height) and `central` (the em box centre).
const BASELINE_MIDDLE: f64 = 0.27;
const BASELINE_CENTRAL: f64 = 0.35;

/// JavaScript's `Math.round` (halves round up, also below zero).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `utils.calculateTextDimensions`: the widest line's rounded width and the sum
/// of each line's rounded bounding-box height.
fn text_dims(lines: &[String], font_size: f64) -> (f64, f64) {
    if lines.iter().all(|l| l.is_empty()) && lines.len() <= 1 {
        return (0.0, 0.0);
    }
    let w = lines
        .iter()
        .map(|l| js_round(crate::text::line_width(l, font_size, false)))
        .fold(0.0, f64::max);
    let h = lines.iter().map(|l| svg_line_box(font_size, l)).sum();
    (w, h)
}

/// One `<text>` line with `text-anchor: middle`, its attribute `y` turned into
/// the alphabetic baseline by `shift` em.
fn line_text(x: f64, y: f64, shift: f64, line: &str, font_size: f64, fill: &Color) -> Text {
    Text {
        x,
        y: y + shift * font_size,
        lines: vec![line.to_string()],
        line_height: font_size,
        font_size,
        bold: false,
        anchor: Anchor::Middle,
        fill: fill.clone(),
    }
}

/// svgDraw `drawText` with `valign: center` and a positive `textMargin`: one
/// `<text>` per line at `round(y + (2·above + margin) / 2)`, where `above` is the
/// height of the lines before it. `dy1em` adds the `dy="1em"` notes and messages
/// carry; `shift` is the line's dominant-baseline offset.
#[allow(clippy::too_many_arguments)]
fn draw_text_lines(
    out: &mut Vec<Element>,
    class: &str,
    x: f64,
    y: f64,
    margin: f64,
    lines: &[String],
    font_size: f64,
    fill: &Color,
    dy1em: bool,
    shift: f64,
) {
    let mut above = 0.0;
    for line in lines {
        let ty = js_round(y + (2.0 * above + margin) / 2.0) + if dy1em { font_size } else { 0.0 };
        out.push(Element::classed_text(
            line_text(x, ty, shift, line, font_size, fill),
            class,
        ));
        above += svg_line_box(font_size, line);
    }
}

/// `byTspan`: lines centred on `cy` (`dominant-baseline: central`), 1em apart.
fn tspan_lines(cx: f64, cy: f64, lines: &[String], font_size: f64, fill: &Color) -> Vec<Text> {
    let n = lines.len() as f64;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let dy = i as f64 * font_size - font_size * (n - 1.0) / 2.0;
            line_text(cx, cy + dy, BASELINE_CENTRAL, l, font_size, fill)
        })
        .collect()
}

/// A participant as the layout sees it (`actor.x`, `width`, `height`, `margin`).
struct ActorBox {
    x: f64,
    width: f64,
    height: f64,
    margin: Option<f64>,
    lines: Vec<String>,
    text_h: f64,
}

impl ActorBox {
    fn center(&self) -> f64 {
        self.x + self.width / 2.0
    }
}

/// One entry of mermaid's message list, the `+`/`-` shorthands expanded.
#[derive(Clone, Copy)]
enum Op<'a> {
    Msg(&'a Message),
    ActStart(usize),
    ActEnd(usize),
    Note(NotePlacement, &'a str),
    LoopStart(BlockKind, &'a str),
    Section(&'a str),
    LoopEnd(BlockKind),
    Autonumber(bool, Option<u32>, Option<u32>),
}

fn ops_of(d: &SequenceDiagram) -> Vec<Op<'_>> {
    let mut ops = Vec::new();
    let mut open = Vec::new();
    for ev in &d.events {
        match ev {
            Event::Message(m) => {
                ops.push(Op::Msg(m));
                match m.activation {
                    Some(true) => ops.push(Op::ActStart(m.to)),
                    Some(false) => ops.push(Op::ActEnd(m.from)),
                    None => {}
                }
            }
            Event::Note { placement, text } => ops.push(Op::Note(*placement, text)),
            Event::Activate(a) => ops.push(Op::ActStart(*a)),
            Event::Deactivate(a) => ops.push(Op::ActEnd(*a)),
            Event::BlockStart { kind, label } => {
                open.push(*kind);
                ops.push(Op::LoopStart(*kind, label));
            }
            Event::BlockSection { label } => ops.push(Op::Section(label)),
            Event::BlockEnd => ops.push(Op::LoopEnd(open.pop().unwrap_or(BlockKind::Loop))),
            Event::Autonumber { on, start, step } => ops.push(Op::Autonumber(*on, *start, *step)),
        }
    }
    ops
}

/// `buildMessageModel`.
#[derive(Clone)]
struct MsgModel {
    startx: f64,
    stopx: f64,
    width: f64,
    from_bounds: f64,
    to_bounds: f64,
}

/// `buildNoteModel`.
#[derive(Clone)]
struct NoteModel {
    startx: f64,
    width: f64,
    lines: Vec<String>,
}

/// An activation bar: `bounds.activations` entries.
struct Activation {
    actor: usize,
    startx: f64,
    stopx: f64,
    starty: f64,
    /// Index of its (initially empty) anchor group in the drawing.
    slot: usize,
}

/// A frame (`loop`, `alt`, ..., `rect`) being laid out: `bounds.sequenceItems`.
struct LoopModel {
    kind: BlockKind,
    startx: Option<f64>,
    starty: f64,
    stopx: Option<f64>,
    stopy: Option<f64>,
    title: Vec<String>,
    sections: Vec<(f64, Vec<String>)>,
    fill: Option<String>,
}

/// `bounds`: the running vertical position and the extents of the drawing, of
/// the open frames and of the open activations.
struct Bounds {
    startx: Option<f64>,
    starty: Option<f64>,
    stopx: Option<f64>,
    stopy: Option<f64>,
    vpos: f64,
    items: Vec<LoopModel>,
    acts: Vec<Activation>,
    box_margin: f64,
}

fn upd(slot: &mut Option<f64>, v: f64, f: fn(f64, f64) -> f64) {
    *slot = Some(slot.map_or(v, |o| f(v, o)));
}

impl Bounds {
    fn bump(&mut self, v: f64) {
        self.vpos += v;
        upd(&mut self.stopy, self.vpos, f64::max);
    }

    /// `bounds.insert`, with `updateBounds`' quirk: frame `i` of `n` open ones grows
    /// by `(n - i)·boxMargin`, activation `j` by `-j·boxMargin`.
    fn insert(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        let (sx, ex) = (x1.min(x2), x1.max(x2));
        let (sy, ey) = (y1.min(y2), y1.max(y2));
        upd(&mut self.startx, sx, f64::min);
        upd(&mut self.starty, sy, f64::min);
        upd(&mut self.stopx, ex, f64::max);
        upd(&mut self.stopy, ey, f64::max);
        let bm = self.box_margin;
        let len = self.items.len() as f64;
        let mut cnt = 0.0;
        for item in self.items.iter_mut() {
            cnt += 1.0;
            let n = len - cnt + 1.0;
            item.starty = item.starty.min(sy - n * bm);
            upd(&mut item.stopy, ey + n * bm, f64::max);
            upd(&mut self.startx, sx - n * bm, f64::min);
            upd(&mut self.stopx, ex + n * bm, f64::max);
            upd(&mut item.startx, sx - n * bm, f64::min);
            upd(&mut item.stopx, ex + n * bm, f64::max);
            upd(&mut self.starty, sy - n * bm, f64::min);
            upd(&mut self.stopy, ey + n * bm, f64::max);
        }
        for act in self.acts.iter_mut() {
            cnt += 1.0;
            let n = len - cnt + 1.0;
            act.starty = act.starty.min(sy - n * bm);
            upd(&mut self.startx, sx - n * bm, f64::min);
            upd(&mut self.stopx, ex + n * bm, f64::max);
        }
    }
}

/// `activationBounds`: the actor's centre ±1, widened by its open activations.
fn activation_bounds(a: usize, actors: &[ActorBox], acts: &[(usize, f64, f64)]) -> (f64, f64) {
    let c = actors[a].center();
    acts.iter()
        .filter(|x| x.0 == a)
        .fold((c - 1.0, c + 1.0), |(l, r), x| (l.min(x.1), r.max(x.2)))
}

/// Where a new activation of `a` starts horizontally (`newActivation`).
fn activation_x(a: usize, actors: &[ActorBox], stacked: usize) -> f64 {
    actors[a].center() + (stacked as f64 - 1.0) * ACTIVATION_W / 2.0
}

fn build_message_model(
    m: &Message,
    actors: &[ActorBox],
    acts: &[(usize, f64, f64)],
    cfg: &SequenceConfig,
) -> MsgModel {
    let (from_l, from_r) = activation_bounds(m.from, actors, acts);
    let (to_l, to_r) = activation_bounds(m.to, actors, acts);
    let right = from_l <= to_l;
    let mut startx = if right { from_r } else { from_l };
    let mut stopx = if right { to_l } else { to_r };
    let open = m.head == MsgHead::None;
    let adjust = |v: f64| if right { -v } else { v };
    if cfg.look == Look::Neo {
        if !open {
            stopx += adjust(3.0);
        }
        if m.bidirectional {
            startx -= adjust(3.0);
        }
    }
    let to_activation = (to_l - to_r).abs() > 2.0;
    if m.from == m.to {
        stopx = startx;
    } else {
        if m.activation == Some(true) && !to_activation {
            stopx += adjust(ACTIVATION_W / 2.0 - 1.0);
        }
        if !open {
            stopx += adjust(3.0);
        }
        if m.bidirectional {
            startx -= adjust(3.0);
        }
    }
    let lines = label_lines(&m.text);
    let (tw, _) = text_dims(&lines, cfg.message_font_size);
    let all = [from_l, from_r, to_l, to_r];
    MsgModel {
        startx,
        stopx,
        width: (tw + 2.0 * cfg.wrap_padding)
            .max((startx - stopx).abs() + 2.0 * cfg.wrap_padding)
            .max(cfg.width),
        from_bounds: all.iter().copied().fold(f64::INFINITY, f64::min),
        to_bounds: all.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

fn build_note_model(
    placement: NotePlacement,
    text: &str,
    actors: &[ActorBox],
    cfg: &SequenceConfig,
) -> NoteModel {
    let lines = label_lines(text);
    let (tw, _) = text_dims(&lines, cfg.note_font_size);
    let (from, to) = match placement {
        NotePlacement::LeftOf(a) | NotePlacement::RightOf(a) => (a, a),
        NotePlacement::Over(a, b) => (a, b),
    };
    let (fa, ta) = (&actors[from], &actors[to]);
    let need = tw + 2.0 * cfg.note_margin;
    let (startx, width) = match placement {
        NotePlacement::RightOf(_) => {
            let w = (fa.width / 2.0 + ta.width / 2.0).max(need);
            (fa.x + (fa.width + cfg.actor_margin) / 2.0, w)
        }
        NotePlacement::LeftOf(_) => {
            let w = (fa.width / 2.0 + ta.width / 2.0).max(need);
            (fa.x - w + (fa.width - cfg.actor_margin) / 2.0, w)
        }
        NotePlacement::Over(..) if from == to => {
            let w = fa.width.max(cfg.width).max(need);
            (fa.x + (fa.width - w) / 2.0, w)
        }
        NotePlacement::Over(..) => {
            let w = (fa.center() - ta.center()).abs() + cfg.actor_margin;
            let sx = if fa.x < ta.x {
                fa.center() - cfg.actor_margin / 2.0
            } else {
                ta.center() - cfg.actor_margin / 2.0
            };
            (sx, w)
        }
    };
    NoteModel {
        startx,
        width,
        lines,
    }
}

/// Render a sequence diagram.
pub fn render(d: &SequenceDiagram, cfg: &SequenceConfig, pal: &Palette) -> Scene {
    let neo = cfg.look == Look::Neo;
    let n = d.participants.len();
    let bm = cfg.box_margin;
    let ops = ops_of(d);

    // --- calculateActorMargins -------------------------------------------
    let mut actors: Vec<ActorBox> = d
        .participants
        .iter()
        .map(|p| {
            let lines = label_lines(&p.label);
            let (tw, th) = text_dims(&lines, cfg.actor_font_size);
            let height = if neo {
                (GLYPH_BAND_H + GLYPH_LABEL_GAP + th + LABEL_LIFELINE_GAP).max(cfg.height)
            } else {
                cfg.height
            };
            ActorBox {
                x: 0.0,
                width: cfg.width.max(tw + 2.0 * cfg.wrap_padding),
                height,
                margin: None,
                lines,
                text_h: th,
            }
        })
        .collect();
    let row_h = actors.iter().map(|a| a.height).fold(cfg.height, f64::max);

    // getMaxMessageWidthPerActor
    let mut max_w: Vec<Option<f64>> = vec![None; n];
    let mut widen = |i: usize, w: f64| max_w[i] = Some(max_w[i].map_or(w, |o| o.max(w)));
    for op in &ops {
        let (from, to, w, note) = match *op {
            Op::Msg(m) => {
                let (tw, _) = text_dims(&label_lines(&m.text), cfg.message_font_size);
                (m.from, m.to, tw + 2.0 * cfg.wrap_padding, None)
            }
            Op::Note(p, text) => {
                let (tw, _) = text_dims(&label_lines(text), cfg.note_font_size);
                let (f, t) = match p {
                    NotePlacement::LeftOf(a) | NotePlacement::RightOf(a) => (a, a),
                    NotePlacement::Over(a, b) => (a, b),
                };
                (f, t, tw + 2.0 * cfg.wrap_padding, Some(p))
            }
            _ => continue,
        };
        let prev = to.checked_sub(1);
        let next = (to + 1 < n).then_some(to + 1);
        match note {
            Some(NotePlacement::LeftOf(_)) if prev.is_none() => continue,
            Some(NotePlacement::RightOf(_)) if next.is_none() => continue,
            _ => {}
        }
        if note.is_none() && Some(from) == next {
            widen(to, w);
        } else if note.is_none() && Some(from) == prev {
            widen(from, w);
        } else if note.is_none() && from == to {
            widen(from, w / 2.0);
        } else if let Some(NotePlacement::RightOf(_)) = note {
            widen(from, w);
        } else if let Some(NotePlacement::LeftOf(_)) = note {
            if let Some(p) = prev {
                widen(p, w);
            }
        } else if let Some(NotePlacement::Over(..)) = note {
            if let Some(p) = prev {
                widen(p, w / 2.0);
            }
            if next.is_some() {
                widen(from, w / 2.0);
            }
        }
    }
    for i in 0..n {
        let Some(w) = max_w[i] else { continue };
        let next_half = if i + 1 < n {
            actors[i + 1].width / 2.0
        } else {
            0.0
        };
        actors[i].margin =
            Some((w + cfg.actor_margin - actors[i].width / 2.0 - next_half).max(cfg.actor_margin));
    }

    // Boxes: an inner margin (`boxTextMargin`, more for a wide title).
    let box_of: Vec<Option<usize>> = (0..n)
        .map(|i| d.boxes.iter().position(|b| b.members.contains(&i)))
        .collect();
    let box_lines: Vec<Vec<String>> = d.boxes.iter().map(|b| label_lines(&b.label)).collect();
    let box_text_max_h = box_lines
        .iter()
        .map(|l| text_dims(l, cfg.message_font_size).1)
        .fold(0.0, f64::max);
    let box_margins: Vec<f64> = d
        .boxes
        .iter()
        .zip(&box_lines)
        .map(|(b, lines)| {
            let total = b
                .members
                .iter()
                .map(|&m| actors[m].width + actors[m].margin.unwrap_or(0.0))
                .sum::<f64>()
                + 8.0 * bm
                - 2.0 * cfg.box_text_margin;
            let (tw, _) = text_dims(lines, cfg.message_font_size);
            let min_w = total.max(tw + 2.0 * cfg.wrap_padding);
            cfg.box_text_margin
                + if total < min_w {
                    (min_w - total) / 2.0
                } else {
                    0.0
                }
        })
        .collect();

    let mut b = Bounds {
        startx: None,
        starty: None,
        stopx: None,
        stopy: None,
        vpos: 0.0,
        items: Vec::new(),
        acts: Vec::new(),
        box_margin: bm,
    };
    if !d.boxes.is_empty() {
        b.bump(bm);
        if d.boxes.iter().any(|x| !x.label.is_empty()) {
            b.bump(box_text_max_h);
        }
    }

    // --- addActorRenderingData ------------------------------------------
    let mut box_x = vec![0.0; d.boxes.len()];
    let mut box_w = vec![0.0; d.boxes.len()];
    let (mut prev_width, mut prev_margin) = (0.0, 0.0);
    let mut prev_box: Option<usize> = None;
    let actor_top = b.vpos;
    for i in 0..n {
        let bx = box_of[i];
        if let Some(p) = prev_box.filter(|&p| Some(p) != bx) {
            prev_margin += bm + box_margins[p];
        }
        if let Some(c) = bx.filter(|&c| Some(c) != prev_box) {
            box_x[c] = prev_width + prev_margin;
            prev_margin += box_margins[c];
        }
        actors[i].height = row_h;
        let margin = actors[i].margin.unwrap_or(cfg.actor_margin);
        actors[i].x = prev_width + prev_margin;
        b.insert(
            actors[i].x,
            0.0,
            actors[i].x + actors[i].width,
            actors[i].height,
        );
        prev_width += actors[i].width + prev_margin;
        if let Some(c) = bx {
            box_w[c] = prev_width + box_margins[c] - box_x[c];
        }
        prev_margin = margin;
        prev_box = bx;
    }
    if n > 0 {
        b.bump(row_h);
    }

    // --- calculateLoopBounds: message models and frame title widths -------
    let mut msg_models: Vec<Option<MsgModel>> = vec![None; ops.len()];
    let mut note_models: Vec<Option<NoteModel>> = vec![None; ops.len()];
    let mut loop_widths: Vec<Option<f64>> = vec![None; ops.len()];
    {
        struct LoopW {
            from: f64,
            to: f64,
            width: f64,
            ops: Vec<usize>,
        }
        let mut all: Vec<LoopW> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut acts: Vec<(usize, f64, f64)> = Vec::new();
        for (oi, op) in ops.iter().enumerate() {
            match *op {
                Op::LoopStart(k, _) if k != BlockKind::Rect => {
                    all.push(LoopW {
                        from: f64::MAX,
                        to: f64::MIN,
                        width: 0.0,
                        ops: vec![oi],
                    });
                    stack.push(all.len() - 1);
                }
                Op::Section(label) if !label.is_empty() => {
                    if let Some(&top) = stack.last() {
                        all[top].ops.push(oi);
                    }
                }
                Op::LoopEnd(k) if k != BlockKind::Rect => {
                    stack.pop();
                }
                Op::ActStart(a) => {
                    let stacked = acts.iter().filter(|x| x.0 == a).count();
                    let x = activation_x(a, &actors, stacked);
                    acts.push((a, x, x + ACTIVATION_W));
                }
                Op::ActEnd(a) => {
                    if let Some(p) = acts.iter().rposition(|x| x.0 == a) {
                        acts.remove(p);
                    }
                }
                _ => {}
            }
            match *op {
                Op::Note(p, text) => {
                    let nm = build_note_model(p, text, &actors, cfg);
                    for &s in &stack {
                        let c = &mut all[s];
                        c.from = c.from.min(nm.startx);
                        c.to = c.to.max(nm.startx + nm.width);
                        c.width = c.width.max((c.from - c.to).abs()) - LABEL_BOX_W;
                    }
                    note_models[oi] = Some(nm);
                }
                Op::Msg(m) => {
                    let mm = build_message_model(m, &actors, &acts, cfg);
                    if mm.startx != 0.0 && mm.stopx != 0.0 {
                        for &s in &stack {
                            let c = &mut all[s];
                            if mm.startx == mm.stopx {
                                let (fa, ta) = (&actors[m.from], &actors[m.to]);
                                c.from = (fa.x - mm.width / 2.0)
                                    .min(fa.x - fa.width / 2.0)
                                    .min(c.from);
                                c.to = (ta.x + mm.width / 2.0).max(ta.x + fa.width / 2.0).max(c.to);
                                c.width = c.width.max((c.to - c.from).abs()) - LABEL_BOX_W;
                            } else {
                                c.from = mm.startx.min(c.from);
                                c.to = mm.stopx.max(c.to);
                                c.width = c.width.max(mm.width) - LABEL_BOX_W;
                            }
                        }
                    }
                    msg_models[oi] = Some(mm);
                }
                _ => {}
            }
        }
        for l in &all {
            for &oi in &l.ops {
                loop_widths[oi] = Some(l.width);
            }
        }
    }

    // --- The main pass: vertical layout, drawing in mermaid's order. ------
    let mf = cfg.message_font_size;
    let mut lowered: Vec<Element> = Vec::new(); // in lowering order
    let mut body: Vec<Element> = Vec::new();
    let mut backgrounds: Vec<LoopModel> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut seq_index = 1.0f64;
    let mut seq_step = 1.0f64;
    let mut numbering = cfg.show_sequence_numbers;
    let frame_title = |label: &str, oi: usize| -> Vec<String> {
        let Some(w) = loop_widths[oi].filter(|_| !label.is_empty()) else {
            return Vec::new();
        };
        let lines = label_lines(&format!("[{label}]"));
        crate::text::wrap_lines(
            &lines,
            w - 2.0 * cfg.wrap_padding,
            mf,
            false,
            crate::text::BreakRule::Spaces,
        )
    };
    for (oi, op) in ops.iter().enumerate() {
        match *op {
            Op::Autonumber(on, start, step) => {
                if let Some(s) = start {
                    seq_index = s as f64;
                }
                if let Some(s) = step {
                    seq_step = s as f64;
                }
                numbering = on;
            }
            Op::ActStart(a) => {
                let stacked = b.acts.iter().filter(|x| x.actor == a).count();
                let x = activation_x(a, &actors, stacked);
                body.push(Element::group("activation", Vec::new()));
                b.acts.push(Activation {
                    actor: a,
                    startx: x,
                    stopx: x + ACTIVATION_W,
                    starty: b.vpos + 2.0,
                    slot: body.len() - 1,
                });
            }
            Op::ActEnd(a) => {
                let Some(p) = b.acts.iter().rposition(|x| x.actor == a) else {
                    continue;
                };
                let act = b.acts.remove(p);
                let mut vpos = b.vpos;
                let mut starty = act.starty;
                if starty + 18.0 > vpos {
                    starty = vpos - 6.0;
                    vpos += 12.0;
                }
                let depth = b.acts.iter().filter(|x| x.actor == a).count();
                let r = Rect::new(act.startx, starty, act.stopx - act.startx, vpos - starty);
                // Colour themes paint an activation in its actor's slot, falling back
                // to `mainBkg` for the fill (svgDraw.js `drawActivation`).
                let paint = match actor_slot(pal, a) {
                    Some(sc) => {
                        Paint::both(sc.fill.as_ref().unwrap_or(&pal.main_bkg), &sc.border, 1.0)
                    }
                    None => Paint::both(&pal.activation_bkg, &pal.activation_border, 1.0),
                };
                body[act.slot] = Element::group(
                    "activation",
                    vec![Element::classed_shape(
                        Geometry::Rect { rect: r, rx: 0.0 },
                        paint,
                        format!("activation{}", depth % 3),
                    )],
                );
                b.insert(act.startx, vpos - 10.0, act.stopx, vpos);
            }
            Op::Note(..) => {
                let Some(nm) = note_models[oi].clone() else {
                    continue;
                };
                b.bump(bm);
                let starty = b.vpos;
                let text_h: f64 = nm
                    .lines
                    .iter()
                    .map(|l| svg_line_box(cfg.note_font_size, l))
                    .sum();
                let h = js_round(text_h) + 2.0 * cfg.note_margin;
                let r = Rect::new(nm.startx, starty, nm.width, h);
                let mut els = vec![Element::classed_shape(
                    Geometry::Rect { rect: r, rx: 0.0 },
                    Paint::both(&pal.note_bkg, &pal.note_border, 1.0),
                    "note",
                )];
                draw_text_lines(
                    &mut els,
                    "noteText",
                    js_round(nm.startx + nm.width / 2.0),
                    starty,
                    cfg.note_margin,
                    &nm.lines,
                    cfg.note_font_size,
                    &pal.note_text,
                    true,
                    BASELINE_MIDDLE,
                );
                body.push(Element::group("note", els));
                b.bump(h);
                b.insert(nm.startx, starty, nm.startx + nm.width, starty + h);
            }
            Op::LoopStart(kind, label) => {
                b.bump(bm);
                if kind == BlockKind::Rect {
                    b.items.push(LoopModel {
                        kind,
                        startx: None,
                        starty: b.vpos,
                        stopx: None,
                        stopy: None,
                        title: Vec::new(),
                        sections: Vec::new(),
                        fill: Some(label.to_string()),
                    });
                    b.bump(bm);
                    continue;
                }
                let title = frame_title(label, oi);
                let post = bm
                    + cfg.box_text_margin
                    + if title.is_empty() {
                        0.0
                    } else {
                        text_dims(&title, mf).1.max(LABEL_BOX_H)
                    };
                b.items.push(LoopModel {
                    kind,
                    startx: None,
                    starty: b.vpos,
                    stopx: None,
                    stopy: None,
                    title,
                    sections: Vec::new(),
                    fill: None,
                });
                b.bump(post);
            }
            Op::Section(label) => {
                b.bump(bm + cfg.box_text_margin);
                let title = frame_title(label, oi);
                let post = bm
                    + if title.is_empty() {
                        0.0
                    } else {
                        text_dims(&title, mf).1.max(LABEL_BOX_H)
                    };
                let y = b.vpos;
                if let Some(l) = b.items.last_mut() {
                    l.sections.push((y, title));
                }
                b.bump(post);
            }
            Op::LoopEnd(_) => {
                let Some(l) = b.items.pop() else { continue };
                let stopy = l.stopy.unwrap_or(b.vpos);
                if l.kind == BlockKind::Rect {
                    backgrounds.push(l);
                } else {
                    body.push(frame_elements(&l, pal, cfg));
                }
                let dv = stopy - b.vpos;
                b.bump(dv);
            }
            Op::Msg(m) => {
                let Some(model) = msg_models[oi].clone() else {
                    continue;
                };
                // boundMessage
                let starty = b.vpos;
                b.bump(10.0);
                let lines = label_lines(&m.text);
                let (tw, th) = text_dims(&lines, mf);
                let line_h = th / lines.len().max(1) as f64;
                b.bump(line_h);
                let mut total = th - 10.0;
                let line_y;
                if model.startx == model.stopx {
                    total += bm;
                    line_y = b.vpos + total;
                    total += 30.0;
                    let dx = (tw / 2.0).max(cfg.width / 2.0);
                    b.insert(
                        model.startx - dx,
                        b.vpos - 10.0 + total,
                        model.stopx + dx,
                        b.vpos + 30.0 + total,
                    );
                } else {
                    total += bm;
                    line_y = b.vpos + total;
                    b.insert(model.startx, line_y - 10.0, model.stopx, line_y);
                }
                b.bump(total);
                let stopy = starty + line_h + total;
                b.insert(model.from_bounds, starty, model.to_bounds, stopy);
                pending.push(Pending {
                    model,
                    msg: m.clone(),
                    line_y,
                    starty,
                    number: numbering.then_some(seq_index),
                });
                seq_index = ((seq_index + seq_step) * 100.0).round() / 100.0;
            }
        }
    }

    // --- Actors, messages, footer -----------------------------------------
    let shadow = neo.then(|| {
        // insertDropShadow: a hard 4px offset at 6%, black only for redux / redux-color.
        let black = pal
            .drop_shadow
            .as_ref()
            .is_some_and(|s| s.dx == 4.0 && s.color.css().eq_ignore_ascii_case("#000000"));
        Color::new(if black { "#000000" } else { "#FFFFFF" })
    });
    let footer_y = b.vpos + 2.0 * bm;
    let lifeline_end = if cfg.mirror_actors { footer_y } else { 2000.0 };
    for (i, p) in d.participants.iter().enumerate() {
        let (lowered_el, figure) = actor_elements(
            p,
            i,
            &actors[i],
            actor_top,
            false,
            Some(lifeline_end),
            shadow.as_ref(),
            pal,
            cfg,
        );
        lowered.push(lowered_el);
        body.extend(figure);
    }
    for pm in &pending {
        body.push(message_elements(pm, pal, cfg));
    }
    if cfg.mirror_actors {
        b.bump(2.0 * bm);
        for (i, p) in d.participants.iter().enumerate() {
            let (lowered_el, figure) = actor_elements(
                p,
                i,
                &actors[i],
                footer_y,
                true,
                None,
                shadow.as_ref(),
                pal,
                cfg,
            );
            lowered.push(lowered_el);
            body.extend(figure);
        }
        if n > 0 {
            b.bump(row_h + bm);
        }
    }
    for l in &backgrounds {
        let (Some(sx), Some(ex)) = (l.startx, l.stopx) else {
            continue;
        };
        let fill = l
            .fill
            .as_deref()
            .filter(|f| !f.is_empty())
            .map_or_else(|| pal.actor_bkg.clone(), Color::new);
        let stopy = l.stopy.unwrap_or(l.starty);
        lowered.push(Element::classed_shape(
            Geometry::Rect {
                rect: Rect::new(sx, l.starty, ex - sx, stopy - l.starty),
                rx: 0.0,
            },
            Paint::fill(&fill),
            "rect",
        ));
    }
    for (bi, bx) in d.boxes.iter().enumerate() {
        if bx.members.is_empty() {
            continue;
        }
        let height = b.vpos;
        b.insert(box_x[bi], 0.0, box_x[bi] + box_w[bi], height);
        let pad = 2.0 * bm;
        let rect = Rect::new(
            box_x[bi] - pad,
            -pad * 0.25,
            box_w[bi] + 2.0 * pad,
            height + pad * 0.75,
        );
        let fill = Color::new(bx.color.clone().unwrap_or_else(|| "none".into()));
        // `g rect.rect { stroke: nodeBorder }` outranks the box's own stroke.
        let mut els = vec![Element::classed_shape(
            Geometry::Rect { rect, rx: 0.0 },
            Paint::both(&fill, &pal.node_border, 1.0),
            "rect",
        )];
        let cy = cfg.box_text_margin + box_text_max_h / 2.0;
        for t in tspan_lines(
            box_x[bi] + box_w[bi] / 2.0,
            cy,
            &box_lines[bi],
            mf,
            &pal.text_color,
        ) {
            els.push(Element::text(t));
        }
        lowered.push(Element::group("box", els));
    }
    if !d.boxes.is_empty() {
        b.bump(bm);
    }

    let (sx, sy) = (b.startx.unwrap_or(0.0), b.starty.unwrap_or(0.0));
    let (ex, ey) = (b.stopx.unwrap_or(0.0), b.stopy.unwrap_or(0.0));
    let mut height = ey - sy + 2.0 * cfg.diagram_margin_y;
    if cfg.mirror_actors {
        height = height - bm + BOTTOM_MARGIN_ADJ;
    }
    let width = ex - sx + 2.0 * cfg.diagram_margin_x;
    let title = d.title.as_ref().filter(|t| !t.is_empty());
    let title_band = if title.is_some() { 40.0 } else { 0.0 };
    let neo_band = if neo && n > 0 { 30.0 } else { 0.0 };
    let view_box = Rect::new(
        sx - cfg.diagram_margin_x,
        -(cfg.diagram_margin_y + title_band),
        width,
        height + title_band + neo_band,
    );

    let mut title_el = Vec::new();
    if let Some(t) = title {
        // A plain `<text>` (the diagram's font, start-anchored) at
        // x = width / 2 - 2·diagramMarginX and y = -25.
        title_el.push(Element::text(Text {
            x: (ex - sx) / 2.0 - 2.0 * cfg.diagram_margin_x,
            y: -25.0,
            lines: vec![t.clone()],
            line_height: pal.font_size * 1.5,
            font_size: pal.font_size,
            bold: false,
            anchor: Anchor::Start,
            fill: pal.text_color.clone(),
        }));
    }

    lowered.reverse();
    let elements = vec![
        Element::group("lowered", lowered),
        Element::group("body", body),
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

/// A message laid out by `boundMessage`, drawn after the top actors.
struct Pending {
    model: MsgModel,
    msg: Message,
    line_y: f64,
    starty: f64,
    number: Option<f64>,
}

/// A marker path in its own units placed at `at`, pointing along `dir`, with
/// `refX/refY` at `at` and the marker's units scaled by `scale`.
fn marker_points(
    pts: &[(f64, f64)],
    refp: (f64, f64),
    scale: f64,
    at: Point,
    dir: Point,
) -> Vec<Point> {
    let perp = Point::new(-dir.y, dir.x);
    pts.iter()
        .map(|&(mx, my)| {
            at.plus(dir.scale((mx - refp.0) * scale))
                .plus(perp.scale((my - refp.1) * scale))
        })
        .collect()
}

/// The `arrowhead` marker (`M -1 0 L 10 5 L 0 10 z`, `refX` 7.9, user space).
fn arrowhead(at: Point, dir: Point, color: &Color) -> Element {
    let pts = marker_points(
        &[(-1.0, 0.0), (10.0, 5.0), (0.0, 10.0)],
        (7.9, 5.0),
        1.0,
        at,
        dir,
    );
    Element::classed_shape(
        Geometry::Path(geom::polygon(&pts)),
        Paint::both(color, color, 1.0),
        "arrowhead",
    )
}

/// JavaScript's number-to-string for the autonumber values (rounded to 0.01).
fn js_number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// `drawMessage`: text, line (or self-loop), arrow markers and the autonumber.
fn message_elements(pm: &Pending, pal: &Palette, cfg: &SequenceConfig) -> Element {
    let (m, model) = (&pm.msg, &pm.model);
    let (sx, ex, ly) = (model.startx, model.stopx, pm.line_y);
    let mf = cfg.message_font_size;
    let mut els = Vec::new();
    draw_text_lines(
        &mut els,
        "messageText",
        js_round(sx.min(ex) + (ex - sx).abs() / 2.0),
        pm.starty + 10.0,
        cfg.wrap_padding,
        &label_lines(&m.text),
        mf,
        &pal.signal_text,
        true,
        BASELINE_MIDDLE,
    );
    let numbered = pm.number.is_some();
    let paint = {
        let p = Paint::stroke(&pal.signal_color, 1.5);
        if m.dotted {
            p.dashed(vec![3.0, 3.0])
        } else {
            p
        }
    };
    let class = if m.dotted {
        "messageLine1"
    } else {
        "messageLine0"
    };
    // (point, direction) of the end marker and of the start marker.
    let (end, start) = if sx == ex {
        let lsx = sx
            + if numbered && m.bidirectional {
                10.0
            } else {
                0.0
            };
        els.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(lsx, ly)),
                PathCmd::CubicTo(
                    Point::new(lsx + 60.0, ly - 10.0),
                    Point::new(sx + 60.0, ly + 30.0),
                    Point::new(sx, ly + 20.0),
                ),
            ]),
            paint,
            class,
        ));
        let unit = |x: f64, y: f64| {
            let l = x.hypot(y);
            Point::new(x / l, y / l)
        };
        (
            (Point::new(sx, ly + 20.0), unit(-60.0, -10.0)),
            (Point::new(lsx, ly), unit(-60.0, 10.0)),
        )
    } else {
        let x1 = if !numbered {
            sx
        } else if m.bidirectional {
            if sx < ex {
                sx + 2.0 * SEQ_NUM_R
            } else {
                sx - SEQ_NUM_R
            }
        } else {
            sx + SEQ_NUM_R
        };
        els.push(Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(x1, ly)),
                PathCmd::LineTo(Point::new(ex, ly)),
            ]),
            paint,
            class,
        ));
        let dir = Point::new(if ex >= x1 { 1.0 } else { -1.0 }, 0.0);
        (
            (Point::new(ex, ly), dir),
            (Point::new(x1, ly), dir.scale(-1.0)),
        )
    };
    match m.head {
        MsgHead::None => {}
        MsgHead::Filled => {
            els.push(arrowhead(end.0, end.1, &pal.signal_color));
            if m.bidirectional {
                els.push(arrowhead(start.0, start.1, &pal.signal_color));
            }
        }
        MsgHead::Cross => {
            // `crosshead`: `M 1,2 L 6,7 M 6,2 L 1,7` at 1pt, scaled by the 1.5px line.
            let p = marker_points(
                &[(1.0, 2.0), (6.0, 7.0), (6.0, 2.0), (1.0, 7.0)],
                (4.0, 4.5),
                1.5,
                end.0,
                end.1,
            );
            els.push(Element::path(
                vec![
                    PathCmd::MoveTo(p[0]),
                    PathCmd::LineTo(p[1]),
                    PathCmd::MoveTo(p[2]),
                    PathCmd::LineTo(p[3]),
                ],
                Paint::stroke(&pal.signal_color, 2.0),
            ));
        }
        MsgHead::Async => {
            // `filled-head`: unstyled, so it takes the root's text-colour fill.
            let p = marker_points(
                &[(18.0, 7.0), (9.0, 13.0), (14.0, 7.0), (9.0, 1.0)],
                (15.5, 7.0),
                1.5,
                end.0,
                end.1,
            );
            els.push(Element::path(
                geom::polygon(&p),
                Paint::fill(&pal.text_color),
            ));
        }
    }
    if let Some(num) = pm.number {
        // Self messages and left-to-right ones number at the source.
        let ax = if sx <= ex {
            model.from_bounds + 1.0
        } else {
            model.to_bounds - 1.0
        };
        els.push(Element::classed_shape(
            Geometry::Circle {
                c: Point::new(ax, ly),
                r: 2.0 * SEQ_NUM_R,
            },
            Paint::fill(&pal.signal_color),
            "sequenceNumber",
        ));
        let label = js_number(num);
        let digits = label.len();
        let size = if digits > 5 {
            7.0
        } else if digits > 3 {
            9.0
        } else {
            12.0
        };
        els.push(Element::classed_text(
            Text {
                x: ax,
                y: ly + 4.0,
                lines: vec![label],
                line_height: size,
                font_size: size,
                bold: false,
                anchor: Anchor::Middle,
                fill: pal.sequence_number.clone(),
            },
            "sequenceNumber",
        ));
    }
    Element::group("message", els)
}

/// svgDraw `drawLoop`: the dashed frame, its section dividers, the tag (taller in
/// the neo look) and the bracketed titles.
fn frame_elements(l: &LoopModel, pal: &Palette, cfg: &SequenceConfig) -> Element {
    let (Some(sx), Some(ex)) = (l.startx, l.stopx) else {
        return Element::group("frame", Vec::new());
    };
    let (sy, ey) = (l.starty, l.stopy.unwrap_or(l.starty));
    let mf = cfg.message_font_size;
    let line = Paint::stroke(&pal.label_box_border, 2.0).dashed(vec![2.0, 2.0]);
    let seg = |a: (f64, f64), b: (f64, f64), p: &Paint| {
        Element::classed_shape(
            Geometry::Path(vec![
                PathCmd::MoveTo(Point::new(a.0, a.1)),
                PathCmd::LineTo(Point::new(b.0, b.1)),
            ]),
            p.clone(),
            "loopLine",
        )
    };
    let mut els = vec![
        seg((sx, sy), (ex, sy), &line),
        seg((ex, sy), (ex, ey), &line),
        seg((sx, ey), (ex, ey), &line),
        seg((sx, sy), (sx, ey), &line),
    ];
    let section_line = Paint::stroke(&pal.label_box_border, 2.0).dashed(vec![3.0, 3.0]);
    for (y, _) in &l.sections {
        els.push(seg((sx, *y), (ex, *y), &section_line));
    }
    let tag_h = LABEL_BOX_H + if cfg.look == Look::Neo { 15.0 } else { 0.0 };
    let cut = 7.0;
    els.push(Element::classed_shape(
        Geometry::Path(geom::polygon(&[
            Point::new(sx, sy),
            Point::new(sx + LABEL_BOX_W, sy),
            Point::new(sx + LABEL_BOX_W, sy + tag_h - cut),
            Point::new(sx + LABEL_BOX_W - cut * 1.2, sy + tag_h),
            Point::new(sx, sy + tag_h),
        ])),
        Paint::both(&pal.label_box_bkg, &pal.label_box_border, 1.0),
        "labelBox",
    ));
    draw_text_lines(
        &mut els,
        "labelText",
        js_round(sx + LABEL_BOX_W / 2.0),
        sy + tag_h / 2.0,
        cfg.box_text_margin,
        &[l.kind.word().to_string()],
        mf,
        &pal.label_text,
        false,
        BASELINE_MIDDLE,
    );
    let top = cfg.box_margin + cfg.box_text_margin;
    draw_text_lines(
        &mut els,
        "loopText",
        sx + LABEL_BOX_W / 2.0 + (ex - sx) / 2.0,
        sy + top,
        cfg.box_text_margin,
        &l.title,
        mf,
        &pal.loop_text,
        false,
        0.0,
    );
    for (y, title) in &l.sections {
        draw_text_lines(
            &mut els,
            "sectionTitle",
            sx + (ex - sx) / 2.0,
            y + top,
            cfg.box_text_margin,
            title,
            mf,
            &pal.loop_text,
            false,
            0.0,
        );
    }
    Element::group("frame", els)
}

/// Palette slot of the `index`-th participant (svgDraw.js `paletteColor(palette,
/// actorIndexMap.get(name))`, the index being the participant's declaration order).
fn actor_slot(pal: &Palette, index: usize) -> Option<SlotColors> {
    Some(pal.color_slots.as_ref()?.slot(ColorSlot(index)))
}

/// svgDraw `drawActor` for one participant at `y` (`starty` or, in the footer,
/// `stopy`). Returns what mermaid lowers beneath everything (the participant box
/// with its lifeline, or a stick figure's lifeline) and what it draws in place
/// (the stick figure).
#[allow(clippy::too_many_arguments)]
fn actor_elements(
    p: &Participant,
    index: usize,
    a: &ActorBox,
    y: f64,
    footer: bool,
    lifeline_end: Option<f64>,
    shadow: Option<&Color>,
    pal: &Palette,
    cfg: &SequenceConfig,
) -> (Element, Vec<Element>) {
    let neo = cfg.look == Look::Neo;
    // `.actor { stroke-width: strokeWidth ?? 1 }`; an empty background palette leaves the
    // stylesheet's `actorBkg` in charge (d3 removes the inline fill).
    let slot = actor_slot(pal, index);
    let actor_border = slot.as_ref().map_or(&pal.actor_border, |s| &s.border);
    let actor_bkg = slot
        .as_ref()
        .and_then(|s| s.fill.as_ref())
        .unwrap_or(&pal.actor_bkg);
    let c = a.center();
    let fs = cfg.actor_font_size;
    let lifeline = |y1: f64| {
        lifeline_end.map(|y2| {
            // `stroke-width="0.5px"`, but the stylesheet's `.actor-man circle, line`
            // (a bare `line`: every line) makes it 2px.
            Element::classed_shape(
                Geometry::Path(vec![
                    PathCmd::MoveTo(Point::new(c, y1)),
                    PathCmd::LineTo(Point::new(c, y2)),
                ]),
                Paint::stroke(&pal.actor_line, 2.0),
                "actor-line",
            )
        })
    };
    match p.kind {
        ParticipantKind::Participant => {
            let mut els: Vec<Element> = lifeline(y + a.height).into_iter().collect();
            let rx = if neo { 6.0 } else { 3.0 };
            let r = Rect::new(a.x, y, a.width, a.height);
            if let Some(sh) = shadow {
                els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::new(r.x + 4.0, r.y + 4.0, r.w, r.h),
                        rx,
                    },
                    Paint::fill(sh).with_opacity(0.06),
                    "drop-shadow",
                ));
            }
            els.push(Element::classed_shape(
                Geometry::Rect { rect: r, rx },
                Paint::both(actor_bkg, actor_border, pal.stroke_width.unwrap_or(1.0)),
                "actor",
            ));
            for t in tspan_lines(c, y + a.height / 2.0, &a.lines, fs, &pal.actor_text) {
                els.push(Element::classed_text(t, "actor"));
            }
            (Element::group("actor", els), Vec::new())
        }
        ParticipantKind::Actor => {
            // actorBands.ts: under neo the figure fills the glyph band above the
            // label, both anchored at the lifeline datum; classic draws it at fixed
            // offsets from the row top.
            let (lifeline_y, glyph_bottom, label_y) = if !neo {
                (
                    y + 80.0,
                    0.0,
                    y + 35.0 + (ACTOR_GLYPH_BOTTOM - ACTOR_GLYPH_TOP) / 2.0,
                )
            } else if footer {
                let top = y + LABEL_LIFELINE_GAP;
                let bottom = top + GLYPH_BAND_H;
                (0.0, bottom, bottom + GLYPH_LABEL_GAP + a.text_h / 2.0)
            } else {
                let datum = y + a.height;
                (
                    datum,
                    datum - LABEL_LIFELINE_GAP - a.text_h - GLYPH_LABEL_GAP,
                    datum - LABEL_LIFELINE_GAP - a.text_h / 2.0,
                )
            };
            let scale = if neo {
                GLYPH_BAND_H / (ACTOR_GLYPH_BOTTOM - ACTOR_GLYPH_TOP)
            } else {
                1.0
            };
            let gy = |o: f64| {
                if neo {
                    glyph_bottom - (ACTOR_GLYPH_BOTTOM - o) * scale
                } else {
                    y + o
                }
            };
            let gx = |o: f64| c + o * scale;
            let line = Paint::stroke(actor_border, 2.0);
            let half = ACTOR_TYPE_W / 2.0;
            let mut els = vec![
                Element::line(
                    Point::new(c, gy(25.0)),
                    Point::new(c, gy(45.0)),
                    line.clone(),
                ),
                Element::line(
                    Point::new(gx(-half), gy(33.0)),
                    Point::new(gx(half), gy(33.0)),
                    line.clone(),
                ),
                Element::line(
                    Point::new(gx(-half), gy(60.0)),
                    Point::new(c, gy(45.0)),
                    line.clone(),
                ),
                Element::line(
                    Point::new(c, gy(45.0)),
                    Point::new(gx(half - 2.0), gy(60.0)),
                    line,
                ),
                // `.actor-man circle { fill: actorBkg }` outranks the group's colours.
                Element::shape(
                    Geometry::Circle {
                        c: Point::new(c, gy(10.0)),
                        r: 15.0 * scale,
                    },
                    Paint::both(&pal.actor_bkg, actor_border, 2.0),
                ),
            ];
            for t in tspan_lines(c, label_y, &a.lines, fs, &pal.actor_text) {
                els.push(Element::classed_text(t, "actor"));
            }
            let lowered = Element::group(
                "actor-lifeline",
                if footer {
                    Vec::new()
                } else {
                    lifeline(lifeline_y).into_iter().collect()
                },
            );
            (lowered, vec![Element::group("actor-man", els)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_what_the_jison_grammar_rejects() {
        let bad = [
            "sequenceDiagram\nA->>B",
            "sequenceDiagram\nNote over : x",
            "sequenceDiagram\nautonumber x y\nA->>B: x",
            "sequenceDiagram\nA->>B: x\ndeactivate B",
            "sequenceDiagram\nA->>+B: x\nB-->>-A: y\nB-->>-A: z",
        ];
        for src in bad {
            assert!(parse(src, None).is_err(), "{src}");
        }
        let e = parse("sequenceDiagram\nA->>B: x\ndeactivate B", None).unwrap_err();
        assert!(
            e.to_string()
                .contains("Trying to inactivate an inactive participant (B)")
        );
        assert!(
            parse(
                "sequenceDiagram\nautonumber 10 5\nA->>+B: x\nB-->>-A: y",
                None
            )
            .is_ok()
        );
    }

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

    /// Every element of the scene, depth first.
    fn walk<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
        out.push(e);
        if let Element::Group { children, .. } = e {
            for c in children {
                walk(c, out);
            }
        }
    }

    fn all(s: &Scene) -> Vec<&Element> {
        let mut out = Vec::new();
        for e in &s.elements {
            walk(e, &mut out);
        }
        out
    }

    /// Shapes with CSS class `class`.
    fn shapes<'a>(s: &'a Scene, class: &str) -> Vec<&'a Geometry> {
        all(s)
            .into_iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom,
                    class: Some(c),
                    ..
                } if c == class => Some(geom),
                _ => None,
            })
            .collect()
    }

    fn rects(s: &Scene, class: &str) -> Vec<Rect> {
        shapes(s, class)
            .into_iter()
            .filter_map(|g| match g {
                Geometry::Rect { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// Start points of the shapes with class `class` drawn as paths.
    fn path_starts(s: &Scene, class: &str) -> Vec<(Point, Point)> {
        shapes(s, class)
            .into_iter()
            .filter_map(|g| match g {
                Geometry::Path(cmds) => match (cmds.first(), cmds.last()) {
                    (Some(PathCmd::MoveTo(a)), Some(PathCmd::LineTo(b))) => Some((*a, *b)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn lifeline_xs(s: &Scene) -> Vec<f64> {
        let mut xs: Vec<f64> = path_starts(s, "actor-line")
            .iter()
            .map(|(a, _)| a.x)
            .collect();
        xs.sort_by(f64::total_cmp);
        xs
    }

    fn texts(s: &Scene) -> Vec<&Text> {
        all(s)
            .into_iter()
            .filter_map(|e| match e {
                Element::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect()
    }

    fn pal() -> Palette {
        crate::theme::palette(crate::theme::ThemeName::Default, &[])
    }

    fn cfg(look: Look) -> SequenceConfig {
        SequenceConfig {
            look,
            ..SequenceConfig::default()
        }
    }

    fn draw(src: &str, look: Look) -> Scene {
        render(&parse(src, None).unwrap(), &cfg(look), &pal())
    }

    #[test]
    fn title_and_lifelines_as_mermaid_draws_them() {
        // mermaid 12.3.0, the gallery's sequence: lifelines 2px (the stylesheet's
        // bare `line` rule), the title a plain start-anchored text at y = -25, at
        // half the drawing's width less 2·diagramMarginX, in 40px the viewBox adds.
        let s = draw("sequenceDiagram\ntitle Hi\nA->>B: x", Look::Classic);
        let plain = draw("sequenceDiagram\nA->>B: x", Look::Classic);
        for e in all(&s) {
            if let Element::Shape {
                paint,
                class: Some(c),
                ..
            } = e
            {
                if c == "actor-line" {
                    assert_eq!(paint.stroke_width, 2.0);
                }
            }
        }
        let title = texts(&s)
            .into_iter()
            .find(|t| t.lines == ["Hi"])
            .unwrap()
            .clone();
        assert_eq!(title.anchor, Anchor::Start);
        assert!(!title.bold);
        // Two 150-wide actors 50 apart: 350 wide.
        assert_eq!((title.x, title.y), (350.0 / 2.0 - 100.0, -25.0));
        assert_eq!(s.view_box.y, plain.view_box.y - 40.0);
        assert_eq!(s.view_box.h, plain.view_box.h + 40.0);
    }

    #[test]
    fn classic_and_neo_heights_follow_mermaid() {
        // mermaid 12.3.0: `participant A / participant B / A->>B: hello /
        // B-->>A: hi there` is 259 tall in the classic look and 305 under neo
        // (73px actor rows from the band model, plus neo's 30px).
        let src = "sequenceDiagram\nparticipant A\nparticipant B\nA->>B: hello\nB-->>A: hi there";
        let classic = draw(src, Look::Classic);
        let neo = draw(src, Look::Neo);
        assert_eq!(
            (
                classic.view_box.x,
                classic.view_box.y,
                classic.view_box.w,
                classic.view_box.h
            ),
            (-50.0, -10.0, 450.0, 259.0)
        );
        assert_eq!((neo.view_box.w, neo.view_box.h), (450.0, 305.0));
        let heights = |s: &Scene| rects(s, "actor").iter().map(|r| r.h).collect::<Vec<_>>();
        assert_eq!(heights(&classic), [65.0; 4]);
        assert_eq!(heights(&neo), [73.0; 4]);
        // Messages: 10 + text + (text - 10) + boxMargin below the row; the arrow
        // stops 3px short (6px under neo) of the target's lifeline edge.
        let lines = path_starts(&neo, "messageLine0");
        assert_eq!(lines[0].0, Point::new(76.0, 73.0 + 44.0));
        assert_eq!(lines[0].1, Point::new(268.0, 117.0));
        let lines = path_starts(&classic, "messageLine0");
        assert_eq!(lines[0].1, Point::new(271.0, 65.0 + 44.0));
    }

    #[test]
    fn neo_stick_figure_fills_the_glyph_band() {
        // actorBands.ts: the figure is scaled by 44/65 with its feet 6px above the
        // label, whose centre sits 6px + half its height above the lifeline datum.
        let s = draw("sequenceDiagram\nactor U as User\nU->>U: x", Look::Neo);
        let circle = all(&s)
            .into_iter()
            .filter_map(|e| match e {
                Element::Shape { geom, .. } => Some(geom),
                _ => None,
            })
            .find_map(|g| match g {
                Geometry::Circle { c, r } => Some((*c, *r)),
                _ => None,
            })
            .unwrap();
        let k = 44.0 / 65.0;
        let glyph_bottom = 73.0 - 6.0 - 17.0 - 6.0;
        assert!((circle.1 - 15.0 * k).abs() < 1e-9);
        assert!((circle.0.y - (glyph_bottom - 50.0 * k)).abs() < 1e-9);
        let label = texts(&s).into_iter().find(|t| t.lines == ["User"]).unwrap();
        assert!((label.y - (73.0 - 6.0 - 8.5 + 0.35 * 16.0)).abs() < 1e-9);
        // The lifeline starts at the datum (classic: 80px below the row top).
        assert_eq!(path_starts(&s, "actor-line")[0].0.y, 73.0);
        let classic = draw("sequenceDiagram\nactor U as User\nU->>U: x", Look::Classic);
        assert_eq!(path_starts(&classic, "actor-line")[0].0.y, 80.0);
    }

    #[test]
    fn autonumbers_and_notes_as_mermaid_sizes_them() {
        let s = draw(
            "sequenceDiagram\nautonumber\nA->>B: x\nNote over A,B: TLS everywhere",
            Look::Neo,
        );
        // The marker's r=6 circle doubled by the bare `line` rule's 2px.
        let r = shapes(&s, "sequenceNumber")
            .into_iter()
            .find_map(|g| match g {
                Geometry::Circle { r, .. } => Some(*r),
                _ => None,
            });
        assert_eq!(r, Some(12.0));
        // Note: its text's bounding box (18) plus 2 * noteMargin, over both
        // lifelines plus actorMargin.
        let note = rects(&s, "note")[0];
        assert_eq!((note.x, note.w, note.h), (50.0, 250.0, 38.0));
    }

    #[test]
    fn participant_boxes_follow_mermaid_margins() {
        let s = draw(
            "sequenceDiagram\nbox Aqua One\nparticipant A\nend\nbox Two\nparticipant B\nend\nparticipant C",
            Look::Classic,
        );
        let xs = lifeline_xs(&s);
        let boxes = rects(&s, "rect");
        assert_eq!(boxes.len(), 2);
        let (one, two) = if boxes[0].x < boxes[1].x {
            (boxes[0], boxes[1])
        } else {
            (boxes[1], boxes[0])
        };
        // Box = actor (150) + boxTextMargin (5) + 2 * boxMargin (20) on each side.
        assert_eq!(one.x, xs[0] - 100.0);
        assert_eq!(one.right(), xs[0] + 100.0);
        // Neighbouring boxes: actorMargin + boxMargin + 2 * boxTextMargin - 2 * 20 apart.
        assert_eq!(two.x - one.right(), 20.0);
        // The participant after a box is pushed out by boxMargin + boxTextMargin.
        assert_eq!(xs[2] - xs[1], 150.0 + 50.0 + 15.0);
        // Both boxes start `boxMargin / 2` above the top.
        assert_eq!((one.y, two.y), (-5.0, -5.0));
    }

    #[test]
    fn nested_rects_paint_inner_on_top() {
        let s = draw(
            "sequenceDiagram\nA->>B: x\nrect rgb(1, 2, 3)\nA->>B: y\nrect rgb(4, 5, 6)\nA->>B: z\nend\nend",
            Look::Classic,
        );
        let rects: Vec<(Rect, String)> = all(&s)
            .into_iter()
            .filter_map(|e| match e {
                Element::Shape {
                    geom: Geometry::Rect { rect, .. },
                    paint,
                    class: Some(c),
                } if c == "rect" => Some((*rect, paint.fill.0.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 2);
        // Outer first (painted below), inner inset by boxMargin on each side.
        assert_eq!(rects[0].1, "rgb(1, 2, 3)");
        assert_eq!(rects[1].0.x - rects[0].0.x, 10.0);
        assert!(rects[1].0.y > rects[0].0.y && rects[1].0.bottom() < rects[0].0.bottom());
    }

    #[test]
    fn frame_spacing_follows_mermaid() {
        // loop title: boxMargin before the frame, boxMargin + boxTextMargin + max(text, 20) after.
        let h = |src: &str| draw(src, Look::Classic).view_box.h;
        let titled = h("sequenceDiagram\nA->>B: x\nloop L\nA->>B: y\nend");
        let bare = h("sequenceDiagram\nA->>B: x\nloop\nA->>B: y\nend");
        assert_eq!(titled - bare, LABEL_BOX_H);
    }

    #[test]
    fn long_messages_widen_gaps() {
        let short = draw("sequenceDiagram\nA->>B: hi", Look::Classic);
        let long = draw(
            "sequenceDiagram\nA->>B: a very very very very very very very long message text",
            Look::Classic,
        );
        assert!(long.view_box.w > short.view_box.w);
    }

    #[test]
    fn actor_names_cannot_contain_arrow_characters() {
        assert!(parse("sequenceDiagram\nA-->>>B: bad", None).is_err());
        assert!(parse("sequenceDiagram\nA-->>B: ok", None).is_ok());
    }
}
