// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS notarization submission, stapling and Gatekeeper verification.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use auro_core::AuroError;

const XCRUN: &str = "/usr/bin/xcrun";
const DITTO: &str = "/usr/bin/ditto";
const SPCTL: &str = "/usr/sbin/spctl";

/// Runs an `xcrun`/`ditto`/`spctl` subcommand.
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

/// Submits an artifact to `notarytool`, staples its ticket and can verify the result locally.
pub struct MacNotarizer {
    runner: Box<dyn NotarizeRunner>,
}

impl Default for MacNotarizer {
    fn default() -> Self {
        Self::new()
    }
}

impl MacNotarizer {
    /// Creates a notarizer that runs the macOS tools with the operating system.
    pub fn new() -> Self {
        Self::with_runner(SystemNotarizeRunner)
    }

    /// Creates a notarizer with a test or custom command runner.
    pub fn with_runner(runner: impl NotarizeRunner + 'static) -> Self {
        MacNotarizer {
            runner: Box::new(runner),
        }
    }

    /// Submits `artifact` using a keychain profile, staples the ticket and validates it.
    ///
    /// The profile must already exist in the login keychain (create it with
    /// `xcrun notarytool store-credentials`). Auro never handles an Apple ID password in argv or
    /// environment variables. A `.app` bundle is zipped with `ditto` for submission and the
    /// ticket is stapled to the bundle; `.dmg` and `.pkg` are submitted directly. A bare `.zip`
    /// cannot be stapled and is rejected unless `skip_staple` is set.
    pub fn notarize(
        &self,
        artifact: &Path,
        profile: &str,
        skip_staple: bool,
    ) -> Result<(), AuroError> {
        let kind = artifact_kind(artifact)?;
        if !artifact.exists() {
            return Err(AuroError::bundle(format!(
                "notarization artifact `{}` is missing",
                artifact.display()
            )));
        }
        if kind == ArtifactKind::AppBundle && !artifact.is_dir() {
            return Err(AuroError::bundle(format!(
                "macOS app bundle `{}` is not a directory",
                artifact.display()
            )));
        }
        if profile.trim().is_empty() {
            return Err(AuroError::bundle("notarization profile is empty"));
        }
        if kind == ArtifactKind::Zip && !skip_staple {
            return Err(AuroError::bundle(
                "a zipped `.app` cannot be stapled; notarize the `.app` bundle or pass --skip-staple",
            ));
        }

        match kind {
            ArtifactKind::AppBundle => self.notarize_app(artifact, profile, skip_staple),
            ArtifactKind::Dmg | ArtifactKind::Pkg | ArtifactKind::Zip => {
                self.notarize_file(artifact, profile, skip_staple)
            }
        }
    }

    /// Checks a notarized artifact's stapled ticket and Gatekeeper assessment.
    ///
    /// This is runtime verification of a release artifact: `xcrun stapler validate` proves the
    /// ticket is attached, and `spctl --assess` asks Gatekeeper whether the artifact is accepted
    /// for its type. It runs no network request and does not resubmit anything.
    pub fn verify(&self, artifact: &Path) -> Result<(), AuroError> {
        let kind = artifact_kind(artifact)?;
        if !artifact.exists() {
            return Err(AuroError::bundle(format!(
                "verification artifact `{}` is missing",
                artifact.display()
            )));
        }
        if kind == ArtifactKind::AppBundle && !artifact.is_dir() {
            return Err(AuroError::bundle(format!(
                "macOS app bundle `{}` is not a directory",
                artifact.display()
            )));
        }
        if !matches!(
            kind,
            ArtifactKind::AppBundle | ArtifactKind::Dmg | ArtifactKind::Pkg
        ) {
            return Err(AuroError::bundle(
                "only `.app`, `.dmg` and `.pkg` artifacts can be verified",
            ));
        }
        let (program, args) = validate_command(artifact);
        check(self.runner.run(&program, &args)?, "stapler validation")?;
        let (program, args) = assess_command(kind, artifact);
        check(self.runner.run(&program, &args)?, "Gatekeeper assessment")?;
        Ok(())
    }

