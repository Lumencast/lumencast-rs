//! [`Scene`] — addressable leaf-grain state container with broadcast
//! fan-out of patches to subscribers.

use std::collections::HashMap;
use std::sync::Arc;

use lumencast_protocol::frames::State;
use lumencast_protocol::types::{Cause, Patch, SceneId, SceneVersion};
use lumencast_protocol::{ErrorCode, LeafPath, types::check_leaf_value};
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::ServerError;
use crate::input::{InputSpec, check_constraint};
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

/// Broadcast payload : a delta's patches paired with optional
/// LSDP/1.1 §3.2.3 provenance metadata. Subscribers consume both ;
/// 1.0 callers leave `cause = None` to preserve the legacy wire shape.
#[derive(Clone, Debug)]
pub(crate) struct DeltaPayload {
    pub patches: Arc<[Patch]>,
    pub cause: Option<Cause>,
}

#[derive(Debug)]
pub(crate) struct SceneInner {
    id: SceneId,
    version: SceneVersion,
    pub(crate) store: Store,
    pub(crate) tx: broadcast::Sender<DeltaPayload>,
    /// Canonical bytes of the LSML bundle backing this scene (set when
    /// the scene is registered via [`crate::ServerHandle::register_bundle`]).
    bundle_bytes: RwLock<Option<Arc<Vec<u8>>>>,
    /// Declared `operator_inputs` for this scene. `None` means
    /// permissive — every patch is accepted (subject to role/value
    /// checks elsewhere). `Some(_)` enables strict enforcement: any
    /// path not in the map yields `UNKNOWN_PATH`, any value violating
    /// the spec's constraints yields `INVALID_VALUE`.
    declared_inputs: RwLock<Option<HashMap<String, InputSpec>>>,
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
                declared_inputs: RwLock::new(None),
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
        let _ = self.inner.tx.send(DeltaPayload {
            patches,
            cause: None,
        });
        Ok(())
    }

    /// Apply many `(path, value)` patches atomically and broadcast a
    /// single delta containing all of them.
    pub fn emit<I, S>(&self, patches: I) -> Result<(), ServerError>
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        self.emit_with_cause(patches, None)
    }

    /// LSDP/1.1 §3.2.3 — same as [`Self::emit`] but the resulting Delta
    /// frame carries the supplied [`Cause`] as provenance. Adapters
    /// and operator-input pipelines use this to thread origin info
    /// through to the wire. 1.0 callers stay on `emit`.
    pub fn emit_with_cause<I, S>(&self, patches: I, cause: Option<Cause>) -> Result<(), ServerError>
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
        let _ = self.inner.tx.send(DeltaPayload {
            patches: arc,
            cause,
        });
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
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<DeltaPayload> {
        self.inner.tx.subscribe()
    }

    /// Declare the full set of `operator_inputs` for this scene.
    ///
    /// Switches the scene from permissive (the default) to strict
    /// validation: subsequent `input` frames whose patch path isn't
    /// in `specs` yield an `UNKNOWN_PATH` error frame, and per-spec
    /// constraint violations yield `INVALID_VALUE`. Replaces any
    /// previously declared set.
    #[must_use]
    pub fn with_operator_inputs(self, specs: Vec<InputSpec>) -> Self {
        let map: HashMap<String, InputSpec> = specs
            .into_iter()
            .map(|spec| (spec.path.as_str().to_string(), spec))
            .collect();
        *self.inner.declared_inputs.write() = Some(map);
        self
    }

    /// Shorthand for [`Scene::with_operator_inputs`] when only path
    /// declaredness matters (no per-spec constraints).
    #[must_use]
    pub fn with_declared_inputs(self, paths: Vec<LeafPath>) -> Self {
        let specs: Vec<InputSpec> = paths.into_iter().map(InputSpec::new).collect();
        self.with_operator_inputs(specs)
    }

    /// Returns `true` if this scene enforces declared-inputs (strict
    /// mode). Useful for the WS handler to skip work in permissive
    /// mode.
    #[must_use]
    pub fn enforces_declared_inputs(&self) -> bool {
        self.inner.declared_inputs.read().is_some()
    }

    /// Validate one patch against the scene's declared inputs.
    ///
    /// Returns `Ok(())` when permissive (no declarations) or when the
    /// patch matches its spec. Returns `Err(InputRejection { … })`
    /// otherwise — the WS handler turns this into the corresponding
    /// `error` frame.
    pub fn check_input_patch(&self, patch: &Patch) -> Result<(), InputRejection> {
        let guard = self.inner.declared_inputs.read();
        let Some(declared) = guard.as_ref() else {
            return Ok(());
        };
        let Some(spec) = declared.get(patch.path.as_str()) else {
            return Err(InputRejection {
                code: ErrorCode::UnknownPath,
                message: format!("path {:?} not declared in operator_inputs", patch.path),
                path: patch.path.as_str().to_string(),
            });
        };
        if let Some(message) = check_constraint(spec, &patch.value) {
            return Err(InputRejection {
                code: ErrorCode::InvalidValue,
                message,
                path: patch.path.as_str().to_string(),
            });
        }
        Ok(())
    }
}

/// Reason a patch was rejected by [`Scene::check_input_patch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRejection {
    /// LSDP/1 error code (`UNKNOWN_PATH` or `INVALID_VALUE`).
    pub code: ErrorCode,
    /// Human-readable description.
    pub message: String,
    /// Offending leaf path. Echoed back in the `error` frame's `path`
    /// field so the harness can localise authoring mistakes.
    pub path: String,
}
