// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform services Auro provides beyond the renderer.
//!
//! This crate is the home for OS integrations that are not GPUI-specific. It starts with
//! autostart; notifications, keychain, tray, global shortcuts and deep links land here as they are
//! specced and implemented.

use std::path::Path;

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

pub mod deep_link;
pub mod keychain;
pub mod notifications;
pub mod shortcuts;
#[cfg(feature = "tray")]
pub mod tray;
pub use deep_link::{AppLinks, DeepLink, DeepLinkError};
pub use keychain::{CredentialStore, Keychain, KeychainError};
pub use notifications::{Notifications, NotificationsBackend, NotificationsError};
pub use shortcuts::{ShortcutBackend, Shortcuts, ShortcutsError};
#[cfg(feature = "tray")]
pub use tray::{
    Tray, TrayBackend, TrayButton, TrayButtonState, TrayClick, TrayError, TrayMenuItem,
};

/// Why an autostart operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AutostartError {
    #[error("autostart error: {0}")]
    AutoLaunch(#[from] auto_launch::Error),
}

/// Controls whether an app is launched at login.
///
/// The backing mechanism is delegated to `auto-launch`: an XDG autostart desktop entry on Linux, a
/// Launch Agent on macOS, and a per-user registry entry on Windows. Auro always uses the
/// least-privileged mode (`CurrentUser`) on Windows.
#[derive(Clone, Debug)]
pub struct Autostart {
    inner: AutoLaunch,
}

impl Autostart {
    /// Creates an autostart service for the given app.
    ///
    /// `app_path` must be absolute and exist on the platform. `args` are passed to the app at
    /// login. On macOS this uses a Launch Agent, not AppleScript login items.
    pub fn new(
        app_name: impl Into<String>,
        app_path: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError> {
        let app_name = app_name.into();
        let app_path = app_path.into();
        let mut builder = AutoLaunchBuilder::new();
        builder
            .set_app_name(&app_name)
            .set_app_path(&app_path)
            .set_linux_launch_mode(auto_launch::LinuxLaunchMode::XdgAutostart)
            .set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent)
            .set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
        if !args.is_empty() {
            builder.set_args(args);
        }
        Ok(Autostart {
            inner: builder.build()?,
        })
    }

    /// Creates an autostart service for the running executable.
    pub fn for_current_app(
        app_name: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError> {
        let path = std::env::current_exe().map_err(|error| {
            auto_launch::Error::Io(std::io::Error::other(format!(
                "cannot resolve the current executable: {error}"
            )))
        })?;
        Self::new(app_name, path.to_string_lossy().into_owned(), args)
    }

    /// Enables launching the app at login.
    pub fn enable(&self) -> Result<(), AutostartError> {
        self.inner.enable()?;
        Ok(())
    }

    /// Disables launching the app at login.
    pub fn disable(&self) -> Result<(), AutostartError> {
        self.inner.disable()?;
        Ok(())
    }

    /// Whether the app is currently launched at login.
    pub fn is_enabled(&self) -> Result<bool, AutostartError> {
        Ok(self.inner.is_enabled()?)
    }
}

/// Convenience alias for the platform path type used by [`Autostart::new`].
pub type AppPath = Path;

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;

    fn app_path() -> Result<std::path::PathBuf, AutostartError> {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("auro-autostart-test");
        std::fs::write(&path, b"placeholder").expect("write");
        // `tempdir` removes itself on drop; the app only has to exist for `enable`.
        std::mem::forget(dir);
        Ok(path)
    }

    fn unique_name() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        format!("auro-autostart-test-{stamp}-{}", std::process::id())
    }

    #[test]
    fn reports_disabled_by_default() {
        let path = app_path().expect("path");
        let autostart =
            Autostart::new(unique_name(), path.to_string_lossy(), &["--quiet"]).expect("autostart");
        assert!(!autostart.is_enabled().expect("enabled"));
    }

    #[test]
    fn enables_and_disables_the_service() {
        let path = app_path().expect("path");
        let autostart =
            Autostart::new(unique_name(), path.to_string_lossy(), &["--quiet"]).expect("autostart");

        autostart.enable().expect("enable");
        assert!(autostart.is_enabled().expect("enabled"));

        autostart.disable().expect("disable");
        assert!(!autostart.is_enabled().expect("enabled"));
    }
}
