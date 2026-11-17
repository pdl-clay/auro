// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux AppImage installation and rollback.
//!
//! An AppImage is one executable file, so an update can replace it with a rename while the old
//! process keeps running from the old inode. Installation keeps a backup beside the target until
//! the app calls [`InstallReceipt::discard`] or [`InstallReceipt::rollback`]; dropping the receipt
//! deliberately leaves the backup in place.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::NamedTempFile;

use crate::{StagedUpdate, UpdateError};

/// The mode an installed AppImage needs to stay executable.
const APPIMAGE_MODE: u32 = 0o755;

/// Installs a staged artifact over an AppImage path on Linux.
#[derive(Clone, Debug)]
pub struct AppImageInstaller {
    target: PathBuf,
}

impl AppImageInstaller {
    /// Creates an installer for the given AppImage path. The path is resolved on install.
    pub fn new(target: impl Into<PathBuf>) -> Self {
        AppImageInstaller {
            target: target.into(),
        }
    }

    /// The path this installer will replace.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Copies a verified staged artifact over the target and returns a rollback receipt.
    ///
    /// The target is resolved through symlinks. A backup is created first: a hard link when the
    /// filesystem supports it, otherwise a copy. The new bytes are written to a temporary file in
    /// the target directory, marked executable, synced, and atomically renamed over the target.
    /// Replacing a running AppImage is safe on Linux; the running process keeps the old inode.
    pub fn install(&self, staged: &StagedUpdate) -> Result<InstallReceipt, UpdateError> {
        let target = resolve_target(&self.target)?;
        let metadata = fs::symlink_metadata(&target)?;
        if !metadata.file_type().is_file() {
            return Err(UpdateError::InstallTargetNotFile { path: target });
        }
        let staged_path = staged.path();
        if !staged_path.is_file() {
            return Err(UpdateError::StagedArtifactMissing {
                path: staged_path.to_path_buf(),
            });
        }

        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "AppImage path has no parent directory",
            )
        })?;
        let backup = create_backup(&target)?;

        let result = replace_target(&target, parent, staged_path);
        if let Err(error) = result {
            let _ = fs::remove_file(&backup);
            return Err(error);
        }

        Ok(InstallReceipt { target, backup })
    }
}

/// The old AppImage retained after an install. Keep it until the new version is known to work.
#[derive(Debug)]
pub struct InstallReceipt {
    target: PathBuf,
    backup: PathBuf,
}

impl InstallReceipt {
    /// The installed AppImage path.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The backup that [`Self::rollback`] restores.
    pub fn backup(&self) -> &Path {
        &self.backup
    }

    /// Restores the backup over the installed AppImage.
    ///
    /// This replaces the target atomically on Linux. A running process keeps its current inode;
    /// the restored file is used on the next launch.
    pub fn rollback(self) -> Result<(), UpdateError> {
        if !self.backup.is_file() {
            return Err(UpdateError::RollbackBackupMissing { path: self.backup });
        }
        fs::rename(&self.backup, &self.target)?;
        Ok(())
    }

    /// Removes the backup after the new version has been verified.
    pub fn discard(self) -> Result<(), UpdateError> {
        match fs::remove_file(&self.backup) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn resolve_target(path: &Path) -> Result<PathBuf, UpdateError> {
    fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            UpdateError::InstallTargetMissing {
                path: path.to_path_buf(),
            }
        } else {
            error.into()
        }
    })
}

fn create_backup(target: &Path) -> Result<PathBuf, UpdateError> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AppImage path has no parent directory",
        )
    })?;
    let metadata = fs::metadata(target)?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "app".into());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    for attempt in 0..16 {
        let candidate = parent.join(format!(
            ".{name}.{}.{stamp}.{attempt}.bak",
            std::process::id()
        ));

        match fs::hard_link(target, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }

        match copy_backup(target, &candidate, &metadata) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot create a unique AppImage backup name",
    )
    .into())
}

