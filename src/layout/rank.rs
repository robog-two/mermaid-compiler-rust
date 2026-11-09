//! Network-simplex ranking (Gansner, Koutsofios, North & Vo, 1993), the method
//! dagre uses: minimise Σ weight·(rank[head] − rank[tail]) subject to
//! rank[head] − rank[tail] ≥ minlen. Runs per weakly-connected component on an
//! acyclic graph whose `rank` is already feasible.

/// A simplified (parallel edges merged) DAG edge.
#[derive(Debug, Clone, Copy)]
pub struct RankEdge {
    pub tail: usize,
    pub head: usize,
    pub minlen: i64,
    pub weight: f64,
}

/// Improve a feasible ranking in place. `max_iters` bounds pivoting per component.
pub fn network_simplex(n: usize, edges: &[RankEdge], rank: &mut [i64], max_iters: usize) -> usize {
    let mut pivots = 0;
    // Merge parallel edges, as dagre's `simplify` does.
    let mut merged: Vec<RankEdge> = Vec::with_capacity(edges.len());
    let mut seen = std::collections::HashMap::with_capacity(edges.len());
    for e in edges {
        match seen.get(&(e.tail, e.head)) {
            Some(&i) => {
                let m: &mut RankEdge = &mut merged[i];
                m.weight += e.weight;
                m.minlen = m.minlen.max(e.minlen);
            }
            None => {
                seen.insert((e.tail, e.head), merged.len());
                merged.push(*e);
            }
        }
    }
    // Components via union-find.
    let mut uf: Vec<usize> = (0..n).collect();
    fn find(uf: &mut [usize], mut x: usize) -> usize {
        while uf[x] != x {
            uf[x] = uf[uf[x]];
            x = uf[x];
        }
        x
    }
    for e in &merged {
        let (a, b) = (find(&mut uf, e.tail), find(&mut uf, e.head));
        if a != b {
            uf[a] = b;
        }
    }
    let mut comp_nodes: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for v in 0..n {
        let r = find(&mut uf, v);
        comp_nodes.entry(r).or_default().push(v);
    }
    let mut comp_edges: std::collections::HashMap<usize, Vec<RankEdge>> =
        std::collections::HashMap::new();
    for e in &merged {
        let r = find(&mut uf, e.tail);
        comp_edges.entry(r).or_default().push(*e);
    }
    for (root, nodes) in comp_nodes {
        if nodes.len() < 2 {
            continue;
        }
        let mut local = vec![usize::MAX; n];
        for (i, &v) in nodes.iter().enumerate() {
            local[v] = i;
        }
        let es: Vec<RankEdge> = comp_edges
            .remove(&root)
            .unwrap_or_default()
            .into_iter()
            .map(|e| RankEdge {
                tail: local[e.tail],
                head: local[e.head],
                ..e
            })
            .collect();
        let mut r: Vec<i64> = nodes.iter().map(|&v| rank[v]).collect();
        pivots += Simplex::new(nodes.len(), &es).run(&mut r, max_iters);
        for (i, &v) in nodes.iter().enumerate() {
            rank[v] = r[i];
        }
    }
    pivots
}

struct Simplex<'a> {
    n: usize,
    edges: &'a [RankEdge],
    /// Incident edge ids per node.
    inc: Vec<Vec<usize>>,
    in_tree_edge: Vec<bool>,
    /// Tree adjacency: (neighbour, edge id).
    tree_adj: Vec<Vec<(usize, usize)>>,
    parent: Vec<Option<(usize, usize)>>,
    low: Vec<usize>,
    lim: Vec<usize>,
    cut: Vec<f64>,
}

impl<'a> Simplex<'a> {
    fn new(n: usize, edges: &'a [RankEdge]) -> Self {
        let mut inc = vec![Vec::new(); n];
        for (i, e) in edges.iter().enumerate() {
            inc[e.tail].push(i);
            inc[e.head].push(i);
        }
        Simplex {
            n,
            edges,
            inc,
            in_tree_edge: vec![false; edges.len()],
            tree_adj: vec![Vec::new(); n],
            parent: vec![None; n],
            low: vec![0; n],
            lim: vec![0; n],
            cut: vec![0.0; edges.len()],
        }
    }

