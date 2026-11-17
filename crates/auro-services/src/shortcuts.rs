// SPDX-License-Identifier: MIT OR Apache-2.0

//! Global keyboard shortcuts.
//!
//! The backend is pluggable so tests can record register/unregister calls instead of grabbing an
//! OS-level hotkey. The default backend is `global-hotkey`.

use std::sync::Arc;

use thiserror::Error;

/// Why a shortcut operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ShortcutsError {
    #[error("shortcut backend error: {0}")]
    Backend(#[from] global_hotkey::Error),

    #[error("invalid hotkey `{0}`: {1}")]
    HotKeyParse(String, #[source] global_hotkey::hotkey::HotKeyParseError),
}

/// A backend that can register and unregister global hotkeys.
pub trait ShortcutBackend: Send + Sync {
    fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;
}

struct GlobalHotkeys {
    manager: Arc<global_hotkey::GlobalHotKeyManager>,
}

impl GlobalHotkeys {
    fn new() -> Result<Self, ShortcutsError> {
        Ok(GlobalHotkeys {
            manager: Arc::new(global_hotkey::GlobalHotKeyManager::new()?),
        })
    }
}

impl ShortcutBackend for GlobalHotkeys {
    fn register(&self, hotkey: &str) -> Result<(), ShortcutsError> {
        self.manager.register(parse_hotkey(hotkey)?)?;
        Ok(())
    }

    fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError> {
        self.manager.unregister(parse_hotkey(hotkey)?)?;
        Ok(())
    }
}

fn parse_hotkey(hotkey: &str) -> Result<global_hotkey::hotkey::HotKey, ShortcutsError> {
    hotkey
        .parse()
        .map_err(|error| ShortcutsError::HotKeyParse(hotkey.to_owned(), error))
}

/// Registers global keyboard shortcuts.
#[derive(Clone)]
pub struct Shortcuts {
    backend: Arc<dyn ShortcutBackend>,
}

impl Shortcuts {
    /// Creates a shortcuts service with the platform backend.
    pub fn new() -> Result<Self, ShortcutsError> {
        Ok(Self::with_backend(GlobalHotkeys::new()?))
    }

    /// Creates a shortcuts service with the given backend.
    pub fn with_backend(backend: impl ShortcutBackend + 'static) -> Self {
        Shortcuts {
            backend: Arc::new(backend),
        }
    }

    /// Registers a global shortcut.
    pub fn register(&self, hotkey: &str) -> Result<(), ShortcutsError> {
        self.backend.register(hotkey)
    }

    /// Unregisters a global shortcut.
    pub fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError> {
        self.backend.unregister(hotkey)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct RecordingBackend {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            RecordingBackend::default()
        }
    }

    impl ShortcutBackend for RecordingBackend {
        fn register(&self, hotkey: &str) -> Result<(), ShortcutsError> {
            self.calls
                .lock()
                .expect("backend")
                .push(("register".to_owned(), hotkey.to_owned()));
            Ok(())
        }

        fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError> {
            self.calls
                .lock()
                .expect("backend")
                .push(("unregister".to_owned(), hotkey.to_owned()));
            Ok(())
        }
    }

    #[test]
    fn registers_and_unregisters_the_hotkey() {
        let backend = RecordingBackend::new();
        let shortcuts = Shortcuts::with_backend(backend.clone());

        shortcuts.register("shift+alt+k").expect("register");
        shortcuts.unregister("shift+alt+k").expect("unregister");

        let calls = backend.calls.lock().expect("backend");
        assert_eq!(
            calls.as_slice(),
            &[
                ("register".to_owned(), "shift+alt+k".to_owned()),
                ("unregister".to_owned(), "shift+alt+k".to_owned())
            ]
        );
    }

    #[test]
    fn backend_errors_surface() {
        struct FailingBackend;

        impl ShortcutBackend for FailingBackend {
            fn register(&self, _hotkey: &str) -> Result<(), ShortcutsError> {
                Err(global_hotkey::Error::FailedToRegister("shift+alt+k".to_owned()).into())
            }

            fn unregister(&self, _hotkey: &str) -> Result<(), ShortcutsError> {
                Err(global_hotkey::Error::FailedToRegister("shift+alt+k".to_owned()).into())
            }
        }

        let shortcuts = Shortcuts::with_backend(FailingBackend);
        assert!(shortcuts.register("shift+alt+k").is_err());
        assert!(shortcuts.unregister("shift+alt+k").is_err());
    }

    #[test]
    fn rejects_an_invalid_hotkey() {
        assert!(matches!(
            parse_hotkey("shift+alt+unsupported-key"),
            Err(ShortcutsError::HotKeyParse(_, _))
        ));
    }
}
