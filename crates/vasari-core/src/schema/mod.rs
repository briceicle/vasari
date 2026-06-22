mod action;
mod attribution;
mod constraint;
mod intent;
mod plan;

pub use action::{Action, PlanRef};
pub use attribution::{Attribution, AttributionTarget, Evidence, EvidenceKind};
pub use constraint::Constraint;
pub use intent::Intent;
pub use plan::{Plan, PlanStep};

use serde::{Deserialize, Serialize};

/// Content-addressed node identifier: sha256 hex of the node's canonical hash input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// All five Vasari node types as a tagged union for storage.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Intent(Intent),
    Plan(Plan),
    Constraint(Constraint),
    Action(Action),
    Attribution(Attribution),
}

impl Node {
    pub fn id(&self) -> &NodeId {
        match self {
            Node::Intent(n) => &n.id,
            Node::Plan(n) => &n.id,
            Node::Constraint(n) => &n.id,
            Node::Action(n) => &n.id,
            Node::Attribution(n) => &n.id,
        }
    }
}
