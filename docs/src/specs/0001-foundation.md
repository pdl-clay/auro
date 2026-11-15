# 0001 - Foundation

**Status:** Draft - awaiting approval

Phase 0 exists to answer one question: **does an Auro app feel like a native app, and is the loop
fast enough to live in?** This spec defines the smallest set of contracts that makes the vertical
slice real, and nothing beyond it.

## Scope

- A Cargo workspace with the crates needed for the slice, and only those.
- `Auro.toml`: the configuration surface an app author edits (fields in [0002](0002-config.md)).
- `auro new`, `auro dev`, `auro build`, `auro doctor`.
- A typed command contract: `#[auro::command]`, `generate_handler!`, `AuroError`.
- Application state and app-wide events, reachable from commands and views.
- The GPUI seam: how Auro owns the main thread and hands the renderer a view.
- The generated project skeleton.

## Out of scope

- Plugins, capabilities enforcement, bundling output, updater, signing - later phases, later specs.
- Hot reload by library swap. Phase 0 relaunches the process; the `< 500 ms` target is spec 0004.
- A component kit. Phase 0 ships the primitives the slice needs, styled by tokens.
- Embedded config, embedded assets, `$schema` generation and build-script codegen. Config loads at
  runtime until the bundler lands.
- Mobile.

## Decisions

### D1 - Six crates now, nine at 1.0

Phase 0 materialises `auro` (facade), `auro-core`, `auro-ui`, `auro-ipc`, `auro-macros`,
`auro-cli`. `auro-perms`, `auro-plugins`, `auro-bundle`, `auro-update`, `auro-sign` arrive with the
phase that needs them.

*Rejected:* creating all nine empty crates now. Empty crates invite speculative APIs and make the
workspace look finished when it is not. *Rejected:* folding IPC into `auro-core`. The command
boundary is the thing the project is built around; it gets its own crate from day one.

| Crate | Kind | Depends on | Responsibility |
|---|---|---|---|
| `auro` | lib | core, ui, ipc, macros | `App` builder, window attachment, `prelude`; the only crate an app depends on |
| `auro-core` | lib | ipc | lifecycle, config loading, state store, events, tokio runtime; **no renderer types** |
| `auro-ipc` | lib | - | `CommandArg`, `IpcResponse`, `Invoke`, registry, `AuroError` |
| `auro-macros` | proc-macro | - | `#[command]`, `generate_handler!` |
| `auro-ui` | lib | core | the only crate that names `gpui`; primitives, tokens, themes, `Render` re-export |
| `auro-cli` | bin | core | `new`, `dev`, `build`, `doctor`; no runtime logic |

Rules: no cycles; `auro-ui` holds no policy (no filesystem, no config reads); `gpui` appears in
exactly one manifest; `auro-core` never names a renderer type. The builder lives in the facade
because it is the one place that must know both the app model and the renderer.

*Rejected:* the builder in `auro-core`. It would force `gpui` into core's manifest or a
renderer-abstraction trait layer that buys nothing in Phase 0.

### D2 - GPUI is pinned exactly

```toml
gpui = "=0.2.2"   # in auro-ui only
```

Edition 2024, resolver 3, workspace MSRV **1.90** (the floor `tauri-bundler` 2.10 declares).
Exact pins, not ranges: GPUI is pre-1.0 and breaks on minor bumps. Upgrading is a deliberate
change with a changelog entry, not something `cargo update` does behind our back.

*Rejected:* a git dependency on Zed's `main`. It turns every `cargo build` into a lottery and is
how a compat layer becomes a fork.

### D3 - Two executors, one bridge

GPUI ships its own executors (smol-based). `Context::spawn` polls on the main thread; nothing in
GPUI runs a tokio reactor. A command body calling `tokio::fs` from GPUI's executor panics with
"no reactor running".

Therefore `#[auro::command]` is a **code transform, not metadata**: it wraps the body in
`auro_core::async_runtime::spawn(async move { /* body */ })` and awaits the task, so the body always
runs on Auro's multi-threaded tokio runtime no matter who awaits it. A sync command runs through
`spawn_blocking`. The runtime handle is a process-global installed by `App::run` (or the test
harness) before the first view exists.

The other rules:

- Views, entities and window state are `!Send` and never cross threads.
- Command arguments and return values are `Send + 'static`; results come back through
  `Entity::update` / `AsyncApp::update`, then `cx.notify()` schedules the repaint.
