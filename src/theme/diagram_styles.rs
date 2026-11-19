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
        DiagramKind::Sequence => sequence(o),
        DiagramKind::Mindmap => mindmap(o),
        DiagramKind::Timeline => timeline(o),
        DiagramKind::Er => er(o),
        DiagramKind::Requirement => requirement(o),
        DiagramKind::Class => class(o),
        DiagramKind::Flowchart => flowchart(o),
        DiagramKind::State => state(o),
        DiagramKind::Block => block(o),
        DiagramKind::Gantt => t(o, include_str!("css/gantt.css")),
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

fn sequence(o: &Opts) -> String {
    let shadow = o.get("dropShadow").unwrap_or("none").to_string();
    let neo = o.look == crate::config::Look::Neo;
    tpl(o, include_str!("css/sequence.css"), &|e| match e {
        "@1" | "@2" | "@4" => Some(shadow.clone()),
        "@3" => Some(if neo { shadow.clone() } else { "none".into() }),
        _ => None,
    })
}

/// khroma `fade`.
fn fade(c: &str, a: f64) -> String {
    khroma::fade(c, a)
}

/// `getIconStyles()`.
fn icon_styles() -> String {
    ICON_STYLES.to_string()
}

fn look_name(o: &Opts) -> &'static str {
    match o.look {
        crate::config::Look::Classic => "classic",
        crate::config::Look::Neo => "neo",
        crate::config::Look::HandDrawn => "handDrawn",
    }
}

/// `options.dropShadow ? options.dropShadow.replace('url(#drop-shadow)', ...) : 'none'`.
fn scoped_shadow(o: &Opts) -> String {
    match o.get("dropShadow").filter(|d| !d.is_empty()) {
        Some(d) => d.replace(
            "url(#drop-shadow)",
            &format!("url({}-drop-shadow)", o.svg_sel),
        ),
        None => "none".to_string(),
    }
}

/// A per-palette-slot rule generator (`genColor`): `tmpl` is expanded once per
/// slot of a colour theme's `borderColorArray`, `extra` resolving the
/// template's own expressions; nothing for any other theme.
fn gen_color(
    o: &Opts,
    tmpl: &str,
    extra: &dyn Fn(&str, usize, &str, &str) -> Option<String>,
) -> String {
    let Some((border, bkg)) = o.color_arrays() else {
        return String::new();
    };
    let look = look_name(o);
    let mut out = String::new();
    for i in 0..border.len() {
        let b = &border[i % border.len()];
        let fill = if bkg.is_empty() {
            String::new()
        } else {
            format!("fill: {};", bkg[i % bkg.len()])
        };
        let slot = format!("[data-look=\"{look}\"][data-color-id=\"color-{i}\"]");
        out += &tpl(o, tmpl, &|e| match e {
            "look" => Some(look.to_string()),
            "i" => Some(i.to_string()),
            "borderColor" => Some(b.clone()),
            "fill" | "tint" => Some(fill.clone()),
            "slot" => Some(slot.clone()),
            _ => extra(e, i, b, &fill),
        });
    }
    out
}

fn er(o: &Opts) -> String {
    let neo = o.look == crate::config::Look::Neo;
    let color_theme = o.color_arrays().is_some();
    let er_bg = o.get("erEdgeLabelBackground").map(str::to_string);
    let sw = o.s("strokeWidth");
    let colors = gen_color(o, include_str!("css/er_color.css"), &|e, _, _, _| match e {
        "@1" => Some(String::new()),
        _ => None,
    });
    let label_bkg = match (&er_bg, color_theme) {
        (Some(b), true) => b.clone(),
        _ => fade(&o.s("tertiaryColor"), 0.5),
    };
    let edge_bkg = match (&er_bg, color_theme) {
        (Some(b), true) => b.clone(),
        _ => o.s("edgeLabelBackground"),
    };
    let width = if neo { sw } else { "1px".to_string() };
    tpl(o, include_str!("css/er.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" => Some(label_bkg.clone()),
        "@3" | "@4" => Some(edge_bkg.clone()),
        "@5" | "@6" => Some(width.clone()),
        "@7" => Some(fade(&o.s("tertiaryColor"), 0.5)),
        _ => None,
    })
}

fn requirement(o: &Opts) -> String {
    let neo = o.look == crate::config::Look::Neo;
    let colors = gen_color(o, include_str!("css/requirement_color.css"), &|e, _, _, fill| {
        (e == "@1").then(|| fill.to_string())
    });
    let width = if neo {
        o.s("strokeWidth")
    } else {
        "1px".to_string()
    };
    let bkg = o
        .get("requirementEdgeLabelBackground")
        .map_or_else(|| o.s("edgeLabelBackground"), str::to_string);
    tpl(o, include_str!("css/requirement.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" => Some(width.clone()),
        "@3" => Some(bkg.clone()),
        _ => None,
    })
}

