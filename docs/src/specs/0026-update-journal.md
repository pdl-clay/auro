# 0026 - Update journal and restart helper

**Status:** Implemented (2026-11-18; journal, helper-side apply/relaunch, reconciliation and cleanup;
automatic startup recovery, elevation ([spec 0029](0029-linux-package-elevation.md)) and helper-side `.dmg` apply are separate)

Spec 0025 stops after the platform apply. A running app still needs to record what it installed,
hand off after it exits, and relaunch the replacement. This spec fixes that handoff.

## Scope

- `auro-update::ApplyJournal`: a durable, atomically written JSON record for one apply attempt.
- `ApplyKind`: `None`, `AppImage`, `WindowsInstaller`, `MacApp`, `MacPkg`, `DebPackage`, `RpmPackage`.
- States: `Prepared`, `Applied`, `RebootRequired`, `Failed`, `RolledBack`, `Restarted`.
- Helper-side platform apply for a prepared journal, using the installers from specs 0014/0025.
- An optional caller-supplied older `.deb`/`.rpm` in the journal; the helper reinstalls it when the
  primary package apply fails with `InstallerFailed` ([spec 0030](0030-linux-package-rollback.md)).
- `Relaunch`: executable, arguments and working directory for the replacement process.
- `ApplyJournal::spawn_helper`: locks the exit handoff and starts the same executable in helper
  mode.
- `ApplyJournal::reconcile`: classifies the next-start action from state and running version.
- `ApplyJournal::finalize`: removes the retained backup, journal and lock after `Complete`.
- `App::try_run` helper bootstrap: when `RESTART_HELPER_ENV` is set, wait for the old process to
  release the lock and relaunch without starting GPUI.
- Facade re-exports behind the `updater` feature.

## Out of scope

- `.dmg`, `.deb`/`.rpm` automatic rollback and signed macOS helper policy; `.pkg` apply is
  [spec 0031](0031-macos-package-apply.md).
- Root-owned helper installation, UAC, `sudo` or Auro-managed passwords.
- A separate helper binary, service, launchd job or Windows scheduled task.
- State snapshots across the restart; that contract belongs to spec 0004.
- Automatic cleanup of journals, backups or old artifacts.
- Automatically wiring `reconcile`/`finalize` into every app startup; the app calls them.
- Timeouts longer than the fixed helper wait, or a user-facing progress UI.

## Decisions

### D1 - The journal is the durable source of truth

The journal is a small JSON file (format version 2) written with a sibling temporary file,
`sync_all`, and an atomic rename. It stores the format version, release version, artifact path,
optional target and backup, state, diagnostic message, relaunch command and lock path. The reader
checks `format` before decoding the full schema, so an older journal is a typed `JournalFormat`
rather than a missing-field JSON error.

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

### D3 - The helper applies a prepared plan, then relaunches

A `Prepared` journal may carry an `ApplyKind`, the staged manifest and the app-provided public key.
The helper verifies the manifest signature, artifact size and SHA-256, then requires the manifest
version to equal the journal version and the manifest platform to equal this host. This catches
corruption, staging/mixup and wrong-release errors; it is not a defense against a fully forged
journal, because the key and manifest travel in the same local, user-writable file., records a retained backup for AppImage/macOS, marks the journal
`Applied`, and only then relaunches. An `Applied` journal with `ApplyKind::None` keeps the
synchronous-apply behavior: the helper just relaunches. A Windows installer that returns
`3010`/`1641` leaves the journal `RebootRequired` and does not relaunch the old binary as if the
update were complete.

If apply fails, the helper marks the journal `Failed` with the diagnostic, leaves the journal for
inspection and does not relaunch. If the journal is neither `Prepared` with an apply kind nor
`Applied`, the helper refuses to launch anything. Because the helper's stdio is detached, it also
records any later relaunch/lock failure into the journal before returning.

*Rejected:* a second platform-apply implementation inside the helper. The helper reuses the same
`AppImageInstaller`, `WindowsInstaller` and `MacAppInstaller` code. *Rejected:* automatic elevation.
It is still a separate privilege/contract problem.

### D4 - The helper removes its own trigger and detaches

