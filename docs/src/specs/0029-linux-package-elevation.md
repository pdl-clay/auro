# 0029 - Linux package elevation

**Status:** Implemented (2026-11-18; helper-side `pkexec` package apply, explicit opt-in)

Spec 0027 runs `dpkg -i`/`rpm -U` shell-free but leaves the process unprivileged. This spec adds one
elevation path for the restart helper: authenticate through polkit and let the package manager run as
root. The app process never becomes root.

## Scope

- `LinuxElevation`: `None` (direct) or `Pkexec` (polkit).
- `LinuxPackageInstaller::elevated()` and `with_elevation`.
- Helper-side package apply through `pkexec /usr/bin/dpkg -i ...` or
  `pkexec /usr/bin/rpm -U ...`.
- Passing the opened staged artifact through the helper's `/proc/<pid>/fd/<fd>`.
- Typed `UpdateError::ElevationFailed` for a non-zero `pkexec` result.

## Out of scope

- A root-owned helper binary, setuid installation, or a system polkit policy file.
- `sudo`, password prompts implemented by Auro, or interactive terminal policy.
- Package rollback, downgrade, dependency repair, or a package-manager preference policy.
- Package signature verification independent of the signed manifest hash.
- Progress UI, polkit agent installation, or headless/SSH authentication.

## Decisions

### D1 - Elevation is explicit and helper-side

`LinuxPackageInstaller::new()` keeps the unprivileged behavior from spec 0027. `.elevated()` selects
`LinuxElevation::Pkexec`; the restart helper uses that path for `ApplyKind::DebPackage` and
`ApplyKind::RpmPackage`. The app never calls `setuid`, re-executes itself as root, or wraps the
package manager in a shell.

**Rejected:** making the whole app root. It gives package-manager and maintainer-script code the
app's environment and UI lifetime. **Rejected:** automatic `sudo`. It can block on a terminal that a
GUI app does not have.

### D2 - `pkexec` gets an argument vector, not a command string

The elevated command is:

```text
/usr/bin/pkexec /usr/bin/dpkg -i /proc/<helper-pid>/fd/<fd>
/usr/bin/pkexec /usr/bin/rpm -U /proc/<helper-pid>/fd/<fd>
```

All program paths are absolute. The package path is a held, read-only descriptor opened by the
helper immediately before the call; the helper keeps the `File` alive until `pkexec` returns, so a
path replacement cannot silently redirect the package. The journal verifies the manifest, platform
and artifact before this point.

**Rejected:** `/bin/sh -c "pkexec dpkg -i $path"`. It adds quoting and injection risk for no gain.
**Rejected:** `sudo`. It is not guaranteed to exist, may require a TTY, and has different policy
semantics from the desktop polkit agent.

### D3 - Failure is explicit; the package manager owns the transaction

`pkexec` exit `0` succeeds. Any other code is `ElevationFailed { program, code }`; `pkexec` commonly
uses `126` for a denied/cancelled authorization and `127` for a missing command. A missing
`/usr/bin/pkexec` itself surfaces as an I/O error. The helper records `Failed` in the journal and
does not relaunch the app.

A package manager transaction is not rolled back by Auro. `dpkg`/`rpm` own their database and may
leave a package installed even when a later trigger fails; automatic downgrade would be less safe
than the package manager's own recovery. `Target` is not used for packages.

### D4 - Polkit policy remains the deployment boundary

The command relies on the standard `org.freedesktop.policykit.exec` action and a running polkit
authentication agent. A distro that wants a narrower, branded prompt should install a policy for a
dedicated root-owned helper; that is deliberately a future change. A headless session without an
agent fails closed with `ElevationFailed`.

## Interfaces

```rust
pub enum LinuxElevation { None, Pkexec }

pub struct LinuxPackageInstaller { /* runner, elevation */ }

impl LinuxPackageInstaller {
    pub fn new() -> Self;                         // direct, no elevation
    pub fn elevated(self) -> Self;                // polkit through pkexec
    pub fn with_elevation(
        runner: impl InstallerRunner + 'static,
        elevation: LinuxElevation,
    ) -> Self;
    pub fn install(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError>;
}
```

`UpdateError` adds `ElevationFailed { program: PathBuf, code: Option<i32> }`.

## Acceptance

1. Direct `.deb` still runs `/usr/bin/dpkg -i <staged>` (`runs_dpkg_for_a_deb`).
2. Elevated `.deb` runs `pkexec` with `/usr/bin/dpkg`, `-i`, and a `/proc/<pid>/fd/<fd>` path
   (`elevated_deb_runs_pkexec_dpkg_with_a_held_descriptor`).
3. Elevated `.rpm` runs `pkexec` with `/usr/bin/rpm -U` (`elevated_rpm_runs_pkexec_rpm`).
4. A `pkexec` exit of 126 is `ElevationFailed` (`reports_pkexec_authorization_failure`).
5. The restart helper routes `ApplyKind::DebPackage`/`RpmPackage` through the elevated installer.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Root-owned helper and a branded polkit policy | Packaging/distro follow-up |
| Package rollback and downgrade policy | [Package rollback](0030-linux-package-rollback.md) |
| apt/dnf/zypper policy and dependency repair | Package-manager spec |
| Headless/SSH authorization | Operations follow-up |
