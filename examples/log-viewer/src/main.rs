use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use auro::gpui::{App as GpuiApp, AppContext, Entity, Global, ScrollHandle};
use auro::prelude::*;
use gpui_component::{
    ActiveTheme, Root, Theme as KitTheme, ThemeMode, button::Button as KitButton, h_flex,
    input::Input, input::InputEvent, input::InputState, switch::Switch, v_flex,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
struct LogLine {
    seq: u64,
    level: Level,
    message: String,
}

#[derive(Default)]
struct LogModel {
    lines: Vec<LogLine>,
    received: u64,
    filter: String,
    errors_only: bool,
    dark: bool,
}

impl LogModel {
    fn push(&mut self, line: LogLine) {
        self.received += 1;
        self.lines.push(line);
        if self.lines.len() > 2000 {
            let excess = self.lines.len() - 2000;
            self.lines.drain(0..excess);
        }
    }

    fn visible(&self) -> impl Iterator<Item = &LogLine> {
        let filter = self.filter.to_lowercase();
        self.lines.iter().filter(move |line| {
            (!self.errors_only || line.level == Level::Error)
                && (filter.is_empty() || line.message.to_lowercase().contains(&filter))
        })
    }
}

struct ModelGlobal(Entity<LogModel>);

impl Global for ModelGlobal {}

struct MainView {
    model: Entity<LogModel>,
    filter: Entity<InputState>,
    paused: Arc<AtomicBool>,
    scroll: ScrollHandle,
}

impl MainView {
    fn new(
        model: Entity<LogModel>,
        paused: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter logs..."));
        cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = this.filter.read(cx).value().to_string();
                this.model.update(cx, |model, cx| {
                    model.filter = text;
                    cx.notify();
                });
            }
        })
        .detach();
        cx.observe(&model, |_, _, cx| cx.notify()).detach();

        MainView {
            model,
            filter,
            paused,
            scroll: ScrollHandle::default(),
        }
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.model.read(cx);
        let paused = self.paused.load(Ordering::SeqCst);
        let visible: Vec<_> = model.visible().collect();
        let lines: Vec<_> = visible
            .iter()
            .rev()
            .take(500)
            .rev()
            .map(|line| {
                let color = match line.level {
                    Level::Error => theme.red,
                    Level::Warn => theme.yellow,
                    Level::Info => theme.green,
                    Level::Debug => theme.muted_foreground,
                };
                h_flex()
                    .gap_3()
                    .text_size(px(12.0))
                    .child(
                        div()
                            .w_16()
                            .text_color(theme.muted_foreground)
                            .child(format!("#{}", line.seq)),
                    )
                    .child(div().w_20().text_color(color).child(line.level.label()))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.foreground)
                            .child(line.message.clone()),
                    )
            })
            .collect();

        if !paused {
            self.scroll.scroll_to_bottom();
        }

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .p_3()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_size(px(18.0)).child("Log Viewer"))
                    .child(Input::new(&self.filter).w_64())
                    .child(
                        KitButton::new("pause")
                            .label(if paused { "Resume" } else { "Pause" })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                let paused = !this.paused.load(Ordering::SeqCst);
                                this.paused.store(paused, Ordering::SeqCst);
                                cx.notify();
                            })),
                    )
                    .child(KitButton::new("clear").label("Clear").on_click(cx.listener(
                        |this, _, _window, cx| {
                            this.model.update(cx, |model, cx| {
                                model.lines.clear();
                                model.received = 0;
                                cx.notify();
                            });
                        },
                    )))
                    .child(
                        KitButton::new("settings")
                            .label("Settings")
                            .on_click(|_, _, cx| {
                                if let Err(error) = windows::open("settings", cx) {
                                    eprintln!("settings: {error}");
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .id("log-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(lines),
            )
            .child(
                h_flex()
                    .p_2()
                    .gap_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} shown / {} received",
                        model.visible().count(),
                        model.received
                    ))
                    .child(if paused { "paused" } else { "live" }),
            )
    }
}

struct SettingsView {
    model: Entity<LogModel>,
}

impl SettingsView {
    fn new(model: Entity<LogModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        SettingsView { model }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.model.read(cx);
        let errors_only = model.errors_only;
        let dark = model.dark;

        v_flex()
            .size_full()
            .p_4()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(div().text_size(px(18.0)).child("Settings"))
            .child(
                Switch::new("errors")
                    .checked(errors_only)
                    .label("Errors only")
                    .on_click(cx.listener(|this, checked, _window, cx| {
                        this.model.update(cx, |model, cx| {
                            model.errors_only = *checked;
                            cx.notify();
                        });
                    })),
            )
            .child(
                Switch::new("dark")
                    .checked(dark)
                    .label("Dark theme")
                    .on_click(cx.listener(|this, checked, _window, cx| {
                        this.model.update(cx, |model, cx| {
                            model.dark = *checked;
                            cx.notify();
                        });
                        KitTheme::change(
                            if *checked {
                                ThemeMode::Dark
                            } else {
                                ThemeMode::Light
                            },
                            None,
                            cx,
                        );
                        cx.refresh_windows();
                    })),
            )
            .child(
                KitButton::new("close")
                    .label("Close settings")
                    .on_click(|_, _, cx| {
                        windows::close("settings", cx);
                    }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child("The window is declared in Auro.toml and managed by auro::windows."),
            )
    }
}

fn main() {
    let paused = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let app = App::new();
    let sender = app.event_sender();

    app.on_event(|line: &LogLine, cx| {
        let model = cx.global::<ModelGlobal>().0.clone();
        model.update(cx, |model, cx| {
            model.push(line.clone());
            cx.notify();
        });
    })
    .on_lifecycle({
        let stop = Arc::clone(&stop);
        move |event, _cx| {
            if matches!(event, LifecycleEvent::Exiting) {
                stop.store(true, Ordering::SeqCst);
            }
        }
    })
    .setup({
        let sender = sender.clone();
        let paused = Arc::clone(&paused);
        let stop = Arc::clone(&stop);
        move |cx: &mut GpuiApp, config: &Config| {
            gpui_component::init(cx);
            let dark = matches!(config.app.windows[0].theme, auro::Theme::Dark);
            KitTheme::change(
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                None,
                cx,
            );
            let model = cx.new(|_| LogModel {
                dark,
                ..LogModel::default()
            });
            cx.set_global(ModelGlobal(model));

            let sender = sender.clone();
            let paused = Arc::clone(&paused);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let messages = [
                    ("cache warmed in 42 ms", Level::Debug),
                    ("loaded 1,204 rows", Level::Info),
                    ("config reloaded", Level::Info),
                    ("slow query: 812 ms", Level::Warn),
                    ("connection refused: 127.0.0.1:5432", Level::Error),
                    ("snapshot written", Level::Info),
                    ("retrying in 250 ms", Level::Warn),
                ];
                let mut seq = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    if !paused.load(Ordering::SeqCst) {
                        seq += 1;
                        let (message, level) = messages[(seq as usize) % messages.len()];
                        let _ = sender.emit(LogLine {
                            seq,
                            level,
                            message: message.to_owned(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(350));
                }
            });
        }
    })
    .root_with::<Root>("main", {
        let paused = Arc::clone(&paused);
        move |window, cx| {
            let model = cx.global::<ModelGlobal>().0.clone();
            let view = cx.new(|cx| MainView::new(model, paused.clone(), window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        }
    })
    .root_with::<Root>("settings", |window, cx| {
        let model = cx.global::<ModelGlobal>().0.clone();
        let view = cx.new(|cx| SettingsView::new(model, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .run();
}
