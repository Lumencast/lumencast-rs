//! Scenario types and YAML deserialisation.
//!
//! Mirrors the shape published in `lumencast-protocol/conformance/v1/`.
//! Unknown fields are accepted (`#[serde(other)]` on step kinds) so
//! forward-compatible scenarios still load.

#![allow(missing_docs)]

use std::collections::BTreeMap;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tag attached to a scenario; drives the `--tag required` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tag {
    /// Spec compliance — must pass.
    Required,
    /// Quality / edge cases — tracked, not blocking.
    Recommended,
    /// Performance / large state — best effort.
    Extended,
}

/// What end of the protocol the scenario exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    /// Drives the server.
    Server,
    /// Drives the runtime/client.
    Runtime,
    /// Either side (most scenarios).
    Any,
}

/// One inline bundle declared inside a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineBundle {
    /// Scene id (also the bundle id).
    pub id: String,
    /// Inline JSON body the harness will canonical-hash to populate
    /// `$BUNDLE.<id>.hash` placeholders.
    pub inline: Value,
}

/// Scenario document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Stable name (= filename stem).
    pub name: String,

    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Tag (required / recommended / extended).
    #[serde(default = "default_tag")]
    pub tag: Tag,

    /// Side under test.
    #[serde(default = "default_target")]
    pub target: Target,

    /// Bundles declared inline.
    #[serde(default)]
    pub bundles: Vec<InlineBundle>,

    /// Initial scene state for `setup`.
    #[serde(default)]
    pub initial_state: BTreeMap<String, Value>,

    /// Steps, in order.
    pub steps: Vec<Step>,
}

fn default_tag() -> Tag {
    Tag::Required
}
fn default_target() -> Target {
    Target::Any
}

/// One step in a scenario. Each step is one YAML map with exactly one
/// of the following keys (custom `Deserialize` so the YAML stays
/// natural — `- client-sends: {frame: ...}` rather than a tagged enum).
#[derive(Debug, Clone, Serialize)]
pub enum Step {
    /// Harness sends a JSON frame to the server.
    ClientSends { frame: Value },
    /// Harness reads a frame and structurally matches it.
    ServerSends { frame: Value },
    /// Assert the harness's shadow state.
    ExpectRuntimeState { state: BTreeMap<String, Value> },
    /// Assert the server's authoritative state via `GET /test/state`.
    ExpectServerState { state: BTreeMap<String, Value> },
    /// Assert no frame arrives within `duration_ms`.
    ExpectNoFrameFor { duration_ms: u64 },
    /// Assert a particular client behaviour.
    ExpectClientAction(ClientAction),
    /// Schedule a server-side delta via `POST /test/emit`.
    ServerEmits { patches: Vec<EmitPatch> },
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StepVisitor;

        impl<'de> Visitor<'de> for StepVisitor {
            type Value = Step;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a single-key map { kebab-step-name: body }")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Step, M::Error> {
                let key: String = map
                    .next_key()?
                    .ok_or_else(|| de::Error::custom("step must have one key"))?;
                let body: Value = map.next_value()?;
                if map.next_key::<String>()?.is_some() {
                    return Err(de::Error::custom(format!(
                        "step {key:?}: extra keys forbidden"
                    )));
                }
                step_from_body(&key, body).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_map(StepVisitor)
    }
}

