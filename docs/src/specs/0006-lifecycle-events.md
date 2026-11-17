# 0006 - Lifecycle events

**Status:** Implemented (2026-11-17)

Phase 1 needs a way for an app to react to its own lifecycle: load data when it is ready, clean up
when a window closes, and know when the last window is going away. This spec defines that contract.
It is the first slice of the "lifecycle and app-wide events" item; user-defined app events are a
separate spec.

## Scope

- A typed `LifecycleEvent` enum in `auro-core`.
- `App::on_lifecycle` handlers, run on the main thread.
- Window-opened, window-closed and ready/exiting ordering for windows declared in `Auro.toml`.
- Tracking the window handles Auro opened so a close can be attributed to a label.

## Out of scope

- A user-defined app event bus. The generic typed event bus is the next core slice.
- Runtime window opening, closing or focusing by label (`WindowManager`).
- Cancelling exit. `Exiting` is informational in Phase 1.
- Focus, resize, move and platform lifecycle events.
- macOS/Windows-specific termination (Cmd-Q, session end) beyond the GPUI close path.

## Decisions

### D1 - The event is a closed, typed enum

```rust
#[non_exhaustive]
pub enum LifecycleEvent {
    Ready,
    WindowOpened { label: String },
    WindowClosed { label: String },
    Exiting,
}
```

`LifecycleEvent::label()` returns the label for window events. The enum lives in `auro-core`, so it
carries no renderer type; it is `Clone`, `Debug`, `PartialEq` and `Eq` for tests and logging.

### D2 - Handlers are main-thread and ordered

`App::on_lifecycle(|event, cx| ...)` registers a handler. Handlers run synchronously on the GPUI
main thread, in registration order, with the app handle. They may update entities and managed state;
they must not block (the same rule as render). A panic propagates: Auro does not swallow lifecycle
handler failures.

Handlers are `Fn`, not `FnOnce`; they can be called once per event for the process lifetime.

### D3 - Order

At startup:

```
setups (registration order)
for each configured window, in declaration order:
    window opens -> WindowOpened { label }
Ready
```

Closing:

```
WindowClosed { label } for each tracked window that disappeared
Exiting                      (only when no windows remain)
quit
```

If several tracked windows disappear between callbacks, each gets `WindowClosed` before `Exiting`.
`Ready` is emitted after the startup loop even if a window failed to open; the failure is logged by
the opener, not hidden.

### D4 - Only Auro-opened windows are tracked

The close path compares the handles Auro recorded when it opened each window against
`cx.windows()`. A window a toolkit or the app opened directly through GPUI is not tracked and emits
no `WindowClosed`; runtime opening through Auro arrives with the window manager, which must extend
this registry.

A configured window with no root view is not opened and emits no `WindowOpened`.

### D5 - Exiting is informational

`Exiting` fires immediately before `cx.quit()` when the last window closes. It cannot veto or defer
the exit; a cancellable exit needs a return-value contract and belongs to a later lifecycle spec.
Ctrl-C, SIGTERM and platform termination are not lifecycle events in Phase 1.

### D6 - No lifecycle state crosses threads

Handlers and the handle registry are main-thread-only (`Rc`/`RefCell` internally). An app that wants
background work to react to lifecycle should send its own message to the main thread; Auro does not
expose a `Send` lifecycle receiver in this slice.

## Interfaces

```rust,ignore
use auro::prelude::*;

App::new()
    .on_lifecycle(|event, cx| match event {
        LifecycleEvent::Ready => {
            // load the app's data, update an entity, cx.notify()
        }
        LifecycleEvent::WindowClosed { label } => {
            // release per-window resources
        }
        LifecycleEvent::Exiting => {
            // final flush; cannot cancel
        }
        _ => {}
    })
    .root::<LogViewer>("main")
    .run();
```

`App::on_lifecycle` is additive and chainable. Registration order is invocation order.

## Acceptance

1. A headless test opens a configured window and records `WindowOpened { label }` before `Ready`;
   the root builder runs after setups (`app::tests::setup_runs_before_roots_and_lifecycle_reports_ready`).
2. Lifecycle handlers run in registration order; the same test records two handlers.
3. `WindowClosed` and `Exiting` dispatch to handlers and preserve payloads
   (`app::tests::lifecycle_reports_window_close_and_exit`).
4. The opened handle is tracked for close attribution; the first test asserts one tracked handle.
5. A real app with a lifecycle handler logs `Ready`/`WindowOpened`, and a real window close logs
   `WindowClosed`/`Exiting` before exit. Recorded in the pull request; the close path remains a
   manual run because GPUI's test platform does not simulate a compositor close.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Generic user-defined app event bus, background-to-main delivery | Next core spec |
| Runtime `WindowManager`: open/close/focus by label; extend the registry | Phase 1 window spec |
| Cancellable `ExitRequested` and platform termination events | Later lifecycle spec |
| Focus/resize/move events and per-window subscriptions | After the window manager |
| Whether lifecycle handlers should receive `&Config` or a window handle | Revisit with the window manager |
