// SPDX-License-Identifier: MIT OR Apache-2.0

//! The signed, static plugin index.
//!
//! The index is embedded at build time and is never fetched, refreshed or extended at runtime.
//! Its signature covers the compact JSON bytes of the entry list, so adding, reordering or editing
//! an entry invalidates the signature.

use std::collections::HashSet;

use auro_sign::{PublicKey, SignError};
use semver::Version;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

/// Base64 Ed25519 public key that verifies the shipped bootstrap index.
///
/// The maintainers' key signs the empty index committed at `index/plugins.json`; the matching
/// secret is deliberately not part of this repository. Rotation and revocation of this pin is an
/// open decision owned by the maintainers and is not implemented here: changing the pin requires a
/// reviewed source change and a new signed index.
pub const EMBEDDED_INDEX_PUBLIC_KEY_BASE64: &str = "HNZGhikWjII4PrSHv9MtiPl9GdZx+87TNPAK4PmDgKY=";

/// Largest accepted signed index, in bytes.
pub const MAX_INDEX_BYTES: usize = 64 * 1024;
/// Largest accepted number of index entries.
pub const MAX_INDEX_ENTRIES: usize = 1024;

const EMBEDDED_INDEX_JSON: &str = include_str!("../index/plugins.json");

/// Why a signed index was rejected.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IndexError {
    /// The signed index exceeded the 64 KiB input limit.
    #[error("index is {0} bytes; limit is 65536")]
    TooLarge(usize),

    /// The bytes were not a JSON object with `entries` and `signature`.
    #[error("invalid index JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),

    /// Too many entries were declared.
    #[error("too many index entries: {0}; limit is 1024")]
    TooManyEntries(usize),

    /// The same plugin id appeared twice.
    #[error("duplicate plugin id `{0}`")]
    DuplicateId(String),

    /// Entries must be ordered by id so the signed bytes have one canonical form.
    #[error("index entries are not sorted by id")]
    UnsortedEntries,

    /// An entry id was not a reverse-DNS-ish identifier.
    #[error("invalid plugin id `{0}`")]
    InvalidId(String),

    /// An entry version was not a semantic version.
    #[error("invalid semver version `{0}`")]
    InvalidVersion(String),

    /// An entry crate name was not a package identifier.
    #[error("invalid crate name `{0}`")]
    InvalidCrateName(String),

    /// An entry artifact digest was not 64 lowercase hex digits.
    #[error("invalid artifact SHA-256 digest `{0}`")]
    InvalidHash(String),

    /// An entry manifest digest was not 64 lowercase hex digits.
    #[error("invalid manifest SHA-256 digest `{0}`")]
    InvalidManifestHash(String),

    /// The signature was malformed or did not verify over the entry list.
    #[error("index signature is invalid: {0}")]
    InvalidSignature(#[source] SignError),

    /// The embedded public key could not be decoded.
    #[error("embedded public key is invalid: {0}")]
    InvalidKey(#[source] SignError),

    /// An individual string exceeded the 1024-byte limit.
    #[error("field `{field}` is {length} bytes; limit is 1024")]
    StringTooLarge {
        /// Wire name of the field.
        field: &'static str,
        /// Measured length in bytes.
        length: usize,
    },
}

/// One pinned plugin in the signed index.
#[derive(Clone, Debug, Serialize)]
pub struct IndexEntry {
    /// Reverse-DNS-ish plugin id.
    pub id: String,
    /// Exact semantic version pinned for this id.
    #[serde(serialize_with = "serialize_version")]
    pub version: Version,
    /// Crate that supplies the plugin artifact.
    pub crate_name: String,
    /// Lowercase hex SHA-256 of the artifact.
    pub artifact_sha256: String,
    /// Lowercase hex SHA-256 of the canonical manifest bytes.
    pub manifest_sha256: String,
}

impl<'de> Deserialize<'de> for IndexEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireIndexEntry::deserialize(deserializer)?;
        entry_from_wire(wire).map_err(serde::de::Error::custom)
    }
}

