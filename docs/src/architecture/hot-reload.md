# Hot reload

This is the existential bet of the project. If the edit loop stays at a multi-second/minute
rebuild-and-relaunch, nobody leaves the web, and the rest of Auro does not matter.

> Target: edit to see in **under 500 ms**, with app state preserved.

The target and observable contract are fixed; the mechanism is under revision. Normative dev-loop
contract, staging, and acceptance are drafted in [spec 0004](../specs/0004-hot-reload.md).

## Contract

- **Default today:** `auro dev` rebuilds and relaunches the app process (Phase 0). A saved change to
  a visible relaunch measured ~60 s on the development machine (relink dominates, 2026-11-15). This
  is not hot reload.
- **Hot reload goal:** a warm, view-only change becomes visible in under 500 ms with opted-in state
  preserved, without closing the window.
- **Honesty rule:** a process restart is never presented as hot reload. A failed build keeps the
  previous UI active. A reload that cannot be applied safely falls back to a state-preserving
  restart only when state can be restored, and the outcome is labelled explicitly.

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

## Direction under investigation

Per [ADR 0008](../decisions/0008-hot-reload-mechanism-revision.md), the mechanism must keep one
GPUI `Application`/window/event loop and reload updated UI code in the same process and type
universe. The leading direction to prototype is same-process/in-process reload (e.g.
function-level hotpatching, subject to platform/toolchain/GPUI feasibility), or another approach
proven to satisfy the contract. The chosen mechanism, layout/build model, and measurements are
recorded in spec 0004 when proven.

## State preservation

Regardless of mechanism, reloadable state that opts in is captured as a versioned,
`serde`-compatible snapshot owned by the host before apply or before a state-preserving restart, and
restored after success. State that cannot be restored (incompatible version/key, non-serialisable
resource, or explicit reset) must be reported, never silently replaced.

## Fallback

When a change is not safely reloadable in-process (host/bootstrap/toolchain/config changes, failed
compatibility/apply, or mechanism limitations), the fallback is a **state-preserving process
restart**: persist snapshot, restart, restore. Slower, but it preserves context and is labelled
honestly.

## Constraints this imposes on application code

- Views and reloadable state must be reconstructible from state/snapshots.
- Reloadable tasks, subscriptions, and callbacks must be cancellable or invalidatable before replaced
  code can run.
- Entity/view handles and callbacks must not be stored in global/static locations that outlive the
  reload unit or prevent safe invalidation.

## Open problems

- A proven in-process reload mechanism compatible with GPUI 0.2.2 and Tier 1 platforms.
- Build/layout strategy that makes warm view-only updates fast enough (compile/apply/restore/repaint
  within budget), not just correct.
- Snapshot format/versioning and compatibility policy for app-defined reloadable state.
- Debugging, symbols, breakpoints, and backtraces through the chosen reload mechanism.
- Classification rules and project layout for hot-reload-capable apps.
