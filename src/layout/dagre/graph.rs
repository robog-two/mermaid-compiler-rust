//! A port of graphlib's `Graph` (as bundled by dagre-d3-es), keeping its
//! iteration orders: graphlib keeps nodes, edges, children and neighbours in
//! plain JS objects, whose keys enumerate integer-like keys first (ascending),
//! then every other key in insertion order. dagre's results depend on those
//! orders, so [`OMap`] reproduces them.

use std::collections::HashMap;

/// The key of the implicit root of a compound graph (graphlib's `GRAPH_NODE`).
const ROOT: &str = "\u{0}";

/// `v` as a JS array index (a canonical integer below 2^32 − 1), which JS objects
/// enumerate before their other keys.
fn array_index(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if b.len() > 1 && b[0] == b'0' {
        return None;
    }
    k.parse::<u64>()
        .ok()
        .filter(|&n| n < u32::MAX as u64)
        .map(|n| n as u32)
}

/// A string-keyed map that enumerates like a JS object.
#[derive(Debug, Clone)]
pub struct OMap<V> {
    map: HashMap<String, (u64, V)>,
    seq: u64,
}

impl<V> Default for OMap<V> {
    fn default() -> Self {
        OMap {
            map: HashMap::new(),
            seq: 0,
        }
    }
}

impl<V> OMap<V> {
    pub fn get(&self, k: &str) -> Option<&V> {
        self.map.get(k).map(|e| &e.1)
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        self.map.get_mut(k).map(|e| &mut e.1)
    }
    pub fn contains(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }
    /// Sets `k` (keeping its place if it is already there).
    pub fn insert(&mut self, k: &str, v: V) {
        if let Some(e) = self.map.get_mut(k) {
            e.1 = v;
        } else {
            self.seq += 1;
            self.map.insert(k.to_string(), (self.seq, v));
        }
    }
    pub fn remove(&mut self, k: &str) -> Option<V> {
        self.map.remove(k).map(|e| e.1)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// The keys in JS enumeration order.
    pub fn keys(&self) -> Vec<String> {
        let mut ks: Vec<(Option<u32>, u64, &String)> = self
            .map
            .iter()
            .map(|(k, (s, _))| (array_index(k), *s, k))
            .collect();
        ks.sort_by(|a, b| match (a.0, b.0) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.1.cmp(&b.1),
        });
        ks.into_iter().map(|(_, _, k)| k.clone()).collect()
    }
    /// The values in JS enumeration order.
    pub fn values(&self) -> Vec<&V> {
        self.keys()
            .iter()
            .map(|k| &self.map[k.as_str()].1)
            .collect()
    }
}

/// An edge's identity (graphlib's `edgeObj`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EdgeObj {
    pub v: String,
    pub w: String,
    pub name: Option<String>,
}

impl EdgeObj {
    pub fn new(v: &str, w: &str, name: Option<&str>) -> Self {
        EdgeObj {
            v: v.to_string(),
            w: w.to_string(),
            name: name.map(str::to_string),
        }
    }
}

/// graphlib's `Graph`, with node labels `N` and edge labels `E`. A node or edge
/// set without a label gets `N::default()` / `E::default()`.
#[derive(Debug, Clone)]
pub struct Graph<N, E> {
    directed: bool,
    multigraph: bool,
    compound: bool,
    nodes: OMap<N>,
    parent: HashMap<String, String>,
    children: HashMap<String, OMap<()>>,
    in_: HashMap<String, OMap<EdgeObj>>,
    out: HashMap<String, OMap<EdgeObj>>,
    preds: HashMap<String, OMap<usize>>,
    sucs: HashMap<String, OMap<usize>>,
    edge_objs: OMap<EdgeObj>,
    edge_labels: HashMap<String, E>,
}

fn edge_id(directed: bool, v: &str, w: &str, name: Option<&str>) -> String {
    let (v, w) = if !directed && v > w { (w, v) } else { (v, w) };
    format!("{v}\u{1}{w}\u{1}{}", name.unwrap_or("\u{0}"))
}

impl<N: Default, E: Default> Graph<N, E> {
    pub fn new(directed: bool, multigraph: bool, compound: bool) -> Self {
        let mut children = HashMap::new();
        if compound {
            children.insert(ROOT.to_string(), OMap::default());
        }
        Graph {
            directed,
            multigraph,
            compound,
            nodes: OMap::default(),
            parent: HashMap::new(),
            children,
            in_: HashMap::new(),
            out: HashMap::new(),
            preds: HashMap::new(),
            sucs: HashMap::new(),
            edge_objs: OMap::default(),
            edge_labels: HashMap::new(),
        }
    }

