// SPDX-License-Identifier: MIT OR Apache-2.0

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use auro_core::events::{QueuedEvent, channel};
use auro_core::{
    AppInstance, AuroError, Config, EventSender, InstanceMessage, LifecycleEvent, State,
    StateStore, WindowConfig,
};
use auro_ui::gpui::AssetSource;
use auro_ui::{
    AnyWindowHandle, App as GpuiApp, AppContext, Application, Bounds, Entity, Global, Pixels,
    Render, Size, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, px, size,
};
use tokio::sync::mpsc::UnboundedReceiver;

type Setup = Box<dyn Fn(&mut GpuiApp, &Config)>;
pub(crate) type OpenWindow =
    Rc<dyn Fn(&mut GpuiApp, &Config, &WindowConfig) -> Option<AnyWindowHandle>>;
type LifecycleHandler = Rc<dyn Fn(&LifecycleEvent, &mut GpuiApp)>;
type EventHandler = Rc<dyn Fn(&mut GpuiApp, &dyn Any)>;

/// Main-thread lifecycle subscribers, in registration order.
#[derive(Default)]
pub(crate) struct Lifecycle {
    handlers: RefCell<Vec<LifecycleHandler>>,
}

impl Lifecycle {
    pub(crate) fn on(&self, handler: impl Fn(&LifecycleEvent, &mut GpuiApp) + 'static) {
        self.handlers.borrow_mut().push(Rc::new(handler));
    }

    pub(crate) fn emit(&self, event: &LifecycleEvent, cx: &mut GpuiApp) {
        // Clone the handles so a handler that closes a window can re-enter `emit` without
        // tripping the `RefCell` borrow.
        let handlers: Vec<_> = self.handlers.borrow().iter().cloned().collect();
        for handler in handlers {
            handler(event, cx);
        }
    }
}

/// Main-thread subscribers for typed events.
#[derive(Clone, Default)]
struct EventBus {
    handlers: Rc<RefCell<HashMap<TypeId, Vec<EventHandler>>>>,
}

impl EventBus {
    fn on<E: Send + 'static>(&self, handler: impl Fn(&E, &mut GpuiApp) + 'static) {
        let handler = Rc::new(move |cx: &mut GpuiApp, payload: &dyn Any| {
            if let Some(event) = payload.downcast_ref::<E>() {
                handler(event, cx);
            }
        });
        self.handlers
            .borrow_mut()
            .entry(TypeId::of::<E>())
            .or_default()
            .push(handler);
    }

    fn handlers_for(&self, type_id: TypeId) -> Vec<EventHandler> {
        self.handlers
            .borrow()
            .get(&type_id)
            .cloned()
            .unwrap_or_default()
    }

    fn dispatch(&self, type_id: TypeId, payload: &dyn Any, cx: &mut GpuiApp) {
        for handler in self.handlers_for(type_id) {
            handler(cx, payload);
        }
    }
}

impl Global for EventBus {}

/// Set by `auro dev`; the app then reports when each window's first repaint is requested.
const TRACE_ENV: &str = "AURO_DEV_TRACE";

/// Set by `auro bench`; the app prints when the first frame is requested.
pub const STARTUP_TRACE_ENV: &str = "AURO_STARTUP_TRACE";

/// The application builder.
///
/// Windows and identity come from `Auro.toml`; the builder attaches a root view to a window label.
/// Auro is the application layer: the component toolkit (or none) is the app's choice, initialized
/// in [`App::setup`] and wrapped by [`App::root_with`] when it needs a custom root.
pub struct App {
    states: Arc<StateStore>,
    roots: Vec<(&'static str, OpenWindow)>,
    setups: Vec<Setup>,
    lifecycle: Rc<Lifecycle>,
    events: EventBus,
    event_rx: Option<UnboundedReceiver<QueuedEvent>>,
    sender: EventSender,
    single_instance: bool,
    #[cfg(feature = "services")]
    deep_links: Option<auro_services::AppLinks>,
    assets: Option<Arc<dyn AssetSource>>,
    activate_on_second_instance: bool,
}

impl App {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        let states = StateStore::new();
        states.insert(sender.clone());
        App {
            states: Arc::new(states),
            roots: Vec::new(),
            setups: Vec::new(),
            lifecycle: Rc::new(Lifecycle::default()),
            events: EventBus::default(),
            event_rx: Some(receiver),
            sender: sender.clone(),
            single_instance: false,
            assets: None,
            activate_on_second_instance: false,
            #[cfg(feature = "services")]
            deep_links: None,
        }
    }

