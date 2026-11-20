// SPDX-License-Identifier: MIT OR Apache-2.0

//! System tray icons, menus and click callbacks.
//!
//! The backend is pluggable so tests can inspect visibility, tooltip and menu changes and invoke
//! callbacks instead of creating a real tray icon. The default backend is `tray-icon`.

use std::sync::Arc;

use thiserror::Error;

/// Why a tray operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrayError {
    #[error("tray backend error: {0}")]
    Backend(#[from] tray_icon::Error),

    #[error("tray menu error: {0}")]
    Menu(#[from] tray_icon::menu::Error),

    #[error("tray icon error: {0}")]
    Icon(#[from] tray_icon::BadIcon),

    #[error("invalid tray icon: {0}")]
    InvalidIcon(&'static str),
}

/// Raw RGBA pixels for a tray icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl TrayIcon {
    /// Creates an icon from tightly packed RGBA pixels.
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, TrayError> {
        if width == 0 || height == 0 {
            return Err(TrayError::InvalidIcon("dimensions must be non-zero"));
        }
        if !rgba.len().is_multiple_of(4) {
            return Err(tray_icon::BadIcon::ByteCountNotDivisibleBy4 {
                byte_count: rgba.len(),
            }
            .into());
        }
        let width_x_height = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(usize::MAX);
        let pixel_count = rgba.len() / 4;
        if pixel_count != width_x_height {
            return Err(tray_icon::BadIcon::DimensionsVsPixelCount {
                width,
                height,
                width_x_height,
                pixel_count,
            }
            .into());
        }
        Ok(TrayIcon {
            rgba,
            width,
            height,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Tightly packed RGBA pixels.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

type MenuHandler = Arc<dyn Fn(&str) + Send + Sync>;
type ClickHandler = Arc<dyn Fn(TrayClick) + Send + Sync>;

/// One item in a tray context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuItem {
    /// Stable identifier delivered to [`Tray::on_menu_event`].
    pub id: String,
    /// Label shown by the platform.
    pub label: String,
    /// Whether the item can be activated.
    pub enabled: bool,
}

impl TrayMenuItem {
    /// Creates an enabled menu item.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        TrayMenuItem {
            id: id.into(),
            label: label.into(),
            enabled: true,
        }
    }

    /// Enables or disables the item.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// The mouse button that produced a tray click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Middle button.
    Middle,
}

/// Whether the tray button is pressed or released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayButtonState {
    /// Button released.
    Up,
    /// Button pressed.
    Down,
}

/// A click on the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayClick {
    /// Button that changed state.
    pub button: TrayButton,
    /// New button state.
    pub state: TrayButtonState,
}

/// A backend that can show and hide a tray icon and deliver its events.
pub trait TrayBackend {
    fn set_visible(&self, visible: bool) -> Result<(), TrayError>;
    fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError>;
    fn is_visible(&self) -> bool;

    /// Replaces the context menu. Some Linux backends only allow the content to change.
    fn set_menu(&self, items: Vec<TrayMenuItem>) -> Result<(), TrayError>;

    /// Installs the menu-event handler. The platform event source is process-global.
    fn set_menu_event_handler(&self, handler: Arc<dyn Fn(&str) + Send + Sync>);

    /// Installs the tray-click handler. The platform event source is process-global.
    fn set_click_event_handler(&self, handler: Arc<dyn Fn(TrayClick) + Send + Sync>);

    /// Replaces or clears the tray icon.
    fn set_icon(&self, icon: Option<TrayIcon>) -> Result<(), TrayError>;
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

    fn set_menu(&self, items: Vec<TrayMenuItem>) -> Result<(), TrayError> {
        use tray_icon::menu::{IsMenuItem, Menu, MenuItem};

        let menu = Menu::new();
        let entries: Vec<MenuItem> = items
            .into_iter()
            .map(|item| MenuItem::with_id(item.id, item.label, item.enabled, None))
            .collect();
        let entries_ref: Vec<&dyn IsMenuItem> =
            entries.iter().map(|item| item as &dyn IsMenuItem).collect();
        menu.append_items(&entries_ref)?;
        self.icon.set_menu(Some(Box::new(menu)));
        Ok(())
    }

    fn set_menu_event_handler(&self, handler: MenuHandler) {
        tray_icon::menu::MenuEvent::set_event_handler(Some(
            move |event: tray_icon::menu::MenuEvent| {
                handler(event.id.as_ref());
            },
        ));
    }

    fn set_icon(&self, icon: Option<TrayIcon>) -> Result<(), TrayError> {
        let icon = icon
            .map(|icon| tray_icon::Icon::from_rgba(icon.rgba, icon.width, icon.height))
            .transpose()?;
        self.icon.set_icon(icon)?;
        Ok(())
    }

    fn set_click_event_handler(&self, handler: ClickHandler) {
        tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
            if let tray_icon::TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                handler(TrayClick {
                    button: match button {
                        tray_icon::MouseButton::Left => TrayButton::Left,
                        tray_icon::MouseButton::Right => TrayButton::Right,
                        tray_icon::MouseButton::Middle => TrayButton::Middle,
                    },
                    state: match button_state {
                        tray_icon::MouseButtonState::Up => TrayButtonState::Up,
                        tray_icon::MouseButtonState::Down => TrayButtonState::Down,
                    },
                });
            }
        }));
    }
}

