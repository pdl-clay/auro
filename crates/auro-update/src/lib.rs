// SPDX-License-Identifier: MIT OR Apache-2.0

//! Signed update manifests.
//!
//! A manifest describes one release artifact: version, platform, URL, size and SHA-256. Its
//! signature covers a canonical JSON payload with the signature field omitted, so changing any
//! signed field invalidates it. [`Updater`] adds the runtime half: it checks the offer, downloads
//! the artifact, verifies its size and SHA-256, and stages it. Platform installation and rollback
//! are separate concerns.

use std::fs;
use std::path::{Path, PathBuf};

pub use auro_sign::PublicKey;
use auro_sign::{SignError, SigningKey};
use serde::{Deserialize, Serialize};

/// The manifest format this build understands.
pub const FORMAT_VERSION: u32 = 1;

/// The update journal format this build understands.
pub const JOURNAL_FORMAT_VERSION: u32 = 2;

/// Why a manifest or artifact failed verification.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("signature error: {0}")]
    Sign(#[from] SignError),

    #[error("invalid manifest JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unsupported manifest format {0}; this build understands {FORMAT_VERSION}")]
    UnsupportedFormat(u32),

    #[error("manifest has no signature")]
    MissingSignature,

    #[error("artifact size mismatch: manifest says {expected}, file is {actual}")]
    ArtifactSize { expected: u64, actual: u64 },

    #[error("artifact is larger than the manifest size of {expected} bytes")]
    ArtifactTooLarge { expected: u64 },

    #[error("artifact SHA-256 mismatch")]
    ArtifactHash,

    #[error("manifest field `{field}` {reason}")]
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },

    #[error("invalid {field} `{value}`; expected a semantic version")]
    InvalidVersion { field: &'static str, value: String },

    #[error("manifest platform `{actual}` does not match this build `{expected}`")]
    PlatformMismatch { expected: String, actual: String },

    #[error("offered version {offered} is not newer than the running version {current}")]
    NotNewer { current: String, offered: String },

    #[error("download failed: {0}")]
    Http(#[from] ureq::Error),

    #[error("install target `{}` does not exist", .path.display())]
    InstallTargetMissing { path: PathBuf },

    #[error("install target `{}` is not a regular file", .path.display())]
    InstallTargetNotFile { path: PathBuf },

    #[error("install target `{}` is not a directory", .path.display())]
    InstallTargetNotDirectory { path: PathBuf },

    #[error("staged artifact `{}` is missing", .path.display())]
    StagedArtifactMissing { path: PathBuf },

    #[error("rollback backup `{}` is missing", .path.display())]
    RollbackBackupMissing { path: PathBuf },

    #[error("unsupported installer artifact `{}`; expected {expected}", .path.display())]
    UnsupportedInstaller {
        path: PathBuf,
        expected: &'static str,
    },

    #[error("installer `{}` failed with exit code {code:?}", .path.display())]
    InstallerFailed { path: PathBuf, code: Option<i32> },

    #[error("update archive `{}` contains no .app bundle", .path.display())]
    AppBundleMissing { path: PathBuf },

    #[error("update archive `{}` contains more than one .app bundle", .path.display())]
    AppBundleAmbiguous { path: PathBuf },

    #[cfg(any(target_os = "macos", test))]
    #[error("archive error: {0}")]
    Archive(#[from] zip::result::ZipError),

    #[error("cannot install `{}`; restoring backup `{}` failed: {message}", .target.display(), .backup.display())]
    RestoreFailed {
        target: PathBuf,
        backup: PathBuf,
        message: String,
    },

    #[error("cannot roll back `{}`; backup `{}` remains: {message}", .target.display(), .backup.display())]
    RollbackFailed {
        target: PathBuf,
        backup: PathBuf,
        message: String,
    },

    #[error("archive entry `{}` escapes the extraction directory through a symlink", .path.display())]
    UnsafeArchive { path: PathBuf },

    #[error("update journal `{}` is missing", .path.display())]
    JournalMissing { path: PathBuf },

    #[error(
        "unsupported update journal format {0}; this build understands {JOURNAL_FORMAT_VERSION}"
    )]
    JournalFormat(u32),

    #[error("update journal is {actual:?}; expected Applied")]
    JournalNotApplied { actual: crate::journal::ApplyState },

    #[error("restart lock `{}` is busy: {message}", .path.display())]
    RestartLock { path: PathBuf, message: String },

    #[error("timed out waiting for the app to exit before restart")]
    RestartTimeout,

    #[error("journal lock `{}` is not the sibling of `{}`", .lock.display(), .path.display())]
    JournalLockPath { path: PathBuf, lock: PathBuf },

    #[error("relaunch executable `{}` is missing", .path.display())]
    RelaunchMissing { path: PathBuf },

    #[error("journal has no apply kind for a prepared helper")]
    ApplyKindMissing,

    #[error("journal has no signed manifest for a prepared helper")]
    ManifestMissing,

    #[error("journal has no public key for a prepared helper")]
    VerificationKeyMissing,

    #[error("journal version `{journal}` does not match manifest version `{manifest}`")]
    JournalVersionMismatch { journal: String, manifest: String },

    #[error("journal manifest platform `{actual}` does not match this host `{expected}`")]
    JournalPlatformMismatch { expected: String, actual: String },

    #[error("apply target is missing for {kind:?}")]
    ApplyTargetMissing { kind: crate::journal::ApplyKind },

    #[error("apply kind {kind:?} is not supported on this host")]
    UnsupportedApply { kind: crate::journal::ApplyKind },

    #[error("update backup `{}` is not beside target `{}`", .backup.display(), .target.display())]
    JournalBackupPath { backup: PathBuf, target: PathBuf },
}

