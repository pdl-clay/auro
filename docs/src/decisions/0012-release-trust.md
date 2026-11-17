# 0012 - Release trust: Ed25519 manifests, delegated platform signing

**Status:** Accepted (2026-11-17)

## Context

Phase 2 needs two different trust problems solved:

1. **OS code signing**: macOS codesign/notarization and Windows Authenticode. `tauri-bundler`
   already drives these platform tools and exposes the settings/environment hooks (ADR 0011).
2. **Update authenticity**: a client must reject a manifest or artifact that was tampered with.
   Auro owns this format; Tauri's manifest shape is not Auro's contract.

Options for update authenticity:

1. Reuse minisign/Tauri signature files. Rejected as the only contract: it ties Auro's manifest
   format to a third-party CLI and its key handling.
2. HMAC with a shared secret. Rejected: a client that can verify can also forge.
3. Ed25519 detached signatures over a canonical JSON payload. Chosen.
4. Platform store signatures only. Rejected: they do not cover a cross-platform update manifest.

## Decision

- `auro-sign` provides Ed25519 keys, detached signatures and SHA-256 file hashes. Secret keys are
  32-byte base64 seeds in a 0600 text file; public keys and signatures are base64. Verification is
  strict (malleable signatures rejected).
- `[bundle.macos]` and `[bundle.windows]` map signing inputs onto `tauri-bundler`; Auro does not
  invoke `codesign`/`signtool` itself. Notarization credentials stay in the environment.
- `auro-update` defines manifest format v1. The signature covers the manifest payload excluding the
  signature field; size and SHA-256 cover the artifact. The client format is Auro's, independent of
  Tauri's updater.
- Auro never stores signing secrets in the repository. Unsigned bundles remain valid build outputs;
  signing is a configured capability, not a hard failure.
- The updater runtime (download/apply/rollback/deltas/rollout) is explicitly **not** part of this
  decision; this ADR covers the trust format and key mechanics.

## Consequences

**Easier**

- A small, auditable signature format with no new service dependency.
- Platform signing reuses the bundler and its CI/credential conventions.
- Tests can exercise key/signature/manifest behavior without certificates.

**Harder**

- Key distribution, rotation and revocation still need a client/trust contract.
- Losing a secret key means losing the ability to publish updates accepted by clients pinned to its
  public key; backup/HSM policy is operational work.
- Platform signing/notarization still requires certificates and environment secrets; the unsigned
  smoke tests do not prove a signed release.
- The manifest is not yet consumed by a runtime updater, so the end-to-end update path is unproven.

**Forbidden**

- Signing keys or certificates in the repository, test fixtures or logs.
- Presenting an unsigned artifact as signed/notarized.
- Changing manifest field names/order without a format bump and migration.

## Revisit when

- The runtime updater and rollback contract are specified.
- Key rotation/revocation or an HSM/keychain-backed signer is needed.
- Deltas or staged rollout change the manifest shape.
