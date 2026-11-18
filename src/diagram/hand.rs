//! mermaid's `look: handDrawn` node shapes: the rough.js calls each upstream
//! shape file makes in its `node.look === 'handDrawn'` branch, in the shape's
//! own frame (the node's centre at the origin).
//!
//! mermaid then sizes the node by the drawn group's `getBBox()`
//! (`updateNodeBounds`), which the random strokes push a few pixels past the
//! nominal box: [`HandNode::bbox`] is that box, and the layout uses its size.
//! The shapes' `intersect` functions keep their own mix of nominal geometry
//! and that size; [`HandNode::boundary`] follows each.

use super::graph::Shape;
use super::shapes::{Boundary, ShapeMetrics};
use crate::geom::{PathCmd, Point, Rect};
use crate::rough::{self, Drawable, Op, Options};

/// What `userNodeOverrides` reads from the node and the config.
#[derive(Debug, Clone, PartialEq)]
pub struct HandStyle {
    /// `fill` style, else `themeVariables.mainBkg`.
    pub fill: String,
    /// `stroke` style, else `themeVariables.nodeBorder`.
    pub stroke: String,
    /// `stroke-width` style, else 1.3.
    pub stroke_width: f64,
    /// `handDrawnSeed` (a zero seed already replaced, see [`rough::FALLBACK_SEED`]).
    pub seed: f64,
    /// `themeVariables.radius` (rounded rectangles).
    pub radius: f64,
    /// `themeVariables.lineColor` (state start and end, fork bars).
    pub line: String,
    /// `stateBorder ?? nodeBorder` (the state end's dot).
    pub end_dot: String,
    /// `noteBkgColor` and `noteBorderColor` (notes).
    pub note_fill: String,
    pub note_stroke: String,
    /// The graph is laid out across (fork bars stand upright).
    pub horizontal: bool,
    /// A state diagram's node (its states are `rx: 10` rects).
    pub state: bool,
}

/// One rough.js drawing placed at `at` in the node's frame.
#[derive(Debug, Clone)]
pub struct Piece {
    pub drawable: Drawable,
    pub at: Point,
    /// Counted in the box `updateNodeBounds` measures.
    pub measured: bool,
}

/// A hand-drawn node in its own frame.
#[derive(Debug, Clone)]
pub struct HandNode {
    pub pieces: Vec<Piece>,
    /// `getBBox()` of the measured group, in the node's frame.
    pub bbox: Rect,
    /// The size the layout gets, where the shape does not take the bbox's.
    pub size: Option<(f64, f64)>,
    boundary: Bound,
}

/// How a shape's `intersect` sees the node (`w`/`h` are its measured size).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bound {
    /// `intersect.rect(node)`.
    Rect,
    /// `intersect.circle(node, r)`.
    Circle(f64),
    /// `intersect.circle(node, node.width / 2)` (the measured width).
    MeasuredCircle,
    /// `intersect.polygon(node, points)` with points given in [`HandNode::poly`].
    Polygon,
    /// question.ts: a diamond as wide as the measured box.
    Diamond,
    /// cylinder.ts: the box's crossing moved by the nominal caps.
    Cylinder(f64, f64),
}

impl HandNode {
    /// The edge-clipping boundary of the node centred on `c` with measured
    /// size `w`×`h`.
    pub fn boundary(&self, shape: Shape, m: &ShapeMetrics, c: Point, w: f64, h: f64) -> Boundary {
        match self.boundary {
            Bound::Rect => Boundary::Rect(Rect::centered(c, w, h)),
            Bound::Circle(r) => Boundary::Ellipse(c, r, r),
            Bound::MeasuredCircle => Boundary::Ellipse(c, w / 2.0, w / 2.0),
            Bound::Diamond => {
                let s = w;
                let local = vec![
                    Point::new(s / 2.0, 0.0),
                    Point::new(s, -s / 2.0),
                    Point::new(s / 2.0, -s),
                    Point::new(0.0, -s / 2.0),
                ];
                Boundary::Polygon(c, in_box(c, w, h, local))
            }
            Bound::Polygon => Boundary::Polygon(c, in_box(c, w, h, poly(shape, m))),
            Bound::Cylinder(rx, ry) => Boundary::Cylinder(Rect::centered(c, w, h), rx, ry),
        }
    }
}

