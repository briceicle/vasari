use serde::{Deserialize, Serialize};
use serde_json::json;

use super::NodeId;
use crate::hash::node_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribution {
    pub id: NodeId,
    pub action_id: NodeId,
    pub target: AttributionTarget,
    /// Confidence that this action produced this target. 0.0–1.0.
    /// EXCLUDED from content hash — computed annotation, not identity.
    pub confidence: f32,
    pub evidence: Vec<Evidence>,
    pub parent_ids: Vec<NodeId>,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

fn default_schema_version() -> String {
    "1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttributionTarget {
    /// A specific line range within a file.
    LineRange {
        path: String,
        start: u32,
        end: u32,
    },
    /// The whole file — used when a precise range could not be determined
    /// (e.g. an Edit whose location couldn't be located in the file content).
    /// Replaces the former `LineRange { start: 1, end: u32::MAX }` sentinel.
    WholeFile {
        path: String,
    },
    CommitSha {
        sha: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// The action touched this exact range.
    ExactRange,
    /// Heuristic match (e.g., diff fuzzed across whitespace).
    Fuzzed,
    /// A later edit may have overwritten this range.
    Overwritten,
    /// Synthesized from span/session data; not directly observed.
    Inferred,
}

impl Attribution {
    pub fn new(
        action_id: NodeId,
        target: AttributionTarget,
        confidence: f32,
        evidence: Vec<Evidence>,
        parent_ids: Vec<NodeId>,
    ) -> Self {
        let id = NodeId(node_id(&Self::hash_input(&action_id, &target, &parent_ids)));
        Self {
            id,
            action_id,
            target,
            confidence,
            evidence,
            parent_ids,
            schema_version: default_schema_version(),
        }
    }

    /// confidence and evidence are intentionally excluded — confidence is a computed
    /// annotation and evidence may be refined without changing what was attributed.
    fn hash_input(
        action_id: &NodeId,
        target: &AttributionTarget,
        parent_ids: &[NodeId],
    ) -> serde_json::Value {
        let target_json = match target {
            AttributionTarget::LineRange { path, start, end } => {
                json!({ "kind": "line_range", "path": path, "start": start, "end": end })
            }
            AttributionTarget::WholeFile { path } => {
                json!({ "kind": "whole_file", "path": path })
            }
            AttributionTarget::CommitSha { sha } => {
                json!({ "kind": "commit_sha", "sha": sha })
            }
        };
        json!({
            "type": "attribution",
            "action_id": action_id.as_str(),
            "target": target_json,
            "parent_ids": parent_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_file_and_line_range_hash_differently() {
        let action_id = NodeId("abc123".into());
        let whole = Attribution::new(
            action_id.clone(),
            AttributionTarget::WholeFile {
                path: "src/auth.rs".into(),
            },
            0.7,
            vec![],
            vec![],
        );
        let range = Attribution::new(
            action_id,
            AttributionTarget::LineRange {
                path: "src/auth.rs".into(),
                start: 1,
                end: 10,
            },
            0.7,
            vec![],
            vec![],
        );
        assert_ne!(
            whole.id, range.id,
            "whole-file and line-range targets are distinct identities"
        );
    }

    #[test]
    fn whole_file_round_trips_through_serde() {
        let attr = Attribution::new(
            NodeId("abc123".into()),
            AttributionTarget::WholeFile {
                path: "src/x.rs".into(),
            },
            0.7,
            vec![],
            vec![],
        );
        let json = serde_json::to_string(&attr).unwrap();
        assert!(json.contains("\"kind\":\"whole_file\""));
        let decoded: Attribution = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, attr.id);
    }

    #[test]
    fn confidence_change_does_not_change_id() {
        let action_id = NodeId("abc123".into());
        let target = AttributionTarget::LineRange {
            path: "src/auth.ts".into(),
            start: 47,
            end: 47,
        };
        let a = Attribution::new(action_id.clone(), target.clone(), 1.0, vec![], vec![]);
        let b = Attribution::new(action_id, target, 0.7, vec![], vec![]);
        assert_eq!(a.id, b.id);
    }
}
