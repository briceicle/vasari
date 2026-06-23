/// Adapter for OTLP JSON exports with GenAI semantic conventions ≥ 1.30.0.
///
/// Reads a single OTLP JSON file (or stdin) and reconstructs the span tree
/// via a two-pass algorithm — OTLP exports are not topologically sorted, so
/// we index all spans by spanId first, then walk from roots down.
///
/// GenAI spans of interest:
///   gen_ai.operation.name = "chat" | "generate" | "complete" | "stream"
///   gen_ai.system           — model vendor ("anthropic", "openai", …)
///   gen_ai.request.model    — model name
///   gen_ai.prompt           — user prompt text (deprecated but widely used)
///   gen_ai.completion       — model completion text (deprecated but widely used)
///
/// Mapping to Vasari nodes:
///   root span (no parentSpanId, gen_ai op) → SessionStart + UserPrompt
///   child spans                             → ToolCall events
use std::collections::HashMap;
use std::io::{BufReader, Read};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::{
    error::VasariError,
    ingest::{IngestAdapter, IngestEvent, IngestSource},
};

pub struct OtelGenAiAdapter;

impl IngestAdapter for OtelGenAiAdapter {
    fn parse(&self, source: IngestSource) -> Result<Vec<IngestEvent>, VasariError> {
        let mut buf = String::new();
        match source {
            IngestSource::File(path) => {
                std::fs::File::open(&path)
                    .map_err(VasariError::Io)?
                    .read_to_string(&mut buf)
                    .map_err(VasariError::Io)?;
            }
            IngestSource::Stdin => {
                BufReader::new(std::io::stdin())
                    .read_to_string(&mut buf)
                    .map_err(VasariError::Io)?;
            }
        }
        parse_otlp_json(&buf)
    }
}

