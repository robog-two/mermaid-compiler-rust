//! Error types shared by the functional core.

use std::fmt;

/// Every failure the pure rendering pipeline can produce.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The diagram text could not be parsed.
    Parse { line: usize, message: String },
    /// No diagram keyword was recognised (mirrors mermaid's `UnknownDiagramError`).
    UnknownDiagram { text: String },
    /// The diagram keyword is valid mermaid, but this implementation does not render it.
    UnsupportedDiagram { kind: String },
    /// Invalid configuration (config file, front-matter or `%%{init}%%` directive).
    Config(String),
}

impl Error {
    /// Convenience constructor for parse errors.
    pub fn parse(line: usize, message: impl Into<String>) -> Self {
        Error::Parse {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { line, message } => write!(f, "Parse error on line {line}:\n{message}"),
            Error::UnknownDiagram { text } => write!(
                f,
                "No diagram type detected matching given configuration for text: {text}"
            ),
            Error::UnsupportedDiagram { kind } => write!(
                f,
                "Diagram type \"{kind}\" is valid mermaid but is not implemented by this mmdc yet"
            ),
            Error::Config(msg) => write!(f, "Configuration error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias for the core.
pub type Result<T> = std::result::Result<T, Error>;
