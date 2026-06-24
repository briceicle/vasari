//! Scrub a Claude Code session JSONL for corpus inclusion.
//!
//! Reuses the same redaction walker as ingest (`vasari_core::redact_value`),
//! redacting every string leaf (keys and values) of every record. Reads a
//! `.jsonl` file (or stdin with `-`) and prints to stdout.
//!
//! This is a BEST-EFFORT first layer only. The underlying redactor is heuristic
//! (prefix regexes + an entropy threshold) and WILL miss prefix-less, low-entropy,
//! or prose-shaped secrets. It is NOT a sufficient privacy gate on its own: always
//! follow it with an independent secret scanner (gitleaks/trufflehog) and a manual
//! eyeball before committing, and confirm the source content is yours to publish
//! (see SCRIPT.md §Privacy).
//!
//! Usage:
//!   cargo run -p vasari-core --example scrub_session -- path/to/session.jsonl > scrubbed.jsonl
//!   cat session.jsonl | cargo run -p vasari-core --example scrub_session -- -

use std::io::{self, BufRead, Write};

use vasari_core::redact_value;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: scrub_session <session.jsonl | ->");
        std::process::exit(2);
    });

    let reader: Box<dyn BufRead> = if arg == "-" {
        Box::new(io::BufReader::new(io::stdin()))
    } else {
        match std::fs::File::open(&arg) {
            Ok(f) => Box::new(io::BufReader::new(f)),
            Err(e) => {
                eprintln!("scrub_session: cannot open {arg}: {e}");
                std::process::exit(1);
            }
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut line_no = 0u64;
    for line in reader.lines() {
        line_no += 1;
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("scrub_session: read error on line {line_no}: {e}");
                std::process::exit(1);
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => {
                let scrubbed = redact_value(value);
                // Compact one-record-per-line, matching the JSONL input shape.
                let _ = writeln!(out, "{scrubbed}");
            }
            Err(e) => {
                // Do NOT pass through an unparsable line (it might carry a raw secret).
                eprintln!("scrub_session: skipping unparsable line {line_no}: {e}");
            }
        }
    }
}
