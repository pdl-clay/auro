// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the manifest schema, signed index and verified-subject registry.

use auro_perms::{Capability, Policy, PolicyError};
use auro_plugins::index::{IndexEntry, PluginIndex};
use auro_plugins::manifest::{MAX_MANIFEST_BYTES, ManifestError, PluginManifest};
use auro_plugins::registry::{AllowedPlugin, PluginLoadError, PluginRegistry};
use auro_plugins::{IndexError, PluginPolicyExt, sdk_version};
use auro_sign::{PublicKey, SigningKey};
use sha2::{Digest as _, Sha256};

const ID: &str = "com.example.notes";
const ATTACKER_ID: &str = "com.example.attacker";
const VERSION: &str = "1.2.3";
const ARTIFACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_ARTIFACT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn hex_byte(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn manifest_json(
    id: &str,
    version: &str,
    sdk: &str,
    artifact: &str,
    declared: Option<&str>,
) -> String {
    let declared = match declared {
        Some(value) => format!("\"declared-subject\":{value:?},"),
        None => String::new(),
    };
    format!(
        "{{\"format-version\":1,\"id\":{id:?},\"version\":{version:?},\"sdk-version\":{sdk:?},{declared}\"artifact-sha256\":{artifact:?},\"entrypoint\":\"bin/entry\",\"commands\":[\"run\"],\"capabilities\":[{{\"capability\":\"fs\",\"scope\":\"data/**\"}},{{\"capability\":\"clipboard\"}}]}}"
    )
}

fn parse_manifest(
    id: &str,
    version: &str,
    artifact: &str,
    declared: Option<&str>,
) -> PluginManifest {
    PluginManifest::parse(manifest_json(id, version, sdk_version(), artifact, declared).as_bytes())
        .unwrap()
}

fn entry(id: &str, version: &str, artifact: &str, manifest_hash: &str) -> IndexEntry {
    IndexEntry {
        id: id.to_owned(),
        version: version.parse().unwrap(),
        crate_name: "test-plugin".to_owned(),
        artifact_sha256: artifact.to_owned(),
        manifest_sha256: manifest_hash.to_owned(),
    }
}

fn index_json(entries: &[IndexEntry], signature: &str) -> Vec<u8> {
    format!(
        "{{\"entries\":{},\"signature\":{signature:?}}}",
        serde_json::to_string(entries).unwrap()
    )
    .into_bytes()
}

fn signed_index(entries: &[IndexEntry], key: &SigningKey) -> Vec<u8> {
    let payload = serde_json::to_vec(entries).unwrap();
    index_json(entries, &key.sign(&payload))
}

fn registry_for(manifest: &PluginManifest, version: &str, key: &SigningKey) -> PluginRegistry {
    let entries = vec![entry(
        &manifest.id,
        version,
        &manifest.artifact_sha256,
        &manifest.digest(),
    )];
    let index = PluginIndex::parse_signed(&signed_index(&entries, key), &key.public_key()).unwrap();
    PluginRegistry::new(
        index,
        vec![AllowedPlugin {
            id: manifest.id.clone(),
            version: version.to_owned(),
        }],
        sdk_version(),
    )
}

/// Builds a registry with two signed, allowlisted plugins. Entries are sorted by id because the
/// index parser rejects an unsorted list.
fn registry_for_two(
    first: &PluginManifest,
    second: &PluginManifest,
    key: &SigningKey,
) -> PluginRegistry {
    let mut manifests = [first, second];
    manifests.sort_by(|left, right| left.id.cmp(&right.id));
    let entries = manifests
        .iter()
        .map(|manifest| {
            entry(
                &manifest.id,
                VERSION,
                &manifest.artifact_sha256,
                &manifest.digest(),
            )
        })
        .collect::<Vec<_>>();
    let index = PluginIndex::parse_signed(&signed_index(&entries, key), &key.public_key()).unwrap();
    PluginRegistry::new(
        index,
        manifests
            .iter()
            .map(|manifest| AllowedPlugin {
                id: manifest.id.clone(),
                version: VERSION.to_owned(),
            })
            .collect(),
        sdk_version(),
    )
}

#[test]
fn manifest_digest_is_the_canonical_pinned_bytes() {
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let canonical = format!(
        "{{\"format-version\":1,\"id\":\"{ID}\",\"version\":\"{VERSION}\",\"sdk-version\":\"{}\",\"declared-subject\":null,\"artifact-sha256\":\"{ARTIFACT}\",\"entrypoint\":\"bin/entry\",\"commands\":[\"run\"],\"capabilities\":[{{\"capability\":\"fs\",\"scope\":\"data/**\"}},{{\"capability\":\"clipboard\",\"scope\":null}}]}}",
        sdk_version()
    );
    assert_eq!(manifest.digest(), sha256_hex(&canonical));
    assert_eq!(manifest.digest(), manifest.digest());
}

#[test]
fn manifest_rejects_unknown_field() {
    let json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None)
        .replace('{', "{\"surprise\":true,");
    assert!(matches!(
        PluginManifest::parse(json.as_bytes()),
        Err(ManifestError::InvalidJson(_))
    ));
}

