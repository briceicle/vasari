use sha2::{Digest, Sha256};

/// Compute the Vasari node ID: sha256 of the RFC 8785 JCS canonical form
/// of the hash-input value.
///
/// Full RFC 8785 compliance requires specific number serialization and unicode
/// escape handling beyond key-sorting. This implementation covers the critical
/// invariant (key-sorted JSON, no whitespace) which is sufficient for hash
/// stability within a single implementation. Full JCS compliance is tracked
/// in INTENT-SPEC.md §3 (Canonicalization).
pub fn node_id(hash_input: &serde_json::Value) -> String {
    let canonical = canonical_json_bytes(hash_input);
    let digest = Sha256::digest(&canonical);
    hex::encode(digest)
}

/// Produce deterministic JSON bytes with recursively sorted object keys.
///
/// This is the key-ordering invariant of RFC 8785 JCS. Number formatting
/// and unicode escaping to full spec is deferred (see hash.rs comment above).
pub fn canonical_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    fn sort_keys(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let sorted = keys
                    .into_iter()
                    .map(|k| (k.clone(), sort_keys(&map[k])))
                    .collect();
                serde_json::Value::Object(sorted)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(sort_keys).collect())
            }
            other => other.clone(),
        }
    }
    let sorted = sort_keys(value);
    // serde_json compact serialization: no extra whitespace, stable key order.
    serde_json::to_vec(&sorted).expect("valid JSON value cannot fail to serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_is_stable() {
        let a = json!({ "z": 1, "a": 2, "m": 3 });
        let b = json!({ "m": 3, "z": 1, "a": 2 });
        assert_eq!(canonical_json_bytes(&a), canonical_json_bytes(&b));
    }

    #[test]
    fn nested_objects_sorted() {
        let v = json!({ "b": { "z": 1, "a": 2 }, "a": 1 });
        let bytes = canonical_json_bytes(&v);
        let s = std::str::from_utf8(&bytes).unwrap();
        // "a" key should appear before "b" at the top level
        assert!(s.find('"').is_some());
        let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
        let obj = parsed.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(keys[0], "a");
        assert_eq!(keys[1], "b");
    }

    #[test]
    fn node_id_is_hex_sha256() {
        let id = node_id(&json!({ "type": "intent", "text": "hello" }));
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn same_content_same_id() {
        let v = json!({ "type": "intent", "text": "add JWT verification" });
        assert_eq!(node_id(&v), node_id(&v));
    }
}
