// SPDX-License-Identifier: MIT OR Apache-2.0

//! Runtime half of the updater: decide, download, verify and stage.
//!
//! Staging stops before installation. The staged file is content-addressed by the artifact SHA-256,
//! so a retry either reuses a file that still verifies or replaces it. Platform installation and
//! rollback are a separate contract.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use auro_sign::PublicKey;
use semver::Version;
use tempfile::NamedTempFile;

use crate::{Manifest, UpdateError, current_platform};

/// What an offered manifest means relative to the running version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    /// The offered version is the running version.
    UpToDate,
    /// The offered version is newer.
    Available,
    /// The offered version is older. A signed older manifest can be replayed, so the runtime does
    /// not stage it; rollback needs a separate explicit policy.
    Downgrade,
}

/// A verified artifact waiting in the staging directory.
#[derive(Clone, Debug)]
pub struct StagedUpdate {
    manifest: Manifest,
    path: PathBuf,
}

impl StagedUpdate {
    fn new(manifest: Manifest, path: PathBuf) -> Self {
        StagedUpdate { manifest, path }
    }

    /// The signed manifest that describes this artifact.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The verified artifact in the staging directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the staged update and returns its manifest and path.
    pub fn into_parts(self) -> (Manifest, PathBuf) {
        (self.manifest, self.path)
    }
}

/// Checks signed manifests and stages their artifacts.
///
/// Construct one per app, with the public key pinned in the app and the running version. The
/// staging directory should be writable by the app and persists across restarts so an interrupted
/// download can be retried.
pub struct Updater {
    staging_dir: PathBuf,
    key: PublicKey,
    current_version: Version,
    platform: String,
    agent: ureq::Agent,
}

