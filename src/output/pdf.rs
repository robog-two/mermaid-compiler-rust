//! `Scene` → PDF 1.4, written by hand (no dependencies).
//!
//! Text uses the PDF standard Helvetica fonts (always available, never embedded),
//! whose metrics are the same ones the layout measured with, so text placement
//! matches the SVG. Characters outside Windows-1252 are replaced with `?`.

use crate::color::{self, Rgba};
use crate::geom::{PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, Element, Geometry, Gradient, Paint, Scene, Text};
use crate::text::line_width;
use std::collections::BTreeMap;
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
    /// Graphics states: alpha in thousandths, and whether the blend mode is
    /// `Multiply`.
    alphas: Vec<(u32, bool)>,
    /// While drawing a text outline: its width in user space. Glyphs are then
    /// stroked (render mode 1) instead of filled.
    text_outline: Option<f64>,
    /// Glyphs drawn with each embedded TeX Gyre Heros face, with their characters.
    embedded: [BTreeMap<u16, char>; 4],
    /// Stroke gradients, drawn as axial shading patterns `/P{index}`.
    gradients: Vec<Gradient>,
    /// Blurred drop shadows: image XObjects `/Sh{index}` in the shadow colour
    /// through their blurred-silhouette soft masks.
    shadows: Vec<(Rgba, super::shadow::AlphaImage)>,
}

fn n(v: f64) -> String {
    crate::output::svg::num(v)
}

