//! `gitGraph` diagrams.
//!
//! Supported statements: `commit` (`id:`, `msg:`, `tag:` (repeatable),
//! `type: NORMAL|REVERSE|HIGHLIGHT`), `branch name [order: n]`,
//! `checkout name` / `switch name`, `merge name` (`id:`, `tag:`, `type:`),
//! `cherry-pick id: "x"` (`parent:`, `tag:`), and the orientation header
//! `gitGraph LR:` / `TB:` / `BT:`.
//! Config (`gitGraph` section): `showBranches`, `showCommitLabel`,
//! `mainBranchName`, `mainBranchOrder`, `parallelCommits`, `rotateCommitLabel`.
//!
//! **Commit ids.** mermaid gives commits without an `id:` a random id
//! `<seq>-<7 random hex>`. To keep rendering pure and reproducible we use
//! `<seq>-<7 hex digits of FNV-1a(seq, branch, message)>` instead: same shape,
//! stable across runs.
//!
//! Layout and drawing port `gitGraphRenderer.ts`: commits 50px apart (or one
//! step past their furthest parent with `parallelCommits`, BT included), both
//! `drawCommits` passes (the arrows use the first pass's positions), the
//! commit map's handling of a repeated id, `findLane` for rerouted arrows, and
//! the viewBox as the drawing's `getBBox()` (text measured as mermaid measures
//! it while drawing) plus `diagramPadding`. With `rotateCommitLabel` (the
//! default) commit labels and their backgrounds are turned -45° and every lane
//! gets 40px extra room; TB/BT tags are always turned +45°.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{line_width, svg_bbox_width, svg_line_box};
use crate::theme::Palette;

/// Graph orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    LR,
    TB,
    BT,
}

/// Commit kinds (`MERGE`/`CHERRY_PICK` are created by those statements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitType {
    Normal,
    Reverse,
    Highlight,
    Merge,
    CherryPick,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub id: String,
    pub message: String,
    pub seq: usize,
    pub kind: CommitType,
    /// Visual override (`merge ... type: REVERSE`).
    pub custom_type: Option<CommitType>,
    /// The merge commit id was user-supplied (merge labels are shown only then).
    pub custom_id: bool,
    pub tags: Vec<String>,
    /// Indices into `GitGraph::commits`; first parent first.
    pub parents: Vec<usize>,
    pub branch: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub name: String,
    pub order: Option<f64>,
}

/// Parsed git graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GitGraph {
    pub title: Option<String>,
    pub orientation: Orientation,
    pub commits: Vec<Commit>,
    /// Branches in creation order (index 0 = main).
    pub branches: Vec<Branch>,
}

impl GitGraph {
    /// Branch indices sorted by display order (mermaid's `getBranchesAsObjArray`).
    pub fn lane_order(&self) -> Vec<usize> {
        let mut v: Vec<(f64, usize)> = self
            .branches
            .iter()
            .enumerate()
            .map(|(i, b)| (b.order.unwrap_or_else(|| default_order(i)), i))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, i)| i).collect()
    }
}

/// mermaid uses `parseFloat("0." + i)` for branches without an explicit order.
fn default_order(i: usize) -> f64 {
    format!("0.{i}").parse().unwrap_or(0.0)
}

/// Options that affect parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseOptions {
    pub main_branch_name: String,
    pub main_branch_order: f64,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            main_branch_name: "main".into(),
            main_branch_order: 0.0,
        }
    }
}

fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0xff)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn auto_id(seq: usize, branch: &str, msg: &str) -> String {
    let h = fnv(&[&seq.to_string(), branch, msg]);
    format!("{seq}-{:07x}", h & 0xfff_ffff)
}

/// Key/value attributes of a statement (`id: "x" tag: "v1" type: HIGHLIGHT`).
#[derive(Default)]
struct Attrs {
    id: Option<String>,
    msg: Option<String>,
    tags: Vec<String>,
    kind: Option<String>,
    order: Option<String>,
    parent: Option<String>,
}

fn parse_attrs(s: &str, line_no: usize, stmt: &str) -> Result<Attrs> {
    let mut a = Attrs::default();
    let mut rest = s.trim();
    let err = |m: String| Error::parse(line_no, format!("{stmt}\n{m}"));
    while !rest.is_empty() {
        // Bare quoted string = message (legacy `commit "msg"`).
        if rest.starts_with('"') {
            let (v, r) = take_value(rest).ok_or_else(|| err("Unterminated string".into()))?;
            a.msg = Some(v);
            rest = r.trim_start();
            continue;
        }
        let Some(colon) = rest.find(':') else {
            return Err(err(format!("Unexpected '{rest}'")));
        };
        let key = rest[..colon].trim();
        let (v, r) = take_value(rest[colon + 1..].trim_start())
            .ok_or_else(|| err(format!("Expecting a value for '{key}'")))?;
        match key {
            "id" => a.id = Some(v),
            "msg" => a.msg = Some(v),
            "tag" => a.tags.push(v),
            "type" => a.kind = Some(v),
            "order" => a.order = Some(v),
            "parent" => a.parent = Some(v),
            _ => return Err(err(format!("Unknown attribute '{key}'"))),
        }
        rest = r.trim_start();
    }
    Ok(a)
}

/// A quoted string or a bare word; returns (value, remainder).
fn take_value(s: &str) -> Option<(String, &str)> {
    if let Some(inner) = s.strip_prefix('"') {
        let end = inner.find('"')?;
        return Some((inner[..end].to_string(), &inner[end + 1..]));
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].to_string(), &s[end..]))
}

fn parse_type(v: &str, line_no: usize, stmt: &str) -> Result<CommitType> {
    match v {
        "NORMAL" => Ok(CommitType::Normal),
        "REVERSE" => Ok(CommitType::Reverse),
        "HIGHLIGHT" => Ok(CommitType::Highlight),
        _ => Err(Error::parse(
            line_no,
            format!("{stmt}\nExpecting 'NORMAL', 'REVERSE' or 'HIGHLIGHT', got '{v}'"),
        )),
    }
}

/// Split `name rest` where name may be quoted.
fn split_name(s: &str) -> Option<(String, &str)> {
    take_value(s.trim())
}

