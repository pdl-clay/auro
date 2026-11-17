// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::Path;

use auro_core::AuroError;
use auro_sign::SigningKey;
use auro_update::Manifest;

/// Creates a signing key and prints its public key.
pub fn generate_key(out: &Path) -> Result<(), AuroError> {
    let key = SigningKey::generate();
    key.save_to(out)
        .map_err(|error| AuroError::Message(format!("cannot save signing key: {error}")))?;
    println!("public key: {}", key.public_key().to_base64());
    Ok(())
}

/// Builds, signs and writes an update manifest for a local artifact.
#[allow(clippy::too_many_arguments)]
pub fn manifest(
    artifact: &Path,
    version: &str,
    url: &str,
    key_path: &Path,
    out: &Path,
    notes: &str,
    platform: Option<&str>,
    published_at: &str,
) -> Result<(), AuroError> {
    let key = SigningKey::load_from(key_path)
        .map_err(|error| AuroError::Message(format!("cannot load signing key: {error}")))?;
    let platform = platform
        .map(str::to_owned)
        .unwrap_or_else(auro_update::current_platform);
    let manifest = Manifest::for_artifact(artifact, version, platform, url, notes, published_at)
        .and_then(|manifest| manifest.sign(&key))
        .map_err(|error| AuroError::Message(format!("cannot create manifest: {error}")))?;
    let json = manifest
        .to_json()
        .map_err(|error| AuroError::Message(format!("cannot encode manifest: {error}")))?;
    std::fs::write(out, json)?;
    println!("{}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_a_manifest_for_a_local_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let artifact = temp.path().join("app.tar.gz");
        let key_path = temp.path().join("release.key");
        let out = temp.path().join("latest.json");
        std::fs::write(&artifact, b"artifact bytes").expect("artifact");

        let key = SigningKey::generate();
        key.save_to(&key_path).expect("key");

        manifest(
            &artifact,
            "1.2.3",
            "https://example.test/app.tar.gz",
            &key_path,
            &out,
            "notes",
            Some("linux-x86_64"),
            "2026-11-17T12:00:00Z",
        )
        .expect("manifest");

        let parsed =
            Manifest::from_json(&std::fs::read_to_string(&out).expect("json")).expect("parse");
        parsed.verify(&key.public_key()).expect("signature");
        parsed
            .verify_artifact(&artifact, &key.public_key())
            .expect("artifact");
        assert_eq!(parsed.platform, "linux-x86_64");
        assert_eq!(parsed.version, "1.2.3");
    }

    #[test]
    fn generate_key_writes_a_loadable_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("release.key");
        generate_key(&path).expect("key");
        let loaded = SigningKey::load_from(&path).expect("load");
        assert!(!loaded.public_key().to_base64().is_empty());
    }
}
