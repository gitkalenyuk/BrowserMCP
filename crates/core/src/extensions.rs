//! [`Extensions`]: a small typed "services" map shared by every command call.
//!
//! Commands are stateless unit structs, but some features need state that lives longer than
//! one call — for example the per-tab ref registry of the perception layer (`@7:e3` must still
//! mean the same button in the next call). Such a feature puts one value of its own type into
//! the map once at startup, and every call reads it back with [`Extensions::get`]:
//!
//! ```
//! use std::sync::Mutex;
//! use bmcp_core::Extensions;
//!
//! #[derive(Default)]
//! struct Counter(Mutex<u32>);
//!
//! let ext = Extensions::new();
//! ext.insert(Counter::default());
//! *ext.get::<Counter>().unwrap().0.lock().unwrap() += 1;
//! assert_eq!(*ext.get::<Counter>().unwrap().0.lock().unwrap(), 1);
//! ```
//!
//! The key is the Rust type itself (`TypeId`), so two crates can never clash by accident,
//! and nobody needs to know the other services' types. The [`crate::Registry`] owns one
//! `Arc<Extensions>` and hands it to every [`crate::Ctx`] it runs (`ctx.extensions`), so the
//! API layers (MCP, REST, WS) get it without any extra plumbing.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// Type-keyed map of shared services. Cheap to read from many threads at once.
#[derive(Default)]
pub struct Extensions {
    // `RwLock` so services can be added after the registry is shared (`Arc`), e.g. lazily.
    map: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Extensions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `value`, replacing an earlier value of the same type. Returns the shared handle.
    pub fn insert<T: Any + Send + Sync>(&self, value: T) -> Arc<T> {
        let value = Arc::new(value);
        self.write().insert(TypeId::of::<T>(), value.clone());
        value
    }

    /// The value of type `T`, if one was inserted.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let any = self.read().get(&TypeId::of::<T>())?.clone();
        // The map only ever stores a `T` under `TypeId::of::<T>()`, so this cannot fail.
        any.downcast::<T>().ok()
    }

    /// The value of type `T`, inserting `make()` first if there is none yet.
    pub fn get_or_insert_with<T: Any + Send + Sync>(&self, make: impl FnOnce() -> T) -> Arc<T> {
        if let Some(value) = self.get::<T>() {
            return value;
        }
        let mut map = self.write();
        // Another thread may have inserted it between our read and write lock.
        let entry = map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(make()))
            .clone();
        entry
            .downcast::<T>()
            .expect("Extensions stores each value under its own TypeId")
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    // A panic while holding the lock cannot leave the map half-updated (insert is one
    // operation), so a poisoned lock is safe to keep using.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.map.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.map.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct A(u32);
    #[derive(Debug, PartialEq)]
    struct B(&'static str);

    #[test]
    fn typed_get_insert_replace() {
        let ext = Extensions::new();
        assert!(ext.get::<A>().is_none());
        ext.insert(A(1));
        ext.insert(B("x"));
        assert_eq!(*ext.get::<A>().unwrap(), A(1));
        assert_eq!(*ext.get::<B>().unwrap(), B("x"));
        ext.insert(A(2));
        assert_eq!(*ext.get::<A>().unwrap(), A(2));
        assert_eq!(ext.len(), 2);
    }

    #[test]
    fn get_or_insert_keeps_the_first_value() {
        let ext = Extensions::new();
        let first = ext.get_or_insert_with(|| A(1));
        let second = ext.get_or_insert_with(|| A(99));
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(*second, A(1));
    }
}