mod journal;

pub use journal::{
    ApplyJournal, ApplyKind, ApplyPlan, ApplyState, RESTART_HELPER_ENV, Recovery, Relaunch,
    run_restart_helper,
};

mod installer;

pub use installer::{InstallerRunner, SystemInstallerRunner};

mod runtime;

pub use runtime::{StagedUpdate, UpdateDecision, Updater};

#[cfg(any(target_os = "windows", test))]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{WindowsInstallOutcome, WindowsInstaller, WindowsInstallerKind};

#[cfg(any(target_os = "linux", test))]
mod linux_package;

#[cfg(target_os = "linux")]
pub use linux_package::{LinuxPackageInstaller, LinuxPackageKind};

#[cfg(any(target_os = "macos", test))]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{MacAppInstaller, MacAppReceipt};

#[cfg(target_os = "linux")]
mod appimage;

#[cfg(target_os = "linux")]
pub use appimage::{AppImageInstaller, InstallReceipt};

/// A signed release manifest.
///
/// Field order is part of the signed payload. Do not reorder or rename fields without bumping
/// [`FORMAT_VERSION`] and writing a migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub notes: String,
    pub published_at: String,
    pub signature: String,
}

#[derive(Serialize)]
struct Payload<'a> {
    format: u32,
    version: &'a str,
    platform: &'a str,
    url: &'a str,
    size: u64,
    sha256: &'a str,
    notes: &'a str,
    published_at: &'a str,
}

impl Manifest {
    /// Creates an unsigned manifest for one artifact.
    pub fn new(
        version: impl Into<String>,
        platform: impl Into<String>,
        url: impl Into<String>,
        size: u64,
        sha256: impl Into<String>,
        notes: impl Into<String>,
        published_at: impl Into<String>,
    ) -> Self {
        Manifest {
            format: FORMAT_VERSION,
            version: version.into(),
            platform: platform.into(),
            url: url.into(),
            size,
            sha256: sha256.into(),
            notes: notes.into(),
            published_at: published_at.into(),
            signature: String::new(),
        }
    }

    /// Builds a manifest for a local artifact, using its size and SHA-256.
    pub fn for_artifact(
        artifact: &Path,
        version: impl Into<String>,
        platform: impl Into<String>,
        url: impl Into<String>,
        notes: impl Into<String>,
        published_at: impl Into<String>,
    ) -> Result<Self, UpdateError> {
        let bytes = fs::read(artifact)?;
        Ok(Manifest::new(
            version,
            platform,
            url,
            bytes.len() as u64,
            auro_sign::sha256_file(artifact)?,
            notes,
            published_at,
        ))
    }

    /// Signs the canonical payload and stores the base64 signature.
    pub fn sign(mut self, key: &SigningKey) -> Result<Self, UpdateError> {
        self.signature = key.sign(&self.payload()?);
        Ok(self)
    }

