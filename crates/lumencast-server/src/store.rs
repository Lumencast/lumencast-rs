//! In-memory leaf-grain state store backing one [`Scene`](crate::Scene).

use std::collections::HashMap;

use lumencast_protocol::frames::State;
use parking_lot::RwLock;
use serde_json::Value;

/// Mutable map of `path → value`. Locking is global write/read; for
/// v0.1 the workload is push-heavy on small numbers of connections, so
/// per-shard locking is not yet justified.
#[derive(Debug, Default)]
pub(crate) struct Store {
    inner: RwLock<HashMap<String, Value>>,
}

impl Store {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replace `path` with `value`. Returns the previous value, if any.
    pub(crate) fn set(&self, path: &str, value: Value) -> Option<Value> {
        self.inner.write().insert(path.to_string(), value)
    }

    /// Apply many `(path, value)` updates atomically.
    pub(crate) fn apply<I>(&self, patches: I)
    where
        I: IntoIterator<Item = (String, Value)>,
    {
        let mut g = self.inner.write();
        for (path, value) in patches {
            g.insert(path, value);
        }
    }

    /// Drop `path`. Returns the previous value, if any.
    #[allow(dead_code)]
    pub(crate) fn remove(&self, path: &str) -> Option<Value> {
        self.inner.write().remove(path)
    }

    /// Snapshot the entire state (cloned).
    pub(crate) fn snapshot(&self) -> State {
        let g = self.inner.read();
        let mut out = State::new();
        for (k, v) in g.iter() {
            out.insert(k.clone(), v.clone());
        }
        out
    }

    /// Seed the store with bundle defaults. Existing entries are
    /// overwritten.
    pub(crate) fn seed<I>(&self, defaults: I)
    where
        I: IntoIterator<Item = (String, Value)>,
    {
        let mut g = self.inner.write();
        for (k, v) in defaults {
            g.insert(k, v);
        }
    }
}
