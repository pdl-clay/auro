// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS `.app` bundle replacement from a staged `.zip`.
//!
//! The app bundle is swapped as a directory: the old bundle is renamed aside, the extracted
//! bundle is renamed into place, and the old bundle is retained until the caller discards it or
//! rolls back. `.dmg` mounting and restart orchestration are separate concerns.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zip::ZipArchive;

use crate::{StagedUpdate, UpdateError};

/// Maximum expanded bytes one `.app.zip` may produce.
///
/// The manifest verifies the compressed artifact, not its expansion. A signed but malformed or
/// hostile release must not be able to fill the disk after the app has exited.
const MAX_EXPANDED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Maximum number of entries one `.app.zip` may contain.
const MAX_ARCHIVE_ENTRIES: usize = 100_000;

/// Maximum link target one symlink entry may declare.
///
/// No filesystem accepts a target longer than `PATH_MAX`, and the target has to fit in memory
/// because it is the entry's payload; bounding it keeps a lying header from forcing a huge read.
const MAX_SYMLINK_TARGET_BYTES: u64 = 4096;

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

fn extract_archive(archive_path: &Path, destination: &Path) -> Result<(), UpdateError> {
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;
    let entries = archive.len();
    check_archive_budget(entries, 0, archive_path)?;

    // The central-directory sizes are attacker-controlled, so they only narrow each read. The
    // budget is charged for bytes actually produced by the decompressor and every entry must
    // produce exactly the size it declared.
    let mut written_total = 0u64;
    for index in 0..entries {
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| UpdateError::UnsafeArchive {
                path: archive_path.to_path_buf(),
            })?;
        let out_path = destination.join(&relative);
        let declared = entry.size();
        let is_dir = entry.is_dir();
        let is_symlink = entry.is_symlink();
        let mode = entry.unix_mode();

        if is_dir {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        // A previous entry can be a symlink to a directory outside the extraction root; creating
        // or writing a file below it would follow that link out of the sandbox. The lexical
        // containment check in `validate_extracted_tree` runs after the bytes are on disk, so the
        // parent chain is checked before every write.
        reject_symlinked_parents(destination, &relative, archive_path)?;
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let remaining = MAX_EXPANDED_BYTES - written_total;
        let limit = declared.min(remaining);
        if is_symlink {
            let limit = limit.min(MAX_SYMLINK_TARGET_BYTES);
            let mut target = Vec::new();
            let read = (&mut entry).take(limit + 1).read_to_end(&mut target)?;
            charge_expansion(read as u64, declared, &mut written_total, archive_path)?;
            write_symlink(&out_path, &relative, &target, archive_path)?;
            continue;
        }

        let mut output = fs::File::create(&out_path)?;
        let written = io::copy(&mut (&mut entry).take(limit + 1), &mut output)?;
        drop(output);
        charge_expansion(written, declared, &mut written_total, archive_path)?;
        set_file_mode(&out_path, mode)?;
    }
    Ok(())
}

