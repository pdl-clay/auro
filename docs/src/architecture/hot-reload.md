# Hot reload

This is the existential bet of the project. If the edit loop stays at a multi-second/minute
rebuild-and-relaunch, nobody leaves the web, and the rest of Auro does not matter.

> Target: edit to see in **under 500 ms**, with app state preserved.

The target and observable contract are fixed; the mechanism is under revision with prototype
evidence. Normative dev-loop contract, staging, and acceptance are drafted in
[spec 0004](../specs/0004-hot-reload.md).

## Contract

- **Default today:** `auro dev` rebuilds and relaunches the app process (Phase 0). A saved change to
  a visible relaunch measured ~60 s on the development machine (relink dominates, 2026-11-15). This
  is not hot reload.
- **Hot reload goal:** a warm, view-only change becomes visible in under 500 ms with opted-in state
  preserved, without closing the window.
- **Honesty rule:** a process restart is never presented as hot reload. A failed build keeps the
  previous UI active. A reload or patch that cannot be applied safely falls back to a
  state-preserving restart only when state can be restored, and the outcome is labelled explicitly.

## Slice 0 finding (2026-11-16)

[ADR 0005](../decisions/0005-hot-reload-as-the-bet.md) (superseded) assumed a `cdylib` compiled UI
swapped by the host via `libloading`. A Slice 0 viability spike
([spec 0004](../specs/0004-hot-reload.md), Linux x86_64, Wayland/niri, rustc 1.99.1, debug,
`gpui = "=0.2.2"`, `libloading = "0.8.9"`) found that mechanism **infeasible** for GPUI view/render
code on the current stack:

- `dlopen`/`dlclose` mechanics work for isolated C-ABI symbol calls, but mounting GPUI views from a
  separately loaded library into a host-owned GPUI application is incompatible with GPUI 0.2.2’s
  type/instance model (`Window::replace_root<E: Render>` generic root, `AnyView`/`Entity<V>`
  downcast and host `App` entity storage, thread-local element arena, `TypeId` identity, upstream
  GPUI rlib-only).
- A realistic GPUI-view `cdylib` (trivial view change, 429 MB debug artifact) warm-relinked in
  ~171 s wall / 2m50s cargo in the spike run, far outside the < 500 ms budget.

`cdylib`/`libloading` swap for GPUI view code is rejected unless new evidence resolves mount,
type/instance identity, unload safety, and measured performance.

## Option 1 consolidation (2026-11-16, prototype evidence)

Follow-up throwaway prototypes (Linux x86_64, Wayland/niri, rustc 1.99.1/cargo 1.99.1, debug
`debug = 0`, warm `dx serve --hot-patch`, `gpui = "=0.2.2"`, `subsecond 0.7.10`,
`dioxus-devtools 0.7.10`, `dx 0.7.10`, not integrated into `auro` crates/CLI) evaluated same-process
function-level hot-patching of the **tip crate**:

- **Mechanism exercised:** explicit stable `subsecond::HotFn::current(...)` boundaries in reloadable
  view code, `dioxus_devtools::connect_subsecond()`/devserver patch application, host-owned GPUI
  `Application`/window/event loop and entity state (ticker-updated counter, stable host string) kept
  in the same process/type universe.
- **Minimal-case results:** plain Rust `HotFn` patch observed at ~125 ms (`rev=0`→`rev=1`, counter
  continued). GPUI 0.2.2 app warm view edits measured ~271-450 ms save-to-first-new-render with
  host state preserved (s1: median 371.5 ms, N=10, values 438/409/371/422/372/406/339/343/350/300;
  s6: median 346 ms, N=20, 20/20 stable). Helper/layout/style changes, coordinated transient struct
  layout changes, multi-module tip-crate edits (functionally applied), build/syntax-error recovery,
  and non-dx instrumented run were demonstrated.
- **Constraints observed:** patchable code is tip-crate code reached via hot boundaries (changes
  outside boundary or init-captured async work not retroactive, e.g. `Render` impl outside `HotFn`
  and running ticker future unchanged). Tip-crate statics/render-local counters reset on patch
  (host entity state preserved). `thread_local!` used in a patched path can fail patch generation
  (`R_X86_64_TPOFF32`/`-shared`) while the app stays alive on previous code. `dx`/target-dir
  isolation and build artifact contention require care.
- **Status:** evidenced experimental candidate for the in-process direction on Linux x86_64/GPUI
  0.2.2 in minimal conditions; **not** a D6 claim for representative Auro scaffolds/apps, not
  integrated into `auro dev`, and not proven across platforms/profiles.
