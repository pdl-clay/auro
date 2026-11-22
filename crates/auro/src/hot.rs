// SPDX-License-Identifier: MIT OR Apache-2.0

//! Experimental in-process hot boundaries (spec 0004, Phase 1 opt-in).
//!
//! A hot boundary is a function the dev host can replace while the process keeps running. The
//! boundary returns **framework-free data**: on Windows the replacement unit cannot construct GPUI
//! elements, so the view renders the data it receives. Patching also requires a build with
//! `debug_assertions`; release builds call the compiled function and ignore patches.
//!
//! A patch changes code, not the cached frame. [`watch`] wakes the main thread when a patch
//! commits and notifies every open root view, so the next frame renders the patched boundary
//! instead of the frame the dev host replaced. [`hot`] reports the first render of each revision
//! as `auro:trace hot_render <revision>`, which is what `auro dev` turns into save-to-visible
//! timing.
//!
//! Enabled by the `hot-reload` feature plus [`ENABLE_ENV`]. Off by default; `auro dev` keeps its
//! rebuild-and-relaunch loop.

use std::error::Error;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Once;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use auro_ui::App as GpuiApp;
use tokio::sync::watch;

/// Which process and build a devserver patch is addressed to. Also returned by
/// [`apply_patch_from_path`], which applies a patch without a devserver.
#[derive(Debug, PartialEq, Eq)]
pub enum PatchVerdict {
    Apply,
    OtherProcess,
    OtherBuild,
}

/// Why a patch artifact could not be applied from disk.
#[cfg(any(test, feature = "hot-reload"))]
#[derive(Debug)]
pub enum PatchError {
    /// The artifact path could not be read.
    Read(std::io::Error),
    /// The artifact is not a `subsecond::JumpTable` JSON document.
    Decode(serde_json::Error),
    /// The jump table was rejected by `subsecond::apply_patch`.
    Apply(subsecond::PatchError),
    /// A malformed jump table panicked inside `subsecond::apply_patch`.
    Panic,
}

#[cfg(any(test, feature = "hot-reload"))]
impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(f, "cannot read patch artifact: {error}"),
            Self::Decode(error) => write!(f, "invalid patch artifact: {error}"),
            Self::Apply(error) => write!(f, "patch rejected: {error}"),
            Self::Panic => f.write_str("patch loader panicked"),
        }
    }
}

#[cfg(any(test, feature = "hot-reload"))]
impl Error for PatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Apply(error) => Some(error),
            Self::Panic => None,
        }
    }
}

/// Applies a `subsecond::JumpTable` artifact produced by `dx tools hotpatch`.
///
/// This is the disk-driven counterpart of [`connect_devserver`] for tests and tools: no devserver
/// identity fields are available, so a readable artifact is applied to this process. The same
/// panic containment as the devserver path keeps a malformed artifact from killing the host.
#[cfg(any(test, feature = "hot-reload"))]
pub fn apply_patch_from_path(path: &Path) -> Result<PatchVerdict, PatchError> {
    let artifact = std::fs::read_to_string(path).map_err(PatchError::Read)?;
    let table: subsecond::JumpTable =
        serde_json::from_str(&artifact).map_err(PatchError::Decode)?;
    // SAFETY: the caller supplies a dx-generated jump table for this process. `apply_patch` still
    // unwraps while resolving the patch's `main` sentinel, so contain a malformed artifact exactly
    // as `connect_devserver` does; a rejection leaves the previous jump table committed.
    match std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        subsecond::apply_patch(table)
    })) {
        Ok(Ok(())) => Ok(PatchVerdict::Apply),
        Ok(Err(error)) => Err(PatchError::Apply(error)),
        Err(_) => Err(PatchError::Panic),
    }
}

/// True when code registered at `registered_revision` has been replaced by a later apply.
///
/// A callback captured from a boundary must check this before touching state owned by that
/// boundary; after an apply (or rollback) the old code is stale even if it is still callable.
#[cfg(any(test, feature = "hot-reload"))]
pub fn is_stale(registered_revision: u64) -> bool {
    registered_revision != revision()
}

