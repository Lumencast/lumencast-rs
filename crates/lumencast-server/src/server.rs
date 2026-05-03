//! [`Server`] — top-level handle and builder.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use dashmap::DashMap;
use lumencast_protocol::Bundle;
use lumencast_protocol::types::{SceneId, SceneVersion};
use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::auth::Authenticator;
use crate::bundle_route::scene_bundle_route;
use crate::config::ServerConfig;
use crate::error::ServerError;
use crate::scene::Scene;
use crate::ws_handler::ws_route;

/// Default placeholder version used when a scene is registered without
/// a real LSML bundle hash.
pub(crate) fn placeholder_version() -> SceneVersion {
    let mut s = String::with_capacity("sha256:".len() + 64);
    s.push_str("sha256:");
    for _ in 0..64 {
        s.push('0');
    }
    SceneVersion::from(s)
}

/// Capacity of the server-wide event broadcast.
const EVENT_CAPACITY: usize = 64;

/// Server-wide event multiplexed to every connected subscriber.
#[derive(Debug, Clone)]
pub(crate) enum ServerEvent {
    /// The active scene was swapped server-side.
    SceneSwap { scene_id: SceneId },
}

/// Shared server state, held inside an `Arc` and used as axum router
/// state.
pub(crate) struct ServerInner {
    pub(crate) auth: Arc<dyn Authenticator>,
    pub(crate) scenes: DashMap<String, Scene>,
    pub(crate) active: RwLock<Option<SceneId>>,
    pub(crate) events: broadcast::Sender<ServerEvent>,
    pub(crate) config: ServerConfig,
}

impl ServerInner {
    pub(crate) fn scene(&self, id: &str) -> Option<Scene> {
        self.scenes.get(id).map(|r| r.clone())
    }
}

/// Top-level server handle.
pub struct Server {
    pub(crate) inner: Arc<ServerInner>,
    pub(crate) listener: TcpListener,
}

/// Cheap-to-clone handle exposing scene management on a running server.
///
/// Obtained via [`Server::handle`]. Survives [`Server::run`] (which
/// consumes the parent), so adapter tasks can keep mutating scenes
/// after the server starts.
#[derive(Clone)]
pub struct ServerHandle {
    inner: Arc<ServerInner>,
}

impl ServerHandle {
    /// Register a new scene with a placeholder bundle version.
    pub fn new_scene(&self, id: impl Into<SceneId>) -> Result<Scene, ServerError> {
        self.new_scene_with_version(id, placeholder_version())
    }

    /// Register a new scene with an explicit bundle `version`.
    pub fn new_scene_with_version(
        &self,
        id: impl Into<SceneId>,
        version: SceneVersion,
    ) -> Result<Scene, ServerError> {
        let id = id.into();
        if self.inner.scenes.contains_key(id.as_str()) {
            return Err(ServerError::DuplicateScene(id.0));
        }
        let scene = Scene::new(id.clone(), version);
        self.inner.scenes.insert(id.0, scene.clone());
        let mut active = self.inner.active.write();
        if active.is_none() {
            *active = Some(scene.id().clone());
        }
        Ok(scene)
    }

    /// Register a scene from an LSML bundle. The bundle's
    /// `scene_version` is recomputed from canonical JSON, defaults are
    /// seeded into the scene's store, and the canonical bytes are
    /// cached for serving via `GET /scenes/:id/:version`.
    pub fn register_bundle(&self, bundle: Bundle) -> Result<Scene, ServerError> {
        let bundle = bundle.with_computed_version()?;
        let id = bundle.scene_id.clone();
        if self.inner.scenes.contains_key(id.as_str()) {
            return Err(ServerError::DuplicateScene(id.0));
        }
        let scene = Scene::new(id.clone(), bundle.scene_version.clone());
        scene.seed(bundle.defaults.iter().map(|(k, v)| (k.clone(), v.clone())));
        scene.attach_bundle_bytes(bundle.to_canonical_bytes()?);
        self.inner.scenes.insert(id.0, scene.clone());
        let mut active = self.inner.active.write();
        if active.is_none() {
            *active = Some(scene.id().clone());
        }
        Ok(scene)
    }

