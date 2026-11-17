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

pub struct Counter {
    state: Arc<Mutex<CounterState>>,
    autosave: Option<Autosave>,
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
        Counter { state, autosave }
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().expect("state lock").count;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(format!("Clicked {count} times"))
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
