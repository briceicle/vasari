use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::NodeId;
use crate::hash::node_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: NodeId,
    /// Ticket URL, prompt text, or file reference — what triggered this intent.
    pub source: String,
    /// Human-readable description of the intent.
    pub text: String,
    pub created_at: DateTime<Utc>,
    /// Empty for root intents; one prior Intent ID for amendments.
    pub parent_ids: Vec<NodeId>,
}

impl Intent {
    pub fn new(source: String, text: String, parent_ids: Vec<NodeId>) -> Self {
        let created_at = Utc::now();
        let id = NodeId(node_id(&Self::hash_input(
            &source,
            &text,
            &created_at,
            &parent_ids,
        )));
        Self { id, source, text, created_at, parent_ids }
    }

    fn hash_input(
        source: &str,
        text: &str,
        created_at: &DateTime<Utc>,
        parent_ids: &[NodeId],
    ) -> serde_json::Value {
        json!({
            "type": "intent",
            "source": source,
            "text": text,
            "created_at": created_at.to_rfc3339(),
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_deterministic_for_same_timestamp() {
        let ts = Utc::now();
        let parents: Vec<NodeId> = vec![];
        let a = Intent {
            id: NodeId(node_id(&Intent::hash_input(
                "ACME-411",
                "Add JWT verification",
                &ts,
                &parents,
            ))),
            source: "ACME-411".into(),
            text: "Add JWT verification".into(),
            created_at: ts,
            parent_ids: parents.clone(),
        };
        let b = Intent {
            id: NodeId(node_id(&Intent::hash_input(
                "ACME-411",
                "Add JWT verification",
                &ts,
                &parents,
            ))),
            source: "ACME-411".into(),
            text: "Add JWT verification".into(),
            created_at: ts,
            parent_ids: parents,
        };
        assert_eq!(a.id, b.id);
    }
}
