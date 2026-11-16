//! fcose's spectral draft layout (`spectralLayout`, cytoscape-fcose 2.2.0)
//! and the layout-base 2.0.1 `Matrix` / `SVD` helpers it and the constraint
//! transformation use. fcose runs it before CoSE when `randomize` is true:
//! a greedy sample of BFS pivots, the distance matrix's regularised
//! pseudo-inverse and two power iterations starting from `Math.random`
//! vectors (mermaid's seeded mulberry32), every step in the same order of
//! floating-point operations as the JavaScript.

use super::fcose::{Edge, Mulberry32, Node};

/// The spectral result: a centre per cytoscape node (`None` for compounds,
/// which fcose sizes from their members instead). A coordinate is NaN where
/// fcose's own arithmetic produced one (see [`layout`]).
pub(super) type Centers = Vec<Option<(f64, f64)>>;

const INFINITY: f64 = 1e8;
const SMALL: f64 = 1e-9;
const PI_TOL: f64 = 1e-7;
const SAMPLE_SIZE: usize = 25;

/// Cytoscape's view of the elements the layout runs on.
struct Graph<'a> {
    nodes: &'a [Node],
    children: Vec<Vec<usize>>,
    /// Each node's connected edges, in the order they were added (a loop
    /// once).
    node_edges: Vec<Vec<usize>>,
    edges: &'a [Edge],
}

impl Graph<'_> {
    fn is_parent(&self, v: usize) -> bool {
        !self.children[v].is_empty()
    }

    /// `descendants()`: children depth first, each before its own.
    fn descendants(&self, v: usize, out: &mut Vec<usize>) {
        for &c in &self.children[v] {
            out.push(c);
            self.descendants(c, out);
        }
    }

    /// `neighborhood().nodes()`: the other end of each connected edge (the
    /// node itself for a loop), first occurrence first.
    fn neighborhood(&self, v: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for &e in &self.node_edges[v] {
            let ed = &self.edges[e];
            let other = if ed.source == v { ed.target } else { ed.source };
            if !out.contains(&other) {
                out.push(other);
            }
        }
        out
    }

    fn degree(&self, v: usize) -> usize {
        self.node_edges[v].len()
    }

    fn parent(&self, v: usize) -> Option<usize> {
        self.nodes[v].parent.filter(|&p| p < self.nodes.len())
    }

    /// `getTopMostNodes`: the members of `set` none of whose ancestors is in
    /// `set`, in `set`'s order.
    fn top_most(&self, set: &[usize]) -> Vec<usize> {
        set.iter()
            .copied()
            .filter(|&v| {
                let mut p = self.parent(v);
                while let Some(q) = p {
                    if set.contains(&q) {
                        return false;
                    }
                    p = self.parent(q);
                }
                true
            })
            .collect()
    }

    /// `aux.connectComponents`: one dummy per call joined to the least
    /// connected top-most node of each component, when there are several.
    fn connect_components(&self, mut top: Vec<usize>, dummies: &mut Vec<Vec<usize>>) {
        let mut visited = vec![false; self.nodes.len()];
        let mut visited_top: Vec<usize> = Vec::new();
        let mut connected = false;
        let mut count = 1;
        let mut to_dummy = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        loop {
            let first = top[0];
            let mut members = vec![first];
            self.descendants(first, &mut members);
            visited_top.push(first);
            for v in members {
                queue.push_back(v);
                visited[v] = true;
            }
            while let Some(cur) = queue.pop_front() {
                for nb in self.neighborhood(cur) {
                    // The top-most node that is `nb` or one of its ancestors.
                    let mut a = Some(nb);
                    let mut hit = None;
                    while let Some(q) = a {
                        if top.contains(&q) {
                            hit = Some(q);
                            break;
                        }
                        a = self.parent(q);
                    }
                    let Some(t) = hit else { continue };
                    if visited[t] {
                        continue;
                    }
                    let mut members = vec![t];
                    self.descendants(t, &mut members);
                    for v in members {
                        queue.push_back(v);
                        visited[v] = true;
                        if top.contains(&v) {
                            visited_top.push(v);
                        }
                    }
                }
            }
            if visited_top.len() == top.len() {
                connected = true;
            }
            if !connected || count > 1 {
                let mut min_node = visited_top[0];
                let mut min = self.degree(min_node);
                for &v in &visited_top {
                    if self.degree(v) < min {
                        min = self.degree(v);
                        min_node = v;
                    }
                }
                to_dummy.push(min_node);
                top.retain(|v| !visited_top.contains(v));
                visited_top.clear();
                count += 1;
            }
            if connected {
                break;
            }
        }
        if !to_dummy.is_empty() {
            dummies.push(to_dummy);
        }
    }
}

