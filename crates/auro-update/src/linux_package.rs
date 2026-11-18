// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux `.deb` and `.rpm` package installation.
//!
//! The package manager owns the transaction. The helper runs it shell-free from an absolute path;
//! elevation is the caller's responsibility and a permission failure is reported as
//! [`UpdateError::InstallerFailed`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{InstallerRunner, StagedUpdate, SystemInstallerRunner, UpdateError};

/// The Linux package format detected from the staged artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxPackageKind {
    /// Debian package.
    Deb,
    /// RPM package.
    Rpm,
}

impl LinuxPackageKind {
    /// Detects the package format from `.deb` or `.rpm`.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("deb") {
            Some(Self::Deb)
        } else if extension.eq_ignore_ascii_case("rpm") {
            Some(Self::Rpm)
        } else {
            None
        }
    }
}

/// Runs staged `.deb`/`.rpm` packages through the host package manager.
pub struct LinuxPackageInstaller {
    runner: Box<dyn InstallerRunner>,
}

impl Default for LinuxPackageInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxPackageInstaller {
    /// Creates an installer that runs package commands with the operating system.
    pub fn new() -> Self {
        Self::with_runner(SystemInstallerRunner)
    }

    /// Creates an installer with a test or custom command runner.
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self {
        LinuxPackageInstaller {
            runner: Box::new(runner),
        }
    }

    /// Runs `dpkg -i` or `rpm -U` on the staged package.
    ///
    /// The package manager's exit code is the result. Elevation is not requested; a non-root
    /// invocation that the package manager rejects surfaces as `InstallerFailed`.
    pub fn install(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError> {
        let path = staged.path();
        if !path.is_file() {
            return Err(UpdateError::StagedArtifactMissing {
                path: path.to_path_buf(),
            });
        }
        let kind =
            LinuxPackageKind::from_path(path).ok_or_else(|| UpdateError::UnsupportedInstaller {
                path: path.to_path_buf(),
                expected: ".deb or .rpm",
            })?;
        let (program, args) = command_for(kind, path);
        let code = self.runner.run(&program, &args)?;
        if code == Some(0) {
            Ok(kind)
        } else {
            Err(UpdateError::InstallerFailed {
                path: path.to_path_buf(),
                code,
            })
        }
    }
}

fn command_for(kind: LinuxPackageKind, artifact: &Path) -> (PathBuf, Vec<OsString>) {
    match kind {
        LinuxPackageKind::Deb => (
            PathBuf::from("/usr/bin/dpkg"),
            vec![OsString::from("-i"), artifact.as_os_str().to_owned()],
        ),
        LinuxPackageKind::Rpm => (
            PathBuf::from("/usr/bin/rpm"),
            vec![OsString::from("-U"), artifact.as_os_str().to_owned()],
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auro_sign::SigningKey;

    use super::*;
    use crate::{Manifest, Updater};

    type Call = (PathBuf, Vec<OsString>);

    #[derive(Clone, Default)]
    struct MockRunner {
        code: Option<i32>,
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl MockRunner {
        fn with_code(code: i32) -> Self {
            MockRunner {
                code: Some(code),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("runner").clone()
        }
    }

    impl InstallerRunner for MockRunner {
        fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, UpdateError> {
            self.calls
                .lock()
                .expect("runner")
                .push((program.to_path_buf(), args.to_vec()));
            Ok(self.code)
        }
    }

    fn staged(temp: &tempfile::TempDir, extension: &str) -> StagedUpdate {
        let key = SigningKey::generate();
        let source = temp.path().join(format!("package.{extension}"));
        std::fs::write(&source, b"package bytes").expect("package");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "linux-x86_64",
            format!("https://example.test/package.{extension}"),
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("linux-x86_64")
            .stage_from_path(&manifest, &source)
            .expect("stage")
    }

    #[test]
    fn runs_dpkg_for_a_deb() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone());

        assert_eq!(
            installer.install(&staged).expect("install"),
            LinuxPackageKind::Deb
        );
        assert_eq!(
            runner.calls(),
            [(
                PathBuf::from("/usr/bin/dpkg"),
                vec![OsString::from("-i"), staged.path().as_os_str().to_owned()]
            )]
        );
    }

    #[test]
    fn runs_rpm_for_an_rpm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "rpm");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone());

        assert_eq!(
            installer.install(&staged).expect("install"),
            LinuxPackageKind::Rpm
        );
        assert_eq!(
            runner.calls(),
            [(
                PathBuf::from("/usr/bin/rpm"),
                vec![OsString::from("-U"), staged.path().as_os_str().to_owned()]
            )]
        );
    }

    #[test]
    fn reports_package_manager_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        assert!(matches!(
            LinuxPackageInstaller::with_runner(MockRunner::with_code(1)).install(&staged),
            Err(UpdateError::InstallerFailed { code: Some(1), .. })
        ));
    }

    #[test]
    fn rejects_unknown_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "AppImage");
        assert!(matches!(
            LinuxPackageInstaller::with_runner(MockRunner::with_code(0)).install(&staged),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }
}
