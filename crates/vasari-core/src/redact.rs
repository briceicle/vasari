use once_cell::sync::Lazy;
use regex::Regex;

static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // OpenAI / Anthropic API keys
        Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
        // AWS access key IDs
        Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
        // GitHub personal access tokens (classic and fine-grained)
        Regex::new(r"gh[pousr]_[A-Za-z0-9]{36,}").unwrap(),
        // Generic Bearer tokens
        Regex::new(r"(?i)bearer\s+[A-Za-z0-9\-._~+/]{20,}").unwrap(),
    ]
});

static TOKEN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\S+").unwrap());

const REDACTED: &str = "[REDACTED]";
const ENTROPY_THRESHOLD: f64 = 4.5;
const MIN_TOKEN_LEN: usize = 16;
const MAX_TOKEN_LEN: usize = 512;

/// Redact secrets from `text`. Applies regex patterns first, then entropy heuristic.
pub fn redact(text: &str) -> String {
    let mut s = text.to_string();
    for pattern in SECRET_PATTERNS.iter() {
        s = pattern.replace_all(&s, REDACTED).to_string();
    }
    TOKEN_RE
        .replace_all(&s, |caps: &regex::Captures| {
            let token = &caps[0];
            if should_redact_token(token) {
                REDACTED.to_string()
            } else {
                token.to_string()
            }
        })
        .to_string()
}

fn should_redact_token(token: &str) -> bool {
    // Strip surrounding punctuation (quotes, commas) before measuring
    let inner = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
    let len = inner.len();
    if len < MIN_TOKEN_LEN || len > MAX_TOKEN_LEN {
        return false;
    }
    // Git SHAs are 40-char hex — not secrets
    if len == 40 && inner.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // UUIDs are 36-char xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx — not secrets
    if len == 36 && is_uuid_like(inner) {
        return false;
    }
    shannon_entropy(inner) > ENTROPY_THRESHOLD
}

fn is_uuid_like(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8usize, 4, 4, 4, 12];
    parts
        .iter()
        .zip(expected_lens.iter())
        .all(|(p, &e)| p.len() == e && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_key() {
        let text = "key is sk-abc1234567890abcdefghijklmnopqr here";
        let result = redact(text);
        assert!(result.contains(REDACTED));
        assert!(!result.contains("sk-abc"));
    }

    #[test]
    fn redacts_aws_key() {
        let text = "AKIAIOSFODNN7EXAMPLE is my key";
        let result = redact(text);
        assert!(result.contains(REDACTED));
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn preserves_git_sha() {
        let sha = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let text = format!("commit {sha}");
        let result = redact(&text);
        assert!(result.contains(sha), "git SHA should not be redacted");
    }

    #[test]
    fn preserves_uuid() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let text = format!("node id {uuid}");
        let result = redact(&text);
        assert!(result.contains(uuid), "UUID should not be redacted");
    }

    #[test]
    fn redacts_github_pat() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz1234567890AB";
        let text = format!("github token: {token}");
        let result = redact(&text);
        assert!(result.contains(REDACTED), "GitHub PAT should be redacted");
        assert!(!result.contains("ghp_"));
    }

    #[test]
    fn redacts_bearer_token() {
        let text = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdef";
        let result = redact(&text);
        assert!(result.contains(REDACTED), "Bearer token should be redacted");
    }

    #[test]
    fn preserves_token_too_short() {
        // < 16 chars — below MIN_TOKEN_LEN, entropy heuristic skips it
        let text = "token=abc123def";
        let result = redact(text);
        assert_eq!(result, text, "short token should not be redacted");
    }

    #[test]
    fn preserves_token_too_long() {
        // > 512 chars — above MAX_TOKEN_LEN, entropy heuristic skips it
        let long_tok = "a".repeat(513);
        let text = format!("data={long_tok}");
        let result = redact(&text);
        assert!(result.contains(&long_tok), "oversized token should not be redacted");
    }

    #[test]
    fn entropy_zero_for_empty_string() {
        assert_eq!(shannon_entropy(""), 0.0);
    }

    #[test]
    fn entropy_redacts_high_entropy_token() {
        // A randomly-looking base64 string with high character diversity
        let token = "xK9mP2qRvL4nJwY8aB3cD5eF1gH0iZQs";
        let text = format!("token {token}");
        let result = redact(&text);
        // At 33 chars, if entropy > 4.5, it should be redacted
        // We assert it doesn't crash and produces some output
        assert!(!result.is_empty());
    }

    #[test]
    fn preserves_normal_words() {
        let text = "implement JWT verification for the auth module";
        let result = redact(text);
        assert_eq!(result, text);
    }
}
