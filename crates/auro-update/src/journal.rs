// SPDX-License-Identifier: MIT OR Apache-2.0

//! Durable apply journal and the restart helper handshake.
//!
//! The app writes a journal, then spawns the same executable in helper mode. A prepared journal
//! carries the platform action; the helper waits for the app to release a lock file, applies the
//! staged artifact through the platform installers, and relaunches the new executable. An
//! already-applied journal is relaunch-only. The journal is written atomically and remains on disk
//! for diagnosis after the restart.

use std::ffi::OsStr;
use std::fs::{File, TryLockError};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{JOURNAL_FORMAT_VERSION, Manifest, PublicKey, StagedUpdate, UpdateError, trust};

/// Environment variable used to enter restart-helper mode.
pub const RESTART_HELPER_ENV: &str = "AURO_UPDATE_RESTART_HELPER";

const LOCK_WAIT: Duration = Duration::from_secs(120);
const LOCK_POLL: Duration = Duration::from_millis(50);

static HELPER_LOCKS: OnceLock<Mutex<Vec<File>>> = OnceLock::new();

/// The platform action a prepared helper performs before relaunching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyKind {
    /// No platform apply; the helper only relaunches an already-applied update.
    #[default]
    None,
    /// Linux AppImage replacement.
    AppImage,
    /// Windows MSI/NSIS installer execution.
    WindowsInstaller,
    /// Debian package installation.
    DebPackage,
    /// RPM package installation.
    RpmPackage,
    /// macOS `.app` bundle replacement.
    MacApp,
    /// macOS package installation.
    MacPkg,
}

fn kind_supported(kind: ApplyKind) -> bool {
    match kind {
        ApplyKind::None => false,
        ApplyKind::AppImage | ApplyKind::DebPackage | ApplyKind::RpmPackage => {
            cfg!(target_os = "linux")
        }
        ApplyKind::WindowsInstaller => cfg!(target_os = "windows"),
        ApplyKind::MacApp | ApplyKind::MacPkg => cfg!(target_os = "macos"),
    }
}

fn preflight_target(kind: ApplyKind, target: Option<&Path>) -> Result<(), UpdateError> {
    match kind {
        ApplyKind::AppImage => {
            let target = target.ok_or(UpdateError::ApplyTargetMissing { kind })?;
            if !target.exists() {
                return Err(UpdateError::InstallTargetMissing {
                    path: target.to_path_buf(),
                });
            }
            if !target.is_file() {
                return Err(UpdateError::InstallTargetNotFile {
                    path: target.to_path_buf(),
                });
            }
            preflight_writable_parent(target)?;
        }
        ApplyKind::MacApp => {
            let target = target.ok_or(UpdateError::ApplyTargetMissing { kind })?;
            if !target.exists() {
                return Err(UpdateError::InstallTargetMissing {
                    path: target.to_path_buf(),
                });
            }
            if !target.is_dir() {
                return Err(UpdateError::InstallTargetNotDirectory {
                    path: target.to_path_buf(),
                });
            }
            preflight_writable_parent(target)?;
        }
        _ => {}
    }
    Ok(())
}

fn preflight_writable_parent(target: &Path) -> Result<(), UpdateError> {
    // The installers resolve symlinks before creating their backup and temporary file. Check the
    // real parent, not the (possibly writable) directory that holds the link (#83).
    let resolved =
        std::fs::canonicalize(target).map_err(|error| UpdateError::InstallTargetNotWritable {
            path: target.to_path_buf(),
            message: format!("cannot resolve the target: {error}"),
        })?;
    let parent = resolved
        .parent()
        .ok_or_else(|| UpdateError::InstallTargetNotWritable {
            path: target.to_path_buf(),
            message: "has no parent directory".to_owned(),
        })?;
    tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        UpdateError::InstallTargetNotWritable {
            path: target.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    Ok(())
}

fn preflight_artifact(kind: ApplyKind, path: &Path) -> Result<(), UpdateError> {
    let (expected, matches) = match kind {
        // The AppImage installer accepts any file name; requiring `.AppImage` rejected valid
        // artifacts (issue #60).
        ApplyKind::AppImage => return Ok(()),
        ApplyKind::WindowsInstaller => (
            ".msi or .exe",
            has_extension(path, "msi") || has_extension(path, "exe"),
        ),
        ApplyKind::DebPackage => (".deb", has_extension(path, "deb")),
        ApplyKind::RpmPackage => (".rpm", has_extension(path, "rpm")),
        ApplyKind::MacApp => (".zip containing a .app bundle", has_extension(path, "zip")),
        ApplyKind::MacPkg => (".pkg", has_extension(path, "pkg")),
        ApplyKind::None => return Ok(()),
    };
    if matches {
        Ok(())
    } else {
        Err(UpdateError::UnsupportedInstaller {
            path: path.to_path_buf(),
            expected,
        })
    }
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn preflight_package_manager(kind: ApplyKind) -> Result<(), UpdateError> {
    if let Some(program) = missing_package_manager(kind, |path| path.is_file()) {
        return Err(UpdateError::PackageManagerMissing { kind, program });
    }
    Ok(())
}

fn missing_package_manager(kind: ApplyKind, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let programs: &[&str] = match kind {
        ApplyKind::DebPackage => &["/usr/bin/pkexec", "/usr/bin/dpkg"],
        ApplyKind::RpmPackage => &["/usr/bin/pkexec", "/usr/bin/rpm"],
        _ => return None,
    };
    programs
        .iter()
        .map(Path::new)
        .find(|program| !exists(program))
        .map(Path::to_path_buf)
}

/// A caller-supplied older package to reinstall if the primary package apply fails.
///
/// The artifact and manifest use the same contract as a normal update: the helper verifies the
/// signature, platform, size and SHA-256 again before running an elevated downgrade. Auro never
/// discovers or retains this package; the app or release policy supplies it.
#[derive(Clone, Debug)]
pub struct PackageRollback {
    /// Already-staged `.deb` or `.rpm`.
    pub artifact: PathBuf,
    /// Signed manifest describing the older package.
    pub manifest: Manifest,
    /// Public key pinned by the app.
    pub public_key: PublicKey,
}

impl PackageRollback {
    /// Creates a package rollback from an already-staged artifact.
    pub fn new(artifact: impl Into<PathBuf>, manifest: Manifest, public_key: PublicKey) -> Self {
        PackageRollback {
            artifact: artifact.into(),
            manifest,
            public_key,
        }
    }
}

/// The platform action and verification data for a helper-side apply.
#[derive(Clone, Debug)]
pub struct ApplyPlan {
    /// Platform action.
    pub kind: ApplyKind,
    /// Already-staged, already-downloaded artifact.
    pub artifact: PathBuf,
    /// Platform target (AppImage path or macOS `.app`).
    pub target: Option<PathBuf>,
    /// Signed manifest describing the artifact.
    pub manifest: Manifest,
    /// Public key pinned by the app.
    pub public_key: PublicKey,
    /// Older package to reinstall if a `.deb`/`.rpm` apply fails.
    pub rollback: Option<PackageRollback>,
}

impl ApplyPlan {
    /// Creates an apply plan.
    pub fn new(
        kind: ApplyKind,
        artifact: impl Into<PathBuf>,
        target: Option<PathBuf>,
        manifest: Manifest,
        public_key: PublicKey,
    ) -> Self {
        ApplyPlan {
            kind,
            artifact: artifact.into(),
            target,
            manifest,
            public_key,
            rollback: None,
        }
    }

    /// Attaches an older package that the helper applies if the primary package apply fails.
    ///
    /// Only valid for [`ApplyKind::DebPackage`] and [`ApplyKind::RpmPackage`];
    /// [`ApplyJournal::create_for_apply`] rejects any other kind.
    pub fn with_package_rollback(mut self, rollback: PackageRollback) -> Self {
        self.rollback = Some(rollback);
        self
    }
}

/// What the next app start should do with a journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// A prepared plan needs the helper to apply it.
    RetryApply,
    /// The journal is prepared but has no apply kind; inspect it manually.
    Prepared,
    /// The new version is already running; finalize the journal.
    Complete,
    /// The update is applied but the current process is not the new version; relaunch.
    Relaunch,
    /// The installer asked for a reboot; do not relaunch as if complete.
    AwaitReboot,
    /// The apply failed; retain the journal and backup for diagnosis/rollback.
    Failed,
    /// The previous artifact was restored.
    RolledBack,
}

/// The durable state of one apply/restart attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyState {
    /// The journal exists, but no platform apply has completed.
    Prepared,
    /// The platform apply completed and a restart may be requested.
    Applied,
    /// The apply failed. The journal keeps the diagnostic message.
    Failed,
    /// The previous artifact was restored.
    RolledBack,
    /// The installer completed but requires a reboot before the new version is usable.
    RebootRequired,

    /// The helper launched the replacement executable.
    Restarted,
}

