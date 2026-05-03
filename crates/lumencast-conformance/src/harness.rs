//! Public harness API. Stable shape ahead of upstream scenarios landing.

use serde::{Deserialize, Serialize};

/// Where the harness should drive its conformance run.
#[derive(Debug, Clone)]
pub enum Target {
    /// Drive a server reachable at this WebSocket URL.
    Server(String),
    /// Drive a client process via the supplied command-line.
    Client(Vec<String>),
}

/// Configuration for one conformance run.
#[derive(Debug, Clone)]
pub struct Config {
    /// What we are testing.
    pub target: Target,
    /// Tag filter — `["required"]` is conformance, `["recommended"]` is
    /// quality, `["extended"]` is best-effort. Empty runs everything.
    pub tags: Vec<String>,
}

/// Outcome of a single scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioOutcome {
    /// Scenario name as declared in the upstream manifest.
    pub name: String,
    /// Whether the scenario passed.
    pub passed: bool,
    /// Free-form failure description, if any.
    pub message: Option<String>,
}

/// Top-level report from a conformance run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Total scenarios executed.
    pub total: usize,
    /// How many passed.
    pub passed: usize,
    /// Per-scenario outcomes, ordered by execution.
    pub outcomes: Vec<ScenarioOutcome>,
}

impl Report {
    /// Returns `true` if every scenario passed.
    #[must_use]
    pub fn all_passed(&self) -> bool {
        self.passed == self.total
    }
}

/// Run the conformance suite. Currently a no-op pending the upstream
/// scenarios — returns an empty [`Report`].
///
/// This function is `async` because future scenarios will involve a
/// live WebSocket dialogue against the target.
#[allow(clippy::unused_async)]
pub async fn run(_config: Config) -> Result<Report, HarnessError> {
    Ok(Report {
        total: 0,
        passed: 0,
        outcomes: Vec::new(),
    })
}

/// Failures raised by the harness itself (not scenario failures).
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// Could not parse the upstream manifest.
    #[error("manifest: {0}")]
    Manifest(String),

    /// Network or process error driving the target.
    #[error("driver: {0}")]
    Driver(String),
}
