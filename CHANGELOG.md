# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Version scheme: `MAJOR.MINOR.PATCH.BUILD` (gstack convention).

---

## [Unreleased]

### Changed

**vasari-core** — attribution quality on real sessions. With the parser fixed,
`vasari why` resolved every line to the session's umbrella prompt with a generic
`Edit <file>` step, and ingested context-compaction recaps as if they were intents:

- **Plan steps now carry the agent's stated rationale.** The `text`/`thinking`
  narration that precedes a tool call (in real sessions, in its own record) is
  threaded through `IngestEvent::ToolCall` and becomes the plan-step goal, so
  `vasari why`'s "via" line reads e.g. *"add a shared path validator"* instead of
  *"Edit src/foo.rs"*. Falls back to the `<tool> <path>` label when absent.
- **Auto-generated `user` turns are no longer treated as intents.** Context
  compaction recaps ("This session is being continued…") and local-command
  output caveats are filtered, so they no longer surface as multi-paragraph
  "why" answers.

### Fixed

**vasari-core** (`crates/vasari-core`) — `vasari why` returned **zero attributions
on real Claude Code sessions**. Three compounding parser bugs, each masked by the
synthetic `"human"`-typed test fixtures:

- **Record type.** The adapter only matched `type:"human"`, but real sessions emit
  `type:"user"`. Every user turn — including the one that becomes the session
  Intent — was skipped, so ingest produced nothing. Both spellings are now accepted.
- **Absolute paths dropped.** Real sessions record absolute tool `file_path`s; the
  shared `is_safe_path` check rejected anything starting with `/`, so every
  Edit/Write/Read was discarded at the adapter and again in the attribution
  synthesizer. Paths are now relativized against each record's `cwd` before the
  safety check.
- **Query/storage path mismatch.** Even when stored, absolute paths never matched
  the repo-relative path `vasari why <path>` queries with. Relativization fixes
  this end-to-end.

- `tests/fixtures/claude_code/real-session-shape.jsonl` + `ingest_integration`
  regression test mirroring the real on-disk schema (`user` type, `cwd`, absolute
  paths) so these regressions fail loudly; unit tests for `relativize`.

## [0.2.1.0] — 2026-06-24

### Added

**vasari-core** (`crates/vasari-core`)

- `eval` module — attribution accuracy gate for `vasari why`. Scores the resolver
  against a labeled corpus: Wilson score lower bound (plain form), token-overlap intent
  matching, precision + recall, and **single-intent vs multi-intent file accuracy
  reported separately** so a blended number can't hide a multi-intent collapse. The
  gate is only evaluated at `n ≥ 100` (`InsufficientCorpus` below that — a 10-line
  pilot can't clear the 70% Wilson floor). Measures file→intent accuracy honestly:
  the attributor is whole-file, so `:line` is not validated in v0.x.
- `redact_value()` — promoted from the private ingest walker and now redacts JSON
  **object keys** as well as values, closing a path where a secret used as a key could
  survive scrubbing and ingest.
- `examples/scrub_session.rs` — reuses `redact_value` to scrub a session JSONL for
  corpus inclusion (best-effort first layer; pair with gitleaks/trufflehog).

**Tests & corpus**

- `tests/attribution_accuracy.rs` — always-run synthetic evaluator test (incl. a
  multi-intent file and a duplicate-label dedup case) + the `#[ignore]`d real-corpus
  gate (`cargo test --test attribution_accuracy -- --ignored`).
- `tests/corpus/attribution/` — `run.sh` (gate runner + `scrub` subcommand),
  `SCRIPT.md` (scripted multi-intent corpus recipe), `LABELING.md` (label format,
  discovery command, worked example); README updated.

## [0.2.0.0] — 2026-06-22

### Added

**vasari-core** (`crates/vasari-core`)

- `vasari ingest claude-code <path>` — parse Claude Code session JSONL into the intent graph:
  human/assistant/system/summary record types; filters /commands, boilerplate, dotdot/absolute paths
- `vasari ingest otel-genai <path>` — parse OTLP JSON (GenAI semconv ≥ 1.30.0) via two-pass
  span tree reconstruction; root spans become UserPrompt/SystemInstruction, child spans become ToolCall
- `IngestAdapter` trait + `IngestEvent` enum — adapter contract for both ingest paths
- `run_pipeline()` — shared parse → redact → synthesize → store → index pipeline; non-fatal
  degradations collected into `IngestSummary.degraded` (never panics)
- `redact()` — secret redaction: regex patterns (sk-*, AKIA*, ghp_/gho_/ghs_/ghr_/ghu_*, Bearer),
  Shannon entropy heuristic (>4.5 bits/char), preserves 40-hex git SHAs and 36-char UUIDs
- `extract_constraints()` — rule-based constraint extraction from must/shall/always/never/must-not
  keywords with sentence-level splitting and `ConstraintPolarity` (Mandatory | Prohibitive)
- `ConstraintPolarity` enum on `Constraint` nodes — included in content hash (different polarity → different node ID)
- `schema_version` field on all five node types — stored but excluded from content hash
- `Intent::new_at()` constructor for deterministic timestamps during ingest
- `ObjectStore::iter_all()` — walks objects/ for `vasari sessions` / `vasari files`
- `ObjectStore::lookup_attributions()` now deduplicates on read (sort + dedup) to handle re-ingest
- `regex` and `once_cell` added to workspace dependencies
- Integration tests: end-to-end ingest, idempotency, iter_all, fixture-based golden tests
- Test fixtures: `tests/fixtures/claude_code/simple-edit-session.jsonl`, `tests/fixtures/otel/simple-gen-ai.json`
- 70+ unit tests across all new modules

**vasari** (`crates/vasari`)

- `vasari ingest claude-code <path>` / `otel-genai <path>` — subcommand shape (replaces stub)
- `vasari sessions` — list Intent nodes sorted by creation time
- `vasari files` — list files with attribution coverage
- `vasari constrain "<text>" --plan <id> [--polarity mandatory|prohibitive]` — pin a constraint manually
- `vasari fsck` — rebuild index from object store (was wired in v0.1; now has a real backing method)

**Docs / Spec**

- `INTENT-SPEC.md` updated: `schema_version` excluded from hash (with rationale), semconv pin ≥ 1.30.0

### Changed

- `vasari ingest` is now a subcommand (`claude-code` / `otel-genai`) rather than `--adapter` flag
- `Constraint` nodes now include `polarity` field (breaking schema change — no stored nodes to migrate)

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
