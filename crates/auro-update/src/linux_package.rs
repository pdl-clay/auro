// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux `.deb` and `.rpm` package installation.
//!
//! The package manager owns the transaction. The helper runs it shell-free from an absolute path;
//! elevation is the caller's responsibility and a permission failure is reported as
//! [`UpdateError::InstallerFailed`].

use std::ffi::OsString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process;

use crate::{InstallerRunner, StagedUpdate, SystemInstallerRunner, UpdateError};

#[cfg(unix)]
const PKEXEC: &str = "/usr/bin/pkexec";

/// How a package-manager command obtains root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum LinuxElevation {
    /// Run the package manager as the current user.
    #[default]
    None,
    /// Authenticate through polkit and run the package manager as root with `pkexec`.
    Pkexec,
}

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
    elevation: LinuxElevation,
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
            elevation: LinuxElevation::None,
        }
    }

    /// Requests polkit elevation through `pkexec` before running the package manager.
    ///
    /// The helper passes the opened staged artifact through `/proc/<pid>/fd/<fd>` so the package
    /// manager does not reopen a path that can be replaced after verification.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn elevated(mut self) -> Self {
        self.elevation = LinuxElevation::Pkexec;
        self
    }

    /// Creates an installer with an explicit elevation policy.
    pub fn with_elevation(
        runner: impl InstallerRunner + 'static,
        elevation: LinuxElevation,
    ) -> Self {
        LinuxPackageInstaller {
            runner: Box::new(runner),
            elevation,
        }
    }

    /// Runs `dpkg -i` or `rpm -U` on the staged package.
    ///
    /// Without elevation a permission failure is reported as `InstallerFailed`. With
    /// [`LinuxElevation::Pkexec`], a non-zero `pkexec` result (including a denied polkit prompt) is
    /// [`UpdateError::ElevationFailed`]; a missing `pkexec` executable surfaces as an I/O error.
    pub fn install(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError> {
        self.execute(staged, PackageOperation::Install)
    }

    /// Installs a caller-supplied older package as a rollback.
    ///
    /// `.deb` uses `dpkg --force-downgrade -i`; `.rpm` uses `rpm -U --oldpackage`. Auro does not
    /// obtain or verify the older package beyond the staged artifact contract; the caller must
    /// provide an authenticated, already-staged package. The package manager owns dependency and
    /// transaction policy.
    pub fn rollback(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError> {
        self.execute(staged, PackageOperation::Rollback)
    }

    fn execute(
        &self,
        staged: &StagedUpdate,
        operation: PackageOperation,
    ) -> Result<LinuxPackageKind, UpdateError> {
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
        let (program, args, held) = match self.elevation {
            LinuxElevation::None => {
                let (program, args) = command_for(kind, path, operation);
                (program, args, None)
            }
            LinuxElevation::Pkexec => {
                #[cfg(unix)]
                {
                    // Keep the descriptor open until the privileged child opens the `/proc` path.
                    // SECURITY: this avoids reopening a user-replaceable path after verification.
                    let artifact = File::open(path)?;
                    let descriptor = PathBuf::from(format!(
                        "/proc/{}/fd/{}",
                        process::id(),
                        artifact.as_raw_fd()
                    ));
                    let (package, package_args) = command_for(kind, &descriptor, operation);
                    let mut args = Vec::with_capacity(package_args.len() + 1);
                    args.push(package.into_os_string());
                    args.extend(package_args);
                    (PathBuf::from(PKEXEC), args, Some(artifact))
                }
                #[cfg(not(unix))]
                {
                    return Err(UpdateError::UnsupportedInstaller {
                        path: path.to_path_buf(),
                        expected: "Unix pkexec",
                    });
                }
            }
        };
        let code = self.runner.run(&program, &args)?;
        drop(held);
        if code == Some(0) {
            Ok(kind)
        } else if self.elevation == LinuxElevation::Pkexec {
            Err(UpdateError::ElevationFailed { program, code })
        } else {
            Err(UpdateError::InstallerFailed {
                path: path.to_path_buf(),
                code,
            })
        }
    }
}

#[derive(Clone, Copy)]
enum PackageOperation {
    Install,
    Rollback,
}

fn command_for(
    kind: LinuxPackageKind,
    artifact: &Path,
    operation: PackageOperation,
) -> (PathBuf, Vec<OsString>) {
    match (kind, operation) {
        (LinuxPackageKind::Deb, PackageOperation::Install) => (
            PathBuf::from("/usr/bin/dpkg"),
            vec![OsString::from("-i"), artifact.as_os_str().to_owned()],
        ),
        (LinuxPackageKind::Deb, PackageOperation::Rollback) => (
            PathBuf::from("/usr/bin/dpkg"),
            vec![
                OsString::from("--force-downgrade"),
                OsString::from("-i"),
                artifact.as_os_str().to_owned(),
            ],
        ),
        (LinuxPackageKind::Rpm, PackageOperation::Install) => (
            PathBuf::from("/usr/bin/rpm"),
            vec![OsString::from("-U"), artifact.as_os_str().to_owned()],
        ),
        (LinuxPackageKind::Rpm, PackageOperation::Rollback) => (
            PathBuf::from("/usr/bin/rpm"),
            vec![
                OsString::from("-U"),
                OsString::from("--oldpackage"),
                artifact.as_os_str().to_owned(),
            ],
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
    fn rollback_rpm_uses_oldpackage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "rpm");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone());

        assert_eq!(
            installer.rollback(&staged).expect("rollback"),
            LinuxPackageKind::Rpm
        );
        assert_eq!(
            runner.calls(),
            [(
                PathBuf::from("/usr/bin/rpm"),
                vec![
                    OsString::from("-U"),
                    OsString::from("--oldpackage"),
                    staged.path().as_os_str().to_owned()
                ]
            )]
        );
    }

    #[test]
    fn rollback_deb_forces_a_downgrade() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone());

        assert_eq!(
            installer.rollback(&staged).expect("rollback"),
            LinuxPackageKind::Deb
        );
        assert_eq!(
            runner.calls(),
            [(
                PathBuf::from("/usr/bin/dpkg"),
                vec![
                    OsString::from("--force-downgrade"),
                    OsString::from("-i"),
                    staged.path().as_os_str().to_owned()
                ]
            )]
        );
    }

    #[test]
    fn rollback_reports_package_manager_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "rpm");
        let installer = LinuxPackageInstaller::with_runner(MockRunner::with_code(2));
        assert!(matches!(
            installer.rollback(&staged),
            Err(UpdateError::InstallerFailed { code: Some(2), .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn elevated_rollback_uses_the_held_descriptor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "rpm");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone()).elevated();

        installer.rollback(&staged).expect("rollback");
        let calls = runner.calls();
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/pkexec"));
        assert_eq!(
            &calls[0].1[..3],
            [
                OsString::from("/usr/bin/rpm"),
                OsString::from("-U"),
                OsString::from("--oldpackage")
            ]
        );
        assert!(
            calls[0].1[3]
                .to_string_lossy()
                .starts_with(&format!("/proc/{}/fd/", std::process::id()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn elevated_deb_runs_pkexec_dpkg_with_a_held_descriptor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone()).elevated();

        assert_eq!(
            installer.install(&staged).expect("install"),
            LinuxPackageKind::Deb
        );
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/pkexec"));
        let args = &calls[0].1;
        assert_eq!(args[0], OsString::from("/usr/bin/dpkg"));
        assert_eq!(args[1], OsString::from("-i"));
        assert!(
            args[2]
                .to_string_lossy()
                .starts_with(&format!("/proc/{}/fd/", std::process::id())),
            "elevated package path must be the helper's held descriptor: {args:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn elevated_rpm_runs_pkexec_rpm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "rpm");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_runner(runner.clone()).elevated();

        assert_eq!(
            installer.install(&staged).expect("install"),
            LinuxPackageKind::Rpm
        );
        let calls = runner.calls();
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/pkexec"));
        assert_eq!(
            &calls[0].1[..2],
            [OsString::from("/usr/bin/rpm"), OsString::from("-U")]
        );
    }

    #[test]
    fn reports_pkexec_authorization_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        let installer = LinuxPackageInstaller::with_runner(MockRunner::with_code(126)).elevated();
        assert!(matches!(
            installer.install(&staged),
            Err(UpdateError::ElevationFailed {
                code: Some(126),
                ..
            })
        ));
    }

    #[test]
    fn explicit_none_elevation_runs_the_direct_installer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "deb");
        let runner = MockRunner::with_code(0);
        let installer = LinuxPackageInstaller::with_elevation(runner.clone(), LinuxElevation::None);
        installer.install(&staged).expect("install");
        assert_eq!(runner.calls()[0].0, PathBuf::from("/usr/bin/dpkg"));
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
