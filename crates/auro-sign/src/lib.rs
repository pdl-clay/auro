// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ed25519 keys and signatures used by Auro's release artifacts.
//!
//! The secret key is a 32-byte Ed25519 seed encoded as base64 in a small text file. Public keys
//! and signatures are base64 too, so a manifest or a command line can carry them without a binary
//! format. This crate is independent of the bundler and of the UI; `auro-update` builds the signed
//! manifest on top of it.

use std::fmt;
use std::fs;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{
    Signature as DalekSignature, Signer as _, SigningKey as DalekSigningKey, VerifyingKey,
};
use rand_core::OsRng;
use sha2::{Digest as _, Sha256};

/// Why a key, signature or file operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid base64: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("invalid signing key: expected a 32-byte seed")]
    InvalidKey,

    #[error("invalid public key: expected 32 bytes")]
    InvalidPublicKey,

    #[error("invalid signature: expected 64 bytes")]
    InvalidSignature,

    #[error("signature verification failed")]
    Verification,

    #[error("the key file has no key line")]
    MissingKey,
}

/// A secret Ed25519 signing key.
///
/// Debug output never includes the seed.
#[derive(Clone)]
pub struct SigningKey {
    inner: DalekSigningKey,
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningKey")
            .field("public_key", &self.public_key().to_base64())
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// Generates a fresh key from the operating system RNG.
    pub fn generate() -> Self {
        SigningKey {
            inner: DalekSigningKey::generate(&mut OsRng),
        }
    }

    /// Reads a key from a 32-byte base64 seed.
    pub fn from_seed_base64(seed: &str) -> Result<Self, SignError> {
        let bytes = BASE64.decode(seed.trim())?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| SignError::InvalidKey)?;
        Ok(SigningKey {
            inner: DalekSigningKey::from_bytes(&seed),
        })
    }

    /// The base64 seed. Treat this as a secret.
    pub fn seed_base64(&self) -> String {
        BASE64.encode(self.inner.to_bytes())
    }

    /// The matching public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey {
            inner: self.inner.verifying_key(),
        }
    }

    /// Signs bytes and returns a base64 signature.
    pub fn sign(&self, bytes: &[u8]) -> String {
        BASE64.encode(self.inner.sign(bytes).to_bytes())
    }

    /// Loads a key file. Lines starting with `#` and blank lines are ignored.
    pub fn load_from(path: &Path) -> Result<Self, SignError> {
        let text = fs::read_to_string(path)?;
        let seed = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .ok_or(SignError::MissingKey)?;
        Self::from_seed_base64(seed)
    }

    /// Writes the key file with a fixed header and owner-only permissions where supported.
    pub fn save_to(&self, path: &Path) -> Result<(), SignError> {
        fs::write(
            path,
            format!("# Auro signing key v1\n{}\n", self.seed_base64()),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

/// A public key that verifies signatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKey {
    inner: VerifyingKey,
}

impl PublicKey {
    pub fn from_base64(key: &str) -> Result<Self, SignError> {
        let bytes = BASE64.decode(key.trim())?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| SignError::InvalidPublicKey)?;
        let inner = VerifyingKey::from_bytes(&bytes).map_err(|_| SignError::InvalidPublicKey)?;
        Ok(PublicKey { inner })
    }

    pub fn to_base64(&self) -> String {
        BASE64.encode(self.inner.to_bytes())
    }

    /// Verifies a base64 signature over the exact bytes.
    pub fn verify(&self, bytes: &[u8], signature: &str) -> Result<(), SignError> {
        let raw = BASE64.decode(signature.trim())?;
        let raw: [u8; 64] = raw.try_into().map_err(|_| SignError::InvalidSignature)?;
        let signature = DalekSignature::from_bytes(&raw);
        self.inner
            .verify_strict(bytes, &signature)
            .map_err(|_| SignError::Verification)
    }
}

/// Signs a file's contents.
pub fn sign_file(key: &SigningKey, path: &Path) -> Result<String, SignError> {
    Ok(key.sign(&fs::read(path)?))
}

/// Verifies a base64 signature against a file's contents.
pub fn verify_file(key: &PublicKey, path: &Path, signature: &str) -> Result<(), SignError> {
    key.verify(&fs::read(path)?, signature)
}

/// The lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String, SignError> {
    let bytes = fs::read(path)?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_the_exact_bytes() {
        let key = SigningKey::generate();
        let public = key.public_key();
        let signature = key.sign(b"release");
        public.verify(b"release", &signature).expect("valid");
        assert!(public.verify(b"releasf", &signature).is_err());
        assert!(public.verify(b"release", "not base64").is_err());
    }

    #[test]
    fn key_file_round_trips_with_owner_only_permissions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("release.key");
        let key = SigningKey::generate();
        key.save_to(&path).expect("save");
        let loaded = SigningKey::load_from(&path).expect("load");
        assert_eq!(loaded.public_key(), key.public_key());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn signs_and_verifies_a_file_and_hashes_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("artifact");
        fs::write(&path, b"artifact bytes").expect("write");
        let key = SigningKey::generate();
        let signature = sign_file(&key, &path).expect("sign");
        verify_file(&key.public_key(), &path, &signature).expect("verify");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "4659fc0570122b0e0aa14f4ff7c261b1fe51795a01ba79963f462ebf40d7520d"
        );
    }

    #[test]
    fn rejects_bad_key_sizes() {
        assert!(SigningKey::from_seed_base64("AAAA").is_err());
        assert!(PublicKey::from_base64("AAAA").is_err());
    }
}
