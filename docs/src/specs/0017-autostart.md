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

### D1 - Delegate macOS and Windows, own the Linux entry

Auro writes the Linux XDG entry itself and delegates the macOS Launch Agent and Windows registry
entry to `auto-launch`. The platform modes are:

| Platform | Mode | Artifact |
|---|---|---|
| Linux | `XdgAutostart` | `~/.config/autostart/<app>.desktop` |
| macOS | `LaunchAgent` | `~/Library/LaunchAgents/<app>.plist` |
| Windows | `CurrentUser` | `HKCU\...\Run` value |

Auro owns the input contract: `app_name` must be one file-name component (no separators, control
characters or `.`/`..`), and `app_path` must be absolute and exist (on Linux, a file with at least
one execute bit). On Linux Auro also writes the desktop entry itself: `$XDG_CONFIG_HOME/autostart` when that variable is an absolute
path, otherwise `$HOME/.config/autostart`, with the executable and every argument quoted for a
Desktop Entry `Exec` value (`%` -> `%%`, backslash/quote/backtick/`$` escaping, control characters
rejected). The entry is written to a sibling temporary file and renamed into place, so a pre-created
symlink at `<app>.desktop` is replaced rather than followed. Without that, a path with spaces split
into different arguments, a path with `%f` or a newline changed the launched command or injected
keys, and `XDG_CONFIG_HOME` was ignored.

macOS and Windows keep delegating to `auto-launch`. *Rejected:* AppleScript login items. They need a
macOS GUI session and are less deterministic than a Launch Agent. *Rejected:* keeping
`auto-launch` on Linux after its raw `Exec` interpolation and `dirs::home_dir()` path proved
unfixable through its public API.

### D2 - Least privilege on Windows

`WindowsEnableMode::CurrentUser` is used instead of the default `Dynamic` mode, which would
otherwise try the system registry and require elevation. A user-level app should not ask for admin.

### D3 - The current executable is a convenience, not a policy

`for_current_app` resolves the running app and builds the same service as `new`. On Linux, a
non-empty `$APPIMAGE` wins over `std::env::current_exe()`: inside an AppImage the latter points at
the temporary squashfs mount, which disappears when the process exits, so the autostart entry would
launch a missing file at the next login. Elsewhere the path is `current_exe()`. The app name is
explicit; the path is the running binary.

### D4 - No admin, no silent fallback

The service does not fall back to a different mechanism when enable fails. A failed enable is an
error the app can surface.

## Interfaces

```rust
pub enum AutostartError {
    AutoLaunch(auto_launch::Error), // macOS/Windows
    Invalid { field: &'static str, message: String },
    Io(std::io::Error),
}

pub struct Autostart { /* Linux entry data, or auto_launch::AutoLaunch */ }

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
3. The Linux entry honors `XDG_CONFIG_HOME` and quotes `Exec` paths/arguments with spaces
   (`autostart_dir_honors_xdg_config_home`, `linux_entry_quotes_paths_and_arguments_with_spaces`).
4. A relative, missing or non-file `app_path` is rejected
   (`rejects_a_relative_or_missing_app_path`).
5. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| System-wide autostart and admin elevation | Autostart follow-up |
| Custom macOS bundle configuration | Autostart follow-up |
| Scheduling and start conditions | Autostart follow-up |
| Other platform services | Platform services specs |
