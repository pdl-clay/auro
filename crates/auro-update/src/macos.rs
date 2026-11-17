// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS `.app` bundle replacement from a staged `.zip`.
//!
//! The app bundle is swapped as a directory: the old bundle is renamed aside, the extracted
//! bundle is renamed into place, and the old bundle is retained until the caller discards it or
//! rolls back. `.dmg` mounting and restart orchestration are separate concerns.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zip::ZipArchive;

use crate::{StagedUpdate, UpdateError};

/// Replaces a macOS `.app` directory with a staged `.zip` archive.
#[derive(Clone, Debug)]
pub struct MacAppInstaller {
    target: PathBuf,
}

impl MacAppInstaller {
    /// Creates an installer for the given `.app` path. The path is resolved on install.
    pub fn new(target: impl Into<PathBuf>) -> Self {
        MacAppInstaller {
            target: target.into(),
        }
    }

    /// The path this installer will replace.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Extracts a staged `.zip` containing exactly one top-level `.app`, then swaps it into place.
    ///
    /// The old bundle is retained as [`MacAppReceipt::backup`]. Dropping the receipt leaves the
    /// backup in place, matching the conservative AppImage receipt.
    pub fn install(&self, staged: &StagedUpdate) -> Result<MacAppReceipt, UpdateError> {
        let target = resolve_target(&self.target)?;
        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "app bundle path has no parent directory",
            )
        })?;
        let staged_path = staged.path();
        if !staged_path.is_file() {
            return Err(UpdateError::StagedArtifactMissing {
                path: staged_path.to_path_buf(),
            });
        }
        if !staged_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            return Err(UpdateError::UnsupportedInstaller {
                path: staged_path.to_path_buf(),
                expected: ".zip containing a .app bundle",
            });
        }

        let extraction = tempfile::tempdir_in(parent)?;
        extract_archive(staged_path, extraction.path())?;
        validate_extracted_tree(extraction.path())?;
        let bundle = find_bundle(extraction.path(), staged_path)?;
        let backup = unique_backup(&target)?;

        fs::rename(&target, &backup)?;
        if let Err(error) = fs::rename(&bundle, &target) {
            if let Err(restore) = fs::rename(&backup, &target) {
                return Err(UpdateError::RestoreFailed {
                    target,
                    backup,
                    message: restore.to_string(),
                });
            }
            return Err(error.into());
        }

        Ok(MacAppReceipt { target, backup })
    }
}

/// The old `.app` retained after an install.
#[derive(Debug)]
pub struct MacAppReceipt {
    target: PathBuf,
    backup: PathBuf,
}

impl MacAppReceipt {
    /// The installed `.app` path.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The backup that [`Self::rollback`] restores.
    pub fn backup(&self) -> &Path {
        &self.backup
    }

    /// Removes the installed bundle and restores the backup.
    pub fn rollback(self) -> Result<(), UpdateError> {
        if !self.backup.is_dir() {
            return Err(UpdateError::RollbackBackupMissing { path: self.backup });
        }
        match fs::symlink_metadata(&self.target) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&self.target)?,
            Ok(_) => fs::remove_file(&self.target)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::rename(&self.backup, &self.target).map_err(|error| UpdateError::RollbackFailed {
            target: self.target.clone(),
            backup: self.backup.clone(),
            message: error.to_string(),
        })?;
        Ok(())
    }

    /// Removes the backup after the new bundle has been verified.
    pub fn discard(self) -> Result<(), UpdateError> {
        match fs::remove_dir_all(&self.backup) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn resolve_target(path: &Path) -> Result<PathBuf, UpdateError> {
    let target = fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            UpdateError::InstallTargetMissing {
                path: path.to_path_buf(),
            }
        } else {
            error.into()
        }
    })?;
    if !fs::metadata(&target)?.is_dir() {
        return Err(UpdateError::InstallTargetNotDirectory { path: target });
    }
    Ok(target)
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), UpdateError> {
    let file = fs::File::open(archive)?;
    let mut archive = ZipArchive::new(file)?;
    archive.extract(destination)?;
    Ok(())
}

fn validate_extracted_tree(root: &Path) -> Result<(), UpdateError> {
    let root = fs::canonicalize(root)?;
    validate_entries(&root, &root)
}

