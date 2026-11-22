// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed wire protocol for the out-of-process plugin host.
//!
//! The handshake fails closed: both peers must agree on the exact protocol version, SDK version,
//! per-command and per-event schema digests, and capabilities hash before any command is
//! dispatched. Unknown fields are rejected everywhere, and no string subject is ever part of a
//! wire message — the host injects the verified subject from its own loaded instance at the effect
//! site. Timeouts, bounded queues and backpressure are stage 3 concerns and have no wire fields yet.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::manifest::ManifestGrant;

/// Protocol version both peers must declare. A different version is never downgraded.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest accepted serialized JSON frame, in bytes.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// First message sent by a plugin to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// Exact protocol version implemented by the plugin.
    pub protocol_version: u32,
    /// Exact SDK version the plugin was built against.
    pub sdk_version: String,
    /// Lowercase hex SHA-256 of each command/event schema, keyed by schema name.
    pub schema_digests: BTreeMap<String, String>,
    /// Lowercase hex SHA-256 of the canonical capability grants.
    pub capabilities_hash: String,
}

/// Host reply to a valid [`Hello`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HelloAck {
    /// Exact protocol version implemented by the host.
    pub protocol_version: u32,
    /// Lowercase hex SHA-256 of the grants the host applies to this plugin.
    pub capabilities_hash: String,
}

/// A request sent from the host to a plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginCommand {
    /// Read a file the host has already authorized and canonicalized.
    ReadFile {
        /// Path supplied by the app and checked at the effect site.
        path: String,
    },
    /// Write bytes to a file the host has already authorized and canonicalized.
    WriteFile {
        /// Path supplied by the app and checked at the effect site.
        path: String,
        /// File contents.
        bytes: Vec<u8>,
    },
    /// Liveness probe; carries no arguments.
    Ping,
}

/// A versioned request envelope: the only command message the host sends.
///
/// The request id is part of the signed schema, so a plugin can echo it without the host trusting
/// any caller-supplied subject or correlation value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Identifier the reply must echo.
    pub request_id: u64,
    /// Typed command to execute.
    pub command: PluginCommand,
}

/// A reply sent from a plugin to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginReply {
    /// The command completed without a payload.
    Ok {
        /// Identifier of the command being answered.
        request_id: u64,
    },
    /// The command returned file contents.
    FileContent {
        /// Identifier of the command being answered.
        request_id: u64,
        /// File contents.
        bytes: Vec<u8>,
    },
    /// The command failed.
    Error {
        /// Identifier of the command being answered.
        request_id: u64,
        /// Typed failure rendered as text for the host error channel.
        message: String,
    },
}

/// An unsolicited message sent from a plugin to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginEvent {
    /// A watched file changed.
    FileChanged {
        /// Canonical path reported by the plugin.
        path: String,
    },
}

/// Why a plugin handshake was rejected.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NegotiationError {
    /// The plugin declared a protocol version this host does not implement.
    #[error("unsupported protocol version {found}; expected {expected}")]
    UnsupportedProtocol {
        /// Version required by the host.
        expected: u32,
        /// Version declared by the plugin.
        found: u32,
    },

    /// The plugin was built against a different SDK release.
    #[error("unsupported SDK version `{found}`; expected `{expected}`")]
    SdkMismatch {
        /// Version required by the host.
        expected: String,
        /// Version declared by the plugin.
        found: String,
    },

    /// A schema digest did not match the host's copy.
    #[error("schema digest mismatch for `{key}`: expected `{expected}`, found `{found}`")]
    SchemaMismatch {
        /// Name of the command or event schema.
        key: String,
        /// Digest computed by the host.
        expected: String,
        /// Digest declared by the plugin.
        found: String,
    },

    /// The plugin omitted a required schema digest.
    #[error("missing schema digest for `{key}`")]
    MissingSchema {
        /// Name of the missing command or event schema.
        key: String,
    },

    /// The plugin declared a digest the host does not know.
    #[error("unexpected schema digest for `{key}`")]
    UnexpectedSchema {
        /// Name of the unknown schema.
        key: String,
    },

    /// The plugin's capability hash did not match the grants applied by the host.
    #[error("capabilities hash mismatch: expected `{expected}`, found `{found}`")]
    CapabilityMismatch {
        /// Hash computed from the host's grants.
        expected: String,
        /// Hash declared by the plugin.
        found: String,
    },

    /// A frame could not be read, sized or deserialized.
    #[error("invalid protocol message: {0}")]
    InvalidMessage(String),
}

