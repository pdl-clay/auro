# 0019 - Keychain

**Status:** Implemented (2026-11-17)

Phase 2 needs apps to be able to persist a secret in the platform keychain. This spec fixes the
keychain service contract. The store is pluggable so tests can use an in-memory backend. The
`auro::services` re-export requires the facade's `services` feature.

## Scope

- `auro-services::Keychain`: set, read and delete one secret.
- `CredentialStore`: a pluggable store trait; the default is `keyring`.
- `auro::services::Keychain` re-export.

## Out of scope

- Multiple entries per service/user.
- Key rotation, expiry or secure in-memory handling.
- Protecting secrets from the user who owns the machine.
- Platform-specific options (biometric prompt, per-service access controls).
- Other platform services.

## Decisions

### D1 - `keyring` is the default store

Auro delegates the per-OS mechanics to `keyring`: macOS Keychain Services, Windows Credential
Manager and the Secret Service on Linux.

*Rejected:* implementing the three stores inside Auro. `keyring` is MIT/Apache licensed and already
handles the quirks.

### D2 - One entry per service/user

`Keychain::new(service, username)` opens a single entry. The app owns naming; Auro does not enforce
a namespace. This keeps the API small and avoids inventing a policy.

### D3 - The store is pluggable for tests

`Keychain::with_store` accepts any `CredentialStore`. Tests can inject an in-memory store and
verify the round-trip without touching the real keychain. Production code uses `Keychain::new()`.

*Rejected:* a mock flag on the production type. It keeps the boundary explicit and makes the test
double a test-only type.

### D4 - Errors surface as typed

`KeychainError` wraps the platform store error. The caller sees the failure immediately; there is no
retry or fallback inside the service.

## Interfaces

```rust
pub enum KeychainError { /* keyring::Error */ }

pub trait CredentialStore: Send + Sync {
    fn set_password(&self, password: &str) -> Result<(), KeychainError>;
    fn get_password(&self) -> Result<String, KeychainError>;
    fn delete_credential(&self) -> Result<(), KeychainError>;
}

pub struct Keychain { /* store */ }

impl Keychain {
    pub fn new(service: &str, username: &str) -> Result<Self, KeychainError>;
    pub fn with_store(store: impl CredentialStore + 'static) -> Self;
    pub fn set_password(&self, password: impl Into<String>) -> Result<(), KeychainError>;
    pub fn get_password(&self) -> Result<String, KeychainError>;
    pub fn delete_credential(&self) -> Result<(), KeychainError>;
}
```

Usage:

```rust,ignore
let keychain = auro::services::Keychain::new("com.example.app", "api-token")?;
keychain.set_password("hunter2")?;
let password = keychain.get_password()?;
keychain.delete_credential()?;
```

## Acceptance

1. `set_password` and `get_password` round-trip, and `delete_credential` removes the secret
   (`round_trips_a_secret`).
2. A store error surfaces as `KeychainError`
   (`backend_errors_surface`).
3. `auro::services` re-exports the API surface (`crates/auro/src/lib.rs`).
4. `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked`, and `mdbook build docs` pass.

## Open questions

| Question | Where it settles |
|---|---|
| Multiple entries and service namespaces | Keychain follow-up |
| Key rotation, expiry and secure memory | Keychain follow-up |
| Platform-specific access controls | Keychain follow-up |
| Other platform services | Platform services specs |
