# 0037 - GitHub Releases update channel

**Status:** Implemented (2026-11-24; prerelease opt-in and private state dirs added 2026-11-25;
Linux AppImage channel verified end to end against Auri with `0.1.0-alpha.15`)

Specs 0012-0015 define signed manifests, staging, platform apply and restart, but leave discovery to
the app. This spec fixes the first served channel: a public GitHub Releases endpoint that hands a
signed manifest to the existing updater. Linux AppImage is the first apply target.

## Scope

- An opt-in `[updater]` section in `Auro.toml`: `provider`, `repository`, `public-key`,
  `manifest-asset`, `interval-secs`, `check-on-start` and `include-prereleases` (spec 0002).
- `auro_update::GithubChannel`: resolve the stable latest release or, when opted in, a prerelease
  from the release list; fetch one named signed manifest asset, check it with `Updater`,
  download/stage the artifact, prepare or start the existing apply/restart journal, and record the
  last check time.
- A documented release recipe using the existing `auro generate-key`, `auro manifest` and
  `auro delta` commands plus `gh release upload` (specs 0011, 0012, 0028).
- Bounded manifest reads, a strict HTTPS GitHub asset host allowlist, and typed errors for a
  malformed API response or a missing asset.
- Delta staging when the caller supplies the currently installed artifact as a base.

## Out of scope

- An update UI, a background scheduler, retry policy, download resume and channel selection.
- Key rotation, revocation, manifest expiry, private repositories and authenticated API requests.
- Windows, macOS and package-manager apply; those installers keep their own contracts.
- A release-side upload command; the recipe uses existing CLI commands and `gh`.
- Trust in the GitHub API response. The pinned Ed25519 signature remains the only release
  authenticity decision (ADR 0012, ADR 0026).

## Decisions

### D1 - GitHub Releases is transport, not trust

The channel reads `GET /repos/{owner}/{repo}/releases/latest` by default. When
`include-prereleases` is set, it reads `GET /repos/{owner}/{repo}/releases` and selects the first
non-draft release. It never treats the release tag, asset list or API response as authenticity. The
asset bytes are parsed as a v2 manifest and verified by `Updater::check` with the pinned public key
before any artifact request. A tampered or wrong-key manifest fails before the artifact URL is used.

*Rejected:* trusting the release tag or an unsigned `latest.json` index.

### D2 - The app pins one key in config; no keys are fetched

`[updater].public-key` is the base64 Ed25519 public key printed by `auro generate-key`. No key
endpoint, rotation or revocation is contacted. `GithubChannel::new` parses it once and fails with a
typed error for a malformed key.

*Rejected:* fetching a key from the same release, which would make the signer self-certifying.

### D3 - Strict repository, asset URL and body limits

`repository` is exactly `owner/repo` with GitHub-safe name characters; it is interpolated into an
API path with no slash or userinfo possible. A manifest asset's `browser_download_url` must be
HTTPS on `github.com`, `api.github.com` or `objects.githubusercontent.com`, with no userinfo, port
or `..` path segment. Manifest bodies are capped at 1 MiB and a missing named asset is a typed
error. Artifact bytes keep the exact size and SHA-256 bound from spec 0013.

*Rejected:* accepting any URL from an unauthenticated release response as a fetch target.

### D4 - The app owns the clock and the install target

`interval-secs` (default `3600`) and `check-on-start` (default `true`) drive `due()`. A successful
due check records a Unix-seconds timestamp in the state directory. `apply` takes the AppImage path
at runtime, because an installed AppImage's path is host-specific (often `$APPIMAGE`), not a build
time config value. The state directory must not be derived from the working directory: AppImage
changes cwd to its read-only mount. `GithubChannel::new` creates the state directory and its staging
subdirectory with mode `0700` on Unix, then the existing journal trust checks apply. After `apply`
starts the helper the app must exit so the helper's lock is released.

*Rejected:* a framework timer on the GPUI main thread, a config field naming a machine-specific
absolute path, and writing update state under an AppImage's read-only cwd.

### D5 - One verification/staging/apply path

The channel is a thin adapter. It calls `Updater::check`, `Updater::download` (or
`Updater::download_delta` with a supplied base), `ApplyJournal::create_for_apply` and
`ApplyJournal::spawn_helper`. It adds no second HTTP stack, no alternate signature check and no
alternate installer.

*Rejected:* a channel-specific installer or a copy of the staging code.

### D6 - No GitHub credentials in the app

Only public releases are supported: no `Authorization` header and no token config key. The release
operator's token stays in the release recipe's `gh` invocation.

*Rejected:* a config token that would be shipped to end users.

### D7 - Stable by default, prereleases opt-in

`/releases/latest` deliberately ignores prereleases and drafts. The default channel therefore
tracks stable releases only. `include-prereleases = true` switches to `/releases` and selects the
first non-draft entry, including a prerelease; drafts are still skipped. This keeps a public alpha
app from silently jumping to a draft while allowing an explicit pre-release track.

*Rejected:* always using `/releases`, which would make an unlabelled first entry override a stable
release.

## Interfaces

