# TODOS

Known deferred items as of v0.1.0.0.

## v0.2 targets

- [ ] **`vasari ingest --adapter claude-code`** — parse Claude Code session JSONL
  (`~/.claude/projects/<session>/`) into Intent → Plan → Action → Attribution nodes.
  Inference rules: §6 of INTENT-SPEC.md.

- [ ] **`vasari ingest --adapter otel-genai`** — parse OTEL GenAI spans (≥ semconv 1.30.0)
  into the intent graph. Same §6 rules.

- [ ] **`vasari verify`** — Sigstore keyless DSSE signing for each node; in-toto v1 Statement
  wrapper. See `docs/why-not-just-in-toto.md`.

- [ ] **Full RFC 8785 JCS** — add IEEE 754 number serialization and unicode escape normalization
  to `crates/vasari-core/src/hash.rs`. Current implementation sorts keys only.

- [ ] **Python wheel** — maturin build + UniFFI bindings so `pip install vasari` works.

- [ ] **Attribution corpus** — hand-label 100 lines from a real Claude Code session,
  run `tests/corpus/attribution/run.sh`, confirm ≥80% accuracy ship gate passes.

## CI/CD (deferred from v0.1)

- [ ] **GitHub Actions release workflow** — on tag `v*`, run `cargo test`, `cargo clippy`,
  `cargo build --release`, publish crate to crates.io, create GitHub release with binary.
  Deferred because cargo/rustup is not available in the current CI environment and the
  release workflow needs to be authored once it is.

## Known bugs (pre-existing, not blocking v0.1.x)

- **`ObjectStore::object_path` panics on NodeIDs shorter than 2 chars** — the index
  scanner reads raw lines from disk into `NodeId` without length validation; `&s[..2]`
  panics on short or empty strings. Fix: validate in `object_path`, return
  `VasariError::InvalidNodeId`. Introduced in v0.1.0.0.

- **`encode_path` encodes `.` the same as `..`** — the guard
  `".." | "." => "%2E%2E"` maps both dot forms to the same string. A path component
  `.` should encode as `%2E`. Causes false-positive deduplication of `.` and `..` in
  index paths. Introduced in v0.1.0.0.

## Known limitations (not bugs)

- `get()` does not verify content hash on read — silent corruption is possible.
  `vasari fsck` is the recovery path. Hash verification on `get()` deferred to v0.2.

- `NodeId` inner String is public — callers can construct invalid IDs.
  Validation at ingest boundary deferred until adapters ship.
