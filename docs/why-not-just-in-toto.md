# Why Not Just in-toto?

*The Assignment: which parts of the Vasari intent graph map cleanly onto
in-toto v1's `Statement / Predicate / Subject` model, and which parts don't?*

---

## What in-toto v1 gives you

in-toto's core primitive is a signed attestation:

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [{ "name": "pkg:pypi/vasari@0.1.0", "digest": { "sha256": "abc…" } }],
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": { … }
}
```

- **Statement**: the signed envelope (via DSSE + Sigstore keyless).
- **Subject**: the artifact being attested — a build output identified by digest.
- **Predicate**: a typed, arbitrary JSON blob describing a claim about that artifact.

SLSA Provenance, the canonical predicate, answers: *"How was this binary produced?"*
The model works because the subject is a discrete artifact (a binary, a container image,
a package) and the predicate is a single, bounded claim about its provenance.

---

## What maps cleanly (~60%)

### 1. Signing envelope — use as-is

Every Vasari node fits inside an in-toto Statement wrapped in DSSE and signed via Sigstore.
`vasari verify` is exactly this. Each node type becomes a custom predicateType:

```
https://vasari.sh/Intent/v0.1
https://vasari.sh/Plan/v0.1
https://vasari.sh/Constraint/v0.1
https://vasari.sh/Action/v0.1
https://vasari.sh/Attribution/v0.1
```

The Rekor transparency log can record each signed node, giving tamper-evidence
for free. This is the in-toto success case for Vasari.

### 2. Content-addressing — compatible, not identical

in-toto subjects carry a `digest` map (`sha256`, `sha512`, `gitBlob`, etc.).
Vasari's `NodeId = sha256(JCS(hash_input))` is the same idea: stable identity
derived from content. The difference: in-toto's digest is over the artifact
bytes; Vasari's is over the node's canonical JSON. Both are content-addressed;
the hash function input differs.

`Attribution` nodes can use the file-path + git blob SHA as the `subject`,
which maps cleanly:

```json
"subject": [{ "name": "src/auth.ts", "digest": { "gitBlob": "<blob-sha>" } }]
```

This is probably the cleanest mapping in the whole model.

### 3. Custom predicates — exactly right for node content

Each Vasari node type maps onto a Predicate with no friction. The `Intent`
predicate carries `source`, `text`, `created_at`, and `parent_ids`; the
`Attribution` predicate carries `action_id`, `target`, `confidence`, and
`evidence[]`. There is no structural clash. You could implement all five node
types as in-toto predicates today.

---

## What doesn't map (~40%)

### 1. Intent has no artifact subject

in-toto's fundamental premise: "here is a signed claim *about an artifact*."
`Intent` is a root node with no upstream artifact. The ticket text, the user
prompt, or the conversational context is the source — there is no build output
being attested. You can put the Intent's own hash in `subject.digest`, but that
makes the Statement an attestation about itself, which breaks the model
semantically. in-toto has no concept of an attestation that is its own subject.

Workaround considered: use `subject = [{ "name": "vasari:session/<id>" }]` as
a synthetic artifact. This works mechanically but destroys the semantics: you
are asserting "here is a claim about a Vasari session" when you mean "here is
a node that *is* the intent context." The confusion compounds at query time.

### 2. DAG traversal is outside the spec

`vasari why src/auth.ts:47` walks Attribution → Action → Plan (at step_index)
→ Intent. This requires:
- Locating all Attribution nodes whose `target` contains line 47 of `src/auth.ts`
- Following `action_id` to the Action node
- Following `plan_ref.plan_id + step_index` to the Plan node
- Following `intent_ids` to the root Intent

in-toto has no query language. Each Statement is a standalone signed blob.
The spec gives you no way to traverse parent references across Statements.
You have to ingest all Statements into your own graph store (which is what
Vasari's object store is) and implement the traversal yourself. So you're
using in-toto as a signing envelope *on top of* your graph store — not as
the graph store itself. This is the right layering, but it means in-toto
cannot replace the data model.

### 3. Confidence scores have no analog

`Attribution.confidence: 0.0–1.0` is a first-class field in Vasari's model —
it's what lets `vasari why` report the weakest link in the attribution chain.
There is nothing like this in any in-toto predicate type. You put it in the
predicate blob and it works, but it's invisible to any in-toto tooling. No
policy engine, no Rekor query, no SLSA verifier knows what to do with it.

This isn't a fatal gap — confidence is an application-level concern — but it
means the in-toto ecosystem cannot reason about Vasari's attribution quality
without Vasari-specific extensions.

### 4. `vasari diff` is a cross-Statement operation

Diffing two Plan DAGs requires comparing the ordered step sequences of two
separate graphs, aligning goals by string similarity, and reporting the first
divergence point. in-toto has no concept of comparing two predicate instances
of the same type. SLSA Provenance has no `diff` verb. This operation is entirely
outside the spec.

### 5. Line-level attribution vs artifact-level

SLSA provenance proves "this binary came from this source commit." Vasari proves
"this *line* came from this intent." The subject granularity is fundamentally
different. A git blob SHA gets you to file-level; you still need the line range
in the predicate, which no in-toto tooling understands. git-notes is a better
fit for line anchoring (it's what `git blame --notes=vasari` uses), but git-notes
is also not in the in-toto model.

---

## Verdict

| Layer | Use in-toto? | Notes |
|-------|-------------|-------|
| Signing (`vasari verify`) | Yes — exactly | DSSE + Sigstore keyless, Rekor log |
| Subject (Attribution → file) | Yes — fits cleanly | `gitBlob` digest in subject |
| Node content (all 5 types) | Yes — as custom predicates | No structural friction |
| Graph store (object store) | No — build your own | in-toto has no traversal semantics |
| DAG traversal (`why`, `diff`) | No — outside the spec | Query layer is Vasari's job |
| Confidence scores | Partial — in predicate blob | Invisible to in-toto tooling |
| Intent root node | Partial — synthetic subject | Subject-as-self breaks semantics |

**Answer to the HN question ("why not just in-toto?"):**

in-toto is a signing framework for build artifacts. Vasari is a graph database
for engineering intent. They operate at different layers. The correct relationship
is: Vasari uses in-toto as its signing mechanism (the `vasari verify` path),
the same way a Git commit uses SHA-1 as its identity mechanism — but SHA-1 is
not Git, and in-toto is not Vasari.

The 40% that doesn't map (DAG traversal, confidence aggregation, the `diff` verb,
the intent-as-root problem) is exactly the 40% that makes Vasari a new primitive
rather than a configuration of an existing one. If it all mapped, there would be
nothing to build.

---

*Written as The Assignment prerequisite. Committed to `docs/why-not-just-in-toto.md`
before any Rust. See INTENT-SPEC.md for the full schema; see `crates/vasari-core/`
for the implementation.*
