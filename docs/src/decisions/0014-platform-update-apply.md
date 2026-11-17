# 0014 - Platform updates use the native installer or bundle swap

**Status:** Accepted (2026-11-17)

## Context

The updater now has a verified staged artifact ([spec 0013](../specs/0013-updater-runtime.md)).
Linux AppImages are replaced in place ([ADR 0013](0013-appimage-updates.md)). Windows and macOS
have different artifact shapes and privilege models:

1. Run a generic command supplied by the app. Rejected: it makes platform policy invisible, cannot
   classify exit codes, and gives the framework no safe test seam.
2. Add a privileged helper/restart protocol immediately. Deferred: it is needed for a full update
   UX, but it is a separate protocol and not required to prove the apply step.
3. Run the native Windows MSI/NSIS installer silently, and swap a macOS `.app` from a staged zip.
   Chosen for the next apply layer.
4. Mount macOS `.dmg` images or delegate to a package manager. Deferred: both need more platform
   setup and privilege handling.

## Decision

Windows apply runs the staged MSI through `msiexec /i <artifact> /qn /norestart`, or the staged NSIS
installer with `/S`, using an injected, shell-free `InstallerRunner`. Exit `0` succeeds; `3010` and
`1641` succeed with `restart_required`; every other code is `InstallerFailed`.

macOS apply accepts a staged `.zip` with exactly one top-level `.app`, extracts it with the zip
path sanitizer, rejects symlinks that resolve outside the extraction tree, renames the old bundle
to a hidden backup beside the target, renames the new bundle into place, and returns a receipt that
can roll back or discard the backup. No installer waits for the app to exit, restarts it or requests
elevation.

## Consequences

**Easier**

- Windows command construction and exit-code policy are explicit and unit-testable through a fake
  runner.
- macOS replacement is filesystem-local, keeps a recoverable old bundle, and is testable on Linux.
- There is still no generic "run this command" framework hook.

**Harder**

- Windows installers may fail under UAC or because the app is running; the caller must surface that.
- macOS `.dmg`/`.pkg`, code-signing checks, package-manager updates and restart orchestration remain
  separate work.
- A crash between the two macOS renames leaves a backup and no target; a persistent apply journal is
  still needed.

**Forbidden**

- Claiming an update is applied when the installer failed or the bundle was not swapped.
- Building a shell command from artifact paths.
- Automatically restarting a running app from these installers.

## Revisit when

- A helper/restart protocol or a persistent apply journal is specified.
- `.dmg`/`.pkg`, `.deb`/`.rpm` or delta/rollout apply changes the artifact shape.
- A Windows/macOS release needs privilege escalation or code-signing verification before install.
