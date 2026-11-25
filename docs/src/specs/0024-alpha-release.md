# 0024 - Alpha release and crates.io

**Status:** Implemented (2026-11-18 for `v0.1.0-alpha.1`; 2026-11-19 for `v0.1.0-alpha.2`;
2026-11-20 for `v0.1.0-alpha.3` and `v0.1.0-alpha.4`; 2026-11-21 for `v0.1.0-alpha.5`,
`v0.1.0-alpha.6`, `v0.1.0-alpha.7`, `v0.1.0-alpha.8`, `v0.1.0-alpha.9`, `v0.1.0-alpha.10`,
`v0.1.0-alpha.11`, `v0.1.0-alpha.12`, `v0.1.0-alpha.13` and `v0.1.0-alpha.14`). All twelve crates
are published; final screenshots remain follow-ups.

The first public alpha publishes the workspace crates as `0.1.0-alpha.1`. This spec collects the
release contract that is not owned by an earlier feature spec: versioning, publish order, crate
metadata, facade features, and the scaffold's dependency mode.

## Scope

- One version for every crate: `0.1.0-alpha.1`.
- Publish the eleven workspace crates in dependency order.
- Per-crate README, homepage, keywords/categories and licence files.
- `publish = ["crates-io"]` only in the release commit.
- Facade features: services are opt-in; tray is separately opt-in.
- `auro new` emits a path dependency inside this checkout and a version dependency when the CLI is
  installed from crates.io.

## Out of scope

- Changing the public API after the alpha except for critical fixes.
- AccessKit/IME; updater deltas/rollout now have their own spec 0028.
- Signed/notarized release artifacts; those remain credential-gated.
- A website, announcement, or support policy beyond the existing docs.

## Decisions

### D1 - Alpha version

The first public version is `0.1.0-alpha.1` for every crate. The workspace version and every
internal dependency version move together; a workspace-local skew is a bug. `0.0.0` is a build
placeholder and is never published. `0.1.0-alpha.2` follows the same rule for the second alpha.

### D2 - Publish leaves first

crates.io resolves dependencies from the registry, not from path dependencies. Publication order:

1. `auro-macros`, `auro-ipc`, `auro-sign`, `auro-perms`, `auro-services`
2. `auro-core`
3. `auro-update`
4. `auro-ui`
5. `auro-bundle`
6. `auro`
7. `auro-cli`

The first five fit the new-crate burst; the remaining six are spaced by the crates.io limit of one
new crate per ten minutes. Each crate is dry-run and verified before the next.

### D3 - Services are opt-in at the facade

`auro-services` keeps its OS integrations separate. The facade features are:

```toml
[features]
services = ["dep:auro-services"]
tray = ["services", "auro-services/tray"]
```

`services` is off by default. `tray` is off by default even when services are enabled, because it
adds a D-Bus status-notifier backend and platform event handlers to apps that do not need them.
Users who need a tray enable `tray` explicitly.

### D4 - Scaffold emits the right dependency

`auro new` cannot know at build time whether it runs from this checkout or from crates.io. It emits
a path dependency when `../crates/auro` exists relative to the CLI manifest, and otherwise emits
the CLI's own `CARGO_PKG_VERSION` string. The in-repo path keeps development against the checkout;
the registry version keeps an installed CLI's generated app buildable.

### D5 - Metadata is release material

Every crate carries a description, licence, repository, homepage, README, keywords and categories.
The README is inside the crate root so Cargo includes it in the tarball. Licence files are included
or referenced from the package. `cargo package --list` and `cargo publish --dry-run` are the
verification steps.

### D6 - Publication is intentional

`publish = false` stays in the workspace until the release commit. The release commit changes it to
`["crates-io"]` and carries the version bump in the same change. Publishing is never part of an
ordinary feature PR.

## Acceptance

1. `cargo metadata` reports `0.1.0-alpha.1` for every crate and no path-only internal dependency
   lacking a registry version.
2. `cargo package -p <leaf>` succeeds for each leaf crate, and `cargo publish --dry-run -p <leaf>`
   succeeds after the release switch.
3. `cargo tree -p auro -e normal` contains no `tray-icon` in the default feature set;
   `--features services` adds the service crates; `--features tray` adds `tray-icon`. No feature
   set pulls GTK or `libappindicator`.
4. `cargo test -p auro-cli` covers the in-repo path and installed-version scaffold modes.
5. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.
6. The release commit is tagged and all crates are published in the order above, with the
   rate-limit waits recorded.

## Progress (2026-11-17)

- Workspace version and all internal dependency requirements are `0.1.0-alpha.1`; `auro --version`
  reports it.
- `auro` features `services` and `tray` are in place; default `cargo tree -p auro` contains no
  `tray-icon`, and no feature set pulls GTK or `libappindicator`.
- `auro new` emits the workspace path in this checkout and the CLI's version when installed from a
  registry; both modes are tested.
- Every crate has a crate-root README, homepage, keywords and categories; `cargo package` includes
  the README.
- `auro-macros`, `auro-ipc`, `auro-sign`, `auro-perms` and `auro-services` package and verify.
  `auro-perms` completed `cargo publish --dry-run` with the release switch locally enabled.
