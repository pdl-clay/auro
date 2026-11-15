# 0002 - Rust-only frontend and typed IPC

**Status:** Accepted

## Context

Tauri's default IPC is `invoke("save_document", { path, body })`: the command name is a string,
the payload is JSON, and a typo surfaces as a runtime error in a browser console. The type
information exists at compile time in the Rust command, but it is discarded at the boundary.

Auro has no JavaScript. If the frontend is Rust and the backend is Rust, the boundary can be a
function call. Serialisation is still needed for plugins and for persisted state, but not for
first-party UI-to-command traffic.

## Decision

Schemas are derived from Rust signatures (`schemars`), the call site is a plain `async fn`, and
there is no stringly-typed command dispatch in first-party code.

```rust,ignore
#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> { /* ... */ }

let saved = api::save_document(doc.path, doc.body).await?;
```

## Consequences

**Easier**

- A renamed command, a wrong argument type or a wrong arity is a compile error.
- No `invoke` string table, no payload casing mismatches, no "command not found" at runtime.
- The schema is generated from the same source of truth, so it cannot drift.

**Harder**

- Deriving schemas requires `Serialize`/`Deserialize`/`JsonSchema` bounds on command arguments,
  which some types do not satisfy cheaply (closures, borrowed data, platform handles).
- Commands must be registered before the app runs; there is no dynamic discovery.
- Editors and tooling lose the "just call an endpoint" affordance a web frontend has.

**Forbidden**

- A string-keyed command bus for first-party calls.
- A second schema definition maintained by hand next to the Rust signature.
- Hand-written `serde` implementations whose field names diverge from the schema.