impl Updater {
    /// Creates an updater for the current platform.
    ///
    /// `current_version` must be a semantic version; the app normally passes
    /// `env!("CARGO_PKG_VERSION")`. Returns [`UpdateError::InvalidVersion`] for anything else.
    pub fn new(
        staging_dir: impl Into<PathBuf>,
        key: PublicKey,
        current_version: &str,
    ) -> Result<Self, UpdateError> {
        let current_version =
            Version::parse(current_version).map_err(|_| UpdateError::InvalidVersion {
                field: "current version",
                value: current_version.to_owned(),
            })?;
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_per_call(Some(Duration::from_secs(300)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .build();
        Ok(Updater {
            staging_dir: staging_dir.into(),
            key,
            current_version,
            platform: current_platform(),
            agent: ureq::Agent::new_with_config(config),
        })
    }

    /// Overrides the platform key. Tests use this; production should use the host platform.
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = platform.into();
        self
    }

    /// The running version the updater compares against.
    pub fn current_version(&self) -> &Version {
        &self.current_version
    }

    /// The platform key this updater accepts.
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// Verifies the manifest and classifies the offer. Does not touch the network.
    pub fn check(&self, manifest: &Manifest) -> Result<UpdateDecision, UpdateError> {
        let offered = self.verify_offer(manifest)?;
        Ok(if offered > self.current_version {
            UpdateDecision::Available
        } else if offered < self.current_version {
            UpdateDecision::Downgrade
        } else {
            UpdateDecision::UpToDate
        })
    }

    /// Downloads, verifies and stages the offered artifact.
    ///
    /// Only a newer version is accepted; [`Self::check`] reports older or equal offers without
    /// downloading them. A previously staged file that still verifies is reused.
    pub fn download(&self, manifest: &Manifest) -> Result<StagedUpdate, UpdateError> {
        self.require_available(manifest)?;
        let target = self.artifact_path(manifest);
        if target.is_file() && manifest.verify_artifact(&target, &self.key).is_ok() {
            return Ok(StagedUpdate::new(manifest.clone(), target));
        }

        fs::create_dir_all(&self.staging_dir)?;
        let mut response = self.agent.get(&manifest.url).call()?;
        self.write_artifact(manifest, response.body_mut().as_reader(), &target)?;
        Ok(StagedUpdate::new(manifest.clone(), target))
    }

    /// Verifies a local artifact and copies it into the staging directory.
    ///
    /// This is the same path as [`Self::download`] after the bytes arrive: it accepts only a newer
    /// version, enforces the manifest size and SHA-256, and reuses an already staged valid file.
    pub fn stage_from_path(
        &self,
        manifest: &Manifest,
        artifact: &Path,
    ) -> Result<StagedUpdate, UpdateError> {
        self.require_available(manifest)?;
        let target = self.artifact_path(manifest);
        if target.is_file() && manifest.verify_artifact(&target, &self.key).is_ok() {
            return Ok(StagedUpdate::new(manifest.clone(), target));
        }

        fs::create_dir_all(&self.staging_dir)?;
        self.write_artifact(manifest, fs::File::open(artifact)?, &target)?;
        Ok(StagedUpdate::new(manifest.clone(), target))
    }

    fn verify_offer(&self, manifest: &Manifest) -> Result<Version, UpdateError> {
        manifest.verify(&self.key)?;
        if manifest.platform != self.platform {
            return Err(UpdateError::PlatformMismatch {
                expected: self.platform.clone(),
                actual: manifest.platform.clone(),
            });
        }
        if manifest.url.trim().is_empty() {
            return Err(UpdateError::InvalidField {
                field: "url",
                reason: "is empty",
            });
        }
        Version::parse(&manifest.version).map_err(|_| UpdateError::InvalidVersion {
            field: "manifest version",
            value: manifest.version.clone(),
        })
    }

    fn require_available(&self, manifest: &Manifest) -> Result<(), UpdateError> {
        let offered = self.verify_offer(manifest)?;
        if offered > self.current_version {
            return Ok(());
        }
        Err(UpdateError::NotNewer {
            current: self.current_version.to_string(),
            offered: offered.to_string(),
        })
    }

    /// The staging path is content-addressed, so a different manifest cannot overwrite the bytes
    /// described by this one. The extension is preserved for installers that dispatch on it.
    fn artifact_path(&self, manifest: &Manifest) -> PathBuf {
        let mut name = manifest.sha256.clone();
        if let Some(extension) = safe_extension(&manifest.url) {
            name.push('.');
            name.push_str(extension);
        }
        self.staging_dir.join(name)
    }

    fn write_artifact(
        &self,
        manifest: &Manifest,
        mut reader: impl Read,
        target: &Path,
    ) -> Result<(), UpdateError> {
        let mut temp = NamedTempFile::new_in(&self.staging_dir)?;
        {
            let mut limited = reader.by_ref().take(manifest.size);
            let copied = std::io::copy(&mut limited, temp.as_file_mut())?;
            if copied != manifest.size {
                return Err(UpdateError::ArtifactSize {
                    expected: manifest.size,
                    actual: copied,
                });
            }
        }

        let mut extra = [0u8; 1];
        if reader.read(&mut extra)? != 0 {
            return Err(UpdateError::ArtifactTooLarge {
                expected: manifest.size,
            });
        }

        temp.as_file_mut().sync_all()?;
        manifest.verify_artifact(temp.path(), &self.key)?;
        temp.persist(target).map_err(|error| error.error)?;
        Ok(())
    }
}

fn safe_extension(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = Path::new(path).extension()?.to_str()?;
    if extension.is_empty()
        || extension.len() > 16
        || !extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(extension)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use auro_sign::SigningKey;

    use super::*;

    const BYTES: &[u8] = b"artifact bytes";

    fn signed_manifest(
        key: &SigningKey,
        bytes: &[u8],
        version: &str,
        platform: &str,
        url: &str,
    ) -> Manifest {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("artifact");
        fs::write(&path, bytes).expect("write artifact");
        Manifest::for_artifact(&path, version, platform, url, "", "2026-11-17T12:00:00Z")
            .expect("manifest")
            .sign(key)
            .expect("sign")
    }

    fn serve_once(body: &'static [u8]) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("headers");
            stream.write_all(body).expect("body");
        });
        (format!("http://{address}/app-1.2.3.AppImage"), handle)
    }

    fn updater(key: &SigningKey, current: &str) -> (tempfile::TempDir, Updater) {
        let temp = tempfile::tempdir().expect("tempdir");
        let updater = Updater::new(temp.path(), key.public_key(), current)
            .expect("updater")
            .with_platform("linux-x86_64");
        (temp, updater)
    }

    #[test]
    fn classifies_versions_and_rejects_a_different_platform() {
        let key = SigningKey::generate();
        let (_temp, updater) = updater(&key, "1.0.0");

        let newer = signed_manifest(
            &key,
            BYTES,
            "1.1.0",
            "linux-x86_64",
            "https://example.test/app.AppImage",
        );
        assert_eq!(
            updater.check(&newer).expect("check"),
            UpdateDecision::Available
        );

        let same = signed_manifest(
            &key,
            BYTES,
            "1.0.0",
            "linux-x86_64",
            "https://example.test/app.AppImage",
        );
        assert_eq!(
            updater.check(&same).expect("check"),
            UpdateDecision::UpToDate
        );

        let older = signed_manifest(
            &key,
            BYTES,
            "0.9.0",
            "linux-x86_64",
            "https://example.test/app.AppImage",
        );
        assert_eq!(
            updater.check(&older).expect("check"),
            UpdateDecision::Downgrade
        );

        let other_platform = signed_manifest(
            &key,
            BYTES,
            "1.1.0",
            "windows-x86_64",
            "https://example.test/app.AppImage",
        );
        assert!(matches!(
            updater.check(&other_platform),
            Err(UpdateError::PlatformMismatch { .. })
        ));

        let empty_url = signed_manifest(&key, BYTES, "1.1.0", "linux-x86_64", "");
        assert!(matches!(
            updater.check(&empty_url),
            Err(UpdateError::InvalidField { field: "url", .. })
        ));
    }

    #[test]
    fn rejects_invalid_versions_and_bad_current_version() {
        let key = SigningKey::generate();
        let (_temp, updater) = updater(&key, "1.0.0");
        let invalid = signed_manifest(
            &key,
            BYTES,
            "not-a-version",
            "linux-x86_64",
            "https://example.test/app.AppImage",
        );
        assert!(matches!(
            updater.check(&invalid),
            Err(UpdateError::InvalidVersion {
                field: "manifest version",
                ..
            })
        ));

        let temp = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            Updater::new(temp.path(), key.public_key(), "not-a-version"),
            Err(UpdateError::InvalidVersion {
                field: "current version",
                ..
            })
        ));
    }

    #[test]
    fn verifies_before_touching_the_network() {
        let key = SigningKey::generate();
        let wrong = SigningKey::generate();
        let (_temp, updater) = updater(&key, "1.0.0");
        let manifest = signed_manifest(
            &wrong,
            BYTES,
            "1.1.0",
            "linux-x86_64",
            "http://127.0.0.1:9/unreachable",
        );
        assert!(matches!(
            updater.download(&manifest),
            Err(UpdateError::Sign(_))
        ));
    }

    #[test]
    fn downloads_verifies_and_reuses_a_staged_artifact() {
        let key = SigningKey::generate();
        let (temp, updater) = updater(&key, "1.0.0");
        let (url, server) = serve_once(BYTES);
        let manifest = signed_manifest(&key, BYTES, "1.2.3", "linux-x86_64", &url);

        let staged = updater.download(&manifest).expect("download");
        server.join().expect("server");
        assert_eq!(staged.manifest(), &manifest);
        assert_eq!(
            staged.path(),
            temp.path().join(format!("{}.AppImage", manifest.sha256))
        );
        manifest
            .verify_artifact(staged.path(), &key.public_key())
            .expect("artifact");

        // The server accepted one connection; a second call must use the verified file.
        updater.download(&manifest).expect("cached download");
    }

    #[test]
    fn rejects_tampered_short_and_oversized_downloads() {
        let key = SigningKey::generate();
        let (temp, updater) = updater(&key, "1.0.0");

        let (url, server) = serve_once(b"tampered bytes");
        let manifest = signed_manifest(&key, BYTES, "1.2.3", "linux-x86_64", &url);
        assert!(matches!(
            updater.download(&manifest),
            Err(UpdateError::ArtifactHash)
        ));
        server.join().expect("server");
        assert!(
            !temp
                .path()
                .join(format!("{}.AppImage", manifest.sha256))
                .exists()
        );

        let (url, server) = serve_once(b"short");
        let manifest = signed_manifest(&key, BYTES, "1.2.3", "linux-x86_64", &url);
        assert!(matches!(
            updater.download(&manifest),
            Err(UpdateError::ArtifactSize {
                expected: 14,
                actual: 5
            })
        ));
        server.join().expect("server");
        assert!(
            !temp
                .path()
                .join(format!("{}.AppImage", manifest.sha256))
                .exists()
        );

        let (url, server) = serve_once(b"artifact bytes plus extra");
        let manifest = signed_manifest(&key, BYTES, "1.2.3", "linux-x86_64", &url);
        assert!(matches!(
            updater.download(&manifest),
            Err(UpdateError::ArtifactTooLarge { .. })
        ));
        server.join().expect("server");
        assert!(
            !temp
                .path()
                .join(format!("{}.AppImage", manifest.sha256))
                .exists()
        );
    }

    #[test]
    fn refuses_to_stage_an_up_to_date_version() {
        let key = SigningKey::generate();
        let (_temp, updater) = updater(&key, "1.2.3");
        let manifest = signed_manifest(
            &key,
            BYTES,
            "1.2.3",
            "linux-x86_64",
            "http://127.0.0.1:9/unreachable",
        );
        assert!(matches!(
            updater.download(&manifest),
            Err(UpdateError::NotNewer { .. })
        ));
    }

    #[test]
    fn stages_a_local_artifact() {
        let key = SigningKey::generate();
        let (temp, updater) = updater(&key, "1.0.0");
        let source = temp.path().join("local.AppImage");
        fs::write(&source, BYTES).expect("write");
        let manifest = signed_manifest(
            &key,
            BYTES,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/app.AppImage",
        );

        let staged = updater.stage_from_path(&manifest, &source).expect("stage");
        manifest
            .verify_artifact(staged.path(), &key.public_key())
            .expect("artifact");
    }
}
