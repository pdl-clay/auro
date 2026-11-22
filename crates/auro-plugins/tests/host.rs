// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the out-of-process plugin host.
//!
//! The child fixture is this test binary itself: the host spawns it with
//! `AURO_PLUGIN_TEST_CHILD=1` and `--exact plugin_child_mode`. The child connects to the private
//! endpoint, sends a configurable `Hello`, acknowledges the host and answers typed commands.
//! The manifest and signed index are generated around the hash of the current test executable.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use auro_perms::Capability;
use auro_plugins::PluginPolicyExt;
use auro_plugins::channel::PluginChannel;
use auro_plugins::effects::EffectError;
use auro_plugins::host::{
    CommandPolicy, DEFAULT_CALL_TIMEOUT, HostError, PluginAvailability, PluginFailure, PluginHost,
    PluginState, REQUEST_QUEUE_CAPACITY,
};
use auro_plugins::protocol::{
    self, HelloAck, NegotiationError, PROTOCOL_VERSION, PluginCommand, PluginReply,
};
use auro_plugins::{
    AllowedPlugin, ManifestGrant, PluginIndex, PluginLoadError, PluginManifest, PluginRegistry,
    VerifiedSubject, sdk_version,
};
use auro_sign::SigningKey;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const ID: &str = "dev.example.fixture";
const VERSION: &str = "1.0.0";
const INSTANCE: &str = "inst_1";

#[cfg(windows)]
const ENTRYPOINT: &str = "plugin-bin.bat";
#[cfg(not(windows))]
const ENTRYPOINT: &str = "plugin-bin";

fn in_child_mode() -> bool {
    std::env::var_os("AURO_PLUGIN_TEST_CHILD").is_some()
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[derive(Serialize)]
struct SignedEntry<'a> {
    id: &'a str,
    version: &'a str,
    crate_name: &'a str,
    artifact_sha256: &'a str,
    manifest_sha256: &'a str,
}

struct FixtureOptions {
    grant_fs: bool,
    commands: CommandPolicy,
    hello: Option<Value>,
    reply_id: u64,
    reply_kind: &'static str,
    reply_raw: Option<Value>,
    artifact_hash: Option<String>,
    silent: bool,
    child_action: Option<&'static str>,
    reply_delay_ms: u64,
    expect_load_error: bool,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            grant_fs: false,
            commands: CommandPolicy::deny_all(),
            hello: None,
            reply_id: 0, // 0 = echo the request id; nonzero forces a mismatch
            reply_kind: "ok",
            reply_raw: None,
            artifact_hash: None,
            silent: false,
            child_action: None,
            reply_delay_ms: 0,
            expect_load_error: false,
        }
    }
}

struct Fixture {
    _temp: TempDir,
    host: PluginHost,
    allowed: PathBuf,
    load_error: Option<HostError>,
}

/// A tempdir whose own mode is 0700: the host validates existing ancestors before creating the
/// private channel directory, and `tempfile::tempdir()` uses the process umask (often 0755).
fn tempdir_private() -> TempDir {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("temp dir")
    }
    #[cfg(windows)]
    {
        tempfile::tempdir().expect("temp dir")
    }
}

