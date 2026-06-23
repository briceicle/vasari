# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Version scheme: `MAJOR.MINOR.PATCH.BUILD` (gstack convention).

---

## [0.1.1.0] — 2026-06-22

### Added

**vasari-core** (`crates/vasari-core`)

- `vasari_core::why(store, path, line)` — resolve a file:line to the intent chain that produced it.
  Returns `Option<ResolveChain>` (highest-confidence match), or `None` if no attribution covers
  that line.
- `vasari_core::why_all(store, path, line)` — returns all overlapping `ResolveChain` results for
  cases where multiple agent actions cover the same line range.
- `ResolveChain` struct: answer-first field layout (`intents`, `plan`, `plan_step_index`, `action`,
  `attribution`, `constraints`). Convenience methods: `plan_step()`, `confidence()`,
  `primary_intent()`.
- `VasariError::PlanStepOutOfBounds` — propagated when an `Action` references a step index
  beyond `plan.steps.len()`, indicating graph corruption. Error message includes the plan ID,
  requested index, and actual step count.
- `AttributionNotFound` error now embeds remediation guidance (`vasari ingest` / `vasari fsck`)
  so library callers (including future Python/UniFFI bindings) see actionable messages.
- 7 integration tests in `resolve.rs`: full happy path, empty store, highest-confidence
  selection, multiple overlapping attributions, out-of-bounds step index, multi-intent plan,
  zero-intent plan, missing action node soft-log.

### Changed

**vasari** (`crates/vasari`)

- `vasari why` now uses `vasari_core::why_all` and displays results intent-first: the "why"
  answer (intent text + source + timestamp) leads, followed by the plan step and tool, then
  confidence. Attribution node IDs are no longer shown in default output.
- `vasari why --json` flag added: outputs one JSON object per attribution chain on stdout,
  suitable for piping into dashboards or other tools. Fields: `intent_text`, `intent_source`,
  `intent_at`, `plan_step_goal`, `plan_step_index`, `plan_step_total`, `action_tool`,
  `confidence`, `attribution_id`.

[0.1.1.0]: https://github.com/briceicle/vasari/releases/tag/v0.1.1.0

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
