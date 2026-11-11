//! `gitGraph` diagrams.
//!
//! Supported statements: `commit` (`id:`, `msg:`, `tag:` (repeatable),
//! `type: NORMAL|REVERSE|HIGHLIGHT`), `branch name [order: n]`,
//! `checkout name` / `switch name`, `merge name` (`id:`, `tag:`, `type:`),
//! `cherry-pick id: "x"` (`parent:`, `tag:`), and the orientation header
//! `gitGraph LR:` / `TB:` / `BT:`.
//! Config (`gitGraph` section): `showBranches`, `showCommitLabel`,
//! `mainBranchName`, `mainBranchOrder`, `parallelCommits`.
//!
//! **Commit ids.** mermaid gives commits without an `id:` a random id
//! `<seq>-<7 random hex>`. To keep rendering pure and reproducible we use
//! `<seq>-<7 hex digits of FNV-1a(seq, branch, message)>` instead: same shape,
//! stable across runs.
//!
//! Commit labels are drawn horizontally (this renderer has no text rotation,
//! so mermaid's default `rotateCommitLabel` is not applied); the commit step
//! grows when labels are wider than mermaid's 50px spacing so they never overlap.

use crate::color::{self, Rgba};
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
    pub title_top_margin: f64,
    pub diagram_padding: f64,
}

