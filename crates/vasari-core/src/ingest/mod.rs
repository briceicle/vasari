use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
    error::{DegradedReason, VasariError},
    extract::constraints::extract_constraints,
    redact::redact,
    schema::{
        Action, Attribution, AttributionTarget, Intent, Node, NodeId, Plan, PlanRef, PlanStep,
    },
    store::ObjectStore,
};

/// Where to read session data from.
pub enum IngestSource {
    File(PathBuf),
    Stdin,
}

/// Events emitted by an adapter during session parsing.
#[derive(Debug)]
pub enum IngestEvent {
    /// Signals the start of a session; provides the source label for the Intent.
    SessionStart {
        source: String,
        started_at: DateTime<Utc>,
    },
    /// A substantive user message — each one opens a new turn (Intent + Plan).
    UserPrompt {
        text: String,
        timestamp: DateTime<Utc>,
    },
    /// Any tool invocation observed in the session.
    ToolCall {
        name: String,
        args: Value,
        result_summary: String,
        timestamp: DateTime<Utc>,
    },
    /// System-level instruction (e.g., CLAUDE.md content, session preamble).
    SystemInstruction { text: String },
}

/// Outcome of a `run_pipeline` call.
#[derive(Debug, Default)]
pub struct IngestSummary {
    pub intents_created: usize,
    pub plans_created: usize,
    pub constraints_created: usize,
    pub actions_created: usize,
    pub attributions_created: usize,
    pub degraded: Vec<DegradedReason>,
}

/// Adapter contract: parse a source into a flat list of IngestEvents.
pub trait IngestAdapter {
    fn parse(&self, source: IngestSource) -> Result<Vec<IngestEvent>, VasariError>;
}

/// A redacted tool call: (tool name, args, result summary, timestamp).
type ToolCallData = (String, Value, String, DateTime<Utc>);

/// One conversational turn: a substantive user prompt plus the tool calls the
/// agent made in response to it (in document order). Each turn becomes one
/// Intent + one Plan, so `vasari why` resolves a line to the specific request
/// that produced it rather than the whole session's first prompt.
struct Turn {
    text: String,
    timestamp: DateTime<Utc>,
    tools: Vec<ToolCallData>,
}

