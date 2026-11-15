# What Auro takes from Tauri

Evidence for the [foundation spec](0001-foundation.md). Studied source: `tauri-apps/tauri` at
commit `3aac98b` (2026-11-13), crate versions 2.12.x; published versions checked on crates.io on
2026-11-15.

The rule this document applies: **copy the shape, depend on the parts that are expensive and
webview-free, and never depend on anything that carries a web engine into our build graph.**

## Verdict per crate

| Crate | Reusable? | Why |
|---|---|---|
| `tauri-bundler` | **Yes, as a dependency** | No `tauri` dependency; the only Tauri crate it needs is `tauri-utils` for config/resource types (`crates/tauri-bundler/Cargo.toml:18`). Webview coupling is three narrow spots: the WebView2 installer plumbing, a `webkit2gtk` file copy in the AppImage, and the `__TAURI_BUNDLE_TYPE` binary patch, which degrades to a warning |
| `tauri-macos-sign` | **Yes, later** | Codesign/notarize helpers with no webview involvement (`crates/tauri-macos-sign`) |
| `tauri-utils` | **Shape only** | Webview-free (manifest has no `wry`/`webkit`/`tao`), but it is a heavy dependency: HTML parsing (`kuchikiki`/`dom_query`), `aes-gcm`, `brotli`, `plist`, `cargo_metadata` (`crates/tauri-utils/Cargo.toml:16-53`). We want its *ideas* - config schema, ACL - not its 30 transitive crates |
| `tauri-macros` | **Shape only** | Emits code against `::tauri` paths and says "Don't depend on this crate directly" (`crates/tauri-macros/src/lib.rs:7`). It supports `root = "crate"` (`src/command/wrapper.rs:45`), but `generate_handler!` also calls `tauri_utils::acl::read_allowed_commands` (`src/handler.rs:103`), so reuse drags `tauri-utils` into the proc-macro |
| `tauri-runtime` | **No** | Not webview-free at the crate level: unconditional `webkit2gtk`, `webview2-com`, `gtk` (`crates/tauri-runtime/Cargo.toml:41-46`), and `gtk::ApplicationWindow` leaks into `WindowDispatch` and `RawWindow` (`src/lib.rs:788-798`, `src/window.rs:655`) |
| `tauri-runtime-wry` | **No** | It is the webview (`pub use wry`, `src/lib.rs:98`) |
| `tauri-codegen` | **No** | Embeds assets, hashes/compresses them, injects CSP (`crates/tauri-codegen`) - all webview concerns |
| `tauri` | **No** | `wry` is a default feature (`crates/tauri/Cargo.toml:185-192`); `PlatformWebview` exposes the raw webview (`src/webview/mod.rs:153-195`) |
| `tauri-cli` | **Shape only** | Its command set and dev flow are the reference; the code is tied to `tauri-build`/`tauri-codegen` |
| `tauri-plugin` | **Shape only** | Plugin builder/hooks are generic enough to imitate |

## What we copy as shape

| Tauri | Auro | Note |
|---|---|---|
| `#[tauri::command]` + `generate_handler!` | `#[auro::command]` + `auro::generate_handler!` | Same authoring experience; the wrapper emits a `__cmd__<fn>` macro and a command-name macro (`crates/tauri-macros/src/command/wrapper.rs:155,297`) |
| `CommandArg` / `IpcResponse` / `InvokeError` | same names, our traits | JSON assumptions are confined to three blanket impls (`crates/tauri/src/ipc/command.rs:196-335`); our first-party path is an in-process typed call |
| `Manager::manage` + `State<'_, T>` | same | State is resolved per command from the app handle |
| `tauri.conf.json` sections (`app`, `build`, `bundle`, `plugins`) | `Auro.toml` sections (`app`, `dev`, `bundle`, `plugins`) | TOML instead of JSON, `build` renamed to `dev`, webview keys dropped |
| `capabilities/*.json` + `tauri-utils` ACL resolution | same file format and resolution idea | Scopes stay data; enforcement stays at the call site |
| `bundle_project(&Settings)` | `auro_bundle::bundle(&Config)` mapping into that call | Our config stays ours; the mapping is one module |

## What the Tauri study changes in our plans

1. **No runtime crate to reuse.** `tauri-runtime` is not an abstraction we can adopt; Auro writes
   its own `Runtime`-shaped seam over GPUI. The trait *shape* (window dispatch + handle) is still
   the right model.
2. **Bundling is not the place to be original.** Mirroring `tauri-bundler`'s `Settings` and its
   per-OS tool matrix (codesign/dmg, WiX/makensis, AppImage/deb/rpm) buys years of edge cases.
