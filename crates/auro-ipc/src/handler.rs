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
        self.try_command(name, dispatch)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Registers a command, rejecting a duplicate name instead of shadowing the first one.
    pub fn try_command(
        self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
    ) -> Result<Self, AuroError> {
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
        self.try_command_with_schema(name, dispatch, input, output)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Registers a command with schemas, rejecting a duplicate name.
    pub fn try_command_with_schema(
        self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
        input: Value,
        output: Value,
    ) -> Result<Self, AuroError> {
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
    ) -> Result<Self, AuroError> {
        if self.contains(name) {
            return Err(AuroError::Message(format!(
                "duplicate command registration `{name}`"
            )));
        }
        self.entries.push(Entry {
            name,
            dispatch: Arc::new(dispatch),
            schema,
        });
        Ok(self)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: Invoke) -> ResponseFuture {
        Box::pin(async { Response::ok(()) })
    }

    #[test]
    fn a_duplicate_name_is_rejected_instead_of_shadowing_the_first() {
        let handler = CommandHandler::new().command("save", noop);
        let error = handler
            .try_command("save", noop)
            .expect_err("duplicate command");
        assert!(error.to_string().contains("duplicate command"), "{error}");
        assert!(error.to_string().contains("save"), "{error}");
    }
}
