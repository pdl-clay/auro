# 0008 - Hot reload mechanism revised after feasibility spike

**Status:** Accepted (supersedes [0005](0005-hot-reload-as-the-bet.md))

## Context

[ADR 0005](0005-hot-reload-as-the-bet.md) fixed hot reload with state preservation as the project
bet and assumed a specific mechanism: compile UI views as a `cdylib`, load/swap it with
`libloading`, and hand a `serde` snapshot to the new library, with a state-preserving process
restart as the documented fallback.

Slice 0 of [spec 0004](../specs/0004-hot-reload.md) ran a viability spike on 2026-11-16 (Linux
7.3.4 x86_64, Wayland/niri, 12 CPUs, 22 GiB RAM, rustc 1.99.1 stable, cargo 1.99.1, debug profile,
warm incremental builds, `gpui = "=0.2.2"`, `libloading = "0.8.9"`) to validate that mechanism
before committing implementation effort.

Observed evidence:

- **Mechanics:** `dlopen`/`dlsym`/`dlclose` of versioned C-ABI `cdylib` artifacts worked for symbol
  calls, including a 429 MB debug `cdylib` that statically linked GPUI (load/unload cycles and A/B
  copy swaps returned; `dlclose` returning is necessary but not sufficient proof of safe unload
  under a live GUI/session with threads, TLS, and platform resources).
- **Boundary feasibility (GPUI 0.2.2):** mounting UI view/render code from a separately loaded
  library into a host-owned GPUI application is not feasible under standard Rust/cargo builds.
  `Window::replace_root` is generic over the concrete `Render` root type; `AnyView` stores an
  entity plus a monomorphized `render::<V>` path that downcasts to `Entity<V>` and updates the view
  in the host `App` entity map; `AnyElement` is arena-backed in a thread-local element arena; and
  entities/refcounts depend on `TypeId` and host `App` storage. Upstream GPUI 0.2.2 ships as an
  rlib only, so host and reloadable library would carry separate GPUI instances, statics, type
  identities, and runtime stores. Crossing Rust/GPUI types over a C-ABI boundary is unsound and
  incompatible with normal GPUI view construction (`impl Render`, `div()`/elements/entities).
- **Build cost:** warm incremental rebuild of a realistic GPUI-view `cdylib` (trivial string change
  in a view) took **2m50s** cargo / ~171 s wall in the spike run (full relink of a 429 MB debug
  `cdylib`, with package-cache/build-dir lock contention noted). Toy comparisons rebuilt in 1.81 s
  (monolithic GPUI bin), 0.60 s (trivial GPUI-dependent `cdylib` export), and 0.17 s (no-dep
  `cdylib`), showing that artifact split alone is not sufficient and a realistic view library relink
  is prohibitive for the < 500 ms budget.
- **Phase 0 baseline:** relaunch loop save-to-visible remains ~60 s (relink-dominated), not hot
  reload.

## Decision

Auro retains the hot-reload goal and contract from ADR 0005, but rejects the assumed
`cdylib`/`libloading` view-swap mechanism for GPUI view/render code on the current stack.

- **Retained:** target < 500 ms save-to-visible for a warm view-only change with preserved opted-in
  state (a target until measured per spec 0004 D6); a long-lived dev host that owns the GPUI
  application/window/event loop, runtime, managed host state, and snapshot store; versioned,
  opt-in `serde`-compatible snapshot/restore with visible resets; failed builds keep the previous
  UI running; failed or unsafe reloads fall back honestly and are never labelled "hot reloaded".
- **Rejected:** swapping a separately loaded/unloaded `cdylib` that constructs or mounts GPUI views
  into a host-owned GPUI application as the hot-reload mechanism, given GPUI 0.2.2 type/instance
  boundaries and observed rebuild cost for a realistic view artifact.
- **Direction:** pursue a same-process reload mechanism that keeps one GPUI/App instance and
  reloaded UI code in the same process and type universe (function-level/in-process hotpatching is
  the leading candidate to prototype, subject to OS/architecture/toolchain feasibility), or another
  mechanism proven to satisfy the observable contract. The selected mechanism, its layout/build
  model, and measured performance are recorded in spec 0004 and follow-up ADRs/RFCs as they are
  proven.
- **Until proven:** `auro dev` remains a relaunch loop by default. State-preserving process restart
  is the documented fallback when a reload cannot be performed safely; it is never presented as hot
  reload.

## Consequences

**Easier**

- Implementation effort is not spent on a mechanism shown infeasible for the current renderer/API.
- The observable contract (latency target, state preservation, honest diagnostics/fallback) stays
  stable while the mechanism is researched.

**Harder**

- The path to < 500 ms is less certain: in-process/function-level patching is research-heavy and
  platform/toolchain constrained.
- Achieving fast reload likely requires a build/layout strategy that minimises relink/patch scope
  for view changes, not just code splitting.
- Specs and tooling must distinguish "reloadable change", "state-preserving restart", and "hard
  failure" without assuming library unload/load.

**Forbidden**

- Presenting a process restart as hot reload.
- Claiming the < 500 ms target is met without measurement recorded per spec 0004 D6.
- A reload path that leaves tasks, callbacks, or state running against replaced/unloaded code.
- Reintroducing `cdylib` swap for GPUI view code without new evidence that resolves the
  type/instance, mount, unload, and performance constraints recorded in spec 0004.

## References

- [ADR 0005 - Hot reload with state preservation](0005-hot-reload-as-the-bet.md) (superseded)
- [Spec 0004 - Hot reload](../specs/0004-hot-reload.md)
- [Hot reload architecture](../architecture/hot-reload.md)
