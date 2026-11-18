// SPDX-License-Identifier: MIT OR Apache-2.0

//! Global keyboard shortcuts.
//!
//! The backend is pluggable so tests can record register/unregister calls and invoke callbacks
//! instead of grabbing an OS-level hotkey. The default backend is `global-hotkey` and dispatches
//! pressed events on a background thread.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use thiserror::Error;

/// Why a shortcut operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ShortcutsError {
    #[error("shortcut backend error: {0}")]
    Backend(#[from] global_hotkey::Error),

    #[error("invalid hotkey `{0}`: {1}")]
    HotKeyParse(String, #[source] global_hotkey::hotkey::HotKeyParseError),

    #[error("shortcut event loop failed: {0}")]
    EventLoop(#[from] std::io::Error),
}

type ShortcutHandler = Arc<dyn Fn() + Send + Sync>;
type ShortcutHandlers = HashMap<u32, Vec<ShortcutHandler>>;

/// A backend that can register and unregister global hotkeys and deliver their callbacks.
pub trait ShortcutBackend {
    fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;

    /// Registers a callback for a pressed hotkey. The callback runs off the app thread.
    fn on_shortcut(&self, hotkey: &str, handler: Arc<dyn Fn() + Send + Sync>);
}

#[derive(Default)]
struct ShortcutDispatcher {
    handlers: Mutex<ShortcutHandlers>,
}

impl ShortcutDispatcher {
    fn on_shortcut(&self, id: u32, handler: ShortcutHandler) {
        self.handlers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .entry(id)
            .or_default()
            .push(handler);
    }

    fn dispatch(&self, id: u32) {
        let handlers = self
            .handlers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&id)
            .cloned()
            .unwrap_or_default();
        for handler in handlers {
            handler();
        }
    }
}

struct GlobalHotkeys {
    manager: Rc<global_hotkey::GlobalHotKeyManager>,
    dispatcher: Arc<ShortcutDispatcher>,
}

impl GlobalHotkeys {
    fn new() -> Result<Self, ShortcutsError> {
        let manager = Rc::new(global_hotkey::GlobalHotKeyManager::new()?);
        let dispatcher = Arc::new(ShortcutDispatcher::default());
        let worker = Arc::clone(&dispatcher);
        thread::Builder::new()
            .name("auro-shortcuts".to_owned())
            .spawn(move || {
                while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().recv() {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        worker.dispatch(event.id);
                    }
                }
            })
            .map_err(ShortcutsError::EventLoop)?;
        Ok(GlobalHotkeys {
            manager,
            dispatcher,
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

    fn on_shortcut(&self, hotkey: &str, handler: ShortcutHandler) {
        if let Ok(parsed) = parse_hotkey(hotkey) {
            self.dispatcher.on_shortcut(parsed.id(), handler);
        }
    }
}

fn parse_hotkey(hotkey: &str) -> Result<global_hotkey::hotkey::HotKey, ShortcutsError> {
    hotkey
        .parse()
        .map_err(|error| ShortcutsError::HotKeyParse(hotkey.to_owned(), error))
}

/// Registers global keyboard shortcuts and their callbacks.
#[derive(Clone)]
pub struct Shortcuts {
    backend: Rc<dyn ShortcutBackend>,
}

impl Shortcuts {
    /// Creates a shortcuts service with the platform backend.
    pub fn new() -> Result<Self, ShortcutsError> {
        Ok(Self::with_backend(GlobalHotkeys::new()?))
    }

    /// Creates a shortcuts service with the given backend.
    pub fn with_backend(backend: impl ShortcutBackend + 'static) -> Self {
        Shortcuts {
            backend: Rc::new(backend),
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

    /// Registers a callback for a pressed hotkey.
    ///
    /// The callback runs on the platform event thread; use an [`EventSender`] or another channel
    /// to move work onto the app thread. Register the hotkey separately with [`Self::register`].
    ///
    /// [`EventSender`]: https://docs.rs/auro-core/latest/auro_core/struct.EventSender.html
    pub fn on_shortcut<F>(&self, hotkey: &str, handler: F) -> Result<(), ShortcutsError>
    where
        F: Fn() + Send + Sync + 'static,
    {
        parse_hotkey(hotkey)?;
        self.backend.on_shortcut(hotkey, Arc::new(handler));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    type TestHandlers = HashMap<String, Vec<ShortcutHandler>>;

    #[derive(Clone, Default)]
    struct RecordingBackend {
        calls: Arc<Mutex<Vec<(String, String)>>>,
        handlers: Arc<Mutex<TestHandlers>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            RecordingBackend::default()
        }

        fn fire(&self, hotkey: &str) {
            let handlers = self
                .handlers
                .lock()
                .expect("backend")
                .get(hotkey)
                .cloned()
                .unwrap_or_default();
            for handler in handlers {
                handler();
            }
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

        fn on_shortcut(&self, hotkey: &str, handler: ShortcutHandler) {
            self.handlers
                .lock()
                .expect("backend")
                .entry(hotkey.to_owned())
                .or_default()
                .push(handler);
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
    fn dispatches_a_pressed_hotkey() {
        let backend = RecordingBackend::new();
        let shortcuts = Shortcuts::with_backend(backend.clone());
        let fired = Arc::new(AtomicUsize::new(0));

        shortcuts
            .on_shortcut("shift+alt+k", {
                let fired = Arc::clone(&fired);
                move || {
                    fired.fetch_add(1, Ordering::SeqCst);
                }
            })
            .expect("callback");
        backend.fire("shift+alt+k");
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_an_invalid_callback_hotkey() {
        let shortcuts = Shortcuts::with_backend(RecordingBackend::new());
        assert!(matches!(
            shortcuts.on_shortcut("shift+alt+unsupported-key", || {}),
            Err(ShortcutsError::HotKeyParse(_, _))
        ));
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

            fn on_shortcut(&self, _hotkey: &str, _handler: ShortcutHandler) {}
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