    /// A cloneable sender for background producers.
    ///
    /// The same handle is managed as `State<EventSender>`; use this when building a view or a
    /// setup closure that needs to emit without going through the state store.
    pub fn event_sender(&self) -> EventSender {
        self.sender.clone()
    }

    /// Resolves managed state before the app runs.
    ///
    /// Useful for capturing `State<T>` in a view builder or a toolkit setup closure. The same
    /// store is resolved by commands that declare `State<T>`.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<State<T>> {
        self.states.state::<T>()
    }

    /// Stores application state reachable from commands as `State<T>`.
    pub fn manage<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.states.insert(value);
        self
    }

    /// Refuses to start when another process holds the app identifier's instance lock.
    ///
    /// A secondary process forwards its arguments and working directory to the primary, then gets
    /// [`AuroError::Instance`] before GPUI starts; [`Self::run`] prints it and exits with code 2.
    /// Subscribe to [`InstanceMessage`] with [`Self::on_event`] on the primary to react, or use
    /// [`Self::activate_on_second_instance`] for the first-root focus default.
    pub fn single_instance(mut self) -> Self {
        self.single_instance = true;
        self
    }

    /// Focuses the first root window when a second launch is forwarded to this instance.
    ///
    /// This enables [`Self::single_instance`] too. The first registered root is focused and the app
    /// is activated. Apps that need a different window or custom behavior can leave this off and
    /// use [`Self::on_event`] with [`InstanceMessage`].
    pub fn activate_on_second_instance(mut self) -> Self {
        self.single_instance = true;
        self.activate_on_second_instance = true;
        self
    }

    /// Installs the GPUI asset source used by `svg().path()` and other file-backed elements.
    ///
    /// The source is type-erased behind an `Arc<dyn AssetSource>` and applied to the GPUI
    /// `Application` in [`Self::try_run`]. Without this call GPUI has no assets, so `svg().path()`
    /// does not resolve. [`crate::assets::FileAssets`] and [`crate::assets::EmbeddedAssets`] cover
    /// the common cases; an app can implement `auro::gpui::AssetSource` itself.
    pub fn assets(mut self, source: impl AssetSource) -> Self {
        self.assets = Some(Arc::new(source));
        self
    }

    /// Feeds launch and forwarded instance arguments into a shared [`auro_services::AppLinks`].
    ///
    /// The initial process arguments are parsed at startup. When single-instance mode forwards an
    /// [`InstanceMessage`], its arguments are pushed into the same queue. The app keeps its own
    /// clone and drains it on the main thread.
    #[cfg(feature = "services")]
    pub fn deep_links(mut self, links: auro_services::AppLinks) -> Self {
        self.deep_links = Some(links);
        self
    }

    /// Runs before any window opens, for toolkit and theme initialization.
    ///
    /// Auro ships no component kit. This is where an app initializes the toolkit it chose (for
    /// example `gpui_component::init(cx)`) and applies the theme from `Auro.toml`. Setups run in
    /// registration order, once per app, after GPUI starts.
    pub fn setup(mut self, setup: impl Fn(&mut GpuiApp, &Config) + 'static) -> Self {
        self.setups.push(Box::new(setup));
        self
    }

    /// Registers a main-thread lifecycle handler.
    ///
    /// Handlers run synchronously, in registration order, when the app becomes
    /// [`LifecycleEvent::Ready`], when a tracked window opens or closes, and when the last window
    /// closes ([`LifecycleEvent::Exiting`]). They may update entities and state; they must not
    /// block. A panic propagates through the app. Exit cannot be cancelled in Phase 1.
    pub fn on_lifecycle(self, handler: impl Fn(&LifecycleEvent, &mut GpuiApp) + 'static) -> Self {
        self.lifecycle.on(handler);
        self
    }

    /// Registers a typed application event handler on the main thread.
    ///
    /// Handlers run in registration order when the event is delivered. A background producer holds
    /// an [`EventSender`] (also reachable as managed state) and calls `emit`; the facade pumps the
    /// queue on the main thread. `emit(event, cx)` on the main thread dispatches immediately.
    /// Events with no subscriber are dropped.
    pub fn on_event<E: Send + 'static>(self, handler: impl Fn(&E, &mut GpuiApp) + 'static) -> Self {
        self.events.on(handler);
        self
    }

    /// Attaches a root view to the window declared with this label in `Auro.toml`.
    pub fn root<V: Render + Default + 'static>(mut self, label: &'static str) -> Self {
        self.roots.push((
            label,
            Rc::new(|cx, config, window| open_window::<V>(cx, config, window)),
        ));
        self
    }

    /// Attaches a root view built by the app instead of `V::default()`.
    ///
    /// Use this when the app's toolkit needs to own the window root (dialogs, notifications,
    /// focus state) or when the view needs constructor arguments:
    ///
    /// ```rust,ignore
    /// App::new()
    ///     .setup(|cx, _config| gpui_component::init(cx))
    ///     .root_with::<gpui_component::Root>("main", |window, cx| {
    ///         let view = cx.new(|_| Editor::default());
    ///         cx.new(|cx| gpui_component::Root::new(view, window, cx))
    ///     })
    ///     .run();
    /// ```
    pub fn root_with<V: Render + 'static>(
        mut self,
        label: &'static str,
        build: impl Fn(&mut Window, &mut GpuiApp) -> Entity<V> + 'static,
    ) -> Self {
        self.roots.push((
            label,
            Rc::new(move |cx, config, window| open_window_with(cx, config, window, &build)),
        ));
        self
    }

    /// The window the built-in second-instance activation focuses.
    ///
    /// Uses `app.default-window`; `Config::default_window` already falls back to the first
    /// declared window, so this follows the config contract instead of the root registration
    /// order.
    fn activation_label(&self, config: &Config) -> Option<String> {
        if !self.activate_on_second_instance {
            return None;
        }
        config.default_window().map(|window| window.label.clone())
    }

    /// Rejects a config window that has no root before GPUI starts.
    ///
    /// A window declared in `Auro.toml` is opened at startup, and `windows::open` cannot create it
    /// without a registered root. Failing here turns a headless, apparently hung app into a named
    /// configuration error.
    fn validate_roots(&self, config: &Config) -> Result<(), AuroError> {
        let mut labels = HashSet::new();
        for (label, _) in &self.roots {
            if !labels.insert(*label) {
                return Err(AuroError::window(
                    *label,
                    "duplicate root view; each window label must have exactly one root",
                ));
            }
        }
        for window in &config.app.windows {
            if !self.roots.iter().any(|(label, _)| *label == window.label) {
                return Err(AuroError::window(
                    &window.label,
                    "no root view attached; call App::root or App::root_with for every window declared in Auro.toml",
                ));
            }
        }
        Ok(())
    }

    /// Runs the app, returning before the event loop starts if configuration is invalid, the display
    /// cannot host a GPUI window, or another instance holds the single-instance lock.
    ///
    /// With the `updater` feature, setting `AURO_UPDATE_RESTART_HELPER` makes this run the restart
    /// helper instead of loading config or starting GPUI. The helper waits for the previous process,
    /// applies a prepared update when present, relaunches and returns.
    pub fn try_run(self) -> Result<(), AuroError> {
        #[cfg(feature = "updater")]
        if let Some(path) =
            std::env::var_os(auro_update::RESTART_HELPER_ENV).filter(|path| !path.is_empty())
        {
            return auro_update::run_restart_helper(std::path::Path::new(&path))
                .map_err(|error| AuroError::Message(format!("update restart helper: {error}")));
        }

        crate::platform::preflight()?;

        #[cfg(feature = "hot-reload")]
        crate::hot::install();

        if std::env::var_os(TRACE_ENV).is_some() {
            // Lets the dev loop tell a relaunch from the first launch (spec 0004, D4).
            eprintln!("auro:trace pid {}", std::process::id());
        }

        let config = Config::load(&std::env::current_dir()?)?;
        self.validate_roots(&config)?;
        let instance_listener = if self.single_instance {
            let instance = AppInstance::acquire(&config.app.identifier)?;
            if instance.is_primary() {
                let sender = self.sender.clone();
                Some(instance.listen(move |message| {
                    let _ = sender.emit(message);
                })?)
            } else {
                let args = std::env::args_os()
                    .skip(1)
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect();
                let message = InstanceMessage::new(args, std::env::current_dir()?);
                if let Err(error) = instance.forward(&message) {
                    tracing::warn!(%error, "failed to forward arguments to the primary instance");
                }
                return Err(AuroError::instance(
                    &config.app.identifier,
                    "another instance is already running",
                ));
            }
        } else {
            None
        };
        let activation_label = self.activation_label(&config);
        #[cfg(feature = "services")]
        let deep_links = self.deep_links.clone();
        let App {
            states,
            roots,
            setups,
            lifecycle,
            events,
            event_rx,
            assets,
            ..
        } = self;

        if let Some(label) = activation_label {
            events.on(move |_: &InstanceMessage, cx| {
                let _ = crate::windows::focus(&label, cx);
                cx.activate(true);
            });
        }

        #[cfg(feature = "services")]
        if let Some(links) = deep_links {
            let initial: Vec<String> = std::env::args_os()
                .skip(1)
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect();
            links.push_raw(&initial);
            let forwarded = links.clone();
            events.on(move |message: &InstanceMessage, _cx| {
                forwarded.push_raw(&message.args);
            });
        }

        let application = Application::new();
        let application = match assets {
            Some(source) => application.with_assets(crate::assets::ErasedAssets(source)),
            None => application,
        };
        application.run(move |cx| {
            // Held for the process lifetime: commands resolve state through this store.
            let _states = states;

            let mut event_rx = event_rx.expect("event receiver is created once");
            cx.set_global(events.clone());
            let pump_events = events.clone();
            cx.spawn(async move |cx| {
                while let Some(queued) = event_rx.recv().await {
                    let (type_id, payload) = queued.into_parts();
                    let _ = cx.update(|cx| pump_events.dispatch(type_id, payload.as_ref(), cx));
                }
            })
            .detach();

            let open_windows = Rc::new(RefCell::new(Vec::<(AnyWindowHandle, String)>::new()));

            // Register before setups/Ready so a handler that closes a window still gets the close
            // path; the registry fills in as windows open.
            observe_closes(cx, Rc::clone(&lifecycle), Rc::clone(&open_windows));
            open_all(
                cx,
                &config,
                setups,
                roots,
                lifecycle,
                Rc::clone(&open_windows),
            );
            if std::env::var_os(STARTUP_TRACE_ENV).is_some() {
                let handles: Vec<_> = open_windows
                    .borrow()
                    .iter()
                    .map(|(handle, _)| *handle)
                    .collect();
                for handle in handles {
                    let _ = handle.update(cx, |_, window, _| {
                        window.on_next_frame(|_, _| println!("auro:startup"));
                    });
                }
            }
            #[cfg(feature = "hot-reload")]
            crate::hot::watch(cx);
            cx.activate(true);
        });

        drop(instance_listener);
        Ok(())
    }

    /// Runs the app and exits with code 2 on a configuration, platform or single-instance error.
    pub fn run(self) {
        if let Err(error) = self.try_run() {
            eprintln!("auro: {error}");
            std::process::exit(2);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Emits a typed event to the main-thread subscribers.
///
/// Call this from a view or lifecycle handler. Background code uses [`EventSender`] instead, which
/// queues the event for the pump. Events with no subscriber are dropped.
pub fn emit<E: Send + 'static>(event: E, cx: &mut GpuiApp) {
    let handlers = cx.global::<EventBus>().handlers_for(TypeId::of::<E>());
    for handler in handlers {
        handler(cx, &event as &dyn Any);
    }
}

/// Attributes a closed GPUI window to the label Auro opened it with.
fn observe_closes(
    cx: &mut GpuiApp,
    lifecycle: Rc<Lifecycle>,
    open_windows: Rc<RefCell<Vec<(AnyWindowHandle, String)>>>,
) {
    cx.on_window_closed(move |cx| {
        let open = cx.windows();
        let mut closed = Vec::new();
        open_windows.borrow_mut().retain(|(handle, label)| {
            if open.contains(handle) {
                true
            } else {
                closed.push(label.clone());
                false
            }
        });

        for label in closed {
            lifecycle.emit(&LifecycleEvent::WindowClosed { label }, cx);
        }

        if cx.windows().is_empty() {
            lifecycle.emit(&LifecycleEvent::Exiting, cx);
            cx.quit();
        }
    })
    .detach();
}

/// Runs the app's setups and opens every configured window that has a root.
fn open_all(
    cx: &mut GpuiApp,
    config: &Config,
    setups: Vec<Setup>,
    roots: Vec<(&'static str, OpenWindow)>,
    lifecycle: Rc<Lifecycle>,
    open_windows: Rc<RefCell<Vec<(AnyWindowHandle, String)>>>,
) {
    let manager = crate::windows::WindowManager::new(
        config.clone(),
        roots,
        Rc::clone(&lifecycle),
        Rc::clone(&open_windows),
    );
    cx.set_global(manager);

    for setup in setups {
        setup(cx, config);
    }

    for window in &config.app.windows {
        match crate::windows::open(&window.label, cx) {
            Ok(_) => {
                if std::env::var_os(TRACE_ENV).is_some() {
                    eprintln!("auro:trace window_requested {}", window.label);
                }
            }
            Err(error) => {
                tracing::error!(window = window.label, %error, "failed to open window");
            }
        }
    }

    lifecycle.emit(&LifecycleEvent::Ready, cx);
}

fn open_window<V: Render + Default + 'static>(
    cx: &mut GpuiApp,
    config: &Config,
    window: &WindowConfig,
) -> Option<AnyWindowHandle> {
    open_window_with(cx, config, window, &|_, cx| cx.new(|_cx| V::default()))
}

fn open_window_with<V: Render + 'static>(
    cx: &mut GpuiApp,
    config: &Config,
    window: &WindowConfig,
    build: &dyn Fn(&mut Window, &mut GpuiApp) -> Entity<V>,
) -> Option<AnyWindowHandle> {
    let title = window_title(config, window);
    let options = window_options(config, window, &title, cx);

    // GPUI 0.2.2 applies `WindowOptions::titlebar.title` on X11, Windows and macOS but not on
    // Wayland, where the compositor then reports no title. Setting it on the window is the path
    // every platform honors, and it keeps the title identical across backends.
    match cx.open_window(options, |gpui_window, cx| {
        gpui_window.set_window_title(&title);
        build(gpui_window, cx)
    }) {
        Ok(handle) => Some(*handle),
        Err(error) => {
            tracing::error!(window = window.label, %error, "failed to open window");
            None
        }
    }
}