/// `spectralLayout` over every element. `positions` are the cytoscape
/// positions, which only a graph of at most two nodes (dummies included)
/// keeps: the first stays where it is and the second gets x =
/// `NaN` (fcose adds mermaid's `idealEdgeLength` function to a number, a
/// string that is not a number).
pub(super) fn layout(
    nodes: &[Node],
    edges: &[Edge],
    positions: &[(f64, f64)],
    node_separation: f64,
    rng: &mut Mulberry32,
) -> Centers {
    let n = nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, nd) in nodes.iter().enumerate() {
        if let Some(p) = nd.parent.filter(|&p| p < n) {
            children[p].push(i);
        }
    }
    let mut node_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, e) in edges.iter().enumerate() {
        if e.source >= n || e.target >= n {
            continue;
        }
        node_edges[e.source].push(k);
        if e.target != e.source {
            node_edges[e.target].push(k);
        }
    }
    let g = Graph {
        nodes,
        children,
        node_edges,
        edges,
    };
    let mut out: Centers = vec![None; n];
    if n == 0 {
        return out;
    }
    let all: Vec<usize> = (0..n).collect();
    let mut dummies: Vec<Vec<usize>> = Vec::new();
    g.connect_components(g.top_most(&all), &mut dummies);
    for p in 0..n {
        if g.is_parent(p) {
            let mut desc = Vec::new();
            g.descendants(p, &mut desc);
            g.connect_components(g.top_most(&desc), &mut dummies);
        }
    }
    // nodeIndexes: the leaves, then the dummies.
    let mut index_of = vec![usize::MAX; n];
    let mut leaves = Vec::new();
    for (v, slot) in index_of.iter_mut().enumerate() {
        if !g.is_parent(v) {
            *slot = leaves.len();
            leaves.push(v);
        }
    }
    let size = leaves.len() + dummies.len();
    // parentChildMap: a compound stands for its least connected leaf child.
    let mut rep = vec![usize::MAX; n];
    for (p, slot) in rep.iter_mut().enumerate() {
        if !g.is_parent(p) {
            continue;
        }
        let mut kids = g.children[p].clone();
        while !kids.iter().any(|&c| !g.is_parent(c)) {
            kids = g.children[kids[0]].clone();
        }
        let childless: Vec<usize> = kids.into_iter().filter(|&c| !g.is_parent(c)).collect();
        let mut best = 0;
        let mut min = g.degree(childless[0]);
        for (i, &c) in childless.iter().enumerate() {
            if g.degree(c) < min {
                min = g.degree(c);
                best = i;
            }
        }
        *slot = childless[best];
    }
    let idx = |v: usize| {
        if g.is_parent(v) {
            index_of[rep[v]]
        } else {
            index_of[v]
        }
    };
    let mut neigh: Vec<Vec<usize>> = vec![Vec::new(); size];
    for v in 0..n {
        let i = idx(v);
        for nb in g.neighborhood(v) {
            neigh[i].push(idx(nb));
        }
    }
    for (k, list) in dummies.iter().enumerate() {
        let d = leaves.len() + k;
        for &v in list {
            let i = idx(v);
            neigh[d].push(i);
            neigh[i].push(d);
        }
    }

    let (xs, ys) = if size > 2 {
        draft(&neigh, node_separation, rng)
    } else {
        let first = positions.get(leaves[0]).copied().unwrap_or((0.0, 0.0));
        let mut xs = vec![first.0];
        let mut ys = vec![first.1];
        if size == 2 {
            xs.push(f64::NAN);
            ys.push(first.1);
        }
        (xs, ys)
    };
    for (i, &v) in leaves.iter().enumerate() {
        out[v] = Some((xs[i], ys[i]));
    }
    out
}

