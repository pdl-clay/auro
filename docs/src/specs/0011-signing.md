# 0011 - Signing and release keys

**Status:** Implemented (2026-11-17; platform signing delegated, notarization credentials not exercised)

Phase 2 needs signed artifacts and a root of trust for update manifests. Auro owns the key format
and the manifest signature; it delegates OS code signing to the platform toolchain through
`tauri-bundler` ([ADR 0011](../decisions/0011-bundling-delegates.md), [ADR 0012](../decisions/0012-release-trust.md)).

## Scope

- `auro-sign`: Ed25519 signing keys, signatures, SHA-256 file hashes and key files.
- `[bundle.macos]` and `[bundle.windows]` config keys mapped to the bundler.
- `auro generate-key --out <path>`.
- The config-to-signing mapping for macOS codesign/hardened runtime/entitlements and Windows
  Authenticode fields.

## Out of scope

- Provisioning Apple Developer or Windows Authenticode certificates.
- Storing keys in a platform keychain; the initial contract is a 0600 key file.
- A signing service or HSM.
- Notarization credentials in the repository; they remain environment variables.
- Runtime updater download/apply (see spec 0012).

## Decisions

### D1 - Ed25519 keys, base64 text files

A secret key is a 32-byte Ed25519 seed. The key file is:

```
# Auro signing key v1
<base64 seed>
```

`auro generate-key` writes it with owner-only permissions on Unix and prints the public key. A
public key is base64 of 32 bytes; a signature is base64 of 64 bytes. Debug output never prints the
seed. Verification uses `verify_strict`, so malleable signatures are rejected.

### D2 - SHA-256 for artifacts

`auro-sign::sha256_file` produces lowercase hex. Manifests carry the digest and size; the signature
covers the manifest payload, not the artifact directly.

### D3 - Platform signing is delegated

- macOS: `[bundle.macos]` maps to `MacOsSettings` (`signing_identity`, entitlements, hardened
  runtime, skip stapling). Notarization uses the bundler's environment contract
  (`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`); no credentials are config keys.
- Windows: `[bundle.windows]` maps to `WindowsSettings` (certificate thumbprint, digest algorithm,
  timestamp URL/TSP, custom sign command + args, downgrade policy). WebView2 remains skipped.
- Auro does not shell out to `codesign`/`signtool` itself; the bundler owns the platform commands.

### D4 - No signing secrets in the repository

Key files and certificates are outside the app source tree and CI secrets. `auro bundle` works
unsigned when no identity is configured; the macOS smoke workflow exercises exactly that path.

## Interfaces

```rust
pub struct SigningKey;
impl SigningKey {
    pub fn generate() -> Self;
    pub fn from_seed_base64(seed: &str) -> Result<Self, SignError>;
    pub fn seed_base64(&self) -> String;
    pub fn public_key(&self) -> PublicKey;
    pub fn sign(&self, bytes: &[u8]) -> String;
    pub fn load_from(path: &Path) -> Result<Self, SignError>;
    pub fn save_to(&self, path: &Path) -> Result<(), SignError>;
}

pub struct PublicKey;
impl PublicKey {
    pub fn from_base64(key: &str) -> Result<Self, SignError>;
    pub fn to_base64(&self) -> String;
    pub fn verify(&self, bytes: &[u8], signature: &str) -> Result<(), SignError>;
}

pub fn sign_file(key: &SigningKey, path: &Path) -> Result<String, SignError>;
pub fn verify_file(key: &PublicKey, path: &Path, signature: &str) -> Result<(), SignError>;
pub fn sha256_file(path: &Path) -> Result<String, SignError>;
```

```text
auro generate-key --out release.key
```

## Acceptance

1. Sign/verify accepts the exact bytes and rejects a changed byte or malformed signature
   (`auro-sign` tests).
2. Key files round-trip and are 0600 on Unix (`key_file_round_trips_with_owner_only_permissions`).
3. File signing/hashing works and rejects tampering (`signs_and_verifies_a_file_and_hashes_it`).
4. `[bundle.macos]`/`[bundle.windows]` values reach the bundler settings
   (`auro-bundle` `maps_signing_settings`).
5. `auro generate-key` writes a loadable key (`auro-cli` test).
6. Unsigned Linux/Windows/macOS bundles still build; platform signing is not claimed without
   credentials.
7. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Keychain/HSM-backed keys | Platform-services/keychain slice |
| Notarization credential wiring and CI secret setup | Signing CI follow-up |
| Windows certificate provisioning and signtool verification | Windows signing follow-up |
| Key rotation and revocation | Updater/trust spec |
| Timestamp format for manifests | Spec 0012 / update service |
