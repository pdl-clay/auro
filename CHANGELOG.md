# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) from 1.0. Before 1.0, a
breaking change bumps the minor version; the patch version is for compatible fixes.

**No release has been tagged.** Nothing here is published on crates.io, and there is no
installable `auro-cli` yet.

## [Unreleased]

### Added

- `auro-sign`: Ed25519 signing keys, detached signatures and SHA-256 file hashes; key files are
  0600 on Unix and debug output hides the seed.
- `auro-update`: signed update manifest format v1 with canonical JSON payloads, artifact
  size/SHA-256 verification, and `auro manifest`.
- `auro-update`: runtime `Updater` checks signature/platform/semantic version, downloads with
  connect/per-call/per-read timeouts and an exact manifest-size bound, verifies SHA-256, and stages
  content-addressed artifacts atomically; `stage_from_path` uses the same path for local artifacts.
  Staged files that still verify are reused. Apply paths live in specs 0014/0025; deltas and staged
  rollout are not implemented (spec 0013). The facade exposes this as `auro::update` behind the
  opt-in `updater` feature; `PublicKey` is re-exported for key loading.
- `auro-update`: Windows `WindowsInstaller` runs staged MSI (`msiexec /i ... /qn /norestart`) or
  NSIS (`/S`) installers through an injectable, shell-free command runner and maps code `3010` to
  `restart_required`. macOS `MacAppInstaller` extracts a staged `.app.zip`, swaps exactly one
  top-level `.app` into place, and returns a receipt with rollback/discard. The facade re-exports
  each installer on its host OS (spec 0025).
- `auro-update`: Linux `AppImageInstaller` replaces a staged AppImage atomically, sets mode `0755`,
  and retains a hard-link (or copy) backup. `InstallReceipt` can roll the previous file back or
  discard the backup after the new version is accepted. Windows/macOS apply is in spec 0025;
  package-manager apply, elevation, deltas and staged rollout are not implemented (spec 0014,
  ADR 0013).
- `auro-update`: `ApplyJournal` records one apply attempt atomically (`Prepared`, `Applied`,
  `RebootRequired`, `Failed`, `RolledBack`, `Restarted`), with artifact/target/backup and relaunch
  command. The
  restart helper re-executes the app with `AURO_UPDATE_RESTART_HELPER`, waits on a lock until the old
  process exits, then applies a prepared `AppImage`/MSI-NSIS/`.app` plan through the platform
  installers or relaunches an already-applied update. The helper re-verifies the manifest and
  artifact with the app-provided public key, records `Failed` or `RebootRequired` diagnostics, retains the
  backup and relaunches with the helper variable removed (specs 0026, ADR 0015).
- `auro-perms`: deny-by-default capability `Policy` with `fs`/`net`/`shell`/`clipboard`/
  `notification`/`keychain` grants, compiled glob scopes for the scoped capabilities, typed
  `Denied` errors, and `auro::perms` re-exports (spec 0016).
- `auro-core`: `AppInstance` and `App::single_instance()` use a per-user advisory file lock to
  refuse a secondary process before GPUI starts. The secondary forwards its arguments and working
  directory over a local socket, then gets `AuroError::Instance` (`run` exits 2); the primary
  receives `InstanceMessage` on its event pump. Automatic primary-window activation is app policy
  (spec 0015).
- `auro-services`: `Autostart` and `Autostart::for_current_app` use `auto-launch` to enable,
  disable and query login-item autostart on Linux/macOS/Windows. Windows always uses the
  per-user registry mode; macOS uses a Launch Agent (spec 0017).
- `auro-services`: `Notifications` shows desktop notifications through `notify-rust`, with a
  pluggable backend trait for tests (spec 0018).
- `auro-services`: `Keychain` stores, reads and deletes one secret through `keyring`, with a
  pluggable store trait for tests (spec 0019).
- `auro-services`: `Shortcuts` registers and unregisters global hotkeys through `global-hotkey`
  and dispatches pressed-hotkey callbacks on a background thread, with a pluggable backend trait
  for tests (spec 0020).