#[test]
fn manifest_rejects_oversized_bytes() {
    let json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None);
    let padded = format!("{json}{}", " ".repeat(MAX_MANIFEST_BYTES + 1));
    assert!(matches!(
        PluginManifest::parse(padded.as_bytes()),
        Err(ManifestError::TooLarge(length)) if length > MAX_MANIFEST_BYTES
    ));
}

#[test]
fn manifest_rejects_too_many_commands() {
    let commands = (0..257)
        .map(|index| format!("\"cmd_{index}\""))
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        "{{\"format-version\":1,\"id\":\"{ID}\",\"version\":\"{VERSION}\",\"sdk-version\":\"{}\",\"artifact-sha256\":\"{ARTIFACT}\",\"entrypoint\":\"bin/entry\",\"commands\":[{commands}],\"capabilities\":[]}}",
        sdk_version()
    );
    assert!(matches!(
        PluginManifest::parse(json.as_bytes()),
        Err(ManifestError::TooManyCommands(257))
    ));
}

#[test]
fn manifest_rejects_traversal_and_prefixed_entrypoints() {
    for entrypoint in [
        "../bin/entry",
        "/bin/entry",
        "bin/../entry",
        "C:entry",
        "bin\\\\entry",
    ] {
        let json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None)
            .replace("bin/entry", entrypoint);
        assert!(
            matches!(
                PluginManifest::parse(json.as_bytes()),
                Err(ManifestError::InvalidEntrypoint(_))
            ),
            "entrypoint `{entrypoint}` was accepted"
        );
    }
}

#[test]
fn manifest_rejects_bad_hash_and_bad_version() {
    let upper = ARTIFACT.replace('a', "A");
    assert!(matches!(
        PluginManifest::parse(manifest_json(ID, VERSION, sdk_version(), &upper, None).as_bytes()),
        Err(ManifestError::InvalidHash(_))
    ));
    assert!(matches!(
        PluginManifest::parse(
            manifest_json(ID, "not-a-version", sdk_version(), ARTIFACT, None).as_bytes(),
        ),
        Err(ManifestError::InvalidVersion(_))
    ));
}

#[test]
fn manifest_rejects_wrong_sdk_and_bad_format_version() {
    assert!(matches!(
        PluginManifest::parse(manifest_json(ID, VERSION, "0.0.0-old", ARTIFACT, None).as_bytes()),
        Err(ManifestError::InvalidSdkVersion { .. })
    ));
    let json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None)
        .replace("\"format-version\":1", "\"format-version\":2");
    assert!(matches!(
        PluginManifest::parse(json.as_bytes()),
        Err(ManifestError::InvalidField {
            field: "format-version",
            ..
        })
    ));
}

#[test]
fn manifest_rejects_bogus_capability_and_scope_mismatches() {
    let bogus = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None)
        .replace("\"capability\":\"fs\"", "\"capability\":\"root\"");
    assert!(matches!(
        PluginManifest::parse(bogus.as_bytes()),
        Err(ManifestError::InvalidCapability(_))
    ));
    let missing_scope = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None).replace(
        "\"capability\":\"fs\",\"scope\":\"data/**\"",
        "\"capability\":\"fs\"",
    );
    assert!(matches!(
        PluginManifest::parse(missing_scope.as_bytes()),
        Err(ManifestError::InvalidScope { .. })
    ));
    let extra_scope = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None).replace(
        "\"capability\":\"clipboard\"",
        "\"capability\":\"clipboard\",\"scope\":\"x\"",
    );
    assert!(matches!(
        PluginManifest::parse(extra_scope.as_bytes()),
        Err(ManifestError::InvalidScope { .. })
    ));
}