fn parse_otlp_json(json: &str) -> Result<Vec<IngestEvent>, VasariError> {
    let root: Value = serde_json::from_str(json)?;

    // Collect all spans from resourceSpans[*].scopeSpans[*].spans[*]
    let mut all_spans: Vec<Value> = Vec::new();

    if let Some(resource_spans) = root.get("resourceSpans").and_then(|v| v.as_array()) {
        for rs in resource_spans {
            if let Some(scope_spans) = rs.get("scopeSpans").and_then(|v| v.as_array()) {
                for ss in scope_spans {
                    if let Some(spans) = ss.get("spans").and_then(|v| v.as_array()) {
                        all_spans.extend(spans.iter().cloned());
                    }
                }
            }
        }
    }

    if all_spans.is_empty() {
        return Ok(vec![]);
    }

    // Pass 1: index by spanId
    let mut by_span_id: HashMap<String, &Value> = HashMap::new();
    for span in &all_spans {
        if let Some(id) = span.get("spanId").and_then(|v| v.as_str()) {
            by_span_id.insert(id.to_string(), span);
        }
    }

    // Pass 2: walk from roots (no parentSpanId or parentSpanId not in index)
    let roots: Vec<&Value> = all_spans
        .iter()
        .filter(|span| {
            span.get("parentSpanId")
                .and_then(|v| v.as_str())
                .map(|pid| pid.is_empty() || !by_span_id.contains_key(pid))
                .unwrap_or(true)
        })
        .collect();

    let mut events: Vec<IngestEvent> = Vec::new();
    let mut saw_session_start = false;

    let earliest_ts = all_spans
        .iter()
        .filter_map(|s| s.get("startTimeUnixNano").and_then(parse_unix_nano))
        .min()
        .unwrap_or_else(Utc::now);

    for root_span in roots {
        let span_start = root_span
            .get("startTimeUnixNano")
            .and_then(parse_unix_nano)
            .unwrap_or(earliest_ts);

        let attrs = span_attrs(root_span);
        let op = attrs.get("gen_ai.operation.name").map(|s| s.as_str());
        let system = attrs.get("gen_ai.system").map(|s| s.as_str());
        let model = attrs.get("gen_ai.request.model").map(|s| s.as_str());

        // Only process spans that look like GenAI operations.
        let is_gen_ai = op.is_some()
            || system.is_some()
            || attrs.contains_key("gen_ai.prompt");
        if !is_gen_ai {
            continue;
        }

        // Emit SessionStart from the first root GenAI span.
        if !saw_session_start {
            let source_label = format!(
                "otel:{}:{}",
                system.unwrap_or("unknown"),
                model.unwrap_or("unknown")
            );
            events.push(IngestEvent::SessionStart {
                source: source_label,
                started_at: earliest_ts,
            });
            saw_session_start = true;
        }

        // UserPrompt from gen_ai.prompt attribute.
        if let Some(prompt) = attrs.get("gen_ai.prompt") {
            let text = prompt.trim().to_string();
            if !text.is_empty() {
                events.push(IngestEvent::UserPrompt {
                    text,
                    timestamp: span_start,
                });
            }
        }

        // Walk children.
        let span_id = root_span
            .get("spanId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let children: Vec<&Value> = all_spans
            .iter()
            .filter(|s| {
                s.get("parentSpanId")
                    .and_then(|v| v.as_str())
                    .map(|pid| pid == span_id)
                    .unwrap_or(false)
            })
            .collect();

        for child in children {
            if let Some(ev) = child_span_to_event(child) {
                events.push(ev);
            }
        }

        // Completion as SystemInstruction so constraints are extracted.
        if let Some(completion) = attrs.get("gen_ai.completion") {
            let text = completion.trim().to_string();
            if !text.is_empty() {
                events.push(IngestEvent::SystemInstruction { text });
            }
        }
    }

    // Orphan spans: parent was referenced but not present in the export.
    // Emit them as ToolCalls so we don't silently drop recorded work.
    for span in &all_spans {
        let parent = span
            .get("parentSpanId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !parent.is_empty() && !by_span_id.contains_key(parent) {
            if let Some(ev) = child_span_to_event(span) {
                events.push(ev);
            }
        }
    }

    Ok(events)
}

/// Map a child span to a ToolCall IngestEvent.
fn child_span_to_event(span: &Value) -> Option<IngestEvent> {
    let name = span.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
    let attrs = span_attrs(span);
    let timestamp = span
        .get("startTimeUnixNano")
        .and_then(parse_unix_nano)
        .unwrap_or_else(Utc::now);

    let result_summary = attrs
        .get("gen_ai.completion")
        .map(|s| s.to_string())
        .unwrap_or_default();

    let mut args = serde_json::Map::new();
    for (k, v) in &attrs {
        args.insert(k.clone(), Value::String(v.clone()));
    }

    Some(IngestEvent::ToolCall {
        name: name.to_string(),
        args: Value::Object(args),
        result_summary,
        timestamp,
    })
}

/// Extract span attributes into a flat String→String map.
fn span_attrs(span: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(attrs) = span.get("attributes").and_then(|v| v.as_array()) {
        for attr in attrs {
            let key = attr.get("key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                continue;
            }
            if let Some(val) = attr.get("value") {
                let str_val = extract_attr_value(val);
                if !str_val.is_empty() {
                    map.insert(key.to_string(), str_val);
                }
            }
        }
    }
    map
}

/// OTLP attribute values are typed: { "stringValue": "…" } | { "intValue": "…" } | etc.
fn extract_attr_value(val: &Value) -> String {
    if let Some(s) = val.get("stringValue").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(i) = val.get("intValue").and_then(|v| v.as_i64()) {
        return i.to_string();
    }
    if let Some(i) = val.get("intValue").and_then(|v| v.as_str()) {
        return i.to_string();
    }
    if let Some(d) = val.get("doubleValue").and_then(|v| v.as_f64()) {
        return d.to_string();
    }
    if let Some(b) = val.get("boolValue").and_then(|v| v.as_bool()) {
        return b.to_string();
    }
    String::new()
}

/// Parse an OTLP Unix nanosecond timestamp (string or number) into DateTime<Utc>.
fn parse_unix_nano(val: &Value) -> Option<DateTime<Utc>> {
    let nanos: i64 = if let Some(s) = val.as_str() {
        s.parse().ok()?
    } else if let Some(n) = val.as_i64() {
        n
    } else if let Some(n) = val.as_u64() {
        n as i64
    } else {
        return None;
    };
    let secs = nanos / 1_000_000_000;
    let nsecs = (nanos % 1_000_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_otlp(prompt: &str) -> String {
        format!(
            r#"{{
  "resourceSpans": [{{
    "resource": {{"attributes": []}},
    "scopeSpans": [{{
      "scope": {{"name": "opentelemetry-anthropic", "version": "0.1.0"}},
      "spans": [{{
        "traceId": "abcdef1234567890abcdef1234567890",
        "spanId": "1234567890abcdef",
        "name": "gen_ai.chat",
        "startTimeUnixNano": "1704067200000000000",
        "endTimeUnixNano": "1704067205000000000",
        "attributes": [
          {{"key": "gen_ai.system", "value": {{"stringValue": "anthropic"}}}},
          {{"key": "gen_ai.operation.name", "value": {{"stringValue": "chat"}}}},
          {{"key": "gen_ai.request.model", "value": {{"stringValue": "claude-3-5-sonnet"}}}},
          {{"key": "gen_ai.prompt", "value": {{"stringValue": "{prompt}"}}}}
        ]
      }}]
    }}]
  }}]
}}"#
        )
    }

    #[test]
    fn parses_minimal_otlp() {
        let json = minimal_otlp("Add JWT verification to the auth module");
        let events = parse_otlp_json(&json).unwrap();

        let has_session = events
            .iter()
            .any(|e| matches!(e, IngestEvent::SessionStart { .. }));
        let has_prompt = events.iter().any(|e| {
            matches!(e, IngestEvent::UserPrompt { text, .. } if text.contains("JWT"))
        });

        assert!(has_session, "should have SessionStart");
        assert!(has_prompt, "should have UserPrompt from gen_ai.prompt");
    }

    #[test]
    fn empty_spans_produces_empty_events() {
        let json = r#"{"resourceSpans": []}"#;
        let events = parse_otlp_json(json).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn non_gen_ai_spans_are_skipped() {
        let json = r#"{
  "resourceSpans": [{
    "scopeSpans": [{
      "spans": [{
        "spanId": "abc123",
        "name": "http.request",
        "startTimeUnixNano": "1704067200000000000",
        "attributes": [
          {"key": "http.method", "value": {"stringValue": "GET"}}
        ]
      }]
    }]
  }]
}"#;
        let events = parse_otlp_json(json).unwrap();
        assert!(events.is_empty(), "non-gen_ai spans should produce no events");
    }

    #[test]
    fn orphan_span_becomes_tool_call() {
        // A span whose parentSpanId is not in the export (orphan)
        let json = r#"{
  "resourceSpans": [{
    "scopeSpans": [{
      "spans": [{
        "spanId": "orphan001",
        "parentSpanId": "missing001",
        "name": "tool.call",
        "startTimeUnixNano": "1704067200000000000",
        "attributes": [
          {"key": "gen_ai.operation.name", "value": {"stringValue": "tool"}}
        ]
      }]
    }]
  }]
}"#;
        let events = parse_otlp_json(json).unwrap();
        let tool_calls: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::ToolCall { .. }))
            .collect();
        assert!(!tool_calls.is_empty(), "orphan span should become a ToolCall");
    }

    #[test]
    fn gen_ai_completion_becomes_system_instruction() {
        let json = r#"{
  "resourceSpans": [{
    "scopeSpans": [{
      "spans": [{
        "spanId": "root001",
        "name": "gen_ai.chat",
        "startTimeUnixNano": "1704067200000000000",
        "attributes": [
          {"key": "gen_ai.operation.name", "value": {"stringValue": "chat"}},
          {"key": "gen_ai.prompt", "value": {"stringValue": "Implement JWT auth"}},
          {"key": "gen_ai.completion", "value": {"stringValue": "I will add JWT verification to your module."}}
        ]
      }]
    }]
  }]
}"#;
        let events = parse_otlp_json(json).unwrap();
        let sys: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, IngestEvent::SystemInstruction { text } if text.contains("JWT verification")))
            .collect();
        assert!(!sys.is_empty(), "gen_ai.completion should become SystemInstruction");
    }

    #[test]
    fn extract_attr_value_int_as_string() {
        // OTLP sometimes encodes int64 as a JSON string
        let val = serde_json::json!({"intValue": "42"});
        assert_eq!(extract_attr_value(&val), "42");
    }

    #[test]
    fn extract_attr_value_double() {
        let val = serde_json::json!({"doubleValue": 3.14});
        assert_eq!(extract_attr_value(&val), "3.14");
    }

    #[test]
    fn extract_attr_value_bool() {
        let val = serde_json::json!({"boolValue": true});
        assert_eq!(extract_attr_value(&val), "true");
    }

    #[test]
    fn parse_unix_nano_string_form() {
        let val = serde_json::json!("1704067200000000000");
        let dt = parse_unix_nano(&val).unwrap();
        assert_eq!(dt.timestamp(), 1704067200);
    }

    #[test]
    fn parse_unix_nano_u64_form() {
        let val = serde_json::json!(1704067200000000000u64);
        let dt = parse_unix_nano(&val).unwrap();
        assert_eq!(dt.timestamp(), 1704067200);
    }

    #[test]
    fn two_pass_reconstructs_parent_child() {
        // Child span appears before parent in the export (unsorted).
        let json = r#"{
  "resourceSpans": [{
    "scopeSpans": [{
      "spans": [
        {
          "spanId": "child0001",
          "parentSpanId": "root0001",
          "name": "tool.use",
          "startTimeUnixNano": "1704067201000000000",
          "attributes": []
        },
        {
          "spanId": "root0001",
          "name": "gen_ai.chat",
          "startTimeUnixNano": "1704067200000000000",
          "attributes": [
            {"key": "gen_ai.operation.name", "value": {"stringValue": "chat"}},
            {"key": "gen_ai.prompt", "value": {"stringValue": "Implement auth"}}
          ]
        }
      ]
    }]
  }]
}"#;
        let events = parse_otlp_json(json).unwrap();
        let has_session = events
            .iter()
            .any(|e| matches!(e, IngestEvent::SessionStart { .. }));
        let has_prompt = events
            .iter()
            .any(|e| matches!(e, IngestEvent::UserPrompt { .. }));
        let has_tool = events
            .iter()
            .any(|e| matches!(e, IngestEvent::ToolCall { .. }));
        assert!(has_session);
        assert!(has_prompt);
        assert!(has_tool, "child span should become ToolCall");
    }
}
