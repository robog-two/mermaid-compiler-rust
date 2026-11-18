//! `swimlane-beta` diagrams (mermaid 12.3 `swimlane`).
//!
//! Upstream a swimlane diagram is a flowchart drawn by a different layout engine
//! (`rendering-util/layout-algorithms/swimlanes`): the flowchart parser and node
//! shapes are reused, every top-level subgraph is a lane, and nodes outside any
//! subgraph go to a synthesised untitled lane. This module is a port of that
//! engine as mermaid 12.3.0 ships it:
//!
//! 1. [`prepare`] parses the body with our flowchart parser and assigns each node
//!    to its lane; [`layout_data`] turns the result into mermaid's `LayoutData`
//!    (`getData`, `prepareLayoutForSwimlanes`, `createEdgeLabelNodes`).
//! 2. [`run_layout`] is `runSwimlaneLayoutCore`: the lane-aware Sugiyama phases and
//!    `writeBackToLayoutData` ([`sugiyama`]), `routeEdgesOrthogonal` ([`router`]),
//!    and `postProcessSwimlaneLayout`: the LR/RL/BT transforms ([`transform`]) and
//!    the clean-up passes ([`post`], [`materialized`], [`crossings`]).
//! 3. [`draw`] paints lanes (`clusters/swimlane.js`), nested subgraph boxes, nodes
//!    through the shared flowchart shape code, edges cut at their end shapes as
//!    `insertEdge` does for this layout, rounded, with line hops, and the labels
//!    where the layout anchored them.

#![allow(clippy::needless_range_loop)]

mod crossings;
mod geometry;
mod materialized;
pub mod model;
mod post;
mod router;
mod sugiyama;
mod transform;

use super::flowchart;
use super::graph::{self, ClusterId, Endpoint, GraphDiagram, Head, LineKind, Shape};
use super::markers::{self, End, MarkerColors};
use crate::config::{Look as ConfigLook, MermaidConfig};
use crate::error::Result;
use crate::geom::{self, Bounds, Point, Rect};
use crate::json::Json;
use crate::layout::Direction;
use crate::linejump;
use crate::scene::{DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::TextBlock;
use crate::theme::{ColorSlot, Palette};
use std::collections::{HashMap, HashSet};

/// Line-hop radius (`applySwimlaneLineJumps`: `jumpRadius: 6`).
const HOP_RADIUS: f64 = 6.0;
/// Corner radius of the `rounded` edge curve (`generateRoundedPath(points, 5)`).
const EDGE_CORNER_RADIUS: f64 = 5.0;
/// Edge labels wrap at this width (`max-width: 120px` on swimlane labels).
const LABEL_WRAP: f64 = 120.0;

// ---------------------------------------------------------------------------
// Preparation: flowchart IR → lanes
// ---------------------------------------------------------------------------

/// A lane: a top-level subgraph, or the untitled lane holding loose nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// The subgraph, `None` for the synthesised default lane.
    pub cluster: Option<ClusterId>,
    pub id: String,
    pub title: String,
    pub slot: ColorSlot,
}

/// A parsed swimlane diagram: the flowchart graph plus its lanes.
#[derive(Debug, Clone, PartialEq)]
pub struct Swimlanes {
    pub graph: GraphDiagram,
    /// Lanes in drawing order (the default lane, if any, first).
    pub lanes: Vec<Lane>,
    /// Lane index of every node.
    pub lane_of: Vec<usize>,
}

/// Parse a `swimlane-beta [dir]` body.
pub fn parse(body: &str, title: Option<String>) -> Result<Swimlanes> {
    let src = flowchart_source(body);
    let g = flowchart::parse(&src, title)?;
    Ok(prepare(g, &lanes_with_content(body)))
}

/// The layout a swimlane diagram is drawn with when it is not its own: mermaid
/// resolves `layout` per diagram (`resolveAppearance`), the latest layer's
/// `swimlane.layout` or `layout` over the schema's `swimlane` default. Any other
/// layout draws the diagram as the flowchart it is parsed as (the lanes are its
/// subgraphs), laid out by that algorithm.
pub fn other_layout(cfg: &MermaidConfig) -> Option<crate::config::LayoutAlgorithm> {
    let name = cfg.raw.iter().rev().find_map(|j| {
        j.get("swimlane")
            .and_then(|s| s.get("layout"))
            .and_then(Json::as_str)
            .or_else(|| j.get("layout").and_then(Json::as_str))
    })?;
    (name != "swimlane").then(|| crate::config::LayoutAlgorithm::from_name(name))
}

/// A `swimlane-beta [dir]` body parsed as the flowchart it is.
pub fn parse_flowchart(
    body: &str,
    title: Option<String>,
    opts: flowchart::ParseOptions,
) -> Result<GraphDiagram> {
    flowchart::parse_with(&flowchart_source(body), title, opts)
}

/// The body with its `swimlane-beta` keyword replaced by `flowchart`.
fn flowchart_source(body: &str) -> String {
    match body.find("swimlane-beta") {
        Some(i) => format!(
            "{}flowchart{}",
            &body[..i],
            &body[i + "swimlane-beta".len()..]
        ),
        None => body.to_string(),
    }
}

/// Ids of the top-level subgraphs that list something between `subgraph` and `end`.
/// A lane that lists members but ends up with none lost them all to an earlier lane
/// (`dropLanesEmptiedBySharedNodes`); one the author left empty stays.
fn lanes_with_content(body: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut depth = 0usize;
    let mut current: Option<String> = None;
    let mut auto = 0usize;
    for (_, stmt) in super::statements(body) {
        let (kw, rest) = match stmt.find(char::is_whitespace) {
            Some(i) => (&stmt[..i], stmt[i..].trim()),
            None => (stmt.as_str(), ""),
        };
        match kw {
            "subgraph" => {
                let id = if rest.starts_with('"') || rest.is_empty() {
                    auto += 1;
                    format!("subGraph{}", auto - 1)
                } else {
                    let end = rest.find(['[', ' ', '\t']).unwrap_or(rest.len());
                    rest[..end].trim().to_string()
                };
                if depth == 0 {
                    current = Some(id);
                } else if let Some(c) = &current {
                    out.insert(c.clone());
                }
                depth += 1;
            }
            "end" if rest.is_empty() && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    current = None;
                }
            }
            _ => {
                if let Some(c) = current.as_ref().filter(|_| depth > 0) {
                    out.insert(c.clone());
                }
            }
        }
    }
    out
}

