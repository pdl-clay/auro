# 0027 - Linux package apply

**Status:** Implemented (2026-11-18; `.deb`/`.rpm` command execution and helper integration;
pkexec elevation is [spec 0029](0029-linux-package-elevation.md) and caller-supplied rollback,
including the opt-in helper rollback, is
[spec 0030](0030-linux-package-rollback.md); selecting a downgrade target remains out of scope)

Spec 0014 handles a single-file AppImage. This spec covers distribution packages: run `dpkg -i` for
a staged `.deb`, or `rpm -U` for a staged `.rpm`, through the same shell-free runner used by the
Windows installer.

## Scope

- `auro-update::LinuxPackageInstaller`: detect `.deb`/`.rpm`, run the package manager and classify
  its exit code.
- `ApplyKind::DebPackage`/`ApplyKind::RpmPackage` for helper-side apply.
- Facade exports on Linux.

## Out of scope

- A root-owned helper, `sudo`, polkit policy installation or an Auro-managed password prompt.
- Choosing or retaining the package to downgrade to; the caller supplies it (spec 0030).
- Signature verification of the package itself; the staged artifact is already verified by the
  unsigned manifest hash.
- Choosing between `dpkg`, `apt`, `rpm`, `dnf` or `zypper`; this uses the low-level package tool.
- Package-manager UI, progress reporting, restart scheduling and dependency repair.

## Decisions

### D1 - Absolute package-manager paths, shell-free

The staged extension selects the command:

| Format | Program | Arguments |
|---|---|---|
| `.deb` | `/usr/bin/dpkg` | `-i <artifact>` |
| `.rpm` | `/usr/bin/rpm` | `-U <artifact>` |

`InstallerRunner` receives an argument vector and uses `std::process::Command` directly. The
absolute paths avoid a PATH hijack; no shell or command string is built. Exit `0` succeeds and any
other code is `InstallerFailed`.

*Rejected:* invoking a package manager through a shell. It adds quoting and path-injection risk.
*Rejected:* auto-detecting APT/DNF/Zypper. The low-level tool is deterministic; dependency policy
belongs to a later package-manager spec.

### D2 - Elevation is a separate, explicit policy

`LinuxPackageInstaller::new()` runs as the current user; a package manager that needs root fails
with `InstallerFailed`. The restart helper uses the explicit `pkexec` policy in
[spec 0029](0029-linux-package-elevation.md) for `DebPackage`/`RpmPackage`. The app itself never
self-elevates, and `sudo` is not used.

### D3 - The helper reuses the platform apply layer

A prepared journal can carry `ApplyKind::DebPackage` or `ApplyKind::RpmPackage`; the helper applies
the staged package through `LinuxPackageInstaller` before relaunching. No backup is recorded because
the package manager owns the transaction. If the plan also carries a `PackageRollback` and the apply
fails with `InstallerFailed`, the helper runs the caller-supplied older package as
[spec 0030](0030-linux-package-rollback.md) describes. Otherwise it records `Failed` and does not
relaunch.

## Interfaces

```rust
pub enum LinuxPackageKind { Deb, Rpm }

pub enum LinuxElevation { None, Pkexec }

pub struct LinuxPackageInstaller { /* runner, elevation */ }

impl LinuxPackageInstaller {
    pub fn new() -> Self;                         // direct
    pub fn elevated(self) -> Self;                // pkexec, spec 0029
    pub fn with_runner(runner: impl InstallerRunner + 'static) -> Self;
    pub fn install(
        &self,
        staged: &StagedUpdate,
    ) -> Result<LinuxPackageKind, UpdateError>;
}
```

`ApplyKind` gains `DebPackage` and `RpmPackage`. Errors reuse `UnsupportedInstaller`,
`StagedArtifactMissing` and `InstallerFailed`.

Usage:

```rust,ignore
let kind = auro::update::LinuxPackageKind::from_path(staged.path()).unwrap();
let journal = auro::update::ApplyJournal::create_for_apply(
    state_dir.join("apply.json"),
    manifest.version.clone(),
    auro::update::ApplyPlan::new(
        auro::update::ApplyKind::DebPackage,
        staged.path().to_path_buf(),
        None,
        manifest.clone(),
        updater.public_key(),
    ),
    auro::update::Relaunch::current()?,
)?;
journal.spawn_helper()?;
cx.quit();
```

The snippet is an intended call shape, not an app that exists yet.

## Acceptance

1. `.deb` runs `/usr/bin/dpkg -i <staged>` (`runs_dpkg_for_a_deb`).
2. `.rpm` runs `/usr/bin/rpm -U <staged>` (`runs_rpm_for_an_rpm`).
3. A non-zero package-manager code is `InstallerFailed` (`reports_package_manager_failure`).
   Elevated failures are `ElevationFailed` (`reports_pkexec_authorization_failure`).
4. An unknown extension is `UnsupportedInstaller` (`rejects_unknown_artifacts`).
5. `ApplyKind::DebPackage`/`RpmPackage` dispatch through the elevated helper on Linux.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, `cargo test -p auro-services --features tray --locked`,
   and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Root-owned helper and narrowed polkit policy | [Package elevation](0029-linux-package-elevation.md) |
| Automatic package rollback and downgrade policy | [Package rollback](0030-linux-package-rollback.md) |
| apt/dnf/zypper policy and dependency repair | Package-manager spec |
| Package signature verification | Release/package trust spec |
| Restart scheduling after package install | Apply/restart follow-up |
