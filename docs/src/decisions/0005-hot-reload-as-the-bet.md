# 0005 - Hot reload with state preservation

**Status:** Accepted (target under research; fallback defined)

## Context

Rust compile times are the only real reason a developer stays on the web. Tauri's edit loop is
fast because the frontend is a script; Auro's frontend is Rust, so the loop is the product.

A 20-second `cargo build` between edits would make the rest of the framework irrelevant for anyone
doing UI work.

## Decision

The UI compiles as a `cdylib`, the host loads it with `libloading`, and a successful rebuild
hot-swaps the renderer while app state survives as a `serde` snapshot. The target is
**≤ 500 ms from save to visible change**. Function-level patching (`subsecond`-style) is an
optimisation on top of the swap. If the target is missed, the documented fallback is a
state-preserving process restart, labelled as such.

## Consequences

**Easier**

- UI iteration stops being a compile-time problem.
- State preservation across reload makes the loop usable for real work, not just a blank window.

**Harder**

- Every view must be reconstructible from serialisable state; ad hoc render-time state is lost.
- The host and the UI library share an ABI, which must be pinned and tested across toolchains.
- Debugging crosses a `libloading` boundary: symbols, breakpoints and backtraces need work.
- Linux `dlclose` semantics with live threads are a known hazard.

**Forbidden**

- Presenting a process restart as hot reload.
- A reload path that leaves tasks running against the unloaded library.
- Storing entity handles in process-global `static`s.
