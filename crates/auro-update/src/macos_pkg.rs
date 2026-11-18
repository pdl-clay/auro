// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS `.pkg` package installation.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{InstallerRunner, StagedUpdate, SystemInstallerRunner, UpdateError};

const INSTALLER: &str = "/usr/sbin/installer";
const OSASCRIPT: &str = "/usr/bin/osascript";

/// How a macOS package-manager command obtains administrator rights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MacPkgElevation {
    /// Run `/usr/sbin/installer` as the current user.
    #[default]
    None,
    /// Prompt through AppleScript's administrator privileges and run the installer as root.
    Osascript,
}

/// Runs a staged macOS `.pkg` through `/usr/sbin/installer`.
pub struct MacPkgInstaller {
    runner: Box<dyn InstallerRunner>,
    elevation: MacPkgElevation,
}

impl Default for MacPkgInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl MacPkgInstaller {
    /// Creates an installer that runs `/usr/sbin/installer` with the operating system.
    pub fn new() -> Self {
        Self::with_runner(SystemInstallerRunner)
    }

    /// Creates an installer with a test or custom command runner.
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self {
        MacPkgInstaller {
            runner: Box::new(runner),
            elevation: MacPkgElevation::None,
        }
    }

    /// Requests the standard macOS administrator prompt through `osascript`.
    pub fn elevated(mut self) -> Self {
        self.elevation = MacPkgElevation::Osascript;
        self
    }

    /// Creates an installer with an explicit elevation policy.
    pub fn with_elevation(
        runner: impl InstallerRunner + 'static,
        elevation: MacPkgElevation,
    ) -> Self {
        MacPkgInstaller {
            runner: Box::new(runner),
            elevation,
        }
    }

    /// Runs `/usr/sbin/installer -pkg <staged> -target /`.
    ///
    /// The elevated path invokes `osascript` with a fixed AppleScript that uses
    /// `quoted form` for the package path; the package path is an argument, not interpolated into
    /// the script source. A non-zero `osascript` result is `ElevationFailed`; a direct installer
    /// failure is `InstallerFailed`.
    pub fn install(&self, staged: &StagedUpdate) -> Result<(), UpdateError> {
        let path = staged.path();
        if !path.is_file() {
            return Err(UpdateError::StagedArtifactMissing {
                path: path.to_path_buf(),
            });
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pkg"))
        {
            return Err(UpdateError::UnsupportedInstaller {
                path: path.to_path_buf(),
                expected: ".pkg",
            });
        }
        let (program, args) = command_for(path, self.elevation);
        let code = self.runner.run(&program, &args)?;
        if code == Some(0) {
            Ok(())
        } else if self.elevation == MacPkgElevation::Osascript {
            Err(UpdateError::ElevationFailed { program, code })
        } else {
            Err(UpdateError::InstallerFailed {
                path: path.to_path_buf(),
                code,
            })
        }
    }
}

fn command_for(path: &Path, elevation: MacPkgElevation) -> (PathBuf, Vec<OsString>) {
    match elevation {
        MacPkgElevation::None => (
            PathBuf::from(INSTALLER),
            vec![
                OsString::from("-pkg"),
                path.as_os_str().to_owned(),
                OsString::from("-target"),
                OsString::from("/"),
            ],
        ),
        MacPkgElevation::Osascript => (
            PathBuf::from(OSASCRIPT),
            vec![
                OsString::from("-e"),
                OsString::from("on run argv"),
                OsString::from("-e"),
                OsString::from(
                    "do shell script \"/usr/sbin/installer -pkg \" & quoted form of item 1 of argv & \" -target /\" with administrator privileges",
                ),
                OsString::from("-e"),
                OsString::from("end run"),
                path.as_os_str().to_owned(),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use auro_sign::SigningKey;

    use crate::{InstallerRunner, Manifest, StagedUpdate, UpdateError, Updater};

    use super::*;

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

    fn staged(temp: &tempfile::TempDir) -> StagedUpdate {
        let key = SigningKey::generate();
        let source = temp.path().join("Demo.pkg");
        std::fs::write(&source, b"pkg bytes").expect("package");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "macos-aarch64",
            "https://example.test/Demo.pkg",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("macos-aarch64")
            .stage_from_path(&manifest, &source)
            .expect("stage")
    }

    #[test]
    fn runs_the_macos_installer_for_a_pkg() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp);
        let runner = MockRunner::with_code(0);
        let installer = MacPkgInstaller::with_runner(runner.clone());

        installer.install(&staged).expect("install");
        assert_eq!(
            runner.calls(),
            [(
                PathBuf::from("/usr/sbin/installer"),
                vec![
                    OsString::from("-pkg"),
                    staged.path().as_os_str().to_owned(),
                    OsString::from("-target"),
                    OsString::from("/")
                ]
            )]
        );
    }

    #[test]
    fn elevated_pkg_uses_osascript_and_quoted_form() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp);
        let runner = MockRunner::with_code(0);
        let installer = MacPkgInstaller::with_runner(runner.clone()).elevated();

        installer.install(&staged).expect("install");
        let calls = runner.calls();
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/osascript"));
        let args = &calls[0].1;
        assert_eq!(args[0], OsString::from("-e"));
        assert_eq!(args[1], OsString::from("on run argv"));
        assert_eq!(args[2], OsString::from("-e"));
        assert_eq!(
            args[3],
            OsString::from(
                "do shell script \"/usr/sbin/installer -pkg \" & quoted form of item 1 of argv & \" -target /\" with administrator privileges"
            )
        );
        assert_eq!(args[4], OsString::from("-e"));
        assert_eq!(args[5], OsString::from("end run"));
        assert_eq!(args[6], staged.path().as_os_str());
    }

    #[test]
    fn explicit_none_elevation_runs_the_direct_installer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp);
        let runner = MockRunner::with_code(0);
        let installer = MacPkgInstaller::with_elevation(runner.clone(), MacPkgElevation::None);
        installer.install(&staged).expect("install");
        assert_eq!(runner.calls()[0].0, PathBuf::from("/usr/sbin/installer"));
    }

    #[test]
    fn reports_pkg_installer_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp);
        assert!(matches!(
            MacPkgInstaller::with_runner(MockRunner::with_code(1)).install(&staged),
            Err(UpdateError::InstallerFailed { code: Some(1), .. })
        ));
    }

    #[test]
    fn reports_osascript_authorization_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp);
        assert!(matches!(
            MacPkgInstaller::with_runner(MockRunner::with_code(1))
                .elevated()
                .install(&staged),
            Err(UpdateError::ElevationFailed { code: Some(1), .. })
        ));
    }

    #[test]
    fn rejects_unknown_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let source = temp.path().join("Demo.dmg");
        std::fs::write(&source, b"dmg").expect("artifact");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "macos-aarch64",
            "https://example.test/Demo.dmg",
            "",
            "2026-11-18T00:00:00Z",
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
            MacPkgInstaller::with_runner(MockRunner::with_code(0)).install(&staged),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }
}
