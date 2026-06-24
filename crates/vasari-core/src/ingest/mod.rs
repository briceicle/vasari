use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
    error::{DegradedReason, VasariError},
    extract::constraints::extract_constraints,
    redact::redact,
    schema::{
        Action, Attribution, AttributionTarget, Evidence, EvidenceKind, Intent, Node, NodeId, Plan,
        PlanRef, PlanStep,
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

/// Whether a tool-call file path is safe to ingest. Single source of truth for
/// path safety, shared by every adapter and the attribution synthesizer so the
/// rule can't drift between entry points: reject parent-dir traversal (`..`) and
/// absolute paths (which point outside the target repo).
pub(crate) fn is_safe_path(path: &str) -> bool {
    !path.contains("..") && !path.starts_with('/')
}

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

    // Running, document-order view of each file's content, so Edits can be
    // located against the file as the agent saw it (from Read results).
    let mut file_contents: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

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
            // A Read result is the file's content as the agent saw it — record it
            // so a later Edit can be located against it.
            if name == "Read" {
                if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
                    if !result_summary.is_empty() {
                        file_contents.insert(path.to_string(), result_summary.clone());
                    }
                }
            }

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

            for attr in
                attributions_for_action(name, args, &action_id, &intent_id, &mut file_contents)
            {
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

/// Confidence for an attribution whose exact line range was located.
const EXACT_CONFIDENCE: f32 = 1.0;
/// Confidence for a whole-file degrade (range could not be located).
const WHOLE_FILE_CONFIDENCE: f32 = 0.7;

/// Produce attribution node(s) for a file-writing tool call, computing line
/// ranges structurally where possible.
///
/// - **Write** `{file_path, content}` → `LineRange 1..N` (N = line count); the
///   file's content becomes `content`.
/// - **Edit** `{file_path, old_string, new_string}` → locate `old_string` in the
///   file's known content (captured from a prior Read result) to get a
///   `LineRange`; on success the content is updated so later edits resolve
///   against the post-edit file. If the content is unknown or `old_string` is
///   not found, degrade to `WholeFile`.
/// - **MultiEdit** `{file_path, edits:[{old_string,new_string}]}` → one
///   attribution per edit, applied in sequence.
///
/// `file_contents` is the running, document-order view of each file's text.
/// Returns an empty vec for non-file tools.
fn attributions_for_action(
    tool: &str,
    args: &Value,
    action_id: &NodeId,
    intent_id: &NodeId,
    file_contents: &mut std::collections::HashMap<String, String>,
) -> Vec<Attribution> {
    let path = match tool {
        "Edit" | "Write" | "MultiEdit" => args.get("file_path").and_then(|v| v.as_str()),
        _ => return vec![],
    };
    let Some(path) = path else { return vec![] };
    // Defence-in-depth: the adapter pre-validates, but never trust a path here.
    if !is_safe_path(path) {
        return vec![];
    }

    let make = |target, confidence, kind| {
        Attribution::new(
            action_id.clone(),
            target,
            confidence,
            vec![Evidence {
                kind,
                details: Value::Null,
            }],
            vec![intent_id.clone()],
        )
    };

    match tool {
        "Write" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let lines = line_count(content);
            file_contents.insert(path.to_string(), content.to_string());
            vec![make(
                AttributionTarget::LineRange {
                    path: path.to_string(),
                    start: 1,
                    end: lines,
                },
                EXACT_CONFIDENCE,
                EvidenceKind::ExactRange,
            )]
        }
        "Edit" => {
            let old = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            vec![edit_attribution(path, old, new, file_contents, &make)]
        }
        "MultiEdit" => {
            let edits = args.get("edits").and_then(|v| v.as_array());
            match edits {
                Some(edits) if !edits.is_empty() => edits
                    .iter()
                    .map(|e| {
                        let old = e.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                        let new = e.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                        edit_attribution(path, old, new, file_contents, &make)
                    })
                    .collect(),
                // Malformed MultiEdit → whole-file degrade.
                _ => vec![make(
                    AttributionTarget::WholeFile {
                        path: path.to_string(),
                    },
                    WHOLE_FILE_CONFIDENCE,
                    EvidenceKind::Fuzzed,
                )],
            }
        }
        _ => vec![],
    }
}

