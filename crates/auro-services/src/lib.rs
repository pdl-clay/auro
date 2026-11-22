// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform services Auro provides beyond the renderer.
//!
//! This crate is the home for OS integrations that are not GPUI-specific. It starts with
//! autostart; notifications, keychain, tray, global shortcuts and deep links land here as they are
//! specced and implemented.

use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(not(target_os = "linux"))]
use auto_launch::{AutoLaunch, AutoLaunchBuilder};

pub mod deep_link;
mod desktop_entry;
pub mod keychain;
pub mod notifications;
pub mod shortcuts;
#[cfg(feature = "tray")]
pub mod tray;
pub use deep_link::{AppLinks, DeepLink, DeepLinkError, register_scheme};
pub use keychain::{CredentialStore, Keychain, KeychainError};
pub use notifications::{Notifications, NotificationsBackend, NotificationsError};
pub use shortcuts::{ShortcutBackend, Shortcuts, ShortcutsError};
#[cfg(feature = "tray")]
pub use tray::{
    Tray, TrayBackend, TrayButton, TrayButtonState, TrayClick, TrayError, TrayIcon, TrayMenuItem,
};

/// Why an autostart operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AutostartError {
    #[error("autostart error: {0}")]
    AutoLaunch(#[from] auto_launch::Error),

    #[error("invalid autostart {field}: {message}")]
    Invalid {
        field: &'static str,
        message: String,
    },

    #[error("autostart I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Controls whether an app is launched at login.
///
/// Linux writes the XDG autostart entry itself so `XDG_CONFIG_HOME` and Desktop Entry escaping are
/// honored; macOS and Windows delegate the Launch Agent/registry mechanics to `auto-launch`, which
/// always uses the least-privileged mode (`CurrentUser`) on Windows.
#[derive(Clone, Debug)]
pub struct Autostart {
    #[cfg(target_os = "linux")]
    linux: LinuxAutostart,
    #[cfg(not(target_os = "linux"))]
    inner: AutoLaunch,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct LinuxAutostart {
    app_name: String,
    app_path: String,
    args: Vec<String>,
}

impl Autostart {
    /// Creates an autostart service for the given app.
    ///
    /// `app_path` must be absolute and exist. `args` are passed to the app at login. On macOS this
    /// uses a Launch Agent, not AppleScript login items.
    pub fn new(
        app_name: impl Into<String>,
        app_path: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError> {
        let app_name = app_name.into();
        desktop_entry::validate_app_name(&app_name).map_err(|message| AutostartError::Invalid {
            field: "app_name",
            message,
        })?;

        let app_path = app_path.into();
        validate_app_path(&app_path)?;
        let args: Vec<String> = args
            .iter()
            .map(|argument| argument.as_ref().to_owned())
            .collect();

        #[cfg(target_os = "linux")]
        {
            Ok(Autostart {
                linux: LinuxAutostart {
                    app_name,
                    app_path,
                    args,
                },
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut builder = AutoLaunchBuilder::new();
            builder
                .set_app_name(&app_name)
                .set_app_path(&app_path)
                .set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent)
                .set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
            if !args.is_empty() {
                builder.set_args(&args);
            }
            Ok(Autostart {
                inner: builder.build()?,
            })
        }
    }

    /// Creates an autostart service for the running executable.
    pub fn for_current_app(
        app_name: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError> {
        let path = autostart_program(std::env::var_os("APPIMAGE")).map_err(|error| {
            auto_launch::Error::Io(std::io::Error::other(format!(
                "cannot resolve the current executable: {error}"
            )))
        })?;
        Self::new(app_name, path.to_string_lossy().into_owned(), args)
    }

    /// Enables launching the app at login.
    pub fn enable(&self) -> Result<(), AutostartError> {
        #[cfg(target_os = "linux")]
        {
            let base = linux_autostart_base()?;
            let directory = desktop_entry::ensure_user_directory(&base, "autostart")?;
            let path = self.linux.path_in(&directory);
            desktop_entry::write_atomic(&path, &self.linux.entry()?)?;
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.inner.enable()?;
            Ok(())
        }
    }

    /// Disables launching the app at login.
    pub fn disable(&self) -> Result<(), AutostartError> {
        #[cfg(target_os = "linux")]
        {
            match std::fs::remove_file(self.linux.path()?) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.inner.disable()?;
            Ok(())
        }
    }

    /// Whether the app is currently launched at login.
    pub fn is_enabled(&self) -> Result<bool, AutostartError> {
        #[cfg(target_os = "linux")]
        {
            Ok(self.linux.path()?.is_file())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(self.inner.is_enabled()?)
        }
    }
}

fn validate_app_path(app_path: &str) -> Result<(), AutostartError> {
    let path = Path::new(app_path);
    if !path.is_absolute() {
        return Err(AutostartError::Invalid {
            field: "app_path",
            message: "must be an absolute path".to_owned(),
        });
    }
    if !path.exists() {
        return Err(AutostartError::Invalid {
            field: "app_path",
            message: format!("`{app_path}` does not exist"),
        });
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if !path.is_file() {
            return Err(AutostartError::Invalid {
                field: "app_path",
                message: format!("`{app_path}` is not a file"),
            });
        }
        let executable = path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
        if !executable {
            return Err(AutostartError::Invalid {
                field: "app_path",
                message: format!("`{app_path}` is not executable"),
            });
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
impl LinuxAutostart {
    fn path(&self) -> Result<PathBuf, AutostartError> {
        Ok(self.path_in(&linux_autostart_dir()?))
    }

    fn path_in(&self, directory: &Path) -> PathBuf {
        directory.join(format!("{}.desktop", self.app_name))
    }

    fn entry(&self) -> Result<String, AutostartError> {
        let exec = desktop_entry::quote_exec_arg(&self.app_path).map_err(|message| {
            AutostartError::Invalid {
                field: "app_path",
                message,
            }
        })?;
        let args = self
            .args
            .iter()
            .map(|argument| {
                desktop_entry::quote_exec_arg(argument).map_err(|message| AutostartError::Invalid {
                    field: "args",
                    message,
                })
            })
            .collect::<Result<Vec<_>, AutostartError>>()?
            .join(" ");
        let exec = if args.is_empty() {
            exec
        } else {
            format!("{exec} {args}")
        };
        Ok(format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nComment={} startup script\nExec={exec}\nStartupNotify=false\nTerminal=false\n",
            self.app_name, self.app_name
        ))
    }
}

#[cfg(target_os = "linux")]
fn linux_autostart_dir() -> Result<PathBuf, AutostartError> {
    linux_autostart_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

#[cfg(target_os = "linux")]
fn linux_autostart_dir_from(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, AutostartError> {
    Ok(linux_autostart_base_from(xdg_config_home, home)?.join("autostart"))
}

/// The XDG config base the `autostart` directory lives under.
///
/// `enable` needs the base separately so it can create and validate `~/.config/autostart`
/// without duplicating the XDG resolution rules.
#[cfg(target_os = "linux")]
fn linux_autostart_base() -> Result<PathBuf, AutostartError> {
    linux_autostart_base_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

#[cfg(target_os = "linux")]
fn linux_autostart_base_from(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, AutostartError> {
    if let Some(xdg) = xdg_config_home.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(xdg);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    let home = home
        .filter(|value| !value.is_empty())
        .ok_or(AutostartError::Invalid {
            field: "HOME",
            message: "XDG_CONFIG_HOME is unset and HOME cannot be resolved".to_owned(),
        })?;
    Ok(PathBuf::from(home).join(".config"))
}

/// The stable executable an autostart entry should launch.
///
/// Inside an AppImage, `current_exe()` points at the temporary squashfs mount, which disappears
/// when the process exits; `$APPIMAGE` names the file the user launched.
fn autostart_program(
    appimage: Option<std::ffi::OsString>,
) -> Result<std::path::PathBuf, std::io::Error> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = appimage.filter(|path| !path.is_empty()) {
        return Ok(std::path::PathBuf::from(appimage));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = appimage;
    std::env::current_exe()
}

/// Convenience alias for the platform path type used by [`Autostart::new`].
pub type AppPath = Path;

/// Helpers for tests that change process-wide state.
///
/// The umask and environment are process-global, so every test that touches them holds the same
/// lock; otherwise two tests could observe each other's mask or XDG paths.
#[cfg(test)]
mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static PROCESS_STATE: Mutex<()> = Mutex::new(());

    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        PROCESS_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(target_os = "linux")]
    pub(crate) struct UmaskGuard(libc::mode_t);

    #[cfg(target_os = "linux")]
    impl UmaskGuard {
        pub(crate) fn set(mask: libc::mode_t) -> Self {
            // SAFETY: `umask` has no preconditions; the previous mask is restored on drop while
            // the caller still holds the process-state lock.
            Self(unsafe { libc::umask(mask) })
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            // SAFETY: restoring the mask captured by `set`.
            unsafe {
                libc::umask(self.0);
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(target_os = "linux")]
    impl EnvGuard {
        pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: the caller holds the process-state lock, so no other test reads or writes
            // the environment concurrently.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same lock invariant as `set`.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;

    fn app_path() -> Result<std::path::PathBuf, AutostartError> {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("auro-autostart-test");
        std::fs::write(&path, b"placeholder").expect("write");
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable mode");
        }
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
        let _lock = crate::test_support::lock();
        let path = app_path().expect("path");
        let autostart =
            Autostart::new(unique_name(), path.to_string_lossy(), &["--quiet"]).expect("autostart");
        assert!(!autostart.is_enabled().expect("enabled"));
    }

    #[test]
    fn rejects_an_app_name_that_escapes_the_autostart_directory() {
        assert!(matches!(
            Autostart::new("../escaped", "/opt/app", &[] as &[&str]),
            Err(AutostartError::Invalid { .. })
        ));
    }

    #[test]
    fn appimage_path_is_preferred_for_the_current_app() {
        #[cfg(target_os = "linux")]
        {
            let program = autostart_program(Some(std::ffi::OsString::from("/opt/Probe.AppImage")))
                .expect("program");
            assert_eq!(program, std::path::PathBuf::from("/opt/Probe.AppImage"));
        }
        #[cfg(not(target_os = "linux"))]
        {
            let program = autostart_program(None).expect("program");
            assert!(program.is_absolute());
        }
    }

    #[test]
    fn rejects_a_relative_or_missing_app_path() {
        for path in ["relative-app", "/definitely/missing/auro-app"] {
            let error = Autostart::new(unique_name(), path, &[] as &[&str])
                .expect_err("app_path must be absolute and exist");
            assert!(
                matches!(
                    error,
                    AutostartError::Invalid {
                        field: "app_path",
                        ..
                    }
                ),
                "{error}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_a_non_executable_app_path() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempdir().expect("tempdir");
        let app = directory.path().join("not-executable");
        std::fs::write(&app, b"placeholder").expect("app");
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o644)).expect("mode");
        let error = Autostart::new(unique_name(), app.to_string_lossy(), &[] as &[&str])
            .expect_err("app_path must be executable");
        assert!(
            matches!(
                error,
                AutostartError::Invalid {
                    field: "app_path",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_entry_quotes_paths_and_arguments_with_spaces() {
        let _lock = crate::test_support::lock();
        let name = unique_name();
        let directory = tempdir().expect("tempdir");
        let app = directory.path().join("my app");
        std::fs::write(&app, b"placeholder").expect("app");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755))
                .expect("executable mode");
        }
        let autostart = Autostart::new(
            &name,
            app.to_string_lossy().into_owned(),
            &["--title", "a b"],
        )
        .expect("autostart");
        autostart.enable().expect("enable");

        let entry = linux_autostart_dir()
            .expect("autostart dir")
            .join(format!("{name}.desktop"));
        let text = std::fs::read_to_string(&entry).expect("entry");
        autostart.disable().expect("disable");

        assert!(
            text.contains(&format!("Exec=\"{}\" \"--title\" \"a b\"", app.display())),
            "{text}"
        );
        assert!(!entry.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn enabling_replaces_a_symlink_without_writing_through_it() {
        use std::os::unix::fs::symlink;

        let _lock = crate::test_support::lock();
        let name = unique_name();
        let path = app_path().expect("path");
        let autostart =
            Autostart::new(&name, path.to_string_lossy(), &[] as &[&str]).expect("autostart");
        let directory = linux_autostart_dir().expect("autostart dir");
        std::fs::create_dir_all(&directory).expect("autostart dir");
        let entry = directory.join(format!("{name}.desktop"));
        let victim_dir = tempdir().expect("tempdir");
        let victim = victim_dir.path().join("victim");
        std::fs::write(&victim, b"SAFE").expect("victim");
        symlink(&victim, &entry).expect("symlink");

        autostart.enable().expect("enable");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"SAFE");
        assert!(
            !std::fs::symlink_metadata(&entry)
                .expect("entry")
                .file_type()
                .is_symlink(),
            "autostart must replace the link with a regular file"
        );
        autostart.disable().expect("disable");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn autostart_dir_honors_xdg_config_home() {
        let xdg = std::path::PathBuf::from("/tmp/auro-xdg-config");
        assert_eq!(
            linux_autostart_dir_from(
                Some(xdg.clone().into_os_string()),
                Some(std::ffi::OsString::from("/tmp/auro-home")),
            )
            .expect("xdg"),
            xdg.join("autostart")
        );

        assert_eq!(
            linux_autostart_dir_from(
                Some(std::ffi::OsString::from("relative")),
                Some(std::ffi::OsString::from("/tmp/auro-home")),
            )
            .expect("fallback"),
            std::path::PathBuf::from("/tmp/auro-home/.config/autostart")
        );

        assert!(linux_autostart_dir_from(None, None).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn autostart_directory_is_created_private_under_umask() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::test_support::{EnvGuard, UmaskGuard};

        let _lock = crate::test_support::lock();
        let xdg = tempdir().expect("tempdir");
        let config = xdg.path().join("config");
        let _umask = UmaskGuard::set(0o002);
        let _xdg = EnvGuard::set("XDG_CONFIG_HOME", &config);

        let path = app_path().expect("path");
        let autostart = Autostart::new(unique_name(), path.to_string_lossy(), &[] as &[&str])
            .expect("autostart");
        autostart.enable().expect("enable");

        let directory = config.join("autostart");
        let mode = std::fs::metadata(&directory)
            .expect("autostart directory")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "the autostart directory must not inherit the umask"
        );
        let base_mode = std::fs::metadata(&config)
            .expect("missing XDG base")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            base_mode, 0o755,
            "a missing XDG base must be created with an explicit mode"
        );
        autostart.disable().expect("disable");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn autostart_refuses_a_group_writable_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::test_support::EnvGuard;

        let _lock = crate::test_support::lock();
        let xdg = tempdir().expect("tempdir");
        let _xdg = EnvGuard::set("XDG_CONFIG_HOME", xdg.path());
        let directory = xdg.path().join("autostart");
        std::fs::create_dir(&directory).expect("autostart");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o775)).expect("mode");

        let path = app_path().expect("path");
        let autostart = Autostart::new(unique_name(), path.to_string_lossy(), &[] as &[&str])
            .expect("autostart");
        let error = autostart
            .enable()
            .expect_err("a group-writable directory must be refused");
        assert!(
            error.to_string().contains(&directory.display().to_string()),
            "{error}"
        );
        assert!(matches!(error, AutostartError::Io(_)), "{error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn autostart_refuses_a_symlinked_directory() {
        use std::os::unix::fs::symlink;

        use crate::test_support::EnvGuard;

        let _lock = crate::test_support::lock();
        let xdg = tempdir().expect("tempdir");
        let _xdg = EnvGuard::set("XDG_CONFIG_HOME", xdg.path());
        let victim = tempdir().expect("tempdir");
        let directory = xdg.path().join("autostart");
        symlink(victim.path(), &directory).expect("symlink");

        let path = app_path().expect("path");
        let autostart = Autostart::new(unique_name(), path.to_string_lossy(), &[] as &[&str])
            .expect("autostart");
        let error = autostart
            .enable()
            .expect_err("a symlinked directory must be refused");
        assert!(
            error.to_string().contains(&directory.display().to_string()),
            "{error}"
        );
        assert!(
            std::fs::read_dir(victim.path())
                .expect("victim directory")
                .next()
                .is_none(),
            "must not write through the symlink"
        );
    }

    #[test]
    fn enables_and_disables_the_service() {
        let _lock = crate::test_support::lock();
        let path = app_path().expect("path");
        let autostart =
            Autostart::new(unique_name(), path.to_string_lossy(), &["--quiet"]).expect("autostart");

        autostart.enable().expect("enable");
        assert!(autostart.is_enabled().expect("enabled"));

        autostart.disable().expect("disable");
        assert!(!autostart.is_enabled().expect("enabled"));
    }
}
