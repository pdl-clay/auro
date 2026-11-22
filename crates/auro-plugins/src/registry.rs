// SPDX-License-Identifier: MIT OR Apache-2.0

//! Verified subjects from a signed index and an explicit app allowlist.
//!
//! [`PluginRegistry::resolve`] is the trust boundary: it binds a plugin instance to the indexed
//! version, artifact hash and manifest digest, then derives the effective subject from those
//! verified values. A subject declared by the manifest is metadata and is never consulted here.

use std::collections::HashSet;
use std::fmt;
use std::sync::Mutex;

use auro_perms::{PluginSubject, PolicyError};
use semver::Version;
use thiserror::Error;

use crate::index::PluginIndex;
use crate::manifest::{MANIFEST_FORMAT_VERSION, PluginManifest};

/// One id/version pair the app explicitly trusts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedPlugin {
    /// Plugin id from the signed index.
    pub id: String,
    /// Exact version from the signed index.
    pub version: String,
}

/// A subject bound to verified identity, safe to hand to `auro-perms`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VerifiedSubject {
    subject: PluginSubject,
}

impl VerifiedSubject {
    /// The canonical subject string, for example
    /// `plugin:com.example.notes@1.2.3.a1b2c3d4.inst_01`.
    pub fn as_str(&self) -> &str {
        self.subject.as_str()
    }

    /// The underlying capability subject.
    pub fn as_plugin_subject(&self) -> &PluginSubject {
        &self.subject
    }
}

impl fmt::Display for VerifiedSubject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.subject.as_str())
    }
}

/// Why a plugin instance was not admitted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginLoadError {
    /// The id is absent from the signed index.
    #[error("plugin `{0}` is not in the signed index")]
    NotIndexed(String),

    /// The app allowlist does not contain this exact id and version.
    #[error("plugin `{id}` version `{version}` is not allowed by the app")]
    VersionNotAllowed {
        /// Requested plugin id.
        id: String,
        /// Requested version.
        version: String,
    },

    /// The requested version is not the one pinned in the index.
    #[error("plugin version mismatch: index pins `{expected}`, requested `{found}`")]
    VersionMismatch {
        /// Version pinned by the index.
        expected: String,
        /// Version requested by the caller.
        found: String,
    },

    /// The plugin was built against a different SDK version.
    #[error("SDK version mismatch: host has `{expected}`, manifest declares `{found}`")]
    SdkMismatch {
        /// Host SDK version.
        expected: String,
        /// Version declared in the manifest.
        found: String,
    },

    /// The artifact or manifest hash does not match the index pin.
    #[error("artifact hash mismatch: index pins `{expected}`, observed `{found}`")]
    ArtifactHashMismatch {
        /// Hash pinned by the index.
        expected: String,
        /// Hash supplied by the caller or manifest.
        found: String,
    },

    /// The canonical manifest digest does not match the index pin.
    #[error("manifest digest mismatch: index pins `{expected}`, observed `{found}`")]
    ManifestDigestMismatch {
        /// Digest pinned by the index.
        expected: String,
        /// Digest computed from the manifest.
        found: String,
    },

    /// The manifest describes a different plugin than the one requested.
    #[error("manifest identity mismatch: expected `{expected_id}`, found `{found_id}`")]
    ManifestIdentityMismatch {
        /// Requested id.
        expected_id: String,
        /// Requested version.
        expected_version: String,
        /// Id declared by the manifest.
        found_id: String,
        /// Version declared by the manifest.
        found_version: String,
    },

    /// The derived subject was not accepted by `auro-perms`.
    #[error("invalid plugin subject `{text}`: {detail}")]
    InvalidSubject {
        /// Subject string that was rejected.
        text: String,
        /// Human-readable validation detail from `auro-perms`.
        detail: String,
    },

    /// Another live instance already resolved to the same subject.
    #[error("duplicate plugin subject `{0}`")]
    DuplicateSubject(String),

    /// The subject used a namespace reserved for the host or the app.
    #[error("reserved subject namespace `{0}`")]
    ReservedSubject(String),

    /// The index was not built by `PluginIndex::parse_signed` or is internally invalid.
    #[error("invalid signed index entry")]
    InvalidIndex,

    /// The manifest was not built by `PluginManifest::parse` or is internally invalid.
    #[error("invalid plugin manifest")]
    InvalidManifest,

    /// The loaded-subject set was poisoned by a panic in another thread.
    #[error("plugin registry state is poisoned")]
    RegistryPoisoned,
}

/// Resolves indexed, allowed plugins to verified subjects.
pub struct PluginRegistry {
    index: PluginIndex,
    allow: Vec<AllowedPlugin>,
    sdk_version: String,
    loaded: Mutex<HashSet<PluginSubject>>,
}

