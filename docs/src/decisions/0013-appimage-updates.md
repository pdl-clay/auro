# 0013 - AppImage updates use in-place replacement with a backup receipt

**Status:** Accepted (2026-11-17)

## Context

The updater stages a signed, verified artifact ([spec 0013](../specs/0013-updater-runtime.md)).
Applying it differs per distribution: Linux AppImages are one executable file, Windows uses MSI or
NSIS installers, macOS bundles need bundle-level replacement, and `.deb`/`.rpm` updates belong to a
package manager.

Options for the first apply path:

1. A generic "run this command" install hook. Rejected: it makes every app responsible for
   platform policy and gives the framework no way to verify rollback.
2. A helper process that always waits for the app to exit. Deferred: it is needed for Windows and
   macOS, but adds a protocol before the single-file Linux case is proven.
3. In-place replacement of a single-file AppImage, with a retained backup. Chosen for Linux.
4. Package-manager integration for `.deb`/`.rpm`. Out of scope: it needs elevated privileges and
   distro-specific behavior.

## Decision

On Linux, `AppImageInstaller` replaces an AppImage atomically: keep the old file as a hard-link
backup (copy fallback), write the staged artifact to a temporary file in the target directory, set
mode `0755`, sync, and rename it over the target. It returns an `InstallReceipt`; the caller must
either `discard` the backup after verifying the new version or `rollback` it. Dropping the receipt
does not delete the backup.

Compiled only on Linux. Other platforms and package formats get separate apply contracts; there is
no generic installer-command hook. Installing does not restart the app.

## Consequences

**Easier**

- A running AppImage can be updated safely; the old process keeps its inode.
- Rollback is a single rename and does not depend on the release server still serving old bytes.
- The failure mode is conservative: an unhandled receipt leaves the old file recoverable.

**Harder**

- The app must manage receipt lifecycle and disk space for backups.
- A crash before the caller discards leaves a hidden backup; there is no automatic cleanup.
- Owner, group, xattrs and SELinux labels are not preserved beyond the copied bytes and mode.
- Windows, macOS and package-manager updates still need their own design.

**Forbidden**

- Running an installer from a generic framework hook.
- Claiming an AppImage update is installed when the target was not replaced or verified.
- Deleting the backup before the new version has been accepted.

## Revisit when

- Windows/macOS apply or a helper/restart protocol is specified.
- A package-manager or delta update path changes the artifact shape.
- A persistent receipt or rollback-on-next-launch policy is needed.