/// `allBFS(true)`, `sample()` and `powerIteration()`.
fn draft(neigh: &[Vec<usize>], node_separation: f64, rng: &mut Mulberry32) -> (Vec<f64>, Vec<f64>) {
    let size = neigh.len();
    let sample_size = size.min(SAMPLE_SIZE);
    // Entries BFS never reaches stay `undefined` in fcose: NaN here.
    let mut c = vec![vec![f64::NAN; sample_size]; size];
    let mut samples = vec![0usize; sample_size];
    let mut min_dist = vec![INFINITY; size];
    let bfs = |c: &mut Vec<Vec<f64>>, min_dist: &mut Vec<f64>, pivot: usize, col: usize| {
        let mut distance = vec![INFINITY; size];
        let mut path = vec![pivot];
        let mut front = 0;
        distance[pivot] = 0.0;
        while front < path.len() {
            let cur = path[front];
            front += 1;
            for &nb in &neigh[cur] {
                if distance[nb] == INFINITY {
                    distance[nb] = distance[cur] + 1.0;
                    path.push(nb);
                }
            }
            c[cur][col] = distance[cur] * node_separation;
        }
        let mut max_dist = 0.0;
        let mut max_ind = 1;
        for i in 0..size {
            if c[i][col] < min_dist[i] {
                min_dist[i] = c[i][col];
            }
        }
        for (i, &d) in min_dist.iter().enumerate() {
            if d > max_dist {
                max_dist = d;
                max_ind = i;
            }
        }
        max_ind
    };
    let mut sample = (rng.next() * size as f64).floor() as usize;
    for (col, s) in samples.iter_mut().enumerate() {
        *s = sample;
        sample = bfs(&mut c, &mut min_dist, sample, col);
    }
    for row in c.iter_mut() {
        for v in row.iter_mut() {
            *v *= *v;
        }
    }
    let mut phi = vec![vec![0.0; sample_size]; sample_size];
    for (i, row) in phi.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = c[samples[j]][i];
        }
    }
    // sample(): the regularised pseudo-inverse of PHI.
    let svd = svd(phi);
    let q = &svd.s;
    let max_s = q[0] * q[0] * q[0];
    let mut sig = vec![vec![0.0; sample_size]; sample_size];
    for (i, row) in sig.iter_mut().enumerate() {
        row[i] = q[i] / (q[i] * q[i] + max_s / (q[i] * q[i]));
    }
    let inv = mult_mat(&mult_mat(&svd.v, &sig), &transpose(&svd.u));

    // powerIteration().
    let mut y1 = vec![0.0; size];
    let mut y2 = vec![0.0; size];
    for i in 0..size {
        y1[i] = rng.next();
        y2[i] = rng.next();
    }
    let mut y1 = normalize(&y1);
    let mut y2 = normalize(&y2);
    let step = |v: &[f64]| mult_gamma(&mult_l(&mult_gamma(v), &c, &inv));
    let converged = |current: f64, previous: f64| {
        let t = (current / previous).abs();
        (1.0..=1.0 + PI_TOL).contains(&t)
    };
    // fcose loops until the ratio settles; a NaN never does, so stop
    // after far more iterations than any finite run needs.
    const GUARD: usize = 1_000_000;
    let mut previous = SMALL;
    let mut theta1;
    let mut v1;
    let mut guard = 0;
    loop {
        v1 = y1.clone();
        y1 = step(&v1);
        theta1 = dot(&v1, &y1);
        y1 = normalize(&y1);
        let current = dot(&v1, &y1);
        guard += 1;
        if converged(current, previous) || guard > GUARD {
            break;
        }
        previous = current;
    }
    v1 = y1;
    previous = SMALL;
    let mut theta2;
    guard = 0;
    loop {
        let mut v2 = y2.clone();
        v2 = minus(&v2, &mult_cons(&v1, dot(&v1, &v2)));
        y2 = step(&v2);
        theta2 = dot(&v2, &y2);
        y2 = normalize(&y2);
        let current = dot(&v2, &y2);
        guard += 1;
        if converged(current, previous) || guard > GUARD {
            break;
        }
        previous = current;
    }
    (
        mult_cons(&v1, theta1.abs().sqrt()),
        mult_cons(&y2, theta2.abs().sqrt()),
    )
}

/// `Matrix.multMat`.
pub(super) fn mult_mat(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let (inner, cols) = (a[0].len(), b[0].len());
    a.iter()
        .map(|row| {
            (0..cols)
                .map(|j| {
                    let mut s = 0.0;
                    for k in 0..inner {
                        s += row[k] * b[k][j];
                    }
                    s
                })
                .collect()
        })
        .collect()
}