fn fixture(mut options: FixtureOptions) -> Fixture {
    let temp = tempdir_private();
    let root = temp.path().join("root");
    let package = root.join(ID);
    fs::create_dir_all(&package).expect("package dir");
    let allowed = package.join("allowed");
    fs::create_dir_all(&allowed).expect("allowed dir");
    fs::write(allowed.join("ok.txt"), b"inside").expect("allowed file");

    let executable = std::env::current_exe().expect("current exe");
    // Copying the 80+ MiB test binary into every fixture filled tmpfs. A tiny launcher keeps each
    // fixture to a few bytes while still spawning a real child process from the pinned entrypoint.
    #[cfg(windows)]
    let script = format!("@echo off\r\n\"{}\" %*\r\n", executable.display());
    #[cfg(not(windows))]
    let script = format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", executable.display());
    let artifact_hash = options
        .artifact_hash
        .take()
        .unwrap_or_else(|| hex(Sha256::digest(script.as_bytes())));
    let bin = package.join(ENTRYPOINT);
    fs::write(&bin, script.as_bytes()).expect("write launcher");
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("launcher mode");
    }

    let grants = vec![ManifestGrant {
        capability: Capability::Fs,
        scope: Some("**".to_owned()),
    }];
    let manifest_bytes = serde_json::to_vec(&json!({
        "format-version": 1,
        "id": ID,
        "version": VERSION,
        "sdk-version": sdk_version(),
        "declared-subject": null,
        "artifact-sha256": artifact_hash,
        "entrypoint": ENTRYPOINT,
        "commands": ["read_file", "write_file", "ping"],
        "capabilities": grants,
    }))
    .expect("manifest JSON");
    let manifest = PluginManifest::parse(&manifest_bytes).expect("valid manifest");
    fs::write(package.join("manifest.json"), &manifest_bytes).expect("write manifest");
    let digest = manifest.digest();

    let entry = SignedEntry {
        id: ID,
        version: VERSION,
        crate_name: "fixture",
        artifact_sha256: &artifact_hash,
        manifest_sha256: &digest,
    };
    let entries = serde_json::to_vec(&vec![entry]).expect("index entries");
    let key = SigningKey::generate();
    let signature = key.sign(&entries);
    let index_bytes = format!(
        r#"{{"entries":{},"signature":"{}"}}"#,
        String::from_utf8(entries).expect("UTF-8 entries"),
        signature
    );
    let index = PluginIndex::parse_signed(index_bytes.as_bytes(), &key.public_key())
        .expect("valid signed index");
    let allow = vec![AllowedPlugin {
        id: ID.to_owned(),
        version: VERSION.to_owned(),
    }];

    // Resolve once on a probe registry to obtain the verified subject before the policy is built.
    // The host gets a separate registry with the same pin, so its own load does not see a
    // duplicate subject.
    let probe = PluginRegistry::new(index.clone(), allow.clone(), sdk_version());
    let subject: VerifiedSubject = probe
        .resolve(ID, VERSION, &artifact_hash, &manifest, INSTANCE)
        .expect("probe subject");
    let policy = if options.grant_fs {
        auro_perms::Policy::deny_all()
            .allow_plugin(&subject, Capability::Fs, Some("**"))
            .expect("fs grant")
    } else {
        auro_perms::Policy::deny_all()
    };
    let registry = PluginRegistry::new(index, allow, sdk_version());
    let mut host = PluginHost::new(registry, policy, root.clone(), options.commands);
    host.set_private_dir(temp.path().join("private"));

    let hello = options.hello.take().unwrap_or_else(|| {
        json!({
            "protocol_version": PROTOCOL_VERSION,
            "sdk_version": sdk_version(),
            "schema_digests": protocol::schema_digests(),
            "capabilities_hash": protocol::capabilities_hash(&manifest.capabilities),
        })
    });
    host.set_child_env("AURO_PLUGIN_TEST_CHILD", "1");
    host.set_child_env("AURO_PLUGIN_TEST_HELLO", hello.to_string());
    host.set_child_env("AURO_PLUGIN_TEST_REQUEST_ID", "1");
    if options.reply_id != 0 {
        host.set_child_env("AURO_PLUGIN_TEST_REPLY_ID", options.reply_id.to_string());
    }
    host.set_child_env("AURO_PLUGIN_TEST_REPLY", options.reply_kind);
    if let Some(raw) = options.reply_raw {
        host.set_child_env("AURO_PLUGIN_TEST_REPLY_RAW", raw.to_string());
    }
    if options.silent {
        host.set_child_env("AURO_PLUGIN_TEST_SILENT", "1");
    }
    if let Some(action) = options.child_action {
        host.set_child_env("AURO_PLUGIN_TEST_CHILD_ACTION", action);
    }
    if options.reply_delay_ms != 0 {
        host.set_child_env(
            "AURO_PLUGIN_TEST_REPLY_DELAY_MS",
            options.reply_delay_ms.to_string(),
        );
    }
    host.add_child_arg("--exact");
    host.add_child_arg("plugin_child_mode");
    let load_error = host.load(ID, VERSION, INSTANCE).err();
    if !options.expect_load_error {
        assert!(load_error.is_none(), "load failed: {load_error:?}");
    }

    Fixture {
        _temp: temp,
        host,
        allowed,
        load_error,
    }
}

