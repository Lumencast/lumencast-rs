//! Placeholder substitution.
//!
//! Two families of tokens are recognised:
//!
//! - `$TOKEN_OPERATOR`, `$TOKEN_VIEWER`, `$TOKEN_SERVICE`,
//!   `$TOKEN_TEST`, `$TOKEN_INVALID` — opaque token values supplied by
//!   the harness via `interop/fixtures/canonical-tokens.json`.
//! - `$BUNDLE.<id>.hash` — content hash of the inline bundle named
//!   `<id>`, computed via the LSML 1.0 canonical hasher.

use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Resolved placeholder map: literal `$TOKEN_*` and `$BUNDLE.<id>.hash`
/// strings → replacement strings.
#[derive(Debug, Clone, Default)]
pub struct Substitutions(pub BTreeMap<String, String>);

impl Substitutions {
    /// Build from `(tokens, bundle_hashes)`.
    #[must_use]
    pub fn new(
        tokens: &BTreeMap<String, String>,
        bundle_hashes: &BTreeMap<String, String>,
    ) -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in tokens {
            map.insert(k.clone(), v.clone());
        }
        for (id, hash) in bundle_hashes {
            map.insert(format!("$BUNDLE.{id}.hash"), hash.clone());
        }
        Self(map)
    }

    /// Substitute every `$…` placeholder appearing as a string value
    /// inside `value`. Object keys and array indices are left alone.
    #[must_use]
    pub fn apply(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.replace_str(s)),
            Value::Array(arr) => Value::Array(arr.iter().map(|v| self.apply(v)).collect()),
            Value::Object(obj) => {
                let mut out = serde_json::Map::with_capacity(obj.len());
                for (k, v) in obj {
                    out.insert(k.clone(), self.apply(v));
                }
                Value::Object(out)
            }
            // Numbers, booleans, null pass through.
            other => other.clone(),
        }
    }

    fn replace_str(&self, s: &str) -> String {
        if !s.contains('$') {
            return s.to_string();
        }
        // Greedy whole-string replacement first (most common case in
        // scenarios: a value that is exactly `"$TOKEN_OPERATOR"`).
        if let Some(replacement) = self.0.get(s) {
            return replacement.clone();
        }
        // Otherwise scan and replace each occurrence longest-key-first
        // to avoid `$TOKEN` matching before `$TOKEN_OPERATOR`.
        let mut keys: Vec<&String> = self.0.keys().collect();
        keys.sort_by_key(|b| std::cmp::Reverse(b.len()));
        let mut out = s.to_string();
        for key in keys {
            if out.contains(key.as_str()) {
                out = out.replace(key.as_str(), &self.0[key]);
            }
        }
        out
    }
}

/// Compute the canonical content hash of an inline bundle JSON value.
///
/// Uses the same canonicalisation as
/// [`lumencast_protocol::Bundle::compute_content_hash`]: serialise via
/// `serde_json` (which uses `BTreeMap` ordering by default), no
/// whitespace, then SHA-256 the bytes. Inline scenario bundles are
/// not full LSML documents, so we don't drop a `scene_version` field
/// — the spec leaves this to the scenario author.
pub fn hash_inline_bundle(value: &Value) -> String {
    let canonical = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    let digest = hasher.finalize();
    let mut s = String::with_capacity("sha256:".len() + 64);
    s.push_str("sha256:");
    for byte in &digest {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn substitutions() -> Substitutions {
        let tokens = [
            ("$TOKEN_OPERATOR".to_string(), "tok-op-1".to_string()),
            ("$TOKEN_VIEWER".to_string(), "tok-vw-1".to_string()),
        ]
        .into_iter()
        .collect();
        let bundles = [("t".to_string(), "sha256:abc".to_string())]
            .into_iter()
            .collect();
        Substitutions::new(&tokens, &bundles)
    }

    #[test]
    fn substitutes_token_value() {
        let s = substitutions();
        let out = s.apply(&json!({
            "type": "subscribe",
            "token": "$TOKEN_OPERATOR",
            "scene": "main"
        }));
        assert_eq!(out["token"], json!("tok-op-1"));
    }

    #[test]
    fn substitutes_bundle_hash() {
        let s = substitutions();
        let out = s.apply(&json!({"scene_version": "$BUNDLE.t.hash"}));
        assert_eq!(out["scene_version"], json!("sha256:abc"));
    }

    #[test]
    fn passes_through_non_strings() {
        let s = substitutions();
        let out = s.apply(&json!({"seq": 1, "ok": true, "x": null}));
        assert_eq!(out["seq"], json!(1));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["x"], Value::Null);
    }

    #[test]
    fn hash_is_deterministic() {
        let v = json!({"id": "t", "state": {"a": 1, "b": 2}});
        assert_eq!(hash_inline_bundle(&v), hash_inline_bundle(&v));
    }

    #[test]
    fn hash_changes_with_content() {
        let a = hash_inline_bundle(&json!({"id": "t", "x": 1}));
        let b = hash_inline_bundle(&json!({"id": "t", "x": 2}));
        assert_ne!(a, b);
    }
}
