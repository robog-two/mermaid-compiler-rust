//! `usecase` diagrams (mermaid 12.3 `usecase`). Not implemented yet.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::scene::Scene;
use crate::theme::Palette;

/// Parse, lay out and draw a `usecase` diagram.
pub fn render(
    _body: &str,
    _title: Option<String>,
    _cfg: &MermaidConfig,
    _pal: &Palette,
) -> Result<Scene> {
    Err(Error::UnsupportedDiagram {
        kind: "usecase".to_string(),
    })
}
