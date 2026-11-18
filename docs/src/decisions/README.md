# Decision records

Architecture decisions live here as short, numbered records. They exist so that a contributor -
or an AI assistant - can tell the difference between "this is how it works" and "this was chosen,
and here is what it costs".

## Format

Every record has the same four sections:

| Section | Content |
|---|---|
| **Status** | `Proposed`, `Accepted`, `Superseded by NNNN`, `Deprecated` |
| **Context** | the forces at play, without advocacy |
| **Decision** | one sentence, then what it implies |
| **Consequences** | what gets easier, what gets harder, what is now forbidden |

A record is immutable once accepted. To change a decision, write a new record that supersedes it;
do not edit history.

## When to write one

Write a record when a choice:

- is expensive to reverse (wire format, threading model, licence, dependency direction);
- constrains future code (a rule reviewers must enforce);
- would otherwise be rediscovered in every review thread.

Do not write one for naming a private function, choosing a test helper, or anything a `Cargo.toml`
diff explains better than prose.

## Relationship to RFCs

- **ADR** - a decision that has been made, recorded after the fact.
- **RFC** - a proposal that needs discussion before it is made. See
  [rfcs/README.md](https://github.com/pdl-clay/auro/blob/main/rfcs/README.md).

An accepted RFC that changes the architecture produces an ADR. The RFC keeps the discussion; the
ADR keeps the conclusion.

## Index

| # | Decision | Status |
|---|---|---|
| [0001](0001-tauri-app-model-gpui-renderer.md) | Tauri's app model, GPUI's renderer | Accepted |
| [0002](0002-rust-only-frontend.md) | Rust-only frontend and typed IPC | Accepted |
| [0003](0003-capabilities-at-the-real-boundary.md) | Capabilities at the real boundary | Accepted |
| [0004](0004-main-thread-ownership.md) | Main thread belongs to GPUI | Accepted |
| [0005](0005-hot-reload-as-the-bet.md) | Hot reload with state preservation | Superseded by [0008](0008-hot-reload-mechanism-revision.md) |
| [0006](0006-dual-license.md) | Dual MIT/Apache-2.0 | Accepted |
| [0007](0007-no-styling-language.md) | No styling language in 1.0 | Accepted |
| [0008](0008-hot-reload-mechanism-revision.md) | Hot reload mechanism revised after feasibility spike | Accepted |
| [0009](0009-toolkits-are-app-level.md) | Toolkits are the application's choice | Accepted |
| [0010](0010-typed-events-main-thread.md) | Typed events, delivered on the main thread | Accepted |
| [0011](0011-bundling-delegates.md) | Bundling delegates to `tauri-bundler` | Accepted |
| [0012](0012-release-trust.md) | Release trust: Ed25519 manifests, delegated platform signing | Accepted |
| [0013](0013-appimage-updates.md) | AppImage updates: in-place replacement with a backup receipt | Accepted |
| [0014](0014-platform-update-apply.md) | Platform updates use the native installer or bundle swap | Accepted |
| [0015](0015-self-helper-restart.md) | Restart through the app executable and a durable journal | Accepted |
| [0016](0016-deltas-and-rollout.md) | Delta patches are base-bound; rollout is deterministic | Accepted |
| [0017](0017-linux-package-elevation.md) | Linux package elevation is explicit pkexec, never self-root | Accepted |
| [0018](0018-linux-package-rollback.md) | Package rollback requires an explicit older artifact | Accepted |
| [0019](0019-macos-package-elevation.md) | macOS package elevation uses an explicit AppleScript prompt | Accepted |
