# 0013 - Updater runtime: check, download and stage

**Status:** Implemented (2026-11-18; full and delta staging and staged rollout; platform apply is
[spec 0014](0014-appimage-apply.md)/[0025](0025-platform-apply.md); package-manager apply and
helper-side apply are separate; Linux package elevation is [spec 0029](0029-linux-package-elevation.md))

Spec 0012 fixes the signed manifest. This spec fixes the first runtime half: deciding whether an
offer is acceptable, downloading it, verifying the bytes and putting a verified artifact in a
staging directory. Installation starts in [spec 0014](0014-appimage-apply.md) for AppImage and
[spec 0025](0025-platform-apply.md) for Windows/macOS.

## Scope

- `auro-update::Updater`: a pinned public key, the running semantic version, the host platform and a
  staging directory.
- `check`: manifest signature, format, platform and semantic-version comparison.
- `download`: HTTP(S) fetch of a newer artifact, with a bounded body and exact manifest size.
- `stage_from_path`: the same verification and staging path for an artifact already on disk.
- Content-addressed staging with safe extension preservation, atomic publication and reuse of a
  staged file that still verifies.

## Out of scope

- Installing, launching an installer, replacing the running app, restarting, and rollback (Linux
  AppImage apply/rollback is [spec 0014](0014-appimage-apply.md); Windows/macOS apply is
  [spec 0025](0025-platform-apply.md); package-manager artifacts remain separate).
- Channels and update UI.
- Background scheduling, retry policy, resume of an interrupted download and UI prompts.
- Key distribution, rotation and revocation inside the app.
- Hosting and CDN behavior.

## Decisions

### D1 - Verify the offer before moving bytes

`check` verifies the manifest signature and format, rejects a platform that is not this build, and
parses both versions as semantic versions. It returns one of three decisions:

| Decision | Meaning | `download` behavior |
|---|---|---|
| `UpToDate` | offered version equals the running version | refuses |
| `Available` | offered version is newer | downloads/stages |
| `Downgrade` | offered version is older | refuses |

A signed older manifest can be replayed, so downgrade is a decision for a caller that has an
explicit rollback policy; the default runtime never stages it. The explicit rollback entry points
are `stage_rollback_from_path`/`download_rollback` in
[spec 0030](0030-linux-package-rollback.md), which require a strictly older signed version. A
malformed version is a typed error, not a lexical comparison.

*Rejected:* comparing version strings. `1.10.0` sorts before `1.9.0` lexically. *Rejected:*
accepting a manifest for another platform and letting the installer fail later.

### D2 - Staging is content-addressed

The staged filename is the lowercase artifact SHA-256, plus the URL's extension when it is a short
ASCII-alphanumeric token. The hash is the identity: two manifests with different bytes cannot
collide, and the URL cannot inject a path. `Manifest::verify` now rejects a SHA-256 that is not 64
lowercase hexadecimal characters before any artifact or path work happens.

Existing staged bytes are reused only when the manifest signature, size and SHA-256 all verify.

*Rejected:* using the last URL path segment as the filename. It is attacker-controlled and can
contain traversal or platform-reserved names.

### D3 - Exact size, then hash, then atomic publish

The artifact is written to a temporary file in the staging directory. The reader is limited to the
manifest size; one additional byte is read to reject an oversized body. A short body is an
`ArtifactSize` error. The temporary file is synced, verified against the manifest's size and
SHA-256, then persisted at its content-addressed path. A failed download publishes nothing: the
temporary file is dropped, and a previously staged file is left untouched.

*Rejected:* downloading to the final path and verifying afterwards. A crash or a failed hash would
leave a plausible-looking but unusable file.

### D4 - HTTP client and limits

Downloads use `ureq` 3 with rustls and the webpki roots. The agent uses a 10 s connect timeout, a
300 s per-call timeout and a 30 s per-read timeout; the manifest size bounds the body. Redirects
use ureq's default limit. HTTP URLs are accepted because the signature is the trust boundary;
production channels should use HTTPS, while tests and local mirrors may use HTTP.

*Rejected:* adding a second HTTP stack (for example `reqwest`) for one GET. `ureq` is already in the
workspace through the bundler.

### D5 - Deltas and rollout are separate signed fields

Manifest v2 adds base-bound bsdiff-compatible deltas and deterministic percentage rollout. Full
staging remains the fallback. The runtime verifies the base, patch and resulting full artifact before
publishing it. Details are in [spec 0028](0028-deltas-rollout.md).

