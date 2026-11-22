// SPDX-License-Identifier: MIT OR Apache-2.0

//! Verified subjects from a signed index and an explicit app allowlist.
//!
//! [`PluginRegistry::resolve`] is the trust boundary: it binds a plugin instance to the indexed
//! version, artifact hash and manifest digest, then derives the effective subject from those
//! verified values. A subject declared by the manifest is metadata and is never consulted here.

use std::collections::HashSet;
use std::fmt;
use std::sync::Mutex;

use auro_perms::{Capability, Denied, Grant, Policy, PolicyError};
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
///
/// The field is private and [`PluginRegistry::resolve`] is the only constructor. There is no
/// public `new`, `From` or deserialization path: a subject cannot be fabricated from a string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VerifiedSubject {
    subject: String,
}

impl VerifiedSubject {
    /// The canonical subject string, for example
    /// `plugin:com.example.notes@1.2.3.a1b2c3d4.inst_01`.
    pub fn as_str(&self) -> &str {
        &self.subject
    }
}

impl fmt::Display for VerifiedSubject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.subject)
    }
}

/// Plugin-aware capability grants and checks on [`Policy`].
///
/// This trait supplies the verified subject string to the string-based primitives in
/// `auro-perms`. A grant can only name a [`VerifiedSubject`] returned by
/// [`PluginRegistry::resolve`], after the signed index, hashes, allowlist and instance checks.
///
/// The effect site still canonicalizes and revalidates paths before calling [`Self::check_plugin`]
/// (spec 0016 D2/D5): scoped values must be canonicalized after opening a handle, and raw
/// argument strings must never be passed for scoped checks. This trait performs no I/O and
/// resolves no paths; `auro-perms` remains pure.
pub trait PluginPolicyExt: Sized {
    /// Grants `capability` to the verified plugin `subject`.
    ///
    /// Scoped capabilities require a pattern; unscoped capabilities must not carry one.
    fn allow_plugin(
        self,
        subject: &VerifiedSubject,
        capability: Capability,
        pattern: Option<&str>,
    ) -> Result<Self, PolicyError>;

    /// Answers a capability check for the verified plugin `subject`.
    ///
    /// `Some(value)` selects a scoped check, `None` an unscoped one. The effect site is
    /// responsible for the canonicalization described on the trait.
    fn check_plugin(
        &self,
        subject: &VerifiedSubject,
        capability: Capability,
        value: Option<&str>,
    ) -> Result<(), Denied>;
}

impl PluginPolicyExt for Policy {
    fn allow_plugin(
        self,
        subject: &VerifiedSubject,
        capability: Capability,
        pattern: Option<&str>,
    ) -> Result<Self, PolicyError> {
        let grant = match (capability.is_scoped(), pattern) {
            (true, Some(pattern)) => Grant::scoped(subject.as_str(), capability, pattern)?,
            (true, None) => return Err(PolicyError::ScopeRequired { capability }),
            (false, None) => Grant::new(subject.as_str(), capability)?,
            (false, Some(_)) => return Err(PolicyError::ScopeNotSupported { capability }),
        };
        Ok(self.allow(grant))
    }

    fn check_plugin(
        &self,
        subject: &VerifiedSubject,
        capability: Capability,
        value: Option<&str>,
    ) -> Result<(), Denied> {
        match value {
            Some(value) => self.check_scope(subject.as_str(), capability, value),
            None => self.check(subject.as_str(), capability),
        }
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

    /// The instance id or the derived subject failed load-time validation.
    #[error("invalid plugin subject `{text}`: {detail}")]
    InvalidSubject {
        /// Subject string or instance id that was rejected.
        text: String,
        /// Human-readable validation detail.
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
    loaded: Mutex<HashSet<VerifiedSubject>>,
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

    /// Validates the subject derived inside [`Self::resolve`].
    ///
    /// This is private on purpose: no caller-supplied string can become a [`VerifiedSubject`].
    /// The subject passed here is built from signed-index values, never from a manifest
    /// declaration. The `host` and `app` namespaces are reserved for the host and the app; a
    /// plugin id whose first label claims one of them is rejected before it is admitted.
    fn validate_derived_subject(text: &str) -> Result<(), PluginLoadError> {
        let namespace = text.split_once(':').map_or(text, |(head, _)| head);
        let id_namespace = text
            .strip_prefix("plugin:")
            .unwrap_or(text)
            .split(['@', '.'])
            .next()
            .unwrap_or("");
        for reserved in [namespace, id_namespace] {
            if reserved.eq_ignore_ascii_case("host") || reserved.eq_ignore_ascii_case("app") {
                return Err(PluginLoadError::ReservedSubject(reserved.to_owned()));
            }
        }
        let valid = !text.is_empty()
            && text.len() <= 128
            && text.starts_with("plugin:")
            && text.len() > "plugin:".len()
            && !text.ends_with('.')
            && !text.contains("..")
            && !text.chars().any(char::is_control)
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
            });
        if !valid {
            return Err(PluginLoadError::InvalidSubject {
                text: text.to_owned(),
                detail: "subject must be a short `plugin:`-prefixed ASCII identity".to_owned(),
            });
        }
        Ok(())
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
        Self::validate_derived_subject(&text)?;
        let subject = VerifiedSubject {
            subject: text.clone(),
        };

        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| PluginLoadError::RegistryPoisoned)?;
        if loaded.contains(&subject) {
            return Err(PluginLoadError::DuplicateSubject(text));
        }
        loaded.insert(subject.clone());
        Ok(subject)
    }
}
