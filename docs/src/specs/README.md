# Specifications

A specification describes **what a slice of work must do and the contracts it must satisfy**,
precisely enough that implementation is mechanical. It is not a proposal and not a decision record.

| Document | Question it answers | Mutability |
|---|---|---|
| [`ROADMAP.md`](https://github.com/pdl-clay/auro/blob/main/ROADMAP.md) | what ships in which phase, and the exit criteria | living |
| **spec** (here) | what exactly must exist, with which interfaces and acceptance checks | living until the phase closes |
| [ADR](../decisions/README.md) | why a choice was made, and what it costs | immutable once accepted |
| [RFC](https://github.com/pdl-clay/auro/blob/main/rfcs/README.md) | a design that needs discussion before it is decided | frozen at decision |

A spec that changes an architectural decision needs a superseding ADR in the same pull request. A
spec that adds scope beyond the phase it belongs to needs an RFC first.

## Format

Every spec carries:

- **Status** - `Draft`, `Approved`, `Implemented`, `Superseded by NNNN`
- **Scope** and **Out of scope** - the boundary is the useful part
- **Decisions** - what this spec fixes, each with the alternative it rejected
- **Interfaces** - types, functions, files, CLI surface, config keys, error cases
- **Acceptance** - checks that prove it works, runnable by someone who did not write it
- **Open questions** - what is deliberately deferred, and to where

## Rules

- A spec describes behaviour that can be verified. "It feels right" is not acceptance.
- Examples in a spec are normative only when marked as such; otherwise they illustrate.
- An interface change lands in the spec before it lands in code, in the same pull request.
- Numbers (sizes, latencies, limits) are either measured or marked as a target. Never invented.
- When implementation teaches something the spec got wrong, the spec is corrected in the same
  pull request that learns it.

## Index

| # | Spec | Phase | Status |
|---|---|---|---|
| - | [What Auro takes from Tauri](tauri-basis.md) | evidence | living |
| [0001](0001-foundation.md) | Foundation: workspace, runtime seam, commands, state, dev loop | 0 | Implemented |
| [0002](0002-config.md) | `Auro.toml` schema, merge rules, validation | 0 | Implemented |
| [0004](0004-hot-reload.md) | Hot reload: save-to-visible with preserved state, measurability, mechanism under revision | 1, 3 | Draft |
| [0005](0005-toolkit-seam.md) | Toolkit seam: setup hook, app-owned root, no framework toolkit | 1 | Implemented |
| [0006](0006-lifecycle-events.md) | Lifecycle events: ready, window open/close, exiting | 1 | Implemented |
| [0007](0007-events.md) | Typed application events: sender, main-thread pump, handlers | 1 | Implemented |
| [0008](0008-command-schema.md) | Command schemas: input/output metadata on the handler | 1 | Implemented |
| [0009](0009-window-manager.md) | Window manager: open/close/focus by configured label | 1 | Implemented |
| [0010](0010-bundling.md) | Bundling: map `[bundle]` to `tauri-bundler`, host formats, `auro bundle` | 2 | Implemented (Linux, Windows and macOS verified) |
| [0011](0011-signing.md) | Signing keys, artifact hashes, platform signing config | 2 | Implemented |
| [0012](0012-update-manifests.md) | Signed update manifests: format, verify, CLI | 2 | Implemented (download/stage in 0013; AppImage apply in 0014) |
| [0013](0013-updater-runtime.md) | Updater runtime: check, download, verify and stage | 2 | Implemented (AppImage apply in 0014; other installers pending) |
| [0014](0014-appimage-apply.md) | AppImage apply: atomic replacement and rollback receipt | 2 | Implemented (Linux only) |
| [0015](0015-single-instance.md) | Single instance: advisory lock before startup | 2 | Implemented (forwarding/activation pending) |
| [0016](0016-capabilities.md) | Capability manifests and enforcement | 2 | Implemented (host wiring pending) |
| [0017](0017-autostart.md) | Autostart service | 2 | Implemented |