fn class(o: &Opts) -> String {
    let colors = gen_color(o, include_str!("css/class_color.css"), &|e, _, _, fill| {
        (e == "@1").then(|| fill.to_string())
    });
    tpl(o, include_str!("css/class.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" => Some(icon_styles()),
        _ => None,
    })
}

fn flowchart(o: &Opts) -> String {
    let bkg = o.color_arrays().map(|c| c.1).unwrap_or_default();
    let colors = gen_color(
        o,
        include_str!("css/flowchart_color.css"),
        &|e, i, _, _| {
            let look = look_name(o);
            let slot = format!("[data-look=\"{look}\"][data-color-id=\"color-{i}\"]");
            let lane = |suffix: &str| {
                format!(
                    "{slot}.swimlane.cluster .swimlane-title{suffix}, {slot}.swimlane.cluster .swimlane-body{suffix}"
                )
            };
            let collapsed =
                |suffix: &str| format!("{slot}.node {suffix}, {slot}.rough-node {suffix}");
            match e {
                "@1" => Some(lane(" path:nth-of-type(2)")),
                "@2" => Some(if bkg.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{} {{ stroke: {}; }}",
                        lane(" path:first-of-type"),
                        bkg[i % bkg.len()]
                    )
                }),
                "@3" => Some(collapsed(".collapsed-group")),
                "@4" => Some(collapsed(".collapsed-group path")),
                "@5" => Some(collapsed(".collapsed-indicator")),
                "@6" => Some(collapsed(".collapsed-separator")),
                _ => None,
            }
        },
    );
    tpl(o, include_str!("css/flowchart.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" => Some(fade(&o.s("edgeLabelBackground"), 0.5)),
        "@3" => Some(icon_styles()),
        _ => None,
    })
}

fn state(o: &Opts) -> String {
    let colors = gen_color(o, include_str!("css/state_color.css"), &|_, _, _, _| None);
    let alt = o
        .get("altBackground")
        .filter(|v| !v.is_empty())
        .unwrap_or("#efefef")
        .to_string();
    let stroke = if o.get("useGradient") == Some("true") {
        format!("url({}-gradient)", o.svg_sel)
    } else {
        o.or(&["stateBorder", "nodeBorder"])
    };
    tpl(o, include_str!("css/state.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" | "@3" => Some(alt.clone()),
        "@4" => Some(stroke.clone()),
        "@5" => Some(scoped_shadow(o)),
        _ => None,
    })
}

fn block(o: &Opts) -> String {
    let colors = gen_color(o, include_str!("css/block_color.css"), &|_, _, _, _| None);
    tpl(o, include_str!("css/block.css"), &|e| match e {
        "@1" => Some(colors.clone()),
        "@2" => Some(fade(&o.s("mainBkg"), 0.5)),
        "@3" => Some(fade(&o.s("clusterBkg"), 0.5)),
        "@4" => Some(fade(&o.s("clusterBorder"), 0.2)),
        "@5" => Some(icon_styles()),
        _ => None,
    })
}

fn theme_is(o: &Opts, names: &[&str]) -> bool {
    names.contains(&o.theme.name())
}

fn includes_redux(o: &Opts) -> bool {
    o.theme.name().contains("redux")
}

fn indexed(o: &Opts, prefix: &str, i: usize) -> String {
    o.s(&format!("{prefix}{i}"))
}