fn canonical_string(path: &std::path::Path) -> String {
    fs::canonicalize(path)
        .expect("canonical path")
        .to_string_lossy()
        .into_owned()
}

fn hello_with(mutate: impl FnOnce(&mut Value)) -> Option<Value> {
    let mut hello = json!({
        "protocol_version": PROTOCOL_VERSION,
        "sdk_version": sdk_version(),
        "schema_digests": protocol::schema_digests(),
        "capabilities_hash": protocol::capabilities_hash(&[ManifestGrant {
            capability: Capability::Fs,
            scope: Some("**".to_owned()),
        }]),
    });
    mutate(&mut hello);
    Some(hello)
}

fn assert_handshake_error(hello: Option<Value>, check: impl Fn(&HostError) -> bool) {
    let mut fixture = fixture(FixtureOptions {
        hello,
        ..FixtureOptions::default()
    });
    let error = fixture.host.handshake(ID).expect_err("handshake must fail");
    assert!(check(&error), "unexpected error: {error:?}");
    assert!(matches!(error, HostError::Handshake(_)));
}

#[test]
fn failed_negotiation_disables_all_later_calls() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        hello: hello_with(|hello| {
            hello["protocol_version"] = json!(999);
        }),
        commands: CommandPolicy::deny_all().allow_ping(),
        ..FixtureOptions::default()
    });
    let error = fixture.host.handshake(ID).expect_err("future protocol");
    assert!(
        matches!(
            error,
            HostError::Handshake(NegotiationError::UnsupportedProtocol { .. })
        ),
        "{error:?}"
    );
    // The instance is terminal: no command and no retry may reuse the refused channel.
    let call = fixture.host.call(ID, 1, PluginCommand::Ping);
    assert!(
        matches!(call, Err(HostError::HandshakeRequired)),
        "{call:?}"
    );
    let retry = fixture.host.handshake(ID);
    assert!(
        matches!(retry, Err(HostError::HandshakeRequired)),
        "{retry:?}"
    );
}