/// Parse a gitGraph body.
pub fn parse(body: &str, title: Option<String>, opts: &ParseOptions) -> Result<GitGraph> {
    let mut g = GitGraph {
        title,
        orientation: Orientation::LR,
        commits: Vec::new(),
        branches: vec![Branch {
            name: opts.main_branch_name.clone(),
            order: Some(opts.main_branch_order),
        }],
    };
    // Head commit per branch.
    let mut heads: Vec<Option<usize>> = vec![None];
    let mut cur = 0usize;
    let mut header = false;
    for (line_no, stmt) in super::statements(body) {
        let s = stmt.as_str();
        if !header {
            let rest = s
                .strip_prefix("gitGraph")
                .ok_or_else(|| Error::parse(line_no, "Expecting 'gitGraph'"))?
                .trim();
            g.orientation = match rest.trim_end_matches(':').trim() {
                "" | "LR" => Orientation::LR,
                "TB" => Orientation::TB,
                "BT" => Orientation::BT,
                o => {
                    return Err(Error::parse(
                        line_no,
                        format!("{s}\nUnknown orientation '{o}', expecting LR, TB or BT"),
                    ));
                }
            };
            header = true;
            continue;
        }
        let (word, rest) = match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], s[i..].trim()),
            None => (s, ""),
        };
        let seq = g.commits.len();
        match word {
            "title" => g.title = Some(rest.to_string()),
            "commit" => {
                let a = parse_attrs(rest, line_no, s)?;
                let kind = match &a.kind {
                    Some(t) => parse_type(t, line_no, s)?,
                    None => CommitType::Normal,
                };
                let msg = a.msg.unwrap_or_default();
                let id =
                    a.id.unwrap_or_else(|| auto_id(seq, &g.branches[cur].name, &msg));
                g.commits.push(Commit {
                    id,
                    message: msg,
                    seq,
                    kind,
                    custom_type: None,
                    custom_id: false,
                    tags: a.tags,
                    parents: heads[cur].into_iter().collect(),
                    branch: cur,
                });
                heads[cur] = Some(seq);
            }
            "branch" => {
                let (name, r) = split_name(rest).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting a branch name"))
                })?;
                let a = parse_attrs(r, line_no, s)?;
                if g.branches.iter().any(|b| b.name == name) {
                    return Err(Error::parse(
                        line_no,
                        format!(
                            "Trying to create an existing branch. (Help: Either use a new name if you want create a new branch or try using \"checkout {name}\")"
                        ),
                    ));
                }
                let order = match a.order {
                    Some(o) => Some(o.parse::<f64>().map_err(|_| {
                        Error::parse(line_no, format!("{s}\nExpecting a number for 'order'"))
                    })?),
                    None => None,
                };
                g.branches.push(Branch { name, order });
                heads.push(heads[cur]);
                cur = g.branches.len() - 1;
            }
            "checkout" | "switch" => {
                let (name, _) = split_name(rest).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting a branch name"))
                })?;
                cur = g.branches.iter().position(|b| b.name == name).ok_or_else(|| {
                    Error::parse(
                        line_no,
                        format!(
                            "Trying to checkout branch which is not yet created. (Help try using \"branch {name}\")"
                        ),
                    )
                })?;
            }
            "merge" => {
                let (name, r) = split_name(rest).ok_or_else(|| {
                    Error::parse(line_no, format!("{s}\nExpecting a branch name"))
                })?;
                let a = parse_attrs(r, line_no, s)?;
                let fail = |m: String| Err(Error::parse(line_no, m));
                let Some(other) = g.branches.iter().position(|b| b.name == name) else {
                    return fail(format!(
                        "Incorrect usage of \"merge\". Branch to be merged ({name}) does not exist"
                    ));
                };
                if other == cur {
                    return fail(
                        "Incorrect usage of \"merge\". Cannot merge a branch to itself".into(),
                    );
                }
                let Some(head) = heads[cur] else {
                    return fail(format!(
                        "Incorrect usage of \"merge\". Current branch ({})has no commits",
                        g.branches[cur].name
                    ));
                };
                let Some(other_head) = heads[other] else {
                    return fail(format!(
                        "Incorrect usage of \"merge\". Branch to be merged ({name}) has no commits"
                    ));
                };
                if g.commits[head].branch == other {
                    return fail(format!("Cannot merge branch '{name}' into itself."));
                }
                if head == other_head {
                    return fail(
                        "Incorrect usage of \"merge\". Both branches have same head".into(),
                    );
                }
                if let Some(id) = &a.id {
                    if g.commits.iter().any(|c| &c.id == id) {
                        return fail(format!(
                            "Incorrect usage of \"merge\". Commit with id:{id} already exists, use different custom id"
                        ));
                    }
                }
                let custom_type = match &a.kind {
                    Some(t) => Some(parse_type(t, line_no, s)?),
                    None => None,
                };
                let msg = format!("merged branch {name} into {}", g.branches[cur].name);
                g.commits.push(Commit {
                    id: a
                        .id
                        .clone()
                        .unwrap_or_else(|| auto_id(seq, &g.branches[cur].name, &msg)),
                    message: msg,
                    seq,
                    kind: CommitType::Merge,
                    custom_type,
                    custom_id: a.id.is_some(),
                    tags: a.tags,
                    parents: vec![head, other_head],
                    branch: cur,
                });
                heads[cur] = Some(seq);
            }
            "cherry-pick" => {
                let a = parse_attrs(rest, line_no, s)?;
                let fail = |m: &str| Err(Error::parse(line_no, m.to_string()));
                let Some(src) =
                    a.id.as_ref()
                        .and_then(|id| g.commits.iter().position(|c| &c.id == id))
                else {
                    return fail(
                        "Incorrect usage of \"cherryPick\". Source commit id should exist and provided",
                    );
                };
                let parent_idx = match &a.parent {
                    Some(p) => {
                        let ok = g.commits[src]
                            .parents
                            .iter()
                            .any(|&pi| g.commits[pi].id == *p);
                        if !ok {
                            return fail(
                                "Invalid operation: The specified parent commit is not an immediate parent of the cherry-picked commit.",
                            );
                        }
                        Some(p.clone())
                    }
                    None => None,
                };
                if g.commits[src].kind == CommitType::Merge && parent_idx.is_none() {
                    return fail(
                        "Incorrect usage of cherry-pick: If the source commit is a merge commit, an immediate parent commit must be specified.",
                    );
                }
                if g.commits[src].branch == cur {
                    return fail(
                        "Incorrect usage of \"cherryPick\". Source commit is already on current branch",
                    );
                }
                let Some(head) = heads[cur] else {
                    return Err(Error::parse(
                        line_no,
                        format!(
                            "Incorrect usage of \"cherry-pick\". Current branch ({})has no commits",
                            g.branches[cur].name
                        ),
                    ));
                };
                let src_id = g.commits[src].id.clone();
                let tags = if a.tags.is_empty() {
                    vec![match &parent_idx {
                        Some(p) if g.commits[src].kind == CommitType::Merge => {
                            format!("cherry-pick:{src_id}|parent:{p}")
                        }
                        _ => format!("cherry-pick:{src_id}"),
                    }]
                } else {
                    a.tags.into_iter().filter(|t| !t.is_empty()).collect()
                };
                let msg = format!(
                    "cherry-picked {} into {}",
                    g.commits[src].message, g.branches[cur].name
                );
                g.commits.push(Commit {
                    id: auto_id(seq, &g.branches[cur].name, &msg),
                    message: msg,
                    seq,
                    kind: CommitType::CherryPick,
                    custom_type: None,
                    custom_id: false,
                    tags,
                    parents: vec![head, src],
                    branch: cur,
                });
                heads[cur] = Some(seq);
            }
            _ => {
                return Err(Error::parse(
                    line_no,
                    format!(
                        "{s}\nExpecting 'commit', 'branch', 'checkout', 'switch', 'merge' or 'cherry-pick'"
                    ),
                ));
            }
        }
    }
    if !header {
        return Err(Error::parse(1, "Expecting 'gitGraph'"));
    }
    Ok(g)
}

/// Rendering options.
#[derive(Debug, Clone, PartialEq)]
pub struct GitConfig {
    pub show_branches: bool,
    pub show_commit_label: bool,
    pub parallel_commits: bool,
    /// `rotateCommitLabel`: commit labels turned -45° (mermaid default).
    pub rotate_commit_label: bool,
    pub title_top_margin: f64,
    pub diagram_padding: f64,
}

impl GitConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        GitConfig {
            show_branches: cfg.section_bool("gitGraph", "showBranches", true),
            show_commit_label: cfg.section_bool("gitGraph", "showCommitLabel", true),
            parallel_commits: cfg.section_bool("gitGraph", "parallelCommits", false),
            rotate_commit_label: cfg.section_bool("gitGraph", "rotateCommitLabel", true),
            title_top_margin: cfg.section_f64("gitGraph", "titleTopMargin", 25.0),
            diagram_padding: cfg.section_f64("gitGraph", "diagramPadding", 8.0),
        }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        GitConfig {
            show_branches: true,
            show_commit_label: true,
            parallel_commits: false,
            rotate_commit_label: true,
            title_top_margin: 25.0,
            diagram_padding: 8.0,
        }
    }
}

/// `.commit-label` / `.tag-label` font size (`commitLabelFontSize`, `tagLabelFontSize`).
const LABEL_FONT: f64 = 10.0;
const TAG_FONT: f64 = 10.0;
/// Branch labels are SVG text in the root font (no stylesheet rule sizes them).
const BRANCH_FONT: f64 = 16.0;
/// `gitGraphRenderer.ts` constants.
const LAYOUT_OFFSET: f64 = 10.0;
const COMMIT_STEP: f64 = 40.0;
const DEFAULT_POS: f64 = 30.0;
const PX: f64 = 4.0;
const PY: f64 = 2.0;

fn shown_label(c: &Commit, gc: &GitConfig) -> bool {
    gc.show_commit_label
        && c.kind != CommitType::CherryPick
        && (c.kind != CommitType::Merge || c.custom_id)
}

/// mermaid's commit `Map`, keyed by id: a commit that reuses an id replaces the
/// earlier one's entry (which keeps its place in the key order), and parents
/// name ids, so they resolve to whichever commit holds the id last.
struct Commits<'a> {
    g: &'a GitGraph,
    /// The commit stored under each key, keys in insertion order.
    keys: Vec<usize>,
    /// Commit index → the commit its id resolves to.
    resolve: Vec<usize>,
}

impl<'a> Commits<'a> {
    fn new(g: &'a GitGraph) -> Self {
        let mut keys: Vec<usize> = Vec::new();
        let mut slot: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, c) in g.commits.iter().enumerate() {
            match slot.get(c.id.as_str()) {
                Some(&k) => keys[k] = i,
                None => {
                    slot.insert(&c.id, keys.len());
                    keys.push(i);
                }
            }
        }
        let resolve = g
            .commits
            .iter()
            .map(|c| keys[slot[c.id.as_str()]])
            .collect();
        Commits { g, keys, resolve }
    }

    fn parents(&self, c: usize) -> Vec<usize> {
        self.g.commits[c]
            .parents
            .iter()
            .map(|&p| self.resolve[p])
            .collect()
    }
}

/// `drawText(name).getBBox()` of a branch label as mermaid measures it while
/// drawing: rows (split at `\n` / `<br>`) as `<tspan dy="1em" x="0">`, each
/// as wide as its ink. Returns (width, height, top below the text's origin).
fn branch_label_box(name: &str) -> (f64, f64, f64, Vec<String>) {
    let rows: Vec<String> = split_rows(name);
    let ascent = (0.905 * BRANCH_FONT).round();
    let (mut w, mut top, mut bottom) = (0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    for (i, r) in rows.iter().enumerate() {
        let baseline = BRANCH_FONT * (i + 1) as f64;
        w = w.max(svg_bbox_width(r, BRANCH_FONT));
        top = top.min(baseline - ascent);
        bottom = bottom.max(baseline - ascent + svg_line_box(BRANCH_FONT, r));
    }
    if rows.is_empty() {
        return (0.0, 0.0, 0.0, rows);
    }
    (w, bottom - top, top, rows)
}

/// `txt.split(/\\n|\n|<br\s*\/?>/gi)`, each row trimmed.
fn split_rows(s: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("\\n").or_else(|| rest.strip_prefix('\n')) {
            rows.push(std::mem::take(&mut cur));
            rest = r;
            continue;
        }
        let lower = rest.get(..3).map(str::to_ascii_lowercase);
        if lower.as_deref() == Some("<br") {
            let after = rest[3..].trim_start();
            let after = after.strip_prefix('/').unwrap_or(after);
            if let Some(r) = after.strip_prefix('>') {
                rows.push(std::mem::take(&mut cur));
                rest = r;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        cur.push(c);
        rest = &rest[c.len_utf8()..];
    }
    rows.push(cur);
    rows.into_iter().map(|r| r.trim().to_string()).collect()
}

/// Where a commit is drawn (`getCommitPosition`), and the walk's `pos` there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub commit: usize,
    pub x: f64,
    pub y: f64,
    pub pos_with_offset: f64,
    pub pos: f64,
}

/// Lanes and commit positions, as `gitGraphRenderer.ts` computes them.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// `branchPos.pos` per branch index.
    pub lane: Vec<f64>,
    /// Display index (colour) per branch index.
    pub lane_index: Vec<usize>,
    /// Commits in drawing order with their positions (second `drawCommits` pass).
    pub placed: Vec<Placed>,
    /// `commitPos` after the first pass, which the arrows are drawn from.
    pub arrow_at: Vec<Option<Point>>,
    /// `maxPos` when the branches are drawn.
    pub max_pos: f64,
}