fn copy_backup(target: &Path, backup: &Path, metadata: &fs::Metadata) -> std::io::Result<()> {
    let mut source = fs::File::open(target)?;
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)?;
    let result = (|| {
        std::io::copy(&mut source, &mut destination)?;
        destination.set_permissions(metadata.permissions())?;
        destination.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(backup);
    }
    result
}

fn replace_target(target: &Path, parent: &Path, staged: &Path) -> Result<(), UpdateError> {
    let mut temp = NamedTempFile::new_in(parent)?;
    let mut source = fs::File::open(staged)?;
    std::io::copy(&mut source, temp.as_file_mut())?;
    temp.as_file_mut()
        .set_permissions(fs::Permissions::from_mode(APPIMAGE_MODE))?;
    temp.as_file_mut().sync_all()?;
    temp.persist(target).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use auro_sign::SigningKey;

    use super::*;
    use crate::{Manifest, Updater};

    fn staged_update(temp: &tempfile::TempDir, key: &SigningKey, bytes: &[u8]) -> StagedUpdate {
        let source = temp.path().join("new.AppImage");
        fs::write(&source, bytes).expect("source");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/app.AppImage",
            "",
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest")
        .sign(key)
        .expect("sign");
        let updater = Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("linux-x86_64");
        updater.stage_from_path(&manifest, &source).expect("stage")
    }

    fn target(temp: &tempfile::TempDir) -> PathBuf {
        let path = temp.path().join("App.AppImage");
        fs::write(&path, b"old appimage").expect("old target");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
        path
    }

    #[test]
    fn installs_and_rolls_back_an_appimage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let staged = staged_update(&temp, &key, b"new appimage");
        let target = target(&temp);

        let receipt = AppImageInstaller::new(&target)
            .install(&staged)
            .expect("install");
        assert_eq!(fs::read(&target).expect("read"), b"new appimage");
        assert_eq!(
            fs::metadata(&target)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(fs::read(receipt.backup()).expect("backup"), b"old appimage");

        receipt.rollback().expect("rollback");
        assert_eq!(fs::read(&target).expect("read"), b"old appimage");
        assert_eq!(
            fs::metadata(&target)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn discards_the_backup_after_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let staged = staged_update(&temp, &key, b"new appimage");
        let target = target(&temp);

        let receipt = AppImageInstaller::new(&target)
            .install(&staged)
            .expect("install");
        let backup = receipt.backup().to_path_buf();
        receipt.discard().expect("discard");
        assert!(!backup.exists());
        assert_eq!(fs::read(&target).expect("read"), b"new appimage");
    }

    #[test]
    fn rejects_a_missing_or_non_file_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let staged = staged_update(&temp, &key, b"new appimage");

        let missing = temp.path().join("missing.AppImage");
        assert!(matches!(
            AppImageInstaller::new(&missing).install(&staged),
            Err(UpdateError::InstallTargetMissing { .. })
        ));

        let directory = temp.path().join("dir.AppImage");
        fs::create_dir(&directory).expect("directory");
        assert!(matches!(
            AppImageInstaller::new(&directory).install(&staged),
            Err(UpdateError::InstallTargetNotFile { .. })
        ));
    }

    #[test]
    fn missing_staged_artifact_leaves_the_target_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let staged = staged_update(&temp, &key, b"new appimage");
        let target = target(&temp);
        fs::remove_file(staged.path()).expect("remove staged");

        assert!(matches!(
            AppImageInstaller::new(&target).install(&staged),
            Err(UpdateError::StagedArtifactMissing { .. })
        ));
        assert_eq!(fs::read(&target).expect("read"), b"old appimage");
    }

    #[test]
    fn rollback_reports_a_missing_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let staged = staged_update(&temp, &key, b"new appimage");
        let target = target(&temp);

        let receipt = AppImageInstaller::new(&target)
            .install(&staged)
            .expect("install");
        fs::remove_file(receipt.backup()).expect("remove backup");
        assert!(matches!(
            receipt.rollback(),
            Err(UpdateError::RollbackBackupMissing { .. })
        ));
        assert_eq!(fs::read(&target).expect("read"), b"new appimage");
    }
}
