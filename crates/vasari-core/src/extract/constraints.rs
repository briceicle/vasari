use once_cell::sync::Lazy;
use regex::Regex;

use crate::schema::{Constraint, ConstraintPolarity, NodeId};

// Sentence boundary: period/bang/question followed by whitespace, or newline.
static SENTENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.!?]\s+|\n+").unwrap());

const MANDATORY_KEYWORDS: &[&str] = &[
    "must ", "must\t", "shall ", "always ", "required", "need to",
];

const PROHIBITIVE_KEYWORDS: &[&str] = &[
    "must not",
    "must-not",
    "mustn't",
    "should not",
    "should-not",
    "shouldn't",
    "never ",
    "never\t",
    "prohibited",
    "forbidden",
    "do not",
    "don't",
];

/// Extract constraint nodes from `text`, derived from `derived_from`.
///
/// Splits on sentence boundaries, detects polarity from keywords, and skips
/// sentences that are too short to be meaningful constraints (< 3 words).
pub fn extract_constraints(
    text: &str,
    derived_from: NodeId,
    parent_ids: Vec<NodeId>,
) -> Vec<Constraint> {
    let mut constraints = Vec::new();

    for sentence in split_sentences(text) {
        let trimmed = sentence.trim();
        if trimmed.split_whitespace().count() < 3 {
            continue;
        }
        let lower = trimmed.to_lowercase();

        let polarity = if PROHIBITIVE_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            ConstraintPolarity::Prohibitive
        } else if MANDATORY_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            ConstraintPolarity::Mandatory
        } else {
            continue;
        };

        constraints.push(Constraint::new(
            trimmed.to_string(),
            derived_from.clone(),
            polarity,
            parent_ids.clone(),
        ));
    }

    constraints
}

fn split_sentences(text: &str) -> Vec<&str> {
    SENTENCE_RE.split(text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::NodeId;

    fn dummy_id() -> NodeId {
        NodeId("deadbeef".repeat(8))
    }

    #[test]
    fn extracts_mandatory() {
        let text = "You must validate all inputs. Always sanitize before writing to disk.";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert!(!cs.is_empty());
        assert!(cs
            .iter()
            .all(|c| c.polarity == ConstraintPolarity::Mandatory));
    }

    #[test]
    fn extracts_prohibitive() {
        let text = "Never commit credentials to git. You must not store tokens in plaintext.";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert_eq!(cs.len(), 2);
        assert!(cs
            .iter()
            .all(|c| c.polarity == ConstraintPolarity::Prohibitive));
    }

    #[test]
    fn skips_short_sentences() {
        let text = "Must do.";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert!(
            cs.is_empty(),
            "sentence too short to be a meaningful constraint"
        );
    }

    #[test]
    fn skips_plain_sentences() {
        let text = "The code reads a file. It returns the content as a string.";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert!(cs.is_empty());
    }

    #[test]
    fn splits_on_newline() {
        let text = "You must validate inputs\nNever log passwords";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert!(cs.len() >= 2, "newline should split into two sentences");
    }

    #[test]
    fn both_polarities_in_same_text() {
        let text = "You must validate all inputs. Never store passwords in plaintext.";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        let has_mandatory = cs
            .iter()
            .any(|c| c.polarity == ConstraintPolarity::Mandatory);
        let has_prohibitive = cs
            .iter()
            .any(|c| c.polarity == ConstraintPolarity::Prohibitive);
        assert!(has_mandatory, "should have a mandatory constraint");
        assert!(has_prohibitive, "should have a prohibitive constraint");
    }

    #[test]
    fn parent_ids_threaded_through() {
        let parent = NodeId("parent123".repeat(5));
        let text = "You must validate all user inputs before processing.";
        let cs = extract_constraints(text, dummy_id(), vec![parent.clone()]);
        assert!(!cs.is_empty());
        assert_eq!(cs[0].parent_ids, vec![parent]);
    }

    #[test]
    fn derived_from_is_correct() {
        let derived = dummy_id();
        let text = "Always sanitize database queries.";
        let cs = extract_constraints(text, derived.clone(), vec![]);
        assert!(!cs.is_empty());
        assert_eq!(cs[0].derived_from, derived);
    }

    #[test]
    fn sentence_at_eof_without_trailing_whitespace_is_extracted() {
        // The sentence-split regex matches `[.!?]\s+` — a sentence ending at EOF
        // (no trailing whitespace after the period) must still be captured.
        let text = "You must validate all inputs before writing";
        let cs = extract_constraints(text, dummy_id(), vec![]);
        assert_eq!(
            cs.len(),
            1,
            "sentence without trailing period should produce one constraint"
        );
    }
}
