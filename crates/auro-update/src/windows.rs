// SPDX-License-Identifier: MIT OR Apache-2.0

//! Windows MSI and NSIS installer execution.
//!
//! The installer is already verified and staged by [`crate::Updater`]. This module only runs it
//! silently and classifies its exit code; elevation and restart orchestration are the caller's
//! responsibility.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{InstallerRunner, StagedUpdate, SystemInstallerRunner, UpdateError};

/// The Windows installer format detected from the staged artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsInstallerKind {
    /// Windows Installer package.
    Msi,
    /// Assumed NSIS setup executable. A non-NSIS `.exe` is run with `/S` and may misbehave.
    Nsis,
}

impl WindowsInstallerKind {
    /// Detects the installer format from `.msi` or `.exe`.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("msi") {
            Some(Self::Msi)
        } else if extension.eq_ignore_ascii_case("exe") {
            Some(Self::Nsis)
        } else {
            None
        }
    }
}

/// The result of running a Windows installer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowsInstallOutcome {
    /// Installer format that ran.
    pub kind: WindowsInstallerKind,
    /// The installer asked for or initiated a reboot (`3010`/`1641`).
    pub restart_required: bool,
}

/// Runs staged MSI and NSIS installers silently.
pub struct WindowsInstaller {
    runner: Box<dyn InstallerRunner>,
}

impl Default for WindowsInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsInstaller {
    /// Creates an installer that runs commands with the operating system.
    pub fn new() -> Self {
        Self::with_runner(SystemInstallerRunner)
    }

    /// Creates an installer with a test or custom command runner.
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self {
        WindowsInstaller {
            runner: Box::new(runner),
        }
    }

    /// Runs the staged installer and classifies its exit code.
    ///
    /// `0` succeeds. `3010` (reboot required) and `1641` (reboot initiated) succeed and report
    /// `restart_required`. Every other code (including no code because the process was
    /// terminated) is `InstallerFailed`.
    pub fn install(&self, staged: &StagedUpdate) -> Result<WindowsInstallOutcome, UpdateError> {
        let path = staged.path();
        if !path.is_file() {
            return Err(UpdateError::StagedArtifactMissing {
                path: path.to_path_buf(),
            });
        }
        let kind = WindowsInstallerKind::from_path(path).ok_or_else(|| {
            UpdateError::UnsupportedInstaller {
                path: path.to_path_buf(),
                expected: ".msi or .exe",
            }
        })?;
        let (program, args) = command_for(kind, path);
        let code = self.runner.run(&program, &args)?;
        let restart_required = matches!(code, Some(3010 | 1641));
        if code == Some(0) || restart_required {
            Ok(WindowsInstallOutcome {
                kind,
                restart_required,
            })
        } else {
            Err(UpdateError::InstallerFailed {
                path: path.to_path_buf(),
                code,
            })
        }
    }
}

fn msiexec_path() -> PathBuf {
    msiexec_path_from(std::env::var_os("SystemRoot"))
}

fn msiexec_path_from(system_root: Option<OsString>) -> PathBuf {
    let root = system_root
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join("msiexec.exe")
}

fn command_for(kind: WindowsInstallerKind, artifact: &Path) -> (PathBuf, Vec<OsString>) {
    match kind {
        WindowsInstallerKind::Msi => (
            msiexec_path(),
            vec![
                OsString::from("/i"),
                artifact.as_os_str().to_owned(),
                OsString::from("/qn"),
                OsString::from("/norestart"),
            ],
        ),
        WindowsInstallerKind::Nsis => (artifact.to_path_buf(), vec![OsString::from("/S")]),
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

        fn calls(&self) -> Vec<(PathBuf, Vec<OsString>)> {
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
        let source = temp.path().join(format!("installer.{extension}"));
        std::fs::write(&source, b"installer bytes").expect("artifact");
        let manifest = Manifest::for_artifact(
            &source,
            "1.2.3",
            "windows-x86_64",
            format!("https://example.test/installer.{extension}"),
            "",
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");
        Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("windows-x86_64")
            .stage_from_path(&manifest, &source)
            .expect("stage")
    }

    #[test]
    fn resolves_msiexec_without_a_path_lookup() {
        assert_eq!(
            msiexec_path_from(None),
            PathBuf::from(r"C:\Windows")
                .join("System32")
                .join("msiexec.exe")
        );
        assert_eq!(
            msiexec_path_from(Some(OsString::from(r"D:\Windows"))),
            PathBuf::from(r"D:\Windows")
                .join("System32")
                .join("msiexec.exe")
        );
    }

    #[test]
    fn runs_msi_installer_silently() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "msi");
        let runner = MockRunner::with_code(0);
        let installer = WindowsInstaller::with_runner(runner.clone());

        let outcome = installer.install(&staged).expect("install");
        assert_eq!(outcome.kind, WindowsInstallerKind::Msi);
        assert!(!outcome.restart_required);
        assert_eq!(
            runner.calls(),
            [(
                msiexec_path(),
                vec![
                    OsString::from("/i"),
                    staged.path().as_os_str().to_owned(),
                    OsString::from("/qn"),
                    OsString::from("/norestart")
                ]
            )]
        );
    }

    #[test]
    fn runs_nsis_installer_silently() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "exe");
        let runner = MockRunner::with_code(0);
        let installer = WindowsInstaller::with_runner(runner.clone());

        let outcome = installer.install(&staged).expect("install");
        assert_eq!(outcome.kind, WindowsInstallerKind::Nsis);
        assert_eq!(
            runner.calls(),
            [(staged.path().to_path_buf(), vec![OsString::from("/S")])]
        );
    }

    #[test]
    fn maps_reboot_required_and_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "msi");

        for code in [3010, 1641] {
            let reboot = WindowsInstaller::with_runner(MockRunner::with_code(code))
                .install(&staged)
                .expect("reboot");
            assert!(reboot.restart_required);
        }

        let failed = WindowsInstaller::with_runner(MockRunner::with_code(1603))
            .install(&staged)
            .expect_err("failure");
        assert!(matches!(
            failed,
            UpdateError::InstallerFailed {
                code: Some(1603),
                ..
            }
        ));
    }

    #[test]
    fn reports_a_terminated_installer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "msi");
        let runner = MockRunner {
            code: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        };

        assert!(matches!(
            WindowsInstaller::with_runner(runner).install(&staged),
            Err(UpdateError::InstallerFailed { code: None, .. })
        ));
    }

    #[test]
    fn reports_a_missing_staged_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "msi");
        std::fs::remove_file(staged.path()).expect("remove staged");

        assert!(matches!(
            WindowsInstaller::with_runner(MockRunner::with_code(0)).install(&staged),
            Err(UpdateError::StagedArtifactMissing { .. })
        ));
    }

    #[test]
    fn rejects_unknown_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let staged = staged(&temp, "AppImage");
        assert!(matches!(
            WindowsInstaller::with_runner(MockRunner::with_code(0)).install(&staged),
            Err(UpdateError::UnsupportedInstaller { .. })
        ));
    }
}
