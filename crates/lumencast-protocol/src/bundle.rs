//! LSML 1.0 scene bundle — parser, validator, and content-hash
//! computation.
//!
//! See [LSML 1.0 §3] for the canonicalization rules used to derive the
//! `scene_version` content hash.
//!
//! [LSML 1.0 §3]: https://github.com/Lumencast/lumencast-protocol/blob/main/spec/LSML-1.md#3-identity--content-addressing

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::types::{SceneId, SceneVersion};

/// LSML schema major version this crate accepts.
pub const SUPPORTED_LSML_MAJOR: u32 = 1;

/// Placeholder content hash used during canonical-form hashing
/// (`sha256:` followed by 64 zeroes).
pub fn placeholder_hash() -> String {
    let mut s = String::with_capacity("sha256:".len() + 64);
    s.push_str("sha256:");
    for _ in 0..64 {
        s.push('0');
    }
    s
}

/// Failure raised by the bundle module.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// JSON parse failure.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// Bundle is missing a required field or has the wrong shape.
    #[error("invalid bundle: {0}")]
    Invalid(String),

    /// Bundle declares an `lsml` major version this crate does not
    /// support (LSML 1.0 §2).
    #[error("incompatible LSML version: {got} (supported: 1.x)")]
    Incompatible {
        /// Version string from the bundle.
        got: String,
    },

    /// `scene_version` does not match the canonical content hash.
    #[error("content hash mismatch: bundle declares {declared}, computed {computed}")]
    HashMismatch {
        /// Hash declared in the bundle.
        declared: String,
        /// Hash actually computed from the canonical form.
        computed: String,
    },
}

/// One row of a bundle's `operator_inputs` array (LSML 1.0 §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorInput {
    /// Target leaf path (must live under the `__inputs.*` namespace).
    pub path: String,
    /// Human-readable label for the operator UI.
    pub label: String,
    /// Type tag — `"string"`, `"number"`, `"boolean"`, `"enum"`, …
    #[serde(rename = "type")]
    pub kind: String,
    /// Type-specific constraints (`maxLength`, `min`, `max`, `pattern`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Value>,
    /// Roles allowed to write — subset of `["operator", "service"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writable_by: Vec<String>,
    /// Optional grouping label for operator UI organisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// For `"enum"` typed inputs: the allowed values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Value>,
}

