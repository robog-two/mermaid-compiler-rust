//! `Scene` → PDF 1.4, written by hand (no dependencies).
//!
//! Text uses the PDF standard Helvetica fonts (always available, never embedded),
//! whose metrics are the same ones the layout measured with, so text placement
//! matches the SVG. Characters outside Windows-1252 are replaced with `?`.

use crate::color::{self, Rgba};
use crate::geom::{PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, Element, Geometry, Paint, Scene, Text};
use crate::text::line_width;
use std::fmt::Write as _;
use std::str::FromStr;

/// Paper sizes accepted by `--pdf-paper-format` (Puppeteer's, in inches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    Letter,
    Legal,
    Tabloid,
    Ledger,
    A0,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
}

impl Paper {
    pub const NAMES: [&'static str; 11] = [
        "Letter", "Legal", "Tabloid", "Ledger", "A0", "A1", "A2", "A3", "A4", "A5", "A6",
    ];

    /// Width and height in CSS pixels (96 per inch).
    pub fn size_px(self) -> (f64, f64) {
        let (w, h) = match self {
            Paper::Letter => (8.5, 11.0),
            Paper::Legal => (8.5, 14.0),
            Paper::Tabloid => (11.0, 17.0),
            Paper::Ledger => (17.0, 11.0),
            Paper::A0 => (33.1, 46.8),
            Paper::A1 => (23.4, 33.1),
            Paper::A2 => (16.54, 23.4),
            Paper::A3 => (11.7, 16.54),
            Paper::A4 => (8.27, 11.7),
            Paper::A5 => (5.83, 8.27),
            Paper::A6 => (4.13, 5.83),
        };
        (w * 96.0, h * 96.0)
    }
}

impl FromStr for Paper {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s {
            "Letter" => Paper::Letter,
            "Legal" => Paper::Legal,
            "Tabloid" => Paper::Tabloid,
            "Ledger" => Paper::Ledger,
            "A0" => Paper::A0,
            "A1" => Paper::A1,
            "A2" => Paper::A2,
            "A3" => Paper::A3,
            "A4" => Paper::A4,
            "A5" => Paper::A5,
            "A6" => Paper::A6,
            _ => return Err(()),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfOptions {
    pub background: String,
    /// Rendered diagram size in CSS pixels.
    pub width: f64,
    pub height: f64,
    pub paper: Option<Paper>,
}

/// Puppeteer's page body margin, which mmdc's PDF output includes.
const BODY_MARGIN_PX: f64 = 8.0;
const PX_TO_PT: f64 = 0.75;

struct Ctx {
    out: String,
    alphas: Vec<u32>,
}

fn n(v: f64) -> String {
    crate::output::svg::num(v)
}

impl Ctx {
    fn gs_for(&mut self, alpha: f64) -> Option<String> {
        if alpha >= 0.999 {
            return None;
        }
        let key = (alpha.clamp(0.0, 1.0) * 1000.0).round() as u32;
        let idx = match self.alphas.iter().position(|&a| a == key) {
            Some(i) => i,
            None => {
                self.alphas.push(key);
                self.alphas.len() - 1
            }
        };
        Some(format!("/GS{idx} gs\n"))
    }

    fn path(&mut self, cmds: &[PathCmd]) {
        for c in cmds {
            let _ = match c {
                PathCmd::MoveTo(p) => writeln!(self.out, "{} {} m", n(p.x), n(p.y)),
                PathCmd::LineTo(p) => writeln!(self.out, "{} {} l", n(p.x), n(p.y)),
                PathCmd::CubicTo(a, b, p) => writeln!(
                    self.out,
                    "{} {} {} {} {} {} c",
                    n(a.x),
                    n(a.y),
                    n(b.x),
                    n(b.y),
                    n(p.x),
                    n(p.y)
                ),
                PathCmd::Close => writeln!(self.out, "h"),
            };
        }
    }

