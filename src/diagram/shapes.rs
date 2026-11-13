//! Node-shape geometry for the graph-family renderers, transcribed from
//! mermaid's `rendering-util/rendering-elements/shapes/*.ts` (classic look).
//!
//! Sizing is split from drawing: [`ShapeMetrics`] carries every look-dependent
//! padding/factor explicitly, [`size`] turns a measured label into the node's
//! outer box, and [`outline`] draws the shape inside that box.

use super::graph::Shape;
use crate::config::Look;
use crate::geom::{self, PathCmd, Point, Rect};
use crate::scene::Geometry;
use std::f64::consts::PI;

/// Extra width / height a shape adds around its label (both sides together).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelPad {
    pub x: f64,
    pub y: f64,
}

/// Look-dependent sizing parameters of one node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeMetrics {
    pub pad: LabelPad,
    /// Raw `node.padding` (some shapes position their label with it).
    pub node_padding: f64,
    /// Hexagon point inset `m = h / hexagon_factor`.
    pub hexagon_factor: f64,
    /// Gap between the two rings of a double circle.
    pub double_circle_gap: f64,
    /// Offset of the back sheets of a stacked rectangle.
    pub stack_offset: f64,
    /// Document wave amplitude as a fraction of the body height.
    pub wave_ratio: f64,
    /// Corner radius of `Shape::Rounded` (`roundedRect.ts`: `themeVariables.radius ?? 5`).
    pub corner_radius: f64,
    /// `node.look === 'neo'` (shapes with look branches outside `pad`).
    pub neo: bool,
    /// Measured label size; shapes sized from their drawn bounding box rebuild
    /// their geometry from it in [`outline`]. Set with [`ShapeMetrics::with_label`].
    pub label: (f64, f64),
}

/// `subroutine.ts` / `shadedProcess.ts` FRAME_WIDTH.
const FRAME: f64 = 8.0;
/// `card.ts` NOTCH_SIZE.
const NOTCH: f64 = 12.0;
/// `windowPane.ts` rectOffset.
const PANE: f64 = 10.0;
/// `taggedRect.ts` TAG_RATIO.
const TAG_RATIO: f64 = 0.2;
/// `collapsedGroup.ts`: `node.padding` of a collapsed container.
pub const COLLAPSED_PAD: f64 = 8.0;
/// `collapsedGroup.ts`: SEPARATOR_GAP + INDICATOR_ROW_HEIGHT.
pub const COLLAPSED_ROWS: f64 = 8.0 + 20.0;

impl ShapeMetrics {
    /// Classic-look values (`node.look !== 'neo'`) for `shape` with `node.padding = p`.
    pub fn classic(shape: Shape, p: f64) -> Self {
        let pad = |x: f64, y: f64| LabelPad { x, y };
        let pad = match shape {
            // squareRect.ts: labelPaddingX = 2p on each side, labelPaddingY = p.
            // datastore.ts draws through drawRect with the same padding in every look.
            Shape::Rect | Shape::State | Shape::Datastore => pad(4.0 * p, 2.0 * p),
            Shape::Rounded | Shape::Note => pad(2.0 * p, 2.0 * p),
            // circle.ts: radius = labelRadius + p/2 (halfPadding).
            Shape::Circle => pad(p, p),
            // doubleCircle.ts: inner radius = labelRadius + p.
            Shape::DoubleCircle => pad(2.0 * p, 2.0 * p),
            Shape::InvTrapezoid => pad(2.0 * p, 2.0 * p),
            Shape::LinedRect
            | Shape::Delay
            | Shape::SlopedRect
            | Shape::WindowPane
            | Shape::NotchedPentagon
            | Shape::StackedRect
            | Shape::TaggedRect
            | Shape::Document
            | Shape::LinedDocument
            | Shape::LinedCylinder
            | Shape::CurvedTrapezoid => pad(2.0 * p, 2.0 * p),
            Shape::HorizontalCylinder => pad(p / 2.0, p / 2.0),
            _ => pad(p, p),
        };
        ShapeMetrics {
            pad,
            node_padding: p,
            hexagon_factor: 4.0,
            double_circle_gap: 5.0,
            stack_offset: 5.0,
            wave_ratio: 1.0 / 8.0,
            corner_radius: 5.0,
            neo: false,
            label: (0.0, 0.0),
        }
    }
}

impl ShapeMetrics {
    /// The same metrics remembering the measured label size.
    pub fn with_label(self, tw: f64, th: f64) -> Self {
        ShapeMetrics {
            label: (tw, th),
            ..self
        }
    }

    /// The metrics for `look` (`HandDrawn` sizes like `Classic`).
    pub fn for_look(shape: Shape, p: f64, look: Look) -> Self {
        match look {
            Look::Neo => Self::neo(shape, p),
            Look::Classic | Look::HandDrawn => Self::classic(shape, p),
        }
    }

    /// `node.look === 'neo'` values from the upstream shape files. `pad` holds the
    /// total extra width / height each shape adds around its label.
    pub fn neo(shape: Shape, p: f64) -> Self {
        let pad = |x: f64, y: f64| LabelPad { x, y };
        let classic = Self::classic(shape, p);
        let pad = match shape {
            // squareRect.ts: labelPaddingX 16, labelPaddingY 12, on each side.
            Shape::Rect | Shape::State => pad(32.0, 24.0),
            // stadium.ts: paddingX 40, paddingY 24 (totals).
            Shape::Stadium => pad(40.0, 24.0),
            // subroutine.ts: 28 / 12 (totals, besides the two frames).
            Shape::Subroutine => pad(28.0, 12.0),
            // cylinder.ts: 24 / 24.
            Shape::Cylinder => pad(24.0, 24.0),
            // circle.ts: radius = labelRadius + 16; doubleCircle.ts: inner ring the same.
            Shape::Circle | Shape::DoubleCircle => pad(32.0, 32.0),
            // rectLeftInvArrow.ts: 21 / 12, applied on both sides.
            Shape::Asymmetric => pad(42.0, 24.0),
            // hexagon.ts: h = th + 70 (wa), w = tw + 2m + 32 (ha); the names are swapped
            // upstream and reproduced as drawn.
            Shape::Hexagon => pad(32.0, 70.0),
            // leanRight/leanLeft/trapezoid.ts: labelPaddingX = 2p; Y unchanged.
            Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid => pad(2.0 * p, p),
            // invertedTrapezoid.ts: X = 2p, both padding values doubled.
            Shape::InvTrapezoid => pad(4.0 * p, 2.0 * p),
            // triangle.ts / flippedTriangle.ts: labelPaddingX = 2p.
            Shape::Triangle | Shape::FlippedTriangle => pad(2.0 * p, classic.pad.y),
            // card.ts: w = tw + 2·28 (no notch term), h = th + 2·24.
            Shape::Card => pad(56.0 - NOTCH, 48.0),
            // shadedProcess.ts: 16 / 12 per side, one frame instead of two.
            Shape::LinedRect => pad(32.0 - FRAME, 24.0),
            // dividedRect.ts: 16 / 16 (totals).
            Shape::DividedRect => pad(16.0, 16.0),
            // linedCylinder.ts: 16 / 24 per side.
            Shape::LinedCylinder => pad(32.0, 48.0),
            // tiltedCylinder.ts: labelPadding 12 (classic p/2).
            Shape::HorizontalCylinder => pad(12.0, 12.0),
            // halfRoundedRectangle, slopedRect, windowPane, trapezoidalPentagon,
            // taggedRect, waveEdgedRectangle, curvedTrapezoid, multiRect: 16 / 12 per side.
            Shape::Delay
            | Shape::SlopedRect
            | Shape::WindowPane
            | Shape::NotchedPentagon
            | Shape::TaggedRect
            | Shape::Document
            | Shape::LinedDocument
            | Shape::CurvedTrapezoid
            | Shape::StackedRect => pad(32.0, 24.0),
            // roundedRect, question, note, state start/end, fork, choice, text,
            // hourglass, crossedCircle: no neo branch.
            _ => classic.pad,
        };
        ShapeMetrics {
            pad,
            // hexagon.ts: f = 3.5.
            hexagon_factor: 3.5,
            // doubleCircle.ts: gap 12.
            double_circle_gap: 12.0,
            // multiRect.ts: rectOffset 10.
            stack_offset: 10.0,
            // waveEdgedRectangle.ts: amplitude h/4.
            wave_ratio: 1.0 / 4.0,
            neo: true,
            ..classic
        }
    }
}

/// Outer size of a label-bearing shape and where its label sits relative to
/// the node centre. `None` for shapes sized by the caller (tables, boxes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeSize {
    pub w: f64,
    pub h: f64,
    pub label_offset: Point,
}

/// Cylinder end-cap radius `ry = rx / (2.5 + w / 50)` (`cylinder.ts`).
fn cyl_ry(w: f64) -> f64 {
    w / 2.0 / (2.5 + w / 50.0)
}

