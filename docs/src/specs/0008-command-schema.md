# 0008 - Command schema registry

**Status:** Implemented (2026-11-17)

Phase 0 made commands typed Rust functions and built a `CommandHandler` that dispatches by name.
Phase 1 adds the metadata that makes the handler useful outside first-party Rust: a JSON Schema for
each command's input and output. The handler is not wired into `App` yet; plugins are the consumer.

## Scope

- `schemars` 1.x as the schema source.
- `#[auro::command]` derives the input/output schema from the signature.
- `CommandHandler` stores and exposes per-command schemas.
- Input schema composition for the command's non-state arguments.

## Out of scope

- Wiring the registry into `App` or auto-generating docs from it (the handler remains a boundary
  for plugins).
- Plugin transport and schema negotiation (Phase 3).
- Schema evolution/versioning policy.
- JSON Schema `$id` and remote references.
- Serializing `State<T>` values; state is resolved, not sent.

## Decisions

### D1 - The macro owns schema derivation

`schemars` is a dependency of `auro-ipc` and is re-exported as `auro::ipc::schemars`, so a consumer
does not need its own dependency for primitive arguments. A user argument type implements
`schemars::JsonSchema`; the generated code is a compile error otherwise. The output type comes from
`Result<T, AuroError>`; `T` must implement `JsonSchema`.

`State<T>` arguments are excluded from the input schema: they are resolved from the app, never sent
in the payload.

### D2 - Input is one object schema

Input is a JSON Schema object:

```json
{
  "type": "object",
  "properties": { "current": { "type": "integer", ... } },
  "required": ["current"],
  "$defs": { ... }
}
```

Property names use the command's `rename_all` setting. An argument whose schema accepts null is
optional and omitted from `required`; this covers `Option<T>` and unconstrained schemas such as
`serde_json::Value`. A `$ref` or other constrained schema stays required. Every other non-state
argument is required. `State<T>` is treated as managed state only when unqualified or written as
`auro::State`/`auro::ipc::State`; a payload type such as `payload::State<T>` is deserialized. A
`rename_all` that maps two arguments to one key is a compile error. Registering two commands with
the same name is rejected (`try_command` returns a typed error; the builder and
`generate_handler!` panic with the name). Per-argument
`$defs`/`definitions` are hoisted to the root so `#/$defs/...` references resolve;
duplicate definition names are last-wins in this slice. Schemas are computed once when the handler
is built, not reflected at dispatch time.

### D3 - Output is the `Ok` type

For `Result<T, AuroError>`, the stored output schema is `schema_for!(T)`. `()` is a valid output and
serializes as `{"type":"null"}`. The error shape is not part of the command output schema;
`Response` already carries the error kind/message.

### D4 - Metadata is additive on the handler

`CommandHandler::command(name, dispatch)` remains for manual dispatch. The macro uses
`command_with_schema(name, dispatch, input, output)`. Accessors:

- `schemas() -> impl Iterator<Item = &CommandSchema>`
- `schema(name) -> Option<&CommandSchema>`

`CommandSchema { name, input, output }` is public so a future plugin/docs consumer can serialize it.
A command registered without metadata has no schema entry; it still dispatches and appears in
`names()`.

### D5 - The schema is a JSON value, not a live generator

The handler stores `serde_json::Value` snapshots. Dispatch never re-runs `schemars`, so a command
call cannot fail because schema generation failed at runtime: derivation problems are compile-time.

## Interfaces

```rust,ignore
#[auro::command(rename_all = "camelCase")]
async fn greet(first_name: String, count: u32) -> Result<Greeting, AuroError> { ... }

let handler = auro::generate_handler![greet];
let schema = handler.schema("greet").expect("registered with schema");
// schema.input["properties"]["firstName"]["type"] == "string"
// schema.output["$ref"] or the Greeting shape
```

```rust
pub struct CommandSchema {
    pub name: &'static str,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
}

impl CommandHandler {
    pub fn command_with_schema(
        self,
        name: &'static str,
        dispatch: impl Fn(Invoke) -> ResponseFuture + Send + Sync + 'static,
        input: serde_json::Value,
        output: serde_json::Value,
    ) -> Self;

    pub fn schemas(&self) -> impl Iterator<Item = &CommandSchema> + '_;
    pub fn schema(&self, name: &str) -> Option<&CommandSchema>;
}
```

## Acceptance

1. A command with a primitive argument stores an input schema with that property under
   `properties`, listed in `required`; an `Option<T>` argument is not listed in `required`.
2. `State<T>` arguments do not appear in the input schema.
3. `rename_all = "camelCase"` changes the property key.
4. The output schema is derived from the `Ok` type; `()` becomes a null schema.
5. A manually registered command remains dispatchable and has no schema entry.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Wiring the registry into `App` and generated docs | Plugin/docs slice |
| Schema negotiation, versioning and compatibility | Phase 3 plugin spec |
| Definition-name collisions when two types share a name | Revisit when struct args are used in the wild |
| Optional command arguments | Later command-contract change |
| `$id` and cross-file references | Not planned for 1.0 |
