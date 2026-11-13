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
fn graph_options(cfg: &MermaidConfig) -> diagram::graph::RenderOptions {
    diagram::graph::RenderOptions {
        subgraph_title_margin: cfg.subgraph_title_margin(),
        markdown_no_wrap: !cfg.top_bool("markdownAutoWrap", true),
        ..Default::default()
    }
}

/// Parse, lay out and draw one mermaid definition.
///
/// Parse errors report line numbers the way mermaid does: counted in the text left
/// after front-matter, directives and comment lines are removed
/// ([`diagram::mermaid_line_numbers`]).
pub fn render(source: &str, base: &MermaidConfig) -> Result<Rendered> {
    render_source(source, base).map_err(|e| match e {
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

fn render_source(source: &str, base: &MermaidConfig) -> Result<Rendered> {
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
    let (mut scene, use_max_width) = match kind {
        DiagramType::Flowchart => {
            let opts = diagram::flowchart::ParseOptions {
                inherit_dir: cfg.section_bool("flowchart", "inheritDir", false),
            };
            let g = diagram::flowchart::parse_with(&pre.body, pre.title.clone(), opts)?;
            (
                diagram::graph::render_with(&g, &cfg.flowchart, &pal, &graph_options(&cfg)),
                cfg.flowchart.use_max_width,
            )
        }
        DiagramType::State => {
            let mut g = diagram::state::parse(&pre.body, pre.title.clone())?;
            if cfg.state.layout != crate::config::LayoutAlgorithm::Elk {
                g = diagram::state::without_note_groups(g);
            }
            (
                diagram::graph::render_with(&g, &cfg.state, &pal, &graph_options(&cfg)),
                cfg.state.use_max_width,
            )
        }
        DiagramType::Class => {
            let opts = diagram::class::ParseOptions {
                hierarchical_namespaces: cfg.section_bool("class", "hierarchicalNamespaces", true),
            };
            let g = diagram::class::parse_with(&pre.body, pre.title.clone(), opts)?;
            (
                diagram::graph::render_with(&g, &cfg.class, &pal, &graph_options(&cfg)),
                cfg.class.use_max_width,
            )
        }
        DiagramType::Sequence => {
            let d = diagram::sequence::parse(&pre.body, pre.title.clone())?;
            (
                diagram::sequence::render(&d, &cfg.sequence, &pal),
                cfg.sequence.use_max_width,
            )
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
            diagram::gantt::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
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
        DiagramType::Swimlane => (
            diagram::swimlane::render(&pre.body, pre.title.clone(), &cfg, &pal)?,
            cfg.use_max_width("swimlane"),
        ),
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
            cfg.section_bool("ishikawa", "useMaxWidth", false),
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
    };
    scene.acc_title = pre.acc_title;
    scene.acc_descr = pre.acc_descr;
    Ok(Rendered {
        scene,
        theme_css: cfg.theme_css.clone(),
        use_max_width,
    })
}
