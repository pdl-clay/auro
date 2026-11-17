# 0020 - Global shortcuts

**Status:** Implemented (2026-11-17)

Phase 2 needs apps to be able to register global keyboard shortcuts. This spec fixes the shortcut
service contract. The backend is pluggable so tests can record calls instead of grabbing an
OS-level hotkey. The `auro::services` re-export requires the facade's `services` feature.

## Scope

- `auro-services::Shortcuts`: register and unregister a global shortcut by string.
- `ShortcutBackend`: a pluggable backend trait; the default is `global-hotkey`.
- `auro::services::Shortcuts` re-export.

## Out of scope

- Hotkey events and callbacks after registration.
- Registration persistence across app restarts.
- Custom key parsing outside `global-hotkey`.
- Other platform services.

## Decisions

### D1 - `global-hotkey` is the default backend

Auro delegates the per-OS mechanics to `global-hotkey`, which uses the Windows registry, X11
grab, and macOS `Carbon` register hotkey.

*Rejected:* implementing the three mechanisms inside Auro. `global-hotkey` is Apache/MIT licensed
and already handles the quirks.

### D2 - The backend is pluggable for tests

`Shortcuts::with_backend` accepts any `ShortcutBackend`. Tests can inject a recorder and verify
register/unregister calls without touching OS state. Production code uses `Shortcuts::new()`.

`ShortcutBackend` is not `Send + Sync`, because `global-hotkey`'s Windows manager holds a raw
pointer. The service stores the backend in an `Rc`, so it is main-thread-only like a GPUI view.

*Rejected:* a mock flag on the production type. It keeps the boundary explicit and makes the test
double a test-only type. *Rejected:* wrapping the manager in a mutex. It would still not be `Send`
on Windows and would add no useful sharing.

### D3 - No callback yet

The service only registers and unregisters a hotkey. A hotkey event and a callback to the app are a
later slice.

## Interfaces

```rust
pub enum ShortcutsError { /* global_hotkey::Error */ }

pub trait ShortcutBackend: Send + Sync {
    fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;
}

pub struct Shortcuts { /* backend */ }

impl Shortcuts {
    pub fn new() -> Result<Self, ShortcutsError>;
    pub fn with_backend(backend: impl ShortcutBackend + 'static) -> Self;
    pub fn register(&self, hotkey: &str) -> Result<(), ShortcutsError>;
    pub fn unregister(&self, hotkey: &str) -> Result<(), ShortcutsError>;
}
```

Usage:

```rust,ignore
let shortcuts = auro::services::Shortcuts::new()?;
shortcuts.register("shift+alt+k")?;
shortcuts.unregister("shift+alt+k")?;
```

## Acceptance

1. `register` and `unregister` call the backend with the hotkey
   (`registers_and_unregisters_the_hotkey`).
2. A backend error surfaces as `ShortcutsError`
   (`backend_errors_surface`).
3. An invalid hotkey is rejected before registration
   (`rejects_an_invalid_hotkey`).
4. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
5. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Hotkey events and callbacks | Shortcut UX spec |
| Persisting shortcuts across restarts | Shortcut UX spec |
| Custom key parsing | Shortcut UX spec |
| Other platform services | Platform services specs |
