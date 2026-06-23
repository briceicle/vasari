use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::NodeId;
use crate::hash::node_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: NodeId,
    pub tool: String,
    /// Hashed if the args contain sensitive values (e.g., credentials, PII).
    pub args: serde_json::Value,
    /// Human-readable summary of the result — EXCLUDED from content hash.
    pub result_summary: String,
    pub timestamp: DateTime<Utc>,
    pub plan_ref: PlanRef,
    pub parent_ids: Vec<NodeId>,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

fn default_schema_version() -> String {
    "1".to_string()
}

/// Reference to a specific step within a Plan.
/// Steps are sub-records of Plan nodes, not standalone nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRef {
    pub plan_id: NodeId,
    pub step_index: usize,
}

impl Action {
    pub fn new(
        tool: String,
        args: serde_json::Value,
        result_summary: String,
        timestamp: DateTime<Utc>,
        plan_ref: PlanRef,
        parent_ids: Vec<NodeId>,
    ) -> Self {
        let id = NodeId(node_id(&Self::hash_input(
            &tool,
            &args,
            &timestamp,
            &plan_ref,
            &parent_ids,
        )));
        Self { id, tool, args, result_summary, timestamp, plan_ref, parent_ids, schema_version: default_schema_version() }
    }

    /// result_summary is intentionally excluded — it's an annotation, not identity.
    fn hash_input(
        tool: &str,
        args: &serde_json::Value,
        timestamp: &DateTime<Utc>,
        plan_ref: &PlanRef,
        parent_ids: &[NodeId],
    ) -> serde_json::Value {
        json!({
            "type": "action",
            "tool": tool,
            "args": args,
            "timestamp": timestamp.to_rfc3339(),
            "plan_ref": {
                "plan_id": plan_ref.plan_id.as_str(),
                "step_index": plan_ref.step_index,
            },
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}