/// Size of `shape` around a `tw`×`th` label.
pub fn size(
    shape: Shape,
    tw: f64,
    th: f64,
    m: &ShapeMetrics,
    horizontal: bool,
) -> Option<ShapeSize> {
    let (px, py) = (m.pad.x, m.pad.y);
    let at = |w: f64, h: f64| ShapeSize {
        w,
        h,
        label_offset: Point::default(),
    };
    let off = |w: f64, h: f64, dx: f64, dy: f64| ShapeSize {
        w,
        h,
        label_offset: Point::new(dx, dy),
    };
    Some(match shape {
        Shape::Rect
        | Shape::State
        | Shape::Rounded
        | Shape::Note
        | Shape::Text
        | Shape::Datastore => at(tw + px, th + py),
        Shape::Folder
        | Shape::Bucket
        | Shape::Console
        | Shape::Browser
        | Shape::Person
        | Shape::Bang
        | Shape::Cloud
        | Shape::BraceLeft
        | Shape::BraceRight
        | Shape::Braces
        | Shape::Bolt
        | Shape::StackedDocument
        | Shape::BowTieRect
        | Shape::TaggedDocument
        | Shape::PaperTape
        | Shape::LinedDocument => {
            let l = local(shape, tw, th, m)?;
            let b = l.bounds();
            off(b.w, b.h, l.label.x, l.label.y)
        }
        Shape::Stadium => {
            // stadium.ts stadiumDimensions (50-point arcs).
            let h = th + py;
            let inscribed = h * (PI / (2.0 * 49.0)).cos();
            let cap = (inscribed.powi(2) - th.powi(2)).max(0.0).sqrt();
            let w = (tw + h / 4.0 + px)
                .max(1.5 * h + (h - inscribed))
                .max(tw + px + h - cap);
            at(w, h)
        }
        Shape::Subroutine => at(tw + 2.0 * FRAME + px, th + py),
        Shape::Cylinder => {
            let w = tw + px;
            let ry = cyl_ry(w);
            // Body `h = th + p + ry` plus both caps; the label sits p/1.5 below centre.
            off(w, th + py + 3.0 * ry, 0.0, m.node_padding / 1.5)
        }
        Shape::Circle => {
            let d = tw.hypot(th) + px;
            at(d, d)
        }
        Shape::DoubleCircle => {
            let d = tw.hypot(th) + px + 2.0 * m.double_circle_gap;
            at(d, d)
        }
        Shape::Asymmetric => {
            let h = th + py;
            off(tw + px + h / 4.0, h, h / 8.0, 0.0)
        }
        Shape::Diamond => {
            let s = tw + px + th + py;
            at(s, s)
        }
        Shape::Hexagon => {
            let h = th + py;
            at(tw + px + 2.0 * h / m.hexagon_factor, h)
        }
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid | Shape::InvTrapezoid => {
            let h = th + py;
            at(tw + px + h, h)
        }
        Shape::Card => at(tw + px + NOTCH, th + py),
        Shape::LinedRect => off(tw + px + 2.0 * FRAME, th + py, FRAME / 2.0, 0.0),
        Shape::Delay => {
            let h = th.max(10.0) + py;
            let r = h / 2.0;
            let cap = r - (r * r - (th / 2.0).powi(2)).max(0.0).sqrt();
            at(tw.max(15.0) + 2.0 * cap + px, h)
        }
        Shape::Triangle | Shape::FlippedTriangle => {
            let h = tw + px + th;
            let dy = h / 2.0 - m.node_padding / 2.0 - th / 2.0;
            let dy = if shape == Shape::Triangle { dy } else { -dy };
            off(h, h, 0.0, dy)
        }
        Shape::Hourglass => at(30.0, 30.0),
        Shape::CrossedCircle => at(60.0, 60.0),
        Shape::SlopedRect => {
            let h = th + py;
            off(tw + px, 1.5 * h, 0.0, h / 4.0)
        }
        Shape::WindowPane => off(tw + px + PANE, th + py + PANE, PANE / 2.0, PANE / 2.0),
        Shape::DividedRect => {
            let h = th + py;
            off(tw + px, 1.2 * h, 0.0, 0.1 * h)
        }
        Shape::NotchedPentagon => at(tw + px, th + py),
        Shape::StackedRect => {
            let o = m.stack_offset;
            off(tw + px + 2.0 * o, th + py + 2.0 * o, -o, o)
        }
        Shape::TaggedRect => {
            let h = th + py;
            at(tw + px + TAG_RATIO * h, h)
        }
        Shape::Document => {
            let h = th + py;
            let amp = h * m.wave_ratio;
            off(tw + px, h + 2.0 * amp, 0.0, -amp)
        }
        // collapsedGroup.ts: padding 8 around the title, an 8px gap and a 20px row of
        // dots below it, at least 80 wide; the title sits above the centre.
        Shape::CollapsedGroup => off(
            (tw + 2.0 * COLLAPSED_PAD).max(80.0),
            th + COLLAPSED_ROWS + 2.0 * COLLAPSED_PAD,
            0.0,
            -COLLAPSED_ROWS / 2.0,
        ),
        Shape::LinedCylinder => {
            let w = tw + px;
            let ry = cyl_ry(w);
            off(w, th + py + 3.0 * ry, 0.0, ry)
        }
        Shape::HorizontalCylinder => {
            let h = th + py;
            let rx = cyl_ry(h);
            off(tw + px + 3.0 * rx, h, -rx, 0.0)
        }
        Shape::CurvedTrapezoid => {
            let h = (th + py).max(5.0);
            let r = h / 2.0;
            let cap = r - (r * r - (th / 2.0).powi(2)).max(0.0).sqrt();
            let side = (h / 4.0).max(cap);
            let w = 20f64.max(tw + px + 2.0 * side).max((tw + px) * 1.25);
            at(w, h)
        }
        // stateStart.ts / stateEnd.ts / filledCircle.ts: 14px circles.
        Shape::StateStart | Shape::SmallCircle | Shape::StateEnd => at(14.0, 14.0),
        // forkJoin.ts: 70×10, rotated for LR/RL.
        Shape::ForkBar if horizontal => at(10.0, 70.0),
        Shape::ForkBar => at(70.0, 10.0),
        // choice.ts: 28×28 diamond.
        Shape::Choice => at(28.0, 28.0),
        Shape::Compartments | Shape::Invisible | Shape::EntityTable | Shape::Requirement => {
            return None;
        }
    })
}

/// Whether a shape draws its label (some upstream shapes clear `node.label`).
pub fn shows_label(shape: Shape) -> bool {
    !matches!(
        shape,
        Shape::StateStart
            | Shape::SmallCircle
            | Shape::StateEnd
            | Shape::ForkBar
            | Shape::Choice
            | Shape::Invisible
            | Shape::Hourglass
            | Shape::CrossedCircle
            | Shape::Bolt
    )
}

/// Clipping boundary of a drawn node.
#[derive(Debug, Clone, PartialEq)]
pub enum Boundary {
    Rect(Rect),
    Ellipse(Point, f64, f64),
    Polygon(Point, Vec<Point>),
    /// A possibly concave outline clipped like `intersect.polygon`: of the crossings
    /// between the centre and the outer point, the one nearest the outer point.
    Outline(Point, Vec<Point>),
    /// A cylinder: its bounding box and end-cap radius `ry`, clipped as
    /// `cylinder.ts`'s `intersect` does (the box's crossing, moved by the cap).
    Cylinder(Rect, f64),
}

impl Boundary {
    pub fn clip(&self, toward: Point) -> Point {
        match self {
            Boundary::Rect(r) => geom::clip_rect(r, toward),
            Boundary::Ellipse(c, rx, ry) => geom::clip_ellipse(*c, *rx, *ry, toward),
            Boundary::Polygon(c, pts) => geom::clip_polygon(*c, pts, toward),
            Boundary::Outline(c, pts) => outline_clip(*c, pts, toward),
            Boundary::Cylinder(r, ry) => cylinder_clip(r, *ry, toward),
        }
    }
}

/// `cylinder.ts`'s `intersect`: the bounding box's crossing, moved along y by
/// what the end cap's ellipse (`rx` = half the width) leaves of `ry` there.
fn cylinder_clip(r: &Rect, ry: f64, toward: Point) -> Point {
    let c = r.center();
    let mut pos = geom::intersect_rect(r, toward);
    let rx = r.w / 2.0;
    let x = pos.x - c.x;
    if rx != 0.0
        && (x.abs() < r.w / 2.0 || (x.abs() == r.w / 2.0 && (pos.y - c.y).abs() > r.h / 2.0 - ry))
    {
        let mut y = ry * ry * (1.0 - x * x / (rx * rx));
        if y > 0.0 {
            y = y.sqrt();
        }
        y = ry - y;
        if toward.y - c.y > 0.0 {
            y = -y;
        }
        pos.y += y;
    }
    pos
}

/// How a part of an outline is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartStyle {
    /// The node's fill and stroke (`basic label-container`).
    Body,
    /// Stroke only, in the node's stroke colour and width.
    Line,
    /// Takes up space but paints nothing (`rect.text { fill: none; stroke-width: 0 }`).
    Hidden,
    /// The node's fill without a stroke.
    Fill,
    /// A 1px stroke in `themeVariables.nodeBorder` (decorations of console/browser/bucket).
    AccentLine,
    /// Like [`PartStyle::AccentLine`] at 60% opacity.
    AccentFaint,
    /// Filled with `themeVariables.nodeBorder`, no stroke.
    AccentDot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub geom: Geometry,
    pub style: PartStyle,
}

/// Drawn parts of a shape plus its edge-clipping boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub parts: Vec<Part>,
    pub boundary: Boundary,
    /// Decorative text drawn inside the shape (the `console` prompt).
    pub glyphs: Vec<Glyph>,
}

/// A short decoration string, `x`/`y` = start of its baseline, in the accent colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub at: Point,
    pub text: &'static str,
    pub font_size: f64,
    pub bold: bool,
}

fn body(geom: Geometry) -> Part {
    Part {
        geom,
        style: PartStyle::Body,
    }
}

fn line(cmds: Vec<PathCmd>) -> Part {
    Part {
        geom: Geometry::Path(cmds),
        style: PartStyle::Line,
    }
}

fn seg(a: Point, b: Point) -> Vec<PathCmd> {
    vec![PathCmd::MoveTo(a), PathCmd::LineTo(b)]
}

fn poly_outline(c: Point, pts: Vec<Point>) -> Outline {
    Outline {
        parts: vec![body(Geometry::Path(geom::polygon(&pts)))],
        boundary: Boundary::Polygon(c, pts),
        glyphs: Vec::new(),
    }
}

/// `generateCirclePoints` (util.ts): note the result is negated.
fn circle_points(cx: f64, cy: f64, r: f64, n: usize, a0: f64, a1: f64) -> Vec<Point> {
    let (s, e) = (a0.to_radians(), a1.to_radians());
    let step = (e - s) / (n - 1) as f64;
    (0..n)
        .map(|i| {
            let a = s + step * i as f64;
            Point::new(-(cx + r * a.cos()), -(cy + r * a.sin()))
        })
        .collect()
}

