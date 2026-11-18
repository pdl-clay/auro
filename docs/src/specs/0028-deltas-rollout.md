# 0028 - Binary deltas and staged rollout

**Status:** Implemented (2026-11-18; bsdiff-compatible deltas and deterministic percentage rollout)

Spec 0012's flat manifest and spec 0013's full-artifact staging are enough to ship, but not enough to
save bandwidth or release gradually. This spec adds two optional, signed fields to manifest format
v2 and the runtime paths that consume them.

## Scope

- Manifest format v2 with optional `delta` and `rollout` metadata.
- `create_delta`: produce a bsdiff-compatible patch.
- `Manifest::with_delta` and `Manifest::with_rollout`.
- `Updater::stage_delta_from_path` and `download_delta`: verify a base, verify the patch, apply it,
  then verify and stage the full artifact.
- `Updater::with_rollout_key`: deterministic selection for a percentage rollout.
- CLI `auro delta` and the matching `auro manifest` delta/rollout options.

## Out of scope

- A server-side rollout control plane or percentage update without a new manifest.
- Delta chains, reverse deltas or delta rollback.
- Automatic full-download fallback inside `download_delta`; the caller can fall back to `download`.
- Delta patches for directory/bundle artifacts. The patch applies to the full staged file.
- Signed separate delta files; the patch hash is covered by the signed manifest.
- Key rotation, channels and rollout telemetry.

## Decisions

### D1 - Manifest v2 adds optional signed fields

`FORMAT_VERSION` is now 2. A v2 manifest keeps the v1 fields and adds optional:

```json
"delta": {
  "from_version": "1.0.0",
  "from_sha256": "<base sha256>",
  "url": "https://example.test/app-1.0.0.patch",
  "size": 1234,
  "sha256": "<patch sha256>"
},
"rollout": { "percentage": 25 }
```

Both are omitted when absent, so a full v2 manifest has the same shape as v1 plus `format: 2`. The
signature payload includes whichever optional fields are present. Unknown fields remain rejected;
invalid hashes, empty delta URLs/versions and percentages above 100 are typed errors.

*Rejected:* an unsigned sidecar for rollout/delta. It would let a mirror change who receives an
update without invalidating the release signature.

### D2 - Delta is bsdiff-compatible and base-bound

`auro delta` and `create_delta` use `qbsdiff` to produce a bsdiff 4-compatible patch. A delta carries
the base version and SHA-256. `stage_delta_from_path`/`download_delta`:

1. require an available (and selected) manifest;
2. compare the running version and base hash;
3. verify the patch size and SHA-256;
4. apply it to the base;
5. verify the resulting full artifact size and SHA-256 against the manifest; and
6. persist the full artifact at the same content-addressed path as a full download.

The patch is not trusted just because it came over HTTPS; the signed manifest covers its hash and
the resulting full artifact is re-verified.

*Rejected:* applying a patch in place without a full-artifact hash. A bad patch must not become the
installed artifact.

### D3 - Rollout is deterministic and fails closed

`Rollout { percentage }` is selected per install. `Updater::with_rollout_key` supplies a stable
local key; the bucket is SHA-256 of `key + version + artifact hash`, reduced modulo 100. `0` is
always held back, `100` is always available, and a partial rollout with no key is held back. The
same key/version/artifact always selects the same bucket.

`UpdateDecision` gains `HeldBack`; `download`/`download_delta` return `UpdateError::HeldBack` rather
than downloading. A future server can publish a 100% manifest to force completion.

*Rejected:* random per-launch selection. It flaps users between available and held back. *Rejected:*
using the machine hostname or username directly; the caller supplies a stable, app-owned key.

### D4 - Full downloads remain the fallback

`download` is unchanged and does not require a delta. An app that has no suitable base, or receives
`DeltaBaseMismatch`/`DeltaVersionMismatch`, can call `download` for the full artifact. This keeps the
failure mode explicit and preserves the existing staging contract.

## Interfaces

```rust
pub enum UpdateDecision { UpToDate, Available, HeldBack, Downgrade }

pub struct Delta {
    pub from_version: String,
    pub from_sha256: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

pub struct Rollout { pub percentage: u8 }

impl Manifest {
    pub fn with_delta(self, delta: Delta) -> Self;
    pub fn with_rollout(self, rollout: Rollout) -> Self;
}

pub fn create_delta(base: &Path, target: &Path) -> Result<Vec<u8>, UpdateError>;

impl Updater {
    pub fn with_rollout_key(self, key: impl Into<String>) -> Self;
    pub fn download_delta(
        &self,
        manifest: &Manifest,
        base: &Path,
    ) -> Result<StagedUpdate, UpdateError>;
    pub fn stage_delta_from_path(
        &self,
        manifest: &Manifest,
        base: &Path,
        patch: &Path,
    ) -> Result<StagedUpdate, UpdateError>;
}
```

CLI:

```text
auro delta --from old.bin --to new.bin --out app.patch
auro manifest --artifact new.bin ... \
  --rollout-percentage 25 \
  --delta-from old.bin --delta-from-version 1.0.0 \
  --delta-url https://example.test/app.patch --delta-out app.patch
```

`UpdateError` adds `HeldBack`, `DeltaMissing`, `DeltaBaseMismatch` and `DeltaVersionMismatch`.
`FORMAT_VERSION` is 2; v1 manifests are rejected with `UnsupportedFormat(2)`.

## Acceptance

1. Manifest v2 signs/verifies delta and rollout fields and rejects a rollout above 100
   (`signs_delta_and_rollout_metadata`, `rejects_an_invalid_rollout_percentage`).
2. A local delta patch is applied to its matching base; the full artifact verifies
   (`applies_a_local_delta_and_verifies_the_full_artifact`).
3. A delta for a different base is rejected (`rejects_a_delta_for_the_wrong_base`).
4. 0% is `HeldBack`, 100% is `Available`, a partial rollout without a key is held, and a key selects
   deterministically (`staged_rollout_is_deterministic_and_gates_downloads`).
5. CLI writes a delta patch and manifest delta/rollout metadata (`creates_a_patch_file`,
   `writes_delta_metadata_and_rollout`).
6. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Server-side percentage control without a new manifest | Rollout service follow-up |
| Delta chains and reverse deltas | Delta follow-up |
| Delta telemetry and bandwidth accounting | Release metrics follow-up |
| Channel policy and forced 100% completion | Release UX/spec |
