# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) from 1.0. Before 1.0, a
breaking change bumps the minor version; the patch version is for compatible fixes.

**No release has been tagged.** Nothing here is published on crates.io, and there is no
installable `auro-cli` yet.

## [Unreleased]

### Added

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
