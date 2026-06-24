#!/usr/bin/env bash
# Attribution accuracy gate — human-facing wrapper.
#
#   bash tests/corpus/attribution/run.sh           # run the gate, print the report
#   bash tests/corpus/attribution/run.sh scrub IN  # scrub a session JSONL to stdout
#
# The gate itself is the cargo test `attribution_accuracy_gate` (it is #[ignore]'d
# so the default `cargo test` stays fast and corpus-independent). The full per-line
# table is also written to target/attribution-report.txt on every run.
set -euo pipefail

# Repo root = two levels up from this script (tests/corpus/attribution/).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

cmd="${1:-gate}"
case "$cmd" in
  scrub)
    in="${2:?usage: run.sh scrub <session.jsonl>}"
    cargo run -q -p vasari-core --example scrub_session -- "$in"
    ;;
  gate|"")
    echo "Running attribution accuracy gate (this needs a committed corpus)…"
    # --nocapture so the per-line table reaches the terminal even on PASS.
    cargo test -p vasari-core --test attribution_accuracy -- --ignored --nocapture
    echo
    echo "Full report: $ROOT/target/attribution-report.txt"
    ;;
  *)
    echo "unknown command: $cmd (use: gate | scrub <file>)" >&2
    exit 2
    ;;
esac
