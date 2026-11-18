# 0026 - Update journal and restart helper

**Status:** Implemented (2026-11-18; journal and relaunch helper; the helper does not apply the
artifact, request elevation or recover an interrupted journal)

Spec 0025 stops after the platform apply. A running app still needs to record what it installed,
hand off after it exits, and relaunch the replacement. This spec fixes that handoff.

## Scope

- `auro-update::ApplyJournal`: a durable, atomically written JSON record for one apply attempt.
- States: `Prepared`, `Applied`, `Failed`, `RolledBack`, `Restarted`.
- `Relaunch`: executable, arguments and working directory for the replacement process.
- `ApplyJournal::spawn_helper`: locks the exit handoff and starts the same executable in helper
  mode.
- `App::try_run` helper bootstrap: when `RESTART_HELPER_ENV` is set, wait for the old process to
  release the lock and relaunch without starting GPUI.
- Facade re-exports behind the `updater` feature.

## Out of scope

- Applying the artifact inside the helper. The app applies synchronously first and marks the
  journal `Applied`; the helper only relaunches.
- Elevation, UAC, `sudo` or a privileged helper.
- A separate helper binary, service, launchd job or Windows scheduled task.
- State snapshots across the restart; that contract belongs to spec 0004.
- Automatic cleanup of journals, backups or old artifacts.
- Recovering an interrupted `Prepared` journal on the next launch.
- Timeouts longer than the fixed helper wait, or a user-facing progress UI.

## Decisions

### D1 - The journal is the durable source of truth

The journal is a small JSON file written with a sibling temporary file, `sync_all`, and an atomic
rename. It stores the format version, release version, artifact path, optional target and backup,
state, diagnostic message, relaunch command and lock path.

Every state transition is written through with a sibling temporary file, `sync_all` and rename;
parent-directory sync is not performed. The journal has no implicit default state on load: a
missing, malformed or unknown-format file, or a lock path that is not the journal's sibling, is a
typed error. This makes a crash visible instead of turning it into a fresh update.

*Rejected:* an in-memory receipt. A process that exits for an update cannot report what it was
doing. *Rejected:* using the journal itself as the lock; atomic replacement changes the inode.

### D2 - The helper is the same executable, not a second binary

`spawn_helper` opens a sibling lock file, takes an exclusive lock, and spawns
`std::env::current_exe()` with `AURO_UPDATE_RESTART_HELPER=<journal>`. `App::try_run` sees that
variable before loading config or starting GPUI and calls `run_restart_helper`.

The lock handle is retained in a process-global guard. The helper checks `Applied` before opening
the lock, then polls until the running app exits (120 s timeout), reloads the journal, and requires
`Applied` again; anything else is a typed `JournalNotApplied`. The lock path must be the journal's
sibling, and the relaunch executable must exist before spawning.

This keeps a single artifact to bundle and works for AppImage, Windows installers and macOS apps,
including when the old executable path is replaced while the helper process is still running. On
Linux, `Relaunch::current` uses `$APPIMAGE` because `current_exe()` points at the temporary squashfs
mount, which disappears when the old process exits. The helper is re-executed from the executable
path as it exists after apply; the new build must honor the same environment/journal contract.

*Rejected:* a separate helper crate/binary and packaging convention. It adds a second build artifact
and a discovery problem before the handoff is proven. *Rejected:* a shell loop or `cmd /C` waiting
for a PID. It builds commands from paths and behaves differently per platform.

### D3 - The helper only relaunches

The platform apply stays in the app process (specs 0014/0025), so the same code that verified the
artifact also performs the platform action. The helper does not duplicate installer policy,
elevation or rollback; it launches the replacement only after the app has marked the journal
`Applied`.

If the apply fails, the app writes `Failed` and must not call `spawn_helper`. If the journal is not
`Applied` when the helper runs, the helper refuses to launch anything. Failed helper attempts write
the diagnostic into the journal before returning, because the helper's stdio is detached.

*Rejected:* moving apply into the helper in this slice. It would need a serialized platform apply
plan, a key/verification policy and an elevation contract before it could be safe.

### D4 - The helper removes its own trigger and detaches

The helper process is started in its own process group (Unix `process_group(0)`, Windows
`CREATE_NEW_PROCESS_GROUP`) so a terminal process-group signal does not kill the handoff. The
relaunched process is spawned with `RESTART_HELPER_ENV` removed, standard streams null, the
journal's working directory and arguments. The helper records `Restarted` after a successful spawn
and exits. It never builds a shell command.