- `auro-services`: `Tray` shows and hides a tray icon, sets its tooltip, installs a flat menu and
  dispatches menu/click callbacks through `tray-icon`, with a pluggable backend trait for tests
  (spec 0021).
- `auro-ui`: `Button` is a focusable tab stop and activates its click handler on Enter or Space
  without modifiers; held keys do not repeat (spec 0023). AccessKit and IME remain upstream gaps.
- `auro-services`: `AppLinks` parses, filters and queues URL-scheme deep links;
  `register_scheme` writes per-user Windows/Linux registrations (macOS stays packager-owned)
  (spec 0022).
- `auro`: platform services are opt-in (`services`), and `tray` is a separate feature because
  `tray-icon` requires GTK/AppIndicator development libraries on Linux.
- `auro new`: an installed CLI now emits `auro = "0.1.0-alpha.1"` (the CLI version) instead of an
  invalid `path = "0.0.0"`; in-repo runs still emit the workspace path.
- `[Unreleased] alpha preparation`: workspace and all internal dependencies are
  `0.1.0-alpha.1`; every crate has a crate-root README, homepage, keywords and categories; the
  facade's services/tray features and the scaffold's installed-version mode are in place. Spec 0024
  records the version, metadata, feature and publish order contract. Publication is still pending.
- `[bundle.macos]` and `[bundle.windows]` signing inputs (identity, entitlements, hardened runtime,
  certificate thumbprint, timestamping, custom sign command/args); Windows keeps WebView2 skipped.
- `auro generate-key --out <path>` and `auro manifest --artifact ... --key ... --out ...`.
- `auro-bundle` and `auro bundle`: map `[bundle]` onto `tauri-bundler 2.10.5`, build the release
  binary, and emit host artifacts. Linux `.deb`, `.rpm` and `.AppImage` and Windows MSI/NSIS were
  produced for fixture apps; the Windows installers installed, ran the app and uninstalled
  cleanly, and a macOS `macos-latest` runner produced an unsigned `.app` and `.dmg` through the
  `bundle-smoke` workflow. AppImage preflight rejects a missing square PNG with a typed error instead of letting
  the bundler panic. Windows installers skip WebView2 (Auro has no webview), run the bundler from
  the app directory for icon/resource resolution, and strip `canonicalize`'s `\\?\` prefix before
  invoking WiX/NSIS. Spec 0010 and ADR 0011 record the delegation and its limits.
- `examples/log-viewer`: the Phase 1 reference app (log viewer plus settings window) built with
  Auro and application-level `gpui-component 0.5.1`. It exercises the toolkit seam, typed events
  from a background thread, lifecycle cleanup, and `auro::windows` open/close by label.
- `windows` manager: `open`, `close`, `focus`, `is_open` and `handle` by the labels declared in
  `Auro.toml`. Startup uses the same path; open is idempotent, close is attributed by the existing
  lifecycle observer, and an undeclared label or missing root returns `AuroError::Window`
  (spec 0009).
- Command schemas: `#[auro::command]` now derives an input object schema (non-state arguments,
  `rename_all` keys, required list, hoisted `$defs`) and an output schema from
  `Result<T, AuroError>`; `CommandHandler` exposes `schemas()` and `schema(name)`. `schemars` is
  re-exported through `auro::ipc` (spec 0008).
- Typed application events: `EventSender` (cloneable, `Send + Sync`, auto-managed as
  `State<EventSender>`), `App::on_event::<E>`, and a main-thread `emit(event, cx)`. Background
  producers queue events; a GPUI foreground pump dispatches them in order. No strings, no
  worker-thread UI mutation (ADR 0010, spec 0007).
- `App::state::<T>()` and `App::event_sender()` expose managed state and the sender before the app
  runs, for views, setup closures and first-party command calls.
