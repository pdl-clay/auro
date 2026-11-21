# 0010 - Bundling

**Status:** Implemented (Linux, Windows and macOS verified 2026-11-17)

Phase 2 needs installable artifacts. Auro does not write a bundler: `auro-bundle` maps the
`[bundle]` section of `Auro.toml` onto `tauri-bundler` 2.10 and delegates the platform formats to it
([ADR 0011](../decisions/0011-bundling-delegates.md)).

## Scope

- The `auro-bundle` crate: config-to-bundler mapping and artifact paths.
- The `auro bundle` CLI command: release build, then bundle.
- Host formats: macOS `app`/`dmg`, Windows `msi`/`nsis`, Linux `appimage`/`deb`/`rpm`.
- Mapping of identity, descriptions, license, category, icons and resources.
- Typed errors (`AuroError::Bundle`).

## Out of scope

- Code signing, notarization and the signed updater (later Phase 2 specs).
- Cross-compilation as a supported workflow; the bundler warns and is not exercised by CI.
- Custom installer UI, installer hooks and file associations.
- Generating icon assets beyond the scaffold's placeholder; apps provide their own through
  `[bundle].icon`.
- Package formats outside the config schema (APK, Snap, Flatpak, …).

## Decisions

### D1 - Delegate to `tauri-bundler`, pinned exactly

`tauri-bundler = "=2.10.5"` is pinned in the workspace. Its own documentation warns that minor
releases may change the API; an upgrade is a deliberate change that must run the three-OS CI. The
bundler crate is an implementation detail of `auro-bundle`, not Auro's public API.

### D2 - The config is the only input

`auro-bundle` reads the already-validated `Config`. It does not read `Cargo.toml` package metadata
except through the binary name the CLI supplies. Mapping:

| `Auro.toml` | `tauri-bundler` |
|---|---|
| `app.identifier` | `BundleSettings::identifier` |
| `app.product-name` | `PackageSettings::product_name` |
| `app.version` | `PackageSettings::version` |
| `app.description` | `PackageSettings::description` (product name when empty) |
| `app.authors` / `app.homepage` | `PackageSettings` fields; homepage also in bundle settings |
| `app.license` | `BundleSettings::license` |
| `bundle.category` | `BundleSettings::category` (`AppCategory`) |
| `bundle.short-description` / `long-description` | bundle settings |
| `bundle.copyright` / `license-file` | bundle settings |
| `bundle.icon` / `resources` | absolute paths derived from the app directory |
| `bundle.targets` | `PackageType` list; `"all"` uses the host set |

An unknown category or package format is `AuroError::Bundle`, not a bundler panic.

### D3 - `auro bundle` builds first

`auro bundle` loads and version-checks the config, rejects a config with
`[bundle] active = false` (`AuroError::Bundle`; bundling is opt-out, not a silent no-op), runs
`cargo build --release`, resolves the executable name through `cargo metadata` (`default-run`, else
the sole enabled binary target, including auto-discovered `src/bin/*.rs`; targets whose
`required-features` are disabled are ignored, feature flags (`--features`, `-F`, `--all-features`,
`--no-default-features`, including `package/feature` names) select the enabled set like Cargo, an
explicit `--bin` in `[dev].build-args` wins, and several enabled binaries without `default-run` are
an error naming only those candidates), then calls
`auro_bundle::bundle_in(dir, &config, &binary, &target)`. The target root is
`CARGO_TARGET_DIR` resolved like cargo (relative values under the invocation directory), or
`<app>/target`; the bundler reads `<target>/release` and artifacts land under
`<target>/release/bundle/<format>/`. Every produced path is printed, one per line; an empty
artifact list is a typed `AuroError::Bundle`, not a successful no-op.

The app directory is canonicalised, so bundler paths are stable regardless of where the command is
run.

### D4 - Host formats only

Phase 2 supports the host's formats. `bundle.targets` is already validated per host in spec 0002;
`"all"` resolves to the host set. Cross-compilation is not a supported CLI workflow and is not a
claim; the bundler's experimental warning is not suppressed.

### D5 - Signing is not claimed yet

`auro bundle` does not configure signing identities, certificates, notarization or an updater.
`tauri-bundler` may sign when the host environment provides credentials; Auro does not promise it.
The signing spec and the updater spec own those contracts.

## Interfaces

