//! Attribution accuracy evaluation.
//!
//! Scores `vasari why` against a labeled corpus and reports the v0.1 go/no-go
//! gate: does `why` return the correct top-level Intent often enough to be the
//! hero verb? See `tests/corpus/attribution/` for the corpus + protocol.
//!
//! What this measures, honestly: **file→intent attribution accuracy**. The current
//! attributor is whole-file (every line of a file resolves to the same intent), so
//! the `:line` in `why <file>:<line>` is decorative under v0.1. The evaluator
//! reports single-intent-file vs multi-intent-file accuracy separately so a blended
//! number can't hide a multi-intent collapse, and reports recall (`why_all` contains
//! the right intent) alongside precision (`why`'s single top pick is right).

use serde::Deserialize;

use std::collections::HashSet;

use crate::error::VasariError;
use crate::resolve::why_all;
use crate::store::ObjectStore;

/// z for a 95% confidence interval.
pub const Z_95: f64 = 1.96;
/// Default token-overlap threshold for matching a returned intent to a label.
pub const MATCH_THRESHOLD: f64 = 0.6;
/// Minimum corpus size before the gate is eligible to PASS/FAIL.
/// Below this the result is reported but the gate is not evaluated — a 10-line
/// pilot cannot clear the Wilson floor (8/10 ≈ 0.49), so gating on it would
/// wedge `why` off permanently.
pub const N_MIN: u64 = 100;
/// Accuracy bar (design doc): ≥80% correct top-level Intent.
pub const ACCURACY_BAR: f64 = 0.80;
/// Wilson 95% lower-bound bar (eng-review T4): ≥70%.
pub const WILSON_BAR: f64 = 0.70;

/// Wilson score interval lower bound for a binomial proportion.
///
/// Plain Wilson form (no continuity correction). Chosen over the normal
/// approximation because the corpus is small-n, where the normal approximation
/// is badly miscalibrated.
pub fn wilson_lower_bound(successes: u64, n: u64, z: f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n_f = n as f64;
    let phat = successes as f64 / n_f;
    let z2 = z * z;
    let denom = 1.0 + z2 / n_f;
    let center = phat + z2 / (2.0 * n_f);
    let margin = z * ((phat * (1.0 - phat) / n_f) + z2 / (4.0 * n_f * n_f)).sqrt();
    ((center - margin) / denom).max(0.0)
}

/// Tokenize for matching: lowercase, split on non-alphanumerics, drop empties.
fn normalize_tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// True if `candidate` covers at least `threshold` of `expected`'s tokens.
///
/// Token-overlap, not substring-subset: a short label like "auth" must not match
/// a long unrelated prompt and inflate accuracy.
pub fn intent_matches(expected: &str, candidate: &str, threshold: f64) -> bool {
    let exp = normalize_tokens(expected);
    if exp.is_empty() {
        return false;
    }
    let cand: std::collections::HashSet<String> = normalize_tokens(candidate).into_iter().collect();
    let hits = exp.iter().filter(|t| cand.contains(*t)).count();
    (hits as f64 / exp.len() as f64) >= threshold
}

/// One ground-truth label. `expected_intent` is a list so multi-intent files can
/// name every contributor; an empty list means "should return nothing."
#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub file: String,
    pub line: u32,
    #[serde(default)]
    pub expected_intent: Vec<String>,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f32,
    #[serde(default)]
    pub notes: Option<String>,
}

fn default_confidence_threshold() -> f32 {
    0.5
}

/// Parse a labels JSONL document (one [`Label`] per non-empty line).
pub fn parse_labels(jsonl: &str) -> Result<Vec<Label>, serde_json::Error> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

/// Per-label scoring outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Top pick matched an expected intent (also a recall hit).
    PrecisionHit,
    /// Top pick wrong, but `why_all` contained a match (recall hit, precision miss).
    RecallOnly,
    /// Answered, nothing matched.
    Wrong,
    /// Expected nothing and got nothing.
    NoAnswerCorrect,
    /// Expected something, got nothing.
    NoAnswerMiss,
}

impl Outcome {
    fn precision_ok(self) -> bool {
        matches!(self, Outcome::PrecisionHit | Outcome::NoAnswerCorrect)
    }
    fn recall_ok(self) -> bool {
        matches!(
            self,
            Outcome::PrecisionHit | Outcome::RecallOnly | Outcome::NoAnswerCorrect
        )
    }
    fn symbol(self) -> &'static str {
        match self {
            Outcome::PrecisionHit | Outcome::NoAnswerCorrect => "✓",
            Outcome::RecallOnly => "~",
            Outcome::Wrong | Outcome::NoAnswerMiss => "✗",
        }
    }
}

/// One scored row in the report.
#[derive(Debug, Clone)]
pub struct Row {
    pub file: String,
    pub line: u32,
    pub expected: Vec<String>,
    pub got: Option<String>,
    pub multi_intent: bool,
    pub outcome: Outcome,
}