/// The walk of `drawCommits` (positions only), updating `commitPos` and `maxPos`.
struct Walker<'a> {
    cm: &'a Commits<'a>,
    o: Orientation,
    parallel: bool,
    lane: &'a [f64],
    /// `REDUX_BRANCH_LABEL_PADDING_Y / 2 + 1` under redux geometry, else -2.
    spine_dy: f64,
    at: Vec<Option<Point>>,
    max_pos: f64,
}

impl Walker<'_> {
    fn vertical(&self) -> bool {
        self.o != Orientation::LR
    }

    /// `findClosestParent`: the parent furthest along (BT: the lowest `y`).
    fn closest_parent(&self, parents: &[usize]) -> Option<usize> {
        let bt = self.o == Orientation::BT;
        let mut target = if bt { f64::INFINITY } else { 0.0 };
        let mut closest = None;
        for &p in parents {
            let Some(at) = self.at[p] else { continue };
            let v = if self.vertical() { at.y } else { at.x };
            if (bt && v <= target) || (!bt && v >= target) {
                closest = Some(p);
                target = v;
            }
        }
        closest
    }

    /// `calculatePosition`.
    fn calculate_position(&self, k: usize) -> f64 {
        let parents = self.cm.parents(k);
        let zero = Point::new(0.0, 0.0);
        let current = || self.at[k].unwrap_or(zero);
        if !parents.is_empty() {
            if let Some(cp) = self.closest_parent(&parents) {
                let pp = self.at[cp].unwrap_or(zero);
                return match self.o {
                    Orientation::TB => pp.y + COMMIT_STEP,
                    Orientation::BT => current().y - COMMIT_STEP,
                    Orientation::LR => pp.x + COMMIT_STEP,
                };
            }
            return 0.0;
        }
        match self.o {
            Orientation::TB => DEFAULT_POS,
            Orientation::BT => current().y - COMMIT_STEP,
            Orientation::LR => 0.0,
        }
    }

    /// `setParallelBTPos`.
    fn parallel_bt(&mut self, sorted: &[usize], default_pos: f64) {
        let g = self.cm.g;
        let lane = |k: usize| self.lane[g.commits[k].branch];
        let (mut cur, mut max_position) = (default_pos, default_pos);
        let mut roots = Vec::new();
        for &k in sorted {
            let parents = self.cm.parents(k);
            if !parents.is_empty() {
                let cp = self.closest_parent(&parents);
                let y = cp.and_then(|p| self.at[p]).map_or(0.0, |p| p.y);
                cur = y + COMMIT_STEP;
                max_position = cur.max(max_position);
            } else {
                roots.push(k);
            }
            self.at[k] = Some(Point::new(lane(k), cur + LAYOUT_OFFSET));
        }
        cur = max_position;
        for &k in &roots {
            self.at[k] = Some(Point::new(lane(k), cur + default_pos));
        }
        for &k in sorted {
            let parents = self.cm.parents(k);
            if parents.is_empty() {
                continue;
            }
            // `findClosestParentBT`: the parent highest up.
            let mut best = f64::INFINITY;
            let mut closest = None;
            for &p in &parents {
                if let Some(at) = self.at[p]
                    && at.y <= best
                {
                    closest = Some(p);
                    best = at.y;
                }
            }
            cur = closest.and_then(|p| self.at[p]).map_or(0.0, |p| p.y) - COMMIT_STEP;
            if cur <= max_position {
                max_position = cur;
            }
            self.at[k] = Some(Point::new(lane(k), cur - LAYOUT_OFFSET));
        }
    }

    /// One `drawCommits` pass.
    fn walk(&mut self) -> Vec<Placed> {
        let g = self.cm.g;
        let bt = self.o == Orientation::BT;
        let vertical = self.vertical();
        let mut pos = if vertical { DEFAULT_POS } else { 0.0 };
        let mut sorted = self.cm.keys.clone();
        sorted.sort_by_key(|&k| g.commits[k].seq);
        if bt {
            if self.parallel {
                self.parallel_bt(&sorted, pos);
            }
            sorted.reverse();
        }
        let mut placed = Vec::with_capacity(sorted.len());
        for &k in &sorted {
            if self.parallel {
                pos = self.calculate_position(k);
            }
            let wo = if bt && self.parallel {
                pos
            } else {
                pos + LAYOUT_OFFSET
            };
            let branch = self.lane[g.commits[k].branch];
            let (x, y) = if vertical {
                (branch, wo)
            } else {
                (wo, branch + self.spine_dy)
            };
            self.at[k] = Some(if vertical {
                Point::new(x, wo)
            } else {
                Point::new(wo, y)
            });
            placed.push(Placed {
                commit: k,
                x,
                y,
                pos_with_offset: wo,
                pos,
            });
            pos = if bt && self.parallel {
                pos + COMMIT_STEP
            } else {
                pos + COMMIT_STEP + LAYOUT_OFFSET
            };
            if pos > self.max_pos {
                self.max_pos = pos;
            }
        }
        placed
    }
}

/// Compute lanes and commit positions (`setBranchPosition`, `drawCommits`).
/// `redux`: the redux themes' geometry (spines 7px below the branch position).
pub fn layout(g: &GitGraph, gc: &GitConfig, redux: bool) -> Layout {
    let vertical = g.orientation != Orientation::LR;
    let order = g.lane_order();
    let mut lane = vec![0.0; g.branches.len()];
    let mut lane_index = vec![0; g.branches.len()];
    let mut p = 0.0;
    for (idx, &b) in order.iter().enumerate() {
        lane[b] = p;
        lane_index[b] = idx;
        let half = if vertical {
            branch_label_box(&g.branches[b].name).0 / 2.0
        } else {
            0.0
        };
        p += 50.0 + if gc.rotate_commit_label { 40.0 } else { 0.0 } + half;
    }
    let cm = Commits::new(g);
    let mut w = Walker {
        cm: &cm,
        o: g.orientation,
        parallel: gc.parallel_commits,
        lane: &lane,
        spine_dy: if redux { 12.0 / 2.0 + 1.0 } else { -2.0 },
        at: vec![None; g.commits.len()],
        max_pos: 0.0,
    };
    // `drawCommits(.., false)` before the branches and arrows, then again to draw.
    w.walk();
    let (arrow_at, max_pos) = (w.at.clone(), w.max_pos);
    let placed = w.walk();
    Layout {
        lane,
        lane_index,
        placed,
        arrow_at,
        max_pos,
    }
}

/// Which branch of upstream's `styles.js` styles the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `normalTheme`: `git{i}` / `gitInv{i}` / `gitBranchLabel{i}`.
    Classic,
    /// `neo`, `neo-dark`: lane 0 in `nodeBorder`, the rest as `normalTheme`.
    Neo,
    /// `redux`, `redux-dark`: everything in `nodeBorder`.
    Redux,
    /// `redux-color`, `redux-dark-color`: lane 0 in `nodeBorder`, the rest
    /// `borderColorArray`.
    ReduxColor,
}

/// The theme-dependent colours and sizes of upstream's `styles.js` (`genColor` /
/// `normalTheme`) and `gitGraphRenderer.ts` (`REDUX_GEOMETRY_THEMES`, `DARK_THEMES`).
///
/// Class index `k` is the lane index reduced as upstream's `calcColorIndex` does.
/// Elements whose class sets no fill inherit the root `fill: textColor`.
struct GitStyle<'a> {
    pal: &'a Palette,
    family: Family,
    /// `REDUX_GEOMETRY_THEMES`: smaller bullets, thin arrows, bold branch labels.
    redux: bool,
    /// `DARK_THEMES`: the cherry-pick marks are drawn black.
    dark: bool,
}

impl<'a> GitStyle<'a> {
    fn new(pal: &'a Palette) -> Self {
        use crate::theme::ThemeName as T;
        let family = match pal.theme {
            T::Neo | T::NeoDark => Family::Neo,
            T::Redux | T::ReduxDark => Family::Redux,
            T::ReduxColor | T::ReduxDarkColor => Family::ReduxColor,
            _ => Family::Classic,
        };
        GitStyle {
            pal,
            family,
            redux: matches!(family, Family::Redux | Family::ReduxColor),
            dark: matches!(
                pal.theme,
                T::Dark | T::ReduxDark | T::ReduxDarkColor | T::NeoDark
            ),
        }
    }

    /// `calcColorIndex(lane, limit, useColorTheme)`.
    fn index(&self, lane: usize, limit: usize) -> usize {
        if self.family == Family::ReduxColor && lane > 0 {
            (lane - 1) % (limit - 1) + 1
        } else {
            lane % limit
        }
    }

    /// Class index of commits and arrows (`THEME_COLOR_LIMIT` = 8).
    fn commit_index(&self, lane: usize) -> usize {
        self.index(lane, 8)
    }

    /// Class index of branch labels (the theme's 12 under redux geometry).
    fn branch_index(&self, lane: usize) -> usize {
        self.index(lane, if self.redux { 12 } else { 8 })
    }

    fn git(&self, k: usize) -> Color {
        self.pal.git[k % 8].clone()
    }

    fn border(&self, k: usize) -> Color {
        match &self.pal.color_slots {
            Some(slots) => slots.border[k % slots.border.len()].clone(),
            None => self.pal.node_border.clone(),
        }
    }

    /// The lane colour: `.commit{k}` stroke and `.arrow{k}`.
    fn lane(&self, k: usize) -> Color {
        match self.family {
            Family::Classic => self.git(k),
            Family::Neo if k > 0 => self.git(k),
            Family::ReduxColor if k > 0 => self.border(k),
            _ => self.pal.node_border.clone(),
        }
    }

    /// `.commit{k}` fill, or the inherited one where the class sets none.
    fn commit_fill(&self, k: usize) -> Color {
        match self.family {
            Family::Classic => self.git(k),
            Family::Neo | Family::ReduxColor if k > 0 => self.lane(k),
            _ => self.inherited_fill(),
        }
    }