```rust,ignore
pub fn host_targets() -> &'static [&'static str];
pub fn package_types(config: &Config) -> Result<Vec<PackageType>, AuroError>;
pub fn bundle(dir: &Path, config: &Config, binary: &str) -> Result<Vec<PathBuf>, AuroError>;
```

```text
auro bundle [--dir <app>]
```

`AuroError::Bundle { message }` has kind `"bundle"`.

## Acceptance

1. Explicit targets map to the requested `PackageType`s; `"all"` maps to the host set
   (`auro-bundle` unit tests).
2. The manifest metadata reaches the bundler settings: identifier, icons, binary name, category,
   output directory, and package types (`maps_the_manifest_metadata`).
3. An unknown category is a typed bundle error, not a panic (`rejects_an_unknown_category`).
4. `binary_name` resolves from `cargo metadata`, including auto-discovered `src/bin` targets, honors
   `required-features` and `[dev].build-args` (including `--bin`), and rejects several enabled
   candidates without `default-run` (`auro-cli` tests).
5. `auro bundle` on a real scaffolded app builds release and produces the host's requested
   artifacts, printing their paths. Recorded with host, formats and environment.
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Status (2026-11-17)

Linux verified on x86_64 with `tauri-bundler 2.10.5`. A fixture app (plain release binary,
`targets = ["deb", "rpm", "appimage"]`, a 512x512 PNG icon) produced:

```
target/release/bundle/deb/Bundle Demo_0.1.0_amd64.deb
target/release/bundle/rpm/Bundle Demo-0.1.0-1.x86_64.rpm
target/release/bundle/appimage/Bundle Demo_0.1.0_amd64.AppImage
```

`auro bundle` printed each path. A missing/non-square AppImage icon is rejected before the bundler
runs, because `tauri-bundler` panics in that case. macOS `.app`/`.dmg` are wired through the same
mapping but have not been produced on that host in this environment; the three-OS CI is the place
to verify them.

Windows 11 x86_64 (WinBoat guest, rustc/cargo 1.99.0 MSVC, `tauri-bundler 2.10.5`) verified the
same mapping with `targets = ["msi", "nsis"]` and a 256x256 `.ico`:

```
target\release\bundle\msi\Bundle Demo_0.1.0_x64_en-US.msi          (0.61 MB)
target\release\bundle\nsis\Bundle Demo_0.1.0_x64-setup.exe         (0.17 MB)
```

The MSI installed silently, ran `bundle-demo.exe` (`bundle demo on windows`), and uninstalled
cleanly; the NSIS setup installed per-user, ran the same binary, and uninstalled. The NSIS
uninstaller finishes asynchronously, so the install directory disappears a few seconds after the
uninstaller returns.
Windows installers set `webview_install_mode = Skip` because Auro's app layer has no webview;
otherwise the Tauri template tries to download/install WebView2. Two Windows-only path issues were
fixed in the same change: the bundler call now runs with the app directory as cwd (icons/resources
resolve relative to it) and `canonicalize`'s `\\?\` verbatim prefix is stripped before WiX/NSIS
tools see it.

macOS verification (unsigned, no local Mac): the [`bundle-smoke`](https://github.com/pdl-clay/auro/blob/main/.github/workflows/bundle-smoke.yml)
workflow ran on a GitHub `macos-latest` arm64 runner. A fixture app with `targets = ["app", "dmg"]`
and a 512x512 PNG icon produced:

```
target/release/bundle/macos/Bundle Smoke.app
target/release/bundle/dmg/Bundle Smoke_0.1.0_aarch64.dmg
```

The workflow ran the app binary and accepted the DMG with `hdiutil imageinfo`. No signing identity
is configured, so the artifacts are unsigned; signing/notarization belong to the signing spec.

## Open questions

| Question | Where it settles |
|---|---|
| Codesign/notarization and Windows signing | [macOS notarization](0032-macos-notarization.md) and signing spec |
| Package-manager apply and restart/rollback orchestration | [Linux packages](0027-linux-packages.md), [elevation](0029-linux-package-elevation.md); deltas/rollout are [spec 0028](0028-deltas-rollout.md) |
| Icon placeholders in the scaffold | Bundling follow-up; apps may supply icons today |
| Cross-compilation | Not planned for 1.0; revisit if demand appears |
| AppImage tool downloads and offline builds | Document the bundler's cache/tooling behavior |