    fn slack(&self, rank: &[i64], e: usize) -> i64 {
        let e = &self.edges[e];
        rank[e.head] - rank[e.tail] - e.minlen
    }

    fn other(&self, e: usize, v: usize) -> usize {
        let ed = &self.edges[e];
        if ed.tail == v { ed.head } else { ed.tail }
    }

    fn add_tree_edge(&mut self, e: usize) {
        self.in_tree_edge[e] = true;
        let (t, h) = (self.edges[e].tail, self.edges[e].head);
        self.tree_adj[t].push((h, e));
        self.tree_adj[h].push((t, e));
    }

    fn remove_tree_edge(&mut self, e: usize) {
        self.in_tree_edge[e] = false;
        let (t, h) = (self.edges[e].tail, self.edges[e].head);
        self.tree_adj[t].retain(|&(_, x)| x != e);
        self.tree_adj[h].retain(|&(_, x)| x != e);
    }

    /// Build a spanning tree of tight edges, shifting ranks as needed (dagre's `feasibleTree`).
    fn feasible_tree(&mut self, rank: &mut [i64]) {
        let mut in_tree = vec![false; self.n];
        in_tree[0] = true;
        let mut size = 1;
        let mut members = vec![0usize];
        loop {
            // Grow through tight edges.
            let mut stack = members.clone();
            while let Some(v) = stack.pop() {
                for i in 0..self.inc[v].len() {
                    let e = self.inc[v][i];
                    let w = self.other(e, v);
                    if !in_tree[w] && self.slack(rank, e) == 0 {
                        in_tree[w] = true;
                        size += 1;
                        members.push(w);
                        stack.push(w);
                        self.add_tree_edge(e);
                    }
                }
            }
            if size >= self.n {
                break;
            }
            // Tighten the minimum-slack edge leaving the tree.
            let best = (0..self.edges.len())
                .filter(|&e| in_tree[self.edges[e].tail] != in_tree[self.edges[e].head])
                .min_by_key(|&e| self.slack(rank, e));
            let Some(e) = best else { break };
            let s = self.slack(rank, e);
            let delta = if in_tree[self.edges[e].tail] { s } else { -s };
            for &v in &members {
                rank[v] += delta;
            }
        }
    }

    /// Post-order numbering (`low`/`lim`) and parents from node 0.
    fn init_low_lim(&mut self) {
        self.parent.iter_mut().for_each(|p| *p = None);
        let mut next = 1usize;
        let mut visited = vec![false; self.n];
        // (node, index into tree_adj, low)
        let mut stack: Vec<(usize, usize, usize)> = vec![(0, 0, next)];
        visited[0] = true;
        while let Some(&mut (v, ref mut i, low)) = stack.last_mut() {
            if *i < self.tree_adj[v].len() {
                let (w, e) = self.tree_adj[v][*i];
                *i += 1;
                if !visited[w] {
                    visited[w] = true;
                    self.parent[w] = Some((v, e));
                    stack.push((w, 0, next));
                }
            } else {
                self.low[v] = low;
                self.lim[v] = next;
                next += 1;
                stack.pop();
            }
        }
    }

