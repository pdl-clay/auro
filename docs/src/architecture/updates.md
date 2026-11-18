# Updates

Status: the pieces below exist in tree and are unit-tested, but no update has been served, signed
with a release key, installed on a packaged app and verified end to end on all three platforms.
Apple credentials and a macOS runtime are unavailable here.

The contract lives in the specs: [0012](../specs/0012-update-manifests.md) (manifest),
[0013](../specs/0013-updater-runtime.md) (check/stage), [0014](../specs/0014-appimage-apply.md),
[0025](../specs/0025-platform-apply.md), [0026](../specs/0026-update-journal.md),
[0027](../specs/0027-linux-packages.md), [0029](../specs/0029-linux-package-elevation.md),
[0030](../specs/0030-linux-package-rollback.md), [0031](../specs/0031-macos-package-apply.md) and
[0032](../specs/0032-macos-notarization.md).

## Trust model

One Ed25519 key pair per app. The app pins the public key; the release operator signs a JSON
manifest whose signature covers version, platform, URL, size, SHA-256, notes, publication time and
the optional delta/rollout fields. The manifest signature is the trust boundary, not TLS and not
the package manager. A signed older manifest can be replayed, so the normal updater refuses a
downgrade; an explicit rollback path exists for a caller that chooses an older release.

## Staging

`Updater` checks the manifest before any network request, downloads with timeouts and an exact
size bound, verifies SHA-256, and publishes the artifact atomically under its content-addressed
name in a staging directory. A staged file that still verifies is reused. `stage_from_path` is the
same path for an artifact already on disk. Deltas are base-bound and produce a full artifact that
is verified again; a staged rollout is a deterministic percentage keyed by a stable per-install
value. Full download is always the fallback.

## Apply

| Artifact | Path | Backup/rollback |
|---|---|---|
| AppImage | Atomic replacement, mode 0755 | Hard-link/copy backup, explicit receipt |
| `.deb` | `dpkg -i` through `pkexec` | Package manager transaction; opt-in older-package rollback |
| `.rpm` | `rpm -U` through `pkexec` | Package manager transaction; opt-in older-package rollback |
| MSI/NSIS | Silent installer, `3010` means reboot | Installer policy |
| macOS `.app` zip | Extract, swap one bundle | Retained bundle backup, explicit receipt |
| macOS `.pkg` | `/usr/sbin/installer`, AppleScript authorization | Installer policy |

The restart helper runs the apply, not the app. Elevation is explicit and helper-side: `pkexec`
for Linux package managers, the standard AppleScript administrator prompt for `.pkg`. The app
process never becomes root. `pkexec` codes 126/127 are authorization/execution failures; a package
manager's own non-zero code is an installer failure.

## Journal and restart

The app writes an atomically replaced JSON journal (`Prepared`), keeps a lock file and spawns the
same executable with `AURO_UPDATE_RESTART_HELPER`. The helper waits for the old process to release
the lock, re-verifies the manifest and artifact, applies the platform action, marks `Applied` (or
`RebootRequired`), and relaunches. Failures are `Failed` with a diagnostic and no relaunch.
The app classifies the next start with `reconcile` and cleans up with `finalize` after `Complete`.

The journal is user-writable local state, but its path is a trust boundary: it names the
verification key, staged artifact, target and relaunch command, so a journal planted by another
local user would choose what the helper runs. `ApplyJournal::load`, `save` and the helper lock open
refuse a journal or lock in a directory another user can write: Unix checks owner, mode, symlinks
and ancestors; Windows rejects reparse points and checks the owner/DACL (compile-verified,
runtime-unverified). See [spec 0033](../specs/0033-update-journal-trust.md). The helper still
re-verifies the signed manifest and artifact; path trust does not replace signature verification.

## Rollback

- AppImage and macOS `.app` installers return a receipt; the old artifact stays beside the target
  until the app discards it.
- A Linux package rollback needs an older `.deb`/`.rpm`. `Updater::stage_rollback_from_path` or
  `download_rollback` verifies and stages one; `LinuxPackageInstaller::rollback` runs the explicit
  downgrade flags. An `ApplyPlan` may carry that older package, in which case the helper retries it
  once after an `InstallerFailed`, marks `RolledBack` and relaunches. A declined authorization, a
  tampered artifact or a non-package apply never triggers that retry
  ([ADR 0021](../decisions/0021-opt-in-package-rollback.md)).
- Auro does not discover, retain or synthesize the older package, and does not decide whether a
  downgrade is dependency-safe. The package manager owns that transaction.

## Notarization and signing

Signing inputs are mapped to the platform bundler. Notarization is separate release hardening:
`auro notarize` submits a `.dmg`/`.pkg`/`.app` with an existing keychain profile, staples and
validates the ticket, and `--verify` runs the matching Gatekeeper assessment. It does not replace
the Auro manifest signature. See [macOS notarization](../specs/0032-macos-notarization.md).

## Not implemented

- a served update channel, metadata expiry/generation, release key rotation and revocation
  ([RFC 0001](../../../rfcs/0001-update-metadata-trust.md));
- a real notarization submission or a macOS `.pkg` apply on a macOS host;
- automatic retention of a previous package;
- an update UI, scheduling, channels or resume of an interrupted download;
- a public comparison or pinned CI baseline for update size/time.