fn window_title(config: &Config, window: &WindowConfig) -> String {
    window
        .title
        .clone()
        .unwrap_or_else(|| config.app.product_name.clone())
}

fn window_options(
    config: &Config,
    window: &WindowConfig,
    title: &str,
    cx: &mut GpuiApp,
) -> WindowOptions {
    let window_size = size(px(window.width as f32), px(window.height as f32));
    let window_bounds = if window.fullscreen {
        Some(WindowBounds::Fullscreen(Bounds::centered(
            None,
            window_size,
            cx,
        )))
    } else if window.center {
        Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        )))
    } else {
        // No position in the schema: let the platform place the window instead of centering it.
        None
    };

    WindowOptions {
        window_bounds,
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            ..Default::default()
        }),
        app_id: Some(config.app.identifier.clone()),
        is_resizable: window.resizable,
        show: window.visible,
        window_min_size: min_size(window),
        window_background: if window.transparent {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        // GPUI has no "no decorations" mode; client-side decorations are the closest, and GPUI
        // draws none, so the window appears undecorated where the platform honors it (Wayland).
        window_decorations: Some(if window.decorations {
            WindowDecorations::Server
        } else {
            WindowDecorations::Client
        }),
        ..Default::default()
    }
}

fn min_size(window: &WindowConfig) -> Option<Size<Pixels>> {
    match (window.min_width, window.min_height) {
        (None, None) => None,
        // GPUI takes one size for both axes; 0 means no constraint on the axis that was not set.
        (width, height) => Some(size(
            px(width.unwrap_or(0) as f32),
            px(height.unwrap_or(0) as f32),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auro_core::{AppConfig, BundleConfig, DevConfig, Theme};
    use auro_ui::gpui::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div,
    };

    #[derive(Default)]
    struct Blank;

    #[test]
    fn single_instance_is_opt_in() {
        let app = App::new();
        assert!(!app.single_instance);
        let app = app.single_instance();
        assert!(app.single_instance);

        let app = App::new().root::<Blank>("main");
        let config = config(None);
        assert_eq!(app.activation_label(&config), None);
        let app = app.activate_on_second_instance();
        assert!(app.single_instance);
        assert!(app.activate_on_second_instance);
        assert_eq!(app.activation_label(&config), Some("main".to_owned()));

        let mut two_windows = config.clone();
        let mut second = two_windows.app.windows[0].clone();
        second.label = "secondary".to_owned();
        two_windows.app.windows.push(second);
        two_windows.app.default_window = Some("secondary".to_owned());
        assert_eq!(
            app.activation_label(&two_windows),
            Some("secondary".to_owned()),
            "the configured default window wins over declaration order"
        );
    }

    impl Render for Blank {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn config(title: Option<&str>) -> Config {
        Config {
            schema: None,
            app: AppConfig {
                identifier: "com.example.test".to_owned(),
                product_name: "Product".to_owned(),
                version: "0.1.0".to_owned(),
                description: String::new(),
                authors: Vec::new(),
                homepage: String::new(),
                license: String::new(),
                default_window: None,
                windows: vec![WindowConfig {
                    label: "main".to_owned(),
                    title: title.map(str::to_owned),
                    width: 960,
                    height: 640,
                    min_width: None,
                    min_height: None,
                    max_width: None,
                    max_height: None,
                    resizable: true,
                    decorations: true,
                    transparent: false,
                    always_on_top: false,
                    visible: true,
                    center: true,
                    fullscreen: false,
                    theme: Theme::System,
                }],
            },
            dev: DevConfig::default(),
            plugins: auro_core::config::PluginConfig::default(),
            bundle: BundleConfig::default(),
            updater: None,
        }
    }

    #[test]
    fn missing_roots_are_reported_before_the_event_loop() {
        let config = config(None);
        let error = App::new()
            .validate_roots(&config)
            .expect_err("a declared window without a root is a configuration error");
        assert!(matches!(error, AuroError::Window { ref label, .. } if label == "main"));

        App::new()
            .root::<Blank>("main")
            .validate_roots(&config)
            .expect("every declared window is rooted");

        let mut two_windows = config.clone();
        let mut second = two_windows.app.windows[0].clone();
        second.label = "secondary".to_owned();
        two_windows.app.windows.push(second);
        let error = App::new()
            .root::<Blank>("main")
            .validate_roots(&two_windows)
            .expect_err("the second window has no root");
        assert!(matches!(error, AuroError::Window { ref label, .. } if label == "secondary"));
    }

    #[test]
    fn duplicate_root_labels_are_reported_before_the_event_loop() {
        let config = config(None);
        let error = App::new()
            .root::<Blank>("main")
            .root::<Blank>("main")
            .validate_roots(&config)
            .expect_err("one root per window label");
        assert!(matches!(error, AuroError::Window { ref label, .. } if label == "main"));
        assert!(error.to_string().contains("duplicate"), "{error}");
    }

    #[test]
    fn opens_window_with_the_configured_title() {
        let cx = TestAppContext::single();
        let config = config(Some("Custom title"));
        let handle = cx
            .update(|cx| open_window::<Blank>(cx, &config, &config.app.windows[0]))
            .expect("the window opens");

        let mut vcx = VisualTestContext::from_window(handle, &cx);
        assert_eq!(vcx.window_title().as_deref(), Some("Custom title"));
    }

    #[test]
    fn window_title_falls_back_to_the_product_name() {
        let config = config(None);
        assert_eq!(window_title(&config, &config.app.windows[0]), "Product");
    }

    #[test]
    fn honors_a_single_minimum_dimension() {
        let mut window = config(None).app.windows[0].clone();
        window.min_width = Some(320);

        let minimum = min_size(&window).expect("one dimension is enough");
        assert_eq!(minimum.width, px(320.0));
        assert_eq!(minimum.height, px(0.0));
    }

    #[derive(Default)]
    struct Wrapped;

    impl Render for Wrapped {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn setup_runs_before_roots_and_lifecycle_reports_ready() {
        use std::sync::{Arc, Mutex};

        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let setup_order = Arc::clone(&order);
        let root_order = Arc::clone(&order);
        let events = Arc::new(Mutex::new(Vec::<LifecycleEvent>::new()));
        let lifecycle_events = Arc::clone(&events);
        let lifecycle_order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let first_handler = Arc::clone(&lifecycle_order);
        let second_handler = Arc::clone(&lifecycle_order);
        let mut config = config(Some("Custom title"));
        config.app.windows[0].theme = Theme::Dark;

        let app = App::new()
            .setup(move |_cx, config| {
                assert_eq!(config.app.product_name, "Product");
                assert_eq!(config.app.windows[0].theme, Theme::Dark);
                setup_order.lock().expect("order").push("setup");
            })
            .root_with::<Wrapped>("main", move |_window, cx| {
                root_order.lock().expect("order").push("root");
                cx.new(|_| Wrapped)
            })
            .on_lifecycle(move |event, _cx| {
                lifecycle_events.lock().expect("events").push(event.clone());
            })
            .on_lifecycle(move |_event, _cx| {
                first_handler.lock().expect("order").push("first");
            })
            .on_lifecycle(move |_event, _cx| {
                second_handler.lock().expect("order").push("second");
            });

        let App {
            states: _,
            roots,
            setups,
            lifecycle,
            ..
        } = app;
        let open_windows = Rc::new(RefCell::new(Vec::new()));
        let observed_windows = Rc::clone(&open_windows);
        let cx = TestAppContext::single();
        cx.update(|cx| open_all(cx, &config, setups, roots, lifecycle, open_windows));

        assert_eq!(
            order.lock().expect("order").as_slice(),
            ["setup", "root"],
            "setups must run before a root builder"
        );
        assert_eq!(
            events.lock().expect("events").as_slice(),
            [
                LifecycleEvent::WindowOpened {
                    label: "main".to_owned()
                },
                LifecycleEvent::Ready
            ],
            "window events precede Ready"
        );
        assert_eq!(
            lifecycle_order.lock().expect("order").as_slice(),
            ["first", "second", "first", "second"],
            "lifecycle handlers run in registration order for each event"
        );
        assert_eq!(cx.windows().len(), 1, "the declared window must open");
        assert_eq!(observed_windows.borrow().len(), 1, "the handle is tracked");
    }

    #[test]
    fn lifecycle_reports_window_close_and_exit() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::<LifecycleEvent>::new()));
        let recorded = Arc::clone(&events);
        let lifecycle = Lifecycle::default();
        lifecycle.on(move |event, _cx| {
            recorded.lock().expect("events").push(event.clone());
        });

        let cx = TestAppContext::single();
        cx.update(|cx| {
            lifecycle.emit(
                &LifecycleEvent::WindowClosed {
                    label: "main".to_owned(),
                },
                cx,
            );
            lifecycle.emit(&LifecycleEvent::Exiting, cx);
        });

        assert_eq!(
            events.lock().expect("events").as_slice(),
            [
                LifecycleEvent::WindowClosed {
                    label: "main".to_owned()
                },
                LifecycleEvent::Exiting
            ]
        );
    }

    fn two_window_config() -> Config {
        let mut config = config(Some("Main"));
        let mut secondary = config.app.windows[0].clone();
        secondary.label = "secondary".to_owned();
        secondary.title = Some("Secondary".to_owned());
        let mut unrooted = secondary.clone();
        unrooted.label = "unrooted".to_owned();
        unrooted.title = Some("Unrooted".to_owned());
        config.app.windows.push(secondary);
        config.app.windows.push(unrooted);
        config
    }

    #[test]
    fn window_manager_opens_focuses_closes_and_reopens() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::<LifecycleEvent>::new()));
        let recorded = Arc::clone(&events);
        let config = two_window_config();

        let app = App::new()
            .root::<Blank>("main")
            .root::<Blank>("secondary")
            .on_lifecycle(move |event, _cx| {
                recorded.lock().expect("events").push(event.clone());
            });

        let App {
            states: _,
            roots,
            setups,
            lifecycle,
            ..
        } = app;
        let open_windows = Rc::new(RefCell::new(Vec::new()));
        let cx = TestAppContext::single();

        cx.update(|cx| {
            observe_closes(cx, Rc::clone(&lifecycle), Rc::clone(&open_windows));
            open_all(
                cx,
                &config,
                setups,
                roots,
                Rc::clone(&lifecycle),
                open_windows,
            );
            assert_eq!(cx.windows().len(), 2);

            let main = crate::windows::handle("main", cx).expect("main open");
            assert!(crate::windows::is_open("main", cx));
            assert!(crate::windows::is_open("secondary", cx));
            assert!(crate::windows::focus("main", cx));
            assert!(
                matches!(crate::windows::open("main", cx), Ok(handle) if handle == main),
                "opening an open window is idempotent"
            );

            assert!(crate::windows::close("main", cx));
            assert!(!crate::windows::is_open("main", cx));
            assert!(crate::windows::is_open("secondary", cx));

            let error = crate::windows::open("nope", cx)
                .err()
                .expect("unknown label");
            assert_eq!(error.kind(), "window");
            let error = crate::windows::open("unrooted", cx)
                .err()
                .expect("declared without a root");
            assert_eq!(error.kind(), "window");
            assert!(!crate::windows::close("nope", cx));
            assert!(!crate::windows::focus("nope", cx));

            assert!(crate::windows::open("main", cx).is_ok());
            assert!(crate::windows::is_open("main", cx));
            assert!(crate::windows::close("secondary", cx));
            assert!(crate::windows::is_open("main", cx));
            assert!(crate::windows::close("main", cx));
            assert!(cx.windows().is_empty());
        });

        assert_eq!(
            events.lock().expect("events").as_slice(),
            [
                LifecycleEvent::WindowOpened {
                    label: "main".to_owned()
                },
                LifecycleEvent::WindowOpened {
                    label: "secondary".to_owned()
                },
                LifecycleEvent::Ready,
                LifecycleEvent::WindowClosed {
                    label: "main".to_owned()
                },
                LifecycleEvent::WindowOpened {
                    label: "main".to_owned()
                },
                LifecycleEvent::WindowClosed {
                    label: "secondary".to_owned()
                },
                LifecycleEvent::WindowClosed {
                    label: "main".to_owned()
                },
                LifecycleEvent::Exiting,
            ]
        );
    }

    #[derive(Debug, PartialEq)]
    struct Ping(u32);

    #[derive(Debug, PartialEq)]
    struct Pong(u32);

    #[test]
    fn delivers_typed_events_and_keeps_types_isolated() {
        use std::sync::{Arc, Mutex};

        let pings = Arc::new(Mutex::new(Vec::new()));
        let pongs = Arc::new(Mutex::new(Vec::new()));
        let ping_log = Arc::clone(&pings);
        let pong_log = Arc::clone(&pongs);

        let app = App::new()
            .on_event(move |event: &Ping, _cx| {
                ping_log.lock().expect("pings").push(event.0);
            })
            .on_event(move |event: &Pong, _cx| {
                pong_log.lock().expect("pongs").push(event.0);
            });

        let App { events, .. } = app;
        let cx = TestAppContext::single();
        cx.update(|cx| cx.set_global(events.clone()));
        cx.update(|cx| emit(Ping(1), cx));
        cx.update(|cx| emit(Pong(2), cx));

        assert_eq!(*pings.lock().expect("pings"), [1]);
        assert_eq!(*pongs.lock().expect("pongs"), [2]);
    }

    #[test]
    fn queues_background_events_for_the_main_thread_pump() {
        use std::sync::{Arc, Mutex};

        let app = App::new();
        let App {
            states,
            events,
            event_rx,
            ..
        } = app;
        let mut rx = event_rx.expect("receiver");

        let sender = states.state::<EventSender>().expect("sender in state");
        assert!(sender.emit(Ping(9)), "the queue accepts the event");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&seen);
        let second = Arc::clone(&seen);
        events.on(move |event: &Ping, _cx| {
            first.lock().expect("events").push(("first", event.0));
        });
        events.on(move |event: &Ping, _cx| {
            second.lock().expect("events").push(("second", event.0));
        });

        let (type_id, payload) = rx.try_recv().expect("queued event").into_parts();
        let cx = TestAppContext::single();
        cx.update(|cx| events.dispatch(type_id, payload.as_ref(), cx));

        assert_eq!(
            seen.lock().expect("events").as_slice(),
            [("first", 9), ("second", 9)],
            "handlers run in registration order"
        );
    }

    #[test]
    fn exposes_managed_state_and_the_event_sender_before_run() {
        let app = App::new();
        assert!(app.state::<EventSender>().is_some());
        assert!(
            app.event_sender().emit(Ping(1)),
            "the receiver is still alive before run"
        );
    }

    #[test]
    fn maps_transparency_decorations_and_centering() {
        let cx = TestAppContext::single();
        let config = config(Some("Custom title"));
        let mut window = config.app.windows[0].clone();
        window.transparent = true;
        window.decorations = false;
        window.center = false;

        let options = cx.update(|cx| window_options(&config, &window, "Custom title", cx));
        assert_eq!(
            options.window_background,
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
        assert!(
            options.window_bounds.is_none(),
            "center = false leaves placement to the platform"
        );
    }

    #[test]
    fn defaults_to_opaque_decorated_and_centered() {
        let cx = TestAppContext::single();
        let config = config(Some("Custom title"));

        let options =
            cx.update(|cx| window_options(&config, &config.app.windows[0], "Custom title", cx));
        assert_eq!(
            options.window_background,
            WindowBackgroundAppearance::Opaque
        );
        assert_eq!(options.window_decorations, Some(WindowDecorations::Server));
        assert!(options.window_bounds.is_some());
    }
}
