//! Test control plane (LSDP/1 interop `CONTROL.md`).
//!
//! Gated behind the `interop-control-plane` feature. **Never** bundle
//! into production builds — this module deliberately exposes
//! authoritative state mutation over plain HTTP.
//!
//! # Endpoints
//!
//! - `POST /test/setup` — drop every scene, register the scenario
//!   bundle, prime initial state, install tokens.
//! - `POST /test/reset` — drop scenes + tokens.
//! - `GET /test/state` — return `{scene_id, scene_version, state}`.
//! - `POST /test/emit {patches}` — apply a delta to the active scene.
//! - `GET /test/health` — liveness.
//!
//! Mount with [`router`].

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, body::Body};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{Identity, MapAuthenticator};
use crate::input::{InputSpec, InputType};
use crate::role::Role;
use crate::server::ServerHandle;
use lumencast_protocol::types::{SceneId, SceneVersion};

/// Control-plane version reported by `/test/health`.
pub const CONTROL_PLANE_VERSION: u32 = 1;

/// Server identifier reported by `/test/health`.
pub const SERVER_ID: &str = "lumencast-rs";

/// Shared state passed to every endpoint handler.
#[derive(Clone)]
pub struct TestControlState {
    /// Server-side scene management.
    pub server: ServerHandle,
    /// Token store. Must be the **same** instance the running server
    /// authenticates against.
    pub auth: MapAuthenticator,
    /// WebSocket URL the harness should dial after `setup` succeeds.
    pub ws_url: String,
}

/// Build the axum router for the control plane.
pub fn router(state: TestControlState) -> Router {
    Router::new()
        .route("/test/setup", post(handle_setup))
        .route("/test/reset", post(handle_reset))
        .route("/test/state", get(handle_state))
        .route("/test/emit", post(handle_emit))
        .route("/test/health", get(handle_health))
        .with_state(Arc::new(state))
}

// --- /test/setup ----------------------------------------------------

#[derive(Debug, Deserialize)]
struct SetupRequest {
    #[serde(default)]
    scenario: Option<String>,
    #[serde(default, deserialize_with = "deser_btreemap_or_null")]
    tokens: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deser_vec_or_null")]
    bundles: Vec<SetupBundle>,
    #[serde(default, deserialize_with = "deser_btreemap_or_null")]
    initial_state: BTreeMap<String, Value>,
}

/// Accept `null` as an empty `BTreeMap`. The cross-language harness
/// emits `"initial_state": null` for scenarios with no seeded state ;
/// stricter SDKs reject it as 422 otherwise.
fn deser_btreemap_or_null<'de, D, T>(de: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<BTreeMap<String, T>>::deserialize(de)?.unwrap_or_default())
}

/// Accept `null` as an empty `Vec`.
fn deser_vec_or_null<'de, D, T>(de: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(de)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct SetupBundle {
    id: String,
    hash: String,
    /// Inline bundle JSON. Accepted but currently ignored — we pin
    /// the declared `hash` per CONTROL.md without recomputing.
    #[serde(default)]
    #[allow(dead_code)]
    inline: Option<Value>,
}

#[derive(Debug, Serialize)]
struct SetupResponse {
    ws_url: String,
    scene_id: String,
    scene_version: String,
}