- `LifecycleEvent` and `App::on_lifecycle`: main-thread handlers for `Ready`, `WindowOpened`,
  `WindowClosed` and `Exiting`, in registration order. Auro tracks the window handles it opened so
  a close is attributed to the configured label; the last close emits `Exiting` before quitting
  (spec 0006). User-defined app events and runtime window management are the next core slices.
- `App::setup` and `App::root_with`: an app can initialize the component toolkit it chose (for
  example `gpui_component::init`) and build its window root (for example a toolkit `Root`), while
  `App::root` stays the `Default`-view shortcut. `Theme` is re-exported for the config handoff.
  Auro itself ships no component toolkit (ADR 0009, spec 0005).
- `auro doctor` reports a non-system `theme` as toolkit-owned, pointing at `App::setup`.
- `auro doctor` reports the `gpui` version the app resolves, read from its `Cargo.lock` (spec 0001).
- `[dev] hot = true` runs the hot-patch devserver (`dx serve --hot-patch`) instead of rebuilding and
  relaunching: the loop forwards devserver output, timestamps saves and prints
  `auro: hot patch applied (<took> ms, <elapsed> ms after the save)`. Requires dioxus-cli 0.7.10
  (`dx` on `PATH` or `AURO_DX`) and the app's `hot-reload` feature; scaffolded apps now emit
  `Dioxus.toml` and that feature.
- Scaffolded views expose a data-only hot boundary (`ViewData` from `view_data()`, wrapped by
  `auro::hot::hot` when the feature is on) and log `auro: view data -> …` when it changes, so a
  reload is observable. Verified on niri/Wayland: one patch (3198 ms) changed
  `Clicked accent=0x1f2937` to `Clicks accent=0x7c2d12` in the same window. On Windows 11 the patch
  applies (4679 ms, handler fires, loop survives) but the patched boundary did not run — no
  `view data -> Clicks` line — so reload is not yet demonstrated there.

- `auro dev` emits one JSON trace line per save (`event`, `window`, `save_to_build_ms`,
  `build_to_window_ms`, `save_to_window_ms`); the app prints `auro:trace window_requested <label>`
  when `AURO_DEV_TRACE` is set, so save-to-visible is computed without a stopwatch (spec 0004, D6).
  First measured run in relaunch mode: 2730 ms median save→repaint-requested on niri/Wayland, warm
  cache, debug build.
- `auro dev` labels each relaunch by what happened to the snapshot: `auro: restarted (state restored)`
  or `auro: restarted (state reset: <reason>)`. A first start has nothing to restart, so it prints no
  label (spec 0004, D4).
- `auro-core::snapshot`, re-exported from `auro`: a JSON envelope with `format_version`, build
  identity and per-entry `version`/payload, plus the `ReloadSnapshot` opt-in trait. `restore`
  reports a reset (`Missing`, `VersionMismatch`, `Decode`) instead of defaulting silently, and an
  unknown format version fails to decode (spec 0004, D3). `Snapshot::save_to` writes atomically and
  `load_from` separates a missing file from an unreadable one; `Autosave` captures watched entries on
  the caller's thread and writes after a 250 ms quiet period (`tick`) or on `flush`, to
  `<platform data dir>/<identifier>/dev-snapshot.json`. Not yet wired to a reload or restart.
- Scaffolded apps opt their counter into that snapshot: `CounterState` implements
  `ReloadSnapshot`, the view restores on start (`auro: state restored from <path>` or
  `auro: state reset: <reason>`) and saves 350 ms after a click; the generated manifest gains
  `serde` with the `derive` feature.
- Workspace of six crates: `auro` (builder), `auro-core` (config, state), `auro-ipc` (commands,
  runtime bridge), `auro-macros` (`#[command]`, `generate_handler!`), `auro-ui` (GPUI re-exports,
  `Button`, tokens) and `auro-cli` (`new`, `dev`, `build`, `doctor`).
- `Auro.toml` parsing: platform overrides, RFC 7396 merge, and validation that rejects unknown keys,
  duplicate window labels and paths escaping the app directory.
