//! Edge end markers of the graph-family renderers, transcribed from mermaid's
//! `rendering-util/rendering-elements/markers.js` and the line pull-backs in
//! `utils/lineWithOffset.ts` (`markerOffsets`).
//!
//! We draw markers as plain geometry in user space instead of SVG `<marker>`s:
//! a point `(x, y)` of a marker definition lands at
//! `base + dir·(x − refX)·s + perp·(y − refY)·s` (end markers; start markers
//! are mirrored), where `base` is the line end after the pull-back and `s` the
//! marker scale (`markerWidth / viewBox width`, or the stroke width for
//! `markerUnits="strokeWidth"` markers).

use super::graph::Head;
use crate::geom::{PathCmd, Point};
use crate::scene::{Color, Element, Geometry, Paint};

/// Which end of the edge a marker sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

/// Fill of a marker part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// Line colour (`.marker { fill: lineColor }`).
    Line,
    /// Node background (`lollipop` → `mainBkg`).
    Background,
    None,
}

#[derive(Debug, Clone, Copy)]
enum Seg {
    M(f64, f64),
    L(f64, f64),
    Q(f64, f64, f64, f64),
}

#[derive(Debug, Clone, Copy)]
enum Part {
    /// Closed polygon.
    Poly(&'static [(f64, f64)]),
    /// Open path.
    Path(&'static [Seg]),
    Circle(f64, f64, f64),
}

#[derive(Debug, Clone, Copy)]
struct Def {
    ref_x: f64,
    ref_y: f64,
    parts: &'static [Part],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    /// `markerUnits="userSpaceOnUse"` with this `markerWidth / viewBox` ratio.
    Fixed(Ratio),
    /// Default `markerUnits="strokeWidth"`: scales with the edge stroke.
    StrokeWidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ratio {
    One,
    /// `viewBox 0 0 10 10`, `markerWidth 8`.
    Point,
    /// `viewBox 0 0 10 10`, `markerWidth 11`.
    Circle,
}

impl Ratio {
    fn value(self) -> f64 {
        match self {
            Ratio::One => 1.0,
            Ratio::Point => 0.8,
            Ratio::Circle => 1.1,
        }
    }
}

struct Marker {
    end: Def,
    /// Mirrored definition used at the start of an edge (`…Start` markers);
    /// `None` reuses `end` pointed into the start node.
    start: Option<Def>,
    scale: Scale,
    /// How far the line is shortened (`markerOffsets`).
    pullback: f64,
    fill: Fill,
    /// Stroke width inside the marker, in marker units.
    stroke: f64,
}

const DIAMOND: &[(f64, f64)] = &[(18.0, 7.0), (9.0, 13.0), (1.0, 7.0), (9.0, 1.0)];
const FOOT_END: &[Seg] = &[
    Seg::M(9.0, 18.0),
    Seg::Q(27.0, 0.0, 45.0, 18.0),
    Seg::Q(27.0, 36.0, 9.0, 18.0),
];
const FOOT_MANY_END: &[Seg] = &[
    Seg::M(21.0, 18.0),
    Seg::Q(39.0, 0.0, 57.0, 18.0),
    Seg::Q(39.0, 36.0, 21.0, 18.0),
];
const FOOT_START: &[Seg] = &[
    Seg::M(0.0, 18.0),
    Seg::Q(18.0, 0.0, 36.0, 18.0),
    Seg::Q(18.0, 36.0, 0.0, 18.0),
];

fn marker(head: Head, neo: bool) -> Option<Marker> {
    let def = |ref_x, ref_y, parts| Def {
        ref_x,
        ref_y,
        parts,
    };
    Some(match head {
        Head::None => return None,
        // point: viewBox 10, markerWidth 8, stroke 1, markerOffsets.arrow_point = 4.
        Head::Arrow => Marker {
            end: def(
                5.0,
                5.0,
                &[Part::Poly(&[(0.0, 0.0), (10.0, 5.0), (0.0, 10.0)])],
            ),
            start: Some(def(
                4.5,
                5.0,
                &[Part::Poly(&[(0.0, 5.0), (10.0, 10.0), (10.0, 0.0)])],
            )),
            scale: Scale::Fixed(Ratio::Point),
            pullback: 4.0,
            fill: Fill::Line,
            stroke: 1.0,
        },
        // circle: viewBox 10, markerWidth 11, refX 11 / -1.
        Head::Circle => Marker {
            end: def(11.0, 5.0, &[Part::Circle(5.0, 5.0, 5.0)]),
            start: Some(def(-1.0, 5.0, &[Part::Circle(5.0, 5.0, 5.0)])),
            scale: Scale::Fixed(Ratio::Circle),
            pullback: 0.0,
            fill: Fill::Line,
            stroke: 1.0,
        },
        // cross: viewBox 11, markerWidth 11, refX 12 / -1, refY 5.2, stroke 2.
        Head::Cross => {
            const X: &[Seg] = &[
                Seg::M(1.0, 1.0),
                Seg::L(10.0, 10.0),
                Seg::M(10.0, 1.0),
                Seg::L(1.0, 10.0),
            ];
            Marker {
                end: def(12.0, 5.2, &[Part::Path(X)]),
                start: Some(def(-1.0, 5.2, &[Part::Path(X)])),
                scale: Scale::Fixed(Ratio::One),
                pullback: 0.0,
                fill: Fill::Line,
                stroke: 2.0,
            }
        }
        // barb (state transitions): refX 19, refY 7, no pull-back.
        Head::Barb => Marker {
            end: def(
                19.0,
                7.0,
                &[Part::Poly(&[
                    (19.0, 7.0),
                    (9.0, 13.0),
                    (14.0, 7.0),
                    (9.0, 1.0),
                ])],
            ),
            start: None,
            scale: Scale::Fixed(Ratio::One),
            pullback: 0.0,
            fill: Fill::Line,
            stroke: 1.0,
        },
        // extension (hollow triangle): markerOffsets 17.25.
        Head::Triangle => Marker {
            end: def(
                1.0,
                7.0,
                &[Part::Poly(&[(1.0, 1.0), (1.0, 13.0), (18.0, 7.0)])],
            ),
            start: Some(def(
                18.0,
                7.0,
                &[Part::Poly(&[(1.0, 7.0), (18.0, 13.0), (18.0, 1.0)])],
            )),
            scale: Scale::Fixed(Ratio::One),
            pullback: 17.25,
            fill: Fill::None,
            stroke: 1.0,
        },
        // aggregation / composition: markerOffsets 17.25.
        Head::Diamond | Head::DiamondFilled => Marker {
            end: def(1.0, 7.0, &[Part::Poly(DIAMOND)]),
            start: Some(def(18.0, 7.0, &[Part::Poly(DIAMOND)])),
            scale: Scale::Fixed(Ratio::One),
            pullback: 17.25,
            fill: if head == Head::DiamondFilled {
                Fill::Line
            } else {
                Fill::None
            },
            stroke: 1.0,
        },
        // dependency: markerOffsets 6.
        Head::Dependency => Marker {
            end: def(
                13.0,
                7.0,
                &[Part::Poly(&[
                    (18.0, 7.0),
                    (9.0, 13.0),
                    (14.0, 7.0),
                    (9.0, 1.0),
                ])],
            ),
            start: Some(def(
                6.0,
                7.0,
                &[Part::Poly(&[
                    (5.0, 7.0),
                    (9.0, 13.0),
                    (1.0, 7.0),
                    (9.0, 1.0),
                ])],
            )),
            scale: Scale::Fixed(Ratio::One),
            pullback: 6.0,
            fill: Fill::Line,
            stroke: 1.0,
        },
        // lollipop: r 6 circle, markerOffsets 13.5.
        Head::Lollipop => Marker {
            end: def(1.0, 7.0, &[Part::Circle(7.0, 7.0, 6.0)]),
            start: Some(def(13.0, 7.0, &[Part::Circle(7.0, 7.0, 6.0)])),
            scale: Scale::Fixed(Ratio::One),
            pullback: 13.5,
            fill: Fill::Background,
            stroke: 1.0,
        },
        // ER cardinalities (markerUnits = strokeWidth; `.marker { fill: none !important }`).
        Head::ErOnlyOne => Marker {
            end: def(
                18.0,
                9.0,
                &[Part::Path(&[
                    Seg::M(3.0, 0.0),
                    Seg::L(3.0, 18.0),
                    Seg::M(9.0, 0.0),
                    Seg::L(9.0, 18.0),
                ])],
            ),
            start: Some(def(
                0.0,
                9.0,
                &[Part::Path(&[
                    Seg::M(9.0, 0.0),
                    Seg::L(9.0, 18.0),
                    Seg::M(15.0, 0.0),
                    Seg::L(15.0, 18.0),
                ])],
            )),
            scale: Scale::StrokeWidth,
            pullback: 0.0,
            fill: Fill::None,
            stroke: 1.0,
        },
        Head::ErZeroOrOne => Marker {
            end: def(
                30.0,
                9.0,
                &[
                    Part::Circle(9.0, 9.0, 6.0),
                    Part::Path(&[Seg::M(21.0, 0.0), Seg::L(21.0, 18.0)]),
                ],
            ),
            start: Some(def(
                0.0,
                9.0,
                &[
                    Part::Circle(21.0, 9.0, 6.0),
                    Part::Path(&[Seg::M(9.0, 0.0), Seg::L(9.0, 18.0)]),
                ],
            )),
            scale: Scale::StrokeWidth,
            pullback: 0.0,
            fill: Fill::None,
            stroke: 1.0,
        },
        Head::ErOneOrMore => Marker {
            end: def(
                27.0,
                18.0,
                &[
                    Part::Path(&[Seg::M(3.0, 9.0), Seg::L(3.0, 27.0)]),
                    Part::Path(FOOT_END),
                ],
            ),
            start: Some(def(
                18.0,
                18.0,
                &[
                    Part::Path(FOOT_START),
                    Part::Path(&[Seg::M(42.0, 9.0), Seg::L(42.0, 27.0)]),
                ],
            )),
            scale: Scale::StrokeWidth,
            pullback: 0.0,
            fill: Fill::None,
            stroke: 1.0,
        },
        Head::ErZeroOrMore => Marker {
            end: def(
                39.0,
                18.0,
                // zero_or_more_neo moves the circles: cx 11 (end), 45.5 (start).
                if neo {
                    &[Part::Circle(11.0, 18.0, 6.0), Part::Path(FOOT_MANY_END)]
                } else {
                    &[Part::Circle(9.0, 18.0, 6.0), Part::Path(FOOT_MANY_END)]
                },
            ),
            start: Some(def(
                18.0,
                18.0,
                if neo {
                    &[Part::Circle(45.5, 18.0, 6.0), Part::Path(FOOT_START)]
                } else {
                    &[Part::Circle(48.0, 18.0, 6.0), Part::Path(FOOT_START)]
                },
            )),
            scale: Scale::StrokeWidth,
            pullback: 0.0,
            fill: Fill::None,
            stroke: 1.0,
        },
        // requirement_arrow: open 20×20 chevron, tip on the line end.
        Head::OpenArrow => Marker {
            end: def(
                20.0,
                10.0,
                &[Part::Path(&[
                    Seg::M(0.0, 0.0),
                    Seg::L(20.0, 10.0),
                    Seg::L(0.0, 20.0),
                ])],
            ),
            start: None,
            scale: Scale::StrokeWidth,
            pullback: 0.0,
            fill: Fill::None,
            stroke: 1.0,
        },
        // requirement_contains: r 9 circle with a cross, starting at the node.
        Head::CirclePlus => {
            const PARTS: &[Part] = &[
                Part::Circle(10.0, 10.0, 9.0),
                Part::Path(&[
                    Seg::M(1.0, 10.0),
                    Seg::L(19.0, 10.0),
                    Seg::M(10.0, 1.0),
                    Seg::L(10.0, 19.0),
                ]),
            ];
            Marker {
                // Only a start marker exists upstream; mirror it for the end.
                end: def(20.0, 10.0, PARTS),
                start: Some(def(0.0, 10.0, PARTS)),
                scale: Scale::StrokeWidth,
                pullback: 0.0,
                fill: Fill::None,
                stroke: 1.0,
            }
        }
    })
}

/// Colours a marker may use.
#[derive(Debug, Clone, Copy)]
pub struct MarkerColors<'a> {
    pub line: &'a Color,
    pub background: &'a Color,
}

/// Distance the edge line is shortened at an end carrying `head`.
pub fn pullback(head: Head) -> f64 {
    marker(head, false).map_or(0.0, |m| m.pullback)
}

/// Marker geometry for `head` at the node boundary point `tip`, with `dir`
/// the unit direction pointing into the node. `width` is the edge stroke.
/// Under the neo look the ER and requirement markers are the `*_neo` variants:
/// `markerUnits="userSpaceOnUse"` (no stroke scaling) stroked with `strokeWidth`.
pub fn elements(
    head: Head,
    end: End,
    tip: Point,
    dir: Point,
    colors: MarkerColors<'_>,
    width: f64,
    neo: bool,
) -> Vec<Element> {
    let Some(m) = marker(head, neo) else {
        return Vec::new();
    };
    let (s, stroke) = match m.scale {
        Scale::Fixed(r) => (r.value(), m.stroke * r.value()),
        // requirement_arrow_neo draws its 20-unit chevron in a 25×20 viewBox: 0.8.
        Scale::StrokeWidth if neo && head == Head::OpenArrow => (0.8, width * 0.8),
        Scale::StrokeWidth if neo => (1.0, width),
        Scale::StrokeWidth => (width, m.stroke * width),
    };
    let (def, sign) = match (end, m.start) {
        (End::Start, Some(d)) => (d, -1.0),
        _ => (m.end, 1.0),
    };
    let perp = Point::new(-dir.y, dir.x);
    let base = tip.minus(dir.scale(m.pullback));
    let at = |x: f64, y: f64| {
        base.plus(dir.scale(sign * (x - def.ref_x) * s))
            .plus(perp.scale(sign * (y - def.ref_y) * s))
    };
    let fill = match m.fill {
        Fill::Line => colors.line.clone(),
        Fill::Background => colors.background.clone(),
        Fill::None => Color::none(),
    };
    let paint = Paint::both(&fill, colors.line, stroke);
    def.parts
        .iter()
        .map(|part| {
            let geom = match *part {
                Part::Poly(pts) => {
                    let pts: Vec<Point> = pts.iter().map(|&(x, y)| at(x, y)).collect();
                    Geometry::Path(crate::geom::polygon(&pts))
                }
                Part::Circle(cx, cy, r) => Geometry::Circle {
                    c: at(cx, cy),
                    r: r * s,
                },
                Part::Path(segs) => Geometry::Path(path(segs, &at)),
            };
            let paint = match part {
                Part::Path(_) => Paint::stroke(colors.line, stroke),
                _ => paint.clone(),
            };
            Element::classed_shape(geom, paint, "marker")
        })
        .collect()
}

fn path(segs: &[Seg], at: &dyn Fn(f64, f64) -> Point) -> Vec<PathCmd> {
    let mut cur = Point::default();
    segs.iter()
        .map(|seg| match *seg {
            Seg::M(x, y) => {
                cur = at(x, y);
                PathCmd::MoveTo(cur)
            }
            Seg::L(x, y) => {
                cur = at(x, y);
                PathCmd::LineTo(cur)
            }
            Seg::Q(qx, qy, x, y) => {
                // Quadratic → cubic: control points 2/3 of the way to Q.
                let (q, e) = (at(qx, qy), at(x, y));
                let c1 = cur.plus(q.minus(cur).scale(2.0 / 3.0));
                let c2 = e.plus(q.minus(e).scale(2.0 / 3.0));
                cur = e;
                PathCmd::CubicTo(c1, c2, e)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(els: &[Element]) -> (f64, f64, f64, f64) {
        let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut add = |p: Point| {
            b = (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y));
        };
        for e in els {
            if let Element::Shape { geom, .. } = e {
                match geom {
                    Geometry::Path(cmds) => {
                        for c in cmds {
                            match c {
                                PathCmd::MoveTo(p) | PathCmd::LineTo(p) => add(*p),
                                PathCmd::CubicTo(_, _, p) => add(*p),
                                PathCmd::Close => {}
                            }
                        }
                    }
                    Geometry::Circle { c, r } => {
                        add(Point::new(c.x - r, c.y - r));
                        add(Point::new(c.x + r, c.y + r));
                    }
                    Geometry::Rect { .. } => {}
                }
            }
        }
        b
    }

    fn draw(head: Head, end: End) -> Vec<Element> {
        draw_with(head, end, 1.0, false)
    }

    fn draw_with(head: Head, end: End, width: f64, neo: bool) -> Vec<Element> {
        let c = Color::new("#333");
        let colors = MarkerColors {
            line: &c,
            background: &c,
        };
        // Pointing right (+x) into a node whose boundary is at x = 100.
        elements(
            head,
            end,
            Point::new(100.0, 0.0),
            Point::new(1.0, 0.0),
            colors,
            width,
            neo,
        )
    }

    /// Classic ER/requirement markers scale with the stroke (`markerUnits` default);
    /// the `*_neo` ones are fixed in user space and stroked with `strokeWidth`.
    #[test]
    fn neo_markers_do_not_scale_with_the_stroke() {
        let classic = bounds(&draw_with(Head::ErOnlyOne, End::End, 2.0, false));
        assert_eq!(classic.3 - classic.1, 36.0);
        let neo = bounds(&draw_with(Head::ErOnlyOne, End::End, 2.0, true));
        assert_eq!(neo.3 - neo.1, 18.0);
        let widths: Vec<f64> = draw_with(Head::ErOnlyOne, End::End, 2.0, true)
            .iter()
            .filter_map(|e| match e {
                Element::Shape { paint, .. } => Some(paint.stroke_width),
                _ => None,
            })
            .collect();
        assert!(widths.iter().all(|&w| w == 2.0), "{widths:?}");
        // requirement_arrow_neo: 20-unit chevron in a 25×20 viewBox → 16 × 16.
        let (x0, y0, x1, y1) = bounds(&draw_with(Head::OpenArrow, End::End, 2.0, true));
        assert!((x1 - x0 - 16.0).abs() < 1e-9 && (y1 - y0 - 16.0).abs() < 1e-9);
    }

    #[test]
    fn point_arrow_is_8_by_8_with_tip_on_boundary() {
        let (x0, y0, x1, y1) = bounds(&draw(Head::Arrow, End::End));
        assert!((x1 - 100.0).abs() < 1e-9 && (x1 - x0 - 8.0).abs() < 1e-9);
        assert!((y1 - y0 - 8.0).abs() < 1e-9);
        assert_eq!(pullback(Head::Arrow), 4.0);
    }

    #[test]
    fn extension_triangle_is_17_long_and_12_wide() {
        let (x0, y0, x1, y1) = bounds(&draw(Head::Triangle, End::End));
        assert!((x1 - 99.75).abs() < 1e-9 && (x1 - x0 - 17.0).abs() < 1e-9);
        assert!((y1 - y0 - 12.0).abs() < 1e-9);
        // The start variant sits in the same place.
        let (sx0, _, sx1, _) = bounds(&draw(Head::Triangle, End::Start));
        assert!((sx0 - x0).abs() < 1e-9 && (sx1 - x1).abs() < 1e-9);
    }

    #[test]
    fn er_only_one_bars_sit_9_and_15_from_the_node() {
        let (x0, y0, x1, y1) = bounds(&draw(Head::ErOnlyOne, End::End));
        assert_eq!((x0, x1), (85.0, 91.0));
        assert_eq!(y1 - y0, 18.0);
        let (sx0, _, sx1, _) = bounds(&draw(Head::ErOnlyOne, End::Start));
        assert_eq!((sx0, sx1), (85.0, 91.0));
    }

    #[test]
    fn circle_head_radius() {
        let els = draw(Head::Circle, End::End);
        match &els[0] {
            Element::Shape {
                geom: Geometry::Circle { c, r },
                ..
            } => {
                assert!((r - 5.5).abs() < 1e-9);
                assert!((c.x - (100.0 - 6.6)).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
    }
}
