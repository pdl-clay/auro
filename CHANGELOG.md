# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) from 1.0. Before 1.0, a
breaking change bumps the minor version; the patch version is for compatible fixes.

**No release has been tagged.** Nothing here is published on crates.io, and there is no
installable `auro-cli` yet.

## [Unreleased]

### Added

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
  versioned snapshot/restore, failure/fallback semantics, measurability, staged acceptance, and
  Slice 0 (2026-11-16) feasibility findings; specs index/SUMMARY and architecture/roadmap
  updated to reference it.
- [ADR 0008](docs/src/decisions/0008-hot-reload-mechanism-revision.md) (supersedes ADR 0005):
  retains the hot-reload goal/contract and rejects `cdylib`/`libloading` swap of GPUI view code
  after the Slice 0 spike; mechanism under revision.

### Fixed

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
- Spec 0004, ADR 0005/0008, architecture, roadmap, AGENTS, specs index and overview corrected
  after the Slice 0 hot-reload viability spike: `cdylib`/`libloading` swap of GPUI view code
  rejected as infeasible for the current stack, relaunch remains default, and the mechanism is
  under revision toward a same-process approach.
