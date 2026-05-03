//! [`Scene`] — addressable leaf-grain state container with broadcast
//! fan-out of patches to subscribers.

use std::sync::Arc;

use lumencast_protocol::frames::State;
use lumencast_protocol::types::{Patch, SceneId, SceneVersion};
use lumencast_protocol::{LeafPath, types::check_leaf_value};
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::ServerError;
use crate::store::Store;

/// Capacity of the per-scene broadcast channel. Old patches are
/// overwritten; lagged subscribers will detect the lag and reconnect.
const BROADCAST_CAPACITY: usize = 1024;

/// A scene — bundle identity, leaf state, and a fan-out channel of
/// patches.
///
/// `Scene` is a cheap-to-clone handle around an `Arc`, safe to pass to
/// adapter tasks.
#[derive(Debug, Clone)]
pub struct Scene {
    inner: Arc<SceneInner>,
}

#[derive(Debug)]
pub(crate) struct SceneInner {
    id: SceneId,
    version: SceneVersion,
    pub(crate) store: Store,
    pub(crate) tx: broadcast::Sender<Arc<[Patch]>>,
    /// Canonical bytes of the LSML bundle backing this scene (set when
    /// the scene is registered via [`crate::ServerHandle::register_bundle`]).
    bundle_bytes: RwLock<Option<Arc<Vec<u8>>>>,
}

impl Scene {
    pub(crate) fn new(id: SceneId, version: SceneVersion) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(SceneInner {
                id,
                version,
                store: Store::new(),
                tx,
                bundle_bytes: RwLock::new(None),
            }),
        }
    }

    /// Attach canonical LSML bundle bytes to this scene, so the server
    /// can serve them at the content-addressed URL.
    pub(crate) fn attach_bundle_bytes(&self, bytes: Vec<u8>) {
        *self.inner.bundle_bytes.write() = Some(Arc::new(bytes));
    }

    /// Bundle bytes if this scene was registered with one.
    #[must_use]
    pub fn bundle_bytes(&self) -> Option<Arc<Vec<u8>>> {
        self.inner.bundle_bytes.read().clone()
    }

    /// Stable scene identifier.
    #[must_use]
    pub fn id(&self) -> &SceneId {
        &self.inner.id
    }

    /// Content hash of the bundle this scene corresponds to.
    #[must_use]
    pub fn version(&self) -> &SceneVersion {
        &self.inner.version
    }

    /// Set a single leaf and broadcast the resulting patch.
    pub fn set(&self, path: &str, value: Value) -> Result<(), ServerError> {
        let path = LeafPath::parse(path).map_err(ServerError::Protocol)?;
        check_leaf_value(&value).map_err(|e| ServerError::InvalidValue(e.to_string()))?;
        self.inner.store.set(path.as_str(), value.clone());
        let patches: Arc<[Patch]> = Arc::from([Patch::new(path, value)]);
        // ignore receiver count: zero subscribers is fine.
        let _ = self.inner.tx.send(patches);
        Ok(())
    }

    /// Apply many `(path, value)` patches atomically and broadcast a
    /// single delta containing all of them.
    pub fn emit<I, S>(&self, patches: I) -> Result<(), ServerError>
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        let mut parsed: Vec<Patch> = Vec::new();
        for (path, value) in patches {
            let path = LeafPath::parse(path.into()).map_err(ServerError::Protocol)?;
            check_leaf_value(&value).map_err(|e| ServerError::InvalidValue(e.to_string()))?;
            parsed.push(Patch::new(path, value));
        }
        if parsed.is_empty() {
            return Ok(());
        }
        self.inner.store.apply(
            parsed
                .iter()
                .map(|p| (p.path.as_str().to_string(), p.value.clone())),
        );
        let arc: Arc<[Patch]> = Arc::from(parsed.into_boxed_slice());
        let _ = self.inner.tx.send(arc);
        Ok(())
    }

    /// Seed the scene's store with defaults from an LSML bundle.
    /// Bypasses the broadcast channel — used at scene construction
    /// time, before any subscriber is attached.
    pub fn seed<I, S>(&self, defaults: I)
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        self.inner
            .store
            .seed(defaults.into_iter().map(|(k, v)| (k.into(), v)));
    }

    /// Read a snapshot of the full state (cloned).
    #[must_use]
    pub fn snapshot_state(&self) -> State {
        self.inner.store.snapshot()
    }

    /// Subscribe to the patch broadcast channel.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Arc<[Patch]>> {
        self.inner.tx.subscribe()
    }
}