- Any `await` inside a command must not hold an entity borrow.

*Rejected:* one thread per command. Unbounded, untraceable, hostile to cancellation. *Rejected:*
running commands on GPUI's executor. `tokio` APIs would panic, and blocking work would stall the
window.

### D4 - Commands are Rust functions with an attribute

```rust
#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> { /* ... */ }
```

- Attribute options: `name = "..."`, `rename_all = "camelCase" | "snake_case"`. Async is detected
  from the `async fn` itself; a sync function is allowed and runs blocking.
- Arguments: `serde::Deserialize + schemars::JsonSchema + Send + 'static`, or `State<T>`.
- Result: `Result<T, AuroError>` with `T: Serialize + Send + 'static`.
- `generate_handler![save_document, load_document]` builds a `CommandHandler` that maps a command
  name to a typed dispatch. In Phase 0 it exists and is tested; it is **not** wired into `App`.
  Wiring arrives with plugins, when something other than first-party Rust calls it.
- The first-party call site is a plain function call (`api::save_document(...)`); the handler is
  the boundary for the future plugin and webview paths.
- `AuroError` is `#[non_exhaustive]` with `From<std::io::Error>` and `From<serde_json::Error>`.

*Rejected:* reusing `tauri-macros` with `root = "::auro"`. The macro also pulls
`tauri_utils::acl::read_allowed_commands` at expansion time, so reuse buys a proc-macro dependency
on a crate that is about to jump to 3.0.

### D5 - State is owned, never borrowed across threads

- `App::manage(value)` stores `T: Send + Sync + 'static` behind an `Arc`.
- `State<T>` is a cheap owned handle (`Arc<T>`, `Deref<Target = T>`), **not** `State<'_, T>`: a
  borrowing handle cannot be captured by the `Send + 'static` command future. The macro resolves
  `State` arguments before hopping to tokio, so the body receives the owned handle.
- Views read state through the app handle; one owner per piece of state.
- App-wide notifications are an enum in `auro-core`, emitted through the app handle. Window events
  stay GPUI events. No untyped broadcast payloads.

*Rejected:* a global `EventBus` with string topics. It is the failure mode the project exists to
avoid. *Rejected:* `State<'_, T>` copied from Tauri. Tauri's commands can borrow because its
invocation runs on a different boundary; ours cannot.

### D6 - Configuration

`Auro.toml` next to the app's `Cargo.toml`; TOML; sections `[app]`, `[dev]`, `[bundle]`,
`[plugins]`; platform overrides in `Auro.<platform>.toml` merged key by key. Full field table,
defaults and validation rules: [0002](0002-config.md).

- **Config is authoritative for identity and windows.** `app.identifier`, `app.product-name`,
  `app.version` and the window list come from the file. The builder attaches root views to windows
  by label: `App::new().root::<Editor>("main").run()`.
- Discovery, in order: `AURO_CONFIG` (a path), then `Auro.toml` in the current directory, then in
  the directory of the running executable. Missing file is an error with exit code 2.
- CLI overrides travel to the child process as `AURO_CONFIG_OVERRIDES`, a JSON Merge Patch. There
  is no other CLI-to-app channel.
- `app.version` versus `Cargo.toml` is checked by `auro build` and `auro dev` (which have the
  manifest), not by the running app.
- `App::run(self)` prints the `AuroError::Config` diagnostic and exits 2; `App::try_run(self) ->
  Result<(), AuroError>` is the non-exiting entry point used by tests.

Loaded at runtime in Phase 0. Embedded at bundle time in Phase 2, when there is a bundler to embed
it into.

*Rejected:* the Rust builder owning identity and window geometry. It makes `Auro.toml` decorative
and splits one truth across two files.

### D7 - The dev loop relaunches

`auro dev` = load config, `cargo build`, spawn the binary with `AURO_CONFIG_OVERRIDES`, watch
`src/**` and `Auro.toml`, kill, rebuild, relaunch. Ctrl-C stops the child and exits 0. No dev
server, no frontend directory, no static file serving.

*Rejected:* starting with the `cdylib` hot-reload architecture. It is the project's bet, but it is
not the first thing to build; a relaunch loop proves the app model without risking the schedule.

### D8 - Bundling is delegated

Phase 2 wraps `tauri-bundler` (`2.10.x`, pinned) behind `auro-bundle`, mapping our `[bundle]`
section into its `Settings`. It is the one Tauri crate we depend on. Its own docs warn that minor
releases may change the API, so an upgrade is a change that must run the three-OS CI.

