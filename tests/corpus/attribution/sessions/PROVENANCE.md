# Corpus provenance

This is a **scripted** corpus (SCRIPT.md "Approach B — machine truth"), not a
capture of real user sessions. Every session is synthetic and authored by
`tests/corpus/attribution/generate.py`, so the ground truth is known by
construction and there is no privacy/IP blast radius (no real prompts, paths,
secrets, or third-party code).

**Real on-disk schema.** Sessions mirror what Claude Code actually writes —
records typed `"user"`, absolute tool `file_path`s under a per-record `cwd`, and
Read results in `cat -n` form — so the gate exercises the real ingest paths (the
earlier `"human"`/relative-path/plain-read corpus silently passed even when those
paths were broken). Each file is Read then wholly rewritten, so attributions
resolve to exact line ranges rather than whole-file degrades.

**Dogfooding on real sessions:** to score the gate against your own scrubbed real
sessions, see the local-corpus workflow in `LABELING.md` (`sessions-local/` +
`labels-local/`, both gitignored).

- **Sessions:** `sessions/sNN.jsonl` — one clear intent each (one prompt + edits).
- **Multi-intent stress case:** `src/shared.rs` is edited by `s01` and `s02`, so
  `why` must contend with two intents on one file.
- **Labels:** `labels/*.jsonl`, keyed by `(file, line) -> expected_intent` (the
  prompt text). Keying on `(file,line)` — not node IDs — means labels survive the
  node-ID churn that schema changes cause.
- **Regenerate:** `python3 tests/corpus/attribution/generate.py`
- **Run the gate:** `bash tests/corpus/attribution/run.sh`

Current size: 10 sessions, 115 labeled `(file,line)` rows (≥ N_MIN = 100). The
gate is enforced in CI. Grow it by adding scripts to `generate.py`.
