// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::error::AuroError;
use crate::state::{State, StateStore};

/// What the caller sent: a command name and a JSON payload.
#[derive(Debug, Clone)]
pub struct InvokeMessage {
    pub command: String,
    pub payload: serde_json::Value,
}

impl InvokeMessage {
    pub fn new(command: impl Into<String>, payload: serde_json::Value) -> Self {
        InvokeMessage {
            command: command.into(),
            payload,
        }
    }
}

/// One dispatch: the message plus the state it may reach.
pub struct Invoke {
    pub message: InvokeMessage,
    states: Arc<StateStore>,
}

impl Invoke {
    pub fn new(message: InvokeMessage, states: Arc<StateStore>) -> Self {
        Invoke { message, states }
    }

    pub fn command(&self) -> &str {
        &self.message.command
    }

    /// Extracts one named argument from the payload.
    pub fn arg<T: DeserializeOwned>(&self, key: &str) -> Result<T, AuroError> {
        let value = self
            .message
            .payload
            .get(key)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value).map_err(|error| AuroError::Command {
            name: self.message.command.clone(),
            message: format!("argument `{key}`: {error}"),
        })
    }

    /// Resolves managed state by type.
    pub fn state<T: Send + Sync + 'static>(&self) -> Result<State<T>, AuroError> {
        self.states.state::<T>().ok_or_else(|| AuroError::Command {
            name: self.message.command.clone(),
            message: format!("state `{}` is not managed", std::any::type_name::<T>()),
        })
    }
}

impl std::fmt::Debug for Invoke {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invoke")
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}
