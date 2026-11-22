// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validation of plugin manifests.
//!
//! A manifest is hostile input. [`PluginManifest::parse`] enforces every limit before any value
//! is used, rejects unknown fields and wrong types, and never deserializes into
//! `serde_json::Value`. `declared-subject` is carried as metadata only: policy decisions use the
//! verified subject derived by [`crate::registry::PluginRegistry::resolve`].

use std::collections::HashSet;

use auro_perms::Capability;
use semver::Version;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// The only supported manifest format version.
pub const MANIFEST_FORMAT_VERSION: u32 = 1;
/// Largest accepted manifest, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Largest accepted individual string field, in bytes.
pub const MAX_STRING_BYTES: usize = 1024;
/// Largest accepted number of commands.
pub const MAX_COMMANDS: usize = 256;
/// Largest accepted number of capability grants.
pub const MAX_CAPABILITIES: usize = 64;

/// Why a manifest was rejected.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// The manifest exceeded the 64 KiB input limit.
    #[error("manifest is {0} bytes; limit is 65536")]
    TooLarge(usize),

    /// The bytes were not a JSON object matching the manifest schema.
    #[error("invalid manifest JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),

    /// A field had the wrong shape or an out-of-range value.
    #[error("invalid field `{field}`: {reason}")]
    InvalidField {
        /// Wire name of the field.
        field: &'static str,
        /// Why the value was rejected.
        reason: String,
    },

    /// The version was not a semantic version.
    #[error("invalid semver version `{0}`")]
    InvalidVersion(String),

    /// The declared SDK version was not the exact version of this crate.
    #[error("unsupported SDK version `{found}`; expected `{expected}`")]
    InvalidSdkVersion {
        /// Value declared by the manifest.
        found: String,
        /// Version required by the host.
        expected: String,
    },

    /// The artifact digest was not 64 lowercase hex digits.
    #[error("invalid artifact SHA-256 digest `{0}`")]
    InvalidHash(String),

    /// The entry point was absolute, prefixed or contained traversal components.
    #[error("invalid entrypoint `{0}`")]
    InvalidEntrypoint(String),

    /// Too many command names were declared.
    #[error("too many commands: {0}; limit is 256")]
    TooManyCommands(usize),

    /// Too many capability grants were declared.
    #[error("too many capabilities: {0}; limit is 64")]
    TooManyCapabilities(usize),

    /// The plugin id was empty.
    #[error("plugin id is empty")]
    EmptyId,

    /// A command name was empty, too long or outside `[A-Za-z0-9_]+`.
    #[error("invalid command name `{0}`")]
    InvalidCommand(String),

    /// A command name appeared more than once.
    #[error("duplicate command name `{0}`")]
    DuplicateCommand(String),

    /// A capability name was not recognised by `auro-perms`.
    #[error("unknown capability `{0}`")]
    InvalidCapability(String),

    /// A scoped capability lacked a scope, or an unscoped capability declared one.
    #[error("invalid scope for `{capability}`: {reason}")]
    InvalidScope {
        /// Capability whose scope was invalid.
        capability: &'static str,
        /// Why the scope was rejected.
        reason: &'static str,
    },

    /// An individual string exceeded the 1024-byte limit.
    #[error("field `{field}` is {length} bytes; limit is 1024")]
    StringTooLarge {
        /// Wire name of the field.
        field: &'static str,
        /// Measured length in bytes.
        length: usize,
    },
}

/// One capability requested by a plugin manifest.
///
/// The host does not grant a request automatically: the app's allowlist and policy decide what is
/// actually permitted. This type only records what the plugin asked for.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ManifestGrant {
    /// The requested capability.
    #[serde(serialize_with = "serialize_capability")]
    pub capability: Capability,
    /// Scope pattern for a scoped capability; `None` for an unscoped one.
    pub scope: Option<String>,
}

impl<'de> Deserialize<'de> for ManifestGrant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireGrant::deserialize(deserializer)?;
        grant_from_wire(wire).map_err(serde::de::Error::custom)
    }
}

/// A validated plugin manifest.
///
/// The wire schema uses `kebab-case` names and `deny_unknown_fields` (see [`WireManifest`]).
/// Canonical serialization through [`PluginManifest::digest`] uses the same field order, so the
/// digest is stable for a manifest with the same values.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginManifest {
    /// Manifest schema version; always [`MANIFEST_FORMAT_VERSION`].
    pub format_version: u32,
    /// Reverse-DNS-ish plugin id, at most 128 ASCII bytes.
    pub id: String,
    /// Exact semantic version of the plugin.
    #[serde(serialize_with = "serialize_version")]
    pub version: Version,
    /// Exact SDK version string; must equal [`crate::sdk_version`].
    pub sdk_version: String,
    /// Metadata only; never used for policy.
    pub declared_subject: Option<String>,
    /// Lowercase hex SHA-256 of the plugin artifact.
    pub artifact_sha256: String,
    /// Relative path of the plugin entry point.
    pub entrypoint: String,
    /// Command names exposed by the plugin.
    pub commands: Vec<String>,
    /// Capabilities the plugin requests.
    pub capabilities: Vec<ManifestGrant>,
}

impl<'de> Deserialize<'de> for PluginManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireManifest::deserialize(deserializer)?;
        Self::try_from(wire).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct WireManifest {
    format_version: u32,
    id: String,
    version: String,
    sdk_version: String,
    declared_subject: Option<String>,
    artifact_sha256: String,
    entrypoint: String,
    commands: Vec<String>,
    capabilities: Vec<WireGrant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct WireGrant {
    capability: String,
    scope: Option<String>,
}

fn serialize_version<S>(version: &Version, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&version.to_string())
}