fn mindmap(o: &Opts) -> String {
    let neo = o.look == crate::config::Look::Neo;
    let neutral = o.theme == super::ThemeName::Neutral;
    let mut sections = String::new();
    for i in 0..limit(o) {
        let j = i as i64 - 1;
        let sw = if neo {
            (10 - j * 2).max(2)
        } else {
            17 - 3 * i as i64
        };
        let neo_label = if theme_is(o, &["redux", "redux-dark"]) {
            o.s("nodeBorder")
        } else {
            indexed(o, "cScaleLabel", if neutral { 1 } else { i })
        };
        sections += &tpl(o, include_str!("css/mindmap_section.css"), &|e| match e {
            "i - 1" => Some(j.to_string()),
            "sw" => Some(sw.to_string()),
            "neoLabel" => Some(neo_label.clone()),
            "@1" | "@5" => Some(indexed(o, "cScale", i)),
            "@2" | "@3" | "@4" => Some(indexed(o, "cScaleLabel", i)),
            "@6" => Some(indexed(o, "cScaleInv", i)),
            "@7" => Some(if theme_is(o, &["redux", "redux-dark", "neutral"]) {
                o.s("mainBkg")
            } else {
                indexed(o, "cScale", i)
            }),
            "@8" => Some(if theme_is(o, &["redux", "redux-dark"]) {
                o.s("nodeBorder")
            } else {
                indexed(o, "cScale", i)
            }),
            "@9" => Some(if includes_redux(o) || theme_is(o, &["neo-dark"]) {
                o.s("nodeBorder")
            } else {
                indexed(o, "cScale", i)
            }),
            _ => None,
        });
    }
    let neo_root_label = if includes_redux(o) {
        o.s("nodeBorder")
    } else {
        indexed(o, "cScaleLabel", if neutral { 1 } else { 0 })
    };
    let gradient = if o.get("useGradient") == Some("true")
        && o.get("mainBkg").is_some_and(|v| !v.is_empty())
    {
        let mut g = String::new();
        for i in 0..limit(o) {
            g += &tpl(o, include_str!("css/mindmap_grad.css"), &|e| match e {
                "i - 1" => Some((i as i64 - 1).to_string()),
                "svgId" => Some(o.svg_sel.clone()),
                "mainBkg" => Some(o.s("mainBkg")),
                _ => None,
            });
        }
        g
    } else {
        String::new()
    };
    tpl(o, include_str!("css/mindmap.css"), &|e| match e {
        "@1" => Some(sections.clone()),
        "@2" => Some(if includes_redux(o) {
            o.s("nodeBorder")
        } else {
            o.s("gitBranchLabel0")
        }),
        "@3" => Some(if includes_redux(o) {
            o.s("mainBkg")
        } else {
            o.s("git0")
        }),
        "@4" => Some(gradient.clone()),
        "scopedDropShadow" => Some(scoped_shadow(o)),
        "neoRootLabel" => Some(neo_root_label.clone()),
        _ => None,
    })
}

fn timeline(o: &Opts) -> String {
    let redux = includes_redux(o);
    let neutral = o.theme == super::ThemeName::Neutral;
    let raw_id = o.svg_sel.trim_start_matches('#').to_string();
    let colors = o.color_arrays();
    let dark_theme = o.theme.name().contains("dark");
    let shadow = if raw_id.is_empty() {
        o.get("dropShadow").unwrap_or("none").to_string()
    } else {
        format!("url(#{raw_id}-drop-shadow)")
    };
    let mut sections = String::new();
    for i in 0..limit(o) {
        let j = i as i64 - 1;
        let sw = 17 - 3 * i as i64;
        if redux {
            let slot = colors.as_ref().map(|(b, _)| b[i % b.len()].clone());
            let color = slot.clone().unwrap_or_else(|| o.s("mainBkg"));
            let stroke = slot.clone().unwrap_or_else(|| o.s("nodeBorder"));
            sections += &tpl(o, include_str!("css/timeline_redux.css"), &|e| match e {
                "i - 1" => Some(j.to_string()),
                "sw" => Some(sw.to_string()),
                "stroke" => Some(stroke.clone()),
                "scopedDropShadow" => Some(shadow.clone()),
                "@1" => Some(if dark_theme && colors.is_some() {
                    o.s("mainBkg")
                } else {
                    color.clone()
                }),
                "@2" => Some(indexed(o, "cScaleLabel", i)),
                "@3" => Some(indexed(o, "cScale", i)),
                "@4" => Some(indexed(o, "cScaleInv", i)),
                _ => None,
            });
        } else {
            sections += &tpl(o, include_str!("css/timeline_classic.css"), &|e| match e {
                "i - 1" => Some(j.to_string()),
                "sw" => Some(sw.to_string()),
                "@1" | "@4" => Some(indexed(o, "cScale", i)),
                "@2" | "@3" | "@6" => Some(indexed(o, "cScaleLabel", i)),
                "@5" => Some(indexed(o, "cScaleInv", i)),
                _ => None,
            });
        }
    }
    let mut gradient = String::new();
    if o.get("useGradient") == Some("true") && !raw_id.is_empty() && !neutral {
        for i in 0..limit(o) {
            gradient += &tpl(o, include_str!("css/timeline_grad.css"), &|e| match e {
                "i - 1" => Some((i as i64 - 1).to_string()),
                "rawSvgId" => Some(raw_id.clone()),
                _ => None,
            });
        }
    }
    tpl(o, include_str!("css/timeline.css"), &|e| match e {
        "@1" => Some(sections.clone()),
        "gradientSections" => Some(gradient.clone()),
        _ => None,
    })
}
