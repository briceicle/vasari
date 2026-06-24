#!/usr/bin/env python3
"""Deterministic scripted-corpus generator (SCRIPT.md "Approach B — machine truth").

Emits Claude Code session JSONL + (file,line)->intent labels under
tests/corpus/attribution/{sessions,labels}/. Each session is one clear intent
(one prompt + edits), so the ground truth is known by construction. Two sessions
deliberately edit src/shared.rs to exercise the multi-intent-file case.

Re-run after changing the scripts:
    python3 tests/corpus/attribution/generate.py

Labels are keyed by (file,line)->expected_intent (the prompt text), so they
survive node-ID churn across schema versions. Run the gate with:
    bash tests/corpus/attribution/run.sh
"""
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
SESSIONS = HERE / "sessions"
LABELS = HERE / "labels"
LINES_PER_FILE = 6  # labeled lines per edited file (whole-file attribution → any line)

# (session id, prompt, [edited files]). src/shared.rs is touched by s01 and s02.
SCRIPTS = [
    ("s01", "add rate limit middleware to the api", ["src/api.rs", "src/shared.rs"]),
    ("s02", "fix the logging format in the logger", ["src/logger.rs", "src/shared.rs"]),
    ("s03", "add JWT verification to the auth module", ["src/auth.rs", "src/token.rs"]),
    ("s04", "implement pagination for the users endpoint", ["src/users.rs", "src/page.rs"]),
    ("s05", "add retry with backoff to the http client", ["src/http.rs", "src/retry.rs"]),
    ("s06", "cache database query results in memory", ["src/db.rs", "src/cache.rs"]),
    ("s07", "validate request payloads with a schema", ["src/schema.rs", "src/validate.rs"]),
    ("s08", "add structured error types to the parser", ["src/parser.rs", "src/errors.rs"]),
    ("s09", "write integration tests for the api", ["src/api_test.rs", "src/fixtures.rs"]),
    ("s10", "add a config loader reading from env", ["src/config.rs", "src/env.rs"]),
]

SHARED = "src/shared.rs"


def session_records(sid, prompt, files):
    """A minimal but well-formed Claude Code session: prompt + Edit per file,
    each with its tool_result in the following human record (exercises E4)."""
    recs = [{
        "type": "human", "timestamp": f"2024-02-01T10:00:00Z",
        "uuid": f"{sid}-u0", "parentUuid": None,
        "message": {"role": "user", "content": prompt},
    }]
    for i, f in enumerate(files):
        tid = f"{sid}-t{i}"
        recs.append({
            "type": "assistant", "timestamp": "2024-02-01T10:00:01Z",
            "uuid": f"{sid}-a{i}", "parentUuid": f"{sid}-u0",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": tid, "name": "Edit",
                 "input": {"file_path": f, "old_string": "// old", "new_string": "// new"}},
            ]},
        })
        recs.append({
            "type": "human", "timestamp": "2024-02-01T10:00:02Z",
            "uuid": f"{sid}-r{i}", "parentUuid": f"{sid}-a{i}",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tid,
                 "content": [{"type": "text", "text": f"The file {f} has been updated successfully."}]},
            ]},
        })
    return recs


def write_jsonl(path, rows):
    path.write_text("".join(json.dumps(r) + "\n" for r in rows))


def main():
    SESSIONS.mkdir(parents=True, exist_ok=True)
    LABELS.mkdir(parents=True, exist_ok=True)
    for old in list(SESSIONS.glob("*.jsonl")) + list(LABELS.glob("*.jsonl")):
        old.unlink()

    prompt_for_shared = []
    for sid, prompt, files in SCRIPTS:
        write_jsonl(SESSIONS / f"{sid}.jsonl", session_records(sid, prompt, files))
        # Label every single-intent file this session edited (skip the shared one).
        rows = []
        for f in files:
            if f == SHARED:
                prompt_for_shared.append(prompt)
                continue
            for line in range(1, LINES_PER_FILE + 1):
                rows.append({"file": f, "line": line, "expected_intent": [prompt],
                             "confidence_threshold": 0.5})
        write_jsonl(LABELS / f"{sid}.jsonl", rows)

    # Multi-intent file: both contributing prompts.
    shared_rows = [{"file": SHARED, "line": line, "expected_intent": prompt_for_shared,
                    "confidence_threshold": 0.5,
                    "notes": "multi-intent: edited by s01 and s02"}
                   for line in range(1, LINES_PER_FILE + 1)]
    write_jsonl(LABELS / "shared.jsonl", shared_rows)

    # A file no intent touched → why should return nothing.
    write_jsonl(LABELS / "untouched.jsonl",
                [{"file": "src/untouched.rs", "line": 1, "expected_intent": []}])

    n = sum(1 for _ in LABELS.glob("*.jsonl") for _ in open(_))
    print(f"wrote {len(SCRIPTS)} sessions, {n} labeled (file,line) rows")


if __name__ == "__main__":
    main()
