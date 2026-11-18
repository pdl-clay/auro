# 0015 - Restart through the app executable and a durable journal

**Status:** Accepted (2026-11-18)

## Context

After an apply, the running process must not launch its replacement itself: the new process would
race the old one, and the old process may still need to flush state. A helper must wait for the app
to exit, then launch the replacement. Options:

1. Ship a separate helper binary and add it to every app bundle. Rejected for now: it adds a second
   artifact, discovery rules and a packaging contract before the handshake is proven.
2. Use a shell loop or a platform service. Rejected: shell construction is platform-specific and
   unsafe for paths; a service is a larger privilege/deployment story.
3. Re-execute the app executable in helper mode, gated by an environment variable, and coordinate
   with a lock file plus a durable journal. Chosen.

## Decision

`ApplyJournal` records the applied version, artifact, optional target/backup, relaunch command and
state. `spawn_helper` takes an exclusive sibling lock and spawns the current executable with
`AURO_UPDATE_RESTART_HELPER` pointing at the journal. On Linux the relaunch command prefers
`$APPIMAGE`; `current_exe()` would name the temporary AppImage mount. `App::try_run` recognizes that variable before
config/GPUI, waits for the old process to release the lock, requires the journal to say `Applied`,
and launches the recorded command with the helper variable removed.

The helper process is detached into its own process group. It only relaunches. Platform apply
remains in the app process (specs 0014/0025); a failed apply writes `Failed` and never spawns the
helper. Because helper re-exec happens after apply, the executable at the relaunch path after the
swap must honor the same environment/journal contract.

## Consequences

**Easier**

- No second executable or packaging convention is required.
- The handoff is testable without GPUI and works on all three hosts.
- The journal gives a durable trace of what was applied, even when the relaunch fails.

**Harder**

- The app must call `App::try_run`/`App::run` for helper mode to exist.
- The lock has a 120 s wait; a longer shutdown needs a policy.
- Journals and backups accumulate until a later cleanup/recovery policy.
- The helper does not apply; a crash before `spawn_helper` leaves a journal to inspect, not an
  automatic recovery.

**Forbidden**

- Launching the replacement before the old process releases the handoff lock.
- Re-entering helper mode in the replacement process.
- Claiming a restart when the journal is not `Applied`.

## Revisit when

- Helper-side apply, elevation or OS services are specified.
- Apps need a shutdown longer than the fixed wait.
- A journal recovery/cleanup policy is added.
