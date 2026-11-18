# 0030 - Linux package rollback

**Status:** Implemented (2026-11-18; explicit caller-supplied rollback package; no automatic
downgrade policy)

Spec 0027 applies a staged `.deb`/`.rpm` and spec 0029 elevates that path. A failed or unwanted
package update still needs an exit. This spec adds an explicit rollback operation for an older
package that the caller already has; it does not invent, download or derive one.

## Scope

- `LinuxPackageInstaller::rollback(&StagedUpdate)`.
- `dpkg --force-downgrade -i` for a `.deb`.
- `rpm -U --oldpackage` for an `.rpm`.
- Reuse of `LinuxElevation::Pkexec` and the held-descriptor command path.
- Typed `InstallerFailed`/`ElevationFailed` on failure.

## Out of scope

- Automatic rollback after a failed install or trigger.
- Downloading, discovering, repacking or retaining the previous package.
- Deciding whether a downgrade is safe for dependencies or the system.
- Package holds, pinning, channels and downgrade approval policy.
- A journal-side rollback transaction. `ApplyJournal::mark_rolled_back` remains caller-driven.

## Decisions

### D1 - The caller supplies the older package

`rollback` takes the same `StagedUpdate` contract as `install`. The caller must authenticate,
download and stage the older package; the manifest/artifact verification that produced it is the
caller's responsibility (normally the same updater path used for any release). Auro does not query
the package manager for a cached package or guess a version.

**Rejected:** selecting a previous version from the package manager cache. The cache is not an
update contract, may contain untrusted local state, and differs between dpkg and rpm.
**Rejected:** using the network inside the installer. Download policy and signature verification
belong to the updater, not to the privileged apply step.

### D2 - Use the package manager's explicit downgrade flag

|Format|Program|Arguments|
|---|---|---|
|`.deb`|`/usr/bin/dpkg`|`--force-downgrade -i <artifact>`|
|`.rpm`|`/usr/bin/rpm`|`-U --oldpackage <artifact>`|

The flags are explicit so a rollback is not silently interpreted as a normal upgrade. `dpkg`'s
manual warns that it does not perform dependency checking on downgrades; that risk belongs to the
caller and the package manager, not to an implicit Auro policy.

*Rejected:* `dpkg -i` without the downgrade flag. It makes the operation depend on the installed
dpkg version's default and obscures intent. *Rejected:* removing and reinstalling a package; that
loses configuration and is not a rollback.

### D3 - Elevation and failure semantics are unchanged

The rollback path uses the same `LinuxElevation::Pkexec` held-descriptor command as install. A
non-zero `pkexec` result is `ElevationFailed`; a package-manager failure is `InstallerFailed`. The
operation never relaunches the app itself. If the caller uses `ApplyJournal`, it can call
`mark_rolled_back` after a successful rollback, but no automatic failure hook exists.

## Interfaces

```rust
impl LinuxPackageInstaller {
    pub fn rollback(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError>;
}
```

The method uses the same `LinuxElevation` policy as `install`:

```rust,ignore
let installer = auro::update::LinuxPackageInstaller::new().elevated();
let kind = installer.rollback(&older_staged)?;
```

The snippet is an intended call shape; obtaining `older_staged` is app/release policy.

## Acceptance

1. `.rpm` rollback runs `/usr/bin/rpm -U --oldpackage <staged>`
   (`rollback_rpm_uses_oldpackage`).
2. `.deb` rollback runs `/usr/bin/dpkg --force-downgrade -i <staged>`
   (`rollback_deb_forces_a_downgrade`).
3. Elevated rollback passes the held `/proc/<pid>/fd/<fd>` path through `pkexec`
   (`elevated_rollback_uses_the_held_descriptor`).
4. A package-manager failure is `InstallerFailed` with the exit code
   (`rollback_reports_package_manager_failure`).
5. A missing staged artifact or unknown extension follows the install contract
   (`StagedArtifactMissing`/`UnsupportedInstaller`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Automatic rollback after a failed trigger | Package transaction policy |
| Retaining or downloading the previous package | Release artifact retention |
| Dependency-safe downgrade and holds | Package-manager policy |
| Journal-driven rollback execution | Apply journal follow-up |
