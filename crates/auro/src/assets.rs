// SPDX-License-Identifier: MIT OR Apache-2.0

//! Asset sources for GPUI elements that load bytes, including `svg().path()`.
//!
//! The trait is GPUI's [`AssetSource`], re-exported at `auro::gpui::AssetSource`. Auro only adds a
//! type-erased adapter for the builder and two small ready-made sources. The app chooses where the
//! bytes come from; Auro does not fetch anything at runtime.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auro_ui::gpui::{AssetSource, Result, SharedString};

/// An [`AssetSource`] whose concrete type has been erased for the app builder.
///
/// GPUI still receives a concrete `AssetSource`; the delegation is complete, so capabilities of the
/// original source (paths, caches, embedded bytes) are preserved.
pub(crate) struct ErasedAssets(pub(crate) Arc<dyn AssetSource>);

impl AssetSource for ErasedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        self.0.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        self.0.list(path)
    }
}

fn invalid_path(path: &str, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid asset path `{path}`: {reason}"),
    )
}

/// A validated asset was changed, or cannot be accessed safely on this platform.
///
/// These errors are returned as typed values inside GPUI's `anyhow::Result`, so callers can use
/// `downcast_ref::<AssetAccessError>()` to distinguish a swap from an ordinary I/O failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum AssetAccessError {
    /// A symlink or reparse point was found where the validated path requires a real entry.
    Link {
        /// The asset path that was rejected.
        path: String,
    },
    /// The opened handle is not the canonical target (type, device, inode or index differs).
    IdentityMismatch {
        /// The asset path that was rejected.
        path: String,
    },
    /// This platform cannot perform the operation without an unsafe path-based race.
    Unsupported {
        /// The asset path that was rejected.
        path: String,
        /// The operation that cannot be performed safely.
        operation: &'static str,
    },
}

impl AssetAccessError {
    /// The I/O error category for this rejection.
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Unsupported { .. } => io::ErrorKind::Unsupported,
            Self::Link { .. } | Self::IdentityMismatch { .. } => io::ErrorKind::InvalidData,
        }
    }
}

impl fmt::Display for AssetAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link { path } => {
                write!(
                    f,
                    "asset `{path}` became a symlink or reparse point after validation"
                )
            }
            Self::IdentityMismatch { path } => {
                write!(f, "asset `{path}` changed between validation and open")
            }
            Self::Unsupported { path, operation } => {
                write!(
                    f,
                    "cannot safely {operation} asset `{path}` on this platform"
                )
            }
        }
    }
}

impl std::error::Error for AssetAccessError {}

/// An [`AssetSource`] rooted at a filesystem directory chosen by the app.
///
/// Asset paths use `/` (and reject `\`) regardless of the host OS. Absolute paths, drive prefixes,
/// empty components and `.`/`..` are rejected before any I/O. The root and target are canonicalized
/// to check containment; the file is then opened through directory descriptors with no symlink
/// following, and the opened object's identity must match the canonical target before any byte is
/// read. On Windows the final handle is opened with `FILE_FLAG_OPEN_REPARSE_POINT` and checked the
/// same way. See [`AssetAccessError`] for the typed rejections.
#[derive(Clone, Debug)]
pub struct FileAssets {
    root: PathBuf,
}

fn split_path(path: &str) -> Result<Vec<&str>> {
    if path.contains('\\') {
        return Err(invalid_path(path, "backslashes are not allowed").into());
    }
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." | ".." => {
                return Err(invalid_path(path, "non-normal component").into());
            }
            part if part.contains(':') => {
                return Err(invalid_path(path, "absolute or drive-prefixed component").into());
            }
            part => parts.push(part),
        }
    }
    Ok(parts)
}

fn join_parts(root: &Path, parts: &[&str]) -> PathBuf {
    let mut target = root.to_path_buf();
    for part in parts {
        target.push(part);
    }
    target
}

fn canonicalize_target(root: &Path, parts: &[&str], path: &str) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(join_parts(root, parts))?;
    if !canonical.starts_with(root) {
        return Err(invalid_path(path, "resolves outside the asset root").into());
    }
    Ok(canonical)
}