fn validate_entries(root: &Path, directory: &Path) -> Result<(), UpdateError> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            let resolved = fs::canonicalize(&path)
                .map_err(|_| UpdateError::UnsafeArchive { path: path.clone() })?;
            if !resolved.starts_with(root) {
                return Err(UpdateError::UnsafeArchive { path });
            }
        } else if metadata.file_type().is_dir() {
            validate_entries(root, &path)?;
        }
    }
    Ok(())
}

fn find_bundle(directory: &Path, archive: &Path) -> Result<PathBuf, UpdateError> {
    let mut bundles = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_dir())
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        {
            bundles.push(path);
        }
    }
    match bundles.len() {
        0 => Err(UpdateError::AppBundleMissing {
            path: archive.to_path_buf(),
        }),
        1 => Ok(bundles.remove(0)),
        _ => Err(UpdateError::AppBundleAmbiguous {
            path: archive.to_path_buf(),
        }),
    }
}

fn unique_backup(target: &Path) -> Result<PathBuf, UpdateError> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "app bundle path has no parent directory",
        )
    })?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "App.app".into());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    for attempt in 0..16 {
        let candidate = parent.join(format!(
            ".{name}.{}.{stamp}.{attempt}.bak",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot create a unique app bundle backup name",
    )
    .into())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use auro_sign::SigningKey;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::{Manifest, Updater};

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).expect("zip");
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("entry");
            writer.write_all(bytes).expect("entry bytes");
        }
        writer.finish().expect("finish zip");
    }

    fn stage_archive(temp: &tempfile::TempDir, archive: &Path) -> StagedUpdate {
        let key = SigningKey::generate();
        let manifest = Manifest::for_artifact(
            archive,
            "1.2.3",
            "macos-aarch64",
            "https://example.test/Demo.app.zip",
            "",
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("macos-aarch64")
            .stage_from_path(&manifest, archive)
            .expect("stage")
    }

    fn staged(temp: &tempfile::TempDir, entries: &[(&str, &[u8])]) -> StagedUpdate {
        let archive = temp.path().join("Demo.app.zip");
        write_zip(&archive, entries);
        stage_archive(temp, &archive)
    }

    fn target(temp: &tempfile::TempDir) -> PathBuf {
        let path = temp.path().join("Demo.app");
        fs::create_dir_all(path.join("Contents")).expect("old bundle");
        fs::write(path.join("Contents").join("old.txt"), b"old").expect("old file");
        path
    }

    #[test]
    fn installs_and_rolls_back_an_app_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(
            &temp,
            &[
                ("Demo.app/Contents/Info.plist", b"new plist"),
                ("Demo.app/Contents/MacOS/Demo", b"new binary"),
            ],
        );
        let target = target(&temp);
        let installer = MacAppInstaller::new(&target);
        assert_eq!(installer.target(), target);

        let receipt = installer.install(&staged).expect("install");
        assert_eq!(receipt.target(), target);
        assert_eq!(
            fs::read(target.join("Contents/Info.plist")).expect("new plist"),
            b"new plist"
        );
        assert_eq!(
            fs::read(receipt.backup().join("Contents/old.txt")).expect("old backup"),
            b"old"
        );

        receipt.rollback().expect("rollback");
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
        assert!(!target.join("Contents/Info.plist").exists());
    }

    #[test]
    fn discards_the_backup_after_success() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, &[("Demo.app/Contents/Info.plist", b"new")]);
        let target = target(&temp);

        let receipt = MacAppInstaller::new(&target)
            .install(&staged)
            .expect("install");
        let backup = receipt.backup().to_path_buf();
        receipt.discard().expect("discard");
        assert!(!backup.exists());
        assert!(target.join("Contents/Info.plist").exists());
    }

    #[test]
    fn rejects_an_archive_without_an_app_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, &[("README.txt", b"not a bundle")]);
        let target = target(&temp);

        assert!(matches!(
            MacAppInstaller::new(&target).install(&staged),
            Err(UpdateError::AppBundleMissing { .. })
        ));
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
    }

    #[test]
    fn rejects_an_ambiguous_archive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(
            &temp,
            &[
                ("One.app/Contents/Info.plist", b"one"),
                ("Two.app/Contents/Info.plist", b"two"),
            ],
        );
        let target = target(&temp);

        assert!(matches!(
            MacAppInstaller::new(&target).install(&staged),
            Err(UpdateError::AppBundleAmbiguous { .. })
        ));
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
    }

    #[test]
    fn rollback_reports_a_missing_backup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, &[("Demo.app/Contents/Info.plist", b"new")]);
        let target = target(&temp);

        let receipt = MacAppInstaller::new(&target)
            .install(&staged)
            .expect("install");
        fs::remove_dir_all(receipt.backup()).expect("remove backup");
        assert!(matches!(
            receipt.rollback(),
            Err(UpdateError::RollbackBackupMissing { .. })
        ));
        assert!(target.join("Contents/Info.plist").exists());
    }

    #[test]
    fn rejects_a_missing_staged_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, &[("Demo.app/Contents/Info.plist", b"new")]);
        fs::remove_file(staged.path()).expect("remove staged");
        let target = target(&temp);

        assert!(matches!(
            MacAppInstaller::new(&target).install(&staged),
            Err(UpdateError::StagedArtifactMissing { .. })
        ));
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
    }

    #[test]
    fn rejects_missing_or_non_directory_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, &[("Demo.app/Contents/Info.plist", b"new")]);

        assert!(matches!(
            MacAppInstaller::new(temp.path().join("Missing.app")).install(&staged),
            Err(UpdateError::InstallTargetMissing { .. })
        ));

        let file = temp.path().join("File.app");
        fs::write(&file, b"not a directory").expect("file");
        assert!(matches!(
            MacAppInstaller::new(&file).install(&staged),
            Err(UpdateError::InstallTargetNotDirectory { .. })
        ));
    }

    #[test]
    fn corrupt_archive_leaves_the_target_untouched() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("Demo.app.zip");
        fs::write(&archive, b"not a zip").expect("archive");
        let staged = stage_archive(&temp, &archive);
        let target = target(&temp);

        assert!(matches!(
            MacAppInstaller::new(&target).install(&staged),
            Err(UpdateError::Archive(_))
        ));
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn allows_symlinks_inside_the_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("Demo.app.zip");
        let file = fs::File::create(&archive).expect("zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "Demo.app/Contents/Frameworks/A.framework/Versions/A/Resources/info",
                SimpleFileOptions::default(),
            )
            .expect("entry");
        writer.write_all(b"inside").expect("entry bytes");
        writer
            .add_symlink(
                "Demo.app/Contents/Frameworks/A.framework/Versions/Current",
                "A",
                SimpleFileOptions::default(),
            )
            .expect("symlink");
        writer.finish().expect("finish zip");

        let staged = stage_archive(&temp, &archive);
        let target = target(&temp);
        MacAppInstaller::new(&target)
            .install(&staged)
            .expect("install");
        assert_eq!(
            fs::read(
                target.join("Contents/Frameworks/A.framework/Versions/Current/Resources/info")
            )
            .expect("inside symlink"),
            b"inside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_escaping_the_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("Demo.app.zip");
        let file = fs::File::create(&archive).expect("zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("Demo.app/Contents/Info.plist", SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(b"plist").expect("entry bytes");
        writer
            .add_symlink(
                "Demo.app/Contents/escape",
                "../../../../outside",
                SimpleFileOptions::default(),
            )
            .expect("symlink");
        writer.finish().expect("finish zip");

        let staged = stage_archive(&temp, &archive);
        let target = target(&temp);
        assert!(matches!(
            MacAppInstaller::new(&target).install(&staged),
            Err(UpdateError::UnsafeArchive { .. })
        ));
        assert_eq!(
            fs::read(target.join("Contents/old.txt")).expect("old"),
            b"old"
        );
    }

    #[test]
    fn rejects_an_unsupported_archive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let source = temp.path().join("Demo.dmg");
        fs::write(&source, b"not a zip").expect("dmg");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "macos-aarch64",
            "https://example.test/Demo.dmg",
            "",
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        let staged = Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("macos-aarch64")
            .stage_from_path(&manifest, &source)
            .expect("stage");

        assert!(matches!(
            MacAppInstaller::new(target(&temp)).install(&staged),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }
}
