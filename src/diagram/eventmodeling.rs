//! `eventmodeling` diagrams (mermaid 12.3 `eventmodeling`).
//!
//! The grammar is the langium `event-modeling.langium`: time frames (`tf` /
//! `timeframe`) and reset frames (`rf` / `resetframe`) of the entity types `ui`,
//! `pcr`/`processor`, `cmd`/`command`, `rmo`/`readmodel`, `evt`/`event`, with
//! optional `->> NN` source frames, a `[[DataBlock]]` reference and inline data
//! (`{…}`, `"…"`, `'…'`, optionally typed with `` `json` `` etc.), plus `data`,
//! `entity`, `note` and `gwt` statements (the last three are parsed and, as
//! upstream, not drawn).
//!
//! Layout follows `db.ts` literally: frames are positioned one after another along
//! the timeline, each in the swimlane of its entity type (UI/Automation, Command/Read
//! Model, Events, or a new `UI/A: ns`, `C/RM: ns`, `Stream: ns` lane for a namespaced
//! identifier), overlapping the previous box by `boxOverlap`; a relation is inferred
//! from the closest earlier frame in another lane unless the frame is a reset frame
//! or names its sources with `->>`.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use crate::theme::Palette;

/// `diagramProps` of `db.ts`.
const SWIMLANE_MIN_HEIGHT: f64 = 70.0;
const SWIMLANE_PADDING: f64 = 15.0;
const SWIMLANE_GAP: f64 = 10.0;
const BOX_PADDING: f64 = 10.0;
const BOX_OVERLAP: f64 = 90.0;
const BOX_MIN_WIDTH: f64 = 80.0;
const BOX_MAX_WIDTH: f64 = 450.0;
const BOX_MIN_HEIGHT: f64 = 80.0;
const BOX_MAX_HEIGHT: f64 = 750.0;
const CONTENT_START_X: f64 = 250.0;
const TEXT_MAX_WIDTH: f64 = 450.0 - 2.0 * 10.0;
const BOX_TEXT_PADDING: f64 = 10.0;
/// Text measurement of `calculateTextProps`: 16px bold.
const TEXT_FONT: f64 = 16.0;
/// `<code>` keeps the 16px the svg root sets explicitly (the browser's 13px
/// monospace default applies only to an unset size).
const CODE_FONT: f64 = 16.0;
/// Advance of the monospace `<code>` font, in em.
const CODE_ADVANCE: f64 = 0.6;
/// `line-height: normal` of the HTML label's name and code lines, and their
/// baselines below each line box's top (measured in Chromium).
const NAME_LINE_HEIGHT: f64 = 18.5;
const NAME_BASELINE: f64 = 15.2;
const CODE_LINE_HEIGHT: f64 = 19.0;
const CODE_BASELINE: f64 = 15.2;

/// The five entity types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    Ui,
    Processor,
    Command,
    ReadModel,
    Event,
}

impl EntityType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ui" => EntityType::Ui,
            "pcr" | "processor" => EntityType::Processor,
            "cmd" | "command" => EntityType::Command,
            "rmo" | "readmodel" => EntityType::ReadModel,
            "evt" | "event" => EntityType::Event,
            _ => return None,
        })
    }
}

/// A time frame or reset frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Frame number as written (`01`).
    pub name: String,
    pub reset: bool,
    pub entity: EntityType,
    /// Qualified entity identifier (`Ns.Name`).
    pub identifier: String,
    pub sources: Vec<String>,
    pub data_ref: Option<String>,
    /// Raw inline data including its delimiters.
    pub inline_data: Option<String>,
    pub line: usize,
}

/// A `data Name { … }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct DataBlock {
    pub name: String,
    /// Raw block text from `{` to `}`.
    pub value: String,
}

/// A parsed event model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EventModel {
    pub frames: Vec<Frame>,
    pub data: Vec<DataBlock>,
    /// `entity` declarations.
    pub entities: Vec<String>,
}

