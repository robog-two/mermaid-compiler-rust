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
//! Commit labels follow `gitGraphRenderer.ts`: with `rotateCommitLabel` (the
//! default) they and their backgrounds are turned -45°, and every lane gets
//! 40px extra room; TB/BT tags are always turned +45°. With
//! `rotateCommitLabel: false` the LR commit step grows when labels/tags are
//! wider than mermaid's 50px spacing so horizontal labels never overlap
//! (mermaid lets them overlap).

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::geom::{self, Bounds, PathCmd, Point, Rect};
use crate::scene::{Anchor, Color, DiagramKind, Element, Geometry, Paint, Scene, Text};
use crate::text::{TextBlock, line_width};
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

const LABEL_FONT: f64 = 10.0;
const TAG_FONT: f64 = 10.0;
const BRANCH_FONT: f64 = 14.0;

/// Abstract layout: `along` = commit axis, `across` = lane axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Lane coordinate per branch index.
    pub lane: Vec<f64>,
    /// Display index (colour) per branch index.
    pub lane_index: Vec<usize>,
    /// `along` coordinate per commit.
    pub pos: Vec<f64>,
    /// Extent of the commit axis.
    pub max_pos: f64,
}

fn shown_label(c: &Commit, gc: &GitConfig) -> bool {
    gc.show_commit_label
        && c.kind != CommitType::CherryPick
        && (c.kind != CommitType::Merge || c.custom_id)
}

/// Compute lanes and commit positions.
pub fn layout(g: &GitGraph, gc: &GitConfig) -> Layout {
    let vertical = g.orientation != Orientation::LR;
    let order = g.lane_order();
    let mut lane = vec![0.0; g.branches.len()];
    let mut lane_index = vec![0; g.branches.len()];
    let mut p = 0.0;
    for (idx, &b) in order.iter().enumerate() {
        lane[b] = p;
        lane_index[b] = idx;
        // mermaid's `setBranchPosition`.
        p += 50.0 + if gc.rotate_commit_label { 40.0 } else { 0.0 };
        if vertical {
            p += line_width(&g.branches[b].name, BRANCH_FONT, false) / 2.0;
        }
    }
    // Commit step: mermaid uses 50; with horizontal LR labels widen it so
    // labels/tags never collide.
    let step = if vertical || gc.rotate_commit_label {
        50.0
    } else {
        let widest = g
            .commits
            .iter()
            .map(|c| {
                let lw = if shown_label(c, gc) {
                    line_width(&c.id, LABEL_FONT, false) + 8.0
                } else {
                    0.0
                };
                let tw = c
                    .tags
                    .iter()
                    .map(|t| line_width(t, TAG_FONT, false) + 16.0)
                    .fold(0.0, f64::max);
                lw.max(tw)
            })
            .fold(0.0, f64::max);
        widest.max(50.0)
    };
    let start = if vertical { 30.0 } else { 0.0 };
    let mut pos = vec![0.0; g.commits.len()];
    let mut max_pos: f64 = start;
    for (i, c) in g.commits.iter().enumerate() {
        let slot = if gc.parallel_commits {
            c.parents
                .iter()
                .map(|&pi| pos[pi] + step)
                .fold(start + 10.0, f64::max)
        } else {
            start + 10.0 + step * i as f64
        };
        pos[i] = slot;
        max_pos = max_pos.max(slot + step - 10.0);
    }
    if g.orientation == Orientation::BT {
        for p in &mut pos {
            // mermaid walks the commits newest-first from `defaultPos`, so
            // the newest sits at start + 10 and the branch labels at max_pos.
            *p = max_pos - *p;
        }
    }
    Layout {
        lane,
        lane_index,
        pos,
        max_pos,
    }
}

/// mermaid's `LAYOUT_OFFSET`: a commit sits this far past its slot start.
const LAYOUT_OFFSET: f64 = 10.0;