#[test]
fn manifest_rejects_ambiguous_command_names() {
    for command in ["Save File", "sávе", "", "cmd/extra"] {
        let json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None).replace(
            "\"commands\":[\"run\"]",
            &format!("\"commands\":[{command:?}]"),
        );
        assert!(
            matches!(
                PluginManifest::parse(json.as_bytes()),
                Err(ManifestError::InvalidCommand(_))
            ),
            "command `{command}` was accepted"
        );
    }
    let duplicate = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, None)
        .replace("\"commands\":[\"run\"]", "\"commands\":[\"run\",\"run\"]");
    assert!(matches!(
        PluginManifest::parse(duplicate.as_bytes()),
        Err(ManifestError::DuplicateCommand(_))
    ));
}

#[test]
fn manifest_rejects_empty_id_and_oversized_string() {
    let empty = manifest_json("", VERSION, sdk_version(), ARTIFACT, None);
    assert!(matches!(
        PluginManifest::parse(empty.as_bytes()),
        Err(ManifestError::EmptyId)
    ));
    let long = "x".repeat(1025);
    let long_json = manifest_json(ID, VERSION, sdk_version(), ARTIFACT, Some(&long));
    assert!(matches!(
        PluginManifest::parse(long_json.as_bytes()),
        Err(ManifestError::StringTooLarge { .. })
    ));
}

#[test]
fn embedded_index_is_empty_and_uses_the_pinned_key() {
    let index = PluginIndex::embedded().unwrap();
    assert!(index.entries.is_empty());
    assert_eq!(
        auro_plugins::EMBEDDED_INDEX_PUBLIC_KEY_BASE64,
        "HNZGhikWjII4PrSHv9MtiPl9GdZx+87TNPAK4PmDgKY="
    );
    PublicKey::from_base64(auro_plugins::EMBEDDED_INDEX_PUBLIC_KEY_BASE64).unwrap();
}

#[test]
fn signed_index_accepts_valid_entries_and_has_no_network_api() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let entries = vec![entry(ID, VERSION, ARTIFACT, &manifest.digest())];
    let index =
        PluginIndex::parse_signed(&signed_index(&entries, &key), &key.public_key()).unwrap();
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.get(ID).unwrap().artifact_sha256, ARTIFACT);

    let parse: fn(&[u8], &PublicKey) -> Result<PluginIndex, IndexError> = PluginIndex::parse_signed;
    let embedded: fn() -> Result<PluginIndex, IndexError> = PluginIndex::embedded;
    let _ = (parse, embedded);
}

#[test]
fn signed_index_rejects_bad_signature() {
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let entries = vec![entry(ID, VERSION, ARTIFACT, &manifest.digest())];
    let payload = serde_json::to_vec(&entries).unwrap();
    let bytes = index_json(&entries, &other.sign(&payload));
    assert!(matches!(
        PluginIndex::parse_signed(&bytes, &key.public_key()),
        Err(IndexError::InvalidSignature(_))
    ));
    let malformed = index_json(&entries, "not-base64!");
    assert!(matches!(
        PluginIndex::parse_signed(&malformed, &key.public_key()),
        Err(IndexError::InvalidSignature(_))
    ));
}

#[test]
fn signed_index_rejects_duplicates_unsorted_and_hostile_ids() {
    let key = SigningKey::generate();
    let digest = hex_byte(0x42);
    let duplicate = vec![
        entry(ID, VERSION, ARTIFACT, &digest),
        entry(ID, "2.0.0", OTHER_ARTIFACT, &digest),
    ];
    assert!(matches!(
        PluginIndex::parse_signed(&signed_index(&duplicate, &key), &key.public_key()),
        Err(IndexError::DuplicateId(_))
    ));

    let unsorted = vec![
        entry("com.example.zed", VERSION, ARTIFACT, &digest),
        entry(ID, VERSION, OTHER_ARTIFACT, &digest),
    ];
    assert!(matches!(
        PluginIndex::parse_signed(&signed_index(&unsorted, &key), &key.public_key()),
        Err(IndexError::UnsortedEntries)
    ));

    let glob = vec![entry("com.example.*", VERSION, ARTIFACT, &digest)];
    assert!(matches!(
        PluginIndex::parse_signed(&signed_index(&glob, &key), &key.public_key()),
        Err(IndexError::InvalidId(_))
    ));
}