/// LSML 1.0 scene bundle.
///
/// Use [`Bundle::parse`] to read JSON, [`Bundle::validate`] to enforce
/// invariants, and [`Bundle::compute_content_hash`] to derive the
/// `sha256:` content hash that becomes its [`SceneVersion`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    /// LSML schema version (`"1.0"` for this spec).
    pub lsml: String,
    /// Stable scene identifier.
    pub scene_id: SceneId,
    /// Content hash. Set to [`placeholder_hash`] during canonical
    /// hashing.
    pub scene_version: SceneVersion,
    /// Root primitive node of the visual tree.
    pub layout: Value,
    /// Operator-controllable inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operator_inputs: Vec<OperatorInput>,
    /// Server-side adapter declarations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_adapters: Vec<Value>,
    /// Initial `path → value` map for paths that no adapter or input
    /// otherwise sets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub defaults: BTreeMap<String, Value>,
    /// Asset URL allow-list and integrity hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<Value>,
    /// Internationalisation table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n: Option<Value>,
    /// Authoring metadata (not consumed by runtimes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl Bundle {
    /// Parse a bundle from a JSON byte slice.
    pub fn parse(bytes: &[u8]) -> Result<Self, BundleError> {
        let bundle: Bundle = serde_json::from_slice(bytes)?;
        bundle.validate()?;
        Ok(bundle)
    }

    /// Parse a bundle from a JSON string.
    pub fn parse_str(s: &str) -> Result<Self, BundleError> {
        Self::parse(s.as_bytes())
    }

    /// Enforce static invariants required by LSML 1.x:
    ///
    /// - `lsml` major version must be supported (LSML 1.0 §2)
    /// - `scene_id` is non-empty
    /// - Every `operator_inputs.path` lives under `__inputs.*` (§8)
    /// - `animate` blocks only target `transition`, `transform`,
    ///   `opacity`, or `filter` (§6)
    /// - Every `image` primitive declares `alt` (§13)
    pub fn validate(&self) -> Result<(), BundleError> {
        let major = parse_major(&self.lsml).ok_or_else(|| {
            BundleError::Invalid(format!("malformed `lsml` version: {:?}", self.lsml))
        })?;
        if major != SUPPORTED_LSML_MAJOR {
            return Err(BundleError::Incompatible {
                got: self.lsml.clone(),
            });
        }
        if self.scene_id.as_str().is_empty() {
            return Err(BundleError::Invalid("scene_id MUST NOT be empty".into()));
        }
        for op in &self.operator_inputs {
            if !(op.path == "__inputs" || op.path.starts_with("__inputs.")) {
                return Err(BundleError::Invalid(format!(
                    "operator_inputs[{:?}].path MUST live under `__inputs.*`",
                    op.path
                )));
            }
        }
        check_layout_invariants(&self.layout)?;
        Ok(())
    }

    /// Returns `true` if `scene_version` matches the canonical content
    /// hash of this bundle.
    pub fn is_self_consistent(&self) -> Result<bool, BundleError> {
        let computed = self.compute_content_hash()?;
        Ok(computed == self.scene_version)
    }

    /// Re-derive [`Bundle::scene_version`] from the canonical form
    /// (LSML 1.0 §3).
    pub fn compute_content_hash(&self) -> Result<SceneVersion, BundleError> {
        let bytes = self.to_canonical_bytes_for_hashing()?;
        Ok(hash_to_scene_version(&bytes))
    }

    /// Set `scene_version` to its canonical hash and return self.
    pub fn with_computed_version(mut self) -> Result<Self, BundleError> {
        self.scene_version = self.compute_content_hash()?;
        Ok(self)
    }

    /// Serialise this bundle in canonical JSON form (sorted keys, no
    /// insignificant whitespace) **with `scene_version` left as
    /// declared**. Use this to serve the bundle over HTTP — clients
    /// will hash it themselves to verify integrity.
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, BundleError> {
        let value = serde_json::to_value(self)?;
        Ok(serde_json::to_vec(&value)?)
    }

    fn to_canonical_bytes_for_hashing(&self) -> Result<Vec<u8>, BundleError> {
        let mut value = serde_json::to_value(self)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "scene_version".to_string(),
                Value::String(placeholder_hash()),
            );
        }
        Ok(serde_json::to_vec(&value)?)
    }
}

fn parse_major(lsml: &str) -> Option<u32> {
    lsml.split('.').next()?.parse().ok()
}

/// Allowed top-level keys inside an `animate` block (LSML 1.0 §6).
const ANIMATE_ALLOWED: &[&str] = &["transition", "transform", "opacity", "filter"];

/// Closed catalogue of LSML 1.0 primitive kinds (§4).
const PRIMITIVE_KINDS: &[&str] = &[
    "stack", "grid", "frame", "text", "image", "shape", "media", "repeat",
];

/// Walk the layout subtree iteratively (children + repeat templates)
/// and check the LSML 1.0 invariants on each node.
fn check_layout_invariants(root: &Value) -> Result<(), BundleError> {
    let mut stack: Vec<&Value> = vec![root];
    while let Some(node) = stack.pop() {
        let Some(obj) = node.as_object() else {
            continue;
        };

        // Closed catalogue (§4): every node MUST declare a known `kind`.
        let Some(kind) = obj.get("kind").and_then(Value::as_str) else {
            return Err(BundleError::Invalid(
                "primitive node MUST declare a `kind` field".into(),
            ));
        };
        if !PRIMITIVE_KINDS.contains(&kind) {
            return Err(BundleError::Invalid(format!(
                "unknown primitive kind {kind:?} (LSML 1.0 catalogue: {})",
                PRIMITIVE_KINDS.join(", ")
            )));
        }

        // Animation discipline (§6): reject non-allowed properties under `animate`.
        if let Some(animate) = obj.get("animate").and_then(Value::as_object) {
            for key in animate.keys() {
                if !ANIMATE_ALLOWED.contains(&key.as_str()) {
                    return Err(BundleError::Invalid(format!(
                        "animate.{key} is not animatable (allowed: {})",
                        ANIMATE_ALLOWED.join(", ")
                    )));
                }
            }
        }

        // Per-kind structural checks (§4).
        check_per_kind(kind, obj)?;

        if let Some(children) = obj.get("children").and_then(Value::as_array) {
            stack.extend(children.iter());
        }
        if let Some(template) = obj.get("template") {
            stack.push(template);
        }
    }
    Ok(())
}