/// Child fixture: speaks the wire protocol from inside this test binary.
#[test]
fn plugin_child_mode() {
    if !in_child_mode() {
        return;
    }
    let endpoint = std::env::var("AURO_PLUGIN_ENDPOINT").expect("endpoint");
    let mut stream = auro_plugins::channel::connect(&endpoint).expect("connect");
    let hello: Value =
        serde_json::from_str(&std::env::var("AURO_PLUGIN_TEST_HELLO").expect("hello"))
            .expect("hello JSON");
    protocol::write_frame(&mut stream, &hello).expect("send hello");
    let ack: HelloAck = match protocol::read_frame(&mut stream) {
        Ok(ack) => ack,
        Err(_) => return, // handshake failed; the host never acknowledges
    };
    assert_eq!(ack.protocol_version, PROTOCOL_VERSION);

    // When set, the environment deliberately forces a mismatched reply id; otherwise the child
    // echoes the id from the typed request envelope.
    let forced_reply_id: Option<u64> = std::env::var("AURO_PLUGIN_TEST_REPLY_ID")
        .ok()
        .and_then(|text| text.parse().ok());
    let kind = std::env::var("AURO_PLUGIN_TEST_REPLY").unwrap_or_default();
    let raw = std::env::var("AURO_PLUGIN_TEST_REPLY_RAW").ok();
    if std::env::var_os("AURO_PLUGIN_TEST_SILENT").is_some() {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    let action = std::env::var("AURO_PLUGIN_TEST_CHILD_ACTION").ok();
    let delay = std::env::var("AURO_PLUGIN_TEST_REPLY_DELAY_MS")
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(0);
    while let Ok(request) = protocol::read_frame::<_, protocol::Request>(&mut stream) {
        if let Some(action) = &action {
            match action.as_str() {
                "abort" => std::process::abort(),
                "exit" => std::process::exit(7),
                "panic" => {
                    std::panic::set_hook(Box::new(|_| {}));
                    panic!("plugin panic");
                }
                _ => {}
            }
        }
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let reply_id = forced_reply_id.unwrap_or(request.request_id);
        if let Some(raw) = &raw {
            let value: Value = serde_json::from_str(raw).expect("raw reply JSON");
            protocol::write_frame(&mut stream, &value).expect("send raw reply");
            continue;
        }
        let reply = match kind.as_str() {
            "error" => PluginReply::Error {
                request_id: reply_id,
                message: "plugin failed".to_owned(),
            },
            _ => match request.command {
                PluginCommand::ReadFile { .. } => PluginReply::FileContent {
                    request_id: reply_id,
                    bytes: b"from plugin".to_vec(),
                },
                _ => PluginReply::Ok {
                    request_id: reply_id,
                },
            },
        };
        protocol::write_frame(&mut stream, &reply).expect("send reply");
    }
}

#[test]
fn valid_handshake_and_typed_ping() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let subject = fixture.host.loaded(ID).expect("loaded").subject().clone();
    assert!(subject.as_str().starts_with("plugin:"));
    let reply = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect("ping reply");
    assert!(matches!(reply, PluginReply::Ok { request_id: 1 }));
}

#[test]
fn handshake_fails_closed_on_protocol_versions() {
    if in_child_mode() {
        return;
    }
    assert_handshake_error(
        hello_with(|hello| hello["protocol_version"] = json!(0)),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::UnsupportedProtocol {
                    expected: PROTOCOL_VERSION,
                    found: 0,
                })
            )
        },
    );
    assert_handshake_error(
        hello_with(|hello| hello["protocol_version"] = json!(PROTOCOL_VERSION + 1)),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::UnsupportedProtocol {
                    expected: PROTOCOL_VERSION,
                    ..
                })
            )
        },
    );
}

#[test]
fn handshake_fails_closed_on_sdk_versions() {
    if in_child_mode() {
        return;
    }
    assert_handshake_error(
        hello_with(|hello| hello["sdk_version"] = json!("0.0.0-old")),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::SdkMismatch { .. })
            )
        },
    );
    assert_handshake_error(
        hello_with(|hello| hello["sdk_version"] = json!("999.0.0-future")),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::SdkMismatch { .. })
            )
        },
    );
}

#[test]
fn handshake_fails_closed_on_schema_digests() {
    if in_child_mode() {
        return;
    }
    assert_handshake_error(
        hello_with(|hello| hello["schema_digests"]["command.ping"] = json!("deadbeef")),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::SchemaMismatch { .. })
            )
        },
    );
    assert_handshake_error(
        hello_with(|hello| {
            hello["schema_digests"]
                .as_object_mut()
                .expect("digests object")
                .remove("event.file_changed");
        }),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::MissingSchema { .. })
            )
        },
    );
    assert_handshake_error(
        hello_with(|hello| hello["schema_digests"]["command.evil"] = json!("deadbeef")),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::UnexpectedSchema { .. })
            )
        },
    );
    assert_handshake_error(
        hello_with(|hello| {
            hello
                .as_object_mut()
                .expect("hello object")
                .remove("schema_digests");
        }),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::InvalidMessage(_))
            )
        },
    );
}