The journal is intentionally left on disk for diagnosis. Cleanup is the app's or a future recovery
policy's job.

## Interfaces

```rust
pub const RESTART_HELPER_ENV: &str = "AURO_UPDATE_RESTART_HELPER";

pub enum ApplyState {
    Prepared,
    Applied,
    Failed,
    RolledBack,
    Restarted,
}

pub struct Relaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl Relaunch {
    pub fn current() -> Result<Self, UpdateError>;
}

pub struct ApplyJournal { /* path, data */ }

impl ApplyJournal {
    pub fn create(
        path: impl Into<PathBuf>,
        version: impl Into<String>,
        artifact: impl Into<PathBuf>,
        target: Option<PathBuf>,
        relaunch: Relaunch,
    ) -> Result<Self, UpdateError>;
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, UpdateError>;
    pub fn path(&self) -> &Path;
    pub fn state(&self) -> ApplyState;
    pub fn backup(&self) -> Option<&Path>;
    pub fn set_backup(&mut self, backup: impl Into<PathBuf>) -> Result<(), UpdateError>;
    pub fn mark_applied(&mut self) -> Result<(), UpdateError>;
    pub fn mark_failed(&mut self, message: impl Into<String>) -> Result<(), UpdateError>;
    pub fn mark_rolled_back(&mut self) -> Result<(), UpdateError>;
    pub fn mark_restarted(&mut self) -> Result<(), UpdateError>;
    pub fn record_message(&mut self, message: impl Into<String>) -> Result<(), UpdateError>;
    pub fn spawn_helper(&self) -> Result<(), UpdateError>;
    pub fn remove(self) -> Result<(), UpdateError>;
}

pub fn run_restart_helper(path: &Path) -> Result<(), UpdateError>;
```

Errors added to `UpdateError`: `JournalMissing`, `JournalFormat`, `JournalNotApplied`,
`JournalLockPath`, `RelaunchMissing`, `RestartLock` and `RestartTimeout`.

Usage:

```rust,ignore
let mut journal = auro::update::ApplyJournal::create(
    state_dir.join("apply.json"),
    manifest.version.clone(),
    staged.path().to_path_buf(),
    Some(target.clone()),
    auro::update::Relaunch::current()?,
)?;
let receipt = installer.install(&staged)?;
journal.set_backup(receipt.backup())?;
journal.mark_applied()?;
journal.spawn_helper()?;
cx.quit();
```

The snippet is an intended call shape, not an app that exists yet. The app must hold the journal
until `spawn_helper` returns; the lock is retained by the helper module.

## Acceptance

1. A prepared journal round-trips through disk; state, version, target, backup and relaunch command
   survive (`round_trips_state_through_disk`).
2. Missing and unsupported-format journals are typed errors
   (`rejects_a_missing_or_unknown_journal`).
3. `spawn_helper` refuses a non-`Applied` journal and a held lock (`refuses_to_spawn_before_apply`,
   `refuses_to_spawn_when_the_lock_is_held`), and spawns a helper with the journal path in
   `AURO_UPDATE_RESTART_HELPER` (`spawns_a_helper_with_the_journal_path`).
4. `run_restart_helper` launches the configured command, marks `Restarted`, and leaves the journal
   readable (`helper_relaunches_and_marks_the_journal`); on Linux it honors `$APPIMAGE`
   (`uses_the_appimage_path_for_relaunch`).
5. The helper refuses an unapplied journal and a state flipped while waiting
   (`helper_rejects_an_unapplied_journal`, `helper_refuses_a_state_flipped_while_waiting`).
6. A missing relaunch executable is rejected and recorded (`helper_records_a_launch_failure`).
7. A missing, malformed or non-sibling-lock journal is typed (`rejects_a_missing_or_unknown_journal`,
   `rejects_malformed_json`, `rejects_a_non_sibling_lock_path`).
8. `App::try_run` enters helper mode from `RESTART_HELPER_ENV` before config/GPUI (compile-time
   façade check).
9. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, `cargo check -p auro --features updater --locked`, and
   `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Applying inside the helper and crash recovery | Apply/restart follow-up |
| Elevation and an OS service/scheduled task | Platform update UX/spec |
| Journal/backup cleanup policy | Operations follow-up |
| Persistent download resume | Updater resume spec |
| State preservation across the relaunch | Spec 0004 snapshot contract |
