# TODOS

Known deferred items as of v0.2.0.0.

## v0.2 targets — COMPLETED in this release

- [x] **`vasari ingest claude-code`** — Claude Code session JSONL parsing, implemented in
  `crates/vasari-core/src/adapters/claude_code.rs`

- [x] **`vasari ingest otel-genai`** — OTEL GenAI span parsing (semconv ≥ 1.30.0), two-pass
  span tree reconstruction, implemented in `crates/vasari-core/src/adapters/otel.rs`

- [x] **`vasari sessions`** / **`vasari files`** — list Intent nodes and attributed files

- [x] **`vasari constrain`** — manually pin constraints with polarity (mandatory | prohibitive)

## v0.3 targets

- [ ] **`vasari verify`** — Sigstore keyless DSSE signing for each node; in-toto v1 Statement
  wrapper. See `docs/why-not-just-in-toto.md`.

- [ ] **Full RFC 8785 JCS** — add IEEE 754 number serialization and unicode escape normalization
  to `crates/vasari-core/src/hash.rs`. Current implementation sorts keys only.

- [ ] **Python wheel** — maturin build + UniFFI bindings so `pip install vasari` works.

- [x] **Attribution accuracy gate** — harness + committed corpus shipped (E7). Scripted
  corpus via `tests/corpus/attribution/generate.py` (10 sessions, 115 labeled
  `(file,line)` rows, `src/shared.rs` multi-intent). Gate: n=115, accuracy 100%,
  Wilson95-lower 96.8% — PASS, enforced in CI. Grow the corpus by adding scripts.

- [ ] **MCP ingest adapter** — parse MCP tool-call streams into the intent graph (v0.2+ per plan).

## CI/CD

- [x] **CI workflow** — `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build`,
  `cargo test`, and the attribution accuracy gate run on every push
  (`.github/workflows/ci.yml`, ubuntu + macos).

- [ ] **Release workflow** — on tag `v*`, build binaries for `aarch64-darwin`,
  `x86_64-linux`, `aarch64-linux`, publish to crates.io, create a GitHub release.

## Known bugs (pre-existing, not blocking v0.1.x)

- **`encode_path` encodes `.` the same as `..`** — the guard
  `".." | "." => "%2E%2E"` maps both dot forms to the same string. A path component
  `.` should encode as `%2E`. Causes false-positive deduplication of `.` and `..` in
  index paths. Introduced in v0.1.0.0.

## Known limitations (not bugs)

- `get()` does not verify content hash on read — silent corruption is possible.
  `vasari fsck` is the recovery path. Hash verification on `get()` deferred to v0.3.

- `NodeId` inner String is public — callers can construct invalid IDs.
  `object_path()` now validates hex format (v0.2 fix); full newtype enforcement deferred to v0.3.

- Shannon entropy redaction heuristic may false-positive on some legitimate high-entropy tokens
  (e.g., base64-encoded binary data in prompts). Tuning deferred until we have real session corpus data.

- Claude Code JSONL format is undocumented and may change. The adapter is snapshot-tested
  against `tests/fixtures/claude_code/`; a format change will fail the integration tests loudly.

- OTEL adapter span walk is one level deep — root → child only. Grandchild spans (root → child →
  grandchild) are silently dropped; they are neither children of a root nor orphans (their parent
  exists in `by_span_id`). In practice, gen_ai traces are shallow, but deep tool-call trees would
  lose the leaf spans. Fix: recurse into each child's children in `parse_otlp_json`.

- OTEL adapter child walk is O(roots × spans) — for each root span it scans all spans for direct
  children. For large exports this degrades; a `children_of: HashMap<&str, Vec<&Value>>` built in
  Pass 1 would make it O(spans). Acceptable at current scale.

- OTEL adapter reads the entire file into memory (`read_to_string`). A very large OTLP JSON export
  could cause OOM. Streaming JSON parsing deferred until there is evidence of large-file use.

- `parse_target` in `main.rs` (used by `vasari why`) has no unit tests for edge cases: empty
  string, missing colon, line=0, line number overflow, path containing colons. Low risk for a CLI
  but worth covering before adding any programmatic consumers.