#[test]
fn handshake_fails_closed_on_capability_hash() {
    if in_child_mode() {
        return;
    }
    assert_handshake_error(
        hello_with(|hello| hello["capabilities_hash"] = json!("0".repeat(64))),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::CapabilityMismatch { .. })
            )
        },
    );
}

#[test]
fn handshake_rejects_unknown_subject_field() {
    if in_child_mode() {
        return;
    }
    assert_handshake_error(
        hello_with(|hello| hello["subject"] = json!("host")),
        |error| {
            matches!(
                error,
                HostError::Handshake(NegotiationError::InvalidMessage(_))
            )
        },
    );
}

#[test]
fn command_outside_allowlist_is_denied_before_dispatch() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions::default());
    fixture.host.handshake(ID).expect("valid handshake");
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("ping is not allowed");
    assert!(
        matches!(error, HostError::CommandNotAllowed("ping")),
        "{error:?}"
    );
}

#[test]
fn file_effect_outside_policy_is_denied() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_read_file(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let path = canonical_string(&fixture.allowed.join("ok.txt"));
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::ReadFile { path })
        .expect_err("deny-all policy");
    assert!(
        matches!(error, HostError::Effect(EffectError::Denied(_))),
        "{error:?}"
    );
}

#[test]
fn allowed_file_effects_read_and_write() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        grant_fs: true,
        commands: CommandPolicy::deny_all()
            .allow_read_file()
            .allow_write_file(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let file = fixture.allowed.join("ok.txt");
    let path = canonical_string(&file);
    let reply = fixture
        .host
        .call(
            ID,
            7,
            PluginCommand::WriteFile {
                path: path.clone(),
                bytes: b"written".to_vec(),
            },
        )
        .expect("write");
    assert!(matches!(reply, PluginReply::Ok { request_id: 7 }));
    assert_eq!(fs::read(&file).expect("file after write"), b"written");
    let reply = fixture
        .host
        .call(ID, 8, PluginCommand::ReadFile { path })
        .expect("read");
    assert!(matches!(
        reply,
        PluginReply::FileContent { request_id: 8, .. }
    ));
}

#[test]
fn traversal_path_is_rejected_before_effects() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        grant_fs: true,
        commands: CommandPolicy::deny_all().allow_read_file(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let path = format!("{}/../secret.txt", canonical_string(&fixture.allowed));
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::ReadFile { path })
        .expect_err("traversal");
    assert!(
        matches!(error, HostError::Effect(EffectError::InvalidPath(_))),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_before_effects() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        grant_fs: true,
        commands: CommandPolicy::deny_all().allow_read_file(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    // The target lives outside the grant root, and the request uses the link path itself so the
    // effect site sees the symlink rather than a pre-canonicalized target.
    let outside = fixture._temp.path().join("secret.txt");
    fs::write(&outside, b"secret").expect("outside file");
    let link = fixture.allowed.join("escape.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let error = fixture
        .host
        .call(
            ID,
            1,
            PluginCommand::ReadFile {
                path: link.to_string_lossy().into_owned(),
            },
        )
        .expect_err("symlink escape");
    assert!(
        matches!(error, HostError::Effect(EffectError::Symlink(_))),
        "{error:?}"
    );
}

#[test]
fn mismatched_reply_request_id_is_typed_error() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        reply_id: 2,
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("wrong reply id");
    assert!(
        matches!(
            error,
            HostError::ReplyMismatch {
                expected: 1,
                found: 2
            }
        ),
        "{error:?}"
    );
}

#[test]
fn reply_with_unknown_subject_field_is_rejected() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        reply_raw: Some(json!({
            "type": "ok",
            "request_id": 1,
            "subject": "host"
        })),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("unknown field");
    assert!(matches!(error, HostError::ProtocolIo(_)), "{error:?}");
}

