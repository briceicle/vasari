/// Adapter for Claude Code session JSONL files.
///
/// Claude Code writes each conversation turn as a newline-delimited JSON record
/// at `~/.claude/projects/<session>/<timestamp>.jsonl`.  Each record has a
/// top-level `type` ("human" | "assistant" | "summary" | "system") and a
/// `message` sub-object in the Claude Messages API format.
///
/// Filtering rules that match the ingest plan:
///   • Skip records whose user message begins with "/" (slash-command invocations).
///   • Skip queue-operation records (tool_result entries with no user-visible text).
///   • The *first* non-command, non-empty user text becomes the session Intent.
///   • ToolCall events are emitted for Edit, Write, MultiEdit, Bash, and Read.
///   • Edit / Write / MultiEdit also produce an AttributionTarget via run_pipeline.
use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
    error::VasariError,
    ingest::{IngestAdapter, IngestEvent, IngestSource},
};

pub struct ClaudeCodeAdapter;

impl IngestAdapter for ClaudeCodeAdapter {
    fn parse(&self, source: IngestSource) -> Result<Vec<IngestEvent>, VasariError> {
        let reader: Box<dyn BufRead> = match source {
            IngestSource::File(path) => {
                let file = std::fs::File::open(&path).map_err(VasariError::Io)?;
                Box::new(BufReader::new(file))
            }
            IngestSource::Stdin => Box::new(BufReader::new(std::io::stdin())),
        };

        parse_jsonl(reader)
    }
}

fn parse_jsonl(reader: impl BufRead) -> Result<Vec<IngestEvent>, VasariError> {
    let mut events: Vec<IngestEvent> = Vec::new();
    let mut session_start: Option<DateTime<Utc>> = None;
    let mut session_source: Option<String> = None;
    let mut found_intent = false;

    // Parse every line up front. A tool_use block (assistant record) and its
    // tool_result (the *following* human record) are correlated only by
    // tool_use_id, so we need the whole document before emitting ToolCall events.
    let mut records: Vec<Value> = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx as u64 + 1;
        let line = line.map_err(VasariError::Io)?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => records.push(v),
            // Graceful degradation: record the parse error, keep going.
            Err(e) => events.push(IngestEvent::SystemInstruction {
                text: format!("[parse error on line {line_no}: {e}]"),
            }),
        }
    }

    // Pass 1: build tool_use_id -> result-text from tool_result blocks. This is
    // where Read results (file contents) and Edit/Write confirmations live.
    let mut results: HashMap<String, String> = HashMap::new();
    for record in &records {
        collect_tool_results(record, &mut results);
    }

    // Pass 2: emit events in document order.
    for record in &records {
        // Extract timestamp from the record (top-level "timestamp" field).
        let timestamp = record
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        // Track the earliest timestamp as session start.
        if session_start.map(|st| timestamp < st).unwrap_or(true) {
            session_start = Some(timestamp);
        }

        let record_type = record.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match record_type {
            "system" => {
                // System prompt — contains CLAUDE.md or other instructions.
                if let Some(text) = extract_text_from_record(record) {
                    events.push(IngestEvent::SystemInstruction { text });
                }
            }
            "summary" => {
                // Claude Code compresses old context into a summary record.
                // Treat the summary as a SystemInstruction so constraints in it are captured.
                if let Some(text) = record.get("summary").and_then(|v| v.as_str()) {
                    events.push(IngestEvent::SystemInstruction {
                        text: text.to_string(),
                    });
                }
            }
            "human" => {
                // Human / user turn.  May contain a user text message or tool results.
                let message = match record.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let content = message.get("content");

                // Check for plain-text user messages.
                let text = extract_user_text(content);

                if let Some(text) = text {
                    // Skip slash-command invocations (e.g. "/autoplan …", "/clear").
                    if text.starts_with('/') {
                        continue;
                    }
                    // Skip very short boilerplate ("continue", "y", "ok", etc.).
                    if is_boilerplate(&text) {
                        continue;
                    }

                    let ts = record
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or(timestamp);

                    if !found_intent {
                        // Set the session source label from the first substantive message.
                        let source_label = record
                            .get("uuid")
                            .and_then(|v| v.as_str())
                            .map(|s| format!("claude-code:{s}"))
                            .unwrap_or_else(|| "claude-code:unknown".to_string());
                        session_source = Some(source_label);
                        found_intent = true;
                    }

                    events.push(IngestEvent::UserPrompt {
                        text,
                        timestamp: ts,
                    });
                }
            }
            "assistant" => {
                // Assistant turn — look for tool_use blocks.
                let message = match record.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let content = message.get("content");
                if let Some(content_arr) = content.and_then(|c| c.as_array()) {
                    for block in content_arr {
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            if let Some(ev) = parse_tool_use_block(block, timestamp, &results) {
                                events.push(ev);
                            }
                        }
                    }
                }
            }
            _ => {
                // Unknown record type — skip.
            }
        }
    }

    // Prepend SessionStart now that we know the source label and start time.
    let source = session_source.unwrap_or_else(|| "claude-code:unknown".to_string());
    let started_at = session_start.unwrap_or_else(Utc::now);
    events.insert(0, IngestEvent::SessionStart { source, started_at });

    Ok(events)
}

