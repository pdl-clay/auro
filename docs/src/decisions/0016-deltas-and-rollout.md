# 0016 - Delta patches are base-bound; rollout is deterministic

**Status:** Accepted (2026-11-18)

## Context

The updater can verify and stage full artifacts, but the roadmap calls for deltas and staged
rollout. Both change the signed manifest shape and both have failure modes that can strand or
fragment an install:

1. Apply a patch without verifying the base or the resulting full artifact.
2. Pick rollout membership randomly or from unstable machine data.
3. Put delta/rollout metadata in an unsigned sidecar.
4. Use a custom binary patch format that only Auro can produce.

## Decision

Manifest format v2 adds optional signed `delta` and `rollout` fields. Deltas are
bsdiff-4-compatible patches (`qbsdiff`), bound to the running version and base SHA-256; the patch
size/hash and resulting full artifact size/hash are verified before staging. A caller can fall back
to a full download explicitly.

Rollout uses a caller-supplied stable install key and a SHA-256 bucket over key, version and
artifact hash. 0 holds, 100 admits, and a partial percentage without a key holds. The selection is
deterministic; the manifest is the only rollout control plane in this slice.

## Consequences

**Easier**

- Operators can attach a patch and percentage to the same signed release object.
- A bad or mismatched patch cannot become the installed artifact.
- A support engineer can reproduce rollout membership from the key/version/hash.

**Harder**

- A manifest format change requires all clients to understand v2; v1 is rejected.
- Delta generation and storage become release-pipeline work.
- Changing rollout percentage requires a newly signed manifest, not a server toggle.
- Delta application needs both the old artifact and temporarily enough disk for the new one.

**Forbidden**

- Trusting a delta URL or patch bytes without the signed metadata.
- Random per-launch rollout selection.
- Replacing the base before the full result verifies.
- Claiming a delta was applied when the staged full artifact does not match the manifest.

## Revisit when

- A server-side rollout control plane or channels are specified.
- Delta chains, reverse deltas or delta rollback are needed.
- Artifact formats require a patch algorithm other than bsdiff 4.
