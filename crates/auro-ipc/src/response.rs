// SPDX-License-Identifier: MIT OR Apache-2.0

use serde::Serialize;

use crate::error::AuroError;

/// The payload a command returns.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Response {
    Ok { value: serde_json::Value },
    Err { kind: String, message: String },
}

impl Response {
    /// Serialises a successful result. A value that cannot be serialised becomes an error
    /// response: the boundary never panics on a `Serialize` implementation.
    pub fn ok<T: Serialize>(value: T) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => Response::Ok { value },
            Err(error) => Response::from_error(&AuroError::from(error)),
        }
    }

    pub fn from_error(error: &AuroError) -> Self {
        Response::Err {
            kind: error.kind().to_owned(),
            message: error.to_string(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Response::Ok { .. })
    }
}

/// A boxed, `Send` response future: what a [`crate::CommandHandler`] stores per command.
pub type ResponseFuture = std::pin::Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