- Release switch, tag and ordered upload completed; see the publication record below.

### Publication (2026-11-18)

- Tag `v0.1.0-alpha.1` points at `ac4d996` (the merge of the release-preparation changes).
- All eleven crates report `0.1.0-alpha.1` as their maximum published version on crates.io:
  `auro-macros`, `auro-ipc`, `auro-sign`, `auro-perms`, `auro-services`, `auro-core`,
  `auro-update`, `auro-ui`, `auro-bundle`, `auro`, `auro-cli`.
- The crates.io listing is the publication evidence. The rate-limit waits were not retained as a
  repository log, so acceptance item 6 is satisfied for the tag and versions, not for a retained
  wait log.

### Publication (2026-11-19) - `0.1.0-alpha.2`

- Workspace version and internal requirements are `0.1.0-alpha.2`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the StatusNotifierItem tray backend (no GTK), the typed Wayland startup
  failure (`AuroError::Platform`) and the Windows journal ancestor validation.
- Tag `v0.1.0-alpha.2` points at the release commit.

### Publication (2026-11-20) - `0.1.0-alpha.3`

- Workspace version and internal requirements are `0.1.0-alpha.3`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the twenty fixes from #28-#47 and the same-app Tauri/Electron benchmark
  harness.
- Tag `v0.1.0-alpha.3` points at the release commit.

### Publication (2026-11-20) - `0.1.0-alpha.4`

- Workspace version and internal requirements are `0.1.0-alpha.4`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the eight follow-up fixes from #49-#56.
- Tag `v0.1.0-alpha.4` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.5`

- Workspace version and internal requirements are `0.1.0-alpha.5`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the five follow-up fixes from #58-#62.
- Tag `v0.1.0-alpha.5` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.6`

- Workspace version and internal requirements are `0.1.0-alpha.6`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the two follow-up fixes from #64-#65.
- Tag `v0.1.0-alpha.6` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.7`

- Workspace version and internal requirements are `0.1.0-alpha.7`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the two follow-up fixes from #67-#68.
- Tag `v0.1.0-alpha.7` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.8`

- Workspace version and internal requirements are `0.1.0-alpha.8`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the twenty fixes from #70-#79 and #81-#90.
- Tag `v0.1.0-alpha.8` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.9`

- Workspace version and internal requirements are `0.1.0-alpha.9`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the six security fixes from #92-#97.
- Tag `v0.1.0-alpha.9` points at the release commit.

### Publication (2026-11-21) - `0.1.0-alpha.10`

- Workspace version and internal requirements are `0.1.0-alpha.10`; all eleven crates report it as
  their maximum published version on crates.io.
- The release carries the four residue fixes from #99-#102.
- Tag `v0.1.0-alpha.10` points at the release commit.

### Publication (2026-11-22) - `0.1.0-alpha.11`

- Workspace version and internal requirements are `0.1.0-alpha.11`; all twelve crates report it as
  their maximum published version on crates.io.
- The release adds the GPUI asset source API (`App::assets`, `FileAssets`, `EmbeddedAssets`) and the
  accepted plugin host stages 1-2 behind the default-off `plugins` feature (specs 0034/0035).
- Tag `v0.1.0-alpha.11` points at the release commit.

### Publication (2026-11-22) - `0.1.0-alpha.12`

- Workspace version and internal requirements are `0.1.0-alpha.12`; all twelve crates report it as
  their maximum published version on crates.io.
- The release closes the `FileAssets` check/open race with fd-verified reads and carries the stage 3
  plugin host hardening (specs 0035/0036, ADR 0025).
- Tag `v0.1.0-alpha.12` points at the release commit.

### Publication (2026-11-22) - `0.1.0-alpha.13`

- Workspace version and internal requirements are `0.1.0-alpha.13`; all twelve crates report it as
  their maximum published version on crates.io.
- The release closes the hot reload GA readiness plan: the reloadable-state author contract,
  failure-path and stale-code regressions, a real dx patch e2e test, the D6 measurement pipeline
  with published per-OS evidence, and clean dev-session exit semantics. Hot reload GA uses per-OS
  targets (Linux < 500 ms, Windows and macOS < 1 s) with a labelled snapshot-restart fallback; no
  platform claims 500 ms without its own measurement.
- Tag `v0.1.0-alpha.13` points at the release commit.

### Publication (2026-11-24) - `0.1.0-alpha.14`

- Workspace version and internal requirements are `0.1.0-alpha.14`; all twelve crates report it as
  their maximum published version on crates.io.
- The release adds the opt-in GitHub Releases update channel: `[updater]` pins the repository and
  public key, and `GithubChannel` resolves a signed manifest, stages the artifact and hands off to
  the AppImage apply journal and restart helper (spec 0037, ADR 0026). Only public releases and
  Linux AppImage apply are in this slice.
- Tag `v0.1.0-alpha.14` points at the release commit.

## Open questions

| Question | Where it settles |
|---|---|
| Final README copy and screenshots | Release follow-up |
| Whether `auro-perms` is also feature-gated | Alpha follow-up |
| Whether `auro` keeps a default service feature | Alpha follow-up |
| Long-term release automation/trusted publishing | Release engineering follow-up |
