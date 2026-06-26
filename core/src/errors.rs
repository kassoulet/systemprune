//! Error types used across SystemPrune.

use thiserror::Error;

/// Top-level error returned by SystemPrune APIs.
#[derive(Debug, Error)]
pub enum SystemPruneError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Raised when a native engine command fails or returns a non-zero
/// exit code.
#[derive(Debug, Clone, Error)]
#[error("{message} (engine={engine:?}, returncode={returncode:?}, command={command:?})")]
pub struct EngineError {
    pub message: String,
    pub engine: String,
    pub command: Vec<String>,
    pub returncode: Option<i32>,
    pub stderr: String,
}

impl EngineError {
    pub fn new(
        message: impl Into<String>,
        engine: impl Into<String>,
        command: Vec<String>,
        returncode: Option<i32>,
        stderr: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            engine: engine.into(),
            command,
            returncode,
            stderr: stderr.into(),
        }
    }
}

/// Raised when an engine's CLI output cannot be parsed into items.
#[derive(Debug, Clone, Error)]
#[error("failed to parse {engine} output at {context}: {message}")]
pub struct ParseError {
    pub engine: String,
    pub context: String,
    pub message: String,
}

impl ParseError {
    pub fn new(
        engine: impl Into<String>,
        context: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            engine: engine.into(),
            context: context.into(),
            message: message.into(),
        }
    }
}

/// Convenience alias used by most APIs.
pub type Result<T> = std::result::Result<T, SystemPruneError>;

/// Render a command line for human-readable error messages.
pub fn format_command(argv: &[String]) -> String {
    argv.iter()
        .map(|s| {
            if s.chars().any(char::is_whitespace) {
                format!("{:?}", s)
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