/// `intersect.polygon`'s placement: the points' least x and y on the box's
/// left and top sides.
fn in_box(c: Point, w: f64, h: f64, local: Vec<Point>) -> Vec<Point> {
    let min_x = local.iter().map(|q| q.x).fold(f64::INFINITY, f64::min);
    let min_y = local.iter().map(|q| q.y).fold(f64::INFINITY, f64::min);
    let (left, top) = (c.x - w / 2.0 - min_x, c.y - h / 2.0 - min_y);
    local
        .into_iter()
        .map(|q| Point::new(left + q.x, top + q.y))
        .collect()
}

/// The polygon a shape's `intersect` uses (its nominal points).
fn poly(shape: Shape, m: &ShapeMetrics) -> Vec<Point> {
    let (tw, th) = m.label;
    let p = m.node_padding;
    let pt = Point::new;
    match shape {
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid => {
            let h = th + p;
            let w = tw + p;
            match shape {
                Shape::LeanRight => vec![
                    pt(-3.0 * h / 6.0, 0.0),
                    pt(w, 0.0),
                    pt(w + 3.0 * h / 6.0, -h),
                    pt(0.0, -h),
                ],
                Shape::LeanLeft => vec![
                    pt(0.0, 0.0),
                    pt(w + 3.0 * h / 6.0, 0.0),
                    pt(w, -h),
                    pt(-(3.0 * h) / 6.0, -h),
                ],
                _ => vec![
                    pt(-3.0 * h / 6.0, 0.0),
                    pt(w + 3.0 * h / 6.0, 0.0),
                    pt(w, -h),
                    pt(0.0, -h),
                ],
            }
        }
        Shape::InvTrapezoid => {
            let h = th + 2.0 * p;
            let w = tw + 2.0 * p;
            vec![
                pt(0.0, 0.0),
                pt(w, 0.0),
                pt(w + 3.0 * h / 6.0, -h),
                pt(-3.0 * h / 6.0, -h),
            ]
        }
        Shape::Hexagon => {
            let h = th + p;
            let mm = h / 4.0;
            let w = tw + 2.0 * mm + p;
            vec![
                pt(mm, 0.0),
                pt(w - mm, 0.0),
                pt(w, -h / 2.0),
                pt(w - mm, -h),
                pt(mm, -h),
                pt(0.0, -h / 2.0),
            ]
        }
        Shape::Asymmetric => asym_points(tw, th, p).to_vec(),
        Shape::Subroutine => {
            let w = tw + p;
            let h = th + p;
            vec![
                pt(0.0, 0.0),
                pt(w, 0.0),
                pt(w, -h),
                pt(0.0, -h),
                pt(0.0, 0.0),
                pt(-8.0, 0.0),
                pt(w + 8.0, 0.0),
                pt(w + 8.0, -h),
                pt(-8.0, -h),
                pt(-8.0, 0.0),
            ]
        }
        Shape::Stadium => {
            let (w, h) = stadium_dims(tw, th, p);
            stadium_points(w, h)
        }
        Shape::Choice => vec![pt(0.0, 14.0), pt(14.0, 0.0), pt(0.0, -14.0), pt(-14.0, 0.0)],
        _ => Vec::new(),
    }
}

fn asym_points(tw: f64, th: f64, p: f64) -> [Point; 5] {
    let w = tw + p;
    let h = th + p;
    let (x, y) = (-w / 2.0, -h / 2.0);
    let notch = y / 2.0;
    [
        Point::new(x + notch, y),
        Point::new(x, 0.0),
        Point::new(x + notch, -y),
        Point::new(-x, -y),
        Point::new(-x, y),
    ]
}