    fn shape(&mut self, geom: &Geometry, paint: &Paint) {
        let fill = resolve(&paint.fill);
        let stroke = if paint.stroke_width > 0.0 {
            resolve(&paint.stroke)
        } else {
            None
        };
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.out.push_str("q\n");
        let opacity = paint.opacity.unwrap_or(1.0);
        let fa = fill.map(|c| c.a * opacity).unwrap_or(1.0);
        let sa = stroke.map(|c| c.a * opacity).unwrap_or(1.0);
        // One ExtGState per distinct alpha; when fill and stroke differ use the smaller.
        if let Some(g) = self.gs_for(fa.min(sa)) {
            self.out.push_str(&g);
        }
        if let Some(c) = fill {
            let (r, g, b) = c.unit();
            let _ = writeln!(self.out, "{} {} {} rg", n(r), n(g), n(b));
        }
        if let Some(c) = stroke {
            let (r, g, b) = c.unit();
            let _ = writeln!(
                self.out,
                "{} {} {} RG {} w",
                n(r),
                n(g),
                n(b),
                n(paint.stroke_width)
            );
            if let Some(d) = &paint.dash {
                let _ = writeln!(
                    self.out,
                    "[{}] 0 d",
                    d.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ")
                );
            }
        }
        match geom {
            Geometry::Rect { rect, rx } => self.path(&rounded_rect(rect, *rx)),
            Geometry::Circle { c, r } => self.path(&crate::geom::ellipse(*c, *r, *r)),
            Geometry::Path(cmds) => self.path(cmds),
        }
        let op = match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B",
            (true, false) => "f",
            (false, true) => "S",
            (false, false) => "n",
        };
        let _ = writeln!(self.out, "{op}\nQ");
    }

    fn text(&mut self, t: &Text) {
        let Some(c) = resolve(&t.fill) else { return };
        let (r, g, b) = c.unit();
        let font = if t.bold { "F2" } else { "F1" };
        self.out.push_str("q\n");
        if let Some(gs) = self.gs_for(c.a) {
            self.out.push_str(&gs);
        }
        let _ = writeln!(self.out, "{} {} {} rg", n(r), n(g), n(b));
        for (i, line) in t.lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let w = line_width(line, t.font_size, t.bold);
            let x = match t.anchor {
                Anchor::Start => t.x,
                Anchor::Middle => t.x - w / 2.0,
                Anchor::End => t.x - w,
            };
            let y = t.y + i as f64 * t.line_height;
            // The page CTM flips y; flip glyphs back with a negative d in Tm.
            let _ = writeln!(
                self.out,
                "BT /{font} 1 Tf {} 0 0 {} {} {} Tm ({}) Tj ET",
                n(t.font_size),
                n(-t.font_size),
                n(x),
                n(y),
                pdf_string(line)
            );
        }
        self.out.push_str("Q\n");
    }

    fn element(&mut self, el: &Element) {
        match el {
            Element::Group { children, .. } => children.iter().for_each(|c| self.element(c)),
            Element::Shape { geom, paint, .. } => self.shape(geom, paint),
            Element::Text { text, .. } => self.text(text),
            Element::Rotated {
                degrees,
                origin,
                children,
            } => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                let (ox, oy) = (origin.x, origin.y);
                let _ = writeln!(
                    self.out,
                    "q {} {} {} {} {} {} cm",
                    n(cos),
                    n(sin),
                    n(-sin),
                    n(cos),
                    n(ox - ox * cos + oy * sin),
                    n(oy - ox * sin - oy * cos)
                );
                children.iter().for_each(|c| self.element(c));
                self.out.push_str("Q\n");
            }
        }
    }
}

fn resolve(c: &Color) -> Option<Rgba> {
    if c.is_none() {
        return None;
    }
    color::parse(c.css()).filter(|c| c.a > 0.0)
}

fn rounded_rect(r: &Rect, rx: f64) -> Vec<PathCmd> {
    let rx = rx.min(r.w / 2.0).min(r.h / 2.0);
    if rx <= 0.0 {
        return crate::geom::polygon(&[
            Point::new(r.x, r.y),
            Point::new(r.right(), r.y),
            Point::new(r.right(), r.bottom()),
            Point::new(r.x, r.bottom()),
        ]);
    }
    use std::f64::consts::{FRAC_PI_2, PI};
    let mut c = vec![
        PathCmd::MoveTo(Point::new(r.x + rx, r.y)),
        PathCmd::LineTo(Point::new(r.right() - rx, r.y)),
    ];
    c.extend(crate::geom::arc_cubics(
        Point::new(r.right() - rx, r.y + rx),
        rx,
        -FRAC_PI_2,
        0.0,
    ));
    c.push(PathCmd::LineTo(Point::new(r.right(), r.bottom() - rx)));
    c.extend(crate::geom::arc_cubics(
        Point::new(r.right() - rx, r.bottom() - rx),
        rx,
        0.0,
        FRAC_PI_2,
    ));
    c.push(PathCmd::LineTo(Point::new(r.x + rx, r.bottom())));
    c.extend(crate::geom::arc_cubics(
        Point::new(r.x + rx, r.bottom() - rx),
        rx,
        FRAC_PI_2,
        PI,
    ));
    c.push(PathCmd::LineTo(Point::new(r.x, r.y + rx)));
    c.extend(crate::geom::arc_cubics(
        Point::new(r.x + rx, r.y + rx),
        rx,
        PI,
        PI + FRAC_PI_2,
    ));
    c.push(PathCmd::Close);
    c
}