The helper process is started in its own process group (Unix `process_group(0)`, Windows
`CREATE_NEW_PROCESS_GROUP`) so a terminal process-group signal does not kill the handoff. The
relaunched process is spawned with `RESTART_HELPER_ENV` removed, standard streams null, the
journal's working directory and arguments. The helper records `Restarted` after a successful spawn
and exits. It never builds a shell command.

The journal is intentionally left on disk for diagnosis. Cleanup is the app's or a future recovery
policy's job.

### D5 - The journal's directory is a trust boundary

The journal carries the verification key, staged artifact path, platform target and the relaunch
program and arguments. The helper trusts all of them; re-verifying the artifact with the key the
journal names proves internal consistency, not who wrote the file. The journal and its lock must
therefore live in a directory only the current user can write (for example the app's data
directory), and the caller must not point `ApplyJournal` or `RESTART_HELPER_ENV` at a shared
location. A journal planted by another local user would otherwise be able to name its own key,
artifact and relaunch executable, and a package apply would reach the elevation prompt with that
caller-supplied package.

Auro enforces the journal boundary in `ApplyJournal::load`, `save` and the helper's lock open
([spec 0033](0033-update-journal-trust.md)). Unix opens the journal, lock and directories without
following symlinks, requires the file and immediate directory to be owned by the effective user,
refuses group/other write bits, and rejects an ancestor that another user can write (root-owned
sticky ancestors such as `/tmp` are allowed only as ancestors). Windows opens reparse points
themselves, rejects the reparse attribute, and requires the owner and every write/delete/ACL ACE to
belong to the current user, `SYSTEM`, `Administrators` or the creator-owner SIDs. A path that
cannot be proven private is `UpdateError::JournalTrust`; there is no public bypass.

The Windows implementation is compile-checked for `x86_64-pc-windows-msvc` but has not been run on
a Windows host, and it does not walk ancestors; a shared ancestor that can rename the private
directory remains a gap. The staging directory and verify-then-open handoff to an external
installer remain open (RFC 0001). The helper never self-elevates; the elevation prompt and the
package manager remain the user's decision (spec 0029).

## Interfaces

```rust
pub const RESTART_HELPER_ENV: &str = "AURO_UPDATE_RESTART_HELPER";

pub enum ApplyKind {
    None,
    AppImage,
    WindowsInstaller,
    MacApp,
    MacPkg,
}

pub enum ApplyState {
    Prepared,
    Applied,
    RebootRequired,
    Failed,
    RolledBack,
    Restarted,
}

pub struct ApplyPlan {
    pub kind: ApplyKind,
    pub artifact: PathBuf,
    pub target: Option<PathBuf>,
    pub manifest: Manifest,
    pub public_key: PublicKey,
    pub rollback: Option<PackageRollback>,
}

impl ApplyPlan {
    pub fn new(
        kind: ApplyKind,
        artifact: impl Into<PathBuf>,
        target: Option<PathBuf>,
        manifest: Manifest,
        public_key: PublicKey,
    ) -> Self;
    pub fn with_package_rollback(self, rollback: PackageRollback) -> Self;
}

pub struct Relaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl Relaunch {
    pub fn current() -> Result<Self, UpdateError>;
}

pub enum Recovery {
    RetryApply,
    Prepared,
    Complete,
    Relaunch,
    AwaitReboot,
    Failed,
    RolledBack,
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
    pub fn create_for_apply(
        path: impl Into<PathBuf>,
        version: impl Into<String>,
        plan: ApplyPlan,
        relaunch: Relaunch,
    ) -> Result<Self, UpdateError>;
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, UpdateError>;
    pub fn path(&self) -> &Path;
    pub fn state(&self) -> ApplyState;
    pub fn backup(&self) -> Option<&Path>;
    pub fn rollback_artifact(&self) -> Option<&Path>;
    pub fn set_backup(&mut self, backup: impl Into<PathBuf>) -> Result<(), UpdateError>;
    pub fn reconcile(&self, current_version: &str) -> Result<Recovery, UpdateError>;
    pub fn finalize(self) -> Result<(), UpdateError>;
    pub fn mark_applied(&mut self) -> Result<(), UpdateError>;
    pub fn mark_failed(&mut self, message: impl Into<String>) -> Result<(), UpdateError>;
    pub fn mark_rolled_back(&mut self) -> Result<(), UpdateError>;
    pub fn mark_reboot_required(&mut self) -> Result<(), UpdateError>;
    pub fn mark_restarted(&mut self) -> Result<(), UpdateError>;
    pub fn record_message(&mut self, message: impl Into<String>) -> Result<(), UpdateError>;
    pub fn spawn_helper(&self) -> Result<(), UpdateError>;
    pub fn remove(self) -> Result<(), UpdateError>;
}

pub fn run_restart_helper(path: &Path) -> Result<(), UpdateError>;
```

