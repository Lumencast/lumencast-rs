//! Public harness API: load scenarios, run them through a server,
//! report pass/fail.

#![allow(missing_docs)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::ControlClient;
use crate::player::run_scenario;
use crate::scenario::{Scenario, Tag};

/// Where the harness should drive its conformance run.
#[derive(Debug, Clone)]
pub enum Target {
    /// Drive a server reachable at `(ws_url, control_url)`.
    Server {
        /// LSDP/1 WebSocket URL (e.g. `ws://127.0.0.1:8081/lsdp.v1`).
        ws_url: String,
        /// HTTP control-plane base URL (e.g. `http://127.0.0.1:9000`).
        control_url: String,
    },
}

/// Configuration for one conformance run.
#[derive(Debug, Clone)]
pub struct Config {
    /// What we are testing.
    pub target: Target,
    /// Tag filter — empty means run everything.
    pub tags: Vec<Tag>,
    /// Optional single-scenario filter (matches by `scenario.name`).
    pub scenario_filter: Option<String>,
    /// Token vocabulary (placeholder → token-value), as produced from
    /// `interop/fixtures/canonical-tokens.json`.
    pub tokens: BTreeMap<String, String>,
    /// Either a directory of `.yaml` files, or an explicit list. If
    /// neither is set, the harness loads its built-in baseline (which
    /// is empty until the upstream suite ships scenarios — every run
    /// therefore returns 0 outcomes today).
    pub scenarios: ScenariosSource,
}

/// Where to load scenarios from.
#[derive(Debug, Clone, Default)]
pub enum ScenariosSource {
    /// Read every `*.yaml` file from this directory (non-recursive).
    Directory(PathBuf),
    /// Explicit list of (name, yaml-source) pairs — useful for tests.
    Inline(Vec<(String, String)>),
    /// No scenarios. The harness reports `Report { total: 0, … }`.
    #[default]
    Empty,
}

/// Outcome of a single scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    /// Scenario name.
    pub name: String,
    /// Whether the scenario passed.
    pub passed: bool,
    /// Whether the scenario was skipped (target mismatch, tag filter,
    /// unsupported step kind in a runtime-only path). When `true`, the
    /// scenario doesn't count toward the FAIL bucket — exit codes and
    /// matrix reports treat it as N/A.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
    /// Free-form failure / skip description.
    pub message: Option<String>,
}

/// Top-level report from a conformance run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Total scenarios executed.
    pub total: usize,
    /// How many passed.
    pub passed: usize,
    /// How many were skipped (auto-skip, tag filter, etc.).
    #[serde(default)]
    pub skipped: usize,
    /// Per-scenario outcomes, ordered by execution.
    pub outcomes: Vec<Outcome>,
}

impl Report {
    /// Returns `true` if every non-skipped scenario passed. Skipped
    /// scenarios (auto-skip on target mismatch, etc.) don't count
    /// against success — they're N/A, not failures.
    #[must_use]
    pub fn all_passed(&self) -> bool {
        self.passed + self.skipped == self.total
    }
}

/// Run the conformance suite.
pub async fn run(config: Config) -> Result<Report, HarnessError> {
    let scenarios = load_scenarios(&config.scenarios)?;
    let scenarios = filter_scenarios(scenarios, &config);
    let Target::Server { control_url, .. } = &config.target;
    let control = ControlClient::new(control_url.clone());

    let mut outcomes = Vec::with_capacity(scenarios.len());
    let mut passed = 0;
    let total = scenarios.len();

    let mut skipped = 0usize;
    for scenario in scenarios {
        // Auto-skip runtime-target scenarios — this harness drives a
        // server, not a runtime. Mirrors the Go SDK behaviour.
        if matches!(scenario.target, crate::scenario::Target::Runtime) {
            skipped += 1;
            outcomes.push(Outcome {
                name: scenario.name.clone(),
                passed: false,
                skipped: true,
                message: Some("runtime-targeted scenario, harness drives a server".to_string()),
            });
            continue;
        }
        match run_scenario(&scenario, &control, &config.tokens).await {
            Ok(()) => {
                passed += 1;
                outcomes.push(Outcome {
                    name: scenario.name.clone(),
                    passed: true,
                    skipped: false,
                    message: None,
                });
            }
            Err(e) => {
                // Best-effort cleanup so the next scenario starts fresh.
                let _ = control.reset().await;
                outcomes.push(Outcome {
                    name: scenario.name.clone(),
                    passed: false,
                    skipped: false,
                    message: Some(e.to_string()),
                });
            }
        }
    }

    Ok(Report {
        total,
        passed,
        skipped,
        outcomes,
    })
}

fn load_scenarios(source: &ScenariosSource) -> Result<Vec<Scenario>, HarnessError> {
    match source {
        ScenariosSource::Empty => Ok(Vec::new()),
        ScenariosSource::Inline(list) => list
            .iter()
            .map(|(_name, yaml)| Scenario::parse(yaml).map_err(HarnessError::Parse))
            .collect(),
        ScenariosSource::Directory(dir) => {
            let mut out = Vec::new();
            let read = std::fs::read_dir(dir)
                .map_err(|e| HarnessError::Driver(format!("cannot read {}: {e}", dir.display())))?;
            let mut paths: Vec<_> = read
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yaml"))
                .collect();
            paths.sort();
            for path in paths {
                let text = std::fs::read_to_string(&path).map_err(|e| {
                    HarnessError::Driver(format!("cannot read {}: {e}", path.display()))
                })?;
                let scenario = Scenario::parse(&text).map_err(HarnessError::Parse)?;
                out.push(scenario);
            }
            Ok(out)
        }
    }
}

fn filter_scenarios(scenarios: Vec<Scenario>, config: &Config) -> Vec<Scenario> {
    scenarios
        .into_iter()
        .filter(|s| config.tags.is_empty() || config.tags.contains(&s.tag))
        .filter(|s| {
            config
                .scenario_filter
                .as_ref()
                .is_none_or(|name| s.name == *name)
        })
        .collect()
}

/// Failures raised by the harness itself (not scenario failures).
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("parse: {0}")]
    Parse(crate::scenario::ScenarioParseError),

    #[error("driver: {0}")]
    Driver(String),
}
