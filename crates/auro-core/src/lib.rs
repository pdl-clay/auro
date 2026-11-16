// SPDX-License-Identifier: MIT OR Apache-2.0

//! The application model: configuration, state, and the runtime bridge.
//!
//! Renderer-agnostic on purpose: nothing here names GPUI.

pub mod config;
pub mod snapshot;

pub use auro_ipc::AuroError;
pub use auro_ipc::runtime;
pub use auro_ipc::state::{State, StateStore};
pub use config::{AppConfig, BundleConfig, BundleTargets, Config, DevConfig, Theme, WindowConfig};
pub use snapshot::{FORMAT_VERSION, ReloadSnapshot, ResetReason, Restored, Snapshot};
