// SPDX-License-Identifier: MIT OR Apache-2.0

//! Runtime half of the updater: decide, download, verify and stage.
//!
//! Staging stops before installation. The staged file is content-addressed by the artifact SHA-256,
//! so a retry either reuses a file that still verifies or replaces it. Platform installation and
//! rollback are a separate contract.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use auro_sign::PublicKey;
use semver::Version;
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;

use crate::Delta;

use crate::{Manifest, UpdateError, current_platform};

/// What an offered manifest means relative to the running version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    /// The offered version is the running version.
    UpToDate,
    /// The offered version is newer.
    Available,
    /// The offered version is newer but this install is outside the staged rollout percentage.
    HeldBack,
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

    pub(crate) fn from_parts(manifest: Manifest, path: PathBuf) -> Self {
        Self::new(manifest, path)
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
    rollout_key: Option<String>,
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
            rollout_key: None,
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

    /// The pinned public key used to verify manifests and artifacts.
    pub fn public_key(&self) -> PublicKey {
        self.key
    }

    /// Sets a stable per-install key for deterministic staged-rollout selection.
    pub fn with_rollout_key(mut self, key: impl Into<String>) -> Self {
        self.rollout_key = Some(key.into());
        self
    }

    /// Verifies the manifest and classifies the offer. Does not touch the network.
    pub fn check(&self, manifest: &Manifest) -> Result<UpdateDecision, UpdateError> {
        let offered = self.verify_offer(manifest)?;
        Ok(if offered > self.current_version {
            if let Some(rollout) = manifest.rollout
                && !self.selected(manifest, rollout)
            {
                UpdateDecision::HeldBack
            } else {
                UpdateDecision::Available
            }
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

    /// Downloads a manifest's binary delta and applies it to `base`.
    ///
    /// The base must match the delta's `from_version` and `from_sha256`. The resulting full
    /// artifact is verified against the manifest and staged like a full download. Callers that
    /// want a fallback can call [`Self::download`] when this returns an error.
    pub fn download_delta(
        &self,
        manifest: &Manifest,
        base: &Path,
    ) -> Result<StagedUpdate, UpdateError> {
        self.require_available(manifest)?;
        let delta = manifest.delta.as_ref().ok_or(UpdateError::DeltaMissing)?;
        let target = self.artifact_path(manifest);
        if target.is_file() && manifest.verify_artifact(&target, &self.key).is_ok() {
            return Ok(StagedUpdate::new(manifest.clone(), target));
        }
        self.check_delta_base(manifest, delta, base)?;
        fs::create_dir_all(&self.staging_dir)?;
        let delta_path = self.staging_dir.join(format!("{}.delta", delta.sha256));
        if !delta_path.is_file() || verify_blob(&delta_path, delta.size, &delta.sha256).is_err() {
            let mut response = self.agent.get(&delta.url).call()?;
            write_blob(
                &mut response.body_mut().as_reader(),
                delta.size,
                &delta_path,
                &self.staging_dir,
            )?;
            verify_blob(&delta_path, delta.size, &delta.sha256)?;
        }
        self.apply_delta(manifest, base, &delta_path, &target)?;
        Ok(StagedUpdate::new(manifest.clone(), target))
    }

    /// Applies a local bsdiff-compatible patch to `base` and stages the verified full artifact.
    pub fn stage_delta_from_path(
        &self,
        manifest: &Manifest,
        base: &Path,
        patch: &Path,
    ) -> Result<StagedUpdate, UpdateError> {
        self.require_available(manifest)?;
        let delta = manifest.delta.as_ref().ok_or(UpdateError::DeltaMissing)?;
        self.check_delta_base(manifest, delta, base)?;
        verify_blob(patch, delta.size, &delta.sha256)?;
        let target = self.artifact_path(manifest);
        self.apply_delta(manifest, base, patch, &target)?;
        Ok(StagedUpdate::new(manifest.clone(), target))
    }

    fn check_delta_base(
        &self,
        manifest: &Manifest,
        delta: &Delta,
        base: &Path,
    ) -> Result<(), UpdateError> {
        let expected = self.current_version.to_string();
        if delta.from_version != expected {
            return Err(UpdateError::DeltaVersionMismatch {
                expected,
                actual: delta.from_version.clone(),
            });
        }
        if !base.is_file() || auro_sign::sha256_file(base)? != delta.from_sha256 {
            return Err(UpdateError::DeltaBaseMismatch);
        }
        if manifest.version.is_empty() {
            return Err(UpdateError::InvalidField {
                field: "version",
                reason: "is empty",
            });
        }
        Ok(())
    }

    fn apply_delta(
        &self,
        manifest: &Manifest,
        base: &Path,
        patch: &Path,
        target: &Path,
    ) -> Result<(), UpdateError> {
        fs::create_dir_all(&self.staging_dir)?;
        let source = fs::read(base)?;
        let patch = fs::read(patch)?;
        let patcher = qbsdiff::Bspatch::new(&patch)?;
        let mut output = Vec::with_capacity(manifest.size as usize);
        patcher.apply(&source, Cursor::new(&mut output))?;
        let mut temp = NamedTempFile::new_in(&self.staging_dir)?;
        temp.as_file_mut().write_all(&output)?;
        temp.as_file_mut().sync_all()?;
        manifest.verify_artifact(temp.path(), &self.key)?;
        temp.persist(target).map_err(|error| error.error)?;
        Ok(())
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
        match self.check(manifest)? {
            UpdateDecision::Available => Ok(()),
            UpdateDecision::HeldBack => Err(UpdateError::HeldBack {
                version: manifest.version.clone(),
            }),
            UpdateDecision::Downgrade | UpdateDecision::UpToDate => Err(UpdateError::NotNewer {
                current: self.current_version.to_string(),
                offered: manifest.version.clone(),
            }),
        }
    }

    fn selected(&self, manifest: &Manifest, rollout: crate::Rollout) -> bool {
        if rollout.percentage >= 100 {
            return true;
        }
        if rollout.percentage == 0 {
            return false;
        }
        let Some(key) = self.rollout_key.as_deref() else {
            return false;
        };
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hasher.update(b"\n");
        hasher.update(manifest.version.as_bytes());
        hasher.update(b"\n");
        hasher.update(manifest.sha256.as_bytes());
        let digest = hasher.finalize();
        let bucket = u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix")) % 100;
        bucket < u64::from(rollout.percentage)
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

fn verify_blob(path: &Path, size: u64, sha256: &str) -> Result<(), UpdateError> {
    let bytes = fs::read(path)?;
    if bytes.len() as u64 != size {
        return Err(UpdateError::ArtifactSize {
            expected: size,
            actual: bytes.len() as u64,
        });
    }
    if auro_sign::sha256_file(path)? != sha256 {
        return Err(UpdateError::ArtifactHash);
    }
    Ok(())
}

fn write_blob(
    reader: &mut impl Read,
    size: u64,
    target: &Path,
    staging: &Path,
) -> Result<(), UpdateError> {
    let mut temp = NamedTempFile::new_in(staging)?;
    let mut limited = reader.by_ref().take(size);
    let copied = std::io::copy(&mut limited, temp.as_file_mut())?;
    if copied != size {
        return Err(UpdateError::ArtifactSize {
            expected: size,
            actual: copied,
        });
    }
    let mut extra = [0u8; 1];
    if reader.read(&mut extra)? != 0 {
        return Err(UpdateError::ArtifactTooLarge { expected: size });
    }
    temp.as_file_mut().sync_all()?;
    temp.persist(target).map_err(|error| error.error)?;
    Ok(())
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

    fn signed_with_rollout(key: &SigningKey, bytes: &[u8], percentage: u8) -> Manifest {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("artifact");
        fs::write(&path, bytes).expect("artifact");
        Manifest::for_artifact(
            &path,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/app.bin",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .with_rollout(crate::Rollout { percentage })
        .sign(key)
        .expect("sign")
    }

    #[test]
    fn staged_rollout_is_deterministic_and_gates_downloads() {
        let key = SigningKey::generate();
        let (_temp, updater) = updater(&key, "1.0.0");
        let full = signed_with_rollout(&key, BYTES, 100);
        assert_eq!(
            updater.check(&full).expect("check"),
            UpdateDecision::Available
        );

        let held = signed_with_rollout(&key, BYTES, 0);
        assert_eq!(
            updater.check(&held).expect("check"),
            UpdateDecision::HeldBack
        );
        assert!(matches!(
            updater.download(&held),
            Err(UpdateError::HeldBack { .. })
        ));

        let partial = signed_with_rollout(&key, BYTES, 50);
        assert_eq!(
            updater.check(&partial).expect("check"),
            UpdateDecision::HeldBack,
            "without a rollout key a partial rollout must stay closed"
        );
        let with_key = Updater::new(
            tempfile::tempdir().expect("tempdir").path(),
            key.public_key(),
            "1.0.0",
        )
        .expect("updater")
        .with_platform("linux-x86_64")
        .with_rollout_key("stable-install");
        let first = with_key.check(&partial).expect("first");
        let second = with_key.check(&partial).expect("second");
        assert_eq!(first, second, "rollout selection must be stable");
    }

    #[test]
    fn applies_a_local_delta_and_verifies_the_full_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let base = temp.path().join("base.bin");
        let target = temp.path().join("target.bin");
        let patch_path = temp.path().join("app.patch");
        fs::write(&base, b"the old application bytes").expect("base");
        fs::write(&target, b"the new application bytes with changes").expect("target");
        let mut patch = Vec::new();
        qbsdiff::Bsdiff::new(
            &fs::read(&base).expect("base"),
            &fs::read(&target).expect("target"),
        )
        .compare(Cursor::new(&mut patch))
        .expect("diff");
        fs::write(&patch_path, &patch).expect("patch");
        let manifest = Manifest::for_artifact(
            &target,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/target.bin",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .with_delta(crate::Delta {
            from_version: "1.0.0".to_owned(),
            from_sha256: auro_sign::sha256_file(&base).expect("base hash"),
            url: "https://example.test/app.patch".to_owned(),
            size: patch.len() as u64,
            sha256: auro_sign::sha256_file(&patch_path).expect("patch hash"),
        })
        .sign(&key)
        .expect("sign");
        let updater = Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("linux-x86_64");
        let staged = updater
            .stage_delta_from_path(&manifest, &base, &patch_path)
            .expect("apply delta");
        assert_eq!(
            fs::read(staged.path()).expect("staged"),
            fs::read(&target).expect("target")
        );
        manifest
            .verify_artifact(staged.path(), &key.public_key())
            .expect("full artifact");
    }

    #[test]
    fn rejects_a_delta_for_the_wrong_base() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = SigningKey::generate();
        let base = temp.path().join("base.bin");
        let target = temp.path().join("target.bin");
        let patch = temp.path().join("app.patch");
        fs::write(&base, b"old").expect("base");
        fs::write(&target, b"new").expect("target");
        let mut patch_bytes = Vec::new();
        qbsdiff::Bsdiff::new(b"old", b"new")
            .compare(Cursor::new(&mut patch_bytes))
            .expect("diff");
        fs::write(&patch, &patch_bytes).expect("patch");
        let manifest = Manifest::for_artifact(
            &target,
            "1.2.3",
            "linux-x86_64",
            "https://example.test/target.bin",
            "",
            "2026-11-18T00:00:00Z",
        )
        .expect("manifest")
        .with_delta(crate::Delta {
            from_version: "1.0.0".to_owned(),
            from_sha256: auro_sign::sha256_file(&base).expect("hash"),
            url: "https://example.test/app.patch".to_owned(),
            size: patch_bytes.len() as u64,
            sha256: auro_sign::sha256_file(&patch).expect("patch hash"),
        })
        .sign(&key)
        .expect("sign");
        let updater = Updater::new(temp.path().join("staging"), key.public_key(), "1.0.0")
            .expect("updater")
            .with_platform("linux-x86_64");
        fs::write(&base, b"different base").expect("tamper base");
        assert!(matches!(
            updater.stage_delta_from_path(&manifest, &base, &patch),
            Err(UpdateError::DeltaBaseMismatch)
        ));
    }
}
