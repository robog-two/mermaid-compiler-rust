//! The pure entry point: diagram text + configuration → positioned `Scene`.

use crate::config::MermaidConfig;
use crate::diagram::{self, DiagramType};
use crate::error::{Error, Result};
use crate::scene::Scene;

/// A rendered diagram, ready for any encoder.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub scene: Scene,
    /// mermaid `themeCSS` (from config or `--cssFile`).
    pub theme_css: Option<String>,
    /// mermaid's per-diagram `useMaxWidth`.
    pub use_max_width: bool,
    /// The `height` attribute of a fixed-size (`useMaxWidth: false`) svg where
    /// it is not the viewBox's height (a sequence diagram's).
    pub svg_height: Option<f64>,
    /// What mermaid's own stylesheet of this diagram depends on.
    pub style: Option<crate::theme::styles::StyleSpec>,
}

impl Rendered {
    /// Accessible title (`accTitle`), used by Markdown mode for image titles.
    pub fn title(&self) -> Option<&str> {
        self.scene.acc_title.as_deref()
    }
    /// Accessible description (`accDescr`), used by Markdown mode for alt text.
    pub fn description(&self) -> Option<&str> {
        self.scene.acc_descr.as_deref()
    }
}

/// Shared [`diagram::graph::RenderOptions`] of the flowchart / state / class renderers.
/// A graph diagram with the references its HTML labels make to registered
/// icon packs (`fa:fa-home` with a pack named `fa`) turned into the packs' icons.
fn packed(
    g: diagram::graph::GraphDiagram,
    gcfg: &crate::config::GraphConfig,
    cfg: &MermaidConfig,
) -> diagram::graph::GraphDiagram {
    if cfg.icon_packs.is_empty() {
        return g;
    }
    diagram::graph::with_icons_from(&g, gcfg, &cfg.icon_packs).into_owned()
}

fn graph_options(cfg: &MermaidConfig) -> diagram::graph::RenderOptions {
    diagram::graph::RenderOptions {
        subgraph_title_margin: cfg.subgraph_title_margin(),
        markdown_no_wrap: !cfg.top_bool("markdownAutoWrap", true),
        ..Default::default()
    }
}

/// What mermaid reads from its surroundings rather than from the diagram: the
/// library never reads the clock itself, so a caller that wants a gantt chart's
/// today line passes the time in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderEnv {
    /// "Now" in milliseconds since 1970-01-01T00:00Z (mermaid's `new Date()`);
    /// `None` leaves out everything that depends on it (the gantt today line).
    pub now_ms: Option<i64>,
    /// The bytes of the remote images the diagram links to ([`remote_images`]),
    /// by URL, as the caller fetched them; an image missing here cannot be
    /// decoded, as in a browser without network.
    pub images: RemoteImages,
}

/// Fetched remote images by URL.
pub type RemoteImages = std::sync::Arc<std::collections::BTreeMap<String, Vec<u8>>>;

/// The remote images (URLs other than `data:`) a definition's diagram links
/// to, which mermaid's browser would fetch to size and draw them. Empty when
/// there are none, or the definition does not parse.
pub fn remote_images(source: &str, base: &MermaidConfig) -> Vec<String> {
    let graph = || -> Result<diagram::graph::GraphDiagram> {
        let pre = diagram::preprocess(source)?;
        let mut cfg = base.clone();
        for layer in &pre.config_layers {
            cfg = cfg.with_directive(layer).map_err(Error::Config)?;
        }
        let kind = diagram::detect(&pre.body)?;
        let cfg = cfg.scoped(kind.config_key());
        let opts = diagram::flowchart::ParseOptions {
            inherit_dir: cfg.section_bool("flowchart", "inheritDir", false),
        };
        match kind {
            DiagramType::Flowchart => {
                diagram::flowchart::parse_with(&pre.body, pre.title.clone(), opts)
            }
            DiagramType::Swimlane if diagram::swimlane::other_layout(&cfg).is_some() => {
                diagram::swimlane::parse_flowchart(&pre.body, pre.title.clone(), opts)
            }
            _ => Err(Error::Render(String::new())),
        }
    };
    graph()
        .map(|g| diagram::flowchart::remote_images(&g))
        .unwrap_or_default()
}

/// Parse, lay out and draw one mermaid definition, as of no particular time
/// (see [`render_with_env`]).
///
/// Parse errors report line numbers the way mermaid does: counted in the text left
/// after front-matter, directives and comment lines are removed
/// ([`diagram::mermaid_line_numbers`]).
pub fn render(source: &str, base: &MermaidConfig) -> Result<Rendered> {
    render_with_env(source, base, &RenderEnv::default())
}

