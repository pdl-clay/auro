// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use auro::ipc::{CommandHandler, Invoke, InvokeMessage, Response, StateStore};
use auro::prelude::*;

#[derive(Debug, PartialEq)]
struct Counter(u64);

#[auro::command]
async fn increment(current: u64) -> Result<u64, AuroError> {
    Ok(current + 1)
}

#[auro::command]
fn double(value: u64) -> Result<u64, AuroError> {
    Ok(value * 2)
}

#[auro::command]
async fn read_counter(state: State<Counter>) -> Result<u64, AuroError> {
    Ok(state.0)
}

#[auro::command]
async fn read_counter_via_prelude(state: auro::prelude::State<Counter>) -> Result<u64, AuroError> {
    Ok(state.0)
}

#[auro::command(rename_all = "camelCase")]
async fn greet(first_name: String) -> Result<String, AuroError> {
    Ok(format!("hello {first_name}"))
}

#[auro::command(name = "renamed")]
async fn original(value: u64) -> Result<u64, AuroError> {
    Ok(value)
}

#[auro::command]
async fn noop() -> Result<(), AuroError> {
    Ok(())
}

#[auro::command]
async fn optional(value: Option<u64>) -> Result<Option<u64>, AuroError> {
    Ok(value)
}

#[auro::command]
async fn raw(value: serde_json::Value) -> Result<serde_json::Value, AuroError> {
    Ok(value)
}

#[auro::command]
async fn async_boom() -> Result<(), AuroError> {
    panic!("async command panicked")
}

#[auro::command]
fn sync_boom() -> Result<(), AuroError> {
    panic!("sync command panicked")
}

fn handler() -> CommandHandler {
    auro::generate_handler![increment, double, read_counter, greet, original]
}

fn setup() -> (Arc<StateStore>, CommandHandler) {
    let states = Arc::new(StateStore::new());
    states.insert(Counter(41));
    (states, handler())
}

fn invoke(states: &Arc<StateStore>, command: &str, payload: serde_json::Value) -> Invoke {
    Invoke::new(InvokeMessage::new(command, payload), Arc::clone(states))
}

fn run(states: &Arc<StateStore>, command: &str, payload: serde_json::Value) -> Response {
    let invoke = invoke(states, command, payload);
    auro::ipc::runtime::runtime().block_on(handler().dispatch(invoke))
}