/// The command the helper launches after the app exits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relaunch {
    /// Executable to launch.
    pub program: PathBuf,
    /// Arguments after the executable name.
    pub args: Vec<String>,
    /// Working directory for the new process.
    pub cwd: PathBuf,
}

impl Relaunch {
    /// Captures the executable, arguments and working directory for a relaunch.
    ///
    /// On Linux an AppImage launch uses `$APPIMAGE`; `current_exe()` points at the temporary
    /// squashfs mount, which disappears when the old process exits. Non-UTF-8 arguments are
    /// converted lossily because the journal is JSON.
    pub fn current() -> Result<Self, UpdateError> {
        let program = relaunch_program(std::env::var_os("APPIMAGE"))?;
        let args = std::env::args_os()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let cwd = std::env::current_dir()?;
        Ok(Relaunch { program, args, cwd })
    }
}

fn relaunch_program(appimage: Option<std::ffi::OsString>) -> Result<PathBuf, UpdateError> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = appimage.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(appimage));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = appimage;
    Ok(std::env::current_exe()?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JournalSpec {
    artifact: PathBuf,
    target: Option<PathBuf>,
    kind: ApplyKind,
    manifest: Option<Manifest>,
    verify_key: Option<String>,
    rollback: Option<RollbackData>,
    relaunch: Relaunch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RollbackData {
    artifact: PathBuf,
    manifest: Manifest,
    verify_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JournalData {
    format: u32,
    version: String,
    #[serde(default)]
    kind: ApplyKind,
    #[serde(default)]
    manifest: Option<Manifest>,
    #[serde(default)]
    verify_key: Option<String>,
    #[serde(default)]
    rollback: Option<RollbackData>,
    artifact: PathBuf,
    target: Option<PathBuf>,
    backup: Option<PathBuf>,
    state: ApplyState,
    message: Option<String>,
    relaunch: Relaunch,
    lock_file: PathBuf,
}

/// A persisted update journal.
#[derive(Clone, Debug)]
pub struct ApplyJournal {
    path: PathBuf,
    data: JournalData,
}

impl ApplyJournal {
    /// Creates and writes a `Prepared` journal.
    ///
    /// The lock file sits beside the journal (`<name>.lock`).
    pub fn create(
        path: impl Into<PathBuf>,
        version: impl Into<String>,
        artifact: impl Into<PathBuf>,
        target: Option<PathBuf>,
        relaunch: Relaunch,
    ) -> Result<Self, UpdateError> {
        Self::create_with(
            path,
            version,
            JournalSpec {
                artifact: artifact.into(),
                target,
                kind: ApplyKind::None,
                manifest: None,
                verify_key: None,
                rollback: None,
                relaunch,
            },
        )
    }

    /// Creates a `Prepared` journal whose helper applies `plan` before relaunching.
    ///
    /// The manifest and app-provided public key are kept so the helper can re-verify the
    /// artifact's signature, size and SHA-256 before applying it.
    pub fn create_for_apply(
        path: impl Into<PathBuf>,
        version: impl Into<String>,
        plan: ApplyPlan,
        relaunch: Relaunch,
    ) -> Result<Self, UpdateError> {
        if plan.rollback.is_some()
            && !matches!(plan.kind, ApplyKind::DebPackage | ApplyKind::RpmPackage)
        {
            return Err(UpdateError::RollbackPlanUnsupported { kind: plan.kind });
        }
        let version = version.into();
        Self::preflight_plan(&version, &plan)?;
        let rollback = plan.rollback.map(|rollback| RollbackData {
            artifact: rollback.artifact,
            manifest: rollback.manifest,
            verify_key: Some(rollback.public_key.to_base64()),
        });
        Self::create_with(
            path,
            version,
            JournalSpec {
                artifact: plan.artifact,
                target: plan.target,
                kind: plan.kind,
                manifest: Some(plan.manifest),
                verify_key: Some(plan.public_key.to_base64()),
                rollback,
                relaunch,
            },
        )
    }

    /// Runs the helper's validation while the app is still alive.
    ///
    /// The helper repeats every check after the handoff (defense in depth), but a plan that fails
    /// here must not close the app: the helper would mark the journal `Failed` and never relaunch.
    fn preflight_plan(version: &str, plan: &ApplyPlan) -> Result<(), UpdateError> {
        if plan.kind == ApplyKind::None {
            return Err(UpdateError::ApplyKindMissing);
        }
        if !kind_supported(plan.kind) {
            return Err(UpdateError::UnsupportedApply { kind: plan.kind });
        }
        preflight_target(plan.kind, plan.target.as_deref())?;
        plan.manifest
            .verify_artifact(&plan.artifact, &plan.public_key)?;
        if plan.manifest.version != version {
            return Err(UpdateError::JournalVersionMismatch {
                journal: version.to_owned(),
                manifest: plan.manifest.version.clone(),
            });
        }
        let host = crate::current_platform();
        if plan.manifest.platform != host {
            return Err(UpdateError::JournalPlatformMismatch {
                expected: host,
                actual: plan.manifest.platform.clone(),
            });
        }
        preflight_artifact(plan.kind, &plan.artifact)?;
        if let Some(rollback) = &plan.rollback {
            rollback
                .manifest
                .verify_artifact(&rollback.artifact, &rollback.public_key)?;
            if rollback.manifest.platform != crate::current_platform() {
                return Err(UpdateError::PlatformMismatch {
                    expected: crate::current_platform(),
                    actual: rollback.manifest.platform.clone(),
                });
            }
            preflight_artifact(plan.kind, &rollback.artifact)?;
            let failed =
                semver::Version::parse(version).map_err(|_| UpdateError::InvalidVersion {
                    field: "journal version",
                    value: version.to_owned(),
                })?;
            let older = semver::Version::parse(&rollback.manifest.version).map_err(|_| {
                UpdateError::InvalidVersion {
                    field: "rollback manifest version",
                    value: rollback.manifest.version.clone(),
                }
            })?;
            if older >= failed {
                return Err(UpdateError::RollbackVersionNotOlder {
                    failed: failed.to_string(),
                    rollback: older.to_string(),
                });
            }
        }
        preflight_package_manager(plan.kind)?;
        Ok(())
    }

    fn create_with(
        path: impl Into<PathBuf>,
        version: impl Into<String>,
        spec: JournalSpec,
    ) -> Result<Self, UpdateError> {
        let path = path.into();
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        let journal = ApplyJournal {
            data: JournalData {
                format: JOURNAL_FORMAT_VERSION,
                version: version.into(),
                kind: spec.kind,
                manifest: spec.manifest,
                verify_key: spec.verify_key,
                rollback: spec.rollback,
                artifact: spec.artifact,
                target: spec.target,
                backup: None,
                state: ApplyState::Prepared,
                message: None,
                relaunch: spec.relaunch,
                lock_file: path.with_extension("lock"),
            },
            path,
        };
        journal.save()?;
        Ok(journal)
    }

    /// Loads and validates a journal.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, UpdateError> {
        let path = path.into();
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut file = match trust::open_journal(&path) {
            Ok(file) => file,
            Err(UpdateError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(UpdateError::JournalMissing { path });
            }
            Err(error) => return Err(error),
        };
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let value: serde_json::Value = serde_json::from_str(&text)?;
        let format = value
            .get("format")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(u64::from(JOURNAL_FORMAT_VERSION));
        if format != u64::from(JOURNAL_FORMAT_VERSION) {
            return Err(UpdateError::JournalFormat(
                format.try_into().unwrap_or(u32::MAX),
            ));
        }
        let data: JournalData = serde_json::from_value(value)?;
        let expected_lock = path.with_extension("lock");
        if data.lock_file != expected_lock {
            return Err(UpdateError::JournalLockPath {
                path,
                lock: data.lock_file,
            });
        }
        trust::validate_lock_path(&data.lock_file)?;
        Ok(ApplyJournal { path, data })
    }

    /// The journal file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The journaled release version.
    pub fn version(&self) -> &str {
        &self.data.version
    }

    /// The platform action the helper performs, if any.
    pub fn kind(&self) -> ApplyKind {
        self.data.kind
    }

    /// The signed manifest retained for a helper-side apply.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.data.manifest.as_ref()
    }

    /// The pinned verification key retained for a helper-side apply.
    pub fn verify_key(&self) -> Option<&str> {
        self.data.verify_key.as_deref()
    }

    /// The older package retained for an automatic rollback, if the plan has one.
    pub fn rollback_artifact(&self) -> Option<&Path> {
        self.data
            .rollback
            .as_ref()
            .map(|rollback| rollback.artifact.as_path())
    }

    /// The verified artifact that was applied.
    pub fn artifact(&self) -> &Path {
        &self.data.artifact
    }

    /// The platform target, when the apply kind has one.
    pub fn target(&self) -> Option<&Path> {
        self.data.target.as_deref()
    }

    /// The retained rollback artifact, when one exists.
    pub fn backup(&self) -> Option<&Path> {
        self.data.backup.as_deref()
    }

    /// The current journal state.
    pub fn state(&self) -> ApplyState {
        self.data.state
    }

    /// The last diagnostic message, if any.
    pub fn message(&self) -> Option<&str> {
        self.data.message.as_deref()
    }

    /// The command the helper will launch.
    pub fn relaunch(&self) -> &Relaunch {
        &self.data.relaunch
    }

    /// The lock file the app holds until it exits.
    pub fn lock_file(&self) -> &Path {
        &self.data.lock_file
    }

    /// Records a retained rollback artifact.
    pub fn set_backup(&mut self, backup: impl Into<PathBuf>) -> Result<(), UpdateError> {
        self.data.backup = Some(backup.into());
        self.save()
    }

    /// Marks the platform apply as complete.
    pub fn mark_applied(&mut self) -> Result<(), UpdateError> {
        self.transition(ApplyState::Applied, None)
    }

    /// Marks the platform apply as failed with a diagnostic.
    pub fn mark_failed(&mut self, message: impl Into<String>) -> Result<(), UpdateError> {
        self.transition(ApplyState::Failed, Some(message.into()))
    }

    /// Marks the previous artifact as restored.
    pub fn mark_rolled_back(&mut self) -> Result<(), UpdateError> {
        self.transition(ApplyState::RolledBack, None)
    }

    /// Records a diagnostic without changing the current state.
    pub fn record_message(&mut self, message: impl Into<String>) -> Result<(), UpdateError> {
        if self.data.message.is_none() {
            self.data.message = Some(message.into());
            self.save()?;
        }
        Ok(())
    }

    /// Records that the installer requires a reboot before the new version can run.
    pub fn mark_reboot_required(&mut self) -> Result<(), UpdateError> {
        self.transition(ApplyState::RebootRequired, None)
    }

    /// Marks the helper handoff as complete.
    pub fn mark_restarted(&mut self) -> Result<(), UpdateError> {
        self.transition(ApplyState::Restarted, None)
    }

    /// Classifies what the next app start should do with this journal.
    ///
    /// `current_version` is the running app version (normally `env!("CARGO_PKG_VERSION")`). A
    /// journal whose version matches the running app is complete; a mismatched applied/restarted
    /// journal still needs a relaunch. Call [`Self::finalize`] only for `Complete`.
    pub fn reconcile(&self, current_version: &str) -> Result<Recovery, UpdateError> {
        let current =
            semver::Version::parse(current_version).map_err(|_| UpdateError::InvalidVersion {
                field: "current version",
                value: current_version.to_owned(),
            })?;
        let journal = semver::Version::parse(&self.data.version).map_err(|_| {
            UpdateError::InvalidVersion {
                field: "journal version",
                value: self.data.version.clone(),
            }
        })?;
        Ok(match self.data.state {
            ApplyState::Prepared if self.data.kind != ApplyKind::None => Recovery::RetryApply,
            ApplyState::Prepared => Recovery::Prepared,
            ApplyState::Applied | ApplyState::Restarted if current == journal => Recovery::Complete,
            ApplyState::Applied | ApplyState::Restarted => Recovery::Relaunch,
            ApplyState::RebootRequired => Recovery::AwaitReboot,
            ApplyState::Failed => Recovery::Failed,
            ApplyState::RolledBack => Recovery::RolledBack,
        })
    }

    /// Removes the retained backup, journal and lock file after a completed update.
    ///
    /// Call this only after [`Self::reconcile`] returned [`Recovery::Complete`]. A backup must be
    /// a normal file name directly under the target's canonical parent, so paths containing `..`,
    /// a drive prefix or an unexpected root are rejected, and the backup is removed through its
    /// canonical parent. Missing files are ignored.
    pub fn finalize(self) -> Result<(), UpdateError> {
        let backup = if let Some(backup) = self.data.backup.as_deref() {
            let Some(target) = self.data.target.as_deref() else {
                return Err(UpdateError::JournalBackupTargetMissing {
                    backup: backup.to_path_buf(),
                });
            };
            Some(finalize_backup_path(backup, target)?)
        } else {
            None
        };

        // Remove the journal first. If that fails, the backup is still the rollback copy the
        // journal names; an orphaned backup is preferable to a journal pointing at a deleted one.
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(backup) = backup {
            remove_path(&backup)?;
        }
        let _ = std::fs::remove_file(&self.data.lock_file);
        Ok(())
    }

    /// Removes the journal file. The lock file is intentionally left for the next attempt.
    pub fn remove(self) -> Result<(), UpdateError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Spawns the same executable in restart-helper mode and retains the exit lock.
    ///
    /// A `Prepared` journal must carry an [`ApplyKind`]; an `Applied` journal is relaunch-only.
    /// The helper waits until this process releases the lock file, applies a prepared plan, then
    /// relaunches [`Self::relaunch`]. The lock handle is kept in a process-global guard, so callers
    /// do not have to hold a return value.
    pub fn spawn_helper(&self) -> Result<(), UpdateError> {
        self.spawn_helper_with(&std::env::current_exe()?)
    }

    fn helper_ready(&self) -> bool {
        self.data.state == ApplyState::Applied
            || (self.data.state == ApplyState::Prepared && self.data.kind != ApplyKind::None)
    }

    fn spawn_helper_with(&self, executable: &Path) -> Result<(), UpdateError> {
        if !self.helper_ready() {
            return Err(UpdateError::JournalNotApplied {
                actual: self.data.state,
            });
        }

        let lock = open_lock(&self.data.lock_file)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(UpdateError::RestartLock {
                    path: self.data.lock_file.clone(),
                    message: "another process already holds the restart lock".to_owned(),
                });
            }
            Err(TryLockError::Error(error)) => {
                return Err(UpdateError::RestartLock {
                    path: self.data.lock_file.clone(),
                    message: error.to_string(),
                });
            }
        }

        let mut command = detached_command(executable);
        command
            .env(RESTART_HELPER_ENV, &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        retain_lock(lock);
        Ok(())
    }

    fn transition(
        &mut self,
        state: ApplyState,
        message: Option<String>,
    ) -> Result<(), UpdateError> {
        self.data.state = state;
        self.data.message = message;
        self.save()
    }

    fn save(&self) -> Result<(), UpdateError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            create_parent(parent)?;
        }
        trust::validate_parent(&self.path)?;
        let directory = parent.unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &self.data)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        drop(trust::open_journal(&self.path)?);
        Ok(())
    }
}