/// Assign every node to its lane (`prepareLayoutForSwimlanes`).
pub fn prepare(g: GraphDiagram, with_content: &HashSet<String>) -> Swimlanes {
    let top = |mut c: ClusterId| {
        for _ in 0..=g.clusters.len() {
            match g.clusters[c.0].parent {
                Some(p) => c = p,
                None => break,
            }
        }
        c
    };
    let node_top: Vec<Option<ClusterId>> = g.nodes.iter().map(|n| n.cluster.map(top)).collect();
    let edge_targets: HashSet<usize> = g
        .edges
        .iter()
        .flat_map(|e| [e.from, e.to])
        .filter_map(|ep| match ep {
            Endpoint::Cluster(c) => Some(top(c).0),
            Endpoint::Node(_) => None,
        })
        .collect();
    let mut lanes = Vec::new();
    if node_top.iter().any(Option::is_none) {
        lanes.push(Lane {
            cluster: None,
            id: "__swimlane_default__".into(),
            title: String::new(),
            slot: ColorSlot(g.clusters.len()),
        });
    }
    for (ci, c) in g.clusters.iter().enumerate() {
        if c.parent.is_some() {
            continue;
        }
        let empty = !node_top.contains(&Some(ClusterId(ci)));
        if empty && with_content.contains(&c.id) && !edge_targets.contains(&ci) {
            continue; // emptied by nodes an earlier lane kept
        }
        lanes.push(Lane {
            cluster: Some(ClusterId(ci)),
            id: c.id.clone(),
            title: c.title.clone(),
            slot: c.color_slot.unwrap_or(ColorSlot(ci)),
        });
    }
    // `prepareLayoutForSwimlanes`: the synthesised lane takes the slot after the
    // highest one of the subgraphs that are still drawn.
    if lanes.first().is_some_and(|l| l.cluster.is_none()) {
        let kept = |ci: usize| {
            let t = top(ClusterId(ci));
            lanes.iter().any(|l| l.cluster == Some(t))
        };
        let next = g
            .clusters
            .iter()
            .enumerate()
            .filter(|(ci, _)| kept(*ci))
            .map(|(ci, c)| c.color_slot.unwrap_or(ColorSlot(ci)).0 + 1)
            .max()
            .unwrap_or(0);
        lanes[0].slot = ColorSlot(next);
    }
    let lane_of = node_top
        .iter()
        .map(|t| {
            lanes
                .iter()
                .position(|l| l.cluster == *t)
                .unwrap_or_default()
        })
        .collect();
    Swimlanes {
        graph: g,
        lanes,
        lane_of,
    }
}

// ---------------------------------------------------------------------------
// Layout: the diagram as mermaid's `LayoutData`, through `runSwimlaneLayoutCore`
// ---------------------------------------------------------------------------

/// mermaid's shape name for a node, as far as the layout tells shapes apart.
fn shape_name(s: Shape) -> &'static str {
    match s {
        Shape::Diamond => "diamond",
        Shape::Hexagon => "hexagon",
        Shape::NotchedPentagon => "notch-pent",
        _ => "squareRect",
    }
}

/// mermaid's `arrowType` name of an edge end.
fn arrow_type(h: Head) -> Option<String> {
    match h {
        Head::None => None,
        Head::Arrow => Some("arrow_point".into()),
        Head::Circle => Some("arrow_circle".into()),
        Head::Cross => Some("arrow_cross".into()),
        _ => Some("arrow_point".into()),
    }
}

/// The subgraphs in the order the flowchart grammar reduces them (a subgraph
/// closes after the ones nested in it): a post-order walk in declaration order.
fn close_order(g: &GraphDiagram) -> Vec<usize> {
    fn walk(g: &GraphDiagram, c: usize, out: &mut Vec<usize>) {
        for (k, o) in g.clusters.iter().enumerate() {
            if o.parent == Some(ClusterId(c)) {
                walk(g, k, out);
            }
        }
        out.push(c);
    }
    let mut out = Vec::new();
    for (c, cl) in g.clusters.iter().enumerate() {
        if cl.parent.is_none() {
            walk(g, c, &mut out);
        }
    }
    out
}

/// flowDb's edge ids: `L_<start>_<end>_<n>`, `n` 0 for the first edge between
/// the two and one more than the count of earlier ones after that.
fn edge_ids(g: &GraphDiagram) -> Vec<String> {
    let mut seen: Vec<(String, String)> = Vec::new();
    g.edges
        .iter()
        .map(|e| {
            let (s, t) = (endpoint_id(g, e.from), endpoint_id(g, e.to));
            let earlier = seen.iter().filter(|(a, b)| *a == s && *b == t).count();
            seen.push((s.clone(), t.clone()));
            let n = if earlier == 0 { 0 } else { earlier + 1 };
            format!("L_{s}_{t}_{n}")
        })
        .collect()
}