#[test]
fn dispatches_an_async_command() {
    let (states, _) = setup();
    match run(&states, "increment", serde_json::json!({ "current": 1 })) {
        Response::Ok { value } => assert_eq!(value, 2),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn dispatches_a_sync_command_through_the_blocking_pool() {
    let (states, _) = setup();
    match run(&states, "double", serde_json::json!({ "value": 21 })) {
        Response::Ok { value } => assert_eq!(value, 42),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn resolves_managed_state() {
    let (states, _) = setup();
    match run(&states, "read_counter", serde_json::json!({})) {
        Response::Ok { value } => assert_eq!(value, 41),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn resolves_state_through_the_prelude_path() {
    let states = Arc::new(StateStore::new());
    states.insert(Counter(41));
    let handler = auro::generate_handler![read_counter_via_prelude];
    let invoke = invoke(&states, "read_counter_via_prelude", serde_json::json!({}));
    match auro::ipc::runtime::runtime().block_on(handler.dispatch(invoke)) {
        Response::Ok { value } => assert_eq!(value, 41),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn applies_rename_all_to_argument_keys() {
    let (states, _) = setup();
    match run(&states, "greet", serde_json::json!({ "firstName": "ana" })) {
        Response::Ok { value } => assert_eq!(value, "hello ana"),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn honours_an_explicit_command_name() {
    let (states, _) = setup();
    match run(&states, "renamed", serde_json::json!({ "value": 7 })) {
        Response::Ok { value } => assert_eq!(value, 7),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn reports_a_missing_argument_as_a_command_error() {
    let (states, _) = setup();
    match run(&states, "increment", serde_json::json!({})) {
        Response::Err { kind, .. } => assert_eq!(kind, "command"),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn reports_an_unknown_command() {
    let (states, handler) = setup();
    let invoke = invoke(&states, "nope", serde_json::json!({}));
    let response = auro::ipc::runtime::runtime().block_on(handler.dispatch(invoke));
    match response {
        Response::Err { kind, .. } => assert_eq!(kind, "unknown-command"),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn lists_registered_commands() {
    let (_, handler) = setup();
    let names: Vec<_> = handler.names().collect();
    assert_eq!(
        names,
        ["increment", "double", "read_counter", "greet", "renamed"]
    );
}

#[test]
fn describes_inputs_and_outputs() {
    let (_, handler) = setup();
    let schema = handler.schema("increment").expect("increment schema");

    assert_eq!(schema.input["type"], "object");
    assert_eq!(schema.input["properties"]["current"]["type"], "integer");
    assert_eq!(schema.input["required"], serde_json::json!(["current"]));
    assert_eq!(schema.output["type"], "integer");
    assert!(schema.output.get("$ref").is_none());
}

#[test]
fn state_arguments_are_not_in_the_schema() {
    let (_, handler) = setup();
    let schema = handler.schema("read_counter").expect("schema");

    assert_eq!(schema.input["properties"], serde_json::json!({}));
    assert!(schema.input.get("required").is_none());
}

#[test]
fn schema_property_names_follow_rename_all() {
    let (_, handler) = setup();
    let schema = handler.schema("greet").expect("schema");

    assert!(
        schema.input["properties"].get("firstName").is_some(),
        "{}",
        schema.input
    );
    assert!(schema.input["properties"].get("first_name").is_none());
}

#[test]
fn describes_a_unit_output_as_null() {
    let handler = auro::generate_handler![noop];
    let schema = handler.schema("noop").expect("schema");
    assert_eq!(schema.output["type"], "null");
}

#[test]
fn an_optional_argument_is_not_required_in_the_schema() {
    let handler = auro::generate_handler![optional];
    let schema = handler.schema("optional").expect("schema");
    assert!(schema.input.get("required").is_none(), "{}", schema.input);
    assert_eq!(
        schema.input["properties"]["value"]["type"],
        serde_json::json!(["integer", "null"])
    );
}

#[test]
fn a_json_value_argument_is_optional_in_the_schema_and_at_runtime() {
    let states = Arc::new(StateStore::new());
    let handler = auro::generate_handler![raw];
    let schema = handler.schema("raw").expect("schema");
    assert!(schema.input.get("required").is_none(), "{}", schema.input);

    let invoke = invoke(&states, "raw", serde_json::json!({}));
    match auro::ipc::runtime::runtime().block_on(handler.dispatch(invoke)) {
        Response::Ok { value } => assert!(value.is_null(), "{value}"),
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn a_panicking_async_command_becomes_an_error_response() {
    let states = Arc::new(StateStore::new());
    let handler = auro::generate_handler![async_boom, noop];
    let invoke = invoke(&states, "async_boom", serde_json::json!({}));
    let response = auro::ipc::runtime::runtime().block_on(handler.dispatch(invoke));
    match response {
        Response::Err { kind, message } => {
            assert_eq!(kind, "command");
            assert!(message.contains("panicked"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn a_panicking_sync_command_becomes_an_error_response() {
    let states = Arc::new(StateStore::new());
    let handler = auro::generate_handler![sync_boom, noop];
    let invoke = invoke(&states, "sync_boom", serde_json::json!({}));
    let response = auro::ipc::runtime::runtime().block_on(handler.dispatch(invoke));
    match response {
        Response::Err { kind, message } => {
            assert_eq!(kind, "command");
            assert!(message.contains("panicked"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
#[should_panic(expected = "duplicate command registration")]
fn duplicate_commands_are_rejected_when_the_handler_is_built() {
    let _ = auro::generate_handler![noop, noop];
}

#[test]
fn manual_commands_have_no_schema_entry() {
    let handler = auro::ipc::CommandHandler::new().command("manual", |_| {
        Box::pin(async { auro::ipc::Response::ok(()) })
    });

    assert!(handler.contains("manual"));
    assert!(handler.schema("manual").is_none());
    assert_eq!(handler.schemas().count(), 0);
}
