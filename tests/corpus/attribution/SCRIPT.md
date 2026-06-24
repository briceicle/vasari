# Corpus generation recipe (Approach B — scripted multi-intent, machine truth)

The accuracy corpus is **machine-labeled**: we drive an agent through scripted tasks
where we know which intent produced which file, so the labels are ground truth by
construction. This is the only way to exercise the case where `vasari why` actually
breaks — two intents touching one file — which a single real session cannot do
(`run_pipeline` mints exactly one Intent per Claude Code session).

## What you produce

```
tests/corpus/attribution/
  sessions/   <session-id>.jsonl   one scripted task per file (scrubbed)
  labels/     <session-id>.jsonl   one label per (file, line) — see LABELING.md
```

## Recipe

1. **Pick a small PUBLIC throwaway repo.** Using a public repo (not your private
   work) removes the IP/privacy blast radius entirely — see §Privacy.

2. **Run K distinct scripted tasks, each its own Claude Code session**, each a single
   clear intent (one prompt → some edits). Capture each session's JSONL from
   `~/.claude/projects/<project>/<id>.jsonl`. Record the prompt text — it becomes the
   intent label.

3. **Deliberately make ≥2 tasks edit the SAME file.** That is the multi-intent-file
   stress case (cross-session accumulation), where `why`'s single top pick is decided
   by a hash tie-break on equal-confidence whole-file attributions. The evaluator
   reports single-intent vs multi-intent accuracy separately so this collapse is
   visible, not hidden. Keep several single-intent files as the clean baseline.

4. **Scrub each session** before committing:
   ```
   bash tests/corpus/attribution/run.sh scrub sessions-raw/<id>.jsonl > sessions/<id>.jsonl
   ```
   Then run an independent secret scanner and eyeball it (see §Privacy).

5. **Write labels** (`labels/<id>.jsonl`) per LABELING.md. For a machine corpus the
   labels come straight from step 2: each edited file's `expected_intent` is the
   session prompt(s) that touched it. Multi-intent files list every contributor.

6. **Grow toward n ≥ 100.** Below 100 labeled lines the gate reports the number but is
   not eligible to PASS/FAIL (a 10-line pilot can't clear the 70% Wilson floor). Until
   then, `run.sh` prints `INSUFFICIENT_CORPUS`.

7. **Run the gate:** `bash tests/corpus/attribution/run.sh`

## Privacy (read before committing any session)

- Scrubbing reuses the ingest redactor (`vasari_core::redact_value`) — **best-effort**:
  it misses short tokens and prose-shaped secrets (see `TODOS.md`). It is NOT the gate.
- Run `gitleaks detect --no-git --source sessions/` (or trufflehog) as a second layer.
- Scrubbing secrets ≠ clearing for public redistribution. A real work session can
  carry third-party/employer IP, internal paths, ticket IDs, names — none of which are
  "secrets." Prefer a session run against a **public** repo so there is nothing to clear.
- Treat a committed session as **permanent and public** (git history is forever). Record
  provenance in `sessions/PROVENANCE.md` without leaking private content.