static INSTALL: Once = Once::new();

/// Bumped by the dev host after a patch is committed; read on the main thread.
static REVISION: AtomicU64 = AtomicU64::new(0);
/// Highest revision whose render was reported. Only the first frame of a revision is traced.
static REPORTED: AtomicU64 = AtomicU64::new(0);
/// Wakes the main thread when a patch commits. The value is the committed [`REVISION`].
static PATCHES: OnceLock<watch::Sender<u64>> = OnceLock::new();

/// Environment variable that turns hot boundaries on. Only the value `1` enables them.
pub const ENABLE_ENV: &str = "AURO_HOT_RELOAD";

/// The build identity the devserver stamps on a patch (`dioxus-cli-config` reads the same key).
const BUILD_ID_ENV: &str = "DIOXUS_BUILD_ID";

/// Connects to the dev host and reports applied patches.
///
/// Called by [`crate::App::try_run`]; calling it again is a no-op. When [`ENABLE_ENV`] is not `1`
/// the process keeps its compiled code and nothing is registered.
///
/// The connection is owned here rather than delegated to `dioxus_devtools::connect_subsecond`:
/// that helper unwraps a failed apply, so one patch that cannot be loaded kills the devserver
/// listener and every later edit is ignored. A rejected patch must leave the previous code
/// running and the listener alive.
pub fn install() {
    INSTALL.call_once(|| {
        if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
            return;
        }
        let (sender, _receiver) = watch::channel(0);
        let _ = PATCHES.set(sender);
        subsecond::register_handler(std::sync::Arc::new(|| {
            let revision = REVISION.fetch_add(1, Ordering::SeqCst) + 1;
            eprintln!("auro:trace hot_patch {revision}");
            if let Some(patches) = PATCHES.get() {
                // No receiver is not an error: the window may not be open yet, and the boundary
                // still runs the patched code on the next frame.
                let _ = patches.send(revision);
            }
        }));
        connect_devserver();
    });
}

/// Applies patches addressed to this process and build, reporting failures instead of panicking.
fn connect_devserver() {
    dioxus_devtools::connect(|message| {
        let dioxus_devtools::DevserverMsg::HotReload(update) = message else {
            return;
        };
        let Some(table) = update.jump_table else {
            return;
        };
        let our_build = std::env::var(BUILD_ID_ENV)
            .ok()
            .and_then(|value| value.parse().ok());
        match patch_verdict(
            update.for_pid,
            update.for_build_id,
            std::process::id(),
            our_build,
        ) {
            PatchVerdict::Apply => {}
            PatchVerdict::OtherProcess | PatchVerdict::OtherBuild => return,
        }
        // `apply_patch` unwraps while resolving the patch's `main` sentinel, so a malformed or
        // partly written library can panic. Contain it: the old jump table stays committed and
        // the websocket stays open for the next edit.
        let applied = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            subsecond::apply_patch(table)
        }));
        match applied {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("auro:trace hot_patch_failed {error}"),
            Err(_) => eprintln!("auro:trace hot_patch_failed patch loader panicked"),
        }
    });
}

