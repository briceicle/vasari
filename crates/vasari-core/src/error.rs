use thiserror::Error;

#[derive(Debug, Error)]
pub enum VasariError {
    #[error("node not found: {0}")]
    NodeNotFound(String),
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("invalid node id: {0}")]
    InvalidNodeId(String),
    #[error("ambiguous id prefix '{prefix}' matches {count} nodes — use more characters")]
    AmbiguousPrefix { prefix: String, count: usize },
    #[error("no attribution found for {path}:{line} — run `vasari ingest` to populate the graph, or `vasari fsck` to rebuild the index")]
    AttributionNotFound { path: String, line: u32 },
    #[error("plan not found: {0}")]
    PlanNotFound(String),
    #[error("plan step index {step_index} out of bounds for plan {plan_id} ({step_count} steps) — graph may be corrupt; run `vasari fsck`")]
    PlanStepOutOfBounds {
        plan_id: String,
        step_index: usize,
        step_count: usize,
    },
    #[error("degraded ingest: {0}")]
    Degraded(DegradedReason),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Typed degradation reasons for the shared ingest pipeline.
///
/// The adapter layer returns these rather than panicking when sessions
/// have missing fields. The caller decides whether to skip or surface.
#[derive(Debug, Clone)]
pub enum DegradedReason {
    MissingTimestamp { tool: String },
    MissingArgs { tool: String },
    UnknownTool(String),
    UnparsableSession { source: String, detail: String },
    UnparsableSpan { span_name: String, detail: String },
    EmptySession { source: String },
}

impl std::fmt::Display for DegradedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTimestamp { tool } => write!(f, "missing timestamp on tool call '{tool}'"),
            Self::MissingArgs { tool } => write!(f, "missing args on tool call '{tool}'"),
            Self::UnknownTool(t) => write!(f, "unknown tool '{t}'"),
            Self::UnparsableSession { source, detail } => {
                write!(f, "unparsable session from {source}: {detail}")
            }
            Self::UnparsableSpan { span_name, detail } => {
                write!(f, "unparsable span '{span_name}': {detail}")
            }
            Self::EmptySession { source } => {
                write!(f, "no ingest-relevant events found in session: {source}")
            }
        }
    }
}
