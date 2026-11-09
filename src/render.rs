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

/// Parse, lay out and draw one mermaid definition.
pub fn render(source: &str, base: &MermaidConfig) -> Result<Rendered> {
    let pre = diagram::preprocess(source)?;
    let mut cfg = base.clone();
    for layer in &pre.config_layers {
        cfg = cfg.with_json(layer).map_err(Error::Config)?;
    }
    let pal = cfg.palette();
    let kind = diagram::detect(&pre.body)?;
    let (mut scene, use_max_width) = match kind {
        DiagramType::Flowchart => {
            let g = diagram::flowchart::parse(&pre.body, pre.title.clone())?;
            (
                diagram::graph::render(&g, &cfg.flowchart, &pal),
                cfg.flowchart.use_max_width,
            )
        }
        DiagramType::State => {
            let g = diagram::state::parse(&pre.body, pre.title.clone())?;
            (
                diagram::graph::render(&g, &cfg.state, &pal),
                cfg.state.use_max_width,
            )
        }
        DiagramType::Class => {
            let g = diagram::class::parse(&pre.body, pre.title.clone())?;
            (
                diagram::graph::render(&g, &cfg.class, &pal),
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
    };
    scene.acc_title = pre.acc_title;
    scene.acc_descr = pre.acc_descr;
    Ok(Rendered {
        scene,
        theme_css: cfg.theme_css.clone(),
        use_max_width,
    })
}
