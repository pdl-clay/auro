# 0021 - Opt-in rollback after a failed package apply

**Status:** Accepted (2026-11-18)

## Context

ADR [0018](0018-linux-package-rollback.md) gives the Linux package path an explicit rollback
operation but forbids invoking it automatically, because Auro cannot invent a safe previous
package. It closes with a revisit condition: "the apply journal gains an explicit rollback plan and
failure hook".

The journal now carries that plan when the caller supplies one. The remaining question is whether a
package-manager failure should use it automatically, and on which failures.

## Decision

A `.deb`/`.rpm` `ApplyPlan` may carry a `PackageRollback`: an already-staged older artifact, its
signed manifest and the app's pinned key. If the primary apply fails with `InstallerFailed`, the
helper re-verifies that package, requires a strictly lower version and the host platform, runs the
elevated downgrade, marks the journal `RolledBack`, and relaunches the old app without overwriting
that state.

The hook runs only on `InstallerFailed`. `ElevationFailed` (the user declined or `pkexec` could not
execute), a missing/tampered artifact, a platform mismatch, a version that is not older, and any
non-package apply do not trigger it. To make that distinction real, `pkexec` codes 126/127 (or no
code) stay `ElevationFailed`; other non-zero codes are the child package manager's status and are
`InstallerFailed`.

The caller still chooses and supplies the older package. Auro never queries a package cache,
retains a previous package or decides that a downgrade is dependency-safe.

## Consequences

**Easier**

- An app that already carries a rollback package can recover from a failed package transaction
  without a second app-level protocol.
- A declined authorization does not cause a second privileged prompt.
- The journal records the reverted outcome for the next start.

**Harder**

- The apply plan and journal are larger and carry a second manifest and key.
- A rollback package must be obtained and stored by release policy before it is needed.
- A rollback failure leaves two diagnostics in one `Failed` journal.

**Forbidden**

- Automatic rollback without a caller-supplied older package.
- Treating a package cache as a rollback source.
- Rolling back after a user declined elevation, or claiming a rollback when the package manager
  rejected the older package.
- Presenting a reverted update as a successful one; the state is `RolledBack`, not `Applied`.

This record supersedes only the "does not automatically invoke rollback on failure" and "no
automatic failure hook" sentences of ADR 0018; its artifact contract and downgrade flags remain.
