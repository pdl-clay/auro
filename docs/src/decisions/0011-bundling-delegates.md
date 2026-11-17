# 0011 - Bundling delegates to `tauri-bundler`

**Status:** Accepted (2026-11-17)

## Context

Phase 2 must produce `.app`/`.dmg`, MSI/NSIS, AppImage/deb/rpm. Writing and maintaining a bundler
means WiX, NSIS, macOS bundles, codesign/notarization plumbing, Debian/RPM metadata, AppImage
tooling and platform edge cases: months of work that is not the product. The roadmap already says
Auro keeps Tauri's packaging model; the only question is which implementation.

Options:

1. Write `auro-bundle` from scratch. Rejected: enormous platform surface, not differentiating.
2. Shell out to each platform's tools. Rejected: every consumer would get a different, untested
   path; metadata and error handling would be ad hoc.
3. Wrap [`tauri-bundler`](https://crates.io/crates/tauri-bundler) behind `auro-bundle`. Chosen.
4. Use `cargo-bundle`. Rejected: less coverage of Windows installers and Linux packaging.

`tauri-bundler` 2.10.5 is Apache-2.0/MIT, the same dual-license family as Auro, and the roadmap
already commits to wrapping it. Its own documentation warns that minor releases may change the
public API.

## Decision

`auro-bundle` pins `tauri-bundler = "=2.10.5"` and maps the validated `Auro.toml` `[bundle]`
section onto its `Settings`.

- The crate owns the config-to-bundler mapping, host package-type resolution and the typed
  `AuroError::Bundle`.
- `auro bundle` builds the release binary and delegates artifact creation to the bundler; it does
  not reimplement packaging.
- Host formats only. `bundle.targets = "all"` resolves to the host set; cross-compilation is not a
  supported Phase 2 workflow.
- Signing, notarization, updater manifests and deltas are **not** claimed by this decision; later
  specs own those contracts.
- Upgrading `tauri-bundler` is a deliberate change that must pass the three-OS CI, because a minor
  release may change the API.
- Auro does not patch or fork the bundler. If a blocking bug appears, the vendoring rule and a
  follow-up ADR apply.

## Consequences

**Easier**

- Auro gains platform packaging without owning WiX, NSIS, DMG, AppImage, deb or rpm internals.
- The app model and the packaging metadata stay one source of truth (`Auro.toml`).
- The license chain remains permissive.

**Harder**

- The dependency can change its API on a minor bump; upgrades are deliberate and CI-verified.
- Auro inherits the bundler's platform limitations, tool downloads and cache behavior.
- Signing/notarization/updater integration still needs explicit contracts and tests.
- Debugging a packaging failure can cross into a third-party crate; the error is surfaced as
  `AuroError::Bundle`, not hidden.

**Forbidden**

- Presenting a third-party bundler warning or failure as success.
- Claiming signed/notarized artifacts before the signing spec is implemented and tested.
- Adding a second packaging implementation without an RFC/ADR that supersedes this one.

## Revisit when

- `tauri-bundler` stops supporting a platform Auro targets, or its API churn becomes unmanageable.
- Auro needs a format the bundler cannot produce (Snap/Flatpak/APK), which needs a separate
  decision.
- Signing/notarization requirements outgrow what the bundler exposes.
