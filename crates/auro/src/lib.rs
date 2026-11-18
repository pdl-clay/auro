// SPDX-License-Identifier: MIT OR Apache-2.0

//! Auro: Tauri's app model, GPUI's renderer, no webview.
//!
//! The facade crate: it owns the builder, connects the app model to the renderer, and is the only
//! crate an app depends on.

extern crate self as auro;

pub use auro_core::AppInstance;
pub use auro_core::InstanceMessage;
pub use auro_core::{AuroError, Config, EventSender, LifecycleEvent, State, StateStore, Theme};
pub use auro_core::{Autosave, ReloadSnapshot, ResetReason, Restored, Snapshot};
pub use auro_macros::{command, generate_handler};
pub use auro_ui::gpui;

pub mod perms {
    pub use auro_perms::{Capability, Denied, Grant, Policy, PolicyError, UnknownCapability};
}

#[cfg(feature = "services")]
pub mod services {
    pub use auro_services::{
        AppLinks, AppPath, Autostart, AutostartError, CredentialStore, DeepLink, DeepLinkError,
        Keychain, KeychainError, Notifications, NotificationsBackend, NotificationsError,
        ShortcutBackend, Shortcuts, ShortcutsError,
    };

    #[cfg(feature = "tray")]
    pub use auro_services::{
        Tray, TrayBackend, TrayButton, TrayButtonState, TrayClick, TrayError, TrayIcon,
        TrayMenuItem,
    };
}

mod app;
pub mod windows;

#[cfg(feature = "updater")]
/// Signed update manifests, staging, platform apply and restart handoff (specs 0012-0015, 0025-0031).
///
/// Enable the `updater` feature to use this module.
pub mod update {
    pub use auro_update::{
        ApplyJournal, ApplyKind, ApplyPlan, ApplyState, FORMAT_VERSION, InstallerRunner,
        JOURNAL_FORMAT_VERSION, Manifest, PackageRollback, PublicKey, RESTART_HELPER_ENV, Recovery,
        Relaunch, StagedUpdate, SystemInstallerRunner, UpdateDecision, UpdateError, Updater,
        current_platform, run_restart_helper,
    };

    #[cfg(target_os = "linux")]
    pub use auro_update::{
        AppImageInstaller, InstallReceipt, LinuxElevation, LinuxPackageInstaller, LinuxPackageKind,
    };

    #[cfg(target_os = "windows")]
    pub use auro_update::{WindowsInstallOutcome, WindowsInstaller, WindowsInstallerKind};

    #[cfg(target_os = "macos")]
    pub use auro_update::{MacAppInstaller, MacAppReceipt, MacPkgElevation, MacPkgInstaller};
}

#[cfg(feature = "hot-reload")]
pub mod hot;

pub use app::{App, STARTUP_TRACE_ENV, emit};

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
    pub use auro_core::{
        AuroError, Config, EventSender, InstanceMessage, LifecycleEvent, State, Theme,
    };
    pub use auro_macros::{command, generate_handler};
    pub use auro_ui::prelude::*;

    pub use crate::windows;
    pub use crate::{App, emit};
    pub use crate::{Autosave, ReloadSnapshot, ResetReason, Restored, Snapshot};
}