fn check_per_kind(
    kind: &str,
    obj: &serde_json::Map<String, Value>,
) -> Result<(), BundleError> {
    match kind {
        "image" if !obj.contains_key("alt") => Err(BundleError::Invalid(
            "`image` primitive MUST declare `alt` (LSML 1.0 §13)".into(),
        )),
        "stack" => {
            let dir = obj.get("direction").and_then(Value::as_str);
            if matches!(dir, Some("horizontal" | "vertical")) {
                Ok(())
            } else {
                Err(BundleError::Invalid(
                    "`stack.direction` MUST be \"horizontal\" or \"vertical\"".into(),
                ))
            }
        }
        "grid" if !obj.contains_key("columns") => {
            Err(BundleError::Invalid("`grid.columns` is required".into()))
        }
        "shape" => {
            let geom = obj.get("geometry").and_then(Value::as_str);
            match geom {
                Some("rect" | "circle") => Ok(()),
                Some("path") if obj.contains_key("pathData") => Ok(()),
                Some("path") => Err(BundleError::Invalid(
                    "`shape` with geometry=\"path\" requires `pathData`".into(),
                )),
                _ => Err(BundleError::Invalid(
                    "`shape.geometry` MUST be \"rect\", \"circle\" or \"path\"".into(),
                )),
            }
        }
        "media" => {
            let hint = obj.get("kind_hint").and_then(Value::as_str);
            if matches!(hint, Some("video" | "audio")) {
                Ok(())
            } else {
                Err(BundleError::Invalid(
                    "`media.kind_hint` MUST be \"video\" or \"audio\"".into(),
                ))
            }
        }
        "repeat" if !obj.contains_key("scope") => {
            Err(BundleError::Invalid("`repeat.scope` is required".into()))
        }
        "repeat" if !obj.contains_key("template") => Err(BundleError::Invalid(
            "`repeat.template` is required".into(),
        )),
        _ => Ok(()),
    }
}

fn hash_to_scene_version(canonical: &[u8]) -> SceneVersion {
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    let digest = hasher.finalize();
    let mut s = String::with_capacity("sha256:".len() + 64);
    s.push_str("sha256:");
    for byte in &digest {
        write_hex_byte(&mut s, *byte);
    }
    SceneVersion::new(s)
}

