//! Flatten a JSON value into leaf-grain `(path, value)` pairs that can
//! be passed to [`crate::Scene::emit`].
//!
//! Objects are flattened into dot-separated keys; everything else
//! (strings, numbers, booleans, null, arrays) is emitted as a leaf
//! value at its current path. This matches LSDP/1's wire constraint:
//! patch values MUST NOT be JSON objects.

use serde_json::Value;

/// Flatten `value` rooted at `prefix` into `(path, value)` pairs.
///
/// Empty objects and missing prefixes yield no output. Arrays are
/// passed through as-is — they're legal LSDP/1 leaf values.
#[must_use]
pub fn flatten_into_pairs(prefix: &str, value: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk(prefix, value, &mut out);
    out
}

fn walk(path: &str, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let next = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(&next, v, out);
            }
        }
        _ => {
            if !path.is_empty() {
                out.push((path.to_string(), value.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattens_nested_object() {
        let v = json!({
            "match": {
                "score": { "home": 3, "away": 1 },
                "minute": 42
            }
        });
        let mut pairs = flatten_into_pairs("game", &v);
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            pairs,
            vec![
                ("game.match.minute".to_string(), json!(42)),
                ("game.match.score.away".to_string(), json!(1)),
                ("game.match.score.home".to_string(), json!(3)),
            ]
        );
    }

    #[test]
    fn arrays_are_left_intact() {
        let v = json!({ "players": ["alice", "bob"] });
        let pairs = flatten_into_pairs("team", &v);
        assert_eq!(
            pairs,
            vec![("team.players".to_string(), json!(["alice", "bob"]))]
        );
    }

    #[test]
    fn scalar_at_prefix() {
        let pairs = flatten_into_pairs("status", &json!("live"));
        assert_eq!(pairs, vec![("status".to_string(), json!("live"))]);
    }

    #[test]
    fn empty_prefix_drops_top_level_scalar() {
        // No path means nothing to emit.
        let pairs = flatten_into_pairs("", &json!(42));
        assert!(pairs.is_empty());
    }
}