/// A tray icon, its menu and event callbacks.
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

    /// Replaces the context menu.
    pub fn set_menu(&self, items: &[TrayMenuItem]) -> Result<(), TrayError> {
        self.backend.set_menu(items.to_vec())
    }

    /// Replaces or clears the icon.
    pub fn set_icon(&self, icon: Option<TrayIcon>) -> Result<(), TrayError> {
        self.backend.set_icon(icon)
    }

    /// Registers a menu callback. The id is the one from [`TrayMenuItem::new`].
    ///
    /// The callback runs on the platform event thread; use an [`EventSender`] or another channel
    /// to move work onto the app thread. Only one callback can be installed per process because
    /// `tray-icon` exposes a process-global event handler.
    ///
    /// [`EventSender`]: https://docs.rs/auro-core/latest/auro_core/struct.EventSender.html
    pub fn on_menu_event<F>(&self, handler: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.backend.set_menu_event_handler(Arc::new(handler));
    }

    /// Registers a tray-click callback.
    ///
    /// The callback runs on the platform event thread; use an [`EventSender`] or another channel
    /// to move work onto the app thread. Only one callback can be installed per process because
    /// `tray-icon` exposes a process-global event handler.
    ///
    /// [`EventSender`]: https://docs.rs/auro-core/latest/auro_core/struct.EventSender.html
    pub fn on_click<F>(&self, handler: F)
    where
        F: Fn(TrayClick) + Send + Sync + 'static,
    {
        self.backend.set_click_event_handler(Arc::new(handler));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc;

    use super::*;

    #[derive(Default)]
    struct RecordingState {
        visible: bool,
        tooltip: Option<String>,
        icon: Option<TrayIcon>,
        menu: Vec<TrayMenuItem>,
        menu_handler: Option<MenuHandler>,
        click_handler: Option<ClickHandler>,
    }

    #[derive(Clone, Default)]
    struct RecordingBackend {
        state: Arc<Mutex<RecordingState>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            RecordingBackend::default()
        }

        fn menu(&self) -> Vec<TrayMenuItem> {
            self.state.lock().expect("backend").menu.clone()
        }

        fn fire_menu(&self, id: &str) {
            let handler = self
                .state
                .lock()
                .expect("backend")
                .menu_handler
                .clone()
                .expect("menu handler");
            handler(id);
        }

        fn fire_click(&self, click: TrayClick) {
            let handler = self
                .state
                .lock()
                .expect("backend")
                .click_handler
                .clone()
                .expect("click handler");
            handler(click);
        }
    }

    impl TrayBackend for RecordingBackend {
        fn set_visible(&self, visible: bool) -> Result<(), TrayError> {
            self.state.lock().expect("backend").visible = visible;
            Ok(())
        }

        fn set_tooltip(&self, tooltip: Option<String>) -> Result<(), TrayError> {
            self.state.lock().expect("backend").tooltip = tooltip;
            Ok(())
        }

        fn is_visible(&self) -> bool {
            self.state.lock().expect("backend").visible
        }

        fn set_menu(&self, items: Vec<TrayMenuItem>) -> Result<(), TrayError> {
            self.state.lock().expect("backend").menu = items;
            Ok(())
        }

        fn set_menu_event_handler(&self, handler: MenuHandler) {
            self.state.lock().expect("backend").menu_handler = Some(handler);
        }

        fn set_click_event_handler(&self, handler: ClickHandler) {
            self.state.lock().expect("backend").click_handler = Some(handler);
        }

        fn set_icon(&self, icon: Option<TrayIcon>) -> Result<(), TrayError> {
            self.state.lock().expect("backend").icon = icon;
            Ok(())
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
    fn sets_and_clears_the_icon() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend.clone());
        let icon = TrayIcon::from_rgba(vec![255, 0, 0, 255], 1, 1).expect("icon");

        tray.set_icon(Some(icon.clone())).expect("set icon");
        assert_eq!(backend.state.lock().expect("backend").icon, Some(icon));
        tray.set_icon(None).expect("clear icon");
        assert_eq!(backend.state.lock().expect("backend").icon, None);
        assert!(TrayIcon::from_rgba(vec![0; 3], 1, 1).is_err());
        assert!(TrayIcon::from_rgba(Vec::new(), 0, 0).is_err());
        assert!(TrayIcon::from_rgba(vec![0; 4], u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn updates_the_tooltip() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend.clone());

        tray.set_tooltip(Some("Ready".to_owned())).expect("tooltip");
        assert_eq!(
            backend.state.lock().expect("backend").tooltip.as_deref(),
            Some("Ready")
        );
        tray.set_tooltip(None).expect("clear tooltip");
        assert_eq!(backend.state.lock().expect("backend").tooltip, None);
    }

    #[test]
    fn sets_a_menu_and_dispatches_its_events() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend.clone());
        let items = vec![
            TrayMenuItem::new("open", "Open"),
            TrayMenuItem::new("quit", "Quit").enabled(false),
        ];

        tray.set_menu(&items).expect("menu");
        assert_eq!(backend.menu(), items);

        let (tx, rx) = mpsc::channel();
        tray.on_menu_event(move |id| tx.send(id.to_owned()).expect("receiver"));
        backend.fire_menu("open");
        assert_eq!(rx.recv().expect("event"), "open");
    }

    #[test]
    fn dispatches_clicks() {
        let backend = RecordingBackend::new();
        let tray = Tray::with_backend(backend.clone());
        let (tx, rx) = mpsc::channel();

        tray.on_click(move |click| tx.send(click).expect("receiver"));
        let click = TrayClick {
            button: TrayButton::Right,
            state: TrayButtonState::Up,
        };
        backend.fire_click(click);
        assert_eq!(rx.recv().expect("click"), click);
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

            fn set_menu(&self, _items: Vec<TrayMenuItem>) -> Result<(), TrayError> {
                Err(tray_icon::menu::Error::NotAChildOfThisMenu.into())
            }

            fn set_menu_event_handler(&self, _handler: MenuHandler) {}

            fn set_click_event_handler(&self, _handler: ClickHandler) {}

            fn set_icon(&self, _icon: Option<TrayIcon>) -> Result<(), TrayError> {
                Err(tray_icon::Error::NotMainThread.into())
            }
        }

        let tray = Tray::with_backend(FailingBackend);
        assert!(tray.set_visible(true).is_err());
        assert!(tray.set_tooltip(None).is_err());
        assert!(tray.set_menu(&[TrayMenuItem::new("quit", "Quit")]).is_err());
    }
}