    /// Verifies the format, the SHA-256 shape and the signature. Does not download or hash the
    /// artifact.
    pub fn verify(&self, key: &PublicKey) -> Result<(), UpdateError> {
        if self.format != FORMAT_VERSION {
            return Err(UpdateError::UnsupportedFormat(self.format));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(UpdateError::InvalidField {
                field: "sha256",
                reason: "must be 64 lowercase hexadecimal characters",
            });
        }
        if self.signature.is_empty() {
            return Err(UpdateError::MissingSignature);
        }
        key.verify(&self.payload()?, &self.signature)?;
        Ok(())
    }

    /// Verifies the manifest signature, then the artifact's size and SHA-256.
    pub fn verify_artifact(&self, path: &Path, key: &PublicKey) -> Result<(), UpdateError> {
        self.verify(key)?;
        let bytes = fs::read(path)?;
        if bytes.len() as u64 != self.size {
            return Err(UpdateError::ArtifactSize {
                expected: self.size,
                actual: bytes.len() as u64,
            });
        }
        if auro_sign::sha256_file(path)? != self.sha256 {
            return Err(UpdateError::ArtifactHash);
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, UpdateError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(text: &str) -> Result<Self, UpdateError> {
        Ok(serde_json::from_str(text)?)
    }

    fn payload(&self) -> Result<Vec<u8>, UpdateError> {
        Ok(serde_json::to_vec(&Payload {
            format: self.format,
            version: &self.version,
            platform: &self.platform,
            url: &self.url,
            size: self.size,
            sha256: &self.sha256,
            notes: &self.notes,
            published_at: &self.published_at,
        })?)
    }
}

/// The platform key used by manifests: `<os>-<arch>`.
pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn manifest() -> Manifest {
        Manifest::new(
            "1.2.3",
            "linux-x86_64",
            "https://example.test/app-1.2.3.tar.gz",
            42,
            SHA256,
            "notes",
            "2026-11-17T12:00:00Z",
        )
    }

    #[test]
    fn signs_and_verifies_a_manifest() {
        let key = SigningKey::generate();
        let signed = manifest().sign(&key).expect("sign");
        signed.verify(&key.public_key()).expect("verify");
    }

    #[test]
    fn rejects_a_tampered_field() {
        let key = SigningKey::generate();
        let mut signed = manifest().sign(&key).expect("sign");
        signed.version = "9.9.9".to_owned();
        assert!(signed.verify(&key.public_key()).is_err());
    }

    #[test]
    fn rejects_an_empty_signature_and_unknown_format() {
        let key = SigningKey::generate();
        assert!(manifest().verify(&key.public_key()).is_err());

        let signed = manifest().sign(&key).expect("sign");
        let mut future = signed.clone();
        future.format = 2;
        assert!(matches!(
            future.verify(&key.public_key()),
            Err(UpdateError::UnsupportedFormat(2))
        ));
    }

    #[test]
    fn rejects_a_malformed_artifact_hash() {
        let key = SigningKey::generate();
        let mut signed = manifest().sign(&key).expect("sign");
        signed.sha256 = "not-a-sha256".to_owned();
        assert!(matches!(
            signed.verify(&key.public_key()),
            Err(UpdateError::InvalidField {
                field: "sha256",
                ..
            })
        ));
    }

    #[test]
    fn verifies_an_artifact_and_its_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("app.tar.gz");
        fs::write(&path, b"artifact bytes").expect("write");

        let key = SigningKey::generate();
        let signed = Manifest::for_artifact(
            &path,
            "0.1.0",
            current_platform(),
            "https://example.test/app.tar.gz",
            "",
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest")
        .sign(&key)
        .expect("sign");

        signed
            .verify_artifact(&path, &key.public_key())
            .expect("artifact");

        fs::write(&path, b"tampered").expect("write");
        assert!(signed.verify_artifact(&path, &key.public_key()).is_err());
    }

    #[test]
    fn json_round_trips() {
        let key = SigningKey::generate();
        let signed = manifest().sign(&key).expect("sign");
        let text = signed.to_json().expect("json");
        let parsed = Manifest::from_json(&text).expect("parse");
        assert_eq!(parsed, signed);
        parsed.verify(&key.public_key()).expect("verify");
    }
}
