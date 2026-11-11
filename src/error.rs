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
            // Langium-based parsers (railroad, treeView, wardley, ...) already say where the
            // error is, and mermaid wraps them as `MermaidParseError` ("Parsing failed: ...").
            Error::Parse { message, .. }
                if message.starts_with("Parse error on line ")
                    || message.starts_with("Lexer error on line ") =>
            {
                write!(f, "Parsing failed: {message}")
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langium_style_messages_are_not_prefixed_twice() {
        let e = Error::parse(2, "Parse error on line 2, column 5: Expecting `=`.");
        assert_eq!(
            e.to_string(),
            "Parsing failed: Parse error on line 2, column 5: Expecting `=`."
        );
        assert_eq!(
            Error::parse(3, "Expecting 'end'").to_string(),
            "Parse error on line 3:\nExpecting 'end'"
        );
    }
}