/// `getData` + `prepareLayoutForSwimlanes` + `createEdgeLabelNodes`, with the
/// measured node and label sizes.
fn layout_data(
    sl: &Swimlanes,
    sizes: &[(f64, f64)],
    labels: &[Option<(f64, f64)>],
) -> model::LayoutData {
    let g = &sl.graph;
    let mut nodes = Vec::new();
    let order = close_order(g);
    let lane_ids: HashSet<&str> = sl.lanes.iter().map(|l| l.id.as_str()).collect();
    for &ci in order.iter().rev() {
        let c = &g.clusters[ci];
        if c.parent.is_none() && !lane_ids.contains(c.id.as_str()) {
            continue; // a lane emptied by the nodes an earlier lane kept
        }
        let mut n = model::LNode::new(c.id.clone(), model::Origin::Cluster(ci));
        n.is_group = true;
        n.parent_id = c.parent.map(|p| g.clusters[p.0].id.clone());
        n.padding = Some(8.0);
        n.shape = if c.parent.is_none() {
            "swimlane".into()
        } else {
            "rect".into()
        };
        nodes.push(n);
    }
    let default_lane = sl.lanes.iter().find(|l| l.cluster.is_none());
    for (i, nd) in g.nodes.iter().enumerate() {
        let mut n = model::LNode::new(nd.id.clone(), model::Origin::Vertex(i));
        n.parent_id = match nd.cluster {
            Some(c) => Some(g.clusters[c.0].id.clone()),
            None => default_lane.map(|l| l.id.clone()),
        };
        n.shape = shape_name(nd.shape).into();
        n.width = Some(sizes[i].0);
        n.height = Some(sizes[i].1);
        nodes.push(n);
    }
    if let Some(l) = default_lane {
        let mut n = model::LNode::new(l.id.clone(), model::Origin::DefaultLane);
        n.is_group = true;
        n.padding = Some(20.0);
        n.shape = "swimlane".into();
        nodes.push(n);
    }
    let ids = edge_ids(g);
    let edges = g
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| model::LEdge {
            id: ids[ei].clone(),
            origin: Some(ei),
            start: endpoint_id(g, e.from),
            end: endpoint_id(g, e.to),
            points: None,
            label_node_id: None,
            is_layout_only: false,
            arrow_type_start: arrow_type(e.start),
            arrow_type_end: arrow_type(e.end),
        })
        .collect();
    let mut data = model::LayoutData {
        nodes,
        edges,
        direction: match g.direction {
            Direction::LeftRight => "LR",
            Direction::RightLeft => "RL",
            Direction::BottomTop => "BT",
            _ => "TB",
        }
        .into(),
    };
    sugiyama::create_edge_label_nodes(&mut data, &|ei| labels[ei].unwrap_or((-1.0, -1.0)));
    data
}

/// `runSwimlaneLayoutCore`: positions, routes and the clean-up passes.
pub fn run_layout(
    data: &mut model::LayoutData,
    node_gap: f64,
    layer_gap: f64,
    ignore_cross_lane: bool,
) {
    sugiyama::layout_core(data, node_gap, layer_gap, ignore_cross_lane);
    for e in data.edges.iter_mut() {
        e.points = None;
    }
    router::route_edges_orthogonal(data);
    post_process(data);
}

/// `postProcessSwimlaneLayout`.
fn post_process(data: &mut model::LayoutData) {
    let has_content = data.nodes.iter().any(|n| !n.is_group);
    let dir = data.direction.clone();
    if (dir == "LR" || dir == "RL") && has_content && !transform::apply_lr(data, dir == "RL") {
        return;
    }
    if dir == "BT" && has_content && !transform::apply_bt(data) {
        return;
    }
    for e in data.edges.iter_mut() {
        if e.is_layout_only {
            continue;
        }
        if let Some(p) = e.points.as_ref().filter(|p| p.len() >= 2) {
            e.points = Some(geometry::simplify(&geometry::orthogonalize(p)));
        }
    }
    post::simplify_detoured_edges(data);
    post::straighten_collinear_sibling_detours(data);
    post::port_swap_to_l_shape(data);
    post::anchor_labels_to_polyline(data);
    post::clip_edge_endpoints(data);
    post::collapse_short_terminal_stub(data);
    post::nudge_shared_interior_subpaths(data);
    materialized::separate_shared_rendered_terminal_lanes(data);
    materialized::collapse_redundant_rectangular_doglegs(data);
    materialized::lift_obstacle_hugging_same_side_rails(data);
    materialized::swap_destination_terminal_tails(data);
    let finalize = |data: &mut model::LayoutData| {
        crossings::resolve_rendered_orthogonal_crossings(data);
        materialized::separate_shared_rendered_terminal_lanes(data);
        materialized::reassign_crossing_external_rail_channels(data);
        crossings::shortcut_redundant_orthogonal_jogs(data);
        post::anchor_labels_to_polyline(data);
        post::prepare_edge_endpoints(data);
        materialized::lift_obstacle_hugging_same_side_rails(data);
        post::anchor_labels_to_polyline(data);
        post::prepare_edge_endpoints(data);
    };
    finalize(data);
    post::nudge_shared_interior_subpaths(data);
    finalize(data);
    for _ in 0..2 {
        materialized::lift_top_lane_title_bands_above_rails(data);
        materialized::shift_left_lane_title_bands_left_of_rails(data);
    }
}

/// `AXIS_TOLERANCE` of `insertEdge`'s swimlane clipping.
const AXIS_TOLERANCE: f64 = 0.5;

fn shares_column(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < AXIS_TOLERANCE
}
fn shares_row(a: Point, b: Point) -> bool {
    (a.y - b.y).abs() < AXIS_TOLERANCE
}
fn coincides(a: Point, b: Point) -> bool {
    shares_column(a, b) && shares_row(a, b)
}

/// `dockFinalPoint`: where the edge docks on its head shape, on the axis it arrived on.
fn dock_final_point(
    original_last: Point,
    last_inner: Point,
    before: Point,
    candidate: Point,
) -> Point {
    let was_orthogonal =
        shares_column(original_last, last_inner) || shares_row(original_last, last_inner);
    let keeps_axis = if shares_column(original_last, last_inner)
        && !shares_row(original_last, last_inner)
    {
        shares_column(candidate, last_inner)
    } else if shares_row(original_last, last_inner) && !shares_column(original_last, last_inner) {
        shares_row(candidate, last_inner)
    } else {
        shares_column(candidate, last_inner) || shares_row(candidate, last_inner)
    };
    if was_orthogonal && !keeps_axis {
        return original_last;
    }
    let (heading, from) = if coincides(original_last, last_inner) {
        (last_inner, before)
    } else {
        (original_last, last_inner)
    };
    if (heading.x - from.x) * (candidate.x - last_inner.x)
        + (heading.y - from.y) * (candidate.y - last_inner.y)
        < 0.0
    {
        return last_inner;
    }
    if shares_row(heading, from) && !shares_column(heading, from) {
        return Point::new(candidate.x, last_inner.y);
    }
    if shares_column(heading, from) && !shares_row(heading, from) {
        return Point::new(last_inner.x, candidate.y);
    }
    candidate
}