    /// The fill a bullet without its own inherits: `.commit-bullets { fill: nodeBorder }`
    /// under the neo colour generator, the root's `textColor` otherwise.
    fn inherited_fill(&self) -> Color {
        if self.neo_gen() {
            self.pal.node_border.clone()
        } else {
            self.pal.text_color.clone()
        }
    }

    /// `.branch` stroke: `commitLineColor ?? lineColor` (the redux themes set
    /// `commitLineColor = #BDBCCC`).
    fn branch_line(&self) -> Color {
        if self.redux {
            Color::new("#BDBCCC")
        } else {
            self.pal.line_color.clone()
        }
    }

    /// `.commit-highlight{k}` (fill, stroke).
    fn highlight(&self, k: usize) -> (Color, Color) {
        let inv = || self.pal.git_inv[k % 8].clone();
        match self.family {
            Family::Classic => (inv(), inv()),
            Family::Neo if k > 0 => (inv(), inv()),
            Family::ReduxColor if k > 0 => (self.border(k), self.border(k)),
            Family::ReduxColor => (self.pal.main_bkg.clone(), self.pal.node_border.clone()),
            _ => (self.pal.node_border.clone(), self.pal.node_border.clone()),
        }
    }

    fn stroke_width(&self) -> f64 {
        self.pal.stroke_width.unwrap_or(1.0)
    }

    /// `.arrow`: the theme's `strokeWidth` under redux geometry, else 8.
    fn arrow_width(&self) -> f64 {
        if self.redux { self.stroke_width() } else { 8.0 }
    }

    /// `.label{k}` of the branch-label box: fill, and stroke with its width.
    fn label_box(&self, k: usize) -> (Color, Option<(Color, f64)>) {
        let sw = self.stroke_width();
        let main = self.pal.main_bkg.clone();
        match self.family {
            Family::Classic => (self.git(k), None),
            // `genGitGraphGradient`: `url(#gradient)` from `gradientStart` (#0042eb) to
            // `gradientStop` (#eb0042); drawn in the start colour.
            Family::Neo => (main, Some((Color::new("#0042eb"), sw))),
            Family::ReduxColor if k > 0 => {
                let b = self.border(k);
                (if self.dark { main } else { b.clone() }, Some((b, sw)))
            }
            _ => (main, Some((self.pal.node_border.clone(), sw))),
        }
    }

    /// `.branch-label{k}` text colour.
    fn branch_label(&self, k: usize) -> Color {
        match self.family {
            Family::Classic => self.pal.git_branch_label[k % 8].clone(),
            Family::Neo if k > 0 => self.pal.git_branch_label[k % 8].clone(),
            _ => self.pal.node_border.clone(),
        }
    }

    /// Left-to-right spines (and their commits) sit off the branch position:
    /// 2px up, or under redux geometry half the 12px label padding plus one down.
    fn spine_dy(&self) -> f64 {
        if self.redux { 12.0 / 2.0 + 1.0 } else { -2.0 }
    }

    /// `NEO_COLOR_GEN_THEMES`.
    fn neo_gen(&self) -> bool {
        self.family != Family::Classic
    }
}

/// An orthogonal polyline whose inner corners are quarter circles of radius `r`
/// (mermaid's `L .. A r r ..` arrows): each arc starts `r` before its corner and
/// ends `r` after it.
fn rounded(points: &[Point], r: f64) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(points[0])];
    for i in 1..points.len() {
        let p = points[i];
        if i + 1 < points.len() && r > 0.0 {
            let din = p.minus(points[i - 1]).norm();
            let dout = points[i + 1].minus(p).norm();
            let a = p.minus(din.scale(r));
            let b = p.plus(dout.scale(r));
            let k = 0.552_284_75;
            cmds.push(PathCmd::LineTo(a));
            cmds.push(PathCmd::CubicTo(
                a.plus(din.scale(r * k)),
                b.minus(dout.scale(r * k)),
                b,
            ));
        } else {
            cmds.push(PathCmd::LineTo(p));
        }
    }
    cmds
}

/// The `getBBox()` of one-line SVG text (start-anchored at `x`, baseline `y`),
/// as measured while mermaid draws: the run's ink, the font's rounded ascent
/// above the baseline.
fn text_box(x: f64, y: f64, s: &str, font: f64) -> Rect {
    Rect::new(
        x,
        y - (0.905 * font).round(),
        svg_bbox_width(s, font),
        svg_line_box(font, s),
    )
}

/// Render a gitGraph.
pub fn render(
    body: &str,
    title: Option<String>,
    cfg: &MermaidConfig,
    pal: &Palette,
) -> Result<Scene> {
    let opts = ParseOptions {
        main_branch_name: cfg
            .section("gitGraph", "mainBranchName")
            .and_then(|j| j.as_str())
            .unwrap_or("main")
            .to_string(),
        main_branch_order: cfg.section_f64("gitGraph", "mainBranchOrder", 0.0),
    };
    let g = parse(body, title, &opts)?;
    let gc = GitConfig::from_config(cfg);
    let lay = layout(&g, &gc, GitStyle::new(pal).redux);
    Ok(draw(&g, &lay, &gc, pal))
}

/// mermaid's `findLane`: a free coordinate between two lanes for rerouted
/// arrows (taken lanes are kept 10px apart; after five halvings the candidate
/// is used anyway).
fn find_lane(lanes: &mut Vec<f64>, y1: f64, y2: f64, depth: usize) -> f64 {
    let candidate = y1 + (y1 - y2).abs() / 2.0;
    if depth > 5 {
        return candidate;
    }
    if lanes.iter().all(|l| (l - candidate).abs() >= 10.0) {
        lanes.push(candidate);
        return candidate;
    }
    let diff = (y1 - y2).abs();
    find_lane(lanes, y1, y2 - diff / 5.0, depth + 1)
}

