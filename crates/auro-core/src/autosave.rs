// SPDX-License-Identifier: MIT OR Apache-2.0

//! Debounced autosave of opted-in state (spec 0004, D3).
//!
//! Capture runs on the caller's thread on purpose: GPUI state belongs to the main thread, so a
//! background worker must never read it. [`Autosave::tick`] is meant to be called from a frame or
//! timer; after a quiet period it captures the watched entries and writes the envelope atomically.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use auro_ipc::AuroError;

use crate::snapshot::{ReloadSnapshot, Snapshot};

type Capture = Box<dyn Fn(&mut Snapshot) -> Result<(), AuroError>>;

/// Debounce applied by [`Autosave::dev`].
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Writes opted-in state to a snapshot file after edits settle.
pub struct Autosave {
    path: PathBuf,
    build: String,
    captures: Vec<Capture>,
    debounce: Duration,
    pending_since: Option<Instant>,
    writes: u64,
}

impl Autosave {
    /// Creates an autosave for an explicit path and build identity.
    pub fn new(path: impl Into<PathBuf>, build: impl Into<String>, debounce: Duration) -> Self {
        Autosave {
            path: path.into(),
            build: build.into(),
            captures: Vec::new(),
            debounce,
            pending_since: None,
            writes: 0,
        }
    }

    /// Creates an autosave for the app's dev snapshot path, or `None` when the platform has no
    /// data directory.
    pub fn dev(identifier: &str, build: impl Into<String>) -> Option<Self> {
        Some(Self::new(
            Snapshot::dev_path(identifier)?,
            build,
            DEFAULT_DEBOUNCE,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of snapshots written so far, for diagnostics and tests.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Registers one opted-in value. `get` is called on the thread that calls [`Autosave::tick`].
    pub fn watch<T, F>(&mut self, get: F) -> &mut Self
    where
        T: ReloadSnapshot + 'static,
        F: Fn() -> T + 'static,
    {
        self.captures.push(Box::new(move |snapshot| {
            let value = get();
            snapshot.insert(&value)
        }));
        self
    }

    /// Marks state as changed; the write happens once edits have been quiet for the debounce.
    pub fn mark_dirty(&mut self) {
        // The window restarts on every change, so a burst saves once it settles.
        self.pending_since = Some(Instant::now());
    }

    /// Writes now if a change is pending and the debounce has elapsed. Returns whether it wrote.
    pub fn tick(&mut self) -> Result<bool, AuroError> {
        let Some(since) = self.pending_since else {
            return Ok(false);
        };
        if since.elapsed() < self.debounce {
            return Ok(false);
        }
        self.pending_since = None;
        self.write()?;
        Ok(true)
    }

    /// Captures and writes immediately, ignoring the debounce.
    pub fn flush(&mut self) -> Result<(), AuroError> {
        self.pending_since = None;
        self.write()
    }

    fn write(&mut self) -> Result<(), AuroError> {
        let mut snapshot = Snapshot::new(self.build.clone());
        // Capture before touching the file: a capture error must not leave a stale-but-newer file.
        for capture in &self.captures {
            capture(&mut snapshot)?;
        }
        snapshot.save_to(&self.path)?;
        self.writes += 1;
        Ok(())
    }
}

impl std::fmt::Debug for Autosave {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Autosave")
            .field("path", &self.path)
            .field("build", &self.build)
            .field("debounce", &self.debounce)
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{Restored, Snapshot};
    use serde::{Deserialize, Serialize};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Counter {
        count: u64,
    }

    impl ReloadSnapshot for Counter {
        const KEY: &'static str = "test.counter";
        const VERSION: u32 = 1;
    }

    fn autosave(dir: &Path, debounce: Duration) -> Autosave {
        Autosave::new(dir.join("dev-snapshot.json"), "build-1", debounce)
    }

    #[test]
    fn waits_for_the_debounce_before_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(Mutex::new(Counter { count: 1 }));
        let mut saved = autosave(temp.path(), Duration::from_millis(80));
        let observed = Arc::clone(&state);
        saved.watch(move || observed.lock().expect("lock").clone());

        saved.mark_dirty();
        assert!(!saved.tick().expect("tick"), "must not write immediately");
        std::thread::sleep(Duration::from_millis(120));
        assert!(saved.tick().expect("tick"), "must write after the debounce");
        assert_eq!(saved.writes(), 1);
    }

    #[test]
    fn coalesces_a_burst_into_one_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(Mutex::new(Counter { count: 0 }));
        let mut saved = autosave(temp.path(), Duration::from_millis(60));
        let observed = Arc::clone(&state);
        saved.watch(move || observed.lock().expect("lock").clone());

        for _ in 0..5 {
            saved.mark_dirty();
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(80));
        assert!(saved.tick().expect("tick"));
        assert_eq!(saved.writes(), 1);

        let mut loaded = Snapshot::load_from(saved.path())
            .expect("load")
            .expect("file written");
        assert_eq!(
            loaded.restore::<Counter>(),
            Restored::Restored(Counter { count: 0 })
        );
    }

    #[test]
    fn a_later_write_replaces_the_previous_snapshot_atomically() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(Mutex::new(Counter { count: 1 }));
        let mut saved = autosave(temp.path(), Duration::from_secs(0));
        let observed = Arc::clone(&state);
        saved.watch(move || observed.lock().expect("lock").clone());

        saved.flush().expect("first flush");
        state.lock().expect("lock").count = 9;
        saved.flush().expect("second flush");

        let mut loaded = Snapshot::load_from(saved.path())
            .expect("load")
            .expect("file");
        assert_eq!(
            loaded.restore::<Counter>(),
            Restored::Restored(Counter { count: 9 })
        );
        assert!(
            !saved.path().with_extension("json.tmp").exists(),
            "temporary file must be renamed, not left behind"
        );
        assert_eq!(saved.writes(), 2);
    }

    #[test]
    fn the_debounce_restarts_on_each_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut saved = autosave(temp.path(), Duration::from_millis(500));
        saved.watch(|| Counter { count: 0 });

        saved.mark_dirty();
        std::thread::sleep(Duration::from_millis(100));
        saved.mark_dirty();
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !saved.tick().expect("tick"),
            "250 ms after the first change but 150 ms after the last: still quiet-less"
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(saved.tick().expect("tick"), "writes once edits settle");
        assert_eq!(saved.writes(), 1);
    }

    #[test]
    fn missing_file_loads_as_none_and_a_broken_one_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("dev-snapshot.json");
        assert!(Snapshot::load_from(&path).expect("missing").is_none());

        std::fs::write(&path, "not json").expect("write");
        assert!(
            Snapshot::load_from(&path).is_err(),
            "a corrupted snapshot must not look absent"
        );
    }

    #[test]
    fn dev_path_is_derived_from_the_identifier() {
        let base = Path::new("/data");
        assert_eq!(
            Snapshot::path_in(base, "com.example.demo"),
            Path::new("/data/com.example.demo/dev-snapshot.json")
        );
    }
}
