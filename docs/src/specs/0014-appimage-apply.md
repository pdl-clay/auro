# 0014 - AppImage apply and rollback

**Status:** Implemented (2026-11-17; Linux AppImage only; Windows/macOS apply is
[spec 0025](0025-platform-apply.md), restart handoff is [spec 0026](0026-update-journal.md);
helper-side apply and package-manager installers are separate; elevation is [spec 0029](0029-linux-package-elevation.md))

Spec 0013 stops at a verified staged artifact. This spec adds the first apply path: replacing a
single-file Linux AppImage, with a receipt that can roll the previous file back. It deliberately
does not generalise installation into a command hook.

## Scope

- `auro-update::AppImageInstaller`: install a staged artifact over an existing AppImage.
- `InstallReceipt`: report the installed target and retained backup, roll back, or discard the
  backup.
- Atomic in-place replacement, executable permissions, and a backup that survives a crash until
  explicitly discarded.

## Out of scope

- `.deb`/`.rpm` package installation, macOS `.dmg`/`.pkg`, and the Windows/MSI/NSIS and macOS
  `.app` details owned by [spec 0025](0025-platform-apply.md).
- Restarting the app, waiting for a process to exit, or a helper-process protocol; the journal/
  restart handoff is [spec 0026](0026-update-journal.md).
- Deltas, staged rollout, update channels and downgrade policy.
- Preserving owner, group, extended attributes, SELinux labels or other file metadata beyond mode.
- Signature verification; the artifact is already a [`StagedUpdate`](0013-updater-runtime.md).

## Decisions

### D1 - AppImage is replaced as a single file

An AppImage is one executable file on Linux. Installation resolves the target through symlinks,
writes the staged bytes to a temporary file in the target directory, sets mode `0755`, syncs it,
and renames it over the target. On Linux, the running process keeps the old inode, so replacing a
running AppImage is safe; the new file is used on the next launch.

*Rejected:* extracting or patching the AppImage. Auro ships the whole artifact and verifies its
hash in the staging layer.

### D2 - A backup is created before the replacement

The old target is retained beside the installed file. A hard link is preferred because it is
instant and preserves the old inode and metadata; a copy is the fallback for filesystems without
hard links. The backup path is hidden, name-scoped to the target, and unique per process/timestamp.

`InstallReceipt` owns the backup. Dropping it does not delete the backup: an app must choose
`discard` after verifying the new version, or `rollback` to restore. This makes the default failure
mode conservative, at the cost of leaving a backup until the app acts.

### D3 - Rollback is an atomic rename of the backup

`InstallReceipt::rollback` renames the backup over the target. A missing backup is a typed error and
leaves the installed file in place. The receipt is consumed; a caller that needs to retry can
inspect the path before consuming it. `discard` removes the backup and is idempotent.

*Rejected:* deleting the old file immediately. A bad update would then require re-downloading the
old release, which may no longer be served.

### D4 - Apply is Linux-only, not a generic installer

`AppImageInstaller` and `InstallReceipt` are compiled only on Linux. Windows installers, macOS app
bundles and distribution packages have different restart, privilege and rollback requirements and
get their own contracts. Auro does not expose a "run this command to install" hook, which would move
platform policy into every app and make rollback unverifiable.

### D5 - No automatic restart

Installing does not relaunch the app or wait for it to exit. The caller decides when to restart
after installation and when to call `discard`. A restart/helper protocol is a separate slice.

## Interfaces

```rust
pub struct AppImageInstaller { /* target path */ }

impl AppImageInstaller {
    pub fn new(target: impl Into<PathBuf>) -> Self;
    pub fn target(&self) -> &Path;
    pub fn install(&self, staged: &StagedUpdate) -> Result<InstallReceipt, UpdateError>;
}

pub struct InstallReceipt { /* installed target, backup */ }

impl InstallReceipt {
    pub fn target(&self) -> &Path;
    pub fn backup(&self) -> &Path;
    pub fn rollback(self) -> Result<(), UpdateError>;
    pub fn discard(self) -> Result<(), UpdateError>;
}
```

Errors added to `UpdateError`: `InstallTargetMissing`, `InstallTargetNotFile`,
`StagedArtifactMissing` and `RollbackBackupMissing`.

Usage:

```rust,ignore
let staged = updater.download(&manifest)?;
let receipt = auro::update::AppImageInstaller::new(current_appimage).install(&staged)?;
// Restart into the new version, then:
receipt.discard()?;
```

The snippet is an intended call shape, not code that exists as an app.

## Acceptance

1. Installing replaces the target bytes, sets mode `0755`, and retains the old bytes in the
   receipt's backup; rollback restores the old bytes and mode
   (`installs_and_rolls_back_an_appimage`).
2. `discard` removes the backup and leaves the installed file (`discards_the_backup_after_success`).
3. A missing target and a non-file target fail with typed errors
   (`rejects_a_missing_or_non_file_target`).
4. A missing staged artifact fails without changing the target
   (`missing_staged_artifact_leaves_the_target_untouched`).
5. A rollback with a missing backup reports `RollbackBackupMissing` and leaves the installed file
   (`rollback_reports_a_missing_backup`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Windows MSI/NSIS apply and privilege handling | [Windows and macOS apply](0025-platform-apply.md) / Windows UX |
| macOS `.dmg`/`.pkg` replacement and notarization | macOS updater spec |
| `.deb`/`.rpm` package-manager updates | [Linux package apply](0027-linux-packages.md) |
| Restart/helper protocol and crash-safe receipt | [Update journal and restart helper](0026-update-journal.md) |
| Preserving xattrs, SELinux labels and ownership | Platform apply hardening |
| Rollout, deltas and downgrade policy | Updater rollout spec |
