# vasari

**Intent attribution for autonomous coding agents.**

Like its namesake — Giorgio Vasari, the Florentine biographer who invented art
attribution by looking at brushwork and asking *who painted this, and why?* —
Vasari looks at a line of code and answers the same question:

```
$ vasari why src/auth.ts:47
Intent: Add JWT verification before the user-id lookup (ACME-411)
  ↳ Plan step 4 of 6
  ↳ Tool calls: read_file, edit_file
  ↳ Confidence: 0.87
  Source: ticket ACME-411 ("auth regression on /me endpoint")
```

## What it is

Vasari is a **content-addressed intent graph** — Git for agent cognition. Intent
objects have content hashes, parents, and merges; an amended plan is `commit
--amend`; an agent handoff is a merge; `vasari why` is `git blame` over cognition.

OTEL/MCP are adapters. The primitive — not the dashboard — is the product.

## Two hero verbs (v0.1)

```
vasari why <file>:<line>         # what intent caused this line to exist?
vasari diff <plan-a> <plan-b>    # where did this agent's plan diverge from spec?
```

## Install

```
cargo install vasari          # CLI binary
pip install vasari            # same binary, via maturin wheel
```

*(v0.1 ships CLI-only. Python library bindings via UniFFI are v0.2.)*

## Ingest

```
vasari ingest --adapter claude-code ~/.claude/projects/your-session/
vasari ingest --adapter otel-genai ./spans.jsonl
```

## Status

Early development. See `INTENT-SPEC.md` for the schema.
See `docs/why-not-just-in-toto.md` for the architecture rationale.

## License

Apache-2.0
