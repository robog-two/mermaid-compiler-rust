//! `direction/lrTransform.ts`: the LR/RL rotation of the canonical top-to-bottom
//! layout (lane boxes re-tiled across the flow) and the BT/RL mirrors.

use super::model::{LayoutData, Rb, num_cmp};
use super::sugiyama::{LaneModel, flow_axis_stretch};
use std::collections::{HashMap, HashSet};

const LANE_TITLE_BAND: f64 = 36.0;

struct Bounds {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

fn bounds_for(children: &[(f64, f64, f64, f64)]) -> Option<Bounds> {
    let mut b = Bounds {
        min_x: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        min_y: f64::INFINITY,
        max_y: f64::NEG_INFINITY,
    };
    for &(x, y, w, h) in children {
        b.min_x = b.min_x.min(x - w / 2.0);
        b.max_x = b.max_x.max(x + w / 2.0);
        b.min_y = b.min_y.min(y - h / 2.0);
        b.max_y = b.max_y.max(y + h / 2.0);
    }
    (b.min_x != f64::INFINITY && b.min_y != f64::INFINITY).then_some(b)
}

/// The boxes of positioned children of `id` (`boundsForChildren` input).
fn children_of(data: &LayoutData, id: &str) -> Vec<(f64, f64, f64, f64)> {
    data.nodes
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(id))
        .filter_map(|n| Some((n.x?, n.y?, n.width.unwrap_or(0.0), n.height.unwrap_or(0.0))))
        .collect()
}

fn group_depth(data: &LayoutData, i: usize) -> usize {
    let by_id: HashMap<&str, usize> = data
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    seen.insert(&data.nodes[i].id);
    let mut depth = 0;
    let mut parent = data.nodes[i].parent_id.as_deref();
    while let Some(pid) = parent {
        if seen.contains(pid) {
            break;
        }
        let Some(&pi) = by_id.get(pid) else { break };
        if !data.nodes[pi].is_group {
            break;
        }
        seen.insert(pid);
        depth += 1;
        parent = data.nodes[pi].parent_id.as_deref();
    }
    depth
}

/// `recomputeNestedGroupBounds`.
fn recompute_nested_group_bounds(data: &mut LayoutData) {
    let mut groups: Vec<(usize, usize)> = data
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_group && n.parent_id.is_some())
        .map(|(i, _)| (i, group_depth(data, i)))
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));
    for (gi, _) in groups {
        let id = data.nodes[gi].id.clone();
        if let Some(b) = bounds_for(&children_of(data, &id)) {
            let g = &mut data.nodes[gi];
            let pad = g.padding.unwrap_or(20.0);
            g.x = Some((b.min_x + b.max_x) / 2.0);
            g.y = Some((b.min_y + b.max_y) / 2.0);
            g.width = Some((b.max_x - b.min_x).max(0.0) + pad);
            g.height = Some((b.max_y - b.min_y).max(0.0) + pad);
        }
    }
}