#[test]
fn artifact_hash_mismatch_blocks_load() {
    if in_child_mode() {
        return;
    }
    let fixture = fixture(FixtureOptions {
        artifact_hash: Some("0".repeat(64)),
        expect_load_error: true,
        ..FixtureOptions::default()
    });
    assert!(matches!(
        fixture.load_error,
        Some(HostError::Load(
            PluginLoadError::ArtifactHashMismatch { .. }
        ))
    ));
}

#[test]
fn predictable_name_and_occupied_channel_are_typed_errors() {
    if in_child_mode() {
        return;
    }
    let temp = tempdir_private();
    let dir = temp.path().join("private");
    let predictable = PluginChannel::new(&dir, ID, "short");
    assert!(matches!(predictable, Err(HostError::PredictableName)));

    let secret = auro_plugins::channel::generate_secret();
    let first = PluginChannel::new(&dir, ID, &secret).expect("first channel");
    // The endpoint carries a short id tag and half the secret; the full secret stays in the token
    // file, so a long id cannot overflow sun_path on macOS.
    assert!(!first.endpoint_name().contains(ID));
    assert!(first.endpoint_name().contains(&secret[..16]));
    let conflict = PluginChannel::new(&dir, ID, &secret);
    assert!(matches!(conflict, Err(HostError::ChannelConflict(_))));
}

#[test]
fn outside_grant_and_unresolved_paths_are_typed_errors() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        grant_fs: true,
        commands: CommandPolicy::deny_all().allow_read_file(),
        ..FixtureOptions::default()
    });
    fixture.host.handshake(ID).expect("valid handshake");

    let outside = fixture._temp.path().join("outside.txt");
    fs::write(&outside, b"secret").expect("outside file");
    let error = fixture
        .host
        .call(
            ID,
            1,
            PluginCommand::ReadFile {
                path: canonical_string(&outside),
            },
        )
        .expect_err("outside grant");
    assert!(
        matches!(error, HostError::Effect(EffectError::OutsideGrant(_))),
        "{error:?}"
    );

    let missing = fixture.allowed.join("missing.txt");
    let error = fixture
        .host
        .call(
            ID,
            2,
            PluginCommand::ReadFile {
                path: std::path::Path::new(&canonical_string(&fixture.allowed))
                    .join("missing.txt")
                    .to_string_lossy()
                    .into_owned(),
            },
        )
        .expect_err("missing file");
    assert!(
        matches!(error, HostError::Effect(EffectError::UnresolvedPath(_))),
        "{error:?}"
    );
    assert!(!missing.exists());
}

#[test]
fn call_before_handshake_is_typed_error() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        ..FixtureOptions::default()
    });
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("handshake is required");
    assert!(matches!(error, HostError::HandshakeRequired), "{error:?}");
}

#[cfg(unix)]
#[test]
fn symlinked_private_dir_is_rejected_without_chmodding_the_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempdir_private();
    let victim = temp.path().join("victim");
    fs::create_dir(&victim).expect("victim dir");
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o755)).expect("victim mode");
    let link = temp.path().join("link");
    symlink(&victim, &link).expect("symlink");

    let error = PluginChannel::new(&link, ID, &auro_plugins::channel::generate_secret())
        .expect_err("a symlinked private dir is not trusted");
    assert!(
        matches!(error, HostError::InsecureDirectory(_)),
        "{error:?}"
    );
    let mode = fs::metadata(&victim)
        .expect("victim metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "rejection must not chmod the symlink target");
}

