// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform keychain access.
//!
//! The store is pluggable so tests can use an in-memory backend. The default backend is
//! `keyring`, which maps to the macOS Keychain, Windows Credential Manager and the
//! Secret Service on Linux.

use thiserror::Error;

/// Why a keychain operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KeychainError {
    #[error("keychain store error: {0}")]
    Store(#[from] keyring::Error),
}

/// A store that can persist one secret.
pub trait CredentialStore: Send + Sync {
    fn set_password(&self, password: &str) -> Result<(), KeychainError>;
    fn get_password(&self) -> Result<String, KeychainError>;
    fn delete_credential(&self) -> Result<(), KeychainError>;
}

struct PlatformKeychain {
    entry: keyring::Entry,
}

impl CredentialStore for PlatformKeychain {
    fn set_password(&self, password: &str) -> Result<(), KeychainError> {
        self.entry.set_password(password)?;
        Ok(())
    }

    fn get_password(&self) -> Result<String, KeychainError> {
        Ok(self.entry.get_password()?)
    }

    fn delete_credential(&self) -> Result<(), KeychainError> {
        self.entry.delete_credential()?;
        Ok(())
    }
}

/// Accesses one keychain entry.
#[derive(Clone)]
pub struct Keychain {
    store: std::sync::Arc<dyn CredentialStore>,
}

impl Keychain {
    /// Opens the keychain entry for `service`/`username` on the platform store.
    pub fn new(service: &str, username: &str) -> Result<Self, KeychainError> {
        let entry = keyring::Entry::new(service, username)?;
        Ok(Keychain::with_store(PlatformKeychain { entry }))
    }

    /// Creates a keychain handle backed by the given store.
    pub fn with_store(store: impl CredentialStore + 'static) -> Self {
        Keychain {
            store: std::sync::Arc::new(store),
        }
    }

    /// Stores a secret.
    pub fn set_password(&self, password: impl Into<String>) -> Result<(), KeychainError> {
        self.store.set_password(&password.into())
    }

    /// Reads the secret.
    pub fn get_password(&self) -> Result<String, KeychainError> {
        self.store.get_password()
    }

    /// Deletes the secret.
    pub fn delete_credential(&self) -> Result<(), KeychainError> {
        self.store.delete_credential()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct MockStore {
        entries: Arc<Mutex<HashMap<String, String>>>,
    }

    impl MockStore {
        fn new() -> Self {
            MockStore::default()
        }
    }

    impl CredentialStore for MockStore {
        fn set_password(&self, password: &str) -> Result<(), KeychainError> {
            self.entries
                .lock()
                .expect("store")
                .insert("secret".to_owned(), password.to_owned());
            Ok(())
        }

        fn get_password(&self) -> Result<String, KeychainError> {
            self.entries
                .lock()
                .expect("store")
                .get("secret")
                .cloned()
                .ok_or_else(|| keyring::Error::NoEntry.into())
        }

        fn delete_credential(&self) -> Result<(), KeychainError> {
            self.entries.lock().expect("store").remove("secret");
            Ok(())
        }
    }

    #[test]
    fn round_trips_a_secret() {
        let store = MockStore::new();
        let keychain = Keychain::with_store(store.clone());

        keychain.set_password("hunter2").expect("set");
        assert_eq!(keychain.get_password().expect("get"), "hunter2");

        keychain.delete_credential().expect("delete");
        assert!(keychain.get_password().is_err());
    }

    #[test]
    fn backend_errors_surface() {
        struct FailingStore;

        impl CredentialStore for FailingStore {
            fn set_password(&self, _password: &str) -> Result<(), KeychainError> {
                Err(keyring::Error::NoEntry.into())
            }

            fn get_password(&self) -> Result<String, KeychainError> {
                Err(keyring::Error::NoEntry.into())
            }

            fn delete_credential(&self) -> Result<(), KeychainError> {
                Err(keyring::Error::NoEntry.into())
            }
        }

        let keychain = Keychain::with_store(FailingStore);
        assert!(keychain.set_password("hunter2").is_err());
        assert!(keychain.get_password().is_err());
        assert!(keychain.delete_credential().is_err());
    }
}
