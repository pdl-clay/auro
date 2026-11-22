// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal `fs` effects performed on behalf of a verified plugin.
//!
//! Every effect resolves the path, confines it to the canonical grant root and calls
//! [`PluginPolicyExt::check_plugin`] in the same function that opens the handle. The host injects
//! the [`VerifiedSubject`] from its loaded instance; no subject is ever read from the wire.
//! Creating a file that does not exist is deliberately rejected in this stage (spec 0035).

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use auro_perms::{Capability, Denied, Policy};
use thiserror::Error;

use crate::registry::{PluginPolicyExt, VerifiedSubject};

/// Why a plugin file effect was refused.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EffectError {
    /// The path was not absolute or contained a prefix, `.`/`..` or root component in a position
    /// where a plain file path cannot have one.
    #[error("invalid file path `{0}`")]
    InvalidPath(String),

    /// The path does not resolve to an existing file. Creating new files is not part of this stage.
    #[error("path `{0}` does not resolve to an existing file")]
    UnresolvedPath(String),

    /// The resolved path escaped the canonical grant root.
    #[error("path `{0}` is outside the plugin grant root")]
    OutsideGrant(String),

    /// A symlink, junction or reparse point was found or appeared during the effect.
    #[error("symlink or reparse point at `{0}`")]
    Symlink(String),

    /// The resolved path is not a regular file.
    #[error("path `{0}` is not a regular file")]
    NotRegularFile(String),

    /// The policy denied the effect.
    #[error("capability denied: {0}")]
    Denied(#[from] Denied),

    /// An operating-system I/O error occurred.
    #[error("file effect failed: {0}")]
    Io(#[from] io::Error),
}

/// Host-side file effects scoped to one grant root.
///
/// The root is canonicalized at each effect so an app can create it after constructing the
/// gateway. Both the root and the requested path must resolve inside the same canonical tree.
#[derive(Clone, Debug)]
pub struct EffectGateway {
    policy: Policy,
    root: PathBuf,
}

impl EffectGateway {
    /// Creates a gateway using `policy` and a root directory at `root`.
    ///
    /// No I/O happens here: an unresolvable root is reported by the first effect that uses it.
    pub fn new(policy: Policy, root: impl Into<PathBuf>) -> Self {
        Self {
            policy,
            root: root.into(),
        }
    }

    /// Reads a file after validating and authorizing `path` for `subject`.
    pub fn read_file(&self, subject: &VerifiedSubject, path: &str) -> Result<Vec<u8>, EffectError> {
        let canonical = self.authorized_path(subject, path)?;
        let mut file = open_file(&canonical, false, path)?;
        revalidate(&canonical, path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(EffectError::Io)?;
        Ok(bytes)
    }

    /// Replaces a file's contents after validating and authorizing `path` for `subject`.
    ///
    /// The handle is opened without `O_TRUNC`; truncation happens only after the post-open
    /// revalidation, so a swap between the policy check and the open cannot truncate a file that
    /// was never authorized.
    pub fn write_file(
        &self,
        subject: &VerifiedSubject,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), EffectError> {
        let canonical = self.authorized_path(subject, path)?;
        let mut file = open_file(&canonical, true, path)?;
        revalidate(&canonical, path)?;
        file.set_len(0).map_err(EffectError::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(EffectError::Io)?;
        file.write_all(bytes).map_err(EffectError::Io)?;
        file.flush().map_err(EffectError::Io)?;
        Ok(())
    }

    /// The compiled policy used at each effect site.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The configured grant root, before canonicalization.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn authorized_path(
        &self,
        subject: &VerifiedSubject,
        path: &str,
    ) -> Result<PathBuf, EffectError> {
        validate_lexical(path)?;
        let requested = Path::new(path);
        let meta = std::fs::symlink_metadata(requested).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                EffectError::UnresolvedPath(path.to_owned())
            } else {
                EffectError::Io(error)
            }
        })?;
        reject_link(&meta, path)?;
        if !meta.is_file() {
            return Err(EffectError::NotRegularFile(path.to_owned()));
        }
        let canonical = canonicalize(requested, path)?;
        let root = canonicalize(&self.root, path)?;
        if !canonical.starts_with(&root) {
            return Err(EffectError::OutsideGrant(path.to_owned()));
        }
        let canonical_str = canonical
            .to_str()
            .ok_or_else(|| EffectError::InvalidPath(path.to_owned()))?;
        // Effect site: the verified subject and the canonical value are checked immediately
        // before the handle is opened. No earlier check is trusted (spec 0034 D5).
        self.policy
            .check_plugin(subject, Capability::Fs, Some(canonical_str))?;
        Ok(canonical)
    }
}