Errors added to `UpdateError`: `JournalMissing`, `JournalFormat`, `JournalNotApplied`,
`JournalLockPath`, `RelaunchMissing`, `ApplyKindMissing`, `ManifestMissing`,
`ApplyTargetMissing`, `UnsupportedApply`, `VerificationKeyMissing`, `JournalVersionMismatch`,
`JournalPlatformMismatch`, `JournalBackupPath`, `JournalBackupTargetMissing`, `RestartLock` and
`RestartTimeout`.

Usage:

```rust,ignore
let journal = auro::update::ApplyJournal::create_for_apply(
    state_dir.join("apply.json"),
    manifest.version.clone(),
    auro::update::ApplyPlan::new(
        auro::update::ApplyKind::AppImage,
        staged.path().to_path_buf(),
        Some(target.clone()),
        manifest.clone(),
        updater.public_key(),
    ),
    auro::update::Relaunch::current()?,
)?;
journal.spawn_helper()?;
cx.quit();
// The helper applies the staged artifact and relaunches after this process exits.
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
   (`uses_the_appimage_path_for_relaunch`). A prepared AppImage plan is verified and applied before
   relaunch, with its backup retained (`helper_applies_an_appimage_before_relaunch`); a tampered
   artifact is rejected (`helper_rejects_a_tampered_staged_artifact`), a version/platform mismatch
   is rejected before apply (`helper_rejects_a_manifest_version_mismatch`,
   `helper_rejects_a_manifest_platform_mismatch`), an unsupported kind marks `Failed`
   (`helper_marks_an_unsupported_apply_failed`), and a reboot request is recorded
   (`records_reboot_state`).
5. The helper refuses an unapplied journal and a state flipped while waiting
   (`helper_rejects_an_unapplied_journal`, `helper_refuses_a_state_flipped_while_waiting`).
6. `reconcile` classifies prepared/applied/current/failed states; `finalize` removes a sibling
   backup, journal and lock, while refusing an unrelated backup
   (`classifies_recovery_actions`, `finalizes_backup_journal_and_lock`,
   `refuses_to_finalize_an_unrelated_backup`, `refuses_to_finalize_a_backup_without_a_target`).
7. A rollback plan survives the journal round trip and is rejected for a non-package apply
   (`round_trips_a_package_rollback_plan`,
   `rejects_a_rollback_plan_for_a_non_package_apply`).
8. A missing relaunch executable is rejected and recorded (`helper_records_a_launch_failure`).
9. A missing, malformed, old-format or non-sibling-lock journal is typed
   (`rejects_a_missing_or_unknown_journal`, `rejects_malformed_json`,
   `reports_an_old_format_before_schema_fields`, `rejects_a_non_sibling_lock_path`).
10. `App::try_run` enters helper mode from `RESTART_HELPER_ENV` before config/GPUI (compile-time
   façade check).
11. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, `cargo check -p auro --features updater --locked`, and
   `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Automatic startup recovery and cleanup policy | Apply/restart follow-up |
| Root-owned helper, polkit policy and an OS service/scheduled task | [Package elevation](0029-linux-package-elevation.md) |
| Automatic package rollback | [Package rollback](0030-linux-package-rollback.md) |
| Journal/backup cleanup policy | Operations follow-up |
| Persistent download resume | Updater resume spec |
| State preservation across the relaunch | Spec 0004 snapshot contract |