- Typed commands: the attribute moves the body onto the Auro runtime (GPUI polls its own executor),
  resolves `State<T>` as an owned handle, and dispatch is covered by tests for sync, async, state,
  renaming and unknown commands.
- `auro new` scaffolds an app that builds; until the first release it wires the `auro` path
  dependency and the vendored `xattr` patch, since a generated app is its own workspace. `auro dev`
  rebuilds and relaunches on change; `auro doctor` reports the toolchain, native libraries, and
  config keys the platform cannot honor.
- Vendored `xattr 0.2.3` under `vendor/`: `libc 0.2.191` dropped `ENOATTR` on Linux and the
  `gpui` -> `gpui_http_client` -> `zed-async-tar` chain still references it.
- Documentation book (`docs/`, mdBook) covering architecture, decision records, platform support,
  performance budget and repository standards.
- Decision records 0001-0007: Tauri's app model with GPUI's renderer, Rust-only typed IPC,
  capabilities at the plugin and content boundary, main-thread ownership, hot reload with state
  preservation, dual licensing, and no styling language in 1.0.
- `rfcs/` with the proposal process and a template.
- `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue forms, pull request template, `AGENTS.md`.
- `CHANGELOG.md`, `.editorconfig`, `rust-toolchain.toml`, `.gitattributes`.
- CI: docs build gate, Linux system dependencies on the lint/test leg, concurrency cancellation,
  least-privilege permissions, timeouts, third-party actions pinned to commit SHAs.
- CI: GitHub Pages deployment for the documentation book.
- Headless GPUI integration test in `auro-ui` that renders a root view through `TestAppContext` and
  asserts `status == "Ready"` and a non-empty rendered tree via `debug_bounds` (spec 0001
  acceptance 7).
- Headless GPUI integration test in `auro` that clicks a `Button`, dispatches an async
  `#[auro::command]` through the scaffold-style flow, and asserts the view state updates
  (`clicking_button_dispatches_async_command_and_updates_view`), covering the dispatch/update part
  of spec 0001 acceptance 3.
- `auro-ui::Button` `debug_selector` (test-support) and `auro-ui` `test-support` feature for
  headless element bounds in tests.
- Draft [spec 0004](docs/src/specs/0004-hot-reload.md) (hot reload): save-to-visible contract,
  versioned snapshot/restore, failure/fallback semantics, measurability, staged acceptance,
  Slice 0 (2026-11-16) feasibility findings, and Option 1 consolidation evidence (2026-11-16);
  specs index/SUMMARY and architecture/roadmap updated to reference it.
- [ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md) (supersedes ADR 0005):
  retains the hot-reload goal/contract and rejects `cdylib`/`libloading` swap of GPUI view code
  after the Slice 0 spike; mechanism under revision with prototype evidence recorded in spec 0004.
- Option 1 hot-reload consolidation spike evidence (2026-11-16, throwaway prototypes, not
  integrated): Linux x86_64/Wayland, `gpui = "=0.2.2"`, `subsecond 0.7.10`/`dioxus-devtools
  0.7.10`/`dx 0.7.10`. In-process tip-crate function-level patching via explicit `HotFn`
  boundaries with host-owned GPUI application/window/entities/state showed warm view changes in
  ~271-450 ms save-to-first-new-render in minimal scenarios (s1 median 371.5 ms, N=10; s6 median
  346 ms, N=20, 20/20 stable) with host state preserved; helper/layout/style changes, coordinated
  transient struct layout changes, multi-module tip-crate edits (functionally applied), build/
  syntax-error recovery, and non-dx run viability observed. Documented constraints: tip-crate-only
  hot boundary (outside-boundary/init-captured work not retroactive), tip-crate statics/reset on
  patch, `thread_local!` in a patched path fails patch generation while app stays on previous
  code, and tooling/layout/snapshot integration plus representative Auro-scaffold D6 measurement
  still pending.