/// Run the ingest pipeline on a list of events.
///
/// Pipeline: redact → synthesize → store → index.
/// Returns a summary of what was created; non-fatal degradations are
/// collected into `summary.degraded` rather than returned as errors.
pub fn run_pipeline(
    events: Vec<IngestEvent>,
    store: &ObjectStore,
) -> Result<IngestSummary, VasariError> {
    let mut summary = IngestSummary::default();

    // Walk events in DOCUMENT order, grouping tool calls into turns. Each
    // substantive user prompt opens a new turn; tool calls attach to the most
    // recent preceding prompt. Document order (not timestamps, which are coarse
    // and can fall back to wall-clock at ingest) is the causal chain.
    let mut session_source = String::from("unknown-session");
    let mut system_instructions: Vec<String> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    // Tool calls seen before the first prompt — attached to turn 1.
    let mut pre_prompt_tools: Vec<ToolCallData> = Vec::new();

    for event in events {
        match event {
            IngestEvent::SessionStart { source, .. } => {
                session_source = source;
            }
            IngestEvent::UserPrompt { text, timestamp } => {
                turns.push(Turn {
                    text: redact(&text),
                    timestamp,
                    tools: Vec::new(),
                });
            }
            IngestEvent::ToolCall {
                name,
                args,
                result_summary,
                timestamp,
            } => {
                let tc = (name, redact_value(args), redact(&result_summary), timestamp);
                match turns.last_mut() {
                    Some(turn) => turn.tools.push(tc),
                    None => pre_prompt_tools.push(tc),
                }
            }
            IngestEvent::SystemInstruction { text } => {
                system_instructions.push(redact(&text));
            }
        }
    }

    // Require at least one user prompt to form an Intent.
    if turns.is_empty() {
        summary.degraded.push(DegradedReason::EmptySession {
            source: session_source.clone(),
        });
        return Ok(summary);
    }

    // Pre-prompt tool calls belong to the first turn.
    if !pre_prompt_tools.is_empty() {
        let mut prepended = std::mem::take(&mut pre_prompt_tools);
        prepended.append(&mut turns[0].tools);
        turns[0].tools = prepended;
    }

    // One Intent + one Plan per turn. Collect plan IDs so session-level
    // (system-instruction) constraints can be attached to every plan.
    let mut plan_ids: Vec<NodeId> = Vec::with_capacity(turns.len());

    for turn in &turns {
        // --- Intent (this turn's prompt) ---
        let intent = Intent::new_at(
            session_source.clone(),
            turn.text.clone(),
            turn.timestamp,
            vec![],
        );
        let intent_id = intent.id.clone();
        store.put(&Node::Intent(intent))?;
        summary.intents_created += 1;

        // --- Plan: one step per tool call in this turn ---
        let steps: Vec<PlanStep> = turn
            .tools
            .iter()
            .map(|(name, args, _, _)| PlanStep {
                goal: step_goal(name, args),
                constraints: vec![],
            })
            .collect();
        let plan = Plan::new(vec![intent_id.clone()], steps, vec![]);
        let plan_id = plan.id.clone();
        store.put(&Node::Plan(plan))?;
        summary.plans_created += 1;
        plan_ids.push(plan_id.clone());

        // --- Actions + Attributions for this turn ---
        for (step_index, (name, args, result_summary, timestamp)) in turn.tools.iter().enumerate() {
            let action = Action::new(
                name.clone(),
                args.clone(),
                // Truncated annotation only: full results stay in-memory for
                // range computation (see truncate_summary).
                truncate_summary(result_summary),
                *timestamp,
                PlanRef {
                    plan_id: plan_id.clone(),
                    step_index,
                },
                vec![intent_id.clone()],
            );
            let action_id = action.id.clone();
            store.put(&Node::Action(action))?;
            summary.actions_created += 1;

            if let Some(attr) = attribution_for_tool(name, args, &action_id, &intent_id) {
                store.put(&Node::Attribution(attr))?;
                summary.attributions_created += 1;
            }
        }

        // --- Per-prompt constraints: derived from THIS turn's intent ---
        for constraint in extract_constraints(&turn.text, intent_id.clone(), vec![]) {
            store.put(&Node::Constraint(constraint))?;
            summary.constraints_created += 1;
        }
    }

    // --- Session-level constraints: system instructions apply to every plan ---
    for sys in &system_instructions {
        for plan_id in &plan_ids {
            for constraint in extract_constraints(sys, plan_id.clone(), vec![]) {
                store.put(&Node::Constraint(constraint))?;
                summary.constraints_created += 1;
            }
        }
    }

    Ok(summary)
}

