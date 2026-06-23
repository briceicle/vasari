use serde::{Deserialize, Serialize};
use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::NodeId;

    fn dummy_id() -> NodeId {
        NodeId("cafebabe".repeat(8))
    }

    #[test]
    fn polarity_is_included_in_hash() {
        let text = "You must validate all inputs before writing.";
        let derived = dummy_id();
        let mandatory = Constraint::new(text.into(), derived.clone(), ConstraintPolarity::Mandatory, vec![]);
        let prohibitive = Constraint::new(text.into(), derived.clone(), ConstraintPolarity::Prohibitive, vec![]);
        assert_ne!(
            mandatory.id, prohibitive.id,
            "different polarities must produce different IDs"
        );
    }

    #[test]
    fn serde_round_trip() {
        let c = Constraint::new(
            "Never store tokens in plaintext.".into(),
            dummy_id(),
            ConstraintPolarity::Prohibitive,
            vec![],
        );
        let json = serde_json::to_string(&c).unwrap();
        let decoded: Constraint = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, c.id);
        assert_eq!(decoded.polarity, ConstraintPolarity::Prohibitive);
        assert_eq!(decoded.text, c.text);
    }

    #[test]
    fn schema_version_defaults_to_one_on_deserialize() {
        let json = r#"{"id":"abcd1234","text":"must do","derived_from":"ef567890","polarity":"mandatory","parent_ids":[]}"#;
        let c: Constraint = serde_json::from_str(json).unwrap();
        assert_eq!(c.schema_version, "1");
    }

    #[test]
    fn schema_version_excluded_from_hash() {
        // Two constraints identical except schema_version should have the same ID
        // since schema_version is excluded from hash_input.
        let text = "You must always sanitize inputs.";
        let derived = dummy_id();
        let c1 = Constraint::new(text.into(), derived.clone(), ConstraintPolarity::Mandatory, vec![]);
        // Manually mutate schema_version — ID should remain unchanged.
        let mut c2 = c1.clone();
        c2.schema_version = "99".into();
        assert_eq!(c1.id, c2.id, "schema_version must not affect the content hash");
    }
}

use super::NodeId;
use crate::hash::node_id;

/// Whether a constraint must be satisfied (mandatory) or must not be violated (prohibitive).
/// v0.1 `vasari violations` only checks mandatory constraints; prohibitive constraints are
/// recorded but not evaluated (polarity detection requires semantics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintPolarity {
    #[default]
    Mandatory,
    Prohibitive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub id: NodeId,
    pub text: String,
    /// The Intent or Plan this constraint was derived from.
    pub derived_from: NodeId,
    pub polarity: ConstraintPolarity,
    pub parent_ids: Vec<NodeId>,
    /// Schema version for this node type. Stored but excluded from content hash
    /// (annotation, not identity — same pattern as confidence/result_summary).
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

fn default_schema_version() -> String {
    "1".to_string()
}

impl Constraint {
    pub fn new(
        text: String,
        derived_from: NodeId,
        polarity: ConstraintPolarity,
        parent_ids: Vec<NodeId>,
    ) -> Self {
        let id = NodeId(node_id(&Self::hash_input(&text, &derived_from, &polarity, &parent_ids)));
        Self {
            id,
            text,
            derived_from,
            polarity,
            parent_ids,
            schema_version: default_schema_version(),
        }
    }

    fn hash_input(
        text: &str,
        derived_from: &NodeId,
        polarity: &ConstraintPolarity,
        parent_ids: &[NodeId],
    ) -> serde_json::Value {
        let polarity_str = match polarity {
            ConstraintPolarity::Mandatory => "mandatory",
            ConstraintPolarity::Prohibitive => "prohibitive",
        };
        json!({
            "type": "constraint",
            "text": text,
            "derived_from": derived_from.as_str(),
            "polarity": polarity_str,
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}