    /// Notarizes and verifies every macOS artifact in `artifacts`.
    ///
    /// Non-macOS formats are ignored, so a cross-platform bundle run can keep the profile
    /// configured. App bundles are processed before disk images. Because `auro bundle` receives
    /// the artifacts after the bundler has built them, a disk image already contains the app as
    /// it was before stapling; this post-bundle path is therefore a release convenience, not a
    /// replacement for notarizing the app before packaging when an offline first launch matters.
    pub fn notarize_artifacts(
        &self,
        artifacts: &[PathBuf],
        profile: &str,
    ) -> Result<(), AuroError> {
        let mut selected: Vec<&Path> = artifacts
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| {
                matches!(
                    artifact_kind(path),
                    Ok(ArtifactKind::AppBundle | ArtifactKind::Dmg | ArtifactKind::Pkg)
                )
            })
            .collect();
        selected.sort_by_key(|path| match artifact_kind(path) {
            Ok(ArtifactKind::AppBundle) => 0,
            Ok(ArtifactKind::Dmg) => 1,
            Ok(ArtifactKind::Pkg) => 2,
            _ => 3,
        });
        for artifact in selected {
            self.notarize(artifact, profile, false)?;
            self.verify(artifact)?;
        }
        Ok(())
    }

    fn notarize_file(
        &self,
        artifact: &Path,
        profile: &str,
        skip_staple: bool,
    ) -> Result<(), AuroError> {
        let (program, args) = submit_command(artifact, profile);
        check(self.runner.run(&program, &args)?, "notarytool submission")?;
        if skip_staple {
            return Ok(());
        }
        let (program, args) = staple_command(artifact);
        check(self.runner.run(&program, &args)?, "stapler")?;
        let (program, args) = validate_command(artifact);
        check(self.runner.run(&program, &args)?, "stapler validation")?;
        Ok(())
    }

    fn notarize_app(&self, app: &Path, profile: &str, skip_staple: bool) -> Result<(), AuroError> {
        let temporary = tempfile::Builder::new()
            .prefix("auro-notarize-")
            .tempdir()?;
        let name = app
            .file_name()
            .ok_or_else(|| AuroError::bundle("app bundle has no file name"))?;
        let mut zip_name = name.to_os_string();
        zip_name.push(".zip");
        let archive = temporary.path().join(zip_name);

        let (program, args) = zip_command(app, &archive);
        check(self.runner.run(&program, &args)?, "ditto archive")?;
        let (program, args) = submit_command(&archive, profile);
        check(self.runner.run(&program, &args)?, "notarytool submission")?;
        if skip_staple {
            return Ok(());
        }
        let (program, args) = staple_command(app);
        check(self.runner.run(&program, &args)?, "stapler")?;
        let (program, args) = validate_command(app);
        check(self.runner.run(&program, &args)?, "stapler validation")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    AppBundle,
    Dmg,
    Pkg,
    Zip,
}

fn artifact_kind(artifact: &Path) -> Result<ArtifactKind, AuroError> {
    let extension = artifact
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("app") {
        Ok(ArtifactKind::AppBundle)
    } else if extension.eq_ignore_ascii_case("dmg") {
        Ok(ArtifactKind::Dmg)
    } else if extension.eq_ignore_ascii_case("pkg") {
        Ok(ArtifactKind::Pkg)
    } else if extension.eq_ignore_ascii_case("zip") {
        Ok(ArtifactKind::Zip)
    } else {
        Err(AuroError::bundle(format!(
            "unsupported notarization artifact `{}`; expected `.app`, `.dmg`, `.pkg` or `.zip`",
            artifact.display()
        )))
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

fn validate_command(artifact: &Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from(XCRUN),
        vec![
            OsString::from("stapler"),
            OsString::from("validate"),
            artifact.as_os_str().to_owned(),
        ],
    )
}

fn zip_command(app: &Path, archive: &Path) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from(DITTO),
        vec![
            OsString::from("-c"),
            OsString::from("-k"),
            OsString::from("--keepParent"),
            app.as_os_str().to_owned(),
            archive.as_os_str().to_owned(),
        ],
    )
}

