# vasari

**Intent attribution for autonomous coding agents.**

Like its namesake — Giorgio Vasari, the Florentine biographer who invented art
attribution by looking at brushwork and asking *who painted this, and why?* —
Vasari looks at a line of code and answers the same question:

```
$ vasari ingest claude-code ~/.claude/projects/<project>/<session>.jsonl
$ vasari why crates/vasari-core/src/adapters/otel.rs:8
Do a) and b) in parallel
  from  claude-code:cca4eab7… · 2026-06-25 01:04 UTC
  via   Edit · step 44/74 · Now update otel.rs (the other `ToolCall` construction site) to add `rationale: None`:
  conf  1.00
```

That is real output, attributing a line from Vasari's own development. The
answer reads: the line exists because of the prompt *"Do a) and b) in parallel"*,
written by the Edit at plan step 44/74, whose stated reason was *"update otel.rs
… to add `rationale: None`"*. `conf 1.00` means the exact line range was located
against the file the agent had Read; it drops to `0.70` when an edit can't be
located and Vasari falls back to whole-file attribution. Attribution is
file→intent today — see [Status](#status).

## What it is

Vasari is a **content-addressed intent graph** — Git for agent cognition. Intent
objects have content hashes, parents, and merges; an amended plan is `commit
--amend`; an agent handoff is a merge; `vasari why` is `git blame` over cognition.

OTEL/MCP are adapters. The primitive — not the dashboard — is the product.

## Two hero verbs

```
vasari why <file>:<line>         # what intent caused this line to exist?
vasari diff <plan-a> <plan-b>    # where did this agent's plan diverge from spec?
```

`vasari plans` lists plans with the short IDs you pass to `vasari diff`.

## Install

Build from source — a Rust toolchain is the only requirement:

```
git clone https://github.com/briceicle/vasari
cd vasari
cargo install --path crates/vasari    # installs the `vasari` binary
```

Pre-built binaries (crates.io) and a `pip install vasari` wheel are planned, not
yet shipped — see the "Release workflow" and "Python wheel" items in `TODOS.md`.

## Ingest

`vasari ingest <adapter> <file>` reads one session into the graph. Adapters take
a file path, not a directory:

```
vasari ingest claude-code ~/.claude/projects/<project>/<session>.jsonl
vasari ingest otel-genai ./spans.jsonl
```

## Status

Early development (v0.2.x). See `INTENT-SPEC.md` for the schema and
`docs/why-not-just-in-toto.md` for the architecture rationale.

Known limits today, so nothing here oversells:

- **Attribution is whole-file → intent.** A `:line` query resolves to an exact
  line range only when the agent Read the file before editing it (confidence
  `1.00`); otherwise it degrades honestly to a whole-file attribution
  (confidence `0.70`). On real sessions, exact ranges cover roughly half to
  three-quarters of attributed lines.
- **CLI only.** No Python bindings or pre-built packages yet (see Install).
- **Accuracy gate is synthetic.** The committed accuracy corpus mirrors the real
  Claude Code on-disk schema but is machine-generated; its score proves the
  pipeline wires up, not real-world attribution quality.

## License

Apache-2.0
