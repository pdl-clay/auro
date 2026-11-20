// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed command transport.
//!
//! A command is a Rust function. First-party code calls it directly; [`CommandHandler`] is the
//! boundary used when the caller is not Rust - a plugin today, the webview escape hatch later.
//!
//! Everything here is renderer-agnostic: this crate knows nothing about GPUI.

pub mod error;
pub mod handler;
pub mod invoke;
pub mod panic;
pub mod response;
pub mod runtime;
pub mod schema;
pub mod state;

pub use error::AuroError;
pub use handler::CommandHandler;
pub use invoke::{Invoke, InvokeMessage};
pub use panic::catch_command_panic;
pub use response::{Response, ResponseFuture};
pub use schema::{CommandSchema, input_schema};
pub use schemars;
pub use serde_json;
pub use state::{State, StateStore};