### D6 - Staging is not installation

`StagedUpdate` exposes the manifest and the verified path, and nothing else. Applying an update is
a separate contract because it differs per artifact (AppImage replacement, MSI/NSIS execution,
macOS app replacement) and needs a restart/rollback protocol. The Linux AppImage path is
[spec 0014](0014-appimage-apply.md); this slice still runs no installer and replaces no binary.

*Rejected:* a generic "run this command" install hook in the framework. It moves platform policy
into every app and makes rollback unverifiable.

## Interfaces

```rust
pub enum UpdateDecision {
    UpToDate,
    Available,
    Downgrade,
}

pub struct Updater { /* staging dir, key, current version, platform, HTTP agent */ }

impl Updater {
    pub fn new(
        staging_dir: impl Into<PathBuf>,
        key: PublicKey,
        current_version: &str,
    ) -> Result<Self, UpdateError>;
    pub fn with_platform(self, platform: impl Into<String>) -> Self;
    pub fn current_version(&self) -> &semver::Version;
    pub fn platform(&self) -> &str;
    pub fn check(&self, manifest: &Manifest) -> Result<UpdateDecision, UpdateError>;
    pub fn download(&self, manifest: &Manifest) -> Result<StagedUpdate, UpdateError>;
    pub fn stage_from_path(
        &self,
        manifest: &Manifest,
        artifact: &Path,
    ) -> Result<StagedUpdate, UpdateError>;
}

pub struct StagedUpdate { /* manifest, path */ }

impl StagedUpdate {
    pub fn manifest(&self) -> &Manifest;
    pub fn path(&self) -> &Path;
    pub fn into_parts(self) -> (Manifest, PathBuf);
}
```

Errors added to `UpdateError`: `InvalidField`, `InvalidVersion`, `PlatformMismatch`, `NotNewer`,
`ArtifactTooLarge` and `Http`. `Manifest::verify` also rejects a malformed SHA-256.

Usage:

```rust,ignore
let key = auro::update::PublicKey::from_base64(&app.public_key)?;
let updater = auro::update::Updater::new(
    state_dir.join("updates"),
    key,
    env!("CARGO_PKG_VERSION"),
)?;
let manifest = auro::update::Manifest::from_json(&body)?;
if updater.check(&manifest)? == auro::update::UpdateDecision::Available {
    let staged = updater.download(&manifest)?;
    // An install step consumes `staged` in a later slice.
}
```

The snippet documents the intended call shape; its surrounding app code does not exist yet. Enable
the facade's `updater` feature (or depend on `auro-update` directly) to use the module.

## Acceptance

1. Newer, equal and older signed manifests return `Available`, `UpToDate` and `Downgrade`; a
   different platform and an invalid version fail with typed errors
   (`classifies_versions_and_rejects_a_different_platform`,
   `rejects_invalid_versions_and_bad_current_version`).
2. A manifest with the wrong signature fails before any network request
   (`verifies_before_touching_the_network`).
3. A local HTTP server serves the artifact; `download` stages it under `<sha256>.<ext>`, the
   manifest verifies the staged file, and a second call reuses it without another request
   (`downloads_verifies_and_reuses_a_staged_artifact`).
4. A tampered body is rejected with `ArtifactHash`; a short body with `ArtifactSize`; an oversized
   body with `ArtifactTooLarge`; none leaves a final artifact
   (`rejects_tampered_short_and_oversized_downloads`).
5. An equal version is refused before the network (`refuses_to_stage_an_up_to_date_version`).
6. `stage_from_path` copies and verifies a local artifact (`stages_a_local_artifact`).
7. `Manifest::verify` rejects a SHA-256 that is not 64 lowercase hexadecimal characters
   (`rejects_a_malformed_artifact_hash`).
8. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Install/restart/rollback by platform | [AppImage apply](0014-appimage-apply.md) / [Windows and macOS apply](0025-platform-apply.md) / restart spec |
| Deltas and staged rollout selection | [Binary deltas and staged rollout](0028-deltas-rollout.md) |
| Persistent update journal and resume | [Update journal](0026-update-journal.md) / resume spec |
| Key rotation/revocation and pinning in the app | Trust/update client spec |
| Update channels, prompts and scheduling | Platform/update UX follow-up |
