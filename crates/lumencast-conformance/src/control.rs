//! HTTP control-plane client (LSDP/1 interop `CONTROL.md`).

#![allow(missing_docs)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Failure raised by the control client.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("control plane returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("missing field in response: {0}")]
    Missing(&'static str),
}

/// Body of `POST /test/setup`.
#[derive(Debug, Clone, Serialize)]
pub struct SetupRequest {
    pub scenario: String,
    pub tokens: BTreeMap<String, String>,
    pub bundles: Vec<SetupBundle>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub initial_state: BTreeMap<String, Value>,
}

/// One entry in `setup.bundles`.
#[derive(Debug, Clone, Serialize)]
pub struct SetupBundle {
    pub id: String,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline: Option<Value>,
}

/// Body of `setup` response.
#[derive(Debug, Clone, Deserialize)]
pub struct SetupResponse {
    pub ws_url: String,
    pub scene_id: String,
    pub scene_version: String,
}

/// Body of `GET /test/state`.
#[derive(Debug, Clone, Deserialize)]
pub struct StateResponse {
    pub scene_id: String,
    pub scene_version: String,
    pub state: BTreeMap<String, Value>,
}

/// Body of `GET /test/health`.
#[derive(Debug, Clone, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub control_plane_version: u32,
    #[serde(default)]
    pub server: Option<String>,
}

/// HTTP client for the test control plane.
pub struct ControlClient {
    base: String,
    http: reqwest::Client,
}

impl ControlClient {
    /// New client pointing at `base` (e.g. `http://127.0.0.1:9000`).
    pub fn new(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base: base.into(),
            http,
        }
    }

    /// `POST /test/setup`.
    pub async fn setup(&self, body: &SetupRequest) -> Result<SetupResponse, ControlError> {
        let resp = self
            .http
            .post(format!("{}/test/setup", self.base))
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ControlError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json().await?)
    }

    /// `POST /test/reset`.
    pub async fn reset(&self) -> Result<(), ControlError> {
        let resp = self
            .http
            .post(format!("{}/test/reset", self.base))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ControlError::Status { status, body });
        }
        Ok(())
    }

    /// `GET /test/state`.
    pub async fn state(&self) -> Result<StateResponse, ControlError> {
        let resp = self
            .http
            .get(format!("{}/test/state", self.base))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ControlError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json().await?)
    }

    /// `POST /test/emit`.
    pub async fn emit(&self, patches: Vec<(String, Value)>) -> Result<(), ControlError> {
        #[derive(Serialize)]
        struct Body {
            patches: Vec<Patch>,
        }
        #[derive(Serialize)]
        struct Patch {
            path: String,
            value: Value,
        }
        let body = Body {
            patches: patches
                .into_iter()
                .map(|(path, value)| Patch { path, value })
                .collect(),
        };
        let resp = self
            .http
            .post(format!("{}/test/emit", self.base))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ControlError::Status { status, body });
        }
        Ok(())
    }

    /// `GET /test/health`.
    pub async fn health(&self) -> Result<HealthResponse, ControlError> {
        let resp = self
            .http
            .get(format!("{}/test/health", self.base))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ControlError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json().await?)
    }
}