## Contract

## Direction under investigation
- **Default today:** `auro dev` rebuilds and relaunches the app process (Phase 0). A saved change to
  a visible relaunch measured ~60 s on the development machine (relink dominates, 2026-11-15). This
  is not hot reload.
- **Hot reload goal:** a warm, view-only change becomes visible in under 500 ms with opted-in state
  preserved, without closing the window.
- **Honesty rule:** a process restart is never presented as hot reload. A failed build keeps the
  previous UI active. A reload or patch that cannot be applied safely falls back to a
  state-preserving restart only when state can be restored, and the outcome is labelled explicitly.

Per [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md), the mechanism must keep one
GPUI `Application`/window/event loop and reload updated UI code in the same process and type
universe. Current leading evidenced candidate: subsecond/dx function-level tip-crate patching with
explicit `HotFn` boundaries and host-owned snapshot/state, subject to productization (project layout,
tooling integration, snapshot/restore API, change classification, diagnostics, and representative D6
measurement). The chosen mechanism, layout/build model, and measurements are recorded in
[spec 0004](../specs/0004-hot-reload.md) when proven.
## Slice 0 finding (2026-11-16)

## State preservation
[ADR 0005](../decisions/0005-hot-reload-as-the-bet.md) (superseded) assumed a `cdylib` compiled UI
swapped by the host via `libloading`. A Slice 0 viability spike
([spec 0004](../specs/0004-hot-reload.md), Linux x86_64, Wayland/niri, rustc 1.99.1, debug,
`gpui = "=0.2.2"`, `libloading = "0.8.9"`) found that mechanism **infeasible** for GPUI view/render
code on the current stack:

Regardless of mechanism, reloadable state that opts in is captured as a versioned,
`serde`-compatible snapshot owned by the host before apply or before a state-preserving restart, and
restored after success. State that cannot be restored (incompatible version/key, non-serialisable
resource, or explicit reset) must be reported, never silently replaced. Tip-crate statics and other
patch-unit-local state must not be treated as preserved across an in-process patch.
- `dlopen`/`dlclose` mechanics work for isolated C-ABI symbol calls, but mounting GPUI views from a
  separately loaded library into a host-owned GPUI application is incompatible with GPUI 0.2.2’s
  type/instance model (`Window::replace_root<E: Render>` generic root, `AnyView`/`Entity<V>`
  downcast and host `App` entity storage, thread-local element arena, `TypeId` identity, upstream
  GPUI rlib-only).
- A realistic GPUI-view `cdylib` (trivial view change, 429 MB debug artifact) warm-relinked in
  ~171 s wall / 2m50s cargo in the spike run, far outside the < 500 ms budget.

## Fallback

When a change is not safely reloadable in-process (host/bootstrap/toolchain/config changes, failed
compatibility/apply/patch generation, or mechanism limitations), the fallback is a
**state-preserving process restart**: persist snapshot, restart, restore. Slower, but it preserves
context and is labelled honestly.
## Option 1 consolidation (2026-11-16, prototype evidence)

Follow-up throwaway prototypes (Linux x86_64, Wayland/niri, rustc 1.99.1/cargo 1.99.1, debug
`debug = 0`, warm `dx serve --hot-patch`, `gpui = "=0.2.2"`, `subsecond 0.7.10`,
`dioxus-devtools 0.7.10`, `dx 0.7.10`, not integrated into `auro` crates/CLI) evaluated same-process
function-level hot-patching of the **tip crate**:

- **Mechanism exercised:** explicit stable `subsecond::HotFn::current(...)` boundaries in reloadable
  view code, `dioxus_devtools::connect_subsecond()`/devserver patch application, host-owned GPUI
  `Application`/window/event loop and entity state (ticker-updated counter, stable host string) kept
  in the same process/type universe.
- **Minimal-case results:** plain Rust `HotFn` patch observed at ~125 ms (`rev=0`→`rev=1`, counter
  continued). GPUI 0.2.2 app warm view edits measured ~271-450 ms save-to-first-new-render with
  host state preserved (s1: median 371.5 ms, N=10, values 438/409/371/422/372/406/339/343/350/300;
  s6: median 346 ms, N=20, 20/20 stable). Helper/layout/style changes, coordinated transient struct
  layout changes, multi-module tip-crate edits (functionally applied), build/syntax-error recovery,
  and non-dx instrumented run were demonstrated.
