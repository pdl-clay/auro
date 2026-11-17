// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use serde_json::Value;

use crate::error::AuroError;
use crate::invoke::Invoke;
use crate::response::{Response, ResponseFuture};
use crate::schema::CommandSchema;

type Dispatch = Arc<dyn Fn(Invoke) -> ResponseFuture + Send + Sync>;

/// One registered command and its optional metadata.
#[derive(Clone)]
struct Entry {
    name: &'static str,
    dispatch: Dispatch,
    schema: Option<CommandSchema>,
}

/// Maps a command name to its typed dispatch and schema.
///
/// Built by `auro::generate_handler!`. Plugins wire it into the app later.
#[derive(Clone)]
pub struct CommandHandler {
    entries: Vec<Entry>,
}

impl CommandHandler {
    pub fn new() -> Self {
        CommandHandler {
            entries: Vec::new(),
        }
    }

    pub fn command(
        self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
    ) -> Self {
        self.push(name, dispatch, None)
    }

    /// Registers a command with its input/output schemas.
    pub fn command_with_schema(
        self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
        input: Value,
        output: Value,
    ) -> Self {
        let schema = CommandSchema {
            name,
            input,
            output,
        };
        self.push(name, dispatch, Some(schema))
    }

    fn push(
        mut self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
        schema: Option<CommandSchema>,
    ) -> Self {
        self.entries.push(Entry {
            name,
            dispatch: Arc::new(dispatch),
            schema,
        });
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().map(|entry| entry.name)
    }

    /// The schemas registered by `command_with_schema`, in registration order.
    pub fn schemas(&self) -> impl Iterator<Item = &CommandSchema> + '_ {
        self.entries
            .iter()
            .filter_map(|entry| entry.schema.as_ref())
    }

    /// The schema for one command, if it was registered with metadata.
    pub fn schema(&self, name: &str) -> Option<&CommandSchema> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.schema.as_ref())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.name == name)
    }

    /// Dispatches by name; an unknown name resolves to an error response, never a panic.
    pub fn dispatch(&self, invoke: Invoke) -> ResponseFuture {
        match self
            .entries
            .iter()
            .find(|entry| entry.name == invoke.message.command)
        {
            Some(entry) => (entry.dispatch)(invoke),
            None => {
                let name = invoke.message.command.clone();
                Box::pin(async move { Response::from_error(&AuroError::UnknownCommand(name)) })
            }
        }
    }
}

impl Default for CommandHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CommandHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}
