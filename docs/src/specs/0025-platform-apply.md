# 0025 - Windows and macOS apply

**Status:** Implemented (2026-11-17; Windows MSI/NSIS execution and macOS `.app.zip` replacement;
`.dmg`, notarization and package elevation are separate; macOS `.pkg` apply is [spec 0031](0031-macos-package-apply.md), Linux package elevation is [spec 0029](0029-linux-package-elevation.md); restart handoff is spec 0026)

Spec 0013 stops at a verified staged artifact. Spec 0014 adds the Linux AppImage apply path. This
spec adds the next two platform paths: run a Windows MSI/NSIS installer, and replace a macOS `.app`
bundle from a staged `.zip`. Both consume a [`StagedUpdate`], so the manifest signature, artifact
size and SHA-256 were checked before any platform action.

## Scope

- `auro-update::WindowsInstaller`: detect `.msi`/`.exe`, run the installer silently, and classify
  exit codes `0`, `3010` and `1641`.
- `InstallerRunner`: the command boundary, injectable for tests; no shell.
- `auro-update::MacAppInstaller`: extract a `.zip` containing exactly one top-level `.app`, swap it
  over an existing bundle, and return a rollback receipt.
- `MacAppReceipt`: report the installed bundle and backup, roll back, or discard.
- Facade re-exports behind the `updater` feature, gated by the host OS.

## Out of scope

- Elevation, UAC, `sudo`, admin prompts or a privileged helper process.
- Waiting for the running app to exit, relaunching it, or a restart protocol; the journal/restart
  handoff is [spec 0026](0026-update-journal.md).
- Windows rollback. The MSI/NSIS installer owns its install transaction.
- macOS `.dmg` mounting, notarization and Apple package-signature policy; `.pkg` apply is
  [spec 0031](0031-macos-package-apply.md).
- `.deb`/`.rpm` package-manager apply.
- Preserving extended attributes, ACLs, SELinux labels or ownership beyond what the archive and
  filesystem copy provide.
- Deltas, staged rollout, update channels and downgrade policy.

## Decisions

### D1 - Windows runs the OS installer, never a shell

`WindowsInstaller` inspects the staged artifact extension:

| Format | Program | Arguments |
|---|---|---|
| `.msi` | `%SystemRoot%\System32\msiexec.exe` | `/i <artifact> /qn /norestart` |
| `.exe` | `<artifact>` | `/S` (assumed NSIS) |

`InstallerRunner` receives an argument vector and uses `std::process::Command` directly. MSI uses
`%SystemRoot%\System32\msiexec.exe`, not a PATH lookup. The service does not build a command string,
so a path cannot inject shell syntax. A missing artifact is `StagedArtifactMissing`; an unknown
extension is `UnsupportedInstaller`.

Exit code `0` succeeds. `3010` (reboot required) and `1641` (reboot initiated) succeed with
`restart_required: true`. Every other code, including a terminated process with no code, is
`InstallerFailed`. The app decides when to restart; run the installer off the UI thread.

*Rejected:* a generic "run this command" hook. It moves platform policy into app code and makes
the silent flags, exit-code mapping and test boundary invisible. *Rejected:* downloading the
installer again here; only the verified `StagedUpdate` is accepted.

### D2 - macOS swaps the `.app` directory from a zip

