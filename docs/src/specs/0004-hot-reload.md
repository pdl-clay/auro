# 0004 - Hot reload

**Status:** Draft - design and planning; implementation not started

Spec 0001 closed Phase 0 with a relaunch loop and recorded ~60 s save-to-visible on the development
machine (relink dominates). This spec defines the mechanism that must replace that loop for UI work:
a dev host that swaps a reloadable UI library while preserving restorable state, how save-to-visible
is measured, and the fallback when the swap cannot be performed.

It realises [ADR 0005](../decisions/0005-hot-reload-as-the-bet.md) and the
[hot reload architecture](../architecture/hot-reload.md). Where this spec learns from implementation
it must be corrected in the same pull request.

## Scope

- The `auro dev` hot-reload flow: watch, build, swap or fallback, and user-visible reporting.
- The host/UI boundary for development: process ownership, reloadable artifact, and exported
  contract.
- Snapshot, restore, and invalidation of state across a reload.
- Reload eligibility: which source changes may hot-swap and which must restart.
- Failure handling: build errors, load/ABI errors, restore errors, panics, and cancellation.
- Measurability: the definition and recording of save-to-visible latency.
- Staged delivery from experimental hot reload (Phase 1) to GA with state preservation (Phase 3).

## Out of scope

- Shipping hot updates to released binaries (`auro-update`, Phase 2). This spec covers the dev
  loop only.
- Hot reload of plugins or out-of-process plugin hosts (Phase 3), except stating that plugin
  reloadability is a separate contract.
- Function-level patching as a required mechanism. It is an optimisation; the required contract is
  the observable swap/restore behaviour and latency budget, not a specific patching technique.
- Mobile targets.
- Preserving state that is inherently non-serialisable or external (open sockets, OS handles,
  unmanaged threads, GPU resources owned outside the snapshot contract) except making loss visible.
- A stable cross-compiler or cross-version ABI for third parties. The host and UI library are built
  together under a pinned toolchain and dependency set.

## Baseline (measured)

| Path | Observed | Notes |
|---|---|---|
| Phase 0 relaunch (`auro dev`) | ~60 s save-to-visible | Development machine, single visible change, relink dominates; watcher reacts once per edit (spec 0001 status, 2026-11-15). Not a hot reload. |