/// `insertEdge`'s swimlane clipping of an edge's points against its end shapes.
fn clip_to_shapes(
    points: &[Point],
    tail: &dyn Fn(Point) -> Point,
    head: &dyn Fn(Point) -> Point,
    end_pullback: f64,
) -> Vec<Point> {
    if points.len() < 2 {
        return points.to_vec();
    }
    if points.len() == 2 {
        let (first, last) = (points[0], points[1]);
        return if shares_column(first, last) || shares_row(first, last) {
            vec![tail(first), head(last)]
        } else {
            vec![tail(last), head(first)]
        };
    }
    let inner = &points[1..points.len() - 1];
    let first_inner = inner[0];
    let last_inner = inner[inner.len() - 1];
    let new_first = tail(first_inner);
    let original_last = points[points.len() - 1];
    let candidate = head(last_inner);
    let before = if inner.len() >= 2 {
        inner[inner.len() - 2]
    } else {
        points[0]
    };
    let new_last = dock_final_point(original_last, last_inner, before, candidate);
    // `finalDockPoint`.
    let turned = !coincides(original_last, last_inner);
    let length = (new_last.x - last_inner.x).hypot(new_last.y - last_inner.y);
    let final_point = if length >= AXIS_TOLERANCE.max(end_pullback) {
        Some(new_last)
    } else if turned && length >= AXIS_TOLERANCE {
        Some(original_last)
    } else {
        None
    };
    let first_dup = (new_first.x - first_inner.x).abs() < AXIS_TOLERANCE
        && (new_first.y - first_inner.y).abs() < AXIS_TOLERANCE;
    let mut out = Vec::with_capacity(points.len());
    if !first_dup {
        out.push(new_first);
    }
    out.extend_from_slice(inner);
    out.extend(final_point);
    out
}

/// `markerOffsets`: how far a line end is moved for its marker (only
/// `arrow_point` of the flowchart's arrow types is in the table).
fn marker_offset(h: Head) -> f64 {
    if h == Head::Arrow { 4.0 } else { 0.0 }
}

/// `applyMarkerOffsetsToPoints`: each end moved by its offset along its own
/// segment, however short that segment is.
fn apply_marker_offsets(points: &[Point], start: f64, end: f64) -> Vec<Point> {
    let mut out = points.to_vec();
    let n = points.len();
    if n >= 2 && start != 0.0 {
        let (a, b) = (points[0], points[1]);
        let angle = (b.y - a.y).atan2(b.x - a.x);
        out[0] = Point::new(a.x + start * angle.cos(), a.y + start * angle.sin());
    }
    if n >= 2 && end != 0.0 {
        let (a, b) = (points[n - 2], points[n - 1]);
        let angle = (b.y - a.y).atan2(b.x - a.x);
        out[n - 1] = Point::new(b.x - end * angle.cos(), b.y - end * angle.sin());
    }
    out
}

/// `intersect-rect`: where the line from a box's centre to `p` leaves the box.
fn intersect_rect(c: Point, w: f64, h: f64, p: Point) -> Point {
    let dx = p.x - c.x;
    let dy = p.y - c.y;
    let (mut w, mut h) = (w / 2.0, h / 2.0);
    if dy.abs() * w > dx.abs() * h {
        if dy < 0.0 {
            h = -h;
        }
        let sx = if dy == 0.0 { 0.0 } else { h * dx / dy };
        return Point::new(c.x + sx, c.y + h);
    }
    if dx < 0.0 {
        w = -w;
    }
    let sy = if dx == 0.0 { 0.0 } else { w * dy / dx };
    Point::new(c.x + w, c.y + sy)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Parse and draw.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let sl = parse(body, title)?;
    Ok(draw(&sl, cfg, pal))
}

/// An edge label as `labelRect` measures it: an HTML label wrapped at 120px.
fn edge_label_block(label: &str, font: f64) -> Option<TextBlock> {
    let flagged = crate::text::wrap_lines_flagged(
        &crate::text::label_lines(label),
        LABEL_WRAP,
        font,
        false,
        crate::text::BreakRule::Browser,
    );
    // A line that broke at a space keeps it (`white-space: break-spaces`),
    // which moves its glyphs left of the centre.
    let shifts: Vec<f64> = flagged
        .iter()
        .map(|(l, at_space)| {
            if *at_space {
                crate::text::break_space_shift(l, font, false)
            } else {
                0.0
            }
        })
        .collect();
    let lines: Vec<String> = flagged.into_iter().map(|(l, _)| l).collect();
    let natural = TextBlock::new(lines.clone(), font, false);
    if natural.is_empty() {
        return None;
    }
    let mut b = natural.html();
    if lines.len() > 1 {
        b.width = crate::text::html_wrap_width(&lines, LABEL_WRAP, font, false);
    }
    if shifts.iter().any(|&s| s != 0.0) {
        b.shifts = shifts;
    }
    Some(b)
}

/// The fill and stroke of a lane or subgraph box: its own style, else its palette slot.
fn box_paint(style: &graph::StyleOverrides, slot: Option<ColorSlot>, pal: &Palette) -> Paint {
    let slot = slot.and_then(|s| pal.color_slots.as_ref().map(|p| p.slot(s)));
    let fill = style
        .fill
        .clone()
        .or_else(|| slot.as_ref().and_then(|s| s.fill.clone()))
        .unwrap_or_else(|| pal.cluster_bkg.clone());
    let stroke = style
        .stroke
        .clone()
        .or_else(|| slot.as_ref().map(|s| s.border.clone()))
        .unwrap_or_else(|| pal.cluster_border.clone());
    let mut paint = Paint::both(&fill, &stroke, style.stroke_width.unwrap_or(1.0));
    paint.dash = style.dash.clone();
    paint
}

