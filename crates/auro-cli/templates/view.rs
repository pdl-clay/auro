use std::sync::{Arc, Mutex};
use std::time::Duration;

use auro::gpui::Timer;
use auro::prelude::*;

/// The part of this view that survives an `auro dev` restart (spec 0004, D3).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CounterState {
    pub count: u64,
}

impl ReloadSnapshot for CounterState {
    const KEY: &'static str = "counter";
    const VERSION: u32 = 1;
}

/// Data the view renders. The hot boundary returns this, never GPUI types: on Windows the
/// patched unit cannot construct elements (spec 0004).
#[derive(Clone, Copy, PartialEq)]
pub struct ViewData {
    pub title: &'static str,
    pub accent: u32,
}

/// Hot boundary: edit this function and a running `[dev] hot = true` session reloads it.
fn view_data() -> ViewData {
    ViewData {
        title: "Clicked",
        accent: 0x1f2937,
    }
}

#[cfg(feature = "hot-reload")]
fn current_view_data() -> ViewData {
    auro::hot::hot(view_data)
}

#[cfg(not(feature = "hot-reload"))]
fn current_view_data() -> ViewData {
    view_data()
}

pub struct Counter {
    state: Arc<Mutex<CounterState>>,
    autosave: Option<Autosave>,
    last_data: Option<ViewData>,
    last_revision: u64,
}

impl Default for Counter {
    fn default() -> Self {
        let mut state = CounterState::default();
        if let Some(path) = Snapshot::dev_path("{{identifier}}") {
            match Snapshot::load_from(&path) {
                Ok(Some(mut snapshot)) => match snapshot.restore::<CounterState>() {
                    Restored::Restored(saved) => {
                        state = saved;
                        eprintln!("auro: state restored from {}", path.display());
                    }
                    Restored::Reset(reason) => {
                        eprintln!("auro: state reset: {}", reason.message());
                    }
                },
                Ok(None) => {}
                Err(error) => eprintln!("auro: snapshot unreadable: {error}"),
            }
        }

        let state = Arc::new(Mutex::new(state));
        let mut autosave = Autosave::dev("{{identifier}}", env!("CARGO_PKG_VERSION"));
        if let Some(autosave) = autosave.as_mut() {
            let observed = Arc::clone(&state);
            autosave.watch(move || observed.lock().expect("state lock").clone());
        }
        Counter {
            state,
            autosave,
            last_data: None,
            last_revision: 0,
        }
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().expect("state lock").count;
        let data = current_view_data();
        #[cfg(feature = "hot-reload")]
        let revision = auro::hot::revision();
        #[cfg(not(feature = "hot-reload"))]
        let revision = 0;
        if self.last_data != Some(data) || self.last_revision != revision {
            // Visible proof of what the boundary returned after the latest patch. A layout edit
            // outside `view_data` prints the same data: the boundary is the reloadable unit.
            eprintln!(
                "auro: view data -> {} accent={:#x} revision={revision}",
                data.title, data.accent
            );
            self.last_data = Some(data);
            self.last_revision = revision;
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(data.accent))
            .child(format!("{} {count} times", data.title))
            .child(Button::new("increment", "Increment").on_click(cx.listener(
                |this, _, _window, cx| {
                    let current = this.state.lock().expect("state lock").count;
                    cx.spawn(async move |this, cx| {
                        let next = crate::services::increment(current).await?;
                        this.update(cx, |this, cx| {
                            this.state.lock().expect("state lock").count = next;
                            if let Some(autosave) = this.autosave.as_mut() {
                                autosave.mark_dirty();
                            }
                            cx.notify();
                        })
                        .ok();

                        // Let the debounce settle, then write outside the click handler.
                        Timer::after(Duration::from_millis(350)).await;
                        this.update(cx, |this, _cx| {
                            if let Some(autosave) = this.autosave.as_mut()
                                && let Err(error) = autosave.tick()
                            {
                                eprintln!("auro: snapshot write failed: {error}");
                            }
                        })
                        .ok();
                        Ok::<(), AuroError>(())
                    })
                    .detach();
                },
            )))
    }
}