/// Extract a plain-text string from a user message content field.
/// Content can be a bare string or an array of blocks.
fn extract_user_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(s) = content.as_str() {
        let t = s.trim().to_string();
        return if t.is_empty() { None } else { Some(t) };
    }
    if let Some(arr) = content.as_array() {
        // Collect all text blocks; skip tool_result blocks.
        let mut parts = Vec::new();
        for block in arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        let t = t.trim();
                        if !t.is_empty() {
                            parts.push(t.to_string());
                        }
                    }
                }
                "tool_result" => {
                    // Tool results are not user intent — skip.
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            return None;
        }
        return Some(parts.join("\n"));
    }
    None
}

fn extract_text_from_record(record: &Value) -> Option<String> {
    let message = record.get("message")?;
    extract_user_text(message.get("content"))
}

fn parse_tool_use_block(
    block: &Value,
    timestamp: DateTime<Utc>,
    results: &HashMap<String, String>,
) -> Option<IngestEvent> {
    let name = block.get("name").and_then(|v| v.as_str())?.to_string();

    // Only ingest tools we understand.
    match name.as_str() {
        "Edit" | "Write" | "MultiEdit" | "Bash" | "Read" | "Glob" | "Grep" => {}
        _ => return None,
    }

    let args = block
        .get("input")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    // For tools with paths, validate the path doesn't escape the repo.
    if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
        if path.contains("..") {
            return None;
        }
        // Reject absolute paths to files outside the repo (heuristic: /home, /Users, /etc).
        if path.starts_with('/') {
            return None;
        }
    }

    // Correlate this tool_use with its tool_result (keyed by id). For Read this
    // is the file content; for Edit/Write it is the success confirmation. Empty
    // when the session has no matching result (e.g. truncated export).
    let result_summary = block
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|id| results.get(id))
        .cloned()
        .unwrap_or_default();

    Some(IngestEvent::ToolCall {
        name,
        args,
        result_summary,
        timestamp,
    })
}

/// Collect tool_result blocks from a human record into `out` (tool_use_id -> text).
/// Claude Code attaches each tool's result to the *following* human turn as a
/// `tool_result` block keyed by `tool_use_id`.
fn collect_tool_results(record: &Value, out: &mut HashMap<String, String>) {
    if record.get("type").and_then(|v| v.as_str()) != Some("human") {
        return;
    }
    let Some(content) = record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };
    for block in content {
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_result") {
            continue;
        }
        let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(text) = extract_tool_result_text(block.get("content")) {
            out.insert(id.to_string(), text);
        }
    }
}

/// Extract the textual payload of a tool_result `content` field, which may be a
/// bare string or an array of `{type:"text", text:"..."}` blocks.
fn extract_tool_result_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(s) = content.as_str() {
        let t = s.trim();
        return if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        };
    }
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !t.trim().is_empty() {
                        parts.push(t.to_string());
                    }
                }
            }
        }
        return if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        };
    }
    None
}

