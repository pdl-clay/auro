// SPDX-License-Identifier: MIT OR Apache-2.0

//! A process-wide single-instance guard.
//!
//! The guard is an advisory file lock in a per-user runtime directory. The OS releases the lock
//! when the process exits, including a crash; the lock file itself is left in place. This detects
//! duplicates but does not forward arguments or activate the primary window yet.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::AuroError;

/// Holds the primary-instance lock for as long as it is alive.
#[derive(Debug)]
pub struct AppInstance {
    file: File,
    path: PathBuf,
    name: String,
    primary: bool,
}

impl AppInstance {
    /// Tries to become the primary instance for `name`.
    ///
    /// `name` is normally the app identifier from `Auro.toml`. It is sanitised into a lock-file
    /// name; the lock lives under `XDG_RUNTIME_DIR` on Linux when available, otherwise the
    /// per-user temporary directory. A secondary instance gets `is_primary() == false` rather than
    /// an error.
    pub fn acquire(name: &str) -> Result<Self, AuroError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AuroError::instance("", "empty application identifier"));
        }

        let path = lock_path(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path)?;

        let primary = match file.try_lock() {
            Ok(()) => true,
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Error(error)) => {
                return Err(AuroError::instance(
                    name,
                    format!("cannot lock `{}`: {error}", path.display()),
                ));
            }
        };

        Ok(AppInstance {
            file,
            path,
            name: name.to_owned(),
            primary,
        })
    }

    /// Whether this process owns the lock.
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// The application identifier this guard was acquired for.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The lock file. It is intentionally left on disk after the process exits.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AppInstance {
    fn drop(&mut self) {
        // Closing the file releases the OS lock on every supported platform. Unlock explicitly
        // first so the secondary handle never sees a short window where the old lock still exists.
        let _ = self.file.unlock();
    }
}

fn lock_path(name: &str) -> PathBuf {
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .take(128)
        .collect();
    let sanitized = if sanitized.is_empty() {
        "app".to_owned()
    } else {
        sanitized
    };

    lock_dir().join(format!("auro-{sanitized}.lock"))
}

fn lock_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if runtime.is_dir() {
            return runtime;
        }
    }
    std::env::temp_dir()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique_name() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        format!("com.example.auro-instance-{}-{stamp}", std::process::id())
    }

    #[test]
    fn first_guard_is_primary_and_second_is_not() {
        let name = unique_name();
        let first = AppInstance::acquire(&name).expect("first");
        assert!(first.is_primary());
        assert_eq!(first.name(), name);
        assert!(first.path().is_file());

        let second = AppInstance::acquire(&name).expect("second");
        assert!(!second.is_primary());

        drop(first);
        let third = AppInstance::acquire(&name).expect("third");
        assert!(third.is_primary());
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(matches!(
            AppInstance::acquire("  "),
            Err(AuroError::Instance { name, .. }) if name.is_empty()
        ));
    }

    #[test]
    fn path_is_sanitised() {
        let path = lock_path("../com.example/app");
        let file = path.file_name().and_then(|name| name.to_str()).unwrap();
        assert_eq!(file, "auro-..-com.example-app.lock");
    }
}