*Rejected:* writing a bundler. `.dmg` + codesign/notarize + WiX + NSIS + AppImage + deb + rpm is
months of edge cases that are not the product.

### D9 - Window keys are honored where GPUI can express them

GPUI 0.2.2 has no max-size, no always-on-top, no per-window theme setter, and its decorations
switch is platform-specific. Phase 0 honors what the platform can express; the rest is parsed,
validated, and **reported by `auro doctor`** as "accepted, not honored on this platform". A config
key that silently does nothing is worse than one that says so.

*Rejected:* implementing platform-specific window code in Phase 0. It is a Phase 2 item, and it
belongs in `auro-ui` with the rest of the renderer.

## Interfaces

### CLI

| Command | Behaviour |
|---|---|
| `auro new <path>` | scaffold the tree below; refuse a non-empty directory |
| `auro dev` | build, spawn, watch, relaunch |
| `auro build` | `cargo build --release`; no packaging until Phase 2 |
| `auro doctor` | toolchain, GPUI pin, config parse result, platform, Linux dependencies, and config keys the platform cannot honor |
| `auro --version` | the workspace version |

### Generated project

```
demo/
  Auro.toml
  Cargo.toml            # auro = { path = "..." } in-repo, version = "0.x" once published
  src/
    main.rs             # auro::App::new().root::<Root>("main").run()
    views/mod.rs        # the root view
    services/mod.rs     # command functions, the `api` module the UI calls
  icons/                # placeholder + the sizes the bundler needs in Phase 2
```

### The slice, as the author writes it

```rust,ignore
use auro::prelude::*;

#[auro::command]
async fn save_document(path: PathBuf, body: String) -> Result<Saved, AuroError> {
    tokio::fs::write(&path, body).await?;
    Ok(Saved { path, bytes: body.len() })
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Button::new("save", "Save").on_click(cx.listener(
                |this, _, _window, cx| {
                    let doc = this.document.clone();
                    cx.spawn(async move |this, cx| {
                        let saved = api::save_document(doc.path, doc.body).await?;
                        this.update(cx, |this, cx| {
                            this.status = format!("Saved {} bytes", saved.bytes);
                            cx.notify();
                        })
                    })
                    .detach();
                },
            )))
            .child(self.status.clone())
    }
}

fn main() {
    auro::App::new()
        .root::<Editor>("main")
        .run();
}
```

The window comes from `Auro.toml`; `main` names it by label. This is the current GPUI 0.2 API:
`render` receives `&mut Window`, `listener` closures take `(this, event, window, cx)`, `spawn`
yields `(WeakEntity<T>, &mut AsyncApp)`, and `WeakEntity::update` takes any `AppContext`. The
sample in `README.md` is corrected from this spec.

## Acceptance

Runnables, in order, on a clean checkout:

1. `cargo build --workspace --locked` succeeds on Linux, macOS and Windows.
2. `cargo run -p auro-cli -- new /tmp/demo` creates the tree; `cargo check` inside it passes.
3. `cargo run -p auro-cli -- dev` inside `/tmp/demo` opens the window declared in `Auro.toml`; the
   button dispatches an async command and the rendered text changes; closing the window exits 0.
4. Ctrl-C during `auro dev` leaves no child process behind.
5. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked` pass.
6. Unit tests cover: config parse, platform merge, unknown-key rejection, `State` extraction, sync
   and async dispatch through `CommandHandler`, `AuroError` conversion, and `auro new` refusing a
   non-empty directory.
7. One headless test in the app crate: start Auro's runtime, render the root view through
   `TestAppContext`, await the command task, assert the view's `status` field, and assert the
   rendered tree contains that text via `debug_bounds`. GPUI has no text accessor, so the field is
   the contract and the tree check is structural.
8. Save-to-window time is measured once and recorded here. No target is claimed until measured.

## Open questions

| Question | Where it settles |
|---|---|
| Component kit: own primitives vs `gpui-component` | Phase 1 spec |
| Hot reload mechanism and its measurability | Spec 0004 |
| Plugin isolation: out-of-process first or in-process | Phase 3 RFC |
| Does release embed config or ship `Auro.toml` next to the binary | Phase 2 spec |
| Windows/Linux input, IME and a11y parity | Phase 2 audit |
