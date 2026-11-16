# Repository standards

This page is the contract a change to this repository is measured against. It exists so a
contributor, a reviewer, or an AI assistant does not have to infer the project's conventions from
the last commit.

## Baseline files

| Path | Enforces |
|---|---|
| `README.md` | English entry point: what it is, status, how to get involved |
| `README.pt-BR.md` | Portuguese mirror of `README.md` |
| `ROADMAP.md` | the only place phases, dates and targets are defined |
| `CONTRIBUTING.md` | how to build, test and submit |
| `SECURITY.md` | how to report a vulnerability privately |
| `CODE_OF_CONDUCT.md` | Contributor Covenant 2.1 |
| `CHANGELOG.md` | user-visible changes, Keep a Changelog + SemVer |
| `LICENSE-MIT`, `LICENSE-APACHE` | dual licence, see [ADR 0006](../decisions/0006-dual-license.md) |
| `.editorconfig` | LF endings, UTF-8, 2-space YAML/TOML, 4-space Rust |
| `AGENTS.md` | instructions for AI coding assistants |
| `rfcs/` | proposals that need discussion before a decision |
| `docs/` | the published book (mdBook) |
| `.github/workflows/ci.yml` | format, lint, test, docs build |
| `.github/ISSUE_TEMPLATE/`, `.github/PULL_REQUEST_TEMPLATE.md` | the shape of a report or a change |

Absent on purpose: `CODEOWNERS` (one maintainer, the default reviewer is already correct),
`release.yml` (nothing to release), funding, stale bots and labelers.

## Language

- **English is the source of truth** for the README, the book, issues, pull requests, commit
  messages and code comments.
- **`README.pt-BR.md` is a translation, not a fork.** It must contain the same sections, tables and
  code samples as the English README. A change to one that is not mirrored in the other is an
  incomplete change.
- Do not translate identifiers, crate names, commands or error messages.
- Portuguese is fine in Discussions, issues and reviews; the artefact that lands in the tree is
  English.

## Commits and pull requests

- **Conventional Commits.** `type(scope): imperative subject`, type one of `feat`, `fix`, `docs`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Subject at most 72 characters, no
  trailing period, body explains *why*.
- **Signed-off-by required** (`git commit -s`), DCO 1.1. There is no CLA.
- **Squash merge.** The pull request title becomes the commit subject, so it must satisfy the
  rule above.
- **One concern per pull request.** Formatting churn, refactor and behaviour change do not share
  a diff.

## Versioning and changelog

- **SemVer.** Before 1.0, a breaking change bumps the minor version; the patch version is for
  compatible fixes. From 1.0, standard SemVer applies and the roadmap's API freeze takes effect.
- **`CHANGELOG.md` is written by hand** at pull-request time under `## [Unreleased]`, in Keep a
  Changelog sections (`Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`). No
  generated changelog until releases exist.
- Crates are published together at the same version; a workspace-local version skew is a bug.

## Documentation

- The book builds with **mdBook** from `docs/` and is deployed to GitHub Pages.
- `docs/book.toml` sets `create-missing = false`: every entry in `docs/src/SUMMARY.md` must exist,
  and a chapter removed from disk but left in the summary fails the build.
- New chapters are one idea, linked from `SUMMARY.md`, and written in the same voice as the rest:
  direct, present tense, no marketing adjectives, no emoji.
- API examples that cannot compile yet are fenced `rust,ignore` and carry a one-line note saying
  the API does not exist.
- **ADR vs RFC.** A decision that has been made goes to `docs/src/decisions/` and is immutable. A
  proposal that needs discussion goes to `rfcs/` and is numbered. An accepted RFC produces an ADR.

## Rust conventions

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and
  `cargo test --workspace --locked` are the gates, and CI runs them on Linux, macOS and Windows.
- **`Cargo.lock` is committed.** This workspace produces binaries, so the lockfile is part of the
  build; `--locked` makes a dependency change without a lockfile update fail in CI instead of
  silently resolving differently.
- **Dependency licences should be checked, not assumed.** Once `cargo-deny` is configured, a
  `deny.toml` should allow the set compatible with `MIT OR Apache-2.0` and flag incompatible
  licences on transitive bumps. Until that configuration lands, licence review remains a manual part
  of dependency changes.
- Every `.rs` file starts with the SPDX identifier:

  ```rust
  // SPDX-License-Identifier: MIT OR Apache-2.0
  ```

  The workspace inherits `license = "MIT OR Apache-2.0"`; a crate that deviates is a review
  blocker.
- `unsafe` requires a `// SAFETY:` comment stating the invariant that makes it sound.
- Errors are typed. Library code returns `Result<T, AuroError>`; `unwrap`/`expect` are for tests
  and for states the type system proves unreachable, and the latter needs a comment.
- No `println!` for diagnostics in library code; use `tracing`.
- Public items carry a doc comment. A public item without one fails `missing_docs` once that lint
  is enabled at 1.0.

## CI

Gates and supporting jobs, all required to be green before merge:

1. `cargo fmt --all --check` (Linux leg)
2. `cargo clippy --workspace --all-targets --locked -- -D warnings` (Linux, macOS, Windows)
3. `cargo test --workspace --locked` (Linux, macOS, Windows)
4. `cargo build --workspace --locked` on `ubuntu-22.04` (published Linux floor)
5. `mdbook build docs`
6. workflow lint (`actionlint`)

A red gate is never merged and never "fixed" by disabling the check.

Every third-party action is pinned to a full commit SHA with the version in a comment
(`uses: actions/checkout@11d5960... # v4`). Dependabot proposes the bumps, so the pin is updated
deliberately rather than following a moving tag under us. Workflows declare least-privilege
`permissions`, a `concurrency` group, and a `timeout-minutes` per job that can hang.
