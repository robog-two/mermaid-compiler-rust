//! Line hops where two edges cross: a port of mermaid's `lineJump.ts`
//! (`applyLineJumpsToSvg`), which the ELK layout runs after painting
//! (`applyElkLineJumps`, `elk.lineHops`).
//!
//! Every pair of rendered edges is intersected segment by segment. At each proper
//! crossing one edge "gives way": where a horizontal segment crosses a vertical one
//! the horizontal edge hops, otherwise the later edge (in rendering order) does. The
//! giving-way edge's path is then rebuilt from its points with a semicircle (`arc`)
//! or a break (`gap`) of radius [`ELK_JUMP_RADIUS`] centred on each crossing,
//! keeping its rounded corners and its marker offsets. A hop is shrunk to the room
//! left between the bends at either end of its segment and between neighbouring
//! hops, and dropped when that leaves less than 60% of the radius. Crossings that
//! fall inside a rounded corner of either edge carry no hop.

use crate::geom::{self, PathCmd, Point};

/// `JUMP_RADIUS` of `elk/lineHops.ts`.
pub const ELK_JUMP_RADIUS: f64 = 6.0;
/// Spare length kept between a hop and a rounded corner (`CORNER_JUMP_CLEARANCE`).
const CORNER_JUMP_CLEARANCE: f64 = 2.0;
/// A hop narrower than this share of the radius is not drawn (`MIN_USEFUL_RADIUS_RATIO`).
const MIN_USEFUL_RADIUS_RATIO: f64 = 0.6;
const CORNER_EPSILON: f64 = 1e-5;
const ENDPOINT_EPSILON: f64 = 1e-6;

/// How a hop is drawn (`elk.lineHops`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineHops {
    /// `true` / `"arc"` (the default): a semicircle over the other edge.
    #[default]
    Arc,
    /// `"gap"`: the line is broken where it passes the other edge.
    Gap,
    /// `false`: crossings are drawn plainly.
    Off,
}

impl LineHops {
    /// mermaid's reading of `elk.lineHops`: `false` disables hops, `"gap"` selects
    /// gaps and every other value (including a missing key) means arcs.
    pub fn from_json(v: Option<&crate::json::Json>) -> LineHops {
        match v {
            Some(crate::json::Json::Bool(false)) => LineHops::Off,
            Some(crate::json::Json::String(s)) if s == "gap" => LineHops::Gap,
            _ => LineHops::Arc,
        }
    }
}

/// One rendered edge, as `applyLineJumpsToSvg` sees it.
#[derive(Debug, Clone)]
pub struct EdgeGeom {
    /// The edge's points as laid out (`data-points`): clipped to the node outlines,
    /// before any marker offset.
    pub points: Vec<Point>,
    /// `markerOffsets` of the start and end markers (0 for none).
    pub start_offset: f64,
    pub end_offset: f64,
    /// Drawn with the `rounded` curve (else straight segments).
    pub rounded: bool,
    /// Corner radius of the `rounded` curve.
    pub corner_radius: f64,
}

/// A crossing where `segment` of the hopping edge is cut at parameter `t`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Crossing {
    edge: usize,
    segment: usize,
    t: f64,
    point: Point,
}

/// Proper intersection of segments a1–a2 and b1–b2 (`segmentIntersection`):
/// the point and both parameters, or `None` when parallel or touching an end.
fn segment_intersection(a1: Point, a2: Point, b1: Point, b2: Point) -> Option<(Point, f64, f64)> {
    let (dxa, dya) = (a2.x - a1.x, a2.y - a1.y);
    let (dxb, dyb) = (b2.x - b1.x, b2.y - b1.y);
    let denom = dxa * dyb - dya * dxb;
    if denom == 0.0 {
        return None;
    }
    let (dx, dy) = (b1.x - a1.x, b1.y - a1.y);
    let ta = (dx * dyb - dy * dxb) / denom;
    let tb = (dx * dya - dy * dxa) / denom;
    let inside = |t: f64| t > ENDPOINT_EPSILON && t < 1.0 - ENDPOINT_EPSILON;
    if !inside(ta) || !inside(tb) {
        return None;
    }
    Some((Point::new(a1.x + ta * dxa, a1.y + ta * dya), ta, tb))
}

fn is_horizontal(a: Point, b: Point) -> bool {
    (b.x - a.x).abs() >= (b.y - a.y).abs()
}

