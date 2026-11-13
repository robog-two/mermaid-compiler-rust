//! Force-directed layouts ported from the cytoscape.js extensions mermaid uses:
//! CoSE-Bilkent for mindmaps ([`cose`]) and fCoSE for architecture diagrams
//! ([`fcose`]), with the Chromium-exact math they depend on ([`jsmath`]).

pub mod cose;
pub mod fcose;
mod geometry;
pub mod jsmath;