3. **MSRV floor becomes 1.90.** `tauri-bundler` 2.10.4 declares `rust-version = "1.90"`, edition
   2024 (`crates/tauri-bundler/Cargo.toml:13-14`); the same floor applies workspace-wide in Tauri
   (`Cargo.toml:45-46`).
4. **The bundler is not SemVer-stable.** Its own docs say minor releases may add fields and change
   or remove error variants (`crates/tauri-bundler/src/lib.rs:9-14`). Pin the minor and treat an
   upgrade as a change that needs a CI run on all three platforms.
5. **Version window.** crates.io carries `tauri`/`tauri-utils`/`tauri-runtime` at `3.0.0-alpha`
   while the stable line is 2.10-2.12. Depend on the 2.x line, pinned, and do not chase the alpha.

## GPUI findings that constrain the spec

Studied `gpui 0.2.2` (crates.io, 2026-10-30), edition 2024, license Apache-2.0, no declared
`rust-version`.

- The public API is `Application::new().run(|cx: &mut App| ...)` with `cx.open_window(WindowOptions,
  |window, cx| cx.new(|cx| View::new(cx)))` (`examples/hello_world.rs`, `src/platform.rs:1089`).
- `Render::render` takes `&mut Window`: `fn render(&mut self, window: &mut Window, cx: &mut
  Context<Self>)`.
- `Context::spawn` takes `AsyncFnOnce(WeakEntity<T>, &mut AsyncContext)` (`src/app/context.rs:237`),
  and `spawn_in` adds the window (`src/app/context.rs:661`).
- **`ViewContext` no longer exists** (`0` occurrences in the crate). The sample in our README and
  roadmap was written against a pre-0.2 GPUI and does not compile; it is corrected with this spec.
- `gpui-component` 0.9.1 (repo `longbridge/gpui-kit`) ships 75+ components, AccessKit roles, and
  headless UI integration testing. Phase 1's "~20 components" is a decision, not a given.

## Dev loop and build-time shape

- `tauri dev` is an orchestrator, not a code generator: resolve dirs, load and merge config
  (`helpers/config.rs:143-223`), rewrite `Cargo.toml` feature lists from config
  (`interface/rust.rs:140-158`), run `beforeDevCommand`, pick a runner, optionally start a built-in
  static file server (`dev.rs:235-265`), wait for the dev URL, watch folders
  (`dev.rs:99-327`). There is no frontend bundler in the CLI.
- `tauri-build` is a build-script helper for platform metadata, resources and ACL; context/assets
  codegen is opt-in behind a `codegen` feature (`tauri-build/src/lib.rs:481-489,932-939`).
- Asset embedding lives in `tauri-codegen` and is one branch of the generated `Context`
  (`tauri-codegen/src/context.rs:176-207`). The seam is `context_codegen` with
  `ContextData::assets: Option<Expr>` and a re-rootable `root: TokenStream`
  (`tauri-codegen/src/context.rs:37-42,134`).
- **Auro consequence:** there is no "static server" or "frontend dist" step to imitate. `auro dev`
  is: build the app crate, spawn the binary, watch, kill, rebuild, relaunch. Build-time codegen is
  needed only for the capability manifest and the embedded config, and both can start as runtime
  loads instead.

## Capabilities shape (ACL)

Tauri's ACL is separable from the webview, and Auro keeps its file format and resolution model.

- **Capability file** fields: `identifier`, `description`, `local`, `remote`, `windows`,
  `webviews`, `permissions`, `platforms` (`crates/tauri-utils/src/acl/capability.rs:117-215`).
  `windows` and `platforms` apply to Auro unchanged; `webviews` and `remote` do not exist here.
- **Discovery** is directory-based: `capabilities/**/*.{json,json5,toml}` next to the app crate
  (`crates/tauri-build/src/acl.rs:437-459`). An explicit list in the config replaces discovery.
- **Permission files** declare `default`, `set` and `permission`; a permission names `commands`
  with `allow`/`deny` and `scope` with `allow`/`deny` values (`crates/tauri-utils/src/acl/manifest.rs:19-46`,
  `src/acl/mod.rs:181-221`). Deny wins over allow.
- **Resolution happens at build time**, producing `capabilities.json`, `acl-manifests.json` and
  `allowed-commands.json` in `OUT_DIR` (`crates/tauri-build/src/acl.rs:432,472,482`); the runtime
  only evaluates the resolved structure.
- **App-level commands** are not gated by a capability in Tauri; only plugin commands are. That is
  the same boundary Auro already decided on.

## Deliberately not copied

- The webview escape hatch, CSP injection, asset embedding and code signing of web assets.
- `tauri-build`'s Cargo-feature rewriting and `generate_context!` re-rooting: Auro's build story is
  a plain `cargo build` plus, later, a generated capability manifest.
- Mobile targets. Tauri 2 supports them; Auro does not claim to.
