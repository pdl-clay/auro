//! Host-side view code.
//!
//! The hot boundary lives in the `{{name}}-data` crate and is passed to [`run`] as a function
//! pointer. This crate must not depend on that boundary crate: `dx` recompiles a changed crate and
//! its dependents, and keeping the view and element code out of that set is what keeps a boundary
//! edit under the save-to-visible target.

mod services;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use {{crate}}_types::{HotData, ViewData};
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

/// Runs the app with `data` as its hot boundary.
///
/// The function pointer is host-owned state: a patch replaces the function body in the data crate,
/// not this call site.
pub fn run(data: fn() -> HotData) {
    App::new()
        .root_with("main", move |_window, cx| cx.new(|_cx| Counter::new(data)))
        .run();
}

#[cfg(feature = "hot-reload")]
fn current_frame(data: fn() -> HotData) -> Result<HotData, String> {
    // The boundary is pure data, so a panic in patched code can be contained here: the view keeps
    // its previous values and the next render can try again or accept the next patch.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| auro::hot::hot_frame(data)))
        .map_err(|_| "the patched boundary panicked".to_owned())
}

#[cfg(not(feature = "hot-reload"))]
fn current_frame(data: fn() -> HotData) -> Result<HotData, String> {
    Ok(data())
}

pub struct Counter {
    data: fn() -> HotData,
    state: Arc<Mutex<CounterState>>,
    autosave: Option<Autosave>,
    last_data: Option<ViewData>,
    last_revision: u64,
    rejected_revision: Option<u64>,
}

impl Counter {
    fn new(data: fn() -> HotData) -> Self {
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
            data,
            state,
            autosave,
            last_data: None,
            last_revision: 0,
            rejected_revision: None,
        }
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().expect("state lock").count;
        #[cfg(feature = "hot-reload")]
        let revision = auro::hot::revision();
        #[cfg(not(feature = "hot-reload"))]
        let revision = 0;
        let mut rejected = false;
        // Do not retry a frame the patched code already failed to produce; a new patch changes the
        // revision and is tried again.
        let frame = if self.rejected_revision == Some(revision) {
            Err("the patched boundary was rejected".to_owned())
        } else {
            current_frame(self.data).and_then(|frame| frame.decode().map_err(|error| error.to_owned()))
        };
        let data = match frame {
            Ok(data) => {
                self.rejected_revision = None;
                #[cfg(feature = "hot-reload")]
                auro::hot::rendered();
                data
            }
            Err(error) => {
                rejected = true;
                self.rejected_revision = Some(revision);
                if self.last_revision != revision {
                    eprintln!("auro:trace hot_data_rejected {error}");
                    eprintln!("auro: hot data rejected: {error}; previous values stay");
                }
                self.last_data.clone().unwrap_or_default()
            }
        };
        if rejected {
            // The values did not change; do not report them as a new revision.
            self.last_revision = revision;
        }
        if self.last_data.as_ref() != Some(&data) || self.last_revision != revision {
            // Visible proof of what the boundary returned after the latest patch. A layout edit
            // outside the boundary prints the same data: the boundary is the reloadable unit.
            eprintln!(
                "auro: view data -> {} accent={:#x} revision={revision}",
                data.title, data.accent
            );
            self.last_data = Some(data.clone());
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
