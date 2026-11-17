// SPDX-License-Identifier: MIT OR Apache-2.0

//! System tray icons.
//!
//! The backend is pluggable so tests can inspect visibility and tooltip changes instead of creating
//! a real tray icon. The default backend is `tray-icon`.

use std::sync::Arc;

use thiserror::Error;

/// Why a tray operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrayError {
    #[error("tray backend error: {0}")]
    Backend(#[from] tray_icon::Error),
}

/// A backend that can show and hide a tray icon.
pub trait TrayBackend {
    fn set_visible(&self, visible: bool) -> Result<(), TrayError>;
    fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError>;
    fn is_visible(&self) -> bool;
}

struct PlatformTray {
    icon: tray_icon::TrayIcon,
    visible: std::sync::atomic::AtomicBool,
}

impl PlatformTray {
    fn new() -> Result<Self, TrayError> {
        let icon = tray_icon::TrayIconBuilder::new()
            .with_tooltip("Auro")
            .build()?;
        Ok(PlatformTray {
            icon,
            visible: std::sync::atomic::AtomicBool::new(true),
        })
    }
}

impl TrayBackend for PlatformTray {
    fn set_visible(&self, visible: bool) -> Result<(), TrayError> {
        self.icon.set_visible(visible)?;
        self.visible
            .store(visible, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError> {
        self.icon.set_tooltip(tooltip)?;
        Ok(())
    }

    fn is_visible(&self) -> bool {
        self.visible.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A tray icon and its tooltip.
#[derive(Clone)]
pub struct Tray {
    backend: Arc<dyn TrayBackend>,
}

impl Tray {
    /// Creates a tray icon with the platform backend.
    pub fn new() -> Result<Self, TrayError> {
        Ok(Tray::with_backend(PlatformTray::new()?))
    }

    /// Creates a tray icon backed by the given backend.
    pub fn with_backend(backend: impl TrayBackend + 'static) -> Self {
        Tray {
            backend: Arc::new(backend),
        }
    }

    /// Shows or hides the tray icon.
    pub fn set_visible(&self, visible: bool) -> Result<(), TrayError> {
        self.backend.set_visible(visible)
    }

    /// Sets or clears the tray tooltip.
    pub fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError> {
        self.backend.set_tooltip(tooltip)
    }

    /// Whether the tray icon is currently visible.
    pub fn is_visible(&self) -> bool {
        self.backend.is_visible()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct RecordingBackend {
        state: Arc<Mutex<(bool, Option<String>)>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            RecordingBackend::default()
        }
    }

    impl TrayBackend for RecordingBackend {
        fn set_visible(&self, visible: bool) -> Result<(), TrayError> {
            self.state.lock().expect("backend").0 = visible;
            Ok(())
        }

        fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError> {
            self.state.lock().expect("backend").1 = tooltip;
            Ok(())
        }

        fn is_visible(&self) -> bool {
            self.state.lock().expect("backend").0
        }
    }

    #[test]
    fn shows_and_hides_the_icon() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend);

        tray.set_visible(false).expect("hide");
        assert!(!tray.is_visible());
        tray.set_visible(true).expect("show");
        assert!(tray.is_visible());
    }

    #[test]
    fn updates_the_tooltip() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend.clone());

        tray.set_tooltip(Some("Ready".to_owned())).expect("tooltip");
        assert_eq!(
            backend.state.lock().expect("backend").1.as_deref(),
            Some("Ready")
        );
        tray.set_tooltip(None).expect("clear tooltip");
        assert_eq!(backend.state.lock().expect("backend").1, None);
    }

    #[test]
    fn backend_errors_surface() {
        struct FailingBackend;

        impl TrayBackend for FailingBackend {
            fn set_visible(&self, _visible: bool) -> Result<(), TrayError> {
                Err(tray_icon::Error::NotMainThread.into())
            }

            fn set_tooltip(&self, _tooltip: Option<String>) -> Result<(), TrayError> {
                Err(tray_icon::Error::NotMainThread.into())
            }

            fn is_visible(&self) -> bool {
                false
            }
        }

        let tray = Tray::with_backend(FailingBackend);
        assert!(tray.set_visible(true).is_err());
        assert!(tray.set_tooltip(None).is_err());
    }
}