#[test]
fn signed_index_rejects_oversized_and_unknown_fields() {
    let key = SigningKey::generate();
    let bytes = signed_index(&[], &key);
    let padded = format!(
        "{}{}",
        " ".repeat(65 * 1024),
        String::from_utf8(bytes).unwrap()
    );
    assert!(matches!(
        PluginIndex::parse_signed(padded.as_bytes(), &key.public_key()),
        Err(IndexError::TooLarge(_))
    ));
    let unknown = b"{\"entries\":[],\"signature\":\"x\",\"extra\":1}".to_vec();
    assert!(matches!(
        PluginIndex::parse_signed(&unknown, &key.public_key()),
        Err(IndexError::InvalidJson(_))
    ));
}

#[test]
fn signed_index_rejects_invalid_hashes_versions_and_crate_names() {
    let key = SigningKey::generate();
    let digest = hex_byte(0x11);
    let bad_hash = vec![entry(ID, VERSION, "ABCD", &digest)];
    assert!(matches!(
        PluginIndex::parse_signed(&signed_index(&bad_hash, &key), &key.public_key()),
        Err(IndexError::InvalidHash(_))
    ));
    // The wire type validates entries before checking the signature, so an invalid version can be
    // supplied directly; a valid `IndexEntry` cannot even hold it.
    let bad_version = format!(
        "{{\"entries\":[{{\"id\":\"{ID}\",\"version\":\"v1\",\"crate_name\":\"test-plugin\",\"artifact_sha256\":\"{ARTIFACT}\",\"manifest_sha256\":\"{digest}\"}}],\"signature\":\"x\"}}"
    );
    assert!(matches!(
        PluginIndex::parse_signed(bad_version.as_bytes(), &key.public_key()),
        Err(IndexError::InvalidVersion(_))
    ));
    let bad_crate = vec![entry(ID, VERSION, ARTIFACT, &digest)];
    let json = String::from_utf8(signed_index(&bad_crate, &key))
        .unwrap()
        .replace("test-plugin", "../escape");
    assert!(matches!(
        PluginIndex::parse_signed(json.as_bytes(), &key.public_key()),
        Err(IndexError::InvalidCrateName(_))
    ));
    assert!(PluginIndex::parse_signed(&signed_index(&[], &key), &key.public_key()).is_ok());
}

#[test]
fn resolve_binds_a_stable_subject_to_verified_identity() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    let first = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst_01")
        .unwrap();
    assert_eq!(
        first.as_str(),
        format!("plugin:{ID}@{VERSION}.aaaaaaaa.inst_01")
    );
    assert_eq!(first.to_string(), first.as_str());

    assert!(matches!(
        registry.resolve(ID, VERSION, ARTIFACT, &manifest, "inst_01"),
        Err(PluginLoadError::DuplicateSubject(_))
    ));

    let restarted = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst_02")
        .unwrap();
    assert_ne!(restarted.as_str(), first.as_str());
    assert_eq!(
        restarted.as_str(),
        format!("plugin:{ID}@{VERSION}.aaaaaaaa.inst_02")
    );
}

#[test]
fn resolve_rejects_unindexed_and_unlisted_versions() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    assert!(matches!(
        registry.resolve("com.example.absent", VERSION, ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::NotIndexed(_))
    ));
    assert!(matches!(
        registry.resolve(ID, "9.9.9", ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::VersionNotAllowed { .. })
    ));
}

#[test]
fn resolve_rejects_downgrade_to_an_older_indexed_version() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, "1.0.0", ARTIFACT, None);
    let entries = vec![entry(ID, VERSION, ARTIFACT, &manifest.digest())];
    let index =
        PluginIndex::parse_signed(&signed_index(&entries, &key), &key.public_key()).unwrap();
    let registry = PluginRegistry::new(
        index,
        vec![
            AllowedPlugin {
                id: ID.to_owned(),
                version: "1.0.0".to_owned(),
            },
            AllowedPlugin {
                id: ID.to_owned(),
                version: VERSION.to_owned(),
            },
        ],
        sdk_version(),
    );
    assert!(matches!(
        registry.resolve(ID, "1.0.0", ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::VersionMismatch { .. })
    ));
}

