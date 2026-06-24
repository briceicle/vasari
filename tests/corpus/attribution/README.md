# Attribution Test Corpus

Hand-labeled lines for validating `vasari why` attribution accuracy.

## The gate

`vasari why` must return the correct top-level `Intent` for **≥80%** of labeled lines
at confidence ≥0.5, with the **95% Wilson lower bound ≥70%**. This is a go/no-go on the
hero verb (v0.1/v0.2 already shipped), not a release hold. The gate is only evaluated at
**n ≥ 100** labeled lines; below that the evaluator reports the number but returns
`INSUFFICIENT_CORPUS` (a 10-line pilot cannot clear the 70% Wilson floor).

It measures **file→intent attribution accuracy** — the current attributor is whole-file,
so the `:line` is decorative in v0.1. The evaluator reports **single-intent vs
multi-intent file accuracy separately** (so a blended number can't hide a multi-intent
collapse) and **recall** (`why_all` contains the right intent) alongside **precision**
(`why`'s single top pick is right).

## Running the gate

```
bash tests/corpus/attribution/run.sh
# or directly:
cargo test -p vasari-core --test attribution_accuracy -- --ignored --nocapture
```

The full per-line table is written to `target/attribution-report.txt` on every run.
A tiny synthetic corpus is exercised by the always-run test `evaluator_scores_synthetic_corpus`
so the evaluator can't bit-rot while the real gate is `#[ignore]`d.

## Corpus structure

```
tests/corpus/attribution/
  sessions/                 Scrubbed Claude Code session JSONL files
  labels/                   Ground-truth labels (machine-generated; see SCRIPT.md)
    <session-id>.jsonl      One entry per labeled line:
                            {"file":"path","line":N,"expected_intent":["intent text"],"confidence_threshold":0.5}
  SCRIPT.md                 How to generate the scripted multi-intent corpus
  LABELING.md               Label format, discovery command, worked example
  run.sh                    Runs the gate; `run.sh scrub <file>` scrubs a session
```

`expected_intent` is a **list** (multi-intent files name every contributor; `[]` means
"should return nothing"). Full format + worked example: see `LABELING.md`.

## Privacy

Session files must be scrubbed before committing (`bash run.sh scrub <raw.jsonl>`):
- No API keys, tokens, or passwords in `args` fields; no PII in prompt/intent text.
- Scrubbing reuses the ingest redactor (`crates/vasari-core/src/redact.rs`) — best-effort.
  Follow it with an independent secret scanner (gitleaks/trufflehog) and a manual review.
- Prefer a session run against a **public** repo so there is nothing private to clear.

See `SCRIPT.md` §Privacy and INTENT-SPEC.md §5.
