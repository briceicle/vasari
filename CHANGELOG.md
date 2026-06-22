# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Version scheme: `MAJOR.MINOR.PATCH.BUILD` (gstack convention).

---

## [0.1.0.0] — 2026-06-22

### Added

**vasari-core** (`crates/vasari-core`)

- Five-node content-addressed intent graph: `Intent`, `Plan`, `Constraint`, `Action`, `Attribution`
- Content hashing via `sha256(JCS(hash_input))` with RFC 8785 key-sorting
  (partial compliance: key order stable; number and unicode escaping deferred to v0.2)
- `Action.result_summary` and `Attribution.confidence` intentionally excluded from content
  hash — annotations that may be recalibrated without changing node identity
- Git-like object store at `<repo>/.vasari/objects/<sha[0..2]>/<sha[2..]>` with gzip compression
- Derivable line-range index at `index/targets/<encoded-path>/<start>-<end>`
- `encode_path` sanitizes `..` and `.` path components to block index directory traversal
- `ObjectStore::rebuild_index()` for `vasari fsck` recovery
- `VasariError` with `DegradedReason` typed enum for never-panic ingest pipeline
- Unit tests: round-trip, idempotent put, attribution index lookup, confidence hash exclusion,
  key order stability, hash determinism

**vasari** (`crates/vasari`)

- CLI binary with `why <file>:<line>`, `diff <plan-a> <plan-b>`, `ingest`, `verify`, `fsck`
- `vasari why`: Attribution → Action → Plan.steps[] → Intent traversal with confidence display
- `vasari diff`: goal-string alignment, first divergence detection
- `vasari fsck`: full index rebuild from object store
- `vasari ingest --adapter {claude-code,otel-genai}`: stubbed, returns "not yet implemented"
- `vasari verify`: stubbed

**Docs / Spec**

- `INTENT-SPEC.md`: node type table, canonicalization rules, storage layout, v0.1 ship gate
  (≥80% Intent accuracy on 100-line labeled corpus, 95% CI lower bound ≥70%)
- `docs/why-not-just-in-toto.md`: architecture decision record — use in-toto as signing
  envelope for `vasari verify` only; DAG traversal, confidence, and diff are outside in-toto's spec
- `tests/corpus/attribution/README.md`: labeling protocol, privacy rules, calibration pilot

### Not yet implemented

- `vasari ingest --adapter claude-code` — Claude Code session JSONL parsing (v0.2)
- `vasari ingest --adapter otel-genai` — OTEL GenAI span parsing (v0.2)
- `vasari verify` — Sigstore keyless DSSE signing (v0.2)
- Full RFC 8785 number and unicode escaping in canonical JSON (v0.2)
- Python wheel via maturin / UniFFI bindings (v0.2)
- CI/CD release workflow (tracked in TODOS.md)

[0.1.0.0]: https://github.com/briceicle/vasari/releases/tag/v0.1.0.0
