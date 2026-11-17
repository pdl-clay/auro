// SPDX-License-Identifier: MIT OR Apache-2.0

//! Desktop notifications.
//!
//! The backend is pluggable so tests can inject a recorder instead of showing a real notification.
//! The default backend is `notify-rust`, which maps to D-Bus on Linux, NSUserNotificationCenter on
//! macOS and Windows toast notifications.

use std::sync::Arc;

use thiserror::Error;

/// Why a notification failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NotificationsError {
    #[error("notification backend error: {0}")]
    Backend(#[from] notify_rust::error::Error),
}

/// A backend that can show a desktop notification.
pub trait NotificationsBackend: Send + Sync {
    fn show(&self, title: &str, body: &str) -> Result<(), NotificationsError>;
}

struct DesktopNotifications;

impl NotificationsBackend for DesktopNotifications {
    fn show(&self, title: &str, body: &str) -> Result<(), NotificationsError> {
        notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show()?;
        Ok(())
    }
}

/// Sends desktop notifications.
#[derive(Clone)]
pub struct Notifications {
    backend: Arc<dyn NotificationsBackend>,
}

impl Notifications {
    /// Creates a notifications service with the platform backend.
    pub fn new() -> Self {
        Self::with_backend(DesktopNotifications)
    }

    /// Creates a notifications service with the given backend.
    pub fn with_backend(backend: impl NotificationsBackend + 'static) -> Self {
        Notifications {
            backend: Arc::new(backend),
        }
    }

    /// Shows a notification with the given title and body.
    pub fn show(
        &self,
        title: impl AsRef<str>,
        body: impl AsRef<str>,
    ) -> Result<(), NotificationsError> {
        self.backend.show(title.as_ref(), body.as_ref())
    }
}

impl Default for Notifications {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone)]
    struct RecordingBackend {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            RecordingBackend {
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl NotificationsBackend for RecordingBackend {
        fn show(&self, title: &str, body: &str) -> Result<(), NotificationsError> {
            self.calls
                .lock()
                .expect("backend")
                .push((title.to_owned(), body.to_owned()));
            Ok(())
        }
    }

    #[test]
    fn sends_the_title_and_body_to_the_backend() {
        let backend = RecordingBackend::new();
        let notifications = Notifications::with_backend(backend.clone());

        notifications.show("Hello", "World").expect("notification");

        let calls = backend.calls.lock().expect("backend");
        assert_eq!(
            calls.as_slice(),
            &[("Hello".to_owned(), "World".to_owned())]
        );
    }

    #[test]
    fn backend_errors_surface() {
        struct FailingBackend;

        impl NotificationsBackend for FailingBackend {
            fn show(&self, _title: &str, _body: &str) -> Result<(), NotificationsError> {
                Err(NotificationsError::Backend(
                    notify_rust::error::Error::from("no daemon"),
                ))
            }
        }

        let notifications = Notifications::with_backend(FailingBackend);
        assert!(notifications.show("Hello", "World").is_err());
    }
}