- Windows 11 x86_64 plain (non-GPUI) Option 1 prototype evidence (2026-11-16, WinBoat guest,
  throwaway, not integrated): same-process tip-crate `HotFn` hot-patching via
  `subsecond 0.7.10`/`dioxus-devtools 0.7.10`/`dx 0.7.10` (rustc/cargo 1.99.0 MSVC, MSVC Build Tools
  14.44.35207, clean target, external logs, `dx serve --hot-patch --interactive false --platform
  desktop`) completed build in 30.66 s and applied 3/3 warm `src\main.rs` patches in 484/504/499 ms
  with monotonic counter continuity `rev=0 1..361 -> rev=1 421..781 -> rev=2 841..1261 -> rev=3
  1321..1741` in the same process (30 sampled app lines, `ERR_LEN=0`,
  `C:\Users\pdl\wvmt\op1win\logs\plain-proofwin-20261116152539.*`). This demonstrates mechanism
  viability on Windows x86_64 for this minimal plain case only; GPUI 0.2.2 + subsecond/dx sources
  were prepared on the guest but not built/tested, and `auro dev` integration/production/
  representative-D6 claims remain unproven. Caveats: corrupted reused target
  (`libdeps-62094602.a: unknown file type`), inconclusive hidden/background runs, and watcher noise
  from in-project verbose log redirection.
- Windows 11 x86_64 front/back advanced prototype evidence (2026-11-16, WinBoat guest, plain
  non-GPUI `HotFn` tip crate, not integrated): visual UI edits inside the hot boundary applied as
  three warm hot-patches (590/523/527 ms) while host-owned state stayed in the same process —
  `session=s5500`, `items`/`backend_ticks` continued across `ui0` blue/120/Save/light →
  `ui1` green/180/Salvar/dark → `ui2` orange/240/Publicar/contrast + `radius=12 padding=24
  icon=rocket`. A source change to the init-captured backend interval (500→1500 ms) was
  hot-patched into the frontend boundary but the already-running backend task kept
  `active_interval_ms=500` (not retroactive). Restarting the app with the changed source produced a
  new process (`session=s9516`, first render `ui_v=ui2`, `init_backend_ms=1500`) and a 1500 ms
  tick cadence, i.e. the init-captured change only takes effect on restart
  (`adv-fix-20261116161242.*`, `adv-fix-restart-20261116161242.*`, `ERR_LEN=0` both phases).
- GPUI 0.2.2 on Windows (2026-11-16, WinBoat guest): the prototype built in 181 s
  (`target-gpui-win2\debug\gpui-subsecond-spike.exe`, 23.9 MB) and rendered in the active
  interactive console session launched via a scheduled task (`/RU pdl /IT`): pid 708 in session 1,
  206 `[hot_view]` renders, `counter` 0→204, `stable=host-owned-state` preserved, no panic. The same
  executable launched from the SSH/session-0 context panicked with `Creating DirectX renderer` /
  `Creating DirectX resources` / `0x887A0022` (`DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`), and a
  `dx serve --hot-patch` run from that context built in 199.20 s then failed the same way before any
  render. A `dx serve --hot-patch` run finally executed in the interactive session itself (scheduled
  task, `SESSION=1`, clean `target-gpui-hot3`): build 473.85 s, app rendered (`[hot_view] ...
  label=v0`, `counter` 0→43), the first patch compiled and applied — `Hot-patching: src\main.rs took
  1453ms`, `[patch_applied] t_ms=5760` — and the process then died with
  `thread 'main' has overflowed its stack` / `exit code: 0xc000041d` before rendering `label=v1`.
  So on Windows, GPUI render works and the patch is applied, but hot reload is **not usable**: the
  applied patch crashes the main thread (evidence `gpui-hot3-20261116-170828.*`,
  `gpui-hot3-current.result.txt`). Isolation on the same guest: patching a leaf `HotFn`
  (`fn(u64) -> u64`) applied twice in one process with no crash (`bumped=1050` at counter 50,
  `bumped=2193` at counter 193, 303 renders after), and the crash is not stack- or
  address-resolution-related (jump table resolved: 746 entries, RVAs `0xcab0`/`0xcb10`; reproduced
  with a PE-verified 16 MiB stack). A **framework-free `ViewData` hot boundary** — the patched
  function returns `label`/`bg`/`width`/`extra` and host code builds the `div()` — hot-reloaded UI
  values twice in the same window (`1324 ms`, `1084 ms`; `data=v1` green/600, `data=v2` brown/640
  with `panel=dark radius=12 icon=rocket`, 120/187 renders after each, `counter` 4→175,
  `stable` preserved, QMP screenshot `20261116-175532`). Constraint retained: the Windows patch DLL
  is 89,600 bytes carrying no GPUI (only `main` exported), so element-returning view code cannot be
  the patched unit; `HotFn::try_call` also bypasses the jump table when `debug_assertions` is off.
