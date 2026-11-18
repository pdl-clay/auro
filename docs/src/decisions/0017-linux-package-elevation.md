# 0017 - Linux package elevation is explicit pkexec, never self-root

**Status:** Accepted (2026-11-18)

## Context

Spec 0027 deliberately ran `dpkg`/`rpm` as the current user and left elevation out. That is safe but
incomplete: installing a package needs root, and simply running the whole app as root spreads the
privilege across UI, network and update code. A package manager also executes maintainer scripts,
so the boundary must be explicit and auditable.

The alternatives are a root-owned helper (best long-term isolation, needs installation/polkit
policy), `sudo` (TTY/password semantics, not desktop-friendly) and `pkexec` (standard polkit, desktop
agent). The first is more work than this slice; the second is a poor fit for a GUI app.

## Decision

The restart helper uses `pkexec` with absolute paths and an argument vector:

```text
/usr/bin/pkexec /usr/bin/dpkg -i /proc/<helper-pid>/fd/<fd>
/usr/bin/pkexec /usr/bin/rpm -U /proc/<helper-pid>/fd/<fd>
```

The helper opens the already-verified staged artifact and keeps the read-only descriptor alive while
`pkexec` runs. Elevation is opt-in through `LinuxPackageInstaller::elevated()`; `new()` remains the
unprivileged path. Any non-zero `pkexec` result is `ElevationFailed`, the journal records `Failed`,
and the app is not relaunched. Auro does not roll back package-manager transactions.

## Consequences

**Easier**

- Package installs work on desktops with a polkit agent without making the app root.
- Command construction stays shell-free and testable with a mock runner.
- A failed or cancelled authorization is distinguishable from a package-manager failure.

**Harder**

- `pkexec`, `/proc` and a polkit agent are runtime dependencies on Linux.
- A held descriptor does not replace a root-owned helper for a fully hardened same-user threat model.
- Package transactions have no Auro rollback; failures remain in the journal for diagnosis.
- A distro that wants a narrowed policy must add a dedicated helper and policy later.

**Forbidden**

- Re-executing the app as root or calling `setuid`.
- A shell string, `sudo`, or an auto-elevated package path.
- Relaunching after a denied or failed elevation.
- Claiming package rollback when the package manager owns the transaction.

## Revisit when

- A root-owned helper and branded polkit policy are packaged.
- Package rollback/downgrade, dependency repair or channels are specified.