/// Commit label background, text (anchor start; fill set by the caller) and
/// optional rotation `(about, deg)` for a commit at `p`, after mermaid's
/// `drawCommitLabel` (PX = 4, PY = 2).
fn commit_label_geometry(
    o: Orientation,
    p: Point,
    id: &str,
    rotate: bool,
) -> (Rect, Text, Option<(Point, f64)>) {
    let w = line_width(id, LABEL_FONT, false);
    let h = LABEL_FONT * 1.2;
    let text = |x: f64, y: f64| Text {
        x,
        y,
        lines: vec![id.to_string()],
        line_height: h,
        font_size: LABEL_FONT,
        bold: false,
        anchor: Anchor::Start,
        fill: Color::new("#000"),
    };
    if o == Orientation::LR {
        // The whole label group is `translate(r_x, r_y) rotate(-45, pos, y)`
        // with pos = along - LAYOUT_OFFSET; translate-then-rotate equals
        // rotating the shifted label about the shifted centre.
        let (dx, dy) = if rotate {
            (-7.5 - (w + 10.0) / 25.0 * 9.5, 10.0 + w / 25.0 * 8.5)
        } else {
            (0.0, 0.0)
        };
        let rect = Rect::new(p.x - w / 2.0 - 2.0 + dx, p.y + 13.5 + dy, w + 4.0, h + 4.0);
        let t = text(p.x - w / 2.0 + dx, p.y + 25.0 + dy);
        let rot = rotate.then(|| (Point::new(p.x - LAYOUT_OFFSET + dx, p.y + dy), -45.0));
        (rect, t, rot)
    } else {
        let rect = Rect::new(p.x - (w + 16.0 + 5.0), p.y - 12.0, w + 4.0, h + 4.0);
        let t = text(p.x - (w + 16.0), p.y + h - 12.0);
        (rect, t, rotate.then_some((p, -45.0)))
    }
}

fn to_xy(o: Orientation, along: f64, across: f64) -> Point {
    match o {
        Orientation::LR => Point::new(along, across),
        _ => Point::new(across, along),
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

    /// `NEO_COLOR_GEN_THEMES`.
    fn neo_gen(&self) -> bool {
        self.family != Family::Classic
    }
}

/// Orthogonal polyline with rounded corners of radius `r` (quarter circles).
fn rounded(points: &[Point], r: f64) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(points[0])];
    for i in 1..points.len() {
        let p = points[i];
        if i + 1 < points.len() {
            let prev = points[i - 1];
            let next = points[i + 1];
            let din = p.minus(prev);
            let dout = next.minus(p);
            let rr = r.min(din.len() / 2.0).min(dout.len() / 2.0);
            let a = p.minus(din.norm().scale(rr));
            let b = p.plus(dout.norm().scale(rr));
            let k = 0.552_284_75;
            cmds.push(PathCmd::LineTo(a));
            cmds.push(PathCmd::CubicTo(
                a.plus(din.norm().scale(rr * k)),
                b.minus(dout.norm().scale(rr * k)),
                b,
            ));
        } else {
            cmds.push(PathCmd::LineTo(p));
        }
    }
    cmds
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
    let lay = layout(&g, &gc);
    Ok(draw(&g, &lay, &gc, pal))
}