    fn postorder(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.n).collect();
        order.sort_by_key(|&v| self.lim[v]);
        order
    }

    fn calc_cut(&self, child: usize) -> f64 {
        let Some((_, pe)) = self.parent[child] else {
            return 0.0;
        };
        let child_is_tail = self.edges[pe].tail == child;
        let mut cut = self.edges[pe].weight;
        for &e in &self.inc[child] {
            if e == pe {
                continue;
            }
            let is_out = self.edges[e].tail == child;
            let other = self.other(e, child);
            let points_to_head = is_out == child_is_tail;
            let w = self.edges[e].weight;
            cut += if points_to_head { w } else { -w };
            if self.in_tree_edge[e] && self.parent[other].map(|(p, _)| p) == Some(child) {
                let oc = self.cut[e];
                cut += if points_to_head { -oc } else { oc };
            }
        }
        cut
    }

    fn init_cut_values(&mut self) {
        for v in self.postorder() {
            if let Some((_, e)) = self.parent[v] {
                self.cut[e] = self.calc_cut(v);
            }
        }
    }

    fn is_descendant(&self, v: usize, root: usize) -> bool {
        self.low[root] <= self.lim[v] && self.lim[v] <= self.lim[root]
    }

    fn update_ranks(&self, rank: &mut [i64]) {
        let mut order: Vec<usize> = (0..self.n).collect();
        order.sort_by_key(|&v| std::cmp::Reverse(self.lim[v])); // root first (pre-order-ish)
        for v in order {
            if let Some((p, e)) = self.parent[v] {
                let ed = &self.edges[e];
                rank[v] = if ed.tail == v {
                    rank[p] - ed.minlen
                } else {
                    rank[p] + ed.minlen
                };
            }
        }
    }

    /// Returns the number of pivots performed.
    fn run(mut self, rank: &mut [i64], max_iters: usize) -> usize {
        self.feasible_tree(rank);
        if self.in_tree_edge.iter().filter(|&&t| t).count() + 1 < self.n {
            return 0; // not connected (should not happen per component)
        }
        self.init_low_lim();
        self.init_cut_values();
        let mut pivots = 0;
        for _ in 0..max_iters {
            pivots += 1;
            let Some(leave) =
                (0..self.edges.len()).find(|&e| self.in_tree_edge[e] && self.cut[e] < 0.0)
            else {
                break;
            };
            // The tree child endpoint of the leaving edge.
            let (t, h) = (self.edges[leave].tail, self.edges[leave].head);
            let (tail_root, flip) = if self.lim[t] > self.lim[h] {
                (h, true)
            } else {
                (t, false)
            };
            let enter = (0..self.edges.len())
                .filter(|&e| {
                    let ed = &self.edges[e];
                    flip == self.is_descendant(ed.tail, tail_root)
                        && flip != self.is_descendant(ed.head, tail_root)
                })
                .min_by_key(|&e| self.slack(rank, e));
            let Some(enter) = enter else { break };
            self.remove_tree_edge(leave);
            self.add_tree_edge(enter);
            self.init_low_lim();
            self.init_cut_values();
            // Ranks: re-derive from the tree (root keeps its rank).
            self.update_ranks(rank);
        }
        pivots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(tail: usize, head: usize) -> RankEdge {
        RankEdge {
            tail,
            head,
            minlen: 1,
            weight: 1.0,
        }
    }

    fn total(edges: &[RankEdge], r: &[i64]) -> i64 {
        edges.iter().map(|e| r[e.head] - r[e.tail]).sum()
    }

    #[test]
    fn pulls_late_chain_down() {
        // 0→1→2→3→4 main chain; 5→6→4 side chain starts at the top under longest-path.
        let edges = [e(0, 1), e(1, 2), e(2, 3), e(3, 4), e(5, 6), e(6, 4)];
        let mut r = vec![0, 1, 2, 3, 4, 0, 1];
        network_simplex(7, &edges, &mut r, 100);
        assert_eq!(r[4] - r[6], 1);
        assert_eq!(r[6] - r[5], 1);
        assert_eq!(total(&edges, &r), 6);
        for ed in &edges {
            assert!(r[ed.head] - r[ed.tail] >= ed.minlen);
        }
    }

    #[test]
    fn group_move_beats_greedy() {
        // A diamond hanging off a late source: needs moving as a group.
        let edges = [
            e(0, 1),
            e(1, 2),
            e(2, 3),
            e(3, 4),
            e(4, 5),
            e(6, 7),
            e(6, 8),
            e(7, 9),
            e(8, 9),
            e(9, 5),
            e(7, 5),
        ];
        let mut r = vec![0, 1, 2, 3, 4, 5, 0, 1, 1, 2];
        let before = total(&edges, &r);
        network_simplex(10, &edges, &mut r, 100);
        assert!(total(&edges, &r) < before);
        for ed in &edges {
            assert!(r[ed.head] - r[ed.tail] >= ed.minlen);
        }
    }
}
