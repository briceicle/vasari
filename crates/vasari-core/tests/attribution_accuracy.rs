//! Attribution accuracy gate.
//!
//! Two tests:
//!   * `evaluator_scores_synthetic_corpus` — always runs; exercises the evaluator
//!     code path on a tiny in-process corpus so it can't bit-rot while the real
//!     gate is `#[ignore]`d.
//!   * `attribution_accuracy_gate` — `#[ignore]`d; runs the real machine corpus
//!     under `tests/corpus/attribution/`. This is the CI gate.
//!     Run it with: `bash tests/corpus/attribution/run.sh`

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;

use vasari_core::adapters::claude_code::ClaudeCodeAdapter;
use vasari_core::eval::{evaluate, parse_labels, GateStatus, Outcome, MATCH_THRESHOLD, N_MIN};
use vasari_core::ingest::{run_pipeline, IngestAdapter, IngestEvent, IngestSource};
use vasari_core::ObjectStore;

/// Ingest one synthetic single-intent session that edits `files`.
fn ingest_session(store: &ObjectStore, source: &str, prompt: &str, files: &[&str]) {
    let ts = Utc::now();
    let mut events = vec![
        IngestEvent::SessionStart {
            source: source.to_string(),
            started_at: ts,
        },
        IngestEvent::UserPrompt {
            text: prompt.to_string(),
            timestamp: ts,
        },
    ];
    for f in files {
        events.push(IngestEvent::ToolCall {
            name: "Edit".to_string(),
            args: json!({ "file_path": f }),
            result_summary: "ok".to_string(),
            timestamp: ts,
        });
    }
    run_pipeline(events, store).expect("synthetic ingest must succeed");
}

#[test]
fn evaluator_scores_synthetic_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(dir.path()).unwrap();

    // Two distinct intents; src/shared.rs is touched by BOTH → multi-intent file.
    ingest_session(
        &store,
        "sess-a",
        "add rate limit middleware to the api",
        &["src/api.rs", "src/shared.rs"],
    );
    ingest_session(
        &store,
        "sess-b",
        "fix the logging format in the logger",
        &["src/logger.rs", "src/shared.rs"],
    );

    let labels_jsonl = r#"
{"file":"src/api.rs","line":1,"expected_intent":["add rate limit middleware to the api"]}
{"file":"src/api.rs","line":1,"expected_intent":["add rate limit middleware to the api"]}
{"file":"src/logger.rs","line":1,"expected_intent":["fix the logging format in the logger"]}
{"file":"src/shared.rs","line":1,"expected_intent":["add rate limit middleware to the api","fix the logging format in the logger"]}
{"file":"src/untouched.rs","line":1,"expected_intent":[]}
"#;
    let labels = parse_labels(labels_jsonl.trim()).unwrap();
    let report = evaluate(&store, &labels, MATCH_THRESHOLD).unwrap();

    // The duplicate src/api.rs:1 label is deduped → 4 unique rows, not 5.
    assert_eq!(report.n, 4);
    // n < N_MIN, so the gate is reported but not evaluated.
    assert_eq!(report.status(), GateStatus::InsufficientCorpus);

    let row = |f: &str| report.rows.iter().find(|r| r.file == f).unwrap();

    // Single-intent files resolve to exactly their intent.
    assert_eq!(row("src/api.rs").outcome, Outcome::PrecisionHit);
    assert_eq!(row("src/logger.rs").outcome, Outcome::PrecisionHit);
    // A file no intent touched should return nothing.
    assert_eq!(row("src/untouched.rs").outcome, Outcome::NoAnswerCorrect);
    // The multi-intent file is flagged, and the right intent is at least recalled
    // (precision may lose to the hash tie-break on equal-confidence whole-file chains).
    let shared = row("src/shared.rs");
    assert!(shared.multi_intent);
    assert!(
        matches!(shared.outcome, Outcome::PrecisionHit | Outcome::RecallOnly),
        "multi-intent file must at least recall-hit, got {:?}",
        shared.outcome
    );

    // Single-intent accuracy should be perfect; the split must be reported.
    assert_eq!(report.single_correct, report.single_total);
    assert!(report.render().contains("gate:"));
    assert!(report.render().contains("multi-intent files"));
}

#[test]
#[ignore = "needs committed corpus; run: bash tests/corpus/attribution/run.sh"]
fn attribution_accuracy_gate() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/attribution");
    let sessions_dir = corpus.join("sessions");
    let labels_dir = corpus.join("labels");

    let session_files = jsonl_files(&sessions_dir);
    assert!(
        !session_files.is_empty(),
        "no corpus sessions in {} — generate them via tests/corpus/attribution/SCRIPT.md",
        sessions_dir.display()
    );

    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(dir.path()).unwrap();
    for s in &session_files {
        let events = ClaudeCodeAdapter
            .parse(IngestSource::File(s.clone()))
            .unwrap_or_else(|e| panic!("parsing {}: {e}", s.display()));
        run_pipeline(events, &store).unwrap_or_else(|e| panic!("ingesting {}: {e}", s.display()));
    }

    let mut labels = Vec::new();
    for l in jsonl_files(&labels_dir) {
        let text = fs::read_to_string(&l).unwrap();
        labels.extend(
            parse_labels(&text).unwrap_or_else(|e| panic!("parsing labels {}: {e}", l.display())),
        );
    }
    assert!(
        !labels.is_empty(),
        "no labels in {} — see tests/corpus/attribution/LABELING.md",
        labels_dir.display()
    );

    let report = evaluate(&store, &labels, MATCH_THRESHOLD).unwrap();
    let rendered = report.render();
    println!("\n{rendered}");

    // Write the full table where a failing-CI developer can recover it.
    let report_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/attribution-report.txt");
    let _ = fs::write(&report_path, &rendered);

    match report.status() {
        GateStatus::Pass => {}
        GateStatus::Fail => panic!(
            "ACCURACY GATE FAILED: {}\nFull report: {}\nRun: bash tests/corpus/attribution/run.sh",
            report.summary_line(),
            report_path.display()
        ),
        GateStatus::InsufficientCorpus => panic!(
            "INSUFFICIENT_CORPUS: {} (n < {N_MIN}). Grow the corpus via tests/corpus/attribution/SCRIPT.md.\nFull report: {}",
            report.summary_line(),
            report_path.display()
        ),
    }
}

/// All `*.jsonl` files in `dir` (empty if the dir is absent).
fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
