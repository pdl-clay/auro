# 0012 - Update manifests

**Status:** Implemented (manifest format, signing, verification and CLI; runtime apply, deltas,
rollout and rollback are not implemented)

Phase 2 needs a signed description of a release artifact. This spec fixes the manifest format and
its verification; it deliberately stops before the updater runtime (download, stage, apply,
rollback), which is a separate contract.

## Scope

- `auro-update`: manifest format v1, signing, verification, JSON encode/decode.
- Artifact metadata: size and lowercase hex SHA-256.
- `auro manifest --artifact ... --key ... --out ...` to produce a signed manifest.
- Platform keys (`<os>-<arch>`) and publication timestamps as caller-supplied text.

## Out of scope

- Downloading or applying an update, staging, restart orchestration.
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

### D5 - No runtime claim

The signature and hash prevent a tampered manifest/artifact from being accepted by a future client,
but nothing downloads or applies an update yet. `auro dev` and `auro bundle` are unaffected.

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
2. Empty signature and unknown format fail with the typed errors
   (`rejects_an_empty_signature_and_unknown_format`).
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
| Download/apply/staging/restart and rollback | Updater runtime spec |
| Deltas/patches and staged rollout percentages | Updater rollout spec |
| Key pinning and rotation inside the app | Trust/update client spec |
| Update channels and UI | Platform/update UX follow-up |
| Hosting/CSP and mirror behavior | Release service follow-up |
