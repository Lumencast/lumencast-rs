//! HTTP route serving LSML bundles at content-addressed URLs.
//!
//! `GET /scenes/:scene_id/:version_hex` — returns the canonical bundle
//! JSON for the scene if `version_hex` matches its current
//! [`SceneVersion`]. Cache-friendly headers are set so CDNs and
//! browsers can cache forever (the URL is content-addressed and
//! immutable).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::server::ServerInner;

pub(crate) async fn scene_bundle_route(
    State(inner): State<Arc<ServerInner>>,
    Path((scene_id, version_hex)): Path<(String, String)>,
) -> Response {
    let Some(scene) = inner.scene(&scene_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let expected = format!("sha256:{version_hex}");
    if scene.version().as_str() != expected {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(bytes) = scene.bundle_bytes() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let body: Vec<u8> = (*bytes).clone();
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

/// Convenience: turn a body of bytes into an axum response without
/// going through `IntoResponse` to allow pre-set headers.
#[allow(dead_code)]
fn ok_with_body(bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from(bytes))
        .expect("static response builder")
}
