// SPDX-License-Identifier: MIT OR Apache-2.0

//! Auro: Tauri's app model, GPUI's renderer, no webview.
//!
//! The facade crate: it owns the builder, connects the app model to the renderer, and is the only
//! crate an app depends on.

extern crate self as auro;

pub use auro_core::{AuroError, Config, EventSender, LifecycleEvent, State, StateStore, Theme};
pub use auro_core::{Autosave, ReloadSnapshot, ResetReason, Restored, Snapshot};
pub use auro_macros::{command, generate_handler};
pub use auro_ui::gpui;

mod app;

#[cfg(feature = "hot-reload")]
pub mod hot;

pub use app::{App, emit};

/// The implementation surface the macros expand against. Not a user-facing module.
#[doc(hidden)]
pub mod ipc {
    pub use auro_ipc::AuroError;
    pub use auro_ipc::handler::CommandHandler;
    pub use auro_ipc::invoke::{Invoke, InvokeMessage};
    pub use auro_ipc::response::{Response, ResponseFuture};
    pub use auro_ipc::runtime;
    pub use auro_ipc::schema::{CommandSchema, input_schema};
    pub use auro_ipc::state::{State, StateStore};
    pub use auro_ipc::{schemars, serde_json};
}

pub mod prelude {
    pub use auro_core::{AuroError, Config, EventSender, LifecycleEvent, State, Theme};
    pub use auro_macros::{command, generate_handler};
    pub use auro_ui::prelude::*;

    pub use crate::{App, emit};
    pub use crate::{Autosave, ReloadSnapshot, ResetReason, Restored, Snapshot};
}