/// `computeRoundedCorner`: the cut length of the rounded corner at `curr`.
fn corner_cut(prev: Point, curr: Point, next: Point, radius: f64) -> Option<f64> {
    let (d1, d2) = (curr.minus(prev), next.minus(curr));
    let (len1, len2) = (d1.len(), d2.len());
    if len1 < CORNER_EPSILON || len2 < CORNER_EPSILON {
        return None;
    }
    let (n1, n2) = (d1.scale(1.0 / len1), d2.scale(1.0 / len2));
    let angle = (n1.x * n2.x + n1.y * n2.y).clamp(-1.0, 1.0).acos();
    if angle < CORNER_EPSILON || (std::f64::consts::PI - angle).abs() < CORNER_EPSILON {
        return None;
    }
    Some(
        (radius / (angle / 2.0).sin())
            .min(len1 / 2.0)
            .min(len2 / 2.0),
    )
}

/// `crossingSitsInRoundedCorner`: whether parameter `t` of segment `seg` lies on
/// the part of the segment a rounded corner replaces.
fn in_rounded_corner(edge: &EdgeGeom, seg: usize, t: f64) -> bool {
    if !edge.rounded {
        return false;
    }
    let pts = &edge.points;
    let (a, b) = (pts[seg], pts[seg + 1]);
    let seg_len = a.dist(b);
    let d = t * seg_len;
    let entering = (seg > 0)
        .then(|| corner_cut(pts[seg - 1], a, b, edge.corner_radius))
        .flatten();
    if entering.is_some_and(|cut| d < cut) {
        return true;
    }
    let leaving = pts
        .get(seg + 2)
        .and_then(|&next| corner_cut(a, b, next, edge.corner_radius));
    leaving.is_some_and(|cut| seg_len - d < cut)
}

/// `findEdgeIntersections`: every crossing, attributed to the edge that hops.
fn find_crossings(edges: &[EdgeGeom]) -> Vec<Crossing> {
    let mut out = Vec::new();
    for (i, ea) in edges.iter().enumerate() {
        for (j, eb) in edges.iter().enumerate().skip(i + 1) {
            for (si, sa) in ea.points.windows(2).enumerate() {
                for (sj, sb) in eb.points.windows(2).enumerate() {
                    let Some((point, ta, tb)) = segment_intersection(sa[0], sa[1], sb[0], sb[1])
                    else {
                        continue;
                    };
                    if in_rounded_corner(ea, si, ta) || in_rounded_corner(eb, sj, tb) {
                        continue;
                    }
                    let (ah, bh) = (is_horizontal(sa[0], sa[1]), is_horizontal(sb[0], sb[1]));
                    // A horizontal segment hops a vertical one; any other pair hops on
                    // the later edge.
                    out.push(if ah != bh && ah {
                        Crossing {
                            edge: i,
                            segment: si,
                            t: ta,
                            point,
                        }
                    } else {
                        Crossing {
                            edge: j,
                            segment: sj,
                            t: tb,
                            point,
                        }
                    });
                }
            }
        }
    }
    out
}

/// `applyMarkerOffsets`: the first / last point moved inward by the marker offsets.
fn with_marker_offsets(edge: &EdgeGeom) -> Vec<Point> {
    let mut out = edge.points.clone();
    let n = out.len();
    if n < 2 {
        return out;
    }
    let pts = &edge.points;
    if edge.start_offset != 0.0 {
        let ang = (pts[1].y - pts[0].y).atan2(pts[1].x - pts[0].x);
        out[0] = Point::new(
            pts[0].x + edge.start_offset * ang.cos(),
            pts[0].y + edge.start_offset * ang.sin(),
        );
    }
    if edge.end_offset != 0.0 {
        let (a, b) = (pts[n - 2], pts[n - 1]);
        let ang = (b.y - a.y).atan2(b.x - a.x);
        out[n - 1] = Point::new(
            b.x - edge.end_offset * ang.cos(),
            b.y - edge.end_offset * ang.sin(),
        );
    }
    out
}

/// A hop on one segment: distance `d` from the segment start, radius `r`.
struct Jump {
    t: f64,
    point: Point,
    d: f64,
    r: f64,
}