/// The SVG file `mmdc` writes for one definition: [`render_with_env`] and
/// [`crate::output::encode_svg`] run together inside
/// [`crate::output::svg::with_js_numbers`], so every number is printed as
/// JavaScript (and so mermaid) prints it. Calling the two separately gives the
/// compact three-decimal form the PNG and PDF paths are built against.
pub fn render_svg(
    source: &str,
    base: &MermaidConfig,
    env: &RenderEnv,
    enc: &crate::output::EncodeOptions,
) -> Result<String> {
    crate::output::svg::with_js_numbers(|| {
        render_with_env(source, base, env).map(|r| crate::output::encode_svg(&r, enc))
    })
}

/// [`render`] with the inputs mermaid takes from its environment.
pub fn render_with_env(source: &str, base: &MermaidConfig, env: &RenderEnv) -> Result<Rendered> {
    render_source(source, base, env).map_err(|e| match e {
        Error::Parse { line, message } => {
            let map = diagram::mermaid_line_numbers(source);
            let to = |l: usize| map.get(l.wrapping_sub(1)).copied().flatten().unwrap_or(l);
            let mermaid_line = to(line);
            // Langium-style messages repeat the line inside the text.
            let message = ["Parse error on line ", "Lexer error on line "]
                .iter()
                .find_map(|p| {
                    let rest = message.strip_prefix(p)?;
                    let digits = rest.chars().take_while(char::is_ascii_digit).count();
                    let n: usize = rest[..digits].parse().ok()?;
                    Some(format!("{p}{}{}", to(n), &rest[digits..]))
                })
                .unwrap_or(message);
            Error::Parse {
                line: mermaid_line,
                message,
            }
        }
        other => other,
    })
}