/// swimlane.js under the handDrawn look: the title strip and the body as
/// rough rectangles (`userNodeOverrides` with the cluster's colours; a
/// top-to-bottom title's hachure at `fillWeight: 3`, the body unfilled), drawn
/// body first. A palette slot's rules restroke each one's first path (its
/// hachure, in the slot's fill) and its second (the outline, in its border);
/// the lane's own `fill` / `stroke` styles go inline on the title's. Each
/// element with its drawn box.
fn hand_lane(
    title: Rect,
    body: Rect,
    style: &graph::StyleOverrides,
    slot: Option<crate::theme::SlotColors>,
    lr: bool,
    seed: f64,
    pal: &Palette,
) -> Vec<(Element, Rect)> {
    let base = crate::rough::Options {
        roughness: 0.7,
        fill_style: "hachure".into(),
        fill_weight: 1.5,
        hachure_gap: 1.5,
        stroke: pal.cluster_border.0.clone(),
        seed,
        stroke_width: style.stroke_width.unwrap_or(1.3),
        ..Default::default()
    };
    let title_o = crate::rough::Options {
        fill: Some(pal.cluster_bkg.0.clone()),
        fill_weight: if lr { 1.5 } else { 3.0 },
        ..base.clone()
    };
    let body_o = crate::rough::Options {
        fill: Some("none".into()),
        ..base
    };
    // rough.js draws the title first; the body goes in before it.
    let t = crate::rough::rectangle(title.x, title.y, title.w, title.h, title_o);
    let b = crate::rough::rectangle(body.x, body.y, body.w, body.h, body_o);
    [(b, "swimlane-body", false), (t, "swimlane-title", true)]
        .into_iter()
        .map(|(d, class, is_title)| {
            let bx = super::hand::drawable_bbox(&d).unwrap_or_default();
            let hn = super::hand::measured_node(vec![super::hand::Piece {
                drawable: d,
                at: Point::default(),
                measured: true,
            }]);
            let mut els = graph::hand_elements(&hn, Point::default(), style.dash.as_ref());
            for g in &mut els {
                if let Element::Group { children, .. } = g {
                    for (k, el) in children.iter_mut().enumerate() {
                        let Element::Shape { paint, .. } = el else {
                            continue;
                        };
                        if let Some(sl) = &slot {
                            if k == 1 {
                                paint.stroke = sl.border.clone();
                            } else if let Some(f) = &sl.fill {
                                paint.stroke = f.clone();
                            }
                        }
                        if is_title {
                            if k == 1 {
                                if let Some(c) = &style.stroke {
                                    paint.stroke = c.clone();
                                }
                                if let Some(w) = style.stroke_width {
                                    paint.stroke_width = w;
                                }
                            } else if let Some(f) = &style.fill {
                                paint.stroke = f.clone();
                            }
                        }
                    }
                }
            }
            (Element::group(class, els), bx)
        })
        .collect()
}