/// Rejects a destination whose existing parent chain inside the extraction root is not made of
/// real directories.
///
/// Zip entries are written in archive order, so an earlier symlink entry can turn a later entry's
/// parent into a link that escapes the root. Missing components are fine: `create_dir_all` creates
/// them as real directories afterwards.
fn reject_symlinked_parents(
    root: &Path,
    relative: &Path,
    archive: &Path,
) -> Result<(), UpdateError> {
    let components: Vec<_> = relative.components().collect();
    let mut current = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(UpdateError::UnsafeArchive {
                    path: archive.to_path_buf(),
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(UpdateError::UnsafeArchive {
                    path: archive.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Charges bytes actually written against the expansion budget.
///
/// An entry whose decompressed output differs from its declared size is lying about the archive,
/// which is exactly the case the budget must reject even when the total stays under the limit.
fn charge_expansion(
    written: u64,
    declared: u64,
    written_total: &mut u64,
    archive: &Path,
) -> Result<(), UpdateError> {
    let unsafe_archive = || UpdateError::UnsafeArchive {
        path: archive.to_path_buf(),
    };
    if written != declared {
        return Err(unsafe_archive());
    }
    *written_total = written_total
        .checked_add(written)
        .ok_or_else(unsafe_archive)?;
    if *written_total > MAX_EXPANDED_BYTES {
        return Err(unsafe_archive());
    }
    Ok(())
}

#[cfg(unix)]
fn write_symlink(
    out_path: &Path,
    relative: &Path,
    target: &[u8],
    archive_path: &Path,
) -> Result<(), UpdateError> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::symlink;

    let target = Path::new(OsStr::from_bytes(target));
    if !symlink_stays_within(relative, target) {
        return Err(UpdateError::UnsafeArchive {
            path: archive_path.to_path_buf(),
        });
    }
    symlink(target, out_path)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_symlink(
    _out_path: &Path,
    _relative: &Path,
    _target: &[u8],
    archive_path: &Path,
) -> Result<(), UpdateError> {
    // A `.app` bundle is only installed on macOS, where symlinks can be preserved; refusing here
    // keeps a symlink entry from being written as a regular file on other targets.
    Err(UpdateError::UnsafeArchive {
        path: archive_path.to_path_buf(),
    })
}

/// Lexically resolves a link target against the link's parent and requires it to stay inside the
/// extraction root.
///
/// The archive is untrusted, so absolute targets and `..` walks above the root are rejected before
/// the symlink is created.
#[cfg(unix)]
fn symlink_stays_within(link: &Path, target: &Path) -> bool {
    use std::path::Component;

    if target.is_absolute() {
        return false;
    }
    let parent = link.parent().unwrap_or(Path::new(""));
    let mut depth = 0usize;
    for component in parent.components().chain(target.components()) {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return false,
            },
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: Option<u32>) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt as _;

    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: Option<u32>) -> Result<(), UpdateError> {
    Ok(())
}

fn check_archive_budget(entries: usize, expanded: u64, archive: &Path) -> Result<(), UpdateError> {
    if entries > MAX_ARCHIVE_ENTRIES || expanded > MAX_EXPANDED_BYTES {
        return Err(UpdateError::UnsafeArchive {
            path: archive.to_path_buf(),
        });
    }
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
        // macOS tempdirs live under /var, a symlink to /private/var; the receipt records the
        // canonical target, so compare against the canonical form.
        assert_eq!(
            receipt.target(),
            target.canonicalize().expect("canonical target")
        );
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
    fn rejects_archives_over_the_expansion_budget() {
        let path = Path::new("Demo.app.zip");
        check_archive_budget(1, MAX_EXPANDED_BYTES, path).expect("at the byte limit");
        assert!(matches!(
            check_archive_budget(1, MAX_EXPANDED_BYTES + 1, path),
            Err(UpdateError::UnsafeArchive { .. })
        ));
        check_archive_budget(MAX_ARCHIVE_ENTRIES, 0, path).expect("at the entry limit");
        assert!(matches!(
            check_archive_budget(MAX_ARCHIVE_ENTRIES + 1, 0, path),
            Err(UpdateError::UnsafeArchive { .. })
        ));
    }

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

    #[test]
    fn rejects_a_declared_size_lie_instead_of_expanding_it() {
        // A hostile but signature-valid release: the ZIP declares one byte and deflate expands
        // far past it. The budget must be charged for bytes written, not for the header.
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("Demo.app.zip");
        let payload_len = 64 * 1024 * 1024u64;
        {
            let file = fs::File::create(&archive_path).expect("zip");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "Demo.app/Contents/bomb.bin",
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .expect("entry");
            let chunk = vec![0u8; 1024 * 1024];
            for _ in 0..(payload_len / chunk.len() as u64) {
                writer.write_all(&chunk).expect("payload");
            }
            writer.finish().expect("finish zip");
        }
        let mut bytes = fs::read(&archive_path).expect("read zip");
        // Local header: signature 0..4, uncompressed size at 22. Central header: signature
        // 0..4, uncompressed size at 24. Both copies must lie, or the reader trusts the other.
        let local = bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x03, 0x04])
            .expect("local header");
        bytes[local + 22..local + 26].copy_from_slice(&1u32.to_le_bytes());
        let central = bytes
            .windows(4)
            .rposition(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("central header");
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        fs::write(&archive_path, &bytes).expect("patched zip");

        let destination = temp.path().join("extracted");
        fs::create_dir_all(&destination).expect("destination");
        assert!(matches!(
            extract_archive(&archive_path, &destination),
            Err(UpdateError::UnsafeArchive { .. })
        ));
        let expanded = fs::metadata(destination.join("Demo.app/Contents/bomb.bin"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        assert!(
            expanded < payload_len,
            "declared-size lie expanded {expanded} bytes"
        );
    }

    #[test]
    fn rejects_a_file_written_through_a_symlinked_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("Demo.app.zip");
        let file = fs::File::create(&archive_path).expect("zip");
        let mut writer = zip::ZipWriter::new(file);
        // Lexically inside the root, so the link itself is allowed; the next entry tries to use
        // it as a parent directory and must be refused before the write follows the link.
        writer
            .add_symlink(
                "Demo.app/Contents/dir",
                "../..",
                SimpleFileOptions::default(),
            )
            .expect("symlink");
        writer
            .start_file("Demo.app/Contents/dir/evil", SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(b"escaped").expect("entry bytes");
        writer.finish().expect("finish zip");

        let destination = temp.path().join("extracted");
        fs::create_dir_all(&destination).expect("destination");
        assert!(matches!(
            extract_archive(&archive_path, &destination),
            Err(UpdateError::UnsafeArchive { .. })
        ));
        assert!(
            !destination.join("evil").exists(),
            "a later entry must not write through an earlier symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn extracts_directories_modes_and_symlinks() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("Demo.app.zip");
        let file = fs::File::create(&archive_path).expect("zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .add_directory("Demo.app/Contents/MacOS", SimpleFileOptions::default())
            .expect("directory entry");
        writer
            .start_file(
                "Demo.app/Contents/MacOS/Demo",
                SimpleFileOptions::default()
                    .unix_permissions(0o755)
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("executable");
        writer.write_all(b"binary").expect("executable bytes");
        writer
            .start_file(
                "Demo.app/Contents/Resources/notes.txt",
                SimpleFileOptions::default(),
            )
            .expect("resource");
        writer.write_all(b"notes").expect("resource bytes");
        writer
            .add_symlink(
                "Demo.app/Contents/MacOS/Current",
                "Demo",
                SimpleFileOptions::default(),
            )
            .expect("symlink");
        writer.finish().expect("finish zip");

        let destination = temp.path().join("extracted");
        fs::create_dir_all(&destination).expect("destination");
        extract_archive(&archive_path, &destination).expect("extract");

        let executable = destination.join("Demo.app/Contents/MacOS/Demo");
        assert_eq!(fs::read(&executable).expect("executable"), b"binary");
        let mode = fs::metadata(&executable)
            .expect("mode")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "executable bit must survive extraction");
        assert_eq!(
            fs::read(destination.join("Demo.app/Contents/Resources/notes.txt")).expect("notes"),
            b"notes"
        );
        let link = destination.join("Demo.app/Contents/MacOS/Current");
        assert_eq!(fs::read_link(&link).expect("link"), Path::new("Demo"));
        assert_eq!(fs::read(&link).expect("through link"), b"binary");
    }

    #[test]
    fn rejects_a_symlink_entry_escaping_the_extraction_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("Demo.app.zip");
        let file = fs::File::create(&archive_path).expect("zip");
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

        let destination = temp.path().join("extracted");
        fs::create_dir_all(&destination).expect("destination");
        assert!(matches!(
            extract_archive(&archive_path, &destination),
            Err(UpdateError::UnsafeArchive { .. })
        ));
        assert!(
            fs::symlink_metadata(destination.join("Demo.app/Contents/escape")).is_err(),
            "escaping symlink must not be created"
        );
    }
}
