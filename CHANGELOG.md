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

### Fixed

- `auro dev` rebuilt in a loop: the watcher treated file *reads* as changes, and the app opening
  `Auro.toml` on every start re-triggered the build that had just started it.
- Configuration errors read `configuration error in <path>: <reason>` instead of running the path
  and the reason together.

### Changed

- `AGENTS.md`, the specs and this changelog describe the workspace instead of a documentation-only
  repository.
- `CONTRIBUTING.md` gains the development setup, the gates, the commit and sign-off rules, and the
  ADR-versus-RFC split; the design-first framing stays.
- `ROADMAP.md` package-format count and hot-reload comparison character corrected; the Electron
  memory figure and the binary-size claim qualified.
- `README.pt-BR.md` mirrored to `README.md` after the status table, badges and license section
  landed, including the full code sample and the missing hot-reload sentence.
- README and roadmap link to the documentation book, the support matrix, the contributing guide
  and the security policy.
