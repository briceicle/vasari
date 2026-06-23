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

- [ ] **Attribution corpus** — hand-label 100 lines from a real Claude Code session,
  run `tests/corpus/attribution/run.sh`, confirm ≥80% accuracy ship gate passes.

- [ ] **MCP ingest adapter** — parse MCP tool-call streams into the intent graph (v0.2+ per plan).

## CI/CD (deferred from v0.1)

- [ ] **GitHub Actions workflow** — `cargo test`, `cargo clippy`, `cargo fmt --check` on every
  push. Separate release workflow: on tag `v*`, build binaries for `aarch64-darwin`,
  `x86_64-linux`, `aarch64-linux`, publish to crates.io, create GitHub release.
  Note: no Rust toolchain in the current dev environment — author the workflow file
  and test it via GitHub Actions directly.

## Known limitations (not bugs)

- `get()` does not verify content hash on read — silent corruption is possible.
  `vasari fsck` is the recovery path. Hash verification on `get()` deferred to v0.3.

- `NodeId` inner String is public — callers can construct invalid IDs.
  Validation strengthened at ingest boundary; full newtype enforcement deferred to v0.3.

- Shannon entropy redaction heuristic may false-positive on some legitimate high-entropy tokens
  (e.g., base64-encoded binary data in prompts). Tuning deferred until we have real session corpus data.

- Claude Code JSONL format is undocumented and may change. The adapter is snapshot-tested
  against `tests/fixtures/claude_code/`; a format change will fail the integration tests loudly.