/// Whether the gate passed, failed, or could not be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    Pass,
    Fail,
    /// `n < N_MIN` — reported but not gate-eligible.
    InsufficientCorpus,
}

/// Full evaluation result over a corpus.
#[derive(Debug, Clone)]
pub struct EvalReport {
    pub rows: Vec<Row>,
    pub n: u64,
    pub precision_correct: u64,
    pub recall_correct: u64,
    pub single_total: u64,
    pub single_correct: u64,
    pub multi_total: u64,
    pub multi_correct: u64,
    pub match_threshold: f64,
}

impl EvalReport {
    pub fn accuracy(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.precision_correct as f64 / self.n as f64
        }
    }
    pub fn recall(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.recall_correct as f64 / self.n as f64
        }
    }
    pub fn wilson_lower(&self) -> f64 {
        wilson_lower_bound(self.precision_correct, self.n, Z_95)
    }
    pub fn single_accuracy(&self) -> f64 {
        ratio(self.single_correct, self.single_total)
    }
    pub fn multi_accuracy(&self) -> f64 {
        ratio(self.multi_correct, self.multi_total)
    }

    pub fn status(&self) -> GateStatus {
        if self.n < N_MIN {
            GateStatus::InsufficientCorpus
        } else if self.accuracy() >= ACCURACY_BAR && self.wilson_lower() >= WILSON_BAR {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        }
    }

    /// Human-readable report: per-line table + split summary + status.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("Attribution accuracy report\n");
        out.push_str("===========================\n\n");
        out.push_str(&format!(
            "{:<32} {:>5}  {:<5} expected -> got\n",
            "file", "line", "ok"
        ));
        for r in &self.rows {
            let exp = if r.expected.is_empty() {
                "(none)".to_string()
            } else {
                r.expected.join(" | ")
            };
            let got = r.got.as_deref().unwrap_or("(no answer)");
            out.push_str(&format!(
                "{:<32} {:>5}  {:<5} {}{} -> {}\n",
                truncate(&r.file, 32),
                r.line,
                r.outcome.symbol(),
                if r.multi_intent { "[multi] " } else { "" },
                truncate(&exp, 48),
                truncate(got, 48),
            ));
        }
        out.push('\n');
        out.push_str(&format!(
            "n={}  precision(accuracy)={:.1}%  recall={:.1}%  Wilson95-lower={:.1}%  (match_threshold={:.2})\n",
            self.n,
            self.accuracy() * 100.0,
            self.recall() * 100.0,
            self.wilson_lower() * 100.0,
            self.match_threshold,
        ));
        out.push_str(&format!(
            "single-intent files: {}/{} = {:.1}%   multi-intent files: {}/{} = {:.1}%\n",
            self.single_correct,
            self.single_total,
            self.single_accuracy() * 100.0,
            self.multi_correct,
            self.multi_total,
            self.multi_accuracy() * 100.0,
        ));
        let status = match self.status() {
            GateStatus::Pass => format!(
                "PASS (accuracy ≥ {:.0}% and Wilson ≥ {:.0}%)",
                ACCURACY_BAR * 100.0,
                WILSON_BAR * 100.0
            ),
            GateStatus::Fail => format!(
                "FAIL (need accuracy ≥ {:.0}% and Wilson ≥ {:.0}%)",
                ACCURACY_BAR * 100.0,
                WILSON_BAR * 100.0
            ),
            GateStatus::InsufficientCorpus => format!(
                "INSUFFICIENT_CORPUS (n={} < {} — gate not evaluated; grow the corpus)",
                self.n, N_MIN
            ),
        };
        out.push_str(&format!("gate: {status}\n"));
        out
    }

    /// One-line machine-readable summary for a panic/assert message.
    pub fn summary_line(&self) -> String {
        format!(
            "{}/{} correct ({:.1}%), Wilson95-lower={:.1}% (need ≥{:.0}%), \
             single={:.1}% multi={:.1}%",
            self.precision_correct,
            self.n,
            self.accuracy() * 100.0,
            self.wilson_lower() * 100.0,
            WILSON_BAR * 100.0,
            self.single_accuracy() * 100.0,
            self.multi_accuracy() * 100.0,
        )
    }
}