/// Draw `shape` filling the `w`×`h` box centred on `c`. `None` for shapes the
/// caller draws itself.
pub fn outline(shape: Shape, c: Point, w: f64, h: f64, m: &ShapeMetrics) -> Option<Outline> {
    let r = Rect::centered(c, w, h);
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let p = |x: f64, y: f64| Point::new(x, y);
    let rect = |rx: f64| Outline {
        parts: vec![body(Geometry::Rect { rect: r, rx })],
        boundary: Boundary::Rect(r),
        glyphs: Vec::new(),
    };
    Some(match shape {
        Shape::Rect | Shape::Note => rect(0.0),
        // datastore.ts: `stroke-dasharray: w h` leaves only the top and bottom rules.
        Shape::Datastore => Outline {
            parts: vec![
                Part {
                    geom: Geometry::Rect { rect: r, rx: 0.0 },
                    style: PartStyle::Fill,
                },
                line(seg(p(x0, y0), p(x1, y0))),
                line(seg(p(x0, y1), p(x1, y1))),
            ],
            boundary: Boundary::Rect(r),
            glyphs: Vec::new(),
        },
        s if is_bbox_sized(s) => {
            let (tw, th) = m.label;
            return local(s, tw, th, m).map(|l| l.place(r));
        }
        // roundedRect.ts: themeVariables.radius (5); state rects get `rx: 5px` from CSS.
        Shape::Rounded => rect(m.corner_radius),
        Shape::State => rect(5.0),
        Shape::Text => Outline {
            parts: vec![Part {
                geom: Geometry::Rect { rect: r, rx: 0.0 },
                style: PartStyle::Hidden,
            }],
            boundary: Boundary::Rect(r),
            glyphs: Vec::new(),
        },
        Shape::Stadium => {
            let rad = h / 2.0;
            let mut cmds = vec![
                PathCmd::MoveTo(p(x0 + rad, y0)),
                PathCmd::LineTo(p(x1 - rad, y0)),
            ];
            cmds.extend(geom::arc_cubics(p(x1 - rad, c.y), rad, -PI / 2.0, PI / 2.0));
            cmds.push(PathCmd::LineTo(p(x0 + rad, y1)));
            cmds.extend(geom::arc_cubics(p(x0 + rad, c.y), rad, PI / 2.0, 1.5 * PI));
            cmds.push(PathCmd::Close);
            let mut pts = vec![p(x0 + rad, y0), p(x1 - rad, y0)];
            pts.extend(arc_points(p(x1 - rad, c.y), rad, -PI / 2.0, PI / 2.0));
            pts.push(p(x0 + rad, y1));
            pts.extend(arc_points(p(x0 + rad, c.y), rad, PI / 2.0, 1.5 * PI));
            Outline {
                parts: vec![body(Geometry::Path(cmds))],
                boundary: Boundary::Polygon(c, pts),
                glyphs: Vec::new(),
            }
        }
        Shape::Subroutine | Shape::LinedRect => {
            let mut o = rect(0.0);
            // subroutine.ts: inner frame lines FRAME_WIDTH in from each side;
            // shadedProcess.ts: a single line on the left.
            o.parts
                .push(line(seg(p(x0 + FRAME, y0), p(x0 + FRAME, y1))));
            if shape == Shape::Subroutine {
                o.parts
                    .push(line(seg(p(x1 - FRAME, y0), p(x1 - FRAME, y1))));
            }
            o
        }
        Shape::Cylinder | Shape::LinedCylinder => {
            let rx = w / 2.0;
            let ry = cyl_ry(w);
            let top_c = p(c.x, y0 + ry);
            let bot_c = p(c.x, y1 - ry);
            let mut cmds = vec![
                PathCmd::MoveTo(p(x0, y0 + ry)),
                PathCmd::LineTo(p(x0, y1 - ry)),
            ];
            cmds.extend(geom::elliptic_arc_cubics(bot_c, rx, ry, PI, 0.0));
            cmds.push(PathCmd::LineTo(p(x1, y0 + ry)));
            cmds.extend(geom::elliptic_arc_cubics(top_c, rx, ry, 0.0, -PI));
            cmds.push(PathCmd::Close);
            let mut parts = vec![body(Geometry::Path(cmds))];
            let mut front = vec![PathCmd::MoveTo(p(x0, y0 + ry))];
            front.extend(geom::elliptic_arc_cubics(top_c, rx, ry, PI, 0.0));
            parts.push(line(front));
            if shape == Shape::LinedCylinder {
                // linedCylinder.ts: second rim `outerOffset = 0.1 * h` below the top one.
                let off = 0.1 * (h - 2.0 * ry);
                let lc = p(c.x, y0 + ry + off);
                let mut rim = vec![PathCmd::MoveTo(p(x0, y0 + ry + off))];
                rim.extend(geom::elliptic_arc_cubics(lc, rx, ry, PI, 0.0));
                parts.push(line(rim));
            }
            Outline {
                parts,
                boundary: Boundary::Cylinder(r, ry),
                glyphs: Vec::new(),
            }
        }
        Shape::HorizontalCylinder => {
            let ry = h / 2.0;
            let rx = cyl_ry(h);
            let (l, rr) = (x0 + rx, x1 - rx);
            let mut cmds = vec![PathCmd::MoveTo(p(l, y1))];
            cmds.extend(geom::elliptic_arc_cubics(
                p(l, c.y),
                rx,
                ry,
                PI / 2.0,
                1.5 * PI,
            ));
            cmds.push(PathCmd::LineTo(p(rr, y0)));
            cmds.extend(geom::elliptic_arc_cubics(
                p(rr, c.y),
                rx,
                ry,
                -PI / 2.0,
                PI / 2.0,
            ));
            cmds.push(PathCmd::Close);
            let mut inner = vec![PathCmd::MoveTo(p(rr, y0))];
            inner.extend(geom::elliptic_arc_cubics(
                p(rr, c.y),
                rx,
                ry,
                -PI / 2.0,
                -1.5 * PI,
            ));
            Outline {
                parts: vec![body(Geometry::Path(cmds)), line(inner)],
                boundary: Boundary::Rect(r),
                glyphs: Vec::new(),
            }
        }
        Shape::Circle => Outline {
            parts: vec![body(Geometry::Circle { c, r: w / 2.0 })],
            boundary: Boundary::Ellipse(c, w / 2.0, h / 2.0),
            glyphs: Vec::new(),
        },
        Shape::DoubleCircle => Outline {
            parts: vec![
                body(Geometry::Circle { c, r: w / 2.0 }),
                body(Geometry::Circle {
                    c,
                    r: w / 2.0 - m.double_circle_gap,
                }),
            ],
            boundary: Boundary::Ellipse(c, w / 2.0, h / 2.0),
            glyphs: Vec::new(),
        },
        Shape::CrossedCircle => {
            let rad = w / 2.0;
            let d = rad * (PI / 4.0).cos();
            Outline {
                parts: vec![
                    body(Geometry::Circle { c, r: rad }),
                    line(seg(p(c.x - d, c.y + d), p(c.x + d, c.y - d))),
                    line(seg(p(c.x + d, c.y + d), p(c.x - d, c.y - d))),
                ],
                boundary: Boundary::Ellipse(c, rad, rad),
                glyphs: Vec::new(),
            }
        }
        // rectLeftInvArrow.ts: notch depth h/4.
        Shape::Asymmetric => poly_outline(
            c,
            vec![
                p(x0, y0),
                p(x1, y0),
                p(x1, y1),
                p(x0, y1),
                p(x0 + h / 4.0, c.y),
            ],
        ),
        Shape::Diamond | Shape::Choice => {
            poly_outline(c, vec![p(c.x, y0), p(x1, c.y), p(c.x, y1), p(x0, c.y)])
        }
        Shape::Hexagon => {
            let k = h / m.hexagon_factor;
            poly_outline(
                c,
                vec![
                    p(x0 + k, y0),
                    p(x1 - k, y0),
                    p(x1, c.y),
                    p(x1 - k, y1),
                    p(x0 + k, y1),
                    p(x0, c.y),
                ],
            )
        }
        // leanRight.ts & co: the slanted sides run 3h/6 = h/2 horizontally.
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid | Shape::InvTrapezoid => {
            let s = h / 2.0;
            let pts = match shape {
                Shape::LeanRight => vec![p(x0 + s, y0), p(x1, y0), p(x1 - s, y1), p(x0, y1)],
                Shape::LeanLeft => vec![p(x0, y0), p(x1 - s, y0), p(x1, y1), p(x0 + s, y1)],
                Shape::Trapezoid => vec![p(x0 + s, y0), p(x1 - s, y0), p(x1, y1), p(x0, y1)],
                _ => vec![p(x0, y0), p(x1, y0), p(x1 - s, y1), p(x0 + s, y1)],
            };
            poly_outline(c, pts)
        }
        Shape::Card => poly_outline(
            c,
            vec![
                p(x0 + NOTCH, y0),
                p(x1, y0),
                p(x1, y1),
                p(x0, y1),
                p(x0, y0 + NOTCH),
            ],
        ),
        Shape::Delay => {
            let rad = h / 2.0;
            let mut cmds = vec![PathCmd::MoveTo(p(x0, y0)), PathCmd::LineTo(p(x1 - rad, y0))];
            cmds.extend(geom::arc_cubics(p(x1 - rad, c.y), rad, -PI / 2.0, PI / 2.0));
            cmds.push(PathCmd::LineTo(p(x0, y1)));
            cmds.push(PathCmd::Close);
            let mut pts = vec![p(x0, y0)];
            pts.extend(arc_points(p(x1 - rad, c.y), rad, -PI / 2.0, PI / 2.0));
            pts.push(p(x0, y1));
            Outline {
                parts: vec![body(Geometry::Path(cmds))],
                boundary: Boundary::Polygon(c, pts),
                glyphs: Vec::new(),
            }
        }
        Shape::Triangle => poly_outline(c, vec![p(x0, y1), p(x1, y1), p(c.x, y0)]),
        Shape::FlippedTriangle => poly_outline(c, vec![p(x0, y0), p(x1, y0), p(c.x, y1)]),
        // hourglass.ts: (0,0) (w,0) (0,h) (w,h).
        Shape::Hourglass => {
            let pts = vec![p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1)];
            Outline {
                parts: vec![body(Geometry::Path(geom::polygon(&pts)))],
                boundary: Boundary::Rect(r),
                glyphs: Vec::new(),
            }
        }
        // slopedRect.ts: the right edge rises h/2 above the left one.
        Shape::SlopedRect => {
            let top_left = y0 + h / 3.0;
            poly_outline(c, vec![p(x0, top_left), p(x0, y1), p(x1, y1), p(x1, y0)])
        }
        Shape::WindowPane => {
            let mut o = rect(0.0);
            o.parts.push(line(seg(p(x0, y0 + PANE), p(x1, y0 + PANE))));
            o.parts.push(line(seg(p(x0 + PANE, y0), p(x0 + PANE, y1))));
            o
        }
        // dividedRect.ts: a header band of 0.2 × the body height.
        Shape::DividedRect => {
            let mut o = rect(0.0);
            let band = h / 6.0;
            o.parts.push(line(seg(p(x0, y0 + band), p(x1, y0 + band))));
            o
        }
        Shape::NotchedPentagon => poly_outline(
            c,
            vec![
                p(c.x - 0.4 * w, y0),
                p(c.x + 0.4 * w, y0),
                p(x1, c.y - 0.3 * h),
                p(x1, y1),
                p(x0, y1),
                p(x0, c.y - 0.3 * h),
            ],
        ),
        Shape::StackedRect => {
            // multiRect.ts: two sheets peeking out up-right behind the front one.
            let o = m.stack_offset;
            let (bw, bh) = (w - 2.0 * o, h - 2.0 * o);
            let (x, y) = (c.x - bw / 2.0, c.y - bh / 2.0);
            let outer = vec![
                p(x - o, y + o),
                p(x - o, y + bh + o),
                p(x + bw - o, y + bh + o),
                p(x + bw - o, y + bh),
                p(x + bw, y + bh),
                p(x + bw, y + bh - o),
                p(x + bw + o, y + bh - o),
                p(x + bw + o, y - o),
                p(x + o, y - o),
                p(x + o, y),
                p(x, y),
                p(x, y + o),
            ];
            let inner = vec![
                p(x, y + o),
                p(x + bw - o, y + o),
                p(x + bw - o, y + bh),
                p(x + bw, y + bh),
                p(x + bw, y),
                p(x, y),
            ];
            Outline {
                parts: vec![
                    body(Geometry::Path(geom::polygon(&outer))),
                    body(Geometry::Path(geom::polygon(&inner))),
                ],
                boundary: Boundary::Polygon(c, outer),
                glyphs: Vec::new(),
            }
        }
        Shape::TaggedRect => {
            let mut o = rect(0.0);
            let t = TAG_RATIO * h;
            o.parts.push(body(Geometry::Path(geom::polygon(&[
                p(x1 - t, y1),
                p(x1, y1),
                p(x1, y1 - t),
            ]))));
            o
        }
        Shape::Document => {
            // waveEdgedRectangle.ts: 0.8 sine cycles of amplitude `amp` along the bottom.
            let amp = h / (1.0 / m.wave_ratio + 2.0);
            let base = y1 - amp;
            let mut pts = vec![p(x0, y0), p(x1, y0)];
            pts.extend((0..=50).rev().map(|i| {
                let x = x0 + w * i as f64 / 50.0;
                p(x, base + amp * (2.0 * PI * 0.8 * (x - x0) / w).sin())
            }));
            poly_outline(c, pts)
        }
        // collapsedGroup.ts: rounded box (rx 10, the expanded flow container's radius);
        // the separator and the dots are drawn by the caller.
        Shape::CollapsedGroup => rect(10.0),
        Shape::CurvedTrapezoid => {
            let rad = h / 2.0;
            let mut pts = vec![
                p(x1 - rad, y0),
                p(x0 + h / 4.0, y0),
                p(x0, c.y),
                p(x0 + h / 4.0, y1),
            ];
            pts.extend(circle_points(-(x1 - rad), -c.y, rad, 50, 270.0, 90.0));
            poly_outline(c, pts)
        }
        _ => return None,
    })
}

