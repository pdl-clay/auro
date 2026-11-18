# 0031 - macOS package apply

**Status:** Implemented in tree (2026-11-18; `.pkg` command execution, explicit `osascript` elevation
and helper integration). macOS runtime execution and notarization are not verified on this host.

Spec 0025 applies a `.app` bundle from a staged `.zip`. This spec adds the macOS installer-package
path: a staged `.pkg` is run through `/usr/sbin/installer`, directly or through the standard macOS
administrator prompt.

## Scope

- `MacPkgInstaller`: detect `.pkg`, run `/usr/sbin/installer -pkg <staged> -target /`.
- `MacPkgElevation::{None, Osascript}`.
- `ApplyKind::MacPkg` for helper-side apply.
- Facade exports on macOS.

## Out of scope

- `.dmg` mounting and copying.
- Code-signing, notarization or Gatekeeper policy; the package is already the signed release
  artifact and the manifest hash is verified before apply.
- A root-owned LaunchDaemon/helper, `sudo`, or a custom Authorization Services prompt.
- Automatic rollback, downgrade, package receipts or uninstall policy.
- Per-user installs, custom targets, MDM/configuration profiles and installer UI.
- Verifying Apple package signatures independently of the signed manifest hash.

## Decisions

### D1 - Run the system installer shell-free

|Mode|Program|Arguments|
|---|---|---|
|Direct|`/usr/sbin/installer`|`-pkg <staged> -target /`|
|Elevated|`/usr/bin/osascript`|fixed `run` script plus the staged path as its argument|

The direct path uses `InstallerRunner` and an argument vector. No shell string is built in Auro
code. `installer` is the supported macOS package tool; `open`/Installer.app is a UI path, not a
silent apply primitive.

### D2 - Elevation is an explicit AppleScript prompt

The elevated path runs a fixed AppleScript:

```applescript
on run argv
  do shell script "/usr/sbin/installer -pkg " & quoted form of item 1 of argv & " -target /" with administrator privileges
end run
```

The package path travels as `argv`, and `quoted form` performs shell quoting; Auro never
interpolates an untrusted path into script source. The app process remains the user; only
`installer` runs as root after the standard macOS authorization prompt.

**Rejected:** `sudo`. It assumes a terminal/credential cache and does not provide the desktop
authorization sheet. **Rejected:** making the app root. **Rejected:** a custom privileged helper
before its installation and signing story exists.

The path is still reopened by the privileged installer after the journal's verification, so a
same-user artifact replacement between verification and apply remains a residual risk until a
root-owned helper owns the staged copy. The helper limits that window and records failure.

### D3 - Failure is explicit and never relaunches

Direct `installer` exit `0` succeeds; another code is `InstallerFailed`. For the elevated path, a
non-zero `osascript` result is `ElevationFailed` (AppleScript does not expose the child installer's
exit code through `do shell script`); the journal records `Failed` and the helper does not relaunch.
The macOS installer owns its package transaction; Auro does not roll back or repair dependencies.

A missing staged artifact is `StagedArtifactMissing`; an extension other than `.pkg` is
`UnsupportedInstaller`.

## Interfaces

```rust
pub enum MacPkgElevation { None, Osascript }

pub struct MacPkgInstaller { /* runner, elevation */ }

impl MacPkgInstaller {
    pub fn new() -> Self;                         // direct
    pub fn elevated(self) -> Self;                // osascript administrator prompt
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self;
    pub fn with_elevation(
        runner: impl InstallerRunner + 'static,
        elevation: MacPkgElevation,
    ) -> Self;
    pub fn install(&self, staged: &StagedUpdate) -> Result<(), UpdateError>;
}
```

`ApplyKind` gains `MacPkg`. The type is re-exported on macOS and compiled under `cfg(test)` for
command construction tests on other hosts.

Usage:

```rust,ignore
let installer = auro::update::MacPkgInstaller::new().elevated();
installer.install(&staged)?;
```

The snippet is an intended call shape, not an app that exists yet.

## Acceptance

1. Direct `.pkg` runs `/usr/sbin/installer -pkg <staged> -target /`
   (`runs_the_macos_installer_for_a_pkg`).
2. Elevated `.pkg` runs `osascript` with the fixed `quoted form` script and the path as its argument
   (`elevated_pkg_uses_osascript_and_quoted_form`).
3. A direct installer failure is `InstallerFailed` (`reports_pkg_installer_failure`).
4. A prompt/authorization failure is `ElevationFailed`
   (`reports_osascript_authorization_failure`).
5. An explicit `None` elevation policy runs the direct installer
   (`explicit_none_elevation_runs_the_direct_installer`).
6. A non-`.pkg` artifact is `UnsupportedInstaller` (`rejects_unknown_artifacts`).
7. `ApplyKind::MacPkg` dispatches through the elevated installer on macOS.
8. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

|Question|Where it settles|
|---|---|
|macOS runtime verification and signed-package policy|macOS release CI/spec|
|Notarization and Gatekeeper assessment|[macOS notarization](0032-macos-notarization.md)|
|Root-owned privileged helper and artifact-copy ownership|macOS hardening follow-up|
|Automatic rollback/uninstall policy|Package transaction policy|
