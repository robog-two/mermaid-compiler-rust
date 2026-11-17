//! The layout data the swimlane passes work on, kept close to mermaid's
//! `LayoutData` so that each pass can be ported line by line: nodes and edges
//! keyed by string ids, positions that may be unset, and maps that iterate in
//! insertion order as JavaScript's `Map` does.

use crate::geom::Point;
use std::collections::HashMap;

/// `PRECISION.EPSILON` of the layout core and the router.
pub const EPS: f64 = 1e-6;

/// `left / right / top / bottom` bounds (`RectBounds`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rb {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

/// What a layout node stands for in the parsed diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A flowchart vertex (index into the graph's nodes).
    Vertex(usize),
    /// A subgraph (index into the graph's clusters).
    Cluster(usize),
    /// The synthesised lane for nodes outside every subgraph.
    DefaultLane,
    /// The label of an edge (index into the graph's edges).
    EdgeLabel(usize),
}

/// A node of the layout (`Node` / `ClusterNode`).
#[derive(Debug, Clone, PartialEq)]
pub struct LNode {
    pub id: String,
    pub origin: Origin,
    pub is_group: bool,
    pub parent_id: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub padding: Option<f64>,
    pub is_edge_label: bool,
    pub is_dummy: bool,
    /// The shape name mermaid gives the node (`question`, `hexagon`, `swimlane`, ...).
    pub shape: String,
    /// For an edge label: the edge's ends.
    pub edge_start: Option<String>,
    pub edge_end: Option<String>,
    pub layer: Option<usize>,
    pub order: Option<usize>,
    pub swimlane_content_top: Option<f64>,
    pub group_title_rect: Option<Rb>,
}

impl LNode {
    pub fn new(id: impl Into<String>, origin: Origin) -> Self {
        LNode {
            id: id.into(),
            origin,
            is_group: false,
            parent_id: None,
            x: None,
            y: None,
            width: None,
            height: None,
            padding: None,
            is_edge_label: false,
            is_dummy: false,
            shape: String::new(),
            edge_start: None,
            edge_end: None,
            layer: None,
            order: None,
            swimlane_content_top: None,
            group_title_rect: None,
        }
    }
}

/// An edge of the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct LEdge {
    pub id: String,
    /// Index into the graph's edges (`None` for layout-only edges).
    pub origin: Option<usize>,
    pub start: String,
    pub end: String,
    pub points: Option<Vec<Point>>,
    pub label_node_id: Option<String>,
    pub is_layout_only: bool,
    /// Marker names as mermaid calls them (`arrow_point`, `arrow_circle`, ...), or none.
    pub arrow_type_start: Option<String>,
    pub arrow_type_end: Option<String>,
}

/// The whole layout (`LayoutData`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutData {
    pub nodes: Vec<LNode>,
    pub edges: Vec<LEdge>,
    /// `TB`, `TD`, `BT`, `LR` or `RL`, as the diagram says it.
    pub direction: String,
}

impl LayoutData {
    pub fn node(&self, id: &str) -> Option<&LNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// A map from string keys that iterates in insertion order (JavaScript's `Map`).
#[derive(Debug, Clone)]
pub struct OMap<V> {
    keys: Vec<String>,
    vals: Vec<V>,
    index: HashMap<String, usize>,
}

impl<V> Default for OMap<V> {
    fn default() -> Self {
        OMap {
            keys: Vec::new(),
            vals: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<V> OMap<V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, k: &str) -> Option<&V> {
        self.index.get(k).map(|&i| &self.vals[i])
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        match self.index.get(k) {
            Some(&i) => Some(&mut self.vals[i]),
            None => None,
        }
    }
    pub fn has(&self, k: &str) -> bool {
        self.index.contains_key(k)
    }
    /// `map.set(k, v)`: replaces in place, or appends.
    pub fn set(&mut self, k: impl Into<String>, v: V) {
        let k = k.into();
        match self.index.get(&k) {
            Some(&i) => self.vals[i] = v,
            None => {
                self.index.insert(k.clone(), self.keys.len());
                self.keys.push(k);
                self.vals.push(v);
            }
        }
    }
    pub fn entry_or(&mut self, k: &str, f: impl FnOnce() -> V) -> &mut V {
        if !self.index.contains_key(k) {
            self.set(k.to_string(), f());
        }
        let i = self.index[k];
        &mut self.vals[i]
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.keys.iter()
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.vals.iter()
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.vals.iter_mut()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys.iter().zip(self.vals.iter())
    }
}

impl<V: Copy + Default + std::ops::Add<Output = V>> OMap<V> {
    /// `map.set(k, (map.get(k) ?? 0) + v)`.
    pub fn add(&mut self, k: &str, v: V) {
        let cur = self.get(k).copied().unwrap_or_default();
        self.set(k.to_string(), cur + v);
    }
}

/// JavaScript's `Math.round`: halves go up.
pub fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// JavaScript's `Number.prototype.toFixed` for moderate magnitudes: the nearest
/// decimal with `digits` places, a tie taking the larger magnitude.
pub fn to_fixed(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    let scale = 10f64.powi(digits as i32);
    let t = v.abs() * scale;
    // A tie is exactly representable (a dyadic rational), and Rust rounds it to even.
    if t.fract() == 0.5 && t / scale == v.abs() {
        let up = (t.floor() + 1.0) / scale;
        let r = format!("{up:.digits$}");
        return if v < 0.0 { format!("-{r}") } else { r };
    }
    s
}

/// The primary weight of a character under ICU's root collation, as far as ids go:
/// spaces and punctuation, then digits, then letters (case-folded), then the rest.
fn collation_key(c: char) -> (u8, u32) {
    const PUNCT: &str = "\t\n\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    if let Some(i) = PUNCT.find(c) {
        return (0, i as u32);
    }
    if c.is_ascii_digit() {
        return (1, c as u32);
    }
    if c.is_alphabetic() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        return (2, lower as u32);
    }
    (3, c as u32)
}

/// `String.prototype.localeCompare` (ICU root collation) for the ids diagrams use:
/// primary differences first, then lowercase before uppercase.
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let ka: Vec<(u8, u32)> = a.chars().map(collation_key).collect();
    let kb: Vec<(u8, u32)> = b.chars().map(collation_key).collect();
    match ka.cmp(&kb) {
        Ordering::Equal => {}
        o => return o,
    }
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            let ua = ca.is_uppercase();
            let ub = cb.is_uppercase();
            if ua != ub {
                return if ua {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            return ca.cmp(&cb);
        }
    }
    a.len().cmp(&b.len())
}

/// `a - b` as a sort comparator result.
pub fn num_cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// `Math.sign`.
pub fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_takes_ties_up() {
        assert_eq!(to_fixed(0.25, 1), "0.3");
        assert_eq!(to_fixed(-0.25, 1), "-0.3");
        assert_eq!(to_fixed(1.24, 1), "1.2");
        assert_eq!(to_fixed(12.0, 0), "12");
        assert_eq!(to_fixed(2.5, 0), "3");
    }

    #[test]
    fn locale_order_folds_case_and_puts_punctuation_first() {
        use std::cmp::Ordering::*;
        assert_eq!(locale_cmp("a", "B"), Less);
        assert_eq!(locale_cmp("b", "B"), Less);
        assert_eq!(locale_cmp("a_b", "a1"), Less);
        assert_eq!(locale_cmp("n10", "n2"), Less);
        assert_eq!(locale_cmp("L_a_b_0", "L_a_b_0"), Equal);
    }

    #[test]
    fn js_round_takes_halves_up() {
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
    }
}