impl GitConfig {
    pub fn from_config(cfg: &MermaidConfig) -> Self {
        GitConfig {
            show_branches: cfg.section_bool("gitGraph", "showBranches", true),
            show_commit_label: cfg.section_bool("gitGraph", "showCommitLabel", true),
            parallel_commits: cfg.section_bool("gitGraph", "parallelCommits", false),
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
    let max_tags = g.commits.iter().map(|c| c.tags.len()).max().unwrap_or(0) as f64;
    let mut p = 0.0;
    for (idx, &b) in order.iter().enumerate() {
        lane[b] = p;
        lane_index[b] = idx;
        let label_w = line_width(&g.branches[b].name, BRANCH_FONT, false);
        p += if vertical {
            // Labels sit left of commits, tags to the right.
            let lw = g
                .commits
                .iter()
                .filter(|c| c.branch == b && shown_label(c, gc))
                .map(|c| line_width(&c.id, LABEL_FONT, false) + 26.0)
                .fold(0.0, f64::max);
            (50.0 + label_w / 2.0).max(lw + 40.0)
        } else {
            50.0 + if gc.show_commit_label { 15.0 } else { 0.0 } + 20.0 * max_tags
        };
    }
    // Commit step: mermaid uses 50; widen if horizontal labels/tags would collide.
    let step = if vertical {
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
            *p = max_pos + start - *p;
        }
    }
    Layout {
        lane,
        lane_index,
        pos,
        max_pos,
    }
}

fn to_xy(o: Orientation, along: f64, across: f64) -> Point {
    match o {
        Orientation::LR => Point::new(along, across),
        _ => Point::new(across, along),
    }
}

/// Saturated branch colour derived from the theme's section scale. Lanes step
/// five scale entries at a time (150° of hue) so neighbouring lanes contrast.
///
/// The classic themes define `git0..7`, which upstream's `normalTheme` styles use
/// (`.commit{i}` / `.arrow{i}` / `.label{i}` = `git{i % 8}`).
fn git_color(pal: &Palette, i: usize) -> Color {
    if !pal.git.is_empty() {
        return pal.git[i % pal.git.len()].clone();
    }
    let base = pal.scale_color(i * 5 % 12 + i / 12);
    match color::parse(base.css()) {
        Some(rgb) => {
            let (h, s, _) = rgb.to_hsl();
            let l = if pal.dark { 55.0 } else { 45.0 };
            Color::new(Rgba::from_hsl(h, s.max(70.0), l, 1.0).to_hex())
        }
        None => base.clone(),
    }
}

/// `gitBranchLabel{i % 8}`, or a readable colour on `fill` without the git set.
fn branch_label_color(pal: &Palette, i: usize, fill: &Color) -> Color {
    match pal.git_branch_label.get(i % 8) {
        Some(c) => c.clone(),
        None => readable_on(fill),
    }
}

fn readable_on(c: &Color) -> Color {
    match color::parse(c.css()) {
        Some(rgb) if rgb.is_dark() => Color::new("#ffffff"),
        _ => Color::new("#000000"),
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
    let color_of = |b: usize| git_color(pal, lay.lane_index[b]);
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
                Paint::stroke(&pal.line_color, 1.0).dashed(vec![2.0]),
                format!("branch branch{}", lay.lane_index[b] % 8),
            ));
            let tb = TextBlock::new(vec![br.name.clone()], BRANCH_FONT, false);
            let (w, h) = (tb.width + 18.0, tb.height + 4.0);
            let rect = match o {
                Orientation::LR => Rect::new(-w - 5.0 - 14.0, across - h / 2.0, w, h),
                Orientation::TB => Rect::new(across - w / 2.0, lo - h - 10.0, w, h),
                Orientation::BT => Rect::new(across - w / 2.0, hi + 10.0, w, h),
            };
            bounds.add_rect(rect);
            let fill = color_of(b);
            branch_els.push(Element::classed_shape(
                Geometry::Rect { rect, rx: 4.0 },
                Paint::fill(&fill),
                format!("branchLabelBkg label{}", lay.lane_index[b] % 8),
            ));
            branch_els.push(Element::classed_text(
                Text::centered(
                    rect.center(),
                    &tb.lines,
                    BRANCH_FONT,
                    tb.line_height,
                    &branch_label_color(pal, lay.lane_index[b], &fill),
                ),
                format!("branch-label{}", lay.lane_index[b] % 8),
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
                Paint::stroke(&color_of(color_branch), 8.0),
                format!("arrow arrow{}", lay.lane_index[color_branch] % 8),
            ));
        }
    }

    // Commit bullets, labels and tags.
    let mut bullet_els = Vec::new();
    let mut label_els = Vec::new();
    let primary = pal.main_bkg.clone();
    for (ci, c) in g.commits.iter().enumerate() {
        let p = pt(ci);
        let col = color_of(c.branch);
        bounds.add_rect(Rect::centered(p, 24.0, 24.0));
        let cls = format!("commit {} commit{}", c.id, lay.lane_index[c.branch] % 8);
        let symbol = c.custom_type.unwrap_or(c.kind);
        match symbol {
            CommitType::Highlight => {
                // `.commit-highlight{i}`: `gitInv{i % 8}`.
                let inv = match pal.git_inv.get(lay.lane_index[c.branch] % 8) {
                    Some(c) => c.clone(),
                    None => color::parse(col.css())
                        .map(|r| Color::new(r.invert().to_hex()))
                        .unwrap_or_else(|| col.clone()),
                };
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::centered(p, 20.0, 20.0),
                        rx: 0.0,
                    },
                    Paint::both(&inv, &inv, 1.0),
                    format!("{cls} commit-highlight-outer"),
                ));
                bullet_els.push(Element::classed_shape(
                    Geometry::Rect {
                        rect: Rect::centered(p, 12.0, 12.0),
                        rx: 0.0,
                    },
                    Paint::both(&primary, &primary, 1.0),
                    format!("{cls} commit-highlight-inner"),
                ));
            }
            CommitType::CherryPick => {
                bullet_els.push(Element::classed_shape(
                    Geometry::Circle { c: p, r: 10.0 },
                    Paint::both(&col, &col, 1.0),
                    format!("{cls} commit-cherry-pick"),
                ));
                let white = Color::new("#fff");
                for dx in [-3.0, 3.0] {
                    bullet_els.push(Element::shape(
                        Geometry::Circle {
                            c: Point::new(p.x + dx, p.y + 2.0),
                            r: 2.75,
                        },
                        Paint::fill(&white),
                    ));
                    bullet_els.push(Element::line(
                        Point::new(p.x + dx, p.y + 1.0),
                        Point::new(p.x, p.y - 5.0),
                        Paint::stroke(&white, 1.0),
                    ));
                }
            }
            _ => {
                let r = if c.kind == CommitType::Merge {
                    9.0
                } else {
                    10.0
                };
                bullet_els.push(Element::classed_shape(
                    Geometry::Circle { c: p, r },
                    Paint::both(&col, &col, 1.0),
                    cls.clone(),
                ));
                if symbol == CommitType::Merge {
                    bullet_els.push(Element::classed_shape(
                        Geometry::Circle { c: p, r: 6.0 },
                        Paint::both(&primary, &primary, 1.0),
                        format!("{cls} commit-merge"),
                    ));
                }
                if symbol == CommitType::Reverse {
                    bullet_els.push(Element::classed_shape(
                        Geometry::Path(vec![
                            PathCmd::MoveTo(Point::new(p.x - 5.0, p.y - 5.0)),
                            PathCmd::LineTo(Point::new(p.x + 5.0, p.y + 5.0)),
                            PathCmd::MoveTo(Point::new(p.x - 5.0, p.y + 5.0)),
                            PathCmd::LineTo(Point::new(p.x + 5.0, p.y - 5.0)),
                        ]),
                        Paint::stroke(&primary, 3.0),
                        format!("{cls} commit-reverse"),
                    ));
                }
            }
        }
        if shown_label(c, gc) {
            let w = line_width(&c.id, LABEL_FONT, false);
            let h = LABEL_FONT * 1.2;
            let rect = if vertical {
                Rect::new(
                    p.x - w - 4.0 * 4.0 - 7.0,
                    p.y - h / 2.0 - 2.0,
                    w + 4.0,
                    h + 4.0,
                )
            } else {
                Rect::new(p.x - w / 2.0 - 2.0, p.y + 13.5, w + 4.0, h + 4.0)
            };
            bounds.add_rect(rect);
            label_els.push(Element::classed_shape(
                Geometry::Rect { rect, rx: 0.0 },
                Paint::fill(&primary).with_opacity(0.5),
                "commit-label-bkg",
            ));
            label_els.push(Element::classed_text(
                Text::centered(
                    rect.center(),
                    std::slice::from_ref(&c.id),
                    LABEL_FONT,
                    h,
                    &pal.text_color,
                ),
                "commit-label",
            ));
        }
        // Tags: newest first, stacked away from the commit.
        let tag_w = c
            .tags
            .iter()
            .map(|t| line_width(t, TAG_FONT, false))
            .fold(0.0, f64::max);
        for (k, t) in c.tags.iter().rev().enumerate() {
            let h2 = TAG_FONT * 1.2 / 2.0;
            let (poly, hole, center) = if vertical {
                let y = p.y + k as f64 * 20.0;
                let x0 = p.x + 14.0;
                let poly = vec![
                    Point::new(x0, y - 2.0),
                    Point::new(x0 + 8.0, y - h2 - 2.0),
                    Point::new(x0 + 8.0 + tag_w + 8.0, y - h2 - 2.0),
                    Point::new(x0 + 8.0 + tag_w + 8.0, y + h2 + 2.0),
                    Point::new(x0 + 8.0, y + h2 + 2.0),
                    Point::new(x0, y + 2.0),
                ];
                (
                    poly,
                    Point::new(x0 + 4.0, y),
                    Point::new(x0 + 12.0 + tag_w / 2.0, y),
                )
            } else {
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
                Paint::both(&pal.label_box_bkg, &pal.label_box_border, 1.0),
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
            Text::centered(c, std::slice::from_ref(t), 18.0, 22.0, &pal.title_color),
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
}
