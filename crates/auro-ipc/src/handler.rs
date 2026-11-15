// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use crate::error::AuroError;
use crate::invoke::Invoke;
use crate::response::{Response, ResponseFuture};

type Dispatch = Arc<dyn Fn(Invoke) -> ResponseFuture + Send + Sync>;

/// Maps a command name to its typed dispatch.
///
/// Built by `auro::generate_handler!`. Phase 0 tests it; plugins wire it into the app later.
#[derive(Clone)]
pub struct CommandHandler {
    entries: Vec<(&'static str, Dispatch)>,
}

impl CommandHandler {
    pub fn new() -> Self {
        CommandHandler {
            entries: Vec::new(),
        }
    }

    pub fn command(
        mut self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
    ) -> Self {
        self.entries.push((name, Arc::new(dispatch)));
        self
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().map(|(name, _)| *name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|(known, _)| *known == name)
    }

    /// Dispatches by name; an unknown name resolves to an error response, never a panic.
    pub fn dispatch(&self, invoke: Invoke) -> ResponseFuture {
        match self
            .entries
            .iter()
            .find(|(name, _)| *name == invoke.message.command)
        {
            Some((_, dispatch)) => dispatch(invoke),
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