fn write_hex_byte(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_bundle_str() -> String {
        serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "main-stage",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": { "w": 1920, "h": 1080 },
                "children": []
            }
        }))
        .unwrap()
    }

    #[test]
    fn parses_minimal_bundle() {
        let s = minimal_bundle_str();
        let b = Bundle::parse_str(&s).unwrap();
        assert_eq!(b.scene_id.as_str(), "main-stage");
        assert_eq!(b.lsml, "1.0");
    }

    #[test]
    fn rejects_lsml_2() {
        let s = serde_json::to_string(&json!({
            "lsml": "2.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {}
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(matches!(err, BundleError::Incompatible { .. }));
    }

    #[test]
    fn computes_content_hash_deterministic() {
        let s = minimal_bundle_str();
        let b = Bundle::parse_str(&s).unwrap();
        let h1 = b.compute_content_hash().unwrap();
        let h2 = b.compute_content_hash().unwrap();
        assert_eq!(h1, h2);
        assert!(h1.is_well_formed(), "hash must be well-formed: {h1}");
        assert_ne!(h1.as_str(), placeholder_hash().as_str());
    }

    #[test]
    fn content_hash_changes_when_layout_changes() {
        let mut b = Bundle::parse_str(&minimal_bundle_str()).unwrap();
        let h1 = b.compute_content_hash().unwrap();
        b.layout = json!({ "kind": "frame", "size": { "w": 1280, "h": 720 } });
        let h2 = b.compute_content_hash().unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_independent_of_declared_scene_version() {
        let b1 = Bundle::parse_str(&minimal_bundle_str()).unwrap();
        let mut b2 = b1.clone();
        b2.scene_version = SceneVersion::new("sha256:dead".repeat(16));
        assert_eq!(
            b1.compute_content_hash().unwrap(),
            b2.compute_content_hash().unwrap(),
        );
    }

    #[test]
    fn with_computed_version_self_consistent() {
        let b = Bundle::parse_str(&minimal_bundle_str())
            .unwrap()
            .with_computed_version()
            .unwrap();
        assert!(b.is_self_consistent().unwrap());
    }

    #[test]
    fn rejects_non_input_operator_path() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": { "kind": "frame", "size": {"w": 1, "h": 1}, "children": [] },
            "operator_inputs": [{
                "path": "show.title",
                "label": "Title",
                "type": "string",
                "writable_by": ["operator"]
            }]
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("__inputs"),
            "expected message to mention __inputs: {msg}"
        );
    }

    #[test]
    fn rejects_disallowed_animate_property() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "animate": { "width": 100 },
                "children": []
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("animatable"), "{msg}");
    }

    #[test]
    fn accepts_allowed_animate_properties() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "animate": {
                    "transition": { "duration": 200, "easing": "spring" },
                    "transform": { "translate": [0, 0], "scale": 1, "rotate": 0 },
                    "opacity": 1,
                    "filter": { "blur": 0, "brightness": 1 }
                },
                "children": []
            }
        }))
        .unwrap();
        Bundle::parse_str(&s).expect("must parse");
    }

    #[test]
    fn rejects_image_without_alt() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [
                    { "kind": "image", "bind": { "src": "logo.url" }, "size": {"w": 10, "h": 10} }
                ]
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(format!("{err}").contains("alt"));
    }

    #[test]
    fn rejects_image_inside_repeat_template_without_alt() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [{
                    "kind": "repeat",
                    "bind": { "items": "players" },
                    "scope": "p",
                    "template": {
                        "kind": "image",
                        "bind": { "src": "{p}.avatar" },
                        "size": {"w": 50, "h": 50}
                    }
                }]
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(format!("{err}").contains("alt"));
    }

    #[test]
    fn rejects_unknown_kind() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "carousel",
                "children": []
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unknown primitive kind") && msg.contains("carousel"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_stack_without_direction() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [
                    { "kind": "stack", "children": [] }
                ]
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(format!("{err}").contains("stack.direction"));
    }

    #[test]
    fn rejects_shape_path_without_pathdata() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [
                    { "kind": "shape", "geometry": "path" }
                ]
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(format!("{err}").contains("pathData"));
    }

    #[test]
    fn rejects_media_without_kind_hint() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [
                    { "kind": "media", "bind": { "src": "v.url" } }
                ]
            }
        }))
        .unwrap();
        let err = Bundle::parse_str(&s).unwrap_err();
        assert!(format!("{err}").contains("kind_hint"));
    }

    #[test]
    fn accepts_full_catalogue_layout() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1920, "h": 1080},
                "children": [
                    { "kind": "stack", "direction": "vertical", "children": [
                        { "kind": "text", "bind": { "value": "title" } },
                        { "kind": "shape", "geometry": "rect", "size": {"w": 100, "h": 50} },
                        { "kind": "image", "alt": "Logo", "bind": { "src": "logo" }, "size": {"w": 50, "h": 50} },
                        { "kind": "media", "kind_hint": "video", "bind": { "src": "v" }, "size": {"w": 640, "h": 360} }
                    ]},
                    { "kind": "grid", "columns": 3, "children": [] },
                    {
                        "kind": "repeat",
                        "bind": { "items": "players" },
                        "scope": "p",
                        "template": { "kind": "text", "bind": { "value": "{p}.name" } }
                    }
                ]
            }
        }))
        .unwrap();
        Bundle::parse_str(&s).expect("must accept full catalogue");
    }

    #[test]
    fn accepts_image_with_alt() {
        let s = serde_json::to_string(&json!({
            "lsml": "1.0",
            "scene_id": "x",
            "scene_version": placeholder_hash(),
            "layout": {
                "kind": "frame",
                "size": {"w": 1, "h": 1},
                "children": [
                    { "kind": "image", "alt": "Logo", "bind": { "src": "logo.url" }, "size": {"w": 10, "h": 10} }
                ]
            }
        }))
        .unwrap();
        Bundle::parse_str(&s).unwrap();
    }
}