/// `Matrix.transpose`.
pub(super) fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    (0..a[0].len())
        .map(|i| a.iter().map(|row| row[i]).collect())
        .collect()
}

fn mult_cons(a: &[f64], k: f64) -> Vec<f64> {
    a.iter().map(|v| v * k).collect()
}

fn minus(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x - y).collect()
}

/// `Matrix.dotProduct`.
pub(super) fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for (x, y) in a.iter().zip(b) {
        s += x * y;
    }
    s
}

fn normalize(a: &[f64]) -> Vec<f64> {
    let mag = dot(a, a).sqrt();
    a.iter().map(|v| v / mag).collect()
}

/// `Matrix.multGamma`: the vector minus its mean.
pub(super) fn mult_gamma(a: &[f64]) -> Vec<f64> {
    let mut sum = 0.0;
    for v in a {
        sum += v;
    }
    sum *= -1.0 / a.len() as f64;
    a.iter().map(|v| sum + v).collect()
}

/// `Matrix.multL`: `C · INV · (-½ Cᵀ · a)`.
fn mult_l(a: &[f64], c: &[Vec<f64>], inv: &[Vec<f64>]) -> Vec<f64> {
    let cols = c[0].len();
    let temp1: Vec<f64> = (0..cols)
        .map(|i| {
            let mut s = 0.0;
            for (j, row) in c.iter().enumerate() {
                s += -0.5 * row[i] * a[j];
            }
            s
        })
        .collect();
    let temp2: Vec<f64> = inv
        .iter()
        .map(|row| {
            let mut s = 0.0;
            for j in 0..inv.len() {
                s += row[j] * temp1[j];
            }
            s
        })
        .collect();
    c.iter()
        .map(|row| {
            let mut s = 0.0;
            for j in 0..cols {
                s += row[j] * temp2[j];
            }
            s
        })
        .collect()
}

pub(super) struct Svd {
    pub u: Vec<Vec<f64>>,
    pub v: Vec<Vec<f64>>,
    pub s: Vec<f64>,
}

/// `SVD.hypot`.
fn hypot(a: f64, b: f64) -> f64 {
    if a.abs() > b.abs() {
        let r = b / a;
        a.abs() * (1.0 + r * r).sqrt()
    } else if b != 0.0 {
        let r = a / b;
        b.abs() * (1.0 + r * r).sqrt()
    } else {
        0.0
    }
}