/// Strip `%%` and `//` line comments and `/* */` block comments.
fn strip_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i..].find("*/") {
            Some(j) => {
                // Keep line numbers.
                out.extend(rest[i..i + j].chars().filter(|&c| c == '\n'));
                rest = &rest[i + j + 2..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.lines()
        .map(|l| {
            let mut cut = l.len();
            for pat in ["%%", "//"] {
                if let Some(i) = l.find(pat) {
                    cut = cut.min(i);
                }
            }
            &l[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_frame_id(s: &str) -> bool {
    (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_qualified(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|p| {
            let mut c = p.chars();
            c.next()
                .is_some_and(|f| f == '_' || f.is_ascii_alphabetic())
                && c.all(|ch| ch == '_' || ch.is_alphanumeric())
        })
}

const DATA_TYPES: [&str; 8] = [
    "json", "jsobj", "figma", "salt", "uri", "md", "html", "text",
];

/// Split an optional `` `type` `` prefix off `s`.
fn data_type_prefix(line: usize, s: &str) -> Result<&str> {
    let Some(rest) = s.strip_prefix('`') else {
        return Ok(s);
    };
    let end = rest
        .find('`')
        .ok_or_else(|| Error::parse(line, "Expecting a closing '`' after the data type"))?;
    if !DATA_TYPES.contains(&&rest[..end]) {
        return Err(Error::parse(
            line,
            format!("Unknown data type '{}'", &rest[..end]),
        ));
    }
    Ok(rest[end + 1..].trim_start())
}

/// Read a `{ … }` block starting at `lines[i]` (after `head`), possibly spanning lines.
/// Returns the block text and the index of its last line.
fn read_block(lines: &[&str], i: usize, head: &str) -> Result<(String, usize)> {
    let mut start = i;
    let mut text = head.trim().to_string();
    if text.is_empty() {
        // `data Name` with the `{` on the next line.
        start += 1;
        text = lines
            .get(start)
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
    }
    let text = data_type_prefix(start + 1, &text)?.to_string();
    if !text.starts_with('{') {
        return Err(Error::parse(start + 1, "Expecting a data block '{'"));
    }
    if text.ends_with('}') && text.len() > 1 && start == i {
        return Ok((text, start));
    }
    let mut value = text;
    for (k, l) in lines.iter().enumerate().skip(start + 1) {
        value.push('\n');
        value.push_str(l);
        if l.trim() == "}" || l.trim_end().ends_with('}') && l.trim_start().starts_with('}') {
            return Ok((value, k));
        }
    }
    Err(Error::parse(
        lines.len(),
        "Unterminated data block: expecting '}'",
    ))
}

/// Parse an `eventmodeling` body.
pub fn parse(body: &str) -> Result<EventModel> {
    let src = strip_comments(body);
    let lines: Vec<&str> = src.lines().collect();
    let mut model = EventModel::default();
    let mut header = false;
    let mut i = 0;
    while i < lines.len() {
        let line_no = i + 1;
        let raw = lines[i];
        let stmt = raw.trim();
        i += 1;
        if stmt.is_empty() {
            continue;
        }
        if !header {
            let rest = stmt
                .strip_prefix("eventmodeling")
                .ok_or_else(|| Error::parse(line_no, "Expecting 'eventmodeling'"))?;
            header = true;
            if rest.trim().is_empty() {
                continue;
            }
            return Err(Error::parse(
                line_no,
                format!("Unexpected '{}' after 'eventmodeling'", rest.trim()),
            ));
        }
        let (kw, rest) = match stmt.find(char::is_whitespace) {
            Some(k) => (&stmt[..k], stmt[k..].trim()),
            None => (stmt, ""),
        };
        match kw {
            "tf" | "timeframe" | "rf" | "resetframe" => {
                let frame = parse_frame(line_no, kw.starts_with('r'), rest)?;
                if model.frames.iter().any(|f| f.name == frame.name) {
                    return Err(Error::parse(
                        line_no,
                        format!(
                            "Duplicate event modeling frame ID \"{}\" on line {line_no}",
                            frame.name
                        ),
                    ));
                }
                model.frames.push(frame);
            }
            "data" => {
                let (name, head) =
                    match rest.find(|c: char| c.is_whitespace() || c == '{' || c == '`') {
                        Some(k) => (&rest[..k], &rest[k..]),
                        None => (rest, ""),
                    };
                if !is_qualified(name) || name.contains('.') {
                    return Err(Error::parse(line_no, "Expecting a data block name"));
                }
                let (value, last) = read_block(&lines, line_no - 1, head)?;
                model.data.push(DataBlock {
                    name: name.to_string(),
                    value,
                });
                i = last + 1;
            }
            "note" => {
                let (frame, head) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if !is_frame_id(frame) {
                    return Err(Error::parse(
                        line_no,
                        "Expecting a frame number after 'note'",
                    ));
                }
                let (_, last) = read_block(&lines, line_no - 1, head)?;
                i = last + 1;
            }
            "entity" => {
                if !is_qualified(rest) {
                    return Err(Error::parse(line_no, "Expecting an entity name"));
                }
                model.entities.push(rest.to_string());
            }
            "gwt" => {
                // Given/when/then statements may span lines; they are not drawn.
                while i < lines.len() {
                    let next = lines[i].trim();
                    let first = next.split_whitespace().next().unwrap_or("");
                    if first.is_empty()
                        || matches!(first, "given" | "when" | "then")
                        || EntityType::parse(first).is_some()
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            "title" | "accTitle" | "accDescr" | "accTitle:" | "accDescr:" => {}
            _ if stmt.starts_with("accTitle") || stmt.starts_with("accDescr") => {}
            _ => {
                return Err(Error::parse(
                    line_no,
                    format!(
                        "Unexpected '{kw}': expecting 'tf', 'rf', 'data', 'note', 'entity' or 'gwt'"
                    ),
                ));
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'eventmodeling'"));
    }
    Ok(model)
}

fn parse_frame(line: usize, reset: bool, rest: &str) -> Result<Frame> {
    let mut words = rest.splitn(3, char::is_whitespace);
    let name = words.next().unwrap_or("");
    if !is_frame_id(name) {
        return Err(Error::parse(
            line,
            format!("Expecting a frame number (1-3 digits), got '{name}'"),
        ));
    }
    let ty = words.next().unwrap_or("");
    let entity = EntityType::parse(ty).ok_or_else(|| {
        Error::parse(
            line,
            format!(
                "Unknown entity type '{ty}': expecting ui, pcr, processor, cmd, command, rmo, readmodel, evt or event"
            ),
        )
    })?;
    let mut rest = words.next().unwrap_or("").trim();
    let end = rest
        .find(|c: char| !(c == '_' || c == '.' || c.is_alphanumeric()))
        .unwrap_or(rest.len());
    let identifier = &rest[..end];
    if !is_qualified(identifier) {
        return Err(Error::parse(line, "Expecting an entity identifier"));
    }
    rest = rest[end..].trim_start();
    let mut sources = Vec::new();
    while let Some(r) = rest.strip_prefix("->>") {
        let r = r.trim_start();
        let k = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        if !is_frame_id(&r[..k]) {
            return Err(Error::parse(line, "Expecting a frame number after '->>'"));
        }
        sources.push(r[..k].to_string());
        rest = r[k..].trim_start();
    }
    let mut data_ref = None;
    if let Some(r) = rest.strip_prefix("[[") {
        let k = r
            .find("]]")
            .ok_or_else(|| Error::parse(line, "Expecting ']]'"))?;
        data_ref = Some(r[..k].trim().to_string());
        rest = r[k + 2..].trim_start();
    }
    let mut inline_data = None;
    if !rest.is_empty() {
        let d = data_type_prefix(line, rest)?;
        let ok = (d.starts_with('{') && d.ends_with('}'))
            || (d.len() >= 2 && d.starts_with('"') && d.ends_with('"'))
            || (d.len() >= 2 && d.starts_with('\'') && d.ends_with('\''));
        if !ok {
            return Err(Error::parse(line, format!("Unexpected '{d}'")));
        }
        inline_data = Some(d.to_string());
    }
    Ok(Frame {
        name: name.to_string(),
        reset,
        entity,
        identifier: identifier.to_string(),
        sources,
        data_ref,
        inline_data,
        line,
    })
}

// ---------------------------------------------------------------------------
// Positioning (db.ts `getState`)
// ---------------------------------------------------------------------------

/// A swimlane of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Swimlane {
    /// Ordering key: 0.. UI/Automation, 100.. Command/Read Model, 200.. Events.
    pub index: u32,
    pub label: String,
    pub y: f64,
    pub height: f64,
    max_height: f64,
    r: f64,
}

/// A positioned frame.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxModel {
    pub frame: usize,
    pub swimlane: u32,
    pub x: f64,
    pub w: f64,
    pub h: f64,
    pub name: String,
    /// The name as `wrapLabel` breaks it.
    pub name_lines: Vec<String>,
    /// Data lines shown under the name (`wrapLabel`ed; a line may still hold
    /// newlines, which the HTML renders as spaces).
    pub data: Option<Vec<String>>,
}

/// The positioned model.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub boxes: Vec<BoxModel>,
    /// Sorted by index.
    pub swimlanes: Vec<Swimlane>,
    /// (source box, target box).
    pub relations: Vec<(usize, usize)>,
    pub max_r: f64,
}

fn split_identifier(id: &str) -> (Option<&str>, &str) {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() == 2 {
        (Some(parts[0]), parts[1])
    } else {
        (None, id)
    }
}

/// `calculateSwimlaneProps`. Lanes do not remember their namespace upstream, so
/// every namespaced frame opens a new lane after the last one of its kind.
fn swimlane_props(frame: &Frame, lanes: &[Swimlane]) -> (u32, String) {
    let (ns, _) = split_identifier(&frame.identifier);
    let (base, label, prefix) = match frame.entity {
        EntityType::Ui | EntityType::Processor => (0, "UI/Automation", "UI/A: "),
        EntityType::Command | EntityType::ReadModel => (100, "Command/Read Model", "C/RM: "),
        EntityType::Event => (200, "Events", "Stream: "),
    };
    match ns.filter(|n| !n.is_empty()) {
        Some(ns) => {
            let next = lanes
                .iter()
                .map(|l| l.index)
                .filter(|&i| i > base && i < base + 100)
                .fold(base, u32::max)
                + 1;
            (next, format!("{prefix}{ns}"))
        }
        None => (base, label.to_string()),
    }
}

/// JavaScript's `Math.round`.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Split on mermaid's `lineBreakRegex` (`<br>` variants) only; newlines stay.
fn split_br(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let lower = rest.get(..3).map(str::to_ascii_lowercase);
            if lower.as_deref() == Some("<br") {
                if let Some(close) = rest.find('>') {
                    let inner = rest[3..close].trim();
                    if inner.is_empty() || inner == "/" {
                        out.push(std::mem::take(&mut cur));
                        rest = &rest[close + 1..];
                        continue;
                    }
                }
            }
        }
        cur.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.push(cur);
    out
}

/// Width of one line as an SVG `<text>` measures it at the 16px bold font:
/// newlines render as spaces and runs of spaces collapse.
fn measured_width(line: &str) -> f64 {
    let collapsed = line
        .split([' ', '\n'])
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let lead = if line.starts_with([' ', '\n']) && !collapsed.is_empty() {
        " "
    } else {
        ""
    };
    js_round(line_width(&format!("{lead}{collapsed}"), TEXT_FONT, true))
}

/// `calculateTextWidth` at the 16px bold measuring font: the widest line
/// (newlines inside a line render as spaces), rounded.
fn text_width(s: &str) -> f64 {
    split_br(s)
        .iter()
        .map(|l| measured_width(l))
        .fold(0.0, f64::max)
}

/// mermaid's `wrapLabel` with `calculateTextProps`' 16px bold font: words
/// (split on spaces, so runs of spaces collapse) are packed into lines
/// narrower than `max`; a word wider than `max` is broken with hyphens.
/// Returns the lines it would join with `<br/>`.
fn wrap_label(label: &str, max: f64) -> Vec<String> {
    if label.is_empty() {
        return Vec::new();
    }
    let parts = split_br(label);
    if parts.len() > 1 {
        return parts;
    }
    let words: Vec<&str> = label.split(' ').filter(|w| !w.is_empty()).collect();
    let mut done: Vec<String> = Vec::new();
    let mut next = String::new();
    for word in &words {
        let word_len = text_width(&format!("{word} "));
        let next_len = text_width(&next);
        if word_len > max {
            // breakString: characters until the line reaches `max`.
            done.push(std::mem::take(&mut next));
            let chars: Vec<char> = word.chars().collect();
            let mut cur = String::new();
            for (i, ch) in chars.iter().enumerate() {
                let cand = format!("{cur}{ch}");
                if text_width(&cand) >= max {
                    let last = i + 1 == chars.len();
                    done.push(if last { cand } else { format!("{cand}-") });
                    cur.clear();
                } else {
                    cur = cand;
                }
            }
            next = cur;
        } else if next_len + word_len >= max {
            done.push(std::mem::replace(&mut next, (*word).to_string()));
        } else if next.is_empty() {
            next = (*word).to_string();
        } else {
            next = format!("{next} {word}");
        }
    }
    if !words.is_empty() {
        done.push(next);
    }
    done.retain(|l| !l.is_empty());
    done
}

/// `s.substring(0, s.lastIndexOf('}') - 1)`: drops the `}` and the character
/// before it (everything when there is no `}`).
fn cut_before_close(s: &str) -> &str {
    match s.rfind('}') {
        Some(i) => {
            let end = s[..i].char_indices().next_back().map_or(0, |(k, _)| k);
            &s[..end]
        }
        None => "",
    }
}

/// The data a frame shows (`calculateTextProps`' `toHtml` before the spaces
/// become `&nbsp;`): its lines and whether it came from a data block (which
/// appends a trailing `<br/>`).
fn frame_data(frame: &Frame, data: &[DataBlock]) -> Option<(Vec<String>, bool)> {
    let mut out = None;
    if let Some(d) = &frame.inline_data {
        let from = d.find('{').map_or(0, |a| a + 1);
        let inner = cut_before_close(&d[from..]);
        out = Some((wrap_label(inner, TEXT_MAX_WIDTH), false));
    }
    if let Some(r) = &frame.data_ref {
        if let Some(block) = data.iter().find(|b| &b.name == r) {
            let v = &block.value;
            // indexOf('{\n') + 2: from the second character when absent.
            let from = v.find("{\n").map_or_else(
                || v.char_indices().nth(1).map_or(v.len(), |(k, _)| k),
                |a| a + 2,
            );
            let inner = cut_before_close(&v[from..]);
            out = Some((wrap_label(inner, TEXT_MAX_WIDTH), true));
        }
    }
    out
}

/// Width and height `calculateTextDimensions` reports for the frame's HTML
/// content. The markup is measured as literal text, line by line between the
/// `<br/>`s, and a box with data is credited a third of that width (upstream's
/// "temporal workaround").
fn text_dimensions(name: &[String], data: Option<&(Vec<String>, bool)>) -> (f64, f64) {
    let mut lines: Vec<String> = name.to_vec();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines[0] = format!("<b>{}", lines[0]);
    if let Some(l) = lines.last_mut() {
        l.push_str("</b>");
    }
    if let Some((d, from_block)) = data {
        lines.push(String::new());
        let mut code: Vec<String> = d.iter().map(|l| l.replace(' ', "&nbsp;")).collect();
        if *from_block || code.is_empty() {
            code.push(String::new());
        }
        code[0] = format!(
            "<code style=\"text-align: left; display: block;max-width:{TEXT_MAX_WIDTH}px\">{}",
            code[0]
        );
        if let Some(l) = code.last_mut() {
            l.push_str("</code>");
        }
        lines.extend(code);
    }
    let w = lines.iter().map(|l| measured_width(l)).fold(0.0, f64::max);
    let w = if data.is_some() { w / 3.0 } else { w };
    let h = lines
        .iter()
        .map(|l| crate::text::svg_line_box(TEXT_FONT, l))
        .sum();
    (w, h)
}

/// Position frames, lanes and relations (`getState`). Pure.
pub fn position(model: &EventModel) -> State {
    let mut st = State {
        boxes: Vec::new(),
        swimlanes: Vec::new(),
        relations: Vec::new(),
        max_r: 0.0,
    };
    let mut previous_lane: Option<u32> = None;
    for (index, frame) in model.frames.iter().enumerate() {
        let (_, name) = split_identifier(&frame.identifier);
        let name_lines = wrap_label(name, TEXT_MAX_WIDTH);
        let data = frame_data(frame, &model.data);
        let (tw, th) = text_dimensions(&name_lines, data.as_ref());
        let (lane_index, label) = swimlane_props(frame, &st.swimlanes);
        if !st.swimlanes.iter().any(|l| l.index == lane_index) {
            st.swimlanes.push(Swimlane {
                index: lane_index,
                label,
                y: 0.0,
                height: SWIMLANE_MIN_HEIGHT,
                max_height: SWIMLANE_MIN_HEIGHT,
                r: 0.0,
            });
        }
        let w =
            (tw + 2.0 * BOX_TEXT_PADDING).clamp(BOX_MIN_WIDTH, BOX_MAX_WIDTH) + 2.0 * BOX_PADDING;
        let h =
            (th + 2.0 * BOX_TEXT_PADDING).clamp(BOX_MIN_HEIGHT, BOX_MAX_HEIGHT) + 2.0 * BOX_PADDING;
        let lane_r = st
            .swimlanes
            .iter()
            .find(|l| l.index == lane_index)
            .map_or(0.0, |l| l.r);
        // calculateX
        let x = match (previous_lane, st.boxes.last()) {
            (None, _) => CONTENT_START_X,
            (Some(p), _) if p == lane_index && lane_r != 0.0 => lane_r + BOX_PADDING,
            (_, None) => CONTENT_START_X,
            (_, Some(last)) => last.x + last.w + BOX_PADDING - BOX_OVERLAP + BOX_PADDING,
        };
        let r = x + w + BOX_PADDING;
        st.max_r = st.swimlanes.iter().map(|l| l.r).fold(r, f64::max);
        if let Some(lane) = st.swimlanes.iter_mut().find(|l| l.index == lane_index) {
            lane.r = x + w;
            lane.max_height = lane.max_height.max(h);
            lane.height = lane.max_height.max(SWIMLANE_MIN_HEIGHT) + 2.0 * SWIMLANE_PADDING;
        }
        st.boxes.push(BoxModel {
            frame: index,
            swimlane: lane_index,
            x,
            w,
            h,
            name: name.to_string(),
            name_lines,
            data: data.map(|(d, _)| d),
        });
        previous_lane = Some(lane_index);
        st.swimlanes.sort_by_key(|l| l.index);
        let mut y = 0.0;
        for lane in &mut st.swimlanes {
            lane.y = y;
            y += lane.height + SWIMLANE_GAP;
        }

        // Relations (decidePositionRelation).
        let target = index;
        if !frame.sources.is_empty() {
            for (fi, f) in model.frames.iter().enumerate() {
                if frame.sources.contains(&f.name) && fi < st.boxes.len() {
                    st.relations.push((fi, target));
                }
            }
        } else if !(index == 0 || frame.reset) {
            if let Some(src) = (0..index)
                .rev()
                .find(|&b| st.boxes[b].swimlane != lane_index)
            {
                st.relations.push((src, target));
            }
        }
    }
    st
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

struct Colors {
    fill: [Color; 5],
    stroke: [Color; 5],
    lane_bg: Color,
    lane_stroke: Color,
    relation: Color,
    arrowhead: Color,
    box_text: Color,
}

fn colors(cfg: &MermaidConfig, pal: &Palette) -> Colors {
    let var = |k: &str| {
        cfg.theme_variables
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| Color::new(v.clone()))
    };
    let lighten = |c: &str, d: f64| {
        Color::new(
            crate::color::parse(c)
                .map(|c| c.lighten(d).to_hex())
                .unwrap_or_else(|| c.to_string()),
        )
    };
    let (fills, strokes, bg, bgs) = if pal.dark {
        let bg = pal.background.css().to_string();
        (
            [
                Color::new("#2d2d2d"),
                lighten("#5a3d5c", 10.0),
                lighten("#2d3d5a", 10.0),
                lighten("#3d5a2d", 10.0),
                lighten("#5a452d", 10.0),
            ],
            ["#555", "#8a6d8c", "#5c6d8c", "#6d8c5c", "#8c755c"].map(Color::new),
            lighten(&bg, 5.0),
            lighten(&bg, 12.0),
        )
    } else {
        (
            ["white", "#edb3f6", "#bcd6fe", "#d3f1a2", "#ffb778"].map(Color::new),
            ["#dbdada", "#b88cbf", "#679ac3", "#a3b732", "#c19a0f"].map(Color::new),
            Color::new("rgb(250,250,250)"),
            Color::new("rgb(240,240,240)"),
        )
    };
    let names = ["Ui", "Processor", "Command", "ReadModel", "Event"];
    let fill =
        std::array::from_fn(|i| var(&format!("em{}Fill", names[i])).unwrap_or(fills[i].clone()));
    let stroke = std::array::from_fn(|i| {
        var(&format!("em{}Stroke", names[i])).unwrap_or(strokes[i].clone())
    });
    Colors {
        fill,
        stroke,
        lane_bg: var("emSwimlaneBackgroundOdd").unwrap_or(bg),
        lane_stroke: var("emSwimlaneBackgroundStroke").unwrap_or(bgs),
        relation: var("emRelationStroke").unwrap_or_else(|| pal.line_color.clone()),
        arrowhead: var("emArrowhead").unwrap_or_else(|| pal.line_color.clone()),
        box_text: if pal.dark {
            pal.text_color.clone()
        } else {
            Color::new("#000000")
        },
    }
}

fn slot(e: EntityType) -> usize {
    match e {
        EntityType::Ui => 0,
        EntityType::Processor => 1,
        EntityType::Command => 2,
        EntityType::ReadModel => 3,
        EntityType::Event => 4,
    }
}

/// Width of a run of the browser's 13px monospace `<code>` font.
fn code_width(s: &str) -> f64 {
    s.chars().count() as f64 * CODE_FONT * CODE_ADVANCE
}

/// Lay out one `toHtml` line in the `<code>` block: spaces are `&nbsp;`, so
/// only the newlines (rendered as collapsible spaces) are break opportunities.
fn wrap_code(line: &str, max: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur: Option<String> = None;
    for seg in line.split('\n') {
        let seg = seg.replace(' ', "\u{a0}");
        cur = Some(match cur {
            None => seg,
            Some(c) => {
                let cand = format!("{c} {seg}");
                if code_width(&cand) <= max {
                    cand
                } else {
                    out.push(c);
                    seg
                }
            }
        });
    }
    out.extend(cur);
    out
}

/// Parse, lay out and draw an `eventmodeling` diagram.
pub fn render(
    body: &str,
    _title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let model = parse(body)?;
    let st = position(&model);
    Ok(draw(&model, &st, cfg, pal))
}

/// Draw a positioned event model.
pub fn draw(model: &EventModel, st: &State, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let c = colors(cfg, pal);
    let padding = cfg.section_f64("eventmodeling", "padding", 30.0);
    let font = pal.font_size;
    let lane_y = |idx: u32| {
        st.swimlanes
            .iter()
            .find(|l| l.index == idx)
            .map_or(0.0, |l| l.y)
    };
    let mut bounds = Bounds::default();
    let mut lanes = Vec::new();
    for lane in &st.swimlanes {
        let r = Rect::new(0.0, lane.y, st.max_r + SWIMLANE_PADDING, lane.height);
        bounds.add_rect(r);
        lanes.push(Element::group(
            "em-swimlane",
            vec![
                Element::rect(r, 3.0, Paint::both(&c.lane_bg, &c.lane_stroke, 1.0)),
                Element::text(Text {
                    x: 30.0,
                    y: lane.y + 30.0,
                    lines: vec![lane.label.clone()],
                    line_height: font * 1.5,
                    font_size: font,
                    bold: true,
                    anchor: Anchor::Start,
                    fill: pal.text_color.clone(),
                }),
            ],
        ));
    }
    let rect_of = |b: &BoxModel| Rect::new(b.x, lane_y(b.swimlane) + SWIMLANE_PADDING, b.w, b.h);
    let mut boxes = Vec::new();
    for b in &st.boxes {
        let r = rect_of(b);
        bounds.add_rect(r);
        let k = slot(model.frames[b.frame].entity);
        let mut children = vec![Element::rect(
            r,
            3.0,
            Paint::both(&c.fill[k], &c.stroke[k], 1.0),
        )];
        // The HTML content, vertically centred in the inner area: the bold
        // name, an empty line, then the `<code>` block (left-aligned, at most
        // `textMaxWidth` wide).
        let code_w = (b.w - 2.0 * BOX_PADDING).min(TEXT_MAX_WIDTH);
        let data_lines: Vec<String> = b
            .data
            .iter()
            .flatten()
            .flat_map(|l| wrap_code(l, code_w))
            .collect();
        let name_h = b.name_lines.len() as f64 * NAME_LINE_HEIGHT;
        let data_h = if b.data.is_some() {
            NAME_LINE_HEIGHT + data_lines.len() as f64 * CODE_LINE_HEIGHT
        } else {
            0.0
        };
        let top = r.y + r.h / 2.0 - (name_h + data_h) / 2.0;
        children.push(Element::text(Text {
            x: r.x + r.w / 2.0,
            y: top + NAME_BASELINE,
            lines: b.name_lines.clone(),
            line_height: NAME_LINE_HEIGHT,
            font_size: TEXT_FONT,
            bold: true,
            anchor: Anchor::Middle,
            fill: c.box_text.clone(),
        }));
        if !data_lines.is_empty() {
            children.push(Element::classed_text(
                Text {
                    x: r.x + BOX_PADDING,
                    y: top + name_h + NAME_LINE_HEIGHT + CODE_BASELINE,
                    lines: data_lines,
                    line_height: CODE_LINE_HEIGHT,
                    font_size: CODE_FONT,
                    bold: false,
                    anchor: Anchor::Start,
                    fill: c.box_text.clone(),
                },
                "em-data",
            ));
        }
        boxes.push(Element::group("em-box", children));
    }
    let mut relations = Vec::new();
    for &(s, t) in &st.relations {
        let (sb, tb) = (&st.boxes[s], &st.boxes[t]);
        let (sr, tr) = (rect_of(sb), rect_of(tb));
        let upwards = sr.y > tr.y;
        let sx = sr.x + sr.w * 2.0 / 3.0;
        let tx = tr.x + tr.w / 3.0;
        let (sy, ty) = if upwards {
            (sr.y, tr.bottom())
        } else {
            (sr.bottom(), tr.y)
        };
        let (a, b) = (Point::new(sx, sy), Point::new(tx, ty));
        // Marker: `0 0, 10 3.5, 0 7` with refX 10, so the tip sits on the end point.
        let d = b.minus(a).norm();
        let perp = Point::new(-d.y, d.x);
        let head = vec![
            b.minus(d.scale(10.0)).plus(perp.scale(3.5)),
            b,
            b.minus(d.scale(10.0)).minus(perp.scale(3.5)),
        ];
        relations.push(Element::classed_shape(
            Geometry::Path(geom::polyline(&[a, b])),
            Paint::stroke(&c.relation, 1.0),
            "em-relation",
        ));
        relations.push(Element::shape(
            Geometry::Path(geom::polygon(&head)),
            Paint::fill(&c.arrowhead),
        ));
    }
    let mut elements = lanes;
    elements.extend(boxes);
    elements.extend(relations);
    Scene {
        kind: DiagramKind::EventModeling,
        view_box: bounds.rect().pad(padding),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        // Data is HTML `<code>`: the browser's monospace font.
        css: "text.em-data{font-family:monospace !important;}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(st: &State, model: &EventModel) -> Vec<(String, String)> {
        st.relations
            .iter()
            .map(|&(s, t)| (model.frames[s].name.clone(), model.frames[t].name.clone()))
            .collect()
    }

    #[test]
    fn simple_definition() {
        let m = parse(
            "eventmodeling\n    tf 01 ui UI\n    tf 02 cmd RunAction\n    tf 03 evt ActionExecuted",
        )
        .unwrap();
        assert_eq!(m.frames.len(), 3);
        assert_eq!(m.frames[1].entity, EntityType::Command);
        let st = position(&m);
        assert_eq!(
            names(&st, &m),
            [("01".into(), "02".into()), ("02".into(), "03".into())]
        );
        let labels: Vec<&str> = st.swimlanes.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["UI/Automation", "Command/Read Model", "Events"]);
    }

    #[test]
    fn inline_data_and_blocks() {
        let m = parse(
            "eventmodeling\n    tf 01 cmd AddItem { productId: 7 }\n    tf 02 evt ItemAdded [[ItemAddedData]]\n\ndata ItemAddedData\n{\n  productId: 7\n}",
        )
        .unwrap();
        assert_eq!(m.frames[0].inline_data.as_deref(), Some("{ productId: 7 }"));
        assert_eq!(m.frames[1].data_ref.as_deref(), Some("ItemAddedData"));
        assert_eq!(m.data.len(), 1);
        // wrapLabel collapses the indentation; a block adds a trailing `<br/>`.
        assert_eq!(
            frame_data(&m.frames[1], &m.data),
            Some((vec!["productId: 7".to_string()], true))
        );
        // Inline data drops `}` and the character before it, as upstream.
        assert_eq!(
            frame_data(&m.frames[0], &m.data),
            Some((vec!["productId: 7".to_string()], false))
        );
        let typed =
            parse("eventmodeling\ntf 01 rmo UserAdded `json`{ \"name\": \"foo\" }").unwrap();
        assert!(typed.frames[0].inline_data.is_some());
    }

    #[test]
    fn boxes_are_sized_like_calculate_text_props() {
        // Sizes from mermaid 12.3: the HTML markup is measured as literal text
        // (a third of it with data), block newlines as spaces.
        let m = parse(
            "eventmodeling\ntf 01 cmd AddItem [[AddItemData]]\ntf 02 evt HHHH { HHHH: HHHH }\n\ndata AddItemData {\n  HHHH: HHHH\n  productIdentifier: string\n  quantity: number\n}",
        )
        .unwrap();
        let st = position(&m);
        let size = |i: usize| (st.boxes[i].w, st.boxes[i].h);
        assert!((size(0).0 - 427.667).abs() < 0.01 && size(0).1 == 109.0);
        assert!((size(1).0 - 270.333).abs() < 0.01 && size(1).1 == 100.0);
        // Newlines are the only break opportunities in the `<code>` block.
        assert_eq!(
            wrap_code(
                "HHHH: HHHH\n productIdentifier: string\n quantity: number",
                407.0
            ),
            [
                "HHHH:\u{a0}HHHH \u{a0}productIdentifier:\u{a0}string",
                "\u{a0}quantity:\u{a0}number"
            ]
        );
        let lanes: Vec<f64> = st.swimlanes.iter().map(|l| l.height).collect();
        assert_eq!(lanes, [139.0, 130.0]);
    }

    #[test]
    fn qualified_names_open_namespace_lanes() {
        let m = parse("eventmodeling\n    tf 01 ui CartUI\n    tf 02 cmd Inventory.AddItem\n    tf 03 evt Inventory.ItemAdded").unwrap();
        let st = position(&m);
        let labels: Vec<&str> = st.swimlanes.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(
            labels,
            ["UI/Automation", "C/RM: Inventory", "Stream: Inventory"]
        );
        assert_eq!(st.boxes[1].name, "AddItem");
    }

    #[test]
    fn multiple_sources() {
        let m = parse("eventmodeling\n    tf 01 ui CartUI\n    tf 02 cmd AddItem\n    tf 03 cmd RemoveItem\n    tf 04 evt ItemChanged ->> 02 ->> 03").unwrap();
        assert_eq!(m.frames[3].sources, ["02", "03"]);
        let st = position(&m);
        let rel = names(&st, &m);
        assert!(rel.contains(&("02".into(), "04".into())));
        assert!(rel.contains(&("03".into(), "04".into())));
    }

    #[test]
    fn reset_frames_break_inference() {
        let m = parse("eventmodeling\n      tf 01 ui UI\n      tf 02 cmd Command\n      tf 03 evt Event\n      rf 04 rmo ReadModel ->> 03\n      rf 05 ui OtherUI").unwrap();
        let rel = names(&position(&m), &m);
        assert!(rel.contains(&("03".into(), "04".into())));
        assert!(!rel.contains(&("04".into(), "05".into())));
    }

    #[test]
    fn duplicate_frame_ids_are_rejected() {
        let e = parse(
            "eventmodeling\n        tf 01 ui UI\n        %% comment\n        rf 01 evt Event",
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("Duplicate event modeling frame ID \"01\" on line 4"),
            "{e}"
        );
    }

    #[test]
    fn all_entity_types_and_errors() {
        let m = parse("eventmodeling\n tf 01 ui UI\n tf 02 ui UI2\n tf 03 cmd Command\n tf 04 command Command2\n tf 05 evt Event\n tf 06 event Event2\n tf 07 pcr Processor\n tf 08 processor Processor2\n tf 09 rmo ReadModel\n tf 10 readmodel ReadModel2").unwrap();
        assert_eq!(m.frames.len(), 10);
        assert!(parse("eventmodeling\ntf 01 foo X").is_err());
        assert!(parse("eventmodeling\ntf 1234 ui X").is_err());
        assert!(parse("eventmodeling\nbogus").is_err());
    }

    #[test]
    fn boxes_follow_db_geometry() {
        // Lane rows: 130 high (70 + 2·15 padding, boxes 100 high), 10 apart.
        let m = parse("eventmodeling\nrf 02 evt CartCreated\nrf 03 evt ItemAdded\ntf 01 rmo CartUI ->> 02 ->> 03").unwrap();
        let st = position(&m);
        assert_eq!(st.boxes[0].x, CONTENT_START_X);
        assert!((st.boxes[1].x - (st.boxes[0].x + st.boxes[0].w + BOX_PADDING)).abs() < 1e-9);
        // A box in another lane overlaps the previous one by `boxOverlap`.
        let last = &st.boxes[1];
        assert!((st.boxes[2].x - (last.x + last.w + 2.0 * BOX_PADDING - BOX_OVERLAP)).abs() < 1e-9);
        assert_eq!(st.swimlanes[0].height, 130.0);
        assert_eq!(st.swimlanes[1].y, 140.0);
        assert!(st.boxes.iter().all(|b| b.h == 100.0 && b.w >= 100.0));
        assert_eq!(st.relations.len(), 2);
    }

    #[test]
    fn notes_entities_and_gwt_parse() {
        let m = parse("eventmodeling\ntf 01 cmd Add\nentity Cart\nnote 01 {\n  hello\n}\ngwt 01 given\n evt Cart\nthen\n evt Cart\ntf 02 evt Added").unwrap();
        assert_eq!(m.frames.len(), 2);
        assert_eq!(m.entities, ["Cart"]);
    }
}
