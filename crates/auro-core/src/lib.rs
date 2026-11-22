// SPDX-License-Identifier: MIT OR Apache-2.0

//! The application model: configuration, state, and the runtime bridge.
//!
//! Renderer-agnostic on purpose: nothing here names GPUI.

pub mod autosave;
pub mod config;
pub mod events;
pub mod instance;
pub mod lifecycle;
pub mod snapshot;

pub use auro_ipc::AuroError;
pub use auro_ipc::runtime;
pub use auro_ipc::state::{State, StateStore};

/// Signed in-process plugin host (stage 1); requires the `plugins` feature.
#[cfg(feature = "plugins")]
pub use auro_plugins as plugins;
pub use autosave::{Autosave, DEFAULT_DEBOUNCE};
pub use config::{
    AppConfig, BundleConfig, BundleTargets, Config, DevConfig, MacOsBundleConfig, Theme,
    WindowConfig, WindowsBundleConfig,
};
pub use events::EventSender;
pub use instance::{AppInstance, InstanceListener, InstanceMessage};
pub use lifecycle::LifecycleEvent;
pub use snapshot::{FORMAT_VERSION, ReloadSnapshot, ResetReason, Restored, Snapshot};
