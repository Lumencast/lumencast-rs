//! Scenario types and YAML deserialisation.
//!
//! Mirrors the shape published in `lumencast-protocol/conformance/v1/`.
//! Unknown fields are accepted (`#[serde(other)]` on step kinds) so
//! forward-compatible scenarios still load.

#![allow(missing_docs)]

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
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

/// One step in a scenario. The YAML shape follows
/// [`SCENARIO-FORMAT.md`](https://github.com/Lumencast/lumencast-protocol/blob/main/conformance/v1/SCENARIO-FORMAT.md)
/// — every step is a map with a `kind:` discriminator and per-kind
/// body fields at the same level :
///
/// ```yaml
/// - kind: client-sends
///   frame: { v: 1, type: subscribe }
/// ```
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
    /// Trigger a server-side delta via `POST /test/emit`, then validate
    /// the wire frame matches the expected `frame` (which carries the
    /// patches inline per SCENARIO-FORMAT.md § server-emits).
    ServerEmits { frame: Value },
    /// Step kind not handled by this server-target harness. Typically
    /// runtime-only verbs (`client-action`, future extensions). Lazy
    /// — only fails the run when the scenario actually executes ;
    /// runtime-target scenarios are auto-skipped before reaching it.
    Unsupported {
        kind: String,
        #[allow(dead_code)]
        body: BTreeMap<String, Value>,
    },
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: BTreeMap<String, Value> = BTreeMap::deserialize(deserializer)?;
        let kind = raw
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("step is missing required `kind` field"))?
            .to_string();
        step_from_kind(&kind, &raw).map_err(de::Error::custom)
    }
}

fn step_from_kind(kind: &str, raw: &BTreeMap<String, Value>) -> Result<Step, String> {
    fn frame_field(raw: &BTreeMap<String, Value>) -> Result<Value, String> {
        raw.get("frame")
            .cloned()
            .ok_or_else(|| "step missing `frame` field".to_string())
    }
    fn state_field(raw: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, String> {
        let v = raw
            .get("state")
            .cloned()
            .ok_or_else(|| "step missing `state` field".to_string())?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    match kind {
        "client-sends" => Ok(Step::ClientSends {
            frame: frame_field(raw)?,
        }),
        "server-sends" => Ok(Step::ServerSends {
            frame: frame_field(raw)?,
        }),
        "server-emits" => Ok(Step::ServerEmits {
            frame: frame_field(raw)?,
        }),
        "expect-runtime-state" => Ok(Step::ExpectRuntimeState {
            state: state_field(raw)?,
        }),
        "expect-server-state" => Ok(Step::ExpectServerState {
            state: state_field(raw)?,
        }),
        "expect-no-frame-for" => {
            let duration_ms = raw
                .get("duration_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| "expect-no-frame-for missing `duration_ms`".to_string())?;
            Ok(Step::ExpectNoFrameFor { duration_ms })
        }
        "expect-client-action" => {
            let mut body = serde_json::Map::new();
            for (k, v) in raw {
                if k == "kind" {
                    continue;
                }
                body.insert(k.clone(), v.clone());
            }
            let action: ClientAction =
                serde_json::from_value(Value::Object(body)).map_err(|e| e.to_string())?;
            Ok(Step::ExpectClientAction(action))
        }
        other => {
            // Unknown step kind — keep the body around so we can
            // surface a useful error if the scenario actually runs,
            // but don't reject at parse time. Runtime-target
            // scenarios are auto-skipped, so unfamiliar verbs they
            // contain (`client-action`, future extensions) are
            // tolerated.
            let mut body = raw.clone();
            body.remove("kind");
            Ok(Step::Unsupported {
                kind: other.to_string(),
                body,
            })
        }
    }
}

/// One patch carried by `server-emits`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitPatch {
    pub path: String,
    pub value: Value,
}

/// Body of `expect-client-action`.
///
/// The set of `action` names is open-ended at the spec level — new
/// actions may be added without breaking older harnesses (e.g.
/// `fetch-bundle`, `onError`). We therefore parse the action verb as
/// a string and keep the rest of the body as a generic map ; runtime
/// dispatch decides whether a given action is supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientAction {
    /// Action verb (`close-with-reason`, `reconnect`, `fetch-bundle`,
    /// `onError`, …). Unknown verbs are not a parse error — they
    /// surface as runtime "unsupported" failures only if the
    /// containing scenario actually executes (target=server skips
    /// runtime-only scenarios before they reach this step).
    pub action: String,
    /// Arbitrary additional fields per the action's contract.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
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
  - kind: client-sends
    frame:
      v: 1
      type: subscribe
      token: $TOKEN_OPERATOR
      scene: t
  - kind: server-sends
    frame:
      v: 1
      type: snapshot
      seq: 1
      scene_id: t
      scene_version: $BUNDLE.t.hash
      state:
        title: Hello
        count: 0
  - kind: expect-runtime-state
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
  - kind: expect-no-frame-for
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
  - kind: expect-client-action
    action: close-with-reason
    reason: VERSION_GAP
  - kind: expect-client-action
    action: reconnect
";
        let scenario = Scenario::parse(yaml).expect("parses");
        match &scenario.steps[0] {
            Step::ExpectClientAction(action) => {
                assert_eq!(action.action, "close-with-reason");
                assert_eq!(
                    action.fields.get("reason").and_then(Value::as_str),
                    Some("VERSION_GAP")
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
        match &scenario.steps[1] {
            Step::ExpectClientAction(action) => {
                assert_eq!(action.action, "reconnect");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