/// stadium.ts `stadiumDimensions` (classic padding).
fn stadium_dims(tw: f64, th: f64, p: f64) -> (f64, f64) {
    let h = th + p;
    let inscribed = h * (std::f64::consts::PI / (2.0 * 49.0)).cos();
    let cap = (inscribed.powi(2) - th.powi(2)).max(0.0).sqrt();
    let w = (tw + h / 4.0 + p)
        .max(1.5 * h + (h - inscribed))
        .max(tw + p + h - cap);
    (w, h)
}

/// util.ts `generateCirclePoints` (negated, as upstream).
fn circle_points(cx: f64, cy: f64, r: f64, n: usize, a0: f64, a1: f64) -> Vec<Point> {
    let (s, e) = (
        a0 * std::f64::consts::PI / 180.0,
        a1 * std::f64::consts::PI / 180.0,
    );
    let step = (e - s) / (n - 1) as f64;
    (0..n)
        .map(|i| {
            let a = s + i as f64 * step;
            Point::new(-(cx + r * a.cos()), -(cy + r * a.sin()))
        })
        .collect()
}

fn stadium_points(w: f64, h: f64) -> Vec<Point> {
    let r = h / 2.0;
    let mut pts = vec![
        Point::new(-w / 2.0 + r, -h / 2.0),
        Point::new(w / 2.0 - r, -h / 2.0),
    ];
    pts.extend(circle_points(-w / 2.0 + r, 0.0, r, 50, 90.0, 270.0));
    pts.push(Point::new(w / 2.0 - r, h / 2.0));
    pts.extend(circle_points(w / 2.0 - r, 0.0, r, 50, 270.0, 450.0));
    pts
}

/// util.ts `createPathFromPoints`.
fn path_from_points(pts: &[Point]) -> String {
    let mut s: Vec<String> = pts
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}{},{}", if i == 0 { 'M' } else { 'L' }, p.x, p.y))
        .collect();
    s.push("Z".into());
    s.join(" ")
}

/// roundedRectPath.ts `createRoundedRectPathD`.
pub fn rounded_rect_path(x: f64, y: f64, w: f64, h: f64, r: f64) -> String {
    format!(
        "M {} {} H {} A {r} {r} 0 0 1 {} {} V {} A {r} {r} 0 0 1 {} {} H {} A {r} {r} 0 0 1 {} {} V {} A {r} {r} 0 0 1 {} {} Z",
        x + r,
        y,
        x + w - r,
        x + w,
        y + r,
        y + h - r,
        x + w - r,
        y + h,
        x + r,
        x,
        y + h - r,
        y + r,
        x + r,
        y,
    )
}

/// What a cluster's hand-drawn frame depends on besides its rect (its paint
/// leaves the geometry alone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStyle {
    pub seed: f64,
    pub stroke_width: f64,
}

/// clusters.js `rect` under the handDrawn look: `rc.path` of the frame
/// (`roughness: 0.7`, a hachure fill at `fillWeight: 3`), with `r` in the
/// frame mermaid draws it in (the hachure's scan lines are rounded there).
pub fn cluster_frame(r: Rect, style: FrameStyle) -> Drawable {
    let o = Options {
        roughness: 0.7,
        // Any fill: only that there is one shapes the drawing.
        fill: Some("#000".into()),
        fill_weight: 3.0,
        hachure_gap: 1.5,
        seed: style.seed,
        stroke_width: style.stroke_width,
        ..Options::default()
    };
    crate::rough::path(&rounded_rect_path(r.x, r.y, r.w, r.h, 0.0), o)
}

/// `getBBox()` of the group rough.js draws `d` as.
pub fn drawable_bbox(d: &Drawable) -> Option<Rect> {
    let paths: Vec<Vec<PathCmd>> = d
        .sets
        .iter()
        .filter(|s| !s.ops.is_empty())
        .map(|s| set_cmds(&s.ops, Point::default()))
        .collect();
    let [x, y, w, h] = super::svgbbox::cubic_group_box(&paths)?;
    Some(Rect::new(x, y, w, h))
}