```rust
pub const DEFAULT_MANIFEST_ASSET: &str = "auro-update.json";

pub struct ChannelConfig { /* repository, public key, asset name, interval, check-on-start, prereleases */ }

impl ChannelConfig {
    pub fn new(repository: &str, public_key_base64: &str) -> Self;
    pub fn with_manifest_asset(self, name: &str) -> Self;
    pub fn with_interval_secs(self, secs: u64) -> Self;
    pub fn with_check_on_start(self, enabled: bool) -> Self;
    pub fn with_prereleases(self, enabled: bool) -> Self;
}

pub struct GithubChannel { /* settings, Updater, HTTP agent, state paths */ }

impl GithubChannel {
    pub fn new(
        config: ChannelConfig,
        state_dir: impl Into<PathBuf>,
        current_version: &str,
    ) -> Result<Self, UpdateError>;
    pub fn settings(&self) -> &ChannelConfig;
    pub fn manifest(&self) -> Result<Manifest, UpdateError>;
    pub fn check(&self) -> Result<UpdateDecision, UpdateError>;
    pub fn due(&self) -> Result<bool, UpdateError>;
    pub fn record_check(&self) -> Result<(), UpdateError>;
    pub fn check_if_due(&self) -> Result<Option<UpdateDecision>, UpdateError>;
    pub fn stage(&self, base: Option<&Path>) -> Result<Option<StagedUpdate>, UpdateError>;
    pub fn prepare(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<Option<ApplyJournal>, UpdateError>;
    pub fn prepare_if_due(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<Option<ApplyJournal>, UpdateError>;
    pub fn apply(&self, target: impl Into<PathBuf>, base: Option<&Path>)
        -> Result<bool, UpdateError>;
    pub fn apply_if_due(
        &self,
        target: impl Into<PathBuf>,
        base: Option<&Path>,
    ) -> Result<bool, UpdateError>;
}
```

New `UpdateError` variants cover a malformed release response, a missing named asset, an unsafe
asset URL and an oversized manifest. The facade re-exports the channel under the existing `updater`
feature; the config section is always parsed so release-side tools can read it without that feature.

A minimal app flow:

```rust,ignore
let config = Config::load(&std::env::current_dir()?)?;
let state = state_dir("my-app")?; // $XDG_STATE_HOME/my-app, never cwd under AppImage
let Some(settings) = config.updater.as_ref() else { return };
let channel = GithubChannel::new(
    ChannelConfig::new(&settings.repository, &settings.public_key)
        .with_manifest_asset(&settings.manifest_asset)
        .with_interval_secs(settings.interval_secs)
        .with_check_on_start(settings.check_on_start)
        .with_prereleases(settings.include_prereleases),
    state,
    &config.app.version,
)?;
let target = std::env::var_os("APPIMAGE")
    .map(PathBuf::from)
    .unwrap_or(std::env::current_exe()?);
if channel.apply_if_due(target, None)? {
    return; // exit so the helper can take the restart lock
}
```

The runnable version lives in `examples/update-channel/`.

## Release recipe

1. `auro generate-key --out release.key` and pin the printed public key in `Auro.toml`.
2. Build the AppImage and upload it to a public GitHub Release. Copy its browser download URL.
3. `auro manifest --artifact App.AppImage --version <version> --url <download-url> --key release.key
   --out auro-update.json --published-at <RFC3339>`.
4. Optional delta: `auro delta --from old.AppImage --to App.AppImage --out patch.bin`, upload
   `patch.bin`, then repeat step 3 with `--delta-from old.AppImage --delta-from-version <old>
   --delta-url <patch-url> --delta-out patch.bin`.
5. `gh release upload <tag> auro-update.json [--clobber]` (and `patch.bin` when used).
6. Publish the release. The default app fetches `auro-update.json` via `/releases/latest`; set
   `include-prereleases = true` to track a published prerelease instead.

The full recipe, including the platform key and version rules, lives in
`../cookbook/github-release-updates.md`.

## Acceptance

1. A fake GitHub API serves a release JSON with one `auro-update.json` asset; `check` returns
   `Available`, `UpToDate` and `HeldBack` for newer, equal and rollout-gated signed manifests, and
   `stage` leaves a verified content-addressed artifact (`resolves_and_stages_a_signed_release`).
2. A manifest signed by another key is rejected before the artifact request
   (`rejects_a_foreign_key_before_an_artifact_request`).
3. A missing asset, malformed release JSON, non-GitHub or HTTP asset URL and a body over 1 MiB
   each return a typed error and stage nothing (`rejects_bad_release_metadata_and_oversized_manifest`).
4. `due()` honours `interval-secs` and `check-on-start`; `record_check` writes a timestamp that
   survives a new channel instance (`honours_the_check_interval_and_records_the_last_check`).
5. `prepare` writes a `Prepared` AppImage journal with the signed manifest, pinned key and
   relaunch command; `apply` adds `ApplyJournal::spawn_helper`, whose handshake has its own tests
   in spec 0026 (`prepares_an_appimage_apply`).
6. `stage` with a base uses the signed delta when present and falls back to a full download when
   it is not (`stages_a_delta_or_the_full_artifact`).
7. Config parses the documented keys, applies defaults and rejects invalid provider, repository,
   key, asset name and interval (`updater_section_parses_and_validates`).
8. `/releases/latest` is used by default; `include-prereleases = true` selects the first non-draft
   entry from `/releases` (`prereleases_are_opt_in`).
9. `GithubChannel::new` creates state and staging directories with no group/other permission bits
   on Unix (`creates_private_state_directories`).
10. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
    `cargo test --workspace --locked`, `mdbook build docs` and
    `cargo check --manifest-path examples/update-channel/Cargo.toml` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Key rotation, revocation and metadata expiry | [RFC 0001](../../rfcs/0001-update-metadata-trust.md) |
| Update UI, background scheduling and retry/resume | A later UX/transport spec |
| Private releases and token handling | A later ADR; never a token in `Auro.toml` |
| Windows/macOS channels | Reuse this adapter with their platform apply contracts |
