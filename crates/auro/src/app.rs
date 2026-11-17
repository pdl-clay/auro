// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use auro_core::{AuroError, Config, StateStore, WindowConfig};
use auro_ui::{
    App as GpuiApp, AppContext, Application, Bounds, Entity, Pixels, Render, Size, TitlebarOptions,
    Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, px, size,
};

type Setup = Box<dyn Fn(&mut GpuiApp, &Config)>;
type OpenWindow = Box<dyn Fn(&mut GpuiApp, &Config, &WindowConfig)>;

/// Set by `auro dev`; the app then reports when each window's first repaint is requested.
const TRACE_ENV: &str = "AURO_DEV_TRACE";

/// The application builder.
///
/// Windows and identity come from `Auro.toml`; the builder attaches a root view to a window label.
/// Auro is the application layer: the component toolkit (or none) is the app's choice, initialized
/// in [`App::setup`] and wrapped by [`App::root_with`] when it needs a custom root.
pub struct App {
    states: Arc<StateStore>,
    roots: Vec<(&'static str, OpenWindow)>,
    setups: Vec<Setup>,
}

impl App {
    pub fn new() -> Self {
        App {
            states: Arc::new(StateStore::new()),
            roots: Vec::new(),
            setups: Vec::new(),
        }
    }

    /// Stores application state reachable from commands as `State<T>`.
    pub fn manage<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.states.insert(value);
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

    /// Attaches a root view to the window declared with this label in `Auro.toml`.
    pub fn root<V: Render + Default + 'static>(mut self, label: &'static str) -> Self {
        self.roots.push((
            label,
            Box::new(|cx, config, window| open_window::<V>(cx, config, window)),
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
            Box::new(move |cx, config, window| {
                open_window_with(cx, config, window, &build);
            }),
        ));
        self
    }

    /// Runs the app, returning before the event loop starts if configuration is invalid.
    pub fn try_run(self) -> Result<(), AuroError> {
        #[cfg(feature = "hot-reload")]
        crate::hot::install();

        let config = Config::load(&std::env::current_dir()?)?;
        let App {
            states,
            roots,
            setups,
        } = self;

        Application::new().run(move |cx| {
            // Held for the process lifetime: commands resolve state through this store.
            let _states = states;
            open_all(cx, &config, setups, &roots);

            // Closing the last window ends the app; `auro dev` and acceptance expect exit 0.
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });

        Ok(())
    }

    /// Runs the app and exits with code 2 on a configuration error.
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

/// Runs the app's setups and opens every configured window that has a root.
fn open_all(
    cx: &mut GpuiApp,
    config: &Config,
    setups: Vec<Setup>,
    roots: &[(&'static str, OpenWindow)],
) {
    for setup in setups {
        setup(cx, config);
    }

    for window in &config.app.windows {
        match roots.iter().find(|(label, _)| *label == window.label) {
            Some((_, open)) => {
                open(cx, config, window);
                if std::env::var_os(TRACE_ENV).is_some() {
                    eprintln!("auro:trace window_requested {}", window.label);
                }
            }
            None => tracing::error!(
                window = window.label,
                "no root view attached to this window"
            ),
        }
    }
}

fn open_window<V: Render + Default + 'static>(
    cx: &mut GpuiApp,
    config: &Config,
    window: &WindowConfig,
) {
    open_window_with(cx, config, window, &|_, cx| cx.new(|_cx| V::default()));
}

fn open_window_with<V: Render + 'static>(
    cx: &mut GpuiApp,
    config: &Config,
    window: &WindowConfig,
    build: &dyn Fn(&mut Window, &mut GpuiApp) -> Entity<V>,
) {
    let title = window_title(config, window);
    let options = window_options(config, window, &title, cx);

    // GPUI 0.2.2 applies `WindowOptions::titlebar.title` on X11, Windows and macOS but not on
    // Wayland, where the compositor then reports no title. Setting it on the window is the path
    // every platform honors, and it keeps the title identical across backends.
    if let Err(error) = cx.open_window(options, |gpui_window, cx| {
        gpui_window.set_window_title(&title);
        build(gpui_window, cx)
    }) {
        tracing::error!(window = window.label, %error, "failed to open window");
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
            bundle: BundleConfig::default(),
        }
    }

    #[test]
    fn opens_window_with_the_configured_title() {
        let cx = TestAppContext::single();
        let config = config(Some("Custom title"));
        cx.update(|cx| open_window::<Blank>(cx, &config, &config.app.windows[0]));

        let windows = cx.windows();
        assert_eq!(windows.len(), 1, "the declared window must open");

        let mut vcx = VisualTestContext::from_window(windows[0], &cx);
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
    fn setup_runs_before_roots_and_root_with_uses_the_app_builder() {
        use std::sync::{Arc, Mutex};

        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let setup_order = Arc::clone(&order);
        let root_order = Arc::clone(&order);
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
            });

        let App {
            states: _,
            roots,
            setups,
        } = app;
        let cx = TestAppContext::single();
        cx.update(|cx| open_all(cx, &config, setups, &roots));

        assert_eq!(
            order.lock().expect("order").as_slice(),
            ["setup", "root"],
            "setups must run before a root builder"
        );
        assert_eq!(cx.windows().len(), 1, "the declared window must open");
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