#[cfg(unix)]
#[test]
fn shared_temp_ancestor_can_host_a_private_channel_dir() {
    // `/tmp` is mode 1777 (sticky). It is only a platform ancestor; the managed `auro-*` directory
    // below it is created with 0700 and checked strictly.
    let dir = std::env::temp_dir().join(format!("auro-probe-{}", std::process::id()));
    let channel = PluginChannel::new(&dir, ID, &"a".repeat(32))
        .expect("a sticky /tmp ancestor must not block the per-user fallback");
    assert!(channel.endpoint_name().contains(&"a".repeat(16)[..]));
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn unsafe_xdg_runtime_dir_is_ignored_by_the_fallback() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir_private();
    let xdg = temp.path().join("runtime");
    fs::create_dir(&xdg).expect("xdg dir");
    fs::set_permissions(&xdg, fs::Permissions::from_mode(0o755)).expect("xdg mode");

    // Serialize the environment mutation: no other test reads XDG_RUNTIME_DIR.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let previous = std::env::var_os("XDG_RUNTIME_DIR");
    // SAFETY: the mutex excludes other readers of this variable in this test binary.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", &xdg) };
    let chosen = auro_plugins::channel::default_private_dir();
    match previous {
        Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
        None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
    }

    // A group/other-readable XDG candidate is not trusted; the per-user temp fallback is used.
    assert!(
        !chosen.starts_with(&xdg),
        "unsafe XDG_RUNTIME_DIR must be ignored: {chosen:?}"
    );
}

#[test]
fn flood_is_bounded_and_main_thread_stays_responsive() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        reply_delay_ms: 5,
        ..FixtureOptions::default()
    });
    assert_eq!(fixture.host.call_timeout(), DEFAULT_CALL_TIMEOUT);
    fixture.host.set_call_timeout(Duration::from_secs(2));
    fixture.host.handshake(ID).expect("valid handshake");

    // Submitting never waits for a reply. The bounded queue must push back long before 10k calls
    // are accepted, and the submitting thread must remain responsive.
    let started = Instant::now();
    let mut pending = Vec::new();
    let mut rejected = 0;
    for request_id in 0..10_000 {
        match fixture.host.submit(ID, request_id, PluginCommand::Ping) {
            Ok(call) => pending.push(call),
            Err(HostError::QueueFull { .. }) => {
                rejected += 1;
                break;
            }
            Err(error) => panic!("unexpected submit error: {error:?}"),
        }
    }
    assert!(rejected > 0, "10k submissions must hit backpressure");
    assert!(
        pending.len() <= REQUEST_QUEUE_CAPACITY + 16,
        "queue is bounded"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "submitting must not block on replies"
    );

    for call in pending {
        let request_id = call.request_id();
        let reply = fixture.host.wait(call).expect("queued reply");
        assert!(
            matches!(reply, PluginReply::Ok { request_id: found } if found == request_id),
            "{reply:?}"
        );
    }
    let reply = fixture
        .host
        .call(ID, 10_001, PluginCommand::Ping)
        .expect("host still responsive after the flood");
    assert!(matches!(reply, PluginReply::Ok { request_id: 10_001 }));
}

#[test]
fn silent_plugin_times_out_and_releases_grants() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        grant_fs: true,
        commands: CommandPolicy::deny_all().allow_ping(),
        silent: true,
        ..FixtureOptions::default()
    });
    fixture.host.set_call_timeout(Duration::from_secs(2));
    fixture
        .host
        .handshake(ID)
        .expect("silent child still says hello");
    fixture.host.set_call_timeout(Duration::from_millis(250));
    let subject = fixture.host.loaded(ID).expect("loaded").subject().clone();
    let path = canonical_string(&fixture.allowed.join("ok.txt"));

    let started = Instant::now();
    let error = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("a silent plugin must time out");
    assert!(
        matches!(error, HostError::Timeout { ref id, duration } if id.as_str() == ID && duration == Duration::from_millis(250)),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout must bound the wait"
    );
    assert_eq!(
        fixture.host.loaded(ID).expect("loaded").state(),
        PluginState::Failed
    );
    assert!(
        fixture
            .host
            .child_status(ID)
            .expect("child status")
            .is_some(),
        "timed-out child must be reaped"
    );
    assert!(
        fixture
            .host
            .effects()
            .policy()
            .check_plugin(&subject, Capability::Fs, Some(path.as_str()))
            .is_err(),
        "the failed subject's grants must be released"
    );
    let events = fixture.host.take_events();
    assert!(
        events.iter().any(|event: &PluginAvailability| {
            event.id == ID
                && event.state == PluginState::Failed
                && event.cause == Some(PluginFailure::Timeout)
        }),
        "{events:?}"
    );
    let retry = fixture.host.call(ID, 2, PluginCommand::Ping);
    assert!(
        matches!(retry, Err(HostError::HandshakeRequired)),
        "{retry:?}"
    );
}