/// Runs the restart-helper half of the handshake.
///
/// This is called by `App::try_run` when [`RESTART_HELPER_ENV`] is set. It waits for the old app
/// to release the lock, applies a prepared plan (or accepts an already-applied one), and launches
/// the replacement process.
pub fn run_restart_helper(path: &Path) -> Result<(), UpdateError> {
    let result = run_restart_helper_inner(path);
    if let Err(error) = &result
        && let Ok(mut journal) = ApplyJournal::load(path)
    {
        let _ = journal.record_message(error.to_string());
    }
    result
}

fn run_restart_helper_inner(path: &Path) -> Result<(), UpdateError> {
    let journal = ApplyJournal::load(path)?;
    if !journal.helper_ready() {
        return Err(UpdateError::JournalNotApplied {
            actual: journal.data.state,
        });
    }
    let _lock = wait_for_lock(&journal.data.lock_file)?;
    let mut journal = ApplyJournal::load(path)?;
    match journal.data.state {
        ApplyState::Prepared => apply_prepared(&mut journal)?,
        ApplyState::Applied => {}
        actual => return Err(UpdateError::JournalNotApplied { actual }),
    }
    if journal.data.state == ApplyState::RebootRequired {
        return Ok(());
    }

    let relaunch = &journal.data.relaunch;
    if !relaunch.program.is_file() {
        return Err(UpdateError::RelaunchMissing {
            path: relaunch.program.clone(),
        });
    }
    let mut command = detached_command(&relaunch.program);
    command
        .args(&relaunch.args)
        .current_dir(&relaunch.cwd)
        .env_remove(RESTART_HELPER_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // A successful package rollback is the diagnostic the next start must see; overwriting it
    // with `Restarted` would hide that the update was reverted.
    if journal.data.state != ApplyState::RolledBack {
        journal.mark_restarted()?;
    }
    Ok(())
}

fn apply_prepared(journal: &mut ApplyJournal) -> Result<(), UpdateError> {
    let result = apply_prepared_inner(journal);
    if let Err(error) = &result {
        let _ = journal.mark_failed(error.to_string());
    }
    result
}

fn apply_prepared_inner(journal: &mut ApplyJournal) -> Result<(), UpdateError> {
    let kind = journal.data.kind;
    if kind == ApplyKind::None {
        return Err(UpdateError::ApplyKindMissing);
    }
    let manifest = journal
        .data
        .manifest
        .clone()
        .ok_or(UpdateError::ManifestMissing)?;
    let artifact = journal.data.artifact.clone();
    let key = PublicKey::from_base64(
        journal
            .data
            .verify_key
            .as_deref()
            .ok_or(UpdateError::VerificationKeyMissing)?,
    )?;
    manifest.verify_artifact(&artifact, &key)?;
    if manifest.version != journal.data.version {
        return Err(UpdateError::JournalVersionMismatch {
            journal: journal.data.version.clone(),
            manifest: manifest.version.clone(),
        });
    }
    let host = crate::current_platform();
    if manifest.platform != host {
        return Err(UpdateError::JournalPlatformMismatch {
            expected: host,
            actual: manifest.platform.clone(),
        });
    }
    let staged = StagedUpdate::from_parts(manifest, artifact);
    let outcome = match apply_kind(kind, journal, &staged) {
        Ok(outcome) => outcome,
        Err(error @ UpdateError::InstallerFailed { .. }) => {
            let Some(rollback) = journal.data.rollback.as_ref() else {
                return Err(error);
            };
            rollback_package(journal, kind, rollback).map_err(|rollback_error| {
                UpdateError::ApplyAndRollbackFailed {
                    apply: error.to_string(),
                    rollback: rollback_error.to_string(),
                }
            })?;
            journal.mark_rolled_back()?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if let Some(backup) = outcome.backup {
        journal.set_backup(backup)?;
    }
    if outcome.restart_required {
        journal.mark_reboot_required()?;
    } else {
        journal.mark_applied()?;
    }
    Ok(())
}

struct ApplyOutcome {
    backup: Option<PathBuf>,
    restart_required: bool,
}

#[allow(unused_variables)]
fn apply_kind(
    kind: ApplyKind,
    journal: &ApplyJournal,
    staged: &StagedUpdate,
) -> Result<ApplyOutcome, UpdateError> {
    match kind {
        ApplyKind::None => Err(UpdateError::ApplyKindMissing),
        ApplyKind::AppImage => {
            #[cfg(target_os = "linux")]
            {
                let target = journal
                    .data
                    .target
                    .as_deref()
                    .ok_or(UpdateError::ApplyTargetMissing { kind })?;
                let receipt = crate::AppImageInstaller::new(target).install(staged)?;
                Ok(ApplyOutcome {
                    backup: Some(receipt.backup().to_path_buf()),
                    restart_required: false,
                })
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(UpdateError::UnsupportedApply { kind })
            }
        }
        ApplyKind::WindowsInstaller => {
            #[cfg(target_os = "windows")]
            {
                let outcome = crate::WindowsInstaller::new().install(staged)?;
                Ok(ApplyOutcome {
                    backup: None,
                    restart_required: outcome.restart_required,
                })
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(UpdateError::UnsupportedApply { kind })
            }
        }
        ApplyKind::DebPackage | ApplyKind::RpmPackage => {
            #[cfg(target_os = "linux")]
            {
                crate::LinuxPackageInstaller::new()
                    .elevated()
                    .install(staged)?;
                Ok(ApplyOutcome {
                    backup: None,
                    restart_required: false,
                })
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(UpdateError::UnsupportedApply { kind })
            }
        }
        ApplyKind::MacPkg => {
            #[cfg(target_os = "macos")]
            {
                crate::MacPkgInstaller::new().elevated().install(staged)?;
                Ok(ApplyOutcome {
                    backup: None,
                    restart_required: false,
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(UpdateError::UnsupportedApply { kind })
            }
        }
        ApplyKind::MacApp => {
            #[cfg(target_os = "macos")]
            {
                let target = journal
                    .data
                    .target
                    .as_deref()
                    .ok_or(UpdateError::ApplyTargetMissing { kind })?;
                let receipt = crate::MacAppInstaller::new(target).install(staged)?;
                Ok(ApplyOutcome {
                    backup: Some(receipt.backup().to_path_buf()),
                    restart_required: false,
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(UpdateError::UnsupportedApply { kind })
            }
        }
    }
}

/// Re-verifies a caller-supplied older package and reinstalls it through the platform installer.
fn rollback_package(
    journal: &ApplyJournal,
    kind: ApplyKind,
    rollback: &RollbackData,
) -> Result<(), UpdateError> {
    if !matches!(kind, ApplyKind::DebPackage | ApplyKind::RpmPackage) {
        return Err(UpdateError::RollbackPlanUnsupported { kind });
    }
    let key = PublicKey::from_base64(
        rollback
            .verify_key
            .as_deref()
            .ok_or(UpdateError::VerificationKeyMissing)?,
    )?;
    rollback
        .manifest
        .verify_artifact(&rollback.artifact, &key)?;
    let host = crate::current_platform();
    if rollback.manifest.platform != host {
        return Err(UpdateError::PlatformMismatch {
            expected: host,
            actual: rollback.manifest.platform.clone(),
        });
    }
    let failed =
        semver::Version::parse(&journal.data.version).map_err(|_| UpdateError::InvalidVersion {
            field: "journal version",
            value: journal.data.version.clone(),
        })?;
    let older = semver::Version::parse(&rollback.manifest.version).map_err(|_| {
        UpdateError::InvalidVersion {
            field: "rollback manifest version",
            value: rollback.manifest.version.clone(),
        }
    })?;
    if older >= failed {
        return Err(UpdateError::RollbackVersionNotOlder {
            failed: failed.to_string(),
            rollback: older.to_string(),
        });
    }
    let staged = StagedUpdate::from_parts(rollback.manifest.clone(), rollback.artifact.clone());
    apply_package_rollback(kind, &staged)
}

#[allow(unused_variables)]
fn apply_package_rollback(kind: ApplyKind, staged: &StagedUpdate) -> Result<(), UpdateError> {
    #[cfg(target_os = "linux")]
    {
        use crate::LinuxPackageKind;

        let expected = match kind {
            ApplyKind::DebPackage => LinuxPackageKind::Deb,
            ApplyKind::RpmPackage => LinuxPackageKind::Rpm,
            _ => return Err(UpdateError::RollbackPlanUnsupported { kind }),
        };
        if LinuxPackageKind::from_path(staged.path()) != Some(expected) {
            return Err(UpdateError::UnsupportedInstaller {
                path: staged.path().to_path_buf(),
                expected: "a rollback package matching the apply kind",
            });
        }
        crate::LinuxPackageInstaller::new()
            .elevated()
            .rollback(staged)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(UpdateError::UnsupportedApply { kind })
    }
}

/// Resolves the backup to the canonical path [`ApplyJournal::finalize`] may delete.
///
/// The journal records the backup beside the target, but the target may be reached through a
/// symlinked parent or be a symlink itself (an AppImage launched through `~/.local/bin`, for
/// example), so the target file and the parents are resolved before comparing them. A lexical
/// comparison would accept `install/..` as a sibling, and `remove_path` would then delete the
/// whole install directory, so the lexical shape is rejected before canonicalization: the journal
/// only ever deletes a normal name under the real target's canonical parent.
fn finalize_backup_path(backup: &Path, target: &Path) -> Result<PathBuf, UpdateError> {
    let invalid = || UpdateError::JournalBackupPath {
        backup: backup.to_path_buf(),
        target: target.to_path_buf(),
    };
    let name = match backup.file_name() {
        Some(name) if name != OsStr::new(".") && name != OsStr::new("..") => name,
        _ => return Err(invalid()),
    };
    // A symlinked target is common for AppImages. The installer resolves it and stores the backup
    // beside the real file, so the recorded path must resolve the same way; a missing target keeps
    // the recorded path so the error still names what the journal holds.
    let resolved_target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let target_name = match resolved_target.file_name() {
        Some(name) if name != OsStr::new(".") && name != OsStr::new("..") => name,
        _ => return Err(invalid()),
    };
    // Equal names under the canonical parent below are the same entry, so this also catches
    // spellings such as `install/./App.AppImage` that `backup == target` would miss.
    if name == target_name || backup == target || !has_normal_components(backup) {
        return Err(invalid());
    }
    let backup_parent = canonical_parent(backup).ok_or_else(invalid)?;
    let target_parent = canonical_parent(&resolved_target).ok_or_else(invalid)?;
    if backup_parent != target_parent {
        return Err(invalid());
    }
    Ok(backup_parent.join(name))
}

/// Whether every component of `path` is a normal name, allowing one absolute prologue.
///
/// A `Prefix` and `RootDir` are accepted only where they make the path absolute: Windows may open
/// with `Prefix` + `RootDir` and Unix with `RootDir`. A prefix without a root is drive-relative,
/// so its meaning depends on the process CWD; it is rejected along with every `ParentDir` and
/// every root that is not the leading one. A `.` only names the directory itself, so it is
/// allowed; canonicalization resolves it away.
fn has_normal_components(path: &Path) -> bool {
    let mut components = path.components().peekable();
    if matches!(components.peek(), Some(Component::Prefix(_))) {
        components.next();
        if !matches!(components.peek(), Some(Component::RootDir)) {
            return false;
        }
    }
    if matches!(components.peek(), Some(Component::RootDir)) {
        components.next();
    }
    components.all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// Canonicalizes the parent of `path`, resolving symlinks and `..` the kernel would resolve.
fn canonical_parent(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    std::fs::canonicalize(parent).ok()
}

fn remove_path(path: &Path) -> Result<(), UpdateError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path)?,
        Ok(_) => std::fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn detached_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
    }
    command
}

fn create_parent(parent: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;

        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(parent)
    }
}

fn open_lock(path: &Path) -> Result<File, UpdateError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        create_parent(parent)?;
    }
    trust::open_lock(path, true)
}

fn wait_for_lock(path: &Path) -> Result<File, UpdateError> {
    let lock = open_lock(path)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(UpdateError::RestartTimeout);
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::Error(error)) => {
                return Err(UpdateError::RestartLock {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
        }
    }
}

fn retain_lock(lock: File) {
    HELPER_LOCKS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(lock);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn relaunch() -> Relaunch {
        // `/bin/true` is not on macOS; `/bin/sh -c 'exit 0'` is on both.
        Relaunch {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_owned(), "exit 0".to_owned()],
            cwd: std::env::current_dir().expect("cwd"),
        }
    }

    #[cfg(windows)]
    fn relaunch() -> Relaunch {
        // The helper checks `program.is_file()`, so a bare `cmd.exe` is not enough.
        Relaunch {
            program: std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe")),
            args: vec!["/C".to_owned(), "exit 0".to_owned()],
            cwd: std::env::current_dir().expect("cwd"),
        }
    }

    fn journal(temp: &tempfile::TempDir) -> ApplyJournal {
        ApplyJournal::create(
            temp.path().join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            Some(temp.path().join("target")),
            relaunch(),
        )
        .expect("journal")
    }

    #[test]
    fn round_trips_state_through_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        assert_eq!(journal.state(), ApplyState::Prepared);
        journal.mark_applied().expect("applied");
        journal
            .set_backup(temp.path().join("backup"))
            .expect("backup");
        journal.mark_rolled_back().expect("rolled back");

        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert_eq!(loaded.state(), ApplyState::RolledBack);
        assert_eq!(loaded.backup(), Some(temp.path().join("backup").as_path()));
        assert_eq!(loaded.version(), "1.2.3");
        assert_eq!(loaded.relaunch(), &relaunch());
    }

    #[test]
    fn rejects_a_missing_or_unknown_journal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing.json");
        assert!(matches!(
            ApplyJournal::load(&missing),
            Err(UpdateError::JournalMissing { .. })
        ));

        let mut journal = journal(&temp);
        journal.data.format = JOURNAL_FORMAT_VERSION + 1;
        journal.save().expect("save");
        assert!(matches!(
            ApplyJournal::load(journal.path()),
            Err(UpdateError::JournalFormat(_))
        ));
    }

    #[test]
    fn rejects_a_non_sibling_lock_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.data.lock_file = temp.path().join("elsewhere.lock");
        journal.save().expect("save");
        assert!(matches!(
            ApplyJournal::load(journal.path()),
            Err(UpdateError::JournalLockPath { .. })
        ));
    }

    #[test]
    fn reports_an_old_format_before_schema_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        let mut value = serde_json::to_value(&journal.data).expect("json");
        value["format"] = serde_json::json!(1);
        value.as_object_mut().expect("object").remove("kind");
        std::fs::write(journal.path(), serde_json::to_vec(&value).expect("json")).expect("write");
        assert!(matches!(
            ApplyJournal::load(journal.path()),
            Err(UpdateError::JournalFormat(1))
        ));
    }

    #[test]
    fn classifies_recovery_actions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        assert_eq!(
            journal.reconcile("1.2.3").expect("prepared"),
            Recovery::Prepared
        );
        journal.data.kind = ApplyKind::AppImage;
        assert_eq!(
            journal.reconcile("1.2.3").expect("retry"),
            Recovery::RetryApply
        );
        journal.data.kind = ApplyKind::None;
        journal.mark_applied().expect("applied");
        assert_eq!(
            journal.reconcile("1.2.3").expect("complete"),
            Recovery::Complete
        );
        assert_eq!(
            journal.reconcile("1.0.0").expect("relaunch"),
            Recovery::Relaunch
        );
        journal.mark_reboot_required().expect("reboot");
        assert_eq!(
            journal.reconcile("1.2.3").expect("reboot"),
            Recovery::AwaitReboot
        );
        journal.mark_rolled_back().expect("rollback");
        assert_eq!(
            journal.reconcile("1.2.3").expect("rollback"),
            Recovery::RolledBack
        );
        journal.mark_failed("broken").expect("failed");
        assert_eq!(
            journal.reconcile("1.2.3").expect("failed"),
            Recovery::Failed
        );
        assert!(matches!(
            journal.reconcile("not-a-version"),
            Err(UpdateError::InvalidVersion { .. })
        ));
    }

    #[test]
    fn finalizes_backup_journal_and_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        let target = temp.path().join("target");
        std::fs::write(&target, b"new").expect("target");
        let backup = temp.path().join("backup");
        std::fs::write(&backup, b"old").expect("backup");
        std::fs::write(journal.lock_file(), b"").expect("lock");
        journal.set_backup(&backup).expect("set backup");
        let journal_path = journal.path().to_path_buf();
        journal.finalize().expect("finalize");
        assert!(!backup.exists());
        assert!(!journal_path.exists());
        assert!(!temp.path().join("apply.lock").exists());
        assert!(target.exists());
    }

    #[test]
    fn finalizes_when_the_recorded_backup_is_gone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        let backup = temp.path().join("already-gone");
        journal.set_backup(&backup).expect("set backup");
        let journal_path = journal.path().to_path_buf();
        journal.finalize().expect("finalize");
        assert!(!journal_path.exists());
    }

    #[test]
    fn refuses_to_finalize_a_backup_without_a_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = ApplyJournal::create(
            temp.path().join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            None,
            relaunch(),
        )
        .expect("journal");
        let backup = temp.path().join("backup");
        std::fs::write(&backup, b"old").expect("backup");
        journal.set_backup(&backup).expect("set backup");
        assert!(matches!(
            journal.finalize(),
            Err(UpdateError::JournalBackupTargetMissing { .. })
        ));
        assert!(backup.exists());
    }

    #[test]
    fn refuses_to_finalize_an_unrelated_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        let elsewhere = tempfile::tempdir().expect("other");
        let backup = elsewhere.path().join("backup");
        std::fs::write(&backup, b"old").expect("backup");
        journal.set_backup(&backup).expect("set backup");
        assert!(matches!(
            journal.finalize(),
            Err(UpdateError::JournalBackupPath { .. })
        ));
        assert!(backup.exists());
    }

    #[test]
    fn refuses_to_finalize_through_a_parent_dir_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let install = temp.path().join("install");
        std::fs::create_dir(&install).expect("install dir");
        let target = install.join("App.AppImage");
        std::fs::write(&target, b"new").expect("target");
        let mut journal = ApplyJournal::create(
            temp.path().join("state").join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            Some(target.clone()),
            relaunch(),
        )
        .expect("journal");
        // Lexically a sibling of the target, but the kernel resolves it to the install directory.
        let backup = install.join("..");
        journal.set_backup(&backup).expect("set backup");
        let journal_path = journal.path().to_path_buf();
        assert!(matches!(
            journal.finalize(),
            Err(UpdateError::JournalBackupPath { .. })
        ));
        assert!(install.is_dir(), "the install directory must survive");
        assert!(target.exists(), "the target must survive");
        assert!(journal_path.exists(), "the journal must survive");
        assert!(temp.path().is_dir());
    }

    #[test]
    fn refuses_to_finalize_a_backup_that_is_the_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        std::fs::write(&target, b"new").expect("target");
        let mut journal = journal(&temp);
        journal.set_backup(&target).expect("set backup");
        let journal_path = journal.path().to_path_buf();
        assert!(matches!(
            journal.finalize(),
            Err(UpdateError::JournalBackupPath { .. })
        ));
        assert!(target.exists());
        assert!(journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn finalizes_a_backup_beside_the_canonical_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real dir");
        let canonical_target = real.join("App.AppImage");
        std::fs::write(&canonical_target, b"new").expect("target");
        let link = temp.path().join("link");
        symlink(&real, &link).expect("symlink");
        // The receipt stores the backup next to the canonical target while the journal keeps the
        // path the app was launched through.
        let backup = real.join("App.AppImage.bak");
        std::fs::write(&backup, b"old").expect("backup");

        let mut journal = ApplyJournal::create(
            temp.path().join("state").join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            Some(link.join("App.AppImage")),
            relaunch(),
        )
        .expect("journal");
        journal.set_backup(&backup).expect("set backup");
        journal.finalize().expect("finalize");
        assert!(!backup.exists());
        assert!(canonical_target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn finalizes_a_backup_beside_a_symlinked_target_file() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real dir");
        let real_target = real.join("App.AppImage");
        std::fs::write(&real_target, b"new").expect("target");
        // An AppImage launched through `~/.local/bin` is the common shape: the journal records the
        // link while the installer stores the backup beside the real file.
        let link = temp.path().join("App.AppImage");
        symlink(&real_target, &link).expect("symlink");
        let backup = real.join(".App.AppImage.123.bak");
        std::fs::write(&backup, b"old").expect("backup");

        let mut journal = ApplyJournal::create(
            temp.path().join("state").join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            Some(link.clone()),
            relaunch(),
        )
        .expect("journal");
        journal.set_backup(&backup).expect("set backup");
        let journal_path = journal.path().to_path_buf();
        journal
            .finalize()
            .expect("a symlinked target must finalize");
        assert!(!backup.exists(), "the backup must be removed");
        assert!(!journal_path.exists(), "the journal must be removed");
        assert!(real_target.exists());
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink(),
            "finalize must not replace the launch path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn finalize_keeps_the_backup_when_the_journal_cannot_be_removed() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir(&journal_dir).expect("journal dir");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"new").expect("target");
        let backup = temp.path().join("App.AppImage.bak");
        std::fs::write(&backup, b"old").expect("backup");

        let mut journal = ApplyJournal::create(
            journal_dir.join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            Some(target),
            relaunch(),
        )
        .expect("journal");
        journal.set_backup(&backup).expect("set backup");
        let journal_path = journal.path().to_path_buf();

        std::fs::set_permissions(&journal_dir, std::fs::Permissions::from_mode(0o500))
            .expect("read-only journal dir");
        let result = journal.finalize();
        std::fs::set_permissions(&journal_dir, std::fs::Permissions::from_mode(0o700))
            .expect("writable journal dir");

        assert!(result.is_err());
        assert!(backup.exists(), "the rollback copy must survive");
        assert!(journal_path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn create_for_apply_rejects_a_plan_the_helper_would_reject() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let manifest = |version: &str, platform: &str| {
            Manifest::for_artifact(
                &artifact,
                version,
                platform,
                "https://example.test/App.AppImage",
                "",
                "2026-11-18T00:00:00Z",
            )
            .expect("manifest")
            .sign(&key)
            .expect("sign")
        };

        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            None,
            manifest("1.2.3", &crate::current_platform()),
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::ApplyTargetMissing {
                kind: ApplyKind::AppImage
            })
        ));

        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            Some(target.clone()),
            manifest("9.9.9", &crate::current_platform()),
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::JournalVersionMismatch { .. })
        ));

        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            Some(target.clone()),
            manifest("1.2.3", "not-this-host"),
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::JournalPlatformMismatch { .. })
        ));

        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            temp.path().join("missing.AppImage"),
            Some(target),
            manifest("1.2.3", &crate::current_platform()),
            key.public_key(),
        );
        assert!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            )
            .is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preflight_rejects_a_directory_target_and_a_wrong_artifact_format() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");

        let directory = temp.path().join("AppDir");
        std::fs::create_dir(&directory).expect("target dir");
        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            Some(directory.clone()),
            manifest.clone(),
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::InstallTargetNotFile { .. })
        ));

        let rpm = temp.path().join("new.rpm");
        std::fs::write(&rpm, b"rpm").expect("rpm");
        let rpm_manifest = Manifest::for_artifact(
            &rpm,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/new.rpm",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let plan = ApplyPlan::new(
            ApplyKind::DebPackage,
            &rpm,
            None,
            rpm_manifest,
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn preflight_rejects_a_read_only_target_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let directory = temp.path().join("install");
        std::fs::create_dir(&directory).expect("install dir");
        let (kind, target, artifact) = if cfg!(target_os = "macos") {
            let target = directory.join("App.app");
            std::fs::create_dir(&target).expect("app bundle");
            (ApplyKind::MacApp, target, temp.path().join("new.zip"))
        } else {
            let target = directory.join("App.AppImage");
            std::fs::write(&target, b"old appimage").expect("target");
            (
                ApplyKind::AppImage,
                target,
                temp.path().join("new.AppImage"),
            )
        };
        std::fs::write(&artifact, b"new artifact").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/App",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");

        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555))
            .expect("read-only install dir");
        let result = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(kind, &artifact, Some(target), manifest, key.public_key()),
            relaunch(),
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
            .expect("restore install dir");

        assert!(matches!(
            result,
            Err(UpdateError::InstallTargetNotWritable { .. })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preflight_rejects_a_writable_symlink_to_a_read_only_target() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let readonly = temp.path().join("readonly");
        let writable = temp.path().join("writable");
        std::fs::create_dir(&readonly).expect("readonly dir");
        std::fs::create_dir(&writable).expect("writable dir");
        let real = readonly.join("Real.AppImage");
        std::fs::write(&real, b"old appimage").expect("real target");
        let target = writable.join("App.AppImage");
        symlink(&real, &target).expect("target symlink");
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");

        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555))
            .expect("read-only real dir");
        let result = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::AppImage,
                &artifact,
                Some(target),
                manifest,
                key.public_key(),
            ),
            relaunch(),
        );
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755))
            .expect("restore real dir");

        assert!(matches!(
            result,
            Err(UpdateError::InstallTargetNotWritable { .. })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preflight_accepts_an_appimage_without_an_extension() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("staged-appimage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/appimage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            Some(target),
            manifest,
            key.public_key(),
        );
        ApplyJournal::create_for_apply(temp.path().join("apply.json"), "1.2.3", plan, relaunch())
            .expect("an extensionless AppImage is valid");
    }

    #[test]
    fn missing_package_manager_reports_the_required_program() {
        let missing =
            missing_package_manager(ApplyKind::DebPackage, |_| false).expect("missing program");
        assert!(missing.ends_with("pkexec"), "{}", missing.display());

        let missing =
            missing_package_manager(ApplyKind::DebPackage, |path| path.ends_with("pkexec"))
                .expect("missing dpkg");
        assert!(missing.ends_with("dpkg"), "{}", missing.display());

        assert!(missing_package_manager(ApplyKind::RpmPackage, |_| true).is_none());
        assert!(missing_package_manager(ApplyKind::AppImage, |_| false).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preflight_rejects_an_apply_kind_this_host_cannot_execute() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("installer.msi");
        std::fs::write(&artifact, b"installer").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/installer.msi",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let plan = ApplyPlan::new(
            ApplyKind::WindowsInstaller,
            &artifact,
            None,
            manifest,
            key.public_key(),
        );
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::UnsupportedApply {
                kind: ApplyKind::WindowsInstaller
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preflight_rejects_a_rollback_package_that_does_not_match_the_kind() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.deb");
        std::fs::write(&artifact, b"new package").expect("artifact");
        let rollback = temp.path().join("old.rpm");
        std::fs::write(&rollback, b"old package").expect("rollback");
        let manifest = |path: &Path, version: &str| {
            Manifest::for_artifact(
                path,
                version,
                crate::current_platform(),
                "https://example.test/package",
                "",
                "2026-11-18T00:00:00Z",
            )
            .expect("manifest")
            .sign(&key)
            .expect("sign")
        };
        let plan = ApplyPlan::new(
            ApplyKind::DebPackage,
            &artifact,
            None,
            manifest(&artifact, "1.2.3"),
            key.public_key(),
        )
        .with_package_rollback(PackageRollback::new(
            &rollback,
            manifest(&rollback, "1.1.0"),
            key.public_key(),
        ));
        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }

    #[test]
    fn records_reboot_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_reboot_required().expect("reboot");
        assert_eq!(
            ApplyJournal::load(journal.path()).expect("load").state(),
            ApplyState::RebootRequired
        );
    }

    #[test]
    fn rejects_malformed_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("apply.json");
        std::fs::write(&path, b"not json").expect("write");
        assert!(matches!(
            ApplyJournal::load(path),
            Err(UpdateError::Json(_))
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn uses_the_appimage_path_for_relaunch() {
        let appimage = PathBuf::from("/tmp/Demo.AppImage");
        assert_eq!(
            relaunch_program(Some(appimage.clone().into_os_string())).expect("program"),
            appimage
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawns_a_helper_with_the_journal_path() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        let marker = temp.path().join("helper-env");
        let script = temp.path().join("helper.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$AURO_UPDATE_RESTART_HELPER\" > \"{}\"\n",
                marker.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");

        journal.spawn_helper_with(&script).expect("spawn");
        for _ in 0..20 {
            if marker.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            std::fs::read_to_string(&marker).expect("helper env"),
            journal.path().display().to_string()
        );
    }

    #[test]
    fn refuses_to_spawn_when_the_lock_is_held() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        let lock = open_lock(journal.lock_file()).expect("lock");
        lock.try_lock().expect("parent lock");
        assert!(matches!(
            journal.spawn_helper(),
            Err(UpdateError::RestartLock { .. })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn round_trips_a_package_rollback_plan() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let new_artifact = temp.path().join("new.deb");
        std::fs::write(&new_artifact, b"new package").expect("new");
        let old_artifact = temp.path().join("old.deb");
        std::fs::write(&old_artifact, b"old package").expect("old");
        let new_manifest = Manifest::for_artifact(
            &new_artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/new.deb",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let old_manifest = Manifest::for_artifact(
            &old_artifact,
            "1.1.0",
            crate::current_platform(),
            "https://example.test/old.deb",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        // Bypass the host preflight: this test covers journal serialization, not whether this
        // machine has dpkg/rpm installed.
        let journal = ApplyJournal::create_with(
            temp.path().join("apply.json"),
            "1.2.3",
            JournalSpec {
                artifact: new_artifact.clone(),
                target: None,
                kind: ApplyKind::DebPackage,
                manifest: Some(new_manifest),
                verify_key: Some(key.public_key().to_base64()),
                rollback: Some(RollbackData {
                    artifact: old_artifact.clone(),
                    manifest: old_manifest,
                    verify_key: Some(key.public_key().to_base64()),
                }),
                relaunch: relaunch(),
            },
        )
        .expect("journal");

        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert_eq!(loaded.kind(), ApplyKind::DebPackage);
        assert_eq!(loaded.rollback_artifact(), Some(old_artifact.as_path()));
    }

    #[test]
    fn rejects_a_rollback_plan_for_a_non_package_apply() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"appimage").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            crate::current_platform(),
            "https://example.test/new.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let plan = ApplyPlan::new(
            ApplyKind::AppImage,
            &artifact,
            Some(temp.path().join("App.AppImage")),
            manifest.clone(),
            key.public_key(),
        )
        .with_package_rollback(PackageRollback::new(&artifact, manifest, key.public_key()));

        assert!(matches!(
            ApplyJournal::create_for_apply(
                temp.path().join("apply.json"),
                "1.2.3",
                plan,
                relaunch(),
            ),
            Err(UpdateError::RollbackPlanUnsupported {
                kind: ApplyKind::AppImage
            })
        ));
    }

    #[test]
    fn rollback_verification_requires_an_older_verified_package() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let old_artifact = temp.path().join("old.deb");
        std::fs::write(&old_artifact, b"old package").expect("old");
        let journal = journal(&temp);
        let host = crate::current_platform();

        let manifest = |version: &str, platform: &str| {
            Manifest::for_artifact(
                &old_artifact,
                version,
                platform,
                "https://example.test/old.deb",
                "",
                "2026-11-18T00:00:00Z",
            )
            .expect("manifest")
            .sign(&key)
            .expect("sign")
        };
        let rollback = |manifest: Manifest| RollbackData {
            artifact: old_artifact.clone(),
            manifest,
            verify_key: Some(key.public_key().to_base64()),
        };

        assert!(matches!(
            rollback_package(
                &journal,
                ApplyKind::DebPackage,
                &rollback(manifest("1.2.3", &host)),
            ),
            Err(UpdateError::RollbackVersionNotOlder { .. })
        ));
        let other_platform = if host == "linux-x86_64" {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        };
        assert!(matches!(
            rollback_package(
                &journal,
                ApplyKind::DebPackage,
                &rollback(manifest("1.1.0", other_platform)),
            ),
            Err(UpdateError::PlatformMismatch { .. })
        ));

        let older = manifest("1.1.0", &host);
        std::fs::write(&old_artifact, b"bad package").expect("tamper");
        assert!(matches!(
            rollback_package(&journal, ApplyKind::DebPackage, &rollback(older)),
            Err(UpdateError::ArtifactHash)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rollback_package_format_must_match_the_plan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = temp.path().join("old.rpm");
        std::fs::write(&artifact, b"rpm package").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.1.0",
            crate::current_platform(),
            "https://example.test/old.rpm",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest");
        let staged = StagedUpdate::from_parts(manifest, artifact);
        assert!(matches!(
            apply_package_rollback(ApplyKind::DebPackage, &staged),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }

    #[test]
    fn removing_a_journal_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        let path = journal.path().to_path_buf();
        journal.remove().expect("remove");
        assert!(!path.exists());

        let journal = self::journal(&temp);
        journal.remove().expect("remove again");
    }

    #[test]
    fn refuses_to_spawn_before_apply() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        assert!(matches!(
            journal.spawn_helper(),
            Err(UpdateError::JournalNotApplied {
                actual: ApplyState::Prepared
            })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_applies_an_appimage_before_relaunch() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let journal = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::AppImage,
                &artifact,
                Some(target.clone()),
                manifest,
                key.public_key(),
            ),
            relaunch(),
        )
        .expect("journal");

        run_restart_helper(journal.path()).expect("helper");
        assert_eq!(std::fs::read(&target).expect("target"), b"new appimage");
        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert_eq!(loaded.state(), ApplyState::Restarted);
        assert!(loaded.backup().is_some_and(|backup| backup.is_file()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_rejects_a_tampered_staged_artifact() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let journal = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::AppImage,
                &artifact,
                Some(target.clone()),
                manifest,
                key.public_key(),
            ),
            relaunch(),
        )
        .expect("journal");

        std::fs::write(&artifact, b"tampered appimage").expect("tamper");
        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::ArtifactSize { .. } | UpdateError::ArtifactHash)
        ));
        assert_eq!(std::fs::read(&target).expect("target"), b"old appimage");
        assert_eq!(
            ApplyJournal::load(journal.path()).expect("load").state(),
            ApplyState::Failed
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_rejects_a_manifest_version_mismatch() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let mut journal = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::AppImage,
                &artifact,
                Some(target.clone()),
                manifest,
                key.public_key(),
            ),
            relaunch(),
        )
        .expect("journal");
        // Simulate a journal written against a different version; the preflight now rejects this
        // while the app is alive, so rewrite the journal to keep the helper's check covered.
        journal.data.version = "9.9.9".to_owned();
        journal.save().expect("save");

        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::JournalVersionMismatch { .. })
        ));
        assert_eq!(std::fs::read(&target).expect("target"), b"old appimage");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_rejects_a_manifest_platform_mismatch() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("new.AppImage");
        std::fs::write(&artifact, b"new appimage").expect("artifact");
        let target = temp.path().join("App.AppImage");
        std::fs::write(&target, b"old appimage").expect("target");
        let valid = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let other_platform = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "windows-x86_64",
            "https://example.test/App.AppImage",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let mut journal = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::AppImage,
                &artifact,
                Some(target.clone()),
                valid,
                key.public_key(),
            ),
            relaunch(),
        )
        .expect("journal");
        // The preflight rejects this while the app is alive; swap it in afterwards to keep the
        // helper's platform check covered.
        journal.data.manifest = Some(other_platform);
        journal.save().expect("save");

        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::JournalPlatformMismatch { .. })
        ));
        assert_eq!(std::fs::read(&target).expect("target"), b"old appimage");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn helper_marks_an_unsupported_apply_failed() {
        use auro_sign::SigningKey;

        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let artifact = temp.path().join("installer.msi");
        std::fs::write(&artifact, b"installer").expect("artifact");
        let manifest = Manifest::for_artifact(
            &artifact,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/installer.msi",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let journal = ApplyJournal::create_with(
            temp.path().join("apply.json"),
            "1.2.3",
            JournalSpec {
                artifact: artifact.clone(),
                target: None,
                kind: ApplyKind::WindowsInstaller,
                manifest: Some(manifest),
                verify_key: Some(key.public_key().to_base64()),
                rollback: None,
                relaunch: relaunch(),
            },
        )
        .expect("journal");

        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::UnsupportedApply {
                kind: ApplyKind::WindowsInstaller
            })
        ));
        assert_eq!(
            ApplyJournal::load(journal.path()).expect("load").state(),
            ApplyState::Failed
        );
    }

    #[test]
    fn helper_relaunches_and_marks_the_journal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        run_restart_helper(journal.path()).expect("helper");
        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert_eq!(loaded.state(), ApplyState::Restarted);
    }

    #[test]
    fn helper_waits_for_the_app_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        let lock = open_lock(journal.lock_file()).expect("lock");
        lock.try_lock().expect("parent lock");

        let path = journal.path().to_path_buf();
        let helper = std::thread::spawn(move || run_restart_helper(&path));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!helper.is_finished(), "helper must wait for the app lock");
        drop(lock);
        helper.join().expect("join").expect("helper");
        assert_eq!(
            ApplyJournal::load(journal.path()).expect("load").state(),
            ApplyState::Restarted
        );
    }

    #[test]
    fn helper_records_a_launch_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.data.relaunch.program = temp.path().join("missing-binary");
        journal.mark_applied().expect("applied");
        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::RelaunchMissing { .. })
        ));
        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert!(
            loaded
                .message()
                .is_some_and(|message| message.contains("missing-binary"))
        );
    }

    #[test]
    fn helper_refuses_a_state_flipped_while_waiting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        let lock = open_lock(journal.lock_file()).expect("lock");
        lock.try_lock().expect("parent lock");

        let path = journal.path().to_path_buf();
        let helper = std::thread::spawn(move || run_restart_helper(&path));
        std::thread::sleep(Duration::from_millis(100));
        journal.mark_failed("cancelled").expect("failed");
        drop(lock);
        assert!(matches!(
            helper.join().expect("join"),
            Err(UpdateError::JournalNotApplied {
                actual: ApplyState::Failed
            })
        ));
        assert_eq!(
            ApplyJournal::load(journal.path()).expect("load").message(),
            Some("cancelled")
        );
    }

    #[test]
    fn helper_rejects_an_unapplied_journal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        assert!(matches!(
            run_restart_helper(journal.path()),
            Err(UpdateError::JournalNotApplied {
                actual: ApplyState::Prepared
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn accepts_a_private_journal_directory_and_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        ApplyJournal::load(journal.path()).expect("private path accepted");
    }

    #[cfg(unix)]
    #[test]
    fn accepts_a_journal_in_a_private_symlinked_directory() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700))
            .expect("permissions");
        let link = temp.path().join("link");
        symlink(&real, &link).expect("symlink");
        let journal = ApplyJournal::create(
            link.join("apply.json"),
            "1.2.3",
            link.join("artifact"),
            None,
            relaunch(),
        )
        .expect("journal");
        let loaded = ApplyJournal::load(journal.path()).expect("load");
        assert_eq!(loaded.version(), "1.2.3");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_world_writable_journal_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o777))
            .expect("permissions");
        let result = ApplyJournal::load(journal.path());
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("restore permissions");
        assert!(matches!(result, Err(UpdateError::JournalTrust { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_world_writable_journal_file() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        std::fs::set_permissions(journal.path(), std::fs::Permissions::from_mode(0o666))
            .expect("permissions");
        let result = ApplyJournal::load(journal.path());
        std::fs::set_permissions(journal.path(), std::fs::Permissions::from_mode(0o600))
            .expect("restore permissions");
        assert!(matches!(result, Err(UpdateError::JournalTrust { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_journal() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real");
        std::fs::rename(journal.path(), real.join("apply.json")).expect("move");
        symlink(real.join("apply.json"), journal.path()).expect("symlink");
        assert!(matches!(
            ApplyJournal::load(journal.path()),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_shared_ancestor_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let journal = journal(&temp);
        let shared = temp.path().join("shared");
        std::fs::create_dir(&shared).expect("shared");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777))
            .expect("permissions");
        let inner = shared.join("private");
        std::fs::create_dir(&inner).expect("private");
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700))
            .expect("permissions");
        std::fs::rename(journal.path(), inner.join("apply.json")).expect("move");
        let result = ApplyJournal::load(inner.join("apply.json"));
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o700))
            .expect("restore permissions");
        assert!(matches!(result, Err(UpdateError::JournalTrust { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_lock_file() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut journal = journal(&temp);
        journal.mark_applied().expect("applied");
        let target = temp.path().join("lock-target");
        std::fs::write(&target, b"").expect("target");
        symlink(&target, journal.lock_file()).expect("symlink");
        assert!(matches!(
            journal.spawn_helper(),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_junction_journal_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        let link = temp.path().join("junction");
        std::fs::create_dir(&real).expect("real");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .status();
        let Ok(status) = status else {
            eprintln!("skipping junction test: cmd unavailable");
            return;
        };
        if !status.success() {
            eprintln!("skipping junction test: junction creation failed");
            return;
        }
        assert!(matches!(
            trust::validate_parent(&link.join("apply.json")),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_accepts_a_private_journal_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let private = temp.path().join("private");
        std::fs::create_dir(&private).expect("private");
        let result = trust::validate_parent(&private.join("apply.json"));
        let acl = std::process::Command::new("icacls")
            .arg("C:\\")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_else(|| "icacls unavailable".to_owned());
        assert!(
            result.is_ok(),
            "private journal should be accepted: {result:?}\nC:\\ ACL:\n{acl}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_permissive_journal_dacl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let status = std::process::Command::new("icacls")
            .arg(temp.path())
            .args(["/grant", "*S-1-1-0:(OI)(CI)M"])
            .status();
        let Ok(status) = status else {
            eprintln!("skipping DACL test: icacls unavailable");
            return;
        };
        if !status.success() {
            eprintln!("skipping DACL test: icacls failed");
            return;
        }
        assert!(matches!(
            trust::validate_parent(&temp.path().join("apply.json")),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_permissive_ancestor_dacl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shared = temp.path().join("shared");
        let private = shared.join("private");
        std::fs::create_dir(&shared).expect("shared");
        std::fs::create_dir(&private).expect("private");
        let status = std::process::Command::new("icacls")
            .arg(&shared)
            .args(["/grant", "*S-1-1-0:(OI)(CI)M"])
            .status();
        let Ok(status) = status else {
            eprintln!("skipping ancestor DACL test: icacls unavailable");
            return;
        };
        if !status.success() {
            eprintln!("skipping ancestor DACL test: icacls failed");
            return;
        }
        assert!(matches!(
            trust::validate_parent(&private.join("apply.json")),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_symlinked_journal_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real.json");
        let link = temp.path().join("apply.json");
        std::fs::write(&real, b"{}").expect("real");
        if std::os::windows::fs::symlink_file(&real, &link).is_err() {
            eprintln!("skipping symlink test: symlink privilege unavailable");
            return;
        }
        assert!(matches!(
            trust::open_journal(&link),
            Err(UpdateError::JournalTrust { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_create_a_journal_in_a_shared_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o777))
            .expect("permissions");
        let result = ApplyJournal::create(
            temp.path().join("apply.json"),
            "1.2.3",
            temp.path().join("artifact"),
            None,
            relaunch(),
        );
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("restore permissions");
        assert!(matches!(result, Err(UpdateError::JournalTrust { .. })));
    }
}