/// An immutable, verified index.
#[derive(Clone, Debug)]
pub struct PluginIndex {
    /// Entries sorted by id.
    pub entries: Vec<IndexEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSignedIndex {
    entries: Vec<WireIndexEntry>,
    signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIndexEntry {
    id: String,
    version: String,
    crate_name: String,
    artifact_sha256: String,
    manifest_sha256: String,
}

fn serialize_version<S>(version: &Version, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&version.to_string())
}

fn check_string(field: &'static str, value: &str) -> Result<(), IndexError> {
    if value.len() > 1024 {
        return Err(IndexError::StringTooLarge {
            field,
            length: value.len(),
        });
    }
    Ok(())
}

fn entry_from_wire(wire: WireIndexEntry) -> Result<IndexEntry, IndexError> {
    check_string("entries.id", &wire.id)?;
    check_string("entries.version", &wire.version)?;
    check_string("entries.crate_name", &wire.crate_name)?;
    check_string("entries.artifact_sha256", &wire.artifact_sha256)?;
    check_string("entries.manifest_sha256", &wire.manifest_sha256)?;

    crate::validate::plugin_id(&wire.id).map_err(|_| IndexError::InvalidId(wire.id.clone()))?;
    let version = wire
        .version
        .parse::<Version>()
        .map_err(|_| IndexError::InvalidVersion(wire.version.clone()))?;
    crate::validate::crate_name(&wire.crate_name)
        .map_err(|_| IndexError::InvalidCrateName(wire.crate_name.clone()))?;
    if !crate::hex::is_sha256(&wire.artifact_sha256) {
        return Err(IndexError::InvalidHash(wire.artifact_sha256.clone()));
    }
    if !crate::hex::is_sha256(&wire.manifest_sha256) {
        return Err(IndexError::InvalidManifestHash(
            wire.manifest_sha256.clone(),
        ));
    }
    Ok(IndexEntry {
        id: wire.id,
        version,
        crate_name: wire.crate_name,
        artifact_sha256: wire.artifact_sha256,
        manifest_sha256: wire.manifest_sha256,
    })
}

impl PluginIndex {
    /// Verifies and parses a signed index.
    ///
    /// The signature is checked over `serde_json::to_vec(&entries)`: compact JSON, entry fields in
    /// declaration order, no whitespace. Verification happens only after every entry has passed
    /// validation, so a hostile entry cannot reach the registry even with a valid signature.
    /// This function never reads the network or the filesystem.
    pub fn parse_signed(bytes: &[u8], key: &PublicKey) -> Result<Self, IndexError> {
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(IndexError::TooLarge(bytes.len()));
        }
        let wire =
            serde_json::from_slice::<WireSignedIndex>(bytes).map_err(IndexError::InvalidJson)?;
        if wire.entries.len() > MAX_INDEX_ENTRIES {
            return Err(IndexError::TooManyEntries(wire.entries.len()));
        }
        check_string("signature", &wire.signature)?;
        let entries = wire
            .entries
            .into_iter()
            .map(entry_from_wire)
            .collect::<Result<Vec<_>, _>>()?;

        let mut ids = HashSet::with_capacity(entries.len());
        for entry in &entries {
            if !ids.insert(entry.id.as_str()) {
                return Err(IndexError::DuplicateId(entry.id.clone()));
            }
        }
        if entries.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(IndexError::UnsortedEntries);
        }

        let payload = match serde_json::to_vec(&entries) {
            Ok(payload) => payload,
            Err(error) => unreachable!("index entry serialization is infallible: {error}"),
        };
        key.verify(&payload, &wire.signature)
            .map_err(IndexError::InvalidSignature)?;
        Ok(Self { entries })
    }

    /// Parses the index embedded at build time with the pinned public key.
    ///
    /// The shipped key signs the empty bootstrap index only. Rotation, revocation and any move to
    /// a dynamic registry are open decisions for the maintainers, not behaviour implemented here.
    pub fn embedded() -> Result<Self, IndexError> {
        let key = PublicKey::from_base64(EMBEDDED_INDEX_PUBLIC_KEY_BASE64)
            .map_err(IndexError::InvalidKey)?;
        Self::parse_signed(EMBEDDED_INDEX_JSON.as_bytes(), &key)
    }

    /// Looks up a pinned entry by plugin id.
    pub fn get(&self, id: &str) -> Option<&IndexEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}
