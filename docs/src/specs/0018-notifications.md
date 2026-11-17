# 0018 - Notifications

**Status:** Implemented (2026-11-17)

Phase 2 needs apps to be able to show desktop notifications. This spec fixes the notification
service contract. The backend is pluggable so tests can record calls instead of firing a real
notification. The `auro::services` re-export requires the facade's `services` feature.

## Scope

- `auro-services::Notifications`: show a desktop notification with title and body.
- `NotificationsBackend`: a pluggable backend trait; the default is `notify-rust`.
- `auro::services::Notifications` re-export.

## Out of scope

- Icons, images, actions, sounds, urgency or channel selection.
- Scheduling or repeating notifications.
- Notification handling after show (close, click, action callbacks).
- Other platform services.

## Decisions

### D1 - `notify-rust` is the default backend

Auro delegates the per-OS mechanics to `notify-rust`: D-Bus on Linux, NSUserNotificationCenter on
macOS, Windows toast notifications.

*Rejected:* writing a D-Bus/macOS/Windows client inside Auro. `notify-rust` is MIT licensed and
already handles the quirks.

### D2 - The backend is pluggable for tests

`Notifications::with_backend` accepts any `NotificationsBackend`. Tests can inject a recorder and
verify the title/body reach the backend without firing a real notification. Production code uses
`Notifications::new()` and the platform backend.

*Rejected:* a mock flag on the production type. It keeps the boundary explicit and makes the test
double a test-only type.

### D3 - Show only

The service shows a notification and returns; it does not retain a handle to close it later. Click
and action callbacks need the app to own the notification lifetime, which is a later slice.

## Interfaces

```rust
pub enum NotificationsError { /* notify_rust::error::Error */ }

pub trait NotificationsBackend: Send + Sync {
    fn show(&self, title: &str, body: &str) -> Result<(), NotificationsError>;
}

pub struct Notifications { /* backend */ }

impl Notifications {
    pub fn new() -> Self;
    pub fn with_backend(backend: impl NotificationsBackend + 'static) -> Self;
    pub fn show(&self, title: impl AsRef<str>, body: impl AsRef<str>) -> Result<(), NotificationsError>;
}
```

Usage:

```rust,ignore
let notifications = auro::services::Notifications::new();
notifications.show("Update available", "1.1.0 is ready to install")?;
```

## Acceptance

1. `show` sends the title and body to the backend
   (`sends_the_title_and_body_to_the_backend`).
2. A backend error surfaces as `NotificationsError`
   (`backend_errors_surface`).
3. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
4. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Click/action callbacks and closing notifications | Notification UX spec |
| Icons, images, urgency and channels | Notification UX spec |
| Notification categories and quiet hours | Notification UX spec |
| Other platform services | Platform services specs |