impl From<io::Error> for NegotiationError {
    fn from(error: io::Error) -> Self {
        Self::InvalidMessage(error.to_string())
    }
}

impl From<serde_json::Error> for NegotiationError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidMessage(error.to_string())
    }
}

/// Payload schemas mirrored from the wire variants. The wire variants carry an internal `type`
/// tag; the digest covers the argument/result payload, which is what an SDK author compiles
/// against. These types are private so the digest set stays owned by the protocol version.
#[allow(dead_code)] // Schema-only mirrors: fields exist for the derived JSON schema, never read.
mod payload_schema {
    use schemars::JsonSchema;

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub(super) struct ReadFile {
        pub(super) path: String,
    }

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub(super) struct WriteFile {
        pub(super) path: String,
        pub(super) bytes: Vec<u8>,
    }

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub(super) struct Ping {}

    #[derive(JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub(super) struct FileChanged {
        pub(super) path: String,
    }
}

/// Lowercase hex SHA-256 of a compact, deterministic schema serialization.
fn digest_schema<T: JsonSchema>() -> String {
    let schema = schemars::schema_for!(T);
    // Serializing a generated schema is infallible: it contains only JSON scalars and collections.
    let bytes = serde_json::to_vec(&schema)
        .unwrap_or_else(|error| unreachable!("schema serialization is infallible: {error}"));
    crate::hex::encode(Sha256::digest(bytes))
}

/// Expected digest of every command and event schema, keyed by schema name.
///
/// Keys are `command.request`, `command.read_file`, `command.write_file`, `command.ping` and
/// `event.file_changed`.
/// The map is ordered, so a handshake error names the same key on both peers.
pub fn schema_digests() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "command.read_file".to_owned(),
            digest_schema::<payload_schema::ReadFile>(),
        ),
        (
            "command.write_file".to_owned(),
            digest_schema::<payload_schema::WriteFile>(),
        ),
        (
            "command.ping".to_owned(),
            digest_schema::<payload_schema::Ping>(),
        ),
        ("command.request".to_owned(), digest_schema::<Request>()),
        (
            "event.file_changed".to_owned(),
            digest_schema::<payload_schema::FileChanged>(),
        ),
    ])
}

/// Lowercase hex SHA-256 of the canonical serialization of `grants`.
pub fn capabilities_hash(grants: &[ManifestGrant]) -> String {
    // `ManifestGrant` is a closed, serializable type; its canonical byte form cannot fail.
    let bytes = serde_json::to_vec(grants)
        .unwrap_or_else(|error| unreachable!("grant serialization is infallible: {error}"));
    crate::hex::encode(Sha256::digest(bytes))
}

/// Validate a plugin's handshake data against this host before any command is dispatched.
///
/// Every mismatch is fatal; there is no best-effort downgrade. The check does not inspect any
/// subject because subjects never appear on the wire.
pub fn validate_hello(hello: &Hello, grants: &[ManifestGrant]) -> Result<(), NegotiationError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(NegotiationError::UnsupportedProtocol {
            expected: PROTOCOL_VERSION,
            found: hello.protocol_version,
        });
    }

    if hello.sdk_version != crate::sdk_version() {
        return Err(NegotiationError::SdkMismatch {
            expected: crate::sdk_version().to_owned(),
            found: hello.sdk_version.clone(),
        });
    }

    let expected = schema_digests();
    for (key, digest) in &expected {
        match hello.schema_digests.get(key) {
            None => return Err(NegotiationError::MissingSchema { key: key.clone() }),
            Some(found) if found != digest => {
                return Err(NegotiationError::SchemaMismatch {
                    key: key.clone(),
                    expected: digest.clone(),
                    found: found.clone(),
                });
            }
            Some(_) => {}
        }
    }
    if let Some(key) = hello
        .schema_digests
        .keys()
        .find(|key| !expected.contains_key(*key))
    {
        return Err(NegotiationError::UnexpectedSchema { key: key.clone() });
    }

    let expected = capabilities_hash(grants);
    if hello.capabilities_hash != expected {
        return Err(NegotiationError::CapabilityMismatch {
            expected,
            found: hello.capabilities_hash.clone(),
        });
    }

    Ok(())
}

/// Write one length-prefixed JSON frame. The length is a little-endian `u32`.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "frame is {} bytes; limit is {MAX_FRAME_BYTES}",
                payload.len()
            ),
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)
}

/// Read one length-prefixed JSON frame. Oversized and malformed frames are I/O errors; callers
/// performing a handshake map them to [`NegotiationError::InvalidMessage`].
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame is {length} bytes; limit is {MAX_FRAME_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}