/// Draw a laid-out git graph (`gitGraphRenderer.ts` `draw`).
pub fn draw(g: &GitGraph, lay: &Layout, gc: &GitConfig, pal: &Palette) -> Scene {
    let o = g.orientation;
    let vertical = o != Orientation::LR;
    let cm = Commits::new(g);
    let mut bounds = Bounds::default();
    let st = GitStyle::new(pal);
    let ck = |b: usize| st.commit_index(lay.lane_index[b]);
    let max_pos = lay.max_pos;
    let mut lanes: Vec<f64> = Vec::new();

    // drawBranches.
    let mut branch_els = Vec::new();
    if gc.show_branches {
        for &b in &g.lane_order() {
            let pos = lay.lane[b];
            let k = st.branch_index(lay.lane_index[b]);
            let spine = if vertical { pos } else { pos + st.spine_dy() };
            let (a, z) = match o {
                Orientation::LR => (Point::new(0.0, spine), Point::new(max_pos, spine)),
                Orientation::TB => (Point::new(pos, DEFAULT_POS), Point::new(pos, max_pos)),
                Orientation::BT => (Point::new(pos, max_pos), Point::new(pos, DEFAULT_POS)),
            };
            bounds.add_point(a);
            bounds.add_point(z);
            branch_els.push(Element::classed_shape(
                Geometry::Path(vec![PathCmd::MoveTo(a), PathCmd::LineTo(z)]),
                Paint::stroke(&st.branch_line(), st.stroke_width()).dashed(if st.neo_gen() {
                    vec![4.0, 2.0]
                } else {
                    vec![2.0]
                }),
                format!("branch branch{k}"),
            ));
            lanes.push(spine);
            let (bw, bh, btop, rows) = branch_label_box(&g.branches[b].name);
            let (pad_x, pad_y) = if st.redux { (16.0, 12.0) } else { (0.0, 0.0) };
            let rot = if gc.rotate_commit_label { 30.0 } else { 0.0 };
            let (w, h) = (bw + 18.0 + pad_x, bh + 4.0 + pad_y);
            // The box (with its transform) and the label group's translation.
            let (rect, label_at) = match o {
                Orientation::LR => (
                    Rect::new(
                        -bw - 4.0 - rot - 19.0,
                        -bh / 2.0 + 10.0 + spine - 12.0 - pad_y / 2.0,
                        w,
                        h,
                    ),
                    Point::new(-bw - 14.0 - rot + pad_x / 2.0, spine - bh / 2.0 - 2.0),
                ),
                Orientation::TB if st.redux => (
                    Rect::new(
                        pos - bw / 2.0 - 10.0 - pad_x / 2.0 - 3.0,
                        -pad_y - 10.0,
                        w,
                        h,
                    ),
                    Point::new(pos - bw / 2.0 - 5.0, -pad_y * 2.0 + 7.0),
                ),
                Orientation::TB => (
                    Rect::new(pos - bw / 2.0 - 10.0, 0.0, w, h),
                    Point::new(pos - bw / 2.0 - 5.0, 0.0),
                ),
                Orientation::BT if st.redux => (
                    Rect::new(
                        pos - bw / 2.0 - 10.0 - pad_x / 2.0 - 3.0,
                        max_pos + pad_y + 10.0,
                        w,
                        h,
                    ),
                    Point::new(pos - bw / 2.0 - 5.0, max_pos + pad_y * 2.0 + 4.0),
                ),
                Orientation::BT => (
                    Rect::new(pos - bw / 2.0 - 10.0, max_pos, w, h),
                    Point::new(pos - bw / 2.0 - 5.0, max_pos),
                ),
            };
            bounds.add_rect(rect);
            let (fill, stroke) = st.label_box(k);
            let paint = match &stroke {
                Some((c, w)) => Paint::both(&fill, c, *w),
                None => Paint::fill(&fill),
            };
            branch_els.push(Element::classed_shape(
                Geometry::Rect {
                    rect,
                    rx: if st.redux { 0.0 } else { 4.0 },
                },
                paint,
                format!("branchLabelBkg label{k}"),
            ));
            // `<tspan dy="1em" x="0">` rows from the label group's origin.
            if !rows.is_empty() {
                bounds.add_rect(Rect::new(label_at.x, label_at.y + btop, bw, bh));
            }
            let text = Text {
                x: label_at.x,
                y: label_at.y + BRANCH_FONT,
                lines: rows,
                line_height: BRANCH_FONT,
                font_size: BRANCH_FONT,
                bold: false,
                anchor: Anchor::Start,
                fill: st.branch_label(k),
            };
            branch_els.push(Element::classed_text(
                if st.redux { text.bold() } else { text },
                format!("branch-label{k}"),
            ));
        }
    }

    // drawArrows: every stored commit (in key order) from each of its parents,
    // at the positions of the first `drawCommits` pass.
    let mut arrow_els = Vec::new();
    for &ci in &cm.keys {
        let c = &g.commits[ci];
        for pi in cm.parents(ci) {
            let a = &g.commits[pi];
            let (Some(p1), Some(p2)) = (lay.arrow_at[pi], lay.arrow_at[ci]) else {
                continue;
            };
            let first_parent = cm.parents(ci).first().copied();
            let merge_side = c.kind == CommitType::Merge && Some(pi) != first_parent;
            let mut color = if merge_side { a.branch } else { c.branch };
            // shouldRerouteArrow: a commit of the branch that would get the curve
            // lies between the two.
            let b_furthest = if vertical { p1.x < p2.x } else { p1.y < p2.y };
            let curve_branch = if b_furthest { c.branch } else { a.branch };
            let reroute = cm.keys.iter().any(|&x| {
                let xc = &g.commits[x];
                xc.seq > a.seq && xc.seq < c.seq && xc.branch == curve_branch
            });
            let (pts, r): (Vec<Point>, f64) = if reroute {
                let line_y = if p1.y < p2.y {
                    find_lane(&mut lanes, p1.y, p2.y, 0)
                } else {
                    find_lane(&mut lanes, p2.y, p1.y, 0)
                };
                let line_x = if p1.x < p2.x {
                    find_lane(&mut lanes, p1.x, p2.x, 0)
                } else {
                    find_lane(&mut lanes, p2.x, p1.x, 0)
                };
                let pts = if vertical {
                    if p1.x >= p2.x {
                        color = a.branch;
                    }
                    vec![p1, Point::new(line_x, p1.y), Point::new(line_x, p2.y), p2]
                } else {
                    if p1.y >= p2.y {
                        color = a.branch;
                    }
                    vec![p1, Point::new(p1.x, line_y), Point::new(p2.x, line_y), p2]
                };
                (pts, 10.0)
            } else {
                let same = if vertical { p1.x == p2.x } else { p1.y == p2.y };
                if same {
                    (vec![p1, p2], 0.0)
                } else {
                    // Merges turn at the merge commit's row, the rest at the parent's.
                    let turn = match (vertical, merge_side) {
                        (true, true) | (false, false) => Point::new(p1.x, p2.y),
                        (true, false) | (false, true) => Point::new(p2.x, p1.y),
                    };
                    (vec![p1, turn, p2], 20.0)
                }
            };
            for p in &pts {
                bounds.add_point(*p);
            }
            let k = ck(color);
            arrow_els.push(Element::classed_shape(
                Geometry::Path(rounded(&pts, r)),
                Paint::stroke(&st.lane(k), st.arrow_width()),
                format!("arrow arrow{k}"),
            ));
        }
    }

    // drawCommits: bullets, labels and tags.
    let mut bullet_els = Vec::new();
    let (tag_bkg, tag_border) = if st.neo_gen() {
        (pal.main_bkg.clone(), pal.node_border.clone())
    } else {
        (pal.tag_label_bkg.clone(), pal.tag_label_border.clone())
    };
    let mut label_els = Vec::new();
    // `.commit-merge` / `-reverse` / `-highlight-inner`: `mainBkg` under the neo colour
    // generator, `primaryColor` (the same colour in the classic themes) otherwise.
    let primary = pal.main_bkg.clone();
    // Bullet, merge-hole and cherry-pick dot radii, reverse-cross half size, highlight
    // outer / inner sizes and offsets (`useReduxGeometry`).
    let (bullet_r, hole_r, dot_r, cross, (hi_out, hi_in), (off_out, off_in)) = if st.redux {
        (7.0, 5.0, 2.5, 4.0, (14.0, 8.0), (3.0, 2.0))
    } else {
        (10.0, 6.0, 2.75, 5.0, (20.0, 12.0), (0.0, 0.0))
    };
    for pl in &lay.placed {
        let ci = pl.commit;
        let c = &g.commits[ci];
        let p = Point::new(pl.x, pl.y);
        let k = ck(c.branch);
        let (col, col_fill) = (st.lane(k), st.commit_fill(k));
        let cls = format!("commit {} commit{k}", c.id);
        let symbol = c.custom_type.unwrap_or(c.kind);
        match symbol {
            CommitType::Highlight => {
                // `.commit-highlight{k}`.
                let (hf, hs) = st.highlight(k);
                let outer = Rect::new(p.x - 10.0 + off_out, p.y - 10.0 + off_out, hi_out, hi_out);
                let inner = Rect::new(p.x - 6.0 + off_in, p.y - 6.0 + off_in, hi_in, hi_in);
                bounds.add_rect(outer);
                bounds.add_rect(inner);
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: outer,
                        rx: 0.0,
                    },
                    Paint::both(&hf, &hs, 1.0),
                    format!("{cls} commit-highlight-outer"),
                ));
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: inner,
                        rx: 0.0,
                    },
                    Paint::both(&primary, &primary, 1.0),
                    format!("{cls} commit-highlight-inner"),
                ));
            }
            CommitType::CherryPick => {
                // Classed `commit commit-cherry-pick` only (no `commit{k}`): no stroke,
                // and an inherited fill.
                bounds.add_rect(Rect::centered(p, 2.0 * bullet_r, 2.0 * bullet_r));
                bullet_els.push(Element::classed_shape(
                    Geometry::Circle { c: p, r: bullet_r },
                    Paint::fill(&st.inherited_fill()),
                    format!("commit {} commit-cherry-pick", c.id),
                ));
                let mark = Color::new(if st.dark { "#000000" } else { "#fff" });
                for dx in [-3.0, 3.0] {
                    bullet_els.push(Element::shape(
                        Geometry::Circle {
                            c: Point::new(p.x + dx, p.y + 2.0),
                            r: dot_r,
                        },
                        Paint::fill(&mark),
                    ));
                }
                for dx in [3.0, -3.0] {
                    bullet_els.push(Element::line(
                        Point::new(p.x + dx, p.y + 1.0),
                        Point::new(p.x, p.y - 5.0),
                        Paint::stroke(&mark, 1.0),
                    ));
                }
            }
            _ => {
                bounds.add_rect(Rect::centered(p, 2.0 * bullet_r, 2.0 * bullet_r));
                bullet_els.push(Element::classed_shape(
                    Geometry::Circle { c: p, r: bullet_r },
                    Paint::both(&col_fill, &col, 1.0),
                    cls.clone(),
                ));
                if symbol == CommitType::Merge {
                    bullet_els.push(Element::classed_shape(
                        Geometry::Circle { c: p, r: hole_r },
                        Paint::both(&primary, &primary, 1.0),
                        format!("commit commit-merge {} commit{k}", c.id),
                    ));
                }
                if symbol == CommitType::Reverse {
                    // (Filled too: the open cross encloses nothing.)
                    let mut paint =
                        Paint::stroke(&primary, if st.neo_gen() { st.stroke_width() } else { 3.0 });
                    paint.fill = primary.clone();
                    bullet_els.push(Element::classed_shape(
                        Geometry::Path(vec![
                            PathCmd::MoveTo(Point::new(p.x - cross, p.y - cross)),
                            PathCmd::LineTo(Point::new(p.x + cross, p.y + cross)),
                            PathCmd::MoveTo(Point::new(p.x - cross, p.y + cross)),
                            PathCmd::LineTo(Point::new(p.x + cross, p.y - cross)),
                        ]),
                        paint,
                        format!("commit commit-reverse {} commit{k}", c.id),
                    ));
                }
            }
        }
        if shown_label(c, gc) {
            label_els.extend(commit_label(c, pl, o, gc, &st, pal, &mut bounds));
        }
        label_els.extend(commit_tags(
            c,
            pl,
            o,
            pal,
            (&tag_bkg, &tag_border),
            &mut bounds,
        ));
    }

    let mut els = vec![
        Element::group("branches", branch_els),
        Element::group("commit-arrows", arrow_els),
        Element::group("commit-bullets", bullet_els),
        Element::group("commit-labels", label_els),
    ];
    if let Some(t) = g.title.as_ref().filter(|t| !t.is_empty()) {
        // utils `insertTitle`: centred on the drawing's bounding box, its
        // baseline at y = -titleTopMargin.
        let r = bounds.rect();
        let (x, y) = (r.x + r.w / 2.0, -gc.title_top_margin);
        let adv = crate::text::layout_unit_ceil(line_width(t, 18.0, false));
        bounds.add_rect(text_box(x - adv / 2.0, y, t, 18.0));
        els.push(Element::classed_text(
            Text {
                x,
                y,
                lines: vec![t.clone()],
                line_height: 22.0,
                font_size: 18.0,
                bold: false,
                anchor: Anchor::Middle,
                fill: pal.text_color.clone(),
            },
            "gitTitleText",
        ));
    }
    Scene {
        kind: DiagramKind::GitGraph,
        view_box: bounds.rect().pad(gc.diagram_padding),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// `drawCommitLabel`: the id on a half-transparent box below the commit (TB/BT:
/// left of it), sized by the text's `getBBox()`, and with `rotateCommitLabel`
/// turned -45°.
fn commit_label(
    c: &Commit,
    pl: &Placed,
    o: Orientation,
    gc: &GitConfig,
    st: &GitStyle,
    pal: &Palette,
    bounds: &mut Bounds,
) -> Vec<Element> {
    let id = c.id.as_str();
    let (w, h) = (svg_bbox_width(id, LABEL_FONT), svg_line_box(LABEL_FONT, id));
    let ascent = (0.905 * LABEL_FONT).round();
    let (rect, tx, ty) = if o == Orientation::LR {
        (
            Rect::new(
                pl.pos_with_offset - w / 2.0 - PY,
                pl.y + 13.5,
                w + 2.0 * PY,
                h + 2.0 * PY,
            ),
            pl.pos_with_offset - w / 2.0,
            pl.y + 25.0,
        )
    } else {
        (
            Rect::new(
                pl.x - (w + 4.0 * PX + 5.0),
                pl.y - 12.0,
                w + 2.0 * PY,
                h + 2.0 * PY,
            ),
            pl.x - (w + 4.0 * PX),
            pl.y + h - 12.0,
        )
    };
    // Neo colour generator: transparent box, `nodeBorder` text (weight 600
    // under redux geometry).
    let bkg = Element::classed_shape(
        Geometry::Rect { rect, rx: 0.0 },
        if st.neo_gen() {
            Paint::fill(&Color::none())
        } else {
            Paint::fill(&pal.commit_label_bkg).with_opacity(0.5)
        },
        "commit-label-bkg",
    );
    let txt = Element::classed_text(
        Text {
            x: tx,
            y: ty,
            lines: vec![id.to_string()],
            line_height: h,
            font_size: LABEL_FONT,
            bold: st.redux,
            anchor: Anchor::Start,
            fill: if st.neo_gen() {
                pal.node_border.clone()
            } else {
                pal.commit_label_color.clone()
            },
        },
        "commit-label",
    );
    let text_rect = Rect::new(tx, ty - ascent, w, h);
    if !gc.rotate_commit_label {
        bounds.add_rect(rect);
        bounds.add_rect(text_rect);
        return vec![bkg, txt];
    }
    if o == Orientation::LR {
        // The group: `translate(r_x, r_y) rotate(-45, pos, y)` (its box is the
        // union of the box and the text, turned).
        let r_x = -7.5 - (w + 10.0) / 25.0 * 9.5;
        let r_y = 10.0 + w / 25.0 * 8.5;
        let about = Point::new(pl.pos + r_x, pl.y + r_y);
        let mut union = Bounds::default();
        union.add_rect(rect);
        union.add_rect(text_rect);
        let shift = |e: Element| e.translated(r_x, r_y);
        let u = union.rect();
        bounds.add_rotated_rect(Rect::new(u.x + r_x, u.y + r_y, u.w, u.h), about, -45.0);
        vec![Element::Rotated {
            deg: -45.0,
            about,
            children: vec![shift(bkg), shift(txt)],
        }]
    } else {
        // Text and box each `rotate(-45, x, y)`.
        let about = Point::new(pl.x, pl.y);
        bounds.add_rotated_rect(rect, about, -45.0);
        bounds.add_rotated_rect(text_rect, about, -45.0);
        vec![Element::Rotated {
            deg: -45.0,
            about,
            children: vec![bkg, txt],
        }]
    }
}

/// `drawCommitTags`: newest tag first, stacked away from the commit, every tag
/// of a commit as wide and tall as the largest `getBBox()` among them.
fn commit_tags(
    c: &Commit,
    pl: &Placed,
    o: Orientation,
    pal: &Palette,
    (tag_bkg, tag_border): (&Color, &Color),
    bounds: &mut Bounds,
) -> Vec<Element> {
    let mut out = Vec::new();
    if c.tags.is_empty() {
        return out;
    }
    let ascent = (0.905 * TAG_FONT).round();
    let tag_w = c
        .tags
        .iter()
        .map(|t| svg_bbox_width(t, TAG_FONT))
        .fold(0.0, f64::max);
    let h2 = c
        .tags
        .iter()
        .map(|t| svg_line_box(TAG_FONT, t))
        .fold(0.0, f64::max)
        / 2.0;
    let pos = pl.pos;
    let text = |x: f64, y: f64, t: &str| Text {
        x,
        y,
        lines: vec![t.to_string()],
        line_height: TAG_FONT * 1.2,
        font_size: TAG_FONT,
        bold: false,
        anchor: Anchor::Start,
        fill: pal.tag_label_color.clone(),
    };
    for (k, t) in c.tags.iter().rev().enumerate() {
        let y_off = k as f64 * 20.0;
        let tw = svg_bbox_width(t, TAG_FONT);
        if o != Orientation::LR {
            // TB/BT: laid out to the right of the commit, then turned +45° about
            // (x, pos) after a (12,12) / (14,14) shift.
            let y = pos + y_off;
            let poly: Vec<Point> = [
                (pl.x, y + 2.0),
                (pl.x, y - 2.0),
                (pl.x + LAYOUT_OFFSET, y - h2 - 2.0),
                (pl.x + LAYOUT_OFFSET + tag_w + 4.0, y - h2 - 2.0),
                (pl.x + LAYOUT_OFFSET + tag_w + 4.0, y + h2 + 2.0),
                (pl.x + LAYOUT_OFFSET, y + h2 + 2.0),
            ]
            .iter()
            .map(|&(x, y)| Point::new(x + 12.0, y + 12.0))
            .collect();
            let about = Point::new(pl.x + 12.0, pos + 12.0);
            let mut local = Bounds::default();
            for q in &poly {
                local.add_point(*q);
            }
            bounds.add_rotated_rect(local.rect(), about, 45.0);
            let hole = Point::new(pl.x + PX / 2.0 + 12.0, y + 12.0);
            bounds.add_rotated_rect(Rect::centered(hole, 3.0, 3.0), about, 45.0);
            out.push(Element::Rotated {
                deg: 45.0,
                about,
                children: vec![
                    Element::classed_shape(
                        Geometry::Path(geom::polygon(&poly)),
                        Paint::both(tag_bkg, tag_border, 1.0),
                        "tag-label-bkg",
                    ),
                    Element::classed_shape(
                        Geometry::Circle { c: hole, r: 1.5 },
                        Paint::fill(&pal.text_color),
                        "tag-hole",
                    ),
                ],
            });
            let (tx, ty) = (pl.x + 5.0 + 14.0, y + 3.0 + 14.0);
            let about = Point::new(pl.x + 14.0, pos + 14.0);
            bounds.add_rotated_rect(
                Rect::new(tx, ty - ascent, tw, svg_line_box(TAG_FONT, t)),
                about,
                45.0,
            );
            out.push(Element::Rotated {
                deg: 45.0,
                about,
                children: vec![Element::classed_text(text(tx, ty, t), "tag-label")],
            });
            continue;
        }
        let ly = pl.y - 19.2 - y_off;
        let wo = pl.pos_with_offset;
        let poly = vec![
            Point::new(pos - tag_w / 2.0 - PX / 2.0, ly + PY),
            Point::new(pos - tag_w / 2.0 - PX / 2.0, ly - PY),
            Point::new(wo - tag_w / 2.0 - PX, ly - h2 - PY),
            Point::new(wo + tag_w / 2.0 + PX, ly - h2 - PY),
            Point::new(wo + tag_w / 2.0 + PX, ly + h2 + PY),
            Point::new(wo - tag_w / 2.0 - PX, ly + h2 + PY),
        ];
        for q in &poly {
            bounds.add_point(*q);
        }
        let hole = Point::new(pos - tag_w / 2.0 + PX / 2.0, ly);
        bounds.add_rect(Rect::centered(hole, 3.0, 3.0));
        out.push(Element::classed_shape(
            Geometry::Path(geom::polygon(&poly)),
            Paint::both(tag_bkg, tag_border, 1.0),
            "tag-label-bkg",
        ));
        out.push(Element::classed_shape(
            Geometry::Circle { c: hole, r: 1.5 },
            Paint::fill(&pal.text_color),
            "tag-hole",
        ));
        let (tx, ty) = (wo - tw / 2.0, pl.y - 16.0 - y_off);
        bounds.add_rect(text_box(tx, ty, t, TAG_FONT));
        out.push(Element::classed_text(text(tx, ty, t), "tag-label"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"gitGraph
  commit id: "init"
  commit tag: "v0.1"
  branch develop
  checkout develop
  commit type: HIGHLIGHT
  commit
  branch feature order: 3
  commit id: "feat"
  checkout main
  commit type: REVERSE
  merge develop id: "m1" tag: "v1.0"
  switch feature
  commit
  checkout develop
  cherry-pick id: "feat"
"#;

    #[test]
    fn parses_statements() {
        let g = parse(SRC, None, &ParseOptions::default()).unwrap();
        assert_eq!(g.branches.len(), 3);
        assert_eq!(g.commits.len(), 9);
        assert_eq!(g.commits[0].id, "init");
        assert_eq!(g.commits[1].tags, vec!["v0.1"]);
        assert_eq!(g.commits[2].kind, CommitType::Highlight);
        assert_eq!(g.commits[2].parents, vec![1]);
        let m = g.commits.iter().find(|c| c.id == "m1").unwrap();
        assert_eq!(m.kind, CommitType::Merge);
        assert_eq!(m.parents.len(), 2);
        let cp = g.commits.last().unwrap();
        assert_eq!(cp.kind, CommitType::CherryPick);
        assert_eq!(cp.tags, vec!["cherry-pick:feat"]);
        // Lane order: main (0), develop (0.1), feature (3).
        assert_eq!(g.lane_order(), vec![0, 1, 2]);
    }

    #[test]
    fn auto_ids_are_deterministic() {
        let a = parse(SRC, None, &ParseOptions::default()).unwrap();
        let b = parse(SRC, None, &ParseOptions::default()).unwrap();
        assert_eq!(a.commits[1].id, b.commits[1].id);
        assert!(a.commits[1].id.starts_with("1-"));
        assert_eq!(a.commits[1].id.len(), 9);
    }

    #[test]
    fn orientation_header() {
        let g = parse("gitGraph TB:\ncommit", None, &ParseOptions::default()).unwrap();
        assert_eq!(g.orientation, Orientation::TB);
        let g = parse("gitGraph BT:\ncommit", None, &ParseOptions::default()).unwrap();
        assert_eq!(g.orientation, Orientation::BT);
    }

    #[test]
    fn errors_with_lines() {
        let o = ParseOptions::default();
        let e = parse("gitGraph\ncommit\ncheckout nope", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }), "{e:?}");
        let e = parse("gitGraph\ncommit\nbranch main", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("gitGraph\ncommit\nmerge main", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("gitGraph\ncommit\nbranch b\nmerge main", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 4, .. }), "{e:?}");
        let e = parse("gitGraph\ncommit type: WEIRD", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
        let e = parse(
            "gitGraph\ncommit id: \"a\"\ncherry-pick id: \"a\"",
            None,
            &o,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Parse { line: 3, .. }));
        let e = parse("gitGraph\nfoo", None, &o).unwrap_err();
        assert!(matches!(e, Error::Parse { line: 2, .. }));
    }

    #[test]
    fn layout_separates_commits_and_lanes() {
        let g = parse(SRC, None, &ParseOptions::default()).unwrap();
        let gc = GitConfig::default();
        let l = layout(&g, &gc, false);
        let xs: Vec<f64> = l.placed.iter().map(|p| p.x).collect();
        for w in xs.windows(2) {
            assert!(w[1] > w[0]);
        }
        assert!(l.lane[1] > l.lane[0] && l.lane[2] > l.lane[1]);
        let gp = GitConfig {
            parallel_commits: true,
            ..GitConfig::default()
        };
        let lp = layout(&g, &gp, false);
        // In parallel mode a commit is one step after its latest parent.
        let x = |i: usize| lp.placed.iter().find(|p| p.commit == i).unwrap().x;
        assert_eq!(x(3), x(2) + 50.0);
        let s = draw(
            &g,
            &l,
            &gc,
            &crate::theme::palette(crate::theme::ThemeName::Default, &[]),
        );
        assert!(s.element_count() > 20);
    }

    /// `(deg, about, child count)` of every `Rotated` element.
    fn rotations(els: &[Element]) -> Vec<(f64, Point, usize)> {
        let mut out = Vec::new();
        for e in els {
            match e {
                Element::Group { children, .. } => out.extend(rotations(children)),
                Element::Rotated {
                    deg,
                    about,
                    children,
                } => out.push((*deg, *about, children.len())),
                _ => {}
            }
        }
        out
    }

    fn scene_for(src: &str, gc: &GitConfig) -> Scene {
        let g = parse(src, None, &ParseOptions::default()).unwrap();
        let l = layout(&g, gc, false);
        draw(
            &g,
            &l,
            gc,
            &crate::theme::palette(crate::theme::ThemeName::Default, &[]),
        )
    }

    /// Tag boxes follow the tags' `getBBox()`: 11px tall at 10px (12 with a
    /// deep descender), as wide as the widest tag's ink. Reference (mermaid
    /// 12.3.0, the gallery example): `v0.1` spans x 38.330..73.670 and is 15
    /// tall (y -28.7..-13.7); `cherry-pick:hotfix` 16 (y 60.8..76.8).
    #[test]
    fn tag_boxes_follow_get_bbox() {
        let example = include_str!("../../examples/diagrams/gitgraph.mmd");
        // (without its title front matter)
        let body = example.splitn(3, "---\n").nth(2).unwrap();
        let s = scene_for(body, &GitConfig::default());
        fn polys(els: &[Element], out: &mut Vec<Rect>) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Rotated { children, .. } => {
                        polys(children, out)
                    }
                    Element::Shape {
                        geom: Geometry::Path(cmds),
                        class: Some(c),
                        ..
                    } if c == "tag-label-bkg" => {
                        let mut b = Bounds::default();
                        for c in cmds {
                            if let PathCmd::MoveTo(p) | PathCmd::LineTo(p) = c {
                                b.add_point(*p);
                            }
                        }
                        out.push(b.rect());
                    }
                    _ => {}
                }
            }
        }
        let mut tags = Vec::new();
        polys(&s.elements, &mut tags);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-3;
        let v01 = tags[0];
        assert!(
            near(v01.x, 38.330) && near(v01.right(), 73.670) && near(v01.h, 15.0),
            "{v01:?}"
        );
        let cherry = tags.last().unwrap();
        assert!(near(cherry.h, 16.0), "{cherry:?}");
    }

    /// The statements of the comparison cases below: tags, a highlight, a
    /// reverse, a merge with a tag, a cherry-pick, three lanes.
    const FLOW: &str = "  commit id: \"c1\" tag: \"v1\"\n  commit id: \"c2\"\n  branch develop\n  checkout develop\n  commit id: \"d1\" tag: \"alpha\" tag: \"beta\"\n  commit id: \"d2\" type: HIGHLIGHT\n  checkout main\n  commit id: \"c3\" type: REVERSE\n  branch feature\n  commit id: \"f1\"\n  checkout develop\n  commit id: \"d3\"\n  checkout main\n  merge develop id: \"m1\" tag: \"v2\"\n  checkout feature\n  commit id: \"f2\"\n  checkout main\n  cherry-pick id: \"f1\"\n  commit id: \"c4\"\n";

    fn view_box(src: &str) -> Rect {
        crate::render::render(src, &MermaidConfig::default())
            .unwrap()
            .scene
            .view_box
    }

    fn assert_box(got: Rect, want: [f64; 4]) {
        let g = [got.x, got.y, got.w, got.h];
        assert!(
            g.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3),
            "{g:?} != {want:?}"
        );
    }

    /// viewBoxes mermaid 12.3.0 gives these graphs (`getBBox()` of everything drawn,
    /// padded by `diagramPadding`): every orientation, parallel commits (the BT
    /// walk of `setParallelBTPos` included) and a title centred on the drawing.
    #[test]
    fn view_boxes_match_mermaid() {
        let with = |head: &str, cfg: &str| {
            let fm = if cfg.is_empty() {
                String::new()
            } else {
                format!("---\nconfig:\n{cfg}---\n")
            };
            format!("{fm}{head}\n{FLOW}")
        };
        let par = "  gitGraph:\n    parallelCommits: true\n";
        assert_box(
            view_box(&with("gitGraph TB:", "")),
            [-41.4674186706543, -8.0, 307.8424072265625, 596.0],
        );
        assert_box(
            view_box(&with("gitGraph BT:", "")),
            [-41.4674186706543, 22.0, 307.8424072265625, 588.0],
        );
        assert_box(
            view_box(&with("gitGraph", par)),
            [
                -117.1484375,
                -37.20000076293945,
                525.1484375,
                253.78392028808594,
            ],
        );
        assert_box(
            view_box(&with("gitGraph BT:", par)),
            [-41.4674186706543, -98.0, 307.8424072265625, 438.0],
        );
        let titled = "---\ntitle: Example Git diagram\n---\ngitGraph\n   commit id: \"0-a3c0f7e\"\n   commit id: \"1-2b4c1d8\"\n   branch develop\n   checkout develop\n   commit id: \"2-0fd5e9a\"\n   commit id: \"3-9c7e2b1\"\n   checkout main\n   merge develop\n   commit id: \"5-77ab3c0\"\n   commit id: \"6-c1d2e3f\"\n";
        let b = view_box(titled);
        // (The reference's ids are random, and its right and bottom edges are
        // their labels': its left edge is the `develop` label, its top the
        // title's text box above `y = -titleTopMargin`.)
        assert!(
            (b.x - -117.1484375).abs() < 2e-3 && (b.y - -49.0).abs() < 2e-3,
            "{b:?}"
        );
    }

    /// A commit id used twice: mermaid's commit map keeps one entry (the later
    /// commit, in the earlier one's place), so the earlier commit is not drawn
    /// and its children hang off the later one. mermaid 12.3.0's viewBox for
    /// its "MetroLine" example, which repeats `Boston`.
    #[test]
    fn a_repeated_commit_id_replaces_the_earlier_commit() {
        let src = "---\nconfig:\n  theme: 'base'\n  gitGraph:\n    mainBranchName: 'MetroLine1'\n---\ngitGraph\n  commit id:\"NewYork\"\n  commit id:\"Dallas\"\n  branch MetroLine2\n  commit id:\"LosAngeles\"\n  commit id:\"Chicago\"\n  commit id:\"Houston\"\n  branch MetroLine3\n  commit id:\"Phoenix\"\n  commit type: HIGHLIGHT id:\"Denver\"\n  commit id:\"Boston\"\n  checkout MetroLine1\n  commit id:\"Atlanta\"\n  merge MetroLine3\n  commit id:\"Miami\"\n  commit id:\"Washington\"\n  merge MetroLine2 tag:\"MY JUNCTION\"\n  commit id:\"Boston\"\n  commit id:\"Detroit\"\n  commit type:REVERSE id:\"SanFrancisco\"\n";
        assert_box(
            view_box(src),
            [
                -141.1484375,
                -36.70000076293945,
                899.1484375,
                272.2611389160156,
            ],
        );
        let g = parse(
            src.splitn(3, "---\n").nth(2).unwrap(),
            None,
            &ParseOptions {
                main_branch_name: "MetroLine1".into(),
                main_branch_order: 0.0,
            },
        )
        .unwrap();
        let l = layout(&g, &GitConfig::default(), false);
        assert_eq!(l.placed.len(), g.commits.len() - 1);
    }

    /// Merge bullets keep the full radius under their hole; tags are as tall as
    /// their text box (`RC_1`: 11px at 10px, `_` does not reach below the
    /// descent there), and the label colours are `commitLabelColor` on
    /// `commitLabelBackground`, tags `tagLabelColor` / `tagLabelBorder`.
    #[test]
    fn bullets_tags_and_label_colours_follow_mermaid() {
        let s = scene_for(
            "gitGraph\n  commit id: \"a\" tag: \"RC_1\"\n  branch b\n  commit id: \"x\"\n  checkout main\n  merge b\n",
            &GitConfig::default(),
        );
        let mut circles = Vec::new();
        let mut shapes = Vec::new();
        let mut texts = Vec::new();
        fn walk(
            els: &[Element],
            c: &mut Vec<(String, f64)>,
            sh: &mut Vec<(String, Paint, Rect)>,
            t: &mut Vec<(String, Color)>,
        ) {
            for e in els {
                match e {
                    Element::Group { children, .. } | Element::Rotated { children, .. } => {
                        walk(children, c, sh, t)
                    }
                    Element::Shape {
                        geom: Geometry::Circle { r, .. },
                        class: Some(k),
                        ..
                    } => c.push((k.clone(), *r)),
                    Element::Shape {
                        geom,
                        paint,
                        class: Some(k),
                    } => {
                        let r = match geom {
                            Geometry::Rect { rect, .. } => *rect,
                            Geometry::Path(cmds) => geom::path_bbox(cmds).unwrap_or_default(),
                            Geometry::Circle { .. } => Rect::default(),
                        };
                        sh.push((k.clone(), paint.clone(), r));
                    }
                    Element::Text {
                        text,
                        class: Some(k),
                    } => t.push((k.clone(), text.fill.clone())),
                    _ => {}
                }
            }
        }
        walk(&s.elements, &mut circles, &mut shapes, &mut texts);
        let merge: Vec<f64> = circles
            .iter()
            .filter(|(k, _)| k.contains("3-") || k.contains("2-"))
            .map(|(_, r)| *r)
            .collect();
        assert_eq!(merge, vec![10.0, 6.0], "{circles:?}");
        let (_, tag_paint, tag_box) = shapes.iter().find(|(k, ..)| k == "tag-label-bkg").unwrap();
        assert_eq!(tag_box.h, 15.0);
        assert_eq!(tag_paint.stroke.css(), "hsl(240, 60%, 86.2745098039%)");
        assert_eq!(tag_paint.fill.css().to_ascii_lowercase(), "#ececff");
        let (_, bkg, _) = shapes
            .iter()
            .find(|(k, ..)| k == "commit-label-bkg")
            .unwrap();
        assert_eq!(bkg.fill.css().to_ascii_lowercase(), "#ffffde");
        assert_eq!(bkg.opacity, Some(0.5));
        let label = texts.iter().find(|(k, _)| k == "commit-label").unwrap();
        assert_eq!(label.1.css().to_ascii_lowercase(), "#000021");
        let tag = texts.iter().find(|(k, _)| k == "tag-label").unwrap();
        assert_eq!(tag.1.css().to_ascii_lowercase(), "#131300");
    }

    #[test]
    fn commit_labels_rotate_by_default() {
        let src = "gitGraph\n  commit id: \"abc\"\n  commit id: \"defgh\"";
        let s = scene_for(src, &GitConfig::default());
        let rot = rotations(&s.elements);
        assert_eq!(rot.len(), 2);
        // Background and text turn together by -45°.
        assert!(rot.iter().all(|(d, _, n)| *d == -45.0 && *n == 2));
        // LR: about (pos, y) shifted by mermaid's (r_x, r_y), from the label's
        // `getBBox` width.
        let w = svg_bbox_width("abc", LABEL_FONT);
        let (dx, dy) = (-7.5 - (w + 10.0) / 25.0 * 9.5, 10.0 + w / 25.0 * 8.5);
        assert!((rot[0].1.x - (0.0 + dx)).abs() < 1e-9, "{rot:?}");
        // (The spine, and so the commit, sits 2px above the branch position.)
        assert!((rot[0].1.y - (-2.0 + dy)).abs() < 1e-9);
        // Lanes get mermaid's 40px of extra room.
        let g = parse(
            "gitGraph\ncommit\nbranch b\ncommit",
            None,
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(
            layout(&g, &GitConfig::default(), false).lane,
            vec![0.0, 90.0]
        );
    }

    #[test]
    fn no_rotation_when_disabled() {
        let gc = GitConfig {
            rotate_commit_label: false,
            ..GitConfig::default()
        };
        let s = scene_for("gitGraph\n  commit id: \"abc\"", &gc);
        assert!(rotations(&s.elements).is_empty());
        let g = parse(
            "gitGraph\ncommit\nbranch b\ncommit",
            None,
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(layout(&g, &gc, false).lane, vec![0.0, 50.0]);
    }

    #[test]
    fn vertical_labels_and_tags_rotate() {
        let src = "gitGraph TB:\n  commit id: \"abc\" tag: \"v1\"";
        let s = scene_for(src, &GitConfig::default());
        let rot = rotations(&s.elements);
        // Label about the commit centre (-45°), tag shape and tag text (+45°).
        let p = Point::new(0.0, 40.0);
        assert!(rot.contains(&(-45.0, p, 2)), "{rot:?}");
        assert!(rot.contains(&(45.0, Point::new(12.0, 42.0), 2)));
        assert!(rot.contains(&(45.0, Point::new(14.0, 44.0), 1)));
        // BT puts the newest commit first, at defaultPos + LAYOUT_OFFSET.
        let g = parse(
            "gitGraph BT:\ncommit\ncommit",
            None,
            &ParseOptions::default(),
        )
        .unwrap();
        let ys: Vec<(usize, f64)> = layout(&g, &GitConfig::default(), false)
            .placed
            .iter()
            .map(|p| (p.commit, p.y))
            .collect();
        assert_eq!(ys, vec![(1, 40.0), (0, 90.0)]);
    }

    #[test]
    fn theme_styles_follow_upstream() {
        use crate::theme::{ThemeName, palette};
        let css = |c: &Color| c.css().to_ascii_lowercase();
        let classic = palette(ThemeName::Default, &[]);
        let st = GitStyle::new(&classic);
        assert_eq!(css(&st.lane(1)), css(&classic.git[1]));
        // The cherry-pick bullet has no `commit{k}` class: it inherits `textColor`.
        assert_eq!(css(&st.inherited_fill()), "#333");
        let neo = palette(ThemeName::Neo, &[]);
        let st = GitStyle::new(&neo);
        assert_eq!(css(&st.lane(0)), "#000000"); // nodeBorder
        assert_eq!(css(&st.lane(2)), css(&neo.git[2]));
        assert_eq!(st.arrow_width(), 8.0);
        let rdc = palette(ThemeName::ReduxDarkColor, &[]);
        let st = GitStyle::new(&rdc);
        // calcColorIndex skips slot 0 for lanes > 0: lane 8 -> class 1.
        assert_eq!(st.commit_index(8), 1);
        assert_eq!(css(&st.lane(1)), "#2dd4bf");
        assert_eq!(st.arrow_width(), 2.0);
        assert!(st.dark && st.redux);
        assert_eq!(css(&st.label_box(1).0), "#111113"); // dark: mainBkg fill
    }

    #[test]
    fn branch_labels_and_title_follow_mermaid() {
        // mermaid 12.3.0: `main`'s label box is its 16px text box (34.69 x 17)
        // plus 18 x 4, its left edge 53px further left; the title's baseline
        // sits at y = -titleTopMargin.
        let src = "gitGraph\n  commit id: \"a\"";
        let g = parse(src, Some("T".into()), &ParseOptions::default()).unwrap();
        let gc = GitConfig::default();
        let s = draw(
            &g,
            &layout(&g, &gc, false),
            &gc,
            &crate::theme::palette(crate::theme::ThemeName::Default, &[]),
        );
        let mut rect = None;
        let mut title = None;
        fn walk<'a>(els: &'a [Element], r: &mut Option<Rect>, t: &mut Option<&'a Text>) {
            for e in els {
                match e {
                    Element::Group { children, .. } => walk(children, r, t),
                    Element::Shape {
                        geom: Geometry::Rect { rect, .. },
                        class: Some(c),
                        ..
                    } if c.starts_with("branchLabelBkg") => *r = Some(*rect),
                    Element::Text {
                        text,
                        class: Some(c),
                    } if c == "gitTitleText" => *t = Some(text),
                    _ => {}
                }
            }
        }
        walk(&s.elements, &mut rect, &mut title);
        let r = rect.unwrap();
        let tw = svg_bbox_width("main", 16.0);
        assert!((r.w - (tw + 18.0)).abs() < 1e-9 && r.h == 21.0, "{r:?}");
        assert!((r.x - (-tw - 53.0)).abs() < 1e-9, "{r:?}");
        assert_eq!(title.unwrap().y, -25.0);
    }

    #[test]
    fn left_to_right_spines_sit_off_the_branch_position() {
        // mermaid 12.3: `spineY = pos - 2` (redux: pos + 12 / 2 + 1), and the
        // commits follow. Branch lines and bullets of `main` / `develop`:
        let spines = |theme: &str| {
            let cfg = MermaidConfig::default()
                .with_json(&crate::json::parse(&format!("{{\"theme\": \"{theme}\"}}")).unwrap())
                .unwrap();
            let src = "gitGraph\n  commit\n  branch develop\n  commit";
            let s = render(src, None, &cfg, &cfg.palette()).unwrap();
            let mut ys = Vec::new();
            fn walk(els: &[Element], ys: &mut Vec<(String, f64)>) {
                for e in els {
                    match e {
                        Element::Group { children, .. } | Element::Rotated { children, .. } => {
                            walk(children, ys)
                        }
                        Element::Shape {
                            geom: Geometry::Path(p),
                            class: Some(c),
                            ..
                        } if c.starts_with("branch ") => {
                            if let PathCmd::MoveTo(a) = p[0] {
                                ys.push(("branch".into(), a.y));
                            }
                        }
                        Element::Shape {
                            geom: Geometry::Circle { c, .. },
                            class: Some(k),
                            ..
                        } if k.starts_with("commit ") => ys.push(("commit".into(), c.y)),
                        _ => {}
                    }
                }
            }
            walk(&s.elements, &mut ys);
            ys
        };
        let ys = spines("default");
        assert!(ys.contains(&("branch".into(), -2.0)) && ys.contains(&("branch".into(), 88.0)));
        assert!(ys.contains(&("commit".into(), -2.0)) && ys.contains(&("commit".into(), 88.0)));
        let ys = spines("redux");
        assert!(ys.contains(&("branch".into(), 7.0)) && ys.contains(&("commit".into(), 97.0)));
    }
}
