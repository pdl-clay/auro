# 0015 - Single instance

**Status:** Implemented (2026-11-17; duplicate prevention only; argument forwarding and primary
activation are not implemented)

Phase 2 needs every app to be able to enforce one running instance. This spec fixes the first
service contract: a process holds an advisory lock for its configured app identifier, and a second
process detects that it is secondary before GPUI starts. It deliberately does not implement IPC
argument forwarding or window activation yet.

## Scope

- `auro-core::AppInstance`: acquire an advisory per-user file lock, report primary/secondary.
- `App::single_instance()`: opt in when building an app.
- Secondary startup fails with `AuroError::Instance` before any window opens.
- Lock-file naming, owner-only permissions on Unix, and OS release on process exit/crash.

## Out of scope

- Forwarding command-line arguments, deep links or files to the primary instance.
- Activating or focusing the primary window.
- A network/socket protocol between instances.
- Cross-user or system-wide single-instance enforcement.
- Cleaning up lock files after exit (the file is intentionally retained; the OS lock is not).
- Defending against a same-user process that deliberately pre-creates or locks the file.

## Decisions

### D1 - Use the standard library's advisory file lock

`AppInstance` opens a lock file and calls `File::try_lock`. `TryLockError::WouldBlock` means another
process holds it; any other error is an `AuroError::Instance`. The lock is released when the file is
closed, including on a crash; the lock file remains on disk and is reused.

*Rejected:* a PID file. A stale PID can belong to an unrelated process after reboot, and checking it
is racy. *Rejected:* a network listener for a first version; it adds ports/firewalls before argument
forwarding exists.

### D2 - The lock lives in a per-user runtime directory

The path is `auro-<sanitised-id>.lock`. The identifier is sanitised to ASCII alphanumerics, `.`, `-`
and `_`, truncated to 128 characters, so it cannot escape the directory. On Unix the file is created
with mode `0600`. The directory is `XDG_RUNTIME_DIR` when it exists, otherwise the per-user temporary
directory. The identifier is normally `app.identifier` from `Auro.toml`.

Linux containers without `XDG_RUNTIME_DIR` fall back to the temporary directory; the lock is still
per-process-user in the common case, but a shared `/tmp` can be deliberately interfered with by the
same user. That is outside the threat model.

### D3 - Secondary startup is an error, not a silent exit

`App::try_run` returns `AuroError::Instance` before GPUI starts; `App::run` prints it and exits with
code 2. This makes a duplicate launch observable in scripts and logs. Activating the primary window
is a later feature; the secondary process has no IPC channel yet.

*Rejected:* returning success from the secondary process. It would hide a user's attempted launch
and make automated tests unable to distinguish "opened" from "refused".

### D4 - Opt-in per app

Single-instance is enabled with `App::single_instance()`; the default is off. An app that wants
multiple independent windows/processes, or a CLI mode, is not forced into the guard. This mirrors the
app model's preference for explicit capabilities over invisible global policy.

### D5 - Guard lifetime is the app lifetime

The guard is acquired after config load and before `Application::new().run`, and is held until
`try_run` returns. Dropping it closes the lock file. The guard is not stored in managed state; app
code does not need to keep a second handle alive.

## Interfaces

```rust
pub struct AppInstance { /* file, path, name, primary */ }

impl AppInstance {
    pub fn acquire(name: &str) -> Result<Self, AuroError>;
    pub fn is_primary(&self) -> bool;
    pub fn name(&self) -> &str;
    pub fn path(&self) -> &Path;
}

impl App {
    pub fn single_instance(self) -> Self;
}
```

`AuroError` gains `Instance { name, message }` with kind `"instance"`.

Usage:

```rust,ignore
App::new()
    .single_instance()
    .root::<View>("main")
    .run();
```

## Acceptance

1. The first guard is primary; a second guard for the same name is secondary; after dropping the
   first, a new guard is primary again (`first_guard_is_primary_and_second_is_not`).
2. An empty identifier is a typed `AuroError::Instance` (`empty_name_is_rejected`).
3. Lock-file names replace separators and cannot escape the lock directory (`path_is_sanitised`).
4. `App::single_instance()` is opt-in and sets the builder flag (`single_instance_is_opt_in`).
5. `AuroError::Instance` reports kind `"instance"` and includes name/message
   (`instance_errors_have_context_and_kind`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Argument/file/deep-link forwarding to the primary | Single-instance IPC spec |
| Activating the primary window | Single-instance IPC / platform activation spec |
| Lock-file cleanup policy | Operations follow-up |
| A socket/DBus implementation instead of a file lock | Revisit if forwarding or cross-user semantics demand it |
