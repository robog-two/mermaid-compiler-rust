//! Node-shape geometry for the graph-family renderers, transcribed from
//! mermaid's `rendering-util/rendering-elements/shapes/*.ts` (classic look).
//!
//! Sizing is split from drawing: [`ShapeMetrics`] carries every look-dependent
//! padding/factor explicitly, [`size`] turns a measured label into the node's
//! outer box, and [`outline`] draws the shape inside that box.

use super::graph::Shape;
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
}

/// `subroutine.ts` / `shadedProcess.ts` FRAME_WIDTH.
const FRAME: f64 = 8.0;
/// `card.ts` NOTCH_SIZE.
const NOTCH: f64 = 12.0;
/// `windowPane.ts` rectOffset.
const PANE: f64 = 10.0;
/// `taggedRect.ts` TAG_RATIO.
const TAG_RATIO: f64 = 0.2;

impl ShapeMetrics {
    /// Classic-look values (`node.look !== 'neo'`) for `shape` with `node.padding = p`.
    pub fn classic(shape: Shape, p: f64) -> Self {
        let pad = |x: f64, y: f64| LabelPad { x, y };
        let pad = match shape {
            // squareRect.ts: labelPaddingX = 2p on each side, labelPaddingY = p.
            Shape::Rect | Shape::State => pad(4.0 * p, 2.0 * p),
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
        Shape::Rect | Shape::State | Shape::Rounded | Shape::Note | Shape::Text => {
            at(tw + px, th + py)
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
        Shape::StateStart | Shape::StateEnd => at(14.0, 14.0),
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
            | Shape::StateEnd
            | Shape::ForkBar
            | Shape::Choice
            | Shape::Invisible
            | Shape::Hourglass
            | Shape::CrossedCircle
    )
}

/// Clipping boundary of a drawn node.
#[derive(Debug, Clone, PartialEq)]
pub enum Boundary {
    Rect(Rect),
    Ellipse(Point, f64, f64),
    Polygon(Point, Vec<Point>),
}

impl Boundary {
    pub fn clip(&self, toward: Point) -> Point {
        match self {
            Boundary::Rect(r) => geom::clip_rect(r, toward),
            Boundary::Ellipse(c, rx, ry) => geom::clip_ellipse(*c, *rx, *ry, toward),
            Boundary::Polygon(c, pts) => geom::clip_polygon(*c, pts, toward),
        }
    }
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
    };
    Some(match shape {
        Shape::Rect | Shape::Note => rect(0.0),
        // roundedRect.ts: themeVariables.radius (5); state rects get `rx: 5px` from CSS.
        Shape::Rounded => rect(m.corner_radius),
        Shape::State => rect(5.0),
        Shape::Text => Outline {
            parts: vec![Part {
                geom: Geometry::Rect { rect: r, rx: 0.0 },
                style: PartStyle::Hidden,
            }],
            boundary: Boundary::Rect(r),
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
                boundary: Boundary::Rect(r),
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
            }
        }
        Shape::Circle => Outline {
            parts: vec![body(Geometry::Circle { c, r: w / 2.0 })],
            boundary: Boundary::Ellipse(c, w / 2.0, h / 2.0),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(shape: Shape, tw: f64, th: f64, p: f64) -> ShapeSize {
        size(shape, tw, th, &ShapeMetrics::classic(shape, p), false).unwrap()
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
