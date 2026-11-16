# 0004 - Hot reload

**Status:** Draft - revised after Slice 0 feasibility spike and Option 1 consolidation (prototype evidence); implementation not started

Spec 0001 closed Phase 0 with a relaunch loop and recorded ~60 s save-to-visible on the development
machine (relink dominates). This spec defines the contract that must replace that loop for UI work:
a long-lived dev host that applies reloadable UI changes while preserving restorable state, how
save-to-visible is measured, and the fallback when a reload cannot be performed safely.

It realises [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md)
([ADR 0005](../decisions/0005-hot-reload-as-the-bet.md), superseded, set the goal and assumed a
`cdylib`/`libloading` swap mechanism) and the
[hot reload architecture](../architecture/hot-reload.md). Where this spec learns from implementation
or research it must be corrected in the same pull request.

## Slice 0 viability spike (2026-11-16)

A time-boxed spike prototyped the previously drafted mechanism (host-owned GPUI application with a
separately built/loaded/unloaded reloadable UI `cdylib` exporting a C-ABI mount/snapshot/restore
contract) before committing implementation effort.

**Environment:** Linux 7.3.4-1-cachyos x86_64, Wayland/niri (`DISPLAY=:0`, `WAYLAND_DISPLAY=wayland-1`),
12 CPUs, 22 GiB RAM, rustc 1.99.1 (stable-x86_64-unknown-linux-gnu), cargo 1.99.1, debug profile,
warm incremental builds, `gpui = "=0.2.2"` (registry, rlib-only upstream crate), `libloading = "0.8.9"`.

**Observed:**

| Area | Observation |
|---|---|
| Baseline (relaunch) | ~60 s save-to-visible, single visible change, relink dominates (spec 0001, 2026-11-15). Not hot reload. |
| `dlopen`/`dlclose` mechanics (C-ABI symbols only, no live GPUI render/app task graph) | Load/unload, 3-5 cycle load/unload, and A/B copy swap simulations succeeded for a no-dependency `cdylib` and for a 429 MB debug `cdylib` statically linking GPUI (ABI symbol calls returned; `dlclose` returned). Necessary but not sufficient for safe unload under a live GUI session. |
| GPUI 0.2.2 mount boundary | Swapping/mounting GPUI view code from a separately loaded library into a host-owned GPUI application is not feasible under standard Rust/cargo builds. `Window::replace_root` is generic over concrete `E: Render`; `AnyView` stores a monomorphized `render::<V>` that downcasts to `Entity<V>` and mutates host `App` entity storage; `AnyElement` is arena-backed in a thread-local element arena; entities/refcounts use `TypeId` and host `App` maps. Upstream GPUI is rlib-only, so host and library carry separate GPUI instances/statics/type identities. Passing Rust/GPUI view/entity/element types across a C ABI is unsound/incompatible with normal view authoring. |
| Warm rebuild cost (trivial one-line string/view change, debug, after warm cache) | Monolithic GPUI bin: 1.81 s (1m30s initial). Trivial GPUI-dependent `cdylib` export (4.3 MB): 0.60 s (1m26s initial). No-dependency `cdylib`: 0.17 s. Realistic GPUI-view `cdylib` (`div()`/`AnyElement`, 429 MB debug): **2m50s** cargo / ~171 s wall (full relink; package-cache/build-dir lock contention noted, link-dominated). |
| Unload safety | A GPUI-linked `cdylib` had TLS and init/fini arrays; live-window/task/thread/platform-resource behavior was not exercised. Safe quiesce/unload under a running GPUI app remains unproven even if symbol-only `dlclose` returns. |