- `auro::hot` experimental in-process hot boundaries behind the off-by-default `hot-reload` feature
  and `AURO_HOT_RELOAD=1`: `hot()` / `hot_with()` call through `subsecond::HotFn`, `App::try_run`
  installs the dev-host hook, and boundaries return framework-free data because the Windows patch
  unit cannot build GPUI elements. No CLI mode, no snapshot/restore, no change classification;
  `auro dev` still rebuilds and relaunches.

### Fixed

- `[dev] run-args` is passed to the spawned app (it was parsed and ignored), and hot mode says so
  when it cannot forward the args because `dx` owns the process.
- `[app.windows]` options `transparent` and `center` are now applied instead of parsed and
  ignored: transparency reaches the window background, and `center = false` leaves placement to
  the platform. `decorations = false` now requests client-side decorations, which is how GPUI 0.2
  expresses an undecorated window (honored on Wayland; `auro doctor` still names platforms that
  ignore it). A single `min-width` or `min-height` is now honored instead of being dropped.
- `auro build` failed on every app: `Cargo.toml` was parsed as a bare TOML value instead of a
  document, so the version check always reported `invalid Cargo.toml`. It now parses the manifest
  and proceeds to `cargo build --release`.
- `auro dev` now runs the `Auro.toml` ↔ `Cargo.toml` version check (spec 0002); only `auro build`
  performed it before, so a mismatch went unnoticed in the dev loop.
- App configuration falls back to the running executable's directory when the current directory has
  no `Auro.toml`, as specs 0001/0002 require, and the missing-file error names both places searched.
- `[bundle] targets` lists are validated against the known package formats and the host's
  producible set (`app`/`dmg` on macOS, `msi`/`nsis` on Windows, `appimage`/`deb`/`rpm` on Linux);
  an unsupported format is an error, not a silent skip (spec 0002, validation rule 5).
- An `Auro.toml` read failure no longer repeats the path
  (`configuration error in <path>: cannot read the file: ...`).

- A window's configured title now reaches the compositor on Wayland. GPUI 0.2.2 applies
  `WindowOptions::titlebar.title` on X11, Windows and macOS but not on Wayland, so `App` sets the
  title on the window it opens; niri now reports `auro-demo` instead of `(unset)`. Covered by a
  headless unit test that opens a window and reads its title back.

- `auro dev` no longer closes the running window when a rebuild fails: the app is stopped only
  after the replacement binary exists, so a typo keeps the previous UI alive and the session
  reports `build failed, previous UI still running` (spec 0004, D4).

- `auro new` on Windows wrote `path = "C:\..."` into the generated `Cargo.toml`; TOML reads `\U` as a
  unicode escape and rejected the manifest, so the app could not build at all. Generated paths now use
  forward slashes, which cargo accepts on every platform.
- Generated apps declare `[workspace]`, so a parent `Cargo.toml` above the app cannot capture it. On
  Windows an orphan manifest in `%USERPROFILE%` broke the build before any crate compiled.
- `auro dev` on Windows runs the app from `target/auro-dev/<stem>-<pid>-<n>.exe`: Windows locks a
  running image, so relinking the cargo artifact failed with the previous window still open. Failing
  to stage a new copy is recoverable and keeps the previous UI; copies are named per dev session so a
  stale process cannot collide with the next one.