/// `mirrorAxis`.
pub fn mirror_axis(data: &mut LayoutData, along_x: bool) -> bool {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for n in data.nodes.iter().filter(|n| !n.is_group) {
        if let Some(v) = if along_x { n.x } else { n.y } {
            min = min.min(v);
            max = max.max(v);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return false;
    }
    let m = |v: f64| min + max - v;
    for n in data.nodes.iter_mut() {
        if along_x {
            n.x = n.x.map(m);
        } else {
            n.y = n.y.map(m);
        }
        if let Some(t) = n.group_title_rect {
            n.group_title_rect = Some(if along_x {
                Rb {
                    left: m(t.right),
                    right: m(t.left),
                    ..t
                }
            } else {
                Rb {
                    top: m(t.bottom),
                    bottom: m(t.top),
                    ..t
                }
            });
        }
    }
    for e in data.edges.iter_mut() {
        for q in e.points.iter_mut().flatten() {
            if along_x {
                q.x = m(q.x);
            } else {
                q.y = m(q.y);
            }
        }
    }
    true
}

/// `applyBtDirectionTransform`.
pub fn apply_bt(data: &mut LayoutData) -> bool {
    if !data.nodes.iter().any(|n| !n.is_group) {
        return true;
    }
    mirror_axis(data, false)
}

/// `applyLrDirectionTransform`.
pub fn apply_lr(data: &mut LayoutData, rl: bool) -> bool {
    let content: Vec<usize> = (0..data.nodes.len())
        .filter(|&i| !data.nodes[i].is_group)
        .collect();
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    for &i in &content {
        let n = &data.nodes[i];
        min_x = min_x.min(n.x.unwrap_or(0.0));
        min_y = min_y.min(n.y.unwrap_or(0.0));
    }
    if !min_x.is_finite() || !min_y.is_finite() {
        return false;
    }
    let band = LANE_TITLE_BAND;
    let scale = flow_axis_stretch(content.iter().map(|&i| &data.nodes[i]));
    for &i in &content {
        let n = &mut data.nodes[i];
        let (x0, y0) = (n.x.unwrap_or(0.0), n.y.unwrap_or(0.0));
        n.x = Some((y0 - min_y) * scale + band);
        n.y = Some(x0 - min_x);
    }
    for e in data.edges.iter_mut() {
        for q in e.points.iter_mut().flatten() {
            let (x0, y0) = (q.x, q.y);
            q.x = (y0 - min_y) * scale + band;
            q.y = x0 - min_x;
        }
    }
    recompute_nested_group_bounds(data);
    let lanes = LaneModel::new(&data.nodes);
    let lane_idx: Vec<usize> = (0..data.nodes.len())
        .filter(|&i| lanes.is_lane(&data.nodes[i].id))
        .collect();
    if lane_idx.is_empty() {
        if rl {
            mirror_axis(data, true);
        }
        return true;
    }
    let mut children_by_lane: HashMap<String, Vec<(f64, f64, f64, f64)>> = HashMap::new();
    for n in &data.nodes {
        if lanes.is_lane(&n.id) {
            continue;
        }
        let Some(lane) = lanes.lane_id_of(&n.id) else {
            continue;
        };
        if let (Some(x), Some(y)) = (n.x, n.y) {
            children_by_lane.entry(lane).or_default().push((
                x,
                y,
                n.width.unwrap_or(0.0),
                n.height.unwrap_or(0.0),
            ));
        }
    }
    let mut max_pad: f64 = 0.0;
    for &li in &lane_idx {
        let pad = data.nodes[li].padding.unwrap_or(0.0);
        if pad > max_pad {
            max_pad = pad;
        }
    }
    // (lane, content top, content bottom, centre y)
    let mut lb: Vec<(usize, f64, f64, f64)> = Vec::new();
    let mut gmin = f64::INFINITY;
    let mut gmax = f64::NEG_INFINITY;
    for &li in &lane_idx {
        let ch = children_by_lane
            .get(&data.nodes[li].id)
            .cloned()
            .unwrap_or_default();
        let Some(b) = bounds_for(&ch) else { continue };
        gmin = gmin.min(b.min_x);
        gmax = gmax.max(b.max_x);
        lb.push((li, b.min_y, b.max_y, (b.min_y + b.max_y) / 2.0));
    }
    if gmin == f64::INFINITY || gmax == f64::NEG_INFINITY {
        return true;
    }
    let full = (gmax - gmin).max(0.0);
    let hmargin = max_pad.max(10.0);
    let body_w = full + 2.0 * hmargin;
    let lane_w = band + body_w;
    let body_center = (gmin + gmax) / 2.0;
    let body_left = body_center - body_w / 2.0;
    let lane_left = body_left - band;
    let center_x = lane_left + lane_w / 2.0;
    let vmargin = max_pad.max(band);
    lb.sort_by(|a, b| num_cmp(a.3, b.3));
    for i in 0..lb.len() {
        let (li, top, bottom, _) = lb[i];
        let lane_top = if i == 0 {
            top - vmargin
        } else {
            (lb[i - 1].2 + top) / 2.0
        };
        let lane_bottom = if i + 1 == lb.len() {
            bottom + vmargin
        } else {
            (bottom + lb[i + 1].1) / 2.0
        };
        let h = (lane_bottom - lane_top).max(0.0);
        let lane = &mut data.nodes[li];
        lane.x = Some(center_x);
        lane.y = Some((lane_top + lane_bottom) / 2.0);
        lane.width = Some(lane_w);
        lane.height = Some(h);
        lane.swimlane_content_top = Some(top);
        lane.group_title_rect = Some(Rb {
            left: lane_left,
            right: lane_left + band,
            top: lane_top,
            bottom: lane_bottom,
        });
    }
    if rl {
        mirror_axis(data, true);
    }
    true
}
