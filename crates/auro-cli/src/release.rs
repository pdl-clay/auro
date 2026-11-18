// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::Path;

use auro_core::AuroError;
use auro_sign::SigningKey;
use auro_update::{Delta as UpdateDelta, Manifest, Rollout};

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
    rollout_percentage: Option<u8>,
    delta_from: Option<&Path>,
    delta_from_version: Option<&str>,
    delta_url: Option<&str>,
    delta_out: Option<&Path>,
) -> Result<(), AuroError> {
    let key = SigningKey::load_from(key_path)
        .map_err(|error| AuroError::Message(format!("cannot load signing key: {error}")))?;
    let platform = platform
        .map(str::to_owned)
        .unwrap_or_else(auro_update::current_platform);
    let delta = match (delta_from, delta_from_version, delta_url, delta_out) {
        (None, None, None, None) => None,
        (Some(from), Some(from_version), Some(url), Some(out)) => {
            let patch = auro_update::create_delta(from, artifact)
                .map_err(|error| AuroError::Message(format!("cannot create delta: {error}")))?;
            std::fs::write(out, &patch)?;
            Some(UpdateDelta {
                from_version: from_version.to_owned(),
                from_sha256: auro_sign::sha256_file(from).map_err(|error| {
                    AuroError::Message(format!("cannot hash delta base: {error}"))
                })?,
                url: url.to_owned(),
                size: patch.len() as u64,
                sha256: auro_sign::sha256_file(out)
                    .map_err(|error| AuroError::Message(format!("cannot hash delta: {error}")))?,
            })
        }
        _ => {
            return Err(AuroError::Message(
                "delta options must be supplied together".to_owned(),
            ));
        }
    };
    let mut manifest =
        Manifest::for_artifact(artifact, version, platform, url, notes, published_at)
            .map_err(|error| AuroError::Message(format!("cannot create manifest: {error}")))?;
    if let Some(delta) = delta {
        manifest = manifest.with_delta(delta);
    }
    if let Some(percentage) = rollout_percentage {
        manifest = manifest.with_rollout(Rollout { percentage });
    }
    let manifest = manifest
        .sign(&key)
        .map_err(|error| AuroError::Message(format!("cannot create manifest: {error}")))?;
    let json = manifest
        .to_json()
        .map_err(|error| AuroError::Message(format!("cannot encode manifest: {error}")))?;
    std::fs::write(out, json)?;
    println!("{}", out.display());
    Ok(())
}

/// Submits an artifact to `notarytool` and optionally staples the ticket.
pub fn notarize(artifact: &Path, profile: &str, skip_staple: bool) -> Result<(), AuroError> {
    auro_bundle::MacNotarizer::new().notarize(artifact, profile, skip_staple)
}

/// Writes a bsdiff-compatible patch from `from` to `to`.
pub fn delta(from: &Path, to: &Path, out: &Path) -> Result<(), AuroError> {
    let patch = auro_update::create_delta(from, to)
        .map_err(|error| AuroError::Message(format!("cannot create delta: {error}")))?;
    std::fs::write(out, patch)?;
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
            None,
            None,
            None,
            None,
            None,
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
    fn writes_delta_metadata_and_rollout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("old.bin");
        let artifact = temp.path().join("new.bin");
        let key_path = temp.path().join("release.key");
        let out = temp.path().join("latest.json");
        let patch = temp.path().join("app.patch");
        std::fs::write(&base, b"old application bytes").expect("base");
        std::fs::write(&artifact, b"new application bytes").expect("artifact");
        let key = SigningKey::generate();
        key.save_to(&key_path).expect("key");

        manifest(
            &artifact,
            "1.2.3",
            "https://example.test/app.bin",
            &key_path,
            &out,
            "notes",
            Some("linux-x86_64"),
            "2026-11-18T00:00:00Z",
            Some(25),
            Some(&base),
            Some("1.0.0"),
            Some("https://example.test/app.patch"),
            Some(&patch),
        )
        .expect("manifest");

        assert!(patch.is_file());
        let parsed =
            Manifest::from_json(&std::fs::read_to_string(&out).expect("json")).expect("parse");
        parsed.verify(&key.public_key()).expect("signature");
        assert_eq!(parsed.rollout, Some(Rollout { percentage: 25 }));
        let delta = parsed.delta.expect("delta");
        assert_eq!(delta.from_version, "1.0.0");
        assert_eq!(
            delta.from_sha256,
            auro_sign::sha256_file(&base).expect("hash")
        );
        assert_eq!(delta.url, "https://example.test/app.patch");
    }

    #[test]
    fn creates_a_patch_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let from = temp.path().join("old.bin");
        let to = temp.path().join("new.bin");
        let out = temp.path().join("app.patch");
        std::fs::write(&from, b"old").expect("from");
        std::fs::write(&to, b"new").expect("to");
        delta(&from, &to, &out).expect("delta");
        assert!(!std::fs::read(&out).expect("patch").is_empty());
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