async fn handle_setup(
    State(state): State<Arc<TestControlState>>,
    Json(req): Json<SetupRequest>,
) -> Response {
    state.server.clear_scenes();
    state.auth.clear();
    install_tokens(&state.auth, &req.tokens);

    let Some(bundle) = req.bundles.first() else {
        return problem(
            StatusCode::BAD_REQUEST,
            "missing-bundle",
            "setup requires at least one bundle",
        );
    };
    if bundle.id.is_empty() {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid-bundle",
            "bundle id is empty",
        );
    }
    if !bundle.hash.starts_with("sha256:") {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid-hash",
            "bundle.hash must be sha256-prefixed",
        );
    }

    // `inline.scene_id` may override `bundle.id` per the Go reference
    // (`extractInputSpecs`). When absent, fall back to the wrapper id.
    let inline = bundle.inline.as_ref();
    let effective_id: String = inline
        .and_then(|v| v.get("scene_id").and_then(Value::as_str))
        .map_or_else(|| bundle.id.clone(), str::to_string);

    let scene = match state.server.new_scene_with_version(
        SceneId::from(effective_id.as_str()),
        SceneVersion::from(bundle.hash.clone()),
    ) {
        Ok(s) => s,
        Err(e) => {
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "scene-register-failed",
                &e.to_string(),
            );
        }
    };

    // Operator-input declarations: extract from `inline.operator_inputs`
    // and attach to the scene. Mirrors Go's `extractInputSpecs`.
    let mut applied_inputs = 0usize;
    if let Some(specs) = inline.and_then(extract_input_specs) {
        applied_inputs = specs.len();
        // `with_operator_inputs` consumes/returns the scene by value;
        // we only need the side-effect on the shared inner state via
        // its internal RwLock.
        let _kept = scene.clone().with_operator_inputs(specs);
    }

    // Seed: prefer `initial_state`; fall back to `inline.defaults`.
    if !req.initial_state.is_empty() {
        scene.seed(
            req.initial_state
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    } else if let Some(defaults) = inline.and_then(|v| v.get("defaults").and_then(Value::as_object))
    {
        scene.seed(defaults.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    let _ = state
        .server
        .set_active_scene(SceneId::from(effective_id.as_str()));

    tracing::info!(
        scenario = req.scenario.as_deref().unwrap_or("<unnamed>"),
        scene_id = %effective_id,
        scene_version = %bundle.hash,
        token_count = req.tokens.len(),
        operator_inputs = applied_inputs,
        "control-plane setup"
    );

    let body = SetupResponse {
        ws_url: state.ws_url.clone(),
        scene_id: effective_id,
        scene_version: bundle.hash.clone(),
    };
    (StatusCode::OK, Json(body)).into_response()
}

/// Mirror of Go's `extractInputSpecs`: read the `inline.operator_inputs`
/// array (LSML 1.0 §8 shape) and return a `Vec<InputSpec>`. Skips
/// malformed entries silently — the Go server does the same.
///
/// LSML 1.0 §8 nests constraints under a `constraints` object :
///
/// ```yaml
/// operator_inputs:
///   - path: "__inputs.title"
///     type: string
///     constraints: { maxLength: 5 }
/// ```
///
/// The Rust `InputSpec` keeps the constraints flat for ergonomics, so
/// we parse the `path` / `type` fields directly and project
/// `constraints.maxLength` / `min` / `max` / `values` onto the spec.
fn extract_input_specs(inline: &Value) -> Option<Vec<InputSpec>> {
    let arr = inline.get("operator_inputs")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(path) = obj.get("path").and_then(Value::as_str) else {
            continue;
        };
        let mut spec = InputSpec::new(path);
        if let Some(kind_str) = obj.get("type").and_then(Value::as_str)
            && let Ok(kind) = serde_json::from_value::<InputType>(Value::String(kind_str.into()))
        {
            spec = spec.kind(kind);
        }
        if let Some(constraints) = obj.get("constraints").and_then(Value::as_object) {
            if let Some(max) = constraints.get("maxLength").and_then(Value::as_u64)
                && let Ok(max) = u32::try_from(max)
            {
                spec = spec.max_length(max);
            }
            if let Some(min) = constraints.get("min").and_then(Value::as_f64) {
                spec = spec.min(min);
            }
            if let Some(max) = constraints.get("max").and_then(Value::as_f64) {
                spec = spec.max(max);
            }
            if let Some(values) = constraints.get("values").and_then(Value::as_array) {
                let mut vs = Vec::with_capacity(values.len());
                for v in values {
                    if let Some(s) = v.as_str() {
                        vs.push(s.to_string());
                    }
                }
                if !vs.is_empty() {
                    spec = spec.values(vs);
                }
            }
        }
        out.push(spec);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn install_tokens(auth: &MapAuthenticator, tokens: &BTreeMap<String, String>) {
    let mut auth = auth.clone();
    for (placeholder, value) in tokens {
        // $TOKEN_INVALID: never installed (CONTROL.md "Token vocabulary").
        if placeholder == "$TOKEN_INVALID" {
            continue;
        }
        let role = role_for_placeholder(placeholder);
        let Some(role) = role else { continue };
        auth.insert_identity(
            value.clone(),
            Identity {
                subject: placeholder.clone(),
                role,
                paths: None,
            },
        );
    }
}

fn role_for_placeholder(placeholder: &str) -> Option<Role> {
    match placeholder {
        "$TOKEN_OPERATOR" => Some(Role::Operator),
        "$TOKEN_VIEWER" => Some(Role::Viewer),
        "$TOKEN_SERVICE" => Some(Role::Service),
        "$TOKEN_TEST" => Some(Role::Test),
        _ => None,
    }
}

// --- /test/reset ----------------------------------------------------

async fn handle_reset(State(state): State<Arc<TestControlState>>) -> Response {
    state.server.clear_scenes();
    state.auth.clear();
    StatusCode::NO_CONTENT.into_response()
}

// --- /test/state ----------------------------------------------------

#[derive(Debug, Serialize)]
struct StateResponse {
    scene_id: String,
    scene_version: String,
    state: BTreeMap<String, Value>,
}

async fn handle_state(State(state): State<Arc<TestControlState>>) -> Response {
    let Some(active_id) = state.server.active_scene() else {
        return problem(
            StatusCode::CONFLICT,
            "no-active-scene",
            "no scene is currently active; call /test/setup first",
        );
    };
    let Some(scene) = state.server.scene(active_id.as_str()) else {
        return problem(
            StatusCode::CONFLICT,
            "active-scene-missing",
            "active scene reference is stale",
        );
    };
    let body = StateResponse {
        scene_id: scene.id().0.clone(),
        scene_version: scene.version().0.clone(),
        state: scene.snapshot_state().into_iter().collect(),
    };
    (StatusCode::OK, Json(body)).into_response()
}

// --- /test/emit -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct EmitRequest {
    patches: Vec<EmitPatch>,
}

#[derive(Debug, Deserialize)]
struct EmitPatch {
    path: String,
    value: Value,
}

#[derive(Debug, Serialize)]
struct EmitErrorBody {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

async fn handle_emit(
    State(state): State<Arc<TestControlState>>,
    Json(req): Json<EmitRequest>,
) -> Response {
    let Some(active_id) = state.server.active_scene() else {
        return problem(
            StatusCode::CONFLICT,
            "no-active-scene",
            "no scene is currently active",
        );
    };
    let Some(scene) = state.server.scene(active_id.as_str()) else {
        return problem(
            StatusCode::CONFLICT,
            "active-scene-missing",
            "active scene reference is stale",
        );
    };
    if req.patches.is_empty() {
        return (StatusCode::NO_CONTENT, ()).into_response();
    }

    let pairs: Vec<(String, Value)> = req.patches.into_iter().map(|p| (p.path, p.value)).collect();

    if let Err(e) = scene.emit(pairs) {
        let code = match &e {
            crate::error::ServerError::InvalidValue(_) | crate::error::ServerError::Protocol(_) => {
                "INVALID_VALUE"
            }
            _ => "INTERNAL",
        };
        let body = EmitErrorBody {
            code,
            message: e.to_string(),
            path: None,
        };
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// --- /test/health ---------------------------------------------------

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    control_plane_version: u32,
    server: &'static str,
}

async fn handle_health() -> Response {
    let body = HealthResponse {
        status: "ok",
        control_plane_version: CONTROL_PLANE_VERSION,
        server: SERVER_ID,
    };
    (StatusCode::OK, Json(body)).into_response()
}

// --- helpers --------------------------------------------------------

fn problem(status: StatusCode, code: &str, detail: &str) -> Response {
    let body = serde_json::json!({
        "type": format!("https://lumencast.dev/problems/{code}"),
        "title": code,
        "status": status.as_u16(),
        "detail": detail,
    });
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    Response::builder()
        .status(status)
        .header("content-type", "application/problem+json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| status.into_response())
}
