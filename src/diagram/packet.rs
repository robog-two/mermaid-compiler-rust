//! `packet` (`packet-beta`) diagrams: bit-field layouts of network packets.
//!
//! Syntax: `0-15: "Source Port"`, `16: "Flag"` (single bit), `+8: "Next"`
//! (relative bit count), `title ...`. Fields must be contiguous; a field that
//! crosses a row boundary is split like mermaid does. Rows are laid out with a
//! Taffy CSS grid of `bitsPerRow` equal tracks, every field spanning its bits.
//!
//! Config (`packet` section, mermaid defaults): `bitsPerRow` 32, `bitWidth` 32,
//! `rowHeight` 32, `paddingX` 5, `paddingY` 5 (+10 when `showBits`), `showBits` true.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{Bounds, Point, Rect};
use crate::layout::boxes::{self, BoxNode, taffy};
use crate::scene::{Anchor, DiagramKind, Element, Paint, Scene, Text};
use crate::text::{TextBlock, decode_entities};
use crate::theme::Palette;

/// Mermaid refuses to build more than this many words.
const MAX_ROWS: usize = 10_000;

/// One declared field (inclusive bit range).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub start: u64,
    pub end: u64,
    pub label: String,
}

/// Parsed packet diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub title: Option<String>,
    pub fields: Vec<Field>,
}

/// Layout parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacketConfig {
    pub bits_per_row: u64,
    pub bit_width: f64,
    pub row_height: f64,
    pub padding_x: f64,
    pub padding_y: f64,
    pub show_bits: bool,
}

impl PacketConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        let show_bits = cfg.section_bool("packet", "showBits", true);
        let padding_y = cfg.section_f64("packet", "paddingY", 5.0);
        PacketConfig {
            bits_per_row: cfg.section_f64("packet", "bitsPerRow", 32.0).max(1.0) as u64,
            bit_width: cfg.section_f64("packet", "bitWidth", 32.0).max(1.0),
            row_height: cfg.section_f64("packet", "rowHeight", 32.0).max(1.0),
            padding_x: cfg.section_f64("packet", "paddingX", 5.0),
            padding_y: if show_bits {
                padding_y + 10.0
            } else {
                padding_y
            },
            show_bits,
        }
    }
}

impl Default for PacketConfig {
    fn default() -> Self {
        PacketConfig {
            bits_per_row: 32,
            bit_width: 32.0,
            row_height: 32.0,
            padding_x: 5.0,
            padding_y: 15.0,
            show_bits: true,
        }
    }
}

/// Parse a packet body.
pub fn parse(body: &str, title: Option<String>) -> Result<Packet> {
    let mut p = Packet {
        title,
        fields: Vec::new(),
    };
    let mut header = false;
    let mut next_bit: u64 = 0;
    for (i, raw) in body.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !header {
            let rest = line
                .strip_prefix("packet-beta")
                .or_else(|| line.strip_prefix("packet"))
                .ok_or_else(|| Error::parse(line_no, "Expecting 'packet' or 'packet-beta'"))?;
            header = true;
            let rest = rest.trim().trim_start_matches(':').trim();
            if rest.is_empty() {
                continue;
            }
            return Err(Error::parse(
                line_no,
                format!("Unexpected text after 'packet': {rest}"),
            ));
        }
        if let Some(t) = line.strip_prefix("title") {
            if t.is_empty() || t.starts_with(char::is_whitespace) {
                p.title = Some(t.trim().to_string());
                continue;
            }
        }
        let Some((range, label)) = line.split_once(':') else {
            return Err(Error::parse(
                line_no,
                format!("{line}\nExpecting '<start>-<end>: \"label\"' or '+<bits>: \"label\"'"),
            ));
        };
        let label = parse_label(label.trim())
            .ok_or_else(|| Error::parse(line_no, format!("{line}\nExpecting a quoted label")))?;
        let range = range.trim();
        let num = |s: &str| -> Result<u64> {
            s.trim()
                .parse::<u64>()
                .map_err(|_| Error::parse(line_no, format!("{line}\nInvalid bit number '{s}'")))
        };
        let (start, end) = if let Some(bits) = range.strip_prefix('+') {
            let bits = num(bits)?;
            if bits == 0 {
                return Err(Error::parse(
                    line_no,
                    format!("Packet block {next_bit} is invalid. Cannot have a zero bit field."),
                ));
            }
            (next_bit, next_bit + bits - 1)
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (num(a)?, num(b)?);
            if b < a {
                return Err(Error::parse(
                    line_no,
                    format!("Packet block {a} - {b} is invalid. End must be greater than start."),
                ));
            }
            (a, b)
        } else {
            let a = num(range)?;
            (a, a)
        };
        if start != next_bit {
            return Err(Error::parse(
                line_no,
                format!(
                    "Packet block {start} - {end} is not contiguous. It should start from {next_bit}."
                ),
            ));
        }
        next_bit = end + 1;
        p.fields.push(Field { start, end, label });
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'packet'"));
    }
    Ok(p)
}

