// SPDX-License-Identifier: MIT OR Apache-2.0

//! Signed static plugin discovery and verified plugin subjects.
//!
//! Auro does not fetch plugins at runtime. The embedded, signed [`index::PluginIndex`] maps a
//! plugin id to one pinned version, artifact hash and manifest digest; the app adds an explicit
//! allowlist, and [`registry::PluginRegistry`] derives the effective subject from the verified
//! identity. A manifest-declared subject is metadata only and never reaches a policy check.
//!
//! This crate performs no process spawning, no IPC, no network access and no path I/O. The only
//! embedded bytes are the shipped index, included at build time with `include_str!`.

/// Private per-user channel for one plugin instance.
pub mod channel;
/// In-process native plugin backend behind the `plugins-dylib` feature.
pub mod dylib;
/// Capability-checked file effects at the call site.
pub mod effects;
/// Out-of-process host and typed command dispatch.
pub mod host;
/// The signed, static index and its verification rules.
pub mod index;
/// Hostile-input validation of plugin manifests.
pub mod manifest;
/// Typed, versioned RPC messages and fail-closed negotiation.
pub mod protocol;
/// Verification of plugin identity into effective subjects.
pub mod registry;

pub use index::{EMBEDDED_INDEX_PUBLIC_KEY_BASE64, IndexEntry, IndexError, PluginIndex};
pub use manifest::{
    MANIFEST_FORMAT_VERSION, MAX_CAPABILITIES, MAX_COMMANDS, MAX_MANIFEST_BYTES, MAX_STRING_BYTES,
    ManifestError, ManifestGrant, PluginManifest,
};
pub use registry::{
    AllowedPlugin, PluginLoadError, PluginPolicyExt, PluginRegistry, VerifiedSubject,
};

/// The exact SDK version a plugin manifest must declare.
///
/// Comparison is string equality, not semver compatibility: a plugin built against a different
/// `auro-plugins` release is rejected. The version comes from this crate's package version so a
/// release cannot accidentally accept a manifest pinned to the previous SDK.
pub fn sdk_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Hex encoding and digest validation shared by the manifest and index parsers.
pub(crate) mod hex {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    /// Lowercase hex for a byte slice.
    pub(crate) fn encode(bytes: impl AsRef<[u8]>) -> String {
        let bytes = bytes.as_ref();
        let mut out = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            out.push(DIGITS[(byte >> 4) as usize] as char);
            out.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        out
    }

    /// Whether `text` is exactly 64 lowercase hex digits.
    pub(crate) fn is_sha256(text: &str) -> bool {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// Lexical validation of untrusted identifiers. No filesystem operations are performed.
pub(crate) mod validate {
    /// Maximum byte length of an id from either the manifest or the index.
    const MAX_ID_BYTES: usize = 128;
    /// Maximum byte length of a command name.
    const MAX_COMMAND_BYTES: usize = 256;
    /// Maximum byte length of an instance id.
    const MAX_INSTANCE_BYTES: usize = 128;

    /// Reverse-DNS-ish id: at least two nonempty labels of `[A-Za-z0-9_-]`, no leading or
    /// trailing dot. Kept ASCII-only so the verified subject has no case or Unicode ambiguity.
    pub(crate) fn plugin_id(id: &str) -> Result<(), &'static str> {
        if id.is_empty() {
            return Err("id is empty");
        }
        if id.len() > MAX_ID_BYTES {
            return Err("id exceeds 128 bytes");
        }
        if !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err("id contains characters outside [A-Za-z0-9_.-]");
        }
        if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
            return Err("id has an empty dot-separated label");
        }
        if !id.contains('.') {
            return Err("id must have at least two dot-separated labels");
        }
        Ok(())
    }

    /// A relative entry point made of simple segments: no root, prefix, `.`/`..` or separators
    /// other than `/`. Each segment is one ordinary path component.
    pub(crate) fn entrypoint(value: &str) -> Result<(), &'static str> {
        if value.is_empty() {
            return Err("entrypoint is empty");
        }
        if value.starts_with('/') || value.contains('\\') || value.contains(':') {
            return Err("entrypoint must be a relative path with no prefix");
        }
        if value.contains("//") {
            return Err("entrypoint has an empty segment");
        }
        for segment in value.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err("entrypoint has an empty or traversal segment");
            }
            if segment.starts_with('.') || segment.contains("..") {
                return Err("entrypoint has a hidden or traversal segment");
            }
            if !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return Err("entrypoint segment has invalid characters");
            }
        }
        Ok(())
    }

    /// Command names are stable dispatch keys: ASCII `[A-Za-z0-9_]+` only.
    pub(crate) fn command(name: &str) -> Result<(), &'static str> {
        if name.is_empty() {
            return Err("command name is empty");
        }
        if name.len() > MAX_COMMAND_BYTES {
            return Err("command name exceeds 256 bytes");
        }
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("command name must be [A-Za-z0-9_]+");
        }
        Ok(())
    }

    /// Instance ids are part of the verified subject, so they use the same ASCII subset.
    pub(crate) fn instance_id(value: &str) -> Result<(), &'static str> {
        if value.is_empty() {
            return Err("instance id is empty");
        }
        if value.len() > MAX_INSTANCE_BYTES {
            return Err("instance id exceeds 128 bytes");
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("instance id must be [A-Za-z0-9_-]+");
        }
        Ok(())
    }

    /// Crate names are package identifiers, not paths.
    pub(crate) fn crate_name(value: &str) -> Result<(), &'static str> {
        if value.is_empty() {
            return Err("crate name is empty");
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("crate name must be [A-Za-z0-9_-]+");
        }
        Ok(())
    }
}