`MacAppInstaller` accepts a staged `.zip` with exactly one top-level `.app`. It bounds the entry
count before extraction (100,000), then extracts with its own loop: destination names must be
enclosed paths, symlinks are recreated only when their target stays inside the extraction root, and
every file is written through a `Read::take` limit and charged for the bytes actually produced. An
entry whose real output differs from its declared `uncompressed_size`, or a total above 4 GiB, is
`UnsafeArchive`, so a ZIP that declares one byte and expands a thousandfold is rejected instead of
filling the disk (#93). A write whose parent chain inside the extraction root contains a symlink is
refused before the bytes are written, because the post-extraction canonical walk would otherwise
run after the escape. Installed modes are explicit and independent of the umask and of the header:
directories end at `0755`, files at `0644`, a declared executable bit yields `0755`, and setuid,
setgid and sticky bits are never reproduced. Implicit parent directories are normalized in the same
pass before the extracted tree is walked for containment, then the old bundle is renamed to a
hidden backup beside it, the new bundle is renamed into place, and `MacAppReceipt` is returned. The
budget bounds a signed artifact that compresses well but expands past the available disk; exceeding
it is `UnsafeArchive`.

The extraction directory is created beside the target, so the final rename stays on one filesystem.
If the new rename fails, the backup is renamed back. The receipt follows the conservative
`AppImage` rule: dropping it keeps the backup; the app calls `discard` after verifying the new
version or `rollback` to restore the old bundle. Rollback removes the installed bundle first because
a directory rename cannot replace a non-empty directory.

Release tooling must preserve executable modes and symlinks in the `.app.zip`; `ditto -c -k` on
macOS is the reference packaging command. The `zip` dependency enables the deflate codec only, so
artifacts using zstd/deflate64 fail with `Archive`.

*Rejected:* applying a `.dmg` by mounting it and copying. That needs `hdiutil`, mount parsing,
detach handling and a helper/privilege story. A zip is a deterministic file format with a safe
extractor and a straightforward test path. *Rejected:* replacing files inside the bundle in place;
it cannot be atomic and leaves mixed versions after a crash.

### D3 - The command runner is the test seam

`InstallerRunner` is public and returns the exit code or `None`. Production uses
`SystemInstallerRunner`; tests inject a recorder and inspect the exact program and argument vector
without launching an installer. Windows command construction and macOS bundle replacement are
therefore tested on Linux CI, while the host-specific types remain exported only on their host.

*Rejected:* `#[cfg(test)]` branches inside the production install function. The runner keeps the
test double explicit and lets an app provide a custom runner if needed.

### D4 - The installer does not restart or elevate

A Windows installer may return `3010` or `1641`, and a macOS bundle is used on the next launch.
Neither
installer waits for the current process, relaunches the app, or requests elevation. Those are a
separate restart/helper contract; claiming them here would be false.

## Interfaces

```rust
pub trait InstallerRunner {
    fn run(&self, program: &Path, args: &[OsString])
        -> Result<Option<i32>, UpdateError>;
}

pub struct SystemInstallerRunner;

pub enum WindowsInstallerKind { Msi, Nsis }

pub struct WindowsInstallOutcome {
    pub kind: WindowsInstallerKind,
    pub restart_required: bool,
}

pub struct WindowsInstaller { /* runner */ }

impl WindowsInstaller {
    pub fn new() -> Self;
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self;
    pub fn install(
        &self,
        staged: &StagedUpdate,
    ) -> Result<WindowsInstallOutcome, UpdateError>;
}

pub struct MacAppInstaller { /* target */ }

impl MacAppInstaller {
    pub fn new(target: impl Into<PathBuf>) -> Self;
    pub fn target(&self) -> &Path;
    pub fn install(&self, staged: &StagedUpdate) -> Result<MacAppReceipt, UpdateError>;
}

pub struct MacAppReceipt { /* target, backup */ }

impl MacAppReceipt {
    pub fn target(&self) -> &Path;
    pub fn backup(&self) -> &Path;
    pub fn rollback(self) -> Result<(), UpdateError>;
    pub fn discard(self) -> Result<(), UpdateError>;
}
```

`WindowsInstaller` is re-exported on Windows, and `MacAppInstaller`/`MacAppReceipt` on macOS. Both
are unit-tested on Linux through `cfg(test)`.

Errors added to `UpdateError`: `InstallTargetNotDirectory`, `UnsupportedInstaller`,
`InstallerFailed`, `AppBundleMissing`, `AppBundleAmbiguous`, `Archive`, `UnsafeArchive`,
`RestoreFailed` and `RollbackFailed`.

Usage:

```rust,ignore
let staged = updater.download(&manifest)?;
#[cfg(target_os = "windows")]
let outcome = auro::update::WindowsInstaller::new().install(&staged)?;
#[cfg(target_os = "macos")]
let receipt = auro::update::MacAppInstaller::new("/Applications/Demo.app").install(&staged)?;
```

The snippet is an intended call shape, not an app that exists yet.

## Acceptance

1. MSI runs `msiexec` with `/i`, the staged path, `/qn` and `/norestart`
   (`runs_msi_installer_silently`).
2. NSIS runs the staged `.exe` with `/S` (`runs_nsis_installer_silently`).
3. `3010`/`1641` report `restart_required`; another code is `InstallerFailed` and keeps the code
   (`maps_reboot_required_and_failure`).
4. An unknown Windows artifact is `UnsupportedInstaller` (`rejects_unknown_artifacts`).
5. A `.app.zip` replaces the bundle, retains the old bundle in the receipt, and rollback restores
   the old contents (`installs_and_rolls_back_an_app_bundle`).
6. `discard` removes the backup (`discards_the_backup_after_success`).
7. An archive with no `.app` fails with `AppBundleMissing` and leaves the target unchanged
   (`rejects_an_archive_without_an_app_bundle`).
8. A `.dmg` staged artifact is rejected as unsupported (`rejects_an_unsupported_archive`).
9. A symlink in the extracted bundle that resolves outside the extraction tree is rejected as
   `UnsafeArchive` (`rejects_symlinks_escaping_the_bundle`), while an in-bundle symlink is allowed
   (`allows_symlinks_inside_the_bundle`).
10. A zip with two top-level `.app` bundles is rejected without touching the target
    (`rejects_an_ambiguous_archive`).
11. Missing/non-directory macOS targets fail with `InstallTargetMissing`/
    `InstallTargetNotDirectory`, and a corrupt zip fails with `Archive` before touching the target
    (`rejects_missing_or_non_directory_targets`, `corrupt_archive_leaves_the_target_untouched`).
12. A missing staged artifact is `StagedArtifactMissing`
    (`reports_a_missing_staged_artifact`, macOS `rejects_a_missing_staged_artifact`), a terminated
    Windows installer is `InstallerFailed { code: None }` (`reports_a_terminated_installer`), and a
    macOS rollback with a missing backup reports `RollbackBackupMissing`
    (`rollback_reports_a_missing_backup`).
13. An archive over 100,000 entries, or whose real extracted bytes exceed 4 GiB or differ from the
    declared size of an entry, is rejected as `UnsafeArchive`
    (`rejects_archives_over_the_expansion_budget`,
    `rejects_a_declared_size_lie_instead_of_expanding_it`). A symlink entry and a later entry
    written through that symlink are rejected before the bytes leave the extraction root
    (`rejects_a_symlink_entry_escaping_the_extraction_root`,
    `rejects_a_file_written_through_a_symlinked_parent`). Extracted modes ignore the umask:
    directories and declared executables are `0755`, other files `0644`, and setuid is dropped
    (`extraction_modes_do_not_inherit_the_umask`).
14. `cargo fmt --all --check`,
    `cargo clippy --workspace --all-targets --locked -- -D warnings`,
    `cargo test --workspace --locked`, `cargo test -p auro-update --locked`,
    `cargo check -p auro --features updater --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Windows elevation, reboot and installer UI policy | Windows update UX/spec |
| macOS `.dmg` apply | macOS disk-image apply spec |
| macOS `.pkg` apply | [macOS package apply](0031-macos-package-apply.md) |
| `.deb`/`.rpm` package-manager apply | [Linux package apply](0027-linux-packages.md), [elevation](0029-linux-package-elevation.md) |
| Restart/helper protocol and crash-safe apply journal | [Update journal and restart helper](0026-update-journal.md) |
| macOS code-signing preservation and notarization checks | [macOS notarization](0032-macos-notarization.md) |
| Automatic downgrade policy | [Linux package rollback](0030-linux-package-rollback.md) |