/// Draw a laid-out git graph.
pub fn draw(g: &GitGraph, lay: &Layout, gc: &GitConfig, pal: &Palette) -> Scene {
    let o = g.orientation;
    let vertical = o != Orientation::LR;
    let mut bounds = Bounds::default();
    let pt = |c: usize| to_xy(o, lay.pos[c], lay.lane[g.commits[c].branch]);
    let st = GitStyle::new(pal);
    let ck = |b: usize| st.commit_index(lay.lane_index[b]);
    let start: f64 = if vertical { 30.0 } else { 0.0 };
    let (lo, hi) = (start.min(lay.max_pos), lay.max_pos.max(start));

    // Branch lines and labels.
    let mut branch_els = Vec::new();
    if gc.show_branches {
        for (b, br) in g.branches.iter().enumerate() {
            let across = lay.lane[b];
            let a = to_xy(o, lo, across);
            let z = to_xy(o, hi, across);
            bounds.add_point(a);
            bounds.add_point(z);
            branch_els.push(Element::classed_shape(
                Geometry::Path(vec![PathCmd::MoveTo(a), PathCmd::LineTo(z)]),
                Paint::stroke(&st.branch_line(), st.stroke_width()).dashed(if st.neo_gen() {
                    vec![4.0, 2.0]
                } else {
                    vec![2.0]
                }),
                format!("branch branch{}", st.branch_index(lay.lane_index[b])),
            ));
            let tb = TextBlock::new(vec![br.name.clone()], BRANCH_FONT, false);
            let (w, h) = (tb.width + 18.0, tb.height + 4.0);
            // mermaid's `drawBranches`: LR labels move 30px further left when
            // commit labels are rotated (they lean towards the branch label).
            let rot_shift = if gc.rotate_commit_label { 30.0 } else { 0.0 };
            let rect = match o {
                Orientation::LR => {
                    Rect::new(-tb.width - 4.0 - rot_shift - 19.0, across - h / 2.0, w, h)
                }
                Orientation::TB => Rect::new(across - tb.width / 2.0 - 10.0, 0.0, w, h),
                Orientation::BT => Rect::new(across - tb.width / 2.0 - 10.0, hi, w, h),
            };
            bounds.add_rect(rect);
            let k = st.branch_index(lay.lane_index[b]);
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
            let text = Text::centered(
                rect.center(),
                &tb.lines,
                BRANCH_FONT,
                tb.line_height,
                &st.branch_label(k),
            );
            branch_els.push(Element::classed_text(
                if st.redux { text.bold() } else { text },
                format!("branch-label{k}"),
            ));
        }
    }

    // Arrows (parent → child).
    let mut arrow_els = Vec::new();
    let mut lanes_used: Vec<f64> = lay.lane.clone();
    for (ci, c) in g.commits.iter().enumerate() {
        for (pi_n, &pi) in c.parents.iter().enumerate() {
            let a = &g.commits[pi];
            let (pa, pb) = (lay.pos[pi], lay.pos[ci]);
            let (la, lb) = (lay.lane[a.branch], lay.lane[c.branch]);
            let merge_side = c.kind == CommitType::Merge && pi_n != 0;
            let mut color_branch = if merge_side { a.branch } else { c.branch };
            let pts: Vec<(f64, f64)>;
            let radius;
            if (la - lb).abs() < 1e-9 {
                pts = vec![(pa, la), (pb, lb)];
                radius = 0.0;
            } else {
                // Reroute when a commit on the curving branch sits between the two.
                let curve_branch = if lb > la { c.branch } else { a.branch };
                let blocked = g
                    .commits
                    .iter()
                    .any(|x| x.seq > a.seq && x.seq < c.seq && x.branch == curve_branch);
                if blocked {
                    let mid = find_lane(&mut lanes_used, la.min(lb), la.max(lb));
                    if lb < la {
                        color_branch = a.branch;
                    }
                    pts = vec![(pa, la), (pa, mid), (pb, mid), (pb, lb)];
                    radius = 10.0;
                } else if merge_side {
                    pts = vec![(pa, la), (pb, la), (pb, lb)];
                    radius = 20.0;
                } else {
                    pts = vec![(pa, la), (pa, lb), (pb, lb)];
                    radius = 20.0;
                }
            }
            let xy: Vec<Point> = pts.iter().map(|&(al, ac)| to_xy(o, al, ac)).collect();
            arrow_els.push(Element::classed_shape(
                Geometry::Path(rounded(&xy, radius)),
                Paint::stroke(&st.lane(ck(color_branch)), st.arrow_width()),
                format!("arrow arrow{}", ck(color_branch)),
            ));
        }
    }

    // Commit bullets, labels and tags.
    let mut bullet_els = Vec::new();
    let (tag_bkg, tag_border) = if st.neo_gen() {
        (pal.main_bkg.clone(), pal.node_border.clone())
    } else {
        (pal.label_box_bkg.clone(), pal.label_box_border.clone())
    };
    let mut label_els = Vec::new();
    // `.commit-merge` / `-reverse` / `-highlight-inner`: `mainBkg` under the neo colour
    // generator, `primaryColor` (the same colour in the classic themes) otherwise.
    let primary = pal.main_bkg.clone();
    // Bullet, merge-hole and cherry-pick dot radii, reverse-cross half size, highlight
    // outer / inner sizes (`useReduxGeometry`).
    let (bullet_r, hole_r, dot_r, cross, (hi_out, hi_in)) = if st.redux {
        (7.0, 5.0, 2.5, 4.0, (14.0, 8.0))
    } else {
        (10.0, 6.0, 2.75, 5.0, (20.0, 12.0))
    };
    for (ci, c) in g.commits.iter().enumerate() {
        let p = pt(ci);
        let k = ck(c.branch);
        let (col, col_fill) = (st.lane(k), st.commit_fill(k));
        bounds.add_rect(Rect::centered(p, 24.0, 24.0));
        let cls = format!("commit {} commit{k}", c.id);
        let symbol = c.custom_type.unwrap_or(c.kind);
        match symbol {
            CommitType::Highlight => {
                // `.commit-highlight{k}`.
                let (hf, hs) = st.highlight(k);
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::centered(p, hi_out, hi_out),
                        rx: 0.0,
                    },
                    Paint::both(&hf, &hs, 1.0),
                    format!("{cls} commit-highlight-outer"),
                ));
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::centered(p, hi_in, hi_in),
                        rx: 0.0,
                    },
                    Paint::both(&primary, &primary, 1.0),
                    format!("{cls} commit-highlight-inner"),
                ));
            }
            CommitType::CherryPick => {
                // Classed `commit commit-cherry-pick` only (no `commit{k}`): no stroke,
                // and an inherited fill.
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
                    bullet_els.push(Element::line(
                        Point::new(p.x + dx, p.y + 1.0),
                        Point::new(p.x, p.y - 5.0),
                        Paint::stroke(&mark, 1.0),
                    ));
                }
            }
            _ => {
                let r = if c.kind == CommitType::Merge {
                    bullet_r - 1.0
                } else {
                    bullet_r
                };
                bullet_els.push(Element::classed_shape(
                    Geometry::Circle { c: p, r },
                    Paint::both(&col_fill, &col, 1.0),
                    cls.clone(),
                ));
                if symbol == CommitType::Merge {
                    bullet_els.push(Element::classed_shape(
                        Geometry::Circle { c: p, r: hole_r },
                        Paint::both(&primary, &primary, 1.0),
                        format!("{cls} commit-merge"),
                    ));
                }
                if symbol == CommitType::Reverse {
                    bullet_els.push(Element::classed_shape(
                        Geometry::Path(vec![
                            PathCmd::MoveTo(Point::new(p.x - cross, p.y - cross)),
                            PathCmd::LineTo(Point::new(p.x + cross, p.y + cross)),
                            PathCmd::MoveTo(Point::new(p.x - cross, p.y + cross)),
                            PathCmd::LineTo(Point::new(p.x + cross, p.y - cross)),
                        ]),
                        Paint::stroke(&primary, if st.neo_gen() { st.stroke_width() } else { 3.0 }),
                        format!("{cls} commit-reverse"),
                    ));
                }
            }
        }
        if shown_label(c, gc) {
            let (rect, text, rotation) = commit_label_geometry(o, p, &c.id, gc.rotate_commit_label);
            // Neo colour generator: transparent box, `nodeBorder` text (weight 600
            // under redux geometry).
            let bkg = Element::classed_shape(
                Geometry::Rect { rect, rx: 0.0 },
                Paint::fill(&primary).with_opacity(if st.neo_gen() { 0.0 } else { 0.5 }),
                "commit-label-bkg",
            );
            let txt = Element::classed_text(
                Text {
                    fill: if st.neo_gen() {
                        pal.node_border.clone()
                    } else {
                        pal.text_color.clone()
                    },
                    bold: st.redux,
                    ..text
                },
                "commit-label",
            );
            match rotation {
                Some((about, deg)) => {
                    bounds.add_rotated_rect(rect, about, deg);
                    label_els.push(Element::Rotated {
                        deg,
                        about,
                        children: vec![bkg, txt],
                    });
                }
                None => {
                    bounds.add_rect(rect);
                    label_els.push(bkg);
                    label_els.push(txt);
                }
            }
        }
        // Tags: newest first, stacked away from the commit.
        let tag_w = c
            .tags
            .iter()
            .map(|t| line_width(t, TAG_FONT, false))
            .fold(0.0, f64::max);
        for (k, t) in c.tags.iter().rev().enumerate() {
            let h2 = TAG_FONT * 1.2 / 2.0;
            if vertical {
                // mermaid's `drawCommitTags` (TB/BT): the tag is laid out to the
                // right of the commit, then turned +45° about (x, pos) where
                // pos = along - LAYOUT_OFFSET, after a (12,12) / (14,14) shift.
                let pos = p.y - LAYOUT_OFFSET;
                let y = pos + k as f64 * 20.0;
                let poly: Vec<Point> = [
                    (p.x, y + 2.0),
                    (p.x, y - 2.0),
                    (p.x + LAYOUT_OFFSET, y - h2 - 2.0),
                    (p.x + LAYOUT_OFFSET + tag_w + 4.0, y - h2 - 2.0),
                    (p.x + LAYOUT_OFFSET + tag_w + 4.0, y + h2 + 2.0),
                    (p.x + LAYOUT_OFFSET, y + h2 + 2.0),
                ]
                .iter()
                .map(|&(x, y)| Point::new(x + 12.0, y + 12.0))
                .collect();
                let about = Point::new(p.x + 12.0, pos + 12.0);
                for q in &poly {
                    bounds.add_point(q.rotated(about, 45.0));
                }
                label_els.push(Element::Rotated {
                    deg: 45.0,
                    about,
                    children: vec![
                        Element::classed_shape(
                            Geometry::Path(geom::polygon(&poly)),
                            Paint::both(&tag_bkg, &tag_border, 1.0),
                            "tag-label-bkg",
                        ),
                        Element::classed_shape(
                            Geometry::Circle {
                                c: Point::new(p.x + 2.0 + 12.0, y + 12.0),
                                r: 1.5,
                            },
                            Paint::fill(&pal.text_color),
                            "tag-hole",
                        ),
                    ],
                });
                label_els.push(Element::Rotated {
                    deg: 45.0,
                    about: Point::new(p.x + 14.0, pos + 14.0),
                    children: vec![Element::classed_text(
                        Text {
                            x: p.x + 5.0 + 14.0,
                            y: y + 3.0 + 14.0,
                            lines: vec![t.clone()],
                            line_height: TAG_FONT * 1.2,
                            font_size: TAG_FONT,
                            bold: false,
                            anchor: Anchor::Start,
                            fill: pal.label_text.clone(),
                        },
                        "tag-label",
                    )],
                });
                continue;
            }
            let (poly, hole, center) = {
                let ly = p.y - 19.2 - k as f64 * 20.0;
                let left = p.x - tag_w / 2.0 - 4.0;
                let poly = vec![
                    Point::new(left - 8.0, ly + 2.0),
                    Point::new(left - 8.0, ly - 2.0),
                    Point::new(left, ly - h2 - 2.0),
                    Point::new(p.x + tag_w / 2.0 + 4.0, ly - h2 - 2.0),
                    Point::new(p.x + tag_w / 2.0 + 4.0, ly + h2 + 2.0),
                    Point::new(left, ly + h2 + 2.0),
                ];
                (poly, Point::new(left - 4.0, ly), Point::new(p.x, ly))
            };
            for q in &poly {
                bounds.add_point(*q);
            }
            label_els.push(Element::classed_shape(
                Geometry::Path(geom::polygon(&poly)),
                Paint::both(&tag_bkg, &tag_border, 1.0),
                "tag-label-bkg",
            ));
            label_els.push(Element::classed_shape(
                Geometry::Circle { c: hole, r: 1.5 },
                Paint::fill(&pal.text_color),
                "tag-hole",
            ));
            label_els.push(Element::classed_text(
                Text::centered(
                    center,
                    std::slice::from_ref(t),
                    TAG_FONT,
                    TAG_FONT * 1.2,
                    &pal.label_text,
                ),
                "tag-label",
            ));
        }
    }

    let mut els = vec![
        Element::group("branches", branch_els),
        Element::group("commit-arrows", arrow_els),
        Element::group("commit-bullets", bullet_els),
        Element::group("commit-labels", label_els),
    ];
    if let Some(t) = g.title.as_ref().filter(|t| !t.is_empty()) {
        let r = bounds.rect();
        let c = Point::new(r.center().x, r.y - gc.title_top_margin);
        let w = line_width(t, 18.0, false);
        bounds.add_rect(Rect::centered(c, w, 22.0));
        els.push(Element::classed_text(
            Text::centered(c, std::slice::from_ref(t), 18.0, 22.0, &pal.text_color),
            "gitTitleText",
        ));
    }
    let _ = Anchor::Middle;
    Scene {
        kind: DiagramKind::GitGraph,
        view_box: bounds.rect().pad(gc.diagram_padding.max(8.0)),
        elements: els,
        acc_title: None,
        acc_descr: None,
        font_family: pal.font_family.clone(),
        css: String::new(),
    }
}