/// Points along a circular arc (screen angles), for clipping polygons.
fn arc_points(c: Point, r: f64, a0: f64, a1: f64) -> Vec<Point> {
    let n = 16;
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f64 / n as f64;
            Point::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shapes sized from their drawn bounding box.
//
// Upstream builds these in the node's local frame (origin = node centre), applies
// a group transform, and sets the node size from `getBBox()` — so the drawing need
// not be centred in its layout box. [`local`] reproduces that frame; [`size`] takes
// the bounding box and [`outline`] draws the same geometry around the node centre.
// ---------------------------------------------------------------------------

/// Shapes whose layout box is the bounding box of their local geometry.
fn is_bbox_sized(shape: Shape) -> bool {
    matches!(
        shape,
        Shape::Folder
            | Shape::Bucket
            | Shape::Console
            | Shape::Browser
            | Shape::Person
            | Shape::Bang
            | Shape::Cloud
            | Shape::BraceLeft
            | Shape::BraceRight
            | Shape::Braces
            | Shape::Bolt
            | Shape::StackedDocument
            | Shape::BowTieRect
            | Shape::TaggedDocument
            | Shape::PaperTape
            | Shape::LinedDocument
    )
}

/// A shape's geometry in its local frame.
struct Local {
    parts: Vec<Part>,
    glyphs: Vec<Glyph>,
    /// `intersect.polygon` points (aligned to the layout box by their minimum, as
    /// upstream does); empty for `intersect.rect`.
    clip: Vec<Point>,
    /// Label centre.
    label: Point,
}

impl Local {
    fn bounds(&self) -> Rect {
        let mut pts = Vec::new();
        for part in &self.parts {
            geometry_points(&part.geom, &mut pts);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for q in pts {
            x0 = x0.min(q.x);
            y0 = y0.min(q.y);
            x1 = x1.max(q.x);
            y1 = y1.max(q.y);
        }
        if x0 > x1 {
            return Rect::default();
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Draw around the centre of the layout box `r`.
    fn place(self, r: Rect) -> Outline {
        let c = r.center();
        let boundary = if self.clip.is_empty() {
            Boundary::Rect(r)
        } else {
            let min_x = self.clip.iter().map(|q| q.x).fold(f64::INFINITY, f64::min);
            let min_y = self.clip.iter().map(|q| q.y).fold(f64::INFINITY, f64::min);
            Boundary::Outline(c, shifted(&self.clip, r.x - min_x, r.y - min_y))
        };
        Outline {
            parts: self
                .parts
                .into_iter()
                .map(|part| Part {
                    geom: part.geom.translated(c.x, c.y),
                    style: part.style,
                })
                .collect(),
            boundary,
            glyphs: self
                .glyphs
                .into_iter()
                .map(|g| Glyph {
                    at: g.at.plus(c),
                    ..g
                })
                .collect(),
        }
    }
}

/// `intersect-polygon.js`: the crossing of segment `c`→`toward` with the outline
/// that lies nearest `toward` (falls back to the ray clip when there is none).
fn outline_clip(c: Point, pts: &[Point], toward: Point) -> Point {
    let d = toward.minus(c);
    let mut best: Option<f64> = None;
    for i in 0..pts.len() {
        let a = pts[i];
        let e = pts[(i + 1) % pts.len()].minus(a);
        let den = d.x * e.y - d.y * e.x;
        if den.abs() < 1e-12 {
            continue;
        }
        let ac = a.minus(c);
        let t = (ac.x * e.y - ac.y * e.x) / den;
        let u = (ac.x * d.y - ac.y * d.x) / den;
        if (0.0..=1.0).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
            best = Some(best.map_or(t, |bt: f64| bt.max(t)));
        }
    }
    best.map_or_else(
        || geom::clip_polygon(c, pts, toward),
        |t| c.plus(d.scale(t)),
    )
}

/// Points bounding a geometry (cubics sampled).
fn geometry_points(g: &Geometry, out: &mut Vec<Point>) {
    match g {
        Geometry::Rect { rect, .. } => {
            out.push(Point::new(rect.x, rect.y));
            out.push(Point::new(rect.right(), rect.bottom()));
        }
        Geometry::Circle { c, r } => {
            out.push(Point::new(c.x - r, c.y - r));
            out.push(Point::new(c.x + r, c.y + r));
        }
        Geometry::Path(cmds) => {
            let mut cur = Point::default();
            for cmd in cmds {
                match *cmd {
                    PathCmd::MoveTo(q) | PathCmd::LineTo(q) => {
                        out.push(q);
                        cur = q;
                    }
                    PathCmd::CubicTo(a, b, e) => {
                        for i in 1..=32 {
                            let t = i as f64 / 32.0;
                            let u = 1.0 - t;
                            let (k0, k1, k2, k3) =
                                (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                            out.push(Point::new(
                                k0 * cur.x + k1 * a.x + k2 * b.x + k3 * e.x,
                                k0 * cur.y + k1 * a.y + k2 * b.y + k3 * e.y,
                            ));
                        }
                        cur = e;
                    }
                    PathCmd::Close => {}
                }
            }
        }
    }
}

/// `generateFullSineWavePoints` (util.ts): 51 points.
fn sine_wave(x1: f64, y1: f64, x2: f64, y2: f64, amp: f64, cycles: f64) -> Vec<Point> {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let freq = 2.0 * PI / (dx / cycles);
    let mid = y1 + dy / 2.0;
    (0..=50)
        .map(|i| {
            let x = x1 + i as f64 / 50.0 * dx;
            Point::new(x, mid + amp * (freq * (x - x1)).sin())
        })
        .collect()
}

/// `generateCirclePoints` without the negation (curlyBraceRight.ts' local copy).
fn circle_points_pos(cx: f64, cy: f64, r: f64, n: usize, a0: f64, a1: f64) -> Vec<Point> {
    circle_points(cx, cy, r, n, a0, a1)
        .into_iter()
        .map(|q| Point::new(-q.x, -q.y))
        .collect()
}

/// `generateArcPoints` (bowTieRect.ts): 20 points on an ellipse from `a` to `b`.
fn bow_arc_points(a: Point, b: Point, rx: f64, ry: f64, clockwise: bool) -> Vec<Point> {
    let mid = a.lerp(b, 0.5);
    let angle = (b.y - a.y).atan2(b.x - a.x);
    let (tx, ty) = ((b.x - a.x) / 2.0 / rx, (b.y - a.y) / 2.0 / ry);
    let dist = tx.hypot(ty).min(1.0);
    let k = (1.0 - dist * dist).sqrt();
    let s = if clockwise { -1.0 } else { 1.0 };
    let cx = mid.x + k * ry * angle.sin() * s;
    let cy = mid.y - k * rx * angle.cos() * s;
    let start = ((a.y - cy) / ry).atan2((a.x - cx) / rx);
    let end = ((b.y - cy) / ry).atan2((b.x - cx) / rx);
    let mut range = end - start;
    if clockwise && range < 0.0 {
        range += 2.0 * PI;
    }
    if !clockwise && range > 0.0 {
        range -= 2.0 * PI;
    }
    (0..20)
        .map(|i| {
            let t = start + i as f64 / 19.0 * range;
            Point::new(cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

/// One SVG elliptical arc command: radii, `x-axis-rotation` (degrees), the
/// large-arc and sweep flags.
#[derive(Debug, Clone, Copy)]
struct Arc {
    rx: f64,
    ry: f64,
    phi: f64,
    large: bool,
    sweep: bool,
}

/// SVG elliptical arc from `from` to `to` as cubics (SVG 1.1 implementation notes
/// F.6.5/F.6.6, radii scaled up when too small).
fn svg_arc(out: &mut Vec<PathCmd>, from: Point, a: Arc, to: Point) {
    let Some(e) = ArcCenter::new(from, a, to) else {
        out.push(PathCmd::LineTo(to));
        return;
    };
    let n = ((e.dt.abs() / (PI / 2.0)).ceil() as usize).max(1);
    let step = e.dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..n {
        let t0 = e.t1 + step * i as f64;
        let t1 = t0 + step;
        let end = if i + 1 == n { to } else { e.at(t1) };
        out.push(PathCmd::CubicTo(
            e.at(t0).plus(e.tangent(t0).scale(k)),
            e.at(t1).minus(e.tangent(t1).scale(k)),
            end,
        ));
    }
}

/// A closed path of relative SVG arcs (`a rx,ry phi large,sweep dx,dy`) from `start`.
fn relative_arcs(start: Point, arcs: &[(Arc, f64, f64)]) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(start)];
    let mut cur = start;
    for &(a, dx, dy) in arcs {
        let to = Point::new(cur.x + dx, cur.y + dy);
        svg_arc(&mut cmds, cur, a, to);
        cur = to;
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// `bang.ts`: the outline's start and its relative arcs around the node centre.
/// Every arc carries `x-axis-rotation` 1 (an upstream quirk, kept).
fn bang_arcs(tw: f64, th: f64, p: f64) -> (Point, Vec<(Arc, f64, f64)>) {
    let half = p / 2.0;
    let base_w = tw + 10.0 * half;
    let w = base_w.max(tw + 20.0);
    let h = (th + 8.0 * half).max(th + 20.0);
    let r = 0.15 * base_w;
    let a = |rr: f64, dx: f64, dy: f64| {
        (
            Arc {
                rx: rr,
                ry: rr,
                phi: 1.0,
                large: false,
                sweep: false,
            },
            dx,
            dy,
        )
    };
    let arcs = vec![
        a(r, w * 0.25, -h * 0.1),
        a(r, w * 0.25, 0.0),
        a(r, w * 0.25, 0.0),
        a(r, w * 0.25, h * 0.1),
        a(r, w * 0.15, h * 0.33),
        a(r * 0.8, 0.0, h * 0.34),
        a(r, -w * 0.15, h * 0.33),
        a(r, -w * 0.25, h * 0.15),
        a(r, -w * 0.25, 0.0),
        a(r, -w * 0.25, 0.0),
        a(r, -w * 0.25, -h * 0.15),
        a(r, -w * 0.1, -h * 0.33),
        a(r * 0.8, 0.0, -h * 0.34),
        a(r, w * 0.1, -h * 0.33),
    ];
    (Point::new(-w / 2.0, -h / 2.0), arcs)
}

/// `cloud.ts`: the outline's start and its relative arcs around the node centre.
fn cloud_arcs(tw: f64, th: f64, p: f64) -> (Point, Vec<(Arc, f64, f64)>) {
    let half = p / 2.0;
    let (w, h) = (tw + 2.0 * half, th + 2.0 * half);
    let (r1, r2, r3, r4) = (0.15 * w, 0.25 * w, 0.35 * w, 0.2 * w);
    let a = |rx: f64, ry: f64, phi: f64, dx: f64, dy: f64| {
        (
            Arc {
                rx,
                ry,
                phi,
                large: false,
                sweep: true,
            },
            dx,
            dy,
        )
    };
    let arcs = vec![
        a(r1, r1, 0.0, w * 0.25, -w * 0.1),
        a(r3, r3, 1.0, w * 0.4, -w * 0.1),
        a(r2, r2, 1.0, w * 0.35, w * 0.2),
        a(r1, r1, 1.0, w * 0.15, h * 0.35),
        a(r4, r4, 1.0, -w * 0.15, h * 0.65),
        a(r2, r1, 1.0, -w * 0.25, w * 0.15),
        a(r3, r3, 1.0, -w * 0.5, 0.0),
        a(r1, r1, 1.0, -w * 0.25, -w * 0.15),
        a(r1, r1, 1.0, -w * 0.1, -h * 0.35),
        a(r4, r4, 1.0, w * 0.1, -h * 0.65),
    ];
    (Point::new(-w / 2.0, -h / 2.0), arcs)
}

/// The outline of a `bang` or `cloud` node with `node.padding` `p` around a
/// `tw`×`th` label, centred on the origin, and the box Chromium's `getBBox()`
/// reports for the node (see [`super::svgbbox`]). `None` for other shapes.
pub fn arc_outline(shape: Shape, tw: f64, th: f64, p: f64) -> Option<(Vec<PathCmd>, Rect)> {
    let (start, arcs) = match shape {
        Shape::Bang => bang_arcs(tw, th, p),
        Shape::Cloud => cloud_arcs(tw, th, p),
        _ => return None,
    };
    let rel: Vec<super::svgbbox::RelArc> = arcs
        .iter()
        .map(|&(a, dx, dy)| super::svgbbox::RelArc {
            rx: a.rx,
            ry: a.ry,
            phi: a.phi,
            large: a.large,
            sweep: a.sweep,
            dx,
            dy,
        })
        .collect();
    let [x, y, w, h] = super::svgbbox::chromium_box((start.x, start.y), &rel, (tw, th));
    Some((relative_arcs(start, &arcs), Rect::new(x, y, w, h)))
}

/// Centre parameterisation of an SVG arc (SVG 1.1 implementation notes
/// F.6.5/F.6.6, radii scaled up when too small).
struct ArcCenter {
    c: Point,
    rx: f64,
    ry: f64,
    sin_phi: f64,
    cos_phi: f64,
    t1: f64,
    dt: f64,
}

impl ArcCenter {
    fn new(from: Point, a: Arc, to: Point) -> Option<ArcCenter> {
        let (mut rx, mut ry) = (a.rx.abs(), a.ry.abs());
        if rx == 0.0 || ry == 0.0 || from == to {
            return None;
        }
        let (sp, cp) = a.phi.to_radians().sin_cos();
        let hx = (from.x - to.x) / 2.0;
        let hy = (from.y - to.y) / 2.0;
        let x1 = cp * hx + sp * hy;
        let y1 = -sp * hx + cp * hy;
        let lambda = (x1 / rx).powi(2) + (y1 / ry).powi(2);
        if lambda > 1.0 {
            rx *= lambda.sqrt();
            ry *= lambda.sqrt();
        }
        let num = (rx * ry).powi(2) - (rx * y1).powi(2) - (ry * x1).powi(2);
        let den = (rx * y1).powi(2) + (ry * x1).powi(2);
        let mut coef = (num / den).max(0.0).sqrt();
        if a.large == a.sweep {
            coef = -coef;
        }
        let cxp = coef * rx * y1 / ry;
        let cyp = -coef * ry * x1 / rx;
        let cx = cp * cxp - sp * cyp + (from.x + to.x) / 2.0;
        let cy = sp * cxp + cp * cyp + (from.y + to.y) / 2.0;
        let t1 = ((y1 - cyp) / ry).atan2((x1 - cxp) / rx);
        let t2 = ((-y1 - cyp) / ry).atan2((-x1 - cxp) / rx);
        let mut dt = t2 - t1;
        if !a.sweep && dt > 0.0 {
            dt -= 2.0 * PI;
        } else if a.sweep && dt < 0.0 {
            dt += 2.0 * PI;
        }
        Some(ArcCenter {
            c: Point::new(cx, cy),
            rx,
            ry,
            sin_phi: sp,
            cos_phi: cp,
            t1,
            dt,
        })
    }

    fn at(&self, t: f64) -> Point {
        let (ex, ey) = (self.rx * t.cos(), self.ry * t.sin());
        Point::new(
            self.c.x + self.cos_phi * ex - self.sin_phi * ey,
            self.c.y + self.sin_phi * ex + self.cos_phi * ey,
        )
    }

    fn tangent(&self, t: f64) -> Point {
        let (ex, ey) = (-self.rx * t.sin(), self.ry * t.cos());
        Point::new(
            self.cos_phi * ex - self.sin_phi * ey,
            self.sin_phi * ex + self.cos_phi * ey,
        )
    }
}

fn part(geom: Geometry, style: PartStyle) -> Part {
    Part { geom, style }
}

fn body_poly(pts: &[Point]) -> Part {
    body(Geometry::Path(geom::polygon(pts)))
}

fn shifted(pts: &[Point], dx: f64, dy: f64) -> Vec<Point> {
    pts.iter().map(|q| Point::new(q.x + dx, q.y + dy)).collect()
}

/// Local geometry of a bounding-box-sized shape around a `tw`×`th` label.
fn local(shape: Shape, tw: f64, th: f64, m: &ShapeMetrics) -> Option<Local> {
    let p = m.node_padding;
    let neo = m.neo;
    let pt = Point::new;
    let mut l = Local {
        parts: Vec::new(),
        glyphs: Vec::new(),
        clip: Vec::new(),
        label: Point::default(),
    };
    match shape {
        // folder.ts
        Shape::Folder => {
            let w = (tw + 2.0 * p).max(90.0);
            let content = th + 2.0 * p;
            let tab_h = (content * 0.16).clamp(8.0, 14.0);
            let total = content + tab_h;
            let tab_w = (w * 0.38).max(28.0);
            let top = -total / 2.0;
            let pts = vec![
                pt(-w / 2.0, top),
                pt(-w / 2.0 + tab_w, top),
                pt(-w / 2.0 + tab_w, top + tab_h),
                pt(w / 2.0, top + tab_h),
                pt(w / 2.0, total / 2.0),
                pt(-w / 2.0, total / 2.0),
            ];
            l.parts.push(body_poly(&pts));
            l.clip = pts;
            l.label = pt(0.0, top + tab_h + (total - tab_h) / 2.0);
        }
        // bucket.ts
        Shape::Bucket => {
            let w = (tw + 2.0 * p).max(80.0);
            let rim = (w * 0.08).clamp(5.0, 12.0);
            let total = th + 2.0 * p + rim;
            let top = -total / 2.0 + rim;
            let bottom = total / 2.0;
            let bw = w * 0.72;
            let arc = |rx: f64| Arc {
                rx,
                ry: rim,
                phi: 0.0,
                large: false,
                sweep: false,
            };
            let mut cmds = vec![
                PathCmd::MoveTo(pt(-w / 2.0, top)),
                PathCmd::LineTo(pt(-bw / 2.0, bottom)),
            ];
            svg_arc(
                &mut cmds,
                pt(-bw / 2.0, bottom),
                arc(bw / 2.0),
                pt(bw / 2.0, bottom),
            );
            cmds.push(PathCmd::LineTo(pt(w / 2.0, top)));
            svg_arc(&mut cmds, pt(w / 2.0, top), arc(w / 2.0), pt(-w / 2.0, top));
            cmds.push(PathCmd::Close);
            l.parts.push(body(Geometry::Path(cmds)));
            l.parts.push(part(
                Geometry::Path(geom::ellipse(pt(0.0, top), w / 2.0, rim)),
                PartStyle::AccentLine,
            ));
            let half_ellipse = |rx: f64, cy: f64, sign: f64| -> Vec<Point> {
                (0..=12)
                    .map(|i| {
                        let t = PI - i as f64 * PI / 12.0;
                        pt(rx * t.cos(), cy + sign * rim * t.sin())
                    })
                    .collect()
            };
            l.clip = half_ellipse(w / 2.0, top, -1.0);
            l.clip
                .extend(half_ellipse(bw / 2.0, bottom, 1.0).into_iter().rev());
            l.label = pt(0.0, top + (bottom - top) / 2.0);
        }
        // console.ts / browser.ts: a rounded window with a decorated top band.
        Shape::Console | Shape::Browser => {
            let band = if shape == Shape::Console { 20.0 } else { 18.0 };
            let w = (tw + 2.0 * p).max(90.0);
            let h = th + 2.0 * p + band;
            let top = -h / 2.0;
            l.parts.push(body(Geometry::Rect {
                rect: Rect::new(-w / 2.0, top, w, h),
                rx: 12.0,
            }));
            if shape == Shape::Console {
                l.glyphs.push(Glyph {
                    at: pt(-w / 2.0 + 12.0, top + 16.0),
                    text: ">_",
                    font_size: 14.0,
                    bold: true,
                });
            } else {
                l.parts.push(part(
                    Geometry::Path(seg(pt(-w / 2.0, top + band), pt(w / 2.0, top + band))),
                    PartStyle::AccentLine,
                ));
                for i in 0..3 {
                    l.parts.push(part(
                        Geometry::Circle {
                            c: pt(-w / 2.0 + 12.0 + 9.0 * i as f64, top + band / 2.0),
                            r: 2.5,
                        },
                        PartStyle::AccentDot,
                    ));
                }
                l.parts.push(part(
                    Geometry::Rect {
                        rect: Rect::new(
                            -w / 2.0 + 44.0,
                            top + 4.0,
                            (w - 56.0).max(10.0),
                            band - 8.0,
                        ),
                        rx: 3.0,
                    },
                    PartStyle::AccentFaint,
                ));
            }
            l.label = pt(0.0, top + band + (h - band) / 2.0);
        }
        // person.ts
        Shape::Person => {
            let w = (tw + 2.0 * p).max(100.0);
            let head = (w * 0.23).clamp(16.0, 56.0);
            let overlap = head * 0.27;
            let body_h = th + 2.0 * p;
            let body_r = (w * 0.177).min(body_h * 0.45);
            let total = body_h + 2.0 * head - overlap;
            let top = -total / 2.0;
            let body_top = top + 2.0 * head - overlap;
            l.parts.push(body(Geometry::Rect {
                rect: Rect::new(-w / 2.0, body_top, w, body_h),
                rx: body_r,
            }));
            let head_cy = top + head;
            l.parts.push(body(Geometry::Circle {
                c: pt(0.0, head_cy),
                r: head,
            }));
            let phi = ((body_top - head_cy) / head).min(1.0).asin().to_degrees();
            let mut clip = circle_points(0.0, -head_cy, head, 24, 180.0 + phi, -phi);
            let (lx, rx) = (-(-w / 2.0 + body_r), -(w / 2.0 - body_r));
            let (ty, by) = (-(body_top + body_r), -(total / 2.0 - body_r));
            clip.extend(circle_points(lx, ty, body_r, 12, 90.0, 0.0));
            clip.extend(circle_points(lx, by, body_r, 12, 360.0, 270.0));
            clip.extend(circle_points(rx, by, body_r, 12, 270.0, 180.0));
            clip.extend(circle_points(rx, ty, body_r, 12, 180.0, 90.0));
            l.clip = clip;
            l.label = pt(0.0, body_top + body_h / 2.0);
        }
        // bang.ts / cloud.ts
        Shape::Bang | Shape::Cloud => {
            let (start, arcs) = if shape == Shape::Bang {
                bang_arcs(tw, th, p)
            } else {
                cloud_arcs(tw, th, p)
            };
            l.parts
                .push(body(Geometry::Path(relative_arcs(start, &arcs))));
        }
        // curlyBraceLeft.ts / curlyBraceRight.ts / curlyBraces.ts: open braces drawn
        // over an invisible hit polygon, the label pushed to the left.
        Shape::BraceLeft | Shape::BraceRight | Shape::Braces => {
            let (px, py) = if neo { (18.0, 12.0) } else { (p, p) };
            let k = if neo && shape != Shape::BraceLeft {
                2.0
            } else {
                1.0
            };
            let (w, h) = (tw + k * px, th + k * py);
            let r = (h * 0.1).max(5.0);
            let neg = circle_points;
            let left = || {
                let mut v = neg(w / 2.0, -h / 2.0, r, 30, -90.0, 0.0);
                v.push(pt(-w / 2.0 - r, r));
                v.extend(neg(w / 2.0 + 2.0 * r, -r, r, 20, -180.0, -270.0));
                v.extend(neg(w / 2.0 + 2.0 * r, r, r, 20, -90.0, -180.0));
                v.push(pt(-w / 2.0 - r, -h / 2.0));
                v.extend(neg(w / 2.0, h / 2.0, r, 20, 0.0, 90.0));
                v
            };
            // Label top-left corner: brace-l `(-w/2 + r, -h/2 + p/2)`, the others
            // `(-w/2 + p/2, -h/2 + p/2)`.
            let label_x = if shape == Shape::BraceLeft {
                -w / 2.0 + r
            } else {
                -w / 2.0 + p / 2.0
            };
            l.label = pt(label_x + tw / 2.0, -h / 2.0 + p / 2.0 + th / 2.0);
            let (braces, rect, dx) = match shape {
                Shape::BraceLeft => {
                    let mut rect = vec![pt(w / 2.0, -h / 2.0 - r), pt(-w / 2.0, -h / 2.0 - r)];
                    rect.extend(neg(w / 2.0, -h / 2.0, r, 20, -90.0, 0.0));
                    rect.push(pt(-w / 2.0 - r, -r));
                    rect.extend(neg(w / 2.0 + w * 0.1, -r, r, 20, -180.0, -270.0));
                    rect.extend(neg(w / 2.0 + w * 0.1, r, r, 20, -90.0, -180.0));
                    rect.push(pt(-w / 2.0 - r, h / 2.0));
                    rect.extend(neg(w / 2.0, h / 2.0, r, 20, 0.0, 90.0));
                    rect.push(pt(-w / 2.0, h / 2.0 + r));
                    rect.push(pt(w / 2.0, h / 2.0 + r));
                    (vec![left()], rect, r)
                }
                Shape::BraceRight => {
                    let pos = circle_points_pos;
                    let mut brace = pos(w / 2.0, -h / 2.0, r, 20, -90.0, 0.0);
                    brace.push(pt(w / 2.0 + r, -r));
                    brace.extend(pos(w / 2.0 + 2.0 * r, -r, r, 20, -180.0, -270.0));
                    brace.extend(pos(w / 2.0 + 2.0 * r, r, r, 20, -90.0, -180.0));
                    brace.push(pt(w / 2.0 + r, h / 2.0));
                    brace.extend(pos(w / 2.0, h / 2.0, r, 20, 0.0, 90.0));
                    let mut rect = vec![pt(-w / 2.0, -h / 2.0 - r), pt(w / 2.0, -h / 2.0 - r)];
                    rect.extend(pos(w / 2.0, -h / 2.0, r, 20, -90.0, 0.0));
                    rect.push(pt(w / 2.0 + r, -r));
                    rect.extend(pos(w / 2.0 + 2.0 * r, -r, r, 20, -180.0, -270.0));
                    rect.extend(pos(w / 2.0 + 2.0 * r, r, r, 20, -90.0, -180.0));
                    rect.push(pt(w / 2.0 + r, h / 2.0));
                    rect.extend(pos(w / 2.0, h / 2.0, r, 20, 0.0, 90.0));
                    rect.push(pt(w / 2.0, h / 2.0 + r));
                    rect.push(pt(-w / 2.0, h / 2.0 + r));
                    (vec![brace], rect, -r)
                }
                _ => {
                    let x = -w / 2.0 + r + r / 2.0;
                    let mut right = neg(x, -h / 2.0, r, 20, -90.0, -180.0);
                    right.push(pt(w / 2.0 - r / 2.0, r));
                    right.extend(neg(-w / 2.0 - r / 2.0, -r, r, 20, 0.0, 90.0));
                    right.extend(neg(-w / 2.0 - r / 2.0, r, r, 20, -90.0, 0.0));
                    right.push(pt(w / 2.0 - r / 2.0, -r));
                    right.extend(neg(x, h / 2.0, r, 30, -180.0, -270.0));
                    let mut rect = vec![pt(w / 2.0, -h / 2.0 - r), pt(-w / 2.0, -h / 2.0 - r)];
                    rect.extend(neg(w / 2.0, -h / 2.0, r, 20, -90.0, 0.0));
                    rect.push(pt(-w / 2.0 - r, -r));
                    rect.extend(neg(w / 2.0 + 2.0 * r, -r, r, 20, -180.0, -270.0));
                    rect.extend(neg(w / 2.0 + 2.0 * r, r, r, 20, -90.0, -180.0));
                    rect.push(pt(-w / 2.0 - r, h / 2.0));
                    rect.extend(neg(w / 2.0, h / 2.0, r, 20, 0.0, 90.0));
                    rect.push(pt(-w / 2.0, h / 2.0 + r));
                    rect.push(pt(w / 2.0 - r - r / 2.0, h / 2.0 + r));
                    rect.extend(right.iter().copied());
                    (vec![left(), right], rect, r - r / 4.0)
                }
            };
            l.parts.push(part(
                Geometry::Path(geom::polygon(&shifted(&rect, dx, 0.0))),
                PartStyle::Hidden,
            ));
            for b in &braces {
                l.parts.push(line(geom::polyline(&shifted(b, dx, 0.0))));
            }
            l.clip = rect;
        }
        // lightningBolt.ts: 35×70, no label.
        Shape::Bolt => {
            let (w, h, gap) = (35.0, 35.0, 7.0);
            let pts = vec![
                pt(w, 0.0),
                pt(0.0, h + gap / 2.0),
                pt(w - 2.0 * gap, h + gap / 2.0),
                pt(0.0, 2.0 * h),
                pt(w, h - gap / 2.0),
                pt(2.0 * gap, h - gap / 2.0),
            ];
            l.parts.push(body_poly(&shifted(&pts, -w / 2.0, -h)));
            l.clip = pts;
        }
        // multiWaveEdgedRectangle.ts
        Shape::StackedDocument => {
            let (lpx, lpy) = if neo { (16.0, 12.0) } else { (p, p) };
            let (w, h) = (tw + 2.0 * lpx, th + 3.0 * lpy);
            let amp = if neo { h / 4.0 } else { h / 8.0 };
            let fh = h + amp / 2.0;
            let (x, y, o) = (-w / 2.0, -fh / 2.0, 10.0);
            let wave = sine_wave(x - o, y + fh + o, x + w - o, y + fh + o, amp, 0.8);
            let last = wave[wave.len() - 1];
            let mut outer = vec![pt(x - o, y + o), pt(x - o, y + fh + o)];
            outer.extend(wave);
            outer.extend([
                pt(x + w - o, last.y - o),
                pt(x + w, last.y - o),
                pt(x + w, last.y - 2.0 * o),
                pt(x + w + o, last.y - 2.0 * o),
                pt(x + w + o, y - o),
                pt(x + o, y - o),
                pt(x + o, y),
                pt(x, y),
                pt(x, y + o),
            ]);
            let inner = [
                pt(x, y + o),
                pt(x + w - o, y + o),
                pt(x + w - o, last.y - o),
                pt(x + w, last.y - o),
                pt(x + w, y),
                pt(x, y),
            ];
            l.parts.push(body_poly(&shifted(&outer, 0.0, -amp / 2.0)));
            l.parts.push(body_poly(&shifted(&inner, 0.0, -amp / 2.0)));
            l.clip = outer;
            l.label = pt(-o, o - amp);
        }
        // bowTieRect.ts
        Shape::BowTieRect => {
            let (lpx, lpy) = if neo { (16.0, 12.0) } else { (p, p) };
            let h = th + lpy;
            let ry = h / 2.0;
            let rx = ry / (2.5 + h / 50.0);
            let w = tw + 2.0 * lpx;
            let mut pts = vec![pt(w / 2.0, -h / 2.0), pt(-w / 2.0, -h / 2.0)];
            pts.extend(bow_arc_points(
                pt(-w / 2.0, -h / 2.0),
                pt(-w / 2.0, h / 2.0),
                rx,
                ry,
                false,
            ));
            pts.push(pt(w / 2.0, h / 2.0));
            pts.extend(bow_arc_points(
                pt(w / 2.0, h / 2.0),
                pt(w / 2.0, -h / 2.0),
                rx,
                ry,
                true,
            ));
            l.parts.push(body_poly(&shifted(&pts, rx / 2.0, 0.0)));
            l.clip = pts;
        }
        // taggedWaveEdgedRectangle.ts (no neo branch)
        Shape::TaggedDocument => {
            let (w, h) = (tw + 2.0 * p, th + 2.0 * p);
            let amp = h / 8.0;
            let (tag_w, tag_h) = (0.2 * w, 0.2 * h);
            let fh = h + amp;
            let ext = w / 2.0 * 0.1;
            let mut pts = vec![pt(-w / 2.0 - ext, fh / 2.0)];
            pts.extend(sine_wave(
                -w / 2.0 - ext,
                fh / 2.0,
                w / 2.0 + ext,
                fh / 2.0,
                amp,
                0.8,
            ));
            pts.extend([pt(w / 2.0 + ext, -fh / 2.0), pt(-w / 2.0 - ext, -fh / 2.0)]);
            let x = -w / 2.0 + ext;
            let y = -fh / 2.0 - tag_h * 0.4;
            let mut tag = vec![
                pt(x + w - tag_w, (y + h) * 1.3),
                pt(x + w, y + h - tag_h),
                pt(x + w, (y + h) * 0.9),
            ];
            tag.extend(sine_wave(
                x + w,
                (y + h) * 1.25,
                x + w - tag_w,
                (y + h) * 1.3,
                -h * 0.02,
                0.5,
            ));
            l.parts.push(body_poly(&shifted(&pts, 0.0, -amp / 2.0)));
            l.parts.push(body_poly(&shifted(&tag, 0.0, -amp / 2.0)));
            l.clip = pts;
            l.label = pt(0.0, -amp / 2.0);
        }
        // waveRectangle.ts
        Shape::PaperTape => {
            let (lpx, lpy) = if neo { (16.0, 20.0) } else { (p, p) };
            let (w, h) = (tw + 2.0 * lpx, th + lpy);
            let amp = h / 8.0;
            let fh = h + 2.0 * amp;
            let mut pts = vec![pt(-w / 2.0, fh / 2.0)];
            pts.extend(sine_wave(-w / 2.0, fh / 2.0, w / 2.0, fh / 2.0, amp, 1.0));
            pts.push(pt(w / 2.0, -fh / 2.0));
            pts.extend(sine_wave(
                w / 2.0,
                -fh / 2.0,
                -w / 2.0,
                -fh / 2.0,
                amp,
                -1.0,
            ));
            l.parts.push(body_poly(&pts));
            l.clip = pts;
        }
        // linedWaveEdgedRect.ts: the frame line is part of the outline polygon.
        Shape::LinedDocument => {
            let (lpx, lpy) = if neo { (16.0, 12.0) } else { (p, p) };
            let (w, h) = (tw + 2.0 * lpx, th + 2.0 * lpy);
            let amp = if neo { h / 4.0 } else { h / 8.0 };
            let fh = h + amp;
            let ext = w / 2.0 * 0.1;
            let mut pts = vec![pt(-w / 2.0 - ext, -fh / 2.0), pt(-w / 2.0 - ext, fh / 2.0)];
            pts.extend(sine_wave(
                -w / 2.0 - ext,
                fh / 2.0,
                w / 2.0 + ext,
                fh / 2.0,
                amp,
                0.8,
            ));
            pts.extend([
                pt(w / 2.0 + ext, -fh / 2.0),
                pt(-w / 2.0 - ext, -fh / 2.0),
                pt(-w / 2.0, -fh / 2.0),
                pt(-w / 2.0, fh / 2.0 * 1.1),
                pt(-w / 2.0, -fh / 2.0),
            ]);
            l.parts.push(body_poly(&shifted(&pts, 0.0, -amp / 2.0)));
            l.clip = pts;
            l.label = pt(
                -w / 2.0 + p + ext / 2.0 + tw / 2.0,
                -h / 2.0 + p - amp + th / 2.0,
            );
        }
        _ => return None,
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cylinder_clips_as_cylinder_ts() {
        // A 144 x 85.15 cylinder with 13.38 caps, met on its side from above:
        // the box's crossing moved down by the whole cap.
        let r = Rect::centered(
            Point::new(370.0, 212.57434844970703),
            144.0,
            85.14869888475836,
        );
        let b = Boundary::Cylinder(r, 13.382899628252789);
        let p = b.clip(Point::new(503.0, 148.29));
        assert!((p.x - 442.0).abs() < 1e-9, "{p:?}");
        assert!((p.y - 191.155).abs() < 2e-2, "{p:?}");
        // Through the top cap: the ellipse's depth there.
        let top = b.clip(Point::new(370.0, 0.0));
        assert!((top.y - r.y).abs() < 1e-9, "{top:?}");
    }

    fn sized(shape: Shape, tw: f64, th: f64, p: f64) -> ShapeSize {
        size(shape, tw, th, &ShapeMetrics::classic(shape, p), false).unwrap()
    }

    fn sized_neo(shape: Shape, tw: f64, th: f64, p: f64) -> ShapeSize {
        size(shape, tw, th, &ShapeMetrics::neo(shape, p), false).unwrap()
    }

    #[test]
    fn neo_look_uses_its_own_label_padding() {
        // squareRect.ts neo: 16 / 12 per side.
        let r = sized_neo(Shape::Rect, 100.0, 24.0, 15.0);
        assert_eq!((r.w, r.h), (132.0, 48.0));
        // roundedRect has no neo branch.
        assert_eq!(
            sized_neo(Shape::Rounded, 100.0, 24.0, 15.0),
            sized(Shape::Rounded, 100.0, 24.0, 15.0)
        );
        // circle.ts neo: radius = hypot/2 + 16.
        let c = sized_neo(Shape::Circle, 30.0, 40.0, 15.0);
        assert_eq!(c.w, 50.0 + 32.0);
        // doubleCircle.ts neo: inner ring as circle, gap 12.
        let d = sized_neo(Shape::DoubleCircle, 30.0, 40.0, 15.0);
        assert_eq!(d.w, 50.0 + 32.0 + 24.0);
        // hexagon.ts neo: h = th + 70, m = h / 3.5, w = tw + 2m + 32.
        let h = sized_neo(Shape::Hexagon, 100.0, 24.0, 15.0);
        assert_eq!(h.h, 94.0);
        assert!((h.w - (100.0 + 2.0 * 94.0 / 3.5 + 32.0)).abs() < 1e-9);
        // card.ts neo: w = tw + 56, h = th + 48.
        let k = sized_neo(Shape::Card, 100.0, 24.0, 15.0);
        assert_eq!((k.w, k.h), (156.0, 72.0));
        // linedWaveEdgedRect.ts neo (agentflow refdoc): 120×21 label → 152×45 inner box,
        // amplitude 45/4, outer width 1.1·152.
        let l = sized_neo(Shape::LinedDocument, 120.0, 21.0, 15.0);
        assert!((l.w - 167.2).abs() < 1e-9);
        // The bottom of the sampled wave falls just short of the amplitude.
        assert!((l.h - 67.5).abs() < 0.05);
        // collapsedGroup.ts: at least 80 wide, title + 28 + 16 high.
        let g = sized_neo(Shape::CollapsedGroup, 30.0, 21.0, 15.0);
        assert_eq!((g.w, g.h), (80.0, 65.0));
    }

    #[test]
    fn flowchart_rect_uses_square_rect_padding() {
        // squareRect.ts with padding 15: width = tw + 2·(2·15), height = th + 2·15.
        let s = sized(Shape::Rect, 100.0, 24.0, 15.0);
        assert_eq!((s.w, s.h), (160.0, 54.0));
        let r = sized(Shape::Rounded, 100.0, 24.0, 15.0);
        assert_eq!((r.w, r.h), (130.0, 54.0));
    }

    #[test]
    fn diamond_and_circle_sizes() {
        // question.ts: s = (tw + p) + (th + p).
        let d = sized(Shape::Diamond, 80.0, 24.0, 15.0);
        assert_eq!((d.w, d.h), (134.0, 134.0));
        // circle.ts: r = hypot(tw, th)/2 + p/2.
        let c = sized(Shape::Circle, 30.0, 40.0, 15.0);
        assert_eq!(c.w, 65.0);
        // doubleCircle.ts: outer r = hypot/2 + p + 5.
        let dc = sized(Shape::DoubleCircle, 30.0, 40.0, 15.0);
        assert_eq!(dc.w, 2.0 * (25.0 + 15.0 + 5.0));
    }

    #[test]
    fn cylinder_cap_matches_upstream() {
        let s = sized(Shape::Cylinder, 85.0, 24.0, 15.0);
        // w = tw + p = 100 → ry = 50 / (2.5 + 2) = 11.11…; h = th + p + 3·ry.
        assert_eq!(s.w, 100.0);
        let ry = 50.0 / 4.5;
        assert!((s.h - (39.0 + 3.0 * ry)).abs() < 1e-9);
        assert!((s.label_offset.y - 10.0).abs() < 1e-9);
    }

    #[test]
    fn hexagon_and_lean_offsets() {
        let h = sized(Shape::Hexagon, 100.0, 24.0, 15.0);
        assert_eq!((h.w, h.h), (100.0 + 15.0 + 19.5, 39.0));
        let l = sized(Shape::LeanRight, 100.0, 24.0, 15.0);
        assert_eq!((l.w, l.h), (154.0, 39.0));
        let t = sized(Shape::InvTrapezoid, 100.0, 24.0, 15.0);
        assert_eq!((t.w, t.h), (184.0, 54.0));
    }

    fn neo_metrics(shape: Shape, tw: f64, th: f64) -> ShapeMetrics {
        ShapeMetrics::neo(shape, 15.0).with_label(tw, th)
    }

    #[test]
    fn bbox_sized_shapes_match_upstream() {
        // Reference values from mermaid 12.3 (neo look, padding 15, 120×21 label).
        let s = |shape| sized_neo(shape, 120.0, 21.0, 15.0);
        // folder.ts: 150 wide, tab 8.16 → 59.16 high.
        let f = s(Shape::Folder);
        assert!((f.w - 150.0).abs() < 1e-9 && (f.h - 59.16).abs() < 1e-9);
        assert!((f.label_offset.y - 4.08).abs() < 1e-9);
        // bucket.ts: rim 12, the base arc bulges another 12 below the box.
        let b = s(Shape::Bucket);
        assert!((b.w - 150.0).abs() < 1e-6 && (b.h - 75.0).abs() < 1e-6);
        // console.ts / browser.ts: 20 / 18 px bands.
        assert_eq!((s(Shape::Console).w, s(Shape::Console).h), (150.0, 71.0));
        assert_eq!(s(Shape::Browser).h, 69.0);
        // person.ts: head r 34.5, body 51 → 51 + 69 - 9.315.
        assert!((s(Shape::Person).h - 110.685).abs() < 1e-9);
        // lightningBolt.ts: fixed 35×70 whatever the label.
        assert_eq!((s(Shape::Bolt).w, s(Shape::Bolt).h), (35.0, 70.0));
        // curlyBraceLeft.ts neo: w = 138, h = 33, r = 5 → hit polygon 138/2 + 0.6·138 wide.
        let l = s(Shape::BraceLeft);
        assert!((l.w - (69.0 + 0.6 * 138.0)).abs() < 1e-6);
        assert!((l.h - 43.0).abs() < 1e-9);
        assert!((l.label_offset.x - (-64.0 + 60.0)).abs() < 1e-9);
        // waveRectangle.ts neo: w = 152, h = 41, amplitude 5.125.
        let t = s(Shape::PaperTape);
        assert!((t.w - 152.0).abs() < 1e-9);
        assert!(t.h > 41.0 && t.h <= 41.0 + 4.0 * 5.125);
        // bowTieRect.ts neo: w = 152 plus the left bulge rx.
        let rx = 16.5 / (2.5 + 33.0 / 50.0);
        // (20 sampled arc points stop just short of the bulge.)
        assert!((s(Shape::BowTieRect).w - (152.0 + rx)).abs() < 0.05);
    }

    #[test]
    fn svg_arc_matches_endpoint_parameterisation() {
        // A half circle from (-1,0) to (1,0) bulging down (sweep 0 goes through +y).
        let mut cmds = vec![PathCmd::MoveTo(Point::new(-1.0, 0.0))];
        let a = Arc {
            rx: 1.0,
            ry: 1.0,
            phi: 0.0,
            large: false,
            sweep: false,
        };
        svg_arc(&mut cmds, Point::new(-1.0, 0.0), a, Point::new(1.0, 0.0));
        let mut pts = Vec::new();
        geometry_points(&Geometry::Path(cmds), &mut pts);
        let max_y = pts.iter().map(|q| q.y).fold(f64::MIN, f64::max);
        assert!((max_y - 1.0).abs() < 1e-3, "{max_y}");
        // Radii too small for the chord are scaled up.
        let mut cmds = vec![PathCmd::MoveTo(Point::new(0.0, 0.0))];
        svg_arc(
            &mut cmds,
            Point::new(0.0, 0.0),
            Arc {
                rx: 0.1,
                ry: 0.1,
                ..a
            },
            Point::new(4.0, 0.0),
        );
        let mut pts = Vec::new();
        geometry_points(&Geometry::Path(cmds), &mut pts);
        let max_y = pts.iter().map(|q| q.y).fold(f64::MIN, f64::max);
        assert!((max_y - 2.0).abs() < 1e-2, "{max_y}");
    }

    #[test]
    fn bbox_shapes_draw_their_local_geometry_around_the_centre() {
        for shape in [
            Shape::Folder,
            Shape::Bucket,
            Shape::Console,
            Shape::Browser,
            Shape::Person,
            Shape::Bang,
            Shape::Cloud,
            Shape::BraceLeft,
            Shape::BraceRight,
            Shape::Braces,
            Shape::Bolt,
            Shape::StackedDocument,
            Shape::BowTieRect,
            Shape::TaggedDocument,
            Shape::PaperTape,
            Shape::LinedDocument,
        ] {
            let m = neo_metrics(shape, 80.0, 21.0);
            let s = size(shape, 80.0, 21.0, &m, false).unwrap();
            let c = Point::new(500.0, 300.0);
            let o = outline(shape, c, s.w, s.h, &m).unwrap();
            let mut pts = Vec::new();
            for part in &o.parts {
                geometry_points(&part.geom, &mut pts);
            }
            let w = pts.iter().map(|q| q.x).fold(f64::MIN, f64::max)
                - pts.iter().map(|q| q.x).fold(f64::MAX, f64::min);
            assert!((w - s.w).abs() < 1e-6, "{shape:?}");
            // Edges coming from far away stop on the outline, not at the centre.
            let hit = o.boundary.clip(Point::new(c.x - 1000.0, c.y));
            assert!(hit.x < c.x - 5.0, "{shape:?} {hit:?}");
        }
    }

    #[test]
    fn every_sized_shape_has_an_outline() {
        let m = ShapeMetrics::classic(Shape::Rect, 15.0);
        for s in [
            Shape::Rect,
            Shape::Stadium,
            Shape::Subroutine,
            Shape::Cylinder,
            Shape::Card,
            Shape::Document,
            Shape::LinedDocument,
            Shape::CollapsedGroup,
            Shape::CurvedTrapezoid,
            Shape::HorizontalCylinder,
            Shape::StackedRect,
            Shape::Delay,
        ] {
            assert!(
                outline(s, Point::default(), 100.0, 50.0, &m).is_some(),
                "{s:?}"
            );
        }
    }
}
