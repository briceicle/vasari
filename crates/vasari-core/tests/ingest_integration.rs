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

/// Regression test for ingesting *real* Claude Code sessions.
///
/// Real sessions differ from the hand-written fixtures in two ways that each
/// silently produced zero attributions before the fix:
///   1. records carry `type:"user"` (not `"human"`), so every user turn — and
///      therefore the session Intent — was skipped;
///   2. tool `file_path`s are absolute (e.g. `/Users/dev/myrepo/src/auth.rs`),
///      so `is_safe_path` rejected them and dropped every Edit/Write, and even
///      if stored they would not match the repo-relative path `vasari why` queries.
///
/// This fixture mirrors the real on-disk schema; the assertions fail loudly if
/// either regression returns.
#[test]
fn ingests_real_session_user_type_and_absolute_paths() {
    let (store, _dir) = open_store();

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/claude_code/real-session-shape.jsonl");

    let events = ClaudeCodeAdapter
        .parse(IngestSource::File(fixture))
        .expect("fixture should parse without error");

    let summary = run_pipeline(events, &store).expect("pipeline should complete");

    // Bug 1: `type:"user"` records must form an Intent.
    assert!(
        summary.intents_created >= 1,
        "a `user`-type prompt must produce an Intent (got {})",
        summary.intents_created
    );
    // Bug 2: absolute file_paths must not be dropped — Edit + Write attribute.
    assert!(
        summary.attributions_created >= 1,
        "absolute file_paths must still produce attributions (got {})",
        summary.attributions_created
    );

    // Bug 3: coverage is keyed by the *repo-relative* path the user queries,
    // not the absolute path recorded in the session.
    let relative = store
        .lookup_attributions("src/auth.rs", 1)
        .expect("lookup should not error");
    assert!(
        !relative.is_empty(),
        "src/auth.rs (relative) should have attribution coverage"
    );
    let absolute = store
        .lookup_attributions("/Users/dev/myrepo/src/auth.rs", 1)
        .expect("lookup should not error");
    assert!(
        absolute.is_empty(),
        "absolute paths must not leak into the attribution index"
    );

    // The chain resolves back to the user's stated intent.
    let chain = vasari_core::resolve::why(&store, "src/auth.rs", 1)
        .expect("resolve should not error")
        .expect("src/auth.rs:1 should resolve to a chain");
    assert!(
        chain
            .primary_intent()
            .is_some_and(|i| i.text.contains("JWT")),
        "intent should carry the user's goal, got {:?}",
        chain.primary_intent().map(|i| &i.text)
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
                match &a.target {
                    AttributionTarget::LineRange { path, .. }
                    | AttributionTarget::WholeFile { path } => return Some(path.clone()),
                    AttributionTarget::CommitSha { .. } => {}
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
