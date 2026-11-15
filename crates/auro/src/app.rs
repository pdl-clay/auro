// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use auro_core::{AuroError, Config, StateStore, WindowConfig};
use auro_ui::{
    App as GpuiApp, AppContext, Application, Bounds, Pixels, Render, Size, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

type OpenWindow = Box<dyn Fn(&mut GpuiApp, &Config, &WindowConfig)>;

/// The application builder.
///
/// Windows and identity come from `Auro.toml`; the builder attaches a root view to a window label.
pub struct App {
    states: Arc<StateStore>,
    roots: Vec<(&'static str, OpenWindow)>,
}

impl App {
    pub fn new() -> Self {
        App {
            states: Arc::new(StateStore::new()),
            roots: Vec::new(),
        }
    }

    /// Stores application state reachable from commands as `State<T>`.
    pub fn manage<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.states.insert(value);
        self
    }

    /// Attaches a root view to the window declared with this label in `Auro.toml`.
    pub fn root<V: Render + Default + 'static>(mut self, label: &'static str) -> Self {
        self.roots.push((
            label,
            Box::new(|cx, config, window| {
                let options = window_options(config, window, cx);
                if let Err(error) =
                    cx.open_window(options, |_window, cx| cx.new(|_cx| V::default()))
                {
                    tracing::error!(window = window.label, %error, "failed to open window");
                }
            }),
        ));
        self
    }

    /// Runs the app, returning before the event loop starts if configuration is invalid.
    pub fn try_run(self) -> Result<(), AuroError> {
        let config = Config::load(&std::env::current_dir()?)?;
        let App { states, roots } = self;

        Application::new().run(move |cx| {
            // Held for the process lifetime: commands resolve state through this store.
            let _states = states;

            for window in &config.app.windows {
                match roots.iter().find(|(label, _)| *label == window.label) {
                    Some((_, open)) => open(cx, &config, window),
                    None => {
                        tracing::error!(
                            window = window.label,
                            "no root view attached to this window"
                        )
                    }
                }
            }

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

fn window_options(config: &Config, window: &WindowConfig, cx: &mut GpuiApp) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(px(window.width as f32), px(window.height as f32)),
        cx,
    );

    let title = window
        .title
        .clone()
        .unwrap_or_else(|| config.app.product_name.clone());

    WindowOptions {
        window_bounds: Some(if window.fullscreen {
            WindowBounds::Fullscreen(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            ..Default::default()
        }),
        app_id: Some(config.app.identifier.clone()),
        is_resizable: window.resizable,
        show: window.visible,
        window_min_size: min_size(window),
        ..Default::default()
    }
}

fn min_size(window: &WindowConfig) -> Option<Size<Pixels>> {
    match (window.min_width, window.min_height) {
        (Some(width), Some(height)) => Some(size(px(width as f32), px(height as f32))),
        _ => None,
    }
}