fn parse_label(s: &str) -> Option<String> {
    let q = s.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let inner = &s[1..];
    let close = inner.find(q)?;
    if !inner[close + 1..].trim().is_empty() {
        return None;
    }
    Some(decode_entities(&inner[..close]))
}

/// A field segment placed on one row.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub label: String,
}

/// Split fields into rows of `bits_per_row` bits (mermaid's `getNextFittingBlock`).
pub fn rows(p: &Packet, bits_per_row: u64) -> Vec<Vec<Segment>> {
    let bpr = bits_per_row.max(1);
    let mut rows: Vec<Vec<Segment>> = Vec::new();
    for f in &p.fields {
        let mut start = f.start;
        while start <= f.end && rows.len() < MAX_ROWS {
            let row = (start / bpr) as usize;
            let row_end = (row as u64 + 1) * bpr - 1;
            let end = f.end.min(row_end);
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            rows[row].push(Segment {
                start,
                end,
                label: f.label.clone(),
            });
            start = end + 1;
        }
    }
    rows
}

/// Placed segment rectangles per row (before `paddingX` is removed).
pub fn layout(rows: &[Vec<Segment>], pc: &PacketConfig) -> Vec<Vec<Rect>> {
    let bpr = pc.bits_per_row.max(1);
    let row_nodes: Vec<BoxNode> = rows
        .iter()
        .map(|row| {
            let cells = row
                .iter()
                .map(|s| {
                    let style = taffy::Style {
                        size: taffy::Size {
                            width: taffy::style_helpers::auto(),
                            height: taffy::style_helpers::length(pc.row_height as f32),
                        },
                        ..Default::default()
                    };
                    let col = (s.start % bpr) as i16 + 1;
                    let span = (s.end - s.start + 1).min(bpr) as u16;
                    BoxNode::new(boxes::grid_span(style, col, span), Vec::new())
                })
                .collect();
            BoxNode::new(boxes::uniform_grid(bpr as usize, pc.bit_width, 0.0), cells)
        })
        .collect();
    let mut col = boxes::column(pc.padding_y);
    col.padding.top = taffy::style_helpers::length(pc.padding_y as f32);
    let placed = boxes::compute(&BoxNode::new(col, row_nodes));
    placed
        .children
        .iter()
        .map(|r| r.children.iter().map(|c| c.rect).collect())
        .collect()
}

/// Render a packet diagram.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let p = parse(body, title)?;
    Ok(draw(&p, &PacketConfig::from_config(cfg), pal))
}

