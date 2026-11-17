// SPDX-License-Identifier: MIT OR Apache-2.0

//! Main-thread window management by the labels declared in `Auro.toml`.
//!
//! The manager is installed by `App::run`; it is not available before the GPUI application starts.
//! See spec 0009.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use auro_core::{AuroError, Config, LifecycleEvent};
use auro_ui::{AnyWindowHandle, App as GpuiApp, Global};

use crate::app::{Lifecycle, OpenWindow};

pub(crate) struct WindowManager {
    config: Arc<Config>,
    roots: Vec<(&'static str, OpenWindow)>,
    lifecycle: Rc<Lifecycle>,
    open: Rc<RefCell<Vec<(AnyWindowHandle, String)>>>,
}

impl Global for WindowManager {}

impl WindowManager {
    pub(crate) fn new(
        config: Config,
        roots: Vec<(&'static str, OpenWindow)>,
        lifecycle: Rc<Lifecycle>,
        open: Rc<RefCell<Vec<(AnyWindowHandle, String)>>>,
    ) -> Self {
        WindowManager {
            config: Arc::new(config),
            roots,
            lifecycle,
            open,
        }
    }
}

/// Opens the configured window, or returns its existing handle.
///
/// The window is created through the root registered for the label. A label that is not declared,
/// has no root, or could not be opened is `AuroError::Window`. Opening an already-open window is
/// idempotent and emits no second `WindowOpened`.
pub fn open(label: &str, cx: &mut GpuiApp) -> Result<AnyWindowHandle, AuroError> {
    let (root, window, config, lifecycle, open) = {
        let manager = cx.global::<WindowManager>();
        let window = manager
            .config
            .app
            .windows
            .iter()
            .find(|window| window.label == label)
            .cloned()
            .ok_or_else(|| AuroError::window(label, "not declared in Auro.toml"))?;
        let root = manager
            .roots
            .iter()
            .find(|(root_label, _)| *root_label == label)
            .map(|(_, root)| Rc::clone(root))
            .ok_or_else(|| AuroError::window(label, "no root view attached"))?;
        (
            root,
            window,
            Arc::clone(&manager.config),
            Rc::clone(&manager.lifecycle),
            Rc::clone(&manager.open),
        )
    };

    if let Some(handle) = open
        .borrow()
        .iter()
        .find(|(_, open_label)| open_label == label)
        .map(|(handle, _)| *handle)
    {
        if cx.windows().contains(&handle) {
            return Ok(handle);
        }
        open.borrow_mut().retain(|(known, _)| *known != handle);
    }

    let handle = root(cx, &config, &window)
        .ok_or_else(|| AuroError::window(label, "the window could not be opened"))?;

    open.borrow_mut().push((handle, label.to_owned()));
    lifecycle.emit(
        &LifecycleEvent::WindowOpened {
            label: label.to_owned(),
        },
        cx,
    );
    Ok(handle)
}

/// Closes the configured window. Returns false when the label is unknown or not open.
///
/// The close observer owns `WindowClosed`/`Exiting`; this function does not duplicate them.
pub fn close(label: &str, cx: &mut GpuiApp) -> bool {
    let handle = {
        let manager = cx.global::<WindowManager>();
        manager
            .open
            .borrow()
            .iter()
            .find(|(_, open_label)| open_label == label)
            .map(|(handle, _)| *handle)
    };
    let Some(handle) = handle else {
        return false;
    };
    if !cx.windows().contains(&handle) {
        return false;
    }
    handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok()
}

/// Activates the configured window. Returns false when it is not open.
pub fn focus(label: &str, cx: &mut GpuiApp) -> bool {
    let handle = {
        let manager = cx.global::<WindowManager>();
        manager
            .open
            .borrow()
            .iter()
            .find(|(_, open_label)| open_label == label)
            .map(|(handle, _)| *handle)
    };
    let Some(handle) = handle else {
        return false;
    };
    if !cx.windows().contains(&handle) {
        return false;
    }
    handle
        .update(cx, |_, window, _| window.activate_window())
        .is_ok()
}

/// Whether the configured window is currently open.
pub fn is_open(label: &str, cx: &GpuiApp) -> bool {
    let manager = cx.global::<WindowManager>();
    let open = cx.windows();
    manager
        .open
        .borrow()
        .iter()
        .any(|(handle, open_label)| open_label == label && open.contains(handle))
}

/// The current handle for the configured window, if it is open.
pub fn handle(label: &str, cx: &GpuiApp) -> Option<AnyWindowHandle> {
    let manager = cx.global::<WindowManager>();
    let open = cx.windows();
    manager
        .open
        .borrow()
        .iter()
        .find(|(handle, open_label)| open_label == label && open.contains(handle))
        .map(|(handle, _)| *handle)
}
