// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::PathBuf;

/// The single error type crossing a command boundary.
///
/// `#[non_exhaustive]`: adding a variant must not break a downstream `match`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuroError {
    #[error("configuration error in {path}{}: {message}", key_suffix(.key))]
    Config {
        path: PathBuf,
        key: Option<String>,
        message: String,
    },

    #[error("command `{name}` failed: {message}")]
    Command { name: String, message: String },

    #[error("unknown command `{0}`")]
    UnknownCommand(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Message(String),
}

fn key_suffix(key: &Option<String>) -> String {
    match key {
        Some(key) => format!(" (at `{key}`)"),
        None => String::new(),
    }
}

impl AuroError {
    /// Stable machine-readable tag used by [`crate::Response`].
    pub fn kind(&self) -> &'static str {
        match self {
            AuroError::Config { .. } => "config",
            AuroError::Command { .. } => "command",
            AuroError::UnknownCommand(_) => "unknown-command",
            AuroError::Io(_) => "io",
            AuroError::Json(_) => "json",
            AuroError::Message(_) => "message",
        }
    }

    pub fn config(path: impl Into<PathBuf>, key: Option<&str>, message: impl Into<String>) -> Self {
        AuroError::Config {
            path: path.into(),
            key: key.map(str::to_owned),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_an_io_error_with_the_question_mark() {
        fn read() -> Result<String, AuroError> {
            Ok(std::fs::read_to_string("/nonexistent/auro-audit")?)
        }

        let error = read().expect_err("the path does not exist");
        assert_eq!(error.kind(), "io");
    }

    #[test]
    fn converts_a_json_error_with_the_question_mark() {
        fn parse() -> Result<serde_json::Value, AuroError> {
            Ok(serde_json::from_str("{")?)
        }

        let error = parse().expect_err("invalid JSON");
        assert_eq!(error.kind(), "json");
    }
}