    /// Switch the server-wide active scene. Connected subscribers
    /// receive a `scene_changed` frame followed by a fresh `snapshot`.
    pub fn set_active_scene(&self, id: SceneId) -> Result<(), ServerError> {
        if !self.inner.scenes.contains_key(id.as_str()) {
            return Err(ServerError::UnknownScene(id.0));
        }
        *self.inner.active.write() = Some(id.clone());
        let _ = self
            .inner
            .events
            .send(ServerEvent::SceneSwap { scene_id: id });
        Ok(())
    }

    /// Currently active scene identifier (if any has been set).
    #[must_use]
    pub fn active_scene(&self) -> Option<SceneId> {
        self.inner.active.read().clone()
    }

    /// Look up a scene by id.
    #[must_use]
    pub fn scene(&self, id: &str) -> Option<Scene> {
        self.inner.scene(id)
    }
}

impl Server {
    /// Start a builder.
    #[must_use]
    pub fn builder() -> ServerBuilder {
        ServerBuilder::default()
    }

    /// Local address the server is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Get a clone-able handle that survives [`Server::run`].
    #[must_use]
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            inner: self.inner.clone(),
        }
    }

    /// Register a new empty scene and return a [`Scene`] handle.
    pub fn new_scene(&self, id: impl Into<SceneId>) -> Result<Scene, ServerError> {
        self.handle().new_scene(id)
    }

    /// Register a new scene with an explicit bundle `version`.
    pub fn new_scene_with_version(
        &self,
        id: impl Into<SceneId>,
        version: SceneVersion,
    ) -> Result<Scene, ServerError> {
        self.handle().new_scene_with_version(id, version)
    }

    /// Register a scene from an LSML bundle.
    pub fn register_bundle(&self, bundle: Bundle) -> Result<Scene, ServerError> {
        self.handle().register_bundle(bundle)
    }

    /// Switch the server-wide active scene.
    pub fn set_active_scene(&self, id: SceneId) -> Result<(), ServerError> {
        self.handle().set_active_scene(id)
    }

    /// Currently active scene identifier.
    #[must_use]
    pub fn active_scene(&self) -> Option<SceneId> {
        self.handle().active_scene()
    }

    /// Run the server until cancelled. Consumes `self`.
    pub async fn run(self) -> Result<(), ServerError> {
        axum::serve(self.listener, build_router(self.inner)).await?;
        Ok(())
    }

    /// Run the server until cancelled, with a graceful-shutdown
    /// future. Useful for tests.
    pub async fn run_with_shutdown<F>(self, signal: F) -> Result<(), ServerError>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        axum::serve(self.listener, build_router(self.inner))
            .with_graceful_shutdown(signal)
            .await?;
        Ok(())
    }
}

/// Build the axum router with the WS endpoint and the bundle-serving
/// route.
pub(crate) fn build_router(inner: Arc<ServerInner>) -> Router {
    Router::new()
        .route("/ws", get(ws_route))
        .route("/scenes/:scene_id/:version_hex", get(scene_bundle_route))
        .with_state(inner)
}

/// Builder for [`Server`].
#[derive(Default)]
pub struct ServerBuilder {
    addr: Option<String>,
    auth: Option<Arc<dyn Authenticator>>,
    config: ServerConfig,
}

impl ServerBuilder {
    /// Address the server will bind to (e.g. `"127.0.0.1:4000"`).
    #[must_use]
    pub fn listen(mut self, addr: impl Into<String>) -> Self {
        self.addr = Some(addr.into());
        self
    }

    /// Set the authenticator.
    #[must_use]
    pub fn auth<A: Authenticator + 'static>(mut self, auth: A) -> Self {
        self.auth = Some(Arc::new(auth));
        self
    }

    /// Set the authenticator from an existing `Arc<dyn Authenticator>`.
    #[must_use]
    pub fn auth_arc(mut self, auth: Arc<dyn Authenticator>) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Override the default [`ServerConfig`].
    #[must_use]
    pub fn config(mut self, config: ServerConfig) -> Self {
        self.config = config;
        self
    }

    /// Bind the listener and produce a [`Server`].
    pub async fn build(self) -> Result<Server, ServerError> {
        let addr = self.addr.ok_or(ServerError::BuilderMissing("listen"))?;
        let auth = self.auth.ok_or(ServerError::BuilderMissing("auth"))?;
        let listener = TcpListener::bind(&addr).await?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Ok(Server {
            inner: Arc::new(ServerInner {
                auth,
                scenes: DashMap::new(),
                active: RwLock::new(None),
                events,
                config: self.config,
            }),
            listener,
        })
    }
}
