// SPDX-License-Identifier: MIT OR Apache-2.0

//! Experimental in-process hot boundaries (spec 0004, Phase 1 opt-in).
//!
//! A hot boundary is a function the dev host can replace while the process keeps running. The
//! boundary returns **framework-free data**: on Windows the replacement unit cannot construct GPUI
//! elements, so the view renders the data it receives. Patching also requires a build with
//! `debug_assertions`; release builds call the compiled function and ignore patches.
//!
//! A patch changes code, not the cached frame. [`watch`] polls [`revision`] on the main thread and
//! notifies every open root view, so the next frame renders the patched boundary instead of the
//! frame the dev host replaced. [`hot`] reports the first render of each revision as
//! `auro:trace hot_render <revision>`, which is what `auro dev` turns into save-to-visible timing.
//!
//! Enabled by the `hot-reload` feature plus [`ENABLE_ENV`]. Off by default; `auro dev` keeps its
//! rebuild-and-relaunch loop.

use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use auro_ui::App as GpuiApp;
use auro_ui::gpui::Timer;

static INSTALL: Once = Once::new();

/// Bumped by the dev host after a patch is committed; read on the main thread.
static REVISION: AtomicU64 = AtomicU64::new(0);
/// Highest revision whose render was reported. Only the first frame of a revision is traced.
static REPORTED: AtomicU64 = AtomicU64::new(0);

/// Environment variable that turns hot boundaries on. Only the value `1` enables them.
pub const ENABLE_ENV: &str = "AURO_HOT_RELOAD";

/// How often the main thread looks for a committed patch. One frame at 60 Hz is 16 ms; polling
/// faster than the display only burns CPU while a patch is being applied.
const POLL: Duration = Duration::from_millis(16);

/// Connects to the dev host and reports applied patches.
///
/// Called by [`crate::App::try_run`]; calling it again is a no-op. When [`ENABLE_ENV`] is not `1`
/// the process keeps its compiled code and nothing is registered.
pub fn install() {
    INSTALL.call_once(|| {
        if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
            return;
        }
        subsecond::register_handler(std::sync::Arc::new(|| {
            let revision = REVISION.fetch_add(1, Ordering::SeqCst) + 1;
            eprintln!("auro:trace hot_patch {revision}");
        }));
        dioxus_devtools::connect_subsecond();
    });
}

/// The last patch the dev host committed. `0` means no patch has been applied in this process.
pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

/// Calls a no-argument hot boundary: the patched copy when the dev host supplied one, otherwise
/// the compiled copy.
///
/// The returned type must be plain data. GPUI types (elements, entities, handles) cannot cross the
/// boundary on Windows.
pub fn hot<T>(f: fn() -> T) -> T {
    let revision = revision();
    // `subsecond::call` catches its own stale-call panic and retries against the new jump table;
    // `HotFn::call` would unwind into the renderer, which has nothing to catch it.
    let value = subsecond::call(|| subsecond::HotFn::current(f).call(()));
    report_render(revision);
    value
}

/// Calls a one-argument hot boundary. Same data-only constraint as [`hot`].
///
/// The argument must be `Copy`: a stale patch retries the call, and the retry needs the value
/// again.
pub fn hot_with<A: Copy, T>(f: fn(A) -> T, arg: A) -> T {
    let revision = revision();
    let value = subsecond::call(move || subsecond::HotFn::current(f).call((arg,)));
    report_render(revision);
    value
}

/// Prints the first render after a patch, once per revision. The dev loop turns the line into
/// save-to-visible timing (spec 0004, D6).
fn report_render(revision: u64) {
    if revision == 0 {
        return;
    }
    let mut seen = REPORTED.load(Ordering::SeqCst);
    while seen < revision {
        match REPORTED.compare_exchange_weak(seen, revision, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                eprintln!("auro:trace hot_render {revision}");
                return;
            }
            Err(current) => seen = current,
        }
    }
}

/// Re-renders every open root view when a patch is committed.
///
/// A patch replaces code while GPUI keeps the last frame; without a notification the patched
/// boundary would only run on the next user interaction, which is not observable as a reload.
#[cfg(feature = "hot-reload")]
pub(crate) fn watch(cx: &mut GpuiApp) {
    if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
        return;
    }
    cx.spawn(async move |cx| {
        let mut seen = revision();
        loop {
            Timer::after(POLL).await;
            let current = revision();
            if current == seen {
                continue;
            }
            seen = current;
            let _ = cx.update(|cx| {
                for handle in cx.windows() {
                    let _ = handle.update(cx, |view, _window, cx| cx.notify(view.entity_id()));
                }
            });
        }
    })
    .detach();
}
