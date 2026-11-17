# Architecture overview

Auro is an application shell around a renderer it does not own. Every architectural rule below
follows from that split.

```
+-----------------------------------------------------------+
| auro-cli    new . dev . build . bundle . add . doctor     |
+-----------------------------------------------------------+
| auro-core                                                 |
|   App . WindowManager . Config . Lifecycle .              |
|   Command registry . State store . Event bus . Instance    |
+--------------+-------------------+------------------------+
| auro-ui      | auro-ipc          | auro-perms             |
| GPUI + kit   | typed bus         | capabilities           |
+--------------+-------------------+------------------------+
| auro-plugins   in-process (cdylib) | out-of-process RPC   |
+-----------------------------------------------------------+
| auro-bundle . auro-update . auro-sign                     |
+-----------------------------------------------------------+
```

The diagram is a **layering**, not a dependency graph: a layer may use the layer below it, and
`auro-core` is the only crate that composes the others. The Phase 0 workspace fixes the current
`Cargo.toml` edges for the materialized crates; the full crate graph remains the intended
direction as later phases land.

## Responsibilities

| Layer | Owns | Must not |
|---|---|---|
| `auro-cli` | project scaffolding, dev loop, bundling entry points, diagnostics | contain runtime logic the app needs |
| `auro-core` | `App`, window manager, config, lifecycle, command registry, state store, event bus, single-instance guard | render anything |
| `auro-ui` | GPUI re-export, toolkit seam, tokens, minimal primitives | ship a component toolkit; check permissions |
| `auro-ipc` | typed command transport, schema derivation, event delivery | decide policy |
| `auro-perms` | capability manifests, runtime enforcement for plugin and content boundaries | guard code that the compiler already guards |
| `auro-plugins` | plugin manifests, in-process `cdylib` hosts, out-of-process RPC | assume plugin code is trusted |
| `auro-bundle`, `auro-update`, `auro-sign` | packaging per platform, signed manifests, staged downloads and Linux AppImage apply, key handling | run inside the app's main loop |

## Data flow of one interaction

1. **Main thread.** GPUI owns it. A user event arrives and a view handler runs.
2. **Dispatch.** The handler either mutates state directly (cheap, synchronous) or asks the
   command registry for an async command.
3. **Off-thread work.** The command is a plain `async fn` on the `tokio` runtime. File I/O,
   network, parsing - anything that can block - happens here.
4. **Return.** The command returns a typed `Result<T, AuroError>`. There is no serialization
   boundary unless the command crosses the plugin boundary.
5. **Back to the main thread.** The result is applied by updating an `Entity` and calling
   `cx.notify()`, which schedules a render.

```rust,ignore
// Current workspace API (unpublished).
#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> {
    tokio::fs::write(&path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}
```

The call site is a Rust call, so a wrong argument type is a compile error rather than an
`invoke` failure at runtime.

## Where state lives

- **UI state** lives in GPUI entities on the main thread.
- **Application state** lives in the core state store, reachable from commands.
- **Reloadable state** (planned) is captured as a host-owned `serde` snapshot so a future hot reload
  or a state-preserving restart can restore opted-in state. The hot-reload mechanism is under
  revision ([spec 0004](../specs/0004-hot-reload.md), [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md)).

The boundary rule that keeps this honest: nothing outside `auro-core` holds a global mutable
reference to app state, and nothing on the main thread blocks on I/O.

## What is not decided yet

Tracked in [ROADMAP §8](https://github.com/pdl-clay/auro/blob/main/ROADMAP.md#8-open-questions):

- a scripting layer for plugin UIs, or Rust only;
- out-of-process plugins first, or in-process first.

Resolved: **component kit** — Auro ships no toolkit; the application chooses one
([ADR 0009](../decisions/0009-toolkits-are-app-level.md),
[choosing a toolkit](toolkits.md)).

Anything in this book that depends on those answers is marked as open where it appears.