/// Recursively redact all string leaves in a JSON value — both object keys and
/// values. Best-effort (the underlying `redact` is heuristic); applied to
/// tool-call args before they are stored in Action nodes, and reused by the
/// corpus scrubber. Keys are redacted too so a secret embedded as a key
/// (e.g. `{"<secret>": "used"}`) cannot slip through.
pub fn redact_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact(&s)),
        Value::Array(arr) => Value::Array(arr.into_iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (redact(&k), redact_value(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Max bytes of a tool result kept in the stored `Action.result_summary`.
/// Results (notably Read = full file contents) are only needed in-memory for
/// range computation; the stored copy is a human-readable annotation.
const MAX_RESULT_SUMMARY: usize = 2000;

/// Truncate a result summary to [`MAX_RESULT_SUMMARY`] bytes on a char boundary,
/// appending an ellipsis marker when truncated.
fn truncate_summary(s: &str) -> String {
    if s.len() <= MAX_RESULT_SUMMARY {
        return s.to_string();
    }
    let mut end = MAX_RESULT_SUMMARY;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &s[..end])
}

/// Human-meaningful goal for a plan step: the tool plus its primary target,
/// so `vasari diff` compares *what was done*, not just which tool ran.
///
/// File tools → `"Edit src/auth.rs"`. Other tools fall back to a short target
/// hint (command / pattern / query, already redacted) or the bare tool name.
fn step_goal(tool: &str, args: &Value) -> String {
    if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
        return format!("{tool} {path}");
    }
    let hint = args
        .get("command")
        .or_else(|| args.get("pattern"))
        .or_else(|| args.get("query"))
        .and_then(|v| v.as_str());
    match hint {
        Some(h) => {
            let truncated: String = h.chars().take(60).collect();
            if h.chars().count() > 60 {
                format!("{tool} {truncated}…")
            } else {
                format!("{tool} {truncated}")
            }
        }
        None => tool.to_string(),
    }
}

/// Produce an Attribution node for Edit/Write tool calls.
/// Returns None for tools that don't produce file-level attributions.
fn attribution_for_tool(
    tool: &str,
    args: &Value,
    action_id: &NodeId,
    intent_id: &NodeId,
) -> Option<Attribution> {
    let path = match tool {
        "Edit" | "Write" | "MultiEdit" => args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }?;

    // Reject paths that escape the repo (adapter should pre-validate, this is defence-in-depth).
    if path.contains("..") {
        return None;
    }

    // Confidence assigned to whole-file attributions — calibrated empirically.
    const WHOLE_FILE_CONFIDENCE: f32 = 0.7;

    Some(Attribution::new(
        action_id.clone(),
        AttributionTarget::LineRange {
            path,
            start: 1,
            // Whole-file sentinel: u32::MAX means "the entire file".
            // A 'vasari why' query at any line number will match this Attribution.
            end: u32::MAX,
        },
        WHOLE_FILE_CONFIDENCE,
        vec![],
        vec![intent_id.clone()],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn make_store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn empty_session_is_degraded_not_error() {
        let (store, _dir) = make_store();
        let summary = run_pipeline(vec![], &store).unwrap();
        assert_eq!(summary.intents_created, 0);
        assert_eq!(summary.degraded.len(), 1);
        assert!(matches!(
            summary.degraded[0],
            DegradedReason::EmptySession { .. }
        ));
    }

    #[test]
    fn minimal_session_creates_intent_and_plan() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::SessionStart {
                source: "test-session".into(),
                started_at: Utc::now(),
            },
            IngestEvent::UserPrompt {
                text: "Add JWT verification to the auth module".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/auth.rs", "old_string": "", "new_string": "" }),
                result_summary: "Edited src/auth.rs".into(),
                timestamp: Utc::now(),
            },
        ];

        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.intents_created, 1);
        assert_eq!(summary.plans_created, 1);
        assert_eq!(summary.actions_created, 1);
        assert_eq!(summary.attributions_created, 1);
        assert!(summary.degraded.is_empty());
    }

    #[test]
    fn multi_turn_session_creates_one_intent_per_turn() {
        let (store, _dir) = make_store();
        let t0 = Utc::now();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "Add JWT verification to auth".into(),
                timestamp: t0,
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/auth.rs", "old_string": "a", "new_string": "b" }),
                result_summary: String::new(),
                timestamp: t0,
            },
            IngestEvent::UserPrompt {
                text: "Now refactor the date helpers".into(),
                timestamp: t0,
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/dates.rs", "old_string": "c", "new_string": "d" }),
                result_summary: String::new(),
                timestamp: t0,
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.intents_created, 2, "one intent per turn");
        assert_eq!(summary.plans_created, 2, "one plan per turn");

        // why on lines from different turns returns the turn-specific intent.
        let a = crate::resolve::why(&store, "src/auth.rs", 1)
            .unwrap()
            .unwrap();
        let d = crate::resolve::why(&store, "src/dates.rs", 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            a.primary_intent().unwrap().text,
            "Add JWT verification to auth"
        );
        assert_eq!(
            d.primary_intent().unwrap().text,
            "Now refactor the date helpers"
        );
    }

    #[test]
    fn tool_calls_before_first_prompt_attach_to_turn_one() {
        let (store, _dir) = make_store();
        let t0 = Utc::now();
        let events = vec![
            // A tool call with no preceding prompt (e.g. a resumed session).
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/early.rs", "old_string": "a", "new_string": "b" }),
                result_summary: String::new(),
                timestamp: t0,
            },
            IngestEvent::UserPrompt {
                text: "the actual first prompt".into(),
                timestamp: t0,
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.intents_created, 1);
        // The pre-prompt edit is attributed to turn 1's intent, not dropped.
        let chain = crate::resolve::why(&store, "src/early.rs", 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            chain.primary_intent().unwrap().text,
            "the actual first prompt"
        );
    }

    #[test]
    fn step_goal_carries_file_path_and_falls_back() {
        assert_eq!(
            step_goal("Edit", &json!({ "file_path": "src/auth.rs" })),
            "Edit src/auth.rs"
        );
        assert_eq!(
            step_goal("Bash", &json!({ "command": "cargo test" })),
            "Bash cargo test"
        );
        assert_eq!(
            step_goal("Grep", &json!({ "pattern": "TODO" })),
            "Grep TODO"
        );
        // No target hint → bare tool name.
        assert_eq!(step_goal("Read", &json!({})), "Read");
        // Long hints are truncated with an ellipsis.
        let long = "x".repeat(80);
        let goal = step_goal("Bash", &json!({ "command": long }));
        assert!(goal.starts_with("Bash "));
        assert!(goal.ends_with('…'));
    }

    #[test]
    fn plan_step_goal_reflects_edited_file() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "do the thing".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/auth.rs", "old_string": "a", "new_string": "b" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];
        run_pipeline(events, &store).unwrap();
        let plan = store
            .iter_all()
            .unwrap()
            .into_iter()
            .find_map(|n| if let Node::Plan(p) = n { Some(p) } else { None })
            .expect("a plan was created");
        assert_eq!(plan.steps[0].goal, "Edit src/auth.rs");
    }

    #[test]
    fn constraint_keywords_in_prompt_create_constraints() {
        let (store, _dir) = make_store();
        let events = vec![IngestEvent::UserPrompt {
            text:
                "You must validate all inputs before writing. Never store passwords in plaintext."
                    .into(),
            timestamp: Utc::now(),
        }];

        let summary = run_pipeline(events, &store).unwrap();
        assert!(summary.constraints_created >= 2);
    }

    #[test]
    fn dotdot_path_produces_no_attribution() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "test".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Write".into(),
                args: json!({ "file_path": "../escape/path.rs" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];

        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.attributions_created, 0);
    }

    #[test]
    fn multi_edit_produces_attribution() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "refactor the module".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "MultiEdit".into(),
                args: json!({ "file_path": "src/lib.rs" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.attributions_created, 1);
    }

    #[test]
    fn bash_tool_produces_no_attribution() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "run tests".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Bash".into(),
                args: json!({ "command": "cargo test" }),
                result_summary: "all tests passed".into(),
                timestamp: Utc::now(),
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(
            summary.attributions_created, 0,
            "Bash tool should not produce attribution"
        );
    }

    #[test]
    fn multiple_tool_calls_produce_multiple_actions() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "add jwt and update tests".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/auth.rs" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Write".into(),
                args: json!({ "file_path": "src/jwt.rs" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Bash".into(),
                args: json!({ "command": "cargo test" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert_eq!(summary.actions_created, 3);
        assert_eq!(
            summary.attributions_created, 2,
            "only Edit + Write produce attributions"
        );
    }

    #[test]
    fn tool_call_args_are_redacted_before_storage() {
        let (store, _dir) = make_store();
        let secret = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz1234567890abcdef";
        let events = vec![
            IngestEvent::UserPrompt {
                text: "write the config".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Write".into(),
                args: json!({ "file_path": "config.toml", "new_content": secret }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];
        run_pipeline(events, &store).unwrap();

        // Retrieve all Action nodes and verify the secret is not present in args.
        let nodes = store.iter_all().unwrap();
        for node in nodes {
            if let crate::Node::Action(action) = node {
                let serialized = serde_json::to_string(&action.args).unwrap();
                assert!(
                    !serialized.contains("sk-ant-api03-"),
                    "Anthropic API key must not survive in stored Action args"
                );
            }
        }
    }

    #[test]
    fn system_instruction_constraints_are_extracted() {
        let (store, _dir) = make_store();
        let events = vec![
            IngestEvent::UserPrompt {
                text: "make the thing work".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::SystemInstruction {
                text: "You must always validate user input. Never access external APIs without approval.".into(),
            },
        ];
        let summary = run_pipeline(events, &store).unwrap();
        assert!(
            summary.constraints_created >= 2,
            "system instruction constraints should be extracted"
        );
    }
}
