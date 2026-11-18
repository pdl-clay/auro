// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS notarization submission and ticket stapling.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use auro_core::AuroError;

const XCRUN: &str = "/usr/bin/xcrun";

/// Runs an `xcrun` subcommand.
pub trait NotarizeRunner {
    /// Runs `program` with `args`, with no shell interpretation.
    fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, AuroError>;
}

/// Runs notarization commands with the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNotarizeRunner;

impl NotarizeRunner for SystemNotarizeRunner {
    fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, AuroError> {
        let status = std::process::Command::new(program).args(args).status()?;
        Ok(status.code())
    }
}

/// Submits an artifact to `notarytool` and optionally staples its ticket.
pub struct MacNotarizer {
    runner: Box<dyn NotarizeRunner>,
}

impl Default for MacNotarizer {
    fn default() -> Self {
        Self::new()
    }
}

impl MacNotarizer {
    /// Creates a notarizer that runs `xcrun` with the operating system.
    pub fn new() -> Self {
        Self::with_runner(SystemNotarizeRunner)
    }

    /// Creates a notarizer with a test or custom command runner.
    pub fn with_runner(runner: impl NotarizeRunner + 'static) -> Self {
        MacNotarizer {
            runner: Box::new(runner),
        }
    }

    /// Submits `artifact` using a keychain profile and optionally staples the ticket.
    ///
    /// The profile must already exist in the login keychain (create it with
    /// `xcrun notarytool store-credentials`). Auro never handles an Apple ID password in argv or
    /// environment variables. The submitted artifact is usually a `.pkg`, `.dmg` or a zipped
    /// `.app`; notarytool decides whether the artifact format is accepted.
    pub fn notarize(
        &self,
        artifact: &Path,
        profile: &str,
        skip_staple: bool,
    ) -> Result<(), AuroError> {
        if !artifact.is_file() {
            return Err(AuroError::bundle(format!(
                "notarization artifact `{}` is missing",
                artifact.display()
            )));
        }
        if profile.trim().is_empty() {
            return Err(AuroError::bundle("notarization profile is empty"));
        }

        let (program, args) = submit_command(artifact, profile);
        check(self.runner.run(&program, &args)?, "notarytool submission")?;
        if skip_staple {
            return Ok(());
        }
        let (program, args) = staple_command(artifact);
        check(self.runner.run(&program, &args)?, "stapler")?;
        Ok(())
    }
}

fn check(code: Option<i32>, action: &str) -> Result<(), AuroError> {
    if code == Some(0) {
        Ok(())
    } else {
        Err(AuroError::bundle(format!(
            "{action} failed with exit code {code:?}"
        )))
    }
}

fn submit_command(artifact: &Path, profile: &str) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from(XCRUN),
        vec![
            OsString::from("notarytool"),
            OsString::from("submit"),
            artifact.as_os_str().to_owned(),
            OsString::from("--keychain-profile"),
            OsString::from(profile),
            OsString::from("--wait"),
        ],
    )
}

fn staple_command(artifact: &Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from(XCRUN),
        vec![
            OsString::from("stapler"),
            OsString::from("staple"),
            artifact.as_os_str().to_owned(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use auro_core::AuroError;

    use super::*;

    type Call = (PathBuf, Vec<OsString>);

    #[derive(Clone, Default)]
    struct MockRunner {
        codes: Arc<Mutex<Vec<Option<i32>>>>,
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl MockRunner {
        fn with_codes(codes: &[Option<i32>]) -> Self {
            MockRunner {
                codes: Arc::new(Mutex::new(codes.to_vec())),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("runner").clone()
        }
    }

    impl NotarizeRunner for MockRunner {
        fn run(&self, program: &Path, args: &[OsString]) -> Result<Option<i32>, AuroError> {
            self.calls
                .lock()
                .expect("runner")
                .push((program.to_path_buf(), args.to_vec()));
            Ok(self.codes.lock().expect("codes").remove(0))
        }
    }

    fn artifact(temp: &tempfile::TempDir) -> PathBuf {
        let path = temp.path().join("Demo.pkg");
        std::fs::write(&path, b"package").expect("artifact");
        path
    }

    #[test]
    fn submits_and_staples_with_xcrun() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(0)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        notarizer
            .notarize(&artifact, "auro-release", false)
            .expect("notarize");
        assert_eq!(
            runner.calls(),
            [
                (
                    PathBuf::from("/usr/bin/xcrun"),
                    vec![
                        OsString::from("notarytool"),
                        OsString::from("submit"),
                        artifact.as_os_str().to_owned(),
                        OsString::from("--keychain-profile"),
                        OsString::from("auro-release"),
                        OsString::from("--wait"),
                    ]
                ),
                (
                    PathBuf::from("/usr/bin/xcrun"),
                    vec![
                        OsString::from("stapler"),
                        OsString::from("staple"),
                        artifact.as_os_str().to_owned(),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn can_skip_stapling() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        notarizer
            .notarize(&artifact, "auro-release", true)
            .expect("notarize");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn reports_submission_failure_and_does_not_staple() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(1)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        assert!(
            notarizer
                .notarize(&artifact, "auro-release", false)
                .is_err()
        );
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn reports_stapling_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(1)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        assert!(
            notarizer
                .notarize(&artifact, "auro-release", false)
                .is_err()
        );
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn rejects_a_missing_artifact_or_empty_profile() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runner = MockRunner::with_codes(&[]);
        let notarizer = MacNotarizer::with_runner(runner.clone());
        assert!(
            notarizer
                .notarize(&temp.path().join("missing.pkg"), "auro-release", false)
                .is_err()
        );
        let artifact = artifact(&temp);
        assert!(notarizer.notarize(&artifact, " ", false).is_err());
        assert!(runner.calls().is_empty());
    }
}