#[cfg(target_os = "linux")]
fn process_counts() -> (usize, usize) {
    let threads = fs::read_dir("/proc/self/task").expect("task dir").count();
    let fds = fs::read_dir("/proc/self/fd").expect("fd dir").count();
    (threads, fds)
}

#[test]
fn abort_exit_and_panic_children_are_typed_and_reaped() {
    if in_child_mode() {
        return;
    }
    #[cfg(target_os = "linux")]
    let before = process_counts();

    for action in ["exit", "abort", "panic"] {
        let mut fixture = fixture(FixtureOptions {
            commands: CommandPolicy::deny_all().allow_ping(),
            child_action: Some(action),
            ..FixtureOptions::default()
        });
        fixture.host.set_call_timeout(Duration::from_secs(2));
        fixture.host.handshake(ID).expect("valid handshake");
        let error = fixture
            .host
            .call(ID, 1, PluginCommand::Ping)
            .expect_err("child death must be typed");
        assert!(
            matches!(
                error,
                HostError::PluginExited { ref id, .. } | HostError::ChildDied { ref id }
                    if id.as_str() == ID
            ),
            "{action}: {error:?}"
        );
        assert!(
            fixture
                .host
                .child_status(ID)
                .expect("child status")
                .is_some(),
            "{action}: child must be reaped"
        );
        let events = fixture.host.take_events();
        assert!(
            events.iter().any(|event| {
                event.id == ID
                    && event.state == PluginState::Failed
                    && matches!(
                        event.cause,
                        Some(PluginFailure::Exited { .. } | PluginFailure::ChildDied)
                    )
            }),
            "{action}: {events:?}"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let after = process_counts();
        assert!(
            after.0 <= before.0 + 8,
            "thread count grew from {before:?} to {after:?}"
        );
        assert!(
            after.1 <= before.1 + 16,
            "fd count grew from {before:?} to {after:?}"
        );
    }
}

#[test]
fn availability_events_track_ready_and_failed_transitions() {
    if in_child_mode() {
        return;
    }
    let mut fixture = fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        ..FixtureOptions::default()
    });
    assert!(fixture.host.take_events().iter().any(|event| {
        event.id == ID && event.state == PluginState::Starting && event.cause.is_none()
    }));
    fixture.host.handshake(ID).expect("valid handshake");
    assert!(fixture.host.take_events().iter().any(|event| {
        event.id == ID && event.state == PluginState::Ready && event.cause.is_none()
    }));
    assert!(fixture.host.take_events().is_empty());

    let reply = fixture
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect("responsive child");
    assert!(matches!(reply, PluginReply::Ok { request_id: 1 }));

    let mut mismatch = crate::fixture(FixtureOptions {
        commands: CommandPolicy::deny_all().allow_ping(),
        reply_id: 2,
        ..FixtureOptions::default()
    });
    mismatch.host.handshake(ID).expect("valid handshake");
    let error = mismatch
        .host
        .call(ID, 1, PluginCommand::Ping)
        .expect_err("mismatched reply");
    assert!(
        matches!(error, HostError::ReplyMismatch { .. }),
        "{error:?}"
    );
    let events = mismatch.host.take_events();
    assert!(
        events.iter().any(|event| {
            event.id == ID
                && event.state == PluginState::Failed
                && event.cause == Some(PluginFailure::Protocol)
        }),
        "{events:?}"
    );
}