/// Resolve one Edit to an attribution, updating `file_contents` on success.
fn edit_attribution(
    path: &str,
    old: &str,
    new: &str,
    file_contents: &mut std::collections::HashMap<String, String>,
    make: &impl Fn(AttributionTarget, f32, EvidenceKind) -> Attribution,
) -> Attribution {
    if let Some(content) = file_contents.get(path) {
        if !old.is_empty() {
            if let Some(byte_idx) = content.find(old) {
                let start = content[..byte_idx].matches('\n').count() as u32 + 1;
                let end = start + line_count(new).saturating_sub(1);
                // Apply the edit so subsequent edits resolve against new content.
                let updated = content.replacen(old, new, 1);
                file_contents.insert(path.to_string(), updated);
                return make(
                    AttributionTarget::LineRange {
                        path: path.to_string(),
                        start,
                        end,
                    },
                    EXACT_CONFIDENCE,
                    EvidenceKind::ExactRange,
                );
            }
        }
    }
    // Unknown content or old_string not found → honest whole-file degrade.
    make(
        AttributionTarget::WholeFile {
            path: path.to_string(),
        },
        WHOLE_FILE_CONFIDENCE,
        EvidenceKind::Fuzzed,
    )
}

/// Number of lines a string spans (at least 1; a trailing newline doesn't add one).
fn line_count(s: &str) -> u32 {
    if s.is_empty() {
        return 1;
    }
    s.lines().count().max(1) as u32
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

    fn attr_target_for(store: &ObjectStore, path: &str) -> AttributionTarget {
        store
            .iter_all()
            .unwrap()
            .into_iter()
            .find_map(|n| match n {
                Node::Attribution(a) => {
                    let p = match &a.target {
                        AttributionTarget::LineRange { path, .. }
                        | AttributionTarget::WholeFile { path } => path.clone(),
                        AttributionTarget::CommitSha { .. } => return None,
                    };
                    (p == path).then_some(a.target)
                }
                _ => None,
            })
            .expect("attribution for path")
    }

    #[test]
    fn write_attribution_is_line_range_one_to_n() {
        let (store, _dir) = make_store();
        let content = "line1\nline2\nline3\n";
        let events = vec![
            IngestEvent::UserPrompt {
                text: "create the module".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Write".into(),
                args: json!({ "file_path": "src/new.rs", "content": content }),
                result_summary: "New file created".into(),
                timestamp: Utc::now(),
            },
        ];
        run_pipeline(events, &store).unwrap();
        match attr_target_for(&store, "src/new.rs") {
            AttributionTarget::LineRange { start, end, .. } => {
                assert_eq!((start, end), (1, 3));
            }
            other => panic!("expected LineRange, got {other:?}"),
        }
    }

    #[test]
    fn edit_located_against_prior_read_yields_line_range() {
        let (store, _dir) = make_store();
        // Read provides the file content; the Edit targets line 3.
        let file = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
        let events = vec![
            IngestEvent::UserPrompt {
                text: "rewrite function c".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Read".into(),
                args: json!({ "file_path": "src/x.rs" }),
                result_summary: file.into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/x.rs", "old_string": "fn c() {}", "new_string": "fn c() { c2(); }" }),
                result_summary: "ok".into(),
                timestamp: Utc::now(),
            },
        ];
        run_pipeline(events, &store).unwrap();
        match attr_target_for(&store, "src/x.rs") {
            AttributionTarget::LineRange { start, end, .. } => {
                assert_eq!(start, 3, "fn c() is on line 3");
                assert_eq!(end, 3, "single-line replacement");
            }
            other => panic!("expected LineRange, got {other:?}"),
        }
    }

    #[test]
    fn edit_without_known_content_degrades_to_whole_file() {
        let (store, _dir) = make_store();
        // No preceding Read → content unknown → whole-file degrade.
        let events = vec![
            IngestEvent::UserPrompt {
                text: "tweak it".into(),
                timestamp: Utc::now(),
            },
            IngestEvent::ToolCall {
                name: "Edit".into(),
                args: json!({ "file_path": "src/y.rs", "old_string": "a", "new_string": "b" }),
                result_summary: String::new(),
                timestamp: Utc::now(),
            },
        ];
        run_pipeline(events, &store).unwrap();
        assert!(matches!(
            attr_target_for(&store, "src/y.rs"),
            AttributionTarget::WholeFile { .. }
        ));
    }

    #[test]
    fn is_safe_path_rejects_traversal_and_absolute() {
        assert!(is_safe_path("src/auth.rs"));
        assert!(is_safe_path("a/b/c.rs"));
        assert!(!is_safe_path("../etc/passwd"));
        assert!(!is_safe_path("a/../../b"));
        assert!(!is_safe_path("/etc/passwd"));
        assert!(!is_safe_path("/Users/x/secret"));
    }

    #[test]
    fn line_count_handles_edges() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("one"), 1);
        assert_eq!(line_count("a\nb"), 2);
        assert_eq!(line_count("a\nb\n"), 2);
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
