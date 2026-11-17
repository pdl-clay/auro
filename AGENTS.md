# Repository Guidelines

Instructions for AI coding assistants and contributors working in this repository.

**Read this first: the workspace exists; the framework is pre-alpha.** Eleven crates under `crates/`
compile and their tests pass, `auro new` produces a project that builds against this checkout, and
the UI layer is one primitive. Nothing is published. Anything described below as *planned* is a
target recorded in the roadmap or a spec, not code you can find with a symbol search. Never present
a planned API as an existing one, and do not invent files to make a design look implemented.

## Non-negotiable: documentation and changelog move with the code

Every change that alters behaviour, a public API, a config key, a CLI flag, or an architectural
decision ships **in the same pull request** as:

| Touched | Must also change |
|---|---|
| anything a user can observe | `CHANGELOG.md` under `## [Unreleased]` |
| a contract in a spec | the spec in `docs/src/specs/` |
| an architectural decision | a new or superseding ADR in `docs/src/decisions/` |
| the pitch, quickstart or feature list | `README.md` **and** `README.pt-BR.md` |
| a convention in this file | `AGENTS.md` itself |
| a phase, target or exit criterion | `ROADMAP.md` |

A pull request missing them is incomplete, not "later". If you cannot name which document a change
touches, the change is not understood yet. When in doubt, update the doc and the changelog: a
redundant entry costs a line, a stale doc costs a user's afternoon.

## Project Overview

