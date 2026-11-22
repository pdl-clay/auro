// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the typed plugin handshake and frame codec.

use auro_plugins::protocol::{
    Hello, HelloAck, MAX_FRAME_BYTES, NegotiationError, PROTOCOL_VERSION, PluginCommand,
    PluginEvent, PluginReply, capabilities_hash, read_frame, schema_digests, validate_hello,
    write_frame,
};
use auro_plugins::sdk_version;

fn canonical_hello() -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        sdk_version: sdk_version().to_owned(),
        schema_digests: schema_digests(),
        capabilities_hash: capabilities_hash(&[]),
    }
}

#[test]
fn canonical_hello_is_accepted() {
    let hello = canonical_hello();
    assert!(validate_hello(&hello, &[]).is_ok());
}

#[test]
fn old_protocol_version_is_rejected() {
    let mut hello = canonical_hello();
    hello.protocol_version = 0;
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::UnsupportedProtocol {
            expected: 1,
            found: 0
        })
    ));
}

#[test]
fn future_protocol_version_is_rejected() {
    let mut hello = canonical_hello();
    hello.protocol_version = PROTOCOL_VERSION + 1;
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::UnsupportedProtocol { .. })
    ));
}

#[test]
fn old_sdk_version_is_rejected() {
    let mut hello = canonical_hello();
    hello.sdk_version = "0.0.0".to_owned();
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::SdkMismatch { found, .. }) if found == "0.0.0"
    ));
}

#[test]
fn future_sdk_version_is_rejected() {
    let mut hello = canonical_hello();
    hello.sdk_version = "9999.0.0".to_owned();
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::SdkMismatch { found, .. }) if found == "9999.0.0"
    ));
}

#[test]
fn changed_digest_is_rejected() {
    let mut hello = canonical_hello();
    hello
        .schema_digests
        .insert("command.read_file".to_owned(), "0".repeat(64));
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::SchemaMismatch { key, .. }) if key == "command.read_file"
    ));
}

#[test]
fn missing_digest_is_rejected() {
    let mut hello = canonical_hello();
    hello.schema_digests.remove("event.file_changed");
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::MissingSchema { key }) if key == "event.file_changed"
    ));
}

#[test]
fn extra_digest_is_rejected() {
    let mut hello = canonical_hello();
    hello
        .schema_digests
        .insert("command.unknown".to_owned(), "0".repeat(64));
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::UnexpectedSchema { key }) if key == "command.unknown"
    ));
}

#[test]
fn capabilities_mismatch_is_rejected() {
    let mut hello = canonical_hello();
    hello.capabilities_hash = "0".repeat(64);
    assert!(matches!(
        validate_hello(&hello, &[]),
        Err(NegotiationError::CapabilityMismatch { .. })
    ));
}

#[test]
fn hello_rejects_unknown_field() {
    let mut value = serde_json::to_value(canonical_hello()).expect("hello is serializable");
    value
        .as_object_mut()
        .expect("hello serializes to an object")
        .insert("subject".to_owned(), serde_json::json!("plugin:host"));
    assert!(serde_json::from_value::<Hello>(value).is_err());
}

#[test]
fn command_rejects_subject_field() {
    let parsed = serde_json::from_str::<PluginCommand>(r#"{"type":"read_file","path":"data/x"}"#)
        .expect("valid command JSON");
    assert_eq!(
        parsed,
        PluginCommand::ReadFile {
            path: "data/x".to_owned()
        }
    );
    assert!(
        serde_json::from_str::<PluginCommand>(
            r#"{"type":"read_file","path":"data/x","subject":"plugin:example"}"#
        )
        .is_err()
    );
}

#[test]
fn frame_roundtrip_preserves_every_message_type() {
    let mut buffer = Vec::new();
    let hello = canonical_hello();
    write_frame(&mut buffer, &hello).expect("hello frame fits");
    assert_eq!(
        read_frame::<_, Hello>(&mut buffer.as_slice()).expect("hello decodes"),
        hello
    );

    buffer.clear();
    let ack = HelloAck {
        protocol_version: PROTOCOL_VERSION,
        capabilities_hash: capabilities_hash(&[]),
    };
    write_frame(&mut buffer, &ack).expect("ack frame fits");
    assert_eq!(
        read_frame::<_, HelloAck>(&mut buffer.as_slice()).unwrap(),
        ack
    );

    buffer.clear();
    let commands = [
        PluginCommand::ReadFile {
            path: "data/x".to_owned(),
        },
        PluginCommand::WriteFile {
            path: "data/y".to_owned(),
            bytes: vec![0, 1, 2, 255],
        },
        PluginCommand::Ping,
    ];
    for command in commands {
        write_frame(&mut buffer, &command).expect("command frame fits");
        assert_eq!(
            read_frame::<_, PluginCommand>(&mut buffer.as_slice()).unwrap(),
            command
        );
        buffer.clear();
    }

    let replies = [
        PluginReply::Ok { request_id: 7 },
        PluginReply::FileContent {
            request_id: 8,
            bytes: vec![42],
        },
        PluginReply::Error {
            request_id: 9,
            message: "denied".to_owned(),
        },
    ];
    for reply in replies {
        write_frame(&mut buffer, &reply).expect("reply frame fits");
        assert_eq!(
            read_frame::<_, PluginReply>(&mut buffer.as_slice()).unwrap(),
            reply
        );
        buffer.clear();
    }

    let event = PluginEvent::FileChanged {
        path: "data/z".to_owned(),
    };
    write_frame(&mut buffer, &event).expect("event frame fits");
    assert_eq!(
        read_frame::<_, PluginEvent>(&mut buffer.as_slice()).unwrap(),
        event
    );
}

#[test]
fn oversized_frame_is_rejected() {
    let length = (MAX_FRAME_BYTES as u32) + 1;
    let framed = length.to_le_bytes().to_vec();
    assert!(read_frame::<_, Hello>(&mut framed.as_slice()).is_err());

    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        sdk_version: sdk_version().to_owned(),
        schema_digests: schema_digests(),
        capabilities_hash: "a".repeat(MAX_FRAME_BYTES),
    };
    let mut buffer = Vec::new();
    assert!(write_frame(&mut buffer, &hello).is_err());
    assert!(buffer.is_empty());
}