Targets below are targets until measured by the method in [D6](#d6---measurability).

## Decisions

### D1 - A long-lived dev host swaps a reloadable UI library

`auro dev` in hot-reload mode runs a **host process** that owns the GPUI `Application`, windows,
event loop, Auro runtime, managed host state, and the snapshot store for the lifetime of the dev
session. The application UI is compiled as a **reloadable dynamic library** (`cdylib`) artifact.
On a successful UI build the host snapshots reloadable state, tears down reloadable entities and
tasks, unloads the previous library, loads the new library, performs a version/ABI handshake,
restores state, and repaints. The window stays open across a successful swap.

*Rejected:* recompiling and relinking the whole app process on every save as the hot path. It is the
current ~60 s bottleneck. *Rejected:* claiming a process restart is hot reload (forbidden by ADR
0005). *Rejected:* a host that discards the window on every swap, because it destroys the
“feels native” loop this project exists to provide.

### D2 - Host-stable and reloadable code have an explicit boundary

Code that must survive a library unload lives in the host-stable side: process bootstrap, window
lifecycle, the tokio runtime, host-owned managed state containers, snapshot persistence, and the
loader. Code that defines views and UI behaviour lives in the reloadable library.

The generated project layout and the `auro` API must make this boundary explicit. A single-crate
bin scaffold (Phase 0 shape) is sufficient for relaunch mode but is not sufficient for cdylib swap;
hot-reload-capable projects require a reloadable UI target and a host entry that can run it. The
exact scaffold shape (separate `ui` crate, dual-target crate, or generated host wrapper) is fixed by
the implementation of this spec and recorded here when chosen; until then, implementations must not
assume the Phase 0 `src/main.rs` + `src/views` layout is reloadable without structural change.

The reloadable library exports a small, versioned C-ABI contract (illustrative, non-normative until
implemented):

```rust,ignore
// exported by the reloadable UI library
#[no_mangle]
pub extern "C" fn auro_ui_abi_version() -> u32;

#[no_mangle]
pub unsafe extern "C" fn auro_ui_mount(host: *const HostApi, out: *mut UiApi) -> ReloadStatus;

#[no_mangle]
pub unsafe extern "C" fn auro_ui_snapshot(sink: *mut SnapshotSink) -> ReloadStatus;

#[no_mangle]
pub unsafe extern "C" fn auro_ui_restore(snapshot: *const SnapshotView) -> ReloadStatus;

#[no_mangle]
pub unsafe extern "C" fn auro_ui_unmount() -> ReloadStatus;
```

The contract must be sufficient to: verify compatibility, mount roots into host-provided windows,
capture a snapshot, restore from a snapshot, and unmount before unload. `HostApi`/`UiApi` carry
function pointers and opaque handles only; no Rust types cross the boundary except through
host-owned, versioned, `repr(C)` data or serialized bytes.

*Rejected:* passing Rust trait objects, entities, or generic types across `dlopen` boundaries.
*Rejected:* relying on incidental symbol/layout compatibility without an explicit handshake.

### D3 - State survives as a versioned, opt-in snapshot

The host owns the snapshot store. A snapshot is a versioned envelope containing named, typed entries
for state that opts into preservation. At minimum the format records:

- `format_version` (snapshot schema version, integer).
- a build/session identity sufficient to diagnose mismatches (e.g. UI library hash or build id).
- per-entry payloads: a stable key, an entry version, and serialized bytes (the canonical encoding
  is `serde`-compatible; the wire encoding is fixed at implementation time and recorded here).

State categories:

| Category | Owner | Reload behaviour |
|---|---|---|
| View/UI state explicitly marked restorable | Reloadable library, via snapshot contract | Serialized before unload, restored after mount. On decode/version mismatch, reset that entry and report it. |
| Host-managed state outside the unloaded library | Host | Preserved in memory across swap when its type/definition is host-stable; never passed through the unloaded library as a borrowed Rust reference. |
| Ephemeral/UI-local state not opted in | Reloadable library | Reset on swap. Reset must be visible in dev diagnostics, not silent. |
| External/non-serialisable resources | OS/runtime | Recreated or dropped according to the resource contract; loss reported. |

A view or state unit opts into preservation by implementing the reload snapshot/restore hooks for
its stable key. “Stable key” means stable across saves for the same logical UI element within a dev
session (window label + root/view identity as defined by the API). Renaming/removing a key is a
reset of that entry.

*Rejected:* snapshotting arbitrary memory or entity graphs implicitly. *Rejected:* storing entity
handles in process-global `static`s (forbidden by ADR 0005). *Rejected:* a silent fallback to
default state on mismatch.

### D4 - Successful swap, failed build, and fallback are distinct outcomes

| Outcome | Condition | Host behaviour | User-visible label |
|---|---|---|---|
| Hot-swapped | UI build succeeds, handshake passes, restore completes (with or without per-entry resets) | Keep window and host state; mount new UI; repaint | “reloaded” with duration and any reset warnings |
| Build failed | `cargo`/UI build fails | Keep previous library mounted and interactive; do not unload it | compiler/build diagnostics; “previous UI still running” |
| Swap failed, recovered | Build succeeds but load/handshake/mount/restore fails and previous UI can remain valid | Keep or remount previous UI as applicable; do not present as reloaded | explicit failure reason |
| State-preserving restart (fallback) | Swap cannot be completed safely, or a configured/required restart change occurred | Persist snapshot to a host-controlled location, restart the app/host session, restore on boot | “restarted (state restored)” or “restarted (state reset: \<reason\>)” - never “hot reloaded” |
| Hard failure | Restart/restore impossible | Exit non-zero with diagnostic | error |

A failed build must never unload the currently running UI (architecture contract). Tasks spawned by
the reloadable library that can outlive a swap must be cancellable; `unmount`/teardown runs before
unload, and the host must not call into an unloaded library (forbidden by ADR 0005).

*Rejected:* unloading on failure. *Rejected:* leaving tasks running against unloaded code.
*Rejected:* presenting a restart as hot reload.

### D5 - ABI, toolchain, and dependency compatibility are enforced, not assumed

Host and reloadable UI are compiled with the same Rust toolchain channel, target triple, and pinned
`gpui`/`auro-*` versions for a dev session. The handshake checks, at minimum, `abi_version` and a
build identity; a mismatch is a swap failure (D4), not undefined behaviour. Debug/release profile
mixing is either rejected explicitly or defined as supported and tested - the implementation must
choose one and record it.

Linux `dlclose` may defer unloading while threads or TLS from the library remain alive. The spec
requires that teardown quiesces reloadable tasks before unload and that leaked unloads are detected
and surfaced in dev diagnostics; a safe fallback (restart) is preferred over use-after-unload.

*Rejected:* promising a stable ABI across toolchain upgrades in 0.x. *Rejected:* silent best-effort
loads on mismatch.

### D6 - Measurability

“Save-to-visible” is defined as: **wall-clock time from the editor save event observed by the
watcher to the first frame presenting the updated UI** (or, when pixels cannot be observed
headlessly, to the host’s “reload committed and repaint requested” event - the method must be stated
with the measurement). Each recorded measurement states:

- machine/OS, CPU class, storage class (if relevant), debug/release, warm/cold incremental state;
- what changed (view-only, style/token, command signature, config, dependency) and file count;
- whether the outcome was hot-swapped, reset entries, or fallback restart;
- median and spread over N >= 10 edits after warm-up (or an explained smaller N).

The host must emit machine-readable timing events in dev mode (log/trace or structured output)
sufficient to compute this metric without manual stopwatch use. `AURO_DEV_TRACE` may be extended for
this purpose. Numbers published in docs, roadmap, or performance budget are measured or explicitly
marked as targets.

Target (from ADR 0005 / roadmap): **< 500 ms save-to-visible** for a view-only change on a warm
incremental build, with state preserved. This is a target, not a guarantee, until recorded under D6.

*Rejected:* unverifiable latency claims. *Rejected:* counting only compile time while excluding
load/restore/repaint.

### D7 - Reload eligibility and staged delivery

Not every edit is safely swappable. The dev loop classifies changes:

| Change class | Expected path | Notes |
|---|---|---|
| Reloadable UI/view code and resources within the UI library | Hot-swap | Primary path; must preserve opted-in state per D3. |
| Host-stable/bootstrap code, host dependency graph, toolchain/config affecting the host build | State-preserving restart or full restart | Classified and reported; never a silent no-op. |
| `Auro.toml` / window-identity changes | Reload config in host where safe; otherwise restart with explanation | Must not corrupt running host assumptions. |
| ABI/build identity mismatch after rebuild | Swap failed → fallback per D4/D5 | |

The classification rules are part of the implementation contract and must be documented for app
authors once implemented.

Staged delivery (aligns roadmap):

- **Experimental (Phase 1):** opt-in hot reload in `auro dev` for supported project layouts;
  view-only swaps; snapshot/restore for opted-in view state; visible diagnostics and timing; fallback
  to state-preserving restart where swap is not possible. Correctness and honesty over coverage.
- **GA (Phase 3):** default dev experience for supported apps; documented state contract stable enough
  for third-party app authors; measured latency published per platform tier; CI/perf-budget integration
  as the budget becomes enforceable.

*Rejected:* enabling an unsafe/undefined swap path by default. *Rejected:* blocking the entire
feature on function-level patching research.

## Interfaces

This section fixes the surfaces this spec introduces or constrains. Signatures shown as
`rust,ignore` are illustrative until implemented; normative details are the behavioural contracts
above and the keys/items listed here.

### CLI (`auro-cli`)

| Surface | Behaviour |
|---|---|
| `auro dev` | Default remains the documented relaunch loop until hot reload is declared stable for the selected project; hot reload is enabled by an explicit opt-in (flag and/or `[dev]` config key defined at implementation time and recorded in spec 0002/CLI docs). |
| `auro dev` (hot-reload mode) | Starts/uses the dev host, watches configured paths, builds the reloadable artifact(s), performs swap or fallback per D4, streams diagnostics, and emits timing events per D6. |
| Exit/status | Ctrl-C stops host and child tasks cleanly (exit 0). Natural app exit in dev follows current contract. A failed build keeps the session alive with previous UI; a non-recoverable host/swap failure exits non-zero with a labelled reason. |
| Diagnostics | Build failures, swap failures, ABI mismatches, snapshot resets, and fallbacks are reported with actionable labels distinguishing “hot reloaded”, “previous UI still running”, and “restarted (state restored/reset)”. |

### Configuration (`Auro.toml`, coordinated with spec 0002)

Additions required by this spec (exact key names finalised at implementation; defaults must preserve
current relaunch behaviour until GA):

- A dev setting to enable/select hot reload mode (experimental opt-in).
- Watch/build inputs sufficient to distinguish host-stable and reloadable rebuilds, or an explicit
  documented convention derived from project layout.
- Optional dev settings for snapshot/fallback behaviour if needed (e.g. restart-on-swap-failure
  policy) - only if they change observable behaviour; otherwise internal.

Any new key is added to spec 0002, validation, and `auro doctor` in the same change that implements
it.

### Application / runtime API (`auro`, `auro-core`, `auro-ui`, `auro-ipc`)

- A way to declare reloadable roots/views and their stable snapshot identity.
- A snapshot/restore hook for opted-in view state (trait or callback), invoked only on the main
  thread with host-provided serialization context.
- A host-visible distinction between host-stable managed state and reloadable-library-owned state, so
  unload safety (D3, D5) can be enforced or diagnosed.
- Teardown/cancellation integration so reloadable tasks and subscriptions are dropped before unload.

Illustrative (non-normative):

```rust,ignore
pub trait ReloadSnapshot: serde::Serialize + serde::de::DeserializeOwned {
    const SNAPSHOT_KEY: &'static str;
    const SNAPSHOT_VERSION: u32;
}
```

### Dev host / reloadable library contract

- Versioned ABI handshake (`abi_version`, build id/hash, profile/feature flags as needed).
- Mount/unmount/snapshot/restore entrypoints with explicit status/error reporting (no panics across
  FFI boundary; panics trapped and reported as swap failure).
- Host API passed as opaque, versioned function table; no direct sharing of allocator-owned Rust
  collections or entities across the boundary.

### Generated project layout

`auro new` must continue to scaffold a project that builds and runs under the default (relaunch) dev
loop. When hot reload mode is enabled for a project, the project must satisfy the documented host/UI
layout and export contract; `auro doctor` reports whether the current project is hot-reload capable
and why not. Migration/upgrade steps for existing Phase 0 scaffolds are documented in the
implementing change.

## Acceptance

Checks are runnable by someone who did not write the implementation. Items that require a real
display or platform-specific behaviour are marked; headless/CI-safe subsets are preferred where
possible.

1. **Build/swap correctness (headless where feasible):** a host test harness loads a reloadable UI
   library fixture, mounts it, applies a state mutation, swaps to a newly built fixture with changed
   view output, and asserts opted-in state is restored and the new output is observable. A fixture
   whose snapshot version/key is incompatible resets only the affected entry and reports a visible
   reset diagnostic.
2. **Build failure keeps UI live:** with a mounted UI, an intentionally failing rebuild leaves the
   previous UI mounted and interactive; diagnostics are surfaced; no unload occurs.
3. **Swap failure safety:** an ABI/handshake mismatch or load failure does not crash the host; the
   outcome follows D4 and is labelled (previous UI retained or explicit state-preserving restart).
4. **No unloaded-code execution:** teardown cancels/drops reloadable tasks before unload (verified by
   a fixture that would otherwise touch library state after unmount; test fails if invoked).
5. **Fallback honesty:** a forced fallback restart restores snapshot state when possible and is
   reported as “restarted (state restored/reset)”, never as “hot reloaded”.
6. **Measurability:** in hot-reload mode the dev session emits timing/trace data sufficient to
   compute save-to-visible per D6; at least one measured run is recorded in this spec (or linked
   artefact) with machine, method, N, median, and outcome class before experimental mode is described
   as meeting any latency target.
7. **CLI/dev integration (manual/platform where needed):** `auro dev` on a hot-reload-capable demo
   project performs a view-only hot swap without closing the window; Ctrl-C and natural window-close
   semantics remain correct (spec 0001 acceptance 3/4). Platform limitations are recorded, not hidden.
8. **Documentation and gates:** `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass. Spec 0001 open question, roadmap,
   performance budget, changelog, and architecture notes are updated to reference this spec and the
   measured status.

GA additionally requires: measured < 500 ms save-to-visible for the documented warm view-only case on
each Tier 1 platform (or an explicit, approved revision of the target recorded in the performance
budget), published numbers, and a stable documented author contract for reloadable state.

## Open questions

| Question | Where it settles |
|---|---|
| Exact scaffold/layout for host + reloadable UI (`ui` crate, dual-target, generated host wrapper) and migration from Phase 0 layout | Implementation of this spec; recorded here when chosen |
| Canonical snapshot encoding (JSON, CBOR, postcard/bincode-class) and compatibility policy | Implementation; must remain `serde`-compatible at the API boundary |
| How managed `State<T>` participates across swaps when its type is defined in reloadable code vs host-stable code | Implementation + follow-up ADR/spec if it changes state ownership rules (D5 of spec 0001) |
| Debug/release mixing, optimization levels, and symbol visibility required for reliable `dlopen`/backtraces | Implementation, per platform notes |
| Function-level patching feasibility and integration point (`subsecond`-style) | Research; optimisation only, not required for acceptance 1-8 |
| In-app surfacing of build/swap diagnostics (overlay/panel) vs terminal-only in experimental mode | `auro-ui`/dev UX follow-up; terminal reporting is the minimum |
| Windows and macOS `dlopen`/unload semantics and code-signing implications for dev libraries | Platform verification during implementation |

