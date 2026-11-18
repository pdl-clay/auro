# 0015 - Single instance

**Status:** Implemented (2026-11-17; duplicate prevention and argument forwarding; automatic
activation is app policy)

Phase 2 needs every app to be able to enforce one running instance. A process holds an advisory lock
for its configured app identifier; a second process detects that it is secondary before GPUI starts,
forwards its arguments and working directory over a local socket, and exits. The primary receives a
typed [`InstanceMessage`] on its normal event pump.

## Scope

- `auro-core::AppInstance`: acquire an advisory per-user file lock and report primary/secondary.
- `InstanceMessage`: forwarded arguments and working directory.
- Primary listener and secondary forwarding over a local socket.
- `App::single_instance()`: opt in when building an app; forwarding requires no app code.
- `InstanceListener`: keeps the primary lock and forwarding thread alive.
- `AuroError::Instance` before any window opens on the secondary.
- Lock/socket naming, owner-only lock permissions on Unix, and OS release on exit/crash.

## Out of scope

- Automatically activating or focusing the primary window; the app decides from
  `InstanceMessage`.
- Deep-link/file association policy; forwarded arguments are just arguments.
- Cross-user or system-wide single-instance enforcement.
- Cleaning up lock or socket files after exit (the lock file is retained; the socket file is
  reused or overwritten on the next start).
- Defending against a same-user process that deliberately pre-creates or locks the file.
- A stable public IPC protocol for third-party clients.

## Decisions

### D1 - Use the standard library's advisory file lock

`AppInstance` opens a lock file and calls `File::try_lock`. `TryLockError::WouldBlock` means another
process holds it; any other error is an `AuroError::Instance`. The lock is released when the file is
closed, including on a crash; the lock file remains on disk and is reused.

*Rejected:* a PID file. A stale PID can belong to an unrelated process after reboot, and checking it
is racy. *Rejected:* a TCP listener; a local socket has no ports, firewall rules or network exposure.

### D2 - The lock lives in a per-user runtime directory

The path is `auro-<sanitised-id>.lock`. The identifier is sanitised to ASCII alphanumerics, `.`, `-`
and `_`, truncated to 128 characters, so it cannot escape the directory. On Unix the file is created
with mode `0600`. The directory is `XDG_RUNTIME_DIR` when it exists, otherwise the per-user temporary
directory. The identifier is normally `app.identifier` from `Auro.toml`.

Linux containers without `XDG_RUNTIME_DIR` fall back to the temporary directory; the lock is still
per-process-user in the common case, but a shared `/tmp` can be deliberately interfered with by the
same user. That is outside the threat model.

### D3 - Secondary startup forwards, then exits with an error

The secondary acquires the lock (finding it already held), forwards `InstanceMessage { args, cwd }`,
and returns `AuroError::Instance` before GPUI starts. `App::run` prints it and exits with code 2. The
primary receives the message on its event pump; automatic window activation is app policy.

*Rejected:* returning success from the secondary process. It would hide a user's attempted launch
and make automated tests unable to distinguish "opened" from "refused".

### D4 - Opt-in per app

Single-instance is enabled with `App::single_instance()`; the default is off. An app that wants
multiple independent windows/processes, or a CLI mode, is not forced into the guard. This mirrors the
app model's preference for explicit capabilities over invisible global policy.

### D5 - Guard and listener lifetime are the app lifetime

The lock and listener are acquired after config load and before `Application::new().run`, and are
held until `try_run` returns. Dropping the guard closes the lock; dropping the listener detaches the
forwarding thread and the process exit releases the socket. Neither is stored in managed state.

### D6 - Local socket, one JSON line per message

On Unix the primary binds a filesystem socket in the per-user runtime directory (name derived from
the same sanitised identifier, truncated to 32 characters to fit `sun_path`); Linux applies mode
`0600`, and macOS uses the per-user `$TMPDIR`. Windows uses a named pipe. The listener accepts
connections in a background thread, reads one JSON line capped at 64 KiB, and sends the decoded
`InstanceMessage` to the app's `EventSender`. The secondary writes the same JSON line and closes
the connection.

This avoids an abstract Linux socket, which has no filesystem permissions: a different local user
could otherwise inject messages. `ListenerOptions::try_overwrite` handles a stale socket name; the
file lock remains the authority that prevents two primaries. Invalid JSON is logged and never
invokes the app handler.

*Rejected:* a shared-memory or file-drop protocol. It needs polling or a second notification channel
and is harder to test than a local socket. *Rejected:* an abstract socket, because peer credentials
would become a second authentication path.

## Interfaces

```rust
pub struct InstanceMessage {
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub struct AppInstance { /* file, path, name, primary */ }

impl AppInstance {
    pub fn acquire(name: &str) -> Result<Self, AuroError>;
    pub fn is_primary(&self) -> bool;
    pub fn name(&self) -> &str;
    pub fn path(&self) -> &Path;
    pub fn forward(&self, message: &InstanceMessage) -> Result<(), AuroError>;
    pub fn listen(
        self,
        handler: impl Fn(InstanceMessage) + Send + 'static,
    ) -> Result<InstanceListener, AuroError>;
}

pub struct InstanceListener { /* guard, thread */ }

impl App {
    pub fn single_instance(self) -> Self;
    pub fn on_event<E: Send + 'static>(
        self,
        handler: impl Fn(&E, &mut GpuiApp) + 'static,
    ) -> Self;
}
```

`AuroError` gains `Instance { name, message }` with kind `"instance"`.

Usage:

```rust,ignore
App::new()
    .single_instance()
    .on_event(|message: &auro::InstanceMessage, cx| {
        // `message.args` and `message.cwd` are the second launch's.
        let _ = auro::windows::focus("main", cx);
    })
    .root::<View>("main")
    .run();
```

## Acceptance

1. The first guard is primary; a second guard for the same name is secondary; after dropping the
   first, a new guard is primary again (`first_guard_is_primary_and_second_is_not`).
2. A secondary forwards an `InstanceMessage` to the primary listener
   (`forwards_a_message_to_the_primary_listener`).
3. Primary forwarding and secondary listening are rejected
   (`primary_cannot_forward_or_secondary_cannot_listen`).
4. An empty identifier is a typed `AuroError::Instance` (`empty_name_is_rejected`).
5. Lock-file names replace separators and cannot escape the lock directory (`path_is_sanitised`).
6. `App::single_instance()` is opt-in and sets the builder flag (`single_instance_is_opt_in`).
7. `AuroError::Instance` reports kind `"instance"` and includes name/message
   (`instance_errors_have_context_and_kind`).
8. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Automatic primary-window activation | App policy / platform activation follow-up |
| File and deep-link routing into the primary | Deep-link and instance IPC follow-up |
| Lock/socket cleanup policy | Operations follow-up |
| Stable IPC protocol for third-party clients | Revisit only if required |
| Windows/macOS behavior under real desktop sessions | Platform audit |
