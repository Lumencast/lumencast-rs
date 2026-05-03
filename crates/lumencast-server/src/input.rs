//! Operator-input declarations and per-spec constraint validation
//! (LSML 1.0 §8 + LSDP/1 §4.2).
//!
//! A [`Scene`](crate::Scene) registered with a non-empty
//! `declared_inputs` set rejects any `input` patch whose path is not
//! declared (`UNKNOWN_PATH`) or whose value violates the spec
//! constraints (`INVALID_VALUE`). Mirrors the Go SDK's
//! `server.InputSpec` exactly so cross-language conformance scenarios
//! match byte-for-byte on the resulting error frames.

use lumencast_protocol::LeafPath;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// LSML 1.0 §8 input type tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputType {
    /// `"string"`.
    String,
    /// `"number"`.
    Number,
    /// `"boolean"`.
    Boolean,
    /// `"enum"`.
    Enum,
    /// `"color"` — CSS-style color string.
    Color,
    /// `"date"`.
    Date,
    /// `"time"`.
    Time,
    /// `"path-ref"` — reference to a leaf path.
    PathRef,
    /// `"image-ref"` — reference to a bundled image asset.
    ImageRef,
}

/// One declaration in a scene's `operator_inputs` array.
///
/// Mirrors `server.InputSpec` from `lumencast-go`. `kind` is optional
/// because some scenarios declare only the path (no constraints) —
/// matches Go's `WithDeclaredInputs` shorthand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputSpec {
    /// Target leaf path (must live under `__inputs.*`).
    pub path: LeafPath,
    /// Type tag. `None` means "declared but untyped" — only path
    /// declaredness is checked.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<InputType>,
    /// `string`-only: maximum length in **chars** (not bytes), per
    /// the Go reference implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// `number`-only: inclusive minimum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// `number`-only: inclusive maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// `enum`-only: allowed values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

impl InputSpec {
    /// Build a minimal spec — `path` only, no constraints.
    pub fn new(path: impl Into<LeafPath>) -> Self {
        Self {
            path: path.into(),
            kind: None,
            max_length: None,
            min: None,
            max: None,
            values: Vec::new(),
        }
    }

    /// Set the type tag.
    #[must_use]
    pub fn kind(mut self, kind: InputType) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Set the `max_length` constraint (`string` type).
    #[must_use]
    pub fn max_length(mut self, max: u32) -> Self {
        self.max_length = Some(max);
        self
    }

    /// Set the `min` constraint (`number` type).
    #[must_use]
    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    /// Set the `max` constraint (`number` type).
    #[must_use]
    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// Set the `enum` allowed values.
    #[must_use]
    pub fn values(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.values = values.into_iter().map(Into::into).collect();
        self
    }
}

/// Run the per-spec constraint check on `value`. Returns
/// `Some(message)` describing the violation when invalid, `None`
/// when valid.
///
/// Mirrors the Go `checkConstraint` function: shipped constraints are
/// `max_length` (string), `min`/`max` (number), and `values` (enum).
/// Other types currently fall through with no enforcement; future
/// LSML constraints land here without breaking the call site.
#[must_use]
pub fn check_constraint(spec: &InputSpec, value: &Value) -> Option<String> {
    if let Some(kind) = spec.kind
        && let Some(message) = check_type(kind, value)
    {
        return Some(message);
    }
    match spec.kind {
        Some(InputType::String) => check_string(spec, value),
        Some(InputType::Number) => check_number(spec, value),
        Some(InputType::Enum) => check_enum(spec, value),
        // Boolean / Color / Date / Time / PathRef / ImageRef:
        // type-tag check is enough for v1; richer validation is
        // tracked at the spec level.
        _ => None,
    }
}

fn check_type(kind: InputType, value: &Value) -> Option<String> {
    let ok = match kind {
        InputType::String
        | InputType::Color
        | InputType::Date
        | InputType::Time
        | InputType::PathRef
        | InputType::ImageRef
        | InputType::Enum => value.is_string(),
        InputType::Number => value.is_number(),
        InputType::Boolean => value.is_boolean(),
    };
    if ok {
        None
    } else {
        Some(format!(
            "expected {kind:?} type, got {}",
            json_kind_label(value)
        ))
    }
}

fn check_string(spec: &InputSpec, value: &Value) -> Option<String> {
    let s = value.as_str()?;
    if let Some(max) = spec.max_length {
        let count = s.chars().count();
        if count > max as usize {
            return Some(format!("string length {count} exceeds max_length {max}"));
        }
    }
    None
}

fn check_number(spec: &InputSpec, value: &Value) -> Option<String> {
    let n = value.as_f64()?;
    if let Some(min) = spec.min
        && n < min
    {
        return Some(format!("value {n} below min {min}"));
    }
    if let Some(max) = spec.max
        && n > max
    {
        return Some(format!("value {n} above max {max}"));
    }
    None
}

fn check_enum(spec: &InputSpec, value: &Value) -> Option<String> {
    let s = value.as_str()?;
    if spec.values.is_empty() {
        return None;
    }
    if !spec.values.iter().any(|v| v == s) {
        return Some(format!("value {s:?} not in enum {:?}", spec.values));
    }
    None
}

fn json_kind_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_max_length_enforced() {
        let spec = InputSpec::new(LeafPath::from("__inputs.title"))
            .kind(InputType::String)
            .max_length(5);
        assert!(check_constraint(&spec, &json!("ok")).is_none());
        assert!(check_constraint(&spec, &json!("hello")).is_none());
        let err = check_constraint(&spec, &json!("hello!")).unwrap();
        assert!(err.contains("max_length"));
    }

    #[test]
    fn string_max_length_counts_chars_not_bytes() {
        let spec = InputSpec::new(LeafPath::from("__inputs.title"))
            .kind(InputType::String)
            .max_length(3);
        // "héllo" has 5 chars; "écu" has 3.
        assert!(check_constraint(&spec, &json!("écu")).is_none());
        assert!(check_constraint(&spec, &json!("héllo")).is_some());
    }

    #[test]
    fn number_range() {
        let spec = InputSpec::new(LeafPath::from("__inputs.score"))
            .kind(InputType::Number)
            .min(0.0)
            .max(100.0);
        assert!(check_constraint(&spec, &json!(50)).is_none());
        assert!(check_constraint(&spec, &json!(0)).is_none());
        assert!(check_constraint(&spec, &json!(100)).is_none());
        assert!(check_constraint(&spec, &json!(-1)).is_some());
        assert!(check_constraint(&spec, &json!(101)).is_some());
    }

    #[test]
    fn enum_membership() {
        let spec = InputSpec::new(LeafPath::from("__inputs.theme"))
            .kind(InputType::Enum)
            .values(["dark", "light", "high-contrast"]);
        assert!(check_constraint(&spec, &json!("dark")).is_none());
        assert!(check_constraint(&spec, &json!("blue")).is_some());
    }

    #[test]
    fn type_mismatch_caught_first() {
        let spec = InputSpec::new(LeafPath::from("__inputs.score")).kind(InputType::Number);
        assert!(check_constraint(&spec, &json!("nope")).is_some());
    }

    #[test]
    fn untyped_spec_accepts_anything() {
        let spec = InputSpec::new(LeafPath::from("__inputs.x"));
        assert!(check_constraint(&spec, &json!("string")).is_none());
        assert!(check_constraint(&spec, &json!(42)).is_none());
        assert!(check_constraint(&spec, &json!(true)).is_none());
    }
}
