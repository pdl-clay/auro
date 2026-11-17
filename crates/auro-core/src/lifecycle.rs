// SPDX-License-Identifier: MIT OR Apache-2.0

//! Application lifecycle events.
//!
//! The events are emitted on the main thread, in registration order, by the facade. They are
//! informational in Phase 1: a handler can update entities and state, but it cannot cancel exit
//! or veto a window closing.

/// What happened to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LifecycleEvent {
    /// Setups ran and every configured window that has a root was opened.
    Ready,
    /// A window with this label was opened.
    WindowOpened { label: String },
    /// A tracked window with this label was closed.
    WindowClosed { label: String },
    /// The last window closed; the app is about to quit.
    Exiting,
}

impl LifecycleEvent {
    /// The window label for window events, or `None` for app-level events.
    pub fn label(&self) -> Option<&str> {
        match self {
            LifecycleEvent::WindowOpened { label } | LifecycleEvent::WindowClosed { label } => {
                Some(label)
            }
            LifecycleEvent::Ready | LifecycleEvent::Exiting => None,
        }
    }
}