fn step_from_body(key: &str, body: Value) -> Result<Step, String> {
    fn take<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    match key {
        "client-sends" => {
            #[derive(Deserialize)]
            struct B {
                frame: Value,
            }
            let b: B = take(body)?;
            Ok(Step::ClientSends { frame: b.frame })
        }
        "server-sends" => {
            #[derive(Deserialize)]
            struct B {
                frame: Value,
            }
            let b: B = take(body)?;
            Ok(Step::ServerSends { frame: b.frame })
        }
        "expect-runtime-state" => {
            #[derive(Deserialize)]
            struct B {
                state: BTreeMap<String, Value>,
            }
            let b: B = take(body)?;
            Ok(Step::ExpectRuntimeState { state: b.state })
        }
        "expect-server-state" => {
            #[derive(Deserialize)]
            struct B {
                state: BTreeMap<String, Value>,
            }
            let b: B = take(body)?;
            Ok(Step::ExpectServerState { state: b.state })
        }
        "expect-no-frame-for" => {
            #[derive(Deserialize)]
            struct B {
                duration_ms: u64,
            }
            let b: B = take(body)?;
            Ok(Step::ExpectNoFrameFor {
                duration_ms: b.duration_ms,
            })
        }
        "expect-client-action" => {
            let action: ClientAction = take(body)?;
            Ok(Step::ExpectClientAction(action))
        }
        "server-emits" => {
            #[derive(Deserialize)]
            struct B {
                patches: Vec<EmitPatch>,
            }
            let b: B = take(body)?;
            Ok(Step::ServerEmits { patches: b.patches })
        }
        other => Err(format!("unknown step kind {other:?}")),
    }
}

/// One patch carried by `server-emits`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitPatch {
    pub path: String,
    pub value: Value,
}

/// Body of `expect-client-action`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum ClientAction {
    /// The client closed the connection with a specific reason.
    CloseWithReason { reason: String },
    /// The client opened a fresh connection.
    Reconnect,
}

/// Failure raised while parsing a scenario file.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioParseError {
    /// YAML parse failure.
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    /// IO failure (only when loading from disk).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl Scenario {
    /// Parse a scenario from a YAML string.
    pub fn parse(text: &str) -> Result<Self, ScenarioParseError> {
        Ok(serde_yaml_ng::from_str(text)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_scenario() {
        let yaml = r"
name: subscribe-snapshot-delta
description: Reference scenario from CONTROL.md
tag: required
target: any
bundles:
  - id: t
    inline:
      v: 1
      kind: frame
      id: t
      state:
        title: Hello
        count: 0
initial_state:
  title: Hello
  count: 0
steps:
  - client-sends:
      frame:
        v: 1
        type: subscribe
        token: $TOKEN_OPERATOR
        scene: t
  - server-sends:
      frame:
        v: 1
        type: snapshot
        seq: 1
        scene_id: t
        scene_version: $BUNDLE.t.hash
        state:
          title: Hello
          count: 0
  - expect-runtime-state:
      state:
        title: Hello
        count: 0
";
        let scenario = Scenario::parse(yaml).expect("parses");
        assert_eq!(scenario.name, "subscribe-snapshot-delta");
        assert_eq!(scenario.tag, Tag::Required);
        assert_eq!(scenario.bundles.len(), 1);
        assert_eq!(scenario.steps.len(), 3);
    }

    #[test]
    fn parses_expect_no_frame_for() {
        let yaml = r"
name: rate-limit
steps:
  - expect-no-frame-for:
      duration_ms: 250
";
        let scenario = Scenario::parse(yaml).expect("parses");
        match &scenario.steps[0] {
            Step::ExpectNoFrameFor { duration_ms } => assert_eq!(*duration_ms, 250),
            other => panic!("unexpected step: {other:?}"),
        }
    }

    #[test]
    fn parses_expect_client_action() {
        let yaml = r"
name: gap-reconnect
steps:
  - expect-client-action:
      action: close-with-reason
      reason: VERSION_GAP
  - expect-client-action:
      action: reconnect
";
        let scenario = Scenario::parse(yaml).expect("parses");
        match &scenario.steps[0] {
            Step::ExpectClientAction(ClientAction::CloseWithReason { reason }) => {
                assert_eq!(reason, "VERSION_GAP");
            }
            other => panic!("unexpected: {other:?}"),
        }
        match &scenario.steps[1] {
            Step::ExpectClientAction(ClientAction::Reconnect) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
