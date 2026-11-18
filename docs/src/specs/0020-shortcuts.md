# 0020 - Global shortcuts

**Status:** Implemented (2026-11-17; registration and pressed callbacks)

Phase 2 needs apps to be able to register global keyboard shortcuts and react to them. This spec
fixes the shortcut service contract. The backend is pluggable so tests can record calls and invoke
callbacks instead of grabbing an OS-level hotkey. The `auro::services` re-export requires the
facade's `services` feature.

## Scope

- `auro-services::Shortcuts`: register and unregister a global shortcut by string.
- `Shortcuts::on_shortcut`: register a callback invoked when that hotkey is pressed.
- `ShortcutBackend`: a pluggable backend trait; the default is `global-hotkey`.
- `auro::services::Shortcuts` re-export.

## Out of scope

- Registration persistence across app restarts.
- Custom key parsing outside `global-hotkey`.
- Callbacks for key release, repeat suppression or modifier-only hotkeys.
- Other platform services.

## Decisions

### D1 - `global-hotkey` is the default backend

Auro delegates the per-OS mechanics to `global-hotkey`, which uses the Windows registry, X11
grab, and macOS `Carbon` register hotkey.

*Rejected:* implementing the three mechanisms inside Auro. `global-hotkey` is Apache/MIT licensed
and already handles the quirks.

### D2 - The backend is pluggable for tests

`Shortcuts::with_backend` accepts any `ShortcutBackend`. Tests can inject a recorder, verify
register/unregister calls, and fire callbacks without touching OS state. Production code uses
`Shortcuts::new()`.

`ShortcutBackend` is not `Send + Sync`, because `global-hotkey`'s Windows manager holds a raw
pointer. The service stores the backend in an `Rc`, so it is main-thread-only like a GPUI view.
Callbacks *are* `Send + Sync`; they run off the app thread.

*Rejected:* a mock flag on the production type. It keeps the boundary explicit and makes the test
double a test-only type. *Rejected:* wrapping the manager in a mutex. It would still not be `Send`
on Windows and would add no useful sharing.

### D3 - Pressed events dispatch on a service thread

`Shortcuts::on_shortcut` validates the hotkey and stores the callback keyed by its text. The default
backend maps `HotKey::id()` to that text at registration time and spawns one `auro-shortcuts` thread
that reads `GlobalHotKeyEvent::receiver()`. `Pressed` events look up the hotkey and invoke every
registered callback; `Released` events are ignored.

Registration and callback attachment are separate: an app calls `register(hotkey)` to take the OS
hotkey and `on_shortcut(hotkey, callback)` to react. This lets an app register before or after the
callback is installed.

The callback runs off the app thread. Apps use `EventSender` (or another channel) to move the work
onto the main thread; the service does not depend on GPUI or `auro-core`.

*Rejected:* polling inside the GPUI event loop. It forces every app to write a timer and a drain
loop. *Rejected:* invoking a GPUI entity from the service. The service would depend on the renderer
and the callback would need all the main-thread rules.

## Interfaces

```rust
pub enum ShortcutsError { /* backend, parse, event-loop spawn */ }

pub trait ShortcutBackend {
    fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    fn on_shortcut(
        &self,
        hotkey: &str,
        handler: Arc<dyn Fn() + Send + Sync>,
    );
}

pub struct Shortcuts { /* backend */ }

impl Shortcuts {
    pub fn new() -> Result<Self, ShortcutsError>;
    pub fn with_backend(backend: impl ShortcutBackend + 'static) -> Self;
    pub fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    pub fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    pub fn on_shortcut<F>(&self, hotkey: &str, handler: F) -> Result<(), ShortcutsError>
    where
        F: Fn() + Send + Sync + 'static;
}
```

Usage:

```rust,ignore
let shortcuts = auro::services::Shortcuts::new()?;
shortcuts.on_shortcut("shift+alt+k", move || {
    // Send to the app thread.
})?;
shortcuts.register("shift+alt+k")?;
shortcuts.unregister("shift+alt+k")?;
```

## Acceptance

1. `register` and `unregister` call the backend with the hotkey
   (`registers_and_unregisters_the_hotkey`).
2. A pressed hotkey invokes its callback (`dispatches_a_pressed_hotkey`).
3. An invalid callback hotkey is rejected before the backend is called
   (`rejects_an_invalid_callback_hotkey`).
4. A backend error surfaces as `ShortcutsError` (`backend_errors_surface`).
5. An invalid hotkey is rejected before registration (`rejects_an_invalid_hotkey`).
6. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
7. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Persisting shortcuts across restarts | Shortcut UX spec |
| Custom key parsing | Shortcut UX spec |
| Release events and repeat suppression | Shortcut UX spec |
| Other platform services | Platform services specs |
