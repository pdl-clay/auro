# 0027 - Linux package apply

**Status:** Implemented (2026-11-18; `.deb`/`.rpm` command execution and helper integration;
elevation, rollback and package-manager policy are not implemented)

Spec 0014 handles a single-file AppImage. This spec covers distribution packages: run `dpkg -i` for
a staged `.deb`, or `rpm -U` for a staged `.rpm`, through the same shell-free runner used by the
Windows installer.

## Scope

- `auro-update::LinuxPackageInstaller`: detect `.deb`/`.rpm`, run the package manager and classify
  its exit code.
- `ApplyKind::DebPackage`/`ApplyKind::RpmPackage` for helper-side apply.
- Facade exports on Linux.

## Out of scope

- Elevation, `sudo`, `pkexec`, polkit or a privileged helper.
- Rollback or package downgrade policy; the package manager owns the transaction.
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

### D2 - No implicit elevation

The helper and app run as the current user. A package manager that needs root will fail with a
non-zero code and a typed `InstallerFailed`; Auro does not invoke `sudo`, `pkexec` or a privileged
helper. The app can surface the failure or choose a different install path.

### D3 - The helper reuses the platform apply layer

A prepared journal can carry `ApplyKind::DebPackage` or `ApplyKind::RpmPackage`; the helper applies
the staged package through `LinuxPackageInstaller` before relaunching. No backup is recorded because
the package manager owns rollback. The helper still records `Failed` and does not relaunch on
failure.

## Interfaces

```rust
pub enum LinuxPackageKind { Deb, Rpm }

pub struct LinuxPackageInstaller { /* runner */ }

impl LinuxPackageInstaller {
    pub fn new() -> Self;
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
4. An unknown extension is `UnsupportedInstaller` (`rejects_unknown_artifacts`).
5. `ApplyKind::DebPackage`/`RpmPackage` dispatch through the helper on Linux.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, `cargo test -p auro-services --features tray --locked`,
   and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Elevation/polkit/privileged helper | Platform package UX spec |
| Package rollback and downgrade policy | Package updater spec |
| apt/dnf/zypper policy and dependency repair | Package-manager spec |
| Package signature verification | Release/package trust spec |
| Restart scheduling after package install | Apply/restart follow-up |