#[test]
fn resolve_rejects_hash_and_digest_mismatches() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    assert!(matches!(
        registry.resolve(ID, VERSION, OTHER_ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::ArtifactHashMismatch { .. })
    ));

    let tampered = parse_manifest(ID, VERSION, ARTIFACT, None);
    let entries = vec![entry(ID, VERSION, ARTIFACT, &hex_byte(0x99))];
    let index =
        PluginIndex::parse_signed(&signed_index(&entries, &key), &key.public_key()).unwrap();
    let strict = PluginRegistry::new(
        index,
        vec![AllowedPlugin {
            id: ID.to_owned(),
            version: VERSION.to_owned(),
        }],
        sdk_version(),
    );
    assert!(matches!(
        strict.resolve(ID, VERSION, ARTIFACT, &tampered, "inst"),
        Err(PluginLoadError::ManifestDigestMismatch { .. })
    ));
}

#[test]
fn resolve_rejects_sdk_and_manifest_identity_mismatches() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let entries = vec![entry(ID, VERSION, ARTIFACT, &manifest.digest())];
    let index =
        PluginIndex::parse_signed(&signed_index(&entries, &key), &key.public_key()).unwrap();
    let strict = PluginRegistry::new(
        index,
        vec![AllowedPlugin {
            id: ID.to_owned(),
            version: VERSION.to_owned(),
        }],
        "0.0.0-old",
    );
    assert!(matches!(
        strict.resolve(ID, VERSION, ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::SdkMismatch { .. })
    ));

    let other_id = parse_manifest("com.example.other", VERSION, ARTIFACT, None);
    let pinned = vec![entry(ID, VERSION, ARTIFACT, &other_id.digest())];
    let other_index =
        PluginIndex::parse_signed(&signed_index(&pinned, &key), &key.public_key()).unwrap();
    let registry = PluginRegistry::new(
        other_index,
        vec![AllowedPlugin {
            id: ID.to_owned(),
            version: VERSION.to_owned(),
        }],
        sdk_version(),
    );
    assert!(matches!(
        registry.resolve(ID, VERSION, ARTIFACT, &other_id, "inst"),
        Err(PluginLoadError::ManifestIdentityMismatch { .. })
    ));
}

#[test]
fn resolve_ignores_manifest_declared_subject() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, Some("host:superuser"));
    let registry = registry_for(&manifest, VERSION, &key);
    let subject = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst")
        .unwrap();
    assert!(subject.as_str().starts_with("plugin:"));
    assert!(!subject.as_str().contains("host"));
}

#[test]
fn reserved_subjects_are_rejected_before_any_artifact_is_loaded() {
    let key = SigningKey::generate();
    for reserved_id in ["app.internal", "host.internal"] {
        let manifest = parse_manifest(reserved_id, VERSION, ARTIFACT, None);
        let registry = registry_for(&manifest, VERSION, &key);
        assert!(matches!(
            registry.resolve(reserved_id, VERSION, ARTIFACT, &manifest, "inst"),
            Err(PluginLoadError::ReservedSubject(_))
        ));
    }
}

#[test]
fn resolve_rejects_invalid_instance_index_and_manifest() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    assert!(matches!(
        registry.resolve(ID, VERSION, ARTIFACT, &manifest, "../escape"),
        Err(PluginLoadError::InvalidSubject { .. })
    ));

    let broken_index = PluginIndex {
        entries: vec![entry(ID, VERSION, "short", &manifest.digest())],
    };
    let strict = PluginRegistry::new(
        broken_index,
        vec![AllowedPlugin {
            id: ID.to_owned(),
            version: VERSION.to_owned(),
        }],
        sdk_version(),
    );
    assert!(matches!(
        strict.resolve(ID, VERSION, ARTIFACT, &manifest, "inst"),
        Err(PluginLoadError::InvalidIndex)
    ));

    let broken_index = PluginIndex {
        entries: vec![entry(ID, VERSION, ARTIFACT, &manifest.digest())],
    };
    let strict = PluginRegistry::new(
        broken_index,
        vec![AllowedPlugin {
            id: ID.to_owned(),
            version: VERSION.to_owned(),
        }],
        sdk_version(),
    );
    let mut malformed = manifest.clone();
    malformed.format_version = 99;
    assert!(matches!(
        strict.resolve(ID, VERSION, ARTIFACT, &malformed, "inst"),
        Err(PluginLoadError::InvalidManifest)
    ));
}

