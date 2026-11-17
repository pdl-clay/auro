# 0009 - Window manager

**Status:** Implemented (2026-11-17)

The lifecycle slice tracks the windows Auro opens and reports their close. Phase 1 also needs the
app to act on a window by its configured label: open a settings window from a menu, focus it, close
it, and know whether it is open. This spec fixes that contract for windows declared in `Auro.toml`.

## Scope

- A main-thread window manager installed by `App::run`.
- `open`, `close`, `focus`, `is_open` and `handle` by configured label.
- Interaction with `LifecycleEvent::WindowOpened` / `WindowClosed` / `Exiting`.
- A typed error for an unknown label or a missing root.

## Out of scope

- Creating a window that is not declared in `Auro.toml`.
- Multiple simultaneous instances of one label.
- Window position/size persistence, modal parents, focus/resize events.
- Preventing close or exit; the lifecycle contract stays informational.
- Platform-specific window controls beyond what GPUI exposes.
- Managing windows that the app or a toolkit opened directly through GPUI.

## Decisions

### D1 - The label is the identity

A window is identified by the `label` declared in `app.windows`. The configuration is
authoritative: open requires a declared label with an attached root. Two windows with the same
label are rejected by config validation, so the manager stores at most one handle per label.

### D2 - The manager is installed at run

`App::run` installs a main-thread `WindowManager` global before setups, holds the parsed config, the
root builders, the lifecycle dispatcher and the same handle registry the close observer uses. This
is why the close observer and the manager cannot drift: they share the registry.

The manager is not available before `App::run`; calling a window function then is a programming
error and panics with a clear message, like `emit` before the event bus is installed.

### D3 - Open

`windows::open(label, cx)`:

- unknown label -> `AuroError::Window { label, message }`;
- label declared but no root attached -> the same typed error;
- already open -> returns the existing handle, does not create a second window and does not emit a
  second `WindowOpened`;
- otherwise builds the root through the registered builder, records the handle, emits
  `LifecycleEvent::WindowOpened { label }` and returns the handle.

Opening does not focus the window unless the root/GPUI does so; `focus` is explicit.

### D4 - Close

`windows::close(label, cx)` removes the GPUI window. The existing close observer attributes the
disappearance to the label, emits `WindowClosed`, and emits `Exiting` if it was the last window.
`close` returns `false` when the label is unknown or not open; it does not panic.

### D5 - Focus and queries

- `focus(label, cx)` activates the window (`Window::activate_window`) and returns `false` when it
  is not open.
- `is_open(label, cx)` checks the live GPUI window list, not just a stale registry entry.
- `handle(label, cx)` returns the current `AnyWindowHandle`.

All are main-thread only. None blocks or performs I/O.

### D6 - Startup is unchanged

`App::run` still opens every configured window that has a root, in declaration order, through the
manager's `open` path, then emits `Ready`. The manager therefore owns the startup open as well;
there is one code path for initial and later opens.

## Interfaces

```rust,ignore
use auro::{prelude::*, windows};

fn open_settings(cx: &mut gpui::App) {
    match windows::open("settings", cx) {
        Ok(_) => {}
        Err(error) => eprintln!("settings: {error}"),
    }
}

fn close_settings(cx: &mut gpui::App) {
    let _ = windows::close("settings", cx);
}

fn focus_settings(cx: &mut gpui::App) -> bool {
    windows::focus("settings", cx)
}

let open = windows::is_open("settings", cx);
let handle = windows::handle("settings", cx);
```

`AuroError::Window { label, message }` has kind `"window"`; the message names the label and whether
it is undeclared or has no root.

## Acceptance

1. Opening an undeclared label returns `AuroError::Window`; opening a declared label with no root
   returns the same type.
2. Opening twice returns the same handle and emits `WindowOpened` once.
3. Closing an open window removes it and emits `WindowClosed`; closing the last window emits
   `Exiting` through the existing close observer.
4. Reopening a closed configured window works and emits `WindowOpened` again.
5. `focus` returns true for an open window and false otherwise; `is_open`/`handle` agree with
   `cx.windows()`.
6. A real app opens and closes a second configured window from a lifecycle handler; recorded in the
   pull request with the environment.
7. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Focus/resize/move events and per-window subscriptions | After this slice |
| Window position/size persistence across runs | Platform-services slice |
| Modal parent/child relationships | Platform-services slice |
| Preventing close or exit | Later lifecycle spec |
| Multiple instances of one label (e.g. an untitled document window) | Needs an instance id, not a label; later |