impl PluginRegistry {
    /// Creates a registry from a verified index and the app's explicit allowlist.
    ///
    /// `sdk_version` is compared by exact string equality with each manifest. Callers normally
    /// pass [`crate::sdk_version`]. `allow` is not a grant: it only names which indexed versions
    /// may be loaded; capabilities remain a separate policy decision.
    pub fn new(index: PluginIndex, allow: Vec<AllowedPlugin>, sdk_version: &str) -> Self {
        Self {
            index,
            allow,
            sdk_version: sdk_version.to_owned(),
            loaded: Mutex::new(HashSet::new()),
        }
    }

    /// Validates an already formed subject string.
    ///
    /// [`Self::resolve`] uses this for the subject it derives; it is public so a host can validate
    /// subjects received from other components and so the reserved-namespace mapping can be
    /// exercised without a private key. Nothing here consults the manifest's declared subject.
    pub fn verify_subject(text: &str) -> Result<VerifiedSubject, PluginLoadError> {
        let subject = PluginSubject::new(text).map_err(|error| match error {
            PolicyError::ReservedSubject(value) => PluginLoadError::ReservedSubject(value),
            other => PluginLoadError::InvalidSubject {
                text: text.to_owned(),
                detail: other.to_string(),
            },
        })?;
        Ok(VerifiedSubject { subject })
    }

    /// Verifies a plugin instance and returns its effective subject.
    ///
    /// Checks run in this order: the id is indexed; the version is allowed and equals the pin;
    /// the SDK version matches exactly; the artifact hash matches the pin and the manifest; the
    /// canonical manifest digest matches the pin; the manifest id and version match the request;
    /// the derived subject is valid; and no live instance already holds that subject.
    ///
    /// The subject is `plugin:<id>@<version>.<first 8 artifact hash chars>.<instance_id>`. A
    /// restarted instance must use a different `instance_id`; it then receives a distinct subject
    /// and inherits no grant state. No process is spawned and no file is read.
    pub fn resolve(
        &self,
        id: &str,
        version: &str,
        artifact_sha256: &str,
        manifest: &PluginManifest,
        instance_id: &str,
    ) -> Result<VerifiedSubject, PluginLoadError> {
        let entry = self
            .index
            .get(id)
            .ok_or_else(|| PluginLoadError::NotIndexed(id.to_owned()))?;
        if crate::validate::plugin_id(&entry.id).is_err()
            || !crate::hex::is_sha256(&entry.artifact_sha256)
            || !crate::hex::is_sha256(&entry.manifest_sha256)
        {
            return Err(PluginLoadError::InvalidIndex);
        }

        if !self
            .allow
            .iter()
            .any(|allowed| allowed.id == id && allowed.version == version)
        {
            return Err(PluginLoadError::VersionNotAllowed {
                id: id.to_owned(),
                version: version.to_owned(),
            });
        }
        let requested =
            version
                .parse::<Version>()
                .map_err(|_| PluginLoadError::VersionNotAllowed {
                    id: id.to_owned(),
                    version: version.to_owned(),
                })?;
        if entry.version != requested {
            return Err(PluginLoadError::VersionMismatch {
                expected: entry.version.to_string(),
                found: version.to_owned(),
            });
        }

        if manifest.sdk_version != self.sdk_version {
            return Err(PluginLoadError::SdkMismatch {
                expected: self.sdk_version.clone(),
                found: manifest.sdk_version.clone(),
            });
        }

        if artifact_sha256 != entry.artifact_sha256
            || manifest.artifact_sha256 != entry.artifact_sha256
        {
            return Err(PluginLoadError::ArtifactHashMismatch {
                expected: entry.artifact_sha256.clone(),
                found: artifact_sha256.to_owned(),
            });
        }

        if manifest.format_version != MANIFEST_FORMAT_VERSION
            || crate::validate::plugin_id(&manifest.id).is_err()
            || !crate::hex::is_sha256(&manifest.artifact_sha256)
        {
            return Err(PluginLoadError::InvalidManifest);
        }
        let digest = manifest.digest();
        if digest != entry.manifest_sha256 {
            return Err(PluginLoadError::ManifestDigestMismatch {
                expected: entry.manifest_sha256.clone(),
                found: digest,
            });
        }

        if manifest.id != id || manifest.version != requested {
            return Err(PluginLoadError::ManifestIdentityMismatch {
                expected_id: id.to_owned(),
                expected_version: version.to_owned(),
                found_id: manifest.id.clone(),
                found_version: manifest.version.to_string(),
            });
        }

        crate::validate::instance_id(instance_id).map_err(|reason| {
            PluginLoadError::InvalidSubject {
                text: instance_id.to_owned(),
                detail: reason.to_owned(),
            }
        })?;
        let hash_prefix = &artifact_sha256[..8];
        let text = format!("plugin:{id}@{version}.{hash_prefix}.{instance_id}");
        let subject = Self::verify_subject(&text)?;

        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| PluginLoadError::RegistryPoisoned)?;
        if loaded.contains(&subject.subject) {
            return Err(PluginLoadError::DuplicateSubject(text));
        }
        loaded.insert(subject.subject.clone());
        Ok(subject)
    }
}
