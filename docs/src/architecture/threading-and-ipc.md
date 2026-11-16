# Threading and IPC

Two rules generate every other rule in this chapter:

1. GPUI owns the main thread, and all UI mutation happens on it.
2. Commands run off it and return typed results.

## Threading model

| Work | Thread | Mechanism |
|---|---|---|
| Rendering, layout, input, focus | main | `Application::run` |
| Entity mutation | main | `cx.update`, `cx.listener`, `AsyncApp::update` |
| Spawning async work | main -> runtime | `cx.spawn` |
| File, network, process, parsing | `tokio` worker threads | `async fn` in a command |
| Plugin RPC | worker threads | out-of-process transport |

Rules:

- **Never block the main thread.** No `std::fs`, no `reqwest::blocking`, no `Command::output`,
  no synchronous `sleep` in a render or event handler.
- **Never touch an entity from a worker thread.** Move the result to the main thread first, then
  mutate and `cx.notify()`.
- **Never `await` while holding a UI borrow.** Clone what you need, drop the borrow, then spawn.

## What a command is

A command is a Rust function with a typed signature. The macro derives the argument schema, the
result schema, and the registry entry; it does not introduce a wire format the compiler cannot
see.

```rust,ignore
// Current workspace API (unpublished).
#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> {
    tokio::fs::write(&path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}
```

Constraints the macro is expected to enforce:

- arguments and results implement `serde::Serialize` / `Deserialize` plus `schemars::JsonSchema`,
  because the plugin boundary still needs a schema;
- errors are a single `AuroError` type, with `From` conversions for `std::io::Error` and
  `serde_json::Error` so `?` works in command bodies;
- commands are registered at app build time, not discovered at runtime by string name.

## Why "typed" is the whole point

Tauri's default call is `invoke("save_document", { path, body })`: the name is a string, the
payload is JSON, and a typo is a runtime error in the browser console. Auro has no browser and no
JS, so the call site is:

```rust,ignore
let saved = api::save_document(doc.path, doc.body).await?;
```

A wrong argument count, a wrong type, or a renamed command is a compile error. The schema still
exists, but for classification and plugin transport, not for runtime argument checking.

## Events

Events flow **main-ward**: a background task finishes and updates the entity that cares, rather
than broadcasting an untyped payload that every view filters. This avoids the usual event-bus
failure mode where a renamed event silently stops working.

Where a broadcast is genuinely needed (app-wide notifications, menu commands), the event type is
an enum in `auro-core` and subscribers get a typed receiver.

## Error handling

- Library code returns `Result<T, AuroError>`; no `unwrap` outside tests, and no `unwrap` in a
  command.
- `AuroError` is `#[non_exhaustive]` from the start: adding a variant must not be a breaking
  change for downstream matches.
- A command error surfaces to the caller as a value. It does not panic, and it does not get
  stringified into a dialog by the framework - the app decides how to present it.
- Panics in plugin code must not take down the host; the plugin boundary catches them and
  reports a typed failure.