/// Very short single-word responses that don't carry intent.
fn is_boilerplate(text: &str) -> bool {
    const BOILERPLATE: &[&str] = &[
        "y",
        "yes",
        "ok",
        "okay",
        "continue",
        "go",
        "go ahead",
        "sure",
        "great",
        "thanks",
        "thank you",
        "looks good",
        "lgtm",
        "done",
        "proceed",
        "next",
        "k",
        "yep",
        "yup",
    ];
    let lower = text.trim().to_lowercase();
    BOILERPLATE.contains(&lower.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn tool_result_is_correlated_into_tool_call() {
        // Read's result (file content) arrives in the *next* human record,
        // keyed by tool_use_id. The adapter must thread it onto the ToolCall.
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","uuid":"u1","message":{"role":"user","content":"Read the auth file"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"src/auth.rs"}}]}}
{"type":"human","timestamp":"2024-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"pub fn authenticate() {}"}]}]}}"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let read_result = events.iter().find_map(|e| match e {
            IngestEvent::ToolCall {
                name,
                result_summary,
                ..
            } if name == "Read" => Some(result_summary.clone()),
            _ => None,
        });
        assert_eq!(read_result.as_deref(), Some("pub fn authenticate() {}"));
    }

    #[test]
    fn tool_call_without_result_has_empty_summary() {
        // No matching tool_result (e.g. truncated export) → empty, not a panic.
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","uuid":"u1","message":{"role":"user","content":"edit it"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t9","name":"Edit","input":{"file_path":"src/x.rs","old_string":"a","new_string":"b"}}]}}"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let edit_result = events.iter().find_map(|e| match e {
            IngestEvent::ToolCall {
                name,
                result_summary,
                ..
            } if name == "Edit" => Some(result_summary.clone()),
            _ => None,
        });
        assert_eq!(edit_result.as_deref(), Some(""));
    }

    #[test]
    fn parses_minimal_session() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","uuid":"u1","message":{"role":"user","content":"Add JWT verification"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"src/auth.rs","old_string":"","new_string":"// jwt"}}]}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let has_user = events
            .iter()
            .any(|e| matches!(e, IngestEvent::UserPrompt { text, .. } if text.contains("JWT")));
        let has_tool = events
            .iter()
            .any(|e| matches!(e, IngestEvent::ToolCall { name, .. } if name == "Edit"));
        assert!(has_user, "should have UserPrompt");
        assert!(has_tool, "should have ToolCall for Edit");
    }

    #[test]
    fn skips_slash_commands() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"/autoplan implement the auth module"}}
{"type":"human","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":"Add JWT verification"}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let user_prompts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::UserPrompt { .. }))
            .collect();
        assert_eq!(user_prompts.len(), 1);
        assert!(
            matches!(&user_prompts[0], IngestEvent::UserPrompt { text, .. } if text.contains("JWT"))
        );
    }

    #[test]
    fn rejects_dotdot_paths() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"test"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"../../etc/passwd"}}]}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let has_tool = events
            .iter()
            .any(|e| matches!(e, IngestEvent::ToolCall { .. }));
        assert!(!has_tool, "path with .. should be rejected");
    }

    #[test]
    fn rejects_absolute_paths() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"test"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/etc/passwd"}}]}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let has_tool = events
            .iter()
            .any(|e| matches!(e, IngestEvent::ToolCall { .. }));
        assert!(!has_tool, "absolute path should be rejected");
    }

    #[test]
    fn skips_boilerplate_user_messages() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"continue"}}
{"type":"human","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":"Add JWT verification to auth"}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let prompts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::UserPrompt { .. }))
            .collect();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn parses_system_record_as_system_instruction() {
        let jsonl = r#"{"type":"system","timestamp":"2024-01-01T00:00:00Z","message":{"role":"system","content":"You must validate all inputs."}}
{"type":"human","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":"Implement it"}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let sys: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::SystemInstruction { .. }))
            .collect();
        assert!(
            !sys.is_empty(),
            "system record should produce SystemInstruction"
        );
    }

    #[test]
    fn parses_summary_record_as_system_instruction() {
        let jsonl = r#"{"type":"summary","timestamp":"2024-01-01T00:00:00Z","summary":"Prior work: implemented JWT verification."}
{"type":"human","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":"Continue with tests"}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let sys: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::SystemInstruction { .. }))
            .collect();
        assert!(
            !sys.is_empty(),
            "summary record should produce SystemInstruction"
        );
    }

    #[test]
    fn gracefully_handles_malformed_json_line() {
        let jsonl = "this is not json\n{\"type\":\"human\",\"timestamp\":\"2024-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"Add JWT verification\"}}\n";
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        // Should not error; malformed line becomes a SystemInstruction with parse error note
        let prompts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::UserPrompt { .. }))
            .collect();
        assert_eq!(prompts.len(), 1, "should still parse the valid line");
    }

    #[test]
    fn content_as_bare_string_is_extracted() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Add JWT auth to the module"}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let prompts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::UserPrompt { text, .. } if text.contains("JWT")))
            .collect();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn tool_result_blocks_are_skipped_as_user_intent() {
        // A human turn containing only tool_result blocks should not produce a UserPrompt
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"real intent"}}
{"type":"human","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"Edit succeeded."}]}]}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let prompts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::UserPrompt { .. }))
            .collect();
        assert_eq!(
            prompts.len(),
            1,
            "tool_result turn should not produce UserPrompt"
        );
    }

    #[test]
    fn unknown_tool_name_is_filtered_out() {
        let jsonl = r#"{"type":"human","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"test"}}
{"type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"UnknownTool","input":{"arg":"val"}}]}}
"#;
        let events = parse_jsonl(Cursor::new(jsonl)).unwrap();
        let tools: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::ToolCall { .. }))
            .collect();
        assert!(tools.is_empty(), "unknown tool should be filtered out");
    }
}