/// Draw a parsed swimlane diagram.
pub fn draw(sl: &Swimlanes, cfg: &MermaidConfig, pal: &Palette) -> Scene {
    let gcfg = &cfg.flowchart;
    // Labels go through the flowchart's `createText`, Font Awesome icons and all.
    let g = &*graph::with_icons(&sl.graph, gcfg);
    let font = pal.font_size;
    let horizontal = g.direction.is_horizontal();
    let sizes: Vec<(f64, f64)> = g
        .nodes
        .iter()
        .map(|n| graph::node_size(n, DiagramKind::Flowchart, gcfg, pal, horizontal))
        .collect();
    let label_blocks: Vec<Option<TextBlock>> = g
        .edges
        .iter()
        .map(|e| e.label.as_ref().and_then(|l| edge_label_block(l, font)))
        .collect();
    let label_sizes: Vec<Option<(f64, f64)>> = label_blocks
        .iter()
        .map(|b| b.as_ref().map(|b| (b.width, b.height)))
        .collect();
    // The layout reads each node's size from `getBBox()`: single-precision floats.
    let bbox = |(w, h): (f64, f64)| (geom::chromium_f32(w), geom::chromium_f32(h));
    let measured: Vec<(f64, f64)> = sizes.iter().map(|&s| bbox(s)).collect();
    let label_measured: Vec<Option<(f64, f64)>> = label_sizes.iter().map(|s| s.map(bbox)).collect();
    let mut data = layout_data(sl, &measured, &label_measured);
    run_layout(
        &mut data,
        gcfg.node_spacing,
        gcfg.rank_spacing,
        cfg.section_bool("swimlane", "ignoreCrossLaneEdges", true),
    );
    let at = |id: &str| data.node(id);
    let center = |n: &model::LNode| Point::new(n.x.unwrap_or(0.0), n.y.unwrap_or(0.0));
    let lr_lanes = data.direction == "LR";
    let hand_seed = (gcfg.look == ConfigLook::HandDrawn).then_some(gcfg.hand_drawn_seed);

    let mut bounds = Bounds::default();
    // Lanes and the subgraphs nested in them, in the order `getData` lists them.
    // A box is widened to its title (`clusters.js` / `swimlane.js`), which the
    // edges to it are cut against.
    let mut cluster_boxes: HashMap<usize, (Point, f64, f64)> = HashMap::new();
    let mut lane_els = Vec::new();
    for n in &data.nodes {
        let (title, style, slot, cid) = match n.origin {
            model::Origin::Cluster(ci) => {
                let c = &g.clusters[ci];
                (
                    c.title.clone(),
                    c.style.clone(),
                    Some(c.color_slot.unwrap_or(ColorSlot(ci))),
                    Some(ci),
                )
            }
            model::Origin::DefaultLane => {
                let slot = sl
                    .lanes
                    .iter()
                    .find(|l| l.cluster.is_none())
                    .map(|l| l.slot);
                (String::new(), Default::default(), slot, None)
            }
            _ => continue,
        };
        let t = TextBlock::label(&title, font).html();
        let (bw, bh) = if t.is_empty() {
            (0.0, 0.0)
        } else {
            (t.width, t.height)
        };
        let pad = n.padding.unwrap_or(0.0);
        let nw = n.width.unwrap_or(0.0);
        let width = if nw <= bw + pad { bw + pad } else { nw };
        let height = n.height.unwrap_or(0.0);
        let c = center(n);
        if let Some(ci) = cid {
            cluster_boxes.insert(ci, (c, width, height));
        }
        let paint = box_paint(&style, slot, pal);
        let color = style
            .color
            .clone()
            .unwrap_or_else(|| pal.title_color.clone());
        let lane_top = c.y - height / 2.0;
        let lane_left = c.x - width / 2.0;
        if n.shape == "swimlane" {
            let mut children = Vec::new();
            let (title_rect, body_rect, label_at, rotate);
            if lr_lanes {
                let tw = bh + 8.0;
                title_rect = Rect::new(lane_left, lane_top, tw, height);
                body_rect = Rect::new(lane_left + tw, lane_top, (width - tw).max(0.0), height);
                label_at = Point::new(lane_left + tw / 2.0, c.y);
                rotate = true;
            } else {
                let content_top = n.swimlane_content_top.unwrap_or(lane_top + height / 3.0);
                let th = bh.min((content_top - lane_top).max(0.0));
                title_rect = Rect::new(lane_left, lane_top, width, th);
                body_rect = Rect::new(
                    lane_left,
                    lane_top + th,
                    width,
                    (c.y + height / 2.0 - lane_top - th).max(0.0),
                );
                label_at = Point::new(c.x, lane_top + th / 2.0);
                rotate = false;
            }
            bounds.add_rect(title_rect);
            bounds.add_rect(body_rect);
            if let Some(seed) = hand_seed {
                let lane = hand_lane(
                    title_rect,
                    body_rect,
                    &style,
                    slot.and_then(|s| pal.color_slots.as_ref().map(|p| p.slot(s))),
                    lr_lanes,
                    seed,
                    pal,
                );
                for (el, b) in lane {
                    bounds.add_rect(b);
                    children.push(el);
                }
            } else {
                children.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: body_rect,
                        rx: 0.0,
                    },
                    paint.clone(),
                    "swimlane-body",
                ));
                children.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: title_rect,
                        rx: 0.0,
                    },
                    paint,
                    "swimlane-title",
                ));
            }
            if !t.is_empty() {
                // The label's box counts towards the diagram's bounds even where
                // it overflows a title strip squeezed below its height.
                bounds.add_rect(if rotate {
                    Rect::centered(label_at, bh, bw)
                } else {
                    Rect::centered(label_at, bw, bh)
                });
                let text = Text::centered(label_at, &t.lines, font, t.line_height, &color);
                let label = Element::classed_text(text, "cluster-label swimlane-label");
                children.push(if rotate {
                    Element::Rotated {
                        deg: -90.0,
                        about: label_at,
                        children: vec![label],
                    }
                } else {
                    label
                });
            }
            lane_els.push(Element::Group {
                id: Some(n.id.clone()),
                class: Some("cluster swimlane".into()),
                children,
            });
        } else {
            let r = Rect::new(lane_left, lane_top, width, height);
            bounds.add_rect(r);
            let mut children = vec![Element::rect(r, 0.0, paint)];
            // `.cluster rect` casts the neo look's shadow; only the lanes'
            // (`.swimlane.cluster rect`) are reset to `filter: none`.
            if let Some(sh) = pal.drop_shadow.as_ref().filter(|_| gcfg.look == ConfigLook::Neo) {
                graph::cast_shadow(&mut children, sh);
            }
            if !t.is_empty() {
                children.push(Element::classed_text(
                    Text::centered(
                        Point::new(c.x, lane_top + t.height / 2.0),
                        &t.lines,
                        font,
                        t.line_height,
                        &color,
                    ),
                    "cluster-label",
                ));
            }
            lane_els.push(Element::Group {
                id: Some(n.id.clone()),
                class: Some("cluster".into()),
                children,
            });
        }
    }

    // Nodes (their outlines cut the edges).
    let mut node_els = Vec::new();
    let mut boundaries = Vec::with_capacity(g.nodes.len());
    for (i, nd) in g.nodes.iter().enumerate() {
        let c = at(&nd.id).map(center).unwrap_or_default();
        let (el, boundary) =
            graph::draw_node(nd, i, DiagramKind::Flowchart, gcfg, pal, horizontal, c);
        // A rough node's box is its drawing's, off its centre.
        match graph::node_drawn_box(nd, DiagramKind::Flowchart, gcfg, pal, horizontal) {
            Some(b) => bounds.add_rect(Rect::new(b.x + c.x, b.y + c.y, b.w, b.h)),
            None => bounds.add_rect(Rect::centered(c, sizes[i].0, sizes[i].1)),
        }
        boundaries.push(boundary);
        node_els.push(el);
    }
    let shape_cut = |ep: Endpoint, toward: Point| -> Point {
        match ep {
            Endpoint::Node(i) => boundaries[i.0].clip(toward),
            Endpoint::Cluster(c) => match cluster_boxes.get(&c.0) {
                Some(&(cc, w, h)) => intersect_rect(cc, w, h, toward),
                None => toward,
            },
        }
    };

    // Edges: the laid-out points cut at their end shapes (`insertEdge`).
    let mut routes: Vec<(usize, Vec<Point>)> = Vec::new();
    for e in &data.edges {
        let Some(ei) = e.origin else { continue };
        let Some(pts) = e.points.as_ref().filter(|p| p.len() >= 2) else {
            continue;
        };
        let ge = &g.edges[ei];
        let tail = |p: Point| shape_cut(ge.from, p);
        let head = |p: Point| shape_cut(ge.to, p);
        let cut = clip_to_shapes(pts, &tail, &head, markers::pullback_for(ge.end, false));
        if cut.len() >= 2 {
            routes.push((ei, cut));
        }
    }
    let hops = linejump::LineHops::from_json(cfg.section("swimlane", "lineHops"));
    let neo = gcfg.look == ConfigLook::Neo;
    let geoms: Vec<linejump::EdgeGeom> = routes
        .iter()
        .map(|(ei, pts)| linejump::EdgeGeom {
            points: pts.clone(),
            start_offset: marker_offset(g.edges[*ei].start),
            end_offset: marker_offset(g.edges[*ei].end),
            rounded: true,
            corner_radius: EDGE_CORNER_RADIUS,
        })
        .collect();
    let hopped = linejump::line_hops(&geoms, HOP_RADIUS, hops);

    let edge_width = pal.stroke_width.unwrap_or(1.0);
    let mut edge_els = Vec::new();
    for (k, (ei, pts)) in routes.iter().enumerate() {
        let e = &g.edges[*ei];
        // An invisible link keeps its (unstroked) path upstream, which the
        // diagram's `getBBox` takes in like any other.
        for p in pts {
            bounds.add_point(*p);
        }
        if e.line == LineKind::Invisible {
            continue;
        }
        let last = pts.len() - 1;
        let stroke = e
            .style
            .stroke
            .clone()
            .unwrap_or_else(|| pal.line_color.clone());
        let width = e
            .style
            .stroke_width
            .unwrap_or(if e.line == LineKind::Thick {
                3.5
            } else {
                edge_width
            });
        let colors = MarkerColors {
            line: &stroke,
            background: &pal.main_bkg,
        };
        // The marker sits on the line end pulled back by its offset.
        let drawn = apply_marker_offsets(pts, marker_offset(e.start), marker_offset(e.end));
        let mut heads = markers::elements(
            e.end,
            End::End,
            pts[last],
            pts[last].minus(pts[last - 1]).norm(),
            colors,
            width,
            neo,
        );
        let end_heads = heads.len();
        heads.extend(markers::elements(
            e.start,
            End::Start,
            pts[0],
            pts[0].minus(pts[1]).norm(),
            colors,
            width,
            neo,
        ));
        let plain = geom::rounded_path(&drawn, EDGE_CORNER_RADIUS);
        let mut cmds = hopped[k].clone().unwrap_or_else(|| plain.clone());
        if let Some(seed) = hand_seed {
            // `insertEdge` hands the rounded path to rough.js (roughness 0.3)
            // and moves the markers onto the drawn ends.
            let (rough, moved) = graph::rough_edge(
                &plain,
                &drawn,
                true,
                Point::default(),
                seed,
                std::mem::take(&mut heads),
                end_heads,
            );
            if let Some(b) = geom::path_bbox(&rough) {
                bounds.add_rect(b);
            }
            cmds = rough;
            heads = moved;
        }
        let mut paint = Paint::stroke(&stroke, width);
        // A neo edge hides its first and last `markerOffsets2` px under its
        // markers by an inline pattern measured along its path (`insertEdge`).
        let inline = neo
            .then(|| {
                let hidden = (
                    markers::hidden_end(e.start, neo),
                    markers::hidden_end(e.end, neo),
                );
                let patterned = matches!(e.line, LineKind::Dotted | LineKind::Dashed);
                let d = graph::neo_dash(geom::path_length(&plain), hidden, patterned)?;
                if hopped[k].is_none() {
                    Some(d)
                } else {
                    graph::hopped_dash(&d, geom::path_length(&cmds)).or(Some(d))
                }
            })
            .flatten();
        paint.dash = e.style.dash.clone().or(inline).or_else(|| match e.line {
            _ if neo => None,
            LineKind::Dotted => Some(vec![2.0]),
            LineKind::Dashed => Some(vec![3.0]),
            _ => None,
        });
        let from_id = endpoint_id(g, e.from);
        let to_id = endpoint_id(g, e.to);
        let mut children = vec![Element::classed_shape(
            Geometry::Path(cmds),
            paint,
            format!("flowchart-link LS-{from_id} LE-{to_id}"),
        )];
        children.extend(heads);
        edge_els.push(Element::Group {
            id: Some(format!("L-{from_id}-{to_id}-{ei}")),
            class: Some("edgePath".into()),
            children,
        });
    }
    // Labels sit where the layout anchored their label nodes.
    let mut label_els = Vec::new();
    for e in &data.edges {
        let (Some(ei), Some(lid)) = (e.origin, e.label_node_id.as_ref()) else {
            continue;
        };
        let (Some(block), Some(ln)) = (&label_blocks[ei], at(lid)) else {
            continue;
        };
        let c = center(ln);
        let r = Rect::centered(c, block.width, block.height);
        bounds.add_rect(r);
        let ge = &g.edges[ei];
        let color = ge
            .style
            .color
            .clone()
            .unwrap_or_else(|| pal.text_color.clone());
        // swimlane's edge label: a 0.1px `rect` at its centre, which
        // `.edgeLabel rect` fills at half opacity, then the label.
        let dot = Element::rect(
            Rect::new(c.x, c.y, 0.1, 0.1),
            0.0,
            Paint::fill(&pal.edge_label_background).with_opacity(0.5),
        );
        let text = Text::centered(c, &block.lines, font, block.line_height, &color);
        label_els.push(Element::group(
            "edgeLabel",
            vec![
                dot,
                Element::rect(r, 0.0, Paint::fill(&pal.edge_label_background)),
                if block.shifts.is_empty() {
                    Element::text(text)
                } else {
                    graph::shifted_lines(text, &block.shifts, "")
                },
            ],
        ));
    }

    let mut title_els = Vec::new();
    if let Some(t) = g.title.as_ref().filter(|t| !t.trim().is_empty()) {
        let content = bounds.rect();
        let tb = TextBlock::new(vec![t.clone()], font * 1.125, false);
        let center = Point::new(
            content.x + content.w / 2.0,
            content.y - gcfg.title_top_margin + tb.height / 2.0 - tb.height,
        );
        bounds.add_rect(Rect::centered(center, tb.width, tb.height));
        title_els.push(Element::classed_text(
            Text::centered(
                center,
                &tb.lines,
                tb.font_size,
                tb.line_height,
                &pal.title_color,
            ),
            "flowchartTitleText",
        ));
    }
    let mut elements = vec![
        Element::group("clusters", lane_els),
        Element::group("edgePaths", edge_els),
        Element::group("edgeLabels", label_els),
        Element::group("nodes", node_els),
    ];
    elements.extend(title_els);
    Scene {
        kind: DiagramKind::Swimlane,
        view_box: bounds.rect().pad(gcfg.diagram_padding),
        elements,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

fn endpoint_id(g: &GraphDiagram, e: Endpoint) -> String {
    match e {
        Endpoint::Node(i) => g.nodes[i.0].id.clone(),
        Endpoint::Cluster(c) => g.clusters[c.0].id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORT: &str = "swimlane-beta LR
  subgraph Customer
    request[Request service]
    receive[Receive update]
  end
  subgraph Support
    triage[Triage request]
    answer[Send answer]
  end
  subgraph Engineering
    investigate[Investigate issue]
    fix[Prepare fix]
  end
  request --> triage
  triage -->|Known issue| answer
  answer --> receive";

    fn lane_ids(sl: &Swimlanes) -> Vec<&str> {
        sl.lanes.iter().map(|l| l.id.as_str()).collect()
    }

    /// Path data of every shape in `src`'s scene.
    fn drawn_paths(src: &str) -> Vec<String> {
        fn walk(els: &[Element], out: &mut Vec<String>) {
            for e in els {
                match e {
                    Element::Group { children, .. }
                    | Element::Rotated { children, .. }
                    | Element::Translated { children, .. } => walk(children, out),
                    Element::Shape {
                        geom: Geometry::Path(cmds),
                        ..
                    } => out.push(crate::output::svg::path_data(cmds)),
                    _ => {}
                }
            }
        }
        let r = crate::render::render(src, &MermaidConfig::default()).unwrap();
        let mut out = Vec::new();
        walk(&r.scene.elements, &mut out);
        out
    }

    #[test]
    fn hand_drawn_lanes_and_nodes_match_mermaid() {
        // mermaid 12.3.0, `look: handDrawn`, seed 21: the nodes are laid out
        // by their rough drawings' boxes (the lanes come out 935 wide, not
        // 953 as at the nominal sizes); the first node's rough rectangle and
        // the last lane's body start here.
        let src = "---\nconfig:\n  look: handDrawn\n  handDrawnSeed: 21\n  layout: swimlane\n---\n\
                   swimlane-beta LR\n  subgraph Customer\n    request[Request service]\n    \
                   receive[Receive update]\n  end\n  subgraph Support\n    triage[Triage request]\n    \
                   answer[Send answer]\n  end\n  subgraph Engineering\n    \
                   investigate[Investigate issue]\n    fix[Prepare fix]\n  end\n  \
                   request --> triage\n  triage -->|Known issue| answer\n  \
                   triage -->|Needs code change| investigate\n  investigate --> fix --> answer\n  \
                   answer --> receive\n";
        let paths = drawn_paths(src);
        for start in ["M-54.131,-25.349C", "M-72.584,411.258C"] {
            assert!(paths.iter().any(|d| d.starts_with(start)), "{start}");
        }
    }

    #[test]
    fn top_level_subgraphs_are_lanes_in_order() {
        let sl = parse(SUPPORT, None).unwrap();
        assert_eq!(lane_ids(&sl), ["Customer", "Support", "Engineering"]);
        assert_eq!(sl.graph.direction, Direction::LeftRight);
    }

    #[test]
    fn loose_nodes_get_a_default_lane_first() {
        let sl = parse(
            "swimlane-beta\n  start --> a\n  subgraph ops [Operations]\n    a\n  end",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 2);
        assert!(sl.lanes[0].cluster.is_none());
        assert_eq!(sl.lanes[1].title, "Operations");
        assert_eq!(sl.lane_of, [0, 1]);
    }

    /// mermaid 12.3.0 draws only `lane1`: the node stays in the first lane that
    /// names it, and the lane it was named in again is left empty and dropped.
    #[test]
    fn shared_node_stays_in_first_lane_and_emptied_lane_is_dropped() {
        let sl = parse(
            "swimlane-beta LR\nsubgraph lane1[Performer A]\n  a[\"step one\"]\nend\nsubgraph phase1[Phase 1]\n  a\nend",
            None,
        )
        .unwrap();
        assert_eq!(lane_ids(&sl), ["lane1"]);
        assert_eq!(sl.lane_of, [0]);
    }

    /// A lane the author wrote empty is kept (mermaid 12.3.0 draws `l1` and `l2`).
    #[test]
    fn author_empty_lane_is_kept() {
        let sl = parse(
            "swimlane-beta LR\nsubgraph l1\n  a\nend\nsubgraph l2\nend",
            None,
        )
        .unwrap();
        assert_eq!(lane_ids(&sl), ["l1", "l2"]);
    }

    #[test]
    fn nested_subgraph_belongs_to_its_lane() {
        let sl = parse(
            "swimlane-beta LR\nsubgraph lane1[Performer A]\n  subgraph ph1[Phase 1]\n    a[\"step one\"]\n  end\nend",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 1);
        assert_eq!(sl.lane_of, [0]);
    }

    /// The default lane takes the colour slot after the subgraphs that keep a
    /// lane (mermaid 12.3.0: `data-color-id` `color-1`, then `color-2`).
    #[test]
    fn default_lane_takes_the_slot_after_the_kept_subgraphs() {
        let sl = parse(
            "swimlane-beta\nsubgraph A\na\nend\nsubgraph B\na\nend\na-->b",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes.len(), 2);
        assert_eq!(sl.lanes[0].cluster, None);
        assert_eq!(sl.lanes[0].slot, ColorSlot(1));
        let sl = parse(
            "swimlane-beta\nsubgraph A\na\nend\nsubgraph B\nc\nend\na-->b",
            None,
        )
        .unwrap();
        assert_eq!(sl.lanes[0].slot, ColorSlot(2));
    }

    #[test]
    fn bad_input_is_a_parse_error() {
        assert!(parse("swimlane-beta XY\n a --> b", None).is_err());
        assert!(parse("swimlane-beta\nsubgraph A\n a", None).is_err());
    }

    fn config_from(yaml: &str) -> MermaidConfig {
        MermaidConfig::default()
            .with_directive(&crate::config::parse_yaml_subset(yaml))
            .unwrap()
    }

    /// mermaid's `resolveAppearance`: `swimlane.layout` or `layout`, the
    /// latest layer first, over the schema's `swimlane` default.
    #[test]
    fn a_layout_other_than_swimlane_draws_a_flowchart() {
        use crate::config::LayoutAlgorithm;
        assert_eq!(other_layout(&MermaidConfig::default()), None);
        assert_eq!(other_layout(&config_from("layout: swimlane")), None);
        assert_eq!(
            other_layout(&config_from("layout: dagre")),
            Some(LayoutAlgorithm::Dagre)
        );
        assert_eq!(
            other_layout(&config_from("layout: elk")),
            Some(LayoutAlgorithm::Elk)
        );
        assert_eq!(
            other_layout(&config_from("layout: elk\nswimlane:\n  layout: dagre")),
            Some(LayoutAlgorithm::Dagre)
        );
    }

    #[test]
    fn marker_offsets_move_each_end_along_its_own_segment() {
        // `applyMarkerOffsetsToPoints`: however short the segment, the end moves
        // the full offset along it (past the point before, here).
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(0.0, 10.0),
            Point::new(0.0, 12.5),
        ];
        let out = apply_marker_offsets(&pts, 0.0, marker_offset(Head::Arrow));
        assert!(out[2].dist(Point::new(0.0, 8.5)) < 1e-12, "{:?}", out[2]);
        assert_eq!(out[0], pts[0]);
        // Circles and crosses are not in `markerOffsets`.
        assert_eq!(marker_offset(Head::Circle), 0.0);
        assert_eq!(marker_offset(Head::Cross), 0.0);
    }
}