- **Constraints observed:** patchable code is tip-crate code reached via hot boundaries (changes
  outside boundary or init-captured async work not retroactive, e.g. `Render` impl outside `HotFn`
  and running ticker future unchanged). Tip-crate statics/render-local counters reset on patch
  (host entity state preserved). `thread_local!` used in a patched path can fail patch generation
  (`R_X86_64_TPOFF32`/`-shared`) while the app stays alive on previous code. `dx`/target-dir
  isolation and build artifact contention require care.
- **Status:** evidenced experimental candidate for the in-process direction on Linux x86_64/GPUI
  0.2.2 in minimal conditions; **not** a D6 claim for representative Auro scaffolds/apps, not
  integrated into `auro dev`, and not proven across platforms/profiles.
- **Windows plain prototype follow-up (2026-11-16, WinBoat guest, not integrated):** the same
  non-GPUI tip-crate `HotFn` mechanism was validated on Windows 11 Pro 10.0.26200.0 x64
  (rustc/cargo 1.99.0 MSVC, `dx`/`subsecond`/`dioxus-devtools` 0.7.10) with
  `dx serve --hot-patch --interactive false --platform desktop`, a clean target, and external logs.
  Build completed in 30.66 s and 3/3 warm `src\main.rs` hot-patches applied in 484/504/499 ms with
  monotonic counter continuity `rev=0 1..361 -> rev=1 421..781 -> rev=2 841..1261 -> rev=3 1321..1741`
  in the same process (`ERR_LEN=0`, `plain-proofwin-20261116152539.*`), demonstrating mechanism
  viability on Windows x86_64 for this minimal plain case only (not a D6 benchmark). GPUI 0.2.2 +
  subsecond/dx sources were prepared on the guest but not built/tested; no `auro dev` integration
  or production/representative-D6 claim. Caveats: corrupted reused target (`libdeps-62094602.a:
  unknown file type`), inconclusive hidden/background runs, and watcher noise from in-project
  verbose log redirection.

## Direction under investigation

Per [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md), the mechanism must keep one
GPUI `Application`/window/event loop and reload updated UI code in the same process and type
universe. Current leading evidenced candidate: subsecond/dx function-level tip-crate patching with
explicit `HotFn` boundaries and host-owned snapshot/state, subject to productization (project layout,
tooling integration, snapshot/restore API, change classification, diagnostics, and representative D6
measurement). The chosen mechanism, layout/build model, and measurements are recorded in
[spec 0004](../specs/0004-hot-reload.md) when proven.

## State preservation

Regardless of mechanism, reloadable state that opts in is captured as a versioned,
`serde`-compatible snapshot owned by the host before apply or before a state-preserving restart, and
restored after success. State that cannot be restored (incompatible version/key, non-serialisable
resource, or explicit reset) must be reported, never silently replaced. Tip-crate statics and other
patch-unit-local state must not be treated as preserved across an in-process patch.

## Fallback

When a change is not safely reloadable in-process (host/bootstrap/toolchain/config changes, failed
compatibility/apply/patch generation, or mechanism limitations), the fallback is a
**state-preserving process restart**: persist snapshot, restart, restore. Slower, but it preserves
context and is labelled honestly.

## Constraints this imposes on application code

- Reloadable UI logic must cross explicit, stable hot boundaries; code outside those boundaries (or
  already captured in running futures/callbacks/init state) must not be assumed to update in place.
- Views and reloadable state must be reconstructible from host-owned snapshots/state.
- Reloadable tasks, subscriptions, and callbacks must be cancellable or invalidatable before replaced
  code can run.
- Preserved state must live on the host-stable side or in snapshotable form; tip-crate/patch-unit
  statics, thread-locals, and layout-incompatible values need reset or migration handling.
- Entity/view handles and callbacks must not be stored in global/static locations that outlive the
  reload unit or prevent safe invalidation.

## Open problems

- Product integration in `auro dev` (opt-in, dx/devserver or equivalent driver, target-dir isolation,
  machine-readable timing/trace, diagnostics).
- Project/layout/build strategy so warm view-only changes patch a small unit while host state stays
  stable (tip-crate boundary for Auro scaffolds/apps).
- Snapshot format/versioning, incompatible-change classification, and state migration/reset policy.
- Representative D6 measurements on Auro apps/scaffolds (warm/cold, debug/release, platform matrix).
- Debugging, symbols, breakpoints, and backtraces through the chosen reload mechanism.
- Windows and macOS feasibility, signing/entitlements, and CI verification.
- Windows GPUI feasibility, macOS feasibility, signing/entitlements, debugger behavior, and CI verification (plain non-GPUI tip-crate patching was prototyped on Windows 11 x86_64, 2026-11-16; not representative).