fn patch_verdict(
    for_pid: Option<u32>,
    for_build: Option<u64>,
    our_pid: u32,
    our_build: Option<u64>,
) -> PatchVerdict {
    if for_pid != Some(our_pid) {
        return PatchVerdict::OtherProcess;
    }
    match (for_build, our_build) {
        (Some(their_build), Some(build)) if their_build != build => PatchVerdict::OtherBuild,
        _ => PatchVerdict::Apply,
    }
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
///
/// This reports the render immediately. Use [`hot_frame`] plus [`rendered`] when the caller
/// validates the value before it becomes the visible frame, so a rejected frame is not counted as
/// a reload.
pub fn hot<T>(f: fn() -> T) -> T {
    let value = hot_frame(f);
    rendered();
    value
}

/// Calls a hot boundary without reporting the render. Call [`rendered`] after the value is
/// accepted as the visible frame.
pub fn hot_frame<T>(f: fn() -> T) -> T {
    // `subsecond::call` catches its own stale-call panic and retries against the new jump table;
    // `HotFn::call` would unwind into the renderer, which has nothing to catch it.
    subsecond::call(|| subsecond::HotFn::current(f).call(()))
}

/// Reports the first accepted render of the current revision. This is the marker `auro dev` turns
/// into save-to-visible timing (spec 0004, D6).
pub fn rendered() {
    report_render(revision());
}

/// Calls a one-argument hot boundary. Same data-only constraint as [`hot`].
///
/// The argument must be `Copy`: a stale patch retries the call, and the retry needs the value
/// again.
pub fn hot_with<A: Copy, T>(f: fn(A) -> T, arg: A) -> T {
    let value = subsecond::call(move || subsecond::HotFn::current(f).call((arg,)));
    rendered();
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
///
/// The devserver calls the patch handler on its own thread; a watch channel wakes this future
/// without a main-thread poll, so a commit reaches the frame loop as soon as the executor runs the
/// task. A poll would add up to its interval to every reload and keep waking an idle app.
#[cfg(feature = "hot-reload")]
pub(crate) fn watch(cx: &mut GpuiApp) {
    if std::env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
        return;
    }
    let Some(patches) = PATCHES.get() else {
        return;
    };
    let mut updates = patches.subscribe();
    cx.spawn(async move |cx| {
        let mut seen = *updates.borrow();
        loop {
            if updates.changed().await.is_err() {
                return;
            }
            let current = *updates.borrow_and_update();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_our_process_and_build() {
        assert_eq!(
            patch_verdict(Some(7), Some(11), 7, Some(11)),
            PatchVerdict::Apply
        );
        assert_eq!(
            patch_verdict(Some(8), Some(11), 7, Some(11)),
            PatchVerdict::OtherProcess
        );
        assert_eq!(
            patch_verdict(None, Some(11), 7, Some(11)),
            PatchVerdict::OtherProcess
        );
        assert_eq!(
            patch_verdict(Some(7), Some(12), 7, Some(11)),
            PatchVerdict::OtherBuild
        );
        // Without a build id on either side the pid is the only identity available.
        assert_eq!(
            patch_verdict(Some(7), None, 7, Some(11)),
            PatchVerdict::Apply
        );
        assert_eq!(
            patch_verdict(Some(7), Some(12), 7, None),
            PatchVerdict::Apply
        );
    }

    #[test]
    fn apply_failure_is_contained() {
        assert_eq!(
            patch_verdict(Some(999), Some(11), 7, Some(11)),
            PatchVerdict::OtherProcess
        );
        assert_eq!(
            patch_verdict(Some(7), Some(999), 7, Some(11)),
            PatchVerdict::OtherBuild
        );

        let before = revision();
        let table = subsecond::JumpTable {
            lib: std::path::PathBuf::from("/auro/no-such-hot-patch.so"),
            map: Default::default(),
            aslr_reference: 0,
            new_base_address: 0,
            ifunc_count: 0,
        };
        // SAFETY: the library path does not exist, so apply_patch returns a Dlopen error before
        // touching any executable memory; catch_unwind covers a malformed-table panic.
        let applied = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
            subsecond::apply_patch(table)
        }));
        assert!(
            matches!(applied, Ok(Err(_)) | Err(_)),
            "a bad patch must be rejected, not committed"
        );
        assert_eq!(
            revision(),
            before,
            "a rejected patch must not advance the revision"
        );
    }

    #[test]
    fn disk_artifact_errors_are_reported_and_staleness_tracks_revision() {
        let missing = std::env::temp_dir().join("auro-no-such-jump-table.json");
        assert!(matches!(
            apply_patch_from_path(&missing),
            Err(PatchError::Read(_))
        ));
        let current = revision();
        assert!(!is_stale(current));
        assert!(is_stale(current.wrapping_add(1)));
    }
}