/// mermaid's `findLane`: a free coordinate between two lanes for rerouted arrows.
fn find_lane(lanes: &mut Vec<f64>, a: f64, b: f64) -> f64 {
    let mut hi = b;
    for _ in 0..6 {
        let cand = a + (hi - a).abs() / 2.0;
        if lanes.iter().all(|l| (l - cand).abs() >= 10.0) {
            lanes.push(cand);
            return cand;
        }
        hi -= (hi - a).abs() / 5.0;
    }
    a + (b - a) / 2.0
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
        let l = layout(&g, &gc);
        for w in l.pos.windows(2) {
            assert!(w[1] > w[0]);
        }
        assert!(l.lane[1] > l.lane[0] && l.lane[2] > l.lane[1]);
        let gp = GitConfig {
            parallel_commits: true,
            ..GitConfig::default()
        };
        let lp = layout(&g, &gp);
        // In parallel mode a commit is one step after its latest parent.
        assert!(lp.pos[3] > lp.pos[2]);
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
        let l = layout(&g, gc);
        draw(
            &g,
            &l,
            gc,
            &crate::theme::palette(crate::theme::ThemeName::Default, &[]),
        )
    }

    #[test]
    fn commit_labels_rotate_by_default() {
        let src = "gitGraph\n  commit id: \"abc\"\n  commit id: \"defgh\"";
        let s = scene_for(src, &GitConfig::default());
        let rot = rotations(&s.elements);
        assert_eq!(rot.len(), 2);
        // Background and text turn together by -45°.
        assert!(rot.iter().all(|(d, _, n)| *d == -45.0 && *n == 2));
        // LR: about (pos, y) shifted by mermaid's (r_x, r_y).
        let w = line_width("abc", LABEL_FONT, false);
        let (dx, dy) = (-7.5 - (w + 10.0) / 25.0 * 9.5, 10.0 + w / 25.0 * 8.5);
        assert!((rot[0].1.x - (0.0 + dx)).abs() < 1e-9, "{rot:?}");
        assert!((rot[0].1.y - dy).abs() < 1e-9);
        // Lanes get mermaid's 40px of extra room.
        let g = parse(
            "gitGraph\ncommit\nbranch b\ncommit",
            None,
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(layout(&g, &GitConfig::default()).lane, vec![0.0, 90.0]);
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
        assert_eq!(layout(&g, &gc).lane, vec![0.0, 50.0]);
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
        assert_eq!(layout(&g, &GitConfig::default()).pos, vec![90.0, 40.0]);
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
}
