// SPDX-License-Identifier: MIT OR Apache-2.0

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

/// An owned handle to one piece of application state.
///
/// Deliberately not `State<'_, T>`: a borrowing handle cannot be captured by the `Send + 'static`
/// future a command becomes.
pub struct State<T> {
    inner: Arc<T>,
}

impl<T> State<T> {
    pub fn new(inner: Arc<T>) -> Self {
        State { inner }
    }

    pub fn arc(&self) -> Arc<T> {
        Arc::clone(&self.inner)
    }
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        State {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> std::fmt::Debug for State<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").finish_non_exhaustive()
    }
}

/// A process-local map of type to value, behind an `Arc`.
///
/// The concrete type, not a trait: `state::<T>()` is generic, so a trait object could not expose
/// it without an allocation per lookup.
#[derive(Default)]
pub struct StateStore {
    values: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl StateStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a value, replacing any previous value of the same type.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) {
        let mut values = self.values.lock().expect("state store poisoned");
        values.insert(TypeId::of::<T>(), Arc::new(value));
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.values
            .lock()
            .expect("state store poisoned")
            .contains_key(&TypeId::of::<T>())
    }

    /// Resolves a value by type.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<State<T>> {
        let values = self.values.lock().expect("state store poisoned");
        let stored = values.get(&TypeId::of::<T>())?;
        let typed = Arc::clone(stored).downcast::<T>().ok()?;
        Some(State::new(typed))
    }
}

impl std::fmt::Debug for StateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateStore").finish_non_exhaustive()
    }
}
