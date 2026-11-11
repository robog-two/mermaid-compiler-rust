//! `wardley` diagrams (mermaid 12.3 `wardley`). Not implemented yet.

use crate::config::MermaidConfig;
use crate::error::{Error, Result};
use crate::scene::Scene;
use crate::theme::Palette;

/// Parse, lay out and draw a `wardley` diagram.
pub fn render(
    _body: &str,
    _title: Option<String>,
    _cfg: &MermaidConfig,
    _pal: &Palette,
) -> Result<Scene> {
    Err(Error::UnsupportedDiagram {
        kind: "wardley".to_string(),
    })
}