**Conclusion:** the drafted `cdylib`/`libloading` swap of GPUI view/render code is **rejected** for the
current stack (GPUI 0.2.2, standard linkage). The observable hot-reload contract remains; the
mechanism must be revised (see [D1](#d1---long-lived-dev-host-same-process-reload-mechanism-tbd)).


## Option 1 consolidation spike (2026-11-16)

Follow-up throwaway prototypes (not integrated into `auro` crates/CLI) evaluated the leading
same-process candidate on Linux 7.3.4-1-cachyos x86_64, Wayland/niri (`:0`/`wayland-1`), 12 CPUs,
22 GiB RAM, rustc 1.99.1/cargo 1.99.1 (stable), `dx 0.7.10`, `subsecond 0.7.10`,
`dioxus-devtools 0.7.10`, `gpui = "=0.2.2"` (registry, rlib-only), debug profile `debug = 0`, warm
`dx serve --hot-patch`, xattr patch as in workspace. Artifacts: `/tmp/auro-option1-spike`
(`subsecond-plain`, `gpui-subsecond-spike`, scenario logs/JSON).

**Mechanism exercised:** tip-crate function-level hot-patching (jump-table/ThinLink) with explicit
stable `subsecond::HotFn::current(...)` boundaries; `dioxus_devtools::connect_subsecond()`;
host-owned GPUI `Application`/window/event loop and entity/managed state in the same process/type
universe (root entity `counter` ticker + stable host string).

| Case | Observation |
|---|---|
| Plain Rust (non-GPUI) `HotFn` patch | `rev=0`→`rev=1` in tip-crate function: `Hot-patching ... took 125ms`, output changed while loop counter continued (state/process preserved). |
| s1 warm latency/state, minimal GPUI app (N=10, sequential trivial label edits) | 10/10 visible, host `counter`/`stable`/pid preserved. Save→first-new-render: 438,409,371,422,372,406,339,343,350,300 ms (median 371.5, min 300, max 438). `dx` hot-patch: 351,328,326,322,322,304,289,297,293,284 ms. |
| s2 view/helper/layout/style (4 edits) | Helper-only change (label unchanged) 304 ms; label+added child 292 ms; style/bg 297 ms; helper+label 383 ms; helper changes visible; host state preserved. |
| s3 outside boundary / init not retroactive | Patches visible (314,321 ms), state preserved. `Render` impl change outside `HotFn` boundary not reflected in rendered stable value; init-created host state literal not retroactive. |
| s4 tip-crate statics/reset | Before: `n=23 tip=1022 counter=19`; first patched render: `n=1 tip=1000 counter=24`, `stable` preserved. Tip-crate counters/statics reset; host entity state preserved. |
| s5 build/syntax failure + recovery | Broken edit kept app alive on previous `v0` with progressing counter (no crash/apply of broken code); fix patched (`314 ms`) to `v1-fixed` with `n=1 tip=1000 counter=1247`, state preserved. |
| s6 longevity (N=20) | 20/20 visible, all state preserved, final `v20`, same pid. Save→first-new-render median 346 ms (min 271, max 370). |
| s7 init-captured async work | Ticker-interval source edit (100→400 ms) patched but running ticker rate unchanged (~10.00→10.00 cps): captured future not retroactive. |
| s8 coordinated transient struct layout change | Added `Vm` field with coordinated construct/log/UI update in same edit: visible (362 ms), `extra` logged, counter progressed, no error. |
| s9 multi-module tip crate | Baseline `ex=m0`. Module-only edit visible as `ex=m1` (269 ms) with host state preserved; module+main visible as `v1 ex=m2`; `dx` logged hot-patches for `src/extra.rs` and `src/main.rs` (functionally applied). |
| s10 no dx/devserver | `cargo build` + run without DIOXUS/devserver: alive, 59 renders, counter 0→56 progressed, no panic (SIGINT exit) - instrumented app runs without hotpatch tooling. |
| s11 `thread_local!` in patched path | `Failed to generate patch` (`rust-lld` `R_X86_64_TPOFF32 ... cannot be used with -shared`), no patch applied, app stayed alive on `v0` with progressing counter (honest failure). |

**Conclusion:** subsecond/dx tip-crate `HotFn` patching is a **viable experimental same-process
candidate** on Linux x86_64/GPUI 0.2.2 in minimal prototype conditions (sub-500 ms warm view edits
and preserved host state demonstrated), but **not** integrated into `auro dev` and **not** a D6 claim
for representative Auro scaffolds/apps or other platforms/profiles. Required follow-up: Auro project
layout/tip-crate/build strategy, host-owned versioned snapshot/restore and incompatible-change
classification/reset, `auro dev` opt-in integration (dx/devserver or equivalent driver, target-dir
isolation, timing/trace and diagnostics), and representative D6 measurements.


## Scope

- The `auro dev` hot-reload flow: watch, build, apply reload or fallback, and user-visible reporting.
- The host/reloadable-UI boundary for development: process ownership, reload unit, and compatibility
  contract.
- Snapshot, restore, and invalidation of state across a reload or state-preserving restart.
- Reload eligibility: which source changes may reload in-process and which must restart.
- Failure handling: build errors, incompatibility/apply errors, restore errors, panics, and
  cancellation/rollback.
- Measurability: the definition and recording of save-to-visible latency.
- Staged delivery from experimental hot reload (Phase 1) to GA with state preservation (Phase 3).

## Out of scope

- Shipping hot updates to released binaries (`auro-update`, Phase 2). This spec covers the dev
  loop only.
- Hot reload of plugins or out-of-process plugin hosts (Phase 3), except stating that plugin
  reloadability is a separate contract.
- Requiring a specific patching/swap technology. The required contract is observable
  reload/restore behaviour and measured latency, not a named technique, until a mechanism is proven
  and recorded here.
- Mobile targets.
- Preserving state that is inherently non-serialisable or external (open sockets, OS handles,
  unmanaged threads, GPU resources owned outside the snapshot contract) except making loss visible.
- A stable cross-compiler or cross-version reload ABI for third parties in 0.x.

## Baseline (measured)

| Path | Observed | Notes |
|---|---|---|
| Phase 0 relaunch (`auro dev`) | ~60 s save-to-visible | Development machine, single visible change, relink dominates; watcher reacts once per edit (spec 0001 status, 2026-11-15). Not a hot reload. |
| Slice 0 spike: realistic GPUI-view `cdylib` warm rebuild | 2m50s cargo / ~171 s wall | Debug, trivial view string change, 429 MB `cdylib`, GPUI 0.2.2, Linux x86_64; lock contention noted, relink-dominated. Rejects cdylib-view-swap as a latency path. |

Targets below are targets until measured by the method in [D6](#d6---measurability).

## Decisions

### D1 - Long-lived dev host, same-process reload (mechanism TBD)

`auro dev` in hot-reload mode runs a **host process** that owns the GPUI `Application`, windows,
event loop, Auro runtime, managed host state, and the snapshot store for the lifetime of the dev
session. UI code changes that are classified as reloadable must become visible without closing the
window and without losing opted-in state, by applying updated code/state in the same process and
same GPUI/App/type universe.

The concrete reload mechanism is **not yet selected for product integration**. It must satisfy the
observable contract in this spec (D3, D4, D6, D7) and be proven by prototype/measurement and
integrated safely before being declared supported.

**Rejected by Slice 0:** compiling UI as a separately loaded/unloaded `cdylib` and swapping it via
`libloading` for GPUI view/render code. This is infeasible with GPUI 0.2.2 under standard builds due
to generic/type-bound root mounting, `AnyView`/entity/`TypeId`/arena dependence on a single GPUI
instance and host `App`, upstream GPUI being rlib-only, and prohibitive relink cost for a realistic
view library. It must not be reintroduced for GPUI view code without new evidence resolving mount,
type/instance identity, unload safety, and measured performance.

**Evidenced candidate (Option 1 consolidation, 2026-11-16, prototype, not integrated):**
same-process function-level hot-patching of the tip crate via `subsecond 0.7.10` +
`dioxus-devtools 0.7.10` driven by `dx 0.7.10` (jump-table/ThinLink), with explicit stable
`HotFn::current(...)` boundaries in reloadable view code and host-owned GPUI
application/window/entities/managed state. On Linux x86_64/Wayland (`gpui = "=0.2.2"`, rustc/cargo
1.99.1, debug `debug = 0`, warm minimal app), observed save-to-first-new-render was ~271-450 ms
with host state preserved (s1: median 371.5 ms, N=10; s6: median 346 ms, N=20, 20/20 stable), and
core contract properties were demonstrated in prototype scenarios (patch visibility, host-state
preservation, build/syntax-error recovery keeping previous UI, patch-generation failure keeping app
alive on previous code). Documented constraints: patchable code limited to tip-crate hot
boundaries (outside-boundary/init-captured work not retroactive), tip-crate statics/reset on
patch, `thread_local!` in patched path can fail patch generation, and tooling/layout/snapshot/
classification/integration and representative Auro-scaffold D6 measurement remain pending.

*Rejected:* recompiling and relinking the whole app process on every save as the hot path. *Rejected:*
claiming a process restart is hot reload (forbidden by ADR 0008). *Rejected:* discarding the window
on every successful reload.

### D2 - Host-stable and reloadable code have an explicit boundary

Code that must survive a reload lives on the host-stable side: process bootstrap, window lifecycle,
the tokio runtime, host-owned managed state containers, snapshot persistence, and the reload/apply
machinery. Code that defines views and reloadable UI behaviour lives in the reloadable side.

The generated project layout and the `auro` API must make this boundary explicit once a mechanism is
chosen. A single-crate bin scaffold (Phase 0 shape) is sufficient for relaunch mode but is not known
sufficient for in-process hot reload; hot-reload-capable projects may require a specific layout,
build flags, or patchable/reloadable compilation units. The exact shape is fixed by the
implementation/prototype that proves the mechanism and recorded here when chosen.

State, tasks, subscriptions, and callbacks owned by reloadable code must have an explicit teardown or
invalidation contract so replaced code is never invoked after a reload completes or is rolled back.

*Rejected:* implicit, arbitrary mutation of live memory without a snapshot/restore and
teardown/rollback contract. *Rejected:* storing entity handles or view callbacks in process-global
state that cannot be invalidated on reload.

### D3 - State survives as a versioned, opt-in snapshot

The host owns the snapshot store. A snapshot is a versioned envelope containing named, typed entries
for state that opts into preservation. At minimum the format records:

- `format_version` (snapshot schema version, integer).
- a build/session identity sufficient to diagnose mismatches (e.g. build id/hash of the reload
  unit).
- per-entry payloads: a stable key, an entry version, and serialized bytes (the canonical encoding
  is `serde`-compatible; the wire encoding is fixed at implementation time and recorded here).

State categories:

| Category | Owner | Reload behaviour |
|---|---|---|
| View/UI state explicitly marked restorable | Reloadable UI, via snapshot contract | Serialized before apply/restart fallback, restored after successful apply/start. On decode/version mismatch, reset that entry and report it. |
| Host-managed state outside the replaced/restarted UI unit | Host | Preserved in memory across in-process reload when host-stable; preserved across state-preserving restart only through snapshot/persisted host state. Never passed as a borrowed reference that outlives its owner. |
| Ephemeral/UI-local state not opted in | Reloadable UI | Reset on reload/restart. Reset must be visible in dev diagnostics, not silent. |
| External/non-serialisable resources | OS/runtime | Recreated or dropped according to the resource contract; loss reported. |

A view or state unit opts into preservation by implementing reload snapshot/restore hooks for its
stable key. “Stable key” means stable across saves for the same logical UI element within a dev
session (window label + root/view identity as defined by the API). Renaming/removing a key is a
reset of that entry.

*Rejected:* snapshotting arbitrary memory or entity graphs implicitly. *Rejected:* a silent fallback
to default state on mismatch.

### D4 - Successful reload, failed build, and fallback are distinct outcomes

| Outcome | Condition | Host behaviour | User-visible label |
|---|---|---|---|
| Hot-reloaded | Reloadable build/patch succeeds, compatibility checks pass, apply and restore complete (with or without per-entry resets) | Keep window and host state; present updated UI; repaint | “reloaded” with duration and any reset warnings |
| Build failed | Build/patch compilation fails | Keep previous UI active and interactive; do not apply a partial reload | compiler/build diagnostics; “previous UI still running” |
| Reload failed, recovered | Build/patch succeeds but compatibility/apply/restore fails and previous UI remains valid or can be safely rolled back | Keep or restore previous UI as applicable; do not present as reloaded | explicit failure reason |
| State-preserving restart (fallback) | Reload cannot be completed safely, or a configured/required restart change occurred | Persist snapshot to a host-controlled location, restart app/host session, restore on boot | “restarted (state restored)” or “restarted (state reset: \<reason\>)” - never “hot reloaded” |
| Hard failure | Restart/restore impossible | Exit non-zero with diagnostic | error |

A failed build must never apply broken code or leave the session without a usable previous UI.
Reloadable tasks/callbacks that can outlive a reload must be cancellable or invalidated before
replaced code can run; the host must not invoke replaced code after a failed or completed reload.

*Rejected:* applying partial/broken reloads. *Rejected:* leaving tasks running against replaced code.
*Rejected:* presenting a restart as hot reload.

### D5 - Compatibility is enforced, not assumed

The dev host and reloadable UI are built under a compatible toolchain/profile/target/dependency set
for the selected reload mechanism. Before apply, the host checks a build identity and any
mechanism-specific compatibility data required to avoid undefined behaviour; a mismatch is a reload
failure (D4), not a best-effort apply.

If the chosen mechanism involves dynamically loaded artifacts, teardown/quiesce and unload safety are
required; leaked unloads or live references into unloaded code are treated as reload failure with
safe fallback. If the chosen mechanism patches in-process code, patch applicability/rollback safety
must be explicit and diagnosable.

*Rejected:* promising stable cross-version reload compatibility in 0.x. *Rejected:* silent
best-effort apply on mismatch.

### D6 - Measurability

“Save-to-visible” is defined as: **wall-clock time from the editor save event observed by the
watcher to the first frame presenting the updated UI** (or, when pixels cannot be observed
headlessly, to the host’s “reload committed and repaint requested” event - the method must be stated
with the measurement). Each recorded measurement states:

- machine/OS, CPU class, storage class (if relevant), debug/release, warm/cold incremental state;
- what changed (view-only, style/token, command signature, config, dependency) and file count;
- whether the outcome was hot-reloaded, reset entries, or fallback restart;
- median and spread over N >= 10 edits after warm-up (or an explained smaller N).

The host must emit machine-readable timing events in dev mode (log/trace or structured output)
sufficient to compute this metric without manual stopwatch use. `AURO_DEV_TRACE` may be extended for
this purpose. Numbers published in docs, roadmap, or performance budget are measured or explicitly
marked as targets.

Target (from ADR 0008 / roadmap): **< 500 ms save-to-visible** for a view-only change on a warm
incremental build, with state preserved. This is a target, not a guarantee, until recorded under D6.

*Rejected:* unverifiable latency claims. *Rejected:* counting only compile time while excluding
apply/restore/repaint.

### D7 - Reload eligibility and staged delivery

Not every edit is safely reloadable in-process. The dev loop classifies changes:

| Change class | Expected path | Notes |
|---|---|---|
| Reloadable UI/view code and resources within the supported reload unit | Hot-reload | Primary path; must preserve opted-in state per D3. |
| Host-stable/bootstrap code, host dependency graph, toolchain/config affecting host/runtime compatibility | State-preserving restart or full restart | Classified and reported; never a silent no-op. |
| `Auro.toml` / window-identity changes | Reload config in host where safe; otherwise restart with explanation | Must not corrupt running host assumptions. |
| Compatibility/build identity mismatch after rebuild | Reload failed → fallback per D4/D5 | |

The classification rules are part of the implementation contract and must be documented for app
authors once implemented.

Staged delivery (aligns roadmap):

- **Experimental (Phase 1):** opt-in hot reload in `auro dev` for supported project layouts and
  mechanisms; view-only reloads; snapshot/restore for opted-in view state; visible diagnostics and
  timing; fallback to state-preserving restart where reload is not possible. Correctness and honesty
  over coverage.
- **GA (Phase 3):** default dev experience for supported apps; documented state contract stable enough
  for third-party app authors; measured latency published per platform tier; CI/perf-budget integration
  as the budget becomes enforceable.

*Rejected:* enabling an unsafe/undefined reload path by default. *Rejected:* declaring GA without
measured D6 evidence for the documented case.

## Interfaces

This section fixes the surfaces this spec introduces or constrains. Signatures shown as
`rust,ignore` are illustrative until implemented; normative details are the behavioural contracts
above and the keys/items listed here.

### CLI (`auro-cli`)

| Surface | Behaviour |
|---|---|
| `auro dev` | Default remains the documented relaunch loop until hot reload is declared stable for the selected project; hot reload is enabled by an explicit opt-in (flag and/or `[dev]` config key defined at implementation time and recorded in spec 0002/CLI docs). |
| `auro dev` (hot-reload mode) | Starts/uses the dev host, watches configured paths, builds the reload unit(s), performs reload or fallback per D4, streams diagnostics, and emits timing events per D6. |
| Exit/status | Ctrl-C stops host and child tasks cleanly (exit 0). Natural app exit in dev follows current contract. A failed build keeps the session alive with previous UI; a non-recoverable host/reload failure exits non-zero with a labelled reason. |
| Diagnostics | Build failures, compatibility mismatches, apply/restore failures, snapshot resets, and fallbacks are reported with actionable labels distinguishing “reloaded”, “previous UI still running”, and “restarted (state restored/reset)”. |

### Configuration (`Auro.toml`, coordinated with spec 0002)

Additions required by this spec (exact key names finalised at implementation; defaults must preserve
current relaunch behaviour until GA):

- A dev setting to enable/select hot reload mode (experimental opt-in).
- Watch/build inputs sufficient to distinguish host-stable and reloadable rebuilds, or an explicit
  documented convention derived from project layout/mechanism.
- Optional dev settings for snapshot/fallback behaviour if needed (e.g. restart-on-reload-failure
  policy) - only if they change observable behaviour; otherwise internal.

Any new key is added to spec 0002, validation, and `auro doctor` in the same change that implements
it.

### Application / runtime API (`auro`, `auro-core`, `auro-ui`, `auro-ipc`)

- A way to declare reloadable roots/views and their stable snapshot identity.
- A snapshot/restore hook for opted-in view state (trait or callback), invoked only on the main
  thread with host-provided serialization context.
- A host-visible distinction between host-stable managed state and reloadable-UI-owned state, so
  reload safety (D2, D3, D5) can be enforced or diagnosed.
- Teardown/cancellation/invalidation integration so reloadable tasks and subscriptions cannot run
  against replaced code after reload.

Illustrative (non-normative):

```rust,ignore
pub trait ReloadSnapshot: serde::Serialize + serde::de::DeserializeOwned {
    const SNAPSHOT_KEY: &'static str;
    const SNAPSHOT_VERSION: u32;
}
```

### Reload mechanism contract (to be defined by prototype)

The implementation must define and record a mechanism-specific contract covering, at minimum:

- build identity and compatibility checks performed before apply;
- how updated UI code is applied in the same GPUI/App process;
- snapshot capture/restore ordering and rollback on failure;
- teardown/quiesce of reloadable tasks, callbacks, and subscriptions;
- diagnostics for incompatibility, apply failure, and reset state.

A separately loaded/unloaded `cdylib` C-ABI contract for GPUI view code is not part of this spec
after the Slice 0 rejection unless new evidence overturns that conclusion.

### Generated project layout

`auro new` must continue to scaffold a project that builds and runs under the default (relaunch) dev
loop. When hot reload mode is enabled for a project, the project must satisfy the documented layout
and build/mechanism requirements; `auro doctor` reports whether the current project is hot-reload
capable and why not. Migration/upgrade steps for existing Phase 0 scaffolds are documented in the
implementing change.

## Acceptance

Checks are runnable by someone who did not write the implementation. Items that require a real
display or platform-specific behaviour are marked; headless/CI-safe subsets are preferred where
possible. Acceptance 1-5 define the contract a proven mechanism must satisfy; they are not met by the
current relaunch loop. Prototype evidence (Option 1 consolidation, 2026-11-16, not integrated)
demonstrated analogous contract properties in minimal GPUI 0.2.2 scenarios (host-state
preservation across patches, build/syntax failure keeping previous UI alive, patch-generation
failure keeping app on previous code, and warm view-only save-to-first-new-render ~271-450 ms,
s1/s6) but does not satisfy these acceptance items for `auro dev`/Auro integration.

1. **Reload correctness (headless where feasible):** a host test harness mounts/runs a reloadable UI
   fixture, applies a state mutation, applies a newly built fixture with changed view output, and
   asserts opted-in state is restored and the new output is observable. A fixture whose snapshot
   version/key is incompatible resets only the affected entry and reports a visible reset diagnostic.
2. **Build failure keeps UI live:** with a running UI, an intentionally failing rebuild leaves the
   previous UI active and interactive; diagnostics are surfaced; no broken reload is applied.
3. **Reload failure safety:** a compatibility/apply/restore failure does not crash the host; the
   outcome follows D4 and is labelled (previous UI retained/rolled back or explicit state-preserving
   restart).
4. **No replaced-code execution:** teardown/invalidation prevents reloadable tasks/callbacks from
   running against replaced code after apply/rollback (verified by a fixture that would otherwise
   touch stale state; test fails if invoked).
5. **Fallback honesty:** a forced fallback restart restores snapshot state when possible and is
   reported as “restarted (state restored/reset)”, never as “hot reloaded”.
6. **Measurability:** in hot-reload mode the dev session emits timing/trace data sufficient to
   compute save-to-visible per D6; at least one measured run is recorded in this spec (or linked
   artefact) with machine, method, N, median, and outcome class before experimental mode is described
   as meeting any latency target.
7. **CLI/dev integration (manual/platform where needed):** `auro dev` on a hot-reload-capable demo
   project performs a view-only reload without closing the window; Ctrl-C and natural window-close
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
| Reload mechanism: in-process/function-level patching feasibility (e.g. subsecond-style) on Linux x86_64 with GPUI 0.2.2, and alternatives if not viable | **Partially evidenced (prototype, 2026-11-16):** subsecond/dx tip-crate `HotFn` patching viable in minimal GPUI 0.2.2 cases with constraints; productization, representative D6, and other platforms remain open. Mechanism recorded here; ADR revision/addition only if decision changes. |
| Exact scaffold/layout/build flags for hot-reload-capable projects and migration from Phase 0 layout | Implementation of this spec; recorded here when chosen |
| Canonical snapshot encoding (JSON, CBOR, postcard/bincode-class) and compatibility policy | Implementation; must remain `serde`-compatible at the API boundary |
| How managed `State<T>` participates when its type is defined in reloadable code vs host-stable code | Implementation + follow-up ADR/spec if it changes state ownership rules (D5 of spec 0001) |
| Debug/release mixing, optimization levels, and symbol/debug info required by the chosen mechanism | Implementation, per platform notes |
| In-app surfacing of build/reload diagnostics (overlay/panel) vs terminal-only in experimental mode | `auro-ui`/dev UX follow-up; terminal reporting is the minimum |
| Windows and macOS feasibility, code-signing implications, and debugger behavior for the chosen mechanism | Platform verification during implementation |