/// layout-base's `SVD.svd` (JAMA's one-sided Golub–Kahan), quirks included.
#[allow(clippy::many_single_char_names, clippy::needless_range_loop)]
pub(super) fn svd(mut a: Vec<Vec<f64>>) -> Svd {
    let m = a.len();
    let n = a[0].len();
    let nu = m.min(n);
    let mut s = vec![0.0; (m + 1).min(n)];
    let mut u = vec![vec![0.0; nu]; m];
    let mut v = vec![vec![0.0; n]; n];
    let mut e = vec![0.0; n];
    let mut work = vec![0.0; m];
    let nct = (m as isize - 1).min(n as isize).max(0) as usize;
    let nrt = (n as isize - 2).min(m as isize).max(0) as usize;
    for k in 0..nct.max(nrt) {
        if k < nct {
            s[k] = 0.0;
            for i in k..m {
                s[k] = hypot(s[k], a[i][k]);
            }
            if s[k] != 0.0 {
                if a[k][k] < 0.0 {
                    s[k] = -s[k];
                }
                for i in k..m {
                    a[i][k] /= s[k];
                }
                a[k][k] += 1.0;
            }
            s[k] = -s[k];
        }
        for j in k + 1..n {
            if k < nct && s[k] != 0.0 {
                let mut t = 0.0;
                for i in k..m {
                    t += a[i][k] * a[i][j];
                }
                t = -t / a[k][k];
                for i in k..m {
                    a[i][j] += t * a[i][k];
                }
            }
            e[j] = a[k][j];
        }
        if k < nct {
            for i in k..m {
                u[i][k] = a[i][k];
            }
        }
        if k < nrt {
            e[k] = 0.0;
            for i in k + 1..n {
                e[k] = hypot(e[k], e[i]);
            }
            if e[k] != 0.0 {
                if e[k + 1] < 0.0 {
                    e[k] = -e[k];
                }
                for i in k + 1..n {
                    e[i] /= e[k];
                }
                e[k + 1] += 1.0;
            }
            e[k] = -e[k];
            if k + 1 < m && e[k] != 0.0 {
                for i in k + 1..m {
                    work[i] = 0.0;
                }
                for j in k + 1..n {
                    for i in k + 1..m {
                        work[i] += e[j] * a[i][j];
                    }
                }
                for j in k + 1..n {
                    let t = -e[j] / e[k + 1];
                    for i in k + 1..m {
                        a[i][j] += t * work[i];
                    }
                }
            }
            for i in k + 1..n {
                v[i][k] = e[i];
            }
        }
    }
    let mut p = n.min(m + 1);
    if nct < n {
        s[nct] = a[nct][nct];
    }
    if m < p {
        s[p - 1] = 0.0;
    }
    if nrt + 1 < p {
        e[nrt] = a[nrt][p - 1];
    }
    e[p - 1] = 0.0;
    for j in nct..nu {
        for i in 0..m {
            u[i][j] = 0.0;
        }
        u[j][j] = 1.0;
    }
    for k in (0..nct).rev() {
        if s[k] != 0.0 {
            for j in k + 1..nu {
                let mut t = 0.0;
                for i in k..m {
                    t += u[i][k] * u[i][j];
                }
                t = -t / u[k][k];
                for i in k..m {
                    u[i][j] += t * u[i][k];
                }
            }
            for i in k..m {
                u[i][k] = -u[i][k];
            }
            u[k][k] += 1.0;
            // JAMA clears rows 0..k; layout-base stops one short.
            for i in 0..k.saturating_sub(1) {
                u[i][k] = 0.0;
            }
        } else {
            for i in 0..m {
                u[i][k] = 0.0;
            }
            u[k][k] = 1.0;
        }
    }
    for k in (0..n).rev() {
        if k < nrt && e[k] != 0.0 {
            for j in k + 1..nu {
                let mut t = 0.0;
                for i in k + 1..n {
                    t += v[i][k] * v[i][j];
                }
                t = -t / v[k + 1][k];
                for i in k + 1..n {
                    v[i][j] += t * v[i][k];
                }
            }
        }
        for i in 0..n {
            v[i][k] = 0.0;
        }
        v[k][k] = 1.0;
    }
    let pp = p - 1;
    // Math.pow(2, -52) and Math.pow(2, -966).
    let eps = f64::EPSILON;
    let tiny = f64::from_bits(57 << 52);
    while p > 0 {
        // `k` runs down to -1 in the JavaScript.
        let mut k: isize = p as isize - 2;
        while k >= 0 {
            let ku = k as usize;
            if e[ku].abs() <= tiny + eps * (s[ku].abs() + s[ku + 1].abs()) {
                e[ku] = 0.0;
                break;
            }
            k -= 1;
        }
        let kase;
        if k == p as isize - 2 {
            kase = 4;
        } else {
            let mut ks: isize = p as isize - 1;
            while ks >= k {
                if ks == k {
                    break;
                }
                let ksu = ks as usize;
                let t = (if ks != p as isize { e[ksu].abs() } else { 0.0 })
                    + (if ks != k + 1 { e[ksu - 1].abs() } else { 0.0 });
                if s[ksu].abs() <= tiny + eps * t {
                    s[ksu] = 0.0;
                    break;
                }
                ks -= 1;
            }
            if ks == k {
                kase = 3;
            } else if ks == p as isize - 1 {
                kase = 1;
            } else {
                kase = 2;
                k = ks;
            }
        }
        k += 1;
        let k = k as usize;
        match kase {
            1 => {
                let mut f = e[p - 2];
                e[p - 2] = 0.0;
                let mut j = p - 2;
                loop {
                    if j < k {
                        break;
                    }
                    let t = hypot(s[j], f);
                    let cs = s[j] / t;
                    let sn = f / t;
                    s[j] = t;
                    if j != k {
                        f = -sn * e[j - 1];
                        e[j - 1] *= cs;
                    }
                    for i in 0..n {
                        let t = cs * v[i][j] + sn * v[i][p - 1];
                        v[i][p - 1] = -sn * v[i][j] + cs * v[i][p - 1];
                        v[i][j] = t;
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                }
            }
            2 => {
                let mut f = e[k - 1];
                e[k - 1] = 0.0;
                for j in k..p {
                    let t = hypot(s[j], f);
                    let cs = s[j] / t;
                    let sn = f / t;
                    s[j] = t;
                    f = -sn * e[j];
                    e[j] *= cs;
                    for i in 0..m {
                        let t = cs * u[i][j] + sn * u[i][k - 1];
                        u[i][k - 1] = -sn * u[i][j] + cs * u[i][k - 1];
                        u[i][j] = t;
                    }
                }
            }
            3 => {
                let scale = s[p - 1]
                    .abs()
                    .max(s[p - 2].abs())
                    .max(e[p - 2].abs())
                    .max(s[k].abs())
                    .max(e[k].abs());
                let sp = s[p - 1] / scale;
                let spm1 = s[p - 2] / scale;
                let epm1 = e[p - 2] / scale;
                let sk = s[k] / scale;
                let ek = e[k] / scale;
                let b = ((spm1 + sp) * (spm1 - sp) + epm1 * epm1) / 2.0;
                let c = sp * epm1 * (sp * epm1);
                let mut shift = 0.0;
                if b != 0.0 || c != 0.0 {
                    shift = (b * b + c).sqrt();
                    if b < 0.0 {
                        shift = -shift;
                    }
                    shift = c / (b + shift);
                }
                let mut f = (sk + sp) * (sk - sp) + shift;
                let mut g = sk * ek;
                for j in k..p - 1 {
                    let mut t = hypot(f, g);
                    let mut cs = f / t;
                    let mut sn = g / t;
                    if j != k {
                        e[j - 1] = t;
                    }
                    f = cs * s[j] + sn * e[j];
                    e[j] = cs * e[j] - sn * s[j];
                    g = sn * s[j + 1];
                    s[j + 1] *= cs;
                    for i in 0..n {
                        t = cs * v[i][j] + sn * v[i][j + 1];
                        v[i][j + 1] = -sn * v[i][j] + cs * v[i][j + 1];
                        v[i][j] = t;
                    }
                    t = hypot(f, g);
                    cs = f / t;
                    sn = g / t;
                    s[j] = t;
                    f = cs * e[j] + sn * s[j + 1];
                    s[j + 1] = -sn * e[j] + cs * s[j + 1];
                    g = sn * e[j + 1];
                    e[j + 1] *= cs;
                    if j < m - 1 {
                        for i in 0..m {
                            t = cs * u[i][j] + sn * u[i][j + 1];
                            u[i][j + 1] = -sn * u[i][j] + cs * u[i][j + 1];
                            u[i][j] = t;
                        }
                    }
                }
                e[p - 2] = f;
            }
            _ => {
                let mut k = k;
                if s[k] <= 0.0 {
                    s[k] = if s[k] < 0.0 { -s[k] } else { 0.0 };
                    for i in 0..=pp {
                        v[i][k] = -v[i][k];
                    }
                }
                while k < pp {
                    if s[k] >= s[k + 1] {
                        break;
                    }
                    s.swap(k, k + 1);
                    if k < n - 1 {
                        for row in v.iter_mut() {
                            row.swap(k, k + 1);
                        }
                    }
                    if k < m - 1 {
                        for row in u.iter_mut() {
                            row.swap(k, k + 1);
                        }
                    }
                    k += 1;
                }
                p -= 1;
            }
        }
    }
    Svd { u, v, s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * (1.0 + b.abs())
    }

    #[test]
    fn svd_reconstructs_the_matrix() {
        let a = vec![
            vec![4.0, 1.0, -2.0],
            vec![1.0, 3.0, 0.5],
            vec![-2.0, 0.5, 5.0],
        ];
        let r = svd(a.clone());
        // Singular values sorted descending and non-negative.
        assert!(r.s.windows(2).all(|w| w[0] >= w[1]) && r.s.iter().all(|&x| x >= 0.0));
        let sig: Vec<Vec<f64>> = (0..3)
            .map(|i| (0..3).map(|j| if i == j { r.s[i] } else { 0.0 }).collect())
            .collect();
        let back = mult_mat(&mult_mat(&r.u, &sig), &transpose(&r.v));
        for i in 0..3 {
            for j in 0..3 {
                assert!(close(back[i][j], a[i][j]), "{back:?}");
            }
        }
    }

    #[test]
    fn hypot_matches_layout_base() {
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(0.0, 0.0), 0.0);
        assert_eq!(hypot(-3.0, 0.0), 3.0);
    }

    #[test]
    fn mult_gamma_centres() {
        assert_eq!(mult_gamma(&[1.0, 2.0, 3.0]), vec![-1.0, 0.0, 1.0]);
    }
}