fn assess_command(kind: ArtifactKind, artifact: &Path) -> (PathBuf, Vec<OsString>) {
    let mut args = vec![
        OsString::from("--assess"),
        OsString::from("--type"),
        OsString::from(match kind {
            ArtifactKind::AppBundle => "execute",
            ArtifactKind::Dmg => "open",
            ArtifactKind::Pkg => "install",
            ArtifactKind::Zip => "open",
        }),
    ];
    if kind == ArtifactKind::Dmg {
        args.push(OsString::from("--context"));
        args.push(OsString::from("context:primary-signature"));
    }
    args.push(OsString::from("--verbose=4"));
    args.push(artifact.as_os_str().to_owned());
    (PathBuf::from(SPCTL), args)
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

    fn app_bundle(temp: &tempfile::TempDir) -> PathBuf {
        let path = temp.path().join("Demo.app");
        std::fs::create_dir(&path).expect("app bundle");
        path
    }

    #[test]
    fn submits_staples_and_validates_with_xcrun() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(0), Some(0)]);
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
                (
                    PathBuf::from("/usr/bin/xcrun"),
                    vec![
                        OsString::from("stapler"),
                        OsString::from("validate"),
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
    fn reports_stapling_failure_and_does_not_validate() {
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
    fn reports_stapler_validation_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(0), Some(1)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        assert!(
            notarizer
                .notarize(&artifact, "auro-release", false)
                .is_err()
        );
        assert_eq!(runner.calls().len(), 3);
    }

    #[test]
    fn zips_and_notarizes_an_app_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app = app_bundle(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(0), Some(0), Some(0)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        notarizer
            .notarize(&app, "auro-release", false)
            .expect("notarize");

        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/ditto"));
        assert_eq!(
            &calls[0].1[..3],
            [
                OsString::from("-c"),
                OsString::from("-k"),
                OsString::from("--keepParent")
            ]
        );
        assert_eq!(calls[0].1[3].as_os_str(), app.as_os_str());
        let archive = PathBuf::from(&calls[0].1[4]);
        assert!(
            archive.to_string_lossy().ends_with("Demo.app.zip"),
            "archive: {}",
            archive.display()
        );

        assert_eq!(calls[1].0, PathBuf::from("/usr/bin/xcrun"));
        assert_eq!(
            &calls[1].1[..2],
            [OsString::from("notarytool"), OsString::from("submit")]
        );
        assert_eq!(calls[1].1[2].as_os_str(), archive.as_os_str());

        assert_eq!(
            &calls[2].1[..2],
            [OsString::from("stapler"), OsString::from("staple")]
        );
        assert_eq!(calls[2].1[2].as_os_str(), app.as_os_str());
        assert_eq!(
            &calls[3].1[..2],
            [OsString::from("stapler"), OsString::from("validate")]
        );
        assert_eq!(calls[3].1[2].as_os_str(), app.as_os_str());
    }

    #[test]
    fn a_zip_needs_skip_staple_and_then_submits_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive = temp.path().join("Demo.app.zip");
        std::fs::write(&archive, b"zip").expect("archive");
        let runner = MockRunner::with_codes(&[Some(0)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        assert!(notarizer.notarize(&archive, "auro-release", false).is_err());
        assert!(runner.calls().is_empty());

        notarizer
            .notarize(&archive, "auro-release", true)
            .expect("submit");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn verifies_app_pkg_and_dmg_with_stapler_and_gatekeeper() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app = app_bundle(&temp);
        let pkg = artifact(&temp);
        let dmg = temp.path().join("Demo.dmg");
        std::fs::write(&dmg, b"disk image").expect("dmg");

        for (artifact, assess) in [
            (app, vec!["--assess", "--type", "execute", "--verbose=4"]),
            (pkg, vec!["--assess", "--type", "install", "--verbose=4"]),
            (
                dmg,
                vec![
                    "--assess",
                    "--type",
                    "open",
                    "--context",
                    "context:primary-signature",
                    "--verbose=4",
                ],
            ),
        ] {
            let runner = MockRunner::with_codes(&[Some(0), Some(0)]);
            let notarizer = MacNotarizer::with_runner(runner.clone());
            notarizer.verify(&artifact).expect("verify");
            let calls = runner.calls();
            assert_eq!(calls.len(), 2);
            assert_eq!(
                calls[0].1,
                [
                    OsString::from("stapler"),
                    OsString::from("validate"),
                    artifact.as_os_str().to_owned()
                ]
            );
            assert_eq!(calls[1].0, PathBuf::from("/usr/sbin/spctl"));
            let mut expected: Vec<OsString> = assess.into_iter().map(OsString::from).collect();
            expected.push(artifact.as_os_str().to_owned());
            assert_eq!(calls[1].1, expected);
        }
    }

    #[test]
    fn reports_gatekeeper_rejection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = artifact(&temp);
        let runner = MockRunner::with_codes(&[Some(0), Some(1)]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        let error = notarizer.verify(&artifact).expect_err("rejected");
        assert!(error.to_string().contains("Gatekeeper"), "{error}");
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn notarizes_and_verifies_macos_artifacts_and_skips_other_formats() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app = app_bundle(&temp);
        let dmg = temp.path().join("Demo.dmg");
        std::fs::write(&dmg, b"disk image").expect("dmg");
        let deb = temp.path().join("demo.deb");
        std::fs::write(&deb, b"package").expect("deb");
        let runner = MockRunner::with_codes(&[
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
        ]);
        let notarizer = MacNotarizer::with_runner(runner.clone());

        notarizer
            .notarize_artifacts(&[dmg.clone(), deb.clone(), app.clone()], "auro-release")
            .expect("notarize artifacts");

        let calls = runner.calls();
        assert_eq!(calls.len(), 11);
        assert_eq!(calls[0].0, PathBuf::from("/usr/bin/ditto"));
        assert_eq!(calls[0].1[3].as_os_str(), app.as_os_str());
        assert_eq!(calls[1].1[0], OsString::from("notarytool"));
        assert_eq!(calls[2].1[0], OsString::from("stapler"));
        assert_eq!(calls[3].1[0], OsString::from("stapler"));
        assert_eq!(calls[4].0, PathBuf::from("/usr/bin/xcrun"));
        assert_eq!(calls[5].0, PathBuf::from("/usr/sbin/spctl"));
        assert_eq!(calls[6].1[2].as_os_str(), dmg.as_os_str());
        assert_eq!(calls[7].1[0], OsString::from("stapler"));
        assert_eq!(calls[8].1[0], OsString::from("stapler"));
        assert_eq!(calls[9].0, PathBuf::from("/usr/bin/xcrun"));
        assert_eq!(calls[10].0, PathBuf::from("/usr/sbin/spctl"));
        assert!(
            calls
                .iter()
                .all(|(_, args)| !args.iter().any(|arg| arg.as_os_str() == deb.as_os_str())),
            "a .deb must not be sent to notarization"
        );
    }

    #[test]
    fn rejects_a_missing_artifact_empty_profile_or_unknown_extension() {
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
        assert!(
            notarizer
                .notarize(&temp.path().join("Demo.tar"), "auro-release", false)
                .is_err()
        );
        assert!(notarizer.verify(&temp.path().join("Demo.tar")).is_err());
        assert!(runner.calls().is_empty());
    }
}
