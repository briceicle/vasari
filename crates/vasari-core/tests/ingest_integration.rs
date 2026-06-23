/// End-to-end ingest integration tests.
///
/// These tests exercise the full pipeline from raw session data through storage
/// to the `vasari why`-equivalent lookup.  They don't require a running process —
/// they call the library directly.
use vasari_core::{
    adapters::claude_code::ClaudeCodeAdapter,
    adapters::otel::OtelGenAiAdapter,
    ingest::{run_pipeline, IngestAdapter, IngestSource},
    schema::AttributionTarget,
    Node, ObjectStore,
};

fn open_store() -> (ObjectStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(dir.path()).unwrap();
    (store, dir)
}

/// Golden-file test: ingest the simple-edit-session fixture and verify that
/// `lookup_attributions("src/auth.rs", 1)` returns at least one entry that
/// chains back to an Intent with the expected text.
#[test]
fn golden_claude_code_ingest_end_to_end() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/claude_code/simple-edit-session.jsonl");

    let events = ClaudeCodeAdapter
        .parse(IngestSource::File(fixture))
        .expect("fixture should parse without error");

    let summary = run_pipeline(events, &store).expect("pipeline should complete");

    assert_eq!(summary.intents_created, 1, "one session → one intent");
    assert_eq!(summary.plans_created, 1, "one plan per session");
    assert!(summary.actions_created >= 2, "at least Edit + Write");
    assert!(
        summary.attributions_created >= 2,
        "at least Edit + Write produce attributions"
    );
    assert!(
        summary.constraints_created >= 2,
        "must validate + never store keywords"
    );
    assert!(
        summary.degraded.is_empty(),
        "no degradations expected on clean fixture"
    );

    // Verify attribution lookup reaches an Intent.
    let attr_ids = store
        .lookup_attributions("src/auth.rs", 1)
        .expect("lookup should not error");
    assert!(
        !attr_ids.is_empty(),
        "src/auth.rs should have attribution coverage"
    );

    let Some(Node::Attribution(attr)) = store.get(&attr_ids[0]).unwrap() else {
        panic!("attribution node should be in store");
    };
    let Some(Node::Action(action)) = store.get(&attr.action_id).unwrap() else {
        panic!("action node should be in store");
    };
    let Some(Node::Plan(plan)) = store.get(&action.plan_ref.plan_id).unwrap() else {
        panic!("plan node should be in store");
    };
    let intent_id = plan
        .intent_ids
        .first()
        .expect("plan has at least one intent");
    let Some(Node::Intent(intent)) = store.get(intent_id).unwrap() else {
        panic!("intent node should be in store");
    };

    assert!(
        intent.text.contains("JWT"),
        "intent text should contain the user's goal: got '{}'",
        intent.text
    );
}

/// Idempotency test: ingesting the same fixture twice should not duplicate
/// attributions in the lookup result.
#[test]
fn ingest_is_idempotent() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/claude_code/simple-edit-session.jsonl");

    for _ in 0..2 {
        let events = ClaudeCodeAdapter
            .parse(IngestSource::File(fixture.clone()))
            .expect("fixture should parse");
        run_pipeline(events, &store).expect("pipeline should complete");
    }

    // Verify that re-ingesting the same session doesn't create a second Intent.
    let nodes = store.iter_all().unwrap();
    let intent_count = nodes
        .iter()
        .filter(|n| matches!(n, Node::Intent(_)))
        .count();
    assert_eq!(
        intent_count, 1,
        "two ingest runs of the same session should produce exactly one Intent"
    );

    let attr_ids = store
        .lookup_attributions("src/auth.rs", 1)
        .expect("lookup should not error");

    // Dedup-on-read means the second ingest should not increase the count.
    // There should be exactly as many entries as unique attribution nodes.
    let mut sorted = attr_ids.clone();
    sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    sorted.dedup();
    assert_eq!(
        attr_ids.len(),
        sorted.len(),
        "lookup_attributions should deduplicate on read"
    );
}

/// OTEL golden-file test: verify that the simple-gen-ai fixture produces
/// at least one Intent with a non-empty text.
#[test]
fn golden_otel_ingest_creates_intent() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/otel/simple-gen-ai.json");

    let events = OtelGenAiAdapter
        .parse(IngestSource::File(fixture))
        .expect("fixture should parse without error");

    let summary = run_pipeline(events, &store).expect("pipeline should complete");

    assert_eq!(summary.intents_created, 1);
    assert!(
        summary.actions_created >= 1,
        "at least one ToolCall expected"
    );
}

/// Verify that `iter_all` + Intent filtering backs `vasari sessions`.
#[test]
fn iter_all_returns_ingested_intents() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/claude_code/simple-edit-session.jsonl");
    let events = ClaudeCodeAdapter
        .parse(IngestSource::File(fixture))
        .unwrap();
    run_pipeline(events, &store).unwrap();

    let nodes = store.iter_all().unwrap();
    let intents: Vec<_> = nodes
        .iter()
        .filter(|n| matches!(n, Node::Intent(_)))
        .collect();
    assert_eq!(intents.len(), 1);

    let attributions: Vec<_> = nodes
        .iter()
        .filter(|n| matches!(n, Node::Attribution(_)))
        .collect();
    assert!(!attributions.is_empty());
}

/// Verify that `iter_all` backs `vasari files` — attributed paths are discovered.
#[test]
fn iter_all_discovers_attributed_files() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/claude_code/simple-edit-session.jsonl");
    let events = ClaudeCodeAdapter
        .parse(IngestSource::File(fixture))
        .unwrap();
    run_pipeline(events, &store).unwrap();

    let nodes = store.iter_all().unwrap();
    let paths: std::collections::HashSet<String> = nodes
        .iter()
        .filter_map(|n| {
            if let Node::Attribution(a) = n {
                if let AttributionTarget::LineRange { path, .. } = &a.target {
                    return Some(path.clone());
                }
            }
            None
        })
        .collect();

    assert!(
        paths.contains("src/auth.rs"),
        "src/auth.rs should be attributed"
    );
    assert!(
        paths.contains("src/jwt.rs"),
        "src/jwt.rs should be attributed"
    );
}