Auro is an application framework for native Rust desktop apps: **Tauri's app model with GPUI as the
renderer**. It keeps the expensive, unglamorous parts of Tauri (config, capabilities, plugins,
bundling, signed updater, dev server, single-instance, multi-window) and replaces the webview with
[GPUI](https://gpui.rs), Zed's GPU-accelerated Rust UI toolkit. The frontend is Rust: no HTML, no
CSS engine, no JavaScript in the app runtime.

Status: pre-alpha, designed in public. No release, no crates.io publication, no installable CLI.

Four non-goals are enforced in review, not aspirational:

| Not | Consequence for code |
|---|---|
| A webview wrapper | the webview is a scoped Phase 4 escape hatch for OAuth/embedded docs |
| A GUI toolkit | widgets are GPUI's; Auro is the application layer |
| A GPUI fork | pin upstream, keep one thin compat module, never `gpui` outside `auro-ui` |
| "CSS for Rust" | no styling language in 1.0 - builder API plus tokens only |

Authoritative documents: `README.md` (pitch), `ROADMAP.md` (phases and targets),
`docs/src/architecture/` (how it fits), `docs/src/decisions/` (why, immutable).

## Architecture & Data Flow

Workspace of twelve crates total (eleven materialized now, one planned), layered top-down:

```
auro-cli        new . dev . build . bundle . add . doctor
auro-core       App . WindowManager . Config . Lifecycle . Command registry . State store . Event bus . Instance
auro-ui         GPUI re-export + tokens           |  auro-ipc  typed bus  |  auro-perms capabilities
auro-plugins    in-process (cdylib)  |  out-of-process RPC
auro-bundle . auro-sign . auro-update
```

The diagram is a layering: a crate may use the layer below, `auro-core` composes the rest, cycles
are a design error. The exact `Cargo.toml` edges are not fixed yet; do not treat the diagram as a
dependency graph.

Threading and data flow, in order:

1. GPUI owns the main thread. Input arrives, a view handler runs there.
2. Cheap mutation happens inline; anything that blocks goes to a command.
3. A command is a plain `async fn` on the `tokio` runtime, declared with `#[auro::command]` and
   returning `Result<T, AuroError>`; file I/O and network calls happen here, never in render.
4. The result returns to the main thread and is applied by updating an `Entity`, then `cx.notify()`.
5. Render happens on the main thread. No I/O, no locks held across `await`, no entity access off-thread.

Typed IPC is the defining decision: the schema is derived from the Rust signature (`schemars`) and
the call site is a Rust call, so a renamed command or a wrong argument type is a compile error. There
is no `invoke("name", {...})` string dispatch in first-party code.

Capabilities guard only the real runtime boundary: plugin code, untrusted content, and the future
webview hatch. Grants are deny-by-default, scoped (`fs`, `net`, `shell`, `clipboard`, `notification`,
`keychain`), compiled into the host, and checked at the call site that performs the effect.
`auro-perms` returns a verdict and performs no I/O.

Hot reload is the project's existential bet: keep one GPUI `Application`/window/event loop in a
long-lived host and apply reloadable UI changes in-process (same process/type universe) with
opted-in state preserved as a versioned `serde`-compatible snapshot, target `< 500 ms`
save-to-visible for a warm view-only change (target until measured per case, per
[spec 0004](docs/src/specs/0004-hot-reload.md)). A Slice 0 viability spike rejected
`cdylib`/`libloading` swap of GPUI view code for the current stack (GPUI 0.2.2). Option 1
consolidation (2026-11-16, throwaway prototypes, not integrated) evidenced
`subsecond 0.7.10` + `dioxus-devtools 0.7.10`/`dx 0.7.10` function-level tip-crate patching via
explicit `HotFn` boundaries with host-owned GPUI state as a viable experimental candidate on
Linux x86_64/Wayland in minimal cases (~271-450 ms save-to-first-new-render, host state preserved)
with documented constraints (tip-crate-only boundary; outside-boundary/init-captured work not
retroactive; tip-crate statics reset on patch; `thread_local!` in a patched path can fail patch
generation while the app stays on previous code; tooling/layout/snapshot integration and
representative Auro-scaffold D6 measurement pending). `auro dev` remains relaunch by default; a
state-preserving process restart is the documented fallback and is never presented as hot reload.

## Key Directories

| Path | Purpose |
|---|---|
| `crates/` | the workspace: `auro` (facade + builder), `auro-bundle` (packaging via `tauri-bundler`), `auro-sign` (Ed25519 keys/signatures), `auro-update` (signed manifests, staging and Linux AppImage apply), `auro-core` (config, state, lifecycle, single-instance guard), `auro-ipc` (commands, runtime bridge), `auro-perms` (capability manifest and enforcement), `auro-services` (platform services: autostart, notifications, keychain, global shortcuts, tray), `auro-macros` (`#[command]`, `generate_handler!`), `auro-ui` (the only GPUI crate), `auro-cli` |
| `vendor/xattr` | vendored `xattr 0.2.3` with the Linux `ENOATTR` fix; see `[patch.crates-io]` in `Cargo.toml` |
| `docs/` | mdBook source for the published documentation book |
| `docs/book.toml` | book config: `src = "src"`, `build-dir = "book"`, `create-missing = false` |
| `docs/src/SUMMARY.md` | the table of contents; a chapter missing from disk fails the build |
| `docs/src/architecture/` | overview, crates, threading/IPC, capabilities, hot reload |
| `docs/src/decisions/` | immutable ADRs, `NNNN-title.md`, indexed in `README.md` |
| `docs/src/reference/` | platform support, performance budget, repository standards |
| `rfcs/` | design proposals: process in `README.md`, template at `0000-template.md` |
| `.github/workflows/` | `ci.yml` (format, clippy, test, docs), `docs.yml` (Pages deploy) |
| `.github/ISSUE_TEMPLATE/`, `.github/PULL_REQUEST_TEMPLATE.md` | report and change shapes |

Do not add a second documentation tree. Architecture prose goes in `docs/`, proposals in `rfcs/`,
decisions in ADRs, and the pitch in `README.md`.

## Development Commands

Documentation (works today):

```bash
cargo install mdbook --version 0.4.40 --locked
mdbook serve docs          # http://localhost:3000, live reload
mdbook build docs          # output in docs/book (gitignored)
```

Rust gates (the workspace exists; run all of them before pushing):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
```

Linux build dependencies (same set CI installs):

```bash
sudo apt-get install -y --no-install-recommends \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libvulkan-dev libasound2-dev libfontconfig-dev libssl-dev pkg-config \
  libgtk-3-dev libayatana-appindicator3-dev
```

CI runs those gates on Linux, macOS and Windows plus an `ubuntu-22.04` floor job, and
`mdbook build docs`. `Cargo.lock` is committed and CI passes `--locked`.

## Code Conventions & Common Patterns

- **Rust stable**, edition 2024, `rustfmt` defaults. `rust-toolchain.toml` pins the channel and
  components; do not add a `rustfmt.toml` that fights the default style.
- **SPDX header** on every `.rs` file: `// SPDX-License-Identifier: MIT OR Apache-2.0`.
- **Errors are typed.** `AuroError` is `#[non_exhaustive]`, has `From<std::io::Error>` and
  `From<serde_json::Error>` so `?` works in command bodies. `unwrap`/`expect` only in tests or for
  states the type system proves unreachable, with a comment.
- **Async:** `tokio` for background work; `cx.spawn` from UI; hop back with `cx.update` before
  mutating. Clone data out of a borrow before `await`. No `std::fs`, blocking HTTP, `Command::output`
  or `sleep` on the main thread.
- **Naming:** crates `auro-*` in manifests and `auro_*` in code; commands are `snake_case` verb
  phrases (`save_document`); events are an enum in `auro-core`; entities are nouns (`Editor`).
- **Public API:** doc comment on every public item; add to `auro::prelude` only when it is stable
  enough to promise; `#[non_exhaustive]` on public enums.
- **GPUI style:** builder chains, tokens, no second styling system:
  `div().flex().flex_col().gap_2().p_4().child(...)`.
- **`unsafe`** requires a `// SAFETY:` comment naming the invariant that makes it sound.
- **Comments explain why.** ADRs and `docs/` carry the reasoning; don't duplicate it in code.
- **Docs voice:** English, present tense, direct, no emoji, no marketing adjectives. Examples that
  cannot compile yet use a `rust,ignore` fence and a note saying the API does not exist.
- **Commits:** Conventional Commits (`feat|fix|docs|refactor|perf|test|build|ci|chore|revert`),
  imperative, 72-character subject, signed off with `git commit -s` (DCO, no CLA).
- **Changelog:** add a `CHANGELOG.md` entry under `[Unreleased]` for any user-visible change.

## Important Files

| File | Why it matters |
|---|---|
| `README.md` | the pitch and the only comparison/performance tables users see |
| `README.pt-BR.md` | translation; must mirror `README.md` section for section |
| `ROADMAP.md` | the only place phases, targets and exit criteria are defined |
| `AGENTS.md` | this file |
| `CLAUDE.md` | one-line import of `AGENTS.md` for Claude Code; never add content here |
| `docs/src/reference/repository-standards.md` | the normative convention list; follow it over habit |
| `docs/src/decisions/0007-no-styling-language.md` | the rule that kills the most plausible scope creep |
| `rfcs/README.md` | when a change needs an RFC instead of a pull request |
| `CONTRIBUTING.md` | gates, commit style, licensing |
| `SECURITY.md` | private disclosure path; never a public issue |
| `CODE_OF_CONDUCT.md` | Contributor Covenant 2.1 |
| `CHANGELOG.md` | Keep a Changelog, SemVer from 1.0 |
| `LICENSE-MIT`, `LICENSE-APACHE` | dual licence, user's choice |
| `.github/workflows/ci.yml` | format/clippy/test/docs gates and the Linux floor job |
| `.github/workflows/bundle-smoke.yml` | macOS `.app`/`.dmg` packaging smoke on a GitHub runner |
| `.github/workflows/docs.yml` | builds the book and deploys it to GitHub Pages |
| `rust-toolchain.toml`, `.gitattributes`, `.editorconfig` | toolchain pin; LF endings and indentation so the Windows leg is stable |

## Runtime/Tooling Preferences

- **Runtime:** Rust stable via `rustup`. No Node, no npm, no JS toolchain anywhere in the repo, and
  none for the docs site either.
- **Docs toolchain:** mdBook, pinned to `0.4.40` in CI and in `CONTRIBUTING.md`. Keep the pin in sync
  if you bump it.
- **Build system:** plain Cargo. No Makefile, no `justfile`, no custom task runner; commands live in
  CI and in `CONTRIBUTING.md`.
- **Lockfile:** `Cargo.lock` is committed and CI runs with `--locked`. A dependency change without a
  lockfile update must fail, not resolve silently.
- **CI:** GitHub Actions. Three-OS matrix (ubuntu, macOS, Windows) for gates, plus an `ubuntu-22.04`
  job because that is the published Linux floor. Least-privilege `permissions: contents: read`,
  concurrency cancellation, explicit timeouts, and every third-party action pinned to a full commit
  SHA with its version in a trailing comment. Never replace a pinned SHA with a moving tag.
- **Dependabot:** `github-actions` and `cargo`, both weekly. It updates the pinned SHAs and the
  lockfile; a dependency bump must keep `--locked` green.
- **Vendored patches:** a dependency that cannot build is patched under `vendor/`, with the reason
  and the removal condition in the `[patch.crates-io]` comment. Never patch silently.
- **Release:** nothing is released and no workflow exists. Do not add a release workflow before
  Phase 1.

## Testing & QA

`cargo test --workspace --locked` is the gate. It covers config parsing and merging, command
dispatch (sync, async, state, rename), `auro new`, CLI/dev helpers, and a headless GPUI render test
in `auro-ui` (`TestAppContext`, asserting view status and a non-empty rendered tree via
`debug_bounds`).

Expectations:

- Unit tests live beside the module; integration tests in `tests/`. `cargo test --workspace --locked`
  is the gate; CI runs it on all three OSes.
- A behaviour change needs a test that fails without it. A test that only re-states an implementation
  detail is deleted in review.
- Micro-benchmarks use `criterion`. Startup, RAM and binary size are measured by a separate
  process-level harness, not by a unit benchmark.
- The performance budget (`docs/src/reference/performance-budget.md`) is a CI gate only once it is
  implemented; until then its status column states what is measured or unmeasured and must not be
  presented as a result.
- Platform rows move from "unverified" to "verified" only when CI exercises them on every pull
  request and a maintainer confirms a real desktop session. Do not upgrade a support claim because a
  build or test run succeeded once.
- Documentation QA: `mdbook build docs` must pass, every `SUMMARY.md` target must exist, internal
  links must resolve, and `README.md`/`README.pt-BR.md` must stay in sync.

## Change Checklist

1. Does it change an architectural decision? Write a superseding ADR; never edit an accepted one.
2. Is it a new design that needs discussion? Write an RFC first.
3. Does it touch the roadmap's claims (targets, phases, platform list)? Update `ROADMAP.md` in the
   same change, or do not make the claim.
4. Does it add a user-visible behaviour? Add a `CHANGELOG.md` entry.
5. Did it change `README.md`? Mirror it in `README.pt-BR.md`.
6. Run the gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs`. For doc-only changes, `mdbook build docs`
   is the required proof; cargo gates apply when Rust code or workspace behaviour changes.