/// `rewriteEdgePath`: the edge's path with a hop at each of its crossings.
fn rewrite(edge: &EdgeGeom, jumps: &[Crossing], radius: f64, style: LineHops) -> Vec<PathCmd> {
    let points = with_marker_offsets(edge);
    let n = points.len();
    if n < 2 {
        return Vec::new();
    }
    let mut by_seg: Vec<Vec<Jump>> = (0..n - 1).map(|_| Vec::new()).collect();
    for c in jumps {
        let Some(list) = by_seg.get_mut(c.segment) else {
            continue;
        };
        let seg_len = points[c.segment].dist(points[c.segment + 1]);
        list.push(Jump {
            t: c.t,
            point: c.point,
            d: c.t * seg_len,
            r: radius,
        });
    }
    let min_useful = radius * MIN_USEFUL_RADIUS_RATIO;
    let mut out = vec![PathCmd::MoveTo(points[0])];
    for (i, mut seg_jumps) in by_seg.into_iter().enumerate() {
        let (a, b) = (points[i], points[i + 1]);
        let seg_len = a.dist(b);
        let u = if seg_len == 0.0 {
            Point::default()
        } else {
            b.minus(a).scale(1.0 / seg_len)
        };
        // `getArcSweepFlag`: along the dominant axis, positive travel sweeps clockwise.
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let clockwise = if dx.abs() >= dy.abs() {
            dx >= 0.0
        } else {
            dy >= 0.0
        };
        let start_consumed = if edge.rounded && i > 0 {
            corner_cut(points[i - 1], a, b, edge.corner_radius).unwrap_or(0.0)
        } else {
            0.0
        };
        let upcoming = (edge.rounded && i + 1 < n - 1)
            .then(|| corner_cut(a, b, points[i + 2], edge.corner_radius))
            .flatten();
        let end_stop = seg_len - upcoming.unwrap_or(0.0);

        // Clamp each hop to the room between the bends, drop the ones left too small,
        // then keep neighbouring hops off each other.
        seg_jumps.sort_by(|p, q| p.t.total_cmp(&q.t));
        seg_jumps.retain_mut(|j| {
            let room = (j.d - start_consumed).min(end_stop - j.d) - CORNER_JUMP_CLEARANCE;
            j.r = j.r.min(room);
            j.r >= min_useful
        });
        for k in 1..seg_jumps.len() {
            let gap = seg_jumps[k].d - seg_jumps[k - 1].d;
            if seg_jumps[k - 1].r + seg_jumps[k].r > gap {
                let half = gap / 2.0;
                seg_jumps[k - 1].r = seg_jumps[k - 1].r.min(half);
                seg_jumps[k].r = seg_jumps[k].r.min(half);
            }
        }
        for j in seg_jumps.iter().filter(|j| j.r >= min_useful) {
            let pre = j.point.minus(u.scale(j.r));
            let post = j.point.plus(u.scale(j.r));
            out.push(PathCmd::LineTo(pre));
            match style {
                LineHops::Gap => out.push(PathCmd::MoveTo(post)),
                _ => {
                    let a0 = (-u.y).atan2(-u.x);
                    let sweep = if clockwise {
                        std::f64::consts::PI
                    } else {
                        -std::f64::consts::PI
                    };
                    out.extend(geom::arc_cubics(j.point, j.r, a0, a0 + sweep));
                }
            }
        }
        match upcoming {
            Some(cut) => {
                let corner = b;
                let next = points[i + 2];
                let start = corner.minus(u.scale(cut));
                let end = corner.plus(next.minus(corner).norm().scale(cut));
                out.push(PathCmd::LineTo(start));
                // Q corner end, as its exact cubic.
                out.push(PathCmd::CubicTo(
                    start.lerp(corner, 2.0 / 3.0),
                    end.lerp(corner, 2.0 / 3.0),
                    end,
                ));
            }
            None => out.push(PathCmd::LineTo(b)),
        }
    }
    out
}

