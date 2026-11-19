//! Each diagram's `styles.ts` (the `diagramStyles` part of `getStyles`),
//! transcribed from mermaid 12.3.0. See [`super::styles`].

use super::khroma;
use super::styles::Opts;
use super::tpl::tpl;
use crate::scene::DiagramKind;

fn t(o: &Opts, template: &str) -> String {
    tpl(o, template, &|_| None)
}

/// The `limit` of `THEME_COLOR_LIMIT` loops.
fn limit(o: &Opts) -> usize {
    o.get("THEME_COLOR_LIMIT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12)
}

/// `lighten` or `darken` by `n` points depending on the theme's `darkMode`.
fn adjuster(o: &Opts, color: &str, n: f64) -> String {
    if o.get("darkMode").is_some_and(|v| v == "true") {
        khroma::darken(color, n)
    } else {
        khroma::lighten(color, n)
    }
}

/// The diagram's own styles; `None` where this port has no transcription.
pub(super) fn diagram(kind: DiagramKind, o: &Opts) -> Option<String> {
    Some(match kind {
        DiagramKind::Pie => pie(o),
        DiagramKind::Sankey => t(o, SANKEY),
        DiagramKind::Venn => t(o, VENN),
        DiagramKind::Packet => PACKET.to_string(),
        DiagramKind::Architecture => t(o, ARCHITECTURE),
        DiagramKind::Ishikawa => t(o, ISHIKAWA),
        DiagramKind::EventModeling => String::new(),
        DiagramKind::TreeView => TREE_VIEW.to_string(),
        DiagramKind::Treemap => treemap(o),
        DiagramKind::Radar => radar(o),
        DiagramKind::Cynefin => cynefin(o),
        DiagramKind::Journey => journey(o),
        DiagramKind::Kanban => kanban(o),
        // No styles.ts: mermaid logs "No theme found" and adds nothing.
        DiagramKind::Quadrant | DiagramKind::XyChart | DiagramKind::Info => String::new(),
        _ => return None,
    })
}

fn pie(o: &Opts) -> String {
    let ff = o.s("fontFamily");
    let p = o.pal();
    let or = |k: &str, d: String| o.get(k).map_or(d, str::to_string);
    format!(
        ".pieCircle{{ stroke: {}; stroke-width : {}; opacity : {}; }}\n\
         .pieCircle.highlighted{{ scale: 1.05; opacity: 1; }}\n\
         .pieCircle.highlightedOnHover:hover{{ transition-duration: 250ms; scale: 1.05; opacity: 1; }}\n\
         .pieOuterCircle{{ stroke: {}; stroke-width: {}; fill: none; }}\n\
         .pieTitleText {{ text-anchor: middle; font-size: {}; fill: {}; font-family: {ff}; }}\n\
         .slice {{ font-family: {ff}; fill: {}; font-size:{}; }}\n\
         .legend text {{ fill: {}; font-family: {ff}; font-size: {}; }}\n",
        o.s("pieStrokeColor"),
        or("pieStrokeWidth", Opts::px(p.pie_stroke_width)),
        or("pieOpacity", super::styles::js_num(p.pie_opacity)),
        o.s("pieOuterStrokeColor"),
        or("pieOuterStrokeWidth", Opts::px(p.pie_outer_stroke_width)),
        or("pieTitleTextSize", Opts::px(p.pie_title_text_size)),
        o.s("pieTitleTextColor"),
        o.s("pieSectionTextColor"),
        or("pieSectionTextSize", Opts::px(p.pie_section_text_size)),
        o.s("pieLegendTextColor"),
        or("pieLegendTextSize", Opts::px(p.pie_legend_text_size)),
    )
}

const SANKEY: &str = r#"
.label { font-family: ${options.fontFamily}; }
.node-labels { font-family: ${options.fontFamily}; }
.sankey-label-bg {
  stroke: ${options.mainBkg || options.background || '#fff'};
  stroke-width: 4px;
  stroke-linejoin: round;
  paint-order: stroke;
}
.sankey-label-fg { fill: ${options.textColor}; }
.node rect { shape-rendering: crispEdges; }
.link { fill: none; stroke-opacity: 0.5; mix-blend-mode: multiply; }
"#;

const VENN: &str = r#"
.venn-title { font-size: 32px; fill: ${options.vennTitleTextColor}; font-family: ${options.fontFamily}; }
.venn-circle text { font-size: 48px; font-family: ${options.fontFamily}; }
.venn-intersection text { font-size: 48px; fill: ${options.vennSetTextColor}; font-family: ${options.fontFamily}; }
.venn-text-node { font-family: ${options.fontFamily}; color: ${options.vennSetTextColor}; }
"#;

const PACKET: &str = r#"
.packetByte { font-size: 10px; }
.packetByte.start { fill: black; }
.packetByte.end { fill: black; }
.packetLabel { fill: black; font-size: 12px; }
.packetTitle { fill: black; font-size: 14px; }
.packetBlock { stroke: black; stroke-width: 1; fill: #efefef; }
"#;

const ARCHITECTURE: &str = r#"
.edge { stroke-width: ${options.archEdgeWidth}; stroke: ${options.archEdgeColor}; fill: none; }
.arrow { fill: ${options.archEdgeArrowColor}; }
.node-bkg { fill: none; stroke: ${options.archGroupBorderColor}; stroke-width: ${options.archGroupBorderWidth}; stroke-dasharray: 8; }
.node-icon-text { display: flex; align-items: center; }
.node-icon-text > div {
  color: #fff;
  margin: 1px;
  height: fit-content;
  text-align: center;
  overflow: hidden;
  display: -webkit-box;
  -webkit-box-orient: vertical;
}
"#;

const ISHIKAWA: &str = r#"
.ishikawa .ishikawa-spine, .ishikawa .ishikawa-branch, .ishikawa .ishikawa-sub-branch {
  stroke: ${options.lineColor}; stroke-width: 2; fill: none;
}
.ishikawa .ishikawa-sub-branch { stroke-width: 1; }
.ishikawa .ishikawa-arrow { fill: ${options.lineColor}; }
.ishikawa .ishikawa-head { fill: ${options.mainBkg}; stroke: ${options.lineColor}; stroke-width: 2; }
.ishikawa .ishikawa-label-box { fill: ${options.mainBkg}; stroke: ${options.lineColor}; stroke-width: 2; }
.ishikawa text { font-family: ${options.fontFamily}; font-size: ${options.fontSize}; fill: ${options.textColor}; }
.ishikawa .ishikawa-head-label { font-weight: 600; text-anchor: middle; dominant-baseline: middle; font-size: 14px; }
.ishikawa .ishikawa-label { text-anchor: end; }
.ishikawa .ishikawa-label.cause { text-anchor: middle; dominant-baseline: middle; }
.ishikawa .ishikawa-label.align { text-anchor: end; dominant-baseline: middle; }
.ishikawa .ishikawa-label.up { dominant-baseline: baseline; }
.ishikawa .ishikawa-label.down { dominant-baseline: hanging; }
"#;

const TREE_VIEW: &str = r#"
.treeView-node-label { font-size: 16px; fill: black; white-space: pre; }
.treeView-node-dir { font-weight: bold; }
.treeView-node-line { stroke: black; }
.treeView-node-icon { color: #546e7a; }
.treeView-node-description { font-size: 16px; fill: #6a9955; font-style: italic; white-space: pre; }
.treeView-highlight-bg { fill: rgba(255, 193, 7, 0.15); stroke: #ffc107; stroke-width: 1; }
"#;

fn treemap(o: &Opts) -> String {
    t(
        o,
        r#"
.treemapNode.section { stroke: black; stroke-width: 1; fill: #efefef; }
.treemapNode.leaf { stroke: black; stroke-width: 1; fill: #efefef; }
.treemapLabel { fill: ${options.textColor}; font-size: 12px; }
.treemapValue { fill: ${options.textColor}; font-size: 10px; }
.treemapTitle { fill: ${options.titleColor}; font-size: 14px; }
"#,
    )
}

fn radar(o: &Opts) -> String {
    let r = |k: &str| o.s(&format!("radar.{k}"));
    let mut s = format!(
        ".radarTitle {{ font-size: {}; color: {}; dominant-baseline: hanging; text-anchor: middle; }}\n\
         .radarAxisLine {{ stroke: {}; stroke-width: {}; }}\n\
         .radarAxisLabel {{ font-size: {}px; color: {}; }}\n\
         .radarGraticule {{ fill: {}; fill-opacity: {}; stroke: {}; stroke-width: {}; }}\n\
         .radarLegendText {{ text-anchor: start; font-size: {}px; dominant-baseline: hanging; }}\n",
        o.s("fontSize"),
        o.s("titleColor"),
        r("axisColor"),
        r("axisStrokeWidth"),
        r("axisLabelFontSize"),
        r("axisColor"),
        r("graticuleColor"),
        r("graticuleOpacity"),
        r("graticuleColor"),
        r("graticuleStrokeWidth"),
        r("legendFontSize"),
    );
    for i in 0..limit(o) {
        let c = o.s(&format!("cScale{i}"));
        s += &format!(
            ".radarCurve-{i} {{ color: {c}; fill: {c}; fill-opacity: {}; stroke: {c}; stroke-width: {}; }}\n\
             .radarLegendBox-{i} {{ fill: {c}; fill-opacity: {}; stroke: {c}; }}\n",
            r("curveOpacity"),
            r("curveStrokeWidth"),
            r("curveOpacity"),
        );
    }
    s
}

fn cynefin(o: &Opts) -> String {
    let c = |k: &str| o.s(&format!("cynefin.{k}"));
    let n = |k: &str| c(k).parse::<f64>().unwrap_or(f64::NAN);
    let js = super::styles::js_num;
    format!(
        ".cynefinDomain {{ stroke: none; }}\n\
         .cynefinDomainLabel {{ font-size: {}px; font-weight: bold; fill: {}; }}\n\
         .cynefinSubtitle {{ font-size: {}px; fill: {}; font-style: italic; }}\n\
         .cynefinItem {{ fill-opacity: 0.95; stroke: {}; stroke-width: 1; }}\n\
         .cynefinItemText {{ font-size: {}px; fill: {}; }}\n\
         .cynefinItemOverflow {{ fill-opacity: 0.6; stroke: {}; stroke-width: 1; stroke-dasharray: 3 2; }}\n\
         .cynefinBoundary {{ stroke: {}; stroke-width: {}; stroke-dasharray: 6 3; }}\n\
         .cynefinCliff {{ stroke: {}; stroke-width: {}; }}\n\
         .cynefinConfusion {{ stroke: {}; stroke-width: 1.5; stroke-dasharray: 4 2; }}\n\
         .cynefinArrowLine {{ stroke: {}; stroke-width: {}; fill: none; }}\n\
         .cynefinArrowHead {{ fill: {}; stroke: none; }}\n\
         .cynefinArrowLabel {{ font-size: {}px; fill: {}; }}\n\
         .cynefinTitle {{ font-size: {}px; font-weight: bold; fill: {}; }}\n",
        c("domainFontSize"),
        c("labelColor"),
        js(n("itemFontSize") - 1.0),
        c("textColor"),
        c("boundaryColor"),
        c("itemFontSize"),
        c("textColor"),
        c("boundaryColor"),
        c("boundaryColor"),
        c("boundaryWidth"),
        c("cliffColor"),
        c("cliffWidth"),
        c("boundaryColor"),
        c("arrowColor"),
        c("arrowWidth"),
        c("arrowColor"),
        js(n("itemFontSize") - 1.0),
        c("textColor"),
        js(n("domainFontSize") + 2.0),
        c("labelColor"),
    )
}

/// `getIconStyles()`.
const ICON_STYLES: &str = r#"
  .label-icon { display: inline-block; height: 1em; overflow: visible; vertical-align: -0.125em; }
  .node .label-icon path { fill: currentColor; stroke: revert; stroke-width: revert; }
"#;

fn journey(o: &Opts) -> String {
    let fill_type = |i: usize| -> String {
        if o.get("fillType0").is_some() {
            format!("fill: {}", o.s(&format!("fillType{i}")))
        } else {
            String::new()
        }
    };
    let actor = |i: usize| -> String {
        match o.get(&format!("actor{i}")) {
            Some(v) if !v.is_empty() => format!("fill: {v}"),
            _ => String::new(),
        }
    };
    let face = match o.get("faceColor") {
        Some(v) if !v.is_empty() => format!("fill: {v}"),
        _ => "fill: #FFF8DC".to_string(),
    };
    let mut s = t(
        o,
        r#"
.label { font-family: ${options.fontFamily}; color: ${options.textColor}; }
.mouth { stroke: #666; }
line { stroke: ${options.textColor} }
.legend { fill: ${options.textColor}; font-family: ${options.fontFamily}; }
.label text { fill: #333; }
.label { color: ${options.textColor} }
.face { @FACE; stroke: #999; }
.node rect, .node circle, .node ellipse, .node polygon, .node path {
  fill: ${options.mainBkg}; stroke: ${options.nodeBorder}; stroke-width: 1px;
}
.node .label { text-align: center; }
.node.clickable { cursor: pointer; }
.arrowheadPath { fill: ${options.arrowheadColor}; }
.edgePaths .path { stroke: ${options.lineColor}; stroke-width: 1.5px; }
.flowchart-link { stroke: ${options.lineColor}; fill: none; }
.edgeLabel {
  background-color: ${options.edgeLabelBackground};
  rect { opacity: 0.5; }
  text-align: center;
}
.cluster rect { }
.cluster text { fill: ${options.titleColor}; }
div.mermaidTooltip {
  position: absolute; text-align: center; max-width: 200px; padding: 2px;
  font-family: ${options.fontFamily}; font-size: 12px;
  background: ${options.tertiaryColor}; border: 1px solid ${options.border2};
  border-radius: 2px; pointer-events: none; z-index: 100;
}
"#,
    )
    .replace("@FACE", &face);
    for i in 0..8 {
        s += &format!(".task-type-{i}, .section-type-{i}  {{ {}; }}\n", fill_type(i));
    }
    for i in 0..6 {
        s += &format!(".actor-{i} {{ {}; }}\n", actor(i));
    }
    s + ICON_STYLES
}

fn kanban(o: &Opts) -> String {
    let mut s = String::from(".edge { stroke-width: 3; }\n");
    for i in 0..limit(o) {
        let j = i as i64 - 1;
        let sw = 17 - 3 * i as i64;
        let scale = o.s(&format!("cScale{i}"));
        let adj = adjuster(o, &scale, 10.0);
        s += &format!(
            ".section-{j} rect, .section-{j} path, .section-{j} circle, .section-{j} polygon, .section-{j} path {{ fill: {adj}; stroke: {adj}; }}\n\
             .section-{j} text {{ fill: {}; }}\n\
             .node-icon-{j} {{ font-size: 40px; color: {}; }}\n\
             .section-edge-{j}{{ stroke: {scale}; }}\n\
             .edge-depth-{j}{{ stroke-width: {sw}; }}\n\
             .section-{j} line {{ stroke: {} ; stroke-width: 3; }}\n\
             .disabled, .disabled circle, .disabled text {{ fill: lightgray; }}\n\
             .disabled text {{ fill: #efefef; }}\n\
             .node rect, .node circle, .node ellipse, .node polygon, .node path {{ fill: {}; stroke: {}; stroke-width: 1px; }}\n\
             .kanban-ticket-link {{ fill: {}; stroke: {}; text-decoration: underline; }}\n",
            o.s(&format!("cScaleLabel{i}")),
            o.s(&format!("cScaleLabel{i}")),
            o.s(&format!("cScaleInv{i}")),
            o.s("background"),
            o.s("nodeBorder"),
            o.s("background"),
            o.s("nodeBorder"),
        );
    }
    s += &t(
        o,
        r#"
.section-root rect, .section-root path, .section-root circle, .section-root polygon { fill: ${options.git0}; }
.section-root text { fill: ${options.gitBranchLabel0}; }
.icon-container { height:100%; display: flex; justify-content: center; align-items: center; }
.edge { fill: none; }
.cluster-label, .label { color: ${options.textColor}; fill: ${options.textColor}; }
.kanban-label {
  dy: 1em; alignment-baseline: middle; text-anchor: middle; dominant-baseline: middle; text-align: center;
}
"#,
    );
    s + ICON_STYLES
}
