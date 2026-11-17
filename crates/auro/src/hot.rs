// SPDX-License-Identifier: MIT OR Apache-2.0

//! Experimental in-process hot boundaries (spec 0004, Phase 1 opt-in).
//!
//! A hot boundary is a function the dev host can replace while the process keeps running. The
//! boundary returns **framework-free data**: on Windows the replacement unit cannot construct GPUI
//! elements, so the view renders the data it receives. Patching also requires a build with
//! `debug_assertions`; release builds call the compiled function and ignore patches.
//!
//! Enabled by the `hot-reload` feature plus [`ENABLE_ENV`]. Off by default; `auro dev` keeps its
//! rebuild-and-relaunch loop.

use std::sync::{Arc, Once};

static INSTALL: Once = Once::new();

/// Environment variable that turns hot boundaries on. Only the value `1` enables them.
pub const ENABLE_ENV: &str = "AURO_HOT_RELOAD";

/// Connects to the dev host and reports applied patches.
///
/// Called by [`crate::App::try_run`]; calling it again is a no-op. When [`ENABLE_ENV`] is not `1`
/// the process keeps its compiled code and nothing is registered.
pub fn install() {
    INSTALL.call_once(|| {
        if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
            return;
        }
        subsecond::register_handler(Arc::new(|| eprintln!("auro: hot patch handler fired")));
        dioxus_devtools::connect_subsecond();
    });
}

/// Calls a no-argument hot boundary: the patched copy when the dev host supplied one, otherwise
/// the compiled copy.
///
/// The returned type must be plain data. GPUI types (elements, entities, handles) cannot cross the
/// boundary on Windows.
pub fn hot<T>(f: fn() -> T) -> T {
    subsecond::HotFn::current(f).call(())
}

/// Calls a one-argument hot boundary. Same data-only constraint as [`hot`].
pub fn hot_with<A, T>(f: fn(A) -> T, arg: A) -> T {
    subsecond::HotFn::current(f).call((arg,))
}