fn render_source(source: &str, base: &MermaidConfig, env: &RenderEnv) -> Result<Rendered> {
    let pre = diagram::preprocess(source)?;
    let mut cfg = base.clone();
    for layer in &pre.config_layers {
        cfg = cfg.with_directive(layer).map_err(Error::Config)?;
    }
    let kind = diagram::detect(&pre.body)?;
    // mermaid 12 resolves `theme` / `look` per diagram type: flowchart, sequence, class,
    // state, ER and requirement default to `redux-color` + `neo` unless a directive or
    // the site config (`-t`, `--configFile`) says otherwise.
    let cfg = cfg.scoped(kind.config_key());
    let pal = cfg.palette();
    // Text is measured in the face the theme's `fontFamily` names (the redux themes'
    // Recursive Variable, which mmdc loads as a web font; see `text::webfonts`).
    let mut svg_height = None;
    let (mut scene, use_max_width) =
        crate::text::with_family(&pal.font_family, || -> Result<_> {
            Ok(match kind {
                DiagramType::Flowchart => {
                    let opts = diagram::flowchart::ParseOptions {
                        inherit_dir: cfg.section_bool("flowchart", "inheritDir", false),
                    };
                    let mut g = diagram::flowchart::parse_with(&pre.body, pre.title.clone(), opts)?;
                    diagram::flowchart::attach_assets(&mut g, &cfg.icon_packs, &env.images)?;
                    let g = packed(g, &cfg.flowchart, &cfg);
                    (
                        diagram::graph::render_with(&g, &cfg.flowchart, &pal, &graph_options(&cfg)),
                        cfg.flowchart.use_max_width,
                    )
                }
                DiagramType::State => {
                    let g = diagram::state::parse(&pre.body, pre.title.clone())?;
                    let g = packed(g, &cfg.state, &cfg);
                    (
                        diagram::graph::render_with(&g, &cfg.state, &pal, &graph_options(&cfg)),
                        cfg.state.use_max_width,
                    )
                }
                DiagramType::Class => {
                    let opts = diagram::class::ParseOptions {
                        hierarchical_namespaces: cfg.section_bool(
                            "class",
                            "hierarchicalNamespaces",
                            true,
                        ),
                    };
                    let g = diagram::class::parse_with(&pre.body, pre.title.clone(), opts)?;
                    let g = packed(g, &cfg.class, &cfg);
                    let mut options = graph_options(&cfg);
                    // Notes and lollipop interfaces wrap at labelHelper's fallback width,
                    // `flowchart.wrappingWidth` (an HTML note keeps `white-space: nowrap`;
                    // class boxes size their text themselves).
                    options.node_wrap = Some(cfg.flowchart.wrapping_width);
                    (
                        diagram::graph::render_with(&g, &cfg.class, &pal, &options),
                        cfg.class.use_max_width,
                    )
                }
                DiagramType::Sequence => {
                    let d = diagram::sequence::parse(&pre.body, pre.title.clone())?;
                    let (scene, h) = diagram::sequence::render_sized(&d, &cfg.sequence, &pal);
                    svg_height = Some(h);
                    (scene, cfg.sequence.use_max_width)
                }
                DiagramType::Pie => {
                    let p = diagram::pie::parse(&pre.body, pre.title.clone())?;
                    (
                        diagram::pie::render(&p, &cfg.pie, &pal),
                        cfg.pie.use_max_width,
                    )
                }
                DiagramType::Er => (
                    diagram::er::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("er"),
                ),
                DiagramType::Gantt => (
                    diagram::gantt::render(&pre.body, pre.title.clone(), &cfg, &pal, env.now_ms)?,
                    cfg.use_max_width("gantt"),
                ),
                DiagramType::Journey => (
                    diagram::journey::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("journey"),
                ),
                DiagramType::GitGraph => (
                    diagram::gitgraph::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("gitGraph"),
                ),
                DiagramType::Mindmap => (
                    diagram::mindmap::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("mindmap"),
                ),
                DiagramType::Timeline => (
                    diagram::timeline::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("timeline"),
                ),
                DiagramType::Quadrant => (
                    diagram::quadrant::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("quadrantChart"),
                ),
                DiagramType::Requirement => (
                    diagram::requirement::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("requirement"),
                ),
                DiagramType::C4 => (
                    diagram::c4::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("c4"),
                ),
                DiagramType::Sankey => (
                    diagram::sankey::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("sankey"),
                ),
                DiagramType::XyChart => (
                    diagram::xychart::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("xyChart"),
                ),
                DiagramType::Block => (
                    diagram::block::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("block"),
                ),
                DiagramType::Packet => (
                    diagram::packet::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("packet"),
                ),
                DiagramType::Architecture => (
                    diagram::architecture::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("architecture"),
                ),
                DiagramType::Kanban => (
                    diagram::kanban::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("kanban"),
                ),
                DiagramType::Radar => (
                    diagram::radar::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("radar"),
                ),
                DiagramType::Treemap => (
                    diagram::treemap::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("treemap"),
                ),
                DiagramType::ZenUml => (
                    diagram::zenuml::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("zenuml"),
                ),
                DiagramType::Info => (
                    diagram::info::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("info"),
                ),
                DiagramType::Agentflow => (
                    diagram::agentflow::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("agentflow"),
                ),
                DiagramType::Swimlane => match diagram::swimlane::other_layout(&cfg) {
                    Some(layout) => {
                        let opts = diagram::flowchart::ParseOptions {
                            inherit_dir: cfg.section_bool("flowchart", "inheritDir", false),
                        };
                        let mut g =
                            diagram::swimlane::parse_flowchart(&pre.body, pre.title.clone(), opts)?;
                        diagram::flowchart::attach_assets(&mut g, &cfg.icon_packs, &env.images)?;
                        let g = packed(g, &cfg.flowchart, &cfg);
                        let fcfg = crate::config::GraphConfig {
                            layout,
                            ..cfg.flowchart.clone()
                        };
                        let mut scene =
                            diagram::graph::render_with(&g, &fcfg, &pal, &graph_options(&cfg));
                        scene.kind = crate::scene::DiagramKind::Swimlane;
                        (scene, cfg.use_max_width("swimlane"))
                    }
                    None => (
                        diagram::swimlane::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                        cfg.use_max_width("swimlane"),
                    ),
                },
                DiagramType::EventModeling => (
                    diagram::eventmodeling::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("eventmodeling"),
                ),
                DiagramType::TreeView => (
                    diagram::treeview::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("treeView"),
                ),
                DiagramType::Ishikawa => (
                    diagram::ishikawa::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.section_bool("ishikawa", "useMaxWidth", true),
                ),
                DiagramType::Railroad => (
                    diagram::railroad::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("railroad"),
                ),
                DiagramType::Venn => (
                    diagram::venn::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("venn"),
                ),
                DiagramType::Wardley => (
                    diagram::wardley::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("wardley-beta") && cfg.use_max_width("wardley"),
                ),
                DiagramType::Cynefin => (
                    diagram::cynefin::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("cynefin"),
                ),
                DiagramType::Usecase => (
                    diagram::usecase::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
                    cfg.use_max_width("usecase"),
                ),
            })
        })?;
    if !cfg.icon_packs.is_empty() {
        scene.elements =
            crate::icons::place_pack_icons(std::mem::take(&mut scene.elements), &cfg.icon_packs);
    }
    let scene_kind = scene.kind;
    scene.acc_title = pre.acc_title;
    scene.acc_descr = pre.acc_descr;
    Ok(Rendered {
        scene,
        theme_css: cfg.theme_css.clone(),
        use_max_width,
        svg_height,
        style: Some(crate::theme::styles::StyleSpec {
            kind: scene_kind,
            theme: cfg.theme,
            look: cfg.look,
            theme_variables: cfg.theme_variables.clone(),
            font_family: cfg.font_family.clone(),
            theme_css: cfg.theme_css.clone(),
        }),
    })
}