/// `userNodeOverrides(node, {})`.
pub fn node_options(st: &HandStyle) -> Options {
    Options {
        roughness: 0.7,
        fill: Some(st.fill.clone()),
        fill_style: "hachure".into(),
        fill_weight: 1.5,
        hachure_gap: 1.5,
        stroke: st.stroke.clone(),
        seed: st.seed,
        stroke_width: st.stroke_width,
        ..Options::default()
    }
}

fn piece(drawable: Drawable, at: Point) -> Piece {
    Piece {
        drawable,
        at,
        measured: true,
    }
}

/// The path commands of one op set.
pub fn set_cmds(ops: &[Op], at: Point) -> Vec<PathCmd> {
    ops.iter()
        .map(|op| match *op {
            Op::Move([x, y]) => PathCmd::MoveTo(Point::new(x + at.x, y + at.y)),
            Op::LineTo([x, y]) => PathCmd::LineTo(Point::new(x + at.x, y + at.y)),
            Op::BCurveTo([a, b, c, d, e, f]) => PathCmd::CubicTo(
                Point::new(a + at.x, b + at.y),
                Point::new(c + at.x, d + at.y),
                Point::new(e + at.x, f + at.y),
            ),
        })
        .collect()
}

/// The hand-drawn `shape` around a label of `m.label`, or `None` for shapes
/// not drawn here.
pub fn node(shape: Shape, m: &ShapeMetrics, st: &HandStyle) -> Option<HandNode> {
    let (tw, th) = m.label;
    let p = m.node_padding;
    let o = || node_options(st);
    let origin = Point::default();
    let (pieces, boundary) = match shape {
        // drawRect (squareRect: labelPadding 2p / p): rc.rectangle without rx.
        Shape::Rect => {
            let (w, h) = (tw + 4.0 * p, th + 2.0 * p);
            let d = rough::rectangle(-w / 2.0, -h / 2.0, w, h, o());
            (vec![piece(d, origin)], Bound::Rect)
        }
        // A state: roundedRect (`themeVariables.radius ?? 5`, `padding` 8
        // round the label), a rough path.
        Shape::Rounded if st.state => {
            let (w, h) = (tw + 2.0 * p, th + 2.0 * p);
            let path = rounded_rect_path(-w / 2.0, -h / 2.0, w, h, st.radius);
            (vec![piece(rough::path(&path, o()), origin)], Bound::Rect)
        }
        // roundedRect: drawRect with rx = ry = radius, a rough path.
        Shape::Rounded => {
            let (w, h) = (tw + 2.0 * p, th + 2.0 * p);
            let path = rounded_rect_path(-w / 2.0, -h / 2.0, w, h, st.radius);
            (vec![piece(rough::path(&path, o()), origin)], Bound::Rect)
        }
        // question.ts: the diamond path in a frame moved by (-s/2 + 0.5, s/2).
        Shape::Diamond => {
            let s = tw + p + th + p;
            let path = format!(
                "M{},{} L{},{} L{},{} L{},{} Z",
                s / 2.0,
                0.0,
                s,
                -s / 2.0,
                s / 2.0,
                -s,
                0.0,
                -s / 2.0
            );
            let at = Point::new(-s / 2.0 + 0.5, s / 2.0);
            (vec![piece(rough::path(&path, o()), at)], Bound::Diamond)
        }
        // circle.ts: radius = half the label's diagonal + p/2.
        Shape::Circle => {
            let r = tw.hypot(th) / 2.0 + p / 2.0;
            let d = rough::circle(0.0, 0.0, r * 2.0, o());
            (vec![piece(d, origin)], Bound::Circle(r))
        }
        // doubleCircle.ts: two circles of roughness 0.2, stroked 2.5 and 1.5.
        Shape::DoubleCircle => {
            let inner = tw.hypot(th) / 2.0 + p;
            let outer = inner + 5.0;
            let oo = Options {
                roughness: 0.2,
                stroke_width: 2.5,
                ..o()
            };
            let io = Options {
                roughness: 0.2,
                stroke_width: 1.5,
                ..o()
            };
            (
                vec![
                    piece(rough::circle(0.0, 0.0, outer * 2.0, oo), origin),
                    piece(rough::circle(0.0, 0.0, inner * 2.0, io), origin),
                ],
                Bound::Circle(outer),
            )
        }
        // stadium.ts: its 50-point arcs as one rough path.
        Shape::Stadium => {
            let (w, h) = stadium_dims(tw, th, p);
            let path = path_from_points(&stadium_points(w, h));
            (vec![piece(rough::path(&path, o()), origin)], Bound::Polygon)
        }
        // subroutine.ts: a rectangle 16 wider than the body and its two
        // frame lines; only the rectangle is measured.
        Shape::Subroutine => {
            let tot_w = tw + 16.0 + p;
            let h = th + p;
            let w = tot_w - 16.0;
            let (x, y) = (-tot_w / 2.0, -h / 2.0);
            let rect = rough::rectangle(x, y, w + 16.0, h, o());
            let l1 = rough::line(x + 8.0, y, x + 8.0, y + h, o());
            let l2 = rough::line(x + 8.0 + w, y, x + 8.0 + w, y + h, o());
            (
                vec![
                    piece(rect, origin),
                    Piece {
                        drawable: l2,
                        at: origin,
                        measured: false,
                    },
                    Piece {
                        drawable: l1,
                        at: origin,
                        measured: false,
                    },
                ],
                Bound::Polygon,
            )
        }
        // rectLeftInvArrow.ts: always rough, moved by (-notch/2, 0).
        Shape::Asymmetric => {
            let pts = asym_points(tw, th, p);
            let notch = (-(th + p) / 2.0) / 2.0;
            let d = rough::path(&path_from_points(&pts), o());
            (
                vec![piece(d, Point::new(-notch / 2.0, 0.0))],
                Bound::Polygon,
            )
        }
        Shape::Hexagon => {
            let h = th + p;
            let mm = h / 4.0;
            let w = tw + 2.0 * mm + p;
            let path = format!(
                "M{},{} L{},{} L{},{} L{},{} L{},{} L{},{} Z",
                mm,
                0.0,
                w - mm,
                0.0,
                w,
                -h / 2.0,
                w - mm,
                -h,
                mm,
                -h,
                0.0,
                -h / 2.0
            );
            let d = rough::path(&path, o());
            (
                vec![piece(d, Point::new(-w / 2.0, h / 2.0))],
                Bound::Polygon,
            )
        }
        Shape::LeanRight | Shape::LeanLeft | Shape::Trapezoid | Shape::InvTrapezoid => {
            let pts = poly(shape, m);
            let (w, h) = if shape == Shape::InvTrapezoid {
                (tw + 2.0 * p, th + 2.0 * p)
            } else {
                (tw + p, th + p)
            };
            let d = rough::path(&path_from_points(&pts), o());
            (
                vec![piece(d, Point::new(-w / 2.0, h / 2.0))],
                Bound::Polygon,
            )
        }
        // cylinder.ts: the outline without its top's front half, which is a
        // separate unfilled path left where the shape's frame starts (only the
        // outline is moved, and measured).
        Shape::Cylinder => {
            let w = tw + p;
            let rx = w / 2.0;
            let ry = rx / (2.5 + w / 50.0);
            let h = th + p + ry;
            let outer = format!(
                "M{},{} M{},{} a{},{} 0,0,0 {},0 l0,{} a{},{} 0,0,0 {},0 l0,{}",
                0.0, ry, w, ry, rx, ry, -w, h, rx, ry, w, -h
            );
            let inner = format!("M{},{} a{},{} 0,0,0 {},0", -w / 2.0, -h / 2.0, rx, ry, w);
            let io = Options {
                fill: Some("none".into()),
                ..o()
            };
            (
                vec![
                    piece(
                        rough::path(&outer, o()),
                        Point::new(-w / 2.0, -(h / 2.0 + ry)),
                    ),
                    Piece {
                        drawable: rough::path(&inner, io),
                        at: origin,
                        measured: false,
                    },
                ],
                Bound::Cylinder(rx, ry),
            )
        }
        // stateStart.ts: `rc.circle(0, 0, 14, solidStateFill(lineColor))`.
        Shape::StateStart | Shape::SmallCircle => {
            let so = Options {
                fill: Some(st.line.clone()),
                hachure_angle: 120.0,
                hachure_gap: 1.5,
                fill_weight: 1.5,
                roughness: 0.7,
                stroke: st.line.clone(),
                seed: st.seed,
                ..Options::default()
            };
            (
                vec![piece(rough::circle(0.0, 0.0, 14.0, so), origin)],
                Bound::MeasuredCircle,
            )
        }
        // stateEnd.ts: a 14px ring stroked `lineColor` at 2 round a solid 5px
        // dot, one measured group.
        Shape::StateEnd => {
            let oo = Options {
                stroke: st.line.clone(),
                stroke_width: 2.0,
                ..o()
            };
            let io = Options {
                fill: Some(st.end_dot.clone()),
                stroke: st.end_dot.clone(),
                stroke_width: 2.0,
                fill_style: "solid".into(),
                ..o()
            };
            (
                vec![
                    piece(rough::circle(0.0, 0.0, 14.0, oo), origin),
                    piece(rough::circle(0.0, 0.0, 5.0, io), origin),
                ],
                Bound::MeasuredCircle,
            )
        }
        // forkJoin.ts: a 70×10 bar (10×70 across) of `lineColor`.
        Shape::ForkBar => {
            let (w, h) = if st.horizontal {
                (10.0, 70.0)
            } else {
                (70.0, 10.0)
            };
            let fo = Options {
                fill: Some(st.line.clone()),
                stroke: st.line.clone(),
                ..o()
            };
            let d = rough::rectangle(-w / 2.0, -h / 2.0, w, h, fo);
            (vec![piece(d, origin)], Bound::Rect)
        }
        // choice.ts: the 28px diamond as a rough path; the node stays 28×28.
        Shape::Choice => {
            let path = path_from_points(&[
                Point::new(0.0, 14.0),
                Point::new(14.0, 0.0),
                Point::new(0.0, -14.0),
                Point::new(-14.0, 0.0),
            ]);
            (vec![piece(rough::path(&path, o()), origin)], Bound::Polygon)
        }
        // note.ts: a rectangle padded by `node.padding`, in the note colours.
        Shape::Note => {
            let (w, h) = (tw + 2.0 * p, th + 2.0 * p);
            let no = Options {
                fill: Some(st.note_fill.clone()),
                stroke: st.note_stroke.clone(),
                ..o()
            };
            let d = rough::rectangle(-w / 2.0, -h / 2.0, w, h, no);
            (vec![piece(d, origin)], Bound::Rect)
        }
        _ => return None,
    };
    // The measured pieces share one frame (one group, or two groups side by
    // side in doubleCircle's).
    let at = pieces.iter().find(|pc| pc.measured)?.at;
    let paths: Vec<Vec<PathCmd>> = pieces
        .iter()
        .filter(|pc| pc.measured)
        .flat_map(|pc| &pc.drawable.sets)
        .filter(|s| !s.ops.is_empty())
        .map(|s| set_cmds(&s.ops, Point::default()))
        .collect();
    let [x, y, w, h] = super::svgbbox::cubic_group_box(&paths)?;
    let bbox = Rect::new(x + at.x, y + at.y, w, h);
    // forkJoin.ts adds half the state padding (8) to the measured size;
    // choice.ts keeps 28×28.
    let size = match shape {
        Shape::ForkBar => Some((bbox.w + 4.0, bbox.h + 4.0)),
        Shape::Choice => Some((28.0, 28.0)),
        _ => None,
    };
    Some(HandNode {
        pieces,
        bbox,
        size,
        boundary,
    })
}