#[cfg(unix)]
mod platform {
    use std::collections::BTreeSet;
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path};

    use super::{AssetAccessError, Result, invalid_path};

    // Every component is opened without following links; parents must be directories.
    const FILE_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    const DIR_FLAGS: libc::c_int = FILE_FLAGS | libc::O_DIRECTORY;

    struct Dir(*mut libc::DIR);

    impl Drop for Dir {
        fn drop(&mut self) {
            // SAFETY: `self.0` came from a successful `fdopendir` and is closed only here.
            unsafe { libc::closedir(self.0) };
        }
    }

    fn link(path: &str) -> AssetAccessError {
        AssetAccessError::Link {
            path: path.to_owned(),
        }
    }

    fn mismatch(path: &str) -> AssetAccessError {
        AssetAccessError::IdentityMismatch {
            path: path.to_owned(),
        }
    }

    fn c_name(name: &OsStr, path: &str) -> Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| invalid_path(path, "interior NUL byte").into())
    }

    fn open_at(dir: libc::c_int, name: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
        // SAFETY: `name` is NUL-terminated, `dir` is an open directory fd (or `AT_FDCWD`), and a
        // successful `openat` returns a new fd that this function exclusively owns.
        let raw = unsafe { libc::openat(dir, name.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` succeeded, so `raw` is a fresh fd not owned by anyone else.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn open_required(
        dir: libc::c_int,
        name: &CStr,
        flags: libc::c_int,
        path: &str,
    ) -> Result<OwnedFd> {
        match open_at(dir, name, flags) {
            Ok(fd) => Ok(fd),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ELOOP) | Some(libc::ENOTDIR)
                ) =>
            {
                Err(link(path).into())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn open_optional(
        dir: libc::c_int,
        name: &CStr,
        flags: libc::c_int,
        path: &str,
    ) -> Result<Option<OwnedFd>> {
        match open_at(dir, name, flags) {
            Ok(fd) => Ok(Some(fd)),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ELOOP) | Some(libc::ENOTDIR)
                ) =>
            {
                Err(link(path).into())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn open_canonical_root(root: &Path, path: &str) -> Result<OwnedFd> {
        // Walk from `/` with O_NOFOLLOW so no ancestor of the canonical root can be swapped for a
        // symlink between the canonicalization and the open.
        let mut current = open_required(libc::AT_FDCWD, c"/", DIR_FLAGS, path)?;
        for component in root.components() {
            if let Component::Normal(name) = component {
                let name = c_name(name, path)?;
                current = open_required(current.as_raw_fd(), &name, DIR_FLAGS, path)?;
            }
        }
        Ok(current)
    }

    fn open_chain(
        root: &Path,
        parts: &[&str],
        path: &str,
        final_flags: libc::c_int,
    ) -> Result<Option<OwnedFd>> {
        let mut current = open_canonical_root(root, path)?;
        for (index, part) in parts.iter().enumerate() {
            let flags = if index + 1 == parts.len() {
                final_flags
            } else {
                DIR_FLAGS
            };
            let name = c_name(OsStr::new(part), path)?;
            match open_optional(current.as_raw_fd(), &name, flags, path)? {
                Some(next) => current = next,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    fn verify(raw: libc::c_int, canonical: &Path, path: &str, directory: bool) -> Result<()> {
        let mut info: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: zeroed `libc::stat` is a valid value, `info` is writable, and `raw` is an open fd.
        if unsafe { libc::fstat(raw, &mut info) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let expected_type = if directory {
            libc::S_IFDIR
        } else {
            libc::S_IFREG
        };
        if info.st_mode & libc::S_IFMT != expected_type {
            return Err(mismatch(path).into());
        }
        let metadata = std::fs::symlink_metadata(canonical)?;
        let expected_type_ok = if directory {
            metadata.file_type().is_dir()
        } else {
            metadata.file_type().is_file()
        };
        if !expected_type_ok
            || metadata.dev() != info.st_dev as u64
            || metadata.ino() != info.st_ino as u64
        {
            return Err(mismatch(path).into());
        }
        Ok(())
    }

    fn enumerate(fd: &OwnedFd) -> Result<Vec<String>> {
        // `fdopendir` takes ownership on success, so hand it a close-on-exec duplicate and keep
        // the original fd.
        // SAFETY: `fd` is a valid, open directory fd; `fcntl` returns a fresh fd or -1.
        let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: `duplicate` is an fd we own; `fdopendir` owns it only when it succeeds.
        let dir = unsafe { libc::fdopendir(duplicate) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `fdopendir` failed, so ownership of `duplicate` was not transferred.
            unsafe { libc::close(duplicate) };
            return Err(error.into());
        }
        let dir = Dir(dir);
        let mut names = BTreeSet::new();
        loop {
            // SAFETY: `dir.0` is a valid `DIR`; `readdir` returns a valid entry or null.
            let entry = unsafe { libc::readdir(dir.0) };
            if entry.is_null() {
                break;
            }
            // SAFETY: a non-null `dirent` has a NUL-terminated `d_name` inside the struct.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            let name = OsStr::from_bytes(name.to_bytes()).to_string_lossy();
            if name.as_ref() == "." || name.as_ref() == ".." {
                continue;
            }
            if !name.contains('/') && !name.contains('\\') {
                names.insert(name.into_owned());
            }
        }
        Ok(names.into_iter().collect())
    }

    pub(super) fn open_file(root: &Path, parts: &[&str], path: &str) -> Result<Option<File>> {
        Ok(open_chain(root, parts, path, FILE_FLAGS)?.map(File::from))
    }

    pub(super) fn verify_file(file: &File, canonical: &Path, path: &str) -> Result<()> {
        verify(file.as_raw_fd(), canonical, path, false)
    }

    pub(super) fn list_dir(
        root: &Path,
        parts: &[&str],
        canonical: &Path,
        path: &str,
    ) -> Result<Vec<String>> {
        let Some(fd) = open_chain(root, parts, path, DIR_FLAGS)? else {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        };
        verify(fd.as_raw_fd(), canonical, path, true)?;
        enumerate(&fd)
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use super::{AssetAccessError, Result, join_parts};

    // Win32 file flags and attributes (no new dependency for these constants).
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

    fn open(target: &Path, directory: bool) -> io::Result<File> {
        let flags = if directory {
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS
        } else {
            FILE_FLAG_OPEN_REPARSE_POINT
        };
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(flags);
        options.open(target)
    }

    /// Volume serial and file index from an open handle, matching Unix's device/inode check.
    fn identity(file: &File) -> Option<(u32, u64)> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `file` owns a valid handle for the duration of this call, and `info` is a
        // writable, correctly aligned BY_HANDLE_FILE_INFORMATION. A failed call leaves a zeroed
        // struct, which the caller treats as no identity.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return None;
        }
        Some((
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ))
    }

    fn verify(file: &File, canonical: &Path, path: &str, directory: bool) -> Result<()> {
        let opened = file.metadata()?;
        // Open the canonical entry without following links so the identity comes from a handle
        // too; a swap to a reparse point or a different file is rejected below.
        let expected_file = open(canonical, directory)?;
        let expected = expected_file.metadata()?;
        if opened.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || expected.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(AssetAccessError::Link {
                path: path.to_owned(),
            }
            .into());
        }
        let opened_dir = opened.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0;
        let expected_dir = expected.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0;
        let identity_matches = match (identity(file), identity(&expected_file)) {
            (Some((left_volume, left_index)), Some((right_volume, right_index))) => {
                left_volume == right_volume && left_index == right_index
            }
            _ => false,
        };
        if opened_dir != directory || expected_dir != directory || !identity_matches {
            return Err(AssetAccessError::IdentityMismatch {
                path: path.to_owned(),
            }
            .into());
        }
        Ok(())
    }

    pub(super) fn open_file(root: &Path, parts: &[&str], _path: &str) -> Result<Option<File>> {
        match open(&join_parts(root, parts), false) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn verify_file(file: &File, canonical: &Path, path: &str) -> Result<()> {
        verify(file, canonical, path, false)
    }

    pub(super) fn list_dir(
        root: &Path,
        parts: &[&str],
        canonical: &Path,
        path: &str,
    ) -> Result<Vec<String>> {
        // The handle can be verified, but std has no fd/handle-based directory enumeration on
        // Windows. Falling back to `read_dir(canonical)` would reopen by path, so report that
        // limitation instead of claiming the containment check is race-free.
        let file = open(&join_parts(root, parts), true)?;
        verify(&file, canonical, path, true)?;
        Err(AssetAccessError::Unsupported {
            path: path.to_owned(),
            operation: "directory listing",
        }
        .into())
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::fs::File;
    use std::path::Path;

    use super::{AssetAccessError, Result};

    pub(super) fn open_file(_root: &Path, _parts: &[&str], path: &str) -> Result<Option<File>> {
        Err(AssetAccessError::Unsupported {
            path: path.to_owned(),
            operation: "file access",
        }
        .into())
    }

    pub(super) fn verify_file(_file: &File, _canonical: &Path, _path: &str) -> Result<()> {
        Ok(())
    }

    pub(super) fn list_dir(
        _root: &Path,
        _parts: &[&str],
        _canonical: &Path,
        path: &str,
    ) -> Result<Vec<String>> {
        Err(AssetAccessError::Unsupported {
            path: path.to_owned(),
            operation: "directory listing",
        }
        .into())
    }
}

impl FileAssets {
    /// Creates a source for `root`. Errors for an unusable root are reported by `load`/`list`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory this source is scoped to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl AssetSource for FileAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let parts = split_path(path)?;
        let root = std::fs::canonicalize(&self.root)?;
        let Some(mut file) = platform::open_file(&root, &parts, path)? else {
            return Ok(None);
        };
        let canonical = canonicalize_target(&root, &parts, path)?;
        platform::verify_file(&file, &canonical, path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(Cow::Owned(bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let parts = split_path(path)?;
        let root = std::fs::canonicalize(&self.root)?;
        let canonical = canonicalize_target(&root, &parts, path)?;
        Ok(platform::list_dir(&root, &parts, &canonical, path)?
            .into_iter()
            .map(SharedString::from)
            .collect())
    }
}

/// An [`AssetSource`] backed by a static table of `(path, bytes)` pairs.
///
/// The table is the whole trust boundary: no filesystem or network access is performed. Build the
/// bytes with `include_bytes!` in the app crate.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedAssets {
    entries: &'static [(&'static str, &'static [u8])],
}

impl EmbeddedAssets {
    /// Creates a source over a static table. Duplicate paths use the first entry.
    pub const fn new(entries: &'static [(&'static str, &'static [u8])]) -> Self {
        Self { entries }
    }
}

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(self
            .entries
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut names = BTreeSet::new();
        for (name, _) in self.entries {
            if let Some(rest) = name.strip_prefix(&prefix) {
                if let Some((child, remainder)) = rest.split_once('/') {
                    if !child.is_empty() && !remainder.is_empty() {
                        names.insert(child.to_owned());
                    }
                } else if !rest.is_empty() {
                    names.insert(rest.to_owned());
                }
            }
        }
        Ok(names.into_iter().map(SharedString::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_load_and_list_without_io() {
        static ENTRIES: &[(&str, &[u8])] = &[
            ("icons/logo.svg", b"<svg/>"),
            ("icons/nested/other.svg", b"<svg/>"),
        ];
        let assets = EmbeddedAssets::new(ENTRIES);
        assert_eq!(
            assets.load("icons/logo.svg").expect("load").as_deref(),
            Some(&b"<svg/>"[..])
        );
        assert!(assets.load("missing").expect("load").is_none());
        let names: Vec<String> = assets
            .list("icons")
            .expect("list")
            .into_iter()
            .map(|name| name.to_string())
            .collect();
        assert_eq!(names, ["logo.svg", "nested"]);
    }

    #[cfg(unix)]
    #[test]
    fn file_assets_reject_traversal_and_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("assets");
        std::fs::create_dir(&root).expect("root");
        std::fs::write(root.join("logo.svg"), b"inside").expect("file");
        let outside = temp.path().join("secret.svg");
        std::fs::write(&outside, b"outside").expect("outside");
        symlink(&outside, root.join("escape.svg")).expect("symlink");

        let assets = FileAssets::new(&root);
        assert_eq!(
            assets.load("logo.svg").expect("load").as_deref(),
            Some(&b"inside"[..])
        );
        for path in [
            "../secret.svg",
            "/etc/passwd",
            "icons/../../secret.svg",
            "a\\..\\b",
        ] {
            assert!(assets.load(path).is_err(), "{path} must be rejected");
        }
        assert!(
            assets.load("escape.svg").is_err(),
            "symlink escape must fail"
        );
    }
}
