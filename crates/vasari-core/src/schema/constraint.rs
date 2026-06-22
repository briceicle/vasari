use serde::{Deserialize, Serialize};
use serde_json::json;

use super::NodeId;
use crate::hash::node_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub id: NodeId,
    pub text: String,
    /// The Intent or Plan this constraint was derived from.
    pub derived_from: NodeId,
    pub parent_ids: Vec<NodeId>,
}

impl Constraint {
    pub fn new(text: String, derived_from: NodeId, parent_ids: Vec<NodeId>) -> Self {
        let id = NodeId(node_id(&Self::hash_input(&text, &derived_from, &parent_ids)));
        Self { id, text, derived_from, parent_ids }
    }

    fn hash_input(text: &str, derived_from: &NodeId, parent_ids: &[NodeId]) -> serde_json::Value {
        json!({
            "type": "constraint",
            "text": text,
            "derived_from": derived_from.as_str(),
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}