/// Draw a parsed packet.
pub fn draw(p: &Packet, pc: &PacketConfig, pal: &Palette) -> Scene {
    const LABEL_FONT: f64 = 12.0;
    const BYTE_FONT: f64 = 10.0;
    const TITLE_FONT: f64 = 14.0;
    let rows = rows(p, pc.bits_per_row);
    let rects = layout(&rows, pc);
    let mut els = Vec::new();
    let mut bounds = Bounds::default();
    let width = pc.bit_width * pc.bits_per_row as f64 + 2.0;
    bounds.add_rect(Rect::new(0.0, 0.0, width, pc.padding_y));
    let fill = pal.main_bkg.clone();
    let stroke = pal.node_border.clone();
    for (row, rr) in rows.iter().zip(&rects) {
        let mut g = Vec::new();
        for (seg, r) in row.iter().zip(rr) {
            let rect = Rect::new(r.x + 1.0, r.y, (r.w - pc.padding_x).max(1.0), r.h);
            bounds.add_rect(rect);
            g.push(Element::classed_shape(
                crate::scene::Geometry::Rect { rect, rx: 0.0 },
                Paint::both(&fill, &stroke, 1.0),
                "packetBlock",
            ));
            let tb = TextBlock::new(vec![seg.label.clone()], LABEL_FONT, false);
            g.push(Element::classed_text(
                Text::centered(
                    rect.center(),
                    &tb.lines,
                    LABEL_FONT,
                    tb.line_height,
                    &pal.text_color,
                ),
                "packetLabel",
            ));
            if pc.show_bits {
                let y = rect.y - 2.0;
                let single = seg.start == seg.end;
                let start = Text {
                    x: if single { rect.center().x } else { rect.x },
                    y,
                    lines: vec![seg.start.to_string()],
                    line_height: BYTE_FONT,
                    font_size: BYTE_FONT,
                    bold: false,
                    anchor: if single {
                        Anchor::Middle
                    } else {
                        Anchor::Start
                    },
                    fill: pal.text_color.clone(),
                };
                bounds.add_point(Point::new(rect.x, y - BYTE_FONT));
                g.push(Element::classed_text(start, "packetByte start"));
                if !single {
                    let end = Text {
                        x: rect.right(),
                        y,
                        lines: vec![seg.end.to_string()],
                        line_height: BYTE_FONT,
                        font_size: BYTE_FONT,
                        bold: false,
                        anchor: Anchor::End,
                        fill: pal.text_color.clone(),
                    };
                    g.push(Element::classed_text(end, "packetByte end"));
                }
            }
        }
        els.push(Element::group("packetRow", g));
    }
    let content_bottom = rects
        .iter()
        .flatten()
        .map(|r| r.bottom())
        .fold(pc.padding_y, f64::max);
    if let Some(t) = p.title.as_ref().filter(|t| !t.is_empty()) {
        let total_row = pc.row_height + pc.padding_y;
        let c = Point::new(width / 2.0, content_bottom + total_row / 2.0);
        let tb = TextBlock::new(vec![t.clone()], TITLE_FONT, false);
        bounds.add_rect(Rect::centered(c, tb.width, total_row));
        els.push(Element::classed_text(
            Text::centered(c, &tb.lines, TITLE_FONT, tb.line_height, &pal.title_color),
            "packetTitle",
        ));
    } else {
        bounds.add_point(Point::new(0.0, content_bottom + pc.padding_y));
    }
    Scene {
        kind: DiagramKind::Packet,
        view_box: bounds.rect().pad(8.0),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "packet-beta\ntitle TCP\n0-15: \"Source Port\"\n16-31: \"Destination Port\"\n32-63: \"Sequence Number\"\n+4: \"Offset\"\n68: \"Flag\"\n+50: \"Long\"";

    #[test]
    fn parses_ranges_and_relative_bits() {
        let p = parse(SRC, None).unwrap();
        assert_eq!(p.title.as_deref(), Some("TCP"));
        assert_eq!(p.fields.len(), 6);
        assert_eq!((p.fields[3].start, p.fields[3].end), (64, 67));
        assert_eq!((p.fields[4].start, p.fields[4].end), (68, 68));
        assert_eq!((p.fields[5].start, p.fields[5].end), (69, 118));
    }

    #[test]
    fn errors_report_lines() {
        let e = parse("packet\n0-10: \"a\"\n12-14: \"b\"", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("packet\n\n5-1: \"a\"", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("packet\n0-1: nope", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse("packet\n+0: \"z\"", None).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
    }

    #[test]
    fn splits_fields_across_rows() {
        let p = parse(SRC, None).unwrap();
        let r = rows(&p, 32);
        assert_eq!(r.len(), 4);
        // "Long" 69..118 is split at 95/96.
        assert_eq!(r[2].last().map(|s| (s.start, s.end)), Some((69, 95)));
        assert_eq!(r[3][0].start, 96);
        for row in &r {
            let bits: u64 = row.iter().map(|s| s.end - s.start + 1).sum();
            assert!(bits <= 32);
        }
    }

    #[test]
    fn grid_layout_matches_bit_positions() {
        let p = parse(SRC, None).unwrap();
        let pc = PacketConfig::default();
        let r = rows(&p, pc.bits_per_row);
        let rects = layout(&r, &pc);
        assert_eq!(rects[0][0].x, 0.0);
        assert_eq!(rects[0][0].w, 16.0 * 32.0);
        assert_eq!(rects[0][1].x, 16.0 * 32.0);
        assert_eq!(rects[0][0].y, pc.padding_y);
        assert_eq!(rects[1][0].y, 2.0 * pc.padding_y + pc.row_height);
        // Second-row "Flag" at bit 68 → column 4.
        assert_eq!(rects[2][1].x, 4.0 * 32.0);
        assert_eq!(rects[2][1].w, 32.0);
    }
}
