# Labeling protocol

A label is ground truth for one `(file, line)`: which Intent(s) should `vasari why`
return there. For the machine corpus (see SCRIPT.md) the answer is known from the
generation script. For the small human holdout, follow the discovery step below so
you copy the value instead of inventing it.

## Format

`labels/<session-id>.jsonl`, one JSON object per line:

```json
{"file":"src/foo.rs","line":42,"expected_intent":["<intent text>"],"confidence_threshold":0.5,"notes":"optional"}
```

- `expected_intent` is a **list**. Single-intent file → one entry. Multi-intent file →
  every contributing intent. **Empty list `[]` means "should return nothing"** (a line
  no intent touched).
- `confidence_threshold` (default `0.5`): an answer below this confidence counts as
  "no answer," not a wrong answer.
- The evaluator matches by **token overlap** (≥60% of the expected tokens present in
  the returned intent text), so use the intent's real prompt text, not a one-word tag.

## Discovery — copy the value, don't invent it

```
# Ingest the corpus session into a scratch store, then read what `why` returns:
vasari ingest claude-code tests/corpus/attribution/sessions/<id>.jsonl
vasari why <file>:<line> --json | jq '{intent_source, intent_text, confidence}'
```

Paste `intent_text` into `expected_intent`. If the line should resolve to a *different*
intent than what `why` currently returns, paste that intent's real prompt text instead
(that is exactly the kind of miss the gate exists to catch).

## Worked example

Session `abc123` ran the prompt *"add rate limit middleware to the api"* and edited
`src/api.rs`. Discovery:

```
$ vasari why src/api.rs:12 --json | jq '{intent_text, confidence}'
{ "intent_text": "add rate limit middleware to the api", "confidence": 0.7 }
```

Label (correct attribution):
```json
{"file":"src/api.rs","line":12,"expected_intent":["add rate limit middleware to the api"],"confidence_threshold":0.5}
```

A line that no intent should explain (e.g. a pre-existing file the agent never edited):
```json
{"file":"src/legacy.rs","line":3,"expected_intent":[],"confidence_threshold":0.5,"notes":"untouched by the session"}
```

## Pilot scale-up rule (numeric, not "plausible")

Before labeling all 100 lines, label 10 and run `bash tests/corpus/attribution/run.sh`.
Proceed to the full corpus **only if** pilot accuracy ≥ 6/10 **and** no single error
class dominates the failing-lines list. Otherwise file an issue tagged
`attribution-rules` before labeling more — the rules or the bar need work first.
