// SPDX-License-Identifier: MIT OR Apache-2.0

//! Versioned, opt-in state snapshots (spec 0004, D3).
//!
//! The envelope is JSON. Each entry carries its own version, so an incompatible entry resets on
//! its own and is reported; a snapshot that cannot be understood at all is an error, never a
//! silent reset.

use std::collections::BTreeMap;

use auro_ipc::AuroError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Snapshot schema version written by this build.
pub const FORMAT_VERSION: u32 = 1;

/// State that opts into preservation across a reload or a state-preserving restart.
///
/// `KEY` must stay stable for the same logical state within a dev session: renaming or removing it
/// is a reset of that entry, and the reset is reported by [`Snapshot::restore`].
pub trait ReloadSnapshot: Serialize + DeserializeOwned {
    const KEY: &'static str;
    const VERSION: u32;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    version: u32,
    payload: serde_json::Value,
}

/// A versioned envelope of named, typed entries plus the identity of the build that wrote it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    format_version: u32,
    build: String,
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

/// What [`Snapshot::restore`] did with one entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Restored<T> {
    /// The entry was present, version-compatible and decodable.
    Restored(T),
    /// The entry was dropped; the caller starts from its default and must surface this.
    Reset(ResetReason),
}

/// Why an entry was reset. Every variant is diagnostic text the dev loop can print.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResetReason {
    Missing,
    VersionMismatch { found: u32, expected: u32 },
    Decode(String),
}

impl ResetReason {
    /// One line an app or the dev loop can show without matching on the variant.
    pub fn message(&self) -> String {
        match self {
            ResetReason::Missing => "no snapshot entry; state starts from its default".to_owned(),
            ResetReason::VersionMismatch { found, expected } => format!(
                "snapshot entry version {found} cannot be read by version {expected}; state starts from its default"
            ),
            ResetReason::Decode(error) => {
                format!("snapshot entry did not decode ({error}); state starts from its default")
            }
        }
    }
}

impl Snapshot {
    /// Starts an empty snapshot for the build that is saving it.
    pub fn new(build: impl Into<String>) -> Self {
        Snapshot {
            format_version: FORMAT_VERSION,
            build: build.into(),
            entries: BTreeMap::new(),
        }
    }

    /// Identity of the build that wrote the snapshot. Diagnostic only: a rebuild changes it.
    pub fn build(&self) -> &str {
        &self.build
    }

    /// Number of entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes `value` under its stable key, replacing any previous entry for that key.
    pub fn insert<T: ReloadSnapshot>(&mut self, value: &T) -> Result<(), AuroError> {
        let payload = serde_json::to_value(value)?;
        self.entries.insert(
            T::KEY.to_owned(),
            Entry {
                version: T::VERSION,
                payload,
            },
        );
        Ok(())
    }

    /// Takes one entry out of the snapshot.
    ///
    /// An entry is consumed: restoring twice reports [`ResetReason::Missing`] the second time. A
    /// version or decode mismatch drops that entry and returns the reason; it never substitutes a
    /// default silently.
    pub fn restore<T: ReloadSnapshot>(&mut self) -> Restored<T> {
        let Some(entry) = self.entries.remove(T::KEY) else {
            return Restored::Reset(ResetReason::Missing);
        };
        if entry.version != T::VERSION {
            return Restored::Reset(ResetReason::VersionMismatch {
                found: entry.version,
                expected: T::VERSION,
            });
        }
        match serde_json::from_value(entry.payload) {
            Ok(value) => Restored::Restored(value),
            Err(error) => Restored::Reset(ResetReason::Decode(error.to_string())),
        }
    }

    /// Encodes the envelope as JSON.
    pub fn encode(&self) -> Result<String, AuroError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Decodes an envelope written by [`Snapshot::encode`].
    ///
    /// A format version this build does not understand is an error: the whole file is unusable, so
    /// silently starting from defaults would hide a real incompatibility.
    pub fn decode(raw: &str) -> Result<Self, AuroError> {
        let snapshot: Snapshot = serde_json::from_str(raw)?;
        if snapshot.format_version != FORMAT_VERSION {
            return Err(AuroError::Message(format!(
                "snapshot format {} is not supported (this build writes {FORMAT_VERSION})",
                snapshot.format_version
            )));
        }
        Ok(snapshot)
    }