fn validate_lexical(value: &str) -> Result<(), EffectError> {
    let invalid = || EffectError::InvalidPath(value.to_owned());
    // `canonicalize` on Windows returns a `\\?\` verbatim path; the prefix is not a duplicated
    // separator and must be accepted at the effect site.
    let lexical = value.strip_prefix(r"\\?\").unwrap_or(value);
    let parts: Vec<&str> = lexical.split(['/', '\\']).collect();
    // The first empty part is the leading separator of an absolute path; any other empty part is
    // a duplicated separator and is rejected rather than silently normalized by canonicalize.
    if value.is_empty()
        || parts
            .iter()
            .enumerate()
            .any(|(index, part)| *part == "." || *part == ".." || (index > 0 && part.is_empty()))
    {
        return Err(invalid());
    }
    let path = Path::new(value);
    let rooted = path.is_absolute() || value.starts_with(['/', '\\']);
    if !rooted {
        return Err(invalid());
    }
    let mut components = path.components();
    // A drive/UNC prefix is only legal as the very first component, and must be followed by a
    // root directory. A prefix anywhere else is rejected.
    if matches!(components.clone().next(), Some(Component::Prefix(_))) {
        components.next();
    }
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(invalid());
    }
    if !components.all(|component| matches!(component, Component::Normal(_))) {
        return Err(invalid());
    }
    Ok(())
}

fn reject_link(meta: &std::fs::Metadata, path: &str) -> Result<(), EffectError> {
    if meta.file_type().is_symlink() {
        return Err(EffectError::Symlink(path.to_owned()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT is 0x400 in the Win32 API.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(EffectError::Symlink(path.to_owned()));
        }
    }
    Ok(())
}

fn canonicalize(path: &Path, original: &str) -> Result<PathBuf, EffectError> {
    std::fs::canonicalize(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            EffectError::UnresolvedPath(original.to_owned())
        } else {
            EffectError::Io(error)
        }
    })
}

fn open_file(canonical: &Path, write: bool, original: &str) -> Result<std::fs::File, EffectError> {
    let mut options = std::fs::OpenOptions::new();
    if write {
        // No truncate: revalidate before changing the file's length.
        options.write(true);
    } else {
        options.read(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT is 0x00200000 in the Win32 API. Revalidation below
        // rejects the reparse point instead of following it.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(canonical).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return EffectError::Symlink(original.to_owned());
        }
        EffectError::Io(error)
    })?;
    let meta = file.metadata().map_err(EffectError::Io)?;
    reject_link(&meta, original)?;
    if !meta.is_file() {
        return Err(EffectError::NotRegularFile(original.to_owned()));
    }
    Ok(file)
}

fn revalidate(canonical: &Path, original: &str) -> Result<(), EffectError> {
    let after = canonicalize(canonical, original)?;
    if after != canonical {
        return Err(EffectError::Symlink(original.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_prefix_and_relative_paths() {
        for path in [
            "relative/file",
            "/root/../secret",
            "/root/./file",
            "/root//file",
        ] {
            assert!(validate_lexical(path).is_err(), "{path}");
        }
        assert!(validate_lexical("/root/file").is_ok());
        #[cfg(windows)]
        {
            assert!(validate_lexical(r"C:\root\file").is_ok());
            assert!(validate_lexical(r"C:root\file").is_err());
        }
    }
}
