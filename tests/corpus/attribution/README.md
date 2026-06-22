# Attribution Test Corpus

Hand-labeled lines for validating `vasari why` attribution accuracy.

## Ship gate

v0.1 ships only if `vasari why` returns the correct top-level `Intent` for
**≥80%** of labeled lines at confidence ≥0.5 (95% CI lower bound ≥70%).

## Calibration pilot

Before scaling labeling to 100 lines:
1. Label 10 lines from a real Claude Code session.
2. Run the rule-based attributor against them.
3. If accuracy is plausibly ≥60%, proceed to full corpus. Otherwise revise rules or the bar.

## Corpus structure

```
tests/corpus/attribution/
  sessions/                 Scrubbed Claude Code session JSONL files
  labels/                   Hand-labeled ground truth
    <session-id>.jsonl      One entry per labeled line:
                            {"file": "path", "line": N, "intent_text": "...", "confidence_threshold": 0.5}
  run.sh                    Evaluates vasari why against labels, prints accuracy
```

## Privacy

Session files must be scrubbed of secrets before committing:
- No API keys, tokens, or passwords in `args` fields
- No PII in prompt or intent text
- `args` containing sensitive values replaced with `{"_redacted": true}`

See INTENT-SPEC.md §5 and the redaction rules in `crates/vasari-core/src/store/`.
