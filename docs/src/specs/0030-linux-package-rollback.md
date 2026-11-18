# 0030 - Linux package rollback

**Status:** Implemented (2026-11-18; explicit caller-supplied rollback package staged through the
updater, plus an opt-in helper rollback when a package-manager apply fails; Auro never obtains a
previous package on its own)

Spec 0027 applies a staged `.deb`/`.rpm` and spec 0029 elevates that path. A failed or unwanted
package update still needs an exit. This spec adds an explicit rollback operation for an older
package that the caller already has; it does not invent, download or derive one.

## Scope

- `Updater::stage_rollback_from_path` and `Updater::download_rollback` for a signed, strictly older
  manifest and artifact.
- `LinuxPackageInstaller::rollback(&StagedUpdate)`.
- `PackageRollback` and `ApplyPlan::with_package_rollback` for an opt-in automatic rollback.
- Helper-side rollback when a `.deb`/`.rpm` apply fails with `InstallerFailed`.
- `dpkg --force-downgrade -i` for a `.deb`.
- `rpm -U --oldpackage` for an `.rpm`.
- Reuse of `LinuxElevation::Pkexec` and the held-descriptor command path.
- Typed `InstallerFailed`/`ElevationFailed` on failure.

## Out of scope

- Automatic rollback without a caller-supplied older package.
- Downloading, discovering, repacking or retaining the previous package.
- Deciding whether a downgrade is safe for dependencies or the system.
- Package holds, pinning, channels and downgrade approval policy.

## Decisions

### D1 - The caller supplies the older package

`rollback` takes the same `StagedUpdate` contract as `install`. The caller must authenticate,
download and stage the older package; the manifest/artifact verification that produced it is the
caller's responsibility. Auro does not query the package manager for a cached package or guess a
version.

The staging side has an explicit rollback path, because the normal updater refuses to stage an
older version:

```rust
impl Updater {
    pub fn stage_rollback_from_path(
        &self,
        manifest: &Manifest,
        artifact: &Path,
    ) -> Result<StagedUpdate, UpdateError>;
    pub fn download_rollback(&self, manifest: &Manifest) -> Result<StagedUpdate, UpdateError>;
}
```

Both verify the manifest signature, format, platform and artifact size/SHA-256 exactly like
[spec 0013](0013-updater-runtime.md), then require `offered < running`. Equal or newer offers are
`NotOlder`. This keeps a signed older manifest from being replayed through the normal update path
while still allowing a deliberate rollback.

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

The rollback path uses the same `LinuxElevation::Pkexec` held-descriptor command as install.
`pkexec` codes `126`/`127` (authorization/execution) are `ElevationFailed`; a package-manager
failure is `InstallerFailed` ([spec 0029](0029-linux-package-elevation.md)). The operation never
relaunches the app itself.

### D4 - Automatic rollback is opt-in and only follows a package-manager failure

`ApplyPlan::with_package_rollback` attaches an already-staged older package to a
`DebPackage`/`RpmPackage` plan. The journal stores the older artifact, its signed manifest and its
pinned key. If the primary apply fails with `InstallerFailed`, the helper:

1. re-verifies the older manifest, artifact size/SHA-256 and host platform;
2. requires the older manifest version to be strictly lower than the journal version;
3. runs the elevated `rollback` path (`dpkg --force-downgrade`/`rpm -U --oldpackage`);
4. marks the journal `RolledBack` and relaunches the old app without overwriting that state.

It does not roll back on `ElevationFailed` (the user declined or `pkexec` failed), a missing or
tampered artifact, a platform mismatch, or any non-package apply. Those are not evidence that the
package manager changed the installation. A rollback failure leaves the journal `Failed` with both
the apply and rollback diagnostics.

The caller still chooses the target version and supplies the package. Auro does not keep a
package-manager cache, synthesize a `.deb`/`.rpm` from installed files, or decide that a downgrade
is dependency-safe. `RollbackVersionNotOlder` rejects a same/newer "rollback"
([ADR 0021](../decisions/0021-opt-in-package-rollback.md)).

## Interfaces

```rust
impl LinuxPackageInstaller {
    pub fn rollback(&self, staged: &StagedUpdate) -> Result<LinuxPackageKind, UpdateError>;
}

pub struct PackageRollback {
    pub artifact: PathBuf,
    pub manifest: Manifest,
    pub public_key: PublicKey,
}

impl ApplyPlan {
    pub fn with_package_rollback(self, rollback: PackageRollback) -> Self;
}

impl ApplyJournal {
    pub fn rollback_artifact(&self) -> Option<&Path>;
}
```

The method uses the same `LinuxElevation` policy as `install`:

```rust,ignore
let older_staged = updater.stage_rollback_from_path(&older_manifest, &older_package)?;
let installer = auro::update::LinuxPackageInstaller::new().elevated();
let kind = installer.rollback(&older_staged)?;
```

The snippet is an intended call shape; obtaining and authorizing `older_manifest` and
`older_package` is app/release policy. For an automatic retry after a package-manager failure,
attach the same staged package to the plan:

```rust,ignore
let plan = ApplyPlan::new(
    ApplyKind::DebPackage,
    staged.path(),
    None,
    manifest,
    updater.public_key(),
)
.with_package_rollback(PackageRollback::new(
    older_staged.path(),
    older_manifest,
    updater.public_key(),
));
```

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
6. `stage_rollback_from_path` stages a signed older artifact, reuses a staged file that still
   verifies, and rejects equal/newer offers with `NotOlder`
   (`stages_a_signed_older_artifact_for_rollback`,
   `rollback_staging_rejects_newer_equal_and_tampered_offers`).
7. `download_rollback` stages a signed older artifact and refuses a newer one before the network
   (`downloads_a_signed_rollback_artifact`, `refuses_a_rollback_download_before_the_network`).
8. A journal round-trips the rollback artifact and rejects a rollback plan for a non-package apply
   (`round_trips_a_package_rollback_plan`, `rejects_a_rollback_plan_for_a_non_package_apply`).
9. Helper rollback verification refuses a same/newer version, a wrong platform and a tampered
   artifact (`rollback_verification_requires_an_older_verified_package`), and an elevated
   package-manager failure is `InstallerFailed` so the auto-rollback branch can see it
   (`elevated_package_manager_failure_is_an_installer_failure`).
10. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Automatic rollback with a package obtained by Auro | Package transaction policy; Auro cannot obtain a previous package |
| Automatic rollback of AppImage/MSI/`.app` applies | Platform installer receipts (specs 0014/0025) |
| Retaining or downloading the previous package | Release artifact retention |
| Dependency-safe downgrade and holds | Package-manager policy |
| Journal-driven rollback execution | Apply journal follow-up |
