//! A browser-free, pure-Rust reimplementation of mermaid-cli's `mmdc`.
//!
//! Architecture ("functional core, imperative shell"):
//!
//! * **Core** (everything in this library): pure functions from text to data.
//!   `render::render` parses a definition, runs the layout engine and produces a
//!   [`scene::Scene`]; `output::*` serialise a scene to SVG / PDF / PNG bytes;
//!   `cli::parse`, `plan` and `markdown` make every CLI decision without I/O.
//! * **Shell** (`src/main.rs`): reads files and stdin, writes files and stdout,
//!   prints logs and chooses the exit code.
//!
//! ```
//! use mmdc::{config::MermaidConfig, output, render};
//! // The SVG exactly as the `mmdc` binary writes it.
//! let svg = render::render_svg(
//!     "graph TD\nA-->B",
//!     &MermaidConfig::default(),
//!     &render::RenderEnv::default(),
//!     &output::EncodeOptions::default(),
//! )
//! .unwrap();
//! assert!(svg.starts_with("<svg id=\"my-svg\""));
//! ```

#![forbid(unsafe_code)]

pub mod cli;
pub mod color;
pub mod config;
pub mod diagram;
pub mod error;
pub mod fonts;
pub mod geom;
pub mod iconify;
pub mod icons;
pub mod json;
pub mod layout;
pub mod linejump;
pub mod markdown;
pub mod output;
pub mod plan;
pub mod render;
pub mod rough;
pub mod scene;
pub mod text;
pub mod theme;

/// Version reported by `mmdc -V`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
