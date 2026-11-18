# 0019 - macOS package elevation uses an explicit AppleScript prompt

**Status:** Accepted (2026-11-18)

## Context

A `.pkg` needs `/usr/sbin/installer` and, for a system install, administrator rights. The app
process must not become root. macOS offers no stable, simple direct elevation API: `sudo` assumes a
terminal, Authorization Services is deprecated, and a signed privileged helper is a separate
installation and trust project. The standard desktop authorization sheet is exposed through
AppleScript's `do shell script ... with administrator privileges`.

## Decision

`MacPkgInstaller::elevated()` runs `/usr/bin/osascript` with a fixed `run` handler. The staged path
is passed as an argument and `quoted form` quotes it for the one shell command; no untrusted path is
inserted into script source. The app remains the user and the installer runs as root only after the
system prompt. Direct `installer` execution remains available as `MacPkgElevation::None`.

The macOS installer owns its package transaction. Auro records failure, does not relaunch, and
does not roll back or repair dependencies. Notarization and Gatekeeper policy remain separate.

## Consequences

**Easier**

- A signed `.pkg` can be applied with the standard macOS authorization sheet.
- Command construction is testable with `InstallerRunner` on any host.
- No app-wide root execution, `sudo` credential cache, or custom helper is required for this slice.

**Harder**

- `osascript` and a GUI authorization session are runtime dependencies.
- AppleScript hides the installer child's exit code; failures are classified as elevation failures.
- The privileged installer reopens the staged path; a root-owned helper is still required for a
  stronger same-user threat model.
- Notarization and package-signature policy need their own release work.

**Forbidden**

- Re-executing the app as root or calling `setuid`.
- Building a shell command by interpolating an unquoted path.
- Using `sudo` as a silent or terminal-dependent fallback.
- Claiming notarization or automatic rollback from this apply path.

## Revisit when

- A signed privileged helper and its installation story exist.
- Notarization/Gatekeeper policy is specified.
- Package rollback or a non-system install target is required.