impl Ctx {
    fn gs_for(&mut self, alpha: f64, multiply: bool) -> Option<String> {
        if alpha >= 0.999 && !multiply {
            return None;
        }
        let key = ((alpha.clamp(0.0, 1.0) * 1000.0).round() as u32, multiply);
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
        for c in crate::geom::flatten_arcs(cmds).iter() {
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
                // Flattened above.
                PathCmd::MoveBy(_) | PathCmd::ArcTo { .. } => Ok(()),
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
        // A browser does not draw a path whose data stops at a NaN (mermaid's
        // edge with an arrowhead on a zero-length end segment).
        let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();
        if let Geometry::Path(cmds) = geom
            && !cmds.iter().all(|c| match c {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) => finite(p),
                PathCmd::CubicTo(a, b, p) => finite(a) && finite(b) && finite(p),
                PathCmd::Close => true,
                PathCmd::MoveBy(p) => finite(p),
                PathCmd::ArcTo { rx, ry, to, .. } => rx.is_finite() && ry.is_finite() && finite(to),
            })
        {
            return;
        }
        if fill.is_none() && stroke.is_none() {
            return;
        }
        // A bounding-box gradient in user space; one with no box to span paints
        // nothing, as in SVG.
        let grad = paint
            .stroke_gradient
            .as_ref()
            .map(|g| g.in_user_space(geom));
        let stroke = match grad {
            Some(None) => None,
            _ => stroke,
        };
        let grad = grad.flatten();
        if let Some(sh) = &paint.shadow {
            self.shadow(geom, paint, fill, stroke, sh);
        }
        // A fill-only opacity: fill, then stroke over it, each with its own alpha.
        if let (Some(_), Some(_), Some(_)) = (paint.fill_opacity, fill, stroke) {
            let mut f = paint.clone();
            f.stroke = crate::scene::Color::none();
            f.shadow = None;
            self.shape(geom, &f);
            let mut s = paint.clone();
            s.fill = crate::scene::Color::none();
            s.fill_opacity = None;
            s.shadow = None;
            s.stroke_gradient = grad;
            self.shape(geom, &s);
            return;
        }
        self.out.push_str("q\n");
        let opacity = paint.opacity.unwrap_or(1.0);
        let fa = fill
            .map(|c| c.a * opacity * paint.fill_opacity.unwrap_or(1.0))
            .unwrap_or(1.0);
        let sa = stroke.map(|c| c.a * opacity).unwrap_or(1.0);
        // One ExtGState per distinct alpha; when fill and stroke differ use the smaller.
        if let Some(g) = self.gs_for(fa.min(sa), paint.multiply) {
            self.out.push_str(&g);
        }
        if let Some(c) = fill {
            let (r, g, b) = c.unit();
            let _ = writeln!(self.out, "{} {} {} rg", n(r), n(g), n(b));
        }
        if let Some(c) = stroke {
            if let Some(grad) = &grad {
                let k = match self.gradients.iter().position(|x| x == grad) {
                    Some(k) => k,
                    None => {
                        self.gradients.push(grad.clone());
                        self.gradients.len() - 1
                    }
                };
                let _ = writeln!(
                    self.out,
                    "/Pattern CS /P{k} SCN {} w",
                    n(paint.stroke_width)
                );
            } else {
                let (r, g, b) = c.unit();
                let _ = writeln!(
                    self.out,
                    "{} {} {} RG {} w",
                    n(r),
                    n(g),
                    n(b),
                    n(paint.stroke_width)
                );
            }
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
            Geometry::Line { a, b } => self.path(&Geometry::line_path(*a, *b)),
        }
        let op = match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B",
            (true, false) => "f",
            (false, true) => "S",
            (false, false) => "n",
        };
        let _ = writeln!(self.out, "{op}\nQ");
    }

    /// A CSS `drop-shadow()`: the shape's silhouette (fill and stroke alpha)
    /// blurred into a soft mask, offset, painted in the shadow colour under the
    /// shape. The shape's `opacity` fades its shadow with it.
    fn shadow(
        &mut self,
        geom: &Geometry,
        paint: &Paint,
        fill: Option<Rgba>,
        stroke: Option<Rgba>,
        sh: &crate::scene::Shadow,
    ) {
        let Some(c) = resolve(&sh.color) else { return };
        let cmds = match geom {
            Geometry::Rect { rect, rx } => rounded_rect(rect, *rx),
            Geometry::Circle { c, r } => crate::geom::ellipse(*c, *r, *r),
            Geometry::Path(cmds) => cmds.clone(),
            Geometry::Line { a, b } => Geometry::line_path(*a, *b),
        };
        let subpaths = super::shadow::flatten(&cmds);
        let fill_a = fill.map_or(0.0, |f| f.a * paint.fill_opacity.unwrap_or(1.0));
        let stroke_a = stroke.map_or(0.0, |s| s.a);
        let Some(img) =
            super::shadow::silhouette(&subpaths, fill_a, stroke_a, paint.stroke_width, sh.std_dev)
        else {
            return;
        };
        let r = img.rect;
        let k = self.shadows.len();
        self.shadows.push((c, img));
        self.out.push_str("q\n");
        if let Some(g) = self.gs_for(c.a * paint.opacity.unwrap_or(1.0), paint.multiply) {
            self.out.push_str(&g);
        }
        // The image's unit square, its first row at the top (the page CTM flips y).
        let _ = writeln!(
            self.out,
            "{} 0 0 {} {} {} cm /Sh{k} Do\nQ",
            n(r.w),
            n(-r.h),
            n(r.x + sh.dx),
            n(r.y + sh.dy + r.h)
        );
    }

    /// A text block; with `outline`, each glyph is first stroked `width` wide in
    /// that colour with round joins, then filled over it (`paint-order: stroke`).
    fn text(&mut self, t: &Text, outline: Option<(&crate::scene::Color, f64)>) {
        let Some(c) = resolve(&t.fill) else { return };
        let (r, g, b) = c.unit();
        self.out.push_str("q\n");
        let outline = outline.and_then(|(oc, w)| resolve(oc).map(|oc| (oc, w)));
        let alpha = outline.map_or(c.a, |(oc, _)| c.a.min(oc.a));
        if let Some(gs) = self.gs_for(alpha, false) {
            self.out.push_str(&gs);
        }
        let _ = writeln!(self.out, "{} {} {} rg", n(r), n(g), n(b));
        if let Some((oc, w)) = outline {
            let (r, g, b) = oc.unit();
            // Render mode 2 fills then strokes; mode 1 strokes alone. Stroking
            // first and filling after (one text object each) keeps the stroke
            // under the fill.
            let _ = writeln!(self.out, "{} {} {} RG 1 j 1 J", n(r), n(g), n(b));
            self.text_outline = Some(w);
            self.text_lines(t);
            self.text_outline = None;
            self.out.push_str("0 Tr\n");
        }
        self.text_lines(t);
        self.out.push_str("Q\n");
    }

    fn text_lines(&mut self, t: &Text) {
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
            if crate::icons::has_icons(line) {
                self.line_with_icons(line, x, y, t);
                continue;
            }
            self.line(line, x, y, t.font_size, t.bold);
        }
    }

    /// A display line with Font Awesome icons: its text runs where the layout put
    /// them and each icon's outline filled in the text colour.
    fn line_with_icons(&mut self, line: &str, x0: f64, y: f64, t: &Text) {
        for piece in crate::icons::line_pieces(line, t.font_size, t.bold) {
            match piece {
                crate::icons::Piece::Text { x, text } => {
                    if !crate::text::plain(&text).is_empty() {
                        self.line(&text, x0 + x, y, t.font_size, t.bold);
                    }
                }
                crate::icons::Piece::Icon { x, icon } => {
                    let d = crate::icons::outline(icon);
                    if d.is_empty() {
                        continue;
                    }
                    let (tx, ty, s) = crate::icons::placement(icon, x0 + x, y, t.font_size);
                    let _ = writeln!(self.out, "q {s:.7} 0 0 {s:.7} {} {} cm", n(tx), n(ty));
                    self.path(&d);
                    // Outline pass: stroke the icon (the pen is not scaled with it).
                    if let Some(w) = self.text_outline {
                        let _ = writeln!(self.out, "{} w", n(w / s));
                        self.out.push_str("S\nQ\n");
                    } else {
                        self.out.push_str("f\nQ\n");
                    }
                }
            }
        }
    }

    /// One line of text starting at `x` on baseline `y`: one text object, a font
    /// switch per run of emphasis, and the bundled TeX Gyre Heros for characters
    /// the standard Helvetica fonts lack ([`crate::fonts`]).
    fn line(&mut self, line: &str, x: f64, y: f64, font_size: f64, bold: bool) {
        // The page CTM flips y; flip glyphs back with a negative d in Tm.
        let _ = write!(
            self.out,
            "BT {} 0 0 {} {} {} Tm",
            n(font_size),
            n(-font_size),
            n(x),
            n(y),
        );
        // The pen width is in text space, which `Tm` scales by the font size.
        if let Some(w) = self.text_outline {
            let _ = write!(self.out, " 1 Tr {} w", n(w / font_size));
        }
        for run in crate::text::styled_runs(line, bold) {
            let face = crate::fonts::face_index(run.bold, run.italic);
            let mut rest = run.text.as_str();
            while !rest.is_empty() {
                let embedded = |c: char| win_ansi(c).is_none() && embedded_glyph(face, c).is_some();
                let split =
                    |p: &dyn Fn(char) -> bool| rest.find(|c: char| !p(c)).unwrap_or(rest.len());
                if embedded(rest.chars().next().unwrap_or(' ')) {
                    let end = split(&embedded);
                    let mut hex = String::new();
                    for c in rest[..end].chars() {
                        if let Some(g) = embedded_glyph(face, c) {
                            self.embedded[face].insert(g, c);
                            let _ = write!(hex, "{g:04X}");
                        }
                    }
                    let _ = write!(self.out, " /H{} 1 Tf <{hex}> Tj", face + 1);
                    rest = &rest[end..];
                } else {
                    let end = split(&|c| !embedded(c));
                    let _ = write!(
                        self.out,
                        " /F{} 1 Tf ({}) Tj",
                        face + 1,
                        pdf_string(&rest[..end])
                    );
                    rest = &rest[end..];
                }
            }
        }
        self.out.push_str(" ET\n");
    }

    fn element(&mut self, el: &Element) {
        match el {
            Element::Group { children, .. } => children.iter().for_each(|c| self.element(c)),
            Element::Clipped { rect, children } => {
                let _ = writeln!(
                    self.out,
                    "q\n{} {} {} {} re W n",
                    n(rect.x),
                    n(rect.y),
                    n(rect.w),
                    n(rect.h)
                );
                children.iter().for_each(|c| self.element(c));
                self.out.push_str("Q\n");
            }
            Element::Shape { geom, paint, .. } => self.shape(geom, paint),
            Element::Text { text, .. } => self.text(text, None),
            Element::OutlinedText {
                text, color, width, ..
            } => self.text(text, Some((color, *width))),
            Element::Translated { dx, dy, children } => {
                let _ = writeln!(self.out, "q\n1 0 0 1 {} {} cm", n(*dx), n(*dy));
                children.iter().for_each(|c| self.element(c));
                self.out.push_str("Q\n");
            }
            Element::Rotated {
                deg,
                about,
                children,
            } => {
                // Same matrix as SVG `rotate(deg cx cy)`; the page CTM already flips y.
                let (sin, cos) = deg.to_radians().sin_cos();
                let e = about.x - cos * about.x + sin * about.y;
                let f = about.y - sin * about.x - cos * about.y;
                let _ = writeln!(
                    self.out,
                    "q\n{} {} {} {} {} {} cm",
                    n(cos),
                    n(sin),
                    n(-sin),
                    n(cos),
                    n(e),
                    n(f)
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

/// A character's code in WinAnsiEncoding (Windows-1252), if it has one.
fn win_ansi(ch: char) -> Option<u8> {
    Some(match ch {
        ' '..='~' | '\u{a0}'..='\u{ff}' => ch as u32 as u8,
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    })
}

/// The bundled face's glyph for a character (none without the `fonts` feature).
fn embedded_glyph(face: usize, c: char) -> Option<u16> {
    crate::fonts::face(face)?.glyph(c)
}

/// Encode text as a PDF literal string in WinAnsiEncoding (`?` for characters it
/// lacks).
fn pdf_string(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let byte = win_ansi(ch).unwrap_or(b'?');
        if matches!(byte, b'(' | b')' | b'\\') {
            out.push('\\');
        }
        if byte >= 0x80 {
            let _ = write!(out, "\\{byte:03o}");
        } else {
            out.push(byte as char);
        }
    }
    out
}

/// The PDF objects that embed one TeX Gyre Heros face, numbered from `first`: a
/// Type0 font over an OpenType CIDFont (CIDs are glyph ids, `Identity-H`), its
/// descriptor, the font file, and a ToUnicode map so the text can be copied.
fn embedded_font_objects(face: usize, glyphs: &BTreeMap<u16, char>, first: usize) -> Vec<Vec<u8>> {
    let Some(f) = crate::fonts::face(face) else {
        return Vec::new();
    };
    let name = crate::fonts::POSTSCRIPT_NAMES[face];
    let scale = 1000.0 / f64::from(f.units_per_em);
    let widths: String = glyphs
        .keys()
        .map(|&g| format!("{g} [{}] ", n(f64::from(f.advance(g)) * scale)))
        .collect();
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
         1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
    );
    let entries: Vec<_> = glyphs.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (g, c) in chunk {
            let mut units = [0u16; 2];
            let hex: String = c
                .encode_utf16(&mut units)
                .iter()
                .map(|u| format!("{u:04X}"))
                .collect();
            let _ = writeln!(cmap, "<{g:04X}> <{hex}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end\n");
    let flags = 32 + if f.italic_angle != 0.0 { 64 } else { 0 };
    let [x0, y0, x1, y1] = f.bbox.map(|v| n(f64::from(v) * scale));
    let mut file = format!(
        "<< /Subtype /OpenType /Length {} >>\nstream\n",
        f.data.len()
    )
    .into_bytes();
    file.extend_from_slice(f.data);
    file.extend_from_slice(b"\nendstream");
    vec![
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>",
            first + 1,
            first + 4
        )
        .into_bytes(),
        format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R /DW 0 /W [{widths}] >>",
            first + 2
        )
        .into_bytes(),
        format!(
            "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} /FontBBox [{x0} {y0} {x1} {y1}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile3 {} 0 R >>",
            n(f.italic_angle),
            n(f64::from(f.ascent) * scale),
            n(f64::from(f.descent) * scale),
            n(f64::from(f.cap_height) * scale),
            first + 3
        )
        .into_bytes(),
        file,
        format!("<< /Length {} >>\nstream\n{cmap}endstream", cmap.len()).into_bytes(),
    ]
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
        text_outline: None,
        embedded: Default::default(),
        gradients: Vec::new(),
        shadows: Vec::new(),
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
    let base = [a, 0.0, 0.0, -a, m - vb.x * a, page_h - m + vb.y * a];
    let _ = writeln!(
        ctx.out,
        "q {} 0 0 {} {} {} cm",
        n(base[0]),
        n(base[3]),
        n(base[4]),
        n(base[5])
    );
    for el in &scene.elements {
        ctx.element(el);
    }
    ctx.out.push_str("Q\n");

    let mut objects: Vec<Vec<u8>> = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    let gs_base = 9;
    let ext: String = (0..ctx.alphas.len())
        .map(|i| format!("/GS{i} {} 0 R ", gs_base + i))
        .collect();
    // Embedded faces follow the graphics states, five objects each.
    let mut embedded = String::new();
    let mut font_objects = Vec::new();
    for (face, glyphs) in ctx.embedded.iter().enumerate() {
        if glyphs.is_empty() {
            continue;
        }
        let first = gs_base + ctx.alphas.len() + font_objects.len();
        let objs = embedded_font_objects(face, glyphs, first);
        if !objs.is_empty() {
            let _ = write!(embedded, " /H{} {first} 0 R", face + 1);
            font_objects.extend(objs);
        }
    }
    // Shading patterns follow the embedded faces, one object each. A pattern's
    // space is the page's default space, so its matrix is the diagram's `cm`.
    let pattern_base = gs_base + ctx.alphas.len() + font_objects.len();
    let patterns: String = (0..ctx.gradients.len())
        .map(|i| format!(" /P{i} {} 0 R", pattern_base + i))
        .collect();
    let patterns = if patterns.is_empty() {
        String::new()
    } else {
        format!(" /Pattern <<{patterns} >>")
    };
    // Shadow images follow the patterns, two objects each (the image, its mask).
    let shadow_base = pattern_base + ctx.gradients.len();
    let xobjects: String = (0..ctx.shadows.len())
        .map(|i| format!(" /Sh{i} {} 0 R", shadow_base + 2 * i))
        .collect();
    let patterns = if xobjects.is_empty() {
        patterns
    } else {
        format!("{patterns} /XObject <<{xobjects} >>")
    };
    objects.push(format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 8 0 R{embedded} >> /ExtGState << {ext}>>{patterns} >> >>",
        n(page_w),
        n(page_h)
    ).into_bytes());
    objects.push(
        format!(
            "<< /Length {} >>\nstream\n{}endstream",
            ctx.out.len(),
            ctx.out
        )
        .into_bytes(),
    );
    for face in [
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-Oblique",
        "Helvetica-BoldOblique",
    ] {
        objects.push(
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{face} /Encoding /WinAnsiEncoding >>"
            )
            .into_bytes(),
        );
    }
    for &(alpha, multiply) in &ctx.alphas {
        let v = n(alpha as f64 / 1000.0);
        let bm = if multiply { " /BM /Multiply" } else { "" };
        objects.push(format!("<< /Type /ExtGState /ca {v} /CA {v}{bm} >>").into_bytes());
    }
    let version = if font_objects.is_empty() {
        "1.4"
    } else {
        // An OpenType font file (`FontFile3 /OpenType`) needs PDF 1.6.
        "1.6"
    };
    objects.extend(font_objects);
    for g in &ctx.gradients {
        let rgb = |c: &Color| {
            let (r, g, b) = resolve(c).map_or((0.0, 0.0, 0.0), |c| c.unit());
            format!("{} {} {}", n(r), n(g), n(b))
        };
        objects.push(
            format!(
                "<< /PatternType 2 /Matrix [{}] /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [{} 0 {} 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [{}] /C1 [{}] /N 1 >> /Extend [true true] >> >>",
                base.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" "),
                n(g.x1),
                n(g.x2),
                rgb(&g.from),
                rgb(&g.to)
            )
            .into_bytes(),
        );
    }

    // A 1×1 image of the shadow colour whose soft mask (any size) is the blurred
    // silhouette: the mask's alpha over the image's unit square.
    for (i, (c, img)) in ctx.shadows.iter().enumerate() {
        let mask_obj = shadow_base + 2 * i + 1;
        let mut base = format!(
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mask_obj} 0 R /Length 3 >>\nstream\n"
        )
        .into_bytes();
        base.extend_from_slice(&[c.r, c.g, c.b]);
        base.extend_from_slice(b"\nendstream");
        objects.push(base);
        let data = super::shadow::run_length(&img.alpha);
        let mut mask = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true /Filter /RunLengthDecode /Length {} >>\nstream\n",
            img.w,
            img.h,
            data.len()
        )
        .into_bytes();
        mask.extend_from_slice(&data);
        mask.extend_from_slice(b"\nendstream");
        objects.push(mask);
    }

    let mut pdf: Vec<u8> = format!("%PDF-{version}\n").into_bytes();
    pdf.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(obj);
        pdf.extend_from_slice(b"\nendobj\n");
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
    fn gradient_strokes_become_axial_shading_patterns() {
        use crate::scene::{DiagramKind, Gradient};
        let paint = Paint::stroke(&Color::new("#4e79a7"), 20.0).with_stroke_gradient(Gradient {
            x1: 10.0,
            x2: 590.0,
            from: Color::new("#4e79a7"),
            to: Color::new("#e15759"),
            bbox: false,
        });
        let scene = Scene {
            kind: DiagramKind::Sankey,
            view_box: Rect::new(0.0, 0.0, 600.0, 400.0),
            elements: vec![Element::line(
                Point::new(10.0, 50.0),
                Point::new(590.0, 60.0),
                paint,
            )],
            acc_title: None,
            acc_descr: None,
            font_family: "sans-serif".into(),
            css: String::new(),
        };
        let opt = PdfOptions {
            background: "white".into(),
            width: 600.0,
            height: 400.0,
            paper: None,
        };
        let pdf = String::from_utf8_lossy(&to_pdf(&scene, &opt)).into_owned();
        assert!(pdf.contains("/Pattern CS /P0 SCN 20 w"), "{pdf}");
        assert!(pdf.contains("/Pattern << /P0 9 0 R >>"), "{pdf}");
        assert!(pdf.contains("/ShadingType 2 /ColorSpace /DeviceRGB /Coords [10 0 590 0]"));
        // Pattern space is the page's: the diagram's own `cm`.
        assert!(pdf.contains("/Matrix [0.75 0 0 -0.75 6 306]"), "{pdf}");
    }

    /// What the scene's CSS does in a browser (`mix-blend-mode: multiply` on
    /// sankey links, `paint-order: stroke` label halos) the PDF draws itself.
    #[test]
    fn multiply_blend_and_outlined_text() {
        use crate::scene::{Anchor, DiagramKind, Text};
        let link = Paint::stroke(&Color::new("#4e79a7"), 20.0)
            .with_opacity(0.5)
            .with_multiply();
        let label = Text {
            x: 10.0,
            y: 20.0,
            lines: vec!["Coal 75.57".into()],
            line_height: 16.8,
            font_size: 14.0,
            bold: false,
            weight: None,
            anchor: Anchor::Start,
            fill: Color::new("#333"),
        };
        let scene = Scene {
            kind: DiagramKind::Sankey,
            view_box: Rect::new(0.0, 0.0, 600.0, 400.0),
            elements: vec![
                Element::line(Point::new(10.0, 50.0), Point::new(590.0, 60.0), link),
                Element::OutlinedText {
                    text: label,
                    class: Some("sankey-label-bg".into()),
                    color: Color::new("#ffffff"),
                    width: 4.0,
                },
            ],
            acc_title: None,
            acc_descr: None,
            font_family: "sans-serif".into(),
            css: String::new(),
        };
        let opt = PdfOptions {
            background: "white".into(),
            width: 600.0,
            height: 400.0,
            paper: None,
        };
        let pdf = String::from_utf8_lossy(&to_pdf(&scene, &opt)).into_owned();
        assert!(pdf.contains("<< /Type /ExtGState /ca 0.5 /CA 0.5 /BM /Multiply >>"));
        // Stroked first (4px in text space scaled by 1/14), round joins, then filled.
        let stroke = pdf.find("1 1 1 RG 1 j 1 J").expect("outline colour");
        let outline = pdf
            .find("1 Tr 0.286 w /F1 1 Tf (Coal 75.57) Tj")
            .expect("outline");
        let reset = pdf.find("0 Tr\n").expect("fill mode restored");
        let fill = pdf.rfind("/F1 1 Tf (Coal 75.57) Tj").expect("fill");
        assert!(stroke < outline && outline < reset && reset < fill, "{pdf}");
        // SVG output writes plain text; the halo there comes from the CSS.
        let svg = crate::output::svg::to_svg(
            &scene,
            &crate::output::svg::SvgOptions {
                id: "my-svg",
                background: "white",
                sizing: crate::output::svg::SvgSizing::Fixed {
                    width: 600.0,
                    height: 400.0,
                },
                theme_css: None,
            },
        );
        assert!(svg.contains("<text class=\"sankey-label-bg\""), "{svg}");
    }

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