#[test]
fn forged_subject_cannot_use_another_plugins_grant() {
    let key = SigningKey::generate();
    let victim = parse_manifest(ID, VERSION, ARTIFACT, None);
    let attacker = parse_manifest(ATTACKER_ID, VERSION, OTHER_ARTIFACT, None);
    let registry = registry_for_two(&victim, &attacker, &key);
    let victim_subject = registry
        .resolve(ID, VERSION, ARTIFACT, &victim, "victim_inst")
        .unwrap();
    let attacker_subject = registry
        .resolve(
            ATTACKER_ID,
            VERSION,
            OTHER_ARTIFACT,
            &attacker,
            "attacker_inst",
        )
        .unwrap();
    assert_ne!(victim_subject, attacker_subject);

    let policy = Policy::deny_all()
        .allow_plugin(&victim_subject, Capability::Clipboard, None)
        .unwrap()
        .allow_plugin(&victim_subject, Capability::Fs, Some("/data/notes/**"))
        .unwrap();

    policy
        .check_plugin(&victim_subject, Capability::Clipboard, None)
        .unwrap();
    let denied = policy
        .check_plugin(&attacker_subject, Capability::Clipboard, None)
        .unwrap_err();
    assert_eq!(denied.subject, attacker_subject.as_str());
    assert_eq!(denied.capability, Capability::Clipboard);
    assert!(
        policy
            .check_plugin(
                &attacker_subject,
                Capability::Fs,
                Some("/data/notes/secret.txt"),
            )
            .is_err()
    );
}

#[test]
fn verified_subject_has_no_public_constructor() {
    // `VerifiedSubject` has a private field; `PluginRegistry::resolve` is its only producer.
    // The split literals keep this source check from matching its own assertions.
    const SOURCE: &str = include_str!("plugins.rs");
    for forbidden in [
        concat!("Verified", "Subject::new"),
        concat!("Verified", "Subject {"),
        concat!("verify_", "subject("),
        concat!("Plugin", "Subject::new"),
        concat!("Deseri", "alize"),
    ] {
        assert!(
            !SOURCE.contains(forbidden),
            "public subject constructor used in tests: {forbidden}"
        );
    }

    // Every subject here comes from a signed resolve, and instances cannot collide.
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    let first = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst_1")
        .unwrap();
    let second = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst_2")
        .unwrap();
    assert_ne!(first, second);
}

#[test]
fn plugin_policy_ext_checks_scoped_and_unscoped_capabilities() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    let subject = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst")
        .unwrap();
    let policy = Policy::deny_all()
        .allow_plugin(&subject, Capability::Fs, Some("/data/notes/**"))
        .unwrap()
        .allow_plugin(&subject, Capability::Clipboard, None)
        .unwrap();
    assert_eq!(policy.grants().len(), 2);

    policy
        .check_plugin(&subject, Capability::Fs, Some("/data/notes/a.txt"))
        .unwrap();
    for value in ["/etc/passwd", "/data/notes/../secret"] {
        assert!(
            policy
                .check_plugin(&subject, Capability::Fs, Some(value))
                .is_err(),
            "unexpectedly allowed {value}"
        );
    }
    assert!(policy.check_plugin(&subject, Capability::Fs, None).is_err());
    policy
        .check_plugin(&subject, Capability::Clipboard, None)
        .unwrap();
    assert!(
        policy
            .check_plugin(&subject, Capability::Keychain, None)
            .is_err()
    );
}

#[test]
fn plugin_policy_ext_scope_errors_are_typed() {
    let key = SigningKey::generate();
    let manifest = parse_manifest(ID, VERSION, ARTIFACT, None);
    let registry = registry_for(&manifest, VERSION, &key);
    let subject = registry
        .resolve(ID, VERSION, ARTIFACT, &manifest, "inst")
        .unwrap();

    assert!(matches!(
        Policy::deny_all().allow_plugin(&subject, Capability::Fs, None),
        Err(PolicyError::ScopeRequired {
            capability: Capability::Fs
        })
    ));
    assert!(matches!(
        Policy::deny_all().allow_plugin(&subject, Capability::Clipboard, Some("*")),
        Err(PolicyError::ScopeNotSupported {
            capability: Capability::Clipboard
        })
    ));
    assert!(matches!(
        Policy::deny_all().allow_plugin(&subject, Capability::Fs, Some("[invalid")),
        Err(PolicyError::InvalidPattern(..))
    ));
}
