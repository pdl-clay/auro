# 0017 - Autostart

**Status:** Implemented (2026-11-17)

Phase 2 needs apps to be able to opt into launching at login. This spec fixes the autostart service
contract. It delegates the platform mechanics to `auto-launch` and always uses the
least-privileged Windows mode. The `auro::services` re-export requires the facade's `services`
feature.

## Scope

- `auro-services::Autostart`: enable, disable and query whether an app launches at login.
- `Autostart::new(app_name, app_path, args)` and `Autostart::for_current_app(app_name, args)`.
- Platform modes: XDG autostart on Linux, Launch Agent on macOS, per-user registry entry on
  Windows.
- `auro::services::Autostart` re-export.

## Out of scope

- System-wide autostart or admin elevation.
- Editing existing autostart entries created by other tools.
- Scheduling, delayed start, or start conditions beyond login.
- Custom app bundle configuration on macOS.
- Other platform services.

## Decisions

### D1 - Delegate the platform mechanics

`auto-launch` owns the per-OS implementation. Auro maps it to a single type and picks the platform
mode:

| Platform | Mode | Artifact |
|---|---|---|
| Linux | `XdgAutostart` | `~/.config/autostart/<app>.desktop` |
| macOS | `LaunchAgent` | `~/Library/LaunchAgents/<app>.plist` |
| Windows | `CurrentUser` | `HKCU\...\Run` value |

*Rejected:* implementing the three mechanisms inside Auro. `auto-launch` is small, MIT licensed,
and already handles the quirks. *Rejected:* AppleScript login items. They need a macOS GUI session
and are less deterministic than a Launch Agent.

### D2 - Least privilege on Windows

`WindowsEnableMode::CurrentUser` is used instead of the default `Dynamic` mode, which would
otherwise try the system registry and require elevation. A user-level app should not ask for admin.

### D3 - The current executable is a convenience, not a policy

`for_current_app` resolves `std::env::current_exe()` and builds the same service as `new`. It is a
convenience for the common case. The app name is explicit; the path is the running binary.

### D4 - No admin, no silent fallback

The service does not fall back to a different mechanism when enable fails. A failed enable is an
error the app can surface.

## Interfaces

```rust
pub enum AutostartError { /* auto_launch::Error */ }

pub struct Autostart { /* auto_launch::AutoLaunch */ }

impl Autostart {
    pub fn new(
        app_name: impl Into<String>,
        app_path: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError>;
    pub fn for_current_app(
        app_name: impl Into<String>,
        args: &[impl AsRef<str>],
    ) -> Result<Self, AutostartError>;
    pub fn enable(&self) -> Result<(), AutostartError>;
    pub fn disable(&self) -> Result<(), AutostartError>;
    pub fn is_enabled(&self) -> Result<bool, AutostartError>;
}
```

Usage:

```rust,ignore
let autostart = auro::services::Autostart::for_current_app("com.example.app", &["--quiet"])?;
if !autostart.is_enabled()? {
    autostart.enable()?;
}
```

## Acceptance

1. A freshly created service reports not enabled
   (`reports_disabled_by_default`).
2. `enable` and `disable` round-trip the autostart state
   (`enables_and_disables_the_service`).
3. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
4. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| System-wide autostart and admin elevation | Autostart follow-up |
| Custom macOS bundle configuration | Autostart follow-up |
| Scheduling and start conditions | Autostart follow-up |
| Other platform services | Platform services specs |
