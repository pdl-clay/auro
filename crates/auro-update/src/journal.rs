// SPDX-License-Identifier: MIT OR Apache-2.0

//! Durable apply journal and the restart helper handshake.
//!
//! The app writes a journal, then spawns the same executable in helper mode. A prepared journal
//! carries the platform action; the helper waits for the app to release a lock file, applies the
//! staged artifact through the platform installers, and relaunches the new executable. An
//! already-applied journal is relaunch-only. The journal is written atomically and remains on disk
//! for diagnosis after the restart.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{JOURNAL_FORMAT_VERSION, Manifest, PublicKey, StagedUpdate, UpdateError};

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
        }
    }
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
    relaunch: Relaunch,
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
        Self::create_with(
            path,
            version,
            JournalSpec {
                artifact: plan.artifact,
                target: plan.target,
                kind: plan.kind,
                manifest: Some(plan.manifest),
                verify_key: Some(plan.public_key.to_base64()),
                relaunch,
            },
        )
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
        let text = std::fs::read_to_string(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                UpdateError::JournalMissing { path: path.clone() }
            } else {
                UpdateError::Io(error)
            }
        })?;
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
            std::fs::create_dir_all(parent)?;
        }
        let directory = parent.unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &self.data)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
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
    journal.mark_restarted()?;
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
    let outcome = apply_kind(kind, journal, &staged)?;
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
                crate::LinuxPackageInstaller::new().install(staged)?;
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

fn open_lock(path: &Path) -> Result<File, UpdateError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
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
        Relaunch {
            program: PathBuf::from("/bin/true"),
            args: Vec::new(),
            cwd: std::env::current_dir().expect("cwd"),
        }
    }

    #[cfg(windows)]
    fn relaunch() -> Relaunch {
        Relaunch {
            program: PathBuf::from("cmd.exe"),
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
            "9.9.9",
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
        let manifest = Manifest::for_artifact(
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
        let journal = ApplyJournal::create_for_apply(
            temp.path().join("apply.json"),
            "1.2.3",
            ApplyPlan::new(
                ApplyKind::WindowsInstaller,
                &artifact,
                None,
                manifest,
                key.public_key(),
            ),
            relaunch(),
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
}