    pub fn is_directed(&self) -> bool {
        self.directed
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn nodes(&self) -> Vec<String> {
        self.nodes.keys()
    }

    pub fn sources(&self) -> Vec<String> {
        self.nodes()
            .into_iter()
            .filter(|v| self.in_.get(v).is_none_or(OMap::is_empty))
            .collect()
    }

    /// `setNode(v)`: adds `v` with the default label if it is new.
    pub fn ensure_node(&mut self, v: &str) {
        if !self.nodes.contains(v) {
            self.add_new(v, N::default());
        }
    }

    /// `setNode(v, label)`.
    pub fn set_node(&mut self, v: &str, label: N) {
        if let Some(l) = self.nodes.get_mut(v) {
            *l = label;
        } else {
            self.add_new(v, label);
        }
    }

    fn add_new(&mut self, v: &str, label: N) {
        self.nodes.insert(v, label);
        if self.compound {
            self.parent.insert(v.to_string(), ROOT.to_string());
            self.children.insert(v.to_string(), OMap::default());
            self.children.get_mut(ROOT).unwrap().insert(v, ());
        }
        self.in_.insert(v.to_string(), OMap::default());
        self.preds.insert(v.to_string(), OMap::default());
        self.out.insert(v.to_string(), OMap::default());
        self.sucs.insert(v.to_string(), OMap::default());
    }

    pub fn node(&self, v: &str) -> Option<&N> {
        self.nodes.get(v)
    }

    pub fn node_mut(&mut self, v: &str) -> Option<&mut N> {
        self.nodes.get_mut(v)
    }

    /// The label of a node known to exist.
    pub fn n(&self, v: &str) -> &N {
        self.nodes.get(v).expect("node")
    }

    pub fn n_mut(&mut self, v: &str) -> &mut N {
        self.nodes.get_mut(v).expect("node")
    }

    pub fn has_node(&self, v: &str) -> bool {
        self.nodes.contains(v)
    }

    pub fn remove_node(&mut self, v: &str) -> Option<N> {
        let label = self.nodes.remove(v)?;
        if self.compound {
            if let Some(p) = self.parent.remove(v) {
                if let Some(c) = self.children.get_mut(&p) {
                    c.remove(v);
                }
            }
            for child in self.children(Some(v)) {
                self.set_parent(&child, None);
            }
            self.children.remove(v);
        }
        for e in self.in_.get(v).map(|m| m.keys()).unwrap_or_default() {
            let obj = self.in_[v].get(&e).unwrap().clone();
            self.remove_edge(&obj);
        }
        self.in_.remove(v);
        self.preds.remove(v);
        for e in self.out.get(v).map(|m| m.keys()).unwrap_or_default() {
            let obj = self.out[v].get(&e).unwrap().clone();
            self.remove_edge(&obj);
        }
        self.out.remove(v);
        self.sucs.remove(v);
        Some(label)
    }

    /// `setParent(v, parent)` (`None`: the root).
    pub fn set_parent(&mut self, v: &str, parent: Option<&str>) {
        assert!(self.compound, "Cannot set parent in a non-compound graph");
        let parent = match parent {
            None => ROOT.to_string(),
            Some(p) => {
                self.ensure_node(p);
                p.to_string()
            }
        };
        self.ensure_node(v);
        if let Some(old) = self.parent.get(v).cloned() {
            if let Some(c) = self.children.get_mut(&old) {
                c.remove(v);
            }
        }
        self.parent.insert(v.to_string(), parent.clone());
        self.children.get_mut(&parent).unwrap().insert(v, ());
    }

    pub fn parent(&self, v: &str) -> Option<String> {
        if !self.compound {
            return None;
        }
        self.parent.get(v).filter(|p| p.as_str() != ROOT).cloned()
    }

    /// `children(v)` (`None`: the root's). Empty for an unknown node (where
    /// graphlib returns `undefined`; see [`Graph::has_children_map`]).
    pub fn children(&self, v: Option<&str>) -> Vec<String> {
        let key = v.unwrap_or(ROOT);
        if self.compound {
            self.children.get(key).map(OMap::keys).unwrap_or_default()
        } else if v.is_none() {
            self.nodes()
        } else {
            Vec::new()
        }
    }

    pub fn has_children(&self, v: &str) -> bool {
        self.compound && self.children.get(v).is_some_and(|c| !c.is_empty())
    }

    pub fn predecessors(&self, v: &str) -> Vec<String> {
        self.preds.get(v).map(OMap::keys).unwrap_or_default()
    }

    pub fn successors(&self, v: &str) -> Vec<String> {
        self.sucs.get(v).map(OMap::keys).unwrap_or_default()
    }

    /// `neighbors(v)`: `_.union(predecessors, successors)`.
    pub fn neighbors(&self, v: &str) -> Vec<String> {
        let mut out = self.predecessors(v);
        for s in self.successors(v) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    pub fn edges(&self) -> Vec<EdgeObj> {
        self.edge_objs.values().into_iter().cloned().collect()
    }

    /// `setEdge(v, w, label, name)`.
    pub fn set_edge(&mut self, v: &str, w: &str, label: E, name: Option<&str>) {
        self.set_edge_with(v, w, Some(label), name);
    }

    /// `setEdge(v, w)` without a label: keeps an existing edge's label.
    pub fn ensure_edge(&mut self, v: &str, w: &str, name: Option<&str>) {
        self.set_edge_with(v, w, None, name);
    }

    fn set_edge_with(&mut self, v: &str, w: &str, label: Option<E>, name: Option<&str>) {
        let e = edge_id(self.directed, v, w, name);
        if let Some(old) = self.edge_labels.get_mut(&e) {
            if let Some(l) = label {
                *old = l;
            }
            return;
        }
        assert!(
            name.is_none() || self.multigraph,
            "Cannot set a named edge when isMultigraph = false"
        );
        self.ensure_node(v);
        self.ensure_node(w);
        self.edge_labels
            .insert(e.clone(), label.unwrap_or_default());
        let (v, w) = if !self.directed && v > w {
            (w, v)
        } else {
            (v, w)
        };
        let obj = EdgeObj::new(v, w, name);
        self.edge_objs.insert(&e, obj.clone());
        let p = self.preds.get_mut(w).unwrap();
        let c = p.get(v).copied().unwrap_or(0);
        p.insert(v, c + 1);
        let s = self.sucs.get_mut(v).unwrap();
        let c = s.get(w).copied().unwrap_or(0);
        s.insert(w, c + 1);
        self.in_.get_mut(w).unwrap().insert(&e, obj.clone());
        self.out.get_mut(v).unwrap().insert(&e, obj);
    }

    pub fn edge(&self, v: &str, w: &str, name: Option<&str>) -> Option<&E> {
        self.edge_labels.get(&edge_id(self.directed, v, w, name))
    }

    pub fn edge_mut(&mut self, v: &str, w: &str, name: Option<&str>) -> Option<&mut E> {
        self.edge_labels
            .get_mut(&edge_id(self.directed, v, w, name))
    }

    pub fn e(&self, e: &EdgeObj) -> &E {
        self.edge(&e.v, &e.w, e.name.as_deref()).expect("edge")
    }

    pub fn e_mut(&mut self, e: &EdgeObj) -> &mut E {
        self.edge_mut(&e.v, &e.w, e.name.as_deref()).expect("edge")
    }

    pub fn has_edge(&self, v: &str, w: &str, name: Option<&str>) -> bool {
        self.edge_labels
            .contains_key(&edge_id(self.directed, v, w, name))
    }

    pub fn remove_edge(&mut self, e: &EdgeObj) -> Option<E> {
        let id = edge_id(self.directed, &e.v, &e.w, e.name.as_deref());
        let obj = self.edge_objs.remove(&id)?;
        let label = self.edge_labels.remove(&id);
        let (v, w) = (obj.v.as_str(), obj.w.as_str());
        if let Some(p) = self.preds.get_mut(w) {
            dec(p, v);
        }
        if let Some(s) = self.sucs.get_mut(v) {
            dec(s, w);
        }
        if let Some(m) = self.in_.get_mut(w) {
            m.remove(&id);
        }
        if let Some(m) = self.out.get_mut(v) {
            m.remove(&id);
        }
        label
    }

    pub fn in_edges(&self, v: &str) -> Vec<EdgeObj> {
        self.in_
            .get(v)
            .map(|m| m.values().into_iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn out_edges(&self, v: &str) -> Vec<EdgeObj> {
        self.out
            .get(v)
            .map(|m| m.values().into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// `nodeEdges(v)`: in-edges, then out-edges.
    pub fn node_edges(&self, v: &str) -> Vec<EdgeObj> {
        let mut out = self.in_edges(v);
        out.extend(self.out_edges(v));
        out
    }
}

fn dec(m: &mut OMap<usize>, k: &str) {
    if let Some(c) = m.get_mut(k) {
        *c -= 1;
        if *c == 0 {
            m.remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_enumerate_like_a_js_object() {
        let mut m = OMap::default();
        for k in ["b", "10", "a", "2", "01", "-1"] {
            m.insert(k, ());
        }
        assert_eq!(m.keys(), ["2", "10", "b", "a", "01", "-1"]);
        m.remove("b");
        m.insert("b", ());
        assert_eq!(m.keys(), ["2", "10", "a", "01", "-1", "b"]);
    }

    #[test]
    fn graph_keeps_graphlib_orders() {
        let mut g: Graph<(), ()> = Graph::new(true, true, true);
        g.set_edge("x", "y", (), Some("e1"));
        g.set_edge("x", "1", (), Some("e2"));
        g.set_parent("y", Some("c"));
        assert_eq!(g.nodes(), ["1", "x", "y", "c"]);
        assert_eq!(g.successors("x"), ["1", "y"]);
        assert_eq!(g.children(Some("c")), ["y"]);
        assert_eq!(g.children(None), ["1", "x", "c"]);
        g.remove_node("x");
        assert!(g.edges().is_empty());
        let mut u: Graph<(), ()> = Graph::new(false, false, false);
        u.set_edge("b", "a", (), None);
        assert!(u.has_edge("a", "b", None));
        assert_eq!(u.neighbors("b"), ["a"]);
    }
}