/// The rewritten path of every edge that hops at least one crossing (`None` for the
/// others, whose path is left as drawn). `edges` is in rendering order, which decides
/// which of two non-orthogonal crossing segments gives way.
pub fn line_hops(edges: &[EdgeGeom], radius: f64, style: LineHops) -> Vec<Option<Vec<PathCmd>>> {
    let mut out = vec![None; edges.len()];
    if style == LineHops::Off {
        return out;
    }
    let crossings = find_crossings(edges);
    for (i, edge) in edges.iter().enumerate() {
        let mine: Vec<Crossing> = crossings.iter().filter(|c| c.edge == i).copied().collect();
        if !mine.is_empty() && edge.points.len() >= 2 {
            out[i] = Some(rewrite(edge, &mine, radius, style));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(points: &[(f64, f64)]) -> EdgeGeom {
        EdgeGeom {
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            start_offset: 0.0,
            end_offset: 0.0,
            rounded: true,
            corner_radius: 5.0,
        }
    }

    fn arcs(cmds: &[PathCmd]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, PathCmd::CubicTo(..)))
            .count()
    }

    #[test]
    fn horizontal_segment_hops_the_vertical_one() {
        // Whichever comes first, the horizontal edge gives way.
        let (v, h) = (
            edge(&[(50.0, 0.0), (50.0, 100.0)]),
            edge(&[(0.0, 50.0), (100.0, 50.0)]),
        );
        let swapped = line_hops(&[h.clone(), v.clone()], ELK_JUMP_RADIUS, LineHops::Arc);
        assert!(swapped[0].is_some() && swapped[1].is_none());
        let out = line_hops(&[v, h], ELK_JUMP_RADIUS, LineHops::Arc);
        assert!(out[0].is_none());
        let cmds = out[1].as_ref().unwrap();
        assert_eq!(cmds[0], PathCmd::MoveTo(Point::new(0.0, 50.0)));
        assert_eq!(cmds[1], PathCmd::LineTo(Point::new(44.0, 50.0)));
        // A semicircle (two quarter cubics) over the top, ending at 56,50.
        assert_eq!(arcs(cmds), 2);
        let PathCmd::CubicTo(_, _, top) = cmds[2] else {
            panic!()
        };
        assert!((top.x - 50.0).abs() < 1e-9 && (top.y - 44.0).abs() < 1e-9);
        let PathCmd::CubicTo(_, _, end) = cmds[3] else {
            panic!()
        };
        assert!(end.dist(Point::new(56.0, 50.0)) < 1e-9);
        assert_eq!(cmds[4], PathCmd::LineTo(Point::new(100.0, 50.0)));
    }

    #[test]
    fn leftward_hop_also_bulges_up_and_vertical_bulges_right() {
        let left = [
            edge(&[(50.0, 0.0), (50.0, 100.0)]),
            edge(&[(100.0, 50.0), (0.0, 50.0)]),
        ];
        let cmds = line_hops(&left, 6.0, LineHops::Arc)[1].clone().unwrap();
        let PathCmd::CubicTo(_, _, top) = cmds[2] else {
            panic!()
        };
        assert!(top.dist(Point::new(50.0, 44.0)) < 1e-9);
        // Two diagonal-free but same-orientation edges: the later one hops; here a
        // vertical over a vertical cannot cross, so use two slanted segments.
        let slanted = [
            edge(&[(0.0, 0.0), (40.0, 100.0)]),
            edge(&[(40.0, 0.0), (0.0, 100.0)]),
        ];
        let out = line_hops(&slanted, 6.0, LineHops::Arc);
        assert!(out[0].is_none() && out[1].is_some());
        let PathCmd::CubicTo(_, _, mid) = out[1].as_ref().unwrap()[2] else {
            panic!()
        };
        // Downward travel (dominant axis y): the arc swings to the right of centre.
        assert!(mid.x > 20.0);
    }

    #[test]
    fn gap_style_breaks_the_line() {
        let edges = [
            edge(&[(50.0, 0.0), (50.0, 100.0)]),
            edge(&[(0.0, 50.0), (100.0, 50.0)]),
        ];
        let cmds = line_hops(&edges, 6.0, LineHops::Gap)[1].clone().unwrap();
        assert_eq!(
            cmds,
            vec![
                PathCmd::MoveTo(Point::new(0.0, 50.0)),
                PathCmd::LineTo(Point::new(44.0, 50.0)),
                PathCmd::MoveTo(Point::new(56.0, 50.0)),
                PathCmd::LineTo(Point::new(100.0, 50.0)),
            ]
        );
    }

    #[test]
    fn off_parallel_and_touching_edges_do_not_hop() {
        let crossing = [
            edge(&[(50.0, 0.0), (50.0, 100.0)]),
            edge(&[(0.0, 50.0), (100.0, 50.0)]),
        ];
        assert!(
            line_hops(&crossing, 6.0, LineHops::Off)
                .iter()
                .all(Option::is_none)
        );
        let parallel = [
            edge(&[(0.0, 0.0), (100.0, 0.0)]),
            edge(&[(0.0, 10.0), (100.0, 10.0)]),
        ];
        assert!(
            line_hops(&parallel, 6.0, LineHops::Arc)
                .iter()
                .all(Option::is_none)
        );
        // Sharing an endpoint (two edges out of one port) is not a crossing.
        let fan = [
            edge(&[(0.0, 0.0), (100.0, 0.0)]),
            edge(&[(0.0, 0.0), (0.0, 100.0)]),
        ];
        assert!(
            line_hops(&fan, 6.0, LineHops::Arc)
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn hops_keep_clear_of_rounded_corners() {
        // Crossing 4 after a bend: inside the corner's cut, so no hop at all.
        let vertical = edge(&[(14.0, 0.0), (14.0, 100.0)]);
        let corner = edge(&[(10.0, 0.0), (10.0, 50.0), (100.0, 50.0)]);
        assert!(
            line_hops(&[vertical, corner.clone()], 6.0, LineHops::Arc)
                .iter()
                .all(Option::is_none)
        );
        // Crossing 8 after the bend: the hop is shrunk to the room left (8 - 5 - 2 = 1),
        // under 60% of the radius, so the path is rewritten without one.
        let vertical = edge(&[(18.0, 0.0), (18.0, 100.0)]);
        let out = line_hops(&[vertical, corner.clone()], 6.0, LineHops::Arc);
        let cmds = out[1].as_ref().unwrap();
        let plain = geom::rounded_path(&corner.points, 5.0);
        assert_eq!(cmds.len(), plain.len());
        assert_eq!(arcs(cmds), 1);
        // Crossing 15 after the bend: the radius is cut to 15 - 5/sin 45° - 2 ≈ 5.93.
        let vertical = edge(&[(25.0, 0.0), (25.0, 100.0)]);
        let out = line_hops(&[vertical, corner], 6.0, LineHops::Arc);
        let cmds = out[1].as_ref().unwrap();
        let r = 15.0 - 5.0 * 2f64.sqrt() - 2.0;
        assert_eq!(arcs(cmds), 3);
        assert!(matches!(cmds[3], PathCmd::LineTo(p) if p.dist(Point::new(25.0 - r, 50.0)) < 1e-9));
    }

    #[test]
    fn close_hops_share_the_gap_and_too_close_ones_vanish() {
        let h = edge(&[(0.0, 50.0), (100.0, 50.0)]);
        let v = |x: f64| edge(&[(x, 0.0), (x, 100.0)]);
        // 10 apart: both radii halved to 5.
        let out = line_hops(&[v(40.0), v(50.0), h.clone()], 6.0, LineHops::Gap);
        let cmds = out[2].as_ref().unwrap();
        assert_eq!(cmds[1], PathCmd::LineTo(Point::new(35.0, 50.0)));
        assert_eq!(cmds[2], PathCmd::MoveTo(Point::new(45.0, 50.0)));
        assert_eq!(cmds[3], PathCmd::LineTo(Point::new(45.0, 50.0)));
        assert_eq!(cmds[4], PathCmd::MoveTo(Point::new(55.0, 50.0)));
        // 6 apart: radius 3 < 3.6, neither drawn.
        let out = line_hops(&[v(47.0), v(53.0), h], 6.0, LineHops::Gap);
        assert_eq!(out[2].as_ref().unwrap().len(), 2);
    }

    #[test]
    fn marker_offsets_shorten_the_rewritten_path() {
        let mut h = edge(&[(0.0, 50.0), (100.0, 50.0)]);
        h.end_offset = 4.0;
        h.start_offset = 2.0;
        let out = line_hops(
            &[edge(&[(50.0, 0.0), (50.0, 100.0)]), h],
            6.0,
            LineHops::Gap,
        );
        let cmds = out[1].as_ref().unwrap();
        assert_eq!(cmds[0], PathCmd::MoveTo(Point::new(2.0, 50.0)));
        assert_eq!(
            *cmds.last().unwrap(),
            PathCmd::LineTo(Point::new(96.0, 50.0))
        );
    }

    #[test]
    fn config_values() {
        use crate::json::Json;
        assert_eq!(LineHops::from_json(None), LineHops::Arc);
        assert_eq!(LineHops::from_json(Some(&Json::Bool(true))), LineHops::Arc);
        assert_eq!(LineHops::from_json(Some(&Json::Bool(false))), LineHops::Off);
        assert_eq!(
            LineHops::from_json(Some(&Json::String("gap".into()))),
            LineHops::Gap
        );
        assert_eq!(
            LineHops::from_json(Some(&Json::String("arc".into()))),
            LineHops::Arc
        );
    }
}