- A failed rebuild prints the tail of cargo's stderr instead of a generic "cargo build failed", which
  had hidden a locked-executable error on Windows.
- `Snapshot::load_from` tolerates a UTF-8 BOM, which some tools write by default; without this the
  snapshot was reported unreadable and state silently started from defaults.
- `auro dev` rebuilt in a loop: the watcher treated file *reads* as changes, and the app opening
  `Auro.toml` on every start re-triggered the build that had just started it.
- Closing the last window now quits the Auro app (GPUI `on_window_closed` + `quit`), and `auro dev`
  detects child process exit instead of idling until a file change or Ctrl-C.
- Configuration errors read `configuration error in <path>: <reason>` instead of running the path
  and the reason together.
- CI installs `libxkbcommon-x11-dev` on Linux so the workspace test link finds `-lxkbcommon-x11`
  (the `ubuntu-latest` leg failed at `25da81f`; macOS, Windows and the `ubuntu-22.04` build passed).

### Changed

- Phase 2 is in progress: the bundler slice is in tree and Linux packaging is verified. Phase 1 is
  closed; README/README.pt-BR/book status reflect both.
- Phase 1 is closed: the toolkit seam, lifecycle events, typed events, command schemas, window
  manager and the log-viewer reference app are in place, and README/README.pt-BR/book status say
  Phase 1 is complete. The command registry remains unwired until plugins; hot reload stays
  experimental.
- Auro ships no component toolkit: Phase 1 no longer promises ~20 components in `auro-ui`.
  Toolkits are application dependencies, initialized in `App::setup` and wrapped with
  `App::root_with`; the book documents `gpui-component 0.5.x` as the tested line while GPUI is
  pinned (ADR 0009, spec 0005). `auro-ui` keeps re-exports, tokens and minimal primitives.
- Phase 0 is closed: specs 0001 and 0002 are `Implemented`, their acceptance items are recorded as
  met, and `README.md`, `README.pt-BR.md` and the book status say so. App-wide events and lifecycle
  moved to Phase 1; scaffold icons move to Phase 2 with the bundler.
- `AGENTS.md`, the specs and this changelog describe the workspace instead of a documentation-only
  repository.
- `CONTRIBUTING.md` gains the development setup, the gates, the commit and sign-off rules, and the
  ADR-versus-RFC split; the design-first framing stays.
- `ROADMAP.md` package-format count and hot-reload comparison character corrected; the Electron
  memory figure and the binary-size claim qualified; Phase 0 progress recorded, and the resolved
  license moved out of open questions.
- `README.md` and `README.pt-BR.md` mirrored: status reflects the existing workspace and in-progress
  Phase 0 (unpublished, no released CLI), with the full code sample kept in sync.
- README and roadmap link to the documentation book, the support matrix, the contributing guide
  and the security policy.
- Spec 0001, the platform support table and the performance budget record acceptance status, the
  measured ~60 s save-to-relaunch time, and CI results: run `43776830848` for `5a5d389` is green on
  macOS, Windows, `ubuntu-latest` and `ubuntu-22.04` (the `ubuntu-latest` test link failed on
  `-lxkbcommon-x11` at `25da81f`, run `43775097578`, before `libxkbcommon-x11-dev` was added).
- Docs reviewed for consistency: introduction, architecture, crates, standards and repository
  guidelines now describe the current workspace state; illustrative API notes no longer claim the
  workspace does not exist.
- Spec 0004, ADR 0005/0008 references, architecture, roadmap, AGENTS, specs index and overview
  corrected after the Slice 0 viability spike and Option 1 consolidation: `cdylib`/`libloading`
  swap of GPUI view code rejected for the current stack; subsecond/dx tip-crate `HotFn` patching
  recorded as an evidenced experimental candidate (prototype, Linux x86_64/GPUI 0.2.2, minimal
  cases) with constraints and pending integration/representative D6 measurement; `auro dev`
  remains relaunch by default and restarts remain honest fallbacks.
