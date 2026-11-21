# 0015 - Single instance

**Status:** Implemented (2026-11-17; duplicate prevention, argument forwarding and opt-in
activation helper)

Phase 2 needs every app to be able to enforce one running instance. A process holds an advisory lock
for its configured app identifier; a second process detects that it is secondary before GPUI starts,
forwards its arguments and working directory over a local socket, and exits. The primary receives a
typed [`InstanceMessage`] on its normal event pump.

## Scope

- `auro-core::AppInstance`: acquire an advisory per-user file lock and report primary/secondary.
- `InstanceMessage`: forwarded arguments and working directory.
- Primary listener and secondary forwarding over a local socket.
- `App::single_instance()`: opt in when building an app; forwarding requires no app code.
- `App::activate_on_second_instance()`: opt-in focus of the first root window on a forwarded
  launch.
- `InstanceListener`: keeps the primary lock and forwarding thread alive.
- `AuroError::Instance` before any window opens on the secondary.
- Lock/socket naming, owner-only lock permissions on Unix, and OS release on exit/crash.

## Out of scope

- Custom activation policy; `App::activate_on_second_instance()` focuses the first root by
  default, while apps with other needs use `InstanceMessage` and `on_event`.
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

The path is `auro-<sanitised-id>-<hash>.lock`. The identifier is sanitised to ASCII alphanumerics,
`.`, `-` and `_` and truncated to 128 characters, so it cannot escape the directory; a stable
16-hex-digit FNV-1a hash of the full identifier follows the truncation, so two identifiers that
share their first 128 characters use different locks. On Unix the file is created with mode
`0600`. The directory is `XDG_RUNTIME_DIR` when it is a real directory owned by this user
with no group/other permission bits; otherwise the process uses a private `auro-<uid>` subdirectory
of the temporary directory, created with mode `0700` and re-validated (ownership, owner-only mode,
not a symlink) before use. The identifier is normally `app.identifier` from `Auro.toml`.

On a shared `/tmp`, a predictable name alone is not enough: another local user could pre-create the
directory, lock file or socket and receive forwarded arguments. An `XDG_RUNTIME_DIR` that fails the
check is ignored in favor of the fallback; a fallback directory that fails is refused with
`AuroError::Instance`. An attacker who pre-creates the per-user directory can still deny
single-instance operation (the app refuses to start that guard), which is a denial of service, not
an information leak. Interference by a same-user process remains outside the threat model.

### D3 - Secondary startup forwards, then exits with an error

The secondary acquires the lock (finding it already held), forwards `InstanceMessage { args, cwd }`,
and returns `AuroError::Instance` before GPUI starts. `App::run` prints it and exits with code 2. The
primary receives the message on its event pump. `App::activate_on_second_instance()` opts into
focusing `app.default-window` (the first declared window when the key is unset) and activating the
app; apps that need another window use `InstanceMessage` and `on_event`.

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

### D6 - Local socket, dual framing with an acknowledgement

On Unix the primary binds a filesystem socket in the per-user runtime directory selected by D2 (name
derived from a short sanitised prefix plus a stable hash of the full identifier, so long
reverse-DNS identifiers sharing a prefix cannot collide and the path stays inside `sun_path`); Linux applies
mode `0600`, and macOS uses the private directory under its per-user `$TMPDIR`. Windows uses a
named pipe with the same label. The listener accepts connections in a background thread, reads one
message, and sends the decoded `InstanceMessage` to the app's `EventSender`.

Two wire formats are accepted for one release: alpha.2's newline-delimited JSON and alpha.3's
8-byte big-endian length followed by JSON. The first byte distinguishes them (`{` or whitespace is
legacy JSON; a length prefix is binary). A framed message is acknowledged with one `0x06` byte
after the message is queued. A single handler worker drains the queue, so a slow app handler cannot
stall the listener, delay the acknowledgement, or cause the legacy retry to duplicate a launch. A
secondary tries the frame first and, if no acknowledgement arrives within 500 ms, reconnects and
writes the alpha.2 line. This lets alpha.4 forward to an alpha.2
primary (line fallback) and to an alpha.3 primary (frame accepted, legacy retry discarded), while
alpha.2 and alpha.3 secondaries can forward to alpha.4. Windows named pipes expose no receive
timeout, so the Windows secondary uses the legacy line directly.

The payload limit is 1 MiB. `forward` serialises first and returns `AuroError::Instance` for an
oversized message instead of writing a truncated frame and exiting as if delivery succeeded; the
listener independently rejects an oversized declared length or line. The limit replaces the former
64 KiB read, which silently truncated and dropped long launch arguments.

This avoids an abstract Linux socket, which has no filesystem permissions: a different local user
could otherwise inject messages. `ListenerOptions::try_overwrite` handles a stale socket name; the
file lock remains the authority that prevents two primaries. Invalid JSON is logged and never
invokes the app handler.

On Windows the named pipe is machine-global rather than behind a per-user directory; isolation
relies on the pipe's default ACL, and multi-user behavior has not been verified on a real desktop
session. The forwarded payload is arguments and a working directory, never secrets by contract.

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
    pub fn activate_on_second_instance(self) -> Self;
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
    .activate_on_second_instance()
    .root::<View>("main")
    .run();

// Or handle the forwarded launch yourself:
App::new()
    .single_instance()
    .on_event(|message: &auro::InstanceMessage, cx| {
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
6. `App::single_instance()` is opt-in and sets the builder flag; the activation helper enables
   both flags (`single_instance_is_opt_in`).
7. `AuroError::Instance` reports kind `"instance"` and includes name/message
   (`instance_errors_have_context_and_kind`).
8. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Custom activation behavior | App policy / `on_event` |
| File and deep-link routing into the primary | Deep-link and instance IPC follow-up |
| Lock/socket cleanup policy | Operations follow-up |
| Stable IPC protocol for third-party clients | Revisit only if required |
| Windows/macOS behavior under real desktop sessions | Platform audit |