fn serialize_capability<S>(capability: &Capability, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(capability.as_str())
}

fn check_string(field: &'static str, value: &str) -> Result<(), ManifestError> {
    if value.len() > MAX_STRING_BYTES {
        return Err(ManifestError::StringTooLarge {
            field,
            length: value.len(),
        });
    }
    Ok(())
}

fn grant_from_wire(wire: WireGrant) -> Result<ManifestGrant, ManifestError> {
    let capability = wire
        .capability
        .parse::<Capability>()
        .map_err(|_| ManifestError::InvalidCapability(wire.capability.clone()))?;
    if let Some(scope) = wire.scope.as_deref() {
        check_string("capabilities.scope", scope)?;
    }
    match (capability.is_scoped(), wire.scope.as_deref()) {
        (true, Some(scope)) if !scope.is_empty() => {}
        (true, _) => {
            return Err(ManifestError::InvalidScope {
                capability: capability.as_str(),
                reason: "requires a non-empty scope",
            });
        }
        (false, None) => {}
        (false, Some(_)) => {
            return Err(ManifestError::InvalidScope {
                capability: capability.as_str(),
                reason: "does not accept a scope",
            });
        }
    }
    Ok(ManifestGrant {
        capability,
        scope: wire.scope,
    })
}

impl TryFrom<WireManifest> for PluginManifest {
    type Error = ManifestError;

    fn try_from(wire: WireManifest) -> Result<Self, Self::Error> {
        if wire.format_version != MANIFEST_FORMAT_VERSION {
            return Err(ManifestError::InvalidField {
                field: "format-version",
                reason: format!("expected {MANIFEST_FORMAT_VERSION}"),
            });
        }
        check_string("id", &wire.id)?;
        check_string("version", &wire.version)?;
        check_string("sdk-version", &wire.sdk_version)?;
        check_string("artifact-sha256", &wire.artifact_sha256)?;
        check_string("entrypoint", &wire.entrypoint)?;
        if let Some(subject) = wire.declared_subject.as_deref() {
            check_string("declared-subject", subject)?;
        }

        if wire.id.is_empty() {
            return Err(ManifestError::EmptyId);
        }
        crate::validate::plugin_id(&wire.id).map_err(|reason| ManifestError::InvalidField {
            field: "id",
            reason: reason.to_owned(),
        })?;

        let version = wire
            .version
            .parse::<Version>()
            .map_err(|_| ManifestError::InvalidVersion(wire.version.clone()))?;
        if wire.sdk_version != crate::sdk_version() {
            return Err(ManifestError::InvalidSdkVersion {
                found: wire.sdk_version.clone(),
                expected: crate::sdk_version().to_owned(),
            });
        }
        if !crate::hex::is_sha256(&wire.artifact_sha256) {
            return Err(ManifestError::InvalidHash(wire.artifact_sha256.clone()));
        }
        crate::validate::entrypoint(&wire.entrypoint)
            .map_err(|_| ManifestError::InvalidEntrypoint(wire.entrypoint.clone()))?;

        if wire.commands.len() > MAX_COMMANDS {
            return Err(ManifestError::TooManyCommands(wire.commands.len()));
        }
        let mut seen = HashSet::with_capacity(wire.commands.len());
        for command in &wire.commands {
            check_string("commands", command)?;
            crate::validate::command(command)
                .map_err(|_| ManifestError::InvalidCommand(command.clone()))?;
            if !seen.insert(command.as_str()) {
                return Err(ManifestError::DuplicateCommand(command.clone()));
            }
        }

        if wire.capabilities.len() > MAX_CAPABILITIES {
            return Err(ManifestError::TooManyCapabilities(wire.capabilities.len()));
        }
        let capabilities = wire
            .capabilities
            .into_iter()
            .map(grant_from_wire)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            format_version: wire.format_version,
            id: wire.id,
            version,
            sdk_version: wire.sdk_version,
            declared_subject: wire.declared_subject,
            artifact_sha256: wire.artifact_sha256,
            entrypoint: wire.entrypoint,
            commands: wire.commands,
            capabilities,
        })
    }
}

impl PluginManifest {
    /// Parses and validates a manifest from JSON bytes.
    ///
    /// Inputs larger than [`MAX_MANIFEST_BYTES`] are rejected before deserialization. Every field
    /// is checked here; a value that passes this function is safe to use for identity checks.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge(bytes.len()));
        }
        let wire =
            serde_json::from_slice::<WireManifest>(bytes).map_err(ManifestError::InvalidJson)?;
        Self::try_from(wire)
    }

    /// Lowercase hex SHA-256 of the canonical manifest bytes.
    ///
    /// This is the digest pinned by an index entry. Canonical bytes are
    /// `serde_json::to_vec(self)`: kebab-case field names in declaration order, no whitespace,
    /// no map keys. A byte-for-byte change to the manifest changes this value, so a pinned index
    /// cannot be satisfied by a rewritten manifest.
    pub fn digest(&self) -> String {
        let bytes = match serde_json::to_vec(self) {
            Ok(bytes) => bytes,
            Err(error) => unreachable!("manifest serialization is infallible: {error}"),
        };
        let digest = Sha256::digest(bytes);
        crate::hex::encode(&digest[..])
    }
}
