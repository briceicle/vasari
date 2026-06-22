# INTENT-SPEC.md — Vasari Intent Attribution Specification

*Status: stub. The spec fills in as the two ingest adapters (claude-code,
otel-genai) are built. Per the design doc: "write INTENT-SPEC.md while building
the second adapter, not before — the spec records what the two adapters forced
into common shape."*

---

## §1 Overview

Vasari defines a content-addressed intent graph for recording and querying the
relationship between engineering intent (tickets, prompts, plans) and code
(files, line ranges, commits produced by autonomous coding agents).

The five node types are: `Intent`, `Plan`, `Constraint`, `Action`, `Attribution`.

## §2 Node Types

See `crates/vasari-core/src/schema/` for the canonical Rust definitions.

| Node          | Required fields                                                                 | Edges                                    |
| ------------- | ------------------------------------------------------------------------------- | ---------------------------------------- |
| `Intent`      | `id`, `source`, `text`, `created_at`                                            | `parent_ids` → prior Intent (amendment)  |
| `Plan`        | `id`, `intent_ids[]`, `steps[]`                                                 | `intent_ids` → Intent                    |
| `Constraint`  | `id`, `text`, `derived_from`                                                    | `derived_from` → Intent or Plan          |
| `Action`      | `id`, `tool`, `args`, `timestamp`, `plan_ref`                                   | `plan_ref.plan_id` → Plan step           |
| `Attribution` | `id`, `action_id`, `target`, `confidence`, `evidence[]`                         | `action_id` → Action                     |

## §3 Canonicalization

Node IDs: `sha256(JCS(hash_input))` where JCS is RFC 8785 JSON Canonicalization
Scheme (key-sorted, no whitespace, specific number and unicode handling).

Fields excluded from the content hash (annotations, not identity):
- `Action.result_summary`
- `Attribution.confidence`

Rationale: these fields may be updated without changing what the node *is*
(e.g., confidence is recalibrated, summaries are recomputed). Excluding them
prevents churn and ensures the same logical action or attribution gets the same
ID across recalibration runs.

## §4 Storage

```
<repo>/.vasari/
  objects/<sha[0..2]>/<sha[2..]>   canonical JSON, gzip-compressed
  index/targets/<encoded-path>/<start>-<end>   attribution lookup index
  refs/                             human-readable refs
  HEAD                              current intent context
```

The `index/` directory is derivable from the object store. `vasari fsck`
rebuilds it. Applications must not assume the index is authoritative; the
object store is authoritative for `vasari why`.

## §5 Attribution

v0.1 uses rule-based attribution:
- `Action.tool ∈ {edit_file, write_file, str_replace}` → parse `args.path`
  and line range → emit one `Attribution` per affected line range.
- `confidence = 1.0` for exact range match
- `confidence = 0.7` for heuristic/fuzzed match
- `confidence = 0.3` if a later cleanup edit may have overwritten it

**v0.1 ship gate:** `vasari why` must return the correct top-level `Intent`
for ≥80% of lines on a hand-labeled 100-line corpus (95% CI lower bound ≥70%).
Corpus: `tests/corpus/attribution/`. Labeling protocol: `tests/corpus/attribution/README.md`.

## §6 OTEL GenAI Adapter Inference Rules

OTEL GenAI spans don't carry `Plan` or `Constraint` natively. Synthesis rules:

- `Intent` ← top-level `gen_ai.operation.name` span's input prompt or linked `code.task.description`
- `Plan` ← ordered child agent spans within an intent; each agent step = one `Plan.steps[]` entry
- `Constraint` ← span attributes matching `vasari.constraint.*` prefix, plus `gen_ai.system_instructions` clauses
- `Action` ← each `mcp.tool.call` / `gen_ai.tool.call` span

Anything not synthesizable lands in `evidence[]` with `kind: inferred`.

Pinned semconv version: *TBD at v0.1 cut (target: ≥ 1.30.0 stable)*.

## §7 Signing (opt-in)

`vasari verify` wraps each node in an in-toto v1 Statement with the node's
canonical JSON as the predicate, signed via Sigstore keyless DSSE. See
`docs/why-not-just-in-toto.md` for the rationale.

Sign-by-default is deferred to v0.2.

---

*This spec is updated with each adapter PR. The definitive schema is the Rust
source in `crates/vasari-core/src/schema/`.*