/// Encode text as a PDF literal string in WinAnsiEncoding.
fn pdf_string(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let byte: u8 = match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                ch as u8
            }
            c if (' '..='~').contains(&c) => c as u8,
            '\u{a0}'..='\u{ff}' => ch as u32 as u8,
            '€' => 0x80,
            '‚' => 0x82,
            '„' => 0x84,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '™' => 0x99,
            _ => b'?',
        };
        if byte >= 0x80 {
            let _ = write!(out, "\\{byte:03o}");
        } else {
            out.push(byte as char);
        }
    }
    out
}

/// Serialise a scene to a single-page PDF.
pub fn to_pdf(scene: &Scene, opt: &PdfOptions) -> Vec<u8> {
    let (page_w_px, page_h_px, draw_w) = match opt.paper {
        None => (
            (opt.width.ceil() + 2.0 * BODY_MARGIN_PX),
            (opt.height.ceil() + 2.0 * BODY_MARGIN_PX),
            opt.width,
        ),
        Some(p) => {
            let (pw, ph) = p.size_px();
            (pw, ph, opt.width.min(pw - 2.0 * BODY_MARGIN_PX))
        }
    };
    let page_w = page_w_px * PX_TO_PT;
    let page_h = page_h_px * PX_TO_PT;
    let vb = scene.view_box;
    let k = if vb.w > 0.0 { draw_w / vb.w } else { 1.0 };
    let a = k * PX_TO_PT;
    let mut ctx = Ctx {
        out: String::new(),
        alphas: Vec::new(),
    };

    // Page background (Puppeteer prints the body background) unless transparent.
    if let Some(bg) = color::parse(&opt.background).filter(|c| c.a > 0.0) {
        let (r, g, b) = bg.unit();
        let _ = writeln!(
            ctx.out,
            "{} {} {} rg 0 0 {} {} re f",
            n(r),
            n(g),
            n(b),
            n(page_w),
            n(page_h)
        );
    }
    let m = BODY_MARGIN_PX * PX_TO_PT;
    let _ = writeln!(
        ctx.out,
        "q {} 0 0 {} {} {} cm",
        n(a),
        n(-a),
        n(m - vb.x * a),
        n(page_h - m + vb.y * a)
    );
    for el in &scene.elements {
        ctx.element(el);
    }
    ctx.out.push_str("Q\n");

    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into());
    let gs_base = 7;
    let ext: String = (0..ctx.alphas.len())
        .map(|i| format!("/GS{i} {} 0 R ", gs_base + i))
        .collect();
    objects.push(format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /ExtGState << {ext}>> >> >>",
        n(page_w),
        n(page_h)
    ));
    objects.push(format!(
        "<< /Length {} >>\nstream\n{}endstream",
        ctx.out.len(),
        ctx.out
    ));
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    );
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .into(),
    );
    for &alpha in &ctx.alphas {
        let v = n(alpha as f64 / 1000.0);
        objects.push(format!("<< /Type /ExtGState /ca {v} /CA {v} >>"));
    }

    let mut pdf: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for o in offsets {
        let _ = writeln!(tail, "{o:010} 00000 n ");
    }
    let _ = write!(
        tail,
        "trailer\n<< /Size {} /Root 1 0 R /Producer (mmdc-rs) >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    pdf.extend_from_slice(tail.as_bytes());
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_escaping() {
        assert_eq!(pdf_string("a(b)\\"), "a\\(b\\)\\\\");
        assert_eq!(pdf_string("é"), "\\351");
        assert_eq!(pdf_string("中"), "?");
    }

    #[test]
    fn paper_names_parse() {
        for name in Paper::NAMES {
            assert!(name.parse::<Paper>().is_ok());
        }
    }
}