    /// Writes the envelope to `path`, replacing it atomically.
    ///
    /// The bytes go to a sibling temporary file which is then renamed, so a crash or a kill leaves
    /// either the previous snapshot or the new one, never a half-written file.
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), AuroError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, self.encode()?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    /// Reads an envelope, or `None` when the file does not exist yet.
    ///
    /// A file that exists but cannot be read is an error: treating it as absent would silently
    /// discard the user's state.
    pub fn load_from(path: &std::path::Path) -> Result<Option<Self>, AuroError> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Snapshot::decode(&raw).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Path of the dev snapshot for this app identifier.
    ///
    /// Platform data directory: `%LOCALAPPDATA%` on Windows, `Application Support` on macOS,
    /// `$XDG_DATA_HOME` (or `~/.local/share`) elsewhere.
    pub fn dev_path(identifier: &str) -> Option<std::path::PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|home| std::path::PathBuf::from(home).join(".local").join("share"))
                })
        }?;
        Some(Self::path_in(&base, identifier))
    }

    /// The dev snapshot path under an explicit data directory. Split out so it is testable
    /// without touching the process environment.
    pub fn path_in(base: &std::path::Path, identifier: &str) -> std::path::PathBuf {
        base.join(identifier).join("dev-snapshot.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Counter {
        count: u64,
    }

    impl ReloadSnapshot for Counter {
        const KEY: &'static str = "demo.counter";
        const VERSION: u32 = 1;
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct CounterV2 {
        count: u64,
    }

    impl ReloadSnapshot for CounterV2 {
        const KEY: &'static str = "demo.counter";
        const VERSION: u32 = 2;
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Filter {
        text: String,
    }

    impl ReloadSnapshot for Filter {
        const KEY: &'static str = "demo.filter";
        const VERSION: u32 = 1;
    }

    #[test]
    fn round_trips_through_the_encoded_envelope() {
        let mut saved = Snapshot::new("build-1");
        saved.insert(&Counter { count: 7 }).expect("insert");
        saved
            .insert(&Filter { text: "abc".into() })
            .expect("insert");
        let raw = saved.encode().expect("encode");

        let mut loaded = Snapshot::decode(&raw).expect("decode");
        assert_eq!(loaded.build(), "build-1");
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.restore::<Counter>(),
            Restored::Restored(Counter { count: 7 })
        );
        assert_eq!(
            loaded.restore::<Filter>(),
            Restored::Restored(Filter { text: "abc".into() })
        );
        assert_eq!(loaded.len(), 0);
    }

    #[test]
    fn version_mismatch_resets_only_that_entry() {
        let mut saved = Snapshot::new("build-1");
        saved.insert(&Counter { count: 3 }).expect("insert");
        saved
            .insert(&Filter {
                text: "keep".into(),
            })
            .expect("insert");
        let raw = saved.encode().expect("encode");
        let mut loaded = Snapshot::decode(&raw).expect("decode");

        assert_eq!(
            loaded.restore::<CounterV2>(),
            Restored::Reset(ResetReason::VersionMismatch {
                found: 1,
                expected: 2
            })
        );
        assert_eq!(
            loaded.restore::<Filter>(),
            Restored::Restored(Filter {
                text: "keep".into()
            })
        );
    }

    #[test]
    fn a_missing_entry_is_reported_rather_than_defaulted() {
        let mut snapshot = Snapshot::new("build-1");
        assert_eq!(
            snapshot.restore::<Counter>(),
            Restored::Reset(ResetReason::Missing)
        );
    }

    #[test]
    fn an_undecodable_payload_resets_with_the_error() {
        let mut saved = Snapshot::new("build-1");
        saved
            .insert(&Filter { text: "abc".into() })
            .expect("insert");
        let mut raw: serde_json::Value =
            serde_json::from_str(&saved.encode().expect("encode")).expect("json");
        raw["entries"]["demo.counter"] = serde_json::json!({
            "version": 1,
            "payload": {"count": "not a number"}
        });
        let mut loaded = Snapshot::decode(&raw.to_string()).expect("decode");

        match loaded.restore::<Counter>() {
            Restored::Reset(ResetReason::Decode(error)) => {
                assert!(!error.is_empty(), "decode reason must be diagnostic")
            }
            other => panic!("expected a decode reset, got {other:?}"),
        }
        assert_eq!(
            loaded.restore::<Filter>(),
            Restored::Restored(Filter { text: "abc".into() })
        );
    }

    #[test]
    fn rejects_a_format_version_this_build_does_not_understand() {
        let mut raw: serde_json::Value =
            serde_json::from_str(&Snapshot::new("build-1").encode().expect("encode"))
                .expect("json");
        raw["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
        let error = Snapshot::decode(&raw.to_string()).expect_err("rejects");
        assert!(
            error.to_string().contains("not supported"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn reset_reasons_are_usable_as_text() {
        assert!(
            ResetReason::VersionMismatch {
                found: 1,
                expected: 2
            }
            .message()
            .contains("version 1")
        );
        assert!(ResetReason::Missing.message().contains("default"));
    }
}