fn ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Evaluate `vasari why` over a labeled corpus already ingested into `store`.
pub fn evaluate(
    store: &ObjectStore,
    labels: &[Label],
    match_threshold: f64,
) -> Result<EvalReport, VasariError> {
    // Dedup on (file, line) so a corpus can't pad `n` past the gate threshold
    // with duplicate labels (e.g. two label files covering the same line).
    let mut seen = HashSet::new();
    let labels: Vec<&Label> = labels
        .iter()
        .filter(|l| seen.insert((l.file.as_str(), l.line)))
        .collect();

    let mut rows = Vec::with_capacity(labels.len());
    let (mut precision_correct, mut recall_correct) = (0u64, 0u64);
    let (mut single_total, mut single_correct) = (0u64, 0u64);
    let (mut multi_total, mut multi_correct) = (0u64, 0u64);

    for label in &labels {
        let multi = label.expected_intent.len() > 1;

        // One lookup per label. Sort by confidence descending (stable) to match
        // `resolve::why`'s tie-break, so the first above-threshold chain is the
        // same "top pick" `why` would return; the rest feed recall.
        let mut chains = why_all(store, &label.file, label.line)?;
        chains.sort_by(|a, b| {
            b.confidence()
                .partial_cmp(&a.confidence())
                .unwrap_or(std::cmp::Ordering::Less)
        });
        let above: Vec<_> = chains
            .iter()
            .filter(|c| c.confidence() >= label.confidence_threshold)
            .collect();

        // Top pick (precision) and all above-threshold intents (recall).
        let top = above
            .first()
            .and_then(|c| c.primary_intent().map(|i| i.text.clone()));
        let all_texts: Vec<String> = above
            .iter()
            .filter_map(|c| c.primary_intent().map(|i| i.text.clone()))
            .collect();

        let matches_any = |cand: &str| -> bool {
            label
                .expected_intent
                .iter()
                .any(|e| intent_matches(e, cand, match_threshold))
        };

        let outcome = if label.expected_intent.is_empty() {
            match top {
                None => Outcome::NoAnswerCorrect,
                Some(_) => Outcome::Wrong,
            }
        } else if let Some(t) = &top {
            if matches_any(t) {
                Outcome::PrecisionHit
            } else if all_texts.iter().any(|c| matches_any(c)) {
                Outcome::RecallOnly
            } else {
                Outcome::Wrong
            }
        } else {
            Outcome::NoAnswerMiss
        };

        if outcome.precision_ok() {
            precision_correct += 1;
        }
        if outcome.recall_ok() {
            recall_correct += 1;
        }
        if multi {
            multi_total += 1;
            if outcome.precision_ok() {
                multi_correct += 1;
            }
        } else {
            single_total += 1;
            if outcome.precision_ok() {
                single_correct += 1;
            }
        }

        rows.push(Row {
            file: label.file.clone(),
            line: label.line,
            expected: label.expected_intent.clone(),
            got: top,
            multi_intent: multi,
            outcome,
        });
    }

    Ok(EvalReport {
        rows,
        n: labels.len() as u64,
        precision_correct,
        recall_correct,
        single_total,
        single_correct,
        multi_total,
        multi_correct,
        match_threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 0.005, "expected {b}, got {a}");
    }

    #[test]
    fn wilson_matches_known_vectors() {
        // Plan test vectors.
        approx(wilson_lower_bound(8, 10, Z_95), 0.490);
        approx(wilson_lower_bound(80, 100, Z_95), 0.711);
        // A perfect pilot still can't clear the 0.70 floor until n grows.
        assert!(wilson_lower_bound(10, 10, Z_95) < 0.75);
        // n=0 is defined as 0.0, not NaN.
        approx(wilson_lower_bound(0, 0, Z_95), 0.0);
    }

    #[test]
    fn token_overlap_not_substring() {
        // Covered tokens above threshold → match.
        assert!(intent_matches(
            "add auth",
            "Add authentication: auth guard",
            0.5
        ));
        // A single short token that isn't present → no match (no substring inflation).
        assert!(!intent_matches(
            "auth",
            "refactor the logging pipeline",
            0.6
        ));
        // Exact-ish match.
        assert!(intent_matches(
            "add rate limit middleware",
            "Add rate limit middleware to /api",
            0.8
        ));
        // Empty expected never matches.
        assert!(!intent_matches("", "anything", 0.0));
    }

    #[test]
    fn insufficient_corpus_below_n_min() {
        let report = EvalReport {
            rows: vec![],
            n: 10,
            precision_correct: 10,
            recall_correct: 10,
            single_total: 10,
            single_correct: 10,
            multi_total: 0,
            multi_correct: 0,
            match_threshold: MATCH_THRESHOLD,
        };
        assert_eq!(report.status(), GateStatus::InsufficientCorpus);
    }

    #[test]
    fn gate_pass_and_fail_at_scale() {
        let pass = EvalReport {
            rows: vec![],
            n: 100,
            precision_correct: 85,
            recall_correct: 90,
            single_total: 100,
            single_correct: 85,
            multi_total: 0,
            multi_correct: 0,
            match_threshold: MATCH_THRESHOLD,
        };
        assert_eq!(pass.status(), GateStatus::Pass);

        let fail = EvalReport {
            precision_correct: 60,
            ..pass.clone()
        };
        assert_eq!(fail.status(), GateStatus::Fail);
    }
}
