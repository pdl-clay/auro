// SPDX-License-Identifier: MIT OR Apache-2.0

//! Asset sources for GPUI elements that load bytes, including `svg().path()`.
//!
//! The trait is GPUI's [`AssetSource`], re-exported at `auro::gpui::AssetSource`. Auro only adds a
//! type-erased adapter for the builder and two small ready-made sources. The app chooses where the
//! bytes come from; Auro does not fetch anything at runtime.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io;
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

/// An [`AssetSource`] rooted at a filesystem directory chosen by the app.
///
/// Asset paths use `/` (and reject `\`) regardless of the host OS. The root is canonicalized on
/// every lookup; a path is accepted only when it stays under that root after symlink resolution.
/// Absolute paths, drive prefixes, empty components and `.`/`..` are rejected before any I/O.
#[derive(Clone, Debug)]
pub struct FileAssets {
    root: PathBuf,
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

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let root = std::fs::canonicalize(&self.root)?;
        let mut target = root.clone();
        if !path.is_empty() {
            for component in path.split(['/', '\\']) {
                if component.is_empty() || component == "." || component == ".." {
                    return Err(invalid_path(path, "non-normal component").into());
                }
                if component.contains(':') {
                    return Err(invalid_path(path, "absolute or drive-prefixed component").into());
                }
                target.push(component);
            }
        }
        let canonical = std::fs::canonicalize(&target)?;
        if !canonical.starts_with(&root) {
            return Err(invalid_path(path, "resolves outside the asset root").into());
        }
        Ok(canonical)
    }
}

impl AssetSource for FileAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let target = self.resolve(path)?;
        match std::fs::read(&target) {
            Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let target = self.resolve(path)?;
        let mut names = BTreeSet::new();
        for entry in std::fs::read_dir(&target)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if !name.contains('/') && !name.contains('\\') {
                names.insert(name.into_owned());
            }
        }
        Ok(names.into_iter().map(SharedString::from).collect())
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
