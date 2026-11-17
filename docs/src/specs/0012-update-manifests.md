# 0012 - Update manifests

**Status:** Implemented (manifest format, signing, verification and CLI; runtime download/staging is
[spec 0013](0013-updater-runtime.md); Linux AppImage apply/rollback is
[spec 0014](0014-appimage-apply.md); other installers, deltas and rollout are not implemented)

Phase 2 needs a signed description of a release artifact. This spec fixes the manifest format and
its verification; the runtime is specified separately ([0013](0013-updater-runtime.md) covers
check/download/stage and [0014](0014-appimage-apply.md) the first apply path).

## Scope

- `auro-update`: manifest format v1, signing, verification, JSON encode/decode.
- Artifact metadata: size and lowercase hex SHA-256.
- `auro manifest --artifact ... --key ... --out ...` to produce a signed manifest.
- Platform keys (`<os>-<arch>`) and publication timestamps as caller-supplied text.

## Out of scope

- Downloading, staging, applying or rolling back in this spec (the runtime is
  [0013](0013-updater-runtime.md) and [0014](0014-appimage-apply.md)).
- Deltas, binary patches, staged rollout percentages, channels, rollback execution.
- Hosting and CDN behavior.
- Key distribution beyond the public key string; no built-in key pinning in this slice.
- UI for update prompts.

## Decisions

### D1 - Format v1 is a flat JSON object

```json
{
  "format": 1,
  "version": "1.2.3",
  "platform": "linux-x86_64",
  "url": "https://example.test/app-1.2.3.tar.gz",
  "size": 42,
  "sha256": "...",
  "notes": "...",
  "published_at": "2026-11-17T12:00:00Z",
  "signature": "<base64 Ed25519 signature>"
}
```

Unknown fields are rejected. `format` other than 1 is an unsupported-format error, not a guess.
`sha256` is exactly 64 lowercase hexadecimal characters; verification rejects anything else before
hashing an artifact.

### D2 - Signature covers a canonical payload

The payload is the same fields **without** `signature`, serialized with `serde_json` in the struct's
field order. The signature is Ed25519 over those bytes. A field reorder or rename is a format
change; bump `format` and migrate. `Manifest::verify` checks format, presence of a signature, then
the signature.

### D3 - Artifact metadata is verified separately

`Manifest::for_artifact` reads the local file, records its byte length and SHA-256.
`Manifest::verify_artifact` verifies the manifest signature first, then size and hash. A signed
manifest for a different file fails.

### D4 - CLI shape

```text
auro generate-key --out release.key
auro manifest \
  --artifact target/release/bundle/app.tar.gz \
  --version 1.2.3 \
  --url https://example.test/app.tar.gz \
  --key release.key \
  --out latest.json \
  --notes "..." \
  --published-at 2026-11-17T12:00:00Z \
  [--platform linux-x86_64]
```

The platform defaults to `<std::env::consts::OS>-<std::env::consts::ARCH>`. The command prints the
manifest path.

### D5 - Trust, not installation

The signature and hash prevent a tampered manifest/artifact from being accepted. [Spec
0013](0013-updater-runtime.md) downloads and stages a verified artifact; [spec
0014](0014-appimage-apply.md) replaces a Linux AppImage and can roll it back. Other apply paths
remain separate, and nothing restarts or replaces a running app automatically.

## Interfaces

```rust
pub struct Manifest {
    pub format: u32,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub notes: String,
    pub published_at: String,
    pub signature: String,
}

impl Manifest {
    pub fn new(...) -> Self;
    pub fn for_artifact(...) -> Result<Self, UpdateError>;
    pub fn sign(self, key: &SigningKey) -> Result<Self, UpdateError>;
    pub fn verify(&self, key: &PublicKey) -> Result<(), UpdateError>;
    pub fn verify_artifact(&self, path: &Path, key: &PublicKey) -> Result<(), UpdateError>;
    pub fn to_json(&self) -> Result<String, UpdateError>;
    pub fn from_json(text: &str) -> Result<Self, UpdateError>;
}
```

## Acceptance

1. Signed manifest verifies; tampering with a field fails (`signs_and_verifies_a_manifest`,
   `rejects_a_tampered_field`).
2. Empty signature, unknown format and malformed SHA-256 fail with the typed errors
   (`rejects_an_empty_signature_and_unknown_format`, `rejects_a_malformed_artifact_hash`).
3. Artifact size/hash and manifest signature are both checked; a changed artifact fails
   (`verifies_an_artifact_and_its_metadata`).
4. JSON round-trips byte-for-byte at the value level (`json_round_trips`).
5. The CLI creates a manifest that verifies and describes the artifact (`auro-cli` release test).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Download/apply/staging/restart and rollback | [Updater runtime](0013-updater-runtime.md) / apply spec |
| Deltas/patches and staged rollout percentages | Updater rollout spec |
| Key pinning and rotation inside the app | Trust/update client spec |
| Update channels and UI | Platform/update UX follow-up |
| Hosting/CSP and mirror behavior | Release service follow-up |
