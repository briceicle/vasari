use serde::{Deserialize, Serialize};
use serde_json::json;

use super::NodeId;
use crate::hash::node_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: NodeId,
    pub intent_ids: Vec<NodeId>,
    pub steps: Vec<PlanStep>,
    pub parent_ids: Vec<NodeId>,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

fn default_schema_version() -> String {
    "1".to_string()
}

/// One step in a plan. Steps are sub-records of Plan, not standalone nodes.
/// Actions reference steps via (plan_id, step_index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub goal: String,
    pub constraints: Vec<String>,
}

impl Plan {
    pub fn new(intent_ids: Vec<NodeId>, steps: Vec<PlanStep>, parent_ids: Vec<NodeId>) -> Self {
        let id = NodeId(node_id(&Self::hash_input(&intent_ids, &steps, &parent_ids)));
        Self {
            id,
            intent_ids,
            steps,
            parent_ids,
            schema_version: default_schema_version(),
        }
    }

    fn hash_input(
        intent_ids: &[NodeId],
        steps: &[PlanStep],
        parent_ids: &[NodeId],
    ) -> serde_json::Value {
        json!({
            "type": "plan",
            "intent_ids": intent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            "steps": steps.iter().map(|s| json!({
                "goal": s.goal,
                "constraints": s.constraints,
            })).collect::<Vec<_>>(),
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}
