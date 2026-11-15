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

#[auro::command(rename_all = "camelCase")]
async fn greet(first_name: String) -> Result<String, AuroError> {
    Ok(format!("hello {first_name}"))
}

#[auro::command(name = "renamed")]
async fn original(value: u64) -> Result<u64, AuroError> {
    Ok(value)
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
